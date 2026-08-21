//! Harvester unit-welding: the machine mirror of the building repair
//! suite. Headless scenarios through the public API only, like
//! `repair.rs` — billing exactness, the stacked-welder refund, the
//! chase-not-weld rule for walking patients, fire winning ties, and the
//! command validation ring.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::{
    BuildingKind, Command, Event, Faction, Order, PlayerCommand, PlayerId, Scenario, State, Target,
    UnitId, UnitKind, UnitRepairSource,
};

fn arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "weld-arena".into(),
        seed: 42,
        map: vec![
            "################".into(),
            "#1.............#".into(),
            "#..............#".into(),
            "#.....##.......#".into(),
            "#.....##...s...#".into(),
            "#..........s...#".into(),
            "#............2.#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 200,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn run_until(
    state: &mut State,
    max_ticks: u64,
    mut stop: impl FnMut(&State, &[Event]) -> bool,
) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..max_ticks {
        let report = state.tick(&[]);
        let done = stop(state, &report.events);
        all.extend(report.events);
        if done {
            return all;
        }
    }
    panic!("condition not reached within {max_ticks} ticks");
}

const PATIENT_MAX: u32 = 60; // harvester max_hp, the suite's patient

/// Walks the raider beside the patient (auto-acquire does the gnawing),
/// lets it chew to at most `floor` hp, then pulls it back to its corner.
/// Returns the wound.
fn wound(state: &mut State, patient: UnitId, raider: UnitId, floor: u32) -> u32 {
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![raider],
            goal: TilePos::new(6, 2),
            queue: false,
        },
    )]);
    run_until(state, 2_000, |s, _| s.unit(patient).unwrap().hp <= floor);
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![raider],
            goal: TilePos::new(12, 6),
            queue: false,
        },
    )]);
    run_until(state, 600, |s, _| {
        s.unit(raider).unwrap().tile() == TilePos::new(12, 6)
    });
    let hp = state.unit(patient).unwrap().hp;
    assert!(
        hp > 0 && hp < PATIENT_MAX,
        "test premise: the gnawing must leave a live, weldable patient (hp {hp})"
    );
    hp
}

/// The standard cast: a welder, a patient (both harvesters — unit weld
/// steps are 0/1 hp on the harvester ramp, which the exact-billing
/// tests count on), and an enemy scuttler to do the wounding.
fn cast() -> Vec<UnitSpec> {
    vec![
        unit(0, UnitKind::Harvester, 2, 5), // welder
        unit(0, UnitKind::Harvester, 4, 2), // patient
        unit(1, UnitKind::Scuttler, 12, 6), // raider
    ]
}

#[derive(Clone, Copy, Debug)]
enum DepartingWork {
    Attack,
    Harvest,
    Build,
    Repair,
    Salvage,
    RepairUnit,
}

fn weld_step_progress(kind: UnitKind) -> u32 {
    let stats = kind.stats();
    (0..stats.train_ticks)
        .find(|p| {
            stats.max_hp * (p + 1) / stats.train_ticks > stats.max_hp * *p / stats.train_ticks
        })
        .expect("every unit repair ramp eventually gains hp")
}

