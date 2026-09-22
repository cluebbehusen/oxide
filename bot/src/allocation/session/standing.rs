//! Standing-force alternatives for exact connected-operation ownership contexts.

use super::{ConnectedOffenseKey, ConnectedPortfolioContext, FreshConnectedProposal};
use crate::standing_force::{
    CapabilityDemand, StandingForceCommitment, StandingForceContext, StandingForceProposal,
    StandingGroundTarget, StandingProductionCommitment, derive_standing_force_with_demand,
};
use crate::{
    PublicMapBriefing, difficulty::DifficultyTuning, intelligence::StrategicIntelligence,
    observation::Observation, orient::Orientation, profile::ResolvedProfile,
    resources::ResourceSnapshot,
};
use chassis::grid::TilePos;
use oxide_sim::ids::UnitId;
use oxide_sim::stats::BuildingKind;
use std::collections::BTreeMap;

pub(super) struct StandingForceInputs<'a> {
    pub(super) observer: Option<&'a dyn crate::observer::PhaseObserver>,
    pub(super) observation: &'a Observation,
    pub(super) intelligence: &'a StrategicIntelligence,
    pub(super) profile: &'a ResolvedProfile,
    pub(super) tuning: DifficultyTuning,
    pub(super) resources: &'a ResourceSnapshot,
    pub(super) home: TilePos,
    pub(super) public_map: &'a PublicMapBriefing,
    pub(super) orientation: Orientation,
    pub(super) eligible: bool,
    pub(super) derivation: &'a StandingForceDerivation,
    pub(super) committed_production: &'a [StandingProductionCommitment],
    pub(super) funded_repairers: Vec<UnitId>,
    pub(super) saving: Option<&'a StandingForceCommitment>,
    pub(super) recon_demands: &'a [CapabilityDemand],
}

impl StandingForceInputs<'_> {
    pub(super) fn prepare(
        &self,
        base_exclusions: &[UnitId],
        connected: Option<&FreshConnectedProposal>,
    ) -> (StandingForcePreparation, Vec<CapabilityDemand>) {
        let Some(connected) = connected else {
            let (proposals, demands) = self.derive(base_exclusions, &[]);
            return (StandingForcePreparation::Unconditional(proposals), demands);
        };
        let plan = OwnershipContexts::new(base_exclusions, connected);
        let mut evaluated = Vec::with_capacity(plan.ownership.len());
        let mut capability_demands = None;
        for ownership in &plan.ownership {
            let (proposals, demands) = self.derive(&ownership.units, &ownership.production);
            // Economy uses the first context, even when it produces no demand.
            capability_demands.get_or_insert(demands);
            evaluated.push(proposals);
        }
        let contexts = plan
            .contexts
            .into_iter()
            .map(|(context, index)| ContextualStandingForce {
                context,
                proposals: evaluated[index].clone(),
            })
            .collect();
        (
            StandingForcePreparation::ConnectedContexts(contexts),
            capability_demands.unwrap_or_default(),
        )
    }

    pub(super) fn derive(
        &self,
        unit_exclusions: &[UnitId],
        connected_paid_production: &[StandingProductionCommitment],
    ) -> (Vec<StandingForceProposal>, Vec<CapabilityDemand>) {
        let _scope = crate::observer::PhaseScope::new(
            self.observer,
            crate::observer::BotPhase::StandingForce,
        );
        if !self.eligible {
            return (Vec::new(), Vec::new());
        }
        let owned_production =
            merged_production_commitments(self.committed_production, connected_paid_production);
        let mut context = StandingForceContext::new(unit_exclusions, &owned_production)
            .with_repair_work(&self.derivation.work.repair_work)
            .with_funded_repairers(&self.funded_repairers)
            .with_protection_work(&self.derivation.work.protection_work)
            .with_ground_routing(
                StandingGroundTarget::footprint(self.home, BuildingKind::Foundry.base_stats().size),
                Some(self.public_map),
                &self.derivation.projection_targets,
                Some(self.orientation),
            );
        if let Some((anchor, target_strength)) = self.derivation.expansion_security_need {
            context = context.with_expansion_security(
                StandingGroundTarget::footprint(anchor, BuildingKind::Foundry.base_stats().size),
                target_strength,
            );
        }
        let (mut proposals, mut demands) = derive_standing_force_with_demand(
            self.observation,
            self.intelligence,
            self.profile,
            self.tuning,
            self.resources,
            context,
        );
        if let Some(saving) = self.saving {
            proposals.retain(|proposal| {
                proposal.accumulation().is_none()
                    && !saving.covers(proposal.reason(), proposal.key().service)
            });
        }
        demands.extend_from_slice(self.recon_demands);
        if let Some(request) = self.derivation.work.raid.as_ref().filter(|request| {
            request
                .newly_claimed
                .iter()
                .all(|id| !unit_exclusions.contains(id))
        }) {
            proposals.push(StandingForceProposal::for_raid(
                request.clone().with_paid_ownership(&owned_production),
                self.profile,
            ));
        }
        (proposals, demands)
    }
}

