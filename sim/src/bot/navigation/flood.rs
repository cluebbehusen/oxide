//! Cardinal connectivity and placement witnesses, plus bounded eight-way reach.

use crate::bot::query_work::QueryPurpose;
use chassis::grid::{CARDINALS, DIAGONALS, TilePos};
use std::{cell::RefCell, collections::VecDeque};

thread_local! {
    static REACH: RefCell<ReachScratch> = RefCell::default();
}

#[derive(Default)]
struct ReachScratch {
    seen: Vec<bool>,
    frontier: VecDeque<TilePos>,
}

fn area(width: i32, height: i32) -> usize {
    usize::try_from(width)
        .ok()
        .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
        .unwrap_or(0)
}

pub(in crate::bot) fn tile_index(width: i32, height: i32, tile: TilePos) -> Option<usize> {
    (tile.x >= 0 && tile.y >= 0 && tile.x < width && tile.y < height)
        .then(|| tile.y as usize * width as usize + tile.x as usize)
}

struct ReachLease(ReachScratch);
impl ReachLease {
    fn new() -> Self {
        Self(REACH.with_borrow_mut(std::mem::take))
    }
}
impl Drop for ReachLease {
    fn drop(&mut self) {
        REACH.with_borrow_mut(|scratch| *scratch = std::mem::take(&mut self.0));
    }
}

impl ReachScratch {
    fn search(
        &mut self,
        width: i32,
        height: i32,
        starts: impl IntoIterator<Item = TilePos>,
        enter: impl Fn(TilePos) -> bool,
        goal: impl Fn(TilePos) -> bool,
    ) -> bool {
        #[cfg(test)]
        super::work::record(|work| {
            work.searches += 1;
            work.components += 1;
        });
        self.seen.resize(area(width, height), false);
        self.seen.fill(false);
        self.frontier.clear();
        for start in starts {
            if let Some(index) = tile_index(width, height, start)
                && !self.seen[index]
            {
                self.seen[index] = true;
                self.frontier.push_back(start);
            }
        }
        while let Some(tile) = self.frontier.pop_front() {
            #[cfg(test)]
            super::work::record(|work| {
                work.expanded += 1;
            });

            if goal(tile) {
                return true;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let next = tile.offset(dx, dy);
                if let Some(index) = tile_index(width, height, next)
                    && !self.seen[index]
                    && enter(next)
                {
                    self.seen[index] = true;
                    self.frontier.push_back(next);
                }
            }
        }
        false
    }
}

/// Starts are admitted even when blocked, matching callers that escape footprints.
pub(in crate::bot) fn reaches_any(
    width: i32,
    height: i32,
    starts: impl IntoIterator<Item = TilePos>,
    enter: impl Fn(TilePos) -> bool,
    goal: impl Fn(TilePos) -> bool,
) -> bool {
    ReachLease::new()
        .0
        .search(width, height, starts, enter, goal)
}

pub(in crate::bot) fn component(
    width: i32,
    height: i32,
    start: TilePos,
    enter: impl Fn(TilePos) -> bool,
) -> Option<Vec<bool>> {
    tile_index(width, height, start)?;
    let mut lease = ReachLease::new();
    lease.0.search(width, height, [start], enter, |_| false);
    Some(lease.0.seen.clone())
}

/// Sparse membership for small regions such as a worker's initial danger area.
pub(in crate::bot) fn component_tiles(
    width: i32,
    height: i32,
    start: TilePos,
    enter: impl Fn(TilePos) -> bool,
) -> std::collections::BTreeSet<TilePos> {
    if tile_index(width, height, start).is_none() {
        return Default::default();
    }
    #[cfg(test)]
    super::work::record(|work| {
        work.searches += 1;
        work.components += 1;
    });
    let mut component = std::collections::BTreeSet::from([start]);
    let mut frontier = vec![start];
    while let Some(tile) = frontier.pop() {
        #[cfg(test)]
        super::work::record(|work| {
            work.expanded += 1;
        });

        for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
            let neighbor = tile.offset(dx, dy);
            if tile_index(width, height, neighbor).is_some()
                && enter(neighbor)
                && component.insert(neighbor)
            {
                frontier.push(neighbor);
            }
        }
    }
    component
}

pub(in crate::bot) fn labels(
    query_purpose: QueryPurpose,
    open: &[bool],
    map_size: (i32, i32),
) -> Vec<u32> {
    crate::bot::query_work::record(
        query_purpose,
        crate::bot::query_work::QueryOperation::Components,
        open.len(),
    );
    let index = |tile: TilePos| (tile.y * map_size.0 + tile.x) as usize;
    #[cfg(test)]
    super::work::record(|work| {
        work.searches += 1;
        work.components += 1;
    });
    let mut labels = vec![0; open.len()];
    let mut next_label = 1u32;
    for start_index in 0..open.len() {
        if !open[start_index] || labels[start_index] != 0 {
            continue;
        }
        let start = TilePos::new(
            start_index as i32 % map_size.0,
            start_index as i32 / map_size.0,
        );
        labels[start_index] = next_label;
        let mut frontier = std::collections::VecDeque::from([start]);
        while let Some(tile) = frontier.pop_front() {
            #[cfg(test)]
            super::work::record(|work| {
                work.expanded += 1;
            });

            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let next = tile.offset(dx, dy);
                if next.x < 0 || next.y < 0 || next.x >= map_size.0 || next.y >= map_size.1 {
                    continue;
                }
                let next_index = index(next);
                if open[next_index] && labels[next_index] == 0 {
                    labels[next_index] = next_label;
                    frontier.push_back(next);
                }
            }
        }
        next_label = next_label
            .checked_add(1)
            .expect("a ground map cannot contain u32::MAX components");
    }
    labels
}

