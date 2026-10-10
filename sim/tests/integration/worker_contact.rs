//! Work requires stationary physical contact; cargo releases only after a full stop.
use crate::common;
use chassis::grid::as_index;

use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;
use common::{building, cmd, open_arena, unit};
use oxide_sim::{BuildingKind, Command, Event, Order, PlayerId, State, UnitId, UnitKind};
use serde_json::json;

fn release_scene(kind: UnitKind) -> (State, UnitId, oxide_sim::BuildingId) {
    let mut scenario = open_arena(24, 16, vec![unit(0, kind, 9, 7)]);
    scenario.players[0].scrap = 100;
    scenario
        .buildings
        .push(building(0, BuildingKind::Foundry, 10, 6));
    let state = scenario.build().unwrap();
    let foundry = state
        .buildings()
        .iter()
        .find(|b| b.anchor == TilePos::new(10, 6))
        .unwrap()
        .id;
    let id = state.units()[0].id;
    let point = state
        .contact_surface(state.building(foundry).unwrap())
        .stance(
            TilePos::new(9, 7).center(),
            kind.stats().radius + oxide_sim::stats::WORK_FOOTPRINT_GAP,
        );
    let mut data = serde_json::to_value(state).unwrap();
    data["tick"] = json!(1);
    data["units"][0]["pos"] = json!(point);
    data["units"][0]["carrying"] = json!(7);
    data["units"][0]["order"] = json!(Order::ReturnCargo {
        foundry,
        repair: false
    });
    (serde_json::from_value(data).unwrap(), id, foundry)
}

#[test]
fn cargo_stays_aboard_until_the_tenth_stationary_tick_even_when_reissued_and_restored() {
    for kind in [UnitKind::Harvester, UnitKind::Excavator] {
        let (mut state, id, foundry) = release_scene(kind);
        let mut control_data = serde_json::to_value(&state).unwrap();
        control_data["units"][0]["order"] = json!(Order::Idle);
        let mut control: State = serde_json::from_value(control_data).unwrap();
        for elapsed in 1..=oxide_sim::stats::UNLOAD_TICKS {
            control.tick(&[]);
            let bank = control.player(PlayerId(0)).scrap;
            let report = state.tick(&[cmd(
                0,
                Command::ReturnCargo {
                    units: vec![id],
                    foundry: Some(foundry),
                    repair: false,
                },
            )]);
            let worker = state.unit(id).unwrap();
            let deposits: Vec<_> = report
                .events
                .iter()
                .filter(|e| matches!(e, Event::ScrapDeposited { .. }))
                .collect();
            if elapsed < oxide_sim::stats::UNLOAD_TICKS {
                assert_eq!(worker.carrying, 7);
                assert_eq!(worker.unloading.unwrap().elapsed, elapsed);
                assert_eq!(state.player(PlayerId(0)).scrap, bank);
                assert!(deposits.is_empty());
                let mut restored: State =
                    serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
                let mut control = state.clone();
                assert_eq!(restored.tick(&[]), control.tick(&[]));
                assert_eq!(restored.hash(), control.hash());
            } else {
                assert_eq!(worker.carrying, 0);
                assert!(worker.unloading.is_none());
                assert_eq!(state.player(PlayerId(0)).scrap, bank + 7);
                assert!(
                    matches!(deposits.as_slice(), [Event::ScrapDeposited { unit, foundry: destination, amount: 7, .. }] if *unit == id && *destination == foundry)
                );
            }
        }
        for _ in 0..20 {
            assert!(
                !state
                    .tick(&[])
                    .events
                    .iter()
                    .any(|e| matches!(e, Event::ScrapDeposited { .. }))
            );
        }
    }
}

