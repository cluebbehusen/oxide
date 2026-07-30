//! Bots: command sources that read [`crate::State`] and emit
//! [`crate::PlayerCommand`]s, exactly like a mouse or the debug socket.
//!
//! The modern brains are layered (the architecture both the scripted
//! and any learned policy share):
//!
//! ```text
//! Observation -> policy Intent -> Executive -> PlayerCommand[]
//! ```
//!
//! [`observation`] builds what a bot may know — omnisciently or
//! fog-honestly; [`Intent`]s are the policy's vocabulary; the
//! [`Executive`] owns army bookkeeping and lowers intents to commands.
//! The legacy rule-cascade bot survives as [`classic::Bot`], the benchmark
//! opponent every new tier must beat.

pub mod brain;
pub mod classic;
pub mod executive;
pub mod gym;
pub mod neural;
pub mod observation;
pub mod orient;
pub mod tiers;
pub mod utility;

pub use brain::Brain;
pub use classic::Bot;
pub use executive::{Army, ArmyId, ArmyState, Doctrine, Executive, Intent, LoweringRules};
pub use gym::{
    ACTION_COUNT, ACTION_HEADS, Action, ActionPlan, CONSTRUCTION_ACTIONS,
    CONSTRUCTION_PLAN_TIMEOUT_TICKS, Decision, FEATURE_COUNT, FEATURE_NAMES, GYM_VERSION, GymBot,
    OPERATION_ACTIONS, PRODUCTION_ACTIONS,
};
pub use neural::{
    CONDITIONING_COUNT, DEALT_AGGRESSION_MAX, DEALT_AGGRESSION_MIN, DECISION_STREAM_BASE,
    LADDER_CADENCE, Level, NeuralBot, QuantNet, deal_aggression,
};
pub use observation::{BuildingObs, Observation, UnitObs};
pub use orient::Orientation;
pub use tiers::Difficulty;
pub use utility::{Dials, UtilityPolicy};

/// A bot seat as the shell and driver run it: the shipped neural
/// ladder when the scenario configures one, the legacy rule cascade
/// otherwise (which is what keeps replays recorded before bot configs
/// existed and
/// fixtures reproducing).
#[derive(Debug, Clone)]
pub enum SeatBot {
    /// The legacy rule-based benchmark bot.
    Classic(Box<Bot>),
    /// The shipped ladder network.
    Neural(Box<NeuralBot>),
}

impl SeatBot {
    /// Commands for this tick.
    pub fn act(&mut self, state: &crate::state::State) -> Vec<crate::command::PlayerCommand> {
        match self {
            SeatBot::Classic(b) => b.act(state),
            SeatBot::Neural(b) => b.act(state),
        }
    }

    /// The player this bot drives.
    pub fn player(&self) -> crate::ids::PlayerId {
        match self {
            SeatBot::Classic(b) => b.player(),
            SeatBot::Neural(b) => b.player(),
        }
    }
}

/// Every bot a scenario asks for, honoring each seat's `bot_config`.
pub fn seat_bots(scenario: &crate::Scenario) -> Vec<SeatBot> {
    scenario
        .players
        .iter()
        .enumerate()
        .filter(|(_, p)| p.bot)
        .map(|(i, p)| {
            let player = crate::ids::PlayerId(i as u8);
            match p.bot_config {
                Some(config) => SeatBot::Neural(Box::new(NeuralBot::ladder(
                    player,
                    scenario.seed,
                    config.level,
                    config.aggression,
                    p.faction,
                ))),
                None => SeatBot::Classic(Box::new(Bot::new(player, scenario.seed))),
            }
        })
        .collect()
}
