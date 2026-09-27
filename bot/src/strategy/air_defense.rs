//! Anti-air assessment of operation clusters and flight corridors.

use super::*;

const APPROACH_TILES: i32 = 3;

const MOBILE_AA_EXPOSURE_TICKS: u64 = 200;

const MOBILE_AA_SURVIVAL_MARGIN: u64 = 2;

const DEDICATED_MOBILE_AA_WEIGHT: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AirborneCorridorStatus {
    Clear,
    NeedsRecon,
    Defended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ClusterAirDefense {
    pub(super) has_targets: bool,
    targetable: Option<Target>,
    pub(super) evidence: AirDefenseEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SuppressionEngagement {
    pub(super) target: Target,
    pub(super) firing_stands: Vec<(UnitId, TilePos)>,
}

/// The connected cluster's anti-air assessment and the anti-air target the
/// operation must suppress first, if any.
pub(super) fn stage_air_defense(
    op: &AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
) -> (Option<ClusterAirDefense>, Option<Target>) {
    if plan.airborne() {
        let flak = targetable_corridor_flak(
            context.ev.intel,
            context.ev.home,
            op.target,
            context.landing_sites,
        );
        (None, flak.map(Target::Building))
    } else {
        let assessment = cluster_air_defense(op, plan, context.ev.intel);
        (Some(assessment), assessment.targetable)
    }
}

pub(super) fn prosecutable_cluster_air_defense_target(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
    orientation: Orientation,
) -> Option<SuppressionEngagement> {
    let cluster = operation_target_cluster(op, plan, intel);
    let mut targets = target_cluster_air_defense(intel, &cluster)
        .sources
        .into_iter()
        .filter(|source| source.evidence == ContactEvidence::Current)
        .filter_map(|source| current_aa_contact(intel, source.source)?.suppression_target());
    targets.find_map(|target| {
        artillery_firing_assignments(obs, intel, &op.artillery, target, public_map, orientation)
            .map(|firing_stands| SuppressionEngagement {
                target,
                firing_stands,
            })
    })
}

pub(super) fn cluster_air_defense(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &StrategicIntelligence,
) -> ClusterAirDefense {
    let cluster = operation_target_cluster(op, plan, intel);
    let has_targets = !cluster.is_empty();
    let assessment = target_cluster_air_defense(intel, &cluster);
    let mut current_coverage = false;
    let mut remembered_coverage = false;
    let mut targetable = None;

    for source in assessment.sources {
        match source.evidence {
            ContactEvidence::Current => {
                if let Some(contact) = current_aa_contact(intel, source.source) {
                    current_coverage = true;
                    targetable = targetable.or_else(|| contact.suppression_target());
                }
            }
            ContactEvidence::Remembered if source.confidence > 0 => {
                remembered_coverage = true;
            }
            ContactEvidence::Remembered => {}
        }
    }

    let evidence = if current_coverage {
        AirDefenseEvidence::CurrentCoverage
    } else if remembered_coverage {
        AirDefenseEvidence::RememberedCoverage
    } else if assessment.all_target_tiles_visible {
        AirDefenseEvidence::VisibleWithoutKnownCoverage
    } else {
        AirDefenseEvidence::Unknown
    };

    ClusterAirDefense {
        has_targets,
        targetable,
        evidence,
    }
}

pub(super) fn targetable_flak(aa: &AirDefenseAssessment) -> Option<BuildingId> {
    aa.sources.iter().find_map(|source| {
        if source.evidence == ContactEvidence::Current
            && let AirDefenseSource::Building {
                id: Some(id),
                kind: BuildingKind::FlakTurret,
                ..
            } = source.source
        {
            Some(id)
        } else {
            None
        }
    })
}

pub(super) fn targetable_corridor_flak(
    intel: &StrategicIntelligence,
    home: TilePos,
    target: TilePos,
    landing_sites: &[TilePos],
) -> Option<BuildingId> {
    flight_objectives(target, landing_sites)
        .into_iter()
        .flat_map(|objective| flight_corridor(home, objective))
        .find_map(|tile| targetable_flak(&intel.air_defense_at(tile)))
}

pub(super) fn corridor_clear(
    intel: &StrategicIntelligence,
    home: TilePos,
    target: TilePos,
    landing_sites: &[TilePos],
) -> bool {
    flight_objectives(target, landing_sites)
        .into_iter()
        .all(|objective| {
            let known_route_is_clear = flight_corridor(home, objective).into_iter().all(|tile| {
                matches!(
                    intel.air_defense_at(tile).evidence(),
                    AirDefenseEvidence::VisibleWithoutKnownCoverage | AirDefenseEvidence::Unknown
                )
            });
            known_route_is_clear
                && approach(home, objective).all(|tile| {
                    intel.air_defense_at(tile).evidence()
                        == AirDefenseEvidence::VisibleWithoutKnownCoverage
                })
        })
}

pub(super) fn route_without_known_air_defense(
    intel: &StrategicIntelligence,
    home: TilePos,
    target: TilePos,
) -> bool {
    flight_corridor(home, target)
        .into_iter()
        .chain(approach(home, target))
        .all(|tile| {
            matches!(
                intel.air_defense_at(tile).evidence(),
                AirDefenseEvidence::VisibleWithoutKnownCoverage | AirDefenseEvidence::Unknown
            )
        })
}

pub(super) fn flight_objectives(target: TilePos, landing_sites: &[TilePos]) -> Vec<TilePos> {
    let mut objectives = vec![target];
    objectives.extend_from_slice(landing_sites);
    objectives.sort_unstable_by_key(|tile| (tile.y, tile.x));
    objectives.dedup();
    objectives
}

pub(super) fn airborne_corridor_status(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    landing_sites: &[TilePos],
) -> AirborneCorridorStatus {
    let objectives = flight_objectives(op.target, landing_sites);
    let mut mobile_sources = std::collections::BTreeMap::new();

    for assessment in objectives
        .iter()
        .flat_map(|objective| flight_corridor(home, *objective))
        .map(|tile| intel.air_defense_at(tile))
    {
        for source in assessment
            .sources
            .iter()
            .filter(|source| source.evidence == ContactEvidence::Current)
        {
            match source.source {
                AirDefenseSource::Building { .. } => return AirborneCorridorStatus::Defended,
                AirDefenseSource::Unit { id, kind, .. } => {
                    mobile_sources
                        .entry(id)
                        .and_modify(|entry: &mut (UnitKind, u32)| {
                            entry.1 = entry.1.max(source.firepower_per_100_ticks);
                        })
                        .or_insert((kind, source.firepower_per_100_ticks));
                }
            }
        }
    }

    let wing_hp = air_strike_members(op, plan, obs)
        .into_iter()
        .filter_map(|id| unit(obs, id))
        .fold(0u64, |total, member| {
            total.saturating_add(u64::from(member.hp))
        });
    let mobile_firepower = mobile_sources
        .values()
        .fold(0u64, |total, (kind, firepower)| {
            let weight = if kind.role() == Role::AntiAir {
                DEDICATED_MOBILE_AA_WEIGHT
            } else {
                1
            };
            total.saturating_add(u64::from(*firepower).saturating_mul(weight))
        });
    let projected_damage = mobile_firepower
        .saturating_mul(MOBILE_AA_EXPOSURE_TICKS)
        .div_ceil(100);
    if projected_damage.saturating_mul(MOBILE_AA_SURVIVAL_MARGIN) > wing_hp {
        return AirborneCorridorStatus::Defended;
    }

    let route_is_clear = |assessment: &AirDefenseAssessment| match assessment.evidence() {
        AirDefenseEvidence::VisibleWithoutKnownCoverage | AirDefenseEvidence::Unknown => true,
        AirDefenseEvidence::CurrentCoverage => current_mobile_coverage_is_fresh(assessment),
        AirDefenseEvidence::RememberedCoverage => false,
    };
    let approach_is_clear = |assessment: &AirDefenseAssessment| {
        assessment.target_visible
            && match assessment.evidence() {
                AirDefenseEvidence::VisibleWithoutKnownCoverage => true,
                AirDefenseEvidence::CurrentCoverage => current_mobile_coverage_is_fresh(assessment),
                AirDefenseEvidence::RememberedCoverage | AirDefenseEvidence::Unknown => false,
            }
    };
    let clear = objectives.into_iter().all(|objective| {
        flight_corridor(home, objective)
            .into_iter()
            .map(|tile| intel.air_defense_at(tile))
            .all(|assessment| route_is_clear(&assessment))
            && approach(home, objective)
                .map(|tile| intel.air_defense_at(tile))
                .all(|assessment| approach_is_clear(&assessment))
    });
    if clear {
        AirborneCorridorStatus::Clear
    } else {
        AirborneCorridorStatus::NeedsRecon
    }
}

fn current_mobile_coverage_is_fresh(assessment: &AirDefenseAssessment) -> bool {
    assessment
        .sources
        .iter()
        .all(|source| match source.evidence {
            ContactEvidence::Current => matches!(source.source, AirDefenseSource::Unit { .. }),
            ContactEvidence::Remembered => source.confidence == 0,
        })
}

fn flight_corridor(home: TilePos, target: TilePos) -> Vec<TilePos> {
    let mut tiles = Vec::new();
    let mut current = home;
    let dx = (target.x - home.x).abs();
    let step_x = (target.x - home.x).signum();
    let dy = -(target.y - home.y).abs();
    let step_y = (target.y - home.y).signum();
    let mut error = dx + dy;
    loop {
        tiles.push(current);
        if current == target {
            break;
        }
        let twice_error = error.saturating_mul(2);
        if twice_error >= dy {
            error += dy;
            current.x += step_x;
        }
        if twice_error <= dx {
            error += dx;
            current.y += step_y;
        }
    }
    tiles
}

pub(super) fn approach(home: TilePos, target: TilePos) -> impl Iterator<Item = TilePos> {
    let dx = (home.x - target.x).signum();
    let dy = (home.y - target.y).signum();
    (0..=APPROACH_TILES).map(move |step| target.offset(dx * step, dy * step))
}

#[derive(Debug, Default)]
pub(super) struct CurrentSuppressionNeeds {
    pub(super) targets: Vec<Target>,
    pub(super) has_untargetable_current: bool,
}

pub(super) fn current_cluster_suppression_needs(
    intel: &StrategicIntelligence,
    cluster: &[&BuildingContact],
) -> CurrentSuppressionNeeds {
    let mut needs = CurrentSuppressionNeeds::default();
    for source in target_cluster_air_defense(intel, cluster).sources {
        if source.evidence != ContactEvidence::Current {
            continue;
        }
        let Some(contact) = current_aa_contact(intel, source.source) else {
            continue;
        };
        match contact.suppression_target() {
            Some(target) => needs.targets.push(target),
            None => needs.has_untargetable_current = true,
        }
    }
    needs.targets.sort_unstable();
    needs.targets.dedup();
    needs
}
