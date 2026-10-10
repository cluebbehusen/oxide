//! Frozen inspection of the already-final live battlefield.

use crate::action::{Action, ActionEvent, ActionResolver, BindingMap, Context};
use crate::camera::controls::{MinimapPoint, ViewerHands, event_point, held_pan, pan_toward};
use crate::game::Game;
use crate::numeric;
use crate::{render, theme};
use macroquad::prelude::*;
#[cfg(test)]
use oxide_protocol::Key;
use oxide_protocol::RawEvent;

/// Camera-only state for the final battlefield view.
#[derive(Default)]
pub struct FinalMapScreen {
    resolver: ActionResolver,
    /// The corner Back button.
    back: crate::button::BackButton,
    /// Middle-drag, wheel, minimap and finger camera control.
    hands: ViewerHands,
}

impl FinalMapScreen {
    /// Opens a frozen map inspector.
    pub fn open() -> Self {
        Self::default()
    }

    /// Applies camera input. Returns true when the report should reopen.
    pub fn update(
        &mut self,
        bindings: &BindingMap,
        events: &[RawEvent],
        dt: f32,
        camera_prefs: crate::config::CameraPrefs,
        mouse: &mut Vec2,
        game: &mut Game,
    ) -> bool {
        let ui = render::ui_scale();
        let (back, events) = self.back.route(events);
        if back {
            return true;
        }
        for event in &events {
            match event {
                RawEvent::KeyDown { key } => {
                    if self
                        .resolver
                        .key_edge_in(bindings, *key, true, Context::FinalMap)
                        == Some(ActionEvent::Pressed(Action::Back))
                    {
                        return true;
                    }
                }
                RawEvent::KeyUp { key } => {
                    self.resolver
                        .key_edge_in(bindings, *key, false, Context::FinalMap);
                }
                _ => {
                    let minimap =
                        event_point(event).map_or_else(MinimapPoint::default, |p| MinimapPoint {
                            under: render::minimap_world_at(&game.view(), p),
                            clamped: render::minimap_world_clamped(&game.view(), p),
                        });
                    self.hands.pointer(
                        event,
                        minimap,
                        &mut game.presentation.camera,
                        mouse,
                        camera_prefs,
                        ui,
                    );
                }
            }
        }
        pan_toward(
            &mut game.presentation.camera,
            held_pan(&self.resolver),
            camera_prefs,
            dt,
        );
        false
    }

    /// Draws the Back button and the compact camera-help strip over the
    /// battlefield.
    pub fn draw_hud(bindings: &BindingMap, mouse: Vec2) {
        let scale = render::ui_scale();
        crate::button::draw_back(mouse);
        let size = 17.0 * scale;
        let line = if !crate::hints::showing() {
            "FINAL BATTLEFIELD".to_string()
        } else if crate::platform::TOUCH_ONLY {
            "FINAL BATTLEFIELD | drag to pan | pinch to zoom".to_string()
        } else {
            format!(
                "FINAL BATTLEFIELD | {} pan up / minimap / middle drag | wheel zoom | {} report",
                bindings.labels(Action::PanUp),
                bindings.label(Action::Back)
            )
        };
        let width = measure_text(&line, None, numeric::font_size(size), 1.0).width;
        let x = (screen_width() - width) * 0.5;
        let y = screen_height() - 14.0 * scale;
        draw_rectangle(
            x - 10.0 * scale,
            y - size,
            width + 20.0 * scale,
            size + 10.0 * scale,
            Color::from_rgba(15, 15, 19, 220),
        );
        draw_text(&line, x, y, size, theme::TEXT_PRIMARY);
    }
}

#[cfg(test)]
mod tests;
