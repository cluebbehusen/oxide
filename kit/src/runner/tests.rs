use super::*;

#[test]
fn traced_step_preserves_the_authoritative_command_and_tick_path() {
    for (scenario, seats) in [
        (Scenario::skirmish(), vec![1]),
        (crate::controller::mixed_skirmish(), vec![0, 1]),
    ] {
        let mut ordinary_state = scenario.build().unwrap();
        let mut traced_state = scenario.build().unwrap();
        let mut ordinary_bots = seat_controllers(&scenario).unwrap();
        let mut traced_bots = seat_controllers(&scenario).unwrap();
        let mut ordinary_replay = GameReplay::new(SIM_VERSION, scenario.clone());
        let mut traced_replay = GameReplay::new(SIM_VERSION, scenario);
        let mut traces = Vec::new();

        for _ in 0..25 {
            let ordinary_report = step(
                &mut ordinary_state,
                &mut ordinary_bots,
                Some(&mut ordinary_replay),
            );
            let traced = step_traced(
                &mut traced_state,
                &mut traced_bots,
                Some(&mut traced_replay),
            );
            assert_eq!(traced.report, ordinary_report);
            traces.extend(traced.traces);
        }

        let mut traced_seats: Vec<u8> = traces.iter().map(|trace| trace.player.0).collect();
        traced_seats.sort_unstable();
        traced_seats.dedup();
        assert_eq!(traced_seats, seats, "every configured bot should think");
        assert!(traces.iter().all(|trace| trace.tick < 25));
        assert_eq!(traced_state.hash(), ordinary_state.hash());
        assert_eq!(
            serde_json::to_vec(&traced_replay).unwrap(),
            serde_json::to_vec(&ordinary_replay).unwrap(),
            "diagnostics must not alter replay commands"
        );
    }
}
