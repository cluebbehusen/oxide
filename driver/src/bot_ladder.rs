//! The difficulty ladder: `oxide-opponent` against itself at two rungs.
//!
//! A manifest names duel maps, stances, runs, seed bases, a tick limit and
//! comparisons, each a higher rung, a lower rung and the share of decided
//! legs the higher rung must win. Every map, comparison, stance and run is a
//! pair of legs: the higher rung in seat zero, then in seat one. Both seats
//! share one personality seed, so a leg differs between the seats only in
//! difficulty. A comparison passes once the higher rung's share of decided
//! legs reaches its gate over at least the manifest's number of decided pairs.

use crate::bot_eval::{
    DEFAULT_STALL_LOOP_LIMIT, EvaluationBatchOptions, EvaluationLeg, EvaluationPlan, Termination,
    ensure_unique_execution_plans, evaluate_batch,
};
use crate::evaluation::{
    ManifestMap, MapFamily, MatchMode, REPLAY_INDEX_FILE, distinct, load_scenarios, seat_teams,
    seated_plan,
};
use crate::ledger::{PairShares, SeatLedger, WorthShare, render_shares, team_shares, worth_shares};
use crate::seat_summary::{SeatEvidence, SeatSummary, SeatSummaryBuilder, rank};
use anyhow::{Context, Result, bail, ensure};
use oxide_sim::Scenario;
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// One rung against a lower one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    /// The rung expected to win.
    pub higher: BotDifficulty,
    /// The rung it plays.
    pub lower: BotDifficulty,
    /// Per mille of decided legs the higher rung must win.
    pub gate: u32,
}

/// A ladder definition.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LadderManifest {
    /// Name recorded in every row.
    pub name: String,
    /// Maximum ticks per leg.
    pub tick_limit: u64,
    /// Seed cells per map, comparison and stance.
    pub runs: u64,
    /// Simulation seed of run zero; each run adds one.
    pub scenario_seed_base: u64,
    /// Personality seed of run zero, shared by both seats; each run adds one.
    pub personality_seed_base: u64,
    /// Stances, applied to both seats.
    pub stances: Vec<BotStance>,
    /// Rung pairs to compare.
    pub comparisons: Vec<Comparison>,
    /// Decided pairs a comparison needs before its gate counts.
    pub min_decided_pairs: u32,
    /// Duel maps.
    pub maps: Vec<ManifestMap>,
}

impl LadderManifest {
    /// Reads and validates a manifest.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading ladder manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&text)
            .with_context(|| format!("parsing ladder manifest {}", path.display()))?;
        manifest
            .validate()
            .with_context(|| format!("validating ladder manifest {}", path.display()))?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.name.is_empty() && !self.name.contains(char::is_whitespace),
            "manifest name must be a non-empty word"
        );
        ensure!(self.tick_limit > 0, "tick limit must be positive");
        ensure!(self.runs > 0, "runs must be positive");
        ensure!(
            self.min_decided_pairs > 0,
            "the decided-pair minimum must be positive"
        );
        for base in [self.scenario_seed_base, self.personality_seed_base] {
            ensure!(
                base.checked_add(self.runs - 1).is_some(),
                "seed range overflows u64"
            );
        }
        ensure!(!self.stances.is_empty(), "stances are empty");
        ensure!(!self.comparisons.is_empty(), "comparisons are empty");
        ensure!(!self.maps.is_empty(), "maps are empty");
        ensure!(distinct(&self.stances), "stances repeat a value");
        let rank = |rung: BotDifficulty| BotDifficulty::ALL.iter().position(|each| *each == rung);
        for comparison in &self.comparisons {
            ensure!(
                rank(comparison.higher) > rank(comparison.lower),
                "{} does not rank above {}",
                comparison.higher,
                comparison.lower
            );
            ensure!(
                comparison.gate <= 1_000,
                "a gate is per mille of decided legs, got {}",
                comparison.gate
            );
        }
        let rungs: Vec<(BotDifficulty, BotDifficulty)> = self
            .comparisons
            .iter()
            .map(|comparison| (comparison.higher, comparison.lower))
            .collect();
        ensure!(distinct(&rungs), "comparisons repeat a rung pair");
        let keys: Vec<String> = self.maps.iter().map(ManifestMap::key).collect();
        ensure!(distinct(&keys), "maps repeat a scenario name");
        Ok(())
    }

    /// Loads every map, resolving relative paths against `base`, and checks
    /// that each builds.
    pub fn scenarios(&self, base: &Path) -> Result<Vec<Scenario>> {
        load_scenarios(&self.maps, base)
    }
}

