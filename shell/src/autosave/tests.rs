use super::*;

fn write_record(game: &mut Game, dir: &Path) -> Result<SaveOutcome, SaveError> {
    let mut job = SaveJob::capture(game, None);
    job.dir = Some(dir.to_owned());
    let result = job.run()?;
    game.autosave_done = true;
    Ok(result)
}
fn write_named(game: &Game, name: &str, dir: &Path, saved_at: u64) -> Result<PathBuf, SaveError> {
    let mut job = SaveJob::capture(game, Some(name));
    job.dir = Some(dir.to_owned());
    job.meta.saved_at = Some(saved_at);
    match job.run()? {
        SaveOutcome::Wrote(path) => Ok(path),
        _ => unreachable!(),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oxide-autosave-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::remove_dir_all(&dir).ok();
    dir
}

#[test]
fn pending_tick_zero_input_is_saved_and_finished_matches_remain_recordings() {
    let dir = scratch("pending-and-finished");
    let mut game = Game::new(oxide_sim::Scenario::skirmish()).unwrap();
    game.issue(oxide_sim::Command::Train {
        building: oxide_sim::BuildingId(0),
        kind: oxide_sim::UnitKind::Harvester,
    });
    let Ok(SaveOutcome::Wrote(path)) = write_record(&mut game, &dir) else {
        panic!("pending input must not be discarded")
    };
    let mut restored = crate::saved_game::load(&path).unwrap();
    assert_eq!(*restored.pending, *game.pending);
    assert_eq!(restored.state.current_tick(), 0);
    assert_eq!(restored.do_tick().events, game.do_tick().events);
    restored.issue(oxide_sim::Command::Surrender);
    restored.do_tick();
    let Ok(SaveOutcome::Wrote(finished)) = write_record(&mut restored, &dir) else {
        panic!("finished record writes")
    };
    let record = oxide_kit::load_replay(&finished).unwrap();
    assert_eq!(record.meta.kind.as_deref(), Some("match"));
    let mut playback = oxide_kit::playback::Playback::load(record).unwrap();
    playback.seek(restored.state.current_tick());
    assert_eq!(playback.state.hash(), restored.state.hash());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_lan_match_leaves_only_a_watch_only_recording() {
    let dir = scratch("lan-match");
    let mut duel = oxide_sim::Scenario::skirmish();
    for player in &mut duel.players {
        player.bot = false;
        player.bot_config = None;
    }
    let mut game = Game::networked(
        duel,
        oxide_sim::PlayerId(0),
        crate::game::network::NetRole::Host,
        macroquad::prelude::vec2(1280.0, 800.0),
    )
    .unwrap();
    game.issue(oxide_sim::Command::Train {
        building: oxide_sim::BuildingId(0),
        kind: oxide_sim::UnitKind::Harvester,
    });
    let Ok(SaveOutcome::Wrote(path)) = write_record(&mut game, &dir) else {
        panic!("an undecided LAN match still records its history")
    };
    let record = oxide_kit::load_replay(&path).unwrap();
    assert_eq!(record.meta.kind.as_deref(), Some("match"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_tick_zero_clean_exit_finishes_existing_recovery_without_writing_a_save() {
    use oxide_kit::recovery::{RecoveryWriter, inspect, latest_diagnostic_record};
    let root = scratch("zero-recovery");
    let baseline = GameReplay::new(oxide_sim::SIM_VERSION, oxide_sim::Scenario::skirmish());
    let old = RecoveryWriter::start(root.clone(), baseline, 0, crate::build_identity()).unwrap();
    old.prepared(0, &[]);
    old.completed(1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while old.status().durable_tick != 1 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let old_path = old.directory().to_owned();
    drop(old);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::fs::File::open(old_path.join("lease"))
        .unwrap()
        .try_lock()
        .is_err()
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for public_path in [true, false] {
        // Hold startup past the save path's bounded wait for recovery.
        let budget = std::fs::File::options()
            .read(true)
            .write(true)
            .open(root.join("budget.lock"))
            .unwrap();
        budget.lock().unwrap();
        let mut game = Game::new(oxide_sim::Scenario::skirmish()).unwrap();
        game.recovery_root = Some(root.clone());
        game.start_recovery();
        let directory = game.recovery.as_ref().unwrap().directory().to_owned();
        let outcome = if public_path {
            SaveJob::capture(&game, None).run()
        } else {
            write_record(&mut game, &root.join("saves"))
        };
        assert!(matches!(outcome, Ok(SaveOutcome::NothingToSave)));
        let writer = game.recovery.as_ref().unwrap();
        assert!(!writer.status().ready);
        drop(budget);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let status = writer.status();
            assert!(status.error.is_none(), "{status:?}");
            if status.clean {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{status:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(inspect(&directory).unwrap().clean);
        drop(game);
        assert_eq!(latest_diagnostic_record(&root).unwrap().directory, old_path);
    }
    assert!(!root.join("saves").exists());
    for entry in std::fs::read_dir(&root).unwrap().flatten() {
        let lease = entry.path().join("lease");
        if lease.exists() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::fs::File::open(&lease).unwrap().try_lock().is_err() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn outcomes_distinguish_nothing_already_and_wrote() {
    let dir = scratch("outcomes");
    let mut game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
    assert!(
        matches!(
            write_record(&mut game, &dir),
            Ok(SaveOutcome::NothingToSave)
        ),
        "an unstarted game writes nothing"
    );
    game.advance_ticks(1);
    let Ok(SaveOutcome::Wrote(path)) = write_record(&mut game, &dir) else {
        panic!("a started game writes its record");
    };
    assert!(path.exists());
    assert!(
        matches!(write_record(&mut game, &dir), Ok(SaveOutcome::AlreadySaved)),
        "one record per session"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn records_carry_their_kind_and_a_named_save_leaves_the_session_recorder_alone() {
    let dir = scratch("kind-stamp");
    let mut game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
    game.advance_ticks(1);
    let saved =
        write_named(&game, "before the big push", &dir, 1_784_721_600).expect("a named save lands");
    let record = crate::saved_game::inspect(&saved).expect("loads back");
    assert_eq!(record.meta.kind.as_deref(), Some("save"));
    assert_eq!(
        record.meta.description.as_deref(),
        Some("before the big push")
    );
    assert_eq!(record.meta.saved_at, Some(1_784_721_600));
    assert!(
        game.recorder.meta.description.is_none(),
        "the live recorder never inherits the name"
    );
    assert!(
        !game.autosave_done,
        "an explicit save is not the session's quit record"
    );
    // A second save on the same tick walks the name instead of
    // overwriting the first.
    let again = write_named(&game, "again", &dir, 1_784_721_601).expect("the twin walks a counter");
    assert_ne!(saved, again);
    // The quit autosave stamps its own kind.
    let Ok(SaveOutcome::Wrote(auto_path)) = write_record(&mut game, &dir) else {
        panic!("the quit record lands");
    };
    let auto = crate::saved_game::inspect(&auto_path).expect("loads back");
    assert_eq!(auto.meta.kind.as_deref(), Some("autosave"));
    assert!(auto.meta.description.is_none());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unwritable_dir_reports_instead_of_lying() {
    // The would-be directory exists as a file, so create_dir_all
    // refuses — the class of trouble the old bool swallowed.
    let dir = scratch("unwritable");
    std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
    std::fs::write(&dir, b"in the way").unwrap();
    let mut game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
    game.advance_ticks(1);
    let err = write_record(&mut game, &dir).expect_err("the failure surfaces");
    assert!(matches!(err, SaveError::CreateDir { .. }));
    assert!(err.player_line().is_ascii(), "the menu font is Latin-1");
    assert!(!game.autosave_done, "a failed save still owes a record");
    std::fs::remove_file(&dir).ok();
}

#[test]
fn a_failed_publication_removes_only_its_uncommitted_reservation() {
    let dir = scratch("failed-reservation");
    std::fs::create_dir_all(&dir).unwrap();
    let held = free_path(&dir, "save", 1, 0).unwrap();
    let path = held.path.clone();
    assert!(held.publish(|_| Err::<(), _>("write refused")).is_err());
    assert!(!path.exists());
    let held = free_path(&dir, "save", 1, 0).unwrap();
    let path = held.path.clone();
    assert!(
        held.publish(|path| {
            chassis::fsx::write_atomic(path, |writer| {
                writer.write_all(b"published")?;
                Ok::<_, std::io::Error>(())
            })
            .unwrap();
            Err::<(), _>("late durability failure")
        })
        .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"published");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn collisions_beyond_one_thousand_remain_exclusively_reserved() {
    let dir = scratch("many-collisions");
    std::fs::create_dir_all(&dir).unwrap();
    let mut game = Game::new(oxide_sim::Scenario::skirmish()).expect("game");
    game.advance_ticks(1);
    let tick = game.state.current_tick();
    let seed = game.scenario.seed;
    for n in 0..1000 {
        let extension = "oxsave";
        let path = if n == 0 {
            dir.join(format!("save-{tick:010}.{extension}"))
        } else {
            dir.join(format!("save-{tick:010}-{seed}-{n}.{extension}"))
        };
        std::fs::write(path, b"occupied").expect("create known collision");
    }

    let first = write_named(&game, "first beyond the cutoff", &dir, 1)
        .expect("the first high-collision save lands");
    let second = write_named(&game, "second beyond the cutoff", &dir, 2)
        .expect("the next high-collision save lands");
    assert_ne!(first, second, "each save owns a distinct destination");
    assert_eq!(
        crate::saved_game::inspect(&first)
            .unwrap()
            .meta
            .description
            .as_deref(),
        Some("first beyond the cutoff")
    );
    assert_eq!(
        crate::saved_game::inspect(&second)
            .unwrap()
            .meta
            .description
            .as_deref(),
        Some("second beyond the cutoff")
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rotation_does_not_count_an_in_flight_reservation() {
    let dir = scratch("in-flight-rotation");
    let scenario = oxide_sim::Scenario::skirmish();
    for ticks in 1..=KEEP_AUTOSAVES {
        let mut game = Game::new(scenario.clone()).expect("game");
        game.advance_ticks(ticks as u64);
        let Ok(SaveOutcome::Wrote(path)) = write_record(&mut game, &dir) else {
            panic!("completed autosave lands");
        };
        crate::saved_game::inspect(&path).expect("completed autosave loads");
    }

    let mut game = Game::new(scenario).expect("game");
    game.advance_ticks(100);
    let held = free_path(
        &dir,
        "autosave",
        game.state.current_tick(),
        game.scenario.seed,
    )
    .expect("another saver holds an in-flight destination");
    let Ok(SaveOutcome::Wrote(new_path)) = write_record(&mut game, &dir) else {
        panic!("the concurrent completed autosave lands");
    };
    assert!(new_path.exists(), "rotation keeps the new completed save");

    let completed: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| crate::saved_game::inspect(path).is_ok())
        .collect();
    assert_eq!(
        completed.len(),
        KEEP_AUTOSAVES,
        "the marker consumes no completed-record retention slot"
    );
    assert!(completed.contains(&new_path));

    drop(held);
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        KEEP_AUTOSAVES,
        "a failed concurrent saver leaves the full completed shelf"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn rotation_touches_only_its_own_kind() {
    let dir = scratch("kinds");
    std::fs::create_dir_all(&dir).unwrap();
    // Six autosaves (one over budget), one match, one explicit
    // save, one fresh temp. Names sort in age order so the mtime
    // tie-break stays deterministic.
    for n in 0..6 {
        std::fs::write(dir.join(format!("autosave-000000000{n}.oxsave")), b"{}").unwrap();
    }
    std::fs::write(dir.join("match-0000000100.json"), b"{}").unwrap();
    std::fs::write(dir.join("save-outpost.json"), b"{}").unwrap();
    std::fs::write(dir.join("autosave-legacy.json"), b"{}").unwrap();
    std::fs::write(dir.join("autosave-0000000001.tmp.42.0"), b"live").unwrap();
    rotate(&dir);
    assert!(
        !dir.join("autosave-0000000000.oxsave").exists(),
        "only the oldest autosave died"
    );
    for n in 1..6 {
        assert!(dir.join(format!("autosave-000000000{n}.oxsave")).exists());
    }
    assert!(dir.join("match-0000000100.json").exists());
    assert!(dir.join("autosave-legacy.json").exists());
    assert!(
        dir.join("save-outpost.json").exists(),
        "explicit saves are never rotation's to take"
    );
    assert!(
        dir.join("autosave-0000000001.tmp.42.0").exists(),
        "a fresh temp is a write in flight, not an orphan"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn matches_rotate_against_their_own_budget() {
    let dir = scratch("matches");
    std::fs::create_dir_all(&dir).unwrap();
    for n in 0..=KEEP_MATCHES {
        std::fs::write(dir.join(format!("match-{n:010}.json")), b"{}").unwrap();
    }
    rotate(&dir);
    assert!(!dir.join(format!("match-{:010}.json", 0)).exists());
    for n in 1..=KEEP_MATCHES {
        assert!(dir.join(format!("match-{n:010}.json")).exists());
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_directory_at_budget_loses_nothing() {
    let dir = scratch("at-budget");
    std::fs::create_dir_all(&dir).unwrap();
    for n in 0..KEEP_AUTOSAVES {
        std::fs::write(dir.join(format!("autosave-{n:010}.oxsave")), b"{}").unwrap();
    }
    rotate(&dir);
    for n in 0..KEEP_AUTOSAVES {
        assert!(dir.join(format!("autosave-{n:010}.oxsave")).exists());
    }
    std::fs::remove_dir_all(&dir).ok();
}
