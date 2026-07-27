//! Standing chrome and overlays: the top bar with its controls hint,
//! toasts, the salvage hover tooltip, the omniscient debug overlay,
//! and the endgame verdict. The LayoutModel publish rides in the hud
//! so drawn and clickable can never disagree.

use super::*;

/// Hovered salvage says what it holds: live amounts on visible ground,
/// remembered amounts under the dim — the same memory rule as every
/// renderer, so the tooltip can't leak what fog took back.
pub(crate) fn draw_salvage_tooltip(game: &Game, input: &InputState) {
    if game.layout.get().chrome_owns(input.mouse) {
        return;
    }
    let world = game.camera.to_world(input.mouse);
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    let vision = game.my_vision();
    if !vision.explored(tile) && !game.all_seeing() {
        return;
    }
    let (scrap, wreck) = if vision.visible(tile) || game.all_seeing() {
        (
            game.state.map().scrap_at(tile),
            game.state.map().wreck_at(tile),
        )
    } else {
        (vision.remembered_scrap(tile), vision.remembered_wreck(tile))
    };
    let text = match (scrap > 0, wreck > 0) {
        (true, _) => format!("scrap {scrap}"),
        (_, true) => format!("wreck {wreck}"),
        _ => return,
    };
    let s = ui_scale();
    let dims = measure_text(&text, None, (16.0 * s) as u16, 1.0);
    let (x, y) = (input.mouse.x + 14.0 * s, input.mouse.y - 10.0 * s);
    draw_rectangle(
        x - 4.0 * s,
        y - 14.0 * s,
        dims.width + 8.0 * s,
        20.0 * s,
        PANEL,
    );
    draw_text(&text, x, y, 16.0 * s, SCRAP_COLOR);
}

pub(crate) fn draw_overlay(game: &Game, alpha: f32) {
    let (min, max) = visible_tiles(game);
    for x in min.x..=max.x {
        let a = game.camera.to_screen(vec2(x as f32, min.y as f32));
        let b = game.camera.to_screen(vec2(x as f32, max.y as f32));
        draw_line(a.x, a.y, b.x, b.y, 1.0, BONE_FAINT);
    }
    for y in min.y..=max.y {
        let a = game.camera.to_screen(vec2(min.x as f32, y as f32));
        let b = game.camera.to_screen(vec2(max.x as f32, y as f32));
        draw_line(a.x, a.y, b.x, b.y, 1.0, BONE_FAINT);
    }
    for unit in game.state.units() {
        let pos = game.draw_pos(unit.id, unit.pos, alpha);
        let screen = game.camera.to_screen(pos);
        draw_text(
            format!("u{} {}hp", unit.id.0, unit.hp),
            screen.x + 8.0,
            screen.y - 8.0,
            16.0,
            BONE,
        );
        if let Some(path) = &unit.path {
            let mut previous = screen;
            for waypoint in path.waypoints.iter().skip(path.next as usize) {
                let next = game
                    .camera
                    .to_screen(vec2(waypoint.x as f32 + 0.5, waypoint.y as f32 + 0.5));
                draw_line(previous.x, previous.y, next.x, next.y, 1.0, BONE_FAINT);
                previous = next;
            }
        }
    }
    let info = format!(
        "tick {}  fps {}  zoom {:.0}  center ({:.1},{:.1})",
        game.state.current_tick(),
        get_fps(),
        game.camera.zoom,
        game.camera.center.x,
        game.camera.center.y,
    );
    let s = ui_scale();
    draw_text(&info, screen_width() - 420.0 * s, 54.0 * s, 18.0 * s, BONE);
}

