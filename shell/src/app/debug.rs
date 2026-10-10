//! Answers debug-socket requests against whichever session owns the screen.

use super::screenshot::PendingScreenshot;
use super::{App, Install, Screen, ScreenKind};
use crate::debug_server::IncomingRequest;
use crate::game::Game;
use macroquad::prelude::{screen_height, screen_width};
use oxide_protocol::{
    CameraView, OverlayView, Reply, Request, ResponseEnvelope, SavedView, UiView,
};
use oxide_sim::{PlayerCommand, Scenario};

/// The mutating verbs a read-only viewer refuses: commands and session
/// swaps would act through the replay onto the hidden match behind it.
pub(super) fn viewer_refuses(request: &Request) -> bool {
    matches!(
        request,
        Request::BeginPerformanceWindow { .. }
            | Request::SendCommand { .. }
            | Request::LoadScenario { .. }
            | Request::LoadReplay { .. }
            | Request::SaveReplay { .. }
    )
}

pub(super) fn frozen_map_refuses(request: &Request) -> bool {
    viewer_refuses(request)
        || matches!(
            request,
            Request::AdvanceTicks { .. }
                | Request::PresentTicks { .. }
                | Request::Resume
                | Request::SetSpeed { .. }
        )
}

/// Whether a debug request is refused by a local guard or answered,
/// decided before any state is touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Route {
    /// The final battlefield is frozen; time and session verbs bounce.
    RefuseFrozen,
    /// A local mutating verb while the read-only viewer owns the screen.
    RefuseViewer,
    /// A verb a LAN match cannot honor on this machine.
    RefuseLockstep,
    /// A locally-answered verb against the app's own state.
    Local,
}

/// The local routing guards around shared protocol dispatch. Frozen-map
/// and lockstep refusal run before dispatch; the viewer's read-only guard
/// runs after a request proves not to be shared. `net` is the live match's
/// role and bound seat when it is a LAN match.
pub(super) fn route(
    playback: bool,
    final_map: bool,
    net: Option<(crate::game::network::NetRole, oxide_sim::PlayerId)>,
    request: &Request,
) -> Route {
    if final_map && frozen_map_refuses(request) {
        return Route::RefuseFrozen;
    }
    if !playback && net.is_some_and(|(role, seat)| lockstep_refuses(role, seat, request)) {
        return Route::RefuseLockstep;
    }
    if playback && viewer_refuses(request) {
        return Route::RefuseViewer;
    }
    Route::Local
}

/// What a LAN match refuses: its clock, speed, and roster belong to the
/// session, only the host pauses, and a machine speaks only for its seat.
pub(super) fn lockstep_refuses(
    role: crate::game::network::NetRole,
    seat: oxide_sim::PlayerId,
    request: &Request,
) -> bool {
    matches!(
        request,
        Request::AdvanceTicks { .. }
            | Request::PresentTicks { .. }
            | Request::SetSpeed { .. }
            | Request::LoadScenario { .. }
            | Request::LoadReplay { .. }
            | Request::BeginPerformanceWindow { .. }
    ) || (role == crate::game::network::NetRole::Client
        && matches!(request, Request::Pause | Request::Resume))
        || matches!(request, Request::SendCommand { player, .. } if *player != seat)
}

