//! The one description of where chrome sits this frame.
//!
//! The renderer computes a [`LayoutModel`] as it draws and publishes it
//! on the `Game`; hit-testing reads the same model. There is no second
//! copy of the geometry to fall out of sync — the 0.8 bug where clicks
//! leaked through the palette's second row existed precisely because
//! drawing and hit-testing each kept their own arithmetic. New screens
//! and widgets grow this model rather than freehand math.

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

/// Top-bar height in logical px at 1x scale — the ONE source both the
/// layout's hit-testing and the chrome's drawing read (the duplicated-
/// geometry class stays structurally extinct only while it isn't
/// duplicated).
pub const TOP_BAR_H: f32 = 40.0;

/// Minimum touch target edge in logical px (platform guidance says a
/// fingertip needs ~44).
pub const MIN_TOUCH_TARGET: f32 = 44.0;

/// Pads a hit rect out to the minimum touch target, centered — the
/// TOUCH paths hit-test through this so small chrome stays tappable;
/// mouse paths keep the exact drawn rect.
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
    let rows = (rows_fit as usize).min(groups);
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
/// clearance from the anchor AND the margin held against the window
/// edges, so callers pass it already ui-scaled.
///
/// Clamping is the point: a tall box on a short window pins under the
/// top bar rather than climbing off screen, and a wide box near the
/// right edge slides back inside. A box with nowhere to fit pins to
/// the top — a clipped tail beats a clipped header.
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
    /// Computes the frame's chrome geometry. `panel_top` is the band's
    /// top edge (`f32::INFINITY` when no panel is shown).
    #[allow(clippy::too_many_arguments)]
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the slot arrays move into the model"
    )]
    pub fn compute(
        viewport: Vec2,
        ui: f32,
        panel_top: f32,
        panel_right: f32,
        orders: Rect,
        minimap: Rect,
        idle_badge: Rect,
        menu_button: Rect,
        pause_status: Rect,
        mode_ribbon: Rect,
        roster_slots: [(Rect, CardAction); 8],
        roster_count: usize,
        cards: [(Rect, CardAction); 16],
        card_count: usize,
        queue_slots: [(Rect, CardAction); 8],
        queue_count: usize,
    ) -> Self {
        Self {
            top_bar_h: TOP_BAR_H * ui,
            performance: Rect::new(0.0, 0.0, 0.0, 0.0),
            panel_top,
            panel_right,
            panel_regions: [
                Rect::new(
                    0.0,
                    panel_top,
                    panel_right,
                    (viewport.y - panel_top).max(0.0),
                ),
                Rect::new(0.0, 0.0, 0.0, 0.0),
            ],
            orders,
            minimap,
            idle_badge,
            alert_badge: Rect::new(0.0, 0.0, 0.0, 0.0),
            group_column: Rect::new(0.0, 0.0, 0.0, 0.0),
            group_slots: [None; crate::action::CONTROL_GROUPS],
            menu_button,
            pause_status,
            mode_ribbon,
            queue_toggle: Rect::new(0.0, 0.0, 0.0, 0.0),
            roster_slots,
            roster_count,
            cards,
            card_count,
            queue_slots,
            queue_count,
            queue_stop: (Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::None),
        }
    }

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
mod tests {
    use super::*;
    use macroquad::prelude::vec2;

    #[test]
    fn selection_corner_owns_its_arms_but_not_the_open_notch() {
        let model = LayoutModel {
            top_bar_h: 40.0,
            panel_regions: [
                Rect::new(0.0, 500.0, 228.0, 300.0),
                Rect::new(228.0, 724.0, 500.0, 76.0),
            ],
            orders: Rect::new(0.0, 388.0, 204.0, 112.0),
            ..LayoutModel::default()
        };
        assert!(model.chrome_owns(vec2(100.0, 600.0)));
        assert!(model.chrome_owns(vec2(400.0, 760.0)));
        assert!(model.chrome_owns(vec2(100.0, 410.0)));
        assert!(!model.chrome_owns(vec2(400.0, 600.0)));
        assert!(!model.chrome_owns(vec2(229.0, 723.0)));
        assert!(!model.chrome_owns(vec2(800.0, 760.0)));
    }

