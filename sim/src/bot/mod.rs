//! Bots: command sources that read [`crate::State`] and emit
//! [`crate::PlayerCommand`]s, exactly like a mouse or the debug socket.
//!
//! The player-facing controller is layered:
//!
//! ```text
//! fog-honest Observation
//!   -> StrategicIntelligence
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

pub mod brain;
pub mod difficulty;
pub mod executive;
pub mod intelligence;
pub mod lift;
pub mod observation;
pub mod orient;
pub mod profile;
pub mod raid;
mod routing;
pub mod strategy;
pub mod team;
pub mod utility;

pub use brain::Brain;
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
pub use strategy::{
    AirOperation, AirOperationPhase, AirRecoveryReason, StrategicDecision, StrategicPlanner,
};
pub use team::{TeamReliefExitReason, TeamReliefOperation, TeamReliefPhase, TeamReliefPlanner};
pub use utility::{Dials, UtilityPolicy};

/// A bot seat as the shell and driver run it.
#[derive(Debug, Clone)]
pub struct SeatBot(Box<Brain>);

impl SeatBot {
    /// Commands for this tick.
    pub fn act(&mut self, state: &crate::state::State) -> Vec<crate::command::PlayerCommand> {
        self.0.act(state)
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
pub fn seat_bots(scenario: &crate::Scenario) -> Vec<SeatBot> {
    scenario
        .players
        .iter()
        .enumerate()
        .filter(|(_, p)| p.bot)
        .filter_map(|(i, p)| {
            let player = crate::ids::PlayerId(i as u8);
            p.bot_config
                .map(|config| SeatBot(Box::new(Brain::scripted(player, scenario.seed, config))))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
    use crate::{PlayerId, Scenario};

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
        let mut seated = seat_bots(&scenario);

        assert_eq!(seated.len(), configs.len());
        for (index, (seat, config)) in seated.iter_mut().zip(configs).enumerate() {
            let player = PlayerId(index as u8);
            let mut direct = Brain::scripted(player, scenario.seed, config);

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
            seat_bots(&scenario).is_empty(),
            "a config on a human seat and a config-less bot flag are both empty chairs"
        );
    }
}