fn monotone_cardinal_path(
    open: &[bool],
    width: i32,
    start: TilePos,
    goal: TilePos,
) -> Option<Vec<TilePos>> {
    let index = |tile: TilePos| (tile.y * width + tile.x) as usize;
    let mut visited = vec![false; open.len()];
    let mut route = vec![start];
    visited[index(start)] = true;
    // A successful probe follows the BFS tie order at the Manhattan lower bound.
    // Bound failed probes so walls add little work before the complete search.
    for _ in 0..512 {
        let current = *route.last()?;
        #[cfg(test)]
        super::work::record(|work| work.expanded += 1);
        if current == goal {
            return Some(route);
        }
        let next = [
            (current.x != goal.x).then(|| current.offset((goal.x - current.x).signum(), 0)),
            (current.y != goal.y).then(|| current.offset(0, (goal.y - current.y).signum())),
        ]
        .into_iter()
        .flatten()
        .find(|tile| open[index(*tile)] && !visited[index(*tile)]);
        if let Some(next) = next {
            visited[index(next)] = true;
            route.push(next);
        } else {
            route.pop();
        }
    }
    None
}

pub(in crate::bot) fn cardinal_path(
    open_tiles: &[bool],
    map_size: (i32, i32),
    start: TilePos,
    goal: TilePos,
) -> Option<Vec<TilePos>> {
    let in_bounds =
        |tile: TilePos| tile.x >= 0 && tile.y >= 0 && tile.x < map_size.0 && tile.y < map_size.1;
    if !in_bounds(start) || !in_bounds(goal) {
        return None;
    }
    let index = |tile: TilePos| (tile.y * map_size.0 + tile.x) as usize;
    if !open_tiles[index(start)] || !open_tiles[index(goal)] {
        return None;
    }
    let tile = |index: usize| TilePos::new(index as i32 % map_size.0, index as i32 / map_size.0);
    let start_index = index(start);
    let goal_index = index(goal);
    #[cfg(test)]
    super::work::record(|work| {
        work.searches += 1;
    });
    if start.manhattan(goal) >= 16
        && let Some(route) = monotone_cardinal_path(open_tiles, map_size.0, start, goal)
    {
        #[cfg(test)]
        super::work::record(|work| work.paths += 1);
        return Some(route);
    }
    let mut parent = vec![usize::MAX; open_tiles.len()];
    let mut open = std::collections::VecDeque::from([start]);
    parent[start_index] = start_index;
    while let Some(current) = open.pop_front() {
        #[cfg(test)]
        super::work::record(|work| {
            work.expanded += 1;
        });

        if current == goal {
            break;
        }
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let next = current.offset(dx, dy);
            if !in_bounds(next) || !open_tiles[index(next)] {
                continue;
            }
            let next_index = index(next);
            if parent[next_index] != usize::MAX {
                continue;
            }
            parent[next_index] = index(current);
            open.push_back(next);
        }
    }
    if parent[goal_index] == usize::MAX {
        return None;
    }
    let mut route = Vec::new();
    let mut cursor = goal_index;
    loop {
        route.push(tile(cursor));
        if cursor == start_index {
            break;
        }
        cursor = parent[cursor];
    }
    #[cfg(test)]
    super::work::record(|work| {
        work.paths += 1;
    });
    route.reverse();
    Some(route)
}

pub(in crate::bot) fn within_steps(
    width: i32,
    height: i32,
    starts: &[TilePos],
    passable: impl Fn(TilePos) -> bool,
    maximum_steps: usize,
) -> Vec<bool> {
    let area = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .unwrap_or(0);
    #[cfg(test)]
    super::work::record(|work| {
        work.searches += 1;
    });
    let mut distance = vec![u8::MAX; area];
    let mut open = VecDeque::new();
    for start in starts.iter().copied() {
        let Some(index) = tile_index(width, height, start) else {
            continue;
        };
        if distance[index] == u8::MAX {
            distance[index] = 0;
            open.push_back(start);
        }
    }

    while let Some(current) = open.pop_front() {
        #[cfg(test)]
        super::work::record(|work| {
            work.expanded += 1;
        });

        let current_index = tile_index(width, height, current)
            .expect("the local route flood contains only in-bounds tiles");
        let steps = usize::from(distance[current_index]);
        if steps >= maximum_steps {
            continue;
        }
        let mut cardinal_open = [false; 4];
        for (dx, dy) in CARDINALS {
            let next = current.offset(dx, dy);
            if passable(next) {
                let slot = if dy == 0 {
                    usize::from(dx < 0)
                } else {
                    2 + usize::from(dy < 0)
                };
                cardinal_open[slot] = true;
                enqueue_local_route_tile(&mut distance, &mut open, width, height, next, steps + 1);
            }
        }
        for (dx, dy) in DIAGONALS {
            if cardinal_open[usize::from(dx < 0)] && cardinal_open[2 + usize::from(dy < 0)] {
                let next = current.offset(dx, dy);
                if passable(next) {
                    enqueue_local_route_tile(
                        &mut distance,
                        &mut open,
                        width,
                        height,
                        next,
                        steps + 1,
                    );
                }
            }
        }
    }

    distance
        .into_iter()
        .map(|steps| usize::from(steps) <= maximum_steps)
        .collect()
}

fn enqueue_local_route_tile(
    distance: &mut [u8],
    open: &mut VecDeque<TilePos>,
    width: i32,
    height: i32,
    tile: TilePos,
    steps: usize,
) {
    let Some(index) = tile_index(width, height, tile) else {
        return;
    };
    if usize::from(distance[index]) <= steps {
        return;
    }
    distance[index] = u8::try_from(steps).unwrap_or(u8::MAX);
    open.push_back(tile);
}

#[cfg(test)]
mod tests;
