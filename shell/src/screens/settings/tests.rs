use super::*;
use macroquad::prelude::vec2;

/// Where `row` sits on this build's settings face.
fn index(row: Row) -> usize {
    rows(crate::platform::TOUCH_ONLY)
        .iter()
        .position(|offered| *offered == row)
        .expect("the row is offered on this build")
}

/// The Controls face's lines before Reset: the action each row binds,
/// `None` for section headings.
fn control_rows() -> Vec<Option<Action>> {
    controls_lines(&Config::default(), 0)
        .iter()
        .filter_map(|line| match line {
            Line::Header(_) => Some(None),
            Line::Row(_, Item::Binding(action)) => Some(Some(*action)),
            Line::Row(_, Item::ResetBindings | Item::Setting(_)) => None,
        })
        .collect()
}

#[test]
fn a_capture_prompt_is_information_and_key_help_is_coaching() {
    let config = Config::default();
    let mut screen = SettingsScreen::open(&config);
    assert_eq!(screen.subtitle(), "");
    assert!(
        screen.coaching().is_some(),
        "the settings help waits until stuck"
    );
    screen.goto_controls(&config, 1);
    assert!(screen.coaching().is_some(), "the controls key help too");
    screen.face = Face::Controls {
        rebinding: Some(Action::PanUp),
    };
    assert!(
        screen.subtitle().starts_with("press the new chord"),
        "an armed capture always says it is listening"
    );
    assert_eq!(screen.coaching(), None);
}

#[test]
fn the_settings_hint_speaks_touch_on_touch_only_builds() {
    assert!(settings_hint(false).contains("{confirm}"));
    crate::platform::assert_touch_copy(settings_hint(true));
}

#[test]
fn a_touch_only_build_hides_rows_it_cannot_use() {
    let touch = rows(true);
    for hidden in [
        Row::EdgePan,
        Row::LeftHandedPreset,
        Row::Controls,
        Row::OpenDiagnostics,
    ] {
        assert!(!touch.contains(&hidden), "{hidden:?} needs a desktop");
    }
    assert_eq!(touch.len(), Row::ALL.len() - 4);
    assert_eq!(rows(false), Row::ALL.to_vec(), "desktop keeps every row");
}

#[test]
fn marker_settings_apply_live_and_remain_touch_reachable_in_small_windows() {
    crate::render::set_viewport(640.0, 400.0);
    crate::render::set_user_scale(1.5);
    let mut config = Config::default();
    let mut screen = SettingsScreen::open(&config);
    screen.menu.select(index(Row::MarkerTiming));
    for (label, midpoint) in [("Earlier", 26.0), ("Later", 14.0), ("Standard", 20.0)] {
        let update = drive(&mut screen, &mut config, &press(Key::Enter), false);
        assert!(update.dirty);
        assert!(screen.menu.view().items[index(Row::MarkerTiming)].ends_with(label));
        assert_eq!(crate::strategic_markers::marker_alpha(midpoint), 0.5);
    }
    screen.menu.select(index(Row::MarkerSize));
    for size in [1.25, 1.5, 0.75, 1.0] {
        let rect = screen
            .menu
            .item_rect(index(Row::MarkerSize))
            .expect("selected size row visible");
        let (x, y) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
        let update = drive(
            &mut screen,
            &mut config,
            &[
                RawEvent::TouchDown { id: 1, x, y },
                RawEvent::TouchUp { id: 1, x, y },
            ],
            false,
        );
        assert!(update.dirty);
        assert_eq!(config.markers.scale, size);
        assert_eq!(crate::strategic_markers::prefs(), config.markers);
    }
    crate::render::set_viewport(1280.0, 800.0);
    crate::render::set_user_scale(1.0);
}

