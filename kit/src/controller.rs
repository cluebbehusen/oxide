//! One bot seat's `oxide-opponent` controller and the own order failures its
//! host has recorded for it.

use oxide_opponent::{MapModel, Opponent, OwnEvents};
use oxide_sim::scenario::{BotConfig, ScenarioError};
use oxide_sim::{PlayerCommand, PlayerId, Scenario, State, TickReport};
use serde::{Deserialize, Serialize};
use std::cell::OnceCell;
use std::sync::Arc;

/// One decision's diagnostic.
pub type SeatTrace = oxide_opponent::Trace;

/// The map model a roster's seats share. It is built from the scenario for
/// the first seat, so a roster without one never builds it.
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

/// A configured bot seat as the shell and driver run it, beside the own order
/// failures its host has recorded since the seat's last decision.
#[derive(Debug, Clone)]
pub struct SeatController {
    controller: Opponent,
    events: OwnEvents,
}

/// One seat's continuation: its policy state and its undelivered own events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerCheckpoint {
    /// The seat's policy state.
    pub controller: oxide_opponent::Checkpoint,
    /// Events recorded since the seat's last decision.
    pub events: OwnEvents,
}

impl SeatController {
    /// The one place a configured seat becomes a running controller.
    pub fn configured(
        player: PlayerId,
        config: BotConfig,
        opponent_map: &OpponentMap<'_>,
    ) -> Result<Self, ScenarioError> {
        Ok(Self {
            controller: Opponent::new(player, config, opponent_map.model()?),
            events: OwnEvents::default(),
        })
    }

    /// The seat this controller drives.
    pub fn player(&self) -> PlayerId {
        self.controller.player()
    }

    /// The seat's policy.
    pub fn opponent(&self) -> &Opponent {
        &self.controller
    }

    /// Scrap the seat is holding back for a saving target.
    pub fn protected_scrap(&self) -> u32 {
        self.controller.protected_scrap()
    }

    /// Whether this seat is scheduled to decide at this tick. A due seat can
    /// still produce no commands.
    pub fn decision_due(&self, state: &State) -> bool {
        self.controller.decision_due(state)
    }

    /// Commands for this tick.
    pub fn act(&mut self, state: &State) -> Vec<PlayerCommand> {
        self.controller.act(state, &mut self.events)
    }

    /// Commands plus a decision trace, when this tick decided.
    pub fn act_traced(&mut self, state: &State) -> (Vec<PlayerCommand>, Option<SeatTrace>) {
        self.controller.act_traced(state, &mut self.events)
    }

