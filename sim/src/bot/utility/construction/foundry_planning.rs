//! Rank resource regions before pricing exact Foundry footprints.

use super::*;
use std::collections::BTreeSet;

const EXACT_SITES: usize = 8;

impl UtilityPolicy {
    pub(super) fn regional_foundry_opportunities(
        &self,
        obs: &Observation,
        context: FoundryClaimContext<'_>,
        public_map: &PublicMapBriefing,
        economy: expansion::ExpansionEconomy,
        danger: &danger::HarvestDangerProjection,
        required: Option<TilePos>,
    ) -> Vec<expansion::FoundryOpportunity> {
        let FoundryClaimContext {
            home,
            projected_foundries,
            support_extractors,
            ordinary_frontiers,
            ..
        } = context;
        if !support_extractors && !ordinary_frontiers {
            return Vec::new();
        }
        let mut road_reach = None;
        let unsupported = if support_extractors {
            Self::unsupported_extractors(obs, home, projected_foundries, &mut road_reach)
                .into_iter()
                .filter(|tile| !self.harvest_location_contested(*tile))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let scraps = obs
            .known_scrap
            .iter()
            .copied()
            .filter(|(tile, amount)| {
                *amount > 0
                    && obs.visible(*tile)
                    && !self.dead_nodes.contains(tile)
                    && !self.harvest_location_contested(*tile)
                    && road_reach
                        .get_or_insert_with(|| Self::known_road_reach(obs, home))
                        .frame_reached(*tile)
                    && !Self::enemy_controls_frontier(obs, projected_foundries, *tile)
            })
            .collect::<Vec<_>>();
        if unsupported.is_empty() && scraps.is_empty() {
            return Vec::new();
        }
        let size = BuildingKind::Foundry.base_stats().size;
        self.prepare_ground_producer_egress(obs);
        let supported = |anchor| {
            unsupported
                .iter()
                .filter(|&&extractor| Self::foundry_supports_extractor(anchor, extractor))
                .count() as u32
        };
        let anchors = if let Some(required) = required {
            vec![required]
        } else {
            let regions = public_map.regions();
            let clusters = regions.clusters(
                scraps
                    .iter()
                    .map(|(tile, _)| *tile)
                    .chain(unsupported.iter().copied()),
            );
            let amounts = scraps
                .iter()
                .copied()
                .collect::<std::collections::BTreeMap<_, _>>();
            let estimates = clusters
                .iter()
                .filter_map(|cluster| {
                    let amount = cluster
                        .iter()
                        .filter_map(|tile| amounts.get(tile))
                        .copied()
                        .fold(0_u32, u32::saturating_add);
                    if amount == 0 {
                        return None;
                    }
                    let field = regions.distances(cluster[0]);
                    let old = projected_foundries
                        .iter()
                        .filter_map(|&anchor| field.estimate(anchor))
                        .min();
                    Some((amount, old, field))
                })
                .collect::<Vec<_>>();
            let mut selected = Vec::new();
            for cluster in clusters {
                let count = i64::try_from(cluster.len()).unwrap_or(i64::MAX).max(1);
                let center = TilePos::new(
                    (cluster.iter().map(|tile| i64::from(tile.x)).sum::<i64>() / count) as i32,
                    (cluster.iter().map(|tile| i64::from(tile.y)).sum::<i64>() / count) as i32,
                );
                let radius = cluster
                    .iter()
                    .map(|tile| tile.chebyshev(center))
                    .max()
                    .unwrap_or(0)
                    + crate::stats::EXTRACTOR_SUPPORT_RADIUS
                    + size.0.max(size.1);
                let mut candidates = BTreeSet::new();
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        let anchor = center.offset(dx - size.0 / 2, dy - size.1 / 2);
                        if self.placement_valid_prepared(obs, BuildingKind::Foundry, anchor) {
                            candidates.insert(anchor);
                        }
                    }
                }
                let best = candidates
                    .into_iter()
                    .map(|anchor| {
                        let mut scrap = expansion::ScrapSummary::default();
                        for (amount, old, field) in &estimates {
                            if let (Some(old_distance), Some(new_distance)) =
                                (*old, field.estimate(anchor))
                                && new_distance < old_distance
                            {
                                scrap.include(expansion::ScrapLogistics {
                                    amount: *amount,
                                    old_distance,
                                    new_distance,
                                });
                            }
                        }
                        expansion::FoundryOpportunity::quote_summary(
                            anchor,
                            supported(anchor),
                            scrap,
                            economy,
                        )
                    })
                    .max_by_key(|quote| {
                        (
                            quote.projected_return,
                            quote.recurring_gain_per_minute,
                            std::cmp::Reverse(quote.anchor.y),
                            std::cmp::Reverse(quote.anchor.x),
                        )
                    });
                if let Some(best) = best {
                    selected.push(best);
                }
            }
            let mut seen = BTreeSet::new();
            let mut ranked = expansion::rank_foundry_opportunities(selected)
                .into_iter()
                .filter(|quote| seen.insert(quote.anchor))
                .map(|quote| quote.anchor)
                .collect::<Vec<_>>();
            if ranked.len() > EXACT_SITES {
                let remaining = ranked.len() - 1;
                let offset = usize::try_from((obs.tick / 24) % remaining as u64).unwrap_or(0);
                ranked[1..].rotate_left(offset);
                ranked.truncate(EXACT_SITES);
            }
            ranked
        };
        if anchors.is_empty() {
            return Vec::new();
        }
        let blocked = self.foundry_logistics_blocked_layout(public_map, danger);
        let mut routes = self.expansion_routing_cache.borrow_mut();
        let previous = (!scraps.is_empty()).then(|| {
            routes.danger_aware_source_set(
                public_map,
                &blocked,
                projected_foundries.iter().flat_map(|&anchor| {
                    (0..size.1).flat_map(move |dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
                }),
            )
        });
        let mut opportunities = Vec::new();
        for anchor in anchors {
            let mut scrap = expansion::ScrapSummary::default();
            if let Some(previous) = &previous {
                let next = routes.danger_aware_source_set(
                    public_map,
                    &blocked,
                    (0..size.1).flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy))),
                );
                for &(tile, amount) in &scraps {
                    if let (Some(old_distance), Some(new_distance)) = (
                        previous.footprint_distance(tile, (1, 1)),
                        next.footprint_distance(tile, (1, 1)),
                    ) && new_distance < old_distance
                    {
                        scrap.include(expansion::ScrapLogistics {
                            amount,
                            old_distance,
                            new_distance,
                        });
                    }
                }
            }
            if let Some(quote) = expansion::FoundryOpportunity::admitted_summary(
                anchor,
                supported(anchor),
                ordinary_frontiers,
                scrap,
                economy,
            ) {
                opportunities.push(quote);
            }
        }
        expansion::rank_foundry_opportunities(opportunities)
    }
}
