//! Ordered admission from observed ownership through the final utility grant.

use super::operations::{
    OperationContext, OperationParticipants, OperationSettlement, RetainedDecisions,
    lift_unavailable, remove_producer_intents, settle_operations,
};
use super::{
    AllocationBudgetOutcome, AllocationParticipants, AllocationSession, AllocationSessionContext,
    AllocationSessionOutcome, PlannerClaims, RetainedOperationWork,
};
use crate::{
    PublicMapBriefing,
    difficulty::DifficultyTuning,
    executive::{Army, Intent},
    intelligence::StrategicIntelligence,
    lift::LiftPlanner,
    observation::Observation,
    observer::PhaseObserver,
    orient::Orientation,
    profile::ResolvedProfile,
    raid::{RaidPlanner, RaidPlanningContext},
    resources::ProducerLaneReservations,
    strategy::{
        AirOperationPhase, LiftSupportRequest, StrategicCoordination, StrategicDecision,
        StrategicPlanner, StrategicThinkContext,
    },
    team::TeamReliefPlanner,
    trace::{
        ChannelPhase, ChannelState, ChannelTrace, CoreGateTrace, DecisionTraceRecorder,
        RaidAttentionTrace, ScrapBudgetTrace, bounded_count, channel_effects,
        connected_force_trace,
    },
    utility::{
        DecisionEvidence, Dials, FoundryHandoff, StrategicUtilityContext, UtilityPolicy,
        combat_core_status,
    },
};
use chassis::grid::TilePos;
use oxide_sim::ids::UnitId;

pub(crate) struct DecisionContext<'a> {
    pub evidence: DecisionEvidence<'a>,
    pub dials: &'a Dials,
    pub profile: &'a ResolvedProfile,
    pub tuning: DifficultyTuning,
    pub observation: &'a Observation,
    pub home: TilePos,
    pub public_map: &'a PublicMapBriefing,
    pub orientation: Orientation,
    pub armies: &'a [Army],
    pub enlisted: &'a [UnitId],
}

pub(crate) struct AdmittedWork {
    pub intents: Vec<Intent>,
    pub reservations: Vec<UnitId>,
    pub utility: UtilityGrant,
}

/// The only remaining capital, actors, and producer access utility may use.
/// Future queue reservations remain timing constraints, never paid inventory.
pub(crate) struct UtilityGrant {
    pub reservations: Vec<UnitId>,
    core_exclusions: Vec<UnitId>,
    prior_commitment: u32,
    foundry: FoundryHandoff,
    voluntary_guard: u32,
    producer_lanes: ProducerLaneReservations,
}

impl UtilityGrant {
    pub(crate) fn context<'a>(
        &'a self,
        intents: Vec<Intent>,
        intelligence: &'a StrategicIntelligence,
        public_map: &'a PublicMapBriefing,
        evidence: DecisionEvidence<'a>,
    ) -> StrategicUtilityContext<'a> {
        StrategicUtilityContext::new(
            &self.reservations,
            intelligence.units(),
            intelligence.buildings(),
            public_map,
            intents,
            evidence,
        )
        .with_combat_core_exclusions(&self.core_exclusions)
        .with_prior_scrap_commitment(self.prior_commitment)
        .with_foundry_handoff(self.foundry)
        .with_voluntary_scrap_guard(self.voluntary_guard)
        .with_producer_lane_reservations(&self.producer_lanes)
    }
}

