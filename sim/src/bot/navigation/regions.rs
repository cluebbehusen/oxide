//! Public-terrain connectivity and coarse travel estimates for strategic ranking.

use crate::bot::PublicMapBriefing;
#[cfg(test)]
use crate::bot::observation::ObservationData;
use chassis::grid::TilePos;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::sync::Arc;

const REGION_SIDE: i32 = 16;
const ABSENT: usize = usize::MAX;
const STEPS: [(i32, i32); 8] = [
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
    (1, 1),
];

#[derive(Debug, PartialEq, Eq)]
pub(in crate::bot) struct StaticRegions {
    width: i32,
    height: i32,
    labels: Vec<usize>,
    anchors: Vec<TilePos>,
    edges: Vec<Vec<(usize, u32)>>,
    components: Vec<usize>,
}

impl StaticRegions {
    pub(in crate::bot) fn region_at(&self, tile: TilePos) -> Option<usize> {
        self.index(tile)
            .map(|index| self.labels[index])
            .filter(|region| *region != ABSENT)
    }

    pub(in crate::bot) fn connects(&self, starts: &[TilePos], goals: &[TilePos]) -> bool {
        starts
            .iter()
            .filter_map(|&tile| self.region_at(tile))
            .any(|start| {
                goals
                    .iter()
                    .filter_map(|&tile| self.region_at(tile))
                    .any(|goal| self.components[start] == self.components[goal])
            })
    }

    pub(in crate::bot) fn clusters(
        &self,
        points: impl IntoIterator<Item = TilePos>,
    ) -> Vec<Vec<TilePos>> {
        let mut clusters = BTreeMap::<usize, Vec<TilePos>>::new();
        for point in points {
            let Some(region) = self
                .index(point)
                .map(|index| self.labels[index])
                .filter(|&region| region != ABSENT)
            else {
                continue;
            };
            clusters.entry(region).or_default().push(point);
        }
        clusters
            .into_values()
            .map(|mut points| {
                points.sort_unstable_by_key(|point| (point.y, point.x));
                points.dedup();
                points
            })
            .collect()
    }

    pub(in crate::bot) fn build(map: &PublicMapBriefing) -> Self {
        let width = map.map_width();
        let height = map.map_height();
        let cells = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
            .unwrap_or(0);
        let mut result = Self {
            width,
            height,
            labels: vec![ABSENT; cells],
            anchors: Vec::new(),
            edges: Vec::new(),
            components: Vec::new(),
        };
        let open: Vec<_> = (0..cells)
            .map(|index| map.terrain_at(result.tile(index)) == Some(crate::map::Terrain::Ground))
            .collect();
        let mut frontier = VecDeque::new();
        for index in 0..cells {
            if !open[index] || result.labels[index] != ABSENT {
                continue;
            }
            let anchor = result.tile(index);
            let region = result.anchors.len();
            result.anchors.push(anchor);
            result.labels[index] = region;
            frontier.push_back(anchor);
            while let Some(tile) = frontier.pop_front() {
                for (dx, dy) in STEPS {
                    let next = tile.offset(dx, dy);
                    if next.x.div_euclid(REGION_SIDE) != anchor.x / REGION_SIDE
                        || next.y.div_euclid(REGION_SIDE) != anchor.y / REGION_SIDE
                        || !result.step_open(tile, next, &open)
                    {
                        continue;
                    }
                    let next_index = result.index(next).expect("open tile is in bounds");
                    if result.labels[next_index] == ABSENT {
                        result.labels[next_index] = region;
                        frontier.push_back(next);
                    }
                }
            }
        }
        let mut edges = vec![BTreeMap::<usize, u32>::new(); result.anchors.len()];
        for index in 0..cells {
            let region = result.labels[index];
            if region == ABSENT {
                continue;
            }
            let tile = result.tile(index);
            for (dx, dy) in STEPS {
                let next = tile.offset(dx, dy);
                if !result.step_open(tile, next, &open) {
                    continue;
                }
                let other = result.labels[result.index(next).expect("open tile is in bounds")];
                if region == other {
                    continue;
                }
                let cost = super::octile(result.anchors[region], tile)
                    .saturating_add(super::octile(tile, next))
                    .saturating_add(super::octile(next, result.anchors[other]));
                edges[region]
                    .entry(other)
                    .and_modify(|prior| *prior = (*prior).min(cost))
                    .or_insert(cost);
            }
        }
        result.edges = edges
            .into_iter()
            .map(|edges| edges.into_iter().collect())
            .collect();
        result.components = vec![ABSENT; result.anchors.len()];
        let mut pending = Vec::new();
        for origin in 0..result.anchors.len() {
            if result.components[origin] != ABSENT {
                continue;
            }
            result.components[origin] = origin;
            pending.push(origin);
            while let Some(region) = pending.pop() {
                for &(neighbor, _) in &result.edges[region] {
                    if result.components[neighbor] == ABSENT {
                        result.components[neighbor] = origin;
                        pending.push(neighbor);
                    }
                }
            }
        }
        result
    }

