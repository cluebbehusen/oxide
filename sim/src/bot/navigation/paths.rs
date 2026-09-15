//! Retained canonical paths, distance bounds, and complete endpoint-set ranking.

use super::{BlockedRect, KnownGrid, octile, search::Search};
use chassis::grid::TilePos;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    mem::size_of,
};

const MIB: usize = 1024 * 1024;
const ENTRY_ALLOWANCE: usize = 1024;
const MIN_FIELD_SPAN: i32 = 30;
const TRACKED_FIELD_GOALS: usize = 256;

type FieldKey = (Option<BlockedRect>, TilePos);

type PathKey = (Option<BlockedRect>, TilePos, TilePos);

/// Bot-owned derived routes. Hypothetical layouts cannot evict normal boards;
/// ground-only edits also leave air routes intact.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::bot) struct PathQueries {
    pub(in crate::bot) costs: crate::bot::navigation::CostQueries,
    ground: Vec<Generation>,
    air: Vec<Generation>,
    hypothetical: Vec<Generation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Generation {
    width: i32,
    height: i32,
    blocked: Box<[bool]>,
    paths: BTreeMap<PathKey, Box<[TilePos]>>,
    path_order: VecDeque<PathKey>,
    distances: BTreeMap<TilePos, Box<[u32]>>,
    distance_order: VecDeque<TilePos>,
    field_work: BTreeMap<TilePos, u32>,
    overlay_distances: BTreeMap<FieldKey, Box<[u32]>>,
    overlay_distance_order: VecDeque<FieldKey>,
    overlay_distance_bytes: usize,
    path_bytes: usize,
    distance_bytes: usize,
    budget: usize,
}

impl Generation {
    fn payload_budget(&self) -> usize {
        self.budget.saturating_sub(self.blocked.len()) / 2
    }

    fn reserve_path(&mut self, bytes: usize) -> bool {
        let budget = self.payload_budget();
        if bytes > budget {
            return false;
        }
        while self.path_bytes.saturating_add(bytes) > budget {
            let key = self.path_order.pop_front().expect("retained route budget");
            let path = self.paths.remove(&key).expect("retained route key");
            self.path_bytes -= path.len() * size_of::<TilePos>() + ENTRY_ALLOWANCE;
        }
        self.path_bytes += bytes;
        true
    }

    fn reserve_distance(&mut self, bytes: usize) -> bool {
        let budget = self.payload_budget() / 2;
        if bytes > budget {
            return false;
        }
        // Full-map distance fields must not evict the endpoint routes they
        // help prune. Candidate fields also have a separate eviction budget.
        while self.distance_bytes.saturating_add(bytes) > budget {
            let goal = self
                .distance_order
                .pop_front()
                .expect("retained distance budget");
            let field = self
                .distances
                .remove(&goal)
                .expect("retained distance goal");
            self.field_work.remove(&goal);
            self.distance_bytes -= field.len() * size_of::<u32>() + ENTRY_ALLOWANCE;
        }
        self.distance_bytes += bytes;
        true
    }
    fn reserve_overlay_distance(&mut self, bytes: usize) -> bool {
        let budget = self.payload_budget() / 2;
        if bytes > budget {
            return false;
        }
        while self.overlay_distance_bytes.saturating_add(bytes) > budget {
            let key = self
                .overlay_distance_order
                .pop_front()
                .expect("retained candidate distance budget");
            let field = self
                .overlay_distances
                .remove(&key)
                .expect("retained candidate distance key");
            self.overlay_distance_bytes -= field.len() * size_of::<u32>() + ENTRY_ALLOWANCE;
        }
        self.overlay_distance_bytes += bytes;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::bot) enum CacheClass {
    Ground,
    Air,
    Hypothetical,
}

#[derive(Clone, Copy)]
pub(in crate::bot) struct PathBoard<'a> {
    pub grid: KnownGrid<'a>,
    pub class: CacheClass,
    pub cache: &'a RefCell<PathQueries>,
}

