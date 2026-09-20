//! Cached source, producer, target, and repair connectivity within one observation.

use super::ServiceTarget;
use super::commands::{RouteProjection, air_production_spawn_tile, production_spawn_doorstep};
use crate::bot::query_work::QueryPurpose;
use crate::bot::{Orientation, PublicMapBriefing, observation::Observation};
use crate::ids::BuildingId;
use crate::stats::{Domain, UnitKind};
use chassis::{Tick, grid::TilePos};
use std::collections::BTreeMap;

#[derive(Debug, Default)]
struct RouteComponentIndex {
    representatives: Vec<TilePos>,
    tiles: BTreeMap<TilePos, Option<usize>>,
}

impl RouteComponentIndex {
    fn component(&mut self, routes: &mut RouteProjection<'_>, tile: TilePos) -> Option<usize> {
        if let Some(component) = self.tiles.get(&tile) {
            return *component;
        }
        if !routes.reaches(tile, tile) {
            self.tiles.insert(tile, None);
            return None;
        }
        for (component, representative) in self.representatives.iter().copied().enumerate() {
            if routes.reaches(tile, representative) {
                self.tiles.insert(tile, Some(component));
                return Some(component);
            }
        }
        let component = self.representatives.len();
        self.representatives.push(tile);
        self.tiles.insert(tile, Some(component));
        Some(component)
    }
}

pub(crate) struct ServiceRoutes<'a> {
    pub(in crate::bot) query_purpose: QueryPurpose,
    obs: &'a Observation,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Option<Orientation>,
    ground_routes: RouteProjection<'a>,
    air_routes: Option<RouteProjection<'a>>,
    ground_components: RouteComponentIndex,
    air_components: RouteComponentIndex,
    ground_target_components: BTreeMap<ServiceTarget, Vec<usize>>,
    air_target_components: BTreeMap<ServiceTarget, Vec<usize>>,
    ground_producer_components: BTreeMap<BuildingId, Option<usize>>,
    air_producer_components: BTreeMap<BuildingId, Option<usize>>,
    travel: BTreeMap<(UnitKind, TilePos, TilePos), Option<Tick>>,
}

impl<'a> ServiceRoutes<'a> {
    pub(crate) fn new(
        query_purpose: QueryPurpose,
        obs: &'a Observation,
        public_map: Option<&'a PublicMapBriefing>,
        orientation: Option<Orientation>,
    ) -> Self {
        Self {
            query_purpose,
            obs,
            public_map,
            orientation,
            ground_routes: service_route_projection(
                query_purpose,
                obs,
                Domain::Ground,
                public_map,
                orientation,
            ),
            air_routes: None,
            ground_components: RouteComponentIndex::default(),
            air_components: RouteComponentIndex::default(),
            ground_target_components: BTreeMap::new(),
            air_target_components: BTreeMap::new(),
            ground_producer_components: BTreeMap::new(),
            air_producer_components: BTreeMap::new(),
            travel: BTreeMap::new(),
        }
    }

    pub(crate) fn origin_serves(
        &mut self,
        origin: TilePos,
        kind: UnitKind,
        service: ServiceTarget,
    ) -> bool {
        let Some(component) = self.component(kind.stats().domain, origin) else {
            return false;
        };
        self.components_for_target(kind.stats().domain, service)
            .binary_search(&component)
            .is_ok()
    }

    pub(in crate::bot) fn repair_travel(
        &mut self,
        from: TilePos,
        goal: TilePos,
        kind: UnitKind,
    ) -> Option<Tick> {
        let key = (kind, from, goal);
        if let Some(travel) = self.travel.get(&key) {
            return *travel;
        }
        let travel = self
            .origin_serves(from, kind, ServiceTarget::point(goal))
            .then(|| {
                let cost = self
                    .ground_routes
                    .safe_command_route_cost(from, goal, false)?;
                Some(super::travel::travel_ticks(kind, cost))
            })
            .flatten();
        self.travel.insert(key, travel);
        travel
    }

    pub(crate) fn producer_reaches_any(
        &mut self,
        producer: BuildingId,
        kind: UnitKind,
        targets: &[ServiceTarget],
    ) -> bool {
        let Some(producer_component) = self.producer_component(producer, kind) else {
            return false;
        };
        self.components_for_targets(kind.stats().domain, targets)
            .binary_search(&producer_component)
            .is_ok()
    }

