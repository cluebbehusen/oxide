//! Replays: the complete input record of a deterministic run.
//!
//! A replay is setup, an optional game-owned origin, and tick-stamped commands.
//! With a deterministic sim that is the whole run: any live session (human,
//! bot, or agent over the debug socket) can be saved and re-executed headless,
//! bit for bit.
//!
//! This crate does not know what a setup or a command is; games instantiate
//! [`Replay`] with their own serde-able types. Files are JSON so people and
//! agents can read them directly.

use crate::Tick;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::Read as _;
use std::path::Path;

/// Largest replay document accepted from disk. Bounding bytes before JSON
/// parsing keeps an untrusted file from forcing an unbounded allocation.
pub const MAX_REPLAY_BYTES: usize = 64 << 20;

/// Most commands accepted in a loaded replay.
pub const MAX_REPLAY_COMMANDS: usize = 1_000_000;

/// A recorded segment: metadata, setup, optional origin, and recorded commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "S: Deserialize<'de>, C: Deserialize<'de>, O: Deserialize<'de>"))]
pub struct Replay<S, C, O = ()> {
    /// Provenance and context for the run.
    pub meta: ReplayMeta,
    /// Everything needed to construct the initial state (scenario, seed…).
    pub setup: S,
    /// All commands, in nondecreasing tick order.
    pub commands: Vec<TimedCommand<C>>,
    /// Optional game-owned snapshot from which this segment starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<O>,
}

/// Game-specific validation for a recording that starts after setup.
pub trait RecordingOrigin<S> {
    /// Absolute tick of the saved state, before commands at this tick execute.
    fn start_tick(&self) -> Tick;
    /// Validate the snapshot and its relationship to the authored setup.
    fn validate(&self, setup: &S) -> Result<(), ReplayError>;
}

impl<S> RecordingOrigin<S> for () {
    fn start_tick(&self) -> Tick {
        0
    }
    fn validate(&self, _setup: &S) -> Result<(), ReplayError> {
        Ok(())
    }
}

/// Provenance carried alongside a replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayMeta {
    /// Version of the sim that recorded this replay. Replays are only
    /// guaranteed to reproduce on the version that wrote them.
    pub sim_version: u32,
    /// Free-form context (who played, what was being tested).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Absolute end tick, so playback knows when the segment is fully
    /// reproduced (commands alone only bound it from below).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks: Option<Tick>,
    /// What kind of record this is. Chassis assigns no meaning; games write
    /// and classify their own tags (Oxide uses "autosave", "save", "match").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Wall-clock save time in unix seconds, supplied by the caller. It is
    /// recorder provenance only; no sim path or hash consumes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<u64>,
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
        recorded: u32,
        /// Version doing the loading.
        running: u32,
    },
}

impl<S, C, O: RecordingOrigin<S>> Replay<S, C, O> {
    /// Starts an empty replay for a run of `setup`.
    pub fn new(sim_version: u32, setup: S) -> Self {
        Self {
            meta: ReplayMeta {
                sim_version,
                description: None,
                ticks: None,
                kind: None,
                saved_at: None,
            },
            setup,
            commands: Vec::new(),
            origin: None,
        }
    }

    /// Starts a segment at a validated snapshot, keeping absolute command ticks.
    pub fn with_origin(sim_version: u32, setup: S, origin: O) -> Result<Self, ReplayError> {
        let mut replay = Self::new(sim_version, setup);
        replay.meta.ticks = Some(origin.start_tick());
        replay.origin = Some(origin);
        replay.validate(None)?;
        Ok(replay)
    }

    /// First absolute tick available in this recording.
    pub fn start_tick(&self) -> Tick {
        self.origin.as_ref().map_or(0, RecordingOrigin::start_tick)
    }

