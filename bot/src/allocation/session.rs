//! One atomic player-facing allocation transaction.
//!
//! The domain planners still decide what work is worth doing. This module owns
//! the lifecycle around those decisions: assemble exact prior claims, resolve
//! the shared portfolio once, then either commit every accepted payload or
//! restore every speculative planner mutation.

use super::{
    AllocationConflict, AllocationError, AllocationPersonality, ClaimBundle, ClaimBundleError,
    ClaimOwner, ConnectedOffenseKey, ConnectedPortfolioContext, CoordinatorInputError,
    CrossDomainAllocation, CrossDomainSettlement, DefenseInvestmentKey, DomainInvestmentProposal,
    ImportedObligation, LegacyChannel, LegacyDecisionRequest, ObligationClass, ObligationKey,
    ProducerJobClaim, ProposalKey, StandingForceKey, Urgency, active_connected_obligation,
    active_connected_revision_investment_proposal, active_connected_revision_obligation,
    clamped_current_reserve_obligation, connected_investment_proposal, current_reserve_at,
    defense_investment_proposals, economic_investment_claims, economic_investment_proposal,
    fixed_production_current_reserve, forecast_reserve_through, foundry_investment_proposal,
    fresh_emergency_defense_obligation, imported_obligation, legacy_decision_obligation,
    legacy_unit_obligation, observed_builder_obligations, saved_foundry_obligation,
    standing_force_investment_proposals,
};
use crate::PublicMapBriefing;
use crate::difficulty::{DifficultyTuning, strategic_admission_tick};
use crate::executive::Intent;
use crate::intelligence::StrategicIntelligence;
use crate::lift::{
    ActiveLiftProductionObligation, LiftAdmission, LiftAirSupport, LiftOperation, LiftPlanner,
    LiftProducerAssignment, LiftProducerFunding, LiftProducerTiming,
};
use crate::observation::Observation;
#[cfg(test)]
use crate::observation::ObservationData;
use crate::orient::Orientation;
use crate::profile::ResolvedProfile;
#[cfg(test)]
use crate::query_work::QueryPurpose;
use crate::raid::RaidPlanner;
use crate::resources::{ProducerLaneReservations, ResourceSnapshot};
use crate::standing_force::{
    StandingForceContext, StandingGroundTarget, StandingProductionCommitment,
    derive_standing_force_with_demand,
};
use crate::strategy::{
    ActiveConnectedObligation, AirOperation, AirOperationOutcome, AirOperationPhase,
    FreshConnectedProposal, FreshConnectedProposalRequest, LiftSupportRequest,
    RejectedConnectedCandidate, StrategicCoordination, StrategicDecision, StrategicPlanner,
    StrategicThinkContext, StrategicThinkResult, connected_preparation_horizon,
};
use crate::team::TeamReliefPlanner;
use crate::trace::{
    AllocationCoordinatorFailureReasonTrace, AllocationCoordinatorStageTrace, AllocationTrace,
};
use crate::utility::{
    AirCapacityDemand, CombatCoreStatus, Dials, EconomicInvestment, EconomicInvestmentContext,
    FreshDefenseProposal, FreshEmergencyDefense, FreshEmergencyDefenseContext,
    FreshFoundryInvestment, FreshFoundryProposal, FreshFoundryProposalContext, SHALLOW_QUEUE_DEPTH,
    SavedFoundryReadiness, SupportWorkSnapshot, UtilityPolicy, ValidatedFoundryObligation,
    combat_core_status,
};
use chassis::Tick;
use chassis::grid::TilePos;
use oxide_sim::ids::UnitId;
use oxide_sim::stats::{BuildingKind, Domain, UnitKind};

mod retained;
mod standing;
use retained::{RetainedPreparation, RetainedWork};
use standing::{
    StandingForceDerivation, StandingForceInputs, StandingForcePreparation, StandingForceWork,
};

#[derive(Default)]
struct SupportPreparation {
    assignments: Vec<crate::utility::RepairAssignment>,
    construction: Vec<EconomicInvestment>,
    repair_work: Vec<crate::standing_force::RepairWork>,
}

struct FreshInvestmentInputs<'a> {
    active_revision: ActiveRevisionPreparation,
    defense_admission_reserve: u32,
    recon_paid_exclusions: &'a [(oxide_sim::ids::BuildingId, UnitKind, usize)],
    standing_force: StandingForceWork,
}

/// One current view over every exact unit retained by a planner.
///
/// Recreate this value after a planner mutates. Each selector names the owners
/// intentionally included at that decision boundary, avoiding positional
/// slices and hand-maintained clone chains in the frame loop.
pub(crate) struct PlannerClaims<'a> {
    enlisted: &'a [UnitId],
    strategy: &'a StrategicPlanner,
    raids: &'a RaidPlanner,
    lifts: &'a LiftPlanner,
}

impl<'a> PlannerClaims<'a> {
    pub(crate) const fn new(
        enlisted: &'a [UnitId],
        strategy: &'a StrategicPlanner,
        raids: &'a RaidPlanner,
        lifts: &'a LiftPlanner,
    ) -> Self {
        Self {
            enlisted,
            strategy,
            raids,
            lifts,
        }
    }

    fn air(&self) -> Option<&crate::strategy::AirOperation> {
        self.strategy.air_operation()
    }

    fn raid_reservations(&self) -> &[UnitId] {
        self.raids.reservations()
    }

