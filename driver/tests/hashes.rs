//! Focused rule contracts with cross-platform state hashes.
//! Behavioral premises and results are checked before any golden can be blessed.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use support::{Fixture, bless_gate, check_or_bless};

#[path = "support/contracts.rs"]
mod contracts;

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
        err.contains("explicit approval from the human user"),
        "the refusal must direct agents to the human compatibility decision: {err}"
    );
    assert!(
        bless_gate(Some(&stored), &actual, true).is_ok(),
        "the explicit same-version override must license the bless"
    );
}

#[test]
fn a_different_version_or_fresh_fixture_licenses_the_bless() {
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
fn simulation_contracts_match_hash_fixtures() {
    let actual = contracts::compute_hashes();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/state-hashes.json");
    check_or_bless(&fixture, actual);
}
