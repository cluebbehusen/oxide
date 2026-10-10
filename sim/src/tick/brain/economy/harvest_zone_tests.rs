use super::*;
use crate::scenario::{BuildingSpec, PlayerSpec, ScenarioMode, UnitSpec};
use crate::stats::BuildingKind;
use crate::{PlayerId, Scenario, UnitKind};

#[test]
fn return_cargo_reuses_exhausted_floods_per_worker_and_safety_pass() {
    let mut rows = vec![vec!['.'; 32]; 20];
    for (y, row) in rows.iter_mut().enumerate() {
        for (x, tile) in row.iter_mut().enumerate() {
            if y == 0 || y == 19 || x == 0 || x == 31 || x == 15 {
                *tile = '#';
            }
        }
    }
    rows[2][18] = '1';
    rows[16][28] = '2';
    let scenario = serde_json::json!({
        "name": "sealed-worker-drop-offs", "map": rows.into_iter().map(|row| row.into_iter().collect::<String>()).collect::<Vec<_>>(),
        "players": [
            {"name": "F", "scrap": 0, "bot": false},
            {"name": "C", "scrap": 0, "bot": true}
        ],
        "units": [
            {"player": 0, "kind": "harvester", "x": 4, "y": 5},
            {"player": 0, "kind": "excavator", "x": 6, "y": 5},
            {"player": 0, "kind": "harvester", "x": 18, "y": 8}
        ],
        "buildings": [
            {"player": 0, "kind": "foundry", "x": 23, "y": 2},
            {"player": 0, "kind": "foundry", "x": 18, "y": 11},
            {"player": 0, "kind": "foundry", "x": 23, "y": 11}
        ]
    });
    let state = Scenario::from_json(&scenario.to_string())
        .unwrap()
        .build()
        .unwrap();
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    // The first worker's failed safety pass proves the sealed side for
    // the phase, so the second worker searches only while ignoring
    // danger.
    for (worker, searches) in state.units[..2].iter().zip([2, 1]) {
        let before = danger.route_search_count();
        assert_eq!(
            return_cargo_destination(&state, &danger, worker.id, None),
            None
        );
        assert_eq!(danger.route_search_count() - before, searches);
    }
    assert!(return_cargo_destination(&state, &danger, state.units[2].id, None).is_some());
}

#[test]
fn return_cargo_resets_reachability_before_ignoring_danger() {
    let scenario = serde_json::json!({
        "name": "danger-blocked-return", "map": [
            "##########################",
            "#1....................2..#",
            "#........................#",
            "#........................#",
            "#........................#",
            "#........................#",
            "##########################"
        ],
        "players": [
            {"name": "F", "scrap": 0, "bot": false},
            {"name": "C", "scrap": 0, "bot": true}
        ],
        "units": [
            {"player": 0, "kind": "harvester", "x": 16, "y": 3},
            {"player": 1, "kind": "scuttler", "x": 12, "y": 3}
        ]
    });
    let state = Scenario::from_json(&scenario.to_string())
        .unwrap()
        .build()
        .unwrap();
    let worker = state.units[0].id;
    let foundry = state
        .buildings
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap();
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    assert!(
        known_rect_route(
            &state,
            &danger,
            worker,
            foundry.anchor,
            foundry.kind.size(),
            true,
            None
        )
        .is_none()
    );
    // The failed safe search above already settles the safety pass; the
    // pass that ignores danger must still search afresh and succeed.
    let before = danger.route_search_count();
    assert_eq!(
        return_cargo_destination(&state, &danger, worker, None),
        Some(foundry.id)
    );
    assert_eq!(danger.route_search_count() - before, 1);
}

/// A failure inside a small pocket keeps no proof: walking the whole
/// map to record it would cost more than repeating the pocket's search.
#[test]
fn a_failed_search_from_a_small_pocket_keeps_no_proof() {
    let scenario = serde_json::json!({
        "name": "pocketed-return", "map": [
            "##########################",
            "#1....................2..#",
            "#..............###.......#",
            "#..............#.#.......#",
            "#..............###.......#",
            "#........................#",
            "##########################"
        ],
        "players": [
            {"name": "F", "scrap": 0, "bot": false},
            {"name": "C", "scrap": 0, "bot": true}
        ],
        "units": [{"player": 0, "kind": "harvester", "x": 16, "y": 3}]
    });
    let state = Scenario::from_json(&scenario.to_string())
        .unwrap()
        .build()
        .unwrap();
    let worker = state.units[0].id;
    let foundry = state
        .buildings
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap();
    let (anchor, size) = (foundry.anchor, foundry.kind.size());
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    let route = || known_rect_route(&state, &danger, worker, anchor, size, true, None);
    assert!(route().is_none());
    let before = danger.route_search_count();
    assert!(route().is_none());
    assert!(
        danger.route_search_count() > before,
        "the pocket searches again"
    );
}

