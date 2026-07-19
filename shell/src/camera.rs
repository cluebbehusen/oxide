//! The 2D camera: world units (tiles) to screen pixels and back.
//!
//! Pure presentation — nothing here may influence the sim. All f32 math on
//! purpose; determinism is the sim's job.

use macroquad::prelude::{Vec2, screen_height, screen_width, vec2};

/// Zoom bounds in pixels per tile.
const ZOOM_MIN: f32 = 8.0;
const ZOOM_MAX: f32 = 96.0;

/// A pan/zoom camera over the tile grid.
pub struct Camera {
    /// World point at the viewport center.
    pub center: Vec2,
    /// Pixels per world unit.
    pub zoom: f32,
    map_size: Vec2,
}

impl Camera {
    /// A camera looking at `focus` on a `width`×`height`-tile map.
    pub fn new(focus: Vec2, width: i32, height: i32) -> Self {
        let mut camera = Self {
            center: focus,
            zoom: 32.0,
            map_size: vec2(width as f32, height as f32),
        };
        camera.clamp();
        camera
    }

    /// Screen pixels for a world point.
    pub fn to_screen(&self, world: Vec2) -> Vec2 {
        (world - self.center) * self.zoom + vec2(screen_width(), screen_height()) * 0.5
    }

    /// World point under a screen pixel.
    pub fn to_world(&self, screen: Vec2) -> Vec2 {
        (screen - vec2(screen_width(), screen_height()) * 0.5) / self.zoom + self.center
    }

    /// Pans by a world-space delta.
    pub fn pan(&mut self, delta: Vec2) {
        self.center += delta;
        self.clamp();
    }

    /// Zooms by wheel notches, keeping the world point under `cursor_px`
    /// stationary — zoom goes where you're looking.
    pub fn zoom_at(&mut self, cursor_px: Vec2, notches: f32) {
        let anchor = self.to_world(cursor_px);
        self.zoom = (self.zoom * 1.15f32.powf(notches)).clamp(ZOOM_MIN, ZOOM_MAX);
        let after = self.to_world(cursor_px);
        self.center += anchor - after;
        self.clamp();
    }

    /// The visible world rectangle `[min, max]`.
    pub fn world_rect(&self) -> (Vec2, Vec2) {
        let half = vec2(screen_width(), screen_height()) * 0.5 / self.zoom;
        (self.center - half, self.center + half)
    }

    fn clamp(&mut self) {
        // Keep the view inside the map, with slack when zoomed far out.
        let half = vec2(screen_width(), screen_height()) * 0.5 / self.zoom;
        let slack = vec2(2.0, 2.0);
        let lo = (half - slack).min(self.map_size * 0.5);
        let hi = (self.map_size - half + slack).max(self.map_size * 0.5);
        self.center = self.center.clamp(lo, hi);
    }
}
