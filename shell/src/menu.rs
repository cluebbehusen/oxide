//! The list menu the row-based screens share, plus scenario discovery and
//! the map preview cache.
//!
//! A menu is a titled list navigated by arrow keys, Enter, the mouse, or a
//! finger. Menu input arrives through the same [`RawEvent`] funnel as
//! gameplay, so injected events drive menus exactly like hardware.

use crate::numeric;
use crate::numeric::Fit;
use macroquad::prelude::*;
use oxide_protocol::{MouseButton, RawEvent};
use oxide_sim::Scenario;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::press::Press;
use crate::theme::{
    SURFACE_MENU, TEXT_BODY, TEXT_DISABLED, TEXT_PRIMARY, TEXT_SECONDARY, TEXT_TITLE,
};

const ITEM_HEIGHT: f32 = 44.0;
const ITEM_WIDTH: f32 = 420.0;
/// The space between rows, left out of each row's target.
const ROW_GAP: f32 = 6.0;
/// The tightest a desktop list packs its rows before it scrolls.
const MIN_ROW: f32 = 30.0;
/// Travel, in logical px at 1x, past which a finger scrolls the list
/// instead of tapping a row.
const DRAG_SLOP: f32 = 8.0;

thread_local! {
    static MENU_BINDINGS: std::cell::RefCell<crate::action::BindingMap> = std::cell::RefCell::new(crate::action::BindingMap::classic());
}

/// Points menu hints at `bindings`, copying them only when they changed.
pub(crate) fn set_bindings(bindings: &crate::action::BindingMap) {
    MENU_BINDINGS.with(|current| {
        if *current.borrow() != *bindings {
            current.borrow_mut().clone_from(bindings);
        }
    });
}

pub(crate) fn binding_hint(template: &str) -> String {
    use crate::action::Action;
    MENU_BINDINGS.with(|bindings| {
        let bindings = bindings.borrow();
        let mut text = template.to_string();
        for (token, action) in [
            ("{confirm}", Action::Confirm),
            ("{back}", Action::Back),
            ("{up}", Action::MenuUp),
            ("{down}", Action::MenuDown),
            ("{left}", Action::MenuLeft),
            ("{right}", Action::MenuRight),
            ("{delete}", Action::DeleteSave),
        ] {
            text = text.replace(token, &bindings.label(action));
        }
        text
    })
}

fn ui() -> f32 {
    crate::render::ui_scale()
}

/// Where a Controls row's columns sit: the action's name, then its
/// primary and secondary chords. Drawing and clicking share it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BindingColumns {
    /// The action's name.
    pub name: Rect,
    /// The primary chord.
    pub primary: Rect,
    /// The secondary chord.
    pub secondary: Rect,
}

impl BindingColumns {
    /// The columns across `row` at UI scale `s`.
    pub fn of(row: Rect, s: f32) -> Self {
        let column = |x: f32, w: f32| Rect::new(x, row.y, w, row.h);
        Self {
            name: column(row.x + 18.0 * s, row.w * 0.56),
            primary: column(row.x + row.w * 0.61, row.w * 0.18),
            secondary: column(row.x + row.w * 0.81, row.w * 0.18),
        }
    }

    /// The chord slot a pointer at `x` picks: each chord owns the space
    /// from its column to the next. `None` over the name.
    pub fn slot_at(&self, x: f32) -> Option<usize> {
        if x >= self.secondary.x {
            Some(1)
        } else if x >= self.primary.x {
            Some(0)
        } else {
            None
        }
    }
}

/// A titled, selectable list.
///
/// Three independent pieces of state, deliberately: `selected` is the
/// keyboard cursor and activation target, `scroll` is which window of
/// rows is shown, and `hover` is only a highlight. If hover moves the
/// selection or the window follows it, a stationary pointer can walk the
/// whole list by itself.
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
    /// frame, so only whole accumulated rows scroll.
    wheel_accum: f32,
    /// Row armed by a mouse press or the owning finger.
    press: Press<usize>,
    /// The first finger down, which scrolls the list once it drags. A
    /// touch-only player has no wheel or paging keys, so this is the
    /// only way to reach rows below the window.
    drag: Option<TouchDrag>,
    /// Section-label rows: drawn dimmer, skipped by the cursor, never
    /// activated.
    headers: Vec<usize>,
    /// Horizontal offset of the list as a fraction of the viewport
    /// width, zero for a centered list: the codex shifts its list left
    /// to make room for the page beside it.
    pub shift: f32,
}