fn departure_case(work: DepartingWork, tick: u64) -> (State, UnitId, UnitId, PlayerCommand) {
    let patient_kind = if matches!(work, DepartingWork::Attack) {
        UnitKind::Sentinel
    } else {
        UnitKind::Harvester
    };
    let mut units = vec![
        unit(0, UnitKind::Harvester, 4, 5), // welder
        unit(0, patient_kind, 5, 5),        // patient
    ];
    match work {
        DepartingWork::Attack => units.push(unit(1, UnitKind::Sentinel, 10, 5)),
        DepartingWork::RepairUnit => units.push(unit(0, UnitKind::Sentinel, 10, 5)),
        _ => {}
    }
    let mut scenario = arena(units);
    if matches!(work, DepartingWork::Repair | DepartingWork::Salvage) {
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 9,
            y: 2,
        });
    }
    let state = scenario.build().unwrap();
    let (welder, patient) = (state.units()[0].id, state.units()[1].id);
    let extra_unit = state.units().get(2).map(|u| u.id);
    let work_building = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0) && b.kind == BuildingKind::Turret)
        .map(|b| b.id);

    let mut json = serde_json::to_value(state).unwrap();
    json["tick"] = serde_json::json!(tick);
    json["units"][0]["order"] = serde_json::json!({"order": "repair_unit", "unit": patient});
    json["units"][0]["progress"] = serde_json::json!(weld_step_progress(patient_kind));
    json["units"][1]["hp"] = serde_json::json!(patient_kind.stats().max_hp - 10);
    if matches!(work, DepartingWork::RepairUnit) {
        json["units"][2]["hp"] = serde_json::json!(UnitKind::Sentinel.stats().max_hp - 10);
    }
    if matches!(work, DepartingWork::Repair) {
        let building = work_building.expect("repair case has a Turret");
        let slot = json["buildings"]
            .as_array()
            .unwrap()
            .iter()
            .position(|b| b["id"] == serde_json::json!(building))
            .unwrap();
        json["buildings"][slot]["hp"] =
            serde_json::json!(BuildingKind::Turret.base_stats().max_hp - 10);
    }
    let state: State = serde_json::from_value(json).unwrap();

    let command = match work {
        DepartingWork::Attack => Command::Attack {
            units: vec![patient],
            target: Target::Unit(extra_unit.expect("attack case has a target")),
            queue: false,
        },
        DepartingWork::Harvest => Command::Harvest {
            units: vec![patient],
            node: TilePos::new(11, 5),
            queue: false,
        },
        DepartingWork::Build => Command::Build {
            units: vec![patient],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(9, 2),
            queue: false,
            defer: false,
        },
        DepartingWork::Repair => Command::Repair {
            units: vec![patient],
            building: work_building.expect("repair case has a Turret"),
            queue: false,
        },
        DepartingWork::Salvage => Command::Salvage {
            units: vec![patient],
            building: work_building.expect("salvage case has a Turret"),
            queue: false,
        },
        DepartingWork::RepairUnit => Command::RepairUnit {
            units: vec![patient],
            target: extra_unit.expect("unit-repair case has a target"),
            queue: false,
        },
    };
    (state, welder, patient, cmd(0, command))
}

#[test]
fn harvesters_weld_wounded_machines_for_a_price() {
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let hurt = wound(&mut state, patient, raider, 45);
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |s, _| {
        s.unit(patient).unwrap().hp == PATIENT_MAX
    });
    let spent = bank_before - state.player(PlayerId(0)).scrap;
    let healed = PATIENT_MAX - hurt;
    assert!(spent > 0, "welding a machine is never free");
    assert!(
        spent < healed,
        "but under a scrap per hp on the harvester's price (spent {spent} for {healed} hp)"
    );
    // The job wraps up on its own; the patient was never re-ordered.
    run_until(&mut state, 20, |s, _| {
        s.unit(welder).unwrap().order == Order::Idle
    });
    assert_eq!(state.unit(patient).unwrap().order, Order::Idle);
}

#[test]
fn the_torch_bills_its_first_scrap_before_free_hp_can_land() {
    // Billing lands at each interval's start: the first coin drops with
    // (or before) the first welded hp, so chip welds pay their coin.
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    wound(&mut state, patient, raider, 45);
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 100, |s, _| {
        s.player(PlayerId(0)).scrap < bank_before
    });
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        bank_before - 1,
        "exactly one scrap up front, not a free first interval"
    );
}

