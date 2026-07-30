//! The instruments' fan-out: a fixed worker pool pulling independent
//! deterministic sims off one shared queue.
//!
//! Every balance instrument has the same shape — build a job list, play
//! each job in its own sim, fold the results — so the pool lives here
//! once instead of in each of them. Results come back in job order, so a
//! caller's report is a function of its job list and nothing else; the
//! thread count never reaches a verdict.

use anyhow::Result;
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
    if jobs.is_empty() {
        return Ok(Vec::new());
    }
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, R)>> = Mutex::new(Vec::with_capacity(jobs.len()));
    let failure: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(jobs.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
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
mod tests {
    use super::*;

    #[test]
    fn results_come_back_in_job_order() {
        let jobs: Vec<u64> = (0..500).collect();
        let out = fan_out(&jobs, |&j| Ok(j * 2)).unwrap();
        assert_eq!(out, jobs.iter().map(|j| j * 2).collect::<Vec<_>>());
    }

    #[test]
    fn an_empty_queue_spawns_nothing() {
        let jobs: Vec<u64> = Vec::new();
        assert!(fan_out(&jobs, |_| Ok(0u64)).unwrap().is_empty());
    }

    #[test]
    fn a_failing_job_surfaces_its_error() {
        let jobs: Vec<u64> = (0..64).collect();
        let err = fan_out(&jobs, |&j| {
            if j == 17 {
                anyhow::bail!("job {j} refused")
            } else {
                Ok(j)
            }
        })
        .unwrap_err();
        assert!(err.to_string().contains("job 17 refused"));
    }
}