    fn step_open(&self, from: TilePos, to: TilePos, open: &[bool]) -> bool {
        let is_open = |tile| self.index(tile).is_some_and(|index| open[index]);
        is_open(to)
            && (from.x == to.x
                || from.y == to.y
                || (is_open(TilePos::new(from.x, to.y)) && is_open(TilePos::new(to.x, from.y))))
    }

    fn index(&self, tile: TilePos) -> Option<usize> {
        ((0..self.width).contains(&tile.x) && (0..self.height).contains(&tile.y))
            .then(|| tile.y as usize * self.width as usize + tile.x as usize)
    }

    fn tile(&self, index: usize) -> TilePos {
        TilePos::new(
            (index % self.width as usize) as i32,
            (index / self.width as usize) as i32,
        )
    }

    pub(in crate::bot) fn distances(self: &Arc<Self>, source: TilePos) -> RegionalDistances {
        let mut costs = vec![u32::MAX; self.anchors.len()];
        let mut frontier = BinaryHeap::new();
        let origin = self
            .index(source)
            .map_or(ABSENT, |index| self.labels[index]);
        if origin != ABSENT {
            costs[origin] = super::octile(source, self.anchors[origin]);
            frontier.push(Reverse((costs[origin], origin)));
        }
        while let Some(Reverse((cost, region))) = frontier.pop() {
            if costs[region] != cost {
                continue;
            }
            for &(next, edge) in &self.edges[region] {
                let candidate = cost.saturating_add(edge);
                if candidate < costs[next] {
                    costs[next] = candidate;
                    frontier.push(Reverse((candidate, next)));
                }
            }
        }
        RegionalDistances {
            regions: Arc::clone(self),
            source,
            origin,
            costs,
        }
    }
}

pub(in crate::bot) struct RegionalDistances {
    regions: Arc<StaticRegions>,
    source: TilePos,
    origin: usize,
    costs: Vec<u32>,
}

