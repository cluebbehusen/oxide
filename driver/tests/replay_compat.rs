//! Compatibility checks at the file-loading boundary for historical replays.

use chassis::replay::ReplayError;
use oxide_kit::GameReplay;
use oxide_sim::{SIM_VERSION, Scenario};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn checkpoint_origins_reach_every_read_only_replay_surface() {
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    for _ in 0..37 {
        state.tick(&[]);
    }
    let origin = oxide_kit::recording::WorldOrigin::capture(&scenario, &state).unwrap();
    let mut replay = GameReplay::with_origin(SIM_VERSION, scenario, origin).unwrap();
    replay.meta.ticks = Some(43);
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let file = TempReplay(
        std::env::temp_dir().join(format!("oxide-origin-{}-{id}.json", std::process::id())),
    );
    replay.save(&file.0).unwrap();
    let replay = oxide_kit::load_replay(&file.0).unwrap();
    let report = oxide_driver::replay_inspect::inspect(&replay, &[37, 43], None, false).unwrap();
    assert_eq!(report.start_tick, 37);
    assert_eq!(
        report
            .snapshots
            .iter()
            .map(|snapshot| snapshot.tick)
            .collect::<Vec<_>>(),
        [37, 43]
    );
    assert_eq!(report.command_activity[0].longest_silence.from_tick, 37);
    assert_eq!(report.command_activity[0].longest_silence.duration_ticks, 6);
    assert!(oxide_driver::replay_inspect::inspect(&replay, &[36], None, false).is_err());
    let options = oxide_driver::replay_summary::SummaryOptions {
        until: None,
        every: None,
        minimaps: oxide_driver::replay_summary::MinimapMode::None,
    };
    let report = oxide_driver::replay_summary::summarize(&replay, &options).unwrap();
    assert_eq!(report.scenario.start_tick, 37);
    assert_eq!(report.scenario.effective_ticks, 43);
    assert!(
        report
            .tech_reach
            .iter()
            .flat_map(|seat| &seat.firsts)
            .all(|first| first.tick >= 37)
    );
    let end = oxide_kit::runner::run_replay(&replay, None, false).unwrap();
    assert_eq!(end.current_tick(), 43);
    assert!(oxide_driver::session::Session::resume(replay).is_err());
}

struct TempReplay(PathBuf);

impl Drop for TempReplay {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn legacy_bot_replay() -> TempReplay {
    let replay: GameReplay = GameReplay::new("0.0.0-legacy", Scenario::skirmish());
    let mut document = serde_json::to_value(replay).expect("current replay serializes");
    document["setup"]["players"][1]["bot_config"] = json!({"level": "medium"});

    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "oxide-legacy-bot-replay-{}-{id}.json",
        std::process::id()
    ));
    std::fs::write(
        &path,
        serde_json::to_vec(&document).expect("legacy fixture serializes"),
    )
    .expect("legacy fixture is written");
    TempReplay(path)
}

#[test]
fn legacy_bot_setup_reaches_version_validation_and_the_archaeology_flag() {
    let fixture = legacy_bot_replay();
    let replay = oxide_kit::load_replay(&fixture.0).expect("legacy setup remains loadable");
    assert!(matches!(
        replay.validate(Some(SIM_VERSION)),
        Err(ReplayError::VersionMismatch { .. })
    ));

    let refused = std::process::Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .arg("replay")
        .arg(&fixture.0)
        .output()
        .expect("run replay without compatibility flag");
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("replay was recorded on sim"),
        "the normal path should refuse at version validation: {}",
        String::from_utf8_lossy(&refused.stderr)
    );

    let allowed = std::process::Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .arg("replay")
        .arg(&fixture.0)
        .arg("--allow-version-mismatch")
        .output()
        .expect("run replay with compatibility flag");
    assert!(
        allowed.status.success(),
        "the archaeology flag should reach playback: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );
}