    fn lift(&self) -> Option<&LiftOperation> {
        self.lifts.operation()
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

    /// External owners from the raid planner's point of view.
    pub(crate) fn without_raid(&self, relief: &[UnitId]) -> Vec<UnitId> {
        prior_planner_claims(self.enlisted, self.air(), relief, &[], self.lift())
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
    air: Option<&crate::strategy::AirOperation>,
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

fn residual_current_after_obligations(
    resources: &ResourceSnapshot,
    obligations: &[ImportedObligation],
    horizon: Tick,
    cadence: Tick,
    planning: &crate::planning::PlanningWork,
) -> Option<u32> {
    let mut allocation = CrossDomainAllocation::new(resources, horizon, cadence).ok()?;
    for obligation in obligations.iter().cloned() {
        allocation.import(obligation);
    }
    allocation
        .resolve_planned(AllocationPersonality::default(), None, planning)
        .ok()
        .map(|settlement| settlement.residual_current_scrap())
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
    strategy: StrategicPlanner,
    team: TeamReliefPlanner,
    lifts: LiftPlanner,
    raids: RaidPlanner,
}

impl PlannerSnapshots {
    pub(crate) fn capture(
        strategy: &StrategicPlanner,
        team: &TeamReliefPlanner,
        lifts: &LiftPlanner,
        raids: &RaidPlanner,
    ) -> Self {
        Self {
            strategy: strategy.clone(),
            team: team.clone(),
            lifts: lifts.clone(),
            raids: raids.clone(),
        }
    }

    fn restore(self, participants: &mut AllocationParticipants<'_>) {
        restore_ownership(self.strategy, participants.strategy, |planner| {
            &mut planner.outcomes
        });
        restore_ownership(self.team, participants.team, |planner| {
            &mut planner.outcomes
        });
        restore_ownership(self.lifts, participants.lifts, |planner| {
            &mut planner.outcomes
        });
        restore_ownership(self.raids, participants.raids, |planner| {
            &mut planner.outcomes
        });
    }
}

fn restore_ownership<T>(
    mut snapshot: T,
    current: &mut T,
    outcomes: fn(&mut T) -> &mut crate::experience::OutcomeJournal,
) {
    // Commit adapters do not observe outcomes. Retained-work observations are
    // facts even when speculative capital or membership must be rolled back.
    *outcomes(&mut snapshot) = std::mem::take(outcomes(current));
    *current = snapshot;
}

/// Immutable evidence shared by every phase of one allocation pass.
pub(crate) struct AllocationSessionContext<'a> {
    pub(crate) evidence: crate::utility::DecisionEvidence<'a>,
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
    pub(crate) strategy: &'a mut StrategicPlanner,
    pub(crate) lifts: &'a mut LiftPlanner,
    pub(crate) team: &'a mut TeamReliefPlanner,
    pub(crate) raids: &'a mut RaidPlanner,
}

/// Named resource channels returned to the residual planners and trace.
#[derive(Debug, Default)]
pub(crate) struct AllocationBudgetOutcome {
    pub(crate) foundry_saving: u32,
    pub(crate) airworks_capacity: u32,
    pub(crate) opening_bootstrap: u32,
    pub(crate) raw_residual_scrap: u32,
    pub(crate) residual_scrap: u32,
    pub(crate) connected_spendable: u32,
    pub(crate) connected_forecast_hold: u32,
    pub(crate) utility_spendable: u32,
    pub(crate) prior_operation_spendable: u32,
    pub(crate) voluntary_scrap_guard: u32,
}

impl AllocationBudgetOutcome {
    fn frozen(foundry_saving: u32, opening_bootstrap: u32, voluntary_scrap_guard: u32) -> Self {
        Self {
            foundry_saving,
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
    pub(crate) fresh_defense_intents: Vec<Intent>,
    /// Commands maintaining observed work survive allocation failure.
    pub(crate) maintenance_intents: Vec<Intent>,
    pub(crate) fresh_economy_intents: Vec<Intent>,
    pub(crate) allocated_producer_intents: Vec<Intent>,
    pub(crate) allocation_ok: bool,
    pub(crate) accepted_connected: bool,
    pub(crate) producer_lane_reservations: ProducerLaneReservations,
    pub(crate) budget: AllocationBudgetOutcome,
    pub(crate) foundry_handoff: crate::utility::FoundryHandoff,
}

/// One typed allocation transaction over already-advanced legacy planners.
pub(crate) struct AllocationSession<'a> {
    observer: Option<&'a dyn crate::observer::PhaseObserver>,
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
            observer: None,
            context,
            participants,
            advanced,
            trace,
        }
    }

    pub(crate) fn with_observer(
        mut self,
        observer: Option<&'a dyn crate::observer::PhaseObserver>,
    ) -> Self {
        self.observer = observer;
        self
    }

    /// Observes retained work, then prepares, resolves, and commits or restores
    /// once. No domain is asked to rerank a payload after preparation.
    pub(crate) fn run(mut self) -> AllocationSessionOutcome {
        let _scope =
            crate::observer::PhaseScope::new(self.observer, crate::observer::BotPhase::Allocation);
        let observed = self.observe_retained_work();
        let snapshot_scope =
            crate::observer::PhaseScope::new(self.observer, crate::observer::BotPhase::Snapshot);
        let snapshots = CommitSnapshots {
            policy: self.participants.policy.speculative_checkpoint(),
        };
        drop(snapshot_scope);
        let prepared = self.prepare(observed);
        let resolved = self.resolve(prepared, snapshots);
        self.commit_or_restore(resolved)
    }

    fn retained_work(&mut self) -> RetainedWork<'_, 'a> {
        RetainedWork::new(&self.context, &mut self.participants, &mut self.advanced)
    }

    fn observe_retained_work(&mut self) -> ObservedAllocation {
        let initial_claims = snapshot_claims(&self.context, &self.participants);
        let resources = ResourceSnapshot::from_observation(self.context.observation);
        let observed_context = EconomicInvestmentContext {
            evidence: self.context.evidence,
            obligations: &[],
            obs: self.context.observation,
            resources: &resources,
            profile: self.context.profile,
            briefing: self.context.public_map,
            orientation: self.context.orientation,
            unavailable: &initial_claims.planner_claims,
            demands: &[],
            cadence: self.context.dials.cadence,
            unit_contacts: self.context.intelligence.units(),
            building_contacts: self.context.intelligence.buildings(),
            protected_scrap: 0,
            air_work: &[],
        };
        let recon_scope = crate::observer::PhaseScope::new(
            self.observer,
            crate::observer::BotPhase::Reconnaissance,
        );
        let recon_intents = self
            .participants
            .policy
            .observe_reconnaissance(observed_context, self.context.home);
        let recon_paid_exclusions = self
            .participants
            .policy
            .state
            .reconnaissance
            .paid_exclusions();
        drop(recon_scope);
        let support_scope =
            crate::observer::PhaseScope::new(self.observer, crate::observer::BotPhase::Support);
        let support_snapshot = self
            .participants
            .policy
            .support_work_snapshot(observed_context);
        self.participants
            .policy
            .observe_support_work(&support_snapshot, self.context.observation.tick);
        let support_intents = self
            .participants
            .policy
            .observe_support_deployments(observed_context, &support_snapshot.protection);
        drop(support_scope);
        let mut maintenance_intents = support_intents;
        maintenance_intents.extend(recon_intents);
        ObservedAllocation {
            resources,
            support_snapshot,
            recon_paid_exclusions,
            maintenance_intents,
        }
    }

    /// Imports exact prior work against the observed resource picture and asks
    /// each migrated domain for at most one already-ranked proposal.
    fn prepare(&mut self, observed: ObservedAllocation) -> PreparedAllocation {
        let ObservedAllocation {
            resources,
            support_snapshot,
            mut recon_paid_exclusions,
            maintenance_intents,
        } = observed;
        let RetainedPreparation {
            claims,
            mut obligations,
            mut saved,
            air_lift,
            active_revision,
            prospective_carrier_floor,
            emergency_defense,
        } = self
            .retained_work()
            .prepare(resources, &mut recon_paid_exclusions, &support_snapshot);
        let support = self.prepare_support(&claims, &obligations, &support_snapshot);
        let fresh_support_relief = self.prepare_support_relief(&claims);
        let fresh_support_deployments =
            if claims.opening_core.ready && self.participants.policy.economic_saving().is_none() {
                self.participants.policy.prepare_support_deployments(
                    EconomicInvestmentContext {
                        evidence: self.context.evidence,
                        obligations: &[],
                        obs: self.context.observation,
                        resources: &obligations.resources,
                        profile: self.context.profile,
                        briefing: self.context.public_map,
                        orientation: self.context.orientation,
                        unavailable: &claims.planner_claims,
                        demands: &[],
                        cadence: self.context.dials.cadence,
                        unit_contacts: self.context.intelligence.units(),
                        building_contacts: self.context.intelligence.buildings(),
                        protected_scrap: 0,
                        air_work: &[],
                    },
                    self.context.tuning,
                    self.context.dials.minimum_core_equivalents,
                )
            } else {
                vec![]
            };
        let recon_paid_unavailable = self.committed_standing_production();
        let operational_scout_queues = self.participants.strategy.reconnaissance_paid_claims(
            self.context.observation,
            &obligations.resources,
            &recon_paid_exclusions,
        );
        let raid_work =
            self.prepare_raid_procurement(&claims, &obligations, &recon_paid_unavailable);
        let mut recon_unavailable = claims.planner_claims.clone();
        if let Some(request) = &raid_work {
            recon_unavailable.extend_from_slice(&request.members);
        }

        recon_unavailable.extend(self.participants.team.reservations());

        let protection_work = self
            .participants
            .policy
            .discretionary_protection_work(self.context.observation.tick, self.context.tuning)
            .into_iter()
            .filter(|request| {
                request.missing > 0
                    && !fresh_support_deployments
                        .iter()
                        .any(|deployment| deployment.covers(request))
                    && !self
                        .participants
                        .policy
                        .state
                        .support_deployments
                        .active
                        .iter()
                        .any(|deployment| deployment.covers(request))
            })
            .take(self.context.tuning.attention_slots)
            .collect();
        self.participants.policy.state.reconnaissance.operational = self
            .participants
            .strategy
            .air_operation()
            .and_then(|operation| {
                if operation.phase == crate::strategy::AirOperationPhase::Recover {
                    return None;
                }
                Some(crate::utility::OperationalReconWork {
                    target: operation.target,
                    scout: operation.scout,
                    goal: operation.scout_dispatch.map(|(_, goal)| goal),
                    deadline: self.participants.strategy.air_capacity_deadline()?,
                    paid: operational_scout_queues.clone(),
                })
            });
        let fresh_reconnaissance = if self.context.dials.scouting {
            self.participants.policy.prepare_reconnaissance(
                EconomicInvestmentContext {
                    evidence: self.context.evidence,
                    obligations: &[],
                    obs: self.context.observation,
                    resources: &obligations.resources,
                    profile: self.context.profile,
                    briefing: self.context.public_map,
                    orientation: self.context.orientation,
                    unavailable: &recon_unavailable,
                    demands: &[],
                    cadence: self.context.dials.cadence,
                    unit_contacts: self.context.intelligence.units(),
                    building_contacts: self.context.intelligence.buildings(),
                    protected_scrap: 0,
                    air_work: &[],
                },
                self.context.tuning,
                self.context.dials.minimum_core_equivalents,
                claims.opening_core.ready && self.participants.policy.economic_saving().is_none(),
                &operational_scout_queues,
            )
        } else {
            Vec::new()
        };
        // Revision recovery can release imported claims after quote generation.
        let defense_admission_reserve = active_revision
            .defense_admission_reserve(air_lift.voluntary_scrap_guard, prospective_carrier_floor);
        let mut fresh = self.prepare_fresh_investments(
            &claims,
            &saved,
            &mut obligations,
            FreshInvestmentInputs {
                active_revision,
                defense_admission_reserve,
                recon_paid_exclusions: &recon_paid_exclusions,
                standing_force: StandingForceWork {
                    repair_work: support.repair_work,
                    protection_work,
                    raid: raid_work.filter(|request| {
                        request.missing > 0
                            || !request.newly_claimed.is_empty()
                            || !request.new_paid_claims().is_empty()
                    }),
                },
            },
        );
        if self
            .retained_work()
            .reconcile_revision(&mut saved, &air_lift, &mut obligations, &fresh)
        {
            fresh.connected = None;
            fresh.connected_accepted_at = None;
            fresh.connected_reserve_deadline = self
                .context
                .observation
                .tick
                .saturating_add(connected_preparation_horizon());
            let committed_production = self.committed_standing_production();
            let standing_force = self
                .standing_force_inputs(
                    &claims,
                    &obligations,
                    &fresh.standing_force_derivation,
                    &committed_production,
                )
                .derive(&claims.strategic_core_exclusions, &[]);
            fresh.standing_force = StandingForcePreparation::Unconditional(standing_force.0);
            fresh.economy.clear();
        }

        let allocation_horizon = allocation_horizon(
            &self.context,
            &self.participants,
            &saved,
            &fresh,
            obligations.active_connected.as_ref(),
        );

        PreparedAllocation {
            resources: obligations.resources,
            maintenance_intents,
            obligations: obligations.obligations,
            coordinator_failure: obligations.coordinator_failure,
            opening_core: claims.opening_core,
            allow_new_voluntary_operations: claims.opening_core.ready
                && self.participants.policy.economic_saving().is_none(),
            planner_claims: claims.planner_claims,
            strategic_core_exclusions: claims.strategic_core_exclusions,
            active_connected: obligations.active_connected,
            active_lift: obligations.active_lift,
            fresh_lift_producer_jobs: air_lift.fresh_lift_producer_jobs,
            saved_foundry: saved.obligation,
            fresh_foundry: fresh.foundry,
            fresh_defense: fresh.defense,
            fresh_economy: fresh.economy,
            fresh_support: support.assignments,
            fresh_support_construction: support.construction,
            fresh_support_relief,
            fresh_support_deployments,
            fresh_reconnaissance,
            fresh_connected: fresh.connected,
            standing_force: fresh.standing_force,
            connected_accepted_at: fresh.connected_accepted_at,
            connected_reserve_deadline: fresh.connected_reserve_deadline,
            allocation_horizon,
            active_lift_precedes_foundry: air_lift.active_lift_precedes_foundry,
            active_lift_spendable: air_lift.active_lift_spendable,
            foundry_saving: saved.saving,
            opening_bootstrap: air_lift.opening_bootstrap,
            voluntary_scrap_guard: air_lift.voluntary_scrap_guard,
            prospective_carrier_floor,
            rejected_connected_candidate: fresh.rejected_connected_candidate,
            staged_strategy: obligations.staged_strategy,
            emergency_defense,
            team_decision: core::mem::take(&mut self.advanced.team_decision),
            lift_decision: air_lift.lift_decision,
            raid_decision: core::mem::take(&mut self.advanced.raid_decision),
        }
    }

    fn prepare_support(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &ObligationPreparation,
        support_snapshot: &SupportWorkSnapshot,
    ) -> SupportPreparation {
        let context = support_context(&self.context, claims, &obligations.resources);
        let allow_repair = claims.opening_core.ready && self.context.dials.repair;
        if allow_repair
            && strategic_admission_tick(self.context.observation.tick)
            && self.participants.policy.economic_saving().is_none()
        {
            let fresh = self.participants.policy.discretionary_support_work(
                support_snapshot,
                self.context.observation.tick,
                self.context.tuning,
            );
            SupportPreparation {
                repair_work: fresh.unit_work(),
                assignments: self
                    .participants
                    .policy
                    .prepared_repair_assignments(context, &fresh),
                construction: self
                    .participants
                    .policy
                    .prepared_repair_bays(context, &fresh),
            }
        } else {
            SupportPreparation::default()
        }
    }

    fn prepare_support_relief(
        &mut self,
        claims: &ClaimSnapshot,
    ) -> Option<crate::team::TeamReliefOperation> {
        self.participants
            .team
            .prepare_relief(crate::team::TeamReliefPreparation {
                tuning: self.context.tuning,
                obs: self.context.observation,
                home: self.context.home,
                admission: crate::team::TeamReliefAdmission {
                    additionally_reserved: &claims.planner_claims,
                    allow_new_operation: claims.opening_core.ready
                        && self.participants.policy.economic_saving().is_none()
                        && self.context.tuning.attention_slots > 0,
                    core_reservations: &claims.strategic_core_exclusions,
                    minimum_core_equivalents: u64::from(
                        self.context.dials.minimum_core_equivalents,
                    ),
                },
                map: self.context.public_map,
                orientation: self.context.orientation,
            })
    }

    fn prepare_fresh_investments(
        &mut self,
        claims: &ClaimSnapshot,
        saved: &SavedFoundryPreparation,
        obligations: &mut ObligationPreparation,
        inputs: FreshInvestmentInputs<'_>,
    ) -> FreshInvestmentPreparation {
        let FreshInvestmentInputs {
            active_revision,
            defense_admission_reserve,
            recon_paid_exclusions,
            standing_force: work,
        } = inputs;
        let admission_tick = strategic_admission_tick(self.context.observation.tick)
            && claims.opening_core.ready
            && self.participants.policy.economic_saving().is_none()
            && !saved.blocked
            && obligations.coordinator_failure.is_none();
        let available_builders =
            available_allocation_builders(&obligations.resources, &obligations.obligations);
        let available_builder_units = self
            .context
            .observation
            .my_units
            .iter()
            .filter(|unit| available_builders.binary_search(&unit.id).is_ok())
            .collect::<Vec<_>>();
        let foundry_scope =
            crate::observer::PhaseScope::new(self.observer, crate::observer::BotPhase::Foundry);
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
        drop(foundry_scope);
        let expansion_security_need = foundry_investment
            .as_ref()
            .and_then(FreshFoundryInvestment::preparation_need)
            .or(saved.preparation_need);
        let mut foundry = match foundry_investment {
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
            match self.participants.strategy.fresh_connected_minimum_proposal(
                FreshConnectedProposalRequest::new(
                    self.context.profile,
                    self.context.tuning,
                    self.context.observation,
                    &obligations.resources,
                    self.context.intelligence,
                    self.context.home,
                    StrategicCoordination {
                        planning: Some(&self.participants.policy.planning),
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
                )
                .with_paid_exclusions(recon_paid_exclusions),
            ) {
                Ok(proposal) => proposal,
                Err(rejected) => {
                    rejected_connected_candidate = Some(rejected);
                    None
                }
            }
        } else {
            None
        };
        let connected = connected_candidate;
        let defense_reinforcement_exclusions =
            defense_reinforcement_exclusions(&claims.strategic_core_exclusions, connected.as_ref());
        let connected_accepted_at = obligations
            .active_connected
            .as_ref()
            .map(ActiveConnectedObligation::accepted_at)
            .or_else(|| connected.as_ref().map(FreshConnectedProposal::accepted_at));
        if connected.is_none() {
            self.retained_work().retain_legacy_air(obligations);
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
        let saved_layout_allows_defense = saved.obligation.is_none_or(|foundry| {
            !UtilityPolicy::build_layout_covers_assigned_builder(
                self.context.observation,
                &[(BuildingKind::Foundry, foundry.anchor(), foundry.builder())],
            )
        });
        let defense_scope =
            crate::observer::PhaseScope::new(self.observer, crate::observer::BotPhase::Defense);
        let mut defense = if admission_tick && saved_layout_allows_defense {
            self.participants.policy.fresh_defense_proposals(
                self.context.profile,
                self.context.observation,
                &obligations.resources,
                self.context.public_map,
                self.context.orientation,
                self.context.home,
                self.context.intelligence.units(),
                self.context.intelligence.buildings(),
                &available_builder_units,
                &defense_reinforcement_exclusions,
                0,
                fixed_production_current_reserve(&obligations.resources, &obligations.obligations),
                defense_admission_reserve,
                self.context.evidence,
            )
        } else {
            Vec::new()
        };
        if let Some(saved_foundry) = saved.obligation {
            defense.retain(|proposal| {
                self.participants
                    .policy
                    .combined_build_layout_with_builders_is_safe(
                        self.context.observation,
                        self.context.public_map,
                        self.context.intelligence.units(),
                        self.context.intelligence.buildings(),
                        self.context.orientation,
                        &[
                            (
                                BuildingKind::Foundry,
                                saved_foundry.anchor(),
                                saved_foundry.builder(),
                            ),
                            (proposal.kind(), proposal.anchor(), proposal.builder()),
                        ],
                    )
            });
        }
        drop(defense_scope);
        let standing_force_derivation = StandingForceDerivation {
            projection_targets: standing_force_projection_targets(
                &self.context,
                &self.participants,
            ),
            expansion_security_need,
            work,
        };
        let (standing_force, capability_demands) = self
            .standing_force_inputs(
                claims,
                obligations,
                &standing_force_derivation,
                &committed_production,
            )
            .prepare(&claims.strategic_core_exclusions, connected.as_ref());
        let unavailable_economy_workers = self
            .context
            .observation
            .my_units
            .iter()
            .filter(|unit| !available_builders.contains(&unit.id))
            .map(|unit| unit.id)
            .collect::<Vec<_>>();
        let air_work = economic_air_work(
            &self.context,
            &self.participants,
            &self.advanced.lift_unavailable,
        );
        let economy_scope =
            crate::observer::PhaseScope::new(self.observer, crate::observer::BotPhase::Economy);
        let economy = if admission_tick
            && claims.opening_core.ready
            && self.participants.policy.economic_saving().is_none()
        {
            let economic_context = EconomicInvestmentContext {
                evidence: self.context.evidence,
                obligations: &obligations.obligations,
                obs: self.context.observation,
                resources: &obligations.resources,
                profile: self.context.profile,
                briefing: self.context.public_map,
                orientation: self.context.orientation,
                unavailable: &unavailable_economy_workers,
                demands: &capability_demands,
                air_work: &air_work,
                cadence: self.context.dials.cadence,
                unit_contacts: self.context.intelligence.units(),
                building_contacts: self.context.intelligence.buildings(),
                protected_scrap: current_reserve_at(
                    &obligations.obligations,
                    self.context.observation.tick,
                )
                .saturating_add(
                    self.participants.policy.shallow_sentinel_capital_reserve(
                        self.context.dials,
                        self.context.observation,
                        self.context.home,
                        self.context.public_map,
                        &[],
                    ),
                ),
            };
            let mut economic_quotes = self.participants.policy.economic_quotes(economic_context);
            if foundry.is_none()
                && let Some(FreshFoundryInvestment::Ready(proposal)) = economic_quotes
                    .capacity_foundry(
                        self.context.dials,
                        FreshFoundryProposalContext {
                            home: self.context.home,
                            available_builders: &available_builders,
                            combat_core_exclusions: &claims.strategic_core_exclusions,
                            unit_contacts: self.context.intelligence.units(),
                            building_contacts: self.context.intelligence.buildings(),
                            public_map: self.context.public_map,
                            same_think_intents: &self.advanced.team_decision.intents,
                            current_scrap: self.context.observation.scrap,
                            protected_reserve: economic_context.protected_scrap,
                        },
                    )
            {
                foundry = Some(proposal);
            }
            economic_quotes.investments()
        } else {
            Vec::new()
        };
        drop(economy_scope);
        FreshInvestmentPreparation {
            foundry,
            defense,
            economy,
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

        committed.extend(
            self.participants
                .raids
                .paid_claims()
                .iter()
                .map(|claim| StandingProductionCommitment::paid(claim.producer, claim.kind)),
        );

        committed.extend(
            self.participants
                .strategy
                .paid_connected_production(self.context.observation)
                .into_iter()
                .map(|assignment| {
                    StandingProductionCommitment::paid(assignment.producer(), assignment.kind())
                }),
        );

        committed.extend(
            self.participants
                .lifts
                .issued_production_assignments(self.context.observation.tick)
                .into_iter()
                .map(|assignment| {
                    StandingProductionCommitment::paid(assignment.producer(), assignment.kind())
                }),
        );

        committed
    }

    fn standing_force_inputs<'b>(
        &'b self,
        claims: &ClaimSnapshot,
        obligations: &'b ObligationPreparation,
        derivation: &'b StandingForceDerivation,
        committed_production: &'b [StandingProductionCommitment],
    ) -> StandingForceInputs<'b> {
        StandingForceInputs {
            observer: self.observer,
            observation: self.context.observation,
            intelligence: self.context.intelligence,
            profile: self.context.profile,
            tuning: self.context.tuning,
            resources: &obligations.resources,
            home: self.context.home,
            public_map: self.context.public_map,
            orientation: self.context.orientation,
            eligible: claims.opening_core.ready
                && obligations.coordinator_failure.is_none()
                && (self.participants.policy.economic_saving().is_none()
                    || self
                        .context
                        .observation
                        .my_buildings
                        .iter()
                        .any(|building| {
                            building.kind == BuildingKind::Fabricator && building.built
                        })),
            derivation,
            committed_production,
            funded_repairers: self
                .participants
                .policy
                .state
                .support_work
                .repairs
                .iter()
                .map(|repair| repair.key.worker)
                .collect(),
            saving: self.participants.policy.state.standing_saving.as_ref(),
            recon_demands: self
                .participants
                .policy
                .state
                .reconnaissance
                .capability_demands(),
        }
    }

    fn prepare_raid_procurement(
        &self,
        claims: &ClaimSnapshot,
        obligations: &ObligationPreparation,
        committed_production: &[StandingProductionCommitment],
    ) -> Option<crate::raid::RaidProcurementRequest> {
        let load = usize::from(self.participants.strategy.air_operation().is_some())
            + usize::from(self.participants.team.operation().is_some())
            + usize::from(self.participants.lifts.operation().is_some());
        let admitted = claims.opening_core.ready
            && obligations.coordinator_failure.is_none()
            && self.participants.policy.economic_saving().is_none()
            && (load == 0 || self.context.tuning.attention_slots >= (load + 1) * 2);
        self.participants.raids.muster_request(
            crate::raid::RaidPlanningContext::new(
                self.context.profile,
                self.context.tuning,
                self.context.observation,
                self.context.home,
                self.context.enlisted,
                &claims.planner_claims,
            )
            .with_admission(admitted)
            .with_paid_production(committed_production),
            &obligations.resources,
            Some(self.context.public_map),
            Some(self.context.orientation),
        )
    }

    /// Resolves the complete prepared portfolio once. Domain payloads stay
    /// opaque and retain their exact original ranking and identity.
    fn resolve(
        &mut self,
        mut prepared: PreparedAllocation,
        snapshots: CommitSnapshots,
    ) -> ResolvedAllocation {
        let _scope =
            crate::observer::PhaseScope::new(self.observer, crate::observer::BotPhase::Portfolio);
        let settlement = self.resolve_portfolio(&mut prepared);
        ResolvedAllocation {
            prepared,
            settlement,
            snapshots,
        }
    }

    fn resolve_portfolio(
        &mut self,
        prepared: &mut PreparedAllocation,
    ) -> Result<CrossDomainSettlement, AllocationFailure> {
        if let Some(failure) = prepared.coordinator_failure.take() {
            return Err(AllocationFailure::Coordinator(failure));
        }
        let revises_active = prepared
            .fresh_connected
            .as_ref()
            .is_some_and(FreshConnectedProposal::revises_active_operation);
        let mut allocation = CrossDomainAllocation::new(
            &prepared.resources,
            prepared.allocation_horizon,
            self.context.dials.cadence,
        )
        .map_err(|error| {
            AllocationFailure::Coordinator((
                AllocationCoordinatorStageTrace::CapacityProjection,
                (&error).into(),
            ))
        })?;
        let layouts = Self::fresh_layouts(prepared);
        let allocatable_voluntary_scrap_guard = prepared
            .voluntary_scrap_guard
            .min(prepared.resources.current_scrap().amount());
        let active_revision_voluntary_scrap_guard = prepared
            .fresh_connected
            .as_ref()
            .filter(|proposal| proposal.revises_active_operation())
            .and_then(|_| {
                residual_current_after_obligations(
                    &prepared.resources,
                    &prepared.obligations,
                    prepared.allocation_horizon,
                    self.context.dials.cadence,
                    &self.participants.policy.planning,
                )
            })
            .map_or(allocatable_voluntary_scrap_guard, |residual| {
                residual.min(allocatable_voluntary_scrap_guard)
            });
        for obligation in prepared.obligations.iter().cloned() {
            allocation.import(obligation);
        }
        for (rank, recon) in prepared.fresh_reconnaissance.iter().cloned().enumerate() {
            allocation.offer(
                super::reconnaissance_investment_proposal(recon)
                    .with_domain_preference(rank)
                    .with_personality_preference(u16::from(self.context.profile.traits.guile))
                    .with_minimum_residual_scrap(prepared.prospective_carrier_floor)
                    .with_voluntary_scrap_guard(allocatable_voluntary_scrap_guard),
            );
        }
        for (rank, repair) in prepared.fresh_support.iter().cloned().enumerate() {
            allocation.offer(
                super::InvestmentProposal::fresh(
                    ProposalKey::Support(repair.key),
                    repair.case,
                    repair.claims(false),
                    super::DomainPayload::Support(repair),
                )
                .with_domain_preference(rank)
                .with_voluntary_scrap_guard(allocatable_voluntary_scrap_guard),
            );
        }
        for (rank, deployment) in prepared
            .fresh_support_deployments
            .iter()
            .cloned()
            .enumerate()
        {
            allocation.offer(
                super::InvestmentProposal::fresh(
                    ProposalKey::SupportDeployment(deployment.key),
                    deployment.case(),
                    deployment.claims(),
                    super::DomainPayload::SupportDeployment(deployment),
                )
                .with_domain_preference(rank)
                .with_voluntary_scrap_guard(allocatable_voluntary_scrap_guard),
            );
        }
        if let Some(relief) = prepared.fresh_support_relief.clone() {
            allocation.offer(
                super::InvestmentProposal::fresh(
                    ProposalKey::SupportRelief(relief.foundry),
                    super::ProposalCase {
                        urgency: super::Urgency::Pressing,
                        confidence: super::Confidence::Current,
                        value: super::StrategicValue::Decisive,
                        time_to_impact: super::TimeToImpact::Near,
                        safety: super::ExecutionSafety::Managed,
                    },
                    ClaimBundle::new(0, vec![], vec![], relief.members.clone(), vec![], vec![])
                        .expect("frozen relief members are canonical"),
                    super::DomainPayload::SupportRelief(relief),
                )
                .with_personality_preference(u16::from(self.context.profile.traits.support)),
            );
        }
        for (rank, bay) in prepared
            .fresh_support_construction
            .iter()
            .cloned()
            .enumerate()
        {
            let claims = economic_investment_claims(&bay)
                .expect("a fully funded Bay has one builder and site");
            allocation.offer(
                super::InvestmentProposal::fresh(
                    ProposalKey::SupportConstruction(bay.key),
                    bay.case,
                    claims,
                    super::DomainPayload::SupportConstruction(bay),
                )
                .with_domain_preference(rank)
                .with_voluntary_scrap_guard(allocatable_voluntary_scrap_guard),
            );
        }
        for (rank, proposal) in prepared.fresh_economy.iter().cloned().enumerate() {
            match economic_investment_proposal(proposal) {
                Ok(proposal) => allocation.offer(
                    proposal
                        .with_domain_preference(rank)
                        .with_voluntary_scrap_guard(allocatable_voluntary_scrap_guard)
                        .with_minimum_residual_scrap(prepared.prospective_carrier_floor),
                ),
                Err(error) => {
                    return Err(AllocationFailure::Coordinator((
                        AllocationCoordinatorStageTrace::EconomyProposalAdaptation,
                        error.into(),
                    )));
                }
            }
        }
        if let Some(proposal) = prepared.fresh_foundry.take() {
            match foundry_investment_proposal(proposal) {
                Ok(proposal) => allocation.offer(
                    proposal
                        .with_voluntary_scrap_guard(allocatable_voluntary_scrap_guard)
                        .with_minimum_residual_scrap(prepared.prospective_carrier_floor),
                ),
                Err(error) => {
                    return Err(AllocationFailure::Coordinator((
                        AllocationCoordinatorStageTrace::FoundryProposalAdaptation,
                        error.into(),
                    )));
                }
            }
        }
        match defense_investment_proposals(core::mem::take(&mut prepared.fresh_defense)) {
            Ok(proposals) => {
                for proposal in proposals {
                    allocation.offer(
                        proposal
                            .with_voluntary_scrap_guard(allocatable_voluntary_scrap_guard)
                            .with_minimum_residual_scrap(prepared.prospective_carrier_floor),
                    );
                }
            }
            Err(error) => {
                return Err(AllocationFailure::Coordinator((
                    AllocationCoordinatorStageTrace::DefenseProposalAdaptation,
                    error.into(),
                )));
            }
        }
        if let Some(proposal) = prepared.fresh_connected.take() {
            if proposal.revises_active_operation() {
                allocation.offer(
                    active_connected_revision_investment_proposal(proposal)
                        .with_voluntary_scrap_guard(active_revision_voluntary_scrap_guard)
                        .with_minimum_residual_scrap(prepared.prospective_carrier_floor),
                );
            } else {
                match connected_investment_proposal(proposal) {
                    Ok(proposal) => allocation.offer(
                        proposal
                            .with_voluntary_scrap_guard(allocatable_voluntary_scrap_guard)
                            .with_minimum_residual_scrap(prepared.prospective_carrier_floor),
                    ),
                    Err(error) => {
                        return Err(AllocationFailure::Coordinator((
                            AllocationCoordinatorStageTrace::ConnectedProposalAdaptation,
                            error.into(),
                        )));
                    }
                }
            }
        }
        match core::mem::take(&mut prepared.standing_force) {
            StandingForcePreparation::Unconditional(standing_force) => {
                match standing_force_investment_proposals(standing_force) {
                    Ok(proposals) => {
                        for proposal in proposals {
                            allocation.offer(
                                standing_force_with_voluntary_guard(
                                    proposal,
                                    allocatable_voluntary_scrap_guard,
                                )
                                .with_minimum_residual_scrap(prepared.prospective_carrier_floor),
                            );
                        }
                    }
                    Err(error) => {
                        return Err(AllocationFailure::Coordinator((
                            AllocationCoordinatorStageTrace::StandingForceProposalAdaptation,
                            error.into(),
                        )));
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
                                    .with_minimum_residual_scrap(prepared.prospective_carrier_floor)
                                })
                                .collect(),
                        ),
                        Err(error) => {
                            return Err(AllocationFailure::Coordinator((
                                AllocationCoordinatorStageTrace::StandingForceProposalAdaptation,
                                error.into(),
                            )));
                        }
                    }
                }
            }
        }
        allocation.apply_experience(
            self.context.evidence.experience,
            self.context.observation.tick,
        );
        let mut checked = std::collections::BTreeMap::new();
        let policy = &*self.participants.policy;
        let context = &self.context;
        let observer = self.observer;
        let mut validate_layout = |selected: &[ProposalKey]| {
            let selected_layouts = layouts
                .iter()
                .filter(|(key, _)| selected.contains(key))
                .collect::<Vec<_>>();
            if selected_layouts.len() < 2 {
                return None;
            }
            let keys = selected_layouts
                .iter()
                .map(|(key, _)| *key)
                .collect::<Vec<_>>();
            let safe = *checked.entry(keys.clone()).or_insert_with(|| {
                let _scope =
                    crate::observer::PhaseScope::new(observer, crate::observer::BotPhase::Layouts);
                policy.combined_build_layout_with_builders_is_safe(
                    context.observation,
                    context.public_map,
                    context.intelligence.units(),
                    context.intelligence.buildings(),
                    context.orientation,
                    &selected_layouts
                        .iter()
                        .map(|(_, build)| *build)
                        .collect::<Vec<_>>(),
                )
            });
            if safe {
                None
            } else {
                super::IncompatibleLayoutSet::from_keys(keys)
            }
        };
        match allocation.resolve_validated(
            AllocationPersonality::from_profile(self.context.profile),
            self.trace.as_deref_mut(),
            &mut validate_layout,
            &policy.planning,
        ) {
            Ok(settlement) => Ok(settlement),
            Err(AllocationError::Deferred) => self
                .resolve_committed(prepared, revises_active)
                .ok_or(AllocationFailure::Unsettled),
            Err(_) => Err(AllocationFailure::Unsettled),
        }
    }

    fn resolve_committed(
        &mut self,
        prepared: &mut PreparedAllocation,
        revises_active: bool,
    ) -> Option<CrossDomainSettlement> {
        if revises_active {
            remove_active_connected_obligation(&mut prepared.obligations);
            prepared.active_connected = {
                self.participants.strategy.active_connected_obligation(
                    FreshConnectedProposalRequest::new(
                        self.context.profile,
                        self.context.tuning,
                        self.context.observation,
                        &prepared.resources,
                        self.context.intelligence,
                        self.context.home,
                        StrategicCoordination {
                            planning: Some(&self.participants.policy.planning),
                            enlisted: &prepared.planner_claims,
                            lift_support: None,
                            allow_new_operation: false,
                            protected_current_scrap: 0,
                            protected_forecast_scrap: 0,
                            public_map: Some(self.context.public_map),
                            orientation: self.context.orientation,
                        },
                    )
                    .with_paid_exclusions(
                        &self
                            .participants
                            .policy
                            .state
                            .reconnaissance
                            .paid_exclusions(),
                    ),
                )
            };
            if let Some(active) = &prepared.active_connected {
                prepared
                    .obligations
                    .push(active_connected_obligation(active).ok()?);
            }
        }
        if prepared.obligations.iter().any(|obligation| {
            matches!(obligation.key, ObligationKey::ConnectedOffense { .. })
                && !obligation.claims.producer_jobs().is_empty()
        }) {
            return None;
        }
        prepared.obligations.retain(|obligation| {
            obligation
                .claims
                .producer_jobs()
                .iter()
                .all(|job| job.fixed_assignment().is_some())
        });
        prepared.fresh_lift_producer_jobs = 0;
        prepared.fresh_connected = None;
        let mut allocation = CrossDomainAllocation::new(
            &prepared.resources,
            prepared.allocation_horizon,
            self.context.dials.cadence,
        )
        .ok()?;
        for obligation in &prepared.obligations {
            allocation.import(obligation.clone());
        }
        allocation
            .resolve_planned(
                AllocationPersonality::from_profile(self.context.profile),
                self.trace.as_deref_mut(),
                &self.participants.policy.planning,
            )
            .ok()
    }

    fn fresh_layouts(
        prepared: &PreparedAllocation,
    ) -> Vec<(ProposalKey, (BuildingKind, TilePos, UnitId))> {
        let mut layouts = Vec::new();
        if let Some(foundry) = prepared.fresh_foundry.as_ref() {
            layouts.push((
                ProposalKey::FoundryExpansion(super::FoundryExpansionKey {
                    anchor: foundry.anchor(),
                }),
                (BuildingKind::Foundry, foundry.anchor(), foundry.builder()),
            ));
        }
        layouts.extend(prepared.fresh_defense.iter().map(|defense| {
            (
                ProposalKey::Defense(DefenseInvestmentKey {
                    kind: defense.kind(),
                    anchor: defense.anchor(),
                }),
                (defense.kind(), defense.anchor(), defense.builder()),
            )
        }));
        layouts.extend(prepared.fresh_economy.iter().filter_map(|economy| {
            economy
                .build()
                .map(|build| (ProposalKey::Economy(economy.key), build))
        }));
        layouts.extend(
            prepared
                .fresh_support_construction
                .iter()
                .filter_map(|bay| {
                    bay.build()
                        .map(|build| (ProposalKey::SupportConstruction(bay.key), build))
                }),
        );
        layouts.sort_unstable_by_key(|(key, _)| *key);
        layouts
    }

    fn commit_settlement(
        &mut self,
        prepared: &mut PreparedAllocation,
        settlement: CrossDomainSettlement,
    ) -> Result<CommitEffects, CoordinatorFailure> {
        let mut effects = CommitEffects::frozen(prepared);
        let producer_schedule = settlement.producer_schedule().to_vec();
        let voluntary_scrap_guard = if settlement.voluntary_scrap_guard_satisfied() {
            0
        } else {
            prepared.voluntary_scrap_guard
        };
        effects.budget.voluntary_scrap_guard = voluntary_scrap_guard;
        effects.budget.raw_residual_scrap = settlement.residual_current_scrap();
        effects.budget.residual_scrap = effects
            .budget
            .raw_residual_scrap
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
        if self.participants.policy.state.standing_saving.as_ref().is_some_and(|saving|
            producer_schedule.iter().any(|job| job.enqueued_at == self.context.observation.tick
                && matches!(job.owner, ClaimOwner::Obligation { key: ObligationKey::StandingForceSaving(key), .. }
                    if key == saving.proposal.key()))) {
            self.participants.policy.state.standing_saving = None;
        }

        if let Some(saved) = self.participants.policy.economic_saving().cloned() {
            let current = settlement
                .capital_assignment(ClaimOwner::Obligation {
                    class: ObligationClass::PersistentPlan,
                    accepted_at: saved.observed_at,
                    key: ObligationKey::SavedEconomy(saved.key),
                })
                .expect("retained economic capital is allocated")
                .current_scrap;
            self.participants.policy.commit_economic_investment(
                saved,
                current,
                &mut effects.fresh_economy_intents,
            );
        }

        self.commit_emergency_defense(prepared, &mut effects);
        self.bind_saved_foundry_funding(prepared, &settlement)?;
        self.refresh_and_bind_lift(prepared, &producer_schedule)?;
        let mut payloads = settlement.into_payloads();
        self.dispatch_ready_saved_foundry(prepared, &mut effects)?;
        self.commit_fresh_connected(&mut payloads, &mut effects)?;
        self.participants
            .strategy
            .record_connected_purchases(&producer_schedule, self.context.observation.tick);
        self.commit_fresh_foundry(prepared, &mut payloads, &mut effects)?;
        for job in &producer_schedule {
            if let ClaimOwner::Obligation {
                key: ObligationKey::Reconnaissance(key),
                ..
            } = job.owner
                && !self.participants.policy.bind_reconnaissance_funding(
                    key,
                    *job,
                    self.context.observation,
                )
            {
                return Err((
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
                ));
            }
        }
        if let Some(recon) = payloads.take_reconnaissance() {
            let funding = producer_schedule
                .iter()
                .find(|job| {
                    job.owner == ClaimOwner::Proposal(ProposalKey::Reconnaissance(recon.key()))
                })
                .copied();
            let scheduled = match recon.observer {
                crate::utility::ReconObserver::Live(_) => true,
                crate::utility::ReconObserver::Queued { .. } => true,
                crate::utility::ReconObserver::Purchase { producer, kind, .. } => {
                    producer_schedule.iter().any(|job| {
                        job.owner == ClaimOwner::Proposal(ProposalKey::Reconnaissance(recon.key()))
                            && job.producer == producer
                            && job.kind == kind
                            && job.ready_at < recon.funding_deadline()
                    })
                }
            };
            if !scheduled
                || !self.participants.policy.commit_reconnaissance(
                    recon,
                    funding,
                    self.context.observation,
                    &mut effects.fresh_economy_intents,
                )
            {
                return Err((
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
                ));
            }
        }
        if let Some(economy) = payloads.take_economy() {
            let current = economy.current_capital;
            self.participants.policy.commit_economic_investment(
                economy,
                current,
                &mut effects.fresh_economy_intents,
            );
        }
        if let Some(deployment) = payloads.take_support_deployment()
            && !self.participants.policy.commit_support_deployment(
                deployment,
                self.context.observation,
                &mut effects.fresh_economy_intents,
            )
        {
            return Err((
                AllocationCoordinatorStageTrace::ObligationCollection,
                AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
            ));
        }
        if let Some(defense) = payloads.take_defense() {
            self.participants
                .policy
                .commit_adjudicated_defense(defense, &mut effects.fresh_defense_intents);
        }
        if let Some(repair) = payloads.take_support()
            && !self.participants.policy.commit_repair_assignment(
                repair,
                self.context.observation,
                &mut effects.fresh_economy_intents,
            )
        {
            return Err((
                AllocationCoordinatorStageTrace::ObligationCollection,
                AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
            ));
        }
        if let Some(standing_force) = payloads.take_standing_force() {
            if standing_force.accumulation().is_some()
                && let Some(job) = producer_schedule.iter().find(|job| {
                    job.owner
                        == ClaimOwner::Proposal(ProposalKey::StandingForce(standing_force.key()))
                        && job.enqueued_at > self.context.observation.tick
                })
            {
                self.participants.policy.state.standing_saving =
                    Some(crate::standing_force::StandingForceCommitment {
                        proposal: standing_force.clone(),
                        job: *job,
                    });
            }
            let scheduled = producer_schedule.iter().any(|job| {
                job.owner == ClaimOwner::Proposal(ProposalKey::StandingForce(standing_force.key()))
                    && job.enqueued_at == self.context.observation.tick
                    && job.kind == standing_force.key_kind()
            });
            debug_assert!(
                standing_force.accumulation().is_some()
                    || scheduled
                        == standing_force
                            .raid
                            .as_ref()
                            .is_none_or(|raid| raid.missing > 0)
            );
            if let Some(request) = standing_force.raid {
                let count = producer_schedule
                    .iter()
                    .filter(|job| {
                        job.owner
                            == ClaimOwner::Proposal(ProposalKey::StandingForce(
                                crate::allocation::StandingForceKey {
                                    kind: UnitKind::Scuttler,
                                    service: StandingGroundTarget::point(request.tile),
                                },
                            ))
                            && job.enqueued_at == self.context.observation.tick
                    })
                    .count();
                if count != request.missing
                    || !{
                        self.participants
                            .raids
                            .commit_procurement(request, self.context.observation.tick)
                            && self.participants.raids.bind_procurement(
                                self.context.observation,
                                &producer_schedule,
                                self.context.public_map,
                                self.context.orientation,
                            )
                    }
                {
                    return Err((
                        AllocationCoordinatorStageTrace::ObligationCollection,
                        AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
                    ));
                }
            }
        }
        if let Some(support) = payloads.take_support_procurement() {
            let scheduled = producer_schedule.iter().any(|job| {
                job.owner == ClaimOwner::Proposal(ProposalKey::SupportProcurement(support.key()))
                    && job.enqueued_at == self.context.observation.tick
                    && job.kind == support.key_kind()
            });
            debug_assert_eq!(scheduled, support.accumulation().is_none());
        }
        if let Some(bay) = payloads.take_support_construction() {
            effects.fresh_economy_intents.push(bay.intent());
        }
        if let Some(relief) = payloads.take_support_relief() {
            if let Some(decision) = { self.participants.team.commit_relief(relief) } {
                prepared.team_decision = decision;
            } else {
                return Err((
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
                ));
            }
        }
        self.participants
            .policy
            .bind_reconnaissance_queue_order(&producer_schedule, self.context.observation);
        Ok(effects)
    }

    fn refresh_and_bind_lift(
        &mut self,
        prepared: &PreparedAllocation,
        producer_schedule: &[super::ScheduledProducerJob],
    ) -> Result<(), CoordinatorFailure> {
        if prepared.active_lift.is_none() && prepared.fresh_lift_producer_jobs == 0 {
            return Ok(());
        }
        let planner = &mut *self.participants.lifts;
        let mut due_ordinals = Vec::new();
        if let Some(active) = prepared.active_lift.as_ref() {
            let assignments = active_lift_producer_assignments(active, producer_schedule);
            if planner
                .refresh_active_production_funding(active, &assignments)
                .is_err()
            {
                return Err((
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
                ));
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
                return Err((
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
                ));
            }
        }
        planner.mark_producers_issued(&due_ordinals);
        Ok(())
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
    ) -> Result<(), CoordinatorFailure> {
        let Some(saved) = prepared.saved_foundry else {
            return Ok(());
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
            return Err((
                AllocationCoordinatorStageTrace::SavedFoundryDispatch,
                AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
            ));
        }
        Ok(())
    }

    fn dispatch_ready_saved_foundry(
        &mut self,
        prepared: &PreparedAllocation,
        effects: &mut CommitEffects,
    ) -> Result<(), CoordinatorFailure> {
        if let Some(saved) = prepared.saved_foundry
            && saved.ready_to_build()
            && !self
                .participants
                .policy
                .dispatch_validated_foundry(saved, &mut effects.fresh_foundry_intents)
        {
            return Err((
                AllocationCoordinatorStageTrace::SavedFoundryDispatch,
                AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
            ));
        }
        Ok(())
    }

    fn commit_fresh_connected(
        &mut self,
        payloads: &mut super::AcceptedDomainPayloads,
        effects: &mut CommitEffects,
    ) -> Result<(), CoordinatorFailure> {
        let Some(connected) = payloads.take_connected() else {
            return Ok(());
        };
        let planner = &mut *self.participants.strategy;
        planner
            .commit_connected_proposal(connected)
            .map_err(|error| {
                (
                    AllocationCoordinatorStageTrace::ConnectedProposalCommit,
                    error.into(),
                )
            })?;
        effects.accepted_connected = true;
        Ok(())
    }

    fn commit_fresh_foundry(
        &mut self,
        prepared: &PreparedAllocation,
        payloads: &mut super::AcceptedDomainPayloads,
        effects: &mut CommitEffects,
    ) -> Result<(), CoordinatorFailure> {
        let Some(foundry) = payloads.take_foundry() else {
            return Ok(());
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
            return Err((
                AllocationCoordinatorStageTrace::FoundryProposalCommit,
                AllocationCoordinatorFailureReasonTrace::ExistingFoundryCommitment,
            ));
        }
        Ok(())
    }

    /// Applies every exact selected payload, or restores every participant to
    /// the transaction snapshots when any adaptation or dispatch fails.
    fn commit_or_restore(mut self, resolved: ResolvedAllocation) -> AllocationSessionOutcome {
        let ResolvedAllocation {
            mut prepared,
            settlement,
            snapshots,
        } = resolved;
        let committed = settlement.and_then(|settlement| {
            self.commit_settlement(&mut prepared, settlement)
                .map_err(AllocationFailure::Coordinator)
        });
        let allocation_ok = committed.is_ok();
        let effects = match committed {
            Ok(effects) => effects,
            Err(failure) => {
                if let AllocationFailure::Coordinator((stage, reason)) = failure
                    && let Some(trace) = self.trace.as_deref_mut()
                {
                    trace.record_coordinator_failure(stage, reason);
                }
                CommitEffects::frozen(&prepared)
            }
        };

        let mut planner_claims = core::mem::take(&mut prepared.planner_claims);
        let mut strategic_core_exclusions =
            core::mem::take(&mut prepared.strategic_core_exclusions);
        let mut team_decision = core::mem::take(&mut prepared.team_decision);
        let mut lift_decision = core::mem::take(&mut prepared.lift_decision);
        let mut raid_decision = core::mem::take(&mut prepared.raid_decision);
        let mut staged_strategy = prepared.staged_strategy.take();
        if !allocation_ok {
            self.participants
                .policy
                .restore_checkpoint(snapshots.policy);
            self.advanced.snapshots.restore(&mut self.participants);
            team_decision = StrategicDecision::default();
            lift_decision = StrategicDecision::default();
            raid_decision = StrategicDecision::default();
            if staged_strategy.is_some() {
                staged_strategy = Some(StrategicThinkResult::default());
            }
            let restored_claims = PlannerClaims::new(
                self.context.enlisted,
                self.participants.strategy,
                self.participants.raids,
                self.participants.lifts,
            );
            let restored_team_core = self.participants.team.core_reservations();
            planner_claims = restored_claims.all(&restored_team_core);
            strategic_core_exclusions = restored_claims.core_exclusions(&restored_team_core);
        }

        let committed = PlannerClaims::new(
            self.context.enlisted,
            self.participants.strategy,
            self.participants.raids,
            self.participants.lifts,
        );
        let team_members = self.participants.team.core_reservations();
        planner_claims.extend(committed.all(&team_members));
        strategic_core_exclusions.extend(committed.core_exclusions(&team_members));
        for claims in [&mut planner_claims, &mut strategic_core_exclusions] {
            claims.extend(self.participants.policy.state.reconnaissance.reservations());
            claims.extend(self.participants.policy.support_reservations());
            claims.sort_unstable();
            claims.dedup();
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
            fresh_defense_intents: effects.fresh_defense_intents,
            maintenance_intents: prepared.maintenance_intents,
            fresh_economy_intents: effects.fresh_economy_intents,
            allocated_producer_intents: effects.allocated_producer_intents,
            allocation_ok,
            accepted_connected: effects.accepted_connected,
            producer_lane_reservations: effects.producer_lane_reservations,
            budget: effects.budget,
            foundry_handoff: self.participants.policy.foundry_handoff(),
        }
    }
}

