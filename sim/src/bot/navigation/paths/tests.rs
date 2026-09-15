use super::*;
use crate::bot::navigation::octile as octile_cost;
use crate::stats::PATH_EXPANSION_CAP;

struct TestGrid {
    width: i32,
    height: i32,
    ground: Vec<bool>,
    air: Vec<bool>,
}
impl TestGrid {
    fn new(width: i32, height: i32, mut terrain: impl FnMut(TilePos) -> char) -> Self {
        let mut ground = Vec::new();
        let mut air = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let cell = terrain(TilePos::new(x, y));
                ground.push(cell == '#' || cell == '^');
                air.push(cell == '^');
            }
        }
        Self {
            width,
            height,
            ground,
            air,
        }
    }
    fn borrowed<'a>(
        &'a self,
        cache: &'a RefCell<PathQueries>,
        hypothetical: bool,
    ) -> TestBoard<'a> {
        TestBoard {
            grid: self,
            cache,
            hypothetical,
        }
    }
}
struct TestBoard<'a> {
    grid: &'a TestGrid,
    cache: &'a RefCell<PathQueries>,
    hypothetical: bool,
}
impl TestBoard<'_> {
    fn board(&self, domain: CacheClass) -> PathBoard<'_> {
        let (blocked, class) = match domain {
            CacheClass::Air => (&self.grid.air, CacheClass::Air),
            _ => (
                &self.grid.ground,
                if self.hypothetical {
                    CacheClass::Hypothetical
                } else {
                    CacheClass::Ground
                },
            ),
        };
        PathBoard {
            grid: KnownGrid::new(self.grid.width, self.grid.height, blocked).unwrap(),
            class,
            cache: self.cache,
        }
    }
    fn open(&self, tile: TilePos, overlay: Option<BlockedRect>, domain: CacheClass) -> bool {
        self.board(domain).grid.open(tile, overlay)
    }
}
fn path(
    board: &TestBoard<'_>,
    start: TilePos,
    goal: TilePos,
    overlay: Option<BlockedRect>,
    domain: CacheClass,
    search: &mut Search,
) -> Option<Vec<TilePos>> {
    board.board(domain).path(start, goal, overlay, search)
}
fn bound(board: &TestBoard<'_>, start: TilePos, goal: TilePos, domain: CacheClass) -> u32 {
    board.board(domain).bound(start, goal)
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
    let cache = RefCell::new(PathQueries::default());
    let scenario = TestGrid::new(40, 24, |_| '.');
    let map = scenario;
    let ground = map.borrowed(&cache, false);
    let (start, goal) = (TilePos::new(8, 12), TilePos::new(30, 12));
    let mut scratch = Search::default();
    let expected = path(&ground, start, goal, None, CacheClass::Ground, &mut scratch);
    let air = path(&ground, start, goal, None, CacheClass::Air, &mut scratch);
    let baseline = cache.borrow().clone();
    for gap in 0..6 {
        let scenario = TestGrid::new(40, 24, |tile| {
            if tile.x == 20 && tile.y != gap {
                '#'
            } else {
                '.'
            }
        });
        let map = scenario;
        let overlay = map.borrowed(&cache, true);
        path(
            &overlay,
            start,
            goal,
            None,
            CacheClass::Ground,
            &mut scratch,
        );
        assert_eq!(
            path(&overlay, start, goal, None, CacheClass::Air, &mut scratch),
            air
        );
    }
    let retained = cache.borrow();
    assert_eq!(retained.ground, baseline.ground);
    assert_eq!(retained.air, baseline.air);
    assert_eq!(retained.hypothetical.len(), 2);
    drop(retained);
    assert_eq!(
        path(&ground, start, goal, None, CacheClass::Ground, &mut scratch),
        expected
    );
}

