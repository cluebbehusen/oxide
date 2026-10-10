//! The command band, the orders dock, and the hover tooltip: the
//! selection panel's drawn form. Geometry it publishes goes into the
//! `LayoutModel`; the pure card model lives in `crate::panel`.

use super::*;
use crate::numeric;
use crate::numeric::Fit;
use crate::render::prim::{fill_rect, stroke_rect};

use super::panel_layout::{InfoLayout, PanelGeometry, measure_info};

const CARD_RIGHT_INSET: f32 = 12.0;
const CARD_LEFT_INSET: f32 = 8.0;
const RALLY_ICON_SIZE: f32 = 24.0;
const RALLY_CONTEXT_WIDTH: f32 = RALLY_ICON_SIZE + 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct PanelPacking {
    right: f32,
    available: f32,
    per_row: usize,
    rally_count: usize,
    band_h: f32,
    top: f32,
    hides_minimap: bool,
}

fn card_metrics(viewport: Vec2, scale: f32) -> (f32, f32, f32, f32) {
    let small = viewport.x / scale < 800.0 || viewport.y / scale < 500.0;
    let left = if small { 204.0 } else { 228.0 };
    let (width, height) = if small { (66.0, 76.0) } else { (116.0, 48.0) };
    (left * scale, width * scale, height * scale, 6.0 * scale)
}

fn rally_group_width(card_width: f32, scale: f32) -> f32 {
    RALLY_CONTEXT_WIDTH * scale
        + card_width.max((2.0 * crate::theme::MIN_TOUCH_TARGET + 6.0) * scale)
}

fn panel_packing_at_right(
    viewport: Vec2,
    scale: f32,
    right: f32,
    cards_shown: usize,
    rally_count: usize,
    hides_minimap: bool,
) -> PanelPacking {
    let (cards_x, card_w, card_h, gap) = card_metrics(viewport, scale);
    let first_width = if rally_count > 0 {
        rally_group_width(card_w, scale)
    } else {
        card_w
    };
    let available =
        (right - cards_x - (CARD_LEFT_INSET + CARD_RIGHT_INSET) * scale).max(first_width);
    let per_row = 1 + numeric::to_usize(((available - first_width) / (card_w + gap)).floor());
    let cards_h = if cards_shown == 0 {
        0.0
    } else {
        grouped_card_rows(cards_shown, rally_count, per_row) as f32 * (card_h + 4.0 * scale)
    };
    let band_h = (16.0 * scale + cards_h).max(72.0 * scale);
    PanelPacking {
        right,
        available,
        per_row,
        rally_count,
        band_h,
        top: viewport.y - band_h,
        hides_minimap,
    }
}

fn panel_packing(
    viewport: Vec2,
    minimap: Rect,
    scale: f32,
    cards_len: usize,
    rally_count: usize,
) -> PanelPacking {
    let cards_shown = cards_len.min(16);
    let reserved_right = if minimap.w > 0.0 {
        (minimap.x - 8.0 * scale).max(300.0 * scale).min(viewport.x)
    } else {
        viewport.x
    };
    let max_band_h = (viewport.y - crate::layout::TOP_BAR_H * scale).max(0.0);
    let packing = panel_packing_at_right(
        viewport,
        scale,
        reserved_right,
        cards_shown,
        rally_count,
        false,
    );
    if packing.band_h > max_band_h && reserved_right < viewport.x {
        panel_packing_at_right(viewport, scale, viewport.x, cards_shown, rally_count, true)
    } else {
        packing
    }
}

fn action_panel_rect(packing: PanelPacking, left: f32, right: f32, has_cards: bool) -> Rect {
    if has_cards {
        Rect::new(left, packing.top, right - left, packing.band_h)
    } else {
        Rect::new(0.0, 0.0, 0.0, 0.0)
    }
}

fn grouped_card_slot(index: usize, rally_count: usize, per_row: usize) -> (usize, usize, bool) {
    let per_row = per_row.max(1);
    if rally_count == 0 {
        return (index / per_row, index % per_row, false);
    }
    if index < rally_count {
        return (0, 0, false);
    }
    let production_index = index - rally_count;
    if per_row == 1 {
        return (1 + production_index, 0, false);
    }
    let production_columns = per_row - 1;
    (
        production_index / production_columns,
        1 + production_index % production_columns,
        true,
    )
}

fn grouped_card_rows(shown: usize, rally_count: usize, per_row: usize) -> usize {
    if rally_count == 0 {
        shown.div_ceil(per_row.max(1)).max(1)
    } else if per_row <= 1 {
        1 + shown - rally_count
    } else {
        (shown - rally_count).div_ceil(per_row - 1).max(1)
    }
}

fn rally_card_count(cards: &[crate::panel::Card]) -> usize {
    use crate::panel::CardAction;
    cards
        .iter()
        .take(16)
        .take_while(|card| matches!(card.action, CardAction::ArmRally | CardAction::ClearRally))
        .count()
}

fn grouped_card_gap(
    shown: usize,
    rally_count: usize,
    per_row: usize,
    available: f32,
    card_width: f32,
    ordinary_gap: f32,
) -> f32 {
    let per_row = per_row.max(1);
    if rally_count == 0 || rally_count >= shown || per_row == 1 {
        return 0.0;
    }
    let occupied_columns = 1 + (shown - rally_count).min(per_row - 1);
    let ordinary_width = occupied_columns as f32 * card_width
        + occupied_columns.saturating_sub(1) as f32 * ordinary_gap;
    (available - ordinary_width).clamp(0.0, 10.0 * card_width / 66.0)
}

fn command_card_geometry(
    viewport: Vec2,
    scale: f32,
    packing: PanelPacking,
    cards: &[crate::panel::Card],
) -> (Vec<Rect>, f32) {
    let (left, width, height, gap) = card_metrics(viewport, scale);
    let shown = cards.len().min(16);
    let rally_count = packing.rally_count;
    let section_gap = grouped_card_gap(
        shown,
        rally_count,
        packing.per_row,
        packing.available - (rally_group_width(width, scale) - width),
        width,
        gap,
    );
    let slots: Vec<Rect> = (0..shown)
        .map(|index| {
            let (row, column, after_rally) = grouped_card_slot(index, rally_count, packing.per_row);
            let group_width = rally_group_width(width, scale);
            let rally_width = ((group_width
                - RALLY_CONTEXT_WIDTH * scale
                - gap * rally_count.saturating_sub(1) as f32)
                / rally_count.max(1) as f32)
                .max(crate::theme::MIN_TOUCH_TARGET * scale);
            let rally = index < rally_count;
            Rect::new(
                left + CARD_LEFT_INSET * scale
                    + column as f32 * (width + gap)
                    + if after_rally {
                        section_gap + group_width - width
                    } else {
                        0.0
                    }
                    + if rally {
                        RALLY_CONTEXT_WIDTH * scale + index as f32 * (rally_width + gap)
                    } else {
                        0.0
                    },
                packing.top + 10.0 * scale + row as f32 * (height + 4.0 * scale),
                if rally { rally_width } else { width },
                height,
            )
        })
        .collect();
    let band_width = if shown == 0 {
        left
    } else {
        slots
            .iter()
            .map(|rect| rect.x + rect.w)
            .fold(left, f32::max)
            + CARD_RIGHT_INSET * scale
    };
    (slots, band_width.min(packing.right))
}

