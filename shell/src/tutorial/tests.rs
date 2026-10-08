use super::*;

#[test]
fn each_lesson_waits_for_its_demonstration() {
    let mut t = Tutorial::new();
    let mut demo = Demo::default();
    assert!(t.advance(demo), "school is in session");
    assert_eq!(t.step, 0);
    demo.trained = true;
    assert!(t.advance(demo));
    assert_eq!(t.step, 1, "training graduates lesson one only");
    demo.harvested = true;
    assert!(t.advance(demo));
    assert_eq!(t.step, 1, "an accepted order alone is not income");
    demo.deposited = true;
    demo.built = true;
    assert!(t.advance(demo));
    assert_eq!(t.step, 3, "already-demonstrated steps skip in one pass");
}

#[test]
fn a_prodigy_graduates_immediately() {
    let mut t = Tutorial::new();
    let demo = Demo {
        trained: true,
        trained_fighter: true,
        harvested: true,
        deposited: true,
        built: true,
        advanced: true,
        paused_menu: true,
    };
    assert!(!t.advance(demo), "nothing left to teach");
}

#[test]
fn the_coach_prices_the_lesson_and_names_the_exit_when_broke() {
    let game = crate::game::Game::with_viewport(
        tutorial_scenario(),
        macroquad::prelude::vec2(1280.0, 800.0),
    )
    .expect("the tutorial scenario builds");
    let t = Tutorial::new();
    let line = t.coach(&game).expect("the training lesson has a price");
    match line {
        CoachLine::Status(s) => {
            assert!(s.contains("50 scrap"), "names the harvester's price: {s}");
            assert!(s.contains("260"), "names the live bank: {s}");
        }
        CoachLine::Recovery(_) => panic!("a funded bank needs no rescue"),
    }

    let mut broke = tutorial_scenario();
    broke.players[0].scrap = 0;
    let game = crate::game::Game::with_viewport(broke, macroquad::prelude::vec2(1280.0, 800.0))
        .expect("the broke variant builds");
    let t = Tutorial { step: 3 };
    match t.coach(&game).expect("the fighter lesson has a price") {
        CoachLine::Recovery(s) => {
            assert!(
                s.contains("idle badge"),
                "offers to select an idle harvester: {s}"
            );
        }
        CoachLine::Status(s) => panic!("zero bank, zero income must nudge, got: {s}"),
    }
}

#[test]
fn every_card_string_is_ascii() {
    // The bundled font cannot be trusted to cover typographic punctuation.
    for step in &STEPS {
        for touch_only in [false, true] {
            for line in std::iter::once(&step.title).chain(step.body(touch_only)) {
                assert!(line.is_ascii(), "non-ASCII in card text: {line}");
            }
        }
    }
}

#[test]
fn touch_lessons_name_no_keys_or_mouse_buttons() {
    for step in &STEPS {
        for line in step.body(true) {
            crate::platform::assert_touch_copy(line);
        }
    }
    crate::platform::assert_touch_copy(recovery_line(true));
}
