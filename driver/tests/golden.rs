//! Golden-image regression tests.
//!
//! The driver's software renderer is bit-deterministic, so these compare
//! PNG bytes exactly — no tolerance thresholds to tune. When an intentional
//! sim or renderer change moves the pixels:
//!
//! 1. `BLESS=1 cargo test -p oxide-driver` to regenerate,
//! 2. *look at* the regenerated PNGs in `driver/tests/goldens/`,
//! 3. commit them together with the change and say why.

use oxide_driver::{render, runner};
use oxide_sim::Scenario;
use std::path::PathBuf;

fn golden_check(name: &str, state: &oxide_sim::State) {
    let actual = render::png_bytes(state).unwrap();
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(format!("{name}.png"));

    if std::env::var_os("BLESS").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &actual).unwrap();
        eprintln!("blessed {}", golden.display());
        return;
    }
    let expected = std::fs::read(&golden).unwrap_or_else(|_| {
        panic!(
            "missing golden {} — run `BLESS=1 cargo test -p oxide-driver` and commit it",
            golden.display()
        )
    });
    if expected != actual {
        let actual_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target")
            .join(format!("golden-actual-{name}.png"));
        std::fs::write(&actual_path, &actual).unwrap();
        panic!(
            "golden mismatch for {name}: inspect {} vs {}, re-bless if the change is intended",
            golden.display(),
            actual_path.display()
        );
    }
}

#[test]
fn skirmish_opening_matches_golden() {
    let state = Scenario::skirmish().build().unwrap();
    golden_check("skirmish-t0", &state);
}

#[test]
fn skirmish_midgame_matches_golden() {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
    }
    let outcome = runner::run_scenario(&scenario, 1200, true, false).unwrap();
    golden_check("skirmish-t1200", &outcome.state);
}
