use super::*;
use crate::grid::cell_count;
use crate::rng::Pcg32;

fn below(rng: &mut Pcg32, bound: i32) -> i32 {
    i32::try_from(rng.next_below(u32::try_from(bound).unwrap())).unwrap()
}

#[test]
fn expansion_count_tracks_success_exhaustion_caps_and_early_exits() {
    let mut scratch = AstarScratch::default();
    let start = TilePos::new(0, 0);
    let goal = TilePos::new(4, 0);
    assert!(astar_with_scratch(5, 1, start, goal, |_| true, 10, &mut scratch).is_some());
    assert_eq!(scratch.last_expansions(), 4);
    assert!(astar_with_scratch(5, 1, start, goal, |_| true, 1, &mut scratch).is_none());
    assert_eq!(scratch.last_expansions(), 2);
    assert!(!scratch.last_search_exhausted());
    assert!(astar_with_scratch(5, 1, start, goal, |tile| tile.x != 2, 10, &mut scratch).is_none());
    assert_eq!(scratch.last_expansions(), 2);
    assert!(scratch.last_search_exhausted());
    assert!(astar_with_scratch(5, 1, start, goal, |_| false, 10, &mut scratch).is_none());
    assert_eq!(scratch.last_expansions(), 0);
    assert!(astar_with_scratch(5, 1, start, start, |_| false, 10, &mut scratch).is_some());
    assert_eq!(scratch.last_expansions(), 0);
    assert!(astar_with_scratch(0, 0, start, goal, |_| true, 10, &mut scratch).is_none());
    assert_eq!(scratch.last_expansions(), 0);
}

/// Reference A* with the same expansion body and a global
/// `(f, h, query-oriented rank, index)` tuple heap for the open set. The
/// dial must pop this exact order, so any divergence in paths or
/// exhaustion isolates to the queue.
fn reference_astar(
    width: i32,
    height: i32,
    start: TilePos,
    goal: TilePos,
    mut passable: impl FnMut(TilePos) -> bool,
    max_expansions: u32,
) -> Option<Vec<TilePos>> {
    let in_bounds = |p: TilePos| p.x >= 0 && p.y >= 0 && p.x < width && p.y < height;
    if !in_bounds(start) || !in_bounds(goal) {
        return None;
    }
    let index = |p: TilePos| p.row_major(width);
    if start == goal {
        return Some(Vec::new());
    }
    if !passable(goal) {
        return None;
    }
    let cells = crate::grid::cell_count(width, height);
    let reverse_ties = (goal.y, goal.x) < (start.y, start.x);
    let tie_rank = |cell_index: usize| {
        if reverse_ties {
            cells - 1 - cell_index
        } else {
            cell_index
        }
    };
    let mut best_g = vec![u32::MAX; cells];
    let mut came_from = vec![usize::MAX; cells];
    let mut open = BinaryHeap::new();
    best_g[index(start)] = 0;
    open.push(Reverse((
        heuristic(start, goal),
        heuristic(start, goal),
        tie_rank(index(start)),
        index(start),
    )));
    let mut expansions = 0;
    while let Some(Reverse((f, _h, _rank, current_idx))) = open.pop() {
        let current = TilePos::from_row_major(current_idx, width);
        let g = best_g[current_idx];
        if f > g.saturating_add(heuristic(current, goal)) {
            continue;
        }
        if current == goal {
            let mut path = Vec::new();
            let mut idx = current_idx;
            while idx != index(start) {
                path.push(TilePos::from_row_major(idx, width));
                idx = came_from[idx];
            }
            path.reverse();
            return Some(path);
        }
        expansions += 1;
        if expansions > max_expansions {
            return None;
        }
        let mut visit = |next: TilePos, step_cost: u32, open: &mut BinaryHeap<_>| {
            let next_idx = index(next);
            let tentative = g + step_cost;
            if tentative < best_g[next_idx] {
                best_g[next_idx] = tentative;
                came_from[next_idx] = current_idx;
                let h = heuristic(next, goal);
                open.push(Reverse((tentative + h, h, tie_rank(next_idx), next_idx)));
            }
        };
        let mut cardinal_open = [false; 4];
        for (dx, dy) in CARDINALS {
            let next = current.offset(dx, dy);
            if in_bounds(next) && passable(next) {
                let slot = if dy == 0 {
                    usize::from(dx < 0)
                } else {
                    2 + usize::from(dy < 0)
                };
                cardinal_open[slot] = true;
                visit(next, STRAIGHT_COST, &mut open);
            }
        }
        for (dx, dy) in DIAGONALS {
            if cardinal_open[usize::from(dx < 0)] && cardinal_open[2 + usize::from(dy < 0)] {
                let next = current.offset(dx, dy);
                if in_bounds(next) && passable(next) {
                    visit(next, DIAGONAL_COST, &mut open);
                }
            }
        }
    }
    None
}

