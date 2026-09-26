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

/// Optional player-facing frame timing, independent of the debug overlay.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceDisplay {
    /// No collection or display.
    #[default]
    Off,
    /// Smoothed frames per second.
    Fps,
    /// Frame intervals, CPU work, and recent hitch history.
    Detailed,
}

impl PerformanceDisplay {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Fps => "FPS",
            Self::Detailed => "Detailed",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Fps,
            Self::Fps => Self::Detailed,
            Self::Detailed => Self::Off,
        }
    }
}

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
    #[serde(default = "default_volume")]
    pub music: f32,
}

fn default_volume() -> f32 {
    1.0
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

/// Touch gesture timing windows, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TouchPrefs {
    /// Two taps inside this window read as a double-tap.
    pub double_tap_ms: u32,
    /// A still finger held this long fires the context gesture.
    pub long_press_ms: u32,
}

impl Default for TouchPrefs {
    fn default() -> Self {
        Self {
            double_tap_ms: 300,
            long_press_ms: 350,
        }
    }
}

impl TouchPrefs {
    /// The invariant a hand-edited config cannot break: a lazy
    /// double-tap must never read as a long-press, so the press window
    /// always sits strictly above the tap window.
    pub fn clamped(self) -> Self {
        Self {
            double_tap_ms: self.double_tap_ms.clamp(50, 1000),
            long_press_ms: self
                .long_press_ms
                .clamp(50, 2000)
                .max(self.double_tap_ms.clamp(50, 1000) + 50),
        }
    }
}

/// Strategic marker transition and size in logical screen pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MarkerPrefs {
    pub start: f32,
    pub end: f32,
    pub scale: f32,
}

impl Default for MarkerPrefs {
    fn default() -> Self {
        Self {
            start: 24.0,
            end: 16.0,
            scale: 1.0,
        }
    }
}

impl MarkerPrefs {
    pub fn clamped(self) -> Self {
        let finite = |value: f32, default: f32, min: f32, max: f32| {
            if value.is_finite() {
                value.clamp(min, max)
            } else {
                default
            }
        };
        let end = finite(self.end, 16.0, 8.0, 30.0);
        Self {
            start: finite(self.start, 24.0, end + 1.0, 40.0).max(end + 1.0),
            end,
            scale: finite(self.scale, 1.0, 0.75, 1.5),
        }
    }

    pub fn timing_label(self) -> &'static str {
        match (self.start, self.end) {
            (24.0, 16.0) => "Standard",
            (30.0, 22.0) => "Earlier",
            (18.0, 10.0) => "Later",
            _ => "Custom",
        }
    }

    pub fn cycle_timing(&mut self) {
        (self.start, self.end) = match (self.start, self.end) {
            (24.0, 16.0) => (30.0, 22.0),
            (30.0, 22.0) => (18.0, 10.0),
            _ => (24.0, 16.0),
        };
    }
}

/// The whole persisted surface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Optional local detailed diagnostics; recovery recording is independent.
    #[serde(default)]
    pub diagnostics: bool,
    #[serde(default)]
    pub markers: MarkerPrefs,
    /// Optional performance HUD; older configs leave it disabled.
    #[serde(default)]
    pub performance_display: PerformanceDisplay,
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
    /// Accessibility: colorblind-safe allegiance accents (indicator
    /// colors only; sprite art is untouched).
    #[serde(default)]
    pub colorblind: bool,
    /// Touch gesture timing (absent in configs saved before touch).
    #[serde(default)]
    pub touch: TouchPrefs,
    /// Actions the player EXPLICITLY unbound (Controls > X). A missing
    /// binding row alone is ambiguous — it also means "verb added
    /// after this config was saved" — and the migration that adopts
    /// classic chords for new verbs must not resurrect a deliberate
    /// unbinding on every restart.
    #[serde(default)]
    pub unbound: Vec<crate::action::Action>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            diagnostics: false,
            markers: MarkerPrefs::default(),
            performance_display: PerformanceDisplay::Off,
            version: CONFIG_VERSION,
            bindings: BindingMap::classic(),
            volumes: Volumes::default(),
            ui_scale: 1.0,
            camera: CameraPrefs::default(),
            window: (1280, 800),
            reduced_motion: false,
            colorblind: false,
            touch: TouchPrefs::default(),
            unbound: Vec::new(),
        }
    }
}

