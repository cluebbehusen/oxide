//! The command band, the orders dock, and the hover tooltip — the
//! selection panel's entire drawn form. Geometry it publishes rides
//! the LayoutModel; the pure card model lives in crate::panel.

use super::*;

/// The panel's clickable geometry: cards, card count, queue slots,
/// queue count, band top.
type PanelGeometry = (
    [(Rect, crate::panel::CardAction); 16],
    usize,
    [(Rect, crate::panel::CardAction); 8],
    usize,
    f32,
    f32,
    Rect,
);

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
    // Cards stop short of the minimap on narrow windows; the band
    // itself hugs its content instead of spanning the screen — a
    // full-width bar left a dead stretch between the cards and the
    // minimap, which floats in the corner at its own size.
    let right = if mini.w > 0.0 {
        (mini.x - 8.0 * s).max(300.0 * s)
    } else {
        screen_width()
    };
    // Cards wrap instead of vanishing: a 640px window must keep every
    // command reachable, so the band grows taller as rows accumulate.
    let (cw, ch, gap) = (66.0 * s, 80.0 * s, 6.0 * s);
    let cards_x = 150.0 * s;
    let available = (right - cards_x).max(cw);
    let per_row = (((available + gap) / (cw + gap)).floor() as usize).max(1);
    let shown = panel.cards.len().min(16);
    let rows = shown.div_ceil(per_row).max(1);
    let band_h = (20.0 * s + rows as f32 * (ch + 4.0 * s)).max(120.0 * s);
    let top = screen_height() - band_h;
    let used_cols = shown.min(per_row).max(1) as f32;
    let band_w = (cards_x + used_cols * (cw + gap)).max(220.0 * s) + 6.0 * s;
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
    let icon_source = |icon: &CardIcon| match icon {
        CardIcon::Unit(kind) => sprites.unit(*kind, faction),
        CardIcon::Building(kind) => sprites.building(*kind, faction),
        CardIcon::Verb(v) => sprites.verb_icon(*v),
    };

    // Portrait block: sprite, name, status.
    let psize = 56.0 * s;
    draw_texture_ex(
        sprites.texture(),
        12.0 * s,
        top + 12.0 * s,
        WHITE,
        DrawTextureParams {
            dest_size: Some(vec2(psize, psize)),
            source: Some(icon_source(&panel.portrait)),
            ..Default::default()
        },
    );
    draw_text(&panel.title, 12.0 * s, top + 88.0 * s, 17.0 * s, BONE);
    draw_text(&panel.sub, 12.0 * s, top + 106.0 * s, 14.0 * s, BONE_FAINT);

    // Command cards, wrapping into as many rows as the width demands.
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut cards = [(zero, CardAction::None); 16];
    let mut card_count = 0;
    for (i, card) in panel.cards.iter().take(16).enumerate() {
        let (row, col) = (i / per_row, i % per_row);
        let rect = Rect::new(
            cards_x + col as f32 * (cw + gap),
            top + 10.0 * s + row as f32 * (ch + 4.0 * s),
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
            draw_texture_ex(
                sprites.texture(),
                rect.x + (rect.w - isz) * 0.5,
                rect.y + 6.0 * s,
                tint,
                DrawTextureParams {
                    dest_size: Some(vec2(isz, isz)),
                    source: Some(icon_source(&card.icon)),
                    ..Default::default()
                },
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
            if card.enabled { BONE } else { BONE_FAINT },
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
                    BONE_FAINT
                },
            );
        }
        if !card.hotkey.is_empty() {
            draw_text(
                &card.hotkey,
                rect.x + 3.0 * s,
                rect.y + 13.0 * s,
                12.0 * s,
                BONE_FAINT,
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
        let (qw, qgap) = (44.0 * s, 4.0 * s);
        let label_h = 22.0 * s;
        // The dock lives between the top bar and the band; at a small
        // window a full queue would climb off the screen, so chips that
        // don't fit fold into a "+N" line instead of vanishing upward.
        let avail = (top - 54.0 * s - label_h).max(qw + qgap);
        let max_fit = ((avail / (qw + qgap)).floor() as usize).max(1);
        let n = panel.queue.len().min(8).min(max_fit);
        let hidden = panel.queue.len().min(8) - n;
        let more_h = if hidden > 0 { 16.0 * s } else { 0.0 };
        let dock_h = label_h + n as f32 * (qw + qgap) + more_h + 6.0 * s;
        let dock_w = qw + 16.0 * s;
        let dock_top = top - dock_h;
        dock = Rect::new(0.0, dock_top, dock_w, dock_h);
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
            panel.queue_label,
            8.0 * s,
            dock_top + 15.0 * s,
            13.0 * s,
            BONE_FAINT,
        );
        for (i, card) in panel.queue.iter().take(n).enumerate() {
            let rect = Rect::new(8.0 * s, dock_top + label_h + i as f32 * (qw + qgap), qw, qw);
            let hovered = rect.contains(input.mouse);
            draw_rectangle(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                Color::new(0.14, 0.14, 0.18, 1.0),
            );
            draw_rectangle_lines(
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                1.2 * s,
                if hovered {
                    BONE
                } else {
                    Color::new(0.45, 0.45, 0.52, 0.8)
                },
            );
            {
                let isz = 34.0 * s;
                draw_texture_ex(
                    sprites.texture(),
                    rect.x + (rect.w - isz) * 0.5,
                    rect.y + 5.0 * s,
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(isz, isz)),
                        source: Some(icon_source(&card.icon)),
                        ..Default::default()
                    },
                );
            }
            // The head of a production queue wears its progress.
            if i == 0
                && let Some(bid) = game.selection.building
                && let Some(building) = game.state.building(bid)
                && let Some(&kind) = building.queue.front()
            {
                let total = kind.stats().train_ticks.max(1);
                let frac = (building.progress as f32 / total as f32).clamp(0.0, 1.0);
                draw_rectangle(
                    rect.x,
                    rect.y + rect.h - 3.0 * s,
                    rect.w * frac,
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
                dock_top + dock_h - 8.0 * s,
                13.0 * s,
                BONE_FAINT,
            );
        }
    }
    (
        cards,
        card_count,
        queue_slots,
        queue_count,
        top,
        band_w,
        dock,
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
    let s = ui_scale();
    let hovered = layout.cards[..layout.card_count]
        .iter()
        .enumerate()
        .find(|(_, (r, _))| r.w > 0.0 && r.contains(input.mouse))
        .and_then(|(i, _)| panel.cards.get(i))
        .or_else(|| {
            layout.queue_slots[..layout.queue_count]
                .iter()
                .enumerate()
                .find(|(_, (r, _))| r.w > 0.0 && r.contains(input.mouse))
                .and_then(|(i, _)| panel.queue.get(i))
        });
    let Some(card) = hovered else {
        return;
    };
    let mut lines: Vec<(String, Color)> = Vec::new();
    let header = if card.hotkey.is_empty() {
        card.title.clone()
    } else {
        format!("{}   [{}]", card.title, card.hotkey)
    };
    lines.push((header, BONE));
    if let Some(cost) = card.cost {
        lines.push((format!("{cost} scrap"), SCRAP_COLOR));
    }
    for d in &card.desc {
        lines.push((d.clone(), BONE_FAINT));
    }
    if let Some(why) = &card.why {
        lines.push((why.clone(), DANGER));
    }
    let size = 15.0 * s;
    let pad = 8.0 * s;
    let width = lines
        .iter()
        .map(|(l, _)| measure_text(l, None, size as u16, 1.0).width)
        .fold(0.0f32, f32::max)
        + pad * 2.0;
    let line_h = 18.0 * s;
    let height = lines.len() as f32 * line_h + pad * 1.5;
    let x = input.mouse.x.min(screen_width() - width - 4.0 * s).max(0.0);
    let y = layout.panel_top - height - 6.0 * s;
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
