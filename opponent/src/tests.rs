use super::*;
use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{
    BotController, BotStance, BuildingSpec, PlayerSpec, ScenarioMode, UnitSpec,
};
use oxide_sim::{BuildingId, Command, Event, Faction, Scenario, StallReason, UnitId, UnitKind};

/// A half-turn-symmetric arena. Each seat's Harvester stands equally far from
/// its two nearby scrap nodes, so the choice depends on the tie-break.
const ARENA: [&str; 12] = [
    "########################",
    "#......................#",
    "#...............s......#",
    "#......s...............#",
    "#......................#",
    "#..1...............2...#",
    "#......................#",
    "#......................#",
    "#...............s......#",
    "#......s...............#",
    "#......................#",
    "########################",
];

fn config() -> BotConfig {
    BotConfig::opponent(BotDifficulty::Standard, BotStance::Balanced, 11)
}

fn arena(scrap: u32) -> Scenario {
    let seat = |name: &str, faction| PlayerSpec {
        name: name.into(),
        faction,
        team: None,
        scrap,
        bot: true,
        bot_config: Some(config()),
    };
    Scenario {
        mode: ScenarioMode::Match,
        name: "opponent arena".into(),
        seed: 5,
        map: ARENA.map(str::to_owned).to_vec(),
        players: vec![
            seat("west", Faction::Ferrous),
            seat("east", Faction::Cupric),
        ],
        units: vec![
            harvester(0, 7, 6),
            harvester(0, 6, 6),
            harvester(1, 16, 5),
            harvester(1, 17, 5),
        ],
        buildings: Vec::new(),
        meta: None,
    }
}

fn harvester(player: u8, x: i32, y: i32) -> UnitSpec {
    UnitSpec {
        player,
        kind: UnitKind::Harvester,
        x,
        y,
    }
}

fn seat_units(state: &State, player: PlayerId) -> Vec<UnitId> {
    state
        .units()
        .iter()
        .filter(|unit| unit.player == player)
        .map(|unit| unit.id)
        .collect()
}

fn foundries(state: &State, player: PlayerId) -> Vec<BuildingId> {
    state
        .buildings()
        .iter()
        .filter(|building| building.player == player && building.kind == BuildingKind::Foundry)
        .map(|building| building.id)
        .collect()
}

fn trains(commands: &[PlayerCommand]) -> Vec<(BuildingId, UnitKind)> {
    commands
        .iter()
        .filter_map(|command| match command.command {
            Command::Train { building, kind } => Some((building, kind)),
            _ => None,
        })
        .collect()
}

fn surrender(player: u8) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command: Command::Surrender,
    }
}

fn advance_to(state: &mut State, tick: u64, commands: &[PlayerCommand]) {
    state.tick(commands);
    while state.current_tick() < tick {
        state.tick(&[]);
    }
}

#[test]
fn a_staged_foundry_harvests_and_trains() {
    let mut state = arena(200).build().unwrap();
    let mut opponent = Opponent::new(PlayerId(0), config());
    assert_eq!(opponent.player(), PlayerId(0));
    assert_eq!(opponent.profile(), &ResolvedProfile::resolve(config()));

    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let mut harvesters = seat_units(&state, PlayerId(0));
    harvesters.sort_unstable();
    let foundry = foundries(&state, PlayerId(0))[0];
    assert_eq!(
        commands,
        [
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Harvest {
                    units: harvesters,
                    node: TilePos::new(7, 9),
                    queue: false,
                },
            },
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Train {
                    building: foundry,
                    kind: UnitKind::Harvester,
                },
            },
        ]
    );
    assert_eq!(
        trace,
        Some(Trace {
            tick: 0,
            player: PlayerId(0),
            bank: 200,
            events: Vec::new(),
            spent: UnitKind::Harvester.stats().cost,
            purchases: vec![Purchase {
                building: foundry,
                kind: UnitKind::Harvester,
            }],
            unit_orders: 1,
        })
    );
    let report = state.tick(&commands);
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. })),
        "{:?}",
        report.events
    );
    assert_eq!(
        opponent.act(&state, &mut OwnEvents::default()),
        Vec::new(),
        "off-cadence tick"
    );
}

