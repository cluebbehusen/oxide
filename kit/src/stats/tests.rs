use super::*;
use crate::runner;
use oxide_sim::Scenario;
use oxide_sim::scenario::ScenarioMode;

fn record_activity(ticks: u64) -> GameReplay {
    use chassis::grid::TilePos;
    use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
    use oxide_sim::{BuildingKind, Command, Event, Faction, PlayerCommand, Target, UnitKind};
    let mut map = vec![vec!['.'; 30]; 20];
    map[1][1] = '1';
    map[17][27] = '2';
    map[3][5] = 's';
    let scenario = Scenario {
        mode: ScenarioMode::Match,
        name: "statistics activity".into(),
        seed: 42,
        map: map
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: [Faction::Ferrous, Faction::Cupric]
            .into_iter()
            .map(|faction| PlayerSpec {
                name: format!("{faction:?}"),
                faction,
                team: None,
                scrap: 2000,
                bot: false,
                bot_config: None,
            })
            .collect(),
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 4,
                y: 3,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 4,
                y: 7,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 12,
                y: 12,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 14,
                y: 12,
            },
        ],
        buildings: Vec::<BuildingSpec>::new(),
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let units: Vec<_> = state.units().iter().map(|unit| unit.id).collect();
    let foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap()
        .id;
    let opening: Vec<_> = [
        Command::Harvest {
            units: vec![units[0]],
            node: TilePos::new(5, 3),
            queue: false,
        },
        Command::Build {
            units: vec![units[1]],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(5, 7),
            queue: false,
            defer: false,
        },
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
        Command::Attack {
            units: vec![units[2]],
            target: Target::Unit(units[3]).into(),
            queue: false,
        },
    ]
    .into_iter()
    .map(|command| PlayerCommand {
        player: PlayerId(0),
        command,
    })
    .collect();
    let mut replay = GameReplay::new(oxide_sim::SIM_VERSION, scenario);
    let mut events = Vec::new();
    for tick in 0..ticks {
        let commands = if tick == 0 { opening.as_slice() } else { &[] };
        for command in commands {
            replay.record(tick, command.clone());
        }
        let report = state.tick(commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, Event::CommandRejected { .. }))
        );
        events.extend(report.events);
    }
    if ticks >= 600 {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::ScrapDeposited { amount, .. } if *amount > 0))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::UnitTrained { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::BuildingCompleted { .. }))
        );
        assert!(events.iter().any(|e| matches!(e, Event::UnitDied { .. })));
    }
    replay.meta.ticks = Some(ticks);
    replay
}

fn track_replay(replay: &GameReplay) -> MatchStats {
    let total = replay.meta.ticks.expect("recorded duration");
    let mut state = replay.setup.build().expect("scenario builds");
    let mut cursor = replay.cursor();
    let mut live = LiveMatchStats::new(&state);
    for tick in 0..total {
        let commands: Vec<_> = cursor
            .take_tick(tick)
            .iter()
            .map(|timed| timed.command.clone())
            .collect();
        let report = state.tick(&commands);
        live.observe(&state, &report.events);
    }
    live.snapshot(&state)
}

#[test]
fn a_claimed_billion_ticks_is_an_error_not_a_hang() {
    let mut scenario = Scenario::skirmish();
    crate::bench::all_bots(&mut scenario);
    let outcome = runner::run_scenario(&scenario, 60, true, true).unwrap();
    let mut replay = outcome.replay.unwrap();
    replay.meta.ticks = Some(1_000_000_000);
    assert!(compute(&replay, 100).is_err());
}

#[test]
fn the_final_state_is_always_sampled() {
    let mut scenario = Scenario::skirmish();
    crate::bench::all_bots(&mut scenario);
    // 100 ticks with stride 41: without the closing sample the last
    // column would sit at tick 82 and closing numbers would be stale.
    let outcome = runner::run_scenario(&scenario, 100, true, true).unwrap();
    let stats = compute(&outcome.replay.unwrap(), 41).unwrap();
    assert_eq!(stats.sample_ticks.last(), Some(&100));
    assert_eq!(stats.final_tick, 100);
}

