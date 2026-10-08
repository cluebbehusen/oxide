use super::*;
use crate::numeric;

const VIEWPORT: Vec2 = vec2(1280.0, 800.0);

#[test]
fn minimap_keeps_map_aspect_and_fits_its_budget() {
    for (w, h) in [(40, 24), (26, 16), (44, 20), (48, 30), (256, 256)] {
        let rect = minimap_rect_scaled(w, h, VIEWPORT, 1.0);
        assert!(rect.w <= MINIMAP_MAX.x + 0.01 && rect.h <= MINIMAP_MAX.y + 0.01);
        let map_aspect = w as f32 / h as f32;
        assert!(
            (rect.w / rect.h - map_aspect).abs() < 0.01,
            "{w}x{h} squished to {}x{}",
            rect.w,
            rect.h
        );
    }
}

#[test]
fn minimap_hugs_the_bottom_right_at_any_ui_scale() {
    for s in [1.0, 2.0] {
        let rect = minimap_rect_scaled(40, 24, VIEWPORT, s);
        assert_eq!(rect.x + rect.w, VIEWPORT.x - 12.0 * s);
        assert_eq!(rect.y + rect.h, VIEWPORT.y - 12.0 * s);
    }
}

#[test]
fn minimap_clicks_map_back_to_world_tiles() {
    let (map_w, map_h) = (40, 24);
    let rect = minimap_rect_scaled(map_w, map_h, VIEWPORT, 1.0);
    let scale = rect.w / map_w as f32;
    // The pixel at a tile center's minimap position maps back to it.
    for tile in [(0, 0), (20, 12), (39, 23)] {
        let screen = vec2(
            rect.x + (tile.0 as f32 + 0.5) * scale,
            rect.y + (tile.1 as f32 + 0.5) * scale,
        );
        let world = minimap_world_in(rect, map_w, screen).unwrap();
        assert_eq!(
            (
                numeric::to_i32(world.x.floor()),
                numeric::to_i32(world.y.floor())
            ),
            tile
        );
    }
    assert!(map_h as f32 * scale <= rect.h + 0.01);
}

#[test]
fn clicks_off_the_minimap_are_not_world_clicks() {
    let rect = minimap_rect_scaled(40, 24, VIEWPORT, 1.0);
    assert!(minimap_world_in(rect, 40, vec2(rect.x - 1.0, rect.y)).is_none());
    assert!(minimap_world_in(rect, 40, vec2(0.0, 0.0)).is_none());
}
