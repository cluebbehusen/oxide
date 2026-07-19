//! The Oxide shell: a thin macroquad window over the deterministic sim.
//!
//! Frame order is fixed and matters:
//!
//! 1. drain debug-socket requests (screenshots defer to post-render),
//! 2. gather input events — polled hardware first, then injected — and run
//!    them through the one input mapper,
//! 3. advance the sim by wall clock (unless paused; `advance_ticks` from the
//!    socket bypasses the clock entirely — that's driven mode),
//! 4. render with interpolation,
//! 5. capture any requested screenshot from the finished frame.
//!
//! Nothing in this crate may affect game outcomes except by staging
//! tick-stamped commands. If a feature can't be expressed that way, it
//! belongs in the sim.

mod assets;
mod camera;
mod debug_server;
mod game;
mod input;
mod render;

use anyhow::{Context, Result};
use clap::Parser;
use debug_server::IncomingRequest;
use game::Game;
use macroquad::prelude::*;
use oxide_protocol::{
    AdvancedView, CameraView, HashView, OverlayView, RawEvent, Reply, Request, ResponseEnvelope,
    SavedView, ScreenshotView, StateView, StatusView,
};
use oxide_sim::{PlayerCommand, SIM_VERSION, Scenario};
use std::sync::mpsc::{Receiver, Sender};

#[derive(Parser)]
#[command(name = "oxide-shell", version, about = "Oxide, playable")]
struct Args {
    /// Scenario JSON path (default: the built-in skirmish).
    #[arg(long)]
    scenario: Option<String>,

    /// Serve the debug protocol on --port.
    #[arg(long)]
    debug_server: bool,

    /// Debug server port.
    #[arg(long, default_value_t = oxide_protocol::DEFAULT_PORT)]
    port: u16,

    /// Start with the sim clock stopped: time advances only via the debug
    /// socket (driven mode). Rendering still runs.
    #[arg(long)]
    paused: bool,

    /// Wall-clock speed multiplier.
    #[arg(long, default_value_t = 1.0)]
    speed: f64,
}

fn window_conf() -> Conf {
    Conf {
        window_title: "Oxide".to_string(),
        window_width: 1280,
        window_height: 800,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("fatal: {err:#}");
        std::process::exit(1);
    }
}

/// A screenshot request parked until after this frame renders.
struct PendingScreenshot {
    id: u64,
    path: String,
    reply: Sender<ResponseEnvelope>,
}

async fn run() -> Result<()> {
    let args = Args::parse();
    let scenario = match &args.scenario {
        Some(path) => Scenario::load(path).with_context(|| format!("loading {path}"))?,
        None => Scenario::skirmish(),
    };
    let sprites = assets::Sprites::load().await?;
    let mut game = Game::new(scenario)?;
    game.paused = args.paused;
    game.speed = args.speed;

    let debug_rx: Option<Receiver<IncomingRequest>> = if args.debug_server {
        Some(debug_server::spawn(args.port)?)
    } else {
        None
    };
    let mut input = input::InputState::new();
    let mut injected: Vec<RawEvent> = Vec::new();
    let mut pending_shot: Option<PendingScreenshot> = None;

    loop {
        let dt = get_frame_time();

        if let Some(rx) = &debug_rx {
            while let Ok(incoming) = rx.try_recv() {
                handle_request(incoming, &mut game, &mut injected, &mut pending_shot);
            }
        }

        let mut events = input::poll_events(&input);
        events.append(&mut injected);
        input::apply_events(&mut game, &mut input, &events);
        input::update_held(&mut game, &input, dt);

        game.advance_wall_clock(dt);
        game.update_fx(dt);
        render::draw(&game, &sprites, &input);

        if let Some(shot) = pending_shot.take() {
            let response = match capture_screenshot(&shot.path) {
                Ok((width, height)) => ResponseEnvelope::ok(
                    shot.id,
                    Reply::Screenshot(ScreenshotView {
                        path: shot.path,
                        width,
                        height,
                    }),
                ),
                Err(err) => ResponseEnvelope::err(shot.id, format!("screenshot: {err:#}")),
            };
            shot.reply.send(response).ok();
        }
        next_frame().await;
    }
}

fn capture_screenshot(path: &str) -> Result<(u32, u32)> {
    if let Some(parent) = std::path::Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let image = get_screen_data();
    image.export_png(path);
    Ok((u32::from(image.width), u32::from(image.height)))
}

