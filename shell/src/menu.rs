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
    /// Fractional wheel accumulation: trackpads deliver hundredths per
    /// frame, and treating each as a full row made scrolling frantic.
    wheel_accum: f32,
    /// Row armed by a press; activation happens on release inside the
    /// same row, so dragging away cancels.
    pressed: Option<usize>,
    /// Section-label rows: drawn dimmer, skipped by the cursor, never
    /// activated — the map browser's format headings.
    headers: Vec<usize>,
}

fn view_w() -> f32 {
    crate::render::viewport().x
}

fn view_h() -> f32 {
    crate::render::viewport().y
}

impl Menu {
    /// Builds a menu with the first row highlighted.
    pub fn new(title: impl Into<String>, items: Vec<String>) -> Self {
        Self::with_headers(title, items, Vec::new())
    }

    /// A menu whose `headers` rows are section labels: skipped by the
    /// cursor, inert to clicks, drawn as headings.
    pub fn with_headers(title: impl Into<String>, items: Vec<String>, headers: Vec<usize>) -> Self {
        let mut menu = Self {
            title: title.into(),
            items,
            selected: 0,
            scroll: 0,
            hover: None,
            wheel_accum: 0.0,
            pressed: None,
            headers,
        };
        menu.selected = menu.snap_clamped(0, 1);
        menu
    }

    /// Whether a row is a section label.
    pub fn is_header(&self, index: usize) -> bool {
        self.headers.contains(&index)
    }

    /// The nearest non-header row from `index`, walking in `dir` and
    /// WRAPPING — the arrow keys' semantics (Up from the first real row
    /// lands on the last). Falls back to `index` on an all-header list.
    fn snap(&self, index: usize, dir: i64) -> usize {
        let n = self.items.len();
        if n == 0 {
            return 0;
        }
        let mut i = index.min(n - 1);
        for _ in 0..n {
            if !self.is_header(i) {
                return i;
            }
            i = (i as i64 + dir).rem_euclid(n as i64) as usize;
        }
        index.min(n - 1)
    }

    /// The nearest non-header row from `index` WITHOUT wrapping: walk
    /// `dir` to the list's edge, then fall back the other way — jump
    /// keys (Home, End, paging) and programmatic selects must land
    /// NEAR their target, never teleport across the list because a
    /// section label sat in the way (PageUp once snapped to the bottom
    /// Back row through the browser's leading header).
    fn snap_clamped(&self, index: usize, dir: i64) -> usize {
        let n = self.items.len() as i64;
        if n == 0 {
            return 0;
        }
        let start = (index as i64).min(n - 1);
        let mut i = start;
        while (0..n).contains(&i) {
            if !self.is_header(i as usize) {
                return i as usize;
            }
            i += dir;
        }
        let mut i = start - dir;
        while (0..n).contains(&i) {
            if !self.is_header(i as usize) {
                return i as usize;
            }
            i -= dir;
        }
        start as usize
    }

