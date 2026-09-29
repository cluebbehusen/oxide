use super::*;

/// Two islands across a pit no ground unit crosses and neither side sees
/// over, half-turn symmetric.
const STRAIT: [&str; 24] = [
    "########################################",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#..1.........~~~~~~~~~~~~~~........2...#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "#............~~~~~~~~~~~~~~............#",
    "########################################",
];

/// West's Fabricator and Airworks.
const TECH: [(BuildingKind, i32, i32); 2] = [
    (BuildingKind::Fabricator, 7, 4),
    (BuildingKind::Airworks, 7, 17),
];

/// West's two Skyhooks and eight Sentinels.
const LIFT: [(UnitKind, i32, i32); 10] = [
    (UnitKind::Skyhook, 10, 8),
    (UnitKind::Skyhook, 10, 15),
    (UnitKind::Sentinel, 6, 10),
    (UnitKind::Sentinel, 7, 10),
    (UnitKind::Sentinel, 8, 10),
    (UnitKind::Sentinel, 9, 10),
    (UnitKind::Sentinel, 6, 13),
    (UnitKind::Sentinel, 7, 13),
    (UnitKind::Sentinel, 8, 13),
    (UnitKind::Sentinel, 9, 13),
];

const EAST_START: TilePos = TilePos::new(35, 11);

/// The strait with West's tech and lift force.
pub(super) fn strait() -> Scenario {
    let mut scenario = field();
    scenario.map = STRAIT.map(str::to_owned).to_vec();
    for (kind, x, y) in TECH {
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind,
            x,
            y,
        });
    }
    for (kind, x, y) in LIFT {
        scenario.units.push(unit(0, kind, x, y));
    }
    scenario
}

fn lift(missions: &[MissionStatus]) -> Option<MissionStatus> {
    missions
        .iter()
        .copied()
        .find(|mission| matches!(mission.kind, MissionKind::Lift { .. }))
}

fn east_start() -> MissionKind {
    MissionKind::Lift {
        owner: PlayerId(1),
        building: BuildingKind::Foundry,
        anchor: EAST_START,
    }
}

fn loads(commands: &[PlayerCommand]) -> Vec<(UnitId, Vec<UnitId>)> {
    commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Load {
                units, transport, ..
            } => Some((*transport, units.clone())),
            _ => None,
        })
        .collect()
}

fn unloads(commands: &[PlayerCommand]) -> Vec<(UnitId, TilePos, bool)> {
    commands
        .iter()
        .filter_map(|command| match command.command {
            Command::Unload {
                transport,
                at,
                queue,
            } => Some((transport, at, queue)),
            _ => None,
        })
        .collect()
}

fn commands_for(boarding: &[(UnitId, Vec<UnitId>)]) -> Vec<PlayerCommand> {
    boarding
        .iter()
        .map(|(transport, units)| PlayerCommand {
            player: PlayerId(0),
            command: Command::Load {
                units: units.clone(),
                transport: *transport,
                queue: false,
            },
        })
        .collect()
}

fn standing(state: &State, kind: BuildingKind, anchor: TilePos) -> bool {
    state
        .buildings()
        .iter()
        .any(|building| building.kind == kind && building.anchor == anchor)
}

fn skyhooks(state: &State, player: u8) -> Vec<&oxide_sim::state::Unit> {
    state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(player) && unit.kind == UnitKind::Skyhook)
        .collect()
}

/// Plays West until a decision leaves its lift in `phase`, returning the
/// commands of every decision on the way.
fn until_phase(opponent: &mut Opponent, state: &mut State, phase: Phase) -> Vec<PlayerCommand> {
    let mut history = Vec::new();
    loop {
        assert!(state.current_tick() < 3_000, "no lift reached {phase:?}");
        let (commands, trace) = opponent.act_traced(state, &mut OwnEvents::default());
        history.extend(commands.iter().cloned());
        state.tick(&commands);
        if trace.is_some_and(|trace| lift(&trace.missions).is_some_and(|m| m.phase == phase)) {
            return history;
        }
    }
}

