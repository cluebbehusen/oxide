use super::*;
use crate::bot_eval::EvaluationController;
use chassis::replay::Replay;
use oxide_sim::Scenario;
use oxide_sim::scenario::{BotDifficulty, BotStance};

fn seats(description: Option<String>) -> Vec<SeatPlayer> {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = false;
    }
    let mut replay: GameReplay = Replay::new(SIM_VERSION, scenario);
    replay.meta.ticks = Some(12);
    replay.meta.description = description;
    ledger(Path::new("unit.json"), &replay)
        .unwrap()
        .seats
        .into_iter()
        .map(|seat| seat.controller)
        .collect()
}

#[test]
fn evaluation_replays_name_the_controllers_that_drove_their_seats() {
    let prime = BotConfig::new(BotDifficulty::Prime, BotStance::Balanced, 9);
    let controllers =
        serde_json::to_string(&[Some(EvaluationController::configured(prime)), None]).unwrap();
    assert_eq!(
        seats(Some(format!(
            "bot-eval candidate=unit; leg=forward; controllers={controllers}"
        ))),
        [SeatPlayer::Bot { config: prime }, SeatPlayer::Human]
    );
    assert_eq!(
        seats(Some(
            "bot-eval candidate=unit; controllers=[not json".into()
        )),
        [SeatPlayer::Unknown, SeatPlayer::Unknown]
    );
    assert_eq!(
        seats(Some(format!(
            "bot-eval candidate=a; controllers=[]; leg=forward; controllers={controllers}"
        ))),
        [SeatPlayer::Bot { config: prime }, SeatPlayer::Human],
        "the last marker holds the controllers"
    );
    assert_eq!(seats(None), [SeatPlayer::Human, SeatPlayer::Human]);
}
