use super::*;
use oxide_sim::{BuildingId, Command, Scenario, UnitKind};
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "oxide-player-save-{}-{}.oxsave",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).ok();
    }
}
fn meta(game: &Game) -> ReplayMeta {
    let mut meta = game.recorder.meta.clone();
    meta.kind = Some("save".into());
    meta.description = Some("before the push".into());
    meta.ticks = Some(game.state.current_tick());
    meta.saved_at = Some(42);
    meta
}

#[test]
fn save_restores_pending_input_and_exact_future_without_any_recording_history() {
    let path = Fixture::new();
    let mut original = Game::new(Scenario::skirmish()).unwrap();
    original.advance_ticks(121);
    original.issue(Command::Train {
        building: BuildingId(0),
        kind: UnitKind::Harvester,
    });
    let start = original.state.current_tick();
    let bank = original.state.player(original.presentation.human).scrap;
    write(&original, meta(&original), &path.0).unwrap();
    assert!(std::fs::read(&path.0).unwrap().starts_with(MAGIC));
    // Missing historical commands cannot affect the saved continuation.
    original.recorder.commands.clear();
    let mut restored = load(&path.0).unwrap();
    assert!(restored.presentation.paused);
    assert_eq!(restored.state.hash(), original.state.hash());
    assert_eq!(restored.recorder.start_tick(), start);
    assert!(restored.recorder.commands.is_empty());
    assert_eq!(*restored.pending, *original.pending);
    assert_eq!(
        restored.state.player(restored.presentation.human).scrap,
        bank
    );
    for _ in 0..120 {
        assert_eq!(restored.do_tick().events, original.do_tick().events);
        assert_eq!(restored.state.hash(), original.state.hash());
    }
    assert!(restored.pending.is_empty());
    assert_eq!(
        restored
            .recorder
            .commands
            .iter()
            .filter(|c| c.command.player == restored.presentation.human
                && matches!(
                    c.command.command,
                    Command::Train {
                        kind: UnitKind::Harvester,
                        ..
                    }
                ))
            .count(),
        1
    );
    assert_eq!(
        serde_json::to_value(&restored.recorder.commands).unwrap(),
        serde_json::to_value(&original.recorder.commands).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&restored).unwrap()["session"]["stats"],
        serde_json::to_value(&original).unwrap()["session"]["stats"]
    );
    assert_eq!(restored.demo, original.demo);
    assert_eq!(
        restored.presentation.boundary_fog,
        original.presentation.boundary_fog
    );
    let mut recording = restored.recorder.clone();
    recording.meta.ticks = Some(restored.state.current_tick());
    let mut playback = oxide_kit::playback::Playback::load(recording).unwrap();
    playback.seek(restored.state.current_tick());
    assert_eq!(playback.state.hash(), restored.state.hash());
    // A second save needs neither the first save nor the suffix archive.
    write(&restored, meta(&restored), &path.0).unwrap();
    let again = load(&path.0).unwrap();
    assert_eq!(again.state.hash(), restored.state.hash());
    assert_eq!(again.recorder.start_tick(), restored.state.current_tick());
    assert!(again.recorder.commands.is_empty());
}
#[test]
fn metadata_reads_stop_before_the_payload_and_corruption_is_checked_on_load() {
    struct MetadataOnly {
        inner: std::io::Cursor<Vec<u8>>,
        limit: u64,
        payload_attempted: bool,
    }
    impl Read for MetadataOnly {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            if self.inner.position().saturating_add(bytes.len() as u64) > self.limit {
                self.payload_attempted = true;
                return Err(std::io::Error::other("catalog touched checkpoint payload"));
            }
            self.inner.read(bytes)
        }
    }
    impl Seek for MetadataOnly {
        fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
            let position = self.inner.seek(from)?;
            if position > self.limit {
                self.payload_attempted = true;
                return Err(std::io::Error::other(
                    "catalog sought into checkpoint payload",
                ));
            }
            Ok(position)
        }
    }
    let path = Fixture::new();
    let game = Game::new(Scenario::skirmish()).unwrap();
    write(&game, meta(&game), &path.0).unwrap();
    let original = std::fs::read(&path.0).unwrap();
    let header_end = 12 + u32::from_le_bytes(original[8..12].try_into().unwrap()) as usize;
    let mut reader = MetadataOnly {
        inner: std::io::Cursor::new(original.clone()),
        limit: header_end as u64,
        payload_attempted: false,
    };
    let info = inspect_reader(&path.0, &mut reader, original.len() as u64).unwrap();
    assert!(!reader.payload_attempted);
    assert_eq!(reader.inner.position(), header_end as u64);
    assert_eq!(info.meta.ticks, Some(0));
    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    std::fs::write(&path.0, &corrupt).unwrap();
    assert!(
        inspect(&path.0).unwrap().problem.is_none(),
        "header eligibility is not payload validation"
    );
    assert!(
        prepare_load(&path.0).is_err(),
        "checksum protects the payload"
    );
    for bytes in [
        original[..original.len() - 1].to_vec(),
        [original.as_slice(), b"extra"].concat(),
    ] {
        std::fs::write(&path.0, bytes).unwrap();
        assert!(prepare_load(&path.0).is_err());
        assert!(inspect(&path.0).is_err());
    }
    let mut malformed = original;
    malformed[8..12].copy_from_slice(&(MAX_HEADER + 1).fit::<u32>().to_le_bytes());
    std::fs::write(&path.0, malformed).unwrap();
    assert!(prepare_load(&path.0).is_err());
}

