use super::*;

#[test]
fn marker_preferences_round_trip_and_clamp_without_resetting_other_settings() {
    let dir = std::env::temp_dir().join(format!("oxide-config-markers-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config {
        ui_scale: 1.25,
        ..Config::default()
    };
    config.save_to(&path).unwrap();
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
    let dir = std::env::temp_dir().join(format!("oxide-config-unbind-{}", std::process::id()));
    let path = dir.join("config.json");
    let mut config = Config::default();
    config
        .bindings
        .unbind_slot(crate::action::Action::Patrol, 0);
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
fn performance_modes_and_the_last_join_address_persist() {
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
    assert_eq!(Config::load_from(Some(path)), config);
    std::fs::remove_dir_all(dir).unwrap();
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

#[test]
fn the_driver_quiet_config_has_the_current_shape() {
    // Driver captures install this file as the shell's settings, and a
    // stale shape would silently fall back to defaults.
    let config: Config =
        serde_json::from_str(include_str!("../../../driver/src/quiet_config.json")).unwrap();
    assert_eq!(config.version, CONFIG_VERSION);
    assert_eq!(config.volumes.master, 0.0);
}
