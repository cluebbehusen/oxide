//! Atomic, durable file writes — the one way anything lands on disk.
//!
//! Every persistence site shares the same failure modes: a crash
//! mid-write must never publish a truncated file, a failed write must
//! never leave a temp behind, and a rewrite must replace the previous
//! record on every platform (std's `rename` replaces existing
//! destinations on Windows too — `MoveFileExW` with
//! `MOVEFILE_REPLACE_EXISTING`). [`write_atomic`] owns that contract
//! once; [`sweep_temps`] reaps orphans left by crashes predating the
//! guard.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Writes `path` atomically and durably: parent directories are
/// created, the payload goes to a uniquely named sibling temp, is
/// flushed and fsynced, and only then renamed over the destination.
/// The temp is removed on every error path, so a failed save leaves
/// nothing behind.
///
/// The closure's error type only needs a `From<io::Error>` conversion,
/// so serialization errors keep their own classification instead of
/// being flattened into IO.
pub fn write_atomic<E, F>(path: impl AsRef<Path>, write: F) -> Result<(), E>
where
    E: From<std::io::Error>,
    F: FnOnce(&mut dyn Write) -> Result<(), E>,
{
    let path = path.as_ref();
    let parent = parent_dir(path);
    std::fs::create_dir_all(parent)?;
    // Unique temp name: two sessions (or two threads of one) saving
    // the same stem concurrently must not clobber each other.
    static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp.{}.{nonce}", std::process::id()));
    struct TempGuard<'a>(Option<&'a Path>);
    impl Drop for TempGuard<'_> {
        fn drop(&mut self) {
            if let Some(path) = self.0 {
                std::fs::remove_file(path).ok();
            }
        }
    }
    let mut guard = TempGuard(Some(&tmp));
    let file = std::fs::File::create(&tmp)?;
    let mut writer = std::io::BufWriter::new(file);
    write(&mut writer)?;
    writer.flush()?;
    // Flush only reaches the OS page cache; without the fsync a power
    // loss after the rename can still publish a truncated file.
    writer.get_ref().sync_all()?;
    drop(writer);
    std::fs::rename(&tmp, path)?;
    guard.0 = None;
    // The rename itself lives in the directory; without syncing it a
    // power loss can roll the swap back to the OLD record (intact —
    // never truncated — but stale). The failure propagates: a
    // directory we just wrote into refusing to sync is a signal, and
    // swallowing it would let a caller report durably-saved over a
    // rename the disk never committed. Unix-only: Windows cannot open
    // a directory for fsync, and there the rename's durability rides
    // the OS.
    #[cfg(unix)]
    std::fs::File::open(parent).and_then(|d| d.sync_all())?;
    Ok(())
}

/// Removes orphaned `*.tmp.*` siblings (the naming [`write_atomic`]
/// uses) older than `older_than` from `dir`, returning how many were
/// reaped. The age threshold keeps a write in flight safe from a
/// concurrent sweep.
pub fn sweep_temps(dir: &Path, older_than: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut swept = 0;
    for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
        let temp_named = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains(".tmp."));
        if !temp_named || !path.is_file() {
            continue;
        }
        let orphaned = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age >= older_than);
        if orphaned && std::fs::remove_file(&path).is_ok() {
            swept += 1;
        }
    }
    swept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("chassis-fsx-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temps_in(dir: &Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(".tmp."))
            })
            .collect()
    }

    #[test]
    fn a_bare_path_uses_the_current_directory_as_its_parent() {
        assert_eq!(parent_dir(Path::new("session.json")), Path::new("."));
    }

    #[test]
    fn writing_twice_to_one_path_keeps_the_second_payload() {
        // The cross-platform replace contract, pinned on every CI OS:
        // a rewrite lands and its content wins.
        let dir = scratch("twice");
        let path = dir.join("record.json");
        write_atomic::<std::io::Error, _>(&path, |w| w.write_all(b"first")).unwrap();
        write_atomic::<std::io::Error, _>(&path, |w| w.write_all(b"second")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert!(temps_in(&dir).is_empty(), "no temp survives a success");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_write_leaves_no_temp_behind() {
        // The closure fails mid-write: the guard must reap the temp.
        let dir = scratch("closure-err");
        let path = dir.join("record.json");
        let result = write_atomic::<std::io::Error, _>(&path, |w| {
            w.write_all(b"partial")?;
            Err(std::io::Error::other("serialization refused"))
        });
        assert!(result.is_err());
        assert!(!path.exists(), "nothing was published");
        assert!(temps_in(&dir).is_empty(), "the temp was removed");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_rewrite_preserves_the_previous_record() {
        let dir = scratch("preserve-on-error");
        let path = dir.join("record.json");
        std::fs::write(&path, b"complete old record").unwrap();

        let result = write_atomic::<std::io::Error, _>(&path, |writer| {
            writer.write_all(b"truncated replacement")?;
            Err(std::io::Error::other("serialization refused"))
        });

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"complete old record",
            "a failed save must not damage the last durable record"
        );
        assert!(temps_in(&dir).is_empty(), "the failed temp was removed");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_rename_leaves_no_temp_behind() {
        // The destination exists as a directory, so the final rename
        // fails after the temp was fully written.
        let dir = scratch("rename-err");
        let path = dir.join("record.json");
        std::fs::create_dir_all(&path).unwrap();
        let result = write_atomic::<std::io::Error, _>(&path, |w| w.write_all(b"payload"));
        assert!(result.is_err());
        assert!(temps_in(&dir).is_empty(), "the temp was removed");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_sweep_reaps_only_old_temps() {
        let dir = scratch("sweep");
        std::fs::write(dir.join("orphan.tmp.999.0"), b"stale").unwrap();
        std::fs::write(dir.join("keeper.json"), b"real").unwrap();
        // Zero threshold: everything temp-named is old enough.
        assert_eq!(sweep_temps(&dir, Duration::ZERO), 1);
        assert!(dir.join("keeper.json").exists(), "real files never swept");
        // A fresh temp under a real threshold is a write in flight.
        std::fs::write(dir.join("inflight.tmp.999.1"), b"live").unwrap();
        assert_eq!(sweep_temps(&dir, Duration::from_secs(3600)), 0);
        assert!(dir.join("inflight.tmp.999.1").exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
