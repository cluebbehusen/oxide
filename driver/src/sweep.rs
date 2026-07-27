//! The decisiveness sweep: N seeds of bot-vs-bot on one 1v1 scenario at
//! one ladder level, each seed played twice — once with personalities
//! dealt by the sim, once with the same two values exchanged between
//! the seats. Where `balance-probe` reads what armies were made of,
//! this reads whether games *end*: decided/undecided counts, seat bias
//! that survives the personality exchange, and decision-tick medians.
//! The 0.12 bot phases gate on it.
//!
//! The exchange swaps only the personality knob; each seat keeps its
//! own blunder stream, which is a per-seat variable at any degraded
//! level. A lean that shows up in both orientations is the map or the
//! engine, not personality luck.

use anyhow::{Context, Result};
use oxide_sim::bot::{Level, deal_aggression, seat_bots};
use oxide_sim::scenario::{BotConfig, Scenario};
use oxide_sim::{GameResult, PlayerId, State};
use serde::Serialize;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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
    /// Whether the two seats' dealt personalities were exchanged.
    pub swapped: bool,
    /// The personality each seat actually played, seat-indexed.
    pub aggression: [u32; 2],
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
    /// Ladder level swept.
    pub level: String,
    /// Seeds swept (each played in both orientations).
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
    /// Undecided counts by orientation: [dealt, swapped].
    pub undecided_by_orientation: [u32; 2],
    /// Median decision tick over decided matches.
    pub median_decision_tick: Option<u64>,
    /// Mean personality of winning seats over victories.
    pub mean_winner_aggression: Option<f64>,
    /// Mean personality of losing seats over victories.
    pub mean_loser_aggression: Option<f64>,
    /// Every match, in (seed, orientation) order.
    pub matches: Vec<SweepMatch>,
}

/// Runs the sweep headless and returns the aggregate. Matches fan out
/// across a worker pool pulling from a shared queue, like the map
/// sweeps — every match is an independent deterministic sim.
pub fn run_sweep(
    scenario: &str,
    level: Level,
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

    let jobs: Vec<(u64, bool)> = (0..seeds)
        .flat_map(|offset| [(offset, false), (offset, true)])
        .collect();
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<SweepMatch>> = Mutex::new(Vec::with_capacity(jobs.len()));
    let failure: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(jobs.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(offset, swapped)) = jobs.get(i) else {
                        break;
                    };
                    match play(&base, level, seed_base + offset, swapped, max_ticks) {
                        Ok(m) => {
                            eprintln!(
                                "  seed {} {} · {} ticks · {:?}",
                                m.seed,
                                if m.swapped { "swap" } else { "deal" },
                                m.ticks,
                                m.outcome
                            );
                            results.lock().unwrap().push(m);
                        }
                        Err(err) => {
                            *failure.lock().unwrap() = Some(err);
                            break;
                        }
                    }
                }
            });
        }
    });
    if let Some(err) = failure.into_inner().unwrap() {
        return Err(err);
    }
    let mut matches = results.into_inner().unwrap();
    matches.sort_by_key(|m| (m.seed, m.swapped));

    let mut victories = 0u32;
    let mut draws = 0u32;
    let mut undecided = 0u32;
    let mut seat_wins = [0u32; 2];
    let mut undecided_by_orientation = [0u32; 2];
    let mut decision_ticks: Vec<u64> = Vec::new();
    let mut winner_aggression: Vec<u32> = Vec::new();
    let mut loser_aggression: Vec<u32> = Vec::new();
    for m in &matches {
        match m.outcome {
            SweepOutcome::Victory { seat } => {
                victories += 1;
                seat_wins[seat as usize] += 1;
                decision_ticks.push(m.ticks);
                winner_aggression.push(m.aggression[seat as usize]);
                loser_aggression.push(m.aggression[1 - seat as usize]);
            }
            SweepOutcome::Draw => {
                draws += 1;
                decision_ticks.push(m.ticks);
            }
            SweepOutcome::Undecided => {
                undecided += 1;
                undecided_by_orientation[usize::from(m.swapped)] += 1;
            }
        }
    }
    decision_ticks.sort_unstable();
    let mean = |values: &[u32]| {
        (!values.is_empty())
            .then(|| values.iter().map(|&a| f64::from(a)).sum::<f64>() / values.len() as f64)
    };
    Ok(SweepReport {
        scenario: base.name.clone(),
        level: format!("{level:?}"),
        seeds,
        max_ticks,
        victories,
        draws,
        undecided,
        seat_wins,
        undecided_by_orientation,
        median_decision_tick: (!decision_ticks.is_empty())
            .then(|| decision_ticks[decision_ticks.len() / 2]),
        mean_winner_aggression: mean(&winner_aggression),
        mean_loser_aggression: mean(&loser_aggression),
        matches,
    })
}

