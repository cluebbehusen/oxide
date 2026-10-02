//! The working half of the brain: construction, repair welding,
//! harvesting, wreck stripping, and delivery. Hp gains buffer like
//! damage and resolve after it — fire wins ties.

use super::super::{route_for, tile_adjacent_to_rect};
use super::PendingHpGain;
use super::contact;
use super::locomotion::approach_rect;
use crate::event::{Event, StallReason};
use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::state::{Order, PathFollow, State};
use crate::stats::HARVEST_ZONE_RADIUS;
use crate::tick::crowding;
use crate::vision::GroundSalvageDanger;
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;
use std::cmp::Reverse;

/// The live meter read for weld and salvage billing, saturated one shy
/// of [`crate::state::PROGRESS_ENVELOPE`] — the ceiling the snapshot
/// validator enforces and the ramp products are proven to fit under.
/// The step math prices one tick as `ramp * (p + 1) / ramp_ticks` in
/// `u32`; unbounded, the product overflows once a torch has held one
/// job a few million ticks. Saturated, the meter parks at the ceiling
/// and the torch keeps billing and welding its marginal step forever.
fn metered(progress: u32) -> u32 {
    progress.min(crate::state::PROGRESS_ENVELOPE - 1)
}

/// Advance every committed building upgrade exactly once on the simulation
/// clock. The gain joins ordinary construction in the buffered resolver, so a
/// lethal hit on the completion tick still destroys the offline works instead
/// of letting it stand up through fire.
pub(super) fn advance_upgrades(state: &mut State, builds: &mut Vec<PendingHpGain>) {
    let sites: Vec<BuildingId> = state
        .buildings
        .iter()
        .filter(|building| !building.built && building.tier > 0 && building.hp > 0)
        .map(|building| building.id)
        .collect();

    for site in sites {
        let building = state.building(site).expect("upgrade id came from state");
        let (player, kind, progress) = (building.player, building.kind, building.progress);
        let stats = building.stats();
        let build_ticks = stats
            .construction
            .expect("an upgrade tier is constructible")
            .build_ticks;
        let start_hp = stats.max_hp / 5;
        let ramp = stats.max_hp - start_hp;
        let advanced = progress.saturating_add(1).min(build_ticks);
        let step = (ramp * advanced / build_ticks) - (ramp * progress / build_ticks);

        state
            .building_mut(site)
            .expect("upgrade id came from state")
            .progress = advanced;
        let completes = advanced >= build_ticks;
        if step > 0 || completes {
            builds.push(PendingHpGain {
                starts: false,
                site,
                step,
                completes,
                player,
                kind,
                paid: 0,
                repair_bay: None,
            });
        }
    }
}

