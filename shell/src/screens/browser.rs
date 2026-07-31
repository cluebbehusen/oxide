//! The map browser: a thumbnail grid, sectioned by format.
//!
//! Cards carry the fog-free preview, the map's name, and its pace
//! badge; sections ("1v1", "2v2", …) are drawn bands, not rows, so the
//! grouping is visible at a glance and the cursor only ever rests on a
//! map. Layout is a pure function of (entries, viewport, scale), which
//! keeps every hit-test headless-testable; drawing reads the same
//! rects it publishes.

use crate::menu::{PreviewCache, ScenarioEntry};
use macroquad::prelude::{
    Color, DrawTextureParams, Rect, Vec2, draw_rectangle, draw_rectangle_lines, draw_text,
    draw_texture_ex, measure_text, vec2,
};
use oxide_protocol::{Key, MouseButton, RawEvent};

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
    pressed: Option<usize>,
    touch_id: Option<u64>,
    touch_last_y: f32,
    touch_travel: f32,
    touch_pressed: Option<usize>,
    /// The viewport the last frame handled — resize detection for the
    /// snap-back guard, which must fire on resize and NEVER per frame.
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
/// (band_x, band_w, card_w, card_h, heading_h, top, bottom).
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
    // At least one heading + card row must ALWAYS fit the window, or
    // the grid draws nothing while Enter still activates the hidden
    // selection. The 16ui row gap below matches layout().
    // The floor is PHYSICAL: when even compressed chrome can't afford
    // it, cards run small but never to zero — a zero-height card is
    // unclickable while Enter still fires the hidden selection.
    let card_h = (card_w * 0.5 + 26.0 * ui)
        .min(bottom - top - heading_h - 16.0 * ui)
        .max(40.0);
    (band_x, band_w, card_w, card_h, heading_h, top, bottom)
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
            pressed: None,
            touch_id: None,
            touch_last_y: 0.0,
            touch_travel: 0.0,
            touch_pressed: None,
            last_view: vec2(0.0, 0.0),
        }
    }

    /// Re-selects the remembered map by PATH (section sorts must never
    /// move the highlight onto a different map).
    pub fn select_path(&mut self, entries: &[ScenarioEntry], path: &Option<std::path::PathBuf>) {
        if let Some(i) = entries.iter().position(|e| &e.path == path) {
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
        // A resize can shrink the window out from under the selection —
        // layout() recomputes each frame but scroll state does not.
        // The guard fires on RESIZE ONLY: run per frame it would snap
        // every wheel scroll straight back to the selection, and the
        // shelf's lower half could never be reached by trackpad.
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
        let last = entries.len() - 1;
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
            match *event {
                RawEvent::KeyDown { key: Key::Escape } => return Out::Back,
                RawEvent::KeyDown { key: Key::Enter } => {
                    // Enter never fires a card the player can't see:
                    // an off-screen selection scrolls into view first,
                    // and the SECOND Enter commits.
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
                RawEvent::KeyDown { key: Key::Left } => {
                    self.selected = self.selected.saturating_sub(1);
                    self.ensure_visible(entries);
                }
                RawEvent::KeyDown { key: Key::Right } => {
                    self.selected = (self.selected + 1).min(last);
                    self.ensure_visible(entries);
                }
                RawEvent::KeyDown { key: Key::Up } | RawEvent::KeyDown { key: Key::Down } => {
                    let down = matches!(*event, RawEvent::KeyDown { key: Key::Down });
                    let all = lines(entries, cols);
                    let (li, ci) = Self::locate(entries, cols, self.selected);
                    let mut target = li as i64;
                    loop {
                        target += if down { 1 } else { -1 };
                        if target < 0 || target as usize >= all.len() {
                            break;
                        }
                        if let Line::Cards(row) = &all[target as usize] {
                            self.selected = row[ci.min(row.len() - 1)];
                            break;
                        }
                    }
                    self.ensure_visible(entries);
                }
                RawEvent::KeyDown { key: Key::Home } => {
                    self.selected = 0;
                    self.scroll_y = 0.0;
                }
                RawEvent::KeyDown { key: Key::End } => {
                    self.selected = last;
                    self.ensure_visible(entries);
                }
                RawEvent::Wheel { delta } => {
                    if !delta.is_finite() {
                        continue;
                    }
                    let max = max_scroll(&lines(entries, cols), view, ui);
                    let wheel_pixels = 56.0 * ui;
                    self.scroll_y = (self.scroll_y - delta * wheel_pixels).clamp(0.0, max);
                    self.hover = card_at(self, *mouse);
                    // Browsing is not choosing: the wheel moves the
                    // window and ONLY the window. (The old drag-along
                    // rule silently retargeted Enter while the player
                    // was just looking; Enter now scrolls an
                    // off-screen selection back instead.)
                }
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(x, y);
                    self.hover = card_at(self, *mouse);
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    self.pressed = card_at(self, vec2(x, y));
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    let released = card_at(self, vec2(x, y));
                    let armed = self.pressed.take();
                    if let (Some(a), Some(r)) = (armed, released)
                        && a == r
                    {
                        // First click selects; a click on the already-
                        // selected card commits. Browsing by pointer
                        // can't misfire a launch, and the double-click
                        // reflex reads as select-then-play.
                        if a == self.selected {
                            return Out::Pick(a);
                        }
                        self.selected = a;
                    }
                }
                RawEvent::TouchDown { id, x, y } => {
                    if self.touch_id.is_none() && y >= shelf_top && y < shelf_bottom {
                        let point = vec2(x, y);
                        self.touch_id = Some(id);
                        self.touch_last_y = y;
                        self.touch_travel = 0.0;
                        self.touch_pressed = card_at(self, point);
                        *mouse = point;
                        self.hover = self.touch_pressed;
                    }
                }
                RawEvent::TouchMove { id, x, y } if self.touch_id == Some(id) => {
                    let dy = y - self.touch_last_y;
                    self.touch_last_y = y;
                    self.touch_travel += dy.abs();
                    let max = max_scroll(&lines(entries, cols), view, ui);
                    self.scroll_y = (self.scroll_y - dy).clamp(0.0, max);
                    *mouse = vec2(x, y);
                    self.hover = None;
                }
                RawEvent::TouchUp { id, x, y } if self.touch_id == Some(id) => {
                    let released = card_at(self, vec2(x, y));
                    let armed = self.touch_pressed.take();
                    self.touch_id = None;
                    let was_tap = self.touch_travel <= 8.0 * ui;
                    self.touch_travel = 0.0;
                    self.hover = None;
                    if was_tap
                        && let (Some(a), Some(r)) = (armed, released)
                        && a == r
                    {
                        if a == self.selected {
                            return Out::Pick(a);
                        }
                        self.selected = a;
                    }
                }
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
            let dims = measure_text(label, None, (22.0 * ui) as u16, 1.0);
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
            draw_rectangle(rect.x, rect.y, rect.w, rect.h, SURFACE_MENU);
            let label_h = 30.0 * ui;
            let thumb = Rect::new(
                rect.x + 4.0 * ui,
                rect.y + 4.0 * ui,
                rect.w - 8.0 * ui,
                rect.h - label_h - 8.0 * ui,
            );
            if let Some(tex) = previews.get(*entry_idx, entry) {
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
            draw_rectangle_lines(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                if selected { 3.0 } else { 1.5 },
                border,
            );
            let name_size = 17.0 * ui;
            let name = measure_text(&entry.label, None, name_size as u16, 1.0);
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
        let dims = measure_text("OXIDE", None, title_size as u16, 1.0);
        draw_text(
            "OXIDE",
            (view.x - dims.width) * 0.5,
            72.0 * ui,
            title_size,
            TEXT_TITLE,
        );
        // Scroll cues.
        if layout.more_above {
            draw_text(
                "^",
                band_x + band_w * 0.5,
                110.0 * ui,
                22.0 * ui,
                TEXT_SECONDARY,
            );
        }
        if layout.more_below {
            draw_text(
                "v",
                band_x + band_w * 0.5,
                view.y - 66.0 * ui,
                22.0 * ui,
                TEXT_SECONDARY,
            );
        }
        // A draw-only scrollbar thumb: where the window sits in the
        // shelf, at a glance. The wheel is the drag; this just tells
        // the truth about how much shelf is off screen.
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
        // The selected map's story, above the hint line.
        if let Some(entry) = entries.get(self.selected) {
            let blurb = entry
                .blurb
                .clone()
                .unwrap_or_else(|| "machines eating a dead world".to_string());
            let mut size = 18.0 * ui;
            let mut dims = measure_text(&blurb, None, size as u16, 1.0);
            let max = view.x * 0.8;
            if dims.width > max {
                size = (size * max / dims.width).max(12.0 * ui);
                dims = measure_text(&blurb, None, size as u16, 1.0);
            }
            draw_text(
                &blurb,
                (view.x - dims.width) * 0.5,
                view.y - 44.0 * ui,
                size,
                TEXT_PRIMARY,
            );
        }
        let hint = "Arrows/click select - Enter or click again plays - Esc back";
        let dims = measure_text(hint, None, (16.0 * ui) as u16, 1.0);
        draw_text(
            hint,
            (view.x - dims.width) * 0.5,
            view.y - 20.0 * ui,
            16.0 * ui,
            TEXT_SECONDARY,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(label: &str, seats: usize) -> ScenarioEntry {
        ScenarioEntry {
            seats,
            label: label.to_string(),
            blurb: None,
            path: Some(std::path::PathBuf::from(format!("{label}.json"))),
            theme: String::new(),
        }
    }

    fn shelf() -> Vec<ScenarioEntry> {
        let mut v: Vec<ScenarioEntry> = (0..9).map(|i| entry(&format!("m{i}"), 2)).collect();
        v.push(entry("t0", 4));
        v.push(entry("t1", 4));
        v
    }

    fn press(b: &mut Browser, entries: &[ScenarioEntry], key: Key) -> Out {
        let mut mouse = vec2(0.0, 0.0);
        b.handle(entries, &[RawEvent::KeyDown { key }], &mut mouse)
    }

    #[test]
    fn arrows_walk_the_grid_by_row_and_column() {
        let entries = shelf();
        let mut b = Browser::new();
        press(&mut b, &entries, Key::Right);
        assert_eq!(b.selected, 1);
        press(&mut b, &entries, Key::Down);
        assert_eq!(b.selected, 5, "down keeps the column (4-wide grid)");
        press(&mut b, &entries, Key::Down);
        assert_eq!(b.selected, 8, "a short row clamps the column");
        press(&mut b, &entries, Key::Down);
        assert_eq!(b.selected, 9, "and the next step crosses the section");
        press(&mut b, &entries, Key::Up);
        assert_eq!(b.selected, 8);
        press(&mut b, &entries, Key::Up);
        assert_eq!(
            b.selected, 4,
            "the column narrows through a one-wide row (standard grid feel)"
        );
    }

    #[test]
    fn wheel_scroll_moves_the_window_and_only_the_window() {
        let entries = shelf();
        let mut b = Browser::new();
        let mut mouse = vec2(0.0, 0.0);
        // Scroll far past the first section ACROSS SEPARATE FRAMES —
        // the frame-start guard once snapped the window back to the
        // selection between events, so a single-call test proved
        // nothing. Browsing must not retarget Enter.
        for _ in 0..8 {
            b.handle(&entries, &[RawEvent::Wheel { delta: -1.0 }], &mut mouse);
            b.handle(&entries, &[], &mut mouse);
            assert_eq!(b.selected, 0, "the wheel chose a map");
        }
        // The window moved (and stopped with its tail against the
        // viewport — see `the_wheel_stops_at_the_last_full_screenful`).
        assert!(b.scroll_y > 0.0, "the window actually moved");

        // Enter with the selection off screen scrolls it back first;
        // only the second Enter commits — it never fires blind.
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let (_, _, _, _, _, shelf_top, shelf_bottom) = metrics(view, ui);
        assert!(
            !b.layout(&entries, view, ui)
                .cards
                .iter()
                .any(|(e, rect)| *e == b.selected
                    && rect.y >= shelf_top
                    && rect.y + rect.h <= shelf_bottom),
            "precondition: the selection is not fully visible"
        );
        let out = b.handle(
            &entries,
            &[RawEvent::KeyDown { key: Key::Enter }],
            &mut mouse,
        );
        assert_eq!(out, Out::Stay, "the first Enter only scrolls back");
        assert!(
            b.layout(&entries, view, ui)
                .cards
                .iter()
                .any(|(e, rect)| *e == b.selected
                    && rect.y >= shelf_top
                    && rect.y + rect.h <= shelf_bottom),
            "the selection is fully back on screen"
        );
        let out = b.handle(
            &entries,
            &[RawEvent::KeyDown { key: Key::Enter }],
            &mut mouse,
        );
        assert_eq!(out, Out::Pick(0), "the second Enter commits");
    }

    #[test]
    fn the_wheel_stops_at_the_last_full_screenful() {
        let entries = shelf();
        let mut b = Browser::new();
        let mut mouse = vec2(0.0, 0.0);
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let full = b.layout(&entries, view, ui).cards.len();
        assert!(full >= 4, "precondition: the window shows several cards");
        // Scroll far past the end: the wheel once clamped to the LAST
        // line, parking the tail row alone at the top of empty screen.
        for _ in 0..60 {
            b.handle(&entries, &[RawEvent::Wheel { delta: -1.0 }], &mut mouse);
        }
        let end = b.layout(&entries, view, ui);
        assert!(!end.more_below, "the shelf's tail is on screen");
        let (_, _, _, _, _, _, shelf_bottom) = metrics(view, ui);
        let tail_bottom = end
            .cards
            .iter()
            .map(|(_, rect)| rect.y + rect.h)
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (tail_bottom - shelf_bottom).abs() < 0.01,
            "the tail sits against the viewport instead of above empty space"
        );
        assert_eq!(
            end.scroll_offset, end.scroll_max,
            "the pixel clamp lands exactly at the shelf tail"
        );
        // Moving back by any visible amount puts the tail below the
        // viewport again.
        b.handle(&entries, &[RawEvent::Wheel { delta: 1.0 }], &mut mouse);
        assert!(
            b.layout(&entries, view, ui).more_below,
            "one line above the clamp, the tail is off screen"
        );
        // The keyboard still reaches the last row.
        b.handle(&entries, &[RawEvent::KeyDown { key: Key::End }], &mut mouse);
        assert_eq!(b.selected, entries.len() - 1);
        assert!(
            b.layout(&entries, view, ui)
                .cards
                .iter()
                .any(|(e, _)| *e == b.selected),
            "End shows the last card"
        );
    }

    #[test]
    fn trackpad_fractions_scroll_immediately_and_smoothly() {
        let entries = shelf();
        let mut b = Browser::new();
        let mut mouse = vec2(0.0, 0.0);
        let before = b.layout(
            &entries,
            crate::render::viewport(),
            crate::render::ui_scale(),
        );
        b.handle(&entries, &[RawEvent::Wheel { delta: -0.11 }], &mut mouse);
        let first = b.scroll_y;
        assert!(first > 0.0, "a fractional trackpad event moves immediately");
        assert!(
            first < before.viewport_height / 4.0,
            "a tiny gesture cannot jump a substantial part of the shelf"
        );
        b.handle(&entries, &[RawEvent::Wheel { delta: -0.11 }], &mut mouse);
        assert!(
            b.scroll_y > first && b.scroll_y < first * 2.1,
            "equal fractional events produce continuous pixel motion"
        );
    }

    #[test]
    fn a_resize_scrolls_the_window_back_to_the_selection() {
        let entries = shelf();
        let mut b = Browser::new();
        let mut mouse = vec2(0.0, 0.0);
        b.handle(&entries, &[RawEvent::KeyDown { key: Key::End }], &mut mouse);
        // The window shrinks out from under the tail selection; the
        // next frame must scroll back to it, or Enter fires a card
        // the player cannot see.
        crate::render::set_viewport(640.0, 400.0);
        b.handle(
            &entries,
            &[RawEvent::MouseMove { x: 0.0, y: 0.0 }],
            &mut mouse,
        );
        let ui = crate::render::ui_scale();
        let visible: Vec<usize> = b
            .layout(&entries, vec2(640.0, 400.0), ui)
            .cards
            .iter()
            .map(|(e, _)| *e)
            .collect();
        assert!(
            visible.contains(&b.selected),
            "the selection is back on screen (visible {visible:?}, selected {})",
            b.selected
        );
    }

    #[test]
    fn a_small_window_at_max_scale_still_shows_cards() {
        let entries = shelf();
        let b = Browser::new();
        // 640x400 at the 150% user scale used to reject every card
        // row: headings drew, cards vanished, Enter still fired.
        let layout = b.layout(&entries, vec2(640.0, 400.0), 1.5);
        assert!(
            !layout.cards.is_empty(),
            "at least one card row fits every supported window"
        );
        for (_, r) in &layout.cards {
            assert!(r.h > 20.0, "cards stay tall enough to read and click");
        }
    }

    #[test]
    fn enter_picks_and_escape_backs_out() {
        let entries = shelf();
        let mut b = Browser::new();
        assert_eq!(press(&mut b, &entries, Key::End), Out::Stay);
        assert_eq!(b.selected, 10);
        assert_eq!(press(&mut b, &entries, Key::Enter), Out::Pick(10));
        assert_eq!(press(&mut b, &entries, Key::Escape), Out::Back);
    }

    #[test]
    fn clicks_commit_on_release_inside_the_same_card() {
        let entries = shelf();
        let mut b = Browser::new();
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let layout = b.layout(&entries, view, ui);
        let (target, rect) = layout.cards[2];
        let (cx, cy) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
        let mut mouse = vec2(0.0, 0.0);
        let click = [
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x: cx,
                y: cy,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: cx,
                y: cy,
            },
        ];
        // The first click on a non-selected card SELECTS it — browsing
        // by pointer must not misfire a launch.
        let out = b.handle(&entries, &click, &mut mouse);
        assert_eq!(out, Out::Stay, "the first click only selects");
        assert_eq!(b.selected, target);
        // The second click on the now-selected card commits.
        let out = b.handle(&entries, &click, &mut mouse);
        assert_eq!(out, Out::Pick(target));
        // Dragging away cancels.
        let out = b.handle(
            &entries,
            &[
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: cx,
                    y: cy,
                },
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: cx + rect.w * 2.0,
                    y: cy,
                },
            ],
            &mut mouse,
        );
        assert_eq!(out, Out::Stay);
    }

    #[test]
    fn touch_drag_scrolls_while_touch_taps_select_and_commit() {
        let entries = shelf();
        let mut b = Browser::new();
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let (_, rect) = b.layout(&entries, view, ui).cards[2];
        let (x, y) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
        let mut mouse = vec2(0.0, 0.0);

        b.handle(&entries, &[RawEvent::TouchDown { id: 1, x, y }], &mut mouse);
        let out = b.handle(
            &entries,
            &[RawEvent::TouchMove {
                id: 1,
                x,
                y: y - 30.0,
            }],
            &mut mouse,
        );
        assert_eq!(out, Out::Stay);
        assert!(b.scroll_y > 0.0, "an upward drag moves down the shelf");
        let out = b.handle(
            &entries,
            &[RawEvent::TouchUp {
                id: 1,
                x,
                y: y - 30.0,
            }],
            &mut mouse,
        );
        assert_eq!(out, Out::Stay, "a drag cannot activate a card");
        assert_eq!(b.selected, 0, "a drag cannot retarget keyboard focus");

        b.scroll_y = 0.0;
        let tap = [
            RawEvent::TouchDown { id: 2, x, y },
            RawEvent::TouchUp { id: 2, x, y },
        ];
        assert_eq!(b.handle(&entries, &tap, &mut mouse), Out::Stay);
        assert_eq!(b.selected, 2, "the first tap selects");
        assert_eq!(
            b.handle(&entries, &tap, &mut mouse),
            Out::Pick(2),
            "a second tap commits"
        );
    }

    #[test]
    fn the_remembered_pick_is_found_by_path() {
        let entries = shelf();
        let mut b = Browser::new();
        b.select_path(&entries, &entries[7].path.clone());
        assert_eq!(b.selected, 7);
        b.select_path(&entries, &Some(std::path::PathBuf::from("gone.json")));
        assert_eq!(b.selected, 7, "a vanished file keeps the old ground");
    }
}
