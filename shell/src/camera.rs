//! The 2D camera: world units (tiles) to screen pixels and back.
//!
//! Pure presentation — nothing here may influence the sim. All f32 math on
//! purpose; determinism is the sim's job.
//!
//! The camera never queries the window: the viewport is injected (once per
//! frame by the main loop), which keeps every method a pure function of
//! `Camera` state — and therefore unit-testable without a window.

use macroquad::prelude::{Vec2, vec2};

/// Zoom bounds in *logical* pixels per tile. Logical is the whole
/// story: macroquad's coordinate space already absorbs the retina
/// multiple in its backing store, so multiplying dpi in here rendered
/// the world at double scale on every high-dpi display — the same
/// double-scaling disease the ui-scale audit cured in chrome.
const ZOOM_MIN: f32 = 8.0;
const ZOOM_MAX: f32 = 96.0;
const ZOOM_DEFAULT: f32 = 32.0;

/// A pan/zoom camera over the tile grid.
pub struct Camera {
    /// World point at the viewport center.
    pub center: Vec2,
    /// Physical pixels per world unit.
    pub zoom: f32,
    target_zoom: f32,
    /// Cursor the current zoom glide is anchored on.
    zoom_anchor: Option<Vec2>,
    zoom_min: f32,
    zoom_max: f32,
    viewport: Vec2,
    map_size: Vec2,
}

impl Camera {
    /// A camera looking at `focus` on a `width`×`height`-tile map,
    /// rendered into a `viewport`-logical-pixel window.
    pub fn new(focus: Vec2, width: i32, height: i32, viewport: Vec2) -> Self {
        let mut camera = Self {
            center: focus,
            zoom: ZOOM_DEFAULT,
            target_zoom: ZOOM_DEFAULT,
            zoom_anchor: None,
            zoom_min: ZOOM_MIN,
            zoom_max: ZOOM_MAX,
            viewport,
            map_size: vec2(width as f32, height as f32),
        };
        camera.clamp();
        camera
    }

    /// Current viewport size in pixels.
    pub fn viewport(&self) -> Vec2 {
        self.viewport
    }

    /// Tracks a window resize (called once per frame); re-clamps so the
    /// view never strands outside the map.
    pub fn set_viewport(&mut self, viewport: Vec2) {
        if viewport != self.viewport {
            self.viewport = viewport;
            self.clamp();
        }
    }

    /// Screen pixels for a world point.
    pub fn to_screen(&self, world: Vec2) -> Vec2 {
        (world - self.center) * self.zoom + self.viewport * 0.5
    }

    /// World point under a screen pixel.
    pub fn to_world(&self, screen: Vec2) -> Vec2 {
        (screen - self.viewport * 0.5) / self.zoom + self.center
    }

    /// Pans by a world-space delta.
    pub fn pan(&mut self, delta: Vec2) {
        self.center += delta;
        self.clamp();
    }

    /// Requests a zoom by wheel notches; the camera glides there over the
    /// next frames ([`Camera::update`]), keeping the world point under
    /// `cursor_px` stationary the whole way — smooth for trackpads and
    /// notchy wheels alike.
    pub fn zoom_at(&mut self, cursor_px: Vec2, notches: f32) {
        self.target_zoom =
            (self.target_zoom * 1.15f32.powf(notches)).clamp(self.zoom_min, self.zoom_max);
        self.zoom_anchor = Some(cursor_px);
    }

    /// Advances the zoom glide. Call once per frame.
    pub fn update(&mut self, dt: f32) {
        if self.zoom == self.target_zoom {
            self.zoom_anchor = None;
            return;
        }
        let anchor_px = self.zoom_anchor.unwrap_or(self.viewport * 0.5);
        let anchor_world = self.to_world(anchor_px);
        let t = (12.0 * dt).min(1.0);
        self.zoom += (self.target_zoom - self.zoom) * t;
        if (self.target_zoom - self.zoom).abs() < 0.01 {
            self.zoom = self.target_zoom;
            self.zoom_anchor = None;
        }
        let after = self.to_world(anchor_px);
        self.center += anchor_world - after;
        self.clamp();
    }

    /// The visible world rectangle `[min, max]`.
    pub fn world_rect(&self) -> (Vec2, Vec2) {
        let half = self.viewport * 0.5 / self.zoom;
        (self.center - half, self.center + half)
    }

    fn clamp(&mut self) {
        // Keep the view inside the map, with slack when zoomed far out.
        let half = self.viewport * 0.5 / self.zoom;
        let slack = vec2(2.0, 2.0);
        let lo = (half - slack).min(self.map_size * 0.5);
        let hi = (self.map_size - half + slack).max(self.map_size * 0.5);
        self.center = self.center.clamp(lo, hi);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        Camera::new(vec2(20.0, 12.0), 40, 24, vec2(1280.0, 800.0))
    }

