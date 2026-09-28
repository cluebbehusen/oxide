#![doc = include_str!("../README.md")]

mod checkpoint;
mod policy;
mod profile;
mod trace;

pub use checkpoint::Checkpoint;
pub use profile::{PersonalityTraits, ResolvedProfile, Specialty};
pub use trace::{Purchase, Trace};

use oxide_sim::observation::ObservationData;
use oxide_sim::scenario::{BotConfig, BotDifficulty};
use oxide_sim::{BuildingKind, PlayerCommand, PlayerId, State};

/// One seat driven by `oxide-opponent`.
#[derive(Debug, Clone)]
pub struct Opponent {
    player: PlayerId,
    profile: ResolvedProfile,
}

impl Opponent {
    /// Creates the controller for `player` from its scenario configuration.
    pub fn new(player: PlayerId, config: BotConfig) -> Self {
        Self {
            player,
            profile: ResolvedProfile::resolve(config),
        }
    }

    /// The seat this controller drives.
    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// The resolved difficulty, stance and personality.
    pub fn profile(&self) -> &ResolvedProfile {
        &self.profile
    }

    /// Whether this seat decides at the state's tick. A due seat can still
    /// produce no commands.
    pub fn decision_due(&self, state: &State) -> bool {
        state.result().is_none()
            && state
                .current_tick()
                .is_multiple_of(decision_interval(self.profile.difficulty))
    }

    /// Commands for this tick.
    pub fn act(&mut self, state: &State) -> Vec<PlayerCommand> {
        self.decide(state)
            .map_or_else(Vec::new, |(_, decision)| decision.commands)
    }

    /// Commands for this tick plus a trace of the decision that produced them.
    /// Ticks without a decision return no trace.
    pub fn act_traced(&mut self, state: &State) -> (Vec<PlayerCommand>, Option<Trace>) {
        let Some((observation, decision)) = self.decide(state) else {
            return (Vec::new(), None);
        };
        let trace = Trace {
            tick: observation.tick,
            player: self.player,
            bank: observation.scrap,
            spent: decision.spent,
            purchases: decision.purchases,
            unit_orders: decision.unit_orders,
        };
        (decision.commands, Some(trace))
    }

    fn decide(&self, state: &State) -> Option<(ObservationData, policy::Decision)> {
        if !self.decision_due(state)
            || state.player(self.player).resigned
            || !state.buildings().iter().any(|building| {
                building.player == self.player
                    && building.kind == BuildingKind::Foundry
                    && building.built
            })
        {
            return None;
        }
        let observation = ObservationData::fog_honest(state, self.player);
        let decision = policy::decide(&observation);
        Some((observation, decision))
    }
}

/// Ticks between decisions. Scrapheap decides half as often as the other rungs.
fn decision_interval(difficulty: BotDifficulty) -> u64 {
    match difficulty {
        BotDifficulty::Scrapheap => 24,
        BotDifficulty::Standard | BotDifficulty::Veteran | BotDifficulty::Prime => 12,
    }
}

#[cfg(test)]
mod tests;
