//! Lower-priority operation admission within the allocation pipeline.
//! Grants are consumed in stable order after portfolio settlement.

use crate::bot::allocation::{
    ClaimBundle, LegacyChannel, LegacyDecisionRequest, PlannerClaims, legacy_decision_obligation,
    lift_air_support, prior_planner_claims,
};
use crate::bot::difficulty::DifficultyTuning;
use crate::bot::executive::{Army, ArmyState, Intent};
use crate::bot::intelligence::StrategicIntelligence;
use crate::bot::lift::{LiftAdmission, LiftAirSupport, LiftPlanner};
use crate::bot::observation::Observation;
#[cfg(test)]
use crate::bot::observation::ObservationData;
use crate::bot::profile::ResolvedProfile;
use crate::bot::raid::{RaidPlanner, RaidPlanningContext};
use crate::bot::resources::ProducerLaneReservations;
use crate::bot::resources::ResourceSnapshot;
use crate::bot::strategy::{AirOperationPhase, StrategicDecision, StrategicPlanner};
use crate::bot::team::TeamReliefPlanner;
use crate::bot::utility::combat_core_status;
use crate::ids::UnitId;
use chassis::grid::TilePos;

pub(super) struct OperationContext<'a> {
    pub(super) cadence: chassis::Tick,
    pub(super) profile: &'a ResolvedProfile,
    pub(super) tuning: DifficultyTuning,
    pub(super) observation: &'a Observation,
    pub(super) intelligence: &'a StrategicIntelligence,
    pub(super) home: TilePos,
    pub(super) armies: &'a [Army],
    pub(super) enlisted: &'a [UnitId],
    pub(super) utility_reservations: &'a [UnitId],
    pub(super) minimum_core_equivalents: u64,
    pub(super) allocation_ok: bool,
    pub(super) allow_new_voluntary_operations: bool,
    pub(super) connected_is_typed: bool,
    pub(super) raw_residual_scrap: u32,
    pub(super) residual_scrap: u32,
    pub(super) allocation_utility_spendable: u32,
    pub(super) producer_lanes: &'a ProducerLaneReservations,
}

pub(super) struct OperationParticipants<'a> {
    pub(super) strategy: &'a mut StrategicPlanner,
    pub(super) lifts: &'a mut LiftPlanner,
    pub(super) team: &'a mut TeamReliefPlanner,
    pub(super) raids: &'a mut RaidPlanner,
}

