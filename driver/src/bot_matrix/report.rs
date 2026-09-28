//! Scores matrix rows: head-to-head pairs, decided rates, failure incidents
//! and income, overall and by difficulty, stance and map family.

use super::{MapFamily, MatrixLabel, Pairing};
use crate::bot_eval::{
    EvaluationControllerKind, EvaluationLeg, INCOME_CHECKPOINTS, IncomeSample, SeatFailures,
    Termination,
};
use anyhow::{Context, Result, bail, ensure};
use oxide_kit::recovery::BuildIdentity;
use oxide_sim::GameResult;
use oxide_sim::scenario::{BotDifficulty, BotStance};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

/// The fields of one matrix row that scoring reads.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoredRow {
    /// Matrix position.
    pub matrix: MatrixLabel,
    /// Candidate that produced the row.
    pub candidate: String,
    /// Frozen `oxide-bot` digest of the producing build.
    pub oxide_bot_digest: String,
    /// Producing build.
    pub build: BuildIdentity,
    /// Seat-pair leg.
    pub leg: EvaluationLeg,
    /// Why the leg stopped.
    pub termination: Termination,
    /// Final result, absent when undecided.
    pub result: Option<GameResult>,
    /// Seats on the winning team.
    pub winner_seats: Vec<u8>,
    /// Controller by seat.
    pub seats: Vec<ScoredSeat>,
    /// QA evidence by seat.
    pub evidence: Vec<ScoredEvidence>,
}

/// A seat's controller.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoredSeat {
    /// Player seat.
    pub seat: u8,
    /// Controller family.
    pub controller: EvaluationControllerKind,
}

/// A seat's failure and income evidence.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoredEvidence {
    /// Player seat.
    pub seat: u8,
    /// Detected failure episodes.
    pub failures: SeatFailures,
    /// Income checkpoints reached.
    pub income: Vec<IncomeSample>,
}

/// Reads matrix rows from JSONL files, in file and line order.
pub fn load_rows(paths: &[PathBuf]) -> Result<Vec<ScoredRow>> {
    let mut rows = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading matrix rows {}", path.display()))?;
        for (line, record) in text.lines().enumerate() {
            if record.trim().is_empty() {
                continue;
            }
            let row: ScoredRow = serde_json::from_str(record).with_context(|| {
                format!("{}:{} is not a bot-matrix row", path.display(), line + 1)
            })?;
            ensure!(
                row.seats.len() == row.evidence.len()
                    && row.seats.iter().zip(&row.evidence).enumerate().all(
                        |(index, (seat, evidence))| {
                            usize::from(seat.seat) == index && usize::from(evidence.seat) == index
                        }
                    ),
                "{}:{} has inconsistent seats",
                path.display(),
                line + 1
            );
            rows.push(row);
        }
    }
    Ok(rows)
}

/// Head-to-head pairs by result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PairTally {
    /// The new bot won both legs.
    pub new_wins_both: u32,
    /// Each side won one leg.
    pub split: u32,
    /// `oxide-bot` won both legs.
    pub old_wins_both: u32,
    /// At least one leg ended without a winner.
    pub undecided: u32,
}

/// Legs by termination.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LegTally {
    /// Legs counted.
    pub legs: u32,
    /// Legs the simulation decided, draws included.
    pub decided: u32,
    /// Legs stopped by a repeated-stall loop.
    pub stall_loops: u32,
}

/// The new bot's share of head-to-head legs with a winner.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NewShare {
    /// Legs the new bot won.
    pub new_wins: u32,
    /// Legs `oxide-bot` won.
    pub old_wins: u32,
    /// `new_wins` over both.
    pub share: f64,
    /// 95% Wilson interval of the share.
    pub wilson: [f64; 2],
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

/// Failure incidents and income for one controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControllerTally {
    /// Controller family.
    pub controller: EvaluationControllerKind,
    /// Seats this controller played, summed over legs.
    pub seat_legs: u32,
    /// Repeated impossible-order episodes.
    pub repeated_orders: u64,
    /// Abandoned paid-construction episodes.
    pub abandoned_sites: u64,
    /// Production-starvation episodes.
    pub starved_producers: u64,
    /// Income medians by checkpoint.
    pub income: Vec<IncomeMedian>,
}

