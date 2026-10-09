use super::*;
use oxide_sim::scenario::BotConfig;
use oxide_sim::{Command, PlayerId};

fn checkpoint() -> SessionCheckpoint {
    let mut scenario = Scenario::skirmish();
    for seat in &mut scenario.players {
        seat.bot = false;
        seat.bot_config = None;
    }
    let mut state = scenario.build().unwrap();
    let mut stats = LiveMatchStats::new(&state);
    for _ in 0..105 {
        let report = state.tick(&[]);
        stats.observe(&state, &report.events);
    }
    let pending = vec![PlayerCommand {
        player: PlayerId(0),
        command: Command::Stop { units: vec![] },
    }];
    SessionCheckpoint::capture(&scenario, &state, &[], &pending, Some(&stats)).unwrap()
}

#[test]
fn checkpoint_round_trip_is_observational_and_keeps_statistics_stride() {
    let original = checkpoint();
    let bytes = serde_json::to_vec(&original).unwrap();
    let decoded: SessionCheckpoint = serde_json::from_slice(&bytes).unwrap();
    let mut a = original.restore().unwrap();
    let mut b = decoded.restore().unwrap();
    assert_eq!(a.state.current_tick(), 105);
    assert_eq!(a.pending, b.pending);
    assert_eq!(a.state.hash(), b.state.hash());
    for _ in 0..120 {
        let report = a.state.tick(&std::mem::take(&mut a.pending));
        let other = b.state.tick(&std::mem::take(&mut b.pending));
        assert_eq!(report.events, other.events);
        a.stats.as_mut().unwrap().observe(&a.state, &report.events);
        b.stats.as_mut().unwrap().observe(&b.state, &other.events);
        assert_eq!(
            a.stats.as_ref().unwrap().snapshot(&a.state),
            b.stats.as_ref().unwrap().snapshot(&b.state)
        );
    }
}

#[test]
fn checkpoint_rejects_a_world_with_different_scenario_rules() {
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = false;
        player.bot_config = None;
    }
    let state = scenario.build().unwrap();
    scenario.mode = oxide_sim::scenario::ScenarioMode::Sandbox;
    let error = SessionCheckpoint::capture(&scenario, &state, &[], &[], None).unwrap_err();
    assert!(error.to_string().contains("scenario mode mismatch"));
    let state = scenario.build().unwrap();
    let restored = SessionCheckpoint::capture(&scenario, &state, &[], &[], None)
        .unwrap()
        .restore()
        .unwrap();
    assert_eq!(
        restored.state.mode(),
        oxide_sim::scenario::ScenarioMode::Sandbox
    );
}

#[test]
fn checkpoint_rejects_incompatible_or_inconsistent_parts() {
    let original = checkpoint();
    for version in [0, VERSION + 1] {
        let mut bad = original.clone();
        bad.version = version;
        assert!(bad.restore().is_err());
    }
    let mut bad = original.clone();
    bad.sim_version = SIM_VERSION + 1;
    assert!(bad.restore().is_err());
    let mut bad = original.clone();
    bad.pending[0].player = PlayerId(255);
    assert!(bad.restore().is_err());
    let mut bad = original.clone();
    bad.scenario.players[0].bot = true;
    bad.scenario.players[0].bot_config = Some(BotConfig::default());
    assert!(bad.restore().is_err());
    let json = serde_json::to_value(original).unwrap();
    for (field, value) in [
        ("every", serde_json::json!(0)),
        ("every", serde_json::json!(3)),
        ("stats", serde_json::json!({})),
    ] {
        let mut bad = json.clone();
        bad["stats"][field] = value;
        let result = serde_json::from_value::<SessionCheckpoint>(bad)
            .map_err(anyhow::Error::from)
            .and_then(SessionCheckpoint::restore);
        assert!(result.is_err());
    }
    assert!(serde_json::from_slice::<SessionCheckpoint>(b"{}").is_err());
}

#[test]
fn checkpoint_rejects_changes_to_the_captured_setup_and_world_pair() {
    let original = checkpoint();
    for change_world in [false, true] {
        let mut bad = original.clone();
        if change_world {
            bad.state.tick(&[]);
        } else {
            bad.scenario.name.push('!');
        }
        let bytes = serde_json::to_vec(&bad).unwrap();
        let error = serde_json::from_slice::<SessionCheckpoint>(&bytes)
            .unwrap()
            .restore()
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("scenario/world binding mismatch")
        );
    }
}

