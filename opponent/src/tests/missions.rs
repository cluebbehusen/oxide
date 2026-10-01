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
        scenario.units.extend(workforce(0));
        // A Turret out on the field: the seat has one, but it guards nothing
        // near home.
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 13,
            y: 9,
        });
        scenario.units.push(unit(1, UnitKind::Sentinel, 9, 5));
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
            let over = crate::missions::MISSION_CAP + 1;
            let list: Vec<serde_json::Value> = (0..over)
                .map(|id| {
                    let mut copy = mission.clone();
                    copy["id"] = id.into();
                    copy
                })
                .collect();
            missions["list"] = list.into();
            missions["next"] = over.into();
        }),
        "checkpoint holds too many missions"
    );
}

#[test]
fn a_mission_as_large_as_a_late_army_restores() {
    let units: Vec<u32> = (1..=600).collect();
    let missions: crate::missions::Missions = serde_json::from_value(serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {
                "task": "attack",
                "target": {"owner": 1, "building": "foundry", "anchor": {"x": 19, "y": 5}},
                "phase": "travel",
            },
            "since": 0,
            "units": units,
            "goal": {"x": 10, "y": 5},
        }],
        "waiting": null,
    }))
    .unwrap();
    assert_eq!(missions.validate(100, 24, 12, 0), Ok(()));
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

/// Where a seat's picket stands east of its Foundry, where the enemy Bombard
/// shelling it stands, beyond the seat's sight but in range, and where the
/// enemy Kestrel spotting for it hovers, clear of the seat's guns; West's
/// spots, turned half about the field for East.
const PICKET: (i32, i32) = (8, 12);
const GUN: (i32, i32) = (17, 12);
const SPOTTER: (i32, i32) = (12, 5);

fn turned(player: u8, (x, y): (i32, i32)) -> (i32, i32) {
    if player == 0 {
        (x, y)
    } else {
        (47 - x, 23 - y)
    }
}

/// Orders each enemy Bombard of `shelled` at that seat's picket.
fn open_fire(state: &mut State, shelled: &[u8]) {
    let orders: Vec<PlayerCommand> = shelled
        .iter()
        .map(|&player| {
            let at = |spot| {
                let (x, y) = turned(player, spot);
                at(state, x, y)
            };
            PlayerCommand {
                player: PlayerId(1 - player),
                command: Command::Attack {
                    units: vec![at(GUN)],
                    target: AttackTarget::Unit(at(PICKET)),
                    queue: false,
                },
            }
        })
        .collect();
    state.tick(&orders);
}

/// Plays `state` to a decision tick on which every seat of `shelled` sees a
/// shell coming.
fn until_shells_come(scenario: &Scenario, state: &mut State, shelled: &[u8]) {
    let seat = seat(scenario, 0);
    let coming = |state: &State, player: u8| {
        !ObservationData::fog_honest(state, PlayerId(player))
            .incoming_shells
            .is_empty()
    };
    while !seat.decision_due(state) || !shelled.iter().all(|player| coming(state, *player)) {
        assert!(state.current_tick() < 600, "premise: the Bombards fire");
        state.tick(&[]);
    }
}

/// The field with each of `shelled` holding its garrison and picket, and the
/// other seat a Bombard and a Kestrel for that picket.
fn shelling(shelled: &[u8]) -> Scenario {
    let mut scenario = field();
    for &player in shelled {
        for spot in GARRISON.into_iter().chain([PICKET]) {
            let (x, y) = turned(player, spot);
            scenario.units.push(unit(player, UnitKind::Sentinel, x, y));
        }
    }
    for &player in shelled {
        for (kind, spot) in [(UnitKind::Bombard, GUN), (UnitKind::Kestrel, SPOTTER)] {
            let (x, y) = turned(player, spot);
            scenario.units.push(unit(1 - player, kind, x, y));
        }
    }
    scenario
}

/// `shelling(shelled)` played until the shells come.
fn shelled(shelled: &[u8]) -> (Scenario, State) {
    let scenario = shelling(shelled);
    let mut state = scenario.build().unwrap();
    open_fire(&mut state, shelled);
    until_shells_come(&scenario, &mut state, shelled);
    (scenario, state)
}

