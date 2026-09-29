use super::*;

/// West army spots near home.
const WEST: [(i32, i32); 8] = [
    (6, 9),
    (7, 9),
    (8, 9),
    (9, 9),
    (6, 10),
    (7, 10),
    (8, 10),
    (9, 10),
];

/// The field with `west` West Sentinels and the given East units.
fn armed(west: usize, east: &[(UnitKind, i32, i32)]) -> Scenario {
    let mut scenario = field();
    for (x, y) in &WEST[..west] {
        scenario.units.push(unit(0, UnitKind::Sentinel, *x, *y));
    }
    for (kind, x, y) in east {
        scenario.units.push(unit(1, *kind, *x, *y));
    }
    scenario
}

fn east_start() -> MissionKind {
    MissionKind::Attack {
        owner: PlayerId(1),
        building: BuildingKind::Foundry,
        anchor: TilePos::new(43, 11),
    }
}

fn attack(missions: &[MissionStatus]) -> Option<MissionStatus> {
    missions
        .iter()
        .copied()
        .find(|mission| matches!(mission.kind, MissionKind::Attack { .. }))
}

/// Attack phases in the order the traces show them.
fn phases(traces: &[Trace]) -> Vec<Phase> {
    let mut seen = Vec::new();
    for trace in traces {
        if let Some(mission) = attack(&trace.missions)
            && seen.last() != Some(&mission.phase)
        {
            seen.push(mission.phase);
        }
    }
    seen
}

#[test]
fn an_opportunity_above_the_stance_minimum_launches() {
    let scenario = armed(8, &[]);
    let state = scenario.build().unwrap();
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let mission = attack(&trace.unwrap().missions).expect("an attack forms");
    assert_eq!(mission.kind, east_start(), "no enemy building is known yet");
    assert_eq!(mission.phase, Phase::Gather);
    let sent = hunts(&commands);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1, mission.goal, "the army gathers at its rally");
    assert_eq!(sent[0].0.len(), mission.units as usize);
    assert!(sent[0].0.len() < WEST.len(), "only the force it needs");
}

#[test]
fn an_army_below_the_stance_minimum_stays_home() {
    let scenario = armed(6, &[]);
    let state = scenario.build().unwrap();
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    assert!(hunts(&commands).is_empty());
    assert!(trace.unwrap().missions.is_empty());
}

#[test]
fn known_defenses_hold_an_attack_until_waiting_lowers_the_margin() {
    let scenario = armed(8, &[]);
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, 9_600, &[]);
    let now = state.current_tick();
    let staged = |waiting: Option<u64>| {
        let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
        json["memory"]["units"] = (0..5)
            .map(|index: u32| {
                serde_json::json!({
                    "id": 1_000 + index,
                    "kind": "sentinel",
                    "tile": {"x": 41, "y": 10 + index},
                    "seen": now,
                })
            })
            .collect();
        json["missions"]["waiting"] = serde_json::json!(waiting);
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap()
    };
    let launches = |mut opponent: Opponent| {
        let trace = opponent.act_traced(&state, &mut OwnEvents::default()).1;
        attack(&trace.unwrap().missions).is_some()
    };
    assert!(
        !launches(staged(None)),
        "remembered defenders ask for twice their value"
    );
    assert!(
        launches(staged(Some(0))),
        "a long wait brings the margin down to even"
    );
}

#[test]
fn a_losing_attack_withdraws_to_its_rally() {
    let east: Vec<(UnitKind, i32, i32)> = (8..=15)
        .flat_map(|y| [(UnitKind::Sentinel, 40, y), (UnitKind::Sentinel, 41, y)])
        .collect();
    let scenario = armed(8, &east);
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    let traces = play(&mut opponent, &mut state, 1_200, &[]);
    let seen = phases(&traces);
    let withdrew = seen
        .iter()
        .position(|phase| *phase == Phase::Withdraw)
        .unwrap_or_else(|| panic!("{seen:?}"));
    assert!(seen[..withdrew].contains(&Phase::Travel), "{seen:?}");
    let rally = traces
        .iter()
        .find_map(|trace| attack(&trace.missions).filter(|mission| mission.phase == Phase::Gather))
        .unwrap()
        .goal;
    let withdrawal = traces
        .iter()
        .find_map(|trace| {
            attack(&trace.missions).filter(|mission| mission.phase == Phase::Withdraw)
        })
        .unwrap();
    assert_eq!(withdrawal.goal, rally);
}