pub(crate) fn draw_hud(game: &Game, sprites: &Sprites, input: &InputState) {
    let s = ui_scale();
    // A spectator commands nothing: no bank, no unit count, no idle
    // nag — the viewer's transport bar is its own chrome. The layout
    // still publishes below so the minimap stays clickable.
    let mut idle_badge = Rect::new(0.0, 0.0, 0.0, 0.0);
    if !game.spectate {
        // Top bar.
        draw_rectangle(
            0.0,
            0.0,
            screen_width(),
            crate::layout::TOP_BAR_H * s,
            PANEL,
        );
        let me = game.state.player(game.human);
        let my_units = game
            .state
            .units()
            .iter()
            .filter(|u| u.player == game.human)
            .count();
        draw_text(
            format!("SCRAP {}", me.scrap),
            12.0 * s,
            22.0 * s,
            22.0 * s,
            SCRAP_COLOR,
        );
        draw_text(
            format!("UNITS {my_units}"),
            150.0 * s,
            22.0 * s,
            22.0 * s,
            BONE,
        );
        // Idle harvesters are money on the ground; the badge nags in
        // danger red and clicking it (or N) cycles through them. Tick
        // count is a debug fact; it rides the F1 overlay, not the
        // player's bar.
        let idle = crate::input::idle_harvesters(game).len();
        if idle > 0 {
            let label = format!("IDLE {idle}");
            let dims = measure_text(&label, None, (22.0 * s) as u16, 1.0);
            let x = 270.0 * s;
            draw_text(&label, x, 22.0 * s, 22.0 * s, DANGER);
            idle_badge = Rect::new(x - 4.0 * s, 4.0 * s, dims.width + 8.0 * s, 26.0 * s);
        }
        if game.paused {
            draw_text("PAUSED (P)", 360.0 * s, 22.0 * s, 22.0 * s, DANGER);
        } else if (game.speed - 1.0).abs() > f64::EPSILON {
            draw_text(
                format!("SPEED x{:.2}", game.speed),
                360.0 * s,
                22.0 * s,
                22.0 * s,
                SCRAP_COLOR,
            );
        }
        // Controls coaching fills the bar's empty right half, dropping
        // trailing segments when a narrow window runs out of room. Live
        // chords, not folklore: a rebound key changes the prompt.
        use crate::action::{Action, BindingMap};
        let label = |a: Action| {
            input
                .bindings
                .chord_for(a)
                .map(BindingMap::chord_label)
                .unwrap_or_else(|| "unbound".to_string())
        };
        let pans = [
            Action::PanLeft,
            Action::PanRight,
            Action::PanUp,
            Action::PanDown,
        ]
        .map(label);
        let pan = if pans == ["Left", "Right", "Up", "Down"].map(String::from) {
            "arrows pan".to_string()
        } else {
            format!("{}/{}/{}/{} pan", pans[0], pans[1], pans[2], pans[3])
        };
        let segments = [
            "LMB select".to_string(),
            "RMB move/engage".to_string(),
            "1-9 train".to_string(),
            format!("{} build", label(Action::ToggleBuildPalette)),
            pan,
            "Esc menu".to_string(),
            format!("{} debug", label(Action::ToggleOverlay)),
        ];
        let max_w = screen_width() - 540.0 * s;
        let mut hint = String::new();
        for seg in segments {
            let candidate = if hint.is_empty() {
                seg
            } else {
                format!("{hint} · {seg}")
            };
            if measure_text(&candidate, None, (16.0 * s) as u16, 1.0).width > max_w {
                break;
            }
            hint = candidate;
        }
        if !hint.is_empty() {
            let width = measure_text(&hint, None, (16.0 * s) as u16, 1.0).width;
            draw_text(
                &hint,
                screen_width() - width - 10.0 * s,
                21.0 * s,
                16.0 * s,
                BONE_FAINT,
            );
        }
    }

    *game.panel_model.borrow_mut() = crate::panel::build(game, &input.bindings);
    let panel = game.panel_model.borrow();
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut cards = [(zero, crate::panel::CardAction::None); 16];
    let mut card_count = 0;
    let mut queue_slots = [(zero, crate::panel::CardAction::None); 8];
    let mut queue_count = 0;
    let mut panel_top = f32::INFINITY;
    let mut panel_right = 0.0;
    let mut orders_dock = Rect::new(0.0, 0.0, 0.0, 0.0);
    if let Some(panel) = panel.as_ref() {
        let (c, cc, q, qc, top, right, dock) = draw_panel(game, sprites, input, panel);
        cards = c;
        card_count = cc;
        queue_slots = q;
        queue_count = qc;
        panel_top = top;
        panel_right = right;
        orders_dock = dock;
    }
    // Publish the frame's chrome geometry — the model hit-testing reads.
    game.layout.set(crate::layout::LayoutModel::compute(
        vec2(screen_width(), screen_height()),
        s,
        panel_top,
        panel_right,
        orders_dock,
        minimap_rect(game),
        idle_badge,
        cards,
        card_count,
        queue_slots,
        queue_count,
    ));

    // Toasts: rejected orders and stalled units, newest at the bottom.
    for (i, toast) in game.toasts.iter().rev().take(3).enumerate() {
        let fade = (1.0 - (toast.age - 1.5).max(0.0)).clamp(0.0, 1.0);
        let y = screen_height() - (60.0 + 24.0 * i as f32) * s;
        let color = Color::new(0.92, 0.5, 0.45, fade);
        draw_text(&toast.text, 12.0 * s, y, 20.0 * s, color);
    }

    // Spectator strip: a foundry-less seat on a living team stays in
    // the match by design — masterless machines finish their orders and
    // the team plays on — but the human deserves to be told the seat
    // has no voice left. Commands still route; the sim rejects them.
    if game.state.result().is_none()
        && !game
            .state
            .buildings()
            .iter()
            .any(|b| b.player == game.human && b.kind == oxide_sim::BuildingKind::Foundry)
    {
        let text = "ELIMINATED - SPECTATING";
        let dims = measure_text(text, None, (24.0 * s) as u16, 1.0);
        let x = (screen_width() - dims.width) * 0.5;
        draw_rectangle(
            x - 12.0 * s,
            40.0 * s,
            dims.width + 24.0 * s,
            30.0 * s,
            PANEL,
        );
        draw_text(text, x, 60.0 * s, 24.0 * s, DANGER);
    }
}

