//! The shelf classifies resumable checkpoints and watchable recordings.
//! Unavailable revisions stay visible with a reason; malformed files are skipped.

#[cfg(test)]
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
    /// Whether this build can load or watch the record.
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
    for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
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
        // file reports the copy date, and only the metadata tells the
        // truth about when the save was made.
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
        let blurb = if let Some(problem) = replay.problem {
            format!("unavailable: {problem} | {{delete}} twice deletes")
        } else if kind.resumable() {
            let what = match kind {
                RecordKind::Save => "a saved game",
                _ => "a live session",
            };
            let action = if replay.legacy {
                "reconstructs and loads paused"
            } else {
                "loads paused"
            };
            format!("{what} | {{confirm}} {action} | {{delete}} twice deletes")
        } else {
            format!(
                "{} seats | sim v{} | {{confirm}} watches | {{delete}} twice deletes",
                replay.seats, replay.meta.sim_version
            )
        };
        out.push((
            RecordTime { saved_at, modified },
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
mod tests {
    use super::*;
    use crate::game::GameReplay;

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
    fn shelf_skips_oversized_records_without_hiding_valid_neighbors() {
        let dir = std::env::temp_dir().join(format!(
            "oxide-shelf-bounds-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let valid: GameReplay =
            chassis::replay::Replay::new(SIM_VERSION, oxide_sim::Scenario::skirmish());
        valid.save(dir.join("valid.json")).unwrap();
        std::fs::File::create(dir.join("oversized.json"))
            .unwrap()
            .set_len(chassis::replay::MAX_REPLAY_BYTES as u64 + 1)
            .unwrap();

        let mut out = Vec::new();
        scan(&dir, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1.path.file_stem().unwrap(), "valid");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn catalog_orders_autosaves_by_saved_time_then_mtime_then_path() {
        let dir = std::env::temp_dir().join(format!("oxide-save-order-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let game = crate::game::Game::new(oxide_sim::Scenario::skirmish()).unwrap();
        for (name, saved_at, modified) in [
            ("copied", 99, 900),
            ("older", 100, 100),
            ("a", 100, 101),
            ("b", 100, 101),
            ("newest", 101, 1),
        ] {
            let mut meta = game.recorder.meta.clone();
            meta.kind = Some("autosave".into());
            meta.ticks = Some(game.state.current_tick());
            meta.saved_at = Some(saved_at);
            let path = dir.join(format!("{name}.oxsave"));
            crate::saved_game::write_capture(game.capture_save(), meta, &path).unwrap();
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(modified))
                .unwrap();
        }
        for reverse in [false, true] {
            let mut found = Vec::new();
            scan(&dir, &mut found);
            if reverse {
                found.reverse();
            }
            let candidates = newest_first(found)
                .into_iter()
                .filter(|entry| entry.compatible && entry.kind == RecordKind::Autosave)
                .map(|entry| {
                    entry
                        .path
                        .file_stem()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>();
            assert_eq!(candidates, ["newest", "b", "a", "older", "copied"]);
        }
        std::fs::remove_dir_all(dir).unwrap();
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
        assert_eq!(elide(&long_ascii), format!("{}...", "a".repeat(23)));
        // 27 chars, with byte offset 23 landing inside the first é —
        // the byte-sliced version panicked exactly here.
        let multibyte = format!("{}ééééé", "a".repeat(22));
        assert_eq!(elide(&multibyte), format!("{}é...", "a".repeat(22)));
    }

    #[test]
    fn scan_uses_record_metadata_without_trusting_malformed_neighbors() {
        let dir = std::env::temp_dir().join(format!(
            "oxide-shelf-metadata-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp directory");
        let scenario = oxide_sim::Scenario::skirmish();
        let mut save: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario);
        save.meta.kind = Some("save".to_string());
        save.meta.description = Some("before the push".to_string());
        save.meta.sim_version = "0.0.1".to_string();
        save.meta.saved_at = Some(u64::MAX);
        save.record(
            42,
            oxide_sim::PlayerCommand {
                player: oxide_sim::PlayerId(0),
                command: oxide_sim::Command::Surrender,
            },
        );
        save.save(dir.join("neutral.json")).expect("save record");
        std::fs::write(dir.join("broken.json"), b"not json").expect("bad neighbor");
        std::fs::write(dir.join("ignored.txt"), b"not a replay").expect("other extension");

        let mut out = Vec::new();
        scan(&dir, &mut out);
        assert_eq!(out.len(), 1, "bad neighbors do not hide the good record");
        let entry = &out[0].1;
        assert_eq!(entry.kind, RecordKind::Save, "the metadata tag wins");
        assert!(!entry.compatible);
        assert!(entry.label.starts_with("before the push |"));
        assert!(
            entry.label.contains("| t43 |"),
            "duration comes from the command tail"
        );
        assert!(
            entry.blurb.contains("unavailable"),
            "saves are loaded, not watched"
        );
        assert_ne!(
            out[0].0.saved_at,
            std::time::UNIX_EPOCH,
            "overflowing saved_at falls back to mtime"
        );

        std::fs::remove_dir_all(dir).ok();
    }
}