#[test]
fn changed_knowledge_invalidates_paths_even_when_the_old_path_remains_open() {
    let cache = RefCell::new(PathQueries::default());
    let (start, goal) = (TilePos::new(8, 12), TilePos::new(30, 12));
    let mut scratch = Search::default();
    let mut paths = Vec::new();
    for obstructed in [true, false, true] {
        let scenario = TestGrid::new(40, 24, |tile| {
            if obstructed && tile.x == 20 && tile.y != 3 {
                '#'
            } else {
                '.'
            }
        });
        let map = scenario;
        let ground = map.borrowed(&cache, false);
        let actual = path(&ground, start, goal, None, CacheClass::Ground, &mut scratch).unwrap();
        let expected = chassis::path::astar(
            ground.grid.width,
            ground.grid.height,
            start,
            goal,
            |tile| ground.open(tile, None, CacheClass::Ground),
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
    let cache = RefCell::new(PathQueries::default());
    let mut scratch = Search::default();
    let (start, goal) = (TilePos::new(8, 12), TilePos::new(30, 12));
    for terrain in ['.', '^', '.'] {
        let scenario = TestGrid::new(40, 24, |tile| if tile.x == 20 { terrain } else { '.' });
        let map = scenario;
        let ground = map.borrowed(&cache, false);
        let route = path(&ground, start, goal, None, CacheClass::Air, &mut scratch);
        assert_eq!(route.is_some(), terrain == '.');
        assert_eq!(cache.borrow().air.len(), 1);
    }
}

#[test]
fn capped_failures_never_become_exhausted_component_evidence() {
    let map = TestGrid::new(256, 256, |tile| {
        if (239..=241).contains(&tile.x)
            && (239..=241).contains(&tile.y)
            && tile != TilePos::new(240, 240)
        {
            '#'
        } else {
            '.'
        }
    });
    let cache = RefCell::new(PathQueries::default());
    let ground = map.borrowed(&cache, false);
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
                CacheClass::Ground,
                &mut scratch
            )
            .is_none()
        );
        assert!(!scratch.last_search_exhausted());
        assert!(!scratch.last_search_reached(start));
    }
    assert_eq!(
        bound(&ground, start, unreachable, CacheClass::Ground),
        u32::MAX
    );
    assert!(
        path(
            &ground,
            start,
            TilePos::new(12, 12),
            None,
            CacheClass::Ground,
            &mut scratch
        )
        .is_some()
    );
    assert!(!scratch.last_search_exhausted());
}

#[test]
fn cache_hits_clear_exhaustion_and_eviction_preserves_answers() {
    let cache = RefCell::new(PathQueries::default());
    let scenario = TestGrid::new(40, 24, |tile| if tile.x == 20 { '#' } else { '.' });
    let map = scenario;
    let ground = map.borrowed(&cache, false);
    let (start, goal, unreachable) = (
        TilePos::new(8, 12),
        TilePos::new(12, 12),
        TilePos::new(30, 12),
    );
    let mut scratch = Search::default();
    let expected = path(&ground, start, goal, None, CacheClass::Ground, &mut scratch).unwrap();
    assert!(
        path(
            &ground,
            start,
            unreachable,
            None,
            CacheClass::Ground,
            &mut scratch
        )
        .is_none()
    );
    assert!(scratch.last_search_exhausted());
    assert_eq!(
        path(&ground, start, goal, None, CacheClass::Ground, &mut scratch),
        Some(expected.clone())
    );
    assert!(!scratch.last_search_exhausted());
    let candidate = BlockedRect {
        anchor: goal,
        size: (1, 1),
    };
    assert!(
        path(
            &ground,
            start,
            goal,
            Some(candidate),
            CacheClass::Ground,
            &mut scratch
        )
        .is_none()
    );
    assert!(!scratch.last_search_exhausted());
    // A single field fits in its partition; replacing it retains the path.
    let cells = ground.grid.ground.len();
    cache.borrow_mut().ground[0].budget = cells + 4 * (cells * 4 + ENTRY_ALLOWANCE);
    assert_eq!(bound(&ground, start, goal, CacheClass::Ground), 40);
    assert_eq!(
        bound(&ground, start, unreachable, CacheClass::Ground),
        u32::MAX
    );
    assert_eq!(
        path(&ground, start, goal, None, CacheClass::Ground, &mut scratch),
        Some(expected.clone())
    );
    let retained = cache.borrow();
    assert!(
        cells + retained.ground[0].path_bytes + retained.ground[0].distance_bytes
            <= retained.ground[0].budget
    );
    assert!(retained.ground[0].paths.contains_key(&(None, start, goal)));
    drop(retained);
    {
        let mut cache = cache.borrow_mut();
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
        bound(&ground, start, goal, CacheClass::Ground),
        octile_cost(start, goal)
    );
    let other = TilePos::new(13, 12);
    assert!(
        path(
            &ground,
            start,
            other,
            None,
            CacheClass::Ground,
            &mut scratch
        )
        .is_some()
    );
    let blocked = TilePos::new(20, 12);
    assert_eq!(
        bound(&ground, blocked, goal, CacheClass::Ground),
        octile_cost(blocked, goal)
    );
    assert_eq!(
        bound(&ground, TilePos::new(-1, 0), goal, CacheClass::Ground),
        octile_cost(TilePos::new(-1, 0), goal)
    );
}

