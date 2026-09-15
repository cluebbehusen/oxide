//! Retained public-terrain distances for logistics and threat travel.

use crate::bot::PublicMapBriefing;
use crate::stats::BuildingKind;
use chassis::grid::TilePos;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct PublicGroundDistances {
    width: i32,
    height: i32,
    distances: Vec<u32>,
}

/// An owned field job, including the passability preparation before traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct PublicFieldWork {
    width: i32,
    height: i32,
    sources: Vec<TilePos>,
    open: Vec<bool>,
    traversal: Option<super::distance_work::DistanceWork>,
    ready: Option<Arc<PublicGroundDistances>>,
}

impl PublicFieldWork {
    pub(in crate::bot) fn new(map: &PublicMapBriefing, sources: Vec<TilePos>) -> Self {
        Self {
            width: map.map_width().max(0),
            height: map.map_height().max(0),
            sources,
            open: Vec::new(),
            traversal: None,
            ready: None,
        }
    }

    pub(in crate::bot) fn is_ready(&self) -> bool {
        self.ready.is_some()
    }

    pub(in crate::bot) fn advance(
        &mut self,
        map: &PublicMapBriefing,
        blocked: &BlockedGroundLayout,
        budget: &mut crate::bot::planning::WorkBudget,
    ) -> crate::bot::planning::Progress<Arc<PublicGroundDistances>> {
        use crate::bot::planning::Progress;
        if let Some(ready) = &self.ready {
            return Progress::Ready(Arc::clone(ready));
        }
        let cells = self.width as usize * self.height as usize;
        if self.traversal.is_none() {
            while self.open.len() < cells {
                if !budget.charge(1) {
                    return Progress::Deferred;
                }
                let index = self.open.len();
                let tile = TilePos::new(
                    (index % self.width as usize) as i32,
                    (index / self.width as usize) as i32,
                );
                self.open
                    .push(PublicGroundDistances::ground_open(map, tile) && !blocked.contains(tile));
            }
            self.traversal = Some(super::distance_work::DistanceWork::new(
                self.width,
                self.height,
                std::mem::take(&mut self.open),
                self.sources.iter().copied(),
            ));
        }
        match self.traversal.as_mut().unwrap().advance(budget) {
            Progress::Deferred => Progress::Deferred,
            Progress::ProvenInfeasible => unreachable!("a field retains unreachable cells"),
            Progress::Ready(()) => {
                let distances = self.traversal.take().unwrap().into_distances();
                let ready = Arc::new(PublicGroundDistances {
                    width: self.width,
                    height: self.height,
                    distances,
                });
                self.ready = Some(Arc::clone(&ready));
                Progress::Ready(ready)
            }
        }
    }
}

/// Exact dynamic ground exclusions for expansion logistics routing.
///
/// The dense membership form makes every Dijkstra edge check constant-time,
/// while equality remains an exact invalidation key for both projected danger
/// and the policy's independently retained contested-work regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct BlockedGroundLayout {
    width: i32,
    height: i32,
    blocked: Vec<bool>,
}

impl BlockedGroundLayout {
    pub(in crate::bot) fn from_predicate(
        public_map: &PublicMapBriefing,
        mut blocked: impl FnMut(TilePos) -> bool,
    ) -> Self {
        let width = public_map.map_width();
        let height = public_map.map_height();
        let cells = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let blocked = (0..cells)
            .map(|index| {
                let index = i32::try_from(index).unwrap_or(i32::MAX);
                let tile = if width > 0 {
                    TilePos::new(index % width, index / width)
                } else {
                    TilePos::new(-1, -1)
                };
                blocked(tile)
            })
            .collect();
        Self {
            width,
            height,
            blocked,
        }
    }

