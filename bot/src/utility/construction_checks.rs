//! Exact construction feasibility over one fixed player-knowledge boundary.

use super::*;
use crate::navigation::paths::{PathQueries, path_cost};
use crate::query_work::QueryPurpose;
use crate::{Orientation, PublicMapBriefing, StartingFoundry};
use oxide_sim::map::Terrain;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

mod resource_assets;
pub(super) mod routing_cache;
pub(super) use resource_assets::{ResourceAssets, ResourceRegion, scrap_assets};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum KnowledgeDomain {
    Ground,
    Air,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PlacementFootprint {
    pub(super) anchor: TilePos,
    pub(super) size: (i32, i32),
    pub(super) blocks_ground: bool,
}

pub(super) struct FutureGroundProducerEgress {
    pub(super) footprint: PlacementFootprint,
    pub(super) witnesses: Vec<TilePos>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BuilderSafetyKey {
    builder: UnitId,
    origin: TilePos,
    anchor: TilePos,
    size: (i32, i32),
    defer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BuilderTravelKey {
    builder: UnitId,
    origin: TilePos,
    placement: PlacementFootprint,
}

pub(super) struct BuilderSafetyContext<'a> {
    pub(super) obs: &'a Observation,
    pub(super) routes: &'a routing::BuildRouteProjection<'a>,
    pub(super) danger: &'a super::danger::HarvestDangerProjection,
    pub(super) orientation: Option<Orientation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ResourceAccessKey {
    placement: PlacementFootprint,
    max_detour: Option<u32>,
}

pub(super) fn cached_safe_implicit_builder(
    policy: &UtilityPolicy,
    context: BuilderSafetyContext<'_>,
    cache: &mut ConstructionCache,
    kind: BuildingKind,
    anchor: TilePos,
    builders: &[&UnitObs],
) -> Option<UnitId> {
    let size = kind.base_stats().size;
    let defer =
        (0..size.1).any(|dy| (0..size.0).any(|dx| !context.obs.visible(anchor.offset(dx, dy))));
    let mut ordered = builders.to_vec();
    ordered.sort_unstable_by_key(|builder| (builder.tile.manhattan(anchor), builder.id));
    ordered.into_iter().find_map(|builder| {
        let key = BuilderSafetyKey {
            builder: builder.id,
            origin: builder.tile,
            anchor,
            size,
            defer,
        };
        let safe = if let Some(safe) = cache.builder_safety.get(&key).copied() {
            #[cfg(test)]
            {
                cache.stats.builder_safety_hits += 1;
            }
            safe
        } else {
            let blocked =
                |tile| policy.harvest_location_contested(tile) || context.danger.contains(tile);
            let safe = context.routes.avoids(
                builder,
                routing::BuildCommandTarget {
                    anchor,
                    size,
                    defer,
                },
                context.orientation,
                blocked,
            );
            cache.builder_safety.insert(key, safe);
            #[cfg(test)]
            {
                cache.stats.builder_safety_builds += 1;
            }
            safe
        };
        safe.then_some(builder.id)
    })
}

pub(super) fn cached_builder_travel_cost(
    grounding: &ConstructionGrounding<'_>,
    cache: &mut ConstructionCache,
    builder: &UnitObs,
    placement: PlacementFootprint,
    orientation: Option<Orientation>,
) -> Option<u32> {
    let key = BuilderTravelKey {
        builder: builder.id,
        origin: builder.tile,
        placement,
    };
    if let Some(cost) = cache.builder_travel.get(&key).copied() {
        #[cfg(test)]
        {
            cache.stats.builder_travel_hits += 1;
        }
        return cost;
    }
    let cost = grounding.builder_travel_cost(builder, placement, orientation);
    cache.builder_travel.insert(key, cost);
    #[cfg(test)]
    {
        cache.stats.builder_travel_builds += 1;
    }
    cost
}

pub(super) fn cached_future_ground_producer_egress_survives(
    grounding: &ConstructionGrounding<'_>,
    orientation: Orientation,
    cache: &mut ConstructionCache,
    placement: PlacementFootprint,
) -> bool {
    if !placement.blocks_ground || grounding.future_ground_producers.is_empty() {
        return true;
    }
    if let Some(survives) = cache.future_producer_egress.get(&placement).copied() {
        #[cfg(test)]
        {
            cache.stats.future_producer_egress_hits += 1;
        }
        return survives;
    }
    let survives = grounding.future_ground_producers.iter().all(|producer| {
        future_ground_producer_keeps_egress(
            &grounding.ground,
            Some(placement),
            Some(orientation),
            producer,
        )
    });
    cache.future_producer_egress.insert(placement, survives);
    #[cfg(test)]
    {
        cache.stats.future_producer_egress_builds += 1;
    }
    survives
}

pub(super) fn cached_resource_access_survives(
    grounding: &ConstructionGrounding<'_>,
    cache: &mut ConstructionCache,
    placement: PlacementFootprint,
    max_detour: Option<u32>,
) -> bool {
    let key = ResourceAccessKey {
        placement,
        max_detour,
    };
    if let Some(survives) = cache.resource_access.get(&key).copied() {
        #[cfg(test)]
        {
            cache.stats.resource_access_hits += 1;
        }
        return survives;
    }
    let survives = scrap_access_survives(
        &grounding.ground,
        grounding.resources.iter().map(|region| &region.access),
        placement,
        max_detour,
    );
    cache.resource_access.insert(key, survives);
    #[cfg(test)]
    {
        cache.stats.resource_access_builds += 1;
    }
    survives
}

impl PlacementFootprint {
    pub(super) fn blocks(self, tile: TilePos) -> bool {
        self.blocks_ground && footprint_contains(self.anchor, self.size, tile)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AccessRoute {
    pub(super) foundry: TilePos,
    pub(super) work_tiles: Vec<TilePos>,
    pub(super) path: Vec<TilePos>,
}

pub(super) struct GroundKnowledge<'a> {
    pub(super) query_purpose: QueryPurpose,
    pub(super) obs: &'a Observation,
    pub(super) briefing: &'a PublicMapBriefing,
    pub(super) air_blocked: Vec<bool>,
    pub(super) ground_blocked: Vec<bool>,
    pub(super) routing: Option<&'a std::cell::RefCell<PathQueries>>,
    pub(super) local_routing: std::cell::RefCell<PathQueries>,
    pub(super) planning: Option<&'a crate::planning::PlanningWork>,
    pub(super) local_planning: crate::planning::PlanningWork,
    pub(super) hypothetical: bool,
    pub(super) scrap: BTreeMap<TilePos, u32>,
}

pub(super) fn tile_index(width: i32, height: i32, tile: TilePos) -> Option<usize> {
    (tile.x >= 0 && tile.y >= 0 && tile.x < width && tile.y < height)
        .then(|| tile.y as usize * width as usize + tile.x as usize)
}

pub(super) fn observed_future_ground_producers(obs: &Observation) -> Vec<PlacementFootprint> {
    let mut producers: Vec<_> = obs
        .my_buildings
        .iter()
        .filter(|building| {
            building.hp > 0
                && !building.built
                && building
                    .kind
                    .tier_stats(building.tier)
                    .produces
                    .iter()
                    .any(|unit| unit.stats().domain == Domain::Ground)
        })
        .map(|building| PlacementFootprint {
            anchor: building.anchor,
            size: building.kind.tier_stats(building.tier).size,
            blocks_ground: !building.kind.is_stealthy(),
        })
        .chain(obs.my_units.iter().filter_map(|unit| {
            let (kind, anchor) = unit.founding?;
            kind.base_stats()
                .produces
                .iter()
                .any(|unit| unit.stats().domain == Domain::Ground)
                .then_some(PlacementFootprint {
                    anchor,
                    size: kind.base_stats().size,
                    blocks_ground: !kind.is_stealthy(),
                })
        }))
        .collect();
    producers.sort_unstable();
    producers.dedup();
    producers
}

pub(super) fn future_ground_producers_keep_egress(
    baseline: &GroundKnowledge<'_>,
    combined: &GroundKnowledge<'_>,
    orientation: Option<Orientation>,
    builds: &[(BuildingKind, TilePos)],
) -> bool {
    let mut producers = observed_future_ground_producers(baseline.obs);
    producers.extend(builds.iter().copied().filter_map(|(kind, anchor)| {
        kind.base_stats()
            .produces
            .iter()
            .any(|unit| unit.stats().domain == Domain::Ground)
            .then_some(PlacementFootprint {
                anchor,
                size: kind.base_stats().size,
                blocks_ground: !kind.is_stealthy(),
            })
    }));
    producers.sort_unstable();
    producers.dedup();
    producers.into_iter().all(|footprint| {
        let producer = future_ground_producer_egress_certificate(baseline, orientation, footprint);
        future_ground_producer_keeps_egress(combined, None, orientation, &producer)
    })
}

pub(super) fn planned_ground_producer_spawn_doorstep(
    ground: &GroundKnowledge<'_>,
    footprint: PlacementFootprint,
    candidate: Option<PlacementFootprint>,
    orientation: Option<Orientation>,
) -> Option<TilePos> {
    routing::production_spawn_doorstep_for_open_tiles(
        (ground.obs.map_width, ground.obs.map_height),
        footprint.anchor,
        footprint.size,
        orientation,
        |policy_tile| ground.open(policy_tile, candidate, KnowledgeDomain::Ground),
    )
}

pub(super) fn future_ground_producer_egress_certificate(
    baseline: &GroundKnowledge<'_>,
    orientation: Option<Orientation>,
    footprint: PlacementFootprint,
) -> FutureGroundProducerEgress {
    let Some(start) =
        planned_ground_producer_spawn_doorstep(baseline, footprint, Some(footprint), orientation)
    else {
        return FutureGroundProducerEgress {
            footprint,
            witnesses: Vec::new(),
        };
    };

    let width = baseline.obs.map_width;
    let height = baseline.obs.map_height;
    let reachable = crate::navigation::flood::component(width, height, start, |tile| {
        baseline.open(tile, Some(footprint), KnowledgeDomain::Ground)
    })
    .expect("an open doorstep is in bounds");
    let mut witnesses: Vec<_> = reachable
        .iter()
        .enumerate()
        .filter(|(_, reachable)| **reachable)
        .map(|(index, _)| TilePos::new(index as i32 % width, index as i32 / width))
        .collect();
    witnesses
        .sort_unstable_by_key(|tile| (Reverse(tile.chebyshev(footprint.anchor)), tile.y, tile.x));
    FutureGroundProducerEgress {
        footprint,
        witnesses,
    }
}

pub(super) fn future_ground_producer_keeps_egress(
    combined: &GroundKnowledge<'_>,
    combined_candidate: Option<PlacementFootprint>,
    orientation: Option<Orientation>,
    producer: &FutureGroundProducerEgress,
) -> bool {
    let Some(witness) = producer
        .witnesses
        .iter()
        .copied()
        .find(|tile| combined.open(*tile, combined_candidate, KnowledgeDomain::Ground))
    else {
        return false;
    };
    let Some(spawn) = planned_ground_producer_spawn_doorstep(
        combined,
        producer.footprint,
        combined_candidate,
        orientation,
    ) else {
        return false;
    };
    crate::navigation::search::reachable(
        QueryPurpose::ConstructionExitSafety,
        combined.obs.map_width,
        combined.obs.map_height,
        spawn,
        witness,
        |tile| combined.open(tile, combined_candidate, KnowledgeDomain::Ground),
    )
}

pub(super) fn shortest_path_between(
    ground: &GroundKnowledge<'_>,
    starts: &[TilePos],
    goals: &[TilePos],
    candidate: Option<PlacementFootprint>,
    domain: KnowledgeDomain,
) -> Option<(TilePos, TilePos, Vec<TilePos>)> {
    crate::navigation::paths::shortest_path_between(
        routing_cache::board(ground, domain),
        starts,
        goals,
        candidate.and_then(|candidate| routing_cache::overlay(candidate, domain)),
    )
}

pub(super) fn candidate_affects_path(
    path: &[TilePos],
    candidate: PlacementFootprint,
    domain: KnowledgeDomain,
) -> bool {
    routing_cache::overlay(candidate, domain).is_some_and(|overlay| overlay.affects_path(path))
}

pub(super) fn scrap_work_tiles(
    ground: &GroundKnowledge<'_>,
    cluster: &[TilePos],
    candidate: Option<PlacementFootprint>,
) -> Vec<TilePos> {
    let cluster: BTreeSet<_> = cluster.iter().copied().collect();
    let mut work_tiles = Vec::new();
    for tile in &cluster {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let neighbor = tile.offset(dx, dy);
                if !cluster.contains(&neighbor)
                    && ground.open(neighbor, candidate, KnowledgeDomain::Ground)
                {
                    work_tiles.push(neighbor);
                }
            }
        }
    }
    sorted_tiles(work_tiles)
}

pub(super) fn building_doorsteps(
    ground: &GroundKnowledge<'_>,
    anchor: TilePos,
    size: (i32, i32),
) -> Vec<TilePos> {
    sorted_tiles(
        oxide_sim::geometry::rect_adjacent_tiles(anchor, size)
            .filter(|tile| ground.open(*tile, None, KnowledgeDomain::Ground)),
    )
}

pub(super) fn scrap_access_survives<'a>(
    ground: &GroundKnowledge<'_>,
    mut access_routes: impl Iterator<Item = &'a AccessRoute>,
    candidate: PlacementFootprint,
    max_detour: Option<u32>,
) -> bool {
    if !candidate.blocks_ground {
        return true;
    }
    access_routes.all(|access| {
        if !candidate_affects_path(&access.path, candidate, KnowledgeDomain::Ground) {
            return true;
        }
        shortest_path_between(
            ground,
            &building_doorsteps(
                ground,
                access.foundry,
                BuildingKind::Foundry.base_stats().size,
            ),
            &access.work_tiles,
            Some(candidate),
            KnowledgeDomain::Ground,
        )
        .is_some_and(|(_, _, path)| {
            let detour = path_cost(&path).saturating_sub(path_cost(&access.path));
            max_detour.is_none_or(|limit| detour <= limit)
        })
    })
}

pub(super) fn footprint_contains(anchor: TilePos, size: (i32, i32), tile: TilePos) -> bool {
    tile.x >= anchor.x
        && tile.x < anchor.x + size.0
        && tile.y >= anchor.y
        && tile.y < anchor.y + size.1
}

pub(super) fn footprint_tiles(anchor: TilePos, size: (i32, i32)) -> Vec<TilePos> {
    (0..size.1)
        .flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
        .collect()
}

pub(super) fn sorted_tiles(tiles: impl IntoIterator<Item = TilePos>) -> Vec<TilePos> {
    let mut tiles: Vec<_> = tiles.into_iter().collect();
    tiles.sort_by_key(|tile| (tile.y, tile.x));
    tiles.dedup();
    tiles
}

pub(super) struct ConstructionGrounding<'a> {
    pub(super) public_starts: Vec<StartingFoundry>,
    pub(super) ground: GroundKnowledge<'a>,
    pub(super) build_routes: routing::BuildRouteProjection<'a>,
    pub(super) placement: super::terrain::PlacementGeometry<'a>,
    pub(super) resources: Vec<ResourceRegion>,
    pub(super) future_ground_producers: Vec<FutureGroundProducerEgress>,
}
impl<'a> ConstructionGrounding<'a> {
    pub(super) fn new(
        query_purpose: QueryPurpose,
        policy: &'a UtilityPolicy,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        orientation: Option<Orientation>,
    ) -> Self {
        let public_starts = policy.uncleared_hostile_starts(briefing, obs.me);
        let ground = GroundKnowledge::new(query_purpose, obs, briefing, &public_starts)
            .retained(policy, false);
        let resources = ground.resource_assets(policy);
        let future_ground_producers = orientation.map_or_else(Vec::new, |orientation| {
            observed_future_ground_producers(obs)
                .into_iter()
                .map(|footprint| {
                    future_ground_producer_egress_certificate(&ground, Some(orientation), footprint)
                })
                .collect()
        });
        Self {
            public_starts,
            ground,
            resources,
            future_ground_producers,
            build_routes: routing::BuildRouteProjection::new(query_purpose, obs, Some(briefing)),
            placement: super::terrain::PlacementGeometry::new(obs),
        }
    }
    pub(super) fn builder_travel_cost(
        &self,
        builder: &UnitObs,
        placement: PlacementFootprint,
        orientation: Option<Orientation>,
    ) -> Option<u32> {
        let defer = (0..placement.size.1).any(|dy| {
            (0..placement.size.0)
                .any(|dx| !self.ground.obs.visible(placement.anchor.offset(dx, dy)))
        });
        self.build_routes.cost(
            builder,
            routing::BuildCommandTarget {
                anchor: placement.anchor,
                size: placement.size,
                defer,
            },
            orientation,
        )
    }
}

