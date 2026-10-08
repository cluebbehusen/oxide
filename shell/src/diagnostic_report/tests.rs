use super::*;
use std::time::{Duration, Instant};

fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn home_exports_the_interrupted_match_instead_of_its_empty_active_backdrop() {
    use oxide_kit::recovery::{RecoveryWriter, inspect};
    let root = std::env::temp_dir().join(format!("oxide-report-home-{}", std::process::id()));
    let base = oxide_kit::GameReplay::new(oxide_sim::SIM_VERSION, oxide_sim::Scenario::skirmish());
    let interrupted =
        RecoveryWriter::start(root.clone(), base, 0, crate::build_identity()).unwrap();
    interrupted.prepared(0, &[]);
    interrupted.completed(1);
    until(|| interrupted.status().durable_tick == 1);
    let source = interrupted.directory().to_owned();
    drop(interrupted);
    until(|| {
        std::fs::File::open(source.join("lease"))
            .unwrap()
            .try_lock()
            .is_ok()
    });

    let mut game = crate::game::Game::new(oxide_sim::Scenario::skirmish()).unwrap();
    game.recovery_root = Some(root.clone());
    game.start_recovery();
    until(|| game.recovery.as_ref().unwrap().status().ready);
    let mut job = ReportJob::default();
    job.start(&game).unwrap();
    assert!(
        job.start(&game).is_err(),
        "only one export can run at a time"
    );
    until(|| job.poll().is_some_and(|result| result.is_ok()));
    let report = std::fs::read_dir(root.join("reports"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(inspect(&report).unwrap().replay.meta.ticks, Some(1));
    let active = game.recovery.as_ref().unwrap().directory().to_owned();
    drop(game);
    until(|| {
        std::fs::File::open(active.join("lease"))
            .unwrap()
            .try_lock()
            .is_ok()
    });
    std::fs::remove_dir_all(root).unwrap();
}