/// The world inside a retained same-version save whose walking orders
/// lack their optional fields must still restore and play.
/// Only the simulation state is decoded: the fixture is a profiling
/// checkpoint, and controller memory is not held to its format.
#[test]
fn the_world_in_the_retained_late_skyhook_save_restores_and_plays_on() {
    fn field<'a>(value: &'a ciborium::Value, name: &str) -> &'a ciborium::Value {
        value
            .as_map()
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|(key, _)| key.as_text() == Some(name))
                    .map(|(_, value)| value)
            })
            .unwrap_or_else(|| panic!("the checkpoint carries `{name}`"))
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../driver/tests/fixtures/performance/skyhook-late.oxsave");
    let (_, payload) = read_payload(&path).expect("the retained save decodes");
    let checkpoint: ciborium::Value = ciborium::from_reader(payload.as_slice()).unwrap();
    let mut state: oxide_sim::State = field(field(&checkpoint, "session"), "state")
        .deserialized()
        .expect("the saved world restores");
    assert!(
        state.units().iter().any(|unit| matches!(
            unit.order,
            oxide_sim::Order::Run { .. }
                | oxide_sim::Order::Hunt { .. }
                | oxide_sim::Order::Attack {
                    resume: Some(_),
                    ..
                }
        )),
        "premise: the save holds walking orders in their older shape"
    );
    state
        .validate_invariants()
        .expect("the saved world is valid");
    let start = state.current_tick();
    for _ in 0..5 {
        state.tick(&[]);
        state
            .validate_invariants()
            .expect("the continued world is valid");
    }
    assert_eq!(state.current_tick(), start + 5);
}

#[test]
fn representative_checkpoints_stay_compact_and_continue_exactly() {
    let mut dense = oxide_kit::bench::mass_battle(250, 7);
    oxide_kit::bench::all_bots(&mut dense);
    let skyhook: Scenario =
        serde_json::from_str(include_str!("../../../scenarios/skyhook-anchorage.json")).unwrap();
    for scenario in [dense, skyhook] {
        let path = Fixture::new();
        let mut game = Game::new(scenario).unwrap();
        game.advance_ticks(24);
        write(&game, meta(&game), &path.0).unwrap();
        let mut file = std::fs::File::open(&path.0).unwrap();
        let bytes = file.metadata().unwrap().len();
        let header = read_header(&mut file, bytes).unwrap();
        eprintln!(
            "{}: file={bytes}, decoded={}",
            game.scenario.name, header.decoded_bytes
        );
        assert!(
            bytes <= 1024 * 1024,
            "{}: compressed checkpoint grew to {bytes} bytes",
            game.scenario.name
        );
        assert!(
            header.decoded_bytes <= 8 * 1024 * 1024,
            "{}: checkpoint grew to {} decoded bytes",
            game.scenario.name,
            header.decoded_bytes
        );
        let mut restored = prepare_load(&path.0).unwrap().install();
        assert_eq!(game.hash_hex(), restored.hash_hex());
        let start = game.state.current_tick();
        for _ in 0..24 {
            assert_eq!(game.do_tick().events, restored.do_tick().events);
            assert_eq!(game.hash_hex(), restored.hash_hex());
        }
        let suffix: Vec<_> = game
            .recorder
            .commands
            .iter()
            .filter(|command| command.tick >= start)
            .collect();
        assert_eq!(
            serde_json::to_value(suffix).unwrap(),
            serde_json::to_value(&restored.recorder.commands).unwrap()
        );
    }
}

#[test]
fn header_session_mismatch_and_decoding_expansion_are_rejected() {
    let path = Fixture::new();
    let game = Game::new(Scenario::skirmish()).unwrap();
    write(&game, meta(&game), &path.0).unwrap();
    let original = std::fs::read(&path.0).unwrap();
    let end = 12 + u32::from_le_bytes(original[8..12].try_into().unwrap()) as usize;
    let header: serde_json::Value = serde_json::from_slice(&original[12..end]).unwrap();
    for (field, value) in [
        ("version", serde_json::json!(999)),
        ("map", serde_json::json!("different")),
        ("seats", serde_json::json!(99)),
        ("decoded_bytes", serde_json::json!(1)),
        ("decoded_bytes", serde_json::json!(MAX_BYTES + 1)),
    ] {
        let mut changed = header.clone();
        changed[field] = value;
        let bytes = serde_json::to_vec(&changed).unwrap();
        let mut file = MAGIC.to_vec();
        file.extend(bytes.len().fit::<u32>().to_le_bytes());
        file.extend(bytes);
        file.extend(&original[end..]);
        std::fs::write(&path.0, file).unwrap();
        assert!(prepare_load(&path.0).is_err(), "{field}");
    }
}
