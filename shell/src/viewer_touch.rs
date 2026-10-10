//! Camera gestures for the read-only viewers (replay playback and the
//! final map), where a finger has nothing to select or command: one
//! finger drags the world under the hand, two fingers pinch to zoom.

use crate::camera::Camera;
use macroquad::prelude::{Vec2, vec2};
use oxide_protocol::RawEvent;

/// A pair's spread must change this much, in logical px at 1x, before it
/// reads as a pinch rather than two resting fingers.
pub(crate) const PINCH_START_PX: f32 = 24.0;

/// Zoom notches per logical px of spread change.
pub(crate) const PINCH_NOTCHES_PER_PX: f32 = 0.02;

/// Fingers on the world, in touch-down order. Two at most matter.
#[derive(Debug, Default)]
pub(crate) struct ViewerTouch {
    fingers: Vec<(u64, Vec2)>,
    /// The pair's spread when it formed; a pinch is judged on the
    /// cumulative change so a slow pinch still reads as one.
    pair_start: Option<f32>,
    pinching: bool,
}

fn spread(fingers: &[(u64, Vec2)]) -> f32 {
    (fingers[0].1 - fingers[1].1).length()
}

impl ViewerTouch {
    /// Applies one touch event to `camera`; other events are ignored.
    pub(crate) fn apply(&mut self, event: &RawEvent, camera: &mut Camera, ui: f32) {
        match *event {
            RawEvent::TouchDown { id, x, y } => {
                self.fingers.push((id, vec2(x, y)));
                if self.fingers.len() > 2 {
                    self.fingers.remove(0);
                }
                self.pinching = false;
                self.pair_start = (self.fingers.len() == 2).then(|| spread(&self.fingers));
            }
            RawEvent::TouchMove { id, x, y } => {
                let p = vec2(x, y);
                let Some(index) = self.fingers.iter().position(|(known, _)| *known == id) else {
                    return;
                };
                let before = (self.fingers.len() == 2).then(|| spread(&self.fingers));
                let delta = p - self.fingers[index].1;
                self.fingers[index].1 = p;
                match (self.fingers.len(), before) {
                    (1, _) => {
                        camera.center -= delta / camera.zoom;
                        camera.pan(Vec2::ZERO);
                    }
                    (2, Some(before)) => {
                        let after = spread(&self.fingers);
                        if !self.pinching
                            && self
                                .pair_start
                                .is_some_and(|start| (after - start).abs() > PINCH_START_PX * ui)
                        {
                            self.pinching = true;
                        }
                        if self.pinching && after != before {
                            let middle = (self.fingers[0].1 + self.fingers[1].1) * 0.5;
                            camera.zoom_at(middle, (after - before) * PINCH_NOTCHES_PER_PX);
                        }
                    }
                    _ => {}
                }
            }
            RawEvent::TouchUp { id, .. } | RawEvent::TouchCancel { id } => {
                self.fingers.retain(|(known, _)| *known != id);
                if self.fingers.len() < 2 {
                    self.pinching = false;
                    self.pair_start = None;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