/// `opponent` restored with remembered enemy units, each as `(id, kind, tile)`.
fn remembering(
    opponent: &Opponent,
    scenario: &Scenario,
    state: &State,
    units: &[(UnitId, &str, TilePos)],
) -> Opponent {
    let mut json = serde_json::to_value(opponent.checkpoint()).unwrap();
    json["memory"]["units"] = units
        .iter()
        .map(|(id, kind, tile)| {
            serde_json::json!({
                "id": id.0,
                "kind": kind,
                "tile": {"x": tile.x, "y": tile.y},
                "seen": state.current_tick(),
            })
        })
        .collect();
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    Opponent::restore(&checkpoint, scenario, state, map(scenario)).unwrap()
}

#[test]
fn a_severed_seat_lifts_its_army_across_and_takes_the_target() {
    let scenario = strait();
    let model = map(&scenario);
    let mut state = scenario.build().unwrap();
    let carriers = [at(&state, 10, 8), at(&state, 10, 15)];
    let mut opponent = seat(&scenario, 0);
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let mission = lift(&trace.unwrap().missions).expect("a lift forms");
    assert_eq!(mission.kind, east_start(), "no enemy building is known");
    assert_eq!(mission.phase, Phase::Load);
    let boarding = loads(&commands);
    let mut boarded: Vec<UnitId> = boarding
        .iter()
        .flat_map(|(_, units)| units.clone())
        .collect();
    boarded.sort_unstable();
    assert_eq!(boarding.len(), 2, "one Load per carrier");
    assert_eq!(boarded.len(), 8, "the whole army boards");

    state.tick(&commands);
    let mut drops = Vec::new();
    let mut fought = false;
    while state.current_tick() < 3_000 && standing(&state, BuildingKind::Foundry, EAST_START) {
        let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
        drops.extend(unloads(&commands));
        fought |= trace.is_some_and(|trace| {
            lift(&trace.missions).is_some_and(|mission| mission.phase == Phase::Fight)
        });
        state.tick(&commands);
    }
    assert!(!standing(&state, BuildingKind::Foundry, EAST_START));
    assert!(fought, "the landed army fought as one");
    let mut dropped: Vec<UnitId> = drops.iter().map(|(carrier, ..)| *carrier).collect();
    dropped.sort_unstable();
    assert_eq!(dropped, carriers, "each carrier unloads once");
    let island = model.component(EAST_START);
    for (_, landing, _) in &drops {
        assert_eq!(model.component(*landing), island);
        for (dx, dy) in (-4..=4).flat_map(|dy| (-4..=4).map(move |dx| (dx, dy))) {
            let near = model.component(landing.offset(dx, dy));
            assert!(
                near.is_none() || near == island,
                "riders set down around {landing:?} could land across the pit"
            );
        }
    }
    let home = model.component(TilePos::new(3, 11));
    for skyhook in skyhooks(&state, 0) {
        assert_eq!(model.component(skyhook.tile()), home, "carriers fly home");
    }
}

#[test]
fn a_lift_takes_no_more_carriers_than_one_decision_can_launch() {
    let mut scenario = strait();
    for (x, y) in [(10, 4), (10, 19), (11, 6), (11, 17)] {
        scenario.units.push(unit(0, UnitKind::Skyhook, x, y));
    }
    for x in 2..10 {
        scenario.units.push(unit(0, UnitKind::Sentinel, x, 9));
        scenario.units.push(unit(0, UnitKind::Sentinel, x, 14));
    }
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    let history = until_phase(&mut opponent, &mut state, Phase::Fly);
    let carriers = loads(&history).len();
    assert!(
        (1..=4).contains(&carriers),
        "six Skyhooks stand ready, but a Standard decision's six orders launch four: {carriers}"
    );
}