#[test]
fn a_rejected_welders_prepaid_coin_comes_back() {
    // Two FRESH welders join a patient one hp short of full. Their
    // meters run in phase — both bill their first coin on the tick
    // their first hp comes due — but the ceiling accepts one step and
    // rejects the other whole. The rejected welder's coin must come
    // back at resolution, exactly like the building ledger.
    let mut units = cast();
    units.push(unit(0, UnitKind::Harvester, 2, 6));
    units.push(unit(0, UnitKind::Harvester, 2, 7));
    let mut state = arena(units).build().unwrap();
    let (opener, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let fresh = vec![state.units()[3].id, state.units()[4].id];
    wound(&mut state, patient, raider, 45);
    // Park the fresh pair in body contact BEFORE they are needed, so
    // their first weld ticks are their first meter ticks.
    for (torch, park) in fresh.iter().zip([TilePos::new(3, 2), TilePos::new(5, 2)]) {
        state.tick(&[cmd(
            0,
            Command::Move {
                units: vec![*torch],
                goal: park,
                queue: false,
            },
        )]);
        run_until(&mut state, 300, |s, _| {
            s.unit(*torch).unwrap().tile() == park
        });
    }
    // The opener welds to exactly one hp short (harvester steps are
    // never more than 1 hp per tick: ramp 60 over 100 ticks).
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![opener],
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 900, |s, _| {
        s.unit(patient).unwrap().hp >= PATIENT_MAX - 1
    });
    assert_eq!(
        state.unit(patient).unwrap().hp,
        PATIENT_MAX - 1,
        "test premise: stopped one hp short"
    );
    state.tick(&[cmd(
        0,
        Command::Stop {
            units: vec![opener],
        },
    )]);
    // Both fresh torches take the last hp together.
    let bank_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: fresh.clone(),
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 60, |s, _| {
        s.unit(patient).unwrap().hp == PATIENT_MAX
    });
    let spent = bank_before - state.player(PlayerId(0)).scrap;
    assert_eq!(
        spent, 1,
        "one hp landed, one scrap billed — the rejected step's coin refunds"
    );
}

#[test]
fn a_walking_patient_is_chased_not_welded() {
    // The both-stationary rule: the torch never rides along with a
    // retreat. The welder trails the walking patient and the wound
    // stays open until the patient stops — and the patient's own
    // program is never evicted by the weld.
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let hurt = wound(&mut state, patient, raider, 45);
    // Outside the parked raider's aggro — the walk must stay a walk.
    let goal = TilePos::new(6, 7);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![patient],
            goal,
            queue: false,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |s, _| {
        s.unit(patient).unwrap().tile() == goal
    });
    assert_eq!(
        state.unit(patient).unwrap().hp,
        hurt,
        "no hp landed while the patient walked"
    );
    // Parked, the chase closes and the weld begins.
    run_until(&mut state, 200, |s, _| s.unit(patient).unwrap().hp > hurt);
}

#[test]
fn a_move_landing_mid_weld_rides_no_farewell_heal() {
    // Stationarity is intent: the tick a Move lands, the patient's
    // order is set but its path is not built until its own brain runs
    // — on the parity where the welder thinks first, path.is_none()
    // used to let one heal ride the departure. Both parities must
    // refuse it.
    for offset in [0u32, 1u32] {
        let mut state = arena(cast()).build().unwrap();
        let (welder, patient, raider) = (
            state.units()[0].id,
            state.units()[1].id,
            state.units()[2].id,
        );
        let hurt = wound(&mut state, patient, raider, 40);
        state.tick(&[cmd(
            0,
            Command::RepairUnit {
                units: vec![welder],
                target: patient,
                queue: false,
            },
        )]);
        run_until(&mut state, 600, |s, _| s.unit(patient).unwrap().hp > hurt);
        for _ in 0..offset {
            state.tick(&[]);
        }
        let before = state.unit(patient).unwrap().hp;
        state.tick(&[cmd(
            0,
            Command::Move {
                units: vec![patient],
                goal: TilePos::new(12, 2),
                queue: false,
            },
        )]);
        assert_eq!(
            state.unit(patient).unwrap().hp,
            before,
            "no heal may land on the departure tick (offset {offset})"
        );
    }
}

#[test]
fn every_departing_work_order_rides_no_farewell_heal() {
    for work in [
        DepartingWork::Attack,
        DepartingWork::Harvest,
        DepartingWork::Build,
        DepartingWork::Repair,
        DepartingWork::Salvage,
        DepartingWork::RepairUnit,
    ] {
        for tick in [0, 1] {
            let (mut state, welder, patient, command) = departure_case(work, tick);
            let before_hp = state.unit(patient).unwrap().hp;
            let before_pos = state.unit(patient).unwrap().pos;
            let report = state.tick(&[command]);
            assert!(
                !report
                    .events
                    .iter()
                    .any(|event| matches!(event, Event::CommandRejected { .. })),
                "{work:?} must be a valid departure order at tick {tick}: {:?}",
                report.events
            );
            assert!(
                state.unit(patient).unwrap().pos != before_pos,
                "{work:?} must actually depart at tick {tick}"
            );
            assert_eq!(
                state.unit(patient).unwrap().hp,
                before_hp,
                "{work:?} rode a farewell heal at tick {tick}"
            );
            assert!(
                !report.events.iter().any(|event| matches!(
                    event,
                    Event::UnitRepaired {
                        unit,
                        source: UnitRepairSource::FieldWelder { unit: source },
                        ..
                    } if *unit == patient && *source == welder
                )),
                "{work:?} emitted a field-weld event on departure at tick {tick}"
            );
        }
    }
}

