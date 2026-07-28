//! The replay shelf: everything watchable this machine has kept —
//! autosaves and explicit saves from the platform data dir plus
//! anything under the replays dir (cwd-relative in a workspace run,
//! data-rooted when bundled). Entries carry the metadata the browser shows
//! and an honest compatibility verdict: replays reproduce only on the
//! sim that wrote them, and the browser says so instead of guessing.

use crate::game::GameReplay;
use oxide_sim::SIM_VERSION;
use std::path::PathBuf;

/// What a record on disk is, read from its metadata `kind` tag with a
/// filename-prefix fallback for pre-0.13 files (which carried the rule
/// in their names).
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
    /// saves are LIVE sessions: watching one fog-free mid-match would
    /// scout the enemy, so they resume instead.
    pub fn resumable(self) -> bool {
        matches!(self, RecordKind::Autosave | RecordKind::Save)
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
    /// Whether this sim can honestly replay it.
    pub compatible: bool,
    /// What the record is; decides its shelf section and verb.
    pub kind: RecordKind,
}

/// Days-since-epoch to a civil date (Howard Hinnant's algorithm) —
/// enough calendar for a browser row without pulling a time crate.
fn civil_date(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
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
/// bytes — replay files are user-named, and a byte slice once panicked
/// mid-multibyte-character and took the whole shelf down with it.
fn elide(stem: &str) -> String {
    if stem.chars().count() > 26 {
        let head: String = stem.chars().take(25).collect();
        format!("{head}…")
    } else {
        stem.to_string()
    }
}

fn scan(dir: &std::path::Path, out: &mut Vec<(std::time::SystemTime, ReplayEntry)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(replay) = GameReplay::load(&path) else {
            continue;
        };
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("replay");
        // The kind tag is the rule since 0.13; the filename prefix is
        // the fallback that keeps 0.12-era records classified (their
        // information rule lived in their names).
        let kind = match replay.meta.kind.as_deref() {
            Some("autosave") => RecordKind::Autosave,
            Some("save") => RecordKind::Save,
            Some("match") => RecordKind::Match,
            _ if stem.starts_with("autosave-") => RecordKind::Autosave,
            _ if stem.starts_with("save-") => RecordKind::Save,
            _ => RecordKind::Match,
        };
        // A record's own saved_at outranks mtime: a copied or synced
        // file reports the copy date, and only the metadata tells the
        // truth about when the save was made.
        let modified = replay
            .meta
            .saved_at
            .map(|secs| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .or_else(|| std::fs::metadata(&path).and_then(|m| m.modified()).ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        let date = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| civil_date(d.as_secs()))
            .unwrap_or_default();
        let ticks = replay.meta.ticks.unwrap_or_else(|| {
            replay
                .commands
                .last()
                .map_or(0, |c| c.tick.saturating_add(1))
        });
        let compatible = replay.meta.sim_version == SIM_VERSION;
        // A named save leads with its name; everything else leads with
        // its map and keeps the file stem for identification.
        let label = match replay.meta.description.as_deref() {
            Some(name) => format!(
                "{} · {} · t{} · {}",
                elide(name),
                replay.setup.name,
                ticks,
                date
            ),
            None => format!(
                "{} · t{} · {} · {}",
                replay.setup.name,
                ticks,
                date,
                elide(stem)
            ),
        };
        let blurb = if !compatible {
            let verb = if kind.resumable() {
                "unloadable"
            } else {
                "unwatchable"
            };
            format!(
                "recorded on sim v{}; this build runs v{SIM_VERSION}, {verb}",
                replay.meta.sim_version
            )
        } else if kind.resumable() {
            let what = match kind {
                RecordKind::Save => "a saved game",
                _ => "a live session",
            };
            format!("{what} · Enter loads · X twice deletes")
        } else {
            format!(
                "{} seats · sim v{} · Enter watches · X twice deletes",
                replay.setup.players.len(),
                replay.meta.sim_version
            )
        };
        out.push((
            modified,
            ReplayEntry {
                path,
                label,
                blurb,
                compatible,
                kind,
            },
        ));
    }
}

/// Every known replay, newest first.
pub fn discover() -> Vec<ReplayEntry> {
    let mut found = Vec::new();
    if let Some(dir) = crate::paths::autosave_dir() {
        scan(&dir, &mut found);
    }
    if let Some(dir) = crate::paths::saves_dir() {
        scan(&dir, &mut found);
    }
    scan(&crate::paths::replays_dir(), &mut found);
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found.into_iter().map(|(_, e)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shelf_badge_compares_versions_and_never_guesses() {
        let dir = std::env::temp_dir().join(format!(
            "oxide-shelf-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let scenario = oxide_sim::Scenario::skirmish();
        let ours: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario.clone());
        ours.save(dir.join("ours.json")).unwrap();
        let mut foreign: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario);
        foreign.meta.sim_version = "0.0.1".to_string();
        foreign.save(dir.join("foreign.json")).unwrap();

        let mut out = Vec::new();
        scan(&dir, &mut out);
        let entry = |name: &str| {
            out.iter()
                .map(|(_, e)| e)
                .find(|e| e.path.file_stem().unwrap() == name)
                .expect("scanned")
        };
        assert!(entry("ours").compatible, "our own version wears the badge");
        let foreign = entry("foreign");
        assert!(!foreign.compatible, "a foreign version never does");
        assert!(
            foreign.blurb.contains("0.0.1") && foreign.blurb.contains(SIM_VERSION),
            "the honest badge names both versions: {}",
            foreign.blurb
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn kinds_read_the_metadata_tag_and_fall_back_to_the_0_12_filename_prefix() {
        let dir = std::env::temp_dir().join(format!(
            "oxide-kinds-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let scenario = oxide_sim::Scenario::skirmish();
        // A tagged save under a neutral filename: the tag wins.
        let mut named: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario.clone());
        named.meta.kind = Some("save".to_string());
        named.meta.description = Some("before the push".to_string());
        named.save(dir.join("anything.json")).unwrap();
        // 0.12-era records carry no tag; their names carry the rule.
        let old: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario.clone());
        old.save(dir.join("autosave-0000000042.json")).unwrap();
        let finished: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario);
        finished.save(dir.join("match-0000000099.json")).unwrap();

        let mut out = Vec::new();
        scan(&dir, &mut out);
        let entry = |name: &str| {
            out.iter()
                .map(|(_, e)| e)
                .find(|e| e.path.file_stem().unwrap() == name)
                .expect("scanned")
        };
        assert_eq!(entry("anything").kind, RecordKind::Save);
        assert!(
            entry("anything").label.starts_with("before the push"),
            "a named save leads with its name: {}",
            entry("anything").label
        );
        assert_eq!(entry("autosave-0000000042").kind, RecordKind::Autosave);
        assert_eq!(entry("match-0000000099").kind, RecordKind::Match);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_calendar_is_honest_without_a_time_crate() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(86_399), "1970-01-01", "last second of day one");
        assert_eq!(civil_date(86_400), "1970-01-02");
        // Leap handling around the century rule: 2000-02-29 existed.
        // 951_782_400 = 2000-02-29T00:00:00Z; 951_868_800 = 2000-03-01.
        assert_eq!(civil_date(951_782_400), "2000-02-29");
        assert_eq!(civil_date(951_868_800), "2000-03-01");
        // A modern spot check: 2026-07-22T12:00:00Z.
        assert_eq!(civil_date(1_784_721_600), "2026-07-22");
    }

    #[test]
    fn long_stems_elide_at_char_boundaries() {
        assert_eq!(elide("short"), "short");
        let long_ascii = "a".repeat(30);
        assert_eq!(elide(&long_ascii), format!("{}…", "a".repeat(25)));
        // 27 chars, with byte offset 25 landing inside the first é —
        // the byte-sliced version panicked exactly here.
        let multibyte = format!("{}ééé", "a".repeat(24));
        assert_eq!(elide(&multibyte), format!("{}é…", "a".repeat(24)));
    }
}
