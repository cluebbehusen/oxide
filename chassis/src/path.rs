//! Deterministic A* over a tile grid.
//!
//! Eight-directional movement with no corner cutting: a diagonal step is
//! legal only when both adjacent cardinal tiles are passable, so a unit can
//! never squeeze between two blockers. Costs are integers (10 straight,
//! 14 diagonal) and the octile heuristic is exact for this movement model,
//! so returned paths are optimal.
//!
//! Determinism: the open set orders by `(f, h, query-oriented tile rank)`.
//! The rank is unique and reverses with the query under a map half-turn, so
//! equal-cost routes stay canonical without favoring one absolute corner.

use crate::fx::{Fx, Vec2Fx};
use crate::grid::{CARDINALS, DIAGONALS, TilePos, as_index};
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
/// Direction symmetry is not guaranteed: a segment that grazes a tile
/// corner exactly can round to opposite sides of it depending on which
/// end the walk starts from (e.g. a 1/7 slope, whose reciprocal is inexact
/// in binary). Seat fairness relies on mirror symmetry instead: a
/// 180°-rotated segment over 180°-rotated terrain gets the identical
/// verdict, because every quantity here is sign-symmetric.
pub fn line_blocked(a: Vec2Fx, b: Vec2Fx, mut passable: impl FnMut(TilePos) -> bool) -> bool {
    let start = TilePos::containing(a);
    let end = TilePos::containing(b);
    if start == end {
        return false;
    }
    let delta = b - a;
    let step = |axis: Fx| -> i32 {
        match axis.cmp(&Fx::ZERO) {
            std::cmp::Ordering::Greater => 1,
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
        }
    };
    let step_x = step(delta.x);
    let step_y = step(delta.y);
    // Parametric distance (0..1 along the segment) to the next x/y tile
    // boundary, and per-tile increments, in saturating Q32.32. A delta
    // component can be one ulp, and 1/ulp = 2^32 exceeds the type's range.
    // A t_max of MAX means "this axis crosses no more boundaries within the
    // segment", matching the zero-delta arm; non-degenerate segments get
    // the same values as plain arithmetic. Both operands are non-negative,
    // so saturation lands on MAX, never on MIN (whose abs would panic).
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
    false // step budget exhausted without hitting a blocker
}

/// Whether a body of `radius` sweeping the segment from `a` to `b` crosses a
/// tile that fails `passable`: the center line plus the two parallel edge
/// lines offset by `radius`. The center line's endpoint tiles are never
/// tested, like [`line_blocked`]: the body may be leaving ground it could not
/// enter, and the caller checks the destination tile itself. An edge line's
/// start tile is tested when it lies outside the body's own tile, so a hull
/// that already overlaps a blocked tile cannot sweep along or past it. An
/// edge that only touches a tile boundary does not enter the tile beyond it.
/// A zero-length segment is never blocked.
///
/// The offset pair is exactly sign-symmetric and touching edges are pulled
/// inside the hull, so the verdict keeps [`line_blocked`]'s mirror fairness
/// under a map half-turn.
pub fn swept_line_blocked(
    a: Vec2Fx,
    b: Vec2Fx,
    radius: Fx,
    mut passable: impl FnMut(TilePos) -> bool,
) -> bool {
    let delta = b - a;
    let length = delta.length();
    if length == Fx::ZERO {
        return false;
    }
    if line_blocked(a, b, &mut passable) {
        return true;
    }
    if radius <= Fx::ZERO {
        return false;
    }
    let side = Vec2Fx::new(-delta.y, delta.x) * (radius / length);
    let own = TilePos::containing(a);
    [side, -side].into_iter().any(|side| {
        let (from, to) = (inset(a + side, a), inset(b + side, b));
        let start = TilePos::containing(from);
        (start != own && !passable(start)) || line_blocked(from, to, &mut passable)
    })
}

