use super::*;

#[test]
fn worker_limit_bounds_live_jobs() {
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let jobs: Vec<_> = (0..16).collect();
    let out = fan_out_bounded(&jobs, NonZeroUsize::new(2).unwrap(), |&job| {
        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(count, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(2));
        active.fetch_sub(1, Ordering::SeqCst);
        Ok(job)
    })
    .unwrap();
    assert_eq!(out, jobs);
    assert!(peak.load(Ordering::SeqCst) <= 2);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

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
        }
        Ok(j)
    })
    .unwrap_err();
    assert!(err.to_string().contains("job 17 refused"));
}