#[test]
fn the_back_button_steps_out_one_level_like_escape() {
    let mut config = Config::default();
    let mut screen = SettingsScreen::open(&config);
    assert!(!screen.menu.view().items.iter().any(|item| item == "Back"));
    let back = crate::button::press_back(true);

    screen.goto_controls(&config, 1);
    screen.face = Face::Controls {
        rebinding: Some(Action::PanUp),
    };
    let update = drive(&mut screen, &mut config, &back, false);
    assert_eq!(
        screen.face,
        Face::Controls { rebinding: None },
        "cancels capture"
    );
    assert!(!update.dirty, "a cancelled capture binds nothing");
    assert_eq!(config.bindings, BindingMap::classic());

    drive(&mut screen, &mut config, &back, false);
    assert_eq!(screen.face, Face::Settings);
    assert_eq!(screen.menu.selected, index(Row::Controls));

    let update = drive(&mut screen, &mut config, &back, false);
    assert_eq!(update.out, Out::Leave);
}

fn drive(s: &mut SettingsScreen, config: &mut Config, events: &[RawEvent], ctrl0: bool) -> Update {
    let mut mouse = vec2(0.0, 0.0);
    let mut sounds = Vec::new();
    s.update(events, &mut mouse, &mut sounds, config, ctrl0, false)
}

fn press(key: Key) -> Vec<RawEvent> {
    vec![RawEvent::KeyDown { key }, RawEvent::KeyUp { key }]
}

#[test]
fn cycling_a_row_edits_the_config_and_reports_dirty() {
    let mut config = Config::default();
    let mut s = SettingsScreen::open(&config);
    for _ in 0..index(Row::ReducedMotion) {
        drive(&mut s, &mut config, &press(Key::Down), false);
    }
    let up = drive(&mut s, &mut config, &press(Key::Enter), false);
    assert!(config.reduced_motion);
    assert!(up.dirty, "the caller is told to persist");
    assert_eq!(
        s.menu.selected,
        index(Row::ReducedMotion),
        "the cursor stays on the tuned row"
    );
}

#[test]
fn music_volume_is_a_live_persisted_settings_row() {
    let mut config = Config::default();
    let mut screen = SettingsScreen::open(&config);
    let music = index(Row::MusicVolume);
    for _ in 0..music {
        drive(&mut screen, &mut config, &press(Key::Down), false);
    }
    let update = drive(&mut screen, &mut config, &press(Key::Enter), false);
    assert!(update.dirty);
    assert_eq!(config.volumes.music, 0.0);
    assert_eq!(screen.menu.selected, music);
    assert_eq!(screen.menu.view().items[music], "Music volume: 0%");
}

#[test]
fn music_volume_is_touch_reachable() {
    crate::render::set_viewport(1280.0, 800.0);
    crate::render::set_user_scale(1.0);
    let mut config = Config::default();
    let mut screen = SettingsScreen::open(&config);
    let row = screen.menu.item_rect(3).expect("music row is visible");
    let x = row.x + row.w * 0.5;
    let y = row.y + row.h * 0.5;
    // Another layout test must not move the row between measurement and input.
    std::thread::spawn(|| crate::render::set_user_scale(0.75))
        .join()
        .unwrap();
    let update = drive(
        &mut screen,
        &mut config,
        &[
            RawEvent::TouchDown { id: 11, x, y },
            RawEvent::TouchUp { id: 11, x, y },
        ],
        false,
    );
    assert!(update.dirty);
    assert_eq!(config.volumes.music, 0.0);
    assert_eq!(screen.menu.selected, 3);
}

#[test]
fn the_control_group_row_toggles_the_column() {
    let mut config = Config::default();
    assert_eq!(Row::ControlGroups.label(&config), "Control groups: on");
    cycle_setting(&mut config, Row::ControlGroups);
    assert!(!config.control_groups);
    assert_eq!(Row::ControlGroups.label(&config), "Control groups: off");
    assert!(!render::control_groups(), "the HUD follows at once");
    cycle_setting(&mut config, Row::ControlGroups);
    assert!(render::control_groups());
    assert!(rows(true).contains(&Row::ControlGroups));
    assert!(rows(false).contains(&Row::ControlGroups));
}