    pub(in crate::bot) fn producer_component(
        &mut self,
        producer: BuildingId,
        kind: UnitKind,
    ) -> Option<usize> {
        let domain = kind.stats().domain;
        let cached = match domain {
            Domain::Ground => self.ground_producer_components.get(&producer),
            Domain::Air => self.air_producer_components.get(&producer),
        };
        if let Some(component) = cached {
            return *component;
        }
        let building = self
            .obs
            .my_buildings
            .iter()
            .find(|building| building.id == producer && building.built && building.hp > 0)?;
        let origin = match domain {
            Domain::Ground => production_spawn_doorstep(
                self.query_purpose,
                self.obs,
                building,
                self.public_map,
                self.orientation,
            )?,
            Domain::Air => air_production_spawn_tile(building, self.orientation),
        };
        let component = self.component(domain, origin);
        match domain {
            Domain::Ground => {
                self.ground_producer_components.insert(producer, component);
            }
            Domain::Air => {
                self.air_producer_components.insert(producer, component);
            }
        }
        component
    }

    pub(in crate::bot) fn components_for_targets(
        &mut self,
        domain: Domain,
        targets: &[ServiceTarget],
    ) -> Vec<usize> {
        let mut components = Vec::new();
        for target in targets {
            components.extend(self.components_for_target(domain, *target));
        }
        components.sort_unstable();
        components.dedup();
        components
    }

    pub(in crate::bot) fn components_for_target(
        &mut self,
        domain: Domain,
        target: ServiceTarget,
    ) -> Vec<usize> {
        let cached = match domain {
            Domain::Ground => self.ground_target_components.get(&target),
            Domain::Air => self.air_target_components.get(&target),
        };
        if let Some(components) = cached {
            return components.clone();
        }
        let goals = match (domain, target) {
            (_, ServiceTarget::Point(tile)) => vec![tile],
            (Domain::Ground, ServiceTarget::Footprint { anchor, size }) => {
                crate::tick::rect_adjacent_tiles(anchor, size).collect()
            }
            (Domain::Air, ServiceTarget::Footprint { anchor, size }) => (0..size.1)
                .flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
                .collect(),
        };
        let mut components = goals
            .into_iter()
            .filter_map(|goal| self.component(domain, goal))
            .collect::<Vec<_>>();
        components.sort_unstable();
        components.dedup();
        match domain {
            Domain::Ground => {
                self.ground_target_components
                    .insert(target, components.clone());
            }
            Domain::Air => {
                self.air_target_components
                    .insert(target, components.clone());
            }
        }
        components
    }

    pub(in crate::bot) fn origin_components(
        &mut self,
        domain: Domain,
        origin: TilePos,
    ) -> Vec<usize> {
        if let Some(component) = self.component(domain, origin) {
            return vec![component];
        }
        let mut components = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .filter_map(|(dx, dy)| self.component(domain, origin.offset(dx, dy)))
            .collect::<Vec<_>>();
        components.sort_unstable();
        components.dedup();
        components
    }

    fn component(&mut self, domain: Domain, tile: TilePos) -> Option<usize> {
        match domain {
            Domain::Ground => self
                .ground_components
                .component(&mut self.ground_routes, tile),
            Domain::Air => {
                if self.air_routes.is_none() {
                    self.air_routes = Some(service_route_projection(
                        self.query_purpose,
                        self.obs,
                        Domain::Air,
                        self.public_map,
                        self.orientation,
                    ));
                }
                self.air_components.component(
                    self.air_routes
                        .as_mut()
                        .expect("air projection was initialized"),
                    tile,
                )
            }
        }
    }
}

fn service_route_projection<'a>(
    query_purpose: QueryPurpose,
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Option<Orientation>,
) -> RouteProjection<'a> {
    match (public_map, orientation) {
        (Some(briefing), Some(orientation)) => {
            RouteProjection::with_public_terrain_and_orientation(
                query_purpose,
                obs,
                domain,
                briefing,
                orientation,
            )
        }
        (Some(briefing), None) => {
            RouteProjection::with_public_terrain(query_purpose, obs, domain, briefing)
        }
        (None, Some(orientation)) => {
            RouteProjection::with_orientation(query_purpose, obs, domain, orientation)
        }
        (None, None) => RouteProjection::new(query_purpose, obs, domain),
    }
}