impl PathQueries {
    fn generation(&mut self, grid: KnownGrid<'_>, class: CacheClass) -> Option<&mut Generation> {
        let (entries, blocked, budget, limit) = match class {
            CacheClass::Air => (&mut self.air, grid.blocked, 8 * MIB, 1),
            CacheClass::Hypothetical => (&mut self.hypothetical, grid.blocked, MIB, 2),
            CacheClass::Ground => (&mut self.ground, grid.blocked, 8 * MIB, 1),
        };
        if blocked.len() > budget {
            return None;
        }
        if let Some(index) = entries.iter().position(|entry| {
            entry.width == grid.width
                && entry.height == grid.height
                && entry.blocked.as_ref() == blocked
        }) {
            let entry = entries.remove(index);
            entries.push(entry);
        } else {
            if entries.len() == limit {
                entries.remove(0);
            }
            #[cfg(test)]
            super::work::record(|work| {
                work.generations += 1;
            });
            entries.push(Generation {
                width: grid.width,
                height: grid.height,
                blocked: blocked.into(),
                paths: BTreeMap::new(),
                path_order: VecDeque::new(),
                distances: BTreeMap::new(),
                distance_order: VecDeque::new(),
                field_work: BTreeMap::new(),
                overlay_distances: BTreeMap::new(),
                overlay_distance_order: VecDeque::new(),
                overlay_distance_bytes: 0,
                path_bytes: 0,
                distance_bytes: 0,
                budget,
            });
        }
        entries.last_mut()
    }
}