    #[test]
    fn card_hits_pad_for_fingertips_and_search_roster_first() {
        let mut model = LayoutModel::default();
        let chip = Rect::new(100.0, 500.0, 24.0, 24.0);
        let card = Rect::new(200.0, 500.0, 60.0, 60.0);
        model.roster_slots[0] = (chip, CardAction::FilterKind(oxide_sim::UnitKind::Harvester));
        model.roster_count = 1;
        model.cards[0] = (Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::Upgrade);
        model.cards[1] = (card, CardAction::ArmRally);
        model.card_count = 2;
        model.cards[2] = (Rect::new(300.0, 500.0, 60.0, 60.0), CardAction::ClearRally);

        let beside_chip = vec2(chip.right() + 5.0, chip.center().y);
        assert_eq!(
            card_under(&model, beside_chip, None),
            None,
            "a mouse hits the drawn chip"
        );
        let hit = card_under(&model, beside_chip, Some(1.0)).expect("a fingertip reaches the pad");
        assert_eq!((hit.row, hit.index, hit.rect), (CardRow::Roster, 0, chip));

        let hit = card_under(&model, card.center(), None).expect("the live card");
        assert_eq!(
            (hit.row, hit.index, hit.action),
            (CardRow::Cards, 1, CardAction::ArmRally)
        );
        assert_eq!(
            card_under(&model, vec2(0.0, 0.0), Some(1.0)),
            None,
            "zero-size slots are empty"
        );
        assert_eq!(
            card_under(&model, vec2(330.0, 530.0), None),
            None,
            "past the live count"
        );
    }

    fn compute_at(panel_top: f32, ui: f32) -> LayoutModel {
        let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
        LayoutModel::compute(
            vec2(1280.0, 800.0),
            ui,
            panel_top,
            1280.0,
            zero,
            zero,
            zero,
            zero,
            zero,
            zero,
            [(zero, CardAction::None); 8],
            0,
            [(zero, CardAction::None); 16],
            0,
            [(zero, CardAction::None); 8],
            0,
        )
    }

    #[test]
    fn the_panel_band_owns_exactly_below_its_top() {
        let m = compute_at(700.0, 1.0);
        assert!(m.chrome_owns(vec2(600.0, 770.0)));
        assert!(m.chrome_owns(vec2(600.0, 700.0)));
        assert!(
            !m.chrome_owns(vec2(600.0, 699.0)),
            "the band must not swallow the midfield"
        );
    }

    #[test]
    fn the_band_owns_its_width_and_the_dock_its_rect() {
        let mut m = compute_at(700.0, 1.0);
        m.panel_right = 600.0;
        m.panel_regions[0].w = 600.0;
        m.orders = Rect::new(0.0, 500.0, 60.0, 200.0);
        assert!(m.chrome_owns(vec2(400.0, 750.0)), "inside the band");
        assert!(
            !m.chrome_owns(vec2(900.0, 750.0)),
            "right of a content-width band is world, not chrome"
        );
        assert!(
            m.chrome_owns(vec2(30.0, 600.0)),
            "the orders dock is chrome"
        );
        assert!(
            !m.chrome_owns(vec2(100.0, 600.0)),
            "beside the dock stays world"
        );
    }

    #[test]
    fn no_panel_means_no_band_at_all() {
        let m = compute_at(f32::INFINITY, 2.0);
        assert!(!m.chrome_owns(vec2(600.0, 799.0)));
        assert!(m.chrome_owns(vec2(600.0, 30.0)), "the top bar always owns");
    }

    #[test]
    fn an_armed_mode_ribbon_owns_its_world_pixels() {
        let mut m = compute_at(f32::INFINITY, 1.0);
        m.mode_ribbon = Rect::new(220.0, 640.0, 280.0, MIN_TOUCH_TARGET);
        m.queue_toggle = Rect::new(116.0, 640.0, 96.0, MIN_TOUCH_TARGET);
        assert!(m.chrome_owns(m.mode_ribbon.center()));
        assert!(
            m.chrome_owns(m.queue_toggle.center()),
            "QUEUE is chrome too"
        );
        assert!(
            !m.chrome_owns(vec2(219.0, 660.0)),
            "beside the ribbon remains battlefield"
        );
    }

    #[test]
    fn the_menu_button_sits_in_the_top_bar_with_a_full_touch_target() {
        for ui in [0.75, 1.0, 1.25, 1.5] {
            for width in [640.0, 1133.0, 1280.0, 1920.0] {
                for touch_only in [false, true] {
                    let button = menu_button_rect(width, ui, touch_only);
                    assert!(
                        button.y >= 0.0 && button.y + button.h <= TOP_BAR_H * ui,
                        "the drawn button stays inside the bar at {width}px, ui {ui}"
                    );
                    let pad = touch_pad(button, ui);
                    assert!(pad.w >= MIN_TOUCH_TARGET * ui && pad.h >= MIN_TOUCH_TARGET * ui);
                    assert!(
                        pad.x + pad.w <= width,
                        "the fingertip target stays on screen at {width}px, ui {ui}"
                    );
                    if touch_only {
                        assert!(
                            button.x + button.w <= width - TOUCH_RIGHT_INSET * ui,
                            "clear of a rounded screen corner"
                        );
                    }
                }
            }
        }
    }

