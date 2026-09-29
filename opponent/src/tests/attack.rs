use super::*;

/// A half-turn-symmetric field wide enough that an army gathering near home
/// is out of sight of the enemy, with room for a raid to reach home without
/// crossing the attack's road.
const FIELD: [&str; 24] = [
    "################################################",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..1.......................................2...#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "################################################",
];

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

/// The field with both seats and no units.
fn field() -> Scenario {
    let mut scenario = arena(0);
    scenario.map = FIELD.map(str::to_owned).to_vec();
    scenario.units.clear();
    scenario
}

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

fn runs(commands: &[PlayerCommand]) -> Vec<(Vec<UnitId>, TilePos)> {
    commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Run { units, goal, .. } => Some((units.clone(), *goal)),
            _ => None,
        })
        .collect()
}

/// `state` with `unit` at `hp`.
fn wounded(state: &State, unit: UnitId, hp: u32) -> State {
    let mut value = serde_json::to_value(state).unwrap();
    let units = value["units"].as_array_mut().unwrap();
    let entry = units
        .iter_mut()
        .find(|entry| entry["id"] == unit.0)
        .unwrap();
    entry["hp"] = hp.into();
    serde_json::from_value(value).unwrap()
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
    json["missions"]["list"][0]["phase"] = "travel".into();

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
        unit(1, UnitKind::Sentinel, 12, 22),
        unit(1, UnitKind::Sentinel, 13, 22),
    ]);
    let mut state = scenario.build().unwrap();
    let scuttlers = vec![at(&state, 10, 11), at(&state, 10, 12)];
    let raiders = vec![at(&state, 12, 22), at(&state, 13, 22)];
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
            let mut second = missions["list"][0].clone();
            second["id"] = 1.into();
            second["units"] = serde_json::json!([9_999]);
            missions["list"].as_array_mut().unwrap().push(second);
            missions["next"] = 2.into();
        }),
        "checkpoint mission could not have been recorded"
    );
    assert_eq!(
        rejected(&|missions| {
            missions["list"][0]["kind"] = serde_json::json!({"mission": "defend", "asset": 0});
        }),
        "checkpoint mission is in a phase its kind lacks"
    );
}
