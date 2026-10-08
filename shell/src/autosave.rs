//! Atomic player checkpoints and finished-match recordings.
//!
//! Failure is a first-class outcome here: a quit path that cannot
//! write its record must be able to say so before the process exits,
//! which is why [`SaveJob::run`] reports [`SaveOutcome`] and [`SaveError`]
//! instead of a bool.

use crate::game::Game;
#[cfg(test)]
use crate::game::GameReplay;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How many autosaves survive rotation.
const KEEP_AUTOSAVES: usize = 5;
/// How many finished-match records survive rotation. A decided match
/// is a keepsake, so it gets a wider shelf than live sessions.
const KEEP_MATCHES: usize = 20;
/// A temp older than this is an orphan from a crashed save, not a
/// write in flight.
const TEMP_ORPHAN_AGE: Duration = Duration::from_secs(3600);
/// Invalid-JSON ownership marker held at a destination until its record
/// atomically replaces it.
const RESERVATION_MARKER_PREFIX: &str = "oxide-save-reservation:";

/// What a successful [`SaveJob::run`] call actually did.
#[derive(Debug)]
pub enum SaveOutcome {
    /// A record landed at this path. Runtime quit flows need only the
    /// success; filesystem contract tests inspect the exact destination.
    #[cfg_attr(not(test), allow(dead_code))]
    Wrote(PathBuf),
    /// An unstarted game has nothing worth a file.
    NothingToSave,
    /// This session already wrote its record.
    AlreadySaved,
}

/// Why a save could not land — own-machine facts carrying the path
/// that refused, so the failure is diagnosable and reportable.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    /// No resolvable platform data directory (no HOME/APPDATA).
    #[error("no writable data directory")]
    NoDataDir,
    /// The autosave directory could not be created.
    #[error("could not create {path}: {source}")]
    CreateDir {
        /// The directory that refused.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// The record itself failed to write (disk full, permissions).
    #[error("could not write {path}: {source}")]
    Write {
        /// The record that failed.
        path: PathBuf,
        /// The underlying checkpoint or recording error.
        source: anyhow::Error,
    },
}

impl SaveError {
    /// One ASCII sentence for the toast strip and the failure dialog.
    pub fn player_line(&self) -> String {
        match self {
            SaveError::NoDataDir => {
                "could not save: no writable data folder is available".to_string()
            }
            SaveError::CreateDir { .. } => {
                "could not save: unable to create the save folder".to_string()
            }
            SaveError::Write { .. } => "could not save: unable to write the save file".to_string(),
        }
    }
}

/// Owns an exclusively-created path until an atomic write replaces its
/// marker. A failed write removes the marker, while an error reported
/// after rename leaves the published record intact.
struct PathReservation {
    path: PathBuf,
    marker: Vec<u8>,
    armed: bool,
}

impl PathReservation {
    fn publish<E>(
        mut self,
        write: impl FnOnce(&Path) -> Result<(), E>,
    ) -> Result<PathBuf, (PathBuf, E)> {
        match write(&self.path) {
            Ok(()) => {
                self.armed = false;
                Ok(self.path.clone())
            }
            Err(source) => Err((self.path.clone(), source)),
        }
    }
}

impl Drop for PathReservation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let still_our_marker = std::fs::metadata(&self.path)
            .is_ok_and(|metadata| metadata.len() == self.marker.len() as u64)
            && std::fs::read(&self.path).is_ok_and(|contents| contents == self.marker);
        if still_our_marker {
            std::fs::remove_file(&self.path).ok();
        }
    }
}

/// A collision-free `{prefix}-{tick}` path in `dir`, reserved by
/// `create_new` (`O_EXCL`). Every successful return owns its candidate;
/// collision suffixes have no arbitrary cutoff that can bypass the
/// exclusive create.
fn free_path(dir: &Path, prefix: &str, tick: u64, seed: u64) -> Result<PathReservation, SaveError> {
    static RESERVATION_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut n = 0u64;
    loop {
        let extension = if prefix == "match" { "json" } else { "oxsave" };
        let path = if n == 0 {
            dir.join(format!("{prefix}-{tick:010}.{extension}"))
        } else {
            dir.join(format!("{prefix}-{tick:010}-{seed}-{n}.{extension}"))
        };
        match std::fs::File::create_new(&path) {
            Ok(mut file) => {
                let nonce = RESERVATION_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let marker = format!(
                    "{RESERVATION_MARKER_PREFIX}{}:{nonce}\n",
                    std::process::id()
                )
                .into_bytes();
                if let Err(source) = file.write_all(&marker) {
                    drop(file);
                    std::fs::remove_file(&path).ok();
                    return Err(SaveError::Write {
                        path,
                        source: source.into(),
                    });
                }
                return Ok(PathReservation {
                    path,
                    marker,
                    armed: true,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(SaveError::Write {
                    path,
                    source: source.into(),
                });
            }
        }
        n = n.checked_add(1).ok_or_else(|| SaveError::Write {
            path,
            source: std::io::Error::other("exhausted save path suffixes").into(),
        })?;
    }
}

/// Wall-clock provenance for record metadata — never consumed by any
/// sim path (the sim's ban is on state, not on metadata).
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Retention runs per record kind: live sessions and finished matches
/// each rotate against their own budget, and anything else in the
/// directory — explicit saves included — is never touched. A shared
/// prefix-blind pool once let five quick quits evict every finished
/// match.
fn rotate(dir: &Path) {
    chassis::fsx::sweep_temps(dir, TEMP_ORPHAN_AGE);
    rotate_prefix(dir, "autosave-", KEEP_AUTOSAVES);
    rotate_prefix(dir, "match-", KEEP_MATCHES);
}

fn rotate_prefix(dir: &Path, prefix: &str, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension().and_then(|e| e.to_str())
                == Some(if prefix == "match-" { "json" } else { "oxsave" })
        })
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix))
        })
        .filter(|p| !is_reservation_marker(p))
        .collect();
    // Newest last by modification time; ties by name for determinism.
    files.sort_by_key(|p| {
        (
            std::fs::metadata(p).and_then(|m| m.modified()).ok(),
            p.clone(),
        )
    });
    while files.len() > keep {
        let oldest = files.remove(0);
        std::fs::remove_file(oldest).ok();
    }
}

