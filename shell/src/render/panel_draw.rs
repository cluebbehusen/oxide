//! The command band, the orders dock, and the hover tooltip — the
//! selection panel's entire drawn form. Geometry it publishes rides
//! the LayoutModel; the pure card model lives in crate::panel.

use super::*;

/// The panel's clickable geometry: cards, card count, queue slots,
/// queue count, band bounds, orders dock, and whether the compact
/// layout had to yield the minimap.
type PanelGeometry = (
    [(Rect, crate::panel::CardAction); 8],
    usize,
    [(Rect, crate::panel::CardAction); 16],
    usize,
    [(Rect, crate::panel::CardAction); 8],
    usize,
    f32,
    f32,
    Rect,
    bool,
);

#[derive(Debug, Clone, Copy, PartialEq)]
struct PanelPacking {
    right: f32,
    available: f32,
    per_row: usize,
    roster_shown: usize,
    roster_per_row: usize,
    roster_h: f32,
    capabilities_shown: usize,
    capabilities_h: f32,
    band_h: f32,
    top: f32,
    hides_minimap: bool,
}

#[allow(clippy::too_many_arguments)]
fn panel_packing_at_right(
    viewport: Vec2,
    scale: f32,
    right: f32,
    roster_shown: usize,
    cards_shown: usize,
    capabilities_len: usize,
    compact: bool,
    hides_minimap: bool,
) -> PanelPacking {
    let cards_x = if compact { 210.0 } else { 150.0 } * scale;
    let (card_w, card_h, gap) = (66.0 * scale, 80.0 * scale, 6.0 * scale);
    let available = (right - cards_x).max(card_w);
    let per_row = (((available + gap) / (card_w + gap)).floor() as usize).max(1);
    let (roster_w, roster_h, roster_gap) = (64.0 * scale, 70.0 * scale, 5.0 * scale);
    let roster_per_row =
        (((available + roster_gap) / (roster_w + roster_gap)).floor() as usize).max(1);
    let roster_rows = roster_shown.div_ceil(roster_per_row);
    let roster_h = if roster_shown == 0 {
        0.0
    } else {
        22.0 * scale + roster_rows as f32 * (roster_h + 5.0 * scale)
    };
    let cards_h = if cards_shown == 0 {
        0.0
    } else {
        grouped_card_rows(cards_shown, per_row) as f32 * (card_h + 4.0 * scale)
    };
    let capabilities_h = if capabilities_len == 0 {
        0.0
    } else {
        (22.0 + 18.0 * capabilities_len as f32) * scale
    };
    let minimum_h = if compact { 72.0 } else { 120.0 } * scale;
    let band_h = (20.0 * scale + capabilities_h + roster_h + cards_h).max(minimum_h);
    PanelPacking {
        right,
        available,
        per_row,
        roster_shown,
        roster_per_row,
        roster_h,
        capabilities_shown: capabilities_len,
        capabilities_h,
        band_h,
        top: viewport.y - band_h,
        hides_minimap,
    }
}

fn panel_packing(
    viewport: Vec2,
    minimap: Rect,
    scale: f32,
    roster_len: usize,
    cards_len: usize,
    capabilities_len: usize,
    compact: bool,
) -> PanelPacking {
    let roster_shown = roster_len.min(8);
    let cards_shown = cards_len.min(16);
    let reserved_right = if minimap.w > 0.0 {
        (minimap.x - 8.0 * scale).max(300.0 * scale).min(viewport.x)
    } else {
        viewport.x
    };
    let max_band_h = (viewport.y - crate::layout::TOP_BAR_H * scale).max(0.0);
    let mut packing = panel_packing_at_right(
        viewport,
        scale,
        reserved_right,
        roster_shown,
        cards_shown,
        capabilities_len,
        compact,
        false,
    );
    if packing.band_h > max_band_h && reserved_right < viewport.x {
        packing = panel_packing_at_right(
            viewport,
            scale,
            viewport.x,
            roster_shown,
            cards_shown,
            capabilities_len,
            compact,
            true,
        );
    }
    if packing.band_h > max_band_h && packing.roster_shown > 0 {
        packing = panel_packing_at_right(
            viewport,
            scale,
            viewport.x,
            0,
            cards_shown,
            capabilities_len,
            compact,
            true,
        );
    }
    if packing.band_h > max_band_h && packing.capabilities_shown > 0 {
        packing = panel_packing_at_right(
            viewport,
            scale,
            viewport.x,
            0,
            cards_shown,
            0,
            compact,
            true,
        );
    }
    packing
}

