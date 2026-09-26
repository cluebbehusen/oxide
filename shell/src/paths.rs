//! One owner for every directory Oxide writes: the per-OS config and
//! data roots plus the derived subdirectories the persistence sites
//! share. Path policy lives here so no feature grows its own `#[cfg]`
//! block again.

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

/// Incremental recoveries and optional local diagnostic sidecars.
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
mod tests {
    use super::*;

    #[test]
    fn bundle_probe_finds_mac_and_flat_ios_layouts() {
        let root = std::env::temp_dir().join(format!("oxide-bundle-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let atlas = |dir: &Path| {
            let sprites = dir.join("assets/sprites");
            std::fs::create_dir_all(&sprites).unwrap();
            std::fs::write(sprites.join("atlas.png"), b"").unwrap();
        };
        let mac = root.join("Oxide.app/Contents");
        std::fs::create_dir_all(mac.join("MacOS")).unwrap();
        atlas(&mac.join("Resources"));
        let found = bundle_resources_beside(&mac.join("MacOS")).expect("mac bundle");
        assert!(found.ends_with("MacOS/../Resources"));

        let ios = root.join("Oxide-ios.app");
        atlas(&ios);
        assert_eq!(bundle_resources_beside(&ios), Some(ios.clone()));

        let workspace = root.join("target/debug");
        std::fs::create_dir_all(&workspace).unwrap();
        assert_eq!(bundle_resources_beside(&workspace), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

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
