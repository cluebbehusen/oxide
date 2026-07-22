//! Menus: the main screen and the pause screen.
//!
//! One deliberately plain widget — a titled list navigated by arrow keys,
//! Enter, or the mouse. Menu input arrives through the same [`RawEvent`]
//! funnel as gameplay, so injected events drive menus exactly like hardware
//! (which is also how the menus get tested).

use macroquad::prelude::*;
use oxide_protocol::{Key, MouseButton, RawEvent};
use oxide_sim::Scenario;
use std::path::PathBuf;

const TITLE_COLOR: Color = color_u8!(196, 87, 59, 255);
const ITEM_COLOR: Color = color_u8!(232, 228, 216, 200);
const SELECTED_COLOR: Color = color_u8!(232, 228, 216, 255);
const DIM: Color = color_u8!(232, 228, 216, 110);
const PANEL: Color = color_u8!(20, 20, 24, 230);

const ITEM_HEIGHT: f32 = 44.0;
const ITEM_WIDTH: f32 = 420.0;

fn ui() -> f32 {
    crate::render::ui_scale()
}

/// A titled, selectable list.
///
/// Three independent pieces of state, deliberately: `selected` is the
/// keyboard cursor and activation target, `scroll` is which window of
/// rows is shown, and `hover` is only a highlight. The 0.8 widget fused
/// them — hover moved selection, the window derived from selection —
/// and a stationary pointer could walk the whole list by itself.
pub struct Menu {
    /// Heading above the list.
    pub title: String,
    /// One label per row.
    pub items: Vec<String>,
    /// Keyboard cursor; what Enter activates.
    pub selected: usize,
    /// First visible row — moved by the wheel, paging keys, and
    /// `ensure_visible`; never by the pointer.
    scroll: usize,
    /// Row under the pointer, highlight only.
    hover: Option<usize>,
    /// Row armed by a press; activation happens on release inside the
    /// same row, so dragging away cancels.
    pressed: Option<usize>,
}

impl Menu {
    /// Builds a menu with the first row highlighted.
    pub fn new(title: impl Into<String>, items: Vec<String>) -> Self {
        Self {
            title: title.into(),
            items,
            selected: 0,
            scroll: 0,
            hover: None,
            pressed: None,
        }
    }