#[test]
fn final_delivery_clears_the_dock_after_unloading_at_the_surface() {
    let (state, id, _) = release_scene(UnitKind::Harvester);
    let mut data = serde_json::to_value(&state).unwrap();
    let b = state
        .buildings()
        .iter()
        .find(|b| b.anchor == TilePos::new(10, 6))
        .unwrap();
    data["units"][0]["pos"] = json!(state.contact_surface(b).stance(
        TilePos::new(12, 7).center(),
        UnitKind::Harvester.stats().radius + oxide_sim::stats::WORK_FOOTPRINT_GAP
    ));
    let mut state: State = serde_json::from_value(data).unwrap();
    for _ in 0..oxide_sim::stats::UNLOAD_TICKS {
        state.tick(&[]);
    }
    let worker = state.unit(id).unwrap();
    assert_eq!(worker.carrying, 0);
    assert_eq!(worker.order, Order::Idle);
    assert_eq!(worker.path.as_ref().unwrap().goal, TilePos::new(13, 7));
    for _ in 0..100 {
        state.tick(&[]);
    }
    assert!(state.unit(id).unwrap().pos.x >= Fx::from_num(13));
}

#[test]
fn displacement_restarts_release_and_stop_keeps_the_uncredited_load() {
    let (mut state, id, _) = release_scene(UnitKind::Harvester);
    for _ in 0..6 {
        state.tick(&[]);
    }
    let mut data = serde_json::to_value(&state).unwrap();
    data["units"][0]["pos"] = json!(Vec2Fx::new(Fx::lit("9.02"), Fx::lit("7.5")));
    state = serde_json::from_value(data).unwrap();
    state.tick(&[]);
    assert!(state.unit(id).unwrap().unloading.is_none());
    assert!(
        state
            .unit(id)
            .unwrap()
            .path
            .as_ref()
            .unwrap()
            .final_point
            .is_some()
    );
    let mut resumed = false;
    for _ in 0..100 {
        state.tick(&[]);
        if let Some(release) = state.unit(id).unwrap().unloading {
            assert_eq!(release.elapsed, 1);
            resumed = true;
            break;
        }
        assert_eq!(state.unit(id).unwrap().carrying, 7);
    }
    assert!(resumed);
    state.tick(&[cmd(0, Command::Stop { units: vec![id] })]);
    for _ in 0..20 {
        state.tick(&[]);
    }
    assert_eq!(state.unit(id).unwrap().carrying, 7);
    assert!(state.unit(id).unwrap().unloading.is_none());
    assert!(state.player(PlayerId(0)).scrap < 107);
}

#[test]
fn adjacent_build_repair_and_salvage_approach_before_advancing_work() {
    for kind in [UnitKind::Harvester, UnitKind::Excavator, UnitKind::Tender] {
        for job in ["build", "repair", "salvage"] {
            if kind == UnitKind::Tender && job != "repair" {
                continue;
            }
            let mut scenario = open_arena(24, 16, vec![unit(0, kind, 9, 5)]);
            scenario.players[0].scrap = 1000;
            scenario
                .buildings
                .push(building(0, BuildingKind::Fabricator, 10, 6));
            let state = scenario.build().unwrap();
            let target = state
                .buildings()
                .iter()
                .find(|b| b.anchor == TilePos::new(10, 6))
                .unwrap()
                .id;
            let id = state.units()[0].id;
            let mut data = serde_json::to_value(&state).unwrap();
            let slot = state
                .buildings()
                .iter()
                .position(|b| b.id == target)
                .unwrap();
            if job == "build" {
                data["buildings"][slot]["phase"] = json!({"phase": "site"});
            }
            if job != "salvage" {
                data["buildings"][slot]["hp"] = json!(100);
            }
            data["units"][0]["order"] = json!(match job {
                "build" => Order::Build { site: target },
                "repair" => Order::Repair { building: target },
                _ => Order::Salvage { building: target },
            });
            let mut state: State = serde_json::from_value(data).unwrap();
            let mut worked = false;
            for _ in 0..150 {
                let before = state.unit(id).unwrap();
                let b = state.building(target).unwrap();
                let reach = before.kind.stats().radius + oxide_sim::stats::WORK_REACH;
                let eligible = before.work_stopped()
                    && before
                        .pos
                        .dist_sq(state.contact_surface(b).closest(before.pos))
                        <= reach * reach;
                let old_progress = if job == "build" {
                    b.construction_progress().unwrap_or(0)
                } else {
                    before.progress
                };
                state.tick(&[]);
                let after = state.unit(id).unwrap();
                let progress = if job == "build" {
                    state
                        .building(target)
                        .unwrap()
                        .construction_progress()
                        .unwrap_or(0)
                } else {
                    after.progress
                };
                if progress > old_progress {
                    assert!(eligible, "{kind:?} advanced {job} outside physical reach");
                    worked = true;
                    break;
                }
            }
            assert!(worked, "{kind:?} never reached {job}");
        }
    }
}

