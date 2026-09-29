use super::*;

/// The arena with west units and east raiders placed as given.
fn raided(defenders: &[(UnitKind, i32, i32)], raiders: &[(UnitKind, i32, i32)]) -> Scenario {
    let mut scenario = arena(0);
    for (kind, x, y) in defenders {
        scenario.units.push(unit(0, *kind, *x, *y));
    }
    for (kind, x, y) in raiders {
        scenario.units.push(unit(1, *kind, *x, *y));
    }
    scenario
}

#[test]
fn a_ground_threat_draws_the_nearest_sufficient_defenders() {
    let scenario = raided(
        &[
            (UnitKind::Sentinel, 5, 8),
            (UnitKind::Sentinel, 6, 8),
            (UnitKind::Sentinel, 10, 1),
            (UnitKind::Sentinel, 11, 1),
        ],
        &[(UnitKind::Sentinel, 9, 5)],
    );
    let state = scenario.build().unwrap();
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let mut near = vec![at(&state, 5, 8), at(&state, 6, 8)];
    near.sort_unstable();
    assert_eq!(hunts(&commands), [(near, TilePos::new(9, 5))]);
    let missions = trace.unwrap().missions;
    assert!(
        matches!(
            missions.as_slice(),
            [MissionStatus {
                kind: MissionKind::Defend { .. },
                phase: Phase::Engage,
                units: 2,
                ..
            }]
        ),
        "{missions:?}"
    );
}

#[test]
fn an_air_raid_draws_only_units_that_hit_air_to_guard_the_foundry() {
    let scenario = raided(
        &[
            (UnitKind::Lancer, 5, 8),
            (UnitKind::Lancer, 6, 8),
            (UnitKind::Sentinel, 10, 1),
        ],
        &[(UnitKind::Darter, 9, 5)],
    );
    let state = scenario.build().unwrap();
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let [(units, goal)] = &hunts(&commands)[..] else {
        panic!("{commands:?}");
    };
    assert_eq!(units, &[at(&state, 10, 1)]);
    assert_eq!(
        crate::frame::gap(TilePos::new(3, 5), (2, 2), *goal, (1, 1)),
        0,
        "defenders wait beside the Foundry rather than under the flyer"
    );
}

#[test]
fn a_mixed_raid_draws_defenders_for_each_domain_it_comes_from() {
    let scenario = raided(
        &[
            (UnitKind::Lancer, 5, 8),
            (UnitKind::Lancer, 6, 8),
            (UnitKind::Lancer, 5, 9),
            (UnitKind::Sentinel, 10, 1),
        ],
        &[(UnitKind::Sentinel, 9, 5), (UnitKind::Darter, 9, 6)],
    );
    let state = scenario.build().unwrap();
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let [(units, _)] = &hunts(&commands)[..] else {
        panic!("{commands:?}");
    };
    assert!(
        units.contains(&at(&state, 10, 1)),
        "the only unit that hits air joins, however far"
    );
    let lancers = units
        .iter()
        .filter(|id| {
            state
                .units()
                .iter()
                .any(|unit| unit.id == **id && unit.kind == UnitKind::Lancer)
        })
        .count();
    assert_eq!(lancers, 2, "and only the Lancers the ground raid needs");
}

#[test]
fn a_recovering_defense_lends_its_units_to_another_foundry() {
    let mut scenario = raided(
        &[(UnitKind::Sentinel, 5, 8), (UnitKind::Sentinel, 6, 8)],
        &[(UnitKind::Scuttler, 13, 7)],
    );
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 11,
        y: 9,
    });
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, 12, &[]);
    let home = foundries(&state, PlayerId(0))[0];
    let mut defenders = vec![at(&state, 5, 8), at(&state, 6, 8)];
    defenders.sort_unstable();
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["missions"] = serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {"task": "defend", "asset": home.0, "phase": "recover"},
            "since": 12,
            "units": defenders,
            "goal": {"x": 9, "y": 5},
        }],
    });
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let [(units, _)] = &hunts(&commands)[..] else {
        panic!("{commands:?}");
    };
    assert_eq!(units.len(), 1);
    assert!(defenders.contains(&units[0]));
    let missions = trace.unwrap().missions;
    assert!(
        matches!(
            missions.as_slice(),
            [
                MissionStatus {
                    id: 0,
                    phase: Phase::Recover,
                    units: 1,
                    ..
                },
                MissionStatus {
                    id: 1,
                    phase: Phase::Engage,
                    units: 1,
                    ..
                },
            ]
        ),
        "{missions:?}"
    );
}

