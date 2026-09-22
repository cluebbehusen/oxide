//! State-to-observation adapter; policy receives only the resulting snapshot.
use crate::{
    Brain, Dials, Executive, Observation, PublicMapBriefing, ResolvedProfile, TracedBotAct,
    observer,
};
use std::sync::Arc;
/// A bot seat as the shell and driver run it.
#[derive(Debug, Clone)]
pub struct SeatBot(Box<Brain>);

impl SeatBot {
    /// Captures controller memory and unfinished planning without running a decision.
    pub fn checkpoint(&self) -> Result<crate::checkpoint::BotCheckpoint, String> {
        self.0.checkpoint()
    }

    /// Restores a validated controller at a completed simulation boundary.
    pub fn from_checkpoint(
        checkpoint: &crate::checkpoint::BotCheckpoint,
        scenario: &oxide_sim::Scenario,
        state: &oxide_sim::State,
    ) -> Result<Self, String> {
        Brain::from_checkpoint(checkpoint, scenario, state).map(|brain| Self(Box::new(brain)))
    }

    /// Creates a seat running the configurable player-facing controller.
    pub fn scripted(
        player: oxide_sim::ids::PlayerId,
        config: oxide_sim::scenario::BotConfig,
        public_map: Arc<PublicMapBriefing>,
    ) -> Self {
        Self(Box::new(Brain::scripted(player, config, public_map)))
    }

    /// Whether this seat is scheduled to think at this tick.
    /// A due seat can still produce no commands.
    pub fn decision_due(&self, state: &oxide_sim::State) -> bool {
        state.result().is_none() && self.0.decision_due(state.current_tick())
    }

    /// Commands for this tick.
    pub fn act(
        &mut self,
        state: &oxide_sim::state::State,
    ) -> Vec<oxide_sim::command::PlayerCommand> {
        self.observe(state, None)
            .map_or_else(Vec::new, |obs| self.0.act(&obs))
    }

    /// Commands through the normal controller with optional observational phase callbacks.
    pub fn act_observed(
        &mut self,
        state: &oxide_sim::State,
        observer: &dyn observer::PhaseObserver,
    ) -> Vec<oxide_sim::PlayerCommand> {
        self.observe(state, Some(observer))
            .map_or_else(Vec::new, |obs| self.0.act_observed(&obs, observer))
    }

    /// Commands plus an opt-in player-facing decision trace for this tick.
    pub fn act_traced(&mut self, state: &oxide_sim::state::State) -> TracedBotAct {
        self.observe(state, None).map_or_else(
            || TracedBotAct {
                commands: Vec::new(),
                trace: None,
            },
            |obs| self.0.act_traced(&obs),
        )
    }

    fn observe(
        &self,
        state: &oxide_sim::State,
        observer: Option<&dyn observer::PhaseObserver>,
    ) -> Option<Observation> {
        if !self.decision_due(state) {
            return None;
        }
        let scope = observer::PhaseScope::new(observer, observer::BotPhase::Observation);
        let obs = if self.0.dials().fog_honest {
            Observation::fog_honest(state, self.player())
        } else {
            Observation::omniscient(state, self.player())
        };
        drop(scope);
        if state.player(self.player()).resigned {
            let _capture = crate::query_work::Capture::new(observer);
            let _scope = observer::PhaseScope::new(observer, observer::BotPhase::Maintenance);
            return None;
        }
        Some(obs)
    }

    /// Creates a Standard, Balanced, seed-zero seat.
    pub fn balanced(player: oxide_sim::PlayerId, public_map: Arc<PublicMapBriefing>) -> Self {
        Self(Box::new(Brain::balanced(player, public_map)))
    }

    /// The resolved controller personality.
    pub fn profile(&self) -> &ResolvedProfile {
        self.0.profile()
    }

    /// The controller's cognitive and execution parameters.
    pub fn dials(&self) -> &Dials {
        self.0.dials()
    }

    /// The controller's current army assignments.
    pub fn executive(&self) -> &Executive {
        self.0.executive()
    }

    /// The player this bot drives.
    pub fn player(&self) -> oxide_sim::ids::PlayerId {
        self.0.player()
    }
}

/// Every bot a scenario asks for, honoring each seat's `bot_config`.
///
/// A configured seat receives the fair rules-based opponent. A `bot`
/// seat without a config remains an empty chair rather than silently
/// selecting a controller.
pub fn seat_bots(
    scenario: &oxide_sim::Scenario,
) -> Result<Vec<SeatBot>, oxide_sim::scenario::ScenarioError> {
    let public_map = Arc::new(PublicMapBriefing::from_scenario(scenario)?);
    if scenario
        .players
        .iter()
        .any(|player| player.bot && player.bot_config.is_some())
    {
        public_map.prepare_navigation();
    }
    Ok(scenario
        .players
        .iter()
        .enumerate()
        .filter(|(_, p)| p.bot)
        .filter_map(|(i, p)| {
            let player = oxide_sim::ids::PlayerId(i as u8);
            p.bot_config
                .map(|config| SeatBot::scripted(player, config, Arc::clone(&public_map)))
        })
        .collect())
}

#[cfg(test)]
impl std::ops::Deref for SeatBot {
    type Target = Brain;
    fn deref(&self) -> &Brain {
        &self.0
    }
}
#[cfg(test)]
impl std::ops::DerefMut for SeatBot {
    fn deref_mut(&mut self) -> &mut Brain {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
    use oxide_sim::{PlayerId, Scenario};

    #[test]
    fn another_seats_observation_cannot_advance_the_controller() {
        let scenario = Scenario::skirmish();
        let state = scenario.build().unwrap();
        let briefing = Arc::new(PublicMapBriefing::from_scenario(&scenario).unwrap());
        let mut brain = Brain::balanced(PlayerId(0), briefing);
        let mut untouched = brain.clone();
        let wrong = Observation::fog_honest(&state, PlayerId(1));
        let rejected = brain.act_traced(&wrong);
        assert!(rejected.commands.is_empty());
        assert!(rejected.trace.is_none());
        let own = Observation::fog_honest(&state, PlayerId(0));
        assert_eq!(brain.act_traced(&own), untouched.act_traced(&own));
    }

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
        assert_eq!(
            commands,
            direct.act(&Observation::fog_honest(&state, direct.player()))
        );
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
            assert_eq!(
                commands,
                direct.act(&Observation::fog_honest(&state, direct.player()))
            );
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