#[test]
fn a_stationary_work_order_remains_weldable() {
    for tick in [0, 1] {
        let scenario = arena(vec![
            unit(0, UnitKind::Harvester, 4, 5), // welder
            unit(0, UnitKind::Harvester, 5, 5), // patient and second welder
            unit(0, UnitKind::Harvester, 6, 5), // second patient
        ]);
        let state = scenario.build().unwrap();
        let (welder, patient, other) = (
            state.units()[0].id,
            state.units()[1].id,
            state.units()[2].id,
        );
        let mut json = serde_json::to_value(state).unwrap();
        json["tick"] = serde_json::json!(tick);
        json["units"][0]["order"] = serde_json::json!({"order": "repair_unit", "unit": patient});
        json["units"][0]["progress"] = serde_json::json!(weld_step_progress(UnitKind::Harvester));
        json["units"][1]["hp"] = serde_json::json!(UnitKind::Harvester.stats().max_hp - 10);
        json["units"][2]["hp"] = serde_json::json!(UnitKind::Harvester.stats().max_hp - 10);
        let mut state: State = serde_json::from_value(json).unwrap();

        let before = state.unit(patient).unwrap().hp;
        let report = state.tick(&[cmd(
            0,
            Command::RepairUnit {
                units: vec![patient],
                target: other,
                queue: false,
            },
        )]);
        assert_eq!(
            state.unit(patient).unwrap().pos,
            TilePos::new(5, 5).center()
        );
        assert_eq!(
            state.unit(patient).unwrap().hp,
            before + 1,
            "a pathless RepairUnit worker must remain weldable at tick {tick}"
        );
        assert!(report.events.iter().any(|event| matches!(
            event,
            Event::UnitRepaired {
                unit,
                source: UnitRepairSource::FieldWelder { unit: source },
                amount: 1,
                ..
            } if *unit == patient && *source == welder
        )));
    }
}

#[test]
fn a_patient_that_parks_during_its_brain_is_weldable_on_both_parities() {
    for tick in [0, 1] {
        let scenario = arena(vec![
            unit(0, UnitKind::Harvester, 4, 5), // welder
            unit(0, UnitKind::Sentinel, 5, 5),  // patient
            unit(1, UnitKind::Sentinel, 7, 5),  // in-range target
        ]);
        let state = scenario.build().unwrap();
        let (welder, patient, target) = (
            state.units()[0].id,
            state.units()[1].id,
            state.units()[2].id,
        );
        let mut json = serde_json::to_value(state).unwrap();
        json["tick"] = serde_json::json!(tick);
        json["units"][0]["order"] = serde_json::json!({"order": "repair_unit", "unit": patient});
        json["units"][0]["progress"] = serde_json::json!(weld_step_progress(UnitKind::Sentinel));
        json["units"][1]["hp"] = serde_json::json!(UnitKind::Sentinel.stats().max_hp - 10);
        json["units"][1]["order"] = serde_json::to_value(Order::Attack {
            target: Target::Unit(target),
            resume: None,
        })
        .unwrap();
        json["units"][1]["path"] = serde_json::json!({
            "goal": {"x": 7, "y": 5},
            "waypoints": [{"x": 6, "y": 5}, {"x": 7, "y": 5}],
            "next": 0
        });
        let mut state: State = serde_json::from_value(json).unwrap();

        let before = state.unit(patient).unwrap().hp;
        let report = state.tick(&[]);
        assert!(
            state.unit(patient).unwrap().path.is_none(),
            "the in-range attack must park the patient"
        );
        assert_eq!(
            state.unit(patient).unwrap().hp,
            before + 1,
            "a path cleared by the patient's brain must not suppress the weld at tick {tick}"
        );
        assert!(report.events.iter().any(|event| matches!(
            event,
            Event::UnitRepaired {
                unit,
                source: UnitRepairSource::FieldWelder { unit: source },
                amount: 1,
                ..
            } if *unit == patient && *source == welder
        )));
    }
}

