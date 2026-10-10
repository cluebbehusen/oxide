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
/// of [`crate::state::PROGRESS_ENVELOPE`], the ceiling the snapshot
/// validator enforces. Saturated, the meter parks at the ceiling and the
/// torch keeps billing and welding its marginal step forever.
fn metered(progress: u32) -> u32 {
    progress.min(crate::state::PROGRESS_ENVELOPE - 1)
}

/// The hp a `ramp` spread evenly over `ticks` gains between meter readings
/// `from` and `to`. The products run in `u64`: a long-held weld meter times
/// a large hp ramp passes `u32`.
fn ramp_step(ramp: u32, from: u32, to: u32, ticks: u32) -> u32 {
    let at = |meter: u32| u64::from(ramp) * u64::from(meter) / u64::from(ticks);
    u32::try_from(at(to) - at(from)).expect("a step never exceeds its ramp")
}

/// Advance every committed building upgrade exactly once on the simulation
/// clock. The gain joins ordinary construction in the buffered resolver, so a
/// lethal hit on the completion tick still destroys the offline works instead
/// of letting it stand up through fire.
pub(super) fn advance_upgrades(state: &mut State, builds: &mut Vec<PendingHpGain>) {
    let sites: Vec<BuildingId> = state
        .buildings
        .iter()
        .filter(|building| building.upgrading() && building.hp > 0)
        .map(|building| building.id)
        .collect();

    for site in sites {
        let building = state.building(site).expect("upgrade id came from state");
        let (player, kind) = (building.player, building.kind);
        let stats = building.stats();
        let build_ticks = stats
            .construction
            .expect("an upgrade tier is constructible")
            .build_ticks;
        let start_hp = stats.max_hp / 5;
        let ramp = stats.max_hp - start_hp;
        let (progress, advanced) = state
            .building_mut(site)
            .expect("upgrade id came from state")
            .add_construction_work(1, build_ticks)
            .expect("an upgrading building runs its upgrade meter");
        let step = ramp_step(ramp, progress, advanced, build_ticks);
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
    // mid-brains, but building on a corpse would resurrect it and swallow
    // the destruction event.
    let Some(b) = state
        .building(site)
        .filter(|b| b.player == me && b.under_construction() && b.hp > 0)
    else {
        // Finished, cancelled, or destroyed: the job is over either way.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (anchor, kind) = (b.anchor, b.kind);
    let stats = b.stats();
    let size = kind.size();
    let build_ticks = stats
        .construction
        .expect("sites only exist for buildable kinds")
        .build_ticks;
    let unit = state.unit(id).expect("caller checked");
    if !b.provisional() && unit.work_stopped() && state.in_building_work_reach(unit, site) {
        let start_hp = stats.max_hp / 5;
        let ramp = stats.max_hp - start_hp;
        // An Excavator's crew-tick counts double: same ramp, half the wall
        // clock, telescoping like a second builder.
        let rate = state
            .unit(id)
            .expect("caller checked")
            .kind
            .stats()
            .build_rate;
        let (progress, advanced) = state
            .building_mut(site)
            .expect("just seen")
            .add_construction_work(rate, build_ticks)
            .expect("a site past its blueprint runs its construction meter");
        let starts = progress == 0;
        let step = ramp_step(ramp, progress, advanced, build_ticks);
        // Both the hp gain and the completion are buffered and applied
        // after damage — see PendingHpGain. The builder learns the site is
        // done next tick, through the built-site branch above.
        let completes = advanced >= build_ticks;
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
        .map(|b| (b.id, b.provisional()));
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
    let size = kind.size();
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
        .filter(|b| b.player == me && b.built() && b.hp > 0 && b.hp < b.stats().max_hp)
    else {
        // Healed, destroyed, or never a patient: the job is over.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (kind, tier) = (b.kind, b.tier);
    // The active tier's row: a Bulwark's weld ramp, ceiling, and billing
    // basis are the Bulwark's, not the base Turret's.
    let stats = kind.tier_stats(tier);
    // The welding rate is the construction ramp, except for the Foundry,
    // which uses its own ramp and billing basis so its construction price
    // does not inflate the repair bill for the victory structure.
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
        let bank = state.player(me).scrap;
        let Some(due) = u32::try_from(due).ok().filter(|&due| due <= bank) else {
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
        };
        state.player_mut(me).scrap = bank - due;
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        unit.progress = p + 1;
        let step = ramp_step(ramp, p, p + 1, ramp_ticks);
        if step > 0 {
            builds.push(PendingHpGain {
                starts: false,
                site: building,
                step,
                completes: false,
                player: me,
                kind,
                paid: due,
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
/// the same prepaid milli-scrap meter buildings use. The torch holds only
/// while both bodies stand still inside body-aware tool reach: a walking
/// patient is chased, not welded. Heals buffer like every hp gain and
/// resolve after damage; several welders stack, each billing its own torch
/// time. The patient's own orders are never touched.
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
                // The movement pre-pass, after weld resolution, will make
                // this welder walk off newly claimed ground. It cannot light
                // the torch and move in the same tick.
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

        // The billing meter is the welder's, with the patient's
        // own numbers: ramp is full max_hp (machines have no one-fifth
        // foundation), the clock is its training time, the basis its
        // price. Same ceiling prepay, same survival of no-op reissues.
        let stats = t_kind.stats();
        let (ramp, ramp_ticks) = (stats.max_hp, stats.train_ticks);
        let due = crate::stats::unit_repair_debit(t_kind, p);
        let bank = state.player(me).scrap;
        if due > bank {
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
        state.player_mut(me).scrap = bank - due;
        let unit = state.unit_mut(weld.welder).expect("just seen");
        unit.path = None;
        unit.progress = p + 1;
        let step = ramp_step(ramp, p, p + 1, ramp_ticks);
        if step > 0 {
            heals.push(super::PendingUnitHeal {
                unit: weld.patient,
                step,
                player: me,
                paid: due,
                source: crate::event::UnitRepairSource::FieldWelder { unit: weld.welder },
            });
        }
    }
}

/// Whether the movement pre-pass will give this pathless body an escape
/// path from a claimed building footprint. Weld settlement runs before that
/// pre-pass, so it must predict the same move to uphold the
/// both-bodies-still rule.
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
    for offset in [0_i8, 32, -32, 64, -64, 96, -96, -128] {
        let point = target.pos + chassis::compass::dir(bearing.wrapping_add_signed(offset)) * gap;
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
        b.player == me && b.built() && b.hp > 0 && b.kind != crate::stats::BuildingKind::Foundry
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
        let step = ramp_step(ramp, p, p + 1, ramp_ticks);
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
    anchor: TilePos,
    retiring: bool,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    let Some(hstats) = unit.kind.stats().harvest else {
        // Only harvesters ever get this order; be defensive anyway.
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    };
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
        .is_multiple_of(crate::stats::HARVEST_REPLAN_PERIOD)
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
        Some(index) => {
            index >= crate::stats::HARVEST_DANGER_REACT_ZONE && !danger_replan_window(state, id)
        }
    }
}

/// Index (relative to the walker's next waypoint) of the first
/// lookahead tile `safe` rejects, or `None` for a clear near segment.
/// The scan touches at most [`crate::stats::HARVEST_DANGER_LOOKAHEAD`] tiles however
/// long the route is.
fn first_flagged_waypoint(
    path: &PathFollow,
    mut safe: impl FnMut(TilePos) -> bool,
) -> Option<usize> {
    path.waypoints
        .iter()
        .skip(path.next as usize)
        .take(crate::stats::HARVEST_DANGER_LOOKAHEAD)
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
        unit.kind.stats().radius * crate::stats::SAME_OWNER_COMPRESSION,
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
    let outside = danger.outside_every_envelope(from);
    let route = |goal: TilePos| {
        if outside && safe_route_impossible(state, danger, from, goal, allow_dangerous_goal) {
            return None;
        }
        if failed
            && danger
                .last_route_reachability(&[goal], allow_dangerous_goal)
                .is_some_and(|reachable| !reachable[0])
        {
            return None;
        }
        let found = safe_route(goal);
        failed = found.is_none();
        if failed && outside {
            record_safe_failure(state, danger, player);
        }
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

/// Keeps a failed safe search's component. Only a visible node's live scrap
/// can open closed ground within a phase.
fn record_safe_failure(state: &State, danger: &GroundSalvageDanger, player: PlayerId) {
    danger.record_safe_failure(|tile| {
        state.vision(player).visible(tile) && state.map.scrap_at(tile) > 0
    });
}

fn safe_route_impossible(
    state: &State,
    danger: &GroundSalvageDanger,
    from: TilePos,
    goal: TilePos,
    allow_goal_only: bool,
) -> bool {
    danger.safe_route_impossible(from, goal, allow_goal_only, |tile| {
        state.map.scrap_at(tile) == 0
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

/// Walks the worker toward the nearest safely approachable drop-off. The
/// retained path is honored until the first failed safe route clears it,
/// and the danger-ignoring classification that decides waiting versus
/// stalling runs only after every safe attempt failed. Splitting the passes
/// lets one exhausted flood answer every remaining foundry, so a fully
/// sealed worker floods twice per tick, not twice per foundry. Returns
/// false only when no drop-off is reachable at all: the caller's stall.
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
        let (anchor, size) = (foundry.anchor, foundry.kind.size());
        if !path_cleared {
            let unit = state.unit(id).expect("caller checked");
            let player = unit.player;
            let from = unit.tile();
            if let Some(path) = unit
                .path
                .as_ref()
                .filter(|path| tile_adjacent_to_rect(path.goal, anchor, size))
                // Like every kept position, the route's own end point yields
                // only to a unit that outranks this one there.
                && (from.chebyshev(path.goal) > 1
                    || path
                        .final_point
                        .is_none_or(|point| !crowding::claimed(state, id, point, true)))
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
        let (anchor, size) = (foundry.anchor, foundry.kind.size());
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

/// A danger hold waits visibly: a silent wait would count as an employed
/// worker to every observer, including bot recovery logic.
fn report_danger_hold(state: &State, id: UnitId, events: &mut Vec<Event>) {
    if state
        .current_tick()
        .is_multiple_of(crate::stats::DANGER_HOLD_REPORT_PERIOD)
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

/// Approach one rectangle using only the worker's shared battlefield
/// knowledge. A bounded near-path check reacts to newly known danger
/// without rescanning a long route every tick; a full deterministic A*
/// runs only when that check fails or no path exists yet.
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
        .find(|b| b.anchor == anchor && b.kind.size() == size && b.player == player);
    let mut candidates: Vec<_> = crate::geometry::work_positions(
        anchor,
        size,
        contact::clearance(unit),
        unit.kind.stats().radius * crate::stats::SAME_OWNER_COMPRESSION,
    )
    .into_iter()
    .filter_map(|entry| {
        let goal = crate::geometry::work_tile(
            entry,
            unit.pos,
            crate::geometry::footprint_center(anchor, size),
        );
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
    let safe_shortcut = avoid_danger && danger.outside_every_envelope(from);
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
            if safe_shortcut && safe_route_impossible(state, danger, from, goal, false) {
                return None;
            }
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
                if safe_shortcut {
                    record_safe_failure(state, danger, player);
                }
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
        anchor,
        retiring: false,
    };
    unit.path = None;
    unit.progress = 0;
}

fn begin_retirement(state: &mut State, id: UnitId, node: TilePos, anchor: TilePos) {
    let unit = state.unit_mut(id).expect("caller checked");
    unit.order = Order::Harvest {
        node,
        anchor,
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

/// Strip the wreck from beside it. Decay can remove the last piece first;
/// the dry-source branch above handles that.
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
        seat.recovery = crate::state::Recovery::Ready;
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
    let edge = crate::geometry::footprint_contact(center, building.anchor, building.kind.size());
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
                    foundry.kind.size(),
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
            && building.built()
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
        tile_adjacent_to_rect(tile, foundry.anchor, foundry.kind.size())
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
                && building.built()
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
mod harvest_zone_tests;

#[cfg(test)]
mod tests;
