//! Explicit deliveries replace work and finish at the chosen Foundry.
mod common;

use chassis::grid::TilePos;
use common::{cmd, open_arena, open_arena_with, run_until, unit};
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::{
    BuildingId, BuildingKind, Command, Event, Order, PlayerId, State, UnitId, UnitKind,
};
use serde_json::json;

fn loaded(kind: UnitKind) -> (State, UnitId, BuildingId) {
    let mut state = open_arena(24, 16, vec![unit(0, kind, 9, 5)])
        .build()
        .unwrap();
    let worker = state.units()[0].id;
    let foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap()
        .id;
    let mut data = serde_json::to_value(&state).unwrap();
    data["units"][0]["carrying"] = json!(7);
    state = serde_json::from_value(data).unwrap();
    (state, worker, foundry)
}

fn delivery(worker: UnitId, foundry: Option<BuildingId>, repair: bool) -> Command {
    Command::ReturnCargo {
        units: vec![worker],
        foundry,
        repair,
    }
}

fn accepted(events: &[Event]) {
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "{events:?}"
    );
}

#[test]
fn return_cargo_replaces_work_and_queue_then_deposits_once_and_stays() {
    for kind in [UnitKind::Harvester, UnitKind::Excavator] {
        let (mut state, worker, foundry) = loaded(kind);
        state.tick(&[cmd(
            0,
            Command::Patrol {
                units: vec![worker],
                waypoints: vec![TilePos::new(16, 5), TilePos::new(16, 8)],
            },
        )]);
        assert!(!state.unit(worker).unwrap().queue.is_empty());
        let report = state.tick(&[cmd(0, delivery(worker, None, false))]);
        accepted(&report.events);
        let u = state.unit(worker).unwrap();
        assert!(
            matches!(u.order, Order::ReturnCargo { foundry: f, repair: false } if f == foundry)
        );
        assert!(u.queue.is_empty());
        let restored: State =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(state.hash(), restored.hash());
        let events = run_until(&mut state, 1000, |s, _| {
            s.unit(worker).unwrap().carrying == 0
        });
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(
                    e,
                    Event::ScrapDeposited {
                        player: PlayerId(0),
                        amount: 7
                    }
                ))
                .count(),
            1
        );
        let b = state.building(foundry).unwrap();
        let u = state.unit(worker).unwrap();
        assert!(u.in_harvest_reach(b.anchor, b.stats().size));
        assert!(matches!(u.order, Order::Idle));
        for _ in 0..200 {
            state.tick(&[]);
            assert!(matches!(state.unit(worker).unwrap().order, Order::Idle));
            state.validate_invariants().unwrap();
        }
    }
}

#[test]
fn return_cargo_deposits_before_repair_even_with_an_empty_bank() {
    for kind in [UnitKind::Harvester, UnitKind::Excavator] {
        let (state, worker, foundry) = loaded(kind);
        let mut data = serde_json::to_value(&state).unwrap();
        let b = data["buildings"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|b| b["id"] == json!(foundry))
            .unwrap();
        b["hp"] = json!(b["hp"].as_u64().unwrap() - 1);
        data["players"][0]["scrap"] = json!(0);
        let mut state: State = serde_json::from_value(data).unwrap();
        accepted(
            &state
                .tick(&[cmd(0, delivery(worker, Some(foundry), true))])
                .events,
        );
        let events = run_until(&mut state, 1000, |s, _| {
            s.unit(worker).unwrap().carrying == 0
        });
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::ScrapDeposited { amount: 7, .. }))
        );
        assert!(
            matches!(state.unit(worker).unwrap().order, Order::Repair { building } if building == foundry)
        );
        run_until(&mut state, 500, |s, _| {
            matches!(s.unit(worker).unwrap().order, Order::Idle)
        });
        let b = state.building(foundry).unwrap();
        assert_eq!(b.hp, b.stats().max_hp);
    }
}

