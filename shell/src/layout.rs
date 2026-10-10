//! Where the in-game HUD chrome sits this frame.
//!
//! The HUD layout pass (`render::hud`) computes a [`LayoutModel`] before
//! input and publishes it on the `Game`; hit-testing reads that model and
//! drawing consumes the same pass, so they never keep separate geometry
//! that could disagree.

use crate::numeric;
use crate::panel::CardAction;
use macroquad::prelude::{Rect, Vec2};

/// Where the persistent HUD chrome sits, in window pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutModel {
    /// Read-only performance panel; pointer presses must not reach the map.
    pub performance: Rect,
    /// Height of the top status bar.
    pub top_bar_h: f32,
    /// Top edge of the bottom panel band; the band runs to the window
    /// bottom. `f32::INFINITY` when no panel is shown.
    pub panel_top: f32,
    /// Right edge of the bottom panel band — the band hugs its content
    /// instead of spanning the window, so clicks past it reach the
    /// world. Zero when no panel is shown.
    pub panel_right: f32,
    /// Actual information and action regions; their open notch belongs to the world.
    pub panel_regions: [Rect; 2],
    /// The orders dock on the left edge (production ghosts / order
    /// chips); zero-sized when the queue is empty.
    pub orders: Rect,
    /// The minimap rectangle.
    pub minimap: Rect,
    /// The idle-worker badge in the top bar; zero-sized when nobody
    /// idles. Clicking it cycles idle harvesters.
    pub idle_badge: Rect,
    /// The under-attack badge beside it; zero-sized without a recent
    /// alert. Clicking it jumps the camera to the last alert.
    pub alert_badge: Rect,
    /// The control-group column's plate while it shows; its gaps are
    /// chrome too, so a near miss never reaches the world.
    pub group_column: Rect,
    /// The column's slots, by group; `None` while it is hidden.
    pub group_slots: [Option<(Rect, GroupSlot)>; crate::action::CONTROL_GROUPS],
    /// The menu button at the top bar's right edge; it opens the pause
    /// menu. Zero-sized while spectating.
    pub menu_button: Rect,
    /// The clock or PAUSED status beside the menu button; it toggles
    /// pause like the pause key. Zero-sized while spectating.
    pub pause_status: Rect,
    /// Persistent armed-command ribbon. It is chrome even away from
    /// the command band, and a press anywhere on it cancels the mode.
    pub mode_ribbon: Rect,
    /// The QUEUE toggle beside the ribbon, on touch-only builds.
    pub queue_toggle: Rect,
    /// Mixed-selection roster filters, separate from command cards.
    pub roster_slots: [(Rect, CardAction); 8],
    /// How many roster filters are live this frame.
    pub roster_count: usize,
    /// Command cards: rect plus the action a click performs — the
    /// renderer lays them out, hit-testing replays them verbatim.
    pub cards: [(Rect, CardAction); 16],
    /// How many cards are live this frame.
    pub card_count: usize,
    /// Queue thumbnails (production or orders), same contract.
    pub queue_slots: [(Rect, CardAction); 8],
    /// How many queue slots are live this frame.
    pub queue_count: usize,
    /// The Stop button above the dock; zero-sized while there is none.
    pub queue_stop: (Rect, CardAction),
}

impl Default for LayoutModel {
    fn default() -> Self {
        Self {
            performance: Rect::new(0.0, 0.0, 0.0, 0.0),
            top_bar_h: 0.0,
            panel_top: f32::INFINITY,
            panel_right: 0.0,
            panel_regions: [Rect::new(0.0, 0.0, 0.0, 0.0); 2],
            orders: Rect::new(0.0, 0.0, 0.0, 0.0),
            minimap: Rect::new(0.0, 0.0, 0.0, 0.0),
            idle_badge: Rect::new(0.0, 0.0, 0.0, 0.0),
            alert_badge: Rect::new(0.0, 0.0, 0.0, 0.0),
            group_column: Rect::new(0.0, 0.0, 0.0, 0.0),
            group_slots: [None; crate::action::CONTROL_GROUPS],
            menu_button: Rect::new(0.0, 0.0, 0.0, 0.0),
            pause_status: Rect::new(0.0, 0.0, 0.0, 0.0),
            mode_ribbon: Rect::new(0.0, 0.0, 0.0, 0.0),
            queue_toggle: Rect::new(0.0, 0.0, 0.0, 0.0),
            roster_slots: [(Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::None); 8],
            roster_count: 0,
            cards: [(Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::None); 16],
            card_count: 0,
            queue_slots: [(Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::None); 8],
            queue_count: 0,
            queue_stop: (Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::None),
        }
    }
}

