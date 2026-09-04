//! One atomic player-facing allocation transaction.
//!
//! The domain planners still decide what work is worth doing. This module owns
//! the lifecycle around those decisions: assemble exact prior claims, resolve
//! the shared portfolio once, then either commit every accepted payload or
//! restore every speculative planner mutation.

use super::{
    AllocationConflict, AllocationError, AllocationPersonality, ClaimBundle, ClaimBundleError,
    ClaimOwner, ConnectedOffenseKey, ConnectedPortfolioContext, CoordinatorInputError,
    CrossDomainAllocation, CrossDomainSettlement, DomainInvestmentProposal, ForecastClaim,
    ImportedObligation, LegacyChannel, LegacyDecisionRequest, ObligationClass, ObligationKey,
    ProducerJobClaim, ProposalKey, StandingForceKey, Urgency, active_connected_obligation,
    active_connected_producer_assignments, active_connected_revision_investment_proposal,
    active_connected_revision_obligation, active_connected_revision_producer_assignments,
    clamped_current_reserve_obligation, connected_investment_proposal,
    connected_producer_assignments, current_reserve_at, forecast_reserve_through,
    foundry_investment_proposal, fresh_emergency_defense_obligation, imported_obligation,
    legacy_decision_obligation, legacy_unit_obligation, observed_builder_obligations,
    saved_foundry_obligation, standing_force_investment_proposals,
};
use crate::bot::PublicMapBriefing;
use crate::bot::difficulty::{DifficultyTuning, strategic_admission_tick};
use crate::bot::executive::Intent;
use crate::bot::intelligence::StrategicIntelligence;
use crate::bot::lift::{
    ActiveLiftProductionObligation, LiftAdmission, LiftAirSupport, LiftOperation, LiftPlanner,
    LiftProducerAssignment, LiftProducerFunding, LiftProducerTiming,
};
use crate::bot::observation::Observation;
use crate::bot::orient::Orientation;
use crate::bot::profile::ResolvedProfile;
use crate::bot::raid::RaidPlanner;
use crate::bot::resources::{ProducerLaneReservations, ReservedProducerJob, ResourceSnapshot};
use crate::bot::standing_force::{
    StandingForceContext, StandingForceProposal, StandingGroundTarget,
    StandingProductionCommitment, derive_standing_force_proposals,
};
use crate::bot::strategy::{
    ActiveConnectedObligation, AirOperation, AirOperationOutcome, AirOperationPhase,
    FreshConnectedProposal, FreshConnectedProposalRequest, LiftSupportRequest,
    RejectedConnectedCandidate, StrategicCoordination, StrategicDecision, StrategicPlanner,
    StrategicThinkContext, StrategicThinkResult, connected_preparation_horizon,
};
use crate::bot::team::TeamReliefPlanner;
use crate::bot::trace::{
    AllocationCoordinatorFailureReasonTrace, AllocationCoordinatorStageTrace, AllocationTrace,
};
use crate::bot::utility::{
    CombatCoreStatus, Dials, FreshEmergencyDefense, FreshEmergencyDefenseContext,
    FreshFoundryInvestment, FreshFoundryProposal, FreshFoundryProposalContext,
    ResidualInvestmentReserveContext, SHALLOW_QUEUE_DEPTH, SavedFoundryReadiness, UtilityPolicy,
    ValidatedFoundryObligation, combat_core_status,
};
use crate::ids::UnitId;
use crate::stats::{BuildingKind, Domain, UnitKind};
use chassis::Tick;
use chassis::grid::TilePos;
use std::collections::BTreeMap;

/// One current view over every exact unit retained by a planner.
///
/// Recreate this value after a planner mutates. Each selector names the owners
/// intentionally included at that decision boundary, avoiding positional
/// slices and hand-maintained clone chains in the frame loop.
pub(crate) struct PlannerClaims<'a> {
    enlisted: &'a [UnitId],
    strategy: &'a Option<StrategicPlanner>,
    raids: &'a Option<RaidPlanner>,
    lifts: &'a Option<LiftPlanner>,
}

impl<'a> PlannerClaims<'a> {
    pub(crate) const fn new(
        enlisted: &'a [UnitId],
        strategy: &'a Option<StrategicPlanner>,
        raids: &'a Option<RaidPlanner>,
        lifts: &'a Option<LiftPlanner>,
    ) -> Self {
        Self {
            enlisted,
            strategy,
            raids,
            lifts,
        }
    }

    fn air(&self) -> Option<&crate::bot::strategy::AirOperation> {
        self.strategy
            .as_ref()
            .and_then(StrategicPlanner::air_operation)
    }

    fn raid_reservations(&self) -> &[UnitId] {
        self.raids.as_ref().map_or(&[], RaidPlanner::reservations)
    }

    fn lift(&self) -> Option<&LiftOperation> {
        self.lifts.as_ref().and_then(LiftPlanner::operation)
    }

    /// Everything spoken for from the team planner's point of view.
    pub(crate) fn external_to_team(&self) -> Vec<UnitId> {
        prior_planner_claims(
            self.enlisted,
            self.air(),
            &[],
            self.raid_reservations(),
            self.lift(),
        )
    }

    /// Planner claims excluded from the ordinary opening-core measurement.
    pub(crate) fn core_exclusions(&self, relief: &[UnitId]) -> Vec<UnitId> {
        prior_planner_claims(
            &[],
            self.air(),
            relief,
            self.raid_reservations(),
            self.lift(),
        )
    }

    /// Units owned by persistent planners rather than the Executive.
    pub(crate) fn without_executive(&self, relief: &[UnitId]) -> Vec<UnitId> {
        self.core_exclusions(relief)
    }

    /// Non-Executive claims except the lift planner's own.
    pub(crate) fn without_lift(&self, relief: &[UnitId]) -> Vec<UnitId> {
        prior_planner_claims(&[], self.air(), relief, self.raid_reservations(), None)
    }

    /// Every claim from every source.
    pub(crate) fn all(&self, relief: &[UnitId]) -> Vec<UnitId> {
        prior_planner_claims(
            self.enlisted,
            self.air(),
            relief,
            self.raid_reservations(),
            self.lift(),
        )
    }
}

pub(crate) fn prior_planner_claims(
    enlisted: &[UnitId],
    air: Option<&crate::bot::strategy::AirOperation>,
    relief: &[UnitId],
    raid: &[UnitId],
    lift: Option<&LiftOperation>,
) -> Vec<UnitId> {
    let mut claims = enlisted.to_vec();
    if let Some(operation) = air {
        claims.extend(operation.scout);
        claims.extend(operation.artillery.iter().copied());
        claims.extend(operation.strike_aircraft.iter().copied());
    }
    claims.extend_from_slice(relief);
    claims.extend_from_slice(raid);
    if let Some(operation) = lift {
        if operation.manifests.is_empty() {
            claims.extend(operation.payload.iter().copied());
        } else {
            for manifest in &operation.manifests {
                if !manifest.closed {
                    claims.push(manifest.carrier);
                }
                if manifest.retains_rider_ownership() {
                    claims.extend(manifest.riders.iter().copied());
                }
            }
        }
    }
    claims.sort_unstable();
    claims.dedup();
    claims
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

/// Planner mutations made before the shared allocation verdict.
///
/// Decisions are values, while the original planner snapshots provide the
/// atomic restore point if any exact claim or payload fails later in the pass.
pub(crate) struct AdvancedPlannerWork {
    pub(crate) team_decision: StrategicDecision,
    pub(crate) raid_decision: StrategicDecision,
    pub(crate) team_started_at: Tick,
    pub(crate) lift_started_at: Tick,
    pub(crate) raid_started_at: Tick,
    pub(crate) lift_was_active: bool,
    pub(crate) initial_lift_support: LiftAirSupport,
    pub(crate) lift_unavailable: Vec<UnitId>,
    pub(crate) preliminary_core: CombatCoreStatus,
    pub(crate) preliminary_core_exclusions: Vec<UnitId>,
    pub(crate) snapshots: PlannerSnapshots,
}

#[derive(Clone)]
pub(crate) struct PlannerSnapshots {
    strategy: Option<StrategicPlanner>,
    team: Option<TeamReliefPlanner>,
    lifts: Option<LiftPlanner>,
    raids: Option<RaidPlanner>,
}

impl PlannerSnapshots {
    pub(crate) fn capture(
        strategy: &Option<StrategicPlanner>,
        team: &Option<TeamReliefPlanner>,
        lifts: &Option<LiftPlanner>,
        raids: &Option<RaidPlanner>,
    ) -> Self {
        Self {
            strategy: strategy.clone(),
            team: team.clone(),
            lifts: lifts.clone(),
            raids: raids.clone(),
        }
    }

    fn restore(self, participants: &mut AllocationParticipants<'_>) {
        *participants.strategy = self.strategy;
        *participants.team = self.team;
        *participants.lifts = self.lifts;
        *participants.raids = self.raids;
    }
}

/// Immutable evidence shared by every phase of one allocation pass.
pub(crate) struct AllocationSessionContext<'a> {
    pub(crate) dials: &'a Dials,
    pub(crate) profile: &'a ResolvedProfile,
    pub(crate) tuning: DifficultyTuning,
    pub(crate) observation: &'a Observation,
    pub(crate) home: TilePos,
    pub(crate) public_map: &'a PublicMapBriefing,
    pub(crate) orientation: Orientation,
    pub(crate) intelligence: &'a StrategicIntelligence,
    pub(crate) enlisted: &'a [UnitId],
    pub(crate) lift_support: Option<&'a LiftSupportRequest>,
}

/// Exact bounded capital request retained for one prior legacy operation.
pub(crate) struct BoundedCapitalReserve {
    pub(crate) cadence: Tick,
    pub(crate) bank: u32,
    pub(crate) accepted_at: Tick,
    pub(crate) decision_tick: Tick,
    pub(crate) key: ObligationKey,
    pub(crate) desired: u32,
    pub(crate) forecast_deadline: Tick,
    pub(crate) older_capital_reserve: u32,
}

struct LegacyPlannerClaim<'a> {
    cadence: Tick,
    accepted_at: Tick,
    decision_at: Tick,
    retained_at: Tick,
    channel: LegacyChannel,
    decision: &'a StrategicDecision,
    protect_unspent_current_scrap: bool,
    prior_producer_intents: &'a [Intent],
    retained_units: Vec<UnitId>,
    production_deadline: Tick,
}

/// Mutable participants covered by one all-or-nothing allocation verdict.
pub(crate) struct AllocationParticipants<'a> {
    pub(crate) policy: &'a mut UtilityPolicy,
    pub(crate) strategy: &'a mut Option<StrategicPlanner>,
    pub(crate) lifts: &'a mut Option<LiftPlanner>,
    pub(crate) team: &'a mut Option<TeamReliefPlanner>,
    pub(crate) raids: &'a mut Option<RaidPlanner>,
}

/// Named resource channels returned to the residual planners and trace.
#[derive(Debug, Default)]
pub(crate) struct AllocationBudgetOutcome {
    pub(crate) foundry_saving: u32,
    pub(crate) airworks_capacity: u32,
    pub(crate) opening_bootstrap: u32,
    pub(crate) residual_scrap: u32,
    pub(crate) connected_spendable: u32,
    pub(crate) connected_forecast_hold: u32,
    pub(crate) utility_spendable: u32,
    pub(crate) prior_operation_spendable: u32,
    pub(crate) voluntary_scrap_guard: u32,
}

impl AllocationBudgetOutcome {
    fn frozen(
        foundry_saving: u32,
        airworks_capacity: u32,
        opening_bootstrap: u32,
        voluntary_scrap_guard: u32,
    ) -> Self {
        Self {
            foundry_saving,
            airworks_capacity,
            opening_bootstrap,
            voluntary_scrap_guard,
            connected_forecast_hold: u32::MAX,
            ..Self::default()
        }
    }
}

/// Complete result of one transaction, whether committed or restored.
pub(crate) struct AllocationSessionOutcome {
    pub(crate) opening_core: CombatCoreStatus,
    pub(crate) allow_new_voluntary_operations: bool,
    pub(crate) team_decision: StrategicDecision,
    pub(crate) lift_decision: StrategicDecision,
    pub(crate) raid_decision: StrategicDecision,
    pub(crate) planner_claims: Vec<UnitId>,
    pub(crate) strategic_core_exclusions: Vec<UnitId>,
    pub(crate) connected_continues: bool,
    pub(crate) connected_accepted_at: Option<Tick>,
    pub(crate) rejected_connected_candidate: Option<RejectedConnectedCandidate>,
    pub(crate) staged_strategy: Option<StrategicThinkResult>,
    pub(crate) fresh_emergency_defense_intents: Vec<Intent>,
    pub(crate) fresh_foundry_intents: Vec<Intent>,
    pub(crate) allocated_producer_intents: Vec<Intent>,
    pub(crate) allocation_ok: bool,
    pub(crate) accepted_connected: bool,
    pub(crate) producer_lane_reservations: ProducerLaneReservations,
    pub(crate) budget: AllocationBudgetOutcome,
}

/// One typed allocation transaction over already-advanced legacy planners.
pub(crate) struct AllocationSession<'a> {
    context: AllocationSessionContext<'a>,
    participants: AllocationParticipants<'a>,
    advanced: AdvancedPlannerWork,
    trace: Option<&'a mut AllocationTrace>,
}

impl<'a> AllocationSession<'a> {
    pub(crate) fn new(
        context: AllocationSessionContext<'a>,
        participants: AllocationParticipants<'a>,
        advanced: AdvancedPlannerWork,
        trace: Option<&'a mut AllocationTrace>,
    ) -> Self {
        Self {
            context,
            participants,
            advanced,
            trace,
        }
    }

    /// Runs the named prepare, resolve, and commit-or-restore phases exactly
    /// once. No domain is asked to rerank a payload after preparation.
    pub(crate) fn run(mut self) -> AllocationSessionOutcome {
        let snapshots = CommitSnapshots {
            policy: self.participants.policy.clone(),
        };
        let prepared = self.prepare();
        let resolved = self.resolve(prepared, snapshots);
        self.commit_or_restore(resolved)
    }

    /// Assembles one immutable resource picture, imports exact prior work, and
    /// asks each migrated domain for at most one already-ranked proposal.
    fn prepare(&mut self) -> PreparedAllocation {
        let mut claims = self.snapshot_claims();
        let mut obligations = self.collect_legacy_obligations();
        if obligations.invalid_active_connected {
            self.participants
                .strategy
                .as_mut()
                .expect("an invalid active connected obligation belongs to its planner")
                .recover_unfundable_active_connected(self.context.observation.tick);
            obligations.active_connected = None;
        }
        if obligations.invalid_active_lift {
            self.participants
                .lifts
                .as_mut()
                .expect("an invalid active Lift obligation belongs to its planner")
                .recover_invalid_production(self.context.observation.tick);
            obligations.active_lift = None;
        }
        let emergency_defense = self.prepare_emergency_defense(&claims, &mut obligations);
        let mut air_lift = self.prepare_air_commitments(&claims, &mut obligations);
        let island_admitted_at = self
            .participants
            .strategy
            .as_ref()
            .filter(|planner| {
                planner
                    .air_operation()
                    .is_some_and(|operation| operation.assault_admitted)
            })
            .and_then(StrategicPlanner::air_admitted_at);
        let island_precedes_foundry = island_admitted_at.is_some_and(|accepted_at| {
            self.participants
                .policy
                .operation_precedes_foundry_saving(accepted_at)
        });
        let island_precedes_lift = island_admitted_at.is_some_and(|accepted_at| {
            !self.advanced.lift_was_active || accepted_at <= self.advanced.lift_started_at
        });
        let mut saved = None;
        let mut earlier_producer_intents = Vec::new();
        if island_precedes_lift {
            if !island_precedes_foundry {
                saved = Some(self.prepare_saved_foundry(&claims, &air_lift, &mut obligations));
            }
            self.stage_active_island(&mut claims, &mut obligations);
            if let Some(staged) = obligations.staged_strategy.as_ref() {
                earlier_producer_intents.extend(staged.decision.intents.iter().cloned());
                self.advanced
                    .lift_unavailable
                    .extend(staged.decision.reservations.iter().copied());
                self.advanced.lift_unavailable.sort_unstable();
                self.advanced.lift_unavailable.dedup();
                self.advanced.initial_lift_support = lift_air_support(
                    self.participants
                        .strategy
                        .as_ref()
                        .and_then(StrategicPlanner::air_operation),
                    self.participants
                        .strategy
                        .as_ref()
                        .and_then(StrategicPlanner::terminal_outcome),
                );
            }
            if saved.is_none() && !air_lift.active_lift_precedes_foundry {
                saved = Some(self.prepare_saved_foundry(&claims, &air_lift, &mut obligations));
            }
            self.advance_active_lift(
                &mut obligations,
                &mut air_lift,
                &earlier_producer_intents,
                true,
            );
        } else {
            if !air_lift.active_lift_precedes_foundry {
                saved = Some(self.prepare_saved_foundry(&claims, &air_lift, &mut obligations));
            }
            self.advance_active_lift(
                &mut obligations,
                &mut air_lift,
                &earlier_producer_intents,
                false,
            );
            earlier_producer_intents.extend(air_lift.lift_decision.intents.iter().cloned());
            self.refresh_planner_claims(&mut claims);
            if saved.is_none() && !island_precedes_foundry {
                saved = Some(self.prepare_saved_foundry(&claims, &air_lift, &mut obligations));
            }
            self.stage_active_island(&mut claims, &mut obligations);
        }
        self.refresh_planner_claims(&mut claims);
        self.prepare_standing_army(&claims, &mut obligations);
        let mut saved = saved
            .unwrap_or_else(|| self.prepare_saved_foundry(&claims, &air_lift, &mut obligations));
        if let Some(planner) = self.participants.strategy.as_mut() {
            // Active revision proposals carry the exact planner snapshot they
            // revise, so settle completed queue ownership before deriving one.
            let _ = planner.issued_connected_production_assignments(self.context.observation);
        }
        let active_revision = self.prepare_active_connected_revision(&claims, &mut obligations);
        if active_revision.proposal.is_none() {
            self.downgrade_unfundable_active_connected(&mut saved, &air_lift, &mut obligations);
        }
        let mut fresh =
            self.prepare_fresh_investments(&claims, &saved, &mut obligations, active_revision);
        self.downgrade_unfundable_active_revision(
            &claims,
            &mut saved,
            &air_lift,
            &mut obligations,
            &mut fresh,
        );
        let allocation_horizon =
            self.allocation_horizon(&saved, &fresh, obligations.active_connected.as_ref());

        PreparedAllocation {
            resources: obligations.resources,
            obligations: obligations.obligations,
            coordinator_failure: obligations.coordinator_failure,
            opening_core: claims.opening_core,
            allow_new_voluntary_operations: claims.opening_core.ready,
            planner_claims: claims.planner_claims,
            strategic_core_exclusions: claims.strategic_core_exclusions,
            active_connected: obligations.active_connected,
            active_lift: obligations.active_lift,
            fresh_lift_producer_jobs: air_lift.fresh_lift_producer_jobs,
            saved_foundry: saved.obligation,
            fresh_foundry: fresh.foundry,
            fresh_connected: fresh.connected,
            standing_force: fresh.standing_force,
            connected_accepted_at: fresh.connected_accepted_at,
            connected_reserve_deadline: fresh.connected_reserve_deadline,
            allocation_horizon,
            active_lift_precedes_foundry: air_lift.active_lift_precedes_foundry,
            active_lift_spendable: air_lift.active_lift_spendable,
            foundry_saving: saved.saving,
            airworks_capacity: air_lift.airworks_capacity,
            opening_bootstrap: air_lift.opening_bootstrap,
            voluntary_scrap_guard: air_lift.voluntary_scrap_guard,
            rejected_connected_candidate: fresh.rejected_connected_candidate,
            staged_strategy: obligations.staged_strategy,
            emergency_defense,
            team_decision: core::mem::take(&mut self.advanced.team_decision),
            lift_decision: air_lift.lift_decision,
            raid_decision: core::mem::take(&mut self.advanced.raid_decision),
        }
    }

    fn snapshot_claims(&self) -> ClaimSnapshot {
        let claims = PlannerClaims::new(
            self.context.enlisted,
            self.participants.strategy,
            self.participants.raids,
            self.participants.lifts,
        );
        let team_core_claims = self
            .participants
            .team
            .as_ref()
            .map_or_else(Vec::new, TeamReliefPlanner::core_reservations);
        let planner_claims = claims.all(&team_core_claims);
        let strategic_core_exclusions = claims.core_exclusions(&team_core_claims);
        let opening_core = combat_core_status(
            self.context.observation,
            &strategic_core_exclusions,
            &[],
            u64::from(self.context.dials.minimum_core_equivalents),
        );
        ClaimSnapshot {
            team_core_claims,
            planner_claims,
            strategic_core_exclusions,
            opening_core,
        }
    }

    fn collect_legacy_obligations(&self) -> ObligationPreparation {
        let resources = ResourceSnapshot::from_observation(self.context.observation);
        let mut obligations = Vec::new();
        let mut coordinator_failure = None;
        let mut observed_capital = self.context.observation.scrap;
        match observed_builder_obligations(
            &resources,
            self.context.observation,
            &mut observed_capital,
            self.context
                .observation
                .tick
                .saturating_add(connected_preparation_horizon()),
            self.context.dials.cadence,
        ) {
            Ok(mut observed) => obligations.append(&mut observed),
            Err(error) => retain_first_coordinator_failure(
                &mut coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                Err((&error).into()),
            ),
        }

        retain_first_coordinator_failure(
            &mut coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_legacy_planner_claim(
                &mut obligations,
                &resources,
                LegacyPlannerClaim {
                    cadence: self.context.dials.cadence,
                    accepted_at: self.context.observation.tick,
                    decision_at: self.context.observation.tick,
                    retained_at: self.advanced.team_started_at,
                    channel: LegacyChannel::TeamRelief,
                    decision: &self.advanced.team_decision,
                    protect_unspent_current_scrap: true,
                    prior_producer_intents: &[],
                    retained_units: self
                        .participants
                        .team
                        .as_ref()
                        .map_or_else(Vec::new, TeamReliefPlanner::core_reservations),
                    production_deadline: connected_preparation_horizon()
                        .saturating_add(self.context.observation.tick),
                },
            ),
        );
        retain_first_coordinator_failure(
            &mut coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_legacy_planner_claim(
                &mut obligations,
                &resources,
                LegacyPlannerClaim {
                    cadence: self.context.dials.cadence,
                    accepted_at: self.context.observation.tick,
                    decision_at: self.context.observation.tick,
                    retained_at: self.advanced.raid_started_at,
                    channel: LegacyChannel::Raid,
                    decision: &self.advanced.raid_decision,
                    protect_unspent_current_scrap: true,
                    prior_producer_intents: &self.advanced.team_decision.intents,
                    retained_units: self
                        .participants
                        .raids
                        .as_ref()
                        .map_or_else(Vec::new, |planner| planner.reservations().to_vec()),
                    production_deadline: connected_preparation_horizon()
                        .saturating_add(self.context.observation.tick),
                },
            ),
        );

        let mut active_connected = self
            .participants
            .strategy
            .as_ref()
            .and_then(|planner| planner.active_connected_obligation(self.context.observation));
        let mut active_lift = self
            .participants
            .lifts
            .as_ref()
            .and_then(LiftPlanner::active_production_obligation);
        let mut invalid_active_connected = false;
        let mut invalid_active_lift = false;
        let mut connected_import = match active_connected
            .as_ref()
            .map(active_connected_obligation)
            .transpose()
        {
            Ok(obligation) => obligation,
            Err(error) => {
                invalid_active_connected = true;
                retain_first_coordinator_failure(
                    &mut coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    Err(error.into()),
                );
                None
            }
        };
        let mut lift_import = match active_lift
            .as_ref()
            .map(active_lift_production_obligation)
            .transpose()
        {
            Ok(obligation) => obligation,
            Err(error) => {
                invalid_active_lift = true;
                retain_first_coordinator_failure(
                    &mut coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    Err(error.into()),
                );
                None
            }
        };

        if (active_connected.is_some() || active_lift.is_some())
            && lift_preceding_production_context(
                &resources,
                active_lift.as_ref(),
                active_connected.as_ref(),
                self.context.dials.cadence,
                self.context.observation.tick,
            )
            .is_none()
        {
            let connected_valid = active_connected.as_ref().is_none_or(|active| {
                lift_preceding_production_context(
                    &resources,
                    None,
                    Some(active),
                    self.context.dials.cadence,
                    self.context.observation.tick,
                )
                .is_some()
            });
            let lift_valid = active_lift.as_ref().is_none_or(|active| {
                lift_preceding_production_context(
                    &resources,
                    Some(active),
                    None,
                    self.context.dials.cadence,
                    self.context.observation.tick,
                )
                .is_some()
            });
            match (connected_valid, lift_valid) {
                (false, false) => {
                    invalid_active_connected = connected_import.is_some();
                    invalid_active_lift = lift_import.is_some();
                }
                (false, true) => invalid_active_connected = connected_import.is_some(),
                (true, false) => invalid_active_lift = lift_import.is_some(),
                (true, true) => match (connected_import.as_ref(), lift_import.as_ref()) {
                    (Some(connected), Some(lift)) if connected.owner() < lift.owner() => {
                        invalid_active_lift = true;
                    }
                    (Some(_), Some(_)) => invalid_active_connected = true,
                    _ => {}
                },
            }
        }
        if invalid_active_connected {
            active_connected = None;
            connected_import = None;
        }
        if invalid_active_lift {
            active_lift = None;
            lift_import = None;
        }

        let legacy_air_claims = if active_connected.is_some() {
            obligations.push(
                connected_import
                    .take()
                    .expect("a retained connected operation has an adapted obligation"),
            );
            None
        } else {
            self.participants
                .strategy
                .as_ref()
                .and_then(StrategicPlanner::air_operation)
                .map(|operation| {
                    (
                        self.participants
                            .strategy
                            .as_ref()
                            .and_then(StrategicPlanner::air_admitted_at)
                            .unwrap_or(self.context.observation.tick),
                        prior_planner_claims(&[], Some(operation), &[], &[], None),
                    )
                })
        };

        if active_lift.is_some() {
            obligations.push(
                lift_import
                    .take()
                    .expect("a retained Lift operation has an adapted obligation"),
            );
        }

        ObligationPreparation {
            resources,
            obligations,
            coordinator_failure,
            active_connected,
            active_lift,
            invalid_active_connected,
            invalid_active_lift,
            legacy_air_claims,
            staged_strategy: None,
        }
    }