/// A finger dragging a menu list.
#[derive(Debug, Clone, Copy)]
struct TouchDrag {
    id: u64,
    last_y: f32,
    /// Total distance moved, for the tap-versus-drag slop.
    travel: f32,
    /// Movement not yet spent on a whole row of scrolling.
    carry: f32,
    scrolling: bool,
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
            press: Press::default(),
            drag: None,
            headers,
            shift: 0.0,
        };
        menu.selected = menu.settle(0, false);
        menu
    }

    /// Whether a row is a section label.
    pub fn is_header(&self, index: usize) -> bool {
        self.headers.contains(&index)
    }

    /// The nearest non-header row from `index`, toward the start when
    /// `back`, without wrapping; `index` itself on an all-header list.
    fn settle(&self, index: usize, back: bool) -> usize {
        crate::nav::nearest(self.items.len(), index, back, |row| !self.is_header(row))
            .unwrap_or(index)
    }

    /// Moves the keyboard cursor and scrolls just enough to show it —
    /// the only coupling between selection and the scroll window.
    pub fn select(&mut self, index: usize) {
        self.selected = self.settle(index.min(self.items.len().saturating_sub(1)), false);
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
        self.scroll = (self.scroll.fit::<i64>() + delta)
            .clamp(0, max.fit::<i64>())
            .fit::<usize>();
        // The selection rides inside the window, so Enter never activates
        // a row the wheel has scrolled out of sight, and it never lands on
        // a header.
        let clamped = self
            .selected
            .clamp(self.scroll, self.scroll + visible.saturating_sub(1));
        self.selected = self.settle(clamped, clamped < self.selected);
    }

    /// Follows the dragging finger to `y`. Past the slop the drag owns
    /// the gesture: the armed row is dropped, and each row-height of
    /// travel scrolls one row, with the content following the finger.
    /// Returns whether the drag is scrolling.
    fn drag_to(&mut self, y: f32) -> bool {
        let Some(mut drag) = self.drag else {
            return false;
        };
        let dy = y - drag.last_y;
        drag.last_y = y;
        drag.travel += dy.abs();
        if !drag.scrolling && drag.travel > DRAG_SLOP * ui() {
            drag.scrolling = true;
            self.press.cancel();
            self.hover = None;
        }
        if drag.scrolling {
            drag.carry += dy;
            let (_, row, _, _) = self.layout();
            while drag.carry >= row {
                self.scroll_by(-1);
                drag.carry -= row;
            }
            while drag.carry <= -row {
                self.scroll_by(1);
                drag.carry += row;
            }
        }
        self.drag = Some(drag);
        drag.scrolling
    }

    fn row_at(&self, point: Vec2) -> Option<usize> {
        (0..self.items.len()).find(|i| self.item_rect(*i).is_some_and(|r| r.contains(point)))
    }

    /// Where the list lives this frame: top edge, row height, and the
    /// window of visible rows. The list fits itself between the title
    /// block and the hint line — rows pack as tight as `row_pitch`
    /// allows when the window is short, and past that the list scrolls
    /// around the selection instead of running off the screen.
    fn layout(&self) -> (f32, f32, usize, usize) {
        let s = ui();
        let top_bound = (view_h() * 0.36).max(view_h() * 0.28 + 64.0 * s);
        let bottom_bound = view_h() - 64.0 * s;
        let avail = (bottom_bound - top_bound).max(ITEM_HEIGHT * s);
        let n = self.items.len().max(1);
        let row = row_pitch(avail, n, s, crate::platform::TOUCH_ONLY);
        let visible = numeric::to_usize((avail / row).floor()).clamp(1, n);
        // The window is scroll state, clamped — never a function of the
        // selection, or hovering near an edge walks the list.
        let first = self.scroll.min(n.saturating_sub(visible));
        let top = top_bound + (avail - visible as f32 * row) * 0.5;
        (top, row, first, visible)
    }

    /// Touchable bounds for one visible row.
    pub(crate) fn item_rect(&self, index: usize) -> Option<Rect> {
        let s = ui();
        let (top, row, first, visible) = self.layout();
        if index < first || index >= first + visible {
            return None;
        }
        let width = if self.title == "CONTROLS" {
            (760.0 * s).min(view_w() - 32.0 * s)
        } else {
            ITEM_WIDTH * s
        };
        Some(Rect::new(
            (view_w() - width) * 0.5 + view_w() * self.shift,
            top + (index - first) as f32 * row,
            width,
            row - ROW_GAP * s,
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
        // An empty list has nothing to select, scroll, or activate —
        // and its wrap-around arithmetic divides by zero. The shelf can
        // legitimately be empty on a fresh profile.
        if self.items.is_empty() {
            return None;
        }
        for event in events {
            match *event {
                RawEvent::TouchDown { id, y, .. } if self.drag.is_none() => {
                    self.drag = Some(TouchDrag {
                        id,
                        last_y: y,
                        travel: 0.0,
                        carry: 0.0,
                        scrolling: false,
                    });
                }
                RawEvent::TouchMove { id, y, .. }
                    if self.drag.is_some_and(|drag| drag.id == id) && self.drag_to(y) =>
                {
                    continue;
                }
                RawEvent::TouchUp { id, .. }
                    if self
                        .drag
                        .is_some_and(|drag| drag.id == id && drag.scrolling) =>
                {
                    self.drag = None;
                    continue;
                }
                RawEvent::TouchUp { id, .. } if self.drag.is_some_and(|drag| drag.id == id) => {
                    self.drag = None;
                }
                _ => {}
            }
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
                    *mouse = vec2(x, y);
                    let row = self.row_at(vec2(x, y)).filter(|r| !self.is_header(*r));
                    self.press.mouse_down(row);
                }
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x,
                    y,
                } => {
                    *mouse = vec2(x, y);
                    let released_on = self.row_at(vec2(x, y));
                    if let Some(row) = self.press.mouse_up(released_on) {
                        self.selected = row;
                        return Some(row);
                    }
                }
                RawEvent::TouchDown { id, x, y } if self.press.touch_free() => {
                    *mouse = vec2(x, y);
                    self.hover = self.row_at(*mouse).filter(|r| !self.is_header(*r));
                    self.press.touch_down(id, self.hover);
                }
                RawEvent::TouchMove { id, x, y } if self.press.owns(id) => {
                    *mouse = vec2(x, y);
                    self.hover = self.row_at(*mouse).filter(|r| !self.is_header(*r));
                }
                RawEvent::TouchUp { id, x, y } if self.press.owns(id) => {
                    *mouse = vec2(x, y);
                    let released_on = self.row_at(*mouse);
                    if let Some(row) = self.press.touch_up(released_on) {
                        self.selected = row;
                        return Some(row);
                    }
                }
                RawEvent::KeyDown { .. } => match crate::nav::Nav::decode(event) {
                    Some(crate::nav::Nav::Confirm) if !self.is_header(self.selected) => {
                        return Some(self.selected);
                    }
                    Some(nav) => {
                        let (_, _, _, visible) = self.layout();
                        if let Some(row) = crate::nav::step_line(
                            self.items.len(),
                            self.selected,
                            nav,
                            crate::nav::Axis::Vertical,
                            visible,
                            |row| !self.is_header(row),
                        ) {
                            self.hover = None;
                            self.selected = row;
                            self.ensure_visible();
                        }
                    }
                    None => {}
                },
                _ => {}
            }
        }
        None
    }

    /// Draws the menu (over whatever the caller already drew).
    pub fn draw(&self, subtitle: &str) {
        self.draw_with_coaching(subtitle, None);
    }

    /// Draws the menu with `coaching` in the footer line in place of the
    /// standard key help. The footer is coaching either way: it fades in
    /// only when the player seems stuck.
    pub fn draw_with_coaching(&self, subtitle: &str, coaching: Option<&str>) {
        self.draw_face(subtitle, coaching, false);
    }

    /// Draws the menu while work it started runs: every row dimmed and
    /// inert-looking, `subtitle` saying what is happening, no footer.
    pub fn draw_busy(&self, subtitle: &str) {
        self.draw_face(subtitle, None, true);
    }

    fn draw_face(&self, subtitle: &str, coaching: Option<&str>, busy: bool) {
        let subtitle = binding_hint(subtitle);
        let s = ui();
        let title_size = 96.0 * s;
        let dims = measure_text(&self.title, None, numeric::font_size(title_size), 1.0);
        draw_text(
            &self.title,
            (view_w() - dims.width) * 0.5,
            view_h() * 0.28,
            title_size,
            TEXT_TITLE,
        );
        // The subtitle shrinks to fit the window; map blurbs run long.
        let mut sub_size = 20.0 * s;
        let mut sub_dims = measure_text(&subtitle, None, numeric::font_size(sub_size), 1.0);
        let max_width = view_w() * 0.55;
        if sub_dims.width > max_width {
            sub_size = (sub_size * max_width / sub_dims.width).max(12.0 * s);
            sub_dims = measure_text(&subtitle, None, numeric::font_size(sub_size), 1.0);
        }
        draw_text(
            &subtitle,
            (view_w() - sub_dims.width) * 0.5,
            view_h() * 0.28 + 34.0 * s,
            sub_size,
            TEXT_SECONDARY,
        );

        let (_, row, first, visible) = self.layout();
        let text_size = (26.0 * s * (row / (ITEM_HEIGHT * s))).clamp(18.0 * s, 26.0 * s);
        for (index, label) in self.items.iter().enumerate() {
            let Some(rect) = self.item_rect(index) else {
                continue;
            };
            if self.is_header(index) {
                let size = (20.0 * s).min(text_size);
                let dims = measure_text(label, None, numeric::font_size(size), 1.0);
                draw_text(
                    label,
                    rect.x + (rect.w - dims.width) * 0.5,
                    rect.y + rect.h * 0.68,
                    size,
                    TEXT_SECONDARY,
                );
                continue;
            }
            let selected = index == self.selected && !busy;
            let hovered = self.hover == Some(index) && !busy;
            if selected {
                draw_rectangle(rect.x, rect.y, rect.w, rect.h, SURFACE_MENU);
                draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 2.0, TEXT_TITLE);
            } else if hovered {
                draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.0, TEXT_SECONDARY);
            }
            let color = match (busy, selected) {
                (true, _) => TEXT_DISABLED,
                (false, true) => TEXT_PRIMARY,
                (false, false) => TEXT_BODY,
            };
            if self.title == "CONTROLS"
                && let Some((name, keys)) = label.rsplit_once(": ")
                && let Some((primary, secondary)) = keys.split_once(" | ")
            {
                let columns = BindingColumns::of(rect, s);
                for (text, column) in [
                    (name, columns.name),
                    (primary, columns.primary),
                    (secondary, columns.secondary),
                ] {
                    let measured =
                        measure_text(text, None, numeric::font_size(text_size), 1.0).width;
                    let size = text_size * (column.w / measured.max(1.0)).min(1.0);
                    draw_text(text, column.x, rect.y + rect.h * 0.68, size, color);
                }
            } else {
                draw_text(
                    label,
                    rect.x + 18.0 * s,
                    rect.y + rect.h * 0.68,
                    text_size,
                    color,
                );
            }
        }
        // Scroll cues when the list is windowed.
        if first > 0 {
            let r = self.item_rect(first).unwrap();
            draw_text(
                "^",
                r.x + r.w * 0.5,
                r.y - 6.0 * s,
                22.0 * s,
                TEXT_SECONDARY,
            );
        }
        if first + visible < self.items.len() {
            let r = self.item_rect(first + visible - 1).unwrap();
            draw_text(
                "v",
                r.x + r.w * 0.5,
                r.y + row + 14.0 * s,
                22.0 * s,
                TEXT_SECONDARY,
            );
        }

        if self.items.is_empty() || busy {
            return;
        }

        let scrolls = first > 0 || first + visible < self.items.len();
        let hint = coaching.map_or_else(
            || menu_footer(crate::platform::TOUCH_ONLY, scrolls),
            binding_hint,
        );
        let mut hint_size = 18.0 * s;
        let mut hint_dims = measure_text(&hint, None, numeric::font_size(hint_size), 1.0);
        let max_width = view_w() - 32.0 * s;
        if hint_dims.width > max_width {
            hint_size = (hint_size * max_width / hint_dims.width).max(12.0 * s);
            hint_dims = measure_text(&hint, None, numeric::font_size(hint_size), 1.0);
        }
        draw_text(
            &hint,
            (view_w() - hint_dims.width) * 0.5,
            view_h() - 24.0 * s,
            hint_size,
            crate::hints::fade(TEXT_SECONDARY),
        );
    }
}

