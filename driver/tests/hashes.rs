//! Focused rule contracts with cross-platform state hashes.
//! Behavioral premises and results are checked before any golden can be blessed.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use support::{Fixture, check_or_bless};

#[path = "support/contracts.rs"]
mod contracts;

/// The bless discipline as a pure decision: same-version hash movement on an
/// existing row refuses unless overridden. A missing or other-version fixture
/// licenses the bless; new and removed rows never block (maps come and go
/// without a version change).
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
        "refusing to bless: {} fixture hash(es) moved ({}) at SIM_VERSION {}. \
         Oxide is pre-launch: re-bless at the same version with BLESS_SAME_VERSION=1 \
         and name the moved rows and the reason in the PR (docs/versioning.md).",
        drifted.len(),
        drifted.join(", "),
        oxide_sim::SIM_VERSION,
    ))
}

#[test]
fn same_version_hash_movement_refuses_the_bless() {
    let stored = Fixture {
        sim_version: oxide_sim::SIM_VERSION,
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
        err.contains("BLESS_SAME_VERSION=1"),
        "the refusal must name the same-version override: {err}"
    );
    assert!(
        bless_gate(Some(&stored), &actual, true).is_ok(),
        "the explicit same-version override must license the bless"
    );
}

#[test]
fn a_different_version_or_fresh_fixture_licenses_the_bless() {
    let stored = Fixture {
        sim_version: oxide_sim::SIM_VERSION + 1,
        hashes: BTreeMap::from([("skirmish".to_string(), "aaaa".to_string())]),
    };
    let actual = BTreeMap::from([("skirmish".to_string(), "bbbb".to_string())]);
    assert!(bless_gate(Some(&stored), &actual, false).is_ok());
    assert!(bless_gate(None, &actual, false).is_ok());

    let same_version_same_hashes = Fixture {
        sim_version: oxide_sim::SIM_VERSION,
        hashes: actual.clone(),
    };
    assert!(
        bless_gate(Some(&same_version_same_hashes), &actual, false).is_ok(),
        "a wrapper-only or row-only re-bless within one version is legitimate"
    );
}

#[test]
fn simulation_contracts_match_hash_fixtures() {
    let actual = contracts::compute_hashes();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/state-hashes.json");
    check_or_bless(&fixture, actual, |stored, actual| {
        bless_gate(
            stored,
            actual,
            std::env::var_os("BLESS_SAME_VERSION").is_some(),
        )
    });
}
