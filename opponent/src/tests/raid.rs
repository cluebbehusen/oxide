use super::*;

/// An East building in the middle of the field, in sight of West raiders
/// standing near it.
const OUTPOST: TilePos = TilePos::new(22, 10);

/// West raider spots near the outpost.
const RAIDERS: [(i32, i32); 2] = [(18, 10), (18, 12)];

/// The field with West raiders of `kind` near an East building of `outpost`.
fn outpost(kind: UnitKind, outpost: BuildingKind) -> Scenario {
    let mut scenario = field();
    for (x, y) in RAIDERS {
        scenario.units.push(unit(0, kind, x, y));
    }
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: outpost,
        x: OUTPOST.x,
        y: OUTPOST.y,
    });
    scenario
}

fn raid(missions: &[MissionStatus]) -> Option<MissionStatus> {
    missions
        .iter()
        .copied()
        .find(|mission| matches!(mission.kind, MissionKind::Raid { .. }))
}

fn raiders(state: &State) -> Vec<UnitId> {
    let mut ids: Vec<UnitId> = RAIDERS.iter().map(|(x, y)| at(state, *x, *y)).collect();
    ids.sort_unstable();
    ids
}

fn attacks(commands: &[PlayerCommand]) -> Vec<(Vec<UnitId>, AttackTarget)> {
    commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Attack { units, target, .. } => Some((units.clone(), *target)),
            _ => None,
        })
        .collect()
}

fn building_at(state: &State, anchor: TilePos) -> BuildingId {
    state
        .buildings()
        .iter()
        .find(|building| building.anchor == anchor)
        .unwrap()
        .id
}

#[test]
fn two_idle_scuttlers_raid_a_harvest_line_nobody_guards() {
    let scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    let state = scenario.build().unwrap();
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let mission = raid(&trace.unwrap().missions).expect("a raid forms");
    assert_eq!(
        mission.kind,
        MissionKind::Raid {
            owner: PlayerId(1),
            building: BuildingKind::Foundry,
            anchor: OUTPOST,
        }
    );
    assert_eq!(mission.phase, Phase::Travel);
    assert_eq!(hunts(&commands), [(raiders(&state), mission.goal)]);
}

#[test]
fn no_raid_sets_out_while_a_defense_is_under_way() {
    let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    scenario.units.push(unit(1, UnitKind::Sentinel, 7, 11));
    let state = scenario.build().unwrap();
    let (_, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let missions = trace.unwrap().missions;
    assert!(
        missions
            .iter()
            .any(|mission| matches!(mission.kind, MissionKind::Defend { .. })),
        "premise: {missions:?}"
    );
    assert!(raid(&missions).is_none(), "{missions:?}");
}

#[test]
fn an_outweighed_raid_turns_back_and_leaves_its_target_alone() {
    let scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let checkpoint = opponent.checkpoint();

    let mut guarded = scenario.clone();
    guarded.units.push(unit(1, UnitKind::Warden, 20, 11));
    let state = guarded.build().unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &guarded, &state, map(&guarded)).unwrap();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let mission = raid(&trace.unwrap().missions).expect("the raid is still under way");
    assert_eq!(mission.phase, Phase::Withdraw);
    assert_eq!(runs(&commands), [(raiders(&state), mission.goal)]);
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let abandoned = json["memory"]["abandoned"].to_string();
    assert!(abandoned.contains("\"x\":22"), "{abandoned}");
}

#[test]
fn sappers_blow_up_a_lightly_defended_building() {
    let scenario = outpost(UnitKind::Sapper, BuildingKind::Fabricator);
    let state = scenario.build().unwrap();
    let fabricator = building_at(&state, OUTPOST);
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    assert!(raid(&trace.unwrap().missions).is_some());
    assert_eq!(
        attacks(&commands),
        [(raiders(&state), AttackTarget::Building(fabricator))]
    );
}

