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
    }

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
