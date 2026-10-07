//! Heading-first travel for aircraft that can stop and hover.

use chassis::compass::dir;
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;

use crate::map::Map;
use crate::state::Unit;

fn clear_segment(map: &Map, from: Vec2Fx, to: Vec2Fx) -> bool {
    let open = |tile| {
        map.tile(tile)
            .is_some_and(|cell| !cell.terrain.blocks_air())
    };
    map.clamp_to_envelope(to) == to
        && open(TilePos::containing(to))
        && !chassis::path::line_blocked(from, to, open)
}

pub(super) fn advance(unit: &mut Unit, map: &Map) {
    let stats = unit.kind.stats();
    let rate = unit.kind.cruise_turn_rate();
    let radius = stats.speed * const { Fx::lit("40.75") } / Fx::from_num(rate);
    loop {
        let Some(path) = &mut unit.path else {
            return;
        };
        let Some(&waypoint) = path.waypoints.get(path.next as usize) else {
            unit.path = None;
            return;
        };
        let center = if path.next as usize + 1 == path.waypoints.len() {
            path.final_point.unwrap_or(waypoint.center())
        } else {
            waypoint.center()
        };
        if let Some(&next) = path.waypoints.get(path.next as usize + 1)
            && unit.pos.dist_sq(center) <= radius * radius
            && clear_segment(map, unit.pos, next.center())
        {
            path.next += 1;
            continue;
        }
        let offset = center - unit.pos;
        let distance = offset.length();
        if distance == Fx::ZERO {
            path.next += 1;
            continue;
        }
        let aligned = super::steer_bearing(&mut unit.heading, offset, rate);
        // Below cruise radius, slowing tightens the arc enough to converge
        // on a nearby waypoint instead of orbiting it indefinitely.
        let speed = stats.speed.min(distance * Fx::from_num(rate) / 64);
        let next = if aligned && distance <= stats.speed {
            center
        } else {
            unit.pos + dir(unit.heading) * speed
        };
        if clear_segment(map, unit.pos, next) {
            unit.pos = next;
        } else if aligned {
            // Facing the waypoint and still blocked: the arc has carried us
            // behind a Peak, so replan from the actual position.
            unit.path = None;
        }
        // Otherwise hover while the nose turns toward the waypoint. The
        // route stays: replanning from a spot the body has not left would
        // only plan the same route again.
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{PlayerSpec, Scenario, ScenarioMode, UnitSpec};
    use crate::state::PathFollow;
    use crate::{Faction, UnitKind};

    /// A Darter pressed against a ridge, nose into it, flying a route
    /// toward `waypoint`.
    fn against_ridge(waypoint: TilePos) -> (Unit, Map) {
        let state = Scenario {
            mode: ScenarioMode::Match,
            name: "ridge".into(),
            seed: 1,
            map: vec![
                "1...............".into(),
                "......^.........".into(),
                "......^.........".into(),
                "......^.........".into(),
                "......^.....2...".into(),
                "......^.........".into(),
                "......^.........".into(),
                "................".into(),
            ],
            players: ["West", "East"]
                .map(|name| PlayerSpec {
                    name: name.into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                })
                .into(),
            units: vec![UnitSpec {
                player: 0,
                kind: UnitKind::Darter,
                x: 5,
                y: 3,
            }],
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("ridge builds");
        let mut unit = state.units[0].clone();
        unit.pos = Vec2Fx::new(Fx::lit("5.95"), Fx::lit("3.5"));
        unit.heading = chassis::compass::heading_of(Vec2Fx::new(Fx::ONE, Fx::ZERO));
        unit.path = Some(PathFollow {
            final_point: None,
            goal: waypoint,
            waypoints: vec![waypoint],
            next: 0,
        });
        (unit, state.map.clone())
    }

    #[test]
    fn a_turning_nose_keeps_its_route_and_a_blocked_bearing_drops_it() {
        let (mut turning, map) = against_ridge(TilePos::new(1, 3));
        let start = turning.pos;
        advance(&mut turning, &map);
        assert_eq!(turning.pos, start, "the nose is still turning");
        assert!(turning.path.is_some(), "a turn in place keeps the route");

        let (mut facing, map) = against_ridge(TilePos::new(9, 3));
        advance(&mut facing, &map);
        assert_eq!(facing.pos, start);
        assert!(
            facing.path.is_none(),
            "a ridge across the bearing forces a replan"
        );
    }
}
