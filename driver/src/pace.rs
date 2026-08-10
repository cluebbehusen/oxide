//! The pace sweep: `driver sweep`'s decisiveness reading fanned out over
//! every 1v1 shipped map, tabled beside the `pace` label the map
//! declares and the ground route the audit measures.
//!
//! `pace` is a *geometric* label — `map-audit` gates it on Foundry-to-
//! Foundry route length, and nothing else in the tree measures how long
//! a match actually takes. This instrument measures it: per map,
//! decided against censored, and the decision-tick quartiles in ticks
//! *and* in wall clock, which is the unit a player feels. Label and
//! clock are only loosely correlated; keeping both in one table is the
//! point.
//!
//! Every row is exactly what `driver sweep --scenario <map>` reports at
//! the same dials, so a surprising row is reproducible with one
//! command. Maps therefore run one after another, each fanning its own
//! matches across the pool, rather than pooling every map's matches
//! into a single queue — a row that answers to a single command is
//! worth more here than the packing.
//!
//! Measurement only: the medians move with every artifact generation
//! and every balance bless, so nothing gates on them. The one thing
//! authored from this output is `ScenarioMeta.duration` — the browser's
//! p25-p75 band, an artifact-stamped measurement re-stamped after any
//! bless that moves the clock, never a gate.

use crate::sweep::{SweepOutcome, SweepReport, quantile, run_sweep};
use anyhow::{Context, Result};
use oxide_sim::scenario::Scenario;
use serde::Serialize;

/// A decision-tick figure in both units the instrument reports: the
/// sim's own count, and the clock a player reads.
#[derive(Debug, Clone, Serialize)]
pub struct Elapsed {
    /// Ticks.
    pub ticks: u64,
    /// The same span as `m:ss` at the sim's fixed tick rate.
    pub clock: String,
}

impl Elapsed {
    fn new(ticks: u64) -> Self {
        let secs = ticks / u64::from(oxide_sim::TICKS_PER_SECOND);
        Elapsed {
            ticks,
            clock: format!("{}:{:02}", secs / 60, secs % 60),
        }
    }

    /// `<ticks> (<m:ss>)` — the table cell.
    fn cell(&self) -> String {
        format!("{} ({})", self.ticks, self.clock)
    }
}

/// One map's measured duration, beside what the map claims about
/// itself.
#[derive(Debug, Clone, Serialize)]
pub struct PaceRow {
    /// Scenario name.
    pub scenario: String,
    /// The path the row was swept from.
    pub path: String,
    /// The `pace` label the map declares (empty when it carries no
    /// metadata).
    pub pace: String,
    /// Shortest Foundry-to-Foundry ground route in the audit's weighted
    /// tile-equivalents — the figure the pace bands are gated on.
    pub ground_route: Option<usize>,
    /// Matches played (one per seed).
    pub matches: u32,
    /// Matches that reached a victory or a mutual-death draw.
    pub decided: u32,
    /// Matches the tick cap ended.
    pub undecided: u32,
    /// Undecided share, in percent — the censoring the quantiles below
    /// are blind to.
    pub censored_percent: f64,
    /// First-quartile decision tick, over decided matches only.
    pub p25: Option<Elapsed>,
    /// Median decision tick, over decided matches only.
    pub median: Option<Elapsed>,
    /// Third-quartile decision tick — the grind tail a median hides.
    pub p75: Option<Elapsed>,
    /// The full sweep the row folds, for the record.
    pub sweep: SweepReport,
}

/// The pace sweep's verdict over a scenario directory.
#[derive(Debug, Clone, Serialize)]
pub struct PaceSlate {
    /// The scenario directory swept.
    pub dir: String,
    /// Seeds per map (one Overseer-vs-Overseer match per seed).
    pub seeds: u64,
    /// Tick cap per match.
    pub max_ticks: u64,
    /// Per-map rows, in path order.
    pub per_map: Vec<PaceRow>,
    /// Matches played over the slate.
    pub matches: u32,
    /// Matches decided over the slate.
    pub decided: u32,
    /// Matches the cap ended over the slate.
    pub undecided: u32,
}

/// Sweeps every 1v1 scenario in `dir` and folds each into a row. Maps
/// of any other format are skipped, not refused — the shipped directory
/// mixes formats and the sweep reads 1v1 decisiveness.
pub fn run_pace_sweep(dir: &str, seeds: u64, max_ticks: u64, seed_base: u64) -> Result<PaceSlate> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {dir}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();
    anyhow::ensure!(!paths.is_empty(), "no scenarios under {dir}");

    let mut maps: Vec<(String, Scenario)> = Vec::new();
    for path in &paths {
        let path = path.to_string_lossy().into_owned();
        let scenario = crate::runner::load_scenario(&path)?;
        if scenario.players.len() == 2 {
            maps.push((path, scenario));
        } else {
            eprintln!(
                "  skipping {} ({} seats)",
                scenario.name,
                scenario.players.len()
            );
        }
    }
    anyhow::ensure!(!maps.is_empty(), "no 1v1 scenarios under {dir}");

    let mut per_map = Vec::with_capacity(maps.len());
    for (path, scenario) in &maps {
        eprintln!("\n{}:", scenario.name);
        let sweep = run_sweep(path, seeds, max_ticks, seed_base)?;
        per_map.push(row(path, scenario, sweep)?);
    }
    Ok(PaceSlate {
        dir: dir.to_string(),
        seeds,
        max_ticks,
        matches: per_map.iter().map(|r| r.matches).sum(),
        decided: per_map.iter().map(|r| r.decided).sum(),
        undecided: per_map.iter().map(|r| r.undecided).sum(),
        per_map,
    })
}

