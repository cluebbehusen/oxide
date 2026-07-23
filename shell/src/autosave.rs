//! Autosaves: the recorder is always on, so quitting mid-match costs a
//! file write, and Continue is a replay load. Save-is-a-replay means an
//! autosave can never desync from its history — the whole 0.4 design
//! paying off as a feature.

use crate::game::{Game, GameReplay};
use std::path::PathBuf;

/// How many autosaves survive rotation.
const KEEP: usize = 5;

/// The autosave directory for this platform, if resolvable. Public so
/// the replay browser can shelve what lives here.
pub fn dir() -> Option<PathBuf> {
    autosave_dir()
}

fn autosave_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support/Oxide/autosaves"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Oxide/autosaves"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("oxide/autosaves"))
    }
}

/// Writes the session to disk and rotates old ones out. Live matches
/// save as `autosave-` (what Continue resumes); finished ones save as
/// `match-` — the shelf lists both, so a completed game is always
/// watchable afterward. Unstarted games save nothing.
pub fn save(game: &mut Game) -> bool {
    if game.state.current_tick() == 0 {
        return false;
    }
    let Some(dir) = autosave_dir() else {
        return false;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    game.recorder.meta.ticks = Some(game.state.current_tick());
    // Tick-stamped names collide across sessions (same map, same quit
    // tick); walk a counter until a free name turns up so rotation
    // always keeps the newest sessions instead of overwriting one.
    let tick = game.state.current_tick();
    let prefix = if game.state.result().is_some() {
        "match"
    } else {
        "autosave"
    };
    let mut path = dir.join(format!("{prefix}-{tick:010}.json"));
    let mut n = 0u32;
    while path.exists() && n < 1000 {
        n += 1;
        path = dir.join(format!(
            "{prefix}-{tick:010}-{}-{n}.json",
            game.scenario.seed
        ));
    }
    if game.recorder.save(&path).is_err() {
        return false;
    }
    rotate(&dir);
    true
}

fn rotate(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    // Newest last by modification time; ties by name for determinism.
    files.sort_by_key(|p| {
        (
            std::fs::metadata(p).and_then(|m| m.modified()).ok(),
            p.clone(),
        )
    });
    while files.len() > KEEP {
        let oldest = files.remove(0);
        std::fs::remove_file(oldest).ok();
    }
}

/// The newest autosave this sim version can honestly resume, if any.
/// Version-mismatched files stay on disk (archaeology) but never offer
/// themselves — replays reproduce only on the sim that wrote them.
pub fn latest_compatible() -> Option<PathBuf> {
    let dir = autosave_dir()?;
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
