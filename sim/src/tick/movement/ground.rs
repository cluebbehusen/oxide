use chassis::compass::dir;
use chassis::compass::heading_of;
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;

use crate::state::{GroundTerrain, ParkedBodies, Unit};
use crate::stats::{
    GROUND_ACCEL_TICKS, GROUND_ALIGNED_STEPS, GROUND_BRAKE_TICKS, GROUND_PIVOT_THRESHOLD,
    ROUTE_LOOKAHEAD, WAYPOINT_ACCEPT,
};

fn increment(speed: Fx, ticks: i64) -> Fx {
    Fx::from_bits((speed.to_bits() + ticks - 1) / ticks)
}

/// Whether the body can still drive straight to its current waypoint. An
/// adjacent waypoint keeps the tile rules a route was planned under, so a
/// wide hull beside a wall never loses a leg it could always walk; a
/// waypoint reached by lookahead is held to the swept line that admitted
/// it, so ground claimed beside the leg drops it for a fresh route instead
/// of letting the hull graze the new site.
fn leg_open(unit: &Unit, waypoint: TilePos, terrain: &GroundTerrain) -> bool {
    let here = TilePos::containing(unit.pos);
    if here.chebyshev(waypoint) <= 1 {
        super::early_advance_safe(here, waypoint, terrain)
            || (!terrain.open(here)
                && terrain.contact_clear(
                    unit.pos,
                    waypoint.center(),
                    crate::tick::brain::contact::collision_radius(unit),
                ))
    } else {
        !swept_leg_blocked(
            unit.pos,
            waypoint.center(),
            unit.kind.stats().radius,
            |tile| terrain.open(tile),
        )
    }
}

/// [`chassis::path::swept_line_blocked`], answered first from the leg's
/// bounding box: every tile the sweep can test lies inside it, so a box with
/// no closed tile proves the leg open without walking its three lines.
fn swept_leg_blocked(a: Vec2Fx, b: Vec2Fx, radius: Fx, clear: impl Fn(TilePos) -> bool) -> bool {
    // Margin for the rounding in the sweep's edge offsets.
    let reach = radius + const { Fx::lit("0.015625") };
    let low = TilePos::containing(Vec2Fx::new(a.x.min(b.x) - reach, a.y.min(b.y) - reach));
    let high = TilePos::containing(Vec2Fx::new(a.x.max(b.x) + reach, a.y.max(b.y) + reach));
    let open = (low.y..=high.y).all(|y| (low.x..=high.x).all(|x| clear(TilePos::new(x, y))));
    !open && chassis::path::swept_line_blocked(a, b, radius, clear)
}

pub(super) fn path_point(path: &crate::state::PathFollow, index: usize) -> Vec2Fx {
    if index + 1 == path.waypoints.len() {
        path.final_point
            .unwrap_or_else(|| path.waypoints[index].center())
    } else {
        path.waypoints[index].center()
    }
}

