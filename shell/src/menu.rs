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

use crate::press::{ScrollPress, Swipe};
use crate::theme::{
    SURFACE_MENU, Stroke, TEXT_BODY, TEXT_DISABLED, TEXT_PRIMARY, TEXT_SECONDARY, TEXT_TITLE,
};

const ITEM_HEIGHT: f32 = 44.0;
const ITEM_WIDTH: f32 = 420.0;
/// The space between rows, left out of each row's target.
const ROW_GAP: f32 = 6.0;
/// The tightest a desktop list packs its rows before it scrolls.
const MIN_ROW: f32 = 30.0;

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

/// What a menu row says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    /// One line of text.
    Text(String),
    /// A Controls row: the action's name and its primary and secondary
    /// chords, drawn in [`BindingColumns`]; `marked` brackets the chord
    /// the keyboard has chosen.
    Binding {
        name: String,
        keys: [String; 2],
        marked: Option<usize>,
    },
}

impl std::fmt::Display for Label {
    /// The row as one line, as automation reads it: `Pan up: W | [I]`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Label::Text(text) => f.write_str(text),
            Label::Binding { name, keys, marked } => {
                let key = |slot: usize| {
                    if *marked == Some(slot) {
                        format!("[{}]", keys[slot])
                    } else {
                        keys[slot].clone()
                    }
                };
                write!(f, "{name}: {} | {}", key(0), key(1))
            }
        }
    }
}

impl From<&str> for Label {
    fn from(text: &str) -> Self {
        Label::Text(text.to_string())
    }
}

impl From<String> for Label {
    fn from(text: String) -> Self {
        Label::Text(text)
    }
}

/// One line of a menu: a section heading, or a row standing for `R`.
#[derive(Debug, Clone, PartialEq)]
pub enum Line<R> {
    /// A section label: drawn dimmer, skipped by the cursor, never
    /// activated.
    Header(String),
    /// A selectable row.
    Row(Label, R),
}

/// A row the player activated: what it stands for and, for a Controls
/// row clicked or tapped on a chord, which chord.
#[derive(Debug, Clone, PartialEq)]
pub struct Activation<R> {
    pub value: R,
    pub column: Option<usize>,
}

/// What automation reads off a menu.
pub struct MenuView {
    pub title: String,
    pub selected: usize,
    pub items: Vec<String>,
    pub visible_range: [usize; 2],
    pub hover: Option<usize>,
}

/// Where the list lives this frame.
#[derive(Debug, Clone, Copy)]
struct ListWindow {
    /// The first visible row's top edge.
    top: f32,
    /// The distance from one row to the next.
    pitch: f32,
    /// The first visible row.
    first: usize,
    /// How many rows show.
    visible: usize,
}

/// A titled, selectable list whose rows stand for values of `R`.
///
/// Three independent pieces of state, deliberately: `selected` is the
/// keyboard cursor and activation target, `scroll` is which window of
/// rows is shown, and `hover` is only a highlight. If hover moves the
/// selection or the window follows it, a stationary pointer can walk the
/// whole list by itself.
pub struct Menu<R> {
    /// Heading above the list.
    pub title: String,
    lines: Vec<Line<R>>,
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
    /// Row armed by a mouse press or the owning finger. The first finger
    /// down scrolls the list once it drags: a touch-only player has no
    /// wheel or paging keys, so this is the only way to reach rows below
    /// the window.
    press: ScrollPress<usize>,
    /// Drag travel not yet spent on a whole row of scrolling.
    carry: f32,
    /// Horizontal offset of the list as a fraction of the viewport
    /// width, zero for a centered list: the codex shifts its list left
    /// to make room for the page beside it.
    pub shift: f32,
}

fn view_w() -> f32 {
    crate::render::viewport().x
}

fn view_h() -> f32 {
    crate::render::viewport().y
}

impl Menu<usize> {
    /// A plain list whose rows stand for their own indices.
    pub fn list(title: impl Into<String>, items: Vec<String>) -> Self {
        Self::new(
            title,
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| Line::Row(Label::Text(item), index))
                .collect(),
        )
    }
}

impl<R: Clone> Menu<R> {
    /// A menu over `rows` with no headings, the first row highlighted.
    pub fn rows(title: impl Into<String>, rows: impl IntoIterator<Item = (Label, R)>) -> Self {
        Self::new(
            title,
            rows.into_iter()
                .map(|(label, value)| Line::Row(label, value))
                .collect(),
        )
    }

