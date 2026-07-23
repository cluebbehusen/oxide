//! The read-only replay viewer: the playback engine owns truth, a
//! `Game` is its render vehicle, and this screen owns the transport
//! (pause, seek, speed, camera). Update is windowless — the whole
//! transport drives headless in tests.

use crate::game::{self, Game, GameReplay};
use crate::render;
use anyhow::{Context, Result};
use macroquad::prelude::*;
use oxide_protocol::{Key, MouseButton, RawEvent};

/// A playback viewing session: the engine owns truth, the `Game` is a
/// render vehicle whose state gets replaced after every advance — its
/// recorder, sounds, and effects are simply never fed.
pub struct PlaybackSession {
    pub engine: oxide_kit::playback::Playback,
    pub game: Game,
    pub speed: f32,
    pub paused: bool,
    pub accum: f32,
    pub held: [bool; 4],
    /// A held minimap press steers the camera, like live play.
    pub minimap_drag: bool,
    /// Whether the viewer was opened from a live pause menu — leaving
    /// returns there; every other origin goes Home. A tick-count
    /// heuristic resurrected matches Main Menu had already discarded.
    pub from_pause: bool,
}

impl PlaybackSession {
    pub fn open(path: &str) -> Result<Self> {
        let replay = GameReplay::load(path).with_context(|| format!("loading replay {path}"))?;
        Self::from_replay(replay)
    }

    pub fn from_replay(replay: GameReplay) -> Result<Self> {
        let scenario = replay.setup.clone();
        let engine = oxide_kit::playback::Playback::load(replay)?;
        let mut game = Game::new(scenario)?;
        // Spectator truth: fog-free, but NOT the developer overlay —
        // playback must look like the game, not the debugger.
        game.spectate = true;
        Ok(Self {
            engine,
            game,
            speed: 1.0,
            paused: false,
            accum: 0.0,
            held: [false; 4],
            minimap_drag: false,
            from_pause: false,
        })
    }
}

pub fn playback_hud(pb: &PlaybackSession) {
    let s = render::ui_scale();
    let size = 18.0 * s;
    let full = format!(
        "PLAYBACK  {} / {}  ·  {}x{}  ·  Space pause · PgUp/PgDn seek · Home/End · 1/2/3 speed · Esc leave",
        pb.engine.position(),
        pb.engine.total(),
        pb.speed,
        if pb.paused { "  ·  PAUSED" } else { "" },
    );
    // A 640px window cannot seat the controls hint; the transport
    // numbers alone must never run off both edges.
    let line = if measure_text(&full, None, size as u16, 1.0).width > screen_width() - 16.0 * s {
        format!(
            "PLAYBACK  {} / {}  ·  {}x{}",
            pb.engine.position(),
            pb.engine.total(),
            pb.speed,
            if pb.paused { "  ·  PAUSED" } else { "" },
        )
    } else {
        full
    };
    let width = measure_text(&line, None, size as u16, 1.0).width;
    let x = (screen_width() - width) * 0.5;
    let y = screen_height() - 14.0 * s;
    draw_rectangle(
        x - 10.0 * s,
        y - size,
        width + 20.0 * s,
        size + 10.0 * s,
        Color::from_rgba(15, 15, 19, 220),
    );
    draw_text(&line, x, y, size, Color::from_rgba(232, 228, 216, 255));
}