/// Top-bar height in logical px at 1x scale, read by both hit-testing
/// and drawing.
pub const TOP_BAR_H: f32 = 40.0;

use crate::theme::MIN_TOUCH_TARGET;

/// Pads a hit rect out to the minimum touch target, centered. Touch
/// paths hit-test through this so small chrome stays tappable; mouse
/// paths keep the exact drawn rect.
pub fn touch_pad(rect: Rect, ui: f32) -> Rect {
    let min = MIN_TOUCH_TARGET * ui;
    let grow_w = (min - rect.w).max(0.0);
    let grow_h = (min - rect.h).max(0.0);
    Rect::new(
        rect.x - grow_w * 0.5,
        rect.y - grow_h * 0.5,
        rect.w + grow_w,
        rect.h + grow_h,
    )
}

/// How far a touch-only build holds the menu button off the right edge.
/// iPad screens round their corners, and iPadOS safe areas leave them
/// out, so a button in the corner is clipped.
const TOUCH_RIGHT_INSET: f32 = 20.0;
/// How much further a touch-only build holds the top bar's left group
/// in from the rounded corner.
const TOUCH_LEFT_INSET: f32 = 6.0;

/// The top bar's menu button: a badge-height square held off the right
/// edge, inside the bar so its padded touch target stays mostly chrome.
pub fn menu_button_rect(viewport_w: f32, ui: f32, touch_only: bool) -> Rect {
    let size = 34.0 * ui;
    let margin = if touch_only { TOUCH_RIGHT_INSET } else { 8.0 };
    Rect::new(viewport_w - size - margin * ui, 3.0 * ui, size, size)
}

/// The top bar's measured text widths, in window pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct TopBarText {
    /// The scrap count.
    pub scrap: f32,
    /// The passive income line.
    pub passive: f32,
    /// The UNITS label.
    pub units_label: f32,
    /// The unit count.
    pub units: f32,
    /// The idle badge's text, while any harvester idles.
    pub idle: Option<f32>,
    /// The under-attack badge's text, while an alert is recent.
    pub alert: Option<f32>,
    /// The clock, speed, or PAUSED status.
    pub status: f32,
}

/// Where everything in the top bar sits, in window pixels. Text entries
/// are left edges; badges are their drawn rects (zero when absent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TopBar {
    pub scrap_label_x: f32,
    pub scrap_x: f32,
    pub passive_x: f32,
    pub units_x: f32,
    pub count_x: f32,
    pub idle_badge: Rect,
    pub alert_badge: Rect,
    pub menu_button: Rect,
    pub status_x: f32,
    pub pause_status: Rect,
    /// The span the FPS readout may use: from the left group's end to
    /// the status's start.
    pub status_space: (f32, f32),
}

