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
        BotExecutor::new(std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
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
                        .thread_name(|index| format!("bot-decision-{index}"))
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
mod tests;

#[cfg(test)]
mod background_tests;
