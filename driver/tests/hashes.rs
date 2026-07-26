//! Fixed state-hash fixtures: the cheap, image-free determinism tripwire.
//!
//! Every shipped scenario runs bot-vs-bot to tick 2,000 and its state hash
//! is compared against `tests/goldens/state-hashes.json`. Any sim change
//! that moves behavior shows up here as a one-line diff instead of golden
//! PNG churn, and CI regenerates the file on every OS to prove the
//! cross-platform bit-identical invariant. When a change is intentional:
//! `BLESS=1 cargo test -p oxide-driver`, review the diff, commit, explain.

use oxide_driver::runner;
use oxide_sim::Scenario;
use std::collections::BTreeMap;
use std::path::PathBuf;

const FIXTURE_TICKS: u64 = 2_000;

fn compute_hashes() -> BTreeMap<String, String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    let paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    // Independent deterministic runs; the map keys restore a canonical
    // order whatever the thread finish order.
    let hashes: BTreeMap<String, String> = std::thread::scope(|scope| {
        let handles: Vec<_> = paths
            .iter()
            .map(|path| {
                scope.spawn(move || {
                    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
                    let mut scenario = Scenario::load(path)
                        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
                    for player in &mut scenario.players {
                        player.bot = true;
                        // The fixtures pin the *shipped* opponent: the
                        // neural ladder at full strength, personalities
                        // dealt from the map seed. Weight changes now trip
                        // the tripwire, exactly like rule changes — the
                        // network is part of the sim's behavior.
                        player.bot_config = Some(oxide_sim::scenario::BotConfig {
                            level: oxide_sim::bot::Level::Expert,
                            aggression: None,
                        });
                    }
                    let outcome = runner::run_scenario(&scenario, FIXTURE_TICKS, true, false)
                        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
                    (name, oxide_protocol::hash_hex(outcome.state.hash()))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a fixture run panicked"))
            .collect()
    });
    assert!(
        hashes.len() >= 5,
        "expected the shipped maps, found {}",
        hashes.len()
    );
    hashes
}

#[test]
fn shipped_scenarios_match_hash_fixtures() {
    let actual = compute_hashes();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/state-hashes.json");

    if std::env::var_os("BLESS").is_some() {
        let mut body = serde_json::to_string_pretty(&actual).unwrap();
        body.push('\n');
        std::fs::write(&fixture, body).unwrap();
        eprintln!("blessed {}", fixture.display());
        return;
    }
    let expected: BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(&fixture).unwrap_or_else(|_| {
            panic!(
                "missing fixture {} — run `BLESS=1 cargo test -p oxide-driver` and commit it",
                fixture.display()
            )
        }))
        .unwrap();
    assert_eq!(
        expected,
        actual,
        "state hashes drifted from {} — an unintended sim change, or re-bless deliberately",
        fixture.display()
    );
}
