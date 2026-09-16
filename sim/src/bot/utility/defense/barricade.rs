use super::*;
use crate::bot::navigation::{BlockedRect, KnownGrid, paths::PendingRoute};
use crate::bot::query_work::QueryPurpose;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ApproachCost {
    asset: usize,
    source: ThreatOrigin,
    cost: u32,
    baseline_cost: u32,
    disrupted: bool,
}

impl From<&Approach> for ApproachCost {
    fn from(approach: &Approach) -> Self {
        Self {
            asset: approach.asset,
            source: approach.source,
            cost: path_cost(&approach.path),
            baseline_cost: approach.baseline_cost,
            disrupted: approach.disrupted,
        }
    }
}

impl ApproachCost {
    fn detour(self) -> u32 {
        if self.disrupted {
            self.cost.saturating_sub(self.baseline_cost)
        } else {
            0
        }
    }
}

pub(super) fn supported_costs(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    baseline: &[Approach],
    candidate: PlacementFootprint,
    cache: &mut DefenseEvaluationCache,
) -> Option<Vec<ApproachCost>> {
    refine_supported_costs(ground, assets, baseline, candidate, cache, false)
        .expect("immediate barricade evaluation cannot defer")
}

pub(super) fn refine_supported_costs(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    baseline: &[Approach],
    candidate: PlacementFootprint,
    cache: &mut DefenseEvaluationCache,
    refine: bool,
) -> Result<Option<Vec<ApproachCost>>, PendingRoute> {
    cache_supported_assets(ground, assets, candidate, cache);
    if !cache.barricade_approaches.contains_key(&candidate) {
        let costs = costs_with_candidate(
            ground,
            assets,
            baseline,
            candidate,
            &mut cache.baseline_endpoint_routes,
            refine,
        )?;
        cache.barricade_approaches.insert(candidate, costs);
    }
    let Some(costs) = cache.barricade_approaches[&candidate].as_ref() else {
        return Ok(None);
    };
    // The detour limit also protects approaches to assets outside local support.
    if costs
        .iter()
        .any(|approach| approach.detour() > MAX_BARRICADE_DETOUR_COST)
    {
        return Ok(None);
    }
    let Some(supported) = cache.supported_assets.get(&candidate) else {
        return Ok(None);
    };
    Ok(Some(
        costs
            .iter()
            .copied()
            .filter(|approach| supported.contains(&approach.asset))
            .collect(),
    ))
}

fn costs_with_candidate(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    baseline: &[Approach],
    candidate: PlacementFootprint,
    endpoint_routes: &mut EndpointRoutes,
    refine: bool,
) -> Result<Option<Vec<ApproachCost>>, PendingRoute> {
    let domain = DefenseDomain::Ground;
    let grid = KnownGrid::new(
        ground.obs.map_width,
        ground.obs.map_height,
        &ground.ground_blocked,
    )
    .expect("ground knowledge covers the observed map");
    let overlay = candidate.blocks_ground.then_some(BlockedRect {
        anchor: candidate.anchor,
        size: candidate.size,
    });
    let mut routes = CandidateRoutes::new(ground, candidate, domain);
    if refine {
        routes = routes.with_planning();
    }
    let mut costs = Vec::with_capacity(baseline.len());
    for approach in baseline {
        let mut result = ApproachCost::from(approach);
        if candidate_affects_path(&approach.path, candidate, domain) {
            let asset = &assets[approach.asset].shape;
            let goals = asset.approach_tiles(ground, domain);
            let cost = if approach.source.capability == ThreatCapability::Foothold && !refine {
                ground
                    .routing()
                    .borrow_mut()
                    .costs
                    .between_sets(
                        QueryPurpose::BarricadeDetour,
                        grid,
                        overlay,
                        &approach.source.approach_tiles(ground, domain),
                        &goals,
                    )
                    .available_cost()
            } else {
                approach_path_refined(&mut routes, approach.source, asset, &goals, endpoint_routes)?
                    .map(|(_, path)| path_cost(&path))
            };
            let Some(cost) = cost else { return Ok(None) };
            result.cost = cost;
            result.disrupted = true;
        }
        if result.detour() > MAX_BARRICADE_DETOUR_COST {
            return Ok(None);
        }
        costs.push(result);
    }
    Ok(Some(costs))
}

pub(super) fn coverage(
    assets: &[DefendedAsset],
    planned: &[PlannedDefense],
    candidate: TilePos,
    approaches: impl Iterator<Item = ApproachCost>,
) -> Coverage {
    let mut coverage = Coverage::empty();
    let mut detours = vec![0; assets.len()];
    for approach in approaches {
        if let Some(best) = detours.get_mut(approach.asset) {
            *best = (*best).max(approach.detour());
        }
    }
    for (asset_index, asset) in assets.iter().enumerate() {
        let detour = detours[asset_index];
        if detour == 0 {
            continue;
        }
        coverage.new = coverage.new.saturating_add(asset.value);
        coverage.unplanned_new = coverage.unplanned_new.saturating_add(asset.value);
        coverage.protected_value = coverage.protected_value.saturating_add(asset.value);
        coverage.interception = coverage.interception.saturating_add(
            asset
                .value
                .saturating_mul(detour.div_ceil(10).min(INTERCEPTION_DEPTH as u32)),
        );
        coverage.lateral = 0;
        if planned.iter().any(|defense| {
            defense.profile.kind == BuildingKind::Barricade
                && defense.anchor.chebyshev(candidate) <= 2
        }) {
            coverage.planned_overlap = coverage
                .planned_overlap
                .saturating_add(asset.value.saturating_mul(detour.div_ceil(10)));
        }
    }
    coverage
}

pub(super) fn threat_summary(
    approaches: &[ApproachCost],
    evidence: DefenseOpportunityEvidence,
) -> Option<CandidateThreatSummary> {
    threat_summary_from_sources(
        approaches
            .iter()
            .filter(|approach| approach.detour() > 0)
            .map(|approach| (approach.source, approach.baseline_cost)),
        evidence,
    )
}
