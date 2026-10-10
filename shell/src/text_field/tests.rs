use super::*;
use macroquad::prelude::vec2;

fn feed(field: &mut TextField, events: &[RawEvent]) -> (Edit, Vec<SoundKind>) {
    let mut sounds = Vec::new();
    let edit = field.update(events, &mut sounds);
    (edit, sounds.into_iter().map(|(kind, _)| kind).collect())
}

fn text(value: &str) -> Vec<RawEvent> {
    value.chars().map(|ch| RawEvent::Text { ch }).collect()
}

fn key(key: Key) -> [RawEvent; 1] {
    [RawEvent::KeyDown { key }]
}

#[test]
fn the_field_edits_caps_and_commits_a_trimmed_value() {
    let mut field = TextField::new("JOIN MATCH", "JOIN", "ab\u{e9}c", 5);
    assert_eq!(
        field.menu(0).view().items,
        vec!["abc_"],
        "the prefill is filtered"
    );
    feed(&mut field, &text("def"));
    assert_eq!(
        field.menu(0).view().items,
        vec!["abcde_"],
        "the cap refuses more"
    );
    feed(&mut field, &key(Key::Backspace));
    feed(&mut field, &text(" "));
    assert_eq!(
        feed(&mut field, &key(Key::Enter)).0,
        Edit::Commit("abcd".to_owned())
    );
}

#[test]
fn a_blank_field_refuses_to_commit_and_escape_cancels() {
    let mut field = TextField::new("JOIN MATCH", "JOIN", "  ", 10);
    let (edit, sounds) = feed(&mut field, &key(Key::Enter));
    assert_eq!(edit, Edit::Stay);
    assert_eq!(sounds, vec![SoundKind::Denied]);
    assert_eq!(feed(&mut field, &key(Key::Escape)).0, Edit::Cancel);
}

#[test]
fn the_face_stays_above_an_ipad_keyboard() {
    for view in [
        vec2(1133.0, 744.0),
        vec2(1180.0, 820.0),
        vec2(1194.0, 834.0),
        vec2(1366.0, 1024.0),
    ] {
        for s in [1.0, 1.25, 1.5] {
            let layout = layout(view, s);
            for rect in [layout.field, layout.cancel, layout.confirm] {
                assert!(
                    rect.y + rect.h <= view.y * 0.45,
                    "{rect:?} reaches the keyboard at {view} ui {s}"
                );
            }
        }
    }
    let small = vec2(640.0, 400.0);
    let layout = layout(small, 1.0);
    for rect in [layout.field, layout.cancel, layout.confirm] {
        assert!(rect.x >= 0.0 && rect.x + rect.w <= small.x);
        assert!(rect.y >= layout.hint_y && rect.y + rect.h <= small.y);
    }
    assert!(!layout.cancel.overlaps(&layout.confirm));
    assert!(!layout.field.overlaps(&layout.confirm));
}