/// `point` moved one ulp toward `center` on each axis where it lies exactly
/// on a tile boundary. Flooring an exact boundary would count a touching edge
/// as inside the tile on one side of a body and outside it on the other, so
/// mirrored seats would sweep differently.
fn inset(point: Vec2Fx, center: Vec2Fx) -> Vec2Fx {
    let pull = |edge: Fx, center: Fx| {
        if edge.frac() != Fx::ZERO || edge == center {
            edge
        } else if edge > center {
            edge - Fx::DELTA
        } else {
            edge + Fx::DELTA
        }
    };
    Vec2Fx::new(pull(point.x, center.x), pull(point.y, center.y))
}

const STRAIGHT_COST: u32 = 10;
const DIAGONAL_COST: u32 = 14;

/// Reusable allocation storage for repeated A* queries on one thread.
///
/// Cells are stamped with a per-query generation, so a query costs only the
/// cells it touches; there is no whole-grid clear between queries. A query
/// that exhausts its reachable component keeps that proof until the next
/// call; early exits (invalid endpoints, trivial path, blocked goal) hide any
/// prior proof. [`astar_with_scratch`] returns the same results as [`astar`].
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
    open: DialQueue,
    last_width: i32,
    last_height: i32,
    last_exhausted: bool,
    last_expansions: u32,
}

/// A Dial (bucket) priority queue specialized to this A*'s keys, popping in
/// exact `(f, h, query-oriented rank)` order.
///
/// Two facts make sixteen buckets sufficient, both properties of the
/// octile 10/14 cost model rather than tuning:
///
/// - Pops are f-monotone (the heuristic is consistent), and a relaxed
///   neighbor's key exceeds its parent's by at most two edge costs, so
///   every live key sits in a 28-wide window above the cursor.
/// - Every g sums 10s and 14s and every h is `10*max + 4*min`, so all
///   keys are even: the window holds at most 15 distinct key values,
///   and `(f / 2) % 16` addresses each unambiguously.
///
/// Each bucket is a small binary heap over `(h, rank, index)`, preserving the
/// within-f tie-break exactly while carrying the real index as payload;
/// `came_from` depends on expansion order among equal-f nodes, so approximate
/// orders are not an option.
#[derive(Default)]
struct DialQueue {
    buckets: [BinaryHeap<Reverse<(u32, u32, u32)>>; 16],
    cursor: u32,
    len: usize,
}

impl DialQueue {
    fn clear(&mut self) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        self.len = 0;
    }

    /// Prepares for a query whose first key is `f` (the start's h).
    fn reset(&mut self, f: u32) {
        self.clear();
        self.cursor = f;
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "grid cell indices and ranks fit u32, as asserted below"
    )]
    fn push(&mut self, f: u32, h: u32, rank: usize, index: usize) {
        debug_assert!(f.is_multiple_of(2), "octile keys are even by construction");
        debug_assert!(
            self.len == 0 || (f >= self.cursor && f - self.cursor <= 28),
            "a live key left the dial window: f={f} cursor={}",
            self.cursor
        );
        debug_assert!(u32::try_from(rank).is_ok(), "cell rank exceeds u32");
        debug_assert!(u32::try_from(index).is_ok(), "cell index exceeds u32");
        if self.len == 0 {
            // An empty dial may be handed a key below the stale cursor
            // (a fresh query, or every earlier entry proved stale);
            // the cursor simply re-anchors on it.
            self.cursor = self.cursor.min(f);
        }
        self.buckets[((f >> 1) & 15) as usize].push(Reverse((h, rank as u32, index as u32)));
        self.len += 1;
    }

    fn pop(&mut self) -> Option<(u32, u32, usize)> {
        if self.len == 0 {
            return None;
        }
        for _ in 0..16 {
            let slot = ((self.cursor >> 1) & 15) as usize;
            if let Some(Reverse((h, _rank, index))) = self.buckets[slot].pop() {
                self.len -= 1;
                return Some((self.cursor, h, index as usize));
            }
            self.cursor += 2;
        }
        unreachable!("a live dial key must sit within the 28-wide window");
    }
}