    /// Moves the keyboard cursor and scrolls just enough to show it —
    /// the only coupling between selection and the scroll window.
    pub fn select(&mut self, index: usize) {
        self.selected = self.snap_clamped(index.min(self.items.len().saturating_sub(1)), 1);
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
        // The selection rides inside the window: Enter must never
        // activate a row the wheel has scrolled out of sight (a hidden
        // Quit would be a nasty surprise) — and never lands on a
        // header while riding (Enter on a "section label" activated
        // whatever the caller mapped to nothing).
        let clamped = self
            .selected
            .clamp(self.scroll, self.scroll + visible.saturating_sub(1));
        self.selected = self.snap_clamped(clamped, if clamped < self.selected { -1 } else { 1 });
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
        let top_bound = (view_h() * 0.36).max(view_h() * 0.28 + 64.0 * s);
        let bottom_bound = view_h() - 64.0 * s;
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
            (view_w() - ITEM_WIDTH * s) * 0.5,
            top + (index - first) as f32 * row,
            ITEM_WIDTH * s,
            row - 6.0 * s,
        ))
    }

    /// Row under the pointer, if any.
    pub fn hover(&self) -> Option<usize> {
        self.hover
    }

    /// Right edge of the row boxes — side panels place themselves
    /// strictly beyond it, using the same rect the rows draw with, so
    /// overlap is impossible by construction.
    pub fn rows_right_edge(&self) -> f32 {
        let (_, _, first, _) = self.layout();
        self.item_rect(first).map_or(0.0, |r| r.x + r.w)
    }

    /// Half-open range of rows currently drawn by the scroll window.
    pub fn visible_range(&self) -> [usize; 2] {
        let (_, _, first, visible) = self.layout();
        [first, first + visible]
    }

    /// Feeds a frame of events through the menu; returns the activated row,
    /// if any. Mouse position updates come along in the same events.
    pub fn handle(&mut self, events: &[RawEvent], mouse: &mut Vec2) -> Option<usize> {
        // An empty list has nothing to select, scroll, or activate —
        // and its wrap-around arithmetic divides by zero. The shelf can
        // legitimately be empty on a fresh profile.
        if self.items.is_empty() {
            return None;
        }
        for event in events {
            match *event {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(x, y);
                    self.hover = self.row_at(*mouse).filter(|r| !self.is_header(*r));
                }
                RawEvent::Wheel { delta } => {
                    // Wheel up shows earlier rows; the pointer stays put
                    // and the hover follows whatever slid beneath it.
                    // Whole notches only — fractions accumulate.
                    self.wheel_accum += delta;
                    let steps = self.wheel_accum.trunc();
                    if steps == 0.0 {
                        continue;
                    }
                    self.wheel_accum -= steps;
                    self.scroll_by(if steps > 0.0 { -1 } else { 1 });
                    self.hover = self.row_at(*mouse).filter(|r| !self.is_header(*r));
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    self.pressed = self.row_at(vec2(x, y)).filter(|r| !self.is_header(*r));
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
                    self.hover = None;
                    let up = self.selected.checked_sub(1).unwrap_or(self.items.len() - 1);
                    self.selected = self.snap(up, -1);
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::Down } => {
                    self.hover = None;
                    self.selected = self.snap((self.selected + 1) % self.items.len(), 1);
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::PageUp } => {
                    self.hover = None;
                    let (_, _, _, visible) = self.layout();
                    self.selected = self.snap_clamped(self.selected.saturating_sub(visible), -1);
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::PageDown } => {
                    self.hover = None;
                    let (_, _, _, visible) = self.layout();
                    self.selected = self.snap_clamped(
                        (self.selected + visible).min(self.items.len().saturating_sub(1)),
                        1,
                    );
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::Home } => {
                    self.hover = None;
                    self.selected = self.snap_clamped(0, 1);
                    self.ensure_visible();
                }
                RawEvent::KeyDown { key: Key::End } => {
                    self.hover = None;
                    self.selected = self.snap_clamped(self.items.len().saturating_sub(1), -1);
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
            (view_w() - dims.width) * 0.5,
            view_h() * 0.28,
            title_size,
            TITLE_COLOR,
        );
        // The subtitle shrinks to fit — map blurbs run long, and text
        // spilling off both window edges reads as a defect, not a hook.
        let mut sub_size = 20.0 * s;
        let mut sub_dims = measure_text(subtitle, None, sub_size as u16, 1.0);
        let max_width = view_w() * 0.55;
        if sub_dims.width > max_width {
            sub_size = (sub_size * max_width / sub_dims.width).max(12.0 * s);
            sub_dims = measure_text(subtitle, None, sub_size as u16, 1.0);
        }
        draw_text(
            subtitle,
            (view_w() - sub_dims.width) * 0.5,
            view_h() * 0.28 + 34.0 * s,
            sub_size,
            DIM,
        );

        let (_, row, first, visible) = self.layout();
        let text_size = (26.0 * s * (row / (ITEM_HEIGHT * s))).clamp(18.0 * s, 26.0 * s);
        for (index, label) in self.items.iter().enumerate() {
            let Some(rect) = self.item_rect(index) else {
                continue;
            };
            if self.is_header(index) {
                let size = (20.0 * s).min(text_size);
                let dims = measure_text(label, None, size as u16, 1.0);
                draw_text(
                    label,
                    rect.x + (rect.w - dims.width) * 0.5,
                    rect.y + rect.h * 0.68,
                    size,
                    DIM,
                );
                continue;
            }
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
            (view_w() - hint_dims.width) * 0.5,
            view_h() - 24.0 * s,
            18.0 * s,
            DIM,
        );
    }
}

/// Lazily rendered fog-free map previews, one per scenario row. The
/// driver's software rasterizer draws the built state (the same pixels
/// the golden tests pin), uploaded once as a texture and kept for the
/// session.
#[derive(Default)]
pub struct PreviewCache {
    slots: std::collections::HashMap<usize, Option<Texture2D>>,
}

impl PreviewCache {
    /// The preview for a row, rendering on first request. `None` when
    /// the scenario fails to load or build — the browser just shows no
    /// panel for it.
    pub fn get(&mut self, index: usize, entry: &ScenarioEntry) -> Option<&Texture2D> {
        self.slots
            .entry(index)
            .or_insert_with(|| {
                let scenario = match &entry.path {
                    Some(path) => Scenario::load(path).ok()?,
                    None => Scenario::skirmish(),
                };
                let state = scenario.build().ok()?;
                let pixmap = oxide_kit::render::render_state(&state);
                let texture = Texture2D::from_rgba8(
                    pixmap.width() as u16,
                    pixmap.height() as u16,
                    pixmap.data(),
                );
                texture.set_filter(FilterMode::Nearest);
                Some(texture)
            })
            .as_ref()
    }
}

/// A startable entry on the main menu.
pub struct ScenarioEntry {
    /// Display name (from the file's own `name` field).
    pub label: String,
    /// Seats on the map — the browser's section key (1v1 first).
    pub seats: usize,
    /// One-line browser blurb from the authored metadata, when present:
    /// hook plus pace/mode/richness badges.
    pub blurb: Option<String>,
    /// File path; `None` means the embedded skirmish.
    pub path: Option<PathBuf>,
    /// Theme key from the authored metadata — the preview panel grades
    /// its thumbnail with the same tint the match will wear.
    pub theme: String,
}