#[test]
fn malformed_release_and_endpoint_snapshots_are_rejected() {
    let (mut state, id, _) = release_scene(UnitKind::Harvester);
    state.tick(&[]);
    let base = serde_json::to_value(&state).unwrap();
    for (field, value) in [
        ("elapsed", json!(0)),
        ("elapsed", json!(10)),
        ("foundry", json!(999)),
    ] {
        let mut bad = base.clone();
        bad["units"][0]["unloading"][field] = value;
        assert!(serde_json::from_value::<State>(bad).is_err());
    }
    let mut bad = base.clone();
    bad["units"][0]["carrying"] = json!(0);
    assert!(serde_json::from_value::<State>(bad).is_err());
    let mut displaced = base;
    displaced["units"][0]["pos"] = json!(Vec2Fx::new(Fx::lit("9.02"), Fx::lit("7.5")));
    state = serde_json::from_value(displaced).unwrap();
    state.tick(&[]);
    assert!(state.unit(id).unwrap().path.is_some());
    let base = serde_json::to_value(state).unwrap();
    for (field, value) in [
        ("next", json!(999)),
        ("waypoints", json!([])),
        ("final_point", json!(TilePos::new(20, 12).center())),
    ] {
        let mut bad = base.clone();
        bad["units"][0]["path"][field] = value;
        assert!(serde_json::from_value::<State>(bad).is_err());
    }
}

#[test]
fn field_repair_closes_to_each_pair_of_hulls_before_billing() {
    for welder in [UnitKind::Harvester, UnitKind::Excavator, UnitKind::Tender] {
        for patient in [UnitKind::Scuttler, UnitKind::Sentinel, UnitKind::Excavator] {
            let mut scenario =
                open_arena(24, 16, vec![unit(0, welder, 8, 7), unit(0, patient, 10, 7)]);
            scenario.players[0].scrap = 1000;
            let state = scenario.build().unwrap();
            let (id, target) = (state.units()[0].id, state.units()[1].id);
            let mut data = serde_json::to_value(state).unwrap();
            data["units"][1]["hp"] = json!(patient.stats().max_hp / 2);
            let mut state: State = serde_json::from_value(data).unwrap();
            state.tick(&[cmd(
                0,
                Command::RepairUnit {
                    units: vec![id],
                    target,
                    queue: false,
                },
            )]);
            let mut worked = false;
            for _ in 0..200 {
                let w = state.unit(id).unwrap();
                let p = state.unit(target).unwrap();
                let eligible = w.work_stopped() && p.work_stopped() && w.in_repair_reach(p);
                let before = w.progress;
                state.tick(&[]);
                if state.unit(id).unwrap().progress > before {
                    assert!(eligible, "{welder:?} welded {patient:?} before contact");
                    worked = true;
                    break;
                }
            }
            assert!(worked, "{welder:?} failed to reach {patient:?}");
            let before = state.unit(target).unwrap().hp;
            state.tick(&[cmd(
                0,
                Command::Run {
                    units: vec![target],
                    goal: TilePos::new(15, 7),
                    queue: false,
                },
            )]);
            assert_eq!(state.unit(target).unwrap().hp, before);
        }
    }
}

