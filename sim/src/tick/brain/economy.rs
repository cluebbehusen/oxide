//! The working half of the brain: construction, repair welding,
//! harvesting, wreck stripping, and delivery. Hp gains buffer like
//! damage and resolve after it — fire wins ties.

use super::super::{route_for, tile_adjacent_to_rect};
use super::PendingHpGain;
use super::locomotion::approach_rect;
use crate::event::{Event, StallReason};
use crate::ids::UnitId;
use crate::state::{Order, PathFollow, State};
use crate::stats::RETARGET_RADIUS;
use chassis::grid::TilePos;

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
    let stats = kind.stats();
    let size = stats.size;
    let build_ticks = stats
        .construction
        .expect("sites only exist for buildable kinds")
        .build_ticks;
    let tile = state.unit(id).expect("caller checked").tile();
    if tile_adjacent_to_rect(tile, anchor, size) {
        let start_hp = stats.max_hp / 5;
        let ramp = stats.max_hp - start_hp;
        let b = state.building_mut(site).expect("just seen");
        let step = (ramp * (b.progress + 1) / build_ticks) - (ramp * b.progress / build_ticks);
        b.progress += 1;
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
    let size = kind.stats().size;
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
        .filter(|b| b.player == me && b.built && b.hp > 0 && b.hp < b.kind.stats().max_hp)
    else {
        // Healed, destroyed, or never a patient: the job is over.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let (anchor, kind) = (b.anchor, b.kind);
    let stats = kind.stats();
    let size = stats.size;
    // The welding rate is the construction ramp; the unbuyable Foundry
    // repairs on an authored ramp (and billing basis) of its own.
    let (ramp_ticks, basis) = stats.construction.map_or(
        (
            crate::stats::FOUNDRY_REPAIR_TICKS,
            crate::stats::FOUNDRY_REPAIR_PRICE,
        ),
        |c| (c.build_ticks, c.cost),
    );
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
    let stats = kind.stats();
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

/// The harvest loop: walk to the salvage, extract to capacity, haul to
/// the nearest Foundry, repeat; when the source dies, hop to a neighbor
/// source or go idle. Nodes are worked from an adjacent tile (they block
/// ground); wrecks are worked standing *on* the tile — they are junk on
/// open ground.
pub(super) fn harvest(state: &mut State, id: UnitId, node: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let Some(hstats) = unit.kind.stats().harvest else {
        // Only harvesters ever get this order; be defensive anyway.
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    };
    let (tile, kind, carrying) = (unit.tile(), unit.kind, unit.carrying);
    let node_scrap = state.map.scrap_at(node);
    let node_wreck = state.map.wreck_at(node);

    if carrying >= hstats.capacity {
        deliver(state, id, node, events);
    } else if node_scrap > 0 {
        if tile_adjacent_to_rect(tile, node, (1, 1)) {
            extract(state, id, node, hstats.ticks_per_scrap, events);
        } else if !approach_rect(state, id, node, (1, 1)) {
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
    } else if node_wreck > 0 {
        if tile == node {
            extract_wreck(state, id, node, hstats.ticks_per_scrap);
        } else {
            let keep = state
                .unit(id)
                .expect("caller checked")
                .path
                .as_ref()
                .is_some_and(|p| p.goal == node);
            if !keep {
                let path = route_for(state, kind, tile, node);
                let unit = state.unit_mut(id).expect("caller checked");
                match path {
                    Some(waypoints) => {
                        unit.path = Some(PathFollow {
                            goal: node,
                            waypoints,
                            next: 0,
                        });
                    }
                    None => {
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
        }
    } else {
        // Dry. Find a replacement source, else wrap up.
        match replacement_node(state, node, tile) {
            Some(next) => {
                let unit = state.unit_mut(id).expect("caller checked");
                unit.order = Order::Harvest { node: next };
                unit.path = None;
                unit.progress = 0;
            }
            None if carrying > 0 => deliver(state, id, node, events),
            None => state.unit_mut(id).expect("caller checked").advance_queue(),
        }
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

/// Haul the load to the nearest own Foundry; deposit when adjacent. After
/// depositing, resume the node (or go idle if it is gone for good).
fn deliver(state: &mut State, id: UnitId, node: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let (tile, me, carrying) = (unit.tile(), unit.player, unit.carrying);
    let pos = unit.pos;

    // Only a built Foundry takes deliveries — turrets and half-standing
    // sites are not drop-offs, however conveniently they're placed.
    let nearest = state
        .buildings
        .iter()
        .filter(|b| {
            b.player == me && b.hp > 0 && b.built && b.kind == crate::stats::BuildingKind::Foundry
        })
        .map(|b| (pos.dist_sq(b.center()), b.id))
        .min();
    let Some((_, foundry_id)) = nearest else {
        // Homeless: hold the scrap; the harvest is over, but a queued
        // program can still go on.
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    };
    let foundry = state.building(foundry_id).expect("just found");
    let (anchor, size) = (foundry.anchor, foundry.kind.stats().size);

    if tile_adjacent_to_rect(tile, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.carrying = 0;
        unit.progress = 0;
        unit.path = None;
        // Saturating: a hostile scenario can start a bank near u32::MAX.
        // The event reports what was actually credited, not what was
        // carried — at the ceiling those differ.
        let bank = &mut state.player_mut(me).scrap;
        let credited = bank.saturating_add(carrying) - *bank;
        *bank += credited;
        events.push(Event::ScrapDeposited {
            player: me,
            amount: credited,
        });
        // Nothing left to go back to? Then we're done hauling.
        if state.map.scrap_at(node) == 0
            && state.map.wreck_at(node) == 0
            && replacement_node(state, node, tile).is_none()
        {
            state.unit_mut(id).expect("caller checked").advance_queue();
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

/// The nearest tile still holding node scrap within [`RETARGET_RADIUS`]
/// of a dead source, keyed by (distance from the unit, y, x) so the pick
/// is unique. Wreck tiles are deliberately not candidates: battlefield
/// salvage is directed work (an explicit Harvest still reaches it), and
/// an auto-hop that accepted wrecks chain-walked harvest lines across
/// old battlefields toward the enemy.
fn replacement_node(state: &State, around: TilePos, unit_tile: TilePos) -> Option<TilePos> {
    let mut best: Option<(i32, i32, i32)> = None;
    for dy in -RETARGET_RADIUS..=RETARGET_RADIUS {
        for dx in -RETARGET_RADIUS..=RETARGET_RADIUS {
            let t = around.offset(dx, dy);
            if state.map.scrap_at(t) == 0 {
                continue;
            }
            let key = (t.manhattan(unit_tile), t.y, t.x);
            if best.is_none_or(|b| key < b) {
                best = Some(key);
            }
        }
    }
    best.map(|(_, y, x)| TilePos::new(x, y))
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
