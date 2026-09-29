//! One seat's controller, whichever implementation its scenario selects.
//!
//! A closed enum rather than a trait: checkpoints restore a closed serialized
//! set, background decisions clone the whole roster, and each controller keeps
//! its own trace type.

use oxide_bot::checkpoint::BotCheckpoint;
use oxide_bot::observer::PhaseObserver;
use oxide_bot::{DecisionTrace, PublicMapBriefing, SeatBot, TracedBotAct};
use oxide_opponent::{MapModel, Opponent, OwnEvents};
use oxide_sim::scenario::{BotConfig, BotController, ScenarioError};
use oxide_sim::{PlayerCommand, PlayerId, Scenario, State, TickReport};
use serde::{Deserialize, Serialize};
use std::cell::OnceCell;
use std::sync::Arc;

/// The `oxide-opponent` map model a roster's seats share. It is built from the
/// scenario for the first such seat, so a roster without one never builds it.
pub struct OpponentMap<'a> {
    scenario: &'a Scenario,
    model: OnceCell<Arc<MapModel>>,
}

impl<'a> OpponentMap<'a> {
    /// A holder for `scenario`'s model. Nothing is built yet.
    pub fn new(scenario: &'a Scenario) -> Self {
        Self {
            scenario,
            model: OnceCell::new(),
        }
    }

    fn model(&self) -> Result<Arc<MapModel>, ScenarioError> {
        if let Some(model) = self.model.get() {
            return Ok(Arc::clone(model));
        }
        let model = Arc::new(MapModel::from_scenario(self.scenario)?);
        Ok(Arc::clone(self.model.get_or_init(|| model)))
    }
}

/// A configured bot seat as the shell and driver run it.
#[derive(Debug, Clone)]
pub enum SeatController {
    /// `oxide-bot`.
    Scripted(SeatBot),
    /// `oxide-opponent`, beside the own order failures its host has recorded
    /// since the seat's last decision.
    Opponent {
        /// The seat's policy.
        controller: Opponent,
        /// Consumed by the seat's next decision.
        events: OwnEvents,
    },
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
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ControllerCheckpoint {
    /// `oxide-bot` memory and planning progress.
    Scripted(BotCheckpoint),
    /// `oxide-opponent` state and its undelivered own events.
    Opponent {
        /// The seat's policy state.
        controller: oxide_opponent::Checkpoint,
        /// Events recorded since the seat's last decision.
        events: OwnEvents,
    },
}

impl SeatController {
    /// The one place a configured controller choice becomes a running controller.
    pub fn configured(
        player: PlayerId,
        config: BotConfig,
        public_map: &Arc<PublicMapBriefing>,
        opponent_map: &OpponentMap<'_>,
    ) -> Result<Self, ScenarioError> {
        Ok(match config.controller {
            BotController::Scripted => {
                Self::Scripted(SeatBot::scripted(player, config, Arc::clone(public_map)))
            }
            BotController::Opponent => Self::Opponent {
                controller: Opponent::new(player, config, opponent_map.model()?),
                events: OwnEvents::default(),
            },
        })
    }

    /// The seat this controller drives.
    pub fn player(&self) -> PlayerId {
        match self {
            Self::Scripted(bot) => bot.player(),
            Self::Opponent { controller, .. } => controller.player(),
        }
    }

    /// Which implementation runs the seat.
    pub fn controller(&self) -> BotController {
        match self {
            Self::Scripted(_) => BotController::Scripted,
            Self::Opponent { .. } => BotController::Opponent,
        }
    }

    /// Whether this seat is scheduled to decide at this tick. A due seat can
    /// still produce no commands.
    pub fn decision_due(&self, state: &State) -> bool {
        match self {
            Self::Scripted(bot) => bot.decision_due(state),
            Self::Opponent { controller, .. } => controller.decision_due(state),
        }
    }

    /// Commands for this tick.
    pub fn act(&mut self, state: &State) -> Vec<PlayerCommand> {
        match self {
            Self::Scripted(bot) => bot.act(state),
            Self::Opponent { controller, events } => controller.act(state, events),
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
            Self::Opponent { controller, events } => controller.act(state, events),
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
            Self::Opponent { controller, events } => {
                let (commands, trace) = controller.act_traced(state, events);
                (commands, trace.map(SeatTrace::Opponent))
            }
        }
    }

    /// Captures the seat without running a decision.
    pub fn checkpoint(&self) -> Result<ControllerCheckpoint, String> {
        match self {
            Self::Scripted(bot) => bot.checkpoint().map(ControllerCheckpoint::Scripted),
            Self::Opponent { controller, events } => Ok(ControllerCheckpoint::Opponent {
                controller: controller.checkpoint(),
                events: events.clone(),
            }),
        }
    }

