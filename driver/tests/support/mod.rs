//! Shared state-hash fixture plumbing: the golden shape, the bless gate,
//! and the check-or-bless driver used by every hash-fixture test binary.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Fixture {
    /// The `SIM_VERSION` these hashes were blessed under.
    pub sim_version: String,
    pub hashes: BTreeMap<String, String>,
}

/// The bless discipline as a pure decision: same-version hash movement
/// on an existing row refuses unless explicitly overridden. A missing,
/// pre-stamp, or other-version fixture licenses the bless; new and
/// removed rows never block (maps come and go without a version story).
pub fn bless_gate(
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

/// Compares computed rows against a golden, or rewrites the golden under
/// `BLESS=1` after the gate agrees. Shared verbatim by every hash fixture
/// so one bless discipline governs them all.
pub fn check_or_bless(fixture: &Path, actual: BTreeMap<String, String>) {
    if std::env::var_os("BLESS").is_some() {
        // An absent fixture or the recognized pre-stamp shape (a plain
        // name-to-hash map) blesses freely — those are the one-time
        // migration paths. Anything else that fails to parse is a
        // CORRUPT fixture, and blessing over it would bypass the drift
        // gate; refuse instead. A parseable one gates: same-version
        // hash movement on an existing row is behavior drift wearing a
        // stale version number.
        let stored: Option<Fixture> = match std::fs::read_to_string(fixture) {
            Err(_) => None,
            Ok(raw) => match serde_json::from_str::<Fixture>(&raw) {
                Ok(parsed) => Some(parsed),
                Err(_) if serde_json::from_str::<BTreeMap<String, String>>(&raw).is_ok() => None,
                Err(err) => panic!(
                    "fixture {} is corrupt ({err}) — refusing to bless over it; \
                     inspect or restore it from git first",
                    fixture.display()
                ),
            },
        };
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
        std::fs::write(fixture, body).unwrap();
        eprintln!("blessed {}", fixture.display());
        return;
    }

    let raw = std::fs::read_to_string(fixture).unwrap_or_else(|_| {
        panic!(
            "missing fixture {} — run `BLESS=1 cargo test -p oxide-driver` and commit it",
            fixture.display()
        )
    });
    let expected: Fixture = serde_json::from_str(&raw).unwrap_or_else(|err| {
        panic!(
            "fixture {} lacks its sim_version stamp: {err} — \
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
