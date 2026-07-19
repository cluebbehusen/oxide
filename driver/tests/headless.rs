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
