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
    // boundary, and per-tile increments. All exact Q32.32 arithmetic.
    let (mut t_max_x, t_delta_x) = if step_x == 0 {
        (Fx::MAX, Fx::MAX)
    } else {
        let next_boundary = Fx::from_num(if step_x > 0 { start.x + 1 } else { start.x });
        ((next_boundary - a.x) / delta.x, (Fx::ONE / delta.x).abs())
    };
    let (mut t_max_y, t_delta_y) = if step_y == 0 {
        (Fx::MAX, Fx::MAX)
    } else {
        let next_boundary = Fx::from_num(if step_y > 0 { start.y + 1 } else { start.y });
        ((next_boundary - a.y) / delta.y, (Fx::ONE / delta.y).abs())
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
            t_max_x += t_delta_x;
            t_max_y += t_delta_y;
        } else if t_max_x < t_max_y {
            tile = tile.offset(step_x, 0);
            t_max_x += t_delta_x;
        } else {
            tile = tile.offset(0, step_y);
            t_max_y += t_delta_y;
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
    mut passable: impl FnMut(TilePos) -> bool,
    max_expansions: u32,
) -> Option<Vec<TilePos>> {
    let in_bounds = |p: TilePos| p.x >= 0 && p.y >= 0 && p.x < width && p.y < height;
    if !in_bounds(start) || !in_bounds(goal) {
        return None;
    }
    if start == goal {
        return Some(Vec::new());
    }
    if !passable(goal) {
        return None;
    }

    let index = |p: TilePos| (p.y as usize) * (width as usize) + (p.x as usize);
    let cell_count = (width as usize) * (height as usize);
    let mut best_g = vec![u32::MAX; cell_count];
    let mut came_from = vec![usize::MAX; cell_count];

    let mut open = BinaryHeap::new();
    best_g[index(start)] = 0;
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
            if tentative < best_g[next_idx] {
                best_g[next_idx] = tentative;
                came_from[next_idx] = current_idx;
                let h = heuristic(next, goal);
                open.push(Reverse((tentative + h, h, next_idx)));
            }
        };

        for (dx, dy) in CARDINALS {
            let next = current.offset(dx, dy);
            if in_bounds(next) && passable(next) {
                visit(next, STRAIGHT_COST, &mut open);
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
                visit(next, DIAGONAL_COST, &mut open);
            }
        }
    }
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
