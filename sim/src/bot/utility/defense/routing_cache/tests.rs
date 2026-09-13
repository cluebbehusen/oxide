use super::super::tests::{LEFT_HOME, observation, scenario_with};
use super::super::{
    PlayerId, PublicMapBriefing, UtilityPolicy, path_cost, shortest_path_between,
    shortest_path_between_exhaustive,
};
use super::*;

#[test]
fn retained_routes_match_exhaustive_choices_across_layouts_and_ties() {
    let policy = UtilityPolicy::new();
    for layout in 0..12 {
        let scenario = scenario_with(|tile| {
            if (tile.x * 37 + tile.y * 23 + layout * 11) % 17 < 3 {
                '#'
            } else {
                '.'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let obs = observation(PlayerId(0), LEFT_HOME);
        for hypothetical in [false, true, false] {
            let ground = GroundKnowledge::new(&obs, &map, &[]).retained(&policy, hypothetical);
            for domain in [DefenseDomain::Ground, DefenseDomain::Air] {
                for n in 0..20 {
                    let starts = [TilePos::new(8, n + 1), TilePos::new(9, n + 1)];
                    let goals = [
                        TilePos::new(30, 22 - n),
                        TilePos::new(31, 22 - n),
                        starts[0],
                    ];
                    for candidate in [
                        None,
                        Some(PlacementFootprint {
                            anchor: TilePos::new(18, n + 1),
                            size: (2, 2),
                            blocks_ground: true,
                        }),
                    ] {
                        for goals in [&goals[..2], &goals[..]] {
                            assert_eq!(
                                shortest_path_between(&ground, &starts, goals, candidate, domain),
                                shortest_path_between_exhaustive(
                                    &ground, &starts, goals, candidate, domain
                                )
                            );
                            assert_eq!(
                                shortest_path_between(&ground, goals, &starts, candidate, domain),
                                shortest_path_between_exhaustive(
                                    &ground, goals, &starts, candidate, domain
                                )
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn distance_fields_match_astar_costs_and_corner_rules() {
    let (width, height) = (13, 9);
    for layout in 0..25 {
        let open = |tile: TilePos| (tile.x * 37 + tile.y * 23 + layout * 11) % 17 >= 4;
        for goal in [
            TilePos::new(2, 2),
            TilePos::new(10, 6),
            TilePos::new(0, 0),
            TilePos::new(-1, 0),
        ] {
            let field = distance_field(width, height, goal, open);
            for y in 0..height {
                for x in 0..width {
                    let start = TilePos::new(x, y);
                    if !open(start) {
                        continue;
                    }
                    let expected =
                        chassis::path::astar(width, height, start, goal, open, PATH_EXPANSION_CAP)
                            .map(|mut path| {
                                path.insert(0, start);
                                path_cost(&path)
                            })
                            .unwrap_or(u32::MAX);
                    assert_eq!(field[(y * width + x) as usize], expected);
                }
            }
        }
    }
}

#[test]
fn hypothetical_boards_and_ground_changes_do_not_evict_normal_or_air_routes() {
    let policy = UtilityPolicy::new();
    let scenario = scenario_with(|_| '.');
    let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
    let obs = observation(PlayerId(0), LEFT_HOME);
    let ground = GroundKnowledge::new(&obs, &map, &[]).retained(&policy, false);
    let (start, goal) = (TilePos::new(8, 12), TilePos::new(30, 12));
    let mut scratch = AstarScratch::default();
    let expected = path(
        &ground,
        start,
        goal,
        None,
        DefenseDomain::Ground,
        &mut scratch,
    );
    let air = path(&ground, start, goal, None, DefenseDomain::Air, &mut scratch);
    let baseline = policy.defense_routing_cache.borrow().clone();
    for gap in 0..6 {
        let scenario = scenario_with(|tile| {
            if tile.x == 20 && tile.y != gap {
                '#'
            } else {
                '.'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let overlay = GroundKnowledge::new(&obs, &map, &[]).retained(&policy, true);
        path(
            &overlay,
            start,
            goal,
            None,
            DefenseDomain::Ground,
            &mut scratch,
        );
        assert_eq!(
            path(
                &overlay,
                start,
                goal,
                None,
                DefenseDomain::Air,
                &mut scratch
            ),
            air
        );
    }
    let retained = policy.defense_routing_cache.borrow();
    assert_eq!(retained.ground, baseline.ground);
    assert_eq!(retained.air, baseline.air);
    assert_eq!(retained.hypothetical.len(), 2);
    drop(retained);
    assert_eq!(
        path(
            &ground,
            start,
            goal,
            None,
            DefenseDomain::Ground,
            &mut scratch
        ),
        expected
    );
}

#[test]
fn changed_knowledge_invalidates_paths_even_when_the_old_path_remains_open() {
    let policy = UtilityPolicy::new();
    let obs = observation(PlayerId(0), LEFT_HOME);
    let (start, goal) = (TilePos::new(8, 12), TilePos::new(30, 12));
    let mut scratch = AstarScratch::default();
    let mut paths = Vec::new();
    for obstructed in [true, false, true] {
        let scenario = scenario_with(|tile| {
            if obstructed && tile.x == 20 && tile.y != 3 {
                '#'
            } else {
                '.'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let ground = GroundKnowledge::new(&obs, &map, &[]).retained(&policy, false);
        let actual = path(
            &ground,
            start,
            goal,
            None,
            DefenseDomain::Ground,
            &mut scratch,
        )
        .unwrap();
        let expected = chassis::path::astar(
            obs.map_width,
            obs.map_height,
            start,
            goal,
            |tile| ground.open(tile, None, DefenseDomain::Ground),
            PATH_EXPANSION_CAP,
        )
        .unwrap();
        assert_eq!(actual, expected);
        paths.push(actual);
    }
    assert_ne!(paths[0], paths[1]);
    assert_eq!(paths[0], paths[2]);
}

#[test]
fn air_routes_invalidate_when_peaks_change() {
    let policy = UtilityPolicy::new();
    let obs = observation(PlayerId(0), LEFT_HOME);
    let mut scratch = AstarScratch::default();
    let (start, goal) = (TilePos::new(8, 12), TilePos::new(30, 12));
    for terrain in ['.', '^', '.'] {
        let scenario = scenario_with(|tile| if tile.x == 20 { terrain } else { '.' });
        let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let ground = GroundKnowledge::new(&obs, &map, &[]).retained(&policy, false);
        let route = path(&ground, start, goal, None, DefenseDomain::Air, &mut scratch);
        assert_eq!(route.is_some(), terrain == '.');
        assert_eq!(policy.defense_routing_cache.borrow().air.len(), 1);
    }
}

#[test]
fn capped_failures_never_become_exhausted_component_evidence() {
    use crate::bot::observation::Observation;
    let mut scenario = scenario_with(|_| '.');
    scenario.map = (0..256)
        .map(|y| {
            (0..256)
                .map(|x| {
                    if (x, y) == (4, 10) {
                        '1'
                    } else if (x, y) == (34, 10) {
                        '2'
                    } else if (239..=241).contains(&x)
                        && (239..=241).contains(&y)
                        && (x, y) != (240, 240)
                    {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect()
        })
        .collect();
    let state = scenario.build().unwrap();
    let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
    let obs = Observation::fog_honest(&state, PlayerId(0));
    let policy = UtilityPolicy::new();
    let ground = GroundKnowledge::new(&obs, &map, &[]).retained(&policy, false);
    let mut scratch = AstarScratch::default();
    let start = TilePos::new(8, 12);
    let unreachable = TilePos::new(240, 240);
    for _ in 0..2 {
        assert!(
            path(
                &ground,
                start,
                unreachable,
                None,
                DefenseDomain::Ground,
                &mut scratch
            )
            .is_none()
        );
        assert!(!scratch.last_search_exhausted());
        assert!(!scratch.last_search_reached(start));
    }
    assert_eq!(
        bound(&ground, start, unreachable, DefenseDomain::Ground),
        u32::MAX
    );
    assert!(
        path(
            &ground,
            start,
            TilePos::new(12, 12),
            None,
            DefenseDomain::Ground,
            &mut scratch
        )
        .is_some()
    );
    assert!(!scratch.last_search_exhausted());
}

#[test]
fn cache_hits_clear_exhaustion_and_eviction_preserves_answers() {
    let policy = UtilityPolicy::new();
    let scenario = scenario_with(|tile| if tile.x == 20 { '#' } else { '.' });
    let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
    let obs = observation(PlayerId(0), LEFT_HOME);
    let ground = GroundKnowledge::new(&obs, &map, &[]).retained(&policy, false);
    let (start, goal, unreachable) = (
        TilePos::new(8, 12),
        TilePos::new(12, 12),
        TilePos::new(30, 12),
    );
    let mut scratch = AstarScratch::default();
    let expected = path(
        &ground,
        start,
        goal,
        None,
        DefenseDomain::Ground,
        &mut scratch,
    )
    .unwrap();
    assert!(
        path(
            &ground,
            start,
            unreachable,
            None,
            DefenseDomain::Ground,
            &mut scratch
        )
        .is_none()
    );
    assert!(scratch.last_search_exhausted());
    assert_eq!(
        path(
            &ground,
            start,
            goal,
            None,
            DefenseDomain::Ground,
            &mut scratch
        ),
        Some(expected.clone())
    );
    assert!(!scratch.last_search_exhausted());
    let candidate = PlacementFootprint {
        anchor: goal,
        size: (1, 1),
        blocks_ground: true,
    };
    assert!(
        path(
            &ground,
            start,
            goal,
            Some(candidate),
            DefenseDomain::Ground,
            &mut scratch
        )
        .is_none()
    );
    assert!(!scratch.last_search_exhausted());
    // A single field fits in its partition; replacing it retains the path.
    let cells = ground.ground_blocked.len();
    policy.defense_routing_cache.borrow_mut().ground[0].budget =
        cells + 2 * (cells * 4 + ENTRY_ALLOWANCE);
    assert_eq!(bound(&ground, start, goal, DefenseDomain::Ground), 40);
    assert_eq!(
        bound(&ground, start, unreachable, DefenseDomain::Ground),
        u32::MAX
    );
    assert_eq!(
        path(
            &ground,
            start,
            goal,
            None,
            DefenseDomain::Ground,
            &mut scratch
        ),
        Some(expected.clone())
    );
    let cache = policy.defense_routing_cache.borrow();
    assert!(
        cells + cache.ground[0].path_bytes + cache.ground[0].distance_bytes
            <= cache.ground[0].budget
    );
    assert!(cache.ground[0].paths.contains_key(&(None, start, goal)));
    drop(cache);
    {
        let mut cache = policy.defense_routing_cache.borrow_mut();
        let generation = &mut cache.ground[0];
        generation.paths.clear();
        generation.path_order.clear();
        generation.distances.clear();
        generation.distance_order.clear();
        generation.path_bytes = 0;
        generation.distance_bytes = 0;
        generation.budget = cells;
    }
    assert_eq!(
        bound(&ground, start, goal, DefenseDomain::Ground),
        octile_cost(start, goal)
    );
    let other = TilePos::new(13, 12);
    assert!(
        path(
            &ground,
            start,
            other,
            None,
            DefenseDomain::Ground,
            &mut scratch
        )
        .is_some()
    );
    let blocked = TilePos::new(20, 12);
    assert_eq!(
        bound(&ground, blocked, goal, DefenseDomain::Ground),
        octile_cost(blocked, goal)
    );
    assert_eq!(
        bound(&ground, TilePos::new(-1, 0), goal, DefenseDomain::Ground),
        octile_cost(TilePos::new(-1, 0), goal)
    );
}

#[test]
fn distance_field_churn_preserves_routes_and_bounds_both_payload_classes() {
    let policy = UtilityPolicy::new();
    let scenario = scenario_with(|_| '.');
    let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
    let obs = observation(PlayerId(0), LEFT_HOME);
    let ground = GroundKnowledge::new(&obs, &map, &[]).retained(&policy, false);
    let mut scratch = AstarScratch::default();
    let start = TilePos::new(8, 12);
    let goal = TilePos::new(30, 12);
    let expected = path(&ground, start, goal, None, DefenseDomain::Air, &mut scratch).unwrap();
    let cells = ground.air_blocked.len();
    let field_bytes = cells * size_of::<u32>() + ENTRY_ALLOWANCE;
    policy.defense_routing_cache.borrow_mut().air[0].budget = cells + 2 * field_bytes;
    for x in 10..35 {
        bound(&ground, start, TilePos::new(x, 15), DefenseDomain::Air);
        let cache = policy.defense_routing_cache.borrow();
        let generation = &cache.air[0];
        assert_eq!(
            generation.paths.get(&(None, start, goal)).unwrap().as_ref(),
            expected
        );
        assert_eq!(generation.distances.len(), 1);
        assert!(generation.path_bytes <= generation.payload_budget());
        assert!(generation.distance_bytes <= generation.payload_budget());
    }
    for x in 9..35 {
        path(
            &ground,
            start,
            TilePos::new(x, 16),
            None,
            DefenseDomain::Air,
            &mut scratch,
        );
    }
    let cache = policy.defense_routing_cache.borrow();
    let generation = &cache.air[0];
    assert!(generation.distances.contains_key(&TilePos::new(34, 15)));
    assert!(generation.path_bytes <= generation.payload_budget());
    assert!(generation.paths.len() < 27);
}
