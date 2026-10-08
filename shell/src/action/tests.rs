use super::*;

#[test]
fn exact_chords_beat_bare_ones_and_bare_survives_modifiers() {
    let map = BindingMap::legacy();
    assert_eq!(
        map.resolve(Key::Num1, true, false),
        Some(Action::AssignGroup(1)),
        "ctrl+1 is its own meaning"
    );
    assert_eq!(map.resolve(Key::Num1, false, false), Some(Action::Slot(1)));
    assert_eq!(
        map.resolve(Key::H, true, false),
        Some(Action::TrainSlot(0)),
        "a held modifier never mutes an unmodified binding"
    );
}

#[test]
fn the_left_handed_preset_crosses_every_gameplay_verb_over() {
    // The preset's guarantee: verbs live on the right hand. Salvage
    // shipped after the preset and once stayed marooned on V.
    let map = BindingMap::legacy_left_handed();
    assert_eq!(
        map.chord_for(Action::Salvage),
        Some(Chord::bare(Key::J)),
        "salvage crossed over with the rest"
    );
    assert_eq!(
        map.chord_for(Action::StopOrScrap),
        Some(Chord::bare(Key::M))
    );
    assert_eq!(
        map.chord_for(Action::Hunt),
        Some(Chord::bare(Key::G)),
        "hunt crosses beside run"
    );
}

#[test]
fn rebinding_refuses_collisions_and_moves_the_chord() {
    let mut map = BindingMap::legacy();
    assert!(
        !map.rebind(Action::StopOrScrap, Chord::bare(Key::M)),
        "M belongs to Run in the unit context"
    );
    assert!(map.rebind(Action::StopOrScrap, Chord::ctrl(Key::X)));
    assert_eq!(map.resolve(Key::X, true, false), Some(Action::StopOrScrap));
    assert_eq!(
        map.resolve(Key::X, false, false),
        None,
        "the old chord is gone, not shadowed"
    );
}

#[test]
fn ctrl_digits_past_the_group_count_fall_through_to_slots() {
    let map = BindingMap::legacy();
    assert!(
        map.bindings().iter().all(|b| match b.action {
            Action::AssignGroup(n) => (n as usize) <= CONTROL_GROUPS,
            _ => true,
        }),
        "no chord may point at a group dispatch ignores"
    );
    assert_eq!(
        map.resolve(Key::Num7, true, false),
        Some(Action::Slot(7)),
        "ctrl+7 reaches the palette, not a phantom group"
    );
}

#[test]
fn shift_never_flips_an_assign_into_a_recall() {
    // Shift lingers after queueing orders; Ctrl+Shift+digit must
    // still mean assign, as the classic layout always had it.
    let map = BindingMap::legacy();
    assert_eq!(
        map.resolve(Key::Num4, true, true),
        Some(Action::AssignGroup(4))
    );
}

#[test]
fn releases_pair_with_what_the_press_meant() {
    let map = BindingMap::legacy();
    let mut r = ActionResolver::default();
    assert_eq!(
        r.key_edge(&map, Key::Ctrl, true),
        None,
        "modifiers are truth, not actions"
    );
    assert_eq!(
        r.key_edge(&map, Key::Num2, true),
        Some(ActionEvent::Pressed(Action::AssignGroup(2)))
    );
    // Ctrl comes up before the digit: the release still closes the
    // assign, not a phantom recall.
    r.key_edge(&map, Key::Ctrl, false);
    assert_eq!(
        r.key_edge(&map, Key::Num2, false),
        Some(ActionEvent::Released(Action::AssignGroup(2)))
    );
}

#[test]
fn held_pans_read_back_until_released() {
    let map = BindingMap::legacy();
    let mut r = ActionResolver::default();
    r.key_edge(&map, Key::Left, true);
    assert!(r.is_held(Action::PanLeft));
    r.key_edge(&map, Key::Left, false);
    assert!(!r.is_held(Action::PanLeft));
}

#[test]
fn the_classic_profile_has_no_conflicts_and_covers_the_old_map() {
    let map = BindingMap::legacy();
    assert!(map.conflicts().is_empty());
    // Every key the old hardcoded switchboard answered resolves to
    // something; a silent hole would be a lost shortcut.
    for key in [
        Key::Left,
        Key::Right,
        Key::Up,
        Key::Down,
        Key::X,
        Key::H,
        Key::S,
        Key::P,
        Key::B,
        Key::R,
        Key::F,
        Key::F1,
        Key::Escape,
        Key::Space,
        Key::Enter,
        Key::Num1,
        Key::Num9,
    ] {
        assert!(
            map.resolve(key, false, false).is_some(),
            "{key:?} lost its meaning"
        );
    }
}

#[test]
fn clear_forgets_holds_and_modifiers() {
    let map = BindingMap::legacy();
    let mut r = ActionResolver::default();
    r.key_edge(&map, Key::Ctrl, true);
    r.key_edge(&map, Key::Up, true);
    r.clear();
    assert!(!r.is_held(Action::PanUp));
    assert_eq!(
        r.key_edge(&map, Key::Num3, true),
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
fn old_defaults_migrate_without_stealing_custom_chords_or_reviving_unbindings() {
    let mut map = BindingMap::legacy();
    map.migrate(&[]);
    assert_eq!(map, BindingMap::classic());
    let mut map = BindingMap::legacy();
    assert!(map.rebind(Action::Patrol, Chord::bare(Key::D)));
    map.unbind(Action::Salvage);
    map.migrate(&[Action::Salvage]);
    assert_eq!(map.chord_for(Action::Patrol), Some(Chord::bare(Key::D)));
    assert_eq!(map.chord_at(Action::PanRight, 0), None);
    assert_eq!(
        map.chord_at(Action::PanRight, 1),
        Some(Chord::bare(Key::Right))
    );
    assert_eq!(map.chord_for(Action::Salvage), None);
    assert!(map.valid());
    let saved = serde_json::to_string(&map).unwrap();
    let mut loaded: BindingMap = serde_json::from_str(&saved).unwrap();
    loaded.migrate(&[]);
    assert_eq!(loaded, map);
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
