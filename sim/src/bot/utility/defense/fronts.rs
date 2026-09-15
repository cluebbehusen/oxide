//! Representative mobile approaches for voluntary investment valuation.

use super::*;

pub(super) fn approaches(
    ground: &GroundKnowledge<'_>,
    origins: &[ThreatOrigin],
    assets: &[DefendedAsset],
    domain: DefenseDomain,
) -> crate::bot::planning::Progress<Vec<Approach>> {
    let regions = ground.briefing.regions();
    let mut result = Vec::new();
    for (asset, defended) in assets.iter().enumerate() {
        let goals = defended.shape.approach_tiles(ground, domain);
        let mut groups = BTreeMap::<(ThreatCapability, usize, i32, i32), Vec<ThreatOrigin>>::new();
        let mut individual = Vec::new();
        for &source in origins {
            let Some(goal) = goals
                .iter()
                .min_by_key(|goal| (source.anchor.chebyshev(**goal), goal.y, goal.x))
            else {
                continue;
            };
            let region = match domain {
                DefenseDomain::Ground => regions.region_at(source.anchor),
                DefenseDomain::Air => Some(
                    (source.anchor.y.div_euclid(16)
                        * (ground.obs.map_width as u32).div_ceil(16) as i32
                        + source.anchor.x.div_euclid(16)) as usize,
                ),
            };
            if matches!(source.capability, ThreatCapability::Mobile(_))
                && let Some(region) = region
            {
                groups
                    .entry((
                        source.capability,
                        region,
                        (source.anchor.x - goal.x).signum(),
                        (source.anchor.y - goal.y).signum(),
                    ))
                    .or_default()
                    .push(source);
            } else {
                individual.push(vec![source]);
            }
        }
        individual.extend(groups.into_values().map(|mut sources| {
            sources.sort_by_key(|source| {
                (
                    goals
                        .iter()
                        .map(|goal| source.anchor.chebyshev(*goal))
                        .min(),
                    source.anchor.y,
                    source.anchor.x,
                    source.tie,
                )
            });
            sources
        }));
        let field = std::cell::OnceCell::new();
        let mut selected = Vec::new();
        for sources in individual {
            if let Some(approach) = sources.into_iter().find_map(|source| {
                approach_path(
                    ground,
                    source,
                    &defended.shape,
                    &goals,
                    None,
                    domain,
                    Some(&field),
                )
                .map(|(goal, path)| Approach {
                    asset,
                    source,
                    goal,
                    baseline_cost: path_cost(&path),
                    path,
                    disrupted: false,
                })
            }) {
                selected.push(approach);
            }
        }
        if matches!(field.get(), Some(crate::bot::planning::Progress::Deferred)) {
            return crate::bot::planning::Progress::Deferred;
        }
        selected.sort_by_key(|approach| approach.source);
        result.extend(selected);
    }
    crate::bot::planning::Progress::Ready(result)
}
