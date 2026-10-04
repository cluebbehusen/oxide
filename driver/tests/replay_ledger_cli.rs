//! End-to-end contract coverage for `oxide-driver replay-ledger`.

use chassis::replay::Replay;
use oxide_kit::GameReplay;
use oxide_sim::{SIM_VERSION, Scenario};
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxide-replay-ledger-cli-{name}-{}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn ledger(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .arg("replay-ledger")
        .args(args)
        .output()
        .expect("run replay-ledger")
}

/// A skirmish replay of `ticks` idle ticks.
fn replay(dir: &std::path::Path, ticks: u64) -> PathBuf {
    let mut replay: GameReplay = Replay::new(SIM_VERSION, Scenario::skirmish());
    replay.meta.ticks = Some(ticks);
    let path = dir.join("skirmish.json");
    replay.save(&path).expect("save generated replay fixture");
    path
}

#[test]
fn a_replay_ledger_reports_every_seat_as_json_and_text() {
    let dir = scratch("contract");
    let path = replay(&dir, 3_000);
    let output = ledger(&[path.as_os_str(), "--json".as_ref()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let games: Value = serde_json::from_slice(&output.stdout).unwrap();
    let [game] = games.as_array().unwrap().as_slice() else {
        panic!("{games}");
    };
    assert_eq!(game["ticks"], 3_000);
    let seats = game["seats"].as_array().unwrap();
    assert_eq!(seats.len(), Scenario::skirmish().players.len());
    for seat in seats {
        let ledger = &seat["ledger"];
        assert_eq!(ledger["worth"].as_array().unwrap().len(), 3);
        let foundry = &ledger["buildings"]["foundry"];
        assert!(foundry["starting"].as_u64().unwrap() > 0);
        assert!(
            foundry["passive"].as_u64().unwrap() > 0,
            "the drip: {foundry}"
        );
    }

    let output = ledger(&[path.as_os_str(), path.as_os_str()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("2 games"), "{text}");
    assert!(text.contains("income"), "{text}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_player_save_is_refused_for_its_missing_history() {
    let dir = scratch("save");
    let save = dir.join("game.oxsave");
    std::fs::write(&save, b"OXIDESAV\0header").unwrap();
    let output = ledger(&[save.as_os_str()]);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("match recording"), "{error}");
    std::fs::remove_dir_all(dir).unwrap();
}