    /// Typical desktop widths at 1x: a four-digit bank, a two-digit
    /// income, and a PAUSED status.
    fn sample_text(ui: f32) -> TopBarText {
        TopBarText {
            scrap: 44.0 * ui,
            passive: 120.0 * ui,
            units_label: 38.0 * ui,
            units: 24.0 * ui,
            idle: Some(70.0 * ui),
            alert: None,
            status: 76.0 * ui,
        }
    }

    #[test]
    fn the_desktop_top_bar_keeps_its_legacy_positions() {
        let bar = top_bar(1280.0, 1.0, false, sample_text(1.0));
        assert_eq!((bar.scrap_label_x, bar.scrap_x), (12.0, 70.0));
        assert_eq!(bar.passive_x, 151.0, "a short bank keeps the minimum");
        assert_eq!(
            bar.units_x,
            151.0 + 120.0 + 16.0,
            "a long income pushes the count"
        );
        assert_eq!(bar.count_x, bar.units_x + 60.0);
        assert_eq!(
            bar.idle_badge,
            Rect::new(bar.count_x + 45.0, 3.0, 88.0, 34.0)
        );
        assert_eq!(bar.menu_button, Rect::new(1238.0, 3.0, 34.0, 34.0));
        assert_eq!(bar.status_x, 1238.0 - 12.0 - 76.0);
        assert_eq!(
            bar.pause_status,
            Rect::new(bar.status_x - 6.0, 3.0, 88.0, 34.0)
        );
        assert_eq!(
            bar.status_space,
            (bar.idle_badge.x + bar.idle_badge.w, bar.status_x)
        );
        let touch = top_bar(1280.0, 1.0, true, sample_text(1.0));
        assert_eq!(
            touch.scrap_label_x, 18.0,
            "touch holds the bank off the corner"
        );
        assert_eq!(touch.menu_button.x, 1280.0 - 34.0 - 20.0);
        let no_idle = top_bar(
            1280.0,
            1.0,
            false,
            TopBarText {
                idle: None,
                ..sample_text(1.0)
            },
        );
        assert_eq!(no_idle.idle_badge, Rect::new(0.0, 0.0, 0.0, 0.0));
        assert_eq!(no_idle.status_space.0, no_idle.count_x + 24.0);
    }

    #[test]
    fn the_group_column_stacks_above_the_minimap_and_wraps_when_short() {
        for (viewport, touch_only) in [
            (vec2(1280.0, 800.0), false),
            (vec2(1194.0, 834.0), true),
            (vec2(1133.0, 744.0), true),
        ] {
            let minimap = crate::render::minimap_rect_scaled(64, 40, viewport, 1.0);
            let column = group_column(1.0, touch_only, minimap).expect("room above the minimap");
            assert!(
                column.slots.iter().all(|slot| slot.x == column.slots[0].x),
                "{viewport}: one column"
            );
            assert!(
                column.slots.windows(2).all(|pair| pair[0].y < pair[1].y),
                "group 1 on top"
            );
            assert_eq!(column.plate.x + column.plate.w, minimap.x + minimap.w);
            assert!(
                column.plate.y + column.plate.h < minimap.y,
                "above the minimap"
            );
            assert!(column.plate.y >= TOP_BAR_H, "below the top bar");
            for slot in column.slots {
                assert!(column.plate.contains(slot.center()));
                if touch_only {
                    assert!(slot.w >= MIN_TOUCH_TARGET && slot.h >= MIN_TOUCH_TARGET);
                }
            }
        }
        let small = vec2(640.0, 400.0);
        let minimap = crate::render::minimap_rect_scaled(64, 40, small, 1.0);
        let column = group_column(1.0, true, minimap).expect("wrapped");
        assert!(
            column.slots.iter().any(|slot| slot.x != column.slots[0].x),
            "wraps"
        );
        assert!(column.plate.y >= TOP_BAR_H);
        assert!(column.plate.x >= 0.0);
        assert!(
            group_column(1.0, true, Rect::new(0.0, 0.0, 0.0, 0.0)).is_none(),
            "no minimap, no column"
        );
    }