/// A failed safe search settles later searches from its component until
/// a node beside that component drains open.
#[test]
fn a_failed_safe_search_settles_repeats_until_a_watched_node_drains() {
    let scenario = serde_json::json!({
        "name": "scrap-gated-return", "map": [
            "##########################",
            "#1............#.......2..#",
            "#.............#..........#",
            "#.............s..........#",
            "#.............#..........#",
            "#.............#..........#",
            "##########################"
        ],
        "players": [
            {"name": "F", "scrap": 0, "bot": false},
            {"name": "C", "scrap": 0, "bot": true}
        ],
        "units": [{"player": 0, "kind": "harvester", "x": 17, "y": 3}]
    });
    let mut state = Scenario::from_json(&scenario.to_string())
        .unwrap()
        .build()
        .unwrap();
    let worker = state.units[0].id;
    let gate = TilePos::new(14, 3);
    let foundry = state
        .buildings
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap();
    let (anchor, size) = (foundry.anchor, foundry.kind.size());
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    assert!(state.vision(PlayerId(0)).visible(gate));
    assert!(danger.outside_every_envelope(state.units[0].tile()));
    let route = |state: &State| known_rect_route(state, &danger, worker, anchor, size, true, None);

    assert!(route(&state).is_none());
    let before = danger.route_search_count();
    assert!(route(&state).is_none());
    assert_eq!(
        danger.route_search_count(),
        before,
        "the failure settles the repeat"
    );

    while state.map.extract_scrap(gate).is_some() {}
    assert!(route(&state).is_some(), "the drained gate lapses the proof");
}

/// Two workers of one seat bound for the same drop-off spot: the nearer
/// keeps its route, the farther yields. Yielding to every arrival would
/// make each give the spot up to the other.
#[test]
fn a_held_drop_off_route_yields_only_to_a_nearer_worker() {
    let scenario = serde_json::json!({
        "name": "shared-drop-off", "map": [
            "##########################",
            "#1....................2..#",
            "#........................#",
            "#........................#",
            "#........................#",
            "#........................#",
            "##########################"
        ],
        "players": [
            {"name": "F", "scrap": 0, "bot": false},
            {"name": "C", "scrap": 0, "bot": true}
        ],
        "units": [
            {"player": 0, "kind": "harvester", "x": 9, "y": 3},
            {"player": 0, "kind": "harvester", "x": 10, "y": 3}
        ]
    });
    let mut state = Scenario::from_json(&scenario.to_string())
        .unwrap()
        .build()
        .unwrap();
    let foundry = state
        .buildings
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap();
    let (foundry_id, anchor, size) = (foundry.id, foundry.anchor, foundry.kind.size());
    let goal = TilePos::new(anchor.x + size.0, anchor.y + 1);
    let point = goal.center() - Vec2Fx::new(Fx::lit("0.2"), Fx::ZERO);
    let ids = [state.units[0].id, state.units[1].id];
    for (slot, offset) in [(0, Fx::lit("0.3")), (1, Fx::lit("0.8"))] {
        let unit = &mut state.units[slot];
        unit.worker_mut().carrying = 1;
        unit.pos = point + Vec2Fx::new(offset, Fx::ZERO);
        unit.path = Some(PathFollow {
            goal,
            final_point: Some(point),
            waypoints: vec![goal],
            next: 0,
        });
    }
    let keeps = |state: &mut State, id: UnitId| {
        let danger = GroundSalvageDanger::capture(state, PlayerId(0));
        let mut events = Vec::new();
        assert!(try_drop_offs(
            state,
            &danger,
            id,
            &[foundry_id],
            &mut events
        ));
        state
            .unit(id)
            .unwrap()
            .path
            .as_ref()
            .and_then(|path| path.final_point)
            == Some(point)
    };
    assert!(
        keeps(&mut state, ids[0]),
        "the nearer worker keeps the spot"
    );
    assert!(!keeps(&mut state, ids[1]), "the farther worker yields it");
}