#[test]
fn an_empty_bank_harvests_without_training() {
    let state = arena(0).build().unwrap();
    let (commands, trace) =
        Opponent::new(PlayerId(0), config()).act_traced(&state, &mut OwnEvents::default());
    assert!(trains(&commands).is_empty());
    assert!(
        commands
            .iter()
            .all(|command| matches!(command.command, Command::Harvest { .. }))
    );
    let trace = trace.unwrap();
    assert_eq!((trace.bank, trace.spent), (0, 0));
    assert!(trace.purchases.is_empty());
}

#[test]
fn the_running_total_pays_for_one_unit_across_two_foundries() {
    let mut scenario = arena(UnitKind::Harvester.stats().cost + 10);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 3,
        y: 8,
    });
    let state = scenario.build().unwrap();
    assert_eq!(foundries(&state, PlayerId(0)).len(), 2);
    let commands = Opponent::new(PlayerId(0), config()).act(&state, &mut OwnEvents::default());
    assert_eq!(
        trains(&commands),
        [(foundries(&state, PlayerId(0))[0], UnitKind::Harvester)]
    );
}

#[test]
fn harvesters_fill_six_per_foundry_before_sentinels() {
    let mut scenario = arena(1_000);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 3,
        y: 8,
    });
    scenario.units.extend((1..=9).map(|x| harvester(0, x, 10)));
    let state = scenario.build().unwrap();
    let homes = foundries(&state, PlayerId(0));
    let commands = Opponent::new(PlayerId(0), config()).act(&state, &mut OwnEvents::default());
    assert_eq!(
        trains(&commands),
        [
            (homes[0], UnitKind::Harvester),
            (homes[1], UnitKind::Sentinel)
        ],
        "eleven Harvesters plus one queued fill two Foundries"
    );
}

#[test]
fn decisions_follow_the_difficulty_interval_and_stop_after_the_result() {
    let scenario = arena(200);
    let mut state = scenario.build().unwrap();
    let mut standard = Opponent::new(PlayerId(0), config());
    let scrapheap = Opponent::new(
        PlayerId(0),
        BotConfig::opponent(BotDifficulty::Scrapheap, BotStance::Balanced, 11),
    );
    assert!(standard.decision_due(&state) && scrapheap.decision_due(&state));
    advance_to(&mut state, 12, &[]);
    assert!(standard.decision_due(&state));
    assert!(!scrapheap.decision_due(&state));
    advance_to(&mut state, 24, &[]);
    assert!(scrapheap.decision_due(&state));

    advance_to(&mut state, 36, &[surrender(1)]);
    assert!(state.result().is_some());
    assert!(!standard.decision_due(&state));
    assert_eq!(
        standard.act_traced(&state, &mut OwnEvents::default()),
        (Vec::new(), None)
    );
}

#[test]
fn a_surrendered_or_foundry_less_seat_stays_silent() {
    let mut scenario = arena(200);
    scenario.mode = ScenarioMode::Sandbox;
    let mut state = scenario.build().unwrap();
    advance_to(&mut state, 12, &[surrender(0)]);
    assert!(state.result().is_none() && state.player(PlayerId(0)).resigned);
    let mut resigned = Opponent::new(PlayerId(0), config());
    assert!(resigned.decision_due(&state));
    assert_eq!(
        resigned.act_traced(&state, &mut OwnEvents::default()),
        (Vec::new(), None)
    );
    assert!(
        !Opponent::new(PlayerId(1), config())
            .act(&state, &mut OwnEvents::default())
            .is_empty()
    );

    for row in &mut scenario.map {
        *row = row.replace('1', ".");
    }
    let state = scenario.build().unwrap();
    assert!(foundries(&state, PlayerId(0)).is_empty());
    assert_eq!(
        Opponent::new(PlayerId(0), config()).act_traced(&state, &mut OwnEvents::default()),
        (Vec::new(), None)
    );
}

