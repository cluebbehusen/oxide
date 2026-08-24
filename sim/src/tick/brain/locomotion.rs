//! Getting there: idle auto-acquire, advance and attack-move routing,
//! plain walking, contact-propagated arrival, and doorstep approach.

use super::super::{rect_adjacent_tiles, route_for, route_for_position, tile_adjacent_to_rect};
use super::combat::acquire_target;
use crate::event::{Event, StallReason};
use crate::ids::UnitId;
use crate::state::{Order, PathFollow, State};
use chassis::grid::TilePos;

/// Idle combat units pick fights on their own — on a tether. The
/// leash is set here (and refreshed by retaliation), never by player
/// commands: an explicit attack is a commitment, and `assign` clears
/// any tether the moment a command lands.
pub(super) fn idle(state: &mut State, index: &super::super::spatial::UnitIndex, id: UnitId) {
    // A guard back at its post cools down before it looks for the next
    // fight; the leash clears when the cooldown drains — and the guard
    // is instantly STATIONED again (it verifiably stood the whole
    // cooldown), so the dancer finds no untethered window to bait.
    // Idle with a spent tether and no cooldown means the homecoming
    // just finished (walk's arrival advanced the queue) — arm the
    // post stand.
    if let Some(leash) = state.unit(id).expect("caller checked").leash {
        let unit = state.unit_mut(id).expect("caller checked");
        match leash.cooldown {
            0 => {
                unit.leash.as_mut().expect("just seen").cooldown =
                    crate::stats::LEASH_REACQUIRE_COOLDOWN
            }
            1 => {
                unit.leash = None;
                unit.settled = crate::stats::LEASH_STATION_TICKS;
            }
            _ => unit.leash.as_mut().expect("just seen").cooldown -= 1,
        }
        return;
    }
    if let Some(target) = acquire_target(state, index, id) {
        let unit = state.unit_mut(id).expect("caller checked");
        let anchor = unit.tile();
        let stationed = unit.settled >= crate::stats::LEASH_STATION_TICKS;
        unit.order = Order::Attack {
            target,
            resume: None,
        };
        unit.path = None;
        unit.settled = 0;
        // Only a stationed guard's fight tethers — a unit cycling
        // through idle mid-battle hunts unleashed, like it always
        // did. No blood yet either way: the warm window starts
        // empty, so a bait that never comes in reach is dropped at
        // the radius line exactly.
        if stationed {
            unit.leash = Some(crate::state::Leash {
                anchor,
                patience: 0,
                cooldown: 0,
            });
        }
    } else {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.settled = unit.settled.saturating_add(1);
    }
}

/// March toward the goal, but engage anything that shows up on the way;
/// the attack order remembers the goal and hands it back afterwards.
pub(super) fn attack_move(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    id: UnitId,
    goal: TilePos,
    events: &mut Vec<Event>,
) {
    if let Some(target) = acquire_target(state, index, id) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target,
            resume: Some(goal),
        };
        unit.path = None;
        return;
    }
    walk(state, id, goal, events);
}

/// Walks toward an exact goal tile; going idle on arrival or when no route
/// exists. A unit close to the goal that bumps into an already-settled
/// arrival also counts as arrived — the whole group parks instead of
/// churning around the click point forever.
pub(super) fn walk(state: &mut State, id: UnitId, goal: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let tile = unit.tile();
    // A bounded-turn flier cannot promise an exact tile center — the
    // ring the steering integrator accepts is the arrival contract.
    let arced_in = unit.kind.stats().turn_rate > 0 && {
        let accept = unit.kind.stats().turn_acceptance();
        unit.pos.dist_sq(goal.center()) <= accept * accept
    };
    if tile == goal || arced_in || touching_settled_arrival(state, id, goal) {
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    }
    let unit = state.unit(id).expect("caller checked");
    let has_fresh_path = unit.path.as_ref().is_some_and(|p| p.goal == goal);
    if has_fresh_path {
        return;
    }
    let (pos, kind) = (unit.pos, unit.kind);
    let path = route_for_position(state, kind, pos, goal);
    let unit = state.unit_mut(id).expect("caller checked");
    match path {
        Some(waypoints) => {
            unit.path = Some(PathFollow {
                goal,
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

/// Whether this near-goal unit is in contact with a settled (idle,
/// pathless) unit that itself sits near the same goal — the arrival wave
/// propagates outward from the first unit to park.
fn touching_settled_arrival(state: &State, id: UnitId, goal: TilePos) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let near_sq = crate::stats::ARRIVAL_NEAR * crate::stats::ARRIVAL_NEAR;
    let goal_center = goal.center();
    if unit.pos.dist_sq(goal_center) > near_sq {
        return false;
    }
    let my_stats = unit.kind.stats();
    let my_radius = my_stats.radius;
    let contact_slack = chassis::fx::Fx::lit("0.05");
    // Contact only means anything between bodies that collide: a flyer
    // hovering over a parked crowd is not "touching" it.
    state.units.iter().any(|other| {
        other.id != id
            && other.hp > 0
            && other.kind.stats().domain == my_stats.domain
            && other.path.is_none()
            && other.order == Order::Idle
            && other.pos.dist_sq(goal_center) <= near_sq
            && unit.pos.dist(other.pos) <= my_radius + other.kind.stats().radius + contact_slack
    })
}

/// Ensures the unit is walking to some passable tile touching the rectangle.
/// Returns false when no ring tile is reachable.
pub(super) fn approach_rect(
    state: &mut State,
    id: UnitId,
    anchor: TilePos,
    size: (i32, i32),
) -> bool {
    let (tile, kind) = {
        let u = state.unit(id).expect("caller checked");
        (u.tile(), u.kind)
    };
    let keep = state
        .unit(id)
        .expect("caller checked")
        .path
        .as_ref()
        .is_some_and(|p| tile_adjacent_to_rect(p.goal, anchor, size));
    if keep {
        return true;
    }
    // Candidate doorsteps, nearest first (stable sort, so ties stay in
    // ring order). The nearest few are then rotated by unit id so a crowd
    // heading for the same rectangle fans out across doorsteps instead of
    // magnetizing onto one tile and jamming — the exact configuration that
    // froze bot economies. Only the near face rotates: a lone unit never
    // detours to the building's far side.
    let domain = kind.stats().domain;
    let mut candidates: Vec<TilePos> = rect_adjacent_tiles(anchor, size)
        .filter(|&t| state.passable_for(domain, t))
        .collect();
    candidates.sort_by_key(|t| t.chebyshev(tile));
    let near = candidates.len().min(4);
    if near > 1 {
        candidates[..near].rotate_left(id.0 as usize % near);
    }
    for goal in candidates {
        if let Some(waypoints) = route_for(state, kind, tile, goal) {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                goal,
                waypoints,
                next: 0,
            });
            return true;
        }
    }
    false
}
