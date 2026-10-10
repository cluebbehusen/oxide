use super::*;
use crate::support_tests::replay_fixture as fixture;

#[test]
fn inspection_captures_exact_sorted_ticks_and_fog() {
    let report =
        inspect(&fixture(), &[12, 5, 0, 5], Some(PlayerId(1)), true).expect("fixture inspects");

    assert_eq!(report.schema_version, 1);
    assert_eq!(report.scenario.name, "Skirmish Basin");
    assert_eq!(report.scenario.players[1].seat, 1);
    assert!(report.scenario.players[1].bot);
    assert!(report.scenario.players[1].bot_config.is_some());
    assert_eq!(
        report
            .snapshots
            .iter()
            .map(|snapshot| snapshot.tick)
            .collect::<Vec<_>>(),
        vec![0, 5, 12]
    );
    for snapshot in &report.snapshots {
        assert_eq!(snapshot.state.tick, snapshot.tick);
        assert!(snapshot.state.map.is_some());
        let fog = snapshot.fog.as_ref().expect("fog requested");
        assert_eq!(fog.tick, snapshot.tick);
        assert_eq!(fog.player, 1);
    }
    assert_eq!(
        report.final_state.hash,
        report.snapshots.last().expect("final snapshot").state.hash
    );
}

#[test]
fn activity_reports_command_counts_types_and_earliest_longest_gap() {
    let report = inspect(&fixture(), &[], None, false).expect("fixture inspects");
    let seat0 = &report.command_activity[0];
    assert_eq!(seat0.command_count, 3);
    assert_eq!(seat0.first_command_tick, Some(0));
    assert_eq!(seat0.last_command_tick, Some(10));
    assert_eq!(seat0.by_type.get("stop"), Some(&3));
    assert_eq!(
        seat0.longest_silence,
        CommandSilence {
            from_tick: 0,
            to_tick: 5,
            duration_ticks: 5,
            start_boundary: SilenceBoundary::Command,
            end_boundary: SilenceBoundary::Command,
        }
    );

    let seat1 = &report.command_activity[1];
    assert_eq!(seat1.command_count, 2);
    assert_eq!(
        seat1.longest_silence,
        CommandSilence {
            from_tick: 3,
            to_tick: 8,
            duration_ticks: 5,
            start_boundary: SilenceBoundary::Command,
            end_boundary: SilenceBoundary::Command,
        }
    );
}

#[test]
fn inspection_rejects_snapshot_and_fog_seats_outside_the_replay() {
    let replay = fixture();
    let tick_error = inspect(&replay, &[13], None, false).expect_err("tick is too late");
    assert!(
        tick_error
            .to_string()
            .contains("exceeds replay duration 12")
    );

    let fog_error =
        inspect(&replay, &[], Some(PlayerId(2)), false).expect_err("seat does not exist");
    assert!(
        fog_error
            .to_string()
            .contains("outside this replay's 2 seats")
    );
}