#[test]
fn a_decision_receives_only_its_seats_failures_since_the_last_one() {
    let mut scenario = arena(0);
    for (row, walls) in [(1, "###"), (2, "#.#"), (3, "###")] {
        scenario.map[row].replace_range(10..13, walls);
    }
    let mut state = scenario.build().unwrap();
    let mut opponent = Opponent::new(PlayerId(0), config());
    let mut events = OwnEvents::default();
    let tick = |state: &mut State, events: &mut OwnEvents, commands: &[PlayerCommand]| {
        events.record(PlayerId(0), &state.tick(commands).events);
    };
    let commands = opponent.act(&state, &mut events);
    tick(&mut state, &mut events, &commands);

    let harvester = seat_units(&state, PlayerId(0))[0];
    let train = |player: u8, building: BuildingId| PlayerCommand {
        player: PlayerId(player),
        command: Command::Train {
            building,
            kind: UnitKind::Harvester,
        },
    };
    let staged = [
        train(1, foundries(&state, PlayerId(1))[0]),
        train(0, foundries(&state, PlayerId(0))[0]),
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Move {
                units: vec![harvester],
                goal: TilePos::new(11, 2),
                queue: false,
            },
        },
    ];
    tick(&mut state, &mut events, &staged);
    while state.current_tick() < 12 {
        assert_eq!(opponent.act_traced(&state, &mut events), (Vec::new(), None));
        tick(&mut state, &mut events, &[]);
    }

    let received = opponent.act_traced(&state, &mut events).1.unwrap().events;
    assert!(
        matches!(
            received.as_slice(),
            [
                OwnEvent::CommandRejected {
                    reason: RejectReason::NotEnoughScrap
                },
                OwnEvent::OrderStalled {
                    unit,
                    reason: StallReason::NoRoute,
                    ..
                },
            ] if *unit == harvester
        ),
        "{received:?}"
    );
    assert_eq!(events, OwnEvents::default());
}

#[test]
fn unexplored_scrap_is_never_targeted() {
    let mut scenario = arena(0);
    for y in [3, 9] {
        scenario.map[y].replace_range(7..8, ".");
    }
    let state = scenario.build().unwrap();
    let fogged = ObservationData::fog_honest(&state, PlayerId(0));
    let omniscient = ObservationData::omniscient(&state, PlayerId(0));
    assert!(fogged.known_scrap.is_empty());
    assert_eq!(
        omniscient.known_scrap.len(),
        2,
        "premise: distant scrap exists"
    );
    assert!(
        omniscient
            .known_scrap
            .iter()
            .all(|(node, _)| !fogged.explored(*node))
    );
    assert!(
        Opponent::new(PlayerId(0), config())
            .act(&state, &mut OwnEvents::default())
            .iter()
            .all(|command| !matches!(command.command, Command::Harvest { .. }))
    );
}

#[test]
fn mirrored_seats_issue_mirrored_commands() {
    let state = arena(200).build().unwrap();
    let west = Opponent::new(PlayerId(0), config()).act(&state, &mut OwnEvents::default());
    let east = Opponent::new(PlayerId(1), config()).act(&state, &mut OwnEvents::default());
    assert!(!west.is_empty());

    let rank = |player: PlayerId, unit: UnitId| {
        let mut units = seat_units(&state, player);
        units.sort_unstable();
        units.iter().position(|id| *id == unit).unwrap()
    };
    let east_units = {
        let mut units = seat_units(&state, PlayerId(1));
        units.sort_unstable();
        units
    };
    let (width, height) = (state.map().width(), state.map().height());
    let mirrored: Vec<PlayerCommand> = west
        .into_iter()
        .map(|command| PlayerCommand {
            player: PlayerId(1),
            command: match command.command {
                Command::Harvest { units, node, queue } => {
                    let mut units: Vec<UnitId> = units
                        .into_iter()
                        .map(|unit| east_units[rank(PlayerId(0), unit)])
                        .collect();
                    units.sort_unstable();
                    Command::Harvest {
                        units,
                        node: TilePos::new(width - 1 - node.x, height - 1 - node.y),
                        queue,
                    }
                }
                Command::Train { building, kind } => {
                    let west = foundries(&state, PlayerId(0));
                    let index = west.iter().position(|id| *id == building).unwrap();
                    Command::Train {
                        building: foundries(&state, PlayerId(1))[index],
                        kind,
                    }
                }
                other => panic!("the stub never issues {other:?}"),
            },
        })
        .collect();
    assert_eq!(mirrored, east);
}

