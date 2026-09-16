//! Local protection estimates for refits; no hypothetical army routes are needed.

use super::*;

pub(super) fn quote(
    context: &DefenseThinkContext<'_>,
    building: &BuildingObs,
    horizon: u64,
) -> (u64, DefenseOpportunityEvidence) {
    let absent = (0, DefenseOpportunityEvidence::PublicPrior);
    let Some(upgrade) = building.kind.upgrade_from(building.tier) else {
        return absent;
    };
    if building.kind == BuildingKind::Array {
        return (
            context.array_upgrade_value(building, horizon, u64::from(upgrade.build_ticks)),
            DefenseOpportunityEvidence::PublicPrior,
        );
    }
    let Some(profile) = DefenseProfile::for_kind(building.kind) else {
        return absent;
    };
    let terrain = context.briefing.regions().distances(building.anchor);
    let Some((origins, evidence)) = threat_origin_tiers(
        context.obs,
        context.unit_contacts,
        context.building_contacts,
        &context.grounding.public_starts,
        profile.domain,
    )
    .into_iter()
    .enumerate()
    .find_map(|(tier, origins)| {
        let origins = origins
            .into_iter()
            .filter(|origin| {
                profile.domain == DefenseDomain::Air || terrain.estimate(origin.anchor).is_some()
            })
            .collect::<Vec<_>>();
        let evidence = match tier {
            0 => DefenseOpportunityEvidence::CurrentArmed,
            1 if origins.iter().any(|origin| {
                matches!(origin.capability, ThreatCapability::StaticDefense { .. })
            }) =>
            {
                DefenseOpportunityEvidence::CurrentArmed
            }
            1 => DefenseOpportunityEvidence::CurrentFoothold,
            2 => DefenseOpportunityEvidence::Remembered,
            _ => DefenseOpportunityEvidence::PublicPrior,
        };
        (!origins.is_empty()).then_some((origins, evidence))
    }) else {
        return absent;
    };
    // Refits cannot retreat or be cancelled. Nearby current attackers take
    // precedence over the estimated long-term gain from this firing position.
    if evidence == DefenseOpportunityEvidence::CurrentArmed
        && origins.iter().any(|origin| {
            origin.anchor.chebyshev(building.anchor) <= DEFENSE_RADIUS
                || building_covers(
                    context.obs,
                    context.briefing,
                    building,
                    origin.anchor,
                    profile.domain,
                )
        })
    {
        return (0, evidence);
    }
    let mut upgraded = building.clone();
    upgraded.tier += 1;
    let dps = |building: &BuildingObs| {
        building
            .kind
            .tier_stats(building.tier)
            .weapons
            .iter()
            .filter(|weapon| weapon_targets_domain(weapon, profile.domain))
            .map(|weapon| {
                u64::from(weapon.damage)
                    .saturating_mul(u64::from(weapon.salvo))
                    .saturating_mul(100)
                    / u64::from(weapon.cooldown_ticks.max(1))
            })
            .fold(0, u64::saturating_add)
    };
    let old_dps = dps(building);
    let new_dps = dps(&upgraded);
    if new_dps == 0 {
        return (0, evidence);
    }
    let covers = |defense: &BuildingObs, tile| {
        building_covers(context.obs, context.briefing, defense, tile, profile.domain)
    };
    let existing = existing_defenses(context.obs, profile.domain);
    let active_ticks = horizon.saturating_sub(u64::from(upgrade.build_ticks));
    let mut value = 0_u64;
    for asset in &context.grounding.assets {
        let mut marginal = 0;
        for tile in asset
            .shape
            .approach_tiles(&context.grounding.ground, profile.domain)
        {
            if !covers(&upgraded, tile) {
                continue;
            }
            let before = if covers(building, tile) { old_dps } else { 0 };
            let gain = new_dps
                .saturating_mul(active_ticks)
                .saturating_sub(before.saturating_mul(horizon));
            let redundancy = existing
                .iter()
                .filter(|other| other.id != building.id && covers(other, tile))
                .count() as u64;
            let covered_value =
                u64::from(asset.value).saturating_mul(u64::from(UnitKind::Sentinel.stats().cost));
            marginal = marginal.max(
                covered_value.saturating_mul(gain)
                    / new_dps
                        .saturating_mul(horizon)
                        .saturating_mul(redundancy.saturating_add(1))
                        .max(1),
            );
        }
        value = value.saturating_add(marginal);
    }
    (
        value.saturating_mul(u64::from(building.hp))
            / u64::from(building.kind.tier_stats(building.tier).max_hp.max(1)),
        evidence,
    )
}