#[test]
fn defenders_are_released_after_the_threat_ends_and_return_with_it() {
    let scenario = raided(
        &[(UnitKind::Sentinel, 5, 8), (UnitKind::Sentinel, 6, 8)],
        &[(UnitKind::Scuttler, 9, 5)],
    );
    let mut state = scenario.build().unwrap();
    let raider = at(&state, 9, 5);
    let mut opponent = seat(&scenario, 0);
    let script = [
        (1, run(1, vec![raider], 21, 1)),
        (360, run(1, vec![raider], 9, 5)),
    ];
    let traces = play(&mut opponent, &mut state, 600, &script);

    let phases: Vec<(u64, Phase)> = traces
        .iter()
        .flat_map(|trace| {
            trace
                .missions
                .iter()
                .map(|mission| (mission.id, mission.phase))
        })
        .fold(Vec::new(), |mut seen, step| {
            if seen.last() != Some(&step) {
                seen.push(step);
            }
            seen
        });
    assert!(
        phases.starts_with(&[(0, Phase::Engage), (0, Phase::Recover), (1, Phase::Engage)]),
        "defense, recovery, release, then a new defense: {phases:?}"
    );
    let released = traces
        .iter()
        .position(|trace| trace.missions.is_empty() && trace.tick > 0)
        .expect("the first defense let its units go");
    assert!(
        traces[released..]
            .iter()
            .any(|trace| !trace.missions.is_empty())
    );
}

#[test]
fn a_threat_no_ground_reaches_is_left_to_production() {
    let mut scenario = raided(
        &[(UnitKind::Sentinel, 5, 8), (UnitKind::Sentinel, 6, 8)],
        &[(UnitKind::Sentinel, 10, 3)],
    );
    for y in 1..=5 {
        scenario.map[y].replace_range(8..13, if y == 3 { "##.##" } else { "#####" });
    }
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(
        observation
            .enemy_units
            .iter()
            .any(|enemy| enemy.tile == TilePos::new(10, 3)),
        "premise: the walled-in enemy is seen"
    );
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    assert!(hunts(&commands).is_empty(), "{commands:?}");
    assert!(trace.unwrap().missions.is_empty());
}

#[test]
fn a_moving_threat_is_chased_again_only_once_it_moves_away() {
    let staged = |x: i32, y: i32, tick: u64| {
        let scenario = raided(
            &[(UnitKind::Sentinel, 5, 8), (UnitKind::Sentinel, 6, 8)],
            &[(UnitKind::Scuttler, x, y)],
        );
        let mut state = scenario.build().unwrap();
        if tick > 0 {
            advance_to(&mut state, tick, &[]);
        }
        (scenario, state)
    };
    let (scenario, state) = staged(9, 4, 0);
    let mut opponent = seat(&scenario, 0);
    let goals = |opponent: &mut Opponent, state: &State| {
        hunts(&opponent.act(state, &mut OwnEvents::default()))
            .into_iter()
            .map(|(_, goal)| goal)
            .collect::<Vec<_>>()
    };
    assert_eq!(goals(&mut opponent, &state), [TilePos::new(9, 4)]);
    let (_, nudged) = staged(10, 3, 12);
    assert!(goals(&mut opponent, &nudged).is_empty());
    let (_, moved) = staged(12, 9, 24);
    assert_eq!(goals(&mut opponent, &moved), [TilePos::new(12, 9)]);
}

#[test]
fn defense_outranks_workers_under_a_small_allowance() {
    let mut scenario = raided(
        &[(UnitKind::Sentinel, 5, 9), (UnitKind::Sentinel, 6, 9)],
        &[(UnitKind::Scuttler, 9, 6)],
    );
    for row in [2, 8] {
        scenario.map[row].replace_range(2..3, "s");
    }
    scenario.units.extend((4..=5).map(|x| harvester(0, x, 7)));
    let state = scenario.build().unwrap();
    let scrapheap = BotConfig::opponent(BotDifficulty::Scrapheap, BotStance::Balanced, 11);
    let (commands, trace) =
        seat_with(&scenario, 0, scrapheap).act_traced(&state, &mut OwnEvents::default());
    assert_eq!(hunts(&commands).len(), 1);
    assert_eq!(harvests(&commands).len(), 2, "workers take what is left");
    assert_eq!(trace.unwrap().unit_orders, 3);
}

#[test]
fn a_short_defense_defers_the_saving_target_and_spends_protected_scrap() {
    let threatened = |scrap: u32| {
        let mut scenario = arena(scrap);
        scenario.units.extend([
            harvester(0, 5, 7),
            harvester(0, 4, 7),
            unit(1, UnitKind::Sentinel, 9, 5),
        ]);
        scenario
    };
    let scenario = threatened(400);
    let state = scenario.build().unwrap();
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    assert!(
        commands.iter().all(|command| !matches!(
            command.command,
            Command::Build { kind, .. } if kind != BuildingKind::Turret
        )),
        "the saving target waits; only an emergency Turret goes up: {commands:?}"
    );

    let scenario = threatened(100);
    let state = scenario.build().unwrap();
    let mut calm = scenario.clone();
    calm.units.pop();
    let (quiet, trace) = seat_with(&calm, 0, thrifty())
        .act_traced(&calm.build().unwrap(), &mut OwnEvents::default());
    assert!(trace.unwrap().protected > 0, "premise: scrap is protected");
    assert!(trains(&quiet).is_empty(), "premise");
    let turret = |commands: &[PlayerCommand]| {
        commands.iter().any(|command| {
            matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Turret,
                    ..
                }
            )
        })
    };
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    assert!(turret(&commands), "an emergency Turret first: {commands:?}");

    let scenario = threatened(200);
    let state = scenario.build().unwrap();
    let commands = seat_with(&scenario, 0, thrifty()).act(&state, &mut OwnEvents::default());
    let foundry = foundries(&state, PlayerId(0))[0];
    assert!(turret(&commands), "{commands:?}");
    assert_eq!(trains(&commands), [(foundry, UnitKind::Sentinel)]);
}

