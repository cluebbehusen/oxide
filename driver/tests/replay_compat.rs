//! Compatibility checks at the file-loading boundary for historical replays.

use chassis::replay::{Replay, ReplayError};
use oxide_kit::GameReplay;
use oxide_sim::{PlayerCommand, SIM_VERSION, Scenario};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct TempReplay(PathBuf);

impl Drop for TempReplay {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn legacy_bot_replay() -> TempReplay {
    let replay: GameReplay =
        Replay::<Scenario, PlayerCommand>::new("0.0.0-legacy", Scenario::skirmish());
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
    let replay = GameReplay::load(&fixture.0).expect("legacy setup remains loadable");
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