/// Hundreds of random worlds, byte-compared against the oracle:
/// walls at varied density, non-square dimensions, unreachable
/// goals, tight expansion caps, and scratch reuse across all of
/// it. The dial must match the tuple heap path for path.
#[test]
fn the_dial_matches_the_tuple_heap_on_random_worlds() {
    let mut rng = crate::rng::Pcg32::new(90210, 7);
    let mut scratch = AstarScratch::default();
    for case in 0..300u32 {
        let width = 4 + below(&mut rng, 40);
        let height = 4 + below(&mut rng, 28);
        let density = rng.next_below(45);
        let cells = cell_count(width, height);
        let mut walls = vec![false; cells];
        for wall in &mut walls {
            *wall = rng.next_below(100) < density;
        }
        let start = TilePos::new(below(&mut rng, width), below(&mut rng, height));
        let goal = TilePos::new(below(&mut rng, width), below(&mut rng, height));
        let max_expansions = if rng.next_below(5) == 0 {
            1 + rng.next_below(30)
        } else {
            10_000
        };
        let passable = |p: TilePos| !walls[p.row_major(width)] || p == start;
        let expected = reference_astar(width, height, start, goal, passable, max_expansions);
        let actual = astar_with_scratch(
            width,
            height,
            start,
            goal,
            passable,
            max_expansions,
            &mut scratch,
        );
        assert_eq!(
            actual, expected,
            "case {case}: {width}x{height} density {density} start {start:?} goal {goal:?}"
        );
    }
}
use crate::grid::Grid;

/// Component labels agree with an A* oracle on random masks: two open
/// tiles share a label exactly when A* connects them, and a closed start
/// reaches exactly the components of its open cardinal neighbours.
#[test]
fn cardinal_components_match_astar_reachability() {
    let mut rng = crate::rng::Pcg32::new(0x00C0_FFEE, 3);
    for case in 0..120u32 {
        let width = 1 + below(&mut rng, 9);
        let height = 1 + below(&mut rng, 7);
        let density = rng.next_below(60);
        let walls: Vec<bool> = (0..width * height)
            .map(|_| rng.next_below(100) < density)
            .collect();
        let open = |p: TilePos| !walls[p.row_major(width)];
        let labels = cardinal_components(width, height, open);
        assert_eq!(labels.len(), cell_count(width, height));
        let label = |p: TilePos| labels[p.row_major(width)];
        let tiles: Vec<TilePos> = (0..height)
            .flat_map(|y| (0..width).map(move |x| TilePos::new(x, y)))
            .collect();
        let mut seen = 0;
        for &tile in &tiles {
            assert_eq!(label(tile) == 0, !open(tile), "case {case}: {tile:?}");
            if label(tile) > seen {
                assert_eq!(label(tile), seen + 1, "labels number in discovery order");
                seen = label(tile);
            }
        }
        for &start in &tiles {
            let reachable: Vec<u32> = if open(start) {
                vec![label(start)]
            } else {
                CARDINALS
                    .iter()
                    .map(|&(dx, dy)| start.offset(dx, dy))
                    .filter(|t| t.x >= 0 && t.y >= 0 && t.x < width && t.y < height)
                    .map(label)
                    .filter(|&l| l != 0)
                    .collect()
            };
            for &goal in &tiles {
                let routed = astar(width, height, start, goal, open, 10_000).is_some();
                let labelled = start == goal || reachable.contains(&label(goal));
                assert_eq!(
                    routed, labelled,
                    "case {case}: {width}x{height} {start:?} -> {goal:?}"
                );
            }
        }
    }
    assert!(cardinal_components(0, 3, |_| true).is_empty());
    assert!(cardinal_components(-1, 3, |_| true).is_empty());
}

