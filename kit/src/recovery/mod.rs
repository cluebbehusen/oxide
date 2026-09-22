//! Incremental, bounded recovery records. A prepared tick is not a completed tick.

#[cfg(test)]
mod tests;
mod writer;

use crate::{GameReplay, MAX_REPLAY_TICKS};
use anyhow::{Context, Result, bail, ensure};
use oxide_sim::{PlayerCommand, SIM_VERSION};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub use writer::{RecoveryWriter, WriterStatus};

pub(crate) const MAX_BYTES: u64 = chassis::replay::MAX_REPLAY_BYTES as u64;
pub(crate) const MAGIC: &[u8; 8] = b"OXREC001";
/// Aggregate managed recording budget, excluding explicitly exported reports.
pub const MANAGED_BYTES: u64 = 256 * 1024 * 1024;

/// Build provenance does not participate in replay compatibility or state hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    /// Package version.
    pub version: String,
    /// Recording commit, or `unknown` for a source archive.
    pub revision: String,
    /// Whether the executable's package/dependency trees and shared build inputs
    /// had changes: `true`, `false`, or `unknown` when source could not be inspected.
    pub dirty: String,
    /// Build operating system.
    pub os: String,
    /// Build architecture.
    pub architecture: String,
}
impl Default for BuildIdentity {
    fn default() -> Self {
        Self::new(env!("CARGO_PKG_VERSION"), "unknown", "unknown")
    }
}

impl BuildIdentity {
    /// Identity captured by the host executable's build script.
    pub fn new(version: &str, revision: &str, dirty: &str) -> Self {
        Self {
            version: version.into(),
            revision: revision.into(),
            dirty: dirty.into(),
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
        }
    }
}

/// Whether a recording can resume live play or only reproduce a viewer session.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingKind {
    /// A live match's completed command history.
    #[default]
    LiveMatch,
    /// The complete source replay retained for playback diagnostics.
    Playback,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Header {
    #[serde(default)]
    kind: RecordingKind,
    session: String,
    build: BuildIdentity,
    #[serde(deserialize_with = "crate::replay::deserialize_replay")]
    base: GameReplay,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    checkpoint: Option<crate::checkpoint::SessionCheckpoint>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    session: String,
    sequence: u64,
    event: Event,
}
#[derive(Serialize, Deserialize)]
pub(crate) enum Event {
    Prepared {
        tick: u64,
        commands: Vec<PlayerCommand>,
    },
    Completed {
        tick: u64,
    },
    Clean {
        tick: u64,
    },
}

/// An intact replay prefix and separately labelled evidence beyond that prefix.
#[derive(Serialize)]
pub struct Inspection {
    /// Whether the record belongs to live play or replay viewing.
    pub kind: RecordingKind,
    /// Recorded build, which may differ even when SIM_VERSION matches.
    pub build: BuildIdentity,
    /// Unique recording identity.
    pub session: String,
    /// Only commands belonging to verified completed ticks.
    pub replay: GameReplay,
    /// Live controller/session origin, separate from the world-only replay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<crate::checkpoint::SessionCheckpoint>,
    /// A prepared but uncompleted command batch, never replayed implicitly.
    pub prepared: Option<Vec<PlayerCommand>>,
    /// Corrupt or truncated tail, if any; preceding complete records remain usable.
    pub issue: Option<String>,
    /// Whether a durable clean-close record ended the intact journal.
    pub clean: bool,
}

