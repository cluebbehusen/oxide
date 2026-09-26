//! Driver-level headless checks: the runner's record/replay loop is the
//! same one the shell uses, so this proves the whole recording pipeline
//! without a window.

use oxide_driver::{pool, runner};
use oxide_sim::{PlayerCommand, Scenario, State};
use std::path::{Path, PathBuf};

fn assert_state_round_trip(state: &State) -> anyhow::Result<()> {
    state.validate_invariants()?;
    let restored: State = serde_json::from_slice(&serde_json::to_vec(state)?)?;
    anyhow::ensure!(
        restored.hash() == state.hash(),
        "state changed across a JSON round trip at tick {}",
        state.current_tick()
    );
    Ok(())
}

/// Hash samples and the command log of one soak run. CI compares the hash
/// logs across operating systems; replaying the command log elsewhere
/// separates a bot divergence from a simulation one.
struct SoakTrace {
    stem: PathBuf,
    hashes: String,
    replay: oxide_kit::GameReplay,
}

impl SoakTrace {
    fn new(stem: &Path, scenario: &Scenario) -> Self {
        Self {
            stem: stem.to_owned(),
            hashes: String::new(),
            replay: chassis::replay::Replay::new(oxide_sim::SIM_VERSION, scenario.clone()),
        }
    }

    fn record(&mut self, state: &State, commands: &[PlayerCommand]) {
        for command in commands {
            self.replay.record(state.current_tick(), command.clone());
        }
    }

    fn sample(&mut self, state: &State) {
        use std::fmt::Write;
        writeln!(
            self.hashes,
            "{} {:#018x}",
            state.current_tick(),
            state.hash()
        )
        .expect("writing to a String cannot fail");
    }

    fn finish(mut self, state: &State) -> anyhow::Result<()> {
        if !state.current_tick().is_multiple_of(TRACE_HASH_INTERVAL) {
            self.sample(state);
        }
        self.replay.meta.ticks = Some(state.current_tick());
        std::fs::write(self.stem.with_extension("hashes"), &self.hashes)?;
        self.replay.save(self.stem.with_extension("replay.json"))?;
        Ok(())
    }
}

fn play_and_check_integrity(
    scenario: &Scenario,
    ticks: u64,
    trace: Option<&Path>,
) -> anyhow::Result<()> {
    let mut state = scenario.build()?;
    let mut bots = oxide_bot::seat_bots(scenario)?;
    let mut trace = trace.map(|stem| SoakTrace::new(stem, scenario));
    assert_state_round_trip(&state)?;
    for _ in 0..ticks {
        let commands: Vec<_> = bots.iter_mut().flat_map(|bot| bot.act(&state)).collect();
        if let Some(trace) = &mut trace {
            trace.record(&state, &commands);
        }
        state.tick(&commands);
        let tick = state.current_tick();
        if let Some(trace) = &mut trace
            && tick.is_multiple_of(TRACE_HASH_INTERVAL)
        {
            trace.sample(&state);
        }
        if tick.is_multiple_of(STATE_VALIDATION_INTERVAL) {
            state.validate_invariants()?;
        }
        if tick.is_multiple_of(STATE_ROUND_TRIP_INTERVAL) {
            assert_state_round_trip(&state)?;
        }
        if state.result().is_some() {
            break;
        }
    }
    if let Some(trace) = trace {
        trace.finish(&state)?;
    }
    assert_state_round_trip(&state)
}

/// The shipped maps, biggest file first: the 4v4s are the sweep's
/// critical path and a last-scheduled Compass Grand would add its whole
/// runtime to the tail.
fn shipped_scenarios() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    assert!(
        paths.len() >= 4,
        "expected the shipped maps, found {}",
        paths.len()
    );
    paths.sort_by_key(|p| std::cmp::Reverse(std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)));
    paths
}

/// A shipped map with every seat flipped to a configured bot seat, the
/// shape every launched match declares. All seats use Standard, Balanced,
/// personality seed zero so the integrity run has one explicit profile.
fn all_bots(path: &std::path::Path) -> Scenario {
    let mut scenario =
        Scenario::load(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    for player in &mut scenario.players {
        player.bot = true;
        player.bot_config = Some(oxide_sim::scenario::BotConfig::scripted(
            oxide_sim::scenario::BotDifficulty::Standard,
            oxide_sim::scenario::BotStance::Balanced,
            0,
        ));
    }
    scenario
}

fn bot_skirmish() -> Scenario {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
        player.bot_config = Some(oxide_sim::scenario::BotConfig::default());
    }
    scenario
}

