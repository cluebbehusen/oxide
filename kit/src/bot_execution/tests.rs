use super::*;
use crate::controller::{record_events, seat_controllers};
use oxide_sim::{
    Command, PlayerId, Scenario,
    scenario::{BotConfig, BotDifficulty, BotStance},
};

#[test]
fn mixed_cadences_and_post_result_preserve_commands_and_state() {
    for workers in [0, 1, 2, 4] {
        let mut scenario = Scenario::skirmish();
        for player in &mut scenario.players {
            player.bot = true;
        }
        scenario.players[0].bot_config = Some(BotConfig::new(
            BotDifficulty::Scrapheap,
            BotStance::Balanced,
            9000,
        ));
        scenario.players[1].bot_config = Some(BotConfig::new(
            BotDifficulty::Prime,
            BotStance::Balanced,
            9001,
        ));
        let mut state = scenario.build().unwrap();
        let mut expected = state.clone();
        let mut bots = seat_controllers(&scenario).unwrap();
        bots.reverse();
        let mut expected_bots = bots.clone();
        let executor = BotExecutor::new(workers);
        let saved = |bots: &[SeatController]| {
            serde_json::to_vec(
                &bots
                    .iter()
                    .map(SeatController::checkpoint)
                    .collect::<Vec<_>>(),
            )
            .unwrap()
        };
        for tick in 0..180 {
            let mut commands = executor.commands(&state, &mut bots);
            let mut serial: Vec<_> = expected_bots
                .iter_mut()
                .flat_map(|bot| bot.act(&expected))
                .collect();
            if tick == 120 {
                let surrender = PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Surrender,
                };
                commands.insert(0, surrender.clone());
                serial.insert(0, surrender);
            }
            assert_eq!(commands, serial);
            if tick % 30 == 5 {
                let stops = [0, 1].map(|seat| PlayerCommand {
                    player: PlayerId(seat),
                    command: Command::Stop { units: vec![] },
                });
                commands.extend(stops.clone());
                serial.extend(stops);
            }
            let report = state.tick(&commands);
            let expected_report = expected.tick(&serial);
            assert_eq!(report.events, expected_report.events);
            record_events(&mut bots, &report);
            record_events(&mut expected_bots, &expected_report);
            assert_eq!(saved(&bots), saved(&expected_bots));
            assert_eq!(
                serde_json::to_vec(&state).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
        }
    }
}
#[test]
fn concurrent_matches_fall_back_without_changing_bot_history() {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
    }
    let mut state = scenario.build().unwrap();
    let mut bots = seat_controllers(&scenario).unwrap();
    let mut expected_bots = bots.clone();
    let executor = BotExecutor::new(16);
    let pool = executor.pool.as_ref().unwrap();
    assert_eq!(pool.current_num_threads(), 4);
    for _ in 0..180 {
        let commands = if state.current_tick() < 60 {
            serially(|| {
                assert!(!parallel_due(&state, &bots));
                executor.commands(&state, &mut bots)
            })
        } else if state.current_tick() < 120 {
            let _busy = executor.acquire().unwrap();
            executor.commands(&state, &mut bots)
        } else {
            executor.commands(&state, &mut bots)
        };
        assert_eq!(commands, serial_commands(&state, &mut expected_bots));
        state.tick(&commands);
    }
}
#[test]
fn batch_policy_is_thread_local_and_restored_after_nesting_and_unwinding() {
    assert!(!SERIAL.get());
    serially(|| {
        assert!(SERIAL.get());
        let result = std::panic::catch_unwind(|| serially(|| panic!("interrupted job")));
        assert!(result.is_err());
        assert!(SERIAL.get());
        std::thread::scope(|scope| {
            scope.spawn(|| assert!(!SERIAL.get())).join().unwrap();
        });
    });
    assert!(!SERIAL.get());
    let result = std::panic::catch_unwind(|| serially(|| panic!("interrupted batch")));
    assert!(result.is_err());
    assert!(!SERIAL.get());
}
