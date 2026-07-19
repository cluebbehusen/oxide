//! Replays: the complete input record of a deterministic run.
//!
//! A replay is `setup + tick-stamped commands` — with a deterministic sim,
//! that *is* the run. Any live session (human, bot, or agent over the debug
//! socket) can be saved and later re-executed headless, bit for bit, which
//! turns every play session into a potential regression test.
//!
//! This crate does not know what a setup or a command is; games instantiate
//! [`Replay`] with their own serde-able types. Files are JSON on purpose:
//! replays double as documentation, and agents read them directly.

use crate::Tick;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::Path;

/// A recorded run: metadata, initial setup, and every command ever issued.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Replay<S, C> {
    /// Provenance and context for the run.
    pub meta: ReplayMeta,
    /// Everything needed to construct the initial state (scenario, seed…).
    pub setup: S,
    /// All commands, in nondecreasing tick order.
    pub commands: Vec<TimedCommand<C>>,
}

/// Provenance carried alongside a replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayMeta {
    /// Version of the sim that recorded this replay. Replays are only
    /// guaranteed to reproduce on the version that wrote them.
    pub sim_version: String,
    /// Free-form context (who played, what was being tested).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A command stamped with the tick it executes on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimedCommand<C> {
    /// Execution tick.
    pub tick: Tick,
    /// The game-defined command.
    pub command: C,
}

/// Errors from loading or saving replay files.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// Filesystem failure.
    #[error("replay io: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed replay file.
    #[error("replay format: {0}")]
    Format(#[from] serde_json::Error),
}

impl<S, C> Replay<S, C> {
    /// Starts an empty replay for a run of `setup`.
    pub fn new(sim_version: impl Into<String>, setup: S) -> Self {
        Self {
            meta: ReplayMeta {
                sim_version: sim_version.into(),
                description: None,
            },
            setup,
            commands: Vec::new(),
        }
    }

    /// Appends a command. Panics if `tick` precedes the last recorded tick —
    /// a replay that is not in tick order is corrupt by definition.
    pub fn record(&mut self, tick: Tick, command: C) {
        if let Some(last) = self.commands.last() {
            assert!(
                tick >= last.tick,
                "commands must be recorded in tick order ({tick} < {})",
                last.tick
            );
        }
        self.commands.push(TimedCommand { tick, command });
    }

    /// A cursor for feeding commands back into a sim tick by tick.
    pub fn cursor(&self) -> ReplayCursor<'_, C> {
        ReplayCursor {
            commands: &self.commands,
            pos: 0,
        }
    }

    /// Writes the replay as pretty JSON.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ReplayError>
    where
        S: Serialize,
        C: Serialize,
    {
        let file = std::fs::File::create(path)?;
        serde_json::to_writer_pretty(std::io::BufWriter::new(file), self)?;
        Ok(())
    }

    /// Reads a replay written by [`Replay::save`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ReplayError>
    where
        S: DeserializeOwned,
        C: DeserializeOwned,
    {
        let file = std::fs::File::open(path)?;
        Ok(serde_json::from_reader(std::io::BufReader::new(file))?)
    }
}

/// Streams a replay's commands out in tick order.
#[derive(Debug)]
pub struct ReplayCursor<'a, C> {
    commands: &'a [TimedCommand<C>],
    pos: usize,
}

impl<'a, C> ReplayCursor<'a, C> {
    /// All commands stamped for exactly `tick`.
    ///
    /// Call with strictly increasing ticks; commands stamped earlier than the
    /// requested tick are skipped (they can only appear if ticks were skipped,
    /// in which case the replay cannot reproduce anyway).
    pub fn take_tick(&mut self, tick: Tick) -> &'a [TimedCommand<C>] {
        while self.pos < self.commands.len() && self.commands[self.pos].tick < tick {
            self.pos += 1;
        }
        let start = self.pos;
        while self.pos < self.commands.len() && self.commands[self.pos].tick == tick {
            self.pos += 1;
        }
        &self.commands[start..self.pos]
    }

    /// Whether every command has been consumed.
    pub fn is_finished(&self) -> bool {
        self.pos >= self.commands.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_yields_commands_grouped_by_tick() {
        let mut replay: Replay<(), &str> = Replay::new("0.0.0", ());
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
    #[should_panic(expected = "tick order")]
    fn recording_out_of_order_panics() {
        let mut replay: Replay<(), u8> = Replay::new("0.0.0", ());
        replay.record(5, 1);
        replay.record(4, 2);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = std::env::temp_dir().join("chassis-replay-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.json");

        let mut replay: Replay<u32, String> = Replay::new("1.2.3", 77);
        replay.record(1, "move".to_string());
        replay.save(&path).unwrap();

        let loaded: Replay<u32, String> = Replay::load(&path).unwrap();
        assert_eq!(loaded.meta.sim_version, "1.2.3");
        assert_eq!(loaded.setup, 77);
        assert_eq!(loaded.commands.len(), 1);
        assert_eq!(loaded.commands[0].command, "move");
    }
}