#[test]
fn a_departing_patient_propagates_through_an_in_reach_weld_chain() {
    for tick in [0, 1] {
        let scenario = arena(vec![
            unit(0, UnitKind::Harvester, 4, 5), // A repairs B
            unit(0, UnitKind::Harvester, 5, 5), // B repairs C
            unit(0, UnitKind::Harvester, 6, 5), // C departs
        ]);
        let state = scenario.build().unwrap();
        let (a, b, c) = (
            state.units()[0].id,
            state.units()[1].id,
            state.units()[2].id,
        );
        let mut json = serde_json::to_value(state).unwrap();
        json["tick"] = serde_json::json!(tick);
        json["units"][0]["order"] = serde_json::json!({"order": "repair_unit", "unit": b});
        json["units"][0]["progress"] = serde_json::json!(weld_step_progress(UnitKind::Harvester));
        json["units"][1]["order"] = serde_json::json!({"order": "repair_unit", "unit": c});
        json["units"][1]["progress"] = serde_json::json!(weld_step_progress(UnitKind::Harvester));
        json["units"][1]["hp"] = serde_json::json!(UnitKind::Harvester.stats().max_hp - 10);
        json["units"][2]["hp"] = serde_json::json!(UnitKind::Harvester.stats().max_hp - 10);
        let mut state: State = serde_json::from_value(json).unwrap();

        let before_hp = state.unit(b).unwrap().hp;
        let before_pos = state.unit(b).unwrap().pos;
        let report = state.tick(&[cmd(
            0,
            Command::Move {
                units: vec![c],
                goal: TilePos::new(10, 7),
                queue: false,
            },
        )]);
        assert_eq!(
            state.unit(b).unwrap().hp,
            before_hp,
            "A -> B must be rejected when B chases departing C at tick {tick}"
        );
        assert_ne!(
            state.unit(b).unwrap().pos,
            before_pos,
            "B must take the chase path at tick {tick}"
        );
        assert!(!report.events.iter().any(|event| matches!(
            event,
            Event::UnitRepaired {
                unit,
                source: UnitRepairSource::FieldWelder { unit: source },
                ..
            } if *unit == b && *source == a
        )));
    }
}

#[test]
fn a_patient_evicted_from_freshly_claimed_ground_rides_no_heal() {
    for tick in [0, 1] {
        let scenario = arena(vec![
            unit(0, UnitKind::Harvester, 4, 5), // welder
            unit(0, UnitKind::Harvester, 5, 5), // patient in the new footprint
            unit(0, UnitKind::Harvester, 6, 5), // founder
        ]);
        let state = scenario.build().unwrap();
        let (welder, patient, founder) = (
            state.units()[0].id,
            state.units()[1].id,
            state.units()[2].id,
        );
        let mut json = serde_json::to_value(state).unwrap();
        json["tick"] = serde_json::json!(tick);
        json["units"][0]["order"] = serde_json::json!({"order": "repair_unit", "unit": patient});
        json["units"][0]["progress"] = serde_json::json!(weld_step_progress(UnitKind::Harvester));
        json["units"][1]["hp"] = serde_json::json!(UnitKind::Harvester.stats().max_hp - 10);
        let mut state: State = serde_json::from_value(json).unwrap();

        let before_hp = state.unit(patient).unwrap().hp;
        let before_pos = state.unit(patient).unwrap().pos;
        let report = state.tick(&[cmd(
            0,
            Command::Build {
                units: vec![founder],
                kind: BuildingKind::Turret,
                anchor: TilePos::new(5, 5),
                queue: false,
                defer: false,
            },
        )]);
        assert!(
            state
                .buildings()
                .iter()
                .any(|building| building.anchor == TilePos::new(5, 5)),
            "the command must claim the patient's ground at tick {tick}"
        );
        assert_ne!(
            state.unit(patient).unwrap().pos,
            before_pos,
            "the footprint eviction must move the patient at tick {tick}"
        );
        assert_eq!(
            state.unit(patient).unwrap().hp,
            before_hp,
            "the patient rode a heal while eviction moved it at tick {tick}"
        );
        assert!(!report.events.iter().any(|event| matches!(
            event,
            Event::UnitRepaired {
                unit,
                source: UnitRepairSource::FieldWelder { unit: source },
                ..
            } if *unit == patient && *source == welder
        )));
    }
}

