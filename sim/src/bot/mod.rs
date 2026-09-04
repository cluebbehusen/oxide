//! Bots: command sources that read [`crate::State`] and emit
//! [`crate::PlayerCommand`]s, exactly like a mouse or the debug socket.
//!
//! The player-facing controller is layered:
//!
//! ```text
//! immutable PublicMapBriefing + fog-honest Observation
//!   -> oriented public priors + StrategicIntelligence
//!   -> persistent playbooks + UtilityPolicy Intent
//!   -> Executive
//!   -> PlayerCommand[]
//! ```
//!
//! [`observation`] builds what a bot may know; [`StrategicIntelligence`]
//! distinguishes current evidence from memory; persistent planners and
//! [`UtilityPolicy`] produce [`Intent`]s; and [`Executive`] owns exact unit
//! reservations and lowers those intents to commands. [`Brain::scripted`] is
//! the configurable player-facing opponent. [`Brain::overseer`] remains a
//! separate QA yardstick.

mod allocation;
pub mod brain;
pub mod briefing;
pub mod difficulty;
pub mod executive;
pub mod intelligence;
pub mod lift;
pub mod observation;
pub mod orient;
pub mod profile;
pub mod raid;
mod residual_coordination;
mod resources;
mod routing;
pub mod strategy;
pub mod team;
pub mod trace;
pub mod utility;

pub use brain::Brain;
pub use briefing::{PublicMapBriefing, StartingFoundry};
pub use difficulty::DifficultyTuning;
pub use executive::{Army, ArmyId, ArmyState, Executive, Intent};
pub use intelligence::{
    AirDefenseAssessment, AirDefenseContact, AirDefenseEvidence, AirDefenseSource, BuildingContact,
    ContactEvidence, StrategicIntelligence, UnitContact,
};
pub use lift::{LiftAirSupport, LiftManifest, LiftOperation, LiftPhase, LiftPlanner};
pub use observation::{BuildingObs, Observation, UnitObs};
pub use orient::Orientation;
pub use profile::{PersonalityTraits, ResolvedProfile, Specialty};
pub use raid::{RaidExitReason, RaidObjective, RaidOperation, RaidPhase, RaidPlanner};
use std::sync::Arc;
pub use strategy::{
    AirOperation, AirOperationPhase, AirRecoveryReason, StrategicDecision, StrategicPlanner,
};
pub use team::{TeamReliefExitReason, TeamReliefOperation, TeamReliefPhase, TeamReliefPlanner};
pub use trace::{
    AssignedForceTrace, CapabilityTrace, ChannelEffects, ChannelPhase, ChannelState, ChannelTrace,
    ChannelTraces, ConnectedForceStatus, ConnectedForceTrace, ConnectedPackageTrace,
    ConnectedRecoveryReasonTrace, ConnectedRejectionReasonTrace, ConnectedTargetTrace,
    CoreGateTrace, DECISION_TRACE_VERSION, DecisionControlFlow, DecisionTrace, EvidenceTrace,
    ForceDemandsTrace, ForceFamilyTrace, GateTrace, LoweringTrace, ProviderDemandTrace,
    RaidAttentionTrace, RejectedConnectedCandidateTrace, ScrapBudgetTrace, TargetEvidenceTrace,
    TracedBotAct, UtilityTrace,
};
pub use utility::{Dials, UtilityPolicy};

/// A bot seat as the shell and driver run it.
#[derive(Debug, Clone)]
pub struct SeatBot(Box<Brain>);

impl SeatBot {
    /// Creates a seat running the configurable player-facing controller.
    pub fn scripted(
        player: crate::ids::PlayerId,
        config: crate::scenario::BotConfig,
        public_map: Arc<PublicMapBriefing>,
    ) -> Self {
        Self(Box::new(Brain::scripted(player, config, public_map)))
    }

    /// Creates a seat running the frozen Overseer QA controller.
    pub fn overseer(player: crate::ids::PlayerId, scenario_seed: u64) -> Self {
        Self(Box::new(Brain::overseer(player, scenario_seed)))
    }

    /// Creates the frozen QA controller with a policy identity that stays
    /// fixed when an evaluation exchanges physical seats.
    pub fn overseer_with_policy_seed(player: crate::ids::PlayerId, policy_seed: u64) -> Self {
        Self(Box::new(Brain::overseer_with_policy_seed(
            player,
            policy_seed,
        )))
    }

    /// Commands for this tick.
    pub fn act(&mut self, state: &crate::state::State) -> Vec<crate::command::PlayerCommand> {
        self.0.act(state)
    }

    /// Commands plus an opt-in player-facing decision trace for this tick.
    pub fn act_traced(&mut self, state: &crate::state::State) -> TracedBotAct {
        self.0.act_traced(state)
    }

    /// The player this bot drives.
    pub fn player(&self) -> crate::ids::PlayerId {
        self.0.player()
    }
}

