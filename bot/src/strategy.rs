//! One persistent, fog-honest strategic playbook.
//!
//! This is deliberately not a generic planner. It coordinates reconnaissance,
//! suppression, and an opportunity-scaled strike package, freezes exact members
//! at tactical commitment, then brings survivors home. Normal operations use
//! ground artillery; mature island stalemates can instead mass air attackers
//! against visible flak. Persistent membership prevents ordinary drafting from
//! turning either operation into a trickle attack.

use super::briefing::PublicMapBriefing;
use super::difficulty::{DifficultyTuning, strategic_admission_tick};
use super::executive::Intent;
use super::experience::{Outcome, OutcomeReason};
use super::intelligence::{
    AirDefenseAssessment, AirDefenseEvidence, AirDefenseSource, BuildingContact, ContactEvidence,
    StrategicIntelligence,
};
use super::navigation::commands::{self as routing, RouteProjection, production_spawn_doorstep};
use super::observation::{Observation, UnitObs};
use super::orient::Orientation;
use super::profile::ResolvedProfile;
use super::resources::{
    PaidQueueClaim, ProducerLaneReservations, ProductionAccess, ResourceSnapshot,
    count_paid_queued_ready_with_access, paid_queued_ready_occurrences_with_access,
};
use crate::production::ProductionPlan;
use crate::query_work::QueryPurpose;
use chassis::Tick;
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::TilePos;
use core::cmp::Reverse;
use oxide_sim::ids::{BuildingId, PlayerId, Target, UnitId};
use oxide_sim::scenario::BotStance;
use oxide_sim::stats::{BuildingKind, Domain, QUEUE_CAP, Role, UnitKind, WeaponStats};
use std::collections::BTreeMap;

mod air_defense;
mod campaign_routes;
mod connected;
#[cfg(test)]
pub(crate) mod fixtures;
pub(super) mod force_package;
mod geometry;
mod lifecycle;
mod operation;
mod resources;
mod roster;
mod stages;
mod targeting;
mod turn;

use air_defense::*;
use campaign_routes::CampaignRoutes;
use connected::*;
use geometry::*;
use lifecycle::*;
use operation::*;
use resources::*;
use roster::*;
use stages::*;
use targeting::*;

#[cfg(test)]
pub(crate) use connected::airworks_package_derivations;
pub(crate) use connected::{
    ActiveConnectedObligation, ConnectedConfidence, ConnectedExecutionSafety,
    ConnectedMarginalVariant, ConnectedOffenseClaims, ConnectedOpportunityCase,
    ConnectedPlanRejection, ConnectedProviderJob, ConnectedStrategicValue, ConnectedTimeToImpact,
    ConnectedUrgency, FreshConnectedProposal, RejectedConnectedCandidate, prospective_air_target,
    prospective_airworks_package_value,
};
pub(crate) use lifecycle::EconomyEmergencyRecovery;
pub(crate) use operation::{AirMembership, AirOperationOutcome, AirStage, IslandPreparation};
pub use operation::{AirOperation, AirOperationPhase, AirRecoveryReason};
pub(crate) use turn::{
    AirAdjudication, AirEvidence, AirProcurement, AirTurn, CapitalReserve, ConnectedInputs,
    LiftSupportRequest, ProducerLanes, ThinkInputs, air_adjudication,
};

use force_package::{
    ConnectedForcePackage, ConnectedForcePackageOptions, ConnectedTargetEvidence, ForceFamily,
    ForcePackageRejection, NormalizedCapability, PreparationConstraints, ProductionEvidence,
    ProviderDemand, ProviderDemandTranche, building_value, current_aa_contact,
    current_target_cluster, derive_connected_force_package_options_for_cluster, eligible_producers,
    refine_provider_demands, strike_capability, suppression_capability, target_cluster_air_defense,
};

/// A connected-map combined-arms operation is an expensive second front, not
/// an opening build order. Keep a real fighting roster online before reserving
/// scouts, artillery, and strike aircraft so a seeded specialty cannot hollow
/// out the ordinary line that protects the economy.
const CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER: usize = 12;
/// A connected operation may use only completed production that can finish its
/// whole requested package inside this immutable preparation window. The
/// coordinator's joint resource projection shares it.
pub(crate) const CONNECTED_PREPARATION_HORIZON: Tick = 2_400;

/// One strategic think's ordered requests and resource claims.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrategicDecision {
    /// Ordered intents; suppression precedes bomber holds.
    pub intents: Vec<Intent>,
    /// Canonical exact-unit claims for the executive.
    pub reservations: Vec<UnitId>,
    /// Current scrap held independently of this decision's production requests.
    pub reserved_scrap: u32,
}

