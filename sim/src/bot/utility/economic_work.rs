//! Safe finite work shared by worker and capital investment quotes.

use super::economic_value::{
    HarvestWork, WorkerService, harvest_output, marginal_worker_return, travel_ticks,
};
use super::*;
use crate::bot::navigation::public_fields::PublicGroundDistances;
use crate::bot::query_work::QueryPurpose;

pub(super) struct HarvestRegion {
    pub(super) service: TilePos,
    pub(super) work: HarvestWork,
    pub(super) workers: Vec<WorkerService>,
    distances: PublicGroundDistances,
    doors: Vec<TilePos>,
    producer_access: std::collections::BTreeMap<BuildingId, u32>,
}

impl HarvestRegion {
    pub(super) fn current_output(&self, horizon: u64) -> u64 {
        harvest_output(self.work, &self.workers, horizon)
    }

    pub(super) fn marginal(&self, worker: WorkerService, horizon: u64) -> u64 {
        marginal_worker_return(self.work, &self.workers, worker, horizon)
    }

    pub(super) fn distance(&self, tile: TilePos) -> Option<u32> {
        self.distances.footprint_distance(tile, (1, 1))
    }

    pub(super) fn producer_distance(&self, producer: BuildingId) -> Option<u32> {
        self.producer_access.get(&producer).copied()
    }

    fn harvest_corridor(&self, tile: TilePos, commands: &RouteProjection<'_>) -> bool {
        self.doors
            .iter()
            .min_by_key(|door| (door.chebyshev(tile), door.y, door.x))
            .is_some_and(|door| {
                self.distance(tile).is_some() && commands.direct_line_avoids_blocked(tile, *door)
            })
    }
}

impl UtilityPolicy {
    pub(super) fn orphan_construction_work(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        resources: &ResourceSnapshot,
        orientation: super::super::orient::Orientation,
        unavailable: &[UnitId],
        contacts: (&[UnitContact], &[BuildingContact]),
    ) -> Vec<ConstructionWork> {
        let orphans = obs
            .my_buildings
            .iter()
            .filter(|site| {
                !site.built
                    && site.tier == 0
                    && site.hp > 0
                    && !obs.my_units.iter().any(|unit| unit.site == Some(site.id))
                    && !self.harvest_location_contested(site.anchor)
            })
            .collect::<Vec<_>>();
        if orphans.is_empty() {
            return Vec::new();
        }
        let danger = self.harvest_danger_projection(obs, Some(contacts.0), Some(contacts.1));
        let blocked = |tile| danger.contains(tile) || self.harvest_location_contested(tile);
        let commands = RouteProjection::ground_avoiding_with_public_terrain(
            QueryPurpose::ConstructionAccess,
            obs,
            briefing,
            orientation,
            blocked,
        );
        let mut result = Vec::new();
        for site in orphans {
            let Some(construction) = site.kind.base_stats().construction else {
                continue;
            };
            let (width, height) = site.kind.base_stats().size;
            let doors = (-1..=height)
                .flat_map(|dy| (-1..=width).map(move |dx| site.anchor.offset(dx, dy)))
                .filter(|tile| {
                    routing::ground_open(QueryPurpose::ConstructionAccess, obs, *tile)
                        && !blocked(*tile)
                })
                .collect::<Vec<_>>();
            if doors.is_empty() {
                continue;
            }
            let distances = PublicGroundDistances::from_sources_avoiding(
                QueryPurpose::ConstructionAccess,
                briefing,
                doors.iter().copied(),
                |tile| !commands.open(tile),
            );
            let mut routes = crate::bot::navigation::public_fields::WorkRoutes::new(
                &commands, &distances, &doors,
            );
            let mut baseline = u64::MAX;
            for unit in &obs.my_units {
                if unit.kind.stats().harvest.is_none()
                    || unavailable.contains(&unit.id)
                    || !builder_is_free(obs, unit)
                    || self.scout == Some(unit.id)
                    || self.evacuating_workers.contains(&unit.id)
                {
                    continue;
                }
                if let Some(distance) = routes.distance(unit.tile) {
                    baseline = baseline.min(
                        travel_ticks(unit.kind, distance).saturating_add(
                            u64::from(construction.build_ticks)
                                .div_ceil(u64::from(unit.kind.stats().build_rate.max(1))),
                        ),
                    );
                }
            }
            let mut producer_distances = std::collections::BTreeMap::new();
            for lane in resources.producers() {
                let Some(producer) = obs
                    .my_buildings
                    .iter()
                    .find(|building| building.id == lane.producer)
                else {
                    continue;
                };
                let Some(spawn) = routing::production_spawn_doorstep(
                    QueryPurpose::ConstructionAccess,
                    obs,
                    producer,
                    Some(briefing),
                    Some(orientation),
                ) else {
                    continue;
                };
                let Some(distance) = routes.distance(spawn) else {
                    continue;
                };
                producer_distances.insert(lane.producer, distance);
                for (kind, ready) in lane
                    .queued_readiness()
                    .filter(|(kind, _)| kind.stats().harvest.is_some())
                {
                    baseline = baseline.min(
                        ready
                            .saturating_add(1)
                            .saturating_sub(obs.tick)
                            .saturating_add(travel_ticks(kind, distance))
                            .saturating_add(
                                u64::from(construction.build_ticks)
                                    .div_ceil(u64::from(kind.stats().build_rate.max(1))),
                            ),
                    );
                }
            }
            result.push(ConstructionWork {
                service: site.anchor,
                value: construction.cost,
                build_ticks: u64::from(construction.build_ticks),
                baseline,
                producer_distances,
            });
        }
        result
    }