/// Where a row sits in its ladder. Rows carry their comparison's gate, so a
/// report needs nothing but rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LadderLabel {
    /// Manifest name.
    pub manifest: String,
    /// Scenario file stem.
    pub map: String,
    /// Map shape.
    pub family: MapFamily,
    /// The rung expected to win.
    pub higher: BotDifficulty,
    /// The rung it plays.
    pub lower: BotDifficulty,
    /// Per mille of decided legs the higher rung must win.
    pub gate: u32,
    /// Decided pairs the comparison needs before its gate counts.
    pub min_decided_pairs: u32,
    /// Stance of both seats.
    pub stance: BotStance,
    /// Seed cell.
    pub run: u64,
}

/// One leg to evaluate.
#[derive(Debug, Clone)]
pub struct LadderLeg {
    /// Ladder position.
    pub label: LadderLabel,
    /// Exact evaluation plan.
    pub plan: EvaluationPlan,
}

/// Expands a manifest into legs: for each map, comparison, stance and run,
/// the higher rung in seat zero and then in seat one.
pub fn expand(manifest: &LadderManifest, scenarios: &[Scenario]) -> Result<Vec<LadderLeg>> {
    ensure!(
        scenarios.len() == manifest.maps.len(),
        "{} scenarios for {} manifest maps",
        scenarios.len(),
        manifest.maps.len()
    );
    let mut legs = Vec::new();
    for (map, source) in manifest.maps.iter().zip(scenarios) {
        let teams = seat_teams(source).with_context(|| format!("planning {}", map.path))?;
        ensure!(
            MatchMode::of(&teams)? == MatchMode::Duel,
            "{} is not a duel map",
            map.path
        );
        for comparison in &manifest.comparisons {
            for &stance in &manifest.stances {
                for run in 0..manifest.runs {
                    let label = LadderLabel {
                        manifest: manifest.name.clone(),
                        map: map.key(),
                        family: map.family,
                        higher: comparison.higher,
                        lower: comparison.lower,
                        gate: comparison.gate,
                        min_decided_pairs: manifest.min_decided_pairs,
                        stance,
                        run,
                    };
                    let seat = |difficulty| {
                        BotConfig::new(difficulty, stance, manifest.personality_seed_base + run)
                    };
                    let seed = manifest.scenario_seed_base + run;
                    for (leg, rungs) in [
                        (
                            EvaluationLeg::Forward,
                            [comparison.higher, comparison.lower],
                        ),
                        (
                            EvaluationLeg::Swapped,
                            [comparison.lower, comparison.higher],
                        ),
                    ] {
                        legs.push(LadderLeg {
                            label: label.clone(),
                            plan: seated_plan(source, seed, leg, rungs.into_iter().map(seat)),
                        });
                    }
                }
            }
        }
    }
    ensure_unique_execution_plans(legs.iter().map(|leg| &leg.plan))?;
    Ok(legs)
}

/// Execution settings for [`run_ladder`].
pub struct LadderOptions<'a> {
    /// Candidate recorded in every row.
    pub candidate: &'a str,
    /// Upper bound on concurrent matches.
    pub jobs: NonZeroUsize,
    /// Directory for a replay of every leg.
    pub replay_dir: Option<&'a Path>,
}

/// One published row with its ladder position.
#[derive(Debug, Clone, Serialize)]
pub struct LadderRow {
    /// Ladder position.
    pub ladder: LadderLabel,
    /// The evaluation row.
    #[serde(flatten)]
    pub row: serde_json::Value,
}

