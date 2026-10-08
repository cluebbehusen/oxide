use super::*;
use crate::Scenario;
use crate::scenario::UnitSpec;
use crate::stats::UnitKind;
use chassis::fx::{Fx, Vec2Fx};

#[test]
fn boarding_route_moves_when_tile_centers_are_misleadingly_close() {
    let mut scenario = Scenario::skirmish();
    scenario.name = "exact boarding reach".to_owned();
    scenario.map = vec![
        "############".to_owned(),
        "#1.........#".to_owned(),
        "#..........#".to_owned(),
        "#..........#".to_owned(),
        "#..........#".to_owned(),
        "#........2.#".to_owned(),
        "#..........#".to_owned(),
        "############".to_owned(),
    ];
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Skyhook,
            x: 6,
            y: 4,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 5,
            y: 4,
        },
    ];
    scenario.buildings.clear();
    scenario.meta = None;
    let mut state = scenario.build().expect("boarding arena builds");
    let transport = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Skyhook)
        .expect("scenario has a transport")
        .id;
    let rider = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Sentinel)
        .expect("scenario has a rider")
        .id;
    let carrier_pos = Vec2Fx::new(Fx::lit("6.99"), Fx::lit("4.5"));
    let rider_pos = Vec2Fx::new(Fx::lit("5.01"), Fx::lit("4.5"));
    state.unit_mut(transport).expect("transport lives").pos = carrier_pos;
    state.unit_mut(rider).expect("rider lives").pos = rider_pos;

    let rider_tile = TilePos::containing(rider_pos);
    let reach_sq = crate::stats::LOAD_REACH * crate::stats::LOAD_REACH;
    assert!(rider_tile.center().dist_sq(carrier_pos) <= reach_sq);
    assert!(rider_pos.dist_sq(carrier_pos) > reach_sq);

    let mut pending = Pending::default();
    let mut events = Vec::new();
    board(&mut state, rider, transport, &mut pending, &mut events);

    let path = state
        .unit(rider)
        .and_then(|unit| unit.path.as_ref())
        .expect("the rider receives a real approach path");
    assert_ne!(path.goal, rider_tile);
    assert!(!path.waypoints.is_empty());
    assert!(events.is_empty());
    assert!(pending.boardings.is_empty());
}
