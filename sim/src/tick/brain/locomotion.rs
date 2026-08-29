//! Getting there: idle auto-acquire, advance and attack-move routing,
//! plain walking, contact-propagated arrival, and doorstep approach.

use super::super::landing::{self, Pick, RunIn};
use super::super::{
    flight, rect_adjacent_tiles, rect_approach_key_from, rect_approach_origin, route_for,
    route_for_position, tile_adjacent_to_rect,
};
use super::combat::{acquire_target, acquire_target_from};
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
        // An airframe with nothing to do orbits for a while, then sets
        // itself down on the nearest clear tile. The order is written
        // directly rather than assigned so `settled` survives: a parked
        // aircraft is a stationed guard whose fights tether to its pad.
        let stats = unit.kind.stats();
        let wants_ground = stats.turn_rate > 0
            && !unit.landed
            && unit.settled >= crate::stats::AUTO_LAND_IDLE_TICKS;
        if wants_ground {
            let (tile, pos, heading) = (unit.tile(), unit.pos, unit.heading);
            if let Some(goal) = landing::nearest_landable(
                state,
                stats,
                id,
                tile,
                pos,
                heading,
                crate::stats::AUTO_LAND_SCAN_RADIUS,
                None,
                Pick::StraightIn,
            ) {
                let unit = state.unit_mut(id).expect("caller checked");
                unit.order = Order::Land { goal };
                unit.path = None;
            }
        }
    }
}

/// Flies the run-in onto `goal` and sets the airframe down on its center.
/// Touchdown belongs here, not to the steering ring: the final leg is
/// never accepted early, so a pass either meets the center within
/// [`crate::stats::LANDING_TOUCHDOWN`] or flies through and comes around
/// for another run.
pub(super) fn land(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    id: UnitId,
    goal: TilePos,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if stats.turn_rate == 0 || unit.landed {
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    }
    // Nothing sets down with an enemy in reach of the tile: what the
    // approach brings into sight turns the landing into the fight an idle
    // unit would pick, rather than a one-tick touchdown followed by a
    // scramble. Judged from the tile, so a retreat past a gun still
    // completes.
    if let Some(target) = acquire_target_from(state, index, id, goal.center()) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target,
            resume: None,
        };
        unit.path = None;
        return;
    }
    let (pos, heading, kind) = (unit.pos, unit.heading, unit.kind);
    let center = goal.center();
    let radius = stats.turn_radius();
    let touchdown = crate::stats::LANDING_TOUCHDOWN;
    if pos.dist_sq(center) <= touchdown * touchdown {
        if !landing::landing_clear(state, stats, id, goal, pos) {
            // The tile filled during the approach: go around onto the
            // nearest clear one, or give up when there is none.
            let next = landing::nearest_landable(
                state,
                stats,
                id,
                goal,
                pos,
                heading,
                crate::stats::LANDING_REPLAN_RADIUS,
                Some(goal),
                Pick::StraightIn,
            );
            let unit = state.unit_mut(id).expect("caller checked");
            match next {
                Some(goal) => {
                    unit.order = Order::Land { goal };
                    unit.path = None;
                }
                None => {
                    let (player, pos) = (unit.player, unit.pos);
                    unit.clear_program();
                    events.push(Event::OrderStalled {
                        unit: id,
                        player,
                        pos,
                        reason: StallReason::NoOpenGround,
                    });
                }
            }
            return;
        }
        // Judged where the airframe will actually rest, which is what the
        // validator holds a parked heading to.
        if !flight::escapable(state.map(), pos, heading, radius) {
            // Arrived on a heading no takeoff could fly out of: missed
            // approach, plan a fresh run-in.
            state.unit_mut(id).expect("caller checked").path = None;
            return;
        }
        // The airframe rests where it met the tile: no snap to the center,
        // so the touchdown reads as the end of the run rather than a hop.
        let unit = state.unit_mut(id).expect("caller checked");
        unit.landed = true;
        unit.path = None;
        unit.advance_queue();
        return;
    }
    let on_final = unit
        .path
        .as_ref()
        .is_some_and(|p| p.goal == goal && p.next as usize + 1 == p.waypoints.len());
    if on_final {
        let path = unit.path.as_ref().expect("checked above");
        let len = path.waypoints.len();
        if len >= 2 {
            // The final leg runs from the initial point to the tile. The
            // aircraft chases a carrot on that centerline a couple of turn
            // radii ahead of its own projection, which settles it onto the
            // line from whatever heading it reached the point on, then
            // walks the carrot down to the tile itself.
            let bearing = flight::heading_of(center - path.waypoints[len - 2].center());
            let v = chassis::compass::dir(bearing);
            let d = center - pos;
            let ahead = v.x * d.x + v.y * d.y;
            if ahead < chassis::fx::Fx::ZERO {
                // Overflew the tile without touching down: the leg is spent.
                state.unit_mut(id).expect("caller checked").path = None;
            } else {
                let lookahead = radius + radius;
                let short = (ahead - lookahead).max(chassis::fx::Fx::ZERO);
                let carrot = TilePos::containing(center - v * short);
                let unit = state.unit_mut(id).expect("caller checked");
                let path = unit.path.as_mut().expect("checked above");
                let last = path.waypoints.len() - 1;
                path.waypoints[last] = carrot;
                path.next = last as u32;
                return;
            }
        } else {
            let hv = chassis::compass::dir(heading);
            let d = center - pos;
            let behind = hv.x * d.x + hv.y * d.y < chassis::fx::Fx::ZERO;
            let accept = stats.turn_acceptance();
            if !(behind && pos.dist_sq(center) <= accept * accept) {
                return;
            }
            state.unit_mut(id).expect("caller checked").path = None;
        }
    }
    if state
        .unit(id)
        .expect("caller checked")
        .path
        .as_ref()
        .is_some_and(|p| p.goal == goal)
    {
        return;
    }
    let route = landing::run_in_route(state, stats, kind, pos, heading, goal, RunIn::Landing);
    let unit = state.unit_mut(id).expect("caller checked");
    match route {
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
    if land_at_destination(state, index, id, goal) {
        return;
    }
    walk(state, id, goal, events);
}