#[test]
fn a_patient_committing_its_deferred_found_rides_no_heal() {
    for tick in [0, 1] {
        let scenario = arena(vec![
            unit(0, UnitKind::Harvester, 4, 5), // welder
            unit(0, UnitKind::Harvester, 5, 5), // arrived founder and patient
        ]);
        let state = scenario.build().unwrap();
        let (welder, patient) = (state.units()[0].id, state.units()[1].id);
        let anchor = TilePos::new(6, 5);
        let mut json = serde_json::to_value(state).unwrap();
        json["tick"] = serde_json::json!(tick);
        json["units"][0]["order"] = serde_json::json!({"order": "repair_unit", "unit": patient});
        json["units"][0]["progress"] = serde_json::json!(weld_step_progress(UnitKind::Harvester));
        json["units"][1]["hp"] = serde_json::json!(UnitKind::Harvester.stats().max_hp - 10);
        json["units"][1]["order"] = serde_json::to_value(Order::Found {
            kind: BuildingKind::Turret,
            anchor,
        })
        .unwrap();
        let mut state: State = serde_json::from_value(json).unwrap();

        let before_hp = state.unit(patient).unwrap().hp;
        let report = state.tick(&[]);
        assert!(
            state
                .buildings()
                .iter()
                .any(|building| building.anchor == anchor),
            "the arrived founder must claim its promised ground at tick {tick}"
        );
        assert_eq!(
            state.unit(patient).unwrap().hp,
            before_hp,
            "the founder became newly weldable before its claim resolved at tick {tick}"
        );
        assert!(!report.events.iter().any(|event| matches!(
            event,
            Event::UnitRepaired {
                unit,
                source: UnitRepairSource::FieldWelder { unit: source },
                ..
            } if *unit == patient && *source == welder
        )));
    }
}

#[test]
fn fire_wins_the_tick_and_the_dead_forfeit_their_welds() {
    // A patient under enough fire dies mid-weld: buffered heals land
    // only on machines the volley left standing, so the torch never
    // outbids the guns on the tick they win — and the welder's job
    // simply ends.
    let mut units = cast();
    units.push(unit(1, UnitKind::Scuttler, 12, 7));
    let mut state = arena(units).build().unwrap();
    let (welder, patient, r1) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let r2 = state.units()[3].id;
    let hurt = wound(&mut state, patient, r1, 40);
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
    )]);
    // The weld is live before the guns come back.
    run_until(&mut state, 100, |s, _| s.unit(patient).unwrap().hp > hurt);
    // Two gnawing scuttlers out-pace one torch (20 hp/s vs 12).
    for raider in [r1, r2] {
        state.tick(&[cmd(
            1,
            Command::Move {
                units: vec![raider],
                goal: TilePos::new(5, 3),
                queue: false,
            },
        )]);
    }
    let events = run_until(&mut state, 2_000, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == patient))
    });
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == patient)),
        "the patient must fall despite an active welder"
    );
    assert!(state.unit(patient).is_none(), "nothing resurrects");
    // The welder learns the job is over and stands down (unless the
    // raiders turned on it — a corpse gives no orders either way).
    run_until(&mut state, 20, |s, _| {
        s.unit(welder).is_none_or(|u| u.order == Order::Idle)
    });
}

