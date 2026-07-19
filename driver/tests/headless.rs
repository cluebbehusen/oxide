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

    let replayed = runner::run_replay(&replay, None).unwrap();
    assert_eq!(replayed.tick, outcome.state.tick);
    assert_eq!(replayed.hash(), outcome.state.hash());
}

#[test]
fn run_without_bots_is_quiet_but_valid() {
    let outcome = runner::run_scenario(&Scenario::skirmish(), 100, false, true).unwrap();
    let replay = outcome.replay.unwrap();
    assert!(replay.commands.is_empty(), "nobody issued commands");
    assert_eq!(outcome.state.tick, 100);
}
