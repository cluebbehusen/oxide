//! Player saves are bounded session checkpoints; recordings remain command logs.

use crate::game::Game;
use anyhow::{Context, Result, ensure};
use chassis::replay::ReplayMeta;
use oxide_sim::SIM_VERSION;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

const VERSION: u32 = 1;
const MAX_BYTES: usize = oxide_kit::checkpoint::MAX_BYTES;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    meta: ReplayMeta,
    map: String,
    seats: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document<G> {
    save: Header,
    game: G,
}

#[derive(Deserialize)]
struct Discriminator {
    save: Option<Header>,
}

pub struct RecordInfo {
    pub meta: ReplayMeta,
    pub map: String,
    pub seats: usize,
    pub problem: Option<String>,
    pub legacy: bool,
}

pub fn write(game: &Game, meta: ReplayMeta, path: &Path) -> Result<()> {
    ensure!(
        matches!(meta.kind.as_deref(), Some("save" | "autosave")),
        "invalid save kind"
    );
    ensure!(
        meta.sim_version == SIM_VERSION && meta.ticks == Some(game.state.current_tick()),
        "save metadata mismatch"
    );
    let document = Document {
        save: Header {
            version: VERSION,
            meta,
            map: game.scenario.name.clone(),
            seats: game.scenario.players.len(),
        },
        game,
    };
    let bytes = serde_json::to_vec(&document)?;
    ensure!(bytes.len() <= MAX_BYTES, "save exceeds byte limit");
    chassis::fsx::write_atomic(path, |writer| {
        writer.write_all(&bytes)?;
        Ok(())
    })
}

fn read(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() <= MAX_BYTES as u64,
        "save exceeds byte limit"
    );
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_BYTES, "save exceeds byte limit");
    Ok(bytes)
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

fn decode(bytes: &[u8]) -> Result<Game> {
    let document: Document<Game> = serde_json::from_slice(bytes)?;
    check_header(&document.save)?;
    ensure!(
        document.save.meta.ticks == Some(document.game.state.current_tick())
            && document.save.map == document.game.scenario.name
            && document.save.seats == document.game.scenario.players.len(),
        "save metadata does not match session"
    );
    Ok(document.game)
}

pub fn load(path: &Path, diagnostics: Option<&oxide_kit::diagnostics::Recorder>) -> Result<Game> {
    let bytes = read(path).with_context(|| format!("loading save {}", path.display()))?;
    let header: Discriminator = serde_json::from_slice(&bytes)?;
    if let Some(header) = header.save {
        check_header(&header)?;
        decode(&bytes)
    } else {
        let replay = oxide_kit::load_replay(path)?;
        let mut game = Game::from_replay_observed(replay, diagnostics)?;
        game.recorder = oxide_kit::GameReplay::with_origin(
            SIM_VERSION,
            game.scenario.clone(),
            oxide_kit::recording::WorldOrigin::capture(&game.scenario, &game.state)?,
        )?;
        game.presentation.paused = true;
        Ok(game)
    }
}

