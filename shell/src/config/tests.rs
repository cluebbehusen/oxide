use super::*;

#[test]
fn marker_preferences_migrate_round_trip_and_clamp_without_resetting_other_settings() {
    let dir = std::env::temp_dir().join(format!("oxide-config-markers-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config {
        ui_scale: 1.25,
        ..Config::default()
    };
    config.save_to(&path).unwrap();
    let mut legacy = serde_json::to_value(&config).unwrap();
    legacy.as_object_mut().unwrap().remove("markers");
    std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(Config::load_from(Some(path.clone())), config);

    config.markers = MarkerPrefs {
        start: 30.0,
        end: 22.0,
        scale: 1.5,
    };
    config.save_to(&path).unwrap();
    assert_eq!(Config::load_from(Some(path.clone())), config);

    config.markers = MarkerPrefs {
        start: -100.0,
        end: 300.0,
        scale: -1.0,
    };
    config.save_to(&path).unwrap();
    let loaded = Config::load_from(Some(path));
    assert_eq!(
        loaded.markers,
        MarkerPrefs {
            start: 31.0,
            end: 30.0,
            scale: 0.75
        }
    );
    assert_eq!(loaded.ui_scale, 1.25);
    assert_eq!(
        MarkerPrefs {
            start: f32::NAN,
            end: f32::INFINITY,
            scale: f32::NEG_INFINITY
        }
        .clamped(),
        MarkerPrefs::default()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_customized_map_survives_the_round_trip() {
    // A rebind must survive a restart, so the rest of the classic map
    // (including its one-based Slot(9)) must pass load validation.
    let dir = std::env::temp_dir().join(format!("oxide-config-custom-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config::default();
    assert!(
        config.bindings.rebind(
            crate::action::Action::Patrol,
            crate::action::Chord::bare(oxide_protocol::Key::J)
        ),
        "J is free in the classic map"
    );
    config.save_to(&path).expect("save");
    let loaded = Config::load_from(Some(path));
    assert_eq!(
        loaded.bindings.chord_for(crate::action::Action::Patrol),
        Some(crate::action::Chord::bare(oxide_protocol::Key::J)),
        "the customization survived validation"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_explicit_unbinding_survives_the_restart() {
    // Controls > X removes the row and records the choice; without the
    // tombstone the new-verb migration would read the missing row as an
    // old config and restore the classic chord on every load.
    let dir = std::env::temp_dir().join(format!("oxide-config-unbind-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config::default();
    config.bindings.unbind(crate::action::Action::Patrol);
    config.unbound.push(crate::action::Action::Patrol);
    config.save_to(&path).expect("save");
    let loaded = Config::load_from(Some(path));
    assert_eq!(
        loaded.bindings.chord_for(crate::action::Action::Patrol),
        None,
        "the deliberate unbinding persisted"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_config_saved_before_a_new_verb_adopts_its_classic_chord() {
    // A config saved before an action existed has no row for it.
    // Loading must graft the classic chord in when free instead of
    // leaving the verb keyboardless, without stealing a claimed chord.
    let dir = std::env::temp_dir().join(format!("oxide-config-newverb-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config {
        bindings: BindingMap::legacy(),
        ..Config::default()
    };
    config.bindings.unbind(crate::action::Action::Salvage);
    config.save_to(&path).expect("save");
    let loaded = Config::load_from(Some(path.clone()));
    assert_eq!(
        loaded.bindings.chord_for(crate::action::Action::Salvage),
        Some(crate::action::Chord::bare(oxide_protocol::Key::V)),
        "the missing verb adopted its classic chord"
    );

    // Same again, but the player owns V: the verb stays unbound.
    let mut config = Config {
        bindings: BindingMap::legacy(),
        ..Config::default()
    };
    config.bindings.unbind(crate::action::Action::Salvage);
    assert!(config.bindings.rebind(
        crate::action::Action::Patrol,
        crate::action::Chord::bare(oxide_protocol::Key::V)
    ));
    config.save_to(&path).expect("save");
    let loaded = Config::load_from(Some(path));
    assert_eq!(
        loaded.bindings.chord_for(crate::action::Action::Salvage),
        None,
        "a claimed chord is never stolen back"
    );
    assert_eq!(
        loaded.bindings.chord_for(crate::action::Action::Patrol),
        Some(crate::action::Chord::bare(oxide_protocol::Key::V)),
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn stale_group_chords_drop_without_resetting_the_profile() {
    // A chord to a group beyond the supported count is a row dispatch
    // ignores; loading must shed that row alone, never the user's own
    // customizations with it.
    let dir = std::env::temp_dir().join(format!("oxide-config-stale-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config {
        bindings: BindingMap::legacy(),
        ..Config::default()
    };
    assert!(config.bindings.rebind(
        crate::action::Action::AssignGroup(7),
        crate::action::Chord::ctrl(oxide_protocol::Key::Num7)
    ));
    assert!(config.bindings.rebind(
        crate::action::Action::Patrol,
        crate::action::Chord::bare(oxide_protocol::Key::J)
    ));
    config.save_to(&path).expect("save");
    let loaded = Config::load_from(Some(path));
    assert_eq!(
        loaded
            .bindings
            .chord_for(crate::action::Action::AssignGroup(7)),
        None,
        "the stale chord is gone"
    );
    assert_eq!(
        loaded.bindings.chord_for(crate::action::Action::Patrol),
        Some(crate::action::Chord::bare(oxide_protocol::Key::J)),
        "the customization survived the strip"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn saving_twice_replaces_instead_of_failing() {
    // std's rename replaces existing destinations on every
    // platform; the second save must land and its content win.
    let dir = std::env::temp_dir().join(format!("oxide-config-twice-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config::default();
    config.save_to(&path).expect("first save");
    config.ui_scale = 1.5;
    config.save_to(&path).expect("second save replaces");
    let loaded = Config::load_from(Some(path));
    assert!((loaded.ui_scale - 1.5).abs() < 1e-6, "the newer config won");
    std::fs::remove_dir_all(&dir).ok();
}

#[cfg(unix)]
#[test]
fn a_failed_save_leaves_the_old_config_standing() {
    // A read-only directory refuses the temp file; the previous
    // config must survive untouched and no temp may linger.
    use std::os::unix::fs::PermissionsExt as _;
    let dir = std::env::temp_dir().join(format!("oxide-config-fail-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    let path = dir.join("config.json");
    let config = Config::default();
    config.save_to(&path).expect("first save");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let changed = Config {
        ui_scale: 1.5,
        ..Config::default()
    };
    assert!(changed.save_to(&path).is_err(), "the failure surfaces");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let loaded = Config::load_from(Some(path));
    assert_eq!(loaded, config, "the old config survived the failed save");
    let temps: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(temps.is_empty(), "no temp survives a failed save");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_config_round_trips_through_disk() {
    let dir = std::env::temp_dir().join(format!("oxide-config-test-{}", std::process::id()));
    let path = dir.join("config.json");
    let config = Config {
        ui_scale: 1.25,
        volumes: Volumes {
            master: 0.5,
            ..Volumes::default()
        },
        ..Config::default()
    };
    config.save_to(&path).unwrap();
    let back = Config::load_from(Some(path));
    assert_eq!(back, config);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn performance_modes_persist_and_old_configs_keep_preferences() {
    let dir = std::env::temp_dir().join(format!("oxide-config-performance-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config {
        ui_scale: 1.25,
        ..Config::default()
    };
    config.volumes.master = 0.25;
    for mode in [
        PerformanceDisplay::Off,
        PerformanceDisplay::Fps,
        PerformanceDisplay::Detailed,
    ] {
        config.performance_display = mode;
        config.save_to(&path).unwrap();
        assert_eq!(Config::load_from(Some(path.clone())), config);
    }
    config.last_join_address = Some("connor-mbp:4200".to_owned());
    config.save_to(&path).unwrap();
    assert_eq!(Config::load_from(Some(path.clone())), config);
    let mut old = serde_json::to_value(&config).unwrap();
    old.as_object_mut().unwrap().remove("performance_display");
    old.as_object_mut().unwrap().remove("last_join_address");
    std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
    config.performance_display = PerformanceDisplay::Off;
    config.last_join_address = None;
    assert_eq!(Config::load_from(Some(path)), config);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_config_from_before_the_music_bus_keeps_every_other_setting() {
    let dir = std::env::temp_dir().join(format!("oxide-config-pre-music-{}", std::process::id()));
    let path = dir.join("config.json");
    std::fs::create_dir_all(&dir).unwrap();
    let mut old = serde_json::to_value(Config {
        ui_scale: 1.25,
        volumes: Volumes {
            effects: 0.5,
            ..Volumes::default()
        },
        ..Config::default()
    })
    .unwrap();
    old["volumes"].as_object_mut().unwrap().remove("music");
    std::fs::write(&path, serde_json::to_vec_pretty(&old).unwrap()).unwrap();

    let loaded = Config::load_from(Some(path));
    assert_eq!(loaded.volumes.music, 1.0);
    assert_eq!(loaded.volumes.effects, 0.5);
    assert_eq!(loaded.ui_scale, 1.25);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_touch_preference_the_config_predates_keeps_every_other_setting() {
    let dir = std::env::temp_dir().join(format!("oxide-config-pre-touch-{}", std::process::id()));
    let path = dir.join("config.json");
    std::fs::create_dir_all(&dir).unwrap();
    let mut old = serde_json::to_value(Config {
        ui_scale: 1.25,
        touch: TouchPrefs {
            double_tap_ms: 280,
            ..TouchPrefs::default()
        },
        ..Config::default()
    })
    .unwrap();
    old["touch"]
        .as_object_mut()
        .unwrap()
        .remove("long_press_ms");
    std::fs::write(&path, serde_json::to_vec_pretty(&old).unwrap()).unwrap();

    let loaded = Config::load_from(Some(path));
    assert_eq!(loaded.ui_scale, 1.25, "the rest of the config survives");
    assert_eq!(loaded.touch.double_tap_ms, 280);
    assert_eq!(
        loaded.touch.long_press_ms,
        TouchPrefs::default().long_press_ms
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_config_saved_before_the_group_column_shows_it() {
    let mut old = serde_json::to_value(Config::default()).unwrap();
    old.as_object_mut().unwrap().remove("control_groups");
    let loaded: Config = serde_json::from_value(old).unwrap();
    assert!(loaded.control_groups);
}

#[test]
fn any_trouble_at_all_falls_back_to_defaults() {
    // Missing file.
    let missing = Config::load_from(Some(PathBuf::from("/definitely/not/here.json")));
    assert_eq!(missing, Config::default());
    // Garbage content.
    let dir = std::env::temp_dir().join(format!("oxide-config-garbage-{}", std::process::id()));
    let path = dir.join("config.json");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, "not json at all").unwrap();
    assert_eq!(Config::load_from(Some(path.clone())), Config::default());
    // A future version resets rather than guessing.
    let future = Config {
        version: CONFIG_VERSION + 1,
        ..Config::default()
    };
    std::fs::write(&path, serde_json::to_string(&future).unwrap()).unwrap();
    assert_eq!(Config::load_from(Some(path)), Config::default());
    std::fs::remove_dir_all(&dir).ok();
}
#[test]
fn an_empty_secondary_slot_and_rebound_primary_survive_reload() {
    use crate::action::{Action, Chord};
    let dir = std::env::temp_dir().join(format!("oxide-config-secondary-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config::default();
    config.bindings.unbind_slot(Action::PanUp, 1);
    assert!(
        config
            .bindings
            .rebind(Action::PanUp, Chord::bare(oxide_protocol::Key::I))
    );
    config.save_to(&path).unwrap();
    let loaded = Config::load_from(Some(path));
    assert_eq!(loaded.bindings, config.bindings);
    std::fs::remove_dir_all(dir).unwrap();
}
