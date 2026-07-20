//! The layer-3 smoke test: drive a live shell end to end.
//!
//! Exercises the seams the headless tests cannot — the debug socket, the
//! input funnel, the camera, screenshots, and the session recorder — and
//! finishes by proving the live session reproduces headless (save the
//! replay, re-run it, compare hashes). Run it whenever shell code changes:
//!
//! ```text
//! cargo run -p oxide-driver -- smoke --spawn
//! ```

use crate::client::Client;
use crate::runner::{self, GameReplay};
use anyhow::{Context, Result, bail};
use oxide_protocol::{RawEvent, Reply, Request, StateFilter};
use oxide_sim::{Command, PlayerId, UnitId, UnitKind};
use std::time::Duration;

/// Runs the smoke sequence against `addr`, optionally spawning a shell
/// first (killed on exit, pass or fail).
pub fn run(addr: &str, spawn: bool) -> Result<()> {
    let mut child = if spawn {
        // Spawn on the port we'll actually connect to, not the default.
        let port = addr
            .rsplit(':')
            .next()
            .context("addr must look like host:port")?;
        Some(
            std::process::Command::new("cargo")
                .args([
                    "run",
                    "-p",
                    "oxide-shell",
                    "--",
                    "--debug-server",
                    "--paused",
                    "--port",
                    port,
                ])
                .spawn()
                .context("spawning oxide-shell via cargo")?,
        )
    } else {
        None
    };
    let outcome = execute(addr, spawn);
    if let Some(child) = &mut child {
        child.kill().ok();
        child.wait().ok();
    }
    outcome
}

struct Checks {
    passed: u32,
    failures: Vec<String>,
}

impl Checks {
    fn note(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        if ok {
            println!("  ok   {name}");
            self.passed += 1;
        } else {
            println!("  FAIL {name}: {detail}");
            self.failures.push(name.to_string());
        }
    }
}