impl PathBoard<'_> {
    pub fn path(
        self,
        start: TilePos,
        goal: TilePos,
        overlay: Option<BlockedRect>,
        search: &mut Search,
    ) -> Option<Vec<TilePos>> {
        let key = (overlay, start, goal);
        if let Some(path) = self
            .cache
            .borrow_mut()
            .generation(self.grid, self.class)
            .and_then(|g| g.paths.get(&key))
        {
            #[cfg(test)]
            super::work::record(|work| {
                work.hits += 1;
            });
            search.clear_search_evidence();
            return Some(path.to_vec());
        }
        let eligible = self.grid.open(start, overlay)
            && self.grid.index(goal).is_some()
            && self.grid.blocked.len() <= crate::stats::PATH_EXPANSION_CAP as usize;
        if eligible && overlay.is_none() {
            for (source, destination) in [(start, goal), (goal, start)] {
                let promote = self
                    .cache
                    .borrow_mut()
                    .generation(self.grid, self.class)
                    .is_some_and(|generation| {
                        !generation.distances.contains_key(&destination)
                            && generation
                                .field_work
                                .get(&destination)
                                .copied()
                                .unwrap_or(0)
                                >= self.grid.blocked.len() as u32
                    });
                if promote {
                    self.bound(source, destination);
                }
            }
        }
        let result = {
            let mut cache = self.cache.borrow_mut();
            let field = eligible
                .then(|| cache.generation(self.grid, self.class))
                .flatten()
                .and_then(|generation| {
                    if let Some(overlay) = overlay {
                        if start.chebyshev(goal) < MIN_FIELD_SPAN {
                            return None;
                        }
                        let key = (Some(overlay), goal);
                        if !generation.overlay_distances.contains_key(&key) {
                            if !generation.reserve_overlay_distance(
                                self.grid.blocked.len() * size_of::<u32>() + ENTRY_ALLOWANCE,
                            ) {
                                return None;
                            }
                            let field =
                                distance_field(self.grid.width, self.grid.height, goal, |tile| {
                                    self.grid.open(tile, Some(overlay))
                                });
                            generation
                                .overlay_distances
                                .insert(key, field.into_boxed_slice());
                            generation.overlay_distance_order.push_back(key);
                        }
                        generation.overlay_distances.get(&key)
                    } else {
                        // Existing normal-board bounds are also exact route fields.
                        generation.distances.get(&goal)
                    }
                });
            if let Some(field) = field {
                search.path_with_distances(self.grid, overlay, start, goal, field)
            } else {
                search.path(self.grid.width, self.grid.height, start, goal, |tile| {
                    self.grid.open(tile, overlay)
                })
            }
        };
        if eligible
            && overlay.is_none()
            && let Some(generation) = self.cache.borrow_mut().generation(self.grid, self.class)
        {
            for endpoint in [start, goal] {
                if generation.distances.contains_key(&endpoint) {
                    continue;
                }
                if !generation.field_work.contains_key(&endpoint)
                    && generation.field_work.len() == TRACKED_FIELD_GOALS
                {
                    generation.field_work.pop_first();
                }
                let work = generation.field_work.entry(endpoint).or_default();
                *work = work.saturating_add(search.last_expansions());
            }
        }
        // Failures retain query-specific exhaustion evidence, never a bare cached absence.
        if let Some(path) = &result
            && let Some(generation) = self.cache.borrow_mut().generation(self.grid, self.class)
            && generation.reserve_path(path.len() * size_of::<TilePos>() + ENTRY_ALLOWANCE)
        {
            generation
                .paths
                .insert(key, path.clone().into_boxed_slice());
            generation.path_order.push_back(key);
        }
        result
    }

    fn prepare_endpoint_batch(
        self,
        start: TilePos,
        goal: TilePos,
        queries: usize,
        search: &Search,
    ) {
        let cells = self.grid.blocked.len();
        if queries >= 8
            && cells <= crate::stats::PATH_EXPANSION_CAP as usize
            && (search.last_expansions() as usize).saturating_mul(queries) >= cells
        {
            self.bound(goal, start);
        }
    }

    fn pruning_bound(self, start: TilePos, goal: TilePos) -> u32 {
        let fallback = octile(start, goal);
        if !self.grid.open(start, None) {
            return fallback;
        }
        self.cache
            .borrow_mut()
            .generation(self.grid, self.class)
            .map_or(fallback, |generation| {
                generation
                    .distances
                    .get(&goal)
                    .map(|field| field[self.grid.index(start).unwrap()])
                    .or_else(|| {
                        generation
                            .distances
                            .get(&start)
                            .and_then(|field| self.grid.index(goal).map(|index| field[index]))
                    })
                    .unwrap_or(fallback)
            })
    }

    pub fn bound(self, start: TilePos, goal: TilePos) -> u32 {
        let fallback = octile(start, goal);
        let Some(index) = self.grid.index(start) else {
            return fallback;
        };
        // A blocked origin can escape; a reverse open-tile field cannot represent it.
        if !self.grid.open(start, None) {
            return fallback;
        }
        let mut cache = self.cache.borrow_mut();
        let Some(generation) = cache.generation(self.grid, self.class) else {
            return fallback;
        };
        if let Some(field) = generation.distances.get(&goal) {
            #[cfg(test)]
            super::work::record(|work| {
                work.hits += 1;
            });
            return field[index];
        }
        if !generation
            .reserve_distance(generation.blocked.len() * size_of::<u32>() + ENTRY_ALLOWANCE)
        {
            return fallback;
        }
        let field = distance_field(self.grid.width, self.grid.height, goal, |tile| {
            self.grid.open(tile, None)
        });
        let result = field[index];
        generation.distances.insert(goal, field.into_boxed_slice());
        generation.distance_order.push_back(goal);
        result
    }
}
fn distance_field(
    width: i32,
    height: i32,
    goal: TilePos,
    open: impl Fn(TilePos) -> bool,
) -> Vec<u32> {
    #[cfg(test)]
    super::work::record(|work| {
        work.fields += 1;
        work.searches += 1;
    });
    let width = width.max(0);
    let height = height.max(0);
    let surface = (0..height)
        .flat_map(|y| (0..width).map(move |x| TilePos::new(x, y)))
        .map(open)
        .collect();
    let mut work = super::distance_work::DistanceWork::new(width, height, surface, [goal]);
    let mut budget = crate::bot::planning::WorkBudget::new(usize::MAX);
    let result = work.advance(&mut budget);
    debug_assert_eq!(result, crate::bot::planning::Progress::Ready(()));
    work.into_distances()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct BaselineEndpointRoute {
    path: Option<Vec<TilePos>>,
    exhausted: bool,
}

pub(in crate::bot) type EndpointRoutes =
    BTreeMap<(CacheClass, TilePos, TilePos), BaselineEndpointRoute>;

pub(in crate::bot) struct CandidatePaths<'a> {
    board: PathBoard<'a>,
    overlay: Option<BlockedRect>,
    paths: BTreeMap<(TilePos, TilePos), Vec<TilePos>>,
}

