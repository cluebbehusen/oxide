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
                let p = vec2(x, y);
                // Some platforms re-report every live finger when
                // another lands; a repeat start is not a new finger.
                if let Some(finger) = self.fingers.iter_mut().find(|(known, _)| *known == id) {
                    finger.1 = p;
                    return;
                }
                self.fingers.push((id, p));
                if self.fingers.len() > 2 {
                    self.fingers.remove(0);
                }
                self.pinching = false;
                self.pair_start = (self.fingers.len() == 2).then(|| spread(&self.fingers));
            }
            RawEvent::TouchMove { id, x, y } => {
                let p = vec2(x, y);
                let Some(index) = self.fingers.iter().position(|(known, _)| *known == id) else {
                    // The same platforms report every finger as lifted
                    // when one lifts; a survivor that keeps moving is
                    // taken back without a jump.
                    if self.fingers.len() < 2 {
                        self.fingers.push((id, p));
                    }
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
            RawEvent::TouchUp { id, .. } => {
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
mod tests {
    use super::*;

    fn camera() -> Camera {
        Camera::new(vec2(32.0, 32.0), 64, 64, vec2(1280.0, 800.0))
    }

    fn down(id: u64, x: f32, y: f32) -> RawEvent {
        RawEvent::TouchDown { id, x, y }
    }

    fn moved(id: u64, x: f32, y: f32) -> RawEvent {
        RawEvent::TouchMove { id, x, y }
    }

    #[test]
    fn one_finger_drags_the_world_under_the_hand() {
        let mut camera = camera();
        let mut touch = ViewerTouch::default();
        let before = camera.center;
        touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
        touch.apply(&moved(1, 500.0, 400.0), &mut camera, 1.0);
        assert!(
            camera.center.x > before.x,
            "dragging left reveals ground to the right"
        );
        assert_eq!(camera.center.y, before.y);
    }

    #[test]
    fn a_spread_zooms_in_and_a_still_pair_does_not() {
        let mut camera = camera();
        let mut touch = ViewerTouch::default();
        let zoom = camera.zoom;
        touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
        touch.apply(&down(2, 700.0, 400.0), &mut camera, 1.0);
        touch.apply(&moved(2, 710.0, 400.0), &mut camera, 1.0);
        camera.update(1.0);
        assert_eq!(camera.zoom, zoom, "a small wobble is not a pinch");
        for x in [740.0, 780.0, 820.0] {
            touch.apply(&moved(2, x, 400.0), &mut camera, 1.0);
        }
        camera.update(1.0);
        assert!(camera.zoom > zoom, "spreading the pair zooms in");
    }

    #[test]
    fn a_re_reported_start_does_not_duplicate_a_finger() {
        let mut camera = camera();
        let mut touch = ViewerTouch::default();
        touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
        touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
        let before = camera.center;
        touch.apply(&moved(1, 500.0, 400.0), &mut camera, 1.0);
        assert!(
            camera.center.x > before.x,
            "still one finger, so it still pans"
        );
    }

    #[test]
    fn a_survivor_reported_lifted_resumes_panning_without_a_jump() {
        let mut camera = camera();
        let mut touch = ViewerTouch::default();
        touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
        touch.apply(&down(2, 700.0, 400.0), &mut camera, 1.0);
        touch.apply(
            &RawEvent::TouchUp {
                id: 1,
                x: 600.0,
                y: 400.0,
            },
            &mut camera,
            1.0,
        );
        touch.apply(
            &RawEvent::TouchUp {
                id: 2,
                x: 700.0,
                y: 400.0,
            },
            &mut camera,
            1.0,
        );
        let before = camera.center;
        touch.apply(&moved(2, 650.0, 400.0), &mut camera, 1.0);
        assert_eq!(camera.center, before, "the first move re-anchors");
        touch.apply(&moved(2, 600.0, 400.0), &mut camera, 1.0);
        assert!(camera.center.x > before.x);
    }
}