/// Evaluates every leg and returns their rows in leg order.
pub fn run_ladder(
    legs: &[LadderLeg],
    ticks: u64,
    options: &LadderOptions<'_>,
) -> Result<Vec<LadderRow>> {
    let plans: Vec<(EvaluationPlan, Option<PathBuf>)> = legs
        .iter()
        .enumerate()
        .map(|(index, leg)| {
            let replay = options.replay_dir.map(|dir| {
                let label = &leg.label;
                dir.join(format!(
                    "{index:04}-{}-{}-vs-{}-{}-run{}-{}.json",
                    label.map,
                    label.higher,
                    label.lower,
                    label.stance,
                    label.run,
                    leg.plan.leg.name()
                ))
            });
            (leg.plan.clone(), replay)
        })
        .collect();
    let index = options.replay_dir.map(|dir| dir.join(REPLAY_INDEX_FILE));
    let rows = evaluate_batch(
        &plans,
        &EvaluationBatchOptions {
            ticks,
            stall_loop_limit: Some(DEFAULT_STALL_LOOP_LIMIT),
            candidate: options.candidate,
            jobs: options.jobs,
            output: index.as_deref(),
            trace_output: None,
        },
    )?
    .rows;
    legs.iter()
        .zip(rows)
        .map(|(leg, row)| {
            Ok(LadderRow {
                ladder: leg.label.clone(),
                row: serde_json::to_value(row)?,
            })
        })
        .collect()
}

/// The fields of one ladder row that scoring reads.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoredLadderRow {
    /// Ladder position.
    pub ladder: LadderLabel,
    /// Seat-pair leg.
    pub leg: EvaluationLeg,
    /// Why the leg stopped.
    pub termination: Termination,
    /// Seats on the winning team.
    pub winner_seats: Vec<u8>,
    /// Ticks the leg ran.
    #[serde(default)]
    pub duration_ticks: u64,
    /// Configuration by seat.
    pub seats: Vec<LadderSeat>,
    /// QA evidence by seat.
    #[serde(default)]
    pub evidence: Vec<SeatEvidence>,
}

/// A seat's configured controller.
#[derive(Debug, Clone, Deserialize)]
pub struct LadderSeat {
    /// Player seat.
    pub seat: u8,
    /// The configured controller; its wire form leaves out defaults, which
    /// deserializing restores.
    pub config: Option<BotConfig>,
}

/// Reads ladder rows from JSONL files, in file and line order.
pub fn load_rows(paths: &[PathBuf]) -> Result<Vec<ScoredLadderRow>> {
    let mut rows = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading ladder rows {}", path.display()))?;
        for (line, record) in text.lines().enumerate() {
            if record.trim().is_empty() {
                continue;
            }
            let row: ScoredLadderRow = serde_json::from_str(record).with_context(|| {
                format!("{}:{} is not a bot-ladder row", path.display(), line + 1)
            })?;
            rows.push(row);
        }
    }
    Ok(rows)
}

/// How a comparison stands against its gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Enough decided pairs, and the higher rung's share reaches the gate.
    Pass,
    /// Enough decided pairs, and the share falls short.
    Fail,
    /// Too few decided pairs to judge.
    TooFewPairs,
}

/// Pairs by result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PairTally {
    /// The higher rung won both legs.
    pub higher_both: u32,
    /// Each rung won one leg.
    pub split: u32,
    /// The lower rung won both legs.
    pub lower_both: u32,
    /// At least one leg had no winner.
    pub undecided: u32,
}

impl PairTally {
    /// Pairs whose both legs had a winner.
    pub fn decided(&self) -> u32 {
        self.higher_both + self.split + self.lower_both
    }
}

/// Legs and pairs of one comparison, or of one of its slices.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Tally {
    /// Legs scored.
    pub legs: u32,
    /// Legs with a winner.
    pub decided: u32,
    /// Decided legs the higher rung won.
    pub higher_wins: u32,
    /// The higher rung's share of decided legs, absent with none decided.
    pub share: Option<f64>,
    /// 95% Wilson interval of that share.
    pub wilson: Option<[f64; 2]>,
    /// Pairs by result.
    pub pairs: PairTally,
}

