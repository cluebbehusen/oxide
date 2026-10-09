//! Frozen inspection of the already-final live battlefield.

use crate::action::{Action, ActionEvent, ActionResolver, BindingMap, Context};
use crate::game::Game;
use crate::numeric;
use crate::press::{Fed, Press};
use crate::{render, theme};
use macroquad::prelude::*;
#[cfg(test)]
use oxide_protocol::Key;
use oxide_protocol::{MouseButton, RawEvent};

/// Camera-only state for the final battlefield view.
#[derive(Default)]
pub struct FinalMapScreen {
    pub bindings: BindingMap,
    resolver: ActionResolver,
    middle_anchor: Option<Vec2>,
    minimap_drag: bool,
    /// The corner Back button's press.
    back_press: Press<()>,
    /// The finger steering the camera from the minimap.
    minimap_finger: Option<u64>,
    /// Pan and pinch for fingers on the battlefield.
    viewer_touch: crate::viewer_touch::ViewerTouch,
}

/// A held minimap press keeps steering, clamped so sliding off the edge
/// doesn't stall the pan.
fn steer_minimap(game: &mut Game, p: Vec2) {
    if let Some(world) = render::minimap_world_clamped(&game.view(), p) {
        game.presentation.camera.center = world;
        game.presentation.camera.pan(Vec2::ZERO);
    }
}

impl FinalMapScreen {
    /// Opens a frozen map inspector.
    pub fn open() -> Self {
        Self::default()
    }

    /// Applies camera input. Returns true when the report should reopen.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        dt: f32,
        viewport: Vec2,
        camera_prefs: crate::config::CameraPrefs,
        mouse: &mut Vec2,
        game: &mut Game,
    ) -> bool {
        let ui = render::ui_scale();
        let back = crate::button::corner_slot(0, ui);
        for event in events {
            if !matches!(event, RawEvent::KeyDown { .. } | RawEvent::KeyUp { .. }) {
                match self
                    .back_press
                    .feed(event, |p, _| back.contains(p).then_some(()))
                {
                    Fed::Activated(()) => return true,
                    Fed::Held => continue,
                    Fed::Ignored => {}
                }
            }
            match event {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(*x, *y);
                    if let Some(anchor) = self.middle_anchor {
                        game.presentation
                            .camera
                            .pan((anchor - *mouse) / game.presentation.camera.zoom);
                        self.middle_anchor = Some(*mouse);
                    }
                    if self.minimap_drag {
                        steer_minimap(game, *mouse);
                    }
                }
                RawEvent::TouchDown { id, x, y } => {
                    let p = vec2(*x, *y);
                    if self.minimap_finger == Some(*id) {
                        // A platform's repeat report of the steering finger.
                    } else if self.minimap_finger.is_none()
                        && let Some(world) = render::minimap_world_at(&game.view(), p)
                    {
                        self.minimap_finger = Some(*id);
                        game.presentation.camera.center = world;
                        game.presentation.camera.pan(Vec2::ZERO);
                    } else {
                        self.viewer_touch
                            .apply(event, &mut game.presentation.camera, ui);
                    }
                }
                RawEvent::TouchMove { id, x, y } => {
                    if self.minimap_finger == Some(*id) {
                        steer_minimap(game, vec2(*x, *y));
                    } else {
                        self.viewer_touch
                            .apply(event, &mut game.presentation.camera, ui);
                    }
                }
                RawEvent::TouchUp { id, .. } => {
                    if self.minimap_finger == Some(*id) {
                        self.minimap_finger = None;
                    } else {
                        self.viewer_touch
                            .apply(event, &mut game.presentation.camera, ui);
                    }
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    *mouse = vec2(*x, *y);
                    if let Some(world) = render::minimap_world_at(&game.view(), *mouse) {
                        game.presentation.camera.center = world;
                        game.presentation.camera.pan(Vec2::ZERO);
                        self.minimap_drag = true;
                    }
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    ..
                } => self.minimap_drag = false,
                RawEvent::Wheel { delta } => {
                    let delta = if camera_prefs.zoom_inverted {
                        -*delta
                    } else {
                        *delta
                    };
                    game.presentation.camera.zoom_at(*mouse, delta);
                }
                RawEvent::KeyDown { key } => {
                    if self
                        .resolver
                        .key_edge_in(&self.bindings, *key, true, Context::FinalMap)
                        == Some(ActionEvent::Pressed(Action::Back))
                    {
                        return true;
                    }
                }
                RawEvent::KeyUp { key } => {
                    self.resolver
                        .key_edge_in(&self.bindings, *key, false, Context::FinalMap);
                }
                RawEvent::MouseDown {
                    button: MouseButton::Middle,
                    x,
                    y,
                } => self.middle_anchor = Some(vec2(*x, *y)),
                RawEvent::MouseUp {
                    button: MouseButton::Middle,
                    ..
                } => self.middle_anchor = None,
                _ => {}
            }
        }
        let direction = vec2(
            i32::from(self.resolver.is_held(Action::PanRight)) as f32
                - i32::from(self.resolver.is_held(Action::PanLeft)) as f32,
            i32::from(self.resolver.is_held(Action::PanDown)) as f32
                - i32::from(self.resolver.is_held(Action::PanUp)) as f32,
        );
        if direction != Vec2::ZERO {
            let world_per_second = crate::input::PAN_PX_PER_SEC * camera_prefs.pan_speed
                / game.presentation.camera.zoom;
            game.presentation
                .camera
                .pan(direction.normalize() * world_per_second * dt);
        }
        game.presentation.camera.set_viewport(viewport);
        game.presentation.camera.update(dt);
        false
    }

    /// Draws the Back button and the compact camera-help strip over the
    /// battlefield.
    pub fn draw_hud(&self, mouse: Vec2) {
        let scale = render::ui_scale();
        let back = crate::button::corner_slot(0, scale);
        crate::button::draw(back, "BACK", back.contains(mouse), scale);
        let size = 17.0 * scale;
        let line = if !crate::hints::showing() {
            "FINAL BATTLEFIELD".to_string()
        } else if crate::platform::TOUCH_ONLY {
            "FINAL BATTLEFIELD | drag to pan | pinch to zoom".to_string()
        } else {
            format!(
                "FINAL BATTLEFIELD | {} pan up / minimap / middle drag | wheel zoom | {} report",
                self.bindings.labels(Action::PanUp),
                self.bindings.label(Action::Back)
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