#[test]
fn wounded_units_stay_out_and_wounded_members_leave_between_fights() {
    let mut scenario = armed(8, &[]);
    scenario.units.push(unit(0, UnitKind::Sentinel, 10, 11));
    let state = scenario.build().unwrap();
    let hurt = at(&state, 10, 11);
    let state = wounded(&state, hurt, 25);
    let mut opponent = seat(&scenario, 0);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    let sent = hunts(&commands);
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].0.contains(&hurt), "under half health never joins");

    let mut state = state;
    advance_to(&mut state, 12, &commands);
    let member = sent[0].0[0];
    let state = wounded(&state, member, 15);
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let rally = sent[0].1;
    assert_eq!(runs(&commands), [(vec![member], rally)]);
    assert!(attack(&trace.unwrap().missions).is_some());
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let members = json["missions"]["list"][0]["units"].as_array().unwrap();
    assert!(!members.contains(&member.0.into()), "{members:?}");
}

#[test]
fn a_stalled_attack_gives_up_its_target_for_a_while() {
    let scenario = armed(8, &[]);
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    let launch = opponent.act(&state, &mut OwnEvents::default());
    let (members, rally) = hunts(&launch).remove(0);
    let mut json = serde_json::to_value(opponent.checkpoint()).unwrap();
    json["missions"]["list"][0]["task"]["phase"] = "travel".into();

    let mut state = state;
    advance_to(&mut state, 12, &[]);
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert_eq!(
        runs(&commands),
        [(members, rally)],
        "idle short of the target, the army returns"
    );
    let mission = attack(&trace.unwrap().missions).unwrap();
    assert_eq!(mission.phase, Phase::Recover);

    advance_to(&mut state, 36, &commands);
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(
        trace.unwrap().missions.is_empty(),
        "with its only target given up, the attack disbands"
    );
    assert!(hunts(&commands).is_empty(), "and no new one forms");
}