#[test]
fn bombers_too_few_for_a_strike_harry_a_harvest_line() {
    let scenario = outpost(UnitKind::Buzzard, BuildingKind::Foundry);
    let state = scenario.build().unwrap();
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let missions = trace.unwrap().missions;
    assert!(
        missions
            .iter()
            .all(|mission| !matches!(mission.kind, MissionKind::Strike { .. })),
        "premise: {missions:?}"
    );
    let mission = raid(&missions).expect("a raid forms");
    assert_eq!(hunts(&commands), [(raiders(&state), mission.goal)]);
}

/// An attack on the East start with eight West Sentinels, one still walking
/// in so the army keeps regrouping, a free Sapper, and a known East Turret
/// beside the target when `defended`. The attack is in `phase`.
fn sapping(
    phase: serde_json::Value,
    member: bool,
) -> (State, Opponent, UnitId, Option<BuildingId>) {
    let mut scenario = field();
    let west: Vec<(i32, i32)> = (6..10).flat_map(|x| [(x, 9), (x, 10)]).collect();
    for (x, y) in &west {
        scenario.units.push(unit(0, UnitKind::Sentinel, *x, *y));
    }
    scenario.units.push(unit(0, UnitKind::Sapper, 10, 12));
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Turret,
        x: 40,
        y: 10,
    });
    scenario.units.push(unit(0, UnitKind::Kestrel, 40, 12));
    let mut state = scenario.build().unwrap();
    let sentinels: Vec<UnitId> = west.iter().map(|(x, y)| at(&state, *x, *y)).collect();
    let sapper = at(&state, 10, 12);
    let turret = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Turret)
        .map(|building| building.id);
    state.tick(&[run(0, vec![sentinels[7]], 6, 14)]);
    while state.current_tick() < 12 {
        state.tick(&[]);
    }
    let mut members = sentinels;
    if member {
        members.push(sapper);
    }
    members.sort_unstable();
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["missions"] = serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {
                "task": "attack",
                "target": {"owner": 1, "building": "foundry", "anchor": {"x": 43, "y": 11}},
                "phase": phase,
            },
            "since": 12,
            "units": members,
            "goal": {"x": 11, "y": 11},
        }],
    });
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    (state, opponent, sapper, turret)
}

#[test]
fn an_attack_on_a_defended_target_takes_free_sappers_along() {
    let (state, mut opponent, sapper, _) = sapping(serde_json::json!("recover"), false);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert!(
        runs(&commands).iter().any(|(units, _)| *units == [sapper]),
        "{commands:?}"
    );
}

#[test]
fn a_fighting_attack_sends_its_sappers_at_the_nearest_defense() {
    let (state, mut opponent, sapper, turret) =
        sapping(serde_json::json!({"engage": {"focus": null}}), true);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert_eq!(
        attacks(&commands),
        [(vec![sapper], AttackTarget::Building(turret.unwrap()))]
    );
}

#[test]
fn checkpoints_reject_impossible_raids() {
    let scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let restore = |json: serde_json::Value| {
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).map(|_| ())
    };
    assert!(restore(json.clone()).is_ok());
    let mut off_map = json.clone();
    off_map["missions"]["list"][0]["task"]["target"]["anchor"]["x"] = 99.into();
    assert!(restore(off_map).is_err());
    let mut twice = json.clone();
    let mut second = twice["missions"]["list"][0].clone();
    second["id"] = 1.into();
    second["units"] = serde_json::json!([]);
    twice["missions"]["list"]
        .as_array_mut()
        .unwrap()
        .push(second);
    twice["missions"]["next"] = 2.into();
    assert!(restore(twice).is_err());
}

#[test]
fn mirrored_seats_raid_alike() {
    let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    for (x, y) in RAIDERS {
        scenario
            .units
            .push(unit(1, UnitKind::Scuttler, 48 - 1 - x, 24 - 1 - y));
    }
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 48 - 2 - OUTPOST.x,
        y: 24 - 2 - OUTPOST.y,
    });
    let state = scenario.build().unwrap();
    let west = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let east = seat(&scenario, 1).act(&state, &mut OwnEvents::default());
    assert!(!hunts(&west).is_empty(), "premise: {west:?}");
    assert_eq!(mirror(&state, west), east);
}
