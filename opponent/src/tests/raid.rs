use super::*;

/// An East building in the middle of the field, in sight of West raiders
/// standing near it.
const OUTPOST: TilePos = TilePos::new(22, 10);

/// West raider spots near the outpost.
const RAIDERS: [(i32, i32); 2] = [(18, 10), (18, 12)];

/// The field with West's garrison and West raiders of `kind` near an East
/// building of `outpost`.
fn outpost(kind: UnitKind, outpost: BuildingKind) -> Scenario {
    let mut scenario = field();
    garrison(&mut scenario);
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

/// The single staged raider an unguarded target needs, sent to `goal`.
fn one_raider(state: &State, sent: &[(Vec<UnitId>, TilePos)], goal: TilePos) -> bool {
    matches!(sent, [(units, to)] if units.len() == 1
        && raiders(state).contains(&units[0])
        && *to == goal)
}

#[test]
fn one_of_two_idle_scuttlers_raids_a_harvest_line_nobody_guards() {
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
    let sent = hunts(&commands);
    assert!(
        one_raider(&state, &sent, mission.goal),
        "an unguarded line needs one raider: {sent:?}"
    );
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
    let sent = runs(&commands);
    assert!(one_raider(&state, &sent, mission.goal), "{sent:?}");
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let raided = json["memory"]["raided"].to_string();
    assert!(raided.contains("\"x\":22"), "{raided}");
    let abandoned = json["memory"]["abandoned"].to_string();
    assert!(
        !abandoned.contains("\"x\":22"),
        "larger missions may still go after it: {abandoned}"
    );
}

#[test]
fn sappers_at_the_target_blow_it_up_whatever_stands_there() {
    let mut scenario = outpost(UnitKind::Sapper, BuildingKind::Fabricator);
    scenario.units.push(unit(1, UnitKind::Warden, 20, 11));
    let mut state = scenario.build().unwrap();
    let sappers = raiders(&state);
    let fabricator = building_at(&state, OUTPOST);
    state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Attack {
            units: sappers.clone(),
            target: AttackTarget::Building(fabricator),
            queue: false,
        },
    }]);
    while state.current_tick() < 12 {
        state.tick(&[]);
    }
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["missions"] = serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {
                "task": "raid",
                "target": {"owner": 1, "building": "fabricator", "anchor": {"x": 22, "y": 10}},
                "phase": "strike",
            },
            "since": 0,
            "units": sappers,
            "goal": {"x": 21, "y": 10},
        }],
    });
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(runs(&commands).is_empty(), "{commands:?}");
    let missions = trace.unwrap().missions;
    assert_eq!(
        raid(&missions).map(|mission| mission.phase),
        Some(Phase::Engage),
        "{missions:?} {commands:?}"
    );
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
    let sent = hunts(&commands);
    assert!(one_raider(&state, &sent, mission.goal), "{sent:?}");
}

/// An attack on the East start with eight West Sentinels, one still walking
/// in so the army keeps regrouping, a free Sapper, and a known East Turret
/// beside the target. The attack is in `phase`, with the Sapper a member
/// when `member`.
fn sapping(
    phase: &serde_json::Value,
    member: bool,
    since: u64,
) -> (State, Opponent, UnitId, Option<BuildingId>) {
    let (state, opponent, sappers, turrets) =
        besieging(phase, member, since, &[(40, 10)], &[(10, 12)], |_| {});
    (state, opponent, sappers[0], turrets.first().copied())
}