    pub(super) fn economic_harvest_regions(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        resources: &ResourceSnapshot,
        orientation: super::super::orient::Orientation,
        unavailable: &[UnitId],
        contacts: (&[UnitContact], &[BuildingContact]),
    ) -> Vec<HarvestRegion> {
        if !obs
            .known_scrap
            .iter()
            .chain(&obs.known_wrecks)
            .any(|(tile, amount)| *amount > 0 && obs.visible(*tile))
        {
            return Vec::new();
        }
        let danger = self.harvest_danger_projection(obs, Some(contacts.0), Some(contacts.1));
        let commands = RouteProjection::ground_avoiding_with_public_terrain(
            QueryPurpose::HarvestValuation,
            obs,
            briefing,
            orientation,
            |tile| danger.contains(tile) || self.harvest_location_contested(tile),
        );
        let blocked_cells = (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .map(|tile| !commands.open(tile))
            .collect::<Vec<_>>();
        let blocked = |tile: TilePos| {
            tile.x < 0
                || tile.y < 0
                || tile.x >= obs.map_width
                || tile.y >= obs.map_height
                || blocked_cells[(tile.y * obs.map_width + tile.x) as usize]
        };
        let routes = RouteProjection::ground_avoiding(QueryPurpose::HarvestValuation, obs, blocked);
        let mut dropoffs = obs
            .my_buildings
            .iter()
            .filter(|building| building.built && building.kind.is_drop_off())
            .collect::<Vec<_>>();
        dropoffs
            .sort_unstable_by_key(|building| (building.anchor.y, building.anchor.x, building.id));
        let mut components: Vec<Vec<TilePos>> = Vec::new();
        for building in dropoffs {
            let (width, height) = building.kind.tier_stats(building.tier).size;
            let mut doors = (-1..=height)
                .flat_map(|dy| (-1..=width).map(move |dx| building.anchor.offset(dx, dy)))
                .filter(|tile| !blocked(*tile))
                .collect::<Vec<_>>();
            doors.sort_unstable_by_key(|tile| (tile.y, tile.x));
            for door in doors {
                if let Some(component) = components
                    .iter_mut()
                    .find(|component| routes.reaches(component[0], door))
                {
                    component.push(door);
                } else {
                    components.push(vec![door]);
                }
            }
        }
        let mut regions = components
            .into_iter()
            .map(|sources| HarvestRegion {
                service: sources[0],
                work: HarvestWork {
                    amount: 0,
                    positions: 0,
                    haul_cost: 0,
                },
                workers: Vec::new(),
                doors: sources.clone(),
                producer_access: std::collections::BTreeMap::new(),
                distances: PublicGroundDistances::from_sources_avoiding(
                    QueryPurpose::HarvestValuation,
                    briefing,
                    sources,
                    blocked,
                ),
            })
            .collect::<Vec<_>>();
        let mut positions = vec![BTreeSet::new(); regions.len()];
        let mut weighted_haul = vec![0u128; regions.len()];
        let mut sources = std::collections::BTreeMap::<(i32, i32), u64>::new();
        for &(tile, amount) in obs.known_scrap.iter().chain(&obs.known_wrecks) {
            if amount == 0
                || !obs.visible(tile)
                || self.dead_nodes.contains(&tile)
                || Self::source_in_salvage_incident(obs, tile)
                || danger.contains(tile)
                || self.harvest_location_contested(tile)
            {
                continue;
            }
            let amount_at = sources.entry((tile.y, tile.x)).or_default();
            *amount_at = amount_at.saturating_add(u64::from(amount));
        }
        for ((y, x), amount) in sources {
            let source = TilePos::new(x, y);
            let work_tiles = if obs.known_scrap_at(source) {
                (-1..=1)
                    .flat_map(|dy| (-1..=1).map(move |dx| source.offset(dx, dy)))
                    .filter(|tile| *tile != source && !blocked(*tile))
                    .collect::<Vec<_>>()
            } else if !blocked(source) {
                vec![source]
            } else {
                Vec::new()
            };
            let chosen = regions
                .iter()
                .enumerate()
                .filter_map(|(index, region)| {
                    work_tiles
                        .iter()
                        .filter(|tile| region.harvest_corridor(**tile, &commands))
                        .filter_map(|tile| region.distance(*tile))
                        .min()
                        .map(|distance| (distance, region.service.y, region.service.x, index))
                })
                .min();
            let Some((distance, _, _, index)) = chosen else {
                continue;
            };
            let region = &mut regions[index];
            region.work.amount = region.work.amount.saturating_add(amount);
            weighted_haul[index] = weighted_haul[index]
                .saturating_add(u128::from(distance).saturating_mul(u128::from(amount)));
            positions[index].extend(work_tiles.into_iter().filter(|tile| {
                region.distance(*tile).is_some() && region.harvest_corridor(*tile, &commands)
            }));
        }
        for (index, region) in regions.iter_mut().enumerate() {
            region.work.positions = positions[index].len();
            region.work.haul_cost = u32::try_from(
                weighted_haul[index]
                    .checked_div(u128::from(region.work.amount))
                    .unwrap_or(0),
            )
            .unwrap_or(u32::MAX);
        }
        let mut work_distances = positions.iter().map(|_| None).collect::<Vec<_>>();
        for unit in &obs.my_units {
            if unit.kind.stats().harvest.is_none()
                || unavailable.contains(&unit.id)
                || !builder_is_free(obs, unit)
                || self.scout == Some(unit.id)
                || self.evacuating_workers.contains(&unit.id)
            {
                continue;
            }
            if let Some((index, region)) = regions.iter_mut().enumerate().find(|(_, region)| {
                region.distance(unit.tile).is_some()
                    && region.harvest_corridor(unit.tile, &commands)
            }) && let Some(distance) = work_distances[index]
                .get_or_insert_with(|| {
                    PublicGroundDistances::from_sources_avoiding(
                        QueryPurpose::HarvestValuation,
                        briefing,
                        positions[index].iter().copied(),
                        blocked,
                    )
                })
                .footprint_distance(unit.tile, (1, 1))
            {
                region.workers.push(WorkerService {
                    kind: unit.kind,
                    ready_after: travel_ticks(unit.kind, distance),
                });
            }
        }
        for lane in resources.producers() {
            let Some(building) = obs
                .my_buildings
                .iter()
                .find(|building| building.id == lane.producer)
            else {
                continue;
            };
            let Some(spawn) = routing::production_spawn_doorstep(
                QueryPurpose::HarvestValuation,
                obs,
                building,
                Some(briefing),
                Some(orientation),
            ) else {
                continue;
            };
            for region in &mut regions {
                if let Some(distance) = region.distance(spawn)
                    && region.harvest_corridor(spawn, &commands)
                {
                    region.producer_access.insert(
                        lane.producer,
                        distance.saturating_add(region.work.haul_cost),
                    );
                }
            }
            for (kind, ready_at) in lane.queued_readiness() {
                if kind.stats().harvest.is_none() {
                    continue;
                }
                let Some(region) = regions
                    .iter_mut()
                    .find(|region| region.producer_distance(lane.producer).is_some())
                else {
                    continue;
                };
                let distance = region
                    .producer_distance(lane.producer)
                    .expect("the lane serves this work region");
                region.workers.push(WorkerService {
                    kind,
                    ready_after: ready_at
                        .saturating_add(1)
                        .saturating_sub(obs.tick)
                        .saturating_add(travel_ticks(kind, distance)),
                });
            }
        }
        regions.retain(|region| region.work.amount > 0 && region.work.positions > 0);
        regions
    }
}

pub(super) struct ConstructionWork {
    pub(super) service: TilePos,
    value: u32,
    build_ticks: u64,
    baseline: u64,
    producer_distances: std::collections::BTreeMap<BuildingId, u32>,
}

impl ConstructionWork {
    pub(super) fn marginal(
        &self,
        producer: BuildingId,
        worker: WorkerService,
        horizon: u64,
    ) -> u64 {
        let Some(distance) = self.producer_distances.get(&producer) else {
            return 0;
        };
        let completion = worker
            .ready_after
            .saturating_add(travel_ticks(worker.kind, *distance))
            .saturating_add(
                self.build_ticks
                    .div_ceil(u64::from(worker.kind.stats().build_rate.max(1))),
            );
        u64::from(self.value).saturating_mul(self.baseline.min(horizon).saturating_sub(completion))
            / horizon.max(1)
    }
}
