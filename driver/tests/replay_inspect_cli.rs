//! End-to-end contract coverage for `oxide-driver replay-inspect`.

use chassis::replay::Replay;
use oxide_kit::GameReplay;
use oxide_sim::{Command as SimCommand, PlayerCommand, PlayerId, SIM_VERSION, Scenario, UnitId};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct TempReplay(PathBuf);

impl Drop for TempReplay {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn stop(player: u8, unit: UnitId) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command: SimCommand::Stop { units: vec![unit] },
    }
}

fn save_fixture() -> TempReplay {
    let scenario = Scenario::skirmish();
    let state = scenario.build().expect("skirmish builds");
    let unit = |seat| {
        state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(seat))
            .expect("each seat starts with a unit")
            .id
    };
    let mut replay: GameReplay = Replay::new(SIM_VERSION, scenario);
    replay.record(0, stop(0, unit(0)));
    replay.record(7, stop(1, unit(1)));
    replay.record(118, stop(1, unit(1)));
    replay.meta.ticks = Some(120);

    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "oxide-replay-inspect-{}-{id}.json",
        std::process::id()
    ));
    replay.save(&path).expect("save generated replay fixture");
    TempReplay(path)
}

#[test]
fn replay_inspect_emits_stable_json_snapshots_and_command_silence() {
    let fixture = save_fixture();
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .arg("replay-inspect")
        .arg(&fixture.0)
        .args([
            "--tick",
            "118,0",
            "--tick",
            "7,7",
            "--fog-seat",
            "1",
            "--map",
        ])
        .output()
        .expect("run replay-inspect");
    assert!(
        output.status.success(),
        "replay-inspect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["scenario"]["name"], "Skirmish Basin");
    assert_eq!(report["scenario"]["players"][1]["bot"], true);
    assert_eq!(
        report["scenario"]["players"][1]["bot_config"]["level"],
        "medium"
    );
    assert_eq!(report["final_state"]["tick"], 120);
    assert_eq!(report["final_state"]["recorded_commands"], 3);

    let snapshot_ticks: Vec<u64> = report["snapshots"]
        .as_array()
        .expect("snapshot array")
        .iter()
        .map(|snapshot| snapshot["tick"].as_u64().expect("numeric tick"))
        .collect();
    assert_eq!(snapshot_ticks, vec![0, 7, 118]);
    for snapshot in report["snapshots"].as_array().expect("snapshot array") {
        assert!(snapshot["state"]["map"].is_array());
        assert_eq!(snapshot["fog"]["player"], 1);
        assert_eq!(snapshot["fog"]["tick"], snapshot["tick"]);
    }

    let seat1 = report["command_activity"]
        .as_array()
        .expect("activity array")
        .iter()
        .find(|activity| activity["seat"] == 1)
        .expect("seat 1 activity");
    assert_eq!(seat1["command_count"], 2);
    assert_eq!(seat1["by_type"]["stop"], 2);
    assert_eq!(seat1["longest_silence"]["from_tick"], 7);
    assert_eq!(seat1["longest_silence"]["to_tick"], 118);
    assert_eq!(seat1["longest_silence"]["duration_ticks"], 111);
    assert_eq!(seat1["longest_silence"]["start_boundary"], "command");
    assert_eq!(seat1["longest_silence"]["end_boundary"], "command");
}
