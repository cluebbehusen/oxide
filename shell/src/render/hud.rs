//! The HUD's geometry for one frame, laid out without drawing.
//!
//! [`layout`] measures text through a [`Measure`] function, so it runs
//! headless in tests. [`publish`] installs the hit regions and the panel
//! model together on the presentation; input reads them, and drawing
//! consumes the [`HudGeometry`] that comes back, so a click and the frame
//! that shows its target always agree.

use super::*;
use crate::layout::LayoutModel;

/// Which typeface a measured string is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Face {
    /// The default body font.
    Body,
    /// The display font of the top bar, titles and costs.
    Display,
}

/// The width of a string in a face at a pixel size.
pub(crate) type Measure<'a> = &'a dyn Fn(Face, &str, f32) -> f32;

/// Measures against the window's fonts; needs the macroquad context.
pub(crate) fn window_measure(face: Face, text: &str, size: f32) -> f32 {
    match face {
        Face::Body => measure_text(text, None, crate::numeric::font_size(size), 1.0).width,
        Face::Display => crate::typography::measure(text, size).width,
    }
}

/// The window the HUD lays out against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HudEnv {
    pub(crate) viewport: Vec2,
    pub(crate) ui: f32,
}

impl HudEnv {
    /// The injected window and its UI scale this frame.
    pub(crate) fn current() -> Self {
        Self {
            viewport: viewport(),
            ui: ui_scale(),
        }
    }
}

/// One frame's HUD: the hit regions and panel model input reads, and
/// the placement drawing needs on top of them.
pub(crate) struct Hud {
    layout: LayoutModel,
    panel: Option<crate::panel::Panel>,
    geometry: HudGeometry,
}

/// What drawing needs beyond the published hit regions.
pub(crate) struct HudGeometry {
    pub(super) top_bar: Option<chrome::TopBarLayout>,
    pub(super) panel: Option<panel_draw::PanelLayout>,
    pub(super) group_column: Option<chrome::GroupColumnLayout>,
    pub(super) ribbon: Option<chrome::RibbonLayout>,
    pub(super) queue_toggle: Option<Rect>,
    pub(super) performance: Option<performance::PerformanceLayout>,
}

/// Lays out the HUD for the scene as it stands, before anything draws.
pub(crate) fn layout(
    game: &crate::game::Scene<'_>,
    input: &InputState,
    bindings: &crate::action::BindingMap,
    env: HudEnv,
    performance: Option<&crate::performance::PerformanceView>,
    measure: Measure<'_>,
) -> Hud {
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    // A spectator commands nothing: no bank, unit count, or idle badge;
    // the viewer's transport bar is its own chrome. The minimap stays
    // clickable.
    let top_bar =
        (!game.presentation.spectate).then(|| chrome::top_bar_layout(game, bindings, env, measure));
    let panel = crate::panel::build_for_input(game, bindings, input);
    let panel_layout = panel
        .as_ref()
        .map(|panel| panel_draw::layout_panel(game, input, panel, env, measure));
    let geometry = panel
        .as_ref()
        .zip(panel_layout.as_ref())
        .map_or_else(panel_layout::PanelGeometry::empty, |(panel, layout)| {
            layout.geometry(panel)
        });
    let minimap = if geometry.hides_minimap {
        zero
    } else {
        minimap_rect_scaled(
            game.state.map().width(),
            game.state.map().height(),
            env.viewport,
            env.ui,
        )
    };
    let panel_regions = [geometry.info, geometry.actions];
    let panel_top = panel_regions
        .iter()
        .filter(|r| r.w > 0.0)
        .map(|r| r.y)
        .fold(f32::INFINITY, f32::min);
    let panel_right = panel_regions.iter().map(|r| r.x + r.w).fold(0.0, f32::max);
    let group_column = if game.presentation.spectate {
        None
    } else {
        chrome::group_column_layout(game, input, minimap, env.ui)
    };
    let ribbon = chrome::ribbon_layout(input, &panel_regions, minimap, env, measure);
    let queue_toggle = chrome::queue_toggle_shown(game, input, crate::platform::TOUCH_ONLY)
        .then(|| chrome::queue_toggle_rect(env.viewport, env.ui, &panel_regions));
    let performance = performance.and_then(|view| {
        performance::layout(
            view,
            env,
            top_bar.as_ref().map(|bar| bar.bar.status_space),
            measure,
        )
    });
    let mut group_slots = [None; crate::action::CONTROL_GROUPS];
    if let Some(column) = &group_column {
        for (published, (rect, slot, _)) in group_slots.iter_mut().zip(column.slots) {
            *published = Some((rect, slot));
        }
    }
    let layout = LayoutModel {
        performance: performance
            .as_ref()
            .map_or(zero, super::performance::PerformanceLayout::panel),
        top_bar_h: crate::layout::TOP_BAR_H * env.ui,
        panel_top,
        panel_right,
        panel_regions,
        orders: geometry.orders,
        minimap,
        idle_badge: top_bar
            .as_ref()
            .map_or(zero, chrome::TopBarLayout::idle_badge),
        alert_badge: top_bar
            .as_ref()
            .map_or(zero, chrome::TopBarLayout::alert_badge),
        group_column: group_column.as_ref().map_or(zero, |column| column.plate),
        group_slots,
        menu_button: top_bar.as_ref().map_or(zero, |bar| bar.bar.menu_button),
        pause_status: top_bar.as_ref().map_or(zero, |bar| bar.bar.pause_status),
        mode_ribbon: ribbon.as_ref().map_or(zero, |ribbon| ribbon.rect),
        queue_toggle: queue_toggle.unwrap_or(zero),
        roster_slots: geometry.roster_slots,
        roster_count: geometry.roster_count,
        cards: geometry.cards,
        card_count: geometry.card_count,
        queue_slots: geometry.queue_slots,
        queue_count: geometry.queue_count,
        queue_stop: geometry.queue_stop,
    };
    Hud {
        layout,
        panel,
        geometry: HudGeometry {
            top_bar,
            panel: panel_layout,
            group_column,
            ribbon,
            queue_toggle,
            performance,
        },
    }
}

/// Installs the hit regions and the panel model together, and returns
/// what drawing needs. Input looks a card up in the model by the slot it
/// hit, so the two must never come from different layouts.
pub(crate) fn publish(presentation: &crate::game::Presentation, hud: Hud) -> HudGeometry {
    presentation.layout.set(hud.layout);
    *presentation.panel_model.borrow_mut() = hud.panel;
    hud.geometry
}

/// Lays out the HUD and publishes it.
pub(crate) fn refresh(
    game: &crate::game::Scene<'_>,
    input: &InputState,
    bindings: &crate::action::BindingMap,
    env: HudEnv,
    performance: Option<&crate::performance::PerformanceView>,
    measure: Measure<'_>,
) -> HudGeometry {
    publish(
        game.presentation,
        layout(game, input, bindings, env, performance, measure),
    )
}

#[cfg(test)]
mod tests;