impl StrategicDecision {
    pub(crate) fn production(&self) -> impl Iterator<Item = (BuildingId, UnitKind)> + '_ {
        self.intents.iter().filter_map(|intent| match intent {
            Intent::TrainAt { building, kind } => Some((*building, *kind)),
            _ => None,
        })
    }

    /// Current capital owned by held reserves and exact immediate purchases.
    pub fn committed_scrap(&self) -> u32 {
        self.production()
            .fold(self.reserved_scrap, |total, (_, kind)| {
                total.saturating_add(kind.stats().cost)
            })
    }
}

/// Fog-honest evidence for the connected force package's current revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectedPackageDiagnostics {
    pub(super) admitted_at: Tick,
    pub(super) derived_at: Tick,
    pub(super) preparation_deadline: Tick,
    pub(super) admitted_anchors: Vec<TilePos>,
    pub(super) live_anchors: Vec<TilePos>,
    pub(super) focus: TilePos,
    pub(super) target_anchors: Vec<TilePos>,
    pub(super) target_value: u64,
    pub(super) current_scrap: u32,
    pub(super) forecast_scrap: u32,
    pub(super) minimum_capability: [u64; 3],
    pub(super) useful_capability: [u64; 3],
    pub(super) chosen_capability: [u64; 3],
    pub(super) useful_bombing: u64,
    pub(super) chosen_bombing: u64,
    pub(super) recon: Vec<(UnitKind, usize)>,
    pub(super) suppression: Vec<(UnitKind, usize)>,
    pub(super) strike: Vec<(UnitKind, usize)>,
    pub(super) observed_aa_firepower: u64,
    pub(super) suppressible_aa_firepower: u64,
}

/// Controller-local owner of the active operation and its cooldown.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StrategicPlanner {
    outcomes: super::experience::OutcomeJournal,
    air: Option<ActiveAirOperation>,
    standby: AirStandby,
    cooldown_until: Tick,
    terminal_outcome: Option<AirOperationOutcome>,
}

impl StrategicPlanner {
    /// Creates an idle planner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rejects restored state that could panic, cause unbounded work, or
    /// break an ordering later passes rely on. A forged value that only
    /// changes play, such as a cooldown, is accepted.
    pub(crate) fn valid_checkpoint(&self, map: &PublicMapBriefing, tick: Tick) -> bool {
        let mut members: Vec<_> = self.owned_units().collect();
        members.sort_unstable();
        members.windows(2).all(|pair| pair[0] != pair[1])
            && strictly_increasing(&self.standby.artillery)
            && strictly_increasing(&self.standby.strike_aircraft)
            && self.terminal_outcome.is_none_or(|outcome| match outcome {
                AirOperationOutcome::Released { target, .. }
                | AirOperationOutcome::Aborted { target, .. } => on_map(map, target),
            })
            && self
                .air
                .as_ref()
                .is_none_or(|active| active.valid_checkpoint(map, tick))
    }

    /// Active operation for replay diagnostics.
    pub fn air_operation(&self) -> Option<&AirOperation> {
        self.air.as_ref().map(|active| &active.op)
    }

    pub(crate) fn air_capacity_deadline(&self) -> Option<Tick> {
        let active = self.air.as_ref()?;
        Some(active.plan.package().map_or_else(
            || {
                active
                    .op
                    .started_at
                    .saturating_add(active.plan.assembly_timeout(active.op.started_at))
            },
            |package| package.preparation_deadline,
        ))
    }

    /// Immutable admission tick for resource-priority comparisons. The public
    /// operation's timeout clock may restart when reconnaissance becomes an
    /// assault, but its place in the commitment order does not.
    pub(super) fn air_admitted_at(&self) -> Option<Tick> {
        self.air.as_ref().map(|active| active.plan.admitted_at())
    }

    /// Committed preparation deadline of an admitted connected operation.
    pub(crate) fn connected_deadline(&self) -> Option<Tick> {
        Some(self.air.as_ref()?.plan.connected()?.commitment.deadline)
    }

    /// Connected-package evidence for opt-in decision traces.
    pub(super) fn connected_package_diagnostics(
        &self,
        intel: &StrategicIntelligence,
    ) -> Option<ConnectedPackageDiagnostics> {
        let connected = self.air.as_ref()?.plan.connected()?;
        let package = &connected.package;
        let mut live_anchors: Vec<_> = connected
            .commitment
            .live_members(intel)
            .into_iter()
            .map(|contact| contact.anchor)
            .collect();
        live_anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
        live_anchors.dedup();
        Some(ConnectedPackageDiagnostics {
            admitted_at: connected.commitment.admitted_at,
            derived_at: package.derived_at,
            preparation_deadline: package.preparation_deadline,
            admitted_anchors: connected.commitment.anchors.clone(),
            live_anchors,
            focus: connected.focus,
            target_anchors: package.target_anchors.clone(),
            target_value: package.target_value,
            current_scrap: package.current_scrap,
            forecast_scrap: package.forecast_scrap,
            minimum_capability: capability_components(package.minimum_capability),
            useful_capability: capability_components(package.useful_capability),
            chosen_capability: capability_components(package.chosen_capability),
            useful_bombing: package.useful_bombing,
            chosen_bombing: package.chosen_bombing,
            recon: demand_components(&package.recon),
            suppression: demand_components(&package.suppression),
            strike: demand_components(&package.strike),
            observed_aa_firepower: package.observed_aa_firepower,
            suppressible_aa_firepower: package.suppressible_aa_firepower,
        })
    }