#[test]
fn shells_from_a_gun_out_of_sight_send_fighters_toward_it() {
    let (scenario, state) = shelled(&[0]);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(
        observation
            .enemy_units
            .iter()
            .all(|enemy| enemy.kind != UnitKind::Bombard),
        "premise: the gun is out of sight"
    );
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let [(units, goal)] = &hunts(&commands)[..] else {
        panic!("{commands:?}");
    };
    assert!(!units.is_empty());
    let (picket, gun) = (TilePos::new(PICKET.0, PICKET.1), TilePos::new(GUN.0, GUN.1));
    assert!(
        goal.x > picket.x && goal.chebyshev(picket) >= goal.chebyshev(gun),
        "toward the gun, not the shelled picket: {goal:?}"
    );
    let reach = UnitKind::ALL
        .iter()
        .flat_map(|kind| kind.stats().weapons.iter())
        .filter(|weapon| weapon.indirect)
        .map(|weapon| weapon.range.ceil().to_num::<i32>())
        .max()
        .unwrap();
    assert!(
        goal.chebyshev(picket) <= reach,
        "within artillery reach: {goal:?}"
    );
    assert!(
        trace
            .unwrap()
            .missions
            .iter()
            .any(|mission| matches!(mission.kind, MissionKind::Defend { .. })
                && mission.phase == Phase::Engage),
        "a defense answers it"
    );
}

/// The field with West's garrison and picket, an East Bastion shelling the
/// picket with a Kestrel spotting for it, East Turrets around the Bastion
/// when `guarded`, and a West Kestrel that has seen them all; played until
/// the shells come.
fn bastion(guarded: bool) -> (Scenario, State) {
    let mut scenario = field();
    for spot in GARRISON.into_iter().chain([PICKET]) {
        scenario
            .units
            .push(unit(0, UnitKind::Sentinel, spot.0, spot.1));
    }
    scenario
        .units
        .push(unit(1, UnitKind::Kestrel, SPOTTER.0, SPOTTER.1));
    scenario.units.push(unit(0, UnitKind::Kestrel, 15, 17));
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Bastion,
        x: 16,
        y: 11,
    });
    if guarded {
        for (x, y) in [(19, 10), (19, 13), (19, 11)] {
            scenario.buildings.push(BuildingSpec {
                player: 1,
                kind: BuildingKind::Turret,
                x,
                y,
            });
        }
    }
    let mut state = scenario.build().unwrap();
    until_shells_come(&scenario, &mut state, &[0]);
    (scenario, state)
}

/// Whether `commands` send units beside the staged Bastion.
fn sent_beside_the_bastion(commands: &[PlayerCommand]) -> bool {
    let bastion = TilePos::new(16, 11);
    hunts(commands)
        .iter()
        .any(|(_, goal)| crate::frame::gap(bastion, (2, 2), *goal, (1, 1)) <= 1)
}

#[test]
fn a_lone_bastion_shelling_the_base_is_answered_beside_it() {
    let (scenario, state) = bastion(false);
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert!(sent_beside_the_bastion(&commands), "{commands:?}");
}

#[test]
fn a_bastion_among_guns_the_garrison_cannot_beat_is_left_to_production() {
    let (scenario, state) = bastion(true);
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert!(!sent_beside_the_bastion(&commands), "{commands:?}");
}

#[test]
fn an_upgraded_building_is_worth_every_tier_it_paid_for() {
    let mut scenario = field();
    scenario.units.push(unit(0, UnitKind::Kestrel, 20, 11));
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Turret,
        x: 22,
        y: 11,
    });
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let mut turret = observation.enemy_buildings[0].clone();
    assert_eq!(crate::missions::building_value(&turret), 100);
    turret.tier = 2;
    turret.hp = 900;
    let paid = [100, 150, 300].iter().sum::<u64>();
    assert_eq!(crate::missions::building_value(&turret), paid);
    turret.hp = 450;
    assert_eq!(crate::missions::building_value(&turret), paid / 2);
}

#[test]
fn a_gun_in_sight_shelling_the_base_is_defended_against_where_it_stands() {
    let mut scenario = shelling(&[0]);
    // A West Scuttler beside the gun reveals it. The gun stands beyond the
    // reach that makes a unit a threat to buildings, but its shells land in
    // the base.
    scenario
        .units
        .push(unit(0, UnitKind::Scuttler, GUN.0 + 4, GUN.1));
    let mut state = scenario.build().unwrap();
    open_fire(&mut state, &[0]);
    until_shells_come(&scenario, &mut state, &[0]);
    let gun = at(&state, GUN.0, GUN.1);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    let seen = observation
        .enemy_units
        .iter()
        .find(|enemy| enemy.id == gun)
        .expect("premise: the gun is in sight");
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert!(
        hunts(&commands).iter().any(|(_, goal)| *goal == seen.tile),
        "{commands:?}"
    );
}