/// One slice of the matrix.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupReport {
    /// `overall`, or the difficulty, stance or family it covers.
    pub group: String,
    /// Head-to-head pairs.
    pub pairs: PairTally,
    /// Head-to-head legs.
    pub head_to_head: LegTally,
    /// The new bot's share of won legs; absent when no leg had a winner.
    pub new_share: Option<NewShare>,
    /// Baseline legs.
    pub baseline: LegTally,
    /// Per-controller incidents and income, new bot first.
    pub controllers: Vec<ControllerTally>,
}

/// A provenance value and the legs carrying it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Provenance {
    /// The value.
    pub value: String,
    /// Legs carrying it.
    pub legs: u32,
}

/// The complete matrix report.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MatrixReport {
    /// Head-to-head legs.
    pub head_to_head_legs: u32,
    /// Baseline legs.
    pub baseline_legs: u32,
    /// Frozen `oxide-bot` digests.
    pub references: Vec<Provenance>,
    /// Producing builds.
    pub builds: Vec<Provenance>,
    /// Candidates.
    pub candidates: Vec<Provenance>,
    /// Overall, then by difficulty, stance and map family.
    pub groups: Vec<GroupReport>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LegOutcome {
    NewWin,
    OldWin,
    NoWinner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    Overall,
    Difficulty(u8),
    Stance(u8),
    Family(MapFamily),
}

impl GroupKey {
    fn of(label: &MatrixLabel) -> [Self; 4] {
        [
            Self::Overall,
            Self::Difficulty(label.difficulty as u8),
            Self::Stance(label.stance as u8),
            Self::Family(label.family),
        ]
    }

    fn name(self) -> String {
        match self {
            Self::Overall => "overall".into(),
            Self::Difficulty(index) => {
                format!("difficulty {}", BotDifficulty::ALL[usize::from(index)])
            }
            Self::Stance(index) => format!("stance {}", BotStance::ALL[usize::from(index)]),
            Self::Family(family) => format!("family {}", family.as_str()),
        }
    }
}

#[derive(Default)]
struct ControllerBuilder {
    seat_legs: u32,
    repeated_orders: u64,
    abandoned_sites: u64,
    starved_producers: u64,
    income: BTreeMap<u64, Vec<(u32, u32)>>,
}

#[derive(Default)]
struct GroupBuilder {
    pairs: PairTally,
    head_to_head: LegTally,
    new_wins: u32,
    old_wins: u32,
    baseline: LegTally,
    controllers: BTreeMap<EvaluationControllerKind, ControllerBuilder>,
}

type PairKey = (String, String, u8, u8, u64);