#[test]
fn losing_the_dropoff_cancels_release_and_only_automatic_delivery_retargets() {
    for automatic in [false, true] {
        let (mut state, id, foundry) = release_scene(UnitKind::Harvester);
        if automatic {
            let mut data = serde_json::to_value(state).unwrap();
            data["units"][0]["order"] = json!(Order::Harvest {
                node: TilePos::new(13, 7),
                anchor: TilePos::new(13, 7),
                retiring: true
            });
            state = serde_json::from_value(data).unwrap();
        }
        for _ in 0..5 {
            state.tick(&[]);
        }
        assert_eq!(state.unit(id).unwrap().unloading.unwrap().elapsed, 5);
        let mut data = serde_json::to_value(state).unwrap();
        data["buildings"]
            .as_array_mut()
            .unwrap()
            .retain(|b| b["id"] != json!(foundry));
        state = serde_json::from_value(data).unwrap();
        assert!(
            !state
                .tick(&[])
                .events
                .iter()
                .any(|e| matches!(e, Event::ScrapDeposited { .. }))
        );
        assert!(state.unit(id).unwrap().unloading.is_none());
        assert_eq!(state.unit(id).unwrap().carrying, 7);
        let mut deposited = 0;
        for _ in 0..500 {
            deposited += state
                .tick(&[])
                .events
                .iter()
                .filter(|e| matches!(e, Event::ScrapDeposited { .. }))
                .count();
        }
        assert_eq!(deposited, usize::from(automatic));
        assert_eq!(
            state.unit(id).unwrap().carrying,
            if automatic { 0 } else { 7 }
        );
    }
}

#[test]
fn harvester_closes_to_the_footprint_on_every_approach_side_before_gathering() {
    let node = TilePos::new(10, 7);
    for (dx, dy) in [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ] {
        let mut scenario = open_arena(
            24,
            16,
            vec![unit(
                0,
                UnitKind::Harvester,
                node.x + dx * 2,
                node.y + dy * 2,
            )],
        );
        let mut row: Vec<char> = scenario.map[as_index(node.y)].chars().collect();
        row[as_index(node.x)] = 's';
        scenario.map[as_index(node.y)] = row.into_iter().collect();
        let mut state = scenario.build().unwrap();
        let id = state.units()[0].id;
        state.tick(&[cmd(
            0,
            Command::Harvest {
                units: vec![id],
                node,
                queue: false,
            },
        )]);
        for _ in 0..200 {
            if state.unit(id).unwrap().carrying > 0 {
                break;
            }
            state.tick(&[]);
        }
        let worker = state.unit(id).unwrap();
        assert!(worker.carrying > 0, "approach {dx},{dy} never gathered");
        let edge = oxide_sim::geometry::footprint_contact(worker.pos, node, (1, 1));
        assert!(
            worker.in_work_reach(node, (1, 1)) && worker.pos.dist(edge) <= Fx::lit("0.12"),
            "approach {dx},{dy} left the chassis too far away: {:?}",
            worker.pos
        );
        assert!(worker.work_stopped());
    }
}

#[test]
fn incoming_workers_share_contact_space_without_reserving_it_from_far_away() {
    let node = TilePos::new(12, 8);
    let mut scenario = open_arena(
        24,
        18,
        vec![
            unit(0, UnitKind::Harvester, 9, 8),
            unit(0, UnitKind::Harvester, 9, 9),
            unit(0, UnitKind::Harvester, 10, 11),
        ],
    );
    let mut row: Vec<char> = scenario.map[8].chars().collect();
    row[12] = 'S';
    scenario.map[8] = row.into_iter().collect();
    let mut state = scenario.build().unwrap();
    let ids = state.units().iter().map(|unit| unit.id).collect();
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: ids,
            node,
            queue: false,
        },
    )]);
    assert!(state.units().iter().all(|worker| {
        worker
            .path
            .as_ref()
            .is_some_and(|path| path.final_point.is_some())
    }));
    for _ in 0..2000 {
        state.tick(&[]);
    }
    assert!(state.map().scrap_at(node) < oxide_sim::stats::RICH_SCRAP_NODE_AMOUNT);
    assert!(
        state
            .units()
            .iter()
            .all(|unit| matches!(unit.order, Order::Harvest { .. }))
    );
}

