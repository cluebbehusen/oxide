use super::*;
use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{BotStance, BuildingSpec, PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::{
    AttackTarget, BuildingId, Command, Event, Faction, Scenario, StallReason, UnitId, UnitKind,
};
use std::sync::Arc;

mod attack;
mod catalog;
mod clear;
mod composition;
mod defenses;
mod expansion;
mod layout;
mod lift;
mod missions;
mod placement;
mod raid;
mod saving;
mod scouting;
mod strike;
mod support;
mod teams;
mod workers;

/// A half-turn-symmetric arena. Each seat's Harvesters stand equally far from
/// their two nearby scrap nodes, so the split depends on the tie-break.
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
    BotConfig::new(BotDifficulty::Standard, BotStance::Balanced, 11)
}

/// A seat that seldom builds static defense before it sees a threat, for
/// stagings about its economy.
fn thrifty() -> BotConfig {
    BotConfig::new(BotDifficulty::Standard, BotStance::Aggressive, 38)
}

/// A seat that fortifies readily.
fn fortified() -> BotConfig {
    BotConfig::new(BotDifficulty::Standard, BotStance::Turtle, 35)
}

fn map(scenario: &Scenario) -> Arc<MapModel> {
    Arc::new(MapModel::from_scenario(scenario).unwrap())
}

fn seat(scenario: &Scenario, player: u8) -> Opponent {
    seat_with(scenario, player, config())
}

fn seat_with(scenario: &Scenario, player: u8, config: BotConfig) -> Opponent {
    Opponent::new(PlayerId(player), config, map(scenario))
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

/// The arena with a second west Foundry beside the first.
fn second_foundry(scrap: u32) -> Scenario {
    let mut scenario = arena(scrap);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Foundry,
        x: 3,
        y: 8,
    });
    scenario
}

/// Harvesters for `player` along its edge of the arena, enough to fill every
/// node it works there however full its stance and greed make it.
fn workforce(player: u8) -> Vec<UnitSpec> {
    (1..=10)
        .flat_map(|y| [1, 2].map(|x| (x, y)))
        .map(|(x, y)| {
            if player == 0 {
                harvester(0, x, y)
            } else {
                harvester(1, 23 - x, 11 - y)
            }
        })
        .collect()
}

/// Sentinels for `player` along its edge of the arena, worth the Balanced
/// stance's minimum army, so the seat is not exposed: it neither puts a
/// Turret before tech nor holds Harvesters back for its army.
fn standing_army(player: u8) -> Vec<UnitSpec> {
    (3..=9)
        .map(|x| {
            if player == 0 {
                unit(0, UnitKind::Sentinel, x, 10)
            } else {
                unit(1, UnitKind::Sentinel, 23 - x, 1)
            }
        })
        .collect()
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

fn harvests(commands: &[PlayerCommand]) -> Vec<(TilePos, Vec<UnitId>)> {
    commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Harvest { units, node, .. } => Some((*node, units.clone())),
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

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

fn at(state: &State, x: i32, y: i32) -> UnitId {
    state
        .units()
        .iter()
        .find(|unit| unit.tile() == TilePos::new(x, y))
        .unwrap()
        .id
}

fn hunts(commands: &[PlayerCommand]) -> Vec<(Vec<UnitId>, TilePos)> {
    commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Hunt { units, goal, .. } => Some((units.clone(), *goal)),
            _ => None,
        })
        .collect()
}

fn advances(commands: &[PlayerCommand]) -> Vec<(Vec<UnitId>, TilePos)> {
    commands
        .iter()
        .filter_map(|command| match &command.command {
            Command::Advance { units, goal, .. } => Some((units.clone(), *goal)),
            _ => None,
        })
        .collect()
}

fn run(player: u8, units: Vec<UnitId>, x: i32, y: i32) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command: Command::Run {
            units,
            goal: TilePos::new(x, y),
            queue: false,
        },
    }
}

/// Plays the west seat against scripted east commands until `until`,
/// returning every west trace.
fn play(
    opponent: &mut Opponent,
    state: &mut State,
    until: u64,
    script: &[(u64, PlayerCommand)],
) -> Vec<Trace> {
    let mut traces = Vec::new();
    while state.current_tick() < until {
        let now = state.current_tick();
        let (mut commands, trace) = opponent.act_traced(state, &mut OwnEvents::default());
        traces.extend(trace);
        commands.extend(
            script
                .iter()
                .filter(|(tick, _)| *tick == now)
                .map(|(_, command)| command.clone()),
        );
        state.tick(&commands);
    }
    traces
}

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