/// Runs the sweep, prints the verdict, and optionally lands the raw
/// JSON for the record — the CLI entry.
pub fn sweep_report(
    scenario: &str,
    level: Level,
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
    out: Option<&str>,
) -> Result<()> {
    let report = run_sweep(scenario, level, seeds, max_ticks, seed_base)?;
    println!(
        "\nSEED SWEEP  ·  {}  ·  level {}  ·  {} seeds x 2 orientations  ·  cap {}",
        report.scenario, report.level, report.seeds, report.max_ticks
    );
    println!(
        "decided {} ({} victories, {} draws)  ·  undecided {} (dealt {}, swapped {})",
        report.victories + report.draws,
        report.victories,
        report.draws,
        report.undecided,
        report.undecided_by_orientation[0],
        report.undecided_by_orientation[1],
    );
    println!(
        "seat wins: seat0 {}, seat1 {}",
        report.seat_wins[0], report.seat_wins[1]
    );
    if let Some(median) = report.median_decision_tick {
        println!("median decision tick: {median}");
    }
    if let (Some(winner), Some(loser)) =
        (report.mean_winner_aggression, report.mean_loser_aggression)
    {
        println!("mean aggression: winners {winner:.0}, losers {loser:.0}");
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
    level: Level,
    seed: u64,
    swapped: bool,
    max_ticks: u64,
) -> Result<SweepMatch> {
    // The shipped dealing itself — one definition, no replicated
    // stream: same seed, same personalities as the game deals.
    let dealt = [0u8, 1u8].map(|seat| deal_aggression(seed, PlayerId(seat)));
    let mut sc = base.clone();
    sc.seed = seed;
    for (i, player) in sc.players.iter_mut().enumerate() {
        player.bot = true;
        // The dealt leg passes None so the shipped dealing path runs
        // end-to-end; the swapped leg passes the same two values
        // exchanged.
        let aggression = swapped.then(|| dealt[1 - i]);
        player.bot_config = Some(BotConfig { level, aggression });
    }
    let mut state: State = sc.build().context("building scenario")?;
    let mut bots = seat_bots(&sc);
    for _ in 0..max_ticks {
        crate::runner::step(&mut state, &mut bots, None);
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
        swapped,
        aggression: if swapped { [dealt[1], dealt[0]] } else { dealt },
        ticks: state.current_tick(),
        outcome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two seeds, both orientations, a cap far too small to decide:
    /// the plumbing must account for every job and mirror the
    /// personality pairs exactly.
    #[test]
    fn sweep_accounts_for_every_job_and_mirrors_the_swap() {
        let report = run_sweep("skirmish", Level::Medium, 2, 40, 7_000).unwrap();
        assert_eq!(report.matches.len(), 4);
        assert_eq!(report.victories + report.draws + report.undecided, 4);
        for pair in report.matches.chunks(2) {
            assert_eq!(pair[0].seed, pair[1].seed);
            assert!(!pair[0].swapped && pair[1].swapped);
            assert_eq!(pair[0].aggression[0], pair[1].aggression[1]);
            assert_eq!(pair[0].aggression[1], pair[1].aggression[0]);
        }
    }
}