fn rally_context_rect(first_button: Rect, scale: f32) -> Rect {
    let size = RALLY_ICON_SIZE * scale;
    Rect::new(
        first_button.x - RALLY_CONTEXT_WIDTH * scale,
        first_button.y + (first_button.h - size) * 0.5,
        size,
        size,
    )
}

fn draw_rally_control(card: &crate::panel::Card, rect: Rect, scale: f32) {
    let label = match card.action {
        crate::panel::CardAction::ClearRally => "Clear",
        _ if card.title.starts_with("Reset") => "Reset",
        _ => "Set",
    };
    let color = if card.enabled {
        TEXT_PRIMARY
    } else {
        TEXT_DISABLED
    };
    for (text, size, baseline, tint) in [
        (label, 14.0, rect.y + rect.h * 0.5 - scale, color),
        (
            card.hotkey.as_str(),
            11.0,
            rect.y + rect.h * 0.5 + 15.0 * scale,
            if card.enabled {
                TEXT_SECONDARY
            } else {
                TEXT_DISABLED
            },
        ),
    ] {
        let available = rect.w - 6.0 * scale;
        let mut font_size = size * scale;
        let mut measured = measure_text(text, None, numeric::font_size(font_size), 1.0);
        while measured.width > available && font_size > 8.0 * scale {
            font_size -= 0.5 * scale;
            measured = measure_text(text, None, numeric::font_size(font_size), 1.0);
        }
        if measured.width <= available {
            draw_text(
                text,
                rect.x + (rect.w - measured.width) * 0.5,
                baseline,
                font_size,
                tint,
            );
        }
    }
}

fn card_title_lines(title: &str, measure: impl Fn(&str) -> f32, width: f32) -> Vec<String> {
    if measure(title) > width && title.contains('-') {
        wrap_words(&title.replace('-', "- "), measure, width)
    } else {
        wrap_words(title, measure, width)
    }
}

/// Packs every visible queue chip above the command band: a single
/// column when it fits, otherwise a grid (a full eight-slot production
/// queue becomes 2×4 in a 640×400 window), so no paid, cancelable job
/// hides behind an overflow label.
fn queue_grid(
    queue_len: usize,
    panel_top: f32,
    scale: f32,
    header: f32,
) -> (Rect, [Rect; 8], usize) {
    queue_grid_with_width(queue_len, panel_top, scale, 44.0, header)
}

/// The Stop button's own plate above the dock: the button and its inset.
const STOP_PLATE: f32 = 60.0;
/// The gap between the Stop plate and the dock beneath it.
const STOP_GAP: f32 = 6.0;

/// `header` reserves logical px at the dock's top, above its label.
fn queue_grid_with_width(
    queue_len: usize,
    panel_top: f32,
    scale: f32,
    width: f32,
    header: f32,
) -> (Rect, [Rect; 8], usize) {
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut slots = [zero; 8];
    let count = queue_len.min(slots.len());
    if count == 0 {
        return (zero, slots, 0);
    }
    let (size, gap) = (44.0 * scale, 4.0 * scale);
    let label_h = 24.0 * scale;
    let header = header * scale;
    let available =
        (panel_top - crate::layout::TOP_BAR_H * scale - header - label_h - 2.0 * scale).max(size);
    let max_rows = numeric::to_usize(((available + gap) / (size + gap)).floor()).max(1);
    let columns = count.div_ceil(max_rows).max(1);
    let rows = count.div_ceil(columns);
    let slot_width = width * scale;
    let width = 16.0 * scale + columns as f32 * slot_width + columns.saturating_sub(1) as f32 * gap;
    let height =
        header + label_h + rows as f32 * size + rows.saturating_sub(1) as f32 * gap + 2.0 * scale;
    let dock = Rect::new(0.0, panel_top - height, width, height);
    for (index, slot) in slots.iter_mut().take(count).enumerate() {
        let row = index / columns;
        let column = index % columns;
        *slot = Rect::new(
            8.0 * scale + column as f32 * (slot_width + gap),
            dock.y + header + label_h + row as f32 * (size + gap),
            slot_width,
            size,
        );
    }
    (dock, slots, count)
}

fn collective_queue_grid(
    count: usize,
    top: f32,
    scale: f32,
    viewport_width: f32,
    header: f32,
) -> (Rect, [Rect; 8], usize) {
    if viewport_width / scale < 800.0 {
        return queue_grid_with_width(count, top, scale, 64.0, header);
    }
    let wide = queue_grid_with_width(count, top, scale, 170.0, header);
    if wide.0.right() <= viewport_width {
        wide
    } else {
        queue_grid_with_width(count, top, scale, 64.0, header)
    }
}

fn queue_label_width(panel: &crate::panel::Panel, measure: impl Fn(&str) -> f32) -> f32 {
    use crate::panel::{CardAction, CardIcon};

    if !panel.queue_groups.is_empty() {
        return measure(&panel.queue_label);
    }
    let ticks = panel
        .queue
        .iter()
        .filter_map(|card| match (card.action, card.icon) {
            (CardAction::CancelQueue(..), CardIcon::Unit(kind)) => Some(kind.stats().train_ticks),
            _ => None,
        });
    if ticks.clone().next().is_none() {
        return measure(&panel.queue_label);
    }

    // Reserve from complete jobs, not their changing progress. Digit widths
    // can differ, and a completed head may stay queued while its exit is blocked.
    let digit_width = (0..=9)
        .map(|digit| measure(&digit.to_string()))
        .fold(0.0_f32, f32::max);
    let time_width = |ticks: u32| {
        let seconds = ticks.div_ceil(oxide_sim::TICKS_PER_SECOND).max(1);
        let digits = seconds.ilog10() + 1;
        (digits + 1) as f32 * digit_width + measure(".s")
    };
    time_width(ticks.sum()).max(measure("Ready"))
}

