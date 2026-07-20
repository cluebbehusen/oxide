//! The composed 0.7 bot: observation builder + policy + executive.
//!
//! A [`Brain`] is a command source exactly like [`super::classic::Bot`]:
//! it reads [`State`], emits [`crate::PlayerCommand`]s, and its commands
//! are recorded into replays like anyone else's. Internally each think
//! runs the three layers in order — build the observation the dials
//! allow, let the executive do its housekeeping, ask the policy for
//! intents, lower them to commands.

use super::executive::Executive;
use super::observation::Observation;
use super::utility::{Dials, UtilityPolicy};
use crate::command::PlayerCommand;
use crate::ids::PlayerId;
use crate::state::State;
use chassis::grid::TilePos;
use chassis::rng::Pcg32;

/// One brain, driving one player.
#[derive(Debug, Clone)]
pub struct Brain {
    player: PlayerId,
    dials: Dials,
    policy: UtilityPolicy,
    exec: Executive,
}

impl Brain {
    /// Creates the brain for `player`. The scenario seed jitters the
    /// army-size threshold (±1) so mirror matches don't march in
    /// lockstep forever — the same trick the classic bot uses.
    pub fn new(player: PlayerId, scenario_seed: u64, mut dials: Dials) -> Self {
        let mut rng = Pcg32::new(scenario_seed, 2000 + u64::from(player.0));
        dials.army_size = (dials.army_size + rng.next_below(3))
            .saturating_sub(1)
            .max(2);
        Self {
            player,
            dials,
            policy: UtilityPolicy::new(),
            exec: Executive::default(),
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
        // The wounded rear line lives at the Foundry: behind everything,
        // and every march home routes past friendly production.
        let rear = obs
            .my_buildings
            .iter()
            .filter(|b| b.kind == crate::stats::BuildingKind::Foundry)
            .min_by_key(|b| b.id)
            .map(|b| b.anchor)
            .unwrap_or(TilePos::new(0, 0));
        let mut commands = self.exec.maintain(self.player, &obs, rear);
        let intents = self.policy.think(&self.dials, &obs, &self.exec);
        commands.extend(self.exec.apply(self.player, &obs, &intents));
        commands
    }
}