fn held_worker(wall: Option<(usize, usize)>) -> (State, UnitId, KnownSource, PathFollow) {
    let mut map = vec![
        "##########################".to_owned(),
        "#1....................2..#".to_owned(),
        "#........................#".to_owned(),
        "#.....s..................#".to_owned(),
        "#........................#".to_owned(),
        "#........................#".to_owned(),
        "##########################".to_owned(),
    ];
    if let Some((x, y)) = wall {
        map[y].replace_range(x..=x, "#");
    }
    let scenario = serde_json::json!({
        "name": "danger-held-order", "map": map,
        "players": [
            {"name": "F", "scrap": 0, "bot": false},
            {"name": "C", "scrap": 0, "bot": true}
        ],
        "units": [
            {"player": 0, "kind": "harvester", "x": 16, "y": 3},
            {"player": 1, "kind": "scuttler", "x": 12, "y": 3}
        ]
    });
    let mut state = Scenario::from_json(&scenario.to_string())
        .unwrap()
        .build()
        .unwrap();
    let node = TilePos::new(6, 3);
    let source = known_source(&state, PlayerId(0), node).expect("the node is in sight");
    state.units[0].order = Order::Harvest {
        node,
        anchor: node,
        retiring: false,
    };
    let ordered = PathFollow {
        final_point: None,
        goal: TilePos::new(7, 3),
        waypoints: (7..16).rev().map(|x| TilePos::new(x, 3)).collect(),
        next: 0,
    };
    let worker = state.units[0].id;
    (state, worker, source, ordered)
}

/// Ticks, from the scene's start, on which the held worker searched.
fn searches(
    state: &mut State,
    worker: UnitId,
    source: KnownSource,
    ordered: &PathFollow,
    ticks: u64,
) -> Vec<u64> {
    let start = state.tick;
    let mut searched = Vec::new();
    for tick in start..start + ticks {
        state.tick = tick;
        state.units[0].path = Some(ordered.clone());
        let danger = GroundSalvageDanger::capture(state, PlayerId(0));
        let before = danger.route_search_count();
        assert!(approach_authoritative_source(
            state, &danger, worker, source
        ));
        if danger.route_search_count() > before {
            searched.push(tick - start);
        }
    }
    searched
}

#[test]
fn a_worker_held_by_danger_searches_at_once_then_backs_off_after_a_failure() {
    let (mut state, worker, source, ordered) = held_worker(None);
    let period = crate::stats::HARVEST_DANGER_RETRY_TICKS;
    let searched = searches(&mut state, worker, source, &ordered, 2 * period);
    assert_eq!(searched, [0, period], "no safe detour exists in this lane");
    assert_eq!(
        state.units[0].path.as_ref(),
        Some(&ordered),
        "the ordered route stays in force"
    );
    assert_eq!(state.units[0].danger_retry_at(), Some(state.tick + 1));
}

#[test]
fn a_danger_retry_round_trips_up_to_its_bound() {
    let (mut state, ..) = held_worker(None);
    let bound = state.tick + crate::stats::HARVEST_DANGER_RETRY_TICKS;
    state.units[0].worker_mut().danger_retry_at = Some(bound);
    let restored: State = serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    assert_eq!(restored.units[0].danger_retry_at(), Some(bound));
    state.units[0].worker_mut().danger_retry_at = Some(bound + 1);
    assert!(serde_json::from_str::<State>(&serde_json::to_string(&state).unwrap()).is_err());
}

#[test]
fn a_waypoint_turned_impassable_replans_every_tick() {
    let (mut state, worker, source, ordered) = held_worker(Some((14, 3)));
    assert_eq!(
        searches(&mut state, worker, source, &ordered, 4),
        [0, 1, 2, 3]
    );
    assert_eq!(state.units[0].danger_retry_at(), None);
}

