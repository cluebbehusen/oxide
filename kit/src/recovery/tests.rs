use super::*;
use oxide_sim::{Command, PlayerCommand, PlayerId, Scenario};
use std::time::{Duration, Instant};

fn base() -> GameReplay {
    let mut replay = GameReplay::new(SIM_VERSION, Scenario::skirmish());
    replay.meta.ticks = Some(0);
    replay
}
fn encoded(events: Vec<Event>) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    write_frame(
        &mut bytes,
        &Header {
            kind: RecordingKind::LiveMatch,
            session: "test".into(),
            build: BuildIdentity::default(),
            base: base(),
        },
    )
    .unwrap();
    for (sequence, event) in events.into_iter().enumerate() {
        write_frame(
            &mut bytes,
            &Record {
                session: "test".into(),
                sequence: sequence as u64,
                event,
            },
        )
        .unwrap();
    }
    bytes
}
fn command() -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(0),
        command: Command::Train {
            building: oxide_sim::BuildingId(0),
            kind: oxide_sim::UnitKind::Harvester,
        },
    }
}
fn temp() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "oxide-recovery-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}
fn wait(writer: &RecoveryWriter, test: impl Fn(&WriterStatus) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !test(&writer.status()) {
        assert!(Instant::now() < deadline, "{:?}", writer.status());
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn drop_and_wait(writer: RecoveryWriter) {
    let directory = writer.directory().to_owned();
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !directory.join("status.json").exists() || inactive(&directory).is_none() {
        assert!(
            Instant::now() < deadline,
            "recovery worker did not finish shutdown: {}",
            directory.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn every_truncated_tail_keeps_only_completed_commands() {
    let bytes = encoded(vec![
        Event::Prepared {
            tick: 0,
            commands: vec![command()],
        },
        Event::Completed { tick: 1 },
        Event::Prepared {
            tick: 1,
            commands: vec![],
        },
    ]);
    let header_length = encoded(vec![]).len();
    for end in header_length..=bytes.len() {
        let record = inspect_reader(&mut &bytes[..end]).unwrap();
        let tick = record.replay.meta.ticks.unwrap();
        assert!(tick <= 1);
        assert_eq!(record.replay.commands.len(), tick as usize);
        if tick == 1 {
            assert_eq!(record.replay.commands[0].command, command());
        }
        record.replay.validate(Some(SIM_VERSION)).unwrap();
    }
    let record = inspect_reader(&mut bytes.as_slice()).unwrap();
    assert_eq!(record.prepared, Some(vec![]));
    assert!(!record.clean);
}
#[test]
fn corrupt_and_out_of_order_records_do_not_extend_the_prefix() {
    for bad in [
        Event::Completed { tick: 1 },
        Event::Prepared {
            tick: 2,
            commands: vec![],
        },
        Event::Clean { tick: 1 },
    ] {
        let record = inspect_reader(&mut encoded(vec![bad]).as_slice()).unwrap();
        assert!(record.issue.is_some());
        assert_eq!(record.replay.meta.ticks, Some(0));
    }
    let mut bytes = encoded(vec![
        Event::Prepared {
            tick: 0,
            commands: vec![],
        },
        Event::Completed { tick: 1 },
    ]);
    *bytes.last_mut().unwrap() ^= 1;
    let record = inspect_reader(&mut bytes.as_slice()).unwrap();
    assert!(record.issue.unwrap().contains("checksum"));
    assert_eq!(record.replay.meta.ticks, Some(0));
    let mut bytes = encoded(vec![]);
    write_frame(
        &mut bytes,
        &Record {
            session: "other".into(),
            sequence: 0,
            event: Event::Clean { tick: 0 },
        },
    )
    .unwrap();
    assert!(
        inspect_reader(&mut bytes.as_slice())
            .unwrap()
            .issue
            .is_some()
    );
    let mut bytes = encoded(vec![]);
    write_frame(
        &mut bytes,
        &Record {
            session: "test".into(),
            sequence: 1,
            event: Event::Clean { tick: 0 },
        },
    )
    .unwrap();
    assert!(
        inspect_reader(&mut bytes.as_slice())
            .unwrap()
            .issue
            .is_some()
    );
}
#[test]
fn empty_ticks_and_resumed_prefixes_retain_their_duration() {
    let record = inspect_reader(
        &mut encoded(vec![
            Event::Prepared {
                tick: 0,
                commands: vec![],
            },
            Event::Completed { tick: 1 },
            Event::Clean { tick: 1 },
        ])
        .as_slice(),
    )
    .unwrap();
    assert_eq!(record.replay.meta.ticks, Some(1));
    assert!(record.clean);
    let mut bytes = MAGIC.to_vec();
    write_frame(
        &mut bytes,
        &Header {
            kind: RecordingKind::LiveMatch,
            session: "resume".into(),
            build: BuildIdentity::default(),
            base: record.replay,
        },
    )
    .unwrap();
    for (sequence, event) in [
        Event::Prepared {
            tick: 1,
            commands: vec![command()],
        },
        Event::Completed { tick: 2 },
    ]
    .into_iter()
    .enumerate()
    {
        write_frame(
            &mut bytes,
            &Record {
                session: "resume".into(),
                sequence: sequence as u64,
                event,
            },
        )
        .unwrap();
    }
    let record = inspect_reader(&mut bytes.as_slice()).unwrap();
    assert_eq!(record.replay.meta.ticks, Some(2));
    assert_eq!(record.replay.commands[0].tick, 1);
}
#[test]
fn worker_flushes_exact_prefix_and_excludes_active_recordings() {
    let root = temp();
    let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
    let mut state = Scenario::skirmish().build().unwrap();
    for tick in 0..20 {
        let commands = if tick == 0 { vec![command()] } else { vec![] };
        writer.prepared(tick, &commands);
        state.tick(&commands);
        writer.completed(tick + 1);
    }
    wait(&writer, |s| s.durable_tick == 20);
    assert!(latest_interrupted(&root).is_none());
    let directory = writer.directory().to_path_buf();
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while inactive(&directory).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let found = latest_interrupted(&root).unwrap();
    assert_eq!(found.ticks, 20);
    let replay = inspect(&directory).unwrap().replay;
    let mut reproduced = replay.setup.build().unwrap();
    let mut cursor = replay.cursor();
    for tick in 0..20 {
        let commands = cursor
            .take_tick(tick)
            .iter()
            .map(|c| c.command.clone())
            .collect::<Vec<_>>();
        reproduced.tick(&commands);
    }
    assert_eq!(state.hash(), reproduced.hash());
    let report = root.join("export");
    export(&directory, &report).unwrap();
    assert!(crate::load_replay(report.join("replay.json")).is_ok());
    assert_eq!(inspect(&report).unwrap().replay.meta.ticks, Some(20));
    assert!(export(&directory, &report).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn clean_close_requires_success_and_storage_failure_does_not_block() {
    let root = temp();
    let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
    writer.prepared(0, &[]);
    writer.completed(1);
    writer.finish(1);
    wait(&writer, |s| s.clean);
    assert!(inspect(writer.directory()).unwrap().clean);
    assert!(latest_interrupted(&root).is_none());
    assert!(inactive(writer.directory()).is_none());
    drop_and_wait(writer);
    let bad = root.join("file");
    std::fs::write(&bad, b"not a directory").unwrap();
    let writer = RecoveryWriter::start(bad, base(), 0).unwrap();
    wait(&writer, |s| s.error.is_some());
    writer.prepared(0, &[]);
    assert!(!writer.status().clean);
    drop(writer);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn oversized_command_batch_stops_capture_without_partial_submission() {
    let root = temp();
    let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
    let commands = vec![PlayerCommand {
        player: PlayerId(0),
        command: Command::Stop {
            units: vec![oxide_sim::UnitId(0); 100_000],
        },
    }];
    writer.prepared(0, &commands);
    assert!(writer.status().error.is_some());
    assert_eq!(writer.status().pending_bytes, 0);
    writer.completed(1);
    let directory = writer.directory().to_owned();
    drop_and_wait(writer);
    assert_eq!(inspect(&directory).unwrap().replay.meta.ticks, Some(0));
    std::fs::remove_dir_all(root).unwrap();
}

static ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(super) fn fault(phase: &str) {
    if ARMED.load(std::sync::atomic::Ordering::Acquire)
        && std::env::var("OXIDE_RECOVERY_TEST_PHASE").is_ok_and(|value| value == phase)
    {
        std::fs::write(
            std::env::var_os("OXIDE_RECOVERY_TEST_READY").unwrap(),
            b"ready",
        )
        .unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}
#[test]
#[ignore = "subprocess fixture; invoked by forced_termination_retains_a_valid_prefix"]
fn interrupted_child() {
    let root = PathBuf::from(std::env::var_os("OXIDE_RECOVERY_TEST_ROOT").unwrap());
    let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
    for tick in 0..10 {
        writer.prepared(tick, &[]);
        writer.completed(tick + 1);
    }
    wait(&writer, |status| status.durable_tick == 10);
    let phase = std::env::var("OXIDE_RECOVERY_TEST_PHASE").unwrap();
    ARMED.store(true, std::sync::atomic::Ordering::Release);
    match phase.as_str() {
        "export" => {
            let _ = export(writer.directory(), &root.join("report"));
        }
        "clean" => writer.finish(10),
        "prepared" => {
            writer.prepared(10, &[command()]);
            fault("prepared");
        }
        "journal-write" => {
            writer.prepared(10, &[command()]);
            writer.completed(11);
        }
        _ => panic!("unknown fixture phase"),
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}
#[test]
fn forced_termination_retains_a_valid_prefix() {
    for phase in ["prepared", "journal-write", "clean", "export"] {
        let root = temp();
        let ready = root.join("ready");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "recovery::tests::interrupted_child",
                "--ignored",
                "--nocapture",
            ])
            .env("OXIDE_RECOVERY_TEST_ROOT", &root)
            .env("OXIDE_RECOVERY_TEST_READY", &ready)
            .env("OXIDE_RECOVERY_TEST_PHASE", phase)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child did not reach {phase}");
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("child exited before reaching {phase}: {status}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let recovered = latest_interrupted(&root).unwrap();
        assert_eq!(recovered.ticks, 10, "{phase}");
        assert!(
            inspect(&recovered.directory)
                .unwrap()
                .replay
                .commands
                .is_empty()
        );
        if phase == "export" {
            assert!(
                inspect(&root.join("report")).is_err(),
                "interrupted export cannot appear complete"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn recovered_sources_retire_only_after_an_exact_replacement_is_durable() {
    let root = temp();
    let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
    writer.prepared(0, &[]);
    writer.completed(1);
    wait(&writer, |status| status.durable_tick == 1);
    let source = writer.directory().to_owned();
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while read_lease(&source).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(source.join("watchdog.json"), b"[\"original stall\"]").unwrap();
    let recovered = inspect(&source).unwrap().replay;
    let replacement =
        RecoveryWriter::start_recovered(root.clone(), recovered, 1, Some(source.clone())).unwrap();
    wait(&replacement, |status| status.ready);
    assert!(
        source.join("superseded.json").exists(),
        "ready includes previous evidence and source retirement"
    );
    assert!(
        latest_interrupted(&root).is_none(),
        "source is retired and replacement is active"
    );
    assert_eq!(
        std::fs::read(replacement.directory().join("previous-watchdog.json")).unwrap(),
        b"[\"original stall\"]"
    );
    let report = root.join("export-with-history");
    export(replacement.directory(), &report).unwrap();
    assert!(report.join("previous-manifest.json").exists());
    assert_eq!(
        std::fs::read(report.join("previous-watchdog.json")).unwrap(),
        b"[\"original stall\"]"
    );
    let next = replacement.directory().to_owned();
    drop(replacement);
    while read_lease(&next).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(latest_interrupted(&root).unwrap().directory, next);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queued_commands_stop_explicitly_when_storage_cannot_drain() {
    let root = temp();
    let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
    wait(&writer, |status| status.ready);
    let lock = std::fs::File::open(root.join("budget.lock")).unwrap();
    lock.lock().unwrap();
    for tick in 0..10_000 {
        writer.prepared(tick, &[command()]);
        writer.completed(tick + 1);
        if writer.status().error.is_some() {
            break;
        }
    }
    assert!(writer.status().error.is_some());
    assert!(writer.status().pending_bytes <= 1024 * 1024);
    lock.unlock().unwrap();
    let directory = writer.directory().to_owned();
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while read_lease(&directory).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let record = inspect(&directory).unwrap();
    assert_eq!(
        record.replay.commands.len() as u64,
        record.replay.meta.ticks.unwrap()
    );
    record.replay.validate(Some(SIM_VERSION)).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn retention_preserves_export_readers_and_explicit_reports() {
    let root = temp();
    let mut protected = None;
    let mut protected_directory = PathBuf::new();
    let report = root.join("reports/keep");
    std::fs::create_dir_all(&report).unwrap();
    std::fs::write(report.join("sentinel"), b"keep").unwrap();
    for index in 0..7 {
        let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
        wait(&writer, |status| status.ready);
        writer.finish(0);
        wait(&writer, |status| status.clean);
        let directory = writer.directory().to_owned();
        drop(writer);
        let deadline = Instant::now() + Duration::from_secs(5);
        while read_lease(&directory).is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        if index == 0 {
            let reader = std::fs::File::open(directory.join("readers")).unwrap();
            reader.lock_shared().unwrap();
            protected = Some(reader);
            protected_directory = directory;
        }
    }
    assert!(protected_directory.join("recovery.bin").exists());
    assert!(session_directories(&root).len() <= 5);
    assert_eq!(std::fs::read(report.join("sentinel")).unwrap(), b"keep");
    drop(protected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn report_verification_rejects_a_changed_replay() {
    let root = temp();
    let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
    writer.prepared(0, &[]);
    writer.completed(1);
    wait(&writer, |status| status.durable_tick == 1);
    let report = root.join("reports/export");
    std::fs::create_dir_all(report.parent().unwrap()).unwrap();
    export(writer.directory(), &report).unwrap();
    let mut replay = inspect(&report).unwrap().replay;
    replay.meta.ticks = Some(2);
    replay.save(report.join("replay.json")).unwrap();
    assert!(
        inspect(&report)
            .err()
            .unwrap()
            .to_string()
            .contains("digest")
    );
    let directory = writer.directory().to_owned();
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while read_lease(&directory).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_first_tick_failure_can_be_reported_without_offering_an_empty_resume() {
    let root = temp();
    let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
    wait(&writer, |status| status.ready);
    writer.prepared(0, &[command()]);
    let directory = writer.directory().to_owned();
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while read_lease(&directory).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(latest_interrupted(&root).is_none());
    let source = latest_diagnostic_record(&root).unwrap();
    assert_eq!(source.ticks, 0);
    let report = root.join("report");
    export(&source.directory, &report).unwrap();
    let inspected = inspect(&report).unwrap();
    assert_eq!(inspected.replay.meta.ticks, Some(0));
    assert!(inspected.replay.commands.is_empty());
    assert_eq!(inspected.prepared, Some(vec![command()]));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn recovered_source_can_be_claimed_once_even_by_cached_callers() {
    let root = temp();
    let writer = RecoveryWriter::start(root.clone(), base(), 0).unwrap();
    writer.prepared(0, &[]);
    writer.completed(1);
    wait(&writer, |status| status.durable_tick == 1);
    let source = writer.directory().to_owned();
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while inactive(&source).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let replay = inspect(&source).unwrap().replay;
    let a = RecoveryWriter::start_recovered(root.clone(), replay.clone(), 1, Some(source.clone()))
        .unwrap();
    let b = RecoveryWriter::start_recovered(root.clone(), replay.clone(), 1, Some(source.clone()))
        .unwrap();
    for writer in [&a, &b] {
        wait(writer, |status| status.ready || status.error.is_some());
    }
    assert_ne!(a.status().ready, b.status().ready);
    let (winner, loser) = if a.status().ready { (&a, &b) } else { (&b, &a) };
    assert!(!loser.directory().exists());
    let marker = std::fs::read(source.join("superseded.json")).unwrap();
    let marker_json: serde_json::Value = serde_json::from_slice(&marker).unwrap();
    assert_eq!(
        marker_json["by"],
        winner.directory().file_name().unwrap().to_str().unwrap()
    );
    let late =
        RecoveryWriter::start_recovered(root.clone(), replay, 1, Some(source.clone())).unwrap();
    wait(&late, |status| status.error.is_some());
    assert!(
        late.status()
            .error
            .unwrap()
            .contains("already been superseded")
    );
    assert!(!late.directory().exists());
    assert_eq!(
        std::fs::read(source.join("superseded.json")).unwrap(),
        marker
    );
    let winner_path = winner.directory().to_owned();
    drop((a, b, late));
    while inactive(&winner_path).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn playback_recordings_export_the_full_source_but_never_offer_live_recovery() {
    let root = temp();
    let mut replay = base();
    replay.record(0, command());
    replay.meta.ticks = Some(20);
    let writer = RecoveryWriter::start_playback(root.clone(), replay.clone(), 20).unwrap();
    wait(&writer, |status| status.ready);
    let source = writer.directory().to_owned();
    export(&source, &root.join("report")).unwrap();
    let report = inspect(&root.join("report")).unwrap();
    assert_eq!(report.kind, RecordingKind::Playback);
    assert_eq!(
        chassis::hash::state_hash(&report.replay),
        chassis::hash::state_hash(&replay)
    );
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while inactive(&source).is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(latest_interrupted(&root).is_none());
    assert_eq!(latest_diagnostic_record(&root).unwrap().directory, source);
    let invalid =
        RecoveryWriter::start_recovered(root.clone(), replay, 20, Some(source.clone())).unwrap();
    wait(&invalid, |status| status.error.is_some());
    assert!(
        invalid
            .status()
            .error
            .unwrap()
            .contains("playback diagnostics")
    );
    assert!(!invalid.directory().exists());
    assert!(!source.join("superseded.json").exists());
    drop(invalid);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn recording_kind_defaults_for_legacy_headers_and_rejects_unknown_or_mutating_playback() {
    let header = Header {
        kind: RecordingKind::LiveMatch,
        session: "legacy".into(),
        build: BuildIdentity::default(),
        base: base(),
    };
    let mut json = serde_json::to_value(&header).unwrap();
    json.as_object_mut().unwrap().remove("kind");
    let mut bytes = MAGIC.to_vec();
    write_frame(&mut bytes, &json).unwrap();
    assert_eq!(
        inspect_reader(&mut bytes.as_slice()).unwrap().kind,
        RecordingKind::LiveMatch
    );
    json["kind"] = "unknown".into();
    let mut bytes = MAGIC.to_vec();
    write_frame(&mut bytes, &json).unwrap();
    assert!(inspect_reader(&mut bytes.as_slice()).is_err());
    json["kind"] = "playback".into();
    let mut bytes = MAGIC.to_vec();
    write_frame(&mut bytes, &json).unwrap();
    write_frame(
        &mut bytes,
        &Record {
            session: "legacy".into(),
            sequence: 0,
            event: Event::Prepared {
                tick: 0,
                commands: vec![command()],
            },
        },
    )
    .unwrap();
    let inspection = inspect_reader(&mut bytes.as_slice()).unwrap();
    assert!(inspection.issue.unwrap().contains("immutable"));
    assert!(inspection.prepared.is_none());
    assert!(inspection.replay.commands.is_empty());
}
