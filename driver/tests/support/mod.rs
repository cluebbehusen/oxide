//! Shared state-hash fixture plumbing: the golden shape and the
//! check-or-bless driver used by every hash-fixture test binary.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Fixture {
    /// The `SIM_VERSION` these hashes were blessed under.
    pub sim_version: u32,
    pub hashes: BTreeMap<String, String>,
}

/// Compares computed rows against a golden, or rewrites the golden under
/// `BLESS=1` once `gate` accepts the move from the stored rows.
pub fn check_or_bless(
    fixture: &Path,
    actual: BTreeMap<String, String>,
    gate: impl FnOnce(Option<&Fixture>, &BTreeMap<String, String>) -> Result<(), String>,
) {
    if std::env::var_os("BLESS").is_some() {
        // Blessing over a fixture that exists but does not parse would
        // bypass the drift gate.
        let stored: Option<Fixture> = std::fs::read_to_string(fixture).ok().map(|raw| {
            serde_json::from_str(&raw).unwrap_or_else(|err| {
                panic!(
                    "fixture {} is corrupt ({err}); refusing to bless over it, \
                     inspect or restore it from git first",
                    fixture.display()
                )
            })
        });
        if let Err(refusal) = gate(stored.as_ref(), &actual) {
            panic!("{refusal}");
        }
        let blessed = Fixture {
            sim_version: oxide_sim::SIM_VERSION,
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
            "fixture {} does not parse: {err}; \
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