/// A flier's ground destination is a landing. Once the last step of its
/// program is within run-in reach of the goal and nothing is in
/// acquisition range, the flight hands over to a landing on that tile, or
/// on the nearest landable one. Returns false when the order stands: a
/// queued follow-up, a patrol loop, an enemy in reach, or no ground to
/// park on all keep the plain arrival contract.
pub(super) fn land_at_destination(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    id: UnitId,
    goal: TilePos,
) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if stats.turn_rate == 0 || unit.looping || !unit.queue.is_empty() {
        return false;
    }
    let reach = crate::stats::LANDING_HANDOFF_REACH;
    if unit.pos.dist_sq(goal.center()) > reach * reach {
        return false;
    }
    if acquire_target_from(state, index, id, goal.center()).is_some() {
        return false;
    }
    let (pos, heading) = (unit.pos, unit.heading);
    let pad = landing::nearest_landable(
        state,
        stats,
        id,
        goal,
        pos,
        heading,
        crate::stats::GOAL_SNAP_RADIUS,
        None,
        Pick::Nearest,
    );
    let Some(pad) = pad else {
        return false;
    };
    let unit = state.unit_mut(id).expect("caller checked");
    unit.order = Order::Land { goal: pad };
    unit.path = None;
    true
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
            && other.domain() == unit.domain()
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
    let (tile, kind, player) = {
        let u = state.unit(id).expect("caller checked");
        (u.tile(), u.kind, u.player)
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
    let approach_from = rect_approach_origin(state, player, tile, anchor, size);
    candidates.sort_by_key(|t| rect_approach_key_from(tile, approach_from, anchor, size, *t));
    let near = candidates.len().min(4);
    if near > 1 {
        let rank = crate::ids::owner_local_unit_rank(
            id,
            player,
            state.units.iter().map(|unit| (unit.id, unit.player)),
        );
        candidates[..near].rotate_left(rank % near);
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