#[test]
fn distance_field_churn_preserves_routes_and_bounds_both_payload_classes() {
    let cache = RefCell::new(PathQueries::default());
    let scenario = TestGrid::new(40, 24, |_| '.');
    let map = scenario;
    let ground = map.borrowed(&cache, false);
    let mut scratch = Search::default();
    let start = TilePos::new(8, 12);
    let goal = TilePos::new(30, 12);
    let expected = path(&ground, start, goal, None, CacheClass::Air, &mut scratch).unwrap();
    let cells = ground.grid.air.len();
    let field_bytes = cells * size_of::<u32>() + ENTRY_ALLOWANCE;
    cache.borrow_mut().air[0].budget = cells + 4 * field_bytes;
    for x in 10..35 {
        bound(&ground, start, TilePos::new(x, 15), CacheClass::Air);
        let cache = cache.borrow();
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
            CacheClass::Air,
            &mut scratch,
        );
    }
    let cache = cache.borrow();
    let generation = &cache.air[0];
    assert!(generation.distances.contains_key(&TilePos::new(34, 15)));
    assert!(generation.path_bytes <= generation.payload_budget());
    assert!(generation.paths.len() < 27);
}

#[test]
fn exact_fields_preserve_canonical_paths_across_blocker_layouts_and_caps() {
    let mut scratch = chassis::path::AstarScratch::default();
    for (size, layouts) in [(3, 512), (7, 30)] {
        for layout in 0..layouts {
            let cells = (size * size) as usize;
            let mut rng = chassis::rng::Pcg32::new(layout as u64 + 1, 91);
            let blocked = (0..cells)
                .map(|i| {
                    if size == 3 {
                        layout & (1 << i) != 0
                    } else {
                        rng.next_u32().is_multiple_of(7)
                    }
                })
                .collect::<Vec<_>>();
            let grid = KnownGrid::new(size, size, &blocked).unwrap();
            let open = |tile| grid.open(tile, None);
            for goal_index in 0..cells {
                let goal = TilePos::new(goal_index as i32 % size, goal_index as i32 / size);
                let field = distance_field(size, size, goal, open);
                for start_index in 0..cells {
                    let start = TilePos::new(start_index as i32 % size, start_index as i32 / size);
                    for cap in [2, cells as u32] {
                        let expected = chassis::path::astar(size, size, start, goal, open, cap);
                        let actual = chassis::path::astar_with_distances(
                            (size, size),
                            start,
                            goal,
                            open,
                            cap,
                            &mut scratch,
                            &field,
                        );
                        assert_eq!(
                            actual, expected,
                            "size={size} layout={layout} start={start:?} goal={goal:?} cap={cap}"
                        );
                    }
                }
            }
        }
    }
    // An unavailable field must preserve ordinary routing rather than indexing it.
    assert_eq!(
        chassis::path::astar_with_distances(
            (4, 4),
            TilePos::new(0, 0),
            TilePos::new(3, 3),
            |_| true,
            16,
            &mut scratch,
            &[],
        ),
        chassis::path::astar(4, 4, TilePos::new(0, 0), TilePos::new(3, 3), |_| true, 16)
    );
}

#[test]
fn repeated_long_routes_share_candidate_fields_and_reduce_total_work() {
    let blocked = (0..80 * 60)
        .map(|i| i % 80 == 40 && i / 80 > 3)
        .collect::<Vec<_>>();
    let grid = KnownGrid::new(80, 60, &blocked).unwrap();
    let cache = RefCell::new(PathQueries::default());
    let board = PathBoard {
        grid,
        cache: &cache,
        class: CacheClass::Ground,
    };
    let goal = TilePos::new(4, 40);
    let overlay = Some(BlockedRect {
        anchor: TilePos::new(39, 2),
        size: (1, 1),
    });
    let starts = (20..50).map(|y| TilePos::new(70, y)).collect::<Vec<_>>();
    let (expected, before) = crate::bot::navigation::work::measure(|| {
        starts
            .iter()
            .map(|&start| {
                Search::default().path(80, 60, start, goal, |tile| grid.open(tile, overlay))
            })
            .collect::<Vec<_>>()
    });
    let (actual, after) = crate::bot::navigation::work::measure(|| {
        starts
            .iter()
            .map(|&start| board.path(start, goal, overlay, &mut Search::default()))
            .collect::<Vec<_>>()
    });
    assert_eq!(actual, expected);
    assert_eq!(after.fields, 1);
    assert!(
        after.expanded * 3 < before.expanded * 2,
        "before={before:?}, after={after:?}"
    );
    let (_, warm) = crate::bot::navigation::work::measure(|| {
        for (&start, expected) in starts.iter().zip(&expected) {
            assert_eq!(
                &board.path(start, goal, overlay, &mut Search::default()),
                expected
            );
        }
    });
    assert_eq!(warm.expanded, 0);
    assert_eq!(warm.fields, 0);
}

