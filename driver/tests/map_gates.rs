//! Map-audit gates: every shipped scenario must keep its pace label,
//! spawn fairness, and artillery pressure honest — the measuring stick
//! from `driver map-audit` turned into a tripwire. Terrain on existing
//! maps is frozen (hash fixtures move only by addition); these gates
//! bind labels and future maps, not history.

use oxide_driver::audit::audit;
use oxide_sim::Scenario;
use std::path::PathBuf;

fn shipped() -> Vec<(String, Scenario)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("scenarios dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("utf-8 name")
            .to_string();
        out.push((name, Scenario::load(&path).expect("shipped maps load")));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(out.len() >= 10, "the shipped roster is present");
    out
}

#[test]
fn every_map_carries_complete_metadata() {
    for (name, scenario) in shipped() {
        let meta = scenario
            .meta
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: shipped maps carry metadata"));
        assert!(!meta.hook.is_empty(), "{name}: hook missing");
        assert!(
            matches!(meta.pace.as_str(), "quick" | "standard" | "large" | "vast"),
            "{name}: pace '{}' is not a recognized label",
            meta.pace
        );
        assert!(!meta.mode.is_empty(), "{name}: mode missing");
        assert!(!meta.theme.is_empty(), "{name}: theme missing");
    }
}

#[test]
fn routes_connect_and_pace_labels_hold() {
    // Route bands per pace label, in ground BFS steps between Foundry
    // doorsteps. Disjoint on purpose: an overlapping band gates nothing.
    for (name, scenario) in shipped() {
        let report = audit(&scenario).expect("audit builds");
        let pace = scenario.meta.as_ref().unwrap().pace.clone();
        assert!(!report.routes.is_empty(), "{name}: no hostile pair routed");
        for route in &report.routes {
            let ground = route
                .ground_steps
                .unwrap_or_else(|| panic!("{name}: ground sealed"));
            route
                .air_tiles
                .unwrap_or_else(|| panic!("{name}: sky sealed"));
            // Bands in weighted tile-equivalents (the sim's own 14/10
            // diagonal costs) — recalibrated when the audit stopped
            // counting hops. Disjoint on purpose.
            let band = match pace.as_str() {
                "quick" => 8..=28,
                "standard" => 29..=52,
                "large" => 53..=90,
                // 0.10: matches should run tens of minutes — the vast
                // class exists to hold maps big enough to make it so.
                "vast" => 91..=150,
                other => panic!("{name}: unknown pace '{other}'"),
            };
            assert!(
                band.contains(&ground),
                "{name}: pace '{pace}' promises {band:?} ground steps, measured {ground}"
            );
        }
    }
}

#[test]
fn artillery_pressure_stays_bounded() {
    // Past ~0.5 the map is a siege range, not a battlefield. Quick
    // brawl maps tolerate more by design (Scrapyard is the honest
    // ceiling); standard and large must leave room to maneuver.
    for (name, scenario) in shipped() {
        let report = audit(&scenario).expect("audit builds");
        let pace = scenario.meta.as_ref().unwrap().pace.clone();
        let cap = if pace == "quick" { 0.65 } else { 0.50 };
        for route in &report.routes {
            let Some(pressure) = route.artillery_pressure else {
                continue;
            };
            assert!(
                pressure <= cap,
                "{name}: artillery pressure {pressure:.2} above the {cap} cap for '{pace}'"
            );
        }
    }
}

#[test]
fn spawns_are_fair_to_every_seat() {
    for (name, scenario) in shipped() {
        let report = audit(&scenario).expect("audit builds");
        let seats = &report.seats;
        let first = &seats[0];
        for seat in seats.iter().skip(1) {
            assert_eq!(
                seat.reachable_tiles, first.reachable_tiles,
                "{name}: seat {} rooms differently",
                seat.seat
            );
            assert_eq!(
                seat.nearest_enemy_route, first.nearest_enemy_route,
                "{name}: seat {} meets the enemy on a different clock",
                seat.seat
            );
        }
        // Scrap distance. Duels: the mirror seat measures identically,
        // full stop. 4p: same-parity seats (the measured-equal pairs on
        // the legacy 2v2s) hold strictly; across the parity split a
        // one-tile lean is tolerated on frozen terrain — rebuilt maps
        // should close it to zero.
        let gap = |a: usize, b: usize| (seats[a].nearest_scrap - seats[b].nearest_scrap).abs();
        match seats.len() {
            2 => assert!(gap(0, 1) <= 1e-9, "{name}: duel seats disagree on scrap"),
            4 => {
                assert!(gap(0, 2) <= 1e-9, "{name}: seats 0/2 disagree on scrap");
                assert!(gap(1, 3) <= 1e-9, "{name}: seats 1/3 disagree on scrap");
                assert!(
                    gap(0, 1) <= 1.5,
                    "{name}: scrap leans {:.2} across the team split",
                    gap(0, 1)
                );
            }
            // The 0.10 3v3/4v4 maps are built from identical lanes, so
            // every seat measures scrap identically — hold them to it.
            6 | 8 => {
                for i in 1..seats.len() {
                    assert!(
                        gap(0, i) <= 1e-9,
                        "{name}: seat {i} disagrees on scrap with seat 0"
                    );
                }
            }
            n => panic!("{name}: unexpected seat count {n}"),
        }
    }
}