impl AstarScratch {
    /// Nodes expanded by the last query, including the node that exceeded its cap.
    /// Invalid endpoints, blocked goals, and trivial paths expand no nodes.
    pub fn last_expansions(&self) -> u32 {
        self.last_expansions
    }

    /// Hide reachability evidence without releasing retained search buffers.
    /// Use when changing passability contexts or supplying a cached success
    /// without running another search.
    pub fn clear_search_evidence(&mut self) {
        self.last_exhausted = false;
    }

    /// Whether the previous query exhausted the complete reachable component
    /// instead of finding its goal or hitting the expansion cap.
    pub fn last_search_exhausted(&self) -> bool {
        self.last_exhausted
    }

    /// Whether the previous exhausted query proved `tile` belongs to the
    /// start's reachable component, so one exhausted search can answer for
    /// several alternate goals from the same origin and predicate.
    pub fn last_search_reached(&self, tile: TilePos) -> bool {
        if !self.last_exhausted
            || tile.x < 0
            || tile.y < 0
            || tile.x >= self.last_width
            || tile.y >= self.last_height
        {
            return false;
        }
        let index = tile.row_major(self.last_width);
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
    /// Per-thread scratch behind [`astar`]. Scratch reuse is behavior-identical
    /// to fresh storage, so results never depend on which thread ran the query
    /// or what it searched before.
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
    passable: impl FnMut(TilePos) -> bool,
    max_expansions: u32,
    scratch: &mut AstarScratch,
) -> Option<Vec<TilePos>> {
    astar_inner::<false>(
        (width, height),
        start,
        goal,
        passable,
        max_expansions,
        scratch,
        &[],
    )
}

/// Finds the canonical path while pruning detours with exact reverse distances.
///
/// `distances` must contain row-major shortest costs to `goal` for this exact
/// passability graph, using the same 10/14 costs and no corner cutting as [`astar`].
/// Unreachable cells have cost `u32::MAX`. Queue ordering is unchanged, so tied
/// paths retain their canonical shape. Pruning is disabled when the expansion
/// cap could affect the result, the field dimensions differ, or the start is blocked.
pub fn astar_with_distances(
    dimensions: (i32, i32),
    start: TilePos,
    goal: TilePos,
    mut passable: impl FnMut(TilePos) -> bool,
    max_expansions: u32,
    scratch: &mut AstarScratch,
    distances: &[u32],
) -> Option<Vec<TilePos>> {
    let (width, height) = dimensions;
    let cells = usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)));
    let bounded = start.x >= 0 && start.y >= 0 && start.x < width && start.y < height;
    if cells.is_some_and(|n| n == distances.len() && n <= max_expansions as usize)
        && bounded
        && passable(start)
    {
        astar_inner::<true>(
            dimensions,
            start,
            goal,
            passable,
            max_expansions,
            scratch,
            distances,
        )
    } else {
        astar_inner::<false>(
            dimensions,
            start,
            goal,
            passable,
            max_expansions,
            scratch,
            &[],
        )
    }
}

/// Labels every tile with its 4-connected component under `open`.
///
/// The result is row-major, `width * height` long: 0 marks a closed tile and
/// components are numbered from 1 in row-major order of their first tile.
/// Because [`astar`] never cuts a corner, two open tiles share a label exactly
/// when an A* search from one can reach the other. A search that starts on a
/// closed tile reaches the components of its open cardinal neighbours only.
pub fn cardinal_components(
    width: i32,
    height: i32,
    mut open: impl FnMut(TilePos) -> bool,
) -> Vec<u32> {
    let (Ok(w), Ok(h)) = (usize::try_from(width), usize::try_from(height)) else {
        return Vec::new();
    };
    let mut labels = vec![0u32; w * h];
    let mut closed = vec![false; w * h];
    for (index, closed) in closed.iter_mut().enumerate() {
        *closed = !open(TilePos::from_row_major(index, width));
    }
    let mut next_label = 0u32;
    let mut frontier = Vec::new();
    for start in 0..labels.len() {
        if closed[start] || labels[start] != 0 {
            continue;
        }
        next_label += 1;
        labels[start] = next_label;
        frontier.push(start);
        while let Some(index) = frontier.pop() {
            let tile = TilePos::from_row_major(index, width);
            for (dx, dy) in CARDINALS {
                let next = tile.offset(dx, dy);
                if next.x < 0 || next.y < 0 || next.x >= width || next.y >= height {
                    continue;
                }
                let next_index = next.row_major(width);
                if !closed[next_index] && labels[next_index] == 0 {
                    labels[next_index] = next_label;
                    frontier.push(next_index);
                }
            }
        }
    }
    labels
}

