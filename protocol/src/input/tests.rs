use super::*;
use crate::tests::assert_every_tag_sampled;

fn roundtrip(event: RawEvent) -> RawEvent {
    let json = serde_json::to_string(&event).unwrap();
    serde_json::from_str(&json).unwrap()
}

/// Contiguous index per [`RawEvent`] variant, in declaration order.
fn event_tag(event: &RawEvent) -> usize {
    match event {
        RawEvent::MouseMove { .. } => 0,
        RawEvent::MouseDown { .. } => 1,
        RawEvent::MouseUp { .. } => 2,
        RawEvent::Wheel { .. } => 3,
        RawEvent::KeyDown { .. } => 4,
        RawEvent::KeyUp { .. } => 5,
        RawEvent::TouchDown { .. } => 6,
        RawEvent::TouchMove { .. } => 7,
        RawEvent::TouchUp { .. } => 8,
        RawEvent::Text { .. } => 9,
    }
}

const EVENT_VARIANTS: usize = 10;

#[test]
fn every_raw_event_variant_including_touch_survives_a_roundtrip() {
    // Pins the wire contract for every variant, including the touch trio.
    let events = [
        RawEvent::MouseMove { x: 1.5, y: 2.5 },
        RawEvent::MouseDown {
            button: MouseButton::Left,
            x: 3.0,
            y: 4.0,
        },
        RawEvent::MouseUp {
            button: MouseButton::Right,
            x: 5.0,
            y: 6.0,
        },
        RawEvent::Wheel { delta: -2.0 },
        RawEvent::KeyDown { key: Key::Escape },
        RawEvent::KeyUp { key: Key::Shift },
        RawEvent::TouchDown {
            id: 7,
            x: 8.0,
            y: 9.0,
        },
        RawEvent::TouchMove {
            id: 7,
            x: 10.0,
            y: 11.0,
        },
        RawEvent::TouchUp {
            id: 7,
            x: 12.0,
            y: 13.0,
        },
        RawEvent::Text { ch: 'k' },
    ];
    assert_every_tag_sampled(events.iter().map(event_tag), EVENT_VARIANTS, "raw event");
    for event in events {
        assert_eq!(
            roundtrip(event),
            event,
            "raw event did not survive: {event:?}"
        );
    }
}

#[test]
fn the_middle_mouse_button_survives_a_roundtrip() {
    let event = RawEvent::MouseDown {
        button: MouseButton::Middle,
        x: 0.0,
        y: 0.0,
    };
    assert_eq!(roundtrip(event), event);
}
