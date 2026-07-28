//! Matchup CLI regression coverage.
//!
//! These tests own the report's SHAPE, not its outcomes: which side wins a
//! leg is a balance result that stat and physics blesses legitimately move,
//! so every assertion here checks that a well-formed token appears, never
//! which token it is.

use std::process::Command;

/// The token following `needle`, cut at the next `,` or `)`.
fn token_after<'a>(line: &'a str, needle: &str) -> &'a str {
    let start = line
        .find(needle)
        .map(|i| i + needle.len())
        .unwrap_or_else(|| panic!("`{needle}` missing in line: {line}"));
    line[start..]
        .split([',', ')'])
        .next()
        .expect("split always yields one piece")
        .trim()
}

#[test]
fn matchup_surfaces_both_orientation_verdicts_and_neutral_aggregate() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "matchup",
            "--a",
            "bombard:5,scuttler:5",
            "--b",
            "sentinel:13",
        ])
        .output()
        .expect("run matchup");

    assert!(
        output.status.success(),
        "matchup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("matchup output is UTF-8");

    assert!(
        stdout.contains("A as player 0 / B as player 1:")
            && stdout.contains("A as player 1 / B as player 0:"),
        "both physical orientations must stay visible:\n{stdout}"
    );
    let leg_lines: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains("A as player"))
        .collect();
    assert_eq!(
        leg_lines.len(),
        2,
        "exactly one line per physical leg:\n{stdout}"
    );
    for line in &leg_lines {
        let verdict = token_after(line, "verdict ");
        assert!(
            matches!(verdict, "A" | "B" | "tie" | "unresolved"),
            "each leg must carry a verdict token, got `{verdict}`:\n{line}"
        );
        assert!(
            ["wipe", "no-progress", "cap"]
                .iter()
                .any(|t| line.contains(t)),
            "each leg must carry a termination token:\n{line}"
        );
        let hp_weighted = token_after(line, "[hp-weighted A ")
            .split_whitespace()
            .next()
            .unwrap_or_default();
        assert!(
            hp_weighted.parse::<u64>().is_ok(),
            "each leg must report a wound-discounted value, got `{hp_weighted}`:\n{line}"
        );
    }
    assert!(
        stdout.lines().any(|l| l.starts_with("seats: ")),
        "the report must name the rosters it seated:\n{stdout}"
    );
    assert!(
        stdout.contains("paired mean hp-weighted surviving value"),
        "the wound-discounted value must ride beside the paired mean:\n{stdout}"
    );

    let aggregate = stdout
        .lines()
        .find(|l| l.contains("paired mean surviving purchase value"))
        .unwrap_or_else(|| panic!("the paired neutral aggregate must stay visible:\n{stdout}"));
    let verdict = token_after(aggregate, "(verdict ");
    assert!(
        matches!(verdict, "A" | "B" | "tie" | "unresolved"),
        "the aggregate must carry a verdict token, got `{verdict}`:\n{aggregate}"
    );
    let flips = token_after(aggregate, "verdict flips on swap ");
    assert!(
        matches!(flips, "yes" | "no" | "unresolved"),
        "the aggregate must state whether the verdict flips on swap, got `{flips}`:\n{aggregate}"
    );

    assert!(
        !stdout.contains("seed "),
        "deterministically identical seed replication must not return:\n{stdout}"
    );
}

#[test]
fn matchup_does_not_offer_identical_seed_replication() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args(["matchup", "--help"])
        .output()
        .expect("run matchup help");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("matchup help is UTF-8");
    assert!(
        !stdout.contains("--seeds"),
        "arena seeds do not alter deployment or outcomes:\n{stdout}"
    );
}

/// The line naming the seated rosters, without its label.
fn seats_line(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args(args)
        .output()
        .expect("run matchup");
    assert!(
        output.status.success(),
        "matchup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("matchup output is UTF-8");
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("seats: "))
        .unwrap_or_else(|| panic!("no seats line:\n{stdout}"))
        .to_string()
}

#[test]
fn matchup_seats_one_roster_until_the_roster_is_the_experiment() {
    let duel = ["matchup", "--a", "scuttler:6", "--b", "sentinel:1"];
    let default = seats_line(&duel);
    let (west, east) = default
        .split_once(" / ")
        .unwrap_or_else(|| panic!("the seats line names both seats: {default}"));
    assert_eq!(
        west.trim_start_matches("west "),
        east.trim_start_matches("east "),
        "the default arena must seat one roster, so a leg swap exchanges nothing else: {default}"
    );

    let mut overridden = duel.to_vec();
    overridden.extend(["--factions", "cf"]);
    assert_eq!(seats_line(&overridden), "west Cupric / east Ferrous");

    let mut nonsense = duel.to_vec();
    nonsense.extend(["--factions", "fx"]);
    let refused = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args(&nonsense)
        .output()
        .expect("run matchup");
    assert!(!refused.status.success(), "'fx' is not a roster pair");
}

#[test]
fn matchup_refuses_a_garrison_pitch_the_structures_overlap() {
    let output = Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .args([
            "matchup",
            "--a",
            "scuttler:1",
            "--b",
            "",
            "--b-structures",
            "bastion:1",
            "--garrison-pitch",
            "1",
        ])
        .output()
        .expect("run matchup");

    assert!(
        !output.status.success(),
        "a 2x2 bastion cannot stand on a 1-tile pitch"
    );
}
