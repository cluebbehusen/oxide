//! Standing chrome and overlays: the top bar with its controls hint,
//! toasts, the salvage hover tooltip, the omniscient debug overlay,
//! and the endgame verdict. The LayoutModel publish rides in the hud
//! so drawn and clickable can never disagree.

use super::*;
use crate::render::prim::{fill_rect, line_between, stroke_rect};

/// Fill shared by the top bar's badges: the idle nag and the menu button.
const TOP_BAR_BADGE: Color = color_u8!(57, 45, 30, 255);

/// The paused status names the pause key where one exists. A
/// touch-only build has no key to name, and an unbound key reads bare.
fn paused_status(key: &str, touch_only: bool) -> String {
    if touch_only || key.is_empty() {
        "PAUSED".to_string()
    } else {
        format!("PAUSED [{key}]")
    }
}

/// Three bars on the badge fill: the glyph needs no font coverage or
/// atlas entry, and whole-rect fills stay crisp at any scale.
fn draw_menu_button(rect: Rect, s: f32) {
    fill_rect(rect, TOP_BAR_BADGE);
    let bar_w = 16.0 * s;
    let bar_h = (2.0 * s).max(1.0);
    let x = rect.x + (rect.w - bar_w) * 0.5;
    let middle = rect.y + rect.h * 0.5;
    for offset in [-5.0, 0.0, 5.0] {
        fill_rect(
            Rect::new(x, middle + offset * s - bar_h * 0.5, bar_w, bar_h),
            SCRAP_COLOR,
        );
    }
}

/// Hovered salvage says what it holds: live amounts on visible ground,
/// remembered amounts under the dim — the same memory rule as every
/// renderer, so the tooltip can't leak what fog took back.
pub(crate) fn draw_salvage_tooltip(game: &crate::game::Scene<'_>, input: &InputState) {
    if game.presentation.layout.get().chrome_owns(input.mouse) {
        return;
    }
    let world = game.presentation.camera.to_world(input.mouse);
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    let vision = game.my_vision();
    if !vision.explored(tile) && !game.presentation.all_seeing() {
        return;
    }
    let (scrap, wreck) = if vision.visible(tile) || game.presentation.all_seeing() {
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

pub(crate) fn draw_overlay(game: &crate::game::Scene<'_>, alpha: f32) {
    let (min, max) = visible_tiles(game);
    for x in min.x..=max.x {
        let a = game
            .presentation
            .camera
            .to_screen(vec2(x as f32, min.y as f32));
        let b = game
            .presentation
            .camera
            .to_screen(vec2(x as f32, max.y as f32));
        line_between(a, b, 1.0, BONE_FAINT);
    }
    for y in min.y..=max.y {
        let a = game
            .presentation
            .camera
            .to_screen(vec2(min.x as f32, y as f32));
        let b = game
            .presentation
            .camera
            .to_screen(vec2(max.x as f32, y as f32));
        line_between(a, b, 1.0, BONE_FAINT);
    }
    for unit in game.state.units() {
        let pos = game.presentation.draw_pos(unit.id, unit.pos, alpha);
        let screen = game.presentation.camera.to_screen(pos);
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
                    .presentation
                    .camera
                    .to_screen(vec2(waypoint.x as f32 + 0.5, waypoint.y as f32 + 0.5));
                line_between(previous, next, 1.0, BONE_FAINT);
                previous = next;
            }
        }
    }
}