fn catalog_geometry(
    viewport: Vec2,
    scale: f32,
    minimap: Rect,
    count: usize,
) -> (Rect, Vec<Rect>, bool) {
    let left = 10.0 * scale;
    let right = if minimap.w > 0.0 {
        minimap.x - 8.0 * scale
    } else {
        viewport.x
    };
    let available = (right - left - 10.0 * scale).max(136.0 * scale);
    let max_columns = numeric::to_usize(
        ((available + 4.0 * scale) / (116.0 * scale))
            .floor()
            .max(1.0),
    );
    let rows = count.max(1).div_ceil(max_columns);
    // Thirteen buildings in their four categories plus Back, which
    // takes the free cell under UTILITY.
    let grouped = count == 14 && max_columns >= 7;
    let columns = if grouped {
        7
    } else {
        count.max(1).div_ceil(rows)
    };
    let width = ((available - columns.saturating_sub(1) as f32 * 4.0 * scale) / columns as f32)
        .min(140.0 * scale);
    let row_h = 58.0 * scale;
    let header = 28.0 * scale;
    let height = header + count.div_ceil(columns) as f32 * row_h + 8.0 * scale;
    let band = Rect::new(
        0.0,
        viewport.y - height,
        left + columns as f32 * (width + 4.0 * scale) + 6.0 * scale,
        height,
    );
    let slots = (0..count)
        .map(|i| {
            let (column, row) = if grouped {
                match i {
                    0..=1 => (0, i),
                    2..=5 => (1 + (i - 2) % 2, (i - 2) / 2),
                    6..=9 => (3 + (i - 6) % 2, (i - 6) / 2),
                    _ => (5 + (i - 10) % 2, (i - 10) / 2),
                }
            } else {
                (i % columns, i / columns)
            };
            Rect::new(
                left + column as f32 * (width + 4.0 * scale),
                band.y + header + row as f32 * row_h,
                width,
                row_h - 4.0 * scale,
            )
        })
        .collect();
    (band, slots, grouped)
}

/// A construction category's header: its name, a marker while open,
/// and on desktop the keys that reach it.
fn category_label(
    label: &str,
    open: bool,
    key: &str,
    palette_key: Option<&str>,
    keyless: bool,
) -> String {
    if open {
        format!("{label} *")
    } else if keyless {
        label.to_string()
    } else if let Some(palette_key) = palette_key {
        format!("{label} [{palette_key} > {key}]")
    } else {
        format!("{label} [{key}]")
    }
}

/// What pressing a drawn card does: its own action while enabled, an
/// explanation while disabled.
fn published_action(card: &crate::panel::Card) -> crate::panel::CardAction {
    if card.enabled {
        card.action
    } else {
        crate::panel::CardAction::Refused
    }
}

fn draw_catalog(
    panel: &crate::panel::Panel,
    input: &InputState,
    bindings: &crate::action::BindingMap,
    (band, slots, grouped): (Rect, &[Rect], bool),
    draw_icon: &impl Fn(Rect, &crate::panel::CardIcon, Color),
) {
    use crate::panel::CardAction;
    let s = ui_scale();
    fill_rect(band, crate::theme::SURFACE_BAND);
    draw_rectangle(band.x, band.y, band.w, s, crate::theme::EDGE_WARM);
    if !grouped {
        crate::typography::draw(
            "BUILD",
            12.0 * s,
            band.y + 20.0 * s,
            crate::theme::Type::Body.at(s),
            TEXT_PRIMARY,
        );
    }
    if grouped {
        for (category, (index, label)) in [
            (0, "ECONOMY"),
            (2, "PRODUCTION"),
            (6, "DEFENSE"),
            (10, "UTILITY"),
        ]
        .into_iter()
        .enumerate()
        {
            let key = bindings.label(crate::action::Action::BuildCategory(category.fit::<u8>()));
            let palette_key = bindings.label(crate::action::Action::ToggleBuildPalette);
            let label = category_label(
                label,
                input.build_category == Some(category.fit::<u8>()),
                &key,
                input
                    .build_category
                    .is_some()
                    .then_some(palette_key.as_str()),
                !crate::platform::hands().keys,
            );
            crate::typography::draw(
                &label,
                slots[index].x + 5.0 * s,
                band.y + 20.0 * s,
                crate::theme::Type::Caption.at(s),
                TEXT_SECONDARY,
            );
        }
    }
    let hovered = slots.iter().position(|rect| hovers(input, *rect));
    for (i, (card, &rect)) in panel.cards.iter().zip(slots).enumerate() {
        let armed =
            matches!(card.action, CardAction::ArmBuild(kind) if input.placing == Some(kind));
        let hot = hovered == Some(i);
        fill_rect(
            rect,
            if armed {
                crate::theme::CARD_ARMED
            } else if hot {
                crate::theme::CARD_HOVER
            } else {
                crate::theme::CARD_IDLE
            },
        );
        if armed || hot {
            draw_rectangle(
                rect.x,
                rect.y,
                2.0 * s,
                rect.h,
                if armed { SCRAP_COLOR } else { TEXT_PRIMARY },
            );
        }
        draw_icon(
            Rect::new(rect.x + 5.0 * s, rect.y + 9.0 * s, 34.0 * s, 34.0 * s),
            &card.icon,
            if card.enabled {
                WHITE
            } else {
                Color::new(1.0, 1.0, 1.0, 0.5)
            },
        );
        let names = card_title_lines(
            &card.title,
            |text| {
                measure_text(
                    text,
                    None,
                    numeric::font_size(crate::theme::Type::Body.at(s)),
                    1.0,
                )
                .width
            },
            rect.w - 49.0 * s,
        );
        for (line, name) in names.iter().enumerate() {
            draw_text(
                name,
                rect.x + 44.0 * s,
                rect.y + (if names.len() > 1 { 16.0 } else { 22.0 } + line as f32 * 14.0) * s,
                crate::theme::Type::Body.at(s),
                if card.enabled {
                    TEXT_PRIMARY
                } else {
                    TEXT_SECONDARY
                },
            );
        }
        if let Some(cost) = card.cost {
            crate::typography::draw(
                &cost.to_string(),
                rect.x + 44.0 * s,
                rect.y + 42.0 * s,
                crate::theme::Type::Small.at(s),
                if card.enabled {
                    SCRAP_COLOR
                } else {
                    TEXT_DISABLED
                },
            );
        }
        let key_width = measure_text(
            &card.hotkey,
            None,
            numeric::font_size(crate::theme::Type::Caption.at(s)),
            1.0,
        )
        .width;
        draw_text(
            &card.hotkey,
            rect.x + rect.w - key_width - 5.0 * s,
            rect.y + 42.0 * s,
            crate::theme::Type::Caption.at(s),
            TEXT_SECONDARY,
        );
    }
}

fn selection_info_rect(viewport: Vec2, width: f32, content_height: f32, actions: Rect) -> Rect {
    let height = content_height.max(actions.h);
    Rect::new(0.0, viewport.y - height, width, height)
}

/// Where the command panel's pieces sit this frame, laid out without
/// drawing so input hit-tests exactly what the next draw shows.
pub(crate) enum PanelLayout {
    /// The construction palette: its band, one slot per card, and
    /// whether the slots stand in their four category columns.
    Catalog {
        band: Rect,
        slots: Vec<Rect>,
        grouped: bool,
    },
    /// A selection's information region, command cards, roster and
    /// orders dock.
    Selection(Box<SelectionLayout>),
}

pub(crate) struct SelectionLayout {
    small: bool,
    cards_x: f32,
    measured: InfoLayout,
    info_rect: Rect,
    action_rect: Rect,
    card_rects: Vec<Rect>,
    rally_count: usize,
    roster: Vec<Rect>,
    dock: Option<DockLayout>,
    hides_minimap: bool,
}