#[test]
fn candidate_field_eviction_preserves_normal_fields_and_exact_paths() {
    let blocked = vec![false; 40 * 24];
    let grid = KnownGrid::new(40, 24, &blocked).unwrap();
    let cache = RefCell::new(PathQueries::default());
    let board = PathBoard {
        grid,
        cache: &cache,
        class: CacheClass::Ground,
    };
    let start = TilePos::new(0, 12);
    let goal = TilePos::new(39, 12);
    let expected_bound = board.bound(start, goal);
    let normal = board.path(start, goal, None, &mut Search::default());
    let bytes = blocked.len() * size_of::<u32>() + ENTRY_ALLOWANCE;
    cache.borrow_mut().ground[0].budget = blocked.len() + 4 * bytes;
    for y in 0..24 {
        let overlay = Some(BlockedRect {
            anchor: TilePos::new(20, y),
            size: (1, 1),
        });
        let actual = board.path(start, goal, overlay, &mut Search::default());
        assert_eq!(
            actual,
            chassis::path::astar(
                40,
                24,
                start,
                goal,
                |tile| grid.open(tile, overlay),
                PATH_EXPANSION_CAP
            )
        );
        let cache = cache.borrow();
        let generation = &cache.ground[0];
        assert_eq!(generation.overlay_distances.len(), 1);
        assert_eq!(generation.distances.len(), 1);
        assert!(
            generation.blocked.len()
                + generation.path_bytes
                + generation.distance_bytes
                + generation.overlay_distance_bytes
                <= generation.budget
        );
    }
    assert_eq!(board.bound(start, goal), expected_bound);
    assert_eq!(
        board.path(start, goal, None, &mut Search::default()),
        normal
    );
}

#[test]
fn candidate_fields_fall_back_when_the_retention_budget_cannot_fit_one() {
    let blocked = vec![false; 40 * 24];
    let grid = KnownGrid::new(40, 24, &blocked).unwrap();
    let cache = RefCell::new(PathQueries::default());
    cache
        .borrow_mut()
        .generation(grid, CacheClass::Ground)
        .unwrap()
        .budget = blocked.len();
    let board = PathBoard {
        grid,
        cache: &cache,
        class: CacheClass::Ground,
    };
    let start = TilePos::new(0, 12);
    let goal = TilePos::new(39, 12);
    let overlay = Some(BlockedRect {
        anchor: TilePos::new(20, 12),
        size: (1, 1),
    });
    let (actual, work) = crate::bot::navigation::work::measure(|| {
        board.path(start, goal, overlay, &mut Search::default())
    });
    assert_eq!(
        actual,
        chassis::path::astar(
            40,
            24,
            start,
            goal,
            |tile| grid.open(tile, overlay),
            PATH_EXPANSION_CAP
        )
    );
    assert_eq!(work.fields, 0);
    let cache = cache.borrow();
    assert_eq!(cache.ground[0].overlay_distance_bytes, 0);
    assert_eq!(cache.ground[0].path_bytes, 0);
}

#[test]
fn invalid_long_route_endpoints_do_not_enter_field_arithmetic() {
    let blocked = vec![false; 40 * 24];
    let grid = KnownGrid::new(40, 24, &blocked).unwrap();
    let cache = RefCell::new(PathQueries::default());
    let board = PathBoard {
        grid,
        cache: &cache,
        class: CacheClass::Ground,
    };
    let start = TilePos::new(1, 1);
    let invalid = TilePos::new(i32::MIN, i32::MAX);
    let overlay = Some(BlockedRect {
        anchor: TilePos::new(20, 12),
        size: (1, 1),
    });
    let (_, work) = crate::bot::navigation::work::measure(|| {
        assert_eq!(
            board.path(invalid, start, overlay, &mut Search::default()),
            None
        );
        assert_eq!(
            board.path(start, invalid, overlay, &mut Search::default()),
            None
        );
    });
    assert_eq!(work.fields, 0);
}

