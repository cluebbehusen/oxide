use super::*;
use crate::{GameReplay, recovery};
use oxide_sim::{SIM_VERSION, Scenario, bot::seat_bots};
use std::path::PathBuf;

fn recording() -> (PathBuf, Arc<RecoveryWriter>) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "oxide-diagnostics-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let writer = Arc::new(
        RecoveryWriter::start(
            root.clone(),
            GameReplay::new(SIM_VERSION, Scenario::skirmish()),
            0,
        )
        .unwrap(),
    );
    until(|| writer.status().ready);
    (root, writer)
}
fn until(test: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !test() {
        assert!(Instant::now() < deadline, "diagnostic worker timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn cleanup(root: PathBuf, writer: Arc<RecoveryWriter>, recorder: Recorder) {
    let directory = writer.directory().to_owned();
    drop(recorder);
    drop(writer);
    until(|| recovery::inactive(&directory).is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn observed_parallel_and_serial_commands_match_the_ordinary_controller() {
    let (root, writer) = recording();
    let recorder = Recorder::start(writer.clone()).unwrap();
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
    }
    for serial in [false, true] {
        let mut plain = scenario.build().unwrap();
        let mut observed = plain.clone();
        let mut a = seat_bots(&scenario).unwrap();
        let mut b = a.clone();
        for tick in 0..900 {
            recorder.set_enabled(tick % 100 < 75);
            let expected = crate::bot_execution::commands(&plain, &mut a);
            let actual = if serial {
                crate::bot_execution::serially(|| {
                    crate::bot_execution::commands_observed(&observed, &mut b, Some(&recorder))
                })
            } else {
                crate::bot_execution::commands_observed(&observed, &mut b, Some(&recorder))
            };
            assert_eq!(actual, expected, "tick {tick}");
            let expected = plain.tick(&expected);
            let actual = observed.tick(&actual);
            assert_eq!(expected.events, actual.events);
            assert_eq!(plain.hash(), observed.hash());
        }
    }
    cleanup(root, writer, recorder);
}
#[test]
fn nested_spans_report_exclusive_time_and_do_not_attribute_parallel_work_to_main() {
    let (root, writer) = recording();
    let recorder = Recorder::start(writer.clone()).unwrap();
    let frame = recorder.span(Phase::Frame, 42).unwrap();
    let child = recorder.span(Phase::Simulation, 42).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    drop(child);
    drop(frame);
    until(|| writer.directory().join("timings.json").exists());
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(writer.directory().join("timings.json")).unwrap())
            .unwrap();
    let rows = data["events"].as_array().unwrap();
    let parent = rows.iter().find(|event| event["phase"] == 18).unwrap();
    let child = rows.iter().find(|event| event["phase"] == 12).unwrap();
    assert!(parent["exclusive_us"].as_u64().unwrap() < parent["duration_us"].as_u64().unwrap());
    assert_eq!(child["duration_us"], child["exclusive_us"]);
    assert_eq!(parent["slot"], 0);
    recorder.set_enabled(false);
    assert!(recorder.span(Phase::Simulation, 43).is_none());
    cleanup(root, writer, recorder);
}
#[test]
fn watchdog_survives_a_blocked_journal_writer_and_records_resumption() {
    let (root, writer) = recording();
    let recorder = Recorder::start(writer.clone()).unwrap();
    let budget = std::fs::File::options()
        .read(true)
        .write(true)
        .open(root.join("budget.lock"))
        .unwrap();
    budget.lock().unwrap();
    writer.prepared(0, &[]);
    writer.completed(1);
    let scope = recorder.span(Phase::Simulation, 0).unwrap();
    until(|| writer.directory().join("watchdog.json").exists());
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(writer.directory().join("watchdog.json")).unwrap())
            .unwrap();
    assert_eq!(data[0]["kind"], "suspected stall");
    assert_eq!(data[0]["progress"][0]["phases"][0], 12);
    assert_eq!(writer.status().durable_tick, 0);
    drop(scope);
    budget.unlock().unwrap();
    until(|| {
        std::fs::read_to_string(writer.directory().join("watchdog.json"))
            .unwrap()
            .contains("progress resumed")
    });
    until(|| writer.status().durable_tick == 1);
    cleanup(root, writer, recorder);
}
#[test]
fn optional_event_overflow_is_reported_without_stopping_recovery() {
    let (root, writer) = recording();
    let recorder = Recorder::start(writer.clone()).unwrap();
    // The consumer remains independent; a burst only drops optional samples.
    for _ in 0..30_000 {
        drop(recorder.span(Phase::Screen, 0));
    }
    writer.prepared(0, &[]);
    writer.completed(1);
    until(|| writer.status().durable_tick == 1);
    assert!(writer.status().error.is_none());
    cleanup(root, writer, recorder);
}

#[test]
fn watchdog_reports_a_stalled_bot_worker_and_presentation_wait() {
    let (root, writer) = recording();
    let recorder = Recorder::start(writer.clone()).unwrap();
    let wait = recorder.span(Phase::FrameWait, 30).unwrap();
    let inner = recorder.inner.clone();
    let (release, blocked) = mpsc::channel();
    let bot = std::thread::spawn(move || {
        inner.begin(2, BotPhase::Economy as u8, 30);
        blocked.recv().unwrap();
        inner.end(2, BotPhase::Economy as u8, 30, 0);
    });
    until(|| writer.directory().join("watchdog.json").exists());
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(writer.directory().join("watchdog.json")).unwrap())
            .unwrap();
    let progress = data[0]["progress"].as_array().unwrap();
    assert!(
        progress
            .iter()
            .any(|slot| slot["slot"] == 0 && slot["phases"][0] == 15)
    );
    assert!(
        progress
            .iter()
            .any(|slot| slot["slot"] == 2 && slot["phases"][0] == 6)
    );
    release.send(()).unwrap();
    bot.join().unwrap();
    drop(wait);
    cleanup(root, writer, recorder);
}

