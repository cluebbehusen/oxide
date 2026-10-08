use super::*;
use crate::saves::RecordKind;
use macroquad::prelude::vec2;

fn entry(name: &str, compatible: bool, kind: RecordKind, path: std::path::PathBuf) -> ReplayEntry {
    ReplayEntry {
        path,
        label: name.to_string(),
        blurb: format!("{name} blurb"),
        hint: String::new(),
        compatible,
        kind,
    }
}

fn drive(shelf: &mut Shelf, key: Key) -> Out {
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    shelf.update(
        &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
        &mut mouse,
        &mut sounds,
    )
}

/// Moves the cursor to the labeled row and activates it — by label,
/// so the tests survive the section headers shifting every index.
fn activate(shelf: &mut Shelf, label: &str) -> Out {
    let target = shelf
        .menu
        .items
        .iter()
        .position(|i| i == label)
        .unwrap_or_else(|| panic!("no row labeled {label} in {:?}", shelf.menu.items));
    while shelf.menu.selected < target {
        drive(shelf, Key::Down);
    }
    while shelf.menu.selected > target {
        drive(shelf, Key::Up);
    }
    drive(shelf, Key::Enter)
}

#[test]
fn the_back_button_leaves_even_after_every_record_is_deleted() {
    // The 0.9 regression, pinned structurally: however the shelf is
    // built — first open or post-delete rebuild — it has an exit.
    let mut shelf = Shelf::from_entries(vec![entry(
        "done",
        true,
        RecordKind::Match,
        "/nowhere/m.json".into(),
    )]);
    shelf.set_catalog(Vec::new());
    assert!(shelf.menu.items.is_empty());
    assert_eq!(drive(&mut shelf, Key::Enter), Out::Stay);
    for touch in [false, true] {
        let mut mouse = vec2(0.0, 0.0);
        let out = shelf.update(
            &crate::button::press_back(touch),
            &mut mouse,
            &mut Vec::new(),
        );
        assert_eq!(out, Out::Home);
    }
}

#[test]
fn a_record_says_how_to_act_only_as_coaching() {
    let mut record = entry("done", true, RecordKind::Match, "/nowhere/m.json".into());
    record.hint = "{confirm} watches | {delete} twice deletes".to_string();
    let mut shelf = Shelf::from_entries(vec![record]);
    assert_eq!(shelf.subtitle(), "done blurb", "the details stay visible");
    assert_eq!(
        shelf.coaching().as_deref(),
        Some("{confirm} watches | {delete} twice deletes")
    );
    shelf.arming = Some(shelf.menu.selected);
    assert!(
        shelf.subtitle().contains("again to delete"),
        "an armed delete says so"
    );
    assert_eq!(shelf.coaching(), None);
}

#[test]
fn records_shelve_into_their_sections_and_the_cursor_skips_the_headers() {
    let mut shelf = Shelf::from_entries(vec![
        entry("live", true, RecordKind::Autosave, "/nowhere/a.json".into()),
        entry("named", true, RecordKind::Save, "/nowhere/s.json".into()),
        entry("done", true, RecordKind::Match, "/nowhere/m.json".into()),
    ]);
    assert_eq!(
        shelf.menu.items,
        vec!["SAVES", "live", "named", "REPLAYS", "done"],
        "saves first, replays after"
    );
    assert!(shelf.menu.is_header(0) && shelf.menu.is_header(3));
    assert_eq!(shelf.menu.selected, 1, "the cursor opens on a real row");
    // Walking down never rests on the REPLAYS header.
    drive(&mut shelf, Key::Down);
    drive(&mut shelf, Key::Down);
    assert_eq!(shelf.menu.items[shelf.menu.selected], "done");
}

#[test]
fn enter_loads_a_save_and_watches_a_match() {
    let mut shelf = Shelf::from_entries(vec![
        entry("live", true, RecordKind::Autosave, "/nowhere/a.json".into()),
        entry("done", true, RecordKind::Match, "/nowhere/m.json".into()),
    ]);
    assert_eq!(
        activate(&mut shelf, "live"),
        Out::Load("/nowhere/a.json".into()),
        "a resumable record's verb is Load, never a mid-match scout"
    );
    assert_eq!(
        activate(&mut shelf, "done"),
        Out::Watch("/nowhere/m.json".into())
    );
}

#[test]
fn deletion_requires_two_presses_and_is_dispatched_to_the_owner() {
    let dir = std::env::temp_dir().join(format!("oxide-shelf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("doomed.json");
    std::fs::write(&path, "{}").unwrap();
    let mut shelf =
        Shelf::from_entries(vec![entry("doomed", true, RecordKind::Match, path.clone())]);
    assert_eq!(drive(&mut shelf, Key::X), Out::Stay, "first X only arms");
    assert!(path.exists(), "arming deletes nothing");
    assert_eq!(drive(&mut shelf, Key::X), Out::Delete(path.clone()));
    assert!(path.exists(), "the worker owns disk operations");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_incompatible_record_refuses_its_verb_in_both_sections() {
    let old_save = entry(
        "old-save",
        false,
        RecordKind::Save,
        "/nowhere/s.json".into(),
    );
    let old_match = entry(
        "old-match",
        false,
        RecordKind::Match,
        "/nowhere/m.json".into(),
    );
    let new = entry("new", true, RecordKind::Match, "/nowhere/new.json".into());
    let mut shelf = Shelf::from_entries(vec![old_save, old_match, new]);
    assert_eq!(
        activate(&mut shelf, "old-save"),
        Out::Stay,
        "an incompatible save refuses to load"
    );
    assert_eq!(
        activate(&mut shelf, "old-match"),
        Out::Stay,
        "an incompatible match refuses to watch"
    );
    assert_eq!(
        activate(&mut shelf, "new"),
        Out::Watch("/nowhere/new.json".into())
    );
}
