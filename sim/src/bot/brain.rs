//! The composed player-facing rules-based bot.
//!
//! A [`Brain`] is an ordinary command source: it reads [`State`], emits
//! [`crate::PlayerCommand`]s, and its commands are recorded into
//! replays like anyone else's. Each think builds an observation, updates
//! current and remembered intelligence, advances persistent playbooks, asks the
//! utility policy for remaining work, and lets the executive reserve exact
//! units and lower intents to commands.

use super::PublicMapBriefing;
use super::difficulty::DifficultyTuning;
use super::executive::{Army, ArmyState, Executive, Intent};
use super::intelligence::StrategicIntelligence;
use super::lift::{LiftAirSupport, LiftOperation, LiftPlanner};
use super::observation::Observation;
use super::orient::Orientation;
use super::profile::ResolvedProfile;
use super::raid::RaidPlanner;
use super::strategy::{
    AirOperationOutcome, AirOperationPhase, LiftSupportRequest, StrategicCoordination,
    StrategicDecision, StrategicPlanner,
};
use super::team::TeamReliefPlanner;
use super::utility::{Dials, StrategicUtilityContext, UtilityPolicy};
use crate::command::{Command, PlayerCommand};
use crate::ids::{PlayerId, UnitId};
use crate::scenario::BotConfig;
use crate::state::State;
use chassis::grid::TilePos;
use chassis::rng::Pcg32;
use std::sync::Arc;

/// One brain, driving one player.
#[derive(Debug, Clone)]
pub struct Brain {
    player: PlayerId,
    dials: Dials,
    profile: Option<ResolvedProfile>,
    intelligence: Option<StrategicIntelligence>,
    strategy: Option<StrategicPlanner>,
    lifts: Option<LiftPlanner>,
    team: Option<TeamReliefPlanner>,
    raids: Option<RaidPlanner>,
    policy: UtilityPolicy,
    exec: Executive,
    /// The seat's frame of reference, latched at the first act and
    /// kept for the match — the policy's bot-local tile memory
    /// (blacklists, pending sites, scout rotation) lives in oriented
    /// space, and a mid-game flip when the home Foundry changes would
    /// silently mirror all of it.
    orientation: Option<Orientation>,
    /// Authored pre-match facts are a separate channel from live fog and
    /// memory. Only the player-facing controller receives one.
    public_map: Option<Arc<PublicMapBriefing>>,
    /// The immutable briefing transformed once into the latched policy frame.
    oriented_public_map: Option<PublicMapBriefing>,
}

impl Brain {
    /// Creates the brain for `player`. The scenario seed jitters the
    /// army-size threshold (±1) so mirror matches don't march in
    /// lockstep forever.
    pub fn new(player: PlayerId, scenario_seed: u64, dials: Dials) -> Self {
        Self::with_jitter(player, scenario_seed, 2000 + u64::from(player.0), dials)
    }

    fn with_jitter(
        player: PlayerId,
        jitter_seed: u64,
        jitter_stream: u64,
        mut dials: Dials,
    ) -> Self {
        let mut rng = Pcg32::new(jitter_seed, jitter_stream);
        dials.army_size = (dials.army_size + rng.next_below(3))
            .saturating_sub(1)
            .max(2);
        Self::with_dials(player, dials)
    }

    fn with_dials(player: PlayerId, dials: Dials) -> Self {
        Self {
            player,
            dials,
            profile: None,
            intelligence: None,
            strategy: None,
            lifts: None,
            team: None,
            raids: None,
            policy: UtilityPolicy::new(),
            exec: Executive::default(),
            orientation: None,
            public_map: None,
            oriented_public_map: None,
        }
    }

    /// The default Standard, Balanced, seed-zero player-facing profile.
    pub fn balanced(player: PlayerId, public_map: Arc<PublicMapBriefing>) -> Self {
        Self::scripted(player, BotConfig::default(), public_map)
    }

    /// Creates the player-facing opponent for an exact authored configuration.
    pub fn scripted(
        player: PlayerId,
        config: BotConfig,
        public_map: Arc<PublicMapBriefing>,
    ) -> Self {
        let profile = config.resolve_profile();
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(config.difficulty));
        let mut brain = Self::with_dials(player, dials);
        brain.profile = Some(profile);
        brain.intelligence = Some(StrategicIntelligence::new());
        brain.strategy = Some(StrategicPlanner::new());
        brain.lifts = Some(LiftPlanner::new());
        brain.team = Some(TeamReliefPlanner::new());
        brain.raids = Some(RaidPlanner::new());
        brain.public_map = Some(public_map);
        brain
    }

    /// The stable full-tree QA controller. Keep it separate from the
    /// player-facing constructor so bot tuning cannot silently move
    /// deterministic probes and fairness measurements.
    pub fn overseer(player: PlayerId, scenario_seed: u64) -> Self {
        Self::new(player, scenario_seed, Dials::overseer())
    }

    /// Creates a frozen Overseer with one explicit policy identity that is
    /// independent of the physical seat it drives.
    ///
    /// Evaluation pairs use this constructor so exchanging controllers does
    /// not also exchange the legacy seat-derived army-size jitter. Seed `N`
    /// exactly matches [`Self::overseer`] for seat zero at scenario seed `N`.
    pub fn overseer_with_policy_seed(player: PlayerId, policy_seed: u64) -> Self {
        Self::with_jitter(player, policy_seed, 2000, Dials::overseer())
    }

    /// The player this brain drives.
    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// The dials this brain thinks with.
    pub fn dials(&self) -> &Dials {
        &self.dials
    }

    /// The resolved player-facing personality, absent on custom and QA brains.
    pub fn profile(&self) -> Option<&ResolvedProfile> {
        self.profile.as_ref()
    }

    /// The executive's current bookkeeping (armies, rear line) — for
    /// tests and debug surfaces.
    pub fn executive(&self) -> &Executive {
        &self.exec
    }

    /// Commands for this tick (usually none — brains think on a cadence).
    pub fn act(&mut self, state: &State) -> Vec<PlayerCommand> {
        if state.result().is_some() || !state.current_tick().is_multiple_of(self.dials.cadence) {
            return Vec::new();
        }
        let obs = if self.dials.fog_honest {
            Observation::fog_honest(state, self.player)
        } else {
            Observation::omniscient(state, self.player)
        };
        // The wounded rear line lives on the home-side corner of the Foundry:
        // behind everything, and every march home routes past friendly
        // production. A footprint anchor is not itself a symmetric point goal
        // on an even-sized building, so select the same corner in each seat's
        // oriented frame before the raw executive acts.
        let (rear_anchor, rear_size) = obs
            .my_buildings
            .iter()
            .filter(|b| b.kind == crate::stats::BuildingKind::Foundry)
            .min_by_key(|b| b.id)
            .map(|b| (b.anchor, b.kind.base_stats().size))
            .unwrap_or((TilePos::new(0, 0), (1, 1)));
        let orientation = *self
            .orientation
            .get_or_insert_with(|| Orientation::for_home(&obs, rear_anchor));
        let rear = if self.profile.is_some() {
            player_facing_rear_tile(orientation, rear_anchor, rear_size)
        } else {
            rear_anchor
        };
        let mut commands = if self.profile.is_some() {
            self.exec.maintain_player_facing_with_tactics(
                self.player,
                &obs,
                rear,
                self.dials.coordinated_focus,
                self.dials.coordinated_defense_focus,
            )
        } else {
            self.exec.maintain(self.player, &obs, rear)
        };
        if let Some(recovery) = self.exec.harvester_recovery(self.player, &obs) {
            commands.extend(recovery);
            return commands;
        }
        // The policy thinks in seat-oriented space (see [`Orientation`]):
        // the same logic runs for both seats, so its compass-flavored
        // tie-breaks cannot systematically favor either one.
        let oriented = orientation.observe(&obs);
        let oriented_public_map = if self.profile.is_some() {
            let public_map = self
                .public_map
                .as_deref()
                .expect("a player-facing brain requires a public map briefing");
            Some(
                self.oriented_public_map
                    .get_or_insert_with(|| orientation.briefing(public_map)),
            )
        } else {
            None
        };
        let armies: Vec<_> = self
            .exec
            .armies()
            .iter()
            .map(|a| orientation.army(a.clone()))
            .collect();
        let enlisted: Vec<_> = self.exec.enlisted().collect();
        if self.profile.is_none() {
            let intents = self
                .policy
                .think(&self.dials, &oriented, &armies, &enlisted);
            let intents = orientation.emit(intents);
            let lowered = self.exec.apply(self.player, &obs, &intents);
            for command in &lowered {
                if let Command::Build { kind, anchor, .. } = command.command {
                    let oriented_anchor = orientation.anchor(anchor, kind.base_stats().size);
                    self.policy
                        .record_dispatched_build(&oriented, kind, oriented_anchor);
                }
            }
            commands.extend(lowered);
            return commands;
        }
        let oriented_home = oriented
            .my_buildings
            .iter()
            .filter(|building| building.kind == crate::stats::BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .map(|building| building.anchor)
            .unwrap_or(TilePos::new(0, 0));
        let profile = self
            .profile
            .as_ref()
            .expect("the profile-free controller returned through its legacy path");
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let team_claims = self
            .team
            .as_ref()
            .map_or_else(Vec::new, TeamReliefPlanner::reservations);
        let prior_non_lift_claims = prior_planner_claims(
            &enlisted,
            self.strategy
                .as_ref()
                .and_then(StrategicPlanner::air_operation),
            &team_claims,
            self.raids.as_ref().map_or(&[], RaidPlanner::reservations),
            None,
        );
        let planner_claims = prior_planner_claims(
            &enlisted,
            self.strategy
                .as_ref()
                .and_then(StrategicPlanner::air_operation),
            &team_claims,
            self.raids.as_ref().map_or(&[], RaidPlanner::reservations),
            self.lifts.as_ref().and_then(LiftPlanner::operation),
        );
        let team_external_claims = prior_planner_claims(
            &enlisted,
            self.strategy
                .as_ref()
                .and_then(StrategicPlanner::air_operation),
            &[],
            self.raids.as_ref().map_or(&[], RaidPlanner::reservations),
            self.lifts.as_ref().and_then(LiftPlanner::operation),
        );
        let prior_lift_unavailable =
            lift_unavailable(&oriented, &armies, &enlisted, &prior_non_lift_claims);
        let construction_commitment = UtilityPolicy::deferred_construction_commitment(&oriented);
        let active_air_production_ticks = self
            .strategy
            .as_ref()
            .map_or(0, |strategy| strategy.remaining_airwork_ticks(&oriented))
            .saturating_add(self.lifts.as_ref().map_or(0, |lifts| {
                lifts.remaining_airwork_ticks(&oriented, &prior_lift_unavailable)
            }));
        let air_capacity_active = self
            .strategy
            .as_ref()
            .is_some_and(|strategy| strategy.air_operation().is_some())
            || self
                .lifts
                .as_ref()
                .is_some_and(|lifts| lifts.operation().is_some());
        let airworks_capacity_commitment = self.policy.airworks_capacity_commitment(
            &self.dials,
            &oriented,
            oriented_home,
            air_capacity_active.then_some(active_air_production_ticks),
            &planner_claims,
        );
        let mut strategic_observation = oriented.clone();
        strategic_observation.scrap = strategic_observation
            .scrap
            .saturating_sub(construction_commitment)
            .saturating_sub(airworks_capacity_commitment);
        let lift_support_request = self
            .lifts
            .as_ref()
            .and_then(LiftPlanner::operation)
            .filter(|operation| operation.phase <= super::lift::LiftPhase::AwaitSupport)
            .map(|operation| LiftSupportRequest {
                player: operation.target_player,
                target: operation.target,
                planned_drops: operation.planned_drops.clone(),
            });
        let lift_was_active = lift_support_request.is_some();
        let mut strategic = match (self.intelligence.as_mut(), self.strategy.as_mut()) {
            (Some(intelligence), Some(strategy)) => {
                intelligence.update(&oriented);
                strategy.think_with_lift_support(
                    profile,
                    tuning,
                    &strategic_observation,
                    intelligence,
                    oriented_home,
                    StrategicCoordination {
                        enlisted: &planner_claims,
                        lift_support: lift_support_request.as_ref(),
                    },
                )
            }
            _ => Default::default(),
        };
        let air_active = self
            .strategy
            .as_ref()
            .is_some_and(|strategy| strategy.air_operation().is_some());
        if let Some(team) = self.team.as_mut() {
            let relief = team.think(
                profile,
                tuning,
                &strategic_observation,
                oriented_home,
                &team_external_claims,
                &strategic.reservations,
            );
            merge_strategic(&mut strategic, relief);
        }
        let team_active = self
            .team
            .as_ref()
            .is_some_and(|team| team.operation().is_some());
        // A raid group already mustering or under way keeps ownership while a
        // new lift is planned. A merely optional new raid does not get to
        // shrink the primary island assault before the lift sees the roster.
        let prior_raid_reservations: Vec<_> = self
            .raids
            .as_ref()
            .into_iter()
            .flat_map(|raids| raids.reservations().iter().copied())
            .collect();
        let mut reservations_before_lift = strategic.reservations.clone();
        reservations_before_lift.extend(prior_raid_reservations);
        reservations_before_lift.sort_unstable();
        reservations_before_lift.dedup();
        let lift_unavailable =
            lift_unavailable(&oriented, &armies, &enlisted, &reservations_before_lift);
        let prospective_carrier_commitment = self
            .strategy
            .as_ref()
            .and_then(StrategicPlanner::air_operation)
            .filter(|operation| {
                operation.phase == AirOperationPhase::Recon && !operation.assault_admitted
            })
            .and_then(|operation| {
                self.intelligence.as_ref().and_then(|intelligence| {
                    intelligence.buildings().iter().find(|contact| {
                        contact.player == operation.target_player
                            && contact.anchor == operation.target
                    })
                })
            })
            .and_then(|target| {
                self.lifts.as_ref().map(|lifts| {
                    lifts.prospective_first_carrier_commitment(
                        &oriented,
                        oriented_home,
                        &lift_unavailable,
                        target,
                    )
                })
            })
            .unwrap_or(0);
        let uncommitted_scrap = strategic_observation
            .scrap
            .saturating_sub(strategic.committed_scrap);
        strategic.committed_scrap = strategic
            .committed_scrap
            .saturating_add(prospective_carrier_commitment.min(uncommitted_scrap));
        let mut lift_observation = project_strategic_queues(&strategic_observation, &strategic);
        lift_observation.scrap = lift_observation
            .scrap
            .saturating_sub(strategic.committed_scrap);
        let mut support = self
            .strategy
            .as_ref()
            .map_or(LiftAirSupport::Independent, |strategy| {
                air_support(strategy.air_operation(), strategy.terminal_outcome())
            });
        let air_accepts_new_lift = self
            .strategy
            .as_ref()
            .and_then(StrategicPlanner::air_operation)
            .is_some_and(|operation| operation.phase != AirOperationPhase::Recover);
        if !lift_was_active {
            support = match (air_accepts_new_lift, support) {
                (true, LiftAirSupport::Released { player, target }) => {
                    LiftAirSupport::Suppressing { player, target }
                }
                (true, support @ LiftAirSupport::Suppressing { .. }) => support,
                _ => LiftAirSupport::Independent,
            };
        }
        if let Some(lifts) = self.lifts.as_mut() {
            let lift = lifts.think(&lift_observation, oriented_home, &lift_unavailable, support);
            merge_strategic(&mut strategic, lift);
        }
        let lift_active = self
            .lifts
            .as_ref()
            .is_some_and(|lifts| lifts.operation().is_some());

        let strategic_load =
            usize::from(air_active) + usize::from(team_active) + usize::from(lift_active);
        let raid_claimed = self
            .raids
            .as_ref()
            .is_some_and(|raids| !raids.reservations().is_empty());
        let can_begin_raid = can_admit_optional_raid(tuning, strategic_load);
        if (raid_claimed || can_begin_raid)
            && let Some(raids) = self.raids.as_mut()
        {
            let raid = raids.think(
                profile,
                tuning,
                &strategic_observation,
                oriented_home,
                &planner_claims,
                &strategic.reservations,
            );
            merge_strategic(&mut strategic, raid);
        }
        let outstanding_air_production_ticks = self
            .strategy
            .as_ref()
            .map_or(0, |strategy| strategy.remaining_airwork_ticks(&oriented))
            .saturating_add(self.lifts.as_ref().map_or(0, |lifts| {
                lifts.remaining_airwork_ticks(&oriented, &lift_unavailable)
            }));
        let mut policy_observation = oriented.clone();
        policy_observation.scrap = policy_observation
            .scrap
            .saturating_sub(strategic.committed_scrap);
        let mut reservations = strategic.reservations;
        let intelligence = self
            .intelligence
            .as_ref()
            .expect("the player-facing controller owns strategic intelligence");
        let utility_context = StrategicUtilityContext::new(
            &reservations,
            intelligence.units(),
            intelligence.buildings(),
            oriented_public_map
                .expect("the player-facing utility path has an oriented map briefing"),
            strategic.intents,
        );
        let utility_context = if air_active || lift_active {
            utility_context.with_outstanding_air_production_ticks(outstanding_air_production_ticks)
        } else {
            utility_context
        };
        let mut intents = self.policy.think_with_intelligence(
            &self.dials,
            &policy_observation,
            &armies,
            &enlisted,
            utility_context,
        );
        reservations.extend_from_slice(self.policy.worker_safety_reservations());
        reservations.sort_unstable();
        reservations.dedup();
        self.policy.bind_player_facing_builders(
            &oriented,
            intelligence.units(),
            intelligence.buildings(),
            &enlisted,
            &reservations,
            &mut intents,
        );
        let intents = orientation.emit(intents);
        let lowered = self
            .exec
            .apply_with_reservations(self.player, &obs, &intents, &reservations);
        for command in &lowered {
            if let Some(units) = queue_replacing_non_harvest_units(&command.command) {
                self.policy.record_dispatched_retask(units);
            }
            match &command.command {
                Command::Build { kind, anchor, .. } => {
                    let oriented_anchor = orientation.anchor(*anchor, kind.base_stats().size);
                    self.policy
                        .record_dispatched_build(&oriented, *kind, oriented_anchor);
                }
                Command::Harvest { units, node, .. } => {
                    let oriented_node = orientation.tile(*node);
                    for &unit in units {
                        self.policy
                            .record_dispatched_harvest(&oriented, unit, oriented_node);
                    }
                }
                _ => {}
            }
        }
        commands.extend(lowered);
        commands
    }
}

