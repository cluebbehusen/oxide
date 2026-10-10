use super::*;
use oxide_protocol::MouseButton;
use oxide_sim::Scenario;

#[test]
fn final_map_accepts_only_camera_navigation_and_escape() {
    let viewport = vec2(1280.0, 800.0);
    let mut game = Game::with_viewport(Scenario::skirmish(), viewport).expect("game");
    let mut screen = FinalMapScreen::open();
    let mut mouse = Vec2::ZERO;
    let before = game.presentation.camera.center;
    assert!(!screen.update(
        &crate::action::BindingMap::classic(),
        &[RawEvent::KeyDown { key: Key::Right }],
        0.25,
        crate::config::CameraPrefs::default(),
        &mut mouse,
        &mut game,
    ));
    assert!(game.presentation.camera.center.x > before.x);
    assert!(screen.update(
        &crate::action::BindingMap::classic(),
        &[RawEvent::KeyDown { key: Key::Escape }],
        0.0,
        crate::config::CameraPrefs::default(),
        &mut mouse,
        &mut game,
    ));
    assert_eq!(
        game.state.current_tick(),
        0,
        "camera input never ticks the sim"
    );
}

#[test]
fn the_back_button_returns_to_the_report_by_click_and_by_tap() {
    let viewport = vec2(1280.0, 800.0);
    let mut game = Game::with_viewport(Scenario::skirmish(), viewport).expect("game");
    let back = crate::button::corner_slot(0, render::ui_scale()).center();
    let prefs = crate::config::CameraPrefs::default();
    let mut mouse = Vec2::ZERO;
    let tap = [
        RawEvent::TouchDown {
            id: 1,
            x: back.x,
            y: back.y,
        },
        RawEvent::TouchUp {
            id: 1,
            x: back.x,
            y: back.y,
        },
    ];
    let mut screen = FinalMapScreen::open();
    assert!(screen.update(
        &crate::action::BindingMap::classic(),
        &tap,
        0.0,
        prefs,
        &mut mouse,
        &mut game
    ));
    let click = [
        RawEvent::MouseDown {
            button: MouseButton::Left,
            x: back.x,
            y: back.y,
        },
        RawEvent::MouseUp {
            button: MouseButton::Left,
            x: back.x,
            y: back.y,
        },
    ];
    let mut screen = FinalMapScreen::open();
    assert!(screen.update(
        &crate::action::BindingMap::classic(),
        &click,
        0.0,
        prefs,
        &mut mouse,
        &mut game
    ));
}

#[test]
fn touch_camera_gestures_never_tick_the_sim() {
    let viewport = vec2(1280.0, 800.0);
    let mut game = Game::with_viewport(Scenario::skirmish(), viewport).expect("game");
    let mut screen = FinalMapScreen::open();
    let mut mouse = Vec2::ZERO;
    let before = game.presentation.camera.center;
    let drag = [
        RawEvent::TouchDown {
            id: 1,
            x: 640.0,
            y: 300.0,
        },
        RawEvent::TouchMove {
            id: 1,
            x: 540.0,
            y: 300.0,
        },
        RawEvent::TouchUp {
            id: 1,
            x: 540.0,
            y: 300.0,
        },
    ];
    assert!(!screen.update(
        &crate::action::BindingMap::classic(),
        &drag,
        0.0,
        crate::config::CameraPrefs::default(),
        &mut mouse,
        &mut game,
    ));
    assert!(game.presentation.camera.center.x > before.x);
    assert_eq!(game.state.current_tick(), 0);
    assert!(game.pending.is_empty());
}

