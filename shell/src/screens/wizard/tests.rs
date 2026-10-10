use super::*;
use macroquad::prelude::vec2;
use oxide_protocol::MouseButton;

/// The card at display position `row`, which must be on this page.
fn card(layout: &SetupLayout, row: usize) -> CardRects {
    layout.cards[row].expect("the card is on this page")
}

#[test]
fn the_setup_hint_speaks_touch_on_touch_only_builds() {
    assert_eq!(
        setup_hint(false, true, Cell::Seat, false),
        "{confirm} starts the match - {back} back"
    );
    for one_team in [false, true] {
        for on_start in [false, true] {
            for cell in Cell::ALL {
                crate::platform::assert_touch_copy(setup_hint(one_team, on_start, cell, true));
                assert!(setup_hint(one_team, on_start, cell, false).contains("{back}"));
            }
        }
    }
}

fn press(key: Key) -> Vec<RawEvent> {
    vec![RawEvent::KeyDown { key }, RawEvent::KeyUp { key }]
}

fn drive(w: &mut Wizard, draft: &mut NewMatchDraft, key: Key) -> Out {
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    w.update(&press(key), &mut mouse, draft, &mut sounds)
        .expect("update")
}

/// Activates the browser's first entry (sections put duels first, so
/// entry 0 is always a 1v1; its own test pins that).
fn pick_first_map(w: &mut Wizard, draft: &mut NewMatchDraft) {
    w.browser.selected = 0;
    assert_eq!(drive(w, draft, Key::Enter), Out::Stay);
}

fn explicit_team_setup() -> (Wizard, NewMatchDraft) {
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/trident-plateau.json"
    ));
    let scenario = Scenario::load(&path).expect("shipped team map");
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(scenario, Some(path));
    let mut wizard = Wizard::open(&draft);
    wizard.goto(Step::Setup, &draft);
    (wizard, draft)
}

#[test]
fn every_discovered_map_lands_on_setup_and_launches_as_authored() {
    let count = Wizard::open(&NewMatchDraft::default()).entries.len();
    assert!(count > 0, "the browser always has an embedded fallback");

    for index in 0..count {
        let mut draft = NewMatchDraft::default();
        let mut wizard = Wizard::open(&draft);
        let expected_path = wizard.entries[index].path.clone();
        let expected_seats = wizard.entries[index].seats;
        wizard.browser.selected = index;

        assert_eq!(drive(&mut wizard, &mut draft, Key::Enter), Out::Stay);
        assert_eq!(wizard.step, Step::Setup, "map {index} skipped setup");
        assert_eq!(draft.scenario_path, expected_path, "map {index} changed");
        assert_eq!(draft.seats.len(), expected_seats, "map {index} seat count");
        assert!(
            draft.seats.iter().all(|plan| plan.faction_choice == 0),
            "map {index} did not preserve its authored factions"
        );
        assert_eq!(draft.seat_choice, 0, "map {index} did not open on seat 0");
        assert_eq!(
            wizard.setup_sel,
            draft.seats.len(),
            "map {index} did not preselect Start"
        );
        assert_eq!(
            drive(&mut wizard, &mut draft, Key::Enter),
            Out::Launch,
            "map {index} did not launch with its authored teams"
        );
    }
}

#[test]
fn a_stale_seat_never_carries_across_maps() {
    // Take a late chair on a team map, back out, pick a duel: the chair
    // and choices must reset rather than silently clamp the human into
    // the duel's second seat. Re-entering the same map keeps every
    // answer.
    let team = Scenario::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/compass-grand.json"
    ))
    .expect("shipped map");
    let path = Some(PathBuf::from("compass-grand.json"));
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(team.clone(), path.clone());
    draft.seat_choice = 5;
    draft.seats[3].faction_choice = 2;
    draft.set_scenario(team, path);
    assert_eq!(draft.seat_choice, 5, "same map: the chair survives Back");
    assert_eq!(
        draft.seats[3].faction_choice, 2,
        "same map: choices survive"
    );
    draft.set_scenario(Scenario::skirmish(), None);
    assert_eq!(draft.seat_choice, 0, "new map: the chair resets");
    assert!(draft.seats.iter().all(|p| *p == SeatPlan::default()));
}

#[test]
fn opponent_choices_start_from_the_map_and_follow_backtracking_rules() {
    let mut authored = Scenario::skirmish();
    authored.players[1].bot_config = Some(oxide_sim::scenario::BotConfig::new(
        BotDifficulty::Prime,
        BotStance::Aggressive,
        0xDEAD_BEEF,
    ));
    let path = Some(PathBuf::from("authored-duel.json"));
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(authored.clone(), path.clone());
    assert_eq!(draft.seats[1].difficulty, BotDifficulty::Prime);
    assert_eq!(draft.seats[1].stance, BotStance::Aggressive);

    draft.seats[1].difficulty = BotDifficulty::Scrapheap;
    draft.seats[1].stance = BotStance::Turtle;
    draft.set_scenario(authored.clone(), path);
    assert_eq!(draft.seats[1].difficulty, BotDifficulty::Scrapheap);
    assert_eq!(draft.seats[1].stance, BotStance::Turtle);

    draft.set_scenario(authored, Some(PathBuf::from("another-copy.json")));
    assert_eq!(draft.seats[1].difficulty, BotDifficulty::Prime);
    assert_eq!(draft.seats[1].stance, BotStance::Aggressive);
}

#[test]
fn the_team_chip_defaults_follow_the_authored_teams() {
    let team = Scenario::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/trident-plateau.json"
    ))
    .expect("shipped map");
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(team.clone(), None);
    let choices: Vec<usize> = draft.seats.iter().map(|p| p.team_choice).collect();
    assert_eq!(
        choices,
        vec![1, 1, 1, 2, 2, 2],
        "authored teams open as Team 1 / Team 2"
    );

    // Sparse authored ids still label densely by first appearance,
    // and an omitted seat opens as FFA — the same normalization the
    // sim applies at build.
    let mut sparse = team;
    sparse.players[0].team = Some(9);
    sparse.players[1].team = Some(9);
    sparse.players[2].team = None;
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(sparse, None);
    let choices: Vec<usize> = draft.seats.iter().map(|p| p.team_choice).collect();
    assert_eq!(choices, vec![1, 1, 0, 2, 2, 2]);

    let mut draft = NewMatchDraft::default();
    draft.set_scenario(Scenario::skirmish(), None);
    assert!(
        draft.seats.iter().all(|p| p.team_choice == 0),
        "a map without authored teams opens as FFA"
    );
}