fn connect_with_retry(addr: &str, attempts: u32) -> Result<Client> {
    let mut last_err = None;
    for _ in 0..attempts {
        match Client::connect(addr) {
            Ok(client) => return Ok(client),
            Err(err) => {
                last_err = Some(err);
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no attempts made")))
}

fn execute(addr: &str, patient: bool) -> Result<()> {
    println!("smoke: connecting to {addr}");
    // A spawned shell may need a full cargo build first.
    let mut client = connect_with_retry(addr, if patient { 240 } else { 6 })?;
    let mut checks = Checks {
        passed: 0,
        failures: Vec::new(),
    };

    // Status and clock control. Pause up front: tick-exact assertions
    // against a free-running shell would race its wall clock. The previous
    // pause state is restored on the way out.
    let Reply::Status(status) = client.call(Request::Status)? else {
        bail!("status returned the wrong reply kind");
    };
    checks.note(
        "status responds",
        !status.scenario.is_empty(),
        "empty scenario name",
    );
    let was_running = !status.paused;
    client.call(Request::Pause)?;
    // The checks bail on protocol surprises; the shell's clock must be
    // restored even then, or a failing smoke leaves a paused session.
    let outcome = run_checks(&mut client, &mut checks);
    if was_running {
        client.call(Request::Resume).ok();
    }
    outcome?;

    println!(
        "smoke: {} passed, {} failed",
        checks.passed,
        checks.failures.len()
    );
    if !checks.failures.is_empty() {
        bail!("smoke failures: {}", checks.failures.join(", "));
    }
    Ok(())
}

fn run_checks(client: &mut Client, checks: &mut Checks) -> Result<()> {
    let Reply::Hash(start) = client.call(Request::StateHash)? else {
        bail!("state_hash returned the wrong reply kind");
    };
    let Reply::Advanced(advanced) = client.call(Request::AdvanceTicks { ticks: 10 })? else {
        bail!("advance_ticks returned the wrong reply kind");
    };
    checks.note(
        "advance_ticks moves exactly N ticks",
        advanced.tick == start.tick + 10,
        format!("{} -> {}", start.tick, advanced.tick),
    );

    // Injected wheel input must reach the camera through the real funnel.
    let Reply::Camera(before) = client.call(Request::QueryCamera)? else {
        bail!("query_camera returned the wrong reply kind");
    };
    client.call(Request::InjectEvent {
        event: RawEvent::Wheel { delta: 2.0 },
    })?;
    std::thread::sleep(Duration::from_millis(300)); // a few frames
    let Reply::Camera(after) = client.call(Request::QueryCamera)? else {
        bail!("query_camera returned the wrong reply kind");
    };
    checks.note(
        "wheel zooms the camera in",
        after.zoom > before.zoom,
        format!("zoom {} -> {}", before.zoom, after.zoom),
    );

    // A game command through the socket, verified in sim state.
    let Reply::State(view) = client.call(Request::QueryState {
        filter: StateFilter::default(),
    })?
    else {
        bail!("query_state returned the wrong reply kind");
    };
    let mover = view
        .units
        .iter()
        .find(|u| u.player == 0 && u.kind == UnitKind::Harvester)
        .context("no player-0 harvester on the map")?;
    let (id, start_pos) = (mover.id, mover.pos);
    client.call(Request::SendCommand {
        player: PlayerId(0),
        command: Command::Move {
            queue: false,
            units: vec![UnitId(id)],
            goal: chassis::grid::TilePos::new(mover.tile[0] + 3, mover.tile[1] + 2),
        },
    })?;
    client.call(Request::AdvanceTicks { ticks: 60 })?;
    let Reply::State(view) = client.call(Request::QueryState {
        filter: StateFilter::default(),
    })?
    else {
        bail!("query_state returned the wrong reply kind");
    };
    let moved = view
        .units
        .iter()
        .find(|u| u.id == id)
        .context("unit vanished")?;
    checks.note(
        "move command moves the unit",
        moved.pos != start_pos,
        format!("{start_pos:?} -> {:?}", moved.pos),
    );

    // Screenshot lands on disk, right side up. Absolute, per-process paths:
    // the shell writes relative to ITS working directory, and parallel
    // smoke runs must not clobber each other.
    let pid = std::process::id();
    let shot_path = std::env::current_dir()?
        .join(format!("screenshots/smoke-{pid}.png"))
        .to_string_lossy()
        .into_owned();
    let Reply::Screenshot(shot) = client.call(Request::Screenshot {
        path: Some(shot_path.clone()),
    })?
    else {
        bail!("screenshot returned the wrong reply kind");
    };
    checks.note(
        "screenshot reports dimensions",
        shot.width > 0 && shot.height > 0,
        format!("{}x{}", shot.width, shot.height),
    );
    let png_bytes = std::fs::read(&shot.path).unwrap_or_default();
    checks.note(
        "screenshot file exists and is non-empty",
        !png_bytes.is_empty(),
        format!("{} ({} bytes)", shot.path, png_bytes.len()),
    );
    // Content-aware orientation canary: we paused above, so the red
    // "PAUSED" indicator must sit in the top HUD bar. An upside-down frame
    // (a real shipped bug — GL readback is bottom-up) puts it at the
    // bottom and fails this.
    let oriented = tiny_skia::Pixmap::decode_png(&png_bytes)
        .ok()
        .is_some_and(|pixmap| {
            // The HUD bar (holding the red PAUSED indicator) spans the top
            // ~4% of the frame at any dpi scale; scan the top tenth.
            let band = (pixmap.height() as usize / 10).max(32);
            pixmap
                .pixels()
                .iter()
                .take(pixmap.width() as usize * band)
                .any(|px| px.red() > 180 && px.green() < 120 && px.blue() < 120)
        });
    checks.note(
        "screenshot is right side up (PAUSED indicator in top bar)",
        oriented,
        "no red pause indicator found in the top 32 rows",
    );

    // The decisive check: the live session reproduces headless.
    let Reply::Hash(live) = client.call(Request::StateHash)? else {
        bail!("state_hash returned the wrong reply kind");
    };
    let replay_path = std::env::current_dir()?
        .join(format!("replays/smoke-{pid}.json"))
        .to_string_lossy()
        .into_owned();
    client.call(Request::SaveReplay {
        path: replay_path.clone(),
    })?;
    let replay = GameReplay::load(&replay_path).context("reading saved replay")?;
    let replayed = runner::run_replay(&replay, Some(live.tick), false)?;
    checks.note(
        "saved replay reproduces the live session",
        oxide_protocol::hash_hex(replayed.hash()) == live.hash,
        format!(
            "live {} vs replay {}",
            live.hash,
            oxide_protocol::hash_hex(replayed.hash())
        ),
    );

    // Session resume: loading the replay we just saved must land on the
    // same tick and hash, still recording.
    let resumed = client.call(Request::LoadReplay {
        path: replay_path.clone(),
    })?;
    let resumed_ok = matches!(&resumed, Reply::Status(s) if s.tick == live.tick);
    checks.note(
        "load-replay resumes at the recorded tick",
        resumed_ok,
        format!("reply {resumed:?}"),
    );
    let Reply::Hash(after_resume) = client.call(Request::StateHash)? else {
        bail!("state_hash returned the wrong reply kind");
    };
    checks.note(
        "resumed session matches the live hash",
        after_resume.hash == live.hash,
        format!("{} vs {}", after_resume.hash, live.hash),
    );
    Ok(())
}