#[test]
fn mirrored_seats_under_mirrored_shelling_answer_alike() {
    let (scenario, state) = shelled(&[0, 1]);
    let west = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let east = seat(&scenario, 1).act(&state, &mut OwnEvents::default());
    assert!(!hunts(&west).is_empty(), "premise: {west:?}");
    assert_eq!(mirror(&state, west), east);
}

#[test]
fn a_checkpoint_while_answering_unseen_shelling_resumes_identically() {
    let (scenario, mut state) = shelled(&[0]);
    let mut opponent = seat(&scenario, 0);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert!(!hunts(&commands).is_empty(), "premise: {commands:?}");
    state.tick(&commands);
    let json = serde_json::to_string(&opponent.checkpoint()).unwrap();
    let checkpoint: Checkpoint = serde_json::from_str(&json).unwrap();
    let mut restored = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let until = state.current_tick() + 360;
    while state.current_tick() < until {
        let commands = opponent.act(&state, &mut OwnEvents::default());
        assert_eq!(restored.act(&state, &mut OwnEvents::default()), commands);
        assert_eq!(restored.checkpoint(), opponent.checkpoint());
        state.tick(&commands);
    }
}

#[test]
fn a_defense_re_sends_only_members_that_found_a_route() {
    let staged = |x: i32, y: i32, tick: u64| {
        let scenario = raided(
            &[(UnitKind::Sentinel, 5, 8), (UnitKind::Sentinel, 6, 8)],
            &[(UnitKind::Warden, x, y)],
        );
        let mut state = scenario.build().unwrap();
        if tick > 0 {
            advance_to(&mut state, tick, &[]);
        }
        (scenario, state)
    };
    let (scenario, state) = staged(9, 4, 0);
    let (stuck, free) = (at(&state, 5, 8), at(&state, 6, 8));
    let mut opponent = seat(&scenario, 0);
    let first = hunts(&opponent.act(&state, &mut OwnEvents::default()));
    assert_eq!(first, [(vec![stuck, free], TilePos::new(9, 4))], "premise");
    let (_, moved) = staged(12, 9, 24);
    let mut events = OwnEvents::default();
    events.record(
        PlayerId(0),
        &[oxide_sim::Event::OrderStalled {
            unit: stuck,
            player: PlayerId(0),
            pos: moved.unit(stuck).unwrap().pos,
            reason: oxide_sim::StallReason::NoRoute,
        }],
    );
    assert_eq!(
        hunts(&opponent.act(&moved, &mut events)),
        [(vec![free], TilePos::new(12, 9))]
    );
}

#[test]
fn a_gun_out_of_sight_waits_while_raiders_in_sight_hold_the_foundry() {
    let mut scenario = shelling(&[0]);
    scenario.units.push(unit(1, UnitKind::Buzzard, 4, 5));
    let mut state = scenario.build().unwrap();
    open_fire(&mut state, &[0]);
    until_shells_come(&scenario, &mut state, &[0]);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(
        observation
            .enemy_units
            .iter()
            .any(|enemy| enemy.kind == UnitKind::Buzzard)
            && observation
                .enemy_units
                .iter()
                .all(|enemy| enemy.kind != UnitKind::Bombard),
        "premise: the raider is in sight and the gun is not"
    );
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let [(_, goal)] = &hunts(&commands)[..] else {
        panic!("{commands:?}");
    };
    let foundry = TilePos::new(3, 11);
    assert!(
        crate::frame::gap(foundry, (3, 3), *goal, (1, 1)) <= 1,
        "defenders wait beside the Foundry for the raid: {goal:?}"
    );
}

/// The field with each of `shelled` holding its garrison and picket, the other
/// seat a Bastion in range of that picket and a Kestrel spotting for it, and,
/// when `watched`, a Kestrel of the shelled seat over that Bastion.
fn bastions(shelled: &[u8], watched: bool) -> Scenario {
    let mut scenario = field();
    for &player in shelled {
        for spot in GARRISON.into_iter().chain([PICKET]) {
            let (x, y) = turned(player, spot);
            scenario.units.push(unit(player, UnitKind::Sentinel, x, y));
        }
    }
    for &player in shelled {
        let (x, y) = turned(player, SPOTTER);
        scenario
            .units
            .push(unit(1 - player, UnitKind::Kestrel, x, y));
    }
    for &player in shelled.iter().filter(|_| watched) {
        let (x, y) = turned(player, (16, 9));
        scenario.units.push(unit(player, UnitKind::Kestrel, x, y));
    }
    for &player in shelled {
        // A 2x2 footprint turns about its far corner.
        let (x, y) = turned(player, (16, 11));
        let (x, y) = if player == 0 { (x, y) } else { (x - 1, y - 1) };
        scenario.buildings.push(BuildingSpec {
            player: 1 - player,
            kind: BuildingKind::Bastion,
            x,
            y,
        });
    }
    scenario
}

