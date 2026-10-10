use super::*;
use macroquad::prelude::vec2;

#[test]
fn selection_corner_owns_its_arms_but_not_the_open_notch() {
    let model = LayoutModel {
        top_bar_h: 40.0,
        panel_regions: [
            Rect::new(0.0, 500.0, 228.0, 300.0),
            Rect::new(228.0, 724.0, 500.0, 76.0),
        ],
        orders: Rect::new(0.0, 388.0, 204.0, 112.0),
        ..LayoutModel::default()
    };
    assert!(model.chrome_owns(vec2(100.0, 600.0)));
    assert!(model.chrome_owns(vec2(400.0, 760.0)));
    assert!(model.chrome_owns(vec2(100.0, 410.0)));
    assert!(!model.chrome_owns(vec2(400.0, 600.0)));
    assert!(!model.chrome_owns(vec2(229.0, 723.0)));
    assert!(!model.chrome_owns(vec2(800.0, 760.0)));
}

#[test]
fn card_hits_pad_for_fingertips_and_search_roster_first() {
    let mut model = LayoutModel::default();
    let chip = Rect::new(100.0, 500.0, 24.0, 24.0);
    let card = Rect::new(200.0, 500.0, 60.0, 60.0);
    model.roster_slots[0] = (chip, CardAction::FilterKind(oxide_sim::UnitKind::Harvester));
    model.roster_count = 1;
    model.cards[0] = (Rect::new(0.0, 0.0, 0.0, 0.0), CardAction::Upgrade);
    model.cards[1] = (card, CardAction::ArmRally);
    model.card_count = 2;
    model.cards[2] = (Rect::new(300.0, 500.0, 60.0, 60.0), CardAction::ClearRally);

    let beside_chip = vec2(chip.right() + 5.0, chip.center().y);
    assert_eq!(
        card_under(&model, beside_chip, None),
        None,
        "a mouse hits the drawn chip"
    );
    let hit = card_under(&model, beside_chip, Some(1.0)).expect("a fingertip reaches the pad");
    assert_eq!((hit.row, hit.index, hit.rect), (CardRow::Roster, 0, chip));

    let hit = card_under(&model, card.center(), None).expect("the live card");
    assert_eq!(
        (hit.row, hit.index, hit.action),
        (CardRow::Cards, 1, CardAction::ArmRally)
    );
    assert_eq!(
        card_under(&model, vec2(0.0, 0.0), Some(1.0)),
        None,
        "zero-size slots are empty"
    );
    assert_eq!(
        card_under(&model, vec2(330.0, 530.0), None),
        None,
        "past the live count"
    );
}

/// A 1280x800 layout whose only chrome is the top bar and a full-width
/// band from `panel_top` down.
fn compute_at(panel_top: f32, ui: f32) -> LayoutModel {
    LayoutModel {
        top_bar_h: TOP_BAR_H * ui,
        panel_top,
        panel_right: 1280.0,
        panel_regions: [
            Rect::new(0.0, panel_top, 1280.0, (800.0 - panel_top).max(0.0)),
            Rect::new(0.0, 0.0, 0.0, 0.0),
        ],
        ..LayoutModel::default()
    }
}

#[test]
fn the_panel_band_owns_exactly_below_its_top() {
    let m = compute_at(700.0, 1.0);
    assert!(m.chrome_owns(vec2(600.0, 770.0)));
    assert!(m.chrome_owns(vec2(600.0, 700.0)));
    assert!(
        !m.chrome_owns(vec2(600.0, 699.0)),
        "the band must not swallow the midfield"
    );
}

#[test]
fn the_band_owns_its_width_and_the_dock_its_rect() {
    let mut m = compute_at(700.0, 1.0);
    m.panel_right = 600.0;
    m.panel_regions[0].w = 600.0;
    m.orders = Rect::new(0.0, 500.0, 60.0, 200.0);
    assert!(m.chrome_owns(vec2(400.0, 750.0)), "inside the band");
    assert!(
        !m.chrome_owns(vec2(900.0, 750.0)),
        "right of a content-width band is world, not chrome"
    );
    assert!(
        m.chrome_owns(vec2(30.0, 600.0)),
        "the orders dock is chrome"
    );
    assert!(
        !m.chrome_owns(vec2(100.0, 600.0)),
        "beside the dock stays world"
    );
}

#[test]
fn no_panel_means_no_band_at_all() {
    let m = compute_at(f32::INFINITY, 2.0);
    assert!(!m.chrome_owns(vec2(600.0, 799.0)));
    assert!(m.chrome_owns(vec2(600.0, 30.0)), "the top bar always owns");
}

#[test]
fn an_armed_mode_ribbon_owns_its_world_pixels() {
    let mut m = compute_at(f32::INFINITY, 1.0);
    m.mode_ribbon = Rect::new(220.0, 640.0, 280.0, MIN_TOUCH_TARGET);
    m.queue_toggle = Rect::new(116.0, 640.0, 96.0, MIN_TOUCH_TARGET);
    assert!(m.chrome_owns(m.mode_ribbon.center()));
    assert!(
        m.chrome_owns(m.queue_toggle.center()),
        "QUEUE is chrome too"
    );
    assert!(
        !m.chrome_owns(vec2(219.0, 660.0)),
        "beside the ribbon remains battlefield"
    );
}

