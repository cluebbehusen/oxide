//! The one description of where chrome sits this frame.
//!
//! The renderer computes a [`LayoutModel`] as it draws and publishes it
//! on the `Game`; hit-testing reads the same model. There is no second
//! copy of the geometry to fall out of sync — the 0.8 bug where clicks
//! leaked through the palette's second row existed precisely because
//! drawing and hit-testing each kept their own arithmetic. New screens
//! and widgets grow this model rather than freehand math.

use macroquad::prelude::{Rect, Vec2};

/// Where the persistent HUD chrome sits, in window pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutModel {
    /// Height of the top status bar.
    pub top_bar_h: f32,
    /// Top edge of the bottom panel band; the band runs to the window
    /// bottom. `f32::INFINITY` when no panel is shown.
    pub panel_top: f32,
    /// The minimap rectangle.
    pub minimap: Rect,
}

impl Default for LayoutModel {
    fn default() -> Self {
        Self {
            top_bar_h: 0.0,
            panel_top: f32::INFINITY,
            minimap: Rect::new(0.0, 0.0, 0.0, 0.0),
        }
    }
}

impl LayoutModel {
    /// Computes the frame's chrome geometry. `panel_rows` is how many
    /// bottom rows the HUD actually drew (zero = no panel).
    pub fn compute(viewport: Vec2, ui: f32, panel_rows: usize, minimap: Rect) -> Self {
        Self {
            top_bar_h: 32.0 * ui,
            panel_top: if panel_rows == 0 {
                f32::INFINITY
            } else {
                viewport.y - 36.0 * ui * panel_rows as f32
            },
            minimap,
        }
    }

    /// Whether persistent chrome (top bar or panel band) owns this
    /// point — such clicks must never reach the world. The minimap has
    /// its own richer meaning and is tested separately.
    pub fn chrome_owns(&self, p: Vec2) -> bool {
        p.y <= self.top_bar_h || p.y >= self.panel_top
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    #[test]
    fn the_panel_band_scales_with_its_row_count() {
        let mini = Rect::new(0.0, 0.0, 0.0, 0.0);
        let one = LayoutModel::compute(vec2(1280.0, 800.0), 1.0, 1, mini);
        let three = LayoutModel::compute(vec2(1280.0, 800.0), 1.0, 3, mini);
        assert!(one.chrome_owns(vec2(600.0, 770.0)));
        assert!(
            !one.chrome_owns(vec2(600.0, 700.0)),
            "a single row must not swallow the midfield"
        );
        assert!(
            three.chrome_owns(vec2(600.0, 700.0)),
            "three rows reach higher"
        );
    }

    #[test]
    fn no_panel_means_no_band_at_all() {
        let m = LayoutModel::compute(vec2(1280.0, 800.0), 2.0, 0, Rect::new(0.0, 0.0, 0.0, 0.0));
        assert!(!m.chrome_owns(vec2(600.0, 799.0)));
        assert!(m.chrome_owns(vec2(600.0, 30.0)), "the top bar always owns");
    }
}