impl<'a> CandidatePaths<'a> {
    pub fn new(board: PathBoard<'a>, overlay: Option<BlockedRect>) -> Self {
        Self {
            board,
            overlay,
            paths: BTreeMap::new(),
        }
    }
    pub fn shortest(
        &mut self,
        starts: &[TilePos],
        goals: &[TilePos],
        baseline: &mut EndpointRoutes,
    ) -> Option<(TilePos, TilePos, Vec<TilePos>)> {
        shortest_path_between_cached(self, starts, goals, baseline)
    }
    pub fn path(
        &mut self,
        start: TilePos,
        goal: TilePos,
        search: &mut Search,
    ) -> Option<Vec<TilePos>> {
        if let Some(path) = self.paths.get(&(start, goal)) {
            #[cfg(test)]
            super::work::record(|work| {
                work.hits += 1;
            });
            search.clear_search_evidence();
            return Some(path.clone());
        }
        let path = self.board.path(start, goal, self.overlay, search);
        if let Some(path) = &path {
            self.paths.insert((start, goal), path.clone());
        }
        path
    }
}

pub(in crate::bot) fn path_cost(path: &[TilePos]) -> u32 {
    path.windows(2)
        .map(|pair| {
            if pair[0].x != pair[1].x && pair[0].y != pair[1].y {
                14
            } else {
                10
            }
        })
        .sum()
}
pub(in crate::bot) fn shortest_path_between(
    board: PathBoard<'_>,
    starts: &[TilePos],
    goals: &[TilePos],
    candidate: Option<BlockedRect>,
) -> Option<(TilePos, TilePos, Vec<TilePos>)> {
    let mut pairs: Vec<_> = starts
        .iter()
        .flat_map(|start| goals.iter().map(move |goal| (*start, *goal)))
        .collect();
    pairs.sort_unstable_by_key(|(start, goal)| {
        (octile(*start, *goal), start.y, start.x, goal.y, goal.x)
    });

    let mut best: Option<(TilePos, TilePos, Vec<TilePos>)> = None;
    let mut proven_unreachable = BTreeSet::new();
    let mut scratch = Search::default();
    for (start, goal) in pairs {
        if proven_unreachable.contains(&(start, goal)) {
            continue;
        }
        if best
            .as_ref()
            .is_some_and(|(_, _, path)| octile(start, goal) > path_cost(path))
        {
            // Every remaining pair has a strictly worse obstacle-free lower
            // bound, so none can replace the complete route-choice key.
            break;
        }
        if best
            .as_ref()
            .is_some_and(|(_, _, path)| board.pruning_bound(start, goal) > path_cost(path))
        {
            continue;
        }
        let result = board.path(start, goal, candidate, &mut scratch);
        board.prepare_endpoint_batch(start, goal, goals.len(), &scratch);
        let Some(mut path) = result else {
            if scratch.last_search_exhausted() {
                // One exhaustive search proves the whole passability
                // component. Reuse that proof for its other doorsteps.
                let reached_starts: Vec<_> = starts
                    .iter()
                    .copied()
                    .filter(|tile| scratch.last_search_reached(*tile))
                    .collect();
                for reached_start in reached_starts {
                    for unreachable_goal in goals
                        .iter()
                        .copied()
                        .filter(|tile| !scratch.last_search_reached(*tile))
                    {
                        proven_unreachable.insert((reached_start, unreachable_goal));
                    }
                }
            }
            continue;
        };
        path.insert(0, start);
        let replace = best
            .as_ref()
            .is_none_or(|(best_start, best_goal, best_path)| {
                (
                    path_cost(&path),
                    path.len(),
                    start.y,
                    start.x,
                    goal.y,
                    goal.x,
                    path.as_slice(),
                ) < (
                    path_cost(best_path),
                    best_path.len(),
                    best_start.y,
                    best_start.x,
                    best_goal.y,
                    best_goal.x,
                    best_path.as_slice(),
                )
            });
        if replace {
            best = Some((start, goal, path));
        }
    }
    best
}

