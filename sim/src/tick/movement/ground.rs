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
    } else {
        !chassis::path::swept_line_blocked(
            unit.pos,
            waypoint.center(),
            unit.kind.stats().radius,
            |tile| terrain.open(tile),
        )
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
            let bearing = heading_of(waypoint.center() - unit.pos);
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
            && !chassis::path::swept_line_blocked(unit.pos, candidate.center(), radius, clear)
        {
            target += 1;
        }
        let point = path.waypoints[target].center();
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
        let final_point = path.waypoints.len() == target + 1;
        return Some((point, target, final_point));
    }
}

/// One tick of the ground motor: steer, throttle, roll, and land.
///
/// A rolling hull turns toward its target every tick at the chassis turn
/// rate and keeps rolling through the bend, easing off as the heading error
/// grows; only an error past [`GROUND_PIVOT_THRESHOLD`] brakes along the
/// old heading and pivots in place, and a chassis at rest pivots onto its
/// bearing before it rolls. Off the exact bearing the body travels along
/// its heading, so a bend is a real arc; within eight compass steps it
/// tracks the target point directly and lands on it exactly.
pub(super) fn advance(unit: &mut Unit, terrain: &GroundTerrain, parked: &ParkedBodies) {
    let target = route_target(unit, terrain, parked);
    let max_speed = unit.kind.stats().speed;
    let turn_rate = unit.kind.ground_turn_rate();
    let brake = increment(max_speed, i64::from(GROUND_BRAKE_TICKS));
    let offset = target.map(|(point, _, _)| point - unit.pos);
    let desired = offset
        .filter(|v| *v != Vec2Fx::ZERO)
        .map_or(unit.heading, heading_of);
    let heading_error = |heading: u8| {
        let delta = i16::from(desired.wrapping_sub(heading) as i8);
        (delta, delta.unsigned_abs())
    };
    let (delta, error) = heading_error(unit.heading);
    let pivot = error > u16::from(GROUND_PIVOT_THRESHOLD);
    // A reversal first brakes along the existing heading; every smaller
    // correction steers while rolling.
    if unit.drive_speed == Fx::ZERO || !pivot {
        let rate = i16::from(turn_rate);
        unit.heading = unit
            .heading
            .wrapping_add_signed(delta.clamp(-rate, rate) as i8);
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
    if there == here || (terrain.open(there) && super::early_advance_safe(here, there, terrain)) {
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
        } else if let Some(path) = unit.path.as_mut() {
            path.next = index as u32 + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{PlayerSpec, UnitSpec};
    use crate::state::PathFollow;
    use crate::{Faction, Scenario, UnitKind};

    fn scene(kind: UnitKind) -> crate::State {
        scene_with(kind, &[])
    }

    /// An open arena with rocks at `rocks`, the unit at (8, 8).
    fn scene_with(kind: UnitKind, rocks: &[(usize, usize)]) -> crate::State {
        let mut rows: Vec<Vec<char>> = vec![vec!['.'; 24]; 16];
        rows[1][1] = '1';
        rows[13][20] = '2';
        for &(x, y) in rocks {
            rows[y][x] = '#';
        }
        let rows: Vec<String> = rows.into_iter().map(|r| r.into_iter().collect()).collect();
        Scenario {
            mode: Default::default(),
            name: "ground-motor".into(),
            seed: 1,
            map: rows,
            players: [Faction::Ferrous, Faction::Cupric]
                .into_iter()
                .map(|faction| PlayerSpec {
                    name: "seat".into(),
                    faction,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                })
                .collect(),
            units: vec![UnitSpec {
                player: 0,
                kind,
                x: 8,
                y: 8,
            }],
            buildings: vec![],
            meta: None,
        }
        .build()
        .unwrap()
    }

    fn target(unit: &mut Unit, tile: TilePos) {
        route(unit, vec![tile]);
    }

    fn route(unit: &mut Unit, waypoints: Vec<TilePos>) {
        unit.path = Some(PathFollow {
            goal: *waypoints.last().expect("a route has a goal"),
            waypoints,
            next: 0,
        });
    }

    fn tiles(points: &[(i32, i32)]) -> Vec<TilePos> {
        points.iter().map(|&(x, y)| TilePos::new(x, y)).collect()
    }

    /// The A* staircase from (8, 8) toward the east-southeast.
    fn staircase() -> Vec<TilePos> {
        tiles(&[
            (9, 8),
            (10, 9),
            (11, 9),
            (12, 10),
            (13, 10),
            (14, 11),
            (15, 11),
            (16, 12),
            (17, 12),
        ])
    }

    /// The steering target's route index, without moving the body.
    fn target_index(unit: &mut Unit, state: &crate::State) -> usize {
        route_target(unit, &state.ground_terrain(), &ParkedBodies::default())
            .expect("still en route")
            .1
    }

    #[test]
    fn lookahead_steers_for_the_furthest_visible_waypoint_of_a_staircase() {
        let state = scene(UnitKind::Harvester);
        let mut unit = state.units()[0].clone();
        unit.heading = 0;
        unit.drive_speed = unit.kind.stats().speed;
        route(&mut unit, staircase());
        assert_eq!(target_index(&mut unit, &state), ROUTE_LOOKAHEAD);
        for _ in 0..400 {
            advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
            assert!(state.ground_terrain().open(unit.tile()));
        }
        assert_eq!(unit.pos, TilePos::new(17, 12).center());
        assert!(unit.path.is_none());
    }

    #[test]
    fn the_route_cursor_trails_the_hull_not_the_steering_target() {
        // Tiles the body has not reached stay ahead of the cursor, so a
        // scan of the remaining route (the harvest danger check) still sees
        // every tile the straight leg will cross.
        let state = scene(UnitKind::Harvester);
        let mut unit = state.units()[0].clone();
        unit.heading = 0;
        unit.drive_speed = unit.kind.stats().speed;
        let waypoints = staircase();
        route(&mut unit, waypoints.clone());
        for _ in 0..80 {
            advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
            let Some(path) = unit.path.as_ref() else {
                break;
            };
            let next = path.next as usize;
            // A waypoint counts as reached once the hull is past it or
            // within the deflection reach that accepts it.
            let reach =
                unit.kind.stats().radius.max(WAYPOINT_ACCEPT) + crate::stats::COLLISION_MAX_STEP;
            let ahead = waypoints.last().unwrap().center() - unit.pos;
            for (index, waypoint) in waypoints.iter().enumerate() {
                let offset = waypoint.center() - unit.pos;
                let in_front = offset.x * ahead.x + offset.y * ahead.y > Fx::ZERO;
                let reached = !in_front || offset.length() <= reach;
                assert!(
                    index >= next || reached,
                    "waypoint {index} is still ahead of the hull but behind the cursor {next}"
                );
            }
        }
        assert!(unit.path.is_none(), "never arrived");
    }

    #[test]
    fn a_chassis_at_rest_facing_its_next_waypoint_rolls_before_looking_ahead() {
        let state = scene(UnitKind::Harvester);
        let mut unit = state.units()[0].clone();
        unit.heading = 0;
        route(&mut unit, staircase());
        assert_eq!(target_index(&mut unit, &state), 0);
        advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
        assert!(unit.drive_speed > Fx::ZERO);
        assert!(target_index(&mut unit, &state) > 0);
    }

    #[test]
    fn lookahead_stops_where_a_rock_breaks_the_line() {
        // A rock on the staircase's third step: the route bends around it,
        // and a hull edge line to (11, 10) already clips the rock.
        let state = scene_with(UnitKind::Harvester, &[(11, 9)]);
        let mut unit = state.units()[0].clone();
        unit.heading = 0;
        unit.drive_speed = unit.kind.stats().speed;
        route(
            &mut unit,
            tiles(&[
                (9, 8),
                (10, 9),
                (10, 10),
                (11, 10),
                (12, 10),
                (13, 10),
                (14, 11),
                (15, 11),
                (16, 12),
                (17, 12),
            ]),
        );
        let target = target_index(&mut unit, &state);
        assert!(target < 3, "cut the corner: target = {target}");
        for _ in 0..400 {
            advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
            assert!(state.ground_terrain().open(unit.tile()));
        }
        assert_eq!(unit.pos, TilePos::new(17, 12).center());
    }

    #[test]
    fn a_wide_hull_keeps_the_tile_route_beside_a_wall() {
        let wall: Vec<(usize, usize)> = (9..=16).map(|x| (x, 9)).collect();
        for (kind, skips) in [(UnitKind::Breaker, false), (UnitKind::Harvester, true)] {
            let state = scene_with(kind, &wall);
            let mut unit = state.units()[0].clone();
            unit.heading = 0;
            unit.drive_speed = kind.stats().speed;
            route(
                &mut unit,
                tiles(&[(9, 8), (10, 8), (11, 8), (12, 8), (13, 8)]),
            );
            assert_eq!(target_index(&mut unit, &state) > 0, skips, "{kind:?}");
        }
    }

    #[test]
    fn a_right_angle_bend_is_driven_as_an_arc_without_stopping() {
        // A wall east of column 11 hides the southbound leg until the corner.
        let wall: Vec<(usize, usize)> = (9..=14).map(|y| (11, y)).collect();
        for kind in [UnitKind::Harvester, UnitKind::Sentinel, UnitKind::Breaker] {
            let state = scene_with(kind, &wall);
            let mut unit = state.units()[0].clone();
            unit.heading = 0;
            unit.drive_speed = kind.stats().speed;
            let mut waypoints = tiles(&[(9, 8), (10, 8), (11, 8), (12, 8)]);
            waypoints.extend((9..=14).map(|y| TilePos::new(12, y)));
            route(&mut unit, waypoints);
            let mut turned = false;
            let mut min_speed = kind.stats().speed;
            for _ in 0..600 {
                advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
                assert!(state.ground_terrain().open(unit.tile()), "{kind:?}");
                if unit.path.is_none() {
                    break;
                }
                if unit.heading != 0 {
                    turned = true;
                }
                if turned {
                    min_speed = min_speed.min(unit.drive_speed);
                }
            }
            assert!(turned, "{kind:?} never turned");
            assert!(unit.path.is_none(), "{kind:?} never arrived");
            assert_eq!(unit.pos, TilePos::new(12, 14).center(), "{kind:?}");
            assert!(min_speed > Fx::ZERO, "{kind:?} stopped in the bend");
        }
    }

    #[test]
    fn each_ground_chassis_accelerates_in_six_ticks_and_stops_in_three() {
        for kind in UnitKind::ALL {
            if kind.stats().domain != crate::stats::Domain::Ground {
                continue;
            }
            let state = scene(kind);
            let mut unit = state.units()[0].clone();
            unit.heading = 0;
            target(&mut unit, TilePos::new(20, 8));
            for _ in 0..5 {
                advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
                assert!(unit.drive_speed < kind.stats().speed, "{kind:?}");
            }
            advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
            assert_eq!(unit.drive_speed, kind.stats().speed, "{kind:?}");
            unit.path = None;
            let before = unit.pos;
            for _ in 0..3 {
                advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
            }
            assert_eq!(unit.drive_speed, Fx::ZERO);
            assert!(unit.pos.x > before.x);
            let stopped = unit.pos;
            advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
            assert_eq!(unit.pos, stopped);
        }
    }

    #[test]
    fn a_reversal_from_rest_costs_the_quoted_ticks_over_free_flow() {
        for kind in UnitKind::ALL {
            if kind.stats().domain != crate::stats::Domain::Ground {
                assert_eq!(kind.ground_reversal_ticks(), 0, "{kind:?}");
                continue;
            }
            let state = scene(kind);
            let mut unit = state.units()[0].clone();
            unit.heading = 128;
            target(&mut unit, TilePos::new(20, 8));
            let mut ticks = 0u64;
            while unit.path.is_some() {
                advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
                ticks += 1;
                assert!(ticks < 1_000, "{kind:?}");
            }
            let free_flow = (Fx::from_num(12) / kind.stats().speed)
                .ceil()
                .to_num::<u64>();
            let quoted = kind.ground_reversal_ticks();
            assert!(
                (quoted - 1..=quoted).contains(&(ticks - free_flow)),
                "{kind:?}: {ticks} against {free_flow} + {quoted}"
            );
        }
    }

    #[test]
    fn retasking_a_heavy_chassis_brakes_before_pivoting_and_then_arrives() {
        for kind in [UnitKind::Warden, UnitKind::Avalanche, UnitKind::Breaker] {
            let state = scene(kind);
            let mut unit = state.units()[0].clone();
            unit.heading = 0;
            unit.drive_speed = kind.stats().speed;
            let goal = TilePos::new(3, 8);
            target(&mut unit, goal);
            for _ in 0..3 {
                let before = unit.pos;
                advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
                assert_eq!(unit.heading, 0);
                assert!(unit.pos.x >= before.x);
            }
            assert_eq!(unit.drive_speed, Fx::ZERO);
            for _ in 0..400 {
                advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
            }
            assert_eq!(unit.pos, goal.center(), "{kind:?}");
            assert_eq!(unit.drive_speed, Fx::ZERO);
            assert!(unit.path.is_none());
        }
    }

    #[test]
    fn collision_displacement_does_not_create_motor_speed() {
        let state = scene(UnitKind::Harvester);
        let mut unit = state.units()[0].clone();
        unit.pos.x += Fx::lit("0.05");
        let displaced = unit.pos;
        advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
        assert_eq!(unit.pos, displaced);
        assert_eq!(unit.drive_speed, Fx::ZERO);
    }

    #[test]
    fn braking_does_not_enter_a_newly_claimed_footprint() {
        let state = scene(UnitKind::Harvester);
        let building = &state.buildings()[0];
        let mut unit = state.units()[0].clone();
        unit.pos = building.anchor.center();
        unit.pos.x = Fx::from_num(building.anchor.x) - Fx::lit("0.001");
        unit.heading = 0;
        unit.drive_speed = unit.kind.stats().speed;
        let before = unit.pos;
        advance(&mut unit, &state.ground_terrain(), &ParkedBodies::default());
        assert_eq!(unit.pos, before);
        assert_eq!(unit.drive_speed, Fx::ZERO);
    }
}