/// Builds a passability closure from ASCII rows ('#' blocked).
fn arena(rows: &[&str]) -> (Grid<bool>, i32, i32) {
    let height = i32::try_from(rows.len()).unwrap();
    let width = i32::try_from(rows[0].len()).unwrap();
    let cells = rows
        .iter()
        .flat_map(|r| r.chars())
        .map(|c| c != '#')
        .collect();
    (Grid::from_cells(width, height, cells), width, height)
}

fn find(rows: &[&str], start: (i32, i32), goal: (i32, i32)) -> Option<Vec<TilePos>> {
    let (grid, w, h) = arena(rows);
    astar(
        w,
        h,
        TilePos::new(start.0, start.1),
        TilePos::new(goal.0, goal.1),
        |p| *grid.get(p).unwrap(),
        10_000,
    )
}

#[test]
fn straight_line_is_direct() {
    let path = find(&["....", "....", "...."], (0, 1), (3, 1)).unwrap();
    assert_eq!(path.len(), 3);
    assert_eq!(path.last(), Some(&TilePos::new(3, 1)));
}

#[test]
fn diagonal_line_uses_diagonal_steps() {
    let path = find(&["....", "....", "....", "...."], (0, 0), (3, 3)).unwrap();
    assert_eq!(path.len(), 3, "pure diagonal should take 3 steps, not 6");
}

#[test]
fn routes_around_walls() {
    let path = find(&[".#.", ".#.", "..."], (0, 0), (2, 0)).unwrap();
    assert_eq!(path.last(), Some(&TilePos::new(2, 0)));
    // Must detour below the wall: 2 down-ish, across, 2 up-ish.
    assert!(path.len() >= 4);
    assert!(path.iter().all(|p| p.x != 1 || p.y == 2));
}

#[test]
fn does_not_cut_corners() {
    // Diagonal from (0,0) to (1,1) is blocked by the two '#' tiles even
    // though (1,1) itself is open.
    let path = find(&[".#", "#."], (0, 0), (1, 1));
    assert_eq!(path, None);
}

#[test]
fn unreachable_goal_returns_none() {
    assert_eq!(find(&[".#.", "###", ".#."], (0, 0), (2, 2)), None);
}

#[test]
fn goal_on_blocked_tile_returns_none() {
    assert_eq!(find(&["..", ".#"], (0, 0), (1, 1)), None);
}

#[test]
fn same_start_and_goal_is_empty_path() {
    assert_eq!(find(&[".."], (0, 0), (0, 0)), Some(Vec::new()));
}

#[test]
fn path_is_deterministic_across_repeated_queries() {
    let rows = &["........", ".##..##.", ".#....#.", "........"];
    let first = find(rows, (0, 0), (7, 3)).unwrap();
    for _ in 0..10 {
        assert_eq!(find(rows, (0, 0), (7, 3)).unwrap(), first);
    }
}