pub(crate) fn draw_overlay_info(game: &crate::game::Scene<'_>) {
    let info = format!(
        "tick {}  fps {}  zoom {:.0}  center ({:.1},{:.1})",
        game.state.current_tick(),
        get_fps(),
        game.presentation.camera.zoom,
        game.presentation.camera.center.x,
        game.presentation.camera.center.y,
    );
    let s = ui_scale();
    let panel = game.presentation.layout.get().performance;
    let y = if panel.w > 0.0 {
        panel.y + panel.h + 20.0 * s
    } else {
        60.0 * s
    };
    let size = 14.0 * s;
    let width = measure_text(&info, None, size as u16, 1.0).width;
    draw_text(
        &info,
        (screen_width() - width - 12.0 * s).max(0.0),
        y,
        size,
        BONE,
    );
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

fn draw_mode_ribbon(input: &InputState, regions: &[Rect; 2]) -> (Rect, Rect) {
    let Some(mode) = input.armed_mode() else {
        let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
        return (zero, zero);
    };
    let s = ui_scale();
    let label = format!("MODE  |  {}", mode.label());
    let size = 15.0 * s;
    let width = measure_text(&label, None, size as u16, 1.0).width;
    let estimated_width = (width + 34.0 * s + crate::layout::MIN_TOUCH_TARGET * s)
        .max(210.0 * s)
        .min(screen_width() - 24.0 * s);
    let x = (screen_width() - estimated_width) * 0.5;
    let panel_top = regions
        .iter()
        .filter(|r| r.w > 0.0 && r.x < x + estimated_width && r.x + r.w > x)
        .map(|r| r.y)
        .fold(f32::INFINITY, f32::min);
    let (ribbon, cancel) =
        mode_ribbon_geometry(vec2(screen_width(), screen_height()), s, width, panel_top);
    fill_rect(ribbon, Color::from_rgba(20, 20, 24, 248));
    stroke_rect(ribbon, 1.5 * s, SCRAP_COLOR);
    draw_rectangle(ribbon.x, ribbon.y, 4.0 * s, ribbon.h, SCRAP_COLOR);
    draw_text(
        &label,
        ribbon.x + 14.0 * s,
        ribbon.y + ribbon.h * 0.62,
        size,
        TEXT_PRIMARY,
    );
    fill_rect(cancel, Color::new(0.25, 0.10, 0.11, 1.0));
    stroke_rect(cancel, 1.5 * s, DANGER);
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

pub(crate) fn draw_hud(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    input: &InputState,
    performance: Option<&crate::performance::PerformanceView>,
) {
    let s = ui_scale();
    // A spectator commands nothing: no bank, no unit count, no idle
    // nag — the viewer's transport bar is its own chrome. The layout
    // still publishes below so the minimap stays clickable.
    let mut idle_badge = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut menu_button = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut pause_status = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut status_space = None;
    if !game.presentation.spectate {
        // Top bar.
        draw_rectangle(
            0.0,
            0.0,
            screen_width(),
            crate::layout::TOP_BAR_H * s,
            PANEL,
        );
        use crate::action::{Action, BindingMap};
        let label = |action| {
            input
                .bindings
                .chord_for(action)
                .map(BindingMap::chord_label)
                .unwrap_or_default()
        };
        let scrap = game.state.player(game.presentation.human).scrap;
        let passive: u32 = game
            .state
            .buildings()
            .iter()
            .map(|building| crate::panel::building_income(game, building))
            .sum();
        let my_units = game
            .state
            .units()
            .iter()
            .filter(|unit| unit.player == game.presentation.human)
            .count();
        let scrap_text = scrap.to_string();
        let passive_text = format!("+{passive}/min passive");
        let units_text = my_units.to_string();
        let passive_x =
            (70.0 * s + crate::typography::measure(&scrap_text, 21.0 * s).width + 16.0 * s)
                .max(151.0 * s);
        let units_x = (passive_x
            + measure_text(&passive_text, None, (16.0 * s) as u16, 1.0).width
            + 16.0 * s)
            .max(284.0 * s);
        let count_x = units_x
            + (crate::typography::measure("UNITS", 13.0 * s).width + 12.0 * s).max(60.0 * s);
        crate::typography::draw("SCRAP", 12.0 * s, 26.0 * s, 13.0 * s, TEXT_SECONDARY);
        crate::typography::draw(&scrap_text, 70.0 * s, 27.0 * s, 21.0 * s, SCRAP_COLOR);
        draw_text(&passive_text, passive_x, 26.0 * s, 16.0 * s, TEXT_BODY);
        crate::typography::draw("UNITS", units_x, 26.0 * s, 13.0 * s, TEXT_SECONDARY);
        crate::typography::draw(&units_text, count_x, 27.0 * s, 21.0 * s, TEXT_PRIMARY);
        let idle = crate::input::idle_harvesters(game).len();
        if idle > 0 {
            let text = format!("{idle} idle [{}]", label(Action::CycleIdleWorker));
            let width = measure_text(&text, None, (15.0 * s) as u16, 1.0).width + 18.0 * s;
            let idle_x = count_x
                + (crate::typography::measure(&units_text, 21.0 * s).width + 20.0 * s)
                    .max(45.0 * s);
            idle_badge = Rect::new(idle_x, 3.0 * s, width, 34.0 * s);
            fill_rect(idle_badge, TOP_BAR_BADGE);
            draw_text(
                &text,
                idle_badge.x + 9.0 * s,
                26.0 * s,
                15.0 * s,
                SCRAP_COLOR,
            );
        }
        menu_button = crate::layout::menu_button_rect(screen_width(), s);
        draw_menu_button(menu_button, s);
        let status = if game.presentation.paused {
            paused_status(&label(Action::TogglePause), crate::platform::TOUCH_ONLY)
        } else if (game.presentation.speed - 1.0).abs() > f64::EPSILON {
            format!("x{:.2}", game.presentation.speed)
        } else {
            let seconds = game.state.current_tick() / u64::from(oxide_sim::TICKS_PER_SECOND);
            format!("{}:{:02}", seconds / 60, seconds % 60)
        };
        let width = crate::typography::measure(&status, 14.0 * s).width;
        let occupied_right = (count_x + crate::typography::measure(&units_text, 21.0 * s).width)
            .max(idle_badge.x + idle_badge.w);
        let status_x = menu_button.x - 12.0 * s - width;
        status_space = Some((occupied_right, status_x));
        crate::typography::draw(&status, status_x, 26.0 * s, 14.0 * s, TEXT_PRIMARY);
        // The status toggles pause; its target spans the bar's badge
        // band so the short clock text is still easy to hit.
        pause_status = Rect::new(status_x - 6.0 * s, 3.0 * s, width + 12.0 * s, 34.0 * s);
    }

    *game.presentation.panel_model.borrow_mut() = crate::panel::build_for_input(game, input);
    let panel = game.presentation.panel_model.borrow();
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
    let mut panel_regions = [zero; 2];
    if let Some(panel) = panel.as_ref() {
        let geometry = draw_panel(game, sprites, input, panel);
        roster_slots = geometry.roster_slots;
        roster_count = geometry.roster_count;
        cards = geometry.cards;
        card_count = geometry.card_count;
        queue_slots = geometry.queue_slots;
        queue_count = geometry.queue_count;
        panel_regions = [geometry.info, geometry.actions];
        panel_top = panel_regions
            .iter()
            .filter(|r| r.w > 0.0)
            .map(|r| r.y)
            .fold(f32::INFINITY, f32::min);
        panel_right = panel_regions.iter().map(|r| r.x + r.w).fold(0.0, f32::max);
        orders_dock = geometry.orders;
        if geometry.hides_minimap {
            minimap = zero;
        }
    }
    let (mode_ribbon, mode_cancel) = draw_mode_ribbon(input, &panel_regions);
    // Publish the frame's chrome geometry — the model hit-testing reads.
    let mut layout = crate::layout::LayoutModel::compute(
        vec2(screen_width(), screen_height()),
        s,
        panel_top,
        panel_right,
        orders_dock,
        minimap,
        idle_badge,
        menu_button,
        pause_status,
        mode_ribbon,
        mode_cancel,
        roster_slots,
        roster_count,
        cards,
        card_count,
        queue_slots,
        queue_count,
    );
    layout.panel_regions = panel_regions;
    game.presentation.layout.set(layout);

    if let Some(view) = performance {
        let panel = super::performance::draw(view, status_space);
        let mut layout = game.presentation.layout.get();
        layout.performance = panel;
        game.presentation.layout.set(layout);
    }

    // Toasts: rejected orders and stalled units, newest at the bottom.
    for (i, toast) in game.presentation.toasts.iter().rev().take(3).enumerate() {
        let fade = (1.0 - (toast.age - 1.5).max(0.0)).clamp(0.0, 1.0);
        let origin = toast_origin(
            vec2(screen_width(), screen_height()),
            s,
            if panel_regions[1].w > 0.0 {
                panel_regions[1].y
            } else {
                screen_height()
            },
            Rect::new(0.0, 0.0, orders_dock.w.max(panel_regions[0].w), 0.0),
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
    let resigned = game.state.player(game.presentation.human).resigned;
    if game.state.result().is_none() && !game.state.accepts_commands(game.presentation.human) {
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
pub(crate) fn draw_result_overlay(game: &crate::game::Scene<'_>) {
    if !game.presentation.conceded_banner || game.state.result().is_some() {
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
    let sub = crate::menu::binding_hint("your team fights on | {back} for options");
    let sub_dims = measure_text(&sub, None, (18.0 * s) as u16, 1.0);
    draw_text(
        &sub,
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
    fn the_paused_status_names_a_key_only_where_one_exists() {
        assert_eq!(paused_status("P", false), "PAUSED [P]");
        assert_eq!(paused_status("P", true), "PAUSED");
        assert_eq!(paused_status("", false), "PAUSED");
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
