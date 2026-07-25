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
        let p = state.unit(id).expect("caller checked").progress;
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
        unit.progress += 1;
        let step = (ramp * (p + 1) / ramp_ticks) - (ramp * p / ramp_ticks);
        if step > 0 {
            builds.push(PendingHpGain {
                site: building,
                step,
                completes: false,
                player: me,
                kind,
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
        let p = unit.progress;
        unit.progress += 1;
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

/// The nearest tile still holding salvage — node scrap or wreck — within
/// [`RETARGET_RADIUS`] of a dead source, keyed by (distance from the
/// unit, y, x) so the pick is unique.
fn replacement_node(state: &State, around: TilePos, unit_tile: TilePos) -> Option<TilePos> {
    let mut best: Option<(i32, i32, i32)> = None;
    for dy in -RETARGET_RADIUS..=RETARGET_RADIUS {
        for dx in -RETARGET_RADIUS..=RETARGET_RADIUS {
            let t = around.offset(dx, dy);
            if state.map.scrap_at(t) == 0 && state.map.wreck_at(t) == 0 {
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