#[test]
fn remembered_anti_air_sends_the_carriers_around_it() {
    let mut scenario = strait();
    scenario.units.push(unit(1, UnitKind::Flakhound, 27, 11));
    scenario.units.push(unit(1, UnitKind::Flakhound, 27, 12));
    let state = scenario.build().unwrap();
    let flak: Vec<(UnitId, &str, TilePos)> = [(27, 11), (27, 12)]
        .into_iter()
        .map(|(x, y)| (at(&state, x, y), "flakhound", TilePos::new(x, y)))
        .collect();
    let fly = |mut opponent: Opponent| {
        let mut state = state.clone();
        let history = until_phase(&mut opponent, &mut state, Phase::Fly);
        let launch: Vec<PlayerCommand> = history
            .into_iter()
            .skip_while(|command| {
                !matches!(
                    command.command,
                    Command::Unload { .. } | Command::Run { .. }
                )
            })
            .collect();
        play(&mut opponent, &mut state, 600, &[]);
        let hp: u32 = skyhooks(&state, 0).iter().map(|unit| unit.hp).sum();
        (launch, hp)
    };

    let unaware = seat(&scenario, 0);
    let (launch, hp) = fly(remembering(&unaware, &scenario, &state, &flak));
    let first = runs(&launch[..1]);
    assert_eq!(
        first.len(),
        1,
        "carriers first fly to a via-point: {launch:?}"
    );
    let via = first[0].1;
    let drops = unloads(&launch);
    assert!(!drops.is_empty() && drops.iter().all(|(_, at, queue)| *queue && *at != via));
    assert!(
        runs(&launch[1..]).iter().any(|(_, goal)| *goal == via),
        "and come back the same way"
    );
    let full = 2 * UnitKind::Skyhook.stats().max_hp;
    assert_eq!(hp, full, "no carrier comes under fire");

    let (launch, hp) = fly(unaware);
    assert!(
        unloads(&launch).iter().all(|(_, _, queue)| !queue),
        "premise: straight in"
    );
    assert!(hp < full, "premise: the straight line is under fire");
}

#[test]
fn a_lift_shot_down_gives_up_its_target_and_the_next_lift_picks_another() {
    let mut scenario = strait();
    for (x, y) in [
        (6, 7),
        (7, 7),
        (8, 7),
        (9, 7),
        (6, 16),
        (7, 16),
        (8, 16),
        (9, 16),
    ] {
        scenario.units.push(unit(0, UnitKind::Sentinel, x, y));
    }
    scenario.units.push(unit(0, UnitKind::Skyhook, 11, 2));
    scenario.units.push(unit(0, UnitKind::Skyhook, 11, 21));
    let flak = [TilePos::new(27, 11), TilePos::new(27, 12)];
    for tile in flak {
        scenario
            .units
            .push(unit(1, UnitKind::Flakhound, tile.x, tile.y));
    }
    let fabricator = TilePos::new(27, 14);
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Fabricator,
        x: fabricator.x,
        y: fabricator.y,
    });
    let mut state = scenario.build().unwrap();
    let spare = [at(&state, 11, 2), at(&state, 11, 21)];
    advance_to(
        &mut state,
        12,
        &[run(0, vec![spare[0]], 1, 22), run(0, vec![spare[1]], 1, 1)],
    );
    let mut opponent = seat(&scenario, 0);
    let first = until_phase(&mut opponent, &mut state, Phase::Fly);
    let riders: Vec<UnitId> = loads(&first)
        .into_iter()
        .flat_map(|(_, units)| units)
        .collect();
    for (carrier, ..) in unloads(&first) {
        assert!(
            !spare.contains(&carrier),
            "premise: the spare carriers were busy"
        );
        state = wounded(&state, carrier, 1);
    }

    let traces = play(&mut opponent, &mut state, 1_500, &[]);
    let seen: Vec<MissionKind> = traces
        .iter()
        .filter_map(|trace| lift(&trace.missions))
        .map(|mission| mission.kind)
        .fold(Vec::new(), |mut seen, kind| {
            if seen.last() != Some(&kind) {
                seen.push(kind);
            }
            seen
        });
    let next = MissionKind::Lift {
        owner: PlayerId(1),
        building: BuildingKind::Fabricator,
        anchor: fabricator,
    };
    assert_eq!(seen, [east_start(), next], "{seen:?}");
    let alive = |id: &UnitId| state.units().iter().any(|unit| unit.id == *id);
    assert!(!riders.iter().any(alive), "the riders went down with them");
    let second = traces
        .iter()
        .filter_map(|trace| lift(&trace.missions))
        .find(|mission| mission.kind == next)
        .unwrap();
    for tile in flak {
        let (dx, dy) = (second.goal.x - tile.x, second.goal.y - tile.y);
        assert!(
            dx * dx + dy * dy > 25,
            "the next landing keeps clear of the seen flak"
        );
    }
}