fn is_reservation_marker(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut prefix = [0; RESERVATION_MARKER_PREFIX.len()];
    // Keep this read on one handle. If an atomic rename happens before
    // `open`, this sees and counts the completed replay. If it happens
    // afterward, this sees the old marker and defers its accounting until
    // the publishing saver rotates.
    file.read_exact(&mut prefix).is_ok() && prefix == RESERVATION_MARKER_PREFIX.as_bytes()
}

/// Header-eligible autosaves, newest first. Loading must still validate the payload.
pub(crate) fn candidates() -> Vec<PathBuf> {
    crate::saves::discover(|| false)
        .into_iter()
        .filter(|entry| entry.compatible && entry.kind == crate::saves::RecordKind::Autosave)
        .map(|entry| entry.path)
        .collect()
}

pub(crate) enum SaveData {
    Checkpoint(crate::game::checkpoint::SaveCapture),
    Recording(oxide_kit::GameReplay),
}

pub(crate) struct SaveJob {
    data: Option<SaveData>,
    meta: chassis::replay::ReplayMeta,
    seed: u64,
    dir: Option<PathBuf>,
    named: bool,
    already_saved: bool,
    recovery: Option<std::sync::Arc<oxide_kit::recovery::RecoveryWriter>>,
}

impl SaveJob {
    pub(crate) fn capture(game: &Game, name: Option<&str>) -> Self {
        let named = name.is_some();
        debug_assert!(
            !(named && game.net_role().is_some()),
            "LAN matches offer no named save"
        );
        let needed = named
            || (!game.autosave_done
                && (game.state.current_tick() != 0 || !game.pending.is_empty()));
        // A LAN match cannot resume on one machine, so it keeps the
        // watch-only record a decided match keeps.
        let recording = game.state.result().is_some() || game.net_role().is_some();
        let mut meta = game.recorder.meta.clone();
        meta.ticks = Some(game.state.current_tick());
        meta.saved_at = Some(now_unix());
        meta.kind = Some(
            if named {
                "save"
            } else if recording {
                "match"
            } else {
                "autosave"
            }
            .into(),
        );
        if let Some(name) = name {
            meta.description = Some(name.into());
        }
        let data = needed.then(|| {
            if !named && recording {
                let mut replay = game.recorder.clone();
                replay.meta = meta.clone();
                SaveData::Recording(replay)
            } else {
                SaveData::Checkpoint(game.capture_save())
            }
        });
        Self {
            data,
            meta,
            seed: game.scenario.seed,
            dir: if named {
                crate::paths::saves_dir()
            } else {
                crate::paths::autosave_dir()
            },
            named,
            already_saved: game.autosave_done,
            recovery: (!named).then(|| game.recovery.clone()).flatten(),
        }
    }

    pub(crate) fn run(self) -> Result<SaveOutcome, SaveError> {
        let outcome = if let Some(data) = self.data {
            let dir = self.dir.ok_or(SaveError::NoDataDir)?;
            std::fs::create_dir_all(&dir).map_err(|source| SaveError::CreateDir {
                path: dir.clone(),
                source,
            })?;
            let prefix = self.meta.kind.as_deref().unwrap();
            let path = free_path(&dir, prefix, self.meta.ticks.unwrap(), self.seed)?
                .publish(|path| match data {
                    SaveData::Checkpoint(capture) => {
                        crate::saved_game::write_capture(capture, self.meta.clone(), path)
                    }
                    SaveData::Recording(replay) => replay.save(path).map_err(Into::into),
                })
                .map_err(|(path, source)| SaveError::Write { path, source })?;
            if !self.named {
                rotate(&dir);
            }
            SaveOutcome::Wrote(path)
        } else if self.already_saved {
            SaveOutcome::AlreadySaved
        } else {
            SaveOutcome::NothingToSave
        };
        if let Some(writer) = self.recovery {
            crate::game::finish_recording(&writer, self.meta.ticks.unwrap());
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests;