#[test]
fn performance_cycles_with_keyboard_mouse_and_touch_in_small_windows() {
    use crate::config::PerformanceDisplay;
    use oxide_protocol::MouseButton;
    for scale in [0.75, 1.0, 1.25, 1.5] {
        crate::render::set_viewport(640.0, 400.0);
        crate::render::set_user_scale(scale);
        let mut config = Config::default();
        let mut screen = SettingsScreen::open(&config);
        for _ in 0..index(Row::PerformanceDisplay) {
            drive(&mut screen, &mut config, &press(Key::Down), false);
        }
        for mode in [
            PerformanceDisplay::Fps,
            PerformanceDisplay::Detailed,
            PerformanceDisplay::Off,
        ] {
            let rect = screen
                .menu
                .item_rect(index(Row::PerformanceDisplay))
                .expect("selected row visible");
            let (x, y) = (rect.center().x, rect.center().y);
            let events = match mode {
                PerformanceDisplay::Fps => press(Key::Enter),
                PerformanceDisplay::Detailed => vec![
                    RawEvent::MouseDown {
                        button: MouseButton::Left,
                        x,
                        y,
                    },
                    RawEvent::MouseUp {
                        button: MouseButton::Left,
                        x,
                        y,
                    },
                ],
                PerformanceDisplay::Off => vec![
                    RawEvent::TouchDown { id: 1, x, y },
                    RawEvent::TouchUp { id: 1, x, y },
                ],
            };
            let update = drive(&mut screen, &mut config, &events, false);
            assert!(update.dirty);
            assert_eq!(config.performance_display, mode);
            assert_eq!(screen.menu.selected, index(Row::PerformanceDisplay));
            assert_eq!(
                screen.menu.view().items[index(Row::PerformanceDisplay)],
                format!("Performance display: {}", mode.label())
            );
        }
        assert_eq!(
            screen.menu.view().items[index(Row::LeftHandedPreset)],
            "Apply left-handed bindings"
        );
        assert_eq!(
            screen.menu.view().items[index(Row::Controls)],
            "Controls..."
        );
    }
}

#[test]
fn a_held_modifier_from_another_screen_rides_into_the_captured_chord() {
    // Ctrl went down on the Settings face, so no Ctrl edge appears in
    // this frame's events; only the baseline knows. The capture must
    // still record Ctrl+K.
    let mut config = Config::default();
    let mut s = SettingsScreen::open(&config);
    s.goto_controls(
        &config,
        control_rows()
            .iter()
            .position(|a| *a == Some(Action::Patrol))
            .unwrap(),
    ); // Patrol row
    drive(&mut s, &mut config, &press(Key::Enter), true);
    assert!(matches!(s.face, Face::Controls { rebinding: Some(_) }));
    let up = drive(&mut s, &mut config, &press(Key::K), true);
    assert!(up.dirty);
    assert_eq!(
        config.bindings.chord_for(Action::Patrol),
        Some(Chord {
            key: Key::K,
            ctrl: true,
            shift: false
        }),
        "the held Ctrl rides into the chord"
    );
}

#[test]
fn a_chord_released_within_the_frame_still_reads_its_modifiers() {
    // The whole chord in one batch: Ctrl down, K down, K up, Ctrl up.
    // Batch-final modifier state is false, so only reading the modifiers
    // at the key's press gets this right.
    let mut config = Config::default();
    let mut s = SettingsScreen::open(&config);
    s.goto_controls(
        &config,
        control_rows()
            .iter()
            .position(|a| *a == Some(Action::Patrol))
            .unwrap(),
    );
    drive(&mut s, &mut config, &press(Key::Enter), false);
    let batch = vec![
        RawEvent::KeyDown { key: Key::Ctrl },
        RawEvent::KeyDown { key: Key::K },
        RawEvent::KeyUp { key: Key::K },
        RawEvent::KeyUp { key: Key::Ctrl },
    ];
    drive(&mut s, &mut config, &batch, false);
    assert_eq!(
        config.bindings.chord_for(Action::Patrol),
        Some(Chord {
            key: Key::K,
            ctrl: true,
            shift: false
        })
    );
}

