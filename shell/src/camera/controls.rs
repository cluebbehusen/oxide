//! The camera hands every view of the battlefield shares. Held keys pan and
//! the wheel zooms in live play and both read-only viewers; the viewers add
//! middle-drag, minimap steering, and finger pan and pinch through
//! [`ViewerHands`]. Live play keeps its own pointer funnel, since its
//! presses also select and command.

use super::Camera;
use crate::action::{Action, ActionResolver};
use crate::config::CameraPrefs;
use crate::viewer_touch::ViewerTouch;
use macroquad::prelude::{Vec2, vec2};
use oxide_protocol::{MouseButton, RawEvent};

/// The direction the held pan keys point, before normalizing.
pub(crate) fn held_pan(resolver: &ActionResolver) -> Vec2 {
    let mut direction = Vec2::ZERO;
    if resolver.is_held(Action::PanUp) {
        direction.y -= 1.0;
    }
    if resolver.is_held(Action::PanDown) {
        direction.y += 1.0;
    }
    if resolver.is_held(Action::PanLeft) {
        direction.x -= 1.0;
    }
    if resolver.is_held(Action::PanRight) {
        direction.x += 1.0;
    }
    direction
}

/// Pans `camera` along `direction` at the player's pan speed for `dt`
/// seconds; a zero direction stays put.
pub(crate) fn pan_toward(camera: &mut Camera, direction: Vec2, prefs: CameraPrefs, dt: f32) {
    if direction != Vec2::ZERO {
        let world_per_second = crate::input::PAN_PX_PER_SEC * prefs.pan_speed / camera.zoom;
        camera.pan(direction.normalize() * world_per_second * dt);
    }
}

/// Zooms toward the screen point `at` by one wheel reading, honoring the
/// inverted-zoom preference.
pub(crate) fn wheel_zoom(camera: &mut Camera, at: Vec2, delta: f32, prefs: CameraPrefs) {
    camera.zoom_at(at, if prefs.zoom_inverted { -delta } else { delta });
}

/// A pointer's place against the minimap, read from the scene before the
/// camera is borrowed to move.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MinimapPoint {
    /// The world point under the pointer, when it is on the minimap.
    pub(crate) under: Option<Vec2>,
    /// Where a held press steers, clamped so sliding off the edge doesn't
    /// stall the pan.
    pub(crate) clamped: Option<Vec2>,
}

/// The read-only viewers' camera hands: middle-drag, the wheel, minimap
/// steering by mouse or finger, and finger pan and pinch.
#[derive(Default)]
pub(crate) struct ViewerHands {
    middle_anchor: Option<Vec2>,
    minimap_drag: bool,
    minimap_finger: Option<u64>,
    touch: ViewerTouch,
}

impl ViewerHands {
    /// Applies one pointer event to `camera`, tracking the cursor in `mouse`.
    pub(crate) fn pointer(
        &mut self,
        event: &RawEvent,
        minimap: MinimapPoint,
        camera: &mut Camera,
        mouse: &mut Vec2,
        prefs: CameraPrefs,
        ui: f32,
    ) {
        match *event {
            RawEvent::MouseMove { x, y } => {
                *mouse = vec2(x, y);
                if let Some(anchor) = self.middle_anchor {
                    camera.pan((anchor - *mouse) / camera.zoom);
                    self.middle_anchor = Some(*mouse);
                }
                if self.minimap_drag {
                    steer(camera, minimap.clamped);
                }
            }
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x,
                y,
            } => {
                *mouse = vec2(x, y);
                if let Some(world) = minimap.under {
                    jump(camera, world);
                    self.minimap_drag = true;
                }
            }
            RawEvent::MouseUp {
                button: MouseButton::Left,
                ..
            } => self.minimap_drag = false,
            RawEvent::Wheel { delta } => wheel_zoom(camera, *mouse, delta, prefs),
            RawEvent::MouseDown {
                button: MouseButton::Middle,
                x,
                y,
            } => self.middle_anchor = Some(vec2(x, y)),
            RawEvent::MouseUp {
                button: MouseButton::Middle,
                ..
            } => self.middle_anchor = None,
            RawEvent::TouchDown { id, .. } => {
                if self.minimap_finger == Some(id) {
                    // A platform's repeat report of the steering finger.
                } else if self.minimap_finger.is_none()
                    && let Some(world) = minimap.under
                {
                    self.minimap_finger = Some(id);
                    jump(camera, world);
                } else {
                    self.touch.apply(event, camera, ui);
                }
            }
            RawEvent::TouchMove { id, .. } => {
                if self.minimap_finger == Some(id) {
                    steer(camera, minimap.clamped);
                } else {
                    self.touch.apply(event, camera, ui);
                }
            }
            RawEvent::TouchUp { id, .. } | RawEvent::TouchCancel { id } => {
                if self.minimap_finger == Some(id) {
                    self.minimap_finger = None;
                } else {
                    self.touch.apply(event, camera, ui);
                }
            }
            _ => {}
        }
    }
}

/// The screen point a pointer event carries, if any.
pub(crate) fn event_point(event: &RawEvent) -> Option<Vec2> {
    match *event {
        RawEvent::MouseMove { x, y }
        | RawEvent::MouseDown { x, y, .. }
        | RawEvent::MouseUp { x, y, .. }
        | RawEvent::TouchDown { x, y, .. }
        | RawEvent::TouchMove { x, y, .. }
        | RawEvent::TouchUp { x, y, .. } => Some(vec2(x, y)),
        _ => None,
    }
}

fn jump(camera: &mut Camera, world: Vec2) {
    camera.center = world;
    camera.pan(Vec2::ZERO);
}

fn steer(camera: &mut Camera, clamped: Option<Vec2>) {
    if let Some(world) = clamped {
        jump(camera, world);
    }
}

#[cfg(test)]
mod tests;
