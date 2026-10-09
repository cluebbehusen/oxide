use super::*;

#[test]
fn exact_chords_beat_bare_ones_and_bare_survives_modifiers() {
    let map = BindingMap::classic();
    assert_eq!(
        map.resolve_in(Key::Num1, true, false, Context::Units),
        Some(Action::AssignGroup(1)),
        "ctrl+1 is its own meaning"
    );
    assert_eq!(
        map.resolve_in(Key::Num1, false, false, Context::Units),
        Some(Action::Slot(1))
    );
    assert_eq!(
        map.resolve_in(Key::Q, true, false, Context::Production),
        Some(Action::TrainSlot(0)),
        "a held modifier never mutes an unmodified binding"
    );
}

#[test]
fn rebinding_refuses_collisions_and_moves_the_chord() {
    let mut map = BindingMap::classic();
    assert!(
        !map.rebind(Action::StopOrScrap, Chord::bare(Key::G)),
        "G belongs to Run in the unit context"
    );
    assert!(map.rebind(Action::StopOrScrap, Chord::ctrl(Key::X)));
    assert_eq!(
        map.resolve_in(Key::X, true, false, Context::Units),
        Some(Action::StopOrScrap)
    );
    assert_eq!(
        map.resolve_in(Key::X, false, false, Context::Units),
        None,
        "the old chord is gone, not shadowed"
    );
}

#[test]
fn no_classic_chord_points_past_the_group_count() {
    let map = BindingMap::classic();
    assert!(
        map.bindings().iter().all(|b| match b.action {
            Action::AssignGroup(n) | Action::Slot(n) => (n as usize) <= CONTROL_GROUPS,
            _ => true,
        }),
        "no chord may point at a group dispatch ignores"
    );
}

#[test]
fn shift_never_flips_an_assign_into_a_recall() {
    // Shift lingers after queueing orders; Ctrl+Shift+digit must
    // still mean assign.
    let map = BindingMap::classic();
    assert_eq!(
        map.resolve_in(Key::Num4, true, true, Context::Units),
        Some(Action::AssignGroup(4))
    );
}

#[test]
fn releases_pair_with_what_the_press_meant() {
    let map = BindingMap::classic();
    let mut r = ActionResolver::default();
    assert_eq!(
        r.key_edge_in(&map, Key::Ctrl, true, Context::Units),
        None,
        "modifiers are truth, not actions"
    );
    assert_eq!(
        r.key_edge_in(&map, Key::Num2, true, Context::Units),
        Some(ActionEvent::Pressed(Action::AssignGroup(2)))
    );
    // Ctrl comes up before the digit: the release still closes the
    // assign, not a phantom recall.
    r.key_edge_in(&map, Key::Ctrl, false, Context::Units);
    assert_eq!(
        r.key_edge_in(&map, Key::Num2, false, Context::Units),
        Some(ActionEvent::Released(Action::AssignGroup(2)))
    );
}

#[test]
fn held_pans_read_back_until_released() {
    let map = BindingMap::classic();
    let mut r = ActionResolver::default();
    r.key_edge_in(&map, Key::Left, true, Context::Units);
    assert!(r.is_held(Action::PanLeft));
    r.key_edge_in(&map, Key::Left, false, Context::Units);
    assert!(!r.is_held(Action::PanLeft));
}

#[test]
fn clear_forgets_holds_and_modifiers() {
    let map = BindingMap::classic();
    let mut r = ActionResolver::default();
    r.key_edge_in(&map, Key::Ctrl, true, Context::Units);
    r.key_edge_in(&map, Key::Up, true, Context::Units);
    r.clear();
    assert!(!r.is_held(Action::PanUp));
    assert_eq!(
        r.key_edge_in(&map, Key::Num3, true, Context::Units),
        Some(ActionEvent::Pressed(Action::Slot(3))),
        "ctrl must not survive a clear"
    );
}
#[test]
fn contexts_allow_panel_reuse_but_reserve_camera_and_group_keys() {
    let mut map = BindingMap::classic();
    assert!(map.valid());
    assert!(BindingMap::left_handed().valid());
    assert_eq!(
        map.resolve_in(Key::R, false, false, Context::Units),
        Some(Action::Patrol)
    );
    assert_eq!(
        map.resolve_in(Key::R, false, false, Context::Production),
        Some(Action::TrainSlot(2))
    );
    assert_eq!(
        map.resolve_in(Key::R, false, false, Context::Construction),
        Some(Action::BuildCategory(2))
    );
    assert!(!map.rebind(Action::Build(BuildingKind::Turret), Chord::bare(Key::W)));
    assert!(!map.rebind(Action::TrainSlot(0), Chord::bare(Key::Num1)));
    assert!(map.rebind(Action::Build(BuildingKind::Turret), Chord::ctrl(Key::G)));
    assert_eq!(
        map.resolve_in(Key::G, true, true, Context::BuildCategory(2)),
        Some(Action::Build(BuildingKind::Turret))
    );
    assert_eq!(
        map.resolve_in(Key::G, true, true, Context::BuildCategory(0)),
        Some(Action::Run)
    );
}

#[test]
fn releasing_one_pan_alias_or_changing_context_does_not_release_the_other() {
    let map = BindingMap::classic();
    let mut resolver = ActionResolver::default();
    resolver.key_edge_in(&map, Key::W, true, Context::Units);
    resolver.key_edge_in(&map, Key::Up, true, Context::Units);
    resolver.key_edge_in(&map, Key::Shift, true, Context::Construction);
    resolver.key_edge_in(&map, Key::W, false, Context::Construction);
    assert!(resolver.is_held(Action::PanUp));
    resolver.key_edge_in(&map, Key::Up, false, Context::Construction);
    assert!(!resolver.is_held(Action::PanUp));
    resolver.key_edge_in(&map, Key::W, true, Context::Playback);
    resolver.clear();
    assert!(!resolver.is_held(Action::PanUp));
}

#[test]
fn malformed_secondary_rows_and_unknown_slots_are_rejected() {
    let mut map = BindingMap::classic();
    map.secondary.push(Binding {
        action: Action::Patrol,
        chord: Chord::bare(Key::W),
    });
    assert!(!map.valid());
    let mut map = BindingMap::classic();
    map.bindings.push(Binding {
        action: Action::BuildCategory(9),
        chord: Chord::bare(Key::G),
    });
    assert!(!map.valid());
}

#[test]
fn rebound_menu_keys_emit_only_their_contextual_navigation() {
    use oxide_protocol::RawEvent;
    let mut map = BindingMap::classic();
    assert!(map.rebind(Action::Confirm, Chord::bare(Key::Q)));
    let events = map.menu_events(
        &[
            RawEvent::KeyDown { key: Key::Q },
            RawEvent::KeyDown { key: Key::W },
            RawEvent::Text { ch: 'q' },
        ],
        false,
        false,
    );
    assert_eq!(
        events,
        vec![
            RawEvent::KeyDown { key: Key::Enter },
            RawEvent::Text { ch: 'q' }
        ]
    );
}