#[test]
fn the_menu_button_sits_in_the_top_bar_with_a_full_touch_target() {
    for ui in [0.75, 1.0, 1.25, 1.5] {
        for width in [640.0, 1133.0, 1280.0, 1920.0] {
            for touch_only in [false, true] {
                let button = menu_button_rect(width, ui, touch_only);
                assert!(
                    button.y >= 0.0 && button.y + button.h <= TOP_BAR_H * ui,
                    "the drawn button stays inside the bar at {width}px, ui {ui}"
                );
                let pad = touch_pad(button, ui);
                assert!(pad.w >= MIN_TOUCH_TARGET * ui && pad.h >= MIN_TOUCH_TARGET * ui);
                assert!(
                    pad.x + pad.w <= width,
                    "the fingertip target stays on screen at {width}px, ui {ui}"
                );
                if touch_only {
                    assert!(
                        button.x + button.w <= width - TOUCH_RIGHT_INSET * ui,
                        "clear of a rounded screen corner"
                    );
                }
            }
        }
    }
}

/// Typical desktop widths at 1x: a four-digit bank, a two-digit
/// income, and a PAUSED status.
fn sample_text(ui: f32) -> TopBarText {
    TopBarText {
        scrap: 44.0 * ui,
        passive: 120.0 * ui,
        units_label: 38.0 * ui,
        units: 24.0 * ui,
        idle: Some(70.0 * ui),
        alert: None,
        status: 76.0 * ui,
    }
}

#[test]
fn the_desktop_top_bar_keeps_its_legacy_positions() {
    let bar = top_bar(1280.0, 1.0, false, sample_text(1.0));
    assert_eq!((bar.scrap_label_x, bar.scrap_x), (12.0, 70.0));
    assert_eq!(bar.passive_x, 151.0, "a short bank keeps the minimum");
    assert_eq!(
        bar.units_x,
        151.0 + 120.0 + 16.0,
        "a long income pushes the count"
    );
    assert_eq!(bar.count_x, bar.units_x + 60.0);
    assert_eq!(
        bar.idle_badge,
        Rect::new(bar.count_x + 45.0, 3.0, 88.0, 34.0)
    );
    assert_eq!(bar.menu_button, Rect::new(1238.0, 3.0, 34.0, 34.0));
    assert_eq!(bar.status_x, 1238.0 - 12.0 - 76.0);
    assert_eq!(
        bar.pause_status,
        Rect::new(bar.status_x - 6.0, 3.0, 88.0, 34.0)
    );
    assert_eq!(
        bar.status_space,
        (bar.idle_badge.x + bar.idle_badge.w, bar.status_x)
    );
    let touch = top_bar(1280.0, 1.0, true, sample_text(1.0));
    assert_eq!(
        touch.scrap_label_x, 18.0,
        "touch holds the bank off the corner"
    );
    assert_eq!(touch.menu_button.x, 1280.0 - 34.0 - 20.0);
    let no_idle = top_bar(
        1280.0,
        1.0,
        false,
        TopBarText {
            idle: None,
            ..sample_text(1.0)
        },
    );
    assert_eq!(no_idle.idle_badge, Rect::new(0.0, 0.0, 0.0, 0.0));
    assert_eq!(no_idle.status_space.0, no_idle.count_x + 24.0);
}

#[test]
fn the_group_column_stacks_above_the_minimap_and_wraps_when_short() {
    for (viewport, touch_only) in [
        (vec2(1280.0, 800.0), false),
        (vec2(1194.0, 834.0), true),
        (vec2(1133.0, 744.0), true),
    ] {
        let minimap = crate::render::minimap_rect_scaled(64, 40, viewport, 1.0);
        let column = group_column(1.0, touch_only, minimap).expect("room above the minimap");
        assert!(
            column.slots.iter().all(|slot| slot.x == column.slots[0].x),
            "{viewport}: one column"
        );
        assert!(
            column.slots.windows(2).all(|pair| pair[0].y < pair[1].y),
            "group 1 on top"
        );
        assert_eq!(column.plate.x + column.plate.w, minimap.x + minimap.w);
        assert!(
            column.plate.y + column.plate.h < minimap.y,
            "above the minimap"
        );
        assert!(column.plate.y >= TOP_BAR_H, "below the top bar");
        for slot in column.slots {
            assert!(column.plate.contains(slot.center()));
            if touch_only {
                assert!(slot.w >= MIN_TOUCH_TARGET && slot.h >= MIN_TOUCH_TARGET);
            }
        }
    }
    let small = vec2(640.0, 400.0);
    let minimap = crate::render::minimap_rect_scaled(64, 40, small, 1.0);
    let column = group_column(1.0, true, minimap).expect("wrapped");
    assert!(
        column.slots.iter().any(|slot| slot.x != column.slots[0].x),
        "wraps"
    );
    assert!(column.plate.y >= TOP_BAR_H);
    assert!(column.plate.x >= 0.0);
    assert!(
        group_column(1.0, true, Rect::new(0.0, 0.0, 0.0, 0.0)).is_none(),
        "no minimap, no column"
    );
}

