use super::*;
use crate::{GameReplay, recovery};
use oxide_sim::{SIM_VERSION, Scenario};

fn temp() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "oxide-diagnostics-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn build() -> BuildIdentity {
    BuildIdentity::new("fixture", "diagnostic-host", "false")
}

fn recording(root: &Path) -> Arc<RecoveryWriter> {
    let writer = Arc::new(
        RecoveryWriter::start(
            root.to_owned(),
            GameReplay::new(SIM_VERSION, Scenario::skirmish()),
            0,
            build(),
        )
        .unwrap(),
    );
    until(|| writer.status().ready);
    writer
}

fn until(test: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !test() {
        assert!(Instant::now() < deadline, "diagnostic watchdog timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn context(tick: u64, recording: Option<&Arc<RecoveryWriter>>) -> FrameContext<'_> {
    FrameContext {
        screen: "playing",
        tick,
        units: 12,
        buildings: 3,
        speed: 1.0,
        width: 1280,
        height: 800,
        dpi: 2.0,
        paused: false,
        minimized: Some(false),
        recording,
    }
}

fn incidents(directory: &Path) -> Vec<Value> {
    read_log(&directory.join(INCIDENTS))
        .map(|log| log.incidents)
        .unwrap_or_default()
}

fn has(directory: &Path, kind: &str, thread: &str) -> bool {
    incidents(directory)
        .iter()
        .any(|incident| incident["kind"] == kind && incident["thread"] == thread)
}

#[test]
fn a_main_thread_stall_and_its_resumption_land_in_the_open_recording() {
    let root = temp();
    let writer = recording(&root);
    let directory = writer.directory().to_owned();
    let monitor =
        Monitor::start_with(Some(root.clone()), build(), Duration::from_millis(100)).unwrap();
    monitor.frame(context(7, Some(&writer)));
    monitor.frame(context(8, Some(&writer)));
    let simulation = monitor.stage(Stage::Simulation, 8).unwrap();
    until(|| has(&directory, "stall", "main"));
    drop(simulation);
    until(|| has(&directory, "resumed", "main"));

    let log = incidents(&directory);
    let stall = log
        .iter()
        .find(|incident| incident["kind"] == "stall")
        .unwrap();
    assert_eq!(stall["stage"], "simulation");
    assert_eq!(stall["tick"], 8);
    assert!(stall["stalled_ms"].as_u64().unwrap() >= 100);
    assert_eq!(stall["build"]["revision"], "diagnostic-host");
    assert_eq!(stall["context"]["screen"], "playing");
    assert_eq!(stall["context"]["units"], 12);
    assert_eq!(stall["recent"]["frames"].as_array().unwrap().len(), 2);
    assert_eq!(stall["recent"]["seconds"][0]["ticks"], 1);
    let resumed = log
        .iter()
        .find(|incident| incident["kind"] == "resumed")
        .unwrap();
    assert_eq!(resumed["stage"], "simulation");
    assert!(resumed.get("recent").is_none());
    assert!(
        !root.join(INCIDENTS).exists(),
        "an open recording keeps its own incidents"
    );

    writer.finish(0);
    until(|| writer.status().clean);
    let stalled = monitor.stage(Stage::Draw, 8).unwrap();
    until(|| has(&root, "stall", "main"));
    drop(stalled);
    drop(monitor);
    drop(writer);
    until(|| recovery::inactive(&directory).is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_incident_before_a_recording_is_ready_still_sets_its_ending() {
    let root = temp();
    std::fs::create_dir_all(&root).unwrap();
    // Admission waits on the budget lock, so the recording cannot become ready.
    let budget = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("budget.lock"))
        .unwrap();
    budget.lock().unwrap();
    let writer = Arc::new(
        RecoveryWriter::start(
            root.clone(),
            GameReplay::new(SIM_VERSION, Scenario::skirmish()),
            0,
            build(),
        )
        .unwrap(),
    );
    let monitor =
        Monitor::start_with(Some(root.clone()), build(), Duration::from_secs(60)).unwrap();
    monitor.frame(context(0, None));
    monitor.attach(&writer);
    monitor
        .inner
        .record_panic("first tick", None, "backtrace".into());
    let directory = writer.directory().to_owned();
    let session = directory.file_name().unwrap().to_str().unwrap();
    assert_eq!(incidents(&root)[0]["session"], session);
    assert!(!directory.join(INCIDENTS).exists());

    budget.unlock().unwrap();
    until(|| writer.status().ready);
    monitor.frame(context(1, None));
    drop(writer);
    until(|| recovery::inactive(&directory).is_some());
    let record = recovery::inspect(&directory).unwrap();
    assert_eq!(recovery::ending(&directory, &record), Ending::Panic);
    drop(monitor);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_panic_while_writing_an_incident_does_not_wait_on_its_own_lock() {
    let root = temp();
    let monitor =
        Monitor::start_with(Some(root.clone()), build(), Duration::from_secs(60)).unwrap();
    let held = monitor.inner.writing.lock().unwrap();
    let outer = Writing::enter();
    let started = Instant::now();
    monitor
        .inner
        .record_panic("inside a write", None, "backtrace".into());
    assert!(started.elapsed() < WRITE_WAIT / 2);
    assert_eq!(incidents(&root)[0]["panic"]["message"], "inside a write");
    drop(outer);
    assert!(
        !Writing::enter().0,
        "leaving the outer write clears the mark"
    );
    drop(held);
    drop(monitor);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_busy_bot_seat_reports_its_stall_from_any_thread() {
    let root = temp();
    let monitor =
        Monitor::start_with(Some(root.clone()), build(), Duration::from_millis(100)).unwrap();
    std::thread::scope(|scope| {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let monitor = &monitor;
        let worker = scope.spawn(move || {
            let _seat = monitor.seat(3, 42).unwrap();
            wait.recv().unwrap();
        });
        until(|| has(&root, "stall", "bot seat 3"));
        release.send(()).unwrap();
        worker.join().unwrap();
    });
    until(|| has(&root, "resumed", "bot seat 3"));
    let log = incidents(&root);
    assert_eq!(log[0]["tick"], 42);
    assert_eq!(log[0]["seats"][0]["seat"], 3);
    assert!(
        log.iter().all(|incident| incident["thread"] != "main"),
        "the main thread is unwatched until its first frame"
    );
    assert!(monitor.seat(SEATS as u8, 0).is_none());
    drop(monitor);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_window_minimized_after_its_last_frame_is_not_a_stall() {
    let root = temp();
    let monitor =
        Monitor::start_with(Some(root.clone()), build(), Duration::from_millis(50)).unwrap();
    monitor.frame(context(0, None));
    let present = monitor.stage(Stage::Present, 0).unwrap();
    monitor.minimized(true);
    std::thread::sleep(Duration::from_millis(300));
    assert!(incidents(&root).is_empty());
    drop(present);
    drop(monitor);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn frame_stage_times_partition_the_frame_interval() {
    let monitor = Monitor::start_with(None, build(), Duration::from_secs(60)).unwrap();
    monitor.frame(context(0, None));
    {
        let _input = monitor.stage(Stage::Input, 0).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let _simulation = monitor.stage(Stage::Simulation, 0).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    monitor.frame(context(1, None));
    let recent = monitor.inner.recent.lock().unwrap().json();
    let frame = &recent["frames"][1];
    let stages = frame["stages_us"].as_object().unwrap();
    assert!(stages["input"].as_u64().unwrap() >= 5_000);
    assert!(stages["simulation"].as_u64().unwrap() >= 10_000);
    assert_eq!(
        stages
            .values()
            .map(|value| value.as_u64().unwrap())
            .sum::<u64>(),
        frame["interval_us"].as_u64().unwrap(),
        "nested stages are counted once"
    );
    assert_eq!(recent["seconds"].as_array().unwrap().len(), 1);
}

#[test]
fn other_threads_cannot_move_the_main_thread_heartbeat() {
    let monitor = Monitor::start_with(None, build(), Duration::from_secs(60)).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            assert!(monitor.stage(Stage::Simulation, 9).is_none());
            monitor.progress(9);
            monitor.frame(context(9, None));
        });
    });
    assert_eq!(monitor.inner.main.tick.load(Ordering::Relaxed), 0);
    assert!(!monitor.inner.main.armed.load(Ordering::Relaxed));
    assert!(monitor.inner.recent.lock().unwrap().frames.is_empty());
}

#[test]
fn panics_record_their_message_location_and_bounded_backtrace() {
    let root = temp();
    let monitor =
        Monitor::start_with(Some(root.clone()), build(), Duration::from_secs(60)).unwrap();
    monitor.frame(context(5, None));
    let simulation = monitor.stage(Stage::Simulation, 5).unwrap();
    monitor.inner.record_panic(
        "boom",
        Some("sim/src/tick.rs:1:2".into()),
        "é".repeat(20_000),
    );
    let log = incidents(&root);
    let panic = &log[0];
    assert_eq!(panic["kind"], "panic");
    assert_eq!(
        panic["thread"],
        std::thread::current().name().unwrap_or("unnamed")
    );
    assert_eq!(panic["panic"]["message"], "boom");
    assert_eq!(panic["panic"]["location"], "sim/src/tick.rs:1:2");
    let backtrace = panic["panic"]["backtrace"].as_str().unwrap();
    assert!(!backtrace.is_empty() && backtrace.len() <= MAX_BACKTRACE_BYTES);
    assert_eq!(panic["main"]["stage"], "simulation");
    assert_eq!(panic["main"]["tick"], 5);
    drop(simulation);
    drop(monitor);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "subprocess fixture; invoked by an_installed_monitor_records_a_panic_before_its_process_dies"]
fn panicking_child() {
    let root = PathBuf::from(std::env::var_os("OXIDE_DIAGNOSTICS_TEST_ROOT").unwrap());
    let monitor = Monitor::start(Some(root), build()).unwrap();
    assert!(monitor.install().is_ok());
    let _simulation = stage(Stage::Simulation, 77);
    panic!("child boom");
}

#[test]
fn an_installed_monitor_records_a_panic_before_its_process_dies() {
    let root = temp();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "diagnostics::tests::panicking_child",
            "--ignored",
        ])
        .env("OXIDE_DIAGNOSTICS_TEST_ROOT", &root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    let log = incidents(&root);
    let [panic] = log.as_slice() else {
        panic!("one panic incident: {log:?}");
    };
    assert_eq!(panic["kind"], "panic");
    assert_eq!(panic["panic"]["message"], "child boom");
    assert!(
        panic["panic"]["location"]
            .as_str()
            .unwrap()
            .contains("diagnostics")
    );
    assert!(!panic["panic"]["backtrace"].as_str().unwrap().is_empty());
    assert_eq!(panic["main"]["stage"], "simulation");
    assert_eq!(panic["main"]["tick"], 77);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn incident_logs_keep_the_newest_entries_within_their_bound() {
    let root = temp();
    std::fs::create_dir_all(&root).unwrap();
    for index in 0..MAX_INCIDENTS + 4 {
        append(&root, json!({ "kind": "stall", "index": index })).unwrap();
    }
    let log = read_log(&root.join(INCIDENTS)).unwrap();
    assert_eq!(log.dropped, 4);
    assert_eq!(log.incidents.len(), MAX_INCIDENTS);
    assert_eq!(log.incidents[0]["index"], 4);
    std::fs::write(root.join(INCIDENTS), b"not json").unwrap();
    append(&root, json!({ "kind": "panic" })).unwrap();
    assert_eq!(incidents(&root).len(), 1, "a damaged log restarts");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn endings_follow_the_last_unresolved_incident() {
    let stall = |thread: &str| json!({ "kind": "stall", "thread": thread, "stage": "draw" });
    let waiting = json!({ "kind": "stall", "thread": "main", "stage": "present" });
    let resumed = |thread: &str| json!({ "kind": "resumed", "thread": thread });
    let panic = json!({ "kind": "panic", "thread": "main" });
    assert_eq!(concluding(&[]), Ending::Abnormal);
    assert_eq!(concluding(&[stall("main")]), Ending::Froze);
    assert_eq!(
        concluding(&[stall("main"), resumed("main")]),
        Ending::Abnormal
    );
    assert_eq!(
        concluding(&[stall("main"), stall("bot seat 1"), resumed("bot seat 1")]),
        Ending::Froze
    );
    assert_eq!(concluding(&[stall("main"), panic.clone()]), Ending::Panic);
    assert_eq!(
        concluding(std::slice::from_ref(&waiting)),
        Ending::AwaitingFrame
    );
    assert_eq!(
        concluding(&[waiting, stall("bot seat 2")]),
        Ending::Froze,
        "a stuck seat is a freeze even while the main thread waits"
    );
    assert_eq!(
        concluding(&[panic, stall("main"), resumed("main")]),
        Ending::Abnormal,
        "a caught panic followed by recovery does not end the session"
    );
    let root = temp();
    assert_eq!(ending(&root, None, true, false), Ending::Clean);
    assert_eq!(ending(&root, None, false, true), Ending::InProgress);
    assert_eq!(ending(&root, None, false, false), Ending::Abnormal);
}