    /// Captures the seat without running a decision.
    pub fn checkpoint(&self) -> ControllerCheckpoint {
        ControllerCheckpoint {
            controller: self.controller.checkpoint(),
            events: self.events.clone(),
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
        let map = opponent_map.model().map_err(|error| error.to_string())?;
        Opponent::restore(&checkpoint.controller, scenario, state, map).map(|controller| Self {
            controller,
            events: checkpoint.events.clone(),
        })
    }
}

/// Hands each seat its own order failures from a completed tick. Hosts call
/// this after every tick that runs with these controllers, including
/// fast-forwards whose bot commands they discard.
pub fn record_events(bots: &mut [SeatController], report: &TickReport) {
    for bot in bots {
        bot.events.record(bot.controller.player(), &report.events);
    }
}

/// Every controller a scenario configures, in seat order. A `bot` seat
/// without a config remains an empty chair.
pub fn seat_controllers(scenario: &Scenario) -> Result<Vec<SeatController>, ScenarioError> {
    let opponent_map = OpponentMap::new(scenario);
    scenario
        .players
        .iter()
        .enumerate()
        .filter(|(_, seat)| seat.bot)
        .filter_map(|(index, seat)| {
            seat.bot_config
                .map(|config| (PlayerId::from_index(index), config))
        })
        .map(|(player, config)| SeatController::configured(player, config, &opponent_map))
        .collect()
}

/// Skirmish with a bot in both seats at different difficulties.
#[cfg(test)]
pub(crate) fn mixed_skirmish() -> Scenario {
    use oxide_sim::scenario::{BotDifficulty, BotStance};
    let mut scenario = Scenario::skirmish();
    scenario.players[0].bot = true;
    scenario.players[0].bot_config = Some(BotConfig::new(
        BotDifficulty::Standard,
        BotStance::Balanced,
        3,
    ));
    scenario.players[1].bot = true;
    scenario.players[1].bot_config =
        Some(BotConfig::new(BotDifficulty::Prime, BotStance::Balanced, 4));
    scenario
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_seats_run_in_seat_order_as_their_own_opponent_would() {
        let mut scenario = mixed_skirmish();
        let state = scenario.build().unwrap();
        let mut seats = seat_controllers(&scenario).unwrap();
        assert_eq!(
            seats.iter().map(SeatController::player).collect::<Vec<_>>(),
            [PlayerId(0), PlayerId(1)]
        );
        let model = Arc::new(MapModel::from_scenario(&scenario).unwrap());
        assert!(seats.iter().all(|seat| seat.decision_due(&state)));
        for (seat, player) in seats.iter_mut().zip([0, 1]) {
            let mut direct = Opponent::new(
                PlayerId(player),
                scenario.players[usize::from(player)].bot_config.unwrap(),
                Arc::clone(&model),
            );
            assert_eq!(
                seat.act(&state),
                direct.act(&state, &mut OwnEvents::default())
            );
        }

        scenario.players[0].bot_config = None;
        assert_eq!(seat_controllers(&scenario).unwrap().len(), 1);
    }

    #[test]
    fn checkpoints_restore_the_same_seat_and_reject_a_swapped_one() {
        let scenario = mixed_skirmish();
        let state = scenario.build().unwrap();
        let seats = seat_controllers(&scenario).unwrap();
        let checkpoints: Vec<_> = seats.iter().map(SeatController::checkpoint).collect();
        let json = serde_json::to_value(&checkpoints).unwrap();
        assert_eq!(json[0]["controller"]["player"], 0);
        assert_eq!(json[0]["events"], serde_json::json!([]));
        let decoded: Vec<ControllerCheckpoint> = serde_json::from_value(json).unwrap();
        let opponent_map = OpponentMap::new(&scenario);
        for (seat, checkpoint) in seats.iter().zip(&decoded) {
            let mut restored =
                SeatController::restore(checkpoint, &scenario, &state, &opponent_map).unwrap();
            assert_eq!(restored.player(), seat.player());
            assert_eq!(restored.act(&state), seat.clone().act(&state));
        }

        let mut human = scenario.clone();
        human.players[1].bot = false;
        let human_map = OpponentMap::new(&human);
        assert!(SeatController::restore(&decoded[1], &human, &state, &human_map).is_err());
    }

    #[test]
    fn seats_share_one_map_model_built_on_first_use() {
        let scenario = mixed_skirmish();
        let opponent_map = OpponentMap::new(&scenario);
        assert!(opponent_map.model.get().is_none());
        let first = opponent_map.model().unwrap();
        assert!(Arc::ptr_eq(&first, &opponent_map.model().unwrap()));

        let mut empty = scenario.clone();
        for player in &mut empty.players {
            player.bot_config = None;
        }
        let opponent_map = OpponentMap::new(&empty);
        assert!(seat_controllers(&empty).unwrap().is_empty());
        assert!(opponent_map.model.get().is_none());
    }

    #[test]
    fn hosts_read_the_scrap_a_seat_protects() {
        let scenario = mixed_skirmish();
        let mut state = scenario.build().unwrap();
        let mut seats = seat_controllers(&scenario).unwrap();
        let mut protected = false;
        while state.current_tick() < 2_400 {
            let mut commands = Vec::new();
            for seat in &mut seats {
                let (decided, trace) = seat.act_traced(&state);
                if let Some(trace) = trace {
                    assert_eq!(trace.protected, seat.protected_scrap());
                    protected |= trace.protected > 0;
                }
                commands.extend(decided);
            }
            let report = state.tick(&commands);
            record_events(&mut seats, &report);
        }
        assert!(protected, "premise: a seat saved for something");
    }

    #[test]
    fn seats_hear_only_their_own_failures_at_their_next_decision() {
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
                .map(|seat| serde_json::to_value(seat.checkpoint()).unwrap())
                .collect::<Vec<_>>()
        };
        let mut commands: Vec<_> = seats.iter_mut().flat_map(|seat| seat.act(&state)).collect();
        commands.push(stop(0));
        let report = state.tick(&commands);
        record_events(&mut seats, &report);
        let rejected =
            serde_json::json!([{"event": "command_rejected", "reason": "no_valid_units"}]);
        let before = saved(&seats);
        assert_eq!(before[0]["events"], rejected);
        assert_eq!(before[1]["events"], serde_json::json!([]));

        let mut traces = Vec::new();
        while state.current_tick() <= 12 {
            let mut commands = Vec::new();
            for seat in &mut seats {
                let (issued, trace) = seat.act_traced(&state);
                commands.extend(issued);
                traces.extend(trace.filter(|trace| trace.player == PlayerId(0)));
            }
            let report = state.tick(&commands);
            record_events(&mut seats, &report);
        }
        let [trace] = traces.as_slice() else {
            panic!("one decision of seat zero: {traces:?}");
        };
        assert_eq!(trace.tick, 12);
        assert_eq!(serde_json::to_value(&trace.events).unwrap(), rejected);
        assert_eq!(saved(&seats)[0]["events"], serde_json::json!([]));
    }
}