/// Answers one debug request. Screenshots are parked; everything else
/// responds immediately, between frames, against a settled world.
fn handle_request(
    incoming: IncomingRequest,
    game: &mut Game,
    injected: &mut Vec<RawEvent>,
    pending_shot: &mut Option<PendingScreenshot>,
) {
    let IncomingRequest { id, request, reply } = incoming;
    let outcome: Result<Reply, String> = match request {
        Request::Status => Ok(Reply::Status(StatusView {
            tick: game.state.tick,
            paused: game.paused,
            speed: game.speed,
            scenario: game.scenario.name.clone(),
            sim_version: SIM_VERSION.to_string(),
            result: game.state.result,
            recorded_commands: game.recorder.commands.len(),
        })),
        Request::QueryState { filter } => Ok(Reply::State(StateView::capture(&game.state, filter))),
        Request::QueryCamera => {
            let (lo, hi) = game.camera.world_rect();
            Ok(Reply::Camera(CameraView {
                center: [
                    f64::from(game.camera.center.x),
                    f64::from(game.camera.center.y),
                ],
                zoom: f64::from(game.camera.zoom),
                viewport: [f64::from(screen_width()), f64::from(screen_height())],
                world_rect: [
                    f64::from(lo.x),
                    f64::from(lo.y),
                    f64::from(hi.x),
                    f64::from(hi.y),
                ],
            }))
        }
        Request::StateHash => Ok(Reply::Hash(HashView {
            tick: game.state.tick,
            hash: game.hash_hex(),
        })),
        Request::AdvanceTicks { ticks } => {
            let ticks = ticks.min(1_000_000);
            game.advance_ticks(ticks);
            Ok(Reply::Advanced(AdvancedView {
                ticks,
                tick: game.state.tick,
                hash: game.hash_hex(),
            }))
        }
        Request::Pause => {
            game.paused = true;
            Ok(Reply::Ok)
        }
        Request::Resume => {
            game.paused = false;
            Ok(Reply::Ok)
        }
        Request::SetSpeed { multiplier } => {
            if multiplier.is_finite() && (0.05..=64.0).contains(&multiplier) {
                game.speed = multiplier;
                Ok(Reply::Ok)
            } else {
                Err(format!("speed multiplier {multiplier} outside 0.05..=64"))
            }
        }
        Request::SendCommand { player, command } => {
            if (player.0 as usize) < game.state.players.len() {
                game.pending.push(PlayerCommand { player, command });
                Ok(Reply::Ok)
            } else {
                Err(format!("no such player {player}"))
            }
        }
        Request::InjectEvent { event } => {
            injected.push(event);
            Ok(Reply::Ok)
        }
        Request::Screenshot { path } => {
            let path = path.unwrap_or_else(|| format!("screenshots/tick-{}.png", game.state.tick));
            *pending_shot = Some(PendingScreenshot { id, path, reply });
            return; // responds after the frame renders
        }
        Request::ToggleOverlay => {
            game.overlay = !game.overlay;
            Ok(Reply::Overlay(OverlayView {
                enabled: game.overlay,
            }))
        }
        Request::LoadScenario { path } => Scenario::load(&path)
            .map_err(|err| format!("loading {path}: {err}"))
            .and_then(|scenario| {
                Game::new(scenario).map_err(|err| format!("building scenario: {err:#}"))
            })
            .map(|mut fresh| {
                fresh.paused = game.paused;
                fresh.speed = game.speed;
                fresh.overlay = game.overlay;
                *game = fresh;
                Reply::Ok
            }),
        Request::SaveReplay { path } => {
            game.recorder.meta.ticks = Some(game.state.tick);
            let parent = std::path::Path::new(&path).parent();
            if let Some(parent) = parent
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent).ok();
            }
            match game.recorder.save(&path) {
                Ok(()) => Ok(Reply::Saved(SavedView {
                    path,
                    commands: game.recorder.commands.len(),
                })),
                Err(err) => Err(format!("saving replay: {err}")),
            }
        }
    };
    let response = match outcome {
        Ok(ok) => ResponseEnvelope::ok(id, ok),
        Err(err) => ResponseEnvelope::err(id, err),
    };
    reply.send(response).ok();
}
