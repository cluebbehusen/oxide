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
    drop(writer);
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
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !directory.join("status.json").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(inspect(&directory).unwrap().replay.meta.ticks, Some(0));
    std::fs::remove_dir_all(root).unwrap();
}
