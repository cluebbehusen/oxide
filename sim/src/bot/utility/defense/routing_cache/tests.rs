use super::super::tests::{LEFT_HOME, observation, scenario_with};
use super::super::{DefenseDomain, GroundKnowledge, PlacementFootprint};
use super::super::{
    PlayerId, PublicMapBriefing, UtilityPolicy, shortest_path_between,
    shortest_path_between_exhaustive,
};
use chassis::grid::TilePos;

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

use crate::bot::navigation::search::Search;
use crate::stats::PATH_EXPANSION_CAP;
fn path(
    ground: &GroundKnowledge<'_>,
    start: TilePos,
    goal: TilePos,
    candidate: Option<PlacementFootprint>,
    domain: DefenseDomain,
    search: &mut Search,
) -> Option<Vec<TilePos>> {
    super::board(ground, domain).path(
        start,
        goal,
        candidate.and_then(|candidate| super::overlay(candidate, domain)),
        search,
    )
}
fn bound(
    ground: &GroundKnowledge<'_>,
    start: TilePos,
    goal: TilePos,
    domain: DefenseDomain,
) -> u32 {
    super::board(ground, domain).bound(start, goal)
}

#[test]
fn changed_knowledge_invalidates_paths_even_when_the_old_path_remains_open() {
    let policy = UtilityPolicy::new();
    let obs = observation(PlayerId(0), LEFT_HOME);
    let (start, goal) = (TilePos::new(8, 12), TilePos::new(30, 12));
    let mut scratch = Search::default();
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
    let mut scratch = Search::default();
    let (start, goal) = (TilePos::new(8, 12), TilePos::new(30, 12));
    for terrain in ['.', '^', '.'] {
        let scenario = scenario_with(|tile| if tile.x == 20 { terrain } else { '.' });
        let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let ground = GroundKnowledge::new(&obs, &map, &[]).retained(&policy, false);
        let route = path(&ground, start, goal, None, DefenseDomain::Air, &mut scratch);
        assert_eq!(route.is_some(), terrain == '.');
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
    let mut scratch = Search::default();
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
