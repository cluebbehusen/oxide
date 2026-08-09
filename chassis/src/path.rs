//! Deterministic A* over a tile grid.
//!
//! Eight-directional movement with no corner cutting: a diagonal step is
//! legal only when both adjacent cardinal tiles are passable, so a unit can
//! never squeeze between two blockers. Costs are integers (10 straight,
//! 14 diagonal) and the octile heuristic is exact for this movement model,
//! so returned paths are optimal.
//!
//! Determinism: the open set orders by `(f, h, tile index)` — the index makes
//! every key unique, so ties cannot fall through to any unspecified heap
//! behavior. Same query, same path, every time.

use crate::fx::{Fx, Vec2Fx};
use crate::grid::{CARDINALS, DIAGONALS, TilePos};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Whether the open segment between `a` and `b` crosses a tile that fails
/// `passable`. The endpoints' own tiles are never tested — a shooter fires
/// *from* its tile and a wall-mounted target is hit *on* its tile.
///
/// Deterministic supercover traversal (Amanatides–Woo in fixed point):
/// every tile the segment passes through is visited; an exact corner
/// crossing conservatively visits both adjacent tiles, so a diagonal shot
/// cannot slip between two blockers, mirroring [`astar`]'s no-corner-cut
/// rule.
///
/// Direction symmetry is NOT guaranteed: a segment that grazes a tile
/// corner exactly can round to opposite sides of it depending on which
/// end the walk starts from (1/7-slope shots, say — the reciprocal is
/// inexact in binary). What IS guaranteed, and what seat fairness rests
/// on, is mirror symmetry: a 180°-rotated segment over 180°-rotated
/// terrain computes the identical verdict, because every quantity here
/// is sign-symmetric. A test pins that property.
pub fn line_blocked(a: Vec2Fx, b: Vec2Fx, mut passable: impl FnMut(TilePos) -> bool) -> bool {
    let start = TilePos::containing(a);
    let end = TilePos::containing(b);
    if start == end {
        return false;
    }
    let delta = b - a;
    let step_x: i32 = if delta.x > Fx::ZERO {
        1
    } else if delta.x < Fx::ZERO {
        -1
    } else {
        0
    };
    let step_y: i32 = if delta.y > Fx::ZERO {
        1
    } else if delta.y < Fx::ZERO {
        -1
    } else {
        0
    };
    // Parametric distance (0..1 along the segment) to the next x/y tile
    // boundary, and per-tile increments — SATURATING Q32.32 arithmetic.
    // A delta component can be as small as one fixed-point ulp (two
    // machines a hair apart across a tile boundary), and 1/ulp is 2^32 —
    // past the type's ceiling. Saturation is exactly correct here: a
    // t_max at MAX means "this axis crosses no more boundaries within
    // the segment," which is precisely how the zero-delta arm already
    // behaves, and every non-degenerate segment computes bit-identical
    // values to the plain arithmetic. Deltas are div'd by their abs
    // (positive), so saturation lands on MAX and never on MIN, whose
    // own abs would panic.
    let (mut t_max_x, t_delta_x) = if step_x == 0 {
        (Fx::MAX, Fx::MAX)
    } else {
        let next_boundary = Fx::from_num(if step_x > 0 { start.x + 1 } else { start.x });
        (
            (next_boundary - a.x).abs().saturating_div(delta.x.abs()),
            Fx::ONE.saturating_div(delta.x.abs()),
        )
    };
    let (mut t_max_y, t_delta_y) = if step_y == 0 {
        (Fx::MAX, Fx::MAX)
    } else {
        let next_boundary = Fx::from_num(if step_y > 0 { start.y + 1 } else { start.y });
        (
            (next_boundary - a.y).abs().saturating_div(delta.y.abs()),
            Fx::ONE.saturating_div(delta.y.abs()),
        )
    };

    let mut tile = start;
    // Bounded by the tile-space extent of the segment, corner visits incl.
    let max_steps = (end.x - start.x).abs() + (end.y - start.y).abs() + 2;
    for _ in 0..max_steps {
        if t_max_x == t_max_y && step_x != 0 && step_y != 0 {
            // Exact corner crossing: check both tiles flanking the corner.
            let side_a = tile.offset(step_x, 0);
            let side_b = tile.offset(0, step_y);
            if side_a != end && !passable(side_a) {
                return true;
            }
            if side_b != end && !passable(side_b) {
                return true;
            }
            tile = tile.offset(step_x, step_y);
            t_max_x = t_max_x.saturating_add(t_delta_x);
            t_max_y = t_max_y.saturating_add(t_delta_y);
        } else if t_max_x < t_max_y {
            tile = tile.offset(step_x, 0);
            t_max_x = t_max_x.saturating_add(t_delta_x);
        } else {
            tile = tile.offset(0, step_y);
            t_max_y = t_max_y.saturating_add(t_delta_y);
        }
        if tile == end {
            return false;
        }
        if !passable(tile) {
            return true;
        }
    }
    false // numerically exhausted without hitting anything — clear
}

