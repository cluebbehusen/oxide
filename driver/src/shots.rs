//! The perceptual-diff screenshot suite: ten canonical screens captured
//! from a real automation-mode shell and compared against per-machine
//! references on a tolerance metric.
//!
//! Pixel goldens don't survive GPU or font churn, so this is a LOCAL
//! gate, never a CI one: references live in a gitignored directory,
//! belong to this machine, and re-bless with `--bless` after any
//! intended visual change. The spawned shell gets a throwaway HOME
//! (fresh config with reduced motion on, no autosaves), so the walk and
//! the Home screen's backdrop are reproducible run to run.
//!
//! ```text
//! cargo run -p oxide-driver -- shots            # compare
//! cargo run -p oxide-driver -- shots --bless    # adopt current
//! ```

use crate::auto::{self, SpawnOptions};
use crate::client::Client;
use anyhow::{Result, bail};
use oxide_kit::perceptual::{self, Verdict};
use oxide_protocol::{Key, MouseButton, RawEvent, Reply, Request};
use std::path::{Path, PathBuf};

/// A throwaway HOME whose Oxide config pins the presentation knobs the
/// suite depends on: reduced motion (stills the Home backdrop drift),
/// default scale, classic bindings (empty list resets to classic on
/// load), 1280x800. Both the macOS and XDG locations are written so the
/// suite behaves the same on either platform.
fn scratch_home() -> Result<PathBuf> {
    let home = std::env::temp_dir().join(format!("oxide-shots-home-{}", std::process::id()));
    let config = serde_json::json!({
        "version": 1,
        "bindings": { "bindings": [] },
        "volumes": { "master": 0.0, "effects": 0.0, "ui": 0.0, "music": 0.0 },
        "ui_scale": 1.0,
        "camera": { "pan_speed": 1.0, "edge_pan": false, "zoom_inverted": false },
        "window": [1280, 800],
        "reduced_motion": true,
        "colorblind": false
    });
    let text = serde_json::to_string_pretty(&config).expect("static json");
    for dir in [
        home.join("Library/Application Support/Oxide"),
        home.join(".config/oxide"),
        home.join("AppData/Oxide"),
    ] {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("config.json"), &text)?;
    }
    Ok(home)
}

fn capture(client: &mut Client, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let reply = client.call(Request::Screenshot {
        path: Some(path.to_string_lossy().into_owned()),
    })?;
    match reply {
        Reply::Screenshot(_) => Ok(()),
        other => bail!("screenshot returned {other:?}"),
    }
}

/// Polls the shell's mode report until it matches (screen transitions
/// that load assets or build a Game take more than a frame).
fn wait_mode(client: &mut Client, expected: &str, seconds: u32) -> Result<()> {
    let mut last = String::new();
    for _ in 0..seconds * 10 {
        last = auto::ui(client)?.mode;
        if last == expected {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    bail!("mode never became '{expected}' (still '{last}')");
}

struct Suite<'a> {
    client: &'a mut Client,
    dir: PathBuf,
    bless: bool,
    threshold: f64,
    taken: usize,
    new: Vec<String>,
    failed: Vec<String>,
}

impl Suite<'_> {
    fn shot(&mut self, name: &str, expected_mode: &str) -> Result<()> {
        self.taken += 1;
        auto::assert_mode(self.client, expected_mode, name)?;
        let run = self.dir.join("run").join(format!("{name}.png"));
        let reference = self.dir.join("ref").join(format!("{name}.png"));
        capture(self.client, &run)?;
        if self.bless || !reference.exists() {
            std::fs::create_dir_all(reference.parent().expect("has parent"))?;
            std::fs::copy(&run, &reference)?;
            println!("  NEW  {name} (reference blessed)");
            self.new.push(name.to_string());
            return Ok(());
        }
        match perceptual::diff_pngs(&reference, &run)? {
            Verdict::Score(score) if score <= self.threshold => {
                println!("  ok   {name}  {score:.3}%");
            }
            Verdict::Score(score) => {
                println!(
                    "  FAIL {name}  {score:.3}% > {:.2}% — compare {} vs {}",
                    self.threshold,
                    reference.display(),
                    run.display()
                );
                self.failed.push(name.to_string());
            }
            Verdict::SizeMismatch {
                reference: r,
                candidate: c,
            } => {
                println!(
                    "  FAIL {name}  size {}x{} vs {}x{} — window or DPI changed; re-bless",
                    r.0, r.1, c.0, c.1
                );
                self.failed.push(name.to_string());
            }
        }
        Ok(())
    }
}

