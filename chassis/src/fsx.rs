//! Atomic, durable file writes for persisted records.
//!
//! A crash mid-write must never publish a truncated file, a failed write
//! must never leave a temp behind, and a rewrite must replace the previous
//! record on every platform (std's `rename` replaces existing destinations
//! on Windows too, via `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`).
//! [`write_atomic`] owns that contract; [`sweep_temps`] reaps temps orphaned
//! by a crash, which skips the cleanup guard.

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
    // Two sessions, or two threads of one, saving the same stem
    // concurrently must not clobber each other's temp.
    static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    struct TempGuard<'a>(Option<&'a Path>);
    impl Drop for TempGuard<'_> {
        fn drop(&mut self) {
            if let Some(path) = self.0 {
                std::fs::remove_file(path).ok();
            }
        }
    }
    let path = path.as_ref();
    let parent = parent_dir(path);
    std::fs::create_dir_all(parent)?;
    let nonce = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp.{}.{nonce}", std::process::id()));
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
    // The rename lives in the directory; without syncing it a power loss
    // can roll the swap back to the intact but stale previous record. A
    // sync failure propagates so a caller never reports a durable save
    // over a rename the disk never committed. Unix-only: Windows cannot
    // open a directory for fsync.
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
    for path in entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
    {
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
mod tests;