    pub(in crate::bot) fn contains(&self, tile: TilePos) -> bool {
        PublicGroundDistances::index_for(self.width, self.height, tile)
            .and_then(|index| self.blocked.get(index))
            .copied()
            .unwrap_or(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DangerAwareDistanceGeneration {
    blocked: BlockedGroundLayout,
    #[cfg(test)]
    fields: BTreeMap<TilePos, Arc<PublicGroundDistances>>,
    source_sets: BTreeMap<Vec<TilePos>, Arc<PublicGroundDistances>>,
}

/// Bounded routing memoization for the expansion planner.
///
/// Dynamic logistics fields retain only the sources present in the latest
/// exact danger generation. Terrain-only threat fields likewise retain only
/// currently known threat positions. Authored start fields are permanent for
/// one briefing and therefore bounded by its starting-seat count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::bot) struct PublicRoutes {
    public_map: Option<PublicMapBriefing>,
    danger_aware: Option<DangerAwareDistanceGeneration>,
    threat_fields: BTreeMap<TilePos, Arc<PublicGroundDistances>>,
    footprint_fields: BTreeMap<TilePos, Arc<PublicGroundDistances>>,
    start_fields: BTreeMap<TilePos, Arc<PublicGroundDistances>>,
    #[cfg(test)]
    builds: PublicRouteBuilds,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::bot) struct PublicRouteBuilds {
    pub(in crate::bot) danger_aware: usize,
    pub(in crate::bot) threats: usize,
    pub(in crate::bot) starts: usize,
}

impl PublicRoutes {
    fn prepare_map(&mut self, public_map: &PublicMapBriefing) {
        if self
            .public_map
            .as_ref()
            .is_some_and(|cached| cached == public_map)
        {
            return;
        }
        #[cfg(test)]
        super::work::record(|work| {
            work.generations += 1;
        });
        self.public_map = Some(public_map.clone());
        self.danger_aware = None;
        self.threat_fields.clear();
        self.footprint_fields.clear();
        self.start_fields.clear();
    }

    #[cfg(test)]
    pub(in crate::bot) fn danger_aware_fields(
        &mut self,
        public_map: &PublicMapBriefing,
        blocked: BlockedGroundLayout,
        sources: impl IntoIterator<Item = TilePos>,
    ) -> Vec<(TilePos, Arc<PublicGroundDistances>)> {
        self.prepare_map(public_map);
        let mut sources = sources.into_iter().collect::<Vec<_>>();
        sources.sort_unstable_by_key(|tile| (tile.y, tile.x));
        sources.dedup();
        if self
            .danger_aware
            .as_ref()
            .is_none_or(|generation| generation.blocked != blocked)
        {
            #[cfg(test)]
            super::work::record(|work| work.generations += 1);
            self.danger_aware = Some(DangerAwareDistanceGeneration {
                blocked,
                #[cfg(test)]
                fields: BTreeMap::new(),
                source_sets: BTreeMap::new(),
            });
        }
        let generation = self
            .danger_aware
            .as_mut()
            .expect("a danger-aware generation was prepared");
        generation.fields.retain(|source, _| {
            sources
                .binary_search_by_key(&(source.y, source.x), |tile| (tile.y, tile.x))
                .is_ok()
        });
        for &source in &sources {
            generation.fields.entry(source).or_insert_with(|| {
                #[cfg(test)]
                {
                    self.builds.danger_aware += 1;
                }
                Arc::new(PublicGroundDistances::from_sources_avoiding(
                    public_map,
                    [source],
                    |tile| generation.blocked.contains(tile),
                ))
            });
        }
        sources
            .into_iter()
            .filter_map(|source| {
                generation
                    .fields
                    .get(&source)
                    .map(|field| (source, Arc::clone(field)))
            })
            .collect()
    }

    pub(in crate::bot) fn danger_aware_source_set(
        &mut self,
        public_map: &PublicMapBriefing,
        blocked: &BlockedGroundLayout,
        sources: impl IntoIterator<Item = TilePos>,
    ) -> Arc<PublicGroundDistances> {
        self.prepare_map(public_map);
        if self
            .danger_aware
            .as_ref()
            .is_none_or(|generation| generation.blocked != *blocked)
        {
            self.danger_aware = Some(DangerAwareDistanceGeneration {
                blocked: blocked.clone(),
                #[cfg(test)]
                fields: BTreeMap::new(),
                source_sets: BTreeMap::new(),
            });
            #[cfg(test)]
            super::work::record(|work| work.generations += 1);
        }
        let generation = self.danger_aware.as_mut().unwrap();
        let mut sources = sources.into_iter().collect::<Vec<_>>();
        sources.sort_unstable_by_key(|tile| (tile.y, tile.x));
        sources.dedup();
        if let Some(field) = generation.source_sets.get(&sources) {
            return Arc::clone(field);
        }
        if generation.source_sets.len() >= 9 {
            generation.source_sets.pop_first();
        }
        let field = Arc::new(PublicGroundDistances::from_sources_avoiding(
            public_map,
            sources.iter().copied(),
            |tile| blocked.contains(tile),
        ));
        #[cfg(test)]
        {
            self.builds.danger_aware += 1;
        }
        generation.source_sets.insert(sources, Arc::clone(&field));
        field
    }

    pub(in crate::bot) fn threat_fields(
        &mut self,
        public_map: &PublicMapBriefing,
        sources: impl IntoIterator<Item = TilePos>,
    ) -> BTreeMap<TilePos, Arc<PublicGroundDistances>> {
        self.prepare_map(public_map);
        let mut sources = sources.into_iter().collect::<Vec<_>>();
        sources.sort_unstable_by_key(|tile| (tile.y, tile.x));
        sources.dedup();
        self.threat_fields.retain(|source, _| {
            sources
                .binary_search_by_key(&(source.y, source.x), |tile| (tile.y, tile.x))
                .is_ok()
        });
        for &source in &sources {
            self.threat_fields.entry(source).or_insert_with(|| {
                #[cfg(test)]
                {
                    self.builds.threats += 1;
                }
                Arc::new(PublicGroundDistances::from_sources(public_map, [source]))
            });
        }
        self.threat_fields.clone()
    }

    pub(in crate::bot) fn foundry_fields(
        &mut self,
        public_map: &PublicMapBriefing,
        anchors: impl IntoIterator<Item = TilePos>,
    ) -> BTreeMap<TilePos, Arc<PublicGroundDistances>> {
        self.prepare_map(public_map);
        let mut anchors = anchors.into_iter().collect::<Vec<_>>();
        anchors.sort_unstable();
        anchors.dedup();
        self.footprint_fields
            .retain(|anchor, _| anchors.binary_search(anchor).is_ok());
        for anchor in anchors {
            self.footprint_fields.entry(anchor).or_insert_with(|| {
                #[cfg(test)]
                {
                    self.builds.threats += 1;
                }
                Arc::new(PublicGroundDistances::from_sources(
                    public_map,
                    foundry_footprint_tiles(anchor),
                ))
            });
        }
        self.footprint_fields.clone()
    }

    pub(in crate::bot) fn start_fields(
        &mut self,
        public_map: &PublicMapBriefing,
        starts: &[crate::bot::StartingFoundry],
    ) -> Vec<Arc<PublicGroundDistances>> {
        self.prepare_map(public_map);
        starts
            .iter()
            .map(|start| {
                Arc::clone(self.start_fields.entry(start.anchor).or_insert_with(|| {
                    #[cfg(test)]
                    {
                        self.builds.starts += 1;
                    }
                    Arc::new(PublicGroundDistances::from_sources(
                        public_map,
                        foundry_footprint_tiles(start.anchor),
                    ))
                }))
            })
            .collect()
    }

    #[cfg(test)]
    pub(in crate::bot) fn build_count(&self) -> PublicRouteBuilds {
        self.builds
    }

    #[cfg(test)]
    pub(in crate::bot) fn retained_field_counts(&self) -> (usize, usize, usize) {
        (
            self.danger_aware.as_ref().map_or(0, |generation| {
                generation.fields.len() + generation.source_sets.len()
            }),
            self.threat_fields.len() + self.footprint_fields.len(),
            self.start_fields.len(),
        )
    }
}

impl PublicGroundDistances {
    pub(in crate::bot) fn from_sources(
        public_map: &PublicMapBriefing,
        sources: impl IntoIterator<Item = TilePos>,
    ) -> Self {
        Self::from_sources_avoiding(public_map, sources, |_| false)
    }

    pub(in crate::bot) fn from_sources_avoiding(
        public_map: &PublicMapBriefing,
        sources: impl IntoIterator<Item = TilePos>,
        mut blocked: impl FnMut(TilePos) -> bool,
    ) -> Self {
        #[cfg(test)]
        super::work::record(|work| {
            work.fields += 1;
            work.searches += 1;
        });
        let width = public_map.map_width();
        let height = public_map.map_height();
        let cells = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let open = (0..cells)
            .map(|index| {
                let tile = TilePos::new(
                    (index % width as usize) as i32,
                    (index / width as usize) as i32,
                );
                Self::ground_open(public_map, tile) && !blocked(tile)
            })
            .collect();
        let mut work =
            super::distance_work::DistanceWork::new(width.max(0), height.max(0), open, sources);
        let result = work.advance(&mut crate::bot::planning::WorkBudget::new(usize::MAX));
        debug_assert_eq!(result, crate::bot::planning::Progress::Ready(()));
        let distances = work.into_distances();

        Self {
            width,
            height,
            distances,
        }
    }

    fn ground_open(public_map: &PublicMapBriefing, tile: TilePos) -> bool {
        public_map
            .terrain_at(tile)
            .is_some_and(|terrain| !terrain.blocks_ground())
    }

    fn index_for(width: i32, height: i32, tile: TilePos) -> Option<usize> {
        if tile.x < 0 || tile.y < 0 || tile.x >= width || tile.y >= height {
            return None;
        }
        usize::try_from(tile.y)
            .ok()?
            .checked_mul(usize::try_from(width).ok()?)?
            .checked_add(usize::try_from(tile.x).ok()?)
    }

    /// Visits reachable, fully in-bounds footprint anchors in row-major order.
    /// The index includes unreachable anchors, allowing callers to accumulate
    /// several fields into the same dense candidate array.
    #[cfg(test)]
    pub(in crate::bot) fn visit_footprint_distances(
        &self,
        size: (i32, i32),
        mut visit: impl FnMut(usize, u32),
    ) {
        if size.0 <= 0 || size.1 <= 0 || size.0 > self.width || size.1 > self.height {
            return;
        }
        let width = self.width as usize;
        let columns = (self.width - size.0 + 1) as usize;
        for y in 0..=(self.height - size.1) as usize {
            for x in 0..columns {
                let mut distance = u32::MAX;
                for dy in 0..size.1 as usize {
                    let start = (y + dy) * width + x;
                    for value in &self.distances[start..start + size.0 as usize] {
                        distance = distance.min(*value);
                    }
                }
                if distance != u32::MAX {
                    visit(y * columns + x, distance);
                }
            }
        }
    }

    pub(in crate::bot) fn footprint_distance(
        &self,
        anchor: TilePos,
        size: (i32, i32),
    ) -> Option<u32> {
        (0..size.1)
            .flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
            .filter_map(|tile| {
                Self::index_for(self.width, self.height, tile)
                    .and_then(|index| self.distances.get(index).copied())
                    .filter(|distance| *distance != u32::MAX)
            })
            .min()
    }
}

fn foundry_footprint_tiles(anchor: TilePos) -> impl Iterator<Item = TilePos> {
    let size = BuildingKind::Foundry.base_stats().size;
    (0..size.1).flat_map(move |dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
}

pub(in crate::bot) struct WorkRoutes<'a, 'b> {
    commands: &'a super::commands::RouteProjection<'b>,
    distances: &'a PublicGroundDistances,
    doors: &'a [TilePos],
    origins: BTreeMap<TilePos, Option<u32>>,
}
impl<'a, 'b> WorkRoutes<'a, 'b> {
    pub fn new(
        commands: &'a super::commands::RouteProjection<'b>,
        distances: &'a PublicGroundDistances,
        doors: &'a [TilePos],
    ) -> Self {
        Self {
            commands,
            distances,
            doors,
            origins: BTreeMap::new(),
        }
    }
    pub fn distance(&mut self, origin: TilePos) -> Option<u32> {
        *self.origins.entry(origin).or_insert_with(|| {
            let door = self
                .doors
                .iter()
                .min_by_key(|door| (door.chebyshev(origin), door.y, door.x))?;
            (self.commands.direct_line_avoids_blocked(origin, *door)
                && self.commands.command_path_avoids_blocked(origin, *door))
            .then(|| self.distances.footprint_distance(origin, (1, 1)))
            .flatten()
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn batched_footprint_distances_match_scalar_edges_and_unreachable_tiles() {
        for width in 1..9 {
            for height in 1..8 {
                let distances = super::PublicGroundDistances {
                    width,
                    height,
                    distances: (0..width * height)
                        .map(|i| if i % 7 < 3 { u32::MAX } else { (i * 13) as u32 })
                        .collect(),
                };
                for w in 1..=width + 1 {
                    for h in 1..=height + 1 {
                        let mut actual = Vec::new();
                        distances.visit_footprint_distances((w, h), |index, cost| {
                            actual.push((index, cost))
                        });
                        let expected: Vec<_> = (0..=(height - h))
                            .flat_map(|y| (0..=(width - w)).map(move |x| (x, y)))
                            .filter_map(|(x, y)| {
                                distances
                                    .footprint_distance(chassis::grid::TilePos::new(x, y), (w, h))
                                    .map(|cost| ((y * (width - w + 1) + x) as usize, cost))
                            })
                            .collect();
                        assert_eq!(actual, expected);
                    }
                }
            }
        }
    }

    use super::*;
    use core::cmp::Reverse;
    fn briefing(
        width: i32,
        height: i32,
        walls: impl IntoIterator<Item = TilePos>,
        starts: Vec<crate::bot::StartingFoundry>,
    ) -> PublicMapBriefing {
        let mut non_ground_terrain = walls
            .into_iter()
            .map(|tile| (tile, crate::map::Terrain::Rock))
            .collect::<Vec<_>>();
        non_ground_terrain.sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
        PublicMapBriefing {
            regions: Default::default(),
            map_width: width,
            map_height: height,
            starting_foundries: starts,
            teams: vec![Some(0), Some(1)],
            non_ground_terrain,
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        }
    }

    fn reference_ground_distances(
        public_map: &PublicMapBriefing,
        sources: impl IntoIterator<Item = TilePos>,
        blocked: &BlockedGroundLayout,
    ) -> PublicGroundDistances {
        use std::collections::BinaryHeap;

        let width = public_map.map_width();
        let height = public_map.map_height();
        let cells = usize::try_from(width * height).expect("small test map");
        let mut distances = vec![u32::MAX; cells];
        let mut frontier = BinaryHeap::new();
        for source in sources {
            if !PublicGroundDistances::ground_open(public_map, source) || blocked.contains(source) {
                continue;
            }
            let Some(index) = PublicGroundDistances::index_for(width, height, source) else {
                continue;
            };
            if distances[index] == 0 {
                continue;
            }
            distances[index] = 0;
            frontier.push(Reverse((0u32, source.y, source.x)));
        }
        while let Some(Reverse((distance, y, x))) = frontier.pop() {
            let current = TilePos::new(x, y);
            let Some(current_index) = PublicGroundDistances::index_for(width, height, current)
            else {
                continue;
            };
            if distances[current_index] != distance {
                continue;
            }
            for (dx, dy, step) in [
                (-1, 0, 10),
                (1, 0, 10),
                (0, -1, 10),
                (0, 1, 10),
                (-1, -1, 14),
                (1, -1, 14),
                (-1, 1, 14),
                (1, 1, 14),
            ] {
                let next = current.offset(dx, dy);
                if !PublicGroundDistances::ground_open(public_map, next)
                    || blocked.contains(next)
                    || (dx != 0
                        && dy != 0
                        && (!PublicGroundDistances::ground_open(public_map, current.offset(dx, 0))
                            || blocked.contains(current.offset(dx, 0))
                            || !PublicGroundDistances::ground_open(
                                public_map,
                                current.offset(0, dy),
                            )
                            || blocked.contains(current.offset(0, dy))))
                {
                    continue;
                }
                let Some(next_index) = PublicGroundDistances::index_for(width, height, next) else {
                    continue;
                };
                let next_distance = distance.saturating_add(step);
                if next_distance < distances[next_index] {
                    distances[next_index] = next_distance;
                    frontier.push(Reverse((next_distance, next.y, next.x)));
                }
            }
        }
        PublicGroundDistances {
            width,
            height,
            distances,
        }
    }

    #[test]
    fn bounded_bucket_routes_match_the_reference_dijkstra_exactly() {
        let walls = (0..9)
            .filter(|y| !matches!(y, 2 | 7))
            .map(|y| TilePos::new(6, y))
            .chain([TilePos::new(3, 4), TilePos::new(4, 3)]);
        let public_map = briefing(13, 9, walls, Vec::new());
        let blocked = BlockedGroundLayout::from_predicate(&public_map, |tile| {
            matches!((tile.x, tile.y), (8, 2) | (8, 3) | (9, 3))
        });
        let sources = [
            TilePos::new(1, 1),
            TilePos::new(11, 7),
            TilePos::new(1, 1),
            TilePos::new(-1, -1),
        ];

        let actual = PublicGroundDistances::from_sources_avoiding(&public_map, sources, |tile| {
            blocked.contains(tile)
        });
        let reference = reference_ground_distances(&public_map, sources, &blocked);

        assert_eq!(actual, reference);
    }

    #[test]
    fn distance_expansion_never_requeries_the_passability_predicate() {
        let map = briefing(32, 24, [], Vec::new());
        let mut calls = 0;
        let field =
            PublicGroundDistances::from_sources_avoiding(&map, [TilePos::new(0, 0)], |tile| {
                calls += 1;
                tile.x == 16 && tile.y != 20
            });
        assert_eq!(calls, 32 * 24);
        assert!(
            field
                .footprint_distance(TilePos::new(31, 0), (1, 1))
                .is_some()
        );
        for (width, height) in [(0, 10), (10, 0), (-1, 10)] {
            let empty = PublicGroundDistances::from_sources(
                &briefing(width, height, [], Vec::new()),
                [TilePos::new(0, 0)],
            );
            assert_eq!(empty.footprint_distance(TilePos::new(0, 0), (1, 1)), None);
        }
    }

    #[test]
    fn reverse_footprint_fields_match_forward_node_queries_with_danger_and_walls() {
        let map = briefing(
            18,
            13,
            (0..13).filter(|y| *y != 7).map(|y| TilePos::new(8, y)),
            Vec::new(),
        );
        let blocked = BlockedGroundLayout::from_predicate(&map, |tile| {
            matches!((tile.x, tile.y), (4, 3) | (4, 4) | (5, 4))
        });
        let anchors = [TilePos::new(1, 1), TilePos::new(13, 9)];
        let mut cache = PublicRoutes::default();
        let reverse = cache.danger_aware_source_set(
            &map,
            &blocked,
            anchors.into_iter().flat_map(foundry_footprint_tiles),
        );
        for y in 0..13 {
            for x in 0..18 {
                let node = TilePos::new(x, y);
                let forward = reference_ground_distances(&map, [node], &blocked);
                let expected = anchors
                    .iter()
                    .filter_map(|&anchor| {
                        forward.footprint_distance(anchor, BuildingKind::Foundry.base_stats().size)
                    })
                    .min();
                assert_eq!(
                    reverse.footprint_distance(node, (1, 1)),
                    expected,
                    "{node:?}"
                );
            }
        }
        let repeated = cache.danger_aware_source_set(
            &map,
            &blocked,
            anchors.into_iter().rev().flat_map(foundry_footprint_tiles),
        );
        assert!(Arc::ptr_eq(&reverse, &repeated));
        let changed = BlockedGroundLayout::from_predicate(&map, |_| true);
        assert_eq!(
            cache
                .danger_aware_source_set(
                    &map,
                    &changed,
                    anchors.into_iter().flat_map(foundry_footprint_tiles)
                )
                .footprint_distance(anchors[0], (1, 1)),
            None
        );
    }

    #[test]
    fn footprint_source_set_retention_has_a_fixed_entry_bound() {
        let map = briefing(18, 13, [], Vec::new());
        let blocked = BlockedGroundLayout::from_predicate(&map, |_| false);
        let mut cache = PublicRoutes::default();
        for x in 0..18 {
            let source = TilePos::new(x, 6);
            assert_eq!(
                cache
                    .danger_aware_source_set(&map, &blocked, [source])
                    .footprint_distance(source, (1, 1)),
                Some(0)
            );
            assert!(cache.retained_field_counts().0 <= 9);
        }
    }

    #[test]
    fn routing_cache_reuses_exact_generations_and_keeps_dynamic_fields_bounded() {
        let public_map = briefing(18, 10, [], Vec::new());
        let clear = BlockedGroundLayout::from_predicate(&public_map, |_| false);
        let mut cache = PublicRoutes::default();
        let first = cache.danger_aware_fields(
            &public_map,
            clear.clone(),
            [TilePos::new(3, 3), TilePos::new(14, 7)],
        );
        assert_eq!(cache.build_count().danger_aware, 2);
        assert_eq!(cache.retained_field_counts().0, 2);

        let repeated = cache.danger_aware_fields(
            &public_map,
            clear,
            [TilePos::new(14, 7), TilePos::new(3, 3), TilePos::new(3, 3)],
        );
        assert_eq!(cache.build_count().danger_aware, 2);
        assert_eq!(first.len(), repeated.len());
        assert!(
            first
                .iter()
                .zip(&repeated)
                .all(|(left, right)| { left.0 == right.0 && Arc::ptr_eq(&left.1, &right.1) })
        );

        cache.danger_aware_fields(
            &public_map,
            BlockedGroundLayout::from_predicate(&public_map, |tile| tile == TilePos::new(9, 5)),
            [TilePos::new(14, 7)],
        );
        assert_eq!(cache.build_count().danger_aware, 3);
        assert_eq!(cache.retained_field_counts().0, 1);
    }

    #[test]
    fn row_major_source_retention_reuses_nonmonotone_coordinates() {
        let public_map = briefing(18, 10, [], Vec::new());
        let clear = BlockedGroundLayout::from_predicate(&public_map, |_| false);
        let sources = [TilePos::new(14, 2), TilePos::new(3, 7)];
        let mut cache = PublicRoutes::default();

        cache.danger_aware_fields(&public_map, clear.clone(), sources);
        cache.threat_fields(&public_map, sources);
        let before = cache.build_count();
        cache.danger_aware_fields(&public_map, clear, sources.into_iter().rev());
        cache.threat_fields(&public_map, sources.into_iter().rev());

        assert_eq!(cache.build_count(), before);
        assert_eq!(cache.retained_field_counts(), (2, 2, 0));
    }

    #[test]
    fn dynamic_danger_never_invalidates_terrain_only_threat_or_start_fields() {
        let starts = vec![
            crate::bot::StartingFoundry {
                player: crate::ids::PlayerId(0),
                anchor: TilePos::new(1, 1),
            },
            crate::bot::StartingFoundry {
                player: crate::ids::PlayerId(1),
                anchor: TilePos::new(14, 6),
            },
        ];
        let public_map = briefing(18, 10, [], starts.clone());
        let mut cache = PublicRoutes::default();
        cache.threat_fields(&public_map, [TilePos::new(4, 4), TilePos::new(12, 4)]);
        cache.start_fields(&public_map, &starts);
        let before = cache.build_count();

        let (_, work) = crate::bot::navigation::work::measure(|| {
            for blocked in [TilePos::new(7, 4), TilePos::new(8, 4)] {
                cache.danger_aware_fields(
                    &public_map,
                    BlockedGroundLayout::from_predicate(&public_map, |tile| tile == blocked),
                    [TilePos::new(5, 4)],
                );
                cache.threat_fields(&public_map, [TilePos::new(12, 4), TilePos::new(4, 4)]);
                cache.start_fields(&public_map, &starts[1..]);
            }
        });
        assert_eq!(
            work.fields, 2,
            "only the two changed danger fields need rebuilding: {work:?}"
        );
        assert_eq!(work.generations, 2);
        assert_eq!(work.searches, 2);
        assert!(work.expanded <= 2 * 18 * 10, "{work:?}");

        let after = cache.build_count();
        assert_eq!(after.threats, before.threats);
        assert_eq!(after.starts, before.starts);
        assert_eq!(cache.retained_field_counts(), (1, 2, 2));
    }

    #[test]
    fn routing_cache_replaces_departed_threats_and_resets_for_any_map_change() {
        let public_map = briefing(18, 10, [], Vec::new());
        let mut cache = PublicRoutes::default();
        cache.threat_fields(&public_map, [TilePos::new(3, 3), TilePos::new(9, 3)]);
        cache.threat_fields(&public_map, [TilePos::new(9, 3), TilePos::new(14, 3)]);
        assert_eq!(cache.build_count().threats, 3);
        assert_eq!(cache.retained_field_counts().1, 2);

        let changed_map = briefing(18, 10, [TilePos::new(8, 5)], Vec::new());
        cache.threat_fields(&changed_map, [TilePos::new(9, 3)]);
        assert_eq!(cache.build_count().threats, 4);
        assert_eq!(cache.retained_field_counts(), (0, 1, 0));
    }

    #[test]
    fn controller_field_budget_covers_preparation_and_resumes_after_deferral() {
        use crate::bot::planning::{PlanningWork, Progress};
        let map = briefing(20, 12, (1..10).map(|y| TilePos::new(10, y)), Vec::new());
        let blocked = BlockedGroundLayout::from_predicate(&map, |_| false);
        let sources = [TilePos::new(2, 2)];
        let expected = PublicGroundDistances::from_sources(&map, sources);
        let planning = PlanningWork::with_allowance(60);
        assert_eq!(
            planning.field(0, &map, &blocked, sources),
            Progress::Deferred
        );
        assert_eq!(planning.spent(), 60);
        let saved = planning.clone();
        assert_eq!(
            planning.field(0, &map, &blocked, sources),
            Progress::Deferred
        );
        assert_eq!(
            planning, saved,
            "a repeated call cannot refill the controller allowance"
        );
        let cloned = planning.clone();
        let mut complete = false;
        for tick in (12..120).step_by(12) {
            let actual = planning.field(tick, &map, &blocked, sources);
            assert_eq!(actual, cloned.field(tick, &map, &blocked, sources));
            assert!(planning.spent() <= 60);
            if let Progress::Ready(field) = actual {
                assert_eq!(*field, expected);
                complete = true;
                break;
            }
        }
        assert!(
            complete,
            "the original job must continue beyond its first slice"
        );
        let changed = BlockedGroundLayout::from_predicate(&map, |tile| tile.x == 10);
        assert_eq!(
            planning.field(120, &map, &changed, sources),
            Progress::Deferred
        );
        let expected = PublicGroundDistances::from_sources_avoiding(&map, sources, |tile| {
            changed.contains(tile)
        });
        let mut complete = false;
        for tick in (132..240).step_by(12) {
            if let Progress::Ready(field) = planning.field(tick, &map, &changed, sources) {
                assert_eq!(*field, expected);
                assert_eq!(field.footprint_distance(TilePos::new(17, 2), (1, 1)), None);
                complete = true;
                break;
            }
        }
        assert!(complete);
    }

    #[test]
    fn multiple_field_requests_share_one_controller_allowance() {
        use crate::bot::planning::{PlanningWork, Progress};
        let map = briefing(20, 12, [], Vec::new());
        let blocked = BlockedGroundLayout::from_predicate(&map, |_| false);
        let planning = PlanningWork::with_allowance(60);
        for x in [2, 3, 4] {
            assert_eq!(
                planning.field(24, &map, &blocked, [TilePos::new(x, 2)]),
                Progress::Deferred
            );
            assert_eq!(planning.spent(), 60);
        }
    }

    #[test]
    fn pending_fields_advance_between_admissions_without_reissuing_the_query() {
        use crate::bot::planning::{PlanningWork, Progress};
        let map = briefing(20, 12, [], Vec::new());
        let blocked = BlockedGroundLayout::from_predicate(&map, |_| false);
        let planning = PlanningWork::with_allowance(240);
        for x in [2, 15] {
            assert_eq!(
                planning.field(0, &map, &blocked, [TilePos::new(x, 2)]),
                Progress::Deferred
            );
        }
        let clone = planning.clone();
        for tick in (12..120).step_by(12) {
            planning.begin(tick);
            clone.begin(tick);
            assert_eq!(planning, clone);
            assert!(
                planning.spent() <= 120,
                "background work leaves half for current requests"
            );
            let spent = planning.spent();
            planning.begin(tick);
            assert_eq!(planning.spent(), spent);
            if planning.stats().pending_fields == 0 {
                for x in [2, 15] {
                    let Progress::Ready(field) =
                        planning.field(tick, &map, &blocked, [TilePos::new(x, 2)])
                    else {
                        panic!("completed work must survive until its consumer returns");
                    };
                    assert_eq!(
                        *field,
                        PublicGroundDistances::from_sources(&map, [TilePos::new(x, 2)])
                    );
                }
                assert_eq!(planning.spent(), spent);
                return;
            }
        }
        panic!("both jobs must finish within their lifetime without new admissions");
    }
}
