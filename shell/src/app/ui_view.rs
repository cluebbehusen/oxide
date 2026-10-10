//! The automation surface: what the window shows, as the debug protocol
//! reports it.

use super::{App, Screen};
use crate::menu::Menu;
use crate::render;
use crate::screens::results::ResultsScreen;
use macroquad::prelude::{Rect, screen_height};
use oxide_protocol::UiView;

/// A top-bar control's rect for the automation surface: reported only
/// during live play, and only while the bar draws it.
pub(super) fn live_rect(screen: &Screen, rect: Rect) -> Option<[f32; 4]> {
    (matches!(screen, Screen::Playing) && rect.w > 0.0).then_some([rect.x, rect.y, rect.w, rect.h])
}

/// The debug protocol's stable name for what the player is looking at,
/// which also keys the coaching clock.
pub(super) fn screen_mode(screen: &Screen) -> &'static str {
    match screen {
        Screen::Home(_) => "home",
        Screen::Settings { screen: sc, .. } => sc.mode_name(),
        Screen::Codex { screen: codex, .. } => codex.mode_name(),
        Screen::Wizard(w) => w.mode_name(),
        Screen::Playing => "playing",
        Screen::Playback(_) => "playback",
        Screen::FinalMap(_) => "final_map",
        Screen::Results(_) => "results",
        Screen::Replays(_) => "replays",
        Screen::Lobby { .. } => "lobby",
        Screen::Busy(busy) => busy.mode(),
        Screen::Pause(ps) => {
            if ps.saving_failed() {
                "save_failed"
            } else if ps.naming() {
                "save_name"
            } else if ps.confirming() {
                "confirm_pause"
            } else {
                "pause_menu"
            }
        }
    }
}

pub(super) fn capture_ui(screen: &Screen, app: &App) -> UiView {
    let (mode_name, menu): (&str, Option<&Menu>) = match screen {
        Screen::Home(home) => (screen_mode(screen), Some(&home.menu)),
        Screen::Settings { screen: sc, .. } => (screen_mode(screen), Some(&sc.menu)),
        Screen::Codex { screen: codex, .. } => (screen_mode(screen), Some(&codex.menu)),
        Screen::Wizard(w) => {
            // The wizard's custom screens (grid, setup) report the same
            // protocol surface the row menus do.
            let (title, items, selected) = w.ui_surface(&app.draft);
            // The frame injected this viewport before drawing, so the
            // range reports the grid window the player sees.
            let visible = w.ui_visible_range(&app.draft, render::viewport(), render::ui_scale());
            return UiView {
                mode: screen_mode(screen).to_string(),
                title: Some(title),
                selected: Some(selected),
                items,
                visible_range: Some(visible),
                hover: w.ui_hover(),
                chrome: None,
                panel_regions: None,
                menu_button: None,
                pause_status: None,
                group_column: None,
            };
        }
        Screen::Playing | Screen::Playback(_) | Screen::FinalMap(_) => (screen_mode(screen), None),
        Screen::Results(results) => {
            return UiView {
                mode: screen_mode(screen).to_string(),
                title: Some("MATCH RESULT".to_string()),
                selected: Some(results.selected()),
                items: ResultsScreen::items(),
                visible_range: Some([0, 4]),
                hover: results.hover(),
                chrome: None,
                panel_regions: None,
                menu_button: None,
                pause_status: None,
                group_column: None,
            };
        }
        Screen::Replays(shelf) => (screen_mode(screen), Some(&shelf.menu)),
        Screen::Lobby { screen: lobby, .. } => (screen_mode(screen), Some(&lobby.menu)),
        Screen::Busy(busy) => (screen_mode(screen), Some(&busy.menu)),
        Screen::Pause(ps) => (screen_mode(screen), Some(&ps.menu)),
    };
    UiView {
        mode: mode_name.to_string(),
        title: menu.map(|menu| menu.title.clone()),
        selected: menu.map(|menu| menu.selected),
        items: menu.map_or_else(Vec::new, |menu| menu.items.clone()),
        visible_range: menu.map(Menu::visible_range),
        hover: menu.and_then(Menu::hover),
        menu_button: live_rect(screen, app.game.presentation.layout.get().menu_button),
        pause_status: live_rect(screen, app.game.presentation.layout.get().pause_status),
        group_column: live_rect(screen, app.game.presentation.layout.get().group_column),
        panel_regions: matches!(screen, Screen::Playing).then(|| {
            app.game
                .presentation
                .layout
                .get()
                .panel_regions
                .map(|r| [r.x, r.y, r.w, r.h])
        }),
        chrome: matches!(screen, Screen::Playing).then(|| {
            let l = app.game.presentation.layout.get();
            let m = l.minimap;
            // JSON has no Infinity: an absent panel reports the window
            // bottom, a band no click can land in.
            let panel_top = if l.panel_top.is_finite() {
                l.panel_top
            } else {
                screen_height()
            };
            let o = l.orders;
            [
                l.top_bar_h,
                panel_top,
                m.x,
                m.y,
                m.w,
                m.h,
                l.panel_right,
                o.x,
                o.y,
                o.w,
                o.h,
            ]
        }),
    }
}