/// The point to steer for, its index in the route, and whether it is the
/// route's final waypoint.
///
/// `path.next` stays the physical cursor: the first waypoint the body has
/// not yet reached or passed, so every consumer that scans the remaining
/// route (the harvest danger check above all) still sees every tile ahead.
/// The steering target is looked up fresh each tick as the furthest of the
/// next few waypoints the hull can reach on a straight, clear leg, so a grid
/// staircase is driven as one line and a corner is rounded only once the far
/// side is actually visible.
fn route_target(
    unit: &mut Unit,
    terrain: &GroundTerrain,
    parked: &ParkedBodies,
) -> Option<(Vec2Fx, usize, bool)> {
    let radius = unit.kind.stats().radius;
    let contact_radius = crate::tick::brain::contact::collision_radius(unit);
    loop {
        let path = unit.path.as_ref()?;
        let Some(&waypoint) = path.waypoints.get(path.next as usize) else {
            unit.path = None;
            return None;
        };
        let here = TilePos::containing(unit.pos);
        if !terrain.open(waypoint) || !leg_open(unit, waypoint, terrain) {
            unit.path = None;
            return None;
        }
        let path = unit.path.as_mut().expect("checked above");
        let next = path.waypoints.get(path.next as usize + 1).copied();
        if let Some(next) = next
            && (unit.pos.dist(waypoint.center()) <= WAYPOINT_ACCEPT
                || super::passed_intermediate_waypoint(unit.pos, waypoint, next, radius))
            && super::early_advance_safe(waypoint, next, terrain)
            && super::early_advance_safe(here, next, terrain)
        {
            path.next += 1;
            continue;
        }
        // A chassis at rest that already faces its next waypoint rolls
        // toward it now and finds the longer leg once under way; steering
        // for a farther target from rest would pivot first.
        let facing = unit.drive_speed == Fx::ZERO && {
            let bearing = heading_of(path_point(path, path.next as usize) - unit.pos);
            bearing
                .wrapping_sub(unit.heading)
                .cast_signed()
                .unsigned_abs()
                <= GROUND_ALIGNED_STEPS
        };
        // A friendly body at rest is never steered through: the planned
        // tiles go around it, and a straight leg must too.
        let clear = |tile: TilePos| terrain.open(tile) && !parked.blocks(tile);
        let cursor = path.next as usize;
        let mut target = cursor;
        while !facing
            && target < cursor + ROUTE_LOOKAHEAD
            && let Some(&candidate) = path.waypoints.get(target + 1)
            && clear(candidate)
            && !swept_leg_blocked(unit.pos, path_point(path, target + 1), radius, clear)
        {
            target += 1;
        }
        // A work point is admitted from its goal tile's center. The motor
        // holds a body inside the contact band to that precise clearance, so
        // a straight leg that grazes a neighboring footprint on the way in
        // would stop the hull short of the point for good.
        let mut point = path_point(path, target);
        if target + 1 == path.waypoints.len()
            && (TilePos::containing(point) != path.goal
                || terrain.at_contact(unit.pos, contact_radius))
            && !terrain.contact_clear(unit.pos, point, contact_radius)
        {
            point = path.goal.center();
        }
        // A body docked on a footprint tile was routed by tile rules, but
        // the surface it rests against still bounds its hull. When the
        // straight leg would graze that surface it backs out first; off the
        // footprint, the leg is judged by tile rules again.
        if !terrain.open(here)
            && !terrain.contact_clear(unit.pos, point, contact_radius)
            && let Some(exit) = terrain.contact_exit(unit.pos, contact_radius)
        {
            point = exit;
        }
        // Waypoints the straight leg has already carried the body past are
        // reached: the cursor never trails behind the hull's own plane.
        let ahead = point - unit.pos;
        while (path.next as usize) < target {
            let offset = path.waypoints[path.next as usize].center() - unit.pos;
            if offset.x * ahead.x + offset.y * ahead.y > Fx::ZERO {
                break;
            }
            path.next += 1;
        }
        let final_point = path.waypoints.len() == target + 1 && point == path_point(path, target);
        return Some((point, target, final_point));
    }
}

