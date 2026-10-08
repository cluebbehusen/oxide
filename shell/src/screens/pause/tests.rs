use super::*;
use macroquad::prelude::vec2;

#[test]
fn a_lan_match_menu_offers_no_save_or_restart() {
    let lan = PauseScreen::open(false, true).for_lan_match();
    assert_eq!(lan.menu.title, "MENU");
    assert!(!lan.rows.contains(&Row::SaveGame));
    assert!(!lan.rows.contains(&Row::Restart));
    assert!(lan.rows.contains(&Row::Surrender));
    assert_eq!(lan.rows.len(), lan.menu.items.len());
}

#[test]
fn a_lan_menu_keeps_its_rows_through_every_dialog() {
    let lan_rows = |p: &PauseScreen| {
        assert_eq!(p.menu.title, "MENU");
        assert!(
            !p.menu
                .items
                .iter()
                .any(|i| i == "Save Game" || i == "Restart")
        );
        assert_eq!(p.menu.items.len(), p.rows.len());
    };
    for label in ["Surrender", "Main Menu", "Quit"] {
        let mut p = PauseScreen::open(false, true).for_lan_match();
        activate(&mut p, label);
        assert!(p.confirming());
        drive(&mut p, Key::Enter);
        lan_rows(&p);
        assert_eq!(p.menu.items[p.menu.selected], label);
        assert_eq!(activate(&mut p, "Settings"), Out::Settings);
    }
    let mut p = PauseScreen::open(false, true)
        .for_lan_match()
        .with_save_failed("x".to_string(), LeaveVerb::MainMenu, false);
    assert_eq!(drive(&mut p, Key::Escape), Out::Stay);
    lan_rows(&p);
    assert_eq!(p.menu.items[p.menu.selected], "Main Menu");
}

fn drive(p: &mut PauseScreen, key: Key) -> Out {
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    p.update(
        &[RawEvent::KeyDown { key }, RawEvent::KeyUp { key }],
        &mut mouse,
        &mut sounds,
    )
}

/// Moves the cursor to the labeled row and activates it — by
/// label, never by raw Down counts, so the tests survive row-set
/// changes the way the index math never did.
fn activate(p: &mut PauseScreen, label: &str) -> Out {
    let target = p
        .menu
        .items
        .iter()
        .position(|i| i == label)
        .unwrap_or_else(|| panic!("no row labeled {label} in {:?}", p.menu.items));
    while p.menu.selected < target {
        drive(p, Key::Down);
    }
    while p.menu.selected > target {
        drive(p, Key::Up);
    }
    drive(p, Key::Enter)
}

#[test]
fn a_touch_only_pause_menu_offers_no_quit() {
    for finished in [false, true] {
        let touch = rows(finished, true, false);
        assert!(!touch.contains(&Row::Quit));
        assert_eq!(touch.last(), Some(&Row::MainMenu));
        assert!(rows(finished, true, true).contains(&Row::Quit));
    }
}

#[test]
fn a_notice_reads_as_the_subtitle_until_a_row_is_picked() {
    let mut p = PauseScreen::open(false, true).with_notice("paused after an interruption");
    assert_eq!(p.subtitle("Skirmish"), "paused after an interruption");
    drive(&mut p, Key::Down);
    assert_eq!(
        p.subtitle("Skirmish"),
        "paused after an interruption",
        "moving the cursor is not an activation"
    );
    activate(&mut p, "Restart");
    drive(&mut p, Key::Enter);
    assert_eq!(p.subtitle("Skirmish"), "Skirmish");
}

#[test]
fn consequential_rows_confirm_with_cancel_preselected() {
    let mut p = PauseScreen::open(false, true);
    assert_eq!(activate(&mut p, "Restart"), Out::Stay, "Restart only arms");
    assert!(p.confirming(), "the dialog is up");
    // Bare Enter declines: Cancel is the preselected row.
    assert_eq!(drive(&mut p, Key::Enter), Out::Stay);
    assert!(!p.confirming(), "Cancel closed the dialog");
    assert_eq!(
        p.menu.items[p.menu.selected], "Restart",
        "the cursor returns to the armed row"
    );
    // Armed again, a deliberate second motion confirms.
    drive(&mut p, Key::Enter);
    drive(&mut p, Key::Down);
    assert_eq!(drive(&mut p, Key::Enter), Out::Restart);
    // Quit confirms the same way.
    let mut p = PauseScreen::open(false, true);
    activate(&mut p, "Quit");
    drive(&mut p, Key::Down);
    assert_eq!(drive(&mut p, Key::Enter), Out::Quit);
}