struct BoundedBuffer(Vec<u8>);
impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) as u64 > MAX_BYTES {
            return Err(std::io::Error::other("recovery payload limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn pretty_size(value: &impl Serialize) -> Result<usize> {
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            if self.0 as u64 > MAX_BYTES {
                return Err(std::io::Error::other("replay size limit"));
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer_pretty(&mut count, value)?;
    Ok(count.0)
}

pub(crate) fn write_frame(file: &mut impl Write, value: &impl Serialize) -> Result<usize> {
    let mut bounded = BoundedBuffer(Vec::new());
    serde_json::to_writer(&mut bounded, value)?;
    let bytes = bounded.0;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "recovery record exceeds size limit"
    );
    file.write_all(&(bytes.len() as u32).to_le_bytes())?;
    file.write_all(&chassis::hash::fnv1a(&bytes).to_le_bytes())?;
    file.write_all(&bytes)?;
    Ok(bytes.len() + 12)
}
fn read_frame(reader: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut length = [0; 4];
    if reader.read(&mut length[..1])? == 0 {
        return Ok(None);
    }
    reader
        .read_exact(&mut length[1..])
        .context("truncated length")?;
    let length = u32::from_le_bytes(length) as usize;
    ensure!(
        length > 0 && length as u64 <= MAX_BYTES,
        "invalid recovery frame length"
    );
    let mut checksum = [0; 8];
    reader
        .read_exact(&mut checksum)
        .context("truncated checksum")?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).context("truncated payload")?;
    ensure!(
        chassis::hash::fnv1a(&bytes) == u64::from_le_bytes(checksum),
        "recovery checksum mismatch"
    );
    Ok(Some(bytes))
}

/// Verify a recording without a running game. Only its intact completed prefix is playable.
pub fn inspect(directory: &Path) -> Result<Inspection> {
    if !directory.join("recovery.bin").exists() {
        return inspect_report(directory);
    }
    let file = File::open(directory.join("recovery.bin"))?;
    ensure!(
        file.metadata()?.len() <= MAX_BYTES,
        "journal exceeds recovery size limit"
    );
    inspect_reader(&mut std::io::BufReader::new(file))
}
fn inspect_report(directory: &Path) -> Result<Inspection> {
    let path = directory.join("manifest.json");
    ensure!(
        std::fs::metadata(&path)?.len() <= MAX_BYTES,
        "report manifest too large"
    );
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        manifest["format"] == 1 && manifest["complete"] == true,
        "incomplete or unsupported report"
    );
    let replay = crate::load_replay(directory.join("replay.json"))?;
    ensure!(
        manifest["replay_digest"].as_u64() == Some(chassis::hash::state_hash(&replay)),
        "report replay digest mismatch"
    );
    let session = manifest["session"]
        .as_str()
        .context("missing report session")?
        .to_owned();
    ensure!(
        !session.is_empty() && session.len() <= 128,
        "invalid report session"
    );
    let prepared: Option<Vec<PlayerCommand>> =
        serde_json::from_value(manifest["prepared_commands"].clone())?;
    ensure!(
        prepared.is_none() || manifest["prepared_tick"].as_u64() == replay.meta.ticks,
        "invalid report prepared tick"
    );
    let checkpoint = manifest
        .get("checkpoint")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .flatten();
    let kind = manifest
        .get("kind")
        .cloned()
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    validate_origin(kind, &replay, checkpoint.as_ref())?;
    Ok(Inspection {
        kind,
        build: serde_json::from_value(manifest["build"].clone())?,
        session,
        replay,
        checkpoint,
        prepared,
        issue: serde_json::from_value(manifest["issue"].clone())?,
        clean: manifest["clean"]
            .as_bool()
            .context("missing report close state")?,
    })
}

