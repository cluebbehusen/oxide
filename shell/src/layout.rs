//! The one description of where chrome sits this frame.
//!
//! The renderer computes a [`LayoutModel`] as it draws and publishes it
//! on the `Game`; hit-testing reads the same model. There is no second
//! copy of the geometry to fall out of sync — the 0.8 bug where clicks
//! leaked through the palette's second row existed precisely because
//! drawing and hit-testing each kept their own arithmetic. New screens
//! and widgets grow this model rather than freehand math.

use crate::panel::CardAction;
use macroquad::prelude::{Rect, Vec2};

/// Where the persistent HUD chrome sits, in window pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutModel {
    /// Height of the top status bar.
    pub top_bar_h: f32,
    /// Top edge of the bottom panel band; the band runs to the window
    /// bottom. `f32::INFINITY` when no panel is shown.
    pub panel_top: f32,
    /// Right edge of the bottom panel band — the band hugs its content
    /// instead of spanning the window, so clicks past it reach the
    /// world. Zero when no panel is shown.
    pub panel_right: f32,
    /// The orders dock on the left edge (production ghosts / order
    /// chips); zero-sized when the queue is empty.
    pub orders: Rect,
    /// The minimap rectangle.
    pub minimap: Rect,
    /// The idle-worker badge in the top bar; zero-sized when nobody
    /// idles. Clicking it cycles idle harvesters.
    pub idle_badge: Rect,
    /// Command cards: rect plus the action a click performs — the
    /// renderer lays them out, hit-testing replays them verbatim.
    pub cards: [(Rect, CardAction); 16],
    /// How many cards are live this frame.
    pub card_count: usize,
    /// Queue thumbnails (production or orders), same contract.
    pub queue_slots: [(Rect, CardAction); 8],
    /// How many queue slots are live this frame.
    pub queue_count: usize,
}

impl Default for LayoutModel {
    fn default() -> Self {
        Self {
            top_bar_h: 0.0,
            panel_top: f32::INFINITY,
            panel_right: 0.0,
            orders: Rect::new(0.0, 0.0, 0.0, 0.0),
            minimap: Rect::new(0.0, 0.0, 0.0, 0.0),
            idle_badge: Rect::new(0.0, 0.0, 0.0, 0.0),
            cards: [(Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::None); 16],
            card_count: 0,
            queue_slots: [(Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::None); 8],
            queue_count: 0,
        }
    }
}

impl LayoutModel {
    /// Computes the frame's chrome geometry. `panel_top` is the band's
    /// top edge (`f32::INFINITY` when no panel is shown).
    #[allow(clippy::too_many_arguments)]
    pub fn compute(
        _viewport: Vec2,
        ui: f32,
        panel_top: f32,
        panel_right: f32,
        orders: Rect,
        minimap: Rect,
        idle_badge: Rect,
        cards: [(Rect, CardAction); 16],
        card_count: usize,
        queue_slots: [(Rect, CardAction); 8],
        queue_count: usize,
    ) -> Self {
        Self {
            top_bar_h: 32.0 * ui,
            panel_top,
            panel_right,
            orders,
            minimap,
            idle_badge,
            cards,
            card_count,
            queue_slots,
            queue_count,
        }
    }

    /// Whether persistent chrome (top bar or panel band) owns this
    /// point — such clicks must never reach the world. The minimap has
    /// its own richer meaning and is tested separately.
    pub fn chrome_owns(&self, p: Vec2) -> bool {
        p.y <= self.top_bar_h
            || (p.y >= self.panel_top && p.x <= self.panel_right)
            || (self.orders.w > 0.0 && self.orders.contains(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    fn compute_at(panel_top: f32, ui: f32) -> LayoutModel {
        let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
        LayoutModel::compute(
            vec2(1280.0, 800.0),
            ui,
            panel_top,
            1280.0,
            zero,
            zero,
            zero,
            [(zero, CardAction::None); 16],
            0,
            [(zero, CardAction::None); 8],
            0,
        )
    }

    #[test]
    fn the_panel_band_owns_exactly_below_its_top() {
        let m = compute_at(700.0, 1.0);
        assert!(m.chrome_owns(vec2(600.0, 770.0)));
        assert!(m.chrome_owns(vec2(600.0, 700.0)));
        assert!(
            !m.chrome_owns(vec2(600.0, 699.0)),
            "the band must not swallow the midfield"
        );
    }

    #[test]
    fn the_band_owns_its_width_and_the_dock_its_rect() {
        let mut m = compute_at(700.0, 1.0);
        m.panel_right = 600.0;
        m.orders = Rect::new(0.0, 500.0, 60.0, 200.0);
        assert!(m.chrome_owns(vec2(400.0, 750.0)), "inside the band");
        assert!(
            !m.chrome_owns(vec2(900.0, 750.0)),
            "right of a content-width band is world, not chrome"
        );
        assert!(
            m.chrome_owns(vec2(30.0, 600.0)),
            "the orders dock is chrome"
        );
        assert!(
            !m.chrome_owns(vec2(100.0, 600.0)),
            "beside the dock stays world"
        );
    }

    #[test]
    fn no_panel_means_no_band_at_all() {
        let m = compute_at(f32::INFINITY, 2.0);
        assert!(!m.chrome_owns(vec2(600.0, 799.0)));
        assert!(m.chrome_owns(vec2(600.0, 30.0)), "the top bar always owns");
    }
}
