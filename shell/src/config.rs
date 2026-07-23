//! Persisted presentation config: bindings, volumes, UI scale, camera
//! feel, window size.
//!
//! Strictly cosmetic state — nothing here may affect game outcomes, so
//! it versions independently of replays and loses nothing when it
//! resets. Any read problem (missing file, old version, parse error)
//! falls back to defaults silently: a bad config file must never keep
//! the game from starting.

use crate::action::BindingMap;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bumped when the shape changes incompatibly; mismatches reset to
/// defaults rather than guessing.
const CONFIG_VERSION: u32 = 1;

/// Mixer bus volumes, 0..=1, applied multiplicatively with each clip's
/// authored level.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Volumes {
    /// Everything.
    pub master: f32,
    /// Battle and world clips.
    pub effects: f32,
    /// Chrome sounds.
    pub ui: f32,
    /// Music and ambient beds.
    pub music: f32,
}

impl Default for Volumes {
    fn default() -> Self {
        Self {
            master: 1.0,
            effects: 1.0,
            ui: 1.0,
            music: 1.0,
        }
    }
}

/// Camera feel knobs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CameraPrefs {
    /// Keyboard pan speed multiplier.
    pub pan_speed: f32,
    /// Whether the pointer at a window edge pans.
    pub edge_pan: bool,
    /// Flip wheel-zoom direction.
    pub zoom_inverted: bool,
}

impl Default for CameraPrefs {
    fn default() -> Self {
        Self {
            pan_speed: 1.0,
            edge_pan: false,
            zoom_inverted: false,
        }
    }
}

/// The whole persisted surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Shape version; mismatch resets to defaults.
    pub version: u32,
    /// The active binding profile.
    pub bindings: BindingMap,
    /// Bus volumes.
    pub volumes: Volumes,
    /// User UI scale factor, multiplied with DPI exactly once by the
    /// layout model.
    pub ui_scale: f32,
    /// Camera feel.
    pub camera: CameraPrefs,
    /// Window size at startup, WIDTHxHEIGHT.
    pub window: (u32, u32),
    /// Accessibility: damp decorative animation (alert pulses, ping
    /// rings, muzzle flashes). Informational motion — unit movement,
    /// shell arcs — always stays.
    #[serde(default)]
    pub reduced_motion: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            bindings: BindingMap::classic(),
            volumes: Volumes::default(),
            ui_scale: 1.0,
            camera: CameraPrefs::default(),
            window: (1280, 800),
            reduced_motion: false,
        }
    }
}

/// Platform config directory for Oxide, created on save, never on load.
fn config_dir() -> Option<PathBuf> {
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

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

impl Config {
    /// Loads the persisted config, or defaults on any trouble at all.
    pub fn load() -> Self {
        Self::load_from(config_path())
    }

    /// Clamps a persisted window size into the envelope the CLI
    /// enforces — a hand-edited config must not hand the native
    /// backend an i32-overflowing dimension.
    fn sane_window(window: (u32, u32)) -> (u32, u32) {
        (window.0.clamp(640, 16_384), window.1.clamp(400, 16_384))
    }

    fn load_from(path: Option<PathBuf>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(mut config) if config.version == CONFIG_VERSION => {
                // A hand-edited config with no bindings parses fine and
                // would strip every shortcut — restore the profile
                // rather than ship a keyboardless game. The same file
                // can carry a syntactically valid action whose payload
                // indexes past its array (SetBookmark(4) on a
                // four-slot rack): out-of-range payloads reset the
                // whole profile like any other malformed input.
                let payload_sane = config.bindings.bindings().iter().all(|b| match b.action {
                    crate::action::Action::SetBookmark(i)
                    | crate::action::Action::RecallBookmark(i) => i < 4,
                    crate::action::Action::TrainSlot(i)
                    | crate::action::Action::Slot(i)
                    | crate::action::Action::AssignGroup(i) => i < 9,
                    _ => true,
                });
                if config.bindings.bindings().is_empty() || !payload_sane {
                    config.bindings = BindingMap::classic();
                }
                config.window = Self::sane_window(config.window);
                config
            }
            _ => Self::default(),
        }
    }

    /// Persists atomically (temp + rename), creating the directory.
    // Wired to the Phase D settings screens; tested directly until then.
    #[allow(dead_code)]
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = config_path() else {
            return Ok(()); // headless CI without HOME: nothing to do
        };
        self.save_to(&path)
    }

    fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_string_pretty(self).expect("config serializes"),
        )?;
        // Windows refuses to rename onto an existing file; removing the
        // old config first costs atomicity only in the crash window
        // between the two calls, and the loader falls back to defaults
        // on any unreadable file.
        match std::fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(_) => {
                std::fs::remove_file(path).ok();
                std::fs::rename(&tmp, path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_twice_replaces_instead_of_failing() {
        // Windows refuses rename-onto-existing; the fallback must make
        // the second save land, and its content must win.
        let dir = std::env::temp_dir().join(format!("oxide-config-twice-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config::default();
        config.save_to(&path).expect("first save");
        config.ui_scale = 1.5;
        config.save_to(&path).expect("second save replaces");
        let loaded = Config::load_from(Some(path.clone()));
        assert!((loaded.ui_scale - 1.5).abs() < 1e-6, "the newer config won");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_config_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("oxide-config-test-{}", std::process::id()));
        let path = dir.join("config.json");
        let config = Config {
            ui_scale: 1.25,
            volumes: Volumes {
                master: 0.5,
                ..Volumes::default()
            },
            ..Config::default()
        };
        config.save_to(&path).unwrap();
        let back = Config::load_from(Some(path.clone()));
        assert_eq!(back, config);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn any_trouble_at_all_falls_back_to_defaults() {
        // Missing file.
        let missing = Config::load_from(Some(PathBuf::from("/definitely/not/here.json")));
        assert_eq!(missing, Config::default());
        // Garbage content.
        let dir = std::env::temp_dir().join(format!("oxide-config-garbage-{}", std::process::id()));
        let path = dir.join("config.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "not json at all").unwrap();
        assert_eq!(Config::load_from(Some(path.clone())), Config::default());
        // A future version resets rather than guessing.
        let future = Config {
            version: CONFIG_VERSION + 1,
            ..Config::default()
        };
        std::fs::write(&path, serde_json::to_string(&future).unwrap()).unwrap();
        assert_eq!(Config::load_from(Some(path.clone())), Config::default());
        std::fs::remove_dir_all(&dir).ok();
    }
}