#[test]
fn recorded_scenario_run_reproduces_from_its_replay() {
    // Exercise the runner recording path with a non-empty current-bot log.
    use chassis::replay::Replay;
    use oxide_sim::SIM_VERSION;

    let scenario = bot_skirmish();
    let mut state = scenario.build().unwrap();
    let mut bots = oxide_bot::seat_bots(&scenario).unwrap();
    let mut replay: oxide_kit::GameReplay = Replay::new(SIM_VERSION, scenario);
    for _ in 0..900 {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        for command in &commands {
            replay.record(state.current_tick(), command.clone());
        }
        state.tick(&commands);
    }
    replay.meta.ticks = Some(state.current_tick());
    assert_eq!(replay.meta.ticks, Some(900));
    assert!(!replay.commands.is_empty());

    let replayed = runner::run_replay(&replay, None, false).unwrap();
    assert_eq!(replayed.current_tick(), state.current_tick());
    assert_eq!(replayed.hash(), state.hash());
}

const INTEGRITY_TICKS: u64 = 12_000;
const LARGE_MAP_INTEGRITY_TICKS: u64 = 24_000;
const STATE_VALIDATION_INTERVAL: u64 = 100;
const STATE_ROUND_TRIP_INTERVAL: u64 = 500;
const TRACE_HASH_INTERVAL: u64 = 20;
/// When set, each integrity run writes `<map>.hashes` and
/// `<map>.replay.json` into this directory. CI compares the hash files
/// across operating systems.
const TRACE_DIR_VAR: &str = "OXIDE_SOAK_TRACE_DIR";

fn integrity_horizon(scenario: &Scenario) -> u64 {
    match scenario.meta.as_ref().map(|m| m.pace.as_str()) {
        Some("vast" | "large" | "grand") => LARGE_MAP_INTEGRITY_TICKS,
        _ => INTEGRITY_TICKS,
    }
}

#[test]
fn representative_scenarios_preserve_state_integrity() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    let paths =
        ["skirmish", "twin-forges", "basalt-spine"].map(|name| dir.join(format!("{name}.json")));
    assert_scenarios_preserve_state_integrity(&paths);
}

#[test]
#[ignore = "exhaustive all-map integrity sweep; run on demand"]
fn every_shipped_scenario_preserves_state_integrity() {
    assert_scenarios_preserve_state_integrity(&shipped_scenarios());
}

fn assert_scenarios_preserve_state_integrity(paths: &[PathBuf]) {
    use anyhow::Context;
    let trace_dir = std::env::var_os(TRACE_DIR_VAR).map(PathBuf::from);
    if let Some(dir) = &trace_dir {
        std::fs::create_dir_all(dir).unwrap();
    }
    pool::fan_out(paths, |path| {
        let scenario = all_bots(path);
        let trace = trace_dir
            .as_ref()
            .map(|dir| dir.join(path.file_stem().expect("scenario file name")));
        play_and_check_integrity(&scenario, integrity_horizon(&scenario), trace.as_deref())
            .with_context(|| format!("{} failed state integrity", path.display()))
    })
    .unwrap();
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
    use oxide_sim::{SIM_VERSION, Scenario};
    let mut replay: oxide_kit::GameReplay = Replay::new(SIM_VERSION, Scenario::skirmish());
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
        mode: Default::default(),
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
    let replay: oxide_kit::GameReplay = Replay::new("0.0.0-not-this-sim", Scenario::skirmish());
    let err = runner::run_replay(&replay, None, false).unwrap_err();
    assert!(err.to_string().contains("recorded on sim"), "{err}");
}

#[test]
fn a_version_mismatched_replay_plays_when_the_mismatch_is_allowed() {
    use chassis::replay::Replay;
    let replay: oxide_kit::GameReplay = Replay::new("0.0.0-not-this-sim", Scenario::skirmish());
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
    let mut replay: oxide_kit::GameReplay = Replay::new(SIM_VERSION, Scenario::skirmish());
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

#[test]
fn every_shipped_scenario_names_its_seats_uniquely() {
    // The banner, panel, and stats all address seats by name, and the
    // shell's launch refuses collisions.
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let scenario =
            Scenario::load(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        let mut names: Vec<&str> = scenario.players.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            before,
            "{}: seat names collide",
            path.display()
        );
    }
}