    #[test]
    fn the_alert_badge_follows_the_idle_badge_or_takes_its_place() {
        let text = TopBarText {
            alert: Some(90.0),
            ..sample_text(1.0)
        };
        let bar = top_bar(1280.0, 1.0, false, text);
        assert_eq!(bar.alert_badge.x, bar.idle_badge.x + bar.idle_badge.w + 8.0);
        assert_eq!(bar.alert_badge.w, 108.0);
        let alone = top_bar(1280.0, 1.0, false, TopBarText { idle: None, ..text });
        assert_eq!(alone.alert_badge.x, bar.idle_badge.x, "no idle, same slot");
        let quiet = top_bar(1280.0, 1.0, false, sample_text(1.0));
        assert_eq!(quiet.alert_badge, Rect::new(0.0, 0.0, 0.0, 0.0));
        assert_eq!(
            quiet.idle_badge, bar.idle_badge,
            "an alert moves nothing else"
        );
    }

    #[test]
    fn the_top_bar_keeps_its_groups_in_order_and_on_screen() {
        for ui in [0.75, 1.0, 1.25, 1.5] {
            for width in [640.0, 1024.0, 1133.0, 1194.0, 1280.0, 1920.0] {
                for touch_only in [false, true] {
                    let bar = top_bar(width, ui, touch_only, sample_text(ui));
                    assert!(bar.scrap_label_x < bar.scrap_x);
                    assert!(bar.scrap_x < bar.passive_x && bar.passive_x < bar.units_x);
                    assert!(bar.units_x < bar.count_x && bar.count_x < bar.idle_badge.x);
                    assert!(bar.pause_status.x + bar.pause_status.w <= bar.menu_button.x);
                    let pad = touch_pad(bar.menu_button, ui);
                    assert!(pad.x + pad.w <= width, "{width}px @{ui} touch {touch_only}");
                }
            }
        }
    }

    const VIEW: Vec2 = Vec2::new(1280.0, 800.0);

    #[test]
    fn a_dock_tooltip_tracks_the_chip_it_describes() {
        // Two chips of a full dock, hundreds of px apart: each tooltip
        // centers on ITS chip. Pinning to the band drew both beside the
        // bottom one.
        let size = Vec2::new(240.0, 90.0);
        let top = Rect::new(8.0, 300.0, 44.0, 44.0);
        let bottom = Rect::new(8.0, 620.0, 44.0, 44.0);
        let a = tooltip_origin(top, size, TooltipSide::RightOf, VIEW, 32.0, 6.0);
        let b = tooltip_origin(bottom, size, TooltipSide::RightOf, VIEW, 32.0, 6.0);
        assert_eq!(a.x, 58.0, "clear of the dock, not under the pointer");
        assert_eq!(a.y + size.y * 0.5, top.y + top.h * 0.5);
        assert_eq!(b.y + size.y * 0.5, bottom.y + bottom.h * 0.5);
    }

    #[test]
    fn a_tall_tooltip_stops_at_the_top_bar() {
        // A five-line tooltip raised from a chip near the top of a
        // short window: the header must stay readable, so the box pins
        // below the bar instead of going negative.
        let chip = Rect::new(8.0, 90.0, 44.0, 44.0);
        let size = Vec2::new(240.0, 200.0);
        let o = tooltip_origin(
            chip,
            size,
            TooltipSide::RightOf,
            Vec2::new(1024.0, 400.0),
            32.0,
            6.0,
        );
        assert_eq!(o.y, 38.0, "top bar + gap");
        // Taller than the window has room for: still the top, never a
        // max that fell below the min.
        let o = tooltip_origin(
            chip,
            Vec2::new(240.0, 900.0),
            TooltipSide::RightOf,
            Vec2::new(1024.0, 400.0),
            32.0,
            6.0,
        );
        assert_eq!(o.y, 38.0);
    }

    #[test]
    fn a_wide_tooltip_slides_back_inside_the_window() {
        let card = Rect::new(1150.0, 700.0, 66.0, 80.0);
        let size = Vec2::new(300.0, 60.0);
        let o = tooltip_origin(card, size, TooltipSide::Above, VIEW, 32.0, 6.0);
        assert_eq!(o.x, 974.0, "1280 - 300 - 6");
        assert_eq!(o.y, 634.0, "above the card, not above the band");
        // Wider than the window: pinned left, never a negative origin.
        let o = tooltip_origin(
            card,
            Vec2::new(2000.0, 60.0),
            TooltipSide::Above,
            VIEW,
            32.0,
            6.0,
        );
        assert_eq!(o.x, 0.0);
    }
}