/// The distance from one row to the next. A desktop list packs its rows
/// down to a readable minimum before it scrolls; a touch-only list keeps
/// every row a full fingertip target and scrolls sooner.
fn row_pitch(avail: f32, rows: usize, s: f32, touch_only: bool) -> f32 {
    let (min, max) = if touch_only {
        let pitch = crate::layout::MIN_TOUCH_TARGET + ROW_GAP;
        (pitch, pitch)
    } else {
        (MIN_ROW, ITEM_HEIGHT)
    };
    (avail / rows.max(1) as f32).clamp(min * s, max * s)
}

/// The line under every menu: its keys and clicks on desktop, taps on a
/// touch-only build, plus the drag only when the list actually scrolls.
/// ASCII on purpose: the default font has no glyphs for arrows.
fn menu_footer(touch_only: bool, scrolls: bool) -> String {
    if touch_only {
        return if scrolls {
            "tap to choose - drag to scroll"
        } else {
            "tap to choose"
        }
        .to_string();
    }
    MENU_BINDINGS.with(|bindings| {
        let bindings = bindings.borrow();
        format!(
            "{}/{} select - {} confirm - or click",
            bindings.label(crate::action::Action::MenuUp),
            bindings.label(crate::action::Action::MenuDown),
            bindings.label(crate::action::Action::Confirm)
        )
    })
}

