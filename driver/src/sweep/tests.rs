use super::*;

/// The audit found the fold only ever ran all-undecided: no test
/// produced a victory, so the counters, seat attribution, and the
/// median were one refactor from silently misreporting.
#[test]
fn the_outcome_fold_counts_attributes_and_medians() {
    let m = |seed: u64, ticks: u64, outcome| SweepMatch {
        seed,
        ticks,
        outcome,
    };
    let matches = [
        m(1, 300, SweepOutcome::Victory { seat: 0 }),
        m(2, 100, SweepOutcome::Victory { seat: 0 }),
        m(3, 400, SweepOutcome::Victory { seat: 1 }),
        m(4, 250, SweepOutcome::Draw),
        m(5, 999, SweepOutcome::Undecided),
    ];
    let tally = Tally::of(matches.iter().map(|m| (m.outcome, m.ticks)));
    assert_eq!(tally.victories(), 3);
    assert_eq!(tally.draws, 1);
    assert_eq!(tally.undecided, 1);
    assert_eq!(tally.seat_wins, [2, 1]);
    // Decision ticks sorted: 100, 250, 300, 400 -> median index 2.
    assert_eq!(
        tally.quantile(1, 2),
        Some(300),
        "the draw's tick joins the pool; the cap's does not"
    );
    assert_eq!(Tally::of([]).quantile(1, 2), None);
}

/// Two seeds and a cap far too small to decide: the plumbing must
/// account for every job in seed order.
#[test]
fn sweep_accounts_for_every_job_in_seed_order() {
    let report = run_sweep(
        "skirmish",
        2,
        40,
        7_000,
        oxide_sim::scenario::BotConfig::default(),
    )
    .unwrap();
    assert_eq!(report.matches.len(), 2);
    assert_eq!(report.victories + report.draws + report.undecided, 2);
    assert_eq!(report.matches[0].seed, 7_000);
    assert_eq!(report.matches[1].seed, 7_001);
}

/// The interval brackets the point estimate and never leaves 0..1.
#[test]
fn the_wilson_interval_stays_inside_the_unit_range() {
    for (wins, n) in [(0u32, 1u32), (1, 1), (0, 8), (8, 8), (3, 7), (40, 95)] {
        let [lo, hi] = wilson(wins, n);
        let p = f64::from(wins) / f64::from(n);
        assert!((0.0..=1.0).contains(&lo) && (0.0..=1.0).contains(&hi));
        assert!(lo <= p && p <= hi, "{wins}/{n} -> [{lo}, {hi}]");
    }
}

/// Nearest rank, and both quantiles collapse onto a single sample.
#[test]
fn quantiles_take_the_nearest_rank() {
    assert_eq!(quantile(&[], 1, 2), None);
    assert_eq!(quantile(&[7], 1, 2), Some(7));
    assert_eq!(quantile(&[7], 3, 4), Some(7));
    assert_eq!(quantile(&[1, 2, 3, 4], 1, 2), Some(3));
    assert_eq!(quantile(&[1, 2, 3, 4], 3, 4), Some(4));
}