#[test]
fn missions_live_their_lifecycle() {
    let mut scenario = armed(8, &[]);
    scenario.units.extend([
        unit(1, UnitKind::Scuttler, 10, 11),
        unit(1, UnitKind::Scuttler, 10, 12),
        unit(1, UnitKind::Sentinel, 3, 22),
        unit(1, UnitKind::Sentinel, 4, 22),
    ]);
    let mut state = scenario.build().unwrap();
    let scuttlers = vec![at(&state, 10, 11), at(&state, 10, 12)];
    let raiders = vec![at(&state, 3, 22), at(&state, 4, 22)];
    let mut opponent = seat(&scenario, 0);
    let mut traces: Vec<Trace> = Vec::new();
    let mut raided = None;
    while state.current_tick() < 2_400 {
        let now = state.current_tick();
        let (mut commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
        if now == 1 {
            commands.push(run(1, scuttlers.clone(), 30, 22));
        }
        if let Some(trace) = trace {
            let travelling =
                attack(&trace.missions).is_some_and(|mission| mission.phase == Phase::Travel);
            if travelling && raided.is_none() {
                raided = Some(now);
                commands.push(run(1, raiders.clone(), 6, 11));
            }
            traces.push(trace);
        }
        state.tick(&commands);
    }

    let defends = |trace: &Trace| {
        trace
            .missions
            .iter()
            .filter(|mission| matches!(mission.kind, MissionKind::Defend { .. }))
            .copied()
            .collect::<Vec<_>>()
    };
    assert!(
        !defends(&traces[0]).is_empty(),
        "the raid on home is defended first"
    );
    assert!(
        attack(&traces[0].missions).is_none(),
        "leaving too little to attack"
    );
    let released = traces
        .iter()
        .position(|trace| defends(trace).is_empty())
        .expect("the defense lets its units go once the raider leaves");
    let launched = traces
        .iter()
        .position(|trace| attack(&trace.missions).is_some())
        .expect("the freed army attacks");
    assert!(released <= launched);
    assert!(raided.is_some(), "the attack set out");

    let answered = traces
        .iter()
        .position(|trace| trace.tick > raided.unwrap() && !defends(trace).is_empty())
        .expect("the second raid is defended");
    let before = attack(&traces[answered - 1].missions).unwrap();
    assert_eq!(before.phase, Phase::Travel);
    let after = attack(&traces[answered].missions).map_or(0, |mission| mission.units);
    assert!(
        after < before.units,
        "the travelling attack gave units to the defense"
    );
}

#[test]
fn mirrored_seats_attack_alike() {
    let mut scenario = armed(8, &[]);
    let (width, height) = (48, 24);
    for (x, y) in WEST {
        scenario
            .units
            .push(unit(1, UnitKind::Sentinel, width - 1 - x, height - 1 - y));
    }
    let state = scenario.build().unwrap();
    let west = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let east = seat(&scenario, 1).act(&state, &mut OwnEvents::default());
    assert_eq!(hunts(&west).len(), 1);
    assert_eq!(mirror(&state, west), east);
}

#[test]
fn a_mid_attack_checkpoint_resumes_identically() {
    let scenario = armed(8, &[]);
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    let traces = play(&mut opponent, &mut state, 240, &[]);
    assert!(
        phases(&traces).contains(&Phase::Travel),
        "premise: under way"
    );

    let json = serde_json::to_string(&opponent.checkpoint()).unwrap();
    let checkpoint: Checkpoint = serde_json::from_str(&json).unwrap();
    let mut restored = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    while state.current_tick() < 600 {
        let commands = opponent.act(&state, &mut OwnEvents::default());
        assert_eq!(restored.act(&state, &mut OwnEvents::default()), commands);
        assert_eq!(restored.checkpoint(), opponent.checkpoint());
        state.tick(&commands);
    }
}

#[test]
fn checkpoints_reject_impossible_attacks() {
    let scenario = armed(8, &[]);
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let rejected = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut json = json.clone();
        edit(&mut json["missions"]);
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario))
            .err()
            .unwrap()
    };
    assert_eq!(
        rejected(&|missions| missions["waiting"] = 5.into()),
        "checkpoint mission could not have been recorded"
    );
    assert_eq!(
        rejected(&|missions| {
            missions["list"][0]["task"]["target"]["anchor"] =
                serde_json::json!({"x": i32::MIN, "y": 0});
        }),
        "checkpoint mission could not have been recorded",
        "a target off the map"
    );
    assert_eq!(
        rejected(&|missions| {
            let mut second = missions["list"][0].clone();
            second["id"] = 1.into();
            second["units"] = serde_json::json!([9_999]);
            missions["list"].as_array_mut().unwrap().push(second);
            missions["next"] = 2.into();
        }),
        "checkpoint mission could not have been recorded"
    );
    let mut defend_gathering = json.clone();
    defend_gathering["missions"]["list"][0]["task"] =
        serde_json::json!({"task": "defend", "asset": 0, "phase": "gather"});
    assert!(
        serde_json::from_value::<Checkpoint>(defend_gathering).is_err(),
        "a defense has no gather phase"
    );
}

/// A restored West seat at tick 12 on `scenario`, with `missions` staged.
fn staged(scenario: &Scenario, missions: serde_json::Value) -> (State, Opponent) {
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, 12, &[]);
    let mut json = serde_json::to_value(seat(scenario, 0).checkpoint()).unwrap();
    json["missions"] = missions;
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let opponent = Opponent::restore(&checkpoint, scenario, &state, map(scenario)).unwrap();
    (state, opponent)
}

