//! Controller persistence. The profile and decision interval rebuild from the
//! scenario and the map model is shared scenario data, so a checkpoint holds
//! only the seat.

use crate::{MapModel, Opponent};
use oxide_sim::scenario::BotController;
use oxide_sim::{PlayerId, Scenario, State};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The non-derivable state one seat needs to continue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub(crate) player: PlayerId,
}

impl Opponent {
    /// Captures the seat without running a decision.
    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            player: self.player,
        }
    }

    /// Restores a seat at a completed simulation boundary. The seat must be a
    /// configured `oxide-opponent` bot in both the scenario and the world, and
    /// `map` the model built from that scenario.
    pub fn restore(
        checkpoint: &Checkpoint,
        scenario: &Scenario,
        state: &State,
        map: Arc<MapModel>,
    ) -> Result<Self, String> {
        let seat = usize::from(checkpoint.player.0);
        let spec = scenario
            .players
            .get(seat)
            .filter(|_| seat < state.players().len())
            .ok_or("invalid controller seat")?;
        let config = spec
            .bot_config
            .filter(|_| spec.bot)
            .ok_or("checkpoint seat is not a configured bot")?;
        if config.controller != BotController::Opponent {
            return Err("checkpoint seat is not an oxide-opponent seat".into());
        }
        Ok(Self::new(checkpoint.player, config, map))
    }
}
