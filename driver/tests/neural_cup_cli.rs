//! Native quantized cup CLI coverage.

use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn faction_cup_reports_the_pair_and_each_physical_seat() {
    let weights =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tiny_policy_v9.json");
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "neural-cup",
            "--weights",
            weights.to_str().expect("UTF-8 fixture path"),
            "--seeds",
            "1",
            "--ticks",
            "1",
            "--factions",
            "cf",
        ])
        .output()
        .expect("run neural cup");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
    assert!(stderr.contains("factions cf (override)"), "{stderr}");

    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    let rows: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("cup JSON row"))
        .collect();
    assert_eq!(rows.len(), 2);
    let opponents: Vec<_> = rows
        .iter()
        .map(|row| row["opponent"].as_str().expect("opponent name"))
        .collect();
    assert_eq!(opponents, ["Overseer", "Rusher"]);
    for row in rows {
        assert_eq!(row["profile"], "canonical-slate");
        assert_eq!(row["factions"], "cf");
        assert_eq!(row["factions_source"], "override");
        assert_eq!(row["max_ticks"], 1);
        assert_eq!(row["by_seat"][0]["seat"], 0);
        assert_eq!(row["by_seat"][0]["faction"], "cupric");
        assert_eq!(row["by_seat"][1]["seat"], 1);
        assert_eq!(row["by_seat"][1]["faction"], "ferrous");
    }

    let raw = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "neural-cup",
            "--weights",
            weights.to_str().expect("UTF-8 fixture path"),
            "--seeds",
            "1",
            "--ticks",
            "1",
            "--blunder",
            "0",
        ])
        .output()
        .expect("run raw-profile neural cup");
    assert!(
        raw.status.success(),
        "{}",
        String::from_utf8_lossy(&raw.stderr)
    );
    let rows: Vec<Value> = String::from_utf8(raw.stdout)
        .expect("UTF-8 stdout")
        .lines()
        .map(|line| serde_json::from_str(line).expect("cup JSON row"))
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|row| row["profile"] == "raw"),
        "an explicitly supplied zero is an exact raw override"
    );
}