#[test]
fn a_sealed_worker_settles_every_work_position_with_one_search() {
    let mut rows = vec![vec!['.'; 32]; 20];
    for (y, row) in rows.iter_mut().enumerate() {
        for (x, tile) in row.iter_mut().enumerate() {
            if y == 0 || y == 19 || x == 0 || x == 31 || x == 15 {
                *tile = '#';
            }
        }
    }
    rows[2][18] = '1';
    rows[16][28] = '2';
    rows[7][20] = 's';
    let scenario = serde_json::json!({
        "name": "sealed-worker-source", "map": rows.into_iter().map(|row| row.into_iter().collect::<String>()).collect::<Vec<_>>(),
        "players": [
            {"name": "F", "scrap": 0, "bot": false},
            {"name": "C", "scrap": 0, "bot": true}
        ],
        "units": [{"player": 0, "kind": "harvester", "x": 4, "y": 5}]
    });
    let state = Scenario::from_json(&scenario.to_string())
        .unwrap()
        .build()
        .unwrap();
    let source =
        known_source(&state, PlayerId(0), TilePos::new(20, 7)).expect("the node is in sight");
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    let before = danger.route_search_count();
    assert!(safe_source_route(&state, &danger, state.units[0].id, source).is_none());
    assert_eq!(danger.route_search_count() - before, 1);
}

#[test]
fn a_worker_held_from_its_drop_off_rescans_after_the_retry_period_and_on_command() {
    let scenario = serde_json::json!({
        "name": "danger-held-delivery", "map": [
            "##########################",
            "#1....................2..#",
            "#........................#",
            "#........................#",
            "#........................#",
            "#........................#",
            "##########################"
        ],
        "players": [
            {"name": "F", "scrap": 0, "bot": false},
            {"name": "C", "scrap": 0, "bot": true}
        ],
        "units": [
            {"player": 0, "kind": "harvester", "x": 16, "y": 3},
            {"player": 1, "kind": "scuttler", "x": 12, "y": 3}
        ]
    });
    let mut state = Scenario::from_json(&scenario.to_string())
        .unwrap()
        .build()
        .unwrap();
    let worker = state.units[0].id;
    state.units[0].worker_mut().carrying = 1;
    let foundry = state
        .buildings
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap()
        .id;
    let period = crate::stats::HARVEST_DANGER_RETRY_TICKS;
    let start = crate::stats::DANGER_HOLD_REPORT_PERIOD - 1;
    let mut searched = Vec::new();
    let mut reported = Vec::new();
    for tick in start..=start + period {
        state.tick = tick;
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        let before = danger.route_search_count();
        let mut events = Vec::new();
        assert!(try_drop_offs(
            &mut state,
            &danger,
            worker,
            &[foundry],
            &mut events
        ));
        assert!(state.units[0].path.is_none(), "a held worker stands");
        if danger.route_search_count() > before {
            searched.push(tick - start);
        }
        if events.iter().any(|event| {
            matches!(
                event,
                Event::OrderStalled {
                    reason: StallReason::DangerHold,
                    ..
                }
            )
        }) {
            reported.push(tick);
        }
    }
    assert_eq!(searched, [0, period]);
    assert_eq!(
        reported,
        [crate::stats::DANGER_HOLD_REPORT_PERIOD],
        "the hold stays visible"
    );
    assert!(state.units[0].danger_retry_at().is_some());
    crate::tick::commands::apply(
        &mut state,
        &[crate::PlayerCommand {
            player: PlayerId(0),
            command: crate::Command::Stop {
                units: vec![worker],
            },
        }],
        &mut Vec::new(),
    );
    assert_eq!(
        state.units[0].danger_retry_at(),
        None,
        "a command is judged at once"
    );
}

#[test]
fn arriving_at_the_source_ends_a_route_hold() {
    let (mut state, worker, _, _) = held_worker(None);
    let node = TilePos::new(6, 3);
    let radius = state.units[0].kind.stats().radius;
    state.units[0].pos =
        crate::geometry::work_approach_point(TilePos::new(7, 3), node, (1, 1), radius);
    state.units[0].worker_mut().danger_retry_at = Some(state.tick + 8);
    assert!(state.units[0].in_work_reach(node, (1, 1)), "premise");
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    harvest(
        &mut state,
        &danger,
        worker,
        node,
        node,
        false,
        &mut Vec::new(),
    );
    assert_eq!(state.units[0].danger_retry_at(), None);
}

