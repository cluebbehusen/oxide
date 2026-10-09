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