/// The orders dock: its plate, the list beneath the Stop plate, the Stop
/// plate and button, the visible chips, and how many more are hidden.
struct DockLayout {
    dock: Rect,
    list: Rect,
    stop: Option<(Rect, Rect)>,
    chips: Vec<Rect>,
    hidden: usize,
}

/// Whether the panel is the construction palette rather than a selection.
fn is_catalog(panel: &crate::panel::Panel) -> bool {
    use crate::panel::CardAction;
    panel
        .cards
        .iter()
        .any(|card| matches!(card.action, CardAction::ArmBuild(_)))
        && panel.cards.iter().all(|card| {
            matches!(
                card.action,
                CardAction::ArmBuild(_) | CardAction::ClosePalette
            )
        })
}

/// Lays out the command panel for `panel` in `env`'s window.
pub(crate) fn layout_panel(
    game: &crate::game::Scene<'_>,
    input: &InputState,
    panel: &crate::panel::Panel,
    env: super::hud::HudEnv,
    measure: super::hud::Measure<'_>,
) -> PanelLayout {
    let (viewport, s) = (env.viewport, env.ui);
    let mini = minimap_rect_scaled(
        game.state.map().width(),
        game.state.map().height(),
        viewport,
        s,
    );
    if is_catalog(panel) {
        let (band, slots, grouped) = catalog_geometry(viewport, s, mini, panel.cards.len());
        return PanelLayout::Catalog {
            band,
            slots,
            grouped,
        };
    }
    let small = viewport.x / s < 800.0 || viewport.y / s < 500.0;
    let packing = panel_packing(
        viewport,
        mini,
        s,
        panel.cards.len(),
        rally_card_count(&panel.cards),
    );
    let (cards_x, _, _, _) = card_metrics(viewport, s);
    let measured = measure_info(panel, cards_x, s, small, measure);
    let (card_rects, cards_w) = command_card_geometry(viewport, s, packing, &panel.cards);
    let action_rect = action_panel_rect(packing, cards_x, cards_w, !panel.cards.is_empty());
    let info_rect = selection_info_rect(viewport, cards_x, measured.height, action_rect);
    let top = info_rect.y;
    let (size, gap) = (measured.roster_size, 4.0 * s);
    let roster = (0..panel.roster.len().min(8))
        .map(|index| {
            let row = index / measured.roster_columns;
            let column = index % measured.roster_columns;
            Rect::new(
                8.0 * s + column as f32 * (size + gap),
                top + measured.roster_y + row as f32 * (size + gap),
                size,
                size,
            )
        })
        .collect();
    let dock = (!panel.queue.is_empty() || panel.stop.is_some())
        .then(|| layout_dock(game, input, panel, viewport, s, top, measure));
    PanelLayout::Selection(Box::new(SelectionLayout {
        small,
        cards_x,
        measured,
        info_rect,
        action_rect,
        card_rects,
        rally_count: packing.rally_count,
        roster,
        dock,
        hides_minimap: packing.hides_minimap,
    }))
}

/// Production ghosts or order chips, stacked above the band's left
/// corner so the band itself stays short.
fn layout_dock(
    game: &crate::game::Scene<'_>,
    input: &InputState,
    panel: &crate::panel::Panel,
    viewport: Vec2,
    s: f32,
    top: f32,
    measure: super::hud::Measure<'_>,
) -> DockLayout {
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let toggle_below = super::chrome::queue_toggle_shown(game, input, crate::platform::TOUCH_ONLY);
    let floor = if toggle_below {
        top - super::chrome::queue_dock_lift(s)
    } else {
        top
    };
    let header = if panel.stop.is_some() {
        STOP_PLATE + STOP_GAP
    } else {
        0.0
    };
    let (mut grid_dock, grid_slots, n) = if panel.queue.is_empty() {
        // A lone Stop plate: some other selected unit is busy.
        let side = STOP_PLATE * s;
        (Rect::new(0.0, floor - side, side, side), [zero; 8], 0)
    } else if panel.queue_groups.is_empty() {
        queue_grid(panel.queue.len(), floor, s, header)
    } else {
        collective_queue_grid(panel.queue.len(), floor, s, viewport.x, header)
    };
    if !panel.queue.is_empty() {
        let queue_label_width = queue_label_width(panel, |text| {
            measure(
                super::hud::Face::Body,
                text,
                crate::theme::Type::Small.at(s),
            )
        }) + 16.0 * s;
        grid_dock.w = grid_dock.w.max(queue_label_width);
    }
    let mut dock = grid_dock;
    let hidden = panel.queue.len().saturating_sub(n);
    let more_h = if hidden > 0 { 16.0 * s } else { 0.0 };
    if more_h > 0.0 {
        dock.y -= more_h;
        dock.h += more_h;
    }
    // The dock borders only its open top and right, resting on the
    // band. With the QUEUE toggle beneath it, the plate runs down to
    // the band so the toggle sits inside one closed column.
    if toggle_below {
        dock.h = top - dock.y;
    }
    // The list rests on the band beneath the Stop plate and its gap.
    let list = Rect::new(dock.x, dock.y + header * s, dock.w, dock.h - header * s);
    // The Stop button is as wide as the chips beneath it.
    let stop = panel.stop.as_ref().map(|_| {
        let width = if n > 0 { grid_slots[0].w } else { 44.0 * s };
        (
            Rect::new(0.0, dock.y, width + 16.0 * s, STOP_PLATE * s),
            Rect::new(8.0 * s, dock.y + 8.0 * s, width, 44.0 * s),
        )
    });
    let chips = grid_slots
        .iter()
        .take(n)
        .map(|slot| Rect::new(slot.x, slot.y - more_h, slot.w, slot.h))
        .collect();
    DockLayout {
        dock,
        list,
        stop,
        chips,
        hidden,
    }
}

impl PanelLayout {
    /// The clickable regions this layout publishes for `panel`.
    pub(super) fn geometry(&self, panel: &crate::panel::Panel) -> PanelGeometry {
        let mut geometry = PanelGeometry::empty();
        match self {
            PanelLayout::Catalog { band, slots, .. } => {
                geometry.actions = *band;
                for (i, (card, rect)) in panel.cards.iter().zip(slots).enumerate() {
                    geometry.cards[i] = (*rect, published_action(card));
                }
                geometry.card_count = panel.cards.len();
            }
            PanelLayout::Selection(selection) => {
                geometry.info = selection.info_rect;
                geometry.actions = selection.action_rect;
                geometry.hides_minimap = selection.hides_minimap;
                for (card, rect) in panel.roster.iter().zip(&selection.roster) {
                    geometry.roster_slots[geometry.roster_count] = (*rect, card.action);
                    geometry.roster_count += 1;
                }
                for (card, rect) in panel.cards.iter().zip(&selection.card_rects) {
                    geometry.cards[geometry.card_count] = (*rect, published_action(card));
                    geometry.card_count += 1;
                }
                if let Some(dock) = &selection.dock {
                    geometry.orders = dock.dock;
                    for (card, rect) in panel.queue.iter().zip(&dock.chips) {
                        geometry.queue_slots[geometry.queue_count] = (*rect, card.action);
                        geometry.queue_count += 1;
                    }
                    if let (Some(card), Some((_, button))) = (&panel.stop, dock.stop) {
                        geometry.queue_stop = (button, card.action);
                    }
                }
            }
        }
        geometry
    }
}