    fn refresh_planner_claims(&self, claims: &mut ClaimSnapshot) {
        let refreshed = PlannerClaims::new(
            self.context.enlisted,
            self.participants.strategy,
            self.participants.raids,
            self.participants.lifts,
        );
        claims.planner_claims = refreshed.all(&claims.team_core_claims);
        claims.strategic_core_exclusions = refreshed.core_exclusions(&claims.team_core_claims);
    }

    fn prepare_standing_army(
        &self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) {
        let planner_owned = PlannerClaims::new(
            self.context.enlisted,
            self.participants.strategy,
            self.participants.raids,
            self.participants.lifts,
        )
        .without_executive(&claims.team_core_claims);
        let standing_army = self
            .context
            .enlisted
            .iter()
            .copied()
            .filter(|unit| planner_owned.binary_search(unit).is_err())
            .collect();
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_obligation(
                &mut obligations.obligations,
                legacy_unit_obligation(
                    self.context.observation.tick,
                    LegacyChannel::StandingArmy,
                    0,
                    standing_army,
                ),
            ),
        );
    }

    fn prepare_emergency_defense(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) -> Option<FreshEmergencyDefense> {
        if claims.opening_core.ready || obligations.coordinator_failure.is_some() {
            return None;
        }
        let available_builders =
            available_allocation_builders(&obligations.resources, &obligations.obligations);
        let current_scrap = self
            .context
            .observation
            .scrap
            .saturating_sub(current_reserve_at(
                &obligations.obligations,
                self.context.observation.tick,
            ));
        let defense = self.participants.policy.fresh_emergency_defense(
            self.context.dials,
            self.context.observation,
            FreshEmergencyDefenseContext {
                home: self.context.home,
                available_builders: &available_builders,
                unit_contacts: self.context.intelligence.units(),
                building_contacts: self.context.intelligence.buildings(),
                public_map: self.context.public_map,
                same_think_intents: &self.advanced.team_decision.intents,
                current_scrap,
            },
        )?;
        let imported = push_obligation(
            &mut obligations.obligations,
            fresh_emergency_defense_obligation(self.context.observation.tick, defense),
        );
        let accepted = imported.is_ok();
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            imported,
        );
        accepted.then_some(defense)
    }

    fn prepare_air_commitments(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) -> AirLiftPreparation {
        let mut opening_bootstrap = 0;
        if !claims.opening_core.ready {
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                push_clamped_current_reserve(
                    &mut obligations.obligations,
                    self.context.observation.scrap,
                    self.context.observation.tick,
                    self.context.observation.tick,
                    ObligationKey::OpeningCore { sequence: 0 },
                    claims.opening_core.missing_scrap,
                ),
            );
        } else {
            opening_bootstrap = self
                .participants
                .policy
                .strategic_opening_bootstrap_reserve(
                    self.context.dials,
                    self.context.observation,
                    self.context.home,
                    self.context.public_map,
                );
            for (sequence, amount) in [(1, opening_bootstrap)] {
                if amount > 0 {
                    retain_first_coordinator_failure(
                        &mut obligations.coordinator_failure,
                        AllocationCoordinatorStageTrace::ObligationCollection,
                        push_clamped_current_reserve(
                            &mut obligations.obligations,
                            self.context.observation.scrap,
                            self.context.observation.tick,
                            self.context.observation.tick,
                            ObligationKey::OpeningCore { sequence },
                            amount,
                        ),
                    );
                }
            }
        }
        let voluntary_scrap_guard = if claims.opening_core.ready {
            self.participants.policy.shallow_sentinel_capital_reserve(
                self.context.dials,
                self.context.observation,
                self.context.home,
                self.context.public_map,
                &self.advanced.team_decision.intents,
            )
        } else {
            0
        };

        let connected_air_ticks = self.participants.strategy.as_ref().map_or(0, |planner| {
            planner.remaining_airwork_ticks(self.context.observation)
        });
        let lift_air_ticks = self.participants.lifts.as_ref().map_or(0, |planner| {
            planner
                .remaining_airwork_ticks(self.context.observation, &self.advanced.lift_unavailable)
        });
        let connected_air_active = self
            .participants
            .strategy
            .as_ref()
            .is_some_and(|planner| planner.air_operation().is_some());
        let lift_air_active = self
            .participants
            .lifts
            .as_ref()
            .is_some_and(|planner| planner.operation().is_some());
        let airworks_capacity = if claims.opening_core.ready {
            self.participants.policy.airworks_capacity_commitment(
                self.context.dials,
                self.context.observation,
                self.context.home,
                (connected_air_active || lift_air_active)
                    .then_some(connected_air_ticks.saturating_add(lift_air_ticks)),
                Some(voluntary_scrap_guard),
                &claims.planner_claims,
            )
        } else {
            0
        };
        let active_lift_precedes_foundry = self.advanced.lift_was_active
            && self
                .participants
                .policy
                .operation_precedes_foundry_saving(self.advanced.lift_started_at);
        let lift_airworks_capacity = if claims.opening_core.ready && lift_air_active {
            self.participants.policy.airworks_capacity_commitment(
                self.context.dials,
                self.context.observation,
                self.context.home,
                Some(lift_air_ticks),
                Some(voluntary_scrap_guard),
                &claims.planner_claims,
            )
        } else {
            0
        };
        let lift_deadline = self
            .participants
            .lifts
            .as_ref()
            .and_then(LiftPlanner::operation)
            .map_or_else(
                || {
                    self.context
                        .observation
                        .tick
                        .saturating_add(connected_preparation_horizon())
                },
                |operation| operation.deadline,
            );

        let saved_plan_reserve_already_imported = if claims.opening_core.ready {
            opening_bootstrap
        } else {
            claims
                .opening_core
                .missing_scrap
                .min(self.context.observation.scrap)
        };
        AirLiftPreparation {
            lift_decision: StrategicDecision::default(),
            opening_bootstrap,
            airworks_capacity,
            active_lift_precedes_foundry,
            active_lift_spendable: 0,
            saved_plan_reserve_already_imported,
            lift_airworks_capacity,
            lift_deadline,
            fresh_lift_producer_jobs: 0,
            voluntary_scrap_guard,
        }
    }

    fn advance_active_lift(
        &mut self,
        obligations: &mut ObligationPreparation,
        air_lift: &mut AirLiftPreparation,
        prior_producer_intents: &[Intent],
        protect_active_connected: bool,
    ) {
        if !self.advanced.lift_was_active {
            return;
        }
        if air_lift.lift_airworks_capacity > 0 {
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                push_bounded_capital_reserve(
                    &mut obligations.obligations,
                    &obligations.resources,
                    BoundedCapitalReserve {
                        cadence: self.context.dials.cadence,
                        bank: self.context.observation.scrap,
                        accepted_at: self.advanced.lift_started_at,
                        decision_tick: self.context.observation.tick,
                        key: ObligationKey::Legacy {
                            channel: LegacyChannel::AirworksCapacity,
                            sequence: 1,
                        },
                        desired: air_lift.lift_airworks_capacity,
                        forecast_deadline: air_lift.lift_deadline,
                        older_capital_reserve: 0,
                    },
                ),
            );
        }
        let older_saved_foundry_capital = older_saved_foundry_deferrable_capital(
            &obligations.obligations,
            air_lift.active_lift_precedes_foundry,
        );
        air_lift.active_lift_spendable = self
            .context
            .observation
            .scrap
            .saturating_sub(current_reserve_at(
                &obligations.obligations,
                self.context.observation.tick,
            ))
            .saturating_sub(older_saved_foundry_capital);
        let projected_observation =
            project_producer_intents(self.context.observation, prior_producer_intents);
        let mut preceding_producer_intents = prior_producer_intents.to_vec();
        let retained_production = lift_preceding_production_context(
            &obligations.resources,
            obligations.active_lift.as_ref(),
            protect_active_connected
                .then_some(obligations.active_connected.as_ref())
                .flatten(),
            self.context.dials.cadence,
            self.context.observation.tick,
        );
        let (producer_lane_reservations, due_intents) = match retained_production {
            Some(context) => context,
            None => {
                retain_first_coordinator_failure(
                    &mut obligations.coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    Err(AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected),
                );
                (ProducerLaneReservations::default(), Vec::new())
            }
        };
        preceding_producer_intents.extend(due_intents);
        air_lift.lift_decision = self
            .participants
            .lifts
            .as_mut()
            .expect("an active lift planner exists")
            .think_with_admission_and_producer_lanes(
                &projected_observation,
                self.context.home,
                &self.advanced.lift_unavailable,
                self.advanced.initial_lift_support,
                LiftAdmission {
                    allow_new_commitments: self.advanced.preliminary_core.ready,
                    spendable_scrap: air_lift.active_lift_spendable,
                    core_reservations: &self.advanced.preliminary_core_exclusions,
                    minimum_core_equivalents: u64::from(
                        self.context.dials.minimum_core_equivalents,
                    ),
                },
                &producer_lane_reservations,
            );
        let retained_lift_units = self
            .participants
            .lifts
            .as_ref()
            .and_then(LiftPlanner::operation)
            .map_or_else(Vec::new, |operation| {
                observable_lift_operation_reservations(operation, self.context.observation)
            });
        match feasible_active_lift_current_production_prefix(
            ActiveLiftCurrentProductionContext {
                resources: &obligations.resources,
                cadence: self.context.dials.cadence,
                decision_tick: self.context.observation.tick,
                retained_at: self.advanced.lift_started_at,
                decision: &air_lift.lift_decision,
                prior_producer_intents: &preceding_producer_intents,
                production_deadline: air_lift.lift_deadline,
            },
            &obligations.obligations,
        ) {
            Ok(decision) => air_lift.lift_decision = decision,
            Err(error) => retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                Err(error),
            ),
        }
        let future_context = self
            .participants
            .lifts
            .as_ref()
            .and_then(LiftPlanner::operation)
            .map(|operation| ActiveLiftFutureProductionContext {
                resources: &obligations.resources,
                observation: self.context.observation,
                operation,
                unavailable: &self.advanced.lift_unavailable,
                prior_producer_intents: &preceding_producer_intents,
                lift_decision: &air_lift.lift_decision,
                cadence: self.context.dials.cadence,
                accepted_at: self.advanced.lift_started_at,
            });
        // The operation's desired carrier count remains a tactical target, not
        // debt the economy has already incurred. Preserve only the unpaid
        // prefix that the shared allocator can actually fund and schedule
        // beside every older obligation through the immutable Lift deadline.
        let mut feasibility_obligations = obligations.obligations.clone();
        let provisional_current = push_legacy_planner_claim(
            &mut feasibility_obligations,
            &obligations.resources,
            LegacyPlannerClaim {
                cadence: self.context.dials.cadence,
                accepted_at: self.context.observation.tick,
                decision_at: self.context.observation.tick,
                retained_at: self.advanced.lift_started_at,
                channel: LegacyChannel::Lift,
                decision: &air_lift.lift_decision,
                protect_unspent_current_scrap: false,
                prior_producer_intents: &preceding_producer_intents,
                retained_units: retained_lift_units.clone(),
                production_deadline: air_lift.lift_deadline,
            },
        );
        let future_lift_obligation = if provisional_current.is_ok()
            && obligations.active_lift.is_none()
        {
            future_context.map_or(Ok(None), |context| {
                feasible_active_lift_future_production_obligation(context, &feasibility_obligations)
            })
        } else {
            Ok(None)
        };
        let future_lift_claimed = future_lift_obligation
            .as_ref()
            .is_ok_and(|obligation| obligation.is_some());
        air_lift.fresh_lift_producer_jobs = future_lift_obligation
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map_or(0, |obligation| obligation.claims.producer_jobs().len());
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_legacy_planner_claim(
                &mut obligations.obligations,
                &obligations.resources,
                LegacyPlannerClaim {
                    cadence: self.context.dials.cadence,
                    accepted_at: self.context.observation.tick,
                    decision_at: self.context.observation.tick,
                    retained_at: self.advanced.lift_started_at,
                    channel: LegacyChannel::Lift,
                    decision: &air_lift.lift_decision,
                    protect_unspent_current_scrap: !future_lift_claimed,
                    prior_producer_intents: &preceding_producer_intents,
                    retained_units: retained_lift_units,
                    production_deadline: self
                        .participants
                        .lifts
                        .as_ref()
                        .and_then(LiftPlanner::operation)
                        .map_or_else(
                            || {
                                connected_preparation_horizon()
                                    .saturating_add(self.context.observation.tick)
                            },
                            |operation| operation.deadline,
                        ),
                },
            ),
        );
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            match future_lift_obligation {
                Ok(Some(obligation)) => {
                    obligations.obligations.push(obligation);
                    Ok(())
                }
                Ok(None) => Ok(()),
                Err(error) => Err((&error).into()),
            },
        );
    }

    fn prepare_saved_foundry(
        &mut self,
        claims: &ClaimSnapshot,
        air_lift: &AirLiftPreparation,
        obligations: &mut ObligationPreparation,
    ) -> SavedFoundryPreparation {
        let current_before_saved = self
            .context
            .observation
            .scrap
            .saturating_sub(current_reserve_at(
                &obligations.obligations,
                self.context.observation.tick,
            ))
            .saturating_add(air_lift.saved_plan_reserve_already_imported)
            .min(self.context.observation.scrap);
        let mut obligation = self.participants.policy.validated_foundry_obligation(
            self.context.observation,
            &obligations.resources,
            claims.opening_core.ready,
            current_before_saved,
        );
        let mut preparation_need = None;
        if let Some(saved) = obligation.filter(|saved| !saved.blocked()) {
            let available_builders =
                available_allocation_builders(&obligations.resources, &obligations.obligations);
            match self.participants.policy.saved_foundry_readiness(
                self.context.dials,
                self.context.observation,
                saved,
                FreshFoundryProposalContext {
                    home: self.context.home,
                    available_builders: &available_builders,
                    combat_core_exclusions: &claims.strategic_core_exclusions,
                    unit_contacts: self.context.intelligence.units(),
                    building_contacts: self.context.intelligence.buildings(),
                    public_map: self.context.public_map,
                    same_think_intents: &self.advanced.team_decision.intents,
                    current_scrap: saved.planning_scrap(),
                    protected_reserve: saved.protected_reserve(),
                },
            ) {
                SavedFoundryReadiness::Ready => {}
                SavedFoundryReadiness::NeedsProtection {
                    anchor,
                    target_strength,
                } => {
                    self.participants
                        .policy
                        .release_saved_foundry_for_preparation();
                    obligation = None;
                    preparation_need = Some((anchor, target_strength));
                }
                SavedFoundryReadiness::Blocked => {
                    if self
                        .participants
                        .policy
                        .retain_blocked_foundry_saving(self.context.observation.tick)
                    {
                        obligation = Some(saved.blocked_by_execution());
                    } else {
                        obligation = None;
                    }
                }
            }
        }
        let mut saving = 0;
        let blocked = obligation.is_some_and(ValidatedFoundryObligation::blocked);
        if let Some(saved) = obligation {
            saving = saved
                .current_construction_capital()
                .saturating_add(saved.forecast_construction_capital())
                .saturating_add(saved.protected_reserve());
            let unrepresented_protected_reserve = saved
                .protected_reserve()
                .saturating_sub(air_lift.saved_plan_reserve_already_imported);
            if unrepresented_protected_reserve > 0 {
                retain_first_coordinator_failure(
                    &mut obligations.coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    push_clamped_current_reserve(
                        &mut obligations.obligations,
                        self.context.observation.scrap,
                        saved.accepted_at(),
                        self.context.observation.tick,
                        ObligationKey::OpeningCore { sequence: 3 },
                        unrepresented_protected_reserve,
                    ),
                );
            }
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                push_obligation(
                    &mut obligations.obligations,
                    saved_foundry_obligation(saved),
                ),
            );
        }
        SavedFoundryPreparation {
            obligation,
            saving,
            blocked,
            preparation_need,
        }
    }

    fn stage_active_island(
        &mut self,
        claims: &mut ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) {
        if obligations.coordinator_failure.is_some() {
            return;
        }
        if !self
            .participants
            .strategy
            .as_ref()
            .is_some_and(|planner| planner.has_active_island_operation())
        {
            return;
        }
        let accepted_at = self
            .participants
            .strategy
            .as_ref()
            .and_then(StrategicPlanner::air_admitted_at);
        let protected_current_scrap =
            current_reserve_at(&obligations.obligations, self.context.observation.tick);
        let production_deadline = self
            .context
            .observation
            .tick
            .saturating_add(connected_preparation_horizon());
        let protected_forecast_scrap =
            forecast_reserve_through(&obligations.obligations, production_deadline);
        let Some((producer_lane_reservations, prior_producer_intents)) = retained_producer_context(
            &obligations.resources,
            &obligations.obligations,
            self.context.dials.cadence,
            self.context.observation.tick,
        ) else {
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                Err(AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected),
            );
            return;
        };
        let Some(result) = self.participants.strategy.as_mut().and_then(|planner| {
            planner.stage_active_island(
                StrategicThinkContext::new(
                    self.context.profile,
                    self.context.tuning,
                    self.context.observation,
                    self.context.intelligence,
                    self.context.home,
                    StrategicCoordination {
                        enlisted: &claims.planner_claims,
                        lift_support: self.context.lift_support,
                        allow_new_operation: claims.opening_core.ready,
                        protected_current_scrap,
                        protected_forecast_scrap,
                        public_map: Some(self.context.public_map),
                        orientation: self.context.orientation,
                    },
                )
                .with_producer_lanes(&prior_producer_intents, &producer_lane_reservations),
            )
        }) else {
            return;
        };
        let accepted_at = accepted_at.expect("a staged island operation has an admission tick");
        let retained_units = result.decision.reservations.clone();
        obligations.legacy_air_claims = None;
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_legacy_planner_claim(
                &mut obligations.obligations,
                &obligations.resources,
                LegacyPlannerClaim {
                    cadence: self.context.dials.cadence,
                    accepted_at,
                    decision_at: self.context.observation.tick,
                    retained_at: accepted_at,
                    channel: LegacyChannel::StrategicAir,
                    decision: &result.decision,
                    protect_unspent_current_scrap: true,
                    prior_producer_intents: &prior_producer_intents,
                    retained_units,
                    production_deadline,
                },
            ),
        );
        obligations.staged_strategy = Some(result);
    }

    fn prepare_active_connected_revision(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) -> ActiveRevisionPreparation {
        let deadline = self
            .participants
            .strategy
            .as_ref()
            .and_then(StrategicPlanner::connected_package_diagnostics)
            .map(|diagnostics| diagnostics.preparation_deadline)
            .unwrap_or_else(|| {
                self.context
                    .observation
                    .tick
                    .saturating_add(connected_preparation_horizon())
            });
        let connected_precedes_foundry = self
            .participants
            .strategy
            .as_ref()
            .and_then(StrategicPlanner::air_admitted_at)
            .is_some_and(|accepted_at| {
                self.participants
                    .policy
                    .operation_precedes_foundry_saving(accepted_at)
            });
        let other_obligations = obligations
            .obligations
            .iter()
            .filter(|obligation| {
                !matches!(obligation.key, ObligationKey::ConnectedOffense { .. })
                    && !(connected_precedes_foundry
                        && matches!(obligation.key, ObligationKey::SavedFoundry { .. }))
            })
            .cloned()
            .collect::<Vec<_>>();
        let protected_current_scrap =
            current_reserve_at(&other_obligations, self.context.observation.tick);
        let protected_forecast_scrap = forecast_reserve_through(&other_obligations, deadline);
        let request = FreshConnectedProposalRequest::new(
            self.context.profile,
            self.context.tuning,
            self.context.observation,
            &obligations.resources,
            self.context.intelligence,
            self.context.home,
            StrategicCoordination {
                enlisted: &claims.planner_claims,
                lift_support: None,
                allow_new_operation: true,
                protected_current_scrap,
                protected_forecast_scrap,
                public_map: Some(self.context.public_map),
                orientation: self.context.orientation,
            },
        );
        let revision = self
            .participants
            .strategy
            .as_ref()
            .map_or(Ok(None), |planner| {
                planner.active_connected_revision_proposal(request)
            });
        match revision {
            Ok(None) => ActiveRevisionPreparation::default(),
            Ok(Some(proposal)) => {
                remove_active_connected_obligation(&mut obligations.obligations);
                let adapted = active_connected_revision_obligation(&proposal);
                retain_first_coordinator_failure(
                    &mut obligations.coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    push_obligation(&mut obligations.obligations, adapted),
                );
                obligations.active_connected = None;
                obligations.legacy_air_claims = None;
                ActiveRevisionPreparation {
                    proposal: Some(proposal),
                    rejected: None,
                }
            }
            Err(rejected) => {
                let retained_units = active_air_units(
                    self.participants.strategy.as_ref(),
                    self.context.observation,
                );
                remove_active_connected_obligation(&mut obligations.obligations);
                self.participants
                    .strategy
                    .as_mut()
                    .expect("a rejected active revision belongs to its planner")
                    .reject_active_connected_revision(
                        rejected.reason,
                        self.context.observation.tick,
                    );
                retain_first_coordinator_failure(
                    &mut obligations.coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    push_obligation(
                        &mut obligations.obligations,
                        legacy_unit_obligation(
                            self.context.observation.tick,
                            LegacyChannel::StrategicAir,
                            0,
                            retained_units,
                        ),
                    ),
                );
                obligations.active_connected = None;
                obligations.legacy_air_claims = None;
                ActiveRevisionPreparation {
                    proposal: None,
                    rejected: Some(rejected),
                }
            }
        }
    }

    fn prepare_fresh_investments(
        &mut self,
        claims: &ClaimSnapshot,
        saved: &SavedFoundryPreparation,
        obligations: &mut ObligationPreparation,
        active_revision: ActiveRevisionPreparation,
    ) -> FreshInvestmentPreparation {
        let admission_tick = strategic_admission_tick(self.context.observation.tick)
            && claims.opening_core.ready
            && !saved.blocked
            && obligations.coordinator_failure.is_none();
        let available_builders =
            available_allocation_builders(&obligations.resources, &obligations.obligations);
        let foundry_investment = admission_tick
            .then(|| {
                self.participants.policy.fresh_foundry_investment(
                    self.context.dials,
                    self.context.observation,
                    &obligations.resources,
                    FreshFoundryProposalContext {
                        home: self.context.home,
                        available_builders: &available_builders,
                        combat_core_exclusions: &claims.strategic_core_exclusions,
                        unit_contacts: self.context.intelligence.units(),
                        building_contacts: self.context.intelligence.buildings(),
                        public_map: self.context.public_map,
                        same_think_intents: &self.advanced.team_decision.intents,
                        current_scrap: self.context.observation.scrap,
                        protected_reserve: current_reserve_at(
                            &obligations.obligations,
                            self.context.observation.tick,
                        ),
                    },
                )
            })
            .flatten();
        let expansion_security_need = foundry_investment
            .as_ref()
            .and_then(FreshFoundryInvestment::preparation_need)
            .or(saved.preparation_need);
        let foundry = match foundry_investment {
            Some(FreshFoundryInvestment::Ready(proposal)) => Some(proposal),
            Some(FreshFoundryInvestment::NeedsProtection { .. }) | None => None,
        };

        let mut rejected_connected_candidate = active_revision.rejected;
        let connected_candidate = if active_revision.proposal.is_some() {
            active_revision.proposal
        } else if admission_tick
            && obligations.staged_strategy.is_none()
            && !obligations.invalid_active_connected
        {
            self.participants.strategy.as_ref().and_then(|planner| {
                match planner.fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                    self.context.profile,
                    self.context.tuning,
                    self.context.observation,
                    &obligations.resources,
                    self.context.intelligence,
                    self.context.home,
                    StrategicCoordination {
                        enlisted: &claims.planner_claims,
                        lift_support: None,
                        allow_new_operation: true,
                        protected_current_scrap: current_reserve_at(
                            &obligations.obligations,
                            self.context.observation.tick,
                        ),
                        protected_forecast_scrap: forecast_reserve_through(
                            &obligations.obligations,
                            self.context
                                .observation
                                .tick
                                .saturating_add(connected_preparation_horizon()),
                        ),
                        public_map: Some(self.context.public_map),
                        orientation: self.context.orientation,
                    },
                )) {
                    Ok(proposal) => proposal,
                    Err(rejected) => {
                        rejected_connected_candidate = Some(rejected);
                        None
                    }
                }
            })
        } else {
            None
        };
        let connected = connected_candidate;
        let connected_accepted_at = obligations
            .active_connected
            .as_ref()
            .map(ActiveConnectedObligation::accepted_at)
            .or_else(|| connected.as_ref().map(FreshConnectedProposal::accepted_at));
        if connected.is_none()
            && let Some((accepted_at, mut air_claims)) = obligations.legacy_air_claims.take()
        {
            retain_observed_units(&obligations.resources, &mut air_claims);
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                push_obligation(
                    &mut obligations.obligations,
                    legacy_unit_obligation(accepted_at, LegacyChannel::StrategicAir, 0, air_claims),
                ),
            );
        }
        let connected_reserve_deadline = connected
            .as_ref()
            .map(FreshConnectedProposal::deadline)
            .or_else(|| {
                obligations
                    .active_connected
                    .as_ref()
                    .map(ActiveConnectedObligation::deadline)
            })
            .unwrap_or_else(|| {
                self.context
                    .observation
                    .tick
                    .saturating_add(connected_preparation_horizon())
            });
        let committed_production = self.committed_standing_production();
        let standing_force_derivation = StandingForceDerivation {
            projection_targets: self.standing_force_projection_targets(),
            residual_investment_reserve: self.participants.policy.residual_investment_reserve(
                self.context.dials,
                self.context.observation,
                ResidualInvestmentReserveContext {
                    home: self.context.home,
                    available_builders: &available_builders,
                    unit_contacts: self.context.intelligence.units(),
                    building_contacts: self.context.intelligence.buildings(),
                    public_map: self.context.public_map,
                },
            ),
            expansion_security_need,
        };
        let derive_standing_force =
            |unit_exclusions: &[UnitId],
             connected_paid_production: &[StandingProductionCommitment]| {
                self.derive_standing_force(
                    claims,
                    obligations,
                    &standing_force_derivation,
                    &committed_production,
                    unit_exclusions,
                    connected_paid_production,
                )
            };
        let standing_force = if let Some(proposal) = connected.as_ref() {
            let mut standing_force_cache = Vec::<(
                Vec<UnitId>,
                Vec<StandingProductionCommitment>,
                Vec<StandingForceProposal>,
            )>::new();
            let mut derive_contextual_standing_force =
                |unit_exclusions: &[UnitId], paid_production: &[StandingProductionCommitment]| {
                    if let Some((_, _, proposals)) = standing_force_cache.iter().find(
                        |(cached_exclusions, cached_production, _)| {
                            cached_exclusions == unit_exclusions
                                && cached_production == paid_production
                        },
                    ) {
                        return proposals.clone();
                    }
                    let proposals = derive_standing_force(unit_exclusions, paid_production);
                    standing_force_cache.push((
                        unit_exclusions.to_vec(),
                        paid_production.to_vec(),
                        proposals.clone(),
                    ));
                    proposals
                };
            let key = ConnectedOffenseKey {
                objective: proposal.objective(),
                anchor: proposal.anchor(),
            };
            let mut contexts = Vec::with_capacity(
                usize::from(!proposal.revises_active_operation())
                    .saturating_add(1)
                    .saturating_add(proposal.marginal_variants().len()),
            );
            if !proposal.revises_active_operation() {
                contexts.push(ContextualStandingForce {
                    context: ConnectedPortfolioContext::Absent,
                    proposals: derive_contextual_standing_force(
                        &claims.strategic_core_exclusions,
                        &[],
                    ),
                });
            }
            let mut selected_exclusions = claims.strategic_core_exclusions.clone();
            selected_exclusions.extend_from_slice(proposal.minimum_claims().units());
            selected_exclusions.sort_unstable();
            selected_exclusions.dedup();
            let mut selected_paid_production = proposal
                .minimum_claims()
                .paid_providers()
                .iter()
                .map(|provider| {
                    StandingProductionCommitment::paid(provider.producer(), provider.kind())
                })
                .collect::<Vec<_>>();
            selected_paid_production.sort_unstable();
            contexts.push(ContextualStandingForce {
                context: ConnectedPortfolioContext::Selected {
                    key,
                    marginal_depth: 0,
                },
                proposals: derive_contextual_standing_force(
                    &selected_exclusions,
                    &selected_paid_production,
                ),
            });
            for (marginal_index, marginal) in proposal.marginal_variants().iter().enumerate() {
                selected_exclusions.extend_from_slice(marginal.additions().units());
                selected_exclusions.sort_unstable();
                selected_exclusions.dedup();
                selected_paid_production.extend(marginal.additions().paid_providers().iter().map(
                    |provider| {
                        StandingProductionCommitment::paid(provider.producer(), provider.kind())
                    },
                ));
                selected_paid_production.sort_unstable();
                contexts.push(ContextualStandingForce {
                    context: ConnectedPortfolioContext::Selected {
                        key,
                        marginal_depth: marginal_index.saturating_add(1),
                    },
                    proposals: derive_contextual_standing_force(
                        &selected_exclusions,
                        &selected_paid_production,
                    ),
                });
            }
            StandingForcePreparation::ConnectedContexts(contexts)
        } else {
            StandingForcePreparation::Unconditional(derive_standing_force(
                &claims.strategic_core_exclusions,
                &[],
            ))
        };
        FreshInvestmentPreparation {
            foundry,
            connected,
            standing_force,
            standing_force_derivation,
            connected_accepted_at,
            connected_reserve_deadline,
            rejected_connected_candidate,
        }
    }

    fn committed_standing_production(&mut self) -> Vec<StandingProductionCommitment> {
        let mut committed = Vec::new();
        if let Some(planner) = self.participants.strategy.as_mut() {
            committed.extend(
                planner
                    .issued_connected_production_assignments(self.context.observation)
                    .into_iter()
                    .map(|assignment| {
                        StandingProductionCommitment::paid(assignment.producer(), assignment.kind())
                    }),
            );
        }
        if let Some(planner) = self.participants.lifts.as_ref() {
            committed.extend(
                planner
                    .issued_production_assignments(self.context.observation.tick)
                    .into_iter()
                    .map(|assignment| {
                        StandingProductionCommitment::paid(assignment.producer(), assignment.kind())
                    }),
            );
        }
        committed
    }

    fn derive_standing_force(
        &self,
        claims: &ClaimSnapshot,
        obligations: &ObligationPreparation,
        derivation: &StandingForceDerivation,
        committed_production: &[StandingProductionCommitment],
        unit_exclusions: &[UnitId],
        connected_paid_production: &[StandingProductionCommitment],
    ) -> Vec<StandingForceProposal> {
        if !claims.opening_core.ready || obligations.coordinator_failure.is_some() {
            return Vec::new();
        }
        let owned_production =
            merged_production_commitments(committed_production, connected_paid_production);
        let mut context = StandingForceContext::new(unit_exclusions, &owned_production)
            .with_ground_routing(
                StandingGroundTarget::footprint(
                    self.context.home,
                    BuildingKind::Foundry.base_stats().size,
                ),
                Some(self.context.public_map),
                &derivation.projection_targets,
                Some(self.context.orientation),
            )
            .with_minimum_residual_scrap(derivation.residual_investment_reserve);
        if let Some((anchor, target_strength)) = derivation.expansion_security_need {
            context = context.with_expansion_security(
                StandingGroundTarget::footprint(anchor, BuildingKind::Foundry.base_stats().size),
                target_strength,
            );
        }
        derive_standing_force_proposals(
            self.context.observation,
            self.context.intelligence,
            self.context.profile,
            self.context.tuning,
            &obligations.resources,
            context,
        )
    }

    fn standing_force_projection_targets(&self) -> Vec<StandingGroundTarget> {
        let mut targets = self
            .participants
            .policy
            .uncleared_hostile_starts(self.context.public_map, self.context.observation.me)
            .into_iter()
            .map(|start| {
                StandingGroundTarget::footprint(
                    start.anchor,
                    BuildingKind::Foundry.base_stats().size,
                )
            })
            .collect::<Vec<_>>();
        targets.extend(
            self.context
                .intelligence
                .buildings()
                .iter()
                .filter(|contact| {
                    contact.hp > 0 && contact.confidence_at(self.context.observation.tick) > 0
                })
                .map(|contact| {
                    StandingGroundTarget::footprint(
                        contact.anchor,
                        contact.kind.tier_stats(contact.tier).size,
                    )
                }),
        );
        targets.extend(
            self.context
                .intelligence
                .units()
                .iter()
                .filter(|contact| {
                    contact.hp > 0
                        && contact.confidence_at(self.context.observation.tick) > 0
                        && contact.body_domain() == Domain::Ground
                })
                .map(|contact| StandingGroundTarget::point(contact.tile)),
        );
        targets
    }

    fn downgrade_unfundable_active_revision(
        &mut self,
        claims: &ClaimSnapshot,
        saved: &mut SavedFoundryPreparation,
        air_lift: &AirLiftPreparation,
        obligations: &mut ObligationPreparation,
        fresh: &mut FreshInvestmentPreparation,
    ) {
        let Some(revision) = fresh
            .connected
            .as_ref()
            .filter(|proposal| proposal.revises_active_operation())
        else {
            return;
        };
        let identity = revision.identity();
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: revision.accepted_at(),
            key: ObligationKey::ConnectedOffense {
                objective: identity.objective(),
                anchor: identity.anchor(),
            },
        };
        let horizon = self
            .allocation_horizon(saved, fresh, None)
            .max(air_lift.lift_deadline);
        let Ok(mut allocation) =
            CrossDomainAllocation::new(&obligations.resources, horizon, self.context.dials.cadence)
        else {
            return;
        };
        for obligation in obligations.obligations.iter().cloned() {
            allocation.import(obligation);
        }
        let Err(AllocationError::ObligationConflict {
            obligation,
            conflict,
        }) = allocation.resolve(AllocationPersonality::default(), None)
        else {
            return;
        };
        if obligation != owner || !connected_production_conflict(&conflict) {
            return;
        }
        if self.defer_younger_saved_foundry(saved, obligations, revision.accepted_at()) {
            self.downgrade_unfundable_active_revision(claims, saved, air_lift, obligations, fresh);
            return;
        }

        let retained_units = active_air_units(
            self.participants.strategy.as_ref(),
            self.context.observation,
        );
        obligations
            .obligations
            .retain(|obligation| obligation.owner() != owner);
        self.participants
            .strategy
            .as_mut()
            .expect("an active revision belongs to its planner")
            .recover_unfundable_active_connected(self.context.observation.tick);
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_obligation(
                &mut obligations.obligations,
                legacy_unit_obligation(
                    self.context.observation.tick,
                    LegacyChannel::StrategicAir,
                    0,
                    retained_units,
                ),
            ),
        );
        fresh.connected = None;
        fresh.connected_accepted_at = None;
        fresh.connected_reserve_deadline = self
            .context
            .observation
            .tick
            .saturating_add(connected_preparation_horizon());
        let committed_production = self.committed_standing_production();
        let standing_force = self.derive_standing_force(
            claims,
            obligations,
            &fresh.standing_force_derivation,
            &committed_production,
            &claims.strategic_core_exclusions,
            &[],
        );
        fresh.standing_force = StandingForcePreparation::Unconditional(standing_force);
    }

    fn downgrade_unfundable_active_connected(
        &mut self,
        saved: &mut SavedFoundryPreparation,
        air_lift: &AirLiftPreparation,
        obligations: &mut ObligationPreparation,
    ) {
        let Some(active) = obligations.active_connected.clone() else {
            return;
        };
        let identity = active.identity();
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: active.accepted_at(),
            key: ObligationKey::ConnectedOffense {
                objective: identity.objective(),
                anchor: identity.anchor(),
            },
        };
        let horizon = self
            .context
            .observation
            .tick
            .saturating_add(connected_preparation_horizon())
            .max(
                self.context
                    .observation
                    .tick
                    .saturating_add(self.context.dials.cadence),
            )
            .max(active.deadline())
            .max(air_lift.lift_deadline);
        let horizon = saved
            .obligation
            .map_or(horizon, |foundry| horizon.max(foundry.forecast_deadline()));
        let Ok(mut allocation) =
            CrossDomainAllocation::new(&obligations.resources, horizon, self.context.dials.cadence)
        else {
            return;
        };
        for obligation in obligations.obligations.iter().cloned() {
            allocation.import(obligation);
        }
        let Err(AllocationError::ObligationConflict {
            obligation,
            conflict,
        }) = allocation.resolve(AllocationPersonality::default(), None)
        else {
            return;
        };
        if obligation != owner || !connected_production_conflict(&conflict) {
            return;
        }
        if self.defer_younger_saved_foundry(saved, obligations, active.accepted_at()) {
            self.downgrade_unfundable_active_connected(saved, air_lift, obligations);
            return;
        }

        let mut retained_units = active.units().to_vec();
        retain_observed_units(&obligations.resources, &mut retained_units);
        obligations
            .obligations
            .retain(|obligation| obligation.owner() != owner);
        retain_first_coordinator_failure(
            &mut obligations.coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_obligation(
                &mut obligations.obligations,
                legacy_unit_obligation(
                    active.accepted_at(),
                    LegacyChannel::StrategicAir,
                    0,
                    retained_units,
                ),
            ),
        );
        self.participants
            .strategy
            .as_mut()
            .expect("an active connected obligation can only come from its planner")
            .recover_unfundable_active_connected(self.context.observation.tick);
        obligations.active_connected = None;
    }

    fn defer_younger_saved_foundry(
        &self,
        saved: &mut SavedFoundryPreparation,
        obligations: &mut ObligationPreparation,
        connected_accepted_at: Tick,
    ) -> bool {
        let Some(foundry) = saved.obligation else {
            return false;
        };
        if connected_accepted_at > foundry.accepted_at()
            || !self
                .participants
                .policy
                .operation_precedes_foundry_saving(connected_accepted_at)
        {
            return false;
        }
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: foundry.accepted_at(),
            key: ObligationKey::SavedFoundry {
                anchor: foundry.anchor(),
            },
        };
        let before = obligations.obligations.len();
        obligations
            .obligations
            .retain(|obligation| obligation.owner() != owner);
        if obligations.obligations.len() == before {
            return false;
        }
        saved.obligation = None;
        true
    }

    fn allocation_horizon(
        &self,
        saved: &SavedFoundryPreparation,
        fresh: &FreshInvestmentPreparation,
        active_connected: Option<&ActiveConnectedObligation>,
    ) -> Tick {
        let mut horizon = self
            .context
            .observation
            .tick
            .saturating_add(connected_preparation_horizon())
            .max(
                self.context
                    .observation
                    .tick
                    .saturating_add(self.context.dials.cadence),
            );
        if let Some(obligation) = saved.obligation {
            horizon = horizon.max(obligation.forecast_deadline());
        }
        if let Some(active) = active_connected {
            horizon = horizon.max(active.deadline());
        }
        if let Some(operation) = self
            .participants
            .lifts
            .as_ref()
            .and_then(LiftPlanner::operation)
        {
            horizon = horizon.max(operation.deadline);
        }
        if let Some(proposal) = fresh.foundry.as_ref() {
            horizon = horizon.max(proposal.forecast_deadline());
        }
        if let Some(proposal) = fresh.connected.as_ref() {
            horizon = horizon.max(proposal.deadline());
        }
        fresh.standing_force.for_each(|proposal| {
            horizon = horizon.max(proposal.ready_before());
        });
        horizon
    }

    /// Resolves the complete prepared portfolio once. Domain payloads stay
    /// opaque and retain their exact original ranking and identity.
    fn resolve(
        &mut self,
        mut prepared: PreparedAllocation,
        snapshots: CommitSnapshots,
    ) -> ResolvedAllocation {
        let mut allocation_ok = prepared.coordinator_failure.is_none();
        let mut settlement = None;
        if allocation_ok {
            match CrossDomainAllocation::new(
                &prepared.resources,
                prepared.allocation_horizon,
                self.context.dials.cadence,
            ) {
                Ok(mut allocation) => {
                    let allocatable_voluntary_scrap_guard = prepared
                        .voluntary_scrap_guard
                        .min(prepared.resources.current_scrap().amount());
                    for obligation in prepared.obligations.iter().cloned() {
                        allocation.import(obligation);
                    }
                    if let Some(proposal) = prepared.fresh_foundry.take() {
                        match foundry_investment_proposal(proposal) {
                            Ok(proposal) => allocation.offer(
                                proposal
                                    .with_voluntary_scrap_guard(allocatable_voluntary_scrap_guard),
                            ),
                            Err(error) => {
                                retain_first_coordinator_failure(
                                    &mut prepared.coordinator_failure,
                                    AllocationCoordinatorStageTrace::FoundryProposalAdaptation,
                                    Err(error.into()),
                                );
                                allocation_ok = false;
                            }
                        }
                    }
                    if let Some(proposal) = prepared.fresh_connected.take() {
                        if proposal.revises_active_operation() {
                            allocation
                                .offer(active_connected_revision_investment_proposal(proposal));
                        } else {
                            match connected_investment_proposal(proposal) {
                                Ok(proposal) => {
                                    allocation.offer(proposal.with_voluntary_scrap_guard(
                                        allocatable_voluntary_scrap_guard,
                                    ))
                                }
                                Err(error) => {
                                    retain_first_coordinator_failure(
                                        &mut prepared.coordinator_failure,
                                        AllocationCoordinatorStageTrace::ConnectedProposalAdaptation,
                                        Err(error.into()),
                                    );
                                    allocation_ok = false;
                                }
                            }
                        }
                    }
                    match core::mem::take(&mut prepared.standing_force) {
                        StandingForcePreparation::Unconditional(standing_force) => {
                            match standing_force_investment_proposals(standing_force) {
                                Ok(proposals) => {
                                    for proposal in proposals {
                                        allocation.offer(standing_force_with_voluntary_guard(
                                            proposal,
                                            allocatable_voluntary_scrap_guard,
                                        ));
                                    }
                                }
                                Err(error) => {
                                    retain_first_coordinator_failure(
                                        &mut prepared.coordinator_failure,
                                        AllocationCoordinatorStageTrace::StandingForceProposalAdaptation,
                                        Err(error.into()),
                                    );
                                    allocation_ok = false;
                                }
                            }
                        }
                        StandingForcePreparation::ConnectedContexts(contexts) => {
                            for context in contexts {
                                match standing_force_investment_proposals(context.proposals) {
                                    Ok(proposals) => allocation.offer_context(
                                        context.context,
                                        proposals
                                            .into_iter()
                                            .map(|proposal| {
                                                standing_force_with_voluntary_guard(
                                                    proposal,
                                                    allocatable_voluntary_scrap_guard,
                                                )
                                            })
                                            .collect(),
                                    ),
                                    Err(error) => {
                                        retain_first_coordinator_failure(
                                            &mut prepared.coordinator_failure,
                                            AllocationCoordinatorStageTrace::StandingForceProposalAdaptation,
                                            Err(error.into()),
                                        );
                                        allocation_ok = false;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if allocation_ok {
                        match allocation.resolve(
                            AllocationPersonality::from_profile(self.context.profile),
                            self.trace.as_deref_mut(),
                        ) {
                            Ok(resolved) => settlement = Some(resolved),
                            Err(_) => allocation_ok = false,
                        }
                    }
                }
                Err(error) => {
                    retain_first_coordinator_failure(
                        &mut prepared.coordinator_failure,
                        AllocationCoordinatorStageTrace::CapacityProjection,
                        Err((&error).into()),
                    );
                    allocation_ok = false;
                }
            }
        }
        ResolvedAllocation {
            prepared,
            settlement,
            snapshots,
            allocation_ok,
        }
    }

    fn commit_settlement(
        &mut self,
        prepared: &mut PreparedAllocation,
        settlement: CrossDomainSettlement,
        allocation_ok: &mut bool,
    ) -> CommitEffects {
        let mut effects = CommitEffects::frozen(prepared);
        let producer_schedule = settlement.producer_schedule().to_vec();
        let voluntary_scrap_guard = if settlement.voluntary_scrap_guard_satisfied() {
            0
        } else {
            prepared.voluntary_scrap_guard
        };
        effects.budget.voluntary_scrap_guard = voluntary_scrap_guard;
        effects.budget.residual_scrap = settlement
            .residual_current_scrap()
            .saturating_sub(voluntary_scrap_guard);
        effects.budget.connected_spendable =
            if prepared.active_connected.is_some() || prepared.staged_strategy.is_some() {
                settlement.connected_current_scrap()
            } else {
                settlement
                    .connected_current_scrap()
                    .saturating_sub(voluntary_scrap_guard)
            };
        if prepared.saved_foundry.is_some_and(|saved| {
            prepared
                .connected_accepted_at
                .is_some_and(|accepted_at| accepted_at <= saved.accepted_at())
        }) {
            effects.budget.prior_operation_spendable = effects.budget.connected_spendable;
        }
        if prepared.active_lift_precedes_foundry {
            effects.budget.prior_operation_spendable = effects
                .budget
                .prior_operation_spendable
                .max(prepared.active_lift_spendable);
        }
        effects.budget.connected_forecast_hold =
            settlement.connected_forecast_reserve_through(prepared.connected_reserve_deadline);
        effects.budget.utility_spendable = settlement.utility_current_scrap();
        effects.allocated_producer_intents = producer_schedule
            .iter()
            .filter(|job| job.enqueued_at == self.context.observation.tick)
            .map(|job| Intent::TrainAt {
                building: job.producer,
                kind: job.kind,
            })
            .collect();
        effects.producer_lane_reservations = settlement.producer_lane_reservations().clone();

        self.commit_emergency_defense(prepared, &mut effects);
        self.bind_saved_foundry_funding(prepared, &settlement, allocation_ok);
        self.refresh_and_bind_lift(prepared, &producer_schedule, allocation_ok);
        let mut payloads = settlement.into_payloads();
        self.dispatch_ready_saved_foundry(prepared, &mut effects, allocation_ok);
        self.refresh_active_connected(prepared, &producer_schedule, allocation_ok);
        self.commit_fresh_connected(
            prepared,
            &producer_schedule,
            &mut payloads,
            &mut effects,
            allocation_ok,
        );
        self.commit_fresh_foundry(prepared, &mut payloads, &mut effects, allocation_ok);
        if let Some(standing_force) = payloads.take_standing_force() {
            debug_assert!(producer_schedule.iter().any(|job| {
                job.owner == ClaimOwner::Proposal(ProposalKey::StandingForce(standing_force.key()))
                    && job.enqueued_at == self.context.observation.tick
                    && job.kind == standing_force.key_kind()
            }));
        }
        effects
    }

    fn refresh_and_bind_lift(
        &mut self,
        prepared: &mut PreparedAllocation,
        producer_schedule: &[super::ScheduledProducerJob],
        allocation_ok: &mut bool,
    ) {
        if !*allocation_ok {
            return;
        }
        if prepared.active_lift.is_none() && prepared.fresh_lift_producer_jobs == 0 {
            return;
        }
        let planner = self
            .participants
            .lifts
            .as_mut()
            .expect("an active Lift allocation belongs to its planner");
        let mut due_ordinals = Vec::new();
        if let Some(active) = prepared.active_lift.as_ref() {
            let assignments = active_lift_producer_assignments(active, producer_schedule);
            if planner
                .refresh_active_production_funding(active, &assignments)
                .is_err()
            {
                retain_first_coordinator_failure(
                    &mut prepared.coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    Err(AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected),
                );
                *allocation_ok = false;
                return;
            }
            due_ordinals.extend(assignments.iter().filter_map(|assignment| {
                (assignment.timing().enqueued_at() == self.context.observation.tick)
                    .then_some(assignment.request_ordinal())
            }));
        }
        if prepared.fresh_lift_producer_jobs > 0 {
            let accepted_at = planner
                .operation()
                .expect("fresh Lift production belongs to an active operation")
                .started_at;
            let deadline = planner
                .operation()
                .expect("fresh Lift production belongs to an active operation")
                .deadline;
            let assignments =
                fresh_lift_producer_assignments(planner, accepted_at, producer_schedule);
            if assignments.len() != prepared.fresh_lift_producer_jobs
                || planner
                    .bind_producer_assignments(accepted_at, deadline, assignments)
                    .is_err()
            {
                retain_first_coordinator_failure(
                    &mut prepared.coordinator_failure,
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    Err(AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected),
                );
                *allocation_ok = false;
                return;
            }
        }
        planner.mark_producers_issued(&due_ordinals);
    }

    fn commit_emergency_defense(
        &mut self,
        prepared: &mut PreparedAllocation,
        effects: &mut CommitEffects,
    ) {
        if let Some(defense) = prepared.emergency_defense.take() {
            self.participants
                .policy
                .commit_adjudicated_emergency_defense(
                    defense,
                    &mut effects.fresh_emergency_defense_intents,
                );
        }
    }

    fn bind_saved_foundry_funding(
        &self,
        prepared: &mut PreparedAllocation,
        settlement: &CrossDomainSettlement,
        allocation_ok: &mut bool,
    ) {
        let Some(saved) = prepared.saved_foundry else {
            return;
        };
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: saved.accepted_at(),
            key: ObligationKey::SavedFoundry {
                anchor: saved.anchor(),
            },
        };
        prepared.saved_foundry = match settlement.capital_assignment(owner) {
            Some(assignment) => {
                saved.with_allocated_funding(assignment.current_scrap, assignment.forecast_scrap)
            }
            None if saved.ready_to_build() => Some(saved),
            None => None,
        };
        if prepared.saved_foundry.is_none() {
            retain_first_coordinator_failure(
                &mut prepared.coordinator_failure,
                AllocationCoordinatorStageTrace::SavedFoundryDispatch,
                Err(AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected),
            );
            *allocation_ok = false;
        }
    }

    fn dispatch_ready_saved_foundry(
        &mut self,
        prepared: &mut PreparedAllocation,
        effects: &mut CommitEffects,
        allocation_ok: &mut bool,
    ) {
        if *allocation_ok
            && let Some(saved) = prepared.saved_foundry
            && saved.ready_to_build()
            && !self
                .participants
                .policy
                .dispatch_validated_foundry(saved, &mut effects.fresh_foundry_intents)
        {
            retain_first_coordinator_failure(
                &mut prepared.coordinator_failure,
                AllocationCoordinatorStageTrace::SavedFoundryDispatch,
                Err(AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected),
            );
            *allocation_ok = false;
        }
    }

    fn refresh_active_connected(
        &mut self,
        prepared: &mut PreparedAllocation,
        producer_schedule: &[super::ScheduledProducerJob],
        allocation_ok: &mut bool,
    ) {
        let Some(active) = prepared.active_connected.as_ref() else {
            return;
        };
        let planner = self
            .participants
            .strategy
            .as_mut()
            .expect("an active connected obligation can only come from its planner");
        let assignments = active_connected_producer_assignments(active, producer_schedule);
        if let Err(error) = planner.refresh_active_connected_funding(active, &assignments) {
            retain_first_coordinator_failure(
                &mut prepared.coordinator_failure,
                AllocationCoordinatorStageTrace::ActiveConnectedRefresh,
                Err(error.into()),
            );
            *allocation_ok = false;
        }
    }

    fn commit_fresh_connected(
        &mut self,
        prepared: &mut PreparedAllocation,
        producer_schedule: &[super::ScheduledProducerJob],
        payloads: &mut super::AcceptedDomainPayloads,
        effects: &mut CommitEffects,
        allocation_ok: &mut bool,
    ) {
        let Some(mut connected) = payloads.take_connected() else {
            return;
        };
        let revises_active = connected.revises_active_operation();
        let assignments = if revises_active {
            active_connected_revision_producer_assignments(&connected, producer_schedule)
        } else {
            connected_producer_assignments(&connected, producer_schedule)
        };
        if let Err(error) = connected.bind_producer_assignments(assignments) {
            retain_first_coordinator_failure(
                &mut prepared.coordinator_failure,
                AllocationCoordinatorStageTrace::ConnectedProposalBinding,
                Err(error.into()),
            );
            *allocation_ok = false;
            return;
        }
        let planner = self
            .participants
            .strategy
            .as_mut()
            .expect("a connected proposal can only come from an enabled planner");
        if let Err(error) = planner.commit_connected_proposal(connected) {
            retain_first_coordinator_failure(
                &mut prepared.coordinator_failure,
                AllocationCoordinatorStageTrace::ConnectedProposalCommit,
                Err(error.into()),
            );
            *allocation_ok = false;
        } else {
            effects.accepted_connected = true;
        }
    }

    fn commit_fresh_foundry(
        &mut self,
        prepared: &mut PreparedAllocation,
        payloads: &mut super::AcceptedDomainPayloads,
        effects: &mut CommitEffects,
        allocation_ok: &mut bool,
    ) {
        if !*allocation_ok {
            return;
        }
        let Some(foundry) = payloads.take_foundry() else {
            return;
        };
        if prepared
            .connected_accepted_at
            .is_some_and(|accepted_at| accepted_at <= self.context.observation.tick)
        {
            effects.budget.prior_operation_spendable = effects
                .budget
                .prior_operation_spendable
                .max(effects.budget.connected_spendable);
        }
        if prepared.active_lift_precedes_foundry {
            effects.budget.prior_operation_spendable = effects
                .budget
                .prior_operation_spendable
                .max(prepared.active_lift_spendable);
        }
        if self
            .participants
            .policy
            .commit_adjudicated_foundry(
                foundry,
                self.context.observation.tick,
                &mut effects.fresh_foundry_intents,
            )
            .is_err()
        {
            retain_first_coordinator_failure(
                &mut prepared.coordinator_failure,
                AllocationCoordinatorStageTrace::FoundryProposalCommit,
                Err(AllocationCoordinatorFailureReasonTrace::ExistingFoundryCommitment),
            );
            *allocation_ok = false;
        }
    }

    /// Applies every exact selected payload, or restores every participant to
    /// the transaction snapshots when any adaptation or dispatch fails.
    fn commit_or_restore(mut self, resolved: ResolvedAllocation) -> AllocationSessionOutcome {
        let ResolvedAllocation {
            mut prepared,
            settlement,
            snapshots,
            mut allocation_ok,
        } = resolved;
        let mut effects = match settlement {
            Some(settlement) => {
                self.commit_settlement(&mut prepared, settlement, &mut allocation_ok)
            }
            None => CommitEffects::frozen(&prepared),
        };
        if let Some((stage, reason)) = prepared.coordinator_failure.clone()
            && let Some(trace) = self.trace.as_deref_mut()
        {
            trace.record_coordinator_failure(stage, reason);
        }

        let mut planner_claims = core::mem::take(&mut prepared.planner_claims);
        let mut strategic_core_exclusions =
            core::mem::take(&mut prepared.strategic_core_exclusions);
        let mut team_decision = core::mem::take(&mut prepared.team_decision);
        let mut lift_decision = core::mem::take(&mut prepared.lift_decision);
        let mut raid_decision = core::mem::take(&mut prepared.raid_decision);
        let mut staged_strategy = prepared.staged_strategy.take();
        if !allocation_ok {
            *self.participants.policy = snapshots.policy;
            self.advanced.snapshots.restore(&mut self.participants);
            team_decision = StrategicDecision::default();
            lift_decision = StrategicDecision::default();
            raid_decision = StrategicDecision::default();
            if staged_strategy.is_some() {
                staged_strategy = Some(StrategicThinkResult::default());
            }
            effects = CommitEffects::frozen(&prepared);

            let restored_claims = PlannerClaims::new(
                self.context.enlisted,
                self.participants.strategy,
                self.participants.raids,
                self.participants.lifts,
            );
            let restored_team_core = self
                .participants
                .team
                .as_ref()
                .map_or_else(Vec::new, TeamReliefPlanner::core_reservations);
            planner_claims = restored_claims.all(&restored_team_core);
            strategic_core_exclusions = restored_claims.core_exclusions(&restored_team_core);
        }

        AllocationSessionOutcome {
            opening_core: prepared.opening_core,
            allow_new_voluntary_operations: prepared.allow_new_voluntary_operations,
            team_decision,
            lift_decision,
            raid_decision,
            planner_claims,
            strategic_core_exclusions,
            connected_continues: prepared.active_connected.is_some(),
            connected_accepted_at: prepared.connected_accepted_at,
            rejected_connected_candidate: prepared.rejected_connected_candidate.take(),
            staged_strategy,
            fresh_emergency_defense_intents: effects.fresh_emergency_defense_intents,
            fresh_foundry_intents: effects.fresh_foundry_intents,
            allocated_producer_intents: effects.allocated_producer_intents,
            allocation_ok,
            accepted_connected: effects.accepted_connected,
            producer_lane_reservations: effects.producer_lane_reservations,
            budget: effects.budget,
        }
    }
}

/// Protects the shallow line-unit fund without making it delay a current counter.
fn standing_force_with_voluntary_guard(
    proposal: DomainInvestmentProposal,
    voluntary_scrap_guard: u32,
) -> DomainInvestmentProposal {
    if matches!(
        proposal.key(),
        ProposalKey::StandingForce(StandingForceKey {
            kind: UnitKind::Sentinel,
            ..
        })
    ) {
        proposal.satisfies_voluntary_scrap_guard_within(SHALLOW_QUEUE_DEPTH)
    } else if proposal.case().urgency == Urgency::Pressing {
        proposal
    } else {
        proposal.with_voluntary_scrap_guard(voluntary_scrap_guard)
    }
}

/// Current support signal one air operation exposes to a matching lift.
pub(crate) fn lift_air_support(
    operation: Option<&AirOperation>,
    terminal: Option<AirOperationOutcome>,
) -> LiftAirSupport {
    let Some(operation) = operation else {
        return match terminal {
            Some(AirOperationOutcome::Released { player, target }) => {
                LiftAirSupport::Released { player, target }
            }
            Some(AirOperationOutcome::Aborted { player, target }) => {
                LiftAirSupport::Aborted { player, target }
            }
            None => LiftAirSupport::Independent,
        };
    };
    if !operation.assault_admitted {
        return LiftAirSupport::Independent;
    }
    let shared = (operation.target_player, operation.target);
    match operation.phase {
        AirOperationPhase::Recon
        | AirOperationPhase::Assemble
        | AirOperationPhase::SuppressAa
        | AirOperationPhase::Verify => LiftAirSupport::Suppressing {
            player: shared.0,
            target: shared.1,
        },
        AirOperationPhase::Strike => LiftAirSupport::Released {
            player: shared.0,
            target: shared.1,
        },
        AirOperationPhase::Recover => {
            if operation.recovery_reason == Some(crate::bot::strategy::AirRecoveryReason::Complete)
            {
                LiftAirSupport::Released {
                    player: shared.0,
                    target: shared.1,
                }
            } else {
                LiftAirSupport::Aborted {
                    player: shared.0,
                    target: shared.1,
                }
            }
        }
    }
}

type CoordinatorFailure = (
    AllocationCoordinatorStageTrace,
    AllocationCoordinatorFailureReasonTrace,
);

struct ClaimSnapshot {
    team_core_claims: Vec<UnitId>,
    planner_claims: Vec<UnitId>,
    strategic_core_exclusions: Vec<UnitId>,
    opening_core: CombatCoreStatus,
}

struct ObligationPreparation {
    resources: ResourceSnapshot,
    obligations: Vec<ImportedObligation>,
    coordinator_failure: Option<CoordinatorFailure>,
    active_connected: Option<ActiveConnectedObligation>,
    active_lift: Option<ActiveLiftProductionObligation>,
    invalid_active_connected: bool,
    invalid_active_lift: bool,
    legacy_air_claims: Option<(Tick, Vec<UnitId>)>,
    staged_strategy: Option<StrategicThinkResult>,
}

struct AirLiftPreparation {
    lift_decision: StrategicDecision,
    opening_bootstrap: u32,
    airworks_capacity: u32,
    active_lift_precedes_foundry: bool,
    active_lift_spendable: u32,
    saved_plan_reserve_already_imported: u32,
    lift_airworks_capacity: u32,
    lift_deadline: Tick,
    fresh_lift_producer_jobs: usize,
    voluntary_scrap_guard: u32,
}

struct SavedFoundryPreparation {
    obligation: Option<ValidatedFoundryObligation>,
    saving: u32,
    blocked: bool,
    preparation_need: Option<(TilePos, u64)>,
}

#[derive(Default)]
struct ActiveRevisionPreparation {
    proposal: Option<FreshConnectedProposal>,
    rejected: Option<RejectedConnectedCandidate>,
}

struct FreshInvestmentPreparation {
    foundry: Option<FreshFoundryProposal>,
    connected: Option<FreshConnectedProposal>,
    standing_force: StandingForcePreparation,
    standing_force_derivation: StandingForceDerivation,
    connected_accepted_at: Option<Tick>,
    connected_reserve_deadline: Tick,
    rejected_connected_candidate: Option<RejectedConnectedCandidate>,
}

struct StandingForceDerivation {
    projection_targets: Vec<StandingGroundTarget>,
    residual_investment_reserve: u32,
    expansion_security_need: Option<(TilePos, u64)>,
}

enum StandingForcePreparation {
    Unconditional(Vec<StandingForceProposal>),
    ConnectedContexts(Vec<ContextualStandingForce>),
}

impl Default for StandingForcePreparation {
    fn default() -> Self {
        Self::Unconditional(Vec::new())
    }
}

impl StandingForcePreparation {
    fn for_each(&self, mut visit: impl FnMut(&StandingForceProposal)) {
        match self {
            Self::Unconditional(proposals) => proposals.iter().for_each(&mut visit),
            Self::ConnectedContexts(contexts) => contexts
                .iter()
                .flat_map(|context| &context.proposals)
                .for_each(visit),
        }
    }
}

struct ContextualStandingForce {
    context: ConnectedPortfolioContext,
    proposals: Vec<StandingForceProposal>,
}

struct CommitEffects {
    accepted_connected: bool,
    producer_lane_reservations: ProducerLaneReservations,
    fresh_emergency_defense_intents: Vec<Intent>,
    fresh_foundry_intents: Vec<Intent>,
    allocated_producer_intents: Vec<Intent>,
    budget: AllocationBudgetOutcome,
}

impl CommitEffects {
    fn frozen(prepared: &PreparedAllocation) -> Self {
        Self {
            accepted_connected: false,
            producer_lane_reservations: ProducerLaneReservations::default(),
            fresh_emergency_defense_intents: Vec::new(),
            fresh_foundry_intents: Vec::new(),
            allocated_producer_intents: Vec::new(),
            budget: AllocationBudgetOutcome::frozen(
                prepared.foundry_saving,
                prepared.airworks_capacity,
                prepared.opening_bootstrap,
                prepared.voluntary_scrap_guard,
            ),
        }
    }
}

struct PreparedAllocation {
    resources: ResourceSnapshot,
    obligations: Vec<ImportedObligation>,
    coordinator_failure: Option<CoordinatorFailure>,
    opening_core: CombatCoreStatus,
    allow_new_voluntary_operations: bool,
    planner_claims: Vec<UnitId>,
    strategic_core_exclusions: Vec<UnitId>,
    active_connected: Option<ActiveConnectedObligation>,
    active_lift: Option<ActiveLiftProductionObligation>,
    fresh_lift_producer_jobs: usize,
    saved_foundry: Option<ValidatedFoundryObligation>,
    fresh_foundry: Option<FreshFoundryProposal>,
    fresh_connected: Option<FreshConnectedProposal>,
    standing_force: StandingForcePreparation,
    connected_accepted_at: Option<Tick>,
    connected_reserve_deadline: Tick,
    allocation_horizon: Tick,
    active_lift_precedes_foundry: bool,
    active_lift_spendable: u32,
    foundry_saving: u32,
    airworks_capacity: u32,
    opening_bootstrap: u32,
    voluntary_scrap_guard: u32,
    rejected_connected_candidate: Option<RejectedConnectedCandidate>,
    staged_strategy: Option<StrategicThinkResult>,
    emergency_defense: Option<FreshEmergencyDefense>,
    team_decision: StrategicDecision,
    lift_decision: StrategicDecision,
    raid_decision: StrategicDecision,
}

struct CommitSnapshots {
    policy: UtilityPolicy,
}

struct ResolvedAllocation {
    prepared: PreparedAllocation,
    settlement: Option<CrossDomainSettlement>,
    snapshots: CommitSnapshots,
    allocation_ok: bool,
}

fn push_obligation(
    obligations: &mut Vec<ImportedObligation>,
    obligation: Result<ImportedObligation, ClaimBundleError>,
) -> Result<(), AllocationCoordinatorFailureReasonTrace> {
    match obligation {
        Ok(obligation) => {
            obligations.push(obligation);
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn retain_first_coordinator_failure(
    failure: &mut Option<CoordinatorFailure>,
    stage: AllocationCoordinatorStageTrace,
    result: Result<(), AllocationCoordinatorFailureReasonTrace>,
) {
    if failure.is_none()
        && let Err(reason) = result
    {
        *failure = Some((stage, reason));
    }
}

fn push_clamped_current_reserve(
    obligations: &mut Vec<ImportedObligation>,
    bank: u32,
    accepted_at: Tick,
    decision_tick: Tick,
    key: ObligationKey,
    desired: u32,
) -> Result<(), AllocationCoordinatorFailureReasonTrace> {
    match clamped_current_reserve_obligation(
        obligations,
        bank,
        accepted_at,
        decision_tick,
        key,
        desired,
    ) {
        Ok(Some(obligation)) => {
            obligations.push(obligation);
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn push_bounded_capital_reserve(
    obligations: &mut Vec<ImportedObligation>,
    resources: &ResourceSnapshot,
    reserve: BoundedCapitalReserve,
) -> Result<(), AllocationCoordinatorFailureReasonTrace> {
    let BoundedCapitalReserve {
        cadence,
        bank,
        accepted_at,
        decision_tick,
        key,
        desired,
        forecast_deadline,
        older_capital_reserve,
    } = reserve;
    if desired == 0 {
        return Ok(());
    }
    let available_current = bank.saturating_sub(current_reserve_at(obligations, decision_tick));
    let projection = resources
        .planning_projection(forecast_deadline, cadence)
        .map_err(|error| {
            AllocationCoordinatorFailureReasonTrace::from(&CoordinatorInputError::Projection(error))
        })?;
    let fixed_forecast = forecast_reserve_through(obligations, forecast_deadline);
    let future_production = obligations
        .iter()
        .flat_map(|obligation| obligation.claims.producer_jobs())
        .filter(|job| {
            job.fixed_timing().is_some_and(|(_, enqueued_at, _, _)| {
                enqueued_at > decision_tick && enqueued_at <= forecast_deadline
            })
        })
        .map(|job| job.kind().stats().cost)
        .fold(0, u32::saturating_add);
    let flexible_capital = obligations
        .iter()
        .filter_map(|obligation| obligation.claims.deferrable_capital())
        .filter(|claim| claim.through <= forecast_deadline)
        .map(|claim| claim.amount)
        .fold(0, u32::saturating_add);
    // Future producer jobs do not enter `current_reserve_at` until their
    // enqueue tick. Give every prior flexible claim first use of the observed
    // bank so this fixed legacy prefix cannot make mandatory work infeasible.
    let flexible_prior_capital = future_production
        .saturating_add(flexible_capital)
        .saturating_add(older_capital_reserve);
    let prior_current = flexible_prior_capital.min(available_current);
    let prior_forecast = flexible_prior_capital.saturating_sub(prior_current);
    let available_current = available_current.saturating_sub(prior_current);
    let available_forecast = u32::try_from(projection.forecast_through(forecast_deadline))
        .unwrap_or(u32::MAX)
        .saturating_sub(fixed_forecast)
        .saturating_sub(prior_forecast);
    let current = desired.min(available_current);
    let forecast = desired.saturating_sub(current).min(available_forecast);
    if current == 0 && forecast == 0 {
        return Ok(());
    }
    let claims = ClaimBundle::new(
        current,
        (forecast > 0)
            .then_some(ForecastClaim {
                through: forecast_deadline,
                amount: forecast,
            })
            .into_iter()
            .collect(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )
    .map_err(AllocationCoordinatorFailureReasonTrace::from)?;
    obligations.push(imported_obligation(
        ObligationClass::Legacy,
        accepted_at,
        key,
        claims,
    ));
    Ok(())
}

fn push_legacy_planner_claim(
    obligations: &mut Vec<ImportedObligation>,
    resources: &ResourceSnapshot,
    claim: LegacyPlannerClaim<'_>,
) -> Result<(), AllocationCoordinatorFailureReasonTrace> {
    let LegacyPlannerClaim {
        cadence,
        accepted_at,
        decision_at,
        retained_at,
        channel,
        decision,
        protect_unspent_current_scrap,
        prior_producer_intents,
        mut retained_units,
        production_deadline,
    } = claim;
    retain_observed_units(resources, &mut retained_units);
    let retained_valid = if retained_units.is_empty() {
        Ok(())
    } else {
        push_obligation(
            obligations,
            legacy_unit_obligation(retained_at, channel, 0, retained_units),
        )
    };
    let production_cost = decision
        .intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::TrainAt { kind, .. } => Some(kind.stats().cost),
            _ => None,
        })
        .fold(0, u32::saturating_add);
    let exact_production_decision;
    let decision = if protect_unspent_current_scrap {
        decision
    } else {
        exact_production_decision = StrategicDecision {
            committed_scrap: production_cost,
            ..decision.clone()
        };
        &exact_production_decision
    };
    if decision.committed_scrap == 0
        && !decision
            .intents
            .iter()
            .any(|intent| matches!(intent, Intent::TrainAt { .. }))
    {
        return retained_valid;
    }
    let immediate = legacy_decision_obligation(
        resources,
        LegacyDecisionRequest {
            cadence,
            accepted_at,
            decision_tick: decision_at,
            channel,
            sequence: 1,
            decision,
            prior_producer_intents,
            production_deadline,
        },
    );
    retained_valid?;
    push_coordinator_obligation(obligations, immediate)
}

fn retain_observed_units(resources: &ResourceSnapshot, units: &mut Vec<UnitId>) {
    units.retain(|id| {
        resources
            .units()
            .binary_search_by_key(id, |unit| unit.id)
            .is_ok()
    });
}

fn remove_active_connected_obligation(obligations: &mut Vec<ImportedObligation>) {
    obligations
        .retain(|obligation| !matches!(obligation.key, ObligationKey::ConnectedOffense { .. }));
}

fn active_air_units(planner: Option<&StrategicPlanner>, observation: &Observation) -> Vec<UnitId> {
    let mut units = planner
        .and_then(StrategicPlanner::air_operation)
        .map_or_else(Vec::new, |operation| {
            prior_planner_claims(&[], Some(operation), &[], &[], None)
        });
    let resources = ResourceSnapshot::from_observation(observation);
    retain_observed_units(&resources, &mut units);
    units
}

const fn connected_production_conflict(conflict: &AllocationConflict) -> bool {
    matches!(
        conflict,
        AllocationConflict::UnknownProducer(_)
            | AllocationConflict::ProducerAccess { .. }
            | AllocationConflict::ProducerSchedule { .. }
            | AllocationConflict::ProductionFunding { .. }
    )
}

fn project_producer_intents(obs: &Observation, intents: &[Intent]) -> Observation {
    let mut projected = obs.clone();
    for intent in intents {
        let Intent::TrainAt { building, kind } = intent else {
            continue;
        };
        if let Some(index) = projected
            .my_buildings
            .iter()
            .position(|candidate| candidate.id == *building)
            && let Some(queue) = projected.my_queues.get_mut(index)
        {
            queue.push(*kind);
        }
    }
    projected
}

fn push_coordinator_obligation(
    obligations: &mut Vec<ImportedObligation>,
    obligation: Result<ImportedObligation, CoordinatorInputError>,
) -> Result<(), AllocationCoordinatorFailureReasonTrace> {
    match obligation {
        Ok(obligation) => {
            obligations.push(obligation);
            Ok(())
        }
        Err(error) => Err((&error).into()),
    }
}

fn observable_lift_operation_reservations(
    operation: &LiftOperation,
    observation: &Observation,
) -> Vec<UnitId> {
    let mut observable = observation
        .my_units
        .iter()
        .map(|unit| unit.id)
        .collect::<Vec<_>>();
    observable.sort_unstable();
    observable.dedup();
    prior_planner_claims(&[], None, &[], &[], Some(operation))
        .into_iter()
        .filter(|unit| observable.binary_search(unit).is_ok())
        .collect()
}

#[derive(Clone, Copy)]
struct ActiveLiftFutureProductionContext<'a> {
    resources: &'a ResourceSnapshot,
    observation: &'a Observation,
    operation: &'a LiftOperation,
    unavailable: &'a [UnitId],
    prior_producer_intents: &'a [Intent],
    lift_decision: &'a StrategicDecision,
    cadence: Tick,
    accepted_at: Tick,
}

#[derive(Clone, Copy)]
struct ActiveLiftCurrentProductionContext<'a> {
    resources: &'a ResourceSnapshot,
    cadence: Tick,
    decision_tick: Tick,
    retained_at: Tick,
    decision: &'a StrategicDecision,
    prior_producer_intents: &'a [Intent],
    production_deadline: Tick,
}

fn feasible_active_lift_current_production_prefix(
    context: ActiveLiftCurrentProductionContext<'_>,
    prior_obligations: &[ImportedObligation],
) -> Result<StrategicDecision, AllocationCoordinatorFailureReasonTrace> {
    let ActiveLiftCurrentProductionContext {
        resources,
        cadence,
        decision_tick,
        retained_at,
        decision,
        prior_producer_intents,
        production_deadline,
    } = context;
    let requested = decision
        .intents
        .iter()
        .filter(|intent| matches!(intent, Intent::TrainAt { .. }))
        .count();
    if requested == 0
        || !obligations_resolve(resources, prior_obligations, production_deadline, cadence)?
    {
        return Ok(decision.clone());
    }

    let mut feasible = 0_usize;
    let mut infeasible = requested.saturating_add(1);
    while feasible.saturating_add(1) < infeasible {
        let candidate_count = feasible.saturating_add(infeasible).div_ceil(2);
        let candidate = strategic_decision_with_production_prefix(decision, candidate_count);
        let mut obligations = prior_obligations.to_vec();
        let imported = push_legacy_planner_claim(
            &mut obligations,
            resources,
            LegacyPlannerClaim {
                cadence,
                accepted_at: decision_tick,
                decision_at: decision_tick,
                retained_at,
                channel: LegacyChannel::Lift,
                decision: &candidate,
                protect_unspent_current_scrap: false,
                prior_producer_intents,
                retained_units: Vec::new(),
                production_deadline,
            },
        );
        if imported.is_ok()
            && obligations_resolve(resources, &obligations, production_deadline, cadence)?
        {
            feasible = candidate_count;
        } else {
            infeasible = candidate_count;
        }
    }
    Ok(strategic_decision_with_production_prefix(
        decision, feasible,
    ))
}

fn obligations_resolve(
    resources: &ResourceSnapshot,
    obligations: &[ImportedObligation],
    horizon: Tick,
    cadence: Tick,
) -> Result<bool, AllocationCoordinatorFailureReasonTrace> {
    let horizon = obligation_horizon(obligations, horizon);
    let mut allocation = CrossDomainAllocation::new(resources, horizon, cadence)
        .map_err(|error| AllocationCoordinatorFailureReasonTrace::from(&error))?;
    for obligation in obligations.iter().cloned() {
        allocation.import(obligation);
    }
    Ok(allocation
        .resolve(AllocationPersonality::default(), None)
        .is_ok())
}

fn older_saved_foundry_deferrable_capital(
    obligations: &[ImportedObligation],
    lift_precedes_foundry: bool,
) -> u32 {
    if lift_precedes_foundry {
        return 0;
    }
    obligations
        .iter()
        .filter(|obligation| matches!(obligation.key, ObligationKey::SavedFoundry { .. }))
        .filter_map(|obligation| obligation.claims.deferrable_capital())
        .map(|capital| capital.amount)
        .fold(0, u32::saturating_add)
}

fn obligation_horizon(obligations: &[ImportedObligation], minimum: Tick) -> Tick {
    obligations.iter().fold(minimum, |horizon, obligation| {
        let horizon = obligation
            .claims
            .forecast_scrap()
            .iter()
            .fold(horizon, |horizon, claim| horizon.max(claim.through));
        let horizon = obligation
            .claims
            .deferrable_capital()
            .map_or(horizon, |claim| horizon.max(claim.through));
        obligation
            .claims
            .producer_jobs()
            .iter()
            .fold(horizon, |horizon, job| horizon.max(job.ready_before()))
    })
}

fn strategic_decision_with_production_prefix(
    decision: &StrategicDecision,
    production_limit: usize,
) -> StrategicDecision {
    let mut result = decision.clone();
    let mut retained = 0_usize;
    let mut removed_cost = 0_u32;
    result.intents.retain(|intent| {
        let Intent::TrainAt { kind, .. } = intent else {
            return true;
        };
        if retained < production_limit {
            retained = retained.saturating_add(1);
            true
        } else {
            removed_cost = removed_cost.saturating_add(kind.stats().cost);
            false
        }
    });
    result.committed_scrap = result.committed_scrap.saturating_sub(removed_cost);
    result
}

fn active_lift_future_production_obligation(
    context: ActiveLiftFutureProductionContext<'_>,
) -> Result<Option<ImportedObligation>, CoordinatorInputError> {
    active_lift_future_production_obligation_with_limit(context, usize::MAX)
}

fn active_lift_future_production_obligation_with_limit(
    context: ActiveLiftFutureProductionContext<'_>,
    job_limit: usize,
) -> Result<Option<ImportedObligation>, CoordinatorInputError> {
    let ActiveLiftFutureProductionContext {
        resources,
        observation,
        operation,
        unavailable,
        prior_producer_intents,
        lift_decision,
        cadence,
        accepted_at,
    } = context;
    if operation.phase != crate::bot::lift::LiftPhase::Provision {
        return Ok(None);
    }
    let enqueue_not_before = observation.tick.saturating_add(cadence);
    if enqueue_not_before >= operation.deadline {
        return Ok(None);
    }
    let live = observation
        .my_units
        .iter()
        .filter(|unit| {
            unit.kind == UnitKind::Skyhook
                && unit.cargo == 0
                && unavailable.binary_search(&unit.id).is_err()
        })
        .count();
    let queued = resources
        .producers()
        .iter()
        .map(|producer| producer.queued_kind_ready_before(UnitKind::Skyhook, operation.deadline))
        .sum::<usize>();
    let mut producers = resources
        .planning_projection(operation.deadline, cadence)?
        .producers()
        .to_vec();
    let mut same_think = 0_usize;
    for intent in prior_producer_intents.iter().chain(&lift_decision.intents) {
        let Intent::TrainAt { building, kind } = intent else {
            continue;
        };
        let Some(index) = producers
            .binary_search_by_key(
                building,
                crate::bot::resources::ProducerPlanningProjection::producer,
            )
            .ok()
        else {
            return Err(CoordinatorInputError::ImmediateProducerUnavailable {
                producer: *building,
                kind: *kind,
            });
        };
        let Some(projected) = producers[index].append(*kind, observation.tick) else {
            return Err(CoordinatorInputError::ImmediateProducerUnavailable {
                producer: *building,
                kind: *kind,
            });
        };
        if *kind == UnitKind::Skyhook && projected.ready_at < operation.deadline {
            same_think = same_think.saturating_add(1);
        }
    }
    let remaining = operation
        .desired_carriers
        .saturating_sub(live.saturating_add(queued).saturating_add(same_think))
        .min(job_limit);
    if remaining == 0 {
        return Ok(None);
    }
    let one_skyhook = [UnitKind::Skyhook];
    let eligible_producers = resources
        .producers()
        .iter()
        .filter(|producer| {
            producer
                .horizon_timing(&one_skyhook)
                .is_some_and(|timing| timing.no_block_latest_ready_tick < operation.deadline)
        })
        .map(|producer| producer.producer)
        .collect::<Vec<_>>();
    if eligible_producers.is_empty() {
        return Ok(None);
    }
    let jobs = core::iter::repeat_with(|| {
        ProducerJobClaim::flexible(
            UnitKind::Skyhook,
            enqueue_not_before,
            operation.deadline,
            eligible_producers.clone(),
        )
    })
    .take(remaining)
    .collect();
    let claims = ClaimBundle::new(0, Vec::new(), Vec::new(), Vec::new(), Vec::new(), jobs)?;
    Ok(Some(imported_obligation(
        ObligationClass::PersistentPlan,
        accepted_at,
        ObligationKey::Legacy {
            channel: LegacyChannel::Lift,
            sequence: 2,
        },
        claims,
    )))
}

fn feasible_active_lift_future_production_obligation(
    context: ActiveLiftFutureProductionContext<'_>,
    prior_obligations: &[ImportedObligation],
) -> Result<Option<ImportedObligation>, CoordinatorInputError> {
    let Some(full) = active_lift_future_production_obligation(context)? else {
        return Ok(None);
    };
    let requested = full.claims.producer_jobs().len();
    let mut feasible = 0_usize;
    let mut infeasible = requested.saturating_add(1);
    while feasible.saturating_add(1) < infeasible {
        let candidate_count = feasible.saturating_add(infeasible).div_ceil(2);
        let candidate =
            active_lift_future_production_obligation_with_limit(context, candidate_count)?
                .expect("a positive prefix of a nonempty Lift demand remains nonempty");
        let mut obligations = prior_obligations.to_vec();
        obligations.push(candidate);
        let horizon = obligation_horizon(&obligations, context.operation.deadline);
        let mut allocation =
            CrossDomainAllocation::new(context.resources, horizon, context.cadence)?;
        for obligation in obligations {
            allocation.import(obligation);
        }
        if allocation
            .resolve(AllocationPersonality::default(), None)
            .is_ok()
        {
            feasible = candidate_count;
        } else {
            infeasible = candidate_count;
        }
    }
    if feasible == 0 {
        return Ok(None);
    }
    active_lift_future_production_obligation_with_limit(context, feasible)
}

#[cfg(test)]
fn active_connected_production_context(
    resources: &ResourceSnapshot,
    active: &ActiveConnectedObligation,
    cadence: Tick,
    observed_at: Tick,
) -> Option<(ProducerLaneReservations, Vec<Intent>)> {
    if !active.producer_schedule_is_executable(resources, cadence, observed_at) {
        return None;
    }
    let due: Vec<_> = active
        .provider_jobs()
        .iter()
        .filter(|assignment| assignment.timing().enqueued_at() == observed_at)
        .map(|assignment| Intent::TrainAt {
            building: assignment.producer(),
            kind: assignment.kind(),
        })
        .collect();
    let projection = resources
        .planning_projection(active.deadline(), cadence)
        .ok()?;
    let lanes = ProducerLaneReservations::from_jobs(
        &projection,
        active.provider_jobs().iter().map(|assignment| {
            let timing = assignment.timing();
            ReservedProducerJob {
                producer: assignment.producer(),
                kind: assignment.kind(),
                enqueued_at: timing.enqueued_at(),
                starts_at: timing.starts_at(),
                ready_at: timing.ready_at(),
                ready_before: timing.ready_before(),
            }
        }),
    )
    .ok()?;
    Some((lanes, due))
}

fn active_lift_production_obligation(
    obligation: &ActiveLiftProductionObligation,
) -> Result<ImportedObligation, ClaimBundleError> {
    Ok(imported_obligation(
        ObligationClass::PersistentPlan,
        obligation.accepted_at(),
        ObligationKey::Legacy {
            channel: LegacyChannel::Lift,
            sequence: 2,
        },
        ClaimBundle::new(
            0,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            obligation
                .producer_jobs()
                .iter()
                .copied()
                .map(|assignment| {
                    ProducerJobClaim::fixed(
                        assignment.producer(),
                        assignment.kind(),
                        assignment.timing().enqueued_at(),
                        assignment.timing().starts_at(),
                        assignment.timing().ready_at(),
                        assignment.timing().ready_before(),
                    )
                })
                .collect(),
        )?,
    ))
}

fn lift_preceding_production_context(
    resources: &ResourceSnapshot,
    lift: Option<&ActiveLiftProductionObligation>,
    connected: Option<&ActiveConnectedObligation>,
    cadence: Tick,
    observed_at: Tick,
) -> Option<(ProducerLaneReservations, Vec<Intent>)> {
    let mut jobs = Vec::new();
    let mut funding = Vec::new();
    if let Some(lift) = lift {
        funding.extend(lift.producer_jobs().iter().map(|assignment| {
            let split = assignment.funding();
            RetainedProducerFunding {
                through: assignment.timing().enqueued_at(),
                current_scrap: split.current_scrap(),
                forecast_scrap: split.forecast_scrap(),
            }
        }));
        jobs.extend(
            lift.producer_jobs()
                .iter()
                .map(|assignment| ReservedProducerJob {
                    producer: assignment.producer(),
                    kind: assignment.kind(),
                    enqueued_at: assignment.timing().enqueued_at(),
                    starts_at: assignment.timing().starts_at(),
                    ready_at: assignment.timing().ready_at(),
                    ready_before: assignment.timing().ready_before(),
                }),
        );
    }
    if let Some(connected) = connected {
        funding.extend(connected.provider_jobs().iter().map(|assignment| {
            let split = assignment.funding();
            RetainedProducerFunding {
                through: assignment.timing().enqueued_at(),
                current_scrap: split.current_scrap(),
                forecast_scrap: split.forecast_scrap(),
            }
        }));
        jobs.extend(
            connected
                .provider_jobs()
                .iter()
                .map(|assignment| ReservedProducerJob {
                    producer: assignment.producer(),
                    kind: assignment.kind(),
                    enqueued_at: assignment.timing().enqueued_at(),
                    starts_at: assignment.timing().starts_at(),
                    ready_at: assignment.timing().ready_at(),
                    ready_before: assignment.timing().ready_before(),
                }),
        );
    }
    jobs.sort_unstable_by_key(|job| {
        (
            job.producer,
            job.starts_at,
            job.ready_at,
            job.enqueued_at,
            job.kind,
        )
    });
    if !retained_producer_funding_is_backed(resources, &funding) {
        return None;
    }
    let projection = resources
        .planning_projection(
            jobs.iter()
                .map(|job| job.ready_before)
                .max()
                .unwrap_or(observed_at.saturating_add(cadence)),
            cadence,
        )
        .ok()?;
    let lanes = ProducerLaneReservations::from_jobs(&projection, jobs.iter().copied()).ok()?;
    let due = jobs
        .into_iter()
        .filter(|job| job.enqueued_at == observed_at)
        .map(|job| Intent::TrainAt {
            building: job.producer,
            kind: job.kind,
        })
        .collect();
    Some((lanes, due))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RetainedProducerFunding {
    through: Tick,
    current_scrap: u32,
    forecast_scrap: u32,
}

fn retained_producer_funding_is_backed(
    resources: &ResourceSnapshot,
    assignments: &[RetainedProducerFunding],
) -> bool {
    let mut scheduled = assignments.to_vec();
    scheduled.sort_unstable_by_key(|assignment| assignment.through);
    let mut required = 0_u64;
    let mut index = 0;
    while index < scheduled.len() {
        let through = scheduled[index].through;
        while index < scheduled.len() && scheduled[index].through == through {
            required = required
                .saturating_add(u64::from(scheduled[index].current_scrap))
                .saturating_add(u64::from(scheduled[index].forecast_scrap));
            index += 1;
        }
        let available = u64::from(resources.current_scrap().amount()).saturating_add(u64::from(
            resources.forecast().income_through(through).amount(),
        ));
        if required > available {
            return false;
        }
    }
    true
}

fn fresh_lift_producer_assignments(
    planner: &LiftPlanner,
    accepted_at: Tick,
    schedule: &[super::ScheduledProducerJob],
) -> Vec<LiftProducerAssignment> {
    let owner = ClaimOwner::Obligation {
        class: ObligationClass::PersistentPlan,
        accepted_at,
        key: ObligationKey::Legacy {
            channel: LegacyChannel::Lift,
            sequence: 2,
        },
    };
    let first_ordinal = planner.next_producer_request_ordinal().unwrap_or(0);
    let mut jobs = schedule
        .iter()
        .filter(|job| job.owner == owner)
        .collect::<Vec<_>>();
    jobs.sort_unstable_by_key(|job| job.request_ordinal);
    jobs.into_iter()
        .enumerate()
        .map(|(offset, job)| lift_producer_assignment(first_ordinal.saturating_add(offset), job))
        .collect()
}

fn active_lift_producer_assignments(
    obligation: &ActiveLiftProductionObligation,
    schedule: &[super::ScheduledProducerJob],
) -> Vec<LiftProducerAssignment> {
    let owner = ClaimOwner::Obligation {
        class: ObligationClass::PersistentPlan,
        accepted_at: obligation.accepted_at(),
        key: ObligationKey::Legacy {
            channel: LegacyChannel::Lift,
            sequence: 2,
        },
    };
    let mut jobs = schedule
        .iter()
        .filter(|job| job.owner == owner)
        .collect::<Vec<_>>();
    jobs.sort_unstable_by_key(|job| job.request_ordinal);
    jobs.into_iter()
        .zip(obligation.producer_jobs())
        .map(|(job, retained)| lift_producer_assignment(retained.request_ordinal(), job))
        .collect()
}

fn lift_producer_assignment(
    request_ordinal: usize,
    job: &super::ScheduledProducerJob,
) -> LiftProducerAssignment {
    LiftProducerAssignment::new(
        request_ordinal,
        job.producer,
        job.kind,
        LiftProducerTiming::new(
            job.enqueued_at,
            job.starts_at,
            job.ready_at,
            job.ready_before,
        ),
        LiftProducerFunding::new(job.current_scrap, job.forecast_scrap),
    )
}

fn retained_producer_context(
    resources: &ResourceSnapshot,
    obligations: &[ImportedObligation],
    cadence: Tick,
    observed_at: Tick,
) -> Option<(ProducerLaneReservations, Vec<Intent>)> {
    let mut horizon = observed_at.saturating_add(cadence);
    for obligation in obligations {
        for job in obligation.claims.producer_jobs() {
            horizon = horizon.max(job.ready_before());
        }
        for claim in obligation.claims.forecast_scrap() {
            horizon = horizon.max(claim.through);
        }
        if let Some(claim) = obligation.claims.deferrable_capital() {
            horizon = horizon.max(claim.through);
        }
    }
    let mut allocation = CrossDomainAllocation::new(resources, horizon, cadence).ok()?;
    for obligation in obligations.iter().cloned() {
        allocation.import(obligation);
    }
    let settlement = allocation
        .resolve(AllocationPersonality::default(), None)
        .ok()?;
    let due = settlement
        .producer_schedule()
        .iter()
        .filter(|job| job.enqueued_at == observed_at)
        .map(|job| Intent::TrainAt {
            building: job.producer,
            kind: job.kind,
        })
        .collect();
    Some((settlement.producer_lane_reservations().clone(), due))
}

fn available_allocation_builders(
    resources: &ResourceSnapshot,
    obligations: &[ImportedObligation],
) -> Vec<UnitId> {
    let mut claimed = obligations
        .iter()
        .flat_map(|obligation| {
            obligation
                .claims
                .builders()
                .iter()
                .chain(obligation.claims.units())
                .copied()
        })
        .collect::<Vec<_>>();
    claimed.sort_unstable();
    claimed.dedup();
    resources
        .builders()
        .iter()
        .filter(|builder| {
            builder.obligation.is_none() && claimed.binary_search(&builder.id).is_err()
        })
        .map(|builder| builder.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::{
        Confidence, DeferrableCapitalClaim, ExecutionSafety, ProposalCase, StrategicValue,
        TimeToImpact, Urgency,
    };
    use super::*;
    use crate::bot::briefing::PublicMapBriefing;
    use crate::bot::lift::LiftPhase;
    use crate::bot::observation::{BuildingObs, UnitObs};
    use crate::bot::profile::Specialty;
    use crate::bot::standing_force::{StandingForceFixture, StandingForceReason};
    use crate::bot::strategy::{
        AirRecoveryReason, ConnectedConfidence, ConnectedExecutionSafety, ConnectedOffenseClaims,
        ConnectedOpportunityCase, ConnectedProducerAssignment, ConnectedProducerFunding,
        ConnectedProducerTiming, ConnectedProviderJob, ConnectedStrategicValue,
        ConnectedTimeToImpact, ConnectedUrgency, FreshConnectedProposalFixture,
    };
    use crate::bot::utility::{
        FoundryConfidence, FoundryExecutionSafety, FoundryOpportunityCase, FoundryStrategicValue,
        FoundryTimeToImpact, FoundryUrgency,
    };
    use crate::ids::{BuildingId, PlayerId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
    use crate::stats::{BuildingKind, UnitKind};

    fn observation() -> Observation {
        Observation {
            map_width: 20,
            map_height: 20,
            visible: vec![true; 400],
            explored: vec![true; 400],
            scrap: 40,
            ..Observation::default()
        }
    }

    fn briefing() -> PublicMapBriefing {
        PublicMapBriefing {
            map_width: 20,
            map_height: 20,
            starting_foundries: Vec::new(),
            teams: vec![None],
            non_ground_terrain: Vec::new(),
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        }
    }

    fn owned_unit(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind,
            tile,
            hp: kind.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn observed_building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(player),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn active_lift_fixture() -> (Observation, LiftPlanner, Tick) {
        const HOME: TilePos = TilePos::new(5, 15);
        let mut observation = Observation {
            tick: 0,
            map_width: 64,
            map_height: 32,
            scrap: 10_000,
            enemy_buildings: vec![observed_building(
                500,
                1,
                BuildingKind::Foundry,
                TilePos::new(50, 15),
            )],
            visible: vec![true; 64 * 32],
            explored: vec![true; 64 * 32],
            known_rock: (0..32).map(|y| TilePos::new(32, y)).collect(),
            ..Observation::default()
        };
        observation.my_buildings.extend([
            observed_building(1, 0, BuildingKind::Foundry, HOME.offset(-1, -1)),
            observed_building(2, 0, BuildingKind::Airworks, HOME.offset(4, -4)),
            observed_building(3, 0, BuildingKind::Fabricator, HOME.offset(4, 2)),
        ]);
        observation.enemy_buildings.push(observed_building(
            501,
            1,
            BuildingKind::Foundry,
            TilePos::new(22, 15),
        ));
        observation.my_queues = vec![Vec::new(), Vec::new(), Vec::new()];
        observation.my_units.extend((1..=30).map(|id| {
            owned_unit(
                id,
                UnitKind::Sentinel,
                TilePos::new(8 + (id % 12) as i32, 8 + ((id / 12) % 12) as i32),
            )
        }));
        let mut lift = LiftPlanner::new();
        lift.think_with_admission(
            &observation,
            HOME,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: true,
                spendable_scrap: observation.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 5,
            },
        );
        let remaining = lift.remaining_airwork_ticks(&observation, &[]);
        assert!(
            remaining > 0,
            "the fixture must retain lift-owned Airworks work"
        );
        (observation, lift, remaining)
    }

    fn allocation_run_for(
        observation: &Observation,
        mut strategy: Option<StrategicPlanner>,
        mut lifts: Option<LiftPlanner>,
    ) -> (AllocationTrace, AllocationSessionOutcome) {
        const HOME: TilePos = TilePos::new(5, 15);
        let public_map = PublicMapBriefing {
            map_width: observation.map_width,
            map_height: observation.map_height,
            starting_foundries: Vec::new(),
            teams: vec![None, None],
            non_ground_terrain: Vec::new(),
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        };
        let profile =
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7).resolve_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(observation);
        let mut policy = UtilityPolicy::new();
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut advanced = advanced(snapshots);
        if let Some(operation) = lifts.as_ref().and_then(LiftPlanner::operation) {
            advanced.lift_was_active = true;
            advanced.lift_started_at = operation.started_at;
        }
        let mut trace = AllocationTrace::default();
        let outcome = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation,
                home: HOME,
                public_map: &public_map,
                orientation: Orientation::for_home(observation, HOME),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced,
            Some(&mut trace),
        )
        .run();
        assert!(
            outcome.allocation_ok,
            "the trace fixture must allocate cleanly: {trace:#?}"
        );
        (trace, outcome)
    }

    fn prepared(
        observation: &Observation,
        coordinator_failure: Option<CoordinatorFailure>,
    ) -> PreparedAllocation {
        PreparedAllocation {
            resources: ResourceSnapshot::from_observation(observation),
            obligations: Vec::new(),
            coordinator_failure,
            opening_core: CombatCoreStatus {
                projected_strength: 10,
                target_strength: 10,
                missing_strength: 0,
                missing_scrap: 0,
                ready: true,
            },
            allow_new_voluntary_operations: true,
            planner_claims: vec![UnitId(99)],
            strategic_core_exclusions: vec![UnitId(98)],
            active_connected: None,
            active_lift: None,
            fresh_lift_producer_jobs: 0,
            saved_foundry: None,
            fresh_foundry: None,
            fresh_connected: None,
            standing_force: StandingForcePreparation::default(),
            connected_accepted_at: None,
            connected_reserve_deadline: 120,
            allocation_horizon: 120,
            active_lift_precedes_foundry: false,
            active_lift_spendable: 0,
            foundry_saving: 11,
            airworks_capacity: 12,
            opening_bootstrap: 13,
            voluntary_scrap_guard: 0,
            rejected_connected_candidate: None,
            staged_strategy: None,
            emergency_defense: None,
            team_decision: StrategicDecision {
                committed_scrap: 1,
                ..StrategicDecision::default()
            },
            lift_decision: StrategicDecision {
                committed_scrap: 2,
                ..StrategicDecision::default()
            },
            raid_decision: StrategicDecision {
                committed_scrap: 3,
                ..StrategicDecision::default()
            },
        }
    }

    fn advanced(snapshots: PlannerSnapshots) -> AdvancedPlannerWork {
        AdvancedPlannerWork {
            team_decision: StrategicDecision::default(),
            raid_decision: StrategicDecision::default(),
            team_started_at: 0,
            lift_started_at: 0,
            raid_started_at: 0,
            lift_was_active: false,
            initial_lift_support: LiftAirSupport::Independent,
            lift_unavailable: Vec::new(),
            preliminary_core: CombatCoreStatus {
                projected_strength: 0,
                target_strength: 0,
                missing_strength: 0,
                missing_scrap: 0,
                ready: true,
            },
            preliminary_core_exclusions: Vec::new(),
            snapshots,
        }
    }

    fn prime_profile() -> ResolvedProfile {
        BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 7).resolve_profile()
    }

    fn foundry_case() -> FoundryOpportunityCase {
        FoundryOpportunityCase::fixture(
            FoundryUrgency::Timely,
            FoundryConfidence::Supported,
            FoundryStrategicValue::Material,
            FoundryTimeToImpact::Near,
            FoundryExecutionSafety::Secure,
        )
    }

    fn connected_observation(tick: Tick, scrap: u32) -> Observation {
        const HOME: TilePos = TilePos::new(3, 10);
        const TARGET: TilePos = TilePos::new(24, 10);
        let mut observation = Observation {
            tick,
            scrap,
            map_width: 32,
            map_height: 20,
            visible: vec![true; 32 * 20],
            explored: vec![true; 32 * 20],
            enemy_buildings: vec![observed_building(80, 1, BuildingKind::Crucible, TARGET)],
            ..Observation::default()
        };
        observation.my_units.extend((1..=13).map(|id| {
            owned_unit(
                id,
                UnitKind::Sentinel,
                TilePos::new(
                    6 + i32::try_from(id % 5).unwrap(),
                    8 + i32::try_from(id / 5).unwrap(),
                ),
            )
        }));
        observation
            .my_units
            .push(owned_unit(100, UnitKind::Kestrel, TilePos::new(8, 10)));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        observation.my_buildings = vec![
            observed_building(10, 0, BuildingKind::Foundry, HOME),
            observed_building(11, 0, BuildingKind::Fabricator, TilePos::new(2, 2)),
            observed_building(12, 0, BuildingKind::Airworks, TilePos::new(5, 2)),
            observed_building(13, 0, BuildingKind::Crucible, TilePos::new(8, 2)),
        ];
        observation.my_queues = vec![Vec::new(); observation.my_buildings.len()];
        observation.my_queue_progress = vec![0; observation.my_buildings.len()];
        observation
    }

    fn connected_inventory_transfer_observation(scrap: u32, bombard_count: u32) -> Observation {
        let mut observation = connected_observation(120, scrap);
        observation
            .my_units
            .extend((0..bombard_count).map(|offset| {
                owned_unit(
                    201 + offset,
                    UnitKind::Bombard,
                    TilePos::new(8 + i32::try_from(offset).unwrap(), 11),
                )
            }));
        observation
            .my_units
            .push(owned_unit(301, UnitKind::Moth, TilePos::new(8, 12)));
        observation.my_units.extend((0..4).map(|offset| {
            owned_unit(
                401 + offset,
                UnitKind::Harvester,
                TilePos::new(4 + i32::try_from(offset).unwrap(), 14),
            )
        }));
        observation.enemy_buildings.push(observed_building(
            81,
            1,
            BuildingKind::Turret,
            TilePos::new(21, 13),
        ));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        observation
    }

    fn connected_briefing(observation: &Observation) -> PublicMapBriefing {
        PublicMapBriefing {
            map_width: observation.map_width,
            map_height: observation.map_height,
            starting_foundries: Vec::new(),
            teams: vec![None, None],
            non_ground_terrain: Vec::new(),
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        }
    }

    fn resolve_guarded_standing_fixture(
        kind: UnitKind,
        reason: StandingForceReason,
        urgency: Urgency,
    ) -> CrossDomainSettlement {
        let observation = connected_observation(120, kind.stats().cost);
        let resources = ResourceSnapshot::from_observation(&observation);
        let producer = match kind {
            UnitKind::Sentinel => BuildingId(10),
            UnitKind::Flakhound => BuildingId(11),
            other => panic!("the guard fixture has no producer for {other:?}"),
        };
        let ready_before = observation
            .tick
            .saturating_add(Tick::from(kind.stats().train_ticks))
            .saturating_add(1);
        let standing = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: observation.tick,
            ready_before,
            kind,
            reason,
            specialty: Specialty::Fortification,
            personality_emphasis: 100,
            case: ProposalCase {
                urgency,
                confidence: Confidence::Current,
                value: StrategicValue::Decisive,
                time_to_impact: TimeToImpact::Immediate,
                safety: ExecutionSafety::Secure,
            },
            eligible_producers: vec![producer],
        });
        let proposal = standing_force_investment_proposals(vec![standing])
            .expect("the exact Standing claim is valid")
            .pop()
            .expect("the fixture yields one Standing proposal");
        let proposal =
            standing_force_with_voluntary_guard(proposal, UnitKind::Sentinel.stats().cost);
        let mut allocation = CrossDomainAllocation::new(&resources, ready_before, 12)
            .expect("the bounded producer projection is valid");
        allocation.offer(proposal);
        allocation
            .resolve(AllocationPersonality::default(), None)
            .expect("the optional Standing portfolio resolves")
    }

    fn current_connected_proposal(observation: &Observation) -> FreshConnectedProposal {
        const HOME: TilePos = TilePos::new(3, 10);
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let briefing = connected_briefing(observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(observation);
        let resources = ResourceSnapshot::from_observation(observation);
        StrategicPlanner::new()
            .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                &profile,
                tuning,
                observation,
                &resources,
                &intelligence,
                HOME,
                StrategicCoordination {
                    enlisted: &[],
                    lift_support: None,
                    allow_new_operation: true,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                    public_map: Some(&briefing),
                    orientation: Orientation::for_home(observation, HOME),
                },
            ))
            .expect("the current connected opportunity is feasible")
            .expect("the current connected opportunity needs a force package")
    }

    fn fixture_connected_proposal(
        deadline: Tick,
        provider_jobs: Vec<ConnectedProviderJob>,
    ) -> FreshConnectedProposal {
        FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective: BuildingId(700),
            anchor: TilePos::new(22, 15),
            deadline,
            case: ConnectedOpportunityCase::fixture(
                ConnectedUrgency::Pressing,
                ConnectedConfidence::Current,
                ConnectedStrategicValue::Decisive,
                ConnectedTimeToImpact::Near,
                ConnectedExecutionSafety::Managed,
            ),
            minimum_claims: ConnectedOffenseClaims::fixture(Vec::new(), provider_jobs),
            marginal_additions: Vec::new(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        })
    }

    fn connected_assignments(
        proposal: &FreshConnectedProposal,
        forecast_funded: bool,
    ) -> Vec<ConnectedProducerAssignment> {
        let identity = proposal.identity();
        let mut lanes = Vec::<(BuildingId, Tick)>::new();
        proposal
            .minimum_claims()
            .provider_jobs()
            .iter()
            .enumerate()
            .map(|(request_ordinal, job)| {
                let producer = job.eligible_producers()[0];
                let lane_index = lanes
                    .iter()
                    .position(|(candidate, _)| *candidate == producer)
                    .unwrap_or_else(|| {
                        lanes.push((producer, job.enqueue_not_before()));
                        lanes.len() - 1
                    });
                let starts_at = lanes[lane_index].1.max(job.enqueue_not_before());
                let ready_at = starts_at
                    .saturating_add(Tick::from(job.kind().stats().train_ticks))
                    .saturating_sub(1);
                assert!(ready_at < job.ready_before());
                lanes[lane_index].1 = ready_at.saturating_add(1);
                let cost = job.kind().stats().cost;
                ConnectedProducerAssignment::new(
                    identity,
                    request_ordinal,
                    producer,
                    job.kind(),
                    ConnectedProducerTiming::new(
                        job.enqueue_not_before(),
                        starts_at,
                        ready_at,
                        job.ready_before(),
                    ),
                    if forecast_funded {
                        ConnectedProducerFunding::new(0, cost)
                    } else {
                        ConnectedProducerFunding::new(cost, 0)
                    },
                )
            })
            .collect()
    }

    fn current_connected_planner(
        observation: &Observation,
        forecast_funded: bool,
    ) -> (StrategicPlanner, Vec<ConnectedProducerAssignment>) {
        let mut proposal = current_connected_proposal(observation);
        let assignments = connected_assignments(&proposal, forecast_funded);
        proposal
            .bind_producer_assignments(assignments.clone())
            .expect("the exact minimum schedule binds");
        let mut planner = StrategicPlanner::new();
        planner
            .commit_connected_proposal(proposal)
            .expect("the bound connected package commits");
        (planner, assignments)
    }

    fn run_connected_session(
        observation: &Observation,
        policy: &mut UtilityPolicy,
        strategy: &mut Option<StrategicPlanner>,
    ) -> AllocationSessionOutcome {
        run_connected_session_with_team_decision_and_trace(
            observation,
            policy,
            strategy,
            StrategicDecision::default(),
            None,
        )
    }

    fn run_connected_session_with_team_decision(
        observation: &Observation,
        policy: &mut UtilityPolicy,
        strategy: &mut Option<StrategicPlanner>,
        team_decision: StrategicDecision,
    ) -> AllocationSessionOutcome {
        run_connected_session_with_team_decision_and_trace(
            observation,
            policy,
            strategy,
            team_decision,
            None,
        )
    }

    fn run_connected_session_with_team_decision_and_trace(
        observation: &Observation,
        policy: &mut UtilityPolicy,
        strategy: &mut Option<StrategicPlanner>,
        team_decision: StrategicDecision,
        trace: Option<&mut AllocationTrace>,
    ) -> AllocationSessionOutcome {
        const HOME: TilePos = TilePos::new(3, 10);
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let briefing = connected_briefing(observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(observation);
        let mut lifts = None;
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(strategy, &team, &lifts, &raids);
        let mut work = advanced(snapshots);
        work.team_decision = team_decision;
        AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation,
                home: HOME,
                public_map: &briefing,
                orientation: Orientation::for_home(observation, HOME),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy,
                strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            work,
            trace,
        )
        .run()
    }

    fn advance_connected_after_allocation(
        observation: &Observation,
        strategy: &mut Option<StrategicPlanner>,
        outcome: &AllocationSessionOutcome,
    ) -> StrategicThinkResult {
        const HOME: TilePos = TilePos::new(3, 10);
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let briefing = connected_briefing(observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(observation);
        strategy
            .as_mut()
            .expect("the connected planner remains installed")
            .think_after_connected_adjudication(StrategicThinkContext::new(
                &profile,
                tuning,
                observation,
                &intelligence,
                HOME,
                StrategicCoordination {
                    enlisted: &outcome.planner_claims,
                    lift_support: None,
                    allow_new_operation: outcome.connected_continues
                        || outcome.allow_new_voluntary_operations,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: outcome.budget.connected_forecast_hold,
                    public_map: Some(&briefing),
                    orientation: Orientation::for_home(observation, HOME),
                },
            ))
    }

    fn assert_connected_enters_bounded_recovery(
        observation: &Observation,
        policy: &mut UtilityPolicy,
        strategy: &mut Option<StrategicPlanner>,
        context: &str,
    ) {
        let mut trace = AllocationTrace::default();
        let first = run_connected_session_with_team_decision_and_trace(
            observation,
            policy,
            strategy,
            StrategicDecision::default(),
            Some(&mut trace),
        );
        assert!(
            first.allocation_ok,
            "a stale connected schedule must be downgraded before resolution: {context}"
        );
        assert!(
            trace.connected_context.is_none(),
            "a dropped revision must replace its selected-only Standing contexts: {context}"
        );
        assert!(
            !first.connected_continues,
            "the stale typed obligation cannot survive this allocation pass: {context}"
        );

        let recovery = advance_connected_after_allocation(observation, strategy, &first);
        let operation = strategy
            .as_ref()
            .and_then(StrategicPlanner::air_operation)
            .expect("the failed preparation enters bounded recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover, "{context}");
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::PreparationInfeasible),
            "{context}"
        );
        assert!(
            recovery.decision.intents.iter().any(
                |intent| matches!(intent, Intent::MoveUnits { units, .. } if !units.is_empty())
            ),
            "the recovery transition must issue its one return-home order: {context}"
        );

        let second = run_connected_session(observation, policy, strategy);
        assert!(
            second.allocation_ok,
            "recovery must not re-import the stale producer schedule: {context}"
        );
    }

    #[test]
    fn bounded_capacity_leaves_current_funding_for_older_future_production() {
        let mut observation = observation();
        observation.tick = 120;
        observation.scrap = 100;
        observation.my_buildings.push(BuildingObs {
            id: BuildingId(9),
            player: PlayerId(0),
            kind: BuildingKind::Foundry,
            anchor: TilePos::new(3, 3),
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        observation.my_queues.push(Vec::new());
        let resources = ResourceSnapshot::from_observation(&observation);
        let kind = UnitKind::Harvester;
        let enqueued_at = observation.tick + 12;
        let ready_at = enqueued_at + Tick::from(kind.stats().train_ticks) - 1;
        let deadline = ready_at + 1;
        let older = imported_obligation(
            ObligationClass::PersistentPlan,
            observation.tick - 1,
            ObligationKey::ConnectedOffense {
                objective: BuildingId(90),
                anchor: TilePos::new(12, 12),
            },
            ClaimBundle::new(
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![super::super::ProducerJobClaim::fixed(
                    BuildingId(9),
                    kind,
                    enqueued_at,
                    enqueued_at,
                    ready_at,
                    deadline,
                )],
            )
            .expect("the older fixed production claim is valid"),
        );
        let mut obligations = vec![older];

        push_bounded_capital_reserve(
            &mut obligations,
            &resources,
            BoundedCapitalReserve {
                cadence: 12,
                bank: observation.scrap,
                accepted_at: observation.tick,
                decision_tick: observation.tick,
                key: ObligationKey::Legacy {
                    channel: LegacyChannel::AirworksCapacity,
                    sequence: 1,
                },
                desired: observation.scrap,
                forecast_deadline: deadline,
                older_capital_reserve: 0,
            },
        )
        .expect("the capacity prefix is representable");

        assert_eq!(obligations.len(), 2);
        assert_eq!(
            obligations[1].claims.current_scrap(),
            observation.scrap - kind.stats().cost
        );
        assert!(obligations[1].claims.forecast_scrap().is_empty());
        let mut allocation = CrossDomainAllocation::new(&resources, deadline, 12)
            .expect("the no-income projection is valid");
        for obligation in obligations {
            allocation.import(obligation);
        }
        allocation
            .resolve(AllocationPersonality::default(), None)
            .expect("the older job and bounded capacity fit exactly once");
    }

    #[test]
    fn active_island_producer_context_honors_an_older_saved_foundry_deadline() {
        let observation = connected_observation(120, 1_000);
        let resources = ResourceSnapshot::from_observation(&observation);
        let cadence = 12;
        let producer = BuildingId(12);
        let kind = UnitKind::Kestrel;
        let mut lane = resources
            .planning_projection(observation.tick.saturating_add(1_000), cadence)
            .expect("the test horizon is bounded")
            .producer(producer)
            .expect("the fixture has one Airworks")
            .clone();
        let timing = lane
            .append(kind, observation.tick)
            .expect("the Airworks can accept the island scout immediately");
        let producer_deadline = timing.ready_at.saturating_add(1);
        let foundry_deadline = producer_deadline.saturating_add(600);
        let saved_foundry = imported_obligation(
            ObligationClass::PersistentPlan,
            observation.tick.saturating_sub(2),
            ObligationKey::SavedFoundry {
                anchor: TilePos::new(14, 14),
            },
            ClaimBundle::new(
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .expect("the saved Foundry claims are valid")
            .with_deferrable_capital(DeferrableCapitalClaim {
                through: foundry_deadline,
                amount: 100,
            })
            .expect("the saved Foundry has one bounded capital claim"),
        );
        let active_island = imported_obligation(
            ObligationClass::PersistentPlan,
            observation.tick.saturating_sub(1),
            ObligationKey::Legacy {
                channel: LegacyChannel::StrategicAir,
                sequence: 1,
            },
            ClaimBundle::new(
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![ProducerJobClaim::fixed(
                    producer,
                    kind,
                    observation.tick,
                    timing.starts_at,
                    timing.ready_at,
                    producer_deadline,
                )],
            )
            .expect("the active island producer claim is valid"),
        );

        let (_, due) = retained_producer_context(
            &resources,
            &[saved_foundry, active_island],
            cadence,
            observation.tick,
        )
        .expect("the island lane and older Foundry horizon must settle together");

        assert_eq!(
            due,
            vec![Intent::TrainAt {
                building: producer,
                kind,
            }]
        );
    }

    #[test]
    fn active_lift_future_demand_counts_live_queued_and_same_think_carriers_once() {
        let (mut observation, lift, _) = active_lift_fixture();
        let operation = lift
            .operation()
            .expect("the fixture has an active lift")
            .clone();
        assert!(operation.desired_carriers > 4);
        observation
            .my_units
            .push(owned_unit(900, UnitKind::Skyhook, TilePos::new(10, 10)));
        observation
            .my_units
            .push(owned_unit(901, UnitKind::Skyhook, TilePos::new(11, 10)));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        observation.my_queues[1].push(UnitKind::Skyhook);
        observation.my_queue_progress = vec![0; observation.my_queues.len()];
        let prior = [Intent::TrainAt {
            building: BuildingId(2),
            kind: UnitKind::Skyhook,
        }];
        let decision = StrategicDecision {
            intents: vec![Intent::TrainAt {
                building: BuildingId(2),
                kind: UnitKind::Skyhook,
            }],
            ..StrategicDecision::default()
        };
        let resources = ResourceSnapshot::from_observation(&observation);

        let obligation =
            active_lift_future_production_obligation(ActiveLiftFutureProductionContext {
                resources: &resources,
                observation: &observation,
                operation: &operation,
                unavailable: &[UnitId(901)],
                prior_producer_intents: &prior,
                lift_decision: &decision,
                cadence: 12,
                accepted_at: operation.started_at,
            })
            .expect("the retained demand is representable")
            .expect("two carriers remain unpaid");

        assert_eq!(
            obligation.claims.producer_jobs().len(),
            operation.desired_carriers - 4
        );
        assert!(obligation.claims.producer_jobs().iter().all(|job| {
            job.kind() == UnitKind::Skyhook
                && job.enqueue_not_before() == observation.tick + 12
                && job.ready_before() == operation.deadline
                && job.eligible_producers() == [BuildingId(2)]
        }));
        assert_eq!(obligation.claims.current_scrap(), 0);
    }

    #[test]
    fn active_lift_current_production_keeps_the_maximal_schedulable_prefix() {
        let (mut observation, _, _) = active_lift_fixture();
        observation.tick = 120;
        let cost = UnitKind::Skyhook.stats().cost;
        let first_ready = observation
            .tick
            .saturating_add(Tick::from(UnitKind::Skyhook.stats().train_ticks))
            .saturating_sub(1);
        let deadline = first_ready.saturating_add(1);
        let decision = StrategicDecision {
            intents: vec![
                Intent::TrainAt {
                    building: BuildingId(2),
                    kind: UnitKind::Skyhook,
                },
                Intent::TrainAt {
                    building: BuildingId(2),
                    kind: UnitKind::Skyhook,
                },
            ],
            reservations: vec![UnitId(7)],
            committed_scrap: cost.saturating_mul(2),
        };
        let resources = ResourceSnapshot::from_observation(&observation);

        let prefix = feasible_active_lift_current_production_prefix(
            ActiveLiftCurrentProductionContext {
                resources: &resources,
                cadence: 12,
                decision_tick: observation.tick,
                retained_at: 0,
                decision: &decision,
                prior_producer_intents: &[],
                production_deadline: deadline,
            },
            &[],
        )
        .expect("the exact producer projection is valid");

        assert_eq!(prefix.reservations, decision.reservations);
        assert_eq!(prefix.committed_scrap, cost);
        assert_eq!(
            prefix
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::TrainAt { .. }))
                .count(),
            1,
            "one Skyhook finishes immediately before the strict deadline, while the second cannot"
        );
    }

    #[test]
    fn active_lift_future_production_keeps_the_maximal_fundable_prefix() {
        let (mut observation, lift, _) = active_lift_fixture();
        let operation = lift
            .operation()
            .expect("the fixture has an active lift")
            .clone();
        let cost = UnitKind::Skyhook.stats().cost;
        observation.scrap = cost.saturating_mul(2);
        let resources = ResourceSnapshot::from_observation(&observation);
        let context = ActiveLiftFutureProductionContext {
            resources: &resources,
            observation: &observation,
            operation: &operation,
            unavailable: &[],
            prior_producer_intents: &[],
            lift_decision: &StrategicDecision::default(),
            cadence: 12,
            accepted_at: operation.started_at,
        };

        let prefix = feasible_active_lift_future_production_obligation(context, &[])
            .expect("the exact producer projection is valid")
            .expect("two future carriers are fundable");
        assert_eq!(prefix.claims.producer_jobs().len(), 2);

        let mut accepted = CrossDomainAllocation::new(&resources, operation.deadline, 12)
            .expect("the lift horizon is valid");
        accepted.import(prefix);
        accepted
            .resolve(AllocationPersonality::default(), None)
            .expect("the selected prefix is feasible");

        let three = active_lift_future_production_obligation_with_limit(context, 3)
            .expect("the larger prefix is representable")
            .expect("the desired wave needs at least three carriers");
        let mut rejected = CrossDomainAllocation::new(&resources, operation.deadline, 12)
            .expect("the lift horizon is valid");
        rejected.import(three);
        assert!(matches!(
            rejected.resolve(AllocationPersonality::default(), None),
            Err(AllocationError::ObligationConflict {
                conflict: AllocationConflict::ProductionFunding { .. },
                ..
            })
        ));
    }

    #[test]
    fn active_lift_does_not_count_carriers_that_miss_its_deadline() {
        let (mut observation, lift, _) = active_lift_fixture();
        let operation = lift
            .operation()
            .expect("the fixture has an active lift")
            .clone();
        observation.my_buildings.push(observed_building(
            4,
            0,
            BuildingKind::Airworks,
            TilePos::new(15, 4),
        ));
        observation.my_queues[1] = vec![UnitKind::Condor; crate::stats::QUEUE_CAP - 2];
        observation.my_queues[1].push(UnitKind::Skyhook);
        observation.my_queues.push(Vec::new());
        observation.my_queue_progress = vec![0; observation.my_queues.len()];
        let late_current = [Intent::TrainAt {
            building: BuildingId(2),
            kind: UnitKind::Skyhook,
        }];
        let resources = ResourceSnapshot::from_observation(&observation);

        let obligation =
            active_lift_future_production_obligation(ActiveLiftFutureProductionContext {
                resources: &resources,
                observation: &observation,
                operation: &operation,
                unavailable: &[],
                prior_producer_intents: &late_current,
                lift_decision: &StrategicDecision::default(),
                cadence: 12,
                accepted_at: operation.started_at,
            })
            .expect("the retained demand is representable")
            .expect("the free Airworks can still satisfy the lift");

        assert_eq!(
            obligation.claims.producer_jobs().len(),
            operation.desired_carriers,
            "neither the late paid carrier nor the late current append satisfies demand"
        );
        assert!(
            obligation
                .claims
                .producer_jobs()
                .iter()
                .all(|job| job.eligible_producers() == [BuildingId(4)]),
            "the congested lane cannot satisfy the immutable lift deadline"
        );
    }

    #[test]
    fn active_lift_sees_one_free_slot_after_an_active_connected_current_append() {
        const HOME: TilePos = TilePos::new(5, 15);
        let (observation, lift, _) = active_lift_fixture();
        let operation = lift
            .operation()
            .expect("the fixture has an active lift")
            .clone();
        let profile =
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7).resolve_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let resources = ResourceSnapshot::from_observation(&observation);
        let projection = resources
            .planning_projection(operation.deadline, dials.cadence)
            .expect("the active connected horizon is valid");
        let mut lane = projection
            .producer(BuildingId(2))
            .expect("the fixture has an Airworks")
            .clone();
        let due = lane
            .append(UnitKind::Kestrel, observation.tick)
            .expect("the accepted current append fits");
        let active_connected_due_intents = vec![Intent::TrainAt {
            building: BuildingId(2),
            kind: UnitKind::Kestrel,
        }];
        let active_connected_obligation = imported_obligation(
            ObligationClass::PersistentPlan,
            observation.tick,
            ObligationKey::ConnectedOffense {
                objective: BuildingId(500),
                anchor: TilePos::new(50, 15),
            },
            ClaimBundle::new(
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                vec![super::super::ProducerJobClaim::fixed(
                    BuildingId(2),
                    UnitKind::Kestrel,
                    observation.tick,
                    due.starts_at,
                    due.ready_at,
                    operation.deadline,
                )],
            )
            .expect("the accepted connected append is a valid obligation"),
        );
        let mut obligations = ObligationPreparation {
            resources,
            obligations: vec![active_connected_obligation],
            coordinator_failure: None,
            active_connected: None,
            active_lift: None,
            invalid_active_connected: false,
            invalid_active_lift: false,
            legacy_air_claims: None,
            staged_strategy: None,
        };
        let mut air_lift = AirLiftPreparation {
            lift_decision: StrategicDecision::default(),
            opening_bootstrap: 0,
            airworks_capacity: 0,
            active_lift_precedes_foundry: false,
            active_lift_spendable: 0,
            saved_plan_reserve_already_imported: 0,
            lift_airworks_capacity: 0,
            lift_deadline: operation.deadline,
            fresh_lift_producer_jobs: 0,
            voluntary_scrap_guard: 0,
        };
        let public_map = connected_briefing(&observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let mut strategy = None;
        let mut lifts = Some(lift);
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut work = advanced(snapshots);
        work.lift_was_active = true;
        work.lift_started_at = operation.started_at;
        let mut session = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: HOME,
                public_map: &public_map,
                orientation: Orientation::for_home(&observation, HOME),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            work,
            None,
        );

        session.advance_active_lift(
            &mut obligations,
            &mut air_lift,
            &active_connected_due_intents,
            true,
        );

        assert_eq!(
            air_lift
                .lift_decision
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::TrainAt {
                        building: BuildingId(2),
                        kind: UnitKind::Skyhook,
                    }
                ))
                .count(),
            1,
            "the accepted current prefix occupies one shallow slot, not two"
        );
    }

    #[test]
    fn active_lift_future_airwork_enters_the_shared_allocation() {
        let (mut observation, lift, _) = active_lift_fixture();
        let desired_carriers = lift
            .operation()
            .expect("the fixture has an active lift")
            .desired_carriers;
        observation.my_buildings.push(observed_building(
            4,
            0,
            BuildingKind::Airworks,
            TilePos::new(15, 4),
        ));
        observation.my_queues.push(Vec::new());
        observation.my_queue_progress.push(0);

        let (trace, outcome) =
            allocation_run_for(&observation, Some(StrategicPlanner::new()), Some(lift));

        assert!(outcome.allocation_ok, "{trace:#?}");
        assert!(outcome.accepted_connected, "{trace:#?}");
        let lift_future = trace
            .obligations
            .entries
            .iter()
            .find(|obligation| {
                matches!(
                    obligation.key,
                    crate::bot::trace::ObligationKeyTrace::Legacy {
                        channel: crate::bot::trace::LegacyChannelTrace::Lift,
                        sequence: 2,
                    }
                )
            })
            .expect("the active lift retains its unpaid Skyhook demand");
        assert!(lift_future.claims.producer_jobs.total > 0);
        assert!(lift_future.claims.producer_jobs.entries.iter().all(|job| {
            job.kind == UnitKind::Skyhook
                && matches!(
                    job.access,
                    crate::bot::trace::ProducerJobAccessTrace::Flexible { .. }
                )
        }));
        let connected = trace
            .proposals
            .entries
            .iter()
            .find(|proposal| {
                matches!(
                    proposal.key,
                    crate::bot::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
                )
            })
            .expect("outstanding lift work must not prevent connected allocation");
        assert_eq!(
            connected.disposition,
            crate::bot::trace::ProposalDispositionTrace::Accepted
        );

        let lift_jobs = trace
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                job.kind == UnitKind::Skyhook
                    && matches!(
                        job.owner,
                        crate::bot::trace::ClaimOwnerTrace::Obligation {
                            key: crate::bot::trace::ObligationKeyTrace::Legacy {
                                channel: crate::bot::trace::LegacyChannelTrace::Lift,
                                ..
                            },
                            ..
                        }
                    )
            })
            .count();
        assert_eq!(lift_jobs, desired_carriers);
        for producer in [BuildingId(2), BuildingId(4)] {
            let mut lane = trace
                .producer_schedule
                .entries
                .iter()
                .filter(|job| job.producer == producer)
                .collect::<Vec<_>>();
            lane.sort_unstable_by_key(|job| (job.starts_at, job.ready_at, job.request_ordinal));
            assert!(lane.iter().any(|job| {
                matches!(
                    job.owner,
                    crate::bot::trace::ClaimOwnerTrace::Obligation {
                        key: crate::bot::trace::ObligationKeyTrace::Legacy {
                            channel: crate::bot::trace::LegacyChannelTrace::Lift,
                            ..
                        },
                        ..
                    }
                )
            }));
            assert!(lane.iter().any(|job| {
                matches!(
                    job.owner,
                    crate::bot::trace::ClaimOwnerTrace::Proposal {
                        key: crate::bot::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. },
                    }
                )
            }));
            assert!(
                lane.windows(2)
                    .all(|pair| pair[0].ready_at < pair[1].starts_at),
                "producer {producer:?} has overlapping exact assignments: {lane:#?}"
            );
        }
    }

    #[test]
    fn active_lift_future_airwork_rejects_an_incompatible_shared_lane_proposal() {
        let (observation, lift, _) = active_lift_fixture();
        let mut operation = lift
            .operation()
            .expect("the fixture has an active lift")
            .clone();
        let lift_ticks = Tick::try_from(operation.desired_carriers)
            .expect("the fixture carrier count fits a tick")
            .saturating_mul(Tick::from(UnitKind::Skyhook.stats().train_ticks));
        operation.deadline = observation
            .tick
            .saturating_add(12)
            .saturating_add(lift_ticks);
        let resources = ResourceSnapshot::from_observation(&observation);
        let lift_obligation =
            active_lift_future_production_obligation(ActiveLiftFutureProductionContext {
                resources: &resources,
                observation: &observation,
                operation: &operation,
                unavailable: &[],
                prior_producer_intents: &[],
                lift_decision: &StrategicDecision::default(),
                cadence: 12,
                accepted_at: operation.started_at,
            })
            .expect("the retained demand is representable")
            .expect("the active lift still needs carriers");
        let provider_jobs = vec![ConnectedProviderJob::fixture(
            UnitKind::Kestrel,
            observation.tick,
            operation.deadline,
            vec![BuildingId(2)],
        )];
        let connected = fixture_connected_proposal(operation.deadline, provider_jobs);

        let mut control = CrossDomainAllocation::new(&resources, operation.deadline, 12)
            .expect("the fixture projection is valid");
        control.offer(
            connected_investment_proposal(connected.clone())
                .expect("the connected claim shape is valid"),
        );
        let mut control_trace = AllocationTrace::default();
        control
            .resolve(AllocationPersonality::default(), Some(&mut control_trace))
            .expect("the connected proposal is independently feasible");
        assert_eq!(
            control_trace.proposals.entries[0].disposition,
            crate::bot::trace::ProposalDispositionTrace::Accepted
        );

        let resolve = || {
            let mut allocation = CrossDomainAllocation::new(&resources, operation.deadline, 12)
                .expect("the fixture projection is valid");
            allocation.import(lift_obligation.clone());
            allocation.offer(
                connected_investment_proposal(connected.clone())
                    .expect("the connected claim shape is valid"),
            );
            let mut trace = AllocationTrace::default();
            allocation
                .resolve(AllocationPersonality::default(), Some(&mut trace))
                .expect("mandatory lift work remains feasible");
            trace
        };
        let trace = resolve();
        assert_eq!(
            trace,
            resolve(),
            "shared-lane rejection must be deterministic"
        );

        let proposal = trace
            .proposals
            .entries
            .first()
            .expect("the connected proposal reached shared allocation");
        assert!(matches!(
            proposal.disposition,
            crate::bot::trace::ProposalDispositionTrace::Infeasible {
                conflict: crate::bot::trace::AllocationConflictTrace::ProducerSchedule { .. },
            }
        ));
        assert_eq!(
            trace.producer_schedule.total,
            u32::try_from(operation.desired_carriers)
                .expect("the fixture carrier count fits the trace")
        );
        assert!(trace.producer_schedule.entries.iter().all(|job| {
            job.kind == UnitKind::Skyhook
                && matches!(
                    job.owner,
                    crate::bot::trace::ClaimOwnerTrace::Obligation {
                        key: crate::bot::trace::ObligationKeyTrace::Legacy {
                            channel: crate::bot::trace::LegacyChannelTrace::Lift,
                            sequence: 2,
                        },
                        ..
                    }
                )
        }));
    }

    #[test]
    fn successful_commit_retains_speculative_planners_and_prepared_output() {
        let observation = observation();
        let briefing = briefing();
        let profile =
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7).resolve_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let intelligence = StrategicIntelligence::new();
        let original_policy = UtilityPolicy::new();
        let mut policy = original_policy.clone();
        policy.record_dispatched_build(&observation, BuildingKind::Turret, TilePos::new(4, 4));
        assert_ne!(policy, original_policy);
        let committed_policy = policy.clone();
        let original_strategy = Some(StrategicPlanner::new());
        let mut strategy = None;
        let original_team = Some(TeamReliefPlanner::new());
        let mut team = None;
        let original_lifts = Some(LiftPlanner::new());
        let mut lifts = None;
        let original_raids = Some(RaidPlanner::new());
        let mut raids = None;
        let resources = ResourceSnapshot::from_observation(&observation);
        let settlement = CrossDomainAllocation::new(&resources, 120, dials.cadence)
            .expect("the empty resource projection is valid")
            .resolve(AllocationPersonality::default(), None)
            .expect("an empty portfolio is feasible");
        let session = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: TilePos::new(0, 0),
                public_map: &briefing,
                orientation: Orientation::for_home(&observation, TilePos::new(0, 0)),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(PlannerSnapshots {
                strategy: original_strategy,
                team: original_team,
                lifts: original_lifts,
                raids: original_raids,
            }),
            None,
        );
        let outcome = session.commit_or_restore(ResolvedAllocation {
            prepared: prepared(&observation, None),
            settlement: Some(settlement),
            snapshots: CommitSnapshots {
                policy: original_policy,
            },
            allocation_ok: true,
        });

        assert!(outcome.allocation_ok);
        assert_eq!(outcome.team_decision.committed_scrap, 1);
        assert_eq!(outcome.lift_decision.committed_scrap, 2);
        assert_eq!(outcome.raid_decision.committed_scrap, 3);
        assert_eq!(outcome.planner_claims, vec![UnitId(99)]);
        assert_eq!(outcome.strategic_core_exclusions, vec![UnitId(98)]);
        assert_eq!(policy, committed_policy);
        assert!(strategy.is_none());
        assert!(team.is_none());
        assert!(lifts.is_none());
        assert!(raids.is_none());
    }

    #[test]
    fn successful_commit_lowers_current_jobs_in_the_accepted_schedule_order() {
        let mut observation = observation();
        observation.tick = 120;
        observation.scrap = 10_000;
        let delayed = BuildingId(2);
        let immediate = BuildingId(9);
        observation.my_buildings.extend([
            observed_building(2, 0, BuildingKind::Foundry, TilePos::new(2, 2)),
            observed_building(9, 0, BuildingKind::Foundry, TilePos::new(9, 2)),
        ]);
        observation.my_queues = vec![vec![UnitKind::Sentinel], Vec::new()];
        observation.my_queue_progress = vec![0, 0];

        let profile =
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7).resolve_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let deadline = observation.tick.saturating_add(1_000);
        let resources = ResourceSnapshot::from_observation(&observation);
        let delayed_decision = StrategicDecision {
            intents: vec![Intent::TrainAt {
                building: delayed,
                kind: UnitKind::Harvester,
            }],
            reservations: Vec::new(),
            committed_scrap: UnitKind::Harvester.stats().cost,
        };
        let immediate_decision = StrategicDecision {
            intents: vec![Intent::TrainAt {
                building: immediate,
                kind: UnitKind::Sentinel,
            }],
            reservations: Vec::new(),
            committed_scrap: UnitKind::Sentinel.stats().cost,
        };
        let mut allocation = CrossDomainAllocation::new(&resources, deadline, dials.cadence)
            .expect("the two-producer projection is valid");
        allocation.import(
            legacy_decision_obligation(
                &resources,
                LegacyDecisionRequest {
                    cadence: dials.cadence,
                    accepted_at: 12,
                    decision_tick: observation.tick,
                    channel: LegacyChannel::TeamRelief,
                    sequence: 1,
                    decision: &delayed_decision,
                    prior_producer_intents: &[],
                    production_deadline: deadline,
                },
            )
            .expect("the delayed Foundry append is representable"),
        );
        allocation.import(
            legacy_decision_obligation(
                &resources,
                LegacyDecisionRequest {
                    cadence: dials.cadence,
                    accepted_at: 24,
                    decision_tick: observation.tick,
                    channel: LegacyChannel::Lift,
                    sequence: 1,
                    decision: &immediate_decision,
                    prior_producer_intents: &[],
                    production_deadline: deadline,
                },
            )
            .expect("the immediate Foundry append is representable"),
        );
        let settlement = allocation
            .resolve(AllocationPersonality::default(), None)
            .expect("both mandatory current jobs fit");
        let accepted = settlement
            .producer_schedule()
            .iter()
            .filter(|job| job.enqueued_at == observation.tick)
            .map(|job| Intent::TrainAt {
                building: job.producer,
                kind: job.kind,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            accepted,
            vec![
                Intent::TrainAt {
                    building: immediate,
                    kind: UnitKind::Sentinel,
                },
                Intent::TrainAt {
                    building: delayed,
                    kind: UnitKind::Harvester,
                },
            ],
            "the less-busy higher-id producer must precede the lower-id producer"
        );

        let briefing = briefing();
        let intelligence = StrategicIntelligence::new();
        let original_policy = UtilityPolicy::new();
        let mut policy = original_policy.clone();
        let mut strategy = None;
        let mut lifts = None;
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let session = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: TilePos::new(0, 0),
                public_map: &briefing,
                orientation: Orientation::for_home(&observation, TilePos::new(0, 0)),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(snapshots),
            None,
        );
        let outcome = session.commit_or_restore(ResolvedAllocation {
            prepared: prepared(&observation, None),
            settlement: Some(settlement),
            snapshots: CommitSnapshots {
                policy: original_policy,
            },
            allocation_ok: true,
        });

        assert!(outcome.allocation_ok);
        assert_eq!(
            outcome
                .producer_lane_reservations
                .current_jobs()
                .iter()
                .map(|job| job.producer)
                .collect::<Vec<_>>(),
            vec![delayed, immediate],
            "per-producer prefix bookkeeping intentionally has a different global order"
        );
        assert_eq!(outcome.allocated_producer_intents, accepted);
    }

    #[test]
    fn failed_commit_restores_every_participant_and_freezes_the_outcome() {
        let observation = observation();
        let briefing = briefing();
        let profile =
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7).resolve_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let intelligence = StrategicIntelligence::new();
        let original_policy = UtilityPolicy::new();
        let mut policy = original_policy.clone();
        policy.record_dispatched_build(&observation, BuildingKind::Turret, TilePos::new(4, 4));
        let original_strategy = Some(StrategicPlanner::new());
        let mut strategy = None;
        let original_team = Some(TeamReliefPlanner::new());
        let mut team = None;
        let original_lifts = Some(LiftPlanner::new());
        let mut lifts = None;
        let original_raids = Some(RaidPlanner::new());
        let mut raids = None;
        let session = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: TilePos::new(0, 0),
                public_map: &briefing,
                orientation: Orientation::for_home(&observation, TilePos::new(0, 0)),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(PlannerSnapshots {
                strategy: original_strategy.clone(),
                team: original_team.clone(),
                lifts: original_lifts.clone(),
                raids: original_raids.clone(),
            }),
            None,
        );
        let mut prepared = prepared(
            &observation,
            Some((
                AllocationCoordinatorStageTrace::ObligationCollection,
                AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
            )),
        );
        prepared.staged_strategy = Some(StrategicThinkResult {
            decision: StrategicDecision {
                intents: vec![Intent::TrainAt {
                    building: BuildingId(9),
                    kind: UnitKind::Sentinel,
                }],
                reservations: vec![UnitId(91)],
                committed_scrap: UnitKind::Sentinel.stats().cost,
            },
            rejected_connected_candidate: None,
        });
        prepared.emergency_defense = Some(FreshEmergencyDefense::fixture(
            BuildingKind::Turret,
            TilePos::new(4, 4),
            UnitId(7),
        ));
        let outcome = session.commit_or_restore(ResolvedAllocation {
            prepared,
            settlement: None,
            snapshots: CommitSnapshots {
                policy: original_policy.clone(),
            },
            allocation_ok: false,
        });

        assert!(!outcome.allocation_ok);
        assert_eq!(outcome.team_decision, StrategicDecision::default());
        assert_eq!(outcome.lift_decision, StrategicDecision::default());
        assert_eq!(outcome.raid_decision, StrategicDecision::default());
        assert_eq!(
            outcome.staged_strategy,
            Some(StrategicThinkResult::default())
        );
        assert!(outcome.planner_claims.is_empty());
        assert!(outcome.strategic_core_exclusions.is_empty());
        assert!(outcome.fresh_emergency_defense_intents.is_empty());
        assert!(outcome.fresh_foundry_intents.is_empty());
        assert!(outcome.allocated_producer_intents.is_empty());
        assert_eq!(
            outcome.producer_lane_reservations,
            ProducerLaneReservations::default()
        );
        assert_eq!(outcome.budget.residual_scrap, 0);
        assert_eq!(outcome.budget.utility_spendable, 0);
        assert_eq!(outcome.budget.connected_forecast_hold, u32::MAX);
        assert_eq!(policy, original_policy);
        assert_eq!(strategy, original_strategy);
        assert_eq!(team, original_team);
        assert_eq!(lifts, original_lifts);
        assert_eq!(raids, original_raids);
    }

    #[test]
    fn allocation_failure_restores_policy_state_mutated_during_prepare() {
        let observation = observation();
        let briefing = briefing();
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let intelligence = StrategicIntelligence::new();
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let mut policy = UtilityPolicy::new();
        policy
            .commit_adjudicated_foundry(
                FreshFoundryProposal::fixture(
                    TilePos::new(8, 8),
                    UnitId(777),
                    foundry_cost,
                    0,
                    0,
                    observation.tick,
                    foundry_case(),
                ),
                observation.tick,
                &mut Vec::new(),
            )
            .expect("the fixture installs one exact saved Foundry");
        let original_policy = policy.clone();
        assert_ne!(original_policy, UtilityPolicy::new());
        let resources = ResourceSnapshot::from_observation(&observation);
        let mut prepared_mutation = original_policy.clone();
        assert!(
            prepared_mutation
                .validated_foundry_obligation(&observation, &resources, false, observation.scrap)
                .is_none()
        );
        assert_ne!(
            prepared_mutation, original_policy,
            "this fixture must mutate policy while prepare validates retained work"
        );

        let mut strategy = None;
        let mut lifts = None;
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut work = advanced(snapshots);
        work.team_decision = StrategicDecision {
            intents: vec![Intent::TrainAt {
                building: BuildingId(999),
                kind: UnitKind::Sentinel,
            }],
            reservations: Vec::new(),
            committed_scrap: UnitKind::Sentinel.stats().cost,
        };
        let outcome = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: TilePos::new(0, 0),
                public_map: &briefing,
                orientation: Orientation::for_home(&observation, TilePos::new(0, 0)),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            work,
            None,
        )
        .run();

        assert!(!outcome.allocation_ok);
        assert_eq!(
            policy, original_policy,
            "rollback must start before prepare can clear or age retained policy state"
        );
    }

    #[test]
    fn due_connected_append_preserves_its_later_lane_against_lift_production() {
        const HOME: TilePos = TilePos::new(5, 15);
        let (mut observation, lift, _) = active_lift_fixture();
        observation.my_buildings.push(observed_building(
            4,
            0,
            BuildingKind::Crucible,
            TilePos::new(12, 18),
        ));
        observation.my_queues.push(Vec::new());
        observation.my_queue_progress = vec![0; observation.my_buildings.len()];

        let mut proposal = current_connected_proposal(&observation);
        let mut assignments = connected_assignments(&proposal, false);
        let pair = assignments
            .iter()
            .enumerate()
            .find_map(|(first_index, first)| {
                assignments
                    .iter()
                    .enumerate()
                    .skip(first_index + 1)
                    .find(|(_, later)| later.producer() == first.producer())
                    .map(|(later_index, _)| (first_index, later_index))
            });
        let (due_index, later_index) = pair.expect(
            "the connected minimum must share its Airworks between scout and strike providers",
        );
        let due = assignments[due_index];
        let later = assignments[later_index];
        assert_eq!(due.timing().enqueued_at(), observation.tick);
        let deferred_enqueue = observation.tick.saturating_add(12);
        assert!(deferred_enqueue <= later.timing().starts_at());
        assignments[later_index] = ConnectedProducerAssignment::new(
            proposal.identity(),
            later.request_ordinal(),
            later.producer(),
            later.kind(),
            ConnectedProducerTiming::new(
                deferred_enqueue,
                later.timing().starts_at(),
                later.timing().ready_at(),
                later.timing().ready_before(),
            ),
            ConnectedProducerFunding::new(later.kind().stats().cost, 0),
        );
        let expected_later = assignments[later_index];
        proposal
            .bind_producer_assignments(assignments)
            .expect("delaying the queued append without moving production remains exact");
        let mut strategy = StrategicPlanner::new();
        strategy
            .commit_connected_proposal(proposal)
            .expect("the exact connected schedule commits");
        let mut committed_strategy = Some(strategy.clone());
        let committed = run_connected_session(
            &observation,
            &mut UtilityPolicy::new(),
            &mut committed_strategy,
        );
        assert!(committed.allocation_ok);
        assert_eq!(
            committed
                .allocated_producer_intents
                .iter()
                .filter(|intent| {
                    **intent
                        == (Intent::TrainAt {
                            building: due.producer(),
                            kind: due.kind(),
                        })
                })
                .count(),
            1,
            "the allocator must lower its current job exactly once"
        );
        assert!(
            committed_strategy
                .as_ref()
                .and_then(|planner| planner.active_connected_obligation(&observation))
                .is_some_and(|active| active
                    .provider_jobs()
                    .iter()
                    .any(|assignment| assignment.request_ordinal() == due.request_ordinal())),
            "settlement must leave current jobs visible until post-allocation tactics validate the complete schedule"
        );
        let active = strategy
            .active_connected_obligation(&observation)
            .expect("the committed package exposes its due and future work");
        let resources = ResourceSnapshot::from_observation(&observation);
        let (lanes, due_intents) =
            active_connected_production_context(&resources, &active, 12, observation.tick)
                .expect("the exact connected lane remains executable");
        assert!(due_intents.contains(&Intent::TrainAt {
            building: due.producer(),
            kind: due.kind(),
        }));
        assert!(lanes.jobs().contains(&ReservedProducerJob {
            producer: expected_later.producer(),
            kind: expected_later.kind(),
            enqueued_at: expected_later.timing().enqueued_at(),
            starts_at: expected_later.timing().starts_at(),
            ready_at: expected_later.timing().ready_at(),
            ready_before: expected_later.timing().ready_before(),
        }));

        let projected = project_producer_intents(&observation, &due_intents);
        let admission = LiftAdmission {
            allow_new_commitments: true,
            spendable_scrap: projected.scrap,
            core_reservations: &[],
            minimum_core_equivalents: 0,
        };
        let mut unrestricted_lift = lift.clone();
        let unrestricted = unrestricted_lift.think_with_admission_and_producer_lanes(
            &projected,
            HOME,
            &[],
            LiftAirSupport::Independent,
            admission,
            ProducerLaneReservations::empty(),
        );
        assert!(unrestricted.intents.iter().any(|intent| {
            matches!(
                intent,
                Intent::TrainAt {
                    building,
                    kind: UnitKind::Skyhook,
                } if *building == expected_later.producer()
            )
        }));

        let mut protected_lift = lift;
        let protected = protected_lift.think_with_admission_and_producer_lanes(
            &projected,
            HOME,
            &[],
            LiftAirSupport::Independent,
            admission,
            &lanes,
        );
        assert!(protected.intents.iter().all(|intent| {
            !matches!(
                intent,
                Intent::TrainAt {
                    building,
                    kind: UnitKind::Skyhook,
                } if *building == expected_later.producer()
            )
        }));
        assert!(lanes.jobs().contains(&ReservedProducerJob {
            producer: expected_later.producer(),
            kind: expected_later.kind(),
            enqueued_at: expected_later.timing().enqueued_at(),
            starts_at: expected_later.timing().starts_at(),
            ready_at: expected_later.timing().ready_at(),
            ready_before: expected_later.timing().ready_before(),
        }));
    }

    #[test]
    fn destroyed_connected_producer_enters_bounded_recovery() {
        let mut observation = connected_observation(120, 10_000);
        let (planner, assignments) = current_connected_planner(&observation, false);
        let destroyed = assignments[0].producer();
        let index = observation
            .my_buildings
            .iter()
            .position(|building| building.id == destroyed)
            .expect("the bound producer begins in current sight");
        observation.my_buildings.remove(index);
        observation.my_queues.remove(index);
        observation.my_queue_progress.remove(index);

        assert_connected_enters_bounded_recovery(
            &observation,
            &mut UtilityPolicy::new(),
            &mut Some(planner),
            "destroyed producer",
        );
    }

    #[test]
    fn blocked_connected_queue_ownership_cannot_resurrect_on_later_ordinary_work() {
        let issued_at = 120;
        let mut observation = connected_observation(issued_at, 10_000);
        observation.enemy_buildings = vec![observed_building(
            80,
            1,
            BuildingKind::Turret,
            TilePos::new(24, 10),
        )];
        observation.my_buildings.pop();
        observation.my_queues.pop();
        observation.my_queue_progress.pop();
        observation.my_buildings.push(observed_building(
            14,
            0,
            BuildingKind::Fabricator,
            TilePos::new(8, 2),
        ));
        observation.my_queues.push(Vec::new());
        observation.my_queue_progress.push(0);
        observation.my_units.extend((200..208).map(|id| {
            owned_unit(
                id,
                UnitKind::Condor,
                TilePos::new(8 + i32::try_from(id % 4).unwrap(), 12),
            )
        }));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);

        let (mut planner, assignments) = current_connected_planner(&observation, false);
        let bombard = assignments
            .iter()
            .copied()
            .find(|assignment| assignment.kind() == UnitKind::Bombard)
            .expect("the turret operation buys one suppression Bombard");
        assert_eq!(
            assignments
                .iter()
                .filter(|assignment| assignment.kind() == UnitKind::Bombard)
                .count(),
            1
        );
        planner.mark_current_connected_providers_issued(issued_at);
        let producer_index = observation
            .my_buildings
            .iter()
            .position(|building| building.id == bombard.producer())
            .expect("the exact Fabricator remains observable");
        observation.my_queues[producer_index] = std::iter::once(UnitKind::Bombard)
            .chain(std::iter::repeat_n(
                UnitKind::Tender,
                crate::stats::QUEUE_CAP.saturating_sub(1),
            ))
            .collect();
        observation.my_queue_progress[producer_index] = UnitKind::Bombard.stats().train_ticks;
        observation.tick = bombard.timing().ready_at().saturating_add(1);

        let mut policy = UtilityPolicy::new();
        let mut strategy = Some(planner);
        let blocked = run_connected_session(&observation, &mut policy, &mut strategy);
        assert!(blocked.allocation_ok);
        assert!(
            strategy
                .as_mut()
                .expect("the active operation remains installed")
                .issued_connected_production_assignments(&observation)
                .contains(&bombard),
            "the shipped session must preserve ownership of the blocked paid occurrence"
        );

        observation.tick = observation.tick.saturating_add(12);
        observation.my_queues[producer_index] = vec![UnitKind::Bombard];
        observation.my_queue_progress[producer_index] = 1;
        policy = UtilityPolicy::new();
        let mut progressing_trace = AllocationTrace::default();
        let progressing = run_connected_session_with_team_decision_and_trace(
            &observation,
            &mut policy,
            &mut strategy,
            StrategicDecision::default(),
            Some(&mut progressing_trace),
        );
        assert!(
            progressing.allocation_ok,
            "the progressing queue must allocate cleanly: {progressing_trace:#?}"
        );
        assert_eq!(
            strategy
                .as_mut()
                .expect("the active operation remains installed")
                .issued_connected_production_assignments(&observation),
            Vec::new(),
            "the observed progressing Bombard proves the paid queue occurrence left"
        );

        observation.tick = observation
            .tick
            .saturating_add(Tick::from(UnitKind::Bombard.stats().train_ticks));
        observation.my_queue_progress[producer_index] = UnitKind::Bombard.stats().train_ticks;
        policy = UtilityPolicy::new();
        let later_blocked = run_connected_session(&observation, &mut policy, &mut strategy);
        assert!(later_blocked.allocation_ok);
        assert_eq!(
            strategy
                .as_mut()
                .expect("the active operation remains installed")
                .issued_connected_production_assignments(&observation),
            Vec::new(),
            "released history cannot reclaim the later blocked ordinary Bombard"
        );
    }

    #[test]
    fn connected_enqueue_missed_after_rollback_enters_bounded_recovery() {
        let mut observation = connected_observation(120, 10_000);
        let mut proposal = current_connected_proposal(&observation);
        let identity = proposal.identity();
        let assignments = connected_assignments(&proposal, false)
            .into_iter()
            .map(|assignment| {
                let timing = assignment.timing();
                let shift = 24;
                ConnectedProducerAssignment::new(
                    identity,
                    assignment.request_ordinal(),
                    assignment.producer(),
                    assignment.kind(),
                    ConnectedProducerTiming::new(
                        timing.enqueued_at().saturating_add(shift),
                        timing.starts_at().saturating_add(shift),
                        timing.ready_at().saturating_add(shift),
                        timing.ready_before(),
                    ),
                    ConnectedProducerFunding::new(assignment.kind().stats().cost, 0),
                )
            })
            .collect::<Vec<_>>();
        proposal
            .bind_producer_assignments(assignments.clone())
            .expect("the future exact minimum schedule binds");
        let mut planner = StrategicPlanner::new();
        planner
            .commit_connected_proposal(proposal)
            .expect("the bound connected package commits");
        let first_enqueue = assignments
            .iter()
            .map(|assignment| assignment.timing().enqueued_at())
            .min()
            .expect("the connected minimum retains provider work");
        observation.tick = first_enqueue;
        let invalid_team_decision = StrategicDecision {
            intents: vec![Intent::TrainAt {
                building: BuildingId(999),
                kind: UnitKind::Sentinel,
            }],
            reservations: Vec::new(),
            committed_scrap: UnitKind::Sentinel.stats().cost,
        };
        let mut policy = UtilityPolicy::new();
        let mut strategy = Some(planner);
        let failed = run_connected_session_with_team_decision(
            &observation,
            &mut policy,
            &mut strategy,
            invalid_team_decision,
        );
        assert!(
            !failed.allocation_ok,
            "the unrelated invalid producer must roll the whole allocation pass back"
        );
        let retained = strategy
            .as_ref()
            .and_then(|planner| planner.active_connected_obligation(&observation))
            .expect("rollback must preserve the previously admitted operation");
        assert!(
            retained
                .provider_jobs()
                .iter()
                .any(|assignment| assignment.timing().enqueued_at() == first_enqueue),
            "rollback must leave the due append unpaid until a later pass diagnoses it"
        );

        observation.tick = first_enqueue.saturating_add(12);
        assert_connected_enters_bounded_recovery(
            &observation,
            &mut policy,
            &mut strategy,
            "missed accepted enqueue after rollback",
        );
    }

    #[test]
    fn lost_forecast_source_recovers_instead_of_spending_unbacked_credit() {
        let mut observation = connected_observation(1_200, 10_000);
        let mut proposal = current_connected_proposal(&observation);
        let mut assignments = connected_assignments(&proposal, false);
        let required_scrap = assignments
            .iter()
            .map(|assignment| assignment.kind().stats().cost)
            .fold(0, u32::saturating_add);
        let deferred_enqueue = observation.tick.saturating_add(12);
        let deferred_index = assignments
            .iter()
            .enumerate()
            .rev()
            .find(|(index, assignment)| {
                let timing = assignment.timing();
                let shifted_start = timing.starts_at().max(deferred_enqueue);
                let delay = shifted_start.saturating_sub(timing.starts_at());
                timing.ready_at().saturating_add(delay) < timing.ready_before()
                    && !assignments[index.saturating_add(1)..]
                        .iter()
                        .any(|later| later.producer() == assignment.producer())
            })
            .map(|(index, _)| index)
            .expect("one last lane job can wait for the next decision boundary");
        let deferred = assignments[deferred_index];
        let shifted_start = deferred.timing().starts_at().max(deferred_enqueue);
        let delay = shifted_start.saturating_sub(deferred.timing().starts_at());
        assignments[deferred_index] = ConnectedProducerAssignment::new(
            proposal.identity(),
            deferred.request_ordinal(),
            deferred.producer(),
            deferred.kind(),
            ConnectedProducerTiming::new(
                deferred_enqueue,
                shifted_start,
                deferred.timing().ready_at().saturating_add(delay),
                deferred.timing().ready_before(),
            ),
            ConnectedProducerFunding::new(deferred.kind().stats().cost - 1, 1),
        );
        proposal
            .bind_producer_assignments(assignments)
            .expect("one later append can retain a one-scrap forecast promise");
        let mut planner = StrategicPlanner::new();
        planner
            .commit_connected_proposal(proposal)
            .expect("the exact forecast-funded package commits");

        observation.scrap = required_scrap - 1;
        let lost_source = BuildingId(200);
        observation.my_buildings.push(observed_building(
            lost_source.0,
            0,
            BuildingKind::Reclaimer,
            TilePos::new(12, 14),
        ));
        observation.my_queues.push(Vec::new());
        observation.my_queue_progress.push(0);
        observation.my_units.extend((300..316).map(|id| {
            owned_unit(
                id,
                UnitKind::Harvester,
                TilePos::new(2 + i32::try_from(id % 8).unwrap(), 16),
            )
        }));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        let foundry_index = observation
            .my_buildings
            .iter()
            .position(|building| building.id == BuildingId(10))
            .expect("the home Foundry remains available");
        observation.my_queues[foundry_index] = vec![UnitKind::Sentinel, UnitKind::Sentinel];
        let funded_resources = ResourceSnapshot::from_observation(&observation);
        let funded_obligation = planner
            .active_connected_obligation(&observation)
            .expect("the committed operation exposes its retained schedule");
        assert!(
            funded_obligation.producer_schedule_is_executable(
                &funded_resources,
                12,
                observation.tick,
            ),
            "the delayed append must remain exact before funding is evaluated"
        );
        let mut funded_allocation =
            CrossDomainAllocation::new(&funded_resources, funded_obligation.deadline(), 12)
                .expect("the connected forecast has a valid horizon");
        funded_allocation.import(
            active_connected_obligation(&funded_obligation)
                .expect("the retained package adapts into one obligation"),
        );
        if let Err(error) = funded_allocation.resolve(AllocationPersonality::default(), None) {
            panic!("the completed Reclaimer must make the retained promise allocatable: {error:?}");
        }
        assert_eq!(
            funded_resources
                .planning_projection(deferred_enqueue, 12)
                .expect("the one-step forecast is bounded")
                .forecast_through(deferred_enqueue),
            1,
            "the completed Reclaimer must supply the exact one-scrap shortfall"
        );
        let mut funded_policy = UtilityPolicy::new();
        let mut funded_strategy = Some(planner.clone());
        let funded = run_connected_session(&observation, &mut funded_policy, &mut funded_strategy);
        assert!(funded.allocation_ok);
        assert!(
            funded.connected_continues,
            "the forecast-funded promise must be legitimate while its source exists"
        );
        let issued_cost = funded_strategy
            .as_ref()
            .and_then(|planner| planner.active_connected_obligation(&observation))
            .expect("the funded operation still exposes its accepted current commands")
            .provider_jobs()
            .iter()
            .filter(|assignment| assignment.timing().enqueued_at() == observation.tick)
            .map(|assignment| assignment.kind().stats().cost)
            .fold(0, u32::saturating_add);
        assert!(issued_cost > 0, "the funded pass issues its current prefix");
        observation.scrap = observation
            .scrap
            .checked_sub(issued_cost)
            .expect("accepted current commands consume only their reserved bank");
        funded_strategy
            .as_mut()
            .expect("the funded operation remains active")
            .mark_current_connected_providers_issued(observation.tick);

        let index = observation
            .my_buildings
            .iter()
            .position(|building| building.id == lost_source)
            .expect("the marginal completed source remains observable");
        observation.my_buildings.remove(index);
        observation.my_queues.remove(index);
        observation.my_queue_progress.remove(index);
        observation.tick = observation.tick.saturating_add(12);
        assert_eq!(
            ResourceSnapshot::from_observation(&observation)
                .forecast()
                .income_through(deferred_enqueue)
                .amount(),
            0,
            "removing that exact Reclaimer must erase the promised income"
        );

        let mut strategy = funded_strategy;
        assert_connected_enters_bounded_recovery(
            &observation,
            &mut UtilityPolicy::new(),
            &mut strategy,
            "lost accepted forecast funding",
        );
    }

    #[test]
    fn active_connected_revision_and_saved_foundry_commit_together() {
        let mut observation = connected_observation(1_200, 10_000);
        let builder = UnitId(200);
        observation.my_units.push(owned_unit(
            builder.0,
            UnitKind::Harvester,
            TilePos::new(12, 15),
        ));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        observation.my_buildings.push(observed_building(
            14,
            0,
            BuildingKind::Extractor,
            TilePos::new(20, 14),
        ));
        observation.my_queues.push(Vec::new());
        observation.my_queue_progress.push(0);
        let mut proposal = current_connected_proposal(&observation);
        let identity = proposal.identity();
        let assignments = connected_assignments(&proposal, false)
            .into_iter()
            .map(|assignment| {
                let timing = assignment.timing();
                let shift = 24;
                ConnectedProducerAssignment::new(
                    identity,
                    assignment.request_ordinal(),
                    assignment.producer(),
                    assignment.kind(),
                    ConnectedProducerTiming::new(
                        timing.enqueued_at().saturating_add(shift),
                        timing.starts_at().saturating_add(shift),
                        timing.ready_at().saturating_add(shift),
                        timing.ready_before(),
                    ),
                    ConnectedProducerFunding::new(assignment.kind().stats().cost, 0),
                )
            })
            .collect::<Vec<_>>();
        proposal
            .bind_producer_assignments(assignments.clone())
            .expect("the future exact minimum schedule binds");
        let mut planner = StrategicPlanner::new();
        planner
            .commit_connected_proposal(proposal)
            .expect("the bound connected package commits");
        let fixed_deadline = assignments[0].timing().ready_before();
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let foundry_anchor = TilePos::new(15, 14);
        let mut policy = UtilityPolicy::new();
        policy
            .commit_adjudicated_foundry(
                FreshFoundryProposal::fixture(
                    foundry_anchor,
                    builder,
                    foundry_cost,
                    0,
                    0,
                    fixed_deadline,
                    foundry_case(),
                ),
                observation.tick,
                &mut Vec::new(),
            )
            .expect("the fixture installs one exact saved Foundry");

        observation.tick = observation.tick.saturating_add(12);
        let mut strategy = Some(planner);
        let outcome = run_connected_session(&observation, &mut policy, &mut strategy);

        assert!(outcome.allocation_ok);
        assert!(
            outcome.accepted_connected,
            "the revisable operation must re-enter the typed allocation"
        );
        assert!(outcome.fresh_foundry_intents.contains(&Intent::BuildWith {
            builder,
            kind: BuildingKind::Foundry,
            anchor: foundry_anchor,
        }));
        assert!(
            !outcome.allocated_producer_intents.is_empty(),
            "ample residual capital should still admit fresh standing-force production"
        );
        let retained = strategy
            .as_ref()
            .and_then(|planner| planner.active_connected_obligation(&observation))
            .expect("the revised connected operation remains active");
        assert_eq!(retained.deadline(), fixed_deadline);
        assert_eq!(
            retained
                .provider_jobs()
                .iter()
                .take(assignments.len())
                .map(|assignment| {
                    (
                        assignment.request_ordinal(),
                        assignment.producer(),
                        assignment.kind(),
                        assignment.timing(),
                    )
                })
                .collect::<Vec<_>>(),
            assignments
                .iter()
                .map(|assignment| {
                    (
                        assignment.request_ordinal(),
                        assignment.producer(),
                        assignment.kind(),
                        assignment.timing(),
                    )
                })
                .collect::<Vec<_>>(),
            "a compatible Foundry cannot shift accepted connected jobs while the revision adds new marginal work"
        );
    }

    #[test]
    fn pressing_standing_counter_spends_before_the_voluntary_sentinel_guard() {
        let pressing = resolve_guarded_standing_fixture(
            UnitKind::Flakhound,
            StandingForceReason::AirDefense,
            Urgency::Pressing,
        );
        assert_eq!(pressing.producer_schedule().len(), 1);
        assert_eq!(pressing.producer_schedule()[0].kind, UnitKind::Flakhound);
        assert_eq!(
            pressing.producer_schedule()[0].current_scrap,
            UnitKind::Flakhound.stats().cost
        );
        assert_eq!(pressing.residual_current_scrap(), 0);

        let developmental = resolve_guarded_standing_fixture(
            UnitKind::Flakhound,
            StandingForceReason::ForceProjection,
            Urgency::Developmental,
        );
        assert!(developmental.producer_schedule().is_empty());
        assert_eq!(
            developmental.residual_current_scrap(),
            UnitKind::Flakhound.stats().cost,
            "a voluntary specialist still preserves the shallow line-unit fund"
        );

        let recovery = resolve_guarded_standing_fixture(
            UnitKind::Sentinel,
            StandingForceReason::CoreRecovery,
            Urgency::Pressing,
        );
        assert_eq!(recovery.producer_schedule().len(), 1);
        assert_eq!(recovery.producer_schedule()[0].kind, UnitKind::Sentinel);
        assert!(
            recovery.voluntary_scrap_guard_satisfied(),
            "the shallow Sentinel continues to discharge its own guard"
        );
    }

    #[test]
    fn zero_cost_connected_operation_does_not_require_unowned_guard_capital() {
        let observation = connected_observation(120, 0);
        let deadline = observation.tick.saturating_add(1);
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let briefing = connected_briefing(&observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let original_policy = policy.clone();
        let mut strategy = Some(StrategicPlanner::new());
        let mut lifts = None;
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut input = prepared(&observation, None);
        input.resources = ResourceSnapshot::from_observation(&observation);
        input.fresh_connected = Some(fixture_connected_proposal(deadline, Vec::new()));
        input.allocation_horizon = deadline;
        input.voluntary_scrap_guard = UnitKind::Sentinel.stats().cost;
        let mut session = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: TilePos::new(3, 10),
                public_map: &briefing,
                orientation: Orientation::for_home(&observation, TilePos::new(3, 10)),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(snapshots),
            None,
        );
        let resolved = session.resolve(
            input,
            CommitSnapshots {
                policy: original_policy,
            },
        );
        let outcome = session.commit_or_restore(resolved);

        assert!(outcome.allocation_ok);
        assert!(outcome.accepted_connected);
        assert!(outcome.allocated_producer_intents.is_empty());
        assert_eq!(
            outcome.budget.voluntary_scrap_guard,
            UnitKind::Sentinel.stats().cost,
            "the unavailable current guard cannot block the no-spend operation, but residual work must still preserve the desired shallow fund"
        );
        assert_eq!(outcome.budget.residual_scrap, 0);
        assert_eq!(outcome.budget.utility_spendable, 0);
    }

    #[test]
    fn fresh_standing_force_cannot_defer_a_payable_saved_foundry() {
        let mut observation = connected_observation(1_200, 0);
        let builder = UnitId(200);
        let foundry_anchor = TilePos::new(15, 14);
        observation.my_units.push(owned_unit(
            builder.0,
            UnitKind::Harvester,
            TilePos::new(12, 15),
        ));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        for (id, kind, anchor) in [
            (14, BuildingKind::Extractor, TilePos::new(20, 14)),
            (15, BuildingKind::Reclaimer, TilePos::new(12, 17)),
        ] {
            observation
                .my_buildings
                .push(observed_building(id, 0, kind, anchor));
            observation.my_queues.push(Vec::new());
            observation.my_queue_progress.push(0);
        }
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let forecast_deadline = observation
            .tick
            .saturating_add(Tick::from(foundry_cost).saturating_mul(crate::stats::RECLAIMER_PERIOD))
            .saturating_add(crate::stats::RECLAIMER_PERIOD);
        let mut policy = UtilityPolicy::new();
        let mut initial_intents = Vec::new();
        policy
            .commit_adjudicated_foundry(
                FreshFoundryProposal::fixture(
                    foundry_anchor,
                    builder,
                    0,
                    foundry_cost,
                    0,
                    forecast_deadline,
                    foundry_case(),
                ),
                observation.tick,
                &mut initial_intents,
            )
            .expect("the forecast-backed fixture installs one exact saved Foundry");
        assert!(
            initial_intents.is_empty(),
            "forecast capital cannot dispatch the Foundry at admission"
        );

        observation.tick = observation.tick.saturating_add(12);
        observation.scrap = foundry_cost;
        let resources = ResourceSnapshot::from_observation(&observation);
        let saved = policy
            .validated_foundry_obligation(&observation, &resources, true, observation.scrap)
            .expect("the accepted Foundry remains valid after its bank accrues");
        assert!(
            saved.ready_to_build(),
            "the full current construction cost makes the retained plan payable"
        );

        let standing_ready_before = observation
            .tick
            .saturating_add(Tick::from(UnitKind::Lancer.stats().train_ticks))
            .saturating_add(1);
        let standing = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: observation.tick,
            ready_before: standing_ready_before,
            kind: UnitKind::Lancer,
            reason: StandingForceReason::ForceProjection,
            specialty: Specialty::Siege,
            personality_emphasis: 100,
            case: ProposalCase {
                urgency: Urgency::Pressing,
                confidence: Confidence::Current,
                value: StrategicValue::Decisive,
                time_to_impact: TimeToImpact::Immediate,
                safety: ExecutionSafety::Secure,
            },
            eligible_producers: vec![BuildingId(11)],
        });
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let briefing = connected_briefing(&observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let original_policy = policy.clone();
        let mut strategy = None;
        let mut lifts = None;
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut prepared = prepared(&observation, None);
        prepared.resources = resources;
        prepared.obligations = vec![
            saved_foundry_obligation(saved)
                .expect("the ready saved Foundry has exact mandatory claims"),
        ];
        prepared.saved_foundry = Some(saved);
        prepared.standing_force = StandingForcePreparation::Unconditional(vec![standing]);
        prepared.allocation_horizon = forecast_deadline.max(standing_ready_before);
        prepared.foundry_saving = foundry_cost;
        let mut session = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: TilePos::new(3, 10),
                public_map: &briefing,
                orientation: Orientation::for_home(&observation, TilePos::new(3, 10)),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(snapshots),
            None,
        );
        let resolved = session.resolve(
            prepared,
            CommitSnapshots {
                policy: original_policy,
            },
        );
        let outcome = session.commit_or_restore(resolved);

        assert!(outcome.allocation_ok);
        assert_eq!(
            outcome.fresh_foundry_intents,
            vec![Intent::BuildWith {
                builder,
                kind: BuildingKind::Foundry,
                anchor: foundry_anchor,
            }],
            "fresh optional production cannot move a payable persistent plan back onto forecast"
        );
        assert!(
            outcome.allocated_producer_intents.is_empty(),
            "the Standing Lancer must wait when the older Foundry consumes the current bank"
        );
    }

    #[test]
    fn connected_and_lift_keep_one_shared_future_lane_across_the_next_think() {
        const HOME: TilePos = TilePos::new(3, 10);
        let mut observation = connected_observation(0, 10_000);
        observation.known_rock = (0..observation.map_height)
            .map(|y| TilePos::new(16, y))
            .collect();

        let mut lift = LiftPlanner::new();
        lift.think_with_admission(
            &observation,
            HOME,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: true,
                spendable_scrap: observation.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 5,
            },
        );
        let lift_operation = lift
            .operation()
            .expect("the blocked connected fixture admits a Lift")
            .clone();
        assert_eq!(lift_operation.phase, LiftPhase::Provision);

        let mut connected = current_connected_proposal(&observation);
        let identity = connected.identity();
        let shift = 24;
        let connected_assignments = connected_assignments(&connected, false)
            .into_iter()
            .map(|assignment| {
                let timing = assignment.timing();
                ConnectedProducerAssignment::new(
                    identity,
                    assignment.request_ordinal(),
                    assignment.producer(),
                    assignment.kind(),
                    ConnectedProducerTiming::new(
                        timing.enqueued_at().saturating_add(shift),
                        timing.starts_at().saturating_add(shift),
                        timing.ready_at().saturating_add(shift),
                        timing.ready_before(),
                    ),
                    ConnectedProducerFunding::new(assignment.kind().stats().cost, 0),
                )
            })
            .collect::<Vec<_>>();
        connected
            .bind_producer_assignments(connected_assignments.clone())
            .expect("the future connected schedule binds");
        let mut strategy = StrategicPlanner::new();
        strategy
            .commit_connected_proposal(connected)
            .expect("the connected package commits");

        let airworks = BuildingId(12);
        let lift_starts_at = connected_assignments
            .iter()
            .filter(|assignment| assignment.producer() == airworks)
            .map(|assignment| assignment.timing().ready_at())
            .max()
            .expect("the connected package uses the shared Airworks")
            .saturating_add(1);
        let lift_ready_at = lift_starts_at
            .saturating_add(Tick::from(UnitKind::Skyhook.stats().train_ticks))
            .saturating_sub(1);
        assert!(lift_ready_at < lift_operation.deadline);
        let lift_assignment = LiftProducerAssignment::new(
            0,
            airworks,
            UnitKind::Skyhook,
            LiftProducerTiming::new(
                observation.tick.saturating_add(shift),
                lift_starts_at,
                lift_ready_at,
                lift_operation.deadline,
            ),
            LiftProducerFunding::new(UnitKind::Skyhook.stats().cost, 0),
        );
        lift.bind_producer_assignments(
            lift_operation.started_at,
            lift_operation.deadline,
            vec![lift_assignment],
        )
        .expect("the later exact Lift assignment binds");
        let resources = ResourceSnapshot::from_observation(&observation);
        assert!(
            !lift
                .active_production_obligation()
                .unwrap()
                .producer_schedule_is_executable(&resources, 12, observation.tick),
            "the later retained job is executable only after the preceding connected job"
        );

        observation.tick = observation.tick.saturating_add(12);
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let briefing = connected_briefing(&observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let mut strategy = Some(strategy);
        let mut lifts = Some(lift);
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut work = advanced(snapshots);
        work.lift_was_active = true;
        work.lift_started_at = lift_operation.started_at;
        let outcome = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: HOME,
                public_map: &briefing,
                orientation: Orientation::for_home(&observation, HOME),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            work,
            None,
        )
        .run();

        assert!(outcome.allocation_ok);
        let retained_connected = strategy
            .as_ref()
            .and_then(|planner| planner.active_connected_obligation(&observation))
            .expect("the connected revision remains active");
        assert_eq!(
            &retained_connected.provider_jobs()[..connected_assignments.len()],
            connected_assignments,
            "accepted connected jobs keep their exact identity before new marginal jobs"
        );
        let retained_lift = lifts
            .as_ref()
            .and_then(LiftPlanner::active_production_obligation)
            .expect("the future Lift assignment remains mandatory");
        assert_eq!(retained_lift.producer_jobs(), &[lift_assignment]);
        assert_eq!(
            lifts.as_ref().unwrap().operation().unwrap().phase,
            LiftPhase::Provision,
            "accepted unpaid carrier work keeps the Lift in Provision"
        );
    }

    #[test]
    fn newer_conflict_does_not_discard_an_older_connected_obligation() {
        let observation = connected_observation(120, 10_000);
        let (planner, assignments) = current_connected_planner(&observation, false);
        let active = planner
            .active_connected_obligation(&observation)
            .expect("the committed operation exposes its exact obligation");
        let first = assignments[0];
        let resources = ResourceSnapshot::from_observation(&observation);
        let newer_accepted_at = active.accepted_at().saturating_add(1);
        let newer_key = ObligationKey::Legacy {
            channel: LegacyChannel::TeamRelief,
            sequence: 99,
        };
        let decision = StrategicDecision {
            intents: vec![Intent::TrainAt {
                building: first.producer(),
                kind: first.kind(),
            }],
            reservations: Vec::new(),
            committed_scrap: first.kind().stats().cost,
        };
        let newer = legacy_decision_obligation(
            &resources,
            LegacyDecisionRequest {
                cadence: 12,
                accepted_at: newer_accepted_at,
                decision_tick: first.timing().enqueued_at(),
                channel: LegacyChannel::TeamRelief,
                sequence: 99,
                decision: &decision,
                prior_producer_intents: &[],
                production_deadline: active.deadline(),
            },
        )
        .expect("the newer producer claim is independently legal");
        let active_import = active_connected_obligation(&active)
            .expect("the older connected obligation adapts exactly");
        let active_owner = active_import.owner();
        let mut proof = CrossDomainAllocation::new(&resources, active.deadline(), 12)
            .expect("the fixture horizon is valid");
        proof.import(active_import.clone());
        proof.import(newer.clone());
        assert!(matches!(
            proof.resolve(AllocationPersonality::default(), None),
            Err(AllocationError::ObligationConflict {
                obligation: ClaimOwner::Obligation {
                    class: ObligationClass::Legacy,
                    accepted_at,
                    key,
                },
                conflict: AllocationConflict::ProducerSchedule { .. },
            }) if accepted_at == newer_accepted_at && key == newer_key
        ));

        let mut obligations = ObligationPreparation {
            resources,
            obligations: vec![active_import, newer],
            coordinator_failure: None,
            active_connected: Some(active.clone()),
            active_lift: None,
            invalid_active_connected: false,
            invalid_active_lift: false,
            legacy_air_claims: None,
            staged_strategy: None,
        };
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let dials = Dials::scripted(&profile, tuning);
        let briefing = connected_briefing(&observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let mut strategy = Some(planner);
        let mut lifts = None;
        let mut team = None;
        let mut raids = None;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut session = AllocationSession::new(
            AllocationSessionContext {
                dials: &dials,
                profile: &profile,
                tuning,
                observation: &observation,
                home: TilePos::new(3, 10),
                public_map: &briefing,
                orientation: Orientation::for_home(&observation, TilePos::new(3, 10)),
                intelligence: &intelligence,
                enlisted: &[],
                lift_support: None,
            },
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(snapshots),
            None,
        );
        session.downgrade_unfundable_active_connected(
            &mut SavedFoundryPreparation {
                obligation: None,
                saving: 0,
                blocked: false,
                preparation_need: None,
            },
            &AirLiftPreparation {
                lift_decision: StrategicDecision::default(),
                opening_bootstrap: 0,
                airworks_capacity: 0,
                active_lift_precedes_foundry: false,
                active_lift_spendable: 0,
                saved_plan_reserve_already_imported: 0,
                lift_airworks_capacity: 0,
                lift_deadline: observation.tick.saturating_add(12),
                fresh_lift_producer_jobs: 0,
                voluntary_scrap_guard: 0,
            },
            &mut obligations,
        );

        assert_eq!(obligations.active_connected, Some(active));
        assert!(
            obligations
                .obligations
                .iter()
                .any(|obligation| obligation.owner() == active_owner),
            "a later planner's conflict cannot demote the older connected commitment"
        );
    }

    #[test]
    fn fresh_connected_live_claim_triggers_same_think_standing_replacement() {
        let observation = connected_inventory_transfer_observation(300, 5);
        let mut policy = UtilityPolicy::new();
        let mut strategy = Some(StrategicPlanner::new());
        let mut trace = AllocationTrace::default();

        let outcome = run_connected_session_with_team_decision_and_trace(
            &observation,
            &mut policy,
            &mut strategy,
            StrategicDecision::default(),
            Some(&mut trace),
        );

        assert!(outcome.accepted_connected);
        assert!(
            strategy
                .as_ref()
                .and_then(|planner| planner.active_connected_obligation(&observation))
                .is_some_and(|active| active.units().contains(&UnitId(201))),
            "the accepted operation must own the existing Bombard"
        );
        assert!(
            outcome
                .allocated_producer_intents
                .contains(&Intent::TrainAt {
                    building: BuildingId(12),
                    kind: UnitKind::Buzzard,
                })
        );
        assert!(
            !outcome
                .allocated_producer_intents
                .iter()
                .any(|intent| matches!(
                    intent,
                    Intent::TrainAt {
                        kind: UnitKind::Sentinel | UnitKind::Lancer,
                        ..
                    }
                )),
            "the exact replacement may wait, but cannot turn into an unrelated generic screen"
        );
        assert!(matches!(
            trace
                .connected_context
                .as_ref()
                .map(|context| &context.selected),
            Some(
                crate::bot::trace::ConnectedPortfolioSelectionTrace::Selected {
                    marginal_depth: 0,
                    ..
                }
            )
        ));
        assert!(trace.proposals.entries.iter().any(|proposal| {
            matches!(
                proposal.key,
                crate::bot::trace::ProposalKeyTrace::StandingForce {
                    kind: UnitKind::Lancer,
                    ..
                }
            ) && proposal.case.urgency == crate::bot::trace::UrgencyTrace::Pressing
        }));
        assert_eq!(
            trace
                .producer_schedule
                .entries
                .iter()
                .map(|job| (job.producer, job.kind))
                .collect::<Vec<_>>(),
            vec![(BuildingId(12), UnitKind::Buzzard)]
        );

        let observation = connected_inventory_transfer_observation(400, 5);
        let mut policy = UtilityPolicy::new();
        let mut strategy = Some(StrategicPlanner::new());
        let mut richer_trace = AllocationTrace::default();
        let outcome = run_connected_session_with_team_decision_and_trace(
            &observation,
            &mut policy,
            &mut strategy,
            StrategicDecision::default(),
            Some(&mut richer_trace),
        );

        assert!(outcome.accepted_connected);
        assert!(
            outcome
                .allocated_producer_intents
                .contains(&Intent::TrainAt {
                    building: BuildingId(12),
                    kind: UnitKind::Buzzard,
                })
        );
        assert!(
            outcome
                .allocated_producer_intents
                .contains(&Intent::TrainAt {
                    building: BuildingId(11),
                    kind: UnitKind::Lancer,
                })
        );
        assert!(outcome.allocated_producer_intents.iter().all(|intent| {
            !matches!(
                intent,
                Intent::TrainAt {
                    kind: UnitKind::Sentinel,
                    ..
                }
            )
        }));
        assert_eq!(
            richer_trace
                .producer_schedule
                .entries
                .iter()
                .map(|job| (job.producer, job.kind))
                .collect::<Vec<_>>(),
            vec![
                (BuildingId(12), UnitKind::Buzzard),
                (BuildingId(11), UnitKind::Lancer),
            ]
        );
    }

    #[test]
    fn fresh_connected_paid_queue_claim_triggers_same_think_standing_replacement() {
        let mut observation = connected_inventory_transfer_observation(400, 0);
        let fabricator = observation
            .my_buildings
            .iter()
            .position(|building| building.id == BuildingId(11))
            .expect("the fixture has one Fabricator");
        observation.my_queues[fabricator] = vec![UnitKind::Bombard];
        observation
            .enemy_buildings
            .iter_mut()
            .find(|building| building.kind == BuildingKind::Turret)
            .expect("the fixture has one defensive target")
            .hp = 1;
        let mut policy = UtilityPolicy::new();
        let mut strategy = Some(StrategicPlanner::new());
        let mut trace = AllocationTrace::default();

        let outcome = run_connected_session_with_team_decision_and_trace(
            &observation,
            &mut policy,
            &mut strategy,
            StrategicDecision::default(),
            Some(&mut trace),
        );

        assert!(outcome.accepted_connected);
        assert!(
            outcome
                .allocated_producer_intents
                .contains(&Intent::TrainAt {
                    building: BuildingId(12),
                    kind: UnitKind::Buzzard,
                }),
            "the accepted package still lowers only its missing unpaid provider"
        );
        assert!(
            outcome
                .allocated_producer_intents
                .contains(&Intent::TrainAt {
                    building: BuildingId(11),
                    kind: UnitKind::Lancer,
                }),
            "the paid Bombard belongs to Connected, so Standing must replace its missing siege coverage"
        );
        let lancer = trace
            .proposals
            .entries
            .iter()
            .find(|proposal| {
                matches!(
                    proposal.key,
                    crate::bot::trace::ProposalKeyTrace::StandingForce {
                        kind: UnitKind::Lancer,
                        ..
                    }
                ) && matches!(
                    proposal.disposition,
                    crate::bot::trace::ProposalDispositionTrace::Accepted
                )
            })
            .expect("the selected Connected context accepts the replacement demand");
        assert_eq!(
            lancer.case.urgency,
            crate::bot::trace::UrgencyTrace::Pressing
        );
    }

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
