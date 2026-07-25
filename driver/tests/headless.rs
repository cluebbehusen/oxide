//! Driver-level headless checks: the runner's record/replay loop is the
//! same one the shell uses, so this proves the whole recording pipeline
//! without a window.

use oxide_driver::runner;
use oxide_sim::Scenario;

fn bot_skirmish() -> Scenario {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
    }
    scenario
}

#[test]
fn recorded_scenario_run_reproduces_from_its_replay() {
    let scenario = bot_skirmish();
    let outcome = runner::run_scenario(&scenario, 900, true, true).unwrap();
    let replay = outcome.replay.unwrap();
    assert_eq!(replay.meta.ticks, Some(900));
    assert!(!replay.commands.is_empty());

    let replayed = runner::run_replay(&replay, None, false).unwrap();
    assert_eq!(replayed.current_tick(), outcome.state.current_tick());
    assert_eq!(replayed.hash(), outcome.state.hash());
}

#[test]
fn every_shipped_scenario_builds_and_plays() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        checked += 1;
        let mut scenario =
            Scenario::load(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        for player in &mut scenario.players {
            player.bot = true;
            // The shipped default opponent; a configless flip would field
            // the team-blind classic bot, which team maps now reject.
            player
                .bot_config
                .get_or_insert(oxide_sim::scenario::BotConfig {
                    level: oxide_sim::bot::Level::Medium,
                    aggression: None,
                });
        }
        // Playable means *alive*, not merely parseable: after 12k ticks of
        // bot-vs-bot the match must either be decided or still producing —
        // unit ids are monotonic, so a high id proves Foundries kept
        // working. (Reaching the tick count alone once masked a total
        // economy freeze.)
        let outcome = runner::run_scenario(&scenario, 12_000, true, false)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        assert_eq!(outcome.state.current_tick(), 12_000, "{}", path.display());
        let produced = outcome.state.units().iter().any(|u| u.id.0 >= 16);
        assert!(
            outcome.state.result().is_some() || produced,
            "{}: no victory and no production after 12k ticks — the map stalled",
            path.display()
        );
    }
    assert!(checked >= 4, "expected the shipped maps, found {checked}");
}

#[test]
fn run_without_bots_is_quiet_but_valid() {
    let outcome = runner::run_scenario(&Scenario::skirmish(), 100, false, true).unwrap();
    let replay = outcome.replay.unwrap();
    assert!(replay.commands.is_empty(), "nobody issued commands");
    assert_eq!(outcome.state.current_tick(), 100);
}

#[test]
fn forged_marathon_replays_are_refused() {
    use chassis::replay::Replay;
    use oxide_sim::{PlayerCommand, SIM_VERSION, Scenario};
    let mut replay: Replay<Scenario, PlayerCommand> =
        Replay::new(SIM_VERSION, Scenario::skirmish());
    replay.meta.ticks = Some(u64::MAX - 1);
    let err = runner::run_replay(&replay, None, false).unwrap_err();
    assert!(err.to_string().contains("--allow-long"), "{err}");
}

#[test]
fn load_scenario_resolves_the_skirmish_shorthand() {
    assert_eq!(
        runner::load_scenario("skirmish").unwrap(),
        Scenario::skirmish(),
        "the bare word must resolve to the embedded map, not a file lookup"
    );
}

#[test]
fn load_scenario_names_the_path_when_it_cannot_be_read() {
    let err = runner::load_scenario("definitely/not/a/real/scenario.json").unwrap_err();
    assert!(
        err.to_string()
            .contains("definitely/not/a/real/scenario.json"),
        "the error should name the path it failed on: {err}"
    );
}

