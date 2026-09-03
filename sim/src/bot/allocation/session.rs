//! One atomic player-facing allocation transaction.
//!
//! The domain planners still decide what work is worth doing. This module owns
//! the lifecycle around those decisions: assemble exact prior claims, resolve
//! the shared portfolio once, then either commit every accepted payload or
//! restore every speculative planner mutation.

use super::{
    AllocationPersonality, ClaimBundle, ClaimBundleError, ClaimOwner, CoordinatorInputError,
    CrossDomainAllocation, CrossDomainSettlement, ForecastClaim, ImportedObligation, LegacyChannel,
    ObligationClass, ObligationKey, active_connected_obligation,
    active_connected_producer_assignments, clamped_current_reserve_obligation,
    connected_investment_proposal, connected_producer_assignments, current_reserve_at,
    forecast_reserve_through, foundry_investment_proposal, imported_obligation,
    legacy_decision_obligation, legacy_unit_obligation, observed_builder_obligations,
    saved_foundry_obligation,
};
use crate::bot::PublicMapBriefing;
use crate::bot::difficulty::{DifficultyTuning, strategic_admission_tick};
use crate::bot::executive::Intent;
use crate::bot::intelligence::StrategicIntelligence;
use crate::bot::lift::{LiftAdmission, LiftAirSupport, LiftOperation, LiftPlanner};
use crate::bot::observation::Observation;
use crate::bot::orient::Orientation;
use crate::bot::profile::ResolvedProfile;
use crate::bot::raid::RaidPlanner;
use crate::bot::resources::{ProducerLaneReservations, ResourceSnapshot};
use crate::bot::strategy::{
    ActiveConnectedObligation, FreshConnectedProposal, LiftSupportRequest,
    RejectedConnectedCandidate, StrategicCoordination, StrategicDecision, StrategicPlanner,
    StrategicThinkResult, connected_preparation_horizon,
};
use crate::bot::team::TeamReliefPlanner;
use crate::bot::trace::{
    AllocationCoordinatorFailureReasonTrace, AllocationCoordinatorStageTrace, AllocationTrace,
};
use crate::bot::utility::{
    CombatCoreStatus, Dials, FreshFoundryProposal, FreshFoundryProposalContext, UtilityPolicy,
    ValidatedFoundryObligation, combat_core_status,
};
use crate::ids::UnitId;
use chassis::Tick;
use chassis::grid::TilePos;

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
    pub(crate) shallow_sentinel: u32,
    pub(crate) opening_bootstrap: u32,
    pub(crate) residual_scrap: u32,
    pub(crate) connected_spendable: u32,
    pub(crate) connected_forecast_hold: u32,
    pub(crate) utility_spendable: u32,
    pub(crate) prior_operation_spendable: u32,
}

