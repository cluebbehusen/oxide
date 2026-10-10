use super::*;
use crate::UnitKind;
use crate::scenario::{PlayerSpec, Scenario, ScenarioMode, UnitSpec};
use crate::state::PathFollow;

/// A Kestrel pressed against a ridge, nose into it, flying a route
/// toward `waypoint`.
fn against_ridge(waypoint: TilePos) -> (Unit, Map) {
    let state = Scenario {
        mode: ScenarioMode::Match,
        name: "ridge".into(),
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
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            })
            .into(),
        units: vec![UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
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
