use super::*;

#[test]
fn configured_seats_run_in_seat_order_as_their_own_opponent_would() {
    let mut scenario = mixed_skirmish();
    let state = scenario.build().unwrap();
    let mut seats = seat_controllers(&scenario).unwrap();
    assert_eq!(
        seats.iter().map(SeatController::player).collect::<Vec<_>>(),
        [PlayerId(0), PlayerId(1)]
    );
    let model = Arc::new(MapModel::from_scenario(&scenario).unwrap());
    assert!(seats.iter().all(|seat| seat.decision_due(&state)));
    for (seat, player) in seats.iter_mut().zip([0, 1]) {
        let mut direct = Opponent::new(
            PlayerId(player),
            scenario.players[usize::from(player)].bot_config.unwrap(),
            Arc::clone(&model),
        );
        assert_eq!(
            seat.act(&state),
            direct.act(&state, &mut OwnEvents::default())
        );
    }

    scenario.players[0].bot_config = None;
    assert_eq!(seat_controllers(&scenario).unwrap().len(), 1);
}

#[test]
fn checkpoints_restore_the_same_seat_and_reject_a_swapped_one() {
    let scenario = mixed_skirmish();
    let state = scenario.build().unwrap();
    let seats = seat_controllers(&scenario).unwrap();
    let checkpoints: Vec<_> = seats.iter().map(SeatController::checkpoint).collect();
    let json = serde_json::to_value(&checkpoints).unwrap();
    assert_eq!(json[0]["controller"]["player"], 0);
    assert_eq!(json[0]["events"], serde_json::json!([]));
    let decoded: Vec<ControllerCheckpoint> = serde_json::from_value(json).unwrap();
    let opponent_map = OpponentMap::new(&scenario);
    for (seat, checkpoint) in seats.iter().zip(&decoded) {
        let mut restored =
            SeatController::restore(checkpoint, &scenario, &state, &opponent_map).unwrap();
        assert_eq!(restored.player(), seat.player());
        assert_eq!(restored.act(&state), seat.clone().act(&state));
    }

    let mut human = scenario.clone();
    human.players[1].bot = false;
    let human_map = OpponentMap::new(&human);
    assert!(SeatController::restore(&decoded[1], &human, &state, &human_map).is_err());
}

#[test]
fn seats_share_one_map_model_built_on_first_use() {
    let scenario = mixed_skirmish();
    let opponent_map = OpponentMap::new(&scenario);
    assert!(opponent_map.model.get().is_none());
    let first = opponent_map.model().unwrap();
    assert!(Arc::ptr_eq(&first, &opponent_map.model().unwrap()));

    let mut empty = scenario.clone();
    for player in &mut empty.players {
        player.bot_config = None;
    }
    let opponent_map = OpponentMap::new(&empty);
    assert!(seat_controllers(&empty).unwrap().is_empty());
    assert!(opponent_map.model.get().is_none());
}

#[test]
fn hosts_read_the_scrap_a_seat_protects() {
    let scenario = mixed_skirmish();
    let mut state = scenario.build().unwrap();
    let mut seats = seat_controllers(&scenario).unwrap();
    let mut protected = false;
    while state.current_tick() < 2_400 {
        let mut commands = Vec::new();
        for seat in &mut seats {
            let (decided, trace) = seat.act_traced(&state);
            if let Some(trace) = trace {
                assert_eq!(trace.protected, seat.protected_scrap());
                protected |= trace.protected > 0;
            }
            commands.extend(decided);
        }
        let report = state.tick(&commands);
        record_events(&mut seats, &report);
    }
    assert!(protected, "premise: a seat saved for something");
}

#[test]
fn seats_hear_only_their_own_failures_at_their_next_decision() {
    let scenario = mixed_skirmish();
    let mut state = scenario.build().unwrap();
    let mut seats = seat_controllers(&scenario).unwrap();
    let stop = |player: u8| PlayerCommand {
        player: PlayerId(player),
        command: oxide_sim::Command::Stop { units: Vec::new() },
    };
    let saved = |seats: &[SeatController]| {
        seats
            .iter()
            .map(|seat| serde_json::to_value(seat.checkpoint()).unwrap())
            .collect::<Vec<_>>()
    };
    let mut commands: Vec<_> = seats.iter_mut().flat_map(|seat| seat.act(&state)).collect();
    commands.push(stop(0));
    let report = state.tick(&commands);
    record_events(&mut seats, &report);
    let rejected = serde_json::json!([{"event": "command_rejected", "reason": "no_valid_units"}]);
    let before = saved(&seats);
    assert_eq!(before[0]["events"], rejected);
    assert_eq!(before[1]["events"], serde_json::json!([]));

    let mut traces = Vec::new();
    while state.current_tick() <= 12 {
        let mut commands = Vec::new();
        for seat in &mut seats {
            let (issued, trace) = seat.act_traced(&state);
            commands.extend(issued);
            traces.extend(trace.filter(|trace| trace.player == PlayerId(0)));
        }
        let report = state.tick(&commands);
        record_events(&mut seats, &report);
    }
    let [trace] = traces.as_slice() else {
        panic!("one decision of seat zero: {traces:?}");
    };
    assert_eq!(trace.tick, 12);
    assert_eq!(serde_json::to_value(&trace.events).unwrap(), rejected);
    assert_eq!(saved(&seats)[0]["events"], serde_json::json!([]));
}
