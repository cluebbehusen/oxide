//! Standing chrome and overlays: the top bar with its controls hint,
//! toasts, the salvage hover tooltip, the omniscient debug overlay,
//! and the endgame verdict. The HUD publishes the `LayoutModel` so drawn
//! and clickable regions cannot disagree.

use super::*;
use crate::numeric;
use crate::numeric::Fit;
use crate::render::prim::{fill_rect, line_between, stroke_rect};
use crate::theme::TEXT_ACCENT;

/// Fill shared by the top bar's badges: the idle count, the alert, and
/// the menu button.
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

/// The under-attack badge names the jump key where one exists.
fn alert_badge_text(key: &str, touch_only: bool) -> String {
    if touch_only || key.is_empty() {
        "under attack".to_string()
    } else {
        format!("under attack [{key}]")
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

/// A control-group slot: its number in the corner and, for a saved
/// group, the machines it holds; the "+" offers to save the selection,
/// and an empty group is a faint outline. The group the selection
/// already is wears an accent rim.
fn draw_group_slot(
    rect: Rect,
    slot: crate::layout::GroupSlot,
    count: usize,
    current: bool,
    s: f32,
) {
    let number = slot.number();
    match slot {
        crate::layout::GroupSlot::Recall(_) => {
            fill_rect(rect, TOP_BAR_BADGE);
            let text = count.to_string();
            let mut size = 17.0 * s;
            while measure_text(&text, None, numeric::font_size(size), 1.0).width > rect.w - 14.0 * s
                && size > 11.0 * s
            {
                size -= 1.0 * s;
            }
            // Centered on the glyphs' own box, not the baseline, so the
            // count sits in the middle of the slot at any size.
            let dims = measure_text(&text, None, numeric::font_size(size), 1.0);
            draw_text(
                &text,
                rect.x + (rect.w - dims.width) * 0.5,
                rect.y + (rect.h - dims.height) * 0.5 + dims.offset_y,
                size,
                SCRAP_COLOR,
            );
        }
        crate::layout::GroupSlot::Assign(_) => {
            fill_rect(rect, Color::from_rgba(20, 20, 24, 255));
            stroke_rect(rect, 1.0 * s, TEXT_DISABLED);
            let c = rect.center();
            let arm = 6.0 * s;
            line_between(c - vec2(arm, 0.0), c + vec2(arm, 0.0), 2.0 * s, SCRAP_COLOR);
            line_between(c - vec2(0.0, arm), c + vec2(0.0, arm), 2.0 * s, SCRAP_COLOR);
        }
        crate::layout::GroupSlot::Empty(_) => {
            stroke_rect(rect, 1.0 * s, Color::new(0.9, 0.9, 0.85, 0.12));
        }
    }
    draw_text(
        number.to_string(),
        rect.x + 4.0 * s,
        rect.y + 12.0 * s,
        12.0 * s,
        if matches!(slot, crate::layout::GroupSlot::Empty(_)) {
            TEXT_DISABLED
        } else {
            TEXT_SECONDARY
        },
    );
    if current {
        stroke_rect(rect, 1.5 * s, TEXT_ACCENT);
    }
}

/// The control-group column above the minimap, once groups are in use
/// or there is a selection to save: every group keeps its place, empty
/// ones as faint outlines. Returns the plate and the published slots.
fn draw_group_column(
    game: &crate::game::Scene<'_>,
    input: &InputState,
    minimap: Rect,
    s: f32,
) -> (
    Rect,
    [Option<(Rect, crate::layout::GroupSlot)>; crate::action::CONTROL_GROUPS],
) {
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut published = [None; crate::action::CONTROL_GROUPS];
    let counts = input.group_counts(game);
    let offer = input.group_on_offer(game);
    let shown = crate::render::control_groups()
        && (counts.iter().any(|count| *count > 0) || offer.is_some());
    let Some(column) =
        crate::layout::group_column(s, crate::platform::TOUCH_ONLY, minimap).filter(|_| shown)
    else {
        return (zero, published);
    };
    fill_rect(column.plate, Color::from_rgba(20, 20, 24, 255));
    stroke_rect(column.plate, 1.5 * s, Color::new(0.6, 0.6, 0.65, 0.4));
    let current = input.selected_group(game);
    for (slot, rect) in column.slots.iter().enumerate() {
        let number = slot.fit::<u8>() + 1;
        let action = if counts[slot] > 0 {
            crate::layout::GroupSlot::Recall(number)
        } else if offer == Some(number) {
            crate::layout::GroupSlot::Assign(number)
        } else {
            crate::layout::GroupSlot::Empty(number)
        };
        draw_group_slot(*rect, action, counts[slot], current == Some(number), s);
        published[slot] = Some((*rect, action));
    }
    (column.plate, published)
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
    let tile = numeric::tile_at(world);
    let text = match game.known_salvage(tile) {
        Some(crate::game::Salvage::Scrap(amount)) => format!("scrap {amount}"),
        Some(crate::game::Salvage::Wreck(amount)) => format!("wreck {amount}"),
        None => return,
    };
    let s = ui_scale();
    let dims = measure_text(&text, None, numeric::font_size(16.0 * s), 1.0);
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
    let width = measure_text(&info, None, numeric::font_size(size), 1.0).width;
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
    let label_w = measure_text(&label, None, numeric::font_size(label_size), 1.0).width;
    let cost_w = cost.as_deref().map_or(0.0, |cost| {
        12.0 * s + measure_text(cost, None, numeric::font_size(cost_size), 1.0).width
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
    bindings: &crate::action::BindingMap,
    performance: Option<&crate::performance::PerformanceView>,
) {
    let s = ui_scale();
    // A spectator commands nothing: no bank, unit count, or idle badge;
    // the viewer's transport bar is its own chrome. The layout still
    // publishes below so the minimap stays clickable.
    let mut idle_badge = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut alert_badge = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut menu_button = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut pause_status = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut status_space = None;
    if !game.presentation.spectate {
        use crate::action::{Action, BindingMap};
        // Top bar.
        draw_rectangle(
            0.0,
            0.0,
            screen_width(),
            crate::layout::TOP_BAR_H * s,
            PANEL,
        );
        let label = |action| {
            bindings
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
        // Alerts age out in a few seconds and hold while paused, so the
        // badge shows exactly while the minimap still pulses one.
        let alert_text = (!game.presentation.alerts.is_empty()).then(|| {
            alert_badge_text(&label(Action::JumpToLastAlert), crate::platform::TOUCH_ONLY)
        });
        let status = if game.clock.paused {
            paused_status(&label(Action::TogglePause), crate::platform::TOUCH_ONLY)
        } else if (game.clock.speed - 1.0).abs() > f64::EPSILON {
            format!("x{:.2}", game.clock.speed)
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
                passive: measure_text(&passive_text, None, numeric::font_size(16.0 * s), 1.0).width,
                units_label: crate::typography::measure("UNITS", 13.0 * s).width,
                units: crate::typography::measure(&units_text, 21.0 * s).width,
                idle: idle_text
                    .as_deref()
                    .map(|text| measure_text(text, None, numeric::font_size(15.0 * s), 1.0).width),
                alert: alert_text
                    .as_deref()
                    .map(|text| measure_text(text, None, numeric::font_size(15.0 * s), 1.0).width),
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
        if let Some(text) = &alert_text {
            alert_badge = bar.alert_badge;
            fill_rect(alert_badge, TOP_BAR_BADGE);
            draw_text(
                text,
                alert_badge.x + 9.0 * s,
                26.0 * s,
                15.0 * s,
                crate::theme::TEXT_DANGER,
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

    *game.presentation.panel_model.borrow_mut() =
        crate::panel::build_for_input(game, bindings, input);
    let panel = game.presentation.panel_model.borrow();
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut roster_slots = [(zero, crate::panel::CardAction::None); 8];
    let mut roster_count = 0;
    let mut cards = [(zero, crate::panel::CardAction::None); 16];
    let mut card_count = 0;
    let mut queue_slots = [(zero, crate::panel::CardAction::None); 8];
    let mut queue_count = 0;
    let mut queue_stop = (zero, crate::panel::CardAction::None);
    let mut panel_top = f32::INFINITY;
    let mut panel_right = 0.0;
    let mut orders_dock = Rect::new(0.0, 0.0, 0.0, 0.0);
    let mut minimap = minimap_rect(game);
    let mut panel_regions = [zero; 2];
    if let Some(panel) = panel.as_ref() {
        let geometry = draw_panel(game, sprites, input, bindings, panel);
        roster_slots = geometry.roster_slots;
        roster_count = geometry.roster_count;
        cards = geometry.cards;
        card_count = geometry.card_count;
        queue_slots = geometry.queue_slots;
        queue_count = geometry.queue_count;
        queue_stop = geometry.queue_stop;
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
    let (group_column, group_slots) = if game.presentation.spectate {
        (zero, [None; crate::action::CONTROL_GROUPS])
    } else {
        draw_group_column(game, input, minimap, s)
    };
    let mode_ribbon = draw_mode_ribbon(game, sprites, input, &panel_regions, minimap);
    let queue_toggle = if queue_toggle_shown(game, input, crate::platform::TOUCH_ONLY) {
        let rect = queue_toggle_rect(vec2(screen_width(), screen_height()), s, &panel_regions);
        draw_queue_toggle(rect, input.queue_toggle, s);
        rect
    } else {
        zero
    };
    // Publish the frame's chrome geometry for hit-testing.
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
    layout.queue_stop = queue_stop;
    layout.alert_badge = alert_badge;
    layout.group_column = group_column;
    layout.group_slots = group_slots;
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
        while measure_text(&toast.text, None, numeric::font_size(size), 1.0).width > available
            && size > 12.0 * s
        {
            size -= 1.0 * s;
        }
        let color = Color::new(0.92, 0.5, 0.45, fade);
        draw_text(&toast.text, origin.x, origin.y, size, color);
    }

    // Spectator strip: a foundry-less or resigned seat on a living team
    // stays in the match (its machines finish their orders and the team
    // plays on), so tell the human the seat can no longer act. Commands
    // still route; the sim rejects them.
    let resigned = game.state.player(game.presentation.human).resigned;
    if game.state.result().is_none() && !game.state.accepts_commands(game.presentation.human) {
        let text = if resigned {
            "SURRENDERED - SPECTATING"
        } else {
            "ELIMINATED - SPECTATING"
        };
        let dims = measure_text(text, None, numeric::font_size(24.0 * s), 1.0);
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
/// keeps fighting. This compact exit offer is the only result layer
/// gameplay draws; a decided match moves to the Results screen.
pub(crate) fn draw_result_overlay(game: &crate::game::Scene<'_>) {
    if !game.presentation.conceded_banner || game.state.result().is_some() {
        return;
    }
    let s = ui_scale();
    let text = "SURRENDERED";
    let size = 48.0 * s;
    let dims = measure_text(text, None, numeric::font_size(size), 1.0);
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
    let sub_dims = measure_text(&sub, None, numeric::font_size(18.0 * s), 1.0);
    draw_text(
        &sub,
        (screen_width() - sub_dims.width) * 0.5,
        y + 28.0 * s,
        18.0 * s,
        TEXT_BODY,
    );
}

#[cfg(test)]
mod tests;
