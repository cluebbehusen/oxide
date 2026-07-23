//! The replay shelf: everything watchable this machine has kept —
//! autosaves from the platform data dir plus anything under a local
//! `replays/` directory. Entries carry the metadata the browser shows
//! and an honest compatibility verdict: replays reproduce only on the
//! sim that wrote them, and the browser says so instead of guessing.

use crate::game::GameReplay;
use oxide_sim::SIM_VERSION;
use std::path::PathBuf;

/// One row in the replay browser.
pub struct ReplayEntry {
    /// File on disk.
    pub path: PathBuf,
    /// Browser row label: map, length, file stem.
    pub label: String,
    /// Focused-row detail line.
    pub blurb: String,
    /// Whether this sim can honestly replay it.
    pub compatible: bool,
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
        let modified = std::fs::metadata(&path)
            .and_then(|m| m.modified())
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
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("replay");
        let stem_short = if stem.len() > 26 {
            format!("{}…", &stem[..25])
        } else {
            stem.to_string()
        };
        let label = format!(
            "{} · t{} · {} · {}",
            replay.setup.name, ticks, date, stem_short
        );
        let blurb = if compatible {
            format!(
                "{} seats · sim v{} · Enter watches · X twice deletes",
                replay.setup.players.len(),
                replay.meta.sim_version
            )
        } else {
            format!(
                "recorded on sim v{} — this build speaks v{SIM_VERSION}, unwatchable",
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
            },
        ));
    }
}

/// Every known replay, newest first.
pub fn discover() -> Vec<ReplayEntry> {
    let mut found = Vec::new();
    if let Some(dir) = crate::autosave::dir() {
        scan(&dir, &mut found);
    }
    scan(std::path::Path::new("replays"), &mut found);
    found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    found.into_iter().map(|(_, e)| e).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