#[test]
fn paths_are_equivariant_under_half_turns() {
    fn assert_mirror_path(
        width: i32,
        height: i32,
        blocked: &[bool],
        start: TilePos,
        goal: TilePos,
    ) {
        let rotate = |tile: TilePos| TilePos::new(width - 1 - tile.x, height - 1 - tile.y);
        let open = |tile: TilePos| !blocked[tile.row_major(width)];
        let rotated_open = |tile: TilePos| open(rotate(tile));
        let path = astar(width, height, start, goal, open, 10_000);
        let rotated = astar(
            width,
            height,
            rotate(start),
            rotate(goal),
            rotated_open,
            10_000,
        );
        let expected = path.map(|path| path.into_iter().map(rotate).collect::<Vec<_>>());
        assert_eq!(
            rotated, expected,
            "half-turn changed the canonical route from {start:?} to {goal:?}"
        );
    }

    let width = 7;
    let height = 7;
    let mut counterexample = vec![false; cell_count(width, height)];
    for (x, y) in [
        (0, 5),
        (1, 0),
        (2, 0),
        (2, 2),
        (2, 4),
        (3, 0),
        (3, 6),
        (4, 2),
        (4, 4),
        (4, 6),
        (5, 6),
        (6, 1),
    ] {
        counterexample[TilePos::new(x, y).row_major(width)] = true;
    }
    assert_mirror_path(
        width,
        height,
        &counterexample,
        TilePos::new(2, 1),
        TilePos::new(2, 5),
    );

    let mut rng = crate::rng::Pcg32::new(0x0018_0DE6, 11);
    for _ in 0..256 {
        let mut blocked = (0..width * height)
            .map(|_| rng.next_below(100) < 24)
            .collect::<Vec<_>>();
        let start = TilePos::new(below(&mut rng, width), below(&mut rng, height));
        let mut goal = TilePos::new(below(&mut rng, width), below(&mut rng, height));
        if goal == start {
            goal.x = (goal.x + 1) % width;
        }
        blocked[start.row_major(width)] = false;
        blocked[goal.row_major(width)] = false;
        assert_mirror_path(width, height, &blocked, start, goal);
    }
}

#[test]
fn reusable_scratch_matches_fresh_queries_across_maps_and_failures() {
    let cases = [
        (&["....", ".##.", "...."][..], (0, 0), (3, 2)),
        (&[".#.", "###", ".#."][..], (0, 0), (2, 2)),
        (&["...."][..], (2, 0), (2, 0)),
    ];
    let mut scratch = AstarScratch::default();
    for (rows, start, goal) in cases {
        let (grid, width, height) = arena(rows);
        let start = TilePos::new(start.0, start.1);
        let goal = TilePos::new(goal.0, goal.1);
        let fresh = astar(
            width,
            height,
            start,
            goal,
            |tile| *grid.get(tile).unwrap(),
            10_000,
        );
        let reused = astar_with_scratch(
            width,
            height,
            start,
            goal,
            |tile| *grid.get(tile).unwrap(),
            10_000,
            &mut scratch,
        );
        assert_eq!(reused, fresh);
    }
}

fn prime_exhausted_scratch(scratch: &mut AstarScratch) {
    let (grid, width, height) = arena(&["..#..", "..#..", "..#.."][..]);
    assert_eq!(
        astar_with_scratch(
            width,
            height,
            TilePos::new(0, 1),
            TilePos::new(4, 1),
            |tile| *grid.get(tile).unwrap(),
            10_000,
            scratch,
        ),
        None
    );
    assert!(scratch.last_search_exhausted());
    assert!(scratch.last_search_reached(TilePos::new(1, 2)));
}

#[test]
fn clearing_evidence_retains_storage_and_future_routes() {
    let mut scratch = AstarScratch::default();
    prime_exhausted_scratch(&mut scratch);
    let capacities = (
        scratch.best_g.capacity(),
        scratch.came_from.capacity(),
        scratch.stamp.capacity(),
    );
    scratch.clear_search_evidence();
    assert!(!scratch.last_search_exhausted());
    assert!(!scratch.last_search_reached(TilePos::new(1, 2)));
    assert_eq!(
        capacities,
        (
            scratch.best_g.capacity(),
            scratch.came_from.capacity(),
            scratch.stamp.capacity()
        )
    );
    let search = |scratch: &mut AstarScratch| {
        astar_with_scratch(
            8,
            6,
            TilePos::new(0, 0),
            TilePos::new(7, 5),
            |tile| tile.x != 3 || tile.y == 4,
            1000,
            scratch,
        )
    };
    assert_eq!(search(&mut scratch), search(&mut AstarScratch::default()));
}