#[test]
fn run_scenario_surfaces_a_build_failure_with_context() {
    use oxide_sim::Faction;
    use oxide_sim::scenario::PlayerSpec;
    // Parses fine, but the extra seat has no Foundry anchor on the map, so
    // the build fails; the runner must wrap that, not swallow it.
    let mut scenario = Scenario::skirmish();
    scenario.players.push(PlayerSpec {
        name: "anchorless".into(),
        faction: Faction::Ferrous,
        team: None,
        scrap: 0,
        bot: false,
        bot_config: None,
    });
    let err = match runner::run_scenario(&scenario, 10, false, false) {
        Ok(_) => panic!("an anchorless seat must fail the build"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("building scenario"), "{err}");
}

#[test]
fn an_unfought_match_reports_no_result() {
    let outcome = runner::run_scenario(&Scenario::skirmish(), 300, false, false).unwrap();
    assert!(
        outcome.state.result().is_none(),
        "nobody fought, so the match stays undecided"
    );
}

#[test]
fn a_decided_match_latches_its_result_and_keeps_ticking() {
    use oxide_sim::scenario::{PlayerSpec, UnitSpec};
    use oxide_sim::{Faction, GameResult, UnitKind};

    // A firing squad: seat 0's Sentinels sit inside aggro range of seat 1's
    // lone Foundry and grind it down with no orders at all; seat 1 has no
    // army to answer. The win lands well before the tick budget, which lets
    // us prove run_scenario keeps counting past the victory (frozen ticks
    // included) instead of returning early.
    let ground = ".".repeat(16);
    let mut anchored: Vec<char> = ground.chars().collect();
    anchored[1] = '1';
    anchored[11] = '2';
    let map = vec![
        ground.clone(),
        ground.clone(),
        anchored.into_iter().collect(),
        ground.clone(),
        ground.clone(),
        ground,
    ];
    let mut units = Vec::new();
    for x in [8, 9] {
        for y in [1, 2, 3, 4] {
            units.push(UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x,
                y,
            });
        }
    }
    let scenario = Scenario {
        name: "firing-squad".into(),
        seed: 7,
        map,
        players: vec![
            PlayerSpec {
                name: "attacker".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 100,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "victim".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 100,
                bot: false,
                bot_config: None,
            },
        ],
        units,
        buildings: Vec::new(),
        meta: None,
    };

    let budget = 3_000;
    let outcome = runner::run_scenario(&scenario, budget, false, false).unwrap();
    assert_eq!(
        outcome.state.result(),
        Some(GameResult::Victory { team: 0 }),
        "seat 1's only Foundry should be rubble"
    );
    assert_eq!(
        outcome.state.current_tick(),
        budget,
        "a mid-run victory must not cut the requested tick count short"
    );
}

#[test]
fn a_version_mismatched_replay_is_refused_by_default() {
    use chassis::replay::Replay;
    use oxide_sim::PlayerCommand;
    let replay: Replay<Scenario, PlayerCommand> =
        Replay::new("0.0.0-not-this-sim", Scenario::skirmish());
    let err = runner::run_replay(&replay, None, false).unwrap_err();
    assert!(err.to_string().contains("recorded on sim"), "{err}");
}

#[test]
fn a_version_mismatched_replay_plays_when_the_mismatch_is_allowed() {
    use chassis::replay::Replay;
    use oxide_sim::PlayerCommand;
    let replay: Replay<Scenario, PlayerCommand> =
        Replay::new("0.0.0-not-this-sim", Scenario::skirmish());
    let state = runner::run_replay(&replay, None, true).unwrap();
    assert_eq!(
        state.current_tick(),
        0,
        "an empty replay loads to its opening state even across a version gap"
    );
}

#[test]
fn overriding_the_tick_count_below_the_commands_is_rejected() {
    use chassis::replay::Replay;
    use oxide_sim::{Command, PlayerCommand, PlayerId, SIM_VERSION, UnitId};
    let mut replay: Replay<Scenario, PlayerCommand> =
        Replay::new(SIM_VERSION, Scenario::skirmish());
    replay.record(
        100,
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Stop {
                units: vec![UnitId(0)],
            },
        },
    );
    replay.meta.ticks = Some(200);
    // The override stops playback at 50, stranding the tick-100 command; a
    // silent drop would desync a "resumed" session, so it must be an error.
    let err = runner::run_replay(&replay, Some(50), false).unwrap_err();
    assert!(err.to_string().contains("unconsumed"), "{err}");
}