const STRAIGHT_COST: u32 = 10;
const DIAGONAL_COST: u32 = 14;

/// Reusable allocation storage for repeated A* queries on one thread.
///
/// Grid cells are validity-stamped with a per-query generation counter, so a
/// new query costs only the cells it actually touches — there is no
/// whole-grid clear between queries. A query that exhausts its reachable
/// component leaves that proof available until the next call; cheap
/// invalid/trivial/blocked-goal exits hide any prior proof without paying to
/// clear the retained capacity. Reusing allocations keeps
/// [`astar_with_scratch`] behavior identical to [`astar`].
#[derive(Default)]
pub struct AstarScratch {
    best_g: Vec<u32>,
    came_from: Vec<usize>,
    /// Cell validity stamps: a cell's `best_g`/`came_from` are meaningful only
    /// while its stamp equals `generation`. Stale cells read as untouched.
    stamp: Vec<u32>,
    /// Current query's generation. Bumped per grid search; on wrap-around the
    /// stamp grid is cleared once so stale stamps can never alias.
    generation: u32,
    open: BinaryHeap<Reverse<(u32, u32, usize)>>,
    last_width: i32,
    last_height: i32,
    last_exhausted: bool,
}

impl AstarScratch {
    /// Whether the previous query exhausted the complete reachable component
    /// instead of finding its goal or hitting the expansion cap.
    pub fn last_search_exhausted(&self) -> bool {
        self.last_exhausted
    }

    /// Whether the previous exhausted query proved `tile` belongs to the
    /// start's reachable component.
    ///
    /// This is useful after an exhausted search proves that several alternate
    /// goals are unreachable from the same origin under the same predicate.
    pub fn last_search_reached(&self, tile: TilePos) -> bool {
        if !self.last_exhausted
            || tile.x < 0
            || tile.y < 0
            || tile.x >= self.last_width
            || tile.y >= self.last_height
        {
            return false;
        }
        let index = (tile.y as usize) * (self.last_width as usize) + tile.x as usize;
        self.stamp
            .get(index)
            .is_some_and(|stamp| *stamp == self.generation)
    }

    /// Advances to a fresh generation whose stamps cannot collide with any
    /// stale cell, clearing the stamp grid only on counter wrap-around.
    fn advance_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.fill(0);
            self.generation = 1;
        }
    }

    /// Test-only: fast-forwards the generation counter to exercise the
    /// wrap-around clearing path without four billion queries.
    #[cfg(test)]
    fn force_generation(&mut self, generation: u32) {
        self.generation = generation;
    }
}

thread_local! {
    /// Per-thread scratch behind [`astar`], so every plain call reuses
    /// allocations. Safe for determinism: scratch reuse is behavior-identical
    /// to fresh storage (a differential test pins it), so results never depend
    /// on which thread ran the query or what it searched before.
    static SHARED_SCRATCH: std::cell::RefCell<AstarScratch> =
        std::cell::RefCell::new(AstarScratch::default());
}

/// Octile distance times 10 — exact (not just admissible) for 8-directional
/// grid movement with 10/14 costs.
fn heuristic(a: TilePos, b: TilePos) -> u32 {
    let dx = (a.x - b.x).unsigned_abs();
    let dy = (a.y - b.y).unsigned_abs();
    STRAIGHT_COST * dx.max(dy) + (DIAGONAL_COST - STRAIGHT_COST) * dx.min(dy)
}

/// Finds a shortest path from `start` to `goal`.
///
/// Returns the waypoints *after* `start`, ending with `goal`; an empty path
/// means `start == goal`. Returns `None` if the goal is unreachable, out of
/// bounds, impassable, or the search exceeds `max_expansions` (the caller's
/// guard against pathological queries on large maps).
///
/// `passable` is consulted for every tile except `start` — a unit is allowed
/// to path out of a tile it could not enter.
pub fn astar(
    width: i32,
    height: i32,
    start: TilePos,
    goal: TilePos,
    passable: impl FnMut(TilePos) -> bool,
    max_expansions: u32,
) -> Option<Vec<TilePos>> {
    SHARED_SCRATCH.with(|scratch| {
        astar_with_scratch(
            width,
            height,
            start,
            goal,
            passable,
            max_expansions,
            &mut scratch.borrow_mut(),
        )
    })
}

