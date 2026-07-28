//! The perceptual-diff screenshot suite: twelve canonical screens captured
//! from a real automation-mode shell and compared against per-machine
//! references on a tolerance metric.
//!
//! Pixel goldens don't survive GPU or font churn, so this is a LOCAL
//! gate, never a CI one: references live in a gitignored directory,
//! belong to this machine, and re-bless with `--bless` after any
//! intended visual change. A compare run never adopts a missing
//! reference implicitly; it keeps the run capture and fails with a
//! prompt to bless explicitly. The spawned shell gets a throwaway HOME
//! (fresh config with reduced motion on, no autosaves), so the walk and
//! the Home screen's backdrop are reproducible run to run.
//!
//! ```text
//! cargo run -p oxide-driver -- shots            # compare
//! cargo run -p oxide-driver -- shots --bless    # adopt current
//! ```

use crate::auto::{self, SpawnOptions};
use crate::client::Client;
use anyhow::{Context, Result, bail};
use oxide_kit::perceptual::{self, Verdict};
use oxide_protocol::{Key, MouseButton, RawEvent, Reply, Request};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HOME: AtomicU64 = AtomicU64::new(0);
static NEXT_ADOPTION: AtomicU64 = AtomicU64::new(0);

struct ScratchHome(PathBuf);

impl Drop for ScratchHome {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "warning: could not remove shots scratch HOME {}: {error}",
                self.0.display()
            );
        }
    }
}

