use super::*;
use std::time::{Duration, Instant};

fn completion(worker: &mut Worker) -> (u64, Result<Output>) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(result) = worker.poll() {
            return result;
        }
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
}

#[test]
fn cancellation_keeps_admission_until_the_worker_releases_its_capture() {
    let mut worker = Worker::new().unwrap();
    let (started, observed) = mpsc::sync_channel(1);
    let (release, blocked) = mpsc::sync_channel(1);
    let id = worker
        .start(Box::new(move |cancel| {
            started.send(()).unwrap();
            blocked.recv().unwrap();
            anyhow::ensure!(!cancel.cancelled(), "cancelled");
            Ok(Output::Saved)
        }))
        .unwrap();
    observed.recv().unwrap();
    assert!(worker.cancel());
    assert!(worker.start(Box::new(|_| Ok(Output::Saved))).is_err());
    release.send(()).unwrap();
    let (finished, result) = completion(&mut worker);
    assert_eq!(id, finished);
    assert!(result.is_err());
    assert!(!worker.busy());
    let next = worker
        .start(Box::new(|cancel| {
            cancel.commit()?;
            assert!(!cancel.cancel());
            Ok(Output::Saved)
        }))
        .unwrap();
    assert_ne!(id, next);
    assert!(completion(&mut worker).1.is_ok());
}

#[test]
fn a_failed_job_does_not_poison_the_worker() {
    let mut worker = Worker::new().unwrap();
    worker
        .start(Box::new(|_| panic!("injected worker failure")))
        .unwrap();
    assert!(completion(&mut worker).1.is_err());
    worker.start(Box::new(|_| Ok(Output::Saved))).unwrap();
    assert!(completion(&mut worker).1.is_ok());
}

#[test]
fn continue_skips_a_corrupt_newest_payload_and_reuses_the_restored_older_save() {
    let dir = std::env::temp_dir().join(format!("oxide-continue-worker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut game = Game::new(Scenario::skirmish()).unwrap();
    game.advance_ticks(120);
    let mut meta = game.recorder.meta.clone();
    meta.kind = Some("autosave".into());
    meta.ticks = Some(game.state.current_tick());
    let older = dir.join("older.oxsave");
    crate::saved_game::write_capture(game.capture_save(), meta, &older).unwrap();
    let newer = dir.join("newer.oxsave");
    let mut corrupt = std::fs::read(&older).unwrap();
    *corrupt.last_mut().unwrap() ^= 1;
    std::fs::write(&newer, corrupt).unwrap();
    assert!(
        crate::saved_game::inspect(&newer)
            .unwrap()
            .problem
            .is_none()
    );
    let Output::Loaded(restored) = load_candidates(
        vec![newer, older.clone()],
        false,
        None,
        &Cancellation::default(),
    )
    .unwrap() else {
        panic!()
    };
    let mut restored = restored.install();
    for _ in 0..120 {
        assert_eq!(game.do_tick().events, restored.do_tick().events);
        assert_eq!(game.hash_hex(), restored.hash_hex());
    }
    let cancelled = Cancellation::default();
    assert!(cancelled.cancel());
    assert!(load_candidates(vec![older], false, Some(dir.join("recovery")), &cancelled).is_err());
    assert!(!dir.join("recovery").exists());
    std::fs::remove_dir_all(dir).unwrap();
}