/// Scores rows. Every head-to-head pair must be complete, and no matrix leg
/// may appear twice.
pub fn build_report(rows: &[ScoredRow]) -> Result<MatrixReport> {
    let mut groups: BTreeMap<GroupKey, GroupBuilder> = BTreeMap::new();
    let mut pairs: BTreeMap<PairKey, (&MatrixLabel, [Option<LegOutcome>; 2])> = BTreeMap::new();
    let mut baselines: BTreeSet<PairKey> = BTreeSet::new();
    let (mut head_to_head_legs, mut baseline_legs) = (0, 0);
    for row in rows {
        let label = &row.matrix;
        let key = (
            label.manifest.clone(),
            label.map.clone(),
            label.difficulty as u8,
            label.stance as u8,
            label.run,
        );
        let describe = || {
            format!(
                "{} {} {} {} run {} {:?}",
                label.manifest, label.map, label.difficulty, label.stance, label.run, row.leg
            )
        };
        let leg_outcome = match label.pairing {
            Pairing::HeadToHead => {
                head_to_head_legs += 1;
                let slot = match row.leg {
                    EvaluationLeg::Forward => 0,
                    EvaluationLeg::Swapped => 1,
                    EvaluationLeg::Single => bail!("head-to-head leg {} is unpaired", describe()),
                };
                let outcome = outcome(row).with_context(describe)?;
                let (_, pair) = pairs.entry(key).or_insert((label, [None, None]));
                ensure!(pair[slot].is_none(), "matrix leg {} repeats", describe());
                pair[slot] = Some(outcome);
                Some(outcome)
            }
            Pairing::Baseline => {
                baseline_legs += 1;
                ensure!(baselines.insert(key), "baseline leg {} repeats", describe());
                None
            }
        };
        for group in GroupKey::of(label) {
            let builder = groups.entry(group).or_default();
            let legs = match label.pairing {
                Pairing::HeadToHead => &mut builder.head_to_head,
                Pairing::Baseline => &mut builder.baseline,
            };
            legs.legs += 1;
            legs.decided += u32::from(row.termination == Termination::Decided);
            legs.stall_loops += u32::from(row.termination == Termination::StallLoop);
            match leg_outcome {
                Some(LegOutcome::NewWin) => builder.new_wins += 1,
                Some(LegOutcome::OldWin) => builder.old_wins += 1,
                Some(LegOutcome::NoWinner) | None => {}
            }
            for (seat, evidence) in row.seats.iter().zip(&row.evidence) {
                if seat.controller == EvaluationControllerKind::None {
                    continue;
                }
                let controller = builder.controllers.entry(seat.controller).or_default();
                controller.seat_legs += 1;
                controller.repeated_orders += evidence.failures.repeated_orders.incidents;
                controller.abandoned_sites += evidence.failures.abandoned_sites.incidents;
                controller.starved_producers += evidence.failures.starved_producers.incidents;
                for sample in &evidence.income {
                    controller
                        .income
                        .entry(sample.tick)
                        .or_default()
                        .push((sample.actual_per_minute, sample.saturation_per_minute));
                }
            }
        }
    }
    for (label, legs) in pairs.values() {
        let [Some(forward), Some(swapped)] = *legs else {
            bail!(
                "head-to-head pair {} {} {} {} run {} is incomplete",
                label.manifest,
                label.map,
                label.difficulty,
                label.stance,
                label.run
            );
        };
        let wins = |side| u32::from(forward == side) + u32::from(swapped == side);
        for group in GroupKey::of(label) {
            let tally = &mut groups.get_mut(&group).expect("rows opened groups").pairs;
            match (wins(LegOutcome::NewWin), wins(LegOutcome::OldWin)) {
                (2, _) => tally.new_wins_both += 1,
                (_, 2) => tally.old_wins_both += 1,
                (1, 1) => tally.split += 1,
                _ => tally.undecided += 1,
            }
        }
    }
    Ok(MatrixReport {
        head_to_head_legs,
        baseline_legs,
        references: provenance(rows, |row| row.oxide_bot_digest.clone()),
        builds: provenance(rows, |row| {
            let build = &row.build;
            format!(
                "{} {} dirty={} {}-{}",
                build.version, build.revision, build.dirty, build.os, build.architecture
            )
        }),
        candidates: provenance(rows, |row| row.candidate.clone()),
        groups: groups
            .into_iter()
            .map(|(key, builder)| finish(key, builder))
            .collect(),
    })
}

fn outcome(row: &ScoredRow) -> Result<LegOutcome> {
    let seat_of = |controller| {
        row.seats
            .iter()
            .filter(|seat| seat.controller == controller)
            .map(|seat| seat.seat)
            .collect::<Vec<_>>()
    };
    let (new, old) = (
        seat_of(EvaluationControllerKind::Opponent),
        seat_of(EvaluationControllerKind::Scripted),
    );
    let ([new], [old]) = (new.as_slice(), old.as_slice()) else {
        bail!("a head-to-head leg needs one oxide-opponent and one oxide-bot seat");
    };
    Ok(match row.result {
        Some(GameResult::Victory { .. }) if row.winner_seats.contains(new) => LegOutcome::NewWin,
        Some(GameResult::Victory { .. }) if row.winner_seats.contains(old) => LegOutcome::OldWin,
        _ => LegOutcome::NoWinner,
    })
}

fn provenance(rows: &[ScoredRow], value: impl Fn(&ScoredRow) -> String) -> Vec<Provenance> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for row in rows {
        *counts.entry(value(row)).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(value, legs)| Provenance { value, legs })
        .collect()
}