#[test]
fn chained_panic_metadata_is_bounded_and_persisted_best_effort() {
    let (root, writer) = recording();
    let recorder = Recorder::start(writer.clone()).unwrap();
    recorder.install_panic_hook();
    let result = std::panic::catch_unwind(|| panic!("diagnostic panic fixture"));
    assert!(result.is_err());
    until(|| writer.directory().join("watchdog.json").exists());
    let data: serde_json::Value =
        serde_json::from_slice(&std::fs::read(writer.directory().join("watchdog.json")).unwrap())
            .unwrap();
    assert_eq!(data[0]["kind"], "panic observed");
    assert_eq!(data[0]["panic"]["message"], "diagnostic panic fixture");
    assert!(data[0]["panic"]["line"].as_u64().unwrap() > 0);
    cleanup(root, writer, recorder);
}

#[test]
fn watchdog_updates_when_stalled_workers_change_without_full_resumption() {
    let (root, writer) = recording();
    let recorder = Recorder::start(writer.clone()).unwrap();
    let first = recorder.inner.micros();
    recorder.inner.begin(1, BotPhase::Economy as u8, 30);
    let path = writer.directory().join("watchdog.json");
    until(|| path.exists());
    let incidents = || -> Vec<serde_json::Value> {
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
    };
    let stalled_slots = |incident: &serde_json::Value| -> Vec<u64> {
        incident["progress"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| slot["idle_ms"].as_u64().unwrap() >= 5000)
            .map(|slot| slot["slot"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(stalled_slots(&incidents()[0]), [1]);

    let second = recorder.inner.micros();
    recorder.inner.begin(2, BotPhase::Executive as u8, 30);
    // The first incident establishes enough elapsed time to age the second slot.
    recorder.inner.slots[2]
        .progress
        .store(recorder.inner.micros() - 5_000_000, Ordering::Release);
    until(|| incidents().len() >= 2);
    assert_eq!(stalled_slots(&incidents()[1]), [1, 2]);

    recorder.inner.end(1, BotPhase::Economy as u8, 30, first);
    until(|| incidents().len() >= 3);
    let data = incidents();
    assert_eq!(data[2]["kind"], "suspected stall");
    assert_eq!(stalled_slots(&data[2]), [2]);
    assert_eq!(data[2]["progress"].as_array().unwrap().len(), 1);

    recorder.inner.end(2, BotPhase::Executive as u8, 30, second);
    until(|| incidents().len() >= 4);
    assert_eq!(incidents()[3]["kind"], "progress resumed");
    cleanup(root, writer, recorder);
}

#[test]
fn every_shell_screen_keeps_its_identity_in_persisted_context() {
    let (root, writer) = recording();
    let recorder = Recorder::start(writer.clone()).unwrap();
    for (mode, id) in [
        ("playing", 1),
        ("pause", 2),
        ("playback", 3),
        ("home", 4),
        ("settings", 5),
        ("wizard", 6),
        ("codex", 7),
        ("replays", 8),
        ("results", 9),
        ("final_map", 10),
    ] {
        recorder.frame(FrameContext {
            mode,
            tick: 0,
            units: 0,
            buildings: 0,
            speed: 1.0,
            width: 1280,
            height: 800,
            dpi: 1.0,
            paused: true,
            minimized: None,
        });
        let context = recorder.inner.context();
        assert_eq!(context["screen"], id);
        assert_eq!(
            context["screen_names"][id.to_string()],
            if mode == "pause" { "paused" } else { mode }
        );
        persist(&recorder.inner, "context.json", &context);
        let stored: serde_json::Value = serde_json::from_slice(
            &std::fs::read(writer.directory().join("context.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored["screen"], id);
    }
    cleanup(root, writer, recorder);
}