/// Sentinel spots beside the West Foundry on the field, nearer home than any
/// other staged unit: worth the Balanced stance's home reserve, so offense
/// leaves them there.
const GARRISON: [(i32, i32); 4] = [(2, 11), (2, 12), (2, 13), (4, 14)];

/// The field tile, on its southern rows clear of both starts and the
/// garrison, for the `index`th unit of a full staged mission list.
fn cap_spot(index: usize) -> (i32, i32) {
    (
        2 + i32::try_from(index % 32).unwrap(),
        17 + i32::try_from(index / 32).unwrap(),
    )
}

/// Adds West's garrison to the field.
fn garrison(scenario: &mut Scenario) {
    for (x, y) in GARRISON {
        scenario.units.push(unit(0, UnitKind::Sentinel, x, y));
    }
}

/// The field with both seats and no units.
fn field() -> Scenario {
    let mut scenario = arena(0);
    scenario.map = FIELD.map(str::to_owned).to_vec();
    scenario.units.clear();
    scenario
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

/// The east seat's counterpart of west `commands` on a half-turn-symmetric
/// staging: units by their rank among the seat's units, buildings by rank
/// among its Foundries, and tiles rotated.
fn mirror(state: &State, commands: Vec<PlayerCommand>) -> Vec<PlayerCommand> {
    let sorted = |player: u8| {
        let mut units = seat_units(state, PlayerId(player));
        units.sort_unstable();
        units
    };
    let (west_units, east_units) = (sorted(0), sorted(1));
    let units = |units: Vec<UnitId>| {
        let mut units: Vec<UnitId> = units
            .into_iter()
            .map(|unit| east_units[west_units.iter().position(|id| *id == unit).unwrap()])
            .collect();
        units.sort_unstable();
        units
    };
    let (width, height) = (state.map().width(), state.map().height());
    let rotate = |tile: TilePos| TilePos::new(width - 1 - tile.x, height - 1 - tile.y);
    commands
        .into_iter()
        .map(|command| PlayerCommand {
            player: PlayerId(1),
            command: match command.command {
                Command::Harvest {
                    units: sent,
                    node,
                    queue,
                } => Command::Harvest {
                    units: units(sent),
                    node: rotate(node),
                    queue,
                },
                Command::Hunt {
                    units: sent,
                    goal,
                    queue,
                } => Command::Hunt {
                    units: units(sent),
                    goal: rotate(goal),
                    queue,
                },
                Command::Run {
                    units: sent,
                    goal,
                    queue,
                } => Command::Run {
                    units: units(sent),
                    goal: rotate(goal),
                    queue,
                },
                Command::Advance {
                    units: sent,
                    goal,
                    queue,
                } => Command::Advance {
                    units: units(sent),
                    goal: rotate(goal),
                    queue,
                },
                Command::Attack {
                    units: sent,
                    target: AttackTarget::Unit(enemy),
                    queue,
                } => Command::Attack {
                    units: units(sent),
                    target: AttackTarget::Unit(
                        west_units[east_units.iter().position(|id| *id == enemy).unwrap()],
                    ),
                    queue,
                },
                Command::Load {
                    units: sent,
                    transport,
                    queue,
                } => Command::Load {
                    units: units(sent),
                    transport: units(vec![transport])[0],
                    queue,
                },
                Command::Unload {
                    transport,
                    at,
                    queue,
                } => Command::Unload {
                    transport: units(vec![transport])[0],
                    at: rotate(at),
                    queue,
                },
                Command::Build {
                    units: sent,
                    kind,
                    anchor,
                    queue,
                    defer,
                } => {
                    let (w, h) = kind.base_stats().size;
                    Command::Build {
                        units: units(sent),
                        kind,
                        anchor: TilePos::new(width - w - anchor.x, height - h - anchor.y),
                        queue,
                        defer,
                    }
                }
                Command::Train { building, kind } => {
                    let west = foundries(state, PlayerId(0));
                    let index = west.iter().position(|id| *id == building).unwrap();
                    Command::Train {
                        building: foundries(state, PlayerId(1))[index],
                        kind,
                    }
                }
                other => panic!("this staging never issues {other:?}"),
            },
        })
        .collect()
}

fn advance_to(state: &mut State, tick: u64, commands: &[PlayerCommand]) {
    state.tick(commands);
    while state.current_tick() < tick {
        state.tick(&[]);
    }
}

#[test]
fn a_staged_foundry_spreads_its_harvesters_and_trains_its_army_first() {
    let scenario = arena(200);
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
    assert_eq!(opponent.player(), PlayerId(0));
    assert_eq!(opponent.profile(), &ResolvedProfile::resolve(config()));

    let (commands, trace) = opponent.act_traced(&state, &mut OwnEvents::default());
    let mut harvesters: Vec<UnitId> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
        .map(|unit| unit.id)
        .collect();
    harvesters.sort_unstable();
    let foundry = foundries(&state, PlayerId(0))[0];
    let mut worked = harvests(&commands);
    worked.sort_unstable_by_key(|(node, _)| (node.y, node.x));
    assert_eq!(
        worked
            .iter()
            .map(|(node, units)| (*node, units.len()))
            .collect::<Vec<_>>(),
        [(TilePos::new(7, 3), 1), (TilePos::new(7, 9), 1)],
        "one Harvester on each nearby node"
    );
    let mut sent: Vec<UnitId> = worked.into_iter().flat_map(|(_, units)| units).collect();
    sent.sort_unstable();
    assert_eq!(sent, harvesters);
    assert_eq!(
        trains(&commands),
        [(foundry, UnitKind::Sentinel)],
        "with no army, Harvesters that would cost more than it wait for it"
    );
    assert_eq!(
        trace,
        Some(Trace {
            tick: 0,
            player: PlayerId(0),
            bank: 200,
            events: Vec::new(),
            spent: UnitKind::Sentinel.stats().cost,
            purchases: vec![Purchase::Train {
                building: foundry,
                unit: UnitKind::Sentinel,
            }],
            unit_orders: 2,
            allowance: 6,
            target: None,
            protected: 0,
            missions: Vec::new(),
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
    let scenario = arena(0);
    let state = scenario.build().unwrap();
    let (commands, trace) = seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default());
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
    let scenario = second_foundry(UnitKind::Harvester.stats().cost + 10);
    let state = scenario.build().unwrap();
    assert_eq!(foundries(&state, PlayerId(0)).len(), 2);
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert_eq!(
        trains(&commands),
        [(foundries(&state, PlayerId(0))[0], UnitKind::Harvester)]
    );
}

#[test]
fn ready_foundries_train_harvesters_while_their_nodes_want_more() {
    let mut scenario = second_foundry(1_000);
    scenario.units.extend(standing_army(0));
    let state = scenario.build().unwrap();
    let homes = foundries(&state, PlayerId(0));
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert_eq!(
        trains(&commands),
        [
            (homes[0], UnitKind::Harvester),
            (homes[1], UnitKind::Harvester)
        ]
    );
}

#[test]
fn saturated_harvesting_leaves_idle_foundries_to_production() {
    let mut scenario = second_foundry(1_000);
    scenario.units.extend(workforce(0));
    let state = scenario.build().unwrap();
    let homes = foundries(&state, PlayerId(0));
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert_eq!(
        trains(&commands),
        [
            (homes[0], UnitKind::Sentinel),
            (homes[1], UnitKind::Sentinel)
        ]
    );
}

#[test]
fn a_seat_without_harvesters_queues_one_behind_other_work() {
    let mut scenario = arena(300);
    scenario.units.retain(|unit| unit.player != 0);
    let mut state = scenario.build().unwrap();
    let foundry = foundries(&state, PlayerId(0))[0];
    let busy = PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: foundry,
            kind: UnitKind::Sentinel,
        },
    };
    advance_to(&mut state, 12, &[busy]);
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert_eq!(observation.my_queues[0], [UnitKind::Sentinel], "premise");
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert_eq!(trains(&commands), [(foundry, UnitKind::Harvester)]);
}

#[test]
fn the_allowance_caps_unit_orders_but_not_purchases() {
    let mut scenario = arena(1_000);
    for row in [2, 8] {
        scenario.map[row].replace_range(2..3, "s");
    }
    scenario.units.extend((4..=5).map(|x| harvester(0, x, 7)));
    let state = scenario.build().unwrap();
    let observation = ObservationData::fog_honest(&state, PlayerId(0));
    assert!(
        [(2, 2), (7, 3), (2, 8), (7, 9)]
            .into_iter()
            .all(|(x, y)| observation
                .known_scrap
                .iter()
                .any(|(node, _)| *node == TilePos::new(x, y))),
        "premise: four nodes near the west Foundry"
    );
    let scrapheap = BotConfig::new(BotDifficulty::Scrapheap, BotStance::Balanced, 11);
    let (commands, trace) =
        seat_with(&scenario, 0, scrapheap).act_traced(&state, &mut OwnEvents::default());
    let trace = trace.unwrap();
    assert!(
        trace.allowance < 4,
        "premise: fewer orders than idle workers"
    );
    assert_eq!(
        u32::try_from(harvests(&commands).len()).unwrap(),
        trace.allowance
    );
    assert_eq!(trains(&commands).len(), 1, "a purchase still happens");
    assert_eq!(trace.unit_orders, trace.allowance);
}

#[test]
fn decisions_follow_the_difficulty_interval_and_stop_after_the_result() {
    let scenario = arena(200);
    let mut state = scenario.build().unwrap();
    let mut standard = seat(&scenario, 0);
    let scrapheap = seat_with(
        &scenario,
        0,
        BotConfig::new(BotDifficulty::Scrapheap, BotStance::Balanced, 11),
    );
    let prime = seat_with(
        &scenario,
        0,
        BotConfig::new(BotDifficulty::Prime, BotStance::Balanced, 11),
    );
    assert!(standard.decision_due(&state) && scrapheap.decision_due(&state));
    advance_to(&mut state, 6, &[]);
    assert!(prime.decision_due(&state));
    assert!(!standard.decision_due(&state));
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
    let mut resigned = seat(&scenario, 0);
    assert!(resigned.decision_due(&state));
    assert_eq!(
        resigned.act_traced(&state, &mut OwnEvents::default()),
        (Vec::new(), None)
    );
    assert!(
        !seat(&scenario, 1)
            .act(&state, &mut OwnEvents::default())
            .is_empty()
    );

    for row in &mut scenario.map {
        *row = row.replace('1', ".");
    }
    let state = scenario.build().unwrap();
    assert!(foundries(&state, PlayerId(0)).is_empty());
    assert_eq!(
        seat(&scenario, 0).act_traced(&state, &mut OwnEvents::default()),
        (Vec::new(), None)
    );
}

#[test]
fn a_decision_receives_only_its_seats_failures_since_the_last_one() {
    let mut scenario = arena(0);
    // The sealed goal's nearest reachable tile is the Harvester's own, so its
    // walk ends short on the tick it is ordered, inside the decision window.
    for (row, col, walls) in [
        (4, 9, "#"),
        (5, 8, "###"),
        (6, 8, "#.##"),
        (7, 8, "###"),
        (8, 9, "#"),
    ] {
        scenario.map[row].replace_range(col..col + walls.len(), walls);
    }
    let mut state = scenario.build().unwrap();
    let mut opponent = seat(&scenario, 0);
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
            command: Command::Run {
                units: vec![harvester],
                goal: TilePos::new(9, 6),
                queue: false,
            },
        },
    ];
    tick(&mut state, &mut events, &staged);
    while state.current_tick() < 48 {
        if !opponent.decision_due(&state) {
            assert_eq!(opponent.act_traced(&state, &mut events), (Vec::new(), None));
        }
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
        seat(&scenario, 0)
            .act(&state, &mut OwnEvents::default())
            .iter()
            .all(|command| !matches!(command.command, Command::Harvest { .. }))
    );
}