/// Finds the same shortest path as [`astar`] while reusing caller-owned
/// allocation storage across sequential queries.
pub fn astar_with_scratch(
    width: i32,
    height: i32,
    start: TilePos,
    goal: TilePos,
    mut passable: impl FnMut(TilePos) -> bool,
    max_expansions: u32,
    scratch: &mut AstarScratch,
) -> Option<Vec<TilePos>> {
    scratch.last_width = width.max(0);
    scratch.last_height = height.max(0);
    scratch.last_exhausted = false;
    scratch.open.clear();
    let in_bounds = |p: TilePos| p.x >= 0 && p.y >= 0 && p.x < width && p.y < height;
    if !in_bounds(start) || !in_bounds(goal) {
        return None;
    }
    let index = |p: TilePos| (p.y as usize) * (width as usize) + (p.x as usize);
    if start == goal {
        return Some(Vec::new());
    }
    if !passable(goal) {
        return None;
    }
    let cell_count = (width as usize).checked_mul(height as usize)?;
    // Cells are generation-stamped rather than cleared: only tiles this query
    // actually touches cost anything, so a short path on a huge map stays
    // cheap. Resize never initializes meaningfully — stale cells are dead by
    // stamp mismatch, including retained cells after a dimension change.
    scratch.best_g.resize(cell_count, 0);
    scratch.came_from.resize(cell_count, 0);
    scratch.stamp.resize(cell_count, 0);
    scratch.advance_generation();

    let AstarScratch {
        best_g,
        came_from,
        stamp,
        generation,
        open,
        last_exhausted,
        ..
    } = scratch;
    let generation = *generation;
    best_g[index(start)] = 0;
    stamp[index(start)] = generation;

    open.push(Reverse((
        heuristic(start, goal),
        heuristic(start, goal),
        index(start),
    )));

    let mut expansions = 0;
    while let Some(Reverse((f, _h, current_idx))) = open.pop() {
        let current = TilePos::new(
            (current_idx % (width as usize)) as i32,
            (current_idx / (width as usize)) as i32,
        );
        let g = best_g[current_idx];
        // Stale heap entry: a shorter route to this tile was already expanded.
        if f > g.saturating_add(heuristic(current, goal)) {
            continue;
        }
        if current == goal {
            let mut path = Vec::new();
            let mut idx = current_idx;
            while idx != index(start) {
                path.push(TilePos::new(
                    (idx % (width as usize)) as i32,
                    (idx / (width as usize)) as i32,
                ));
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
            let known = if stamp[next_idx] == generation {
                best_g[next_idx]
            } else {
                u32::MAX
            };
            if tentative < known {
                best_g[next_idx] = tentative;
                stamp[next_idx] = generation;
                came_from[next_idx] = current_idx;
                let h = heuristic(next, goal);
                open.push(Reverse((tentative + h, h, next_idx)));
            }
        };

        for (dx, dy) in CARDINALS {
            let next = current.offset(dx, dy);
            if in_bounds(next) && passable(next) {
                visit(next, STRAIGHT_COST, open);
            }
        }
        for (dx, dy) in DIAGONALS {
            let next = current.offset(dx, dy);
            // No corner cutting: both cardinal companions must be open.
            if in_bounds(next)
                && passable(next)
                && passable(current.offset(dx, 0))
                && passable(current.offset(0, dy))
            {
                visit(next, DIAGONAL_COST, open);
            }
        }
    }
    *last_exhausted = true;
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Grid;

    /// Builds a passability closure from ASCII rows ('#' blocked).
    fn arena(rows: &[&str]) -> (Grid<bool>, i32, i32) {
        let height = rows.len() as i32;
        let width = rows[0].len() as i32;
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
        let (large, width, height) =
            arena(&[".......", ".......", "...#...", ".......", "......."]);
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
        // type's ceiling — this exact geometry panicked in FixedI64
        // arithmetic before the walk went saturating. Adversarial in x,
        // in y, and in both at once, in both directions, over open and
        // blocked ground.
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
        // The fairness the game rests on: a 180°-rotated shot over
        // 180°-rotated terrain gets the identical verdict, so mirrored
        // seats never disagree about the same engagement. (Direction
        // symmetry along ONE segment is deliberately not promised — an
        // exact corner graze can round differently from the two ends;
        // see the doc comment.)
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
}
