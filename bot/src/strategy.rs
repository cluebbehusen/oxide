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

/// Exact transport objective and landing envelope offered to the air planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LiftSupportRequest {
    pub player: PlayerId,
    pub target: TilePos,
    pub planned_drops: Vec<TilePos>,
}

/// Same-observation evidence every air-planner call in one decision reads.
#[derive(Clone, Copy)]
pub(crate) struct AirEvidence<'a> {
    pub(crate) profile: &'a ResolvedProfile,
    pub(crate) tuning: DifficultyTuning,
    pub(crate) obs: &'a Observation,
    pub(crate) intel: &'a StrategicIntelligence,
    pub(crate) home: TilePos,
    pub(crate) public_map: Option<&'a PublicMapBriefing>,
    pub(crate) orientation: Orientation,
}

impl<'a> AirEvidence<'a> {
    fn route(self, target: TilePos) -> ConnectedRouteContext<'a> {
        ConnectedRouteContext::new(
            self.intel,
            self.public_map,
            self.orientation,
            self.home,
            target,
        )
    }
}

/// Current scrap, and forecast scrap through the preparation window, that
/// earlier owners already hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CapitalReserve {
    pub(crate) current: u32,
    pub(crate) forecast: u32,
}

/// Allocation evidence for deriving one connected-operation proposal.
#[derive(Clone, Copy)]
pub(crate) struct ConnectedInputs<'a> {
    pub(crate) planning: &'a crate::planning::PlanningWork,
    pub(crate) resources: &'a ResourceSnapshot,
    /// Units other owners hold.
    pub(crate) unavailable: &'a [UnitId],
    /// Paid queue occurrences other programs own.
    pub(crate) paid_exclusions: &'a [PaidQueueClaim],
    pub(crate) reserve: CapitalReserve,
}

/// Producer work already accepted ahead of the air planner this decision.
#[derive(Clone, Copy)]
pub(crate) struct ProducerLanes<'a> {
    pub(crate) prior_intents: &'a [Intent],
    pub(crate) reservations: &'a ProducerLaneReservations,
}

impl ProducerLanes<'static> {
    pub(crate) fn empty() -> Self {
        Self {
            prior_intents: &[],
            reservations: ProducerLaneReservations::empty(),
        }
    }
}

/// Allocation evidence for recruiting and buying an air operation's members.
#[derive(Clone, Copy)]
pub(crate) struct AirProcurement<'a> {
    pub(crate) planning: &'a crate::planning::PlanningWork,
    /// Units other owners hold.
    pub(crate) unavailable: &'a [UnitId],
    /// Paid queue occurrences other programs own.
    pub(crate) paid_exclusions: &'a [PaidQueueClaim],
    pub(crate) reserve: CapitalReserve,
    pub(crate) lanes: ProducerLanes<'a>,
    /// Whether the operation may buy members. The lifecycle also begins a
    /// new operation only when it may.
    pub(crate) allow: bool,
}

/// Allocation verdicts the post-adjudication lifecycle acts on.
pub(crate) struct ThinkInputs<'a> {
    pub(crate) procurement: AirProcurement<'a>,
    /// Units another owner claimed after allocation; the operation waits
    /// rather than acting through them.
    pub(crate) claimed_elsewhere: &'a [UnitId],
    pub(crate) lift_support: Option<&'a LiftSupportRequest>,
    /// Excludes every unit the operation does not already own.
    pub(crate) owned_only: bool,
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

/// One decision's exclusive access to the air planner, bound to the evidence
/// it observed. Proposals read the planner; only the named transitions change
/// it, in the order allocation reaches them.
pub(crate) struct AirTurn<'a> {
    planner: &'a mut StrategicPlanner,
    ev: AirEvidence<'a>,
}

impl core::ops::Deref for AirTurn<'_> {
    type Target = StrategicPlanner;

    fn deref(&self) -> &StrategicPlanner {
        self.planner
    }
}

/// What the accepted allocation settles for the air planner.
pub(crate) enum AirAdjudication {
    Unchanged,
    /// Retained membership the allocation validated.
    Members(AirMembership),
    /// The accepted fresh proposal or active revision.
    Connected(Box<FreshConnectedProposal>),
}

