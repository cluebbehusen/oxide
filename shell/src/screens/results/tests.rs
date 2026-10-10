use super::*;

fn key(key: Key) -> RawEvent {
    RawEvent::KeyDown { key }
}

#[test]
fn actions_fit_small_and_large_viewports() {
    for viewport in [vec2(640.0, 400.0), vec2(1024.0, 768.0)] {
        let rects = action_rects(viewport, 1.0);
        assert!(rects[0].x >= 0.0);
        let last = rects.last().expect("results always has actions");
        assert!(last.x + last.w <= viewport.x);
        assert!(
            rects
                .iter()
                .all(|rect| rect.h >= crate::layout::MIN_TOUCH_TARGET)
        );
        assert!(
            rects
                .windows(2)
                .all(|pair| pair[0].x + pair[0].w < pair[1].x)
        );
    }
}

#[test]
fn eight_player_results_reserve_a_readable_small_screen_graph() {
    let layout = results_layout(vec2(640.0, 400.0), 1.0, 8);
    let last_row =
        layout.header_y + layout.rule_offset + (7.0 + layout.row_baseline) * layout.row_height;

    assert_eq!(layout.title_size, 32.0);
    assert!(layout.compact_roster);
    assert!(!layout.wide_table);
    assert_eq!(layout.row_size, 12.0);
    assert!(layout.meta_y < layout.header_y);
    assert!(last_row + layout.marker_radius < layout.graph_top);
    assert!(layout.graph_bottom - layout.graph_top >= 80.0);
    assert!(layout.graph_bottom < action_rects(vec2(640.0, 400.0), 1.0)[0].y);
}

