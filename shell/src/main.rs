//! The Oxide shell: a thin macroquad window over the deterministic sim.
//!
//! This file is only the doorstep — argument parsing, window
//! configuration, and the startup trace. The session coordinator (the
//! `App` struct, the payload-carrying `Screen` enum, the frame loop,
//! and the debug dispatch) lives in [`app`].
//!
//! Nothing in this crate may affect game outcomes except by staging
//! tick-stamped commands. If a feature can't be expressed that way, it
//! belongs in the sim.

mod action;
mod app;
mod assets;
mod autosave;
mod camera;
mod config;
mod debug_server;
mod frame_profile;
mod game;
mod input;
mod layout;
mod menu;
mod panel;
mod paths;
mod render;
mod saves;
mod screens;
mod soundtrack;
mod theme;
mod tutorial;

use clap::Parser;
use macroquad::prelude::*;

#[derive(Parser)]
#[command(name = "oxide-shell", version, about = "Oxide, playable")]
struct Args {
    /// Scenario JSON path (skips the menu).
    #[arg(long, conflicts_with = "replay")]
    scenario: Option<String>,

    /// Resume a session from a replay JSON (skips the menu).
    #[arg(long)]
    replay: Option<String>,

    /// Open a replay in the read-only playback viewer (pause, speed,
    /// seek — no recorder, no commands).
    #[arg(long, conflicts_with_all = ["scenario", "replay", "automation"])]
    watch: Option<String>,

    /// Serve the debug protocol on --port (skips the menu unless automated).
    #[arg(long)]
    debug_server: bool,

    /// Deterministic UI-driving mode: start at the main menu and accept only
    /// injected input, never hardware input.
    #[arg(
        long,
        requires = "debug_server",
        conflicts_with_all = ["scenario", "replay"]
    )]
    automation: bool,

    /// Debug server port.
    #[arg(long, default_value_t = oxide_protocol::DEFAULT_PORT)]
    port: u16,

    /// Seconds a silent debug connection may sit before it is closed.
    /// Generous by default — a paused driven-mode agent legitimately
    /// parks between commands; raise it for parked-overnight sessions.
    #[arg(long, requires = "debug_server", default_value_t = 30 * 60,
          value_parser = clap::value_parser!(u64).range(1..))]
    debug_idle_timeout: u64,

    /// Start with the sim clock stopped: time advances only via the debug
    /// socket (driven mode). Rendering still runs.
    #[arg(long)]
    paused: bool,

    /// Wall-clock speed multiplier.
    #[arg(long, default_value_t = 1.0, value_parser = parse_speed)]
    speed: f64,

    /// Window size as WIDTHxHEIGHT (e.g. 800x600) — the UX matrix boots
    /// the shell at every supported size.
    #[arg(long, value_parser = parse_window)]
    window: Option<(u32, u32)>,

    /// Render at logical (non-retina) pixel density, exercising the 1x
    /// layout path on high-DPI displays.
    #[arg(long)]
    no_high_dpi: bool,

    /// Print startup diagnostics to stderr: prologue milestones with
    /// ms-since-entry, then per-frame gap and hardware-event counts for
    /// the first frames. OXIDE_TRACE_STARTUP=1 enables it too (handy
    /// for the packaged .app, where flags are awkward).
    #[arg(long)]
    trace_startup: bool,

    /// Collect bounded native GPU-shell frame timings for
    /// query_performance. Off by default so ordinary play pays no timing or
    /// sample-retention cost.
    #[arg(long, requires = "debug_server")]
    profile_frames: bool,
}

/// Wall-clock zero for the startup trace, pinned by the first caller —
/// `window_conf` runs before the window exists, so it lands close to
/// process entry.
static TRACE_ENTRY: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Frames the startup trace reports before going quiet.
const TRACE_FRAMES: u32 = 200;

