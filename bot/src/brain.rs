//! The composed player-facing rules-based bot.
//!
//! A [`Brain`] is an ordinary command source: it reads a player [`Observation`], emits
//! [`oxide_sim::PlayerCommand`]s, and its commands are recorded into
//! replays like anyone else's. Each think updates
//! current and remembered intelligence, advances persistent playbooks, asks the
//! utility policy for remaining work, and lets the executive reserve exact
//! units and lower intents to commands.

use super::PublicMapBriefing;
use super::allocation::AllocationParticipants;
#[cfg(test)]
use super::allocation::admission::retained_reservations;
#[cfg(test)]
use super::allocation::lift_air_support as air_support;
#[cfg(test)]
use super::allocation::prior_planner_claims;
use super::difficulty::DifficultyTuning;
use super::executive::Executive;
#[cfg(test)]
use super::executive::{Army, ArmyState};
use super::intelligence::StrategicIntelligence;
use super::lift::LiftPlanner;
#[cfg(test)]
use super::lift::{LiftAdmission, LiftAirSupport};
use super::observation::Observation;
use super::observer::{BotPhase, PhaseObserver, PhaseScope};
use super::orient::Orientation;
use super::profile::ResolvedProfile;
use super::raid::RaidPlanner;
#[cfg(test)]
use super::{
    allocation::operations::lift_unavailable,
    executive::Intent,
    strategy::{
        AirOperationPhase, StrategicCoordination, StrategicDecision, StrategicThinkContext,
    },
    utility::combat_core_status,
};
#[cfg(test)]
use super::{
    team::TeamReliefAdmission,
    trace::{ChannelPhase, ChannelState},
};

use super::resources::BuilderLease;
#[cfg(test)]
use super::resources::{ProducerLaneReservations, ReservedProducerJob, ResourceSnapshot};
use super::strategy::StrategicPlanner;
#[cfg(test)]
use super::strategy::{AirOperationOutcome, AirRecoveryReason};
use super::team::TeamReliefPlanner;
use super::trace::{
    DecisionControlFlow, DecisionTraceRecorder, LoweringTrace, TracedBotAct, UtilityTrace,
    bounded_count,
};
use super::utility::{Dials, UtilityPolicy};
#[cfg(test)]
use crate::observation::ObservationData;
use chassis::grid::TilePos;
#[cfg(test)]
use oxide_sim::command::Command;
use oxide_sim::command::PlayerCommand;
use oxide_sim::ids::PlayerId;
#[cfg(test)]
use oxide_sim::ids::UnitId;
use oxide_sim::scenario::BotConfig;
use std::sync::Arc;

mod checkpoint;

/// The personality, intelligence, strategic planners, and authored map briefing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct PlayerFacingMind {
    profile: ResolvedProfile,
    intelligence: StrategicIntelligence,
    battlefield: super::battlefield::Battlefield,
    experience: super::experience::Experience,
    strategy: StrategicPlanner,
    lifts: LiftPlanner,
    team: TeamReliefPlanner,
    raids: RaidPlanner,
    /// Authored pre-match facts are a separate channel from live fog and
    /// memory. Only the player-facing controller receives one.
    public_map: Arc<PublicMapBriefing>,
    /// The immutable briefing transformed once into the latched policy frame.
    oriented_public_map: Option<PublicMapBriefing>,
}

/// One brain, driving one player.
#[derive(Debug, Clone)]
pub struct Brain {
    player: PlayerId,
    dials: Dials,
    mind: Box<PlayerFacingMind>,
    policy: UtilityPolicy,
    exec: Executive,
    /// The seat's frame of reference, latched at the first act and
    /// kept for the match — the policy's bot-local tile memory
    /// (work attempts, failure exclusions, scout rotation) lives in oriented
    /// space, and a mid-game flip when the home Foundry changes would
    /// silently mirror all of it.
    orientation: Option<Orientation>,
}

impl Brain {
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
        let profile = crate::profile::ResolvedProfile::resolve(config);
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(config.difficulty));
        Self {
            player,
            dials,
            policy: UtilityPolicy::new(),
            exec: Executive::default(),
            orientation: None,
            mind: Box::new(PlayerFacingMind {
                profile,
                intelligence: StrategicIntelligence::new(),
                battlefield: Default::default(),
                experience: Default::default(),
                strategy: StrategicPlanner::new(),
                lifts: LiftPlanner::new(),
                team: TeamReliefPlanner::new(),
                raids: RaidPlanner::new(),
                public_map,
                oriented_public_map: None,
            }),
        }
    }

    /// The player this brain drives.
    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// The dials this brain thinks with.
    pub fn dials(&self) -> &Dials {
        &self.dials
    }

    /// The resolved player-facing personality.
    pub fn profile(&self) -> &ResolvedProfile {
        &self.mind.profile
    }

    #[cfg(test)]
    fn mind(&self) -> &PlayerFacingMind {
        &self.mind
    }

    #[cfg(test)]
    fn mind_mut(&mut self) -> &mut PlayerFacingMind {
        &mut self.mind
    }

    /// The executive's current bookkeeping (armies, rear line) — for
    /// tests and debug surfaces.
    pub fn executive(&self) -> &Executive {
        &self.exec
    }

    /// Whether the controller cadence schedules a decision at this tick.
    pub fn decision_due(&self, tick: chassis::Tick) -> bool {
        tick.is_multiple_of(self.dials.cadence)
    }

    /// Commands for this tick (usually none — brains think on a cadence).
    pub fn act(&mut self, obs: &Observation) -> Vec<PlayerCommand> {
        self.act_inner(obs, None, None)
    }

    /// Commands plus an observational trace for a player-facing decision tick.
    ///
    /// Observations for another seat, seats without a physical Foundry, and
    /// cadence skips return no trace. The host adapter gates finished matches. The recorder is stack-local and cannot become controller
    /// or replay state.
    pub fn act_traced(&mut self, obs: &Observation) -> TracedBotAct {
        let mut recorder = Some(DecisionTraceRecorder::default());
        let commands = self.act_inner(obs, recorder.as_mut(), None);
        TracedBotAct {
            commands,
            trace: recorder.and_then(DecisionTraceRecorder::finish),
        }
    }

    pub(super) fn act_observed(
        &mut self,
        obs: &Observation,
        observer: &dyn PhaseObserver,
    ) -> Vec<PlayerCommand> {
        self.act_inner(obs, None, Some(observer))
    }

    fn act_inner(
        &mut self,
        obs: &Observation,
        mut recorder: Option<&mut DecisionTraceRecorder>,
        observer: Option<&dyn PhaseObserver>,
    ) -> Vec<PlayerCommand> {
        if obs.me != self.player || !self.decision_due(obs.tick) {
            return Vec::new();
        }
        let _query_capture = super::query_work::Capture::new(observer);
        let maintenance_scope = PhaseScope::new(observer, BotPhase::Maintenance);
        if !obs.my_buildings.iter().any(|building| {
            !building.provisional && building.kind == oxide_sim::stats::BuildingKind::Foundry
        }) {
            return Vec::new();
        }
        self.policy.planning.begin(obs.tick);
        if let Some(recorder) = recorder.as_deref_mut() {
            recorder.begin(obs);
        }
        // The wounded rear line lives on the home-side corner of the Foundry:
        // behind everything, and every march home routes past friendly
        // production. A footprint anchor is not itself a symmetric point goal
        // on an even-sized building, so select the same corner in each seat's
        // oriented frame before the raw executive acts.
        let (rear_anchor, rear_size) = obs
            .my_buildings
            .iter()
            .filter(|b| !b.provisional && b.kind == oxide_sim::stats::BuildingKind::Foundry)
            .min_by_key(|b| b.id)
            .map(|b| (b.anchor, b.kind.base_stats().size))
            .unwrap_or((TilePos::new(0, 0), (1, 1)));
        let orientation = *self
            .orientation
            .get_or_insert_with(|| Orientation::for_home(obs, rear_anchor));
        let rear = player_facing_rear_tile(orientation, rear_anchor, rear_size);
        self.exec.mission_decisions.clear();
        let mut commands = self.exec.maintain_player_facing_with_tactics(
            self.player,
            obs,
            rear,
            self.dials.coordinated_focus,
            self.dials.coordinated_defense_focus,
        );
        let maintenance_commands = commands.len();
        let oriented = orientation.observe(obs);
        let armies: Vec<_> = self
            .exec
            .armies()
            .iter()
            .map(|army| orientation.army(army.clone()))
            .collect();
        {
            let mind = &mut self.mind;
            let tuning = DifficultyTuning::for_level(mind.profile.difficulty);
            let map = mind
                .oriented_public_map
                .get_or_insert_with(|| orientation.briefing(&mind.public_map));
            mind.battlefield
                .observe(&oriented, &armies, tuning, Some(map));
            mind.experience
                .observe(&oriented, tuning.opponent_force_memory);
            self.policy.observe_work_experience(&oriented);
            for journal in self.exec.ground_outcomes.values_mut() {
                for mut report in std::mem::take(&mut journal.pending) {
                    orientation.episode(&mut report);
                    mind.experience.report(report);
                }
            }
            let live_armies: std::collections::BTreeSet<_> =
                self.exec.armies().iter().map(|army| army.id).collect();
            self.exec
                .ground_outcomes
                .retain(|id, _| live_armies.contains(id));
            self.exec.missions.retain(|id, _| live_armies.contains(id));
            for report in std::mem::take(&mut self.policy.state.work_experience.pending) {
                mind.experience.report(report);
            }
            for journal in [
                &mut mind.strategy.outcomes,
                &mut mind.lifts.outcomes,
                &mut mind.raids.outcomes,
                &mut mind.team.outcomes,
            ] {
                journal.observe_follow_through(&oriented);
                for report in std::mem::take(&mut journal.pending) {
                    mind.experience.report(report);
                }
            }
            mind.battlefield
                .review_approaches(&oriented, &mind.experience);
            mind.strategy.experience = Arc::new(mind.experience.clone());
            if let Some(recorder) = recorder.as_deref_mut() {
                recorder.trace_mut().battlefield = Some(mind.battlefield.assessment().clone());
                recorder.trace_mut().experience = mind.experience.trace();
                recorder.trace_mut().missions = self
                    .exec
                    .missions
                    .iter()
                    .map(|(id, mission)| (id.0, mission.clone()))
                    .collect();
            }
        }
        if let Some(recovery) = self.exec.harvester_recovery(self.player, obs) {
            commands.extend(recovery);
            let recon_paid_exclusions = self.policy.state.reconnaissance.paid_exclusions();
            let strategic_recovery = {
                let mind = &mut self.mind;
                let PlayerFacingMind {
                    profile,
                    strategy,
                    public_map,
                    oriented_public_map,
                    ..
                } = mind.as_mut();
                let oriented_public_map: &PublicMapBriefing =
                    oriented_public_map.get_or_insert_with(|| orientation.briefing(public_map));
                let home = oriented
                    .my_buildings
                    .iter()
                    .filter(|building| {
                        !building.provisional
                            && building.kind == oxide_sim::stats::BuildingKind::Foundry
                    })
                    .min_by_key(|building| building.id)
                    .map(|building| building.anchor)
                    .unwrap_or(TilePos::new(0, 0));
                strategy.recover_unpaid_connected_for_economy_emergency(
                    super::strategy::EconomyEmergencyRecovery {
                        profile,
                        tuning: DifficultyTuning::for_level(profile.difficulty),
                        obs: &oriented,
                        home,
                        public_map: Some(oriented_public_map),
                        orientation,
                        recon_paid_exclusions: &recon_paid_exclusions,
                    },
                )
            };
            if let Some(strategic_recovery) = strategic_recovery {
                let reservations = strategic_recovery.reservations;
                let intents = orientation.emit(strategic_recovery.intents);
                commands.extend(self.exec.apply_with_reservations(
                    self.player,
                    obs,
                    &intents,
                    &reservations,
                ));
            }
            let recovery_commands = commands.len().saturating_sub(maintenance_commands);
            if let Some(recorder) = recorder.as_deref_mut() {
                let trace = recorder.trace_mut();
                trace.control_flow = DecisionControlFlow::HarvesterRecovery;
                trace.lowering = LoweringTrace {
                    maintenance_commands: bounded_count(maintenance_commands),
                    decision_commands: bounded_count(recovery_commands),
                    total_commands: bounded_count(commands.len()),
                };
            }
            if let Some(observer) = observer {
                observer.planning_work(self.policy.planning.stats());
            }
            return commands;
        }
        // The policy thinks in seat-oriented space (see [`Orientation`]):
        // the same logic runs for both seats, so its compass-flavored
        // tie-breaks cannot systematically favor either one.
        drop(maintenance_scope);
        let strategy_scope = PhaseScope::new(observer, BotPhase::Strategy);
        let enlisted: Vec<_> = self.exec.enlisted().collect();
        let mind = &mut self.mind;
        let PlayerFacingMind {
            profile,
            intelligence,
            strategy,
            lifts,
            team,
            raids,
            public_map,
            oriented_public_map,
            battlefield,
            experience,
            ..
        } = mind.as_mut();
        let evidence = super::utility::DecisionEvidence {
            battlefield: battlefield.assessment(),
            experience,
        };
        let profile = &*profile;
        let oriented_public_map: &PublicMapBriefing =
            oriented_public_map.get_or_insert_with(|| orientation.briefing(public_map));
        let oriented_home = oriented
            .my_buildings
            .iter()
            .filter(|building| {
                !building.provisional && building.kind == oxide_sim::stats::BuildingKind::Foundry
            })
            .min_by_key(|building| building.id)
            .map(|building| building.anchor)
            .unwrap_or(TilePos::new(0, 0));
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let super::allocation::AdmittedWork {
            intents: strategic,
            mut reservations,
            utility,
        } = super::allocation::admit_decision(
            super::allocation::DecisionContext {
                evidence,
                dials: &self.dials,
                profile,
                tuning,
                observation: &oriented,
                home: oriented_home,
                public_map: oriented_public_map,
                orientation,
                armies: &armies,
                enlisted: &enlisted,
            },
            AllocationParticipants {
                policy: &mut self.policy,
                strategy,
                lifts,
                team,
                raids,
            },
            intelligence,
            recorder.as_deref_mut(),
            observer,
        );
        let strategic_intents = strategic.len();
        let utility_context =
            utility.context(strategic, intelligence, oriented_public_map, evidence);
        let mut ground_unavailable = utility.reservations.clone();
        ground_unavailable.extend(self.exec.muster_exclusions());
        ground_unavailable.sort_unstable();
        ground_unavailable.dedup();
        let missions: Vec<_> = self
            .exec
            .missions
            .iter()
            .map(|(id, mission)| {
                let mut mission = mission.clone();
                orientation.mission(&mut mission);
                (*id, mission)
            })
            .collect();
        let ground_inputs = super::utility::GroundMissionInputs {
            missions: &missions,
            unavailable: &ground_unavailable,
            enlisted: &enlisted,
            tuning,
            relief: { team.operation() }
                .filter(|operation| operation.phase != super::team::TeamReliefPhase::Withdrawing)
                .map(|operation| (operation.foundry, operation.members.as_slice())),
        };
        let mut intents = self.policy.think_with_intelligence(
            &self.dials,
            &oriented,
            &armies,
            &enlisted,
            utility_context.with_ground_missions(ground_inputs),
        );
        reservations.extend_from_slice(self.policy.worker_safety_reservations());
        reservations.extend(self.policy.state.reconnaissance.reservations());
        reservations.extend(self.policy.support_reservations());
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
        let builder_lease = self.policy.foundry_builder_lease(&oriented).map(|lease| {
            BuilderLease::new(
                lease.builder(),
                lease.kind(),
                orientation.anchor(lease.anchor(), lease.kind().base_stats().size),
            )
        });
        if let Some(recorder) = recorder.as_deref_mut() {
            recorder.trace_mut().support =
                super::trace::SupportTrace::from_policy(&self.policy, oriented.tick);
            recorder.trace_mut().reconnaissance =
                super::trace::ReconnaissanceTrace::from_policy(&self.policy);
            recorder.trace_mut().utility = UtilityTrace {
                input_intents: bounded_count(strategic_intents),
                output_intents: bounded_count(intents.len()),
                reserved_units: bounded_count(reservations.len()),
            };
        }
        let intents = orientation.emit(intents);
        drop(strategy_scope);
        let _executive_scope = PhaseScope::new(observer, BotPhase::Executive);
        let lowered = self.exec.apply_with_builder_lease(
            self.player,
            obs,
            &intents,
            &reservations,
            builder_lease,
        );

        for journal in self.exec.ground_outcomes.values_mut() {
            journal.link_handoff(&lifts.outcomes);
        }

        self.policy
            .record_dispatched_work(&oriented, orientation, &lowered);
        if let Some(recorder) = recorder {
            recorder.trace_mut().mission_decisions = self.exec.mission_decisions.clone();
            recorder.trace_mut().lowering = LoweringTrace {
                maintenance_commands: bounded_count(maintenance_commands),
                decision_commands: bounded_count(lowered.len()),
                total_commands: bounded_count(commands.len().saturating_add(lowered.len())),
            };
        }
        commands.extend(lowered);
        if let Some(observer) = observer {
            observer.planning_work(self.policy.planning.stats());
        }
        commands
    }
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

#[cfg(test)]
fn project_strategic_queues(obs: &Observation, decision: &StrategicDecision) -> Observation {
    project_producer_queues(obs, &decision.intents)
}

