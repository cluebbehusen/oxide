use super::{DefenseDomain, GroundKnowledge, PATH_EXPANSION_CAP, PlacementFootprint};
use chassis::{grid::TilePos, path::AstarScratch};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ops::{Deref, DerefMut},
};

thread_local! {
    static SCRATCH: RefCell<AstarScratch> = RefCell::default();
}

pub(super) struct Scratch(AstarScratch);

impl Default for Scratch {
    fn default() -> Self {
        let mut scratch = SCRATCH.with_borrow_mut(std::mem::take);
        // Reachability evidence belongs to the old passability context.
        scratch.clear_search_evidence();
        Self(scratch)
    }
}

impl Deref for Scratch {
    type Target = AstarScratch;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Scratch {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        SCRATCH.with_borrow_mut(|scratch| *scratch = std::mem::take(&mut self.0));
    }
}

pub(super) struct CandidateRoutes<'a, 'b> {
    ground: &'a GroundKnowledge<'b>,
    candidate: PlacementFootprint,
    domain: DefenseDomain,
    paths: BTreeMap<(TilePos, TilePos), Vec<TilePos>>,
}

impl<'a, 'b> CandidateRoutes<'a, 'b> {
    pub(super) fn new(
        ground: &'a GroundKnowledge<'b>,
        candidate: PlacementFootprint,
        domain: DefenseDomain,
    ) -> Self {
        Self {
            ground,
            candidate,
            domain,
            paths: BTreeMap::new(),
        }
    }

    pub(super) fn context(&self) -> (&'a GroundKnowledge<'b>, PlacementFootprint, DefenseDomain) {
        (self.ground, self.candidate, self.domain)
    }

    pub(super) fn path(
        &mut self,
        start: TilePos,
        goal: TilePos,
        scratch: &mut AstarScratch,
    ) -> Option<Vec<TilePos>> {
        if let Some(path) = self.paths.get(&(start, goal)) {
            // A successful search hides any earlier exhausted-component proof.
            scratch.clear_search_evidence();
            return Some(path.clone());
        }
        let path = chassis::path::astar_with_scratch(
            self.ground.obs.map_width,
            self.ground.obs.map_height,
            start,
            goal,
            |tile| self.ground.open(tile, Some(self.candidate), self.domain),
            PATH_EXPANSION_CAP,
            scratch,
        );
        // Failed searches also carry reachability evidence; do not cache them
        // as a bare None and lose the distinction between exhaustion and a cap.
        if let Some(path) = &path {
            self.paths.insert((start, goal), path.clone());
        }
        path
    }
}
