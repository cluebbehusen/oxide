//! Local building interaction preserves tile routing and permits a bounded final approach.
use chassis::{fx::Fx, grid::TilePos};
use oxide_sim::{Command, Event, PlayerCommand, PlayerId, Scenario, State, Target, UnitKind};
use serde_json::json;

fn scene(kind: &str, own_building: bool) -> State {
    let scenario: Scenario = serde_json::from_value(json!({
        "name":"local contact", "mode":"sandbox", "seed":42, "map":vec!["................................";24],
        "players":[{"name":"Local","faction":"ferrous","scrap":10000,"bot":false},{"name":"Target","faction":"cupric","scrap":0,"bot":false}],
        "units":[{"player":0,"kind":kind,"x":12,"y":9}],
        "buildings":[{"player":if own_building {0} else {1},"kind":"fabricator","x":10,"y":10}]
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
        "name":"sealed wall", "mode":"sandbox", "seed":42, "map":vec!["........................";16],
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
            "name":"unusable stance", "mode":"sandbox", "seed":42,
            "map":map.into_iter().map(|row| row.into_iter().collect::<String>()).collect::<Vec<_>>(),
            "players":[{"name":"Local","faction":"ferrous","scrap":10000,"bot":false},
                       {"name":"Target","faction":"cupric","scrap":0,"bot":false}],
            "units":[{"player":0,"kind":kind,"x":4,"y":4}],
            "buildings":[{"player":if own_building {0} else {1},"kind":"foundry","x":5,"y":5}]
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
            "name":"neighbor corner", "mode":"sandbox", "seed":42, "map":vec!["................................";24],
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