#[cfg(test)]
fn project_producer_queues(obs: &Observation, intents: &[Intent]) -> Observation {
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

#[cfg(test)]
mod tests {
    use super::super::lift::LiftPhase;
    use super::super::observation::{BuildingObs, UnitObs};
    use super::*;
    use crate::SeatBot as Brain;
    use crate::Specialty;
    use chassis::grid::TilePos;
    use oxide_sim::State;
    use oxide_sim::ids::{BuildingId, PlayerId, Target};
    use oxide_sim::scenario::{
        BotDifficulty, BotStance, BuildingSpec, PlayerSpec, Scenario, UnitSpec,
    };
    use oxide_sim::state::Faction;
    use oxide_sim::stats::{BuildingKind, Role, UnitKind};

    fn public_map(scenario: &Scenario) -> Arc<PublicMapBriefing> {
        Arc::new(
            PublicMapBriefing::from_scenario(scenario)
                .expect("the focused scenario has a public map briefing"),
        )
    }

    fn scripted_brain(scenario: &Scenario, player: PlayerId, config: BotConfig) -> Brain {
        Brain::scripted(player, config, public_map(scenario))
    }

    #[test]
    fn inactive_seat_or_only_provisional_foundry_does_not_advance_planners() {
        let scenario = opening_core_team_relief_scenario();
        for difficulty in BotDifficulty::ALL {
            for (surrender, provisional) in [(false, false), (true, false), (false, true)] {
                let mut state = scenario.build().unwrap();
                let config = BotConfig::scripted(difficulty, BotStance::Balanced, 9000);
                let mut brain = scripted_brain(&scenario, PlayerId(0), config);
                let initial = brain.act_traced(&state);
                assert!(initial.trace.is_some());
                state.tick(&initial.commands);
                if surrender {
                    state.tick(&[PlayerCommand {
                        player: PlayerId(0),
                        command: Command::Surrender,
                    }]);
                }
                while !state.current_tick().is_multiple_of(brain.dials.cadence) {
                    state.tick(&[]);
                }
                let before = brain.clone();
                let traced = if surrender {
                    assert!(state.player(PlayerId(0)).resigned);
                    assert!(brain.act(&state).is_empty());
                    brain.act_traced(&state)
                } else {
                    let mut obs = Observation::fog_honest(&state, PlayerId(0));
                    if provisional {
                        for building in &mut obs.my_buildings {
                            if building.kind == BuildingKind::Foundry {
                                building.built = false;
                                building.provisional = true;
                            }
                        }
                    } else {
                        obs.my_buildings
                            .retain(|building| building.kind != BuildingKind::Foundry);
                    }
                    assert!(crate::Brain::act(&mut brain, &obs).is_empty());
                    crate::Brain::act_traced(&mut brain, &obs)
                };
                assert!(traced.commands.is_empty());
                assert!(traced.trace.is_none());
                assert_brain_unchanged(&before, &brain);
                let mut ally = scripted_brain(&scenario, PlayerId(1), config);
                assert!(ally.act_traced(&state).trace.is_some());
            }
        }
    }

    #[test]
    fn home_orientation_uses_a_physical_foundry_instead_of_an_older_plan() {
        let scenario = opening_core_team_relief_scenario();
        let state = scenario.build().unwrap();
        let mut obs = Observation::fog_honest(&state, PlayerId(0));
        let planned = obs
            .my_buildings
            .iter_mut()
            .find(|b| b.kind == BuildingKind::Foundry)
            .unwrap();
        planned.anchor = TilePos::new(3, 1);
        planned.built = false;
        planned.provisional = true;
        let home = TilePos::new(30, 20);
        obs.my_buildings.push(crate::test_support::building(
            99,
            PlayerId(0),
            BuildingKind::Foundry,
            home,
        ));
        obs.my_queues.push(Vec::new());
        obs.my_queue_progress.push(0);
        let mut brain = scripted_brain(&scenario, PlayerId(0), BotConfig::default());
        assert!(crate::Brain::act_traced(&mut brain, &obs).trace.is_some());
        assert_eq!(brain.orientation, Some(Orientation::for_home(&obs, home)));
    }

    fn operation_identity_brain(player: PlayerId, scenario: &Scenario) -> Brain {
        let difficulty = BotDifficulty::Prime;
        let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
        let brain = scripted_brain(scenario, player, config);
        let profile = brain.profile();
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

    #[test]
    fn rollback_keeps_restored_planner_ownership_out_of_residual_work() {
        let restored = [UnitId(9), UnitId(4), UnitId(9), UnitId(12)];
        let mut observation = test_island_observation();
        observation.my_units.extend([
            test_unit(4, UnitKind::Sentinel, TEST_HOME),
            test_unit(9, UnitKind::Sentinel, TEST_HOME),
        ]);
        observation.my_units.sort_unstable_by_key(|unit| unit.id);

        assert_eq!(
            retained_reservations(Vec::new(), &restored, &observation),
            [UnitId(4), UnitId(9)],
            "an empty rolled-back decision preserves live ownership without importing absent cargo or losses"
        );
    }

    fn enlist_opening_core(brain: &mut Brain, state: &State) {
        let obs = Observation::fog_honest(state, PlayerId(0));
        let core: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Sentinel)
            .collect();
        assert_eq!(core.len(), 8);
        let staging = TilePos::new(
            core.iter().map(|unit| unit.tile.x).sum::<i32>() / 8,
            core.iter().map(|unit| unit.tile.y).sum::<i32>() / 8,
        );
        let _ = brain.exec.apply_with_reservations(
            PlayerId(0),
            &obs,
            &[Intent::FormArmy { staging, size: 8 }],
            &[],
        );
        let enlisted: Vec<_> = brain.exec.enlisted().collect();
        assert_eq!(enlisted.len(), 8);
        assert!(enlisted.iter().all(|id| {
            obs.my_units
                .iter()
                .any(|unit| unit.id == *id && unit.kind == UnitKind::Sentinel)
        }));
    }

    fn assert_brain_unchanged(before: &Brain, after: &Brain) {
        assert_eq!(after.player, before.player);
        assert_eq!(after.dials, before.dials);
        assert_eq!(after.mind, before.mind);
        assert_eq!(after.policy, before.policy);
        assert_eq!(after.exec, before.exec);
        assert_eq!(after.orientation, before.orientation);
    }

    #[test]
    fn the_brain_receives_the_public_map_briefing() {
        let scenario = Scenario::skirmish();
        let public_map = public_map(&scenario);
        let scripted = Brain::scripted(PlayerId(0), BotConfig::default(), Arc::clone(&public_map));

        assert!(Arc::ptr_eq(&scripted.mind().public_map, &public_map));
        assert!(scripted.mind().oriented_public_map.is_none());
    }

    #[test]
    fn traced_and_untraced_player_facing_acts_are_behaviorally_identical() {
        let scenario = Scenario::skirmish();
        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 9_113);
        let mut direct_state = scenario.build().expect("the skirmish builds");
        let mut first_traced_state = direct_state.clone();
        let mut second_traced_state = direct_state.clone();
        let mut direct = scripted_brain(&scenario, PlayerId(0), config);
        let mut first_traced = scripted_brain(&scenario, PlayerId(0), config);
        let mut second_traced = scripted_brain(&scenario, PlayerId(0), config);
        let mut traces = 0;

        for _ in 0..600 {
            let direct_commands = direct.act(&direct_state);
            let first = first_traced.act_traced(&first_traced_state);
            let second = second_traced.act_traced(&second_traced_state);

            assert_eq!(first.commands, direct_commands);
            assert_eq!(second.commands, direct_commands);
            assert_eq!(first.trace, second.trace);
            if let Some(trace) = &first.trace {
                traces += 1;
                assert_eq!(trace.tick, direct_state.current_tick());
                assert_eq!(trace.player, PlayerId(0));
                assert_eq!(
                    trace.lowering.total_commands as usize,
                    direct_commands.len()
                );
                assert_eq!(
                    serde_json::to_string(trace).expect("the trace serializes"),
                    serde_json::to_string(second.trace.as_ref().expect("the second trace exists"))
                        .expect("the second trace serializes")
                );
                for effects in [
                    &trace.channels.team_relief.effects,
                    &trace.channels.connected_air.effects,
                    &trace.channels.lift.effects,
                    &trace.channels.raid.effects,
                ] {
                    assert!(effects.unit_claims.windows(2).all(|pair| pair[0] < pair[1]));
                    assert!(effects.unit_claims.len() <= direct_state.units().len());
                }
            }

            direct_state.tick(&direct_commands);
            first_traced_state.tick(&first.commands);
            second_traced_state.tick(&second.commands);
            assert_eq!(first_traced_state.hash(), direct_state.hash());
            assert_eq!(second_traced_state.hash(), direct_state.hash());
        }

        assert_eq!(
            traces,
            600 / direct.dials.cadence,
            "the trace is bounded to actual decision ticks"
        );
        assert_brain_unchanged(&direct, &first_traced);
        assert_brain_unchanged(&direct, &second_traced);
    }

    #[test]
    fn connected_package_trace_is_deterministic_and_behaviorally_observational() {
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let mut scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
        let scout = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
            .expect("the trace scenario has one connected-air scout");
        (scout.x, scout.y) = (42, 19);
        scenario.units.extend([
            UnitSpec {
                player: 0,
                kind: UnitKind::Bombard,
                x: 9,
                y: 17,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 10,
                y: 18,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 11,
                y: 18,
            },
        ]);
        let mut direct_state = scenario
            .build()
            .expect("the connected-operation trace scenario builds");
        let mut traced_state = direct_state.clone();
        let mut direct = foundry_competition_brain(&scenario);
        let mut traced = direct.clone();

        let direct_commands = direct.act(&direct_state);
        let traced_act = traced.act_traced(&traced_state);

        assert_eq!(traced_act.commands, direct_commands);
        let trace = traced_act
            .trace
            .expect("the connected-operation admission is traced");
        assert_eq!(
            trace.connected_force.status,
            super::super::trace::ConnectedForceStatus::Active
        );
        let target = trace
            .connected_force
            .target
            .expect("the connected package records its target");
        assert_eq!(target.kind, BuildingKind::Foundry);
        assert_eq!(
            target.evidence,
            super::super::trace::TargetEvidenceTrace::Current
        );
        let package = trace
            .connected_force
            .package
            .expect("the admitted connected package is recorded");
        assert_eq!(package.admitted_at, direct_state.current_tick());
        assert_eq!(package.derived_at, direct_state.current_tick());
        assert!(package.preparation_deadline > package.derived_at);
        assert!(package.target_anchors.contains(&target.anchor));
        assert!(
            package
                .target_anchors
                .windows(2)
                .all(|pair| (pair[0].y, pair[0].x) < (pair[1].y, pair[1].x))
        );
        assert!(!package.demands.recon.is_empty());
        assert!(!package.demands.suppression.is_empty());
        assert!(!package.demands.strike.is_empty());
        assert!(package.chosen_capability.recon >= package.minimum_capability.recon);
        assert!(package.chosen_capability.suppression >= package.minimum_capability.suppression);
        assert!(package.chosen_capability.strike >= package.minimum_capability.strike);
        assert_eq!(package.observed_aa_firepower, 0);
        assert_eq!(package.suppressible_aa_firepower, 0);
        assert!(trace.connected_force.assigned.scout.is_some());
        assert!(!trace.connected_force.assigned.membership_frozen);

        direct_state.tick(&direct_commands);
        traced_state.tick(&traced_act.commands);
        assert_eq!(traced_state.hash(), direct_state.hash());
        assert_brain_unchanged(&direct, &traced);
    }

    #[test]
    fn terminal_connected_trace_uses_target_evidence_from_the_termination_tick() {
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let mut scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
        let scout = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
            .expect("the trace scenario has one connected-air scout");
        (scout.x, scout.y) = (42, 19);
        for (index, kind) in [
            UnitKind::Bombard,
            UnitKind::Avalanche,
            Role::AirGround.unit_for(scenario.players[0].faction),
            Role::Bomber.unit_for(scenario.players[0].faction),
        ]
        .into_iter()
        .cycle()
        .take(24)
        .enumerate()
        {
            scenario.units.push(UnitSpec {
                player: 0,
                kind,
                x: 7 + i32::try_from(index % 8).expect("small fixture index"),
                y: 16 + i32::try_from(index / 8).expect("small fixture index"),
            });
        }
        let mut state = scenario
            .build()
            .expect("the terminal-trace scenario builds");
        let mut brain = foundry_competition_brain(&scenario);

        let mut frozen = None;
        for _ in 0..1_000 {
            let decision = brain.act_traced(&state);
            if let Some(trace) = &decision.trace
                && trace.connected_force.assigned.membership_frozen
            {
                let target = trace
                    .connected_force
                    .target
                    .expect("the frozen connected package retains its target");
                assert_eq!(
                    target.evidence,
                    super::super::trace::TargetEvidenceTrace::Current
                );
                let package = trace
                    .connected_force
                    .package
                    .as_ref()
                    .expect("the frozen connected force retains its package");
                let assigned = &trace.connected_force.assigned;
                let mut members: Vec<_> = assigned
                    .scout
                    .into_iter()
                    .chain(assigned.suppression.iter().copied())
                    .chain(assigned.strike.iter().copied())
                    .collect();
                members.sort_unstable();
                members.dedup();
                assert!(!members.is_empty());
                frozen = Some((target, package.target_anchors.clone(), members));
            }
            state.tick(&decision.commands);
            if frozen.is_some() {
                break;
            }
        }
        let (target, target_anchors, members) =
            frozen.expect("the connected package reaches exact-id freeze");

        let mut lost_force = Observation::fog_honest(&state, PlayerId(0));
        lost_force
            .my_units
            .retain(|unit| members.binary_search(&unit.id).is_err());
        lost_force.tick = lost_force.tick.next_multiple_of(brain.dials.cadence);
        for building in &mut lost_force.enemy_buildings {
            if building.player == target.player && building.anchor == target.anchor {
                building.seen = false;
            }
        }
        let (width, height) = target.kind.base_stats().size;
        for y in target.anchor.y..target.anchor.y + height {
            for x in target.anchor.x..target.anchor.x + width {
                let index = (y * lost_force.map_width + x) as usize;
                lost_force.visible[index] = false;
            }
        }
        let terminal = super::Brain::act_traced(&mut brain, &lost_force);
        let trace = terminal.trace.expect("the terminal decision is traced");
        assert_eq!(
            trace.connected_force.status,
            super::super::trace::ConnectedForceStatus::Aborted
        );
        let terminal_target = trace
            .connected_force
            .target
            .expect("the terminal trace preserves the package target");
        assert_eq!(
            (
                terminal_target.player,
                terminal_target.kind,
                terminal_target.anchor
            ),
            (target.player, target.kind, target.anchor)
        );
        assert_eq!(
            terminal_target.evidence,
            super::super::trace::TargetEvidenceTrace::Remembered,
            "the lost scout makes the still-live objective a ghost on the termination tick"
        );
        assert_eq!(
            trace
                .connected_force
                .package
                .expect("the terminal trace preserves the frozen package")
                .target_anchors,
            target_anchors
        );
    }

    #[test]
    fn traced_act_marks_recovery_and_omits_non_decisions() {
        let mut scenario = Scenario::skirmish();
        scenario
            .units
            .retain(|unit| unit.player != 0 || unit.kind.stats().harvest.is_none());
        let mut state = scenario.build().expect("the stranded skirmish builds");
        let mut scripted = scripted_brain(&scenario, PlayerId(0), BotConfig::default());

        let recovery = scripted.act_traced(&state);
        let trace = recovery
            .trace
            .expect("a player-facing recovery think is traced");
        assert_eq!(trace.control_flow, DecisionControlFlow::HarvesterRecovery);
        assert_eq!(
            trace.lowering.total_commands as usize,
            recovery.commands.len()
        );
        assert!(trace.budget.is_none());
        assert_eq!(
            trace.channels,
            super::super::trace::ChannelTraces::default()
        );

        state.tick(&recovery.commands);
        assert!(
            scripted.act_traced(&state).trace.is_none(),
            "a cadence skip is not a decision record"
        );
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
                .all(|event| !matches!(event, oxide_sim::event::Event::CommandRejected { .. })),
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
        while state.current_tick() < 100 && state.building(site).unwrap().progress == 0 {
            state.tick(&[]);
        }
        assert!(state.building(site).unwrap().progress > 0);
        while !state.current_tick().is_multiple_of(brain.dials.cadence) {
            state.tick(&[]);
        }
        let active_builder = state
            .units()
            .iter()
            .find(|unit| matches!(unit.order, oxide_sim::state::Order::Build { site: target } if target == site))
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

        while !state.current_tick().is_multiple_of(brain.dials.cadence) {
            state.tick(&[]);
        }
        assert!(matches!(
            state
                .unit(active_builder)
                .expect("the active builder survived")
                .order,
            oxide_sim::state::Order::Idle | oxide_sim::state::Order::Move { .. }
        ));
        assert!(
            state.building(site).is_some_and(|building| !building.built),
            "the interruption must leave a paid unfinished site"
        );

        let deadline = state.current_tick() + brain.dials.cadence * 4;
        let mut resolution = brain.act(&state);
        while resolution.is_empty() && state.current_tick() < deadline {
            state.tick(&[]);
            resolution = brain.act(&state);
        }
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
                .all(|event| !matches!(event, oxide_sim::event::Event::CommandRejected { .. })),
            "the ordinary cancellation must be accepted by State: {resolved:?}"
        );
        assert!(state.building(site).is_none());
        assert!(
            resolved.events.iter().any(|event| matches!(
                event,
                oxide_sim::event::Event::BuildCancelled {
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
        crate::test_support::edit_units(&mut state, |units| {
            for unit in units {
                unit.hp = wounded_hp;
            }
        });
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
            brain.mind().intelligence.observed_at(),
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
                        brain.mind().intelligence.observed_at(),
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
                .all(|event| !matches!(event, oxide_sim::Event::CommandRejected { .. }))
        );
        for defense in defenses {
            assert_eq!(
                applied.building(defense).expect("defense stands").focus,
                applied.attack_objective(PlayerId(0), target)
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
        crate::test_support::edit_units(&mut counterfactual, |units| {
            units.iter_mut().find(|unit| unit.id == hidden).unwrap().hp = 1
        });
        assert_eq!(
            Observation::fog_honest(&state, PlayerId(0)),
            Observation::fog_honest(&counterfactual, PlayerId(0)),
            "the fixture mutation must remain outside the bot's knowledge"
        );

        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_042);
        let mut baseline = scripted_brain(&scenario, PlayerId(0), config);
        let mut changed = scripted_brain(&scenario, PlayerId(0), config);
        let baseline_act = baseline.act_traced(&state);
        let changed_act = changed.act_traced(&counterfactual);
        assert_eq!(baseline_act.commands, changed_act.commands);
        assert_eq!(baseline_act.trace, changed_act.trace);
        assert_eq!(
            serde_json::to_string(&baseline_act.trace).expect("the baseline trace serializes"),
            serde_json::to_string(&changed_act.trace).expect("the counterfactual trace serializes"),
            "a decision trace must not expose authoritative facts absent from the fog-honest observation"
        );
        assert_brain_unchanged(&baseline, &changed);
    }

    #[test]
    fn each_difficulty_reaches_its_opening_core_before_the_first_fabricator() {
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
        let floors = [5_u64, 6_u64];
        let mut first_fabricator = [None; 2];
        let mut core_at_fabricator = [None; 2];

        while state.current_tick() < OPENING_END && first_fabricator.iter().any(Option::is_none) {
            let tick = state.current_tick();
            let statuses = [0, 1].map(|seat| {
                combat_core_status(
                    &Observation::fog_honest(&state, PlayerId(seat as u8)),
                    &[],
                    &[],
                    floors[seat],
                )
            });
            let commands: Vec<_> = brains
                .iter_mut()
                .flat_map(|brain| brain.act(&state))
                .collect();
            for command in &commands {
                let seat = usize::from(command.player.0);
                if matches!(
                    command.command,
                    Command::Build {
                        kind: BuildingKind::Fabricator,
                        ..
                    }
                ) && first_fabricator[seat].is_none()
                {
                    first_fabricator[seat] = Some(tick);
                    core_at_fabricator[seat] = Some(statuses[seat].projected_strength);
                    assert!(
                        statuses[seat].ready,
                        "seat {seat} started its first Fabricator with core status {:?}",
                        statuses[seat]
                    );
                    assert!(
                        statuses[seat].projected_strength >= statuses[seat].target_strength,
                        "post-floor reinforcement may extend the line while capital accumulates"
                    );
                }
            }

            let report = state.tick(&commands);
            for event in report.events {
                if let oxide_sim::Event::CommandRejected { player, reason } = event {
                    let placements: Vec<_> = commands
                        .iter()
                        .filter_map(|command| match &command.command {
                            Command::Build {
                                kind,
                                anchor,
                                units,
                                ..
                            } => Some((
                                *kind,
                                *anchor,
                                state.place_intent_refusal_replacing(
                                    command.player,
                                    *kind,
                                    *anchor,
                                    units,
                                ),
                            )),
                            _ => None,
                        })
                        .collect();
                    panic!(
                        "seat {player} issued a rejected opening command: {reason:?}; tick={}; commands={commands:?}; placements={placements:?}",
                        state.current_tick()
                    );
                }
            }
        }

        assert!(
            first_fabricator.iter().all(Option::is_some),
            "every rung should start a Fabricator within the opening window: first={first_fabricator:?}, scrap={:?}, buildings={:?}, queues={:?}",
            [0, 1].map(|seat| state.player(PlayerId(seat)).scrap),
            state
                .buildings()
                .iter()
                .map(|building| (building.player, building.kind, building.built))
                .collect::<Vec<_>>(),
            state
                .buildings()
                .iter()
                .map(|building| (building.id, building.queue.clone()))
                .collect::<Vec<_>>(),
        );
        assert!(core_at_fabricator.iter().all(Option::is_some));
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
            reserved_scrap: 0,
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
        use super::super::strategy::AirOperation;
        use super::super::team::{TeamReliefOperation, TeamReliefPhase};

        let air = AirOperation {
            target_player: PlayerId(1),
            target_kind: BuildingKind::Foundry,
            target: TilePos::new(20, 8),
            target_id: Some(BuildingId(3)),
            stage: crate::strategy::AirStage::Assemble,
            started_at: 100,
            phase_started_at: 120,
            scout: Some(UnitId(8)),
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery: vec![UnitId(7), UnitId(9)],
            strike_aircraft: vec![UnitId(10), UnitId(11)],
            strike_issued_at: None,
            membership_frozen_at: None,
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
        ghost_recon.stage = crate::strategy::AirStage::Watching;
        assert_eq!(
            air_support(Some(&ghost_recon), None),
            LiftAirSupport::Independent
        );
        let mut released = air.clone();
        released.stage = crate::strategy::AirStage::Strike;
        assert_eq!(
            air_support(Some(&released), None),
            LiftAirSupport::Released {
                player: PlayerId(1),
                target: TilePos::new(20, 8),
            }
        );
        released.stage = crate::strategy::AirStage::Recover {
            reason: crate::strategy::AirRecoveryReason::Complete,
            assault_admitted: true,
        };
        assert!(matches!(
            air_support(Some(&released), None),
            LiftAirSupport::Released { .. }
        ));
        released.stage = crate::strategy::AirStage::Recover {
            reason: crate::strategy::AirRecoveryReason::NewAirDefense,
            assault_admitted: true,
        };
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
    fn a_pending_team_watch_does_not_own_units_before_deployment_acceptance() {
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
        let mut profile = crate::profile::ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            0x0A16_7EA0,
        ));
        profile.traits.support = 70;
        profile.traits.fortification = 65;
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let mut relief = TeamReliefPlanner::new();

        let pending = relief.think_unrestricted(&profile, tuning, &obs, TEST_HOME, &[], &[]);
        assert!(relief.operation().is_none());
        assert!(!pending.reservations.is_empty());
        assert_eq!(
            prior_planner_claims(&[], None, &relief.core_reservations(), &[], None),
            [],
            "an observed watch cannot claim a proposed group before allocation accepts it"
        );
        assert_eq!(
            relief.core_reservations(),
            [],
            "an unaccepted proposal leaves the opening core available"
        );
        obs.tick += tuning.reaction_delay + oxide_sim::TICKS_PER_SECOND as u64;
        let accepted = relief.think_unrestricted(&profile, tuning, &obs, TEST_HOME, &[], &[]);
        assert_eq!(relief.core_reservations(), accepted.reservations);
        assert_eq!(accepted.reservations.len(), 2);
        assert!(!accepted.reservations.contains(&UnitId(1)));
        assert!(!accepted.reservations.contains(&UnitId(2)));
    }

    #[test]
    fn lift_ownership_keeps_landed_assault_riders_in_the_prior_planner_ledger() {
        use super::super::lift::{LiftManifest, LiftOperation, LiftPhase, UnitIdSet};

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
            payload: UnitIdSet::from_ids(vec![UnitId(2), UnitId(3), UnitId(4), UnitId(5)]),
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
                    closed: true,
                },
            ],
            launched: true,
            producer_assignments: Vec::new(),
            issued_producers: Vec::new(),
        };

        assert_eq!(
            prior_planner_claims(&[UnitId(1)], None, &[], &[], Some(&lift)),
            [
                UnitId(1),
                UnitId(2),
                UnitId(3),
                UnitId(4),
                UnitId(5),
                UnitId(20),
            ],
            "landed riders remain operation-owned after their carrier closes"
        );

        let mut provisioning = lift;
        provisioning.phase = LiftPhase::Provision;
        provisioning.manifests.clear();
        provisioning.payload = UnitIdSet::from_ids(vec![UnitId(6), UnitId(7), UnitId(8)]);
        assert_eq!(
            prior_planner_claims(&[], None, &[], &[], Some(&provisioning)),
            [UnitId(6), UnitId(7), UnitId(8)],
            "the exact payload stays owned while its carriers are still training"
        );
    }

    #[test]
    fn landed_lift_assault_cannot_reopen_prime_capital_spending() {
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.players[0].scrap = 50_000;
        let mut kept_sentinels = 0;
        scenario.units.retain(|unit| {
            if unit.kind != UnitKind::Sentinel {
                return true;
            }
            kept_sentinels += 1;
            kept_sentinels <= 16
        });
        scenario.units.extend((0..8).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Skyhook,
            x: 3 + index,
            y: 20,
        }));
        let state = scenario
            .build()
            .expect("the landed-assault opening fixture builds");
        let mut planning = Observation::fog_honest(&state, PlayerId(0));
        let home = planning
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Foundry)
            .expect("the authored home Foundry stands")
            .anchor;
        let original_riders = planning.my_units.clone();
        let mut lifts = LiftPlanner::new();
        let _ = lifts.think_with_admission_and_producer_lanes(
            &planning,
            home,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: true,
                spendable_scrap: planning.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 8,
            },
            crate::resources::ProducerLaneReservations::empty(),
        );
        let manifests = lifts
            .operation()
            .expect("the disconnected objective admits a lift")
            .manifests
            .clone();
        let riders: Vec<_> = manifests
            .iter()
            .flat_map(|manifest| manifest.riders.iter().copied())
            .collect();
        assert_eq!(riders.len(), 8, "the lift leaves Prime's home eight intact");

        planning.my_units.retain(|unit| !riders.contains(&unit.id));
        for manifest in &manifests {
            let carrier = planning
                .my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .expect("every exact carrier remains observable");
            carrier.tile = manifest.pickup;
            carrier.cargo = manifest
                .riders
                .iter()
                .filter_map(|id| original_riders.iter().find(|unit| unit.id == *id))
                .map(|unit| unit.kind.stats().transport_size)
                .sum();
        }
        planning.tick += 1;
        let _ = lifts.think_with_admission_and_producer_lanes(
            &planning,
            home,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: false,
                spendable_scrap: planning.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 8,
            },
            crate::resources::ProducerLaneReservations::empty(),
        );
        assert_eq!(
            lifts.operation().map(|operation| operation.phase),
            Some(LiftPhase::Landing)
        );

        let first_manifest = manifests
            .first()
            .expect("the multi-carrier fixture assigns a first manifest");
        {
            let carrier = planning
                .my_units
                .iter_mut()
                .find(|unit| unit.id == first_manifest.carrier)
                .expect("the first carrier reaches its exact drop");
            carrier.tile = first_manifest.drop;
            carrier.cargo = 0;
            planning
                .my_units
                .extend(first_manifest.riders.iter().map(|id| {
                    let mut rider = original_riders
                        .iter()
                        .find(|unit| unit.id == *id)
                        .expect("the manifest names an exact original rider")
                        .clone();
                    rider.tile = first_manifest.drop;
                    rider
                }));
        }
        planning.my_units.sort_unstable_by_key(|unit| unit.id);
        planning.tick += 1;
        let _ = lifts.think_with_admission_and_producer_lanes(
            &planning,
            home,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: false,
                spendable_scrap: planning.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 8,
            },
            crate::resources::ProducerLaneReservations::empty(),
        );
        let mixed_operation = lifts
            .operation()
            .expect("the remaining loaded carrier keeps the lift landing");
        assert_eq!(mixed_operation.phase, LiftPhase::Landing);
        assert!(mixed_operation.manifests[0].attack_issued);
        assert!(
            mixed_operation
                .manifests
                .iter()
                .skip(1)
                .any(|manifest| !manifest.attack_issued)
        );

        planning.tick += 1;
        let mixed_decision = lifts.think_with_admission_and_producer_lanes(
            &planning,
            home,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: false,
                spendable_scrap: planning.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 8,
            },
            crate::resources::ProducerLaneReservations::empty(),
        );
        assert!(
            first_manifest
                .riders
                .iter()
                .all(|id| mixed_decision.reservations.contains(id)),
            "landed riders remain reserved while another manifest is still landing: {mixed_decision:?}"
        );

        for manifest in manifests.iter().skip(1) {
            let carrier = planning
                .my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .expect("every remaining carrier reaches its exact drop");
            carrier.tile = manifest.drop;
            carrier.cargo = 0;
            planning.my_units.extend(manifest.riders.iter().map(|id| {
                let mut rider = original_riders
                    .iter()
                    .find(|unit| unit.id == *id)
                    .expect("the manifest names an exact original rider")
                    .clone();
                rider.tile = manifest.drop;
                rider
            }));
        }
        planning.my_units.sort_unstable_by_key(|unit| unit.id);
        planning.tick += 1;
        let _ = lifts.think_with_admission_and_producer_lanes(
            &planning,
            home,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: false,
                spendable_scrap: planning.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 8,
            },
            crate::resources::ProducerLaneReservations::empty(),
        );
        let operation = lifts
            .operation()
            .expect("the planner keeps directing its landed assault");
        assert_eq!(operation.phase, LiftPhase::Recover);
        assert!(
            operation
                .manifests
                .iter()
                .all(|manifest| manifest.attack_issued)
        );

        let one_home_sentinel = state
            .units()
            .iter()
            .find(|unit| {
                unit.player == PlayerId(0)
                    && unit.kind == UnitKind::Sentinel
                    && !riders.contains(&unit.id)
            })
            .expect("the original core has one removable home member")
            .id;
        let assault_positions: Vec<_> = operation
            .manifests
            .iter()
            .flat_map(|manifest| {
                manifest
                    .riders
                    .iter()
                    .copied()
                    .map(move |id| (id, manifest.drop))
            })
            .collect();
        let mut observed = Observation::fog_honest(&state, PlayerId(0));
        observed
            .my_units
            .retain(|unit| unit.id != one_home_sentinel);
        for unit in &mut observed.my_units {
            if let Some((_, drop)) = assault_positions
                .iter()
                .find(|(rider, _)| *rider == unit.id)
            {
                unit.tile = *drop;
            }
        }
        assert!(combat_core_status(&observed, &[], &[], 8).ready);
        let exact_lift_claims = prior_planner_claims(&[], None, &[], &[], Some(operation));
        let protected = combat_core_status(&observed, &exact_lift_claims, &[], 8);
        assert!(
            !protected.ready,
            "the seven home hulls, not the eight island riders, define the opening core: {protected:?}"
        );

        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_024);
        let mut control = scripted_brain(&scenario, PlayerId(0), config);

        let control_commands = super::Brain::act(&mut control, &observed);
        assert!(
            control_commands.iter().any(|command| matches!(
                command.command,
                Command::Build { .. } | Command::UpgradeBuilding { .. }
            )),
            "the rich control must prove voluntary capital is otherwise available: {control_commands:?}"
        );

        let mut protected_brain = scripted_brain(&scenario, PlayerId(0), config);

        protected_brain.mind_mut().lifts = lifts;

        let commands = super::Brain::act(&mut protected_brain, &observed);
        assert!(commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Sentinel,
                ..
            }
        )));
        assert!(
            commands.iter().all(|command| match command.command {
                Command::Build { .. } | Command::UpgradeBuilding { .. } => false,
                Command::Train { kind, .. } => kind == UnitKind::Sentinel,
                _ => true,
            }),
            "operation-owned island riders cannot reopen voluntary spending: {commands:?}"
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
    fn active_bulk_lift_spends_its_funded_bank_without_a_factory_tax() {
        let airworks_capacity = BuildingKind::Airworks
            .base_stats()
            .construction
            .expect("Airworks have a construction price")
            .cost
            .saturating_add(UnitKind::Sentinel.stats().cost);
        let carrier_cost = UnitKind::Skyhook.stats().cost;
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.players[0].scrap = airworks_capacity
            .saturating_add(UnitKind::Sentinel.stats().cost)
            .saturating_add(carrier_cost);
        let mut state = scenario
            .build()
            .expect("open-lane bulk-lift scenario builds");
        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 17);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);
        let obs = Observation::fog_honest(&state, PlayerId(0));
        let home = obs
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Foundry)
            .expect("the observation retains the home Foundry")
            .anchor;
        let lifts = &mut brain.mind_mut().lifts;
        let mut seed_obs = obs.clone();
        seed_obs.scrap = 0;
        let seeded = lifts.think_unrestricted(&seed_obs, home, &[], LiftAirSupport::Independent);
        assert!(lifts.operation().is_some());
        assert!(seeded.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )));

        let result = brain.act_traced(&state);
        let carrier_commands = result
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Train {
                        kind: UnitKind::Skyhook,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            carrier_commands, 2,
            "the retained lift may use both planned queue slots without an unfunded factory tax: {:?}",
            result.trace
        );
        assert!(
            result.commands.iter().all(|command| !matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Airworks,
                    ..
                }
            )),
            "factory investment must not seize the older operation's funded bank: commands={:?}, trace={:?}",
            result.commands,
            result.trace
        );
        let report = state.tick(&result.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
    }

    #[test]
    fn active_bulk_lift_retains_its_funding_through_full_queues() {
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
                oxide_sim::event::Event::CommandRejected {
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

        let lifts = &mut brain.mind_mut().lifts;
        let seeded = lifts.think_unrestricted(&obs, home, &[], LiftAirSupport::Independent);
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

        let result = brain.act_traced(&state);
        let commands = result.commands;
        assert!(
            commands.iter().all(|command| !matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Airworks,
                    ..
                }
            )),
            "a full queue must not redirect the retained operation's capital to an unadmitted factory: {commands:?}; trace={:?}",
            result.trace
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
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "strategic reservations and residual utility spending must lower through one legal bank: {:?}",
            report.events
        );
    }

    #[test]
    fn connected_air_and_lift_share_capacity_without_a_residual_factory_tax() {
        let mut scenario = combined_operation_scenario();
        scenario
            .buildings
            .last_mut()
            .expect("the fixture has a reachable enemy structure")
            .kind = BuildingKind::Crucible;
        scenario.buildings.extend([
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Foundry,
                x: 10,
                y: 10,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Airworks,
                x: 16,
                y: 4,
            },
        ]);
        let mut state = scenario
            .build()
            .expect("combined-operation capacity scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);
        while !super::super::difficulty::strategic_admission_tick(state.current_tick()) {
            state.tick(&[]);
        }
        let raw = Observation::fog_honest(&state, PlayerId(0));
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the home Foundry remains visible")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);
        assert!(orientation.is_identity());
        let oriented = orientation.observe(&raw);
        let mut lift = LiftPlanner::new();
        let _ = lift.think_unrestricted(&oriented, home, &[], LiftAirSupport::Independent);
        let lift_airwork = lift.remaining_airwork_ticks(&oriented, &[]);
        assert!(lift.operation().is_some());
        assert!(lift_airwork > 2_400);

        let mut brain = operation_identity_brain(PlayerId(0), &scenario);
        brain.dials.minimum_core_equivalents = 0;
        let capacity_fund = BuildingKind::Airworks
            .base_stats()
            .construction
            .unwrap()
            .cost;
        enlist_opening_core(&mut brain, &state);
        brain.orientation = Some(orientation);
        brain.mind_mut().lifts = lift;

        let next_think = state.current_tick().saturating_add(brain.dials().cadence);
        while state.current_tick() < next_think {
            state.tick(&[]);
        }
        let spendable_after_capacity = UnitKind::Condor.stats().cost;
        crate::test_support::edit_player(&mut state, PlayerId(0), |item| {
            item.scrap = spendable_after_capacity + capacity_fund
        });

        let result = brain.act_traced(&state);
        let trace = result.trace.expect("the capacity decision is traced");
        let budget = trace.budget.expect("the ledger decision is traced");
        assert_eq!(budget.airworks_capacity, 0);
        assert!(
            trace.channels.connected_air.effects.committed_scrap <= budget.bank,
            "operation spending must still fit observed current capital"
        );
        let report = state.tick(&result.commands);
        assert!(
            report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "{:?}",
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
        enlist_opening_core(&mut brain, &state);

        let result = brain.act_traced(&state);
        let commands = result.commands;

        let air = (brain.mind().strategy)
            .air_operation()
            .expect("the wealthy disconnected match starts the bomber operation");
        let lift = (brain.mind().lifts)
            .operation()
            .expect("the same match starts its coordinated bulk lift");
        assert!(lift.desired_carriers >= 8);
        assert!(lift.payload.len() >= 32);
        assert_eq!(
            (lift.target_player, lift.target),
            (air.target_player, air.target),
            "the second-starting lift must inherit the air operation's exact objective"
        );
        assert_eq!(lift.planned_drops.len(), lift.desired_carriers);
        assert!(
            (brain.mind().raids).operation().is_none(),
            "simultaneous air and lift work must consume Prime's optional-operation attention too"
        );

        let report = state.tick(&commands);
        assert!(
            report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "all combined-operation commands must remain ordinary legal commands: {:?}; commands={commands:?}",
            report.events
        );
    }

    #[test]
    fn difficulty_attention_limits_competing_operations_without_dropping_active_raids() {
        let scenario = combined_operation_scenario();
        let mut prepared = scenario
            .build()
            .expect("combined-operation scenario builds");
        for _ in 0..6_000 {
            prepared.tick(&[]);
        }

        for difficulty in BotDifficulty::ALL {
            let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
            let mut brain = scripted_brain(&scenario, PlayerId(0), config);
            enlist_opening_core(&mut brain, &prepared);
            let result = brain.act_traced(&prepared);
            let trace = result
                .trace
                .expect("the competing-operation decision is traced");
            assert!(brain.mind().strategy.air_operation().is_some());
            assert!(brain.mind().lifts.operation().is_some());
            assert!(brain.mind().raids.operation().is_none());
            let attention = trace
                .gates
                .raid_attention
                .expect("admission evaluates optional attention");
            assert_eq!(attention.strategic_load, 2);
            assert!(
                !attention.admitted,
                "{difficulty:?} cannot add a third operation"
            );
        }

        for difficulty in BotDifficulty::ALL {
            let config = BotConfig::scripted(difficulty, BotStance::Balanced, 20_024);
            let mut brain = scripted_brain(&scenario, PlayerId(0), config);
            enlist_opening_core(&mut brain, &prepared);

            let profile = *brain.profile();
            let tuning = DifficultyTuning::for_level(difficulty);
            let obs = Observation::fog_honest(&prepared, PlayerId(0));
            let home = obs
                .my_buildings
                .iter()
                .find(|building| building.kind == BuildingKind::Foundry)
                .expect("the home Foundry stands")
                .anchor;
            let mut prior_raid = RaidPlanner::new();
            let raid_start = prior_raid.think_unrestricted(&profile, tuning, &obs, home, &[], &[]);
            assert!(matches!(
                raid_start.intents.as_slice(),
                [Intent::AttackMoveUnits { .. }]
            ));
            let prior_members = prior_raid
                .operation()
                .expect("zero load admits the raid on every rung")
                .members
                .clone();
            brain.mind_mut().raids = prior_raid;

            let _ = brain.act(&prepared);

            assert!(
                (brain.mind().strategy).air_operation().is_some(),
                "{difficulty:?} must start the air operation in the continuation fixture"
            );
            assert!(
                (brain.mind().lifts).operation().is_some(),
                "{difficulty:?} must start the lift operation in the continuation fixture"
            );
            assert_eq!(
                (brain.mind().raids)
                    .operation()
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
        crate::test_support::set_tick(&mut state, 6_000);

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
                hp: enemy_foundry.hp,
                built: enemy_foundry.built,
                tier: enemy_foundry.tier,
                ..crate::test_support::building(
                    enemy_foundry.id.0,
                    enemy_foundry.player,
                    enemy_foundry.kind,
                    enemy_foundry.anchor,
                )
            });
            let prior = orientation.observe(&prior);
            brain.mind_mut().intelligence.update(&prior);

            let oriented = orientation.observe(&raw);
            let oriented_home = orientation.anchor(home, BuildingKind::Foundry.base_stats().size);
            let mut expected_intelligence = brain.mind().intelligence.clone();
            expected_intelligence.update(&oriented);
            let target = expected_intelligence
                .buildings()
                .first()
                .expect("the synthetic prior sighting creates one contact");
            assert_eq!(
                brain.mind().lifts.prospective_first_carrier_commitment(
                    &oriented,
                    oriented_home,
                    &[],
                    &[],
                    0,
                    target,
                ),
                UnitKind::Skyhook.stats().cost,
                "the fog-honest snapshot warrants exactly one prospective carrier for {difficulty:?}"
            );

            let resources = super::super::resources::ResourceSnapshot::from_observation(&oriented);
            let public_map = orientation.briefing(&brain.mind().public_map);
            let builders = oriented.my_units.iter().collect::<Vec<_>>();
            let unguarded_defense = brain.policy.fresh_defense_proposals(
                &brain.mind().profile,
                &oriented,
                &resources,
                &public_map,
                orientation,
                oriented_home,
                expected_intelligence.units(),
                expected_intelligence.buildings(),
                &builders,
                &[],
                0,
                0,
                0,
                super::super::utility::DecisionEvidence {
                    battlefield: brain.mind().battlefield.assessment(),
                    experience: &brain.mind().experience,
                },
            );
            assert!(
                !unguarded_defense.is_empty(),
                "the fixture offers defense without the carrier hold for {difficulty:?}"
            );

            let act = brain.act_traced(&state);
            let first_trace = act
                .trace
                .as_ref()
                .expect("the prospective hold is traced on its admission think");
            let first_budget = first_trace
                .budget
                .as_ref()
                .expect("the prospective hold is traced on its admission think");
            assert_eq!(
                first_budget.prospective_carrier,
                UnitKind::Skyhook.stats().cost,
                "the first-carrier hold is owned exactly once for {difficulty:?}"
            );
            assert_eq!(
                first_budget.voluntary_scrap_guard,
                UnitKind::Sentinel.stats().cost,
                "the reachable ground objective keeps the overlapping shallow guard visible for {difficulty:?}"
            );
            assert_eq!(
                first_budget.utility_spendable, 0,
                "the exact carrier bank is protected once rather than splitting into additive carrier and shallow holds for {difficulty:?}"
            );
            assert!(
                first_trace
                    .allocation
                    .proposals
                    .entries
                    .iter()
                    .all(|proposal| !matches!(
                        proposal.key,
                        super::super::trace::ProposalKeyTrace::Defense { .. }
                    )),
                "unaffordable defense is rejected before expensive quotation for {difficulty:?}"
            );
            assert!(
                first_trace
                    .allocation
                    .proposals
                    .entries
                    .iter()
                    .all(|proposal| proposal.claims.minimum_residual_scrap
                        >= UnitKind::Skyhook.stats().cost),
                "every fresh voluntary proposal must preserve the prospective carrier floor for {difficulty:?}"
            );
            assert!(
                first_trace
                    .allocation
                    .proposals
                    .entries
                    .iter()
                    .filter(|proposal| matches!(
                        proposal.key,
                        super::super::trace::ProposalKeyTrace::Defense { .. }
                    ))
                    .all(|proposal| proposal.disposition
                        != super::super::trace::ProposalDispositionTrace::Accepted),
                "no voluntary defense may spend the prospective carrier floor for {difficulty:?}"
            );
            let commands = act.commands;
            let operation = (brain.mind().strategy)
                .air_operation()
                .expect("remembered disconnected Foundry starts reconnaissance");
            assert_eq!(
                operation.phase(),
                AirOperationPhase::Recon,
                "{difficulty:?}"
            );
            assert!(!operation.assault_admitted(), "{difficulty:?}");
            assert!(
                (brain.mind().lifts).operation().is_none(),
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

            let mut continued_state = state.clone();
            continued_state.tick(&commands);
            for _ in 1..brain.dials.cadence {
                continued_state.tick(&[]);
            }
            let continued = brain.act_traced(&continued_state);
            let continued_budget = continued
                .trace
                .as_ref()
                .and_then(|trace| trace.budget.as_ref())
                .expect("the active remembered Recon keeps a traced carrier hold");
            assert_eq!(
                continued_budget.prospective_carrier,
                UnitKind::Skyhook.stats().cost,
                "active remembered Recon keeps the exact first-carrier hold for {difficulty:?}"
            );
            assert!(
                continued.commands.iter().all(|command| !matches!(
                    &command.command,
                    Command::Build { .. } | Command::Train { .. } | Command::UpgradeBuilding { .. }
                )),
                "{difficulty:?} spent the active Recon carrier hold: {:?}",
                continued.commands
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
    fn stale_active_recon_releases_its_carrier_floor_before_defense_allocation() {
        let mut scenario = prospective_lift_reservation_scenario();
        scenario.name = "stale Recon releases prospective carrier".into();
        scenario.players[0].scrap = UnitKind::Skyhook.stats().cost;
        scenario
            .buildings
            .retain(|building| building.kind != BuildingKind::Foundry);
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 18,
            y: 19,
        });
        let mut state = scenario
            .build()
            .expect("stale prospective-lift scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);
        let last_seen = state.current_tick().saturating_sub(100);
        let mut brain = brain_with_remembered_lift_target(&scenario, &state, last_seen);

        let admitted = brain.act_traced(&state);
        assert_eq!(
            admitted
                .trace
                .as_ref()
                .and_then(|trace| trace.budget.as_ref())
                .map(|budget| budget.prospective_carrier),
            Some(UnitKind::Skyhook.stats().cost)
        );
        assert_eq!(
            (brain.mind().strategy)
                .air_operation()
                .map(|operation| operation.phase()),
            Some(AirOperationPhase::Recon)
        );

        crate::test_support::set_tick(&mut state, last_seen.saturating_add(541));
        while !state.current_tick().is_multiple_of(24) {
            let next_tick = state.current_tick().saturating_add(1);
            crate::test_support::set_tick(&mut state, next_tick);
        }
        let released = brain.act_traced(&state);
        let trace = released
            .trace
            .as_ref()
            .expect("the stale recovery decision is traced");
        assert_eq!(
            trace
                .budget
                .as_ref()
                .expect("the stale decision retains budget evidence")
                .prospective_carrier,
            0,
            "stale Recon must not impose a phantom Skyhook floor"
        );
        assert!(
            trace.allocation.proposals.entries.iter().any(|proposal| {
                matches!(
                    proposal.key,
                    super::super::trace::ProposalKeyTrace::Defense { .. }
                ) && proposal.disposition == super::super::trace::ProposalDispositionTrace::Accepted
            }),
            "the released carrier bank should remain eligible for the best defensive quote: {trace:#?}"
        );
        let operation = (brain.mind().strategy)
            .air_operation()
            .expect("the stale operation remains observable during recovery");
        assert_eq!(operation.phase(), AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason(),
            Some(AirRecoveryReason::StaleIntelligence)
        );
    }

    #[test]
    fn unreachable_remembered_recon_never_imposes_a_carrier_floor_on_defense() {
        let mut scenario = prospective_lift_reservation_scenario();
        scenario.name = "unreachable Recon releases prospective carrier".into();
        scenario.players[0].scrap = UnitKind::Skyhook.stats().cost;
        scenario
            .buildings
            .retain(|building| building.kind != BuildingKind::Foundry);
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 18,
            y: 19,
        });
        let sealed_scout = TilePos::new(18, 2);
        scenario.map = scenario
            .map
            .iter()
            .enumerate()
            .map(|(y, row)| {
                row.chars()
                    .enumerate()
                    .map(|(x, terrain)| {
                        let tile = TilePos::new(
                            i32::try_from(x).expect("the focused map width fits i32"),
                            i32::try_from(y).expect("the focused map height fits i32"),
                        );
                        if sealed_scout.chebyshev(tile) == 2 {
                            '^'
                        } else {
                            terrain
                        }
                    })
                    .collect()
            })
            .collect();
        let mut state = scenario
            .build()
            .expect("peak-sealed prospective-lift scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);
        let mut brain = brain_with_remembered_lift_target(
            &scenario,
            &state,
            state.current_tick().saturating_sub(100),
        );

        let released = brain.act_traced(&state);
        let trace = released
            .trace
            .as_ref()
            .expect("the unreachable recovery decision is traced");
        assert_eq!(
            trace
                .budget
                .as_ref()
                .expect("the unreachable decision retains budget evidence")
                .prospective_carrier,
            0,
            "a peak-sealed scout route must not impose a phantom Skyhook floor"
        );
        assert!(
            trace.allocation.proposals.entries.iter().any(|proposal| {
                matches!(
                    proposal.key,
                    super::super::trace::ProposalKeyTrace::Defense { .. }
                ) && proposal.disposition == super::super::trace::ProposalDispositionTrace::Accepted
            }),
            "the released carrier bank should remain eligible for the best defensive quote: {:?}",
            trace.allocation.proposals
        );
        let strategy = &brain.mind().strategy;
        assert!(
            strategy.air_operation().is_some_and(|operation| {
                operation.phase() == AirOperationPhase::Recover
                    && operation.recovery_reason() == Some(AirRecoveryReason::UnreachableAirRoute)
                    && !operation.assault_admitted()
            }) || matches!(
                strategy.terminal_outcome(),
                Some(AirOperationOutcome::Aborted { .. })
            ),
            "the route refusal must enter or complete bounded recovery"
        );
    }

    #[test]
    fn fresh_raid_claiming_the_only_lift_payload_releases_the_carrier_hold() {
        let mut scenario = prospective_lift_reservation_scenario();
        scenario.name = "fresh raid releases prospective carrier".into();
        scenario.players[0].scrap = UnitKind::Skyhook.stats().cost;
        let mut retained_sentinels = 0usize;
        scenario.units.retain(|unit| {
            if unit.player != 0 || unit.kind != UnitKind::Sentinel {
                return true;
            }
            retained_sentinels += 1;
            retained_sentinels <= 10
        });
        scenario.units.extend([9, 10].map(|y| UnitSpec {
            player: 0,
            kind: UnitKind::Scuttler,
            x: 16,
            y,
        }));
        let mut state = scenario
            .build()
            .expect("prospective raid/lift scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);

        let raw = Observation::fog_honest(&state, PlayerId(0));
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the home Foundry stands")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);
        let mut brain = scripted_brain(
            &scenario,
            PlayerId(0),
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_024),
        );
        brain.orientation = Some(orientation);
        let enemy_foundry = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(1) && building.kind == BuildingKind::Foundry
            })
            .expect("the enemy Foundry stands beyond the barrier");
        let mut prior = raw.clone();
        prior.tick = prior.tick.saturating_sub(100);
        prior.enemy_buildings.push(BuildingObs {
            hp: enemy_foundry.hp,
            built: enemy_foundry.built,
            tier: enemy_foundry.tier,
            ..crate::test_support::building(
                enemy_foundry.id.0,
                enemy_foundry.player,
                enemy_foundry.kind,
                enemy_foundry.anchor,
            )
        });
        brain
            .mind_mut()
            .intelligence
            .update(&orientation.observe(&prior));

        let scuttlers = raw
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Scuttler)
            .map(|unit| unit.id)
            .collect::<Vec<_>>();
        assert_eq!(scuttlers.len(), 2);
        let act = brain.act_traced(&state);
        let trace = act.trace.expect("the coordinated decision is traced");
        assert!(
            trace
                .allocation
                .proposals
                .entries
                .iter()
                .all(|proposal| proposal.claims.minimum_residual_scrap
                    >= UnitKind::Skyhook.stats().cost),
            "allocation must preview the carrier before residual Raid admission: {:?}",
            trace.allocation.proposals
        );
        assert_eq!(
            (brain.mind().raids)
                .operation()
                .expect("Prime admits the fresh raid beside Recon")
                .members,
            scuttlers
        );
        assert_eq!(
            (brain.mind().strategy)
                .air_operation()
                .map(|operation| operation.phase()),
            Some(AirOperationPhase::Recon)
        );
        assert_eq!(
            trace
                .budget
                .expect("the residual carrier decision is traced")
                .prospective_carrier,
            0,
            "the newly reserved raiders leave no payload for a prospective lift"
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
        brain.dials.minimum_core_equivalents = 0;

        let mut shared_target = None;
        let mut bomber_release = false;
        let mut bomber_release_tick = None;
        let mut lift_held_before_air_release = false;
        let mut first_target_unload_tick = None;
        let mut carrier_loads = Vec::new();
        let mut target_unloads = Vec::new();
        let mut loaded_riders = Vec::new();
        let mut landed_assault = Vec::new();
        let mut allocated_while_cargo_was_in_transit = false;

        for _ in 0..8_000 {
            let cargo_is_transitively_owned = loaded_riders
                .iter()
                .any(|rider| state.unit(*rider).is_none());
            let act = brain.act_traced(&state);
            if cargo_is_transitively_owned && let Some(trace) = act.trace.as_ref() {
                assert!(
                    trace.allocation.error.is_none()
                        && trace.allocation.coordinator_failure.is_none(),
                    "loaded riders are owned through their carrier rather than imported as missing independent units: {:?}",
                    trace.allocation
                );
                allocated_while_cargo_was_in_transit = true;
            }
            let commands = act.commands;
            if shared_target.is_none()
                && let (Some(air), Some(lift)) = (
                    (brain.mind().strategy).air_operation(),
                    (brain.mind().lifts).operation(),
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
                lift_held_before_air_release |=
                    (brain.mind().lifts).operation().is_some_and(|operation| {
                        matches!(
                            operation.phase,
                            LiftPhase::Boarding | LiftPhase::AwaitSupport
                        ) && !operation.manifests.is_empty()
                    }) && !bomber_release;
                for command in &commands {
                    match &command.command {
                        Command::Attack {
                            units,
                            target: command_target,
                            ..
                        } if *command_target
                            == Target::Building(
                                target_id.expect("the shared Foundry is currently identified"),
                            )
                            .into() =>
                        {
                            let strike_aircraft = units
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
                            if strike_aircraft >= 4 && screen >= 2 {
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
                    oxide_sim::event::Event::CommandRejected {
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
            "the shared lift cannot launch before the bomber operation releases it: first_target_unload_tick={first_target_unload_tick:?}, bomber_release_tick={bomber_release_tick:?}"
        );
        assert!(
            carrier_loads.len() >= 3,
            "at least three carriers receive manifests"
        );
        assert!(
            !loaded_riders.is_empty(),
            "the manifests contain a real ground wave"
        );
        assert!(
            allocated_while_cargo_was_in_transit,
            "at least one allocation boundary observes riders transitively owned as carrier cargo"
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
        // This fixture isolates the independent bomber lifecycle. Brain-level
        // opening-core admission is covered by dedicated mixed-roster tests.
        brain.dials.minimum_core_equivalents = 0;

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
                (brain.mind().lifts).operation().is_none(),
                "an air-only roster cannot make the bomber operation depend on a lift"
            );
            let report = state.tick(&commands);
            assert!(
                report.events.iter().all(|event| !matches!(
                    event,
                    oxide_sim::event::Event::CommandRejected {
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

        let (tick, strike_aircraft, screen, wing) =
            launched.expect("the independent bomber wing launches");
        assert!(tick < 10_000);
        assert_eq!(strike_aircraft, 6);
        assert_eq!(screen, 3);
        assert_eq!(
            wing,
            strike_aircraft + screen,
            "the frozen roster launches together"
        );
    }

    #[test]
    fn fallen_prime_core_keeps_paid_work_and_an_active_operation_progressing() {
        let scenario = combined_operation_scenario();
        let mut state = scenario
            .build()
            .expect("the combined-operation continuation builds");
        for _ in 0..6_000 {
            state.tick(&[]);
        }

        let mut brain = operation_identity_brain(PlayerId(0), &scenario);

        let mut prepaid_operation_queue = None;
        for _ in 0..1_000 {
            let commands = brain.act(&state);
            let active = (brain.mind().strategy).air_operation();
            if let Some((building, kind)) =
                commands.iter().find_map(|command| match command.command {
                    Command::Train {
                        building,
                        kind: UnitKind::Condor,
                    } if active.is_some() => Some((building, UnitKind::Condor)),
                    _ => None,
                })
            {
                prepaid_operation_queue = Some((building, kind));
            }
            let report = state.tick(&commands);
            assert!(report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )));
            if prepaid_operation_queue.is_some()
                && (brain.mind().strategy).air_operation().is_some()
            {
                break;
            }
        }
        let (producer, queued_kind) = prepaid_operation_queue
            .expect("the active air operation prepays a Condor in its Airworks queue");
        assert!(
            state
                .building(producer)
                .is_some_and(|building| building.queue.contains(&queued_kind)),
            "the operation-owned production order is paid and remains queued"
        );

        let builder = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
            .expect("the safe home economy retains a voluntary-capital builder")
            .id;
        let capital_anchor = TilePos::new(14, 7);
        let report = state.tick(&[PlayerCommand {
            player: PlayerId(0),
            command: Command::Build {
                units: vec![builder],
                kind: BuildingKind::Array,
                anchor: capital_anchor,
                queue: false,
                defer: false,
            },
        }]);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        let capital_site = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0)
                    && building.kind == BuildingKind::Array
                    && building.anchor == capital_anchor
            })
            .expect("the ordinary command pays for a safe voluntary Array site")
            .id;
        assert!(
            !state
                .building(capital_site)
                .expect("the paid site remains")
                .built,
            "the capital work must still be unfinished before the later loss"
        );

        let lost_core: Vec<_> = state
            .units()
            .iter()
            .filter(|unit| {
                unit.player == PlayerId(0)
                    && matches!(
                        unit.kind.role(),
                        Role::Sentinel | Role::Warden | Role::Breaker
                    )
            })
            .skip(1)
            .map(|unit| unit.id.0)
            .collect();
        assert!(
            lost_core.len() >= 7,
            "the fixture begins with at least Prime's full core before its later loss"
        );
        let mut document = serde_json::to_value(&state).expect("the live continuation serializes");
        document["units"]
            .as_array_mut()
            .expect("units serialize as an array")
            .retain(|unit| {
                !lost_core.contains(&(unit["id"].as_u64().expect("unit ids are numeric") as u32))
            });
        for building in document["buildings"]
            .as_array_mut()
            .expect("buildings serialize as an array")
        {
            building["queue"]
                .as_array_mut()
                .expect("building queues serialize as arrays")
                .retain(|kind| !matches!(kind.as_str(), Some("sentinel" | "warden" | "breaker")));
        }
        state = serde_json::from_value(document).expect("the post-loss state remains valid");
        assert_eq!(
            state
                .units()
                .iter()
                .filter(|unit| {
                    unit.player == PlayerId(0)
                        && matches!(
                            unit.kind.role(),
                            Role::Sentinel | Role::Warden | Role::Breaker
                        )
                })
                .count(),
            1,
            "the later casualty leaves Prime below its eight-equivalent floor"
        );
        assert!(
            state
                .building(producer)
                .is_some_and(|building| building.queue.contains(&queued_kind)),
            "the operation-owned queue survives the core's later losses"
        );
        let post_loss_observation = Observation::fog_honest(&state, PlayerId(0));
        let post_loss_orientation = *brain
            .orientation
            .as_ref()
            .expect("the real Brain latched its player-facing orientation before the loss");
        let post_loss_exclusions = prior_planner_claims(
            &[],
            (brain.mind().strategy).air_operation(),
            &[],
            (brain.mind().raids).reservations(),
            (brain.mind().lifts).operation(),
        );
        let post_loss_oriented = post_loss_orientation.observe(&post_loss_observation);
        let post_loss_core = combat_core_status(
            &post_loss_oriented,
            &post_loss_exclusions,
            &[],
            u64::from(brain.dials.minimum_core_equivalents),
        );
        assert!(
            !post_loss_core.ready,
            "the actual post-loss Brain input must remain below Prime's protected core: \
             {post_loss_core:?}, queues={:?}",
            post_loss_oriented.my_queues
        );
        assert!(
            (brain.mind().strategy).air_operation().is_some(),
            "the paid operation is active before core loss"
        );
        let site_hp_before_loss = state
            .building(capital_site)
            .expect("the paid capital site survived the loss")
            .hp;
        let queue_progress_before_loss = state
            .building(producer)
            .expect("the prepaid operation queue survived the loss")
            .progress;

        let retained_lift_jobs = brain
            .mind()
            .lifts
            .active_production_obligation()
            .map(|obligation| obligation.producer_jobs().to_vec())
            .unwrap_or_default();
        let mut recovery_started = false;
        let mut queued_condor_finished = false;
        let mut operation_continuation_observed = false;
        for _ in 0..1_500 {
            let recovery_observation = Observation::fog_honest(&state, PlayerId(0));
            let recovery_exclusions = prior_planner_claims(
                &[],
                (brain.mind().strategy).air_operation(),
                &[],
                (brain.mind().raids).reservations(),
                (brain.mind().lifts).operation(),
            );
            let core_deficient = !combat_core_status(
                &post_loss_orientation.observe(&recovery_observation),
                &recovery_exclusions,
                &[],
                u64::from(brain.dials.minimum_core_equivalents),
            )
            .ready;
            let operation_was_active = (brain.mind().strategy).air_operation().is_some();
            let commands = brain.act(&state);
            if core_deficient && operation_was_active {
                let strategy = &brain.mind().strategy;
                assert!(
                    strategy.air_operation().is_some() || strategy.terminal_outcome().is_some(),
                    "core loss must not silently discard an active operation"
                );
                operation_continuation_observed = true;
            }
            recovery_started |= core_deficient
                && commands.iter().any(|command| {
                    matches!(
                        command.command,
                        Command::Train {
                            kind: UnitKind::Sentinel,
                            ..
                        }
                    )
                });
            assert!(commands.iter().all(|command| !matches!(
                command.command,
                Command::Cancel { .. } | Command::CancelTrain { .. }
            )));
            if core_deficient {
                assert!(
                    commands.iter().all(|command| match command.command {
                        Command::Build { kind, anchor, .. } => {
                            state.buildings().iter().any(|site| {
                                site.player == PlayerId(0)
                                    && site.kind == kind
                                    && site.anchor == anchor
                            })
                        }
                        Command::Train {
                            kind: UnitKind::Sentinel,
                            ..
                        } => true,
                        Command::Train { building, kind } =>
                            retained_lift_jobs
                                .iter()
                                .any(|job| job.producer() == building
                                    && job.kind() == kind
                                    && job.timing().enqueued_at() == state.current_tick()),
                        Command::UpgradeBuilding { .. } => false,
                        _ => true,
                    }),
                    "a deficient core may execute an existing exact producer commitment but must not admit new specialty capital: {commands:?}"
                );
            }

            let report = state.tick(&commands);
            assert!(report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )));
            queued_condor_finished |= report.events.iter().any(|event| {
                matches!(
                    event,
                    oxide_sim::event::Event::UnitTrained {
                        kind,
                        player: PlayerId(0),
                        ..
                    } if *kind == queued_kind
                )
            });
            if recovery_started
                && queued_condor_finished
                && state
                    .building(capital_site)
                    .is_some_and(|site| site.hp > site_hp_before_loss)
                && operation_continuation_observed
            {
                break;
            }
        }

        assert!(recovery_started, "Prime resumes its ordinary Sentinel line");
        assert!(
            state
                .building(capital_site)
                .is_some_and(|site| site.hp > site_hp_before_loss),
            "the paid voluntary site remains and advances through core recovery"
        );
        assert!(
            state.building(producer).is_some_and(|building| {
                building.progress > queue_progress_before_loss || queued_condor_finished
            }),
            "the prepaid operation queue keeps making ordinary production progress"
        );
        assert!(
            queued_condor_finished,
            "the prepaid operation unit completes without a repurchase"
        );
        assert!(
            operation_continuation_observed,
            "the existing operation survives core loss until its ordinary terminal transition"
        );
    }

    #[test]
    fn opening_core_gate_blocks_fresh_strategic_work_at_the_brain_boundary() {
        let scenario = independent_bomber_operation_scenario();
        let mut state = scenario
            .build()
            .expect("independent bomber scenario builds");
        for _ in 0..6_000 {
            state.tick(&[]);
        }

        let mut gated = operation_identity_brain(PlayerId(0), &scenario);
        let commands = gated.act(&state);
        assert!((gated.mind().strategy).air_operation().is_none());
        assert!((gated.mind().lifts).operation().is_none());
        assert!((gated.mind().raids).operation().is_none());
        assert!(commands.iter().all(|command| !matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Skyhook
                    | UnitKind::Condor
                    | UnitKind::Moth
                    | UnitKind::Buzzard
                    | UnitKind::Darter,
                ..
            }
        )));

        let mut admitted = operation_identity_brain(PlayerId(0), &scenario);
        admitted.dials.minimum_core_equivalents = 0;
        let mut control = state.clone();
        for _ in 0..4_000 {
            let commands = admitted.act(&control);
            if (admitted.mind().strategy).air_operation().is_some() {
                break;
            }
            control.tick(&commands);
        }
        assert!(
            (admitted.mind().strategy).air_operation().is_some(),
            "the same fog-honest snapshot must otherwise qualify for a fresh air operation"
        );
    }

    #[test]
    fn opening_emergency_defense_and_core_recovery_commit_once_through_state() {
        let mut scenario = Scenario::skirmish();
        scenario.name = "opening emergency defense transaction".into();
        scenario.players[0].scrap = BuildingKind::Turret
            .base_stats()
            .construction
            .expect("Turrets are constructible")
            .cost
            .saturating_add(UnitKind::Sentinel.stats().cost);
        scenario
            .units
            .retain(|unit| unit.player != 0 || unit.kind != UnitKind::Sentinel);
        let pressure = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 1 && unit.kind == UnitKind::Sentinel)
            .expect("Skirmish has one hostile Sentinel");
        (pressure.x, pressure.y) = (4, 12);

        let mut state = scenario
            .build()
            .expect("the opening emergency scenario builds");
        let config = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 4_104);
        let mut brain = scripted_brain(&scenario, PlayerId(0), config);
        brain.dials.minimum_core_equivalents = 1;
        brain.dials.harvester_target = 3;
        brain.dials.turret_response = true;
        brain.dials.scouting = false;
        brain.dials.aa_response = false;
        brain.dials.tech = false;
        brain.dials.deep_tech = false;
        brain.dials.radar = false;
        brain.dials.reclaimers = false;
        brain.dials.upgrades = false;
        brain.dials.expansion = false;
        brain.dials.extractors = false;
        brain.dials.mines = false;

        let first = brain.act_traced(&state);
        let trace = first.trace.expect("the emergency allocation is traced");
        let emergency = trace
            .allocation
            .obligations
            .entries
            .iter()
            .find_map(|obligation| match obligation.key {
                super::super::trace::ObligationKeyTrace::EmergencyDefense {
                    building: BuildingKind::Turret,
                    anchor,
                } => Some((anchor, obligation.claims.builders.entries.as_slice())),
                _ => None,
            })
            .expect("the visible ground threat admits one exact emergency Turret");
        let [expected_builder] = emergency.1 else {
            panic!("the frozen emergency payload owns one exact builder: {trace:?}");
        };
        let build_index = first
            .commands
            .iter()
            .position(|command| {
                matches!(
                    &command.command,
                    Command::Build {
                        units,
                        kind: BuildingKind::Turret,
                        anchor,
                        queue: false,
                        defer: false,
                    } if units.as_slice() == [*expected_builder] && *anchor == emergency.0
                )
            })
            .expect("the selected emergency payload lowers without reranking");
        let recovery_index = first
            .commands
            .iter()
            .position(|command| {
                matches!(
                    command.command,
                    Command::Train {
                        kind: UnitKind::Sentinel,
                        ..
                    }
                )
            })
            .expect("capital left after the defense funds core recovery");
        assert!(
            build_index < recovery_index,
            "survival construction must precede ordinary core recovery: {:?}",
            first.commands
        );
        assert_eq!(
            first
                .commands
                .iter()
                .filter(|command| matches!(
                    command.command,
                    Command::Build {
                        kind: BuildingKind::Turret,
                        ..
                    }
                ))
                .count(),
            1
        );

        let report = state.tick(&first.commands);
        assert!(
            report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "State must accept both exactly funded commands: {:?}",
            report.events
        );
        assert_eq!(state.player(PlayerId(0)).scrap, 0);
        assert!(state.buildings().iter().any(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Turret
                && building.anchor == emergency.0
        }));
        assert!(state.buildings().iter().any(|building| {
            building.player == PlayerId(0)
                && building.kind == BuildingKind::Foundry
                && building.queue.front() == Some(&UnitKind::Sentinel)
        }));

        while state.current_tick() < brain.dials.cadence {
            state.tick(&[]);
        }
        let next = brain.act(&state);
        assert!(
            next.iter().all(|command| !matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Turret,
                    ..
                }
            )),
            "the paid in-progress defense must not be admitted twice: {next:?}"
        );
    }

    #[test]
    fn strategic_air_spending_cannot_consume_the_opening_bootstrap_reserve() {
        let mut scenario = independent_bomber_operation_scenario();
        scenario.name = "brain opening bootstrap reserve".into();
        scenario.map[11].replace_range(20..=20, ".");
        scenario.players[0].scrap = 0;
        scenario.units.extend((0..4).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 4 + index,
            y: 8,
        }));
        scenario.units.extend((0..8).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 4 + index,
            y: 10,
        }));
        let mut scouting_state = scenario
            .build()
            .expect("the connected strategic-capital fixture builds");
        for _ in 0..6_000 {
            scouting_state.tick(&[]);
        }

        let mut brain = operation_identity_brain(PlayerId(0), &scenario);

        for _ in 0..1_000 {
            let commands = brain.act(&scouting_state);
            let report = scouting_state.tick(&commands);
            assert!(report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )));
            if (brain.mind().strategy).air_operation().is_some() {
                break;
            }
        }
        assert!(
            (brain.mind().strategy).air_operation().is_some(),
            "current scout sight should establish the strategic operation"
        );
        let mut bootstrap_brain = brain.clone();
        brain.exec = Executive::default();

        let mut bootstrap_scenario = scenario;
        bootstrap_scenario.name = "brain strategic opening bootstrap reserve".into();
        bootstrap_scenario.players[0].scrap = UnitKind::Harvester.stats().cost
            + BuildingKind::Extractor
                .base_stats()
                .construction
                .expect("Extractors have a construction price")
                .cost;
        bootstrap_scenario
            .buildings
            .retain(|building| building.kind != BuildingKind::Reclaimer);
        let mut kept_harvesters = 0usize;
        bootstrap_scenario.units.retain(|unit| {
            if unit.player != 0 || unit.kind != UnitKind::Harvester {
                return unit.kind != UnitKind::Kestrel;
            }
            kept_harvesters += 1;
            kept_harvesters <= 3
        });
        bootstrap_scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 12,
            y: 11,
        });
        let home_frame = TilePos::new(8, 11);
        let row = bootstrap_scenario
            .map
            .get_mut(home_frame.y as usize)
            .expect("the fixture contains the home-frame row");
        let mut bytes = row.as_bytes().to_vec();
        bytes[home_frame.x as usize] = b'E';
        *row = String::from_utf8(bytes).expect("the fixture map remains ASCII");
        let mut bootstrap_state = bootstrap_scenario
            .build()
            .expect("the opening-bootstrap continuation builds");
        while bootstrap_state.current_tick() <= scouting_state.current_tick()
            || !super::super::difficulty::strategic_admission_tick(bootstrap_state.current_tick())
        {
            bootstrap_state.tick(&[]);
        }
        let bootstrap_scrap = UnitKind::Harvester.stats().cost
            + BuildingKind::Extractor
                .base_stats()
                .construction
                .expect("Extractors have a construction price")
                .cost;
        let mut document =
            serde_json::to_value(&bootstrap_state).expect("the bootstrap state serializes");
        document["players"][0]["scrap"] = serde_json::json!(bootstrap_scrap);
        bootstrap_state =
            serde_json::from_value(document).expect("the exact bootstrap bank is valid");
        bootstrap_brain.exec = Executive::default();

        let commands = bootstrap_brain.act(&bootstrap_state);
        assert!(commands.iter().any(|command| matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Harvester,
                ..
            }
        )));
        assert!(commands.iter().any(|command| matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Extractor,
                anchor,
                ..
            } if anchor == home_frame
        )));
        assert!(commands.iter().all(|command| !matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Kestrel | UnitKind::Buzzard | UnitKind::Condor | UnitKind::Skyhook,
                ..
            }
        )));
        let report = bootstrap_state.tick(&commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        assert_eq!(
            bootstrap_state.player(PlayerId(0)).scrap,
            0,
            "the fourth worker and supported home Extractor own the full exact bootstrap bank"
        );
    }

    #[test]
    fn accepted_foundry_saving_owns_the_bank_before_later_connected_air_production() {
        use super::super::profile::PersonalityTraits;

        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
        let state = scenario
            .build()
            .expect("the Foundry-saving air competition scenario builds");
        let profile = ResolvedProfile {
            difficulty: BotDifficulty::Standard,
            stance: BotStance::Balanced,
            personality_seed: 1_616_304,
            primary: Specialty::Air,
            secondary: Specialty::Siege,
            traits: PersonalityTraits {
                air: 70,
                siege: 60,
                support: 35,
                fortification: 35,
                greed: 64,
                guile: 36,
            },
        };
        let mut brain = scripted_brain(
            &scenario,
            PlayerId(0),
            BotConfig::scripted(profile.difficulty, profile.stance, profile.personality_seed),
        );
        brain.dials = Dials::scripted(&profile, DifficultyTuning::for_level(profile.difficulty));
        brain.dials.harvester_target = 4;
        brain.dials.army_size = 100;
        brain.dials.scouting = false;
        brain.dials.extractors = false;
        brain.dials.upgrades = false;
        let mind = brain.mind_mut();
        mind.profile = profile;

        let first_commands = brain.act(&state);
        let raw = Observation::fog_honest(&state, PlayerId(0));
        let orientation = brain
            .orientation
            .expect("the first think latches the policy frame");
        let oriented = orientation.observe(&raw);
        let saved = brain.policy.validated_foundry_saving(&oriented, true);
        assert!(
            saved > foundry_cost - 1,
            "the first think must accept the underfunded expansion before strategic competition: saved={saved}, commands={first_commands:?}, buildings={:?}, units={}, frames={:?}",
            oriented
                .my_buildings
                .iter()
                .map(|building| (building.kind, building.anchor))
                .collect::<Vec<_>>(),
            oriented.my_units.len(),
            oriented.known_frames,
        );
        brain.dials.expansion = false;

        let mut continuation = scenario.clone();
        let scout = continuation
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
            .expect("the continuation has one connected-air scout");
        (scout.x, scout.y) = (42, 19);
        let mut state = continuation
            .build()
            .expect("the connected-air continuation builds");
        state.tick(&[]);
        while !state.current_tick().is_multiple_of(brain.dials.cadence)
            || !super::super::difficulty::strategic_admission_tick(state.current_tick())
        {
            state.tick(&[]);
        }
        brain.mind_mut().strategy = StrategicPlanner::new();
        let mut wealthy_brain = brain.clone();

        let mut document = serde_json::to_value(&state).expect("the state serializes");
        document["players"][0]["scrap"] = serde_json::json!(saved - 1);
        let starved_state: State =
            serde_json::from_value(document.clone()).expect("the underfunded state remains valid");
        let starved_observation = brain
            .orientation
            .expect("the policy frame remains latched")
            .observe(&Observation::fog_honest(&starved_state, PlayerId(0)));
        assert!(
            starved_observation
                .enemy_buildings
                .iter()
                .any(|building| building.kind == BuildingKind::Foundry),
            "the connected opportunity must be in current sight"
        );
        let mut direct_starved_brain = brain.clone();
        let direct_starved_commands = direct_starved_brain.act(&starved_state);
        let starved = brain.act_traced(&starved_state);
        assert_eq!(starved.commands, direct_starved_commands);
        let mut direct_after = starved_state.clone();
        let mut traced_after = starved_state.clone();
        direct_after.tick(&direct_starved_commands);
        traced_after.tick(&starved.commands);
        assert_eq!(traced_after.hash(), direct_after.hash());
        assert_brain_unchanged(&direct_starved_brain, &brain);
        let starved_trace = starved.trace.expect("the admission think is traced");
        let starved_budget = starved_trace
            .budget
            .expect("the admission think records its scrap ledger");
        assert_eq!(starved_budget.foundry_saving, saved);
        assert_eq!(starved_budget.strategic_spendable, 0);
        assert_eq!(
            starved_trace.channels.connected_air.after,
            ChannelState::Idle,
            "an unfunded coherent package must not become an active operation"
        );
        assert_eq!(
            starved_trace.connected_force.status,
            super::super::trace::ConnectedForceStatus::Idle
        );
        assert!(starved_trace.connected_force.package.is_none());
        assert!(
            starved_trace.connected_force.rejected_candidate.is_none(),
            "the connected domain produced a legal proposal for shared adjudication"
        );
        let connected_context = starved_trace
            .allocation
            .connected_context
            .expect("the allocator compared exact connected-presence contexts");
        assert!(
            connected_context.considered >= 2,
            "the absent and minimum connected contexts must both reach shared adjudication"
        );
        assert_eq!(
            connected_context.selected,
            super::super::trace::ConnectedPortfolioSelectionTrace::Absent,
            "the older Foundry obligation must make the later connected context lose"
        );
        assert_eq!(
            starved_trace.channels.connected_air.effects.committed_scrap, 0,
            "connected air may see only bank beyond the frozen Foundry total"
        );
        let mut continued_state = starved_state.clone();
        for _ in 0..brain.dials.cadence {
            continued_state.tick(&[]);
        }
        let mut continued_document =
            serde_json::to_value(&continued_state).expect("the continued state serializes");
        continued_document["players"][0]["scrap"] = serde_json::json!(saved - 1);
        let continued_state = serde_json::from_value(continued_document)
            .expect("the normalized continued state remains valid");
        let continued = brain.act_traced(&continued_state);
        let continued_trace = continued.trace.expect("the next strategic think is traced");
        let continued_budget = continued_trace
            .budget
            .expect("the active operation's next think records its scrap ledger");
        assert_eq!(
            continued_budget.prior_operation_spendable, 0,
            "no operation predates the accepted Foundry saving"
        );
        assert_eq!(continued_budget.strategic_spendable, 0);
        assert_eq!(
            continued_trace.channels.connected_air.after,
            ChannelState::Idle
        );
        assert_eq!(
            continued_trace
                .channels
                .connected_air
                .effects
                .committed_scrap,
            0,
            "a post-saving opportunity cannot move ahead of the Foundry hold"
        );
        let starved_raw = Observation::fog_honest(&starved_state, PlayerId(0));
        assert_eq!(
            brain
                .policy
                .validated_foundry_saving(&orientation.observe(&starved_raw), true),
            saved,
            "strategic competition cannot shrink the accepted saving"
        );

        let operation_fund = UnitKind::Bombard
            .stats()
            .cost
            .max(UnitKind::Avalanche.stats().cost)
            .saturating_add(UnitKind::Condor.stats().cost);
        document["players"][0]["scrap"] = serde_json::json!(saved.saturating_add(operation_fund));
        let funded_state: State =
            serde_json::from_value(document).expect("the fully funded state remains valid");
        let funded = wealthy_brain.act_traced(&funded_state);
        let funded_trace = funded.trace.expect("the funded admission think is traced");
        let funded_budget = funded_trace
            .budget
            .expect("the funded think records its scrap ledger");
        let standing_current_scrap = funded_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                matches!(
                    job.owner,
                    super::super::trace::ClaimOwnerTrace::Proposal {
                        key: super::super::trace::ProposalKeyTrace::StandingForce { .. },
                    }
                )
            })
            .map(|job| job.current_scrap)
            .sum::<u32>();
        let capital_current_scrap = funded_trace
            .allocation
            .proposals
            .entries
            .iter()
            .filter(|proposal| {
                proposal.disposition == super::super::trace::ProposalDispositionTrace::Accepted
                    && matches!(
                        proposal.key,
                        super::super::trace::ProposalKeyTrace::Defense { .. }
                            | super::super::trace::ProposalKeyTrace::Economy { .. }
                    )
            })
            .map(|proposal| proposal.claims.current_scrap)
            .sum::<u32>();
        assert_eq!(funded_budget.foundry_saving, saved);
        assert_eq!(
            funded_budget.voluntary_scrap_guard,
            UnitKind::Sentinel.stats().cost,
            "a selected non-Sentinel standing-force alternative must leave the shallow screen available"
        );
        assert_eq!(
            funded_budget
                .strategic_spendable
                .saturating_add(standing_current_scrap)
                .saturating_add(capital_current_scrap),
            operation_fund.saturating_sub(funded_budget.voluntary_scrap_guard),
            "the accepted Foundry owns only its construction capital; shared allocation may spend the independent excess on connected, standing-force, and defense investment after retaining the shallow screen exactly once: budget={funded_budget:?}, schedule={:?}",
            funded_trace.allocation.producer_schedule,
        );
        assert!(
            funded_trace
                .allocation
                .producer_schedule
                .entries
                .iter()
                .any(|job| matches!(
                    job.owner,
                    super::super::trace::ClaimOwnerTrace::Obligation {
                        key: super::super::trace::ObligationKeyTrace::ConnectedOffense { .. },
                        ..
                    } | super::super::trace::ClaimOwnerTrace::Proposal {
                        key: super::super::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
                    }
                )),
            "connected air may claim the independent excess bank"
        );
        assert!(
            funded
                .commands
                .iter()
                .any(|command| matches!(command.command, Command::Train { .. })),
            "bank covering both obligations must admit ordinary paid air-operation work: {:?}",
            funded.commands
        );
    }

    #[test]
    fn older_construction_promise_owns_forecast_until_current_bank_covers_it() {
        let promised_kind = BuildingKind::Foundry;
        let promised_scrap = promised_kind
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let mut scenario = foundry_saving_air_competition_scenario(0);
        let scout = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
            .expect("the scenario has one connected-operation scout");
        (scout.x, scout.y) = (42, 19);
        scenario.buildings.extend([
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 18,
                y: 4,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 21,
                y: 4,
            },
        ]);
        let mut state = scenario
            .build()
            .expect("the forecast-ownership scenario builds");
        let founder = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
            .expect("the scenario has a builder")
            .id;
        let promised_anchor = TilePos::new(20, 9);
        crate::test_support::edit_units(&mut state, |units| {
            let founder = units.iter_mut().find(|unit| unit.id == founder).unwrap();
            founder.order = oxide_sim::state::Order::Found {
                kind: promised_kind,
                anchor: promised_anchor,
            };
            founder.path = None;
        });

        let mut brain = foundry_competition_brain(&scenario);
        brain.dials.expansion = false;
        brain.dials.minimum_core_equivalents = 0;
        let mut funded_brain = brain.clone();
        let mut funded_state = state.clone();

        let starved = brain.act_traced(&state);
        let starved_trace = starved.trace.expect("the admission think is traced");
        let starved_budget = starved_trace
            .budget
            .expect("the admission think records its scrap ledger");
        assert_eq!(starved_budget.bank, 0);
        assert_eq!(starved_budget.deferred_construction, promised_scrap);
        assert_eq!(starved_budget.strategic_spendable, 0);
        assert_eq!(
            starved_trace.channels.connected_air.after,
            ChannelState::Idle,
            "forecast promised to older construction cannot admit a new operation"
        );
        let rejection = starved_trace
            .connected_force
            .rejected_candidate
            .expect("the current opportunity records the protected forecast");
        assert!(
            matches!(
                rejection.reason,
                super::super::trace::ConnectedRejectionReasonTrace::ProtectedFunds {
                    protected_current_scrap: 0,
                    protected_forecast_scrap,
                    ..
                } if protected_forecast_scrap == promised_scrap
            ),
            "unexpected rejection: {:?}",
            rejection.reason
        );

        crate::test_support::edit_player(&mut funded_state, PlayerId(0), |item| {
            item.scrap = promised_scrap
        });
        let funded = funded_brain.act_traced(&funded_state);
        let funded_trace = funded.trace.expect("the funded think is traced");
        let funded_budget = funded_trace
            .budget
            .expect("the funded think records its scrap ledger");
        assert_eq!(funded_budget.deferred_construction, promised_scrap);
        assert_eq!(
            funded_budget.strategic_spendable, 0,
            "the current bank remains owned by the older construction promise"
        );
        assert!(
            matches!(
                funded_trace.channels.connected_air.after,
                ChannelState::Active(_)
            ),
            "covering the old promise with current capital frees recurring income for admission"
        );
        let package = funded_trace
            .connected_force
            .package
            .expect("the recurring-income surplus admits a concrete package");
        assert_eq!(package.current_scrap, 0);
        assert!(package.forecast_scrap > promised_scrap);
    }

    #[test]
    fn compatible_connected_minimum_and_foundry_defer_only_residual_scale() {
        use super::super::profile::PersonalityTraits;

        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let mut scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
        let scout = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
            .expect("the competition scenario has one connected-air scout");
        (scout.x, scout.y) = (42, 19);
        scenario.units.extend([
            UnitSpec {
                player: 0,
                kind: UnitKind::Bombard,
                x: 9,
                y: 17,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 10,
                y: 18,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 11,
                y: 18,
            },
        ]);
        let mut state = scenario
            .build()
            .expect("the fully staffed connected-air scenario builds");
        let profile = ResolvedProfile {
            difficulty: BotDifficulty::Standard,
            stance: BotStance::Balanced,
            personality_seed: 1_616_304,
            primary: Specialty::Air,
            secondary: Specialty::Siege,
            traits: PersonalityTraits {
                air: 70,
                siege: 60,
                support: 35,
                fortification: 35,
                greed: 64,
                guile: 36,
            },
        };
        let mut brain = scripted_brain(
            &scenario,
            PlayerId(0),
            BotConfig::scripted(profile.difficulty, profile.stance, profile.personality_seed),
        );
        brain.dials = Dials::scripted(&profile, DifficultyTuning::for_level(profile.difficulty));
        brain.dials.harvester_target = 4;
        brain.dials.army_size = 100;
        brain.dials.scouting = false;
        brain.dials.extractors = false;
        brain.dials.upgrades = false;
        let mind = brain.mind_mut();
        mind.profile = profile;

        let admission_tick = state.current_tick();
        let first = brain.act_traced(&state);
        let first_trace = first.trace.as_ref().expect("the admission think is traced");
        assert_eq!(
            first_trace.channels.connected_air.before,
            ChannelState::Idle
        );
        assert_eq!(
            first_trace.channels.connected_air.after,
            ChannelState::Active(ChannelPhase::AirRecon)
        );
        let accepted_keys = first_trace
            .allocation
            .proposals
            .entries
            .iter()
            .filter(|proposal| {
                proposal.disposition == super::super::trace::ProposalDispositionTrace::Accepted
            })
            .map(|proposal| proposal.key)
            .collect::<Vec<_>>();
        assert_eq!(
            accepted_keys.len(),
            4,
            "the staffed connected minimum, expansion, defense, and standing force are compatible: {accepted_keys:?}"
        );
        assert!(accepted_keys.iter().any(|key| matches!(
            key,
            super::super::trace::ProposalKeyTrace::FoundryExpansion { .. }
        )));
        assert!(accepted_keys.iter().any(|key| matches!(
            key,
            super::super::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
        )));
        assert!(
            accepted_keys
                .iter()
                .any(|key| matches!(key, super::super::trace::ProposalKeyTrace::Defense { .. }))
        );
        assert!(accepted_keys.iter().any(|key| matches!(
            key,
            super::super::trace::ProposalKeyTrace::StandingForce { .. }
        )));
        let package = first_trace
            .connected_force
            .package
            .as_ref()
            .expect("the admitted operation exposes its scaled package");
        assert!(
            package
                .demands
                .recon
                .iter()
                .any(|demand| { demand.kind == UnitKind::Kestrel && demand.count > 0 })
        );
        assert!(
            package
                .demands
                .suppression
                .iter()
                .any(|demand| { demand.kind == UnitKind::Bombard && demand.count > 0 })
        );
        assert!(package.demands.strike.iter().any(|demand| {
            matches!(demand.kind, UnitKind::Buzzard | UnitKind::Condor) && demand.count > 0
        }));
        assert!(first.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Buzzard,
                ..
            }
        )));
        let residual_buzzard = first_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .find(|job| job.kind == UnitKind::Buzzard)
            .expect("the residual connected scale retains its optional Buzzard")
            .clone();
        assert!(residual_buzzard.enqueued_at > admission_tick);
        assert_eq!(residual_buzzard.current_scrap, 0);
        assert_eq!(
            residual_buzzard.forecast_scrap,
            UnitKind::Buzzard.stats().cost
        );
        let admitted_at = {
            let strategy = &brain.mind().strategy;
            let operation = strategy
                .air_operation()
                .expect("the normal strategic pass admits connected air");
            assert!(operation.scout.is_some());
            assert!(!operation.artillery.is_empty());
            assert!(!operation.strike_aircraft.is_empty());
            strategy
                .air_admitted_at()
                .expect("the admitted operation records its priority tick")
        };
        assert_eq!(admitted_at, admission_tick);
        let orientation = brain
            .orientation
            .expect("the first think latches the policy frame");
        let first_raw = Observation::fog_honest(&state, PlayerId(0));
        let first_oriented = orientation.observe(&first_raw);
        let saved = brain.policy.validated_foundry_saving(&first_oriented, true);
        assert!(
            saved > state.player(PlayerId(0)).scrap,
            "utility accepts the underfunded Foundry after the fully staffed operation"
        );
        assert!(
            brain.policy.operation_precedes_foundry_saving(admitted_at),
            "same-pass strategic admission precedes utility expansion"
        );

        let report = state.tick(&first.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        while state.current_tick() < admission_tick.saturating_add(brain.dials.cadence) {
            state.tick(&[]);
        }
        assert_eq!(
            state.current_tick(),
            admission_tick.saturating_add(brain.dials.cadence)
        );

        let mut document = serde_json::to_value(&state).expect("the continuation serializes");
        document["players"][0]["scrap"] = serde_json::json!(saved - 1);
        let later_state: State =
            serde_json::from_value(document).expect("the underfunded continuation remains valid");

        let later = brain.act_traced(&later_state);
        let later_trace = later.trace.expect("the later cadence think is traced");
        let later_budget = later_trace
            .budget
            .expect("the later think records its scrap ledger");
        assert!(later_budget.bank < saved);
        assert_eq!(later_budget.foundry_saving, saved);
        assert_eq!(later_budget.strategic_spendable, 0);
        assert!(later_trace.allocation.coordinator_failure.is_none());
        let retained_buzzard = later_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .find(|job| {
                job.producer == residual_buzzard.producer && job.kind == residual_buzzard.kind
            })
            .expect("the earlier operation retains its optional Buzzard demand");
        assert_eq!(retained_buzzard.ready_before, residual_buzzard.ready_before);
        assert!(retained_buzzard.enqueued_at >= later_state.current_tick());
        assert_eq!(brain.mind().strategy.air_admitted_at(), Some(admitted_at));
        assert!(
            matches!(
                retained_buzzard.owner,
                super::super::trace::ClaimOwnerTrace::Proposal {
                    key: super::super::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
                }
            ),
            "optional growth is adjudicated afresh rather than promoted to mandatory debt"
        );
        let matching_commands = later
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Train { building, kind }
                        if building == residual_buzzard.producer
                            && kind == residual_buzzard.kind
                )
            })
            .count();
        assert_eq!(
            matching_commands,
            usize::from(retained_buzzard.enqueued_at == later_state.current_tick()),
            "the retained Buzzard must dispatch exactly on its allocated enqueue tick"
        );
        let later_raw = Observation::fog_honest(&later_state, PlayerId(0));
        assert_eq!(
            brain
                .policy
                .validated_foundry_saving(&orientation.observe(&later_raw), true),
            saved,
            "the earlier operation spends ahead of, but does not erase, the saved Foundry"
        );
    }

    #[test]
    fn one_allocation_dispatches_compatible_foundry_and_connected_offense_commands() {
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let mut scenario = foundry_saving_air_competition_scenario(
            foundry_cost
                .saturating_add(UnitKind::Buzzard.stats().cost)
                .saturating_add(UnitKind::Sentinel.stats().cost)
                .saturating_add(
                    BuildingKind::Turret
                        .base_stats()
                        .construction
                        .expect("Turrets are constructible")
                        .cost,
                )
                .saturating_add(UnitKind::Harvester.stats().cost),
        );
        scenario.name = "simultaneous Foundry and connected offense dispatch".into();
        scenario.buildings.extend((0..8).map(|index| BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: 16 + index * 3,
            y: 2,
        }));
        let scout = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
            .expect("the scenario has one connected-operation scout");
        (scout.x, scout.y) = (42, 19);
        scenario.units.extend([
            UnitSpec {
                player: 0,
                kind: UnitKind::Bombard,
                x: 9,
                y: 17,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 10,
                y: 18,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 11,
                y: 18,
            },
        ]);
        let mut state = scenario
            .build()
            .expect("the simultaneous allocation scenario builds");
        let mut brain = foundry_competition_brain(&scenario);

        let act = brain.act_traced(&state);
        let trace = act.trace.expect("the shared admission boundary is traced");
        assert!(
            act.commands.iter().any(|command| matches!(
                command.command,
                Command::Build {
                    kind: BuildingKind::Foundry,
                    ..
                }
            )),
            "the accepted expansion must lower its exact build command: {:?}",
            act.commands
        );
        assert!(trace.allocation.error.is_none());
        assert!(trace.allocation.coordinator_failure.is_none());
        let accepted_keys = trace
            .allocation
            .proposals
            .entries
            .iter()
            .filter(|proposal| {
                proposal.disposition == super::super::trace::ProposalDispositionTrace::Accepted
            })
            .map(|proposal| proposal.key)
            .collect::<Vec<_>>();
        assert_eq!(accepted_keys.len(), 4);
        assert!(accepted_keys.iter().any(|key| matches!(
            key,
            super::super::trace::ProposalKeyTrace::FoundryExpansion { .. }
        )));
        assert!(accepted_keys.iter().any(|key| matches!(
            key,
            super::super::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
        )));
        assert!(accepted_keys.iter().any(|key| matches!(
            key,
            super::super::trace::ProposalKeyTrace::StandingForce { .. }
        )));
        assert!(
            accepted_keys
                .iter()
                .any(|key| matches!(key, super::super::trace::ProposalKeyTrace::Defense { .. }))
        );
        let connected_jobs = trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                matches!(
                    job.owner,
                    super::super::trace::ClaimOwnerTrace::Proposal {
                        key: super::super::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. },
                    }
                )
            })
            .collect::<Vec<_>>();
        assert!(
            !connected_jobs.is_empty()
                && connected_jobs
                    .iter()
                    .any(|job| job.enqueued_at == state.current_tick())
                && connected_jobs
                    .iter()
                    .any(|job| job.enqueued_at > state.current_tick()),
            "the accepted connected scale must retain both its current append and exact future producer schedule: {connected_jobs:?}"
        );
        let accepted_due = trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .filter(|job| job.enqueued_at == state.current_tick())
            .map(|job| (job.producer, job.kind))
            .collect::<Vec<_>>();
        let lowered = act
            .commands
            .iter()
            .filter_map(|command| match command.command {
                Command::Train { building, kind } => Some((building, kind)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            lowered.starts_with(&accepted_due),
            "allocated current producer work must lower first in exact schedule order: accepted={accepted_due:?}, lowered={lowered:?}"
        );

        let report = state.tick(&act.commands);
        assert!(
            report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "the authoritative State must accept all allocated commands in one transaction: {:?}",
            report.events
        );
    }

    fn connected_provider_due_next_cadence() -> (
        State,
        Brain,
        super::super::trace::ScheduledProducerJobTrace,
        super::super::strategy::AirOperation,
    ) {
        let mut scenario = foundry_saving_air_competition_scenario(
            UnitKind::Buzzard
                .stats()
                .cost
                .saturating_add(UnitKind::Lancer.stats().cost)
                .saturating_add(
                    BuildingKind::Turret
                        .base_stats()
                        .construction
                        .expect("Turrets are constructible")
                        .cost,
                )
                .saturating_add(UnitKind::Sentinel.stats().cost),
        );
        scenario.name = "connected procurement at the next decision".into();
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Foundry,
            x: 40,
            y: 2,
        });
        let scout = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
            .expect("the scenario has one connected-operation scout");
        (scout.x, scout.y) = (42, 19);
        scenario.units.extend([
            UnitSpec {
                player: 0,
                kind: UnitKind::Bombard,
                x: 9,
                y: 17,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 10,
                y: 18,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 11,
                y: 18,
            },
        ]);
        let mut state = scenario
            .build()
            .expect("the delayed connected-provider scenario builds");
        let airworks = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0) && building.kind == BuildingKind::Airworks
            })
            .expect("the scenario has one Airworks")
            .id;
        crate::test_support::edit_building(&mut state, airworks, |airworks_state| {
            airworks_state.queue.extend(core::iter::repeat_n(
                UnitKind::Kestrel,
                oxide_sim::stats::QUEUE_CAP,
            ));
            airworks_state.progress = UnitKind::Kestrel.stats().train_ticks - 1;
        });

        let mut brain = foundry_competition_brain(&scenario);
        brain.dials.expansion = false;

        let admission = brain.act_traced(&state);
        let admission_trace = admission
            .trace
            .as_ref()
            .expect("the connected admission is traced");
        assert!(admission_trace.allocation.error.is_none());
        assert!(admission_trace.allocation.coordinator_failure.is_none());
        assert!(
            admission_trace
                .allocation
                .proposals
                .entries
                .iter()
                .any(|proposal| {
                    proposal.disposition == super::super::trace::ProposalDispositionTrace::Accepted
                        && matches!(
                            proposal.key,
                            super::super::trace::ProposalKeyTrace::Defense { .. }
                        )
                })
        );
        let target = (brain.mind().strategy)
            .air_operation()
            .expect("the connected operation is admitted")
            .clone();
        let due = admission_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                matches!(
                    job.owner,
                    super::super::trace::ClaimOwnerTrace::Proposal {
                        key: super::super::trace::ProposalKeyTrace::ConnectedOffenseMinimum { .. }
                    }
                )
            })
            .min_by_key(|job| job.enqueued_at)
            .cloned()
            .expect("the full queue defers connected procurement");
        assert_eq!(
            due.enqueued_at, brain.dials.cadence,
            "the nearly complete front item should expose one slot at the next decision"
        );

        let report = state.tick(&admission.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        while state.current_tick() < due.enqueued_at {
            state.tick(&[]);
        }

        (state, brain, due, target)
    }

    #[test]
    fn lost_connected_objective_releases_unpaid_demand_before_purchase() {
        let (mut state, mut brain, due, target) = connected_provider_due_next_cadence();

        let target_id = target
            .target_id
            .expect("the connected target was current at admission");
        let mut document = serde_json::to_value(&state).expect("the due state serializes");
        document["buildings"]
            .as_array_mut()
            .expect("buildings serialize as an array")
            .retain(|building| building["id"].as_u64() != Some(u64::from(target_id.0)));
        state = serde_json::from_value(document)
            .expect("removing the currently visible objective preserves state invariants");
        let target_visible = Observation::fog_honest(&state, PlayerId(0));
        let (target_width, target_height) = target.target_kind.base_stats().size;
        assert!(
            (0..target_height)
                .flat_map(|dy| (0..target_width).map(move |dx| target.target.offset(dx, dy)))
                .all(|tile| target_visible.visible(tile)),
            "current sight must prove that the accepted objective disappeared"
        );

        let recovery = brain.act_traced(&state);
        let recovery_trace = recovery
            .trace
            .as_ref()
            .expect("the due recovery decision is traced");
        assert!(recovery_trace.allocation.error.is_none());
        assert!(recovery_trace.allocation.coordinator_failure.is_none());
        assert_eq!(
            recovery_trace.connected_force.status,
            super::super::trace::ConnectedForceStatus::Recovering(
                super::super::trace::ConnectedRecoveryReasonTrace::ObjectiveLost,
            )
        );
        let accepted_due = recovery_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                job.producer == due.producer
                    && job.kind == due.kind
                    && job.enqueued_at == state.current_tick()
                    && matches!(
                        job.owner,
                        super::super::trace::ClaimOwnerTrace::Obligation {
                            class: super::super::trace::ObligationClassTrace::PersistentPlan,
                            key: super::super::trace::ObligationKeyTrace::ConnectedOffense { .. },
                            ..
                        }
                    )
            })
            .count();
        assert_eq!(
            accepted_due, 0,
            "an obsolete quotation must not buy a provider for a lost objective"
        );
        let emitted_due = recovery
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Train { building, kind }
                        if building == due.producer && kind == due.kind
                )
            })
            .count();
        assert_eq!(
            emitted_due, 0,
            "recovery must not turn an old quotation into a new purchase: {:?}",
            recovery.commands
        );

        let report = state.tick(&recovery.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        while !state.current_tick().is_multiple_of(brain.dials.cadence) {
            state.tick(&[]);
        }
        assert!(state.player(PlayerId(0)).scrap >= due.kind.stats().cost);

        let next = brain.act_traced(&state);
        let next_trace = next
            .trace
            .as_ref()
            .expect("the next-cadence recovery decision is traced");
        assert!(
            next_trace
                .allocation
                .producer_schedule
                .entries
                .iter()
                .all(|job| {
                    !(job.producer == due.producer
                        && job.kind == due.kind
                        && job.request_ordinal == due.request_ordinal)
                })
        );
        assert!(
            next.commands.iter().all(|command| !matches!(
                command.command,
                Command::Train { building, kind }
                    if building == due.producer && kind == due.kind
            )),
            "recovery must not retry an unpaid quotation on the next decision: {:?}",
            next.commands
        );
    }

    #[test]
    fn harvester_recovery_cancels_a_due_connected_provider_and_recalls_its_force() {
        let (state, mut brain, due, _) = connected_provider_due_next_cadence();
        assert_ne!(due.kind, UnitKind::Harvester);

        let recovery_bank = UnitKind::Harvester
            .stats()
            .cost
            .saturating_add(due.kind.stats().cost);
        let mut document = serde_json::to_value(&state).expect("the due state serializes");
        document["players"][0]["scrap"] = serde_json::json!(recovery_bank);
        document["units"]
            .as_array_mut()
            .expect("state units serialize as an array")
            .retain(|unit| unit["kind"].as_str() != Some("harvester"));
        for building in document["buildings"]
            .as_array_mut()
            .expect("state buildings serialize as an array")
        {
            building["queue"]
                .as_array_mut()
                .expect("building queues serialize as arrays")
                .retain(|kind| kind.as_str() != Some("harvester"));
        }
        let mut state: State = serde_json::from_value(document)
            .expect("removing every Harvester preserves state invariants");

        let raw = Observation::fog_honest(&state, PlayerId(0));
        assert!(
            raw.my_units
                .iter()
                .all(|unit| unit.kind != UnitKind::Harvester)
                && raw
                    .my_queues
                    .iter()
                    .flatten()
                    .all(|kind| *kind != UnitKind::Harvester),
            "the due decision must begin with no live or queued Harvester"
        );
        let (foundry, home) = raw
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(index, building)| {
                building.kind == BuildingKind::Foundry
                    && building.built
                    && raw.my_queues[*index].len() < oxide_sim::stats::QUEUE_CAP
            })
            .min_by_key(|(_, building)| building.id)
            .map(|(_, building)| (building.id, building.anchor))
            .expect("an affordable completed Foundry has a queue slot");
        assert!(raw.scrap >= UnitKind::Harvester.stats().cost);

        let orientation = brain
            .orientation
            .expect("the connected admission latched an orientation");
        assert!(orientation.is_identity());
        let oriented = orientation.observe(&raw);
        assert!(brain.mind().strategy.air_operation().is_some());
        assert_eq!(due.enqueued_at, state.current_tick());
        let operation = brain.mind().strategy.air_operation().unwrap();
        let reserved = operation
            .scout
            .into_iter()
            .chain(operation.artillery.iter().copied())
            .chain(operation.strike_aircraft.iter().copied())
            .collect::<Vec<_>>();
        assert!(!reserved.is_empty());
        let oriented_home = oriented
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the oriented observation retains the home Foundry")
            .anchor;
        let expected_returning =
            super::super::navigation::commands::routable_command_subset_with_public_terrain_and_orientation(crate::query_work::QueryPurpose::NavigationTest,
                &oriented,
                brain
                    .mind()
                    .oriented_public_map
                    .as_ref()
                    .expect("the admission oriented the public map"),
                &reserved,
                oriented_home,
                orientation,
            );
        assert_eq!(
            expected_returning, reserved,
            "the fixture keeps every reserved operation member routable to home"
        );

        let recovery = brain.act_traced(&state);
        let trace = recovery
            .trace
            .as_ref()
            .expect("the emergency decision is traced");
        assert_eq!(trace.control_flow, DecisionControlFlow::HarvesterRecovery);
        assert_eq!(
            recovery
                .commands
                .iter()
                .filter(|command| matches!(
                    command.command,
                    Command::Train { building, kind: UnitKind::Harvester }
                        if building == foundry
                ))
                .count(),
            1,
            "emergency recovery must emit exactly one affordable Harvester: {:?}",
            recovery.commands
        );
        assert!(
            recovery.commands.iter().all(|command| !matches!(
                command.command,
                Command::Train { building, kind }
                    if building == due.producer && kind == due.kind
            )),
            "the due connected provider must yield to the Harvester: {:?}",
            recovery.commands
        );
        let return_orders = recovery
            .commands
            .iter()
            .filter_map(|command| match &command.command {
                Command::Move { units, goal, queue }
                    if *goal == home
                        && !*queue
                        && units.iter().any(|unit| reserved.contains(unit)) =>
                {
                    Some(units.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            return_orders,
            vec![expected_returning],
            "every routable reserved member must receive one shared return-home order"
        );
        let operation = (brain.mind().strategy)
            .air_operation()
            .expect("the failed connected operation remains visible during recovery");
        assert_eq!(operation.phase(), AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason(),
            Some(super::super::strategy::AirRecoveryReason::PreparationInfeasible)
        );

        let report = state.tick(&recovery.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        while !state.current_tick().is_multiple_of(brain.dials.cadence) {
            state.tick(&[]);
        }
        let next = brain.act_traced(&state);
        assert!(
            next.commands.iter().all(|command| !matches!(
                command.command,
                Command::Train { building, kind }
                    if building == due.producer && kind == due.kind
            )),
            "the canceled provider must not be retried on the next cadence: {:?}",
            next.commands
        );
    }

    #[test]
    fn fresh_island_admission_accounts_for_an_active_lifts_airworks_prefix() {
        use super::super::trace::{ClaimOwnerTrace, LegacyChannelTrace, ObligationKeyTrace};

        let mut scenario = foundry_saving_lift_competition_scenario(50_000);
        scenario.name = "fresh island admission follows active lift production".into();
        let mut state = scenario
            .build()
            .expect("the active-lift island scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);
        let airworks = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0) && building.kind == BuildingKind::Airworks
            })
            .expect("the fixture has one Airworks")
            .id;
        let raw = Observation::fog_honest(&state, PlayerId(0));
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the home Foundry stands")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);
        let lift = seeded_bulk_lift(&state, orientation);

        let mut baseline_brain = foundry_competition_brain(&scenario);
        baseline_brain.dials.expansion = false;
        baseline_brain.dials.minimum_core_equivalents = 0;
        baseline_brain.orientation = Some(orientation);

        let baseline = baseline_brain.act_traced(&state);
        assert!(baseline.trace.as_ref().is_some_and(|trace| {
            trace.allocation.error.is_none() && trace.allocation.coordinator_failure.is_none()
        }));
        let baseline_timeout = (baseline_brain.mind().strategy)
            .air_assembly_timeout()
            .expect("the unprefixed control admits the wealthy-island operation");

        let mut brain = foundry_competition_brain(&scenario);
        brain.dials.expansion = false;
        brain.dials.minimum_core_equivalents = 0;
        brain.orientation = Some(orientation);
        assert!(brain.mind().strategy.air_operation().is_none());
        brain.mind_mut().lifts = lift;

        let act = brain.act_traced(&state);
        let trace = act
            .trace
            .as_ref()
            .expect("the active-lift island admission is traced");
        assert!(
            trace.allocation.error.is_none() && trace.allocation.coordinator_failure.is_none(),
            "the active lift must settle before the fresh island admission: {trace:#?}"
        );
        let accepted_lift_prefix = trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                job.enqueued_at == state.current_tick()
                    && matches!(
                        job.owner,
                        ClaimOwnerTrace::Obligation {
                            key: ObligationKeyTrace::Legacy {
                                channel: LegacyChannelTrace::Lift,
                                sequence: 1,
                            },
                            ..
                        }
                    )
            })
            .map(|job| (job.producer, job.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            accepted_lift_prefix,
            vec![(airworks, UnitKind::Skyhook)],
            "allocation must own the active lift's exact current producer prefix"
        );

        let airworks_training = act
            .commands
            .iter()
            .filter_map(|command| match command.command {
                Command::Train { building, kind } if building == airworks => Some(kind),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            airworks_training,
            vec![UnitKind::Skyhook, UnitKind::Buzzard],
            "the fresh island plan must see the accepted lift prefix and use only the remaining shallow slot"
        );
        assert!(
            (brain.mind().strategy)
                .air_operation()
                .is_some_and(|operation| {
                    operation.assault_admitted() && operation.phase() == AirOperationPhase::Recon
                })
        );
        let prefixed_timeout = (brain.mind().strategy)
            .air_assembly_timeout()
            .expect("the fresh island operation owns an assembly timeout");
        assert_eq!(
            prefixed_timeout,
            baseline_timeout.saturating_add(u64::from(UnitKind::Skyhook.stats().train_ticks)),
            "the island assembly window must cover the allocator-accepted FIFO prefix"
        );

        let report = state.tick(&act.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        assert_eq!(
            state
                .building(airworks)
                .expect("the shared Airworks remains live")
                .queue
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![UnitKind::Skyhook, UnitKind::Buzzard]
        );
    }

    #[test]
    fn fresh_island_admission_preserves_future_lift_work_on_the_only_airworks() {
        let mut scenario = foundry_saving_lift_competition_scenario(50_000);
        scenario.name = "fresh island admission preserves future lift production".into();
        let mut state = scenario
            .build()
            .expect("the future-lift island scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);
        let raw = Observation::fog_honest(&state, PlayerId(0));
        let airworks = raw
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Airworks)
            .expect("the fixture has one Airworks")
            .id;
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the home Foundry stands")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);
        let observed = orientation.observe(&raw);
        let public_map = orientation.briefing(
            &PublicMapBriefing::from_scenario(&scenario)
                .expect("the island fixture has a public briefing"),
        );
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observed);
        let resources = ResourceSnapshot::from_observation(&observed);
        let deadline = observed.tick.saturating_add(2_400);
        let projection = resources
            .planning_projection(deadline, 12)
            .expect("the future lift horizon is valid");
        let enqueue_at = observed.tick.saturating_add(12);
        let timing = projection
            .producer(airworks)
            .expect("the lift owns the sole Airworks")
            .clone()
            .append(UnitKind::Skyhook, enqueue_at)
            .expect("one future carrier fits the empty Airworks");
        let future_lift = ProducerLaneReservations::from_jobs(
            &projection,
            [ReservedProducerJob {
                producer: airworks,
                kind: UnitKind::Skyhook,
                enqueued_at: enqueue_at,
                starts_at: timing.starts_at,
                ready_at: timing.ready_at,
                ready_before: deadline,
            }],
        )
        .expect("the exact future lift schedule overlays the raw observation");
        let brain = foundry_competition_brain(&scenario);
        let profile = *brain.profile();
        let mut planner = StrategicPlanner::new();
        let result = planner.think_after_connected_adjudication(
            StrategicThinkContext::new(
                &profile,
                DifficultyTuning::for_level(profile.difficulty),
                &observed,
                &intelligence,
                home,
                StrategicCoordination {
                    planning: None,
                    enlisted: &[],
                    lift_support: None,
                    allow_new_operation: true,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                    public_map: Some(&public_map),
                    orientation,
                },
            )
            .with_producer_lanes(&[], &future_lift),
        );
        assert!(
            planner
                .air_operation()
                .is_some_and(|operation| operation.assault_admitted()),
            "the regression must exercise an admitted residual island operation"
        );
        assert!(
            result.decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::TrainAt { building, .. } if *building == airworks
            )),
            "an immediate island append must not move the accepted future carrier: {:?}",
            result.decision.intents
        );
    }

    #[test]
    fn fresh_island_admission_uses_an_airworks_disjoint_from_future_lift_work() {
        let mut scenario = foundry_saving_lift_competition_scenario(50_000);
        scenario.name = "fresh island admission uses a disjoint Airworks".into();
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Airworks,
            x: 14,
            y: 3,
        });
        let mut state = scenario
            .build()
            .expect("the disjoint-Airworks island scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);
        let raw = Observation::fog_honest(&state, PlayerId(0));
        let airworks = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Airworks)
            .map(|building| building.id)
            .collect::<Vec<_>>();
        assert_eq!(airworks.len(), 2);
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the home Foundry stands")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);
        let observed = orientation.observe(&raw);
        let public_map = orientation.briefing(
            &PublicMapBriefing::from_scenario(&scenario)
                .expect("the island fixture has a public briefing"),
        );
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observed);
        let resources = ResourceSnapshot::from_observation(&observed);
        let deadline = observed.tick.saturating_add(2_400);
        let projection = resources
            .planning_projection(deadline, 12)
            .expect("the future lift horizon is valid");
        let reserved = airworks[0];
        let enqueue_at = observed.tick.saturating_add(12);
        let timing = projection
            .producer(reserved)
            .expect("the lift owns one exact Airworks")
            .clone()
            .append(UnitKind::Skyhook, enqueue_at)
            .expect("one future carrier fits the reserved Airworks");
        let future_lift = ProducerLaneReservations::from_jobs(
            &projection,
            [ReservedProducerJob {
                producer: reserved,
                kind: UnitKind::Skyhook,
                enqueued_at: enqueue_at,
                starts_at: timing.starts_at,
                ready_at: timing.ready_at,
                ready_before: deadline,
            }],
        )
        .expect("the exact future lift schedule overlays the raw observation");
        let brain = foundry_competition_brain(&scenario);
        let profile = *brain.profile();
        let mut planner = StrategicPlanner::new();
        let result = planner.think_after_connected_adjudication(
            StrategicThinkContext::new(
                &profile,
                DifficultyTuning::for_level(profile.difficulty),
                &observed,
                &intelligence,
                home,
                StrategicCoordination {
                    planning: None,
                    enlisted: &[],
                    lift_support: None,
                    allow_new_operation: true,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                    public_map: Some(&public_map),
                    orientation,
                },
            )
            .with_producer_lanes(&[], &future_lift),
        );
        let residual_airwork = result
            .decision
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::TrainAt { building, kind }
                    if airworks.contains(building) && *kind != UnitKind::Skyhook =>
                {
                    Some((*building, *kind))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            !residual_airwork.is_empty(),
            "the island operation must use the compatible producer instead of being globally blocked: {:?}",
            result.decision.intents
        );
        assert!(
            residual_airwork
                .iter()
                .all(|(producer, _)| *producer != reserved),
            "residual island work must not shift the lift's exact future append: {residual_airwork:?}"
        );
    }

    #[test]
    fn active_lift_and_island_share_the_last_shallow_airworks_slot_without_starvation() {
        let mut scenario = foundry_saving_lift_competition_scenario(50_000);
        scenario.name = "active lift and island share one Airworks slot".into();
        let mut state = scenario
            .build()
            .expect("the shared-Airworks scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);
        let airworks = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0) && building.kind == BuildingKind::Airworks
            })
            .expect("the fixture has one Airworks")
            .id;
        crate::test_support::edit_building(&mut state, airworks, |building| {
            building.queue.push_back(UnitKind::Kestrel);
        });

        let mut brain = foundry_competition_brain(&scenario);
        brain.dials.expansion = false;
        brain.dials.minimum_core_equivalents = 0;
        let profile = *brain.profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let raw = Observation::fog_honest(&state, PlayerId(0));
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the home Foundry stands")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);
        let observed = orientation.observe(&raw);
        let public_map = orientation.briefing(
            &PublicMapBriefing::from_scenario(&scenario)
                .expect("the island fixture has a public briefing"),
        );
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observed);

        let mut strategy = StrategicPlanner::new();
        let island_admission =
            strategy.think_after_connected_adjudication(StrategicThinkContext::new(
                &profile,
                tuning,
                &observed,
                &intelligence,
                home,
                StrategicCoordination {
                    planning: None,
                    enlisted: &[],
                    lift_support: None,
                    allow_new_operation: true,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                    public_map: Some(&public_map),
                    orientation,
                },
            ));
        let island_train = island_admission
            .decision
            .intents
            .iter()
            .find_map(|intent| match intent {
                Intent::TrainAt { building, kind } if *building == airworks => Some(*kind),
                _ => None,
            })
            .expect("the admitted island operation needs the last shallow Airworks slot");
        assert!(strategy.air_operation().is_some_and(|operation| {
            operation.assault_admitted() && operation.phase() == AirOperationPhase::Recon
        }));
        assert!(strategy.connected_package_diagnostics().is_none());
        let island_airwork = strategy.remaining_airwork_ticks(&observed);
        assert!(island_airwork > 0);

        let mut lift = LiftPlanner::new();
        let lift_admission = lift.think_with_admission_and_producer_lanes(
            &observed,
            home,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: true,
                spendable_scrap: observed.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
            crate::resources::ProducerLaneReservations::empty(),
        );
        assert!(lift_admission.intents.contains(&Intent::TrainAt {
            building: airworks,
            kind: UnitKind::Skyhook,
        }));
        assert_eq!(
            strategy.air_admitted_at(),
            lift.operation().map(|operation| operation.started_at),
            "equal admission ticks exercise the historical island-before-lift tie-break"
        );

        let mind = brain.mind_mut();
        mind.intelligence = intelligence;
        mind.strategy = strategy;
        mind.lifts = lift;
        brain.orientation = Some(orientation);

        {
            use super::super::lift::{
                LiftProducerAssignment, LiftProducerFunding, LiftProducerTiming,
            };

            let mut cancelled = brain.clone();
            let operation = cancelled.mind_mut().lifts.operation().unwrap().clone();
            let starts_at = observed.tick + u64::from(UnitKind::Kestrel.stats().train_ticks);
            cancelled
                .mind_mut()
                .lifts
                .bind_producer_assignments(
                    operation.started_at,
                    operation.deadline,
                    vec![LiftProducerAssignment::new(
                        0,
                        airworks,
                        UnitKind::Skyhook,
                        LiftProducerTiming::new(
                            observed.tick + tuning.cadence,
                            starts_at,
                            starts_at + u64::from(UnitKind::Skyhook.stats().train_ticks) - 1,
                            operation.deadline,
                        ),
                        LiftProducerFunding::new(UnitKind::Skyhook.stats().cost, 0),
                    )],
                )
                .unwrap();
            assert!(
                cancelled
                    .mind_mut()
                    .lifts
                    .active_production_obligation()
                    .unwrap()
                    .producer_schedule_is_executable(
                        &ResourceSnapshot::from_observation(&observed),
                        tuning.cadence,
                        observed.tick,
                    )
            );
            let mut cancelled_state = state.clone();
            crate::test_support::edit_building(&mut cancelled_state, airworks, |building| {
                building.queue.clear();
            });
            let recovered = cancelled.act_traced(&cancelled_state);
            let trace = recovered.trace.unwrap();
            assert!(
                !trace.budget.as_ref().unwrap().frozen
                    && trace.allocation.coordinator_failure.is_none(),
                "cancelling a fixed booking's predecessor must not make earlier island staging roll back Lift recovery: {trace:#?}"
            );
            assert!(
                cancelled
                    .mind_mut()
                    .lifts
                    .active_production_obligation()
                    .is_none()
            );
        }

        let first = brain.act_traced(&state);
        let first_trace = first.trace.as_ref().expect("the shared turn is traced");
        assert!(
            first_trace.allocation.error.is_none()
                && first_trace.allocation.coordinator_failure.is_none(),
            "the two active planners must share producer capacity without rolling back: {first_trace:#?}"
        );
        assert_eq!(
            first
                .commands
                .iter()
                .filter(|command| matches!(
                    command.command,
                    Command::Train { building, kind }
                        if building == airworks && kind == island_train
                ))
                .count(),
            1,
            "the equal-tick priority winner owns the one immediately available slot: {:?}",
            first.commands
        );
        assert!(first.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train { building, kind }
                if building == airworks && kind == UnitKind::Skyhook
        )));
        let report = state.tick(&first.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));

        crate::test_support::edit_building(&mut state, airworks, |building| {
            assert_eq!(
                building.queue.iter().copied().collect::<Vec<_>>(),
                vec![UnitKind::Kestrel, island_train]
            );
        });
        let liveness_deadline = state
            .current_tick()
            .saturating_add(u64::from(UnitKind::Kestrel.stats().train_ticks))
            .saturating_add(island_airwork)
            .saturating_add(brain.dials.cadence.saturating_mul(2));
        let mut lift_progressed = false;
        while state.current_tick() <= liveness_deadline {
            let next = brain.act_traced(&state);
            if let Some(trace) = next.trace.as_ref() {
                assert!(
                    trace.allocation.error.is_none()
                        && trace.allocation.coordinator_failure.is_none(),
                    "shared Airworks pressure must not roll either active planner back: {trace:#?}"
                );
            }
            lift_progressed = next.commands.iter().any(|command| {
                matches!(
                    command.command,
                    Command::Train { building, kind }
                        if building == airworks && kind == UnitKind::Skyhook
                )
            });
            let report = state.tick(&next.commands);
            assert!(report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )));
            if lift_progressed {
                break;
            }
        }
        assert!(
            lift_progressed,
            "the lower-priority lift must claim Airworks capacity after the older island operation finishes its retained airwork"
        );
    }

    #[test]
    fn admitted_island_air_trains_before_a_fresh_foundry_without_being_thought_twice() {
        use super::super::trace::{
            ClaimOwnerTrace, LegacyChannelTrace, ObligationKeyTrace, ProducerJobAccessTrace,
        };

        let mut scenario = foundry_saving_air_competition_scenario(50_000);
        scenario.name = "admitted island air precedes a fresh Foundry".into();
        for row in scenario.map.iter_mut().skip(1).take(22) {
            let mut bytes = row.as_bytes().to_vec();
            bytes[38] = b'^';
            *row = String::from_utf8(bytes).expect("the fixture map remains ASCII");
        }
        let scout = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
            .expect("the island fixture has one scout");
        (scout.x, scout.y) = (42, 19);
        let mut state = scenario
            .build()
            .expect("the island allocation scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);

        let mut brain = foundry_competition_brain(&scenario);
        let profile = *brain.profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let raw = Observation::fog_honest(&state, PlayerId(0));
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the home Foundry stands")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);
        let observed = orientation.observe(&raw);
        let public_map = orientation.briefing(
            &PublicMapBriefing::from_scenario(&scenario)
                .expect("the island fixture has a public briefing"),
        );
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&observed);
        let mut planner = StrategicPlanner::new();
        let admission = planner.think_after_connected_adjudication(StrategicThinkContext::new(
            &profile,
            tuning,
            &observed,
            &intelligence,
            home,
            StrategicCoordination {
                planning: None,
                enlisted: &[],
                lift_support: None,
                allow_new_operation: true,
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
                public_map: Some(&public_map),
                orientation,
            },
        ));
        assert!(planner.air_operation().is_some_and(|operation| {
            operation.assault_admitted() && operation.phase() == AirOperationPhase::Recon
        }));
        assert!(planner.connected_package_diagnostics().is_none());
        assert!(admission.decision.intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Buzzard,
                ..
            }
        )));
        let admitted_at = planner
            .air_admitted_at()
            .expect("the island operation has an immutable priority tick");

        let decision_tick = super::super::difficulty::strategic_admission_at_or_after(
            admitted_at.saturating_add(tuning.reaction_delay),
        );
        crate::test_support::set_tick(&mut state, decision_tick);
        crate::test_support::edit_player(&mut state, PlayerId(0), |item| {
            item.scrap = UnitKind::Buzzard
                .stats()
                .cost
                .saturating_add(UnitKind::Sentinel.stats().cost)
        });
        let mind = brain.mind_mut();
        mind.intelligence = intelligence;
        mind.strategy = planner;
        brain.orientation = Some(orientation);

        let act = brain.act_traced(&state);
        let training = act
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Train {
                        kind: UnitKind::Buzzard,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            training, 1,
            "the due island append lowers exactly once: {:?}",
            act.commands
        );
        assert!(act.commands.iter().all(|command| !matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Foundry,
                ..
            }
        )));
        let operation = (brain.mind().strategy)
            .air_operation()
            .expect("the island operation remains active");
        assert_eq!(operation.phase(), AirOperationPhase::Assemble);
        assert_eq!(operation.phase_started_at, decision_tick);
        let operation_scout = operation
            .scout
            .expect("the admitted island operation retains its scout");

        let trace = act.trace.expect("the allocation decision is traced");
        assert!(trace.allocation.proposals.entries.iter().any(|proposal| {
            matches!(
                proposal.key,
                super::super::trace::ProposalKeyTrace::FoundryExpansion { .. }
            )
        }));
        let strategic_air = trace
            .allocation
            .obligations
            .entries
            .iter()
            .find(|obligation| {
                matches!(
                    obligation.key,
                    ObligationKeyTrace::Legacy {
                        channel: LegacyChannelTrace::StrategicAir,
                        sequence: 1,
                    }
                )
            })
            .expect("the due island decision is an explicit prior obligation");
        assert_eq!(strategic_air.accepted_at, admitted_at);
        let job = trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .find(|job| {
                matches!(
                    job.owner,
                    ClaimOwnerTrace::Obligation {
                        accepted_at,
                        key: ObligationKeyTrace::Legacy {
                            channel: LegacyChannelTrace::StrategicAir,
                            sequence: 1,
                        },
                        ..
                    } if accepted_at == admitted_at
                )
            })
            .expect("the island operation retains its exact producer assignment");
        assert_eq!(job.enqueued_at, decision_tick);
        assert!(matches!(
            strategic_air.claims.producer_jobs.entries[0].access,
            ProducerJobAccessTrace::Fixed { enqueued_at, .. } if enqueued_at == decision_tick
        ));

        let report = state.tick(&act.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));

        let mut after_scout_loss = Observation::fog_honest(&state, PlayerId(0));
        after_scout_loss
            .my_units
            .retain(|unit| unit.id != operation_scout);
        after_scout_loss.tick = decision_tick.saturating_add(brain.dials.cadence);
        let after_loss = super::Brain::act_traced(&mut brain, &after_scout_loss);
        let after_loss_trace = after_loss
            .trace
            .expect("the post-loss island decision is traced");
        assert!(
            after_loss_trace.allocation.error.is_none()
                && after_loss_trace.allocation.coordinator_failure.is_none(),
            "a vanished planner member must not become a permanent unknown-unit obligation: {after_loss_trace:#?}"
        );
        assert!(
            after_loss_trace.budget.is_some_and(|budget| !budget.frozen),
            "the remaining domains must keep making progress after the loss"
        );
        assert!(
            (brain.mind().strategy)
                .air_operation()
                .is_none_or(|operation| operation.scout != Some(operation_scout)),
            "the staged planner must release or replace the lost scout"
        );
    }

    #[test]
    fn recon_promoted_to_assault_keeps_its_original_foundry_priority_in_brain() {
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let mut scenario = foundry_saving_air_competition_scenario(foundry_cost - 1);
        scenario.buildings.extend((0..16).map(|index| BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: 16 + (index % 8) * 4,
            y: 2 + (index / 8) * 3,
        }));
        let mut state = scenario
            .build()
            .expect("the Foundry-saving air competition scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);
        let raw = Observation::fog_honest(&state, PlayerId(0));
        assert!(raw.enemy_buildings.is_empty());
        let enemy_foundry = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(1) && building.kind == BuildingKind::Foundry
            })
            .expect("the remembered enemy Foundry stands");
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the home Foundry stands")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);

        let mut brain = foundry_competition_brain(&scenario);
        brain.mind_mut().strategy = StrategicPlanner::new();

        brain.orientation = Some(orientation);
        let mut prior = raw.clone();
        prior.tick = prior.tick.saturating_sub(100);
        prior.enemy_buildings.push(BuildingObs {
            hp: enemy_foundry.hp,
            built: enemy_foundry.built,
            tier: enemy_foundry.tier,
            ..crate::test_support::building(
                enemy_foundry.id.0,
                enemy_foundry.player,
                enemy_foundry.kind,
                enemy_foundry.anchor,
            )
        });
        brain
            .mind_mut()
            .intelligence
            .update(&orientation.observe(&prior));

        let admitted = brain.act_traced(&state);
        let (admitted_at, initial_started_at) = {
            let planner = &brain.mind().strategy;
            let operation = planner
                .air_operation()
                .expect("the remembered objective admits reconnaissance");
            assert!(!operation.assault_admitted());
            (
                planner
                    .air_admitted_at()
                    .expect("the reconnaissance records its immutable admission"),
                operation.started_at,
            )
        };
        assert_eq!(admitted_at, state.current_tick());
        assert_eq!(initial_started_at, admitted_at);
        let oriented = orientation.observe(&raw);
        let saved = brain.policy.validated_foundry_saving(&oriented, true);
        let saved_site = brain
            .policy
            .foundry_builder_lease(&oriented)
            .expect("the saved expansion retains its builder and site")
            .anchor();
        assert!(saved > state.player(PlayerId(0)).scrap);
        assert!(brain.policy.operation_precedes_foundry_saving(admitted_at));
        assert_eq!(
            admitted
                .trace
                .expect("the reconnaissance admission is traced")
                .channels
                .connected_air
                .after,
            ChannelState::Active(ChannelPhase::AirRecon)
        );

        let mut visible_scenario = scenario.clone();
        let scout = visible_scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 0 && unit.kind == UnitKind::Kestrel)
            .expect("the continuation retains the reconnaissance aircraft");
        (scout.x, scout.y) = (42, 19);
        let mut visible_state = visible_scenario
            .build()
            .expect("the current-sight continuation builds");
        crate::test_support::set_tick(
            &mut visible_state,
            super::super::difficulty::next_strategic_admission_tick(state.current_tick()),
        );
        crate::test_support::edit_player(&mut visible_state, PlayerId(0), |item| {
            item.scrap = saved - 1
        });

        let promoted = brain.act_traced(&visible_state);
        let (preserved_admission, restarted_at) = {
            let planner = &brain.mind().strategy;
            let operation = planner
                .air_operation()
                .expect("current sight promotes the reconnaissance");
            assert!(
                operation.assault_admitted(),
                "current reconnaissance must promote through its retained allocation priority: {:?}",
                promoted.trace
            );
            (
                planner
                    .air_admitted_at()
                    .expect("the promoted operation retains its admission"),
                operation.started_at,
            )
        };
        assert_eq!(preserved_admission, admitted_at);
        assert_eq!(restarted_at, visible_state.current_tick());
        assert!(restarted_at > admitted_at);
        assert!(brain.policy.operation_precedes_foundry_saving(admitted_at));
        assert!(!brain.policy.operation_precedes_foundry_saving(restarted_at));
        assert!(promoted.trace.is_some());

        let report = visible_state.tick(&promoted.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        let next_admission =
            super::super::difficulty::next_strategic_admission_tick(visible_state.current_tick());
        while visible_state.current_tick() < next_admission {
            visible_state.tick(&[]);
        }
        crate::test_support::edit_player(&mut visible_state, PlayerId(0), |item| {
            item.scrap = saved - 1
        });
        let continued = brain.act_traced(&visible_state);
        let trace = continued
            .trace
            .expect("the post-promotion decision is traced");
        let budget = trace
            .budget
            .expect("the post-promotion decision records its budget");
        assert_eq!(budget.foundry_saving, saved);
        assert_eq!(budget.strategic_spendable, 0);
        assert!(trace.allocation.coordinator_failure.is_none());
        let connected_jobs = trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                matches!(
                    job.owner,
                    super::super::trace::ClaimOwnerTrace::Obligation {
                        accepted_at,
                        key: super::super::trace::ObligationKeyTrace::ConnectedOffense { .. },
                        ..
                    } if accepted_at == admitted_at
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            !connected_jobs.is_empty(),
            "the promoted operation must retain its procurement priority"
        );
        assert!(
            connected_jobs
                .iter()
                .all(|job| job.enqueued_at >= visible_state.current_tick())
        );
        let due_now = connected_jobs
            .iter()
            .filter(|job| job.enqueued_at == visible_state.current_tick())
            .cloned()
            .collect::<Vec<_>>();
        let future = connected_jobs
            .iter()
            .filter(|job| job.enqueued_at > visible_state.current_tick())
            .cloned()
            .collect::<Vec<_>>();
        let mut connected_pairs = connected_jobs
            .iter()
            .map(|job| (job.producer, job.kind))
            .collect::<Vec<_>>();
        connected_pairs.sort_unstable();
        connected_pairs.dedup();
        for (producer, kind) in connected_pairs {
            let expected = due_now
                .iter()
                .filter(|job| job.producer == producer && job.kind == kind)
                .count();
            let actual = continued
                .commands
                .iter()
                .filter(|command| {
                    matches!(
                        command.command,
                        Command::Train { building, kind: trained }
                            if building == producer && trained == kind
                    )
                })
                .count();
            assert_eq!(
                actual, expected,
                "only connected producer jobs due now may dispatch"
            );
        }

        let report = visible_state.tick(&continued.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        let Some(due_at) = future.iter().map(|job| job.enqueued_at).min() else {
            assert!(
                !due_now.is_empty(),
                "all retained demand was purchased this decision"
            );
            return;
        };
        let future_due = future
            .iter()
            .filter(|job| job.enqueued_at == due_at)
            .cloned()
            .collect::<Vec<_>>();
        while visible_state.current_tick() < due_at {
            visible_state.tick(&[]);
        }
        let due = brain.act_traced(&visible_state);
        let due_trace = due.trace.as_ref().expect("the future dispatch is traced");
        for expected in &future_due {
            assert!(
                due_trace
                    .allocation
                    .producer_schedule
                    .entries
                    .iter()
                    .any(|job| {
                        job.owner == expected.owner
                            && job.producer == expected.producer
                            && job.kind == expected.kind
                            && job.ready_before == expected.ready_before
                    }),
                "the retained force remains funded before its original deadline"
            );
        }
        let mut due_pairs = future_due
            .iter()
            .map(|job| (job.producer, job.kind))
            .collect::<Vec<_>>();
        due_pairs.sort_unstable();
        due_pairs.dedup();
        for (producer, kind) in due_pairs {
            let expected = due_trace
                .allocation
                .producer_schedule
                .entries
                .iter()
                .filter(|job| {
                    job.enqueued_at == visible_state.current_tick()
                        && job.producer == producer
                        && job.kind == kind
                })
                .count();
            let actual = due
                .commands
                .iter()
                .filter(|command| {
                    matches!(
                        command.command,
                        Command::Train { building, kind: trained }
                            if building == producer && trained == kind
                    )
                })
                .count();
            assert_eq!(
                actual, expected,
                "each current allocation row must emit once; earlier forecasts may change"
            );
        }
        let report = visible_state.tick(&due.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        let next_think = due_at.saturating_add(brain.dials.cadence);
        while visible_state.current_tick() < next_think {
            visible_state.tick(&[]);
        }
        let after_due = brain.act_traced(&visible_state);
        let after_due_trace = after_due
            .trace
            .expect("the post-dispatch continuation is traced");
        for issued in future_due {
            assert!(
                after_due_trace
                    .allocation
                    .producer_schedule
                    .entries
                    .iter()
                    .all(|job| {
                        job.owner != issued.owner
                            || job.request_ordinal != issued.request_ordinal
                            || job.producer != issued.producer
                            || job.kind != issued.kind
                            || job.enqueued_at != issued.enqueued_at
                            || job.starts_at != issued.starts_at
                            || job.ready_at != issued.ready_at
                            || job.ready_before != issued.ready_before
                    }),
                "an issued connected assignment must not re-enter allocation"
            );
        }
        let continued_raw =
            orientation.observe(&Observation::fog_honest(&visible_state, PlayerId(0)));
        let retained = brain.policy.validated_foundry_saving(&continued_raw, true);
        if retained == 0 {
            assert!(
                continued_raw
                    .my_buildings
                    .iter()
                    .any(|building| building.kind == BuildingKind::Foundry
                        && building.anchor == saved_site),
                "the original expansion may release its reserve only after paying for its exact foundation"
            );
        } else {
            assert_eq!(retained, saved);
        }
    }

    #[test]
    fn brain_funds_bulk_lifts_according_to_foundry_admission_order() {
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let shallow_guard = UnitKind::Sentinel.stats().cost;

        let mut earlier_scenario = foundry_saving_lift_competition_scenario(
            foundry_cost.saturating_add(shallow_guard).saturating_sub(1),
        );
        let last_carrier = earlier_scenario
            .units
            .iter()
            .rposition(|unit| unit.player == 0 && unit.kind == UnitKind::Skyhook)
            .expect("the lift fixture begins with multiple carriers");
        earlier_scenario.units.remove(last_carrier);
        earlier_scenario
            .buildings
            .extend((0..4).map(|index| BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 2 + index * 3,
                y: 6,
            }));
        let mut earlier_state = earlier_scenario
            .build()
            .expect("the earlier-lift competition scenario builds");
        let raw = Observation::fog_honest(&earlier_state, PlayerId(0));
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the earlier-lift fixture retains its home Foundry")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);
        assert!(orientation.is_identity());
        let lift = seeded_bulk_lift(&earlier_state, orientation);
        let lift_admitted_at = lift
            .operation()
            .expect("the earlier lift is active")
            .started_at;
        let mut earlier_brain = foundry_competition_brain(&earlier_scenario);
        earlier_brain.orientation = Some(orientation);

        earlier_brain.mind_mut().lifts = lift;

        let mut direct_earlier_brain = earlier_brain.clone();
        let direct_continued = direct_earlier_brain.act(&earlier_state);
        let continued = earlier_brain.act_traced(&earlier_state);
        assert_eq!(continued.commands, direct_continued);
        assert_brain_unchanged(&direct_earlier_brain, &earlier_brain);
        let accepted_raw = Observation::fog_honest(&earlier_state, PlayerId(0));
        let accepted_oriented = orientation.observe(&accepted_raw);
        let saved = earlier_brain
            .policy
            .validated_foundry_saving(&accepted_oriented, true);
        assert!(
            saved
                > earlier_state
                    .player(PlayerId(0))
                    .scrap
                    .saturating_sub(shallow_guard),
            "the earlier lift fixture must leave the accepted Foundry underfunded after preserving the shallow screen: saved={saved}, scrap={}, guard={shallow_guard}, trace={:?}",
            earlier_state.player(PlayerId(0)).scrap,
            continued.trace,
        );
        assert!(
            earlier_brain
                .policy
                .operation_precedes_foundry_saving(lift_admitted_at)
        );
        let continued_trace = continued
            .trace
            .expect("the older lift continuation is traced");
        assert!(
            continued_trace.allocation.error.is_none()
                && continued_trace.allocation.coordinator_failure.is_none()
        );
        assert!(
            continued_trace
                .allocation
                .proposals
                .entries
                .iter()
                .any(|proposal| matches!(
                    (proposal.key, &proposal.disposition,),
                    (
                        super::super::trace::ProposalKeyTrace::FoundryExpansion { .. },
                        super::super::trace::ProposalDispositionTrace::Accepted,
                    )
                ))
        );
        let lift_job = continued_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .find(|job| {
                job.kind == UnitKind::Skyhook
                    && matches!(
                        job.owner,
                        super::super::trace::ClaimOwnerTrace::Obligation {
                            accepted_at,
                        key: super::super::trace::ObligationKeyTrace::Legacy {
                            channel: super::super::trace::LegacyChannelTrace::Lift,
                            sequence: 2,
                        },
                            ..
                        } if accepted_at == lift_admitted_at
                    )
            })
            .expect("the older lift retains one exact future carrier")
            .clone();
        let lift_request_ordinal = usize::try_from(lift_job.request_ordinal)
            .expect("the traced Lift ordinal fits the planner's index space");
        let retained = (earlier_brain.mind().lifts)
            .operation()
            .expect("the Lift remains active")
            .producer_assignments
            .iter()
            .find(|assignment| assignment.request_ordinal() == lift_request_ordinal)
            .expect("the exact selected carrier schedule is retained by the Lift");
        assert_eq!(retained.producer(), lift_job.producer);
        assert_eq!(retained.kind(), lift_job.kind);
        assert_eq!(retained.timing().enqueued_at(), lift_job.enqueued_at);
        assert_eq!(retained.timing().starts_at(), lift_job.starts_at);
        assert_eq!(retained.timing().ready_at(), lift_job.ready_at);
        assert_eq!(retained.timing().ready_before(), lift_job.ready_before);
        assert!(lift_job.enqueued_at > earlier_state.current_tick());
        assert_eq!(
            lift_job
                .current_scrap
                .saturating_add(lift_job.forecast_scrap),
            UnitKind::Skyhook.stats().cost
        );
        let due_now = continued_trace
            .allocation
            .producer_schedule
            .entries
            .iter()
            .filter(|job| {
                job.producer == lift_job.producer
                    && job.kind == UnitKind::Skyhook
                    && job.enqueued_at == earlier_state.current_tick()
            })
            .count();
        let issued_now = continued
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command.command,
                    Command::Train {
                        building,
                        kind: UnitKind::Skyhook,
                    } if building == lift_job.producer
                )
            })
            .count();
        assert_eq!(due_now, 1, "the older lift retains one due carrier");
        assert_eq!(
            issued_now, due_now,
            "only producer work due on this observation may lower; the future carrier remains bound"
        );
        let report = earlier_state.tick(&continued.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        while earlier_state.current_tick() < lift_job.enqueued_at {
            earlier_state.tick(&[]);
        }
        let mut direct_due_brain = earlier_brain.clone();
        let direct_due = direct_due_brain.act(&earlier_state);
        let due = earlier_brain.act_traced(&earlier_state);
        assert_eq!(due.commands, direct_due);
        assert_brain_unchanged(&direct_due_brain, &earlier_brain);
        let due_trace = due.trace.as_ref().expect("the due carrier is traced");
        assert!(
            due_trace
                .allocation
                .producer_schedule
                .entries
                .iter()
                .any(|job| {
                    job.owner == lift_job.owner
                        && job.producer == lift_job.producer
                        && job.kind == lift_job.kind
                        && job.request_ordinal == lift_job.request_ordinal
                        && job.enqueued_at == lift_job.enqueued_at
                        && job.starts_at == lift_job.starts_at
                        && job.ready_at == lift_job.ready_at
                        && job.ready_before == lift_job.ready_before
                })
        );
        assert_eq!(
            due.commands
                .iter()
                .filter(|command| matches!(
                    command.command,
                    Command::Train {
                        building,
                        kind: UnitKind::Skyhook,
                    } if building == lift_job.producer
                ))
                .count(),
            1,
            "the older lift's exact carrier must dispatch once on its allocated tick"
        );
        assert_eq!(
            (earlier_brain.mind().lifts)
                .operation()
                .expect("the Lift remains active while its carrier trains")
                .issued_producers,
            vec![lift_request_ordinal],
        );
        let report = earlier_state.tick(&due.commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        let after_due_tick = lift_job
            .enqueued_at
            .saturating_add(earlier_brain.dials.cadence);
        while earlier_state.current_tick() < after_due_tick {
            earlier_state.tick(&[]);
        }
        let after_due = earlier_brain.act_traced(&earlier_state);
        assert!(after_due.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train {
                building,
                kind: UnitKind::Skyhook,
            } if building == lift_job.producer
        )));

        let later_scenario = foundry_saving_lift_competition_scenario(foundry_cost - 1);
        let mut later_state = later_scenario
            .build()
            .expect("the later-lift competition scenario builds");
        let mut later_brain = foundry_competition_brain(&later_scenario);

        let first_commands = later_brain.act(&later_state);
        let later_orientation = later_brain
            .orientation
            .expect("the saving-first think latches its orientation");
        let first_raw = Observation::fog_honest(&later_state, PlayerId(0));
        let first_oriented = later_orientation.observe(&first_raw);
        let later_saved = later_brain
            .policy
            .validated_foundry_saving(&first_oriented, true);
        assert!(later_saved > later_state.player(PlayerId(0)).scrap);
        let report = later_state.tick(&first_commands);
        assert!(report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        while !later_state
            .current_tick()
            .is_multiple_of(later_brain.dials.cadence)
            || !super::super::difficulty::strategic_admission_tick(later_state.current_tick())
        {
            later_state.tick(&[]);
        }
        crate::test_support::edit_player(&mut later_state, PlayerId(0), |item| {
            item.scrap = later_saved - 1
        });
        let later_lift = seeded_bulk_lift(&later_state, later_orientation);
        let later_lift_admitted_at = later_lift
            .operation()
            .expect("the later lift is active")
            .started_at;
        assert!(
            !later_brain
                .policy
                .operation_precedes_foundry_saving(later_lift_admitted_at)
        );
        later_brain.mind_mut().lifts = later_lift;
        later_brain.dials.expansion = false;

        let blocked = later_brain.act_traced(&later_state);
        let blocked_trace = blocked
            .trace
            .expect("the later lift continuation is traced");
        let blocked_budget = blocked_trace
            .budget
            .expect("the later lift continuation records its budget");
        assert_eq!(blocked_budget.foundry_saving, later_saved);
        assert_eq!(
            blocked_budget.prior_operation_spendable, 0,
            "a lift accepted after the Foundry saving receives no predecessor allowance"
        );
        assert_eq!(blocked_budget.strategic_spendable, 0);
        assert_eq!(blocked_trace.channels.lift.effects.committed_scrap, 0);
        assert!(blocked.commands.iter().all(|command| !matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Skyhook,
                ..
            }
        )));
        let blocked_raw = Observation::fog_honest(&later_state, PlayerId(0));
        assert_eq!(
            later_brain
                .policy
                .validated_foundry_saving(&later_orientation.observe(&blocked_raw), true),
            later_saved
        );
    }

    #[test]
    fn mirrored_brain_dispatches_and_releases_the_exact_foundry_lease() {
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        let scenario = mirrored_foundry_saving_scenario(foundry_cost - 1);
        let mut state = scenario
            .build()
            .expect("the mirrored Foundry-saving scenario builds");
        let mut brain = foundry_competition_brain(&scenario);

        let first_commands = brain.act(&state);
        let orientation = brain
            .orientation
            .expect("the southeast home latches an orientation");
        assert!(!orientation.is_identity());
        let raw = Observation::fog_honest(&state, PlayerId(0));
        let oriented = orientation.observe(&raw);
        let saved = brain.policy.validated_foundry_saving(&oriented, true);
        assert!(saved > state.player(PlayerId(0)).scrap);
        let lease = brain
            .policy
            .foundry_builder_lease(&oriented)
            .expect("the accepted expansion owns one exact canonical lease");
        let world_anchor = orientation.anchor(lease.anchor(), lease.kind().base_stats().size);
        assert_ne!(world_anchor, lease.anchor());

        let first_report = state.tick(&first_commands);
        assert!(first_report.events.iter().all(|event| !matches!(
            event,
            oxide_sim::event::Event::CommandRejected {
                player: PlayerId(0),
                ..
            }
        )));
        while !state.current_tick().is_multiple_of(brain.dials.cadence)
            || !super::super::difficulty::strategic_admission_tick(state.current_tick())
        {
            state.tick(&[]);
        }
        crate::test_support::edit_player(&mut state, PlayerId(0), |item| item.scrap = saved);

        let result = brain.act_traced(&state);
        let commands = result.commands;
        assert!(
            commands.iter().any(|command| matches!(
                &command.command,
                Command::Build {
                    units,
                    kind: BuildingKind::Foundry,
                    anchor,
                    ..
                } if units == &[lease.builder()] && *anchor == world_anchor
            )),
            "the canonical lease must lower to its exact mirrored world command: {commands:?}; trace={:?}",
            result.trace
        );
        let funded_raw = Observation::fog_honest(&state, PlayerId(0));
        let funded_oriented = orientation.observe(&funded_raw);
        assert_eq!(
            brain
                .policy
                .validated_foundry_saving(&funded_oriented, true),
            0,
            "dispatching the exact mirrored command clears the persistent saving"
        );
        assert!(
            brain
                .policy
                .foundry_builder_lease(&funded_oriented)
                .is_none()
        );

        let report = state.tick(&commands);
        assert!(
            report.events.iter().all(|event| !matches!(
                event,
                oxide_sim::event::Event::CommandRejected {
                    player: PlayerId(0),
                    ..
                }
            )),
            "the mirrored lease must be legal in authoritative world space: {:?}",
            report.events
        );
    }

    #[test]
    fn exact_prime_core_cannot_be_frozen_into_a_new_team_relief() {
        let scenario = opening_core_team_relief_scenario();
        let state = scenario
            .build()
            .expect("opening-core team-relief scenario builds");

        let mut control = operation_identity_brain(PlayerId(0), &scenario);

        control.mind_mut().profile.traits.support = 70;
        control.dials.minimum_core_equivalents = 0;
        control.act(&state);
        assert!(
            !control.mind().team.reservations().is_empty(),
            "the current allied emergency otherwise freezes an exact relief group"
        );

        let mut gated = operation_identity_brain(PlayerId(0), &scenario);

        gated.mind_mut().profile.traits.support = 70;
        gated.act(&state);

        let relief = &gated.mind().team;
        assert!(relief.operation().is_none());
        assert!(
            relief.reservations().is_empty(),
            "a new relief watch cannot reserve any of Prime's exact eight-unit core"
        );
    }

    #[test]
    fn exact_prime_core_cannot_be_frozen_into_a_new_lift_payload() {
        let scenario = opening_core_lift_scenario();
        let state = scenario.build().expect("opening-core lift scenario builds");

        let mut control = operation_identity_brain(PlayerId(0), &scenario);

        control.dials.minimum_core_equivalents = 0;
        control.act(&state);
        assert!(
            (control.mind().lifts).operation().is_some(),
            "the visible disconnected objective otherwise admits a lift"
        );

        let mut gated = operation_identity_brain(PlayerId(0), &scenario);

        let commands = gated.act(&state);

        assert!(
            (gated.mind().lifts).operation().is_none(),
            "a new lift cannot freeze Prime's exact eight-unit core as payload"
        );
        assert!(commands.iter().all(|command| !matches!(
            command.command,
            Command::Train {
                kind: UnitKind::Skyhook,
                ..
            }
        )));
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
        enlist_opening_core(&mut brain, &state);
        let profile = *brain.profile();
        let obs = Observation::fog_honest(&state, PlayerId(0));
        let home = obs
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Foundry)
            .expect("the home Foundry stands")
            .anchor;
        let mut prior_raid = RaidPlanner::new();
        prior_raid.think_unrestricted(
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
        brain.mind_mut().raids = prior_raid;

        brain.act(&state);

        let lift = (brain.mind().lifts)
            .operation()
            .expect("the independent bulk lift also forms");
        assert!(lift.desired_carriers >= 8);
        assert!(
            prior_members
                .iter()
                .all(|member| !lift.payload.contains(member)),
            "a new lift cannot steal members from a raid already under way"
        );
    }

    #[test]
    fn conflicting_active_legacy_planners_roll_back_as_one_allocation_session() {
        let mut scenario = opening_core_team_relief_scenario();
        scenario.name = "conflicting active legacy planner rollback".into();
        for row in scenario.map.iter_mut().skip(1).take(22) {
            let mut bytes = row.as_bytes().to_vec();
            bytes[20] = b'~';
            *row = String::from_utf8(bytes).expect("the fixture map remains ASCII");
        }
        let mut marker_row = scenario.map[10].as_bytes().to_vec();
        marker_row[24] = b'.';
        marker_row[15] = b'2';
        scenario.map[10] = String::from_utf8(marker_row).expect("the fixture map remains ASCII");
        let pressure = scenario
            .units
            .iter_mut()
            .find(|unit| unit.player == 2 && unit.kind == UnitKind::Sentinel)
            .expect("the team fixture has one pressure unit");
        (pressure.x, pressure.y) = (17, 10);
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Airworks,
            x: 6,
            y: 3,
        });
        scenario.units.extend((0..4).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Skyhook,
            x: 4 + index,
            y: 18,
        }));
        scenario.units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: 32,
            y: 10,
        });
        let mut state = scenario
            .build()
            .expect("the conflicting-operation scenario builds");
        crate::test_support::set_tick(&mut state, 6_000);

        let mut brain = operation_identity_brain(PlayerId(0), &scenario);
        brain.dials.minimum_core_equivalents = 0;

        let mut profile = *brain.profile();
        profile.traits.support = 70;
        profile.traits.fortification = 65;
        brain.mind_mut().profile = profile;
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let obs = Observation::fog_honest(&state, PlayerId(0));
        let home = obs
            .my_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Foundry)
            .expect("the home Foundry stands")
            .anchor;

        let mut team = TeamReliefPlanner::new();
        let _ = team.think_unrestricted(&profile, tuning, &obs, home, &[], &[]);
        assert!(
            !team.reservations().is_empty(),
            "the first current-pressure observation must freeze a credible relief watch: allies={:?}, enemies={:?}",
            obs.ally_buildings,
            obs.enemy_units
        );
        let mut admitted_obs = obs.clone();
        admitted_obs.tick = super::super::difficulty::strategic_admission_at_or_after(
            admitted_obs
                .tick
                .saturating_add(u64::from(oxide_sim::TICKS_PER_SECOND))
                .saturating_add(tuning.reaction_delay),
        );
        crate::test_support::set_tick(&mut state, admitted_obs.tick);
        let _ = team.think_unrestricted(&profile, tuning, &admitted_obs, home, &[], &[]);
        let team_members = team
            .operation()
            .expect("sustained allied pressure admits a relief")
            .members
            .clone();
        let mut lift = LiftPlanner::new();
        let _ = lift.think_with_admission_and_producer_lanes(
            &admitted_obs,
            home,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: true,
                spendable_scrap: admitted_obs.scrap,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
            crate::resources::ProducerLaneReservations::empty(),
        );
        let lift_payload = lift
            .operation()
            .expect("the disconnected enemy Foundry admits a lift")
            .payload
            .to_vec();
        assert!(
            team_members
                .iter()
                .any(|member| lift_payload.binary_search(member).is_ok()),
            "independent legacy admissions must intentionally claim at least one common unit"
        );

        let ally_foundry = admitted_obs
            .ally_buildings
            .iter()
            .find(|building| building.kind == BuildingKind::Foundry)
            .expect("the relief target stands before the conflicting turn")
            .id;
        let mut failed_turn_obs = Observation::fog_honest(&state, PlayerId(0));
        failed_turn_obs
            .ally_buildings
            .retain(|building| building.id != ally_foundry);
        let mut independently_advanced_team = team.clone();
        let _ = independently_advanced_team.think_with_admission(
            &profile,
            tuning,
            &failed_turn_obs,
            home,
            &[],
            TeamReliefAdmission {
                additionally_reserved: &[],
                allow_new_operation: false,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        );
        assert_ne!(
            independently_advanced_team, team,
            "the failed turn must exercise rollback of a real planner transition"
        );

        let team_before = team.clone();
        let lift_before = lift.clone();
        brain.mind_mut().team = team;
        brain.mind_mut().lifts = lift;

        let result = super::Brain::act_traced(&mut brain, &failed_turn_obs);
        let trace = result
            .trace
            .expect("the conflicting allocation boundary is traced");
        assert!(matches!(
            trace.allocation.error,
            Some(super::super::trace::AllocationErrorTrace::ObligationConflict { .. })
        ));
        assert!(
            trace
                .budget
                .is_some_and(|budget| { budget.frozen && budget.utility_spendable == 0 }),
            "a malformed shared session must not reopen the current bank to residual utility"
        );
        let mut restored_team = brain.mind().team.clone();
        assert_eq!(restored_team.outcomes, independently_advanced_team.outcomes);
        restored_team.outcomes = team_before.outcomes.clone();
        assert_eq!(restored_team, team_before);
        let mut restored_lift = brain.mind().lifts.clone();
        assert!(restored_lift.outcomes.pending.is_empty());
        restored_lift.outcomes = lift_before.outcomes.clone();
        assert_eq!(restored_lift, lift_before);
        assert!(result.commands.iter().all(|command| !matches!(
            command.command,
            Command::Build { .. } | Command::Train { .. }
        )));
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
        let profile = *brain.profile();
        let tuning = DifficultyTuning::for_level(profile.difficulty);
        let raids = &mut brain.mind_mut().raids;
        let partial = raids.think_unrestricted(&profile, tuning, &obs, TEST_HOME, &[], &[]);
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
        let complete = brain.mind_mut().raids.think_unrestricted(
            &profile,
            tuning,
            &obs,
            TEST_HOME,
            &enlisted,
            &[],
        );
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
        let unreserved = policy_probe.think_residual(
            brain.dials(),
            &obs,
            std::slice::from_ref(&army),
            &enlisted,
            &[],
            &brain.mind().public_map,
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
            let result = brain.act_traced(&state);
            let commands = result.commands;
            let operation = (brain
                .mind()
                .lifts).operation()
                .unwrap_or_else(|| {
                    panic!(
                        "the severed enemy Foundry freezes a lift payload on think {think}: commands={commands:?}; trace={:?}",
                        result.trace
                    )
                });
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
            let mut strategic_claims = operation.payload.to_vec();
            if let Some(air) = (brain.mind().strategy).air_operation() {
                strategic_claims.extend(air.scout);
                strategic_claims.extend(air.artillery.iter().copied());
                strategic_claims.extend(air.strike_aircraft.iter().copied());
            }
            if let Some(relief) = (brain.mind().team).operation() {
                strategic_claims.extend(relief.members.iter().copied());
            }
            if let Some(raid) = (brain.mind().raids).operation() {
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
            let mission = brain
                .exec
                .missions
                .get(&army.id)
                .expect("the unreserved body receives a defense responsibility");
            assert!(matches!(
                mission.purpose,
                super::super::executive::ArmyPurpose::Defend(_)
            ));
            assert!(
                mission.goal.chebyshev(TilePos::new(5, 6)) <= 8,
                "the defensive service point must answer the visible incursion"
            );
            if think == 0 {
                assert!(
                    commands.iter().any(|command| matches!(
                        &command.command,
                        Command::AttackMove {
                            units,
                            goal,
                            queue: false,
                        } if units == &available && *goal == mission.goal
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
            assert_eq!(staged.target, Some(mission.goal));

            state.tick(&[]);
        }
    }

    #[test]
    fn terminal_air_outcomes_reach_a_boarding_complete_lift_in_the_same_think() {
        let (obs, planner, manifest) = boarding_complete_lift();

        let mut released = planner.clone();
        let released_decision = released.think_unrestricted(
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
        let aborted_decision = aborted.think_unrestricted(
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

    fn opening_core_lift_scenario() -> Scenario {
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.name = "brain opening-core lift admission".into();
        scenario.players[0].scrap = 50_000;
        let mut kept_sentinels = 0usize;
        scenario.units.retain(|unit| {
            if unit.kind != UnitKind::Sentinel {
                return true;
            }
            kept_sentinels += 1;
            kept_sentinels <= 8
        });
        scenario
    }

    fn opening_core_team_relief_scenario() -> Scenario {
        let mut rows = vec![vec!['.'; 40]; 24];
        rows.first_mut().expect("map has a north edge").fill('#');
        rows.last_mut().expect("map has a south edge").fill('#');
        for row in &mut rows {
            row[0] = '#';
            row[39] = '#';
        }
        rows[10][3] = '1';
        rows[10][24] = '2';
        rows[10][36] = '3';

        Scenario {
            name: "brain opening-core team-relief admission".into(),
            seed: 0x0A16_7EA1,
            map: rows
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect(),
            players: vec![
                PlayerSpec {
                    name: "Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: Some(0),
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric ally".into(),
                    faction: Faction::Cupric,
                    team: Some(0),
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric enemy".into(),
                    faction: Faction::Cupric,
                    team: Some(1),
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: core::iter::once(UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 8,
                y: 12,
            })
            .chain((0..8).map(|index| UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 3 + index % 4,
                y: 15 + index / 4,
            }))
            .chain(core::iter::once(UnitSpec {
                player: 2,
                kind: UnitKind::Sentinel,
                x: 28,
                y: 10,
            }))
            .collect(),
            buildings: Vec::new(),
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
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Harvester,
            x: 18,
            y: 20,
        });
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

    fn brain_with_remembered_lift_target(
        scenario: &Scenario,
        state: &State,
        last_seen: u64,
    ) -> Brain {
        let raw = Observation::fog_honest(state, PlayerId(0));
        let home = raw
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the home Foundry stands")
            .anchor;
        let orientation = Orientation::for_home(&raw, home);
        let mut brain = scripted_brain(
            scenario,
            PlayerId(0),
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_024),
        );
        brain.orientation = Some(orientation);
        let enemy_foundry = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(1) && building.kind == BuildingKind::Foundry
            })
            .expect("the enemy Foundry stands beyond the barrier");
        let mut prior = raw;
        prior.tick = last_seen;
        prior.enemy_buildings.push(BuildingObs {
            hp: enemy_foundry.hp,
            built: enemy_foundry.built,
            tier: enemy_foundry.tier,
            ..crate::test_support::building(
                enemy_foundry.id.0,
                enemy_foundry.player,
                enemy_foundry.kind,
                enemy_foundry.anchor,
            )
        });
        brain
            .mind_mut()
            .intelligence
            .update(&orientation.observe(&prior));
        brain
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
        scenario.units.extend((0..8).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 13 + index % 4,
            y: 21 + index / 4,
        }));
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Extractor,
            x: 14,
            y: 6,
        });
        scenario
    }

    fn foundry_saving_air_competition_scenario(scrap: u32) -> Scenario {
        let width = 48usize;
        let height = 24usize;
        let mut rows = vec![vec!['.'; width]; height];
        rows.first_mut().expect("map has a north edge").fill('#');
        rows.last_mut().expect("map has a south edge").fill('#');
        for row in &mut rows {
            row[0] = '#';
            row[width - 1] = '#';
        }
        rows[1][1] = '1';
        rows[20][44] = '2';
        rows[16][30] = 'E';

        let mut units: Vec<_> = (0..4)
            .map(|index| UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 5 + index,
                y: 11,
            })
            .collect();
        units.extend(
            [
                (4, 4),
                (7, 9),
                (10, 10),
                (13, 11),
                (16, 12),
                (19, 13),
                (22, 14),
                (25, 15),
                (27, 17),
                (28, 14),
                (24, 12),
                (20, 10),
            ]
            .into_iter()
            .map(|(x, y)| UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x,
                y,
            }),
        );
        units.push(UnitSpec {
            player: 0,
            kind: UnitKind::Kestrel,
            x: 9,
            y: 18,
        });

        Scenario {
            name: "accepted Foundry saving competes with connected air".into(),
            seed: 1_616_305,
            map: rows
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect(),
            players: vec![
                PlayerSpec {
                    name: "West Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap,
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
            units,
            buildings: vec![
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Fabricator,
                    x: 5,
                    y: 2,
                },
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Airworks,
                    x: 1,
                    y: 6,
                },
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Crucible,
                    x: 5,
                    y: 6,
                },
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Foundry,
                    x: 11,
                    y: 1,
                },
                BuildingSpec {
                    player: 0,
                    kind: BuildingKind::Extractor,
                    x: 30,
                    y: 16,
                },
            ],
            meta: None,
        }
    }

    fn foundry_competition_brain(scenario: &Scenario) -> Brain {
        use super::super::profile::PersonalityTraits;

        let profile = ResolvedProfile {
            difficulty: BotDifficulty::Standard,
            stance: BotStance::Balanced,
            personality_seed: 1_616_304,
            primary: Specialty::Air,
            secondary: Specialty::Siege,
            traits: PersonalityTraits {
                air: 70,
                siege: 60,
                support: 35,
                fortification: 35,
                greed: 64,
                guile: 36,
            },
        };
        let mut brain = scripted_brain(
            scenario,
            PlayerId(0),
            BotConfig::scripted(profile.difficulty, profile.stance, profile.personality_seed),
        );
        brain.dials = Dials::scripted(&profile, DifficultyTuning::for_level(profile.difficulty));
        brain.dials.harvester_target = 4;
        brain.dials.army_size = 100;
        brain.dials.scouting = false;
        brain.dials.extractors = false;
        brain.dials.upgrades = false;
        let mind = brain.mind_mut();
        mind.profile = profile;
        brain
    }

    fn foundry_saving_lift_competition_scenario(scrap: u32) -> Scenario {
        let mut scenario = bulk_lift_capacity_scenario();
        scenario.name = "accepted Foundry saving competes with a bulk lift".into();
        scenario.players[0].scrap = scrap;
        let frame = TilePos::new(14, 11);
        let row = scenario
            .map
            .get_mut(frame.y as usize)
            .expect("the lift fixture contains the frame row");
        let mut bytes = row.as_bytes().to_vec();
        bytes[frame.x as usize] = b'E';
        *row = String::from_utf8(bytes).expect("the lift fixture map remains ASCII");
        scenario.buildings.push(BuildingSpec {
            player: 0,
            kind: BuildingKind::Extractor,
            x: frame.x,
            y: frame.y,
        });
        scenario.units.extend((0..7).map(|index| UnitSpec {
            player: 0,
            kind: UnitKind::Skyhook,
            x: 3 + index,
            y: 20,
        }));
        scenario
    }

    fn seeded_bulk_lift(state: &State, orientation: Orientation) -> LiftPlanner {
        let raw = Observation::fog_honest(state, PlayerId(0));
        let oriented = orientation.observe(&raw);
        let home = oriented
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| building.id)
            .expect("the lift fixture retains its home Foundry")
            .anchor;
        let mut planner = LiftPlanner::new();
        let decision =
            planner.think_unrestricted(&oriented, home, &[], LiftAirSupport::Independent);
        assert!(
            planner.operation().is_some(),
            "the bulk lift must be admissible"
        );
        assert!(
            decision.intents.iter().any(|intent| matches!(
                intent,
                Intent::TrainAt {
                    kind: UnitKind::Skyhook,
                    ..
                }
            )),
            "the seeded lift must still need one carrier: operation={:?}, intents={:?}",
            planner.operation(),
            decision.intents
        );
        planner
    }

    fn mirrored_foundry_saving_scenario(scrap: u32) -> Scenario {
        let mut scenario = foundry_saving_air_competition_scenario(scrap);
        scenario.name = "mirrored Foundry saving dispatch".into();
        let width = i32::try_from(scenario.map[0].len()).expect("fixture width fits i32");
        let height = i32::try_from(scenario.map.len()).expect("fixture height fits i32");
        for row in &mut scenario.map {
            *row = row
                .chars()
                .map(|tile| {
                    if matches!(tile, '1' | '2' | 'E') {
                        '.'
                    } else {
                        tile
                    }
                })
                .collect();
        }
        for (anchor, marker) in [
            (TilePos::new(44, 20), b'1'),
            (TilePos::new(1, 1), b'2'),
            (TilePos::new(16, 6), b'E'),
        ] {
            let row = scenario
                .map
                .get_mut(anchor.y as usize)
                .expect("the mirrored marker row exists");
            let mut bytes = row.as_bytes().to_vec();
            bytes[anchor.x as usize] = marker;
            *row = String::from_utf8(bytes).expect("the mirrored fixture map remains ASCII");
        }
        for unit in &mut scenario.units {
            unit.x = width - 1 - unit.x;
            unit.y = height - 1 - unit.y;
        }
        for building in &mut scenario.buildings {
            let size = building.kind.base_stats().size;
            building.x = width - size.0 - building.x;
            building.y = height - size.1 - building.y;
        }
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
        planner.think_unrestricted(&obs, TEST_HOME, &[], LiftAirSupport::Independent);
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
        planner.think_unrestricted(&obs, TEST_HOME, &[], LiftAirSupport::Independent);
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
        let mut obs = Observation::from_data(ObservationData {
            tick: 0,
            map_width: 64,
            map_height: 32,
            enemy_buildings: vec![test_building(500, 1, BuildingKind::Foundry, TEST_TARGET)],
            visible: vec![true; 64 * 32],
            explored: vec![true; 64 * 32],
            known_rock: (0..32).map(|y| TilePos::new(32, y)).collect(),
            ..crate::test_support::observation_data()
        });
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
        crate::test_support::unit(id, PlayerId(0), kind, tile)
    }

    fn test_building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        crate::test_support::building(id, PlayerId(player), kind, anchor)
    }
}
