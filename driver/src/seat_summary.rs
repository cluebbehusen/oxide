//! Seat evidence pooled over many legs: failure incidents, deliveries,
//! reactivity, income, the impact ledger and attack calibration, with their
//! text tables. The ladder pools it by rung; `bot-summary` pools any
//! evaluation rows by match mode and difficulty.

use crate::bot_eval::{
    AttackCalibration, Deliveries, INCOME_CHECKPOINTS, IncomeSample, SeatFailures, SeatReactivity,
};
use crate::evaluation::MatchMode;
use crate::ledger::{LedgerPool, SeatLedger};
use anyhow::{Context, Result, ensure};
use oxide_sim::scenario::{BotConfig, BotDifficulty};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Unit and building kinds each ledger table shows.
pub const LEDGER_KINDS: usize = 8;

/// The fields of a seat's evidence a summary reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SeatEvidence {
    /// Tick the seat resigned or lost its last Foundry; absent while it stood.
    #[serde(default)]
    pub eliminated_at: Option<u64>,
    /// Detected failure episodes.
    #[serde(default)]
    pub failures: SeatFailures,
    /// Armed ground units trained on severed ground, by outcome; absent from
    /// rows recorded before the diagnostic existed.
    #[serde(default)]
    pub deliveries: Option<Deliveries>,
    /// Income checkpoints reached.
    #[serde(default)]
    pub income: Vec<IncomeSample>,
    /// Situations met and how they were answered; absent from rows recorded
    /// before the detectors existed.
    #[serde(default)]
    pub reactivity: Option<SeatReactivity>,
    /// What the seat's units and buildings did; absent from rows recorded
    /// before the ledger existed.
    #[serde(default)]
    pub ledger: Option<SeatLedger>,
    /// What it believed when it launched each attack, and how they went.
    #[serde(default)]
    pub attacks: Option<AttackCalibration>,
}

/// Median income at one checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IncomeMedian {
    /// Checkpoint tick.
    pub tick: u64,
    /// Seat samples at the checkpoint.
    pub samples: u32,
    /// Median actual scrap per minute.
    pub actual_per_minute: u32,
    /// Median saturation estimate per minute.
    pub saturation_per_minute: u32,
    /// Median of each sample's actual over saturation, in percent.
    pub percent_of_saturation: Option<u32>,
}

/// Evidence of the seats one group played, pooled over legs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeatSummary {
    /// Seats pooled, summed over legs.
    pub seat_legs: u32,
    /// Repeated impossible-order episodes.
    pub repeated_orders: u64,
    /// Abandoned paid-construction episodes.
    pub abandoned_sites: u64,
    /// Seat-level production-starvation episodes.
    pub starved_production: u64,
    /// Missions stuck in one phase past its timeout.
    pub stuck_missions: u64,
    /// Idle-army episodes over the seat-legs whose rows record the detector.
    pub idle_army: u64,
    /// Seat-legs whose rows record the idle-army detector.
    pub idle_army_seat_legs: u32,
    /// Armed ground units trained on severed ground, by outcome, over the
    /// seat-legs whose rows record the diagnostic.
    pub deliveries: Deliveries,
    /// Seat-legs whose rows record deliveries.
    pub deliveries_seat_legs: u32,
    /// Situations met and how they were answered, summed over the seat-legs
    /// whose rows record them; absent when none do.
    pub reactivity: Option<SeatReactivity>,
    /// Seat-legs whose rows record reactivity.
    pub reactivity_seat_legs: u32,
    /// Ticks those seat-legs played while their seat stood, for rates.
    pub reactivity_ticks: u64,
    /// Income medians by checkpoint.
    pub income: Vec<IncomeMedian>,
    /// Pooled impact ledgers over the seat-legs whose rows record one.
    pub ledger: LedgerPool,
    /// Attack calibration summed over the seat-legs whose rows record it;
    /// absent when none do.
    pub attacks: Option<AttackCalibration>,
}

