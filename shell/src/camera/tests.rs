use super::*;

fn camera() -> Camera {
    Camera::new(vec2(20.0, 12.0), 40, 24, vec2(1280.0, 800.0))
}

#[test]
fn screen_world_roundtrip() {
    let cam = camera();
    for point in [vec2(0.0, 0.0), vec2(640.0, 400.0), vec2(1279.0, 799.0)] {
        let world = cam.to_world(point);
        let back = cam.to_screen(world);
        assert!((back - point).length() < 1e-3, "{point:?} -> {back:?}");
    }
}

#[test]
fn zoom_keeps_the_cursor_anchor_fixed() {
    let mut cam = camera();
    let cursor = vec2(300.0, 250.0);
    let before = cam.to_world(cursor);
    cam.zoom_at(cursor, 2.0);
    cam.update(1.0); // saturated step: glide finishes instantly
    let after = cam.to_world(cursor);
    assert!(
        (after - before).length() < 1e-3,
        "anchor drifted: {before:?} -> {after:?}"
    );
    assert!(cam.zoom > 32.0);
}

#[test]
fn zoom_clamps_to_bounds() {
    let mut cam = camera();
    cam.zoom_at(vec2(0.0, 0.0), 100.0);
    cam.update(1.0);
    assert_eq!(cam.zoom, ZOOM_MAX);
    cam.zoom_at(vec2(0.0, 0.0), -100.0);
    cam.update(1.0);
    assert_eq!(cam.zoom, ZOOM_MIN);
}

#[test]
fn pan_clamps_to_map_with_slack() {
    let mut cam = camera();
    cam.pan(vec2(-1000.0, -1000.0));
    let lo = cam.center;
    cam.pan(vec2(2000.0, 2000.0));
    let hi = cam.center;
    assert!(lo.x < hi.x && lo.y < hi.y);
    // Slack allows at most two tiles beyond the edges.
    let (min, _) = {
        cam.center = lo;
        cam.world_rect()
    };
    assert!(min.x >= -3.0 && min.y >= -3.0, "runaway clamp: {min:?}");
}

#[test]
fn resize_reclamps() {
    let mut cam = camera();
    cam.pan(vec2(1000.0, 1000.0)); // pinned at the bottom-right clamp
    let before = cam.center;
    cam.set_viewport(vec2(640.0, 400.0)); // smaller window: clamp loosens
    cam.pan(vec2(1000.0, 1000.0));
    assert!(cam.center.x >= before.x && cam.center.y >= before.y);
}

#[test]
fn a_map_smaller_than_the_viewport_pins_the_view_to_center() {
    // A 4x4 map is dwarfed by the 1280x800 viewport at default zoom, so
    // there is no room to pan: the clamp holds the center on the map's
    // midpoint however hard it is shoved.
    let mut cam = Camera::new(vec2(2.0, 2.0), 4, 4, vec2(1280.0, 800.0));
    let mid = vec2(2.0, 2.0);
    assert!((cam.center - mid).length() < 1e-4);
    for shove in [
        vec2(-500.0, -500.0),
        vec2(500.0, 500.0),
        vec2(500.0, -500.0),
    ] {
        cam.pan(shove);
        assert!(
            (cam.center - mid).length() < 1e-4,
            "pinned center drifted to {:?}",
            cam.center
        );
    }
}

#[test]
fn the_cursor_anchor_holds_through_a_multi_step_zoom_glide() {
    // The existing anchor test saturates the glide in one step; this
    // pins that every *intermediate* frame keeps the world point under
    // the cursor stationary, not just the final one.
    let mut cam = camera();
    let cursor = vec2(800.0, 500.0);
    let anchored = cam.to_world(cursor);
    cam.zoom_at(cursor, 1.0);
    for _ in 0..30 {
        cam.update(1.0 / 60.0); // small dt: the glide is mid-flight here
        assert!(
            (cam.to_world(cursor) - anchored).length() < 1e-2,
            "anchor drifted mid-glide to {:?}",
            cam.to_world(cursor)
        );
    }
    assert!(
        (cam.zoom - ZOOM_DEFAULT * 1.15).abs() < 0.01,
        "glide never reached its one-notch target"
    );
}

#[test]
fn panning_past_the_far_corner_stops_within_slack() {
    // `pan_clamps_to_map_with_slack` pins the lo corner; this pins the
    // hi corner. Shoved past the bottom-right, the visible rect reaches
    // the map edge but overshoots it by at most the two-tile slack.
    let mut cam = camera();
    cam.pan(vec2(1000.0, 1000.0));
    let (_, max) = cam.world_rect();
    assert!(
        max.x >= 40.0 && max.y >= 24.0,
        "clamp fell short of the map edge: {max:?}"
    );
    assert!(
        max.x <= 40.0 + 3.0 && max.y <= 24.0 + 3.0,
        "far edge ran away: {max:?}"
    );
}