impl Tally {
    fn add(&mut self, winners: [Won; 2]) {
        for winner in winners {
            self.legs += 1;
            if let Some(higher) = winner {
                self.decided += 1;
                self.higher_wins += u32::from(higher);
            }
        }
        match winners {
            [Some(true), Some(true)] => self.pairs.higher_both += 1,
            [Some(false), Some(false)] => self.pairs.lower_both += 1,
            [Some(_), Some(_)] => self.pairs.split += 1,
            _ => self.pairs.undecided += 1,
        }
        if self.decided > 0 {
            self.share = Some(f64::from(self.higher_wins) / f64::from(self.decided));
            self.wilson = Some(wilson(self.higher_wins, self.decided));
        }
    }
}

/// One comparison's results.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComparisonReport {
    /// Manifest whose rows these are; manifests never share a tally, since
    /// two can play the same legs.
    pub manifest: String,
    /// The rung expected to win.
    pub higher: BotDifficulty,
    /// The rung it plays.
    pub lower: BotDifficulty,
    /// Per mille of decided legs the higher rung must win.
    pub gate: u32,
    /// Decided pairs needed before the gate counts.
    pub min_decided_pairs: u32,
    /// How the comparison stands.
    pub verdict: Verdict,
    /// Every leg.
    pub overall: Tally,
    /// By stance, in stance order.
    pub stances: Vec<(BotStance, Tally)>,
    /// By map family, in family order.
    pub families: Vec<(MapFamily, Tally)>,
    /// The higher rung's share of net worth, by pair; empty when the rows
    /// carry no ledgers.
    pub worth: Vec<WorthShare>,
}

/// One rung's seats in one manifest, pooled over every comparison it plays.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RungReport {
    /// Manifest whose rows these are.
    pub manifest: String,
    /// The rung.
    pub rung: BotDifficulty,
    /// Its seats' failure incidents, reactivity, income, ledger and attacks.
    pub summary: SeatSummary,
}

/// Every manifest's comparisons, in the order rows first name them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LadderReport {
    /// Comparisons.
    pub comparisons: Vec<ComparisonReport>,
    /// Each rung's seats, by manifest and then from the lowest rung up.
    pub rungs: Vec<RungReport>,
}

/// Which rung won a leg: `Some(true)` for the higher rung, `Some(false)` for
/// the lower, `None` without a winner.
type Won = Option<bool>;

/// A pair's first row index and its forward and swapped legs, once each.
type PairLegs = (usize, [Option<Won>; 2]);

/// The rung that won `row`'s leg, after checking its seats hold the rungs
/// its leg says.
fn winner(row: &ScoredLadderRow) -> Result<Won> {
    let label = &row.ladder;
    let rungs: Vec<Option<BotDifficulty>> = row
        .seats
        .iter()
        .map(|seat| seat.config.map(|config| config.difficulty))
        .collect();
    let expected = match row.leg {
        EvaluationLeg::Forward => [label.higher, label.lower],
        EvaluationLeg::Swapped => [label.lower, label.higher],
        EvaluationLeg::Single => bail!("a ladder leg is forward or swapped, not single"),
    };
    ensure!(
        row.seats.len() == 2
            && row
                .seats
                .iter()
                .enumerate()
                .all(|(index, seat)| usize::from(seat.seat) == index)
            && rungs == expected.map(Some),
        "a {} leg of {} against {} needs oxide-opponent at {:?} by seat, found {:?}",
        row.leg.name(),
        label.higher,
        label.lower,
        expected,
        rungs
    );
    if row.termination != Termination::Decided {
        return Ok(None);
    }
    let [seat] = row.winner_seats[..] else {
        return Ok(None);
    };
    let rung = rungs.get(usize::from(seat)).with_context(|| {
        format!(
            "a {} leg of {} against {} names winner seat {seat}, which it lacks",
            row.leg.name(),
            label.higher,
            label.lower
        )
    })?;
    Ok(Some(*rung == Some(label.higher)))
}