pub(crate) fn admit_decision(
    context: DecisionContext<'_>,
    participants: AllocationParticipants<'_>,
    intelligence: &mut StrategicIntelligence,
    mut recorder: Option<&mut DecisionTraceRecorder>,
    observer: Option<&dyn PhaseObserver>,
) -> AdmittedWork {
    let DecisionContext {
        evidence,
        dials,
        profile,
        tuning,
        observation: oriented,
        home: oriented_home,
        public_map: oriented_public_map,
        orientation,
        armies,
        enlisted,
    } = context;
    let AllocationParticipants {
        policy,
        strategy,
        lifts,
        team,
        raids,
    } = participants;
    let team_before_state = recorder.is_some().then(|| team_channel_state(team));
    let air_before_state = recorder.is_some().then(|| air_channel_state(strategy));
    let lift_before_state = recorder.is_some().then(|| lift_channel_state(lifts));
    let raid_before_state = recorder.is_some().then(|| raid_channel_state(raids));

    raids.reconcile_procurement_routes(oriented, Some(oriented_public_map), Some(orientation));

    // Maintenance on accepted owners survives unrelated allocation rejection.
    let team_was_active = team.operation().is_some();
    let lift_was_active = lifts.operation().is_some();
    let raid_was_active = raids.operation().is_some();
    let team_started_at = team
        .operation()
        .map(|operation| operation.started_at)
        .unwrap_or(oriented.tick);
    let lift_started_at = lifts
        .operation()
        .map(|operation| operation.started_at)
        .unwrap_or(oriented.tick);
    let raid_started_at = raids
        .operation()
        .map(|operation| operation.started_at)
        .unwrap_or(oriented.tick);

    let initial_claims = PlannerClaims::new(enlisted, strategy, raids, lifts);
    let mut initial_team_external = initial_claims.external_to_team();
    initial_team_external.extend(policy.state.reconnaissance.reservations());
    initial_team_external.extend(policy.support_reservations());
    initial_team_external.sort_unstable();
    initial_team_external.dedup();
    let team_decision = team.maintain(
        profile,
        tuning,
        oriented,
        oriented_home,
        &initial_team_external,
    );

    intelligence.update(oriented);
    strategy.observe_operation(profile, oriented, intelligence);
    lifts.observe_operation(oriented);
    policy.refresh_allocation_worker_safety(
        oriented,
        intelligence.units(),
        intelligence.buildings(),
    );
    let lift_support_request = lifts
        .operation()
        .filter(|operation| operation.phase <= crate::lift::LiftPhase::AwaitSupport)
        .map(|operation| LiftSupportRequest {
            player: operation.target_player,
            target: operation.target,
            planned_drops: operation.planned_drops.clone(),
        });
    let claims_after_team = PlannerClaims::new(enlisted, strategy, raids, lifts);
    let team_claims = team.reservations();
    let mut prior_non_lift_claims = claims_after_team.without_lift(&team_claims);
    prior_non_lift_claims.extend(policy.state.reconnaissance.reservations());
    prior_non_lift_claims.extend(policy.support_reservations());
    prior_non_lift_claims.sort_unstable();
    prior_non_lift_claims.dedup();
    let lift_unavailable_before =
        lift_unavailable(oriented, armies, enlisted, &prior_non_lift_claims);
    let mut preliminary_core_exclusions =
        claims_after_team.core_exclusions(&team.core_reservations());
    preliminary_core_exclusions.extend(policy.state.reconnaissance.reservations());
    preliminary_core_exclusions.extend(policy.support_reservations());
    let preliminary_core = combat_core_status(
        oriented,
        &preliminary_core_exclusions,
        &[],
        u64::from(dials.minimum_core_equivalents),
    );
    let mut raid_exclusions =
        PlannerClaims::new(enlisted, strategy, raids, lifts).without_raid(&team_claims);
    raid_exclusions.extend(policy.state.reconnaissance.reservations());
    raid_exclusions.extend(policy.support_reservations());
    raid_exclusions.sort_unstable();
    raid_exclusions.dedup();
    let raid_decision = if raid_was_active {
        raids.think_with_admission(
            RaidPlanningContext::new(
                profile,
                tuning,
                oriented,
                oriented_home,
                enlisted,
                &raid_exclusions,
            )
            .with_admission(false),
        )
    } else {
        StrategicDecision::default()
    };

    let connected_force_before = recorder
        .is_some()
        .then(|| connected_force_trace(strategy, intelligence, None));
    let allocation_outcome = AllocationSession::new(
        AllocationSessionContext {
            evidence,
            dials,
            profile,
            tuning,
            observation: oriented,
            home: oriented_home,
            public_map: oriented_public_map,
            orientation,
            intelligence,
            enlisted,
            lift_support: lift_support_request.as_ref(),
        },
        AllocationParticipants {
            policy,
            strategy,
            lifts,
            team,
            raids,
        },
        RetainedOperationWork {
            team_decision,
            raid_decision,
            team_started_at,
            lift_started_at,
            raid_started_at,
            lift_was_active,
            lift_unavailable: lift_unavailable_before,
            preliminary_core,
            preliminary_core_exclusions,
        },
        recorder
            .as_deref_mut()
            .map(|recorder| &mut recorder.trace_mut().allocation),
    )
    .with_observer(observer)
    .run();
    let AllocationSessionOutcome {
        opening_core,
        allow_new_voluntary_operations,
        team_decision,
        raid_decision,
        planner_claims,
        connected_continues,
        connected_accepted_at,
        mut rejected_connected_candidate,
        island_allocated,
        fresh_emergency_defense_intents,
        fresh_foundry_intents,
        fresh_defense_intents,
        maintenance_intents,
        fresh_economy_intents,
        allocated_producer_intents,
        allocation_ok,
        accepted_connected,
        producer_lane_reservations,
        foundry_handoff,
        budget:
            AllocationBudgetOutcome {
                foundry_saving,
                airworks_capacity,
                opening_bootstrap,
                raw_residual_scrap,
                residual_scrap,
                connected_spendable,
                connected_forecast_hold,
                utility_spendable: allocation_utility_spendable,
                prior_operation_spendable,
                voluntary_scrap_guard,
            },
    } = allocation_outcome;
    if let Some(recorder) = recorder.as_deref_mut() {
        recorder.trace_mut().gates.opening_core = Some(CoreGateTrace {
            projected_strength: opening_core.projected_strength,
            target_strength: opening_core.target_strength,
            missing_strength: opening_core.missing_strength,
            missing_scrap: opening_core.missing_scrap,
            ready: opening_core.ready,
        });
    }
    let fresh_defense_builders = fresh_defense_intents
        .iter()
        .chain(&maintenance_intents)
        .chain(&fresh_economy_intents)
        .filter_map(|intent| match intent {
            Intent::BuildWith { builder, .. } => Some(*builder),
            _ => None,
        })
        .chain(policy.economic_saving().and_then(|saving| saving.builder))
        .collect::<Vec<_>>();
    let mut air_external =
        PlannerClaims::new(enlisted, strategy, raids, lifts).without_air(&team.reservations());
    air_external.extend(policy.state.reconnaissance.reservations());
    air_external.extend(policy.support_reservations());
    let strategic_result = {
        let continue_connected = connected_continues;
        strategy.think_after_connected_adjudication(
            StrategicThinkContext::new(
                profile,
                tuning,
                oriented,
                intelligence,
                oriented_home,
                StrategicCoordination {
                    planning: Some(&policy.planning),
                    enlisted: &planner_claims,
                    lift_support: lift_support_request.as_ref(),
                    allow_new_operation: allocation_ok
                        && (continue_connected || allow_new_voluntary_operations),
                    protected_current_scrap: oriented.scrap.saturating_sub(connected_spendable),
                    protected_forecast_scrap: connected_forecast_hold,
                    public_map: Some(oriented_public_map),
                    orientation,
                },
            )
            .with_external_claims(&air_external)
            .with_owned_members(
                !allocation_ok || connected_continues || island_allocated || accepted_connected,
            )
            .with_producer_lanes(&allocated_producer_intents, &producer_lane_reservations)
            .with_paid_exclusions(&policy.state.reconnaissance.paid_exclusions()),
        )
    };
    if rejected_connected_candidate.is_none() {
        rejected_connected_candidate = strategic_result.rejected_connected_candidate;
    }
    let air_decision_for_trace = strategic_result.decision.clone();
    let mut strategic = strategic_result.decision.into();
    if island_allocated || connected_continues || accepted_connected {
        remove_producer_intents(&mut strategic);
    }
    strategic.intents.splice(0..0, fresh_defense_intents);
    strategic.intents.splice(0..0, fresh_economy_intents);
    strategic.intents.splice(0..0, maintenance_intents);
    strategic.intents.splice(0..0, fresh_foundry_intents);
    strategic
        .intents
        .splice(0..0, fresh_emergency_defense_intents);
    let connected_is_typed = accepted_connected
        || connected_continues
        || (island_allocated && allocation_ok)
        || (strategy
            .air_operation()
            .is_some_and(|op| op.assault_admitted())
            && strategy.connected_deadline().is_some());
    let mut utility_reservations = policy.state.reconnaissance.reservations();
    utility_reservations.extend(policy.support_reservations());
    utility_reservations.sort_unstable();
    utility_reservations.dedup();
    let OperationSettlement {
        strategic,
        team_decision,
        lift_decision,
        raid_decision,
        team_relief_core_ready,
        lift_rejected,
        raid_attention,
        prospective_carrier_hold,
        utility_prior_commitment,
        utility_spendable,
    } = settle_operations(
        OperationContext {
            cadence: dials.cadence,
            profile,
            tuning,
            observation: oriented,
            intelligence,
            home: oriented_home,
            armies,
            enlisted,
            utility_reservations: &utility_reservations,
            minimum_core_equivalents: u64::from(dials.minimum_core_equivalents),
            allocation_ok,
            allow_new_voluntary_operations,
            connected_is_typed,
            raw_residual_scrap,
            residual_scrap,
            allocation_utility_spendable,
            producer_lanes: &producer_lane_reservations,
        },
        OperationParticipants {
            strategy,
            lifts,
            team,
            raids,
        },
        RetainedDecisions {
            strategic,
            team_decision,
            raid_decision,
            allocated_producer_intents,
            team_was_active,
            lift_was_active,
            raid_was_active,
        },
    );

    if strategy
        .air_operation()
        .is_some_and(|operation| lifts.shares_air_objective(operation))
        && let Some(credit) = strategy.outcomes.episode_id()
    {
        lifts.outcomes.share_credit(credit);
    }

    if let Some(recorder) = recorder {
        let trace = recorder.trace_mut();
        if let Some(core_ready) = team_relief_core_ready {
            trace.gates.team_relief_core_ready = Some(core_ready);
        }
        trace.gates.raid_attention = Some(RaidAttentionTrace {
            strategic_load: bounded_count(raid_attention.strategic_load),
            attention_slots: bounded_count(raid_attention.attention_slots),
            admitted: raid_attention.admitted,
        });
        trace.gates.lift_rejected = lift_rejected;
        trace.channels.team_relief = channel_trace(
            team_before_state.expect("a traced decision captured the prior team state"),
            team_channel_state(team),
            &team_decision,
        );
        trace.channels.connected_air = channel_trace(
            air_before_state.expect("a traced decision captured the prior air state"),
            air_channel_state(strategy),
            &air_decision_for_trace,
        );
        trace.channels.lift = channel_trace(
            lift_before_state.expect("a traced decision captured the prior lift state"),
            lift_channel_state(lifts),
            &lift_decision,
        );
        trace.channels.raid = channel_trace(
            raid_before_state.expect("a traced decision captured the prior raid state"),
            raid_channel_state(raids),
            &raid_decision,
        );
        let mut connected_force = connected_force_trace(
            strategy,
            intelligence,
            rejected_connected_candidate.as_ref(),
        );
        connected_force.preserve_terminal_package(
            connected_force_before.expect("a traced decision captured the prior connected force"),
        );
        trace.connected_force = connected_force;
        trace.budget = Some(ScrapBudgetTrace {
            bank: oriented.scrap,
            foundry_saving,
            deferred_construction: UtilityPolicy::deferred_construction_commitment(oriented),
            airworks_capacity,
            opening_bootstrap,
            voluntary_scrap_guard,
            frozen: !allow_new_voluntary_operations || !allocation_ok,
            prior_operation_spendable,
            strategic_spendable: if accepted_connected
                && connected_accepted_at == Some(oriented.tick)
            {
                connected_spendable
            } else {
                0
            },
            strategic_committed: strategic.committed_scrap,
            prospective_carrier: prospective_carrier_hold,
            utility_spendable,
        });
    }
    let mut strategic_core_exclusions = PlannerClaims::new(enlisted, strategy, raids, lifts)
        .core_exclusions(&team.core_reservations());
    strategic_core_exclusions.extend(policy.state.reconnaissance.reservations());
    strategic_core_exclusions.extend(policy.support_reservations());
    strategic_core_exclusions.sort_unstable();
    strategic_core_exclusions.dedup();
    let reservations =
        retained_reservations(strategic.reservations, &strategic_core_exclusions, oriented);
    let mut utility_reservations = reservations.clone();
    utility_reservations.extend(policy.state.reconnaissance.reservations());
    utility_reservations.extend(policy.support_reservations());

    utility_reservations.extend(team.reservations());

    utility_reservations.extend(fresh_defense_builders);
    utility_reservations.sort_unstable();
    utility_reservations.dedup();
    AdmittedWork {
        intents: strategic.intents,
        reservations,
        utility: UtilityGrant {
            reservations: utility_reservations,
            core_exclusions: strategic_core_exclusions,
            prior_commitment: utility_prior_commitment,
            foundry: foundry_handoff,
            voluntary_guard: voluntary_scrap_guard.saturating_sub(prospective_carrier_hold),
            producer_lanes: producer_lane_reservations,
        },
    }
}