fn median(mut values: Vec<u64>) -> Option<u64> {
    values.sort_unstable();
    crate::sweep::quantile(&values, 1, 2)
}

fn finish(key: GroupKey, builder: GroupBuilder) -> GroupReport {
    let decided = builder.new_wins + builder.old_wins;
    let mut controllers: Vec<ControllerTally> = builder
        .controllers
        .into_iter()
        .map(|(controller, tally)| ControllerTally {
            controller,
            seat_legs: tally.seat_legs,
            repeated_orders: tally.repeated_orders,
            abandoned_sites: tally.abandoned_sites,
            starved_producers: tally.starved_producers,
            income: tally
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
        })
        .collect();
    controllers.sort_by_key(|tally| std::cmp::Reverse(tally.controller));
    GroupReport {
        group: key.name(),
        pairs: builder.pairs,
        head_to_head: builder.head_to_head,
        new_share: (decided > 0).then(|| NewShare {
            new_wins: builder.new_wins,
            old_wins: builder.old_wins,
            share: f64::from(builder.new_wins) / f64::from(decided),
            wilson: crate::sweep::wilson(builder.new_wins, decided),
        }),
        baseline: builder.baseline,
        controllers,
    }
}

impl MatrixReport {
    /// A fixed-width text rendering.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "bot-matrix: {} head-to-head legs, {} baseline legs",
            self.head_to_head_legs, self.baseline_legs
        );
        for (label, values) in [
            ("oxide-bot", &self.references),
            ("build", &self.builds),
            ("candidate", &self.candidates),
        ] {
            for value in values {
                let _ = writeln!(out, "{label:<10} {} ({} legs)", value.value, value.legs);
            }
        }
        let _ = writeln!(
            out,
            "\nhead to head: pairs new-both/split/old-both/undecided; new share of won legs; decided legs"
        );
        let _ = writeln!(
            out,
            "{:<22} {:<13} {:<30} {:<18} {:<18} stall loops",
            "group", "pairs", "new share (95% Wilson)", "decided new-old", "decided old-old"
        );
        for group in &self.groups {
            let pairs = &group.pairs;
            let share = group.new_share.as_ref().map_or_else(
                || "no winners".to_string(),
                |share| {
                    format!(
                        "{}/{} {:.0}% [{:.0}-{:.0}%]",
                        share.new_wins,
                        share.new_wins + share.old_wins,
                        100.0 * share.share,
                        100.0 * share.wilson[0],
                        100.0 * share.wilson[1]
                    )
                },
            );
            let _ = writeln!(
                out,
                "{:<22} {:<13} {:<30} {:<18} {:<18} {}/{}",
                group.group,
                format!(
                    "{}/{}/{}/{}",
                    pairs.new_wins_both, pairs.split, pairs.old_wins_both, pairs.undecided
                ),
                share,
                decided(&group.head_to_head),
                decided(&group.baseline),
                group.head_to_head.stall_loops,
                group.baseline.stall_loops,
            );
        }
        let _ = writeln!(out, "\nfailure incidents");
        let _ = writeln!(
            out,
            "{:<22} {:<10} {:>9} {:>16} {:>16} {:>18}",
            "group",
            "controller",
            "seat-legs",
            "repeated orders",
            "abandoned sites",
            "starved producers"
        );
        for group in &self.groups {
            for tally in &group.controllers {
                let _ = writeln!(
                    out,
                    "{:<22} {:<10} {:>9} {:>16} {:>16} {:>18}",
                    group.group,
                    controller_name(tally.controller),
                    tally.seat_legs,
                    tally.repeated_orders,
                    tally.abandoned_sites,
                    tally.starved_producers
                );
            }
        }
        let _ = writeln!(
            out,
            "\nincome per minute: median percent of saturation (actual/saturation, samples)"
        );
        let _ = write!(out, "{:<22} {:<10}", "group", "controller");
        for tick in INCOME_CHECKPOINTS {
            let _ = write!(out, " {:<22}", format!("tick {tick}"));
        }
        let _ = writeln!(out);
        for group in &self.groups {
            for tally in &group.controllers {
                let _ = write!(
                    out,
                    "{:<22} {:<10}",
                    group.group,
                    controller_name(tally.controller)
                );
                for tick in INCOME_CHECKPOINTS {
                    let cell = tally
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
        }
        out
    }
}

