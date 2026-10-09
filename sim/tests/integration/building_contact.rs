//! Local building interaction preserves tile routing and permits a bounded final approach.
use chassis::{fx::Fx, grid::TilePos};
use oxide_sim::{Command, Event, PlayerCommand, PlayerId, Scenario, State, Target, UnitKind};
use serde_json::json;

fn scene(kind: &str, own_building: bool) -> State {
    let scenario: Scenario = serde_json::from_value(json!({
        "name":"local contact", "mode":"sandbox", "map":vec!["................................";24],
        "players":[{"name":"Local","faction":"ferrous","scrap":10000,"bot":false},{"name":"Target","faction":"cupric","scrap":0,"bot":false}],
        "units":[{"player":0,"kind":kind,"x":12,"y":9}],
        "buildings":[{"player":i32::from(!own_building),"kind":"fabricator","x":10,"y":10}]
    })).unwrap();
    scenario.build().unwrap()
}
fn command(command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(0),
        command,
    }
}

#[test]
fn melee_reaches_the_inset_and_can_leave_it() {
    let mut state = scene("scuttler", false);
    let id = state.units()[0].id;
    let building = state.buildings()[0].id;
    let mut hit = false;
    for tick in 0..160 {
        let report = state.tick(&if tick == 0 {
            vec![command(Command::Attack {
                units: vec![id],
                target: Target::Building(building).into(),
                queue: false,
            })]
        } else {
            vec![]
        });
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::AttackHit { .. }))
        {
            let u = state.unit(id).unwrap();
            let b = state.building(building).unwrap();
            let reach = u.kind.stats().radius + u.kind.stats().contact_reach.unwrap();
            assert!(u.pos.dist_sq(state.contact_surface(b).closest(u.pos)) <= reach * reach);
            assert!(
                u.pos.y > Fx::from_num(10),
                "did not enter inset: {:?}",
                u.pos
            );
            let restored: State =
                serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
            assert_eq!(state.hash(), restored.hash());
            hit = true;
            break;
        }
    }
    assert!(hit, "never reached building");
    state.tick(&[command(Command::Run {
        units: vec![id],
        goal: TilePos::new(11, 5),
        queue: false,
    })]);
    for _ in 0..160 {
        state.tick(&[]);
        if state
            .unit(id)
            .unwrap()
            .pos
            .dist_sq(TilePos::new(11, 5).center())
            < Fx::ONE
        {
            break;
        }
    }
    assert!(
        state
            .unit(id)
            .unwrap()
            .pos
            .dist_sq(TilePos::new(11, 5).center())
            < Fx::ONE,
        "could not leave inset: {:?}",
        state.unit(id)
    );
}

#[test]
fn worker_repairs_at_the_surface_then_obeys_a_new_move() {
    for kind in [UnitKind::Harvester, UnitKind::Excavator] {
        let state = scene(
            if kind == UnitKind::Harvester {
                "harvester"
            } else {
                "excavator"
            },
            true,
        );
        let id = state.units()[0].id;
        let building = state.buildings()[0].id;
        let mut data = serde_json::to_value(&state).unwrap();
        data["buildings"][0]["hp"] = json!(100);
        let mut state: State = serde_json::from_value(data).unwrap();
        state.tick(&[command(Command::Repair {
            units: vec![id],
            building,
            queue: false,
        })]);
        let mut repaired = false;
        for _ in 0..250 {
            state.tick(&[]);
            if state.building(building).unwrap().hp > 100 {
                repaired = true;
                break;
            }
        }
        assert!(repaired, "{kind:?} failed to repair: {:?}", state.unit(id));
        state.tick(&[command(Command::Run {
            units: vec![id],
            goal: TilePos::new(16, 10),
            queue: false,
        })]);
        for _ in 0..250 {
            state.tick(&[]);
        }
        assert!(
            state
                .unit(id)
                .unwrap()
                .pos
                .dist_sq(TilePos::new(16, 10).center())
                < Fx::ONE,
            "{kind:?} failed to leave: {:?}",
            state.unit(id)
        );
    }
}