/// Fog-free map previews, one per scenario. The driver's software
/// rasterizer draws the built state (the same pixels the golden tests pin).
/// Loading, building, and rasterizing a map takes long enough to hitch a
/// frame, so that runs on a worker; only the texture upload, which must stay
/// on the window thread, happens here. Entries are keyed by scenario path
/// because the discovery list can be rebuilt while the cache lives on.
pub struct PreviewCache {
    /// Present once requested: `None` while the worker is still drawing it,
    /// and for a scenario that failed to load or build.
    slots: std::collections::HashMap<Option<PathBuf>, Option<Texture2D>>,
    worker: PreviewWorker,
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self {
            slots: HashMap::default(),
            worker: PreviewWorker::spawn(),
        }
    }
}

impl PreviewCache {
    /// The preview for a scenario, requesting it on first sight. `None`
    /// until it is drawn, and for good when it cannot be: the browser just
    /// shows no panel for it.
    pub fn get(&mut self, entry: &ScenarioEntry) -> Option<&Texture2D> {
        for (path, pixels) in self.worker.finished.try_iter() {
            let texture = pixels.map(|pixels| {
                let texture = Texture2D::from_rgba8(pixels.width, pixels.height, &pixels.rgba);
                texture.set_filter(FilterMode::Nearest);
                texture
            });
            self.slots.insert(path, texture);
        }
        if !self.slots.contains_key(&entry.path) {
            self.slots.insert(entry.path.clone(), None);
            // A dead worker leaves the slot empty, which draws as no panel.
            self.worker.requests.send(entry.path.clone()).ok();
        }
        self.slots.get(&entry.path)?.as_ref()
    }
}

