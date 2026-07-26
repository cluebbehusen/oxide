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
    Color, DrawTextureParams, Rect, Vec2, color_u8, draw_rectangle, draw_rectangle_lines,
    draw_text, draw_texture_ex, measure_text, vec2,
};
use oxide_protocol::{Key, MouseButton, RawEvent};

const TITLE_COLOR: Color = color_u8!(196, 87, 59, 255);
const ITEM_COLOR: Color = color_u8!(214, 210, 196, 255);
const DIM: Color = color_u8!(214, 210, 196, 120);
const PANEL: Color = color_u8!(20, 20, 24, 230);

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
}

/// Grid state: the selected entry, scroll, and pointer bookkeeping.
pub struct Browser {
    /// Selected entry index (into the discovery list).
    pub selected: usize,
    /// First visible line of the grid.
    scroll_line: usize,
    hover: Option<usize>,
    pressed: Option<usize>,
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
    let card_h = (card_w * 0.5 + 26.0 * ui).min(bottom - top - heading_h - 16.0 * ui);
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
            scroll_line: 0,
            hover: None,
            pressed: None,
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
        let mut y = top;
        let mut headings = Vec::new();
        let mut cards = Vec::new();
        let mut more_below = false;
        for (li, line) in all.iter().enumerate() {
            if li < self.scroll_line {
                continue;
            }
            let h = match line {
                Line::Heading(_) => heading_h,
                Line::Cards(_) => card_h + gap,
            };
            if y + h > bottom {
                more_below = true;
                break;
            }
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
            y += h;
        }
        Layout {
            headings,
            cards,
            more_above: self.scroll_line > 0,
            more_below,
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
        // Approximate the window in lines from the default viewport;
        // exactness comes from layout() at draw, and the clamp below
        // keeps the selection inside whatever is really shown.
        let view = crate::render::viewport();
        let ui = crate::render::ui_scale();
        let cols = columns(view.x, ui);
        let (li, _) = Self::locate(entries, cols, self.selected);
        if li < self.scroll_line {
            // Show the section heading above a first-in-section card.
            self.scroll_line = li.saturating_sub(1);
        }
        // Walk the window forward until the selected line fits.
        for _ in 0..64 {
            let visible: Vec<usize> = {
                let l = self.layout(entries, view, ui);
                l.cards.iter().map(|(e, _)| *e).collect()
            };
            if visible.contains(&self.selected) {
                break;
            }
            self.scroll_line += 1;
        }
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
        let last = entries.len() - 1;
        let card_at = |browser: &Self, p: Vec2| {
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
                RawEvent::KeyDown { key: Key::Enter } => return Out::Pick(self.selected),
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
                    self.scroll_line = 0;
                }
                RawEvent::KeyDown { key: Key::End } => {
                    self.selected = last;
                    self.ensure_visible(entries);
                }
                RawEvent::Wheel { delta } => {
                    let max = lines(entries, cols).len().saturating_sub(1);
                    if delta > 0.0 {
                        self.scroll_line = self.scroll_line.saturating_sub(1);
                    } else if delta < 0.0 {
                        self.scroll_line = (self.scroll_line + 1).min(max);
                    }
                    self.hover = card_at(self, *mouse);
                    // The row-menu rule: scrolling drags the selection
                    // along, so Enter always fires a card the player
                    // can see — never one hidden past the window edge.
                    let visible: Vec<usize> = self
                        .layout(entries, view, ui)
                        .cards
                        .iter()
                        .map(|(e, _)| *e)
                        .collect();
                    if let (Some(&first), Some(&last_vis)) = (visible.first(), visible.last())
                        && !visible.contains(&self.selected)
                    {
                        let (li, _) = Self::locate(entries, cols, self.selected);
                        self.selected = if li < self.scroll_line {
                            first
                        } else {
                            last_vis
                        };
                    }
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
                        // Release inside the pressed card commits, like
                        // every menu row.
                        self.selected = a;
                        return Out::Pick(a);
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
        let title_size = 64.0 * ui;
        let dims = measure_text("OXIDE", None, title_size as u16, 1.0);
        draw_text(
            "OXIDE",
            (view.x - dims.width) * 0.5,
            72.0 * ui,
            title_size,
            TITLE_COLOR,
        );
        let layout = self.layout(entries, view, ui);
        for (label, rect) in &layout.headings {
            draw_text(
                label,
                rect.x,
                rect.y + rect.h * 0.62,
                22.0 * ui,
                TITLE_COLOR,
            );
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
            draw_rectangle(rect.x, rect.y, rect.w, rect.h, PANEL);
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
                TITLE_COLOR
            } else if hovered {
                DIM
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
                if selected { ITEM_COLOR } else { DIM },
            );
        }
        // Scroll cues.
        let (band_x, band_w, ..) = metrics(view, ui);
        if layout.more_above {
            draw_text("^", band_x + band_w * 0.5, 110.0 * ui, 22.0 * ui, DIM);
        }
        if layout.more_below {
            draw_text(
                "v",
                band_x + band_w * 0.5,
                view.y - 66.0 * ui,
                22.0 * ui,
                DIM,
            );
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
                ITEM_COLOR,
            );
        }
        let hint = "Arrows select - Enter play - Esc back - or click";
        let dims = measure_text(hint, None, (16.0 * ui) as u16, 1.0);
        draw_text(
            hint,
            (view.x - dims.width) * 0.5,
            view.y - 20.0 * ui,
            16.0 * ui,
            DIM,
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
    fn wheel_scroll_drags_the_selection_into_view() {
        let entries = shelf();
        let mut b = Browser::new();
        let mut mouse = vec2(0.0, 0.0);
        // Scroll far past the first section: the selection must ride
        // along, or Enter fires a card the player cannot see.
        for _ in 0..8 {
            b.handle(&entries, &[RawEvent::Wheel { delta: -1.0 }], &mut mouse);
            let view = crate::render::viewport();
            let ui = crate::render::ui_scale();
            let visible: Vec<usize> = b
                .layout(&entries, view, ui)
                .cards
                .iter()
                .map(|(e, _)| *e)
                .collect();
            if !visible.is_empty() {
                assert!(
                    visible.contains(&b.selected),
                    "selection {} hidden by scroll (visible {:?})",
                    b.selected,
                    visible
                );
            }
        }
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
                    x: cx,
                    y: cy,
                },
            ],
            &mut mouse,
        );
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
    fn the_remembered_pick_is_found_by_path() {
        let entries = shelf();
        let mut b = Browser::new();
        b.select_path(&entries, &entries[7].path.clone());
        assert_eq!(b.selected, 7);
        b.select_path(&entries, &Some(std::path::PathBuf::from("gone.json")));
        assert_eq!(b.selected, 7, "a vanished file keeps the old ground");
    }
}