/// Pools seat evidence into a [`SeatSummary`].
#[derive(Debug, Default)]
pub struct SeatSummaryBuilder {
    seat_legs: u32,
    repeated_orders: u64,
    abandoned_sites: u64,
    starved_production: u64,
    stuck_missions: u64,
    idle_army: u64,
    idle_army_seat_legs: u32,
    deliveries: Deliveries,
    deliveries_seat_legs: u32,
    reactivity: Option<SeatReactivity>,
    reactivity_seat_legs: u32,
    reactivity_ticks: u64,
    income: BTreeMap<u64, Vec<(u32, u32)>>,
    ledger: LedgerPool,
    attacks: Option<AttackCalibration>,
}

impl SeatSummaryBuilder {
    /// Adds one seat's evidence from a leg that ran `duration_ticks`.
    pub fn add(&mut self, evidence: &SeatEvidence, duration_ticks: u64) {
        self.seat_legs += 1;
        let failures = &evidence.failures;
        self.repeated_orders += failures.repeated_orders.incidents;
        self.abandoned_sites += failures.abandoned_sites.incidents;
        self.starved_production += failures.starved_production.incidents;
        self.stuck_missions += failures.stuck_missions.incidents;
        if let Some(idle) = &failures.idle_army {
            self.idle_army += idle.incidents;
            self.idle_army_seat_legs += 1;
        }
        if let Some(deliveries) = evidence.deliveries {
            self.deliveries.delivered += deliveries.delivered;
            self.deliveries.lost += deliveries.lost;
            self.deliveries.undelivered += deliveries.undelivered;
            self.deliveries_seat_legs += 1;
        }
        if let Some(reactivity) = &evidence.reactivity {
            self.reactivity
                .get_or_insert_with(SeatReactivity::default)
                .merge(reactivity);
            self.reactivity_seat_legs += 1;
            self.reactivity_ticks += evidence.eliminated_at.unwrap_or(duration_ticks);
        }
        for sample in &evidence.income {
            self.income
                .entry(sample.tick)
                .or_default()
                .push((sample.actual_per_minute, sample.saturation_per_minute));
        }
        if let Some(ledger) = &evidence.ledger {
            self.ledger.add(ledger);
        }
        if let Some(attacks) = &evidence.attacks {
            self.attacks
                .get_or_insert_with(AttackCalibration::default)
                .add(attacks);
        }
    }

    /// The pooled summary.
    pub fn finish(self) -> SeatSummary {
        SeatSummary {
            seat_legs: self.seat_legs,
            repeated_orders: self.repeated_orders,
            abandoned_sites: self.abandoned_sites,
            starved_production: self.starved_production,
            stuck_missions: self.stuck_missions,
            idle_army: self.idle_army,
            idle_army_seat_legs: self.idle_army_seat_legs,
            deliveries: self.deliveries,
            deliveries_seat_legs: self.deliveries_seat_legs,
            reactivity: self.reactivity,
            reactivity_seat_legs: self.reactivity_seat_legs,
            reactivity_ticks: self.reactivity_ticks,
            income: self
                .income
                .into_iter()
                .map(|(tick, samples)| IncomeMedian {
                    tick,
                    samples: samples.len() as u32,
                    actual_per_minute: median(
                        samples
                            .iter()
                            .map(|(actual, _)| u64::from(*actual))
                            .collect(),
                    )
                    .unwrap_or(0) as u32,
                    saturation_per_minute: median(
                        samples
                            .iter()
                            .map(|(_, saturation)| u64::from(*saturation))
                            .collect(),
                    )
                    .unwrap_or(0) as u32,
                    percent_of_saturation: median(
                        samples
                            .iter()
                            .filter(|(_, saturation)| *saturation > 0)
                            .map(|(actual, saturation)| {
                                u64::from(*actual) * 100 / u64::from(*saturation)
                            })
                            .collect(),
                    )
                    .map(|percent| percent as u32),
                })
                .collect(),
            ledger: self.ledger,
            attacks: self.attacks,
        }
    }
}