/// Draws the command panel band laid out by [`layout_panel`].
#[expect(clippy::too_many_lines, reason = "draws the whole selection panel")]
pub(crate) fn draw_panel(
    sprites: &Sprites,
    input: &InputState,
    bindings: &crate::action::BindingMap,
    panel: &crate::panel::Panel,
    layout: &PanelLayout,
) {
    use crate::panel::{CardAction, CardIcon};
    let s = ui_scale();
    let blit = |dest: Rect, source: Rect, tint: Color| {
        sprites.draw_canvas(dest, &[(source, tint)]);
    };
    // Defense art is authored as a base plus a north-facing live mount.
    // Static cards compose the same silhouette without inventing aim.
    // Card art wears its owner's color on the same accent masks the world
    // tints, dimmed with the card.
    let shade = |owner: Color, tint: Color| {
        Color::new(owner.r * tint.r, owner.g * tint.g, owner.b * tint.b, tint.a)
    };
    let building_layers =
        |kind: oxide_sim::BuildingKind, tier: u8, mount: bool, tint: Color, owner: Color| {
            let mut layers = vec![
                (sprites.building_tiered(kind, tier), tint),
                (
                    sprites.building_tiered_accent(kind, tier),
                    shade(owner, tint),
                ),
            ];
            if mount && let Some(mount) = sprites.defense_mount(kind, tier) {
                layers.push((mount, tint));
                if let Some(accent) = sprites.defense_mount_accent(kind, tier) {
                    layers.push((accent, shade(owner, tint)));
                }
            }
            layers
        };
    let unit_layers = |kind: oxide_sim::UnitKind, tint: Color, owner: Color| {
        vec![
            (sprites.unit(kind), tint),
            (sprites.unit_accent(kind), shade(owner, tint)),
        ]
    };
    // An order chip is two composed draws: the subject's own silhouette
    // (translucent under a scaffold while its site is still rising) and
    // the verb as a corner badge on a dark plate, so the pictogram never
    // dissolves into the hull beneath it. Every other icon is one sprite.
    let draw_icon = |dest: Rect, icon: &CardIcon, tint: Color| {
        let CardIcon::Order {
            subject,
            verb,
            ghost,
        } = icon
        else {
            match icon {
                CardIcon::Unit(kind) => {
                    sprites.draw_portrait(dest, &unit_layers(*kind, tint, panel.owner));
                }
                CardIcon::Building(kind, tier) => {
                    sprites.draw_portrait(
                        dest,
                        &building_layers(*kind, *tier, true, tint, panel.owner),
                    );
                }
                CardIcon::Verb(v) => blit(dest, sprites.verb_icon(*v), tint),
                CardIcon::Salvage { wreck } => blit(
                    dest,
                    if *wreck {
                        sprites.wreck_pile()
                    } else {
                        sprites.scrap(
                            oxide_sim::stats::SCRAP_NODE_AMOUNT,
                            oxide_sim::stats::SCRAP_NODE_AMOUNT,
                        )
                    },
                    tint,
                ),
                CardIcon::Order { verb, .. } => blit(dest, sprites.verb_icon(*verb), tint),
            }
            return;
        };
        let hull = if *ghost {
            Color::new(tint.r, tint.g, tint.b, tint.a * 0.7)
        } else {
            tint
        };
        let mut layers = match subject {
            crate::panel::OrderSubject::Unit(kind, owner) => unit_layers(*kind, hull, *owner),
            crate::panel::OrderSubject::Building(kind, owner) => {
                building_layers(*kind, 0, !*ghost, hull, *owner)
            }
        };
        if *ghost {
            // Scaffold and subject share their authored canvas, including its margins.
            layers.push((
                sprites.scaffold(false),
                Color::new(tint.r, tint.g, tint.b, tint.a * 0.45),
            ));
            sprites.draw_canvas(dest, &layers);
        } else {
            sprites.draw_portrait(dest, &layers);
        }
        let badge = dest.w * 0.44;
        let plate = Rect::new(
            dest.x + dest.w - badge,
            dest.y + dest.h - badge,
            badge,
            badge,
        );
        let base = crate::theme::ICON_PLATE;
        fill_rect(
            plate,
            Color {
                a: base.a * tint.a,
                ..base
            },
        );
        blit(plate, sprites.verb_icon(*verb), tint);
    };

    let selection = match layout {
        PanelLayout::Catalog {
            band,
            slots,
            grouped,
        } => {
            draw_catalog(panel, input, bindings, (*band, slots, *grouped), &draw_icon);
            return;
        }
        PanelLayout::Selection(selection) => selection,
    };
    let SelectionLayout {
        small,
        cards_x,
        measured,
        info_rect,
        action_rect,
        card_rects,
        rally_count,
        roster,
        dock,
        hides_minimap: _,
    } = &**selection;
    let (small, cards_x, info_rect, action_rect) = (*small, *cards_x, *info_rect, *action_rect);
    let top = info_rect.y;

    for rect in [info_rect, action_rect] {
        if rect.w == 0.0 {
            continue;
        }
        fill_rect(rect, crate::theme::SURFACE_BAND);
        draw_rectangle(rect.x, rect.y, rect.w, s, crate::theme::EDGE_WARM);
    }
    draw_icon(
        Rect::new(10.0 * s, top + 6.0 * s, 32.0 * s, 32.0 * s),
        &panel.portrait,
        WHITE,
    );
    for (index, title) in measured.title.iter().enumerate() {
        crate::typography::draw(
            title,
            52.0 * s,
            top + (24.0 + index as f32 * 17.0) * s,
            crate::theme::Type::Body.at(s),
            TEXT_PRIMARY,
        );
    }
    for (index, status) in measured.status.iter().enumerate() {
        draw_text(
            status,
            12.0 * s,
            top + measured.health_y
                - (measured.status.len() - index - 1) as f32 * measured.line_h
                - 4.0 * s,
            measured.font,
            TEXT_SECONDARY,
        );
    }
    if let Some(crate::panel::info::Health { hp, max_hp, color }) = panel.info.health {
        let y = top + measured.health_y;
        let value = format!("{hp}/{max_hp}");
        draw_text(
            "Health",
            12.0 * s,
            y + 13.0 * s,
            measured.font,
            TEXT_SECONDARY,
        );
        draw_text(
            &value,
            cards_x
                - 12.0 * s
                - measure_text(&value, None, numeric::font_size(measured.font), 1.0).width,
            y + 13.0 * s,
            measured.font,
            TEXT_PRIMARY,
        );
        draw_rectangle(
            12.0 * s,
            y + 19.0 * s,
            cards_x - 24.0 * s,
            3.0 * s,
            crate::theme::HEALTH_TRACK,
        );
        draw_rectangle(
            12.0 * s,
            y + 19.0 * s,
            (cards_x - 24.0 * s) * (hp as f32 / max_hp.max(1) as f32).clamp(0.0, 1.0),
            3.0 * s,
            color,
        );
    }
    for line in &measured.lines {
        let y = top + line.offset + measured.font;
        if let Some(icon) = line.icon {
            match icon {
                crate::panel::info::StatIcon::Capability(icon) => draw_capability_icon(
                    vec2(18.0 * s, y - 5.0 * s),
                    6.0 * s,
                    icon,
                    capability_icon_color(icon),
                    false,
                ),
                crate::panel::info::StatIcon::Verb(icon) => draw_icon(
                    Rect::new(11.0 * s, y - 12.0 * s, 14.0 * s, 14.0 * s),
                    &CardIcon::Verb(icon),
                    TEXT_SECONDARY,
                ),
            }
        }
        draw_text(
            &line.label,
            32.0 * s,
            y,
            measured.font,
            if line.section {
                TEXT_PRIMARY
            } else {
                TEXT_SECONDARY
            },
        );
        let value_width =
            measure_text(&line.value, None, numeric::font_size(measured.font), 1.0).width;
        draw_text(
            &line.value,
            cards_x - 12.0 * s - value_width,
            y,
            measured.font,
            TEXT_PRIMARY,
        );
    }

    // Mixed-selection roster, visually and geometrically separate from
    // verbs. It reads as "what is in my hand" before "what can it do".
    if !roster.is_empty() {
        let label_y = top + measured.roster_y - 4.0 * s;
        draw_text(
            "SELECTED UNITS",
            12.0 * s,
            label_y,
            crate::theme::Type::Small.at(s),
            TEXT_SECONDARY,
        );
        for (card, &rect) in panel.roster.iter().zip(roster) {
            let hovered = hovers(input, rect);
            fill_rect(
                rect,
                if hovered {
                    crate::theme::CARD_HOVER
                } else {
                    crate::theme::CHIP
                },
            );
            stroke_rect(
                rect,
                if hovered {
                    crate::theme::Stroke::Focus
                } else {
                    crate::theme::Stroke::Edge
                }
                .at(s),
                if hovered {
                    crate::theme::EDGE_FOCUS
                } else {
                    crate::theme::EDGE_CHIP
                },
            );
            let icon_size = (if small { 28.0 } else { 42.0 }) * s;
            draw_icon(
                Rect::new(
                    rect.x + (rect.w - icon_size) * 0.5,
                    rect.y + 3.0 * s,
                    icon_size,
                    icon_size,
                ),
                &card.icon,
                WHITE,
            );
            let label = if small {
                card.title
                    .rsplit_once(" x")
                    .map_or(card.title.as_str(), |(_, count)| count)
            } else {
                &card.title
            };
            let mut size = crate::theme::Type::Caption.at(s);
            let mut dims = measure_text(label, None, numeric::font_size(size), 1.0);
            while dims.width > rect.w - 6.0 * s && size > 8.0 * s {
                size -= 0.5 * s;
                dims = measure_text(label, None, numeric::font_size(size), 1.0);
            }
            draw_text(
                label,
                rect.x + (rect.w - dims.width) * 0.5,
                rect.y + rect.h - 6.0 * s,
                size,
                TEXT_PRIMARY,
            );
        }
    }

    if *rally_count > 0 {
        draw_icon(
            rally_context_rect(card_rects[0], s),
            &CardIcon::Verb(crate::panel::VerbIcon::Rally),
            WHITE,
        );
    }

    // Command cards, wrapping into as many rows as the width demands.
    for (card, &rect) in panel.cards.iter().zip(card_rects) {
        let hovered = hovers(input, rect);
        let selected = matches!(card.action, CardAction::ArmBuild(kind) if input.placing == Some(kind))
            || (card.action == CardAction::ArmRally && !input.rallying.is_empty());
        let bg = if selected {
            crate::theme::CARD_ARMED
        } else if hovered && card.enabled {
            crate::theme::CARD_HOVER
        } else {
            crate::theme::CARD_IDLE
        };
        fill_rect(rect, bg);
        let border = if selected {
            SCRAP_COLOR
        } else if !card.enabled {
            crate::theme::BORDER_FAINT
        } else if hovered {
            crate::theme::EDGE_FOCUS
        } else {
            crate::theme::EDGE_CARD
        };
        stroke_rect(rect, crate::theme::Stroke::Edge.at(s), border);
        let tint = if card.enabled {
            WHITE
        } else {
            Color::new(1.0, 1.0, 1.0, 0.35)
        };
        if matches!(card.action, CardAction::ArmRally | CardAction::ClearRally) {
            draw_rally_control(card, rect, s);
            continue;
        }
        let horizontal = rect.w >= 100.0 * s;
        let icon_size = crate::theme::Type::Heading.at(s);
        draw_icon(
            Rect::new(
                if horizontal {
                    rect.x + 6.0 * s
                } else {
                    rect.x + (rect.w - icon_size) * 0.5
                },
                rect.y + if horizontal { 6.0 } else { 4.0 } * s,
                icon_size,
                icon_size,
            ),
            &card.icon,
            tint,
        );
        let name_x = if horizontal {
            rect.x + 36.0 * s
        } else {
            rect.x + 4.0 * s
        };
        let title_top = rect.y + if horizontal { 4.0 } else { 32.0 } * s;
        let title_bottom = if let Some(cost) = card.cost {
            let dims = measure_text(
                cost.to_string(),
                None,
                numeric::font_size(crate::theme::Type::Body.at(s)),
                1.0,
            );
            rect.y + rect.h - 5.0 * s - dims.offset_y - 3.0 * s
        } else {
            rect.y + rect.h - 4.0 * s
        };
        let title_width = rect.x + rect.w - name_x - 4.0 * s;
        let mut title_size = crate::theme::Type::Small.at(s);
        let (names, ascent) = loop {
            let names = card_title_lines(
                &card.title,
                |text| measure_text(text, None, numeric::font_size(title_size), 1.0).width,
                title_width,
            );
            let mut ascent = 0.0_f32;
            let mut descent = 0.0_f32;
            let mut width = 0.0_f32;
            for name in &names {
                let dims = measure_text(name, None, numeric::font_size(title_size), 1.0);
                ascent = ascent.max(dims.offset_y);
                descent = descent.max(dims.height - dims.offset_y);
                width = width.max(dims.width);
            }
            let height = ascent + descent + names.len().saturating_sub(1) as f32 * title_size;
            if (height <= title_bottom - title_top && width <= title_width) || title_size <= 8.0 * s
            {
                break (names, ascent);
            }
            title_size -= 0.5 * s;
        };
        for (line_index, name) in names.iter().enumerate() {
            draw_text(
                name,
                name_x,
                title_top + ascent + line_index as f32 * title_size,
                title_size,
                TEXT_PRIMARY,
            );
        }
        if let Some(cost) = card.cost {
            let label = format!("{cost}");
            let dims = measure_text(
                &label,
                None,
                numeric::font_size(crate::theme::Type::Body.at(s)),
                1.0,
            );
            draw_text(
                &label,
                rect.x + rect.w - dims.width - 5.0 * s,
                rect.y + rect.h - 5.0 * s,
                crate::theme::Type::Body.at(s),
                if card.enabled {
                    SCRAP_COLOR
                } else {
                    TEXT_DISABLED
                },
            );
        }
        if !card.hotkey.is_empty() {
            draw_text(
                &card.hotkey,
                rect.x + 4.0 * s,
                if horizontal {
                    rect.y + rect.h - 5.0 * s
                } else {
                    rect.y + 13.0 * s
                },
                crate::theme::Type::Small.at(s),
                TEXT_SECONDARY,
            );
        }
        if let (CardAction::Dispatch(crate::action::Action::TrainSlot(_)), CardIcon::Unit(kind)) =
            (card.action, card.icon)
        {
            let label = crate::panel::unit_train_time_label(kind);
            draw_text(
                &label,
                if horizontal {
                    rect.x + 54.0 * s
                } else {
                    rect.x + 4.0 * s
                },
                rect.y + rect.h - 5.0 * s,
                crate::theme::Type::Caption.at(s),
                if card.enabled {
                    TEXT_SECONDARY
                } else {
                    TEXT_DISABLED
                },
            );
        }
    }

    // Orders dock on the left edge: production ghosts or order chips.
    if let Some(dock) = dock {
        draw_dock(panel, input, dock, s, &draw_icon);
    }
}

