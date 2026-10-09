use super::lift::strait;
use super::*;

/// West's two Condors at home on the strait.
const BOMBERS: [(i32, i32); 2] = [(12, 2), (12, 21)];

/// `count` Flakhound tiles around East's start.
fn around_the_start(count: i32) -> Vec<(i32, i32)> {
    (0..count)
        .map(|index| (31 + index % 3, 9 + index / 3))
        .collect()
}

/// West on `scenario`, remembering East Flakhounds seen now at `flak`.
fn bombing(scenario: &Scenario, flak: &[(i32, i32)]) -> (State, Opponent) {
    let state = scenario.build().unwrap();
    let mut json = serde_json::to_value(seat(scenario, 0).checkpoint()).unwrap();
    json["memory"]["units"] = (1_000..)
        .zip(flak)
        .map(|(id, (x, y))| {
            serde_json::json!({
                "id": id,
                "kind": "flakhound",
                "tile": {"x": x, "y": y},
                "seen": 0,
            })
        })
        .collect();
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let opponent = Opponent::restore(&checkpoint, scenario, &state, map(scenario)).unwrap();
    (state, opponent)
}

fn with_bombers(mut scenario: Scenario) -> Scenario {
    for (x, y) in BOMBERS {
        scenario.units.push(unit(0, UnitKind::Condor, x, y));
    }
    scenario
}

fn clear(missions: &[MissionStatus]) -> Option<MissionStatus> {
    missions
        .iter()
        .copied()
        .find(|mission| matches!(mission.kind, MissionKind::Clear { .. }))
}

#[test]
fn splash_bombers_go_together_at_the_anti_air_around_the_lift_target() {
    let scenario = with_bombers(strait());
    let (state, mut opponent) = bombing(&scenario, &around_the_start(3));
    let condors: Vec<UnitId> = BOMBERS.iter().map(|(x, y)| at(&state, *x, *y)).collect();
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let mission = clear(&trace.unwrap().missions).expect("a clearance forms");
    let MissionKind::Clear { aim } = mission.kind else {
        unreachable!();
    };
    assert!(
        (31..34).contains(&aim.x) && aim.y == 9,
        "at the remembered anti-air: {aim:?}"
    );
    let sent: Vec<(Vec<UnitId>, TilePos)> = runs(&commands)
        .into_iter()
        .filter(|(units, _)| units.iter().any(|id| condors.contains(id)))
        .collect();
    let [(units, _)] = &sent[..] else {
        panic!("{commands:?}");
    };
    let mut units = units.clone();
    units.sort_unstable();
    assert_eq!(units, condors, "both bombers, in one order");
}