#[test]
fn the_alert_badge_follows_the_idle_badge_or_takes_its_place() {
    let text = TopBarText {
        alert: Some(90.0),
        ..sample_text(1.0)
    };
    let bar = top_bar(1280.0, 1.0, false, text);
    assert_eq!(bar.alert_badge.x, bar.idle_badge.x + bar.idle_badge.w + 8.0);
    assert_eq!(bar.alert_badge.w, 108.0);
    let alone = top_bar(1280.0, 1.0, false, TopBarText { idle: None, ..text });
    assert_eq!(alone.alert_badge.x, bar.idle_badge.x, "no idle, same slot");
    let quiet = top_bar(1280.0, 1.0, false, sample_text(1.0));
    assert_eq!(quiet.alert_badge, Rect::new(0.0, 0.0, 0.0, 0.0));
    assert_eq!(
        quiet.idle_badge, bar.idle_badge,
        "an alert moves nothing else"
    );
}

#[test]
fn the_top_bar_keeps_its_groups_in_order_and_on_screen() {
    for ui in [0.75, 1.0, 1.25, 1.5] {
        for width in [640.0, 1024.0, 1133.0, 1194.0, 1280.0, 1920.0] {
            for touch_only in [false, true] {
                let bar = top_bar(width, ui, touch_only, sample_text(ui));
                assert!(bar.scrap_label_x < bar.scrap_x);
                assert!(bar.scrap_x < bar.passive_x && bar.passive_x < bar.units_x);
                assert!(bar.units_x < bar.count_x && bar.count_x < bar.idle_badge.x);
                assert!(bar.pause_status.x + bar.pause_status.w <= bar.menu_button.x);
                let pad = touch_pad(bar.menu_button, ui);
                assert!(pad.x + pad.w <= width, "{width}px @{ui} touch {touch_only}");
            }
        }
    }
}

const VIEW: Vec2 = Vec2::new(1280.0, 800.0);

#[test]
fn a_dock_tooltip_tracks_the_chip_it_describes() {
    // Two chips of a full dock, hundreds of px apart: each tooltip
    // centers on its own chip, not on the band.
    let size = Vec2::new(240.0, 90.0);
    let top = Rect::new(8.0, 300.0, 44.0, 44.0);
    let bottom = Rect::new(8.0, 620.0, 44.0, 44.0);
    let a = tooltip_origin(top, size, TooltipSide::RightOf, VIEW, 32.0, 6.0);
    let b = tooltip_origin(bottom, size, TooltipSide::RightOf, VIEW, 32.0, 6.0);
    assert_eq!(a.x, 58.0, "clear of the dock, not under the pointer");
    assert_eq!(a.y + size.y * 0.5, top.y + top.h * 0.5);
    assert_eq!(b.y + size.y * 0.5, bottom.y + bottom.h * 0.5);
}

#[test]
fn a_tall_tooltip_stops_at_the_top_bar() {
    // A five-line tooltip raised from a chip near the top of a
    // short window: the header must stay readable, so the box pins
    // below the bar instead of going negative.
    let chip = Rect::new(8.0, 90.0, 44.0, 44.0);
    let size = Vec2::new(240.0, 200.0);
    let o = tooltip_origin(
        chip,
        size,
        TooltipSide::RightOf,
        Vec2::new(1024.0, 400.0),
        32.0,
        6.0,
    );
    assert_eq!(o.y, 38.0, "top bar + gap");
    // Taller than the window has room for: still the top, never a
    // max that fell below the min.
    let o = tooltip_origin(
        chip,
        Vec2::new(240.0, 900.0),
        TooltipSide::RightOf,
        Vec2::new(1024.0, 400.0),
        32.0,
        6.0,
    );
    assert_eq!(o.y, 38.0);
}

#[test]
fn a_wide_tooltip_slides_back_inside_the_window() {
    let card = Rect::new(1150.0, 700.0, 66.0, 80.0);
    let size = Vec2::new(300.0, 60.0);
    let o = tooltip_origin(card, size, TooltipSide::Above, VIEW, 32.0, 6.0);
    assert_eq!(o.x, 974.0, "1280 - 300 - 6");
    assert_eq!(o.y, 634.0, "above the card, not above the band");
    // Wider than the window: pinned left, never a negative origin.
    let o = tooltip_origin(
        card,
        Vec2::new(2000.0, 60.0),
        TooltipSide::Above,
        VIEW,
        32.0,
        6.0,
    );
    assert_eq!(o.x, 0.0);
}
