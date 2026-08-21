//! The composed rule-based bot: observation builder + policy + executive.
//!
//! A [`Brain`] is an ordinary command source: it reads [`State`], emits
//! [`crate::PlayerCommand`]s, and its commands are recorded into
//! replays like anyone else's. Internally each think runs the three
//! layers in order — build the observation the dials allow, let the
//! executive do its housekeeping, ask the policy for intents, lower
//! them to commands.

use super::executive::Executive;
use super::observation::Observation;
use super::orient::Orientation;
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
    /// The seat's frame of reference, latched at the first act and
    /// kept for the match — the policy's bot-local tile memory
    /// (blacklists, pending sites, scout rotation) lives in oriented
    /// space, and a mid-game flip when the home Foundry changes would
    /// silently mirror all of it.
    orientation: Option<Orientation>,
}

impl Brain {
    /// Creates the brain for `player`. The scenario seed jitters the
    /// army-size threshold (±1) so mirror matches don't march in
    /// lockstep forever.
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
            orientation: None,
        }
    }

    /// The Overseer: the scripted commander with the whole 0.15 tree
    /// switched on. Training infrastructure ONLY — it bootstraps the
    /// gym-v9 retrain as demonstration source, league anchor, and
    /// yardstick, and is deliberately not reachable from any player
    /// surface (no scenario field, no wizard dial, no SeatBot arm).
    pub fn overseer(player: PlayerId, scenario_seed: u64) -> Self {
        Self::new(player, scenario_seed, Dials::overseer())
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
        if let Some(recovery) = self.exec.harvester_recovery(self.player, &obs) {
            commands.extend(recovery);
            return commands;
        }
        // The policy thinks in seat-oriented space (see [`Orientation`]):
        // the same logic runs for both seats, so its compass-flavored
        // tie-breaks cannot systematically favor either one.
        let orientation = *self
            .orientation
            .get_or_insert_with(|| Orientation::for_home(&obs, rear));
        let oriented = orientation.observe(&obs);
        let armies: Vec<_> = self
            .exec
            .armies()
            .iter()
            .map(|a| orientation.army(a.clone()))
            .collect();
        let enlisted: Vec<_> = self.exec.enlisted().collect();
        let intents = self
            .policy
            .think(&self.dials, &oriented, &armies, &enlisted);
        let intents = orientation.emit(intents);
        commands.extend(self.exec.apply(self.player, &obs, &intents));
        commands
    }
}
