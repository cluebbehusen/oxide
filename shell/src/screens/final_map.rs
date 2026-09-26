//! Frozen inspection of the already-final live battlefield.

use crate::action::{Action, ActionEvent, ActionResolver, BindingMap, Context};
use crate::game::Game;
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
    let rect = render::minimap_rect(&game.view());
    let clamped = vec2(
        p.x.clamp(rect.x, rect.x + rect.w - 1.0),
        p.y.clamp(rect.y, rect.y + rect.h - 1.0),
    );
    if let Some(world) = render::minimap_world_at(&game.view(), clamped) {
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
            let world_per_second = 240.0 * camera_prefs.pan_speed / game.presentation.camera.zoom;
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
        let line = if crate::platform::TOUCH_ONLY {
            "FINAL BATTLEFIELD | drag to pan | pinch to zoom".to_string()
        } else {
            format!(
                "FINAL BATTLEFIELD | {} pan up / minimap / middle drag | wheel zoom | {} report",
                self.bindings.labels(Action::PanUp),
                self.bindings.label(Action::Back)
            )
        };
        let width = measure_text(&line, None, size as u16, 1.0).width;
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
mod tests {
    use super::*;
    use oxide_sim::Scenario;

    #[test]
    fn final_map_accepts_only_camera_navigation_and_escape() {
        let viewport = vec2(1280.0, 800.0);
        let mut game = Game::with_viewport(Scenario::skirmish(), viewport).expect("game");
        let mut screen = FinalMapScreen::open();
        let mut mouse = Vec2::ZERO;
        let before = game.presentation.camera.center;
        assert!(!screen.update(
            &[RawEvent::KeyDown { key: Key::Right }],
            0.25,
            viewport,
            crate::config::CameraPrefs::default(),
            &mut mouse,
            &mut game,
        ));
        assert!(game.presentation.camera.center.x > before.x);
        assert!(screen.update(
            &[RawEvent::KeyDown { key: Key::Escape }],
            0.0,
            viewport,
            crate::config::CameraPrefs::default(),
            &mut mouse,
            &mut game,
        ));
        assert_eq!(
            game.state.current_tick(),
            0,
            "camera input never ticks the sim"
        );
    }

    #[test]
    fn the_back_button_returns_to_the_report_by_click_and_by_tap() {
        let viewport = vec2(1280.0, 800.0);
        let mut game = Game::with_viewport(Scenario::skirmish(), viewport).expect("game");
        let back = crate::button::corner_slot(0, render::ui_scale()).center();
        let prefs = crate::config::CameraPrefs::default();
        let mut mouse = Vec2::ZERO;
        let tap = [
            RawEvent::TouchDown {
                id: 1,
                x: back.x,
                y: back.y,
            },
            RawEvent::TouchUp {
                id: 1,
                x: back.x,
                y: back.y,
            },
        ];
        let mut screen = FinalMapScreen::open();
        assert!(screen.update(&tap, 0.0, viewport, prefs, &mut mouse, &mut game));
        let click = [
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: back.x,
                y: back.y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: back.x,
                y: back.y,
            },
        ];
        let mut screen = FinalMapScreen::open();
        assert!(screen.update(&click, 0.0, viewport, prefs, &mut mouse, &mut game));
    }

    #[test]
    fn touch_camera_gestures_never_tick_the_sim() {
        let viewport = vec2(1280.0, 800.0);
        let mut game = Game::with_viewport(Scenario::skirmish(), viewport).expect("game");
        let mut screen = FinalMapScreen::open();
        let mut mouse = Vec2::ZERO;
        let before = game.presentation.camera.center;
        let drag = [
            RawEvent::TouchDown {
                id: 1,
                x: 640.0,
                y: 300.0,
            },
            RawEvent::TouchMove {
                id: 1,
                x: 540.0,
                y: 300.0,
            },
            RawEvent::TouchUp {
                id: 1,
                x: 540.0,
                y: 300.0,
            },
        ];
        assert!(!screen.update(
            &drag,
            0.0,
            viewport,
            crate::config::CameraPrefs::default(),
            &mut mouse,
            &mut game,
        ));
        assert!(game.presentation.camera.center.x > before.x);
        assert_eq!(game.state.current_tick(), 0);
        assert!(game.pending.is_empty());
    }

    #[test]
    fn minimap_drag_and_key_release_have_explicit_camera_lifetimes() {
        let viewport = vec2(1280.0, 800.0);
        let mut game = Game::with_viewport(Scenario::skirmish(), viewport).expect("game");
        let mut screen = FinalMapScreen::open();
        let mut mouse = Vec2::ZERO;
        let rect = render::minimap_rect(&game.view());
        let mut layout = game.presentation.layout.get();
        layout.minimap = rect;
        game.presentation.layout.set(layout);
        let press = vec2(rect.x + 2.0, rect.y + 2.0);
        let expected = render::minimap_world_at(&game.view(), press).expect("inside minimap");
        let original = game.presentation.camera.center;
        game.presentation.camera.center = expected;
        game.presentation.camera.pan(Vec2::ZERO);
        let expected = game.presentation.camera.center;
        game.presentation.camera.center = original;
        game.presentation.camera.pan(Vec2::ZERO);

        screen.update(
            &[RawEvent::MouseDown {
                button: MouseButton::Left,
                x: press.x,
                y: press.y,
            }],
            0.0,
            viewport,
            crate::config::CameraPrefs::default(),
            &mut mouse,
            &mut game,
        );
        assert!(screen.minimap_drag);
        assert!((game.presentation.camera.center - expected).length() < 0.1);

        let dragged = vec2(rect.x + rect.w + 100.0, rect.y + rect.h + 100.0);
        let clamped = vec2(rect.x + rect.w, rect.y + rect.h);
        let expected_after_drag =
            render::minimap_world_at(&game.view(), clamped).expect("clamped inside minimap");
        let after_press = game.presentation.camera.center;
        game.presentation.camera.center = expected_after_drag;
        game.presentation.camera.pan(Vec2::ZERO);
        let expected_after_drag = game.presentation.camera.center;
        game.presentation.camera.center = after_press;
        game.presentation.camera.pan(Vec2::ZERO);
        screen.update(
            &[RawEvent::MouseMove {
                x: dragged.x,
                y: dragged.y,
            }],
            0.0,
            viewport,
            crate::config::CameraPrefs::default(),
            &mut mouse,
            &mut game,
        );
        let after_drag = game.presentation.camera.center;
        assert!((after_drag - expected_after_drag).length() < 0.1);
        assert_ne!(
            after_drag, after_press,
            "a held minimap drag must steer away from its press target"
        );
        screen.update(
            &[
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: dragged.x,
                    y: dragged.y,
                },
                RawEvent::MouseMove {
                    x: press.x,
                    y: press.y,
                },
            ],
            0.0,
            viewport,
            crate::config::CameraPrefs::default(),
            &mut mouse,
            &mut game,
        );
        assert!(!screen.minimap_drag);
        assert_eq!(
            game.presentation.camera.center, after_drag,
            "released drags stop steering"
        );

        screen.update(
            &[
                RawEvent::KeyDown { key: Key::Left },
                RawEvent::KeyUp { key: Key::Left },
            ],
            0.25,
            viewport,
            crate::config::CameraPrefs::default(),
            &mut mouse,
            &mut game,
        );
        assert_eq!(
            game.presentation.camera.center, after_drag,
            "released keys do not pan"
        );
    }
    #[test]
    fn final_map_keeps_the_secondary_camera_hold_after_releasing_a_rebound_primary() {
        use crate::action::Chord;
        let mut game =
            Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(640.0, 400.0)).unwrap();
        game.presentation.camera.center = vec2(18.0, 10.0);
        let mut screen = FinalMapScreen::open();
        assert!(
            screen
                .bindings
                .rebind(Action::PanRight, Chord::bare(Key::L))
        );
        let mut mouse = Vec2::ZERO;
        let prefs = crate::config::CameraPrefs::default();
        screen.update(
            &[
                RawEvent::KeyDown { key: Key::L },
                RawEvent::KeyDown { key: Key::Right },
            ],
            0.1,
            vec2(640.0, 400.0),
            prefs,
            &mut mouse,
            &mut game,
        );
        let halfway = game.presentation.camera.center.x;
        screen.update(
            &[RawEvent::KeyUp { key: Key::L }],
            0.1,
            vec2(640.0, 400.0),
            prefs,
            &mut mouse,
            &mut game,
        );
        let after = game.presentation.camera.center.x;
        assert!(after > halfway);
        screen.update(
            &[RawEvent::KeyUp { key: Key::Right }],
            0.1,
            vec2(640.0, 400.0),
            prefs,
            &mut mouse,
            &mut game,
        );
        assert_eq!(game.presentation.camera.center.x, after);
        assert!(game.pending.is_empty());
    }
}
