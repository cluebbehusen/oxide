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

/// Top-bar height in logical px at 1x scale — the ONE source both the
/// layout's hit-testing and the chrome's drawing read (the duplicated-
/// geometry class stays structurally extinct only while it isn't
/// duplicated).
pub const TOP_BAR_H: f32 = 32.0;

/// Minimum touch target edge in logical px (platform guidance says a
/// fingertip needs ~44).
pub const MIN_TOUCH_TARGET: f32 = 44.0;

/// Pads a hit rect out to the minimum touch target, centered — the
/// TOUCH paths hit-test through this so small chrome stays tappable;
/// mouse paths keep the exact drawn rect.
pub fn touch_pad(rect: Rect, ui: f32) -> Rect {
    let min = MIN_TOUCH_TARGET * ui;
    let grow_w = (min - rect.w).max(0.0);
    let grow_h = (min - rect.h).max(0.0);
    Rect::new(
        rect.x - grow_w * 0.5,
        rect.y - grow_h * 0.5,
        rect.w + grow_w,
        rect.h + grow_h,
    )
}

/// Which side of the rect it describes a tooltip prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipSide {
    /// Above the anchor, left edges aligned — the command band's rule:
    /// the cards sit in a row, so the space above them is free.
    Above,
    /// Right of the anchor, centered on it — the orders dock's rule:
    /// the dock hugs the left edge and stacks upward, so "above" would
    /// land on the neighbouring chip instead of open screen.
    RightOf,
}

/// Places a tooltip box against the rect it describes. `gap` is the
/// clearance from the anchor AND the margin held against the window
/// edges, so callers pass it already ui-scaled.
///
/// Clamping is the point: a tall box on a short window pins under the
/// top bar rather than climbing off screen, and a wide box near the
/// right edge slides back inside. A box with nowhere to fit pins to
/// the top — a clipped tail beats a clipped header.
pub fn tooltip_origin(
    anchor: Rect,
    size: Vec2,
    side: TooltipSide,
    viewport: Vec2,
    top_bar_h: f32,
    gap: f32,
) -> Vec2 {
    let (x, y) = match side {
        TooltipSide::Above => (anchor.x, anchor.y - size.y - gap),
        TooltipSide::RightOf => (
            anchor.x + anchor.w + gap,
            anchor.y + (anchor.h - size.y) * 0.5,
        ),
    };
    let max_x = (viewport.x - size.x - gap).max(0.0);
    let min_y = top_bar_h + gap;
    let max_y = (viewport.y - size.y - gap).max(min_y);
    Vec2::new(x.clamp(0.0, max_x), y.clamp(min_y, max_y))
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
            top_bar_h: TOP_BAR_H * ui,
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

    const VIEW: Vec2 = Vec2::new(1280.0, 800.0);

    #[test]
    fn a_dock_tooltip_tracks_the_chip_it_describes() {
        // Two chips of a full dock, hundreds of px apart: each tooltip
        // centers on ITS chip. Pinning to the band drew both beside the
        // bottom one.
        let size = Vec2::new(240.0, 90.0);
        let top = Rect::new(8.0, 300.0, 44.0, 44.0);
        let bottom = Rect::new(8.0, 620.0, 44.0, 44.0);
        let a = tooltip_origin(top, size, TooltipSide::RightOf, VIEW, 32.0, 6.0);
        let b = tooltip_origin(bottom, size, TooltipSide::RightOf, VIEW, 32.0, 6.0);
        assert_eq!(a.x, 58.0, "clear of the dock, not under the pointer");
        assert_eq!(a.y + size.y * 0.5, top.y + top.h * 0.5);
        assert_eq!(b.y + size.y * 0.5, bottom.y + bottom.h * 0.5);
    }

    #[test]
    fn a_tall_tooltip_stops_at_the_top_bar() {
        // A five-line tooltip raised from a chip near the top of a
        // short window: the header must stay readable, so the box pins
        // below the bar instead of going negative.
        let chip = Rect::new(8.0, 90.0, 44.0, 44.0);
        let size = Vec2::new(240.0, 200.0);
        let o = tooltip_origin(
            chip,
            size,
            TooltipSide::RightOf,
            Vec2::new(1024.0, 400.0),
            32.0,
            6.0,
        );
        assert_eq!(o.y, 38.0, "top bar + gap");
        // Taller than the window has room for: still the top, never a
        // max that fell below the min.
        let o = tooltip_origin(
            chip,
            Vec2::new(240.0, 900.0),
            TooltipSide::RightOf,
            Vec2::new(1024.0, 400.0),
            32.0,
            6.0,
        );
        assert_eq!(o.y, 38.0);
    }

    #[test]
    fn a_wide_tooltip_slides_back_inside_the_window() {
        let card = Rect::new(1150.0, 700.0, 66.0, 80.0);
        let size = Vec2::new(300.0, 60.0);
        let o = tooltip_origin(card, size, TooltipSide::Above, VIEW, 32.0, 6.0);
        assert_eq!(o.x, 974.0, "1280 - 300 - 6");
        assert_eq!(o.y, 634.0, "above the card, not above the band");
        // Wider than the window: pinned left, never a negative origin.
        let o = tooltip_origin(
            card,
            Vec2::new(2000.0, 60.0),
            TooltipSide::Above,
            VIEW,
            32.0,
            6.0,
        );
        assert_eq!(o.x, 0.0);
    }
}