#[test]
fn mirrored_workers_replace_depleted_sources_in_their_local_frame() {
    let mirror = |pos: TilePos| TilePos::new(39 - pos.x, 23 - pos.y);
    let anchor = TilePos::new(7, 3);
    for (from, sources) in [
        (TilePos::new(6, 4), [TilePos::new(7, 2), TilePos::new(8, 3)]),
        (TilePos::new(6, 3), [TilePos::new(7, 2), TilePos::new(7, 4)]),
        (anchor, [TilePos::new(7, 2), TilePos::new(8, 3)]),
    ] {
        for reverse_ids in [false, true] {
            for heading in [0u8, 64, 128, 192] {
                let mut rows = vec![vec!['.'; 40]; 24];
                rows[4][4] = '1';
                rows[18][34] = '2';
                for pos in sources.into_iter().flat_map(|pos| [pos, mirror(pos)]) {
                    rows[chassis::grid::as_index(pos.y)][chassis::grid::as_index(pos.x)] = 's';
                }
                let mut scenario = Scenario::skirmish();
                scenario.map = rows
                    .into_iter()
                    .map(|row| row.into_iter().collect())
                    .collect();
                scenario.units = [from, mirror(from)]
                    .into_iter()
                    .zip(0..)
                    .map(|(pos, player)| UnitSpec {
                        player,
                        kind: UnitKind::Harvester,
                        x: pos.x,
                        y: pos.y,
                    })
                    .collect();
                if reverse_ids {
                    scenario.units.reverse();
                }
                let mut state = scenario.build().unwrap();
                for unit in &mut state.units {
                    unit.heading = if unit.player == PlayerId(0) {
                        heading
                    } else {
                        heading.wrapping_add(128)
                    };
                }
                let selected: Vec<_> = [PlayerId(0), PlayerId(1)]
                    .into_iter()
                    .map(|player| {
                        let worker = state
                            .units
                            .iter()
                            .find(|unit| unit.player == player)
                            .unwrap();
                        let anchor = if player == PlayerId(0) {
                            anchor
                        } else {
                            mirror(anchor)
                        };
                        let danger = GroundSalvageDanger::capture(&state, player);
                        replacement_source(&state, &danger, worker.id, anchor, Some(anchor))
                            .unwrap()
                            .pos
                    })
                    .collect();
                assert_eq!(
                    selected[1],
                    mirror(selected[0]),
                    "from={from:?}, heading={heading}, reverse_ids={reverse_ids}"
                );
            }
        }
    }
}

#[test]
fn the_zone_radius_covers_the_widest_connected_shipped_deposit() {
    // Compass Grand and Trident Plateau carry center deposits whose
    // endpoint span is seven tiles. The work zone is anchored rather
    // than re-centered after each hop, so this exact reach cannot
    // walk onward into a second field.
    assert_eq!(HARVEST_ZONE_RADIUS, 7);
}

#[test]
fn replacement_preserves_worker_affinity_before_route_efficiency() {
    let state = Scenario {
        mode: ScenarioMode::Match,
        name: "harvest-worker-affinity".into(),
        map: vec![
            "####################".into(),
            "#1.....#...........#".into(),
            "#......#...........#".into(),
            "#......#...........#".into(),
            "#......#...........#".into(),
            "#......#s..........#".into(),
            "#......#...........#".into(),
            "#......#...........#".into(),
            "#......#...........#".into(),
            "#....s...........2.#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "West".into(),
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "East".into(),
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 5,
            y: 5,
        }],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .unwrap();
    let worker = state.units[0].id;
    let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
    let local = known_source(&state, PlayerId(0), TilePos::new(8, 5)).unwrap();
    let cheap_route = known_source(&state, PlayerId(0), TilePos::new(5, 9)).unwrap();
    assert!(
        source_route_len(&state, &danger, worker, local).unwrap()
            > source_route_len(&state, &danger, worker, cheap_route).unwrap(),
        "the fixture must make route-first scoring prefer the farther source"
    );
    assert_eq!(
        replacement_source(&state, &danger, worker, TilePos::new(5, 5), None)
            .map(|source| source.pos),
        Some(local.pos),
        "safe reachable sources stay with the worker already closest to them"
    );
}

#[test]
fn per_tick_danger_rechecks_are_constant_even_on_a_long_route() {
    use std::cell::Cell;

    let path = PathFollow {
        final_point: None,
        goal: TilePos::new(127, 1),
        waypoints: (1..=127).map(|x| TilePos::new(x, 1)).collect(),
        next: 3,
    };
    let checks = Cell::new(0);
    assert!(
        first_flagged_waypoint(&path, |_| {
            checks.set(checks.get() + 1);
            true
        })
        .is_none()
    );
    assert_eq!(
        checks.get(),
        crate::stats::HARVEST_DANGER_LOOKAHEAD,
        "the hot-path cost is independent of total route length"
    );
}

