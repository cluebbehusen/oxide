//! HAR-01's acceptance criterion: the windowless session serves the
//! debug protocol exactly as a live shell does.
//!
//! The headless half runs in CI: the session over real TCP through the
//! real `Client`, plus the save-is-a-replay proof (the session's own
//! saved record re-executes to its live hash). The live half — the same
//! script driven through a spawned shell window and the headless session
//! side by side, asserting reply-for-reply identity — needs a native
//! window and runs with the #[ignore]d battery:
//!
//! ```text
//! cargo test -p oxide-driver --test session_parity -- --ignored
//! ```

use anyhow::{Context, Result, bail};
use oxide_driver::client::Client;
use oxide_driver::runner;
use oxide_driver::session::{Session, serve_listener};
use oxide_protocol::framing::Limits;
use oxide_protocol::{Reply, Request, StateFilter, hash_hex};
use oxide_sim::{Command, PlayerId, Scenario};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

/// A private scratch directory, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let id = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("oxide-session-parity-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create scratch dir");
        Self(path)
    }

    fn path(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// Serves a fresh skirmish session on an ephemeral port and connects.
fn session_client() -> Result<Client> {
    let session = Session::new(Scenario::skirmish())?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("bind loopback")?;
    let addr = listener.local_addr()?;
    std::thread::spawn(move || serve_listener(listener, Limits::default(), session));
    Client::connect(&addr.to_string())
}

/// A deterministic little match script: put the first two of `player`'s
/// units on the march. Unit ids come from the state itself so the script
/// survives roster edits.
fn march_command(client: &mut Client, player: u8) -> Result<Command> {
    let Reply::State(view) = client.call(Request::QueryState {
        filter: StateFilter::default(),
    })?
    else {
        bail!("expected a state reply");
    };
    let units: Vec<oxide_sim::UnitId> = view
        .units
        .iter()
        .filter(|u| u.player == player)
        .take(2)
        .map(|u| oxide_sim::UnitId(u.id))
        .collect();
    assert!(!units.is_empty(), "the seat has units to command");
    Ok(Command::AttackMove {
        units,
        goal: chassis::grid::TilePos::new(12, 9),
        queue: false,
    })
}

#[test]
fn the_session_advances_exactly_like_the_canonical_runner() {
    let scenario = Scenario::skirmish();
    let mut session = Session::new(scenario.clone()).expect("build session");
    let Reply::Advanced(view) = session
        .handle(Request::AdvanceTicks { ticks: 1500 })
        .expect("advance")
    else {
        panic!("expected an advanced reply");
    };
    let outcome = runner::run_scenario(&scenario, 1500, true, false).expect("run scenario");
    assert_eq!(view.tick, outcome.state.current_tick());
    assert_eq!(
        view.hash,
        hash_hex(outcome.state.hash()),
        "an undirected session IS the canonical runner composition"
    );
}

#[test]
fn every_live_verb_works_headless_over_tcp_and_the_record_reproduces() -> Result<()> {
    let scratch = Scratch::new();
    let mut client = session_client()?;

    // Status: always driven mode, honestly reported.
    let Reply::Status(status) = client.call(Request::Status)? else {
        bail!("expected a status reply");
    };
    assert_eq!(status.tick, 0);
    assert!(status.paused, "a headless session is always in driven mode");
    assert_eq!(status.scenario, "Skirmish Basin");

    // Commands stage for the next tick, exactly like a paused shell.
    let command = march_command(&mut client, 0)?;
    client.call(Request::SendCommand {
        player: PlayerId(0),
        command,
    })?;

    let Reply::Advanced(advanced) = client.call(Request::AdvanceTicks { ticks: 300 })? else {
        bail!("expected an advanced reply");
    };
    assert_eq!(advanced.tick, 300);

    let Reply::Presented(presented) = client.call(Request::PresentTicks { ticks: 5 })? else {
        bail!("expected a presented reply");
    };
    assert_eq!(presented.tick, 305);

    // The omniscient QA view and the fog-honest player view, side by side.
    let Reply::State(state) = client.call(Request::QueryState {
        filter: StateFilter {
            map: true,
            ..StateFilter::default()
        },
    })?
    else {
        bail!("expected a state reply");
    };
    assert!(state.map.is_some());
    let Reply::Fog(fog) = client.call(Request::QueryFogView {
        player: PlayerId(0),
    })?
    else {
        bail!("expected a fog reply");
    };
    assert_eq!(fog.player, 0);
    assert!(
        fog.units.len() <= state.units.len(),
        "the fog view never reports more than the omniscient one"
    );
    let missing = client
        .call(Request::QueryFogView {
            player: PlayerId(9),
        })
        .expect_err("seat 9 does not exist");
    assert!(missing.to_string().contains("no such player"));

    // CPU screenshot: a real PNG, labeled with its renderer.
    let shot_path = scratch.path("shot.png");
    let Reply::Screenshot(shot) = client.call(Request::Screenshot {
        path: Some(shot_path.clone()),
    })?
    else {
        bail!("expected a screenshot reply");
    };
    assert_eq!(shot.renderer, "cpu");
    assert!(shot.width > 0 && shot.height > 0);
    assert!(std::path::Path::new(&shot_path).exists());

    // Save-is-a-replay: the session's record re-executes to the hash the
    // session itself reports — the drift tripwire the live shell's smoke
    // test runs, now headless in CI.
    let replay_path = scratch.path("session.json");
    let Reply::Saved(saved) = client.call(Request::SaveReplay {
        path: replay_path.clone(),
    })?
    else {
        bail!("expected a saved reply");
    };
    assert!(saved.commands > 0, "the march and the bot were recorded");
    let Reply::Hash(live) = client.call(Request::StateHash)? else {
        bail!("expected a hash reply");
    };
    let replay = oxide_kit::load_replay(&replay_path)?;
    let replayed = runner::run_replay(&replay, None, false)?;
    assert_eq!(
        hash_hex(replayed.hash()),
        live.hash,
        "the saved record must reproduce the live session"
    );

    // Loading that record back is resuming: same tick, same hash.
    let Reply::Status(resumed) = client.call(Request::LoadReplay { path: replay_path })? else {
        bail!("expected a status reply after a replay load");
    };
    assert_eq!(resumed.tick, 305);
    let Reply::Hash(reloaded) = client.call(Request::StateHash)? else {
        bail!("expected a hash reply");
    };
    assert_eq!(reloaded.hash, live.hash);

    // Every windowed or wall-clock verb is refused in words, never
    // silently acknowledged.
    for request in [
        Request::Pause,
        Request::Resume,
        Request::SetSpeed { multiplier: 2.0 },
        Request::QueryCamera,
        Request::QueryUi,
        Request::QueryPerformance { reset: false },
        Request::BeginPerformanceWindow {
            from_tick: 305,
            to_tick: 405,
        },
        Request::ToggleOverlay,
        Request::InjectEvent {
            event: oxide_protocol::RawEvent::KeyDown {
                key: oxide_protocol::Key::Space,
            },
        },
    ] {
        let refusal = client
            .call(request.clone())
            .expect_err(&format!("{request:?} must be refused headless"));
        assert!(
            refusal.to_string().contains("headless session"),
            "{request:?} refusal must say why: {refusal}"
        );
    }
    Ok(())
}

/// The live half: one script, two servers, reply-for-reply identity.
/// Needs a native window (spawns a real shell), so it runs with the
/// #[ignore]d battery, never in CI.
#[test]
#[ignore]
fn the_live_shell_and_the_headless_session_answer_identically() -> Result<()> {
    let scratch = Scratch::new();
    let scenario_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../scenarios/skirmish.json")
        .canonicalize()
        .context("locating the skirmish scenario")?
        .to_string_lossy()
        .into_owned();

    let (_guard, mut shell) = oxide_driver::auto::spawn_shell(&oxide_driver::auto::SpawnOptions {
        port: 4177,
        paused: true,
        home: Some(scratch.0.clone()),
    })?;
    let mut headless = session_client()?;

    // The same scenario file through the same verb on both servers.
    for client in [&mut shell, &mut headless] {
        client.call(Request::LoadScenario {
            path: scenario_path.clone(),
        })?;
    }

    let call_both = |shell: &mut Client, headless: &mut Client, request: Request| -> Result<()> {
        let a = shell.call(request.clone())?;
        let b = headless.call(request.clone())?;
        anyhow::ensure!(
            a == b,
            "replies diverged on {request:?}:\n live: {a:?}\n headless: {b:?}"
        );
        Ok(())
    };

    // Identity on the full reply, not just the hash: status, omniscient
    // state, fog view, presented events — the whole surface both serve.
    call_both(&mut shell, &mut headless, Request::Status)?;
    call_both(
        &mut shell,
        &mut headless,
        Request::QueryState {
            filter: StateFilter {
                map: true,
                ..StateFilter::default()
            },
        },
    )?;

    let command = march_command(&mut shell, 0)?;
    for client in [&mut shell, &mut headless] {
        client.call(Request::SendCommand {
            player: PlayerId(0),
            command: command.clone(),
        })?;
    }

    // Interleave presented steps (event identity) with bulk advances
    // (hash identity) deep enough for combat and economy to run.
    call_both(
        &mut shell,
        &mut headless,
        Request::PresentTicks { ticks: 10 },
    )?;
    call_both(
        &mut shell,
        &mut headless,
        Request::AdvanceTicks { ticks: 500 },
    )?;
    call_both(
        &mut shell,
        &mut headless,
        Request::QueryFogView {
            player: PlayerId(0),
        },
    )?;
    call_both(
        &mut shell,
        &mut headless,
        Request::PresentTicks { ticks: 20 },
    )?;
    call_both(
        &mut shell,
        &mut headless,
        Request::AdvanceTicks { ticks: 1500 },
    )?;
    call_both(&mut shell, &mut headless, Request::StateHash)?;
    call_both(&mut shell, &mut headless, Request::Status)?;
    call_both(
        &mut shell,
        &mut headless,
        Request::QueryState {
            filter: StateFilter {
                map: true,
                ..StateFilter::default()
            },
        },
    )?;
    Ok(())
}
