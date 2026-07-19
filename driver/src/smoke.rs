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
        Some(
            std::process::Command::new("cargo")
                .args([
                    "run",
                    "-p",
                    "oxide-shell",
                    "--",
                    "--debug-server",
                    "--paused",
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

    // Status and clock control.
    let Reply::Status(status) = client.call(Request::Status)? else {
        bail!("status returned the wrong reply kind");
    };
    checks.note(
        "status responds",
        !status.scenario.is_empty(),
        "empty scenario name",
    );
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

    // Screenshot lands on disk with sane dimensions.
    let Reply::Screenshot(shot) = client.call(Request::Screenshot { path: None })? else {
        bail!("screenshot returned the wrong reply kind");
    };
    checks.note(
        "screenshot reports dimensions",
        shot.width > 0 && shot.height > 0,
        format!("{}x{}", shot.width, shot.height),
    );
    let on_disk = std::fs::metadata(&shot.path).map(|m| m.len()).unwrap_or(0);
    checks.note(
        "screenshot file exists and is non-empty",
        on_disk > 0,
        format!("{} ({} bytes)", shot.path, on_disk),
    );

    // The decisive check: the live session reproduces headless.
    let Reply::Hash(live) = client.call(Request::StateHash)? else {
        bail!("state_hash returned the wrong reply kind");
    };
    let replay_path = "replays/smoke.json";
    client.call(Request::SaveReplay {
        path: replay_path.into(),
    })?;
    let replay = GameReplay::load(replay_path).context("reading saved replay")?;
    let replayed = runner::run_replay(&replay, Some(live.tick))?;
    checks.note(
        "saved replay reproduces the live session",
        oxide_protocol::hash_hex(replayed.hash()) == live.hash,
        format!(
            "live {} vs replay {}",
            live.hash,
            oxide_protocol::hash_hex(replayed.hash())
        ),
    );

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
