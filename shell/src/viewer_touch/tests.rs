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
fn a_re_reported_start_does_not_duplicate_a_finger() {
    let mut camera = camera();
    let mut touch = ViewerTouch::default();
    touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
    touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
    let before = camera.center;
    touch.apply(&moved(1, 500.0, 400.0), &mut camera, 1.0);
    assert!(
        camera.center.x > before.x,
        "still one finger, so it still pans"
    );
}

#[test]
fn a_survivor_reported_lifted_resumes_panning_without_a_jump() {
    let mut camera = camera();
    let mut touch = ViewerTouch::default();
    touch.apply(&down(1, 600.0, 400.0), &mut camera, 1.0);
    touch.apply(&down(2, 700.0, 400.0), &mut camera, 1.0);
    touch.apply(
        &RawEvent::TouchUp {
            id: 1,
            x: 600.0,
            y: 400.0,
        },
        &mut camera,
        1.0,
    );
    touch.apply(
        &RawEvent::TouchUp {
            id: 2,
            x: 700.0,
            y: 400.0,
        },
        &mut camera,
        1.0,
    );
    let before = camera.center;
    touch.apply(&moved(2, 650.0, 400.0), &mut camera, 1.0);
    assert_eq!(camera.center, before, "the first move re-anchors");
    touch.apply(&moved(2, 600.0, 400.0), &mut camera, 1.0);
    assert!(camera.center.x > before.x);
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