#[test]
fn every_early_return_clears_previous_reachability() {
    let mut scratch = AstarScratch::default();

    prime_exhausted_scratch(&mut scratch);
    assert_eq!(
        astar_with_scratch(
            5,
            3,
            TilePos::new(0, 0),
            TilePos::new(0, 0),
            |_| true,
            10_000,
            &mut scratch,
        ),
        Some(Vec::new())
    );
    assert!(!scratch.last_search_exhausted());
    assert!(!scratch.last_search_reached(TilePos::new(0, 1)));
    assert!(!scratch.last_search_reached(TilePos::new(1, 2)));

    prime_exhausted_scratch(&mut scratch);
    assert_eq!(
        astar_with_scratch(
            5,
            3,
            TilePos::new(0, 0),
            TilePos::new(4, 2),
            |tile| tile != TilePos::new(4, 2),
            10_000,
            &mut scratch,
        ),
        None
    );
    assert!(!scratch.last_search_exhausted());
    assert!(!scratch.last_search_reached(TilePos::new(1, 2)));

    prime_exhausted_scratch(&mut scratch);
    assert_eq!(
        astar_with_scratch(
            5,
            3,
            TilePos::new(-1, 0),
            TilePos::new(4, 2),
            |_| true,
            10_000,
            &mut scratch,
        ),
        None
    );
    assert!(!scratch.last_search_exhausted());
    assert!(!scratch.last_search_reached(TilePos::new(1, 2)));
}

#[test]
fn cheap_failures_do_not_allocate_the_claimed_grid() {
    let mut scratch = AstarScratch::default();
    prime_exhausted_scratch(&mut scratch);
    let retained_cells = scratch.best_g.len();

    assert_eq!(
        astar_with_scratch(
            i32::MAX,
            i32::MAX,
            TilePos::new(-1, 0),
            TilePos::new(1, 0),
            |_| true,
            1,
            &mut scratch,
        ),
        None
    );
    assert_eq!(scratch.best_g.len(), retained_cells);

    assert_eq!(
        astar_with_scratch(
            100_000,
            100_000,
            TilePos::new(0, 0),
            TilePos::new(1, 0),
            |_| false,
            1,
            &mut scratch,
        ),
        None
    );
    assert_eq!(scratch.best_g.len(), retained_cells);
    assert!(!scratch.last_search_exhausted());
    assert!(!scratch.last_search_reached(TilePos::new(1, 2)));
}

#[test]
fn only_complete_component_searches_advertise_reachability() {
    let mut scratch = AstarScratch::default();
    let open = [".....", ".....", "....."];
    let (grid, width, height) = arena(&open);

    assert!(
        astar_with_scratch(
            width,
            height,
            TilePos::new(0, 1),
            TilePos::new(4, 1),
            |tile| *grid.get(tile).unwrap(),
            10_000,
            &mut scratch,
        )
        .is_some()
    );
    assert!(!scratch.last_search_exhausted());

    assert_eq!(
        astar_with_scratch(
            width,
            height,
            TilePos::new(0, 1),
            TilePos::new(4, 1),
            |tile| *grid.get(tile).unwrap(),
            0,
            &mut scratch,
        ),
        None
    );
    assert!(!scratch.last_search_exhausted());
    assert!(!scratch.last_search_reached(TilePos::new(0, 1)));
}

#[test]
fn scratch_reuse_preserves_exact_ties_across_dimension_changes() {
    let mut scratch = AstarScratch::default();
    let (large, width, height) = arena(&[".......", ".......", "...#...", ".......", "......."]);
    let first = astar_with_scratch(
        width,
        height,
        TilePos::new(0, 2),
        TilePos::new(6, 2),
        |tile| *large.get(tile).unwrap(),
        10_000,
        &mut scratch,
    )
    .unwrap();
    assert_eq!(
        first,
        vec![
            TilePos::new(1, 2),
            TilePos::new(2, 1),
            TilePos::new(3, 1),
            TilePos::new(4, 1),
            TilePos::new(5, 2),
            TilePos::new(6, 2),
        ]
    );

    let (small, small_width, small_height) = arena(&[".#.", "###", ".#."]);
    assert_eq!(
        astar_with_scratch(
            small_width,
            small_height,
            TilePos::new(0, 0),
            TilePos::new(2, 2),
            |tile| *small.get(tile).unwrap(),
            10_000,
            &mut scratch,
        ),
        None
    );
    assert!(scratch.last_search_exhausted());

    let again = astar_with_scratch(
        width,
        height,
        TilePos::new(0, 2),
        TilePos::new(6, 2),
        |tile| *large.get(tile).unwrap(),
        10_000,
        &mut scratch,
    )
    .unwrap();
    assert_eq!(again, first);
}