#[test]
fn mirrored_seats_issue_mirrored_commands() {
    let scenario = arena(200);
    let state = scenario.build().unwrap();
    let west = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let east = seat(&scenario, 1).act(&state, &mut OwnEvents::default());
    assert!(!west.is_empty());
    assert_eq!(mirror(&state, west), east);
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
        let mut seats = [seat(&scenario, 0), seat(&scenario, 1)];
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
    let opponent = seat(&scenario, 1);
    let checkpoint = opponent.checkpoint();
    let json = serde_json::to_value(&checkpoint).unwrap();
    assert_eq!(json["player"], 1);
    let decoded: Checkpoint = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(decoded, checkpoint);
    let mut restored = Opponent::restore(&decoded, &scenario, &state, map(&scenario)).unwrap();
    assert_eq!(restored.player(), opponent.player());
    assert_eq!(restored.profile(), opponent.profile());
    assert_eq!(
        restored.act(&state, &mut OwnEvents::default()),
        opponent.clone().act(&state, &mut OwnEvents::default())
    );
    let mut unknown = json;
    unknown["plans"] = serde_json::json!([]);
    assert!(serde_json::from_value::<Checkpoint>(unknown).is_err());

    let model = map(&scenario);
    let rejected = |scenario: &Scenario, player: u8| {
        Opponent::restore(
            &Checkpoint {
                player: PlayerId(player),
                ..checkpoint.clone()
            },
            scenario,
            &state,
            Arc::clone(&model),
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
}

#[test]
fn a_producer_about_to_finish_queues_its_next_unit_now() {
    let mut scenario = second_foundry(1_000);
    scenario.units.extend(workforce(0));
    let mut state = scenario.build().unwrap();
    let homes = foundries(&state, PlayerId(0));
    let busy: Vec<PlayerCommand> = homes
        .iter()
        .map(|building| PlayerCommand {
            player: PlayerId(0),
            command: Command::Train {
                building: *building,
                kind: UnitKind::Sentinel,
            },
        })
        .collect();
    let finishes = u64::from(UnitKind::Sentinel.stats().train_ticks);
    // The last decision before the Sentinels finish, and the one before it.
    let last = (finishes - 1) / 12 * 12;
    advance_to(&mut state, last - 12, &busy);
    let early = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert!(trains(&early).is_empty(), "{early:?}");
    advance_to(&mut state, last, &[]);
    let late = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert_eq!(
        trains(&late),
        [
            (homes[0], UnitKind::Sentinel),
            (homes[1], UnitKind::Sentinel)
        ],
        "each queues its next unit before it would stand empty"
    );
}

#[test]
fn harvesters_that_outprice_the_army_take_only_what_it_leaves() {
    // Scrap for a Harvester but not a Sentinel: the Foundry the army could
    // not use still trains a Harvester.
    let scenario = arena(60);
    let state = scenario.build().unwrap();
    let foundry = foundries(&state, PlayerId(0))[0];
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    assert_eq!(trains(&commands), [(foundry, UnitKind::Harvester)]);
}

#[test]
fn workers_bought_before_production_never_cost_more_than_the_army() {
    // Two Sentinels against two Harvesters leave the price of one more
    // Harvester before production, not one at each Foundry; production then
    // spends the rest.
    let mut scenario = second_foundry(140);
    scenario.units.push(unit(0, UnitKind::Sentinel, 9, 9));
    scenario.units.push(unit(0, UnitKind::Sentinel, 9, 10));
    let state = scenario.build().unwrap();
    let commands = seat(&scenario, 0).act(&state, &mut OwnEvents::default());
    let workers = trains(&commands)
        .into_iter()
        .filter(|(_, kind)| *kind == UnitKind::Harvester)
        .count();
    assert_eq!(workers, 1, "{commands:?}");
}

#[test]
fn a_seat_without_an_army_puts_up_a_turret_before_tech() {
    let target = |scenario: &Scenario| {
        let state = scenario.build().unwrap();
        seat_with(scenario, 0, thrifty())
            .act_traced(&state, &mut OwnEvents::default())
            .1
            .unwrap()
            .target
            .map(|target| target.investment)
    };
    let mut exposed = arena(0);
    exposed.units.extend(workforce(0));
    assert!(
        matches!(
            target(&exposed),
            Some(Investment::Defense {
                kind: BuildingKind::Turret,
                ..
            })
        ),
        "{:?}",
        target(&exposed)
    );
    let mut armed = exposed.clone();
    armed.units.extend(standing_army(0));
    assert!(
        matches!(target(&armed), Some(Investment::Tech(_))),
        "an army to speak of saves for tech: {:?}",
        target(&armed)
    );
}