fn can_admit_optional_raid(tuning: DifficultyTuning, strategic_load: usize) -> bool {
    strategic_load == 0 || tuning.attention_slots >= (strategic_load + 1).saturating_mul(2)
}

/// The footprint tile that occupies its anchor corner in the owner's oriented
/// frame, mapped back into world space.
///
/// For an unflipped seat this is the raw anchor. A flipped even footprint uses
/// its opposite corner, making the two point goals exact tile mirrors while
/// keeping both inside their Foundries.
fn player_facing_rear_tile(orientation: Orientation, anchor: TilePos, size: (i32, i32)) -> TilePos {
    orientation.tile(orientation.anchor(anchor, size))
}

/// Unit orders that replace a worker's current Harvest program. Keeping this
/// match exhaustive makes a future command variant choose its bookkeeping
/// semantics explicitly.
fn queue_replacing_non_harvest_units(command: &Command) -> Option<&[UnitId]> {
    match command {
        Command::Move {
            units,
            queue: false,
            ..
        }
        | Command::Attack {
            units,
            queue: false,
            ..
        }
        | Command::AttackMove {
            units,
            queue: false,
            ..
        }
        | Command::Build {
            units,
            queue: false,
            ..
        }
        | Command::Repair {
            units,
            queue: false,
            ..
        }
        | Command::Salvage {
            units,
            queue: false,
            ..
        }
        | Command::RepairUnit {
            units,
            queue: false,
            ..
        }
        | Command::Advance {
            units,
            queue: false,
            ..
        }
        | Command::Load {
            units,
            queue: false,
            ..
        }
        | Command::Patrol { units, .. }
        | Command::Stop { units } => Some(units),
        Command::Move { queue: true, .. }
        | Command::Attack { queue: true, .. }
        | Command::AttackMove { queue: true, .. }
        | Command::Harvest { .. }
        | Command::Build { queue: true, .. }
        | Command::Repair { queue: true, .. }
        | Command::Salvage { queue: true, .. }
        | Command::RepairUnit { queue: true, .. }
        | Command::Advance { queue: true, .. }
        | Command::Load { queue: true, .. }
        | Command::Train { .. }
        | Command::Cancel { .. }
        | Command::CancelTrain { .. }
        | Command::SetRally { .. }
        | Command::Surrender
        | Command::FocusFire { .. }
        | Command::CancelFound { .. }
        | Command::UpgradeBuilding { .. }
        | Command::Unload { .. } => None,
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

fn prior_planner_claims(
    enlisted: &[UnitId],
    air: Option<&super::strategy::AirOperation>,
    relief: &[UnitId],
    raid: &[UnitId],
    lift: Option<&LiftOperation>,
) -> Vec<UnitId> {
    let mut claims = enlisted.to_vec();
    if let Some(operation) = air {
        claims.extend(operation.scout);
        claims.extend(operation.artillery.iter().copied());
        claims.extend(operation.bombers.iter().copied());
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
                if !manifest.closed && !manifest.attack_issued {
                    claims.extend(manifest.riders.iter().copied());
                }
            }
        }
    }
    claims.sort_unstable();
    claims.dedup();
    claims
}