fn grouped_card_slot(index: usize, rally_count: usize, per_row: usize) -> (usize, usize, bool) {
    let per_row = per_row.max(1);
    let row = index / per_row;
    let column = index % per_row;
    let boundary_row = rally_count / per_row;
    let follows_rally_in_same_row = rally_count > 0
        && !rally_count.is_multiple_of(per_row)
        && index >= rally_count
        && row == boundary_row;
    (row, column, follows_rally_in_same_row)
}

fn grouped_card_rows(shown: usize, per_row: usize) -> usize {
    shown.div_ceil(per_row.max(1)).max(1)
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
    if rally_count == 0 || rally_count >= shown || rally_count.is_multiple_of(per_row) {
        return 0.0;
    }
    let boundary_row_start = rally_count / per_row * per_row;
    let cards_in_boundary_row = (shown - boundary_row_start).min(per_row);
    let ordinary_width = cards_in_boundary_row as f32 * card_width
        + cards_in_boundary_row.saturating_sub(1) as f32 * ordinary_gap;
    (available - ordinary_width).clamp(0.0, 10.0 * card_width / 66.0)
}

fn panel_sub_lines(sub: &str) -> Vec<String> {
    sub.split_once(" | speed ").map_or_else(
        || vec![sub.to_string()],
        |(detail, speed)| vec![detail.to_string(), format!("speed {speed}")],
    )
}

