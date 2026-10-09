use super::*;
use crate::game::GameReplay;

#[test]
fn shelf_hints_offer_only_the_gestures_the_build_has() {
    assert_eq!(
        entry_hint(Some(("watches", "watch")), false),
        "{confirm} watches | {delete} twice deletes"
    );
    assert_eq!(entry_hint(None, false), "{delete} twice deletes");
    for action in [("watches", "watch"), ("loads paused", "load paused")] {
        crate::platform::assert_touch_copy(&entry_hint(Some(action), true));
    }
    assert_eq!(entry_hint(None, true), "");
}

#[test]
fn the_shelf_badge_compares_versions_and_never_guesses() {
    let dir = std::env::temp_dir().join(format!(
        "oxide-shelf-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let scenario = oxide_sim::Scenario::skirmish();
    let ours: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario.clone());
    ours.save(dir.join("ours.json")).unwrap();
    let mut foreign: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario);
    foreign.meta.sim_version = SIM_VERSION + 1;
    foreign.save(dir.join("foreign.json")).unwrap();

    let mut out = Vec::new();
    scan(&dir, &mut out);
    let entry = |name: &str| {
        out.iter()
            .map(|(_, e)| e)
            .find(|e| e.path.file_stem().unwrap() == name)
            .expect("scanned")
    };
    assert!(entry("ours").compatible, "our own version wears the badge");
    let foreign = entry("foreign");
    assert!(!foreign.compatible, "a foreign version never does");
    assert!(
        foreign.blurb.contains(&format!(
            "recorded on sim {}, this is {SIM_VERSION}",
            SIM_VERSION + 1
        )),
        "the honest badge names both versions: {}",
        foreign.blurb
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn kinds_read_the_metadata_tag() {
    let dir = std::env::temp_dir().join(format!(
        "oxide-kinds-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let scenario = oxide_sim::Scenario::skirmish();
    // A tagged save under a neutral filename: the tag wins.
    let mut named: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario.clone());
    named.meta.kind = Some("save".to_string());
    named.meta.description = Some("before the push".to_string());
    named.save(dir.join("anything.json")).unwrap();
    let finished: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario);
    finished.save(dir.join("match-0000000099.json")).unwrap();

    let mut out = Vec::new();
    scan(&dir, &mut out);
    let entry = |name: &str| {
        out.iter()
            .map(|(_, e)| e)
            .find(|e| e.path.file_stem().unwrap() == name)
            .expect("scanned")
    };
    assert_eq!(entry("anything").kind, RecordKind::Save);
    assert!(
        entry("anything").label.starts_with("before the push"),
        "a named save leads with its name: {}",
        entry("anything").label
    );
    assert_eq!(entry("match-0000000099").kind, RecordKind::Match);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn shelf_skips_oversized_records_without_hiding_valid_neighbors() {
    let dir = std::env::temp_dir().join(format!(
        "oxide-shelf-bounds-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let valid: GameReplay =
        chassis::replay::Replay::new(SIM_VERSION, oxide_sim::Scenario::skirmish());
    valid.save(dir.join("valid.json")).unwrap();
    std::fs::File::create(dir.join("oversized.json"))
        .unwrap()
        .set_len(chassis::replay::MAX_REPLAY_BYTES as u64 + 1)
        .unwrap();

    let mut out = Vec::new();
    scan(&dir, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].1.path.file_stem().unwrap(), "valid");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn catalog_orders_autosaves_by_saved_time_then_mtime_then_path() {
    let dir = std::env::temp_dir().join(format!("oxide-save-order-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let game = crate::game::Game::new(oxide_sim::Scenario::skirmish()).unwrap();
    for (name, saved_at, modified) in [
        ("copied", 99, 900),
        ("older", 100, 100),
        ("a", 100, 101),
        ("b", 100, 101),
        ("newest", 101, 1),
    ] {
        let mut meta = game.recorder.meta.clone();
        meta.kind = Some("autosave".into());
        meta.ticks = Some(game.state.current_tick());
        meta.saved_at = Some(saved_at);
        let path = dir.join(format!("{name}.oxsave"));
        crate::saved_game::write_capture(game.capture_save(), meta, &path).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(modified))
            .unwrap();
    }
    for reverse in [false, true] {
        let mut found = Vec::new();
        scan(&dir, &mut found);
        if reverse {
            found.reverse();
        }
        let candidates = newest_first(found)
            .into_iter()
            .filter(|entry| entry.compatible && entry.kind == RecordKind::Autosave)
            .map(|entry| {
                entry
                    .path
                    .file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(candidates, ["newest", "b", "a", "older", "copied"]);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_calendar_is_honest_without_a_time_crate() {
    assert_eq!(civil_date(0), "1970-01-01");
    assert_eq!(civil_date(86_399), "1970-01-01", "last second of day one");
    assert_eq!(civil_date(86_400), "1970-01-02");
    // Leap handling around the century rule: 2000-02-29 existed.
    // 951_782_400 = 2000-02-29T00:00:00Z; 951_868_800 = 2000-03-01.
    assert_eq!(civil_date(951_782_400), "2000-02-29");
    assert_eq!(civil_date(951_868_800), "2000-03-01");
    // A modern spot check: 2026-07-22T12:00:00Z.
    assert_eq!(civil_date(1_784_721_600), "2026-07-22");
}

#[test]
fn long_stems_elide_at_char_boundaries() {
    assert_eq!(elide("short"), "short");
    let long_ascii = "a".repeat(30);
    assert_eq!(elide(&long_ascii), format!("{}...", "a".repeat(23)));
    // 27 chars, with byte offset 23 landing inside the first é, where
    // byte slicing would panic.
    let multibyte = format!("{}ééééé", "a".repeat(22));
    assert_eq!(elide(&multibyte), format!("{}é...", "a".repeat(22)));
}

#[test]
fn scan_uses_record_metadata_without_trusting_malformed_neighbors() {
    let dir = std::env::temp_dir().join(format!(
        "oxide-shelf-metadata-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp directory");
    let scenario = oxide_sim::Scenario::skirmish();
    let mut save: GameReplay = chassis::replay::Replay::new(SIM_VERSION, scenario);
    save.meta.kind = Some("save".to_string());
    save.meta.description = Some("before the push".to_string());
    save.meta.sim_version = SIM_VERSION + 1;
    save.meta.saved_at = Some(u64::MAX);
    save.record(
        42,
        oxide_sim::PlayerCommand {
            player: oxide_sim::PlayerId(0),
            command: oxide_sim::Command::Surrender,
        },
    );
    save.save(dir.join("neutral.json")).expect("save record");
    std::fs::write(dir.join("broken.json"), b"not json").expect("bad neighbor");
    std::fs::write(dir.join("ignored.txt"), b"not a replay").expect("other extension");

    let mut out = Vec::new();
    scan(&dir, &mut out);
    assert_eq!(out.len(), 1, "bad neighbors do not hide the good record");
    let entry = &out[0].1;
    assert_eq!(entry.kind, RecordKind::Save, "the metadata tag wins");
    assert!(!entry.compatible);
    assert!(entry.label.starts_with("before the push |"));
    assert!(
        entry.label.contains("| t43 |"),
        "duration comes from the command tail"
    );
    assert!(
        entry.blurb.contains("unavailable"),
        "saves are loaded, not watched"
    );
    assert_ne!(
        out[0].0.saved_at,
        std::time::UNIX_EPOCH,
        "overflowing saved_at falls back to mtime"
    );

    std::fs::remove_dir_all(dir).ok();
}