/// Scores rows into pairs by comparison, refusing incomplete or repeated
/// pairs and rows that disagree on a comparison's gate.
pub fn build_report(rows: &[ScoredLadderRow]) -> Result<LadderReport> {
    type ComparisonKey = (String, String, String);
    type PairKey = (String, String, String, String, String, u64);
    let mut pairs: BTreeMap<PairKey, PairLegs> = BTreeMap::new();
    let mut worth: BTreeMap<ComparisonKey, PairShares> = BTreeMap::new();
    let mut rungs: BTreeMap<(String, usize), (BotDifficulty, SeatSummaryBuilder)> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let label = &row.ladder;
        let ledgers: Vec<Option<&SeatLedger>> = row
            .evidence
            .iter()
            .map(|evidence| evidence.ledger.as_ref())
            .collect();
        let higher_seat = match row.leg {
            EvaluationLeg::Swapped => 1,
            _ => 0,
        };
        if ledgers.len() == row.seats.len()
            && let Some(shares) = team_shares(&ledgers, &[0, 1], higher_seat)
        {
            worth
                .entry((
                    label.manifest.clone(),
                    label.higher.to_string(),
                    label.lower.to_string(),
                ))
                .or_default()
                .entry(format!("{} {} {}", label.map, label.stance, label.run))
                .or_default()
                .push(shares);
        }
        for (seat, evidence) in row.seats.iter().zip(&row.evidence) {
            let Some(config) = seat.config else {
                continue;
            };
            rungs
                .entry((label.manifest.clone(), rank(config.difficulty)))
                .or_insert_with(|| (config.difficulty, SeatSummaryBuilder::default()))
                .1
                .add(evidence, row.duration_ticks);
        }
        let key = (
            label.manifest.clone(),
            label.map.clone(),
            label.higher.to_string(),
            label.lower.to_string(),
            label.stance.to_string(),
            label.run,
        );
        let slot = match row.leg {
            EvaluationLeg::Forward => 0,
            _ => 1,
        };
        let winner = winner(row)?;
        let entry = pairs.entry(key).or_insert((index, [None, None]));
        ensure!(
            entry.1[slot].is_none(),
            "a {} leg repeats in {} {} against {}, {} run {}",
            row.leg.name(),
            label.map,
            label.higher,
            label.lower,
            label.stance,
            label.run
        );
        entry.1[slot] = Some(winner);
    }
    let mut order: Vec<&PairLegs> = pairs.values().collect();
    order.sort_by_key(|(index, _)| *index);
    let mut comparisons: Vec<ComparisonReport> = Vec::new();
    for (index, legs) in order {
        let label = &rows[*index].ladder;
        let [Some(forward), Some(swapped)] = *legs else {
            bail!(
                "{} {} against {}, {} run {} lacks one of its two legs",
                label.map,
                label.higher,
                label.lower,
                label.stance,
                label.run
            );
        };
        let position = if let Some(position) = comparisons.iter().position(|each| {
            each.manifest == label.manifest
                && each.higher == label.higher
                && each.lower == label.lower
        }) {
            let each = &comparisons[position];
            ensure!(
                each.gate == label.gate && each.min_decided_pairs == label.min_decided_pairs,
                "{} rows of {} against {} disagree on the gate",
                label.manifest,
                label.higher,
                label.lower
            );
            position
        } else {
            comparisons.push(ComparisonReport {
                manifest: label.manifest.clone(),
                higher: label.higher,
                lower: label.lower,
                gate: label.gate,
                min_decided_pairs: label.min_decided_pairs,
                verdict: Verdict::TooFewPairs,
                overall: Tally::default(),
                stances: Vec::new(),
                families: Vec::new(),
                worth: Vec::new(),
            });
            comparisons.len() - 1
        };
        let report = &mut comparisons[position];
        let winners = [forward, swapped];
        report.overall.add(winners);
        slice(&mut report.stances, label.stance).add(winners);
        slice(&mut report.families, label.family).add(winners);
    }
    for report in &mut comparisons {
        report
            .stances
            .sort_by_key(|(stance, _)| BotStance::ALL.iter().position(|each| each == stance));
        report.families.sort_by_key(|(family, _)| *family);
        if let Some(pairs) = worth.get(&(
            report.manifest.clone(),
            report.higher.to_string(),
            report.lower.to_string(),
        )) {
            report.worth = worth_shares(pairs);
        }
        let tally = &report.overall;
        report.verdict = if tally.pairs.decided() < report.min_decided_pairs.max(1) {
            Verdict::TooFewPairs
        } else if u64::from(tally.higher_wins) * 1_000
            >= u64::from(report.gate) * u64::from(tally.decided)
        {
            Verdict::Pass
        } else {
            Verdict::Fail
        };
    }
    Ok(LadderReport {
        comparisons,
        rungs: rungs
            .into_iter()
            .map(|((manifest, _), (rung, builder))| RungReport {
                manifest,
                rung,
                summary: builder.finish(),
            })
            .collect(),
    })
}

