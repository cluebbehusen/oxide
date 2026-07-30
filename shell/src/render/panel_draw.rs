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
    let combat_h = if panel.combat.is_empty() {
        0.0
    } else {
        (20.0 + 15.0 * panel.combat.len() as f32) * s
    };
    let band_h = (20.0 * s + combat_h + rows as f32 * (ch + 4.0 * s)).max(120.0 * s);
    let top = screen_height() - band_h;
    let used_cols = shown.min(per_row).max(1) as f32;
    let cards_w = (cards_x + used_cols * (cw + gap)).max(220.0 * s) + 6.0 * s;
    let combat_w = panel
        .combat
        .iter()
        .map(|line| measure_text(line, None, (12.0 * s) as u16, 1.0).width)
        .fold(0.0, f32::max);
    let band_w = cards_w.max((cards_x + combat_w + 12.0 * s).min(right));
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
            let source = match icon {
                CardIcon::Unit(kind) => sprites.unit(*kind, faction),
                CardIcon::Building(kind) => sprites.building(*kind, faction),
                CardIcon::Verb(v) => sprites.verb_icon(*v),
                CardIcon::Order { verb, .. } => sprites.verb_icon(*verb),
            };
            blit(dest, source, tint);
            return;
        };
        // The subject wears ITS OWN colors: an attack chip's victim is
        // not the panel owner's faction.
        let source = match subject {
            crate::panel::OrderSubject::Unit(kind, f) => sprites.unit(*kind, *f),
            crate::panel::OrderSubject::Building(kind, f) => sprites.building(*kind, *f),
        };
        let hull = if *ghost {
            Color::new(tint.r, tint.g, tint.b, tint.a * 0.7)
        } else {
            tint
        };
        blit(dest, source, hull);
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

    // Portrait block: sprite, name, status.
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
    draw_text(
        &panel.sub,
        12.0 * s,
        top + 106.0 * s,
        14.0 * s,
        TEXT_SECONDARY,
    );

    // A single unit publishes its static combat capability without a
    // hover. The model contains kind-level weapon facts only, never a
    // live target, current cooldown, or private order state.
    if !panel.combat.is_empty() {
        draw_text("COMBAT", cards_x, top + 15.0 * s, 10.0 * s, TEXT_SECONDARY);
        let max_width = (band_w - cards_x - 12.0 * s).max(40.0 * s);
        for (i, line) in panel.combat.iter().enumerate() {
            let mut font_size = 12.0 * s;
            let mut dims = measure_text(line, None, font_size as u16, 1.0);
            while dims.width > max_width && font_size > 8.0 * s {
                font_size -= 0.5 * s;
                dims = measure_text(line, None, font_size as u16, 1.0);
            }
            draw_text(
                line,
                cards_x,
                top + (31.0 + 15.0 * i as f32) * s,
                font_size,
                TEXT_PRIMARY,
            );
        }
    }

    // Command cards, wrapping into as many rows as the width demands.
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut cards = [(zero, CardAction::None); 16];
    let mut card_count = 0;
    for (i, card) in panel.cards.iter().take(16).enumerate() {
        let (row, col) = (i / per_row, i % per_row);
        let rect = Rect::new(
            cards_x + col as f32 * (cw + gap),
            top + 10.0 * s + combat_h + row as f32 * (ch + 4.0 * s),
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
            &panel.queue_label,
            8.0 * s,
            dock_top + 15.0 * s,
            13.0 * s,
            TEXT_SECONDARY,
        );
        let orders_dock = panel.queue_label.starts_with("orders");
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
            // The chip in progress wears the bright border, not just a
            // "(now)" hidden in its tooltip.
            let active = orders_dock && i == 0;
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
                dock_top + dock_h - 8.0 * s,
                13.0 * s,
                TEXT_SECONDARY,
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
    use crate::layout::TooltipSide;
    let s = ui_scale();
    // The hovered RECT is the anchor, not just the index: the orders
    // dock stacks upward from the band, so a tooltip pinned to the
    // band's top edge described chip 1 beside chip 8.
    let hovered = layout.cards[..layout.card_count]
        .iter()
        .enumerate()
        .find(|(_, (r, _))| r.w > 0.0 && r.contains(input.mouse))
        .and_then(|(i, (r, _))| panel.cards.get(i).map(|c| (c, *r, TooltipSide::Above)))
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
    for d in &card.desc {
        lines.push((d.clone(), TEXT_BODY));
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