fn inspect_reader(reader: &mut impl Read) -> Result<Inspection> {
    let mut magic = [0; 8];
    reader.read_exact(&mut magic)?;
    ensure!(&magic == MAGIC, "unsupported recovery format");
    let header: Header =
        serde_json::from_slice(&read_frame(reader)?.context("missing recovery header")?)?;
    ensure!(
        !header.session.is_empty() && header.session.len() <= 128,
        "invalid session identity"
    );
    validate_origin(header.kind, &header.base, header.checkpoint.as_ref())?;
    let mut tick = header
        .base
        .meta
        .ticks
        .context("recovery base has no duration")?;
    ensure!(tick <= MAX_REPLAY_TICKS, "recovery tick limit");
    ensure!(
        header.base.commands.len() <= chassis::replay::MAX_REPLAY_COMMANDS,
        "recovery command limit"
    );
    let mut result = Inspection {
        kind: header.kind,
        build: header.build,
        session: header.session,
        replay: header.base,
        checkpoint: header.checkpoint,
        prepared: None,
        issue: None,
        clean: false,
    };
    let mut sequence = 0;
    loop {
        let record = match read_frame(reader).and_then(|bytes| {
            bytes
                .map(|b| serde_json::from_slice::<Record>(&b).map_err(Into::into))
                .transpose()
        }) {
            Ok(Some(record)) => record,
            Ok(None) => break,
            Err(error) => {
                result.issue = Some(error.to_string());
                break;
            }
        };
        let apply = || -> Result<()> {
            ensure!(
                record.session == result.session && record.sequence == sequence,
                "session or sequence discontinuity"
            );
            ensure!(!result.clean, "record after clean close");
            ensure!(
                result.kind == RecordingKind::LiveMatch
                    || matches!(record.event, Event::Clean { .. }),
                "playback source replay is immutable"
            );
            match &record.event {
                Event::Prepared { tick: at, commands } => {
                    ensure!(
                        *at == tick && tick < MAX_REPLAY_TICKS && result.prepared.is_none(),
                        "invalid prepared tick"
                    );
                    ensure!(
                        result.replay.commands.len().saturating_add(commands.len())
                            <= chassis::replay::MAX_REPLAY_COMMANDS,
                        "recovery command limit"
                    );
                    if *at == result.replay.start_tick()
                        && let Some(checkpoint) = &result.checkpoint
                    {
                        checkpoint.validate_first_batch(commands)?;
                    }
                }
                Event::Completed { tick: at } => ensure!(
                    *at == tick + 1 && result.prepared.is_some(),
                    "completion without its prepared tick"
                ),
                Event::Clean { tick: at } => ensure!(
                    *at == tick && result.prepared.is_none(),
                    "clean close has unfinished work"
                ),
            }
            Ok(())
        };
        if let Err(error) = apply() {
            result.issue = Some(error.to_string());
            break;
        }
        match record.event {
            Event::Prepared { commands, .. } => result.prepared = Some(commands),
            Event::Completed { .. } => {
                for command in result.prepared.take().expect("validated prepared tick") {
                    result.replay.record(tick, command);
                }
                tick += 1;
                result.replay.meta.ticks = Some(tick);
            }
            Event::Clean { .. } => result.clean = true,
        }
        sequence += 1;
    }
    if result.issue.is_some() {
        result.clean = false;
    }
    result.replay.validate(Some(SIM_VERSION))?;
    Ok(result)
}

pub(crate) fn validate_origin(
    kind: RecordingKind,
    replay: &GameReplay,
    checkpoint: Option<&crate::checkpoint::SessionCheckpoint>,
) -> Result<()> {
    replay.validate(Some(SIM_VERSION))?;
    crate::recording::initial_state(replay)?;
    match checkpoint {
        Some(checkpoint) => {
            ensure!(
                kind == RecordingKind::LiveMatch,
                "playback must not carry controller state"
            );
            checkpoint.validate_origin(replay)?;
        }
        None => ensure!(
            kind == RecordingKind::Playback || replay.origin.is_none(),
            "live recovery origin requires a session checkpoint"
        ),
    }
    Ok(())
}

/// An inactive interrupted recording and its completed duration.
pub struct InterruptedMatch {
    /// Recording directory.
    pub directory: PathBuf,
    /// Completed simulation ticks available.
    pub ticks: u64,
    /// Display name of the recorded scenario.
    pub scenario: String,
}

pub(crate) fn read_lease(directory: &Path) -> Option<File> {
    let file = File::options()
        .read(true)
        .write(true)
        .open(directory.join("lease"))
        .ok()?;
    file.try_lock_shared().ok()?;
    Some(file)
}

pub(crate) fn inactive(directory: &Path) -> Option<File> {
    let file = File::options()
        .read(true)
        .write(true)
        .open(directory.join("lease"))
        .ok()?;
    file.try_lock().ok()?;
    Some(file)
}

/// Find the newest inactive compatible interrupted record without resuming it.
pub fn latest_interrupted(root: &Path) -> Option<InterruptedMatch> {
    latest_record(root, true)
}