#[test]
fn the_team_chip_cycles_through_ffa_and_every_team() {
    let mut draft = NewMatchDraft::default();
    let mut w = Wizard::open(&draft);
    pick_first_map(&mut w, &mut draft); // a duel: FFA, Team 1, Team 2
    let order = seat_display_order(draft.scenario.as_deref().unwrap());

    // Your own card skips the absent opponent chip: Right lands on
    // faction, then team.
    drive(&mut w, &mut draft, Key::Home);
    drive(&mut w, &mut draft, Key::Right);
    drive(&mut w, &mut draft, Key::Right);
    assert_eq!(w.setup_cell, Cell::Team, "the team chip is the last cell");
    drive(&mut w, &mut draft, Key::Enter);
    assert_eq!(
        draft.seats[draft.seat_choice].team_choice, 1,
        "FFA cycles to Team 1"
    );
    drive(&mut w, &mut draft, Key::Enter);
    assert_eq!(draft.seats[draft.seat_choice].team_choice, 2);
    drive(&mut w, &mut draft, Key::Enter);
    assert_eq!(
        draft.seats[draft.seat_choice].team_choice, 0,
        "past the seat count wraps back to FFA"
    );
    assert_eq!(
        w.step,
        Step::Setup,
        "cycling a chip never leaves the screen"
    );

    // The sticky column carries the team cell onto an AI card.
    drive(&mut w, &mut draft, Key::Down);
    drive(&mut w, &mut draft, Key::Enter);
    assert_eq!(draft.seats[order[1]].team_choice, 1);
}

#[test]
fn setup_cells_wrap_past_the_dead_ones() {
    let mut draft = NewMatchDraft::default();
    let mut w = Wizard::open(&draft);
    pick_first_map(&mut w, &mut draft);
    // Your own card has no difficulty or stance chip: the seat, faction
    // and team cells are live.
    drive(&mut w, &mut draft, Key::Home);
    assert_eq!(w.setup_cell, Cell::Seat);
    drive(&mut w, &mut draft, Key::Left);
    assert_eq!(
        w.setup_cell,
        Cell::Team,
        "left from the seat wraps to the team chip"
    );
    drive(&mut w, &mut draft, Key::Right);
    assert_eq!(
        w.setup_cell,
        Cell::Seat,
        "right from the team chip wraps to the seat"
    );
    drive(&mut w, &mut draft, Key::Right);
    assert_eq!(
        w.setup_cell,
        Cell::Faction,
        "the opponent chips are skipped"
    );
    // An AI card has every cell.
    drive(&mut w, &mut draft, Key::Down);
    drive(&mut w, &mut draft, Key::Left);
    assert_eq!(w.setup_cell, Cell::Stance);
}

#[test]
fn a_stale_team_choice_never_carries_across_maps() {
    // As with the chair: re-entering the same map keeps the choice, and a
    // different map re-derives the authored defaults, so a Team 5 chosen
    // on an 8-seat map must not ride into a duel that has no Team 5.
    let team = Scenario::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/compass-grand.json"
    ))
    .expect("shipped map");
    let path = Some(PathBuf::from("compass-grand.json"));
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(team.clone(), path.clone());
    assert_eq!(
        draft.seats[7].team_choice, 2,
        "defaults follow the authored teams"
    );
    draft.seats[7].team_choice = 5;
    draft.set_scenario(team, path);
    assert_eq!(
        draft.seats[7].team_choice, 5,
        "same map: the team choice survives Back"
    );
    draft.set_scenario(Scenario::skirmish(), None);
    assert!(
        draft.seats.iter().all(|p| p.team_choice == 0),
        "new map: every team choice returns to that map's default"
    );
}

#[test]
fn an_all_one_team_draft_disables_start() {
    let mut draft = NewMatchDraft::default();
    let mut w = Wizard::open(&draft);
    pick_first_map(&mut w, &mut draft);
    for plan in &mut draft.seats {
        plan.team_choice = 1;
    }
    drive(&mut w, &mut draft, Key::End);
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    let out = w
        .update(&press(Key::Enter), &mut mouse, &mut draft, &mut sounds)
        .expect("update");
    assert_eq!(out, Out::Stay, "one team, nobody to fight: Start refuses");
    assert_eq!(w.step, Step::Setup, "the screen stays put");
    assert!(
        sounds.contains(&(SoundKind::Denied, None)),
        "the refusal is audible, not silent"
    );
    draft.seats[0].team_choice = 0;
    assert_eq!(
        drive(&mut w, &mut draft, Key::Enter),
        Out::Launch,
        "freeing one seat re-arms Start"
    );
}

fn tap_at(w: &mut Wizard, draft: &mut NewMatchDraft, from: Vec2, to: Vec2) -> Out {
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    let events = [
        RawEvent::TouchDown {
            id: 5,
            x: from.x,
            y: from.y,
        },
        RawEvent::TouchMove {
            id: 5,
            x: to.x,
            y: to.y,
        },
        RawEvent::TouchUp {
            id: 5,
            x: to.x,
            y: to.y,
        },
    ];
    w.update(&events, &mut mouse, draft, &mut sounds)
        .expect("update")
}

#[test]
fn the_corner_back_button_steps_back_like_escape_on_both_steps() {
    crate::render::set_viewport(1280.0, 800.0);
    let back = crate::button::corner_slot(0, crate::render::ui_scale()).center();
    let mut draft = NewMatchDraft::default();
    let mut w = Wizard::open(&draft);
    assert_eq!(tap_at(&mut w, &mut draft, back, back), Out::Home);

    let mut w = Wizard::open(&draft);
    pick_first_map(&mut w, &mut draft);
    assert_eq!(w.step, Step::Setup);
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
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
    assert_eq!(
        w.update(&click, &mut mouse, &mut draft, &mut sounds)
            .expect("update"),
        Out::Stay
    );
    assert_eq!(w.step, Step::Map, "Back walks one step, like Escape");
    assert!(draft.scenario.is_some(), "the pick survives the step back");
}