#[test]
fn identical_runs_and_a_mid_game_clone_repeat_commands_and_hash() {
    let mut scenario = Scenario::skirmish();
    for seat in &mut scenario.players {
        seat.bot = true;
        seat.bot_config = Some(config());
    }
    let run = |ticks: u64| {
        let mut state = scenario.build().unwrap();
        let mut seats = [
            Opponent::new(PlayerId(0), config()),
            Opponent::new(PlayerId(1), config()),
        ];
        let mut history = Vec::new();
        for _ in 0..ticks {
            let commands: Vec<_> = seats
                .iter_mut()
                .flat_map(|seat| seat.act(&state, &mut OwnEvents::default()))
                .collect();
            history.extend(commands.iter().cloned());
            state.tick(&commands);
        }
        (state, seats, history)
    };

    let (first, _, first_history) = run(600);
    let (second, _, second_history) = run(600);
    assert_eq!(first.hash(), second.hash());
    assert_eq!(first_history, second_history);
    assert!(
        first_history
            .iter()
            .any(|command| matches!(command.command, Command::Train { .. }))
    );

    let (mut state, mut seats, _) = run(300);
    let mut clone_state = state.clone();
    let mut clone_seats = seats.clone();
    for _ in 0..300 {
        let commands: Vec<_> = seats
            .iter_mut()
            .flat_map(|seat| seat.act(&state, &mut OwnEvents::default()))
            .collect();
        let cloned: Vec<_> = clone_seats
            .iter_mut()
            .flat_map(|seat| seat.act(&clone_state, &mut OwnEvents::default()))
            .collect();
        assert_eq!(commands, cloned);
        state.tick(&commands);
        clone_state.tick(&cloned);
    }
    assert_eq!(state.hash(), clone_state.hash());
    assert_eq!(state.hash(), first.hash());
}

#[test]
fn checkpoints_round_trip_and_restore_only_opponent_seats() {
    let scenario = arena(200);
    let state = scenario.build().unwrap();
    let opponent = Opponent::new(PlayerId(1), config());
    let checkpoint = opponent.checkpoint();
    let json = serde_json::to_string(&checkpoint).unwrap();
    assert_eq!(json, r#"{"player":1}"#);
    let decoded: Checkpoint = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, checkpoint);
    let mut restored = Opponent::restore(&decoded, &scenario, &state).unwrap();
    assert_eq!(restored.player(), opponent.player());
    assert_eq!(restored.profile(), opponent.profile());
    assert_eq!(
        restored.act(&state, &mut OwnEvents::default()),
        opponent.clone().act(&state, &mut OwnEvents::default())
    );
    assert!(serde_json::from_str::<Checkpoint>(r#"{"player":1,"memory":[]}"#).is_err());

    let rejected = |scenario: &Scenario, player: u8| {
        Opponent::restore(
            &Checkpoint {
                player: PlayerId(player),
            },
            scenario,
            &state,
        )
        .err()
        .unwrap()
    };
    assert_eq!(rejected(&scenario, 2), "invalid controller seat");
    let mut extra = scenario.clone();
    extra.players.push(extra.players[1].clone());
    assert_eq!(
        rejected(&extra, 2),
        "invalid controller seat",
        "the world must hold the seat too"
    );
    let mut human = scenario.clone();
    human.players[1].bot = false;
    assert_eq!(
        rejected(&human, 1),
        "checkpoint seat is not a configured bot"
    );
    let mut empty = scenario.clone();
    empty.players[1].bot_config = None;
    assert_eq!(
        rejected(&empty, 1),
        "checkpoint seat is not a configured bot"
    );
    let mut scripted = scenario.clone();
    scripted.players[1].bot_config = Some(BotConfig {
        controller: BotController::Scripted,
        ..config()
    });
    assert_eq!(
        rejected(&scripted, 1),
        "checkpoint seat is not an oxide-opponent seat"
    );
}
