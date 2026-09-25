//! Compact player checkpoints with independently readable metadata.

#[cfg(test)]
use crate::game::Game;
use crate::game::checkpoint::{GameCheckpoint, RestoredGame, SaveCapture};
use anyhow::{Context, Result, ensure};
use chassis::replay::ReplayMeta;
use oxide_sim::SIM_VERSION;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

const VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"OXIDESAV";
const MAX_HEADER: usize = 64 * 1024;
const MAX_BYTES: usize = oxide_kit::checkpoint::MAX_BYTES;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    meta: ReplayMeta,
    map: String,
    seats: usize,
    compressed_bytes: u64,
    decoded_bytes: u64,
}

pub struct RecordInfo {
    pub meta: ReplayMeta,
    pub map: String,
    pub seats: usize,
    pub problem: Option<String>,
    pub legacy: bool,
}

#[cfg(test)]
fn write(game: &Game, meta: ReplayMeta, path: &Path) -> Result<()> {
    write_capture(game.capture_save(), meta, path)
}

pub(crate) fn write_capture(capture: SaveCapture, meta: ReplayMeta, path: &Path) -> Result<()> {
    let checkpoint = capture.checkpoint()?;
    let mut payload = Vec::new();
    ciborium::into_writer(&checkpoint, &mut payload)?;
    ensure!(
        payload.len() <= MAX_BYTES,
        "save exceeds decoded byte limit"
    );
    // Metadata is obtained from the checkpoint itself, without restoring controllers.
    let (map, seats, tick) = checkpoint.metadata();
    ensure!(meta.ticks == Some(tick), "save metadata mismatch");
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 3)?;
    encoder.include_checksum(true)?;
    encoder.write_all(&payload)?;
    let compressed = encoder.finish()?;
    let header = Header {
        version: VERSION,
        meta,
        map: map.to_owned(),
        seats,
        compressed_bytes: compressed.len() as u64,
        decoded_bytes: payload.len() as u64,
    };
    check_header(&header)?;
    let header = serde_json::to_vec(&header)?;
    ensure!(
        header.len() <= MAX_HEADER && 12 + header.len() + compressed.len() <= MAX_BYTES,
        "save exceeds byte limit"
    );
    chassis::fsx::write_atomic(path, |writer| {
        writer.write_all(MAGIC)?;
        writer.write_all(&(header.len() as u32).to_le_bytes())?;
        writer.write_all(&header)?;
        writer.write_all(&compressed)?;
        Ok(())
    })
}

fn read_header(reader: &mut impl Read, file_bytes: u64) -> Result<Header> {
    ensure!(file_bytes <= MAX_BYTES as u64, "save exceeds byte limit");
    let mut magic = [0; 8];
    reader.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "unsupported player save format");
    let mut length = [0; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(length <= MAX_HEADER, "save header exceeds byte limit");
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    let header: Header = serde_json::from_slice(&bytes)?;
    ensure!(
        header.compressed_bytes <= MAX_BYTES as u64 && header.decoded_bytes <= MAX_BYTES as u64,
        "save payload exceeds byte limit"
    );
    ensure!(
        file_bytes == 12 + length as u64 + header.compressed_bytes,
        "save payload length mismatch"
    );
    Ok(header)
}

fn check_header(header: &Header) -> Result<()> {
    ensure!(
        header.version == VERSION,
        "unsupported save format {} (this build supports {VERSION})",
        header.version
    );
    ensure!(
        header.meta.sim_version == SIM_VERSION,
        "saved on sim v{}; this build runs v{SIM_VERSION}",
        header.meta.sim_version
    );
    ensure!(
        matches!(header.meta.kind.as_deref(), Some("save" | "autosave")),
        "invalid save kind"
    );
    Ok(())
}

pub(crate) fn prepare_load(path: &Path) -> Result<RestoredGame> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("loading save {}", path.display()))?;
    let len = file.metadata()?.len();
    let header = read_header(&mut file, len).context("loading save header")?;
    check_header(&header)?;
    let mut compressed = Vec::new();
    file.take(header.compressed_bytes + 1)
        .read_to_end(&mut compressed)?;
    ensure!(
        compressed.len() as u64 == header.compressed_bytes,
        "save payload length mismatch"
    );
    ensure!(
        compressed
            .get(4)
            .is_some_and(|descriptor| descriptor & 4 != 0),
        "save frame has no checksum"
    );
    let frame_len = zstd::zstd_safe::find_frame_compressed_size(&compressed)
        .map_err(|code| anyhow::anyhow!("invalid compressed frame: {code}"))?;
    ensure!(frame_len == compressed.len(), "trailing compressed payload");
    let mut payload = Vec::new();
    zstd::stream::read::Decoder::new(compressed.as_slice())?
        .take(header.decoded_bytes + 1)
        .read_to_end(&mut payload)?;
    ensure!(
        payload.len() as u64 == header.decoded_bytes,
        "decoded payload length mismatch"
    );
    let mut reader = payload.as_slice();
    let checkpoint: GameCheckpoint = ciborium::from_reader(&mut reader)?;
    ensure!(reader.is_empty(), "trailing checkpoint payload");
    let game = checkpoint.restore()?;
    ensure!(
        header.meta.ticks == Some(game.tick())
            && header.map == game.scenario().name
            && header.seats == game.scenario().players.len(),
        "save metadata does not match session"
    );
    Ok(game)
}