#[test]
fn generation_wraparound_cannot_alias_stale_cells() {
    // Stamp a bunch of cells at generation u32::MAX, then force the next
    // query to wrap. The wrap must clear the stamp grid so cells touched
    // by the old query cannot masquerade as reachable in the new one.
    let mut scratch = AstarScratch::default();
    scratch.force_generation(u32::MAX - 1);
    let (grid, width, height) = arena(&["..#..", "..#..", "..#.."]);
    let walled = |tile: TilePos| *grid.get(tile).unwrap();
    assert_eq!(
        astar_with_scratch(
            width,
            height,
            TilePos::new(0, 1),
            TilePos::new(4, 1),
            walled,
            10_000,
            &mut scratch,
        ),
        None
    );
    assert!(scratch.last_search_exhausted());
    assert!(scratch.last_search_reached(TilePos::new(1, 2)));

    // This query wraps the counter. Only the right half is reachable now.
    let (mirror, ..) = arena(&["..#..", "..#..", "..#.."]);
    let east = |tile: TilePos| *mirror.get(tile).unwrap();
    assert_eq!(
        astar_with_scratch(
            width,
            height,
            TilePos::new(4, 1),
            TilePos::new(0, 1),
            east,
            10_000,
            &mut scratch,
        ),
        None
    );
    assert!(scratch.last_search_exhausted());
    assert!(scratch.last_search_reached(TilePos::new(3, 2)));
    assert!(
        !scratch.last_search_reached(TilePos::new(1, 2)),
        "west-side cell from the pre-wrap query must read stale"
    );
}

#[test]
fn reachability_reflects_only_the_latest_exhausted_query() {
    // Two exhausted queries on same-sized maps with disjoint reachable
    // components: the second query's answers must not inherit cells the
    // first one touched.
    let mut scratch = AstarScratch::default();
    prime_exhausted_scratch(&mut scratch); // reaches the WEST of the wall
    let (grid, width, height) = arena(&["..#..", "..#..", "..#.."]);
    assert_eq!(
        astar_with_scratch(
            width,
            height,
            TilePos::new(4, 1),
            TilePos::new(0, 1),
            |tile| *grid.get(tile).unwrap(),
            10_000,
            &mut scratch,
        ),
        None
    );
    assert!(scratch.last_search_exhausted());
    assert!(scratch.last_search_reached(TilePos::new(4, 0)));
    assert!(
        !scratch.last_search_reached(TilePos::new(0, 0)),
        "cells reached only by the prior query must not leak through"
    );
}

#[test]
fn exhausted_search_proves_reachability_for_alternate_goals() {
    let (grid, width, height) = arena(&["..#..", "..#..", "..#.."]);
    let mut scratch = AstarScratch::default();
    let path = astar_with_scratch(
        width,
        height,
        TilePos::new(0, 1),
        TilePos::new(4, 1),
        |tile| *grid.get(tile).unwrap(),
        10_000,
        &mut scratch,
    );
    assert_eq!(path, None);
    assert!(scratch.last_search_exhausted());
    assert!(scratch.last_search_reached(TilePos::new(1, 2)));
    assert!(!scratch.last_search_reached(TilePos::new(3, 1)));
}

fn center(x: i32, y: i32) -> Vec2Fx {
    TilePos::new(x, y).center()
}

#[test]
fn line_across_open_ground_is_clear() {
    let (grid, ..) = arena(&["......", "......", "......"]);
    let clear = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    assert!(!line_blocked(center(0, 1), center(5, 1), clear));
    assert!(!line_blocked(center(0, 0), center(5, 2), clear));
    assert!(!line_blocked(center(2, 2), center(2, 2), clear));
}

#[test]
fn line_through_a_wall_is_blocked() {
    let (grid, ..) = arena(&["...#..", "...#..", "...#.."]);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    assert!(line_blocked(center(0, 1), center(5, 1), open));
    assert!(line_blocked(center(1, 0), center(5, 2), open));
    // Parallel to the wall on the open side: clear.
    assert!(!line_blocked(center(0, 0), center(2, 2), open));
}

