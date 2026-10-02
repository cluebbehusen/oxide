//! End-to-end contract coverage for `oxide-driver bot-ladder` and
//! `bot-ladder-report`.

use serde_json::Value;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxide-bot-ladder-cli-{name}-{}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn driver(args: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args(args)
        .output()
        .expect("run oxide-driver")
}

fn succeeded(output: &Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// Writes a one-comparison ladder over Skirmish at 24 ticks.
fn manifest(dir: &Path) -> PathBuf {
    let path = dir.join("ladder.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "name": "cli",
            "tick_limit": 24,
            "runs": 1,
            "scenario_seed_base": 7000,
            "personality_seed_base": 9000,
            "stances": ["balanced"],
            "comparisons": [{"higher": "prime", "lower": "scrapheap", "gate": 800}],
            "min_decided_pairs": 40,
            "maps": [{"path": "skirmish", "family": "open"}],
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

fn ladder(manifest: &Path, out: &Path, replays: &Path) -> Output {
    driver(&[
        "bot-ladder".as_ref(),
        manifest.as_os_str(),
        "--jobs".as_ref(),
        "2".as_ref(),
        "--out".as_ref(),
        out.as_os_str(),
        "--replay-dir".as_ref(),
        replays.as_os_str(),
    ])
}

#[test]
fn a_ladder_writes_paired_rows_and_reports_them_against_the_gate() {
    let dir = scratch("one-pair");
    let manifest = manifest(&dir);
    let (out, replays) = (dir.join("out"), dir.join("replays"));
    let report = succeeded(&ladder(&manifest, &out, &replays));
    assert!(
        report.contains("prime against scrapheap: too few decided pairs"),
        "{report}"
    );
    let rows: Vec<Value> = std::fs::read_to_string(out.join("rows.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 2);
    for (row, (leg, rungs)) in rows.iter().zip([
        ("forward", ["prime", "scrapheap"]),
        ("swapped", ["scrapheap", "prime"]),
    ]) {
        assert_eq!(row["leg"], leg);
        assert_eq!(row["ladder"]["higher"], "prime");
        assert_eq!(row["ladder"]["gate"], 800);
        for (seat, rung) in rungs.iter().enumerate() {
            assert_eq!(row["seats"][seat]["controller"], "opponent");
            assert_eq!(row["seats"][seat]["profile"]["difficulty"], *rung);
            assert_eq!(row["seats"][seat]["profile"]["personality_seed"], 9000);
        }
        assert!(Path::new(row["replay"].as_str().unwrap()).exists());
    }
    assert!(replays.join("legs.jsonl").exists());

    let json = succeeded(&driver(&[
        "bot-ladder-report".as_ref(),
        out.join("rows.jsonl").as_os_str(),
        "--json".as_ref(),
    ]));
    let json: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["comparisons"][0]["overall"]["legs"], 2);
    assert_eq!(json["comparisons"][0]["verdict"], "too_few_pairs");

    let again = ladder(&manifest, &out, &dir.join("replays-again"));
    assert!(!again.status.success(), "existing rows are never replaced");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_report_refuses_rows_from_bot_eval() {
    let dir = scratch("not-ladder");
    let rows = dir.join("rows.jsonl");
    succeeded(&driver(&[
        "bot-eval".as_ref(),
        "skirmish".as_ref(),
        "--ticks".as_ref(),
        "1".as_ref(),
        "--candidate".as_ref(),
        "cli".as_ref(),
        "--out".as_ref(),
        rows.as_os_str(),
    ]));
    let report = driver(&["bot-ladder-report".as_ref(), rows.as_os_str()]);
    assert!(!report.status.success());
    assert!(
        String::from_utf8_lossy(&report.stderr).contains("is not a bot-ladder row"),
        "{}",
        String::from_utf8_lossy(&report.stderr)
    );
    std::fs::remove_dir_all(dir).unwrap();
}
