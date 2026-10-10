//! The map browser: a thumbnail grid, sectioned by format.
//!
//! Cards carry the fog-free preview, the map's name, and its pace
//! badge; sections ("1v1", "2v2", …) are drawn bands, not rows, so the
//! grouping is visible at a glance and the cursor only ever rests on a
//! map. Layout is a pure function of (entries, viewport, scale), which
//! keeps every hit-test headless-testable; drawing reads the same
//! rects it publishes.

use crate::menu::{PreviewCache, ScenarioEntry};
use crate::nav::{Nav, step_grid};
use crate::numeric;
use crate::press::{ScrollPress, Swipe};
use crate::render::prim::{fill_rect, stroke_rect};
use macroquad::prelude::{
    Color, DrawTextureParams, Rect, Vec2, draw_rectangle, draw_text, draw_texture_ex, measure_text,
    vec2,
};
use oxide_protocol::{Key, RawEvent};

use crate::theme::{SURFACE_MENU, TEXT_PRIMARY, TEXT_SECONDARY, TEXT_TITLE};

/// What a frame of browser input decided.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Out {
    /// Still browsing.
    Stay,
    /// Escape: back to the front door.
    Back,
    /// A map was activated (entry index).
    Pick(usize),
}

/// One vertical slot of the grid: a section band or a row of cards.
enum Line {
    Heading(String),
    Cards(Vec<usize>),
}

/// The grid's frame geometry: everything visible, in screen rects.
pub struct Layout {
    /// Section bands: label and its text anchor rect.
    pub headings: Vec<(String, Rect)>,
    /// Visible cards: entry index and its full card rect.
    pub cards: Vec<(usize, Rect)>,
    /// Whether rows are clipped above / below the window.
    pub more_above: bool,
    /// See `more_above`.
    pub more_below: bool,
    /// Pixel offset into the shelf.
    pub scroll_offset: f32,
    /// Furthest valid pixel offset.
    pub scroll_max: f32,
    /// Height of the visible shelf window.
    pub viewport_height: f32,
    /// Full height of the shelf's contents.
    pub content_height: f32,
}

/// Grid state: the selected entry, scroll, and pointer bookkeeping.
pub struct Browser {
    /// Selected entry index (into the discovery list).
    pub selected: usize,
    /// Pixel offset into the grid.
    scroll_y: f32,
    /// The card under the pointer — exposed (read-only) through the
    /// wizard's protocol surface so hover-driven row discovery in the
    /// UX battery works on the grid like it does on row menus.
    pub(crate) hover: Option<usize>,
    /// The card a click or tap armed; a finger that drags scrolls the
    /// grid instead.
    press: ScrollPress<usize>,
    /// The viewport the last frame handled, so the snap-back guard fires
    /// only on resize.
    last_view: Vec2,
}

/// The section heading for a seat count.
fn heading(seats: usize) -> String {
    match seats {
        2 => "1v1".to_string(),
        n if n % 2 == 0 => format!("{}v{}", n / 2, n / 2),
        n => format!("{n} seats"),
    }
}

fn columns(view_w: f32, ui: f32) -> usize {
    if view_w < 980.0 * ui { 3 } else { 4 }
}

/// The vertical line list: section bands interleaved with card rows.
fn lines(entries: &[ScenarioEntry], cols: usize) -> Vec<Line> {
    let mut out = Vec::new();
    let mut row: Vec<usize> = Vec::new();
    let mut last_seats = 0;
    for (i, e) in entries.iter().enumerate() {
        if e.seats != last_seats {
            if !row.is_empty() {
                out.push(Line::Cards(std::mem::take(&mut row)));
            }
            out.push(Line::Heading(heading(e.seats)));
            last_seats = e.seats;
        }
        row.push(i);
        if row.len() == cols {
            out.push(Line::Cards(std::mem::take(&mut row)));
        }
    }
    if !row.is_empty() {
        out.push(Line::Cards(row));
    }
    out
}

/// Height of one grid line, including the gap after a card row.
fn line_height(line: &Line, card_h: f32, heading_h: f32, gap: f32) -> f32 {
    match line {
        Line::Heading(_) => heading_h,
        Line::Cards(_) => card_h + gap,
    }
}