pub(super) struct RetainedDecisions {
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

pub(super) struct OperationSettlement {
    pub(super) strategic: StrategicDecision,
    pub(super) team_decision: StrategicDecision,
    pub(super) lift_decision: StrategicDecision,
    pub(super) raid_decision: StrategicDecision,
    pub(super) team_relief_core_ready: Option<bool>,
    pub(super) lift_rejected: bool,
    pub(super) raid_attention: RaidAttentionDecision,
    pub(super) air_active: bool,
    pub(super) lift_active: bool,
    pub(super) prospective_carrier_hold: u32,
    pub(super) utility_prior_commitment: u32,
    pub(super) utility_spendable: u32,
    pub(super) outstanding_air_production_ticks: u64,
}

pub(super) fn settle_operations(
    context: OperationContext<'_>,
    participants: OperationParticipants<'_>,
    work: RetainedDecisions,
) -> OperationSettlement {
    let OperationParticipants {
        strategy,
        lifts,
        team,
        raids,
    } = participants;
    let RetainedDecisions {
        mut strategic,
        team_decision,
        mut lift_decision,
        mut raid_decision,
        allocated_producer_intents,
        team_was_active,
        lift_was_active,
        raid_was_active,
    } = work;

    let mut funds = OperationFunds {
        unguarded: context.raw_residual_scrap,
        spendable: context.residual_scrap,
        prior_utility: context
            .observation
            .scrap
            .saturating_sub(context.allocation_utility_spendable),
        committed: if context.connected_is_typed {
            0
        } else {
            strategic.committed_scrap
        },
    };
    for decision in [&team_decision, &lift_decision, &raid_decision] {
        let mut allocated = decision.clone();
        remove_producer_intents(&mut allocated);
        merge_strategic(&mut strategic, allocated);
    }
    strategic.intents.splice(0..0, allocated_producer_intents);

    let team_relief_core_ready = (!team_was_active).then(|| {
        let claims = PlannerClaims::new(context.enlisted, strategy, raids, lifts);
        let team_claims = team.core_reservations();
        combat_core_status(
            context.observation,
            &claims.core_exclusions(&team_claims),
            &[],
            context.minimum_core_equivalents,
        )
        .ready
    });

    let claims_before_lift = PlannerClaims::new(context.enlisted, strategy, raids, lifts);
    let team_core_claims = team.core_reservations();
    let core_exclusions_after_team = claims_before_lift.core_exclusions(&team_core_claims);
    let mut prior_non_lift = claims_before_lift.without_lift(&team_core_claims);
    prior_non_lift.extend_from_slice(context.utility_reservations);
    let lift_unavailable_after = lift_unavailable(
        context.observation,
        context.armies,
        context.enlisted,
        &prior_non_lift,
    );
    let mut lift_rejected = false;
    if !lift_was_active {
        let support = lift_air_support(strategy.air_operation(), strategy.terminal_outcome());
        let support = match (strategy.air_operation(), support) {
            (Some(operation), LiftAirSupport::Released { player, target })
                if operation.phase != AirOperationPhase::Recover =>
            {
                LiftAirSupport::Suppressing { player, target }
            }
            (Some(operation), support @ LiftAirSupport::Suppressing { .. })
                if operation.phase != AirOperationPhase::Recover =>
            {
                support
            }
            _ => LiftAirSupport::Independent,
        };
        let grant = LiftGrant {
            current_scrap: funds.available(),
            unavailable: &lift_unavailable_after,
            core_exclusions: &core_exclusions_after_team,
            minimum_core_equivalents: context.minimum_core_equivalents,
            allow_new: context.allocation_ok && context.allow_new_voluntary_operations,
            producer_lanes: context.producer_lanes,
            prior_producer_intents: &strategic.intents,
        };
        match grant.prepare(
            context.observation,
            context.home,
            context.cadence,
            support,
            lifts,
        ) {
            Some(accepted) => lift_decision = accepted.commit(lifts),
            None => {
                lift_rejected = true;
                lift_decision = StrategicDecision::default();
            }
        }
        funds.commit(lift_decision.committed_scrap);
        merge_strategic(&mut strategic, lift_decision.clone());
    }

    let air_active = strategy.air_operation().is_some();
    let team_active = team.operation().is_some();
    let lift_active = lifts.operation().is_some();
    let strategic_load =
        usize::from(air_active) + usize::from(team_active) + usize::from(lift_active);
    let raid_claimed = !raids.reservations().is_empty();
    let can_begin_raid = context.allocation_ok
        && context.allow_new_voluntary_operations
        && can_admit_optional_raid(context.tuning, strategic_load);
    if !raid_was_active && (raid_claimed || can_begin_raid) {
        let mut raid_exclusions = PlannerClaims::new(context.enlisted, strategy, raids, lifts)
            .without_raid(&team_core_claims);
        raid_exclusions.extend_from_slice(context.utility_reservations);
        raid_exclusions.sort_unstable();
        raid_exclusions.dedup();

        raid_decision = raids.think_with_admission(
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

        funds.commit(raid_decision.committed_scrap);
        merge_strategic(&mut strategic, raid_decision.clone());
    }

    let claims_after_raid = PlannerClaims::new(context.enlisted, strategy, raids, lifts);
    let core_exclusions_after_raid = claims_after_raid.core_exclusions(&team_core_claims);
    let mut prior_non_lift_after_raid = claims_after_raid.without_lift(&team_core_claims);
    prior_non_lift_after_raid.extend_from_slice(context.utility_reservations);
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
            .air_operation()
            .filter(|operation| {
                operation.phase == AirOperationPhase::Recon && !operation.assault_admitted
            })
            .and_then(|operation| {
                context.intelligence.buildings().iter().find(|contact| {
                    contact.player == operation.target_player && contact.anchor == operation.target
                })
            })
            .map(|target| {
                lifts.prospective_first_carrier_commitment(
                    context.observation,
                    context.home,
                    &lift_unavailable_after_raid,
                    &core_exclusions_after_raid,
                    context.minimum_core_equivalents,
                    target,
                )
            })
            .unwrap_or(0)
    } else {
        0
    };
    let prospective_carrier_hold = funds.hold(prospective_carrier_commitment);
    strategic.committed_scrap = strategic
        .committed_scrap
        .saturating_add(prospective_carrier_hold);

    let outstanding_air_production_ticks =
        { strategy.remaining_airwork_ticks(context.observation) }.saturating_add(
            lifts.remaining_airwork_ticks(context.observation, &lift_unavailable_after_raid),
        );
    let utility_prior_commitment = funds.utility_commitment();
    let utility_spendable = context
        .observation
        .scrap
        .saturating_sub(utility_prior_commitment);
    OperationSettlement {
        strategic,
        team_decision,
        lift_decision,
        raid_decision,
        team_relief_core_ready,
        lift_rejected,
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

struct OperationFunds {
    unguarded: u32,
    spendable: u32,
    prior_utility: u32,
    committed: u32,
}

impl OperationFunds {
    fn available(&self) -> u32 {
        self.spendable.saturating_sub(self.committed)
    }
    fn commit(&mut self, amount: u32) {
        self.committed = self.committed.saturating_add(amount);
    }
    fn hold(&mut self, requested: u32) -> u32 {
        let held = requested.min(self.unguarded.saturating_sub(self.committed));
        self.commit(held);
        held
    }
    fn utility_commitment(&self) -> u32 {
        self.prior_utility.saturating_add(self.committed)
    }
}

fn can_admit_optional_raid(tuning: DifficultyTuning, strategic_load: usize) -> bool {
    strategic_load == 0 || tuning.attention_slots >= (strategic_load + 1).saturating_mul(2)
}

struct LiftGrant<'a> {
    prior_producer_intents: &'a [Intent],
    current_scrap: u32,
    unavailable: &'a [UnitId],
    core_exclusions: &'a [UnitId],
    minimum_core_equivalents: u64,
    allow_new: bool,
    producer_lanes: &'a ProducerLaneReservations,
}

struct AdmittedLift {
    planner: LiftPlanner,
    decision: StrategicDecision,
    claims: ClaimBundle,
}

impl AdmittedLift {
    fn commit(self, planner: &mut LiftPlanner) -> StrategicDecision {
        let Self {
            planner: accepted,
            mut decision,
            claims,
        } = self;
        *planner = accepted;
        decision.committed_scrap = claims.current_scrap().saturating_add(
            claims
                .producer_jobs()
                .iter()
                .map(|job| job.kind().stats().cost)
                .fold(0, u32::saturating_add),
        );
        decision
    }
}

impl LiftGrant<'_> {
    fn prepare(
        &self,
        observation: &Observation,
        home: TilePos,
        cadence: chassis::Tick,
        support: LiftAirSupport,
        planner: &LiftPlanner,
    ) -> Option<AdmittedLift> {
        let mut candidate = planner.clone();
        let decision = candidate.think_with_admission_and_producer_lanes(
            observation,
            home,
            self.unavailable,
            support,
            LiftAdmission {
                allow_new_commitments: self.allow_new,
                spendable_scrap: self.current_scrap,
                core_reservations: self.core_exclusions,
                minimum_core_equivalents: self.minimum_core_equivalents,
            },
            self.producer_lanes,
        );
        let units = prior_planner_claims(&[], None, &[], &[], candidate.operation());
        if candidate.operation().is_some() {
            let mut exclusions = self.core_exclusions.to_vec();
            exclusions.extend_from_slice(&units);
            if !combat_core_status(observation, &exclusions, &[], self.minimum_core_equivalents)
                .ready
            {
                return None;
            }
        }
        let mut claims = if decision
            .intents
            .iter()
            .any(|intent| matches!(intent, Intent::TrainAt { .. }))
        {
            let operation = candidate.operation()?;
            legacy_decision_obligation(
                &ResourceSnapshot::from_observation(observation),
                LegacyDecisionRequest {
                    cadence,
                    accepted_at: operation.started_at,
                    decision_tick: observation.tick,
                    channel: LegacyChannel::Lift,
                    sequence: 1,
                    decision: &decision,
                    prior_producer_intents: self.prior_producer_intents,
                    production_deadline: operation.deadline,
                },
            )
            .ok()?
            .claims
        } else {
            ClaimBundle::new(
                decision.committed_scrap,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .ok()?
        };
        if claims.claimed_capital() > u128::from(self.current_scrap)
            || units
                .iter()
                .any(|id| self.unavailable.binary_search(id).is_ok())
        {
            return None;
        }
        claims.units = units;
        Some(AdmittedLift {
            planner: candidate,
            decision,
            claims,
        })
    }
}

pub(in crate::bot) fn lift_unavailable(
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
    use super::*;
    use crate::bot::observation::{BuildingObs, UnitObs};
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
        let mut strategy = StrategicPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut raids = RaidPlanner::new();
        let outcome = settle_operations(
            OperationContext {
                cadence: 20,
                profile: &profile,
                tuning: DifficultyTuning::for_level(BotDifficulty::Prime),
                observation: &observation,
                intelligence: &intelligence,
                home: HOME,
                armies: &[],
                utility_reservations: &[],
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
            OperationParticipants {
                strategy: &mut strategy,
                lifts: &mut lifts,
                team: &mut team,
                raids: &mut raids,
            },
            RetainedDecisions {
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
    fn operation_grants_and_carrier_hold_preserve_the_utility_remainder() {
        let mut funds = OperationFunds {
            unguarded: 100,
            spendable: 60,
            prior_utility: 20,
            committed: 10,
        };
        assert_eq!(funds.available(), 50);
        funds.commit(30);
        assert_eq!(funds.available(), 20);
        assert_eq!(funds.hold(250), 60);
        assert_eq!(funds.hold(1), 0);
        assert_eq!(funds.utility_commitment(), 120);
    }

    #[test]
    fn lift_preparation_carries_exact_claims_without_mutating_the_owner() {
        let obs = lift_observation();
        let mut planner = LiftPlanner::new();
        let grant = LiftGrant {
            current_scrap: obs.scrap,
            unavailable: &[],
            core_exclusions: &[],
            minimum_core_equivalents: 0,
            allow_new: true,
            producer_lanes: ProducerLaneReservations::empty(),
            prior_producer_intents: &[],
        };
        let accepted = grant
            .prepare(&obs, HOME, 20, LiftAirSupport::Independent, &planner)
            .expect("the island has a funded lift");
        assert!(planner.operation().is_none());
        assert!(!accepted.claims.units().is_empty());
        assert!(accepted.claims.claimed_capital() <= u128::from(obs.scrap));
        assert!(!accepted.claims.producer_jobs().is_empty());
        let expected = accepted.planner.clone();
        let decision = accepted.commit(&mut planner);
        assert_eq!(planner, expected);
        assert!(decision.committed_scrap > 0);
    }

    #[test]
    fn lift_grant_preserves_the_core_and_cannot_spend_protected_capital() {
        let obs = lift_observation();
        let planner = LiftPlanner::new();
        let grant = LiftGrant {
            current_scrap: 0,
            unavailable: &[],
            core_exclusions: &[],
            minimum_core_equivalents: 8,
            allow_new: true,
            producer_lanes: ProducerLaneReservations::empty(),
            prior_producer_intents: &[],
        };
        let accepted = grant
            .prepare(&obs, HOME, 20, LiftAirSupport::Independent, &planner)
            .expect("declining a fresh lift still admits idle planner maintenance");
        assert_eq!(accepted.claims.claimed_capital(), 0);
        assert!(accepted.planner.operation().is_none());
        assert!(accepted.claims.units().is_empty());
        assert_eq!(planner, LiftPlanner::new());
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
        Observation::from_data(ObservationData {
            me: PlayerId(0),
            map_width: width,
            map_height: height,
            visible: vec![true; tile_count],
            explored: vec![true; tile_count],
            faction: Faction::Ferrous,
            ..Default::default()
        })
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
            provisional: false,
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