#[test]
fn a_crowded_crew_delivers_every_last_load_and_clears_the_dropoff() {
    for gap in [1, 4, 10] {
        for blocked in [false, true] {
            for approach in 0..3 {
                let node = TilePos::new(8 + gap, 12);
                let mut rows = vec![vec!['.'; 40]; 26];
                rows[12][as_index(node.x)] = 's';
                if blocked {
                    for row in &mut rows[11..=13] {
                        row[as_index(node.x) + 1] = '#';
                    }
                }
                let scenario: oxide_sim::Scenario = serde_json::from_value(json!({
                    "mode":"sandbox", "name":"crowded-delivery", "map":rows.into_iter().map(|r|r.into_iter().collect::<String>()).collect::<Vec<_>>(),
                    "players":[{"name":"Local","faction":"ferrous","scrap":0,"bot":false}],
                    "buildings":[{"player":0,"kind":"foundry","x":5,"y":11}],
                    "units":(0..12).map(|i| {
                        let (x,y) = match approach { 0 => (node.x-3+i%4,15+i/4), 1 => (node.x-3+i%4,9-i/4), _ => (node.x+3+i/4,10+i%4) };
                        unit(0,UnitKind::Harvester,x,y)
                    }).collect::<Vec<_>>()
                })).unwrap();
                let mut state = scenario.build().unwrap();
                let ids = state.units().iter().map(|u| u.id).collect();
                let command = cmd(
                    0,
                    Command::Harvest {
                        units: ids,
                        node,
                        queue: false,
                    },
                );
                let mut deposited = 0;
                for tick in 0..3000 {
                    let report = state.tick(if tick == 0 {
                        std::slice::from_ref(&command)
                    } else {
                        &[]
                    });
                    for event in report.events {
                        match event {
                            Event::ScrapDeposited { amount, .. } => deposited += amount,
                            Event::CommandRejected { .. } | Event::OrderStalled { .. } => {
                                panic!("unexpected failure: {event:?}")
                            }
                            _ => {}
                        }
                    }
                    if deposited == oxide_sim::stats::SCRAP_NODE_AMOUNT {
                        break;
                    }
                }
                assert_eq!(
                    deposited,
                    oxide_sim::stats::SCRAP_NODE_AMOUNT,
                    "gap {gap}, blocked {blocked}, approach {approach}"
                );
                assert!(state.units().iter().all(|u| u.carrying == 0));
                state.validate_invariants().unwrap();
            }
        }
    }
}

#[test]
fn both_workers_leave_scrap_overhang_to_unload_at_a_neighboring_foundry() {
    let node = TilePos::new(12, 10);
    for kind in [UnitKind::Harvester, UnitKind::Excavator] {
        let mut scenario = open_arena(28, 22, vec![unit(0, kind, 12, 11)]);
        scenario
            .buildings
            .push(building(0, BuildingKind::Foundry, 10, 10));
        let mut row: Vec<char> = scenario.map[10].chars().collect();
        row[12] = 'S';
        scenario.map[10] = row.into_iter().collect();
        let mut state = scenario.build().unwrap();
        let id = state.units()[0].id;
        state.tick(&[cmd(
            0,
            Command::Harvest {
                units: vec![id],
                node,
                queue: false,
            },
        )]);
        let mut deposits = 0;
        for _ in 0..1200 {
            deposits += state
                .tick(&[])
                .events
                .iter()
                .filter(|e| matches!(e,Event::ScrapDeposited{unit,..} if *unit==id))
                .count();
            if deposits == 3 {
                break;
            }
        }
        assert_eq!(deposits, 3, "{kind:?} failed to cycle beside the Foundry");
        state.validate_invariants().unwrap();
    }
}
