//! Oxide's replay file boundary.

use crate::GameReplay;
use chassis::replay::{Replay, ReplayError, ReplayMeta, TimedCommand};
use oxide_sim::{PlayerCommand, Scenario};
use serde::Deserialize;
use std::path::Path;

/// The replay envelope, strict about unknown fields; [`Scenario`] is strict
/// about its own.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayWire {
    meta: ReplayMeta,
    setup: Scenario,
    commands: Vec<TimedCommand<PlayerCommand>>,
    #[serde(default)]
    origin: Option<crate::recording::WorldOrigin>,
}

impl From<ReplayWire> for GameReplay {
    fn from(wire: ReplayWire) -> Self {
        let ReplayWire {
            meta,
            setup,
            commands,
            origin,
        } = wire;
        Replay {
            meta,
            setup,
            commands,
            origin,
        }
    }
}

pub(crate) fn deserialize_replay<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<GameReplay, D::Error> {
    ReplayWire::deserialize(decoder).map(GameReplay::from)
}

/// Absolute end tick of a replay: its recorded duration, or one past its last
/// command when the metadata omits it.
pub fn replay_duration(replay: &GameReplay) -> u64 {
    replay.meta.ticks.unwrap_or_else(|| {
        replay
            .commands
            .last()
            .map_or(replay.start_tick(), |command| {
                command.tick.saturating_add(1)
            })
    })
}

/// [`replay_duration`], refused beyond [`crate::MAX_REPLAY_TICKS`]. The bound
/// is on the effective duration, so a replay without duration metadata is
/// bounded by its final command's tick.
pub fn bounded_replay_duration(replay: &GameReplay) -> anyhow::Result<u64> {
    let total = replay_duration(replay);
    anyhow::ensure!(
        total <= crate::MAX_REPLAY_TICKS,
        "replay spans {total} ticks, beyond the {}-tick bound",
        crate::MAX_REPLAY_TICKS
    );
    Ok(total)
}

/// Feeds a replay's recorded commands back into a state one tick at a time.
pub struct ReplayPlayback<'a> {
    cursor: chassis::replay::ReplayCursor<'a, PlayerCommand>,
}

impl<'a> ReplayPlayback<'a> {
    /// Starts at the replay's first recorded command.
    pub fn new(replay: &'a GameReplay) -> Self {
        Self {
            cursor: replay.cursor(),
        }
    }

    /// Runs the state's current tick with the commands recorded for it.
    pub fn step(&mut self, state: &mut oxide_sim::State) -> oxide_sim::TickReport {
        let commands: Vec<PlayerCommand> = self
            .cursor
            .take_tick(state.current_tick())
            .iter()
            .map(|timed| timed.command.clone())
            .collect();
        state.tick(&commands)
    }

    /// Whether every recorded command has been played. A full-length
    /// playback that leaves commands behind means the replay's duration
    /// metadata is wrong.
    pub fn is_finished(&self) -> bool {
        self.cursor.is_finished()
    }
}

/// Loads an Oxide replay from disk.
pub fn load_replay(path: impl AsRef<Path>) -> Result<GameReplay, ReplayError> {
    GameReplay::load_with_decoder(path, |bytes| {
        Ok(serde_json::from_slice::<ReplayWire>(bytes)?.into())
    })
}

#[cfg(test)]
mod tests;