fn channel_trace(
    before: ChannelState,
    after: ChannelState,
    decision: &StrategicDecision,
) -> ChannelTrace {
    ChannelTrace {
        before,
        after,
        effects: channel_effects(
            decision.intents.len(),
            &decision.reservations,
            decision.committed_scrap(),
        ),
    }
}

fn team_channel_state(planner: &TeamReliefPlanner) -> ChannelState {
    if let Some(operation) = planner.operation() {
        let phase = match operation.phase {
            crate::team::TeamReliefPhase::Preparing => return ChannelState::Preparing,
            crate::team::TeamReliefPhase::Deploying => ChannelPhase::TeamDeploying,
            crate::team::TeamReliefPhase::Holding => ChannelPhase::TeamHolding,
            crate::team::TeamReliefPhase::Withdrawing => ChannelPhase::TeamWithdrawing,
        };
        ChannelState::Active(phase)
    } else {
        ChannelState::Idle
    }
}

fn air_channel_state(planner: &StrategicPlanner) -> ChannelState {
    let Some(operation) = planner.air_operation() else {
        return ChannelState::Idle;
    };
    let phase = match operation.phase() {
        AirOperationPhase::Recon => ChannelPhase::AirRecon,
        AirOperationPhase::Assemble => ChannelPhase::AirAssemble,
        AirOperationPhase::SuppressAa => ChannelPhase::AirSuppressAa,
        AirOperationPhase::Verify => ChannelPhase::AirVerify,
        AirOperationPhase::Strike => ChannelPhase::AirStrike,
        AirOperationPhase::Recover => ChannelPhase::AirRecover,
    };
    ChannelState::Active(phase)
}