    /// A menu over `lines`, the first row highlighted.
    pub fn new(title: impl Into<String>, lines: Vec<Line<R>>) -> Self {
        let mut menu = Self {
            title: title.into(),
            lines,
            selected: 0,
            scroll: 0,
            hover: None,
            wheel_accum: 0.0,
            press: ScrollPress::default(),
            carry: 0.0,
            shift: 0.0,
        };
        menu.selected = menu.settle(0, false);
        menu
    }

    /// Whether a line is a section label.
    pub fn is_header(&self, index: usize) -> bool {
        matches!(self.lines.get(index), Some(Line::Header(_)))
    }

    /// The menu's lines.
    pub fn lines(&self) -> &[Line<R>] {
        &self.lines
    }

    /// What the selected row stands for; `None` on an empty or
    /// all-heading list.
    pub fn value(&self) -> Option<&R> {
        match self.lines.get(self.selected)? {
            Line::Row(_, value) => Some(value),
            Line::Header(_) => None,
        }
    }

    /// Selects the first row whose value `pick` accepts; false when none
    /// does.
    pub fn select_where(&mut self, pick: impl Fn(&R) -> bool) -> bool {
        let found = self
            .lines
            .iter()
            .position(|line| matches!(line, Line::Row(_, value) if pick(value)));
        if let Some(index) = found {
            self.select(index);
        }
        found.is_some()
    }

    /// Replaces the lines in place, keeping the cursor, the scroll window
    /// and the pointer's press: a row relabelled after it cycles a value
    /// stays under the cursor.
    pub fn set_lines(&mut self, lines: Vec<Line<R>>) {
        self.lines = lines;
        self.selected = self.settle(self.selected.min(self.lines.len().saturating_sub(1)), false);
    }

    /// The rows as automation reads them.
    pub fn view(&self) -> MenuView {
        MenuView {
            title: self.title.clone(),
            selected: self.selected,
            items: self
                .lines
                .iter()
                .map(|line| match line {
                    Line::Header(text) => text.clone(),
                    Line::Row(label, _) => label.to_string(),
                })
                .collect(),
            visible_range: self.visible_range(),
            hover: self.hover,
        }
    }

    /// Whether any row draws as Controls columns, which widen the list.
    fn wide(&self) -> bool {
        self.lines
            .iter()
            .any(|line| matches!(line, Line::Row(Label::Binding { .. }, _)))
    }

    /// The nearest non-header row from `index`, toward the start when
    /// `back`, without wrapping; `index` itself on an all-header list.
    fn settle(&self, index: usize, back: bool) -> usize {
        crate::nav::nearest(self.lines.len(), index, back, |row| !self.is_header(row))
            .unwrap_or(index)
    }

    /// Moves the keyboard cursor and scrolls just enough to show it —
    /// the only coupling between selection and the scroll window.
    pub fn select(&mut self, index: usize) {
        self.selected = self.settle(index.min(self.lines.len().saturating_sub(1)), false);
        self.ensure_visible();
    }

    fn ensure_visible(&mut self) {
        let visible = self.window().visible;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
    }

