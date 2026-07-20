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
    /// Total ticks the recorded session ran, so playback knows when the
    /// run is fully reproduced (commands alone only bound it from below).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks: Option<Tick>,
}

/// A command stamped with the tick it executes on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimedCommand<C> {
    /// Execution tick.
    pub tick: Tick,
    /// The game-defined command.
    pub command: C,
}

/// Errors from loading, saving, or validating replay files.
#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    /// Filesystem failure.
    #[error("replay io: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed replay file.
    #[error("replay format: {0}")]
    Format(#[from] serde_json::Error),
    /// Structurally broken replay (recording invariants don't hold).
    #[error("invalid replay: {0}")]
    Invalid(String),
    /// The replay was recorded on a different sim. Deterministic playback
    /// is only guaranteed on the version that wrote it.
    #[error("replay was recorded on sim {recorded}, this is {running}")]
    VersionMismatch {
        /// Version stamped in the file.
        recorded: String,
        /// Version doing the loading.
        running: String,
    },
}

impl<S, C> Replay<S, C> {
    /// Starts an empty replay for a run of `setup`.
    pub fn new(sim_version: impl Into<String>, setup: S) -> Self {
        Self {
            meta: ReplayMeta {
                sim_version: sim_version.into(),
                description: None,
                ticks: None,
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

    /// Checks the invariants recording enforces but deserialization alone
    /// does not — a file is untrusted input even when it parses.
    ///
    /// Verifies command ticks are nondecreasing, that no tick sits at the
    /// counter's ceiling, the recorded duration covers every command, and
    /// (when `expected_version` is given) that the file was written by
    /// this sim. Structure is checked *before* version: callers that
    /// deliberately tolerate a [`ReplayError::VersionMismatch`] must never
    /// thereby accept a malformed log. Call before executing any loaded
    /// replay; a log that fails these can silently produce a different
    /// world, or panic the recorder later.
    pub fn validate(&self, expected_version: Option<&str>) -> Result<(), ReplayError> {
        for pair in self.commands.windows(2) {
            if pair[1].tick < pair[0].tick {
                return Err(ReplayError::Invalid(format!(
                    "commands out of order: tick {} follows {}",
                    pair[1].tick, pair[0].tick
                )));
            }
        }
        if let Some(last) = self.commands.last() {
            // Playback needs at least one tick after the final command;
            // u64::MAX would overflow every "last + 1" downstream.
            if last.tick == u64::MAX {
                return Err(ReplayError::Invalid(
                    "final command sits at the tick counter's ceiling".into(),
                ));
            }
            if let Some(ticks) = self.meta.ticks
                && ticks <= last.tick
            {
                return Err(ReplayError::Invalid(format!(
                    "recorded duration {ticks} does not cover the last command at tick {}",
                    last.tick
                )));
            }
        }
        if let Some(expected) = expected_version
            && self.meta.sim_version != expected
        {
            return Err(ReplayError::VersionMismatch {
                recorded: self.meta.sim_version.clone(),
                running: expected.to_string(),
            });
        }
        Ok(())
    }

    /// A cursor for feeding commands back into a sim tick by tick.
    pub fn cursor(&self) -> ReplayCursor<'_, C> {
        ReplayCursor {
            commands: &self.commands,
            pos: 0,
        }
    }

    /// Writes the replay as pretty JSON: parent directories are created,
    /// the write is flushed, and the file lands atomically (tmp + rename)
    /// so a crash mid-save can't leave a truncated log behind.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ReplayError>
    where
        S: Serialize,
        C: Serialize,
    {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        // Process-unique temp name: two sessions saving the same stem
        // concurrently must not clobber each other's half-written file.
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        {
            let file = std::fs::File::create(&tmp)?;
            let mut writer = std::io::BufWriter::new(file);
            serde_json::to_writer_pretty(&mut writer, self)?;
            use std::io::Write as _;
            writer.flush()?;
        }
        std::fs::rename(&tmp, path)?;
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
    fn validate_accepts_what_record_produces() {
        let mut replay: Replay<(), u8> = Replay::new("1.0.0", ());
        replay.record(3, 1);
        replay.record(3, 2);
        replay.record(9, 3);
        replay.meta.ticks = Some(10);
        assert!(replay.validate(Some("1.0.0")).is_ok());
        assert!(replay.validate(None).is_ok());
    }

    #[test]
    fn validate_checks_structure_before_version() {
        // Callers may deliberately tolerate a version mismatch (replay
        // archaeology); that tolerance must never smuggle in a malformed
        // log. Both defects present -> the structural error wins.
        let mut replay: Replay<(), u8> = Replay::new("0.9.0", ());
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
            replay.validate(Some("1.0.0")),
            Err(ReplayError::Invalid(_))
        ));
    }

    #[test]
    fn validate_rejects_a_command_at_the_tick_ceiling() {
        // Playback computes "last tick + 1"; u64::MAX must die here, not
        // overflow there.
        let mut replay: Replay<(), u8> = Replay::new("1.0.0", ());
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
        let mut replay: Replay<(), u8> = Replay::new("1.0.0", ());
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
        let mut replay: Replay<(), u8> = Replay::new("1.0.0", ());
        replay.record(9, 1);
        replay.meta.ticks = Some(0);
        assert!(matches!(
            replay.validate(None),
            Err(ReplayError::Invalid(_))
        ));

        // Wrong sim version.
        let replay: Replay<(), u8> = Replay::new("0.9.9", ());
        assert!(matches!(
            replay.validate(Some("1.0.0")),
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
        let replay: Replay<u8, u8> = Replay::new("1.0.0", 1);
        replay.save(&path).unwrap();
        assert!(path.exists());
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