/// An attack on the East start with eight West Sentinels, one still walking
/// in so the army keeps regrouping, West Sappers on `sappers` (members when
/// `member`), known East Turrets on `turrets`, and `stage` applied to the
/// scenario. The attack is in `phase`. Returns the Sappers and Turrets.
fn besieging(
    phase: &serde_json::Value,
    member: bool,
    since: u64,
    turrets: &[(i32, i32)],
    sappers: &[(i32, i32)],
    stage: impl FnOnce(&mut Scenario),
) -> (State, Opponent, Vec<UnitId>, Vec<BuildingId>) {
    let mut scenario = field();
    let west: Vec<(i32, i32)> = (6..10).flat_map(|x| [(x, 9), (x, 10)]).collect();
    for (x, y) in &west {
        scenario.units.push(unit(0, UnitKind::Sentinel, *x, *y));
    }
    for (x, y) in sappers {
        scenario.units.push(unit(0, UnitKind::Sapper, *x, *y));
    }
    for (x, y) in turrets {
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Turret,
            x: *x,
            y: *y,
        });
    }
    scenario.units.push(unit(0, UnitKind::Kestrel, 40, 12));
    stage(&mut scenario);
    let mut state = scenario.build().unwrap();
    let sentinels: Vec<UnitId> = west.iter().map(|(x, y)| at(&state, *x, *y)).collect();
    let sappers: Vec<UnitId> = sappers.iter().map(|(x, y)| at(&state, *x, *y)).collect();
    let turrets: Vec<BuildingId> = state
        .buildings()
        .iter()
        .filter(|building| building.kind == BuildingKind::Turret)
        .map(|building| building.id)
        .collect();
    state.tick(&[run(0, vec![sentinels[7]], 6, 14)]);
    while state.current_tick() < 12 {
        state.tick(&[]);
    }
    let mut members = sentinels;
    if member {
        members.extend(&sappers);
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
            "since": since,
            "units": members,
            "goal": {"x": 11, "y": 11},
        }],
    });
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    (state, opponent, sappers, turrets)
}

/// Four East Turrets around the East start, in sight of the West Kestrel.
const RING: [(i32, i32); 4] = [(40, 10), (40, 14), (45, 8), (46, 15)];

#[test]
fn an_attack_takes_a_sapper_for_each_known_defense_around_its_target() {
    let spots: Vec<(i32, i32)> = (10..15).map(|x| (x, 12)).collect();
    let (state, mut opponent, sappers, _) = besieging(
        &serde_json::json!("recover"),
        false,
        12,
        &RING,
        &spots,
        |_| {},
    );
    let commands = opponent.act(&state, &mut OwnEvents::default());
    let taken: Vec<UnitId> = runs(&commands)
        .into_iter()
        .flat_map(|(units, _)| units)
        .filter(|unit| sappers.contains(unit))
        .collect();
    assert_eq!(taken.len(), RING.len(), "{commands:?}");
}

#[test]
fn more_known_defenses_around_an_attack_s_target_train_more_sappers() {
    let trained = |turrets: &[(i32, i32)]| {
        let spots: Vec<(i32, i32)> = (10..13).map(|x| (x, 12)).collect();
        let (state, mut opponent, _, _) = besieging(
            &serde_json::json!("recover"),
            true,
            12,
            turrets,
            &spots,
            |scenario| {
                scenario.players[0].scrap = 1_000;
                scenario.buildings.push(BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Fabricator,
                    x: 3,
                    y: 14,
                });
            },
        );
        trains(&opponent.act(&state, &mut OwnEvents::default()))
            .iter()
            .any(|(_, kind)| *kind == UnitKind::Sapper)
    };
    assert!(!trained(&RING[..3]), "three Sappers answer three Turrets");
    assert!(trained(&RING), "a fourth Turret wants a fourth Sapper");
}