fn draw_dock(
    panel: &crate::panel::Panel,
    input: &InputState,
    layout: &DockLayout,
    s: f32,
    draw_icon: &impl Fn(Rect, &crate::panel::CardIcon, Color),
) {
    use crate::panel::CardIcon;
    let DockLayout {
        dock,
        list,
        stop,
        chips,
        hidden,
    } = layout;
    // Plates border their open sides; the screen edge closes the left.
    let plate = |rect: Rect, bottom: bool| {
        let edge = crate::theme::BORDER_STRONG;
        fill_rect(rect, crate::theme::SURFACE_PLATE);
        draw_rectangle(rect.x, rect.y, rect.w, 1.5 * s, edge);
        draw_rectangle(rect.right() - 1.5 * s, rect.y, 1.5 * s, rect.h, edge);
        if bottom {
            draw_rectangle(rect.x, rect.bottom() - 1.5 * s, rect.w, 1.5 * s, edge);
        }
    };
    if list.h > 0.0 {
        plate(*list, false);
    }
    if !panel.queue.is_empty() {
        draw_text(
            &panel.queue_label,
            8.0 * s,
            list.y + 17.0 * s,
            crate::theme::Type::Small.at(s),
            TEXT_PRIMARY,
        );
    }
    if let (Some(card), Some((stop_plate, rect))) = (&panel.stop, *stop) {
        // Named where the chips beneath it are.
        plate(stop_plate, true);
        let named = rect.w >= 150.0 * s;
        fill_rect(rect, crate::theme::CHIP);
        stroke_rect(
            rect,
            crate::theme::Stroke::Edge.at(s),
            if hovers(input, rect) {
                crate::theme::EDGE_FOCUS
            } else {
                crate::theme::EDGE_CHIP
            },
        );
        let isz = 34.0 * s;
        draw_icon(
            Rect::new(
                rect.x + if named { 4.0 * s } else { (rect.w - isz) * 0.5 },
                rect.y + (rect.h - isz) * 0.5,
                isz,
                isz,
            ),
            &card.icon,
            WHITE,
        );
        if named {
            draw_text(
                &card.title,
                rect.x + 42.0 * s,
                rect.y + 27.0 * s,
                crate::theme::Type::Small.at(s),
                TEXT_PRIMARY,
            );
        }
    }
    let orders_dock = panel.queue_label == "Orders";
    for (i, (card, &rect)) in panel.queue.iter().zip(chips).enumerate() {
        let hovered = hovers(input, rect);
        fill_rect(rect, crate::theme::CHIP);
        // The active order or production head wears the bright border;
        // a ready-but-blocked head remains the queue's current job.
        let group = panel.queue_groups.get(i);
        let active = group.map_or(i == 0, |g| g.active > 0);
        let wide_group = group.is_some() && rect.w >= 150.0 * s;
        stroke_rect(
            rect,
            if active {
                crate::theme::Stroke::Focus
            } else {
                crate::theme::Stroke::Edge
            }
            .at(s),
            if hovered || active {
                crate::theme::EDGE_FOCUS
            } else {
                crate::theme::EDGE_CHIP
            },
        );
        // Order chips carry the same numbers as the world
        // breadcrumbs: chip 2 is waypoint 2.
        if orders_dock && panel.queue.len() > 1 {
            draw_text(
                format!("{}", i + 1),
                rect.x + 3.0 * s,
                rect.y + 13.0 * s,
                crate::theme::Type::Caption.at(s),
                TEXT_SECONDARY,
            );
        }
        {
            let isz = 34.0 * s;
            draw_icon(
                Rect::new(
                    rect.x
                        + if wide_group {
                            4.0 * s
                        } else {
                            (rect.w - isz) * 0.5
                        },
                    rect.y + 5.0 * s,
                    isz,
                    isz,
                ),
                &card.icon,
                WHITE,
            );
        }
        if let Some(group) = group.filter(|_| !wide_group) {
            let label = format!("x{}", group.count);
            let width = measure_text(
                &label,
                None,
                numeric::font_size(crate::theme::Type::Small.at(s)),
                1.0,
            )
            .width
                + 4.0 * s;
            draw_rectangle(
                rect.right() - width - 2.0 * s,
                rect.y + 2.0 * s,
                width,
                14.0 * s,
                crate::theme::SURFACE_CARD,
            );
            draw_text(
                label,
                rect.right() - width,
                rect.y + 13.0 * s,
                crate::theme::Type::Small.at(s),
                TEXT_PRIMARY,
            );
        }
        if let Some(group) = group.filter(|_| wide_group) {
            let name = match card.icon {
                CardIcon::Unit(kind) => crate::typography::entity_name(kind.name()),
                _ => card.title.clone(),
            };
            draw_text(
                format!("{name} x {}", group.count),
                rect.x + 42.0 * s,
                rect.y + 18.0 * s,
                crate::theme::Type::Small.at(s),
                TEXT_PRIMARY,
            );
            draw_text(
                match group.next_ticks {
                    Some(0) => format!("{} building | ready", group.active),
                    Some(ticks) => format!(
                        "{} building | {}",
                        group.active,
                        crate::panel::tick_time_label(ticks)
                    ),
                    None => "waiting".into(),
                },
                rect.x + 42.0 * s,
                rect.y + 34.0 * s,
                crate::theme::Type::Caption.at(s),
                TEXT_SECONDARY,
            );
        }
        // A chip with a measurable job wears its meter: the
        // production head's build, a site's rise, a patient's hp.
        // Read from the model, never peeked back out of the state.
        if let Some(frac) = card.progress {
            draw_rectangle(
                rect.x,
                rect.y + rect.h - 3.0 * s,
                rect.w * frac.clamp(0.0, 1.0),
                3.0 * s,
                SCRAP_COLOR,
            );
        }
    }
    if *hidden > 0 {
        draw_text(
            format!("+{hidden}"),
            12.0 * s,
            dock.y + dock.h - 8.0 * s,
            crate::theme::Type::Small.at(s),
            TEXT_SECONDARY,
        );
    }
}