/// The env var alone must not switch tracing on under `--automation`:
/// the shots and menu_ux harnesses capture stderr from spawned shells,
/// and an exported OXIDE_TRACE_STARTUP would leak into every one. The
/// explicit flag always wins.
fn trace_active(flag: bool, automation: bool, env_set: bool) -> bool {
    flag || (env_set && !automation)
}

fn trace_startup_enabled(args: &Args) -> bool {
    let env_set = std::env::var("OXIDE_TRACE_STARTUP").is_ok_and(|v| v == "1");
    trace_active(args.trace_startup, args.automation, env_set)
}

fn trace_mark(label: &str) {
    let entry = TRACE_ENTRY.get_or_init(std::time::Instant::now);
    eprintln!(
        "[trace-startup] {label} +{:.1}ms",
        entry.elapsed().as_secs_f64() * 1000.0
    );
}

/// Parses `WIDTHxHEIGHT` with sane floors — smaller than 640x400 and the
/// fixed chrome cannot physically fit.
fn parse_window(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once('x')
        .ok_or_else(|| "expected WIDTHxHEIGHT, e.g. 800x600".to_string())?;
    let (w, h): (u32, u32) = (
        w.parse().map_err(|err| format!("{err}"))?,
        h.parse().map_err(|err| format!("{err}"))?,
    );
    if w < 640 || h < 400 {
        return Err("window must be at least 640x400".to_string());
    }
    if w > 16_384 || h > 16_384 {
        // The native config takes i32; 4294967295x400 once reached the
        // backend as -1.
        return Err("window must be at most 16384x16384".to_string());
    }
    Ok((w, h))
}

/// Same envelope the debug socket enforces — the CLI shouldn't accept less
/// sane values than the protocol does.
fn parse_speed(s: &str) -> Result<f64, String> {
    let v: f64 = s.parse().map_err(|err| format!("{err}"))?;
    if v.is_finite() && (0.05..=64.0).contains(&v) {
        Ok(v)
    } else {
        Err("speed must be a finite value within 0.05..=64".to_string())
    }
}

fn window_conf() -> Conf {
    TRACE_ENTRY.get_or_init(std::time::Instant::now);
    // The window is created before `main()` ever sees clap's output, so
    // the size/DPI flags are parsed here too — clap is idempotent and
    // errors surface identically on the second parse in `main()`.
    let args = Args::parse();
    if trace_startup_enabled(&args) {
        trace_mark("window_conf enter");
    }
    let (width, height) = args.window.unwrap_or(config::Config::load().window);
    Conf {
        window_title: "Oxide".to_string(),
        window_width: width as i32,
        window_height: height as i32,
        // Render at native pixel density — pre-atlas this was too many
        // pixels to afford; post-atlas it's crisp text and art for free.
        high_dpi: !args.no_high_dpi,
        // Dock/taskbar face on every backend that takes one (miniquad
        // hands `big` to the macOS dock). Generated by tools/gen_icon.py;
        // the packaged .app carries the same mark as its icns.
        icon: Some(macroquad::miniquad::conf::Icon {
            small: *include_bytes!("../../assets/icon/oxide_16.rgba"),
            medium: *include_bytes!("../../assets/icon/oxide_32.rgba"),
            big: *include_bytes!("../../assets/icon/oxide_64.rgba"),
        }),
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    if let Err(err) = app::run(Args::parse()).await {
        eprintln!("fatal: {err:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_requires_debug_server() {
        assert!(Args::try_parse_from(["oxide-shell", "--automation"]).is_err());
        let args = Args::try_parse_from(["oxide-shell", "--debug-server", "--automation"])
            .expect("automation with the debug server should parse");
        assert!(args.debug_server);
        assert!(args.automation);
    }

    #[test]
    fn trace_env_never_reaches_an_automation_shell() {
        // Harness-spawned shells inherit the developer's environment;
        // only the explicit flag may trace under --automation.
        assert!(!trace_active(false, true, true));
        assert!(trace_active(true, true, false));
        assert!(trace_active(false, false, true));
        assert!(!trace_active(false, false, false));
    }
}
