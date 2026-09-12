use super::*;

pub(super) struct Bounds<'a> {
    ground: &'a GroundKnowledge<'a>,
    goals: Vec<Vec<TilePos>>,
    approaches: &'a [Approach],
}

impl<'a> Bounds<'a> {
    pub(super) fn new(
        ground: &'a GroundKnowledge<'a>,
        assets: &[DefendedAsset],
        approaches: &'a [Approach],
    ) -> Self {
        Self {
            ground,
            goals: assets
                .iter()
                .map(|a| a.shape.approach_tiles(ground, DefenseDomain::Ground))
                .collect(),
            approaches,
        }
    }

    pub(super) fn best(
        &self,
        anchors: Vec<TilePos>,
        context: &CoverageContext<'_>,
        profile: DefenseProfile,
        origins: &[ThreatOrigin],
        mut evaluate: impl FnMut(TilePos) -> Option<(Candidate, UnitId)>,
    ) -> Option<(Candidate, UnitId)> {
        let mut candidates: Vec<_> = anchors
            .into_iter()
            .map(|anchor| self.candidate(context, profile, anchor, origins))
            .collect();
        candidates.sort_by_key(|candidate| candidate.key(profile));
        let mut selected: Option<(Candidate, UnitId)> = None;
        for bound in candidates.into_iter().rev() {
            if (bound.coverage.new == 0 && bound.coverage.reinforced == 0)
                || selected.is_some_and(|(best, _)| bound.key(profile) < best.key(profile))
            {
                continue;
            }
            if let Some(candidate) = evaluate(bound.anchor)
                && selected.is_none_or(|(best, _)| candidate.0.key(profile) >= best.key(profile))
            {
                selected = Some(candidate);
            }
        }
        selected
    }

    pub(super) fn candidate(
        &self,
        context: &CoverageContext<'_>,
        profile: DefenseProfile,
        anchor: TilePos,
        origins: &[ThreatOrigin],
    ) -> Candidate {
        let placement = profile.footprint(anchor);
        let doorsteps = building_doorsteps(self.ground, anchor, placement.size);
        let possible: Vec<_> = self
            .goals
            .iter()
            .map(|goals| {
                doorsteps
                    .iter()
                    .any(|s| goals.iter().any(|g| s.chebyshev(*g) <= DEFENSE_RADIUS))
            })
            .collect();
        let mut uncertain = vec![false; context.assets.len()];
        for approach in self.approaches {
            if possible[approach.asset]
                && candidate_affects_path(&approach.path, placement, profile.domain)
            {
                uncertain[approach.asset] = true;
            }
        }
        let unchanged: Vec<_> = self
            .approaches
            .iter()
            .filter(|a| possible[a.asset] && !uncertain[a.asset])
            .cloned()
            .collect();
        let mut coverage = score_coverage(
            &CoverageContext {
                approaches: &unchanged,
                ..*context
            },
            profile,
            anchor,
        );
        // Removing unsupported assets can remove any penalty. Rerouted assets
        // may attain every positive component, up to its scoring saturation.
        coverage.planned_overlap = 0;
        coverage.redundant = 0;
        coverage.blind_exposure = 0;
        coverage.lateral = 0;
        for (asset, uncertain) in context.assets.iter().zip(uncertain) {
            if !uncertain {
                continue;
            }
            coverage.new = coverage.new.saturating_add(asset.value);
            coverage.reinforced = coverage.reinforced.saturating_add(asset.value);
            coverage.unplanned_new = coverage.unplanned_new.saturating_add(asset.value);
            coverage.unplanned_reinforced =
                coverage.unplanned_reinforced.saturating_add(asset.value);
            coverage.protected_value = coverage.protected_value.saturating_add(asset.value);
            coverage.spotted_reach = coverage.spotted_reach.saturating_add(asset.value);
            coverage.interception = coverage
                .interception
                .saturating_add(asset.value.saturating_mul(INTERCEPTION_DEPTH as u32));
        }
        Candidate {
            anchor,
            builder_travel: 0,
            coverage,
            threat_distance: origins
                .iter()
                .map(|o| o.anchor.manhattan(anchor))
                .min()
                .unwrap_or(i32::MAX),
        }
    }
}