fn astar_inner<const PRUNE: bool>(
    (width, height): (i32, i32),
    start: TilePos,
    goal: TilePos,
    mut passable: impl FnMut(TilePos) -> bool,
    max_expansions: u32,
    scratch: &mut AstarScratch,
    distances: &[u32],
) -> Option<Vec<TilePos>> {
    scratch.last_width = width.max(0);
    scratch.last_height = height.max(0);
    scratch.last_exhausted = false;
    scratch.last_expansions = 0;
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
    let cell_count = as_index(width).checked_mul(as_index(height))?;
    // A half-turn maps row-major index `i` to `cell_count - 1 - i` and
    // reverses the start/goal lexicographic order. Orienting the tie rank by
    // that order therefore gives corresponding cells identical ranks in the
    // rotated query, without changing costs or reachability.
    let reverse_ties = (goal.y, goal.x) < (start.y, start.x);
    let tie_rank = |cell_index: usize| {
        if reverse_ties {
            cell_count - 1 - cell_index
        } else {
            cell_index
        }
    };
    // Resized cells need no initialization: stale cells, including retained
    // cells after a dimension change, are dead by stamp mismatch.
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
        last_expansions,
        ..
    } = scratch;
    let generation = *generation;
    best_g[index(start)] = 0;
    stamp[index(start)] = generation;

    open.reset(heuristic(start, goal));
    open.push(
        heuristic(start, goal),
        heuristic(start, goal),
        tie_rank(index(start)),
        index(start),
    );

    while let Some((f, _h, current_idx)) = open.pop() {
        let current = TilePos::from_row_major(current_idx, width);
        let g = best_g[current_idx];
        // Stale heap entry: a shorter route to this tile was already expanded.
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
        *last_expansions += 1;
        if *last_expansions > max_expansions {
            return None;
        }

        let mut visit = |next: TilePos, step_cost: u32, open: &mut DialQueue| {
            let next_idx = index(next);
            let tentative = g + step_cost;
            if PRUNE && tentative.saturating_add(distances[next_idx]) > distances[index(start)] {
                return;
            }
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
                open.push(tentative + h, h, tie_rank(next_idx), next_idx);
            }
        };

        // The cardinal verdicts double as the diagonals' corner-cut
        // companions, so record them once instead of re-asking the
        // predicate. Slots derive from the offset's sign, not CARDINALS'
        // order, so reordering that constant cannot flip the rule.
        let mut cardinal_open = [false; 4]; // [+x, -x, +y, -y]
        for (dx, dy) in CARDINALS {
            let next = current.offset(dx, dy);
            if in_bounds(next) && passable(next) {
                let slot = if dy == 0 {
                    usize::from(dx < 0)
                } else {
                    2 + usize::from(dy < 0)
                };
                cardinal_open[slot] = true;
                visit(next, STRAIGHT_COST, open);
            }
        }
        for (dx, dy) in DIAGONALS {
            // No corner cutting: both cardinal companions must be
            // open. An out-of-bounds companion shares an axis with an
            // out-of-bounds `next`, so its recorded `false` agrees
            // with what a direct probe would have said.
            if cardinal_open[usize::from(dx < 0)] && cardinal_open[2 + usize::from(dy < 0)] {
                let next = current.offset(dx, dy);
                if in_bounds(next) && passable(next) {
                    visit(next, DIAGONAL_COST, open);
                }
            }
        }
    }
    *last_exhausted = true;
    None
}

#[cfg(test)]
mod tests;
