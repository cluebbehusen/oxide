//! Coordination for strategic work that has not yet migrated into allocation.
//!
//! The shared allocator settles typed work first. This module then gives the
//! remaining Team, Lift, and Raid planners one ordered pass over the exact
//! residual resources before Utility receives what is left.

use super::allocation::{PlannerClaims, lift_air_support};
use super::difficulty::DifficultyTuning;
use super::executive::{Army, ArmyState, Intent};
use super::intelligence::StrategicIntelligence;
use super::lift::{LiftAdmission, LiftAirSupport, LiftPlanner};
use super::observation::Observation;
use super::profile::ResolvedProfile;
use super::raid::{RaidPlanner, RaidPlanningContext};
use super::resources::ProducerLaneReservations;
use super::strategy::{AirOperationPhase, StrategicDecision, StrategicPlanner};
use super::team::{TeamReliefAdmission, TeamReliefPlanner};
use super::utility::combat_core_status;
use crate::ids::UnitId;
use chassis::grid::TilePos;

pub(super) struct ResidualCoordinationContext<'a> {
    pub(super) profile: &'a ResolvedProfile,
    pub(super) tuning: DifficultyTuning,
    pub(super) observation: &'a Observation,
    pub(super) intelligence: &'a StrategicIntelligence,
    pub(super) home: TilePos,
    pub(super) armies: &'a [Army],
    pub(super) enlisted: &'a [UnitId],
    pub(super) minimum_core_equivalents: u64,
    pub(super) allocation_ok: bool,
    pub(super) allow_new_voluntary_operations: bool,
    pub(super) connected_is_typed: bool,
    pub(super) raw_residual_scrap: u32,
    pub(super) residual_scrap: u32,
    pub(super) allocation_utility_spendable: u32,
    pub(super) producer_lanes: &'a ProducerLaneReservations,
}

pub(super) struct ResidualCoordinationParticipants<'a> {
    pub(super) strategy: &'a mut Option<StrategicPlanner>,
    pub(super) lifts: &'a mut Option<LiftPlanner>,
    pub(super) team: &'a mut Option<TeamReliefPlanner>,
    pub(super) raids: &'a mut Option<RaidPlanner>,
}