#[test]
fn return_cargo_still_delivers_when_the_patient_is_healed_en_route() {
    let (state, worker, foundry) = loaded(UnitKind::Harvester);
    let mut data = serde_json::to_value(&state).unwrap();
    data["buildings"][0]["hp"] = json!(1);
    let mut state: State = serde_json::from_value(data).unwrap();
    accepted(
        &state
            .tick(&[cmd(0, delivery(worker, Some(foundry), true))])
            .events,
    );
    let mut data = serde_json::to_value(&state).unwrap();
    data["buildings"][0]["hp"] = json!(state.building(foundry).unwrap().stats().max_hp);
    state = serde_json::from_value(data).unwrap();
    run_until(&mut state, 1000, |s, _| {
        s.unit(worker).unwrap().carrying == 0
    });
    assert!(matches!(state.unit(worker).unwrap().order, Order::Idle));
}

#[test]
fn return_cargo_refusals_preserve_the_existing_program_and_cargo() {
    let (mut state, worker, own) = loaded(UnitKind::Harvester);
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![worker],
            waypoints: vec![TilePos::new(16, 5), TilePos::new(16, 8)],
        },
    )]);
    let foreign = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1))
        .unwrap()
        .id;
    for command in [
        delivery(worker, Some(foreign), false),
        delivery(worker, Some(BuildingId(u32::MAX)), false),
        delivery(worker, None, true),
        delivery(UnitId(u32::MAX), Some(own), false),
    ] {
        let mut rejected = state.clone();
        let mut control = state.clone();
        let report = rejected.tick(&[cmd(0, command)]);
        assert!(
            report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. }))
        );
        control.tick(&[]);
        assert_eq!(control.hash(), rejected.hash());
    }
}

#[test]
fn return_cargo_skips_a_sealed_near_foundry_and_honors_explicit_destinations() {
    let mut scenario = open_arena_with(24, 16, vec![unit(0, UnitKind::Excavator, 5, 6)], |rows| {
        for row in rows.iter_mut().take(4) {
            row[3] = '#';
        }
        for cell in rows[3].iter_mut().take(4) {
            *cell = '#';
        }
    });
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 16,
        y: 4,
    });
    let state = scenario.build().unwrap();
    let worker = state.units()[0].id;
    let near = state
        .buildings()
        .iter()
        .find(|b| b.anchor == TilePos::new(1, 1))
        .unwrap()
        .id;
    let far = state
        .buildings()
        .iter()
        .find(|b| b.anchor == TilePos::new(16, 4))
        .unwrap()
        .id;
    let mut data = serde_json::to_value(&state).unwrap();
    data["units"][0]["carrying"] = json!(5);
    let mut state: State = serde_json::from_value(data).unwrap();
    let mut control = state.clone();
    let report = state.tick(&[cmd(0, delivery(worker, Some(near), false))]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: oxide_sim::command::RejectReason::UnreachableGoal,
            ..
        }
    )));
    control.tick(&[]);
    assert_eq!(control.hash(), state.hash());
    accepted(&state.tick(&[cmd(0, delivery(worker, None, false))]).events);
    assert!(
        matches!(state.unit(worker).unwrap().order, Order::ReturnCargo { foundry, .. } if foundry == far)
    );
    run_until(&mut state, 1000, |s, _| {
        s.unit(worker).unwrap().carrying == 0
    });
}

#[test]
fn return_cargo_retains_the_load_when_its_foundry_disappears() {
    let (mut state, worker, foundry) = loaded(UnitKind::Harvester);
    accepted(
        &state
            .tick(&[cmd(0, delivery(worker, Some(foundry), true))])
            .events,
    );
    let mut data = serde_json::to_value(&state).unwrap();
    data["buildings"]
        .as_array_mut()
        .unwrap()
        .retain(|b| b["id"] != json!(foundry));
    state = serde_json::from_value(data).unwrap();
    let report = state.tick(&[]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { unit, .. } if *unit == worker))
    );
    assert_eq!(state.unit(worker).unwrap().carrying, 7);
    assert!(matches!(state.unit(worker).unwrap().order, Order::Idle));
}

#[test]
fn return_cargo_rejects_forged_worker_and_target_references() {
    let (state, worker, foundry) = loaded(UnitKind::Harvester);
    let base = serde_json::to_value(&state).unwrap();
    for queued in [false, true] {
        for target in [foundry, BuildingId(u32::MAX)] {
            let mut data = base.clone();
            let order = json!({"order": "return_cargo", "foundry": target, "repair": true});
            if queued {
                data["units"][0]["queue"] = json!([order]);
            } else {
                data["units"][0]["order"] = order;
            }
            if target == foundry {
                data["units"][0]["kind"] = json!("sentinel");
                data["units"][0]["carrying"] = json!(0);
            }
            assert!(
                serde_json::from_value::<State>(data).is_err(),
                "accepted forged delivery for {worker:?}"
            );
        }
    }
}