fn snapshot_claims(
    context: &AllocationSessionContext<'_>,
    participants: &AllocationParticipants<'_>,
) -> ClaimSnapshot {
    let claims = PlannerClaims::new(
        context.enlisted,
        participants.strategy,
        participants.raids,
        participants.lifts,
    );
    let team_core_claims = participants.team.core_reservations();
    let mut planner_claims = claims.all(&team_core_claims);
    let mut strategic_core_exclusions = claims.core_exclusions(&team_core_claims);
    append_utility_assignments(participants, &mut planner_claims);
    append_utility_assignments(participants, &mut strategic_core_exclusions);
    let opening_core = combat_core_status(
        context.observation,
        &strategic_core_exclusions,
        &[],
        u64::from(context.dials.minimum_core_equivalents),
    );
    ClaimSnapshot {
        team_core_claims,
        planner_claims,
        strategic_core_exclusions,
        opening_core,
    }
}

fn append_utility_assignments(participants: &AllocationParticipants<'_>, claims: &mut Vec<UnitId>) {
    claims.extend(participants.policy.state.reconnaissance.reservations());
    claims.extend(participants.policy.support_reservations());
    claims.sort_unstable();
    claims.dedup();
}

fn economic_air_work(
    context: &AllocationSessionContext<'_>,
    participants: &AllocationParticipants<'_>,
    lift_unavailable: &[UnitId],
) -> Vec<AirCapacityDemand> {
    let mut demands = Vec::new();
    let planner = &*participants.strategy;
    if let Some(operation) = planner.air_operation()
        && let Some(deadline) = planner.air_capacity_deadline()
    {
        let work_ticks = planner.remaining_airwork_ticks(context.observation);
        if work_ticks > 0 {
            demands.push(AirCapacityDemand {
                work_ticks,
                deadline,
                kind: oxide_sim::stats::Role::Bomber.unit_for(context.observation.faction),
                service: StandingGroundTarget::point(operation.target),
            });
        }
    }
    let planner = &*participants.lifts;
    if let Some(operation) = planner.operation() {
        let work_ticks = planner.remaining_airwork_ticks(context.observation, lift_unavailable);
        if work_ticks > 0 {
            demands.push(AirCapacityDemand {
                work_ticks,
                deadline: operation.deadline,
                kind: UnitKind::Skyhook,
                service: StandingGroundTarget::point(operation.target),
            });
        }
    }
    demands
}