#[test]
fn wide_results_use_room_for_readable_table_copy() {
    let layout = results_layout(vec2(1280.0, 800.0), 1.0, 2);
    let columns = table_columns(32.0, 1248.0, layout.wide_table);

    assert!(!layout.compact_roster);
    assert!(layout.wide_table);
    assert_eq!(layout.header_size, 17.0);
    assert_eq!(layout.row_size, 18.0);
    assert_eq!(layout.graph_label_size, 16.0);
    assert!(columns.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(columns[6] < 1248.0);
    assert!(layout.rule_offset > layout.header_size * 1.5);
}

#[test]
fn maximum_roster_stays_readable_above_the_actions() {
    for viewport in [vec2(640.0, 400.0), vec2(1100.0, 481.0), vec2(1100.0, 720.0)] {
        for scale in [1.0, 2.0] {
            let viewport = viewport * scale;
            let count = oxide_sim::scenario::MAX_PLAYERS;
            let layout = results_layout(viewport, scale, count);
            let last_baseline = layout.header_y
                + layout.rule_offset
                + (count as f32 - 1.0 + layout.row_baseline) * layout.row_height;
            assert!(layout.row_height >= layout.row_size);
            assert!(last_baseline + layout.row_size * 0.25 < action_rects(viewport, scale)[0].y);
        }
    }
}

#[test]
fn compact_columns_and_names_fit_without_breaking_unicode() {
    let columns = table_columns(20.0, 620.0, false);
    assert_eq!(columns[0], 20.0);
    assert!(columns.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(columns[6] < 620.0);
    assert_eq!(clipped_name("short", 8), "short");
    assert_eq!(clipped_name("cupréous", 7), "cupr...");
}

#[test]
fn keyboard_wraps_and_escape_goes_home() {
    let mut screen = ResultsScreen::open();
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    assert_eq!(
        screen.update(
            &[key(Key::Left), key(Key::Enter)],
            &mut mouse,
            vec2(640.0, 400.0),
            1.0,
            &mut sounds,
        ),
        Out::Home
    );
    assert_eq!(screen.selected(), 3);
    assert_eq!(
        screen.update(
            &[key(Key::Escape)],
            &mut mouse,
            vec2(640.0, 400.0),
            1.0,
            &mut sounds,
        ),
        Out::Home
    );
}

#[test]
fn home_and_end_jump_to_the_first_and_last_action() {
    let mut screen = ResultsScreen::open();
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    let mut press = |screen: &mut ResultsScreen, k: Key| {
        screen.update(&[key(k)], &mut mouse, vec2(640.0, 400.0), 1.0, &mut sounds)
    };
    assert_eq!(press(&mut screen, Key::End), Out::Stay);
    assert_eq!(screen.selected(), 3);
    assert_eq!(press(&mut screen, Key::Home), Out::Stay);
    assert_eq!(screen.selected(), 0);
    press(&mut screen, Key::PageDown);
    assert_eq!(screen.selected(), 3, "paging stops at the last action");
}

#[test]
fn final_map_action_is_touchable_and_named_for_automation() {
    let viewport = vec2(640.0, 400.0);
    let rect = action_rects(viewport, 1.0)[2];
    let at = vec2(rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
    let mut screen = ResultsScreen::open();
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();

    assert_eq!(ResultsScreen::items()[2], "VIEW FINAL MAP");
    assert_eq!(
        screen.update(
            &[
                RawEvent::TouchDown {
                    id: 11,
                    x: at.x,
                    y: at.y,
                },
                RawEvent::TouchUp {
                    id: 11,
                    x: at.x,
                    y: at.y,
                },
            ],
            &mut mouse,
            viewport,
            1.0,
            &mut sounds,
        ),
        Out::ViewFinalMap
    );
}

#[test]
fn results_name_shows_the_scripted_controller() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.players[1].bot_config = Some(oxide_sim::scenario::BotConfig::new(
        oxide_sim::scenario::BotDifficulty::Prime,
        oxide_sim::scenario::BotStance::Aggressive,
        19,
    ));
    let game = Game::new(scenario).expect("skirmish builds");
    assert_eq!(
        player_name_with_controller(&game, 1, 24, false),
        "Cupric  PRIME / AGGRESSIVE AI"
    );
    assert_eq!(
        player_name_with_controller(&game, 1, 24, true),
        "Cupric  PRIME/AGGRO"
    );
    assert_eq!(player_name_with_controller(&game, 0, 24, false), "Ferrous");
}

#[test]
fn long_names_and_the_longest_bot_profile_stay_inside_the_player_column() {
    let mut scenario = oxide_sim::Scenario::skirmish();
    scenario.players[1].name = "Extremely Long Cupric Commander".to_string();
    scenario.players[1].bot_config = Some(oxide_sim::scenario::BotConfig::new(
        oxide_sim::scenario::BotDifficulty::Scrapheap,
        oxide_sim::scenario::BotStance::Aggressive,
        23,
    ));
    let game = Game::new(scenario).expect("skirmish builds");

    for viewport in [vec2(640.0, 400.0), vec2(1280.0, 800.0)] {
        let scale = 1.0;
        let layout = results_layout(viewport, scale, game.state.players().len());
        let panel = Rect::new(12.0, 10.0, viewport.x - 24.0, viewport.y - 20.0);
        let left = panel.x + 20.0;
        let right = panel.x + panel.w - 20.0;
        let columns = table_columns(left, right, layout.wide_table);
        let player = &game.state.players()[1];
        let prefix = format!("T{}  ", player.team + 1);
        let player_text_x = columns[0] + 11.0;
        let column_right = player_column_right(left, right, layout.wide_table);
        let measure = |text: &str| text.chars().count() as f32 * layout.row_size * 0.58;
        let fixed_width = measure(&prefix);
        let available = column_right - player_text_x - 8.0;
        let max_name_width = (available - fixed_width).max(0.0);
        let max_name_chars = if viewport.x < 800.0 { 12 } else { 24 };
        let name = fitted_player_name_with_controller(
            &game,
            1,
            max_name_chars,
            layout.compact_roster,
            max_name_width,
            &measure,
        );
        let row = format!("{prefix}{name}");

        assert!(
            name.contains("SCRAP"),
            "the fitted label keeps the difficulty at {viewport:?}: {name}"
        );
        assert!(
            name.contains("AGG"),
            "the fitted label keeps the stance at {viewport:?}: {name}"
        );
        assert!(
            measure(&row) <= available + 0.01,
            "`{row}` crosses the player column at {viewport:?}"
        );
    }

    let narrow_layout = results_layout(vec2(640.0, 400.0), 1.0, 2);
    let narrow_measure = |text: &str| text.chars().count() as f32 * narrow_layout.row_size * 0.58;
    let compact = fitted_player_name_with_controller(
        &game,
        1,
        12,
        narrow_layout.compact_roster,
        175.0,
        &narrow_measure,
    );
    assert!(
        compact.contains("SCRAP/AGGRO"),
        "a narrow duel uses the semantic compact label: {compact}"
    );
}

#[test]
fn touch_activation_requires_the_arming_finger_and_same_action() {
    let viewport = vec2(640.0, 400.0);
    let rematch = action_rects(viewport, 1.0)[0].center();
    let watch = action_rects(viewport, 1.0)[1].center();
    let mut screen = ResultsScreen::open();
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();

    assert_eq!(
        screen.update(
            &[RawEvent::TouchDown {
                id: 7,
                x: watch.x,
                y: watch.y,
            }],
            &mut mouse,
            viewport,
            1.0,
            &mut sounds,
        ),
        Out::Stay
    );
    assert_eq!(screen.press.armed_touch(), Some((7, 1)));

    // A second finger cannot move or resolve the first finger's gesture.
    assert_eq!(
        screen.update(
            &[
                RawEvent::TouchMove {
                    id: 8,
                    x: rematch.x,
                    y: rematch.y,
                },
                RawEvent::TouchUp {
                    id: 8,
                    x: rematch.x,
                    y: rematch.y,
                },
            ],
            &mut mouse,
            viewport,
            1.0,
            &mut sounds,
        ),
        Out::Stay
    );
    assert_eq!(screen.press.armed_touch(), Some((7, 1)));
    assert_eq!(mouse, watch);

    // The owning finger releases on another action, canceling the press.
    assert_eq!(
        screen.update(
            &[
                RawEvent::TouchMove {
                    id: 7,
                    x: rematch.x,
                    y: rematch.y,
                },
                RawEvent::TouchUp {
                    id: 7,
                    x: rematch.x,
                    y: rematch.y,
                },
            ],
            &mut mouse,
            viewport,
            1.0,
            &mut sounds,
        ),
        Out::Stay
    );
    assert_eq!(screen.press.armed_touch(), None);
    assert_eq!(screen.selected(), 0);
    assert!(sounds.is_empty(), "a canceled touch is silent");

    // Cancellation releases ownership so the next gesture can commit.
    assert_eq!(
        screen.update(
            &[
                RawEvent::TouchDown {
                    id: 9,
                    x: watch.x,
                    y: watch.y,
                },
                RawEvent::TouchUp {
                    id: 9,
                    x: watch.x,
                    y: watch.y,
                },
            ],
            &mut mouse,
            viewport,
            1.0,
            &mut sounds,
        ),
        Out::Watch
    );
    assert_eq!(screen.selected(), 1);
    assert_eq!(sounds, vec![(SoundKind::Click, None)]);
}

#[test]
fn mouse_hover_is_inert_and_clicks_commit_only_on_the_armed_action() {
    let viewport = vec2(640.0, 400.0);
    let rematch = action_rects(viewport, 1.0)[0].center();
    let watch = action_rects(viewport, 1.0)[1].center();
    let mut screen = ResultsScreen::open();
    let mut mouse = Vec2::ZERO;
    let mut sounds = Vec::new();

    assert_eq!(
        screen.update(
            &[RawEvent::MouseMove {
                x: watch.x,
                y: watch.y,
            }],
            &mut mouse,
            viewport,
            1.0,
            &mut sounds,
        ),
        Out::Stay
    );
    assert_eq!(screen.hover(), Some(1));
    assert_eq!(screen.selected(), 0, "hover does not move the key cursor");

    assert_eq!(
        screen.update(
            &[
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: rematch.x,
                    y: rematch.y,
                },
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: watch.x,
                    y: watch.y,
                },
            ],
            &mut mouse,
            viewport,
            1.0,
            &mut sounds,
        ),
        Out::Stay,
        "dragging between actions cancels the click"
    );
    assert_eq!(
        screen.update(
            &[
                RawEvent::MouseDown {
                    button: MouseButton::Left,
                    x: rematch.x,
                    y: rematch.y,
                },
                RawEvent::MouseUp {
                    button: MouseButton::Left,
                    x: rematch.x,
                    y: rematch.y,
                },
            ],
            &mut mouse,
            viewport,
            1.0,
            &mut sounds,
        ),
        Out::Rematch
    );
    assert_eq!(sounds, vec![(SoundKind::Click, None)]);
}

