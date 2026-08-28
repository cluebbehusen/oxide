//! Map-audit gates: every shipped scenario must keep its pace label,
//! spawn fairness, artillery pressure, and mirrored authoring honest —
//! the measuring stick from `driver map-audit` turned into a tripwire.
//! Terrain on existing maps is frozen (hash fixtures move only by
//! addition); these gates bind labels and future maps, not history.

use chassis::grid::TilePos;
use oxide_driver::audit::audit;
use oxide_sim::map::Map;
use oxide_sim::scenario::BotConfig;
use oxide_sim::{BuildingKind, PlayerId, Scenario, State};
use std::collections::VecDeque;
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
        assert!(
            scenario.players[0].bot_config.is_none(),
            "{name}: human chair must not carry a bot configuration"
        );
        for (i, seat) in scenario.players.iter().enumerate().skip(1) {
            assert!(seat.bot, "{name}: seat {i} is a dead chair (bot: false)");
            assert!(
                seat.bot_config == Some(BotConfig::default()),
                "{name}: seat {i} is not the shipped scripted opponent"
            );
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
        assert!(
            matches!(
                meta.pace.as_str(),
                "quick" | "standard" | "large" | "vast" | "grand"
            ),
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
    // Route bands per pace label, in effective steps between Foundry
    // doorsteps: ground BFS steps, or the air detour on island pairs
    // that no ground route serves (the sim's connectivity gate already
    // guarantees SOME mover connects every pair). Disjoint on purpose:
    // an overlapping band gates nothing.
    for (name, scenario) in shipped() {
        let report = audit(&scenario).expect("audit builds");
        let meta = scenario.meta.as_ref().unwrap();
        let pace = meta.pace.clone();
        let metric = meta.symmetry == "metric";
        assert!(!report.routes.is_empty(), "{name}: no hostile pair routed");
        let mut min_effective = usize::MAX;
        for route in &report.routes {
            route
                .air_tiles
                .unwrap_or_else(|| panic!("{name}: sky sealed"));
            let effective = route
                .effective_steps()
                .unwrap_or_else(|| panic!("{name}: no mover routes the pair"));
            // Bands in weighted tile-equivalents (the sim's own 14/10
            // diagonal costs) — recalibrated when the audit stopped
            // counting hops. Disjoint on purpose.
            let band = match pace.as_str() {
                "quick" => 8..=28,
                "standard" => 29..=52,
                "large" => 53..=90,
                // Vast maps deliberately support long travel and buildup.
                "vast" => 91..=150,
                // Grand maps cover island wars and very large FFAs.
                "grand" => 151..=400,
                other => panic!("{name}: unknown pace '{other}'"),
            };
            min_effective = min_effective.min(effective);
            if metric {
                // A free-for-all ring spans near and far neighbors by
                // construction; the pace label is the FIRST-contact
                // clock, so the floor binds every pair and the band
                // binds the nearest one (checked after the loop).
                assert!(
                    effective >= *band.start(),
                    "{name}: pace '{pace}' floor {} broken by a {effective}-step pair",
                    band.start()
                );
            } else {
                assert!(
                    band.contains(&effective),
                    "{name}: pace '{pace}' promises {band:?} effective steps, measured {effective}"
                );
            }
        }
        if metric {
            let band = match pace.as_str() {
                "quick" => 8..=28,
                "standard" => 29..=52,
                "large" => 53..=90,
                "vast" => 91..=150,
                "grand" => 151..=400,
                other => panic!("{name}: unknown pace '{other}'"),
            };
            assert!(
                band.contains(&min_effective),
                "{name}: pace '{pace}' promises first contact in {band:?}, measured {min_effective}"
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
fn renewable_economy_prototypes_offer_one_home_claim_and_a_forward_choice() {
    const PROTOTYPES: [&str; 3] = ["cinder-steppe", "skirmish", "the-scattering"];
    let support_radius = oxide_sim::stats::EXTRACTOR_SUPPORT_RADIUS;
    let prototypes: Vec<_> = shipped()
        .into_iter()
        .filter(|(name, _)| PROTOTYPES.contains(&name.as_str()))
        .collect();
    let found: Vec<_> = prototypes.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        found, PROTOTYPES,
        "the renewable-economy gate must exercise every promoted prototype"
    );

    for (name, scenario) in prototypes {
        let (map, foundries) =
            Map::parse(&scenario.map).unwrap_or_else(|error| panic!("{name}: {error}"));
        let frames = map.extractor_frames();
        assert!(
            frames.len() >= scenario.players.len() * 2,
            "{name}: each seat needs a home claim and at least one forward option"
        );

        let state = scenario.build().expect("prototype map builds");
        for (seat, foundry) in foundries {
            let mut distances: Vec<(i32, TilePos)> = frames
                .iter()
                .copied()
                .map(|frame| (extractor_foundry_distance(foundry, frame), frame))
                .collect();
            distances.sort_unstable();
            let home: Vec<_> = distances
                .iter()
                .filter(|(distance, _)| *distance <= support_radius)
                .collect();
            assert_eq!(
                home.len(),
                1,
                "{name}: seat {} needs exactly one frame supported by its starting Foundry: {distances:?}",
                seat.0
            );
            let home = home[0].1;
            for dy in 0..2 {
                for dx in 0..2 {
                    assert!(
                        state.vision(seat).visible(home.offset(dx, dy)),
                        "{name}: seat {} cannot see its opening Extractor frame",
                        seat.0
                    );
                }
            }
            let home_reachable = reachable_builder_ground(&map, &state, seat, home);
            assert!(
                reachable_rect_perimeter(
                    &map,
                    home,
                    BuildingKind::Extractor.base_stats().size,
                    &home_reachable,
                ),
                "{name}: seat {} cannot reach its opening Extractor frame at ({}, {})",
                seat.0,
                home.x,
                home.y
            );
            let forward: Vec<_> = distances
                .iter()
                .filter(|(distance, _)| {
                    *distance > support_radius && *distance <= support_radius + 8
                })
                .map(|(_, frame)| *frame)
                .collect();
            assert!(
                !forward.is_empty(),
                "{name}: seat {} needs a nearby but unsupported natural frame: {distances:?}",
                seat.0
            );
            for frame in forward {
                let reachable = reachable_builder_ground(&map, &state, seat, frame);
                assert!(
                    reachable_rect_perimeter(
                        &map,
                        frame,
                        BuildingKind::Extractor.base_stats().size,
                        &reachable,
                    ),
                    "{name}: seat {} cannot reach its natural Extractor frame at ({}, {})",
                    seat.0,
                    frame.x,
                    frame.y
                );
                assert!(
                    supportable_foundry_anchor(&map, &state, frame, &reachable, support_radius)
                        .is_some(),
                    "{name}: seat {} cannot reach a legal Foundry site that supports forward frame ({}, {})",
                    seat.0,
                    frame.x,
                    frame.y
                );
            }
        }
    }
}

fn reachable_rect_perimeter(
    map: &Map,
    anchor: TilePos,
    size: (i32, i32),
    reachable: &[bool],
) -> bool {
    (anchor.y - 1..=anchor.y + size.1).any(|y| {
        (anchor.x - 1..=anchor.x + size.0).any(|x| {
            let inside =
                x >= anchor.x && x < anchor.x + size.0 && y >= anchor.y && y < anchor.y + size.1;
            !inside
                && x >= 0
                && y >= 0
                && x < map.width()
                && y < map.height()
                && reachable[(y * map.width() + x) as usize]
        })
    })
}

fn extractor_foundry_distance(foundry: TilePos, extractor: TilePos) -> i32 {
    let foundry_size = BuildingKind::Foundry.base_stats().size;
    let extractor_size = BuildingKind::Extractor.base_stats().size;
    let axis = |a: i32, a_len: i32, b: i32, b_len: i32| {
        let a_far = a + a_len - 1;
        let b_far = b + b_len - 1;
        (a - b_far).max(b - a_far).max(0)
    };
    axis(foundry.x, foundry_size.0, extractor.x, extractor_size.0).max(axis(
        foundry.y,
        foundry_size.1,
        extractor.y,
        extractor_size.1,
    ))
}

fn reachable_builder_ground(
    map: &Map,
    state: &State,
    seat: PlayerId,
    restored_frame: TilePos,
) -> Vec<bool> {
    let index = |tile: TilePos| (tile.y * map.width() + tile.x) as usize;
    let blocked = |tile: TilePos| {
        rect_contains(
            restored_frame,
            BuildingKind::Extractor.base_stats().size,
            tile,
        ) || state
            .buildings()
            .iter()
            .any(|building| building.contains(tile))
    };
    let passable = |tile: TilePos| map.terrain_passable(tile) && !blocked(tile);
    let mut reachable = vec![false; (map.width() * map.height()) as usize];
    let mut queue = VecDeque::new();
    for unit in state
        .units()
        .iter()
        .filter(|unit| unit.player == seat && unit.kind.stats().harvest.is_some())
    {
        let tile = unit.tile();
        if passable(tile) && !reachable[index(tile)] {
            reachable[index(tile)] = true;
            queue.push_back(tile);
        }
    }
    assert!(
        !queue.is_empty(),
        "seat {} needs a live starting builder for the forward-economy proof",
        seat.0
    );

    while let Some(tile) = queue.pop_front() {
        for (dx, dy) in chassis::grid::CARDINALS {
            let next = tile.offset(dx, dy);
            if passable(next) && !reachable[index(next)] {
                reachable[index(next)] = true;
                queue.push_back(next);
            }
        }
    }
    reachable
}

fn supportable_foundry_anchor(
    map: &Map,
    state: &State,
    frame: TilePos,
    reachable: &[bool],
    support_radius: i32,
) -> Option<TilePos> {
    let foundry_size = BuildingKind::Foundry.base_stats().size;
    for y in 0..map.height() {
        for x in 0..map.width() {
            let anchor = TilePos::new(x, y);
            if extractor_foundry_distance(anchor, frame) > support_radius
                || !foundry_site_is_legal(map, state, anchor)
            {
                continue;
            }
            let doorstep_reached = (anchor.y - 1..=anchor.y + foundry_size.1).any(|door_y| {
                (anchor.x - 1..=anchor.x + foundry_size.0).any(|door_x| {
                    let inside = door_x >= anchor.x
                        && door_x < anchor.x + foundry_size.0
                        && door_y >= anchor.y
                        && door_y < anchor.y + foundry_size.1;
                    if inside
                        || door_x < 0
                        || door_y < 0
                        || door_x >= map.width()
                        || door_y >= map.height()
                    {
                        return false;
                    }
                    reachable[(door_y * map.width() + door_x) as usize]
                })
            });
            if doorstep_reached {
                return Some(anchor);
            }
        }
    }
    None
}

fn foundry_site_is_legal(map: &Map, state: &State, anchor: TilePos) -> bool {
    let (width, height) = BuildingKind::Foundry.base_stats().size;
    (0..height).all(|dy| {
        (0..width).all(|dx| {
            let tile = anchor.offset(dx, dy);
            map.terrain_passable(tile)
                && !map.tile_in_extractor_frame(tile)
                && state
                    .buildings()
                    .iter()
                    .all(|building| !building.contains(tile))
        })
    })
}

fn rect_contains(anchor: TilePos, size: (i32, i32), tile: TilePos) -> bool {
    tile.x >= anchor.x
        && tile.x < anchor.x + size.0
        && tile.y >= anchor.y
        && tile.y < anchor.y + size.1
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
        let (ew, eh) = BuildingKind::Extractor.base_stats().size;
        for frame in map.extractor_frames() {
            let image = TilePos {
                x: w - ew - frame.x,
                y: h - eh - frame.y,
            };
            assert!(
                map.extractor_frames().contains(&image),
                "{name}: Extractor frame ({}, {}) has no rotated frame at ({}, {})",
                frame.x,
                frame.y,
                image.x,
                image.y
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
