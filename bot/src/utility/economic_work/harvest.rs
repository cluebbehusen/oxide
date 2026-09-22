//! Drop-off service geometry and finite harvest returns.

use super::super::danger::HarvestDangerProjection;
use super::super::economic_value::{HarvestWork, harvest_output, marginal_worker_return};
use super::*;
#[cfg(test)]
use crate::observation::ObservationData;
use crate::orient::Orientation;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::utility) struct HarvestRegion {
    pub(in crate::utility) service: TilePos,
    pub(in crate::utility) work: HarvestWork,
    pub(in crate::utility) workers: Vec<WorkerService>,
    producer_access: std::collections::BTreeMap<BuildingId, u32>,
}

impl HarvestRegion {
    pub(in crate::utility) fn current_output(&self, horizon: u64) -> u64 {
        harvest_output(self.work, &self.workers, horizon)
    }

    pub(in crate::utility) fn marginal(&self, worker: WorkerService, horizon: u64) -> u64 {
        marginal_worker_return(self.work, &self.workers, worker, horizon)
    }

    pub(in crate::utility) fn producer_distance(&self, producer: BuildingId) -> Option<u32> {
        self.producer_access.get(&producer).copied()
    }
}

struct HarvestGeometry<'a, 'cache> {
    commands: RouteProjection<'a>,
    services: &'cache mut [HarvestService],
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::utility) struct HarvestGeometryCache {
    key: Option<HarvestGeometryKey>,
    services: Vec<HarvestService>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HarvestGeometryKey {
    routes: routing::RouteGeometryKey,
    briefing: PublicMapBriefing,
    dropoffs: Vec<(TilePos, BuildingId, (i32, i32))>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HarvestService {
    doors: Vec<TilePos>,
    distances: PublicGroundDistances,
    corridors: std::collections::BTreeMap<TilePos, Option<u32>>,
}

impl HarvestService {
    fn distance(&self, tile: TilePos) -> Option<u32> {
        self.distances.footprint_distance(tile, (1, 1))
    }

    fn corridor_distance(&mut self, tile: TilePos, commands: &RouteProjection<'_>) -> Option<u32> {
        if let Some(distance) = self.corridors.get(&tile) {
            return *distance;
        }
        let distance = self.uncached_corridor_distance(tile, commands);
        if self.corridors.len() == 4096 {
            self.corridors.pop_first();
        }
        self.corridors.insert(tile, distance);
        distance
    }

    fn uncached_corridor_distance(
        &self,
        tile: TilePos,
        commands: &RouteProjection<'_>,
    ) -> Option<u32> {
        let distance = self.distance(tile)?;
        let door = self
            .doors
            .iter()
            .min_by_key(|door| (door.chebyshev(tile), door.y, door.x))?;
        (commands.direct_line_avoids_blocked(tile, *door)
            && commands.command_path_avoids_blocked(tile, *door)
            && commands.command_path_avoids_blocked(*door, tile))
        .then_some(distance)
    }
}

struct HarvestWorkRegion {
    value: HarvestRegion,
    positions: BTreeSet<TilePos>,
}

impl HarvestGeometryCache {
    fn prepare(
        &mut self,
        commands: &RouteProjection<'_>,
        briefing: &PublicMapBriefing,
        obs: &Observation,
    ) -> &mut [HarvestService] {
        let mut dropoffs = obs
            .my_buildings
            .iter()
            .filter(|building| building.built && building.kind.is_drop_off())
            .collect::<Vec<_>>();
        dropoffs
            .sort_unstable_by_key(|building| (building.anchor.y, building.anchor.x, building.id));
        let key = HarvestGeometryKey {
            routes: commands.geometry_key(),
            briefing: briefing.clone(),
            dropoffs: dropoffs
                .iter()
                .map(|building| {
                    (
                        building.anchor,
                        building.id,
                        building.kind.tier_stats(building.tier).size,
                    )
                })
                .collect(),
        };
        if self.key.as_ref() == Some(&key) {
            return &mut self.services;
        }
        let mut components: Vec<Vec<TilePos>> = Vec::new();
        for building in dropoffs {
            let (width, height) = building.kind.tier_stats(building.tier).size;
            let mut doors = (-1..=height)
                .flat_map(|dy| (-1..=width).map(move |dx| building.anchor.offset(dx, dy)))
                .filter(|tile| commands.open(*tile))
                .collect::<Vec<_>>();
            doors.sort_unstable_by_key(|tile| (tile.y, tile.x));
            for door in doors {
                if let Some(component) = components
                    .iter_mut()
                    .find(|component| commands.reaches(component[0], door))
                {
                    component.push(door);
                } else {
                    components.push(vec![door]);
                }
            }
        }
        self.services = components
            .into_iter()
            .map(|doors| HarvestService {
                distances: PublicGroundDistances::from_sources_avoiding(
                    QueryPurpose::HarvestValuation,
                    briefing,
                    doors.iter().copied(),
                    |tile| !commands.open(tile),
                ),
                doors,
                corridors: Default::default(),
            })
            .collect();
        self.key = Some(key);
        &mut self.services
    }
}

impl HarvestGeometry<'_, '_> {
    fn value_resources(
        &mut self,
        policy: &UtilityPolicy,
        obs: &Observation,
        danger: &HarvestDangerProjection,
    ) -> Vec<HarvestWorkRegion> {
        let mut regions = self
            .services
            .iter()
            .map(|service| HarvestWorkRegion {
                value: HarvestRegion {
                    service: service.doors[0],
                    work: HarvestWork {
                        amount: 0,
                        positions: 0,
                        haul_cost: 0,
                    },
                    workers: Vec::new(),
                    producer_access: Default::default(),
                },
                positions: BTreeSet::new(),
            })
            .collect::<Vec<_>>();
        let mut weighted_haul = vec![0u128; regions.len()];
        let mut sources = std::collections::BTreeMap::<(i32, i32), u64>::new();
        for &(tile, amount) in obs.known_scrap.iter().chain(&obs.known_wrecks) {
            if amount == 0
                || !obs.visible(tile)
                || policy.state.work_experience.dead_nodes.contains(&tile)
                || UtilityPolicy::source_in_salvage_incident(obs, tile)
                || danger.contains(tile)
                || policy.harvest_location_contested(tile)
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
                    .filter(|tile| *tile != source && self.commands.open(*tile))
                    .collect::<Vec<_>>()
            } else if self.commands.open(source) {
                vec![source]
            } else {
                Vec::new()
            };
            let chosen = self
                .services
                .iter_mut()
                .enumerate()
                .filter_map(|(index, service)| {
                    let accessible = work_tiles
                        .iter()
                        .filter_map(|&tile| {
                            service
                                .corridor_distance(tile, &self.commands)
                                .map(|distance| (tile, distance))
                        })
                        .collect::<Vec<_>>();
                    let distance = accessible.iter().map(|(_, distance)| *distance).min()?;
                    let anchor = service.doors[0];
                    Some(((distance, anchor.y, anchor.x, index), accessible))
                })
                .min_by_key(|(rank, _)| *rank);
            let Some(((distance, _, _, index), accessible)) = chosen else {
                continue;
            };
            let region = &mut regions[index];
            region.value.work.amount = region.value.work.amount.saturating_add(amount);
            weighted_haul[index] = weighted_haul[index]
                .saturating_add(u128::from(distance).saturating_mul(u128::from(amount)));
            region
                .positions
                .extend(accessible.into_iter().map(|(tile, _)| tile));
        }
        for (region, weighted) in regions.iter_mut().zip(weighted_haul) {
            region.value.work.positions = region.positions.len();
            region.value.work.haul_cost = u32::try_from(
                weighted
                    .checked_div(u128::from(region.value.work.amount))
                    .unwrap_or(0),
            )
            .unwrap_or(u32::MAX);
        }
        regions
    }

    fn credit_live_workers(
        &mut self,
        policy: &UtilityPolicy,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        unavailable: &[UnitId],
        regions: &mut [HarvestWorkRegion],
    ) {
        let mut work_distances = self.services.iter().map(|_| None).collect::<Vec<_>>();
        for unit in &obs.my_units {
            if unit.kind.stats().harvest.is_none()
                || unavailable.contains(&unit.id)
                || !builder_is_free(obs, unit)
                || policy.state.scout == Some(unit.id)
                || policy.state.evacuating_workers.contains(&unit.id)
            {
                continue;
            }
            if let Some(index) =
                self.services
                    .iter_mut()
                    .enumerate()
                    .find_map(|(index, service)| {
                        service
                            .corridor_distance(unit.tile, &self.commands)
                            .map(|_| index)
                    })
                && let Some(distance) = work_distances[index]
                    .get_or_insert_with(|| {
                        PublicGroundDistances::from_sources_avoiding(
                            QueryPurpose::HarvestValuation,
                            briefing,
                            regions[index].positions.iter().copied(),
                            |tile| !self.commands.open(tile),
                        )
                    })
                    .footprint_distance(unit.tile, (1, 1))
            {
                regions[index].value.workers.push(WorkerService {
                    kind: unit.kind,
                    ready_after: travel_ticks(unit.kind, distance),
                });
            }
        }
    }

    fn credit_producers(
        &mut self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        resources: &ResourceSnapshot,
        orientation: Orientation,
        regions: &mut [HarvestWorkRegion],
    ) {
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
            for (service, region) in self.services.iter_mut().zip(regions.iter_mut()) {
                if let Some(distance) = service.corridor_distance(spawn, &self.commands) {
                    region.value.producer_access.insert(
                        lane.producer,
                        distance.saturating_add(region.value.work.haul_cost),
                    );
                }
            }
            for (kind, ready_at) in lane.queued_readiness() {
                if kind.stats().harvest.is_none() {
                    continue;
                }
                let Some(region) = regions
                    .iter_mut()
                    .find(|region| region.value.producer_distance(lane.producer).is_some())
                else {
                    continue;
                };
                let distance = region
                    .value
                    .producer_distance(lane.producer)
                    .expect("the lane serves this work region");
                region.value.workers.push(WorkerService {
                    kind,
                    ready_after: ready_at
                        .saturating_add(1)
                        .saturating_sub(obs.tick)
                        .saturating_add(travel_ticks(kind, distance)),
                });
            }
        }
    }
}