#[test]
fn confirmation_copy_names_the_real_consequence() {
    for (label, consequence) in [
        ("Restart", "progress is discarded and the match starts over"),
        ("Main Menu", "the match is saved before returning home"),
        ("Quit", "the match is saved before quitting"),
    ] {
        let mut p = PauseScreen::open(false, true);
        assert_eq!(activate(&mut p, label), Out::Stay, "{label} only arms");
        assert_eq!(p.subtitle("map"), consequence, "{label} consequence");
    }
}

#[test]
fn escape_resumes_from_the_menu_but_only_cancels_the_dialog() {
    let mut p = PauseScreen::open(false, true);
    assert_eq!(drive(&mut p, Key::Escape), Out::Resume);
    let mut p = PauseScreen::open(false, true);
    activate(&mut p, "Main Menu");
    assert!(p.confirming());
    assert_eq!(
        drive(&mut p, Key::Escape),
        Out::Stay,
        "Escape in the dialog cancels, never resumes past it"
    );
    assert!(!p.confirming());
}

#[test]
fn resume_needs_no_confirmation_and_watch_exists_only_after_the_end() {
    let mut p = PauseScreen::open(true, false);
    assert!(
        !p.menu.items.iter().any(|i| i == "Save Game"),
        "a finished match is a replay, not a resumable named save"
    );
    assert_eq!(drive(&mut p, Key::Enter), Out::Resume);
    let mut p = PauseScreen::open(true, false);
    assert_eq!(activate(&mut p, "Watch Replay"), Out::WatchReplay);
    // Mid-match: no Watch Replay row, and every verb still lands on
    // the right target.
    let mut p = PauseScreen::open(false, true);
    assert!(
        !p.menu.items.iter().any(|i| i == "Watch Replay"),
        "mid-match playback would be a fog-free scout of the enemy"
    );
    assert!(
        p.menu.items.iter().any(|i| i == "Save Game"),
        "a running match can be saved by name"
    );
    assert_eq!(activate(&mut p, "Restart"), Out::Stay, "Restart arms");
    assert!(p.confirming());
    drive(&mut p, Key::Down);
    assert_eq!(drive(&mut p, Key::Enter), Out::Restart);
}

#[test]
fn the_save_failure_dialog_preselects_cancel_and_returns_to_the_verb() {
    let mut p = PauseScreen::open(false, true).with_save_failed(
        "could not save: unable to write the save file".to_string(),
        LeaveVerb::Quit,
        false,
    );
    assert!(p.saving_failed());
    assert!(p.subtitle("map").is_ascii(), "the menu font is Latin-1");
    // Bare Enter declines: Cancel is the preselected row, so a
    // reflexive double-tap never leaves unsaved.
    assert_eq!(drive(&mut p, Key::Enter), Out::Stay);
    assert!(!p.saving_failed(), "Cancel closed the dialog");
    assert_eq!(
        p.menu.items[p.menu.selected], "Quit",
        "the cursor returns to the verb that raised the dialog"
    );
}

#[test]
fn retry_and_leave_unsaved_carry_the_pending_verb() {
    let mut p = PauseScreen::open(false, true).with_save_failed(
        "x".to_string(),
        LeaveVerb::MainMenu,
        false,
    );
    assert_eq!(
        activate(&mut p, "Retry"),
        Out::RetrySave(LeaveVerb::MainMenu, false)
    );
    assert!(p.saving_failed(), "the dialog waits on the retry's verdict");
    assert_eq!(
        activate(&mut p, "Leave without saving"),
        Out::LeaveUnsaved(LeaveVerb::MainMenu),
        "a full disk can never trap the player"
    );
}

#[test]
fn escape_cancels_the_save_failure_dialog_never_the_leave() {
    let mut p =
        PauseScreen::open(false, true).with_save_failed("x".to_string(), LeaveVerb::Quit, false);
    assert_eq!(drive(&mut p, Key::Escape), Out::Stay);
    assert!(!p.saving_failed());
    // A Home-origin dialog cancels back to the front door instead
    // of a pause menu the player never opened.
    let mut p =
        PauseScreen::open(false, true).with_save_failed("x".to_string(), LeaveVerb::Quit, true);
    assert_eq!(drive(&mut p, Key::Escape), Out::Home);
}

