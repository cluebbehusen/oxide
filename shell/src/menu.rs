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

    fn item_rect(&self, index: usize) -> Rect {
        let top = screen_height() * 0.42;
        Rect::new(
            (screen_width() - ITEM_WIDTH) * 0.5,
            top + index as f32 * ITEM_HEIGHT,
            ITEM_WIDTH,
            ITEM_HEIGHT - 8.0,
        )
    }

    /// Feeds a frame of events through the menu; returns the activated row,
    /// if any. Mouse position updates come along in the same events.
    pub fn handle(&mut self, events: &[RawEvent], mouse: &mut Vec2) -> Option<usize> {
        for event in events {
            match *event {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(x, y);
                    for index in 0..self.items.len() {
                        if self.item_rect(index).contains(*mouse) {
                            self.selected = index;
                        }
                    }
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    let click = vec2(x, y);
                    for index in 0..self.items.len() {
                        if self.item_rect(index).contains(click) {
                            return Some(index);
                        }
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
        let title_size = 96.0;
        let dims = measure_text(&self.title, None, title_size as u16, 1.0);
        draw_text(
            &self.title,
            (screen_width() - dims.width) * 0.5,
            screen_height() * 0.28,
            title_size,
            TITLE_COLOR,
        );
        let sub_dims = measure_text(subtitle, None, 20, 1.0);
        draw_text(
            subtitle,
            (screen_width() - sub_dims.width) * 0.5,
            screen_height() * 0.28 + 34.0,
            20.0,
            DIM,
        );

        for (index, label) in self.items.iter().enumerate() {
            let rect = self.item_rect(index);
            let selected = index == self.selected;
            if selected {
                draw_rectangle(rect.x, rect.y, rect.w, rect.h, PANEL);
                draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 2.0, TITLE_COLOR);
            }
            let color = if selected { SELECTED_COLOR } else { ITEM_COLOR };
            draw_text(label, rect.x + 18.0, rect.y + rect.h * 0.68, 26.0, color);
        }

        // ASCII on purpose: the default font has no glyphs for arrows.
        let hint = "Up/Down select - Enter confirm - or click";
        let hint_dims = measure_text(hint, None, 18, 1.0);
        draw_text(
            hint,
            (screen_width() - hint_dims.width) * 0.5,
            screen_height() - 24.0,
            18.0,
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