/// Full content height. The final row does not owe a trailing gap.
fn content_height(all: &[Line], card_h: f32, heading_h: f32, gap: f32) -> f32 {
    let height: f32 = all
        .iter()
        .map(|line| line_height(line, card_h, heading_h, gap))
        .sum();
    if matches!(all.last(), Some(Line::Cards(_))) {
        (height - gap).max(0.0)
    } else {
        height
    }
}

/// Furthest valid pixel offset. At the clamp the tail sits against the
/// bottom of the window, so scrolling can never park one row above an
/// otherwise empty screen.
fn max_scroll(all: &[Line], view: Vec2, ui: f32) -> f32 {
    let (_, _, _, card_h, heading_h, top, bottom) = metrics(view, ui);
    let gap = 16.0 * ui;
    (content_height(all, card_h, heading_h, gap) - (bottom - top)).max(0.0)
}

/// Card and band sizes at this viewport. Returns
/// (`band_x`, `band_w`, `card_w`, `card_h`, `heading_h`, top, bottom).
fn metrics(view: Vec2, ui: f32) -> (f32, f32, f32, f32, f32, f32, f32) {
    let cols = columns(view.x, ui) as f32;
    let band_w = (view.x - 96.0 * ui).min(1120.0 * ui);
    let band_x = (view.x - band_w) * 0.5;
    let gap = 12.0 * ui;
    let card_w = (band_w - gap * (cols - 1.0)) / cols;
    let heading_h = 30.0 * ui;
    // Margins yield before content does: a small window (or a large
    // UI scale) compresses the title and hint zones first.
    let top = (108.0 * ui).min(view.y * 0.22);
    let bottom = view.y - (76.0 * ui).min(view.y * 0.14);
    // At least one heading and card row must always fit the window, or
    // the grid draws nothing while Enter still activates the hidden
    // selection. The 16ui row gap matches `layout()`. The floor is
    // physical: when even compressed chrome can't afford it, cards run
    // small but never to zero.
    let card_h = (card_w * 0.5 + 26.0 * ui)
        .min(bottom - top - heading_h - 16.0 * ui)
        .max(40.0);
    (band_x, band_w, card_w, card_h, heading_h, top, bottom)
}

/// The map grid's coaching line.
fn browser_hint(touch_only: bool) -> &'static str {
    if touch_only {
        "tap a map to select it - tap it again to play"
    } else {
        "{up}/{down} or click select - {confirm} or click again plays - {back} back"
    }
}

impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

impl Browser {
    /// A fresh browser on the first map.
    pub fn new() -> Self {
        Self {
            selected: 0,
            scroll_y: 0.0,
            hover: None,
            press: ScrollPress::default(),
            last_view: vec2(0.0, 0.0),
        }
    }

    /// Re-selects the remembered map by path, so section sorts never move
    /// the highlight onto a different map.
    pub fn select_path(&mut self, entries: &[ScenarioEntry], path: Option<&std::path::Path>) {
        if let Some(i) = entries.iter().position(|e| e.path.as_deref() == path) {
            self.selected = i;
        }
        self.selected = self.selected.min(entries.len().saturating_sub(1));
        self.ensure_visible(entries);
    }

    /// The frame's visible geometry.
    pub fn layout(&self, entries: &[ScenarioEntry], view: Vec2, ui: f32) -> Layout {
        let (band_x, band_w, card_w, card_h, heading_h, top, bottom) = metrics(view, ui);
        let cols = columns(view.x, ui);
        let gap = 16.0 * ui;
        let all = lines(entries, cols);
        let content_height = content_height(&all, card_h, heading_h, gap);
        let viewport_height = bottom - top;
        let scroll_max = (content_height - viewport_height).max(0.0);
        let scroll_offset = self.scroll_y.clamp(0.0, scroll_max);
        let mut y = top - scroll_offset;
        let mut headings = Vec::new();
        let mut cards = Vec::new();
        for line in &all {
            let advance = line_height(line, card_h, heading_h, gap);
            let drawn_h = match line {
                Line::Heading(_) => heading_h,
                Line::Cards(_) => card_h,
            };
            if y + drawn_h > top && y < bottom {
                match line {
                    Line::Heading(label) => {
                        headings.push((label.clone(), Rect::new(band_x, y, band_w, heading_h)));
                    }
                    Line::Cards(row) => {
                        for (ci, &entry) in row.iter().enumerate() {
                            let x = band_x + ci as f32 * (card_w + gap);
                            cards.push((entry, Rect::new(x, y, card_w, card_h)));
                        }
                    }
                }
            }
            y += advance;
        }
        Layout {
            headings,
            cards,
            more_above: scroll_offset > 0.5,
            more_below: scroll_offset + 0.5 < scroll_max,
            scroll_offset,
            scroll_max,
            viewport_height,
            content_height,
        }
    }