    pub(super) fn terminal_outcome(&self) -> Option<AirOperationOutcome> {
        self.terminal_outcome
    }

    pub(crate) fn outcomes_mut(&mut self) -> &mut super::experience::OutcomeJournal {
        &mut self.outcomes
    }

    pub(crate) fn episode_id(&self) -> Option<super::experience::EpisodeId> {
        self.outcomes.episode_id()
    }

    pub(crate) fn owned_units(&self) -> impl Iterator<Item = UnitId> + '_ {
        self.air
            .iter()
            .flat_map(|active| {
                active
                    .op
                    .members()
                    .chain(active.plan.screen().iter().copied())
            })
            .chain(AirRoster::from(&self.standby).members())
    }

    /// Begins one decision's turn: releases dead standby members, refreshes
    /// the operation's target, aborts on current evidence, and settles the
    /// paid ledger. Every later read in the decision sees this state.
    pub(crate) fn observe<'a>(&'a mut self, ev: AirEvidence<'a>) -> AirTurn<'a> {
        let AirEvidence {
            profile,
            obs,
            intel,
            ..
        } = ev;
        self.standby.prune(obs);
        if let Some(active) = &mut self.air {
            refresh_target(&mut active.op, &active.plan, intel);
            if active.op.phase() != AirOperationPhase::Recover {
                abort_if_needed(&mut active.op, &active.plan, profile, obs, intel);
            }
        }
        self.prune_paid_production(obs);
        AirTurn { planner: self, ev }
    }

    fn apply_membership(&mut self, membership: AirMembership, now: Tick) {
        let active = self
            .air
            .as_mut()
            .expect("validated air operation remains active");
        let previous_scout = active.op.scout;
        let previous_artillery = core::mem::replace(&mut active.op.artillery, membership.artillery);
        let previous_strike =
            core::mem::replace(&mut active.op.strike_aircraft, membership.strike_aircraft);
        active.op.scout = membership.scout;
        if let Some(screen) = active.plan.screen_mut() {
            *screen = membership.screen;
        }
        if previous_scout != active.op.scout
            && active.op.scout.is_some()
            && (matches!(active.plan, AirPlan::Connected(_))
                || active.op.phase() == AirOperationPhase::Recon)
        {
            active.op.phase_started_at = now;
        }
        invalidate_reassigned_member_orders(
            &mut active.op,
            previous_scout,
            &previous_artillery,
            &previous_strike,
        );
    }

    /// The remembered objective of an unadmitted reconnaissance watch. Only
    /// such an objective may hold a prospective first carrier's capital.
    pub(crate) fn remembered_recon_target<'i>(
        &self,
        intel: &'i StrategicIntelligence,
    ) -> Option<&'i BuildingContact> {
        self.air
            .as_ref()
            .filter(|active| unadmitted_recon(&active.op))
            .and_then(|active| remembered_objective(&active.op, intel))
    }

    pub(crate) fn has_active_island_operation(&self) -> bool {
        self.air.as_ref().is_some_and(|active| {
            active.op.assault_admitted() && matches!(active.plan, AirPlan::Island(_))
        })
    }

    /// Installs the exact proposal selected by cross-domain adjudication. No
    /// observation is accepted here, so commitment cannot rerank its target,
    /// rebuild its package, or change its producer basis.
    fn commit_connected(&mut self, proposal: FreshConnectedProposal) {
        let revises_active = proposal.revises_active_operation();
        let paid = match &proposal.origin {
            ConnectedProposalOrigin::Active { plan, .. } => plan.paid_production.clone(),
            _ => Vec::new(),
        };
        let mut selected = proposal
            .variants
            .into_iter()
            .nth(proposal.selected_variant)
            .expect("a selected proposal variant came from its retained ladder");
        selected.plan.paid_production = paid;
        self.air = Some(selected.into_active());
        if !revises_active {
            self.standby = AirStandby::default();
        }
        self.terminal_outcome = None;
    }
}

fn capability_components(capability: NormalizedCapability) -> [u64; 3] {
    [capability.recon, capability.suppression, capability.strike]
}

fn demand_components(demands: &[ProviderDemand]) -> Vec<(UnitKind, usize)> {
    demands
        .iter()
        .map(|demand| (demand.kind, demand.count))
        .collect()
}

#[cfg(test)]
mod tests;
