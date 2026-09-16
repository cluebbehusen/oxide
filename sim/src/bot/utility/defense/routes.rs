use super::{DefenseDomain, GroundKnowledge, PlacementFootprint};
use crate::bot::navigation::paths::CandidatePaths;
#[cfg(test)]
pub(super) use crate::bot::navigation::search::Search as Scratch;
#[cfg(test)]
use chassis::grid::TilePos;

pub(super) struct CandidateRoutes<'a, 'b> {
    ground: &'a GroundKnowledge<'b>,
    candidate: PlacementFootprint,
    domain: DefenseDomain,
    pub paths: CandidatePaths<'a>,
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
            paths: CandidatePaths::new(
                super::routing_cache::board(ground, domain),
                super::routing_cache::overlay(candidate, domain),
            ),
        }
    }

    pub(super) fn context(&self) -> (&'a GroundKnowledge<'b>, PlacementFootprint, DefenseDomain) {
        (self.ground, self.candidate, self.domain)
    }

    #[cfg(test)]
    pub(super) fn path(
        &mut self,
        start: TilePos,
        goal: TilePos,
        scratch: &mut Scratch,
    ) -> Option<Vec<TilePos>> {
        self.paths.path(start, goal, scratch)
    }
}
