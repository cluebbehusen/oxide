//! Getting there: idle auto-acquire, attack-move, plain walking,
//! contact-propagated arrival, and doorstep approach.

use super::super::{rect_adjacent_tiles, route_for, tile_adjacent_to_rect};
use super::combat::acquire_target;
use crate::event::{Event, StallReason};
use crate::ids::UnitId;
use crate::state::{Order, PathFollow, State};
use chassis::grid::TilePos;

/// Idle combat units pick fights on their own.
pub(super) fn idle(state: &mut State, id: UnitId) {
    if let Some(target) = acquire_target(state, id) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target,
            resume: None,
        };
        unit.path = None;
    }
}

/// March toward the goal, but engage anything that shows up on the way;
/// the attack order remembers the goal and hands it back afterwards.
pub(super) fn attack_move(state: &mut State, id: UnitId, goal: TilePos, events: &mut Vec<Event>) {
    if let Some(target) = acquire_target(state, id) {
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
    if tile == goal || touching_settled_arrival(state, id, goal) {
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    }
    let unit = state.unit(id).expect("caller checked");
    let has_fresh_path = unit.path.as_ref().is_some_and(|p| p.goal == goal);
    if has_fresh_path {
        return;
    }
    let (tile, kind) = (unit.tile(), unit.kind);
    let path = route_for(state, kind, tile, goal);
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