#[test]
fn return_cargo_cancels_a_partial_harvest_and_can_be_overridden_by_move() {
    let mut state = open_arena_with(24, 16, vec![unit(0, UnitKind::Harvester, 10, 5)], |rows| {
        rows[5][11] = 's'
    })
    .build()
    .unwrap();
    let worker = state.units()[0].id;
    accepted(
        &state
            .tick(&[cmd(
                0,
                Command::Harvest {
                    units: vec![worker],
                    node: TilePos::new(11, 5),
                    queue: false,
                },
            )])
            .events,
    );
    run_until(&mut state, 100, |s, _| {
        s.unit(worker).unwrap().carrying >= 3
    });
    accepted(&state.tick(&[cmd(0, delivery(worker, None, false))]).events);
    let returning: State = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    accepted(
        &state
            .tick(&[cmd(
                0,
                Command::Move {
                    units: vec![worker],
                    goal: TilePos::new(17, 8),
                    queue: false,
                },
            )])
            .events,
    );
    run_until(&mut state, 500, |s, _| {
        matches!(s.unit(worker).unwrap().order, Order::Idle)
    });
    assert!(state.unit(worker).unwrap().carrying > 0);
    let mut state = returning;
    run_until(&mut state, 1000, |s, _| {
        s.unit(worker).unwrap().carrying == 0
    });
    for _ in 0..200 {
        state.tick(&[]);
    }
    assert!(matches!(state.unit(worker).unwrap().order, Order::Idle));
}

#[test]
fn return_cargo_honors_a_far_explicit_foundry_and_saturates_the_bank() {
    let mut scenario = open_arena(24, 16, vec![unit(0, UnitKind::Excavator, 3, 3)]);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 16,
        y: 4,
    });
    let state = scenario.build().unwrap();
    let far = state
        .buildings()
        .iter()
        .find(|b| b.anchor == TilePos::new(16, 4))
        .unwrap()
        .id;
    let worker = state.units()[0].id;
    let mut data = serde_json::to_value(&state).unwrap();
    data["units"][0]["carrying"] = json!(5);
    let mut state: State = serde_json::from_value(data).unwrap();
    accepted(
        &state
            .tick(&[cmd(0, delivery(worker, Some(far), false))])
            .events,
    );
    assert!(
        matches!(state.unit(worker).unwrap().order, Order::ReturnCargo { foundry, .. } if foundry == far)
    );
    let events = run_until(&mut state, 1000, |s, _| {
        s.unit(worker).unwrap().carrying == 0
    });
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ScrapDeposited { amount: 5, .. }))
    );
    let b = state.building(far).unwrap();
    assert!(
        state
            .unit(worker)
            .unwrap()
            .in_harvest_reach(b.anchor, b.stats().size)
    );
    let mut data = serde_json::to_value(&state).unwrap();
    data["units"][0]["carrying"] = json!(5);
    data["players"][0]["scrap"] = json!(u32::MAX - 2);
    state = serde_json::from_value(data).unwrap();
    let report = state.tick(&[cmd(0, delivery(worker, Some(far), false))]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::ScrapDeposited { amount: 2, .. }))
    );
    assert_eq!(state.player(PlayerId(0)).scrap, u32::MAX);
}

#[test]
fn return_cargo_refuses_empty_workers_and_unfinished_foundries_without_changing_work() {
    for empty in [false, true] {
        let (state, worker, _) = loaded(UnitKind::Harvester);
        let mut data = serde_json::to_value(&state).unwrap();
        if empty {
            data["units"][0]["carrying"] = json!(0);
        } else {
            data["buildings"][0]["built"] = json!(false);
            data["buildings"][0]["hp"] = json!(1);
        }
        let mut state: State = serde_json::from_value(data).unwrap();
        let mut control = state.clone();
        let report = state.tick(&[cmd(0, delivery(worker, None, false))]);
        assert!(
            report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. }))
        );
        control.tick(&[]);
        assert_eq!(state.hash(), control.hash());
    }
}