#[test]
fn danger_replan_cadence_uses_owner_local_rank() {
    let mut state = Scenario {
        mode: ScenarioMode::Match,
        name: "owner-local-replan-cadence".into(),
        map: vec![
            "##############".into(),
            "#1.........2.#".into(),
            "#............#".into(),
            "#............#".into(),
            "#............#".into(),
            "##############".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "West".into(),
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "East".into(),
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 4,
                y: 2,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 7,
                y: 2,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 4,
                y: 3,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Harvester,
                x: 7,
                y: 3,
            },
        ],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .expect("the cadence scenario builds");
    let ids: Vec<_> = state
        .units
        .iter()
        .map(|unit| (unit.id, unit.player))
        .collect();
    assert_eq!(
        ids,
        vec![
            (UnitId(0), PlayerId(0)),
            (UnitId(1), PlayerId(1)),
            (UnitId(2), PlayerId(0)),
            (UnitId(3), PlayerId(1)),
        ],
        "the fixture interleaves global ids across equivalent owner ranks"
    );

    state.tick = 0;
    assert!(danger_replan_window(&state, UnitId(0)));
    assert!(danger_replan_window(&state, UnitId(1)));
    assert!(!danger_replan_window(&state, UnitId(2)));
    assert!(!danger_replan_window(&state, UnitId(3)));

    state.tick = crate::stats::HARVEST_REPLAN_PERIOD - 1;
    assert!(!danger_replan_window(&state, UnitId(0)));
    assert!(!danger_replan_window(&state, UnitId(1)));
    assert!(danger_replan_window(&state, UnitId(2)));
    assert!(danger_replan_window(&state, UnitId(3)));
}

#[test]
fn unseen_wreck_selection_reads_frozen_memory_not_live_salvage() {
    let mut state = Scenario {
        mode: ScenarioMode::Match,
        name: "harvest-memory".into(),
        map: vec![
            "##############################".into(),
            "#1...........................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#..........................2.#".into(),
            "#............................#".into(),
            "##############################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "West".into(),
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "East".into(),
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 14,
            y: 3,
        }],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .unwrap();
    let wreck = TilePos::new(15, 3);
    state.map.add_wreck(wreck, 9);
    state.refresh_vision();
    assert_eq!(
        known_source(&state, PlayerId(0), wreck),
        Some(KnownSource {
            pos: wreck,
            amount: 9,
            kind: SourceKind::Wreck,
        })
    );

    state.units[0].pos = TilePos::new(3, 3).center();
    state.refresh_vision();
    assert!(!state.vision(PlayerId(0)).visible(wreck));
    state.map.clear_wreck(wreck);
    assert_eq!(
        known_source(&state, PlayerId(0), wreck),
        Some(KnownSource {
            pos: wreck,
            amount: 9,
            kind: SourceKind::Wreck,
        }),
        "unseen source choice is a belief, not a hidden live-map read"
    );
}

#[test]
fn an_unscouted_enemy_building_cannot_bend_a_route_through_fog() {
    let scenario = Scenario {
        mode: ScenarioMode::Match,
        name: "harvest-route-belief".into(),
        map: vec![
            "##############################".into(),
            "#1...........................#".into(),
            "#............................#".into(),
            "#............................#".into(),
            "#..........................2.#".into(),
            "#............................#".into(),
            "##############################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "West".into(),
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "East".into(),
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: vec![UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 3,
            y: 3,
        }],
        buildings: Vec::new(),
        meta: None,
    };
    let clear = scenario.build().unwrap();
    let mut obscured_scenario = scenario;
    let hidden_anchor = TilePos::new(12, 2);
    obscured_scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Reclaimer,
        x: hidden_anchor.x,
        y: hidden_anchor.y,
    });
    let obscured = obscured_scenario.build().unwrap();
    assert!(!obscured.vision(PlayerId(0)).visible(hidden_anchor));
    assert!(obscured.vision(PlayerId(0)).ghosts().is_empty());

    let source = KnownSource {
        pos: TilePos::new(22, 3),
        amount: 1,
        kind: SourceKind::Wreck,
    };
    let clear_danger = GroundSalvageDanger::capture(&clear, PlayerId(0));
    let obscured_danger = GroundSalvageDanger::capture(&obscured, PlayerId(0));
    let clear_route = safe_source_route(&clear, &clear_danger, clear.units[0].id, source);
    let obscured_route =
        safe_source_route(&obscured, &obscured_danger, obscured.units[0].id, source);
    assert_eq!(
        obscured_route, clear_route,
        "two views that differ only behind fog must choose the same route"
    );
}