/// Whether the mouse hovers `rect`, only while it is the pointer in use.
fn hovers(input: &InputState, rect: Rect) -> bool {
    super::chrome::hover_point(input).is_some_and(|p| rect.contains(p))
}

/// The hover tooltip for panel cards, drawn over everything: name,
/// hotkey, cost, description, weapon lines, and why a disabled card
/// refuses. Rebuilt from the same panel model the frame drew.
pub(crate) fn draw_panel_tooltip(game: &crate::game::Scene<'_>, input: &InputState) {
    use crate::layout::TooltipSide;
    let panel = game.presentation.panel_model.borrow();
    let Some(panel) = panel.as_ref() else {
        return;
    };
    let layout = game.presentation.layout.get();
    if !layout.panel_top.is_finite() {
        return;
    }
    let s = ui_scale();
    // A resting finger previews the card it covers; the mouse previews
    // what it hovers only while it is the pointer in use.
    let pointer = match input.touch_preview() {
        Some(p) => Some((p, Some(s))),
        None => super::chrome::hover_point(input).map(|p| (p, None)),
    };
    let Some(hit) =
        pointer.and_then(|(p, touch_ui)| crate::layout::card_under(&layout, p, touch_ui))
    else {
        return;
    };
    // The hovered rect is the anchor, not just the index: the orders
    // dock stacks upward from the band, so a tooltip pinned to the
    // band's top edge would sit far from a high chip.
    let Some(card) = panel.card(hit.row, hit.index) else {
        return;
    };
    let r = hit.rect;
    let (anchor, side) = if matches!(
        hit.row,
        crate::layout::CardRow::Queue | crate::layout::CardRow::Stop
    ) {
        // Anchored across the dock's full width so the box clears the
        // strip cleanly at any chip inset.
        (
            Rect::new(layout.orders.x, r.y, layout.orders.w.max(r.w), r.h),
            TooltipSide::RightOf,
        )
    } else {
        (r, TooltipSide::Above)
    };
    let mut lines: Vec<(String, Color)> = Vec::new();
    let header = if card.hotkey.is_empty() {
        card.title.clone()
    } else {
        format!("{}   [{}]", card.title, card.hotkey)
    };
    lines.push((header, TEXT_PRIMARY));
    if let Some(cost) = card.cost {
        lines.push((format!("{cost} scrap"), SCRAP_COLOR));
    }
    let intro_lines = lines.len();
    let comparison = matches!(card.action, crate::panel::CardAction::Upgrade)
        .then_some(panel.info.upgrade.as_ref())
        .flatten();
    let size = crate::theme::Type::Body.at(s);
    let pad = 12.0 * s;
    // The box wraps descriptions at a reading width, capped by the
    // window, instead of growing to the longest line.
    let wrap_w = (400.0 * s).min(screen_width() - 40.0 * s);
    for d in &card.desc {
        for line in crate::render::wrap_words(
            d,
            |t| measure_text(t, None, numeric::font_size(size), 1.0).width,
            wrap_w,
        ) {
            lines.push((line, TEXT_BODY));
        }
    }
    if let Some(why) = &card.why {
        lines.push((crate::typography::sentence_case(why), DANGER));
    }
    let text_width = lines
        .iter()
        .map(|(l, _)| measure_text(l, None, numeric::font_size(size), 1.0).width)
        .fold(0.0f32, f32::max);
    let mut columns = [0.0_f32; 2];
    if let Some(comparison) = comparison {
        for (column, header) in columns.iter_mut().zip(["Changes", "After upgrade"]) {
            *column = measure_text(header, None, numeric::font_size(size), 1.0).width;
        }
        for row in &comparison.rows {
            for (column, text) in columns.iter_mut().zip([&row.label, &row.upgraded]) {
                *column = column.max(measure_text(text, None, numeric::font_size(size), 1.0).width);
            }
        }
    }
    let table_width = if comparison.is_some() {
        columns.iter().sum::<f32>() + 20.0 * s
    } else {
        0.0
    };
    let width = text_width.max(table_width) + pad * 2.0;
    let line_h = 22.0 * s;
    let table_height = comparison.map_or(0.0, |c| (c.rows.len() + 2) as f32 * line_h);
    let height = lines.len() as f32 * line_h + table_height + pad * 1.5;
    // The box fits between the top bar and the band so it never covers
    // the command cards.
    let origin = crate::layout::tooltip_origin(
        anchor,
        vec2(width, height),
        side,
        vec2(
            screen_width(),
            if matches!(side, TooltipSide::Above) {
                anchor.y
            } else if layout.panel_regions[1].w > 0.0 {
                layout.panel_regions[1].y
            } else {
                screen_height()
            },
        ),
        layout.top_bar_h,
        6.0 * s,
    );
    let (x, y) = (origin.x, origin.y);
    draw_rectangle(x, y, width, height, crate::theme::SURFACE_CARD);
    draw_rectangle_lines(
        x,
        y,
        width,
        height,
        crate::theme::Stroke::Edge.at(s),
        crate::theme::EDGE_CARD,
    );
    for (i, (line, color)) in lines.iter().enumerate() {
        draw_text(
            line,
            x + pad,
            y + pad + (i as f32 + 0.6) * line_h + if i >= intro_lines { table_height } else { 0.0 },
            size,
            *color,
        );
    }
    if let Some(comparison) = comparison {
        let table_y = y + pad + intro_lines as f32 * line_h;
        let upgraded_x = x + pad + columns[0] + 20.0 * s;
        for (label, left) in [("Changes", x + pad), ("After upgrade", upgraded_x)] {
            draw_text(label, left, table_y + 0.6 * line_h, size, TEXT_SECONDARY);
        }
        draw_rectangle(
            x + pad,
            table_y + line_h,
            table_width,
            s,
            crate::theme::EDGE_CARD,
        );
        for (i, row) in comparison.rows.iter().enumerate() {
            let baseline = table_y + (i as f32 + 1.8) * line_h;
            draw_text(&row.label, x + pad, baseline, size, TEXT_BODY);
            draw_text(&row.upgraded, upgraded_x, baseline, size, SCRAP_COLOR);
        }
    }
}

#[cfg(test)]
mod tests;
