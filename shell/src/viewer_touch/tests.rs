use super::*;

fn camera() -> Camera {
    Camera::new(vec2(32.0, 32.0), 64, 64, vec2(1280.0, 800.0))
}

fn down(id: u64, x: f32, y: f32) -> RawEvent {
    RawEvent::TouchDown { id, x, y }
}

fn moved(id: u64, x: f32, y: f32) -> RawEvent {
    RawEvent::TouchMove { id, x, y }
}

#[test]
fn one_finger_drags_the_world_under_the_hand() {
    let mut camera = camera();
    let mut touch = ViewerTouch::default();
    let before = camera.center;
    touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
    touch.apply(&moved(1, 500.0, 400.0), &mut camera, 1.0);
    assert!(
        camera.center.x > before.x,
        "dragging left reveals ground to the right"
    );
    assert_eq!(camera.center.y, before.y);
}

#[test]
fn a_spread_zooms_in_and_a_still_pair_does_not() {
    let mut camera = camera();
    let mut touch = ViewerTouch::default();
    let zoom = camera.zoom;
    touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
    touch.apply(&down(2, 700.0, 400.0), &mut camera, 1.0);
    touch.apply(&moved(2, 710.0, 400.0), &mut camera, 1.0);
    camera.update(1.0);
    assert_eq!(camera.zoom, zoom, "a small wobble is not a pinch");
    for x in [740.0, 780.0, 820.0] {
        touch.apply(&moved(2, x, 400.0), &mut camera, 1.0);
    }
    camera.update(1.0);
    assert!(camera.zoom > zoom, "spreading the pair zooms in");
}

#[test]
fn a_cancelled_finger_ends_the_pinch_and_its_partner_pans() {
    let mut camera = camera();
    let mut touch = ViewerTouch::default();
    touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
    touch.apply(&down(2, 700.0, 400.0), &mut camera, 1.0);
    for x in [740.0, 780.0, 820.0] {
        touch.apply(&moved(2, x, 400.0), &mut camera, 1.0);
    }
    assert!(touch.pinching, "premise: a pinch");
    touch.apply(&RawEvent::TouchCancel { id: 2 }, &mut camera, 1.0);
    camera.update(1.0);
    let (zoom, before) = (camera.zoom, camera.center);
    touch.apply(&moved(1, 500.0, 400.0), &mut camera, 1.0);
    camera.update(1.0);
    assert_eq!(camera.zoom, zoom, "no pinch without the partner");
    assert!(camera.center.x > before.x, "the partner pans alone");
}
