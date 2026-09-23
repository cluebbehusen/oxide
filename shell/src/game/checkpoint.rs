//! Self-contained shell continuation; historical commands belong to recordings.

use super::*;
use oxide_kit::checkpoint::SessionCheckpoint;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GameCheckpoint {
    version: u32,
    session: SessionCheckpoint,
    human: PlayerId,
    demo: crate::tutorial::Demo,
    concede_stats: Option<oxide_kit::stats::MatchStats>,
    boundary_fog: crate::boundary_fog::BoundaryFog,
}

impl Serialize for Game {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let core = SessionCheckpoint::capture(
            &self.scenario,
            &self.state,
            &self.bots,
            &self.pending,
            Some(&self.live_stats),
        )
        .map_err(serde::ser::Error::custom)?;
        GameCheckpoint {
            version: 2,
            session: core,
            human: self.presentation.human,
            demo: self.demo,
            concede_stats: self.concede_stats.clone(),
            boundary_fog: self.presentation.boundary_fog.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Game {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let checkpoint = GameCheckpoint::deserialize(deserializer)?;
        restore(checkpoint).map_err(serde::de::Error::custom)
    }
}

fn restore(checkpoint: GameCheckpoint) -> Result<Game> {
    anyhow::ensure!(
        checkpoint.version == 2,
        "unsupported shell checkpoint version"
    );
    let recorder = checkpoint.session.recording()?;
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
    let live_stats = core
        .stats
        .ok_or_else(|| anyhow::anyhow!("shell checkpoint requires live statistics"))?;
    let mut presentation =
        Presentation::new(&core.state, checkpoint.human, crate::render::viewport());
    presentation.reset_after_jump(&core.state);
    presentation.boundary_fog = checkpoint.boundary_fog;
    presentation.paused = true;
    presentation.conceded_banner = checkpoint.concede_stats.is_some();
    let end_stats = core
        .state
        .result()
        .map(|_| live_stats.snapshot(&core.state));
    Ok(Game {
        scenario: core.scenario,
        state: ReadOnlyState(core.state),
        bots: core.bots,
        recorder,
        pending: PendingCommands(core.pending),
        live_stats,
        end_stats,
        concede_stats: checkpoint.concede_stats,
        demo: checkpoint.demo,
        presentation,
        autosave_done: false,
        suppress_presentation: false,
        recovery_root: None,
        recovery: None,
        recovery_warned: false,
        diagnostics_warned: false,
        recovery_source: None,
        diagnostics: None,
    })
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
        let mut restored = Game::from_recovery(record, None).unwrap();
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
            let game = restore(checkpoint).unwrap();
            assert_eq!(game.presentation.human, PlayerId(0));
        }
    }
}
