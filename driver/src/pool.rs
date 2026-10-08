//! The instruments' fan-out: a fixed worker pool pulling independent
//! deterministic sims off one shared queue.
//!
//! Every balance instrument has the same shape — build a job list, play
//! each job in its own sim, fold the results — so the pool lives here
//! once instead of in each of them. Results come back in job order, so a
//! caller's report is a function of its job list and nothing else; the
//! thread count never reaches a verdict.

use anyhow::Result;
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Runs `play` over every job across a worker pool, returning the
/// results in job order.
///
/// The first recorded failure is returned; workers stop picking up new
/// jobs once one exists, though a worker already inside a job finishes
/// it. `play` must be self-contained — the jobs are independent sims,
/// which is what makes the fan-out safe at all.
pub fn fan_out<J, R, F>(jobs: &[J], play: F) -> Result<Vec<R>>
where
    J: Sync,
    R: Send,
    F: Fn(&J) -> Result<R> + Sync,
{
    fan_out_bounded(jobs, NonZeroUsize::MAX, play)
}

/// Runs independent jobs with at most `limit` workers and returns input order.
/// Multiple match workers suppress nested bot-seat parallelism. Large outputs
/// should be staged by `play`; completed results are retained until all jobs end.
pub fn fan_out_bounded<J, R, F>(jobs: &[J], limit: NonZeroUsize, play: F) -> Result<Vec<R>>
where
    J: Sync,
    R: Send,
    F: Fn(&J) -> Result<R> + Sync,
{
    if jobs.is_empty() {
        return Ok(Vec::new());
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(jobs.len()));
    let failure: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let workers = std::thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(limit.get())
        .min(jobs.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let run_jobs = || loop {
                    if failure.lock().unwrap().is_some() {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else {
                        break;
                    };
                    match play(job) {
                        Ok(result) => results.lock().unwrap().push((i, result)),
                        Err(err) => {
                            let mut first = failure.lock().unwrap();
                            if first.is_none() {
                                *first = Some(err);
                            }
                            break;
                        }
                    }
                };
                if workers > 1 {
                    oxide_kit::bot_execution::serially(run_jobs);
                } else {
                    run_jobs();
                }
            });
        }
    });
    if let Some(err) = failure.into_inner().unwrap() {
        return Err(err);
    }
    let mut out = results.into_inner().unwrap();
    out.sort_by_key(|(i, _)| *i);
    Ok(out.into_iter().map(|(_, result)| result).collect())
}

#[cfg(test)]
mod tests;