#[test]
fn a_conflicting_chord_is_refused_and_the_notice_names_the_holder() {
    let mut config = Config::default();
    let before = config.bindings.chord_for(Action::Patrol);
    let mut s = SettingsScreen::open(&config);
    s.goto_controls(
        &config,
        control_rows()
            .iter()
            .position(|a| *a == Some(Action::Patrol))
            .unwrap(),
    );
    drive(&mut s, &mut config, &press(Key::Enter), false);
    let up = drive(&mut s, &mut config, &press(Key::G), false);
    assert!(!up.dirty);
    let notice = s.notice.as_ref().expect("the refusal reports");
    assert_eq!(notice.text, "G is already bound to Run");
    assert!(notice.danger);
    assert_eq!(config.bindings.chord_for(Action::Patrol), before);
    // Navigation is not an action: the notice waits to be read.
    drive(&mut s, &mut config, &press(Key::Down), false);
    assert!(s.notice.is_some(), "arrow keys must not eat the notice");
    // The next action clears it.
    drive(&mut s, &mut config, &press(Key::Enter), false);
    assert!(s.notice.is_none(), "arming a row is the next action");
}

#[test]
fn a_conflict_with_a_non_remappable_holder_is_still_named() {
    // Enter is Confirm: not on the remap screen, but a reachable
    // collision whose holder must still be named.
    let mut config = Config::default();
    let mut s = SettingsScreen::open(&config);
    s.goto_controls(
        &config,
        control_rows()
            .iter()
            .position(|a| *a == Some(Action::Patrol))
            .unwrap(),
    );
    drive(&mut s, &mut config, &press(Key::Enter), false);
    drive(&mut s, &mut config, &press(Key::Num1), false);
    assert_eq!(
        s.notice.as_ref().map(|n| n.text.as_str()),
        Some("1 is already bound to Recall group 1")
    );
}

#[test]
fn leaving_the_controls_face_clears_the_notice() {
    let mut config = Config::default();
    let mut s = SettingsScreen::open(&config);
    s.goto_controls(
        &config,
        control_rows()
            .iter()
            .position(|a| *a == Some(Action::Patrol))
            .unwrap(),
    );
    drive(&mut s, &mut config, &press(Key::Enter), false);
    drive(&mut s, &mut config, &press(Key::G), false);
    assert!(s.notice.is_some());
    drive(&mut s, &mut config, &press(Key::Escape), false);
    assert!(
        matches!(s.face, Face::Settings) && s.notice.is_none(),
        "a face change retires the notice with its context"
    );
}

#[test]
fn the_left_handed_preset_moves_the_verbs_and_reset_walks_home() {
    let mut config = Config::default();
    let mut s = SettingsScreen::open(&config);
    for _ in 0..index(Row::LeftHandedPreset) {
        drive(&mut s, &mut config, &press(Key::Down), false);
    }
    let up = drive(&mut s, &mut config, &press(Key::Enter), false);
    assert!(up.dirty);
    assert!(
        s.notice.as_ref().is_some_and(|n| !n.danger),
        "the applied preset confirms on the screen's own notice line"
    );
    assert_eq!(
        config.bindings.chord_for(Action::TrainSlot(0)),
        Some(Chord::bare(Key::O)),
        "training moved to the right hand"
    );
    assert_eq!(
        config.bindings.chord_at(Action::PanLeft, 1),
        Some(Chord::bare(Key::Left)),
        "pans stay on the arrows"
    );
    assert!(
        config.bindings.conflicts().is_empty(),
        "the preset must be conflict-free"
    );
    assert_eq!(
        config.bindings.chord_for(Action::Patrol),
        Some(Chord::bare(Key::Y))
    );
}

#[test]
fn x_unbinds_and_reset_restores_the_classic_map() {
    let mut config = Config::default();
    let mut s = SettingsScreen::open(&config);
    s.goto_controls(
        &config,
        control_rows()
            .iter()
            .position(|a| *a == Some(Action::Patrol))
            .unwrap(),
    );
    let up = drive(&mut s, &mut config, &press(Key::X), false);
    assert!(up.dirty);
    assert_eq!(config.bindings.chord_for(Action::Patrol), None);
    // Reset row restores everything.
    s.menu.select(control_rows().len());
    drive(&mut s, &mut config, &press(Key::Enter), false);
    assert_eq!(
        config.bindings.chord_for(Action::Patrol),
        BindingMap::classic().chord_for(Action::Patrol)
    );
}