fn lift_unavailable(
    obs: &Observation,
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
                    let target_is_contested = obs
                        .enemy_units
                        .iter()
                        .any(|unit| unit.tile.chebyshev(target) <= 8)
                        || obs.enemy_buildings.iter().any(|building| {
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

fn air_support(
    operation: Option<&super::strategy::AirOperation>,
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
        super::strategy::AirOperationPhase::Recon
        | super::strategy::AirOperationPhase::Assemble
        | super::strategy::AirOperationPhase::SuppressAa
        | super::strategy::AirOperationPhase::Verify => LiftAirSupport::Suppressing {
            player: shared.0,
            target: shared.1,
        },
        super::strategy::AirOperationPhase::Strike => LiftAirSupport::Released {
            player: shared.0,
            target: shared.1,
        },
        super::strategy::AirOperationPhase::Recover => {
            if operation.recovery_reason == Some(super::strategy::AirRecoveryReason::Complete) {
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

fn project_strategic_queues(obs: &Observation, decision: &StrategicDecision) -> Observation {
    let mut projected = obs.clone();
    for intent in &decision.intents {
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

#[cfg(test)]
mod tests {
    use super::super::lift::LiftPhase;
    use super::super::observation::{BuildingObs, OBSERVATION_VERSION, UnitObs};
    use super::*;
    use crate::bot::Specialty;
    use crate::ids::{BuildingId, PlayerId, Target};
    use crate::scenario::{BotDifficulty, BotStance, BuildingSpec, PlayerSpec, Scenario, UnitSpec};
    use crate::state::Faction;
    use crate::stats::{BuildingKind, UnitKind};
    use chassis::grid::TilePos;

    fn public_map(scenario: &Scenario) -> Arc<PublicMapBriefing> {
        Arc::new(
            PublicMapBriefing::from_scenario(scenario)
                .expect("the focused scenario has a public map briefing"),
        )
    }

    fn scripted_brain(scenario: &Scenario, player: PlayerId, config: BotConfig) -> Brain {
        Brain::scripted(player, config, public_map(scenario))
    }

    fn operation_identity_brain(player: PlayerId, scenario: &Scenario) -> Brain {
        let difficulty = BotDifficulty::Prime;
        let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
        let brain = scripted_brain(scenario, player, config);
        let profile = brain
            .profile
            .as_ref()
            .expect("the scripted brain owns a resolved profile");
        assert_eq!(
            (profile.primary, profile.secondary),
            (Specialty::Guile, Specialty::Fortification)
        );
        assert_eq!((profile.traits.air, profile.traits.guile), (40, 74));
        assert_eq!(
            brain.dials(),
            &Dials::scripted(profile, DifficultyTuning::for_level(difficulty)),
            "the public config must resolve the profile and matching dials together"
        );
        brain
    }

    fn assert_brain_unchanged(before: &Brain, after: &Brain) {
        assert_eq!(after.player, before.player);
        assert_eq!(after.dials, before.dials);
        assert_eq!(after.profile, before.profile);
        assert_eq!(after.intelligence, before.intelligence);
        assert_eq!(after.strategy, before.strategy);
        assert_eq!(after.lifts, before.lifts);
        assert_eq!(after.team, before.team);
        assert_eq!(after.raids, before.raids);
        assert_eq!(after.policy, before.policy);
        assert_eq!(after.exec, before.exec);
        assert_eq!(after.orientation, before.orientation);
        assert_eq!(after.public_map, before.public_map);
        assert_eq!(after.oriented_public_map, before.oriented_public_map);
    }

    #[test]
    fn only_the_player_facing_controller_receives_the_public_map_briefing() {
        let scenario = Scenario::skirmish();
        let public_map = public_map(&scenario);
        let scripted = Brain::scripted(PlayerId(0), BotConfig::default(), Arc::clone(&public_map));
        let overseer = Brain::overseer(PlayerId(0), scenario.seed);

        assert!(Arc::ptr_eq(
            scripted
                .public_map
                .as_ref()
                .expect("the scripted controller owns the briefing"),
            &public_map
        ));
        assert!(scripted.oriented_public_map.is_none());
        assert!(overseer.public_map.is_none());
        assert!(overseer.oriented_public_map.is_none());
    }

    #[test]
    fn interrupted_unsafe_turret_is_resolved_through_an_ordinary_state_command() {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].scrap = 1_000;
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Turret,
            x: 12,
            y: 5,
        });
        let mut state = scenario
            .build()
            .expect("the interrupted-site fixture builds");
        let requested_builder = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
            .expect("the home economy has a builder")
            .id;
        let anchor = TilePos::new(7, 5);
        let placed = state.tick(&[PlayerCommand {
            player: PlayerId(0),
            command: Command::Build {
                units: vec![requested_builder],
                kind: BuildingKind::Turret,
                anchor,
                queue: false,
                defer: false,
            },
        }]);
        assert!(
            placed
                .events
                .iter()
                .all(|event| !matches!(event, crate::event::Event::CommandRejected { .. })),
            "the paid Turret site must enter through the ordinary command boundary: {placed:?}"
        );
        let site = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0)
                    && building.kind == BuildingKind::Turret
                    && building.anchor == anchor
            })
            .expect("the accepted command placed the intended Turret")
            .id;

        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 9_001);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);
        while state.current_tick() < 12 {
            state.tick(&[]);
        }
        let active_builder = state
            .units()
            .iter()
            .find(|unit| matches!(unit.order, crate::state::Order::Build { site: target } if target == site))
            .map(|unit| unit.id)
            .expect("the builder is still working when danger interrupts the site");
        let evacuation = brain.act(&state);
        assert!(
            evacuation.iter().any(|command| matches!(
                &command.command,
                Command::Move { units, .. } if units.contains(&active_builder)
            )),
            "the visible gun must evacuate the active builder: {evacuation:?}"
        );
        assert!(
            evacuation.iter().all(|command| !matches!(
                command.command,
                Command::Cancel { building } if building == site
            )),
            "the staffed site is interrupted before it becomes an orphan"
        );
        state.tick(&evacuation);

        while state.current_tick() < 24 {
            state.tick(&[]);
        }
        assert!(matches!(
            state
                .unit(active_builder)
                .expect("the active builder survived")
                .order,
            crate::state::Order::Idle | crate::state::Order::Move { .. }
        ));
        assert!(
            state.building(site).is_some_and(|building| !building.built),
            "the interruption must leave a paid unfinished site"
        );

        let resolution = brain.act(&state);
        assert!(
            resolution.iter().any(|command| matches!(
                command.command,
                Command::Cancel { building } if building == site
            )),
            "an observably unsafe orphan Turret must be resolved instead of decaying forever: {resolution:?}"
        );
        let resolved = state.tick(&resolution);
        assert!(
            resolved
                .events
                .iter()
                .all(|event| !matches!(event, crate::event::Event::CommandRejected { .. })),
            "the ordinary cancellation must be accepted by State: {resolved:?}"
        );
        assert!(state.building(site).is_none());
        assert!(
            resolved.events.iter().any(|event| matches!(
                event,
                crate::event::Event::BuildCancelled {
                    building,
                    player: PlayerId(0),
                    refund,
                } if *building == site && *refund > 0
            )),
            "the ordinary partial-refund event must account for the abandoned investment: {resolved:?}"
        );
    }

    fn remote_expansion_defense_scenario() -> Scenario {
        let mut rows = vec![vec!['.'; 40]; 24];
        rows.first_mut().expect("map has a north edge").fill('#');
        rows.last_mut().expect("map has a south edge").fill('#');
        for row in &mut rows {
            row[0] = '#';
            row[39] = '#';
        }
        rows[5][5] = '1';
        rows[18][33] = '2';

        Scenario {
            name: "remote expansion defense".into(),
            seed: 0x0A16_0DEF,
            map: rows
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect(),
            players: vec![
                PlayerSpec {
                    name: "West Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 300,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "East Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 300,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Sentinel,
                    x: 8,
                    y: 7,
                },
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Sentinel,
                    x: 9,
                    y: 7,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Sentinel,
                    x: 27,
                    y: 13,
                },
            ],
            buildings: vec![BuildingSpec {
                player: 0,
                kind: BuildingKind::Foundry,
                x: 24,
                y: 12,
            }],
            meta: None,
        }
    }

    #[test]
    fn player_facing_brains_do_not_send_partial_musters_to_remote_expansions() {
        let scenario = remote_expansion_defense_scenario();
        let state = scenario.build().expect("the defense scenario builds");
        let observation = Observation::fog_honest(&state, PlayerId(0));
        assert_eq!(observation.enemy_units.len(), 1);
        let members: Vec<_> = observation.my_units.iter().map(|unit| unit.id).collect();
        assert_eq!(members.len(), 2);

        for difficulty in BotDifficulty::ALL {
            for personality_seed in 0..64 {
                let config = BotConfig::scripted(difficulty, BotStance::Balanced, personality_seed);
                let mut brain = scripted_brain(&scenario, PlayerId(0), config);
                let staging = TilePos::new(8, 7);
                let muster = brain.exec.apply_with_reservations(
                    PlayerId(0),
                    &observation,
                    &[Intent::FormArmy { staging, size: 2 }],
                    &[],
                );
                assert!(muster.iter().any(|command| matches!(
                    &command.command,
                    Command::AttackMove { units, goal, queue: false }
                        if units == &members && *goal == staging
                )));

                let commands = brain.act(&state);
                let army = &brain.exec.armies()[0];
                assert_eq!(
                    (army.state, army.target),
                    (ArmyState::Staging, None),
                    "{difficulty:?} seed {personality_seed} dispatched the partial body: {commands:?}"
                );
                assert!(commands.iter().all(|command| !matches!(
                    &command.command,
                    Command::AttackMove { units, .. }
                        if units.iter().any(|unit| members.contains(unit))
                )));
            }
        }
    }

    #[test]
    fn player_facing_rear_tiles_mirror_inside_odd_and_even_footprints() {
        let mut obs = test_island_observation();
        obs.map_width = 48;
        obs.map_height = 30;
        let left_anchor = TilePos::new(5, 4);

        for size in [(1, 1), (2, 2), (3, 1), (4, 2)] {
            let right_anchor = TilePos::new(
                obs.map_width - size.0 - left_anchor.x,
                obs.map_height - size.1 - left_anchor.y,
            );
            let left_orientation = Orientation::for_home(&obs, left_anchor);
            let right_orientation = Orientation::for_home(&obs, right_anchor);
            let left = player_facing_rear_tile(left_orientation, left_anchor, size);
            let right = player_facing_rear_tile(right_orientation, right_anchor, size);
            let inside = |tile: TilePos, anchor: TilePos| {
                tile.x >= anchor.x
                    && tile.x < anchor.x + size.0
                    && tile.y >= anchor.y
                    && tile.y < anchor.y + size.1
            };

            assert!(inside(left, left_anchor), "left {size:?}");
            assert!(inside(right, right_anchor), "right {size:?}");
            assert_eq!(
                TilePos::new(obs.map_width - 1 - left.x, obs.map_height - 1 - left.y,),
                right,
                "{size:?} footprint goals must be exact half-turns"
            );
        }
    }

    #[test]
    fn mirrored_wounded_armies_withdraw_to_mirrored_foundry_tiles() {
        let (width, height) = (30, 20);
        let mut rows = vec![vec!['.'; width]; height];
        rows.first_mut().expect("map has a north edge").fill('#');
        rows.last_mut().expect("map has a south edge").fill('#');
        for row in &mut rows {
            row[0] = '#';
            row[width - 1] = '#';
        }
        let left_foundry = TilePos::new(4, 4);
        let right_foundry = TilePos::new(24, 14);
        rows[left_foundry.y as usize][left_foundry.x as usize] = '1';
        rows[right_foundry.y as usize][right_foundry.x as usize] = '2';
        let left_unit_tile = TilePos::new(8, 6);
        let right_unit_tile = TilePos::new(21, 13);
        let scenario = Scenario {
            name: "mirrored wounded withdrawal".into(),
            seed: 1_616_101,
            map: rows
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect(),
            players: vec![
                PlayerSpec {
                    name: "West Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "East Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Sentinel,
                    x: left_unit_tile.x,
                    y: left_unit_tile.y,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Sentinel,
                    x: right_unit_tile.x,
                    y: right_unit_tile.y,
                },
            ],
            buildings: Vec::new(),
            meta: None,
        };
        let mut state = scenario.build().expect("the mirrored withdrawal builds");
        let wounded_hp = UnitKind::Sentinel.stats().max_hp / 4;
        for unit in &mut state.units {
            unit.hp = wounded_hp;
        }
        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_201);
        let public_map = public_map(&scenario);
        let mut brains = [
            Brain::scripted(PlayerId(0), config, Arc::clone(&public_map)),
            Brain::scripted(PlayerId(1), config, public_map),
        ];
        for (index, brain) in brains.iter_mut().enumerate() {
            let (player, staging) = if index == 0 {
                (PlayerId(0), left_unit_tile)
            } else {
                (PlayerId(1), right_unit_tile)
            };
            let obs = Observation::fog_honest(&state, player);
            let formed = brain.exec.apply_with_reservations(
                player,
                &obs,
                &[Intent::FormArmy { staging, size: 1 }],
                &[],
            );
            assert!(
                formed.iter().any(|command| matches!(
                    &command.command,
                    Command::AttackMove { units, goal, queue: false }
                        if units.len() == 1 && *goal == staging
                )),
                "the wounded machine must begin inside a real army"
            );
        }

        let withdrawal_goal = |brain: &mut Brain, player: PlayerId| {
            let commands = brain.act(&state);
            commands
                .iter()
                .find_map(|command| match &command.command {
                    Command::Move {
                        units,
                        goal,
                        queue: false,
                    } if units.iter().any(|unit| {
                        state
                            .unit(*unit)
                            .is_some_and(|member| member.player == player)
                    }) =>
                    {
                        Some(*goal)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| {
                    panic!("{player} did not withdraw its wounded army: {commands:?}")
                })
        };
        let left_goal = withdrawal_goal(&mut brains[0], PlayerId(0));
        let right_goal = withdrawal_goal(&mut brains[1], PlayerId(1));
        let foundry_size = BuildingKind::Foundry.base_stats().size;
        let inside = |goal: TilePos, anchor: TilePos| {
            goal.x >= anchor.x
                && goal.x < anchor.x + foundry_size.0
                && goal.y >= anchor.y
                && goal.y < anchor.y + foundry_size.1
        };
        assert!(inside(left_goal, left_foundry));
        assert!(inside(right_goal, right_foundry));
        assert_eq!(left_goal, left_foundry);
        assert_eq!(
            TilePos::new(
                width as i32 - 1 - left_goal.x,
                height as i32 - 1 - left_goal.y
            ),
            right_goal
        );
    }

    #[test]
    fn a_finished_brain_emits_nothing_and_preserves_all_controller_memory() {
        let scenario = Scenario::skirmish();
        let mut state = scenario.build().expect("the skirmish builds");
        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_042);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);

        let _ = brain.act(&state);
        assert_eq!(
            brain
                .intelligence
                .as_ref()
                .and_then(StrategicIntelligence::observed_at),
            Some(0),
            "the fixture must populate real strategic memory before the match ends"
        );
        state.tick(&[PlayerCommand {
            player: PlayerId(1),
            command: Command::Surrender,
        }]);
        assert!(state.result().is_some());
        let before = brain.clone();

        assert!(brain.act(&state).is_empty());
        assert_brain_unchanged(&before, &brain);
    }

    #[test]
    fn every_real_difficulty_thinks_only_on_its_authored_cadence() {
        for difficulty in BotDifficulty::ALL {
            let scenario = Scenario::skirmish();
            let mut state = scenario.build().expect("the skirmish builds");
            let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_042);
            let mut brain = scripted_brain(&scenario, PlayerId(0), config);
            let cadence = DifficultyTuning::for_level(difficulty).cadence;
            assert_eq!(brain.dials.cadence, cadence);

            for tick in 0..=super::super::difficulty::STRATEGIC_ADMISSION_CADENCE * 2 {
                assert_eq!(state.current_tick(), tick);
                let before = brain.clone();
                let commands = brain.act(&state);
                if tick.is_multiple_of(cadence) {
                    assert_eq!(
                        brain
                            .intelligence
                            .as_ref()
                            .and_then(StrategicIntelligence::observed_at),
                        Some(tick),
                        "{difficulty:?} did not observe on its cadence"
                    );
                } else {
                    assert!(commands.is_empty(), "{difficulty:?} acted at tick {tick}");
                    assert_brain_unchanged(&before, &brain);
                }
                state.tick(&[]);
            }
        }
    }

    fn calibration_open_ferrous() -> Scenario {
        let map = [
            "################################################",
            "#..............................................#",
            "#..............................................#",
            "#.......ss.....................................#",
            "#..............................................#",
            "#....1....E....##..............................#",
            "#..............................................#",
            "#..............................................#",
            "#...................#..........................#",
            "#...........s.......#..........................#",
            "#.............s................................#",
            "#................E.............................#",
            "#..............................................#",
            "#..................S...........................#",
            "#..............................................#",
            "#..............................................#",
            "#...........................S..................#",
            "#............................E.................#",
            "#..............................................#",
            "#................................s.............#",
            "#..........................#.......s...........#",
            "#..........................#...................#",
            "#..............................................#",
            "#...................................E....2.....#",
            "#..............................##..............#",
            "#..............................................#",
            "#.....................................ss.......#",
            "#..............................................#",
            "#..............................................#",
            "################################################",
        ];
        Scenario {
            name: "Calibration Open - Ferrous".into(),
            seed: 1_616_101,
            map: map.into_iter().map(str::to_owned).collect(),
            players: vec![
                PlayerSpec {
                    name: "West Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 150,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "East Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 150,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Harvester,
                    x: 6,
                    y: 8,
                },
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Harvester,
                    x: 7,
                    y: 8,
                },
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Harvester,
                    x: 8,
                    y: 7,
                },
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Sentinel,
                    x: 10,
                    y: 8,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Harvester,
                    x: 41,
                    y: 21,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Harvester,
                    x: 40,
                    y: 21,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Harvester,
                    x: 39,
                    y: 22,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Sentinel,
                    x: 37,
                    y: 21,
                },
            ],
            buildings: Vec::new(),
            meta: None,
        }
    }

    fn prime_defense_focus_scenario() -> Scenario {
        let mut rows = vec![vec!['.'; 30]; 18];
        rows.first_mut().expect("map has a north edge").fill('#');
        rows.last_mut().expect("map has a south edge").fill('#');
        for row in &mut rows {
            row[0] = '#';
            row[29] = '#';
        }
        rows[8][4] = '1';
        rows[8][24] = '2';

        Scenario {
            name: "prime defense focus".into(),
            seed: 0x0A16_DEF0,
            map: rows
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect(),
            players: vec![
                PlayerSpec {
                    name: "West Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "East Cupric".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Sentinel,
                    x: 12,
                    y: 8,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Sentinel,
                    x: 12,
                    y: 10,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Sentinel,
                    x: 24,
                    y: 4,
                },
            ],
            buildings: vec![
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Turret,
                    x: 8,
                    y: 7,
                },
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Turret,
                    x: 8,
                    y: 11,
                },
            ],
            meta: None,
        }
    }

    #[test]
    fn prime_alone_directs_overlapping_defenses_through_an_accepted_player_command() {
        let scenario = prime_defense_focus_scenario();
        let state = scenario.build().expect("the focus scenario builds");
        let prime_config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_042);
        let veteran_config =
            BotConfig::scripted(BotDifficulty::Veteran, BotStance::Balanced, 20_042);
        let mut prime = scripted_brain(&scenario, PlayerId(0), prime_config);
        let mut veteran = scripted_brain(&scenario, PlayerId(0), veteran_config);

        let prime_commands = prime.act(&state);
        let focus = prime_commands
            .iter()
            .find(|command| matches!(command.command, Command::FocusFire { .. }))
            .cloned()
            .expect("Prime should direct the overlapping turret line");
        assert!(
            veteran
                .act(&state)
                .iter()
                .all(|command| !matches!(command.command, Command::FocusFire { .. })),
            "Veteran should retain ordinary static-defense acquisition"
        );

        let (defenses, target) = match &focus.command {
            Command::FocusFire { buildings, target } => (buildings.clone(), *target),
            _ => unreachable!(),
        };
        assert_eq!(defenses.len(), 2);
        assert!(defenses.windows(2).all(|pair| pair[0] < pair[1]));
        let mut applied = state.clone();
        let report = applied.tick(&[focus]);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, crate::Event::CommandRejected { .. }))
        );
        for defense in defenses {
            assert_eq!(
                applied.building(defense).expect("defense stands").focus,
                Some(target)
            );
        }
    }

    #[test]
    fn prime_defense_focus_is_unchanged_by_hidden_authoritative_unit_state() {
        let scenario = prime_defense_focus_scenario();
        let state = scenario.build().expect("the focus scenario builds");
        let mut counterfactual = state.clone();
        let hidden = counterfactual
            .units()
            .iter()
            .find(|unit| unit.tile() == TilePos::new(24, 4))
            .expect("the hidden counterfactual unit exists")
            .id;
        counterfactual
            .unit_mut(hidden)
            .expect("the hidden unit remains live")
            .hp = 1;
        assert_eq!(
            Observation::fog_honest(&state, PlayerId(0)),
            Observation::fog_honest(&counterfactual, PlayerId(0)),
            "the fixture mutation must remain outside the bot's knowledge"
        );

        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_042);
        let mut baseline = scripted_brain(&scenario, PlayerId(0), config);
        let mut changed = scripted_brain(&scenario, PlayerId(0), config);
        assert_eq!(baseline.act(&state), changed.act(&counterfactual));
        assert_brain_unchanged(&baseline, &changed);
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

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum OpeningMacroCommand {
        Harvest {
            workers: Vec<usize>,
            node: TilePos,
        },
        Train {
            kind: UnitKind,
        },
        Build {
            kind: BuildingKind,
            anchor: TilePos,
            units: usize,
        },
        AttackMove {
            goal: TilePos,
            units: usize,
        },
        PaidSustain {
            units: usize,
        },
        Salvage {
            units: usize,
        },
        Upgrade,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct HarvestAdmissionSnapshot {
        workers: Vec<(usize, TilePos, u32)>,
        sources: Vec<(bool, TilePos, u32)>,
    }

    fn opening_harvest_snapshot(
        state: &State,
        brain: &Brain,
        player: PlayerId,
        orientation: Orientation,
    ) -> HarvestAdmissionSnapshot {
        let obs = orientation.observe(&Observation::fog_honest(state, player));
        let mut workers: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| {
                unit.kind.stats().harvest.is_some()
                    && unit.idle
                    && !brain.policy.worker_safety_reservations().contains(&unit.id)
            })
            .map(|unit| {
                (
                    crate::ids::owner_local_unit_rank(
                        unit.id,
                        player,
                        state.units().iter().map(|unit| (unit.id, unit.player)),
                    ),
                    unit.tile,
                    unit.carrying,
                )
            })
            .collect();
        workers.sort_unstable();

        let mut sources: Vec<_> = obs
            .known_scrap
            .iter()
            .filter(|(_, amount)| *amount > 0)
            .map(|(tile, amount)| (false, *tile, *amount))
            .chain(
                obs.known_wrecks
                    .iter()
                    .filter(|(_, amount)| *amount > 0)
                    .map(|(tile, amount)| (true, *tile, *amount)),
            )
            .collect();
        sources.sort_unstable();

        HarvestAdmissionSnapshot { workers, sources }
    }

    fn opening_macro_command(
        state: &State,
        player: PlayerId,
        orientation: Orientation,
        command: &Command,
    ) -> Option<OpeningMacroCommand> {
        match command {
            Command::Harvest { units, node, .. } => Some(OpeningMacroCommand::Harvest {
                workers: units
                    .iter()
                    .map(|unit| {
                        crate::ids::owner_local_unit_rank(
                            *unit,
                            player,
                            state.units().iter().map(|unit| (unit.id, unit.player)),
                        )
                    })
                    .collect(),
                node: orientation.tile(*node),
            }),
            Command::Train { kind, .. } => Some(OpeningMacroCommand::Train { kind: *kind }),
            Command::Build {
                units,
                kind,
                anchor,
                ..
            } => Some(OpeningMacroCommand::Build {
                kind: *kind,
                anchor: orientation.anchor(*anchor, kind.base_stats().size),
                units: units.len(),
            }),
            Command::AttackMove { units, goal, .. } => Some(OpeningMacroCommand::AttackMove {
                goal: orientation.tile(*goal),
                units: units.len(),
            }),
            Command::Repair { units, .. } | Command::RepairUnit { units, .. } => {
                Some(OpeningMacroCommand::PaidSustain { units: units.len() })
            }
            Command::Salvage { units, .. } => {
                Some(OpeningMacroCommand::Salvage { units: units.len() })
            }
            Command::UpgradeBuilding { .. } => Some(OpeningMacroCommand::Upgrade),
            _ => None,
        }
    }

    fn is_voluntary_opening_command(command: &Command) -> bool {
        matches!(
            command,
            Command::Harvest { .. }
                | Command::Train { .. }
                | Command::Build { .. }
                | Command::AttackMove { .. }
                | Command::Repair { .. }
                | Command::RepairUnit { .. }
                | Command::Salvage { .. }
                | Command::UpgradeBuilding { .. }
        )
    }

    #[test]
    fn veteran_does_not_trail_standard_during_the_shared_opening() {
        const OPENING_END: u64 = 5_000;

        let scenario = calibration_open_ferrous();
        let mut state = scenario.build().expect("the calibration opening builds");
        let personality_seed = 1_616_201;
        let public_map = public_map(&scenario);
        let mut brains = [
            Brain::scripted(
                PlayerId(0),
                BotConfig::scripted(
                    BotDifficulty::Standard,
                    BotStance::Balanced,
                    personality_seed,
                ),
                Arc::clone(&public_map),
            ),
            Brain::scripted(
                PlayerId(1),
                BotConfig::scripted(
                    BotDifficulty::Veteran,
                    BotStance::Balanced,
                    personality_seed,
                ),
                public_map,
            ),
        ];
        let orientations = [PlayerId(0), PlayerId(1)].map(|player| {
            let obs = Observation::fog_honest(&state, player);
            let home = obs
                .my_buildings
                .iter()
                .find(|building| building.kind == BuildingKind::Foundry)
                .expect("each calibration seat has a Foundry")
                .anchor;
            Orientation::for_home(&obs, home)
        });
        let mut macro_commands: [Vec<OpeningMacroCommand>; 2] = Default::default();
        let mut hauled = [0_u32; 2];
        let mut foundry_started: [Vec<u64>; 2] = Default::default();
        let mut foundry_completed: [Vec<u64>; 2] = Default::default();
        let mut equivalent_harvest_admissions = 0;

        // The current opening can make contact before its first expansion.
        // Stop at the guarded macro milestone instead of letting that early
        // skirmish skip the part of the opening this test exists to compare.
        while state.current_tick() < OPENING_END && foundry_completed.iter().any(Vec::is_empty) {
            let tick = state.current_tick();
            let harvest_snapshots = [0, 1].map(|seat| {
                opening_harvest_snapshot(
                    &state,
                    &brains[seat],
                    PlayerId(seat as u8),
                    orientations[seat],
                )
            });
            let commands: Vec<_> = brains
                .iter_mut()
                .flat_map(|brain| brain.act(&state))
                .collect();
            let mut harvest_commands: [Vec<OpeningMacroCommand>; 2] = Default::default();
            for command in &commands {
                let seat = usize::from(command.player.0);
                if is_voluntary_opening_command(&command.command) {
                    assert!(
                        super::super::difficulty::strategic_admission_tick(tick),
                        "seat {seat} issued voluntary opening macro at tick {tick} between shared boundaries: {command:?}"
                    );
                }
                if let Some(signature) = opening_macro_command(
                    &state,
                    command.player,
                    orientations[seat],
                    &command.command,
                ) {
                    if matches!(signature, OpeningMacroCommand::Harvest { .. }) {
                        harvest_commands[seat].push(signature);
                    } else {
                        macro_commands[seat].push(signature);
                    }
                }
                if matches!(
                    command.command,
                    Command::Build {
                        kind: BuildingKind::Foundry,
                        ..
                    }
                ) {
                    foundry_started[seat].push(tick);
                }
            }

            if harvest_snapshots[0] == harvest_snapshots[1] {
                if harvest_commands.iter().any(|commands| !commands.is_empty()) {
                    equivalent_harvest_admissions += 1;
                }
                assert_eq!(
                    harvest_commands[0], harvest_commands[1],
                    "equivalent owner-oriented worker/source snapshots must produce the same Harvest assignments at tick {tick}: {harvest_snapshots:?}"
                );
            } else {
                // Difficulty-specific reactions may leave different workers
                // eligible at a later shared boundary. Compare Harvest only
                // when both owner-oriented inputs are equivalent.
            }

            let report = state.tick(&commands);
            for event in report.events {
                match event {
                    crate::Event::ScrapDeposited { player, amount } => {
                        hauled[usize::from(player.0)] += amount;
                    }
                    crate::Event::BuildingCompleted {
                        player,
                        kind: BuildingKind::Foundry,
                        ..
                    } => foundry_completed[usize::from(player.0)].push(tick),
                    crate::Event::CommandRejected { player, reason } => {
                        panic!("seat {player} issued a rejected opening command: {reason:?}")
                    }
                    _ => {}
                }
            }
        }

        assert_eq!(
            macro_commands[0], macro_commands[1],
            "same-profile non-Harvest opening macro must follow the same admitted plan"
        );
        assert!(
            equivalent_harvest_admissions > 0,
            "the fixture never compared a real Harvest decision from equivalent snapshots"
        );
        assert!(
            hauled[1] >= hauled[0],
            "Veteran hauled less than Standard through the first expansion: {hauled:?}"
        );
        assert!(
            state.player(PlayerId(1)).scrap >= state.player(PlayerId(0)).scrap,
            "Veteran banked less than Standard through the first expansion: Standard={}, Veteran={}",
            state.player(PlayerId(0)).scrap,
            state.player(PlayerId(1)).scrap,
        );
        let started = foundry_started.each_ref().map(Vec::len);
        assert_eq!(
            started[0], started[1],
            "the paired opening must start the same number of Foundries: {foundry_started:?}"
        );
        assert!(
            started[0] > 0,
            "the fixture never reached an expansion start"
        );
        let completed = foundry_completed.each_ref().map(Vec::len);
        assert_eq!(
            completed[0], completed[1],
            "the paired opening must complete the same number of Foundries: {foundry_completed:?}"
        );
        assert!(
            completed[0] > 0,
            "the fixture never reached an expansion completion"
        );
        for milestone in 0..started[0] {
            assert!(
                foundry_started[1][milestone] <= foundry_started[0][milestone],
                "Veteran started Foundry {milestone} later: {foundry_started:?}"
            );
        }
        for milestone in 0..completed[0] {
            assert!(
                foundry_completed[1][milestone] <= foundry_completed[0][milestone],
                "Veteran completed Foundry {milestone} later: {foundry_completed:?}"
            );
        }
        let ordinary_core = |player| {
            let live = state
                .units()
                .iter()
                .filter(|unit| unit.player == player && unit.kind == UnitKind::Sentinel)
                .count();
            let queued = state
                .buildings()
                .iter()
                .filter(|building| building.player == player)
                .flat_map(|building| &building.queue)
                .filter(|kind| **kind == UnitKind::Sentinel)
                .count();
            live + queued
        };
        let core = [ordinary_core(PlayerId(0)), ordinary_core(PlayerId(1))];
        assert!(
            core[1] >= core[0],
            "Veteran ordinary core trailed: {core:?}"
        );
    }

    #[test]
    fn dispatched_move_replaces_harvest_memory_but_queued_move_does_not() {
        let unit = UnitId(3);
        let immediate = Command::Move {
            units: vec![unit],
            goal: TilePos::new(8, 5),
            queue: false,
        };
        let queued = Command::Move {
            units: vec![unit],
            goal: TilePos::new(8, 5),
            queue: true,
        };
        let harvest = Command::Harvest {
            units: vec![unit],
            node: TilePos::new(6, 5),
            queue: false,
        };

        assert_eq!(
            queue_replacing_non_harvest_units(&immediate),
            Some(&[unit][..])
        );
        assert_eq!(queue_replacing_non_harvest_units(&queued), None);
        assert_eq!(queue_replacing_non_harvest_units(&harvest), None);
    }

    #[test]
    fn every_nonqueued_worker_retask_clears_its_harvest_assignment() {
        let unit = UnitId(3);
        let commands = [
            Command::Repair {
                units: vec![unit],
                building: BuildingId(9),
                queue: false,
            },
            Command::RepairUnit {
                units: vec![unit],
                target: UnitId(4),
                queue: false,
            },
            Command::Advance {
                units: vec![unit],
                goal: TilePos::new(8, 5),
                queue: false,
            },
            Command::Patrol {
                units: vec![unit],
                waypoints: vec![TilePos::new(8, 5), TilePos::new(9, 5)],
            },
        ];

        for command in &commands {
            assert_eq!(
                queue_replacing_non_harvest_units(command),
                Some(&[unit][..]),
                "{command:?} replaces the worker's current Harvest program"
            );
        }

        for command in [
            Command::Repair {
                units: vec![unit],
                building: BuildingId(9),
                queue: true,
            },
            Command::RepairUnit {
                units: vec![unit],
                target: UnitId(4),
                queue: true,
            },
            Command::Advance {
                units: vec![unit],
                goal: TilePos::new(8, 5),
                queue: true,
            },
        ] {
            assert_eq!(
                queue_replacing_non_harvest_units(&command),
                None,
                "{command:?} preserves the active Harvest until the queue advances"
            );
        }
    }

    #[test]
    fn strategic_training_is_projected_into_the_matching_factory_queue_only() {
        let mut obs = test_island_observation();
        let airworks = BuildingId(2);
        obs.my_buildings.push(test_building(
            airworks.0,
            0,
            BuildingKind::Airworks,
            TEST_HOME.offset(4, 0),
        ));
        obs.my_queues.push(vec![UnitKind::Buzzard]);
        let decision = StrategicDecision {
            intents: vec![
                Intent::MoveUnits {
                    units: vec![UnitId(3)],
                    goal: TEST_HOME,
                },
                Intent::TrainAt {
                    building: airworks,
                    kind: UnitKind::Skyhook,
                },
                Intent::TrainAt {
                    building: BuildingId(999),
                    kind: UnitKind::Skyhook,
                },
            ],
            reservations: vec![UnitId(3)],
            committed_scrap: UnitKind::Skyhook.stats().cost,
        };

        let projected = project_strategic_queues(&obs, &decision);

        assert_eq!(obs.my_queues[1], [UnitKind::Buzzard]);
        assert_eq!(
            projected.my_queues[1],
            [UnitKind::Buzzard, UnitKind::Skyhook]
        );
        assert_eq!(projected.my_queues[0], obs.my_queues[0]);
        assert_eq!(projected.scrap, obs.scrap);
    }

    #[test]
    fn prior_operations_share_one_canonical_ownership_ledger() {
        use super::super::raid::{RaidObjective, RaidOperation, RaidPhase};
        use super::super::strategy::{AirOperation, AirOperationPhase};
        use super::super::team::{TeamReliefOperation, TeamReliefPhase};

        let air = AirOperation {
            target_player: PlayerId(1),
            target_kind: BuildingKind::Foundry,
            target: TilePos::new(20, 8),
            target_id: Some(BuildingId(3)),
            assault_admitted: true,
            phase: AirOperationPhase::Assemble,
            started_at: 100,
            phase_started_at: 120,
            scout: Some(UnitId(8)),
            scout_dispatch: None,
            bomber_hold: None,
            artillery_staging: None,
            artillery: vec![UnitId(7), UnitId(9)],
            bombers: vec![UnitId(10), UnitId(11)],
            strike_issued_at: None,
            recovery_reason: None,
        };
        let relief = TeamReliefOperation {
            ally: PlayerId(2),
            foundry: BuildingId(4),
            anchor: TilePos::new(12, 6),
            members: vec![UnitId(5), UnitId(7)],
            home_defenders: vec![UnitId(6)],
            committed_size: 2,
            committed_max_hp: 200,
            phase: TeamReliefPhase::Deploying,
            started_at: 90,
            phase_started_at: 90,
            exit_reason: None,
            dispatch: None,
        };
        let raid = RaidOperation {
            target_player: PlayerId(1),
            objective: RaidObjective::Unit {
                id: UnitId(30),
                kind: UnitKind::Harvester,
            },
            last_tile: TilePos::new(18, 7),
            members: vec![UnitId(3), UnitId(5)],
            committed_size: 2,
            phase: RaidPhase::Ingress,
            started_at: 80,
            phase_started_at: 80,
            exit_reason: None,
            dispatch: None,
        };

        assert_eq!(
            prior_planner_claims(
                &[UnitId(1), UnitId(8)],
                Some(&air),
                &relief.members,
                &raid.members,
                None,
            ),
            [
                UnitId(1),
                UnitId(3),
                UnitId(5),
                UnitId(7),
                UnitId(8),
                UnitId(9),
                UnitId(10),
                UnitId(11),
            ]
        );

        assert_eq!(
            air_support(Some(&air), None),
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TilePos::new(20, 8),
            }
        );
        let mut ghost_recon = air.clone();
        ghost_recon.assault_admitted = false;
        assert_eq!(
            air_support(Some(&ghost_recon), None),
            LiftAirSupport::Independent
        );
        let mut released = air.clone();
        released.phase = AirOperationPhase::Strike;
        assert_eq!(
            air_support(Some(&released), None),
            LiftAirSupport::Released {
                player: PlayerId(1),
                target: TilePos::new(20, 8),
            }
        );
        released.phase = AirOperationPhase::Recover;
        released.recovery_reason = Some(super::super::strategy::AirRecoveryReason::Complete);
        assert!(matches!(
            air_support(Some(&released), None),
            LiftAirSupport::Released { .. }
        ));
        released.recovery_reason = Some(super::super::strategy::AirRecoveryReason::NewAirDefense);
        assert!(matches!(
            air_support(Some(&released), None),
            LiftAirSupport::Aborted { .. }
        ));
        assert_eq!(
            air_support(
                None,
                Some(AirOperationOutcome::Released {
                    player: PlayerId(1),
                    target: TilePos::new(20, 8),
                }),
            ),
            LiftAirSupport::Released {
                player: PlayerId(1),
                target: TilePos::new(20, 8),
            }
        );
        assert_eq!(
            air_support(
                None,
                Some(AirOperationOutcome::Aborted {
                    player: PlayerId(1),
                    target: TilePos::new(20, 8),
                }),
            ),
            LiftAirSupport::Aborted {
                player: PlayerId(1),
                target: TilePos::new(20, 8),
            }
        );
    }

    #[test]
    fn a_pending_team_watch_enters_the_prior_planner_ledger() {
        let mut obs = test_island_observation();
        obs.known_rock.clear();
        obs.enemy_buildings.clear();
        obs.ally_buildings.push(test_building(
            600,
            2,
            BuildingKind::Foundry,
            TilePos::new(24, 15),
        ));
        obs.enemy_units.push(UnitObs {
            player: PlayerId(1),
            ..test_unit(90, UnitKind::Sentinel, TilePos::new(26, 15))
        });
        obs.my_units = [
            (1, TilePos::new(4, 14)),
            (2, TilePos::new(4, 16)),
            (3, TilePos::new(21, 14)),
            (4, TilePos::new(21, 16)),
            (5, TilePos::new(22, 15)),
            (6, TilePos::new(20, 15)),
        ]
        .map(|(id, tile)| test_unit(id, UnitKind::Sentinel, tile))
        .into();
        let mut profile =
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 0x0A16_7EA0)
                .resolve_profile();
        profile.traits.support = 70;
        profile.traits.fortification = 65;
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let mut relief = TeamReliefPlanner::new();

        let pending = relief.think(&profile, tuning, &obs, TEST_HOME, &[], &[]);
        assert!(relief.operation().is_none());
        assert!(!pending.reservations.is_empty());
        assert_eq!(
            prior_planner_claims(&[], None, &relief.reservations(), &[], None),
            pending.reservations,
            "the earlier air planner must see every exact unit frozen by the credibility watch"
        );
    }

    #[test]
    fn lift_ownership_joins_the_prior_planner_ledger_until_handoff() {
        use super::super::lift::{LiftManifest, LiftOperation, LiftPhase};

        let lift = LiftOperation {
            target_player: PlayerId(1),
            target_id: BuildingId(9),
            target: TilePos::new(20, 8),
            phase: LiftPhase::Landing,
            started_at: 100,
            phase_started_at: 120,
            deadline: 2_000,
            pickup_component: TilePos::new(5, 5),
            desired_carriers: 2,
            payload: vec![UnitId(2), UnitId(3), UnitId(4), UnitId(5)],
            payload_target: 4,
            ground_payload_target: 4,
            planned_drops: vec![TilePos::new(18, 7), TilePos::new(19, 7)],
            manifests: vec![
                LiftManifest {
                    carrier: UnitId(20),
                    riders: vec![UnitId(2), UnitId(3)],
                    pickup: TilePos::new(5, 5),
                    drop: TilePos::new(18, 7),
                    attack_issued: false,
                    load_dispatched: true,
                    boarding_closed: true,
                    unload_attempts: 1,
                    recovery_attempts: 0,
                    aborted: false,
                    closed: false,
                },
                LiftManifest {
                    carrier: UnitId(21),
                    riders: vec![UnitId(4), UnitId(5)],
                    pickup: TilePos::new(6, 5),
                    drop: TilePos::new(19, 7),
                    attack_issued: true,
                    load_dispatched: true,
                    boarding_closed: true,
                    unload_attempts: 1,
                    recovery_attempts: 0,
                    aborted: false,
                    closed: false,
                },
            ],
            launched: true,
        };

        assert_eq!(
            prior_planner_claims(&[UnitId(1)], None, &[], &[], Some(&lift)),
            [UnitId(1), UnitId(2), UnitId(3), UnitId(20), UnitId(21)]
        );

        let mut provisioning = lift;
        provisioning.phase = LiftPhase::Provision;
        provisioning.manifests.clear();
        provisioning.payload = vec![UnitId(6), UnitId(7), UnitId(8)];
        assert_eq!(
            prior_planner_claims(&[], None, &[], &[], Some(&provisioning)),
            [UnitId(6), UnitId(7), UnitId(8)],
            "the exact payload stays owned while its carriers are still training"
        );
    }

    #[test]
    fn lift_may_take_idle_staging_armies_without_stealing_active_defenders() {
        use super::super::executive::ArmyId;

        let mut obs = test_island_observation();
        let armies = [
            Army {
                id: ArmyId(1),
                members: vec![UnitId(1), UnitId(2)],
                state: ArmyState::Staging,
                staging: TilePos::new(3, 3),
                target: None,
                focus: None,
                progress: None,
                issued: None,
                bounces: 0,
            },
            Army {
                id: ArmyId(2),
                members: vec![UnitId(3)],
                state: ArmyState::Staging,
                staging: TilePos::new(4, 4),
                target: Some(TEST_TARGET),
                focus: None,
                progress: None,
                issued: None,
                bounces: 0,
            },
            Army {
                id: ArmyId(3),
                members: vec![UnitId(5)],
                state: ArmyState::Staging,
                staging: TEST_TARGET,
                target: Some(TEST_TARGET),
                focus: None,
                progress: None,
                issued: None,
                bounces: 0,
            },
            Army {
                id: ArmyId(4),
                members: vec![UnitId(6)],
                state: ArmyState::Pushing,
                staging: TilePos::new(4, 4),
                target: Some(TEST_TARGET),
                focus: None,
                progress: None,
                issued: None,
                bounces: 0,
            },
        ];

        assert_eq!(
            lift_unavailable(
                &obs,
                &armies,
                &[
                    UnitId(1),
                    UnitId(2),
                    UnitId(3),
                    UnitId(4),
                    UnitId(5),
                    UnitId(6),
                ],
                &[UnitId(2), UnitId(8)],
            ),
            [UnitId(2), UnitId(4), UnitId(5), UnitId(6), UnitId(8)]
        );

        obs.enemy_buildings.clear();
        obs.enemy_units.push(UnitObs {
            player: PlayerId(1),
            ..test_unit(90, UnitKind::Sentinel, TEST_TARGET.offset(1, 0))
        });
        assert_eq!(
            lift_unavailable(
                &obs,
                &armies,
                &[
                    UnitId(1),
                    UnitId(2),
                    UnitId(3),
                    UnitId(4),
                    UnitId(5),
                    UnitId(6),
                ],
                &[UnitId(2), UnitId(8)],
            ),
            [UnitId(2), UnitId(4), UnitId(5), UnitId(6), UnitId(8)],
            "a visible attacker must keep the objective-holding army out of the lift pool"
        );

        obs.enemy_units.clear();
        assert_eq!(
            lift_unavailable(
                &obs,
                &armies,
                &[
                    UnitId(1),
                    UnitId(2),
                    UnitId(3),
                    UnitId(4),
                    UnitId(5),
                    UnitId(6),
                ],
                &[UnitId(2), UnitId(8)],
            ),
            [UnitId(2), UnitId(4), UnitId(6), UnitId(8)],
            "the same idle holding army becomes transferable once its objective is uncontested"
        );
    }

    #[test]
    fn active_bulk_lift_holds_exact_airworks_capital_through_full_queues() {
        let airworks_cost = BuildingKind::Airworks
            .base_stats()
            .construction
            .expect("Airworks have a construction price")
            .cost;
        let fighting_reserve = UnitKind::Sentinel.stats().cost;
        let queued_cost = 2 * UnitKind::Skyhook.stats().cost + 2 * UnitKind::Sentinel.stats().cost;
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.players[0].scrap = queued_cost + airworks_cost + fighting_reserve;
        let mut state = scenario
            .build()
            .expect("bulk-lift capacity scenario builds");
        let airworks = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0) && building.kind == BuildingKind::Airworks
            })
            .expect("the authored Airworks stands")
            .id;
        let foundry = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0) && building.kind == BuildingKind::Foundry
            })
            .expect("the home Foundry stands")
            .id;
        let queue_orders = [
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Train {
                    building: airworks,
                    kind: UnitKind::Skyhook,
                },
            },
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Train {
                    building: airworks,
                    kind: UnitKind::Skyhook,
                },
            },
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Train {
                    building: foundry,
                    kind: UnitKind::Sentinel,
                },
            },
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Train {
                    building: foundry,
                    kind: UnitKind::Sentinel,
                },
            },
        ];
        let queued = state.tick(&queue_orders);
        assert!(
            queued.events.iter().all(|event| !matches!(
                event,
                crate::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "the setup queues must be legal: {:?}",
            queued.events
        );

        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 17);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);
        while !super::super::difficulty::strategic_admission_tick(state.current_tick()) {
            state.tick(&[]);
        }
        let obs = Observation::fog_honest(&state, PlayerId(0));
        let home = obs
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Foundry)
            .expect("the observation retains the home Foundry")
            .anchor;
        let queues: Vec<_> = obs
            .my_buildings
            .iter()
            .zip(&obs.my_queues)
            .filter(|(building, _)| {
                matches!(
                    building.kind,
                    BuildingKind::Foundry | BuildingKind::Airworks
                )
            })
            .map(|(building, queue)| (building.kind, queue.len()))
            .collect();
        assert!(
            queues.iter().all(|(_, depth)| *depth == 2),
            "both ordinary production queues begin at the planning depth: {queues:?}"
        );
        assert_eq!(
            obs.scrap,
            airworks_cost + fighting_reserve,
            "only the exact extra-Airworks fund remains"
        );

        let lifts = brain
            .lifts
            .as_mut()
            .expect("scripted brains own lift planners");
        let seeded = lifts.think(&obs, home, &[], LiftAirSupport::Independent);
        let operation = lifts
            .operation()
            .expect("the severed enclave starts a lift");
        assert_eq!(operation.phase, LiftPhase::Provision);
        assert!(
            operation.desired_carriers >= 8,
            "the fixture is a bulk lift"
        );
        assert!(
            lifts.remaining_airwork_ticks(&obs, &[]) > 2_400,
            "the active wave needs more than one Airworks' assembly horizon"
        );
        assert!(
            seeded.intents.iter().all(|intent| !matches!(
                intent,
                Intent::TrainAt {
                    building,
                    ..
                } if *building == airworks
            )),
            "the full Airworks queue cannot accept another planned order"
        );

        let commands = brain.act(&state);
        assert!(
            commands.iter().any(|command| matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Airworks,
                    ..
                }
            )),
            "Utility must receive and spend the capital hidden from active strategic production: {commands:?}"
        );
        assert!(
            commands.iter().all(|command| !matches!(
                command.command,
                Command::Train { building, .. } if building == airworks
            )),
            "the already-full Airworks must not consume the held construction fund: {commands:?}"
        );
        let report = state.tick(&commands);
        assert!(
            report.events.iter().all(|event| !matches!(
                event,
                crate::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "strategic reservations and residual utility spending must lower through one legal bank: {:?}",
            report.events
        );
    }

    #[test]
    fn primary_island_operations_claim_their_force_without_fragmenting_a_raid() {
        let scenario = combined_operation_scenario();
        let mut state = scenario
            .build()
            .expect("combined-operation scenario builds");
        for _ in 0..6_000 {
            state.tick(&[]);
        }

        let mut brain = operation_identity_brain(PlayerId(0), &scenario);

        let commands = brain.act(&state);

        let air = brain
            .strategy
            .as_ref()
            .and_then(StrategicPlanner::air_operation)
            .expect("the wealthy disconnected match starts the bomber operation");
        let lift = brain
            .lifts
            .as_ref()
            .and_then(LiftPlanner::operation)
            .expect("the same match starts its coordinated bulk lift");
        assert_eq!(lift.desired_carriers, 8);
        assert_eq!(lift.payload.len(), 32);
        assert_eq!(
            (lift.target_player, lift.target),
            (air.target_player, air.target),
            "the second-starting lift must inherit the air operation's exact objective"
        );
        assert_eq!(lift.planned_drops.len(), lift.desired_carriers);
        assert!(
            brain
                .raids
                .as_ref()
                .and_then(RaidPlanner::operation)
                .is_none(),
            "simultaneous air and lift work must consume Prime's optional-operation attention too"
        );

        let report = state.tick(&commands);
        assert!(
            report.events.iter().all(|event| !matches!(
                event,
                crate::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "all combined-operation commands must remain ordinary legal commands: {:?}",
            report.events
        );
    }

    #[test]
    fn difficulty_attention_admits_new_raids_by_load_but_never_drops_one_in_progress() {
        #[derive(Clone, Copy, Debug)]
        enum PrimaryLoad {
            None,
            Air,
            AirAndLift,
        }

        let scenario = combined_operation_scenario();
        let mut prepared = scenario
            .build()
            .expect("combined-operation scenario builds");
        for _ in 0..6_000 {
            prepared.tick(&[]);
        }

        for difficulty in BotDifficulty::ALL {
            for load in [PrimaryLoad::None, PrimaryLoad::Air, PrimaryLoad::AirAndLift] {
                let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
                let mut brain = scripted_brain(&scenario, PlayerId(0), config);
                brain.team = None;
                match load {
                    PrimaryLoad::None => {
                        brain.strategy = None;
                        brain.lifts = None;
                    }
                    PrimaryLoad::Air => brain.lifts = None,
                    PrimaryLoad::AirAndLift => {}
                }

                let _ = brain.act(&prepared);

                let air_active = brain
                    .strategy
                    .as_ref()
                    .and_then(StrategicPlanner::air_operation)
                    .is_some();
                let lift_active = brain
                    .lifts
                    .as_ref()
                    .and_then(LiftPlanner::operation)
                    .is_some();
                assert_eq!(
                    (air_active, lift_active),
                    match load {
                        PrimaryLoad::None => (false, false),
                        PrimaryLoad::Air => (true, false),
                        PrimaryLoad::AirAndLift => (true, true),
                    },
                    "the fixture must impose the requested strategic load for {difficulty:?}"
                );
                let expected_raid = match load {
                    PrimaryLoad::None => true,
                    PrimaryLoad::Air => {
                        matches!(difficulty, BotDifficulty::Veteran | BotDifficulty::Prime)
                    }
                    PrimaryLoad::AirAndLift => false,
                };
                assert_eq!(
                    brain
                        .raids
                        .as_ref()
                        .and_then(RaidPlanner::operation)
                        .is_some(),
                    expected_raid,
                    "{difficulty:?} with {load:?}"
                );
            }
        }

        for difficulty in BotDifficulty::ALL {
            let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
            let mut brain = scripted_brain(&scenario, PlayerId(0), config);
            brain.team = None;
            let profile = *brain
                .profile
                .as_ref()
                .expect("scripted brains own a resolved profile");
            let tuning = DifficultyTuning::for_level(difficulty);
            let obs = Observation::fog_honest(&prepared, PlayerId(0));
            let home = obs
                .my_buildings
                .iter()
                .find(|building| building.kind == BuildingKind::Foundry)
                .expect("the home Foundry stands")
                .anchor;
            let mut prior_raid = RaidPlanner::new();
            let raid_start = prior_raid.think(&profile, tuning, &obs, home, &[], &[]);
            assert!(matches!(
                raid_start.intents.as_slice(),
                [Intent::AttackMoveUnits { .. }]
            ));
            let prior_members = prior_raid
                .operation()
                .expect("zero load admits the raid on every rung")
                .members
                .clone();
            brain.raids = Some(prior_raid);

            let _ = brain.act(&prepared);

            assert!(
                brain
                    .strategy
                    .as_ref()
                    .and_then(StrategicPlanner::air_operation)
                    .is_some(),
                "{difficulty:?} must start the air operation in the continuation fixture"
            );
            assert!(
                brain
                    .lifts
                    .as_ref()
                    .and_then(LiftPlanner::operation)
                    .is_some(),
                "{difficulty:?} must start the lift operation in the continuation fixture"
            );
            assert_eq!(
                brain
                    .raids
                    .as_ref()
                    .and_then(RaidPlanner::operation)
                    .expect("attention limits cannot suspend a claimed raid")
                    .members,
                prior_members,
                "{difficulty:?}"
            );
        }
    }

    #[test]
    fn every_difficulty_preserves_first_carrier_capital_during_island_recon() {
        let mut scenario = prospective_lift_reservation_scenario();
        scenario.players[0].scrap = UnitKind::Skyhook.stats().cost;
        let mut state = scenario
            .build()
            .expect("prospective-lift reservation scenario builds");
        state.tick = 6_000;

        let raw = Observation::fog_honest(&state, PlayerId(0));
        assert_eq!(
            raw.scrap,
            UnitKind::Skyhook.stats().cost,
            "the only spendable capital must be the first carrier's exact cost"
        );
        assert!(
            raw.enemy_buildings.is_empty(),
            "the objective must be out of current sight"
        );
        assert!(
            (0..raw.map_height).all(|y| raw.known_rock_at(TilePos::new(20, y))),
            "the bot must honestly know the complete ground barrier"
        );
        let enemy_foundry = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(1) && building.kind == BuildingKind::Foundry
            })
            .expect("the enemy Foundry stands beyond the barrier");

        let mut spending_by_difficulty = Vec::new();
        for difficulty in BotDifficulty::ALL {
            let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
            let mut brain = scripted_brain(&scenario, PlayerId(0), config);
            assert!(
                state.current_tick().is_multiple_of(brain.dials().cadence),
                "the shared snapshot must be a think tick for {difficulty:?}"
            );

            let home = raw
                .my_buildings
                .iter()
                .filter(|building| building.kind == BuildingKind::Foundry)
                .min_by_key(|building| building.id)
                .expect("the home Foundry stands")
                .anchor;
            let orientation = Orientation::for_home(&raw, home);
            brain.orientation = Some(orientation);
            let mut prior = raw.clone();
            prior.tick = prior.tick.saturating_sub(100);
            prior.enemy_buildings.push(BuildingObs {
                id: enemy_foundry.id,
                player: enemy_foundry.player,
                kind: enemy_foundry.kind,
                anchor: enemy_foundry.anchor,
                hp: enemy_foundry.hp,
                built: enemy_foundry.built,
                seen: true,
                tier: enemy_foundry.tier,
            });
            let prior = orientation.observe(&prior);
            brain
                .intelligence
                .as_mut()
                .expect("scripted brains own strategic intelligence")
                .update(&prior);

            let oriented = orientation.observe(&raw);
            let oriented_home = orientation.anchor(home, BuildingKind::Foundry.base_stats().size);
            let mut expected_intelligence = brain
                .intelligence
                .as_ref()
                .expect("scripted brains own strategic intelligence")
                .clone();
            expected_intelligence.update(&oriented);
            let target = expected_intelligence
                .buildings()
                .first()
                .expect("the synthetic prior sighting creates one contact");
            assert_eq!(
                brain
                    .lifts
                    .as_ref()
                    .expect("scripted brains own lift planners")
                    .prospective_first_carrier_commitment(&oriented, oriented_home, &[], target,),
                UnitKind::Skyhook.stats().cost,
                "the fog-honest snapshot warrants exactly one prospective carrier for {difficulty:?}"
            );

            let commands = brain.act(&state);
            let operation = brain
                .strategy
                .as_ref()
                .and_then(StrategicPlanner::air_operation)
                .expect("remembered disconnected Foundry starts reconnaissance");
            assert_eq!(operation.phase, AirOperationPhase::Recon, "{difficulty:?}");
            assert!(!operation.assault_admitted, "{difficulty:?}");
            assert!(
                brain
                    .lifts
                    .as_ref()
                    .and_then(LiftPlanner::operation)
                    .is_none(),
                "prospective capital must not start or freeze a lift for {difficulty:?}"
            );
            let spending: Vec<_> = commands
                .iter()
                .filter_map(|command| match &command.command {
                    command @ (Command::Build { .. }
                    | Command::Train { .. }
                    | Command::UpgradeBuilding { .. }) => Some(command.clone()),
                    _ => None,
                })
                .collect();
            assert!(
                spending.is_empty(),
                "{difficulty:?} spent the first-carrier fund before current sight: {commands:?}"
            );
            spending_by_difficulty.push(spending);
        }

        assert!(
            spending_by_difficulty
                .windows(2)
                .all(|pair| pair[0] == pair[1]),
            "higher difficulties must preserve the lower rung's mandatory transport prefix"
        );
    }

    #[test]
    fn coordinated_air_and_bulk_lift_complete_one_shared_objective_cycle() {
        let scenario = combined_lifecycle_scenario();
        let mut state = scenario
            .build()
            .expect("combined lifecycle scenario builds");
        for _ in 0..6_000 {
            state.tick(&[]);
        }

        let mut brain = operation_identity_brain(PlayerId(0), &scenario);

        let mut shared_target = None;
        let mut bomber_release = false;
        let mut bomber_release_tick = None;
        let mut lift_held_before_air_release = false;
        let mut first_target_unload_tick = None;
        let mut carrier_loads = Vec::new();
        let mut target_unloads = Vec::new();
        let mut loaded_riders = Vec::new();
        let mut landed_assault = Vec::new();

        for _ in 0..8_000 {
            let commands = brain.act(&state);
            if shared_target.is_none()
                && let (Some(air), Some(lift)) = (
                    brain
                        .strategy
                        .as_ref()
                        .and_then(StrategicPlanner::air_operation),
                    brain.lifts.as_ref().and_then(LiftPlanner::operation),
                )
            {
                assert_eq!(
                    (air.target_player, air.target),
                    (lift.target_player, lift.target)
                );
                assert!(
                    lift.desired_carriers >= 3,
                    "the fixture must form a bulk lift"
                );
                shared_target = Some((air.target_id, air.target));
            }

            if let Some((target_id, target)) = shared_target {
                lift_held_before_air_release |= brain
                    .lifts
                    .as_ref()
                    .and_then(LiftPlanner::operation)
                    .is_some_and(|operation| {
                        matches!(
                            operation.phase,
                            LiftPhase::Boarding | LiftPhase::AwaitSupport
                        ) && !operation.manifests.is_empty()
                    })
                    && !bomber_release;
                for command in &commands {
                    match &command.command {
                        Command::Attack {
                            units,
                            target: command_target,
                            ..
                        } if *command_target
                            == Target::Building(
                                target_id.expect("the shared Foundry is currently identified"),
                            ) =>
                        {
                            let bombers = units
                                .iter()
                                .filter(|id| {
                                    state.unit(**id).is_some_and(|unit| {
                                        unit.player == PlayerId(0) && unit.kind == UnitKind::Moth
                                    })
                                })
                                .count();
                            let screen = units
                                .iter()
                                .filter(|id| {
                                    state.unit(**id).is_some_and(|unit| {
                                        unit.player == PlayerId(0) && unit.kind == UnitKind::Darter
                                    })
                                })
                                .count();
                            if bombers >= 4 && screen >= 2 {
                                bomber_release = true;
                                bomber_release_tick.get_or_insert(state.current_tick());
                            }
                        }
                        Command::Load {
                            units, transport, ..
                        } if state.unit(*transport).is_some_and(|unit| {
                            unit.player == PlayerId(0) && unit.kind == UnitKind::Skyhook
                        }) =>
                        {
                            carrier_loads.push(*transport);
                            loaded_riders.extend(units.iter().copied());
                        }
                        Command::Unload { transport, at, .. } if at.chebyshev(target) <= 6 => {
                            first_target_unload_tick.get_or_insert(state.current_tick());
                            target_unloads.push(*transport);
                        }
                        Command::AttackMove { units, goal, .. } if goal.chebyshev(target) <= 6 => {
                            landed_assault.extend(
                                units
                                    .iter()
                                    .copied()
                                    .filter(|unit| loaded_riders.contains(unit)),
                            );
                        }
                        _ => {}
                    }
                }
            }

            let report = state.tick(&commands);
            assert!(
                report.events.iter().all(|event| !matches!(
                    event,
                    crate::event::Event::CommandRejected {
                        player: PlayerId(0),
                        ..
                    }
                )),
                "the coordinated lifecycle emitted an illegal command: {:?}",
                report.events
            );

            carrier_loads.sort_unstable();
            carrier_loads.dedup();
            target_unloads.sort_unstable();
            target_unloads.dedup();
            loaded_riders.sort_unstable();
            loaded_riders.dedup();
            landed_assault.sort_unstable();
            landed_assault.dedup();
            if bomber_release
                && target_unloads.len() >= 3
                && !loaded_riders.is_empty()
                && landed_assault == loaded_riders
            {
                break;
            }
        }

        assert!(
            shared_target.is_some(),
            "both operations choose one objective"
        );
        assert!(
            lift_held_before_air_release,
            "the bulk wave boards without launching before the air corridor releases"
        );
        assert!(
            bomber_release,
            "the mixed bomber wing releases on that objective"
        );
        assert!(
            first_target_unload_tick >= bomber_release_tick,
            "the shared lift cannot launch before the bomber operation releases it"
        );
        assert!(
            carrier_loads.len() >= 3,
            "at least three carriers receive manifests"
        );
        assert!(
            !loaded_riders.is_empty(),
            "the manifests contain a real ground wave"
        );
        assert_eq!(
            target_unloads, carrier_loads,
            "the bulk wave launches together"
        );
        assert_eq!(
            landed_assault, loaded_riders,
            "every manifested rider is handed to the shared-objective assault"
        );
    }

    #[test]
    fn a_wealthy_island_brain_launches_grouped_bombers_without_a_lift_payload() {
        let scenario = independent_bomber_operation_scenario();
        let mut state = scenario
            .build()
            .expect("independent bomber scenario builds");
        for _ in 0..6_000 {
            state.tick(&[]);
        }

        assert!(
            state
                .units()
                .iter()
                .filter(|unit| unit.player == PlayerId(0))
                .all(|unit| unit.kind.stats().transport_size == 0)
        );
        let mut brain = operation_identity_brain(PlayerId(0), &scenario);

        let mut launched = None;
        for _ in 0..4_000 {
            let commands = brain.act(&state);
            for command in &commands {
                let units = match &command.command {
                    Command::Attack { units, .. } | Command::AttackMove { units, .. } => units,
                    _ => continue,
                };
                let bomber_count = units
                    .iter()
                    .filter(|id| {
                        state.units().iter().any(|unit| {
                            unit.id == **id
                                && unit.player == PlayerId(0)
                                && unit.kind == UnitKind::Condor
                        })
                    })
                    .count();
                let screen_count = units
                    .iter()
                    .filter(|id| {
                        state.units().iter().any(|unit| {
                            unit.id == **id
                                && unit.player == PlayerId(0)
                                && unit.kind == UnitKind::Buzzard
                        })
                    })
                    .count();
                if bomber_count >= 4 {
                    launched = Some((
                        state.current_tick(),
                        bomber_count,
                        screen_count,
                        units.len(),
                    ));
                    break;
                }
            }

            assert!(
                brain
                    .lifts
                    .as_ref()
                    .and_then(LiftPlanner::operation)
                    .is_none(),
                "an air-only roster cannot make the bomber operation depend on a lift"
            );
            let report = state.tick(&commands);
            assert!(
                report.events.iter().all(|event| !matches!(
                    event,
                    crate::event::Event::CommandRejected {
                        player: PlayerId(0),
                        ..
                    }
                )),
                "the independent bomber operation emitted an illegal command: {:?}",
                report.events
            );
            if launched.is_some() {
                break;
            }
        }

        let (tick, bombers, screen, wing) = launched.expect("the independent bomber wing launches");
        assert!(tick < 10_000);
        assert_eq!(bombers, 6);
        assert_eq!(screen, 3);
        assert_eq!(
            wing,
            bombers + screen,
            "the frozen roster launches together"
        );
    }

    #[test]
    fn an_active_raid_keeps_its_members_when_a_new_bulk_lift_forms() {
        let scenario = combined_operation_scenario();
        let mut state = scenario
            .build()
            .expect("combined-operation scenario builds");
        for _ in 0..6_000 {
            state.tick(&[]);
        }

        let mut brain = operation_identity_brain(PlayerId(0), &scenario);
        let profile = *brain
            .profile
            .as_ref()
            .expect("the scripted brain owns a resolved profile");
        let obs = Observation::fog_honest(&state, PlayerId(0));
        let home = obs
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Foundry)
            .expect("the home Foundry stands")
            .anchor;
        let mut prior_raid = RaidPlanner::new();
        prior_raid.think(
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            home,
            &[],
            &[],
        );
        let prior_members = prior_raid
            .operation()
            .expect("the reachable Extractor starts a raid")
            .members
            .clone();
        assert_eq!(prior_members.len(), 2);
        brain.raids = Some(prior_raid);

        brain.act(&state);

        let lift = brain
            .lifts
            .as_ref()
            .and_then(LiftPlanner::operation)
            .expect("the independent bulk lift also forms");
        assert_eq!(lift.desired_carriers, 8);
        assert!(
            prior_members
                .iter()
                .all(|member| !lift.payload.contains(member)),
            "a new lift cannot steal members from a raid already under way"
        );
    }

    #[test]
    fn partial_guile_muster_is_reserved_before_the_generic_army_draft() {
        let mut obs = test_island_observation();
        obs.known_rock.clear();
        obs.enemy_units.push(UnitObs {
            player: PlayerId(1),
            ..test_unit(600, UnitKind::Harvester, TilePos::new(20, 15))
        });
        obs.my_units
            .push(test_unit(1, UnitKind::Scuttler, TEST_HOME));
        obs.my_units.extend(
            (10..=13)
                .map(|id| test_unit(id, UnitKind::Sentinel, TEST_HOME.offset(id as i32 - 9, 0))),
        );
        obs.my_units.sort_unstable_by_key(|unit| unit.id);

        let briefing_scenario = Scenario::skirmish();
        let mut brain = operation_identity_brain(PlayerId(0), &briefing_scenario);
        let profile = *brain
            .profile
            .as_ref()
            .expect("the scripted brain owns a resolved profile");
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let raids = brain.raids.as_mut().expect("scripted brains own raids");
        let partial = raids.think(&profile, tuning, &obs, TEST_HOME, &[], &[]);
        assert_eq!(partial.reservations, [UnitId(1)]);
        assert!(partial.intents.is_empty());

        let prior_claims = prior_planner_claims(&[], None, &[], raids.reservations(), None);
        brain.exec.apply_with_reservations(
            PlayerId(0),
            &obs,
            &[Intent::FormArmy {
                staging: TEST_HOME,
                size: 5,
            }],
            &prior_claims,
        );
        assert!(
            brain
                .exec
                .armies()
                .iter()
                .flat_map(|army| &army.members)
                .all(|member| *member != UnitId(1)),
            "the partial exact muster must survive a generic draft"
        );

        obs.tick = super::super::difficulty::next_strategic_admission_tick(obs.tick);
        obs.my_units
            .push(test_unit(2, UnitKind::Scuttler, TEST_HOME.offset(1, 0)));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        let enlisted: Vec<_> = brain.exec.enlisted().collect();
        let complete = brain
            .raids
            .as_mut()
            .expect("scripted brains own raids")
            .think(&profile, tuning, &obs, TEST_HOME, &enlisted, &[]);
        assert_eq!(complete.reservations, [UnitId(1), UnitId(2)]);
        assert!(matches!(
            complete.intents.as_slice(),
            [Intent::AttackMoveUnits { units, .. }]
                if units == &[UnitId(1), UnitId(2)]
        ));
    }

    #[test]
    fn provisioning_lift_payload_does_not_ground_unreserved_defenders() {
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 5,
            y: 6,
        });
        let mut state = scenario.build().expect("double-booking scenario builds");
        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 17);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);
        let obs = Observation::fog_honest(&state, PlayerId(0));
        let staging = TilePos::new(8, 15);

        let muster = brain.exec.apply_with_reservations(
            PlayerId(0),
            &obs,
            &[Intent::FormArmy { staging, size: 40 }],
            &[],
        );
        assert!(muster.iter().any(|command| matches!(
            &command.command,
            Command::AttackMove { units, goal, queue: false }
                if units.len() == 40 && *goal == staging
        )));
        let army = brain.exec.armies()[0].clone();
        assert_eq!(army.state, ArmyState::Staging);
        assert_eq!(army.target, None);
        let enlisted: Vec<_> = brain.exec.enlisted().collect();
        let mut policy_probe = brain.policy.clone();
        let unreserved = policy_probe.think_player_facing(
            brain.dials(),
            &obs,
            std::slice::from_ref(&army),
            &enlisted,
            &[],
            brain
                .public_map
                .as_deref()
                .expect("scripted brains own a public map briefing"),
        );
        assert!(
            unreserved.iter().any(|intent| matches!(
                intent,
                Intent::PushArmy {
                    army: candidate,
                    target: TilePos { x: 5, y: 6 },
                } if *candidate == army.id
            )),
            "the fixture must offer the exact ground push that lift reservations suppress: {unreserved:?}"
        );

        for think in 0..2 {
            while !state.current_tick().is_multiple_of(brain.dials().cadence) {
                state.tick(&[]);
            }
            let commands = brain.act(&state);
            let operation = brain
                .lifts
                .as_ref()
                .and_then(LiftPlanner::operation)
                .expect("the severed enemy Foundry freezes a lift payload");
            assert_eq!(operation.phase, LiftPhase::Provision);
            assert!(!operation.payload.is_empty());
            assert!(
                operation
                    .payload
                    .iter()
                    .all(|unit| army.members.contains(unit))
            );
            assert!(
                commands.iter().all(|command| !matches!(
                    &command.command,
                    Command::AttackMove { units, .. }
                        if units.iter().any(|unit| operation.payload.contains(unit))
                )),
                "think {think} double-booked the frozen lift payload: {commands:?}"
            );
            let mut strategic_claims = operation.payload.clone();
            if let Some(air) = brain
                .strategy
                .as_ref()
                .and_then(StrategicPlanner::air_operation)
            {
                strategic_claims.extend(air.scout);
                strategic_claims.extend(air.artillery.iter().copied());
                strategic_claims.extend(air.bombers.iter().copied());
            }
            if let Some(relief) = brain.team.as_ref().and_then(TeamReliefPlanner::operation) {
                strategic_claims.extend(relief.members.iter().copied());
            }
            if let Some(raid) = brain.raids.as_ref().and_then(RaidPlanner::operation) {
                strategic_claims.extend(raid.members.iter().copied());
            }
            strategic_claims.sort_unstable();
            strategic_claims.dedup();
            let available: Vec<_> = army
                .members
                .iter()
                .copied()
                .filter(|unit| strategic_claims.binary_search(unit).is_err())
                .collect();
            if think == 0 {
                assert!(
                    commands.iter().any(|command| matches!(
                        &command.command,
                        Command::AttackMove {
                            units,
                            goal: TilePos { x: 5, y: 6 },
                            queue: false,
                        } if units == &available
                    )),
                    "unreserved members must remain available for the visible emergency: {commands:?}"
                );
            }
            let staged = brain
                .exec
                .armies()
                .iter()
                .find(|candidate| candidate.id == army.id)
                .expect("the defending remainder remains tracked");
            assert_eq!(staged.members, available, "think {think}");
            assert_eq!(staged.state, ArmyState::Pushing);
            assert_eq!(staged.target, Some(TilePos::new(5, 6)));

            state.tick(&[]);
        }
    }

    #[test]
    fn terminal_air_outcomes_reach_a_boarding_complete_lift_in_the_same_think() {
        let (obs, planner, manifest) = boarding_complete_lift();

        let mut released = planner.clone();
        let released_decision = released.think(
            &obs,
            TEST_HOME,
            &[],
            air_support(
                None,
                Some(AirOperationOutcome::Released {
                    player: PlayerId(1),
                    target: TEST_TARGET,
                }),
            ),
        );
        let released_operation = released.operation().expect("released lift remains active");
        assert_eq!(released_operation.phase, LiftPhase::Landing);
        assert!(released_operation.launched);
        assert!(released_decision.intents.contains(&Intent::Unload {
            transport: manifest.carrier,
            at: manifest.drop,
        }));

        let mut aborted = planner;
        let aborted_decision = aborted.think(
            &obs,
            TEST_HOME,
            &[],
            air_support(
                None,
                Some(AirOperationOutcome::Aborted {
                    player: PlayerId(1),
                    target: TEST_TARGET,
                }),
            ),
        );
        let aborted_operation = aborted.operation().expect("loaded carrier must recover");
        assert_eq!(aborted_operation.phase, LiftPhase::Recover);
        assert!(!aborted_operation.launched);
        assert!(
            aborted_decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::Unload { at, .. } if *at == manifest.drop
            )),
            "an aborted corridor must not become an independent target-side launch"
        );
    }

    const TEST_HOME: TilePos = TilePos::new(5, 15);
    const TEST_TARGET: TilePos = TilePos::new(50, 15);

    fn bulk_lift_capacity_scenario() -> Scenario {
        let mut rows = vec![vec!['.'; 40]; 24];
        rows.first_mut().expect("map has a north edge").fill('#');
        rows.last_mut().expect("map has a south edge").fill('#');
        for row in &mut rows {
            row[0] = '#';
            row[39] = '#';
            row[20] = '~';
        }
        rows[11][2] = '1';
        rows[11][36] = '2';

        let mut units: Vec<_> = (0..8)
            .map(|index| UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 4 + index,
                y: 8,
            })
            .collect();
        units.extend((0..40).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 3 + index % 14,
            y: 15 + index / 14,
        }));
        units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: 31,
            y: 11,
        });

        Scenario {
            name: "brain bulk-lift capacity".into(),
            seed: 0x0A16_00C0,
            map: rows
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect(),
            players: vec![
                PlayerSpec {
                    name: "Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units,
            buildings: vec![
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Fabricator,
                    x: 2,
                    y: 3,
                },
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Airworks,
                    x: 6,
                    y: 3,
                },
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Crucible,
                    x: 10,
                    y: 3,
                },
            ],
            meta: None,
        }
    }

    fn prospective_lift_reservation_scenario() -> Scenario {
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.units.retain(|unit| unit.kind != UnitKind::Kestrel);
        scenario.units.extend([2, 11, 21].map(|y| UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: 18,
            y,
        }));
        scenario.buildings.extend([
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Foundry,
                x: 14,
                y: 3,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Array,
                x: 14,
                y: 7,
            },
        ]);
        scenario
    }

    fn combined_operation_scenario() -> Scenario {
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.players[0].scrap = 50_000;
        for unit in scenario
            .units
            .iter_mut()
            .filter(|unit| unit.kind == UnitKind::Sentinel)
        {
            unit.kind = UnitKind::Scuttler;
        }
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: 14,
            y: 9,
        });
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Extractor,
            x: 14,
            y: 6,
        });
        scenario
    }

    fn combined_lifecycle_scenario() -> Scenario {
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.name = "brain combined air and lift lifecycle".into();
        scenario.players[0].faction = Faction::Cupric;
        scenario.players[0].scrap = 50_000;
        scenario.units.clear();
        scenario.units.extend((0..16).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Scuttler,
            x: 3 + index % 8,
            y: 15 + index / 8,
        }));
        scenario.units.extend((0..4).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Skyhook,
            x: 3 + index,
            y: 18,
        }));
        scenario.units.extend((0..6).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Moth,
            x: 8 + index,
            y: 18,
        }));
        scenario.units.extend((0..3).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Darter,
            x: 14 + index,
            y: 18,
        }));
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Gnat,
            x: 31,
            y: 11,
        });
        scenario
    }

    fn independent_bomber_operation_scenario() -> Scenario {
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.players[0].scrap = 0;
        scenario.units.clear();
        scenario.units.extend((0..6).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Condor,
            x: 3 + index,
            y: 15,
        }));
        scenario.units.extend((0..6).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Buzzard,
            x: 9 + index,
            y: 15,
        }));
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: 31,
            y: 11,
        });
        scenario.buildings.extend([
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 14,
                y: 3,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 17,
                y: 3,
            },
        ]);
        scenario
    }

    fn boarding_complete_lift() -> (Observation, LiftPlanner, super::super::lift::LiftManifest) {
        let mut obs = test_island_observation();
        obs.my_units.extend(
            (1..=3).map(|id| test_unit(id, UnitKind::Sentinel, TilePos::new(8 + id as i32, 8))),
        );
        obs.my_units
            .push(test_unit(900, UnitKind::Skyhook, TEST_HOME.offset(0, 8)));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);

        let mut planner = LiftPlanner::new();
        planner.think(&obs, TEST_HOME, &[], LiftAirSupport::Independent);
        let manifest = planner
            .operation()
            .expect("the lift enters boarding")
            .manifests[0]
            .clone();
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .expect("the assigned carrier is observable")
            .tile = manifest.pickup;
        obs.tick += 1;
        planner.think(&obs, TEST_HOME, &[], LiftAirSupport::Independent);
        obs.my_units
            .retain(|unit| !manifest.riders.contains(&unit.id));
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .expect("the assigned carrier survives boarding")
            .cargo = 3;
        obs.tick += 1;
        (obs, planner, manifest)
    }

    fn test_island_observation() -> Observation {
        let mut obs = Observation {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 0,
            map_width: 64,
            map_height: 32,
            my_units: Vec::new(),
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: vec![test_building(500, 1, BuildingKind::Foundry, TEST_TARGET)],
            visible: vec![true; 64 * 32],
            explored: vec![true; 64 * 32],
            known_scrap: Vec::new(),
            known_rock: (0..32).map(|y| TilePos::new(32, y)).collect(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        };
        obs.my_buildings.push(test_building(
            1,
            0,
            BuildingKind::Foundry,
            TEST_HOME.offset(-1, -1),
        ));
        obs.my_queues.push(Vec::new());
        obs
    }

    fn test_unit(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
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

    fn test_building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
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