    /// Restores a validated controller at a completed simulation boundary.
    /// `opponent_map` holds the model for the same scenario.
    pub fn restore(
        checkpoint: &ControllerCheckpoint,
        scenario: &Scenario,
        state: &State,
        opponent_map: &OpponentMap<'_>,
    ) -> Result<Self, String> {
        match checkpoint {
            ControllerCheckpoint::Scripted(checkpoint) => {
                SeatBot::from_checkpoint(checkpoint, scenario, state).map(Self::Scripted)
            }
            ControllerCheckpoint::Opponent { controller, events } => {
                let map = opponent_map.model().map_err(|error| error.to_string())?;
                Opponent::restore(controller, scenario, state, map).map(|controller| {
                    Self::Opponent {
                        controller,
                        events: events.clone(),
                    }
                })
            }
        }
    }
}

/// Hands each `oxide-opponent` seat its own order failures from a completed
/// tick. Hosts call this after every tick that runs with these controllers,
/// including fast-forwards whose bot commands they discard.
pub fn record_events(bots: &mut [SeatController], report: &TickReport) {
    for bot in bots {
        if let SeatController::Opponent { controller, events } = bot {
            events.record(controller.player(), &report.events);
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
    let opponent_map = OpponentMap::new(scenario);
    configured
        .into_iter()
        .map(|(player, config)| {
            SeatController::configured(player, config, &public_map, &opponent_map)
        })
        .collect()
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
        let mut opponent = Opponent::new(
            PlayerId(0),
            scenario.players[0].bot_config.unwrap(),
            Arc::new(MapModel::from_scenario(&scenario).unwrap()),
        );
        assert!(seats.iter().all(|seat| seat.decision_due(&state)));
        assert_eq!(
            seats[0].act(&state),
            opponent.act(&state, &mut OwnEvents::default())
        );
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
        let mut opponent = Opponent::new(
            PlayerId(0),
            scenario.players[0].bot_config.unwrap(),
            Arc::new(MapModel::from_scenario(&scenario).unwrap()),
        );

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
        let (direct_commands, direct) = opponent.act_traced(&state, &mut OwnEvents::default());
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
        assert_eq!(
            json[0],
            serde_json::json!({"opponent": {"controller": {"player": 0}, "events": []}})
        );
        assert!(json[1]["scripted"].is_object());
        let decoded: Vec<ControllerCheckpoint> = serde_json::from_value(json).unwrap();
        let opponent_map = OpponentMap::new(&scenario);
        for (seat, checkpoint) in seats.iter().zip(&decoded) {
            let mut restored =
                SeatController::restore(checkpoint, &scenario, &state, &opponent_map).unwrap();
            assert_eq!(
                (restored.player(), restored.controller()),
                (seat.player(), seat.controller())
            );
            assert_eq!(restored.act(&state), seat.clone().act(&state));
        }

        let mut swapped = scenario.clone();
        swapped.players.swap(0, 1);
        let swapped_map = OpponentMap::new(&swapped);
        for checkpoint in &decoded {
            assert!(SeatController::restore(checkpoint, &swapped, &state, &swapped_map).is_err());
        }
    }

    #[test]
    fn opponent_seats_share_one_map_model_built_on_first_use() {
        let scenario = mixed_skirmish();
        let opponent_map = OpponentMap::new(&scenario);
        assert!(opponent_map.model.get().is_none());
        let first = opponent_map.model().unwrap();
        assert!(Arc::ptr_eq(&first, &opponent_map.model().unwrap()));

        let mut scripted_only = scenario.clone();
        scripted_only.players[0].bot_config = None;
        let opponent_map = OpponentMap::new(&scripted_only);
        let public_map = Arc::new(PublicMapBriefing::from_scenario(&scripted_only).unwrap());
        let config = scripted_only.players[1].bot_config.unwrap();
        SeatController::configured(PlayerId(1), config, &public_map, &opponent_map).unwrap();
        assert!(opponent_map.model.get().is_none());
    }

    #[test]
    fn opponent_seats_hear_only_their_own_failures_at_their_next_decision() {
        let scenario = mixed_skirmish();
        let mut state = scenario.build().unwrap();
        let mut seats = seat_controllers(&scenario).unwrap();
        let stop = |player: u8| PlayerCommand {
            player: PlayerId(player),
            command: oxide_sim::Command::Stop { units: Vec::new() },
        };
        let saved = |seats: &[SeatController]| {
            seats
                .iter()
                .map(|seat| serde_json::to_value(seat.checkpoint().unwrap()).unwrap())
                .collect::<Vec<_>>()
        };
        let mut commands: Vec<_> = seats.iter_mut().flat_map(|seat| seat.act(&state)).collect();
        commands.extend([stop(1), stop(0)]);
        let report = state.tick(&commands);
        let scripted = saved(&seats)[1].clone();
        record_events(&mut seats, &report);
        let rejected =
            serde_json::json!([{"event": "command_rejected", "reason": "no_valid_units"}]);
        let before = saved(&seats);
        assert_eq!(before[0]["opponent"]["events"], rejected);
        assert_eq!(before[1], scripted);

        let mut traces = Vec::new();
        while state.current_tick() <= 12 {
            let mut commands = Vec::new();
            for seat in &mut seats {
                let (issued, trace) = seat.act_traced(&state);
                commands.extend(issued);
                traces.extend(trace.filter(|trace| trace.player() == PlayerId(0)));
            }
            let report = state.tick(&commands);
            record_events(&mut seats, &report);
        }
        let [SeatTrace::Opponent(trace)] = traces.as_slice() else {
            panic!("one opponent decision: {traces:?}");
        };
        assert_eq!(trace.tick, 12);
        assert_eq!(serde_json::to_value(&trace.events).unwrap(), rejected);
        assert_eq!(
            saved(&seats)[0]["opponent"]["events"],
            serde_json::json!([])
        );
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
