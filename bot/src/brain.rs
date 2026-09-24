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
use super::difficulty::DifficultyTuning;
use super::executive::Executive;
use super::intelligence::StrategicIntelligence;
use super::lift::LiftPlanner;
use super::observation::Observation;
use super::observer::{BotPhase, PhaseObserver, PhaseScope};
use super::orient::Orientation;
use super::profile::ResolvedProfile;
use super::raid::RaidPlanner;

use super::resources::BuilderLease;
use super::strategy::StrategicPlanner;
use super::team::TeamReliefPlanner;
use super::trace::{
    DecisionControlFlow, DecisionTraceRecorder, LoweringTrace, TracedBotAct, UtilityTrace,
    bounded_count,
};
use super::utility::{Dials, UtilityPolicy};
use chassis::grid::TilePos;
use oxide_sim::command::PlayerCommand;
use oxide_sim::ids::PlayerId;
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
        assert_eq!(
            (
                self.mind.public_map.map_width(),
                self.mind.public_map.map_height()
            ),
            (obs.map_width, obs.map_height),
            "the controller's briefing must describe the observed map"
        );
        let _query_capture = super::query_work::Capture::new(observer);
        let maintenance_scope = PhaseScope::new(observer, BotPhase::Maintenance);
        // The wounded rear line lives on the home-side corner of the Foundry:
        // behind everything, and every march home routes past friendly
        // production. A footprint anchor is not itself a symmetric point goal
        // on an even-sized building, so select the same corner in each seat's
        // oriented frame before the raw executive acts.
        let Some((rear_anchor, rear_size)) = obs
            .my_buildings
            .iter()
            .filter(|b| !b.provisional && b.kind == oxide_sim::stats::BuildingKind::Foundry)
            .min_by_key(|b| b.id)
            .map(|b| (b.anchor, b.kind.base_stats().size))
        else {
            return Vec::new();
        };
        self.policy.planning.begin(obs.tick);
        if let Some(recorder) = recorder.as_deref_mut() {
            recorder.begin(obs);
        }
        let orientation = *self
            .orientation
            .get_or_insert_with(|| Orientation::for_home(obs, rear_anchor));
        let oriented_home = orientation.anchor(rear_anchor, rear_size);
        let rear = player_facing_rear_tile(orientation, rear_anchor, rear_size);
        self.exec.mission_decisions.clear();
        let tuning = DifficultyTuning::for_level(self.mind.profile.difficulty);
        let mut commands = self.exec.maintain_player_facing_with_tactics(
            self.player,
            obs,
            rear,
            tuning.coordinated_focus,
            tuning.coordinated_defense_focus,
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
            for mut report in self.exec.take_ground_reports() {
                orientation.episode(&mut report);
                mind.experience.report(report);
            }
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
            if let Some(recorder) = recorder.as_deref_mut() {
                recorder.trace_mut().battlefield = Some(mind.battlefield.assessment().clone());
                recorder.trace_mut().experience = mind.experience.trace();
                recorder.trace_mut().missions = self
                    .exec
                    .armies()
                    .iter()
                    .filter_map(|army| army.mission.clone().map(|mission| (army.id.0, mission)))
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
                strategy.recover_unpaid_connected_for_economy_emergency(
                    super::strategy::EconomyEmergencyRecovery {
                        profile,
                        tuning: DifficultyTuning::for_level(profile.difficulty),
                        obs: &oriented,
                        home: oriented_home,
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
        let ground_inputs = super::utility::GroundMissionInputs {
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

        self.exec.link_ground_handoff(&lifts.outcomes);

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
mod tests;