#[test]
fn a_back_press_that_slides_off_does_nothing() {
    crate::render::set_viewport(1280.0, 800.0);
    let back = crate::button::corner_slot(0, crate::render::ui_scale());
    let off = vec2(back.x + back.w + 40.0, back.center().y);
    let mut draft = NewMatchDraft::default();
    let mut w = Wizard::open(&draft);
    assert_eq!(tap_at(&mut w, &mut draft, back.center(), off), Out::Stay);
    assert_eq!(w.step, Step::Map);
    pick_first_map(&mut w, &mut draft);
    assert_eq!(tap_at(&mut w, &mut draft, back.center(), off), Out::Stay);
    assert_eq!(w.step, Step::Setup);
}

#[test]
fn a_step_change_drops_a_half_made_back_press() {
    crate::render::set_viewport(1280.0, 800.0);
    let back = crate::button::corner_slot(0, crate::render::ui_scale()).center();
    let button = MouseButton::Left;
    let mut draft = NewMatchDraft::default();
    let mut w = Wizard::open(&draft);
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    let down = [RawEvent::MouseDown {
        button,
        x: back.x,
        y: back.y,
    }];
    w.update(&down, &mut mouse, &mut draft, &mut sounds)
        .expect("update");
    pick_first_map(&mut w, &mut draft);
    assert_eq!(w.step, Step::Setup);
    let up = [RawEvent::MouseUp {
        button,
        x: back.x,
        y: back.y,
    }];
    w.update(&up, &mut mouse, &mut draft, &mut sounds)
        .expect("update");
    assert_eq!(
        w.step,
        Step::Setup,
        "a press from the grid never backs out of setup"
    );
}

#[test]
fn the_back_button_clears_grid_and_setup_content() {
    let draft = NewMatchDraft::default();
    let w = Wizard::open(&draft);
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/trident-plateau.json"
    ));
    let team_map = Scenario::load(&path).expect("shipped team map");
    let sizes = [
        (vec2(640.0, 400.0), 1.0),
        (vec2(1133.0, 744.0), 1.0),
        (vec2(1194.0, 834.0), 1.0),
        (vec2(1280.0, 800.0), 1.0),
        (vec2(1280.0, 800.0), 1.25),
        (vec2(1920.0, 1080.0), 1.5),
    ];
    for (view, ui) in sizes {
        let back = crate::button::corner_slot(0, ui);
        let grid = w.browser.layout(&w.entries, view, ui);
        let grid_rects = grid
            .headings
            .iter()
            .map(|(_, rect)| *rect)
            .chain(grid.cards.iter().map(|(_, rect)| *rect));
        for rect in grid_rects {
            assert!(
                !back.overlaps(&rect),
                "grid content under Back at {view} ui {ui}"
            );
        }
        for scenario in [Scenario::skirmish(), team_map.clone()] {
            let setup = setup_layout(&scenario, 0, view, ui);
            let content = setup
                .cards
                .iter()
                .flatten()
                .map(|card| card.card)
                .chain(setup.headings.iter().map(|(_, rect)| *rect))
                .chain(setup.start)
                .chain([setup.preview]);
            for rect in content {
                assert!(
                    !back.overlaps(&rect),
                    "setup content under Back at {view} ui {ui}"
                );
            }
        }
    }
}

#[test]
fn escape_unwinds_to_home_and_setup_steps_back_to_the_grid() {
    let mut draft = NewMatchDraft::default();
    let mut w = Wizard::open(&draft);
    assert_eq!(drive(&mut w, &mut draft, Key::Escape), Out::Home);

    let mut w = Wizard::open(&draft);
    pick_first_map(&mut w, &mut draft);
    assert_eq!(w.step, Step::Setup);
    assert_eq!(drive(&mut w, &mut draft, Key::Escape), Out::Stay);
    assert_eq!(w.step, Step::Map, "Esc walks one step");
    assert_eq!(
        w.browser.selected,
        w.entries
            .iter()
            .position(|e| e.path == draft.scenario_path)
            .unwrap(),
        "the grid re-offers the earlier pick, found by PATH"
    );
}

#[test]
fn the_browser_leads_with_duels() {
    let w = Wizard::open(&NewMatchDraft::default());
    assert!(
        w.entries.first().is_some_and(|e| e.seats == 2),
        "entry 0 must be a 1v1: a first Play+Enter never launches a team match"
    );
    let seat_counts: Vec<usize> = w.entries.iter().map(|e| e.seats).collect();
    let mut sorted = seat_counts.clone();
    sorted.sort_unstable();
    assert_eq!(seat_counts, sorted, "sections ascend by format");
}

#[test]
fn a_team_map_runs_the_setup_screen_and_reseats_without_permuting() {
    let (mut w, mut draft) = explicit_team_setup();
    let seats = draft.seats.len();
    assert!(seats > 2, "the explicit fixture is a team map");
    assert_eq!(w.step, Step::Setup);
    assert_eq!(w.setup_sel, seats, "Start preselected under the seat cards");

    // Walk to the second DISPLAY seat; Enter takes the chair
    // inline — no sub-screen.
    drive(&mut w, &mut draft, Key::Home);
    drive(&mut w, &mut draft, Key::Down);
    let order = seat_display_order(draft.scenario.as_deref().unwrap());
    drive(&mut w, &mut draft, Key::Enter);
    assert_eq!(draft.seat_choice, order[1], "the chair moved");
    assert_eq!(w.step, Step::Setup, "and the screen never left");

    let (_, rows, _) = w.ui_surface(&draft);
    assert!(
        rows[0].contains("Difficulty Standard | Stance Balanced"),
        "every opponent plainly shows its configured difficulty and stance"
    );

    // End sits on Start; Enter launches.
    drive(&mut w, &mut draft, Key::End);
    assert_eq!(drive(&mut w, &mut draft, Key::Enter), Out::Launch);

    draft.set_scenario(Scenario::skirmish(), None);
    assert_eq!(draft.seats.len(), 2, "re-derived at the new width");
    assert!(draft.seat_choice < 2, "the chair clamped onto the board");
}