fn median(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    values.get(values.len() / 2).copied()
}

/// A count over the seat-legs whose rows record it: the count alone when every
/// row does, with its coverage when only some do, and `-` when none do.
fn measured(count: u64, recorded: u32, seat_legs: u32) -> String {
    match recorded {
        0 => "-".into(),
        _ if recorded == seat_legs => count.to_string(),
        _ => format!("{count} ({recorded}/{seat_legs} seat-legs)"),
    }
}

/// Renders every summary's tables, one row per labelled group: failure
/// incidents, deliveries when any seat trained armed ground units on
/// severed ground, reactivity, income, and each group's ledger.
pub fn render(out: &mut String, groups: &[(String, &SeatSummary)]) {
    let _ = writeln!(out, "failure incidents");
    let _ = writeln!(
        out,
        "  {:<24} {:>9} {:>16} {:>16} {:>18} {:>15} {:>20}",
        "group",
        "seat-legs",
        "repeated orders",
        "abandoned sites",
        "starved production",
        "stuck missions",
        "idle army"
    );
    for (label, summary) in groups {
        let _ = writeln!(
            out,
            "  {:<24} {:>9} {:>16} {:>16} {:>18} {:>15} {:>20}",
            label,
            summary.seat_legs,
            summary.repeated_orders,
            summary.abandoned_sites,
            summary.starved_production,
            summary.stuck_missions,
            measured(
                summary.idle_army,
                summary.idle_army_seat_legs,
                summary.seat_legs
            )
        );
    }
    let severed = |summary: &SeatSummary| {
        let deliveries = summary.deliveries;
        deliveries.delivered + deliveries.lost + deliveries.undelivered
    };
    if groups.iter().any(|(_, summary)| severed(summary) > 0) {
        let _ = writeln!(
            out,
            "\narmed ground units trained on severed ground, scrap: delivered/lost/undelivered (seat-legs recorded)"
        );
        for (label, summary) in groups.iter().filter(|(_, summary)| severed(summary) > 0) {
            let deliveries = summary.deliveries;
            let _ = writeln!(
                out,
                "  {:<24} {}/{}/{} ({}/{})",
                label,
                deliveries.delivered,
                deliveries.lost,
                deliveries.undelivered,
                summary.deliveries_seat_legs,
                summary.seat_legs
            );
        }
    }
    render_reactivity(out, groups);
    let _ = writeln!(
        out,
        "\nincome per minute: median percent of saturation (actual/saturation, samples)"
    );
    let _ = write!(out, "  {:<24}", "group");
    for tick in INCOME_CHECKPOINTS {
        let _ = write!(out, " {:<22}", format!("tick {tick}"));
    }
    let _ = writeln!(out);
    for (label, summary) in groups {
        let _ = write!(out, "  {label:<24}");
        for tick in INCOME_CHECKPOINTS {
            let cell = summary
                .income
                .iter()
                .find(|median| median.tick == tick)
                .map_or_else(
                    || "-".to_string(),
                    |median| {
                        format!(
                            "{} ({}/{}, {})",
                            median
                                .percent_of_saturation
                                .map_or_else(|| "-".to_string(), |p| format!("{p}%")),
                            median.actual_per_minute,
                            median.saturation_per_minute,
                            median.samples
                        )
                    },
                );
            let _ = write!(out, " {cell:<22}");
        }
        let _ = writeln!(out);
    }
    for (label, summary) in groups {
        if summary.ledger.seats == 0 {
            continue;
        }
        let _ = writeln!(
            out,
            "\nledger, {label}: {} of {} seat-legs recorded",
            summary.ledger.seats, summary.seat_legs
        );
        if let Some(attacks) = &summary.attacks {
            let _ = writeln!(
                out,
                "  attacks, by strength sent against the known defense:"
            );
            attacks.render(out, "    ");
        }
        summary.ledger.render(out, "  ", Some(LEDGER_KINDS));
    }
}