#[test]
fn endpoints_never_block() {
    // Target stands on (2,1), which is itself impassable (a building
    // tile): the segment to it must not count the endpoint.
    let (grid, ..) = arena(&["....", "..#.", "...."]);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    assert!(!line_blocked(center(0, 1), center(2, 1), open));
    assert!(!line_blocked(center(2, 1), center(0, 1), open));
}

#[test]
fn exact_corner_crossing_cannot_slip_between_blockers() {
    // The diagonal from (0,0) to (1,1) passes exactly through the
    // corner shared with (1,0) and (0,1) — both blocked.
    let (grid, ..) = arena(&[".#", "#."]);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    assert!(line_blocked(center(0, 0), center(1, 1), open));
}

#[test]
fn line_is_deterministic_and_symmetric_enough() {
    let rows = &["........", "..##....", "....#...", "........"];
    let (grid, ..) = arena(rows);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    let forward = line_blocked(center(0, 0), center(7, 3), open);
    for _ in 0..10 {
        assert_eq!(line_blocked(center(0, 0), center(7, 3), open), forward);
    }
}

#[test]
fn hairline_deltas_cannot_overflow_the_trace() {
    // Two machines a single fixed-point ulp apart straddling a tile
    // boundary: the parametric setup wants 1/ulp = 2^32, past the
    // type's range. Adversarial in x, in y, and in both at once, in both
    // directions, over open and blocked ground.
    let (grid, ..) = arena(&["....", "....", "....", "...."]);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    let ulp = Fx::from_bits(1);
    let edge_x = Fx::from_num(2);
    let edge_y = Fx::from_num(2);
    let cases = [
        // x hairline, y level.
        (
            Vec2Fx::new(edge_x - ulp, Fx::lit("1.5")),
            Vec2Fx::new(edge_x + ulp, Fx::lit("1.5")),
        ),
        // y hairline, x level.
        (
            Vec2Fx::new(Fx::lit("1.5"), edge_y - ulp),
            Vec2Fx::new(Fx::lit("1.5"), edge_y + ulp),
        ),
        // A diagonal hair across the corner.
        (
            Vec2Fx::new(edge_x - ulp, edge_y - ulp),
            Vec2Fx::new(edge_x + ulp, edge_y + ulp),
        ),
        // Hairline in x while y spans real distance (one axis
        // saturates, the other walks normally).
        (
            Vec2Fx::new(edge_x - ulp, Fx::lit("0.5")),
            Vec2Fx::new(edge_x + ulp, Fx::lit("3.5")),
        ),
    ];
    for (a, b) in cases {
        let fwd = line_blocked(a, b, open);
        let back = line_blocked(b, a, open);
        assert!(!fwd && !back, "open ground stays open for {a:?}->{b:?}");
    }
    // Same hairlines against a wall: the blocker must still be seen.
    let (walled, ..) = arena(&["....", "..#.", "..#.", "...."]);
    let solid = |p: TilePos| walled.get(p).copied().unwrap_or(false);
    let a = Vec2Fx::new(edge_x - ulp, Fx::lit("0.5"));
    let b = Vec2Fx::new(edge_x + ulp, Fx::lit("3.5"));
    assert!(line_blocked(a, b, solid), "the wall is on the path");
    assert!(line_blocked(b, a, solid), "in both directions");
}

#[test]
fn trace_is_mirror_fair() {
    // A 180°-rotated shot over 180°-rotated terrain gets the identical
    // verdict, so mirrored seats never disagree about the same
    // engagement. Direction symmetry along one segment is not promised;
    // see the doc comment.
    let rows = &["........", "..##....", "....#...", ".#......", "........"];
    let (grid, w, h) = arena(rows);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    // The rotated world: cell (x, y) holds what (w-1-x, h-1-y) held.
    let rot_open = |p: TilePos| {
        grid.get(TilePos::new(w - 1 - p.x, h - 1 - p.y))
            .copied()
            .unwrap_or(false)
    };
    let rot = |v: Vec2Fx| Vec2Fx::new(Fx::from_num(w) - v.x, Fx::from_num(h) - v.y);
    for ax in 0..w {
        for ay in 0..h {
            for bx in 0..w {
                for by in 0..h {
                    let (a, b) = (center(ax, ay), center(bx, by));
                    assert_eq!(
                        line_blocked(a, b, open),
                        line_blocked(rot(a), rot(b), rot_open),
                        "mirror-unfair trace {ax},{ay} -> {bx},{by}"
                    );
                }
            }
        }
    }
}

