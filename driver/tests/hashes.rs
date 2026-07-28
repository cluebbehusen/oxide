//! Fixed state-hash fixtures: the cheap, image-free determinism tripwire.
//!
//! Every shipped scenario runs bot-vs-bot to tick 2,000 and its state hash
//! is compared against `tests/goldens/state-hashes.json`. Any sim change
//! that moves behavior shows up here as a one-line diff instead of golden
//! PNG churn, and CI regenerates the file on every OS to prove the
//! cross-platform bit-identical invariant.
//!
//! The fixture carries the `SIM_VERSION` it was blessed under, and the
//! bless path enforces the compatibility discipline mechanically: hash
//! movement while the version stands still is a behavior change wearing
//! last release's number — an old binary would silently reconstruct a
//! different world from the same replay. Bump the workspace version
//! first, then `BLESS=1 cargo test -p oxide-driver`, review the diff,
//! commit, explain. `BLESS_SAME_VERSION=1` overrides the refusal for a
//! deliberate exception; the commit message owns the justification.

use oxide_driver::runner;
use oxide_sim::Scenario;
use std::collections::BTreeMap;
use std::path::PathBuf;

const FIXTURE_TICKS: u64 = 2_000;

#[derive(serde::Serialize, serde::Deserialize)]
struct Fixture {
    /// The `SIM_VERSION` these hashes were blessed under.
    sim_version: String,
    hashes: BTreeMap<String, String>,
}

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

/// The bless discipline as a pure decision: same-version hash movement
/// on an existing row refuses unless explicitly overridden. A missing,
/// pre-stamp, or other-version fixture licenses the bless; new and
/// removed rows never block (maps come and go without a version story).
fn bless_gate(
    stored: Option<&Fixture>,
    actual: &BTreeMap<String, String>,
    override_on: bool,
) -> Result<(), String> {
    let Some(stored) = stored else {
        return Ok(());
    };
    if stored.sim_version != oxide_sim::SIM_VERSION || override_on {
        return Ok(());
    }
    let drifted: Vec<&str> = stored
        .hashes
        .iter()
        .filter(|(name, hash)| actual.get(*name).is_some_and(|a| a != *hash))
        .map(|(name, _)| name.as_str())
        .collect();
    if drifted.is_empty() {
        return Ok(());
    }
    Err(format!(
        "refusing to bless: {} fixture hash(es) moved ({}) but SIM_VERSION is still {} \
         — a behavior change must carry its cycle's workspace version, or an old binary \
         reconstructs a different world from the same replay. Bump the version in \
         Cargo.toml first, or set BLESS_SAME_VERSION=1 for a deliberate exception and \
         justify it in the commit message.",
        drifted.len(),
        drifted.join(", "),
        oxide_sim::SIM_VERSION,
    ))
}

#[test]
fn same_version_hash_movement_refuses_the_bless() {
    let stored = Fixture {
        sim_version: oxide_sim::SIM_VERSION.to_string(),
        hashes: BTreeMap::from([
            ("skirmish".to_string(), "aaaa".to_string()),
            ("retired-map".to_string(), "cccc".to_string()),
        ]),
    };
    let actual = BTreeMap::from([
        ("skirmish".to_string(), "bbbb".to_string()),
        ("brand-new-map".to_string(), "dddd".to_string()),
    ]);
    let err = bless_gate(Some(&stored), &actual, false).unwrap_err();
    assert!(err.contains("skirmish"), "{err}");
    assert!(
        !err.contains("retired-map") && !err.contains("brand-new-map"),
        "row additions and removals must never block: {err}"
    );
    assert!(
        bless_gate(Some(&stored), &actual, true).is_ok(),
        "the explicit override must license the bless"
    );
}

#[test]
fn a_version_bump_or_fresh_fixture_licenses_the_bless() {
    let stored = Fixture {
        sim_version: "0.0.1-not-this-version".to_string(),
        hashes: BTreeMap::from([("skirmish".to_string(), "aaaa".to_string())]),
    };
    let actual = BTreeMap::from([("skirmish".to_string(), "bbbb".to_string())]);
    assert!(bless_gate(Some(&stored), &actual, false).is_ok());
    assert!(bless_gate(None, &actual, false).is_ok());

    let same_version_same_hashes = Fixture {
        sim_version: oxide_sim::SIM_VERSION.to_string(),
        hashes: actual.clone(),
    };
    assert!(
        bless_gate(Some(&same_version_same_hashes), &actual, false).is_ok(),
        "a wrapper-only or row-only re-bless within one version is legitimate"
    );
}

#[test]
fn shipped_scenarios_match_hash_fixtures() {
    let actual = compute_hashes();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/state-hashes.json");

    if std::env::var_os("BLESS").is_some() {
        // A pre-stamp or absent fixture parses to None and blesses
        // freely — that is the one-time migration path. A parseable one
        // gates: same-version hash movement on an existing row is
        // behavior drift wearing a stale version number.
        let stored: Option<Fixture> = std::fs::read_to_string(&fixture)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok());
        let override_on = std::env::var_os("BLESS_SAME_VERSION").is_some();
        if let Err(refusal) = bless_gate(stored.as_ref(), &actual, override_on) {
            panic!("{refusal}");
        }
        let blessed = Fixture {
            sim_version: oxide_sim::SIM_VERSION.to_string(),
            hashes: actual,
        };
        let mut body = serde_json::to_string_pretty(&blessed).unwrap();
        body.push('\n');
        std::fs::write(&fixture, body).unwrap();
        eprintln!("blessed {}", fixture.display());
        return;
    }

    let raw = std::fs::read_to_string(&fixture).unwrap_or_else(|_| {
        panic!(
            "missing fixture {} — run `BLESS=1 cargo test -p oxide-driver` and commit it",
            fixture.display()
        )
    });
    let expected: Fixture = serde_json::from_str(&raw).unwrap_or_else(|err| {
        panic!(
            "fixture {} lacks its sim_version stamp (pre-0.13 shape?): {err} — \
             re-bless with `BLESS=1 cargo test -p oxide-driver`",
            fixture.display()
        )
    });
    assert_eq!(
        expected.sim_version,
        oxide_sim::SIM_VERSION,
        "fixture was blessed under sim {} but the workspace is {} — re-verify the \
         hashes and re-bless so the stamp tells the truth",
        expected.sim_version,
        oxide_sim::SIM_VERSION,
    );
    assert_eq!(
        expected.hashes,
        actual,
        "state hashes drifted from {} — an unintended sim change, or re-bless deliberately",
        fixture.display()
    );
}
