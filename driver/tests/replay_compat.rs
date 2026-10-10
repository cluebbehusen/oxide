//! Checks at the replay file-loading boundary and the `replay` command.

use chassis::replay::ReplayError;
use oxide_kit::GameReplay;
use oxide_sim::{SIM_VERSION, Scenario};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[test]
fn checkpoint_origins_reach_every_read_only_replay_surface() {
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    for _ in 0..37 {
        state.tick(&[]);
    }
    let origin = oxide_kit::recording::WorldOrigin::capture(&scenario, &state).unwrap();
    let mut replay = GameReplay::with_origin(SIM_VERSION, "test", scenario, origin).unwrap();
    replay.meta.ticks = Some(43);
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let file = TempReplay(
        std::env::temp_dir().join(format!("oxide-origin-{}-{id}.json", std::process::id())),
    );
    replay.save(&file.0).unwrap();
    let replay = oxide_kit::load_replay(&file.0).unwrap();
    let report = oxide_driver::replay_inspect::inspect(&replay, &[37, 43], None, false).unwrap();
    assert_eq!(report.start_tick, 37);
    assert_eq!(
        report
            .snapshots
            .iter()
            .map(|snapshot| snapshot.tick)
            .collect::<Vec<_>>(),
        [37, 43]
    );
    assert_eq!(report.command_activity[0].longest_silence.from_tick, 37);
    assert_eq!(report.command_activity[0].longest_silence.duration_ticks, 6);
    assert!(oxide_driver::replay_inspect::inspect(&replay, &[36], None, false).is_err());
    let options = oxide_driver::replay_summary::SummaryOptions {
        until: None,
        every: None,
        minimaps: oxide_driver::replay_summary::MinimapMode::None,
    };
    let report = oxide_driver::replay_summary::summarize(&replay, &options).unwrap();
    assert_eq!(report.scenario.start_tick, 37);
    assert_eq!(report.scenario.effective_ticks, 43);
    assert!(
        report
            .digests
            .iter()
            .flat_map(|digest| &digest.rows)
            .all(|row| row.explored_tiles > 0 && row.explored_delta == 0)
    );
    let at_origin = oxide_driver::replay_summary::summarize(
        &replay,
        &oxide_driver::replay_summary::SummaryOptions {
            until: Some(37),
            ..options
        },
    )
    .unwrap();
    assert_eq!(at_origin.digests.len(), 1);
    assert!(
        at_origin.digests[0]
            .rows
            .iter()
            .all(|row| row.explored_delta == 0)
    );
    assert!(
        report
            .tech_reach
            .iter()
            .flat_map(|seat| &seat.firsts)
            .all(|first| first.tick >= 37)
    );
    let end = oxide_kit::runner::run_replay(&replay, None).unwrap();
    assert_eq!(end.current_tick(), 43);
    assert!(oxide_driver::session::Session::resume(replay).is_err());
}

#[test]
fn checkpoint_exploration_deltas_count_only_new_scouting() {
    use chassis::grid::TilePos;
    use oxide_sim::{Command, PlayerCommand, PlayerId};
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    for _ in 0..37 {
        state.tick(&[]);
    }
    let explored = |state: &oxide_sim::State| -> u64 {
        (0..state.map().height())
            .flat_map(|y| (0..state.map().width()).map(move |x| TilePos::new(x, y)))
            .filter(|&tile| state.vision(PlayerId(0)).explored(tile))
            .count() as u64
    };
    let initial = explored(&state);
    let mut replay = GameReplay::with_origin(
        SIM_VERSION,
        "test",
        scenario.clone(),
        oxide_kit::recording::WorldOrigin::capture(&scenario, &state).unwrap(),
    )
    .unwrap();
    let unit = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0))
        .unwrap()
        .id;
    let command = PlayerCommand {
        player: PlayerId(0),
        command: Command::Run {
            units: vec![unit],
            goal: TilePos::new(18, 8),
            queue: false,
        },
    };
    replay.record(37, command.clone());
    let first = state.tick(&[command]);
    assert!(
        !first
            .events
            .iter()
            .any(|event| matches!(event, oxide_sim::Event::CommandRejected { .. }))
    );
    for _ in 38..237 {
        state.tick(&[]);
    }
    replay.meta.ticks = Some(237);
    let final_explored = explored(&state);
    assert!(final_explored > initial, "scouting must reveal new ground");
    let options = oxide_driver::replay_summary::SummaryOptions {
        until: None,
        every: Some(50),
        minimaps: oxide_driver::replay_summary::MinimapMode::None,
    };
    let report = oxide_driver::replay_summary::summarize(&replay, &options).unwrap();
    assert_eq!(
        report
            .digests
            .iter()
            .map(|digest| digest.rows[0].explored_delta)
            .sum::<u64>(),
        final_explored - initial
    );
    assert_eq!(
        report.digests.last().unwrap().rows[0].explored_tiles,
        final_explored
    );
    let mut from_start = GameReplay::new(SIM_VERSION, "test", scenario);
    from_start.meta.ticks = Some(1);
    let report = oxide_driver::replay_summary::summarize(&from_start, &options).unwrap();
    assert_eq!(
        report.digests[0].rows[0].explored_delta, initial,
        "scenario-start summaries retain their initial visibility"
    );
}