    /// Moves the keyboard cursor and scrolls just enough to show it —
    /// the only coupling between selection and the scroll window.
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.items.len().saturating_sub(1));
        self.ensure_visible();
    }

    fn ensure_visible(&mut self) {
        let (_, _, _, visible) = self.layout();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
    }

    fn scroll_by(&mut self, delta: i64) {
        let (_, _, _, visible) = self.layout();
        let max = self.items.len().saturating_sub(visible);
        self.scroll = (self.scroll as i64 + delta).clamp(0, max as i64) as usize;
    }

    fn row_at(&self, point: Vec2) -> Option<usize> {
        (0..self.items.len()).find(|i| self.item_rect(*i).is_some_and(|r| r.contains(point)))
    }

    /// Where the list lives this frame: top edge, row height, and the
    /// window of visible rows. The list fits itself between the title
    /// block and the hint line — rows shrink when the window is short,
    /// and past the readable minimum the list scrolls around the
    /// selection instead of running off the screen.
    fn layout(&self) -> (f32, f32, usize, usize) {
        let s = ui();
        let top_bound = (screen_height() * 0.36).max(screen_height() * 0.28 + 64.0 * s);
        let bottom_bound = screen_height() - 64.0 * s;
        let avail = (bottom_bound - top_bound).max(ITEM_HEIGHT * s);
        let n = self.items.len().max(1);
        let row = (avail / n as f32).clamp(30.0 * s, ITEM_HEIGHT * s);
        let visible = ((avail / row).floor() as usize).clamp(1, n);
        // The window is scroll state, clamped — never a function of the
        // selection, or hovering near an edge walks the list.
        let first = self.scroll.min(n.saturating_sub(visible));
        let top = top_bound + (avail - visible as f32 * row) * 0.5;
        (top, row, first, visible)
    }

    fn item_rect(&self, index: usize) -> Option<Rect> {
        let s = ui();
        let (top, row, first, visible) = self.layout();
        if index < first || index >= first + visible {
            return None;
        }
        Some(Rect::new(
            (screen_width() - ITEM_WIDTH * s) * 0.5,
            top + (index - first) as f32 * row,
            ITEM_WIDTH * s,
            row - 6.0 * s,
        ))
    }

    /// Row under the pointer, if any.
    pub fn hover(&self) -> Option<usize> {
        self.hover
    }

    /// Half-open range of rows currently drawn by the scroll window.
    pub fn visible_range(&self) -> [usize; 2] {
        let (_, _, first, visible) = self.layout();
        [first, first + visible]
    }

    /// Feeds a frame of events through the menu; returns the activated row,
    /// if any. Mouse position updates come along in the same events.
    pub fn handle(&mut self, events: &[RawEvent], mouse: &mut Vec2) -> Option<usize> {
        for event in events {
            match *event {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(x, y);
                    self.hover = self.row_at(*mouse);
                }
                RawEvent::Wheel { delta } => {
                    // Wheel up shows earlier rows; the pointer stays put
                    // and the hover follows whatever slid beneath it.
                    self.scroll_by(if delta > 0.0 { -1 } else { 1 });
                    self.hover = self.row_at(*mouse);
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    self.pressed = self.row_at(vec2(x, y));
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    let released_on = self.row_at(vec2(x, y));
                    let armed = self.pressed.take();
                    if let (Some(a), Some(r)) = (armed, released_on)
                        && a == r
                    {
                        // A press commits only when it releases inside
                        // the same row.
                        self.selected = a;
                        return Some(a);
                    }
                }
                RawEvent::KeyDown { key: Key::Up } => {
                    self.selected = self.selected.checked_sub(1).unwrap_or(self.items.len() - 1);
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::Down } => {
                    self.selected = (self.selected + 1) % self.items.len();
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::PageUp } => {
                    let (_, _, _, visible) = self.layout();
                    self.selected = self.selected.saturating_sub(visible);
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::PageDown } => {
                    let (_, _, _, visible) = self.layout();
                    self.selected =
                        (self.selected + visible).min(self.items.len().saturating_sub(1));
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::Home } => {
                    self.selected = 0;
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::End } => {
                    self.selected = self.items.len().saturating_sub(1);
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::Enter } => return Some(self.selected),
                _ => {}
            }
        }
        None
    }

    /// Draws the menu (over whatever the caller already drew).
    pub fn draw(&self, subtitle: &str) {
        let s = ui();
        let title_size = 96.0 * s;
        let dims = measure_text(&self.title, None, title_size as u16, 1.0);
        draw_text(
            &self.title,
            (screen_width() - dims.width) * 0.5,
            screen_height() * 0.28,
            title_size,
            TITLE_COLOR,
        );
        let sub_dims = measure_text(subtitle, None, (20.0 * s) as u16, 1.0);
        draw_text(
            subtitle,
            (screen_width() - sub_dims.width) * 0.5,
            screen_height() * 0.28 + 34.0 * s,
            20.0 * s,
            DIM,
        );

        let (_, row, first, visible) = self.layout();
        let text_size = (26.0 * s * (row / (ITEM_HEIGHT * s))).clamp(18.0 * s, 26.0 * s);
        for (index, label) in self.items.iter().enumerate() {
            let Some(rect) = self.item_rect(index) else {
                continue;
            };
            let selected = index == self.selected;
            let hovered = self.hover == Some(index);
            if selected {
                draw_rectangle(rect.x, rect.y, rect.w, rect.h, PANEL);
                draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 2.0, TITLE_COLOR);
            } else if hovered {
                draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.0, DIM);
            }
            let color = if selected { SELECTED_COLOR } else { ITEM_COLOR };
            draw_text(
                label,
                rect.x + 18.0 * s,
                rect.y + rect.h * 0.68,
                text_size,
                color,
            );
        }
        // Scroll cues when the list is windowed.
        if first > 0 {
            let r = self.item_rect(first).unwrap();
            draw_text("^", r.x + r.w * 0.5, r.y - 6.0 * s, 22.0 * s, DIM);
        }
        if first + visible < self.items.len() {
            let r = self.item_rect(first + visible - 1).unwrap();
            draw_text("v", r.x + r.w * 0.5, r.y + row + 14.0 * s, 22.0 * s, DIM);
        }

        // ASCII on purpose: the default font has no glyphs for arrows.
        let hint = "Up/Down select - Enter confirm - or click";
        let hint_dims = measure_text(hint, None, (18.0 * s) as u16, 1.0);
        draw_text(
            hint,
            (screen_width() - hint_dims.width) * 0.5,
            screen_height() - 24.0 * s,
            18.0 * s,
            DIM,
        );
    }
}

/// A startable entry on the main menu.
pub struct ScenarioEntry {
    /// Display name (from the file's own `name` field).
    pub label: String,
    /// File path; `None` means the embedded skirmish.
    pub path: Option<PathBuf>,
}

/// Lists playable scenarios: everything parseable under `scenarios/`, or
/// the embedded skirmish if the directory is missing (e.g. running the
/// binary outside the repo).
pub fn discover_scenarios() -> Vec<ScenarioEntry> {
    let mut entries: Vec<ScenarioEntry> = Vec::new();
    if let Ok(dir) = std::fs::read_dir("scenarios") {
        let mut paths: Vec<PathBuf> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(scenario) = Scenario::load(&path) {
                entries.push(ScenarioEntry {
                    label: scenario.name,
                    path: Some(path),
                });
            }
        }
    }
    if entries.is_empty() {
        entries.push(ScenarioEntry {
            label: Scenario::skirmish().name,
            path: None,
        });
    }
    entries
}