pub(super) struct ResidualPlannerWork {
    pub(super) strategic: StrategicDecision,
    pub(super) team_decision: StrategicDecision,
    pub(super) lift_decision: StrategicDecision,
    pub(super) raid_decision: StrategicDecision,
    pub(super) allocated_producer_intents: Vec<Intent>,
    pub(super) team_was_active: bool,
    pub(super) lift_was_active: bool,
    pub(super) raid_was_active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RaidAttentionDecision {
    pub(super) strategic_load: usize,
    pub(super) attention_slots: usize,
    pub(super) admitted: bool,
}

pub(super) struct ResidualCoordinationOutcome {
    pub(super) strategic: StrategicDecision,
    pub(super) team_decision: StrategicDecision,
    pub(super) lift_decision: StrategicDecision,
    pub(super) raid_decision: StrategicDecision,
    pub(super) team_relief_core_ready: Option<bool>,
    pub(super) team_relief_rolled_back: bool,
    pub(super) lift_rolled_back: bool,
    pub(super) raid_attention: RaidAttentionDecision,
    pub(super) air_active: bool,
    pub(super) lift_active: bool,
    pub(super) prospective_carrier_hold: u32,
    pub(super) utility_prior_commitment: u32,
    pub(super) utility_spendable: u32,
    pub(super) outstanding_air_production_ticks: u64,
}

pub(super) fn coordinate_residual_work(
    context: ResidualCoordinationContext<'_>,
    participants: ResidualCoordinationParticipants<'_>,
    work: ResidualPlannerWork,
) -> ResidualCoordinationOutcome {
    let ResidualCoordinationParticipants {
        strategy,
        lifts,
        team,
        raids,
    } = participants;
    let ResidualPlannerWork {
        mut strategic,
        mut team_decision,
        mut lift_decision,
        mut raid_decision,
        allocated_producer_intents,
        team_was_active,
        lift_was_active,
        raid_was_active,
    } = work;

    let mut post_allocation_commitment = if context.connected_is_typed {
        0
    } else {
        strategic.committed_scrap
    };
    for decision in [&team_decision, &lift_decision, &raid_decision] {
        let mut allocated = decision.clone();
        remove_producer_intents(&mut allocated);
        merge_strategic(&mut strategic, allocated);
    }
    strategic.intents.splice(0..0, allocated_producer_intents);

    let mut team_relief_core_ready = None;
    let mut team_relief_rolled_back = false;
    if !team_was_active {
        let claims = PlannerClaims::new(context.enlisted, strategy, raids, lifts);
        let prior_team_claims = team
            .as_ref()
            .map_or_else(Vec::new, TeamReliefPlanner::core_reservations);
        let external = claims.external_to_team();
        let core_before = claims.core_exclusions(&prior_team_claims);
        let team_core_ready = combat_core_status(
            context.observation,
            &core_before,
            &[],
            context.minimum_core_equivalents,
        )
        .ready;
        let team_gate = team_relief_gate(context.allocation_ok, team_core_ready);
        team_relief_core_ready = Some(team_gate.core_ready);
        let snapshot = team.clone();
        if let Some(planner) = team.as_mut() {
            team_decision = planner.think_with_admission(
                context.profile,
                context.tuning,
                context.observation,
                context.home,
                &external,
                TeamReliefAdmission {
                    additionally_reserved: &[],
                    allow_new_operation: team_gate.allow_new_operation,
                    core_reservations: &core_before,
                    minimum_core_equivalents: context.minimum_core_equivalents,
                },
            );
        }
        let mut resulting_core = team
            .as_ref()
            .map_or_else(Vec::new, TeamReliefPlanner::core_reservations);
        if prior_team_claims.is_empty() && !resulting_core.is_empty() {
            let after = PlannerClaims::new(context.enlisted, strategy, raids, lifts);
            let exclusions = after.core_exclusions(&resulting_core);
            team_relief_rolled_back = roll_back_unless_core_ready(
                context.observation,
                &exclusions,
                context.minimum_core_equivalents,
                team,
                snapshot,
                &mut team_decision,
                Some(&mut resulting_core),
            );
        }
        post_allocation_commitment =
            post_allocation_commitment.saturating_add(team_decision.committed_scrap);
        merge_strategic(&mut strategic, team_decision.clone());
    }

    let claims_before_lift = PlannerClaims::new(context.enlisted, strategy, raids, lifts);
    let team_core_claims = team
        .as_ref()
        .map_or_else(Vec::new, TeamReliefPlanner::core_reservations);
    let core_exclusions_after_team = claims_before_lift.core_exclusions(&team_core_claims);
    let prior_non_lift = claims_before_lift.without_lift(&team_core_claims);
    let lift_unavailable_after = lift_unavailable(
        context.observation,
        context.armies,
        context.enlisted,
        &prior_non_lift,
    );
    let mut lift_rolled_back = false;
    if !lift_was_active {
        let snapshot = lifts.clone();
        let mut support = strategy
            .as_ref()
            .map_or(LiftAirSupport::Independent, |planner| {
                lift_air_support(planner.air_operation(), planner.terminal_outcome())
            });
        let air_accepts_new_lift = strategy
            .as_ref()
            .and_then(StrategicPlanner::air_operation)
            .is_some_and(|operation| operation.phase != AirOperationPhase::Recover);
        support = match (air_accepts_new_lift, support) {
            (true, LiftAirSupport::Released { player, target }) => {
                LiftAirSupport::Suppressing { player, target }
            }
            (true, support @ LiftAirSupport::Suppressing { .. }) => support,
            _ => LiftAirSupport::Independent,
        };
        let lift_spendable = context
            .residual_scrap
            .saturating_sub(post_allocation_commitment);
        if let Some(planner) = lifts.as_mut() {
            lift_decision = planner.think_with_admission_and_producer_lanes(
                context.observation,
                context.home,
                &lift_unavailable_after,
                support,
                LiftAdmission {
                    allow_new_commitments: context.allocation_ok
                        && context.allow_new_voluntary_operations,
                    spendable_scrap: lift_spendable,
                    core_reservations: &core_exclusions_after_team,
                    minimum_core_equivalents: context.minimum_core_equivalents,
                },
                context.producer_lanes,
            );
        }
        if lifts
            .as_ref()
            .is_some_and(|planner| planner.operation().is_some())
        {
            let after = PlannerClaims::new(context.enlisted, strategy, raids, lifts);
            let exclusions = after.core_exclusions(&team_core_claims);
            lift_rolled_back = roll_back_unless_core_ready(
                context.observation,
                &exclusions,
                context.minimum_core_equivalents,
                lifts,
                snapshot,
                &mut lift_decision,
                None,
            );
        }
        post_allocation_commitment =
            post_allocation_commitment.saturating_add(lift_decision.committed_scrap);
        merge_strategic(&mut strategic, lift_decision.clone());
    }

    let air_active = strategy
        .as_ref()
        .is_some_and(|planner| planner.air_operation().is_some());
    let team_active = team
        .as_ref()
        .is_some_and(|planner| planner.operation().is_some());
    let lift_active = lifts
        .as_ref()
        .is_some_and(|planner| planner.operation().is_some());
    let strategic_load =
        usize::from(air_active) + usize::from(team_active) + usize::from(lift_active);
    let raid_claimed = raids
        .as_ref()
        .is_some_and(|planner| !planner.reservations().is_empty());
    let can_begin_raid = context.allocation_ok
        && context.allow_new_voluntary_operations
        && can_admit_optional_raid(context.tuning, strategic_load);
    if !raid_was_active && (raid_claimed || can_begin_raid) {
        let raid_exclusions =
            PlannerClaims::new(context.enlisted, strategy, raids, lifts).all(&team_core_claims);
        if let Some(planner) = raids.as_mut() {
            raid_decision = planner.think_with_admission(
                RaidPlanningContext::new(
                    context.profile,
                    context.tuning,
                    context.observation,
                    context.home,
                    context.enlisted,
                    &raid_exclusions,
                )
                .with_admission(can_begin_raid),
            );
        }
        post_allocation_commitment =
            post_allocation_commitment.saturating_add(raid_decision.committed_scrap);
        merge_strategic(&mut strategic, raid_decision.clone());
    }

    let claims_after_raid = PlannerClaims::new(context.enlisted, strategy, raids, lifts);
    let core_exclusions_after_raid = claims_after_raid.core_exclusions(&team_core_claims);
    let prior_non_lift_after_raid = claims_after_raid.without_lift(&team_core_claims);
    let lift_unavailable_after_raid = lift_unavailable(
        context.observation,
        context.armies,
        context.enlisted,
        &prior_non_lift_after_raid,
    );
    let prospective_carrier_commitment = if context.allocation_ok
        && context.allow_new_voluntary_operations
    {
        strategy
            .as_ref()
            .and_then(StrategicPlanner::air_operation)
            .filter(|operation| {
                operation.phase == AirOperationPhase::Recon && !operation.assault_admitted
            })
            .and_then(|operation| {
                context.intelligence.buildings().iter().find(|contact| {
                    contact.player == operation.target_player && contact.anchor == operation.target
                })
            })
            .and_then(|target| {
                lifts.as_ref().map(|planner| {
                    planner.prospective_first_carrier_commitment(
                        context.observation,
                        context.home,
                        &lift_unavailable_after_raid,
                        &core_exclusions_after_raid,
                        context.minimum_core_equivalents,
                        target,
                    )
                })
            })
            .unwrap_or(0)
    } else {
        0
    };
    let prospective_carrier_hold = applied_prospective_carrier_hold(
        prospective_carrier_commitment,
        context
            .raw_residual_scrap
            .saturating_sub(post_allocation_commitment),
    );
    post_allocation_commitment =
        post_allocation_commitment.saturating_add(prospective_carrier_hold);
    strategic.committed_scrap = strategic
        .committed_scrap
        .saturating_add(prospective_carrier_hold);

    let outstanding_air_production_ticks = strategy
        .as_ref()
        .map_or(0, |planner| {
            planner.remaining_airwork_ticks(context.observation)
        })
        .saturating_add(lifts.as_ref().map_or(0, |planner| {
            planner.remaining_airwork_ticks(context.observation, &lift_unavailable_after_raid)
        }));
    let utility_prior_commitment = context
        .observation
        .scrap
        .saturating_sub(context.allocation_utility_spendable)
        .saturating_add(post_allocation_commitment);
    let utility_spendable = context
        .observation
        .scrap
        .saturating_sub(utility_prior_commitment);
    ResidualCoordinationOutcome {
        strategic,
        team_decision,
        lift_decision,
        raid_decision,
        team_relief_core_ready,
        team_relief_rolled_back,
        lift_rolled_back,
        raid_attention: RaidAttentionDecision {
            strategic_load,
            attention_slots: context.tuning.attention_slots,
            admitted: can_begin_raid,
        },
        air_active,
        lift_active,
        prospective_carrier_hold,
        utility_prior_commitment,
        utility_spendable,
        outstanding_air_production_ticks,
    }
}

fn merge_strategic(into: &mut StrategicDecision, mut additional: StrategicDecision) {
    into.intents.append(&mut additional.intents);
    into.reservations.append(&mut additional.reservations);
    into.reservations.sort_unstable();
    into.reservations.dedup();
    into.committed_scrap = into
        .committed_scrap
        .saturating_add(additional.committed_scrap);
}

pub(super) fn remove_producer_intents(decision: &mut StrategicDecision) {
    decision
        .intents
        .retain(|intent| !matches!(intent, Intent::TrainAt { .. }));
}

fn applied_prospective_carrier_hold(requested: u32, uncommitted_scrap: u32) -> u32 {
    requested.min(uncommitted_scrap)
}

fn can_admit_optional_raid(tuning: DifficultyTuning, strategic_load: usize) -> bool {
    strategic_load == 0 || tuning.attention_slots >= (strategic_load + 1).saturating_mul(2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TeamReliefGate {
    core_ready: bool,
    allow_new_operation: bool,
}

const fn team_relief_gate(allocation_ok: bool, core_ready: bool) -> TeamReliefGate {
    TeamReliefGate {
        core_ready,
        allow_new_operation: allocation_ok && core_ready,
    }
}

fn roll_back_unless_core_ready<P>(
    observation: &Observation,
    candidate_core_exclusions: &[UnitId],
    minimum_core_equivalents: u64,
    planner: &mut Option<P>,
    snapshot: Option<P>,
    decision: &mut StrategicDecision,
    derived_claims: Option<&mut Vec<UnitId>>,
) -> bool {
    if combat_core_status(
        observation,
        candidate_core_exclusions,
        &[],
        minimum_core_equivalents,
    )
    .ready
    {
        return false;
    }
    *planner = snapshot;
    *decision = StrategicDecision::default();
    if let Some(claims) = derived_claims {
        claims.clear();
    }
    true
}

pub(super) fn lift_unavailable(
    observation: &Observation,
    armies: &[Army],
    enlisted: &[UnitId],
    strategic: &[UnitId],
) -> Vec<UnitId> {
    let mut transferable: Vec<_> = armies
        .iter()
        .filter(|army| {
            army.state == ArmyState::Staging
                && army.target.is_none_or(|target| {
                    let holding_target = army.staging.chebyshev(target) <= 8;
                    let target_is_contested = observation
                        .enemy_units
                        .iter()
                        .any(|unit| unit.tile.chebyshev(target) <= 8)
                        || observation.enemy_buildings.iter().any(|building| {
                            building.seen && building.anchor.chebyshev(target) <= 8
                        });
                    !holding_target || !target_is_contested
                })
        })
        .flat_map(|army| army.members.iter().copied())
        .collect();
    transferable.sort_unstable();
    transferable.dedup();

    let mut unavailable: Vec<_> = enlisted
        .iter()
        .copied()
        .filter(|id| transferable.binary_search(id).is_err())
        .collect();
    unavailable.extend_from_slice(strategic);
    unavailable.sort_unstable();
    unavailable.dedup();
    unavailable
}

#[cfg(test)]
mod tests {
    use super::super::allocation::prior_planner_claims;
    use super::super::observation::{BuildingObs, UnitObs};
    use super::*;
    use crate::ids::{BuildingId, PlayerId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
    use crate::state::Faction;
    use crate::stats::{BuildingKind, UnitKind};

    const HOME: TilePos = TilePos::new(5, 15);
    const TARGET: TilePos = TilePos::new(50, 15);

    #[test]
    fn already_imported_active_commitments_are_not_subtracted_from_residual_twice() {
        let mut observation = open_observation(16, 16);
        observation.scrap = 100;
        let profile = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_024,
        ));
        let intelligence = StrategicIntelligence::default();
        let producer_lanes = ProducerLaneReservations::default();
        let mut strategy = None;
        let mut lifts = None;
        let mut team = None;
        let mut raids = None;
        let outcome = coordinate_residual_work(
            ResidualCoordinationContext {
                profile: &profile,
                tuning: DifficultyTuning::for_level(BotDifficulty::Prime),
                observation: &observation,
                intelligence: &intelligence,
                home: HOME,
                armies: &[],
                enlisted: &[],
                minimum_core_equivalents: 0,
                allocation_ok: false,
                allow_new_voluntary_operations: false,
                connected_is_typed: false,
                raw_residual_scrap: 70,
                residual_scrap: 70,
                allocation_utility_spendable: 70,
                producer_lanes: &producer_lanes,
            },
            ResidualCoordinationParticipants {
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            ResidualPlannerWork {
                strategic: StrategicDecision {
                    committed_scrap: 7,
                    ..StrategicDecision::default()
                },
                team_decision: StrategicDecision {
                    committed_scrap: 10,
                    ..StrategicDecision::default()
                },
                lift_decision: StrategicDecision {
                    committed_scrap: 11,
                    ..StrategicDecision::default()
                },
                raid_decision: StrategicDecision {
                    committed_scrap: 9,
                    ..StrategicDecision::default()
                },
                team_was_active: true,
                lift_was_active: true,
                raid_was_active: true,
                allocated_producer_intents: Vec::new(),
            },
        );

        assert_eq!(outcome.strategic.committed_scrap, 37);
        assert_eq!(outcome.utility_prior_commitment, 37);
        assert_eq!(outcome.utility_spendable, 63);
    }

    #[test]
    fn prospective_carrier_hold_never_exceeds_uncommitted_scrap() {
        assert_eq!(applied_prospective_carrier_hold(250, 40), 40);
        assert_eq!(applied_prospective_carrier_hold(40, 250), 40);
        assert_eq!(applied_prospective_carrier_hold(250, 0), 0);
    }

    #[test]
    fn attention_keeps_optional_raids_bounded_without_a_prime_only_fragmentation_case() {
        let tuning = BotDifficulty::ALL.map(DifficultyTuning::for_level);
        assert_eq!(
            tuning.map(|difficulty| can_admit_optional_raid(difficulty, 0)),
            [true; 4],
            "an idle planner may consider a raid at every rung"
        );
        assert_eq!(
            tuning.map(|difficulty| can_admit_optional_raid(difficulty, 1)),
            [false, false, true, true],
            "only the attentive rungs may layer a raid beside one major operation"
        );
        assert_eq!(
            tuning.map(|difficulty| can_admit_optional_raid(difficulty, 2)),
            [false; 4],
            "no rung should peel off raiders while air and lift already run together"
        );
    }

    #[test]
    fn team_relief_trace_evidence_is_independent_of_allocator_health() {
        assert_eq!(
            team_relief_gate(false, true),
            TeamReliefGate {
                core_ready: true,
                allow_new_operation: false,
            },
            "a failed allocation closes admission without rewriting core evidence"
        );
        assert_eq!(
            team_relief_gate(true, false),
            TeamReliefGate {
                core_ready: false,
                allow_new_operation: false,
            }
        );
        assert_eq!(
            team_relief_gate(true, true),
            TeamReliefGate {
                core_ready: true,
                allow_new_operation: true,
            }
        );
    }

    #[test]
    fn team_candidate_rolls_back_when_derived_claims_break_the_core() {
        let obs = team_relief_observation();
        let home = HOME;
        let mut profile = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_024,
        ));
        profile.traits.support = 70;
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut planner = Some(TeamReliefPlanner::new());
        let snapshot = planner.clone();
        let mut decision = planner
            .as_mut()
            .expect("the candidate owns a team planner")
            .think_with_admission(
                &profile,
                tuning,
                &obs,
                home,
                &[],
                TeamReliefAdmission {
                    additionally_reserved: &[],
                    allow_new_operation: true,
                    core_reservations: &[],
                    minimum_core_equivalents: 0,
                },
            );
        let candidate_reservations = planner
            .as_ref()
            .expect("the candidate owns a team planner")
            .reservations();
        let mut derived_claims = planner
            .as_ref()
            .expect("the candidate owns a team planner")
            .core_reservations();

        assert!(
            snapshot
                .as_ref()
                .expect("the snapshot owns a team planner")
                .reservations()
                .is_empty()
        );
        assert!(!candidate_reservations.is_empty());
        assert!(!decision.reservations.is_empty());
        assert!(!derived_claims.is_empty());
        assert!(combat_core_status(&obs, &[], &[], 8).ready);
        assert!(!combat_core_status(&obs, &derived_claims, &[], 8).ready);

        let candidate_exclusions = derived_claims.clone();
        roll_back_unless_core_ready(
            &obs,
            &candidate_exclusions,
            8,
            &mut planner,
            snapshot.clone(),
            &mut decision,
            Some(&mut derived_claims),
        );

        assert_eq!(planner, snapshot);
        assert_eq!(decision, StrategicDecision::default());
        assert!(derived_claims.is_empty());
    }

    #[test]
    fn lift_candidate_rolls_back_when_its_payload_breaks_the_core() {
        let obs = lift_observation();
        let home = HOME;
        let mut planner = Some(LiftPlanner::new());
        let snapshot = planner.clone();
        let mut decision = planner
            .as_mut()
            .expect("the candidate owns a lift planner")
            .think_with_admission(
                &obs,
                home,
                &[],
                LiftAirSupport::Independent,
                LiftAdmission {
                    allow_new_commitments: true,
                    spendable_scrap: obs.scrap,
                    core_reservations: &[],
                    minimum_core_equivalents: 0,
                },
            );
        let candidate_exclusions = prior_planner_claims(
            &[],
            None,
            &[],
            &[],
            planner
                .as_ref()
                .expect("the candidate owns a lift planner")
                .operation(),
        );
        let operation = planner
            .as_ref()
            .expect("the candidate owns a lift planner")
            .operation()
            .expect("the ungated candidate starts a lift");

        assert!(
            snapshot
                .as_ref()
                .expect("the snapshot owns a lift planner")
                .operation()
                .is_none()
        );
        assert!(!operation.payload.is_empty());
        assert_ne!(decision, StrategicDecision::default());
        assert!(combat_core_status(&obs, &[], &[], 8).ready);
        assert!(!combat_core_status(&obs, &candidate_exclusions, &[], 8).ready);

        roll_back_unless_core_ready(
            &obs,
            &candidate_exclusions,
            8,
            &mut planner,
            snapshot.clone(),
            &mut decision,
            None,
        );

        assert_eq!(planner, snapshot);
        assert!(
            planner
                .as_ref()
                .expect("the restored planner exists")
                .operation()
                .is_none()
        );
        assert_eq!(decision, StrategicDecision::default());
    }

    fn team_relief_observation() -> Observation {
        let mut observation = open_observation(40, 24);
        observation.my_buildings = vec![building(1, 0, BuildingKind::Foundry, HOME)];
        observation.my_queues = vec![Vec::new()];
        observation.my_queue_progress = vec![0];
        observation.ally_buildings =
            vec![building(2, 1, BuildingKind::Foundry, TilePos::new(24, 10))];
        observation.enemy_units = vec![unit(100, 2, UnitKind::Sentinel, TilePos::new(28, 10))];
        observation.my_units = (0..8)
            .map(|index| {
                unit(
                    index + 1,
                    0,
                    UnitKind::Sentinel,
                    TilePos::new(3 + (index % 4) as i32, 15 + (index / 4) as i32),
                )
            })
            .collect();
        observation
    }

    fn lift_observation() -> Observation {
        let mut observation = open_observation(64, 32);
        observation.scrap = 50_000;
        observation.known_rock = (0..32).map(|y| TilePos::new(32, y)).collect();
        observation.my_buildings = vec![
            building(1, 0, BuildingKind::Foundry, HOME),
            building(2, 0, BuildingKind::Airworks, HOME.offset(4, -4)),
        ];
        observation.my_queues = vec![Vec::new(), Vec::new()];
        observation.my_queue_progress = vec![0, 0];
        observation.enemy_buildings = vec![building(500, 1, BuildingKind::Foundry, TARGET)];
        observation.my_units = (0..8)
            .map(|index| {
                unit(
                    index + 1,
                    0,
                    UnitKind::Sentinel,
                    HOME.offset((index % 4) as i32, (index / 4) as i32 + 2),
                )
            })
            .collect();
        observation
    }

    fn open_observation(width: i32, height: i32) -> Observation {
        let tile_count = usize::try_from(width.saturating_mul(height)).expect("the map fits usize");
        Observation {
            me: PlayerId(0),
            map_width: width,
            map_height: height,
            visible: vec![true; tile_count],
            explored: vec![true; tile_count],
            faction: Faction::Ferrous,
            ..Observation::default()
        }
    }

    fn unit(id: u32, player: u8, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(player),
            kind,
            tile,
            hp: kind.stats().max_hp,
            idle: player == 0,
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

    fn building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
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
}
