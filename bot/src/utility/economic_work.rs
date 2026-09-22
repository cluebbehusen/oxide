//! Safe finite work shared by worker and capital investment quotes.

use super::economic_value::{WorkerService, travel_ticks};
use super::*;
use crate::navigation::public_fields::PublicGroundDistances;
use crate::query_work::QueryPurpose;

mod harvest;
pub(super) use harvest::{HarvestGeometryCache, HarvestRegion};

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
            let mut routes =
                crate::navigation::public_fields::WorkRoutes::new(&commands, &distances, &doors);
            let mut baseline = u64::MAX;
            for unit in &obs.my_units {
                if unit.kind.stats().harvest.is_none()
                    || unavailable.contains(&unit.id)
                    || !builder_is_free(obs, unit)
                    || self.state.scout == Some(unit.id)
                    || self.state.evacuating_workers.contains(&unit.id)
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