#[test]
fn anti_air_alone_never_counts_as_an_attack() {
    let mut scenario = field();
    for (x, y) in WEST {
        scenario.units.push(unit(0, UnitKind::Flakhound, x, y));
    }
    let state = scenario.build().unwrap();
    let (_, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    assert!(attack(&trace.unwrap().missions).is_none());
}

#[test]
fn a_failed_placement_does_not_spare_the_building_there() {
    let scenario = armed(8, &[]);
    let state = scenario.build().unwrap();
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["memory"]["failures"] = serde_json::json!([
        {"kind": "foundry", "anchor": {"x": 43, "y": 11}, "at": 0}
    ]);
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let (_, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert_eq!(
        attack(&trace.unwrap().missions).map(|mission| mission.kind),
        Some(east_start())
    );
}

#[test]
fn the_mission_cap_holds_back_an_attack() {
    let mut scenario = armed(8, &[]);
    for index in 0..16 {
        scenario
            .units
            .push(unit(0, UnitKind::Sentinel, 6 + index % 8, 14 + index / 8));
    }
    let state = scenario.build().unwrap();
    let home = foundries(&state, PlayerId(0))[0];
    let list: Vec<serde_json::Value> = (0..16)
        .map(|index: i32| {
            let id = at(&state, 6 + index % 8, 14 + index / 8);
            serde_json::json!({
                "id": index,
                "task": {"task": "defend", "asset": home.0, "phase": {"engage": {"focus": null}}},
                "since": 0,
                "units": [id],
                "goal": {"x": 6, "y": 11},
            })
        })
        .collect();
    let (state, mut opponent) = staged(&scenario, serde_json::json!({"next": 16, "list": list}));
    let (_, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let missions = trace.unwrap().missions;
    assert_eq!(missions.len(), 16);
    assert!(attack(&missions).is_none());
    let checkpoint = opponent.checkpoint();
    assert!(Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).is_ok());
}

#[test]
fn a_travelling_attack_in_contact_keeps_its_units_from_a_defense() {
    let mut scenario = field();
    for (x, y) in [
        (20, 11),
        (21, 11),
        (20, 12),
        (21, 12),
        (22, 11),
        (22, 12),
        (23, 11),
    ] {
        scenario.units.push(unit(0, UnitKind::Sentinel, x, y));
    }
    scenario.units.extend([
        unit(1, UnitKind::Sentinel, 28, 11),
        unit(1, UnitKind::Sentinel, 8, 11),
    ]);
    let state = scenario.build().unwrap();
    let mut members: Vec<UnitId> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Sentinel)
        .map(|unit| unit.id)
        .collect();
    members.sort_unstable();
    let missions = serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {
                "task": "attack",
                "target": {"owner": 1, "building": "foundry", "anchor": {"x": 43, "y": 11}},
                "phase": "travel",
            },
            "since": 0,
            "units": members,
            "goal": {"x": 42, "y": 11},
        }],
    });
    let (state, mut opponent) = staged(&scenario, missions);
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(
        hunts(&commands)
            .iter()
            .all(|(units, _)| units.iter().all(|id| !members.contains(id))
                || units.len() == members.len())
    );
    let mission = attack(&trace.unwrap().missions).unwrap();
    assert_eq!(
        mission.units as usize,
        members.len(),
        "no fighting attacker was taken"
    );
}

#[test]
fn reinforcements_gather_before_the_attack_sets_out() {
    let scenario = armed(8, &[]);
    let state = scenario.build().unwrap();
    let mut gathered: Vec<UnitId> = WEST[..6].iter().map(|(x, y)| at(&state, *x, *y)).collect();
    gathered.sort_unstable();
    let missions = serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {
                "task": "attack",
                "target": {"owner": 1, "building": "foundry", "anchor": {"x": 43, "y": 11}},
                "phase": "gather",
            },
            "since": 0,
            "units": gathered,
            "goal": {"x": 11, "y": 11},
        }],
    });
    let (state, mut opponent) = staged(&scenario, missions);
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let sent = hunts(&commands);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(
        sent[0].0.iter().all(|id| !gathered.contains(id)),
        "only the recruits move"
    );
    assert_eq!(
        attack(&trace.unwrap().missions).unwrap().phase,
        Phase::Gather
    );
}