/// Each group's situations: how many arose, the share answered, those missed
/// and moot, and the mean ticks to an answer.
fn render_reactivity(out: &mut String, groups: &[(String, &SeatSummary)]) {
    let recorded: Vec<(&String, &SeatSummary, &SeatReactivity)> = groups
        .iter()
        .filter_map(|(label, summary)| Some((label, *summary, summary.reactivity.as_ref()?)))
        .collect();
    if recorded.is_empty() {
        return;
    }
    let _ = writeln!(
        out,
        "\nreactivity: situations that arose, the share answered in time, missed, moot, mean ticks to answer"
    );
    let _ = writeln!(
        out,
        "  {:<24} {:<15} {:>7} {:>9} {:>7} {:>7} {:>11}",
        "group", "situation", "arose", "answered", "missed", "moot", "mean ticks"
    );
    for (label, summary, reactivity) in recorded {
        for (name, reactions) in reactivity.items() {
            let Some(reactions) = reactions else {
                continue;
            };
            let answered = if reactions.arose == 0 {
                "-".to_string()
            } else {
                format!(
                    "{:.0}%",
                    100.0 * reactions.answered as f64 / reactions.arose as f64
                )
            };
            let mean = reactions
                .answer_ticks
                .checked_div(reactions.answered)
                .map_or_else(|| "-".to_string(), |ticks| ticks.to_string());
            let _ = writeln!(
                out,
                "  {:<24} {:<15} {:>7} {:>9} {:>7} {:>7} {:>11}",
                label, name, reactions.arose, answered, reactions.missed, reactions.moot, mean
            );
        }
        if let Some(switches) = reactivity.target_switches
            && summary.reactivity_ticks > 0
        {
            let _ = writeln!(
                out,
                "  {:<24} target switches per 10k ticks: {:.2} ({} seat-legs)",
                label,
                switches as f64 * 10_000.0 / summary.reactivity_ticks as f64,
                summary.reactivity_seat_legs
            );
        }
    }
}

/// The fields of an evaluation row a summary reads.
#[derive(Debug, Clone, Deserialize)]
struct SummaryRow {
    duration_ticks: u64,
    seats: Vec<SummarySeat>,
    evidence: Vec<SeatEvidence>,
}

/// A seat's team and configured controller.
#[derive(Debug, Clone, Deserialize)]
struct SummarySeat {
    team: u8,
    config: Option<BotConfig>,
}

/// Evaluation rows pooled by match mode and then by difficulty.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RowSummary {
    /// Legs read.
    pub legs: u32,
    /// Each match mode and difficulty with its pooled seats, in mode and
    /// then difficulty order.
    pub groups: Vec<(String, SeatSummary)>,
}

/// Reads evaluation rows from JSONL files and pools every controlled seat's
/// evidence by match mode and difficulty.
pub fn summarize(paths: &[PathBuf]) -> Result<RowSummary> {
    let mut legs = 0;
    let mut groups: BTreeMap<(MatchMode, usize), (String, SeatSummaryBuilder)> = BTreeMap::new();
    for path in paths {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading evaluation rows {}", path.display()))?;
        for (line, record) in text.lines().enumerate() {
            if record.trim().is_empty() {
                continue;
            }
            let row: SummaryRow = serde_json::from_str(record).with_context(|| {
                format!("{}:{} is not an evaluation row", path.display(), line + 1)
            })?;
            ensure!(
                row.seats.len() == row.evidence.len(),
                "{}:{} has inconsistent seats",
                path.display(),
                line + 1
            );
            let teams: Vec<u8> = row.seats.iter().map(|seat| seat.team).collect();
            let mode = MatchMode::of(&teams)
                .with_context(|| format!("{}:{}", path.display(), line + 1))?;
            legs += 1;
            for (seat, evidence) in row.seats.iter().zip(&row.evidence) {
                let Some(config) = seat.config else {
                    continue;
                };
                let rank = rank(config.difficulty);
                groups
                    .entry((mode, rank))
                    .or_insert_with(|| {
                        (
                            format!("{} {}", mode.as_str(), config.difficulty),
                            SeatSummaryBuilder::default(),
                        )
                    })
                    .1
                    .add(evidence, row.duration_ticks);
            }
        }
    }
    Ok(RowSummary {
        legs,
        groups: groups
            .into_values()
            .map(|(label, builder)| (label, builder.finish()))
            .collect(),
    })
}

