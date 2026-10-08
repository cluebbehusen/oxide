use super::*;
use std::ffi::OsStr;

fn menu(items: &[&str], selected: Option<usize>) -> UiView {
    UiView {
        mode: "home".to_string(),
        title: None,
        selected,
        items: items.iter().map(|item| (*item).to_string()).collect(),
        visible_range: None,
        hover: None,
        chrome: None,
        panel_regions: None,
        menu_button: None,
        pause_status: None,
        group_column: None,
    }
}

#[test]
fn cargo_artifact_selection_ignores_noise_and_other_targets() {
    let output = br#"
not json
{"reason":"compiler-artifact","target":{"name":"oxide-sim"},"executable":"/tmp/wrong"}
{"reason":"compiler-artifact","target":{"name":"Oxide"},"executable":null}
{"reason":"build-finished","success":true}
{"reason":"compiler-artifact","target":{"name":"Oxide"},"executable":"/tmp/Oxide"}
"#;
    assert_eq!(
        shell_executable_from_cargo_output(output),
        Some(PathBuf::from("/tmp/Oxide"))
    );
    assert_eq!(shell_executable_from_cargo_output(b"not json\n"), None);
}

#[test]
fn isolated_shells_override_every_platform_writable_root() {
    let home = PathBuf::from("/tmp/oxide-isolated-home");
    let mut command = std::process::Command::new("Oxide");
    command.env("HOME", "/host/home");
    command.env("XDG_CONFIG_HOME", "/host/config");
    command.env("XDG_DATA_HOME", "/host/data");
    command.env("APPDATA", "C:/host/data");

    isolate_home(&mut command, &home);

    let value = |key: &str| {
        command
            .get_envs()
            .find(|(name, _)| *name == OsStr::new(key))
            .and_then(|(_, value)| value)
            .map(std::ffi::OsStr::to_owned)
    };
    assert_eq!(value("HOME"), Some(home.clone().into_os_string()));
    assert_eq!(
        value("XDG_CONFIG_HOME"),
        Some(home.join(".config").into_os_string())
    );
    assert_eq!(
        value("XDG_DATA_HOME"),
        Some(home.join(".local/share").into_os_string())
    );
    assert_eq!(
        value("APPDATA"),
        Some(home.join("AppData").into_os_string())
    );
}

#[test]
fn labeled_activation_prefers_an_exact_row_over_an_earlier_substring() {
    let view = menu(&["REPLAYS", "PLAY", "SETTINGS"], Some(0));
    assert_eq!(
        labeled_activation_keys(&view, "play").unwrap(),
        vec![Key::Down, Key::Enter]
    );
}

#[test]
fn labeled_activation_navigates_in_both_directions_and_defaults_to_the_first_row() {
    let view = menu(&["PLAY", "SETTINGS", "QUIT"], Some(2));
    assert_eq!(
        labeled_activation_keys(&view, "play").unwrap(),
        vec![Key::Up, Key::Up, Key::Enter]
    );

    let no_selection = menu(&["PLAY", "SETTINGS", "QUIT"], None);
    assert_eq!(
        labeled_activation_keys(&no_selection, "settings").unwrap(),
        vec![Key::Down, Key::Enter]
    );
}

#[test]
fn labeled_activation_reports_the_visible_rows_when_no_row_matches() {
    let view = menu(&["PLAY", "SETTINGS"], Some(0));
    let error = labeled_activation_keys(&view, "credits").unwrap_err();
    assert_eq!(
        error.to_string(),
        "no row containing 'credits' in [\"PLAY\", \"SETTINGS\"]"
    );
}

#[test]
fn mode_waits_ride_out_only_named_transitional_screens() {
    assert_eq!(
        classify_mode("home", "home", &["saving"]),
        ModeWait::Arrived
    );
    assert_eq!(
        classify_mode("saving", "home", &["saving"]),
        ModeWait::Transitional
    );
    assert_eq!(
        classify_mode("pause_menu", "home", &["saving"]),
        ModeWait::Unexpected
    );
    assert_eq!(classify_mode("saving", "home", &[]), ModeWait::Unexpected);
}