/// Every bot a scenario asks for, honoring each seat's `bot_config`.
///
/// A configured seat receives the fair rules-based opponent. A `bot`
/// seat without a config remains an empty chair rather than silently
/// selecting a controller.
pub fn seat_bots(
    scenario: &crate::Scenario,
) -> Result<Vec<SeatBot>, crate::scenario::ScenarioError> {
    let public_map = Arc::new(PublicMapBriefing::from_scenario(scenario)?);
    Ok(scenario
        .players
        .iter()
        .enumerate()
        .filter(|(_, p)| p.bot)
        .filter_map(|(i, p)| {
            let player = crate::ids::PlayerId(i as u8);
            p.bot_config
                .map(|config| SeatBot::scripted(player, config, Arc::clone(&public_map)))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
    use crate::{PlayerId, Scenario};

    #[test]
    fn scripted_constructor_matches_the_direct_brain() {
        let scenario = Scenario::skirmish();
        let state = scenario.build().expect("the skirmish builds");
        let public_map = Arc::new(
            PublicMapBriefing::from_scenario(&scenario).expect("the skirmish has a briefing"),
        );
        let player = PlayerId(1);
        let config = BotConfig::scripted(BotDifficulty::Veteran, BotStance::Aggressive, 41);
        let mut seat = SeatBot::scripted(player, config, Arc::clone(&public_map));
        let mut direct = Brain::scripted(player, config, public_map);

        assert_eq!(seat.player(), direct.player());
        assert_eq!(seat.0.profile(), direct.profile());
        assert_eq!(seat.0.dials(), direct.dials());
        let commands = seat.act(&state);
        assert!(!commands.is_empty(), "the opening think should be active");
        assert_eq!(commands, direct.act(&state));
    }

    #[test]
    fn overseer_constructor_matches_the_direct_brain() {
        let scenario = Scenario::skirmish();
        let state = scenario.build().expect("the skirmish builds");
        let player = PlayerId(1);
        let mut seat = SeatBot::overseer(player, scenario.seed);
        let mut direct = Brain::overseer(player, scenario.seed);

        assert_eq!(seat.player(), direct.player());
        assert_eq!(seat.0.profile(), direct.profile());
        assert_eq!(seat.0.dials(), direct.dials());
        let commands = seat.act(&state);
        assert!(!commands.is_empty(), "the opening think should be active");
        assert_eq!(commands, direct.act(&state));
    }

    #[test]
    fn evaluation_overseer_moves_one_policy_identity_between_seats() {
        let policy_seed = 73;
        let left = SeatBot::overseer_with_policy_seed(PlayerId(0), policy_seed);
        let right = SeatBot::overseer_with_policy_seed(PlayerId(1), policy_seed);
        let legacy_left = Brain::overseer(PlayerId(0), policy_seed);
        let legacy_right = Brain::overseer(PlayerId(1), policy_seed);

        assert_eq!(left.0.dials(), right.0.dials());
        assert_eq!(left.0.dials(), legacy_left.dials());
        assert_ne!(
            legacy_left.dials(),
            legacy_right.dials(),
            "this seed must reproduce the seat-dependent identity the evaluation constructor removes"
        );
        assert_eq!(left.player(), PlayerId(0));
        assert_eq!(right.player(), PlayerId(1));
    }

    #[test]
    fn seating_preserves_each_configured_seats_identity_and_commands() {
        let mut scenario = Scenario::skirmish();
        let configs = [
            BotConfig::scripted(BotDifficulty::Scrapheap, BotStance::Turtle, 17),
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Aggressive, 9_876_543_210),
        ];
        for (seat, config) in scenario.players.iter_mut().zip(configs) {
            seat.bot = true;
            seat.bot_config = Some(config);
        }
        let state = scenario.build().expect("the configured skirmish builds");
        let unchanged = state.hash();
        let public_map = Arc::new(
            PublicMapBriefing::from_scenario(&scenario).expect("the skirmish has a briefing"),
        );
        let mut seated = seat_bots(&scenario).expect("the configured skirmish has a briefing");

        assert_eq!(seated.len(), configs.len());
        for (index, (seat, config)) in seated.iter_mut().zip(configs).enumerate() {
            let player = PlayerId(index as u8);
            let mut direct = Brain::scripted(player, config, Arc::clone(&public_map));

            assert_eq!(seat.player(), player);
            assert_eq!(seat.0.profile(), direct.profile());
            assert_eq!(seat.0.dials(), direct.dials());
            let commands = seat.act(&state);
            assert!(!commands.is_empty(), "the opening think should be active");
            assert_eq!(commands, direct.act(&state));
            assert!(commands.iter().all(|command| command.player == player));
        }
        assert_eq!(
            state.hash(),
            unchanged,
            "asking every seat to act must not mutate the authoritative world"
        );
    }

    #[test]
    fn seating_does_not_invent_a_controller_for_an_empty_or_human_chair() {
        let mut scenario = Scenario::skirmish();
        scenario.players[0].bot = false;
        scenario.players[0].bot_config = Some(BotConfig::default());
        scenario.players[1].bot = true;
        scenario.players[1].bot_config = None;

        assert!(
            seat_bots(&scenario)
                .expect("the skirmish has a briefing")
                .is_empty(),
            "a config on a human seat and a config-less bot flag are both empty chairs"
        );
    }
}