struct StandingOwnership {
    units: Vec<UnitId>,
    production: Vec<StandingProductionCommitment>,
}

#[derive(Default)]
struct OwnershipContexts {
    ownership: Vec<StandingOwnership>,
    contexts: Vec<(ConnectedPortfolioContext, usize)>,
}

impl OwnershipContexts {
    fn new(base_exclusions: &[UnitId], proposal: &FreshConnectedProposal) -> Self {
        let mut plan = Self::default();
        if !proposal.revises_active_operation() {
            plan.push(ConnectedPortfolioContext::Absent, base_exclusions, &[]);
        }
        let key = ConnectedOffenseKey {
            objective: proposal.objective(),
            anchor: proposal.anchor(),
        };
        let mut units = base_exclusions.to_vec();
        let mut production = Vec::new();
        for (marginal_depth, claims) in std::iter::once(proposal.minimum_claims())
            .chain(
                proposal
                    .marginal_variants()
                    .iter()
                    .map(|variant| variant.additions()),
            )
            .enumerate()
        {
            units.extend_from_slice(claims.units());
            units.sort_unstable();
            units.dedup();
            production.extend(claims.paid_providers().iter().map(|provider| {
                StandingProductionCommitment::paid(provider.producer(), provider.kind())
            }));
            // Equal paid entries represent distinct queue occurrences.
            production.sort_unstable();
            plan.push(
                ConnectedPortfolioContext::Selected {
                    key,
                    marginal_depth,
                },
                &units,
                &production,
            );
        }
        plan
    }

    fn push(
        &mut self,
        context: ConnectedPortfolioContext,
        units: &[UnitId],
        production: &[StandingProductionCommitment],
    ) {
        let index = self
            .ownership
            .iter()
            .position(|owned| owned.units == units && owned.production == production)
            .unwrap_or_else(|| {
                let index = self.ownership.len();
                self.ownership.push(StandingOwnership {
                    units: units.to_vec(),
                    production: production.to_vec(),
                });
                index
            });
        self.contexts.push((context, index));
    }
}

#[derive(Default)]
pub(super) struct StandingForceDerivation {
    pub(super) projection_targets: Vec<StandingGroundTarget>,
    pub(super) expansion_security_need: Option<(TilePos, u64)>,
    pub(super) work: StandingForceWork,
}

#[derive(Default)]
pub(super) struct StandingForceWork {
    pub(super) repair_work: Vec<crate::standing_force::RepairWork>,
    pub(super) protection_work: Vec<crate::utility::ProtectionRequest>,
    pub(super) raid: Option<crate::raid::RaidProcurementRequest>,
}

pub(super) enum StandingForcePreparation {
    Unconditional(Vec<StandingForceProposal>),
    ConnectedContexts(Vec<ContextualStandingForce>),
}

impl Default for StandingForcePreparation {
    fn default() -> Self {
        Self::Unconditional(Vec::new())
    }
}

impl StandingForcePreparation {
    pub(super) fn for_each(&self, mut visit: impl FnMut(&StandingForceProposal)) {
        match self {
            Self::Unconditional(proposals) => proposals.iter().for_each(&mut visit),
            Self::ConnectedContexts(contexts) => contexts
                .iter()
                .flat_map(|context| &context.proposals)
                .for_each(visit),
        }
    }
}

pub(super) struct ContextualStandingForce {
    pub(super) context: ConnectedPortfolioContext,
    pub(super) proposals: Vec<StandingForceProposal>,
}

fn merged_production_commitments(
    retained: &[StandingProductionCommitment],
    contextual: &[StandingProductionCommitment],
) -> Vec<StandingProductionCommitment> {
    let multiplicities = |commitments: &[StandingProductionCommitment]| {
        commitments.iter().copied().fold(
            BTreeMap::<StandingProductionCommitment, usize>::new(),
            |mut counts, commitment| {
                let count = counts.entry(commitment).or_default();
                *count = count.saturating_add(1);
                counts
            },
        )
    };
    let retained = multiplicities(retained);
    let contextual = multiplicities(contextual);
    let mut keys = retained
        .keys()
        .chain(contextual.keys())
        .copied()
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter()
        .flat_map(|commitment| {
            let count = retained
                .get(&commitment)
                .copied()
                .unwrap_or_default()
                .max(contextual.get(&commitment).copied().unwrap_or_default());
            core::iter::repeat_n(commitment, count)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::{ids::BuildingId, stats::UnitKind};
    #[test]
    fn retained_and_contextual_paid_claims_use_multiset_union() {
        let bombard = StandingProductionCommitment::paid(BuildingId(11), UnitKind::Bombard);
        let moth = StandingProductionCommitment::paid(BuildingId(12), UnitKind::Moth);

        assert_eq!(
            merged_production_commitments(&[bombard, bombard], &[bombard, moth]),
            vec![bombard, bombard, moth],
            "an active revision must not double-own one occurrence, while distinct multiplicity remains exact"
        );
    }
}