/// Folds one map's sweep into its row. The quantiles read the sweep's
/// own match list rather than a second simulation, so the row can never
/// disagree with the record it carries.
fn row(path: &str, scenario: &Scenario, sweep: SweepReport) -> Result<PaceRow> {
    let mut decided_ticks: Vec<u64> = sweep
        .matches
        .iter()
        .filter(|m| !matches!(m.outcome, SweepOutcome::Undecided))
        .map(|m| m.ticks)
        .collect();
    decided_ticks.sort_unstable();
    let matches = sweep.matches.len() as u32;
    let decided = decided_ticks.len() as u32;
    let undecided = matches - decided;
    let ground_route = crate::audit::audit(scenario)
        .with_context(|| format!("auditing {}", scenario.name))?
        .routes
        .iter()
        .filter_map(|r| r.ground_steps)
        .min();
    Ok(PaceRow {
        scenario: scenario.name.clone(),
        path: path.to_string(),
        pace: scenario
            .meta
            .as_ref()
            .map_or_else(String::new, |m| m.pace.clone()),
        ground_route,
        matches,
        decided,
        undecided,
        censored_percent: f64::from(undecided) * 100.0 / f64::from(matches),
        p25: quantile(&decided_ticks, 1, 4).map(Elapsed::new),
        median: quantile(&decided_ticks, 1, 2).map(Elapsed::new),
        p75: quantile(&decided_ticks, 3, 4).map(Elapsed::new),
        sweep,
    })
}

/// Runs the pace sweep, prints the table, and optionally lands the raw
/// JSON for the record — the CLI entry.
pub fn pace_sweep_report(
    dir: &str,
    seeds: u64,
    max_ticks: u64,
    seed_base: u64,
    out: Option<&str>,
) -> Result<()> {
    let slate = run_pace_sweep(dir, seeds, max_ticks, seed_base)?;
    println!(
        "\nPACE SWEEP  ·  {}  ·  {} 1v1 maps  ·  Overseer both seats  ·  {} seeds  ·  cap {}",
        slate.dir,
        slate.per_map.len(),
        slate.seeds,
        slate.max_ticks
    );
    println!(
        "{:<24} {:<9} {:>6}  {:>7} {:>7}  {:>14} {:>14} {:>14}",
        "map", "pace", "route", "decided", "cens", "p25", "median", "p75"
    );
    let cell = |v: &Option<Elapsed>| v.as_ref().map_or_else(|| "-".to_string(), Elapsed::cell);
    for r in &slate.per_map {
        println!(
            "{:<24} {:<9} {:>6}  {:>7} {:>6.1}%  {:>14} {:>14} {:>14}",
            r.scenario,
            if r.pace.is_empty() { "-" } else { &r.pace },
            r.ground_route
                .map_or_else(|| "-".to_string(), |s| s.to_string()),
            format!("{}/{}", r.decided, r.matches),
            r.censored_percent,
            cell(&r.p25),
            cell(&r.median),
            cell(&r.p75),
        );
    }
    println!(
        "\nslate: {}/{} decided ({} censored by the {}-tick cap)",
        slate.decided, slate.matches, slate.undecided, slate.max_ticks
    );
    println!("quantiles cover decided matches only; read them against the censored column");
    if let Some(path) = out {
        std::fs::write(path, serde_json::to_string_pretty(&slate)?)?;
        println!("raw record: {path}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sweep::SweepMatch;

    /// A cap far too small to decide anything: every 1v1 map of the
    /// shipped directory must still produce a row, fully censored, with
    /// no quantile inventing a duration out of nothing — and each row
    /// must carry the two things the measurement is read against.
    #[test]
    fn the_slate_rows_every_duel_map_and_admits_full_censoring() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../scenarios");
        let slate = run_pace_sweep(dir, 1, 20, 3_000).unwrap();
        assert!(slate.per_map.len() >= 10, "the 1v1 roster is present");
        assert_eq!(slate.decided, 0);
        assert_eq!(slate.undecided, slate.matches);
        assert_eq!(slate.matches, slate.per_map.len() as u32);
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
                "{}: shipped maps are connected by ground",
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
}
