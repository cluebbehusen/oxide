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

use crate::grid::{CARDINALS, DIAGONALS, TilePos};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

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
