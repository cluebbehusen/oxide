//! Ordered bot command collection for live and headless sessions.

use std::cell::Cell;
use std::sync::{
    Arc, OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::controller::SeatController;
use oxide_sim::{PlayerCommand, State};
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
pub fn commands(state: &State, bots: &mut [SeatController]) -> Vec<PlayerCommand> {
    if !parallel_due(state, bots) {
        return serial_commands(state, bots);
    }
    executor().commands(state, bots)
}

fn executor() -> &'static BotExecutor {
    static EXECUTOR: OnceLock<BotExecutor> = OnceLock::new();
    EXECUTOR.get_or_init(|| {
        BotExecutor::new(std::thread::available_parallelism().map_or(1, |n| n.get()))
    })
}

/// Speculate from a retained pre-decision controller snapshot. A busy executor,
/// serial batch policy or tick with no due seat leaves the caller unchanged.
/// Dropping the handle discards the result; the bounded worker keeps its permit
/// until it finishes, so abandoned sessions cannot accumulate queued jobs.
pub fn prepare(state: &Arc<State>, bots: &[SeatController]) -> Option<PendingDecision> {
    if may_prepare(state, bots) {
        executor().prepare(state, bots)
    } else {
        None
    }
}

struct Decision {
    bots: Vec<SeatController>,
    commands: Vec<PlayerCommand>,
}

/// One session's speculative batch. Its world identity and tick are checked
/// before consumption; it is never part of a checkpoint.
pub struct PendingDecision {
    world: Weak<State>,
    tick: u64,
    result: mpsc::Receiver<std::thread::Result<Decision>>,
}

impl PendingDecision {
    /// Join once, install complete controllers, then return their ordered commands.
    /// Worker failure propagates without installing partially advanced controllers.
    pub fn finish(self, state: &Arc<State>, bots: &mut Vec<SeatController>) -> Vec<PlayerCommand> {
        assert!(
            self.world.ptr_eq(&Arc::downgrade(state)),
            "bot batch belongs to another world"
        );
        assert_eq!(self.tick, state.current_tick(), "bot batch tick changed");
        let result = self.result.recv().expect("bot worker disconnected");
        let decision = result.unwrap_or_else(|failure| std::panic::resume_unwind(failure));
        assert!(
            bots.iter()
                .map(SeatController::player)
                .eq(decision.bots.iter().map(SeatController::player)),
            "bot batch roster changed"
        );
        *bots = decision.bots;
        decision.commands
    }
}

fn may_prepare(state: &State, bots: &[SeatController]) -> bool {
    !SERIAL.get() && bots.iter().any(|bot| bot.decision_due(state))
}

fn parallel_due(state: &State, bots: &[SeatController]) -> bool {
    !SERIAL.get()
        && bots
            .iter()
            .filter(|bot| bot.decision_due(state))
            .take(2)
            .count()
            == 2
}

fn serial_commands(state: &State, bots: &mut [SeatController]) -> Vec<PlayerCommand> {
    bots.iter_mut().flat_map(|bot| act(state, bot)).collect()
}

fn act(state: &State, bot: &mut SeatController) -> Vec<PlayerCommand> {
    let _seat = crate::diagnostics::seat(bot.player().0, state.current_tick());
    bot.act(state)
}

#[derive(Default)]
struct BotExecutor {
    pool: Option<rayon::ThreadPool>,
    busy: Arc<AtomicBool>,
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
                })
                .flatten(),
            busy: Arc::default(),
        }
    }

    fn acquire(&self) -> Option<Permit> {
        self.busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Permit(self.busy.clone()))
    }

    fn prepare(&self, state: &Arc<State>, bots: &[SeatController]) -> Option<PendingDecision> {
        if !may_prepare(state, bots) {
            return None;
        }
        let pool = self.pool.as_ref()?;
        let permit = self.acquire()?;
        let pending_world = Arc::downgrade(state);
        let tick = state.current_tick();
        let state = state.clone();
        let mut bots = bots.to_vec();
        let (send, result) = mpsc::channel();
        pool.spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let batches: Vec<_> = bots.par_iter_mut().map(|bot| act(&state, bot)).collect();
                let commands = batches.into_iter().flatten().collect();
                drop(state);
                Decision { bots, commands }
            }));
            drop(permit);
            let _ = send.send(result);
        });
        Some(PendingDecision {
            world: pending_world,
            tick,
            result,
        })
    }

    fn commands(&self, state: &State, bots: &mut [SeatController]) -> Vec<PlayerCommand> {
        if parallel_due(state, bots)
            && let Some(pool) = &self.pool
            // Batch runners already parallelize matches. A busy pool must not
            // serialize those matches behind another match's planning work.
            && let Some(_permit) = self.acquire()
        {
            let batches: Vec<Vec<PlayerCommand>> =
                pool.install(|| bots.par_iter_mut().map(|bot| act(state, bot)).collect());
            batches.into_iter().flatten().collect()
        } else {
            serial_commands(state, bots)
        }
    }
}

struct Permit(Arc<AtomicBool>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::{record_events, seat_controllers};
    use oxide_sim::{
        Command, PlayerId, Scenario,
        scenario::{BotConfig, BotDifficulty, BotStance},
    };

    #[test]
    fn mixed_controllers_cadences_and_post_result_preserve_commands_and_state() {
        for workers in [0, 1, 2, 4] {
            let mut scenario = Scenario::skirmish();
            for player in &mut scenario.players {
                player.bot = true;
            }
            scenario.players[0].bot_config = Some(BotConfig::opponent(
                BotDifficulty::Scrapheap,
                BotStance::Balanced,
                9000,
            ));
            scenario.players[1].bot_config = Some(BotConfig::scripted(
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
                        .map(|bot| bot.checkpoint().unwrap())
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
}

#[cfg(test)]
mod background_tests;