#[test]
fn swept_line_respects_the_body_radius() {
    let (grid, _, _) = arena(&["......", "......", "..#...", "......"]);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    let (a, b) = (center(0, 1), center(5, 1));
    assert!(!line_blocked(a, b, open));
    assert!(!swept_line_blocked(a, b, Fx::lit("0.3"), open));
    assert!(swept_line_blocked(a, b, Fx::lit("0.6"), open));
    assert!(!swept_line_blocked(a, a, Fx::lit("0.6"), open));
}

#[test]
fn swept_line_catches_a_pillar_between_its_edges() {
    let (grid, _, _) = arena(&["......", "..#...", "......"]);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    let (a, b) = (center(0, 1), center(5, 1));
    assert!(line_blocked(a, b, open));
    assert!(swept_line_blocked(a, b, Fx::lit("0.6"), open));
}

#[test]
fn swept_line_catches_a_blocked_tile_its_hull_already_overlaps() {
    let (grid, _, _) = arena(&["......", ".##...", ".##...", "......", "......"]);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    // Just south of the block's south-east corner, heading past it: the
    // center line clears the corner, but the hull starts over the block.
    let a = Vec2Fx::new(Fx::lit("2.817357315"), Fx::lit("3.1853985682"));
    let b = center(5, 0);
    assert!(!line_blocked(a, b, open));
    assert!(swept_line_blocked(a, b, Fx::lit("0.3"), open));
    // A body leaving blocked ground is not held by its own tile.
    assert!(!swept_line_blocked(
        center(1, 1),
        center(0, 1),
        Fx::lit("0.3"),
        open
    ));
}

#[test]
fn swept_trace_is_mirror_fair() {
    let rows = &["........", "..##....", "....#...", ".#......", "........"];
    let (grid, w, h) = arena(rows);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    let rot_open = |p: TilePos| {
        grid.get(TilePos::new(w - 1 - p.x, h - 1 - p.y))
            .copied()
            .unwrap_or(false)
    };
    let rot = |v: Vec2Fx| Vec2Fx::new(Fx::from_num(w) - v.x, Fx::from_num(h) - v.y);
    // Ground hulls from the smallest to the widest; at 0.5 a tile-centred
    // edge lies exactly on a tile boundary.
    for radius in ["0.26", "0.35", "0.45", "0.5", "0.55"].map(Fx::lit) {
        for ax in 0..w {
            for ay in 0..h {
                for bx in 0..w {
                    for by in 0..h {
                        let (a, b) = (center(ax, ay), center(bx, by));
                        assert_eq!(
                            swept_line_blocked(a, b, radius, open),
                            swept_line_blocked(rot(a), rot(b), radius, rot_open),
                            "mirror-unfair swept trace {ax},{ay} -> {bx},{by} at {radius}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_hull_touching_a_wall_sweeps_along_it_on_either_side() {
    let (grid, _, _) = arena(&["######", "......", "######"]);
    let open = |p: TilePos| grid.get(p).copied().unwrap_or(false);
    // Legs of one and four tiles put a 0.5 edge exactly on the walls' faces.
    let half = Fx::lit("0.5");
    for (from, to) in [(1, 5), (5, 1), (2, 3), (3, 2)] {
        assert!(!swept_line_blocked(
            center(from, 1),
            center(to, 1),
            half,
            open
        ));
    }
    assert!(swept_line_blocked(
        center(0, 1),
        center(5, 1),
        Fx::lit("0.55"),
        open
    ));
}

#[test]
fn expansion_cap_gives_up_gracefully() {
    let (grid, w, h) = arena(&["....", "....", "...."]);
    let capped = astar(
        w,
        h,
        TilePos::new(0, 0),
        TilePos::new(3, 2),
        |p| *grid.get(p).unwrap(),
        1,
    );
    assert_eq!(capped, None);
}