fn decided(legs: &LegTally) -> String {
    if legs.legs == 0 {
        return "-".into();
    }
    format!(
        "{}/{} {:.0}%",
        legs.decided,
        legs.legs,
        100.0 * f64::from(legs.decided) / f64::from(legs.legs)
    )
}

fn controller_name(controller: EvaluationControllerKind) -> &'static str {
    match controller {
        EvaluationControllerKind::Opponent => "opponent",
        EvaluationControllerKind::Scripted => "scripted",
        EvaluationControllerKind::None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot_eval::FailureTally;

    fn build() -> BuildIdentity {
        BuildIdentity::new("0.0.0", "abc", "false")
    }

    fn label(map: &str, family: MapFamily, run: u64, pairing: Pairing) -> MatrixLabel {
        MatrixLabel {
            manifest: "unit".into(),
            map: map.into(),
            family,
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Balanced,
            run,
            pairing,
        }
    }

    fn evidence(seat: u8, starved: u64, income: Vec<IncomeSample>) -> ScoredEvidence {
        ScoredEvidence {
            seat,
            failures: SeatFailures {
                starved_producers: FailureTally {
                    incidents: starved,
                    examples: Vec::new(),
                },
                ..SeatFailures::default()
            },
            income,
        }
    }

    /// A head-to-head leg; `winner` is the winning controller, if any.
    fn duel(
        label: MatrixLabel,
        leg: EvaluationLeg,
        winner: Option<EvaluationControllerKind>,
    ) -> ScoredRow {
        let controllers = match leg {
            EvaluationLeg::Swapped => [
                EvaluationControllerKind::Scripted,
                EvaluationControllerKind::Opponent,
            ],
            _ => [
                EvaluationControllerKind::Opponent,
                EvaluationControllerKind::Scripted,
            ],
        };
        let winner_seat = winner.map(|winner| {
            controllers
                .iter()
                .position(|controller| *controller == winner)
                .unwrap() as u8
        });
        ScoredRow {
            matrix: label,
            candidate: "unit".into(),
            oxide_bot_digest: "fnv1a64:1".into(),
            build: build(),
            leg,
            termination: if winner.is_some() {
                Termination::Decided
            } else {
                Termination::TickLimit
            },
            result: winner_seat.map(|team| GameResult::Victory { team }),
            winner_seats: winner_seat.into_iter().collect(),
            seats: controllers
                .iter()
                .enumerate()
                .map(|(seat, controller)| ScoredSeat {
                    seat: seat as u8,
                    controller: *controller,
                })
                .collect(),
            evidence: vec![
                evidence(
                    0,
                    1,
                    vec![IncomeSample {
                        tick: 6_000,
                        actual_per_minute: 300,
                        saturation_per_minute: 600,
                    }],
                ),
                evidence(1, 2, Vec::new()),
            ],
        }
    }

    fn baseline(label: MatrixLabel, decided: bool) -> ScoredRow {
        let mut row = duel(label, EvaluationLeg::Single, None);
        row.seats[0].controller = EvaluationControllerKind::Scripted;
        if decided {
            row.termination = Termination::Decided;
            row.result = Some(GameResult::Victory { team: 1 });
            row.winner_seats = vec![1];
        }
        row
    }

    fn pair(
        map: &str,
        family: MapFamily,
        run: u64,
        winners: [Option<EvaluationControllerKind>; 2],
    ) -> Vec<ScoredRow> {
        vec![
            duel(
                label(map, family, run, Pairing::HeadToHead),
                EvaluationLeg::Forward,
                winners[0],
            ),
            duel(
                label(map, family, run, Pairing::HeadToHead),
                EvaluationLeg::Swapped,
                winners[1],
            ),
        ]
    }

    const NEW: Option<EvaluationControllerKind> = Some(EvaluationControllerKind::Opponent);
    const OLD: Option<EvaluationControllerKind> = Some(EvaluationControllerKind::Scripted);

    #[test]
    fn pairs_classify_by_both_legs_and_share_counts_won_legs() {
        let mut rows = Vec::new();
        rows.extend(pair("a", MapFamily::Open, 0, [NEW, NEW]));
        rows.extend(pair("a", MapFamily::Open, 1, [NEW, OLD]));
        rows.extend(pair("b", MapFamily::Severed, 0, [OLD, OLD]));
        rows.extend(pair("b", MapFamily::Severed, 1, [NEW, None]));
        rows.push(baseline(
            label("a", MapFamily::Open, 0, Pairing::Baseline),
            true,
        ));
        rows.push(baseline(
            label("b", MapFamily::Severed, 0, Pairing::Baseline),
            false,
        ));
        let report = build_report(&rows).unwrap();

        assert_eq!((report.head_to_head_legs, report.baseline_legs), (8, 2));
        let names: Vec<&str> = report
            .groups
            .iter()
            .map(|group| group.group.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "overall",
                "difficulty prime",
                "stance balanced",
                "family open",
                "family severed"
            ]
        );
        let overall = &report.groups[0];
        assert_eq!(
            overall.pairs,
            PairTally {
                new_wins_both: 1,
                split: 1,
                old_wins_both: 1,
                undecided: 1,
            }
        );
        let share = overall.new_share.as_ref().unwrap();
        assert_eq!((share.new_wins, share.old_wins), (4, 3));
        assert_eq!(share.wilson, crate::sweep::wilson(4, 7));
        assert_eq!(
            overall.head_to_head,
            LegTally {
                legs: 8,
                decided: 7,
                stall_loops: 0
            }
        );
        assert_eq!(
            overall.baseline,
            LegTally {
                legs: 2,
                decided: 1,
                stall_loops: 0
            }
        );
        let severed = &report.groups[4];
        assert_eq!(severed.pairs.old_wins_both, 1);
        assert_eq!(severed.pairs.undecided, 1);

        let [new, old] = overall.controllers.as_slice() else {
            panic!("two controllers: {:?}", overall.controllers);
        };
        assert_eq!(new.controller, EvaluationControllerKind::Opponent);
        assert_eq!(new.seat_legs, 8);
        assert_eq!(new.starved_producers, 4 + 2 * 4);
        assert_eq!(old.seat_legs, 8 + 4);
        assert_eq!(
            new.income,
            [IncomeMedian {
                tick: 6_000,
                samples: 4,
                actual_per_minute: 300,
                saturation_per_minute: 600,
                percent_of_saturation: Some(50),
            }]
        );
        assert_eq!(report.references[0].legs, 10);
        let text = report.render();
        assert!(text.contains("4/7 57%"), "{text}");
        assert!(text.contains("family severed"), "{text}");
        assert!(text.contains("50% (300/600, 4)"), "{text}");
    }

    #[test]
    fn incomplete_pairs_and_repeated_legs_are_refused() {
        let mut rows = pair("a", MapFamily::Open, 0, [NEW, OLD]);
        rows.pop();
        let error = build_report(&rows).unwrap_err().to_string();
        assert!(error.contains("incomplete"), "{error}");

        let mut rows = pair("a", MapFamily::Open, 0, [NEW, OLD]);
        rows.push(rows[0].clone());
        let error = build_report(&rows).unwrap_err().to_string();
        assert!(error.contains("repeats"), "{error}");

        let mut rows = pair("a", MapFamily::Open, 0, [NEW, OLD]);
        rows[1].seats[1].controller = EvaluationControllerKind::Scripted;
        assert!(
            build_report(&rows).is_err(),
            "a head-to-head leg needs both bots"
        );
    }

    #[test]
    fn rows_that_are_not_matrix_rows_are_refused_with_their_line() {
        let dir = std::env::temp_dir().join(format!("oxide-matrix-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rows.jsonl");
        std::fs::write(&path, "{\"sim_version\":\"x\"}\n").unwrap();
        let error = format!("{:#}", load_rows(std::slice::from_ref(&path)).unwrap_err());
        assert!(
            error.contains("rows.jsonl:1 is not a bot-matrix row"),
            "{error}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