/// Lays out the top bar from measured text: the bank, income, and unit
/// count run left to right from fixed minimums, the idle and alert badges
/// follow them, and the status hangs off the menu button at the right.
pub(crate) fn top_bar(viewport_w: f32, ui: f32, touch_only: bool, text: TopBarText) -> TopBar {
    let inset = if touch_only { TOUCH_LEFT_INSET } else { 0.0 } * ui;
    let scrap_label_x = 12.0 * ui + inset;
    let scrap_x = 70.0 * ui + inset;
    let passive_x = (scrap_x + text.scrap + 16.0 * ui).max(151.0 * ui + inset);
    let units_x = (passive_x + text.passive + 16.0 * ui).max(284.0 * ui + inset);
    let count_x = units_x + (text.units_label + 12.0 * ui).max(60.0 * ui);
    let badge = |x: f32, width: f32| Rect::new(x, 3.0 * ui, width + 18.0 * ui, 34.0 * ui);
    let badges_x = count_x + (text.units + 20.0 * ui).max(45.0 * ui);
    let idle_badge = text.idle.map_or(Rect::new(0.0, 0.0, 0.0, 0.0), |width| {
        badge(badges_x, width)
    });
    let alert_x = if idle_badge.w > 0.0 {
        idle_badge.x + idle_badge.w + 8.0 * ui
    } else {
        badges_x
    };
    let alert_badge = text
        .alert
        .map_or(Rect::new(0.0, 0.0, 0.0, 0.0), |width| badge(alert_x, width));
    let menu_button = menu_button_rect(viewport_w, ui, touch_only);
    let status_x = menu_button.x - 12.0 * ui - text.status;
    let occupied_right = (count_x + text.units)
        .max(idle_badge.x + idle_badge.w)
        .max(alert_badge.x + alert_badge.w);
    TopBar {
        scrap_label_x,
        scrap_x,
        passive_x,
        units_x,
        count_x,
        idle_badge,
        alert_badge,
        menu_button,
        status_x,
        pause_status: Rect::new(
            status_x - 6.0 * ui,
            3.0 * ui,
            text.status + 12.0 * ui,
            34.0 * ui,
        ),
        status_space: (occupied_right, status_x),
    }
}

/// Where the control-group column sits: a plate above the minimap's
/// right edge, mirroring the orders dock on the left, with one square
/// slot per group and group 1 on top. A window too short for one column
/// wraps it into more, leftward.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GroupColumn {
    pub plate: Rect,
    pub slots: [Rect; crate::action::CONTROL_GROUPS],
}

/// Lays out the control-group column over `minimap`. None when the
/// minimap is hidden (a small window's dense panel covers it) or no slot
/// fits below the top bar.
pub(crate) fn group_column(ui: f32, touch_only: bool, minimap: Rect) -> Option<GroupColumn> {
    if minimap.w <= 0.0 {
        return None;
    }
    let side = if touch_only { MIN_TOUCH_TARGET } else { 40.0 } * ui;
    let (gap, pad) = (4.0 * ui, 8.0 * ui);
    let bottom = minimap.y - 8.0 * ui;
    let room = bottom - (TOP_BAR_H + 8.0) * ui - 2.0 * pad;
    let rows_fit = ((room + gap) / (side + gap)).floor();
    if rows_fit < 1.0 {
        return None;
    }
    let groups = crate::action::CONTROL_GROUPS;
    let rows = numeric::to_usize(rows_fit).min(groups);
    let columns = groups.div_ceil(rows);
    let span = |n: usize| n as f32 * side + n.saturating_sub(1) as f32 * gap;
    let (w, h) = (span(columns) + 2.0 * pad, span(rows) + 2.0 * pad);
    let plate = Rect::new(minimap.x + minimap.w - w, bottom - h, w, h);
    let slots = std::array::from_fn(|i| {
        Rect::new(
            plate.x + pad + (i / rows) as f32 * (side + gap),
            plate.y + pad + (i % rows) as f32 * (side + gap),
            side,
            side,
        )
    });
    Some(GroupColumn { plate, slots })
}

/// What a control-group slot does when pressed. Ctrl-click, or a
/// fingertip's long-press, saves the selection to any slot instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupSlot {
    /// A saved group: recall it.
    Recall(u8),
    /// The strip's "+": save the selection as this empty group.
    Assign(u8),
    /// An empty group's place, holding the strip steady.
    Empty(u8),
}

impl GroupSlot {
    /// The group number, 1-based like its key.
    pub fn number(self) -> u8 {
        match self {
            Self::Recall(n) | Self::Assign(n) | Self::Empty(n) => n,
        }
    }
}

/// The control-group slot under `p`: the drawn rect for a cursor, the
/// padded fingertip target when `touch_ui` carries the ui scale.
pub fn group_slot_under(layout: &LayoutModel, p: Vec2, touch_ui: Option<f32>) -> Option<GroupSlot> {
    layout
        .group_slots
        .iter()
        .flatten()
        .find(|(rect, _)| match touch_ui {
            Some(ui) => touch_pad(*rect, ui).contains(p),
            None => rect.contains(p),
        })
        .map(|(_, slot)| *slot)
}