fn standing_force_projection_targets(
    context: &AllocationSessionContext<'_>,
    participants: &AllocationParticipants<'_>,
) -> Vec<StandingGroundTarget> {
    let mut targets = participants
        .policy
        .uncleared_hostile_starts(context.public_map, context.observation.me)
        .into_iter()
        .map(|start| {
            StandingGroundTarget::footprint(start.anchor, BuildingKind::Foundry.base_stats().size)
        })
        .collect::<Vec<_>>();
    targets.extend(
        context
            .intelligence
            .buildings()
            .iter()
            .filter(|contact| contact.hp > 0 && contact.confidence_at(context.observation.tick) > 0)
            .map(|contact| {
                StandingGroundTarget::footprint(
                    contact.anchor,
                    contact.kind.tier_stats(contact.tier).size,
                )
            }),
    );
    targets.extend(
        context
            .intelligence
            .units()
            .iter()
            .filter(|contact| {
                contact.hp > 0
                    && contact.confidence_at(context.observation.tick) > 0
                    && contact.body_domain() == Domain::Ground
            })
            .map(|contact| StandingGroundTarget::point(contact.tile)),
    );
    targets
}

fn allocation_horizon(
    context: &AllocationSessionContext<'_>,
    participants: &AllocationParticipants<'_>,
    saved: &SavedFoundryPreparation,
    fresh: &FreshInvestmentPreparation,
    active_connected: Option<&ActiveConnectedObligation>,
) -> Tick {
    let mut horizon = context
        .observation
        .tick
        .saturating_add(connected_preparation_horizon())
        .max(
            context
                .observation
                .tick
                .saturating_add(context.dials.cadence),
        );
    if let Some(obligation) = saved.obligation {
        horizon = horizon.max(obligation.forecast_deadline());
    }
    if let Some(saving) = participants.policy.economic_saving() {
        horizon = horizon.max(saving.deadline);
    }
    if let Some(saving) = &participants.policy.state.standing_saving {
        horizon = horizon.max(saving.job.ready_before);
    }
    if let Some(active) = active_connected {
        horizon = horizon.max(active.deadline());
    }
    if let Some(operation) = participants.lifts.operation() {
        horizon = horizon.max(operation.deadline);
    }
    fresh.funding_horizon(horizon)
}

fn support_context<'a>(
    context: &'a AllocationSessionContext<'_>,
    claims: &'a ClaimSnapshot,
    resources: &'a ResourceSnapshot,
) -> EconomicInvestmentContext<'a> {
    EconomicInvestmentContext {
        evidence: context.evidence,
        obligations: &[],
        obs: context.observation,
        resources,
        profile: context.profile,
        briefing: context.public_map,
        orientation: context.orientation,
        unavailable: &claims.planner_claims,
        demands: &[],
        cadence: context.dials.cadence,
        unit_contacts: context.intelligence.units(),
        building_contacts: context.intelligence.buildings(),
        protected_scrap: 0,
        air_work: &[],
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
    } else if proposal.claims().current_scrap() >= voluntary_scrap_guard
        || proposal.case().urgency == Urgency::Pressing
    {
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
            if operation.recovery_reason == Some(crate::strategy::AirRecoveryReason::Complete) {
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
    active_lift_precedes_foundry: bool,
    active_lift_spendable: u32,
    saved_plan_reserve_already_imported: u32,
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

impl ActiveRevisionPreparation {
    fn defense_admission_reserve(&self, voluntary_guard: u32, carrier_floor: u32) -> u32 {
        if self.proposal.is_some() {
            0
        } else {
            super::voluntary_construction_admission_reserve(voluntary_guard, carrier_floor)
        }
    }
}

#[derive(Default)]
struct FreshInvestmentPreparation {
    foundry: Option<FreshFoundryProposal>,
    defense: Vec<FreshDefenseProposal>,
    economy: Vec<EconomicInvestment>,
    connected: Option<FreshConnectedProposal>,
    standing_force: StandingForcePreparation,
    standing_force_derivation: StandingForceDerivation,
    connected_accepted_at: Option<Tick>,
    connected_reserve_deadline: Tick,
    rejected_connected_candidate: Option<RejectedConnectedCandidate>,
}

impl FreshInvestmentPreparation {
    fn funding_horizon(&self, mut horizon: Tick) -> Tick {
        if let Some(proposal) = &self.foundry {
            horizon = horizon.max(proposal.forecast_deadline());
        }
        if let Some(proposal) = &self.connected {
            horizon = horizon.max(proposal.deadline());
        }
        // Defense pays current capital only; completion times rank its utility
        // but must not extend the funding window of unrelated investments.
        for proposal in &self.economy {
            horizon = horizon.max(proposal.deadline);
        }
        self.standing_force.for_each(|proposal| {
            horizon = horizon.max(proposal.reservation_deadline());
        });
        horizon
    }
}

struct CommitEffects {
    accepted_connected: bool,
    producer_lane_reservations: ProducerLaneReservations,
    fresh_emergency_defense_intents: Vec<Intent>,
    fresh_foundry_intents: Vec<Intent>,
    fresh_defense_intents: Vec<Intent>,
    fresh_economy_intents: Vec<Intent>,
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
            fresh_defense_intents: Vec::new(),
            fresh_economy_intents: Vec::new(),
            allocated_producer_intents: Vec::new(),
            budget: AllocationBudgetOutcome::frozen(
                prepared.foundry_saving,
                prepared.opening_bootstrap,
                prepared.voluntary_scrap_guard,
            ),
        }
    }
}

struct ObservedAllocation {
    resources: ResourceSnapshot,
    support_snapshot: SupportWorkSnapshot,
    recon_paid_exclusions: Vec<(oxide_sim::ids::BuildingId, UnitKind, usize)>,
    maintenance_intents: Vec<Intent>,
}

struct PreparedAllocation {
    resources: ResourceSnapshot,
    maintenance_intents: Vec<Intent>,
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
    fresh_defense: Vec<FreshDefenseProposal>,
    fresh_economy: Vec<EconomicInvestment>,
    fresh_support: Vec<crate::utility::RepairAssignment>,
    fresh_support_construction: Vec<EconomicInvestment>,
    fresh_support_relief: Option<crate::team::TeamReliefOperation>,
    fresh_support_deployments: Vec<crate::utility::SupportDeployment>,
    fresh_reconnaissance: Vec<crate::utility::ReconProposal>,
    fresh_connected: Option<FreshConnectedProposal>,
    standing_force: StandingForcePreparation,
    connected_accepted_at: Option<Tick>,
    connected_reserve_deadline: Tick,
    allocation_horizon: Tick,
    active_lift_precedes_foundry: bool,
    active_lift_spendable: u32,
    foundry_saving: u32,
    opening_bootstrap: u32,
    voluntary_scrap_guard: u32,
    prospective_carrier_floor: u32,
    rejected_connected_candidate: Option<RejectedConnectedCandidate>,
    staged_strategy: Option<StrategicThinkResult>,
    emergency_defense: Option<FreshEmergencyDefense>,
    team_decision: StrategicDecision,
    lift_decision: StrategicDecision,
    raid_decision: StrategicDecision,
}

struct CommitSnapshots {
    policy: crate::utility::PolicyCheckpoint,
}

#[derive(Debug)]
enum AllocationFailure {
    Coordinator(CoordinatorFailure),
    // The allocator records conflicts and deferral in its own trace.
    Unsettled,
}

