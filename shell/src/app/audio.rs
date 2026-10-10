//! The frame's sound: whichever session is visible feeds the mixer, and the
//! score follows the screen.

use super::{App, Screen};
use crate::game::{Game, SoundKind};
use macroquad::prelude::Vec2;

/// Mixes and plays this frame's queued sounds and steers the score.
pub(super) fn frame(app: &mut App, screen: &mut Screen, dt: f32) {
    // The mixer serves whichever session is visible; playback owns a
    // separate presentation game and therefore a separate sound queue.
    let (queued, cam_center, cam_half_extents, cam_zoom): (
        Vec<(SoundKind, Option<Vec2>)>,
        Vec2,
        Vec2,
        f32,
    ) = match &mut *screen {
        Screen::Playback(pb) => (
            std::mem::take(&mut pb.presentation.sounds_pending),
            pb.presentation.camera.center,
            pb.presentation.camera.viewport() / pb.presentation.camera.zoom * 0.5,
            pb.presentation.camera.zoom,
        ),
        _ => (
            std::mem::take(&mut app.game.presentation.sounds_pending),
            app.game.presentation.camera.center,
            app.game.presentation.camera.viewport() / app.game.presentation.camera.zoom * 0.5,
            app.game.presentation.camera.zoom,
        ),
    };
    let (motor_game, motor_running) = match &*screen {
        Screen::Playback(pb) => (pb.view(), !pb.paused && pb.seeking.is_none()),
        Screen::Playing => (app.game.view(), !app.game.presentation.paused),
        _ => (app.game.view(), false),
    };
    app.mixer.rocket_loops.update(
        &motor_game,
        motor_running,
        &app.sounds.rocket_motors,
        app.config.volumes.master
            * app.config.volumes.effects
            * crate::mixer::Mixer::base_volume(SoundKind::RocketMotor),
    );
    let mixed = crate::audio_mix::frame_mix(queued, cam_center, cam_half_extents, cam_zoom);
    let combat_impulse = mixed
        .iter()
        .any(|event| crate::mixer::spec(event.kind).combat);
    for event in mixed {
        app.mixer
            .play(&app.sounds, event.kind, &app.config.volumes, event.gain);
    }
    if let Some(soundtrack) = &mut app.soundtrack {
        soundtrack.update(
            soundtrack_scene(screen, &app.game),
            combat_impulse,
            dt,
            app.config.volumes,
        );
        soundtrack.apply(&app.sounds);
    }
}

pub(super) fn soundtrack_scene(screen: &Screen, game: &Game) -> crate::soundtrack::Scene {
    match screen {
        Screen::Playing => crate::soundtrack::match_scene(&game.view(), false),
        Screen::Playback(playback) => crate::soundtrack::match_scene(
            &playback.view(),
            playback.paused || playback.seeking.is_some(),
        ),
        Screen::FinalMap(_) => crate::soundtrack::match_scene(&game.view(), true),
        Screen::Busy(_) | Screen::Pause(_) => crate::soundtrack::match_scene(&game.view(), true),
        Screen::Settings { back, .. } | Screen::Codex { back, .. }
            if matches!(**back, Screen::Pause(_)) =>
        {
            crate::soundtrack::match_scene(&game.view(), true)
        }
        Screen::Results(_) => crate::soundtrack::match_scene(&game.view(), false),
        Screen::Home(_)
        | Screen::Lobby { .. }
        | Screen::Settings { .. }
        | Screen::Codex { .. }
        | Screen::Wizard(_)
        | Screen::Replays(_) => crate::soundtrack::Scene::Menu,
    }
}
