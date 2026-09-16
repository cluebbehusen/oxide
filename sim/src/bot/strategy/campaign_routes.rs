//! Shared movement and firing geometry for one immutable campaign planning batch.

use super::*;
use std::cell::RefCell;

type StandOptions = BTreeMap<(SuppressionOrigin, Target), Vec<TilePos>>;

pub(super) struct CampaignRoutes<'a> {
    obs: &'a Observation,
    intel: &'a StrategicIntelligence,
    public_map: Option<&'a PublicMapBriefing>,
    routes: RouteProjection<'a>,
    air: RouteProjection<'a>,
    staging: RefCell<BTreeMap<(TilePos, TilePos), Option<TilePos>>>,
    options: RefCell<StandOptions>,
    legal: RefCell<BTreeMap<(UnitKind, Target), Vec<TilePos>>>,
    reachable: RefCell<BTreeMap<(SuppressionOrigin, Target), bool>>,
    #[cfg(test)]
    queries: std::cell::Cell<usize>,
    #[cfg(test)]
    geometry_queries: std::cell::Cell<usize>,
}

impl core::fmt::Debug for CampaignRoutes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CampaignRoutes").finish_non_exhaustive()
    }
}

impl<'a> CampaignRoutes<'a> {
    pub(super) fn new(
        obs: &'a Observation,
        intel: &'a StrategicIntelligence,
        public_map: Option<&'a PublicMapBriefing>,
        orientation: Orientation,
    ) -> Self {
        Self {
            obs,
            intel,
            public_map,
            routes: route_projection_with_orientation(obs, Domain::Ground, public_map, orientation),
            air: route_projection_with_orientation(obs, Domain::Air, public_map, orientation),
            staging: RefCell::new(BTreeMap::new()),
            options: RefCell::new(BTreeMap::new()),
            legal: RefCell::new(BTreeMap::new()),
            reachable: RefCell::new(BTreeMap::new()),
            #[cfg(test)]
            queries: std::cell::Cell::new(0),
            #[cfg(test)]
            geometry_queries: std::cell::Cell::new(0),
        }
    }

    pub(super) fn ground(&self) -> &RouteProjection<'a> {
        &self.routes
    }

    pub(super) fn air(&self) -> &RouteProjection<'a> {
        &self.air
    }

    pub(super) fn staging(&self, home: TilePos, target: TilePos) -> Option<TilePos> {
        if let Some(staging) = self.staging.borrow().get(&(home, target)) {
            return *staging;
        }
        let staging =
            artillery_staging_with_routes(self.obs, home, target, self.public_map, &self.routes);
        let mut retained = self.staging.borrow_mut();
        if retained.len() < 256 {
            retained.insert((home, target), staging);
        }
        staging
    }

    fn with_stands<T>(
        &self,
        origin: SuppressionOrigin,
        target: Target,
        visit: impl FnOnce(&[TilePos]) -> T,
    ) -> T {
        if let Some(options) = self.options.borrow().get(&(origin, target)) {
            return visit(options);
        }
        #[cfg(test)]
        self.queries.set(self.queries.get() + 1);
        let mut options = self.with_legal(origin.kind, target, |tiles| {
            tiles
                .iter()
                .copied()
                .filter(|stand| self.routes.ground_command_reaches(origin.tile, *stand))
                .collect::<Vec<_>>()
        });
        options.sort_unstable_by_key(|stand| (stand.chebyshev(origin.tile), stand.y, stand.x));
        let result = visit(&options);
        let mut retained = self.options.borrow_mut();
        if retained.len() < 256 {
            retained.insert((origin, target), options);
        }
        result
    }

    fn with_legal<T>(
        &self,
        kind: UnitKind,
        target: Target,
        visit: impl FnOnce(&[TilePos]) -> T,
    ) -> T {
        if let Some(tiles) = self.legal.borrow().get(&(kind, target)) {
            return visit(tiles);
        }
        #[cfg(test)]
        self.geometry_queries.set(self.geometry_queries.get() + 1);
        let tiles = legal_suppression_tiles(
            self.obs,
            kind,
            target,
            self.intel,
            self.public_map,
            |tile| self.routes.open(tile),
        );
        let result = visit(&tiles);
        let mut retained = self.legal.borrow_mut();
        if retained.len() < 256 {
            retained.insert((kind, target), tiles);
        }
        result
    }

    #[cfg(test)]
    pub(super) fn evaluated_queries(&self) -> usize {
        self.queries.get()
    }

    #[cfg(test)]
    pub(super) fn evaluated_geometry(&self) -> usize {
        self.geometry_queries.get()
    }

    pub(super) fn reaches(&self, origin: SuppressionOrigin, target: Target) -> bool {
        if let Some(options) = self.options.borrow().get(&(origin, target)) {
            return !options.is_empty();
        }
        if let Some(reachable) = self.reachable.borrow().get(&(origin, target)) {
            return *reachable;
        }
        let reachable = self.with_legal(origin.kind, target, |tiles| {
            tiles
                .iter()
                .any(|stand| self.routes.ground_command_reaches(origin.tile, *stand))
        });
        let mut retained = self.reachable.borrow_mut();
        if retained.len() < 256 {
            retained.insert((origin, target), reachable);
        }
        reachable
    }

    pub(super) fn assignment(
        &self,
        origins: &[SuppressionOrigin],
        target: Target,
    ) -> Option<Vec<TilePos>> {
        let first = *origins.first()?;
        if origins.iter().all(|origin| *origin == first) {
            let mut stands: Vec<_> = self.with_stands(first, target, |stands| {
                stands.iter().copied().take(origins.len()).collect()
            });
            if stands.len() != origins.len() {
                return None;
            }
            stands.reverse();
            return Some(stands);
        }
        assign_suppression_stands(
            origins.len(),
            origins
                .iter()
                .map(|origin| self.with_stands(*origin, target, <[TilePos]>::to_vec))
                .collect(),
        )
    }
}