fn lift_channel_state(planner: &LiftPlanner) -> ChannelState {
    let Some(operation) = planner.operation() else {
        return ChannelState::Idle;
    };
    let phase = match operation.phase {
        crate::lift::LiftPhase::Provision => ChannelPhase::LiftProvision,
        crate::lift::LiftPhase::Boarding => ChannelPhase::LiftBoarding,
        crate::lift::LiftPhase::AwaitSupport => ChannelPhase::LiftAwaitSupport,
        crate::lift::LiftPhase::Landing => ChannelPhase::LiftLanding,
        crate::lift::LiftPhase::Recover => ChannelPhase::LiftRecover,
    };
    ChannelState::Active(phase)
}

fn raid_channel_state(planner: &RaidPlanner) -> ChannelState {
    if let Some(operation) = planner.operation() {
        let phase = match operation.phase {
            crate::raid::RaidPhase::Ingress => ChannelPhase::RaidIngress,
            crate::raid::RaidPhase::Strike => ChannelPhase::RaidStrike,
            crate::raid::RaidPhase::Egress => ChannelPhase::RaidEgress,
        };
        ChannelState::Active(phase)
    } else {
        ChannelState::Idle
    }
}

pub(crate) fn retained_reservations(
    mut decision_reservations: Vec<UnitId>,
    preserved_planner_ownership: &[UnitId],
    observation: &Observation,
) -> Vec<UnitId> {
    decision_reservations.extend(preserved_planner_ownership.iter().copied().filter(|id| {
        observation
            .my_units
            .binary_search_by_key(id, |unit| unit.id)
            .is_ok()
    }));
    decision_reservations.sort_unstable();
    decision_reservations.dedup();
    decision_reservations
}

#[cfg(test)]
mod tests;
