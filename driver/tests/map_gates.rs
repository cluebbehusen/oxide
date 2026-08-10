//! Map-audit gates: every shipped scenario must keep its pace label,
//! spawn fairness, artillery pressure, and mirrored authoring honest —
//! the measuring stick from `driver map-audit` turned into a tripwire.
//! Terrain on existing maps is frozen (hash fixtures move only by
//! addition); these gates bind labels and future maps, not history.

use chassis::grid::TilePos;
use oxide_driver::audit::audit;
use oxide_sim::map::Map;
use oxide_sim::{BuildingKind, PlayerId, Scenario};
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
fn every_map_seats_a_human_and_live_opponents() {
    // Seat 0 is the human chair; every other seat must actually play.
    // Continental Divide once shipped with both seats bot:false — an
    // advertised 1v1 whose opponent never harvested, trained, or moved.
    for (name, scenario) in shipped() {
        assert!(
            !scenario.players[0].bot,
            "{name}: seat 0 is the human chair"
        );
        for (i, seat) in scenario.players.iter().enumerate().skip(1) {
            assert!(seat.bot, "{name}: seat {i} is a dead chair (bot: false)");
        }
    }
}

#[test]
fn every_map_carries_complete_metadata() {
    for (name, scenario) in shipped() {
        let meta = scenario
            .meta
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: shipped maps carry metadata"));
        assert!(!meta.hook.is_empty(), "{name}: hook missing");
        if scenario.players.len() == 2 {
            assert!(
                !meta.duration.is_empty(),
                "{name}: 1v1 duration measurement missing"
            );
        }
        assert!(
            matches!(meta.pace.as_str(), "quick" | "standard" | "large" | "vast"),
            "{name}: pace '{}' is not a recognized label",
            meta.pace
        );
        assert!(!meta.mode.is_empty(), "{name}: mode missing");
        assert!(
            matches!(meta.richness.as_str(), "lean" | "standard" | "rich"),
            "{name}: richness '{}' is not a recognized label (an empty \
             one once rendered a dangling badge separator in the map list)",
            meta.richness
        );
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
                // 0.15: the grand class — island wars, 12-seat FFAs,
                // maps whose matches are campaigns.
                "grand" => 151..=400,
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
    // The caps preserve the same minimum-route floors the 0.10 caps
    // enforced with the Bombard's 9.5 reach (quick >= ~14.6 steps,
    // everything else >= 19): the 0.15 Avalanche stretched the longest
    // reach to 14, which rescales the ratio, not the geometry the maps
    // must keep. A tier-three siege piece on a knife map is a late
    // commitment, not the opening problem the old cap policed.
    for (name, scenario) in shipped() {
        let report = audit(&scenario).expect("audit builds");
        let pace = scenario.meta.as_ref().unwrap().pace.clone();
        let cap = if pace == "quick" { 0.96 } else { 0.74 };
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
        let symmetry = scenario
            .meta
            .as_ref()
            .map(|m| m.symmetry.clone())
            .unwrap_or_default();
        if symmetry == "metric" {
            metric_fairness(&name, seats);
            continue;
        }
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
            // 0.15 seat counts beyond the legacy lanes: mirrored maps
            // still hold room and clock exactly (asserted above); scrap
            // holds within the cross-parity lean the 4p rule tolerates.
            _ => {
                for i in 1..seats.len() {
                    assert!(
                        gap(0, i) <= 1.5,
                        "{name}: scrap leans {:.2} against seat {i}",
                        gap(0, i)
                    );
                }
            }
        }
    }
}

/// The free-for-all fairness class: no tile mirror to lean on, so every
/// seat's measured position must sit inside a tolerance of the field's
/// mean — room within 5%, first-contact clock within 15%, scrap within
/// 2.5 tiles, and (when the map has frames at all) extractor access
/// within 20%.
fn metric_fairness(name: &str, seats: &[oxide_driver::audit::SeatAudit]) {
    let mean = |xs: &[f64]| xs.iter().sum::<f64>() / xs.len() as f64;
    let rooms: Vec<f64> = seats.iter().map(|s| s.reachable_tiles as f64).collect();
    let room_mean = mean(&rooms);
    for s in seats {
        let dev = (s.reachable_tiles as f64 - room_mean).abs() / room_mean;
        assert!(
            dev <= 0.05,
            "{name}: seat {} rooms {:.1}% off the field",
            s.seat,
            dev * 100.0
        );
    }
    let clocks: Vec<f64> = seats
        .iter()
        .map(|s| {
            s.nearest_enemy_route
                .unwrap_or_else(|| panic!("{name}: seat {} sealed off", s.seat)) as f64
        })
        .collect();
    let clock_mean = mean(&clocks);
    for (s, clock) in seats.iter().zip(&clocks) {
        let dev = (clock - clock_mean).abs() / clock_mean;
        assert!(
            dev <= 0.15,
            "{name}: seat {} meets the enemy {:.1}% off the field's clock",
            s.seat,
            dev * 100.0
        );
    }
    let scrap_mean = mean(&seats.iter().map(|s| s.nearest_scrap).collect::<Vec<_>>());
    for s in seats {
        assert!(
            (s.nearest_scrap - scrap_mean).abs() <= 2.5,
            "{name}: seat {} digs {:.2} tiles off the field's scrap",
            s.seat,
            (s.nearest_scrap - scrap_mean).abs()
        );
    }
    if seats.iter().all(|s| s.nearest_extractor.is_some()) {
        let frames: Vec<f64> = seats.iter().map(|s| s.nearest_extractor.unwrap()).collect();
        let frame_mean = mean(&frames);
        for (s, d) in seats.iter().zip(&frames) {
            let dev = (d - frame_mean).abs() / frame_mean.max(1.0);
            assert!(
                dev <= 0.20,
                "{name}: seat {} reaches its extractor {:.1}% off the field",
                s.seat,
                dev * 100.0
            );
        }
    }
}