fn shortest_path_between_cached(
    routes: &mut CandidatePaths<'_>,
    starts: &[TilePos],
    goals: &[TilePos],
    baseline_endpoint_routes: &mut EndpointRoutes,
) -> Option<(TilePos, TilePos, Vec<TilePos>)> {
    let board = routes.board;
    let candidate = routes.overlay;
    let mut pairs: Vec<_> = starts
        .iter()
        .flat_map(|start| goals.iter().map(move |goal| (*start, *goal)))
        .collect();
    pairs.sort_unstable_by_key(|(start, goal)| {
        (octile(*start, *goal), start.y, start.x, goal.y, goal.x)
    });

    let mut best: Option<(TilePos, TilePos, Vec<TilePos>)> = None;
    let mut scratch = Search::default();
    let mut proven_unreachable = BTreeSet::new();
    for (start, goal) in pairs {
        if proven_unreachable.contains(&(start, goal)) {
            continue;
        }
        if best
            .as_ref()
            .is_some_and(|(_, _, path)| octile(start, goal) > path_cost(path))
        {
            break;
        }

        if best
            .as_ref()
            .is_some_and(|(_, _, path)| board.pruning_bound(start, goal) > path_cost(path))
        {
            continue;
        }
        let key = (board.class, start, goal);
        let baseline = match baseline_endpoint_routes.entry(key) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let mut path = board.path(start, goal, None, &mut scratch);
                board.prepare_endpoint_batch(start, goal, goals.len(), &scratch);
                if let Some(path) = path.as_mut() {
                    path.insert(0, start);
                }
                entry.insert(BaselineEndpointRoute {
                    path,
                    exhausted: scratch.last_search_exhausted(),
                })
            }
        };
        if best.as_ref().is_some_and(|(_, _, best_path)| {
            baseline
                .path
                .as_ref()
                .is_some_and(|baseline_path| path_cost(baseline_path) > path_cost(best_path))
        }) {
            // A blocking footprint cannot improve this endpoint pair over its
            // no-candidate route. Keep equal-cost pairs because the complete
            // route-choice key can still prefer their length, endpoints, or
            // canonical path.
            continue;
        }
        let path = match &baseline.path {
            Some(path) if !candidate.is_some_and(|overlay| overlay.affects_path(path)) => {
                Some(path.clone())
            }
            Some(_) => {
                let mut path = routes.path(start, goal, &mut scratch);
                if let Some(path) = path.as_mut() {
                    path.insert(0, start);
                }
                path
            }
            None if baseline.exhausted => None,
            None => {
                let mut path = routes.path(start, goal, &mut scratch);
                if let Some(path) = path.as_mut() {
                    path.insert(0, start);
                }
                path
            }
        };
        let Some(path) = path else {
            if scratch.last_search_exhausted() {
                // Both baseline and candidate searches are lower bounds on
                // connectivity after adding the candidate's blocking tiles.
                for reached_start in starts
                    .iter()
                    .copied()
                    .filter(|tile| scratch.last_search_reached(*tile))
                {
                    for unreachable_goal in goals
                        .iter()
                        .copied()
                        .filter(|tile| !scratch.last_search_reached(*tile))
                    {
                        proven_unreachable.insert((reached_start, unreachable_goal));
                    }
                }
            }
            continue;
        };
        let replace = best
            .as_ref()
            .is_none_or(|(best_start, best_goal, best_path)| {
                (
                    path_cost(&path),
                    path.len(),
                    start.y,
                    start.x,
                    goal.y,
                    goal.x,
                    path.as_slice(),
                ) < (
                    path_cost(best_path),
                    best_path.len(),
                    best_start.y,
                    best_start.x,
                    best_goal.y,
                    best_goal.x,
                    best_path.as_slice(),
                )
            });
        if replace {
            best = Some((start, goal, path));
        }
    }
    best
}

#[cfg(test)]
mod tests;
