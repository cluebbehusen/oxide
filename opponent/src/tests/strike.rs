use super::*;

/// West Buzzard spots near home.
const WING: [(i32, i32); 6] = [(6, 18), (7, 18), (8, 18), (9, 18), (10, 18), (11, 18)];

const EAST_START: TilePos = TilePos::new(43, 11);

/// The field with six West Buzzards.
fn winged() -> Scenario {
    let mut scenario = field();
    for (x, y) in WING {
        scenario.units.push(unit(0, UnitKind::Buzzard, x, y));
    }
    scenario
}

fn strike(missions: &[MissionStatus]) -> Option<MissionStatus> {
    missions
        .iter()
        .copied()
        .find(|mission| matches!(mission.kind, MissionKind::Strike { .. }))
}

fn target(anchor: TilePos) -> MissionKind {
    MissionKind::Strike {
        owner: PlayerId(1),
        building: BuildingKind::Foundry,
        anchor,
    }
}

/// Strike phases in the order the traces show them.
fn phases(traces: &[Trace]) -> Vec<Phase> {
    let mut seen = Vec::new();
    for trace in traces {
        if let Some(mission) = strike(&trace.missions)
            && seen.last() != Some(&mission.phase)
        {
            seen.push(mission.phase);
        }
    }
    seen
}

fn east_foundry(state: &State) -> Option<u32> {
    state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .map(|building| building.hp)
}

#[test]
fn free_bombers_strike_the_enemy_on_connected_and_severed_maps() {
    let mut severed = super::lift::strait();
    severed.units.clear();
    for (x, y) in WING {
        severed.units.push(unit(0, UnitKind::Buzzard, x, y));
    }
    for (scenario, anchor) in [(winged(), EAST_START), (severed, TilePos::new(35, 11))] {
        let mut state = scenario.build().unwrap();
        let full = east_foundry(&state).unwrap();
        let mut opponent = seat(&scenario, 0);
        let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
        let mission = strike(&trace.unwrap().missions).expect("a strike forms");
        assert_eq!(mission.kind, target(anchor));
        assert_eq!(mission.phase, Phase::Gather);
        let sent = runs(&commands);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1, mission.goal, "the wing gathers beside home");
        assert!(sent[0].0.len() < WING.len(), "only the strength it needs");

        state.tick(&commands);
        let traces = play(&mut opponent, &mut state, 2_400, &[]);
        let seen = phases(&traces);
        assert_eq!(
            seen[..3],
            [Phase::Gather, Phase::Travel, Phase::Engage],
            "{seen:?}"
        );
        assert!(
            east_foundry(&state).is_none_or(|hp| hp < full),
            "the strike hits the Foundry"
        );
    }
}

#[test]
fn known_anti_air_over_a_target_holds_a_strike_back() {
    let scenario = winged();
    let state = scenario.build().unwrap();
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["memory"]["units"] = (0..6)
        .map(|index: u32| {
            serde_json::json!({
                "id": 1_000 + index,
                "kind": "flakhound",
                "tile": {"x": 41, "y": 9 + index},
                "seen": 0,
            })
        })
        .collect();
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(strike(&trace.unwrap().missions).is_none());
    assert!(runs(&commands).is_empty());
}

#[test]
fn a_strike_outweighed_by_anti_air_withdraws_and_gives_up_its_target() {
    let mut scenario = winged();
    for y in 8..=15 {
        scenario.units.push(unit(1, UnitKind::Flakhound, 40, y));
    }
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    let traces = play(&mut opponent, &mut state, 1_800, &[]);
    let seen = phases(&traces);
    let withdrew = seen
        .iter()
        .position(|phase| *phase == Phase::Withdraw)
        .unwrap_or_else(|| panic!("{seen:?}"));
    assert!(seen[..withdrew].contains(&Phase::Travel), "{seen:?}");
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let abandoned = json["memory"]["abandoned"].as_array().unwrap();
    assert!(
        abandoned
            .iter()
            .any(|entry| entry["anchor"] == serde_json::json!({"x": 43, "y": 11})),
        "{abandoned:?}"
    );
}

#[test]
fn mirrored_strikes_issue_mirrored_commands() {
    let mut scenario = winged();
    scenario.players[1].faction = Faction::Ferrous;
    for (x, y) in WING {
        scenario
            .units
            .push(unit(1, UnitKind::Buzzard, 47 - x, 23 - y));
    }
    let mut state = scenario.build().unwrap();
    let mut seats = [seat(&scenario, 0), seat(&scenario, 1)];
    let mut travelled = false;
    while state.current_tick() < 240 {
        let (west, trace) = seats[0].act_traced(&state, &mut OwnEvents::default());
        let east = seats[1].act(&state, &mut OwnEvents::default());
        travelled |= trace.is_some_and(|trace| {
            strike(&trace.missions).is_some_and(|mission| mission.phase == Phase::Travel)
        });
        assert_eq!(
            mirror(&state, west.clone()),
            east,
            "tick {}",
            state.current_tick()
        );
        let commands: Vec<PlayerCommand> = west.into_iter().chain(east).collect();
        state.tick(&commands);
    }
    assert!(travelled, "premise: both strikes set out");
}

#[test]
fn a_mid_strike_checkpoint_resumes_identically() {
    let scenario = winged();
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    while !opponent
        .missions()
        .iter()
        .any(|mission| mission.phase == Phase::Travel)
    {
        assert!(state.current_tick() < 1_200, "premise: the strike sets out");
        let commands = opponent.act(&state, &mut OwnEvents::default());
        state.tick(&commands);
    }
    let json = serde_json::to_string(&opponent.checkpoint()).unwrap();
    let checkpoint: Checkpoint = serde_json::from_str(&json).unwrap();
    let mut restored = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let until = state.current_tick() + 300;
    while state.current_tick() < until {
        let commands = opponent.act(&state, &mut OwnEvents::default());
        assert_eq!(restored.act(&state, &mut OwnEvents::default()), commands);
        assert_eq!(restored.checkpoint(), opponent.checkpoint());
        state.tick(&commands);
    }
}

#[test]
fn checkpoints_reject_a_second_strike() {
    let scenario = winged();
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let mut json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let restore = |json: &serde_json::Value| {
        let checkpoint: Checkpoint = serde_json::from_value(json.clone()).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario))
    };
    assert!(restore(&json).is_ok());
    let missions = &mut json["missions"];
    let mut copy = missions["list"][0].clone();
    copy["id"] = 1.into();
    copy["units"] = serde_json::json!([]);
    missions["list"].as_array_mut().unwrap().push(copy);
    missions["next"] = 2.into();
    assert_eq!(
        restore(&json).err().unwrap(),
        "checkpoint mission could not have been recorded"
    );
}