/// Runs the ten-shot walk. Spawns its own shell on `port`.
pub fn run(port: u16, bless: bool, dir: &Path, threshold: f64) -> Result<()> {
    let home = scratch_home()?;
    let (guard, mut client) = auto::spawn_shell(&SpawnOptions {
        port,
        paused: true,
        home: Some(home.clone()),
    })?;
    let outcome = walk(&mut client, bless, dir, threshold);
    drop(guard);
    std::fs::remove_dir_all(&home).ok();
    outcome
}

fn walk(client: &mut Client, bless: bool, dir: &Path, threshold: f64) -> Result<()> {
    let dir = dir.to_path_buf();
    let dir_abs = if dir.is_absolute() {
        dir
    } else {
        std::env::current_dir()?.join(dir)
    };
    println!("shots: comparing against {}", dir_abs.join("ref").display());
    let mut suite = Suite {
        client,
        dir: dir_abs,
        bless,
        threshold,
        taken: 0,
        new: Vec::new(),
        failed: Vec::new(),
    };

    // The menu face of the game.
    suite.shot("home", "home")?;
    auto::activate_labeled(suite.client, "settings")?;
    suite.shot("settings", "settings")?;
    auto::activate_labeled(suite.client, "controls")?;
    suite.shot("controls", "controls")?;
    auto::press_key(suite.client, Key::Escape)?;
    auto::press_key(suite.client, Key::Escape)?;
    auto::activate_labeled(suite.client, "replays")?;
    suite.shot("replay-shelf", "replays")?;
    auto::press_key(suite.client, Key::Escape)?;

    // The wizard.
    auto::activate_labeled(suite.client, "play")?;
    suite.shot("wizard-map", "main_menu")?;
    // The team-map setup screen (seat cards + the who-is-where map):
    // End is the grid's last entry — the biggest team map.
    auto::press_key(suite.client, Key::End)?;
    auto::press_key(suite.client, Key::Enter)?;
    suite.shot("match-setup", "match_setup")?;
    auto::press_key(suite.client, Key::Escape)?;
    // Back on the grid, return to the first 1v1 for the quick flow.
    auto::press_key(suite.client, Key::Home)?;
    auto::press_key(suite.client, Key::Enter)?;
    suite.shot("wizard-difficulty", "difficulty_menu")?;

    // Into the game (paused clock: tick 0 forever, deterministic HUD).
    auto::press_key(suite.client, Key::Enter)?;
    auto::press_key(suite.client, Key::Enter)?;
    auto::press_key(suite.client, Key::Enter)?;
    wait_mode(suite.client, "playing", 15)?;
    suite.shot("game-hud", "playing")?;

    // Drag-select the starting units (camera opens on the home base).
    auto::inject(
        suite.client,
        RawEvent::MouseDown {
            button: MouseButton::Left,
            x: 350.0,
            y: 220.0,
        },
    )?;
    for (x, y) in [(500.0, 320.0), (700.0, 430.0), (900.0, 560.0)] {
        auto::inject(suite.client, RawEvent::MouseMove { x, y })?;
    }
    auto::inject(
        suite.client,
        RawEvent::MouseUp {
            button: MouseButton::Left,
            x: 900.0,
            y: 560.0,
        },
    )?;
    suite.shot("game-panel", "playing")?;

    // The build palette, then the pause veil. Escape unwinds one layer
    // at a time (palette, then selection, then pause — the mode-cancel
    // rule), so press until the shell reports the pause screen.
    auto::press_key(suite.client, Key::B)?;
    suite.shot("build-palette", "playing")?;
    for _ in 0..4 {
        if auto::ui(suite.client)?.mode == "pause_menu" {
            break;
        }
        auto::press_key(suite.client, Key::Escape)?;
    }
    suite.shot("pause", "pause_menu")?;

    println!(
        "shots: {} compared, {} new, {} failed",
        // Counted, not hardcoded: the walk grows, and a stale literal
        // underflowed the summary the day it did.
        suite
            .taken
            .saturating_sub(suite.new.len() + suite.failed.len()),
        suite.new.len(),
        suite.failed.len()
    );
    if !suite.failed.is_empty() {
        bail!("shot drift: {}", suite.failed.join(", "));
    }
    Ok(())
}
