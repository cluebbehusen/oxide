//! Ordered bot command collection for live and headless sessions.

use std::cell::Cell;
use std::sync::{Mutex, OnceLock};

use oxide_sim::{PlayerCommand, State, bot::SeatBot};
use rayon::prelude::*;

thread_local! {
    static SERIAL: Cell<bool> = const { Cell::new(false) };
}

/// Run synchronous work with serial bot execution on this thread.
/// Use inside workers that already parallelize independent matches.
/// Nested calls and unwinding restore the previous scheduling policy.
pub fn serially<T>(work: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            SERIAL.set(self.0);
        }
    }
    let _restore = Restore(SERIAL.replace(true));
    work()
}

/// Collect commands in input seat order, joining all bot work before returning.
///
/// Two or more due seats may share a process-wide pool of at most four workers.
/// Single-seat ticks, unavailable workers, and concurrent matches use the serial
/// path. Worker availability never changes command ordering or bot inputs.
pub fn commands(state: &State, bots: &mut [SeatBot]) -> Vec<PlayerCommand> {
    if !parallel_due(state, bots) {
        return serial_commands(state, bots);
    }
    static EXECUTOR: OnceLock<BotExecutor> = OnceLock::new();
    EXECUTOR
        .get_or_init(|| {
            BotExecutor::new(std::thread::available_parallelism().map_or(1, |n| n.get()))
        })
        .commands(state, bots)
}

fn parallel_due(state: &State, bots: &[SeatBot]) -> bool {
    !SERIAL.get()
        && bots
            .iter()
            .filter(|bot| bot.decision_due(state))
            .take(2)
            .count()
            == 2
}

fn serial_commands(state: &State, bots: &mut [SeatBot]) -> Vec<PlayerCommand> {
    bots.iter_mut().flat_map(|bot| bot.act(state)).collect()
}

#[derive(Default)]
struct BotExecutor {
    pool: Option<Mutex<rayon::ThreadPool>>,
}

impl BotExecutor {
    fn new(workers: usize) -> Self {
        Self {
            pool: (workers > 1)
                .then(|| {
                    rayon::ThreadPoolBuilder::new()
                        .num_threads(workers.min(4))
                        .thread_name(|index| format!("oxide-bot-{index}"))
                        .build()
                        .ok()
                        .map(Mutex::new)
                })
                .flatten(),
        }
    }

    fn commands(&self, state: &State, bots: &mut [SeatBot]) -> Vec<PlayerCommand> {
        if parallel_due(state, bots)
            && let Some(pool) = &self.pool
            // Batch runners already parallelize matches. A busy pool must not
            // serialize those matches behind another match's planning work.
            && let Ok(pool) = pool.try_lock()
        {
            let batches: Vec<Vec<PlayerCommand>> =
                pool.install(|| bots.par_iter_mut().map(|bot| bot.act(state)).collect());
            batches.into_iter().flatten().collect()
        } else {
            serial_commands(state, bots)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::{
        Command, PlayerId, Scenario,
        bot::seat_bots,
        scenario::{BotConfig, BotDifficulty, BotStance},
    };

    #[test]
    fn mixed_cadences_and_post_result_preserve_commands_and_state() {
        for workers in [0, 1, 2, 4] {
            let mut scenario = Scenario::skirmish();
            for (index, player) in scenario.players.iter_mut().enumerate() {
                player.bot = true;
                player.bot_config = Some(BotConfig::scripted(
                    if index == 0 {
                        BotDifficulty::Scrapheap
                    } else {
                        BotDifficulty::Prime
                    },
                    BotStance::Balanced,
                    9000 + index as u64,
                ));
            }
            let mut state = scenario.build().unwrap();
            let mut expected = state.clone();
            let mut bots = seat_bots(&scenario).unwrap();
            bots.reverse();
            let mut expected_bots = bots.clone();
            let executor = BotExecutor::new(workers);
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
                assert_eq!(state.tick(&commands).events, expected.tick(&serial).events);
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
        let mut bots = seat_bots(&scenario).unwrap();
        let mut expected_bots = bots.clone();
        let executor = BotExecutor::new(16);
        let pool = executor.pool.as_ref().unwrap();
        assert_eq!(pool.lock().unwrap().current_num_threads(), 4);
        for _ in 0..180 {
            let commands = if state.current_tick() < 60 {
                serially(|| {
                    assert!(!parallel_due(&state, &bots));
                    executor.commands(&state, &mut bots)
                })
            } else if state.current_tick() < 120 {
                let _busy = pool.lock().unwrap();
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
}