/// Plays `state` to a decision tick on which West sees a shell coming and,
/// when `away`, no longer sees the Bastion, after Running West's Kestrel home
/// on the first tick.
fn until_bastion_fires(scenario: &Scenario, state: &mut State, away: bool) {
    if away {
        let kestrels: Vec<UnitId> = state
            .units()
            .iter()
            .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Kestrel)
            .map(|unit| unit.id)
            .collect();
        state.tick(&[PlayerCommand {
            player: PlayerId(0),
            command: Command::Run {
                units: kestrels,
                goal: TilePos::new(3, 3),
                queue: false,
            },
        }]);
    }
    let seat = seat(scenario, 0);
    let ready = |state: &State| {
        let observation = ObservationData::fog_honest(state, PlayerId(0));
        !observation.incoming_shells.is_empty()
            && observation
                .enemy_buildings
                .iter()
                .any(|building| building.kind == BuildingKind::Bastion && building.seen != away)
    };
    while !seat.decision_due(state) || !ready(state) {
        assert!(state.current_tick() < 1_200, "premise: the Bastion fires");
        state.tick(&[]);
    }
}

/// Asserts West's one decision on `state` answers the Bastion's shells with a
/// defense hunting beside it.
fn defended_beside_the_bastion(scenario: &Scenario, state: &State, seen: bool) {
    let observation = ObservationData::fog_honest(state, PlayerId(0));
    let known = observation
        .enemy_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Bastion)
        .expect("premise: the Bastion is known");
    assert_eq!(known.seen, seen, "premise: in sight or remembered");
    assert!(
        observation
            .enemy_units
            .iter()
            .all(|enemy| enemy.kind.stats().weapons.is_empty()),
        "premise: no armed enemy unit in sight"
    );
    let (commands, trace) = seat(scenario, 0).act_traced(state, &mut OwnEvents::default());
    let [(units, goal)] = &hunts(&commands)[..] else {
        panic!("{commands:?}");
    };
    assert!(!units.is_empty());
    assert_eq!(
        crate::frame::gap(known.anchor, (2, 2), *goal, (1, 1)),
        0,
        "beside the Bastion, not toward a guess: {goal:?}"
    );
    assert!(
        trace
            .unwrap()
            .missions
            .iter()
            .any(|mission| matches!(mission.kind, MissionKind::Defend { .. })
                && mission.phase == Phase::Engage),
        "a defense answers it"
    );
}

#[test]
fn shells_from_a_bastion_in_sight_send_fighters_beside_it() {
    let scenario = bastions(&[0], true);
    let mut state = scenario.build().unwrap();
    until_bastion_fires(&scenario, &mut state, false);
    defended_beside_the_bastion(&scenario, &state, true);
}

#[test]
fn shells_from_a_remembered_bastion_send_fighters_beside_it() {
    let mut scenario = bastions(&[0], true);
    // A Barricade takes the picket's place: it neither sees the Bastion nor
    // walks out after it, so the Bastion stays remembered once the Kestrel
    // that saw it leaves.
    scenario
        .units
        .retain(|unit| !(unit.player == 0 && (unit.x, unit.y) == PICKET));
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Barricade,
        x: PICKET.0,
        y: PICKET.1,
    });
    let mut state = scenario.build().unwrap();
    until_bastion_fires(&scenario, &mut state, true);
    defended_beside_the_bastion(&scenario, &state, false);
}

#[test]
fn mirrored_seats_shelled_by_mirrored_bastions_answer_alike() {
    let scenario = bastions(&[0, 1], true);
    let mut state = scenario.build().unwrap();
    let both = |state: &State| {
        [0, 1].iter().all(|player| {
            !ObservationData::fog_honest(state, PlayerId(*player))
                .incoming_shells
                .is_empty()
        })
    };
    let seat0 = seat(&scenario, 0);
    while !seat0.decision_due(&state) || !both(&state) {
        assert!(state.current_tick() < 600, "premise: both Bastions fire");
        state.tick(&[]);
    }
    let west = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let east = seat(&scenario, 1).act(&state, &mut OwnEvents::default());
    assert!(!hunts(&west).is_empty(), "premise: {west:?}");
    assert_eq!(mirror(&state, west), east);
}