#[test]
fn a_carrier_back_home_still_loaded_sets_its_riders_down_and_lets_them_go() {
    let scenario = strait();
    let model = map(&scenario);
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let formed = lift(&trace.unwrap().missions).unwrap();
    let mut json = serde_json::to_value(opponent.checkpoint()).unwrap();
    json["missions"]["list"][0]["task"]["phase"] = "fly".into();
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let riders: Vec<UnitId> = loads(&commands)
        .into_iter()
        .flat_map(|(_, units)| units)
        .collect();

    state.tick(&commands);
    while riders
        .iter()
        .any(|id| state.units().iter().any(|unit| unit.id == *id))
        || !state.current_tick().is_multiple_of(12)
    {
        assert!(state.current_tick() < 1_200, "premise: everyone boards");
        state.tick(&[]);
    }
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let commands = opponent.act(&state, &mut OwnEvents::default());
    let drops = unloads(&commands);
    assert_eq!(drops.len(), 2, "{commands:?}");
    for (carrier, at, queue) in drops {
        let carrier = state
            .units()
            .iter()
            .find(|unit| unit.id == carrier)
            .unwrap();
        assert_eq!(
            (at, queue),
            (carrier.tile(), false),
            "set down where it hovers"
        );
    }

    let next = state.current_tick() + 12;
    advance_to(&mut state, next, &commands);
    let (_, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert!(
        trace
            .unwrap()
            .missions
            .iter()
            .all(|mission| mission.id != formed.id),
        "the lift lets everyone go"
    );
    let home = model.component(TilePos::new(3, 11));
    for id in riders {
        let rider = state.units().iter().find(|unit| unit.id == id).unwrap();
        assert_eq!(model.component(rider.tile()), home);
    }
}

#[test]
fn severed_seats_buy_carriers_and_the_airworks_they_need() {
    let mut unhooked = strait();
    unhooked.units.retain(|unit| unit.kind != UnitKind::Skyhook);
    let banked = |mut scenario: Scenario, scrap: u32| {
        for player in &mut scenario.players {
            player.scrap = scrap;
        }
        scenario
    };
    let decide = |scenario: &Scenario| {
        let state = scenario.build().unwrap();
        let (commands, trace) = seat(scenario, 0).act_traced(&state, &mut OwnEvents::default());
        (commands, trace.unwrap())
    };
    let carriers = |commands: &[PlayerCommand]| {
        trains(commands)
            .into_iter()
            .filter(|(_, kind)| *kind == UnitKind::Skyhook)
            .count()
    };

    let severed = banked(unhooked.clone(), 1_000);
    let (commands, trace) = decide(&severed);
    assert_eq!(carriers(&commands), 1, "{commands:?}");
    assert!(
        loads(&commands).is_empty(),
        "nothing boards a carrier not built"
    );
    assert!(lift(&trace.missions).is_none());

    let mut connected = severed.clone();
    connected.map = FIELD.map(str::to_owned).to_vec();
    let (commands, _) = decide(&connected);
    assert_eq!(carriers(&commands), 0, "{commands:?}");

    let army = |commands: &[PlayerCommand]| {
        trains(commands)
            .into_iter()
            .filter(|(_, kind)| *kind != UnitKind::Harvester)
            .count()
    };
    let short = banked(severed.clone(), 200);
    let (commands, _) = decide(&short);
    assert_eq!(army(&commands), 0, "cheaper units wait for the carrier");
    let mut connected = banked(severed.clone(), 400);
    connected.map = FIELD.map(str::to_owned).to_vec();
    let (commands, _) = decide(&connected);
    assert!(
        army(&commands) > 0,
        "premise: the bank buys an army unit: {commands:?}"
    );

    let mut unequipped = banked(unhooked, 100);
    unequipped
        .buildings
        .retain(|building| building.kind != BuildingKind::Airworks);
    let (_, trace) = decide(&unequipped);
    assert_eq!(
        trace.target.map(|target| target.investment),
        Some(Investment::Tech(BuildingKind::Airworks))
    );
}

#[test]
fn mirrored_lifts_issue_mirrored_commands() {
    let mut scenario = strait();
    let (width, height) = (40, 24);
    for (kind, x, y) in TECH {
        let (w, h) = kind.base_stats().size;
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind,
            x: width - x - w,
            y: height - y - h,
        });
    }
    for (kind, x, y) in LIFT {
        scenario
            .units
            .push(unit(1, kind, width - 1 - x, height - 1 - y));
    }
    let mut state = scenario.build().unwrap();
    let mut seats = [seat(&scenario, 0), seat(&scenario, 1)];
    let mut boarded = false;
    loop {
        assert!(state.current_tick() < 600, "premise: the lifts launch");
        let west = seats[0].act(&state, &mut OwnEvents::default());
        let east = seats[1].act(&state, &mut OwnEvents::default());
        boarded |= !loads(&west).is_empty();
        let launched = !unloads(&west).is_empty();
        assert_eq!(
            mirror(&state, west.clone()),
            east,
            "tick {}",
            state.current_tick()
        );
        if launched {
            break;
        }
        let commands: Vec<PlayerCommand> = west.into_iter().chain(east).collect();
        state.tick(&commands);
    }
    assert!(boarded);
}