#[test]
fn stats_recompute_identically_from_the_record() {
    let replay = record_activity(600);
    let a = compute(&replay, 100).unwrap();
    let b = compute(&replay, 100).unwrap();
    assert_eq!(a.final_tick, 600);
    assert_eq!(a.sample_ticks, b.sample_ticks);
    assert_eq!(a.sample_ticks.first(), Some(&0));
    for (pa, pb) in a.players.iter().zip(&b.players) {
        assert_eq!(pa.scrap, pb.scrap, "the record computes one truth");
        assert_eq!(pa.army_value, pb.army_value);
        assert_eq!(pa.scrap_collected, pb.scrap_collected);
        assert_eq!(pa.units_trained, pb.units_trained);
        assert_eq!(pa.buildings_completed, pb.buildings_completed);
    }
    // The fixture must have exercised nonzero activity.
    assert!(
        a.players
            .iter()
            .any(|p| p.army_value.iter().any(|&v| v > 0)),
        "somebody fielded an army"
    );
    assert!(
        a.players.iter().any(|p| p.scrap_collected > 0),
        "somebody delivered salvage"
    );
    assert!(
        a.players.iter().any(|p| p.units_trained > 0),
        "somebody completed production"
    );
}

#[test]
fn live_tracking_matches_tick_by_tick_replay_statistics() {
    let replay = record_activity(40);
    assert_eq!(track_replay(&replay), compute(&replay, 1).unwrap());
}

#[test]
fn live_tracking_stays_bounded_and_keeps_the_exact_final_tick() {
    let mut state = Scenario::skirmish().build().unwrap();
    let mut live = LiveMatchStats::new(&state);
    for _ in 0..5_000 {
        let report = state.tick(&[]);
        live.observe(&state, &report.events);
    }
    let report = live.snapshot(&state);
    assert!(report.sample_ticks.len() <= MAX_LIVE_SAMPLES + 1);
    assert_eq!(report.sample_ticks.first(), Some(&0));
    assert_eq!(report.sample_ticks.last(), Some(&5_000));
    assert_eq!(report.final_tick, 5_000);
    assert!(report.sample_ticks.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn thinned_live_totals_match_recomputed_event_totals() {
    let replay = record_activity(2_000);
    let live = track_replay(&replay);
    let recomputed = compute(&replay, 200).unwrap();

    assert_eq!(live.final_tick, recomputed.final_tick);
    assert!(
        live.sample_ticks
            .windows(2)
            .any(|ticks| ticks[1] - ticks[0] > 1),
        "the fixture must cross adaptive thinning"
    );
    assert!(live.sample_ticks.len() <= MAX_LIVE_SAMPLES + 1);
    for (actual, expected) in live.players.iter().zip(&recomputed.players) {
        assert_eq!(actual.scrap_collected, expected.scrap_collected);
        assert_eq!(actual.units_trained, expected.units_trained);
        assert_eq!(actual.buildings_completed, expected.buildings_completed);
        assert_eq!(actual.units_lost, expected.units_lost);
        assert_eq!(actual.buildings_lost, expected.buildings_lost);
        assert_eq!(actual.buildings_salvaged, expected.buildings_salvaged);
        assert_eq!(actual.scrap.last(), expected.scrap.last());
        assert_eq!(actual.army_value.last(), expected.army_value.last());
    }
    assert!(
        live.players.iter().any(|player| {
            player.scrap_collected > 0 && player.units_trained > 0 && player.buildings_completed > 0
        }),
        "the thinned fixture must include real economy and construction events"
    );
}

#[test]
fn deliberate_salvage_is_not_counted_as_a_building_loss() {
    use chassis::fx::Vec2Fx;
    use oxide_sim::BuildingId;

    let mut players = blank_players(2);
    accumulate_events(
        &mut players,
        &[
            Event::BuildingDestroyed {
                building: BuildingId(7),
                player: PlayerId(0),
                pos: Vec2Fx::ZERO,
            },
            Event::BuildingSalvaged {
                building: BuildingId(8),
                player: PlayerId(0),
                pos: Vec2Fx::ZERO,
                refund: 40,
            },
        ],
    );

    assert_eq!(players[0].buildings_lost, 1);
    assert_eq!(players[0].buildings_salvaged, 1);
    assert_eq!(players[1].buildings_lost, 0);
    assert_eq!(players[1].buildings_salvaged, 0);
}
