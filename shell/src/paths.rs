//! One owner for every directory Oxide writes: the per-OS config and
//! data roots plus the derived subdirectories the persistence sites
//! share. Path policy lives here so no feature grows its own `#[cfg]`
//! block again.

use std::path::PathBuf;

/// Platform config directory for Oxide, created on save, never on load.
pub fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Oxide"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Oxide"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|d| d.join("oxide"))
    }
}

/// Platform data root (autosaves, explicit saves), if resolvable.
pub fn data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Oxide"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Oxide"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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

/// Whether this executable runs from a packaged bundle — the writable
/// half of the probe `assets::resource_root` uses for resources.
fn bundled() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|dir| dir.join("../Resources/assets/sprites/atlas.png").exists())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_dirs_hang_off_the_one_data_root() {
        if let Some(data) = data_dir() {
            assert_eq!(autosave_dir(), Some(data.join("autosaves")));
            assert_eq!(saves_dir(), Some(data.join("saves")));
        }
    }

    #[test]
    fn a_workspace_run_keeps_the_cwd_relative_replays_dir() {
        // Test binaries never sit inside a bundle, so every documented
        // `driver` invocation keeps reading the local replays/ dir.
        assert_eq!(replays_dir(), PathBuf::from("replays"));
    }
}
