//! The shelf classifies resumable checkpoints and watchable recordings.
//! Unavailable revisions stay visible with a reason; malformed files are skipped.

use crate::numeric::Fit;
#[cfg(test)]
use oxide_sim::SIM_VERSION;
use std::path::PathBuf;

/// What a record on disk is, read from its metadata `kind` tag with a
/// filename-prefix fallback for records that carry no tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    /// A live session written on quit; Continue's material.
    Autosave,
    /// A player-named explicit save.
    Save,
    /// A finished match, kept to watch.
    Match,
}

impl RecordKind {
    /// Whether the shelf's verb for this record is Load. Autosaves and
    /// saves are live sessions: watching one fog-free mid-match would
    /// scout the enemy, so they resume instead.
    pub fn resumable(self) -> bool {
        matches!(self, RecordKind::Autosave | RecordKind::Save)
    }
}

pub(crate) fn record_kind(
    meta: &chassis::replay::ReplayMeta,
    path: &std::path::Path,
) -> RecordKind {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    match meta.kind.as_deref() {
        Some("autosave") => RecordKind::Autosave,
        Some("save") => RecordKind::Save,
        Some("match") => RecordKind::Match,
        _ if stem.starts_with("autosave-") => RecordKind::Autosave,
        _ if stem.starts_with("save-") => RecordKind::Save,
        _ => RecordKind::Match,
    }
}

/// One row in the replay browser.
pub struct ReplayEntry {
    /// File on disk.
    pub path: PathBuf,
    /// Browser row label: name (or map), length, date.
    pub label: String,
    /// Focused-row detail line.
    pub blurb: String,
    /// How to act on the focused row: coaching, shown when stuck.
    pub hint: String,
    /// Whether this build can load or watch the record.
    pub compatible: bool,
    /// What the record is; decides its shelf section and verb.
    pub kind: RecordKind,
}

/// How to act on a shelf row: activate it, where it can be activated,
/// and delete it. Deleting takes a key, so a touch-only build leaves the
/// delete clause out.
fn entry_hint(action: Option<(&str, &str)>, touch_only: bool) -> String {
    match (action, touch_only) {
        (Some((_, touch_action)), true) => format!("tap to {touch_action}"),
        (None, true) => String::new(),
        (Some((action, _)), false) => format!("{{confirm}} {action} | {{delete}} twice deletes"),
        (None, false) => "{delete} twice deletes".to_string(),
    }
}

/// Days-since-epoch to a civil date (Howard Hinnant's algorithm) —
/// enough calendar for a browser row without pulling a time crate.
fn civil_date(secs: u64) -> String {
    let days = (secs / 86_400).fit::<i64>();
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Shortens a long file stem for the browser row. Counts chars, not
/// bytes: replay files are user-named, and slicing bytes could split a
/// multibyte character and panic.
fn elide(stem: &str) -> String {
    if stem.chars().count() > 26 {
        let head: String = stem.chars().take(23).collect();
        format!("{head}...")
    } else {
        stem.to_string()
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct RecordTime {
    saved_at: std::time::SystemTime,
    modified: std::time::SystemTime,
}

#[cfg(test)]
fn scan(dir: &std::path::Path, out: &mut Vec<(RecordTime, ReplayEntry)>) {
    scan_cancellable(dir, out, &|| false);
}

fn scan_cancellable(
    dir: &std::path::Path,
    out: &mut Vec<(RecordTime, ReplayEntry)>,
    cancelled: &impl Fn() -> bool,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
    {
        if cancelled() {
            break;
        }
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("json" | "oxsave")
        ) {
            continue;
        }
        let Ok(replay) = crate::saved_game::inspect(&path) else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("replay");
        let kind = record_kind(&replay.meta, &path);
        // A record's own saved_at outranks mtime: a copied or synced
        // file reports the copy date, and only the metadata records when
        // the save was made.
        let modified = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        let saved_at = replay
            .meta
            .saved_at
            // checked: a copied or hand-edited record can carry any
            // u64, and an out-of-range timestamp must fall back to
            // mtime, not panic the whole shelf scan.
            .and_then(|secs| {
                std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs))
            })
            .unwrap_or(modified);
        let date = saved_at
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| civil_date(d.as_secs()))
            .unwrap_or_default();
        let ticks = replay.meta.ticks.unwrap_or(0);
        let compatible = replay.problem.is_none();
        // A named save leads with its name; everything else leads with
        // its map and keeps the file stem for identification.
        let label = match replay.meta.description.as_deref() {
            Some(name) => format!("{} | {} | t{} | {}", elide(name), replay.map, ticks, date),
            None => format!("{} | t{} | {} | {}", replay.map, ticks, date, elide(stem)),
        };
        let touch_only = crate::platform::TOUCH_ONLY;
        let (blurb, action) = if let Some(problem) = replay.problem {
            (format!("unavailable: {problem}"), None)
        } else if kind.resumable() {
            let what = match kind {
                RecordKind::Save => "a saved game",
                _ => "a live session",
            };
            let action = if replay.legacy {
                (
                    "reconstructs and loads paused",
                    "reconstruct and load paused",
                )
            } else {
                ("loads paused", "load paused")
            };
            (what.to_string(), Some(action))
        } else {
            (
                format!("{} seats | sim v{}", replay.seats, replay.meta.sim_version),
                Some(("watches", "watch")),
            )
        };
        let hint = entry_hint(action, touch_only);
        out.push((
            RecordTime { saved_at, modified },
            ReplayEntry {
                path,
                label,
                blurb,
                hint,
                compatible,
                kind,
            },
        ));
    }
}

/// Every known replay, newest first.
pub fn discover(cancelled: impl Fn() -> bool) -> Vec<ReplayEntry> {
    let mut found = Vec::new();
    if let Some(dir) = crate::paths::autosave_dir() {
        scan_cancellable(&dir, &mut found, &cancelled);
    }
    if let Some(dir) = crate::paths::saves_dir() {
        scan_cancellable(&dir, &mut found, &cancelled);
    }
    scan_cancellable(&crate::paths::replays_dir(), &mut found, &cancelled);
    newest_first(found)
}

fn newest_first(mut found: Vec<(RecordTime, ReplayEntry)>) -> Vec<ReplayEntry> {
    found.sort_by(|(left_time, left), (right_time, right)| {
        (right_time, &right.path).cmp(&(left_time, &left.path))
    });
    found.into_iter().map(|(_, entry)| entry).collect()
}

#[cfg(test)]
mod tests;