#[test]
fn a_mid_flight_checkpoint_resumes_identically() {
    let scenario = strait();
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    until_phase(&mut opponent, &mut state, Phase::Fly);
    while !state.current_tick().is_multiple_of(12) {
        state.tick(&[]);
    }
    let carried = skyhooks(&state, 0)
        .iter()
        .map(|unit| unit.cargo.len())
        .sum::<usize>();
    assert!(carried > 0, "premise: riders aboard");

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
fn checkpoints_reject_impossible_lifts() {
    let scenario = strait();
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let json = serde_json::to_value(opponent.checkpoint()).unwrap();
    let restore = |json: &serde_json::Value| {
        let checkpoint: Checkpoint =
            serde_json::from_value(json.clone()).map_err(|error| error.to_string())?;
        Opponent::restore(&checkpoint, &scenario, &state, map(&scenario))
    };
    assert!(restore(&json).is_ok());
    let with = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut json = json.clone();
        edit(&mut json["missions"]);
        restore(&json).err().unwrap()
    };
    assert_eq!(
        with(&|missions| {
            let mut copy = missions["list"][0].clone();
            copy["id"] = 1.into();
            copy["units"] = serde_json::json!([]);
            missions["list"].as_array_mut().unwrap().push(copy);
            missions["next"] = 2.into();
        }),
        "checkpoint mission could not have been recorded",
        "one lift at a time"
    );
    assert_eq!(
        with(&|missions| {
            missions["list"][0]["task"]["target"]["anchor"] = serde_json::json!({"x": 99, "y": 0});
        }),
        "checkpoint mission could not have been recorded"
    );
    assert!(
        with(&|missions| missions["list"][0]["task"]["phase"] = "hover".into())
            .contains("unknown variant")
    );
}