    /// Appends a command. Panics if `tick` precedes the recording origin or
    /// the last recorded tick.
    pub fn record(&mut self, tick: Tick, command: C) {
        assert!(
            tick >= self.start_tick(),
            "command precedes recording origin"
        );
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
    /// does not; a file is untrusted input even when it parses.
    ///
    /// Verifies the command count and origin, that command ticks are
    /// nondecreasing and start at or after the origin, that no tick sits at
    /// the counter's ceiling, that the recorded duration covers every
    /// command, and (when `expected_version` is given) that the file was
    /// written by this sim. Call before executing any loaded replay; a log that
    /// fails these can silently produce a different world, or panic the
    /// recorder later.
    pub fn validate(&self, expected_version: Option<u32>) -> Result<(), ReplayError> {
        self.validate_command_count(MAX_REPLAY_COMMANDS)?;
        if let Some(origin) = &self.origin {
            origin.validate(&self.setup)?;
        }
        let start = self.start_tick();
        if start == u64::MAX
            || self.meta.ticks.is_some_and(|end| end < start)
            || self
                .commands
                .first()
                .is_some_and(|command| command.tick < start)
        {
            return Err(ReplayError::Invalid(
                "recording precedes its origin or has no tick headroom".into(),
            ));
        }
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
                recorded: self.meta.sim_version,
                running: expected,
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

    /// Writes the replay as pretty JSON through [`crate::fsx::write_atomic`].
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ReplayError>
    where
        S: Serialize,
        C: Serialize,
        O: Serialize,
    {
        crate::fsx::write_atomic(path, |writer| {
            serde_json::to_writer_pretty(&mut *writer, self)?;
            Ok(())
        })
    }

    /// Reads a replay written by [`Replay::save`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ReplayError>
    where
        S: DeserializeOwned,
        C: DeserializeOwned,
        O: DeserializeOwned,
    {
        Self::load_with_limits(path, MAX_REPLAY_BYTES, MAX_REPLAY_COMMANDS)
    }

    /// Reads a bounded replay file and delegates its wire format to a
    /// game-specific decoder. The decoded replay still receives the shared
    /// command-count check.
    ///
    /// Use this when a game must inspect versioned setup metadata before it
    /// can produce the current `S`, without duplicating the file-size and
    /// command-count boundary owned by chassis.
    pub fn load_with_decoder(
        path: impl AsRef<Path>,
        decoder: impl FnOnce(&[u8]) -> Result<Self, ReplayError>,
    ) -> Result<Self, ReplayError> {
        Self::load_with_limits_and_decoder(path, MAX_REPLAY_BYTES, MAX_REPLAY_COMMANDS, decoder)
    }

    fn load_with_limits(
        path: impl AsRef<Path>,
        max_bytes: usize,
        max_commands: usize,
    ) -> Result<Self, ReplayError>
    where
        S: DeserializeOwned,
        C: DeserializeOwned,
        O: DeserializeOwned,
    {
        Self::load_with_limits_and_decoder(path, max_bytes, max_commands, |bytes| {
            Ok(serde_json::from_slice(bytes)?)
        })
    }

    fn load_with_limits_and_decoder(
        path: impl AsRef<Path>,
        max_bytes: usize,
        max_commands: usize,
        decoder: impl FnOnce(&[u8]) -> Result<Self, ReplayError>,
    ) -> Result<Self, ReplayError> {
        let file = std::fs::File::open(path)?;
        let length = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
        if length > max_bytes {
            return Err(ReplayError::Invalid(format!(
                "file is {length} bytes, beyond the {max_bytes}-byte limit"
            )));
        }

        let mut bytes = Vec::with_capacity(length);
        file.take(max_bytes as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(ReplayError::Invalid(format!(
                "file grew beyond the {max_bytes}-byte limit while being read"
            )));
        }

        let replay = decoder(&bytes)?;
        replay.validate_command_count(max_commands)?;
        Ok(replay)
    }

    fn validate_command_count(&self, max_commands: usize) -> Result<(), ReplayError> {
        if self.commands.len() > max_commands {
            return Err(ReplayError::Invalid(format!(
                "record contains {} commands, beyond the {max_commands}-command limit",
                self.commands.len()
            )));
        }
        Ok(())
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
mod tests;
