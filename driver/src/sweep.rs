//! The decisiveness sweep: N seeds of Overseer-vs-Overseer on one 1v1
//! scenario. It reads whether games *end*: decided/undecided counts,
//! seat lean, and decision-tick medians.
//!
//! Both seats play [`Brain::overseer`], the stable scripted QA anchor,
//! so any lean the sweep reports is the map or the engine: the two
//! command sources are the same commander. This instrument measures
//! the world rather than the player-facing bot.

use anyhow::{Context, Result};
use oxide_sim::bot::Brain;
use oxide_sim::scenario::Scenario;
use oxide_sim::{GameResult, PlayerId, State};
use serde::Serialize;

/// How one sweep match ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SweepOutcome {
    /// A team won; `seat` is its sole (1v1) seat.
    Victory {
        /// The winning seat.
        seat: u8,
    },
    /// Mutual Foundry death on one tick.
    Draw,
    /// The tick cap arrived first.
    Undecided,
}

/// One match of the sweep.
#[derive(Debug, Clone, Serialize)]
pub struct SweepMatch {
    /// Scenario seed this match ran under.
    pub seed: u64,
    /// Final tick: the decision tick, or the cap.
    pub ticks: u64,
    /// How it ended.
    pub outcome: SweepOutcome,
}

/// The sweep's aggregate verdict.
#[derive(Debug, Clone, Serialize)]
pub struct SweepReport {
    /// Scenario name.
    pub scenario: String,
    /// Seeds swept (one match per seed).
    pub seeds: u64,
    /// Tick cap per match.
    pub max_ticks: u64,
    /// Matches ending in a victory.
    pub victories: u32,
    /// Mutual-death draws.
    pub draws: u32,
    /// Matches that hit the cap.
    pub undecided: u32,
    /// Victories by seat over all matches.
    pub seat_wins: [u32; 2],
    /// Median decision tick over decided matches.
    pub median_decision_tick: Option<u64>,
    /// Every match, in seed order.
    pub matches: Vec<SweepMatch>,
}

/// Runs the sweep headless and returns the aggregate. Matches fan out
/// across a worker pool pulling from a shared queue, like the map
/// sweeps — every match is an independent deterministic sim.
pub fn run_sweep(
    scenario: &str,
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
) -> Result<SweepReport> {
    let base = crate::runner::load_scenario(scenario)?;
    anyhow::ensure!(
        base.players.len() == 2,
        "the sweep reads 1v1 decisiveness; {} has {} seats",
        base.name,
        base.players.len()
    );

    let jobs: Vec<u64> = (0..seeds).collect();
    // The pool returns results in job order, so the record is ordered
    // by seed without a second sort.
    let matches = crate::pool::fan_out(&jobs, |&offset| {
        let m = play(&base, seed_base + offset, max_ticks)?;
        eprintln!("  seed {} · {} ticks · {:?}", m.seed, m.ticks, m.outcome);
        Ok(m)
    })?;

    let (victories, draws, undecided, seat_wins, median_decision_tick) = tally_outcomes(&matches);
    Ok(SweepReport {
        scenario: base.name,
        seeds,
        max_ticks,
        victories,
        draws,
        undecided,
        seat_wins,
        median_decision_tick,
        matches,
    })
}

/// Folds match outcomes into the report counters. Draws count as
/// decisions for the median — a mutual Foundry death decided the game
/// on that tick — while undecided caps stay out of the tick pool.
fn tally_outcomes(matches: &[SweepMatch]) -> (u32, u32, u32, [u32; 2], Option<u64>) {
    let mut victories = 0u32;
    let mut draws = 0u32;
    let mut undecided = 0u32;
    let mut seat_wins = [0u32; 2];
    let mut decision_ticks: Vec<u64> = Vec::new();
    for m in matches {
        match m.outcome {
            SweepOutcome::Victory { seat } => {
                victories += 1;
                seat_wins[seat as usize] += 1;
                decision_ticks.push(m.ticks);
            }
            SweepOutcome::Draw => {
                draws += 1;
                decision_ticks.push(m.ticks);
            }
            SweepOutcome::Undecided => undecided += 1,
        }
    }
    decision_ticks.sort_unstable();
    let median = (!decision_ticks.is_empty()).then(|| decision_ticks[decision_ticks.len() / 2]);
    (victories, draws, undecided, seat_wins, median)
}

