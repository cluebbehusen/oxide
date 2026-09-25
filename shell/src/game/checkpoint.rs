//! Self-contained shell continuation; historical commands belong to recordings.

use super::*;
use oxide_kit::checkpoint::SessionCheckpoint;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GameCheckpoint {
    version: u32,
    session: SessionCheckpoint,
    human: PlayerId,
    demo: crate::tutorial::Demo,
    concede_stats: Option<oxide_kit::stats::MatchStats>,
    boundary_fog: crate::boundary_fog::BoundaryFog,
}

pub(crate) struct SaveCapture {
    scenario: Scenario,
    state: Arc<State>,
    bots: Vec<SeatBot>,
    pending: Vec<oxide_sim::PlayerCommand>,
    stats: oxide_kit::stats::LiveMatchStats,
    human: PlayerId,
    demo: crate::tutorial::Demo,
    concede_stats: Option<oxide_kit::stats::MatchStats>,
    boundary_fog: crate::boundary_fog::BoundaryFog,
}

impl Game {
    pub(crate) fn capture_save(&self) -> SaveCapture {
        SaveCapture {
            scenario: self.scenario.clone(),
            state: Arc::clone(&self.state.0),
            bots: self.bots.clone(),
            pending: self.pending.to_vec(),
            stats: self.live_stats.clone(),
            human: self.presentation.human,
            demo: self.demo,
            concede_stats: self.concede_stats.clone(),
            boundary_fog: self.presentation.boundary_fog.clone(),
        }
    }
}

impl SaveCapture {
    pub(crate) fn checkpoint(self) -> Result<GameCheckpoint> {
        Ok(GameCheckpoint {
            version: 2,
            session: SessionCheckpoint::capture(
                &self.scenario,
                &self.state,
                &self.bots,
                &self.pending,
                Some(&self.stats),
            )?,
            human: self.human,
            demo: self.demo,
            concede_stats: self.concede_stats,
            boundary_fog: self.boundary_fog,
        })
    }
}

impl Serialize for Game {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.capture_save()
            .checkpoint()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Game {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let checkpoint = GameCheckpoint::deserialize(deserializer)?;
        checkpoint
            .restore()
            .map(RestoredGame::install)
            .map_err(serde::de::Error::custom)
    }
}

pub(crate) struct RestoredGame {
    core: oxide_kit::checkpoint::RestoredSession,
    recorder: GameReplay,
    human: PlayerId,
    demo: crate::tutorial::Demo,
    concede_stats: Option<oxide_kit::stats::MatchStats>,
    boundary_fog: crate::boundary_fog::BoundaryFog,
    recovery: Option<Arc<oxide_kit::recovery::RecoveryWriter>>,
    recovery_origin: Option<SessionCheckpoint>,
}

impl GameCheckpoint {
    pub(crate) fn metadata(&self) -> (&str, usize, u64) {
        self.session.metadata()
    }

    pub(crate) fn restore(self) -> Result<RestoredGame> {
        let checkpoint = self;
        anyhow::ensure!(
            checkpoint.version == 2,
            "unsupported shell checkpoint version"
        );
        let recorder = checkpoint.session.recording()?;
        let recovery_origin = Some(checkpoint.session.clone());
        let core = checkpoint.session.restore()?;
        anyhow::ensure!(
            checkpoint.human == Game::local_seat(&core.scenario),
            "invalid local seat"
        );
        anyhow::ensure!(
            checkpoint.boundary_fog.valid_checkpoint(&core.state),
            "invalid boundary fog"
        );
        if let Some(stats) = &checkpoint.concede_stats {
            stats.validate_checkpoint(&core.state)?;
        }
        anyhow::ensure!(
            core.stats.is_some(),
            "shell checkpoint requires live statistics"
        );
        Ok(RestoredGame {
            core,
            recorder,
            human: checkpoint.human,
            demo: checkpoint.demo,
            concede_stats: checkpoint.concede_stats,
            boundary_fog: checkpoint.boundary_fog,
            recovery: None,
            recovery_origin,
        })
    }
}