#[test]
fn escaping_controls_returns_the_cursor_to_the_controls_row() {
    // Leaving Controls must land on the Controls row, so the next Enter
    // reopens the remap screen instead of toggling a neighbouring
    // setting.
    let mut config = Config::default();
    let mut screen = SettingsScreen::open(&config);
    assert_eq!(
        settings_menu(&config).view().items[index(Row::Controls)],
        "Controls...",
        "the derived index names the row it claims"
    );
    screen.menu.select(index(Row::Controls));
    drive(&mut screen, &mut config, &press(Key::Enter), false);
    assert!(matches!(screen.face, Face::Controls { .. }));
    drive(&mut screen, &mut config, &press(Key::Escape), false);
    assert!(matches!(screen.face, Face::Settings));
    assert_eq!(
        screen.menu.selected,
        index(Row::Controls),
        "the cursor comes back to the row that was activated"
    );
}
#[test]
fn every_default_action_is_editable_and_secondary_edit_does_not_replace_primary() {
    let mut config = Config::default();
    let rows = control_rows();
    for binding in config.bindings.bindings() {
        assert!(rows.contains(&Some(binding.action)), "{:?}", binding.action);
    }
    let row = rows.iter().position(|a| *a == Some(Action::PanUp)).unwrap();
    let mut screen = SettingsScreen::open(&config);
    screen.goto_controls(&config, row);
    drive(&mut screen, &mut config, &press(Key::Right), false);
    drive(&mut screen, &mut config, &press(Key::Enter), false);
    assert!(drive(&mut screen, &mut config, &press(Key::I), false).dirty);
    assert_eq!(
        config.bindings.chord_at(Action::PanUp, 0),
        Some(Chord::bare(Key::W))
    );
    assert_eq!(
        config.bindings.chord_at(Action::PanUp, 1),
        Some(Chord::bare(Key::I))
    );
    drive(&mut screen, &mut config, &press(Key::X), false);
    assert_eq!(config.bindings.chord_at(Action::PanUp, 1), None);
    assert_eq!(
        config.bindings.chord_at(Action::PanUp, 0),
        Some(Chord::bare(Key::W))
    );
}
#[test]
fn clicking_the_secondary_column_selects_it_even_when_release_is_a_later_frame() {
    use oxide_protocol::MouseButton;
    let mut config = Config::default();
    let mut screen = SettingsScreen::open(&config);
    let row = control_rows()
        .iter()
        .position(|a| *a == Some(Action::PanUp))
        .unwrap();
    screen.goto_controls(&config, row);
    let (x, y) = pan_up_columns(&screen).secondary.center().into();
    drive(
        &mut screen,
        &mut config,
        &[RawEvent::MouseDown {
            button: MouseButton::Left,
            x,
            y,
        }],
        false,
    );
    assert!(matches!(screen.face, Face::Controls { rebinding: None }));
    drive(
        &mut screen,
        &mut config,
        &[RawEvent::MouseUp {
            button: MouseButton::Left,
            x,
            y,
        }],
        false,
    );
    assert_eq!(screen.binding_slot, 1);
    assert!(drive(&mut screen, &mut config, &press(Key::I), false).dirty);
    assert_eq!(
        config.bindings.chord_at(Action::PanUp, 0),
        Some(Chord::bare(Key::W))
    );
    assert_eq!(
        config.bindings.chord_at(Action::PanUp, 1),
        Some(Chord::bare(Key::I))
    );
}

