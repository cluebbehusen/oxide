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

fn mode_ribbon_geometry(
    viewport: Vec2,
    scale: f32,
    label_width: f32,
    panel_top: f32,
) -> (Rect, Rect) {
    let height = crate::layout::MIN_TOUCH_TARGET * scale;
    let cancel_width = crate::layout::MIN_TOUCH_TARGET * scale;
    let width = (label_width + 34.0 * scale + cancel_width)
        .max(210.0 * scale)
        .min((viewport.x - 24.0 * scale).max(cancel_width));
    let x = (viewport.x - width) * 0.5;
    let preferred_y = if panel_top.is_finite() {
        panel_top - height - 8.0 * scale
    } else {
        viewport.y - height - 12.0 * scale
    };
    let min_y = crate::layout::TOP_BAR_H * scale + 8.0 * scale;
    let max_y = (viewport.y - height - 8.0 * scale).max(min_y);
    let ribbon = Rect::new(x, preferred_y.clamp(min_y, max_y), width, height);
    let cancel = Rect::new(
        ribbon.x + ribbon.w - cancel_width,
        ribbon.y,
        cancel_width,
        height,
    );
    (ribbon, cancel)
}

fn draw_mode_ribbon(input: &InputState, panel_top: f32) -> (Rect, Rect) {
    let Some(mode) = input.armed_mode() else {
        let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
        return (zero, zero);
    };
    let s = ui_scale();
    let label = format!("MODE  |  {}", mode.label());
    let size = 15.0 * s;
    let width = measure_text(&label, None, size as u16, 1.0).width;
    let (ribbon, cancel) =
        mode_ribbon_geometry(vec2(screen_width(), screen_height()), s, width, panel_top);
    draw_rectangle(
        ribbon.x,
        ribbon.y,
        ribbon.w,
        ribbon.h,
        Color::from_rgba(20, 20, 24, 248),
    );
    draw_rectangle_lines(ribbon.x, ribbon.y, ribbon.w, ribbon.h, 1.5 * s, SCRAP_COLOR);
    draw_rectangle(ribbon.x, ribbon.y, 4.0 * s, ribbon.h, SCRAP_COLOR);
    draw_text(
        &label,
        ribbon.x + 14.0 * s,
        ribbon.y + ribbon.h * 0.62,
        size,
        TEXT_PRIMARY,
    );
    draw_rectangle(
        cancel.x,
        cancel.y,
        cancel.w,
        cancel.h,
        Color::new(0.25, 0.10, 0.11, 1.0),
    );
    draw_rectangle_lines(cancel.x, cancel.y, cancel.w, cancel.h, 1.5 * s, DANGER);
    let cancel_label = "CANCEL";
    let cancel_size = 9.0 * s;
    let dims = measure_text(cancel_label, None, cancel_size as u16, 1.0);
    draw_text(
        cancel_label,
        cancel.x + (cancel.w - dims.width) * 0.5,
        cancel.y + cancel.h * 0.59,
        cancel_size,
        TEXT_PRIMARY,
    );
    (ribbon, cancel)
}

