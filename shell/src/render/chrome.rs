//! Standing chrome and overlays: the top bar with its controls hint,
//! toasts, the salvage hover tooltip, the omniscient debug overlay,
//! and the endgame verdict. The LayoutModel publish rides in the hud
//! so drawn and clickable can never disagree.

use super::*;
use crate::render::prim::{fill_rect, line_between, stroke_rect};
use crate::theme::TEXT_ACCENT;

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

/// The idle badge names its key where one exists.
fn idle_badge_text(idle: usize, key: &str, touch_only: bool) -> String {
    if touch_only {
        format!("{idle} idle")
    } else {
        format!("{idle} idle [{key}]")
    }
}

/// The concede banner's way to the menu.
fn concede_hint(touch_only: bool) -> &'static str {
    if touch_only {
        "your team fights on | tap the menu button for options"
    } else {
        "your team fights on | {back} for options"
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

/// Where the cursor hovers: only while the mouse is the pointer in use,
/// never at a stale point on a touch device.
fn hover_point(input: &InputState) -> Option<Vec2> {
    (input.touches.is_empty() && input.last_pointer == crate::input::Pointer::Mouse)
        .then_some(input.mouse)
}

/// Hovered salvage says what it holds, by the same fog rule as the
/// panel. Touch reads it by selecting the pile instead.
pub(crate) fn draw_salvage_tooltip(game: &crate::game::Scene<'_>, input: &InputState) {
    let Some(point) = hover_point(input) else {
        return;
    };
    if game.presentation.layout.get().chrome_owns(point) {
        return;
    }
    let world = game.presentation.camera.to_world(point);
    let tile = TilePos::new(world.x.floor() as i32, world.y.floor() as i32);
    let text = match game.known_salvage(tile) {
        Some(crate::game::Salvage::Scrap(amount)) => format!("scrap {amount}"),
        Some(crate::game::Salvage::Wreck(amount)) => format!("wreck {amount}"),
        None => return,
    };
    let s = ui_scale();
    let dims = measure_text(&text, None, (16.0 * s) as u16, 1.0);
    let (x, y) = (point.x + 14.0 * s, point.y - 10.0 * s);
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

/// The armed-mode ribbon's narrowest width, in logical px at 1x.
const RIBBON_MIN_W: f32 = 210.0;

/// The top of the ribbon: just above the panel band, or above the
/// window's bottom edge, and never under the top bar.
fn ribbon_row_y(viewport: Vec2, scale: f32, panel_top: f32, height: f32) -> f32 {
    let preferred_y = if panel_top.is_finite() {
        panel_top - height - 8.0 * scale
    } else {
        viewport.y - height - 12.0 * scale
    };
    let min_y = crate::layout::TOP_BAR_H * scale + 8.0 * scale;
    let max_y = (viewport.y - height - 8.0 * scale).max(min_y);
    preferred_y.clamp(min_y, max_y)
}

/// The armed-mode ribbon's width for a label of `label_width`.
fn ribbon_width(viewport: Vec2, scale: f32, label_width: f32) -> f32 {
    (label_width + 34.0 * scale)
        .max(RIBBON_MIN_W * scale)
        .min((viewport.x - 24.0 * scale).max(RIBBON_MIN_W * scale))
}

/// Where the armed-mode ribbon sits: centered over the panel band,
/// sliding left only when it would run off the window or into the
/// minimap.
fn ribbon_geometry(viewport: Vec2, scale: f32, panel_top: f32, width: f32, minimap: Rect) -> Rect {
    let height = crate::layout::MIN_TOUCH_TARGET * scale;
    let y = ribbon_row_y(viewport, scale, panel_top, height);
    let blocks_row = minimap.w > 0.0 && minimap.y < y + height && minimap.y + minimap.h > y;
    let right = if blocks_row {
        minimap.x - 8.0 * scale
    } else {
        viewport.x - 12.0 * scale
    };
    let x = ((viewport.x - width) * 0.5)
        .min(right - width)
        .max(12.0 * scale);
    Rect::new(x, y, width, height)
}

/// Whether a touch-only build offers the QUEUE toggle: while the human
/// commands a selected unit, or while QUEUE is on so it can be turned
/// off. Buildings queue nothing, and a spectator commands nothing.
pub(super) fn queue_toggle_shown(
    game: &crate::game::Scene<'_>,
    input: &InputState,
    touch_only: bool,
) -> bool {
    let human = game.presentation.human;
    let commands_units = game
        .presentation
        .selection
        .units
        .iter()
        .any(|id| game.state.unit(*id).is_some_and(|u| u.player == human));
    touch_only && !game.presentation.spectate && (commands_units || input.queue_toggle)
}

/// Where the QUEUE toggle sits: over the panel's left corner, in the
/// orders dock's column, so it never moves as orders queue up; the dock
/// stacks above it instead.
fn queue_toggle_rect(viewport: Vec2, scale: f32, regions: &[Rect; 2]) -> Rect {
    let size = crate::layout::MIN_TOUCH_TARGET * scale;
    let x = 8.0 * scale;
    let floor = regions
        .iter()
        .filter(|r| r.w > 0.0 && r.x < x + size)
        .map(|r| r.y)
        .fold(viewport.y, f32::min);
    Rect::new(x, floor - size - 8.0 * scale, size, size)
}

/// How far the orders dock rises to clear the QUEUE toggle.
pub(super) fn queue_dock_lift(scale: f32) -> f32 {
    (crate::layout::MIN_TOUCH_TARGET + 16.0) * scale
}

/// The QUEUE toggle: a chain of queued waypoints, lit while queueing so
/// the mode can't hide, dark like the orders dock otherwise.
fn draw_queue_toggle(rect: Rect, on: bool, s: f32) {
    let glyph = if on { SURFACE_CARD } else { TEXT_BODY };
    if on {
        fill_rect(rect, TEXT_ACCENT);
    } else {
        fill_rect(rect, Color::from_rgba(20, 20, 24, 255));
        stroke_rect(rect, 1.5 * s, Color::new(0.6, 0.6, 0.65, 0.4));
    }
    let c = rect.center();
    let points = [
        c + vec2(-11.0, 6.0) * s,
        c + vec2(0.0, -6.0) * s,
        c + vec2(11.0, 6.0) * s,
    ];
    line_between(points[0], points[1], 2.0 * s, glyph);
    line_between(points[1], points[2], 2.0 * s, glyph);
    for point in points {
        crate::render::prim::fill_circle(point, 3.5 * s, glyph);
    }
}

/// The armed-mode ribbon, while a mode is armed. Returns its rect (zero
/// when absent).
fn draw_mode_ribbon(
    game: &crate::game::Scene<'_>,
    sprites: &Sprites,
    input: &InputState,
    regions: &[Rect; 2],
    minimap: Rect,
) -> Rect {
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let Some(mode) = input.armed_mode() else {
        return zero;
    };
    let s = ui_scale();
    let viewport = vec2(screen_width(), screen_height());
    let building = match mode {
        crate::input::ArmedMode::Build(kind) => Some(kind),
        _ => None,
    };
    let label = mode.label();
    let cost = building
        .and_then(|kind| kind.base_stats().construction)
        .map(|construction| construction.cost.to_string());
    let (label_size, cost_size) = (18.0 * s, 16.0 * s);
    let icon_w = if building.is_some() { 40.0 * s } else { 0.0 };
    let label_w = measure_text(&label, None, label_size as u16, 1.0).width;
    let cost_w = cost.as_deref().map_or(0.0, |cost| {
        12.0 * s + measure_text(cost, None, cost_size as u16, 1.0).width
    });
    let width = ribbon_width(viewport, s, icon_w + label_w + cost_w);
    // The ribbon sits above whichever panel region lies under it.
    let open = ribbon_geometry(viewport, s, f32::INFINITY, width, minimap);
    let panel_top = regions
        .iter()
        .filter(|r| r.w > 0.0 && r.x < open.x + open.w && r.x + r.w > open.x)
        .map(|r| r.y)
        .fold(f32::INFINITY, f32::min);
    let ribbon = ribbon_geometry(viewport, s, panel_top, width, minimap);
    fill_rect(ribbon, Color::from_rgba(20, 20, 24, 248));
    stroke_rect(ribbon, 1.5 * s, SCRAP_COLOR);
    let mut x = ribbon.x + 10.0 * s;
    if let Some(kind) = building {
        let faction = game.state.player(game.presentation.human).faction;
        let mut layers = vec![(sprites.building_tiered(kind, 0, faction), WHITE)];
        if let Some(mount) = sprites.defense_mount(kind, 0, faction) {
            layers.push((mount, WHITE));
        }
        let icon = Rect::new(
            x,
            ribbon.y + (ribbon.h - 32.0 * s) * 0.5,
            32.0 * s,
            32.0 * s,
        );
        sprites.draw_portrait(icon, &layers);
        x += icon_w;
    }
    let baseline = ribbon.y + ribbon.h * 0.64;
    draw_text(&label, x, baseline, label_size, TEXT_PRIMARY);
    if let Some(cost) = cost {
        draw_text(
            &cost,
            x + label_w + 12.0 * s,
            baseline,
            cost_size,
            SCRAP_COLOR,
        );
    }
    ribbon
}

fn toast_origin(
    viewport: Vec2,
    scale: f32,
    panel_top: f32,
    orders: Rect,
    row: Rect,
    index: usize,
) -> Vec2 {
    let x = if orders.w > 0.0 {
        orders.x + orders.w + 12.0 * scale
    } else {
        12.0 * scale
    };
    // Toasts stack above the ribbon row when it shows, so neither hides
    // the other.
    let panel_top = if row.w > 0.0 {
        panel_top.min(row.y)
    } else {
        panel_top
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
        let idle = crate::input::idle_harvesters(game).len();
        let idle_text = (idle > 0).then(|| {
            idle_badge_text(
                idle,
                &label(Action::CycleIdleWorker),
                crate::platform::TOUCH_ONLY,
            )
        });
        let status = if game.presentation.paused {
            paused_status(&label(Action::TogglePause), crate::platform::TOUCH_ONLY)
        } else if (game.presentation.speed - 1.0).abs() > f64::EPSILON {
            format!("x{:.2}", game.presentation.speed)
        } else {
            let seconds = game.state.current_tick() / u64::from(oxide_sim::TICKS_PER_SECOND);
            format!("{}:{:02}", seconds / 60, seconds % 60)
        };
        let bar = crate::layout::top_bar(
            screen_width(),
            s,
            crate::platform::TOUCH_ONLY,
            crate::layout::TopBarText {
                scrap: crate::typography::measure(&scrap_text, 21.0 * s).width,
                passive: measure_text(&passive_text, None, (16.0 * s) as u16, 1.0).width,
                units_label: crate::typography::measure("UNITS", 13.0 * s).width,
                units: crate::typography::measure(&units_text, 21.0 * s).width,
                idle: idle_text
                    .as_deref()
                    .map(|text| measure_text(text, None, (15.0 * s) as u16, 1.0).width),
                status: crate::typography::measure(&status, 14.0 * s).width,
            },
        );
        crate::typography::draw(
            "SCRAP",
            bar.scrap_label_x,
            26.0 * s,
            13.0 * s,
            TEXT_SECONDARY,
        );
        crate::typography::draw(&scrap_text, bar.scrap_x, 27.0 * s, 21.0 * s, SCRAP_COLOR);
        draw_text(&passive_text, bar.passive_x, 26.0 * s, 16.0 * s, TEXT_BODY);
        crate::typography::draw("UNITS", bar.units_x, 26.0 * s, 13.0 * s, TEXT_SECONDARY);
        crate::typography::draw(&units_text, bar.count_x, 27.0 * s, 21.0 * s, TEXT_PRIMARY);
        if let Some(text) = &idle_text {
            idle_badge = bar.idle_badge;
            fill_rect(idle_badge, TOP_BAR_BADGE);
            draw_text(
                text,
                idle_badge.x + 9.0 * s,
                26.0 * s,
                15.0 * s,
                SCRAP_COLOR,
            );
        }
        menu_button = bar.menu_button;
        draw_menu_button(menu_button, s);
        status_space = Some(bar.status_space);
        crate::typography::draw(&status, bar.status_x, 26.0 * s, 14.0 * s, TEXT_PRIMARY);
        // The status toggles pause; its target spans the bar's badge
        // band so the short clock text is still easy to hit.
        pause_status = bar.pause_status;
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
    let mode_ribbon = draw_mode_ribbon(game, sprites, input, &panel_regions, minimap);
    let queue_toggle = if queue_toggle_shown(game, input, crate::platform::TOUCH_ONLY) {
        let rect = queue_toggle_rect(vec2(screen_width(), screen_height()), s, &panel_regions);
        draw_queue_toggle(rect, input.queue_toggle, s);
        rect
    } else {
        zero
    };
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
        roster_slots,
        roster_count,
        cards,
        card_count,
        queue_slots,
        queue_count,
    );
    layout.panel_regions = panel_regions;
    layout.queue_toggle = queue_toggle;
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
            Rect::new(
                0.0,
                0.0,
                orders_dock
                    .w
                    .max(panel_regions[0].w)
                    .max(queue_toggle.x + queue_toggle.w),
                0.0,
            ),
            mode_ribbon,
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
    let sub = crate::menu::binding_hint(concede_hint(crate::platform::TOUCH_ONLY));
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
    fn hud_copy_names_keys_only_on_desktop() {
        assert_eq!(idle_badge_text(3, "F1", false), "3 idle [F1]");
        crate::platform::assert_touch_copy(&idle_badge_text(3, "F1", true));
        assert!(concede_hint(false).contains("{back}"));
        crate::platform::assert_touch_copy(concede_hint(true));
    }

    #[test]
    fn the_ribbon_stays_on_screen_above_the_panel() {
        for (viewport, scale) in [
            (vec2(1024.0, 768.0), 1.5),
            (vec2(1194.0, 834.0), 1.0),
            (vec2(1366.0, 1024.0), 1.25),
            (vec2(640.0, 400.0), 1.0),
        ] {
            for panel_top in [f32::INFINITY, viewport.y - 120.0 * scale] {
                for label in [60.0, 180.0, 320.0] {
                    let w = ribbon_width(viewport, scale, label * scale);
                    let ribbon = ribbon_geometry(viewport, scale, panel_top, w, Rect::default());
                    assert!(ribbon.x >= 0.0, "{viewport} @{scale}");
                    assert!(ribbon.x + ribbon.w <= viewport.x);
                    assert!(ribbon.y >= crate::layout::TOP_BAR_H * scale);
                    assert!(ribbon.y + ribbon.h <= panel_top.min(viewport.y));
                }
            }
        }
    }

    #[test]
    fn the_queue_toggle_sits_over_the_panel_corner_under_the_dock() {
        let viewport = vec2(1194.0, 834.0);
        for scale in [1.0, 1.25] {
            let info = Rect::new(0.0, 600.0, 300.0 * scale, 234.0);
            let actions = Rect::new(320.0 * scale, 650.0, 600.0, 184.0);
            let toggle = queue_toggle_rect(viewport, scale, &[info, actions]);
            assert!(toggle.x >= 0.0);
            assert!(toggle.y + toggle.h <= info.y, "above the panel it rests on");
            let dock_bottom = info.y - queue_dock_lift(scale);
            assert!(
                dock_bottom <= toggle.y - 8.0 * scale + 0.001,
                "the lifted dock clears it"
            );
            assert_eq!(
                toggle,
                queue_toggle_rect(viewport, scale, &[info, Rect::default()]),
                "the actions band never moves it"
            );
        }
    }

    #[test]
    fn queue_is_offered_only_to_units_or_while_on() {
        let mut game =
            crate::game::Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(1280.0, 800.0))
                .expect("skirmish builds");
        let mut input = InputState::new();
        let shown = |game: &crate::game::Game, input: &InputState| {
            queue_toggle_shown(&game.view(), input, true)
        };
        assert!(!shown(&game, &input), "nothing selected");
        let foundry = game
            .state
            .buildings()
            .iter()
            .find(|b| b.player == game.presentation.human)
            .expect("an own Foundry")
            .id;
        game.presentation.selection.buildings = vec![foundry];
        assert!(!shown(&game, &input), "buildings queue nothing");
        game.presentation.selection.buildings.clear();
        let own = game
            .state
            .units()
            .iter()
            .find(|u| u.player == game.presentation.human)
            .expect("an own unit")
            .id;
        game.presentation.selection.units = vec![own];
        assert!(shown(&game, &input), "an own unit");
        assert!(
            !queue_toggle_shown(&game.view(), &input, false),
            "desktop has Shift"
        );
        game.presentation.selection.units.clear();
        input.queue_toggle = true;
        assert!(shown(&game, &input), "QUEUE on can always be turned off");
        game.presentation.spectate = true;
        assert!(!shown(&game, &input), "never while spectating");
    }

    #[test]
    fn the_ribbon_row_slides_clear_of_the_minimap() {
        let viewport = vec2(1024.0, 768.0);
        let scale = 1.5;
        let minimap = Rect::new(676.0, 520.0, 330.0, 230.0);
        let w = ribbon_width(viewport, scale, 200.0);
        let ribbon = ribbon_geometry(viewport, scale, f32::INFINITY, w, minimap);
        assert!(
            ribbon.x + ribbon.w <= minimap.x,
            "the row stops short of the minimap"
        );
    }

    #[test]
    fn a_touch_device_never_reads_at_a_stale_mouse_point() {
        let mut input = InputState::new();
        input.mouse = vec2(400.0, 300.0);
        input.last_pointer = crate::input::Pointer::Mouse;
        assert_eq!(hover_point(&input), Some(vec2(400.0, 300.0)));
        input.last_pointer = crate::input::Pointer::Touch;
        assert_eq!(hover_point(&input), None);
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
            let origin = toast_origin(viewport, 1.0, panel_top, orders, Rect::default(), index);
            assert!(origin.x > orders.x + orders.w);
            assert!(origin.y < panel_top);
            assert!(origin.y >= crate::layout::TOP_BAR_H + 18.0);
        }
    }

    #[test]
    fn toasts_stack_above_the_ribbon_row() {
        let viewport = vec2(1280.0, 800.0);
        let panel_top = 640.0;
        let ribbon = ribbon_geometry(viewport, 1.0, panel_top, 240.0, Rect::default());
        for index in 0..3 {
            let origin = toast_origin(viewport, 1.0, panel_top, Rect::default(), ribbon, index);
            assert!(origin.y < ribbon.y, "toast {index} clears the row");
        }
    }
}