/// Packs every visible queue chip above the command band. A single
/// column stays pleasantly quiet when it fits; a full eight-slot
/// production queue becomes a 2×4 dock at the 640×400 contract instead
/// of hiding paid, cancelable work behind a "+4" label.
fn queue_grid(queue_len: usize, panel_top: f32, scale: f32) -> (Rect, [Rect; 8], usize) {
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut slots = [zero; 8];
    let count = queue_len.min(slots.len());
    if count == 0 {
        return (zero, slots, 0);
    }
    let (size, gap) = (44.0 * scale, 4.0 * scale);
    let label_h = 22.0 * scale;
    let available = (panel_top - 54.0 * scale - label_h).max(size + gap);
    let max_rows = ((available / (size + gap)).floor() as usize).max(1);
    let columns = count.div_ceil(max_rows).max(1);
    let rows = count.div_ceil(columns);
    let width = 16.0 * scale + columns as f32 * size + columns.saturating_sub(1) as f32 * gap;
    let height = label_h + rows as f32 * (size + gap) + 6.0 * scale;
    let dock = Rect::new(0.0, panel_top - height, width, height);
    for (index, slot) in slots.iter_mut().take(count).enumerate() {
        let row = index / columns;
        let column = index % columns;
        *slot = Rect::new(
            8.0 * scale + column as f32 * (size + gap),
            dock.y + label_h + row as f32 * (size + gap),
            size,
            size,
        );
    }
    (dock, slots, count)
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
    let compact = matches!(panel.portrait, CardIcon::Building(_))
        && panel.cards.is_empty()
        && panel.roster.is_empty();
    let packing = panel_packing(
        vec2(screen_width(), screen_height()),
        mini,
        s,
        panel.roster.len(),
        panel.cards.len(),
        panel.capabilities.len(),
        compact,
    );
    let right = packing.right;
    // Cards wrap instead of vanishing. If reserving the minimap would
    // push the panel through the top bar, the command surface takes the
    // width for this frame; only a still-overfull palette temporarily
    // yields the mixed roster.
    let (cw, ch, gap) = (66.0 * s, 80.0 * s, 6.0 * s);
    let cards_x = if compact { 210.0 } else { 150.0 } * s;
    let available = packing.available;
    let per_row = packing.per_row;
    let roster_shown = packing.roster_shown;
    let (rw, rh, roster_gap) = (64.0 * s, 70.0 * s, 5.0 * s);
    let roster_per_row = packing.roster_per_row;
    let roster_h = packing.roster_h;
    let shown = panel.cards.len().min(16);
    let rally_shown = panel.cards[..shown]
        .iter()
        .take_while(|card| matches!(card.action, CardAction::ArmRally | CardAction::ClearRally))
        .count();
    let section_gap = grouped_card_gap(shown, rally_shown, per_row, available, cw, gap);
    let capabilities_shown = packing.capabilities_shown;
    let capabilities_h = packing.capabilities_h;
    let band_h = packing.band_h;
    let top = packing.top;
    let cards_w = if shown == 0 {
        cards_x
    } else {
        let used_cols = shown.min(per_row).max(1) as f32;
        (cards_x + used_cols * (cw + gap)).max(220.0 * s) + 6.0 * s
    };
    let capabilities_w = panel
        .capabilities
        .iter()
        .take(capabilities_shown)
        .map(|fact| measure_text(&fact.text, None, (13.0 * s) as u16, 1.0).width + 24.0 * s)
        .fold(0.0, f32::max);
    let band_w = cards_w.max((cards_x + capabilities_w + 12.0 * s).min(right));
    // Opaque, unlike the translucent HUD panels: machines drifting
    // beneath the band would ghost through the cards. Top and right
    // edges get the same line — a content-width band needs a corner,
    // not a fill that falls off mid-screen.
    draw_rectangle(0.0, top, band_w, band_h, Color::from_rgba(20, 20, 24, 255));
    draw_rectangle(0.0, top, band_w, 1.5 * s, Color::new(0.6, 0.6, 0.65, 0.4));
    draw_rectangle(
        band_w - 1.5 * s,
        top,
        1.5 * s,
        band_h,
        Color::new(0.6, 0.6, 0.65, 0.4),
    );

    // The panel says whose colors it wears: an inspected ally or
    // enemy draws in its owner's faction, not the viewer's. Own
    // panels carry the human's faction, so roster cards stay right.
    let faction = panel.faction;
    let blit = |dest: Rect, source: Rect, tint: Color| {
        draw_texture_ex(
            sprites.texture(),
            dest.x,
            dest.y,
            tint,
            DrawTextureParams {
                dest_size: Some(vec2(dest.w, dest.h)),
                source: Some(source),
                ..Default::default()
            },
        );
    };
    // Defense art is authored as a base plus a north-facing live mount.
    // Static cards compose the same silhouette without inventing aim.
    let blit_building =
        |dest: Rect, kind: oxide_sim::BuildingKind, faction: oxide_sim::Faction, tint: Color| {
            blit(dest, sprites.building(kind, faction), tint);
            if let Some(mount) = sprites.defense_mount(kind, faction) {
                blit(dest, mount, tint);
            }
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
                CardIcon::Unit(kind) => blit(dest, sprites.unit(*kind, faction), tint),
                CardIcon::Building(kind) => blit_building(dest, *kind, faction, tint),
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
        match subject {
            crate::panel::OrderSubject::Unit(kind, f) => {
                blit(dest, sprites.unit(*kind, *f), hull);
            }
            crate::panel::OrderSubject::Building(kind, f) => {
                blit(dest, sprites.building(*kind, *f), hull);
                // A construction ghost is still a bare foundation under
                // scaffold; live targets and finished orders keep the
                // complete defense silhouette.
                if !*ghost && let Some(mount) = sprites.defense_mount(*kind, *f) {
                    blit(dest, mount, hull);
                }
            }
        }
        if *ghost {
            // The sparse lattice, not the dense one the world opens
            // with: at chip size a full scaffold reads as noise over
            // the silhouette, and the chip's meter already carries
            // how far the site has come.
            blit(
                dest,
                sprites.scaffold(false),
                Color::new(tint.r, tint.g, tint.b, tint.a * 0.45),
            );
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

    // Portrait block: commandless buildings place their labels beside a
    // smaller portrait, reclaiming the empty card row below them.
    if compact {
        let psize = 42.0 * s;
        draw_icon(
            Rect::new(10.0 * s, top + 14.0 * s, psize, psize),
            &panel.portrait,
            WHITE,
        );
        let text_x = 62.0 * s;
        let max_width = cards_x - text_x - 8.0 * s;
        let mut title_size = 15.0 * s;
        while measure_text(&panel.title, None, title_size as u16, 1.0).width > max_width
            && title_size > 10.0 * s
        {
            title_size -= 0.5 * s;
        }
        let mut sub_size = 12.0 * s;
        while measure_text(&panel.sub, None, sub_size as u16, 1.0).width > max_width
            && sub_size > 8.0 * s
        {
            sub_size -= 0.5 * s;
        }
        draw_text(
            &panel.title,
            text_x,
            top + 31.0 * s,
            title_size,
            TEXT_PRIMARY,
        );
        draw_text(&panel.sub, text_x, top + 50.0 * s, sub_size, TEXT_SECONDARY);
    } else {
        let psize = 56.0 * s;
        draw_icon(
            Rect::new(12.0 * s, top + 12.0 * s, psize, psize),
            &panel.portrait,
            WHITE,
        );
        draw_text(
            &panel.title,
            12.0 * s,
            top + 88.0 * s,
            17.0 * s,
            TEXT_PRIMARY,
        );
        let sub_lines = panel_sub_lines(&panel.sub);
        let max_width = cards_x - 24.0 * s;
        let base_size = if sub_lines.len() > 1 { 12.0 } else { 14.0 } * s;
        for (index, line) in sub_lines.iter().enumerate() {
            let mut size = base_size;
            while measure_text(line, None, size as u16, 1.0).width > max_width && size > 8.0 * s {
                size -= 0.5 * s;
            }
            draw_text(
                line,
                12.0 * s,
                top + (if sub_lines.len() > 1 { 103.0 } else { 106.0 }) * s
                    + index as f32 * 14.0 * s,
                size,
                TEXT_SECONDARY,
            );
        }
    }

    // A single entity publishes its static capability without a
    // hover. The model contains kind-level capability facts only,
    // never a live target, current cooldown, or private order state.
    if capabilities_shown > 0 {
        draw_text(
            "CAPABILITIES",
            cards_x,
            top + 15.0 * s,
            10.0 * s,
            TEXT_SECONDARY,
        );
        let max_width = (band_w - cards_x - 12.0 * s).max(40.0 * s);
        for (i, fact) in panel
            .capabilities
            .iter()
            .take(capabilities_shown)
            .enumerate()
        {
            let y = top + (34.0 + 18.0 * i as f32) * s;
            let icon_color = capability_icon_color(fact.icon);
            draw_capability_icon(
                vec2(cards_x + 7.0 * s, y - 4.5 * s),
                5.2 * s,
                fact.icon,
                icon_color,
                false,
            );
            let text_x = cards_x + 20.0 * s;
            let mut font_size = 13.0 * s;
            let mut dims = measure_text(&fact.text, None, font_size as u16, 1.0);
            while dims.width > max_width - 20.0 * s && font_size > 9.0 * s {
                font_size -= 0.5 * s;
                dims = measure_text(&fact.text, None, font_size as u16, 1.0);
            }
            draw_text(&fact.text, text_x, y, font_size, TEXT_PRIMARY);
        }
    }

    // Mixed-selection roster, visually and geometrically separate from
    // verbs. It reads as "what is in my hand" before "what can it do".
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut roster_slots = [(zero, CardAction::None); 8];
    let mut roster_count = 0;
    if roster_shown > 0 {
        let label_y = top + capabilities_h + 15.0 * s;
        draw_text("SELECTED UNITS", cards_x, label_y, 12.0 * s, TEXT_SECONDARY);
        for (index, card) in panel.roster.iter().take(roster_shown).enumerate() {
            let (row, column) = (index / roster_per_row, index % roster_per_row);
            let rect = Rect::new(
                cards_x + column as f32 * (rw + roster_gap),
                top + capabilities_h + 22.0 * s + row as f32 * (rh + 5.0 * s),
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
                    Color::new(0.28, 0.28, 0.33, 1.0)
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
            let icon_size = 42.0 * s;
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
            let mut size = 11.0 * s;
            let mut dims = measure_text(&card.title, None, size as u16, 1.0);
            while dims.width > rect.w - 6.0 * s && size > 8.0 * s {
                size -= 0.5 * s;
                dims = measure_text(&card.title, None, size as u16, 1.0);
            }
            draw_text(
                &card.title,
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
    for (i, card) in panel.cards.iter().take(16).enumerate() {
        let (row, col, after_rally) = grouped_card_slot(i, rally_shown, per_row);
        let rect = Rect::new(
            cards_x + col as f32 * (cw + gap) + if after_rally { section_gap } else { 0.0 },
            top + 10.0 * s + capabilities_h + roster_h + row as f32 * (ch + 4.0 * s),
            cw,
            ch,
        );
        let hovered = rect.contains(input.mouse);
        let bg = if hovered && card.enabled {
            Color::new(0.28, 0.28, 0.33, 1.0)
        } else {
            Color::new(0.16, 0.16, 0.20, 1.0)
        };
        draw_rectangle(rect.x, rect.y, rect.w, rect.h, bg);
        let border = if !card.enabled {
            Color::new(0.4, 0.4, 0.45, 0.5)
        } else if hovered {
            BONE
        } else {
            Color::new(0.55, 0.55, 0.62, 0.9)
        };
        draw_rectangle_lines(rect.x, rect.y, rect.w, rect.h, 1.5 * s, border);
        let tint = if card.enabled {
            WHITE
        } else {
            Color::new(1.0, 1.0, 1.0, 0.35)
        };
        {
            let isz = 42.0 * s;
            draw_icon(
                Rect::new(rect.x + (rect.w - isz) * 0.5, rect.y + 6.0 * s, isz, isz),
                &card.icon,
                tint,
            );
        }
        // The name lives on the card, not only in the tooltip — and it
        // stays whole: a long name shrinks to fit instead of losing its
        // tail ("fabricato", "flak turr").
        let mut nsize = 12.0 * s;
        let mut ndims = measure_text(&card.title, None, nsize as u16, 1.0);
        while ndims.width > rect.w - 4.0 * s && nsize > 8.0 * s {
            nsize -= 1.0;
            ndims = measure_text(&card.title, None, nsize as u16, 1.0);
        }
        draw_text(
            &card.title,
            rect.x + (rect.w - ndims.width) * 0.5,
            rect.y + rect.h - 17.0 * s,
            nsize,
            if card.enabled {
                TEXT_PRIMARY
            } else {
                TEXT_DISABLED
            },
        );
        if let Some(cost) = card.cost {
            let label = format!("{cost}");
            let dims = measure_text(&label, None, (14.0 * s) as u16, 1.0);
            draw_text(
                &label,
                rect.x + (rect.w - dims.width) * 0.5,
                rect.y + rect.h - 5.0 * s,
                14.0 * s,
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
                rect.x + 3.0 * s,
                rect.y + 13.0 * s,
                12.0 * s,
                TEXT_SECONDARY,
            );
        }
        if let (CardAction::Dispatch(crate::action::Action::TrainSlot(_)), CardIcon::Unit(kind)) =
            (card.action, card.icon)
        {
            let label = crate::panel::unit_train_time_label(kind);
            let dims = measure_text(&label, None, (11.0 * s) as u16, 1.0);
            draw_text(
                &label,
                rect.x + rect.w - dims.width - 3.0 * s,
                rect.y + 13.0 * s,
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
        let (mut grid_dock, grid_slots, n) = queue_grid(panel.queue.len(), top, s);
        let queue_label_width =
            measure_text(&panel.queue_label, None, (13.0 * s) as u16, 1.0).width + 16.0 * s;
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
            let active = i == 0;
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
                    Rect::new(rect.x + (rect.w - isz) * 0.5, rect.y + 5.0 * s, isz, isz),
                    &card.icon,
                    WHITE,
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
    (
        roster_slots,
        roster_count,
        cards,
        card_count,
        queue_slots,
        queue_count,
        top,
        band_w,
        dock,
        packing.hides_minimap,
    )
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
    let size = 15.0 * s;
    let pad = 8.0 * s;
    // Descriptions run to two sentences; the box wraps them at a
    // reading width instead of growing to the longest line, which
    // once put a Skyhook tooltip wider than the window.
    let wrap_w = 340.0 * s;
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
    let width = lines
        .iter()
        .map(|(l, _)| measure_text(l, None, size as u16, 1.0).width)
        .fold(0.0f32, f32::max)
        + pad * 2.0;
    let line_h = 18.0 * s;
    let height = lines.len() as f32 * line_h + pad * 1.5;
    // The box's room is the window BETWEEN the top bar and the band:
    // a tooltip that spilled over the command cards would cover what
    // the hand is about to click next.
    let origin = crate::layout::tooltip_origin(
        anchor,
        vec2(width, height),
        side,
        vec2(screen_width(), layout.panel_top.min(screen_height())),
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
        Color::new(0.55, 0.55, 0.62, 0.9),
    );
    for (i, (line, color)) in lines.iter().enumerate() {
        draw_text(
            line,
            x + pad,
            y + pad + (i as f32 + 0.6) * line_h,
            size,
            *color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rally_cards_get_a_compact_visual_break_before_production() {
        assert_eq!(grouped_card_rows(7, 5), 2);
        assert_eq!(grouped_card_slot(0, 2, 5), (0, 0, false));
        assert_eq!(grouped_card_slot(1, 2, 5), (0, 1, false));
        assert_eq!(grouped_card_slot(2, 2, 5), (0, 2, true));
        assert_eq!(grouped_card_slot(4, 2, 5), (0, 4, true));
        assert_eq!(grouped_card_slot(5, 2, 5), (1, 0, false));
        assert_eq!(grouped_card_slot(6, 2, 5), (1, 1, false));
        assert_eq!(grouped_card_gap(7, 2, 5, 400.0, 66.0, 6.0), 10.0);
        assert_eq!(grouped_card_gap(7, 2, 5, 354.0, 66.0, 6.0), 0.0);

        assert_eq!(grouped_card_rows(7, 5), 2);
        assert_eq!(grouped_card_slot(5, 0, 5), (1, 0, false));
    }

    #[test]
    fn eight_paid_slots_form_a_fully_clickable_small_window_dock() {
        // A 120px command band leaves panel_top=280 at the supported
        // 640×400 floor. Every slot must remain present, above the band,
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
        let packing = panel_packing(viewport, minimap, 1.0, 8, 16, 5, false);

        assert!(packing.hides_minimap);
        assert_eq!(packing.right, viewport.x);
        assert_eq!(packing.roster_shown, 0, "the open palette gets the room");
        assert_eq!(
            packing.capabilities_shown, 0,
            "cards stay reachable before facts"
        );
        assert!(packing.top >= crate::layout::TOP_BAR_H);
        assert!(packing.top + packing.band_h <= viewport.y);
    }

    #[test]
    fn a_mixed_small_window_selection_keeps_its_large_roster() {
        let viewport = vec2(640.0, 400.0);
        let minimap = minimap_rect_scaled(40, 24, viewport, 1.0);
        let packing = panel_packing(viewport, minimap, 1.0, 8, 6, 0, false);

        assert!(packing.hides_minimap);
        assert_eq!(packing.roster_shown, 8);
        assert!(packing.top >= crate::layout::TOP_BAR_H);
        assert!(packing.top + packing.band_h <= viewport.y);
        let (dock, slots, count) = queue_grid(8, packing.top, 1.0);
        assert_eq!(count, 8);
        assert!(dock.y >= crate::layout::TOP_BAR_H);
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
        let packing = panel_packing(viewport, minimap, 1.0, 0, 2, 0, false);

        assert!(!packing.hides_minimap);
        assert!(packing.right < viewport.x);
    }

    #[test]
    fn a_commandless_building_does_not_reserve_an_empty_card_row() {
        let viewport = vec2(1280.0, 800.0);
        let minimap = minimap_rect_scaled(40, 24, viewport, 1.0);
        let compact = panel_packing(viewport, minimap, 1.0, 0, 0, 1, true);
        let ordinary = panel_packing(viewport, minimap, 1.0, 0, 0, 1, false);

        assert_eq!(compact.band_h, 72.0);
        assert_eq!(ordinary.band_h, 120.0);
        assert_eq!(compact.top, viewport.y - 72.0);
    }

    #[test]
    fn unit_speed_breaks_onto_a_second_portrait_line() {
        assert_eq!(
            panel_sub_lines("60/60 hp | speed 2.5 tiles/sec"),
            ["60/60 hp", "speed 2.5 tiles/sec"]
        );
        assert_eq!(
            panel_sub_lines("hostile | Standard / Balanced AI | 60/60 hp | speed 3.1 tiles/sec"),
            [
                "hostile | Standard / Balanced AI | 60/60 hp",
                "speed 3.1 tiles/sec"
            ]
        );
        assert_eq!(panel_sub_lines("3 types"), ["3 types"]);
    }
}
