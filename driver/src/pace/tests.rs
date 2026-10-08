use super::*;
use crate::sweep::SweepMatch;

/// A cap far too small to decide anything: every 1v1 map of the
/// shipped directory must still produce a row, fully censored, with
/// no quantile inventing a duration out of nothing — and each row
/// must carry the two things the measurement is read against.
#[test]
fn the_slate_rows_every_duel_map_and_admits_full_censoring() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios");
    let slate =
        run_pace_sweep(dir, 1, 20, 3_000, oxide_sim::scenario::BotConfig::default()).unwrap();
    // This is a broad directory probe rather than a fixed roster
    // count, so adding or retiring maps does not make it brittle.
    assert!(slate.per_map.len() >= 5, "the 1v1 roster is present");
    assert_eq!(slate.decided, 0);
    assert_eq!(slate.undecided, slate.matches);
    assert_eq!(slate.matches, u32::try_from(slate.per_map.len()).unwrap());
    for r in &slate.per_map {
        assert_eq!(r.matches, 1, "{}: one seed, one match", r.scenario);
        assert_eq!(r.censored_percent, 100.0);
        assert!(r.median.is_none(), "{}: no decision to report", r.scenario);
        assert!(r.p25.is_none() && r.p75.is_none());
        assert!(
            !r.pace.is_empty(),
            "{}: shipped maps declare a pace",
            r.scenario
        );
        assert!(
            r.ground_route.is_some(),
            "{}: shipped maps are connected by some mover",
            r.scenario
        );
        assert_eq!(r.sweep.matches.len(), 1);
    }
}

/// The fold reads the sweep's own match list: draws are decisions,
/// the cap is censoring, and the quantiles see only the decided.
#[test]
fn the_fold_counts_draws_as_decisions_and_the_cap_as_censoring() {
    let scenario = crate::runner::load_scenario("skirmish").unwrap();
    let played = |ticks, outcome| SweepMatch {
        seed: 1,
        ticks,
        outcome,
    };
    let sweep = SweepReport {
        bot_config: oxide_sim::scenario::BotConfig::default(),
        sim_version: oxide_sim::SIM_VERSION.to_string(),
        scenario: scenario.name.clone(),
        seeds: 4,
        max_ticks: 999,
        victories: 2,
        draws: 1,
        undecided: 1,
        seat_wins: [1, 1],
        median_decision_tick: Some(200),
        matches: vec![
            played(300, SweepOutcome::Victory { seat: 0 }),
            played(100, SweepOutcome::Victory { seat: 1 }),
            played(200, SweepOutcome::Draw),
            played(999, SweepOutcome::Undecided),
        ],
    };
    let row = row("skirmish", &scenario, sweep).unwrap();
    assert_eq!((row.matches, row.decided, row.undecided), (4, 3, 1));
    assert_eq!(row.censored_percent, 25.0);
    assert_eq!(row.p25.as_ref().unwrap().ticks, 100);
    assert_eq!(row.median.as_ref().unwrap().ticks, 200);
    assert_eq!(row.p75.as_ref().unwrap().ticks, 300);
    assert_eq!(row.pace, "standard");
    assert!(row.ground_route.is_some());
}

/// Ticks read out as the clock a player feels, zero-padded seconds
/// and minutes past the hour left to run.
#[test]
fn elapsed_reads_the_wall_clock() {
    assert_eq!(Elapsed::new(0).clock, "0:00");
    assert_eq!(Elapsed::new(19).clock, "0:00");
    assert_eq!(Elapsed::new(20).clock, "0:01");
    assert_eq!(Elapsed::new(5_541).clock, "4:37");
    assert_eq!(Elapsed::new(26_357).clock, "21:57");
    assert_eq!(Elapsed::new(5_541).cell(), "5541 (4:37)");
}