#[test]
fn minimap_drag_and_key_release_have_explicit_camera_lifetimes() {
    let viewport = vec2(1280.0, 800.0);
    let mut game = Game::with_viewport(Scenario::skirmish(), viewport).expect("game");
    let mut screen = FinalMapScreen::open();
    let mut mouse = Vec2::ZERO;
    let rect = render::minimap_rect(&game.view());
    let mut layout = game.presentation.layout.get();
    layout.minimap = rect;
    game.presentation.layout.set(layout);
    let press = vec2(rect.x + 2.0, rect.y + 2.0);
    let expected = render::minimap_world_at(&game.view(), press).expect("inside minimap");
    let original = game.presentation.camera.center;
    game.presentation.camera.center = expected;
    game.presentation.camera.pan(Vec2::ZERO);
    let expected = game.presentation.camera.center;
    game.presentation.camera.center = original;
    game.presentation.camera.pan(Vec2::ZERO);

    screen.update(
        &crate::action::BindingMap::classic(),
        &[RawEvent::MouseDown {
            button: MouseButton::Left,
            x: press.x,
            y: press.y,
        }],
        0.0,
        crate::config::CameraPrefs::default(),
        &mut mouse,
        &mut game,
    );
    assert!((game.presentation.camera.center - expected).length() < 0.1);

    let dragged = vec2(rect.x + rect.w + 100.0, rect.y + rect.h + 100.0);
    let clamped = vec2(rect.x + rect.w, rect.y + rect.h);
    let expected_after_drag =
        render::minimap_world_at(&game.view(), clamped).expect("clamped inside minimap");
    let after_press = game.presentation.camera.center;
    game.presentation.camera.center = expected_after_drag;
    game.presentation.camera.pan(Vec2::ZERO);
    let expected_after_drag = game.presentation.camera.center;
    game.presentation.camera.center = after_press;
    game.presentation.camera.pan(Vec2::ZERO);
    screen.update(
        &crate::action::BindingMap::classic(),
        &[RawEvent::MouseMove {
            x: dragged.x,
            y: dragged.y,
        }],
        0.0,
        crate::config::CameraPrefs::default(),
        &mut mouse,
        &mut game,
    );
    let after_drag = game.presentation.camera.center;
    assert!((after_drag - expected_after_drag).length() < 0.1);
    assert_ne!(
        after_drag, after_press,
        "a held minimap drag must steer away from its press target"
    );
    screen.update(
        &crate::action::BindingMap::classic(),
        &[
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x: dragged.x,
                y: dragged.y,
            },
            RawEvent::MouseMove {
                x: press.x,
                y: press.y,
            },
        ],
        0.0,
        crate::config::CameraPrefs::default(),
        &mut mouse,
        &mut game,
    );
    assert_eq!(
        game.presentation.camera.center, after_drag,
        "released drags stop steering"
    );

    screen.update(
        &crate::action::BindingMap::classic(),
        &[
            RawEvent::KeyDown { key: Key::Left },
            RawEvent::KeyUp { key: Key::Left },
        ],
        0.25,
        crate::config::CameraPrefs::default(),
        &mut mouse,
        &mut game,
    );
    assert_eq!(
        game.presentation.camera.center, after_drag,
        "released keys do not pan"
    );
}
#[test]
fn final_map_keeps_the_secondary_camera_hold_after_releasing_a_rebound_primary() {
    use crate::action::Chord;
    let mut game =
        Game::with_viewport(oxide_sim::Scenario::skirmish(), vec2(640.0, 400.0)).unwrap();
    game.presentation.camera.center = vec2(18.0, 10.0);
    let mut screen = FinalMapScreen::open();
    let mut bindings = crate::action::BindingMap::classic();
    assert!(bindings.rebind(Action::PanRight, Chord::bare(Key::L)));
    let mut mouse = Vec2::ZERO;
    let prefs = crate::config::CameraPrefs::default();
    screen.update(
        &bindings,
        &[
            RawEvent::KeyDown { key: Key::L },
            RawEvent::KeyDown { key: Key::Right },
        ],
        0.1,
        prefs,
        &mut mouse,
        &mut game,
    );
    let halfway = game.presentation.camera.center.x;
    screen.update(
        &bindings,
        &[RawEvent::KeyUp { key: Key::L }],
        0.1,
        prefs,
        &mut mouse,
        &mut game,
    );
    let after = game.presentation.camera.center.x;
    assert!(after > halfway);
    screen.update(
        &bindings,
        &[RawEvent::KeyUp { key: Key::Right }],
        0.1,
        prefs,
        &mut mouse,
        &mut game,
    );
    assert_eq!(game.presentation.camera.center.x, after);
    assert!(game.pending.is_empty());
}