/// Runs the sweep, prints the verdict, and optionally lands the raw
/// JSON for the record — the CLI entry.
pub fn sweep_report(
    scenario: &str,
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
    out: Option<&str>,
) -> Result<()> {
    let report = run_sweep(scenario, seeds, max_ticks, seed_base)?;
    println!(
        "\nSEED SWEEP  ·  {}  ·  Overseer both seats  ·  {} seeds  ·  cap {}",
        report.scenario, report.seeds, report.max_ticks
    );
    println!(
        "decided {} ({} victories, {} draws)  ·  undecided {}",
        report.victories + report.draws,
        report.victories,
        report.draws,
        report.undecided,
    );
    println!(
        "seat wins: seat0 {}, seat1 {}",
        report.seat_wins[0], report.seat_wins[1]
    );
    if let Some(median) = report.median_decision_tick {
        println!("median decision tick: {median}");
    }
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("raw record: {path}");
    }
    Ok(())
}

/// Plays one match: build, think, step, stop at the decision or the cap.
fn play(base: &Scenario, seed: u64, max_ticks: u64) -> Result<SweepMatch> {
    let mut sc = base.clone();
    sc.seed = seed;
    let mut state: State = sc.build().context("building scenario")?;
    let mut bots: Vec<Brain> = (0..2u8)
        .map(|seat| Brain::overseer(PlayerId(seat), seed))
        .collect();
    for _ in 0..max_ticks {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        state.tick(&commands);
        if state.result().is_some() {
            break;
        }
    }
    let outcome = match state.result() {
        Some(GameResult::Victory { .. }) => SweepOutcome::Victory {
            seat: state
                .winners()
                .first()
                .expect("a 1v1 victory names its seat")
                .0,
        },
        Some(GameResult::Draw) => SweepOutcome::Draw,
        None => SweepOutcome::Undecided,
    };
    Ok(SweepMatch {
        seed,
        ticks: state.current_tick(),
        outcome,
    })
}

/// Nearest-rank quantile over an already-sorted series.
pub(crate) fn quantile(sorted: &[u64], num: usize, den: usize) -> Option<u64> {
    (!sorted.is_empty()).then(|| sorted[(sorted.len() * num / den).min(sorted.len() - 1)])
}

#[cfg(test)]
mod tests {
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
        let matches = vec![
            m(1, 300, SweepOutcome::Victory { seat: 0 }),
            m(2, 100, SweepOutcome::Victory { seat: 0 }),
            m(3, 400, SweepOutcome::Victory { seat: 1 }),
            m(4, 250, SweepOutcome::Draw),
            m(5, 999, SweepOutcome::Undecided),
        ];
        let (victories, draws, undecided, seat_wins, median) = tally_outcomes(&matches);
        assert_eq!(victories, 3);
        assert_eq!(draws, 1);
        assert_eq!(undecided, 1);
        assert_eq!(seat_wins, [2, 1]);
        // Decision ticks sorted: 100, 250, 300, 400 -> median index 2.
        assert_eq!(
            median,
            Some(300),
            "the draw's tick joins the pool; the cap's does not"
        );
        assert_eq!(tally_outcomes(&[]).4, None);
    }

    /// Two seeds and a cap far too small to decide: the plumbing must
    /// account for every job in seed order.
    #[test]
    fn sweep_accounts_for_every_job_in_seed_order() {
        let report = run_sweep("skirmish", 2, 40, 7_000).unwrap();
        assert_eq!(report.matches.len(), 2);
        assert_eq!(report.victories + report.draws + report.undecided, 2);
        assert_eq!(report.matches[0].seed, 7_000);
        assert_eq!(report.matches[1].seed, 7_001);
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
}
