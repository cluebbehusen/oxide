//! Bots: command sources that read [`crate::State`] and emit
//! [`crate::PlayerCommand`]s, exactly like a mouse or the debug socket.
//!
//! Every brain is layered:
//!
//! ```text
//! Observation -> policy Intent -> Executive -> PlayerCommand[]
//! ```
//!
//! [`observation`] builds what a bot may know — omnisciently or
//! fog-honestly; [`Intent`]s are the policy's vocabulary; the
//! [`Executive`] owns army bookkeeping and lowers intents to commands.
//! [`Brain::balanced`] is the player-facing rules-based opponent.
//! [`Brain::overseer`] remains a separate QA yardstick.

pub mod brain;
pub mod executive;
pub mod observation;
pub mod orient;
pub mod utility;

pub use brain::Brain;
pub use executive::{Army, ArmyId, ArmyState, Executive, Intent};
pub use observation::{BuildingObs, Observation, UnitObs};
pub use orient::Orientation;
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
            p.bot_config.map(|crate::scenario::BotConfig::Scripted| {
                SeatBot(Box::new(Brain::balanced(player, scenario.seed)))
            })
        })
        .collect()
}
