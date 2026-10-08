//! Persisted presentation config: bindings, volumes, UI scale, camera
//! feel, window size, and the opponent AI for new matches.
//!
//! Nothing here may affect a running match: the opponent AI is copied into
//! each new match's scenario, which saves and replays then carry. The config
//! versions independently of replays and loses nothing when it
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

fn default_on() -> bool {
    true
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

/// Touch gesture preferences. Fields a config predates take their
/// defaults, so adding one never resets the rest of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
    /// Window size at startup, `WIDTHxHEIGHT`.
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
    /// Show the control-group column above the minimap. Hiding it
    /// leaves keyboard groups working.
    #[serde(default = "default_on")]
    pub control_groups: bool,
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
    /// The host address the last LAN join used.
    #[serde(default)]
    pub last_join_address: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
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
            control_groups: true,
            touch: TouchPrefs::default(),
            unbound: Vec::new(),
            last_join_address: None,
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
mod tests;