#[test]
fn omitted_singleton_teams_keep_their_setup_card() {
    let mut scenario = Scenario::load("../scenarios/trident-plateau.json").expect("shipped");
    // Two explicit teammates then an omitted singleton: every seat must
    // stay in the display order, or Enter on Start would index past it.
    scenario.players[2].team = None;
    let order = seat_display_order(&scenario);
    assert_eq!(
        order.len(),
        scenario.players.len(),
        "every seat keeps a card"
    );
    let mut sorted = order;
    sorted.sort_unstable();
    assert_eq!(sorted, (0..scenario.players.len()).collect::<Vec<_>>());
    let layout = setup_layout(&scenario, 0, vec2(1280.0, 800.0), 1.0);
    assert_eq!(layout.cards.len(), scenario.players.len());

    // A large authored id must not swallow an omitted seat either.
    scenario.players[2].team = Some(202);
    let order = seat_display_order(&scenario);
    assert_eq!(order.len(), scenario.players.len());
}

#[test]
fn the_faction_chip_cycles_on_every_card_including_yours() {
    let (mut w, mut draft) = explicit_team_setup();
    let order = seat_display_order(draft.scenario.as_deref().unwrap());
    assert_eq!(order[0], draft.seat_choice, "the human opens in seat 0");

    // Your own card: Right reaches the faction chip; Enter cycles
    // Auto to Ferrous.
    drive(&mut w, &mut draft, Key::Home);
    drive(&mut w, &mut draft, Key::Right);
    drive(&mut w, &mut draft, Key::Enter);
    assert_eq!(
        draft.seats[draft.seat_choice].faction_choice, 1,
        "your own chip cycled to Ferrous"
    );
    assert_eq!(
        w.step,
        Step::Setup,
        "cycling a chip never leaves the screen"
    );

    // The sticky column carries the faction cell onto an AI card.
    drive(&mut w, &mut draft, Key::Down);
    drive(&mut w, &mut draft, Key::Enter);
    assert_eq!(draft.seats[order[1]].faction_choice, 1);
    // Left from faction walks the two visible bot controls before the
    // seat action. The hidden seeded identity is never exposed.
    drive(&mut w, &mut draft, Key::Left);
    assert_eq!(w.setup_cell, Cell::Stance);
    drive(&mut w, &mut draft, Key::Left);
    assert_eq!(w.setup_cell, Cell::Difficulty);
    drive(&mut w, &mut draft, Key::Left);
    assert_eq!(w.setup_cell, Cell::Seat);
    drive(&mut w, &mut draft, Key::Enter);
    assert_eq!(
        draft.seat_choice, order[1],
        "the next cell left of faction is the seat"
    );
}

#[test]
fn keyboard_cycles_difficulty_and_stance_directly_and_independently() {
    let (mut wizard, mut draft) = explicit_team_setup();
    let order = seat_display_order(draft.scenario.as_deref().unwrap());
    let opponent = order[1];
    let original = draft.seats[opponent];
    drive(&mut wizard, &mut draft, Key::Home);
    drive(&mut wizard, &mut draft, Key::Down);
    drive(&mut wizard, &mut draft, Key::Right);
    assert_eq!(wizard.setup_cell, Cell::Difficulty);
    drive(&mut wizard, &mut draft, Key::Enter);

    assert_eq!(wizard.mode_name(), "match_setup");
    assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
    assert_eq!(draft.seats[opponent].stance, original.stance);
    assert_eq!(
        draft.seats[opponent].faction_choice,
        original.faction_choice
    );
    assert_eq!(draft.seats[opponent].team_choice, original.team_choice);

    drive(&mut wizard, &mut draft, Key::Right);
    assert_eq!(wizard.setup_cell, Cell::Stance);
    drive(&mut wizard, &mut draft, Key::Enter);
    assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
    assert_eq!(draft.seats[opponent].stance, BotStance::Aggressive);
    assert_eq!(
        draft.seats[opponent].faction_choice,
        original.faction_choice
    );
    assert_eq!(draft.seats[opponent].team_choice, original.team_choice);

    let (title, items, selected) = wizard.ui_surface(&draft);
    assert_eq!(title, "MATCH SETUP");
    assert!(items[1].contains("Difficulty Veteran"));
    assert!(items[1].contains("Stance Aggressive"));
    assert_eq!(selected, 1);
    assert!(items.iter().all(|item| !item.contains("seed")));
}