/// Lists playable scenarios: everything parseable under `scenarios/`, or
/// the embedded skirmish if the directory is missing (e.g. running the
/// binary outside the repo).
pub fn discover_scenarios() -> Vec<ScenarioEntry> {
    let mut entries: Vec<ScenarioEntry> = Vec::new();
    if let Ok(dir) = std::fs::read_dir(crate::assets::resource_root().join("scenarios")) {
        let mut paths: Vec<PathBuf> = dir
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(scenario) = Scenario::load(&path) {
                let blurb = scenario.meta.as_ref().map(|m| {
                    // Only badges that exist: a missing field must
                    // not render as a dangling separator.
                    let badges: Vec<&str> = [&m.pace, &m.mode, &m.richness]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .map(String::as_str)
                        .collect();
                    format!("{}  [{}]", m.hook, badges.join(" - "))
                });
                let theme = scenario
                    .meta
                    .as_ref()
                    .map(|m| m.theme.clone())
                    .unwrap_or_default();
                entries.push(ScenarioEntry {
                    seats: scenario.players.len(),
                    label: scenario.name,
                    blurb,
                    path: Some(path),
                    theme,
                });
            }
        }
    }
    if entries.is_empty() {
        entries.push(ScenarioEntry {
            seats: 2,
            label: Scenario::skirmish().name,
            blurb: None,
            path: None,
            theme: String::new(),
        });
    }
    // Sections: 1v1 first (a first Play+Enter must never launch a team
    // match), bigger formats after, alphabetical within each. Callers
    // key the remembered pick by PATH, so re-sorting can't move it.
    entries.sort_by_key(|e| (e.seats, e.label.to_lowercase()));
    entries
}

#[cfg(test)]
mod empty_tests {
    use super::*;
    use macroquad::prelude::vec2;
    use oxide_protocol::{Key, RawEvent};

    #[test]
    fn an_empty_menu_survives_every_key() {
        // A fresh profile's replay shelf has zero rows; wrap-around
        // arithmetic on an empty list once divided by zero.
        let mut menu = Menu::new("EMPTY", Vec::new());
        let mut mouse = vec2(0.0, 0.0);
        for key in [Key::Up, Key::Down, Key::Enter, Key::PageDown, Key::End] {
            let events = [RawEvent::KeyDown { key }];
            assert_eq!(menu.handle(&events, &mut mouse), None);
        }
    }
}

#[cfg(test)]
mod header_tests {
    use super::*;
    use macroquad::prelude::vec2;
    use oxide_protocol::{Key, RawEvent};

    fn sectioned() -> Menu {
        // rows: [H] a b [H] c d
        Menu::with_headers(
            "T",
            ["- one -", "a", "b", "- two -", "c", "d"]
                .into_iter()
                .map(String::from)
                .collect(),
            vec![0, 3],
        )
    }

    fn press(menu: &mut Menu, key: Key) -> Option<usize> {
        let mut mouse = vec2(0.0, 0.0);
        menu.handle(&[RawEvent::KeyDown { key }], &mut mouse)
    }

    #[test]
    fn the_cursor_never_rests_on_a_header() {
        let mut menu = sectioned();
        assert_eq!(menu.selected, 1, "construction snaps off the header");
        press(&mut menu, Key::Down);
        assert_eq!(menu.selected, 2);
        press(&mut menu, Key::Down);
        assert_eq!(menu.selected, 4, "down hops the section label");
        press(&mut menu, Key::Up);
        assert_eq!(menu.selected, 2, "up hops it too");
        press(&mut menu, Key::Home);
        assert_eq!(menu.selected, 1, "Home lands on the first real row");
        press(&mut menu, Key::End);
        assert_eq!(menu.selected, 5, "End on the last real row");
    }

    #[test]
    fn a_header_never_activates() {
        let mut menu = sectioned();
        menu.select(0);
        assert_eq!(menu.selected, 1, "select snaps forward off the header");
        // Enter activates the snapped row, never the header.
        assert_eq!(press(&mut menu, Key::Enter), Some(1));
    }

    #[test]
    fn wheel_scroll_cannot_pin_the_cursor_onto_a_header() {
        // A short window forces the riding clamp; the ride must snap
        // off headers or Enter activates a section label (the wizard
        // maps unmapped rows to Back — a scroll would quit the list).
        let mut menu = sectioned();
        crate::render::set_viewport(1280.0, 400.0);
        menu.select(5);
        let mut mouse = vec2(0.0, 0.0);
        for _ in 0..6 {
            menu.handle(&[RawEvent::Wheel { delta: 1.0 }], &mut mouse);
            assert!(
                !menu.is_header(menu.selected),
                "the riding cursor rests on header row {}",
                menu.selected
            );
        }
        crate::render::set_viewport(1280.0, 800.0);
    }
}