/// A throwaway HOME whose Oxide config pins the presentation knobs the
/// suite depends on: reduced motion (stills the Home backdrop drift),
/// default scale, classic bindings (empty list resets to classic on
/// load), 1280x800. Both the macOS and XDG locations are written so the
/// suite behaves the same on either platform.
fn scratch_home() -> Result<ScratchHome> {
    let home = loop {
        let id = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("oxide-shots-home-{}-{id}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => break ScratchHome(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    };
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
        home.0.join("Library/Application Support/Oxide"),
        home.0.join(".config/oxide"),
        home.0.join("AppData/Oxide"),
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
    captured: Vec<String>,
    failed: Vec<String>,
}

enum ReferenceResult {
    Missing,
    Diff(Verdict),
}

fn check_reference(run: &Path, reference: &Path) -> Result<ReferenceResult> {
    if !reference.exists() {
        return Ok(ReferenceResult::Missing);
    }
    Ok(ReferenceResult::Diff(perceptual::diff_pngs(
        reference, run,
    )?))
}

/// Stages a complete reference directory, then swaps it into place. A
/// failed walk never calls this; a failed staging copy leaves the old set
/// untouched instead of half-blessing a visual change.
fn adopt_reference_set(dir: &Path, names: &[String]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let nonce = NEXT_ADOPTION.fetch_add(1, Ordering::Relaxed);
    let suffix = format!("{}.{}", std::process::id(), nonce);
    let staged = dir.join(format!(".ref-staged-{suffix}"));
    let backup = dir.join(format!(".ref-backup-{suffix}"));
    std::fs::create_dir(&staged)?;

    let staged_result = (|| -> Result<()> {
        for name in names {
            let source = dir.join("run").join(format!("{name}.png"));
            let target = staged.join(format!("{name}.png"));
            std::fs::copy(&source, &target)
                .with_context(|| format!("staging reference {}", source.display()))?;
        }
        let reference = dir.join("ref");
        let had_reference = reference.exists();
        if had_reference {
            std::fs::rename(&reference, &backup)?;
        }
        if let Err(error) = std::fs::rename(&staged, &reference) {
            if had_reference {
                std::fs::rename(&backup, &reference).with_context(|| {
                    format!(
                        "restoring reference set after adoption failed: {}",
                        reference.display()
                    )
                })?;
            }
            return Err(error.into());
        }
        if had_reference && let Err(error) = std::fs::remove_dir_all(&backup) {
            eprintln!(
                "warning: adopted references but could not remove backup {}: {error}",
                backup.display()
            );
        }
        Ok(())
    })();

    if staged.exists() {
        std::fs::remove_dir_all(&staged).ok();
    }
    staged_result
}

impl Suite<'_> {
    fn shot(&mut self, name: &str, expected_mode: &str) -> Result<()> {
        self.taken += 1;
        auto::assert_mode(self.client, expected_mode, name)?;
        let run = self.dir.join("run").join(format!("{name}.png"));
        let reference = self.dir.join("ref").join(format!("{name}.png"));
        capture(self.client, &run)?;
        if self.bless {
            println!("  CAPTURE {name}");
            self.captured.push(name.to_string());
            return Ok(());
        }
        match check_reference(&run, &reference)? {
            ReferenceResult::Missing => {
                println!(
                    "  FAIL {name}  missing reference {}; run capture kept at {}; rerun with \
                     --bless to adopt it",
                    reference.display(),
                    run.display()
                );
                self.failed.push(name.to_string());
            }
            ReferenceResult::Diff(Verdict::Score(score)) if score <= self.threshold => {
                println!("  ok   {name}  {score:.3}%");
            }
            ReferenceResult::Diff(Verdict::Score(score)) => {
                println!(
                    "  FAIL {name}  {score:.3}% > {:.2}% — compare {} vs {}",
                    self.threshold,
                    reference.display(),
                    run.display()
                );
                self.failed.push(name.to_string());
            }
            ReferenceResult::Diff(Verdict::SizeMismatch {
                reference: r,
                candidate: c,
            }) => {
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

/// Runs the twelve-shot walk. Spawns its own shell on `port`.
pub fn run(port: u16, bless: bool, dir: &Path, threshold: f64) -> Result<()> {
    let home = scratch_home()?;
    let (guard, mut client) = auto::spawn_shell(&SpawnOptions {
        port,
        paused: true,
        home: Some(home.0.clone()),
    })?;
    let outcome = walk(&mut client, bless, dir, threshold);
    drop(guard);
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
        captured: Vec::new(),
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
    // Back on the grid, return to the first 1v1: the same setup
    // screen, duel-shaped (seat cards without team headings).
    auto::press_key(suite.client, Key::Home)?;
    auto::press_key(suite.client, Key::Enter)?;
    suite.shot("wizard-duel-setup", "match_setup")?;

    // Into the game (paused clock: tick 0 forever, deterministic HUD).
    // Start is preselected: one Enter launches.
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

    // Settings over the paused match (the pause payload waits), then a
    // refused rebind: the screen-owned notice names the chord's holder
    // above the veil. Last on purpose — the notice's glyph sizes enter
    // the font atlas here, and drawing them earlier re-rasterized the
    // text of every capture that followed.
    auto::activate_labeled(suite.client, "settings")?;
    auto::activate_labeled(suite.client, "controls")?;
    auto::press_key(suite.client, Key::Enter)?;
    auto::press_key(suite.client, Key::M)?;
    suite.shot("controls-conflict", "controls")?;

    if !suite.failed.is_empty() {
        bail!(
            "shot failures: {}; run captures kept in {}",
            suite.failed.join(", "),
            suite.dir.join("run").display()
        );
    }
    if suite.bless {
        adopt_reference_set(&suite.dir, &suite.captured)?;
        for name in &suite.captured {
            println!("  BLESS {name}");
        }
    }
    println!(
        "shots: {} compared, {} blessed, {} failed",
        if suite.bless {
            0
        } else {
            suite.taken.saturating_sub(suite.failed.len())
        },
        suite.captured.len(),
        suite.failed.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ReferenceResult, adopt_reference_set, check_reference};
    use anyhow::Result;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Result<Self> {
            loop {
                let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("oxide-shots-test-{}-{id}", std::process::id()));
                match std::fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn missing_reference_fails_without_adopting_capture() -> Result<()> {
        let dir = TestDir::new()?;
        let run = dir.0.join("run/example.png");
        let reference = dir.0.join("ref/example.png");
        std::fs::create_dir_all(run.parent().expect("has parent"))?;
        std::fs::write(&run, b"current capture")?;

        assert!(matches!(
            check_reference(&run, &reference)?,
            ReferenceResult::Missing
        ));
        assert!(!reference.exists());
        assert_eq!(std::fs::read(run)?, b"current capture");
        Ok(())
    }

    #[test]
    fn bless_explicitly_adopts_the_complete_capture_set() -> Result<()> {
        let dir = TestDir::new()?;
        std::fs::create_dir_all(dir.0.join("run"))?;
        std::fs::create_dir_all(dir.0.join("ref"))?;
        std::fs::write(dir.0.join("run/a.png"), b"new a")?;
        std::fs::write(dir.0.join("run/b.png"), b"new b")?;
        std::fs::write(dir.0.join("ref/a.png"), b"old a")?;

        adopt_reference_set(&dir.0, &["a".to_string(), "b".to_string()])?;

        assert_eq!(std::fs::read(dir.0.join("ref/a.png"))?, b"new a");
        assert_eq!(std::fs::read(dir.0.join("ref/b.png"))?, b"new b");
        Ok(())
    }

    #[test]
    fn failed_staging_leaves_the_old_reference_set_untouched() -> Result<()> {
        let dir = TestDir::new()?;
        std::fs::create_dir_all(dir.0.join("run"))?;
        std::fs::create_dir_all(dir.0.join("ref"))?;
        std::fs::write(dir.0.join("run/a.png"), b"new a")?;
        std::fs::write(dir.0.join("ref/a.png"), b"old a")?;

        assert!(adopt_reference_set(&dir.0, &["a".to_string(), "missing".to_string()]).is_err());
        assert_eq!(std::fs::read(dir.0.join("ref/a.png"))?, b"old a");
        Ok(())
    }
}
