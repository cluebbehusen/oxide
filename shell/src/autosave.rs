//! Autosaves: the recorder is always on, so quitting mid-match costs a
//! file write, and Continue is a replay load. Save-is-a-replay means an
//! autosave can never desync from its history — the whole 0.4 design
//! paying off as a feature.
//!
//! Failure is a first-class outcome here: a quit path that cannot
//! write its record must be able to say so before the process exits,
//! which is why [`save`] reports [`SaveOutcome`] and [`SaveError`]
//! instead of a bool.

use crate::game::{Game, GameReplay};
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
/// Invalid-JSON ownership marker held at a destination until its replay
/// atomically replaces it.
const RESERVATION_MARKER_PREFIX: &str = "oxide-save-reservation:";

/// What a successful [`save`] call actually did.
#[derive(Debug)]
pub enum SaveOutcome {
    /// A record landed at this path. The quit flows only need the
    /// success; the path is the explicit-save UX's payload.
    #[allow(dead_code)]
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
        /// The underlying replay-save error.
        source: chassis::replay::ReplayError,
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

/// Writes the session to disk and rotates old ones out. Live matches
/// save as `autosave-` (what Continue resumes); finished ones save as
/// `match-` — the shelf lists both, so a completed game is always
/// watchable afterward.
pub fn save(game: &mut Game) -> Result<SaveOutcome, SaveError> {
    // The nothing-to-do gates come before directory resolution so a
    // cold quit on a machine with no data dir stays a quiet success.
    if game.state.current_tick() == 0 {
        return Ok(SaveOutcome::NothingToSave);
    }
    // A session saves once: Main Menu already wrote this match, and the
    // same game lingers as the Home backdrop — quitting from there would
    // write a colliding twin and eat a retention slot.
    if game.autosave_done {
        return Ok(SaveOutcome::AlreadySaved);
    }
    let dir = crate::paths::autosave_dir().ok_or(SaveError::NoDataDir)?;
    write_record(game, &dir)
}

/// The testable core: the gates, the name walk, the write, the
/// rotation — everything but the platform directory lookup.
fn write_record(game: &mut Game, dir: &Path) -> Result<SaveOutcome, SaveError> {
    write_record_with(game, dir, |record, path| record.save(path))
}

fn write_record_with(
    game: &mut Game,
    dir: &Path,
    save_replay: impl FnOnce(&GameReplay, &Path) -> Result<(), chassis::replay::ReplayError>,
) -> Result<SaveOutcome, SaveError> {
    if game.state.current_tick() == 0 {
        return Ok(SaveOutcome::NothingToSave);
    }
    if game.autosave_done {
        return Ok(SaveOutcome::AlreadySaved);
    }
    std::fs::create_dir_all(dir).map_err(|source| SaveError::CreateDir {
        path: dir.to_path_buf(),
        source,
    })?;
    let tick = game.state.current_tick();
    let prefix = if game.state.result().is_some() {
        "match"
    } else {
        "autosave"
    };
    game.recorder.meta.ticks = Some(tick);
    // The record says what it is: the shelf classifies on `kind` and
    // falls back to the filename prefix only for pre-0.13 files.
    game.recorder.meta.kind = Some(prefix.to_string());
    game.recorder.meta.saved_at = Some(now_unix());
    // Tick-stamped names collide across sessions (same map, same quit
    // tick); walk a counter until a free name turns up so rotation
    // always keeps the newest sessions instead of overwriting one.
    let path = free_path(dir, prefix, tick, game.scenario.seed)?
        .publish(|path| save_replay(&game.recorder, path))
        .map_err(|(path, source)| SaveError::Write { path, source })?;
    game.autosave_done = true;
    rotate(dir);
    Ok(SaveOutcome::Wrote(path))
}

/// Writes a player-named save into the saves directory, which rotation
/// never touches — a save the player asked for is deleted only by the
/// player. The name lives in the record's `description`, never in the
/// filename, so reserved names, path traversal, and length limits never
/// become a class of bug. Leaves the live recorder untouched (a later
/// quit-autosave must not inherit the name) and never marks the session
/// saved: explicit saves and quit autosaves are independent records.
pub fn save_named(game: &Game, name: &str) -> Result<PathBuf, SaveError> {
    let dir = crate::paths::saves_dir().ok_or(SaveError::NoDataDir)?;
    write_named(game, name, &dir, now_unix())
}

/// The testable core of [`save_named`]: everything but the platform
/// directory lookup and the wall clock.
fn write_named(game: &Game, name: &str, dir: &Path, saved_at: u64) -> Result<PathBuf, SaveError> {
    std::fs::create_dir_all(dir).map_err(|source| SaveError::CreateDir {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut record = game.recorder.clone();
    record.meta.ticks = Some(game.state.current_tick());
    record.meta.description = Some(name.to_string());
    record.meta.kind = Some("save".to_string());
    record.meta.saved_at = Some(saved_at);
    free_path(dir, "save", game.state.current_tick(), game.scenario.seed)?
        .publish(|path| record.save(path))
        .map_err(|(path, source)| SaveError::Write { path, source })
}

/// Owns an exclusively-created path until an atomic write replaces its
/// marker. A failed write removes the marker, while an error reported
/// after rename leaves the published replay intact.
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
/// `create_new` (O_EXCL). Every successful return owns its candidate;
/// collision suffixes have no arbitrary cutoff that can bypass the
/// exclusive create.
fn free_path(dir: &Path, prefix: &str, tick: u64, seed: u64) -> Result<PathReservation, SaveError> {
    static RESERVATION_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut n = 0u64;
    loop {
        let path = if n == 0 {
            dir.join(format!("{prefix}-{tick:010}.json"))
        } else {
            dir.join(format!("{prefix}-{tick:010}-{seed}-{n}.json"))
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
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
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

/// The newest autosave this sim version can honestly resume, if any.
/// Version-mismatched files stay on disk (archaeology) but never offer
/// themselves — replays reproduce only on the sim that wrote them.
pub fn latest_compatible() -> Option<PathBuf> {
    let dir = crate::paths::autosave_dir()?;
    let entries = std::fs::read_dir(dir).ok()?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort_by_key(|p| {
        (
            std::fs::metadata(p).and_then(|m| m.modified()).ok(),
            p.clone(),
        )
    });
    files.into_iter().rev().find(|path| {
        // Continue resumes live sessions only; `match-` records are for
        // the shelf.
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("autosave-"))
            && GameReplay::load(path)
                .map(|r| r.meta.sim_version == oxide_sim::SIM_VERSION)
                .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxide-autosave-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn outcomes_distinguish_nothing_already_and_wrote() {
        let dir = scratch("outcomes");
        let mut game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
        assert!(
            matches!(
                write_record(&mut game, &dir),
                Ok(SaveOutcome::NothingToSave)
            ),
            "an unstarted game writes nothing"
        );
        game.advance_ticks(1);
        let Ok(SaveOutcome::Wrote(path)) = write_record(&mut game, &dir) else {
            panic!("a started game writes its record");
        };
        assert!(path.exists());
        assert!(
            matches!(write_record(&mut game, &dir), Ok(SaveOutcome::AlreadySaved)),
            "one record per session"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn records_carry_their_kind_and_a_named_save_leaves_the_session_recorder_alone() {
        let dir = scratch("kind-stamp");
        let mut game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
        game.advance_ticks(1);
        let saved = write_named(&game, "before the big push", &dir, 1_784_721_600)
            .expect("a named save lands");
        let record = GameReplay::load(&saved).expect("loads back");
        assert_eq!(record.meta.kind.as_deref(), Some("save"));
        assert_eq!(
            record.meta.description.as_deref(),
            Some("before the big push")
        );
        assert_eq!(record.meta.saved_at, Some(1_784_721_600));
        assert!(
            game.recorder.meta.description.is_none(),
            "the live recorder never inherits the name"
        );
        assert!(
            !game.autosave_done,
            "an explicit save is not the session's quit record"
        );
        // A second save on the same tick walks the name instead of
        // overwriting the first.
        let again =
            write_named(&game, "again", &dir, 1_784_721_601).expect("the twin walks a counter");
        assert_ne!(saved, again);
        // The quit autosave stamps its own kind.
        let Ok(SaveOutcome::Wrote(auto_path)) = write_record(&mut game, &dir) else {
            panic!("the quit record lands");
        };
        let auto = GameReplay::load(&auto_path).expect("loads back");
        assert_eq!(auto.meta.kind.as_deref(), Some("autosave"));
        assert!(auto.meta.description.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unwritable_dir_reports_instead_of_lying() {
        // The would-be directory exists as a file, so create_dir_all
        // refuses — the class of trouble the old bool swallowed.
        let dir = scratch("unwritable");
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::write(&dir, b"in the way").unwrap();
        let mut game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
        game.advance_ticks(1);
        let err = write_record(&mut game, &dir).expect_err("the failure surfaces");
        assert!(matches!(err, SaveError::CreateDir { .. }));
        assert!(err.player_line().is_ascii(), "the menu font is Latin-1");
        assert!(!game.autosave_done, "a failed save still owes a record");
        std::fs::remove_file(&dir).ok();
    }

    #[test]
    fn a_failed_autosave_removes_only_its_uncommitted_reservation() {
        let dir = scratch("failed-reservation");
        let mut game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
        game.advance_ticks(1);

        let err = write_record_with(&mut game, &dir, |_, _| {
            Err(std::io::Error::other("write refused").into())
        })
        .expect_err("the injected write fails");
        let SaveError::Write { path, .. } = err else {
            panic!("the write failure keeps its path");
        };
        assert!(
            !path.exists(),
            "a failed write removes its reservation marker"
        );
        assert!(!game.autosave_done, "the session still owes a save");

        let err = write_record_with(&mut game, &dir, |record, path| {
            record.save(path)?;
            Err(std::io::Error::other("late durability failure").into())
        })
        .expect_err("a post-rename failure still reports");
        let SaveError::Write { path, .. } = err else {
            panic!("the late failure keeps its path");
        };
        GameReplay::load(&path).expect("a published replay is not mistaken for the marker");
        assert!(!game.autosave_done, "durability was not confirmed");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn collisions_beyond_one_thousand_remain_exclusively_reserved() {
        let dir = scratch("many-collisions");
        std::fs::create_dir_all(&dir).unwrap();
        let mut game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
        game.advance_ticks(1);
        let tick = game.state.current_tick();
        let seed = game.scenario.seed;
        for n in 0..1000 {
            let path = if n == 0 {
                dir.join(format!("save-{tick:010}.json"))
            } else {
                dir.join(format!("save-{tick:010}-{seed}-{n}.json"))
            };
            std::fs::write(path, b"occupied").expect("create known collision");
        }

        let first = write_named(&game, "first beyond the cutoff", &dir, 1)
            .expect("the first high-collision save lands");
        let second = write_named(&game, "second beyond the cutoff", &dir, 2)
            .expect("the next high-collision save lands");
        assert_ne!(first, second, "each save owns a distinct destination");
        assert_eq!(
            GameReplay::load(&first)
                .unwrap()
                .meta
                .description
                .as_deref(),
            Some("first beyond the cutoff")
        );
        assert_eq!(
            GameReplay::load(&second)
                .unwrap()
                .meta
                .description
                .as_deref(),
            Some("second beyond the cutoff")
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rotation_does_not_count_an_in_flight_reservation() {
        let dir = scratch("in-flight-rotation");
        let scenario = oxide_sim::Scenario::skirmish();
        for ticks in 1..=KEEP_AUTOSAVES {
            let mut game = Game::new(scenario.clone()).expect("game");
            game.advance_ticks(ticks as u64);
            let Ok(SaveOutcome::Wrote(path)) = write_record(&mut game, &dir) else {
                panic!("completed autosave lands");
            };
            GameReplay::load(path).expect("completed autosave loads");
        }

        let mut game = Game::new(scenario).expect("game");
        game.advance_ticks(100);
        let held = free_path(
            &dir,
            "autosave",
            game.state.current_tick(),
            game.scenario.seed,
        )
        .expect("another saver holds an in-flight destination");
        let Ok(SaveOutcome::Wrote(new_path)) = write_record(&mut game, &dir) else {
            panic!("the concurrent completed autosave lands");
        };
        assert!(new_path.exists(), "rotation keeps the new completed save");

        let completed: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| GameReplay::load(path).is_ok())
            .collect();
        assert_eq!(
            completed.len(),
            KEEP_AUTOSAVES,
            "the marker consumes no completed-record retention slot"
        );
        assert!(completed.contains(&new_path));

        drop(held);
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            KEEP_AUTOSAVES,
            "a failed concurrent saver leaves the full completed shelf"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rotation_touches_only_its_own_kind() {
        let dir = scratch("kinds");
        std::fs::create_dir_all(&dir).unwrap();
        // Six autosaves (one over budget), one match, one explicit
        // save, one fresh temp. Names sort in age order so the mtime
        // tie-break stays deterministic.
        for n in 0..6 {
            std::fs::write(dir.join(format!("autosave-000000000{n}.json")), b"{}").unwrap();
        }
        std::fs::write(dir.join("match-0000000100.json"), b"{}").unwrap();
        std::fs::write(dir.join("save-outpost.json"), b"{}").unwrap();
        std::fs::write(dir.join("autosave-0000000001.tmp.42.0"), b"live").unwrap();
        rotate(&dir);
        assert!(
            !dir.join("autosave-0000000000.json").exists(),
            "only the oldest autosave died"
        );
        for n in 1..6 {
            assert!(dir.join(format!("autosave-000000000{n}.json")).exists());
        }
        assert!(dir.join("match-0000000100.json").exists());
        assert!(
            dir.join("save-outpost.json").exists(),
            "explicit saves are never rotation's to take"
        );
        assert!(
            dir.join("autosave-0000000001.tmp.42.0").exists(),
            "a fresh temp is a write in flight, not an orphan"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn matches_rotate_against_their_own_budget() {
        let dir = scratch("matches");
        std::fs::create_dir_all(&dir).unwrap();
        for n in 0..(KEEP_MATCHES + 1) {
            std::fs::write(dir.join(format!("match-{n:010}.json")), b"{}").unwrap();
        }
        rotate(&dir);
        assert!(!dir.join(format!("match-{:010}.json", 0)).exists());
        for n in 1..(KEEP_MATCHES + 1) {
            assert!(dir.join(format!("match-{n:010}.json")).exists());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_directory_at_budget_loses_nothing() {
        let dir = scratch("at-budget");
        std::fs::create_dir_all(&dir).unwrap();
        for n in 0..KEEP_AUTOSAVES {
            std::fs::write(dir.join(format!("autosave-{n:010}.json")), b"{}").unwrap();
        }
        rotate(&dir);
        for n in 0..KEEP_AUTOSAVES {
            assert!(dir.join(format!("autosave-{n:010}.json")).exists());
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