impl RestoredGame {
    pub(crate) fn recover(
        record: oxide_kit::recovery::Inspection,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self> {
        anyhow::ensure!(
            !record.clean && record.kind == oxide_kit::recovery::RecordingKind::LiveMatch,
            "recording is not an interrupted live match"
        );
        let recovery_origin = record.checkpoint.clone();
        let core = if let Some(checkpoint) = record.checkpoint {
            checkpoint.resume_recording_cancellable(&record.replay, &cancelled)?
        } else {
            let scenario = record.replay.setup.clone();
            record.replay.validate(Some(SIM_VERSION))?;
            anyhow::ensure!(
                record.replay.origin.is_none(),
                "missing recovery checkpoint"
            );
            let mut state = scenario.build()?;
            let mut bots = seat_bots(&scenario)?;
            let mut stats = oxide_kit::stats::LiveMatchStats::new(&state);
            let mut cursor = record.replay.cursor();
            let end = oxide_kit::bounded_replay_duration(&record.replay)?;
            for _ in 0..end {
                anyhow::ensure!(!cancelled(), "load cancelled");
                let _ = oxide_kit::bot_execution::commands(&state, &mut bots);
                let commands: Vec<_> = cursor
                    .take_tick(state.current_tick())
                    .iter()
                    .map(|timed| timed.command.clone())
                    .collect();
                let report = state.tick(&commands);
                stats.observe(&state, &report.events);
            }
            anyhow::ensure!(cursor.is_finished(), "unconsumed recovery commands");
            oxide_kit::checkpoint::RestoredSession {
                scenario,
                state,
                bots,
                pending: Vec::new(),
                stats: Some(stats),
            }
        };
        anyhow::ensure!(
            core.stats.is_some(),
            "shell recovery requires live statistics"
        );
        let human = Game::local_seat(&core.scenario);
        let boundary_fog = crate::boundary_fog::BoundaryFog::new(&core.state, human);
        Ok(Self {
            core,
            recorder: record.replay,
            human,
            demo: Default::default(),
            concede_stats: None,
            boundary_fog,
            recovery: None,
            recovery_origin,
        })
    }

    pub(crate) fn start_recovery(
        &mut self,
        root: Option<std::path::PathBuf>,
        source: Option<std::path::PathBuf>,
    ) -> Result<()> {
        if let Some(root) = root {
            let writer = if let Some(checkpoint) = self.recovery_origin.take() {
                oxide_kit::recovery::RecoveryWriter::start_recovered_checkpoint(
                    root,
                    self.recorder.clone(),
                    self.tick(),
                    checkpoint,
                    source,
                    crate::build_identity(),
                )?
            } else {
                oxide_kit::recovery::RecoveryWriter::start_recovered(
                    root,
                    self.recorder.clone(),
                    self.tick(),
                    source,
                    crate::build_identity(),
                )?
            };
            self.recovery = Some(Arc::new(writer));
        }
        Ok(())
    }

    pub(crate) fn scenario(&self) -> &Scenario {
        &self.core.scenario
    }
    pub(crate) fn tick(&self) -> u64 {
        self.core.state.current_tick()
    }
    pub(crate) fn install(self) -> Game {
        let Self {
            core,
            recorder,
            human,
            demo,
            concede_stats,
            boundary_fog,
            recovery,
            recovery_origin: _,
        } = self;
        let live_stats = core.stats.expect("restoration validated statistics");
        let mut presentation = Presentation::new(&core.state, human, crate::render::viewport());
        presentation.reset_after_jump(&core.state);
        presentation.boundary_fog = boundary_fog;
        presentation.paused = true;
        presentation.conceded_banner = concede_stats.is_some();
        let end_stats = core
            .state
            .result()
            .map(|_| live_stats.snapshot(&core.state));
        Game {
            scenario: core.scenario,
            state: ReadOnlyState(Arc::new(core.state)),
            bots: core.bots,
            bot_decision: None,
            recorder,
            pending: PendingCommands(core.pending),
            live_stats,
            end_stats,
            concede_stats,
            demo,
            presentation,
            autosave_done: false,
            suppress_presentation: false,
            recovery_root: None,
            recovery,
            recovery_warned: false,
            diagnostics_warned: false,
            recovery_source: None,
            diagnostics: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_recovery_restores_the_shell_without_replaying_the_opening() {
        let mut original = Game::with_viewport(Scenario::skirmish(), vec2(1280.0, 720.0)).unwrap();
        original.advance_ticks(37);
        original.issue(Command::Train {
            building: oxide_sim::BuildingId(0),
            kind: oxide_sim::UnitKind::Harvester,
        });
        let checkpoint = SessionCheckpoint::capture(
            &original.scenario,
            &original.state,
            &original.bots,
            &original.pending,
            Some(&original.live_stats),
        )
        .unwrap();
        original.recorder = checkpoint.recording().unwrap();
        original.do_tick();
        let mut replay = original.recorder.clone();
        replay.meta.ticks = Some(original.state.current_tick());
        let record = oxide_kit::recovery::Inspection {
            kind: oxide_kit::recovery::RecordingKind::LiveMatch,
            build: Default::default(),
            session: "test".into(),
            replay,
            checkpoint: Some(checkpoint),
            prepared: None,
            issue: None,
            clean: false,
        };
        let root =
            std::env::temp_dir().join(format!("oxide-restored-prefix-{}", std::process::id()));
        let mut prepared = RestoredGame::recover(record, || false).unwrap();
        prepared.start_recovery(Some(root.clone()), None).unwrap();
        let writer = prepared.recovery.as_ref().unwrap().clone();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !writer.status().ready {
            assert!(writer.status().error.is_none(), "{:?}", writer.status());
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let persisted = oxide_kit::recovery::inspect(writer.directory()).unwrap();
        assert_eq!(
            persisted.replay.start_tick(),
            original.recorder.start_tick()
        );
        assert_eq!(
            serde_json::to_value(persisted.replay.commands).unwrap(),
            serde_json::to_value(&original.recorder.commands).unwrap()
        );
        let mut restored = prepared.install();
        assert!(restored.presentation.paused);
        assert!(restored.pending.is_empty());
        assert_eq!(restored.state.hash(), original.state.hash());
        for _ in 0..24 {
            assert_eq!(original.do_tick().events, restored.do_tick().events);
            assert_eq!(original.state.hash(), restored.state.hash());
        }
        assert_eq!(
            original.live_stats.snapshot(&original.state),
            restored.live_stats.snapshot(&restored.state)
        );
        assert_eq!(
            serde_json::to_vec(&original.recorder.commands).unwrap(),
            serde_json::to_vec(&restored.recorder.commands).unwrap()
        );
        finish_recording(&writer, restored.state.current_tick());
        drop(restored);
        drop(writer);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checkpoint_preserves_pending_input_memory_and_statistics() {
        let mut scenario = Scenario::skirmish();
        scenario.players[1].bot = true;
        scenario.players[1].bot_config = Some(oxide_sim::scenario::BotConfig::default());
        let mut original = Game::with_viewport(scenario, vec2(1280.0, 720.0)).unwrap();
        original.advance_ticks(121);
        original.issue(Command::Stop {
            units: vec![original.state.units()[0].id],
        });
        let before = original.state.hash();
        let bytes = serde_json::to_vec(&original).unwrap();
        assert_eq!(before, original.state.hash());
        let mut restored: Game = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(before, restored.state.hash());
        assert_eq!(*original.pending, *restored.pending);
        assert!(restored.presentation.paused);
        let start = restored.state.current_tick();
        assert_eq!(restored.recorder.start_tick(), start);
        assert!(restored.recorder.commands.is_empty());
        for _ in 0..240 {
            assert_eq!(original.do_tick().events, restored.do_tick().events);
            assert_eq!(original.state.hash(), restored.state.hash());
            assert_eq!(
                serde_json::to_vec(
                    &original
                        .recorder
                        .commands
                        .iter()
                        .filter(|c| c.tick >= start)
                        .collect::<Vec<_>>()
                )
                .unwrap(),
                serde_json::to_vec(&restored.recorder.commands).unwrap()
            );
            assert_eq!(
                original.live_stats.snapshot(&original.state),
                restored.live_stats.snapshot(&restored.state)
            );
        }
        assert_eq!(original.demo, restored.demo);
        assert_eq!(
            original.presentation.boundary_fog,
            restored.presentation.boundary_fog
        );
        original.issue(Command::Surrender);
        original.do_tick();
        assert!(original.state.result().is_some());
        let finished: Game =
            serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
        assert_eq!(original.state.hash(), finished.state.hash());
        assert_eq!(original.end_stats, finished.end_stats);
    }

    #[test]
    fn checkpoint_rejects_invalid_shell_metadata_before_installation() {
        let game = Game::with_viewport(Scenario::skirmish(), vec2(1280.0, 720.0)).unwrap();
        let original = serde_json::to_value(&game).unwrap();
        for (key, value) in [
            ("version", serde_json::json!(3)),
            ("human", serde_json::json!(255)),
        ] {
            let mut bad = original.clone();
            bad[key] = value;
            assert!(serde_json::from_value::<Game>(bad).is_err());
        }
        let mut bad = original;
        bad["boundary_fog"]["visible"] = serde_json::json!([{"x":-1,"y":-1}]);
        assert!(serde_json::from_value::<Game>(bad).is_err());
        assert_eq!(game.state.current_tick(), 0);
        assert!(game.pending.is_empty());
    }

    #[test]
    fn checkpoint_requires_the_scenarios_default_local_seat() {
        for human in [0, 1] {
            let mut scenario = Scenario::skirmish();
            for (seat, player) in scenario.players.iter_mut().enumerate() {
                player.bot = seat != human;
                player.bot_config = player.bot.then_some(Default::default());
            }
            let game = Game::with_viewport(scenario, vec2(1280.0, 720.0)).unwrap();
            let original = serde_json::to_value(&game).unwrap();
            let restored: Game = serde_json::from_value(original.clone()).unwrap();
            assert_eq!(restored.presentation.human, PlayerId(human as u8));
            let mut bad = original;
            bad["human"] = serde_json::json!(1 - human);
            let error = serde_json::from_value::<Game>(bad).err().unwrap();
            assert!(error.to_string().contains("invalid local seat"));
        }
        for bots in [false, true] {
            let mut scenario = Scenario::skirmish();
            for player in &mut scenario.players {
                player.bot = bots;
                player.bot_config = None;
            }
            let state = scenario.build().unwrap();
            let stats = oxide_kit::stats::LiveMatchStats::new(&state);
            let session =
                SessionCheckpoint::capture(&scenario, &state, &[], &[], Some(&stats)).unwrap();
            let checkpoint = GameCheckpoint {
                version: 2,
                session,
                human: PlayerId(0),
                demo: Default::default(),
                concede_stats: None,
                boundary_fog: crate::boundary_fog::BoundaryFog::new(&state, PlayerId(0)),
            };
            let game = checkpoint.restore().unwrap().install();
            assert_eq!(game.presentation.human, PlayerId(0));
        }
    }
}

#[cfg(test)]
mod background_tests {
    use super::*;

    #[test]
    fn pause_save_bulk_advance_and_replacement_keep_the_settled_boundary() {
        if std::thread::available_parallelism().map_or(1, |n| n.get()) < 2 {
            return; // A single-core host intentionally has no background executor.
        }
        let mut game = Game::with_viewport(Scenario::skirmish(), vec2(1280.0, 800.0)).unwrap();
        game.advance_ticks(120);
        game.issue(Command::Train {
            building: BuildingId(0),
            kind: UnitKind::Harvester,
        });
        let settled = serde_json::to_vec(&game).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while game.bot_decision.is_none() {
            game.prepare_bot_decision();
            assert!(
                std::time::Instant::now() < deadline,
                "executor admission never became available"
            );
            std::thread::yield_now();
        }
        game.prepare_bot_decision();
        game.presentation.paused = true;
        assert!(!game.advance_wall_clock(1.0, None));
        assert!(game.bot_decision.is_some());
        assert_eq!(serde_json::to_vec(&game).unwrap(), settled);
        let mut loaded: Game = serde_json::from_slice(&settled).unwrap();
        assert!(loaded.bot_decision.is_none());
        game.advance_ticks(120);
        loaded.advance_ticks(120);
        assert!(game.bot_decision.is_none());
        assert_eq!(game.hash_hex(), loaded.hash_hex());
        assert_eq!(
            serde_json::to_vec(&game).unwrap(),
            serde_json::to_vec(&loaded).unwrap()
        );
        game.prepare_bot_decision();
        let replacement = (*loaded.state).clone();
        game.replace_state_after_jump(&replacement);
        assert!(game.bot_decision.is_none());
        game.advance_ticks(120);
        loaded.advance_ticks(120);
        assert_eq!(game.hash_hex(), loaded.hash_hex());
    }
}
