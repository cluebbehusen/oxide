//! The command band, the orders dock, and the hover tooltip — the
//! selection panel's entire drawn form. Geometry it publishes rides
//! the LayoutModel; the pure card model lives in crate::panel.

use super::*;

use super::panel_layout::{PanelGeometry, measure_info};

const CARD_RIGHT_INSET: f32 = 12.0;
const CARD_LEFT_INSET: f32 = 8.0;

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
    card_width.max((2.0 * crate::layout::MIN_TOUCH_TARGET + 6.0) * scale)
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
    let per_row = 1 + ((available - first_width) / (card_w + gap)).floor() as usize;
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
            let rally_width = ((group_width - gap * rally_count.saturating_sub(1) as f32)
                / rally_count.max(1) as f32)
                .max(crate::layout::MIN_TOUCH_TARGET * scale);
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
                        index as f32 * (rally_width + gap)
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
        let mut measured = measure_text(text, None, font_size as u16, 1.0);
        while measured.width > available && font_size > 8.0 * scale {
            font_size -= 0.5 * scale;
            measured = measure_text(text, None, font_size as u16, 1.0);
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

/// Packs every visible queue chip above the command band. A single
/// column stays pleasantly quiet when it fits; a full eight-slot
/// production queue becomes a 2×4 dock in the 640×400 stress case instead
/// of hiding paid, cancelable work behind a "+4" label.
fn queue_grid(queue_len: usize, panel_top: f32, scale: f32) -> (Rect, [Rect; 8], usize) {
    queue_grid_with_width(queue_len, panel_top, scale, 44.0)
}

fn queue_grid_with_width(
    queue_len: usize,
    panel_top: f32,
    scale: f32,
    width: f32,
) -> (Rect, [Rect; 8], usize) {
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut slots = [zero; 8];
    let count = queue_len.min(slots.len());
    if count == 0 {
        return (zero, slots, 0);
    }
    let (size, gap) = (44.0 * scale, 4.0 * scale);
    let label_h = 18.0 * scale;
    let available =
        (panel_top - crate::layout::TOP_BAR_H * scale - label_h - 2.0 * scale).max(size);
    let max_rows = (((available + gap) / (size + gap)).floor() as usize).max(1);
    let columns = count.div_ceil(max_rows).max(1);
    let rows = count.div_ceil(columns);
    let slot_width = width * scale;
    let width = 16.0 * scale + columns as f32 * slot_width + columns.saturating_sub(1) as f32 * gap;
    let height = label_h + rows as f32 * size + rows.saturating_sub(1) as f32 * gap + 2.0 * scale;
    let dock = Rect::new(0.0, panel_top - height, width, height);
    for (index, slot) in slots.iter_mut().take(count).enumerate() {
        let row = index / columns;
        let column = index % columns;
        *slot = Rect::new(
            8.0 * scale + column as f32 * (slot_width + gap),
            dock.y + label_h + row as f32 * (size + gap),
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
) -> (Rect, [Rect; 8], usize) {
    if viewport_width / scale < 800.0 {
        return queue_grid_with_width(count, top, scale, 64.0);
    }
    let wide = queue_grid_with_width(count, top, scale, 170.0);
    if wide.0.right() <= viewport_width {
        wide
    } else {
        queue_grid_with_width(count, top, scale, 64.0)
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
    let total_ticks = ticks.clone().sum();
    let later_ticks = ticks.skip(1).sum();
    let ready_width = if later_ticks == 0 {
        measure("queue ready")
    } else {
        measure("queue ready + ") + time_width(later_ticks)
    };
    (measure("queue ") + time_width(total_ticks)).max(ready_width)
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
    let max_columns = ((available + 4.0 * scale) / (116.0 * scale))
        .floor()
        .max(1.0) as usize;
    let rows = count.max(1).div_ceil(max_columns);
    let grouped = count == 13 && max_columns >= 7;
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

fn draw_catalog(
    panel: &crate::panel::Panel,
    input: &InputState,
    minimap: Rect,
    draw_icon: &impl Fn(Rect, &crate::panel::CardIcon, Color),
) -> PanelGeometry {
    use crate::panel::CardAction;
    let s = ui_scale();
    let (band, slots, grouped) = catalog_geometry(
        vec2(screen_width(), screen_height()),
        s,
        minimap,
        panel.cards.len(),
    );
    draw_rectangle(
        band.x,
        band.y,
        band.w,
        band.h,
        Color::from_rgba(20, 24, 26, 255),
    );
    draw_rectangle(
        band.x,
        band.y,
        band.w,
        s,
        Color::from_rgba(119, 107, 79, 180),
    );
    if !grouped {
        crate::typography::draw("BUILD", 12.0 * s, band.y + 20.0 * s, 15.0 * s, TEXT_PRIMARY);
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
            let label = if input.build_category == Some(category as u8) {
                format!("{label} *")
            } else {
                let key = input
                    .bindings
                    .label(crate::action::Action::BuildCategory(category as u8));
                if input.build_category.is_some() {
                    format!(
                        "{label} [{} > {key}]",
                        input
                            .bindings
                            .label(crate::action::Action::ToggleBuildPalette)
                    )
                } else {
                    format!("{label} [{key}]")
                }
            };
            crate::typography::draw(
                &label,
                slots[index].x + 5.0 * s,
                band.y + 20.0 * s,
                11.0 * s,
                TEXT_SECONDARY,
            );
        }
    }
    let mut cards = [(Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::None); 16];
    let hovered = slots.iter().position(|rect| rect.contains(input.mouse));
    for (i, (card, rect)) in panel.cards.iter().zip(slots).enumerate() {
        let armed =
            matches!(card.action, CardAction::ArmBuild(kind) if input.placing == Some(kind));
        let hot = hovered == Some(i);
        draw_rectangle(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            if armed {
                Color::from_rgba(67, 57, 37, 255)
            } else if hot {
                Color::from_rgba(48, 57, 58, 255)
            } else {
                Color::from_rgba(29, 35, 38, 255)
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
            |text| measure_text(text, None, (15.0 * s) as u16, 1.0).width,
            rect.w - 49.0 * s,
        );
        for (line, name) in names.iter().enumerate() {
            draw_text(
                name,
                rect.x + 44.0 * s,
                rect.y + (if names.len() > 1 { 16.0 } else { 22.0 } + line as f32 * 14.0) * s,
                15.0 * s,
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
                13.0 * s,
                if card.enabled {
                    SCRAP_COLOR
                } else {
                    TEXT_DISABLED
                },
            );
        }
        let key_width = measure_text(&card.hotkey, None, (11.0 * s) as u16, 1.0).width;
        draw_text(
            &card.hotkey,
            rect.x + rect.w - key_width - 5.0 * s,
            rect.y + 42.0 * s,
            11.0 * s,
            TEXT_SECONDARY,
        );
        cards[i] = (
            rect,
            if card.enabled {
                card.action
            } else {
                CardAction::None
            },
        );
    }
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    PanelGeometry {
        info: zero,
        actions: band,
        orders: zero,
        roster_slots: [(zero, CardAction::None); 8],
        roster_count: 0,
        cards,
        card_count: panel.cards.len(),
        queue_slots: [(zero, CardAction::None); 8],
        queue_count: 0,
        hides_minimap: false,
    }
}

fn selection_info_rect(viewport: Vec2, width: f32, content_height: f32, actions: Rect) -> Rect {
    let height = content_height.max(actions.h);
    Rect::new(0.0, viewport.y - height, width, height)
}

/// Draws the command panel band and returns its clickable geometry.
pub(crate) fn draw_panel(
    game: &Game,
    sprites: &Sprites,
    input: &InputState,
    panel: &crate::panel::Panel,
) -> PanelGeometry {
    use crate::panel::{CardAction, CardIcon};
    let s = ui_scale();
    let mini = minimap_rect(game);
    let viewport = vec2(screen_width(), screen_height());
    let small = viewport.x / s < 800.0 || viewport.y / s < 500.0;
    let packing = panel_packing(
        viewport,
        mini,
        s,
        panel.cards.len(),
        rally_card_count(&panel.cards),
    );
    let (cards_x, _, _, _) = card_metrics(viewport, s);
    let measured = measure_info(panel, cards_x, s, small, |text, size| {
        crate::typography::measure(text, size).width
    });
    let (card_rects, cards_w) = command_card_geometry(viewport, s, packing, &panel.cards);
    let action_rect = action_panel_rect(packing, cards_x, cards_w, !panel.cards.is_empty());
    let info_rect = selection_info_rect(viewport, cards_x, measured.height, action_rect);
    let top = info_rect.y;
    let roster_shown = panel.roster.len().min(8);
    let (rw, rh, roster_gap) = (measured.roster_size, measured.roster_size, 4.0 * s);
    let roster_per_row = measured.roster_columns;
    // The panel says whose colors it wears: an inspected ally or
    // enemy draws in its owner's faction, not the viewer's. Own
    // panels carry the human's faction, so roster cards stay right.
    let faction = panel.faction;
    let blit = |dest: Rect, source: Rect, tint: Color| {
        sprites.draw_canvas(dest, &[(source, tint)]);
    };
    // Defense art is authored as a base plus a north-facing live mount.
    // Static cards compose the same silhouette without inventing aim.
    let blit_building = |dest: Rect,
                         kind: oxide_sim::BuildingKind,
                         tier: u8,
                         faction: oxide_sim::Faction,
                         tint: Color| {
        let mut layers = vec![(sprites.building_tiered(kind, tier, faction), tint)];
        if let Some(mount) = sprites.defense_mount(kind, tier, faction) {
            layers.push((mount, tint));
        }
        sprites.draw_portrait(dest, &layers);
    };
    // An order chip is two composed draws: the subject's own silhouette
    // (translucent under a scaffold while its site is still rising) and
    // the verb as a corner badge on a dark plate, so the pictogram
    // never dissolves into the hull beneath it. Every other icon is the
    // one sprite it always was.
    let draw_icon = |dest: Rect, icon: &CardIcon, tint: Color| {
        let CardIcon::Order {
            subject,
            verb,
            ghost,
        } = icon
        else {
            match icon {
                CardIcon::Unit(kind) => {
                    sprites.draw_portrait(dest, &[(sprites.unit(*kind, faction), tint)])
                }
                CardIcon::Building(kind, tier) => blit_building(dest, *kind, *tier, faction, tint),
                CardIcon::Verb(v) => blit(dest, sprites.verb_icon(*v), tint),
                CardIcon::Order { verb, .. } => blit(dest, sprites.verb_icon(*verb), tint),
            }
            return;
        };
        // The subject wears ITS OWN colors: an attack chip's victim is
        // not the panel owner's faction.
        let hull = if *ghost {
            Color::new(tint.r, tint.g, tint.b, tint.a * 0.7)
        } else {
            tint
        };
        let mut layers = match subject {
            crate::panel::OrderSubject::Unit(kind, f) => vec![(sprites.unit(*kind, *f), hull)],
            crate::panel::OrderSubject::Building(kind, f) => {
                let mut layers = vec![(sprites.building(*kind, *f), hull)];
                if !*ghost && let Some(mount) = sprites.defense_mount(*kind, 0, *f) {
                    layers.push((mount, hull));
                }
                layers
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
        draw_rectangle(
            plate.x,
            plate.y,
            plate.w,
            plate.h,
            Color::new(0.05, 0.05, 0.07, tint.a * 0.85),
        );
        blit(plate, sprites.verb_icon(*verb), tint);
    };

    if panel
        .cards
        .iter()
        .all(|card| matches!(card.action, CardAction::ArmBuild(_)))
        && !panel.cards.is_empty()
    {
        return draw_catalog(panel, input, mini, &draw_icon);
    }

    for rect in [info_rect, action_rect] {
        if rect.w == 0.0 {
            continue;
        }
        draw_rectangle(
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            Color::from_rgba(20, 24, 26, 255),
        );
        draw_rectangle(
            rect.x,
            rect.y,
            rect.w,
            s,
            Color::from_rgba(119, 107, 79, 180),
        );
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
            15.0 * s,
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
    if let Some((hp, max_hp)) = panel.info.health {
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
            cards_x - 12.0 * s - measure_text(&value, None, measured.font as u16, 1.0).width,
            y + 13.0 * s,
            measured.font,
            TEXT_PRIMARY,
        );
        draw_rectangle(
            12.0 * s,
            y + 19.0 * s,
            cards_x - 24.0 * s,
            3.0 * s,
            Color::from_rgba(49, 61, 49, 255),
        );
        draw_rectangle(
            12.0 * s,
            y + 19.0 * s,
            (cards_x - 24.0 * s) * (hp as f32 / max_hp.max(1) as f32).clamp(0.0, 1.0),
            3.0 * s,
            Color::from_rgba(165, 180, 142, 255),
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
        let value_width = measure_text(&line.value, None, measured.font as u16, 1.0).width;
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
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut roster_slots = [(zero, CardAction::None); 8];
    let mut roster_count = 0;
    if roster_shown > 0 {
        let label_y = top + measured.roster_y - 4.0 * s;
        draw_text(
            "SELECTED UNITS",
            12.0 * s,
            label_y,
            12.0 * s,
            TEXT_SECONDARY,
        );
        for (index, card) in panel.roster.iter().take(roster_shown).enumerate() {
            let (row, column) = (index / roster_per_row, index % roster_per_row);
            let rect = Rect::new(
                8.0 * s + column as f32 * (rw + roster_gap),
                top + measured.roster_y + row as f32 * (rh + roster_gap),
                rw,
                rh,
            );
            let hovered = rect.contains(input.mouse);
            draw_rectangle(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                if hovered {
                    Color::from_rgba(48, 57, 58, 255)
                } else {
                    Color::new(0.13, 0.13, 0.17, 1.0)
                },
            );
            draw_rectangle_lines(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                if hovered { 2.0 * s } else { 1.2 * s },
                if hovered {
                    BONE
                } else {
                    Color::new(0.48, 0.48, 0.56, 0.9)
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
            let mut size = 11.0 * s;
            let mut dims = measure_text(label, None, size as u16, 1.0);
            while dims.width > rect.w - 6.0 * s && size > 8.0 * s {
                size -= 0.5 * s;
                dims = measure_text(label, None, size as u16, 1.0);
            }
            draw_text(
                label,
                rect.x + (rect.w - dims.width) * 0.5,
                rect.y + rect.h - 6.0 * s,
                size,
                TEXT_PRIMARY,
            );
            roster_slots[roster_count] = (rect, card.action);
            roster_count += 1;
        }
    }

    // Command cards, wrapping into as many rows as the width demands.
    let mut cards = [(zero, CardAction::None); 16];
    let mut card_count = 0;
    for (card, rect) in panel.cards.iter().zip(card_rects) {
        let hovered = rect.contains(input.mouse);
        let selected = matches!(card.action, CardAction::ArmBuild(kind) if input.placing == Some(kind))
            || (card.action == CardAction::ArmRally && !input.rallying.is_empty());
        let bg = if selected {
            Color::from_rgba(61, 47, 31, 255)
        } else if hovered && card.enabled {
            Color::from_rgba(48, 57, 58, 255)
        } else {
            Color::from_rgba(29, 35, 38, 255)
        };
        draw_rectangle(rect.x, rect.y, rect.w, rect.h, bg);
        let border = if selected {
            SCRAP_COLOR
        } else if !card.enabled {
            Color::new(0.4, 0.4, 0.45, 0.5)
        } else if hovered {
            BONE
        } else {
            Color::from_rgba(55, 65, 66, 180)
        };
        draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.5 * s, border);
        let tint = if card.enabled {
            WHITE
        } else {
            Color::new(1.0, 1.0, 1.0, 0.35)
        };
        if matches!(card.action, CardAction::ArmRally | CardAction::ClearRally) {
            draw_rally_control(card, rect, s);
            cards[card_count] = (
                rect,
                if card.enabled {
                    card.action
                } else {
                    CardAction::None
                },
            );
            card_count += 1;
            continue;
        }
        let horizontal = rect.w >= 100.0 * s;
        let icon_size = 24.0 * s;
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
            let dims = measure_text(cost.to_string(), None, (16.0 * s) as u16, 1.0);
            rect.y + rect.h - 5.0 * s - dims.offset_y - 3.0 * s
        } else {
            rect.y + rect.h - 4.0 * s
        };
        let title_width = rect.x + rect.w - name_x - 4.0 * s;
        let mut title_size = 14.0 * s;
        let (names, ascent) = loop {
            let names = card_title_lines(
                &card.title,
                |text| measure_text(text, None, title_size as u16, 1.0).width,
                title_width,
            );
            let mut ascent = 0.0_f32;
            let mut descent = 0.0_f32;
            let mut width = 0.0_f32;
            for name in &names {
                let dims = measure_text(name, None, title_size as u16, 1.0);
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
            let dims = measure_text(&label, None, (16.0 * s) as u16, 1.0);
            draw_text(
                &label,
                rect.x + rect.w - dims.width - 5.0 * s,
                rect.y + rect.h - 5.0 * s,
                16.0 * s,
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
                12.0 * s,
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
                11.0 * s,
                if card.enabled {
                    TEXT_SECONDARY
                } else {
                    TEXT_DISABLED
                },
            );
        }
        cards[card_count] = (
            rect,
            if card.enabled {
                card.action
            } else {
                CardAction::None
            },
        );
        card_count += 1;
    }

    // Orders dock on the left edge: production ghosts or order chips,
    // stacked above the band's corner so the band itself stays short.
    let mut queue_slots = [(zero, CardAction::None); 8];
    let mut queue_count = 0;
    let mut dock = Rect::new(0.0, 0.0, 0.0, 0.0);
    if !panel.queue.is_empty() {
        let (mut grid_dock, grid_slots, n) = if panel.queue_groups.is_empty() {
            queue_grid(panel.queue.len(), top, s)
        } else {
            collective_queue_grid(panel.queue.len(), top, s, viewport.x)
        };
        let queue_label_width = queue_label_width(panel, |text| {
            measure_text(text, None, (13.0 * s) as u16, 1.0).width
        }) + 16.0 * s;
        grid_dock.w = grid_dock.w.max(queue_label_width);
        dock = grid_dock;
        let hidden = panel.queue.len().saturating_sub(n);
        let more_h = if hidden > 0 { 16.0 * s } else { 0.0 };
        if more_h > 0.0 {
            dock.y -= more_h;
            dock.h += more_h;
        }
        let dock_top = dock.y;
        draw_rectangle(
            dock.x,
            dock.y,
            dock.w,
            dock.h,
            Color::from_rgba(20, 20, 24, 255),
        );
        draw_rectangle(
            dock.x,
            dock.y,
            dock.w,
            1.5 * s,
            Color::new(0.6, 0.6, 0.65, 0.4),
        );
        draw_rectangle(
            dock.x + dock.w - 1.5 * s,
            dock.y,
            1.5 * s,
            dock.h,
            Color::new(0.6, 0.6, 0.65, 0.4),
        );
        draw_text(
            &panel.queue_label,
            8.0 * s,
            dock_top + 15.0 * s,
            13.0 * s,
            TEXT_SECONDARY,
        );
        let orders_dock = panel.queue_label.starts_with("orders");
        for (i, card) in panel.queue.iter().take(n).enumerate() {
            let mut rect = grid_slots[i];
            rect.y -= more_h;
            let hovered = rect.contains(input.mouse);
            draw_rectangle(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                Color::new(0.14, 0.14, 0.18, 1.0),
            );
            // The active order or production head wears the bright border;
            // a ready-but-blocked head remains the queue's current job.
            let group = panel.queue_groups.get(i);
            let active = group.map_or(i == 0, |g| g.active > 0);
            let wide_group = group.is_some() && rect.w >= 150.0 * s;
            draw_rectangle_lines(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                if active { 2.0 * s } else { 1.2 * s },
                if hovered || active {
                    BONE
                } else {
                    Color::new(0.45, 0.45, 0.52, 0.8)
                },
            );
            // Order chips carry the same numbers the world breadcrumbs
            // wear — chip 2 IS waypoint 2.
            if orders_dock && panel.queue.len() > 1 {
                draw_text(
                    format!("{}", i + 1),
                    rect.x + 3.0 * s,
                    rect.y + 13.0 * s,
                    11.0 * s,
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
                let width = measure_text(&label, None, (12.0 * s) as u16, 1.0).width + 4.0 * s;
                draw_rectangle(
                    rect.right() - width - 2.0 * s,
                    rect.y + 2.0 * s,
                    width,
                    14.0 * s,
                    Color::from_rgba(20, 20, 24, 235),
                );
                draw_text(
                    label,
                    rect.right() - width,
                    rect.y + 13.0 * s,
                    12.0 * s,
                    BONE,
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
                    13.0 * s,
                    BONE,
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
                    11.0 * s,
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
            queue_slots[queue_count] = (rect, card.action);
            queue_count += 1;
        }
        if hidden > 0 {
            draw_text(
                format!("+{hidden}"),
                12.0 * s,
                dock_top + dock.h - 8.0 * s,
                13.0 * s,
                TEXT_SECONDARY,
            );
        }
    }
    PanelGeometry {
        info: info_rect,
        actions: action_rect,
        orders: dock,
        roster_slots,
        roster_count,
        cards,
        card_count,
        queue_slots,
        queue_count,
        hides_minimap: packing.hides_minimap,
    }
}

/// The hover tooltip for panel cards, drawn over everything: name,
/// hotkey, cost, description, weapon lines, and why a disabled card
/// refuses. Rebuilt from the same panel model the frame drew.
pub(crate) fn draw_panel_tooltip(game: &Game, input: &InputState) {
    let panel = game.panel_model.borrow();
    let Some(panel) = panel.as_ref() else {
        return;
    };
    let layout = game.layout.get();
    if !layout.panel_top.is_finite() {
        return;
    }
    use crate::layout::TooltipSide;
    let s = ui_scale();
    // The hovered RECT is the anchor, not just the index: the orders
    // dock stacks upward from the band, so a tooltip pinned to the
    // band's top edge described chip 1 beside chip 8.
    let hovered = layout.roster_slots[..layout.roster_count]
        .iter()
        .enumerate()
        .find(|(_, (r, _))| r.w > 0.0 && r.contains(input.mouse))
        .and_then(|(i, (r, _))| panel.roster.get(i).map(|c| (c, *r, TooltipSide::Above)))
        .or_else(|| {
            layout.cards[..layout.card_count]
                .iter()
                .enumerate()
                .find(|(_, (r, _))| r.w > 0.0 && r.contains(input.mouse))
                .and_then(|(i, (r, _))| panel.cards.get(i).map(|c| (c, *r, TooltipSide::Above)))
        })
        .or_else(|| {
            layout.queue_slots[..layout.queue_count]
                .iter()
                .enumerate()
                .find(|(_, (r, _))| r.w > 0.0 && r.contains(input.mouse))
                .and_then(|(i, (r, _))| {
                    // Anchored across the dock's full width so the box
                    // clears the strip cleanly at any chip inset.
                    let row = Rect::new(layout.orders.x, r.y, layout.orders.w.max(r.w), r.h);
                    panel.queue.get(i).map(|c| (c, row, TooltipSide::RightOf))
                })
        });
    let Some((card, anchor, side)) = hovered else {
        return;
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
    let comparison = matches!(card.action, crate::panel::CardAction::Upgrade(_))
        .then_some(panel.info.upgrade.as_ref())
        .flatten();
    let size = 17.0 * s;
    let pad = 12.0 * s;
    // Descriptions run to two sentences; the box wraps them at a
    // reading width instead of growing to the longest line, which
    // once put a Skyhook tooltip wider than the window.
    let wrap_w = (400.0 * s).min(screen_width() - 40.0 * s);
    for d in &card.desc {
        for line in
            crate::render::wrap_words(d, |t| measure_text(t, None, size as u16, 1.0).width, wrap_w)
        {
            lines.push((line, TEXT_BODY));
        }
    }
    if let Some(why) = &card.why {
        lines.push((why.clone(), DANGER));
    }
    let text_width = lines
        .iter()
        .map(|(l, _)| measure_text(l, None, size as u16, 1.0).width)
        .fold(0.0f32, f32::max);
    let mut columns = [0.0_f32; 2];
    if let Some(comparison) = comparison {
        for (column, header) in columns.iter_mut().zip(["Changes", "After upgrade"]) {
            *column = measure_text(header, None, size as u16, 1.0).width;
        }
        for row in &comparison.rows {
            for (column, text) in columns.iter_mut().zip([&row.label, &row.upgraded]) {
                *column = column.max(measure_text(text, None, size as u16, 1.0).width);
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
    // The box's room is the window BETWEEN the top bar and the band:
    // a tooltip that spilled over the command cards would cover what
    // the hand is about to click next.
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
    draw_rectangle(x, y, width, height, Color::from_rgba(12, 12, 16, 240));
    draw_rectangle_lines(
        x,
        y,
        width,
        height,
        1.2 * s,
        Color::from_rgba(55, 65, 66, 180),
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
            Color::from_rgba(55, 65, 66, 180),
        );
        for (i, row) in comparison.rows.iter().enumerate() {
            let baseline = table_y + (i as f32 + 1.8) * line_h;
            draw_text(&row.label, x + pad, baseline, size, TEXT_BODY);
            draw_text(&row.upgraded, upgraded_x, baseline, size, SCRAP_COLOR);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collective_queue_keeps_every_kind_visible_in_narrow_windows() {
        for scale in [1.0, 1.5, 2.0] {
            for (width, top) in [(640.0, 110.0), (800.0, 190.0), (1280.0, 650.0)] {
                for count in 1..=8 {
                    let (dock, slots, shown) =
                        collective_queue_grid(count, top * scale, scale, width * scale);
                    assert_eq!(shown, count);
                    assert!(dock.right() <= width * scale);
                    assert!(dock.y >= crate::layout::TOP_BAR_H * scale);
                    for (i, slot) in slots.iter().take(shown).enumerate() {
                        assert!(slot.x >= dock.x && slot.right() <= dock.right());
                        assert!(slot.y >= dock.y && slot.bottom() <= top * scale);
                        assert!(slots[..i].iter().all(|other| !slot.overlaps(other)));
                    }
                }
            }
        }
    }

    #[test]
    fn production_dock_width_survives_countdowns_and_blocked_completion() {
        let mut game =
            Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0)).unwrap();
        let foundry = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player == game.human && b.kind == oxide_sim::BuildingKind::Foundry)
            .unwrap()
            .id;
        game.selection.buildings = vec![foundry];
        let train = oxide_sim::PlayerCommand {
            player: game.human,
            command: oxide_sim::Command::Train {
                building: foundry,
                kind: oxide_sim::UnitKind::Harvester,
            },
        };
        game.state.tick(&[train.clone(), train]);
        let measure = |text: &str| {
            text.chars()
                .map(|c| match c {
                    '1' => 3.0,
                    '8' => 9.0,
                    _ => 7.0,
                })
                .sum::<f32>()
        };
        let mut widths = Vec::new();
        let mut labels = Vec::new();
        for _ in 0..oxide_sim::UnitKind::Harvester.stats().train_ticks {
            let mut panel = crate::panel::build_for_palette(
                &game,
                &crate::action::BindingMap::classic(),
                false,
            )
            .unwrap();
            if panel.queue.len() < 2 {
                assert!(queue_label_width(&panel, measure) < widths[0]);
                break;
            }
            let width = queue_label_width(&panel, measure);
            assert!(measure(&panel.queue_label) <= width);
            labels.push(panel.queue_label.clone());
            widths.push(width);
            panel.queue_label = "queue ready + 5s".into();
            assert_eq!(queue_label_width(&panel, measure), width);
            assert!(measure(&panel.queue_label) <= width);
            game.state.tick(&[]);
        }
        assert!(labels.iter().any(|label| label == "queue 10s"));
        assert!(labels.iter().any(|label| label == "queue 9.9s"));
        assert!(widths.iter().all(|width| *width == widths[0]));
        assert_eq!(game.state.building(foundry).unwrap().queue.len(), 1);
    }

    #[test]
    fn fabricator_corner_reaches_wrapped_actions_and_keeps_queue_above_it() {
        let mut scenario = oxide_sim::Scenario::skirmish();
        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
            player: 0,
            kind: oxide_sim::BuildingKind::Fabricator,
            x: 9,
            y: 3,
        });
        let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
        game.selection.buildings = vec![
            game.state
                .buildings()
                .iter()
                .find(|b| b.player == game.human && b.kind == oxide_sim::BuildingKind::Fabricator)
                .unwrap()
                .id,
        ];
        let panel =
            crate::panel::build_for_palette(&game, &crate::action::BindingMap::classic(), false)
                .unwrap();
        for (viewport, scale) in [(vec2(1280.0, 800.0), 1.0), (vec2(1920.0, 1200.0), 1.5)] {
            let minimap = minimap_rect_scaled(40, 24, viewport, scale);
            let packing = panel_packing(
                viewport,
                minimap,
                scale,
                panel.cards.len(),
                rally_card_count(&panel.cards),
            );
            let (left, _, _, _) = card_metrics(viewport, scale);
            let measured = measure_info(&panel, left, scale, false, |text, size| {
                text.len() as f32 * size * 0.5
            });
            let (cards, right) = command_card_geometry(viewport, scale, packing, &panel.cards);
            let actions = action_panel_rect(packing, left, right, !panel.cards.is_empty());
            assert!(cards.iter().any(|card| card.y > cards[0].y));
            assert!(measured.height < actions.h);
            let info = selection_info_rect(viewport, left, measured.height, actions);
            assert_eq!(info.y, actions.y);
            assert_eq!(info.bottom(), viewport.y);
            let (dock, slots, count) = queue_grid(8, info.y, scale);
            assert_eq!(count, 8);
            assert_eq!(dock.bottom(), info.y);
            assert!(slots[..count].iter().all(|slot| slot.bottom() <= info.y));
        }
    }

    #[test]
    fn production_cards_stay_inside_the_band_across_layouts_and_actions() {
        use crate::action::BindingMap;
        use oxide_sim::{BuildingKind, Command, Faction, PlayerCommand, Scenario};

        for faction in [Faction::Ferrous, Faction::Cupric] {
            for kind in BuildingKind::ALL
                .into_iter()
                .filter(|kind| !kind.base_stats().produces.is_empty())
            {
                for full_queue in [false, true] {
                    let mut scenario = Scenario::skirmish();
                    scenario.players[0].faction = faction;
                    scenario.players[0].scrap = if full_queue { 10_000 } else { 0 };
                    if kind != BuildingKind::Foundry {
                        scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
                            player: 0,
                            kind,
                            x: 9,
                            y: 3,
                        });
                    }
                    let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
                    let building = game
                        .state
                        .buildings()
                        .iter()
                        .find(|building| building.player == game.human && building.kind == kind)
                        .unwrap()
                        .id;
                    game.selection.buildings = vec![building];
                    if full_queue {
                        let unit = *kind
                            .base_stats()
                            .produces
                            .iter()
                            .find(|unit| unit.faction().is_none_or(|owner| owner == faction))
                            .unwrap();
                        for _ in 0..oxide_sim::stats::QUEUE_CAP {
                            game.state.tick(&[PlayerCommand {
                                player: game.human,
                                command: Command::Train {
                                    building,
                                    kind: unit,
                                },
                            }]);
                        }
                        assert_eq!(
                            game.state.building(building).unwrap().queue.len(),
                            oxide_sim::stats::QUEUE_CAP
                        );
                    }
                    for rally in [false, true] {
                        if rally {
                            game.state.tick(&[PlayerCommand {
                                player: game.human,
                                command: Command::SetRally {
                                    building,
                                    rally: Some(chassis::grid::TilePos::new(12, 8)),
                                },
                            }]);
                        }
                        let panel =
                            crate::panel::build_for_palette(&game, &BindingMap::classic(), false)
                                .unwrap();
                        for width in (640..=1920).step_by(17).chain([799, 800, 1280, 1440, 1920]) {
                            for height in [400, 499, 500, 600, 800, 1080] {
                                let viewport = vec2(width as f32, height as f32);
                                for preference in [0.75, 1.0, 1.25, 1.5] {
                                    let scale = effective_ui_scale(preference, viewport);
                                    let minimap = minimap_rect_scaled(40, 24, viewport, scale);
                                    let packing = panel_packing(
                                        viewport,
                                        minimap,
                                        scale,
                                        panel.cards.len(),
                                        rally_card_count(&panel.cards),
                                    );
                                    let (slots, right) = command_card_geometry(
                                        viewport,
                                        scale,
                                        packing,
                                        &panel.cards,
                                    );
                                    assert_eq!(slots.len(), panel.cards.len());
                                    let rally_count = rally_card_count(&panel.cards);
                                    if rally_count > 0 {
                                        assert!(
                                            slots[..rally_count].iter().all(|r| r.y == slots[0].y)
                                        );
                                        let production_x = slots[rally_count].x;
                                        assert!(
                                            slots[rally_count..]
                                                .iter()
                                                .all(|r| r.x >= production_x)
                                        );
                                    }
                                    assert!(packing.top >= crate::layout::TOP_BAR_H * scale);
                                    for (index, rect) in slots.iter().enumerate() {
                                        assert!(
                                            rect.x + rect.w + CARD_RIGHT_INSET * scale
                                                <= right + 0.001,
                                            "{kind:?} {faction:?} rally={rally} queue={full_queue} viewport={viewport:?} scale={scale}: card {index} ends at {}, band ends at {right}",
                                            rect.x + rect.w
                                        );
                                        assert!(
                                            rect.y >= packing.top
                                                && rect.y + rect.h <= viewport.y + 0.001
                                        );
                                        assert!(
                                            rect.w >= crate::layout::MIN_TOUCH_TARGET * scale
                                                && rect.h
                                                    >= crate::layout::MIN_TOUCH_TARGET * scale
                                        );
                                        assert!(
                                            slots[..index]
                                                .iter()
                                                .all(|other| !rect.overlaps(other))
                                        );
                                        assert!(packing.hides_minimap || !rect.overlaps(&minimap));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn narrow_cards_wrap_compound_orders_without_losing_the_hyphen() {
        let width = |text: &str| text.len() as f32;
        assert_eq!(
            card_title_lines("Attack-move", width, 7.0),
            ["Attack-", "move"]
        );
        assert_eq!(
            card_title_lines("Attack-move", width, 11.0),
            ["Attack-move"]
        );
        assert_eq!(
            card_title_lines("Scuttle Charge", width, 8.0),
            ["Scuttle", "Charge"]
        );
    }

    #[test]
    fn construction_catalog_keeps_every_choice_and_minimap_at_supported_sizes() {
        for viewport in [vec2(640.0, 400.0), vec2(1280.0, 800.0), vec2(1440.0, 900.0)] {
            let minimap = minimap_rect_scaled(36, 24, viewport, 1.0);
            let (band, slots, _) = catalog_geometry(viewport, 1.0, minimap, 13);
            assert_eq!(slots.len(), 13);
            assert!(band.y >= crate::layout::TOP_BAR_H);
            assert!(band.w < minimap.x);
            for (i, rect) in slots.iter().enumerate() {
                assert!(rect.w >= crate::layout::MIN_TOUCH_TARGET);
                assert!(rect.h >= crate::layout::MIN_TOUCH_TARGET);
                assert!(band.contains(vec2(rect.x, rect.y)));
                assert!(rect.y + rect.h <= viewport.y);
                assert!(slots[..i].iter().all(|other| !rect.overlaps(other)));
            }
            if viewport.x >= 1280.0 {
                assert!(band.h <= 170.0);
            }
        }
    }

    #[test]
    fn rally_changes_preserve_single_and_grouped_production_geometry() {
        use crate::action::BindingMap;
        use oxide_sim::{BuildingKind, Command, Faction, PlayerCommand, Scenario};

        for faction in [Faction::Ferrous, Faction::Cupric] {
            for kind in BuildingKind::ALL
                .into_iter()
                .filter(|kind| !kind.base_stats().produces.is_empty())
            {
                let mut scenario = Scenario::skirmish();
                scenario.players[0].faction = faction;
                for x in [9, 14] {
                    scenario.buildings.push(oxide_sim::scenario::BuildingSpec {
                        player: 0,
                        kind,
                        x,
                        y: 3,
                    });
                }
                let mut game = Game::with_viewport(scenario, vec2(1280.0, 800.0)).unwrap();
                let producers: Vec<_> = game
                    .state
                    .buildings()
                    .iter()
                    .filter(|b| b.player == game.human && b.kind == kind)
                    .map(|b| b.id)
                    .collect();
                for selection in [
                    vec![producers[0]],
                    producers.clone(),
                    game.state
                        .buildings()
                        .iter()
                        .filter(|b| b.player == game.human)
                        .map(|b| b.id)
                        .collect(),
                ] {
                    game.selection.buildings = selection.clone();
                    let geometry = |game: &Game| {
                        let panel =
                            crate::panel::build_for_palette(game, &BindingMap::classic(), false)
                                .unwrap();
                        assert_eq!(rally_card_count(&panel.cards), 2);
                        let mut result = Vec::new();
                        for viewport in [
                            vec2(640.0, 400.0),
                            vec2(800.0, 600.0),
                            vec2(1280.0, 800.0),
                            vec2(1920.0, 1080.0),
                        ] {
                            for preference in [0.75, 1.0, 1.25, 1.5] {
                                let scale = effective_ui_scale(preference, viewport);
                                let minimap = minimap_rect_scaled(40, 24, viewport, scale);
                                let packing =
                                    panel_packing(viewport, minimap, scale, panel.cards.len(), 2);
                                let (slots, right) =
                                    command_card_geometry(viewport, scale, packing, &panel.cards);
                                assert_eq!(slots[0].y, slots[1].y);
                                assert!(slots[1].x >= slots[0].right());
                                if viewport == vec2(1920.0, 1080.0) && preference == 1.0 {
                                    assert_eq!(packing.band_h, 72.0);
                                }
                                result.push((packing, slots, right));
                            }
                        }
                        result
                    };
                    let baseline = geometry(&game);
                    // Partial and shared rallies exercise both grouped panel states.
                    for ids in [vec![selection[0]], selection.clone()] {
                        for rally in [Some(chassis::grid::TilePos::new(20, 10)), None] {
                            let commands: Vec<_> = ids
                                .iter()
                                .map(|id| PlayerCommand {
                                    player: game.human,
                                    command: Command::SetRally {
                                        building: *id,
                                        rally,
                                    },
                                })
                                .collect();
                            game.state.tick(&commands);
                            assert_eq!(geometry(&game), baseline);
                            let panel = crate::panel::build_for_palette(
                                &game,
                                &BindingMap::classic(),
                                false,
                            )
                            .unwrap();
                            assert_eq!(panel.cards[1].enabled, rally.is_some());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn production_wraps_in_its_own_columns_beside_rally_controls() {
        assert_eq!(grouped_card_rows(7, 1, 6), 2);
        assert_eq!(grouped_card_slot(1, 1, 6), (0, 1, true));
        assert_eq!(grouped_card_slot(6, 1, 6), (1, 1, true));
        assert_eq!(grouped_card_rows(8, 2, 6), 2);
        assert_eq!(grouped_card_slot(0, 2, 6), (0, 0, false));
        assert_eq!(grouped_card_slot(1, 2, 6), (0, 0, false));
        assert_eq!(grouped_card_slot(2, 2, 6), (0, 1, true));
        assert_eq!(grouped_card_slot(7, 2, 6), (1, 1, true));
        assert_eq!(grouped_card_gap(7, 2, 5, 400.0, 66.0, 6.0), 10.0);
        assert_eq!(grouped_card_gap(7, 2, 5, 354.0, 66.0, 6.0), 0.0);

        assert_eq!(grouped_card_rows(7, 0, 5), 2);
        assert_eq!(grouped_card_slot(5, 0, 5), (1, 0, false));
        assert_eq!(grouped_card_rows(7, 2, 1), 6);
        assert_eq!(grouped_card_slot(2, 2, 1), (1, 0, false));
    }

    #[test]
    fn eight_paid_slots_form_a_fully_clickable_small_window_dock() {
        // A 120px command band leaves panel_top=280 in the
        // 640×400 stress case. Every slot must remain present, above the band,
        // and below the top bar rather than folding into "+N".
        let (dock, slots, count) = queue_grid(8, 280.0, 1.0);
        assert_eq!(count, 8);
        assert!(dock.y >= crate::layout::TOP_BAR_H);
        assert_eq!(
            slots[..count]
                .iter()
                .map(|slot| slot.x.to_bits())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            2,
            "the full queue uses two columns"
        );
        assert_eq!(
            slots[..count]
                .iter()
                .map(|slot| slot.y.to_bits())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            4,
            "the full queue uses four rows"
        );
        for slot in &slots[..count] {
            assert!(slot.x >= dock.x && slot.x + slot.w <= dock.x + dock.w);
            assert!(slot.y >= dock.y && slot.y + slot.h <= dock.y + dock.h);
            assert!(slot.y + slot.h <= 280.0);
        }
    }

    #[test]
    fn a_queue_that_fits_stays_in_one_quiet_column() {
        let (_, slots, count) = queue_grid(5, 680.0, 1.0);
        assert_eq!(count, 5);
        assert!(slots[..count].iter().all(|slot| slot.x == slots[0].x));
    }

    #[test]
    fn a_dense_small_window_panel_yields_the_minimap_before_overflowing() {
        let viewport = vec2(640.0, 400.0);
        let minimap = minimap_rect_scaled(40, 24, viewport, 1.0);
        let packing = panel_packing(viewport, minimap, 1.0, 16, 0);

        assert!(packing.hides_minimap);
        assert_eq!(packing.right, viewport.x);
        assert!(packing.top >= crate::layout::TOP_BAR_H);
        assert!(packing.top + packing.band_h <= viewport.y);
    }

    #[test]
    fn a_mixed_small_window_selection_keeps_roster_and_queue_in_the_corner() {
        let mut scenario = oxide_sim::Scenario::skirmish();
        scenario.units.clear();
        for (index, kind) in [
            oxide_sim::UnitKind::Sentinel,
            oxide_sim::UnitKind::Harvester,
            oxide_sim::UnitKind::Tender,
            oxide_sim::UnitKind::Skyhook,
            oxide_sim::UnitKind::Scuttler,
            oxide_sim::UnitKind::Lancer,
            oxide_sim::UnitKind::Bombard,
            oxide_sim::UnitKind::Kestrel,
        ]
        .into_iter()
        .enumerate()
        {
            scenario.units.push(oxide_sim::scenario::UnitSpec {
                player: 0,
                kind,
                x: 6 + index as i32,
                y: 5,
            });
        }
        let mut game = Game::with_viewport(scenario, vec2(640.0, 400.0)).unwrap();
        game.selection.units = game.state.units().iter().map(|u| u.id).collect();
        let panel =
            crate::panel::build_for_palette(&game, &crate::action::BindingMap::classic(), false)
                .unwrap();
        let info = measure_info(&panel, 204.0, 1.0, true, |text, size| {
            text.len() as f32 * size * 0.5
        });
        assert_eq!(panel.roster.len(), 8);
        assert_eq!(info.roster_columns, 4);
        let (dock, slots, count) = queue_grid(8, 400.0 - info.height, 1.0);
        assert_eq!(count, 8);
        assert!(dock.y >= crate::layout::TOP_BAR_H);
        assert!(dock.w <= 204.0);
        assert!(
            slots[..count]
                .iter()
                .all(|slot| slot.y >= crate::layout::TOP_BAR_H)
        );
    }

    #[test]
    fn a_simple_panel_keeps_the_minimap() {
        let viewport = vec2(640.0, 400.0);
        let minimap = minimap_rect_scaled(40, 24, viewport, 1.0);
        let packing = panel_packing(viewport, minimap, 1.0, 2, 0);

        assert!(!packing.hides_minimap);
        assert!(packing.right < viewport.x);
    }

    #[test]
    fn a_commandless_building_does_not_reserve_an_empty_card_row() {
        let viewport = vec2(1280.0, 800.0);
        let minimap = minimap_rect_scaled(40, 24, viewport, 1.0);
        let packing = panel_packing(viewport, minimap, 1.0, 0, 0);
        let actions = action_panel_rect(packing, 228.0, 228.0, false);
        assert_eq!(actions.w, 0.0);
        assert_eq!(actions.h, 0.0);
        let info = selection_info_rect(viewport, 228.0, 218.0, actions);
        assert_eq!(info.y, 582.0);
    }
}
