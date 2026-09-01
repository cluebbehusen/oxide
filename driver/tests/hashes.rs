//! Fixed state-hash fixtures: the cheap, image-free determinism tripwire.
//!
//! Every shipped scenario runs Overseer-vs-Overseer to two horizons and
//! both state hashes are compared against `tests/goldens/state-hashes.json`.
//! Any sim change that moves behavior shows up here as a one-line diff
//! instead of golden PNG churn, and CI regenerates the file on every OS to
//! prove the cross-platform bit-identical invariant.
//!
//! Two horizons because they see different eras: tick 2,000 is a cheap
//! opening-and-economy tripwire, but Overseer-vs-Overseer matches resolve
//! between roughly 5,600 and 36,000 ticks, so combat-phase drift (target
//! selection, engagement radii, producer order under pressure) only moves
//! the 6,000-tick rows. The 2,000-tick rows keep their original bare-name
//! keys; the late rows are keyed `<map>@6000`.
//!
//! The fixture carries the `SIM_VERSION` it was blessed under, and the
//! bless path refuses same-version movement mechanically. When existing
//! rows move, inspect the drift and ask the user to choose the compatibility
//! policy. Changing the workspace version or using `BLESS_SAME_VERSION=1`
//! requires explicit approval from the human user; implementing a simulation
//! change does not imply either approval.

mod support;

use oxide_sim::Scenario;
use std::collections::BTreeMap;
use std::path::PathBuf;
use support::{Fixture, bless_gate, check_or_bless};

const FIXTURE_TICKS: u64 = 2_000;
const LATE_FIXTURE_TICKS: u64 = 6_000;

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
                    let scenario = Scenario::load(path)
                        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
                    // The fixtures pin the stable Overseer in every
                    // seat so player-facing bot tuning cannot move this
                    // rule-and-map tripwire accidentally.
                    let mut state = scenario
                        .build()
                        .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
                    let mut bots: Vec<oxide_sim::bot::Brain> = (0..scenario.players.len())
                        .map(|seat| {
                            oxide_sim::bot::Brain::overseer(
                                oxide_sim::PlayerId(seat as u8),
                                scenario.seed,
                            )
                        })
                        .collect();
                    for _ in 0..FIXTURE_TICKS {
                        let mut commands = Vec::new();
                        for bot in &mut bots {
                            commands.extend(bot.act(&state));
                        }
                        state.tick(&commands);
                    }
                    let early = oxide_protocol::hash_hex(state.hash());
                    for _ in FIXTURE_TICKS..LATE_FIXTURE_TICKS {
                        let mut commands = Vec::new();
                        for bot in &mut bots {
                            commands.extend(bot.act(&state));
                        }
                        state.tick(&commands);
                    }
                    let late = oxide_protocol::hash_hex(state.hash());
                    [
                        (name.clone(), early),
                        (format!("{name}@{LATE_FIXTURE_TICKS}"), late),
                    ]
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("a fixture run panicked"))
            .collect()
    });
    assert!(
        hashes.len() >= 10,
        "expected two rows per shipped map, found {}",
        hashes.len()
    );
    hashes
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
fn shipped_scenarios_match_hash_fixtures() {
    let actual = compute_hashes();
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/state-hashes.json");
    check_or_bless(&fixture, actual);
}