#[derive(Default)]
pub(super) struct ConstructionCache {
    builder_safety: BTreeMap<BuilderSafetyKey, bool>,
    builder_travel: BTreeMap<BuilderTravelKey, Option<u32>>,
    future_producer_egress: BTreeMap<PlacementFootprint, bool>,
    resource_access: BTreeMap<ResourceAccessKey, bool>,
    #[cfg(test)]
    pub(super) stats: ConstructionCacheStats,
}
#[cfg(test)]
#[derive(Default, Clone, Copy)]
pub(super) struct ConstructionCacheStats {
    pub(super) builder_safety_builds: usize,
    pub(super) builder_safety_hits: usize,
    pub(super) builder_travel_builds: usize,
    pub(super) builder_travel_hits: usize,
    pub(super) future_producer_egress_builds: usize,
    pub(super) future_producer_egress_hits: usize,
    pub(super) resource_access_builds: usize,
    pub(super) resource_access_hits: usize,
}

/// A quotation pass holds policy-derived danger and contested-work memory fixed.
/// Eligible builders remain an explicit input to each query.
pub(super) struct ConstructionChecks<'a> {
    policy: &'a UtilityPolicy,
    grounding: ConstructionGrounding<'a>,
    danger: Arc<super::danger::HarvestDangerProjection>,
    orientation: Orientation,
    cache: ConstructionCache,
}
impl<'a> ConstructionChecks<'a> {
    pub(super) fn new(
        query_purpose: QueryPurpose,
        policy: &'a UtilityPolicy,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        unit_contacts: &'a [UnitContact],
        building_contacts: &'a [BuildingContact],
        orientation: Orientation,
    ) -> Self {
        Self {
            policy,
            grounding: ConstructionGrounding::new(
                query_purpose,
                policy,
                obs,
                briefing,
                Some(orientation),
            ),
            danger: policy.harvest_danger_projection(
                obs,
                Some(unit_contacts),
                Some(building_contacts),
            ),
            orientation,
            cache: ConstructionCache::default(),
        }
    }
    pub(super) fn resource_access_survives(&mut self, kind: BuildingKind, anchor: TilePos) -> bool {
        cached_resource_access_survives(
            &self.grounding,
            &mut self.cache,
            PlacementFootprint::new(kind, anchor),
            None,
        )
    }
    pub(super) fn future_ground_producer_egress_survives(
        &mut self,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> bool {
        cached_future_ground_producer_egress_survives(
            &self.grounding,
            self.orientation,
            &mut self.cache,
            PlacementFootprint::new(kind, anchor),
        )
    }
    pub(super) fn safe_implicit_builder(
        &mut self,
        kind: BuildingKind,
        anchor: TilePos,
        builders: &[&UnitObs],
    ) -> Option<UnitId> {
        cached_safe_implicit_builder(
            self.policy,
            BuilderSafetyContext {
                obs: self.grounding.ground.obs,
                routes: &self.grounding.build_routes,
                danger: &self.danger,
                orientation: Some(self.orientation),
            },
            &mut self.cache,
            kind,
            anchor,
            builders,
        )
    }
    pub(super) fn builder_travel_cost(
        &mut self,
        builder: &UnitObs,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> Option<u32> {
        cached_builder_travel_cost(
            &self.grounding,
            &mut self.cache,
            builder,
            PlacementFootprint::new(kind, anchor),
            Some(self.orientation),
        )
    }
}
impl PlacementFootprint {
    pub(super) fn new(kind: BuildingKind, anchor: TilePos) -> Self {
        Self {
            anchor,
            size: kind.base_stats().size,
            blocks_ground: !kind.is_stealthy(),
        }
    }
}

impl<'a> GroundKnowledge<'a> {
    pub(super) fn resource_assets(&self, policy: &UtilityPolicy) -> Vec<ResourceRegion> {
        let foundries = self
            .obs
            .my_buildings
            .iter()
            .filter(|building| {
                building.hp > 0 && building.built && building.kind == BuildingKind::Foundry
            })
            .map(|building| building.anchor)
            .collect::<Vec<_>>();
        scrap_assets(policy, self, &foundries)
    }

    pub(super) fn retained(mut self, policy: &'a UtilityPolicy, hypothetical: bool) -> Self {
        self.routing = Some(&policy.queries.knowledge_paths);
        self.planning = Some(&policy.planning);
        self.hypothetical = hypothetical;
        self
    }

    pub(super) fn planning(&self) -> &crate::planning::PlanningWork {
        self.planning.unwrap_or(&self.local_planning)
    }

    pub(super) fn routing(&self) -> &std::cell::RefCell<PathQueries> {
        self.routing.unwrap_or(&self.local_routing)
    }

    pub(super) fn new(
        query_purpose: QueryPurpose,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        public_starts: &[StartingFoundry],
    ) -> Self {
        let mut scrap: BTreeMap<_, _> = briefing.initial_scrap().iter().copied().collect();
        for tile in sorted_tiles(scrap.keys().copied()) {
            if obs.explored(tile) {
                let amount = obs
                    .known_scrap
                    .binary_search_by_key(&(tile.y, tile.x), |(known, _)| (known.y, known.x))
                    .ok()
                    .map_or(0, |index| obs.known_scrap[index].1);
                if amount == 0 {
                    scrap.remove(&tile);
                } else {
                    scrap.insert(tile, amount);
                }
            }
        }
        for (tile, amount) in &obs.known_scrap {
            if *amount > 0 {
                scrap.insert(*tile, *amount);
            }
        }
        let area = usize::try_from(obs.map_width)
            .ok()
            .and_then(|width| {
                usize::try_from(obs.map_height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let mut terrain = vec![Terrain::Ground; area];
        for (tile, authored) in briefing.non_ground_terrain() {
            if let Some(index) = tile_index(obs.map_width, obs.map_height, *tile) {
                terrain[index] = *authored;
            }
        }
        let mut ground_blocked: Vec<_> = terrain
            .iter()
            .map(|terrain| terrain.blocks_ground())
            .collect();
        for tile in scrap.keys().copied() {
            if let Some(index) = tile_index(obs.map_width, obs.map_height, tile) {
                ground_blocked[index] = true;
            }
        }
        for (anchor, size) in obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .chain(obs.enemy_buildings.iter())
            .filter(|building| building.hp > 0 && !building.kind.is_stealthy())
            .map(|building| {
                (
                    building.anchor,
                    building.kind.tier_stats(building.tier).size,
                )
            })
            .chain(obs.my_units.iter().filter_map(|unit| {
                let (kind, anchor) = unit.founding?;
                (!kind.is_stealthy()).then_some((anchor, kind.base_stats().size))
            }))
            .chain(
                public_starts
                    .iter()
                    .map(|start| (start.anchor, BuildingKind::Foundry.base_stats().size)),
            )
        {
            for tile in footprint_tiles(anchor, size) {
                if let Some(index) = tile_index(obs.map_width, obs.map_height, tile) {
                    ground_blocked[index] = true;
                }
            }
        }
        Self {
            query_purpose,
            obs,
            briefing,
            air_blocked: terrain.iter().map(|terrain| terrain.blocks_air()).collect(),
            ground_blocked,
            routing: None,
            local_routing: Default::default(),
            planning: None,
            local_planning: Default::default(),
            hypothetical: false,
            scrap,
        }
    }

    pub(super) fn open(
        &self,
        tile: TilePos,
        candidate: Option<PlacementFootprint>,
        domain: KnowledgeDomain,
    ) -> bool {
        let Some(index) = tile_index(self.obs.map_width, self.obs.map_height, tile) else {
            return false;
        };
        match domain {
            KnowledgeDomain::Ground => {
                !self.ground_blocked[index]
                    && candidate.is_none_or(|placement| !placement.blocks(tile))
            }
            KnowledgeDomain::Air => !self.air_blocked[index],
        }
    }
}

#[derive(Clone, Copy)]
struct CombinedLayoutContext<'a> {
    obs: &'a Observation,
    briefing: &'a PublicMapBriefing,
    unit_contacts: &'a [UnitContact],
    building_contacts: &'a [BuildingContact],
    orientation: Option<Orientation>,
}

impl UtilityPolicy {
    /// A frozen blocking foundation cannot cover any selected builder's start.
    /// This is a necessary layout condition, independent of route searches.
    pub(crate) fn build_layout_covers_assigned_builder(
        obs: &Observation,
        builds: &[(BuildingKind, TilePos, UnitId)],
    ) -> bool {
        builds.iter().any(|(_, _, builder)| {
            obs.my_units
                .iter()
                .find(|unit| unit.id == *builder)
                .is_some_and(|unit| {
                    builds.iter().any(|(kind, anchor, _)| {
                        let size = kind.base_stats().size;
                        !kind.is_stealthy()
                            && (0..i64::from(size.0))
                                .contains(&(i64::from(unit.tile.x) - i64::from(anchor.x)))
                            && (0..i64::from(size.1))
                                .contains(&(i64::from(unit.tile.y) - i64::from(anchor.y)))
                    })
                })
        })
    }

    /// Verifies a frozen set of construction footprints as one layout, without
    /// reranking any proposal. This is the pairwise allocator preflight for
    /// independently selected Foundry and defense opportunities.
    #[cfg(test)]
    pub(crate) fn combined_build_layout_is_safe(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        builds: &[(BuildingKind, TilePos)],
    ) -> bool {
        self.combined_build_layout_is_safe_inner(
            CombinedLayoutContext {
                obs,
                briefing,
                unit_contacts: &[],
                building_contacts: &[],
                orientation: None,
            },
            builds,
            &[],
        )
    }

    /// The same combined-layout preflight, additionally preserving each exact
    /// selected builder's route to its own footprint.
    pub(crate) fn combined_build_layout_with_builders_is_safe(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        orientation: Orientation,
        builds: &[(BuildingKind, TilePos, UnitId)],
    ) -> bool {
        if Self::build_layout_covers_assigned_builder(obs, builds) {
            return false;
        }
        let footprints: Vec<_> = builds
            .iter()
            .map(|(kind, anchor, _)| (*kind, *anchor))
            .collect();
        self.combined_build_layout_is_safe_inner(
            CombinedLayoutContext {
                obs,
                briefing,
                unit_contacts,
                building_contacts,
                orientation: Some(orientation),
            },
            &footprints,
            builds,
        )
    }

    fn combined_build_layout_is_safe_inner(
        &self,
        context: CombinedLayoutContext<'_>,
        builds: &[(BuildingKind, TilePos)],
        exact_builders: &[(BuildingKind, TilePos, UnitId)],
    ) -> bool {
        let CombinedLayoutContext {
            obs,
            briefing,
            unit_contacts,
            building_contacts,
            orientation,
        } = context;
        if builds.is_empty() {
            return true;
        }
        self.prepare_ground_producer_egress(obs);
        if builds.iter().any(|(kind, anchor)| {
            kind.base_stats().construction.is_none()
                || !self.placement_valid_prepared(
                    obs,
                    *kind,
                    *anchor,
                    FoundationCancellations::default(),
                )
        }) {
            return false;
        }
        let sites: Vec<_> = builds
            .iter()
            .map(|(kind, anchor)| PlacementFootprint {
                anchor: *anchor,
                size: kind.base_stats().size,
                blocks_ground: !kind.is_stealthy(),
            })
            .collect();
        if sites.iter().enumerate().any(|(index, site)| {
            sites
                .iter()
                .skip(index + 1)
                .any(|other| footprints_overlap(*site, *other))
        }) {
            return false;
        }
        let blocking_builds: Vec<_> = builds
            .iter()
            .copied()
            .filter(|(kind, _)| !kind.is_stealthy())
            .collect();
        if let Some((candidate, accepted)) = blocking_builds.split_last()
            && !self.preserves_ground_producer_egress_prepared(accepted, *candidate)
        {
            return false;
        }

        let public_starts = self.uncleared_hostile_starts(briefing, obs.me);
        let baseline_ground = GroundKnowledge::new(
            crate::query_work::QueryPurpose::ConstructionAccess,
            obs,
            briefing,
            &public_starts,
        )
        .retained(self, false);
        let assets = baseline_ground.resource_assets(self);
        let mut combined_ground = GroundKnowledge::new(
            crate::query_work::QueryPurpose::ConstructionAccess,
            obs,
            briefing,
            &public_starts,
        )
        .retained(self, true);
        for site in &sites {
            if !site.blocks_ground {
                continue;
            }
            for tile in footprint_tiles(site.anchor, site.size) {
                let Some(index) = tile_index(obs.map_width, obs.map_height, tile) else {
                    return false;
                };
                combined_ground.ground_blocked[index] = true;
            }
        }
        if !future_ground_producers_keep_egress(
            &baseline_ground,
            &combined_ground,
            orientation,
            builds,
        ) {
            return false;
        }
        if assets.iter().any(|asset| {
            let access = &asset.access;
            shortest_path_between(
                &combined_ground,
                &building_doorsteps(
                    &combined_ground,
                    access.foundry,
                    BuildingKind::Foundry.base_stats().size,
                ),
                &access.work_tiles,
                None,
                KnowledgeDomain::Ground,
            )
            .is_none()
        }) {
            return false;
        }
        if !exact_builders.is_empty() && orientation.is_none() {
            return false;
        }
        let danger =
            self.harvest_danger_projection(obs, Some(unit_contacts), Some(building_contacts));
        exact_builders.iter().all(|(kind, anchor, builder)| {
            let Some(builder) = obs
                .my_units
                .iter()
                .find(|candidate| candidate.id == *builder)
            else {
                return false;
            };
            if !combined_ground.open(builder.tile, None, KnowledgeDomain::Ground) {
                return false;
            }
            let size = kind.base_stats().size;
            let defer =
                (0..size.1).any(|dy| (0..size.0).any(|dx| !obs.visible(anchor.offset(dx, dy))));
            let blocked = |tile| {
                (*kind == BuildingKind::Foundry && !obs.explored(tile))
                    || self.harvest_location_contested(tile)
                    || danger.contains(tile)
            };
            if blocked(builder.tile) {
                return false;
            }
            routing::BuildRouteProjection::new(
                QueryPurpose::ConstructionAccess,
                obs,
                Some(briefing),
            )
            .avoids_with_blockers(
                builder,
                routing::BuildCommandTarget {
                    anchor: *anchor,
                    size,
                    defer,
                },
                Some(
                    orientation
                        .expect("exact combined builder checks require a command orientation"),
                ),
                |tile| {
                    sites.iter().any(|site| {
                        site.blocks_ground
                            && (site.anchor != *anchor || site.size != size)
                            && site.blocks(tile)
                    })
                },
                blocked,
            )
        })
    }
}

fn footprints_overlap(first: PlacementFootprint, second: PlacementFootprint) -> bool {
    first.anchor.x < second.anchor.x + second.size.0
        && second.anchor.x < first.anchor.x + first.size.0
        && first.anchor.y < second.anchor.y + second.size.1
        && second.anchor.y < first.anchor.y + first.size.1
}

#[cfg(test)]
mod tests;