#[test]
fn direct_bot_control_activation_ends_the_input_batch() {
    let (mut wizard, mut draft) = explicit_team_setup();
    let order = seat_display_order(draft.scenario.as_deref().unwrap());
    let opponent = order[1];
    wizard.setup_sel = 1;
    wizard.setup_cell = Cell::Difficulty;
    wizard.setup_page = 0;

    let mut mouse = Vec2::ZERO;
    let mut sounds = Vec::new();
    let out = wizard
        .update(
            &[
                RawEvent::KeyDown { key: Key::Enter },
                RawEvent::KeyDown { key: Key::Right },
                RawEvent::KeyDown { key: Key::Enter },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("activate direct difficulty control");
    assert_eq!(out, Out::Stay);
    assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
    assert_eq!(draft.seats[opponent].stance, BotStance::Balanced);
    assert_eq!(wizard.setup_cell, Cell::Difficulty);
    assert_eq!(wizard.mode_name(), "match_setup");
    assert_eq!(sounds, [(SoundKind::Click, None)]);
}

#[test]
fn mouse_and_touch_cycle_the_direct_bot_controls() {
    crate::render::set_viewport(640.0, 400.0);
    let (mut wizard, mut draft) = explicit_team_setup();
    let scenario = draft.scenario.as_deref().expect("picked scenario");
    let order = seat_display_order(scenario);
    let row = 1;
    let opponent = order[row];
    wizard.setup_sel = row;
    wizard.setup_page = 0;
    let setup = setup_layout(
        scenario,
        draft.seat_choice,
        crate::render::viewport(),
        crate::render::ui_scale(),
    );
    let difficulty = card(&setup, row).difficulty.unwrap().center();
    let stance = card(&setup, row).stance.unwrap().center();
    let mut mouse = Vec2::ZERO;
    let mut sounds = Vec::new();
    wizard
        .update(
            &[
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: difficulty.x,
                    y: difficulty.y,
                },
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: difficulty.x,
                    y: difficulty.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("cycle difficulty");
    assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
    assert_eq!(draft.seats[opponent].stance, BotStance::Balanced);
    assert_eq!(wizard.mode_name(), "match_setup");
    assert_eq!(wizard.setup_cell, Cell::Difficulty);

    wizard
        .update(
            &[
                RawEvent::TouchDown {
                    id: 4,
                    x: stance.x,
                    y: stance.y,
                },
                RawEvent::TouchUp {
                    id: 4,
                    x: stance.x,
                    y: stance.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("cycle stance");
    assert_eq!(draft.seats[opponent].difficulty, BotDifficulty::Veteran);
    assert_eq!(draft.seats[opponent].stance, BotStance::Aggressive);
    assert_eq!(wizard.mode_name(), "match_setup");
    assert_eq!(wizard.setup_cell, Cell::Stance);
    assert_eq!(sounds, [(SoundKind::Click, None); 2]);
}

#[test]
fn the_difficulty_chip_cycles_through_remote_and_hosting_follows_it() {
    crate::render::set_viewport(640.0, 400.0);
    let (mut wizard, mut draft) = explicit_team_setup();
    let scenario = draft.scenario.clone().expect("picked scenario");
    let row = 1;
    let opponent = seat_display_order(&scenario)[row];
    let stance = draft.seats[opponent].stance;
    wizard.setup_sel = row;
    wizard.setup_cell = Cell::Difficulty;
    let start = |wizard: &Wizard, draft: &NewMatchDraft| {
        wizard.ui_surface(draft).1.last().cloned().unwrap()
    };
    let mut seen = Vec::new();
    for _ in 0..5 {
        drive(&mut wizard, &mut draft, Key::Enter);
        let plan = draft.seats[opponent];
        seen.push((!plan.remote).then_some(plan.difficulty));
    }
    assert_eq!(
        seen,
        [
            Some(BotDifficulty::Veteran),
            Some(BotDifficulty::Prime),
            None,
            Some(BotDifficulty::Scrapheap),
            Some(BotDifficulty::Standard),
        ]
    );
    assert_eq!(draft.seats[opponent].stance, stance, "the stance survives");
    assert_eq!(start(&wizard, &draft), "Start match");

    draft.seats[opponent].remote = true;
    assert_eq!(start(&wizard, &draft), "Host match");
    assert!(wizard.ui_surface(&draft).1[row].contains("(remote)"));
    drive(&mut wizard, &mut draft, Key::Right);
    assert_eq!(
        wizard.setup_cell,
        Cell::Faction,
        "the stance chip is inert on a remote chair"
    );
    let layout = setup_layout(
        &scenario,
        draft.seat_choice,
        crate::render::viewport(),
        crate::render::ui_scale(),
    );
    let hidden = card(&layout, row).stance.unwrap().center();
    let tap = [
        RawEvent::TouchDown {
            id: 1,
            x: hidden.x,
            y: hidden.y,
        },
        RawEvent::TouchUp {
            id: 1,
            x: hidden.x,
            y: hidden.y,
        },
    ];
    wizard
        .update(&tap, &mut vec2(0.0, 0.0), &mut draft, &mut Vec::new())
        .expect("tap");
    assert_eq!(draft.seats[opponent].stance, stance);
    assert_ne!(draft.seat_choice, opponent);

    wizard.setup_cell = Cell::Seat;
    drive(&mut wizard, &mut draft, Key::Enter);
    assert_eq!(draft.seat_choice, opponent, "a remote chair can be taken");
    assert_eq!(start(&wizard, &draft), "Start match");
}

#[test]
fn compact_setup_pages_to_a_nonoverlapping_opponent_touch_target() {
    crate::render::set_viewport(640.0, 400.0);
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/compass-grand.json"
    ));
    let scenario = Scenario::load(&path).expect("shipped eight-seat map");
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(scenario, Some(path));
    let mut wizard = Wizard::open(&draft);
    wizard.goto(Step::Setup, &draft);
    assert_eq!(
        wizard.setup_page, 1,
        "Start opens on the page that contains it"
    );

    let view = crate::render::viewport();
    let ui = crate::render::ui_scale();
    let last_page = setup_layout_page(
        draft.scenario.as_deref().unwrap(),
        draft.seat_choice,
        view,
        ui,
        wizard.setup_page,
    );
    let previous = last_page
        .page_prev
        .expect("another opponent page precedes Start");
    assert!(previous.h >= MIN_TOUCH_TARGET);
    let at = previous.center();
    let mut mouse = Vec2::ZERO;
    let mut sounds = Vec::new();
    wizard
        .update(
            &[
                RawEvent::TouchDown {
                    id: 20,
                    x: at.x,
                    y: at.y,
                },
                RawEvent::TouchUp {
                    id: 20,
                    x: at.x,
                    y: at.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .unwrap();
    assert_eq!(wizard.setup_page, 0);
    assert_eq!(wizard.setup_sel, 0);

    let first_page = setup_layout_page(
        draft.scenario.as_deref().unwrap(),
        draft.seat_choice,
        view,
        ui,
        wizard.setup_page,
    );
    let row = 1;
    let difficulty = card(&first_page, row).difficulty.unwrap();
    let touch = card(&first_page, row).touch_cell(Cell::Difficulty).unwrap();
    assert!(touch.h >= MIN_TOUCH_TARGET);
    let at = vec2(touch.center().x, touch.y + 1.0);
    assert!(
        !difficulty.contains(at),
        "the probe exercises the enlarged touch-only area"
    );
    wizard
        .update(
            &[
                RawEvent::TouchDown {
                    id: 21,
                    x: at.x,
                    y: at.y,
                },
                RawEvent::TouchUp {
                    id: 21,
                    x: at.x,
                    y: at.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .unwrap();
    let seat = seat_display_order(draft.scenario.as_deref().unwrap())[row];
    assert_eq!(wizard.mode_name(), "match_setup");
    assert_eq!(draft.seats[seat].difficulty, BotDifficulty::Veteran);
    assert_eq!(draft.seats[seat].stance, BotStance::Balanced);
}

#[test]
fn compact_setup_touch_only_edges_activate_every_editable_chip() {
    crate::render::set_viewport(640.0, 400.0);
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/compass-grand.json"
    ));

    for cell in [Cell::Difficulty, Cell::Stance, Cell::Faction, Cell::Team] {
        let scenario = Scenario::load(&path).expect("shipped eight-seat map");
        let mut draft = NewMatchDraft::default();
        draft.set_scenario(scenario, Some(path.clone()));
        let order = seat_display_order(draft.scenario.as_deref().unwrap());
        let row = 1;
        let seat = order[row];
        assert_ne!(seat, draft.seat_choice);
        let initial_difficulty = draft.seats[seat].difficulty;
        let initial_stance = draft.seats[seat].stance;
        let initial_faction = draft.seats[seat].faction_choice;
        let initial_team = draft.seats[seat].team_choice;
        let mut wizard = Wizard::open(&draft);
        wizard.goto(Step::Setup, &draft);
        wizard.setup_sel = row;
        wizard.setup_page = 0;

        let layout = setup_layout_page(
            draft.scenario.as_deref().unwrap(),
            draft.seat_choice,
            crate::render::viewport(),
            crate::render::ui_scale(),
            0,
        );
        let visual = card(&layout, row).cell(cell).unwrap();
        let touch = card(&layout, row).touch_cell(cell).unwrap();
        let at = vec2(touch.center().x, touch.y + 1.0);
        assert!(
            !visual.contains(at),
            "{cell:?} probe must exercise its touch-only edge"
        );

        let mut mouse = Vec2::ZERO;
        let mut sounds = Vec::new();
        let out = wizard
            .update(
                &[
                    RawEvent::TouchDown {
                        id: 7,
                        x: at.x,
                        y: at.y,
                    },
                    RawEvent::TouchUp {
                        id: 7,
                        x: at.x,
                        y: at.y,
                    },
                ],
                &mut mouse,
                &mut draft,
                &mut sounds,
            )
            .expect("activate compact setup cell");
        assert_eq!(out, Out::Stay);
        assert_eq!(sounds, [(SoundKind::Click, None)]);
        match cell {
            Cell::Difficulty => {
                assert_eq!(
                    draft.seats[seat].difficulty,
                    cycle_difficulty(initial_difficulty, 1)
                );
                assert_eq!(draft.seats[seat].stance, initial_stance);
            }
            Cell::Stance => assert_eq!(draft.seats[seat].stance, cycle_stance(initial_stance, 1)),
            Cell::Faction => assert_eq!(
                draft.seats[seat].faction_choice,
                (initial_faction + 1) % FACTION_CHIP_ITEMS.len()
            ),
            Cell::Team => assert_eq!(
                draft.seats[seat].team_choice,
                (initial_team + 1) % (draft.seats.len() + 1)
            ),
            Cell::Seat => unreachable!("the loop covers the editable chips"),
        }
    }
}

#[test]
fn setup_mouse_activation_requires_release_on_the_armed_cell() {
    crate::render::set_viewport(1280.0, 800.0);
    let (mut wizard, mut draft) = explicit_team_setup();
    let scenario = draft.scenario.as_deref().expect("picked scenario");
    let order = seat_display_order(scenario);
    let layout = setup_layout(
        scenario,
        draft.seat_choice,
        crate::render::viewport(),
        crate::render::ui_scale(),
    );
    let row = 1;
    let seat = order[row];
    let seat_at = card(&layout, row).seat.center();
    let faction_at = card(&layout, row).faction.center();
    let team_at = card(&layout, row).team.center();
    let start_at = layout.start.unwrap().center();
    let mut mouse = Vec2::ZERO;
    let mut sounds = Vec::new();

    let out = wizard
        .update(
            &[
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: faction_at.x,
                    y: faction_at.y,
                },
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: team_at.x,
                    y: team_at.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("update");
    assert_eq!(out, Out::Stay);
    assert_eq!(draft.seats[seat].faction_choice, 0);
    assert!(sounds.is_empty(), "a canceled click is silent");

    let out = wizard
        .update(
            &[
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: faction_at.x,
                    y: faction_at.y,
                },
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: faction_at.x,
                    y: faction_at.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("update");
    assert_eq!(out, Out::Stay);
    assert_eq!(draft.seats[seat].faction_choice, 1);
    assert_eq!(wizard.setup_sel, row);
    assert_eq!(wizard.setup_cell, Cell::Faction);

    let out = wizard
        .update(
            &[
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: seat_at.x,
                    y: seat_at.y,
                },
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: seat_at.x,
                    y: seat_at.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("update");
    assert_eq!(out, Out::Stay);
    assert_eq!(draft.seat_choice, seat, "the clicked chair becomes human");

    let out = wizard
        .update(
            &[
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: start_at.x,
                    y: start_at.y,
                },
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: start_at.x,
                    y: start_at.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("update");
    assert_eq!(out, Out::Launch);
    assert_eq!(sounds.len(), 3, "each committed click has one cue");
}

#[test]
fn setup_touch_activation_belongs_to_its_first_finger_and_armed_cell() {
    crate::render::set_viewport(1280.0, 800.0);
    let (mut wizard, mut draft) = explicit_team_setup();
    let scenario = draft.scenario.as_deref().expect("picked scenario");
    let order = seat_display_order(scenario);
    let layout = setup_layout(
        scenario,
        draft.seat_choice,
        crate::render::viewport(),
        crate::render::ui_scale(),
    );
    let row = 1;
    let seat = order[row];
    let faction = card(&layout, row).faction;
    let faction_at = faction.center();
    let team_at = card(&layout, row).team.center();
    let mut mouse = Vec2::ZERO;
    let mut sounds = Vec::new();

    wizard
        .update(
            &[RawEvent::TouchDown {
                id: 7,
                x: faction_at.x,
                y: faction_at.y,
            }],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("update");
    assert_eq!(
        wizard.setup_press.armed_touch(),
        Some((
            7,
            SetupZone::Card {
                row,
                cell: Cell::Faction
            }
        ))
    );
    assert_eq!(mouse, faction_at);

    // A second finger cannot steal or resolve the first finger's press.
    wizard
        .update(
            &[
                RawEvent::TouchDown {
                    id: 8,
                    x: team_at.x,
                    y: team_at.y,
                },
                RawEvent::TouchUp {
                    id: 8,
                    x: team_at.x,
                    y: team_at.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("update");
    assert_eq!(
        wizard.setup_press.armed_touch(),
        Some((
            7,
            SetupZone::Card {
                row,
                cell: Cell::Faction
            }
        ))
    );
    assert_eq!(mouse, faction_at);

    // The owner releases over another cell, so the gesture cancels.
    wizard
        .update(
            &[
                RawEvent::TouchMove {
                    id: 7,
                    x: team_at.x,
                    y: team_at.y,
                },
                RawEvent::TouchUp {
                    id: 7,
                    x: team_at.x,
                    y: team_at.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("update");
    assert_eq!(wizard.setup_press.armed_touch(), None);
    assert_eq!(draft.seats[seat].faction_choice, 0);
    assert!(sounds.is_empty());

    // A fresh gesture may move within the armed cell and still commit.
    let inside = vec2(faction.x + 2.0, faction.y + 2.0);
    let out = wizard
        .update(
            &[
                RawEvent::TouchDown {
                    id: 9,
                    x: faction_at.x,
                    y: faction_at.y,
                },
                RawEvent::TouchMove {
                    id: 9,
                    x: inside.x,
                    y: inside.y,
                },
                RawEvent::TouchUp {
                    id: 9,
                    x: inside.x,
                    y: inside.y,
                },
            ],
            &mut mouse,
            &mut draft,
            &mut sounds,
        )
        .expect("update");
    assert_eq!(out, Out::Stay);
    assert_eq!(draft.seats[seat].faction_choice, 1);
    assert_eq!(wizard.setup_sel, row);
    assert_eq!(wizard.setup_cell, Cell::Faction);
    assert_eq!(sounds, vec![(SoundKind::Click, None)]);
}

#[test]
fn the_setup_layout_fits_the_smallest_supported_window() {
    // Eight seats at the supported extremes use two compact pages. Every
    // visible item and page control keeps a 44px logical target instead
    // of compressing the full roster into overlapping slivers.
    let scenario = Scenario::load("../scenarios/compass-grand.json").expect("shipped");
    for (view, ui) in [
        (vec2(640.0, 400.0), 0.75),
        (vec2(640.0, 400.0), 1.0),
        (vec2(960.0, 400.0), 1.5),
    ] {
        let pages = [
            setup_layout_page(&scenario, 0, view, ui, 0),
            setup_layout_page(&scenario, 0, view, ui, 1),
        ];
        assert_eq!(pages[0].page_count, 2);
        assert_eq!(pages[0].visible_range, [0, 5]);
        assert_eq!(pages[1].visible_range, [5, 9]);
        assert!(pages[0].page_next.is_some_and(|rect| rect.h >= 44.0));
        assert!(pages[1].page_prev.is_some_and(|rect| rect.h >= 44.0));
        let start = pages[1].start.expect("Start closes the last page");
        assert!(start.h >= 44.0);
        assert!(start.y + start.h <= view.y);

        let mut seen = Vec::new();
        for layout in &pages {
            for (pos, rects) in layout
                .cards
                .iter()
                .enumerate()
                .filter_map(|(pos, rects)| Some((pos, (*rects)?)))
            {
                let card = rects.card;
                seen.push(pos);
                assert!(card.h >= 44.0, "visible cards keep a 44px target");
                assert!(card.y + card.h <= view.y);
                assert!(rects.seat.w > 24.0);
                let touches: Vec<Rect> = Cell::ALL
                    .into_iter()
                    .filter_map(|cell| rects.touch_cell(cell))
                    .collect();
                for touch in &touches {
                    assert!(
                        touch.w >= MIN_TOUCH_TARGET && touch.h >= MIN_TOUCH_TARGET,
                        "every semantic setup target stays at least 44px: {touch:?}"
                    );
                    assert_eq!((touch.y, touch.h), (card.y, card.h));
                    assert!(touch.x >= card.x);
                    assert!(touch.x + touch.w <= card.x + card.w + 0.01);
                }
                for pair in touches.windows(2) {
                    assert!(
                        pair[0].x + pair[0].w <= pair[1].x + 0.01,
                        "semantic setup targets never overlap"
                    );
                }
                for chip in [rects.faction, rects.team] {
                    assert!(chip.h <= card.h + 0.01);
                    assert!(chip.x >= card.x);
                }
            }
        }
        seen.sort_unstable();
        assert_eq!(seen, (0..8).collect::<Vec<_>>());
    }
}

#[test]
fn the_setup_layout_never_overlaps_its_own_parts() {
    let scenario = Scenario::load("../scenarios/compass-grand.json").expect("shipped");
    let layout = setup_layout(&scenario, 0, vec2(1280.0, 800.0), 1.0);
    let cards: Vec<CardRects> = layout.cards.iter().flatten().copied().collect();
    assert_eq!(cards.len(), 8);
    for pair in cards.windows(2) {
        assert!(
            pair[0].card.y + pair[0].card.h <= pair[1].card.y + 0.01,
            "seat cards stack without overlap"
        );
    }
    let last = cards.last().unwrap().card;
    let start = layout.start.expect("the full layout shows Start");
    assert!(start.y >= last.y + last.h, "Start sits under the cards");
    assert!(
        layout.preview.x >= last.x + last.w,
        "the preview never crosses the cards"
    );
    assert!(
        start.y + start.h <= 800.0,
        "everything fits an 800px window"
    );
    for (pos, rects) in cards.iter().enumerate() {
        let card = rects.card;
        for r in Cell::ALL.into_iter().filter_map(|cell| rects.cell(cell)) {
            assert!(
                r.x >= card.x - 0.01
                    && r.y >= card.y - 0.01
                    && r.x + r.w <= card.x + card.w + 0.01
                    && r.y + r.h <= card.y + card.h + 0.01,
                "cell rects nest inside their card"
            );
        }
        if seat_display_order(&scenario)[pos] == 0 {
            assert_eq!((rects.difficulty, rects.stance), (None, None));
        } else {
            let (Some(difficulty), Some(stance)) = (rects.difficulty, rects.stance) else {
                panic!("every opponent carries separate difficulty and stance controls");
            };
            assert!(
                rects.seat.x + rects.seat.w <= difficulty.x + 0.01,
                "the bot controls never overlap the take-seat control"
            );
            assert!(
                difficulty.x + difficulty.w <= stance.x,
                "difficulty and stance controls never overlap"
            );
        }
    }
}

#[test]
fn seat_anchors_reads_the_authored_digits() {
    let map: Vec<String> = vec!["####".into(), "#1.#".into(), "#.2#".into()];
    assert_eq!(seat_anchors(&map), vec![(0, (1, 1)), (1, (2, 2))]);
}

#[test]
fn the_setup_card_and_its_protocol_row_show_the_retinted_name() {
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(Scenario::skirmish(), None);
    let sc = draft.scenario.as_deref().unwrap().clone();

    // Auto keeps the authored names, both seats.
    assert_eq!(effective_name(&sc, &draft, 0), "Ferrous");
    assert_eq!(effective_name(&sc, &draft, 1), "Cupric");

    // Overrides retint the label with the disc, both directions.
    draft.seats[0].faction_choice = 2; // Cupric
    draft.seats[1].faction_choice = 1; // Ferrous
    assert_eq!(effective_name(&sc, &draft, 0), "Cupric");
    assert_eq!(effective_name(&sc, &draft, 1), "Ferrous");

    // QueryUi speaks the same name: the card and the automation
    // surface can't disagree.
    let mut w = Wizard::open(&draft);
    w.step = Step::Setup;
    let (_, items, _) = w.ui_surface(&draft);
    assert_eq!(items[0], "1. Cupric (you) | Cupric | FFA");
    assert_eq!(
        items[1], "2. Ferrous | Difficulty Standard | Stance Balanced | Ferrous | FFA",
        "the protocol row matches the visible opponent card"
    );
}

#[test]
fn the_previewed_name_is_the_launched_name() {
    // The preview reads the same rule launch applies
    // (`Scenario::retint_seat`); a shell-side reimplementation of the
    // rename would diverge on a name without a faction word.
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(Scenario::skirmish(), None);
    draft.seats[0].faction_choice = 2; // Cupric
    draft.seats[1].faction_choice = 1; // Ferrous
    let sc = draft.scenario.as_deref().unwrap().clone();
    for seat in 0..sc.players.len() {
        let previewed = effective_name(&sc, &draft, seat);
        let mut launched = sc.clone();
        launched.retint_seat(seat, effective_faction(&sc, &draft, seat));
        assert_eq!(
            previewed, launched.players[seat].name,
            "seat {seat}: the card promised a name launch didn't deliver"
        );
    }
}

fn grid_entries(n: usize) -> Vec<ScenarioEntry> {
    (0..n)
        .map(|i| ScenarioEntry {
            seats: 2,
            label: format!("m{i}"),
            blurb: None,
            path: Some(PathBuf::from(format!("m{i}.json"))),
            theme: String::new(),
        })
        .collect()
}

#[test]
fn the_map_grids_visible_range_is_the_real_window() {
    let mut draft = NewMatchDraft::default();
    let mut w = Wizard::open(&draft);
    w.entries = grid_entries(24);
    w.browser = Browser::new();

    // A small window clips the grid; the range must say so and
    // must contain the selection End just scrolled to.
    crate::render::set_viewport(640.0, 400.0);
    drive(&mut w, &mut draft, Key::End);
    let view = crate::render::viewport();
    let ui = crate::render::ui_scale();
    let [first, past] = w.ui_visible_range(&draft, view, ui);
    assert!(past <= w.entries.len());
    assert!(
        past - first < w.entries.len(),
        "a 640x400 window cannot show all 24 cards ([{first}, {past}])"
    );
    assert!(
        (first..past).contains(&w.browser.selected),
        "the selection sits inside the reported window"
    );

    // A huge window shows the whole shelf; the resize guard runs
    // on the next handled frame, like the live loop.
    crate::render::set_viewport(2000.0, 4000.0);
    let mut mouse = vec2(0.0, 0.0);
    let _ = w.update(
        &[RawEvent::MouseMove { x: 0.0, y: 0.0 }],
        &mut mouse,
        &mut draft,
        &mut Vec::new(),
    );
    let view = crate::render::viewport();
    let ui = crate::render::ui_scale();
    assert_eq!(
        w.ui_visible_range(&draft, view, ui),
        [0, w.entries.len()],
        "a window tall enough for everything reports everything"
    );
}

#[test]
fn an_empty_grid_reports_an_empty_window() {
    let draft = NewMatchDraft::default();
    let mut w = Wizard::open(&draft);
    w.entries.clear();
    let view = crate::render::viewport();
    let ui = crate::render::ui_scale();
    assert_eq!(w.ui_visible_range(&draft, view, ui), [0, 0]);
}

#[test]
fn compact_setup_reports_only_the_page_actually_on_screen() {
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scenarios/compass-grand.json"
    ));
    let scenario = Scenario::load(&path).expect("shipped eight-seat map");
    let mut draft = NewMatchDraft::default();
    draft.set_scenario(scenario, Some(path));
    let mut w = Wizard::open(&draft);
    w.goto(Step::Setup, &draft);
    crate::render::set_viewport(640.0, 400.0);
    let view = crate::render::viewport();
    let ui = crate::render::ui_scale();
    let len = w.ui_surface(&draft).1.len();
    assert_eq!(len, 9);
    assert_eq!(w.ui_visible_range(&draft, view, ui), [5, 9]);
    drive(&mut w, &mut draft, Key::Home);
    assert_eq!(w.ui_visible_range(&draft, view, ui), [0, 5]);
}
