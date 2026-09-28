//! One seat's controller, whichever implementation its scenario selects.
//!
//! A closed enum rather than a trait: checkpoints restore a closed serialized
//! set, background decisions clone the whole roster, and each controller keeps
//! its own trace type.

use oxide_bot::checkpoint::BotCheckpoint;
use oxide_bot::observer::PhaseObserver;
use oxide_bot::{DecisionTrace, PublicMapBriefing, SeatBot, TracedBotAct};
use oxide_opponent::Opponent;
use oxide_sim::scenario::{BotConfig, BotController, ScenarioError};
use oxide_sim::{PlayerCommand, PlayerId, Scenario, State};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A configured bot seat as the shell and driver run it.
#[derive(Debug, Clone)]
pub enum SeatController {
    /// `oxide-bot`.
    Scripted(SeatBot),
    /// `oxide-opponent`.
    Opponent(Opponent),
}

/// One controller's decision diagnostic. Untagged, so each controller's trace
/// serializes exactly as its own crate defines it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum SeatTrace {
    /// An `oxide-bot` decision.
    Scripted(Box<DecisionTrace>),
    /// An `oxide-opponent` decision.
    Opponent(oxide_opponent::Trace),
}

impl SeatTrace {
    /// The seat whose decision produced the trace.
    pub fn player(&self) -> PlayerId {
        match self {
            Self::Scripted(trace) => trace.player,
            Self::Opponent(trace) => trace.player,
        }
    }

    /// The simulation tick the decision observed.
    pub fn tick(&self) -> u64 {
        match self {
            Self::Scripted(trace) => trace.tick,
            Self::Opponent(trace) => trace.tick,
        }
    }
}

/// One seat's continuation, tagged by controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerCheckpoint {
    /// `oxide-bot` memory and planning progress.
    Scripted(BotCheckpoint),
    /// `oxide-opponent` state.
    Opponent(oxide_opponent::Checkpoint),
}

impl SeatController {
    /// The one place a configured controller choice becomes a running controller.
    pub fn configured(
        player: PlayerId,
        config: BotConfig,
        public_map: &Arc<PublicMapBriefing>,
    ) -> Self {
        match config.controller {
            BotController::Scripted => {
                Self::Scripted(SeatBot::scripted(player, config, Arc::clone(public_map)))
            }
            BotController::Opponent => Self::Opponent(Opponent::new(player, config)),
        }
    }

    /// The seat this controller drives.
    pub fn player(&self) -> PlayerId {
        match self {
            Self::Scripted(bot) => bot.player(),
            Self::Opponent(opponent) => opponent.player(),
        }
    }

    /// Which implementation runs the seat.
    pub fn controller(&self) -> BotController {
        match self {
            Self::Scripted(_) => BotController::Scripted,
            Self::Opponent(_) => BotController::Opponent,
        }
    }

    /// Whether this seat is scheduled to decide at this tick. A due seat can
    /// still produce no commands.
    pub fn decision_due(&self, state: &State) -> bool {
        match self {
            Self::Scripted(bot) => bot.decision_due(state),
            Self::Opponent(opponent) => opponent.decision_due(state),
        }
    }

    /// Commands for this tick.
    pub fn act(&mut self, state: &State) -> Vec<PlayerCommand> {
        match self {
            Self::Scripted(bot) => bot.act(state),
            Self::Opponent(opponent) => opponent.act(state),
        }
    }

    /// Commands with optional phase callbacks. `oxide-opponent` reports no
    /// phases; the caller's seat-total span still covers it.
    pub fn act_observed(
        &mut self,
        state: &State,
        observer: &dyn PhaseObserver,
    ) -> Vec<PlayerCommand> {
        match self {
            Self::Scripted(bot) => bot.act_observed(state, observer),
            Self::Opponent(opponent) => opponent.act(state),
        }
    }

    /// Commands plus a decision trace, when this tick decided.
    pub fn act_traced(&mut self, state: &State) -> (Vec<PlayerCommand>, Option<SeatTrace>) {
        match self {
            Self::Scripted(bot) => {
                let TracedBotAct { commands, trace } = bot.act_traced(state);
                (
                    commands,
                    trace.map(|trace| SeatTrace::Scripted(Box::new(trace))),
                )
            }
            Self::Opponent(opponent) => {
                let (commands, trace) = opponent.act_traced(state);
                (commands, trace.map(SeatTrace::Opponent))
            }
        }
    }

    /// Captures the seat without running a decision.
    pub fn checkpoint(&self) -> Result<ControllerCheckpoint, String> {
        match self {
            Self::Scripted(bot) => bot.checkpoint().map(ControllerCheckpoint::Scripted),
            Self::Opponent(opponent) => Ok(ControllerCheckpoint::Opponent(opponent.checkpoint())),
        }
    }

    /// Restores a validated controller at a completed simulation boundary.
    pub fn restore(
        checkpoint: &ControllerCheckpoint,
        scenario: &Scenario,
        state: &State,
    ) -> Result<Self, String> {
        match checkpoint {
            ControllerCheckpoint::Scripted(checkpoint) => {
                SeatBot::from_checkpoint(checkpoint, scenario, state).map(Self::Scripted)
            }
            ControllerCheckpoint::Opponent(checkpoint) => {
                Opponent::restore(checkpoint, scenario, state).map(Self::Opponent)
            }
        }
    }
}