struct ResolvedAllocation {
    prepared: PreparedAllocation,
    settlement: Result<CrossDomainSettlement, AllocationFailure>,
    snapshots: CommitSnapshots,
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

fn active_air_units(planner: &StrategicPlanner, observation: &Observation) -> Vec<UnitId> {
    let mut units = planner.air_operation().map_or_else(Vec::new, |operation| {
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
    planning: &crate::planning::PlanningWork,
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
    use crate::planning::Progress;
    if requested == 0 {
        return Ok(decision.clone());
    }
    match obligations_resolve(
        resources,
        prior_obligations,
        production_deadline,
        cadence,
        planning,
    )? {
        Progress::Ready(()) => {}
        Progress::ProvenInfeasible => return Ok(decision.clone()),
        Progress::Deferred => return Ok(strategic_decision_with_production_prefix(decision, 0)),
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
        if imported.is_err() {
            infeasible = candidate_count;
            continue;
        }
        match obligations_resolve(
            resources,
            &obligations,
            production_deadline,
            cadence,
            planning,
        )? {
            Progress::Ready(()) => feasible = candidate_count,
            Progress::ProvenInfeasible => infeasible = candidate_count,
            Progress::Deferred => break,
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
    planning: &crate::planning::PlanningWork,
) -> Result<crate::planning::Progress<()>, AllocationCoordinatorFailureReasonTrace> {
    let horizon = obligation_horizon(obligations, horizon);
    let mut allocation = CrossDomainAllocation::new(resources, horizon, cadence)
        .map_err(|error| AllocationCoordinatorFailureReasonTrace::from(&error))?;
    for obligation in obligations.iter().cloned() {
        allocation.import(obligation);
    }
    Ok(
        match allocation.resolve_planned(AllocationPersonality::default(), None, planning) {
            Ok(_) => crate::planning::Progress::Ready(()),
            Err(AllocationError::Deferred) => crate::planning::Progress::Deferred,
            Err(_) => crate::planning::Progress::ProvenInfeasible,
        },
    )
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
            .chain(obligation.claims.foregone_income())
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
    if operation.phase != crate::lift::LiftPhase::Provision {
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
                crate::resources::ProducerPlanningProjection::producer,
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
    planning: &crate::planning::PlanningWork,
) -> Result<Option<ImportedObligation>, CoordinatorInputError> {
    let Some(full) = active_lift_future_production_obligation(context)? else {
        return Ok(None);
    };
    let requested = full.claims.producer_jobs().len();
    let mut feasible = 0_usize;
    let mut best = None;
    let mut infeasible = requested.saturating_add(1);
    while feasible.saturating_add(1) < infeasible {
        let candidate_count = feasible.saturating_add(infeasible).div_ceil(2);
        let candidate =
            active_lift_future_production_obligation_with_limit(context, candidate_count)?
                .expect("a positive prefix of a nonempty Lift demand remains nonempty");
        let mut obligations = prior_obligations.to_vec();
        obligations.push(candidate);
        let horizon = obligation_horizon(&obligations, context.operation.deadline);
        let capacity =
            super::AllocationCapacity::from_snapshot(context.resources, horizon, context.cadence)?;
        match super::forecast::refine_obligation(
            &capacity,
            prior_obligations,
            obligations.pop().unwrap(),
            planning,
        ) {
            crate::planning::Progress::Ready(candidate) => {
                feasible = candidate_count;
                best = Some(candidate);
            }
            crate::planning::Progress::Deferred => return Ok(best),
            crate::planning::Progress::ProvenInfeasible => infeasible = candidate_count,
        }
    }
    Ok(best)
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
    planning: &crate::planning::PlanningWork,
) -> Option<(ProducerLaneReservations, Vec<Intent>)> {
    let horizon = lift
        .map_or(observed_at.saturating_add(cadence), |lift| {
            lift.producer_jobs()
                .iter()
                .map(|job| job.timing().ready_before())
                .max()
                .unwrap_or(observed_at.saturating_add(cadence))
        })
        .max(connected.map_or(0, ActiveConnectedObligation::deadline));
    let mut allocation = CrossDomainAllocation::new(resources, horizon, cadence).ok()?;
    if let Some(lift) = lift {
        allocation.import(active_lift_production_obligation(lift).ok()?);
    }
    if let Some(connected) = connected {
        allocation.import(active_connected_obligation(connected).ok()?);
    }
    let settled = allocation
        .resolve_planned(AllocationPersonality::default(), None, planning)
        .ok()?;
    let due = settled
        .producer_schedule()
        .iter()
        .filter(|job| job.enqueued_at == observed_at)
        .map(|job| Intent::TrainAt {
            building: job.producer,
            kind: job.kind,
        })
        .collect();
    Some((settled.producer_lane_reservations().clone(), due))
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
    planning: &crate::planning::PlanningWork,
) -> Option<(ProducerLaneReservations, Vec<Intent>)> {
    let mut horizon = observed_at.saturating_add(cadence);
    for obligation in obligations {
        for job in obligation.claims.producer_jobs() {
            horizon = horizon.max(job.ready_before());
        }
        for claim in obligation
            .claims
            .forecast_scrap()
            .iter()
            .chain(obligation.claims.foregone_income())
        {
            horizon = horizon.max(claim.through);
        }
        if let Some(claim) = obligation.claims.deferrable_capital() {
            horizon = horizon.max(claim.through);
        }
    }
    let mut allocation = CrossDomainAllocation::new(resources, horizon, cadence).ok()?;
    for obligation in obligations
        .iter()
        .filter(|obligation| {
            obligation
                .claims
                .producer_jobs()
                .iter()
                .all(|job| job.fixed_assignment().is_some())
        })
        .cloned()
    {
        allocation.import(obligation);
    }
    let settlement = allocation
        .resolve_planned(AllocationPersonality::default(), None, planning)
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

/// A typed Defense quote cannot count a live unit that any co-selectable fresh
/// Connected package may own as mobile reinforcement. Defense has one frozen
/// case across Connected portfolio variants, so derive it against their
/// canonical union rather than allowing one unit to justify both investments.
fn defense_reinforcement_exclusions(
    prior: &[UnitId],
    connected: Option<&FreshConnectedProposal>,
) -> Vec<UnitId> {
    let mut excluded = prior.to_vec();
    if let Some(connected) = connected {
        excluded.extend_from_slice(connected.minimum_claims().units());
        for marginal in connected.marginal_variants() {
            excluded.extend_from_slice(marginal.additions().units());
        }
    }
    excluded.sort_unstable();
    excluded.dedup();
    excluded
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
pub(crate) fn test_allocate_policy(
    policy: &mut UtilityPolicy,
    dials: &Dials,
    observation: &Observation,
    public_map: &PublicMapBriefing,
    enlisted: &[UnitId],
) -> AllocationSessionOutcome {
    use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
    let profile = crate::profile::ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        7,
    ));
    let tuning = DifficultyTuning::for_level(profile.difficulty);
    let home = observation
        .my_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Foundry)
        .unwrap()
        .anchor;
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(observation);
    let mut strategy = StrategicPlanner::new();
    let mut lifts = LiftPlanner::new();
    let mut team = TeamReliefPlanner::new();
    let mut raids = RaidPlanner::new();
    let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
    let mut trace = AllocationTrace::default();
    let outcome = AllocationSession::new(
        AllocationSessionContext {
            evidence: Default::default(),
            dials,
            profile: &profile,
            tuning,
            observation,
            home,
            public_map,
            orientation: Orientation::for_home(observation, home),
            intelligence: &intelligence,
            enlisted,
            lift_support: None,
        },
        AllocationParticipants {
            policy,
            strategy: &mut strategy,
            lifts: &mut lifts,
            team: &mut team,
            raids: &mut raids,
        },
        AdvancedPlannerWork {
            team_decision: StrategicDecision::default(),
            raid_decision: StrategicDecision::default(),
            team_started_at: observation.tick,
            lift_started_at: observation.tick,
            raid_started_at: observation.tick,
            lift_was_active: false,
            initial_lift_support: LiftAirSupport::Independent,
            lift_unavailable: enlisted.to_vec(),
            preliminary_core: crate::utility::combat_core_status(
                observation,
                enlisted,
                &[],
                u64::from(dials.minimum_core_equivalents),
            ),
            preliminary_core_exclusions: enlisted.to_vec(),
            snapshots,
        },
        Some(&mut trace),
    )
    .run();
    if !outcome.allocation_ok {
        eprintln!(
            "allocation fixture tick={} error={:?} coordinator={:?}",
            observation.tick, trace.error, trace.coordinator_failure
        );
    }
    outcome
}

#[cfg(test)]
mod tests {
    mod retained;
    use super::super::{
        Confidence, DeferrableCapitalClaim, ExecutionSafety, ProposalCase, StrategicValue,
        TimeToImpact, Urgency,
    };
    use super::standing::ContextualStandingForce;
    use super::*;
    use crate::briefing::PublicMapBriefing;
    use crate::lift::LiftPhase;
    use crate::observation::{BuildingObs, UnitObs};
    use crate::profile::Specialty;
    use crate::standing_force::{StandingForceFixture, StandingForceProposal, StandingForceReason};
    use crate::strategy::{
        AirRecoveryReason, ConnectedConfidence, ConnectedExecutionSafety, ConnectedOffenseClaims,
        ConnectedOpportunityCase, ConnectedProviderJob, ConnectedStrategicValue,
        ConnectedTimeToImpact, ConnectedUrgency, FreshConnectedProposalFixture,
    };
    use crate::trace::{AllocationConflictTrace, ProposalDispositionTrace, ProposalKeyTrace};
    use crate::utility::{
        DefenseConstruction, FoundryConfidence, FoundryExecutionSafety, FoundryOpportunityCase,
        FoundryStrategicValue, FoundryTimeToImpact, FoundryUrgency,
    };
    use oxide_sim::ids::{BuildingId, PlayerId};
    use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
    use oxide_sim::stats::{BuildingKind, UnitKind};

    struct SessionProfile {
        profile: ResolvedProfile,
        tuning: DifficultyTuning,
        dials: Dials,
    }

    impl SessionProfile {
        fn new(profile: ResolvedProfile) -> Self {
            let tuning = DifficultyTuning::for_level(profile.difficulty);
            let dials = Dials::scripted(&profile, tuning);
            Self {
                profile,
                tuning,
                dials,
            }
        }

        fn context<'a>(
            &'a self,
            observation: &'a Observation,
            home: TilePos,
            public_map: &'a PublicMapBriefing,
            intelligence: &'a StrategicIntelligence,
        ) -> AllocationSessionContext<'a> {
            AllocationSessionContext {
                evidence: Default::default(),
                dials: &self.dials,
                profile: &self.profile,
                tuning: self.tuning,
                observation,
                home,
                public_map,
                orientation: Orientation::for_home(observation, home),
                intelligence,
                enlisted: &[],
                lift_support: None,
            }
        }
    }

    #[test]
    fn rollback_preserves_observed_outcomes_without_committing_new_ownership() {
        use crate::experience::{
            Doctrine, EpisodeId, EpisodeOwner, ExperienceKey, Outcome, OutcomeReason,
        };
        let mut policy = UtilityPolicy::default();
        let mut strategy = StrategicPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let obs = observation();
        let journal = &mut raids.outcomes;
        journal.watch(
            &obs,
            EpisodeId {
                owner: EpisodeOwner::Raid,
                serial: 1,
            },
            ExperienceKey {
                doctrine: Doctrine::Pressure,
                x: 3,
                y: 3,
                subject: 4,
            },
            &[],
            1,
        );
        journal.finish(
            &obs,
            Outcome::Aborted,
            OutcomeReason::UnsafeApproach,
            750,
            false,
        );
        let observed = journal.clone();
        snapshots.restore(&mut AllocationParticipants {
            policy: &mut policy,
            strategy: &mut strategy,
            team: &mut team,
            lifts: &mut lifts,
            raids: &mut raids,
        });
        assert_eq!(raids.outcomes, observed);
        assert!(raids.operation().is_none());
    }

    fn observation() -> Observation {
        Observation::from_data(ObservationData {
            map_width: 20,
            map_height: 20,
            visible: vec![true; 400],
            explored: vec![true; 400],
            scrap: 40,
            ..crate::test_support::observation_data()
        })
    }

    fn briefing() -> PublicMapBriefing {
        PublicMapBriefing {
            regions: Default::default(),
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
        crate::test_support::unit(id, PlayerId(0), kind, tile)
    }

    fn observed_building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        crate::test_support::building(id, PlayerId(player), kind, anchor)
    }

    fn active_lift_fixture() -> (Observation, LiftPlanner, Tick) {
        const HOME: TilePos = TilePos::new(5, 15);
        let mut observation = Observation::from_data(ObservationData {
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
            ..crate::test_support::observation_data()
        });
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
        mut strategy: StrategicPlanner,
        mut lifts: LiftPlanner,
    ) -> (AllocationTrace, AllocationSessionOutcome) {
        const HOME: TilePos = TilePos::new(5, 15);
        let public_map = PublicMapBriefing {
            regions: Default::default(),
            map_width: observation.map_width,
            map_height: observation.map_height,
            starting_foundries: Vec::new(),
            teams: vec![None, None],
            non_ground_terrain: Vec::new(),
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        };
        let setup = SessionProfile::new(crate::profile::ResolvedProfile::resolve(
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7),
        ));
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(observation);
        let mut policy = UtilityPolicy::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut advanced = advanced(snapshots);
        if let Some(operation) = (lifts).operation() {
            advanced.lift_was_active = true;
            advanced.lift_started_at = operation.started_at;
        }
        let mut trace = AllocationTrace::default();
        let outcome = AllocationSession::new(
            setup.context(observation, HOME, &public_map, &intelligence),
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
            maintenance_intents: Vec::new(),
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
            fresh_defense: Vec::new(),
            fresh_economy: Vec::new(),
            fresh_support: Vec::new(),
            fresh_support_construction: Vec::new(),
            fresh_support_relief: None,
            fresh_support_deployments: Vec::new(),
            fresh_reconnaissance: Vec::new(),
            fresh_connected: None,
            standing_force: StandingForcePreparation::default(),
            connected_accepted_at: None,
            connected_reserve_deadline: 120,
            allocation_horizon: 120,
            active_lift_precedes_foundry: false,
            active_lift_spendable: 0,
            foundry_saving: 11,

            opening_bootstrap: 13,
            voluntary_scrap_guard: 0,
            prospective_carrier_floor: 0,
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
        crate::profile::ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            7,
        ))
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
        let mut observation = Observation::from_data(ObservationData {
            tick,
            scrap,
            map_width: 32,
            map_height: 20,
            visible: vec![true; 32 * 20],
            explored: vec![true; 32 * 20],
            enemy_buildings: vec![observed_building(80, 1, BuildingKind::Crucible, TARGET)],
            ..crate::test_support::observation_data()
        });
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
            regions: Default::default(),
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
                    planning: None,
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

    fn current_connected_planner(
        observation: &Observation,
    ) -> (StrategicPlanner, Vec<ConnectedProviderJob>) {
        let proposal = current_connected_proposal(observation);
        let jobs = proposal.minimum_claims().provider_jobs().to_vec();
        let mut planner = StrategicPlanner::new();
        planner.commit_connected_proposal(proposal).unwrap();
        (planner, jobs)
    }

    fn connected_obligation(
        planner: &mut StrategicPlanner,
        observation: &Observation,
    ) -> ActiveConnectedObligation {
        let profile = prime_profile();
        let briefing = connected_briefing(observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(observation);
        planner
            .active_connected_obligation(FreshConnectedProposalRequest::new(
                &profile,
                DifficultyTuning::for_level(profile.difficulty),
                observation,
                &ResourceSnapshot::from_observation(observation),
                &intelligence,
                TilePos::new(3, 10),
                StrategicCoordination {
                    planning: None,
                    enlisted: &[],
                    lift_support: None,
                    allow_new_operation: false,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                    public_map: Some(&briefing),
                    orientation: Orientation::for_home(observation, TilePos::new(3, 10)),
                },
            ))
            .expect("an admitted connected operation retains demand")
    }

    fn run_connected_session(
        observation: &Observation,
        policy: &mut UtilityPolicy,
        strategy: &mut StrategicPlanner,
    ) -> AllocationSessionOutcome {
        run_connected_session_with_team_decision_and_trace(
            observation,
            policy,
            strategy,
            StrategicDecision::default(),
            None,
        )
    }

    #[test]
    fn military_saving_keeps_its_paid_time_and_releases_after_purchase() {
        let mut obs = connected_observation(120, 110);
        obs.my_units
            .retain(|unit| unit.kind != UnitKind::Sentinel || unit.id.0 <= 8);
        for id in 101..105 {
            obs.my_units
                .push(owned_unit(id, UnitKind::Harvester, TilePos::new(5, 14)));
        }
        for id in 20..30 {
            let mut income = observed_building(
                id,
                0,
                BuildingKind::Reclaimer,
                TilePos::new(12 + (id - 20) as i32, 2),
            );
            income.tier = 2;
            income.hp = income.kind.tier_stats(2).max_hp;
            obs.my_buildings.push(income);
            obs.my_queues.push(Vec::new());
            obs.my_queue_progress.push(0);
        }
        let mut policy = UtilityPolicy::new();
        let mut trace = AllocationTrace::default();
        let first = run_connected_session_with_team_decision_and_trace(
            &obs,
            &mut policy,
            &mut StrategicPlanner::new(),
            StrategicDecision::default(),
            Some(&mut trace),
        );
        assert!(first.allocation_ok);
        let saving = policy.state.standing_saving.clone().unwrap_or_else(|| {
            panic!("useful higher-tier waiting must survive settlement: {trace:#?}")
        });
        assert!(saving.job.enqueued_at > obs.tick);
        assert!(!first.allocated_producer_intents.iter().any(
            |intent| matches!(intent, Intent::TrainAt { kind, .. } if *kind == saving.job.kind)
        ));
        let original = obs.clone();
        obs.tick += 12;
        obs.scrap = 1000;
        let second = run_connected_session(&obs, &mut policy, &mut StrategicPlanner::new());
        assert!(second.allocation_ok);
        assert_eq!(
            policy.state.standing_saving.as_ref().unwrap().job,
            saving.job
        );
        assert!(!second.allocated_producer_intents.iter().any(
            |intent| matches!(intent, Intent::TrainAt { kind, .. } if *kind == saving.job.kind)
        ));
        obs.tick = saving.job.enqueued_at;
        let purchased = run_connected_session(&obs, &mut policy, &mut StrategicPlanner::new());
        assert!(purchased.allocation_ok);
        assert!(
            purchased
                .allocated_producer_intents
                .contains(&Intent::TrainAt {
                    building: saving.job.producer,
                    kind: saving.job.kind
                })
        );
        assert!(policy.state.standing_saving.is_none());

        for loss in [
            "core",
            "producer",
            "income",
            "objective",
            "emergency",
            "missed purchase",
        ] {
            let mut invalid = original.clone();
            invalid.tick += 12;
            match loss {
                "core" => invalid.my_units.clear(),
                "producer" => invalid
                    .my_buildings
                    .retain(|building| building.id != saving.job.producer),
                "income" => invalid
                    .my_buildings
                    .retain(|building| building.kind != BuildingKind::Reclaimer),
                "objective" => invalid.enemy_buildings.clear(),
                "emergency" => {
                    let mut enemy = owned_unit(999, UnitKind::Breaker, TilePos::new(7, 10));
                    enemy.player = PlayerId(1);
                    invalid.enemy_units.push(enemy);
                }
                "missed purchase" => invalid.tick = saving.job.enqueued_at + 12,
                _ => unreachable!(),
            }
            invalid.my_queues = vec![Vec::new(); invalid.my_buildings.len()];
            invalid.my_queue_progress = vec![0; invalid.my_buildings.len()];
            let mut policy = UtilityPolicy::new();
            policy.state.standing_saving = Some(saving.clone());
            let result = run_connected_session(&invalid, &mut policy, &mut StrategicPlanner::new());
            assert!(result.allocation_ok, "{loss}");
            assert!(
                policy
                    .state
                    .standing_saving
                    .as_ref()
                    .is_none_or(|next| next.job != saving.job),
                "{loss} must release the old unpaid purchase"
            );
        }
    }

    #[test]
    fn retained_raid_queues_are_obligations_before_connected_package_derivation() {
        use crate::raid::RaidPlanningContext;
        const HOME: TilePos = TilePos::new(3, 10);
        for live in [0, 1] {
            let mut obs = connected_observation(120, 10_000);
            if live == 1 {
                obs.my_units
                    .push(owned_unit(101, UnitKind::Scuttler, HOME.offset(4, 0)));
            }
            obs.my_queues[0] = vec![UnitKind::Scuttler; 2 - live];
            let setup = SessionProfile::new(prime_profile());
            let map = connected_briefing(&obs);
            let orientation = Orientation::for_home(&obs, HOME);
            let resources = ResourceSnapshot::from_observation(&obs);
            let mut raid = RaidPlanner::new();
            let request = raid
                .muster_request(
                    RaidPlanningContext::new(&setup.profile, setup.tuning, &obs, HOME, &[], &[]),
                    &resources,
                    Some(&map),
                    Some(orientation),
                )
                .unwrap();
            assert_eq!(request.missing, 0);
            assert!(raid.commit_procurement(request, obs.tick));
            assert!(raid.bind_procurement(&obs, &[], &map, orientation));
            let paid = raid.paid_claims().to_vec();
            obs.tick += 24;
            let mut intelligence = StrategicIntelligence::new();
            intelligence.update(&obs);
            let mut policy = UtilityPolicy::new();
            let mut strategy = StrategicPlanner::new();
            let mut lifts = LiftPlanner::new();
            let mut team = TeamReliefPlanner::new();
            let mut raids = raid;
            let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
            let mut session = AllocationSession::new(
                AllocationSessionContext {
                    orientation,
                    ..setup.context(&obs, HOME, &map, &intelligence)
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
            let observed = session.observe_retained_work();
            let prepared = session.prepare(observed);
            assert!(prepared.coordinator_failure.is_none());
            let claim = prepared
                .obligations
                .iter()
                .find(|obligation| {
                    matches!(
                        obligation.key,
                        ObligationKey::Legacy {
                            channel: LegacyChannel::Raid,
                            sequence: 2
                        }
                    )
                })
                .unwrap();
            assert_eq!(claim.claims.paid_queue(), paid);
            assert!(
                session
                    .committed_standing_production()
                    .iter()
                    .filter(|commitment| commitment.matches(BuildingId(10), UnitKind::Scuttler))
                    .count()
                    >= paid.len()
            );
            let connected = prepared
                .fresh_connected
                .as_ref()
                .expect("a competing package remains feasible");
            for provider in connected.minimum_claims().paid_providers() {
                assert!(
                    !paid
                        .iter()
                        .any(|claim| claim.producer == provider.producer()
                            && claim.kind == provider.kind()
                            && claim.occurrence == provider.occurrence())
                );
            }
            let original_policy = session.participants.policy.clone();
            let resolved = session.resolve(
                prepared,
                CommitSnapshots {
                    policy: original_policy.speculative_checkpoint(),
                },
            );
            let outcome = session.commit_or_restore(resolved);
            assert!(outcome.allocation_ok);
            assert_eq!(raids.paid_claims(), paid);
            assert_eq!(raids.reservations().len(), live);
        }
    }

    fn run_connected_session_with_team_decision(
        observation: &Observation,
        policy: &mut UtilityPolicy,
        strategy: &mut StrategicPlanner,
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
        strategy: &mut StrategicPlanner,
        team_decision: StrategicDecision,
        trace: Option<&mut AllocationTrace>,
    ) -> AllocationSessionOutcome {
        const HOME: TilePos = TilePos::new(3, 10);
        let setup = SessionProfile::new(prime_profile());
        let briefing = connected_briefing(observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(observation);
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(strategy, &team, &lifts, &raids);
        let mut work = advanced(snapshots);
        work.team_decision = team_decision;
        AllocationSession::new(
            setup.context(observation, HOME, &briefing, &intelligence),
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
        strategy: &mut StrategicPlanner,
        outcome: &AllocationSessionOutcome,
    ) -> StrategicThinkResult {
        const HOME: TilePos = TilePos::new(3, 10);
        let profile = prime_profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let briefing = connected_briefing(observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(observation);
        strategy.think_after_connected_adjudication(StrategicThinkContext::new(
            &profile,
            tuning,
            observation,
            &intelligence,
            HOME,
            StrategicCoordination {
                planning: None,
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
        strategy: &mut StrategicPlanner,
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
            .air_operation()
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
            &crate::planning::PlanningWork::default(),
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

        assert!(
            feasible_active_lift_future_production_obligation(
                context,
                &[],
                &crate::planning::PlanningWork::with_allowance(0),
            )
            .unwrap()
            .is_none(),
            "unrefined carrier demand cannot become a mandatory claim"
        );
        let prefix = feasible_active_lift_future_production_obligation(
            context,
            &[],
            &crate::planning::PlanningWork::default(),
        )
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
        observation.my_queues[1] = vec![UnitKind::Condor; oxide_sim::stats::QUEUE_CAP - 2];
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
        let setup = SessionProfile::new(crate::profile::ResolvedProfile::resolve(
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7),
        ));
        let resources = ResourceSnapshot::from_observation(&observation);
        let projection = resources
            .planning_projection(operation.deadline, setup.dials.cadence)
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

            active_lift_precedes_foundry: false,
            active_lift_spendable: 0,
            saved_plan_reserve_already_imported: 0,

            lift_deadline: operation.deadline,
            fresh_lift_producer_jobs: 0,
            voluntary_scrap_guard: 0,
        };
        let public_map = connected_briefing(&observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let mut strategy = StrategicPlanner::new();
        let mut lifts = lift;
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut work = advanced(snapshots);
        work.lift_was_active = true;
        work.lift_started_at = operation.started_at;
        let mut session = AllocationSession::new(
            setup.context(&observation, HOME, &public_map, &intelligence),
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

        session.retained_work().advance_active_lift(
            &mut obligations,
            &mut air_lift,
            &active_connected_due_intents,
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

        let (trace, outcome) = allocation_run_for(&observation, StrategicPlanner::new(), lift);

        assert!(outcome.allocation_ok, "{trace:#?}");
        assert!(outcome.accepted_connected, "{trace:#?}");
        let lift_future = trace
            .obligations
            .entries
            .iter()
            .find(|obligation| {
                matches!(
                    obligation.key,
                    crate::trace::ObligationKeyTrace::Legacy {
                        channel: crate::trace::LegacyChannelTrace::Lift,
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
                    crate::trace::ProducerJobAccessTrace::Flexible { .. }
                )
        }));
        let connected = trace
            .proposals
            .entries
            .iter()
            .find(|proposal| {
                matches!(
                    proposal.key,
                    crate::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
                )
            })
            .expect("outstanding lift work must not prevent connected allocation");
        assert_eq!(
            connected.disposition,
            crate::trace::ProposalDispositionTrace::Accepted
        );

        let lift_jobs = trace
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                job.kind == UnitKind::Skyhook
                    && matches!(
                        job.owner,
                        crate::trace::ClaimOwnerTrace::Obligation {
                            key: crate::trace::ObligationKeyTrace::Legacy {
                                channel: crate::trace::LegacyChannelTrace::Lift,
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
                    crate::trace::ClaimOwnerTrace::Obligation {
                        key: crate::trace::ObligationKeyTrace::Legacy {
                            channel: crate::trace::LegacyChannelTrace::Lift,
                            ..
                        },
                        ..
                    }
                )
            }));
            assert!(lane.iter().any(|job| {
                matches!(
                    job.owner,
                    crate::trace::ClaimOwnerTrace::Proposal {
                        key: crate::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. },
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
            crate::trace::ProposalDispositionTrace::Accepted
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
            crate::trace::ProposalDispositionTrace::Infeasible {
                conflict: crate::trace::AllocationConflictTrace::ProducerSchedule { .. },
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
                    crate::trace::ClaimOwnerTrace::Obligation {
                        key: crate::trace::ObligationKeyTrace::Legacy {
                            channel: crate::trace::LegacyChannelTrace::Lift,
                            sequence: 2,
                        },
                        ..
                    }
                )
        }));
    }

    #[test]
    fn fresh_recon_ownership_reaches_the_same_think_residual_planners() {
        let mut observation = connected_observation(120, 200);
        observation.enemy_buildings.clear();
        observation.visible.fill(false);
        observation.explored.fill(false);
        let briefing = PublicMapBriefing {
            regions: Default::default(),
            map_width: observation.map_width,
            map_height: observation.map_height,
            starting_foundries: vec![crate::StartingFoundry {
                player: PlayerId(1),
                anchor: TilePos::new(24, 10),
            }],
            teams: vec![None, None],
            non_ground_terrain: vec![],
            extractor_frames: vec![],
            initial_scrap: vec![],
        };
        let setup = SessionProfile::new(prime_profile());
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let mut strategy = StrategicPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut trace = AllocationTrace::default();
        let outcome = AllocationSession::new(
            setup.context(&observation, TilePos::new(3, 10), &briefing, &intelligence),
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                team: &mut team,
                lifts: &mut lifts,
                raids: &mut raids,
            },
            advanced(snapshots),
            Some(&mut trace),
        )
        .run();
        assert!(outcome.allocation_ok);
        assert_eq!(
            policy.state.reconnaissance.reservations(),
            [UnitId(100)],
            "{trace:#?}"
        );
        assert!(
            outcome.planner_claims.contains(&UnitId(100)),
            "the legacy island operation must see the scout accepted earlier in this think"
        );
        assert!(outcome.strategic_core_exclusions.contains(&UnitId(100)));
    }

    #[test]
    fn successful_commit_retains_speculative_planners_and_prepared_output() {
        let observation = observation();
        let briefing = briefing();
        let setup = SessionProfile::new(crate::profile::ResolvedProfile::resolve(
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7),
        ));
        let intelligence = StrategicIntelligence::new();
        let original_policy = UtilityPolicy::new();
        let mut policy = original_policy.clone();
        policy.record_dispatched_build(&observation, BuildingKind::Turret, TilePos::new(4, 4));
        assert_ne!(policy, original_policy);
        let committed_policy = policy.clone();
        let original_strategy = StrategicPlanner::new();
        let mut strategy = StrategicPlanner::new();
        let original_team = TeamReliefPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let original_lifts = LiftPlanner::new();
        let mut lifts = LiftPlanner::new();
        let original_raids = RaidPlanner::new();
        let mut raids = RaidPlanner::new();
        let resources = ResourceSnapshot::from_observation(&observation);
        let settlement = CrossDomainAllocation::new(&resources, 120, setup.dials.cadence)
            .expect("the empty resource projection is valid")
            .resolve(AllocationPersonality::default(), None)
            .expect("an empty portfolio is feasible");
        let session = AllocationSession::new(
            setup.context(&observation, TilePos::new(0, 0), &briefing, &intelligence),
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
            settlement: Ok(settlement),
            snapshots: CommitSnapshots {
                policy: original_policy.speculative_checkpoint(),
            },
        });

        assert!(outcome.allocation_ok);
        assert_eq!(outcome.team_decision.committed_scrap, 1);
        assert_eq!(outcome.lift_decision.committed_scrap, 2);
        assert_eq!(outcome.raid_decision.committed_scrap, 3);
        assert_eq!(outcome.planner_claims, vec![UnitId(99)]);
        assert_eq!(outcome.strategic_core_exclusions, vec![UnitId(98)]);
        assert_eq!(policy, committed_policy);
        assert_eq!(strategy, StrategicPlanner::new());
        assert_eq!(team, TeamReliefPlanner::new());
        assert_eq!(lifts, LiftPlanner::new());
        assert_eq!(raids, RaidPlanner::new());
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

        let setup = SessionProfile::new(crate::profile::ResolvedProfile::resolve(
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7),
        ));
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
        let mut allocation = CrossDomainAllocation::new(&resources, deadline, setup.dials.cadence)
            .expect("the two-producer projection is valid");
        allocation.import(
            legacy_decision_obligation(
                &resources,
                LegacyDecisionRequest {
                    cadence: setup.dials.cadence,
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
                    cadence: setup.dials.cadence,
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
        let mut strategy = StrategicPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let session = AllocationSession::new(
            setup.context(&observation, TilePos::new(0, 0), &briefing, &intelligence),
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
            settlement: Ok(settlement),
            snapshots: CommitSnapshots {
                policy: original_policy.speculative_checkpoint(),
            },
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
    fn late_foundry_rejection_restores_an_already_committed_connected_operation() {
        use crate::experience::{
            Doctrine, EpisodeId, EpisodeOwner, ExperienceKey, Outcome, OutcomeReason,
        };
        let mut observation = connected_observation(1_200, 10_000);
        let builder = UnitId(200);
        observation.my_units.push(owned_unit(
            builder.0,
            UnitKind::Harvester,
            TilePos::new(12, 15),
        ));
        let setup = SessionProfile::new(prime_profile());
        let briefing = connected_briefing(&observation);
        let intelligence = StrategicIntelligence::new();
        let connected = current_connected_proposal(&observation);
        let foundry = FreshFoundryProposal::fixture(
            TilePos::new(15, 14),
            builder,
            BuildingKind::Foundry
                .base_stats()
                .construction
                .unwrap()
                .cost,
            0,
            0,
            observation.tick + 1_200,
            foundry_case(),
        );
        let maintenance = vec![Intent::StopUnits {
            units: vec![builder],
        }];

        // A stale prepared quote can conflict with retained ownership even
        // though its resource claims settle. Exercise that internal boundary.
        for restore in [false, true] {
            let mut policy = UtilityPolicy::new();
            policy
                .commit_adjudicated_foundry(foundry.clone(), observation.tick, &mut Vec::new())
                .unwrap();
            let checkpoint = policy.speculative_checkpoint();
            policy.planning = crate::planning::PlanningWork::with_allowance(1);
            let blocked = crate::navigation::public_fields::BlockedGroundLayout::from_predicate(
                &briefing,
                |_| false,
            );
            assert_eq!(
                policy.planning.field(
                    QueryPurpose::NavigationTest,
                    observation.tick,
                    &briefing,
                    &blocked,
                    [TilePos::new(1, 1)],
                ),
                crate::planning::Progress::Deferred
            );
            let expected_policy = policy.clone();
            let original_strategy = StrategicPlanner::new();
            let mut strategy = original_strategy.clone();
            let mut team = TeamReliefPlanner::new();
            let mut lifts = LiftPlanner::new();
            let mut raids = RaidPlanner::new();
            let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
            let journal = &mut raids.outcomes;
            journal.watch(
                &observation,
                EpisodeId {
                    owner: EpisodeOwner::Raid,
                    serial: 1,
                },
                ExperienceKey {
                    doctrine: Doctrine::Pressure,
                    x: 3,
                    y: 3,
                    subject: 4,
                },
                &[],
                1,
            );
            journal.finish(
                &observation,
                Outcome::Aborted,
                OutcomeReason::UnsafeApproach,
                750,
                false,
            );
            let observed_raids = raids.clone();
            let mut input = prepared(&observation, None);
            input.maintenance_intents = maintenance.clone();
            let mut allocation = CrossDomainAllocation::new(
                &input.resources,
                observation.tick + 10_000,
                setup.dials.cadence,
            )
            .unwrap();
            allocation.offer(connected_investment_proposal(connected.clone()).unwrap());
            allocation.offer(foundry_investment_proposal(foundry.clone()).unwrap());
            let settlement = allocation
                .resolve(AllocationPersonality::default(), None)
                .unwrap();
            assert!(!settlement.producer_schedule().is_empty());
            let mut trace = AllocationTrace::default();
            let mut session = AllocationSession::new(
                setup.context(&observation, TilePos::new(3, 10), &briefing, &intelligence),
                AllocationParticipants {
                    policy: &mut policy,
                    strategy: &mut strategy,
                    lifts: &mut lifts,
                    team: &mut team,
                    raids: &mut raids,
                },
                advanced(snapshots),
                Some(&mut trace),
            );
            if restore {
                let outcome = session.commit_or_restore(ResolvedAllocation {
                    prepared: input,
                    settlement: Ok(settlement),
                    snapshots: CommitSnapshots { policy: checkpoint },
                });
                assert!(!outcome.allocation_ok);
                assert!(!outcome.accepted_connected);
                assert_eq!(outcome.maintenance_intents, maintenance);
                assert!(outcome.allocated_producer_intents.is_empty());
                assert!(outcome.fresh_foundry_intents.is_empty());
                assert!(outcome.fresh_economy_intents.is_empty());
                assert!(outcome.fresh_defense_intents.is_empty());
                assert!(outcome.fresh_emergency_defense_intents.is_empty());
                assert_eq!(outcome.team_decision, StrategicDecision::default());
                assert_eq!(outcome.lift_decision, StrategicDecision::default());
                assert_eq!(outcome.raid_decision, StrategicDecision::default());
                assert_eq!(
                    outcome.producer_lane_reservations,
                    ProducerLaneReservations::default()
                );
                assert_eq!(outcome.budget.utility_spendable, 0);
                assert_eq!(outcome.budget.residual_scrap, 0);
                assert_eq!(policy.state, expected_policy.state);
                assert_eq!(policy.planning, expected_policy.planning);
                assert_eq!(strategy, original_strategy);
                assert_eq!(raids, observed_raids);
                let failure = trace.coordinator_failure.unwrap();
                assert_eq!(
                    failure.stage,
                    AllocationCoordinatorStageTrace::FoundryProposalCommit
                );
                assert_eq!(
                    failure.reason,
                    AllocationCoordinatorFailureReasonTrace::ExistingFoundryCommitment
                );
            } else {
                let Err(failure) = session.commit_settlement(&mut input, settlement) else {
                    panic!("the stale Foundry quote must reject after connected commitment");
                };
                assert!(
                    session
                        .participants
                        .strategy
                        .connected_package_diagnostics()
                        .is_some()
                );
                assert_eq!(
                    failure,
                    (
                        AllocationCoordinatorStageTrace::FoundryProposalCommit,
                        AllocationCoordinatorFailureReasonTrace::ExistingFoundryCommitment,
                    )
                );
            }
        }
    }

    #[test]
    fn rejected_allocation_restores_participants_and_preserves_maintenance() {
        let observation = observation();
        let briefing = briefing();
        let setup = SessionProfile::new(crate::profile::ResolvedProfile::resolve(
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 7),
        ));
        let intelligence = StrategicIntelligence::new();
        let original_policy = UtilityPolicy::new();
        let mut policy = original_policy.clone();
        policy.planning = crate::planning::PlanningWork::with_allowance(1);
        let blocked = crate::navigation::public_fields::BlockedGroundLayout::from_predicate(
            &briefing,
            |_| false,
        );
        assert_eq!(
            policy.planning.field(
                QueryPurpose::NavigationTest,
                observation.tick,
                &briefing,
                &blocked,
                [TilePos::new(1, 1)]
            ),
            crate::planning::Progress::Deferred
        );
        let pending = policy.planning.clone();
        let checkpoint = policy.speculative_checkpoint();
        assert_eq!(policy.planning, pending);
        assert_eq!(checkpoint, original_policy.speculative_checkpoint());
        policy.record_dispatched_build(&observation, BuildingKind::Turret, TilePos::new(4, 4));
        let original_strategy = StrategicPlanner::new();
        let mut strategy = StrategicPlanner::new();
        let original_team = TeamReliefPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let original_lifts = LiftPlanner::new();
        let mut lifts = LiftPlanner::new();
        let original_raids = RaidPlanner::new();
        let mut raids = RaidPlanner::new();
        let mut session = AllocationSession::new(
            setup.context(&observation, TilePos::new(0, 0), &briefing, &intelligence),
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
        let maintenance = vec![
            Intent::StopUnits {
                units: vec![UnitId(8)],
            },
            Intent::MoveUnits {
                units: vec![UnitId(9)],
                goal: TilePos::new(3, 3),
            },
        ];
        prepared.maintenance_intents = maintenance.clone();
        let resolved = session.resolve(
            prepared,
            CommitSnapshots {
                policy: original_policy.speculative_checkpoint(),
            },
        );
        let outcome = session.commit_or_restore(resolved);

        assert!(!outcome.allocation_ok);
        assert_eq!(outcome.maintenance_intents, maintenance);
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
        assert!(outcome.fresh_defense_intents.is_empty());
        assert!(outcome.fresh_economy_intents.is_empty());
        assert!(outcome.allocated_producer_intents.is_empty());
        assert_eq!(
            outcome.producer_lane_reservations,
            ProducerLaneReservations::default()
        );
        assert_eq!(outcome.budget.residual_scrap, 0);
        assert_eq!(outcome.budget.utility_spendable, 0);
        assert_eq!(outcome.budget.connected_forecast_hold, u32::MAX);
        assert_eq!(policy.planning, pending);
        assert_eq!(policy.state, original_policy.state);
        assert_eq!(strategy, original_strategy);
        assert_eq!(team, original_team);
        assert_eq!(lifts, original_lifts);
        assert_eq!(raids, original_raids);
    }

    #[test]
    fn allocation_failure_restores_policy_state_mutated_during_prepare() {
        let observation = observation();
        let briefing = briefing();
        let setup = SessionProfile::new(prime_profile());
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
        let resources = ResourceSnapshot::from_observation(&observation);
        let observed_context = EconomicInvestmentContext {
            evidence: Default::default(),
            obligations: &[],
            obs: &observation,
            resources: &resources,
            profile: &setup.profile,
            briefing: &briefing,
            orientation: Orientation::for_home(&observation, TilePos::new(0, 0)),
            unavailable: &[],
            demands: &[],
            cadence: setup.dials.cadence,
            unit_contacts: &[],
            building_contacts: &[],
            protected_scrap: 0,
            air_work: &[],
        };
        policy.observe_reconnaissance(observed_context, TilePos::new(0, 0));
        let support = policy.support_work_snapshot(observed_context);
        policy.observe_support_work(&support, observation.tick);
        policy.observe_support_deployments(observed_context, &support.protection);
        let original_policy = policy.clone();
        assert_ne!(original_policy, UtilityPolicy::new());
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

        let mut strategy = StrategicPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
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
            setup.context(&observation, TilePos::new(0, 0), &briefing, &intelligence),
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
            policy.state, original_policy.state,
            "rollback must start before prepare can clear or age retained policy state"
        );
    }

    #[test]
    fn flexible_connected_demand_and_lift_share_capacity_without_overlap() {
        let (observation, _, _) = active_lift_fixture();
        let (mut strategy, _) = current_connected_planner(&observation);
        let active = connected_obligation(&mut strategy, &observation);
        let resources = ResourceSnapshot::from_observation(&observation);
        let mut allocation = CrossDomainAllocation::new(&resources, active.deadline(), 12).unwrap();
        allocation.import(active_connected_obligation(&active).unwrap());
        allocation.import(imported_obligation(
            ObligationClass::Legacy,
            observation.tick,
            ObligationKey::Legacy {
                channel: LegacyChannel::Lift,
                sequence: 2,
            },
            ClaimBundle::new(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::flexible(
                    UnitKind::Skyhook,
                    observation.tick,
                    active.deadline(),
                    vec![
                        observation
                            .my_buildings
                            .iter()
                            .find(|building| building.kind == BuildingKind::Airworks)
                            .unwrap()
                            .id,
                    ],
                )],
            )
            .unwrap(),
        ));
        let settled = allocation
            .resolve(AllocationPersonality::default(), None)
            .unwrap();
        let mut jobs = settled.producer_schedule().to_vec();
        assert!(jobs.iter().any(|job| job.kind == UnitKind::Skyhook));
        assert!(jobs.len() > 1);
        jobs.sort_by_key(|job| (job.producer, job.starts_at));
        assert!(jobs.windows(2).all(
            |pair| pair[0].producer != pair[1].producer || pair[0].ready_at < pair[1].starts_at
        ));
        assert!(jobs.iter().all(|job| job.ready_at < active.deadline()));
    }

    #[test]
    fn destroyed_connected_producer_enters_bounded_recovery() {
        let mut observation = connected_observation(120, 10_000);
        let (mut planner, assignments) = current_connected_planner(&observation);
        let destroyed = assignments[0].eligible_producers()[0];
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
            &mut planner,
            "destroyed producer",
        );
    }

    #[test]
    fn committed_connected_purchases_enter_paid_ownership() {
        let observation = connected_observation(120, 10_000);
        let (mut strategy, _) = current_connected_planner(&observation);
        let outcome = run_connected_session(&observation, &mut UtilityPolicy::new(), &mut strategy);
        assert!(outcome.allocation_ok);
        let paid = strategy.paid_connected_production(&observation);
        assert!(!paid.is_empty());
        for purchase in paid {
            assert!(
                outcome
                    .allocated_producer_intents
                    .contains(&Intent::TrainAt {
                        building: purchase.producer(),
                        kind: purchase.kind(),
                    })
            );
        }
    }

    #[test]
    fn lost_forecast_source_recovers_instead_of_spending_unbacked_credit() {
        let mut observation = connected_observation(1_200, 10_000);
        let (mut planner, _) = current_connected_planner(&observation);
        observation.scrap = 0;
        let remove = observation
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, building)| {
                matches!(
                    building.kind,
                    BuildingKind::Crucible | BuildingKind::Reclaimer
                )
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        for index in remove.into_iter().rev() {
            observation.my_buildings.remove(index);
            observation.my_queues.remove(index);
            observation.my_queue_progress.remove(index);
        }
        observation.tick += 12;
        assert_connected_enters_bounded_recovery(
            &observation,
            &mut UtilityPolicy::new(),
            &mut planner,
            "the remaining force cannot be funded without completed income",
        );
    }

    #[test]
    fn deferred_mandatory_procurement_preserves_ownership_without_spending() {
        assert_deferred_portfolio_preserves_accepted_production(false);
    }

    #[test]
    fn deferred_portfolio_restores_the_operation_replaced_by_a_staged_revision() {
        assert_deferred_portfolio_preserves_accepted_production(true);
    }

    fn assert_deferred_portfolio_preserves_accepted_production(revising: bool) {
        let observation = connected_observation(1_200, 10_000);
        let proposal = current_connected_proposal(&observation);
        let revision = revising.then(|| proposal.clone().into_active_revision_fixture());
        let mut planner = StrategicPlanner::new();
        planner.commit_connected_proposal(proposal).unwrap();
        let active = connected_obligation(&mut planner, &observation);
        let setup = SessionProfile::new(prime_profile());
        let public_map = connected_briefing(&observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        policy.planning = crate::planning::PlanningWork::with_allowance(0);
        let original_policy = policy.clone();
        let mut strategy = planner;
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut input = prepared(&observation, None);
        input.allocation_horizon = active.deadline();
        input.connected_reserve_deadline = active.deadline();
        input.connected_accepted_at = Some(active.accepted_at());
        if let Some(revision) = revision {
            input
                .obligations
                .push(active_connected_revision_obligation(&revision).unwrap());
            input.fresh_connected = Some(revision);
        } else {
            input
                .obligations
                .push(active_connected_obligation(&active).unwrap());
            input.active_connected = Some(active.clone());
        }
        input.obligations.push(imported_obligation(
            ObligationClass::PersistentPlan,
            observation.tick,
            ObligationKey::Legacy {
                channel: LegacyChannel::Lift,
                sequence: 2,
            },
            ClaimBundle::new(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::flexible(
                    UnitKind::Skyhook,
                    observation.tick,
                    active.deadline(),
                    vec![BuildingId(12)],
                )],
            )
            .unwrap(),
        ));
        input.fresh_lift_producer_jobs = 1;
        let mut trace = AllocationTrace::default();
        let mut session = AllocationSession::new(
            setup.context(
                &observation,
                TilePos::new(3, 10),
                &public_map,
                &intelligence,
            ),
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(snapshots),
            Some(&mut trace),
        );
        let resolved = session.resolve(
            input,
            CommitSnapshots {
                policy: original_policy.speculative_checkpoint(),
            },
        );
        assert!(resolved.settlement.is_err());
        let outcome = session.commit_or_restore(resolved);
        assert!(!outcome.allocation_ok);
        assert!(outcome.allocated_producer_intents.is_empty());
        assert_eq!(outcome.budget.utility_spendable, 0);
        let after = connected_obligation(&mut strategy, &observation);
        assert_eq!(after.provider_jobs(), active.provider_jobs());
        assert_eq!(after.deadline(), active.deadline());
        assert_eq!(policy.planning.spent(), 0);
    }

    #[test]
    fn deferred_revision_preserves_the_operation_and_original_deadline() {
        let mut observation = connected_observation(1_200, 10_000);
        let (mut strategy, _) = current_connected_planner(&observation);
        let before = connected_obligation(&mut strategy, &observation);
        observation.tick += 12;
        let mut policy = UtilityPolicy::new();
        policy.planning = crate::planning::PlanningWork::with_allowance(0);
        let _ = run_connected_session(&observation, &mut policy, &mut strategy);
        let after = connected_obligation(&mut strategy, &observation);
        assert_eq!(after.deadline(), before.deadline());
        assert_eq!(after.accepted_at(), before.accepted_at());
        assert_eq!(after.identity(), before.identity());
        assert_eq!(policy.planning.spent(), 0);
    }

    #[test]
    fn accepted_defense_preserves_the_shared_carrier_floor_without_claiming_it() {
        const HOME: TilePos = TilePos::new(5, 15);
        let mut observation = observation();
        let builder = UnitId(7);
        let anchor = TilePos::new(9, 11);
        let carrier_floor = UnitKind::Skyhook.stats().cost;
        let cost = BuildingKind::Turret
            .base_stats()
            .construction
            .expect("Turrets are constructible")
            .cost;
        observation.scrap = cost.saturating_add(carrier_floor);
        observation.my_units.push(owned_unit(
            builder.0,
            UnitKind::Harvester,
            TilePos::new(7, 12),
        ));

        let setup = SessionProfile::new(prime_profile());
        let public_map = briefing();
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let original_policy = policy.clone();
        let mut strategy = StrategicPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut input = prepared(&observation, None);
        input.prospective_carrier_floor = carrier_floor;
        input.fresh_defense = vec![FreshDefenseProposal::fixture(
            DefenseConstruction::Turret,
            anchor,
            builder,
            ProposalCase {
                urgency: Urgency::Pressing,
                confidence: Confidence::Current,
                value: StrategicValue::Material,
                time_to_impact: TimeToImpact::Near,
                safety: ExecutionSafety::Secure,
            },
            100,
            UnitKind::Sentinel.stats().cost,
        )];
        let mut trace = AllocationTrace::default();
        let mut session = AllocationSession::new(
            setup.context(&observation, HOME, &public_map, &intelligence),
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(snapshots),
            Some(&mut trace),
        );
        let resolved = session.resolve(
            input,
            CommitSnapshots {
                policy: original_policy.speculative_checkpoint(),
            },
        );
        let outcome = session.commit_or_restore(resolved);

        assert!(outcome.allocation_ok);
        assert_eq!(
            outcome.fresh_defense_intents,
            vec![Intent::BuildWith {
                builder,
                kind: BuildingKind::Turret,
                anchor,
            }]
        );
        assert_eq!(outcome.budget.residual_scrap, carrier_floor);
        assert_eq!(outcome.budget.utility_spendable, carrier_floor);
        let proposal = trace
            .proposals
            .entries
            .iter()
            .find(|proposal| {
                proposal.key
                    == ProposalKeyTrace::Defense {
                        kind: BuildingKind::Turret,
                        anchor,
                    }
            })
            .expect("the accepted defense remains visible in the trace");
        assert_eq!(proposal.claims.minimum_residual_scrap, carrier_floor);
    }

    #[test]
    fn active_revision_keeps_full_defense_admission_until_claims_are_stable() {
        let mut revision = ActiveRevisionPreparation::default();
        assert_eq!(revision.defense_admission_reserve(90, 110), 110);
        revision.proposal = Some(fixture_connected_proposal(3_000, vec![]));
        assert_eq!(revision.defense_admission_reserve(90, 110), 0);
        revision.proposal = None;
        assert_eq!(revision.defense_admission_reserve(90, 0), 90);
    }

    #[test]
    fn fresh_funding_horizon_includes_delayed_reinforcement_readiness() {
        let standing = StandingForceProposal::fixture(StandingForceFixture {
            observed_at: 120,
            ready_before: 420,
            kind: UnitKind::Lancer,
            reason: StandingForceReason::SiegePressure,
            specialty: Specialty::Siege,
            personality_emphasis: 100,
            case: ProposalCase {
                urgency: Urgency::Developmental,
                confidence: Confidence::Current,
                value: StrategicValue::Incremental,
                time_to_impact: TimeToImpact::Patient,
                safety: ExecutionSafety::Managed,
            },
            eligible_producers: vec![BuildingId(7)],
        })
        .with_accumulation(3_000, 50);
        let fresh = FreshInvestmentPreparation {
            standing_force: StandingForcePreparation::Unconditional(vec![standing]),
            ..FreshInvestmentPreparation::default()
        };
        assert_eq!(fresh.funding_horizon(2_400), 3_300);
    }

    #[test]
    fn distant_current_paid_defense_does_not_extend_other_domains_funding() {
        let mut fresh = FreshInvestmentPreparation {
            connected: Some(fixture_connected_proposal(3_000, vec![])),
            ..FreshInvestmentPreparation::default()
        };
        let expected = fresh.funding_horizon(2_400);
        assert_eq!(expected, 3_000);
        fresh.defense.push(
            FreshDefenseProposal::fixture(
                DefenseConstruction::Turret,
                TilePos::new(9, 11),
                UnitId(7),
                ProposalCase {
                    urgency: Urgency::Developmental,
                    confidence: Confidence::Prior,
                    value: StrategicValue::Incremental,
                    time_to_impact: TimeToImpact::Patient,
                    safety: ExecutionSafety::Managed,
                },
                100,
                0,
            )
            .with_ready_at(6_000),
        );
        assert_eq!(fresh.defense[0].ready_at(), 6_000);
        assert_eq!(fresh.funding_horizon(2_400), expected);
    }

    #[test]
    fn fresh_connected_units_are_unavailable_to_defense_reinforcement_quotes() {
        let connected = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective: BuildingId(700),
            anchor: TilePos::new(16, 12),
            deadline: 240,
            case: ConnectedOpportunityCase::fixture(
                ConnectedUrgency::Timely,
                ConnectedConfidence::Current,
                ConnectedStrategicValue::Material,
                ConnectedTimeToImpact::Near,
                ConnectedExecutionSafety::Managed,
            ),
            minimum_claims: ConnectedOffenseClaims::fixture(vec![UnitId(3), UnitId(5)], Vec::new()),
            marginal_additions: vec![
                ConnectedOffenseClaims::fixture(vec![UnitId(5), UnitId(7)], Vec::new()),
                ConnectedOffenseClaims::fixture(vec![UnitId(7), UnitId(9)], Vec::new()),
            ],
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });

        assert_eq!(
            defense_reinforcement_exclusions(&[UnitId(1), UnitId(5)], Some(&connected)),
            vec![UnitId(1), UnitId(3), UnitId(5), UnitId(7), UnitId(9)],
            "Defense must not count a unit owned by any co-selectable Connected scale"
        );
        assert_eq!(
            defense_reinforcement_exclusions(&[UnitId(5), UnitId(1)], None),
            vec![UnitId(1), UnitId(5)]
        );
    }

    #[test]
    fn session_rejects_only_the_defense_that_would_seal_a_selected_foundry_layout() {
        const HOME: TilePos = TilePos::new(5, 15);
        let mut observation = observation();
        let producer_anchor = TilePos::new(7, 7);
        let producer_size = BuildingKind::Foundry.base_stats().size;
        let foundry_anchor = producer_anchor.offset(producer_size.0, 0);
        let unsafe_defense_anchor = producer_anchor.offset(0, -1);
        let safe_defense_anchor = TilePos::new(2, 2);
        let foundry_builder = UnitId(7);
        let defense_builder = UnitId(8);
        observation.my_buildings.push(observed_building(
            1,
            0,
            BuildingKind::Foundry,
            producer_anchor,
        ));
        observation.my_queues.push(Vec::new());
        observation.my_queue_progress.push(0);
        observation.my_units.extend([
            owned_unit(
                foundry_builder.0,
                UnitKind::Harvester,
                foundry_anchor.offset(producer_size.0 + 1, 1),
            ),
            owned_unit(defense_builder.0, UnitKind::Harvester, TilePos::new(5, 4)),
        ]);
        observation.known_rock =
            oxide_sim::geometry::rect_adjacent_tiles(producer_anchor, producer_size)
                .filter(|tile| {
                    let covered_by_fresh_foundry = tile.x >= foundry_anchor.x
                        && tile.x < foundry_anchor.x + producer_size.0
                        && tile.y >= foundry_anchor.y
                        && tile.y < foundry_anchor.y + producer_size.1;
                    *tile != unsafe_defense_anchor && !covered_by_fresh_foundry
                })
                .collect();
        observation
            .known_rock
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let defense_cost = BuildingKind::Turret
            .base_stats()
            .construction
            .expect("Turrets are constructible")
            .cost;
        observation.scrap = foundry_cost.saturating_add(defense_cost);

        let setup = SessionProfile::new(prime_profile());
        let public_map = briefing();
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let foundry_build = (BuildingKind::Foundry, foundry_anchor, foundry_builder);
        let unsafe_defense_build = (BuildingKind::Turret, unsafe_defense_anchor, defense_builder);
        let safe_defense_build = (BuildingKind::Turret, safe_defense_anchor, defense_builder);
        let orientation = Orientation::for_home(&observation, HOME);
        assert!(policy.combined_build_layout_with_builders_is_safe(
            &observation,
            &public_map,
            &[],
            &[],
            orientation,
            &[foundry_build],
        ));
        assert!(policy.combined_build_layout_with_builders_is_safe(
            &observation,
            &public_map,
            &[],
            &[],
            orientation,
            &[unsafe_defense_build],
        ));
        assert!(!policy.combined_build_layout_with_builders_is_safe(
            &observation,
            &public_map,
            &[],
            &[],
            orientation,
            &[foundry_build, unsafe_defense_build],
        ));
        assert!(policy.combined_build_layout_with_builders_is_safe(
            &observation,
            &public_map,
            &[],
            &[],
            orientation,
            &[foundry_build, safe_defense_build],
        ));

        let original_policy = policy.clone();
        let mut strategy = StrategicPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut input = prepared(&observation, None);
        input.fresh_foundry = Some(FreshFoundryProposal::fixture(
            foundry_anchor,
            foundry_builder,
            foundry_cost,
            0,
            0,
            input.allocation_horizon,
            foundry_case(),
        ));
        let defense_case = ProposalCase {
            urgency: Urgency::Timely,
            confidence: Confidence::Supported,
            value: StrategicValue::Material,
            time_to_impact: TimeToImpact::Near,
            safety: ExecutionSafety::Secure,
        };
        input.fresh_defense = vec![
            FreshDefenseProposal::fixture(
                DefenseConstruction::Turret,
                unsafe_defense_anchor,
                defense_builder,
                defense_case,
                100,
                0,
            ),
            FreshDefenseProposal::fixture(
                DefenseConstruction::Turret,
                safe_defense_anchor,
                defense_builder,
                defense_case,
                100,
                0,
            ),
        ];
        let mut trace = AllocationTrace::default();
        let mut session = AllocationSession::new(
            setup.context(&observation, HOME, &public_map, &intelligence),
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            advanced(snapshots),
            Some(&mut trace),
        );
        let resolved = session.resolve(
            input,
            CommitSnapshots {
                policy: original_policy.speculative_checkpoint(),
            },
        );
        let outcome = session.commit_or_restore(resolved);

        assert!(outcome.allocation_ok, "{trace:#?}");
        assert_eq!(
            outcome.fresh_foundry_intents,
            vec![Intent::BuildWith {
                builder: foundry_builder,
                kind: BuildingKind::Foundry,
                anchor: foundry_anchor,
            }]
        );
        assert_eq!(
            outcome.fresh_defense_intents,
            vec![Intent::BuildWith {
                builder: defense_builder,
                kind: BuildingKind::Turret,
                anchor: safe_defense_anchor,
            }]
        );
        let unsafe_proposal = trace
            .proposals
            .entries
            .iter()
            .find(|proposal| {
                proposal.key
                    == ProposalKeyTrace::Defense {
                        kind: BuildingKind::Turret,
                        anchor: unsafe_defense_anchor,
                    }
            })
            .expect("the unsafe exact defense remains visible in the allocation trace");
        assert!(matches!(
            &unsafe_proposal.disposition,
            ProposalDispositionTrace::ConflictsWithSelected {
                conflict: AllocationConflictTrace::IncompatibleLayout { keys },
                ..
            } if *keys == vec![
                ProposalKeyTrace::FoundryExpansion { anchor: foundry_anchor },
                ProposalKeyTrace::Defense {
                    kind: BuildingKind::Turret,
                    anchor: unsafe_defense_anchor,
                },
            ]
        ));
    }

    #[test]
    fn four_foundations_can_close_an_exit_that_every_triple_preserves() {
        let mut obs = observation();
        let home = TilePos::new(7, 7);
        obs.my_buildings = vec![observed_building(1, 0, BuildingKind::Foundry, home)];
        obs.my_queues = vec![Vec::new()];
        obs.my_queue_progress = vec![0];
        obs.known_scrap.clear();
        obs.known_rock = vec![
            TilePos::new(6, 6),
            TilePos::new(9, 6),
            TilePos::new(6, 9),
            TilePos::new(7, 9),
            TilePos::new(8, 9),
            TilePos::new(9, 9),
        ];
        obs.known_rock.sort_by_key(|tile| (tile.y, tile.x));
        obs.my_units = vec![
            owned_unit(10, UnitKind::Harvester, TilePos::new(11, 7)),
            owned_unit(11, UnitKind::Harvester, TilePos::new(4, 7)),
            owned_unit(12, UnitKind::Harvester, TilePos::new(7, 5)),
            owned_unit(13, UnitKind::Harvester, TilePos::new(8, 5)),
        ];
        let builds = [
            (BuildingKind::Foundry, TilePos::new(9, 7), UnitId(10)),
            (BuildingKind::RepairBay, TilePos::new(5, 7), UnitId(11)),
            (BuildingKind::Turret, TilePos::new(7, 6), UnitId(12)),
            (BuildingKind::Reclaimer, TilePos::new(8, 6), UnitId(13)),
        ];
        let map = briefing();
        let policy = UtilityPolicy::new();
        let orientation = Orientation::for_home(&obs, home);
        for omitted in 0..4 {
            let triple = builds
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != omitted)
                .map(|(_, build)| *build)
                .collect::<Vec<_>>();
            assert!(
                policy.combined_build_layout_with_builders_is_safe(
                    &obs,
                    &map,
                    &[],
                    &[],
                    orientation,
                    &triple
                ),
                "the omitted foundation {omitted} retains an exit"
            );
        }
        assert!(!policy.combined_build_layout_with_builders_is_safe(
            &obs,
            &map,
            &[],
            &[],
            orientation,
            &builds
        ));
    }

    #[test]
    fn active_connected_revision_marginal_preserves_the_voluntary_scrap_guard() {
        const HOME: TilePos = TilePos::new(3, 10);
        let guard = UnitKind::Sentinel.stats().cost;
        let marginal_kind = UnitKind::Kestrel;
        let mut observation = connected_observation(120, marginal_kind.stats().cost);
        observation.my_queues.iter_mut().for_each(Vec::clear);
        observation.my_queue_progress.fill(0);
        let deadline = observation
            .tick
            .saturating_add(Tick::from(marginal_kind.stats().train_ticks))
            .saturating_add(1);
        let proposal = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective: BuildingId(700),
            anchor: TilePos::new(22, 15),
            deadline,
            case: ConnectedOpportunityCase::fixture(
                ConnectedUrgency::Timely,
                ConnectedConfidence::Current,
                ConnectedStrategicValue::Material,
                ConnectedTimeToImpact::Near,
                ConnectedExecutionSafety::Managed,
            ),
            minimum_claims: ConnectedOffenseClaims::fixture(Vec::new(), Vec::new()),
            marginal_additions: vec![ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![ConnectedProviderJob::fixture(
                    marginal_kind,
                    observation.tick,
                    deadline,
                    vec![BuildingId(12)],
                )],
            )],
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        })
        .into_active_revision_fixture();
        let key = ConnectedOffenseKey {
            objective: proposal.objective(),
            anchor: proposal.anchor(),
        };

        let setup = SessionProfile::new(prime_profile());
        let briefing = connected_briefing(&observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let original_policy = policy.clone();
        let mut strategy = StrategicPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut input = prepared(&observation, None);
        input.fresh_connected = Some(proposal);
        input.standing_force = StandingForcePreparation::ConnectedContexts(vec![
            ContextualStandingForce {
                context: ConnectedPortfolioContext::Selected {
                    key,
                    marginal_depth: 0,
                },
                proposals: Vec::new(),
            },
            ContextualStandingForce {
                context: ConnectedPortfolioContext::Selected {
                    key,
                    marginal_depth: 1,
                },
                proposals: Vec::new(),
            },
        ]);
        input.voluntary_scrap_guard = guard;
        input.allocation_horizon = deadline;
        let mut session = AllocationSession::new(
            setup.context(&observation, HOME, &briefing, &intelligence),
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
                policy: original_policy.speculative_checkpoint(),
            },
        );
        let settlement = resolved
            .settlement
            .expect("the guarded active revision has a feasible minimum context");

        assert!(settlement.producer_schedule().is_empty());
        assert_eq!(
            settlement.residual_current_scrap(),
            marginal_kind.stats().cost
        );
        assert!(
            !settlement.voluntary_scrap_guard_satisfied(),
            "the empty active minimum cannot discharge the shallow reserve"
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
        let setup = SessionProfile::new(prime_profile());
        let briefing = connected_briefing(&observation);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let mut policy = UtilityPolicy::new();
        let original_policy = policy.clone();
        let mut strategy = StrategicPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut input = prepared(&observation, None);
        input.resources = ResourceSnapshot::from_observation(&observation);
        input.fresh_connected = Some(fixture_connected_proposal(deadline, Vec::new()));
        input.allocation_horizon = deadline;
        input.voluntary_scrap_guard = UnitKind::Sentinel.stats().cost;
        let mut session = AllocationSession::new(
            setup.context(&observation, TilePos::new(3, 10), &briefing, &intelligence),
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
                policy: original_policy.speculative_checkpoint(),
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
    fn connected_and_lift_keep_one_shared_future_lane_across_the_next_think() {
        for execute_predecessors in [true, false] {
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

            let connected = current_connected_proposal(&observation);
            let mut allocation = CrossDomainAllocation::new(
                &ResourceSnapshot::from_observation(&observation),
                connected.deadline(),
                12,
            )
            .unwrap();
            allocation.offer(connected_investment_proposal(connected).unwrap());
            let settlement = allocation
                .resolve(AllocationPersonality::default(), None)
                .unwrap();
            let connected_assignments = settlement.producer_schedule().to_vec();
            let mut payloads = settlement.into_payloads();
            let mut strategy = StrategicPlanner::new();
            strategy
                .commit_connected_proposal(payloads.take_connected().unwrap())
                .unwrap();
            let shift = 24;
            let airworks = BuildingId(12);
            let lift_starts_at = connected_assignments
                .iter()
                .filter(|assignment| assignment.producer == airworks)
                .map(|assignment| assignment.ready_at)
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

            if execute_predecessors {
                for job in connected_assignments
                    .iter()
                    .filter(|job| job.enqueued_at == 0)
                {
                    let index = observation
                        .my_buildings
                        .iter()
                        .position(|building| building.id == job.producer)
                        .unwrap();
                    observation.my_queues[index].push(job.kind);
                }
                observation.my_queue_progress = observation
                    .my_queues
                    .iter()
                    .map(|queue| if queue.is_empty() { 0 } else { 12 })
                    .collect();
            }
            observation.tick = observation.tick.saturating_add(12);
            let setup = SessionProfile::new(prime_profile());
            let briefing = connected_briefing(&observation);
            let mut intelligence = StrategicIntelligence::new();
            intelligence.update(&observation);
            let mut policy = UtilityPolicy::new();
            let mut lifts = lift;
            let mut team = TeamReliefPlanner::new();
            let mut raids = RaidPlanner::new();
            let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
            let mut work = advanced(snapshots);
            work.lift_was_active = true;
            work.lift_started_at = lift_operation.started_at;
            let mut trace = AllocationTrace::default();
            let outcome = AllocationSession::new(
                setup.context(&observation, HOME, &briefing, &intelligence),
                AllocationParticipants {
                    policy: &mut policy,
                    strategy: &mut strategy,
                    lifts: &mut lifts,
                    team: &mut team,
                    raids: &mut raids,
                },
                work,
                Some(&mut trace),
            )
            .run();

            assert!(outcome.allocation_ok, "{trace:#?}");
            let retained_connected = connected_obligation(&mut strategy, &observation);
            assert!(retained_connected.deadline() > observation.tick);
            if !execute_predecessors {
                assert!(
                    lifts.active_production_obligation().is_none(),
                    "a fixed Lift booking whose necessary predecessors never ran must recover instead of freezing every later decision"
                );
                continue;
            }
            let retained_lift = (lifts)
                .active_production_obligation()
                .expect("the future Lift assignment remains mandatory");
            assert_eq!(retained_lift.producer_jobs(), &[lift_assignment]);
            assert_eq!(
                lifts.operation().unwrap().phase,
                LiftPhase::Provision,
                "accepted unpaid carrier work keeps the Lift in Provision"
            );
        }
    }

    #[test]
    fn contextual_standing_force_reuses_ownership_and_keeps_first_context_demand() {
        use crate::observer::{BotPhase, PhaseObserver};
        use std::cell::Cell;

        #[derive(Default)]
        struct Derivations(Cell<usize>);
        impl PhaseObserver for Derivations {
            fn enter(&self, phase: BotPhase) {
                if phase == BotPhase::StandingForce {
                    self.0.set(self.0.get() + 1);
                }
            }
            fn exit(&self, _phase: BotPhase) {}
        }

        let observation = connected_inventory_transfer_observation(300, 5);
        let briefing = connected_briefing(&observation);
        let profile = prime_profile();
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observation);
        let resources = ResourceSnapshot::from_observation(&observation);
        let derivation = StandingForceDerivation::default();
        let observer = Derivations::default();
        let inputs = StandingForceInputs {
            observer: Some(&observer),
            observation: &observation,
            intelligence: &intelligence,
            profile: &profile,
            tuning: DifficultyTuning::for_level(profile.difficulty),
            resources: &resources,
            home: TilePos::new(3, 10),
            public_map: &briefing,
            orientation: Orientation::for_home(&observation, TilePos::new(3, 10)),
            eligible: true,
            derivation: &derivation,
            committed_production: &[],
            funded_repairers: Vec::new(),
            saving: None,
            recon_demands: &[],
        };
        let proposal = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective: BuildingId(700),
            anchor: TilePos::new(22, 15),
            deadline: 3_000,
            case: ConnectedOpportunityCase::fixture(
                ConnectedUrgency::Timely,
                ConnectedConfidence::Current,
                ConnectedStrategicValue::Material,
                ConnectedTimeToImpact::Near,
                ConnectedExecutionSafety::Managed,
            ),
            minimum_claims: ConnectedOffenseClaims::fixture(Vec::new(), Vec::new()),
            marginal_additions: vec![
                ConnectedOffenseClaims::fixture(Vec::new(), Vec::new()),
                ConnectedOffenseClaims::fixture(vec![UnitId(201)], Vec::new()),
            ],
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });
        let first = inputs.derive(&[], &[]);
        let claimed = inputs.derive(&[UnitId(201)], &[]);
        assert_ne!(
            first.0, claimed.0,
            "owning the Bombard must change replacement demand"
        );
        for revision in [false, true] {
            let proposal = if revision {
                proposal.clone().into_active_revision_fixture()
            } else {
                proposal.clone()
            };
            observer.0.set(0);
            let (prepared, demands) = inputs.prepare(&[], Some(&proposal));
            let StandingForcePreparation::ConnectedContexts(contexts) = prepared else {
                panic!("connected ownership requires contextual alternatives");
            };
            assert_eq!(
                observer.0.get(),
                2,
                "evaluate each distinct ownership only once"
            );
            assert_eq!(demands, first.1);
            let key = ConnectedOffenseKey {
                objective: proposal.objective(),
                anchor: proposal.anchor(),
            };
            let expected = (!revision)
                .then_some(ConnectedPortfolioContext::Absent)
                .into_iter()
                .chain(
                    (0..3).map(|marginal_depth| ConnectedPortfolioContext::Selected {
                        key,
                        marginal_depth,
                    }),
                )
                .collect::<Vec<_>>();
            assert_eq!(
                contexts
                    .iter()
                    .map(|entry| entry.context)
                    .collect::<Vec<_>>(),
                expected
            );
            for entry in &contexts[..contexts.len() - 1] {
                assert_eq!(entry.proposals, first.0);
            }
            assert_eq!(contexts.last().unwrap().proposals, claimed.0);
        }
    }

    #[test]
    fn connected_live_claim_competes_with_raid_preparation_and_standing_replacement() {
        let observation = connected_inventory_transfer_observation(300, 5);
        let mut policy = UtilityPolicy::new();
        let mut strategy = StrategicPlanner::new();
        let mut trace = AllocationTrace::default();

        let outcome = run_connected_session_with_team_decision_and_trace(
            &observation,
            &mut policy,
            &mut strategy,
            StrategicDecision::default(),
            Some(&mut trace),
        );

        assert!(!outcome.accepted_connected);
        assert_eq!(
            outcome.allocated_producer_intents,
            vec![
                Intent::TrainAt {
                    building: BuildingId(10),
                    kind: UnitKind::Scuttler
                },
                Intent::TrainAt {
                    building: BuildingId(10),
                    kind: UnitKind::Scuttler
                },
            ],
            "raid preparation competes for the small bank with every production planner present"
        );

        let observation = connected_inventory_transfer_observation(400, 5);
        let mut policy = UtilityPolicy::new();
        let mut strategy = StrategicPlanner::new();
        let mut richer_trace = AllocationTrace::default();
        let outcome = run_connected_session_with_team_decision_and_trace(
            &observation,
            &mut policy,
            &mut strategy,
            StrategicDecision::default(),
            Some(&mut richer_trace),
        );

        assert!(outcome.accepted_connected, "{richer_trace:#?}");
        assert!(
            connected_obligation(&mut strategy, &observation)
                .units()
                .contains(&UnitId(201)),
            "the admitted package must own the live Bombard that created replacement demand"
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
        let mut strategy = StrategicPlanner::new();
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
                    crate::trace::ProposalKeyTrace::StandingForce {
                        kind: UnitKind::Lancer,
                        ..
                    }
                ) && matches!(
                    proposal.disposition,
                    crate::trace::ProposalDispositionTrace::Accepted
                )
            })
            .expect("the selected Connected context accepts the replacement demand");
        assert_eq!(lancer.case.urgency, crate::trace::UrgencyTrace::Timely);
    }
}
