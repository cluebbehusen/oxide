use super::*;
use chassis::grid::as_index;

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
pub(super) const TECH: [(BuildingKind, i32, i32); 2] = [
    (BuildingKind::Fabricator, 7, 4),
    (BuildingKind::Airworks, 7, 17),
];

/// West's two Skyhooks and eight Sentinels.
pub(super) const LIFT: [(UnitKind, i32, i32); 10] = [
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

/// The strait with `skyhooks` more Skyhooks and sixteen more Sentinels at
/// West.
fn crowded(skyhooks: &[(i32, i32)]) -> Scenario {
    let mut scenario = strait();
    for (x, y) in skyhooks {
        scenario.units.push(unit(0, UnitKind::Skyhook, *x, *y));
    }
    for x in 2..10 {
        scenario.units.push(unit(0, UnitKind::Sentinel, x, 9));
        scenario.units.push(unit(0, UnitKind::Sentinel, x, 14));
    }
    scenario
}

const MORE_SKYHOOKS: [(i32, i32); 4] = [(10, 4), (10, 19), (11, 6), (11, 17)];

/// `state` with every Sentinel at half health, so each counts half its value
/// toward a lift and the stance minimum takes more carriers.
fn halved(mut state: State) -> State {
    let half = UnitKind::Sentinel.stats().max_hp.div_ceil(2);
    let sentinels: Vec<UnitId> = state
        .units()
        .iter()
        .filter(|unit| unit.kind == UnitKind::Sentinel)
        .map(|unit| unit.id)
        .collect();
    for id in sentinels {
        state = wounded(&state, id, half);
    }
    state
}

fn turtle(difficulty: BotDifficulty) -> BotConfig {
    BotConfig::new(difficulty, BotStance::Turtle, 11)
}

/// The carriers a take-off sends: the last group run of Skyhooks.
fn take_off(state: &State, commands: &[PlayerCommand]) -> Vec<UnitId> {
    let hooks: Vec<UnitId> = skyhooks(state, 0).iter().map(|unit| unit.id).collect();
    runs(commands)
        .into_iter()
        .rev()
        .find(|(units, _)| units.iter().all(|id| hooks.contains(id)))
        .map(|(units, _)| units)
        .unwrap_or_default()
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
fn a_lift_loads_every_carrier_its_need_calls_for_and_they_leave_together() {
    let scenario = crowded(&MORE_SKYHOOKS);
    let mut state = halved(scenario.build().unwrap());
    let mut opponent = seat_with(&scenario, 0, turtle(BotDifficulty::Standard));
    let history = until_phase(&mut opponent, &mut state, Phase::Fly);
    let mut carriers: Vec<UnitId> = loads(&history).into_iter().map(|(id, _)| id).collect();
    carriers.sort_unstable();
    assert!(
        carriers.len() > 4,
        "the minimum at half health takes more carriers than one Standard decision could launch with an Unload each: {carriers:?}"
    );
    let mut flying = take_off(&state, &history);
    flying.sort_unstable();
    assert_eq!(flying, carriers, "they leave together in one order");
}

#[test]
fn a_scrapheap_seat_loads_over_several_decisions_and_lands_its_riders() {
    let scenario = crowded(&MORE_SKYHOOKS);
    let model = map(&scenario);
    let island = model.component(EAST_START);
    let mut state = halved(scenario.build().unwrap());
    let mut opponent = seat_with(&scenario, 0, turtle(BotDifficulty::Scrapheap));
    let mut loading = Vec::new();
    let landed = |state: &State| {
        state.units().iter().any(|unit| {
            unit.player == PlayerId(0)
                && unit.kind == UnitKind::Sentinel
                && model.component(unit.tile()) == island
        })
    };
    while !landed(&state) {
        assert!(state.current_tick() < 3_000, "no rider landed: {loading:?}");
        let commands = opponent.act(&state, &mut OwnEvents::default());
        let issued = loads(&commands).len();
        if issued > 0 {
            loading.push(issued);
        }
        state.tick(&commands);
    }
    assert!(
        loading.len() > 1 && loading.iter().all(|issued| *issued <= 3),
        "carriers load over several three-order decisions: {loading:?}"
    );
}

#[test]
fn a_mid_load_checkpoint_resumes_identically() {
    let mut scenario = crowded(&MORE_SKYHOOKS);
    scenario.players[0].bot_config = Some(turtle(BotDifficulty::Scrapheap));
    let mut state = halved(scenario.build().unwrap());
    let mut opponent = seat_with(&scenario, 0, turtle(BotDifficulty::Scrapheap));
    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let mission = lift(&trace.unwrap().missions).expect("a lift forms");
    assert_eq!(mission.phase, Phase::Load);
    assert!(
        loads(&commands).len() < 5,
        "premise: more carriers board later"
    );
    state.tick(&commands);
    while !state.current_tick().is_multiple_of(24) {
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
fn the_carrier_stock_follows_the_need_and_the_riders_at_home() {
    let skyhook = |commands: &[PlayerCommand]| {
        trains(commands)
            .iter()
            .filter(|(_, kind)| *kind == UnitKind::Skyhook)
            .count()
    };
    let army = |commands: &[PlayerCommand]| {
        trains(commands)
            .iter()
            .filter(|(_, kind)| !matches!(kind, UnitKind::Skyhook | UnitKind::Harvester))
            .count()
    };
    let decide = |scenario: &Scenario, state: &State| {
        seat_with(scenario, 0, turtle(BotDifficulty::Standard))
            .act(state, &mut OwnEvents::default())
    };

    let mut short = crowded(&MORE_SKYHOOKS[..2]);
    short.players[0].scrap = 1_000;
    let state = halved(short.build().unwrap());
    assert_eq!(
        skyhook(&decide(&short, &state)),
        1,
        "four Skyhooks cannot carry the minimum at half health"
    );

    let mut enough = strait();
    enough.units.push(unit(0, UnitKind::Skyhook, 10, 4));
    enough.units.push(unit(0, UnitKind::Sentinel, 6, 11));
    for scrap in [1_000, 200] {
        enough.players[0].scrap = scrap;
        let commands = decide(&enough, &enough.build().unwrap());
        assert_eq!(skyhook(&commands), 0, "three carry all nine riders");
        assert!(
            army(&commands) > 0,
            "with carriers enough nothing waits: {commands:?}"
        );
    }
}

#[test]
fn the_carrier_stock_counts_riders_as_they_pack() {
    let staged = |skyhooks: usize, sentinels: i32| {
        let mut scenario = strait();
        scenario.players[0].scrap = 1_000;
        scenario
            .units
            .retain(|unit| !matches!(unit.kind, UnitKind::Skyhook | UnitKind::Sentinel));
        for (x, y) in [(10, 8), (10, 15), (10, 4), (10, 19)]
            .into_iter()
            .take(skyhooks)
        {
            scenario.units.push(unit(0, UnitKind::Skyhook, x, y));
        }
        for x in 6..6 + sentinels {
            scenario.units.push(unit(0, UnitKind::Sentinel, x, 13));
        }
        for x in 6..10 {
            scenario.units.push(unit(0, UnitKind::Bombard, x, 10));
        }
        let state = scenario.build().unwrap();
        seat_with(&scenario, 0, turtle(BotDifficulty::Standard))
            .act(&state, &mut OwnEvents::default())
    };
    let skyhooks = |commands: &[PlayerCommand]| {
        trains(commands)
            .iter()
            .filter(|(_, kind)| *kind == UnitKind::Skyhook)
            .count()
    };

    let commands = staged(3, 0);
    assert_eq!(
        (skyhooks(&commands), loads(&commands).len()),
        (1, 0),
        "a Skyhook holds one Bombard, so three carry three of the four"
    );
    let commands = staged(4, 0);
    assert_eq!(
        (skyhooks(&commands), loads(&commands).len()),
        (0, 4),
        "four carry them all"
    );
    let commands = staged(3, 4);
    assert_eq!(
        skyhooks(&commands),
        1,
        "four Sentinels fill one Skyhook and the Bombards still take one each"
    );
}

#[test]
fn a_seat_needing_no_lift_leaves_its_carriers_where_they_hover() {
    let mut scenario = field();
    garrison(&mut scenario);
    scenario.units.push(unit(0, UnitKind::Skyhook, 4, 12));
    let state = scenario.build().unwrap();
    let hook = at(&state, 4, 12);
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert!(
        runs(&commands)
            .iter()
            .all(|(units, _)| !units.contains(&hook)),
        "{commands:?}"
    );
}

#[test]
fn an_army_no_lift_could_carry_buys_no_carriers() {
    let mut scenario = strait();
    scenario.players[0].scrap = 1_000;
    scenario
        .units
        .retain(|unit| !matches!(unit.kind, UnitKind::Skyhook | UnitKind::Sentinel));
    for y in 8..14 {
        scenario.units.push(unit(0, UnitKind::Buzzard, 9, y));
    }
    let state = scenario.build().unwrap();
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert!(
        trains(&commands)
            .iter()
            .all(|(_, kind)| *kind != UnitKind::Skyhook),
        "aircraft ride no carrier: {commands:?}"
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
            .skip_while(|command| !matches!(command.command, Command::Run { .. }))
            .collect();
        let mut later = Vec::new();
        let until = state.current_tick() + 600;
        while state.current_tick() < until {
            let commands = opponent.act(&state, &mut OwnEvents::default());
            later.extend(commands.iter().cloned());
            state.tick(&commands);
        }
        let hp: u32 = skyhooks(&state, 0).iter().map(|unit| unit.hp).sum();
        (runs(&launch), unloads(&later), hp)
    };

    let unaware = seat(&scenario, 0);
    let (launch, drops, hp) = fly(remembering(&unaware, &scenario, &state, &flak));
    let via = launch[0].1;
    let landing = launch[1].1;
    assert_ne!(
        via, landing,
        "carriers first fly to a via-point: {launch:?}"
    );
    assert!(
        !drops.is_empty()
            && drops
                .iter()
                .all(|(_, at, queue)| (*at, *queue) == (landing, false)),
        "and set riders down at the landing once there: {drops:?}"
    );
    let full = 2 * UnitKind::Skyhook.stats().max_hp;
    assert_eq!(hp, full, "no carrier comes under fire");

    let (launch, drops, hp) = fly(unaware);
    assert!(
        drops.iter().all(|(_, at, _)| *at == launch[0].1),
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
    let flying = take_off(&state, &first);
    assert!(!flying.is_empty(), "premise: the carriers took off");
    for carrier in flying {
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
fn emptied_carriers_short_of_orders_fly_home_before_the_lift_fights() {
    let scenario = strait();
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let mut json = serde_json::to_value(opponent.checkpoint()).unwrap();
    json["missions"]["list"][0]["task"]["phase"] = "fly".into();
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();

    // The same units, set down on East's island with an interceptor over the
    // strait that the flight home must go around, and two idle Harvesters at
    // home taking two of a Scrapheap seat's three orders.
    let mut landed = strait();
    landed.players[0].bot_config = Some(BotConfig::new(
        BotDifficulty::Scrapheap,
        BotStance::Balanced,
        11,
    ));
    let mut riders = (30..=33).flat_map(|x| [(x, 9), (x, 15)]);
    for spec in &mut landed.units {
        let (x, y) = match spec.kind {
            UnitKind::Skyhook if spec.y < 11 => (30, 11),
            UnitKind::Skyhook => (30, 13),
            _ => riders.next().unwrap(),
        };
        (spec.x, spec.y) = (x, y);
    }
    landed.units.push(unit(1, UnitKind::Sylph, 20, 8));
    for (x, y) in [(2, 14), (2, 15)] {
        landed.map[y].replace_range(x..=x, "s");
    }
    landed.units.push(harvester(0, 4, 14));
    landed.units.push(harvester(0, 4, 15));
    let mut state = landed.build().unwrap();
    let interceptor = [(at(&state, 20, 8), "sylph", TilePos::new(20, 8))];
    let restored = Opponent::restore(&checkpoint, &landed, &state, map(&landed)).unwrap();
    let mut opponent = remembering(&restored, &landed, &state, &interceptor);
    let carriers: Vec<UnitId> = skyhooks(&state, 0).iter().map(|unit| unit.id).collect();

    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    assert_eq!(harvests(&commands).len(), 2, "premise: {commands:?}");
    assert!(
        runs(&commands)
            .iter()
            .all(|(units, _)| units.iter().all(|id| !carriers.contains(id))),
        "no orders are left for the two-leg flight home: {commands:?}"
    );
    assert_eq!(
        lift(&trace.unwrap().missions).map(|mission| mission.phase),
        Some(Phase::Fly),
        "the lift waits for its carriers"
    );

    let mut home_runs = Vec::new();
    let (next, until) = (state.current_tick() + 12, state.current_tick() + 600);
    advance_to(&mut state, next, &commands);
    while state.current_tick() < until {
        let commands = opponent.act(&state, &mut OwnEvents::default());
        home_runs.extend(
            runs(&commands)
                .into_iter()
                .filter(|(units, _)| units.iter().all(|id| carriers.contains(id))),
        );
        state.tick(&commands);
    }
    let model = map(&landed);
    let home = model.component(TilePos::new(3, 11));
    assert!(
        home_runs
            .iter()
            .any(|(_, goal)| model.component(*goal) != home),
        "around the anti-air: {home_runs:?}"
    );
    for hook in skyhooks(&state, 0) {
        assert_eq!(model.component(hook.tile()), home, "{:?}", hook.tile());
    }
}

#[test]
fn a_lift_at_the_member_cap_boards_no_more_and_still_restores() {
    let mut scenario = strait();
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    // Members enough that one more load would pass the cap, standing on any
    // open ground of either island.
    let taken = |tile: TilePos| {
        state.units().iter().any(|unit| unit.tile() == tile)
            || state.buildings().iter().any(|building| {
                let (width, height) = building.stats().size;
                crate::frame::gap(building.anchor, (width, height), tile, (1, 1)) < 2
            })
    };
    let first = u32::try_from(state.units().len()).unwrap();
    let members: Vec<UnitSpec> = (1..23)
        .flat_map(|y| (1..39).map(move |x| TilePos::new(x, y)))
        .filter(|tile| {
            STRAIT[as_index(tile.y)].as_bytes()[as_index(tile.x)] == b'.' && !taken(*tile)
        })
        .take(255)
        .map(|tile| harvester(0, tile.x, tile.y))
        .collect();
    assert_eq!(members.len(), 255, "premise: room for every member");
    scenario.units.extend(members);
    let state = scenario.build().unwrap();
    let mut json = serde_json::to_value(opponent.checkpoint()).unwrap();
    json["missions"]["list"][0]["units"] = (first..first + 255).collect::<Vec<u32>>().into();
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    opponent.act(&state, &mut OwnEvents::default());
    assert!(
        Opponent::restore(&opponent.checkpoint(), &scenario, &state, map(&scenario)).is_ok(),
        "no lift grew past the cap"
    );
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
    let second = |units: serde_json::Value| {
        let mut json = json.clone();
        let missions = &mut json["missions"];
        let mut copy = missions["list"][0].clone();
        copy["id"] = 1.into();
        copy["units"] = units;
        missions["list"].as_array_mut().unwrap().push(copy);
        missions["next"] = 2.into();
        restore(&json)
    };
    assert!(
        second(serde_json::json!([9_999])).is_ok(),
        "lifts run side by side"
    );
    assert_eq!(
        second(json["missions"]["list"][0]["units"].clone())
            .err()
            .unwrap(),
        "checkpoint missions share a unit"
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
fn riders_left_standing_fly_only_with_the_need_aboard_or_disband() {
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
    assert!(
        mission.is_none_or(|mission| mission.id != formed.id),
        "half the payload does not fly"
    );
    assert!(runs(&commands).is_empty());
    assert_eq!(
        unloads(&commands)
            .into_iter()
            .map(|(carrier, _, _)| carrier)
            .collect::<Vec<_>>(),
        [boarding[0].0],
        "the loaded carrier sets its riders down"
    );
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

#[test]
fn a_severed_seat_without_an_airworks_trains_line_units_against_enemies_on_its_ground() {
    let staged = |east: &[(i32, i32)]| {
        let mut scenario = strait();
        scenario.units.clear();
        scenario
            .buildings
            .retain(|building| building.kind != BuildingKind::Airworks);
        for player in &mut scenario.players {
            player.scrap = 400;
        }
        for (x, y) in east {
            scenario.units.push(unit(1, UnitKind::Sentinel, *x, *y));
        }
        scenario
    };
    let line = |state: &State, commands: &[PlayerCommand]| {
        army(state, commands)
            .iter()
            .filter(|(_, kind)| {
                crate::composition::role(*kind) == Some(crate::composition::Role::Line)
            })
            .count()
    };

    let alone = staged(&[]);
    let state = alone.build().unwrap();
    let commands = seat(&alone, 0).act(&state, &mut OwnEvents::default());
    assert_eq!(line(&state, &commands), 0, "premise: nothing to fight");

    let across: Vec<(i32, i32)> = (9..=13).map(|y| (30, y)).collect();
    let far = staged(&across);
    let state = far.build().unwrap();
    let remembered: Vec<(UnitId, &str, TilePos)> = across
        .iter()
        .map(|&(x, y)| (at(&state, x, y), "sentinel", TilePos::new(x, y)))
        .collect();
    let mut opponent = remembering(&seat(&far, 0), &far, &state, &remembered);
    let commands = opponent.act(&state, &mut OwnEvents::default());
    assert_eq!(
        line(&state, &commands),
        0,
        "an army across the strait is no reason for line units"
    );

    let landed: Vec<(i32, i32)> = (9..=13).map(|y| (6, y)).collect();
    let invaded = staged(&landed);
    let state = invaded.build().unwrap();
    let commands = seat(&invaded, 0).act(&state, &mut OwnEvents::default());
    assert!(
        line(&state, &commands) > 0,
        "invaders on its own ground are: {commands:?}"
    );
}

/// The strait with West's tech and a Harvester but no army, and `scrap` for
/// both seats.
fn bare_strait(scrap: u32) -> Scenario {
    let mut scenario = strait();
    scenario.units.retain(|unit| unit.player != 0);
    scenario.units.push(harvester(0, 5, 5));
    for player in &mut scenario.players {
        player.scrap = scrap;
    }
    scenario
}

fn army(state: &State, commands: &[PlayerCommand]) -> Vec<(BuildingKind, UnitKind)> {
    trains(commands)
        .into_iter()
        .filter(|(_, kind)| crate::composition::role(*kind).is_some())
        .map(|(at, kind)| {
            let building = state.buildings().iter().find(|b| b.id == at).unwrap();
            (building.kind, kind)
        })
        .collect()
}

#[test]
fn a_severed_seat_without_an_airworks_saves_for_one_instead_of_line_units() {
    let mut scenario = bare_strait(300);
    scenario
        .buildings
        .retain(|building| building.kind != BuildingKind::Airworks);
    // A Turret already guards home, so the seat saves for tech.
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Turret,
        x: 6,
        y: 14,
    });
    let decide = |scenario: &Scenario| {
        let state = scenario.build().unwrap();
        let (commands, trace) = seat(scenario, 0).act_traced(&state, &mut OwnEvents::default());
        (army(&state, &commands), trace.unwrap())
    };
    let (trained, trace) = decide(&scenario);
    assert!(
        trained.is_empty(),
        "no ground unit could reach anyone: {trained:?}"
    );
    assert_eq!(
        trace.target.map(|target| target.investment),
        Some(Investment::Tech(BuildingKind::Airworks))
    );

    let mut connected = scenario.clone();
    connected.map = FIELD.map(str::to_owned).to_vec();
    let (trained, _) = decide(&connected);
    assert!(
        trained.iter().any(
            |(_, kind)| crate::composition::role(*kind) == Some(crate::composition::Role::Line)
        ),
        "premise: idle time becomes line units where they can walk: {trained:?}"
    );
}

#[test]
fn a_seat_that_gave_up_on_every_target_wants_no_strike_force() {
    let mut scenario = bare_strait(1_000);
    scenario.units.push(unit(0, UnitKind::Kestrel, 5, 5));
    let state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    opponent.act(&state, &mut OwnEvents::default());
    let mut json = serde_json::to_value(opponent.checkpoint()).unwrap();
    json["memory"]["abandoned"] = serde_json::json!([
        {"kind": "foundry", "anchor": {"x": EAST_START.x, "y": EAST_START.y}, "at": 0}
    ]);
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let mut opponent = Opponent::restore(&checkpoint, &scenario, &state, map(&scenario)).unwrap();
    let commands = opponent.act(&state, &mut OwnEvents::default());
    let trained = army(&state, &commands);
    assert!(
        trained.iter().all(|(_, kind)| {
            crate::composition::role(*kind) != Some(crate::composition::Role::AirStrike)
        }),
        "{trained:?}"
    );
}

#[test]
fn a_severed_seat_with_an_airworks_trains_air_strikes_before_it_has_an_army() {
    let mut scenario = bare_strait(1_000);
    // A scout already, so the Airworks is free for the army.
    scenario.units.push(unit(0, UnitKind::Kestrel, 5, 5));
    let state = scenario.build().unwrap();
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let trained = army(&state, &commands);
    assert!(
        trained.iter().any(|(at, kind)| {
            *at == BuildingKind::Airworks
                && crate::composition::role(*kind) == Some(crate::composition::Role::AirStrike)
        }),
        "{trained:?}"
    );
}

#[test]
fn a_payload_worth_two_lifts_flies_together_in_one() {
    let mut scenario = crowded(&MORE_SKYHOOKS);
    // A Kestrel over the strait shows an East outpost on the far shore.
    scenario.units.push(unit(0, UnitKind::Kestrel, 20, 5));
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Fabricator,
        x: 28,
        y: 4,
    });
    let state = scenario.build().unwrap();
    let trace = seat(&scenario, 0)
        .act_traced(&state, &mut OwnEvents::default())
        .1
        .unwrap();
    let lifts: Vec<MissionStatus> = trace
        .missions
        .into_iter()
        .filter(|mission| matches!(mission.kind, MissionKind::Lift { .. }))
        .collect();
    let [lift] = lifts[..] else {
        panic!("{lifts:?}");
    };
    let free_carriers = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Skyhook)
        .count();
    assert!(
        lift.units as usize >= 2 * free_carriers,
        "every carrier boards riders in the one lift: {lift:?}"
    );
}

/// West's ground cut off from East's start by a wall, East's start in a
/// walled corner.
const WALLED: [&str; 24] = [
    "########################################",
    "#..............................#.......#",
    "#..............................#.......#",
    "#..............................#.......#",
    "#.........s....................#..2....#",
    "#..............................#.......#",
    "#..ss..........................#.......#",
    "#..s...........................#.......#",
    "#..............................#.......#",
    "#..............................#########",
    "#....1.................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#..ss..................................#",
    "#..s...................................#",
    "#......................................#",
    "#.........s............................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "#......................................#",
    "########################################",
];

#[test]
fn a_severed_seat_short_of_an_army_does_not_hold_its_tech_for_a_turret() {
    let mut scenario = field();
    scenario.map = WALLED.map(str::to_owned).to_vec();
    scenario.players[0].scrap = 300;
    for y in 9..=12 {
        scenario.units.push(harvester(0, 8, y));
    }
    for y in [10, 11] {
        scenario.units.push(unit(0, UnitKind::Sentinel, 10, y));
    }
    let state = scenario.build().unwrap();
    let (_, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
    assert_eq!(
        trace.unwrap().target.map(|target| target.investment),
        Some(Investment::Tech(BuildingKind::Airworks)),
        "its army reaches the enemy only once an Airworks stands"
    );
}

/// West on the strait restored at `state` under `config`, having last seen
/// East's start at `looked` and remembering `units`.
fn recalled(
    config: BotConfig,
    state: &State,
    looked: u64,
    units: serde_json::Value,
) -> (Scenario, Opponent) {
    let mut scenario = strait();
    scenario.players[0].bot_config = Some(config);
    let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
    json["memory"]["scouted"] = serde_json::json!([looked]);
    json["memory"]["units"] = units;
    let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
    let opponent = Opponent::restore(&checkpoint, &scenario, state, map(&scenario)).unwrap();
    (scenario, opponent)
}

fn lifts(mut opponent: Opponent, state: &State) -> bool {
    let (_, trace) = opponent.act_traced(state, &mut OwnEvents::default());
    lift(&trace.unwrap().missions).is_some()
}

fn rung(difficulty: BotDifficulty) -> BotConfig {
    BotConfig::new(difficulty, BotStance::Balanced, 11)
}

#[test]
fn a_lift_waits_for_a_recent_look_at_its_target_except_at_the_lowest_rung() {
    let mut state = strait().build().unwrap();
    advance_to(&mut state, 1_200, &[]);
    let after = |difficulty, looked| {
        let (_, opponent) = recalled(rung(difficulty), &state, looked, serde_json::json!([]));
        lifts(opponent, &state)
    };
    assert!(after(BotDifficulty::Standard, 1_200), "a fresh look");
    assert!(
        !after(BotDifficulty::Standard, 0),
        "the look has gone stale"
    );
    assert!(!after(BotDifficulty::Prime, 0));
    assert!(
        after(BotDifficulty::Scrapheap, 0),
        "the lowest rung lifts blind"
    );
}

#[test]
fn an_island_army_seen_long_ago_still_holds_a_lift_back() {
    let mut state = strait().build().unwrap();
    advance_to(&mut state, 1_200, &[]);
    let after = |units| {
        let (_, opponent) = recalled(config(), &state, 1_200, units);
        lifts(opponent, &state)
    };
    assert!(after(serde_json::json!([])), "premise: the lift could go");
    let island: Vec<serde_json::Value> = (0..12)
        .map(|index| {
            serde_json::json!({
                "id": 1_000 + index,
                "kind": "sentinel",
                "tile": {"x": 30 + index % 4, "y": 9 + index / 4},
                "seen": 200,
            })
        })
        .collect();
    assert!(
        !after(island.into()),
        "twelve Sentinels seen a thousand ticks ago cannot have left their island"
    );
}

#[test]
fn a_lift_lands_out_of_reach_of_the_guns_it_knows() {
    let mut scenario = strait();
    for (x, y) in [(31, 9), (31, 13)] {
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Turret,
            x,
            y,
        });
    }
    scenario.units.push(unit(0, UnitKind::Kestrel, 34, 6));
    let mut state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert_eq!(
        observation
            .enemy_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Turret)
            .count(),
        2,
        "premise: West has seen both guns"
    );
    let guns = crate::missions::hazards(
        &observation,
        &map(&scenario),
        &crate::memory::Memory::default(),
        oxide_sim::stats::Domain::Ground,
    );
    let mut opponent = seat(&scenario, 0);
    let mut drops = Vec::new();
    while drops.is_empty() {
        assert!(state.current_tick() < 3_000, "no lift landed");
        let commands = opponent.act(&state, &mut OwnEvents::default());
        drops.extend(unloads(&commands));
        state.tick(&commands);
    }
    for (_, landing, _) in drops {
        assert!(
            !guns
                .iter()
                .any(|gun| gun.covers(crate::frame::doubled(landing))),
            "{landing:?} is under a known gun"
        );
    }
}

#[test]
fn the_upper_rungs_time_a_lift_to_bombers_clearing_its_target() {
    let plain = strait();
    let mut bombed = strait();
    bombed.units.push(unit(0, UnitKind::Condor, 12, 2));
    let mut state = bombed.build().unwrap();
    let condor = at(&state, 12, 2);
    advance_to(&mut state, 12, &[run(0, vec![condor], 30, 4)]);
    let mut alone = plain.build().unwrap();
    advance_to(&mut alone, 12, &[]);
    // Six Sentinels on East's island that West's eight alone do not
    // outweigh by any rung's margin.
    let island: Vec<serde_json::Value> = (0..6)
        .map(|index| {
            serde_json::json!({
                "id": 1_000 + index,
                "kind": "sentinel",
                "tile": {"x": 30 + index % 3, "y": 10 + index / 3},
                "seen": 12,
            })
        })
        .collect();
    let after = |difficulty, scenario: &Scenario, state: &State, missions: serde_json::Value| {
        let mut scenario = scenario.clone();
        scenario.players[0].bot_config = Some(rung(difficulty));
        let mut json = serde_json::to_value(seat(&scenario, 0).checkpoint()).unwrap();
        json["memory"]["units"] = island.clone().into();
        json["missions"] = missions;
        let checkpoint: Checkpoint = serde_json::from_value(json).unwrap();
        let opponent = Opponent::restore(&checkpoint, &scenario, state, map(&scenario)).unwrap();
        lifts(opponent, state)
    };
    let none = serde_json::json!({"next": 0, "list": []});
    let clearing = serde_json::json!({
        "next": 1,
        "list": [{
            "id": 0,
            "task": {"task": "clear", "aim": {"x": 34, "y": 9}, "phase": "travel"},
            "since": 0,
            "units": [condor],
            "goal": {"x": 34, "y": 9},
        }],
    });
    assert!(
        !after(BotDifficulty::Prime, &plain, &alone, none),
        "premise: too few alone"
    );
    for (difficulty, counts) in [
        (BotDifficulty::Prime, true),
        (BotDifficulty::Veteran, true),
        (BotDifficulty::Standard, false),
    ] {
        assert_eq!(
            after(difficulty, &bombed, &state, clearing.clone()),
            counts,
            "{difficulty:?} counting the bomber out clearing the way"
        );
    }
}

#[test]
fn a_lift_counts_the_guns_at_its_target_however_far_off_it_lands() {
    let lifts_past = |bastions: &[(i32, i32)]| {
        let mut scenario = strait();
        for (x, y) in bastions {
            scenario.buildings.push(BuildingSpec {
                player: 1,
                kind: BuildingKind::Bastion,
                x: *x,
                y: *y,
            });
        }
        scenario.units.push(unit(0, UnitKind::Kestrel, 35, 7));
        let state = scenario.build().unwrap();
        let observation = ObservationData::fog_honest(&state, PlayerId(0));
        assert_eq!(
            observation.enemy_buildings.len(),
            bastions.len() + 1,
            "premise: West sees the start and its guns"
        );
        lifts(seat(&scenario, 0), &state)
    };
    assert!(lifts_past(&[]), "premise: the start alone is beatable");
    assert!(
        !lifts_past(&[(33, 4), (36, 4)]),
        "two Bastions guarding the start outweigh the lift, though it would land beyond their reach"
    );
}
