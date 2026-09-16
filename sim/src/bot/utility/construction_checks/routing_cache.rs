use super::{GroundKnowledge, KnowledgeDomain, PlacementFootprint};

use crate::bot::navigation::{
    BlockedRect, KnownGrid,
    paths::{CacheClass, PathBoard},
};

pub(in crate::bot::utility) fn board<'a>(
    ground: &'a GroundKnowledge<'_>,
    domain: KnowledgeDomain,
) -> PathBoard<'a> {
    let (blocked, class) = match domain {
        KnowledgeDomain::Air => (&ground.air_blocked, CacheClass::Air),
        KnowledgeDomain::Ground => (
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

pub(in crate::bot::utility) fn overlay(
    candidate: PlacementFootprint,
    domain: KnowledgeDomain,
) -> Option<BlockedRect> {
    (domain == KnowledgeDomain::Ground && candidate.blocks_ground).then_some(BlockedRect {
        anchor: candidate.anchor,
        size: candidate.size,
    })
}