/// Every controller a scenario configures, in seat order.
///
/// All seats share one public map briefing, which prepares navigation only
/// when an `oxide-bot` seat will use it. A `bot` seat without a config remains
/// an empty chair.
pub fn seat_controllers(scenario: &Scenario) -> Result<Vec<SeatController>, ScenarioError> {
    let public_map = Arc::new(PublicMapBriefing::from_scenario(scenario)?);
    let configured: Vec<(PlayerId, BotConfig)> = scenario
        .players
        .iter()
        .enumerate()
        .filter(|(_, seat)| seat.bot)
        .filter_map(|(index, seat)| {
            seat.bot_config
                .map(|config| (PlayerId(index as u8), config))
        })
        .collect();
    if configured
        .iter()
        .any(|(_, config)| config.controller == BotController::Scripted)
    {
        public_map.prepare_navigation();
    }
    Ok(configured
        .into_iter()
        .map(|(player, config)| SeatController::configured(player, config, &public_map))
        .collect())
}

/// Skirmish with an `oxide-opponent` in seat zero and an `oxide-bot` in seat one.
#[cfg(test)]
pub(crate) fn mixed_skirmish() -> Scenario {
    use oxide_sim::scenario::{BotDifficulty, BotStance};
    let mut scenario = Scenario::skirmish();
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::opponent(
        BotDifficulty::Standard,
        BotStance::Balanced,
        3,
    ));
    scenario.players[1].bot = true;
    scenario.players[1].bot_config = Some(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        4,
    ));
    scenario
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_seats_run_their_selected_controller_in_seat_order() {
        let mut scenario = mixed_skirmish();
        let state = scenario.build().unwrap();
        let mut seats = seat_controllers(&scenario).unwrap();
        assert_eq!(
            seats
                .iter()
                .map(|seat| (seat.player(), seat.controller()))
                .collect::<Vec<_>>(),
            [
                (PlayerId(0), BotController::Opponent),
                (PlayerId(1), BotController::Scripted),
            ]
        );
        let mut scripted = oxide_bot::seat_bots(&scenario).unwrap();
        let mut opponent = Opponent::new(PlayerId(0), scenario.players[0].bot_config.unwrap());
        assert!(seats.iter().all(|seat| seat.decision_due(&state)));
        assert_eq!(seats[0].act(&state), opponent.act(&state));
        assert_eq!(seats[1].act(&state), scripted[0].act(&state));

        scenario.players[0].bot_config = None;
        assert_eq!(seat_controllers(&scenario).unwrap().len(), 1);
    }

    #[test]
    fn traces_keep_each_controllers_own_serialization() {
        let scenario = mixed_skirmish();
        let state = scenario.build().unwrap();
        let mut seats = seat_controllers(&scenario).unwrap();
        let mut scripted = oxide_bot::seat_bots(&scenario).unwrap();
        let mut opponent = Opponent::new(PlayerId(0), scenario.players[0].bot_config.unwrap());

        let (commands, trace) = seats[1].act_traced(&state);
        let direct = scripted[0].act_traced(&state);
        assert_eq!(commands, direct.commands);
        let trace = trace.unwrap();
        let direct = direct.trace.unwrap();
        assert_eq!((trace.player(), trace.tick()), (PlayerId(1), 0));
        assert_eq!(
            serde_json::to_vec(&trace).unwrap(),
            serde_json::to_vec(&direct).unwrap()
        );

        let (commands, trace) = seats[0].act_traced(&state);
        let (direct_commands, direct) = opponent.act_traced(&state);
        assert_eq!(commands, direct_commands);
        let trace = trace.unwrap();
        assert_eq!((trace.player(), trace.tick()), (PlayerId(0), 0));
        assert_eq!(
            serde_json::to_vec(&trace).unwrap(),
            serde_json::to_vec(&direct.unwrap()).unwrap()
        );
    }

    #[test]
    fn checkpoints_restore_the_same_controller_and_reject_a_swapped_variant() {
        let scenario = mixed_skirmish();
        let state = scenario.build().unwrap();
        let seats = seat_controllers(&scenario).unwrap();
        let checkpoints: Vec<_> = seats
            .iter()
            .map(|seat| seat.checkpoint().unwrap())
            .collect();
        let json = serde_json::to_value(&checkpoints).unwrap();
        assert_eq!(json[0], serde_json::json!({"opponent": {"player": 0}}));
        assert!(json[1]["scripted"].is_object());
        let decoded: Vec<ControllerCheckpoint> = serde_json::from_value(json).unwrap();
        for (seat, checkpoint) in seats.iter().zip(&decoded) {
            let mut restored = SeatController::restore(checkpoint, &scenario, &state).unwrap();
            assert_eq!(
                (restored.player(), restored.controller()),
                (seat.player(), seat.controller())
            );
            assert_eq!(restored.act(&state), seat.clone().act(&state));
        }

        let mut swapped = scenario.clone();
        swapped.players.swap(0, 1);
        for checkpoint in &decoded {
            assert!(SeatController::restore(checkpoint, &swapped, &state).is_err());
        }
    }

    #[test]
    fn an_opponent_seat_reports_only_its_total_to_an_observer() {
        struct Counting(std::cell::Cell<usize>);
        impl PhaseObserver for Counting {
            fn enter(&self, _: oxide_bot::observer::BotPhase) {
                self.0.set(self.0.get() + 1);
            }
            fn exit(&self, _: oxide_bot::observer::BotPhase) {}
        }
        let scenario = mixed_skirmish();
        let state = scenario.build().unwrap();
        let mut seats = seat_controllers(&scenario).unwrap();
        let mut plain = seats.clone();
        let observer = Counting(std::cell::Cell::new(0));
        assert_eq!(
            seats[0].act_observed(&state, &observer),
            plain[0].act(&state)
        );
        assert_eq!(observer.0.get(), 0);
        assert_eq!(
            seats[1].act_observed(&state, &observer),
            plain[1].act(&state)
        );
        assert!(observer.0.get() > 0);
    }
}
