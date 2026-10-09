//! Self-contained shell continuation; historical commands belong to recordings.

use super::*;
use crate::tutorial::Demo;
use oxide_kit::checkpoint::SessionCheckpoint;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GameCheckpoint {
    session: SessionCheckpoint,
    human: PlayerId,
    demo: crate::tutorial::Demo,
    concede_stats: Option<oxide_kit::stats::MatchStats>,
    boundary_fog: crate::boundary_fog::BoundaryFog,
}

pub(crate) struct SaveCapture {
    scenario: Scenario,
    state: Arc<State>,
    bots: Vec<SeatController>,
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
        let (core, boundary_fog) = if let Some(checkpoint) = record.checkpoint {
            let core = checkpoint.resume_recording_cancellable(&record.replay, &cancelled)?;
            let fog = crate::boundary_fog::BoundaryFog::new(
                &core.state,
                Game::local_seat(&core.scenario),
            );
            (core, fog)
        } else {
            let scenario = record.replay.setup.clone();
            record.replay.validate(Some(SIM_VERSION))?;
            anyhow::ensure!(
                record.replay.origin.is_none(),
                "missing recovery checkpoint"
            );
            let mut state = scenario.build()?;
            let human = Game::local_seat(&scenario);
            let mut boundary_fog = crate::boundary_fog::BoundaryFog::new(&state, human);
            let mut bots = seat_controllers(&scenario)?;
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
                oxide_kit::controller::record_events(&mut bots, &report);
                stats.observe(&state, &report.events);
                boundary_fog.observe(&state, human);
            }
            anyhow::ensure!(cursor.is_finished(), "unconsumed recovery commands");
            (
                oxide_kit::checkpoint::RestoredSession {
                    scenario,
                    state,
                    bots,
                    pending: Vec::new(),
                    stats: Some(stats),
                },
                boundary_fog,
            )
        };
        anyhow::ensure!(
            core.stats.is_some(),
            "shell recovery requires live statistics"
        );
        let human = Game::local_seat(&core.scenario);
        Ok(Self {
            core,
            recorder: record.replay,
            human,
            demo: Demo::default(),
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
            net: None,
            outbox: Vec::new(),
            recovery_root: None,
            recovery,
            recovery_warned: false,
            recovery_source: None,
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod background_tests;