/// The tally for `key`, added on first use.
fn slice<K: PartialEq>(slices: &mut Vec<(K, Tally)>, key: K) -> &mut Tally {
    let index = if let Some(index) = slices.iter().position(|(each, _)| *each == key) {
        index
    } else {
        slices.push((key, Tally::default()));
        slices.len() - 1
    };
    &mut slices[index].1
}

impl LadderReport {
    /// The report as text.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for report in &self.comparisons {
            let verdict = match report.verdict {
                Verdict::Pass => "pass",
                Verdict::Fail => "fail",
                Verdict::TooFewPairs => "too few decided pairs",
            };
            let _ = writeln!(
                out,
                "{}: {} against {}: {verdict} (gate {}% of decided legs over {} decided pairs)",
                report.manifest,
                report.higher,
                report.lower,
                (report.gate + 5) / 10,
                report.min_decided_pairs
            );
            line(&mut out, "overall", &report.overall, report);
            for (stance, tally) in &report.stances {
                line(&mut out, &format!("stance {stance}"), tally, report);
            }
            for (family, tally) in &report.families {
                let name = serde_json::to_value(family)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default();
                line(&mut out, &format!("family {name}"), tally, report);
            }
            if !report.worth.is_empty() {
                let _ = writeln!(
                    out,
                    "  {} share of net worth, by pair: {}",
                    report.higher,
                    render_shares(&report.worth)
                );
            }
            out.push('\n');
        }
        let mut manifests: Vec<&str> = Vec::new();
        for rung in &self.rungs {
            if !manifests.contains(&rung.manifest.as_str()) {
                manifests.push(&rung.manifest);
            }
        }
        for manifest in manifests {
            let _ = writeln!(out, "{manifest}: seats by rung");
            let groups: Vec<(String, &SeatSummary)> = self
                .rungs
                .iter()
                .filter(|rung| rung.manifest == manifest)
                .map(|rung| (rung.rung.to_string(), &rung.summary))
                .collect();
            crate::seat_summary::render(&mut out, &groups);
            out.push('\n');
        }
        out
    }
}

fn line(out: &mut String, group: &str, tally: &Tally, report: &ComparisonReport) {
    let share = match (tally.share, tally.wilson) {
        (Some(share), Some([low, high])) => format!(
            "{}/{} {:.0}% [{:.0}-{:.0}%]",
            tally.higher_wins,
            tally.decided,
            share * 100.0,
            low * 100.0,
            high * 100.0
        ),
        _ => "no decided legs".to_owned(),
    };
    let pairs = &tally.pairs;
    let _ = writeln!(
        out,
        "  {group:<22} {share:<24} pairs {} both {}, split {}, {} both {}, undecided {} ({} decided); {}/{} legs decided",
        report.higher,
        pairs.higher_both,
        pairs.split,
        report.lower,
        pairs.lower_both,
        pairs.undecided,
        pairs.decided(),
        tally.decided,
        tally.legs
    );
}

/// The 95% Wilson score interval for `wins` of `n`, which stays inside
/// 0..1 and remains accurate at small counts.
fn wilson(wins: u32, n: u32) -> [f64; 2] {
    const Z: f64 = 1.959_963_984_540_054;
    let n = f64::from(n);
    let p = f64::from(wins) / n;
    let denominator = 1.0 + Z * Z / n;
    let centre = (p + Z * Z / (2.0 * n)) / denominator;
    let half = Z / denominator * (p * (1.0 - p) / n + Z * Z / (4.0 * n * n)).sqrt();
    [(centre - half).max(0.0), (centre + half).min(1.0)]
}

#[cfg(test)]
mod tests;
