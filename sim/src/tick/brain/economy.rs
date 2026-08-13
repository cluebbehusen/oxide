//! The working half of the brain: construction, repair welding,
//! harvesting, wreck stripping, and delivery. Hp gains buffer like
//! damage and resolve after it — fire wins ties.

use super::super::{rect_adjacent_tiles, route_for, tile_adjacent_to_rect};
use super::PendingHpGain;
use super::locomotion::approach_rect;
use crate::event::{Event, StallReason};
use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::state::{Order, PathFollow, State};
use crate::stats::HARVEST_ZONE_RADIUS;
use crate::vision::GroundSalvageDanger;
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
        .filter(|b| b.player == me && !b.built && b.hp > 0)
    else {
        // Finished, cancelled, or destroyed: the job is over either way.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (anchor, kind) = (b.anchor, b.kind);
    // Tier stats: an upgrading building (tier already bumped, built
    // false) ramps on the NEW tier's price row; a fresh site's tier is
    // zero, so this is the ordinary construction row there.
    let stats = b.stats();
    let size = stats.size;
    let build_ticks = stats
        .construction
        .expect("sites only exist for buildable kinds")
        .build_ticks;
    let tile = state.unit(id).expect("caller checked").tile();
    if tile_adjacent_to_rect(tile, anchor, size) {
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
        let advanced = (b.progress + rate).min(build_ticks);
        let step = (ramp * advanced / build_ticks) - (ramp * b.progress / build_ticks);
        b.progress = advanced;
        // Both the hp gain and the completion are buffered and applied
        // after damage — see PendingHpGain. The builder learns the site is
        // done next tick, through the built-site branch above.
        let completes = b.progress >= build_ticks;
        if step > 0 || completes {
            builds.push(PendingHpGain {
                site,
                step,
                completes,
                player: me,
                kind,
                paid: 0, // construction paid at placement
            });
        }
        state.unit_mut(id).expect("caller checked").path = None;
    } else if !approach_rect(state, id, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// Walk out to a deferred claim ([`Order::Found`]) and, once standing
/// beside — or inside — the promised footprint, buffer the founding for
/// id-ordered resolution after the volley. Adjacency is what makes the
/// arrival re-check honest: a harvester's sight covers every buildable
/// footprint from its doorstep, so the strict predicate reads only
/// ground the founder now sees. A crewmate arriving after the claim
/// stood simply joins the site. No route stalls exactly like every
/// other walk-to-work order.
pub(super) fn found(
    state: &mut State,
    id: UnitId,
    kind: crate::stats::BuildingKind,
    anchor: TilePos,
    events: &mut Vec<Event>,
    founds: &mut Vec<super::PendingFounding>,
) {
    let me = state.unit(id).expect("caller checked").player;
    // The claim already stands (a crewmate founded on an earlier tick,
    // or the player resumed the same corner): join the crew.
    let ours = state
        .buildings
        .iter()
        .find(|b| b.anchor == anchor && b.kind == kind && b.player == me && !b.built && b.hp > 0)
        .map(|b| b.id);
    if let Some(site) = ours {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Build { site };
        unit.path = None;
        unit.progress = 0;
        return;
    }
    // The crew already FINISHED it: a late crewmate's founding is done,
    // not stalled — falling through would read its own standing
    // building as taken ground.
    let done = state
        .buildings
        .iter()
        .any(|b| b.anchor == anchor && b.kind == kind && b.player == me && b.built);
    if done {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.progress = 0;
        unit.advance_queue();
        return;
    }
    let size = kind.base_stats().size;
    let tile = state.unit(id).expect("caller checked").tile();
    let inside = tile.x >= anchor.x
        && tile.x < anchor.x + size.0
        && tile.y >= anchor.y
        && tile.y < anchor.y + size.1;
    if inside || tile_adjacent_to_rect(tile, anchor, size) {
        founds.push(super::PendingFounding {
            unit: id,
            player: me,
            kind,
            anchor,
        });
        state.unit_mut(id).expect("caller checked").path = None;
    } else if !approach_rect(state, id, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
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
    let (anchor, kind) = (b.anchor, b.kind);
    let stats = kind.base_stats();
    let size = stats.size;
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
    let tile = state.unit(id).expect("caller checked").tile();
    if tile_adjacent_to_rect(tile, anchor, size) {
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
                site: building,
                step,
                completes: false,
                player: me,
                kind,
                paid: due as u32,
            });
        }
    } else if !approach_rect(state, id, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
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
/// [`crate::stats::REPAIR_REACH`]: a walking patient is chased, not
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
    let Some(t) = state
        .unit(patient)
        .filter(|t| t.id != id && t.player == me && t.hp > 0 && t.hp < t.kind.stats().max_hp)
    else {
        // Healed, dead, or never a patient: the job is over.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (t_pos, t_tile) = (t.pos, t.tile());
    let reach = crate::stats::REPAIR_REACH;
    let unit = state.unit(id).expect("caller checked");
    let in_reach = unit.pos.dist_sq(t_pos) <= reach * reach;
    if in_reach {
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
        chase_patient(state, id, t_tile, events);
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
            let unit_pos = unit.pos;
            if footprint_eviction_pending(state, weld.welder) {
                // Phase 5, after weld resolution, will make this welder
                // walk off newly claimed ground. It cannot light the
                // torch and move in the same tick.
                stationary[slot] = false;
                continue;
            }
            let Some(t) = state
                .unit(weld.patient)
                .filter(|t| t.player == me && t.hp > 0 && t.hp < t.kind.stats().max_hp)
            else {
                stationary[slot] = false;
                continue;
            };
            let reach = crate::stats::REPAIR_REACH;
            if t.path.is_none()
                && !matches!(t.order, Order::Found { .. })
                && !footprint_eviction_pending(state, weld.patient)
                && unit_pos.dist_sq(t.pos) <= reach * reach
            {
                continue;
            }
            let patient_tile = t.tile();
            stationary[slot] = false;
            chase_patient(state, weld.welder, patient_tile, events);
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
        let (me, unit_pos, p) = (unit.player, unit.pos, metered(unit.progress));
        let Some(t) = state
            .unit(weld.patient)
            .filter(|t| t.player == me && t.hp > 0 && t.hp < t.kind.stats().max_hp)
        else {
            continue;
        };
        debug_assert!(t.path.is_none());
        debug_assert!(!footprint_eviction_pending(state, weld.welder));
        debug_assert!(!footprint_eviction_pending(state, weld.patient));
        debug_assert!(
            unit_pos.dist_sq(t.pos) <= crate::stats::REPAIR_REACH * crate::stats::REPAIR_REACH
        );
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

/// Re-aims the torch carrier at the patient's current tile. The meter
/// survives the walk; only committed torch time bills.
fn chase_patient(state: &mut State, id: UnitId, patient_tile: TilePos, events: &mut Vec<Event>) {
    // Cheap pursuit without per-tick A*: keep a path whose goal has
    // drifted no more than one tile from the patient.
    let unit = state.unit(id).expect("caller checked");
    let kind = unit.kind;
    let tile = unit.tile();
    let keep = unit
        .path
        .as_ref()
        .is_some_and(|pf| pf.goal.chebyshev(patient_tile) <= 1);
    if keep {
        return;
    }
    match route_for(state, kind, tile, patient_tile) {
        Some(waypoints) => {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                goal: patient_tile,
                waypoints,
                next: 0,
            });
        }
        None => {
            let unit = state.unit_mut(id).expect("caller checked");
            let (player, pos) = (unit.player, unit.pos);
            unit.clear_program();
            events.push(Event::OrderStalled {
                unit: id,
                player,
                pos,
                reason: StallReason::NoRoute,
            });
        }
    }
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
    let (anchor, kind) = (b.anchor, b.kind);
    let stats = kind.base_stats();
    let size = stats.size;
    let ramp_ticks = stats
        .construction
        .expect("non-Foundry salvage targets are buildable")
        .build_ticks;
    let tile = state.unit(id).expect("caller checked").tile();
    if tile_adjacent_to_rect(tile, anchor, size) {
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
    } else if !approach_rect(state, id, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// The harvest loop: work one safe remembered source inside a fixed local
/// zone, haul to a Foundry, and return to that same zone. Nodes are worked
/// from an adjacent tile (they block ground); wrecks are worked standing
/// *on* the tile — they are junk on open ground.
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
    match current.kind {
        SourceKind::Scrap => {
            let authoritative = node == anchor;
            let safe_footing = authoritative || !danger.contains(tile);
            if tile_adjacent_to_rect(tile, node, (1, 1)) && safe_footing {
                extract(state, id, node, hstats.ticks_per_scrap, events);
            } else if !approach_source(state, danger, id, current, authoritative) {
                source_route_failed(state, danger, id, node, anchor, events);
            }
        }
        SourceKind::Wreck => {
            if tile == node {
                extract_wreck(state, id, node, hstats.ticks_per_scrap);
            } else if !approach_source(state, danger, id, current, node == anchor) {
                source_route_failed(state, danger, id, node, anchor, events);
            }
        }
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
/// stagger on this period, keyed by unit id so a fleet never re-plans
/// in unison; near-zone threats never wait.
const HARVEST_REPLAN_PERIOD: u64 = 4;

/// Whether this is the tick on which `id` may re-plan a retained route
/// the danger lookahead has flagged beyond the react zone.
fn danger_replan_window(state: &State, id: UnitId) -> bool {
    (state.tick + u64::from(id.0)).is_multiple_of(HARVEST_REPLAN_PERIOD)
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
    candidates.sort_by_key(|(distance, source)| (*distance, source.pos.y, source.pos.x));
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
        let key = (
            distance,
            route_len,
            Reverse(source.amount),
            pos.chebyshev(anchor),
            kind_key,
            pos.y,
            pos.x,
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
    safe_source_route(state, danger, id, source).map(|(_, route)| route.len())
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
    let goal_matches = |goal: TilePos| match source.kind {
        SourceKind::Wreck => goal == source.pos,
        SourceKind::Scrap => tile_adjacent_to_rect(goal, source.pos, (1, 1)),
    };
    let from = unit.tile();
    let keep = unit.path.as_ref().is_some_and(|path| {
        goal_matches(path.goal)
            && keep_flagged_route(state, id, path, |waypoint| {
                danger.route_safe_from(from, waypoint)
            })
    });
    if keep {
        return true;
    }

    let Some((goal, waypoints)) = safe_source_route(state, danger, id, source) else {
        state.unit_mut(id).expect("caller checked").path = None;
        return false;
    };
    state.unit_mut(id).expect("caller checked").path = Some(PathFollow {
        goal,
        waypoints,
        next: 0,
    });
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
    let goal_matches = |goal: TilePos| match source.kind {
        SourceKind::Wreck => goal == source.pos,
        SourceKind::Scrap => tile_adjacent_to_rect(goal, source.pos, (1, 1)),
    };
    if let Some(path) = unit.path.as_ref().filter(|path| goal_matches(path.goal)) {
        let near_route_is_clear = keep_flagged_route(state, id, path, |waypoint| {
            known_ground_passable(state, danger, player, waypoint)
                && danger.route_safe_from(from, waypoint)
        });
        if near_route_is_clear {
            return true;
        }
        if let Some((goal, waypoints)) = authoritative_source_route(state, danger, id, source) {
            state.unit_mut(id).expect("caller checked").path = Some(PathFollow {
                goal,
                waypoints,
                next: 0,
            });
        }
        // No safe detour means the explicitly ordered route remains in
        // force. This is the only path allowed to cross known danger.
        return true;
    }

    if let Some((goal, waypoints)) = authoritative_source_route(state, danger, id, source) {
        state.unit_mut(id).expect("caller checked").path = Some(PathFollow {
            goal,
            waypoints,
            next: 0,
        });
        return true;
    }

    match source.kind {
        SourceKind::Scrap => approach_rect(state, id, source.pos, (1, 1)),
        SourceKind::Wreck => {
            let unit = state.unit(id).expect("caller checked");
            let Some(waypoints) = route_for(state, unit.kind, unit.tile(), source.pos) else {
                state.unit_mut(id).expect("caller checked").path = None;
                return false;
            };
            state.unit_mut(id).expect("caller checked").path = Some(PathFollow {
                goal: source.pos,
                waypoints,
                next: 0,
            });
            true
        }
    }
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
) -> Option<(TilePos, Vec<TilePos>)> {
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
) -> Option<(TilePos, Vec<TilePos>)> {
    safe_source_route(state, danger, id, source)
        .or_else(|| source_route_avoiding_danger(state, danger, id, source, true))
}

fn source_route_avoiding_danger(
    state: &State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    source: KnownSource,
    allow_dangerous_goal: bool,
) -> Option<(TilePos, Vec<TilePos>)> {
    let unit = state.unit(id).expect("caller checked");
    let from = unit.tile();
    let player = unit.player;
    let safe_route = |goal| {
        danger.find_route(from, goal, |tile| {
            known_ground_passable(state, danger, player, tile)
                && ((allow_dangerous_goal && tile == goal) || danger.route_safe_from(from, tile))
        })
    };
    match source.kind {
        SourceKind::Wreck => safe_route(source.pos).map(|route| (source.pos, route)),
        SourceKind::Scrap => {
            let mut candidates: Vec<TilePos> = rect_adjacent_tiles(source.pos, (1, 1))
                .filter(|tile| known_ground_passable(state, danger, player, *tile))
                .filter(|tile| allow_dangerous_goal || !danger.contains(*tile))
                .collect();
            candidates.sort_by_key(|tile| tile.chebyshev(from));
            let near = candidates.len().min(4);
            if near > 1 {
                candidates[..near].rotate_left(id.0 as usize % near);
            }
            let mut reachability = None;
            best_candidate_route(&candidates, from, |rank, goal| {
                if reachability
                    .as_ref()
                    .is_some_and(|reachable: &Vec<bool>| !reachable[rank])
                {
                    return None;
                }
                let route = safe_route(goal);
                if route.is_none() && reachability.is_none() {
                    reachability =
                        danger.last_route_reachability(&candidates, allow_dangerous_goal);
                }
                route
            })
        }
    }
}

/// Finds the route-minimal doorstep without running A* after every remaining
/// candidate's geometric lower bound can no longer beat the current winner.
type RankedRoute = ((usize, i32, usize), TilePos, Vec<TilePos>);

fn best_candidate_route(
    candidates: &[TilePos],
    from: TilePos,
    mut route_to: impl FnMut(usize, TilePos) -> Option<Vec<TilePos>>,
) -> Option<(TilePos, Vec<TilePos>)> {
    let mut best: Option<RankedRoute> = None;
    for (rank, &goal) in candidates.iter().enumerate() {
        if let Some(route) = route_to(rank, goal) {
            let key = (route.len(), goal.chebyshev(from), rank);
            if best.as_ref().is_none_or(|(old, _, _)| key < *old) {
                best = Some((key, goal, route));
            }
        }
        let remaining_lower_bound = candidates[rank + 1..]
            .iter()
            .map(|goal| goal.chebyshev(from) as usize)
            .min();
        if let (Some((key, _, _)), Some(lower_bound)) = (&best, remaining_lower_bound)
            && key.0 <= lower_bound
        {
            break;
        }
    }
    best.map(|(_, goal, route)| (goal, route))
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
fn try_drop_offs(
    state: &mut State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    drop_offs: &[BuildingId],
) -> bool {
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
                && keep_flagged_route(state, id, path, |waypoint| {
                    known_ground_passable(state, danger, player, waypoint)
                        && danger.route_safe_from(from, waypoint)
                })
            {
                return true;
            }
        }
        if let Some((goal, waypoints)) =
            known_rect_route(state, danger, id, anchor, size, true, Some(&mut scan))
        {
            state.unit_mut(id).expect("caller checked").path = Some(PathFollow {
                goal,
                waypoints,
                next: 0,
            });
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
            // Danger-blocked, not sealed: stand and wait for the window.
            return true;
        }
    }
    false
}

fn known_rect_route(
    state: &State,
    danger: &GroundSalvageDanger,
    id: UnitId,
    anchor: TilePos,
    size: (i32, i32),
    avoid_danger: bool,
    mut scan: Option<&mut DropOffScan>,
) -> Option<(TilePos, Vec<TilePos>)> {
    let unit = state.unit(id).expect("caller checked");
    let from = unit.tile();
    let player = unit.player;
    let mut candidates: Vec<TilePos> = rect_adjacent_tiles(anchor, size)
        .filter(|tile| known_ground_passable(state, danger, player, *tile))
        .filter(|tile| !avoid_danger || !danger.contains(*tile))
        .collect();
    candidates.sort_by_key(|tile| tile.chebyshev(from));
    let near = candidates.len().min(4);
    if near > 1 {
        candidates[..near].rotate_left(id.0 as usize % near);
    }
    // The passability predicate is candidate-independent, so one failed
    // search that exhausted the walkable component has already decided
    // every remaining doorstep — including a prior same-scan foundry's
    // flood, which the scratch still holds at entry. Skipping a proven
    // tile returns the identical None without re-flooding to the cap.
    let mut reachability = None;
    if scan.as_deref().is_some_and(|scan| scan.flood_exhausted) {
        reachability = danger.last_route_reachability(&candidates, false);
    }
    for (rank, &goal) in candidates.iter().enumerate() {
        if reachability
            .as_ref()
            .is_some_and(|reachable: &Vec<bool>| !reachable[rank])
        {
            continue;
        }
        let route = danger.find_route(from, goal, |tile| {
            known_ground_passable(state, danger, player, tile)
                && (!avoid_danger || danger.route_safe_from(from, tile))
        });
        if let Some(waypoints) = route {
            return Some((goal, waypoints));
        }
        if reachability.is_none() {
            reachability = danger.last_route_reachability(&candidates, false);
            if reachability.is_some()
                && let Some(scan) = scan.as_deref_mut()
            {
                scan.flood_exhausted = true;
            }
        }
    }
    None
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

/// Stand on the wreck and strip it. Decay can beat the stripper to the
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
    let (tile, me, carrying) = (unit.tile(), unit.player, unit.carrying);
    let drop_offs = drop_offs_by_distance(state, id);
    let at_drop_off = drop_offs.iter().any(|foundry_id| {
        state.building(*foundry_id).is_some_and(|foundry| {
            tile_adjacent_to_rect(tile, foundry.anchor, foundry.stats().size)
        })
    });
    if at_drop_off {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.carrying = 0;
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
            player: me,
            amount: credited,
        });
        if retiring {
            state.unit_mut(id).expect("caller checked").advance_queue();
        }
        return;
    }

    if try_drop_offs(state, danger, id, &drop_offs) {
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
    if drop_offs.iter().any(|foundry_id| {
        let foundry = state
            .building(*foundry_id)
            .expect("collected live drop-off");
        tile_adjacent_to_rect(tile, foundry.anchor, foundry.stats().size)
    }) {
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    }
    if try_drop_offs(state, danger, id, &drop_offs) {
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
    fn the_zone_radius_covers_the_widest_connected_shipped_deposit() {
        // Compass Grand and Trident Plateau carry center deposits whose
        // endpoint span is seven tiles. The work zone is anchored rather
        // than re-centered after each hop, so this exact reach cannot
        // walk onward into a second field.
        assert_eq!(HARVEST_ZONE_RADIUS, 7);
    }

    #[test]
    fn doorstep_search_stops_only_after_remaining_routes_cannot_win() {
        use std::cell::Cell;

        let from = TilePos::new(0, 0);
        let candidates = [TilePos::new(4, 0), TilePos::new(3, 0), TilePos::new(5, 0)];
        let route_len = |goal: TilePos| match goal.x {
            3 => 3,
            4 => 5,
            5 => 5,
            _ => unreachable!(),
        };
        let exhaustive = candidates
            .iter()
            .enumerate()
            .map(|(rank, &goal)| ((route_len(goal), goal.chebyshev(from), rank), goal))
            .min_by_key(|(key, _)| *key)
            .map(|(_, goal)| goal);
        let calls = Cell::new(0);
        let chosen = best_candidate_route(&candidates, from, |_, goal| {
            calls.set(calls.get() + 1);
            Some(vec![goal; route_len(goal)])
        })
        .map(|(goal, _)| goal);

        assert_eq!(chosen, exhaustive);
        assert_eq!(calls.get(), 2, "the dominated final A* is skipped");
    }

    #[test]
    fn doorstep_pruning_matches_exhaustive_selection_across_ties_and_failures() {
        use std::cell::Cell;

        struct Case {
            name: &'static str,
            candidates: Vec<TilePos>,
            route_lengths: Vec<Option<usize>>,
            expected_calls: usize,
        }
        let cases = [
            Case {
                name: "first unreachable",
                candidates: vec![TilePos::new(1, 0), TilePos::new(2, 0), TilePos::new(3, 0)],
                route_lengths: vec![None, Some(2), Some(3)],
                expected_calls: 2,
            },
            Case {
                name: "later shorter geometric candidate",
                candidates: vec![TilePos::new(4, 0), TilePos::new(2, 0), TilePos::new(5, 0)],
                route_lengths: vec![Some(6), Some(2), Some(5)],
                expected_calls: 2,
            },
            Case {
                name: "equal length and distance keeps earlier rotated rank",
                candidates: vec![TilePos::new(-2, 0), TilePos::new(2, 0), TilePos::new(3, 0)],
                route_lengths: vec![Some(2), Some(2), Some(3)],
                expected_calls: 1,
            },
            Case {
                name: "equal length keeps smaller distance",
                candidates: vec![TilePos::new(2, 0), TilePos::new(3, 0)],
                route_lengths: vec![Some(3), Some(3)],
                expected_calls: 1,
            },
            Case {
                name: "rotated nearest four",
                candidates: vec![
                    TilePos::new(4, 0),
                    TilePos::new(2, 0),
                    TilePos::new(1, 0),
                    TilePos::new(3, 0),
                    TilePos::new(5, 0),
                ],
                route_lengths: vec![Some(4), Some(2), Some(1), Some(3), Some(5)],
                expected_calls: 3,
            },
        ];
        let from = TilePos::new(0, 0);
        for case in cases {
            for (goal, route_len) in case.candidates.iter().zip(&case.route_lengths) {
                if let Some(route_len) = route_len {
                    assert!(
                        *route_len >= goal.chebyshev(from) as usize,
                        "{} violates the geometric lower bound",
                        case.name
                    );
                }
            }
            let exhaustive = case
                .candidates
                .iter()
                .copied()
                .enumerate()
                .filter_map(|(rank, goal)| {
                    case.route_lengths[rank].map(|len| ((len, goal.chebyshev(from), rank), goal))
                })
                .min_by_key(|(key, _)| *key)
                .map(|(_, goal)| goal);
            let calls = Cell::new(0);
            let optimized = best_candidate_route(&case.candidates, from, |rank, goal| {
                calls.set(calls.get() + 1);
                case.route_lengths[rank].map(|len| vec![goal; len])
            })
            .map(|(goal, _)| goal);
            assert_eq!(optimized, exhaustive, "{}", case.name);
            assert_eq!(calls.get(), case.expected_calls, "{}", case.name);
        }
    }

    #[test]
    fn replacement_preserves_worker_affinity_before_route_efficiency() {
        let state = Scenario {
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
    fn unseen_wreck_selection_reads_frozen_memory_not_live_salvage() {
        let mut state = Scenario {
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
        let clear = scenario.clone().build().unwrap();
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
