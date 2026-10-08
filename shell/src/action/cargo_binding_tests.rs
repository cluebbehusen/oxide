use super::*;

#[test]
fn cargo_shortcuts_share_keys_only_in_distinct_selection_contexts() {
    for (mut map, key) in [
        (BindingMap::classic(), Key::U),
        (BindingMap::left_handed(), Key::E),
    ] {
        for (context, action) in [
            (Context::Workers, Action::ReturnCargo),
            (Context::Construction, Action::ReturnCargo),
            (Context::Units, Action::Unload),
            (Context::Buildings, Action::Upgrade),
            (Context::Production, Action::Upgrade),
        ] {
            assert_eq!(map.resolve_in(key, false, false, context), Some(action));
        }
        assert!(map.valid());
        assert!(map.conflicts().is_empty());
        assert!(!map.rebind(Action::Run, Chord::bare(key)));
        assert!(map.rebind(Action::ReturnCargo, Chord::ctrl(Key::O)));
        assert_eq!(map.chord_for(Action::Unload), Some(Chord::bare(key)));
    }
}

#[test]
fn shared_cargo_key_migrates_previous_presets_but_keeps_custom_bindings() {
    for (mut map, old_key, new_key) in [
        (BindingMap::classic(), Key::O, Key::U),
        (BindingMap::left_handed(), Key::Q, Key::E),
    ] {
        assert!(map.rebind(Action::ReturnCargo, Chord::bare(old_key)));
        map.revision = 2;
        let mut custom = map.clone();
        assert!(custom.rebind(Action::ReturnCargo, Chord::ctrl(Key::O)));
        custom.migrate(&[]);
        assert_eq!(
            custom.chord_for(Action::ReturnCargo),
            Some(Chord::ctrl(Key::O))
        );
        let mut unbound = map.clone();
        unbound.unbind(Action::ReturnCargo);
        unbound.migrate(&[Action::ReturnCargo]);
        assert_eq!(unbound.chord_for(Action::ReturnCargo), None);
        map.migrate(&[]);
        assert_eq!(
            map.chord_for(Action::ReturnCargo),
            Some(Chord::bare(new_key))
        );
        assert!(map.valid());
    }
}

#[test]
fn return_cargo_migration_supports_customized_profiles() {
    for (base, expected) in [
        (BindingMap::classic(), Key::U),
        (BindingMap::left_handed(), Key::E),
    ] {
        for unload in [None, Some(Chord::ctrl(Key::P))] {
            let mut map = base.clone();
            map.unbind(Action::ReturnCargo);
            map.revision = 1;
            assert!(map.rebind(Action::TogglePause, Chord::ctrl(Key::O)));
            if let Some(chord) = unload {
                assert!(map.rebind(Action::Unload, chord));
            }
            let pause = map.chord_for(Action::TogglePause);
            let mut unbound = map.clone();
            unbound.migrate(&[Action::ReturnCargo]);
            assert_eq!(unbound.chord_for(Action::ReturnCargo), None);
            map.migrate(&[]);
            let expected = unload.unwrap_or(Chord::bare(expected));
            assert_eq!(map.chord_for(Action::ReturnCargo), Some(expected));
            assert_eq!(map.chord_for(Action::Unload), Some(expected));
            assert_eq!(map.chord_for(Action::TogglePause), pause);
            assert!(map.valid());
        }
    }
}

#[test]
fn return_cargo_migration_falls_back_when_unload_conflicts_or_is_unbound() {
    for unload_unbound in [false, true] {
        let mut map = BindingMap::left_handed();
        map.unbind(Action::ReturnCargo);
        map.revision = 1;
        if unload_unbound {
            map.unbind(Action::Unload);
        } else {
            assert!(map.rebind(Action::Unload, Chord::bare(Key::U)));
        }
        let unload = map.chord_for(Action::Unload);
        let construction = map.chord_for(Action::BuildCategory(1));
        map.migrate(&[]);
        assert_eq!(
            map.chord_for(Action::ReturnCargo),
            Some(Chord::bare(Key::E))
        );
        assert_eq!(map.chord_for(Action::Unload), unload);
        assert_eq!(map.chord_for(Action::BuildCategory(1)), construction);
        assert!(map.valid());
    }
}

#[test]
fn return_cargo_migration_keeps_custom_keys_and_unbindings() {
    for mut map in [BindingMap::classic(), BindingMap::left_handed()] {
        let chord = map.chord_for(Action::ReturnCargo).unwrap();
        map.unbind(Action::ReturnCargo);
        map.revision = 1;
        let mut unbound = map.clone();
        unbound.migrate(&[Action::ReturnCargo]);
        assert_eq!(unbound.chord_for(Action::ReturnCargo), None);
        map.migrate(&[]);
        assert_eq!(map.chord_for(Action::ReturnCargo), Some(chord));
        assert!(map.valid());
    }
    let mut map = BindingMap::classic();
    map.unbind(Action::ReturnCargo);
    map.revision = 1;
    map.unbind(Action::Unload);
    map.unbind(Action::Upgrade);
    assert!(map.rebind(Action::Run, Chord::bare(Key::U)));
    map.migrate(&[]);
    assert_eq!(map.chord_for(Action::Run), Some(Chord::bare(Key::U)));
    assert_eq!(map.chord_for(Action::ReturnCargo), None);
}
