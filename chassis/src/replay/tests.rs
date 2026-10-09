use super::*;

#[test]
fn cursor_yields_commands_grouped_by_tick() {
    let mut replay: Replay<(), &str> = Replay::new(0, ());
    replay.record(3, "a");
    replay.record(3, "b");
    replay.record(10, "c");

    let mut cursor = replay.cursor();
    assert!(cursor.take_tick(0).is_empty());
    let at3: Vec<_> = cursor.take_tick(3).iter().map(|t| t.command).collect();
    assert_eq!(at3, vec!["a", "b"]);
    assert!(cursor.take_tick(4).is_empty());
    assert_eq!(cursor.take_tick(10).len(), 1);
    assert!(cursor.is_finished());
}

#[test]
fn cursor_skips_commands_from_ticks_the_caller_skipped() {
    let mut replay: Replay<(), &str> = Replay::new(0, ());
    replay.record(1, "missed-a");
    replay.record(2, "missed-b");
    replay.record(5, "current");

    let mut cursor = replay.cursor();
    let at_five: Vec<_> = cursor
        .take_tick(5)
        .iter()
        .map(|timed| timed.command)
        .collect();
    assert_eq!(at_five, vec!["current"]);
    assert!(cursor.is_finished());
}

#[test]
#[should_panic(expected = "tick order")]
fn recording_out_of_order_panics() {
    let mut replay: Replay<(), u8> = Replay::new(0, ());
    replay.record(5, 1);
    replay.record(4, 2);
}

#[test]
fn validate_accepts_what_record_produces() {
    let mut replay: Replay<(), u8> = Replay::new(1, ());
    replay.record(3, 1);
    replay.record(3, 2);
    replay.record(9, 3);
    replay.meta.ticks = Some(10);
    assert!(replay.validate(Some(1)).is_ok());
    assert!(replay.validate(None).is_ok());
}

#[test]
fn validate_rejects_a_command_at_the_tick_ceiling() {
    // Playback computes "last tick + 1"; u64::MAX must die here, not
    // overflow there.
    let mut replay: Replay<(), u8> = Replay::new(1, ());
    replay.commands = vec![TimedCommand {
        tick: u64::MAX,
        command: 1,
    }];
    assert!(matches!(
        replay.validate(None),
        Err(ReplayError::Invalid(_))
    ));
}

#[test]
fn validate_rejects_tampered_files() {
    // Hand-built (bypassing record) the way a corrupt file would be.
    let mut replay: Replay<(), u8> = Replay::new(1, ());
    replay.commands = vec![
        TimedCommand {
            tick: 9,
            command: 1,
        },
        TimedCommand {
            tick: 3,
            command: 2,
        },
    ];
    assert!(matches!(
        replay.validate(None),
        Err(ReplayError::Invalid(_))
    ));

    // Duration that doesn't cover its own commands.
    let mut replay: Replay<(), u8> = Replay::new(1, ());
    replay.record(9, 1);
    replay.meta.ticks = Some(0);
    assert!(matches!(
        replay.validate(None),
        Err(ReplayError::Invalid(_))
    ));

    // Wrong sim version.
    let replay: Replay<(), u8> = Replay::new(2, ());
    assert!(matches!(
        replay.validate(Some(1)),
        Err(ReplayError::VersionMismatch { .. })
    ));
}

#[test]
fn save_creates_parent_directories() {
    let dir = std::env::temp_dir()
        .join("chassis-replay-test")
        .join("deep")
        .join("nested");
    std::fs::remove_dir_all(&dir).ok();
    let path = dir.join("out.json");
    let replay: Replay<u8, u8> = Replay::new(1, 1);
    replay.save(&path).unwrap();
    assert!(path.exists());
}

#[test]
fn saving_twice_to_one_path_replaces_the_record() {
    // Pinned on every CI platform: a second save onto an existing
    // replay lands (std's rename replaces on Windows too) and its
    // content wins.
    let dir = std::env::temp_dir().join(format!("chassis-replay-twice-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.json");

    let mut first: Replay<u8, &str> = Replay::new(1, 1);
    first.record(1, "early");
    first.save(&path).unwrap();
    let mut second: Replay<u8, &str> = Replay::new(1, 1);
    second.record(1, "early");
    second.record(7, "late");
    second.save(&path).unwrap();

    let loaded: Replay<u8, String> = Replay::load(&path).unwrap();
    assert_eq!(loaded.commands.len(), 2, "the second record won");
    assert_eq!(loaded.commands[1].command, "late");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn absent_metadata_stays_out_of_the_file_and_present_metadata_survives() {
    // Unset optional metadata is omitted from the file, and a file
    // without those fields loads with them absent.
    let bare: Replay<u8, u8> = Replay::new(1, 1);
    let json = serde_json::to_string(&bare).unwrap();
    assert!(!json.contains("kind") && !json.contains("saved_at"));
    let bare_file = r#"{"meta":{"sim_version":1},"setup":1,"commands":[]}"#;
    let loaded: Replay<u8, u8> = serde_json::from_str(bare_file).unwrap();
    assert_eq!(loaded.meta.kind, None);
    assert_eq!(loaded.meta.saved_at, None);

    let mut tagged: Replay<u8, u8> = Replay::new(1, 1);
    tagged.meta.kind = Some("save".to_string());
    tagged.meta.saved_at = Some(1_784_721_600);
    let json = serde_json::to_string(&tagged).unwrap();
    let back: Replay<u8, u8> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.meta.kind.as_deref(), Some("save"));
    assert_eq!(back.meta.saved_at, Some(1_784_721_600));
}

#[test]
fn save_load_roundtrip() {
    let dir = std::env::temp_dir().join("chassis-replay-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("roundtrip.json");

    let mut replay: Replay<u32, String> = Replay::new(3, 77);
    replay.record(1, "move".to_string());
    replay.save(&path).unwrap();

    let loaded: Replay<u32, String> = Replay::load(&path).unwrap();
    assert_eq!(loaded.meta.sim_version, 3);
    assert_eq!(loaded.setup, 77);
    assert_eq!(loaded.commands.len(), 1);
    assert_eq!(loaded.commands[0].command, "move");
}

#[test]
fn load_rejects_oversized_input_before_json_parsing() {
    let dir = std::env::temp_dir().join(format!("chassis-replay-bounds-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oversized.json");
    std::fs::write(&path, vec![b' '; 33]).unwrap();

    let error = Replay::<(), ()>::load_with_limits(&path, 32, 10).unwrap_err();
    assert!(error.to_string().contains("32-byte limit"), "{error}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn load_rejects_too_many_commands_at_the_shared_boundary() {
    let dir = std::env::temp_dir().join(format!(
        "chassis-replay-command-bounds-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("commands.json");
    let mut replay: Replay<(), ()> = Replay::new(1, ());
    for tick in 0..3 {
        replay.record(tick, ());
    }
    replay.save(&path).unwrap();

    let error = Replay::<(), ()>::load_with_limits(&path, MAX_REPLAY_BYTES, 2).unwrap_err();
    assert!(error.to_string().contains("2-command limit"), "{error}");
    std::fs::remove_dir_all(&dir).ok();
}