impl AllocationBudgetOutcome {
    fn frozen(
        foundry_saving: u32,
        airworks_capacity: u32,
        shallow_sentinel: u32,
        opening_bootstrap: u32,
    ) -> Self {
        Self {
            foundry_saving,
            airworks_capacity,
            shallow_sentinel,
            opening_bootstrap,
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
    pub(crate) fresh_foundry_intents: Vec<Intent>,
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
        let prepared = self.prepare();
        let resolved = self.resolve(prepared);
        self.commit_or_restore(resolved)
    }

    /// Assembles one immutable resource picture, imports exact prior work, and
    /// asks each migrated domain for at most one already-ranked proposal.
    fn prepare(&mut self) -> PreparedAllocation {
        let mut claims = self.snapshot_claims();
        let mut obligations = self.collect_legacy_obligations(&claims);
        let air_lift = self.prepare_air_and_lift(&claims, &mut obligations);
        self.stage_active_island(&mut claims, &mut obligations);
        let saved = self.prepare_saved_foundry(&claims, &air_lift, &mut obligations);
        let fresh = self.prepare_fresh_investments(&claims, &saved, &mut obligations);
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
            saved_foundry: saved.obligation,
            fresh_foundry: fresh.foundry,
            fresh_connected: fresh.connected,
            connected_accepted_at: fresh.connected_accepted_at,
            connected_reserve_deadline: fresh.connected_reserve_deadline,
            allocation_horizon,
            active_lift_precedes_foundry: air_lift.active_lift_precedes_foundry,
            active_lift_spendable: air_lift.active_lift_spendable,
            foundry_saving: saved.saving,
            airworks_capacity: air_lift.airworks_capacity,
            shallow_sentinel: air_lift.shallow_sentinel,
            opening_bootstrap: air_lift.opening_bootstrap,
            rejected_connected_candidate: fresh.rejected_connected_candidate,
            staged_strategy: obligations.staged_strategy,
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

    fn collect_legacy_obligations(&self, claims: &ClaimSnapshot) -> ObligationPreparation {
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

        let current_claims = PlannerClaims::new(
            self.context.enlisted,
            self.participants.strategy,
            self.participants.raids,
            self.participants.lifts,
        );
        let planner_owned = current_claims.without_executive(&claims.team_core_claims);
        let standing_army = self
            .context
            .enlisted
            .iter()
            .copied()
            .filter(|unit| planner_owned.binary_search(unit).is_err())
            .collect();
        retain_first_coordinator_failure(
            &mut coordinator_failure,
            AllocationCoordinatorStageTrace::ObligationCollection,
            push_obligation(
                &mut obligations,
                legacy_unit_obligation(
                    self.context.observation.tick,
                    LegacyChannel::StandingArmy,
                    0,
                    standing_army,
                ),
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
                    retained_at: self.advanced.team_started_at,
                    channel: LegacyChannel::TeamRelief,
                    decision: &self.advanced.team_decision,
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

        let active_connected = self
            .participants
            .strategy
            .as_ref()
            .and_then(|planner| planner.active_connected_obligation(self.context.observation));
        let legacy_air_claims = if let Some(active) = active_connected.as_ref() {
            retain_first_coordinator_failure(
                &mut coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                push_obligation(&mut obligations, active_connected_obligation(active)),
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

        ObligationPreparation {
            resources,
            obligations,
            coordinator_failure,
            active_connected,
            legacy_air_claims,
            staged_strategy: None,
        }
    }

    fn prepare_air_and_lift(
        &mut self,
        claims: &ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) -> AirLiftPreparation {
        let mut shallow_sentinel = 0;
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
            shallow_sentinel = self.participants.policy.shallow_sentinel_capital_reserve(
                self.context.dials,
                self.context.observation,
                self.context.home,
                self.context.public_map,
            );
            opening_bootstrap = self
                .participants
                .policy
                .strategic_opening_bootstrap_reserve(
                    self.context.dials,
                    self.context.observation,
                    self.context.home,
                    self.context.public_map,
                );
            for (sequence, amount) in [(1, shallow_sentinel), (2, opening_bootstrap)] {
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
                &claims.planner_claims,
            )
        } else {
            0
        };
        let nominal_foundry_saving = self
            .participants
            .policy
            .validated_foundry_saving(self.context.observation, claims.opening_core.ready);
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
                &claims.planner_claims,
            )
        } else {
            0
        };
        if lift_airworks_capacity > 0 {
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
                        desired: lift_airworks_capacity,
                        forecast_deadline: lift_deadline,
                        older_capital_reserve: if !active_lift_precedes_foundry {
                            nominal_foundry_saving
                        } else {
                            0
                        },
                    },
                ),
            );
        }

        let active_lift_spendable = if self.advanced.lift_was_active {
            self.context
                .observation
                .scrap
                .saturating_sub(current_reserve_at(
                    &obligations.obligations,
                    self.context.observation.tick,
                ))
                .saturating_sub(if !active_lift_precedes_foundry {
                    nominal_foundry_saving
                } else {
                    0
                })
        } else {
            0
        };
        let mut lift_decision = StrategicDecision::default();
        if self.advanced.lift_was_active {
            lift_decision = self
                .participants
                .lifts
                .as_mut()
                .expect("an active lift planner exists")
                .think_with_admission(
                    self.context.observation,
                    self.context.home,
                    &self.advanced.lift_unavailable,
                    self.advanced.initial_lift_support,
                    LiftAdmission {
                        allow_new_commitments: self.advanced.preliminary_core.ready,
                        spendable_scrap: active_lift_spendable,
                        core_reservations: &self.advanced.preliminary_core_exclusions,
                        minimum_core_equivalents: u64::from(
                            self.context.dials.minimum_core_equivalents,
                        ),
                    },
                );
        }
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
                    decision: &lift_decision,
                    retained_units: self
                        .participants
                        .lifts
                        .as_ref()
                        .and_then(LiftPlanner::operation)
                        .map_or_else(Vec::new, |operation| {
                            observable_lift_operation_reservations(
                                operation,
                                self.context.observation,
                            )
                        }),
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

        let saved_plan_reserve_already_imported = if claims.opening_core.ready {
            shallow_sentinel.saturating_add(opening_bootstrap)
        } else {
            claims
                .opening_core
                .missing_scrap
                .min(self.context.observation.scrap)
        };
        AirLiftPreparation {
            lift_decision,
            shallow_sentinel,
            opening_bootstrap,
            airworks_capacity,
            active_lift_precedes_foundry,
            active_lift_spendable,
            saved_plan_reserve_already_imported,
        }
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
        let obligation = self.participants.policy.validated_foundry_obligation(
            self.context.observation,
            &obligations.resources,
            claims.opening_core.ready,
            current_before_saved,
        );
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
        }
    }

    fn stage_active_island(
        &mut self,
        claims: &mut ClaimSnapshot,
        obligations: &mut ObligationPreparation,
    ) {
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
        let Some(result) = self.participants.strategy.as_mut().and_then(|planner| {
            planner.stage_active_island(
                self.context.profile,
                self.context.tuning,
                self.context.observation,
                self.context.intelligence,
                self.context.home,
                StrategicCoordination {
                    enlisted: &claims.planner_claims,
                    lift_support: self.context.lift_support,
                    allow_new_operation: true,
                    protected_current_scrap,
                    protected_forecast_scrap,
                    public_map: Some(self.context.public_map),
                    orientation: self.context.orientation,
                },
            )
        }) else {
            return;
        };
        let accepted_at = accepted_at.expect("a staged island operation has an admission tick");
        let retained_units = self
            .participants
            .strategy
            .as_ref()
            .and_then(StrategicPlanner::air_operation)
            .map_or_else(Vec::new, |operation| {
                prior_planner_claims(&[], Some(operation), &[], &[], None)
            });
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
                    retained_units,
                    production_deadline,
                },
            ),
        );
        let refreshed = PlannerClaims::new(
            self.context.enlisted,
            self.participants.strategy,
            self.participants.raids,
            self.participants.lifts,
        );
        claims.planner_claims = refreshed.all(&claims.team_core_claims);
        claims.strategic_core_exclusions = refreshed.core_exclusions(&claims.team_core_claims);
        obligations.staged_strategy = Some(result);
    }

    fn prepare_fresh_investments(
        &mut self,
        claims: &ClaimSnapshot,
        saved: &SavedFoundryPreparation,
        obligations: &mut ObligationPreparation,
    ) -> FreshInvestmentPreparation {
        let admission_tick = strategic_admission_tick(self.context.observation.tick)
            && claims.opening_core.ready
            && !saved.blocked
            && obligations.coordinator_failure.is_none();
        let available_builders =
            available_allocation_builders(&obligations.resources, &obligations.obligations);
        let foundry = admission_tick
            .then(|| {
                self.participants.policy.fresh_foundry_proposal(
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

        let outstanding_lift_airwork = self.participants.lifts.as_ref().map_or(0, |planner| {
            planner
                .remaining_airwork_ticks(self.context.observation, &self.advanced.lift_unavailable)
        });
        let mut rejected_connected_candidate = None;
        let connected = if admission_tick
            && outstanding_lift_airwork == 0
            && obligations.staged_strategy.is_none()
        {
            self.participants.strategy.as_ref().and_then(|planner| {
                match planner.fresh_connected_minimum_proposal(
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
                ) {
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
        let connected_accepted_at = obligations
            .active_connected
            .as_ref()
            .map(ActiveConnectedObligation::accepted_at)
            .or_else(|| connected.as_ref().map(FreshConnectedProposal::accepted_at));
        if connected.is_none()
            && let Some((accepted_at, air_claims)) = obligations.legacy_air_claims.take()
        {
            retain_first_coordinator_failure(
                &mut obligations.coordinator_failure,
                AllocationCoordinatorStageTrace::ObligationCollection,
                push_obligation(
                    &mut obligations.obligations,
                    legacy_unit_obligation(
                        accepted_at,
                        LegacyChannel::AirworksCapacity,
                        0,
                        air_claims,
                    ),
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
        FreshInvestmentPreparation {
            foundry,
            connected,
            connected_accepted_at,
            connected_reserve_deadline,
            rejected_connected_candidate,
        }
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
        horizon
    }

    /// Resolves the complete prepared portfolio once. Domain payloads stay
    /// opaque and retain their exact original ranking and identity.
    fn resolve(&mut self, mut prepared: PreparedAllocation) -> ResolvedAllocation {
        let snapshots = CommitSnapshots {
            policy: self.participants.policy.clone(),
        };
        let mut allocation_ok = prepared.coordinator_failure.is_none();
        let mut settlement = None;
        if allocation_ok {
            match CrossDomainAllocation::new(
                &prepared.resources,
                prepared.allocation_horizon,
                self.context.dials.cadence,
            ) {
                Ok(mut allocation) => {
                    for obligation in prepared.obligations.iter().cloned() {
                        allocation.import(obligation);
                    }
                    if let Some(proposal) = prepared.fresh_foundry.take() {
                        match foundry_investment_proposal(proposal) {
                            Ok(proposal) => allocation.offer(proposal),
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
                        match connected_investment_proposal(proposal) {
                            Ok(proposal) => allocation.offer(proposal),
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
        effects.budget.residual_scrap = settlement.residual_current_scrap();
        effects.budget.connected_spendable = settlement.connected_current_scrap();
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
        let producer_schedule = settlement.producer_schedule().to_vec();
        effects.producer_lane_reservations = settlement.producer_lane_reservations().clone();

        self.bind_saved_foundry_funding(prepared, &settlement, allocation_ok);
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
        effects
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
        prepared.saved_foundry = settlement.capital_assignment(owner).and_then(|assignment| {
            saved.with_allocated_funding(assignment.current_scrap, assignment.forecast_scrap)
        });
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
        let assignments = connected_producer_assignments(&connected, producer_schedule);
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
            fresh_foundry_intents: effects.fresh_foundry_intents,
            allocation_ok,
            accepted_connected: effects.accepted_connected,
            producer_lane_reservations: effects.producer_lane_reservations,
            budget: effects.budget,
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
    legacy_air_claims: Option<(Tick, Vec<UnitId>)>,
    staged_strategy: Option<StrategicThinkResult>,
}

struct AirLiftPreparation {
    lift_decision: StrategicDecision,
    shallow_sentinel: u32,
    opening_bootstrap: u32,
    airworks_capacity: u32,
    active_lift_precedes_foundry: bool,
    active_lift_spendable: u32,
    saved_plan_reserve_already_imported: u32,
}

struct SavedFoundryPreparation {
    obligation: Option<ValidatedFoundryObligation>,
    saving: u32,
    blocked: bool,
}

struct FreshInvestmentPreparation {
    foundry: Option<FreshFoundryProposal>,
    connected: Option<FreshConnectedProposal>,
    connected_accepted_at: Option<Tick>,
    connected_reserve_deadline: Tick,
    rejected_connected_candidate: Option<RejectedConnectedCandidate>,
}

struct CommitEffects {
    accepted_connected: bool,
    producer_lane_reservations: ProducerLaneReservations,
    fresh_foundry_intents: Vec<Intent>,
    budget: AllocationBudgetOutcome,
}

impl CommitEffects {
    fn frozen(prepared: &PreparedAllocation) -> Self {
        Self {
            accepted_connected: false,
            producer_lane_reservations: ProducerLaneReservations::default(),
            fresh_foundry_intents: Vec::new(),
            budget: AllocationBudgetOutcome::frozen(
                prepared.foundry_saving,
                prepared.airworks_capacity,
                prepared.shallow_sentinel,
                prepared.opening_bootstrap,
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
    saved_foundry: Option<ValidatedFoundryObligation>,
    fresh_foundry: Option<FreshFoundryProposal>,
    fresh_connected: Option<FreshConnectedProposal>,
    connected_accepted_at: Option<Tick>,
    connected_reserve_deadline: Tick,
    allocation_horizon: Tick,
    active_lift_precedes_foundry: bool,
    active_lift_spendable: u32,
    foundry_saving: u32,
    airworks_capacity: u32,
    shallow_sentinel: u32,
    opening_bootstrap: u32,
    rejected_connected_candidate: Option<RejectedConnectedCandidate>,
    staged_strategy: Option<StrategicThinkResult>,
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
        retained_units,
        production_deadline,
    } = claim;
    let retained_valid = if retained_units.is_empty() {
        Ok(())
    } else {
        push_obligation(
            obligations,
            legacy_unit_obligation(retained_at, channel, 0, retained_units),
        )
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
        cadence,
        accepted_at,
        decision_at,
        channel,
        1,
        decision,
        production_deadline,
    );
    retained_valid?;
    push_coordinator_obligation(obligations, immediate)
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
    use super::*;
    use crate::bot::briefing::PublicMapBriefing;
    use crate::bot::observation::BuildingObs;
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
            saved_foundry: None,
            fresh_foundry: None,
            fresh_connected: None,
            connected_accepted_at: None,
            connected_reserve_deadline: 120,
            allocation_horizon: 120,
            active_lift_precedes_foundry: false,
            active_lift_spendable: 0,
            foundry_saving: 11,
            airworks_capacity: 12,
            shallow_sentinel: 13,
            opening_bootstrap: 14,
            rejected_connected_candidate: None,
            staged_strategy: None,
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
        assert!(outcome.fresh_foundry_intents.is_empty());
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
}
