//! Frozen inspection of the already-final live battlefield.

use crate::game::Game;
use crate::{render, theme};
use macroquad::prelude::*;
use oxide_protocol::{Key, MouseButton, RawEvent};

/// Camera-only state for the final battlefield view.
#[derive(Default)]
pub struct FinalMapScreen {
    held: [bool; 4],
    minimap_drag: bool,
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
        for event in events {
            match event {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(*x, *y);
                    if self.minimap_drag {
                        let rect = render::minimap_rect(game);
                        let clamped = vec2(
                            x.clamp(rect.x, rect.x + rect.w - 1.0),
                            y.clamp(rect.y, rect.y + rect.h - 1.0),
                        );
                        if let Some(world) = render::minimap_world_at(game, clamped) {
                            game.camera.center = world;
                            game.camera.pan(Vec2::ZERO);
                        }
                    }
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    *mouse = vec2(*x, *y);
                    if let Some(world) = render::minimap_world_at(game, *mouse) {
                        game.camera.center = world;
                        game.camera.pan(Vec2::ZERO);
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
                    game.camera.zoom_at(*mouse, delta);
                }
                RawEvent::KeyDown { key: Key::Escape } => return true,
                RawEvent::KeyDown { key: Key::Up } => self.held[0] = true,
                RawEvent::KeyDown { key: Key::Down } => self.held[1] = true,
                RawEvent::KeyDown { key: Key::Left } => self.held[2] = true,
                RawEvent::KeyDown { key: Key::Right } => self.held[3] = true,
                RawEvent::KeyUp { key: Key::Up } => self.held[0] = false,
                RawEvent::KeyUp { key: Key::Down } => self.held[1] = false,
                RawEvent::KeyUp { key: Key::Left } => self.held[2] = false,
                RawEvent::KeyUp { key: Key::Right } => self.held[3] = false,
                _ => {}
            }
        }
        let direction = vec2(
            i32::from(self.held[3]) as f32 - i32::from(self.held[2]) as f32,
            i32::from(self.held[1]) as f32 - i32::from(self.held[0]) as f32,
        );
        if direction != Vec2::ZERO {
            let world_per_second = 240.0 * camera_prefs.pan_speed / game.camera.zoom;
            game.camera
                .pan(direction.normalize() * world_per_second * dt);
        }
        game.camera.set_viewport(viewport);
        game.camera.update(dt);
        false
    }

    /// Draws the compact camera-help strip over the battlefield.
    pub fn draw_hud() {
        let scale = render::ui_scale();
        let size = 17.0 * scale;
        let line = "FINAL BATTLEFIELD  |  arrows/minimap pan  |  wheel zoom  |  Esc report";
        let width = measure_text(line, None, size as u16, 1.0).width;
        let x = (screen_width() - width) * 0.5;
        let y = screen_height() - 14.0 * scale;
        draw_rectangle(
            x - 10.0 * scale,
            y - size,
            width + 20.0 * scale,
            size + 10.0 * scale,
            Color::from_rgba(15, 15, 19, 220),
        );
        draw_text(line, x, y, size, theme::TEXT_PRIMARY);
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
        let before = game.camera.center;
        assert!(!screen.update(
            &[RawEvent::KeyDown { key: Key::Right }],
            0.25,
            viewport,
            crate::config::CameraPrefs::default(),
            &mut mouse,
            &mut game,
        ));
        assert!(game.camera.center.x > before.x);
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
    fn minimap_drag_and_key_release_have_explicit_camera_lifetimes() {
        let viewport = vec2(1280.0, 800.0);
        let mut game = Game::with_viewport(Scenario::skirmish(), viewport).expect("game");
        let mut screen = FinalMapScreen::open();
        let mut mouse = Vec2::ZERO;
        let rect = render::minimap_rect(&game);
        let mut layout = game.layout.get();
        layout.minimap = rect;
        game.layout.set(layout);
        let press = vec2(rect.x + 2.0, rect.y + 2.0);
        let expected = render::minimap_world_at(&game, press).expect("inside minimap");
        let original = game.camera.center;
        game.camera.center = expected;
        game.camera.pan(Vec2::ZERO);
        let expected = game.camera.center;
        game.camera.center = original;
        game.camera.pan(Vec2::ZERO);

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
        assert!((game.camera.center - expected).length() < 0.1);

        let dragged = vec2(rect.x + rect.w + 100.0, rect.y + rect.h + 100.0);
        let clamped = vec2(rect.x + rect.w, rect.y + rect.h);
        let expected_after_drag =
            render::minimap_world_at(&game, clamped).expect("clamped inside minimap");
        let after_press = game.camera.center;
        game.camera.center = expected_after_drag;
        game.camera.pan(Vec2::ZERO);
        let expected_after_drag = game.camera.center;
        game.camera.center = after_press;
        game.camera.pan(Vec2::ZERO);
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
        let after_drag = game.camera.center;
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
            game.camera.center, after_drag,
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
        assert_eq!(game.camera.center, after_drag, "released keys do not pan");
    }
}
