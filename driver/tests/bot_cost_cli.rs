//! End-to-end contract coverage for `oxide-driver bot-cost`.

use oxide_driver::bot_cost::Workload;
use oxide_protocol::hash_hex;
use oxide_sim::scenario::BotController;
use serde_json::Value;
use std::process::Command;

fn bot_cost(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .arg("bot-cost")
        .args(args)
        .output()
        .expect("run bot-cost");
    assert!(
        output.status.success(),
        "bot-cost failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("output is UTF-8")
}

/// The same workload through ordinary scheduling, which may run seats in
/// parallel, with no timing.
fn untimed(workload: Workload, controller: BotController, ticks: u64) -> (String, String) {
    let scenario = workload.scenario(controller);
    let mut state = scenario.build().unwrap();
    let mut seats = oxide_kit::controller::seat_controllers(&scenario).unwrap();
    let mut fold = 0_u64;
    while state.current_tick() < ticks && state.result().is_none() {
        let commands = oxide_kit::bot_execution::commands(&state, &mut seats);
        fold = chassis::hash::state_hash(&(fold, state.current_tick(), &commands));
        state.tick(&commands);
    }
    (hash_hex(fold), hash_hex(state.hash()))
}

#[test]
fn timed_runs_report_every_seat_and_match_an_untimed_run() {
    for controller in BotController::ALL {
        let report: Value = serde_json::from_str(&bot_cost(&[
            "duel",
            "--ticks",
            "48",
            "--controller",
            controller.as_str(),
            "--json",
        ]))
        .expect("--json prints one JSON document");

        assert_eq!(report["workload"], "duel");
        assert_eq!(report["final_tick"], 48);
        let seats = report["seats"].as_array().unwrap();
        assert_eq!(seats.len(), 2);
        for (index, seat) in seats.iter().enumerate() {
            assert_eq!(seat["player"], index);
            assert_eq!(seat["controller"], controller.as_str());
            assert_eq!(
                seat["decision"]["count"], 4,
                "decisions at ticks 0, 12, 24 and 36 only"
            );
            assert_eq!(seat["observation"]["count"], 4);
            assert!(seat["decision"]["total_ns"].as_u64().unwrap() > 0);
            assert_eq!(
                seat["orientation"].is_object(),
                controller == BotController::Scripted,
                "orientation is oxide-bot's cost"
            );
        }
        let controllers = report["controllers"].as_array().unwrap();
        assert_eq!(controllers.len(), 1);
        assert_eq!(controllers[0]["controller"], controller.as_str());
        assert_eq!(controllers[0]["seats"], 2);
        assert_eq!(controllers[0]["decision"]["count"], 8);
        assert_eq!(report["simulation"]["count"], 48);

        let (commands, world) = untimed(Workload::Duel, controller, 48);
        assert_eq!(report["command_hash"], commands.as_str());
        assert_eq!(report["final_hash"], world.as_str());
    }
}

#[test]
fn the_table_names_the_workload_and_each_seat() {
    let table = bot_cost(&["mature-armies", "--ticks", "24", "--controller", "opponent"]);
    let (commands, world) = untimed(Workload::MatureArmies, BotController::Opponent, 24);
    assert!(table.starts_with("mature-armies: Mature Armies"));
    assert!(table.contains(&format!("command hash {commands}, final hash {world}")));
    let seats: Vec<_> = table
        .lines()
        .filter(|line| line.contains("opponent"))
        .map(|line| line.split_whitespace().next().unwrap())
        .collect();
    assert_eq!(seats, ["0", "1", "all"]);
}