impl RegionalDistances {
    /// A ranking estimate, not a route, travel-time bound, or safety certificate.
    pub(in crate::bot) fn estimate(&self, target: TilePos) -> Option<u32> {
        let region = self.regions.labels[self.regions.index(target)?];
        if region == ABSENT || self.origin == ABSENT || self.costs[region] == u32::MAX {
            return None;
        }
        if region == self.origin {
            return Some(super::octile(self.source, target));
        }
        Some(self.costs[region].saturating_add(super::octile(self.regions.anchors[region], target)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_connectivity_matches_cardinal_floods_across_terrain_masks() {
        for mask in 0..512 {
            let mut briefing =
                PublicMapBriefing::from_scenario(&crate::Scenario::skirmish()).unwrap();
            briefing.map_width = 3;
            briefing.map_height = 3;
            briefing.non_ground_terrain = (0..9)
                .filter(|i| mask & (1 << i) != 0)
                .map(|i| (TilePos::new(i % 3, i / 3), crate::map::Terrain::Rock))
                .collect();
            let regions = briefing.regions();
            for start in (0..9).map(|i| TilePos::new(i % 3, i / 3)) {
                for goal in (0..9).map(|i| TilePos::new(i % 3, i / 3)) {
                    let expected = briefing.terrain_at(start) == Some(crate::map::Terrain::Ground)
                        && super::super::flood::reaches_any(
                            3,
                            3,
                            [start],
                            |tile| briefing.terrain_at(tile) == Some(crate::map::Terrain::Ground),
                            |tile| tile == goal,
                        );
                    assert_eq!(
                        regions.connects(&[start], &[goal]),
                        expected,
                        "mask={mask}, {start:?} -> {goal:?}"
                    );
                }
            }
            assert!(!regions.connects(&[], &[TilePos::new(0, 0)]));
            assert!(!regions.connects(&[TilePos::new(-1, 0)], &[TilePos::new(0, 0)]));
        }
    }

    #[test]
    fn regions_preserve_local_separation_and_cross_boundary_connectivity() {
        let mut scenario = crate::Scenario::skirmish();
        scenario.map = (0..24).map(|_| ".".repeat(40)).collect();
        scenario.map[0].replace_range(0..1, "1");
        scenario.map[23].replace_range(39..40, "2");
        for row in &mut scenario.map {
            row.replace_range(8..9, "#");
        }
        let briefing = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let regions = briefing.regions();
        assert_ne!(regions.labels[7], regions.labels[9]);
        assert!(!regions.connects(&[TilePos::new(2, 2)], &[TilePos::new(10, 2)]));
        assert!(regions.connects(&[TilePos::new(10, 2)], &[TilePos::new(35, 20)]));
        let left = regions.distances(TilePos::new(2, 2));
        assert!(left.estimate(TilePos::new(7, 21)).is_some());
        assert!(left.estimate(TilePos::new(9, 2)).is_none());
        let right = regions.distances(TilePos::new(10, 2));
        assert!(right.estimate(TilePos::new(38, 22)).is_some());
        assert!(right.estimate(TilePos::new(40, 22)).is_none());
        assert!(regions.anchors.len() < regions.labels.len() / 16);
    }

    #[test]
    fn diagonal_corner_gaps_do_not_connect_regions() {
        let mut scenario = crate::Scenario::skirmish();
        scenario.map = vec!["1#..".into(), "#...".into(), "....".into(), "...2".into()];
        let briefing = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let distances = briefing.regions().distances(TilePos::new(0, 0));
        assert!(distances.estimate(TilePos::new(1, 1)).is_none());
    }

    #[test]
    fn warmed_briefings_share_storage_but_changed_terrain_cannot_reuse_it() {
        let scenario = crate::Scenario::skirmish();
        let briefing = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let cold_identity = briefing.clone();
        let first = briefing.regions();
        assert_eq!(cold_identity, briefing);
        assert!(Arc::ptr_eq(&first, &cold_identity.regions()));
        let mut changed = briefing.clone();
        changed.map_width += 1;
        assert!(!Arc::ptr_eq(&first, &changed.regions()));
        let orientation = crate::bot::orient::Orientation::for_home(
            &crate::bot::Observation::from_data(ObservationData {
                map_width: briefing.map_width(),
                map_height: briefing.map_height(),
                ..Default::default()
            }),
            TilePos::new(briefing.map_width() - 1, briefing.map_height() - 1),
        );
        let rotated = orientation.briefing(&briefing);
        assert!(!Arc::ptr_eq(&first, &rotated.regions()));
        let restored = orientation.briefing(&rotated);
        assert_eq!(restored, briefing);
        assert_eq!(restored.regions(), first);
    }
}
