use super::*;

#[test]
fn the_touch_build_argv_parses_to_ordinary_defaults() {
    let args = Args::parse_from(["Oxide"]);
    assert!(!args.debug_server && !args.automation && !args.paused);
    assert!(args.scenario.is_none() && args.replay.is_none() && args.watch.is_none());
    assert_eq!(args.speed, 1.0);
}

#[test]
fn automation_requires_debug_server() {
    assert!(Args::try_parse_from(["oxide-shell", "--automation"]).is_err());
    let args = Args::try_parse_from(["oxide-shell", "--debug-server", "--automation"])
        .expect("automation with the debug server should parse");
    assert!(args.debug_server);
    assert!(args.automation);
}

#[test]
fn trace_env_never_reaches_an_automation_shell() {
    // Harness-spawned shells inherit the developer's environment;
    // only the explicit flag may trace under --automation.
    assert!(!trace_active(false, true, true));
    assert!(trace_active(true, true, false));
    assert!(trace_active(false, false, true));
    assert!(!trace_active(false, false, false));
}

#[test]
fn public_numeric_options_enforce_their_documented_envelopes() {
    assert_eq!(parse_window("640x400"), Ok((640, 400)));
    assert_eq!(parse_window("16384x16384"), Ok((16_384, 16_384)));
    for invalid in ["800", "639x400", "640x399", "16385x400", "640x16385"] {
        assert!(parse_window(invalid).is_err(), "accepted {invalid}");
    }

    for valid in ["0.05", "1", "64"] {
        assert!(parse_speed(valid).is_ok(), "refused {valid}");
    }
    for invalid in ["0", "0.049", "64.1", "NaN", "inf"] {
        assert!(parse_speed(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn modes_that_depend_on_the_debug_server_are_rejected_without_it() {
    for option in ["--automation", "--profile-frames", "--debug-idle-timeout"] {
        let mut args = vec!["oxide-shell", option];
        if option == "--debug-idle-timeout" {
            args.push("60");
        }
        assert!(Args::try_parse_from(args).is_err(), "accepted {option}");
    }

    assert!(
        Args::try_parse_from([
            "oxide-shell",
            "--watch",
            "match.json",
            "--scenario",
            "map.json"
        ])
        .is_err()
    );
    assert!(
        Args::try_parse_from([
            "oxide-shell",
            "--watch",
            "match.json",
            "--replay",
            "save.json"
        ])
        .is_err()
    );
}

#[test]
fn hosting_needs_a_scenario_and_joining_takes_none() {
    let parse = |args: &[&str]| Args::try_parse_from([&["oxide-shell"], args].concat());
    assert!(parse(&["--host", "0.0.0.0:4200"]).is_err());
    let host = parse(&["--host", "0.0.0.0:4200", "--scenario", "map.json"]).unwrap();
    assert_eq!(host.host.as_deref(), Some("0.0.0.0:4200"));
    let join = parse(&["--join", "10.0.0.2:4200"]).unwrap();
    assert_eq!(join.join.as_deref(), Some("10.0.0.2:4200"));
    for invalid in [
        &["--join", "10.0.0.2:4200", "--scenario", "map.json"][..],
        &["--join", "a:1", "--host", "b:1", "--scenario", "map.json"],
        &[
            "--host",
            "b:1",
            "--scenario",
            "map.json",
            "--replay",
            "save.json",
        ],
        &["--join", "a:1", "--watch", "match.json"],
        &["--join", "a:1", "--paused"],
        &["--join", "a:1", "--speed", "2"],
        &["--host", "b:1", "--scenario", "map.json", "--paused"],
        &["--host", "b:1", "--scenario", "map.json", "--speed", "1"],
    ] {
        assert!(parse(invalid).is_err(), "accepted {invalid:?}");
    }
}
