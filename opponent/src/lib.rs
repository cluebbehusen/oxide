#![doc = include_str!("../README.md")]

mod checkpoint;
mod decision;
mod events;
mod frame;
mod income;
mod investments;
mod map;
mod memory;
mod placement;
mod profile;
mod saving;
mod trace;
mod workers;

pub use checkpoint::Checkpoint;
pub use events::{OwnEvent, OwnEvents};
pub use investments::{Investment, Step};
pub use map::MapModel;
pub use profile::{PersonalityTraits, ResolvedProfile, Specialty};
pub use trace::{NextPurchase, Purchase, SavingTarget, Trace};

use oxide_sim::observation::ObservationData;
use oxide_sim::scenario::{BotConfig, BotDifficulty};
use oxide_sim::{BuildingKind, PlayerCommand, PlayerId, State};
use std::sync::Arc;

/// One seat driven by `oxide-opponent`.
#[derive(Debug, Clone)]
pub struct Opponent {
    player: PlayerId,
    profile: ResolvedProfile,
    map: Arc<MapModel>,
    persistent: decision::Persistent,
}

impl Opponent {
    /// Creates the controller for `player` from its scenario configuration and
    /// the match's shared map model.
    pub fn new(player: PlayerId, config: BotConfig, map: Arc<MapModel>) -> Self {
        Self {
            player,
            profile: ResolvedProfile::resolve(config),
            map,
            persistent: decision::Persistent::default(),
        }
    }

    /// Scrap the seat is holding back for its saving target.
    pub fn protected_scrap(&self) -> u32 {
        self.persistent.saving.protected()
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

    /// Commands for this tick. A decision consumes the seat's buffered own
    /// events; a tick without one leaves them for the next.
    pub fn act(&mut self, state: &State, events: &mut OwnEvents) -> Vec<PlayerCommand> {
        self.decide(state, events)
            .map_or_else(Vec::new, |(_, _, decision)| decision.commands)
    }

    /// Commands for this tick plus a trace of the decision that produced them.
    /// Ticks without a decision return no trace and leave `events` untouched.
    pub fn act_traced(
        &mut self,
        state: &State,
        events: &mut OwnEvents,
    ) -> (Vec<PlayerCommand>, Option<Trace>) {
        let Some((observation, events, decision)) = self.decide(state, events) else {
            return (Vec::new(), None);
        };
        let trace = Trace {
            tick: observation.tick,
            player: self.player,
            bank: observation.scrap,
            events,
            spent: decision.spent,
            purchases: decision.purchases,
            unit_orders: decision.unit_orders,
            allowance: decision.allowance,
            target: decision.target,
            protected: decision.protected,
        };
        (decision.commands, Some(trace))
    }

    fn decide(
        &mut self,
        state: &State,
        events: &mut OwnEvents,
    ) -> Option<(ObservationData, Vec<OwnEvent>, decision::Decision)> {
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
        let events = events.take();
        let rejected = events
            .iter()
            .any(|event| matches!(event, OwnEvent::CommandRejected { .. }));
        let decision = decision::decide(
            &observation,
            rejected,
            &self.map,
            &self.profile,
            &mut self.persistent,
        );
        Some((observation, events, decision))
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
