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
pub struct Menu {
    /// Heading above the list.
    pub title: String,
    /// One label per row.
    pub items: Vec<String>,
    /// Highlighted row.
    pub selected: usize,
}

impl Menu {
    /// Builds a menu with the first row highlighted.
    pub fn new(title: impl Into<String>, items: Vec<String>) -> Self {
        Self {
            title: title.into(),
            items,
            selected: 0,
        }
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
        let first = if n <= visible {
            0
        } else {
            // Keep the selection comfortably inside the window.
            self.selected.saturating_sub(visible / 2).min(n - visible)
        };
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

    /// Half-open range of rows currently drawn by the scroll window.
    pub fn visible_range(&self) -> [usize; 2] {
        let (_, _, first, visible) = self.layout();
        [first, first + visible]
    }

    /// Feeds a frame of events through the menu; returns the activated row,
    /// if any. Mouse position updates come along in the same events.
    pub fn handle(&mut self, events: &[RawEvent], mouse: &mut Vec2) -> Option<usize> {
        // Hit-testing uses one frozen snapshot of the row layout: when
        // the list scrolls, the layout depends on the selection, so
        // mutating the selection mid-scan would shift rows under the
        // pointer and let one coordinate match several of them.
        let rects: Vec<Option<Rect>> = (0..self.items.len()).map(|i| self.item_rect(i)).collect();
        for event in events {
            match *event {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(x, y);
                    if let Some(index) = (0..self.items.len())
                        .find(|i| rects[*i].is_some_and(|r| r.contains(*mouse)))
                    {
                        self.selected = index;
                    }
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    let click = vec2(x, y);
                    if let Some(index) =
                        (0..self.items.len()).find(|i| rects[*i].is_some_and(|r| r.contains(click)))
                    {
                        return Some(index);
                    }
                }
                RawEvent::KeyDown { key: Key::Up } => {
                    self.selected = self.selected.checked_sub(1).unwrap_or(self.items.len() - 1);
                }
                RawEvent::KeyDown { key: Key::Down } => {
                    self.selected = (self.selected + 1) % self.items.len();
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
            if selected {
                draw_rectangle(rect.x, rect.y, rect.w, rect.h, PANEL);
                draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 2.0, TITLE_COLOR);
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