#[test]
fn endpoint_pruning_does_not_build_fields_for_one_off_routes() {
    let grid = TestGrid::new(80, 60, |_| '.');
    let cache = RefCell::new(PathQueries::default());
    let borrowed = grid.borrowed(&cache, false);
    let board = borrowed.board(CacheClass::Ground);
    let starts = [TilePos::new(3, 20), TilePos::new(3, 21)];
    let goals = [TilePos::new(70, 20), TilePos::new(70, 21)];
    let (route, work) = crate::bot::navigation::work::measure(|| {
        shortest_path_between(board, &starts, &goals, None)
    });
    assert_eq!(
        route.unwrap(),
        (
            starts[0],
            goals[0],
            std::iter::once(starts[0])
                .chain(
                    Search::default()
                        .path(80, 60, starts[0], goals[0], |_| true)
                        .unwrap()
                )
                .collect()
        )
    );
    assert_eq!(work.fields, 0, "{work:?}");
    assert!(cache.borrow().ground[0].distances.is_empty());
}

#[test]
fn normal_fields_are_promoted_after_repeated_expensive_routes() {
    let grid = TestGrid::new(
        80,
        60,
        |tile| {
            if tile.x == 40 && tile.y > 3 { '#' } else { '.' }
        },
    );
    let cache = RefCell::new(PathQueries::default());
    let borrowed = grid.borrowed(&cache, false);
    let board = borrowed.board(CacheClass::Ground);
    let goal = TilePos::new(4, 40);
    let starts = (20..50).map(|y| TilePos::new(70, y)).collect::<Vec<_>>();
    let (expected, reference_work) = crate::bot::navigation::work::measure(|| {
        starts
            .iter()
            .map(|&start| {
                Search::default().path(80, 60, start, goal, |tile| board.grid.open(tile, None))
            })
            .collect::<Vec<_>>()
    });
    let (actual, work) = crate::bot::navigation::work::measure(|| {
        starts
            .iter()
            .map(|&start| board.path(start, goal, None, &mut Search::default()))
            .collect::<Vec<_>>()
    });
    assert_eq!(actual, expected);
    assert_eq!(work.fields, 1, "{work:?}");
    assert!(
        work.expanded < reference_work.expanded,
        "{work:?}, {reference_work:?}"
    );
    let (_, warm) = crate::bot::navigation::work::measure(|| {
        for (&start, expected) in starts.iter().zip(&expected) {
            assert_eq!(
                &board.path(start, goal, None, &mut Search::default()),
                expected
            );
        }
    });
    assert_eq!(warm.expanded, 0);
    assert_eq!(warm.fields, 0);
}

#[test]
fn repeated_doorstep_searches_share_an_earned_origin_field() {
    let grid = TestGrid::new(
        80,
        60,
        |tile| {
            if tile.x == 40 && tile.y > 3 { '#' } else { '.' }
        },
    );
    let cache = RefCell::new(PathQueries::default());
    let borrowed = grid.borrowed(&cache, false);
    let board = borrowed.board(CacheClass::Ground);
    let start = TilePos::new(70, 40);
    let goals = (20..50).map(|y| TilePos::new(4, y)).collect::<Vec<_>>();
    let (reference, exhaustive) = crate::bot::navigation::work::measure(|| {
        goals
            .iter()
            .filter_map(|&goal| {
                Search::default()
                    .path(80, 60, start, goal, |tile| board.grid.open(tile, None))
                    .map(|path| {
                        let path = std::iter::once(start).chain(path).collect::<Vec<_>>();
                        (path_cost(&path), path.len(), goal.y, goal.x, path)
                    })
            })
            .min()
            .unwrap()
    });
    let (selected, actual) = crate::bot::navigation::work::measure(|| {
        shortest_path_between(board, &[start], &goals, None).unwrap()
    });
    assert_eq!(
        selected,
        (start, TilePos::new(reference.3, reference.2), reference.4)
    );
    assert!(cache.borrow().ground[0].distances.contains_key(&start));
    assert!(
        actual.expanded < exhaustive.expanded,
        "{actual:?}, {exhaustive:?}"
    );
}

#[test]
fn cached_paths_do_not_charge_the_previous_search_to_a_new_endpoint_batch() {
    let grid = TestGrid::new(80, 60, |_| '.');
    let cache = RefCell::new(PathQueries::default());
    let borrowed = grid.borrowed(&cache, false);
    let board = borrowed.board(CacheClass::Ground);
    let start = TilePos::new(3, 20);
    let goal = TilePos::new(70, 20);
    let expected = board.path(start, goal, None, &mut Search::default());
    let mut search = Search::default();
    search.path(80, 60, start, goal, |tile| tile.x != 40);
    assert!(search.last_expansions() > 0);
    let (_, work) = crate::bot::navigation::work::measure(|| {
        assert_eq!(board.path(start, goal, None, &mut search), expected);
        board.prepare_endpoint_batch(start, goal, 100, &search);
    });
    assert_eq!(search.last_expansions(), 0);
    assert_eq!(work.fields, 0);
    assert_eq!(work.expanded, 0);
}
