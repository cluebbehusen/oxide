use super::*;
use oxide_protocol::MouseButton;

#[test]
fn step_uses_the_presentation_preserving_protocol_method() {
    assert_eq!(
        live_requests(LiveCmd::Step { ticks: 7 }).unwrap(),
        vec![Request::PresentTicks { ticks: 7 }]
    );
}

#[test]
fn advance_units_speaks_the_zero_chase_sim_verb() {
    assert_eq!(
        live_requests(LiveCmd::AdvanceUnits {
            player: 1,
            units: vec![7, 3],
            to: "12,9".to_string(),
            queue: true,
        })
        .unwrap(),
        vec![Request::SendCommand {
            player: PlayerId(1),
            command: Command::Advance {
                units: vec![UnitId(7), UnitId(3)],
                goal: chassis::grid::TilePos::new(12, 9),
                queue: true,
            },
        }]
    );
}

#[test]
fn raw_commands_use_the_strict_debug_wire_boundary() {
    let request = live_requests(LiveCmd::Send {
        player: 1,
        json: r#"{"type":"stop","units":[7,3]}"#.to_string(),
    })
    .unwrap();
    assert_eq!(
        request,
        vec![Request::SendCommand {
            player: PlayerId(1),
            command: Command::Stop {
                units: vec![UnitId(7), UnitId(3)],
            },
        }]
    );

    let error = live_requests(LiveCmd::Send {
        player: 1,
        json: r#"{"type":"stop","units":[7],"unitz":[3]}"#.to_string(),
    })
    .expect_err("unknown command fields must fail before connecting");
    assert!(
        format!("{error:#}").contains("unknown field `unitz` in command"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn presented_capture_rejects_an_unbounded_interval_before_connecting() {
    let error = capture_sequence(
        "127.0.0.1:1",
        2,
        oxide_protocol::MAX_PRESENT_TICKS + 1,
        true,
        std::path::Path::new("not-created"),
    )
    .expect_err("the interval is invalid independently of a live shell");
    assert!(error.to_string().contains("at most 120"));
}

#[test]
fn a_chord_presses_in_order_and_releases_in_reverse() {
    let requests = live_requests(LiveCmd::InjectChord {
        keys: "ctrl+1".to_string(),
    })
    .unwrap();
    let events: Vec<&RawEvent> = requests
        .iter()
        .map(|r| match r {
            Request::InjectEvent { event } => event,
            other => panic!("chords are pure injections, got {other:?}"),
        })
        .collect();
    assert!(
        matches!(
            events[..],
            [
                RawEvent::KeyDown { key: Key::Ctrl },
                RawEvent::KeyDown { key: Key::Num1 },
                RawEvent::KeyUp { key: Key::Num1 },
                RawEvent::KeyUp { key: Key::Ctrl },
            ]
        ),
        "modifiers must wrap the core key: {events:?}"
    );
}

#[test]
fn a_chord_of_nonsense_fails_before_touching_the_socket() {
    assert!(
        live_requests(LiveCmd::InjectChord {
            keys: "ctrl+florb".to_string(),
        })
        .is_err()
    );
}

#[test]
fn drag_expands_to_press_moves_and_release() {
    let requests = live_requests(LiveCmd::InjectDrag {
        from: "10,20".to_string(),
        to: "40,50".to_string(),
        steps: 3,
        button: "left".to_string(),
    })
    .unwrap();
    assert_eq!(requests.len(), 5);
    assert_eq!(
        requests[0],
        Request::InjectEvent {
            event: RawEvent::MouseDown {
                button: MouseButton::Left,
                x: 10.0,
                y: 20.0,
            }
        }
    );
    assert_eq!(
        requests[2],
        Request::InjectEvent {
            event: RawEvent::MouseMove { x: 30.0, y: 40.0 }
        }
    );
    assert_eq!(
        requests[4],
        Request::InjectEvent {
            event: RawEvent::MouseUp {
                button: MouseButton::Left,
                x: 40.0,
                y: 50.0,
            }
        }
    );
}

#[test]
fn tap_and_touch_drag_expand_to_one_finger_gesture() {
    let tap = live_requests(LiveCmd::InjectTap { x: 5.0, y: 6.0 }).unwrap();
    assert_eq!(
        tap,
        vec![
            Request::InjectEvent {
                event: RawEvent::TouchDown {
                    id: INJECTED_FINGER,
                    x: 5.0,
                    y: 6.0,
                }
            },
            Request::InjectEvent {
                event: RawEvent::TouchUp {
                    id: INJECTED_FINGER,
                    x: 5.0,
                    y: 6.0,
                }
            },
        ]
    );
    let drag = live_requests(LiveCmd::InjectTouchDrag {
        from: "10,20".to_string(),
        to: "40,50".to_string(),
        steps: 3,
    })
    .unwrap();
    assert_eq!(drag.len(), 5);
    assert_eq!(
        drag[2],
        Request::InjectEvent {
            event: RawEvent::TouchMove {
                id: INJECTED_FINGER,
                x: 30.0,
                y: 40.0,
            }
        }
    );
    assert!(
        live_requests(LiveCmd::InjectTouchDrag {
            from: "0,0".to_string(),
            to: "1,1".to_string(),
            steps: 0,
        })
        .is_err()
    );
}

#[test]
fn drag_rejects_unbounded_event_counts() {
    let err = live_requests(LiveCmd::InjectDrag {
        from: "0,0".to_string(),
        to: "1,1".to_string(),
        steps: 121,
        button: "left".to_string(),
    })
    .unwrap_err();
    assert!(err.to_string().contains("1..=120"));
}

#[test]
fn every_protocol_key_is_cli_addressable() {
    fn canonical_spelling(key: Key) -> &'static str {
        match key {
            Key::Tab => "tab",
            Key::Up => "up",
            Key::Down => "down",
            Key::Left => "left",
            Key::Right => "right",
            Key::H => "h",
            Key::S => "s",
            Key::A => "a",
            Key::P => "p",
            Key::R => "r",
            Key::B => "b",
            Key::N => "n",
            Key::X => "x",
            Key::Enter => "enter",
            Key::Escape => "escape",
            Key::Space => "space",
            Key::F1 => "f1",
            Key::Shift => "shift",
            Key::Ctrl => "ctrl",
            Key::Num1 => "1",
            Key::Num2 => "2",
            Key::Num3 => "3",
            Key::Num4 => "4",
            Key::Num5 => "5",
            Key::Num6 => "6",
            Key::Num7 => "7",
            Key::Num8 => "8",
            Key::Num9 => "9",
            Key::PageUp => "pageup",
            Key::PageDown => "pagedown",
            Key::Home => "home",
            Key::End => "end",
            Key::F5 => "f5",
            Key::F6 => "f6",
            Key::F7 => "f7",
            Key::F8 => "f8",
            Key::C => "c",
            Key::D => "d",
            Key::E => "e",
            Key::F => "f",
            Key::G => "g",
            Key::I => "i",
            Key::J => "j",
            Key::K => "k",
            Key::L => "l",
            Key::M => "m",
            Key::O => "o",
            Key::Q => "q",
            Key::T => "t",
            Key::U => "u",
            Key::V => "v",
            Key::W => "w",
            Key::Y => "y",
            Key::Z => "z",
            Key::Backspace => "backspace",
        }
    }

    for key in [
        Key::Tab,
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::H,
        Key::S,
        Key::A,
        Key::P,
        Key::R,
        Key::B,
        Key::N,
        Key::X,
        Key::Enter,
        Key::Escape,
        Key::Space,
        Key::F1,
        Key::Shift,
        Key::Ctrl,
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
        Key::PageUp,
        Key::PageDown,
        Key::Home,
        Key::End,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::C,
        Key::D,
        Key::E,
        Key::F,
        Key::G,
        Key::I,
        Key::J,
        Key::K,
        Key::L,
        Key::M,
        Key::O,
        Key::Q,
        Key::T,
        Key::U,
        Key::V,
        Key::W,
        Key::Y,
        Key::Z,
        Key::Backspace,
    ] {
        assert_eq!(
            parse_key(canonical_spelling(key)).unwrap(),
            key,
            "CLI spelling did not round-trip {key:?}"
        );
    }
}
