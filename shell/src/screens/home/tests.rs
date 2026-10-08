use super::*;
use macroquad::prelude::vec2;
use oxide_protocol::Key;

fn pick(home: &mut HomeScreen, row: usize) -> Out {
    home.menu.select(row);
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    home.update(
        &[
            RawEvent::KeyDown { key: Key::Enter },
            RawEvent::KeyUp { key: Key::Enter },
        ],
        &mut mouse,
        &mut sounds,
    )
}

#[test]
fn rows_mean_the_same_verbs_with_and_without_a_continue_row() {
    // The row shift is where a blind index goes wrong (the battery
    // once resumed a match instead of opening the map list).
    let mut fresh = HomeScreen::with_resumable(false);
    assert_eq!(pick(&mut fresh, 0), Out::Play);
    assert_eq!(pick(&mut fresh, 1), Out::Join);
    assert_eq!(pick(&mut fresh, 3), Out::Replays);
    assert_eq!(pick(&mut fresh, 4), Out::Roster);
    assert_eq!(pick(&mut fresh, 5), Out::Settings);
    assert_eq!(pick(&mut fresh, 6), Out::Quit);

    let mut resumable = HomeScreen::with_resumable(true);
    assert_eq!(pick(&mut resumable, 0), Out::Continue);
    assert_eq!(pick(&mut resumable, 1), Out::Play);
    assert_eq!(pick(&mut resumable, 5), Out::Roster);
    assert_eq!(pick(&mut resumable, 7), Out::Quit);
}
#[test]
fn a_touch_only_door_offers_no_quit_and_keeps_its_verbs() {
    let mut door = HomeScreen::build(true, false);
    assert!(!door.menu.items.iter().any(|item| item == "Quit"));
    assert_eq!(pick(&mut door, 0), Out::Continue);
    assert_eq!(pick(&mut door, 6), Out::Settings);
    assert_eq!(door.rows.len(), door.menu.items.len());
}

#[test]
fn recovery_is_explicit_and_does_not_change_continue_or_play() {
    for resumable in [false, true] {
        let mut home = HomeScreen::with_resumable(resumable).with_recovery(Some(
            oxide_kit::recovery::InterruptedMatch {
                directory: "/unused".into(),
                ticks: 2400,
                scenario: "test".into(),
            },
        ));
        assert!(home.menu.items[0].contains("02:00"));
        assert_eq!(pick(&mut home, 0), Out::Recover);
        assert_eq!(
            pick(&mut home, 1),
            if resumable { Out::Continue } else { Out::Play }
        );
        assert_eq!(pick(&mut home, usize::from(resumable) + 6), Out::Settings);
    }
}
