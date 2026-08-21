//! Bots: command sources that read [`crate::State`] and emit
//! [`crate::PlayerCommand`]s, exactly like a mouse or the debug socket.
//!
//! Every brain is layered (the architecture the scripted and any
//! learned policy share):
//!
//! ```text
//! Observation -> policy Intent -> Executive -> PlayerCommand[]
//! ```
//!
//! [`observation`] builds what a bot may know — omnisciently or
//! fog-honestly; [`Intent`]s are the policy's vocabulary; the
//! [`Executive`] owns army bookkeeping and lowers intents to commands.
//! The only scripted commander is [`Brain::overseer`] — training
//! bootstrap and QA yardstick, never a player-facing opponent.

pub mod brain;
pub mod executive;
pub mod gym;
pub mod neural;
pub mod observation;
pub mod orient;
pub mod profile;
pub mod utility;

pub use crate::scenario::{NamedStyle, TeamRole};
pub use brain::Brain;
pub use executive::{Army, ArmyId, ArmyState, Doctrine, Executive, Intent, LoweringRules};
pub use gym::{
    ACTION_COUNT, ACTION_HEADS, Action, ActionPlan, CONSTRUCTION_ACTIONS,
    CONSTRUCTION_PLAN_TIMEOUT_TICKS, Decision, FEATURE_COUNT, FEATURE_NAMES, GYM_VERSION, GymBot,
    OPERATION_ACTIONS, PRODUCTION_ACTIONS,
};
pub use neural::{
    CONDITION_NAMES, CONDITIONING_COUNT, DEALT_AGGRESSION_MAX, DEALT_AGGRESSION_MIN,
    DECISION_STREAM_BASE, Level, NeuralBot, QuantNet, deal_aggression, ladder_condition_values,
    ladder_condition_values_with_facets,
};
pub use observation::{BuildingObs, Observation, UnitObs};
pub use orient::Orientation;
pub use profile::{
    BotProfileError, CanonicalProfile, NAMED_VARIANT_COUNT, PROFILE_CONDITION_COUNT,
    PROFILE_CONDITION_NAMES, PROFILE_ROLE_STREAM, PROFILE_STYLE_STREAM_BASE, PROFILE_TEAM_ROLES,
    PROFILE_VARIANT_STREAM_BASE, ProfileFacets, ResolvedBotProfile, canonical_profiles,
    deal_named_style, deal_style_variant, resolve_bot_profiles, resolve_team_roles,
};
pub use utility::{Dials, UtilityPolicy};

/// A bot seat as the shell and driver run it: a quantized policy
/// evaluated with deterministic integer math.
#[derive(Debug, Clone)]
pub enum SeatBot {
    /// A neural policy driving one seat.
    Neural(Box<NeuralBot>),
}

impl SeatBot {
    /// Commands for this tick.
    pub fn act(&mut self, state: &crate::state::State) -> Vec<crate::command::PlayerCommand> {
        match self {
            SeatBot::Neural(b) => b.act(state),
        }
    }

    /// The player this bot drives.
    pub fn player(&self) -> crate::ids::PlayerId {
        match self {
            SeatBot::Neural(b) => b.player(),
        }
    }
}

/// Every bot a scenario asks for, honoring each seat's `bot_config`.
///
/// The promoted 0.15 actor is the only commander. A configured bot
/// seat resolves its named profile and drives the embedded ladder
/// network; a `bot` seat WITHOUT a config seats no commander at all —
/// an empty chair, not an error, because scenario JSON is loadable
/// content and the wizard and every shipped map always write configs.
pub fn seat_bots(scenario: &crate::Scenario) -> Vec<SeatBot> {
    let profiles = resolve_bot_profiles(scenario)
        .expect("a scenario's bot profiles validate before its bots are seated");
    // Public map knowledge: where every base starts. The same prior a
    // player takes from the map screen; fog still governs the present.
    let anchors: Vec<chassis::grid::TilePos> = scenario
        .start_anchors()
        .map(|list| list.into_iter().map(|(_, tile)| tile).collect())
        .unwrap_or_default();
    scenario
        .players
        .iter()
        .enumerate()
        .filter(|(_, p)| p.bot)
        .filter_map(|(i, p)| {
            let player = crate::ids::PlayerId(i as u8);
            profiles[i].map(|profile| {
                let mut bot = NeuralBot::ladder_resolved(player, scenario.seed, profile, p.faction);
                bot.set_start_anchors(anchors.clone());
                SeatBot::Neural(Box::new(bot))
            })
        })
        .collect()
}