#[cfg(test)]
pub fn load(path: &Path, _diagnostics: Option<&oxide_kit::diagnostics::Recorder>) -> Result<Game> {
    prepare_load(path).map(RestoredGame::install)
}

pub fn inspect(path: &Path) -> Result<RecordInfo> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    inspect_reader(path, &mut file, len)
}

fn inspect_reader(path: &Path, file: &mut (impl Read + Seek), len: u64) -> Result<RecordInfo> {
    let mut magic = [0; 8];
    let compact = file.read_exact(&mut magic).is_ok() && &magic == MAGIC;
    if compact {
        file.seek(SeekFrom::Start(0))?;
        let header = read_header(file, len)?;
        let problem = check_header(&header)
            .err()
            .map(|error| format!("{error:#}"));
        Ok(RecordInfo {
            meta: header.meta,
            map: header.map,
            seats: header.seats,
            problem,
            legacy: false,
        })
    } else if path.extension().and_then(|extension| extension.to_str()) == Some("oxsave") {
        anyhow::bail!("invalid player save header");
    } else if &magic == b"{\"save\":"
        || path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.starts_with("save-") || stem.starts_with("autosave-"))
    {
        Ok(RecordInfo {
            meta: ReplayMeta {
                sim_version: String::new(),
                description: None,
                ticks: None,
                kind: None,
                saved_at: None,
            },
            map: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            seats: 0,
            problem: Some("unsupported player save format".into()),
            legacy: true,
        })
    } else {
        // Replay inspection remains separate from player checkpoint restoration.
        let replay = oxide_kit::load_replay(path)?;
        let kind = crate::saves::record_kind(&replay.meta, path);
        let problem = if kind.resumable() {
            Some("unsupported player save format".into())
        } else {
            replay
                .validate(Some(SIM_VERSION))
                .map_err(anyhow::Error::from)
                .and_then(|()| oxide_kit::bounded_replay_duration(&replay).map(|_| ()))
                .err()
                .map(|error| format!("{error:#}"))
        };
        let ticks = oxide_kit::replay_duration(&replay);
        let mut meta = replay.meta;
        meta.ticks = Some(ticks);
        Ok(RecordInfo {
            meta,
            map: replay.setup.name,
            seats: replay.setup.players.len(),
            problem,
            legacy: true,
        })
    }
}

#[cfg(test)]
mod tests {
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
        let mut restored = load(&path.0, None).unwrap();
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
        let again = load(&path.0, None).unwrap();
        assert_eq!(again.state.hash(), restored.state.hash());
        assert_eq!(again.recorder.start_tick(), restored.state.current_tick());
        assert!(again.recorder.commands.is_empty());
    }
    #[test]
    fn metadata_reads_stop_before_the_payload_and_corruption_is_checked_on_load() {
        let path = Fixture::new();
        let game = Game::new(Scenario::skirmish()).unwrap();
        write(&game, meta(&game), &path.0).unwrap();
        let original = std::fs::read(&path.0).unwrap();
        let header_end = 12 + u32::from_le_bytes(original[8..12].try_into().unwrap()) as usize;
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
        malformed[8..12].copy_from_slice(&((MAX_HEADER + 1) as u32).to_le_bytes());
        std::fs::write(&path.0, malformed).unwrap();
        assert!(prepare_load(&path.0).is_err());
    }

    #[test]
    fn representative_checkpoints_stay_compact_and_continue_exactly() {
        let mut dense = oxide_kit::bench::mass_battle(250, 7);
        oxide_kit::bench::all_bots(&mut dense);
        let skyhook: Scenario =
            serde_json::from_str(include_str!("../../scenarios/skyhook-anchorage.json")).unwrap();
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
            file.extend((bytes.len() as u32).to_le_bytes());
            file.extend(bytes);
            file.extend(&original[end..]);
            std::fs::write(&path.0, file).unwrap();
            assert!(prepare_load(&path.0).is_err(), "{field}");
        }
    }
}
