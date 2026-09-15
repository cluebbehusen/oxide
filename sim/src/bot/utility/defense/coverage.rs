//! Shared approach-tile geometry for a batch of candidate defense sites.

use super::*;

struct TileFacts {
    position: TilePos,
    existing: u8,
    ground_covered: bool,
    planned: bool,
    spotter: bool,
}

struct Sample {
    tile: usize,
    count: u32,
    depth: u32,
}

pub(super) struct Batch<'a> {
    assets_values: &'a [DefendedAsset],
    briefing: &'a PublicMapBriefing,
    profile: DefenseProfile,
    tiles: Vec<TileFacts>,
    assets: Vec<Vec<Sample>>,
}

impl<'a> Batch<'a> {
    pub(super) fn new(context: &CoverageContext<'a>, profile: DefenseProfile) -> Self {
        let mut indices = BTreeMap::new();
        let mut tiles = Vec::new();
        let mut assets: Vec<BTreeMap<usize, Sample>> =
            (0..context.assets.len()).map(|_| BTreeMap::new()).collect();
        for approach in context.approaches {
            for (offset, position) in approach.path.iter().copied().enumerate() {
                let tile = *indices.entry(position).or_insert_with(|| {
                    let index = tiles.len();
                    let existing = context
                        .existing
                        .iter()
                        .filter(|building| {
                            building_covers(
                                context.obs,
                                context.briefing,
                                building,
                                position,
                                profile.domain,
                            )
                        })
                        .take(2)
                        .count() as u8;
                    let ground_covered = if profile.domain == DefenseDomain::Ground {
                        existing > 0
                    } else {
                        profile.kind == BuildingKind::Bastion
                            && context.existing.iter().any(|building| {
                                building_covers(
                                    context.obs,
                                    context.briefing,
                                    building,
                                    position,
                                    DefenseDomain::Ground,
                                )
                            })
                    };
                    tiles.push(TileFacts {
                        position,
                        existing,
                        ground_covered,
                        planned: context.planned.iter().any(|defense| {
                            planned_defense_covers(context.obs, context.briefing, defense, position)
                        }),
                        spotter: profile.kind == BuildingKind::Bastion
                            && durable_spotter_sees(context.obs, position),
                    });
                    index
                });
                let sample = assets[approach.asset].entry(tile).or_insert(Sample {
                    tile,
                    count: 0,
                    depth: 0,
                });
                sample.count = sample.count.saturating_add(1);
                sample.depth = sample.depth.max(
                    approach
                        .path
                        .len()
                        .saturating_sub(1)
                        .saturating_sub(offset)
                        .min(INTERCEPTION_DEPTH) as u32,
                );
            }
        }
        Self {
            assets_values: context.assets,
            briefing: context.briefing,
            profile,
            tiles,
            assets: assets
                .into_iter()
                .map(|samples| samples.into_values().collect())
                .collect(),
        }
    }

    pub(super) fn score(&self, candidate: TilePos, enabled: impl Fn(usize) -> bool) -> Coverage {
        let profile = self.profile;
        let mut geometry = vec![None; self.tiles.len()];
        let mut coverage = Coverage::empty();
        for (asset_index, asset) in self.assets_values.iter().enumerate() {
            if !enabled(asset_index) {
                continue;
            }

            let mut protects = false;
            let mut adds_new = false;
            let mut reinforces = false;
            let mut adds_unplanned_new = false;
            let mut adds_unplanned_reinforcement = false;
            let mut planned_overlap_tiles = 0u32;
            let mut live_overlap_tiles = 0u32;
            let mut blind_exposure = false;
            let mut uses_spotter = false;
            let mut best_depth = 0;
            for sample in &self.assets[asset_index] {
                let facts = &self.tiles[sample.tile];
                let tile = facts.position;
                let (covers, blind, spotted) = *geometry[sample.tile].get_or_insert_with(|| {
                    let sees = profile.kind != BuildingKind::Bastion
                        || candidate_sees(profile, candidate, tile);
                    (
                        defense_covers(self.briefing, profile, candidate, tile)
                            && (sees || facts.spotter),
                        profile.kind == BuildingKind::Bastion
                            && inside_minimum_range(profile, candidate, tile)
                            && !facts.ground_covered,
                        !sees,
                    )
                });
                blind_exposure |= blind;
                if !covers {
                    continue;
                }
                protects = true;
                coverage.lateral = coverage.lateral.min(tile.chebyshev(candidate));
                uses_spotter |= spotted;
                if facts.planned {
                    planned_overlap_tiles = planned_overlap_tiles.saturating_add(sample.count);
                }
                match facts.existing {
                    0 => {
                        adds_new = true;
                        adds_unplanned_new |= !facts.planned;
                    }
                    1 => {
                        reinforces = true;
                        adds_unplanned_reinforcement |= !facts.planned;
                        live_overlap_tiles = live_overlap_tiles.saturating_add(sample.count);
                    }
                    _ => live_overlap_tiles = live_overlap_tiles.saturating_add(sample.count),
                }
                best_depth = best_depth.max(sample.depth);
            }
            if protects {
                coverage.protected_value = coverage.protected_value.saturating_add(asset.value);
                if adds_new {
                    coverage.new = coverage.new.saturating_add(asset.value);
                } else if reinforces {
                    coverage.reinforced = coverage.reinforced.saturating_add(asset.value);
                }
                if adds_unplanned_new {
                    coverage.unplanned_new = coverage.unplanned_new.saturating_add(asset.value);
                } else if adds_unplanned_reinforcement {
                    coverage.unplanned_reinforced =
                        coverage.unplanned_reinforced.saturating_add(asset.value);
                }
                if planned_overlap_tiles > 0 {
                    coverage.planned_overlap = coverage.planned_overlap.saturating_add(
                        asset
                            .value
                            .saturating_mul(planned_overlap_tiles.min(INTERCEPTION_DEPTH as u32)),
                    );
                }
                if live_overlap_tiles > 0 {
                    coverage.redundant = coverage.redundant.saturating_add(
                        asset
                            .value
                            .saturating_mul(live_overlap_tiles.min(INTERCEPTION_DEPTH as u32)),
                    );
                }
                if blind_exposure {
                    coverage.blind_exposure = coverage.blind_exposure.saturating_add(asset.value);
                }
                if uses_spotter {
                    coverage.spotted_reach = coverage.spotted_reach.saturating_add(asset.value);
                }
                coverage.interception = coverage
                    .interception
                    .saturating_add(asset.value.saturating_mul(best_depth));
            }
        }
        coverage
    }
}
