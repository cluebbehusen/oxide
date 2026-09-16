use super::{DefenseDomain, GroundKnowledge, PlacementFootprint};
pub(in crate::bot::utility) use crate::bot::navigation::paths::PathQueries as DefenseRoutingCache;
use crate::bot::navigation::{
    BlockedRect, KnownGrid,
    paths::{CacheClass, PathBoard},
};

pub(super) fn board<'a>(ground: &'a GroundKnowledge<'_>, domain: DefenseDomain) -> PathBoard<'a> {
    let (blocked, class) = match domain {
        DefenseDomain::Air => (&ground.air_blocked, CacheClass::Air),
        DefenseDomain::Ground => (
            &ground.ground_blocked,
            if ground.hypothetical {
                CacheClass::Hypothetical
            } else {
                CacheClass::Ground
            },
        ),
    };
    PathBoard {
        query_purpose: ground.query_purpose,
        grid: KnownGrid::new(ground.obs.map_width, ground.obs.map_height, blocked)
            .expect("knowledge covers the map"),
        class,
        cache: ground.routing(),
    }
}

pub(super) fn overlay(candidate: PlacementFootprint, domain: DefenseDomain) -> Option<BlockedRect> {
    (domain == DefenseDomain::Ground && candidate.blocks_ground).then_some(BlockedRect {
        anchor: candidate.anchor,
        size: candidate.size,
    })
}

#[cfg(test)]
mod tests;