fn type_text(p: &mut PauseScreen, text: &str) {
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    let events: Vec<RawEvent> = text.chars().map(|ch| RawEvent::Text { ch }).collect();
    p.update(&events, &mut mouse, &mut sounds);
}

fn naming_at_1280(suggested: &str) -> (PauseScreen, crate::text_field::Layout) {
    crate::render::set_viewport(1280.0, 800.0);
    let mut p = PauseScreen::open(false, true);
    p.begin_naming(suggested);
    let layout = crate::text_field::layout(vec2(1280.0, 800.0), crate::render::ui_scale());
    (p, layout)
}

fn pointer(p: &mut PauseScreen, events: &[RawEvent]) -> (Out, Vec<SoundKind>) {
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    let out = p.update(events, &mut mouse, &mut sounds);
    (out, sounds.into_iter().map(|(kind, _)| kind).collect())
}

fn tap(at: Vec2) -> [RawEvent; 2] {
    [
        RawEvent::TouchDown {
            id: 1,
            x: at.x,
            y: at.y,
        },
        RawEvent::TouchUp {
            id: 1,
            x: at.x,
            y: at.y,
        },
    ]
}

#[test]
fn the_save_button_commits_like_enter_and_refuses_a_blank_name() {
    let (mut p, layout) = naming_at_1280("Skirmish | t40");
    assert_eq!(
        pointer(&mut p, &tap(layout.confirm.center())).0,
        Out::Save("Skirmish | t40".to_string())
    );
    let (mut p, layout) = naming_at_1280("");
    let click = [
        RawEvent::MouseDown {
            button: oxide_protocol::MouseButton::Left,
            x: layout.confirm.center().x,
            y: layout.confirm.center().y,
        },
        RawEvent::MouseUp {
            button: oxide_protocol::MouseButton::Left,
            x: layout.confirm.center().x,
            y: layout.confirm.center().y,
        },
    ];
    let (out, sounds) = pointer(&mut p, &click);
    assert_eq!(out, Out::Stay);
    assert!(sounds.contains(&SoundKind::Denied));
    assert!(p.naming(), "a blank name keeps the field open");
}

#[test]
fn the_cancel_button_abandons_like_escape() {
    let (mut p, layout) = naming_at_1280("Skirmish | t40");
    assert_eq!(pointer(&mut p, &tap(layout.cancel.center())).0, Out::Stay);
    assert!(!p.naming());
    assert_eq!(p.menu.items[p.menu.selected], "Save Game");
}

#[test]
fn a_save_press_released_elsewhere_keeps_naming() {
    let (mut p, layout) = naming_at_1280("Skirmish | t40");
    let from = layout.confirm.center();
    let events = [
        RawEvent::TouchDown {
            id: 1,
            x: from.x,
            y: from.y,
        },
        RawEvent::TouchUp {
            id: 1,
            x: from.x,
            y: from.y + 300.0,
        },
    ];
    assert_eq!(pointer(&mut p, &events).0, Out::Stay);
    assert!(p.naming());
}

#[test]
fn tapping_the_field_requests_the_keyboard_once() {
    let (mut p, layout) = naming_at_1280("Skirmish | t40");
    assert!(!p.take_keyboard_request());
    pointer(&mut p, &tap(layout.field.center()));
    assert!(p.take_keyboard_request());
    assert!(!p.take_keyboard_request());
}

#[test]
fn save_game_never_confirms_and_bare_enter_saves_the_suggestion() {
    let mut p = PauseScreen::open(false, true);
    assert_eq!(activate(&mut p, "Save Game"), Out::SaveGame);
    assert!(!p.confirming(), "saving destroys nothing — no dialog");
    p.begin_naming("skirmish | t100");
    assert!(p.naming());
    assert_eq!(
        p.menu.items[0], "skirmish | t100_",
        "prefilled, with a static caret"
    );
    // The Start-preselected doctrine: Enter alone commits the
    // suggested name without any typing.
    assert_eq!(
        drive(&mut p, Key::Enter),
        Out::Save("skirmish | t100".to_string())
    );
}