#[test]
fn chord_columns_wrap_and_tab_steps_right() {
    let mut config = Config::default();
    let mut screen = SettingsScreen::open(&config);
    screen.goto_controls(&config, 1);
    assert_eq!(screen.binding_slot, 0);
    drive(&mut screen, &mut config, &press(Key::Left), false);
    assert_eq!(
        screen.binding_slot, 1,
        "left from primary wraps to secondary"
    );
    drive(&mut screen, &mut config, &press(Key::Right), false);
    assert_eq!(
        screen.binding_slot, 0,
        "right from secondary wraps to primary"
    );
    drive(&mut screen, &mut config, &press(Key::Tab), false);
    assert_eq!(screen.binding_slot, 1);
    drive(&mut screen, &mut config, &press(Key::Tab), false);
    assert_eq!(screen.binding_slot, 0);
}

/// The Pan up row's columns, as the Controls face draws them.
fn pan_up_columns(screen: &SettingsScreen) -> crate::menu::BindingColumns {
    let row = control_rows()
        .iter()
        .position(|a| *a == Some(Action::PanUp))
        .unwrap();
    let rect = screen.menu.item_rect(row).unwrap();
    crate::menu::BindingColumns::of(rect, crate::render::ui_scale())
}

/// Opens Controls on the Pan up row with `slot` chosen, clicks the row
/// at `x`, and returns the chosen slot and the row's label.
fn click_pan_up_at(
    slot: usize,
    x: impl Fn(&crate::menu::BindingColumns) -> f32,
) -> (usize, String) {
    use oxide_protocol::MouseButton;
    let mut config = Config::default();
    let mut screen = SettingsScreen::open(&config);
    let row = control_rows()
        .iter()
        .position(|a| *a == Some(Action::PanUp))
        .unwrap();
    screen.binding_slot = slot;
    screen.goto_controls(&config, row);
    let columns = pan_up_columns(&screen);
    let (x, y) = (x(&columns), columns.primary.center().y);
    drive(
        &mut screen,
        &mut config,
        &[
            RawEvent::MouseDown {
                button: MouseButton::Left,
                x,
                y,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                x,
                y,
            },
        ],
        false,
    );
    assert_eq!(
        screen.face,
        Face::Controls {
            rebinding: Some(Action::PanUp)
        }
    );
    (screen.binding_slot, screen.menu.view().items[row].clone())
}

#[test]
fn a_click_picks_the_chord_column_it_lands_in() {
    assert_eq!(click_pan_up_at(0, |c| c.secondary.x + 1.0).0, 1);
    assert_eq!(click_pan_up_at(1, |c| c.secondary.x - 1.0).0, 0);
    assert_eq!(click_pan_up_at(1, |c| c.primary.x + 1.0).0, 0);
}

#[test]
fn a_click_on_the_action_name_keeps_the_chosen_column() {
    assert_eq!(click_pan_up_at(1, |c| c.name.center().x).0, 1);
    assert_eq!(click_pan_up_at(0, |c| c.name.center().x).0, 0);
}

#[test]
fn the_brackets_follow_the_clicked_column_while_capturing() {
    let (_, label) = click_pan_up_at(0, |c| c.secondary.center().x);
    let (_, keys) = label.rsplit_once(": ").unwrap();
    let (primary, secondary) = keys.split_once(" | ").unwrap();
    assert!(!primary.starts_with('['), "{label}");
    assert!(
        secondary.starts_with('[') && secondary.ends_with(']'),
        "{label}"
    );
}

#[test]
fn diagnostics_rows_survive_without_a_capture_toggle() {
    let config = Config::default();
    let menu = settings_menu(&config);
    assert!(
        !menu
            .view()
            .items
            .iter()
            .any(|item| item.starts_with("Diagnostics:"))
    );
    assert_eq!(
        menu.view().items[index(Row::OpenDiagnostics)],
        "Open diagnostics folder"
    );
    assert_eq!(
        menu.view().items[index(Row::ExportDiagnostics)],
        "Export diagnostic report"
    );
    assert_eq!(menu.view().items[index(Row::Controls)], "Controls...");
    let mut old = serde_json::to_value(&config).unwrap();
    old.as_object_mut()
        .unwrap()
        .insert("diagnostics".into(), true.into());
    assert!(
        serde_json::from_value::<Config>(old).is_ok(),
        "a config saved with the old toggle still loads"
    );
}