struct TempReplay(PathBuf);

impl Drop for TempReplay {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn foreign_version_replay() -> TempReplay {
    let replay: GameReplay = GameReplay::new(SIM_VERSION + 1, "test", Scenario::skirmish());
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "oxide-foreign-version-replay-{}-{id}.json",
        std::process::id()
    ));
    std::fs::write(
        &path,
        serde_json::to_vec(&replay).expect("fixture serializes"),
    )
    .expect("fixture is written");
    TempReplay(path)
}

#[test]
fn a_foreign_version_replay_is_refused_at_version_validation() {
    let fixture = foreign_version_replay();
    let replay = oxide_kit::load_replay(&fixture.0).expect("the current setup shape loads");
    assert!(matches!(
        replay.validate(Some(SIM_VERSION)),
        Err(ReplayError::VersionMismatch { .. })
    ));

    let refused = std::process::Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
        .arg("replay")
        .arg(&fixture.0)
        .output()
        .expect("run replay");
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("replay was recorded on sim"),
        "the replay command should refuse at version validation: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[test]
fn hash_every_lines_repeat_exactly_and_end_at_the_final_hash() {
    let mut replay = GameReplay::new(SIM_VERSION, "test", Scenario::skirmish());
    replay.meta.ticks = Some(25);
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let file = TempReplay(
        std::env::temp_dir().join(format!("oxide-hash-every-{}-{id}.json", std::process::id())),
    );
    replay.save(&file.0).unwrap();
    let run = |extra: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
            .arg("replay")
            .arg(&file.0)
            .args(extra)
            .output()
            .expect("run replay");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>()
    };
    let lines = run(&["--hash-every", "10"]);
    assert_eq!(lines, run(&["--hash-every", "10"]));
    let ticks: Vec<_> = lines.iter().map(|line| line["tick"].as_u64()).collect();
    assert_eq!(ticks, [Some(10), Some(20), Some(25)]);
    assert!(lines.iter().all(|line| line["events"].is_string()));
    let plain = run(&[]);
    assert_eq!(plain.len(), 1);
    assert_eq!(lines[2]["hash"], plain[0]["hash"]);
    assert!(plain[0].get("events").is_none());
}

#[test]
fn until_runs_a_prefix_while_a_short_ticks_override_still_refuses() {
    use chassis::replay::Replay;
    use oxide_sim::{Command, PlayerCommand, PlayerId, UnitId};
    let mut replay: GameReplay = Replay::new(SIM_VERSION, "test", Scenario::skirmish());
    replay.record(
        100,
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Stop {
                units: vec![UnitId(0)],
            },
        },
    );
    replay.meta.ticks = Some(200);
    let id = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let file = TempReplay(
        std::env::temp_dir().join(format!("oxide-until-{}-{id}.json", std::process::id())),
    );
    replay.save(&file.0).unwrap();
    let run = |extra: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_oxide-driver"))
            .arg("replay")
            .arg(&file.0)
            .args(extra)
            .output()
            .expect("run replay")
    };
    let prefix = run(&["--until", "50", "--hash-every", "25"]);
    assert!(
        prefix.status.success(),
        "{}",
        String::from_utf8_lossy(&prefix.stderr)
    );
    let ticks: Vec<_> = String::from_utf8(prefix.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["tick"].as_u64())
        .collect();
    assert_eq!(ticks, [Some(25), Some(50), Some(50)]);
    let truncated = run(&["--ticks", "50"]);
    assert!(!truncated.status.success());
    assert!(String::from_utf8_lossy(&truncated.stderr).contains("unconsumed"));
}