#[test]
fn rectangular_building_wall_still_blocks_ordinary_travel() {
    let scenario:Scenario=serde_json::from_value(json!({
        "name":"sealed wall", "mode":"sandbox", "map":vec!["........................";16],
        "players":[{"name":"Local","faction":"ferrous","scrap":0,"bot":false},{"name":"Target","faction":"cupric","scrap":0,"bot":false}],
        "units":[{"player":0,"kind":"harvester","x":7,"y":7}],
        "buildings":(0..16).step_by(2).map(|y|json!({"player":1,"kind":"fabricator","x":12,"y":y})).collect::<Vec<_>>()
    })).unwrap();
    let mut state = scenario.build().unwrap();
    let id = state.units()[0].id;
    state.tick(&[command(Command::Run {
        units: vec![id],
        goal: TilePos::new(18, 7),
        queue: false,
    })]);
    for _ in 0..350 {
        state.tick(&[]);
        assert!(state.unit(id).unwrap().pos.x < Fx::from_num(12));
    }
    for y in 0..16 {
        assert!(!state.passable(TilePos::new(12, y)));
    }
}

#[test]
fn unusable_building_stances_stall_once_and_obey_order_failure_policy() {
    for (kind, own_building) in [("harvester", true), ("sentinel", false)] {
        let mut map = vec![vec!['#'; 12]; 12];
        map[4][4] = '.';
        map[3][4] = '.';
        for row in &mut map[5..7] {
            row[5..7].fill('.');
        }
        let scenario: Scenario = serde_json::from_value(json!({
            "name":"unusable stance", "mode":"sandbox", "map":map.into_iter().map(|row| row.into_iter().collect::<String>()).collect::<Vec<_>>(),
            "players":[{"name":"Local","faction":"ferrous","scrap":10000,"bot":false},
                       {"name":"Target","faction":"cupric","scrap":0,"bot":false}],
            "units":[{"player":0,"kind":kind,"x":4,"y":4}],
            "buildings":[{"player":i32::from(!own_building),"kind":"foundry","x":5,"y":5}]
        })).unwrap();
        let initial = scenario.build().unwrap();
        let id = initial.units()[0].id;
        let building = initial.buildings()[0].id;
        let mut data = serde_json::to_value(initial).unwrap();
        data["buildings"][0]["hp"] = json!(100);
        let mut state: State = serde_json::from_value(data).unwrap();
        let order = if own_building {
            Command::Repair {
                units: vec![id],
                building,
                queue: false,
            }
        } else {
            Command::Attack {
                units: vec![id],
                target: Target::Building(building).into(),
                queue: false,
            }
        };
        let mut events = state
            .tick(&[
                command(order),
                command(Command::Run {
                    units: vec![id],
                    goal: TilePos::new(4, 3),
                    queue: true,
                }),
            ])
            .events;
        for _ in 0..10 {
            events.extend(state.tick(&[]).events);
        }
        assert_eq!(events.iter().filter(|event| matches!(event,
            Event::OrderStalled { unit, reason: oxide_sim::event::StallReason::NoRoute, .. } if *unit == id
        )).count(), 1, "{kind}: {events:?}");
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::CommandRejected { .. })),
            "{kind}: {events:?}"
        );
        let unit = state.unit(id).unwrap();
        assert!(unit.queue.is_empty(), "{kind}: {unit:?}");
        if own_building {
            assert!(
                matches!(unit.order, oxide_sim::Order::Run { goal } if goal.tile() == TilePos::new(4, 3)),
                "{kind}: {unit:?}"
            );
        } else {
            assert!(
                matches!(unit.order, oxide_sim::Order::Idle),
                "{kind}: {unit:?}"
            );
        }
        assert!(
            !matches!(
                unit.order,
                oxide_sim::Order::Repair { .. } | oxide_sim::Order::Attack { .. }
            ),
            "{kind}: {unit:?}"
        );
    }
}