#[test]
fn the_name_field_edits_with_text_and_backspace_and_escape_cancels() {
    let mut p = PauseScreen::open(false, true);
    p.begin_naming("");
    type_text(&mut p, "abc");
    assert_eq!(drive(&mut p, Key::Backspace), Out::Stay);
    assert_eq!(p.menu.items[0], "ab_");
    // Letter KEYS are not text: only Text events edit the buffer,
    // so an injected semantic H cannot type.
    drive(&mut p, Key::H);
    assert_eq!(p.menu.items[0], "ab_");
    assert_eq!(drive(&mut p, Key::Escape), Out::Stay, "Escape abandons");
    assert!(!p.naming());
    assert_eq!(
        p.menu.items[p.menu.selected], "Save Game",
        "the cursor returns to the verb"
    );
    // An empty name refuses to commit instead of writing a blank.
    p.begin_naming("");
    assert_eq!(drive(&mut p, Key::Enter), Out::Stay);
    assert!(p.naming(), "the field waits for a real name");
}

#[test]
fn repeated_backspace_edges_clear_the_name_field_in_one_frame() {
    let mut p = PauseScreen::open(false, true);
    p.begin_naming("oxide");
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    let repeats = [
        RawEvent::KeyDown {
            key: Key::Backspace,
        },
        RawEvent::KeyDown {
            key: Key::Backspace,
        },
        RawEvent::KeyDown {
            key: Key::Backspace,
        },
        RawEvent::KeyDown {
            key: Key::Backspace,
        },
        RawEvent::KeyDown {
            key: Key::Backspace,
        },
    ];
    assert_eq!(p.update(&repeats, &mut mouse, &mut sounds), Out::Stay);
    assert_eq!(p.menu.items[0], "_");
}

#[test]
fn the_name_field_caps_its_length_and_the_verdict_shows_until_the_next_pick() {
    let mut p = PauseScreen::open(false, true);
    p.begin_naming(&"x".repeat(40));
    let shown = p.menu.items[0].clone();
    assert_eq!(
        shown.chars().count(),
        PauseScreen::NAME_MAX + 1,
        "cap+caret"
    );
    type_text(&mut p, "y");
    assert_eq!(p.menu.items[0], shown, "a full field refuses more");
    drive(&mut p, Key::Enter);
    p.end_naming("saved: x".to_string());
    assert!(!p.naming());
    assert_eq!(p.subtitle("map"), "saved: x", "the verdict is the subtitle");
    assert_eq!(p.menu.items[p.menu.selected], "Save Game");
    assert_eq!(activate(&mut p, "Resume"), Out::Resume);
    assert_eq!(p.subtitle("map"), "map", "an activation clears the verdict");
}

#[test]
fn surrender_exists_mid_match_only_and_confirms_with_cancel_preselected() {
    let mut p = PauseScreen::open(false, true);
    assert_eq!(activate(&mut p, "Surrender"), Out::Stay, "Surrender arms");
    assert!(p.confirming(), "conceding asks first");
    assert_eq!(
        p.subtitle("map"),
        "this concedes the match",
        "the dialog names the real consequence, not a thrown-away match"
    );
    // Bare Enter declines: Cancel is the preselected row.
    assert_eq!(drive(&mut p, Key::Enter), Out::Stay);
    assert!(!p.confirming(), "Cancel closed the dialog");
    assert_eq!(
        p.menu.items[p.menu.selected], "Surrender",
        "the cursor returns to the armed row"
    );
    // A deliberate second motion concedes.
    drive(&mut p, Key::Enter);
    drive(&mut p, Key::Down);
    assert_eq!(drive(&mut p, Key::Enter), Out::Surrender);
    // A decided match has nothing left to give up.
    let p = PauseScreen::open(true, false);
    assert!(
        !p.menu.items.iter().any(|i| i == "Surrender"),
        "a decided match offers Watch Replay, not concession"
    );
    // A seat with no voice (resigned or eliminated) gets no verb
    // the sim would only reject.
    let p = PauseScreen::open(false, false);
    assert!(
        !p.menu.items.iter().any(|i| i == "Surrender"),
        "a spectating seat cannot concede twice"
    );
}

#[test]
fn settings_opens_without_confirmation_on_both_faces() {
    // Settings destroys nothing — it must never arm the dialog.
    for finished in [false, true] {
        let mut p = PauseScreen::open(finished, true);
        assert_eq!(activate(&mut p, "Settings"), Out::Settings);
        assert!(!p.confirming(), "Settings is not a destructive row");
    }
}
