//! Artillery geometry and command reachability for one immutable planning batch.

use super::*;
use std::cell::RefCell;

type StandOptions = BTreeMap<(SuppressionOrigin, Target), Vec<TilePos>>;

pub(super) struct SuppressionRoutes<'a> {
    obs: &'a Observation,
    intel: &'a StrategicIntelligence,
    public_map: Option<&'a PublicMapBriefing>,
    routes: RouteProjection<'a>,
    options: RefCell<StandOptions>,
    #[cfg(test)]
    queries: std::cell::Cell<usize>,
}

impl core::fmt::Debug for SuppressionRoutes<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SuppressionRoutes").finish_non_exhaustive()
    }
}

impl<'a> SuppressionRoutes<'a> {
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
            options: RefCell::new(BTreeMap::new()),
            #[cfg(test)]
            queries: std::cell::Cell::new(0),
        }
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
        let options: Vec<_> = suppression_firing_stands(
            &self.routes,
            self.obs,
            origin,
            target,
            self.intel,
            self.public_map,
        )
        .collect();
        let result = visit(&options);
        let mut retained = self.options.borrow_mut();
        if retained.len() < 256 {
            retained.insert((origin, target), options);
        }
        result
    }

    #[cfg(test)]
    pub(super) fn evaluated_queries(&self) -> usize {
        self.queries.get()
    }

    pub(super) fn reaches(&self, origin: SuppressionOrigin, target: Target) -> bool {
        self.with_stands(origin, target, |stands| !stands.is_empty())
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