/// A difficulty's place from the lowest rung up.
pub fn rank(difficulty: BotDifficulty) -> usize {
    BotDifficulty::ALL
        .iter()
        .position(|rung| *rung == difficulty)
        .unwrap_or(0)
}

impl RowSummary {
    /// The summary as text.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "bot-summary: {} legs\n", self.legs);
        let groups: Vec<(String, &SeatSummary)> = self
            .groups
            .iter()
            .map(|(label, summary)| (label.clone(), summary))
            .collect();
        render(&mut out, &groups);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(idle: u64, income: [(u64, u32, u32); 2]) -> serde_json::Value {
        serde_json::json!({
            "seat": 0,
            "failures": {
                "repeated_orders": {"incidents": 1, "examples": []},
                "abandoned_sites": {"incidents": 0, "examples": []},
                "starved_production": {"incidents": 0, "examples": []},
                "stuck_missions": {"incidents": 0, "examples": []},
                "idle_army": {"incidents": idle, "examples": []},
            },
            "income": income
                .iter()
                .map(|(tick, actual, saturation)| serde_json::json!({
                    "tick": tick,
                    "actual_per_minute": actual,
                    "saturation_per_minute": saturation,
                }))
                .collect::<Vec<_>>(),
        })
    }

    fn row(teams: &[u8], difficulties: &[&str], idle: u64) -> String {
        serde_json::json!({
            "duration_ticks": 9_000,
            "seats": teams
                .iter()
                .zip(difficulties)
                .enumerate()
                .map(|(seat, (team, difficulty))| serde_json::json!({
                    "seat": seat,
                    "team": team,
                    "config": {"controller": "opponent", "difficulty": difficulty},
                }))
                .collect::<Vec<_>>(),
            "evidence": teams
                .iter()
                .map(|_| evidence(idle, [(6_000, 900, 1_000), (12_000, 600, 1_200)]))
                .collect::<Vec<_>>(),
        })
        .to_string()
    }

    #[test]
    fn rows_pool_by_mode_and_difficulty_from_the_lowest_rung_up() {
        let dir = std::env::temp_dir().join(format!("oxide-seat-summary-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rows.jsonl");
        let rows = [
            row(&[0, 1], &["prime", "standard"], 2),
            row(&[0, 1], &["prime", "standard"], 0),
            row(&[0, 0, 1, 1], &["standard"; 4], 1),
        ];
        std::fs::write(&path, rows.join("\n")).unwrap();
        let summary = summarize(&[path]).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(summary.legs, 3);
        let groups: Vec<(&str, u32, u64, u64)> = summary
            .groups
            .iter()
            .map(|(label, seats)| {
                (
                    label.as_str(),
                    seats.seat_legs,
                    seats.repeated_orders,
                    seats.idle_army,
                )
            })
            .collect();
        assert_eq!(
            groups,
            [
                ("duel standard", 2, 2, 2),
                ("duel prime", 2, 2, 2),
                ("teams standard", 4, 4, 4),
            ]
        );
        let income = &summary.groups[0].1.income;
        assert_eq!(income.len(), 2);
        assert_eq!(
            (income[0].samples, income[0].percent_of_saturation),
            (2, Some(90))
        );
        let text = summary.render();
        assert!(text.contains("failure incidents"), "{text}");
        assert!(text.contains("teams standard"), "{text}");
    }
}