/// Rasterized preview bytes, ready for the window thread to upload.
struct PreviewPixels {
    width: u16,
    height: u16,
    rgba: Vec<u8>,
}

/// The thread that draws previews. It exits when the cache drops its sender.
struct PreviewWorker {
    requests: std::sync::mpsc::Sender<Option<PathBuf>>,
    finished: std::sync::mpsc::Receiver<(Option<PathBuf>, Option<PreviewPixels>)>,
}

impl PreviewWorker {
    fn spawn() -> Self {
        let (requests, queue) = std::sync::mpsc::channel::<Option<PathBuf>>();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for path in queue {
                let pixels = render_preview(path.as_deref());
                if done.send((path, pixels)).is_err() {
                    break;
                }
            }
        });
        Self { requests, finished }
    }
}

/// Draws one scenario's preview; `None` means the embedded skirmish.
fn render_preview(path: Option<&std::path::Path>) -> Option<PreviewPixels> {
    let scenario = match path {
        Some(path) => Scenario::load(path).ok()?,
        None => Scenario::skirmish(),
    };
    let state = scenario.build().ok()?;
    let pixmap = oxide_kit::render::render_state(&state);
    Some(PreviewPixels {
        width: pixmap.width().fit::<u16>(),
        height: pixmap.height().fit::<u16>(),
        rgba: pixmap.data().to_vec(),
    })
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
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(scenario) = Scenario::load(&path) {
                let blurb = scenario.meta.as_ref().map(|m| {
                    // The duration band shares one badge with the pace
                    // label it qualifies.
                    let pace = match (m.pace.is_empty(), m.duration.is_empty()) {
                        (false, false) => format!("{} | {}", m.pace, m.duration),
                        (false, true) => m.pace.clone(),
                        (true, _) => m.duration.clone(),
                    };
                    // Only badges that exist: a missing field must
                    // not render as a dangling separator.
                    let badges: Vec<&str> = [&pace, &m.mode, &m.richness]
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
    // key the remembered pick by path, so re-sorting can't move it.
    entries.sort_by_key(|e| (e.seats, e.label.to_lowercase()));
    entries
}

#[cfg(test)]
mod footer_tests;

#[cfg(test)]
mod preview_tests;

#[cfg(test)]
mod empty_tests;

#[cfg(test)]
mod header_tests;