    fn scroll_by(&mut self, delta: i64) {
        let visible = self.window().visible;
        let max = self.lines.len().saturating_sub(visible);
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

    /// Scrolls whole rows as a dragging finger moves the list `dy` px.
    /// The content follows the finger.
    fn drag_by(&mut self, dy: f32, began: bool) {
        if began {
            self.carry = 0.0;
            self.hover = None;
        }
        self.carry += dy;
        let row = self.window().pitch;
        while self.carry >= row {
            self.scroll_by(-1);
            self.carry -= row;
        }
        while self.carry <= -row {
            self.scroll_by(1);
            self.carry += row;
        }
    }

    fn row_at(&self, point: Vec2) -> Option<usize> {
        (0..self.lines.len()).find(|i| self.item_rect(*i).is_some_and(|r| r.contains(point)))
    }

    /// Where the list lives this frame: top edge, row height, and the
    /// window of visible rows. The list fits itself between the title
    /// block and the hint line — rows pack as tight as `row_pitch`
    /// allows when the window is short, and past that the list scrolls
    /// around the selection instead of running off the screen.
    fn window(&self) -> ListWindow {
        let s = ui();
        let top_bound = (view_h() * 0.36).max(view_h() * 0.28 + 64.0 * s);
        let bottom_bound = view_h() - 64.0 * s;
        let avail = (bottom_bound - top_bound).max(ITEM_HEIGHT * s);
        let n = self.lines.len().max(1);
        let pitch = row_pitch(avail, n, s, crate::platform::TOUCH_ONLY);
        let visible = numeric::to_usize((avail / pitch).floor()).clamp(1, n);
        ListWindow {
            top: top_bound + (avail - visible as f32 * pitch) * 0.5,
            pitch,
            // The window is scroll state, clamped — never a function of
            // the selection, or hovering near an edge walks the list.
            first: self.scroll.min(n.saturating_sub(visible)),
            visible,
        }
    }

    /// Touchable bounds for one visible row.
    pub(crate) fn item_rect(&self, index: usize) -> Option<Rect> {
        let s = ui();
        let ListWindow {
            top,
            pitch,
            first,
            visible,
        } = self.window();
        if index < first || index >= first + visible {
            return None;
        }
        let width = if self.wide() {
            (760.0 * s).min(view_w() - 32.0 * s)
        } else {
            ITEM_WIDTH * s
        };
        Some(Rect::new(
            (view_w() - width) * 0.5 + view_w() * self.shift,
            top + (index - first) as f32 * pitch,
            width,
            pitch - ROW_GAP * s,
        ))
    }

    /// Half-open range of rows currently drawn by the scroll window.
    pub fn visible_range(&self) -> [usize; 2] {
        let window = self.window();
        [window.first, window.first + window.visible]
    }

    /// Feeds a frame of events through the menu; returns the activated
    /// row, if any. Mouse position updates come along in the same events.
    pub fn handle(&mut self, events: &[RawEvent], mouse: &mut Vec2) -> Option<Activation<R>> {
        // An empty list has nothing to select, scroll, or activate —
        // and its wrap-around arithmetic divides by zero. The shelf can
        // legitimately be empty on a fresh profile.
        if self.lines.is_empty() {
            return None;
        }
        for event in events {
            let mut press = self.press;
            let swipe = press.feed(event, ui(), |p, _| {
                self.row_at(p).filter(|row| !self.is_header(*row))
            });
            self.press = press;
            match swipe {
                Swipe::Scrolled { dy, began } => {
                    self.drag_by(dy, began);
                    continue;
                }
                Swipe::Activated(row) => {
                    let point = crate::press::position(event);
                    if let Some(p) = point {
                        *mouse = p;
                    }
                    self.selected = row;
                    let column = point.zip(self.item_rect(row)).and_then(|(p, rect)| {
                        matches!(self.lines[row], Line::Row(Label::Binding { .. }, _))
                            .then(|| BindingColumns::of(rect, ui()).slot_at(p.x))
                            .flatten()
                    });
                    return self.activation(row, column);
                }
                Swipe::Held | Swipe::Ignored => {}
            }
            match *event {
                RawEvent::MouseMove { x, y } => {
                    *mouse = vec2(x, y);
                    self.hover = self.row_at(*mouse).filter(|r| !self.is_header(*r));
                }
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x,
                    y,
                }
                | RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x,
                    y,
                } => *mouse = vec2(x, y),
                RawEvent::TouchDown { id, x, y } | RawEvent::TouchMove { id, x, y }
                    if self.press.owns(id) =>
                {
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
                RawEvent::KeyDown { .. } => match crate::nav::Nav::decode(event) {
                    Some(crate::nav::Nav::Confirm) if !self.is_header(self.selected) => {
                        return self.activation(self.selected, None);
                    }
                    Some(nav) => {
                        let visible = self.window().visible;
                        if let Some(row) = crate::nav::step_line(
                            self.lines.len(),
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

    fn activation(&self, row: usize, column: Option<usize>) -> Option<Activation<R>> {
        match &self.lines[row] {
            Line::Row(_, value) => Some(Activation {
                value: value.clone(),
                column,
            }),
            Line::Header(_) => None,
        }
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
        let title_size = crate::theme::Type::Display.at(s);
        let dims = measure_text(&self.title, None, numeric::font_size(title_size), 1.0);
        draw_text(
            &self.title,
            (view_w() - dims.width) * 0.5,
            view_h() * 0.28,
            title_size,
            TEXT_TITLE,
        );
        // The subtitle shrinks to fit the window; map blurbs run long.
        let sub_size = crate::typography::fit(
            &subtitle,
            crate::theme::Type::Heading.at(s),
            view_w() * 0.55,
            12.0 * s,
        );
        let sub_dims = measure_text(&subtitle, None, numeric::font_size(sub_size), 1.0);
        draw_text(
            &subtitle,
            (view_w() - sub_dims.width) * 0.5,
            view_h() * 0.28 + 34.0 * s,
            sub_size,
            TEXT_SECONDARY,
        );

        let ListWindow {
            pitch: row,
            first,
            visible,
            ..
        } = self.window();
        let text_size = (crate::theme::Type::Row.at(s) * (row / (ITEM_HEIGHT * s))).clamp(
            crate::theme::Type::Label.at(s),
            crate::theme::Type::Row.at(s),
        );
        for (index, line) in self.lines.iter().enumerate() {
            let Some(rect) = self.item_rect(index) else {
                continue;
            };
            let label = match line {
                Line::Header(text) => {
                    let size = crate::theme::Type::Heading.at(s).min(text_size);
                    let dims = measure_text(text, None, numeric::font_size(size), 1.0);
                    draw_text(
                        text,
                        rect.x + (rect.w - dims.width) * 0.5,
                        rect.y + rect.h * 0.68,
                        size,
                        TEXT_SECONDARY,
                    );
                    continue;
                }
                Line::Row(label, _) => label,
            };
            let selected = index == self.selected && !busy;
            let hovered = self.hover == Some(index) && !busy;
            if selected {
                draw_rectangle(rect.x, rect.y, rect.w, rect.h, SURFACE_MENU);
                draw_rectangle_lines(
                    rect.x,
                    rect.y,
                    rect.w,
                    rect.h,
                    Stroke::Focus.at(s),
                    TEXT_TITLE,
                );
            } else if hovered {
                draw_rectangle_lines(
                    rect.x,
                    rect.y,
                    rect.w,
                    rect.h,
                    Stroke::Hairline.at(s),
                    TEXT_SECONDARY,
                );
            }
            let color = match (busy, selected) {
                (true, _) => TEXT_DISABLED,
                (false, true) => TEXT_PRIMARY,
                (false, false) => TEXT_BODY,
            };
            match label {
                Label::Binding { name, keys, marked } => {
                    let columns = BindingColumns::of(rect, s);
                    let key = |slot: usize| {
                        if *marked == Some(slot) {
                            format!("[{}]", keys[slot])
                        } else {
                            keys[slot].clone()
                        }
                    };
                    for (text, column) in [
                        (name.clone(), columns.name),
                        (key(0), columns.primary),
                        (key(1), columns.secondary),
                    ] {
                        let size = crate::typography::fit(&text, text_size, column.w, 1.0);
                        draw_text(&text, column.x, rect.y + rect.h * 0.68, size, color);
                    }
                }
                Label::Text(text) => {
                    draw_text(
                        text,
                        rect.x + 18.0 * s,
                        rect.y + rect.h * 0.68,
                        text_size,
                        color,
                    );
                }
            }
        }
        // Scroll cues when the list is windowed.
        if first > 0 {
            let r = self.item_rect(first).unwrap();
            draw_text(
                "^",
                r.x + r.w * 0.5,
                r.y - 6.0 * s,
                crate::theme::Type::Heading.at(s),
                TEXT_SECONDARY,
            );
        }
        if first + visible < self.lines.len() {
            let r = self.item_rect(first + visible - 1).unwrap();
            draw_text(
                "v",
                r.x + r.w * 0.5,
                r.y + row + 14.0 * s,
                crate::theme::Type::Heading.at(s),
                TEXT_SECONDARY,
            );
        }

        if self.lines.is_empty() || busy {
            return;
        }

        let scrolls = first > 0 || first + visible < self.lines.len();
        let hint = coaching.map_or_else(
            || menu_footer(crate::platform::hands(), scrolls),
            binding_hint,
        );
        let hint_size = crate::typography::fit(
            &hint,
            crate::theme::Type::Label.at(s),
            view_w() - 32.0 * s,
            12.0 * s,
        );
        let hint_dims = measure_text(&hint, None, numeric::font_size(hint_size), 1.0);
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
        let pitch = crate::theme::MIN_TOUCH_TARGET + ROW_GAP;
        (pitch, pitch)
    } else {
        (MIN_ROW, ITEM_HEIGHT)
    };
    (avail / rows.max(1) as f32).clamp(min * s, max * s)
}

/// The line under every menu: its keys once the player has a keyboard,
/// the pointer's verb, and without keys the drag or scroll only when the
/// list actually scrolls. ASCII on purpose: the default font has no
/// glyphs for arrows.
fn menu_footer(hands: crate::platform::Hands, scrolls: bool) -> String {
    let verb = if hands.touch() { "tap" } else { "click" };
    if !hands.keys {
        let scroll = match (scrolls, hands.touch()) {
            (false, _) => "",
            (true, true) => " - drag to scroll",
            (true, false) => " - scroll for more",
        };
        return format!("{verb} to choose{scroll}");
    }
    MENU_BINDINGS.with(|bindings| {
        let bindings = bindings.borrow();
        format!(
            "{}/{} select - {} confirm - or {verb}",
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
