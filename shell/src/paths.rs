//! One owner for every directory Oxide writes: the per-OS config and
//! data roots plus the derived subdirectories the persistence sites
//! share. Path policy lives here so no feature needs its own `#[cfg]`
//! block.

use std::path::{Path, PathBuf};

/// Platform config directory for Oxide, created on save, never on load.
pub fn config_dir() -> Option<PathBuf> {
    // An iOS app's HOME is its sandbox container, which keeps the same
    // Library layout as a Mac home.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Oxide"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Oxide"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|d| d.join("oxide"))
    }
}

/// Platform data root (autosaves, explicit saves), if resolvable.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Oxide"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Oxide"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|d| d.join("oxide"))
    }
}

/// Where autosave rotation lives; Continue resumes from here.
pub fn autosave_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("autosaves"))
}

/// Incremental recoveries and crash and freeze incident logs.
pub fn recovery_dir() -> Option<PathBuf> {
    data_dir().map(|directory| directory.join("recovery"))
}

/// Explicit, player-initiated saves. Autosave rotation never touches
/// this directory — a save the player asked for is deleted only by
/// the player.
pub fn saves_dir() -> Option<PathBuf> {
    data_dir().map(|d| d.join("saves"))
}

/// Where local replays are browsed. A packaged bundle's cwd is not a
/// usable root (typically `/`), so a bundled shell resolves against
/// the data root; a workspace run keeps the documented cwd-relative
/// `replays/`.
pub fn replays_dir() -> PathBuf {
    if bundled()
        && let Some(dir) = data_dir()
    {
        return dir.join("replays");
    }
    PathBuf::from("replays")
}

/// Whether this executable runs from a packaged bundle.
fn bundled() -> bool {
    bundle_resources().is_some()
}

/// Where a packaged bundle keeps its read-only resources, if this
/// executable runs from one: `Contents/Resources` beside
/// `Contents/MacOS/<exe>` in a macOS .app, or the flat iOS app
/// directory that holds the executable itself. Found by probing for
/// the atlas, the one file no build ships without.
pub fn bundle_resources() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundle_resources_beside(exe.parent()?)
}

fn bundle_resources_beside(exe_dir: &Path) -> Option<PathBuf> {
    [exe_dir.join("../Resources"), exe_dir.to_path_buf()]
        .into_iter()
        .find(|root| root.join("assets/sprites/atlas.png").exists())
}

#[cfg(test)]
mod tests;