impl PlaybackSession {
    /// Applies a frame of transport input and advances the reproduction.
    /// Returns true when the viewer should close. `viewport` is injected
    /// like everywhere else, so tests never need a window.
    pub fn update(
        &mut self,
        events: &[RawEvent],
        dt: f32,
        viewport: Vec2,
        zoom_inverted: bool,
        pan_speed: f32,
        mouse: &mut Vec2,
    ) -> bool {
        let mut seek_to: Option<u64> = None;
        let mut leave = false;
        for e in events {
            match e {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(*x, *y);
                    // A held minimap press keeps steering, clamped so
                    // sliding off the edge doesn't stall the pan — same
                    // feel as live play.
                    if self.minimap_drag {
                        let rect = render::minimap_rect(&self.game);
                        let clamped = vec2(
                            x.clamp(rect.x, rect.x + rect.w - 1.0),
                            y.clamp(rect.y, rect.y + rect.h - 1.0),
                        );
                        if let Some(world) = render::minimap_world_at(&self.game, clamped) {
                            self.game.camera.center = world;
                            self.game.camera.pan(vec2(0.0, 0.0));
                        }
                    }
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    *mouse = vec2(*x, *y);
                    if let Some(world) = render::minimap_world_at(&self.game, *mouse) {
                        self.game.camera.center = world;
                        self.game.camera.pan(vec2(0.0, 0.0));
                        self.minimap_drag = true;
                    }
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    ..
                } => self.minimap_drag = false,
                RawEvent::Wheel { delta } => {
                    let delta = if zoom_inverted { -*delta } else { *delta };
                    self.game.camera.zoom_at(*mouse, delta);
                }
                RawEvent::KeyDown { key } => match key {
                    Key::Escape => leave = true,
                    Key::Space => self.paused = !self.paused,
                    Key::PageUp => {
                        seek_to = Some(self.engine.position().saturating_sub(500));
                    }
                    Key::PageDown => seek_to = Some(self.engine.position() + 500),
                    Key::Home => seek_to = Some(0),
                    Key::End => seek_to = Some(self.engine.total()),
                    Key::Num1 => self.speed = 0.5,
                    Key::Num2 => self.speed = 1.0,
                    Key::Num3 => self.speed = 4.0,
                    Key::Up => self.held[0] = true,
                    Key::Down => self.held[1] = true,
                    Key::Left => self.held[2] = true,
                    Key::Right => self.held[3] = true,
                    _ => {}
                },
                RawEvent::KeyUp { key } => match key {
                    Key::Up => self.held[0] = false,
                    Key::Down => self.held[1] = false,
                    Key::Left => self.held[2] = false,
                    Key::Right => self.held[3] = false,
                    _ => {}
                },
                _ => {}
            }
        }
        if leave {
            return true;
        }
        let mut dir = vec2(0.0, 0.0);
        if self.held[0] {
            dir.y -= 1.0;
        }
        if self.held[1] {
            dir.y += 1.0;
        }
        if self.held[2] {
            dir.x -= 1.0;
        }
        if self.held[3] {
            dir.x += 1.0;
        }
        if dir != vec2(0.0, 0.0) {
            let world_per_sec = 240.0 * pan_speed / self.game.camera.zoom;
            self.game.camera.pan(dir.normalize() * world_per_sec * dt);
        }
        if let Some(target) = seek_to {
            self.engine.seek(target);
            self.accum = 0.0;
            // A seek is a bulk jump: presentation resyncs silently
            // instead of replaying a burst.
            self.game.drop_presentation();
            self.game.state = self.engine.state.clone();
        } else if !self.paused && !self.engine.at_end() {
            self.accum += dt * self.speed;
            let ticks = (self.accum / game::TICK_DT) as u64;
            if ticks > 0 {
                self.accum -= ticks as f32 * game::TICK_DT;
                // One tick per present: fog is per-tick truth, and
                // batching sight checks against the final state judged
                // sounds by the wrong tick's sight. Ticks past the cap
                // are dropped debt, exactly like the live clock after a
                // hitch.
                for _ in 0..ticks.min(24) {
                    let events = self.engine.advance(1);
                    self.game.playback_present(&self.engine.state, &events);
                    if self.engine.at_end() {
                        break;
                    }
                }
            }
        }
        if self.game.state.current_tick() != self.engine.position() {
            self.game.state = self.engine.state.clone();
        }
        self.game.update_fx(dt);
        self.game.camera.set_viewport(viewport);
        self.game.camera.update(dt);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn session() -> PlaybackSession {
        // A short real replay: run the embedded skirmish headless and
        // record it, exactly what a save file contains.
        let scenario = oxide_sim::Scenario::skirmish();
        let outcome = oxide_kit::runner::run_scenario(&scenario, 60, true, true).expect("run");
        let mut replay = outcome.replay.expect("recorded");
        replay.meta.ticks = Some(60);
        PlaybackSession::from_replay(replay).expect("session opens")
    }

    fn key(session: &mut PlaybackSession, key: Key) -> bool {
        let mut mouse = vec2(0.0, 0.0);
        session.update(
            &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
            0.0,
            vec2(1280.0, 800.0),
            false,
            1.0,
            &mut mouse,
        )
    }

    #[test]
    fn the_transport_answers_its_keys() {
        let mut pb = session();
        assert!(!pb.paused);
        key(&mut pb, Key::Space);
        assert!(pb.paused, "space pauses");
        key(&mut pb, Key::Num3);
        assert!((pb.speed - 4.0).abs() < f32::EPSILON, "3 is 4x");
        key(&mut pb, Key::End);
        assert_eq!(pb.engine.position(), 60, "End seeks to the tail");
        key(&mut pb, Key::Home);
        assert_eq!(pb.engine.position(), 0, "Home rewinds");
        key(&mut pb, Key::PageDown);
        assert_eq!(pb.engine.position(), 60, "seeks clamp to the total");
        assert!(key(&mut pb, Key::Escape), "Escape closes the viewer");
    }

    #[test]
    fn paused_time_does_not_advance_the_reproduction() {
        let mut pb = session();
        key(&mut pb, Key::Space);
        let before = pb.engine.position();
        let mut mouse = vec2(0.0, 0.0);
        pb.update(&[], 1.0, vec2(1280.0, 800.0), false, 1.0, &mut mouse);
        assert_eq!(pb.engine.position(), before, "a paused viewer holds still");
    }
}