/// One tick of the ground motor: steer, throttle, roll, and land. Returns
/// whether the step its route asked for was refused.
///
/// A rolling hull turns toward its target every tick at the chassis turn
/// rate and keeps rolling through the bend, easing off as the heading error
/// grows; only an error past [`GROUND_PIVOT_THRESHOLD`] brakes along the
/// old heading and pivots in place, and a chassis at rest pivots onto its
/// bearing before it rolls. Off the exact bearing the body travels along
/// its heading, so a bend is a real arc; within `GROUND_ALIGNED_STEPS` of
/// the bearing it tracks the target point directly and lands on it exactly.
pub(super) fn advance(unit: &mut Unit, terrain: &GroundTerrain, parked: &ParkedBodies) -> bool {
    let target = route_target(unit, terrain, parked);
    let max_speed = unit.kind.stats().speed;
    let turn_rate = unit.kind.ground_turn_rate();
    let brake = increment(max_speed, i64::from(GROUND_BRAKE_TICKS));
    let offset = target.map(|(point, _, _)| point - unit.pos);
    let desired = offset
        .filter(|v| *v != Vec2Fx::ZERO)
        .map_or(unit.heading, heading_of);
    let heading_error = |heading: u8| {
        let delta = i16::from(desired.wrapping_sub(heading).cast_signed());
        (delta, delta.unsigned_abs())
    };
    let (delta, error) = heading_error(unit.heading);
    let pivot = error > u16::from(GROUND_PIVOT_THRESHOLD);
    // A reversal first brakes along the existing heading; every smaller
    // correction steers while rolling.
    if unit.drive_speed == Fx::ZERO || !pivot {
        let rate = i16::from(turn_rate);
        unit.heading = unit.heading.wrapping_add_signed(
            i8::try_from(delta.clamp(-rate, rate)).expect("clamping an i8 delta keeps it an i8"),
        );
    }
    let (_, error) = heading_error(unit.heading);
    let aligned = error <= u16::from(GROUND_ALIGNED_STEPS);
    let distance = offset.map_or(Fx::ZERO, Vec2Fx::length);
    // Inside the last braking step the body lands on the point whatever
    // its bearing; a sub-tick residual must never become something to
    // orbit or pivot toward.
    let landing = target.is_some() && distance <= brake;
    // From rest the chassis pivots onto its bearing before it rolls; only
    // a body already under way steers through a bend.
    let steering = aligned || unit.drive_speed > Fx::ZERO;
    let mut demand = Fx::ZERO;
    if landing {
        demand = distance;
    } else if target.is_some() && steering && error <= u16::from(GROUND_PIVOT_THRESHOLD) {
        // Ease off through a bend: full speed on the bearing, a quarter at
        // the pivot threshold.
        demand = max_speed * Fx::from_num(128 - i32::from(error)) / Fx::from_num(128);
        if !aligned {
            // Off the bearing the arc must be able to close on the target:
            // cap speed so the turn radius stays inside the remaining
            // distance instead of orbiting a nearby point.
            demand = demand.min(distance * Fx::from_num(turn_rate) / Fx::from_num(64));
        }
        // Invert this step plus the remaining two braking steps.
        let arrival = if distance <= brake * 3 {
            (distance + brake) / 2
        } else {
            (distance + brake * 3) / 3
        };
        demand = demand.min(arrival);
    }
    let rate = if demand > unit.drive_speed {
        increment(max_speed, i64::from(GROUND_ACCEL_TICKS))
    } else {
        brake
    };
    unit.drive_speed += (demand - unit.drive_speed).clamp(-rate, rate);
    let travel = if target.is_some() && (aligned || landing) {
        let offset = offset.expect("a target has an offset");
        if unit.drive_speed >= distance {
            offset
        } else {
            offset * (unit.drive_speed / distance)
        }
    } else {
        dir(unit.heading) * unit.drive_speed
    };
    let proposed = unit.pos + travel;
    let here = TilePos::containing(unit.pos);
    let there = TilePos::containing(proposed);
    // A step along an admitted leg passes this rule: any diagonal hop past
    // a blocked flank runs within a hull radius of it, which the swept leg
    // test already refuses. Only an off-bearing arc step, or ground that
    // closed this tick, stops the motor here.
    let contact_radius = crate::tick::brain::contact::collision_radius(unit);
    let allowed = if terrain.at_contact(unit.pos, contact_radius) {
        terrain.contact_clear(unit.pos, proposed, contact_radius)
    } else {
        there == here || (terrain.open(there) && super::early_advance_safe(here, there, terrain))
    };
    let refused = !allowed && proposed != unit.pos;
    if allowed {
        unit.pos = proposed;
    } else {
        unit.drive_speed = Fx::ZERO;
    }
    if let Some((point, index, final_point)) = target
        && unit.pos == point
    {
        if final_point {
            unit.path = None;
            unit.drive_speed = Fx::ZERO;
        } else if let Some(path) = unit.path.as_mut()
            // A stand-in point steered for instead, such as a contact exit,
            // does not pass the waypoint it stood in for.
            && point == path_point(path, index)
        {
            path.next = u32::try_from((index + 1).min(path.waypoints.len() - 1))
                .expect("waypoint counts fit in u32");
        }
    }
    refused
}

#[cfg(test)]
mod tests;
