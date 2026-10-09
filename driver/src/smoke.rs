//! The smoke test: drive a live shell end to end.
//!
//! Exercises the seams the headless tests cannot (the debug socket, the
//! input funnel, the camera, screenshots, and the session recorder) and
//! checks that the live session reproduces headless: save the replay,
//! re-run it, compare hashes. Run it whenever shell code changes:
//!
//! ```text
//! cargo run -p oxide-driver -- smoke --spawn
//! ```

use crate::client::Client;
use anyhow::{Context, Result, bail};
use chassis::grid::as_index;
use oxide_kit::runner;
use oxide_protocol::{RawEvent, Reply, Request, StateFilter};
use oxide_sim::{Command, PlayerId, UnitId, UnitKind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

struct ScratchHome(PathBuf);

impl ScratchHome {
    fn new() -> Result<Self> {
        loop {
            let id = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("oxide-smoke-{}-{id}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!(
                "warning: could not remove smoke scratch HOME {}: {error}",
                self.0.display()
            );
        }
    }
}

/// Runs the smoke sequence against `addr`, optionally spawning a shell
/// first (killed on exit, pass or fail).
pub fn run(addr: &str, spawn: bool) -> Result<()> {
    let scratch_home = spawn.then(ScratchHome::new).transpose()?;
    let guard = if spawn {
        // Spawn on the port we'll actually connect to, not the default.
        let port = addr
            .rsplit(':')
            .next()
            .context("addr must look like host:port")?;
        let executable = crate::auto::build_shell_executable()?;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut command = std::process::Command::new(executable);
        command
            .args([
                "--debug-server",
                "--automation",
                "--paused",
                // Pin the window size: a large enough window clamps the
                // camera immovable on the smoke's map, which would defeat
                // the minimap-jump check.
                "--window",
                "1280x800",
                "--port",
                port,
            ])
            .env("OXIDE_RESOURCE_ROOT", &root);
        crate::auto::isolate_home(
            &mut command,
            &scratch_home.as_ref().expect("created when spawning").0,
        );
        command.current_dir(&scratch_home.as_ref().expect("created when spawning").0);
        Some(crate::auto::ShellGuard::new(
            command.spawn().context("spawning built Oxide shell")?,
        ))
    } else {
        None
    };
    let outcome = execute(addr, spawn);
    drop(guard);
    // A passing smoke removes its replay and screenshot, which would
    // otherwise accumulate in replays/ where the shell's shelf lists them.
    // A failing smoke keeps both for inspection.
    if outcome.is_ok()
        && let Ok(cwd) = std::env::current_dir()
    {
        // Same roots the checks used when writing them.
        let pid = std::process::id();
        std::fs::remove_file(cwd.join(format!("replays/smoke-{pid}.json"))).ok();
        std::fs::remove_file(cwd.join(format!("screenshots/smoke-{pid}.png"))).ok();
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
    // A spawned native window may need a moment to create its frame loop.
    let mut client = connect_with_retry(addr, if patient { 240 } else { 6 })?;
    if patient {
        // Automation intentionally starts at Home and ignores hardware
        // input. Load the smoke fixture through the same public protocol
        // an agent uses, while retaining the shell's driven-clock flag.
        let scenario = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../scenarios/skirmish.json")
            .to_string_lossy()
            .into_owned();
        let reply = client.call(Request::LoadScenario { path: scenario })?;
        if !matches!(reply, Reply::Ok) {
            bail!("loading the smoke scenario returned {reply:?}");
        }
    }
    let mut checks = Checks {
        passed: 0,
        failures: Vec::new(),
    };

    // Status and clock control. Pause up front: tick-exact assertions
    // against a free-running shell would race its wall clock. The previous
    // pause state is restored on the way out.
    let status = client.status()?;
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

/// The logical screen point of a world position under `cam`, the space
/// injected pointer events speak.
#[expect(
    clippy::cast_possible_truncation,
    reason = "logical screen points need only f32 precision"
)]
fn screen_point(cam: &oxide_protocol::CameraView, wx: f64, wy: f64) -> (f32, f32) {
    let [lo_x, lo_y, hi_x, hi_y] = cam.world_rect;
    (
        ((wx - lo_x) / (hi_x - lo_x) * cam.viewport[0]) as f32,
        ((wy - lo_y) / (hi_y - lo_y) * cam.viewport[1]) as f32,
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "the smoke checks run in order against one client"
)]
fn run_checks(client: &mut Client, checks: &mut Checks) -> Result<()> {
    let start = client.state_hash()?;
    let advanced = client.advance(10)?;
    checks.note(
        "advance_ticks moves exactly N ticks",
        advanced.tick == start.tick + 10,
        format!("{} -> {}", start.tick, advanced.tick),
    );
    let view = client.state(StateFilter::default())?;
    let foreign = view
        .units
        .iter()
        .find(|unit| unit.player != 0)
        .context("no opposing unit for a rejection probe")?
        .id;
    client.call(Request::SendCommand {
        player: PlayerId(0),
        command: Command::Stop {
            units: vec![UnitId(foreign)],
        },
    })?;
    let presented = client.present(1)?;
    let rejected = presented.events.iter().any(|event| {
        matches!(
            event,
            oxide_sim::Event::CommandRejected { player, .. } if *player == PlayerId(0)
        )
    });
    checks.note(
        "present_ticks moves exactly N ticks and returns rejection events",
        presented.ticks == 1 && presented.tick == advanced.tick + 1 && rejected,
        format!(
            "{} -> {} ({} events, rejection {rejected})",
            advanced.tick,
            presented.tick,
            presented.events.len()
        ),
    );

    // Injected wheel input must reach the camera through the real funnel.
    let before = client.camera()?;
    client.call(Request::InjectEvent {
        event: RawEvent::Wheel { delta: 2.0 },
    })?;
    std::thread::sleep(Duration::from_millis(300)); // a few frames
    let after = client.camera()?;
    checks.note(
        "wheel zooms the camera in",
        after.zoom > before.zoom,
        format!("zoom {} -> {}", before.zoom, after.zoom),
    );

    // A game command through the socket, verified in sim state.
    let view = client.state(StateFilter::default())?;
    let mover = view
        .units
        .iter()
        .find(|u| u.player == 0 && u.kind == UnitKind::Harvester)
        .context("no player-0 harvester on the map")?;
    let (id, start_pos) = (mover.id, mover.pos);
    client.call(Request::SendCommand {
        player: PlayerId(0),
        command: Command::Run {
            queue: false,
            units: vec![UnitId(id)],
            goal: chassis::grid::TilePos::new(mover.tile[0] + 3, mover.tile[1] + 2),
        },
    })?;
    client.call(Request::AdvanceTicks { ticks: 60 })?;
    let view = client.state(StateFilter::default())?;
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
        path: Some(shot_path),
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
    // The neutral PAUSED label belongs in the upper-right HUD. Count separated
    // glyph strokes so the minimap's straight border cannot mimic the label.
    let oriented = tiny_skia::Pixmap::decode_png(&png_bytes).is_ok_and(|pixmap| {
        let (width, height) = (pixmap.width() as usize, pixmap.height() as usize);
        let text_rows = |flipped: bool| {
            (height / 100..height * 4 / 100)
                .filter(|&y| {
                    let y = if flipped { height - 1 - y } else { y };
                    let mut previous = false;
                    let mut strokes = 0;
                    for x in width * 80 / 100..width * 99 / 100 {
                        let px = pixmap.pixels()[y * width + x];
                        let low = px.red().min(px.green()).min(px.blue());
                        let high = px.red().max(px.green()).max(px.blue());
                        let bright = low > 150 && high - low < 40;
                        strokes += usize::from(bright && !previous);
                        previous = bright;
                    }
                    strokes >= 3
                })
                .count()
        };
        text_rows(false) >= 3 && text_rows(true) < 3
    });
    checks.note(
        "screenshot is right side up (PAUSED indicator in top bar)",
        oriented,
        "pause-label glyphs missing from the upper-right HUD or present in its vertical reflection",
    );

    // The live session must reproduce headless.
    let live = client.state_hash()?;
    let replay_path = std::env::current_dir()?
        .join(format!("replays/smoke-{pid}.json"))
        .to_string_lossy()
        .into_owned();
    client.call(Request::SaveReplay {
        path: replay_path.clone(),
    })?;
    let replay = oxide_kit::load_replay(&replay_path).context("reading saved replay")?;
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

    // Run the original session, with its bots' live memory, past the save
    // point before any reload.
    client.call(Request::AdvanceTicks { ticks: 200 })?;
    let future_live = client.state_hash()?;

    // Session resume: loading the replay we just saved must land on the
    // same tick and hash, still recording.
    let resumed = client.call(Request::LoadReplay { path: replay_path })?;
    let resumed_ok = matches!(&resumed, Reply::Status(s) if s.tick == live.tick);
    checks.note(
        "load-replay resumes at the recorded tick",
        resumed_ok,
        format!("reply {resumed:?}"),
    );
    let after_resume = client.state_hash()?;
    checks.note(
        "resumed session matches the live hash",
        after_resume.hash == live.hash,
        format!("{} vs {}", after_resume.hash, live.hash),
    );

    // A resumed session must continue as the unsaved one did, including
    // the bots, whose memory is rebuilt during the fast-forward.
    client.call(Request::AdvanceTicks { ticks: 200 })?;
    let future_resumed = client.state_hash()?;
    checks.note(
        "resumed session continues identically to the unsaved one",
        future_resumed.hash == future_live.hash,
        format!("{} vs {}", future_resumed.hash, future_live.hash),
    );

    // Armed placement must not misread a minimap click as world ground.
    // Through the real funnel: select a harvester, arm a turret through
    // the palette, click the minimap; the camera must jump and the bank
    // must not move. Reset to a fresh skirmish first: the replay checks
    // above left the world hundreds of ticks in, where wandering machines
    // can sit on any tile this block would target.
    let scenario_path = std::env::current_dir()?.join("scenarios/skirmish.json");
    client.call(Request::LoadScenario {
        path: scenario_path.to_string_lossy().into_owned(),
    })?;
    std::thread::sleep(Duration::from_millis(200));
    let view = client.state(StateFilter {
        map: true,
        ..StateFilter::default()
    })?;
    let scrap_before = view.players[0].scrap;
    let rows = view.map.as_ref().context("asked for the map")?;
    let harvester = view
        .units
        .iter()
        .find(|u| u.player == 0 && u.kind == UnitKind::Harvester)
        .context("no harvester to select")?;
    let cam = client.camera()?;
    let (hx, hy) = screen_point(&cam, harvester.pos[0], harvester.pos[1]);
    for event in [
        RawEvent::MouseDown {
            button: oxide_protocol::MouseButton::Left,
            x: hx,
            y: hy,
        },
        RawEvent::MouseUp {
            button: oxide_protocol::MouseButton::Left,
            x: hx,
            y: hy,
        },
        RawEvent::KeyDown {
            key: oxide_protocol::Key::B,
        },
        RawEvent::KeyUp {
            key: oxide_protocol::Key::B,
        },
        // B opens construction; R selects Defense, Q arms a Turret.
        // Without it the whole check would pass vacuously against an
        // unarmed cursor.
        RawEvent::KeyDown {
            key: oxide_protocol::Key::R,
        },
        RawEvent::KeyUp {
            key: oxide_protocol::Key::R,
        },
        RawEvent::KeyDown {
            key: oxide_protocol::Key::Q,
        },
        RawEvent::KeyUp {
            key: oxide_protocol::Key::Q,
        },
    ] {
        client.call(Request::InjectEvent { event })?;
    }
    std::thread::sleep(Duration::from_millis(200));
    // The minimap rect comes from QueryUi's chrome, the same layout the
    // shell's hit-testing reads.
    let ui = client.ui()?;
    let Some(chrome) = ui.chrome else {
        bail!("playing mode reports chrome geometry");
    };
    let (mm_x, mm_y, mm_w, mm_h) = (
        f64::from(chrome[2]),
        f64::from(chrome[3]),
        f64::from(chrome[4]),
        f64::from(chrome[5]),
    );
    #[expect(
        clippy::cast_possible_truncation,
        reason = "logical screen points need only f32 precision"
    )]
    let (cx, cy) = ((mm_x + mm_w * 0.8) as f32, (mm_y + mm_h * 0.8) as f32);
    for event in [
        RawEvent::MouseDown {
            button: oxide_protocol::MouseButton::Left,
            x: cx,
            y: cy,
        },
        RawEvent::MouseUp {
            button: oxide_protocol::MouseButton::Left,
            x: cx,
            y: cy,
        },
    ] {
        client.call(Request::InjectEvent { event })?;
    }
    std::thread::sleep(Duration::from_millis(200));
    let cam_after = client.camera()?;
    let view_after = client.state(StateFilter::default())?;
    checks.note(
        "minimap click while placing jumps the camera and spends nothing",
        view_after.players[0].scrap == scrap_before && cam_after.center != cam.center,
        format!(
            "scrap {} -> {}, center {:?} -> {:?}",
            scrap_before, view_after.players[0].scrap, cam.center, cam_after.center
        ),
    );
    // Confirm the palette armed a turret: commit the build on visible open
    // ground and watch the scrap move. The minimap jump left the camera
    // over fog, where never-explored ground refuses and remembered ground
    // defers its charge, so recenter home first and aim beside the
    // Foundry, inside its vision, where the charge is immediate.
    for event in [
        RawEvent::KeyDown {
            key: oxide_protocol::Key::Space,
        },
        RawEvent::KeyUp {
            key: oxide_protocol::Key::Space,
        },
    ] {
        client.call(Request::InjectEvent { event })?;
    }
    std::thread::sleep(Duration::from_millis(100));
    let cam2 = client.camera()?;
    // Open ground near the Foundry, inside its vision, clear of every
    // unit's tile.
    let foundry = view
        .buildings
        .iter()
        .find(|b| b.player == 0)
        .context("no own foundry in view")?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "unit positions lie on the map, well inside i32"
    )]
    let occupied: Vec<(i32, i32)> = view
        .units
        .iter()
        .map(|u| (u.pos[0].floor() as i32, u.pos[1].floor() as i32))
        .collect();
    let (fx, fy) = (foundry.anchor[0], foundry.anchor[1]);
    let mut commit: Option<(i32, i32)> = None;
    'scan: for r in 2..6i32 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let (tx, ty) = (fx + dx, fy + dy);
                let open = rows
                    .get(as_index(ty))
                    .and_then(|row| row.chars().nth(as_index(tx)))
                    .is_some_and(|c| c == '.' || c == ',');
                if open && !occupied.contains(&(tx, ty)) {
                    commit = Some((tx, ty));
                    break 'scan;
                }
            }
        }
    }
    let (tx, ty) = commit.context("no open tile near the foundry")?;
    let (bx, by) = screen_point(&cam2, f64::from(tx) + 0.5, f64::from(ty) + 0.5);
    for event in [
        RawEvent::MouseDown {
            button: oxide_protocol::MouseButton::Left,
            x: bx,
            y: by,
        },
        RawEvent::MouseUp {
            button: oxide_protocol::MouseButton::Left,
            x: bx,
            y: by,
        },
    ] {
        client.call(Request::InjectEvent { event })?;
    }
    std::thread::sleep(Duration::from_millis(200));
    // The shell is paused: issued commands stage for the NEXT tick, so
    // the world only reflects the click once the sim advances.
    client.call(Request::AdvanceTicks { ticks: 1 })?;
    let view_committed = client.state(StateFilter::default())?;
    let turret_cost = 100;
    let spent = scrap_before.saturating_sub(view_committed.players[0].scrap);
    let site_up = view_committed
        .buildings
        .iter()
        .any(|b| b.kind == oxide_sim::BuildingKind::Turret && b.player == 0);
    checks.note(
        "armed palette placement commits a turret site on click",
        spent == turret_cost && site_up,
        format!(
            "scrap {} -> {} (want -{turret_cost}), turret site present: {site_up}",
            scrap_before, view_committed.players[0].scrap
        ),
    );
    // Disarm anything lingering.
    for event in [
        RawEvent::KeyDown {
            key: oxide_protocol::Key::Escape,
        },
        RawEvent::KeyUp {
            key: oxide_protocol::Key::Escape,
        },
    ] {
        client.call(Request::InjectEvent { event })?;
    }
    Ok(())
}
