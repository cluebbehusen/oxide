//! Navigation over immutable player knowledge and public terrain.
//!
//! Costs do not certify a command's snapped goal, safety, or canonical path.

pub(super) mod approaches;
pub(super) mod areas;
pub(super) mod commands;
mod components;
pub(super) mod distance_work;
pub(super) mod egress;
pub(super) mod flood;
pub(super) mod inputs;
pub(super) mod paths;
pub(super) mod public_fields;
pub(super) mod regions;
mod safety;
pub(super) mod search;
pub(super) mod service;

#[cfg(test)]
pub(super) mod work;

use chassis::{
    grid::{CARDINALS, DIAGONALS, TilePos},
    path::{AstarScratch, astar_with_scratch},
};
use std::{
    cell::RefCell,
    cmp::{Ordering, Reverse},
    collections::{BTreeMap, BinaryHeap},
    mem::size_of,
};

const CACHE_BYTES: usize = 1024 * 1024;
const ENTRY_ALLOWANCE: usize = 192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct BlockedRect {
    pub anchor: TilePos,
    pub size: (i32, i32),
}

impl BlockedRect {
    pub(in crate::bot) fn contains(self, tile: TilePos) -> bool {
        (0..i64::from(self.size.0)).contains(&(i64::from(tile.x) - i64::from(self.anchor.x)))
            && (0..i64::from(self.size.1)).contains(&(i64::from(tile.y) - i64::from(self.anchor.y)))
    }
    pub fn affects_path(self, path: &[TilePos]) -> bool {
        path.iter().any(|tile| self.contains(*tile))
            || path.windows(2).any(|pair| {
                pair[0].x != pair[1].x
                    && pair[0].y != pair[1].y
                    && (self.contains(TilePos::new(pair[1].x, pair[0].y))
                        || self.contains(TilePos::new(pair[0].x, pair[1].y)))
            })
    }
}

/// A complete passability surface for one movement domain and knowledge policy.
#[derive(Clone, Copy)]
pub(super) struct KnownGrid<'a> {
    width: i32,
    height: i32,
    blocked: &'a [bool],
}