fn toast_origin(viewport: Vec2, scale: f32, panel_top: f32, orders: Rect, index: usize) -> Vec2 {
    let x = if orders.w > 0.0 {
        orders.x + orders.w + 12.0 * scale
    } else {
        12.0 * scale
    };
    let newest = if panel_top.is_finite() {
        panel_top - 12.0 * scale
    } else {
        viewport.y - 24.0 * scale
    };
    vec2(
        x,
        (newest - 24.0 * index as f32 * scale).max((crate::layout::TOP_BAR_H + 18.0) * scale),
    )
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
            TEXT_PRIMARY,
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
            "RMB advance".to_string(),
            format!("{} attack-move", label(Action::AttackMove)),
            "Shift queues".to_string(),
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
                format!("{hint} | {seg}")
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
                TEXT_BODY,
            );
        }
    }

    *game.panel_model.borrow_mut() = crate::panel::build(game, &input.bindings);
    let panel = game.panel_model.borrow();
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut roster_slots = [(zero, crate::panel::CardAction::None); 8];
    let mut roster_count = 0;
    let mut cards = [(zero, crate::panel::CardAction::None); 16];
    let mut card_count = 0;
    let mut queue_slots = [(zero, crate::panel::CardAction::None); 8];
    let mut queue_count = 0;
    let mut panel_top = f32::INFINITY;
    let mut panel_right = 0.0;
    let mut orders_dock = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut minimap = minimap_rect(game);
    if let Some(panel) = panel.as_ref() {
        let (r, rc, c, cc, q, qc, top, right, dock, hides_minimap) =
            draw_panel(game, sprites, input, panel);
        roster_slots = r;
        roster_count = rc;
        cards = c;
        card_count = cc;
        queue_slots = q;
        queue_count = qc;
        panel_top = top;
        panel_right = right;
        orders_dock = dock;
        if hides_minimap {
            minimap = zero;
        }
    }
    let (mode_ribbon, mode_cancel) = draw_mode_ribbon(input, panel_top);
    // Publish the frame's chrome geometry — the model hit-testing reads.
    game.layout.set(crate::layout::LayoutModel::compute(
        vec2(screen_width(), screen_height()),
        s,
        panel_top,
        panel_right,
        orders_dock,
        minimap,
        idle_badge,
        mode_ribbon,
        mode_cancel,
        roster_slots,
        roster_count,
        cards,
        card_count,
        queue_slots,
        queue_count,
    ));

    // Toasts: rejected orders and stalled units, newest at the bottom.
    for (i, toast) in game.toasts.iter().rev().take(3).enumerate() {
        let fade = (1.0 - (toast.age - 1.5).max(0.0)).clamp(0.0, 1.0);
        let origin = toast_origin(
            vec2(screen_width(), screen_height()),
            s,
            panel_top,
            orders_dock,
            i,
        );
        let mut size = 20.0 * s;
        let available = (screen_width() - origin.x - 12.0 * s).max(1.0);
        while measure_text(&toast.text, None, size as u16, 1.0).width > available && size > 12.0 * s
        {
            size -= 1.0 * s;
        }
        let color = Color::new(0.92, 0.5, 0.45, fade);
        draw_text(&toast.text, origin.x, origin.y, size, color);
    }

    // Spectator strip: a foundry-less or resigned seat on a living team
    // stays in the match by design — masterless machines finish their
    // orders and the team plays on — but the human deserves to be told
    // the seat has no voice left. Commands still route; the sim rejects
    // them.
    let resigned = game.state.player(game.human).resigned;
    if game.state.result().is_none()
        && (resigned
            || !game
                .state
                .buildings()
                .iter()
                .any(|b| b.player == game.human && b.kind == oxide_sim::BuildingKind::Foundry))
    {
        let text = if resigned {
            "SURRENDERED - SPECTATING"
        } else {
            "ELIMINATED - SPECTATING"
        };
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

/// A team-game concession can leave the match undecided while the ally
/// keeps fighting. This compact exit offer is the only result-like layer
/// gameplay still draws; a decided match moves to the dedicated Results
/// screen with touchable next steps.
pub(crate) fn draw_result_overlay(game: &Game) {
    if !game.conceded_banner || game.state.result().is_some() {
        return;
    }
    let s = ui_scale();
    let text = "SURRENDERED";
    let size = 48.0 * s;
    let dims = measure_text(text, None, size as u16, 1.0);
    let x = (screen_width() - dims.width) * 0.5;
    let y = screen_height() * 0.38;
    draw_rectangle(
        x - 24.0 * s,
        y - 48.0 * s,
        dims.width + 48.0 * s,
        112.0 * s,
        PANEL,
    );
    draw_text(text, x, y, size, DANGER);
    let sub = "your team fights on | Esc for options";
    let sub_dims = measure_text(sub, None, (18.0 * s) as u16, 1.0);
    draw_text(
        sub,
        (screen_width() - sub_dims.width) * 0.5,
        y + 28.0 * s,
        18.0 * s,
        TEXT_BODY,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_mode_ribbon_and_cancel_fit_the_small_window_contract() {
        for panel_top in [f32::INFINITY, 150.0] {
            let viewport = vec2(640.0, 400.0);
            let (ribbon, cancel) = mode_ribbon_geometry(viewport, 1.0, 180.0, panel_top);
            assert!(ribbon.x >= 0.0 && ribbon.x + ribbon.w <= viewport.x);
            assert!(ribbon.y >= crate::layout::TOP_BAR_H && ribbon.y + ribbon.h <= viewport.y);
            assert_eq!(cancel.h, crate::layout::MIN_TOUCH_TARGET);
            assert_eq!(cancel.w, crate::layout::MIN_TOUCH_TARGET);
            assert!(ribbon.contains(cancel.center()));
        }
    }

    #[test]
    fn toasts_clear_the_panel_and_its_orders_dock() {
        let viewport = vec2(640.0, 400.0);
        let panel_top = 128.0;
        let orders = Rect::new(0.0, 52.0, 400.0, 76.0);
        for index in 0..3 {
            let origin = toast_origin(viewport, 1.0, panel_top, orders, index);
            assert!(origin.x > orders.x + orders.w);
            assert!(origin.y < panel_top);
            assert!(origin.y >= crate::layout::TOP_BAR_H + 18.0);
        }
    }
}
