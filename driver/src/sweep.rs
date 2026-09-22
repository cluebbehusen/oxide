//! The decisiveness sweep: N seeds of configured-bot mirror on one 1v1
//! scenario. It reads whether games *end*: decided/undecided counts,
//! seat lean, and decision-tick medians.
//!
//! Both seats use the same exact current-controller profile. Results measure
//! that configured bot interacting with the simulation and map; symmetric
//! seating does not isolate engine or map fairness.

use anyhow::{Context, Result};
use oxide_bot::seat_bots;
use oxide_sim::scenario::Scenario;
use oxide_sim::{GameResult, State};
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
    /// Exact shared controller profile for this measurement.
    #[serde(serialize_with = "crate::sweep::serialize_bot_config")]
    pub bot_config: oxide_sim::scenario::BotConfig,
    /// Simulation rules used by this measurement.
    pub sim_version: String,
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
    config: oxide_sim::scenario::BotConfig,
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
        let m = play(&base, seed_base + offset, max_ticks, config)?;
        eprintln!("  seed {} · {} ticks · {:?}", m.seed, m.ticks, m.outcome);
        Ok(m)
    })?;

    let tally = Tally::of(matches.iter().map(|m| (m.outcome, m.ticks)));
    Ok(SweepReport {
        bot_config: config,
        sim_version: oxide_sim::SIM_VERSION.to_string(),
        scenario: base.name,
        seeds,
        max_ticks,
        victories: tally.victories(),
        draws: tally.draws,
        undecided: tally.undecided,
        seat_wins: tally.seat_wins,
        median_decision_tick: tally.quantile(1, 2),
        matches,
    })
}

/// The fold every measurement record is read off. Draws count as decisions
/// for the tick pool, since a mutual Foundry death decided the game on that
/// tick, while undecided caps stay out of it.
pub(crate) struct Tally {
    pub(crate) matches: u32,
    pub(crate) seat_wins: [u32; 2],
    pub(crate) draws: u32,
    pub(crate) undecided: u32,
    decision_ticks: Vec<u64>,
}

impl Tally {
    pub(crate) fn of(played: impl IntoIterator<Item = (SweepOutcome, u64)>) -> Self {
        let mut tally = Tally {
            matches: 0,
            seat_wins: [0; 2],
            draws: 0,
            undecided: 0,
            decision_ticks: Vec::new(),
        };
        for (outcome, ticks) in played {
            tally.matches += 1;
            match outcome {
                SweepOutcome::Victory { seat } => {
                    tally.seat_wins[usize::from(seat)] += 1;
                    tally.decision_ticks.push(ticks);
                }
                SweepOutcome::Draw => {
                    tally.draws += 1;
                    tally.decision_ticks.push(ticks);
                }
                SweepOutcome::Undecided => tally.undecided += 1,
            }
        }
        tally.decision_ticks.sort_unstable();
        tally
    }

    pub(crate) fn victories(&self) -> u32 {
        self.seat_wins[0] + self.seat_wins[1]
    }

    /// Undecided share, in percent.
    pub(crate) fn censored_percent(&self) -> f64 {
        if self.matches == 0 {
            0.0
        } else {
            100.0 * f64::from(self.undecided) / f64::from(self.matches)
        }
    }

    /// Nearest-rank quantile of the decision ticks.
    pub(crate) fn quantile(&self, num: usize, den: usize) -> Option<u64> {
        quantile(&self.decision_ticks, num, den)
    }
}

/// Runs the sweep, prints the verdict, and optionally lands the raw
/// JSON for the record — the CLI entry.
pub fn sweep_report(
    scenario: &str,
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
    out: Option<&str>,
    config: oxide_sim::scenario::BotConfig,
) -> Result<()> {
    let report = run_sweep(scenario, seeds, max_ticks, seed_base, config)?;
    println!("controller: {config:?}; sim {}", oxide_sim::SIM_VERSION);
    println!(
        "\nSEED SWEEP  ·  {}  ·  current controller both seats  ·  {} seeds  ·  cap {}",
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
fn play(
    base: &Scenario,
    seed: u64,
    max_ticks: u64,
    config: oxide_sim::scenario::BotConfig,
) -> Result<SweepMatch> {
    let mut sc = base.clone();
    sc.seed = seed;
    let state = play_mirror(sc, config, max_ticks, [0, 1])?;
    Ok(SweepMatch {
        seed,
        ticks: state.current_tick(),
        outcome: outcome_of(&state),
    })
}

/// Seats the configured bot in both seats of a 1v1 and steps to the decision
/// or the cap. `command_order` is the seat order commands are staged in each
/// tick.
pub(crate) fn play_mirror(
    mut scenario: Scenario,
    config: oxide_sim::scenario::BotConfig,
    max_ticks: u64,
    command_order: [usize; 2],
) -> Result<State> {
    oxide_kit::bench::all_bots_with_config(&mut scenario, config);
    let mut state = scenario.build().context("building scenario")?;
    let mut bots = seat_bots(&scenario)?;
    for _ in 0..max_ticks {
        let mut commands = Vec::new();
        for seat in command_order {
            commands.extend(bots[seat].act(&state));
        }
        state.tick(&commands);
        if state.result().is_some() {
            break;
        }
    }
    Ok(state)
}

/// How a finished 1v1 mirror ended.
pub(crate) fn outcome_of(state: &State) -> SweepOutcome {
    match state.result() {
        Some(GameResult::Victory { .. }) => SweepOutcome::Victory {
            seat: state
                .winners()
                .first()
                .expect("a 1v1 victory names its seat")
                .0,
        },
        Some(GameResult::Draw) => SweepOutcome::Draw,
        None => SweepOutcome::Undecided,
    }
}

/// Nearest-rank quantile over an already-sorted series.
pub(crate) fn quantile(sorted: &[u64], num: usize, den: usize) -> Option<u64> {
    (!sorted.is_empty()).then(|| sorted[(sorted.len() * num / den).min(sorted.len() - 1)])
}

pub(crate) fn serialize_bot_config<S: serde::Serializer>(
    config: &oxide_sim::scenario::BotConfig,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeStruct;
    let mut record = serializer.serialize_struct("BotConfig", 3)?;
    record.serialize_field("difficulty", &config.difficulty)?;
    record.serialize_field("stance", &config.stance)?;
    record.serialize_field("personality_seed", &config.personality_seed)?;
    record.end()
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