#[test]
fn builders_reach_a_stance_past_a_neighboring_footprint_corner() {
    for x in [19, 21] {
        let scenario: Scenario = serde_json::from_value(json!({
            "name":"neighbor corner", "mode":"sandbox", "map":vec!["................................";24],
            "players":[{"name":"Local","faction":"ferrous","scrap":10000,"bot":false}],
            "units":[{"player":0,"kind":"harvester","x":x,"y":12}],
            "buildings":[{"player":0,"kind":"flak_turret","x":20,"y":15}]
        }))
        .unwrap();
        let mut state = scenario.build().unwrap();
        let id = state.units()[0].id;
        state.tick(&[command(Command::Build {
            units: vec![id],
            kind: oxide_sim::BuildingKind::Turret,
            anchor: TilePos::new(20, 16),
            queue: false,
            defer: false,
        })]);
        let site = state
            .buildings()
            .iter()
            .find(|b| b.anchor == TilePos::new(20, 16))
            .expect("site placed")
            .id;
        let mut started = false;
        for _ in 0..300 {
            state.tick(&[]);
            if state.building(site).unwrap().progress > 0 {
                started = true;
                break;
            }
        }
        assert!(started, "builder from x={x} froze: {:?}", state.unit(id));
        assert!(state.in_building_work_reach(state.unit(id).unwrap(), site));
    }
}

/// A paid, revealed site at `anchor` beside `buildings`, ordered from a
/// harvester parked a few tiles below it. A distant Fabricator unlocks
/// every kind.
fn paid_site(
    kind: oxide_sim::BuildingKind,
    anchor: TilePos,
    mut buildings: serde_json::Value,
) -> (State, oxide_sim::UnitId, oxide_sim::BuildingId) {
    buildings
        .as_array_mut()
        .unwrap()
        .push(json!({"player":0,"kind":"fabricator","x":28,"y":20}));
    let scenario: Scenario = serde_json::from_value(json!({
        "name":"paid site", "mode":"sandbox", "map":vec!["................................";24],
        "players":[{"name":"Local","faction":"ferrous","scrap":10000,"bot":false}],
        "units":[{"player":0,"kind":"harvester","x":anchor.x,"y":anchor.y + 4}],
        "buildings": buildings,
    }))
    .unwrap();
    let mut state = scenario.build().unwrap();
    let id = state.units()[0].id;
    let report = state.tick(&[command(Command::Build {
        units: vec![id],
        kind,
        anchor,
        queue: false,
        defer: false,
    })]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor && !b.built)
        .unwrap_or_else(|| panic!("{:?}", report.events))
        .id;
    assert!(!state.building(site).unwrap().provisional);
    (state, id, site)
}

/// `state` with `id` at rest at `pos`, walking `waypoints` toward `point`.
fn walking(
    state: &State,
    id: oxide_sim::UnitId,
    pos: chassis::fx::Vec2Fx,
    waypoints: Vec<TilePos>,
    point: Option<chassis::fx::Vec2Fx>,
) -> State {
    let mut data = serde_json::to_value(state).unwrap();
    let unit = data["units"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|unit| unit["id"] == json!(id))
        .unwrap();
    unit["pos"] = json!(pos);
    unit["drive_speed"] = json!(Fx::ZERO);
    unit["path"] = json!(oxide_sim::state::PathFollow {
        final_point: point,
        goal: *waypoints.last().unwrap(),
        waypoints,
        next: 0,
    });
    serde_json::from_value(data).unwrap()
}

/// The tile beside a footprint in direction `(dx, dy)`: a face's first
/// tile, or the diagonal tile off a corner.
fn beside(anchor: TilePos, size: (i32, i32), (dx, dy): (i32, i32)) -> TilePos {
    let along = |d: i32, start: i32, span: i32| match d {
        -1 => start - 1,
        0 => start,
        _ => start + span,
    };
    TilePos::new(along(dx, anchor.x, size.0), along(dy, anchor.y, size.1))
}

/// The footprint's ring of neighboring tiles, clockwise from its top-left
/// corner, so consecutive tiles are cardinal steps.
fn ring(anchor: TilePos, size: (i32, i32)) -> Vec<TilePos> {
    let (left, top) = (anchor.x - 1, anchor.y - 1);
    let (right, bottom) = (anchor.x + size.0, anchor.y + size.1);
    let mut tiles: Vec<_> = (left..right).map(|x| TilePos::new(x, top)).collect();
    tiles.extend((top..bottom).map(|y| TilePos::new(right, y)));
    tiles.extend((left + 1..=right).rev().map(|x| TilePos::new(x, bottom)));
    tiles.extend((top + 1..=bottom).rev().map(|y| TilePos::new(left, y)));
    tiles
}