#[test]
fn reissued_welds_still_pay_for_the_torch_time() {
    // The no-op reissue rule: re-clicking the exact weld keeps the
    // billing meter, so a re-commanded welder never re-enters the
    // prepaid stretch and heals for free.
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    wound(&mut state, patient, raider, 45);
    let bank_before = state.player(PlayerId(0)).scrap;
    let mut healed = false;
    for _ in 0..150 {
        state.tick(&[cmd(
            0,
            Command::RepairUnit {
                units: vec![welder],
                target: patient,
                queue: false,
            },
        )]);
        for _ in 0..3 {
            state.tick(&[]);
        }
        if state.unit(patient).unwrap().hp == PATIENT_MAX {
            healed = true;
            break;
        }
    }
    assert!(healed, "the weld must finish under re-command");
    assert!(
        state.player(PlayerId(0)).scrap < bank_before,
        "and the torch must have been paid for ({} -> {})",
        bank_before,
        state.player(PlayerId(0)).scrap
    );
}

#[test]
fn a_queued_weld_waits_its_turn_then_welds() {
    let mut state = arena(cast()).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let hurt = wound(&mut state, patient, raider, 45);
    let waypoint = TilePos::new(10, 2);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![welder],
            goal: waypoint,
            queue: false,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: true,
        },
    )]);
    assert!(
        matches!(state.unit(welder).unwrap().order, Order::Move { .. }),
        "the march survives the shift-weld"
    );
    assert_eq!(state.unit(welder).unwrap().queue.len(), 1);
    run_until(&mut state, 400, |s, _| {
        s.unit(welder).unwrap().tile() == waypoint
    });
    run_until(&mut state, 2_000, |s, _| s.unit(patient).unwrap().hp > hurt);
}

#[test]
fn weld_refuses_the_healthy_the_foreign_the_flying_and_the_selfish() {
    let mut units = cast();
    // The own guard parks in the far corner, outside aggro of the
    // raider's whole corridor; the enemy pair sits across the map so
    // nothing acquires anything until asked.
    units.push(unit(0, UnitKind::Sentinel, 1, 7)); // a non-worker crew
    units.push(unit(0, UnitKind::Buzzard, 4, 7)); // the future air patient
    units.push(unit(1, UnitKind::Sentinel, 12, 5)); // the poke that wounds it
    units.push(unit(1, UnitKind::Sentinel, 13, 5)); // its second gun
    let mut state = arena(units).build().unwrap();
    let (welder, patient, raider) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let (guard, flyer) = (state.units()[3].id, state.units()[4].id);

    let rejected = |state: &mut State, command: Command, reason: RejectReason| {
        let report = state.tick(&[cmd(0, command)]);
        assert!(
            report.events.contains(&Event::CommandRejected {
                player: PlayerId(0),
                reason,
            }),
            "expected {reason:?}, got {:?}",
            report.events
        );
    };

    // Full health leaves nothing to do.
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![welder],
            target: patient,
            queue: false,
        },
        RejectReason::InvalidTarget,
    );
    // Foreign machines are not patients.
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![welder],
            target: raider,
            queue: false,
        },
        RejectReason::InvalidTarget,
    );
    // A combat unit holds no torch.
    let hurt = wound(&mut state, patient, raider, 45);
    assert!(hurt < PATIENT_MAX);
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![guard],
            target: patient,
            queue: false,
        },
        RejectReason::NoValidUnits,
    );
    // A wounded machine cannot weld itself: the patient drops out of
    // its own crew, and alone that leaves no valid units.
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![patient],
            target: patient,
            queue: false,
        },
        RejectReason::NoValidUnits,
    );
    // The air rule: the buzzard flies into the enemy pair's aggro and
    // their skyward pokes wound it; the wounded flyer still refuses
    // the ground torch — that patient waits for a facility that owns
    // the sky.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![flyer],
            goal: TilePos::new(10, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 2_000, |s, _| {
        s.unit(flyer)
            .is_none_or(|u| u.hp < UnitKind::Buzzard.stats().max_hp)
    });
    assert!(
        state.unit(flyer).is_some(),
        "test premise: the flyer survives its wounding"
    );
    // Pull the flyer home; validation cares only that it is wounded air.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![flyer],
            goal: TilePos::new(4, 7),
            queue: false,
        },
    )]);
    rejected(
        &mut state,
        Command::RepairUnit {
            units: vec![welder],
            target: flyer,
            queue: false,
        },
        RejectReason::InvalidTarget,
    );
}