/// Answers one debug request. Screenshots are parked; everything else
/// responds immediately, between frames, against a settled world.
///
/// Shared requests (state reads, the driven clock) go through
/// [`oxide_protocol::dispatch_shared`] against whichever session owns
/// the screen: the replay viewer when it is up, the live game otherwise.
/// The rest are window-shaped (camera, UI, input, screenshots, the
/// overlay) and answered for the screen the window shows, or
/// live-mutating (commands, loads, saves) and refused while the viewer
/// owns the screen.
pub(super) fn handle_request(
    incoming: IncomingRequest,
    app: &mut App,
    screen: &mut Screen,
    ui_view: &UiView,
) {
    let IncomingRequest { id, request, reply } = incoming;
    if screen.kind() == ScreenKind::Busy && frozen_map_refuses(&request) {
        reply
            .send(ResponseEnvelope::err(
                id,
                "a save or load owns the session; wait for it to finish",
            ))
            .ok();
        return;
    }
    let playback = screen.kind() == ScreenKind::Playback;
    let final_map = screen.kind() == ScreenKind::FinalMap;
    let net = app
        .game
        .net_role()
        .map(|role| (role, app.game.presentation.human));
    match route(playback, final_map, net, &request) {
        Route::RefuseFrozen => {
            reply
                .send(ResponseEnvelope::err(
                    id,
                    "the final battlefield is frozen; return to the report first".to_string(),
                ))
                .ok();
            return;
        }
        Route::RefuseLockstep => {
            reply
                .send(ResponseEnvelope::err(
                    id,
                    "a LAN match owns its clock and seats; this machine cannot do that",
                ))
                .ok();
            return;
        }
        Route::RefuseViewer | Route::Local => {}
    }
    // A viewer-bound `AdvanceTicks` sent to the hidden live match would
    // advance it silently.
    let shared = oxide_protocol::dispatch_shared(screen.visible_session(&mut app.game), &request);
    if let Some(outcome) = shared {
        // Resuming implies gameplay: leave the pause menu too (including
        // Settings or the Codex opened over it), or the sim runs behind a
        // menu that still claims it is paused. This is a screen
        // transition, so it lives here rather than in the session trait.
        if matches!(request, Request::Resume) && outcome.is_ok() && screen.over_pause() {
            *screen = Screen::Playing;
        }
        let envelope = match outcome {
            Ok(inner) => ResponseEnvelope::ok(id, inner),
            Err(error) => ResponseEnvelope::err(id, error),
        };
        reply.send(envelope).ok();
        return;
    }
    // The viewer is read-only: refusing beats acknowledging a request
    // that would silently mutate the hidden match.
    if route(playback, final_map, net, &request) == Route::RefuseViewer {
        let refusal = "the viewer is read-only; leave playback first".to_string();
        reply.send(ResponseEnvelope::err(id, refusal)).ok();
        return;
    }
    let game = &mut app.game;
    let outcome: Result<Reply, String> =
        match request {
            Request::QueryCamera => {
                // Window-shaped answers describe the screen the window
                // shows — the viewer's render vehicle during playback.
                let game = &screen.visible(game).presentation().camera;
                let (lo, hi) = game.world_rect();
                Ok(Reply::Camera(CameraView {
                    center: [f64::from(game.center.x), f64::from(game.center.y)],
                    zoom: f64::from(game.zoom),
                    viewport: [f64::from(screen_width()), f64::from(screen_height())],
                    world_rect: [
                        f64::from(lo.x),
                        f64::from(lo.y),
                        f64::from(hi.x),
                        f64::from(hi.y),
                    ],
                }))
            }
            Request::QueryUi => Ok(Reply::Ui(ui_view.clone())),
            Request::QueryPerformance { reset } => {
                if app.frame_profiler.enabled() {
                    Ok(Reply::Performance(app.frame_profiler.snapshot(reset)))
                } else {
                    Err("native frame profiling is disabled; launch the shell with --profile-frames"
                    .to_string())
                }
            }
            Request::BeginPerformanceWindow { from_tick, to_tick } => {
                if screen.kind() != ScreenKind::Playing {
                    Err("exact frame windows require the live Playing screen".to_string())
                } else if !game.clock.paused {
                    Err("pause the live match before arming a frame window".to_string())
                } else if game.state.current_tick() != from_tick {
                    Err(format!(
                        "profile window starts at tick {from_tick}, but the live match is at {}",
                        game.state.current_tick()
                    ))
                } else {
                    app.frame_profiler
                        .arm(from_tick, to_tick)
                        .map(|()| Reply::Ok)
                }
            }
            Request::ToggleOverlay => {
                let game = screen.visible_presentation(game);
                game.overlay = !game.overlay;
                Ok(Reply::Overlay(OverlayView {
                    enabled: game.overlay,
                }))
            }
            Request::SendCommand { player, command } => {
                if (player.0 as usize) < game.state.players().len() {
                    game.stage(PlayerCommand { player, command });
                    Ok(Reply::Ok)
                } else {
                    Err(format!("no such player {player}"))
                }
            }
            Request::InjectEvent { event } => {
                // The hardware funnel admits only printable ASCII into Text
                // (`input::PointerStream::char_event`); injected events honor
                // the same contract, so a control byte or non-ASCII char is
                // refused rather than persisted into a save name the font
                // cannot draw.
                if let oxide_protocol::RawEvent::Text { ch } = event
                    && !('\u{20}'..='\u{7e}').contains(&ch)
                {
                    Err(format!(
                        "text event {ch:?} is outside printable ASCII; the funnel refuses it"
                    ))
                } else {
                    app.injected.push(event);
                    Ok(Reply::Ok)
                }
            }
            Request::Screenshot { path } => {
                // The default name carries the tick of the world the frame
                // will actually show — the replayed one during playback.
                let shown_tick = screen.visible(game).state().current_tick();
                let path = path.unwrap_or_else(|| format!("screenshots/tick-{shown_tick}.png"));
                app.pending_shots
                    .push(PendingScreenshot { id, path, reply });
                return; // responds after the frame renders
            }
            Request::LoadScenario { path } => Scenario::load(&path)
                .map_err(|err| format!("loading {path}: {err}"))
                .and_then(|scenario| {
                    Game::new(scenario).map_err(|err| format!("building scenario: {err:#}"))
                })
                .map(|fresh| {
                    let paused = app.game.clock.paused;
                    app.install(fresh, Install::local(paused));
                    *screen = Screen::Playing;
                    Reply::Ok
                }),
            Request::LoadReplay { path } => oxide_kit::load_replay(&path)
                .map_err(|err| format!("loading replay {path}: {err}"))
                .and_then(|replay| {
                    Game::from_replay(replay).map_err(|err| format!("resuming replay: {err:#}"))
                })
                .map(|fresh| {
                    let paused = app.game.clock.paused;
                    app.install(fresh, Install::local(paused));
                    *screen = Screen::Playing;
                    Reply::Status(app.game.status_view())
                }),
            Request::SaveReplay { path } => {
                game.recorder.meta.ticks = Some(game.state.current_tick());
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
            // The shared surface was answered above; listing it keeps this
            // match exhaustive, so a new protocol request forces a decision
            // about which side of the capability split it lives on.
            Request::Status
            | Request::QueryState { .. }
            | Request::QueryFogView { .. }
            | Request::StateHash
            | Request::AdvanceTicks { .. }
            | Request::PresentTicks { .. }
            | Request::Pause
            | Request::Resume
            | Request::SetSpeed { .. } => {
                unreachable!("shared requests are answered by dispatch_shared")
            }
        };
    let response = match outcome {
        Ok(ok) => ResponseEnvelope::ok(id, ok),
        Err(err) => ResponseEnvelope::err(id, err),
    };
    reply.send(response).ok();
}

#[cfg(test)]
mod tests;