/// A builder within reach of the surface works where it stands, whatever
/// position its route was taking it to: around a solid footprint, or
/// through a Scuttle Charge's own open tile into its outline.
#[test]
fn a_builder_in_reach_works_where_it_stands_from_every_side_and_corner() {
    use oxide_sim::BuildingKind;
    let around = [
        (0, -1),
        (1, -1),
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
    ];
    for kind in [
        BuildingKind::ScuttleCharge,
        BuildingKind::Turret,
        BuildingKind::Fabricator,
    ] {
        let anchor = TilePos::new(14, 10);
        let size = kind.size();
        let (state, id, site) = paid_site(kind, anchor, json!([]));
        let clearance = UnitKind::Harvester.stats().radius + oxide_sim::stats::WORK_FOOTPRINT_GAP;
        let surface = state.contact_surface(state.building(site).unwrap());
        for side in around {
            let from = beside(anchor, size, side);
            let to = beside(anchor, size, (-side.0, -side.1));
            let pos = surface.stance(from.center(), clearance);
            let waypoints = if kind == BuildingKind::ScuttleCharge {
                vec![anchor, to]
            } else {
                let ring = ring(anchor, size);
                let start = ring.iter().position(|&t| t == from).unwrap();
                let end = ring.iter().position(|&t| t == to).unwrap();
                (1..=(end + ring.len() - start) % ring.len())
                    .map(|step| ring[(start + step) % ring.len()])
                    .collect()
            };
            let mut state = walking(
                &state,
                id,
                pos,
                waypoints,
                Some(surface.stance(to.center(), clearance)),
            );
            assert!(state.in_building_work_reach(state.unit(id).unwrap(), site));
            for _ in 0..3 {
                state.tick(&[]);
            }
            let unit = state.unit(id).unwrap();
            assert!(
                state.building(site).unwrap().progress > 0,
                "{kind:?} from {side:?} never started: {unit:?}"
            );
            assert!(state.in_building_work_reach(unit, site));
        }
    }
}

/// A position chosen before a neighboring site took the ground beside it
/// can no longer be landed on. The builder chooses again rather than
/// shuttling between its goal tile's center and the contact band.
#[test]
fn a_builder_chooses_again_when_new_ground_spoils_its_position() {
    let anchor = TilePos::new(12, 10);
    let (state, id, site) = paid_site(
        oxide_sim::BuildingKind::Reclaimer,
        anchor,
        json!([{"player":0,"kind":"barricade","x":13,"y":10}]),
    );
    let goal = TilePos::new(12, 11);
    let spoiled = state.contact_surface(state.building(site).unwrap()).stance(
        chassis::fx::Vec2Fx::new(Fx::lit("12.98"), Fx::lit("11.33")),
        UnitKind::Harvester.stats().radius + oxide_sim::stats::WORK_FOOTPRINT_GAP,
    );
    let mut state = walking(&state, id, goal.center(), vec![goal], Some(spoiled));
    let mut started = false;
    for _ in 0..100 {
        state.tick(&[]);
        if state.building(site).unwrap().progress > 0 {
            started = true;
            break;
        }
    }
    assert!(started, "builder never started: {:?}", state.unit(id));
}

/// A worker docked on a Foundry's shoulder holds a route whose first leg
/// would graze the Foundry's outline. It backs out of the contact band and
/// walks on instead of standing refused forever.
#[test]
fn a_docked_worker_backs_off_a_footprint_its_first_leg_would_graze() {
    let (state, id, _) = paid_site(
        oxide_sim::BuildingKind::Turret,
        TilePos::new(20, 12),
        json!([{"player":0,"kind":"foundry","x":8,"y":7}]),
    );
    let goal = TilePos::new(10, 9);
    let mut state = walking(
        &state,
        id,
        chassis::fx::Vec2Fx::new(Fx::lit("9.6893"), Fx::lit("7.0726")),
        vec![TilePos::new(10, 7), TilePos::new(10, 8), goal],
        None,
    );
    let mut data = serde_json::to_value(&state).unwrap();
    data["units"][0]["order"] = json!(oxide_sim::Order::Run { goal: goal.into() });
    state = serde_json::from_value(data).unwrap();
    let mut arrived = false;
    for _ in 0..100 {
        state.tick(&[]);
        if state.unit(id).unwrap().tile() == goal {
            arrived = true;
            break;
        }
    }
    assert!(arrived, "worker never left the dock: {:?}", state.unit(id));
}