    /// (line index, column) of an entry in the grid.
    fn locate(entries: &[ScenarioEntry], cols: usize, entry: usize) -> (usize, usize) {
        for (li, line) in lines(entries, cols).iter().enumerate() {
            if let Line::Cards(row) = line
                && let Some(ci) = row.iter().position(|e| *e == entry)
            {
                return (li, ci);
            }
        }
        (0, 0)
    }

    fn ensure_visible(&mut self, entries: &[ScenarioEntry]) {
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let cols = columns(view.x, ui);
        let all = lines(entries, cols);
        let (_, _, _, card_h, heading_h, top, bottom) = metrics(view, ui);
        let gap = 16.0 * ui;
        let (li, _) = Self::locate(entries, cols, self.selected);
        let mut line_top = 0.0;
        for line in &all[..li] {
            line_top += line_height(line, card_h, heading_h, gap);
        }
        let line_bottom = line_top + card_h;
        let viewport_h = bottom - top;
        if line_top < self.scroll_y {
            // Keep the section label with its first row when possible.
            self.scroll_y = if li > 0 && matches!(all[li - 1], Line::Heading(_)) {
                line_top - heading_h
            } else {
                line_top
            };
        } else if line_bottom > self.scroll_y + viewport_h {
            self.scroll_y = line_bottom - viewport_h;
        }
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll(&all, view, ui));
    }

    /// Feeds a frame of events; `mouse` tracks the pointer like the
    /// menus do.
    pub fn handle(
        &mut self,
        entries: &[ScenarioEntry],
        events: &[RawEvent],
        mouse: &mut Vec2,
    ) -> Out {
        if entries.is_empty() {
            return Out::Stay;
        }
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let cols = columns(view.x, ui);
        // A resize can shrink the window out from under the selection:
        // `layout()` recomputes each frame but scroll state does not. The
        // guard fires on resize only; run per frame, it would snap every
        // wheel scroll straight back to the selection.
        if self.last_view != view {
            self.last_view = view;
            let max = max_scroll(&lines(entries, cols), view, ui);
            self.scroll_y = self.scroll_y.clamp(0.0, max);
            let layout = self.layout(entries, view, ui);
            let selected_fully_visible = layout.cards.iter().any(|(entry, rect)| {
                *entry == self.selected
                    && rect.y >= metrics(view, ui).5
                    && rect.y + rect.h <= metrics(view, ui).6
            });
            if !layout.cards.is_empty() && !selected_fully_visible {
                self.ensure_visible(entries);
            }
        }
        let (_, _, _, _, _, shelf_top, shelf_bottom) = metrics(view, ui);
        let card_at = |browser: &Self, p: Vec2| {
            if p.y < shelf_top || p.y >= shelf_bottom {
                return None;
            }
            browser
                .layout(entries, view, ui)
                .cards
                .iter()
                .find(|(_, r)| r.contains(p))
                .map(|(e, _)| *e)
        };
        for event in events {
            // A finger landing outside the grid neither taps nor drags it.
            if matches!(*event, RawEvent::TouchDown { y, .. } if y < shelf_top || y >= shelf_bottom)
            {
                continue;
            }
            let mut press = self.press;
            let swipe = press.feed(event, ui, |p, _| card_at(self, p));
            self.press = press;
            match swipe {
                Swipe::Scrolled { dy, .. } => {
                    let max = max_scroll(&lines(entries, cols), view, ui);
                    self.scroll_y = (self.scroll_y - dy).clamp(0.0, max);
                    self.hover = None;
                    continue;
                }
                Swipe::Activated(card) => {
                    if let Some(p) = crate::press::position(event) {
                        *mouse = p;
                    }
                    // First click selects; a click on the already-selected
                    // card commits. Browsing by pointer can't misfire a
                    // launch, and the double-click reflex reads as
                    // select-then-play.
                    if card == self.selected {
                        return Out::Pick(card);
                    }
                    self.selected = card;
                    continue;
                }
                Swipe::Held | Swipe::Ignored => {}
            }
            match *event {
                RawEvent::KeyDown { key: Key::Escape } => return Out::Back,
                RawEvent::KeyDown { key: Key::Enter } => {
                    // Enter never fires a card the player can't see:
                    // an off-screen selection scrolls into view first,
                    // and the second Enter commits.
                    let shown = self.layout(entries, view, ui);
                    if shown.cards.iter().any(|(entry, rect)| {
                        *entry == self.selected
                            && rect.y >= shelf_top
                            && rect.y + rect.h <= shelf_bottom
                    }) {
                        return Out::Pick(self.selected);
                    }
                    self.ensure_visible(entries);
                }
                RawEvent::KeyDown { .. } => {
                    let Some(nav) = Nav::decode(event) else {
                        continue;
                    };
                    let rows: Vec<usize> = lines(entries, cols)
                        .iter()
                        .filter_map(|line| match line {
                            Line::Cards(row) => Some(row.len()),
                            Line::Heading(_) => None,
                        })
                        .collect();
                    let (_, _, _, card_h, _, top, bottom) = metrics(view, ui);
                    let page_rows =
                        numeric::to_usize(((bottom - top) / (card_h + 16.0 * ui)).floor());
                    if let Some(next) = step_grid(&rows, self.selected, nav, page_rows) {
                        self.selected = next;
                        if nav == Nav::Home {
                            self.scroll_y = 0.0;
                        } else {
                            self.ensure_visible(entries);
                        }
                    }
                }
                RawEvent::Wheel { delta } => {
                    if !delta.is_finite() {
                        continue;
                    }
                    let max = max_scroll(&lines(entries, cols), view, ui);
                    let wheel_pixels = 56.0 * ui;
                    self.scroll_y = (self.scroll_y - delta * wheel_pixels).clamp(0.0, max);
                    self.hover = card_at(self, *mouse);
                    // The wheel scrolls without moving the selection.
                }
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(x, y);
                    self.hover = card_at(self, *mouse);
                }
                RawEvent::TouchDown { id, x, y } if self.press.owns(id) => {
                    *mouse = vec2(x, y);
                    self.hover = card_at(self, *mouse);
                }
                RawEvent::TouchUp { .. } => self.hover = None,
                _ => {}
            }
        }
        Out::Stay
    }

    /// Draws the whole screen: title, sections, cards, the selected
    /// map's blurb, and the key hints.
    pub fn draw(&self, entries: &[ScenarioEntry], previews: &mut PreviewCache) {
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let layout = self.layout(entries, view, ui);
        for (label, rect) in &layout.headings {
            draw_text(label, rect.x, rect.y + rect.h * 0.62, 22.0 * ui, TEXT_TITLE);
            let dims = measure_text(label, None, numeric::font_size(22.0 * ui), 1.0);
            draw_rectangle(
                rect.x + dims.width + 14.0 * ui,
                rect.y + rect.h * 0.5,
                rect.w - dims.width - 14.0 * ui,
                1.0,
                Color::new(0.6, 0.6, 0.65, 0.25),
            );
        }
        for (entry_idx, rect) in &layout.cards {
            let entry = &entries[*entry_idx];
            let selected = *entry_idx == self.selected;
            let hovered = self.hover == Some(*entry_idx);
            fill_rect(*rect, SURFACE_MENU);
            let label_h = 30.0 * ui;
            let thumb = Rect::new(
                rect.x + 4.0 * ui,
                rect.y + 4.0 * ui,
                rect.w - 8.0 * ui,
                rect.h - label_h - 8.0 * ui,
            );
            if let Some(tex) = previews.get(entry) {
                let scale = (thumb.w / tex.width()).min(thumb.h / tex.height());
                let (pw, ph) = (tex.width() * scale, tex.height() * scale);
                draw_texture_ex(
                    tex,
                    thumb.x + (thumb.w - pw) * 0.5,
                    thumb.y + (thumb.h - ph) * 0.5,
                    crate::render::theme_tint(&entry.theme),
                    DrawTextureParams {
                        dest_size: Some(vec2(pw, ph)),
                        ..Default::default()
                    },
                );
            }
            let border = if selected {
                TEXT_TITLE
            } else if hovered {
                TEXT_SECONDARY
            } else {
                Color::new(0.6, 0.6, 0.65, 0.25)
            };
            stroke_rect(*rect, if selected { 3.0 } else { 1.5 }, border);
            let name_size = 17.0 * ui;
            let name = measure_text(&entry.label, None, numeric::font_size(name_size), 1.0);
            draw_text(
                &entry.label,
                rect.x + (rect.w - name.width) * 0.5,
                rect.y + rect.h - 10.0 * ui,
                name_size,
                if selected {
                    TEXT_PRIMARY
                } else {
                    TEXT_SECONDARY
                },
            );
        }
        // Edge rows stay at their true translated positions so wheel
        // and touch input move continuously. Opaque chrome masks clip
        // the portions outside the shelf.
        let (band_x, band_w, _, _, _, top, bottom) = metrics(view, ui);
        draw_rectangle(0.0, 0.0, view.x, top, crate::render::OUTSIDE);
        draw_rectangle(
            0.0,
            bottom,
            view.x,
            (view.y - bottom).max(0.0),
            crate::render::OUTSIDE,
        );
        let title_size = 64.0 * ui;
        let dims = measure_text("OXIDE", None, numeric::font_size(title_size), 1.0);
        draw_text(
            "OXIDE",
            (view.x - dims.width) * 0.5,
            72.0 * ui,
            title_size,
            TEXT_TITLE,
        );
        // A draw-only scrollbar thumb showing where the window sits in
        // the shelf.
        if layout.more_above || layout.more_below {
            let track_h = bottom - top;
            let thumb_h = if layout.content_height > 0.0 {
                (layout.viewport_height / layout.content_height * track_h).clamp(24.0 * ui, track_h)
            } else {
                track_h
            };
            let thumb_top = if layout.scroll_max > 0.0 {
                top + layout.scroll_offset / layout.scroll_max * (track_h - thumb_h)
            } else {
                top
            };
            let x = (band_x + band_w + 10.0 * ui).min(view.x - 8.0 * ui);
            draw_rectangle(x, top, 3.0 * ui, track_h, SURFACE_MENU);
            draw_rectangle(x, thumb_top, 3.0 * ui, thumb_h, TEXT_SECONDARY);
        }
        // The selected map's blurb, above the hint line.
        if let Some(entry) = entries.get(self.selected) {
            let blurb = entry
                .blurb
                .clone()
                .unwrap_or_else(|| "machines eating a dead world".to_string());
            let mut size = 18.0 * ui;
            let mut dims = measure_text(&blurb, None, numeric::font_size(size), 1.0);
            let max = view.x * 0.8;
            if dims.width > max {
                size = (size * max / dims.width).max(12.0 * ui);
                dims = measure_text(&blurb, None, numeric::font_size(size), 1.0);
            }
            draw_text(
                &blurb,
                (view.x - dims.width) * 0.5,
                view.y - 44.0 * ui,
                size,
                TEXT_PRIMARY,
            );
        }
        let hint = crate::menu::binding_hint(browser_hint(crate::platform::TOUCH_ONLY));
        let dims = measure_text(&hint, None, numeric::font_size(16.0 * ui), 1.0);
        draw_text(
            &hint,
            (view.x - dims.width) * 0.5,
            view.y - 20.0 * ui,
            16.0 * ui,
            crate::hints::fade(TEXT_SECONDARY),
        );
    }
}

#[cfg(test)]
mod tests;
