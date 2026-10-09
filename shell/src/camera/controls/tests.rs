use super::*;
use oxide_protocol::Key;

fn camera() -> Camera {
    Camera::new(vec2(100.0, 100.0), 200, 200, vec2(1280.0, 800.0))
}

#[test]
fn held_keys_pan_at_the_preferred_speed_in_any_view() {
    let mut resolver = ActionResolver::default();
    let bindings = crate::action::BindingMap::classic();
    resolver.key_edge_in(
        &bindings,
        Key::Right,
        true,
        crate::action::Context::FinalMap,
    );
    assert_eq!(held_pan(&resolver), vec2(1.0, 0.0));

    let mut slow = camera();
    let mut fast = camera();
    let start = slow.center;
    pan_toward(&mut slow, held_pan(&resolver), CameraPrefs::default(), 0.1);
    let quick = CameraPrefs {
        pan_speed: 2.0,
        ..CameraPrefs::default()
    };
    pan_toward(&mut fast, held_pan(&resolver), quick, 0.1);
    assert!(slow.center.x > start.x);
    assert!((fast.center.x - start.x) > (slow.center.x - start.x) * 1.9);

    let mut still = camera();
    pan_toward(&mut still, Vec2::ZERO, CameraPrefs::default(), 1.0);
    assert_eq!(still.center, start);
}

#[test]
fn the_wheel_honors_the_inverted_zoom_preference() {
    let at = vec2(640.0, 400.0);
    let mut plain = camera();
    let mut inverted = camera();
    wheel_zoom(&mut plain, at, 1.0, CameraPrefs::default());
    wheel_zoom(
        &mut inverted,
        at,
        1.0,
        CameraPrefs {
            zoom_inverted: true,
            ..CameraPrefs::default()
        },
    );
    plain.update(10.0);
    inverted.update(10.0);
    assert!(plain.zoom > camera().zoom);
    assert!(inverted.zoom < camera().zoom);
}

#[test]
fn a_viewer_steers_from_the_minimap_until_the_press_lets_go() {
    let mut hands = ViewerHands::default();
    let mut camera = camera();
    let mut mouse = Vec2::ZERO;
    let prefs = CameraPrefs::default();
    let on_map = |world: Vec2| MinimapPoint {
        under: Some(world),
        clamped: Some(world),
    };
    let press = RawEvent::MouseDown {
        button: MouseButton::Left,
        x: 1200.0,
        y: 700.0,
    };
    hands.pointer(
        &press,
        on_map(vec2(10.0, 10.0)),
        &mut camera,
        &mut mouse,
        prefs,
        1.0,
    );
    let pressed = camera.center;
    assert_eq!(mouse, vec2(1200.0, 700.0));

    let drag = RawEvent::MouseMove {
        x: 1210.0,
        y: 710.0,
    };
    hands.pointer(
        &drag,
        on_map(vec2(30.0, 20.0)),
        &mut camera,
        &mut mouse,
        prefs,
        1.0,
    );
    assert_ne!(camera.center, pressed, "a held press keeps steering");
    let steered = camera.center;

    let release = RawEvent::MouseUp {
        button: MouseButton::Left,
        x: 1210.0,
        y: 710.0,
    };
    hands.pointer(
        &release,
        MinimapPoint::default(),
        &mut camera,
        &mut mouse,
        prefs,
        1.0,
    );
    hands.pointer(
        &drag,
        on_map(vec2(5.0, 5.0)),
        &mut camera,
        &mut mouse,
        prefs,
        1.0,
    );
    assert_eq!(camera.center, steered, "a released press stops steering");
}

#[test]
fn a_viewer_middle_drag_pans_against_the_pointer() {
    let mut hands = ViewerHands::default();
    let mut camera = camera();
    let mut mouse = Vec2::ZERO;
    let prefs = CameraPrefs::default();
    let start = camera.center;
    for event in [
        RawEvent::MouseDown {
            button: MouseButton::Middle,
            x: 600.0,
            y: 400.0,
        },
        RawEvent::MouseMove { x: 500.0, y: 400.0 },
        RawEvent::MouseUp {
            button: MouseButton::Middle,
            x: 500.0,
            y: 400.0,
        },
        RawEvent::MouseMove { x: 300.0, y: 400.0 },
    ] {
        hands.pointer(
            &event,
            MinimapPoint::default(),
            &mut camera,
            &mut mouse,
            prefs,
            1.0,
        );
    }
    let moved = camera.center.x - start.x;
    assert!(
        (moved - 100.0 / camera.zoom).abs() < 1e-4,
        "dragging left by 100px pans right by 100px of world, then release stops it"
    );
}