#[test]
fn every_map_mirrors_its_paired_seats_entry_by_entry() {
    // The metric class opts out: its fairness is measured, not
    // mirrored (see `metric_fairness`).
    // The authoring rule the 0.7 mirror bug broke: a paired seat's
    // starting units must be the entry-by-entry 180-degree image of its
    // partner's, because ids are handed out in list order and every
    // id-order tie-break downstream inherits that order.
    //
    // The pairing is derived from the map, not assumed: rotating a
    // Foundry anchor 180 degrees lands on exactly one other anchor, and
    // that relation is an involution. It reads {0<->1} on duels,
    // {0<->3, 1<->2} or {0<->2, 1<->3} on the 4p maps, and
    // {i <-> n-1-i} on the 6p/8p lane stacks — one rule for all of them.
    //
    // Kinds compare by Role, not by kind: a launch-time retint and any
    // future faction-varied starting unit must still read as a mirror.
    for (name, scenario) in shipped() {
        if scenario
            .meta
            .as_ref()
            .is_some_and(|m| m.symmetry == "metric")
        {
            continue;
        }
        let (map, anchors) =
            Map::parse(&scenario.map).unwrap_or_else(|e| panic!("{name}: map parses ({e})"));
        let (w, h) = (map.width(), map.height());

        // Anchor digits are already ground by the time Map::parse is
        // done, so this is the terrain symmetry the pairing rests on —
        // scrap amounts and cosmetic rubble included.
        for (pos, tile) in map.iter() {
            let image = TilePos {
                x: w - 1 - pos.x,
                y: h - 1 - pos.y,
            };
            assert_eq!(
                map.tile(image),
                Some(tile),
                "{name}: tile ({}, {}) is not the image of its mirror",
                pos.x,
                pos.y
            );
        }

        let (fw, fh) = BuildingKind::Foundry.base_stats().size;
        let anchor = |seat: PlayerId| {
            anchors
                .iter()
                .find(|(p, _)| *p == seat)
                .map(|(_, at)| *at)
                .unwrap_or_else(|| panic!("{name}: seat {} has no Foundry anchor", seat.0))
        };
        // An anchor names the footprint's top-left, so its image sits a
        // footprint in from the rotated corner.
        let partner = |seat: PlayerId| {
            let at = anchor(seat);
            let image = TilePos {
                x: w - fw - at.x,
                y: h - fh - at.y,
            };
            anchors
                .iter()
                .find(|(_, a)| *a == image)
                .map(|(p, _)| *p)
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: seat {}'s anchor rotates onto ({}, {}), where no seat sits",
                        seat.0, image.x, image.y
                    )
                })
        };

        let state = scenario.build().expect("shipped maps build");
        for index in 0..scenario.players.len() {
            let seat = PlayerId(index as u8);
            let mirror = partner(seat);
            assert_ne!(mirror, seat, "{name}: seat {index} is its own mirror");
            assert_eq!(
                partner(mirror),
                seat,
                "{name}: the anchor pairing is not an involution at seat {index}"
            );
            // A seat mirroring its own teammate would hand one team both
            // halves of the map's symmetry and leave the other pair
            // fighting across it.
            assert!(
                state.hostile(seat, mirror),
                "{name}: seat {index} mirrors teammate seat {}",
                mirror.0
            );
            assert_eq!(
                state.players()[index].scrap,
                state.players()[mirror.0 as usize].scrap,
                "{name}: seat {index} banks differently from its mirror"
            );

            let mine: Vec<_> = scenario
                .units
                .iter()
                .filter(|u| u.player == seat.0)
                .collect();
            let theirs: Vec<_> = scenario
                .units
                .iter()
                .filter(|u| u.player == mirror.0)
                .collect();
            assert_eq!(
                mine.len(),
                theirs.len(),
                "{name}: seat {index} starts {} units against its mirror's {}",
                mine.len(),
                theirs.len()
            );
            for (k, (a, b)) in mine.iter().zip(&theirs).enumerate() {
                assert_eq!(
                    (b.x, b.y),
                    (w - 1 - a.x, h - 1 - a.y),
                    "{name}: seat {index}'s unit #{k} is not placed opposite its mirror's",
                );
                assert_eq!(
                    a.kind.role(),
                    b.kind.role(),
                    "{name}: seat {index}'s unit #{k} fills a different role than its mirror's"
                );
            }

            let mine: Vec<_> = scenario
                .buildings
                .iter()
                .filter(|b| b.player == seat.0)
                .collect();
            let theirs: Vec<_> = scenario
                .buildings
                .iter()
                .filter(|b| b.player == mirror.0)
                .collect();
            assert_eq!(
                mine.len(),
                theirs.len(),
                "{name}: seat {index} starts a different number of structures than its mirror"
            );
            for (k, (a, b)) in mine.iter().zip(&theirs).enumerate() {
                assert_eq!(
                    a.kind, b.kind,
                    "{name}: seat {index}'s structure #{k} differs in kind from its mirror's"
                );
                let (bw, bh) = a.kind.base_stats().size;
                assert_eq!(
                    (b.x, b.y),
                    (w - bw - a.x, h - bh - a.y),
                    "{name}: seat {index}'s structure #{k} is not placed opposite its mirror's"
                );
            }
        }
    }
}
