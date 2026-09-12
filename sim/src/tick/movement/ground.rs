use crate::tick::flight::heading_of;
use chassis::compass::dir;
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;

use crate::map::Map;
use crate::state::{Building, Unit};
use crate::stats::WAYPOINT_ACCEPT;

fn increment(speed: Fx, ticks: i64) -> Fx {
    Fx::from_bits((speed.to_bits() + ticks - 1) / ticks)
}

fn route_target(unit: &mut Unit, map: &Map, buildings: &[Building]) -> Option<(Vec2Fx, bool)> {
    loop {
        let path = unit.path.as_mut()?;
        let Some(&waypoint) = path.waypoints.get(path.next as usize) else {
            unit.path = None;
            return None;
        };
        let here = TilePos::containing(unit.pos);
        let open = |tile| {
            map.terrain_passable(tile)
                && !buildings
                    .iter()
                    .any(|b| b.contains(tile) && !b.kind.is_stealthy())
        };
        if !open(waypoint) || !super::early_advance_safe(here, waypoint, map, buildings) {
            unit.path = None;
            return None;
        }
        let next = path.waypoints.get(path.next as usize + 1).copied();
        if let Some(next) = next
            && (unit.pos.dist(waypoint.center()) <= WAYPOINT_ACCEPT
                || super::passed_intermediate_waypoint(
                    unit.pos,
                    waypoint,
                    next,
                    unit.kind.stats().radius,
                ))
            && super::early_advance_safe(waypoint, next, map, buildings)
            && super::early_advance_safe(here, next, map, buildings)
        {
            path.next += 1;
            continue;
        }
        return Some((waypoint.center(), next.is_none()));
    }
}

pub(super) fn advance(unit: &mut Unit, map: &Map, buildings: &[Building]) {
    let target = route_target(unit, map, buildings);
    let max_speed = unit.kind.stats().speed;
    let brake = increment(max_speed, 3);
    let offset = target.map(|(point, _)| point - unit.pos);
    let desired = offset
        .filter(|v| *v != Vec2Fx::ZERO)
        .map_or(unit.heading, heading_of);
    let delta = i16::from(desired.wrapping_sub(unit.heading) as i8);
    let aligned = delta.unsigned_abs() <= 8;
    // A sharp change of direction first brakes along the existing heading.
    if unit.drive_speed == Fx::ZERO || aligned {
        let rate = i16::from(unit.kind.ground_turn_rate());
        unit.heading = unit
            .heading
            .wrapping_add_signed(delta.clamp(-rate, rate) as i8);
    }
    let aligned = desired
        .wrapping_sub(unit.heading)
        .cast_signed()
        .unsigned_abs()
        <= 8;
    let distance = offset.map_or(Fx::ZERO, Vec2Fx::length);
    let mut demand = if target.is_some() && aligned {
        max_speed
    } else {
        Fx::ZERO
    };
    if target.is_some() {
        // Invert this step plus the remaining two braking steps.
        let arrival = if distance <= brake {
            distance
        } else if distance <= brake * 3 {
            (distance + brake) / 2
        } else {
            (distance + brake * 3) / 3
        };
        demand = demand.min(arrival);
    }
    let rate = if demand > unit.drive_speed {
        increment(max_speed, 6)
    } else {
        brake
    };
    unit.drive_speed += (demand - unit.drive_speed).clamp(-rate, rate);
    let travel = if aligned && target.is_some() && unit.drive_speed <= distance {
        let offset = offset.expect("a target has an offset");
        if unit.drive_speed == distance {
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
    if there == here
        || (map.terrain_passable(there)
            && !buildings
                .iter()
                .any(|b| b.contains(there) && !b.kind.is_stealthy())
            && super::early_advance_safe(here, there, map, buildings))
    {
        unit.pos = proposed;
    } else {
        // New construction can remove stopping space between ticks.
        unit.drive_speed = Fx::ZERO;
    }
    if let Some((point, final_point)) = target
        && unit.pos == point
    {
        if final_point {
            unit.path = None;
            unit.drive_speed = Fx::ZERO;
        } else if let Some(path) = unit.path.as_mut() {
            path.next += 1;
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
        let mut rows = vec!["........................".to_string(); 16];
        rows[1] = ".1......................".into();
        rows[13] = "....................2...".into();
        Scenario {
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
        unit.path = Some(PathFollow {
            goal: tile,
            waypoints: vec![tile],
            next: 0,
        });
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
                advance(&mut unit, state.map(), state.buildings());
                assert!(unit.drive_speed < kind.stats().speed, "{kind:?}");
            }
            advance(&mut unit, state.map(), state.buildings());
            assert_eq!(unit.drive_speed, kind.stats().speed, "{kind:?}");
            unit.path = None;
            let before = unit.pos;
            for _ in 0..3 {
                advance(&mut unit, state.map(), state.buildings());
            }
            assert_eq!(unit.drive_speed, Fx::ZERO);
            assert!(unit.pos.x > before.x);
            let stopped = unit.pos;
            advance(&mut unit, state.map(), state.buildings());
            assert_eq!(unit.pos, stopped);
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
                advance(&mut unit, state.map(), state.buildings());
                assert_eq!(unit.heading, 0);
                assert!(unit.pos.x >= before.x);
            }
            assert_eq!(unit.drive_speed, Fx::ZERO);
            for _ in 0..400 {
                advance(&mut unit, state.map(), state.buildings());
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
        advance(&mut unit, state.map(), state.buildings());
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
        advance(&mut unit, state.map(), state.buildings());
        assert_eq!(unit.pos, before);
        assert_eq!(unit.drive_speed, Fx::ZERO);
    }
}