impl UtilityPolicy {
    pub(in crate::utility) fn economic_harvest_regions(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        resources: &ResourceSnapshot,
        orientation: Orientation,
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
        let mut cache = self.queries.harvest_geometry_cache.borrow_mut();
        let services = cache.prepare(&commands, briefing, obs);
        let mut geometry = HarvestGeometry { commands, services };
        let mut regions = geometry.value_resources(self, obs, &danger);
        geometry.credit_live_workers(self, obs, briefing, unavailable, &mut regions);
        geometry.credit_producers(obs, briefing, resources, orientation, &mut regions);
        regions
            .into_iter()
            .map(|region| region.value)
            .filter(|region| region.work.amount > 0 && region.work.positions > 0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harvest_regions_share_dropoffs_without_double_counting_work_or_worker_supply() {
        use super::super::super::test_world as world;
        let scenario = world::scenario_with(|tile| if tile.x == 20 { '#' } else { '.' });
        let map = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let mut obs = world::observation(PlayerId(0), world::LEFT_HOME);
        obs.visible.fill(true);
        {
            let obs = &mut *obs;
            obs.my_buildings.extend([
                world::building(2, obs.me, BuildingKind::Foundry, TilePos::new(28, 10)),
                world::building(3, obs.me, BuildingKind::Foundry, TilePos::new(10, 3)),
            ]);
        }
        obs.my_queues = vec![
            vec![UnitKind::Harvester, UnitKind::Sentinel, UnitKind::Excavator],
            vec![],
            vec![],
        ];
        obs.my_queue_progress = vec![0; 3];
        obs.my_units = vec![
            world::unit(1, obs.me, UnitKind::Harvester, TilePos::new(14, 12)),
            world::unit(2, obs.me, UnitKind::Harvester, TilePos::new(26, 12)),
            world::unit(3, obs.me, UnitKind::Harvester, TilePos::new(17, 12)),
        ];
        obs.known_scrap = vec![(TilePos::new(15, 12), 100), (TilePos::new(16, 12), 100)];
        obs.known_wrecks = vec![(TilePos::new(15, 12), 50), (TilePos::new(26, 12), 500)];
        let policy = UtilityPolicy::new();
        let evaluate = |obs: &Observation, unavailable: &[UnitId]| {
            policy.economic_harvest_regions(
                obs,
                &map,
                &ResourceSnapshot::from_observation(obs),
                Orientation::for_home(obs, world::LEFT_HOME),
                unavailable,
                (&[], &[]),
            )
        };
        let (regions, cold_work) =
            crate::navigation::work::measure(|| evaluate(&obs, &[UnitId(3)]));
        let (repeated, warm_work) =
            crate::navigation::work::measure(|| evaluate(&obs, &[UnitId(3)]));
        assert_eq!(repeated, regions);
        assert_eq!(
            cold_work.fields - warm_work.fields,
            2,
            "both service fields survive the next decision"
        );
        assert_eq!(regions.len(), 2);
        let left = regions.iter().find(|r| r.service.x < 20).unwrap();
        let right = regions.iter().find(|r| r.service.x > 20).unwrap();
        assert_eq!((left.work.amount, left.work.positions), (250, 10));
        assert_eq!((right.work.amount, right.work.positions), (500, 1));
        assert_eq!(
            left.workers.len(),
            3,
            "one available worker and two paid worker occurrences"
        );
        assert_eq!(left.workers[0].ready_after, 0);
        assert!(left.workers[1].ready_after > 0);
        assert!(left.workers[2].ready_after > left.workers[1].ready_after);
        assert_eq!(right.workers.len(), 1);
        assert_eq!(right.workers[0].ready_after, 0);
        assert!(left.producer_distance(BuildingId(0)).is_some());
        assert!(left.producer_distance(BuildingId(2)).is_none());
        assert!(right.producer_distance(BuildingId(0)).is_none());
        assert!(right.producer_distance(BuildingId(2)).is_some());

        obs.known_scrap[0].1 = 40;
        {
            let obs = &mut *obs;
            obs.visible[(12 * obs.map_width + 26) as usize] = false;
        }
        let updated = evaluate(&obs, &[UnitId(1), UnitId(3)]);
        assert_eq!(
            updated.len(),
            1,
            "remembered amounts cannot keep a region productive"
        );
        assert_eq!(updated[0].work.amount, 190);
        assert_eq!(
            updated[0].workers.len(),
            2,
            "availability is recomputed independently of service geometry"
        );
    }

    #[test]
    fn retained_service_geometry_invalidates_every_route_input() {
        use super::super::super::test_world as world;
        let mut obs = world::observation(PlayerId(0), world::LEFT_HOME);
        obs.visible.fill(true);
        obs.known_wrecks = vec![(TilePos::new(15, 12), 500)];
        let mut map = world::briefing();
        let mut policy = UtilityPolicy::new();
        let mut orientation = Orientation::for_home(&obs, world::LEFT_HOME);
        for change in 0..9 {
            match change {
                1 => obs.ally_buildings.push(world::building(
                    8,
                    PlayerId(1),
                    BuildingKind::Foundry,
                    TilePos::new(9, 10),
                )),
                2 => obs.ally_buildings[0].provisional = true,
                3 => map
                    .non_ground_terrain
                    .push((TilePos::new(12, 10), oxide_sim::map::Terrain::Rock)),
                4 => obs.blips.push(TilePos::new(28, 12)),
                5 => policy.state.contested_harvest_regions.push(
                    super::super::super::ContestedHarvestRegion {
                        center: TilePos::new(13, 12),
                        last_evidence: obs.tick,
                        sweep_started_at: None,
                    },
                ),
                6 => orientation = Orientation::for_home(&obs, world::RIGHT_HOME),
                7 => obs.my_buildings[0].built = false,
                8 => {
                    obs.my_buildings[0].built = true;
                    policy.state.contested_harvest_regions.clear();
                }
                _ => {}
            }
            let previous = policy.queries.harvest_geometry_cache.borrow().key.clone();
            let resources = ResourceSnapshot::from_observation(&obs);
            let evaluate = |policy: &UtilityPolicy| {
                policy.economic_harvest_regions(
                    &obs,
                    &map,
                    &resources,
                    orientation,
                    &[],
                    (&[], &[]),
                )
            };
            let mut cold = policy.clone();
            *cold.queries.harvest_geometry_cache.get_mut() = Default::default();
            assert_eq!(evaluate(&policy), evaluate(&cold), "changed input {change}");
            assert_ne!(
                previous,
                policy.queries.harvest_geometry_cache.borrow().key,
                "changed input {change}"
            );
        }
        let key = policy.queries.harvest_geometry_cache.borrow().key.clone();
        policy
            .state
            .work_experience
            .dead_nodes
            .push(TilePos::new(15, 12));
        assert!(
            policy
                .economic_harvest_regions(
                    &obs,
                    &map,
                    &ResourceSnapshot::from_observation(&obs),
                    orientation,
                    &[],
                    (&[], &[])
                )
                .is_empty()
        );
        assert_eq!(
            key,
            policy.queries.harvest_geometry_cache.borrow().key,
            "resource eligibility changes value without changing geometry"
        );
    }

    #[test]
    fn harvest_corridor_requires_safe_command_detours_in_both_directions() {
        let obs = Observation::from_data(ObservationData {
            map_width: 13,
            map_height: 9,
            visible: vec![true; 117],
            explored: vec![true; 117],
            known_rock: (3..=5).map(|y| TilePos::new(6, y)).collect(),
            ..crate::test_support::observation_data()
        });
        let map = PublicMapBriefing {
            regions: Default::default(),
            map_width: obs.map_width,
            map_height: obs.map_height,
            starting_foundries: Vec::new(),
            teams: Vec::new(),
            non_ground_terrain: Vec::new(),
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        };
        let from = TilePos::new(2, 4);
        let to = TilePos::new(10, 4);
        for (danger, outward, returning) in [
            (TilePos::new(6, 2), false, true),
            (TilePos::new(6, 6), true, false),
            (TilePos::new(0, 0), true, true),
        ] {
            let commands =
                RouteProjection::ground_avoiding(QueryPurpose::HarvestValuation, &obs, |tile| {
                    tile == danger
                });
            let mut region = HarvestService {
                distances: PublicGroundDistances::from_sources_avoiding(
                    QueryPurpose::HarvestValuation,
                    &map,
                    [to],
                    |tile| !commands.open(tile),
                ),
                doors: vec![to],
                corridors: Default::default(),
            };
            assert!(region.distance(from).is_some());
            assert!(commands.direct_line_avoids_blocked(from, to));
            assert_eq!(
                region.corridor_distance(from, &commands).is_some(),
                outward && returning
            );
            assert_eq!(commands.command_path_avoids_blocked(from, to), outward);
            assert_eq!(commands.command_path_avoids_blocked(to, from), returning);
            let (_, warm) = crate::navigation::work::measure(|| {
                for _ in 0..100 {
                    assert_eq!(
                        region.corridor_distance(from, &commands).is_some(),
                        outward && returning
                    );
                }
            });
            assert_eq!(
                warm.searches, 0,
                "repeated harvest checks must reuse route proofs"
            );
            for x in -4200..0 {
                assert!(
                    region
                        .corridor_distance(TilePos::new(x, 0), &commands)
                        .is_none()
                );
            }
            assert_eq!(region.corridors.len(), 4096);
            assert_eq!(
                region.corridor_distance(from, &commands).is_some(),
                outward && returning
            );
        }
    }
}