#[test]
fn riders_left_standing_fly_with_half_the_need_aboard_or_disband() {
    let scenario = strait();
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let formed = lift(&trace.unwrap().missions).unwrap();
    let checkpoint = opponent.checkpoint();
    let boarding = loads(&commands);
    let settled = |boarded: &[PlayerCommand]| {
        let mut state = state.clone();
        advance_to(&mut state, 240, boarded);
        let mut opponent =
            Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
        let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
        (commands, lift(&trace.unwrap().missions))
    };
    let stopped = |commands: &[PlayerCommand]| -> Vec<UnitId> {
        commands
            .iter()
            .filter_map(|command| match &command.command {
                Command::Stop { units } => Some(units.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    };

    let (commands, mission) = settled(&[]);
    assert!(mission.is_none_or(|mission| mission.id != formed.id));
    assert!(unloads(&commands).is_empty());
    let mut riders: Vec<UnitId> = boarding
        .iter()
        .flat_map(|(_, units)| units.clone())
        .collect();
    riders.sort_unstable();
    assert_eq!(
        stopped(&commands),
        riders,
        "no one is left walking to a carrier"
    );

    let half: Vec<PlayerCommand> = commands_for(&boarding[..1]);
    let (commands, mission) = settled(&half);
    let mission = mission.unwrap();
    assert_eq!((mission.id, mission.phase), (formed.id, Phase::Fly));
    let drops = unloads(&commands);
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].0, boarding[0].0, "only the loaded carrier flies");
    assert_eq!(stopped(&commands), boarding[1].1);
}

#[test]
fn a_carrier_over_a_building_moves_to_open_ground_before_anyone_boards() {
    let mut scenario = strait();
    scenario.units[0] = unit(0, UnitKind::Skyhook, 8, 5);
    let mut state = scenario.build().unwrap();
    let covered = at(&state, 8, 5);
    let mut opponent = seat(&scenario, 0);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    let moved = runs(&commands);
    assert_eq!(moved.len(), 1, "{commands:?}");
    assert_eq!(moved[0].0, [covered]);
    let (x, y) = (moved[0].1.x, moved[0].1.y);
    assert!(
        !((7..=8).contains(&x) && (4..=5).contains(&y)),
        "off the Fabricator"
    );
    assert!(
        loads(&commands)
            .iter()
            .all(|(carrier, _)| *carrier != covered)
    );

    state.tick(&commands);
    let history = until_phase(&mut opponent, &mut state, Phase::Fly);
    let boarding = loads(&history);
    assert!(boarding.iter().any(|(carrier, _)| *carrier == covered));
    let riders: Vec<UnitId> = boarding.into_iter().flat_map(|(_, units)| units).collect();
    assert_eq!(riders.len(), 8);
    assert!(
        riders
            .iter()
            .all(|id| state.units().iter().all(|unit| unit.id != *id)),
        "every rider boarded"
    );
}

#[test]
fn a_carrier_takes_the_strongest_payload_it_can_hold() {
    let mut scenario = strait();
    scenario.units.clear();
    scenario.units.push(unit(0, UnitKind::Skyhook, 10, 8));
    for x in 6..=9 {
        scenario.units.push(unit(0, UnitKind::Sentinel, x, 10));
    }
    scenario.units.push(unit(0, UnitKind::Breaker, 10, 20));
    let state = scenario.build().unwrap();
    let (skyhook, breaker) = (at(&state, 10, 8), at(&state, 10, 20));
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert_eq!(
        loads(&commands),
        [(skyhook, vec![breaker])],
        "four Sentinels nearer home fall short of the minimum"
    );
}

#[test]
fn a_short_defense_buys_its_army_before_a_carrier() {
    let mut scenario = strait();
    scenario.units.clear();
    for player in &mut scenario.players {
        player.scrap = 400;
    }
    for y in 9..=13 {
        scenario.units.push(unit(1, UnitKind::Sentinel, 6, y));
    }
    let state = scenario.build().unwrap();
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let trained = trains(&commands);
    assert!(
        trained.iter().all(|(_, kind)| *kind != UnitKind::Skyhook),
        "{trained:?}"
    );
    assert!(
        trained
            .iter()
            .any(|(_, kind)| !matches!(kind, UnitKind::Harvester | UnitKind::Skyhook)),
        "premise: the emergency buys an army: {trained:?}"
    );
}
