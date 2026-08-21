//! End-to-end contract coverage for `oxide-driver replay-summary`.

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
        "oxide-replay-summary-{}-{id}.json",
        std::process::id()
    ));
    replay.save(&path).expect("save generated replay fixture");
    TempReplay(path)
}

fn run_summary(fixture: &TempReplay, extra: &[&str]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .arg("replay-summary")
        .arg(&fixture.0)
        .args(extra)
        .output()
        .expect("run replay-summary");
    assert!(
        output.status.success(),
        "replay-summary failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn replay_summary_emits_the_json_contract() {
    let fixture = save_fixture();
    let output = run_summary(&fixture, &["--every", "40", "--json"]);

    let report: Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["scenario"]["name"], "Skirmish Basin");
    assert_eq!(report["scenario"]["effective_ticks"], 120);
    assert_eq!(report["scenario"]["every"], 40);
    assert_eq!(report["seats"].as_array().expect("seat array").len(), 2);

    let digest_ticks: Vec<u64> = report["digests"]
        .as_array()
        .expect("digest array")
        .iter()
        .map(|digest| digest["tick"].as_u64().expect("numeric tick"))
        .collect();
    assert_eq!(digest_ticks, vec![40, 80, 120]);
    for digest in report["digests"].as_array().expect("digest array") {
        assert_eq!(digest["rows"].as_array().expect("row array").len(), 2);
    }
}

#[test]
fn replay_summary_text_carries_header_digests_and_legend() {
    let fixture = save_fixture();
    let output = run_summary(&fixture, &["--every", "60", "--minimaps", "all"]);
    let text = String::from_utf8(output.stdout).expect("utf8 text");
    assert!(text.contains("Skirmish Basin"), "missing header:\n{text}");
    assert!(text.contains("digest t=120"), "missing digest:\n{text}");
    assert!(text.contains("map: a-b units"), "missing legend:\n{text}");
    assert!(
        text.contains("result: undecided at the recording's end"),
        "a quiet fixture must read undecided:\n{text}"
    );
}

fn save_surrender_fixture() -> TempReplay {
    let scenario = Scenario::skirmish();
    let mut replay: GameReplay = Replay::new(SIM_VERSION, scenario);
    replay.record(
        5,
        PlayerCommand {
            player: PlayerId(1),
            command: SimCommand::Surrender,
        },
    );
    replay.meta.ticks = Some(60);

    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "oxide-replay-summary-surrender-{}-{id}.json",
        std::process::id()
    ));
    replay.save(&path).expect("save surrender fixture");
    TempReplay(path)
}

#[test]
fn a_surrender_reaches_the_timeline_outcome_and_post_game_digest() {
    let fixture = save_surrender_fixture();
    let output = run_summary(&fixture, &["--every", "10", "--json"]);
    let report: Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");

    let kinds: Vec<&str> = report["timeline"]
        .as_array()
        .expect("timeline array")
        .iter()
        .map(|moment| moment["kind"].as_str().expect("tagged kind"))
        .collect();
    assert!(kinds.contains(&"resignation"), "kinds: {kinds:?}");
    assert!(kinds.contains(&"elimination"), "kinds: {kinds:?}");
    assert!(kinds.contains(&"game_over"), "kinds: {kinds:?}");

    // Intermediate post-game boundaries are suppressed: only the closing
    // digest survives, marked post-game.
    let digests = report["digests"].as_array().expect("digest array");
    assert_eq!(digests.len(), 1, "digests: {digests:?}");
    assert_eq!(digests[0]["tick"], 60);
    assert_eq!(digests[0]["post_game"], true);

    assert_eq!(report["outcome"]["result"]["outcome"], "victory");
    assert_eq!(report["outcome"]["result"]["team"], 0);
    assert_eq!(report["outcome"]["winner_seats"], serde_json::json!([0]));
    let decided = report["outcome"]["decided_at"].as_u64().expect("decided");
    assert_eq!(
        report["outcome"]["post_game_ticks"].as_u64().expect("post"),
        60 - decided
    );

    let text = String::from_utf8(run_summary(&fixture, &["--every", "10"]).stdout).expect("utf8");
    assert!(text.contains("resigned: seat 1"), "text:\n{text}");
    assert!(text.contains("victory team 0"), "text:\n{text}");
    assert!(text.contains("(post-game)"), "text:\n{text}");
}

#[test]
fn minimap_modes_emit_all_sparse_and_none() {
    let fixture = save_fixture();
    let count = |extra: &[&str]| {
        let text = String::from_utf8(run_summary(&fixture, extra).stdout).expect("utf8");
        text.lines()
            .filter(|line| line.trim_start().starts_with("map: "))
            .count()
    };
    // --every 20 over 120 ticks = digests at 20,40,60,80,100,120.
    assert_eq!(count(&["--every", "20", "--minimaps", "all"]), 6);
    assert_eq!(
        count(&["--every", "20", "--minimaps", "sparse"]),
        2,
        "sparse = every fourth digest plus the final one"
    );
    assert_eq!(count(&["--every", "20", "--minimaps", "none"]), 0);
}

#[test]
fn replay_summary_is_deterministic() {
    let fixture = save_fixture();
    let text_a = run_summary(&fixture, &["--every", "60"]).stdout;
    let text_b = run_summary(&fixture, &["--every", "60"]).stdout;
    assert_eq!(text_a, text_b, "text output must be byte-identical");
    let json_a = run_summary(&fixture, &["--every", "60", "--json"]).stdout;
    let json_b = run_summary(&fixture, &["--every", "60", "--json"]).stdout;
    assert_eq!(json_a, json_b, "JSON output must be byte-identical");
}