impl<'a> KnownGrid<'a> {
    pub(in crate::bot) fn dimensions(self) -> (i32, i32) {
        (self.width, self.height)
    }
    pub(in crate::bot) fn blocked(self) -> &'a [bool] {
        self.blocked
    }

    pub fn new(width: i32, height: i32, blocked: &'a [bool]) -> Option<Self> {
        let area = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        (width > 0 && height > 0 && blocked.len() == area).then_some(Self {
            width,
            height,
            blocked,
        })
    }

    fn index(self, tile: TilePos) -> Option<usize> {
        (tile.x >= 0 && tile.y >= 0 && tile.x < self.width && tile.y < self.height)
            .then(|| tile.y as usize * self.width as usize + tile.x as usize)
    }

    fn tile(self, index: usize) -> TilePos {
        TilePos::new(
            (index % self.width as usize) as i32,
            (index / self.width as usize) as i32,
        )
    }

    fn open(self, tile: TilePos, overlay: Option<BlockedRect>) -> bool {
        self.index(tile).is_some_and(|index| !self.blocked[index])
            && overlay.is_none_or(|rectangle| !rectangle.contains(tile))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CostResult {
    Exact(u32),
    /// Cheapest successful bounded endpoint search; another pair hit its limit.
    Bounded(u32),
    Unreachable,
    SearchLimit,
}

impl CostResult {
    /// Matches the existing bounded per-endpoint routing contract.
    pub fn available_cost(self) -> Option<u32> {
        match self {
            Self::Exact(cost) | Self::Bounded(cost) => Some(cost),
            Self::Unreachable | Self::SearchLimit => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Query {
    overlay: Option<BlockedRect>,
    starts: Box<[TilePos]>,
    goals: Box<[TilePos]>,
}

impl Query {
    fn new(
        grid: KnownGrid<'_>,
        overlay: Option<BlockedRect>,
        starts: &[TilePos],
        goals: &[TilePos],
    ) -> Self {
        let normalize = |tiles: &[TilePos]| {
            let mut tiles: Vec<_> = tiles
                .iter()
                .copied()
                .filter(|tile| grid.index(*tile).is_some())
                .collect();
            tiles.sort_unstable_by_key(|tile| (tile.y, tile.x));
            tiles.dedup();
            tiles.into_boxed_slice()
        };
        Self {
            overlay,
            starts: normalize(starts),
            goals: normalize(goals),
        }
    }

    fn bytes(&self) -> usize {
        ENTRY_ALLOWANCE + (self.starts.len() + self.goals.len()) * size_of::<TilePos>()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Generation {
    width: i32,
    height: i32,
    blocked: Box<[bool]>,
    answers: BTreeMap<Query, CostResult>,
    bytes: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CostQueries {
    generation: Option<Generation>,
    #[cfg(test)]
    work: Work,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Work {
    pub set_searches: usize,
    pub pair_searches: usize,
    pub expanded: usize,
    pub hits: usize,
    pub generations: usize,
}

#[derive(Default)]
struct Scratch {
    distance: Vec<u32>,
    targets: Vec<bool>,
    frontier: BinaryHeap<Reverse<(u32, usize)>>,
    astar: AstarScratch,
}

thread_local! {
    static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::default());
}

impl CostQueries {
    pub fn between_sets(
        &mut self,
        query_purpose: super::query_work::QueryPurpose,
        grid: KnownGrid<'_>,
        overlay: Option<BlockedRect>,
        starts: &[TilePos],
        goals: &[TilePos],
    ) -> CostResult {
        super::query_work::record(
            query_purpose,
            super::query_work::QueryOperation::CostRequest,
            1,
        );
        let query = Query::new(grid, overlay, starts, goals);
        self.prepare(grid);
        if let Some(answer) = self
            .generation
            .as_ref()
            .and_then(|generation| generation.answers.get(&query))
        {
            super::query_work::record(
                query_purpose,
                super::query_work::QueryOperation::CacheHit,
                1,
            );
            #[cfg(test)]
            {
                self.work.hits += 1;
                work::record(|work| work.hits += 1);
            }
            return *answer;
        }
        let answer = SCRATCH.with(|scratch| {
            self.search(
                query_purpose,
                grid,
                &query,
                crate::stats::PATH_EXPANSION_CAP,
                &mut scratch.borrow_mut(),
            )
        });
        if let Some(generation) = self.generation.as_mut() {
            let bytes = query.bytes();
            if bytes <= CACHE_BYTES.saturating_sub(generation.blocked.len()) {
                while generation.bytes + bytes > CACHE_BYTES {
                    let (old, _) = generation
                        .answers
                        .pop_first()
                        .expect("retained answer budget");
                    generation.bytes -= old.bytes();
                }
                generation.bytes += bytes;
                generation.answers.insert(query, answer);
            }
        }
        answer
    }

    fn prepare(&mut self, grid: KnownGrid<'_>) {
        if grid.blocked.len() > CACHE_BYTES {
            self.generation = None;
        } else if self.generation.as_ref().is_none_or(|generation| {
            generation.width != grid.width
                || generation.height != grid.height
                || generation.blocked.as_ref() != grid.blocked
        }) {
            self.generation = Some(Generation {
                width: grid.width,
                height: grid.height,
                blocked: grid.blocked.into(),
                answers: BTreeMap::new(),
                bytes: grid.blocked.len(),
            });
            #[cfg(test)]
            {
                self.work.generations += 1;
                work::record(|work| work.generations += 1);
            }
        }
    }

    fn search(
        &mut self,
        query_purpose: super::query_work::QueryPurpose,
        grid: KnownGrid<'_>,
        query: &Query,
        limit: u32,
        scratch: &mut Scratch,
    ) -> CostResult {
        if query.starts.is_empty() || query.goals.is_empty() {
            return CostResult::Unreachable;
        }
        // A set flood may exhaust a budget that a goal-directed pair would meet.
        // Use it only when the entire graph fits the per-pair expansion limit.
        if grid.blocked.len() > limit as usize {
            return self.pair_cost(query_purpose, grid, query, limit, &mut scratch.astar);
        }
        #[cfg(test)]
        {
            self.work.set_searches += 1;
            work::record(|work| work.searches += 1);
        }
        scratch.distance.resize(grid.blocked.len(), u32::MAX);
        scratch.distance.fill(u32::MAX);
        scratch.targets.resize(grid.blocked.len(), false);
        scratch.targets.fill(false);
        scratch.frontier.clear();
        for goal in &query.goals {
            scratch.targets[grid.index(*goal).expect("normalized goal")] = true;
        }
        for start in &query.starts {
            let index = grid.index(*start).expect("normalized start");
            scratch.distance[index] = 0;
            scratch.frontier.push(Reverse((0, index)));
        }
        let mut expanded = 0;
        while let Some(Reverse((cost, index))) = scratch.frontier.pop() {
            if scratch.distance[index] != cost {
                continue;
            }
            if scratch.targets[index] {
                super::query_work::record(
                    query_purpose,
                    super::query_work::QueryOperation::CostSearch,
                    expanded,
                );
                return CostResult::Exact(cost);
            }
            expanded += 1;
            #[cfg(test)]
            {
                self.work.expanded += 1;
                work::record(|work| work.expanded += 1);
            }
            let tile = grid.tile(index);
            for (dx, dy) in CARDINALS.into_iter().chain(DIAGONALS) {
                let next = tile.offset(dx, dy);
                if !grid.open(next, query.overlay)
                    || (dx != 0
                        && dy != 0
                        && (!grid.open(tile.offset(dx, 0), query.overlay)
                            || !grid.open(tile.offset(0, dy), query.overlay)))
                {
                    continue;
                }
                let next_index = grid.index(next).expect("an open tile is in bounds");
                let next_cost = cost + if dx == 0 || dy == 0 { 10 } else { 14 };
                if next_cost < scratch.distance[next_index] {
                    scratch.distance[next_index] = next_cost;
                    scratch.frontier.push(Reverse((next_cost, next_index)));
                }
            }
        }
        super::query_work::record(
            query_purpose,
            super::query_work::QueryOperation::CostSearch,
            expanded,
        );
        CostResult::Unreachable
    }

    fn pair_cost(
        &mut self,
        query_purpose: super::query_work::QueryPurpose,
        grid: KnownGrid<'_>,
        query: &Query,
        limit: u32,
        scratch: &mut AstarScratch,
    ) -> CostResult {
        let mut pairs: Vec<_> = query
            .starts
            .iter()
            .flat_map(|start| query.goals.iter().map(move |goal| (*start, *goal)))
            .collect();
        pairs.sort_unstable_by_key(|(start, goal)| {
            (octile(*start, *goal), start.y, start.x, goal.y, goal.x)
        });
        let mut best = None;
        let mut limited = false;
        for (start, goal) in pairs {
            if best.is_some_and(|cost| octile(start, goal) > cost) {
                break;
            }
            if start != goal && !grid.open(goal, query.overlay) {
                continue;
            }
            #[cfg(test)]
            {
                self.work.pair_searches += 1;
            }
            let path = astar_with_scratch(
                grid.width,
                grid.height,
                start,
                goal,
                |tile| grid.open(tile, query.overlay),
                limit,
                scratch,
            );
            super::query_work::record(
                query_purpose,
                super::query_work::QueryOperation::PathSearch,
                scratch.last_expansions() as usize,
            );
            #[cfg(test)]
            work::record(|work| {
                work.searches += 1;
                work.expanded += scratch.last_expansions() as usize;
                work.paths += usize::from(path.is_some());
            });
            match path {
                Some(path) => {
                    let mut previous = start;
                    let cost = path
                        .into_iter()
                        .map(|tile| {
                            let cost = octile(previous, tile);
                            previous = tile;
                            cost
                        })
                        .sum::<u32>();
                    best = Some(best.map_or(cost, |best: u32| best.min(cost)));
                }
                None => limited |= !scratch.last_search_exhausted(),
            }
        }
        match (best, limited) {
            (Some(cost), false) => CostResult::Exact(cost),
            (Some(cost), true) => CostResult::Bounded(cost),
            (None, false) => CostResult::Unreachable,
            (None, true) => CostResult::SearchLimit,
        }
    }

    #[cfg(test)]
    pub fn work(&self) -> Work {
        self.work
    }
}

fn octile(from: TilePos, to: TilePos) -> u32 {
    let dx = from.x.abs_diff(to.x);
    let dy = from.y.abs_diff(to.y);
    10 * dx.max(dy) + 4 * dx.min(dy)
}

#[cfg(test)]
mod tests;

/// Stable point or footprint whose movement-domain access defines service.
///
/// This is a real demand target rather than a route-component index. Component
/// indices depend on discovery order, while a point or footprint remains stable
/// across equivalent derivations and can be ordered canonically in row-major
/// map order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceTarget {
    /// A mobile contact or ordinary movement destination.
    Point(TilePos),
    /// A building or planned building whose reachable doorstep is the goal.
    Footprint {
        /// Top-left footprint anchor.
        anchor: TilePos,
        /// Positive footprint width and height.
        size: (i32, i32),
    },
}

impl ServiceTarget {
    pub(crate) const fn point(tile: TilePos) -> Self {
        Self::Point(tile)
    }

    pub(crate) const fn footprint(anchor: TilePos, size: (i32, i32)) -> Self {
        Self::Footprint { anchor, size }
    }
}

impl Ord for ServiceTarget {
    fn cmp(&self, other: &Self) -> Ordering {
        service_target_key(*self).cmp(&service_target_key(*other))
    }
}

impl PartialOrd for ServiceTarget {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn service_target_key(service: ServiceTarget) -> ((i32, i32), u8, i32, i32) {
    match service {
        ServiceTarget::Point(tile) => ((tile.y, tile.x), 0, 0, 0),
        ServiceTarget::Footprint { anchor, size } => ((anchor.y, anchor.x), 1, size.1, size.0),
    }
}