/// The accepted connected proposal, otherwise the membership of the retained
/// connected or island operation the allocation imported.
pub(crate) fn air_adjudication(
    connected: Option<FreshConnectedProposal>,
    active_connected: Option<&ActiveConnectedObligation>,
    island: Option<&IslandPreparation>,
) -> AirAdjudication {
    if let Some(proposal) = connected {
        return AirAdjudication::Connected(Box::new(proposal));
    }
    active_connected
        .map(|active| &active.membership)
        .or_else(|| island.map(|island| &island.membership))
        .map_or(AirAdjudication::Unchanged, |membership| {
            AirAdjudication::Members(membership.clone())
        })
}

impl<'a> AirTurn<'a> {
    /// Lends this turn to a shorter-lived owner such as one allocation pass.
    pub(crate) fn reborrow(&mut self) -> AirTurn<'_> {
        AirTurn {
            planner: self.planner,
            ev: self.ev,
        }
    }

    /// Reconstructs unpaid demand from the retained package and current inventory.
    pub(crate) fn retained_obligation(
        &self,
        resources: &ResourceSnapshot,
        unavailable: &[UnitId],
        paid_exclusions: &[PaidQueueClaim],
    ) -> Option<ActiveConnectedObligation> {
        let active = self
            .air
            .as_ref()
            .filter(|active| active.op.assault_admitted())?;
        let connected = active.plan.connected()?;
        let package = &connected.package;
        let mut membership = AirMembership::from_active(active);
        let obs = self.ev.obs;
        let provider_jobs = if active.op.phase() <= AirOperationPhase::Assemble
            && obs.tick < package.preparation_deadline
            && operation_recovery_reason(
                &active.op,
                &active.plan,
                self.ev.profile,
                obs,
                self.ev.intel,
            )
            .is_none()
        {
            let route = self
                .ev
                .route(connected.focus)
                .excluding_paid(paid_exclusions);
            let resources =
                ConnectedProductionResources::from_package_snapshot_after_current_reserve(
                    obs,
                    connected.commitment.player,
                    package,
                    route,
                    resources,
                    0,
                );
            if active.op.phase() <= AirOperationPhase::Assemble
                && active.op.membership_frozen_at.is_none()
            {
                let owned = reservations(&active.op, &active.plan, obs);
                let unavailable = excluding_owned(unavailable, &owned);
                let mut unavailable =
                    connected_provider_unavailable(obs, &resources.targets, &unavailable, route);
                unavailable.retain(|id| !owned.contains(id));
                if membership.scout.is_none() && active.op.scout_dispatch.is_none() {
                    let mut scouts = Vec::new();
                    assign_provider_demands(&mut scouts, &package.recon, obs, &unavailable);
                    membership.scout = scouts.into_iter().next();
                }
                assign_artillery(&mut membership.artillery, &active.plan, obs, &unavailable);
                assign_strike_aircraft(
                    &mut membership.strike_aircraft,
                    &active.plan,
                    obs,
                    &unavailable,
                );
            }
            missing_package_demands(
                package,
                AirRoster::from(&membership),
                obs,
                &resources.snapshot,
                package.preparation_deadline,
                &resources.access,
            )
            .into_iter()
            .flat_map(|demand| {
                let job = ConnectedProviderJob {
                    kind: demand.kind,
                    enqueue_not_before: obs.tick,
                    ready_before: package.preparation_deadline,
                    eligible_producers: eligible_producers(
                        &resources.snapshot,
                        &resources.access,
                        demand.kind,
                        Some(package.preparation_deadline),
                    ),
                };
                std::iter::repeat_n(job, demand.count)
            })
            .collect()
        } else {
            Vec::new()
        };
        Some(ActiveConnectedObligation {
            identity: connected.commitment.key(),
            accepted_at: connected.commitment.admitted_at,
            deadline: package.preparation_deadline,
            units: membership.units(obs),
            membership,
            provider_jobs,
        })
    }

    pub(crate) fn island_preparation(
        &self,
        procurement: AirProcurement<'_>,
    ) -> Option<IslandPreparation> {
        let active = self.air.as_ref()?;
        let AirPlan::Island(island) = &active.plan else {
            return None;
        };
        if !active.op.assault_admitted() {
            return None;
        }
        let op = &active.op;
        let plan = &active.plan;
        let obs = self.ev.obs;
        let mut membership = AirMembership::from_active(active);
        let mut purchases = ProductionPlan::default();
        if op.phase() <= AirOperationPhase::Assemble {
            let unavailable =
                excluding_owned(procurement.unavailable, &reservations(op, plan, obs));
            let scout = Role::Scout.unit_for(obs.faction);
            membership.scout = retained_scout(membership.scout, obs, &unavailable);
            assign_artillery(&mut membership.artillery, plan, obs, &unavailable);
            assign_strike_aircraft(&mut membership.strike_aircraft, plan, obs, &unavailable);
            assign_exact(
                &mut membership.screen,
                island.desired_screen,
                obs,
                &unavailable,
                |kind| kind == Role::AirGround.unit_for(obs.faction),
            );
            let planning = AirPlanningContext {
                ev: self.ev,
                procurement: AirProcurement {
                    unavailable: &unavailable,
                    ..procurement
                },
                landing_sites: &[],
                connected_resources: None,
            };
            let demands = missing_island_members(
                AirRoster::from(&membership),
                membership.screen.len(),
                island,
                &planning,
                scout,
            );
            purchases = schedule(&planning, &demands);
        }
        Some(IslandPreparation {
            membership,
            purchases,
        })
    }

    /// Re-derives one admitted connected operation from current evidence while
    /// its membership remains revisable. The fixed preparation deadline and
    /// the committed identity and target set are retained; the package is
    /// sized against the committed members only.
    pub(crate) fn connected_revision(
        &self,
        inputs: ConnectedInputs<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let AirEvidence {
            profile,
            obs,
            intel,
            public_map,
            orientation,
            ..
        } = self.ev;
        if intel.observed_at() != Some(obs.tick) {
            return Ok(None);
        }
        let Some(active) = self.air.as_ref() else {
            return Ok(None);
        };
        let AirPlan::Connected(connected) = &active.plan else {
            return Ok(None);
        };
        let package = &connected.package;
        if !active.op.assault_admitted()
            || active.op.phase() > AirOperationPhase::Assemble
            || active.op.membership_frozen_at.is_some()
            || package.derived_at >= obs.tick
            || operation_recovery_reason(&active.op, &active.plan, profile, obs, intel).is_some()
            || obs.tick > connected.commitment.deadline
        {
            return Ok(None);
        }
        let Some(target) = best_current_member(&connected.commitment, connected.focus, intel)
        else {
            return Ok(None);
        };
        let owned = reservations(&active.op, &active.plan, obs);
        let unavailable = excluding_owned(inputs.unavailable, &owned);
        let campaign_routes = CampaignRoutes::new(obs, intel, public_map, orientation);
        let context = FreshConnectedDerivationContext {
            ev: self.ev,
            inputs: ConnectedInputs {
                unavailable: &unavailable,
                ..inputs
            },
            minimum_only: false,
            campaign_routes: &campaign_routes,
            preferred_artillery: &active.op.artillery,
        };
        // A revision sizes the admitted members only; it never searches the
        // radius around its current primary again.
        let proposal = derive_connected_proposal_with_resources(
            context,
            target,
            connected.commitment.sized_members(intel, obs.tick),
            ConnectedProposalOrigin::Active {
                op: active.op.clone(),
                plan: connected.clone(),
            },
            connected.commitment.deadline,
        )
        .map_err(|reason| RejectedConnectedCandidate {
            target: target.clone(),
            reason,
        })?;
        // Tactics act on every live committed member, so a revision that could
        // not size one of them keeps the current package instead.
        let sized = connected.commitment.sized_members(intel, obs.tick);
        if !proposal.variants.iter().all(|variant| {
            sized
                .iter()
                .all(|member| variant.plan.package.target_anchors.contains(&member.anchor))
        }) {
            return Ok(None);
        }
        Ok(Some(proposal))
    }

    /// Proposes one exact common-minimum connected assault without mutating
    /// planner state. Island admission and an already-admitted assault remain
    /// on the ordinary lifecycle path.
    pub(crate) fn fresh_connected(
        &self,
        experience: &super::experience::Experience,
        inputs: ConnectedInputs<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let AirEvidence {
            profile,
            tuning,
            obs,
            intel,
            home,
            public_map,
            orientation,
        } = self.ev;
        if intel.observed_at() != Some(obs.tick)
            || !strategic_admission_tick(obs.tick)
            || obs.tick < self.cooldown_until
        {
            return Ok(None);
        }

        if let Some(active) = &self.air {
            if active.op.assault_admitted() {
                return Ok(None);
            }
            let mut refreshed = active.clone();
            refresh_target(&mut refreshed.op, &refreshed.plan, intel);
            let Some(target) = current_target_contact(&refreshed.op, intel) else {
                return Ok(None);
            };
            if wealthy_island_target(profile, obs, home, target, public_map) {
                return Ok(None);
            }
            let owned = reservations(&refreshed.op, &refreshed.plan, obs);
            let unavailable = excluding_owned(inputs.unavailable, &owned);
            let campaign_routes = CampaignRoutes::new(obs, intel, public_map, orientation);
            return derive_connected_proposal_with_resources(
                FreshConnectedDerivationContext {
                    ev: self.ev,
                    inputs: ConnectedInputs {
                        unavailable: &unavailable,
                        ..inputs
                    },
                    minimum_only: false,
                    campaign_routes: &campaign_routes,
                    preferred_artillery: &refreshed.op.artillery,
                },
                target,
                current_target_cluster(intel, target.player, target.anchor),
                ConnectedProposalOrigin::Remembered {
                    active: active.clone(),
                },
                obs.tick.saturating_add(CONNECTED_PREPARATION_HORIZON),
            )
            .map(Some)
            .map_err(|reason| RejectedConnectedCandidate {
                target: target.clone(),
                reason,
            });
        }

        if select_wealthy_island_target(profile, obs, home, intel, public_map).is_some() {
            return Ok(None);
        }
        let mut current = select_target_candidates(intel, obs.tick, tuning.tactical_memory)
            .into_iter()
            .filter(|target| target.evidence == ContactEvidence::Current)
            .collect::<Vec<_>>();
        current.sort_unstable_by_key(|target| {
            let context = super::experience::ExperienceKey {
                doctrine: super::experience::Doctrine::Air,
                x: target.anchor.x,
                y: target.anchor.y,
                subject: super::experience::ExperienceSubject::Building(target.id),
            };
            let preference = (1024 + i32::from(experience.score(context)) / 2) as u64;
            (
                Reverse(u64::from(building_value(target.kind)) * preference),
                Reverse(target.confidence_at(obs.tick)),
                target.anchor.y,
                target.anchor.x,
                target.player,
                target.kind,
            )
        });
        let Some(first) = current.first().copied() else {
            return Ok(None);
        };
        let combat_roster = combat_roster(obs);
        if combat_roster < CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER {
            return Err(RejectedConnectedCandidate {
                target: first.clone(),
                reason: ConnectedPlanRejection::InsufficientStandingForce {
                    current: combat_roster,
                    required: CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER,
                },
            });
        }

        let mut standby = self.standby.clone();
        standby.prune(obs);
        let unavailable = excluding_owned(inputs.unavailable, &standby.reservations());
        let origin = ConnectedProposalOrigin::Idle {
            standby: self.standby.clone(),
        };
        let mut first_rejection = None;
        let campaign_routes = CampaignRoutes::new(obs, intel, public_map, orientation);
        for target in current {
            match derive_connected_proposal_with_resources(
                FreshConnectedDerivationContext {
                    ev: self.ev,
                    inputs: ConnectedInputs {
                        unavailable: &unavailable,
                        ..inputs
                    },
                    minimum_only: false,
                    campaign_routes: &campaign_routes,
                    preferred_artillery: &standby.artillery,
                },
                target,
                current_target_cluster(intel, target.player, target.anchor),
                origin.clone(),
                obs.tick.saturating_add(CONNECTED_PREPARATION_HORIZON),
            ) {
                Ok(proposal) => return Ok(Some(proposal)),
                Err(reason) if first_rejection.is_none() => {
                    first_rejection = Some(RejectedConnectedCandidate {
                        target: target.clone(),
                        reason,
                    });
                }
                Err(_) => {}
            }
        }
        Err(first_rejection.expect("at least one current target was considered"))
    }

    /// Returns the exact remembered reconnaissance target that the ordinary
    /// post-allocation lifecycle will retain or admit on this observation.
    /// This preview is read-only so shared allocation can preserve capital
    /// needed by the immediately following Lift handoff.
    pub(crate) fn prospective_recon_target(
        &self,
        unavailable: &[UnitId],
        lift_support: Option<&LiftSupportRequest>,
    ) -> Option<&'a BuildingContact> {
        let AirEvidence {
            profile,
            obs,
            intel,
            public_map,
            ..
        } = self.ev;
        if intel.observed_at() != Some(obs.tick) {
            return None;
        }
        let ActiveAirOperation { mut op, plan } = match &self.air {
            Some(active) if unadmitted_recon(&active.op) => active.clone(),
            // Target selection is skipped when nothing could begin anyway.
            None if strategic_admission_tick(obs.tick) => {
                let AirAdmission::Begin(selected) = self.admission(self.ev, true, lift_support)
                else {
                    return None;
                };
                let mut standby = self.standby.clone();
                standby.prune(obs);
                fresh_air_operation(profile, obs, ProducerLanes::empty(), selected, standby)
            }
            Some(_) | None => return None,
        };
        refresh_target(&mut op, &plan, intel);
        let target = remembered_objective(&op, intel)?;
        if operation_recovery_reason(&op, &plan, profile, obs, intel).is_some() {
            return None;
        }
        let owned = reservations(&op, &plan, obs);
        op.scout = remembered_recon_scout(&op, obs, &excluding_owned(unavailable, &owned));
        reachable_scout_goal(
            &op,
            &plan,
            obs,
            intel,
            &landing_sites(lift_support, &op),
            connected_public_map(&plan, public_map),
        )
        .map(|_| target)
    }

    /// Moves the admitted connected operation into bounded recovery as soon as
    /// preparation proves it cannot continue, so every later read in this
    /// allocation sees the recovery phase. The post-allocation think owns the
    /// one return-home order.
    pub(crate) fn recover_connected(&mut self, reason: AirRecoveryReason) {
        let active = self
            .planner
            .air
            .as_mut()
            .expect("an active connected obligation can only come from its planner");
        debug_assert!(active.op.assault_admitted());
        debug_assert!(matches!(active.plan, AirPlan::Connected(_)));
        recover(&mut active.op, reason, self.ev.obs.tick);
    }

    /// Installs the accepted allocation's verdict. Only purchases emitted now
    /// cross from forecast evidence into the connected ledger.
    pub(crate) fn apply(
        &mut self,
        verdict: AirAdjudication,
        schedule: &[crate::allocation::ScheduledProducerJob],
    ) {
        let now = self.ev.obs.tick;
        match verdict {
            AirAdjudication::Unchanged => {}
            AirAdjudication::Members(membership) => self.planner.apply_membership(membership, now),
            AirAdjudication::Connected(proposal) => self.planner.commit_connected(*proposal),
        }
        self.planner.record_connected_purchases(schedule, now);
    }

    /// Runs the ordinary tactical lifecycle after the coordinator has already
    /// accepted or rejected the fresh connected-offense proposal for this
    /// observation. Island and remembered reconnaissance behavior is unchanged.
    pub(crate) fn think(&mut self, inputs: ThinkInputs<'_>) -> StrategicDecision {
        self.planner.think(self.ev, inputs)
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