#[test]
fn mirrored_seats_defend_alike() {
    let mut scenario = arena(0);
    scenario.units.extend([
        unit(0, UnitKind::Sentinel, 5, 8),
        unit(0, UnitKind::Sentinel, 6, 8),
        unit(1, UnitKind::Sentinel, 18, 3),
        unit(1, UnitKind::Sentinel, 17, 3),
        unit(1, UnitKind::Scuttler, 9, 5),
        unit(0, UnitKind::Scuttler, 14, 6),
    ]);
    let state = scenario.build().unwrap();
    let west = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let east = seat(&scenario, 1).act(&state, &mut OwnEvents::default());
    assert_eq!(hunts(&west).len(), 1);
    assert_eq!(mirror(&state, west), east);
}

#[test]
fn a_mid_defense_checkpoint_resumes_identically() {
    let scenario = raided(
        &[(UnitKind::Sentinel, 5, 8), (UnitKind::Sentinel, 6, 8)],
        &[(UnitKind::Scuttler, 9, 5)],
    );
    let mut state = scenario.build().unwrap();
    let raider = at(&state, 9, 5);
    let mut opponent = seat(&scenario, 0);
    play(
        &mut opponent,
        &mut state,
        60,
        &[(1, run(1, vec![raider], 21, 1))],
    );
    assert!(!opponent.missions().is_empty(), "premise: mid-defense");

    let json = serde_json::to_string(&opponent.checkpoint()).unwrap();
    let checkpoint: Checkpoint = serde_json::from_str(&json).unwrap();
    let mut restored = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    while state.current_tick() < 300 {
        let commands = opponent.act(&state, &mut OwnEvents::default());
        assert_eq!(restored.act(&state, &mut OwnEvents::default()), commands);
        assert_eq!(restored.checkpoint(), opponent.checkpoint());
        state.tick(&commands);
    }
}

#[test]
fn checkpoints_reject_impossible_missions() {
    let scenario = raided(
        &[(UnitKind::Sentinel, 5, 8), (UnitKind::Sentinel, 6, 8)],
        &[(UnitKind::Sentinel, 9, 5)],
    );
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    assert_eq!(json["missions"]["list"].as_array().unwrap().len(), 1);
    let restore = |json: &serde_json::Value| {
        let checkpoint: Checkpoint = serde_json::from_value(json.clone()).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario))
    };
    assert!(restore(&json).is_ok());

    let mission = json["missions"]["list"][0].clone();
    let with = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut json = json.clone();
        edit(&mut json["missions"]);
        restore(&json).err().unwrap()
    };
    assert_eq!(
        with(&|missions| missions["list"][0]["since"] = 99.into()),
        "checkpoint mission could not have been recorded"
    );
    assert_eq!(
        with(&|missions| missions["list"][0]["goal"] = serde_json::json!({"x": 99, "y": 0})),
        "checkpoint mission could not have been recorded"
    );
    assert_eq!(
        with(&|missions| missions["next"] = 0.into()),
        "checkpoint mission ids are out of order"
    );
    assert_eq!(
        with(&|missions| missions["next"] = u64::MAX.into()),
        "checkpoint mission ids are out of order",
        "no id is left for the next mission"
    );
    assert_eq!(
        with(&|missions| {
            let units = missions["list"][0]["units"].as_array_mut().unwrap();
            units.reverse();
        }),
        "checkpoint mission units are malformed"
    );
    assert_eq!(
        with(&|missions| missions["list"][0]["units"] = serde_json::json!([])),
        "checkpoint mission units are malformed"
    );
    assert_eq!(
        with(&|missions| {
            let mut copy = mission.clone();
            copy["id"] = 1.into();
            missions["list"].as_array_mut().unwrap().push(copy);
            missions["next"] = 2.into();
        }),
        "checkpoint missions share a unit"
    );
    assert_eq!(
        with(&|missions| {
            let list: Vec<serde_json::Value> = (0..17)
                .map(|id| {
                    let mut copy = mission.clone();
                    copy["id"] = id.into();
                    copy
                })
                .collect();
            missions["list"] = list.into();
            missions["next"] = 17.into();
        }),
        "checkpoint holds too many missions"
    );
}

#[test]
fn an_exhausted_mission_counter_is_rejected_at_any_tick() {
    let missions: crate::missions::Missions = serde_json::from_value(serde_json::json!({
        "next": u64::MAX,
        "list": [],
        "waiting": null,
    }))
    .unwrap();
    assert_eq!(
        missions.validate(u64::MAX / 2, 48, 24, 1).err().unwrap(),
        "checkpoint mission ids are out of order"
    );
}