fn config_path() -> Option<PathBuf> {
    crate::paths::config_dir().map(|d| d.join("config.json"))
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
                config.bindings.migrate(&config.unbound);
                if !config.bindings.valid() {
                    config.bindings = BindingMap::classic();
                    config.unbound.clear();
                }
                config.window = Self::sane_window(config.window);
                config.touch = config.touch.clamped();
                config.markers = config.markers.clamped();
                config
            }
            _ => Self::default(),
        }
    }

    /// Persists atomically (temp + rename), creating the directory.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = config_path() else {
            return Ok(()); // headless CI without HOME: nothing to do
        };
        self.save_to(&path)
    }

    fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self).expect("config serializes");
        chassis::fsx::write_atomic(path, |writer| writer.write_all(text.as_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_preferences_migrate_round_trip_and_clamp_without_resetting_other_settings() {
        let dir = std::env::temp_dir().join(format!("oxide-config-markers-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config {
            ui_scale: 1.25,
            ..Config::default()
        };
        config.save_to(&path).unwrap();
        let mut legacy = serde_json::to_value(&config).unwrap();
        legacy.as_object_mut().unwrap().remove("markers");
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(Config::load_from(Some(path.clone())), config);

        config.markers = MarkerPrefs {
            start: 30.0,
            end: 22.0,
            scale: 1.5,
        };
        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(Some(path.clone())), config);

        config.markers = MarkerPrefs {
            start: -100.0,
            end: 300.0,
            scale: -1.0,
        };
        config.save_to(&path).unwrap();
        let loaded = Config::load_from(Some(path));
        assert_eq!(
            loaded.markers,
            MarkerPrefs {
                start: 31.0,
                end: 30.0,
                scale: 0.75
            }
        );
        assert_eq!(loaded.ui_scale, 1.25);
        assert_eq!(
            MarkerPrefs {
                start: f32::NAN,
                end: f32::INFINITY,
                scale: f32::NEG_INFINITY
            }
            .clamped(),
            MarkerPrefs::default()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_customized_map_survives_the_round_trip() {
        // The whole point of persistence: a rebind must still be there
        // after restart. A payload-validation off-by-one once rejected
        // the classic map's own one-based Slot(9) and reset everything.
        let dir = std::env::temp_dir().join(format!("oxide-config-custom-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config::default();
        assert!(
            config.bindings.rebind(
                crate::action::Action::Patrol,
                crate::action::Chord::bare(oxide_protocol::Key::J)
            ),
            "J is free in the classic map"
        );
        config.save_to(&path).expect("save");
        let loaded = Config::load_from(Some(path));
        assert_eq!(
            loaded.bindings.chord_for(crate::action::Action::Patrol),
            Some(crate::action::Chord::bare(oxide_protocol::Key::J)),
            "the customization survived validation"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_unbinding_survives_the_restart() {
        // Controls > X removes the row AND records the choice; without
        // the tombstone the new-verb migration read the missing row as
        // an old config and resurrected the classic chord every load.
        let dir = std::env::temp_dir().join(format!("oxide-config-unbind-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config::default();
        config.bindings.unbind(crate::action::Action::Patrol);
        config.unbound.push(crate::action::Action::Patrol);
        config.save_to(&path).expect("save");
        let loaded = Config::load_from(Some(path));
        assert_eq!(
            loaded.bindings.chord_for(crate::action::Action::Patrol),
            None,
            "the deliberate unbinding persisted"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_config_saved_before_a_new_verb_adopts_its_classic_chord() {
        // A config saved before an action existed has no row for it.
        // Loading must graft the classic chord in when free instead of
        // leaving the verb keyboardless, without stealing a claimed chord.
        let dir = std::env::temp_dir().join(format!("oxide-config-newverb-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config {
            bindings: BindingMap::legacy(),
            ..Config::default()
        };
        config.bindings.unbind(crate::action::Action::Salvage);
        config.save_to(&path).expect("save");
        let loaded = Config::load_from(Some(path.clone()));
        assert_eq!(
            loaded.bindings.chord_for(crate::action::Action::Salvage),
            Some(crate::action::Chord::bare(oxide_protocol::Key::V)),
            "the missing verb adopted its classic chord"
        );

        // Same again, but the player owns V: the verb stays unbound.
        let mut config = Config {
            bindings: BindingMap::legacy(),
            ..Config::default()
        };
        config.bindings.unbind(crate::action::Action::Salvage);
        assert!(config.bindings.rebind(
            crate::action::Action::Patrol,
            crate::action::Chord::bare(oxide_protocol::Key::V)
        ));
        config.save_to(&path).expect("save");
        let loaded = Config::load_from(Some(path));
        assert_eq!(
            loaded.bindings.chord_for(crate::action::Action::Salvage),
            None,
            "a claimed chord is never stolen back"
        );
        assert_eq!(
            loaded.bindings.chord_for(crate::action::Action::Patrol),
            Some(crate::action::Chord::bare(oxide_protocol::Key::V)),
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stale_group_chords_drop_without_resetting_the_profile() {
        // A config saved before the classic map trimmed Ctrl+6..9 keeps
        // a chord to a group dispatch ignores; loading must shed that
        // row alone, never the user's own customizations with it.
        let dir = std::env::temp_dir().join(format!("oxide-config-stale-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config {
            bindings: BindingMap::legacy(),
            ..Config::default()
        };
        assert!(config.bindings.rebind(
            crate::action::Action::AssignGroup(7),
            crate::action::Chord::ctrl(oxide_protocol::Key::Num7)
        ));
        assert!(config.bindings.rebind(
            crate::action::Action::Patrol,
            crate::action::Chord::bare(oxide_protocol::Key::J)
        ));
        config.save_to(&path).expect("save");
        let loaded = Config::load_from(Some(path));
        assert_eq!(
            loaded
                .bindings
                .chord_for(crate::action::Action::AssignGroup(7)),
            None,
            "the stale chord is gone"
        );
        assert_eq!(
            loaded.bindings.chord_for(crate::action::Action::Patrol),
            Some(crate::action::Chord::bare(oxide_protocol::Key::J)),
            "the customization survived the strip"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saving_twice_replaces_instead_of_failing() {
        // std's rename replaces existing destinations on every
        // platform; the second save must land and its content win.
        let dir = std::env::temp_dir().join(format!("oxide-config-twice-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config::default();
        config.save_to(&path).expect("first save");
        config.ui_scale = 1.5;
        config.save_to(&path).expect("second save replaces");
        let loaded = Config::load_from(Some(path));
        assert!((loaded.ui_scale - 1.5).abs() < 1e-6, "the newer config won");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_save_leaves_the_old_config_standing() {
        // A read-only directory refuses the temp file; the previous
        // config must survive untouched and no temp may linger.
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!("oxide-config-fail-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("config.json");
        let config = Config::default();
        config.save_to(&path).expect("first save");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let changed = Config {
            ui_scale: 1.5,
            ..Config::default()
        };
        assert!(changed.save_to(&path).is_err(), "the failure surfaces");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let loaded = Config::load_from(Some(path));
        assert_eq!(loaded, config, "the old config survived the failed save");
        let temps: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(temps.is_empty(), "no temp survives a failed save");
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
        let back = Config::load_from(Some(path));
        assert_eq!(back, config);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn performance_modes_persist_and_old_configs_keep_preferences() {
        let dir =
            std::env::temp_dir().join(format!("oxide-config-performance-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config {
            ui_scale: 1.25,
            ..Config::default()
        };
        config.volumes.master = 0.25;
        for mode in [
            PerformanceDisplay::Off,
            PerformanceDisplay::Fps,
            PerformanceDisplay::Detailed,
        ] {
            config.performance_display = mode;
            config.save_to(&path).unwrap();
            assert_eq!(Config::load_from(Some(path.clone())), config);
        }
        let mut old = serde_json::to_value(&config).unwrap();
        old.as_object_mut().unwrap().remove("performance_display");
        std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        config.performance_display = PerformanceDisplay::Off;
        assert_eq!(Config::load_from(Some(path)), config);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_config_from_before_the_music_bus_keeps_every_other_setting() {
        let dir =
            std::env::temp_dir().join(format!("oxide-config-pre-music-{}", std::process::id()));
        let path = dir.join("config.json");
        std::fs::create_dir_all(&dir).unwrap();
        let mut old = serde_json::to_value(Config {
            ui_scale: 1.25,
            volumes: Volumes {
                effects: 0.5,
                ..Volumes::default()
            },
            ..Config::default()
        })
        .unwrap();
        old["volumes"].as_object_mut().unwrap().remove("music");
        std::fs::write(&path, serde_json::to_vec_pretty(&old).unwrap()).unwrap();

        let loaded = Config::load_from(Some(path));
        assert_eq!(loaded.volumes.music, 1.0);
        assert_eq!(loaded.volumes.effects, 0.5);
        assert_eq!(loaded.ui_scale, 1.25);
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
        assert_eq!(Config::load_from(Some(path)), Config::default());
        std::fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn an_empty_secondary_slot_and_rebound_primary_survive_reload() {
        use crate::action::{Action, Chord};
        let dir =
            std::env::temp_dir().join(format!("oxide-config-secondary-{}", std::process::id()));
        let path = dir.join("config.json");
        let mut config = Config::default();
        config.bindings.unbind_slot(Action::PanUp, 1);
        assert!(
            config
                .bindings
                .rebind(Action::PanUp, Chord::bare(oxide_protocol::Key::I))
        );
        config.save_to(&path).unwrap();
        let loaded = Config::load_from(Some(path));
        assert_eq!(loaded.bindings, config.bindings);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