#[test]
fn verdict_copy_distinguishes_winning_from_surrendering() {
    let mut victory = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
    victory.stage(oxide_sim::PlayerCommand {
        player: PlayerId(1),
        command: oxide_sim::Command::Surrender,
    });
    victory.present_ticks(1);
    let (title, _, subtitle) = verdict(&victory);
    assert_eq!(title, "VICTORY");
    assert!(subtitle.contains("FERROUS"));

    let mut surrendered = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
    surrendered.issue(oxide_sim::Command::Surrender);
    surrendered.present_ticks(1);
    let (title, _, subtitle) = verdict(&surrendered);
    assert_eq!(title, "SURRENDERED");
    assert_eq!(subtitle, "your machines fell silent");
}

#[test]
fn duration_uses_sim_time() {
    assert_eq!(format_duration(0), "00:00");
    assert_eq!(format_duration(u64::from(TICKS_PER_SECOND) * 125), "02:05");
}

#[test]
fn graph_ceiling_uses_readable_steps() {
    for (value, expected) in [
        (0, 1),
        (1, 1),
        (2, 2),
        (3, 5),
        (11, 20),
        (52, 100),
        (501, 1_000),
    ] {
        assert_eq!(graph_ceiling(value), expected);
    }
}

#[test]
fn graph_uses_sample_time_and_keeps_every_segment() {
    let plot = Rect::new(10.0, 20.0, 100.0, 50.0);
    let points = graph_points(&[0, 41, 82, 100], &[0, 10, 40, 100], 100, 100, plot);
    assert_eq!(points.len(), 4);
    assert_eq!(points.windows(2).count(), 3);
    assert!((points[1].x - 51.0).abs() < f32::EPSILON);
    assert!((points[2].x - 92.0).abs() < f32::EPSILON);
    assert_eq!(points[3], vec2(110.0, 20.0));
}

#[test]
fn every_supported_seat_has_a_distinct_results_marker() {
    let markers = (0..8).map(seat_marker).collect::<Vec<_>>();
    for (index, marker) in markers.iter().enumerate() {
        assert!(
            !markers[..index].contains(marker),
            "seat {index} aliases an earlier graph marker"
        );
    }
}