#[test]
fn a_save_between_an_own_event_and_the_next_decision_resumes_identically() {
    use crate::controller::{record_events, seat_controllers};
    let scenario = crate::controller::mixed_skirmish();
    let mut state = scenario.build().unwrap();
    let mut bots = seat_controllers(&scenario).unwrap();
    for _ in 0..25 {
        crate::runner::step(&mut state, &mut bots, None);
    }
    let origin = SessionCheckpoint::capture(&scenario, &state, &bots, &[], None).unwrap();
    let mut suffix = origin
        .recording(&crate::recovery::BuildIdentity::default())
        .unwrap();
    let mut commands = crate::bot_execution::commands(&state, &mut bots);
    commands.extend([0, 1].map(|seat| PlayerCommand {
        player: PlayerId(seat),
        command: Command::Stop { units: vec![] },
    }));
    let report = crate::runner::record_and_tick(&mut state, &commands, Some(&mut suffix));
    record_events(&mut bots, &report);
    suffix.meta.ticks = Some(state.current_tick());

    let rejected = serde_json::json!([{"event": "command_rejected", "reason": "no_valid_units"}]);
    let json = serde_json::to_value(
        SessionCheckpoint::capture(&scenario, &state, &bots, &[], None).unwrap(),
    )
    .unwrap();
    assert_eq!(json["bots"][0]["events"], rejected);
    let mut restored = serde_json::from_value::<SessionCheckpoint>(json.clone())
        .unwrap()
        .restore()
        .unwrap();
    let mut recovered = origin.resume_recording(&suffix).unwrap();
    let rebuilt = SessionCheckpoint::capture(
        &scenario,
        &recovered.state,
        &recovered.bots,
        &recovered.pending,
        None,
    )
    .unwrap();
    assert_eq!(serde_json::to_value(rebuilt).unwrap(), json);

    let mut received = Vec::new();
    for _ in 0..60 {
        let original = crate::runner::step_traced(&mut state, &mut bots, None);
        for session in [&mut restored, &mut recovered] {
            let resumed = crate::runner::step_traced(&mut session.state, &mut session.bots, None);
            assert_eq!(resumed.report, original.report);
            assert_eq!(resumed.traces, original.traces);
        }
        received.extend(
            original
                .traces
                .into_iter()
                .filter(|trace| !trace.events.is_empty())
                .map(|trace| (trace.tick, serde_json::to_value(trace.events).unwrap())),
        );
    }
    assert_eq!(
        received[..2],
        [(30, rejected.clone()), (36, rejected)],
        "each seat hears its own rejection at its next decision: Prime decides every 6 ticks, Standard every 12"
    );
    assert_eq!(restored.state.hash(), state.hash());
    assert_eq!(recovered.state.hash(), state.hash());
}

#[test]
fn bot_seats_round_trip_and_reject_forged_rosters() {
    let scenario = crate::controller::mixed_skirmish();
    let mut state = scenario.build().unwrap();
    let mut bots = crate::controller::seat_controllers(&scenario).unwrap();
    for _ in 0..150 {
        crate::runner::step(&mut state, &mut bots, None);
    }
    let captured = SessionCheckpoint::capture(&scenario, &state, &bots, &[], None).unwrap();
    let json = serde_json::to_value(&captured).unwrap();
    assert_eq!(json["bots"][0]["controller"]["player"], 0);
    assert_eq!(json["bots"][0]["events"], serde_json::json!([]));
    assert_eq!(json["bots"][1]["controller"]["player"], 1);

    let mut restored = serde_json::from_value::<SessionCheckpoint>(json.clone())
        .unwrap()
        .restore()
        .unwrap();
    assert_eq!(roster(&restored.bots), roster(&bots));
    let mut issued = [0; 2];
    for _ in 0..300 {
        let commands = crate::bot_execution::commands(&state, &mut bots);
        let resumed = crate::bot_execution::commands(&restored.state, &mut restored.bots);
        assert_eq!(commands, resumed);
        for command in &commands {
            issued[usize::from(command.player.0)] += 1;
        }
        assert_eq!(
            state.tick(&commands).events,
            restored.state.tick(&resumed).events
        );
    }
    assert_eq!(state.hash(), restored.state.hash());
    assert!(issued.iter().all(|count| *count > 0), "{issued:?}");

    let forged = |edit: &dyn Fn(&mut serde_json::Value)| {
        let mut bad = json.clone();
        edit(&mut bad);
        serde_json::from_value::<SessionCheckpoint>(bad)
            .map_err(anyhow::Error::from)
            .and_then(SessionCheckpoint::restore)
            .err()
            .expect("a forged controller roster must not restore")
    };
    forged(&|bad| bad["bots"].as_array_mut().unwrap().swap(0, 1));
    forged(&|bad| {
        bad["bots"][1] = serde_json::json!({"controller": {"player": 1}, "events": []});
    });
    forged(&|bad| bad["bots"][0]["memory"] = serde_json::json!([]));
    forged(&|bad| bad["bots"][0] = bad["bots"][1].clone());
    forged(&|bad| bad["bots"].as_array_mut().unwrap().truncate(1));
    forged(&|bad| bad["bots"][0] = serde_json::json!({"oracle": {"player": 0}}));
    let mut reassigned = captured;
    reassigned.scenario.players[0].bot_config = reassigned.scenario.players[1].bot_config;
    assert!(reassigned.restore().is_err());
}