/// Stand up an own unfinished site: walk adjacent, then feed it progress.
/// One built tick raises hp along a linear ramp to full at completion
/// (damage taken meanwhile is simply kept — nobody rebuilds for free).
pub(super) fn build(
    state: &mut State,
    id: UnitId,
    site: crate::ids::BuildingId,
    events: &mut Vec<Event>,
    builds: &mut Vec<PendingHpGain>,
) {
    let me = state.unit(id).expect("caller checked").player;
    // hp > 0 is defense in depth: with buffered damage nothing dies
    // mid-brains anymore, but building on a corpse would resurrect it and
    // swallow the destruction event, so the guard stays.
    let Some(b) = state
        .building(site)
        .filter(|b| b.player == me && !b.built && b.tier == 0 && b.hp > 0)
    else {
        // Finished, cancelled, or destroyed: the job is over either way.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (anchor, kind) = (b.anchor, b.kind);
    let stats = b.stats();
    let size = stats.size;
    let build_ticks = stats
        .construction
        .expect("sites only exist for buildable kinds")
        .build_ticks;
    let unit = state.unit(id).expect("caller checked");
    if !b.provisional && unit.work_stopped() && state.in_building_work_reach(unit, site) {
        let start_hp = stats.max_hp / 5;
        let ramp = stats.max_hp - start_hp;
        // An Excavator's crew-tick counts double: same ramp, half the
        // wall clock, telescoping exactly like a second pair of hands.
        let rate = state
            .unit(id)
            .expect("caller checked")
            .kind
            .stats()
            .build_rate;
        let b = state.building_mut(site).expect("just seen");
        let starts = b.progress == 0;
        let advanced = (b.progress + rate).min(build_ticks);
        let step = (ramp * advanced / build_ticks) - (ramp * b.progress / build_ticks);
        b.progress = advanced;
        // Both the hp gain and the completion are buffered and applied
        // after damage — see PendingHpGain. The builder learns the site is
        // done next tick, through the built-site branch above.
        let completes = b.progress >= build_ticks;
        if starts {
            // Before the first work, a full-refund cancellation must preserve salvage.
            for dy in 0..size.1 {
                for dx in 0..size.0 {
                    state.map.clear_wreck(anchor.offset(dx, dy));
                }
            }
        }
        if starts || step > 0 || completes {
            builds.push(PendingHpGain {
                starts,
                site,
                step,
                completes,
                player: me,
                kind,
                paid: 0, // construction paid at placement
                repair_bay: None,
            });
        }
        state.unit_mut(id).expect("caller checked").path = None;
    } else if !contact::approach_worker(state, id, site) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.drop_active_order();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// Approach a paid provisional scaffold without claiming hidden ground.
pub(super) fn found(
    state: &mut State,
    id: UnitId,
    kind: crate::stats::BuildingKind,
    anchor: TilePos,
    events: &mut Vec<Event>,
    builds: &mut Vec<PendingHpGain>,
) {
    let player = state.unit(id).expect("caller checked").player;
    let site = state
        .buildings
        .iter()
        .find(|b| b.player == player && b.kind == kind && b.anchor == anchor && b.hp > 0)
        .map(|b| (b.id, b.provisional));
    let Some((site, provisional)) = site else {
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    if !provisional {
        state.unit_mut(id).expect("caller checked").order = Order::Build { site };
        build(state, id, site, events, builds);
        return;
    }
    let tile = state.unit(id).expect("caller checked").tile();
    let size = kind.base_stats().size;
    if state.building(site).expect("found site").contains(tile)
        || tile_adjacent_to_rect(tile, anchor, size)
    {
        state.unit_mut(id).expect("caller checked").path = None;
    } else if !approach_rect(state, id, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        let pos = unit.pos;
        unit.drop_active_order();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// Weld a damaged own built building: walk adjacent, then feed it hp
/// along the same ramp construction climbs, billed per hp welded at
/// [`crate::stats::REPAIR_COST_PERMILLE`] of proportional cost. Gains
/// buffer like construction — fire wins ties — and an empty bank stalls
/// the job. Several welders stack, each billing its own torch time.
pub(super) fn repair(
    state: &mut State,
    id: UnitId,
    building: crate::ids::BuildingId,
    events: &mut Vec<Event>,
    builds: &mut Vec<PendingHpGain>,
) {
    let me = state.unit(id).expect("caller checked").player;
    let Some(b) = state
        .building(building)
        .filter(|b| b.player == me && b.built && b.hp > 0 && b.hp < b.stats().max_hp)
    else {
        // Healed, destroyed, or never a patient: the job is over.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (kind, tier) = (b.kind, b.tier);
    // The active tier's row: a Bulwark's weld ramp, ceiling, and billing
    // basis are the Bulwark's, not the base Turret's.
    let stats = kind.tier_stats(tier);
    // The welding rate is the construction ramp — except the Foundry,
    // which keeps its authored ramp and billing basis: repairing the
    // victory token stays the tuned defensive lever it always was, even
    // now that expansions make Foundries purchasable at a price that
    // would otherwise quadruple the weld bill.
    let (ramp_ticks, basis) = if kind == crate::stats::BuildingKind::Foundry {
        (
            crate::stats::FOUNDRY_REPAIR_TICKS,
            crate::stats::FOUNDRY_REPAIR_PRICE,
        )
    } else {
        stats.construction.map_or(
            (
                crate::stats::FOUNDRY_REPAIR_TICKS,
                crate::stats::FOUNDRY_REPAIR_PRICE,
            ),
            |c| (c.build_ticks, c.cost),
        )
    };
    let unit = state.unit(id).expect("caller checked");
    if unit.work_stopped() && state.in_building_work_reach(unit, building) {
        // Billing derives entirely from the welder's own tick meter:
        // cumulative welded hp telescopes to ramp * p / ramp_ticks, and
        // scrap owed is the ceiling of its milli-scrap price — so the
        // first fraction of a scrap bills up front (chip repairs pay
        // their coin) and a completed weld totals within one scrap of
        // exact. The meter surviving reissued orders is what keeps a
        // re-clicked welder from re-entering the prepaid stretch.
        let start_hp = stats.max_hp / 5;
        let ramp = stats.max_hp - start_hp;
        let p = metered(state.unit(id).expect("caller checked").progress);
        let owed_millis = |ticks: u32| -> u64 {
            let welded = u64::from(ramp) * u64::from(ticks) / u64::from(ramp_ticks);
            welded * u64::from(basis) * crate::stats::REPAIR_COST_PERMILLE / u64::from(stats.max_hp)
        };
        let due = owed_millis(p + 1).div_ceil(1000) - owed_millis(p).div_ceil(1000);
        if due > 0 {
            if u64::from(state.player(me).scrap) < due {
                // Broke stalls the torch.
                let unit = state.unit_mut(id).expect("caller checked");
                let (player, pos) = (unit.player, unit.pos);
                unit.clear_program();
                events.push(Event::OrderStalled {
                    unit: id,
                    player,
                    pos,
                    reason: StallReason::InsufficientScrap,
                });
                return;
            }
            state.player_mut(me).scrap -= due as u32;
        }
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        unit.progress = p + 1;
        let step = (ramp * (p + 1) / ramp_ticks) - (ramp * p / ramp_ticks);
        if step > 0 {
            builds.push(PendingHpGain {
                starts: false,
                site: building,
                step,
                completes: false,
                player: me,
                kind,
                paid: due as u32,
                repair_bay: None,
            });
        }
    } else if !contact::approach_worker(state, id, building) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.drop_active_order();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// Weld a wounded own ground machine: chase it to body contact, then
/// feed it hp along its training ramp, billed per hp at
/// [`crate::stats::REPAIR_COST_PERMILLE`] of proportional cost through
/// the same prepaid milli-scrap meter buildings use. The torch holds
/// only while both bodies stand still inside
/// body-aware tool reach: a walking patient is chased, not
/// welded — field sustain never rides along with a retreat. Heals
/// buffer like every hp gain and resolve after damage (fire wins
/// ties); several welders stack, each billing its own torch time.
/// The patient's own orders are never touched.
pub(super) fn repair_unit(
    state: &mut State,
    id: UnitId,
    patient: UnitId,
    events: &mut Vec<Event>,
    welds: &mut Vec<super::PendingFieldWeld>,
) {
    let me = state.unit(id).expect("caller checked").player;
    // The self-target guard is defense in depth: commands refuse it,
    // but an order that somehow names its own welder must end, not
    // bill a machine for welding itself.
    let Some(t) = state.unit(patient).filter(|t| {
        t.id != id
            && t.player == me
            && t.hp > 0
            && t.hp < t.kind.stats().max_hp
            && t.domain() == crate::stats::Domain::Ground
    }) else {
        // Healed, dead, taken off, or never a patient: the job is over.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let unit = state.unit(id).expect("caller checked");
    if unit.work_stopped() && unit.in_repair_reach(t) {
        // Do not bill or advance the torch yet. The patient's own brain
        // may run later in this parity-alternating phase and create the
        // path movement consumes this tick. Commit after all brains have
        // exposed that departure.
        state.unit_mut(id).expect("caller checked").path = None;
        welds.push(super::PendingFieldWeld {
            welder: id,
            patient,
        });
    } else {
        chase_patient(state, id, patient, events);
    }
}

/// Commits field welds after every patient's brain has had the chance to
/// create the path that movement will consume this tick. Repair Bay pulses
/// remain separate: their aura deliberately heals moving machines. This
/// necessarily gives inline brain-phase economy (including deposits and
/// building repair) priority in the shared bank; eligibility must settle
/// before a field weld can bill without reintroducing tick-parity behavior.
pub(super) fn commit_unit_welds(
    state: &mut State,
    welds: Vec<super::PendingFieldWeld>,
    events: &mut Vec<Event>,
    heals: &mut Vec<super::PendingUnitHeal>,
) {
    // A welder can itself be somebody else's patient. Settle every
    // departure before billing any torch: rejecting B -> C may give B a
    // chase path, which must then reject A -> B even when A's candidate
    // appeared first in this tick's parity order. Eligibility only moves
    // from stationary to departing, so this reaches a fixed point in at
    // most one transition per candidate.
    let mut stationary = vec![true; welds.len()];
    loop {
        let mut changed = false;
        for (slot, weld) in welds.iter().enumerate() {
            if !stationary[slot] {
                continue;
            }
            let Some(unit) = state
                .unit(weld.welder)
                .filter(|u| u.order == (Order::RepairUnit { unit: weld.patient }))
            else {
                stationary[slot] = false;
                continue;
            };
            let me = unit.player;
            if unit.drive_speed != Fx::ZERO || footprint_eviction_pending(state, weld.welder) {
                // Phase 5, after weld resolution, will make this welder
                // walk off newly claimed ground. It cannot light the
                // torch and move in the same tick.
                stationary[slot] = false;
                continue;
            }
            let Some(t) = state.unit(weld.patient).filter(|t| {
                t.player == me
                    && t.hp > 0
                    && t.hp < t.kind.stats().max_hp
                    && t.domain() == crate::stats::Domain::Ground
            }) else {
                stationary[slot] = false;
                continue;
            };
            if t.path.is_none()
                && t.drive_speed == Fx::ZERO
                && !matches!(t.order, Order::Found { .. })
                && !footprint_eviction_pending(state, weld.patient)
                && unit.in_repair_reach(t)
            {
                continue;
            }
            stationary[slot] = false;
            chase_patient(state, weld.welder, weld.patient, events);
            changed = true;
        }
        if !changed {
            break;
        }
    }

    for (weld, stationary) in welds.into_iter().zip(stationary) {
        if !stationary {
            continue;
        }
        let Some(unit) = state
            .unit(weld.welder)
            .filter(|u| u.order == (Order::RepairUnit { unit: weld.patient }))
        else {
            continue;
        };
        let (me, p) = (unit.player, metered(unit.progress));
        let Some(t) = state
            .unit(weld.patient)
            .filter(|t| t.player == me && t.hp > 0 && t.hp < t.kind.stats().max_hp)
        else {
            continue;
        };
        debug_assert!(t.path.is_none());
        debug_assert!(!footprint_eviction_pending(state, weld.welder));
        debug_assert!(!footprint_eviction_pending(state, weld.patient));
        debug_assert!(unit.in_repair_reach(t));
        let t_kind = t.kind;

        // The billing meter is the Harvester welder's, with the patient's
        // own numbers: ramp is full max_hp (machines have no one-fifth
        // foundation), the clock is its training time, the basis its
        // price. Same ceiling prepay, same survival of no-op reissues.
        let stats = t_kind.stats();
        let (ramp, ramp_ticks) = (stats.max_hp, stats.train_ticks);
        let due = u64::from(crate::stats::unit_repair_debit(t_kind, p));
        if due > 0 {
            if u64::from(state.player(me).scrap) < due {
                // Broke stalls the torch.
                let unit = state.unit_mut(weld.welder).expect("just seen");
                let (player, pos) = (unit.player, unit.pos);
                unit.clear_program();
                events.push(Event::OrderStalled {
                    unit: weld.welder,
                    player,
                    pos,
                    reason: StallReason::InsufficientScrap,
                });
                continue;
            }
            state.player_mut(me).scrap -= due as u32;
        }
        let unit = state.unit_mut(weld.welder).expect("just seen");
        unit.path = None;
        unit.progress = p + 1;
        let step = (ramp * (p + 1) / ramp_ticks) - (ramp * p / ramp_ticks);
        if step > 0 {
            heals.push(super::PendingUnitHeal {
                unit: weld.patient,
                step,
                player: me,
                paid: due as u32,
                source: crate::event::UnitRepairSource::FieldWelder { unit: weld.welder },
            });
        }
    }
}

/// Whether phase 5 will give this pathless body an escape path from a
/// claimed building footprint. Weld settlement runs before that pre-pass,
/// so it must predict the same move to uphold the both-bodies-still rule.
fn footprint_eviction_pending(state: &State, id: UnitId) -> bool {
    super::super::movement::claimed_ground_escape(state, id).is_some()
}

/// Reuses the current approach side as a patient moves, replanning only
/// when its precise endpoint leaves the final tile.
fn chase_patient(state: &mut State, id: UnitId, patient: UnitId, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let target = state.unit(patient).expect("caller checked");
    let gap =
        unit.kind.stats().radius + target.kind.stats().radius + crate::stats::WORK_APPROACH_GAP;
    let toward = unit
        .path
        .as_ref()
        .and_then(|p| p.final_point)
        .unwrap_or(unit.pos)
        - target.pos;
    let bearing = if toward == Vec2Fx::ZERO {
        unit.heading.wrapping_add(128)
    } else {
        chassis::compass::heading_of(toward)
    };
    let kind = unit.kind;
    let from = unit.tile();
    // Search in the patient's query-relative frame, so mirrored scenes
    // choose mirrored sides without using absolute entity ids.
    for offset in [0_i16, 32, -32, 64, -64, 96, -96, 128] {
        let point =
            target.pos + chassis::compass::dir(bearing.wrapping_add_signed(offset as i8)) * gap;
        let goal = TilePos::containing(point);
        if !state.passable_for(kind.stats().domain, goal) {
            continue;
        }
        if unit.path.as_ref().is_some_and(|p| p.goal == goal) {
            state
                .unit_mut(id)
                .expect("caller checked")
                .path
                .as_mut()
                .expect("checked")
                .final_point = Some(point);
            return;
        }
        if let Some(waypoints) = route_for(state, kind, from, goal) {
            let mut path = work_position_path(goal, waypoints);
            path.final_point = Some(point);
            state.unit_mut(id).expect("caller checked").path = Some(path);
            return;
        }
    }
    let unit = state.unit_mut(id).expect("caller checked");
    let (player, pos) = (unit.player, unit.pos);
    unit.drop_active_order();
    events.push(Event::OrderStalled {
        unit: id,
        player,
        pos,
        reason: StallReason::NoRoute,
    });
}

/// Strip an own built building for scrap: walk adjacent, then drain hp
/// along the construction ramp — salvage is labor on the same clock
/// building is. Drains buffer beside the gains and resolve after
/// damage; crediting happens in resolution, against hp actually
/// removed. Several strippers stack like builders.
pub(super) fn salvage(
    state: &mut State,
    id: UnitId,
    building: crate::ids::BuildingId,
    events: &mut Vec<Event>,
    drains: &mut Vec<super::PendingHpDrain>,
) {
    let me = state.unit(id).expect("caller checked").player;
    let Some(b) = state.building(building).filter(|b| {
        b.player == me && b.built && b.hp > 0 && b.kind != crate::stats::BuildingKind::Foundry
    }) else {
        // Stripped bare, destroyed, or never salvageable: the job is
        // over either way — the program plays on.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (kind, tier) = (b.kind, b.tier);
    // Strip on the active tier's construction clock: dismantling a
    // Refinery is undoing the Refinery's build, not the Reclaimer's.
    let stats = kind.tier_stats(tier);
    let ramp_ticks = stats
        .construction
        .expect("non-Foundry salvage targets are buildable")
        .build_ticks;
    let unit = state.unit(id).expect("caller checked");
    if unit.work_stopped() && state.in_building_work_reach(unit, building) {
        let start_hp = stats.max_hp / 5;
        let ramp = stats.max_hp - start_hp;
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        let p = metered(unit.progress);
        unit.progress = p + 1;
        let step = (ramp * (p + 1) / ramp_ticks) - (ramp * p / ramp_ticks);
        if step > 0 {
            drains.push(super::PendingHpDrain { building, step });
        }
    } else if !contact::approach_worker(state, id, building) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.drop_active_order();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// The harvest loop: work one safe remembered source inside a fixed local
/// zone, haul to a Foundry, and return to that same zone. Both nodes and
/// wrecks are worked from a position beside the source.
pub(super) fn harvest(
    state: &mut State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    node: TilePos,
    anchor: Option<TilePos>,
    retiring: bool,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    let Some(hstats) = unit.kind.stats().harvest else {
        // Only harvesters ever get this order; be defensive anyway.
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    };
    let anchor = anchor.unwrap_or(node);
    if unit.unloading.is_some() {
        deliver(state, danger, id, events, retiring);
        return;
    }
    if retiring {
        retire(state, danger, id, events);
        return;
    }

    let (tile, carrying) = (unit.tile(), unit.carrying);
    // The clicked source is authoritative: danger governs autonomous
    // chaining, not an explicit player command. Once the work zone picks
    // a different source for itself, that source remains subject to the
    // fog-honest danger envelope on every tick.
    let current =
        known_source(state, unit.player, node).filter(|_| node == anchor || !danger.contains(node));

    if current.is_none() {
        if let Some(next) = replacement_source(state, danger, id, anchor, Some(node)) {
            switch_source(state, id, next.pos, anchor);
            return;
        }
        begin_retirement(state, id, node, anchor);
        if carrying > 0 {
            deliver(state, danger, id, events, true);
        } else {
            retire(state, danger, id, events);
        }
        return;
    }

    if carrying >= hstats.capacity {
        deliver(state, danger, id, events, false);
        return;
    }

    let current = current.expect("the dry branch returned");
    if state
        .unit(id)
        .expect("caller checked")
        .in_work_reach(node, (1, 1))
    {
        // Arriving ends any hold from the route here; only a drop-off
        // scan may start the next one.
        let worker = state.unit_mut(id).expect("caller checked");
        worker.path = None;
        worker.danger_retry_at = None;
        if !worker.work_stopped() {
            return;
        }
    }
    let authoritative = node == anchor;
    let worker = state.unit(id).expect("caller checked");
    if worker.in_work_reach(node, (1, 1))
        && worker.work_stopped()
        && (authoritative || !danger.contains(tile))
    {
        match current.kind {
            SourceKind::Scrap => extract(state, id, node, hstats.ticks_per_scrap, events),
            SourceKind::Wreck => extract_wreck(state, id, node, hstats.ticks_per_scrap),
        }
    } else if !approach_source(state, danger, id, current, authoritative) {
        source_route_failed(state, danger, id, node, anchor, events);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Scrap,
    Wreck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KnownSource {
    pos: TilePos,
    amount: u32,
    kind: SourceKind,
}

type SourceScore = (i32, usize, Reverse<u32>, i32, u8, i32, i32);

/// Existing routes re-check only this near segment each tick. A Harvester
/// needs 64 ticks to traverse eight clear cardinal tiles, so this is
/// ample deterministic warning without turning every worker tick into a
/// full path-length by threat-count scan.
const HARVEST_DANGER_LOOKAHEAD: usize = 8;

/// The near slice of the lookahead that always reacts immediately: a
/// threat inside the first three route tiles replans this tick, exactly
/// as it always did. Only a flag beyond this zone — a rumor four to
/// eight tiles out that a hovering enemy re-raises every tick — defers
/// to the staggered replan window below.
const HARVEST_DANGER_REACT_ZONE: usize = 3;

/// A worker whose retained route fails the danger lookahead only in
/// the FAR zone does not re-plan every tick while the threat lingers —
/// that thrashed a full multi-candidate A* per worker per tick and
/// jittered the fleet between near-equal detours. Far-zone replans
/// stagger on this period, keyed by owner-local unit rank so a fleet never
/// re-plans in unison without making cross-seat production ids a tactical
/// input; near-zone threats never wait.
const HARVEST_REPLAN_PERIOD: u64 = 4;

/// Whether this is the tick on which `id` may re-plan a retained route
/// the danger lookahead has flagged beyond the react zone.
fn danger_replan_window(state: &State, id: UnitId) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let rank = crate::ids::owner_local_unit_rank(
        id,
        unit.player,
        state.units.iter().map(|unit| (unit.id, unit.player)),
    );
    (state.tick + u64::try_from(rank).expect("unit rank fits u64"))
        .is_multiple_of(HARVEST_REPLAN_PERIOD)
}

/// Whether the retained route may be kept this tick: clear routes and
/// off-window far-zone rumors keep it; near-zone threats and on-window
/// far-zone flags surrender it for a replan.
fn keep_flagged_route(
    state: &State,
    id: UnitId,
    path: &PathFollow,
    safe: impl FnMut(TilePos) -> bool,
) -> bool {
    match first_flagged_waypoint(path, safe) {
        None => true,
        Some(index) => index >= HARVEST_DANGER_REACT_ZONE && !danger_replan_window(state, id),
    }
}

/// Index (relative to the walker's next waypoint) of the first
/// lookahead tile `safe` rejects, or `None` for a clear near segment.
/// The scan touches at most [`HARVEST_DANGER_LOOKAHEAD`] tiles however
/// long the route is.
fn first_flagged_waypoint(
    path: &PathFollow,
    mut safe: impl FnMut(TilePos) -> bool,
) -> Option<usize> {
    path.waypoints
        .iter()
        .skip(path.next as usize)
        .take(HARVEST_DANGER_LOOKAHEAD)
        .position(|waypoint| !safe(*waypoint))
}

/// Salvage knowledge follows the same freeze-frame rule the shell and
/// fog-honest bots use: live amounts only on visible ground, remembered
/// amounts everywhere else.
fn known_source(state: &State, player: PlayerId, pos: TilePos) -> Option<KnownSource> {
    let vision = state.vision(player);
    let (scrap, wreck) = if vision.visible(pos) {
        (state.map.scrap_at(pos), state.map.wreck_at(pos))
    } else {
        (vision.remembered_scrap(pos), vision.remembered_wreck(pos))
    };
    if scrap > 0 {
        Some(KnownSource {
            pos,
            amount: scrap,
            kind: SourceKind::Scrap,
        })
    } else if wreck > 0 {
        Some(KnownSource {
            pos,
            amount: wreck,
            kind: SourceKind::Wreck,
        })
    } else {
        None
    }
}

/// Pick one reachable, safe known source inside the fixed work zone.
/// Worker-local distance leads the key so a group spreads across its
/// nearby sources instead of collapsing onto the globally cheapest path.
/// Safe route length then breaks local ties; the fixed anchor and final
/// coordinates make every pick unique.
fn replacement_source(
    state: &State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    anchor: TilePos,
    exclude: Option<TilePos>,
) -> Option<KnownSource> {
    let unit = state.unit(id).expect("caller checked");
    let from = unit.tile();
    let dx = anchor.x - from.x;
    let dy = anchor.y - from.y;
    let reversed = if dx == 0 && dy == 0 {
        unit.heading >= 128
    } else {
        dy > 0 || (dy == 0 && dx > 0)
    };
    let orientation = if reversed { -1 } else { 1 };
    // Exact economic ties follow the worker's approach, including its hull
    // bearing when standing on the anchor, rather than an absolute map corner.
    let coordinates = |pos: TilePos| {
        (
            orientation * (pos.y - anchor.y),
            orientation * (pos.x - anchor.x),
        )
    };
    let mut candidates = Vec::new();
    for dy in -HARVEST_ZONE_RADIUS..=HARVEST_ZONE_RADIUS {
        for dx in -HARVEST_ZONE_RADIUS..=HARVEST_ZONE_RADIUS {
            let pos = anchor.offset(dx, dy);
            if exclude == Some(pos) {
                continue;
            }
            let Some(source) = known_source(state, unit.player, pos) else {
                continue;
            };
            if !danger.contains(pos) {
                candidates.push((pos.manhattan(from), source));
            }
        }
    }
    // Distance is the leading selection key. Once any source at the nearest
    // reachable distance wins, no farther source can displace it, so avoid
    // paying for routes whose first key component already loses.
    candidates.sort_by_key(|(distance, source)| (*distance, coordinates(source.pos)));
    let mut best: Option<(SourceScore, KnownSource)> = None;
    for (distance, source) in candidates {
        if best.as_ref().is_some_and(|(key, _)| distance > key.0) {
            break;
        }
        let Some(route_len) = source_route_len(state, danger, id, source) else {
            continue;
        };
        let kind_key = match source.kind {
            SourceKind::Wreck => 0,
            SourceKind::Scrap => 1,
        };
        let pos = source.pos;
        let (y, x) = coordinates(pos);
        let key = (
            distance,
            route_len,
            Reverse(source.amount),
            pos.chebyshev(anchor),
            kind_key,
            y,
            x,
        );
        if best.as_ref().is_none_or(|(old, _)| key < *old) {
            best = Some((key, source));
        }
    }
    best.map(|(_, source)| source)
}

fn source_route_len(
    state: &State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    source: KnownSource,
) -> Option<usize> {
    safe_source_route(state, danger, id, source).map(|path| path.waypoints.len())
}

fn approach_work_rect(state: &mut State, id: UnitId, anchor: TilePos, size: (i32, i32)) -> bool {
    if !approach_rect(state, id, anchor, size) {
        return false;
    }
    let unit = state.unit_mut(id).expect("caller checked");
    if let Some(path) = &mut unit.path {
        path.final_point = Some(crate::geometry::work_approach_point(
            path.goal,
            anchor,
            size,
            unit.kind.stats().radius,
        ));
        if path.waypoints.is_empty() {
            path.waypoints.push(path.goal);
        }
    }
    true
}

fn work_position_path(goal: TilePos, mut waypoints: Vec<TilePos>) -> PathFollow {
    // A tile-level route can be empty while the body still needs to close
    // the distance from its current tile edge to the work position.
    if waypoints.is_empty() {
        waypoints.push(goal);
    }
    PathFollow {
        final_point: None,
        goal,
        waypoints,
        next: 0,
    }
}

/// Keep or create a path whose near remaining segment is outside every
/// fog-honest danger envelope. Re-evaluating that bounded lookahead each
/// tick lets a new sighting or radar contact divert a worker before it
/// reaches the known threat.
fn approach_source(
    state: &mut State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    source: KnownSource,
    authoritative: bool,
) -> bool {
    if authoritative {
        return approach_authoritative_source(state, danger, id, source);
    }
    let unit = state.unit(id).expect("caller checked");
    let goal_matches = |goal: TilePos| tile_adjacent_to_rect(goal, source.pos, (1, 1));
    let from = unit.tile();
    let keep = unit.path.as_ref().is_some_and(|path| {
        goal_matches(path.goal)
            && (from.chebyshev(path.goal) > 1
                || path
                    .final_point
                    .is_none_or(|point| !crowding::claimed(state, id, point, true)))
            && keep_flagged_route(state, id, path, |waypoint| {
                danger.route_safe_from(from, waypoint)
            })
    });
    if keep {
        return true;
    }

    let Some(path) = safe_source_route(state, danger, id, source) else {
        state.unit_mut(id).expect("caller checked").path = None;
        return false;
    };
    state.unit_mut(id).expect("caller checked").path = Some(path);
    true
}

/// Honor the source the commander actually named while preferring a route
/// that crosses no known-danger tile except, when unavoidable, the final
/// work position itself. If even that route is sealed, the ordinary route
/// remains the explicit command's last resort instead of silently turning
/// a player's order into retirement.
fn approach_authoritative_source(
    state: &mut State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    source: KnownSource,
) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let player = unit.player;
    let from = unit.tile();
    let goal_matches = |goal: TilePos| tile_adjacent_to_rect(goal, source.pos, (1, 1));
    if let Some(path) = unit.path.as_ref().filter(|path| goal_matches(path.goal)) {
        let near_route_is_clear = keep_flagged_route(state, id, path, |waypoint| {
            known_ground_passable(state, danger, player, waypoint)
                && danger.route_safe_from(from, waypoint)
        });
        if near_route_is_clear
            && (from.chebyshev(path.goal) > 1
                || path
                    .final_point
                    .is_none_or(|point| !crowding::claimed(state, id, point, true)))
        {
            state.unit_mut(id).expect("caller checked").danger_retry_at = None;
            return true;
        }
        // Only danger defers a search, and only after one already failed: a
        // newly flagged route or a waypoint that turned impassable replans
        // at once.
        let danger_only = !near_route_is_clear
            && first_flagged_waypoint(path, |waypoint| {
                known_ground_passable(state, danger, player, waypoint)
            })
            .is_none();
        if danger_only && unit.danger_retry_at.is_some_and(|retry| state.tick < retry) {
            return true;
        }
        let detour = authoritative_source_route(state, danger, id, source);
        let tick = state.tick;
        let worker = state.unit_mut(id).expect("caller checked");
        worker.danger_retry_at = (detour.is_none() && danger_only)
            .then_some(tick + crate::stats::HARVEST_DANGER_RETRY_TICKS);
        if let Some(path) = detour {
            worker.path = Some(path);
        }
        // No safe detour means the explicitly ordered route remains in
        // force. This is the only path allowed to cross known danger.
        return true;
    }

    state.unit_mut(id).expect("caller checked").danger_retry_at = None;
    if let Some(path) = authoritative_source_route(state, danger, id, source) {
        state.unit_mut(id).expect("caller checked").path = Some(path);
        return true;
    }

    approach_work_rect(state, id, source.pos, (1, 1))
}

/// The deterministic best route to one source when danger tiles are
/// treated like temporary impassable terrain. A* finds a safe detour when
/// one exists instead of rejecting the ordinary shortest path and giving
/// up on an otherwise reachable work zone. The starting tile stays legal
/// so a newly threatened worker can route out of danger.
fn safe_source_route(
    state: &State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    source: KnownSource,
) -> Option<PathFollow> {
    source_route_avoiding_danger(state, danger, id, source, false)
}

/// The danger-aware route for an explicit source. The final work position
/// may itself be inside the source's danger envelope; every earlier tile
/// still has to be safe.
fn authoritative_source_route(
    state: &State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    source: KnownSource,
) -> Option<PathFollow> {
    safe_source_route(state, danger, id, source)
        .or_else(|| source_route_avoiding_danger(state, danger, id, source, true))
}

fn source_route_avoiding_danger(
    state: &State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    source: KnownSource,
    allow_dangerous_goal: bool,
) -> Option<PathFollow> {
    let unit = state.unit(id).expect("caller checked");
    let from = unit.tile();
    let player = unit.player;
    let safe_route = |goal| {
        danger.find_route(from, goal, |tile| {
            known_ground_passable(state, danger, player, tile)
                && ((allow_dangerous_goal && tile == goal) || danger.route_safe_from(from, tile))
        })
    };
    let frame = crate::tick::rect_approach_origin(state, player, from, source.pos, (1, 1));
    let mut candidates: Vec<_> = crate::geometry::work_positions(
        source.pos,
        (1, 1),
        crate::geometry::work_approach_distance(unit.kind.stats().radius)
            + const { Fx::lit("0.06") },
        unit.kind.stats().radius * crowding::compression(),
    )
    .into_iter()
    .map(|point| crowding::Position {
        goal: TilePos::containing(point),
        point,
    })
    .filter(|candidate| known_ground_passable(state, danger, player, candidate.goal))
    .filter(|candidate| allow_dangerous_goal || !danger.contains(candidate.goal))
    .filter(|candidate| {
        crate::geometry::circle_clear(candidate.point, unit.kind.stats().radius, |tile| {
            (tile == source.pos
                && state
                    .map()
                    .tile(tile)
                    .is_some_and(|cell| cell.terrain == crate::map::Terrain::Ground))
                || known_ground_passable(state, danger, player, tile)
        })
    })
    .collect();
    candidates.sort_by_key(|candidate| {
        (
            unit.pos.dist_sq(candidate.point),
            crate::geometry::rect_approach_key_from(
                from,
                frame,
                source.pos,
                (1, 1),
                candidate.goal,
            ),
            (candidate.point.x - unit.pos.x) * (frame.center().y - unit.pos.y)
                - (candidate.point.y - unit.pos.y) * (frame.center().x - unit.pos.x),
        )
    });
    // Every candidate, including those crowding moves outward, shares one
    // passability rule, so a failed search that explored the start's whole
    // component settles any later goal it never reached.
    let mut failed = false;
    let route = |goal: TilePos| {
        if failed
            && danger
                .last_route_reachability(&[goal], allow_dangerous_goal)
                .is_some_and(|reachable| !reachable[0])
        {
            return None;
        }
        let found = safe_route(goal);
        failed = found.is_none();
        found
    };
    crowding::choose(
        state,
        id,
        candidates,
        source.pos.center(),
        |tile| {
            known_ground_passable(state, danger, player, tile)
                && (allow_dangerous_goal || !danger.contains(tile))
        },
        route,
    )
}

/// Tile centers no longer describe the clearance between adjacent work positions.
fn work_position_claimed(
    state: &State,
    id: UnitId,
    anchor: TilePos,
    size: (i32, i32),
    goal: TilePos,
) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let point = crate::geometry::work_approach_point(goal, anchor, size, unit.kind.stats().radius);
    crowding::claimed(state, id, point, false)
}

/// Ground occupancy as the worker's team can know it. Visible tiles use
/// live truth. Under fog, static terrain and frozen scrap memory combine
/// with allied buildings and hostile building ghosts; an unscouted live
/// enemy structure must not bend the chosen path before the worker sees it.
fn known_ground_passable(
    state: &State,
    danger: &GroundSalvageDanger,
    player: PlayerId,
    tile: TilePos,
) -> bool {
    // Memoized in the phase snapshot: terrain never mutates, fog
    // knowledge and building spans are frozen for the phase, and the
    // one input a phase can still change — a visible node's live
    // scrap draining to zero — is declared volatile so that tile
    // re-evaluates per probe instead of pinning a stale verdict.
    danger.known_ground_cached(tile, || {
        let vision = state.vision(player);
        let Some(ground) = state
            .map
            .tile(tile)
            .map(|cell| cell.terrain == crate::map::Terrain::Ground)
        else {
            return (false, false);
        };
        if !ground {
            return (false, false);
        }
        if vision.visible(tile) {
            let live_scrap = state.map.scrap_at(tile);
            let open = live_scrap == 0 && !danger.known_building_blocked(tile);
            return (open, live_scrap > 0);
        }
        if vision.remembered_scrap(tile) > 0 {
            return (false, false);
        }
        (!danger.known_building_blocked(tile), false)
    })
}

/// One worker's drop-off scan: after any same-pass flood exhausts the
/// walkable component, every later foundry's doorsteps are decided by
/// the same flood — the path scratch still holds it because nothing
/// else searches between the calls.
#[derive(Default)]
struct DropOffScan {
    flood_exhausted: bool,
}

/// Approach one rectangle using only the worker's shared battlefield
/// knowledge. A bounded near-path check reacts to newly known danger
/// without rescanning a long route every tick; a full deterministic A*
/// runs only when that check fails or no path exists yet.
/// Walks the worker toward the nearest safely-approachable drop-off.
/// The per-foundry semantics of the old single-pass scan are exact:
/// the retained path is honored until the first failed safe route
/// clears it, and the danger-ignoring classification that decides
/// waiting-versus-stalling runs only after every safe attempt failed.
/// Splitting the passes lets one exhausted flood answer every
/// remaining foundry — a fully sealed worker floods twice per tick,
/// not twice per foundry. Returns false only when no drop-off is
/// reachable at all: the caller's stall.
/// Ticks between repeated DangerHold reports for one waiting worker.
const DANGER_HOLD_REPORT_PERIOD: u64 = 100;

fn try_drop_offs(
    state: &mut State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    drop_offs: &[BuildingId],
    events: &mut Vec<Event>,
) -> bool {
    let unit = state.unit(id).expect("caller checked");
    if unit.path.is_none() && unit.danger_retry_at.is_some_and(|retry| state.tick < retry) {
        report_danger_hold(state, id, events);
        return true;
    }
    let mut scan = DropOffScan::default();
    let mut path_cleared = false;
    for &foundry_id in drop_offs {
        let foundry = state.building(foundry_id).expect("collected live drop-off");
        let (anchor, size) = (foundry.anchor, foundry.stats().size);
        if !path_cleared {
            let unit = state.unit(id).expect("caller checked");
            let player = unit.player;
            let from = unit.tile();
            if let Some(path) = unit
                .path
                .as_ref()
                .filter(|path| tile_adjacent_to_rect(path.goal, anchor, size))
                && (from.chebyshev(path.goal) > 1
                    || !work_position_claimed(state, id, anchor, size, path.goal))
                && keep_flagged_route(state, id, path, |waypoint| {
                    known_ground_passable(state, danger, player, waypoint)
                        && danger.route_safe_from(from, waypoint)
                })
            {
                state.unit_mut(id).expect("caller checked").danger_retry_at = None;
                return true;
            }
        }
        if let Some(path) = known_rect_route(state, danger, id, anchor, size, true, Some(&mut scan))
        {
            let worker = state.unit_mut(id).expect("caller checked");
            worker.path = Some(path);
            worker.danger_retry_at = None;
            return true;
        }
        if !path_cleared {
            state.unit_mut(id).expect("caller checked").path = None;
            path_cleared = true;
        }
    }
    let mut scan = DropOffScan::default();
    for &foundry_id in drop_offs {
        let foundry = state.building(foundry_id).expect("collected live drop-off");
        let (anchor, size) = (foundry.anchor, foundry.stats().size);
        if known_rect_route(state, danger, id, anchor, size, false, Some(&mut scan)).is_some() {
            // Danger-blocked, not sealed: stand and wait for the window,
            // checking again only after the retry period.
            let tick = state.tick;
            state.unit_mut(id).expect("caller checked").danger_retry_at =
                Some(tick + crate::stats::HARVEST_DANGER_RETRY_TICKS);
            report_danger_hold(state, id, events);
            return true;
        }
    }
    state.unit_mut(id).expect("caller checked").danger_retry_at = None;
    false
}

/// A danger hold waits visibly: a silent wait scored as an employed worker to
/// every counter, the bot's recovery logic included.
fn report_danger_hold(state: &State, id: UnitId, events: &mut Vec<Event>) {
    if state
        .current_tick()
        .is_multiple_of(DANGER_HOLD_REPORT_PERIOD)
    {
        let unit = state.unit(id).expect("caller checked");
        events.push(Event::OrderStalled {
            unit: id,
            player: unit.player,
            pos: unit.pos,
            reason: StallReason::DangerHold,
        });
    }
}

fn known_rect_route(
    state: &State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    anchor: TilePos,
    size: (i32, i32),
    avoid_danger: bool,
    mut scan: Option<&mut DropOffScan>,
) -> Option<PathFollow> {
    let unit = state.unit(id).expect("caller checked");
    let from = unit.tile();
    let player = unit.player;
    let frame = crate::tick::rect_approach_origin(state, player, from, anchor, size);
    let building = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor && b.stats().size == size && b.player == player);
    let mut candidates: Vec<_> = crate::geometry::work_positions(
        anchor,
        size,
        contact::clearance(unit),
        unit.kind.stats().radius * crowding::compression(),
    )
    .into_iter()
    .filter_map(|entry| {
        let goal = TilePos::containing(entry);
        let point = building.and_then(|b| contact::endpoint(state, id, b.id, entry))?;
        Some(crowding::Position { goal, point })
    })
    .filter(|candidate| known_ground_passable(state, danger, player, candidate.goal))
    .filter(|candidate| !avoid_danger || !danger.contains(candidate.goal))
    .collect();
    candidates.sort_by_key(|candidate| {
        (
            unit.pos.dist_sq(candidate.point),
            crate::geometry::rect_approach_key_from(from, frame, anchor, size, candidate.goal),
            (candidate.point.x - unit.pos.x) * (frame.center().y - unit.pos.y)
                - (candidate.point.y - unit.pos.y) * (frame.center().x - unit.pos.x),
        )
    });
    // The passability predicate is candidate-independent, so one failed
    // search that exhausted the walkable component has already decided
    // every remaining doorstep, including candidates the chooser moves
    // outward and a prior same-scan foundry's flood, which the scratch
    // still holds at entry. Skipping a proven tile returns the identical
    // None without re-flooding to the cap.
    let mut proven = scan.as_deref().is_some_and(|scan| scan.flood_exhausted);
    crowding::choose(
        state,
        id,
        candidates,
        anchor.center()
            + Vec2Fx::new(Fx::from_num(size.0 - 1), Fx::from_num(size.1 - 1)) / Fx::from_num(2),
        |tile| {
            known_ground_passable(state, danger, player, tile)
                && (!avoid_danger || !danger.contains(tile))
        },
        |goal| {
            if proven
                && danger
                    .last_route_reachability(&[goal], false)
                    .is_some_and(|reachable| !reachable[0])
            {
                return None;
            }
            let route = danger.find_route(from, goal, |tile| {
                known_ground_passable(state, danger, player, tile)
                    && (!avoid_danger || danger.route_safe_from(from, tile))
            });
            if route.is_none() {
                proven = true;
                if danger.last_route_reachability(&[], false).is_some()
                    && let Some(scan) = scan.as_deref_mut()
                {
                    scan.flood_exhausted = true;
                }
            }
            route
        },
    )
}

fn switch_source(state: &mut State, id: UnitId, node: TilePos, anchor: TilePos) {
    let unit = state.unit_mut(id).expect("caller checked");
    unit.order = Order::Harvest {
        node,
        anchor: Some(anchor),
        retiring: false,
    };
    unit.path = None;
    unit.progress = 0;
}

fn begin_retirement(state: &mut State, id: UnitId, node: TilePos, anchor: TilePos) {
    let unit = state.unit_mut(id).expect("caller checked");
    unit.order = Order::Harvest {
        node,
        anchor: Some(anchor),
        retiring: true,
    };
    unit.path = None;
    unit.progress = 0;
}

fn source_route_failed(
    state: &mut State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    node: TilePos,
    anchor: TilePos,
    events: &mut Vec<Event>,
) {
    if let Some(next) = replacement_source(state, danger, id, anchor, Some(node)) {
        switch_source(state, id, next.pos, anchor);
        return;
    }
    let carrying = state.unit(id).expect("caller checked").carrying;
    begin_retirement(state, id, node, anchor);
    if carrying > 0 {
        deliver(state, danger, id, events, true);
    } else {
        retire(state, danger, id, events);
    }
}

/// Strip the wreck from beside it. Decay can beat the stripper to the
/// last piece — the dry-source branch above handles the morning after.
fn extract_wreck(state: &mut State, id: UnitId, node: TilePos, ticks_per_scrap: u32) {
    let unit = state.unit_mut(id).expect("caller checked");
    unit.path = None;
    unit.progress += 1;
    if unit.progress < ticks_per_scrap {
        return;
    }
    unit.progress = 0;
    if state.map.extract_wreck(node).is_some() {
        state.unit_mut(id).expect("caller checked").carrying += 1;
    }
}

/// Stand at the node and chip scrap off it.
fn extract(
    state: &mut State,
    id: UnitId,
    node: TilePos,
    ticks_per_scrap: u32,
    events: &mut Vec<Event>,
) {
    let unit = state.unit_mut(id).expect("caller checked");
    unit.path = None;
    unit.progress += 1;
    if unit.progress < ticks_per_scrap {
        return;
    }
    unit.progress = 0;
    unit.carrying += 1;
    if state.map.extract_scrap(node) == Some(0) {
        events.push(Event::NodeDepleted { pos: node });
    }
}

/// Advances only stationary, uninterrupted work; the whole load releases
/// once, on the last tick, before the ordinary combat-resolution phase.
fn unload_cargo(
    state: &mut State,
    id: UnitId,
    foundry: BuildingId,
    events: &mut Vec<Event>,
) -> bool {
    let unit = state.unit_mut(id).expect("caller checked");
    unit.path = None;
    let release = unit.unloading.get_or_insert(crate::state::Unloading {
        foundry,
        elapsed: 0,
    });
    if release.foundry != foundry {
        *release = crate::state::Unloading {
            foundry,
            elapsed: 0,
        };
    }
    release.elapsed += 1;
    if release.elapsed < crate::stats::UNLOAD_TICKS {
        return false;
    }
    deposit_cargo(state, id, foundry, events);
    true
}

fn deposit_cargo(state: &mut State, id: UnitId, foundry: BuildingId, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let (me, carrying) = (unit.player, unit.carrying);
    let unit = state.unit_mut(id).expect("caller checked");
    unit.carrying = 0;
    unit.unloading = None;
    unit.progress = 0;
    unit.path = None;
    // Saturating: a hostile scenario can start a bank near u32::MAX.
    // The event reports what was actually credited, not what was
    // carried — at the ceiling those differ.
    let seat = state.player_mut(me);
    let credited = seat.scrap.saturating_add(carrying) - seat.scrap;
    seat.scrap += credited;
    if credited > 0 {
        seat.recovery_allowance = 0;
        seat.recovery_target = 0;
        seat.recovery_ready = true;
    }
    events.push(Event::ScrapDeposited {
        unit: id,
        foundry,
        player: me,
        amount: credited,
    });
}

fn finish_delivery(state: &mut State, id: UnitId, foundry: BuildingId) {
    state.unit_mut(id).expect("caller checked").advance_queue();
    let unit = state.unit(id).expect("caller checked");
    if unit.order != Order::Idle {
        return;
    }
    let building = state.building(foundry).expect("live drop-off");
    let center = unit.tile().center();
    let edge = crate::geometry::footprint_contact(center, building.anchor, building.stats().size);
    let outward = center - edge;
    let goal = unit
        .tile()
        .offset(outward.x.signum().to_num(), outward.y.signum().to_num());
    // A finished crew parks outside the contact ring so the remaining loads can dock.
    if let Some(waypoints) = route_for(state, unit.kind, unit.tile(), goal) {
        state.unit_mut(id).expect("caller checked").path =
            Some(work_position_path(goal, waypoints));
    }
}

pub(in crate::tick) fn return_cargo_destination(
    state: &State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    requested: Option<BuildingId>,
) -> Option<BuildingId> {
    let drop_offs = drop_offs_by_distance(state, id);
    for avoid_danger in [true, false] {
        let mut scan = DropOffScan::default();
        if let Some(foundry_id) = drop_offs.iter().copied().find(|foundry_id| {
            if requested.is_some_and(|requested| requested != *foundry_id) {
                return false;
            }
            let foundry = state.building(*foundry_id).expect("live drop-off");
            state.in_building_work_reach(state.unit(id).expect("validated worker"), *foundry_id)
                || known_rect_route(
                    state,
                    danger,
                    id,
                    foundry.anchor,
                    foundry.stats().size,
                    avoid_danger,
                    Some(&mut scan),
                )
                .is_some()
        }) {
            return Some(foundry_id);
        }
    }
    None
}

pub(super) fn return_cargo(
    state: &mut State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    foundry: BuildingId,
    repair: bool,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    if unit.carrying == 0 {
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    }
    if let Some(building) = state.building(foundry).filter(|building| {
        building.player == unit.player
            && building.hp > 0
            && building.built
            && building.kind.is_drop_off()
    }) {
        if state.in_building_work_reach(unit, foundry) {
            let needs_repair = repair && building.hp < building.stats().max_hp;
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = None;
            if !unit.work_stopped() {
                unit.unloading = None;
                return;
            }
            if !unload_cargo(state, id, foundry, events) {
                return;
            }
            let unit = state.unit_mut(id).expect("caller checked");
            if needs_repair {
                unit.order = Order::Repair { building: foundry };
            } else {
                finish_delivery(state, id, foundry);
            }
            return;
        }
        state.unit_mut(id).expect("caller checked").unloading = None;
        if try_drop_offs(state, danger, id, &[foundry], events) {
            return;
        }
    }
    let unit = state.unit_mut(id).expect("caller checked");
    let (player, pos) = (unit.player, unit.pos);
    unit.advance_queue();
    events.push(Event::OrderStalled {
        unit: id,
        player,
        pos,
        reason: StallReason::NoRoute,
    });
}

/// Haul the load to a reachable own Foundry and deposit when adjacent.
/// A retiring contract advances its queued program only after that safe
/// arrival; a live contract keeps its anchored zone and returns next tick.
fn deliver(
    state: &mut State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    events: &mut Vec<Event>,
    retiring: bool,
) {
    let unit = state.unit(id).expect("caller checked");
    let drop_offs = drop_offs_by_distance(state, id);
    let active = unit
        .unloading
        .map(|release| release.foundry)
        .filter(|foundry| drop_offs.contains(foundry));
    let at_drop_off = active.or_else(|| {
        drop_offs
            .iter()
            .copied()
            .find(|foundry_id| state.in_building_work_reach(unit, *foundry_id))
    });
    if let Some(foundry) = at_drop_off
        && state.in_building_work_reach(unit, foundry)
    {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        if !unit.work_stopped() {
            unit.unloading = None;
            return;
        }
        if unload_cargo(state, id, foundry, events) && retiring {
            finish_delivery(state, id, foundry);
        }
        return;
    }
    state.unit_mut(id).expect("caller checked").unloading = None;

    if try_drop_offs(state, danger, id, &drop_offs, events) {
        return;
    }

    // Homeless or sealed off: keep the cargo, but do not erase a queued
    // program. The next order may still bring the worker to a drop-off.
    let unit = state.unit_mut(id).expect("caller checked");
    let (player, pos) = (unit.player, unit.pos);
    unit.advance_queue();
    events.push(Event::OrderStalled {
        unit: id,
        player,
        pos,
        reason: StallReason::NoRoute,
    });
}

/// Sticky retirement: a dry/unsafe worker reaches a built Foundry before
/// becoming idle or starting the next queued order. If no Foundry is
/// reachable, the queue still advances once instead of being erased.
fn retire(state: &mut State, danger: &GroundSalvageDanger, id: UnitId, events: &mut Vec<Event>) {
    if state.unit(id).expect("caller checked").carrying > 0 {
        deliver(state, danger, id, events, true);
        return;
    }
    let tile = state.unit(id).expect("caller checked").tile();
    let drop_offs = drop_offs_by_distance(state, id);
    if let Some(foundry) = drop_offs.iter().copied().find(|foundry_id| {
        let foundry = state
            .building(*foundry_id)
            .expect("collected live drop-off");
        tile_adjacent_to_rect(tile, foundry.anchor, foundry.stats().size)
    }) {
        finish_delivery(state, id, foundry);
        return;
    }
    if try_drop_offs(state, danger, id, &drop_offs, events) {
        return;
    }
    let unit = state.unit_mut(id).expect("caller checked");
    let (player, pos) = (unit.player, unit.pos);
    unit.advance_queue();
    events.push(Event::OrderStalled {
        unit: id,
        player,
        pos,
        reason: StallReason::NoRoute,
    });
}

/// The player's completed drop-off structures, nearest first. The one
/// funnel deliveries and retirement consult — a kind joins the drop-off
/// network by answering [`crate::stats::BuildingKind::is_drop_off`].
fn drop_offs_by_distance(state: &State, id: UnitId) -> Vec<BuildingId> {
    let unit = state.unit(id).expect("caller checked");
    let mut drop_offs: Vec<(chassis::fx::Fx, BuildingId)> = state
        .buildings
        .iter()
        .filter(|building| {
            building.player == unit.player
                && building.hp > 0
                && building.built
                && building.kind.is_drop_off()
        })
        .map(|building| (unit.pos.dist_sq(building.center()), building.id))
        .collect();
    drop_offs.sort_unstable();
    drop_offs
        .into_iter()
        .map(|(_, drop_off_id)| drop_off_id)
        .collect()
}

#[cfg(test)]
mod harvest_zone_tests {
    use super::*;
    use crate::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
    use crate::stats::BuildingKind;
    use crate::{Faction, PlayerId, Scenario, UnitKind};

    #[test]
    fn return_cargo_reuses_exhausted_floods_per_worker_and_safety_pass() {
        let mut rows = vec![vec!['.'; 32]; 20];
        for (y, row) in rows.iter_mut().enumerate() {
            for (x, tile) in row.iter_mut().enumerate() {
                if y == 0 || y == 19 || x == 0 || x == 31 || x == 15 {
                    *tile = '#';
                }
            }
        }
        rows[2][18] = '1';
        rows[16][28] = '2';
        let scenario = serde_json::json!({
            "name": "sealed-worker-drop-offs", "seed": 25,
            "map": rows.into_iter().map(|row| row.into_iter().collect::<String>()).collect::<Vec<_>>(),
            "players": [
                {"name": "F", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "C", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "units": [
                {"player": 0, "kind": "harvester", "x": 4, "y": 5},
                {"player": 0, "kind": "excavator", "x": 6, "y": 5},
                {"player": 0, "kind": "harvester", "x": 18, "y": 8}
            ],
            "buildings": [
                {"player": 0, "kind": "foundry", "x": 23, "y": 2},
                {"player": 0, "kind": "foundry", "x": 18, "y": 11},
                {"player": 0, "kind": "foundry", "x": 23, "y": 11}
            ]
        });
        let state = Scenario::from_json(&scenario.to_string())
            .unwrap()
            .build()
            .unwrap();
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        for worker in &state.units[..2] {
            let before = danger.route_search_count();
            assert_eq!(
                return_cargo_destination(&state, &danger, worker.id, None),
                None
            );
            assert_eq!(danger.route_search_count() - before, 2);
        }
        assert!(return_cargo_destination(&state, &danger, state.units[2].id, None).is_some());
    }

    #[test]
    fn return_cargo_resets_reachability_before_ignoring_danger() {
        let scenario = serde_json::json!({
            "name": "danger-blocked-return", "seed": 25,
            "map": [
                "##########################",
                "#1....................2..#",
                "#........................#",
                "#........................#",
                "#........................#",
                "#........................#",
                "##########################"
            ],
            "players": [
                {"name": "F", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "C", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "units": [
                {"player": 0, "kind": "harvester", "x": 16, "y": 3},
                {"player": 1, "kind": "scuttler", "x": 12, "y": 3}
            ]
        });
        let state = Scenario::from_json(&scenario.to_string())
            .unwrap()
            .build()
            .unwrap();
        let worker = state.units[0].id;
        let foundry = state
            .buildings
            .iter()
            .find(|b| b.player == PlayerId(0))
            .unwrap();
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        assert!(
            known_rect_route(
                &state,
                &danger,
                worker,
                foundry.anchor,
                foundry.stats().size,
                true,
                None
            )
            .is_none()
        );
        let before = danger.route_search_count();
        assert_eq!(
            return_cargo_destination(&state, &danger, worker, None),
            Some(foundry.id)
        );
        assert_eq!(danger.route_search_count() - before, 2);
    }

    fn held_worker(wall: Option<(usize, usize)>) -> (State, UnitId, KnownSource, PathFollow) {
        let mut map = vec![
            "##########################".to_owned(),
            "#1....................2..#".to_owned(),
            "#........................#".to_owned(),
            "#.....s..................#".to_owned(),
            "#........................#".to_owned(),
            "#........................#".to_owned(),
            "##########################".to_owned(),
        ];
        if let Some((x, y)) = wall {
            map[y].replace_range(x..=x, "#");
        }
        let scenario = serde_json::json!({
            "name": "danger-held-order", "seed": 25,
            "map": map,
            "players": [
                {"name": "F", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "C", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "units": [
                {"player": 0, "kind": "harvester", "x": 16, "y": 3},
                {"player": 1, "kind": "scuttler", "x": 12, "y": 3}
            ]
        });
        let mut state = Scenario::from_json(&scenario.to_string())
            .unwrap()
            .build()
            .unwrap();
        let node = TilePos::new(6, 3);
        let source = known_source(&state, PlayerId(0), node).expect("the node is in sight");
        state.units[0].order = Order::Harvest {
            node,
            anchor: None,
            retiring: false,
        };
        let ordered = PathFollow {
            final_point: None,
            goal: TilePos::new(7, 3),
            waypoints: (7..16).rev().map(|x| TilePos::new(x, 3)).collect(),
            next: 0,
        };
        let worker = state.units[0].id;
        (state, worker, source, ordered)
    }

    /// Ticks, from the scene's start, on which the held worker searched.
    fn searches(
        state: &mut State,
        worker: UnitId,
        source: KnownSource,
        ordered: &PathFollow,
        ticks: u64,
    ) -> Vec<u64> {
        let start = state.tick;
        let mut searched = Vec::new();
        for tick in start..start + ticks {
            state.tick = tick;
            state.units[0].path = Some(ordered.clone());
            let danger = GroundSalvageDanger::capture(state, PlayerId(0));
            let before = danger.route_search_count();
            assert!(approach_authoritative_source(
                state, &danger, worker, source
            ));
            if danger.route_search_count() > before {
                searched.push(tick - start);
            }
        }
        searched
    }

    #[test]
    fn a_worker_held_by_danger_searches_at_once_then_backs_off_after_a_failure() {
        let (mut state, worker, source, ordered) = held_worker(None);
        let period = crate::stats::HARVEST_DANGER_RETRY_TICKS;
        let searched = searches(&mut state, worker, source, &ordered, 2 * period);
        assert_eq!(searched, [0, period], "no safe detour exists in this lane");
        assert_eq!(
            state.units[0].path.as_ref(),
            Some(&ordered),
            "the ordered route stays in force"
        );
        assert_eq!(state.units[0].danger_retry_at, Some(state.tick + 1));
    }

    #[test]
    fn a_danger_retry_round_trips_up_to_its_bound() {
        let (mut state, ..) = held_worker(None);
        let bound = state.tick + crate::stats::HARVEST_DANGER_RETRY_TICKS;
        state.units[0].danger_retry_at = Some(bound);
        let restored: State =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(restored.units[0].danger_retry_at, Some(bound));
        let renamed = serde_json::to_string(&state)
            .unwrap()
            .replace("danger_retry_at", "detour_retry_at");
        let restored: State = serde_json::from_str(&renamed).unwrap();
        assert_eq!(
            restored.units[0].danger_retry_at,
            Some(bound),
            "saves from before the rename keep their retry"
        );
        state.units[0].danger_retry_at = Some(bound + 1);
        assert!(serde_json::from_str::<State>(&serde_json::to_string(&state).unwrap()).is_err());
    }

    #[test]
    fn a_waypoint_turned_impassable_replans_every_tick() {
        let (mut state, worker, source, ordered) = held_worker(Some((14, 3)));
        assert_eq!(
            searches(&mut state, worker, source, &ordered, 4),
            [0, 1, 2, 3]
        );
        assert_eq!(state.units[0].danger_retry_at, None);
    }

    #[test]
    fn a_sealed_worker_settles_every_work_position_with_one_search() {
        let mut rows = vec![vec!['.'; 32]; 20];
        for (y, row) in rows.iter_mut().enumerate() {
            for (x, tile) in row.iter_mut().enumerate() {
                if y == 0 || y == 19 || x == 0 || x == 31 || x == 15 {
                    *tile = '#';
                }
            }
        }
        rows[2][18] = '1';
        rows[16][28] = '2';
        rows[7][20] = 's';
        let scenario = serde_json::json!({
            "name": "sealed-worker-source", "seed": 25,
            "map": rows.into_iter().map(|row| row.into_iter().collect::<String>()).collect::<Vec<_>>(),
            "players": [
                {"name": "F", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "C", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "units": [{"player": 0, "kind": "harvester", "x": 4, "y": 5}]
        });
        let state = Scenario::from_json(&scenario.to_string())
            .unwrap()
            .build()
            .unwrap();
        let source =
            known_source(&state, PlayerId(0), TilePos::new(20, 7)).expect("the node is in sight");
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        let before = danger.route_search_count();
        assert!(safe_source_route(&state, &danger, state.units[0].id, source).is_none());
        assert_eq!(danger.route_search_count() - before, 1);
    }

    #[test]
    fn a_worker_held_from_its_drop_off_rescans_after_the_retry_period_and_on_command() {
        let scenario = serde_json::json!({
            "name": "danger-held-delivery", "seed": 25,
            "map": [
                "##########################",
                "#1....................2..#",
                "#........................#",
                "#........................#",
                "#........................#",
                "#........................#",
                "##########################"
            ],
            "players": [
                {"name": "F", "faction": "ferrous", "scrap": 0, "bot": false},
                {"name": "C", "faction": "cupric", "scrap": 0, "bot": true}
            ],
            "units": [
                {"player": 0, "kind": "harvester", "x": 16, "y": 3},
                {"player": 1, "kind": "scuttler", "x": 12, "y": 3}
            ]
        });
        let mut state = Scenario::from_json(&scenario.to_string())
            .unwrap()
            .build()
            .unwrap();
        let worker = state.units[0].id;
        state.units[0].carrying = 1;
        let foundry = state
            .buildings
            .iter()
            .find(|b| b.player == PlayerId(0))
            .unwrap()
            .id;
        let period = crate::stats::HARVEST_DANGER_RETRY_TICKS;
        let start = DANGER_HOLD_REPORT_PERIOD - 1;
        let mut searched = Vec::new();
        let mut reported = Vec::new();
        for tick in start..=start + period {
            state.tick = tick;
            let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
            let before = danger.route_search_count();
            let mut events = Vec::new();
            assert!(try_drop_offs(
                &mut state,
                &danger,
                worker,
                &[foundry],
                &mut events
            ));
            assert!(state.units[0].path.is_none(), "a held worker stands");
            if danger.route_search_count() > before {
                searched.push(tick - start);
            }
            if events.iter().any(|event| {
                matches!(
                    event,
                    Event::OrderStalled {
                        reason: StallReason::DangerHold,
                        ..
                    }
                )
            }) {
                reported.push(tick);
            }
        }
        assert_eq!(searched, [0, period]);
        assert_eq!(
            reported,
            [DANGER_HOLD_REPORT_PERIOD],
            "the hold stays visible"
        );
        assert!(state.units[0].danger_retry_at.is_some());
        crate::tick::commands::apply(
            &mut state,
            &[crate::PlayerCommand {
                player: PlayerId(0),
                command: crate::Command::Stop {
                    units: vec![worker],
                },
            }],
            &mut Vec::new(),
        );
        assert_eq!(
            state.units[0].danger_retry_at, None,
            "a command is judged at once"
        );
    }

    #[test]
    fn arriving_at_the_source_ends_a_route_hold() {
        let (mut state, worker, _, _) = held_worker(None);
        let node = TilePos::new(6, 3);
        let radius = state.units[0].kind.stats().radius;
        state.units[0].pos =
            crate::geometry::work_approach_point(TilePos::new(7, 3), node, (1, 1), radius);
        state.units[0].danger_retry_at = Some(state.tick + 8);
        assert!(state.units[0].in_work_reach(node, (1, 1)), "premise");
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        harvest(
            &mut state,
            &danger,
            worker,
            node,
            None,
            false,
            &mut Vec::new(),
        );
        assert_eq!(state.units[0].danger_retry_at, None);
    }

    #[test]
    fn mirrored_workers_replace_depleted_sources_in_their_local_frame() {
        let mirror = |pos: TilePos| TilePos::new(39 - pos.x, 23 - pos.y);
        let anchor = TilePos::new(7, 3);
        for (from, sources) in [
            (TilePos::new(6, 4), [TilePos::new(7, 2), TilePos::new(8, 3)]),
            (TilePos::new(6, 3), [TilePos::new(7, 2), TilePos::new(7, 4)]),
            (anchor, [TilePos::new(7, 2), TilePos::new(8, 3)]),
        ] {
            for reverse_ids in [false, true] {
                for heading in [0u8, 64, 128, 192] {
                    let mut rows = vec![vec!['.'; 40]; 24];
                    rows[4][4] = '1';
                    rows[18][34] = '2';
                    for pos in sources.into_iter().flat_map(|pos| [pos, mirror(pos)]) {
                        rows[pos.y as usize][pos.x as usize] = 's';
                    }
                    let mut scenario = Scenario::skirmish();
                    scenario.map = rows
                        .into_iter()
                        .map(|row| row.into_iter().collect())
                        .collect();
                    scenario.units = [from, mirror(from)]
                        .into_iter()
                        .enumerate()
                        .map(|(player, pos)| UnitSpec {
                            player: player as u8,
                            kind: UnitKind::Harvester,
                            x: pos.x,
                            y: pos.y,
                        })
                        .collect();
                    if reverse_ids {
                        scenario.units.reverse();
                    }
                    let mut state = scenario.build().unwrap();
                    for unit in &mut state.units {
                        unit.heading = if unit.player == PlayerId(0) {
                            heading
                        } else {
                            heading.wrapping_add(128)
                        };
                    }
                    let selected: Vec<_> = [PlayerId(0), PlayerId(1)]
                        .into_iter()
                        .map(|player| {
                            let worker = state
                                .units
                                .iter()
                                .find(|unit| unit.player == player)
                                .unwrap();
                            let anchor = if player == PlayerId(0) {
                                anchor
                            } else {
                                mirror(anchor)
                            };
                            let danger = GroundSalvageDanger::capture(&state, player);
                            replacement_source(&state, &danger, worker.id, anchor, Some(anchor))
                                .unwrap()
                                .pos
                        })
                        .collect();
                    assert_eq!(
                        selected[1],
                        mirror(selected[0]),
                        "from={from:?}, heading={heading}, reverse_ids={reverse_ids}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_zone_radius_covers_the_widest_connected_shipped_deposit() {
        // Compass Grand and Trident Plateau carry center deposits whose
        // endpoint span is seven tiles. The work zone is anchored rather
        // than re-centered after each hop, so this exact reach cannot
        // walk onward into a second field.
        assert_eq!(HARVEST_ZONE_RADIUS, 7);
    }

    #[test]
    fn replacement_preserves_worker_affinity_before_route_efficiency() {
        let state = Scenario {
            mode: Default::default(),
            name: "harvest-worker-affinity".into(),
            seed: 11,
            map: vec![
                "####################".into(),
                "#1.....#...........#".into(),
                "#......#...........#".into(),
                "#......#...........#".into(),
                "#......#...........#".into(),
                "#......#s..........#".into(),
                "#......#...........#".into(),
                "#......#...........#".into(),
                "#......#...........#".into(),
                "#....s...........2.#".into(),
                "#..................#".into(),
                "####################".into(),
            ],
            players: vec![
                PlayerSpec {
                    name: "Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 5,
                y: 5,
            }],
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .unwrap();
        let worker = state.units[0].id;
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        let local = known_source(&state, PlayerId(0), TilePos::new(8, 5)).unwrap();
        let cheap_route = known_source(&state, PlayerId(0), TilePos::new(5, 9)).unwrap();
        assert!(
            source_route_len(&state, &danger, worker, local).unwrap()
                > source_route_len(&state, &danger, worker, cheap_route).unwrap(),
            "the fixture must make route-first scoring prefer the farther source"
        );
        assert_eq!(
            replacement_source(&state, &danger, worker, TilePos::new(5, 5), None)
                .map(|source| source.pos),
            Some(local.pos),
            "safe reachable sources stay with the worker already closest to them"
        );
    }

    #[test]
    fn per_tick_danger_rechecks_are_constant_even_on_a_long_route() {
        use std::cell::Cell;

        let path = PathFollow {
            final_point: None,
            goal: TilePos::new(127, 1),
            waypoints: (1..=127).map(|x| TilePos::new(x, 1)).collect(),
            next: 3,
        };
        let checks = Cell::new(0);
        assert!(
            first_flagged_waypoint(&path, |_| {
                checks.set(checks.get() + 1);
                true
            })
            .is_none()
        );
        assert_eq!(
            checks.get(),
            HARVEST_DANGER_LOOKAHEAD,
            "the hot-path cost is independent of total route length"
        );
    }

    #[test]
    fn danger_replan_cadence_uses_owner_local_rank() {
        let mut state = Scenario {
            mode: Default::default(),
            name: "owner-local-replan-cadence".into(),
            seed: 13,
            map: vec![
                "##############".into(),
                "#1.........2.#".into(),
                "#............#".into(),
                "#............#".into(),
                "#............#".into(),
                "##############".into(),
            ],
            players: vec![
                PlayerSpec {
                    name: "West".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "East".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Harvester,
                    x: 4,
                    y: 2,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Harvester,
                    x: 7,
                    y: 2,
                },
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Harvester,
                    x: 4,
                    y: 3,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Harvester,
                    x: 7,
                    y: 3,
                },
            ],
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("the cadence scenario builds");
        let ids: Vec<_> = state
            .units
            .iter()
            .map(|unit| (unit.id, unit.player))
            .collect();
        assert_eq!(
            ids,
            vec![
                (UnitId(0), PlayerId(0)),
                (UnitId(1), PlayerId(1)),
                (UnitId(2), PlayerId(0)),
                (UnitId(3), PlayerId(1)),
            ],
            "the fixture interleaves global ids across equivalent owner ranks"
        );

        state.tick = 0;
        assert!(danger_replan_window(&state, UnitId(0)));
        assert!(danger_replan_window(&state, UnitId(1)));
        assert!(!danger_replan_window(&state, UnitId(2)));
        assert!(!danger_replan_window(&state, UnitId(3)));

        state.tick = HARVEST_REPLAN_PERIOD - 1;
        assert!(!danger_replan_window(&state, UnitId(0)));
        assert!(!danger_replan_window(&state, UnitId(1)));
        assert!(danger_replan_window(&state, UnitId(2)));
        assert!(danger_replan_window(&state, UnitId(3)));
    }

    #[test]
    fn unseen_wreck_selection_reads_frozen_memory_not_live_salvage() {
        let mut state = Scenario {
            mode: Default::default(),
            name: "harvest-memory".into(),
            seed: 9,
            map: vec![
                "##############################".into(),
                "#1...........................#".into(),
                "#............................#".into(),
                "#............................#".into(),
                "#..........................2.#".into(),
                "#............................#".into(),
                "##############################".into(),
            ],
            players: vec![
                PlayerSpec {
                    name: "Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 14,
                y: 3,
            }],
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .unwrap();
        let wreck = TilePos::new(15, 3);
        state.map.add_wreck(wreck, 9);
        state.refresh_vision();
        assert_eq!(
            known_source(&state, PlayerId(0), wreck),
            Some(KnownSource {
                pos: wreck,
                amount: 9,
                kind: SourceKind::Wreck,
            })
        );

        state.units[0].pos = TilePos::new(3, 3).center();
        state.refresh_vision();
        assert!(!state.vision(PlayerId(0)).visible(wreck));
        state.map.clear_wreck(wreck);
        assert_eq!(
            known_source(&state, PlayerId(0), wreck),
            Some(KnownSource {
                pos: wreck,
                amount: 9,
                kind: SourceKind::Wreck,
            }),
            "unseen source choice is a belief, not a hidden live-map read"
        );
    }

    #[test]
    fn an_unscouted_enemy_building_cannot_bend_a_route_through_fog() {
        let scenario = Scenario {
            mode: Default::default(),
            name: "harvest-route-belief".into(),
            seed: 10,
            map: vec![
                "##############################".into(),
                "#1...........................#".into(),
                "#............................#".into(),
                "#............................#".into(),
                "#..........................2.#".into(),
                "#............................#".into(),
                "##############################".into(),
            ],
            players: vec![
                PlayerSpec {
                    name: "Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 3,
                y: 3,
            }],
            buildings: Vec::new(),
            meta: None,
        };
        let clear = scenario.build().unwrap();
        let mut obscured_scenario = scenario;
        let hidden_anchor = TilePos::new(12, 2);
        obscured_scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Reclaimer,
            x: hidden_anchor.x,
            y: hidden_anchor.y,
        });
        let obscured = obscured_scenario.build().unwrap();
        assert!(!obscured.vision(PlayerId(0)).visible(hidden_anchor));
        assert!(obscured.vision(PlayerId(0)).ghosts().is_empty());

        let source = KnownSource {
            pos: TilePos::new(22, 3),
            amount: 1,
            kind: SourceKind::Wreck,
        };
        let clear_danger = GroundSalvageDanger::capture(&clear, PlayerId(0));
        let obscured_danger = GroundSalvageDanger::capture(&obscured, PlayerId(0));
        let clear_route = safe_source_route(&clear, &clear_danger, clear.units[0].id, source);
        let obscured_route =
            safe_source_route(&obscured, &obscured_danger, obscured.units[0].id, source);
        assert_eq!(
            obscured_route, clear_route,
            "two views that differ only behind fog must choose the same route"
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::state::PROGRESS_ENVELOPE;

    #[test]
    fn the_live_meter_saturates_below_the_progress_envelope() {
        // The step math's overflow guard: whatever a torch has lived
        // through, the meter it bills from stays under the ceiling the
        // ramp products are proven to fit (state.rs pins the products).
        assert_eq!(super::metered(0), 0);
        assert_eq!(super::metered(PROGRESS_ENVELOPE - 1), PROGRESS_ENVELOPE - 1);
        assert_eq!(super::metered(PROGRESS_ENVELOPE), PROGRESS_ENVELOPE - 1);
        assert_eq!(super::metered(u32::MAX), PROGRESS_ENVELOPE - 1);
    }
}