/// The endgame verdict, drawn over every other layer — at 640x400 the
/// old in-HUD version collided with the minimap and pushed its graph
/// off screen. Geometry clamps to the viewport.
pub(crate) fn draw_result_overlay(game: &Game) {
    let s = ui_scale();
    if let Some(result) = game.state.result() {
        // The human's verdict first — the game knows whose screen this
        // is; "FERROUS WINS" made every ending read like someone else's.
        let winners = game.state.winners();
        let (text, color) = match result {
            GameResult::Victory { .. } if winners.contains(&game.human) => {
                ("VICTORY".to_string(), SCRAP_COLOR)
            }
            GameResult::Victory { .. } => ("DEFEAT".to_string(), DANGER),
            GameResult::Draw => ("MUTUAL DESTRUCTION".to_string(), BONE_FAINT),
        };
        let sub = match result {
            GameResult::Victory { .. } => {
                let names: Vec<String> = winners
                    .into_iter()
                    .map(|p| game.state.player(p).name.to_uppercase())
                    .collect();
                format!("{} take the field", names.join(" & "))
            }
            GameResult::Draw => "no foundry survived".to_string(),
        };
        let size = 56.0 * s;
        let dims = measure_text(&text, None, size as u16, 1.0);
        let x = (screen_width() - dims.width) * 0.5;
        // The whole column (banner + stats + curves + caption) must fit
        // the viewport: center it, then clamp against both edges.
        let seats = game.state.players().len() as f32;
        let column_h = 124.0 * s + seats * 22.0 * s + 96.0 * s + 60.0 * s;
        let y = (screen_height() * 0.4)
            .min(screen_height() - column_h + 48.0 * s)
            .max(56.0 * s);
        draw_rectangle(
            x - 24.0 * s,
            y - 48.0 * s,
            dims.width + 48.0 * s,
            124.0 * s,
            PANEL,
        );
        draw_text(&text, x, y, size, color);
        let sub_dims = measure_text(&sub, None, (20.0 * s) as u16, 1.0);
        draw_text(
            &sub,
            (screen_width() - sub_dims.width) * 0.5,
            y + 26.0 * s,
            20.0 * s,
            BONE_FAINT,
        );
        // The match in numbers: one line per seat from the recomputed
        // record — losses and the peak army it ever fielded — then the
        // army curves themselves, seat-colored, so the shape of the
        // game (the swing, the collapse, the long grind) reads at a
        // glance.
        if let Some(stats) = &game.end_stats {
            let curves_y = y + (92.0 + 22.0 * stats.players.len() as f32) * s;
            let (gw, gh) = (
                (360.0 * s).min(screen_width() - 48.0 * s),
                (96.0 * s).min(screen_height() * 0.2),
            );
            let gx = (screen_width() - gw) * 0.5;
            draw_rectangle(
                gx - 8.0 * s,
                curves_y - 8.0 * s,
                gw + 16.0 * s,
                gh + 16.0 * s,
                PANEL,
            );
            let top = stats
                .players
                .iter()
                .flat_map(|p| p.army_value.iter().copied())
                .max()
                .unwrap_or(1)
                .max(1) as f32;
            for (i, seat) in stats.players.iter().enumerate() {
                let faction = game
                    .state
                    .players()
                    .get(i)
                    .map(|p| p.faction)
                    .unwrap_or(oxide_sim::Faction::Ferrous);
                let color = mini_faction_color(faction);
                let n = seat.army_value.len().max(2);
                let mut prev: Option<macroquad::prelude::Vec2> = None;
                for (k, &v) in seat.army_value.iter().enumerate() {
                    let px = gx + gw * k as f32 / (n - 1) as f32;
                    let py = curves_y + gh - gh * (v as f32 / top);
                    let point = vec2(px, py);
                    if let Some(a) = prev {
                        draw_line(a.x, a.y, point.x, point.y, 1.5 * s, color);
                    }
                    prev = Some(point);
                }
            }
            let cap = "army value over the match";
            let cap_dims = measure_text(cap, None, (13.0 * s) as u16, 1.0);
            draw_text(
                cap,
                (screen_width() - cap_dims.width) * 0.5,
                curves_y + gh + 14.0 * s,
                13.0 * s,
                BONE_FAINT,
            );
            for (i, seat) in stats.players.iter().enumerate() {
                let name = game
                    .state
                    .players()
                    .get(i)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| format!("seat {i}"));
                let peak = seat.army_value.iter().copied().max().unwrap_or(0);
                let line = format!(
                    "{name}: lost {} units, {} buildings · peak army {peak} · scrap {}",
                    seat.units_lost,
                    seat.buildings_lost,
                    seat.scrap.last().copied().unwrap_or(0),
                );
                let dims = measure_text(&line, None, (16.0 * s) as u16, 1.0);
                draw_text(
                    &line,
                    (screen_width() - dims.width) * 0.5,
                    y + (86.0 + 22.0 * i as f32) * s,
                    16.0 * s,
                    BONE_FAINT,
                );
            }
        }
        let hint = "Press Esc to continue";
        let hint_dims = measure_text(hint, None, (20.0 * s) as u16, 1.0);
        draw_text(
            hint,
            (screen_width() - hint_dims.width) * 0.5,
            y + 52.0 * s,
            20.0 * s,
            BONE_FAINT,
        );
    }
}
