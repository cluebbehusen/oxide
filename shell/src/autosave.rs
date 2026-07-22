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

/// Writes the session as an autosave and rotates old ones out. A live,
/// undecided match only — menus over a finished or unstarted game save
/// nothing. Returns whether a file landed.
pub fn save(game: &mut Game) -> bool {
    if game.state.current_tick() == 0 || game.state.result().is_some() {
        return false;
    }
    let Some(dir) = autosave_dir() else {
        return false;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    game.recorder.meta.ticks = Some(game.state.current_tick());
    let path = dir.join(format!("autosave-{:010}.json", game.state.current_tick()));
    // Tick-stamped names collide across sessions; uniquify with the
    // scenario seed so two matches at the same tick both survive.
    let path = if path.exists() {
        dir.join(format!(
            "autosave-{:010}-{}.json",
            game.state.current_tick(),
            game.scenario.seed
        ))
    } else {
        path
    };
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
        GameReplay::load(path)
            .map(|r| r.meta.sim_version == oxide_sim::SIM_VERSION)
            .unwrap_or(false)
    })
}