/// Find diagnostic evidence even when the first simulation tick never completed.
pub fn latest_diagnostic_record(root: &Path) -> Option<InterruptedMatch> {
    latest_record(root, false)
}

fn latest_record(root: &Path, require_completed_tick: bool) -> Option<InterruptedMatch> {
    let mut directories = session_directories(root);
    directories.sort();
    for directory in directories.into_iter().rev() {
        let Some(_lease) = read_lease(&directory) else {
            continue;
        };
        let Ok(record) = inspect(&directory) else {
            continue;
        };
        let ticks = record.replay.meta.ticks.unwrap_or(0);
        let superseded = std::fs::read(directory.join("superseded.json"))
            .ok()
            .filter(|bytes| bytes.len() <= 1024)
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|marker| {
                marker["ticks"].as_u64() == Some(ticks)
                    && marker["by"]
                        .as_str()
                        .is_some_and(|by| by.starts_with("session-"))
            });
        if !record.clean
            && !superseded
            && (!require_completed_tick || (ticks > 0 && record.kind == RecordingKind::LiveMatch))
        {
            return Some(InterruptedMatch {
                directory,
                ticks,
                scenario: record.replay.setup.name,
            });
        }
    }
    None
}

pub(crate) fn session_directories(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.file_name().to_string_lossy().starts_with("session-")
        })
        .map(|entry| entry.path())
        .collect()
}

/// Export a consistent verified prefix and available diagnostics to a new report directory.
/// Existing destinations are refused; named saves and source records are never replaced.
pub fn export(directory: &Path, destination: &Path, running_build: &BuildIdentity) -> Result<()> {
    ensure!(!destination.exists(), "report destination already exists");
    let readers = if directory.join("recovery.bin").exists() {
        let file = File::options()
            .read(true)
            .write(true)
            .open(directory.join("readers"))?;
        file.try_lock_shared()
            .context("recording is being retired")?;
        Some(file)
    } else {
        None
    };
    let record = inspect(directory)?;
    std::fs::create_dir(destination)?;
    let result = (|| -> Result<()> {
        record.replay.save(destination.join("replay.json"))?;
        #[cfg(test)]
        tests::fault("export");
        let manifest = serde_json::json!({ "format": 1, "complete": true, "kind": record.kind, "replay_digest": chassis::hash::state_hash(&record.replay), "session": record.session, "build": record.build, "running_build": running_build, "sim_version": SIM_VERSION, "ticks": record.replay.meta.ticks, "clean": record.clean, "issue": record.issue, "prepared_tick": record.prepared.as_ref().map(|_| record.replay.meta.ticks), "prepared_commands": record.prepared });
        let mut manifest = manifest;
        if let Some(checkpoint) = &record.checkpoint {
            manifest["checkpoint"] = serde_json::to_value(checkpoint)?;
        }
        ensure!(
            pretty_size(&manifest)? <= MAX_BYTES as usize,
            "report manifest too large"
        );
        for name in [
            "timings.json",
            "watchdog.json",
            "context.json",
            "status.json",
            "previous-manifest.json",
            "previous-timings.json",
            "previous-watchdog.json",
            "previous-context.json",
            "previous-status.json",
        ] {
            let source = directory.join(name);
            if let Ok(metadata) = std::fs::symlink_metadata(&source) {
                ensure!(
                    metadata.is_file() && metadata.len() <= 16 * 1024 * 1024,
                    "invalid diagnostic sidecar"
                );
                let bytes = std::fs::read(&source)?;
                chassis::fsx::write_atomic(destination.join(name), |writer| {
                    writer.write_all(&bytes)
                })?;
            }
        }
        chassis::fsx::write_atomic(destination.join("manifest.json"), |writer| {
            serde_json::to_writer_pretty(writer, &manifest).map_err(std::io::Error::other)
        })?;
        Ok(())
    })();
    if let Err(error) = result {
        // The directory was exclusively created by this invocation.
        let _ = std::fs::remove_dir_all(destination);
        bail!("report export failed: {error:#}");
    }
    drop(readers);
    Ok(())
}