/// Which side of the rect it describes a tooltip prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TooltipSide {
    /// Above the anchor, left edges aligned — the command band's rule:
    /// the cards sit in a row, so the space above them is free.
    Above,
    /// Right of the anchor, centered on it — the orders dock's rule:
    /// the dock hugs the left edge and stacks upward, so "above" would
    /// land on the neighbouring chip instead of open screen.
    RightOf,
}

/// Places a tooltip box against the rect it describes. `gap` is the
/// clearance from the anchor and the margin held against the window
/// edges, so callers pass it already ui-scaled.
///
/// The box is clamped on screen: a tall box on a short window pins under
/// the top bar, and a wide box near the right edge slides back inside. A
/// box with nowhere to fit pins to the top, clipping its tail rather than
/// its header.
pub fn tooltip_origin(
    anchor: Rect,
    size: Vec2,
    side: TooltipSide,
    viewport: Vec2,
    top_bar_h: f32,
    gap: f32,
) -> Vec2 {
    let (x, y) = match side {
        TooltipSide::Above => (anchor.x, anchor.y - size.y - gap),
        TooltipSide::RightOf => (
            anchor.x + anchor.w + gap,
            anchor.y + (anchor.h - size.y) * 0.5,
        ),
    };
    let max_x = (viewport.x - size.x - gap).max(0.0);
    let min_y = top_bar_h + gap;
    let max_y = (viewport.y - size.y - gap).max(min_y);
    Vec2::new(x.clamp(0.0, max_x), y.clamp(min_y, max_y))
}

impl LayoutModel {
    /// Whether persistent chrome (top bar or panel band) owns this
    /// point — such clicks must never reach the world. The minimap has
    /// its own richer meaning and is tested separately.
    pub fn chrome_owns(&self, p: Vec2) -> bool {
        p.y <= self.top_bar_h
            || (self.performance.w > 0.0 && self.performance.contains(p))
            || self
                .panel_regions
                .iter()
                .any(|rect| rect.w > 0.0 && rect.contains(p))
            || (self.orders.w > 0.0 && self.orders.contains(p))
            || (self.mode_ribbon.w > 0.0 && self.mode_ribbon.contains(p))
            || (self.queue_toggle.w > 0.0 && self.queue_toggle.contains(p))
            || (self.group_column.w > 0.0 && self.group_column.contains(p))
    }
}

/// The panel row a card sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardRow {
    Roster,
    Cards,
    Queue,
    /// The Stop button above the dock.
    Stop,
}

/// A published panel card under a pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CardHit {
    pub(crate) row: CardRow,
    pub(crate) index: usize,
    pub(crate) rect: Rect,
    pub(crate) action: CardAction,
}

/// The card under `p`, searching the roster, then command cards, then
/// the queue and its Stop button. A fingertip (`touch_ui`) hits through each card's padded
/// touch target; a mouse hits the drawn rect.
pub(crate) fn card_under(layout: &LayoutModel, p: Vec2, touch_ui: Option<f32>) -> Option<CardHit> {
    let rows: [(CardRow, &[(Rect, CardAction)]); 4] = [
        (CardRow::Roster, &layout.roster_slots[..layout.roster_count]),
        (CardRow::Cards, &layout.cards[..layout.card_count]),
        (CardRow::Queue, &layout.queue_slots[..layout.queue_count]),
        (CardRow::Stop, std::slice::from_ref(&layout.queue_stop)),
    ];
    rows.into_iter().find_map(|(row, slots)| {
        slots
            .iter()
            .enumerate()
            .find_map(|(index, &(rect, action))| {
                let target = touch_ui.map_or(rect, |ui| touch_pad(rect, ui));
                (rect.w > 0.0 && target.contains(p)).then_some(CardHit {
                    row,
                    index,
                    rect,
                    action,
                })
            })
    })
}

#[cfg(test)]
mod tests;