#[test]
fn a_line_guarded_beyond_the_raiders_by_the_margin_draws_no_raid() {
    let raided = |guard: bool| {
        let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
        scenario.units.push(unit(0, UnitKind::Scuttler, 18, 11));
        if guard {
            scenario.units.push(unit(1, UnitKind::Sentinel, 24, 11));
        }
        let state = scenario.build().unwrap();
        let (_, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
        raid(&trace.unwrap().missions).is_some()
    };
    assert!(raided(false), "premise: an unguarded line draws a raid");
    assert!(
        !raided(true),
        "three Scuttlers outweigh a Sentinel, but not by the margin"
    );
}

#[test]
fn scuttlers_leave_a_well_guarded_line_to_attacks_however_many_wait() {
    let raided = |guards: i32| {
        let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
        for y in 5..=16 {
            scenario.units.push(unit(0, UnitKind::Scuttler, 17, y));
        }
        for y in 10..10 + guards {
            scenario.units.push(unit(1, UnitKind::Sentinel, 24, y));
        }
        let state = scenario.build().unwrap();
        let (_, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
        raid(&trace.unwrap().missions).is_some()
    };
    assert!(raided(1), "premise: a lightly guarded line draws a raid");
    assert!(
        !raided(3),
        "fourteen Scuttlers outweigh three Sentinels, but it is an attack's work"
    );
}

#[test]
fn scuttlers_leave_a_line_under_known_guns_however_many_wait() {
    let raided = |turret: bool| {
        let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
        for y in 5..=9 {
            scenario.units.push(unit(0, UnitKind::Scuttler, 17, y));
        }
        if turret {
            scenario.buildings.push(BuildingSpec {
                player: 1,
                kind: BuildingKind::Turret,
                x: 22,
                y: 13,
            });
        }
        let state = scenario.build().unwrap();
        let (_, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
        raid(&trace.unwrap().missions).is_some()
    };
    assert!(raided(false), "premise: an unguarded line draws a raid");
    assert!(
        !raided(true),
        "seven Scuttlers outweigh one Turret, but it is an attack's work"
    );
}

#[test]
fn a_turret_out_of_reach_of_the_line_leaves_it_to_raiders() {
    let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    for y in 5..=9 {
        scenario.units.push(unit(0, UnitKind::Scuttler, 17, y));
    }
    scenario.units.push(unit(0, UnitKind::Kestrel, 25, 8));
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Turret,
        x: 29,
        y: 10,
    });
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(
        observation
            .enemy_buildings
            .iter()
            .any(|building| building.kind == BuildingKind::Turret),
        "premise: the Turret is known"
    );
    let (_, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    assert!(raid(&trace.unwrap().missions).is_some());
}

#[test]
fn a_raid_sends_only_the_raiders_its_target_needs() {
    let spots = [(17, 10), (17, 11), (17, 12), (18, 11)];
    let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    for (x, y) in spots {
        scenario.units.push(unit(0, UnitKind::Scuttler, x, y));
    }
    scenario.units.push(unit(1, UnitKind::Sentinel, 24, 11));
    let state = scenario.build().unwrap();
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    let mission = raid(&trace.unwrap().missions).expect("a raid forms");
    let [(sent, goal)] = &hunts(&commands)[..] else {
        panic!("{commands:?}");
    };
    assert_eq!(*goal, mission.goal);
    let worth = |count: usize| count as u64 * u64::from(UnitKind::Scuttler.stats().cost);
    assert!(
        worth(sent.len()) > u64::from(UnitKind::Sentinel.stats().cost),
        "the squad outweighs the guard: {sent:?}"
    );
    assert!(
        sent.len() < RAIDERS.len() + spots.len(),
        "the rest stay home: {sent:?}"
    );
}

#[test]
fn an_attack_on_a_defended_target_takes_free_sappers_along() {
    let (state, mut opponent, sapper, _) = sapping(&serde_json::json!("recover"), false, 12);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert!(
        runs(&commands).iter().any(|(units, _)| *units == [sapper]),
        "{commands:?}"
    );
}

#[test]
fn a_fighting_attack_sends_its_sappers_at_the_nearest_defense() {
    let (state, mut opponent, sapper, turret) =
        sapping(&serde_json::json!({"engage": {"focus": null}}), true, 12);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert_eq!(
        attacks(&commands),
        [(vec![sapper], AttackTarget::Building(turret.unwrap()))]
    );
}

#[test]
fn a_fighting_attack_s_sappers_keep_their_orders_when_it_regroups() {
    let (state, mut opponent, sapper, turret) =
        sapping(&serde_json::json!({"engage": {"focus": null}}), true, 0);
    let late = {
        let mut state = state.clone();
        while state.current_tick() < 3_612 {
            state.tick(&[]);
        }
        state
    };
    let commands = opponent.act(&late, &mut OwnEvents::default());
    assert!(
        attacks(&commands).contains(&(vec![sapper], AttackTarget::Building(turret.unwrap()))),
        "{commands:?}"
    );
    assert!(
        runs(&commands)
            .iter()
            .all(|(units, _)| !units.contains(&sapper)),
        "the retreat leaves the Sapper to its target: {commands:?}"
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
    let twice = |units: serde_json::Value| {
        let mut twice = json.clone();
        let mut second = twice["missions"]["list"][0].clone();
        second["id"] = 1.into();
        second["units"] = units;
        twice["missions"]["list"]
            .as_array_mut()
            .unwrap()
            .push(second);
        twice["missions"]["next"] = 2.into();
        restore(twice)
    };
    assert!(twice(serde_json::json!([])).is_err());
    assert!(
        twice(json["missions"]["list"][0]["units"].clone()).is_err(),
        "two raids never share a unit"
    );
    assert!(
        twice(serde_json::json!([9_999])).is_ok(),
        "raids run side by side"
    );
}

#[test]
fn mirrored_seats_raid_alike() {
    let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    for (x, y) in GARRISON {
        scenario
            .units
            .push(unit(1, UnitKind::Sentinel, 48 - 1 - x, 24 - 1 - y));
    }
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

#[test]
fn raiders_of_two_kinds_raid_distinct_targets_at_once() {
    let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    for (x, y) in [(17, 10), (17, 12)] {
        scenario.units.push(unit(0, UnitKind::Sapper, x, y));
    }
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Foundry,
        x: OUTPOST.x,
        y: OUTPOST.y + 4,
    });
    let state = scenario.build().unwrap();
    let trace = seat(&scenario, 0)
        .act_traced(&state, &mut OwnEvents::default())
        .1
        .unwrap();
    let raids: Vec<MissionStatus> = trace
        .missions
        .into_iter()
        .filter(|mission| matches!(mission.kind, MissionKind::Raid { .. }))
        .collect();
    let [first, second] = raids[..] else {
        panic!("{raids:?}");
    };
    assert_ne!(first.kind, second.kind, "each goes after its own target");
}

#[test]
fn defenses_remembered_out_of_sight_each_want_a_sapper() {
    let spots: Vec<(i32, i32)> = (10..13).map(|x| (x, 12)).collect();
    let (mut state, mut opponent, _, _) = besieging(
        &serde_json::json!("recover"),
        true,
        12,
        &RING,
        &spots,
        |scenario| {
            scenario.players[0].scrap = 1_000;
            scenario.buildings.push(BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: 3,
                y: 14,
            });
        },
    );
    let kestrel = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Kestrel)
        .unwrap()
        .id;
    state.tick(&[run(0, vec![kestrel], 3, 3)]);
    let remembered = |state: &State| {
        ObservationData::fog_honest(state, PlayerId(0))
            .enemy_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Turret && !building.seen)
            .count()
    };
    while remembered(&state) < RING.len() || !opponent.decision_due(&state) {
        assert!(
            state.current_tick() < 600,
            "premise: the Turrets drop out of sight"
        );
        state.tick(&[]);
    }
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert!(
        trains(&commands)
            .iter()
            .any(|(_, kind)| *kind == UnitKind::Sapper),
        "four remembered Turrets want a fourth Sapper"
    );
}

#[test]
fn a_scuttler_out_scouting_leaves_the_raid_stock_short() {
    let mut scenario = outpost(UnitKind::Scuttler, BuildingKind::Foundry);
    scenario.units.retain(|unit| (unit.x, unit.y) != RAIDERS[1]);
    scenario.units.push(harvester(0, 2, 2));
    scenario.players[0].scrap = 1_000;
    let mut state = scenario.build().unwrap();
    let scout = at(&state, RAIDERS[0].0, RAIDERS[0].1);
    advance_to(&mut state, 120, &[]);
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    // Income enough to raid, and the one Scuttler holding the one scouting
    // point.
    json["income"] = serde_json::json!({"previous": null, "per_minute": 2_000});
    json["missions"] = serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {"task": "scout", "point": 0},
            "since": 100,
            "units": [scout],
            "goal": {"x": 40, "y": 11},
        }],
        "waiting": null,
    });
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert!(
        trains(&commands)
            .iter()
            .any(|(_, kind)| *kind == UnitKind::Scuttler),
        "{commands:?}"
    );
}