pub fn inspect(path: &Path) -> Result<RecordInfo> {
    let bytes = read(path)?;
    let header: Discriminator = serde_json::from_slice(&bytes)?;
    if let Some(header) = header.save {
        let problem = check_header(&header)
            .and_then(|()| decode(&bytes).map(drop))
            .err()
            .map(|error| format!("{error:#}"));
        Ok(RecordInfo {
            meta: header.meta,
            map: header.map,
            seats: header.seats,
            problem,
            legacy: false,
        })
    } else {
        let replay = oxide_kit::load_replay(path)?;
        let kind = crate::saves::record_kind(&replay.meta, path);
        let problem = (|| -> Result<()> {
            replay.validate(Some(SIM_VERSION))?;
            oxide_kit::bounded_replay_duration(&replay)?;
            if kind.resumable() {
                ensure!(
                    replay.origin.is_none(),
                    "recording has no controller checkpoint for live continuation"
                );
            }
            Ok(())
        })()
        .err()
        .map(|error| format!("{error:#}"));
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
    use serde_json::json;
    use std::path::PathBuf;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "oxide-player-save-{}-{}.json",
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
    fn legacy_saves_with_no_human_or_multiple_passive_seats_remain_resumable() {
        for bot in [false, true] {
            let path = Fixture::new();
            let mut scenario = Scenario::skirmish();
            for player in &mut scenario.players {
                player.bot = bot;
                player.bot_config = None;
            }
            let mut replay = oxide_kit::GameReplay::new(SIM_VERSION, scenario);
            replay.meta.kind = Some("save".into());
            replay.meta.ticks = Some(0);
            std::fs::write(&path.0, serde_json::to_vec(&replay).unwrap()).unwrap();
            assert!(inspect(&path.0).unwrap().problem.is_none());
            let game = load(&path.0, None).unwrap();
            assert_eq!(game.presentation.human, oxide_sim::PlayerId(0));
        }
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
        let bytes = std::fs::read(&path.0).unwrap();
        let document: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(document["game"].get("recorded").is_none());
        assert!(document["game"].get("recorder").is_none());
        assert_eq!(
            document["game"]["session"]["pending"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
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
    fn legacy_import_reconstructs_once_and_preserves_the_source_file() {
        let legacy = Fixture::new();
        let converted = Fixture::new();
        let mut original = Game::new(Scenario::skirmish()).unwrap();
        original.advance_ticks(73);
        original.recorder.meta = meta(&original);
        original.recorder.save(&legacy.0).unwrap();
        let bytes = std::fs::read(&legacy.0).unwrap();
        assert!(inspect(&legacy.0).unwrap().legacy);
        let mut restored = load(&legacy.0, None).unwrap();
        assert_eq!(restored.state.hash(), original.state.hash());
        assert_eq!(restored.recorder.start_tick(), 73);
        assert!(restored.recorder.commands.is_empty());
        write(&restored, meta(&restored), &converted.0).unwrap();
        assert!(!inspect(&converted.0).unwrap().legacy);
        let mut checkpoint = load(&converted.0, None).unwrap();
        for _ in 0..48 {
            let expected = original.do_tick().events;
            assert_eq!(expected, restored.do_tick().events);
            assert_eq!(expected, checkpoint.do_tick().events);
            assert_eq!(original.state.hash(), checkpoint.state.hash());
        }
        assert_eq!(std::fs::read(&legacy.0).unwrap(), bytes);
    }

    #[test]
    fn compatibility_and_corruption_are_reported_before_installation() {
        let path = Fixture::new();
        let game = Game::new(Scenario::skirmish()).unwrap();
        write(&game, meta(&game), &path.0).unwrap();
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path.0).unwrap()).unwrap();
        for (pointer, value, message) in [
            ("/save/version", json!(999), "save format"),
            ("/save/meta/sim_version", json!("foreign"), "foreign"),
            ("/game/version", json!(999), "shell checkpoint"),
            ("/game/session/version", json!(999), "session checkpoint"),
            (
                "/game/session/bots/0/version",
                json!(999),
                "controller checkpoint",
            ),
            ("/game/human", json!(1), "local seat"),
            ("/save/meta/ticks", json!(42), "metadata"),
            ("/save/map", json!("different map"), "metadata"),
            ("/save/seats", json!(9), "metadata"),
            ("/save/meta/kind", json!("match"), "save kind"),
        ] {
            let mut bad = document.clone();
            *bad.pointer_mut(pointer).expect(pointer) = value;
            std::fs::write(&path.0, serde_json::to_vec(&bad).unwrap()).unwrap();
            let error = load(&path.0, None)
                .err()
                .expect("invalid save cannot install");
            assert!(
                format!("{error:#}").contains(message),
                "{pointer}: {error:#}"
            );
            let info = inspect(&path.0).unwrap();
            assert!(info.problem.unwrap().contains(message));
        }
        std::fs::write(&path.0, b"{\"save\":").unwrap();
        assert!(load(&path.0, None).is_err());
        std::fs::File::create(&path.0)
            .unwrap()
            .set_len(MAX_BYTES as u64 + 1)
            .unwrap();
        assert!(
            load(&path.0, None)
                .err()
                .unwrap()
                .to_string()
                .contains("loading save")
        );
        assert!(
            inspect(&path.0)
                .err()
                .unwrap()
                .to_string()
                .contains("byte limit")
        );
    }

    #[test]
    fn world_only_records_and_unbounded_legacy_saves_cannot_resume() {
        let path = Fixture::new();
        let game = Game::new(Scenario::skirmish()).unwrap();
        let mut replay = oxide_kit::GameReplay::with_origin(
            SIM_VERSION,
            game.scenario.clone(),
            oxide_kit::recording::WorldOrigin::capture(&game.scenario, &game.state).unwrap(),
        )
        .unwrap();
        replay.meta.kind = Some("save".into());
        replay.save(&path.0).unwrap();
        assert!(
            inspect(&path.0)
                .unwrap()
                .problem
                .unwrap()
                .contains("controller checkpoint")
        );
        assert!(load(&path.0, None).is_err());
        replay.origin = None;
        replay.meta.ticks = Some(oxide_kit::MAX_REPLAY_TICKS + 1);
        replay.save(&path.0).unwrap();
        assert!(inspect(&path.0).unwrap().problem.is_some());
        assert!(load(&path.0, None).is_err());
    }
}