#[test]
fn bombers_go_only_once_they_outweigh_all_the_anti_air_around_the_target() {
    // Two groups north and south of East's start, each of which the bombers
    // outweigh alone.
    let groups: Vec<(i32, i32)> = [1, 20]
        .into_iter()
        .flat_map(|y| [(29, y), (30, y), (29, y + 1), (30, y + 1)])
        .collect();
    let scenario = with_bombers(strait());
    let (state, mut opponent) = bombing(&scenario, &groups[..4]);
    let (_, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(
        clear(&trace.unwrap().missions).is_some(),
        "premise: one group"
    );
    let (state, mut opponent) = bombing(&scenario, &groups);
    let (_, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(clear(&trace.unwrap().missions).is_none());
}

#[test]
fn a_seat_whose_ground_reaches_the_enemy_sends_no_clearance() {
    let mut scenario = with_bombers(field());
    garrison(&mut scenario);
    let (state, mut opponent) = bombing(&scenario, &around_the_start(3));
    let (_, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(clear(&trace.unwrap().missions).is_none());
}

#[test]
fn a_seat_that_can_only_lift_keeps_its_splash_bombers_out_of_strikes() {
    let mut scenario = with_bombers(strait());
    // A Fabricator far from the Flakhounds guarding East's start, and a
    // Kestrel that sees both.
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Fabricator,
        x: 29,
        y: 20,
    });
    scenario.units.push(unit(0, UnitKind::Kestrel, 32, 15));
    let (state, mut opponent) = bombing(&scenario, &around_the_start(12));
    let condors: Vec<u32> = BOMBERS.iter().map(|(x, y)| at(&state, *x, *y).0).collect();
    opponent.act(&state, &mut OwnEvents::default());
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    for mission in json["missions"]["list"].as_array().unwrap() {
        let units: Vec<u32> = serde_json::from_value(mission["units"].clone()).unwrap();
        assert!(
            units.iter().all(|id| !condors.contains(id)),
            "{}",
            mission["task"]
        );
    }
}

#[test]
fn a_mid_clearance_checkpoint_resumes_identically() {
    let scenario = with_bombers(strait());
    let (mut state, mut opponent) = bombing(&scenario, &around_the_start(3));
    loop {
        assert!(state.current_tick() < 1_200, "no clearance set out");
        let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
        state.tick(&commands);
        if trace.is_some_and(|trace| {
            clear(&trace.missions).is_some_and(|mission| mission.phase == Phase::Travel)
        }) {
            break;
        }
    }
    while !state.current_tick().is_multiple_of(12) {
        state.tick(&[]);
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
fn checkpoints_reject_a_clearance_aimed_off_the_map() {
    let scenario = with_bombers(strait());
    let (state, mut opponent) = bombing(&scenario, &around_the_start(3));
    opponent.act(&state, &mut OwnEvents::default());
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let list = json["missions"]["list"].as_array().unwrap();
    let index = list
        .iter()
        .position(|mission| mission["task"]["task"] == "clear")
        .expect("premise: a clearance");
    let restore = |json: serde_json::Value| {
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).map(|_| ())
    };
    assert!(restore(json.clone()).is_ok());
    let mut off = json.clone();
    off["missions"]["list"][index]["task"]["aim"] = serde_json::json!({"x": 99, "y": 0});
    assert_eq!(
        restore(off).err().unwrap(),
        "checkpoint mission could not have been recorded"
    );
}

#[test]
fn mirrored_seats_clear_alike() {
    let (width, height) = (40, 24);
    let flip = |(x, y): (i32, i32)| (width - 1 - x, height - 1 - y);
    let mut scenario = with_bombers(strait());
    for (kind, x, y) in super::lift::TECH {
        let (w, h) = kind.size();
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind,
            x: width - x - w,
            y: height - y - h,
        });
    }
    let east = super::lift::LIFT
        .iter()
        .map(|(kind, x, y)| (*kind, (*x, *y)))
        .chain(BOMBERS.iter().map(|tile| (UnitKind::Condor, *tile)));
    for (kind, tile) in east {
        let (x, y) = flip(tile);
        scenario.units.push(unit(1, kind, x, y));
    }
    let state = scenario.build().unwrap();
    let flak = around_the_start(3);
    let remembering = |player: u8, flak: Vec<(i32, i32)>| {
        let mut json = serde_json::to_value(seat(&scenario, player).checkpoint()).unwrap();
        json["memory"]["units"] = (1_000..)
            .zip(flak)
            .map(|(id, (x, y))| {
                serde_json::json!({
                    "id": id,
                    "kind": "flakhound",
                    "tile": {"x": x, "y": y},
                    "seen": 0,
                })
            })
            .collect();
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap()
    };
    let mut seats = [
        remembering(0, flak.clone()),
        remembering(1, flak.into_iter().map(flip).collect()),
    ];
    // The simulation heads a spawned Condor by its column, which a half
    // turn does not mirror, so flights stop mirroring once the bombers turn:
    // compare until the clearance sets out.
    let mut state = state;
    loop {
        assert!(state.current_tick() < 300, "premise: a clearance forms");
        let (west, trace) = seats[0].act_traced(&state, &mut OwnEvents::default());
        let east = seats[1].act(&state, &mut OwnEvents::default());
        assert_eq!(
            mirror(&state, west.clone()),
            east,
            "tick {}",
            state.current_tick()
        );
        if trace.is_some_and(|trace| clear(&trace.missions).is_some()) {
            break;
        }
        let commands: Vec<PlayerCommand> = west.into_iter().chain(east).collect();
        state.tick(&commands);
    }
}

#[test]
fn a_clearance_goes_at_a_known_flak_turret_around_the_target() {
    let mut scenario = with_bombers(strait());
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::FlakTurret,
        x: 30,
        y: 4,
    });
    scenario.units.push(unit(0, UnitKind::Kestrel, 31, 6));
    let (state, mut opponent) = bombing(&scenario, &[]);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(
        observation
            .enemy_buildings
            .iter()
            .any(|building| building.kind == BuildingKind::FlakTurret),
        "premise: West has seen it"
    );
    let (_, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let mission = clear(&trace.unwrap().missions).expect("a clearance forms");
    assert_eq!(
        mission.kind,
        MissionKind::Clear {
            aim: TilePos::new(30, 4)
        }
    );
}