    #[test]
    fn screen_world_roundtrip() {
        let cam = camera();
        for point in [vec2(0.0, 0.0), vec2(640.0, 400.0), vec2(1279.0, 799.0)] {
            let world = cam.to_world(point);
            let back = cam.to_screen(world);
            assert!((back - point).length() < 1e-3, "{point:?} -> {back:?}");
        }
    }

    #[test]
    fn zoom_keeps_the_cursor_anchor_fixed() {
        let mut cam = camera();
        let cursor = vec2(300.0, 250.0);
        let before = cam.to_world(cursor);
        cam.zoom_at(cursor, 2.0);
        cam.update(1.0); // saturated step: glide finishes instantly
        let after = cam.to_world(cursor);
        assert!(
            (after - before).length() < 1e-3,
            "anchor drifted: {before:?} -> {after:?}"
        );
        assert!(cam.zoom > 32.0);
    }

    #[test]
    fn zoom_clamps_to_bounds() {
        let mut cam = camera();
        cam.zoom_at(vec2(0.0, 0.0), 100.0);
        cam.update(1.0);
        assert_eq!(cam.zoom, ZOOM_MAX);
        cam.zoom_at(vec2(0.0, 0.0), -100.0);
        cam.update(1.0);
        assert_eq!(cam.zoom, ZOOM_MIN);
    }

    #[test]
    fn pan_clamps_to_map_with_slack() {
        let mut cam = camera();
        cam.pan(vec2(-1000.0, -1000.0));
        let lo = cam.center;
        cam.pan(vec2(2000.0, 2000.0));
        let hi = cam.center;
        assert!(lo.x < hi.x && lo.y < hi.y);
        // Slack allows at most two tiles beyond the edges.
        let (min, _) = {
            cam.center = lo;
            cam.world_rect()
        };
        assert!(min.x >= -3.0 && min.y >= -3.0, "runaway clamp: {min:?}");
    }

    #[test]
    fn resize_reclamps() {
        let mut cam = camera();
        cam.pan(vec2(1000.0, 1000.0)); // pinned at the bottom-right clamp
        let before = cam.center;
        cam.set_viewport(vec2(640.0, 400.0)); // smaller window: clamp loosens
        cam.pan(vec2(1000.0, 1000.0));
        assert!(cam.center.x >= before.x && cam.center.y >= before.y);
    }

    #[test]
    fn a_map_smaller_than_the_viewport_pins_the_view_to_center() {
        // A 4x4 map is dwarfed by the 1280x800 viewport at default zoom, so
        // there is no room to pan: the clamp holds the center on the map's
        // midpoint however hard it is shoved.
        let mut cam = Camera::new(vec2(2.0, 2.0), 4, 4, vec2(1280.0, 800.0));
        let mid = vec2(2.0, 2.0);
        assert!((cam.center - mid).length() < 1e-4);
        for shove in [
            vec2(-500.0, -500.0),
            vec2(500.0, 500.0),
            vec2(500.0, -500.0),
        ] {
            cam.pan(shove);
            assert!(
                (cam.center - mid).length() < 1e-4,
                "pinned center drifted to {:?}",
                cam.center
            );
        }
    }

    #[test]
    fn the_cursor_anchor_holds_through_a_multi_step_zoom_glide() {
        // The existing anchor test saturates the glide in one step; this
        // pins that every *intermediate* frame keeps the world point under
        // the cursor stationary, not just the final one.
        let mut cam = camera();
        let cursor = vec2(800.0, 500.0);
        let anchored = cam.to_world(cursor);
        cam.zoom_at(cursor, 1.0);
        for _ in 0..30 {
            cam.update(1.0 / 60.0); // small dt: the glide is mid-flight here
            assert!(
                (cam.to_world(cursor) - anchored).length() < 1e-2,
                "anchor drifted mid-glide to {:?}",
                cam.to_world(cursor)
            );
        }
        assert!(
            (cam.zoom - ZOOM_DEFAULT * 1.15).abs() < 0.01,
            "glide never reached its one-notch target"
        );
    }

    #[test]
    fn panning_past_the_far_corner_stops_within_slack() {
        // `pan_clamps_to_map_with_slack` pins the lo corner; this pins the
        // hi corner. Shoved past the bottom-right, the visible rect reaches
        // the map edge but overshoots it by at most the two-tile slack.
        let mut cam = camera();
        cam.pan(vec2(1000.0, 1000.0));
        let (_, max) = cam.world_rect();
        assert!(
            max.x >= 40.0 && max.y >= 24.0,
            "clamp fell short of the map edge: {max:?}"
        );
        assert!(
            max.x <= 40.0 + 3.0 && max.y <= 24.0 + 3.0,
            "far edge ran away: {max:?}"
        );
    }
}
