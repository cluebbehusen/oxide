use super::*;
use oxide_sim::scenario::BotDifficulty;
use oxide_sim::scenario::BotStance;
use oxide_sim::{Command, PlayerId};

#[test]
fn replay_resume_keeps_pending_controller_events_and_recording() {
    let mut scenario = Scenario::skirmish();
    for seat in &mut scenario.players {
        seat.bot = true;
        seat.bot_config = Some(oxide_sim::scenario::BotConfig::default());
    }
    scenario.players[0].bot_config = Some(oxide_sim::scenario::BotConfig::new(
        BotDifficulty::default(),
        BotStance::default(),
        0,
    ));
    let mut original = Session::new(scenario).unwrap();
    for _ in 0..121 {
        original.step();
    }
    original
        .handle(Request::SendCommand {
            player: PlayerId(0),
            command: Command::Stop { units: vec![] },
        })
        .unwrap();
    original.step();
    let controllers = |session: &Session| {
        let checkpoint = oxide_kit::checkpoint::SessionCheckpoint::capture(
            &session.scenario,
            &session.state,
            &session.bots,
            &session.pending,
            None,
        )
        .unwrap();
        serde_json::to_value(checkpoint).unwrap()["bots"].clone()
    };
    let pending = controllers(&original);
    assert_eq!(
        pending[0]["events"],
        serde_json::json!([{"event": "command_rejected", "reason": "no_valid_units"}])
    );
    let mut snapshot = original.recorder.clone();
    snapshot.meta.ticks = Some(original.state.current_tick());
    let mut resumed = Session::resume(snapshot).unwrap();
    assert_eq!(controllers(&resumed), pending);
    for _ in 0..240 {
        assert_eq!(original.step(), resumed.step());
        assert_eq!(original.state.hash(), resumed.state.hash());
    }
    assert_eq!(
        serde_json::to_vec(&original.recorder.commands).unwrap(),
        serde_json::to_vec(&resumed.recorder.commands).unwrap()
    );
    for seat in [PlayerId(0), PlayerId(1)] {
        assert!(
            original
                .recorder
                .commands
                .iter()
                .any(|timed| timed.command.player == seat)
        );
    }
}

fn human_scenario(name: &str) -> Scenario {
    let mut scenario = Scenario::skirmish();
    scenario.name = name.to_string();
    for player in &mut scenario.players {
        player.bot = false;
        player.bot_config = None;
    }
    scenario
}

fn scratch(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "oxide-driver-session-{name}-{}",
        std::process::id()
    ));
    std::fs::remove_dir_all(&path).ok();
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn failed_replacements_leave_state_and_staged_commands_intact() {
    let dir = scratch("failed-replace");
    let mut session = Session::new(human_scenario("original")).unwrap();
    session.handle(Request::AdvanceTicks { ticks: 2 }).unwrap();
    session
        .handle(Request::SendCommand {
            player: PlayerId(0),
            command: Command::Stop { units: Vec::new() },
        })
        .unwrap();
    let before_tick = session.state().current_tick();
    let before_hash = session.state().hash();

    let missing = dir.join("missing-scenario.json");
    assert!(
        session
            .handle(Request::LoadScenario {
                path: missing.to_string_lossy().into_owned(),
            })
            .is_err()
    );
    let malformed = dir.join("malformed-replay.json");
    std::fs::write(&malformed, b"not a replay").unwrap();
    assert!(
        session
            .handle(Request::LoadReplay {
                path: malformed.to_string_lossy().into_owned(),
            })
            .is_err()
    );
    assert_eq!(session.state().current_tick(), before_tick);
    assert_eq!(session.state().hash(), before_hash);

    session.handle(Request::PresentTicks { ticks: 1 }).unwrap();
    assert_eq!(session.status().recorded_commands, 1);
    assert_eq!(session.state().current_tick(), before_tick + 1);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn successful_scenario_replacement_resets_the_entire_session() {
    let dir = scratch("successful-replace");
    let replacement_path = dir.join("replacement.json");
    let replacement = human_scenario("replacement");
    std::fs::write(
        &replacement_path,
        serde_json::to_vec_pretty(&replacement).unwrap(),
    )
    .unwrap();

    let mut session = Session::new(human_scenario("original")).unwrap();
    session.handle(Request::AdvanceTicks { ticks: 4 }).unwrap();
    session
        .handle(Request::SendCommand {
            player: PlayerId(0),
            command: Command::Stop { units: Vec::new() },
        })
        .unwrap();

    assert!(matches!(
        session
            .handle(Request::LoadScenario {
                path: replacement_path.to_string_lossy().into_owned(),
            })
            .unwrap(),
        Reply::Ok
    ));
    let status = session.status();
    assert_eq!(status.scenario, "replacement");
    assert_eq!(status.tick, 0);
    assert_eq!(status.recorded_commands, 0);
    session.handle(Request::PresentTicks { ticks: 1 }).unwrap();
    assert_eq!(
        session.status().recorded_commands,
        0,
        "old pending work was discarded"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn replay_without_duration_resumes_through_its_last_command() {
    let mut replay = GameReplay::new(SIM_VERSION, "test", human_scenario("legacy"));
    replay.record(
        3,
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Stop { units: Vec::new() },
        },
    );
    assert_eq!(replay.meta.ticks, None);
    let expected = runner::run_replay(&replay, None).unwrap();

    let session = Session::resume(replay).unwrap();
    assert_eq!(session.state().current_tick(), 4);
    assert_eq!(session.state().hash(), expected.hash());
    assert_eq!(session.status().recorded_commands, 1);
}

#[test]
fn commands_from_unknown_players_are_not_staged() {
    let mut session = Session::new(human_scenario("players")).unwrap();
    let error = session
        .handle(Request::SendCommand {
            player: PlayerId(99),
            command: Command::Stop { units: Vec::new() },
        })
        .unwrap_err();
    assert!(error.contains("no such player"));
    session.handle(Request::PresentTicks { ticks: 1 }).unwrap();
    assert_eq!(session.status().recorded_commands, 0);
}
