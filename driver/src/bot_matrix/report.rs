//! Scores matrix rows by match mode: head-to-head and mixed pairs, decided
//! rates, placement, failure incidents and income, overall and by difficulty,
//! stance and map family.

use super::{MapFamily, MatchMode, MatrixLabel, Pairing};
use crate::bot_eval::{
    AttackCalibration, Deliveries, EvaluationControllerKind, EvaluationLeg, INCOME_CHECKPOINTS,
    IncomeSample, SeatFailures, SeatReactivity, Termination,
};
use crate::ledger::{
    LedgerPool, PairShares, SeatLedger, WorthShare, render_shares, team_shares, worth_shares,
};
use anyhow::{Context, Result, bail, ensure};
use oxide_kit::recovery::BuildIdentity;
use oxide_sim::GameResult;
use oxide_sim::scenario::{BotDifficulty, BotStance};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

/// Unit and building kinds each controller's ledger table shows.
const LEDGER_KINDS: usize = 10;

/// The fields of one matrix row that scoring reads.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoredRow {
    /// Matrix position.
    pub matrix: MatrixLabel,
    /// Candidate that produced the row.
    pub candidate: String,
    /// Reference digest of the producing build.
    pub reference_digest: String,
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
    /// Ticks the leg ran.
    pub duration_ticks: u64,
    /// Controller by seat.
    pub seats: Vec<ScoredSeat>,
    /// QA evidence by seat.
    pub evidence: Vec<ScoredEvidence>,
}

/// A seat's team and controller.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoredSeat {
    /// Player seat.
    pub seat: u8,
    /// Team the seat played on.
    pub team: u8,
    /// Controller family.
    pub controller: EvaluationControllerKind,
}

/// A seat's elimination, failure and income evidence.
#[derive(Debug, Clone, Deserialize)]
pub struct ScoredEvidence {
    /// Player seat.
    pub seat: u8,
    /// Tick the seat resigned or lost its last Foundry; absent while it stood.
    pub eliminated_at: Option<u64>,
    /// Detected failure episodes.
    pub failures: SeatFailures,
    /// Armed ground units trained on severed ground, by outcome; absent from
    /// rows recorded before the diagnostic existed.
    #[serde(default)]
    pub deliveries: Option<Deliveries>,
    /// Income checkpoints reached.
    pub income: Vec<IncomeSample>,
    /// Situations met and how they were answered; absent from rows recorded
    /// before the detectors existed.
    #[serde(default)]
    pub reactivity: Option<SeatReactivity>,
    /// What the seat's units and buildings did; absent from rows recorded
    /// before the ledger existed.
    #[serde(default)]
    pub ledger: Option<SeatLedger>,
    /// Attack calibration; absent for `oxide-bot` and from rows recorded
    /// before it existed.
    #[serde(default)]
    pub attacks: Option<AttackCalibration>,
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

/// Pairs of one pairing by result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PairTally {
    /// The new bot won both legs.
    pub new_wins_both: u32,
    /// Each side won one leg.
    pub split: u32,
    /// `oxide-bot` won both legs.
    pub old_wins_both: u32,
    /// At least one leg had no winner: undecided, or level on placement.
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

/// The new bot's share of legs with a winner.
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

/// Pairs, legs and the new bot's share for one pairing.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PairingReport {
    /// Pairs by result.
    pub pairs: PairTally,
    /// Legs by termination.
    pub legs: LegTally,
    /// The new bot's share of won legs; absent when no leg had a winner.
    pub new_share: Option<NewShare>,
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

/// Where one controller's seats finished in mixed legs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Placement {
    /// Seats this controller played in mixed legs.
    pub seat_legs: u32,
    /// Mean place, 1 being the last seat standing; tied seats share the mean
    /// of their places.
    pub mean_place: f64,
    /// Median tick a seat was eliminated, or its leg's end for a survivor.
    pub median_survival_ticks: u64,
}

/// Failure incidents, income and placement for one controller.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ControllerTally {
    /// Controller family.
    pub controller: EvaluationControllerKind,
    /// Seats this controller played, summed over legs.
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
    /// Placement in mixed legs; absent without any.
    pub placement: Option<Placement>,
    /// Pooled impact ledgers over the seat-legs whose rows record one.
    pub ledger: LedgerPool,
    /// Attack calibration summed over the seat-legs whose rows record it;
    /// absent when none do.
    pub attacks: Option<AttackCalibration>,
    /// Its side's share of net worth in head-to-head legs, by pair.
    pub worth: Vec<WorthShare>,
}

/// One slice of the matrix.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupReport {
    /// Match mode of the rows it covers.
    pub mode: MatchMode,
    /// `overall`, or the difficulty, stance or family it covers.
    pub group: String,
    /// Head-to-head pairs and legs.
    pub head_to_head: PairingReport,
    /// Mixed pairs and legs.
    pub mixed: PairingReport,
    /// Baseline legs.
    pub baseline: LegTally,
    /// Per-controller incidents, income and placement, new bot first.
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
    /// Mixed legs.
    pub mixed_legs: u32,
    /// Baseline legs.
    pub baseline_legs: u32,
    /// Reference digests: the frozen `oxide-bot` and simulation sources.
    pub references: Vec<Provenance>,
    /// Producing builds.
    pub builds: Vec<Provenance>,
    /// Candidates.
    pub candidates: Vec<Provenance>,
    /// By match mode: overall, then by difficulty, stance and map family.
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
    doubled_places: Vec<u64>,
    survival: Vec<u64>,
    ledger: LedgerPool,
    attacks: Option<AttackCalibration>,
    worth: PairShares,
}

#[derive(Default)]
struct PairingBuilder {
    pairs: PairTally,
    legs: LegTally,
    new_wins: u32,
    old_wins: u32,
}

impl PairingBuilder {
    fn finish(self) -> PairingReport {
        let won = self.new_wins + self.old_wins;
        PairingReport {
            pairs: self.pairs,
            legs: self.legs,
            new_share: (won > 0).then(|| NewShare {
                new_wins: self.new_wins,
                old_wins: self.old_wins,
                share: f64::from(self.new_wins) / f64::from(won),
                wilson: crate::sweep::wilson(self.new_wins, won),
            }),
        }
    }
}

#[derive(Default)]
struct GroupBuilder {
    head_to_head: PairingBuilder,
    mixed: PairingBuilder,
    baseline: LegTally,
    controllers: BTreeMap<EvaluationControllerKind, ControllerBuilder>,
}

impl GroupBuilder {
    fn compared(&mut self, pairing: Pairing) -> &mut PairingBuilder {
        if pairing == Pairing::Mixed {
            &mut self.mixed
        } else {
            &mut self.head_to_head
        }
    }
}

type PairKey = (String, String, u8, u8, u64, Pairing);

type Pair<'a> = (&'a MatrixLabel, MatchMode, [Option<LegOutcome>; 2]);

/// Scores rows. Every compared pair must be complete, and no matrix leg may
/// appear twice.
pub fn build_report(rows: &[ScoredRow]) -> Result<MatrixReport> {
    let mut groups: BTreeMap<(MatchMode, GroupKey), GroupBuilder> = BTreeMap::new();
    let mut pairs: BTreeMap<PairKey, Pair<'_>> = BTreeMap::new();
    let mut baselines: BTreeSet<PairKey> = BTreeSet::new();
    let (mut head_to_head_legs, mut mixed_legs, mut baseline_legs) = (0, 0, 0);
    for row in rows {
        let label = &row.matrix;
        let key = (
            label.manifest.clone(),
            label.map.clone(),
            label.difficulty as u8,
            label.stance as u8,
            label.run,
            label.pairing,
        );
        let describe = || {
            format!(
                "{} {} {} {} run {} {:?} {:?}",
                label.manifest,
                label.map,
                label.difficulty,
                label.stance,
                label.run,
                label.pairing,
                row.leg
            )
        };
        let teams: Vec<u8> = row.seats.iter().map(|seat| seat.team).collect();
        let mode = MatchMode::of(&teams).with_context(describe)?;
        let places = doubled_places(row);
        let ledgers: Vec<Option<&SeatLedger>> = row
            .evidence
            .iter()
            .map(|evidence| evidence.ledger.as_ref())
            .collect();
        let pair = format!(
            "{} {} {} {} {}",
            label.manifest, label.map, label.difficulty, label.stance, label.run
        );
        let leg_outcome = match label.pairing {
            Pairing::HeadToHead | Pairing::Mixed => {
                if label.pairing == Pairing::Mixed {
                    mixed_legs += 1;
                } else {
                    head_to_head_legs += 1;
                }
                let slot = match row.leg {
                    EvaluationLeg::Forward => 0,
                    EvaluationLeg::Swapped => 1,
                    EvaluationLeg::Single => bail!("compared leg {} is unpaired", describe()),
                };
                let outcome = outcome(row, &places).with_context(describe)?;
                let (_, _, pair) = pairs.entry(key).or_insert((label, mode, [None, None]));
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
            let builder = groups.entry((mode, group)).or_default();
            let legs = match label.pairing {
                Pairing::HeadToHead => &mut builder.head_to_head.legs,
                Pairing::Mixed => &mut builder.mixed.legs,
                Pairing::Baseline => &mut builder.baseline,
            };
            legs.legs += 1;
            legs.decided += u32::from(row.termination == Termination::Decided);
            legs.stall_loops += u32::from(row.termination == Termination::StallLoop);
            let compared = builder.compared(label.pairing);
            match leg_outcome {
                Some(LegOutcome::NewWin) => compared.new_wins += 1,
                Some(LegOutcome::OldWin) => compared.old_wins += 1,
                Some(LegOutcome::NoWinner) | None => {}
            }
            for ((seat, evidence), &place) in row.seats.iter().zip(&row.evidence).zip(&places) {
                if seat.controller == EvaluationControllerKind::None {
                    continue;
                }
                let controller = builder.controllers.entry(seat.controller).or_default();
                controller.seat_legs += 1;
                controller.repeated_orders += evidence.failures.repeated_orders.incidents;
                controller.abandoned_sites += evidence.failures.abandoned_sites.incidents;
                controller.starved_production += evidence.failures.starved_production.incidents;
                controller.stuck_missions += evidence.failures.stuck_missions.incidents;
                if let Some(idle) = &evidence.failures.idle_army {
                    controller.idle_army += idle.incidents;
                    controller.idle_army_seat_legs += 1;
                }
                if let Some(deliveries) = evidence.deliveries {
                    controller.deliveries.delivered += deliveries.delivered;
                    controller.deliveries.lost += deliveries.lost;
                    controller.deliveries.undelivered += deliveries.undelivered;
                    controller.deliveries_seat_legs += 1;
                }
                if let Some(reactivity) = &evidence.reactivity {
                    controller
                        .reactivity
                        .get_or_insert_with(SeatReactivity::default)
                        .merge(reactivity);
                    controller.reactivity_seat_legs += 1;
                    controller.reactivity_ticks +=
                        evidence.eliminated_at.unwrap_or(row.duration_ticks);
                }
                for sample in &evidence.income {
                    controller
                        .income
                        .entry(sample.tick)
                        .or_default()
                        .push((sample.actual_per_minute, sample.saturation_per_minute));
                }
                if let Some(ledger) = &evidence.ledger {
                    controller.ledger.add(ledger);
                }
                if let Some(attacks) = &evidence.attacks {
                    controller
                        .attacks
                        .get_or_insert_with(AttackCalibration::default)
                        .add(attacks);
                }
                if label.pairing == Pairing::HeadToHead
                    && let Some(shares) = team_shares(&ledgers, &teams, seat.team)
                {
                    controller
                        .worth
                        .entry(pair.clone())
                        .or_default()
                        .push(shares);
                }
                if label.pairing == Pairing::Mixed {
                    controller.doubled_places.push(place);
                    controller
                        .survival
                        .push(evidence.eliminated_at.unwrap_or(row.duration_ticks));
                }
            }
        }
    }
    for (label, mode, legs) in pairs.values() {
        let [Some(forward), Some(swapped)] = *legs else {
            bail!(
                "{:?} pair {} {} {} {} run {} is incomplete",
                label.pairing,
                label.manifest,
                label.map,
                label.difficulty,
                label.stance,
                label.run
            );
        };
        let wins = |side| u32::from(forward == side) + u32::from(swapped == side);
        for group in GroupKey::of(label) {
            let builder = groups.get_mut(&(*mode, group)).expect("rows opened groups");
            let tally = &mut builder.compared(label.pairing).pairs;
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
        mixed_legs,
        baseline_legs,
        references: provenance(rows, |row| row.reference_digest.clone()),
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

/// Each seat's place doubled, so 2 is the last seat standing: a seat is
/// placed behind every seat that outlasted it, and tied seats share the mean
/// of their places. Survivors tie ahead of every eliminated seat.
fn doubled_places(row: &ScoredRow) -> Vec<u64> {
    let lasted: Vec<u64> = row
        .evidence
        .iter()
        .map(|evidence| evidence.eliminated_at.unwrap_or(u64::MAX))
        .collect();
    lasted
        .iter()
        .map(|&mine| {
            let ahead = lasted.iter().filter(|&&other| other > mine).count() as u64;
            let tied = lasted.iter().filter(|&&other| other == mine).count() as u64;
            2 * ahead + tied + 1
        })
        .collect()
}

/// Which bot won a compared leg. A head-to-head leg goes to the winning side.
/// A mixed leg goes to the bot whose seats have the better mean place, which
/// is the bot whose seats outlast the other's more often; a leg stopped by a
/// stall loop is level.
fn outcome(row: &ScoredRow, doubled_places: &[u64]) -> Result<LegOutcome> {
    let side = |controller| {
        row.seats
            .iter()
            .filter(|seat| seat.controller == controller)
            .collect::<Vec<_>>()
    };
    let (new, old) = (
        side(EvaluationControllerKind::Opponent),
        side(EvaluationControllerKind::Scripted),
    );
    ensure!(
        !new.is_empty() && !old.is_empty() && new.len() + old.len() == row.seats.len(),
        "a compared leg needs oxide-opponent and oxide-bot and no empty seat"
    );
    if row.matrix.pairing == Pairing::HeadToHead {
        let team = |seats: &[&ScoredSeat]| {
            seats
                .iter()
                .all(|seat| seat.team == seats[0].team)
                .then_some(seats[0].team)
        };
        ensure!(
            matches!((team(&new), team(&old)), (Some(new), Some(old)) if new != old),
            "a head-to-head leg needs oxide-opponent on one team and oxide-bot on the other"
        );
        let won = |seats: &[&ScoredSeat]| {
            matches!(row.result, Some(GameResult::Victory { .. }))
                && row.winner_seats.contains(&seats[0].seat)
        };
        return Ok(if won(&new) {
            LegOutcome::NewWin
        } else if won(&old) {
            LegOutcome::OldWin
        } else {
            LegOutcome::NoWinner
        });
    }
    if row.termination == Termination::StallLoop {
        return Ok(LegOutcome::NoWinner);
    }
    let sum = |seats: &[&ScoredSeat]| -> u64 {
        seats
            .iter()
            .map(|seat| doubled_places[usize::from(seat.seat)])
            .sum()
    };
    let (new_seats, old_seats) = (new.len() as u64, old.len() as u64);
    Ok(
        match (sum(&new) * old_seats).cmp(&(sum(&old) * new_seats)) {
            Ordering::Less => LegOutcome::NewWin,
            Ordering::Greater => LegOutcome::OldWin,
            Ordering::Equal => LegOutcome::NoWinner,
        },
    )
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

fn finish((mode, key): (MatchMode, GroupKey), builder: GroupBuilder) -> GroupReport {
    let mut controllers: Vec<ControllerTally> = builder
        .controllers
        .into_iter()
        .map(|(controller, tally)| ControllerTally {
            controller,
            seat_legs: tally.seat_legs,
            repeated_orders: tally.repeated_orders,
            abandoned_sites: tally.abandoned_sites,
            starved_production: tally.starved_production,
            stuck_missions: tally.stuck_missions,
            idle_army: tally.idle_army,
            idle_army_seat_legs: tally.idle_army_seat_legs,
            deliveries: tally.deliveries,
            deliveries_seat_legs: tally.deliveries_seat_legs,
            reactivity: tally.reactivity,
            reactivity_seat_legs: tally.reactivity_seat_legs,
            reactivity_ticks: tally.reactivity_ticks,
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
            placement: (!tally.doubled_places.is_empty()).then(|| Placement {
                seat_legs: tally.doubled_places.len() as u32,
                mean_place: tally.doubled_places.iter().sum::<u64>() as f64
                    / (2 * tally.doubled_places.len()) as f64,
                median_survival_ticks: median(tally.survival).unwrap_or(0),
            }),
            worth: worth_shares(&tally.worth),
            ledger: tally.ledger,
            attacks: tally.attacks,
        })
        .collect();
    controllers.sort_by_key(|tally| std::cmp::Reverse(tally.controller));
    GroupReport {
        mode,
        group: key.name(),
        head_to_head: builder.head_to_head.finish(),
        mixed: builder.mixed.finish(),
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
            "bot-matrix: {} head-to-head legs, {} mixed legs, {} baseline legs",
            self.head_to_head_legs, self.mixed_legs, self.baseline_legs
        );
        for (label, values) in [
            ("reference", &self.references),
            ("build", &self.builds),
            ("candidate", &self.candidates),
        ] {
            for value in values {
                let _ = writeln!(out, "{label:<10} {} ({} legs)", value.value, value.legs);
            }
        }
        if self.head_to_head_legs > 0 {
            self.render_pairing(
                &mut out,
                "head to head: pairs new-both/split/old-both/undecided; new share of won legs; decided legs",
                |group| &group.head_to_head,
            );
        }
        if self.mixed_legs > 0 {
            self.render_pairing(
                &mut out,
                "mixed: pairs new-both/split/old-both/level; new share of legs whose seats outlasted the other bot's; decided legs",
                |group| &group.mixed,
            );
            let _ = writeln!(
                out,
                "\nplacement in mixed legs: mean place (1 is last standing); median survival ticks"
            );
            let _ = writeln!(
                out,
                "{:<13} {:<22} {:<10} {:>9} {:>10} {:>15}",
                "mode", "group", "controller", "seat-legs", "mean place", "median survival"
            );
            for group in &self.groups {
                for tally in &group.controllers {
                    let Some(placement) = &tally.placement else {
                        continue;
                    };
                    let _ = writeln!(
                        out,
                        "{:<13} {:<22} {:<10} {:>9} {:>10.2} {:>15}",
                        group.mode.as_str(),
                        group.group,
                        controller_name(tally.controller),
                        placement.seat_legs,
                        placement.mean_place,
                        placement.median_survival_ticks
                    );
                }
            }
        }
        let _ = writeln!(out, "\nfailure incidents");
        let _ = writeln!(
            out,
            "{:<13} {:<22} {:<10} {:>9} {:>16} {:>16} {:>18} {:>15} {:>20}",
            "mode",
            "group",
            "controller",
            "seat-legs",
            "repeated orders",
            "abandoned sites",
            "starved production",
            "stuck missions",
            "idle army"
        );
        for group in &self.groups {
            for tally in &group.controllers {
                let _ = writeln!(
                    out,
                    "{:<13} {:<22} {:<10} {:>9} {:>16} {:>16} {:>18} {:>15} {:>20}",
                    group.mode.as_str(),
                    group.group,
                    controller_name(tally.controller),
                    tally.seat_legs,
                    tally.repeated_orders,
                    tally.abandoned_sites,
                    tally.starved_production,
                    tally.stuck_missions,
                    measured(tally.idle_army, tally.idle_army_seat_legs, tally.seat_legs)
                );
            }
        }
        let severed = |tally: &ControllerTally| {
            let deliveries = tally.deliveries;
            deliveries.delivered + deliveries.lost + deliveries.undelivered
        };
        if self
            .groups
            .iter()
            .flat_map(|group| &group.controllers)
            .any(|tally| severed(tally) > 0)
        {
            let _ = writeln!(
                out,
                "\narmed ground units trained on severed ground, scrap: delivered/lost/undelivered (seat-legs recorded)"
            );
            let _ = writeln!(
                out,
                "{:<13} {:<22} {:<10} {:>30}",
                "mode", "group", "controller", "deliveries"
            );
            for group in &self.groups {
                for tally in group.controllers.iter().filter(|tally| severed(tally) > 0) {
                    let deliveries = tally.deliveries;
                    let _ = writeln!(
                        out,
                        "{:<13} {:<22} {:<10} {:>30}",
                        group.mode.as_str(),
                        group.group,
                        controller_name(tally.controller),
                        format!(
                            "{}/{}/{} ({}/{})",
                            deliveries.delivered,
                            deliveries.lost,
                            deliveries.undelivered,
                            tally.deliveries_seat_legs,
                            tally.seat_legs
                        )
                    );
                }
            }
        }
        self.render_reactivity(&mut out);
        self.render_ledger(&mut out);
        let _ = writeln!(
            out,
            "\nincome per minute: median percent of saturation (actual/saturation, samples)"
        );
        let _ = write!(out, "{:<13} {:<22} {:<10}", "mode", "group", "controller");
        for tick in INCOME_CHECKPOINTS {
            let _ = write!(out, " {:<22}", format!("tick {tick}"));
        }
        let _ = writeln!(out);
        for group in &self.groups {
            for tally in &group.controllers {
                let _ = write!(
                    out,
                    "{:<13} {:<22} {:<10}",
                    group.mode.as_str(),
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

    /// Each controller's situations over the whole of each mode: how many
    /// arose, the share answered, those missed and moot, and the mean ticks
    /// to an answer.
    fn render_reactivity(&self, out: &mut String) {
        let overall: Vec<(&GroupReport, &ControllerTally, &SeatReactivity)> = self
            .groups
            .iter()
            .filter(|group| group.group == "overall")
            .flat_map(|group| {
                group
                    .controllers
                    .iter()
                    .filter_map(move |tally| Some((group, tally, tally.reactivity.as_ref()?)))
            })
            .collect();
        if overall.is_empty() {
            return;
        }
        let _ = writeln!(
            out,
            "\nreactivity, overall: situations that arose, the share answered in time, missed, moot, mean ticks to answer"
        );
        let _ = writeln!(
            out,
            "{:<13} {:<10} {:<15} {:>7} {:>9} {:>7} {:>7} {:>11}",
            "mode", "controller", "situation", "arose", "answered", "missed", "moot", "mean ticks"
        );
        for (group, tally, reactivity) in overall {
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
                    "{:<13} {:<10} {:<15} {:>7} {:>9} {:>7} {:>7} {:>11}",
                    group.mode.as_str(),
                    controller_name(tally.controller),
                    name,
                    reactions.arose,
                    answered,
                    reactions.missed,
                    reactions.moot,
                    mean
                );
            }
            if let Some(switches) = reactivity.target_switches
                && tally.reactivity_ticks > 0
            {
                let _ = writeln!(
                    out,
                    "{:<13} {:<10} target switches per 10k ticks: {:.2} ({} seat-legs)",
                    group.mode.as_str(),
                    controller_name(tally.controller),
                    switches as f64 * 10_000.0 / tally.reactivity_ticks as f64,
                    tally.reactivity_seat_legs
                );
            }
        }
    }

    /// Each controller's ledger over the whole of each mode: its side's
    /// share of net worth in head-to-head pairs, how its attacks went
    /// against what it believed, and its most-bought units and buildings.
    fn render_ledger(&self, out: &mut String) {
        for group in self.groups.iter().filter(|group| group.group == "overall") {
            for tally in &group.controllers {
                if tally.ledger.seats == 0 {
                    continue;
                }
                let _ = writeln!(
                    out,
                    "\nledger, {} {}: {} of {} seat-legs recorded",
                    group.mode.as_str(),
                    controller_name(tally.controller),
                    tally.ledger.seats,
                    tally.seat_legs
                );
                if !tally.worth.is_empty() {
                    let _ = writeln!(
                        out,
                        "  its side's share of net worth, head-to-head pairs: {}",
                        render_shares(&tally.worth)
                    );
                }
                if let Some(attacks) = &tally.attacks {
                    let _ = writeln!(
                        out,
                        "  attacks, by strength sent against the known defense:"
                    );
                    attacks.render(out, "    ");
                }
                tally.ledger.render(out, "  ", Some(LEDGER_KINDS));
            }
        }
    }

    fn render_pairing(
        &self,
        out: &mut String,
        title: &str,
        pairing: fn(&GroupReport) -> &PairingReport,
    ) {
        let _ = writeln!(out, "\n{title}");
        let _ = writeln!(
            out,
            "{:<13} {:<22} {:<13} {:<30} {:<18} {:<18} stall loops",
            "mode",
            "group",
            "pairs",
            "new share (95% Wilson)",
            "decided new-old",
            "decided old-old"
        );
        for group in &self.groups {
            let report = pairing(group);
            if report.legs.legs == 0 {
                continue;
            }
            let pairs = &report.pairs;
            let share = report.new_share.as_ref().map_or_else(
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
                "{:<13} {:<22} {:<13} {:<30} {:<18} {:<18} {}/{}",
                group.mode.as_str(),
                group.group,
                format!(
                    "{}/{}/{}/{}",
                    pairs.new_wins_both, pairs.split, pairs.old_wins_both, pairs.undecided
                ),
                share,
                decided(&report.legs),
                decided(&group.baseline),
                report.legs.stall_loops,
                group.baseline.stall_loops,
            );
        }
    }
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
            eliminated_at: None,
            failures: SeatFailures {
                starved_production: FailureTally {
                    incidents: starved,
                    examples: Vec::new(),
                },
                stuck_missions: FailureTally {
                    incidents: starved,
                    examples: Vec::new(),
                },
                ..SeatFailures::default()
            },
            deliveries: None,
            income,
            reactivity: None,
            ledger: None,
            attacks: None,
        }
    }

    const N: EvaluationControllerKind = EvaluationControllerKind::Opponent;
    const O: EvaluationControllerKind = EvaluationControllerKind::Scripted;

    /// A leg of 1,000 ticks: each seat's controller, team and elimination
    /// tick, and the winning team, which decides the leg.
    fn leg(
        label: MatrixLabel,
        leg: EvaluationLeg,
        seats: &[(EvaluationControllerKind, u8, Option<u64>)],
        winner: Option<u8>,
    ) -> ScoredRow {
        ScoredRow {
            matrix: label,
            candidate: "unit".into(),
            reference_digest: "fnv1a64:1".into(),
            build: build(),
            leg,
            termination: if winner.is_some() {
                Termination::Decided
            } else {
                Termination::TickLimit
            },
            result: winner.map(|team| GameResult::Victory { team }),
            winner_seats: (0..seats.len() as u8)
                .filter(|&seat| Some(seats[usize::from(seat)].1) == winner)
                .collect(),
            duration_ticks: 1_000,
            seats: seats
                .iter()
                .enumerate()
                .map(|(seat, &(controller, team, _))| ScoredSeat {
                    seat: seat as u8,
                    team,
                    controller,
                })
                .collect(),
            evidence: seats
                .iter()
                .enumerate()
                .map(|(seat, &(_, _, eliminated_at))| ScoredEvidence {
                    eliminated_at,
                    ..evidence(seat as u8, 0, Vec::new())
                })
                .collect(),
        }
    }

    /// A head-to-head leg; `winner` is the winning controller, if any.
    fn duel(
        label: MatrixLabel,
        leg_kind: EvaluationLeg,
        winner: Option<EvaluationControllerKind>,
    ) -> ScoredRow {
        let controllers = match leg_kind {
            EvaluationLeg::Swapped => [O, N],
            _ => [N, O],
        };
        let lost = |seat: usize| {
            winner
                .is_some_and(|winner| controllers[seat] != winner)
                .then_some(999)
        };
        let mut row = leg(
            label,
            leg_kind,
            &[(controllers[0], 0, lost(0)), (controllers[1], 1, lost(1))],
            winner.map(|winner| u8::from(controllers[1] == winner)),
        );
        row.evidence[0] = ScoredEvidence {
            eliminated_at: lost(0),
            ..evidence(
                0,
                1,
                vec![IncomeSample {
                    tick: 6_000,
                    actual_per_minute: 300,
                    saturation_per_minute: 600,
                }],
            )
        };
        row.evidence[1] = ScoredEvidence {
            eliminated_at: lost(1),
            ..evidence(1, 2, Vec::new())
        };
        row
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

        assert_eq!(
            (
                report.head_to_head_legs,
                report.mixed_legs,
                report.baseline_legs
            ),
            (8, 0, 2)
        );
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
        assert!(
            report
                .groups
                .iter()
                .all(|group| group.mode == MatchMode::Duel)
        );
        assert_eq!(
            overall.head_to_head.pairs,
            PairTally {
                new_wins_both: 1,
                split: 1,
                old_wins_both: 1,
                undecided: 1,
            }
        );
        let share = overall.head_to_head.new_share.as_ref().unwrap();
        assert_eq!((share.new_wins, share.old_wins), (4, 3));
        assert_eq!(share.wilson, crate::sweep::wilson(4, 7));
        assert_eq!(
            overall.head_to_head.legs,
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
        assert_eq!(severed.head_to_head.pairs.old_wins_both, 1);
        assert_eq!(severed.head_to_head.pairs.undecided, 1);
        assert_eq!(overall.mixed, PairingReport::default());

        let [new, old] = overall.controllers.as_slice() else {
            panic!("two controllers: {:?}", overall.controllers);
        };
        assert_eq!(new.controller, EvaluationControllerKind::Opponent);
        assert_eq!(new.seat_legs, 8);
        assert_eq!(new.starved_production, 4 + 2 * 4);
        assert_eq!(new.stuck_missions, new.starved_production);
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
        assert!(new.placement.is_none(), "duels have no mixed legs");
        assert_eq!(report.references[0].legs, 10);
        let text = report.render();
        assert!(text.contains("4/7 57%"), "{text}");
        assert!(text.contains("family severed"), "{text}");
        assert!(text.contains("50% (300/600, 4)"), "{text}");
        assert!(!text.contains("placement"), "{text}");
    }

    fn team_label(pairing: Pairing) -> MatrixLabel {
        label("quarry", MapFamily::Open, 0, pairing)
    }

    #[test]
    fn team_maps_score_pure_legs_by_side_and_mixed_legs_by_placement() {
        let rows = vec![
            leg(
                team_label(Pairing::HeadToHead),
                EvaluationLeg::Forward,
                &[
                    (N, 0, None),
                    (N, 0, None),
                    (O, 1, Some(900)),
                    (O, 1, Some(950)),
                ],
                Some(0),
            ),
            leg(
                team_label(Pairing::HeadToHead),
                EvaluationLeg::Swapped,
                &[
                    (O, 0, None),
                    (O, 0, Some(400)),
                    (N, 1, Some(800)),
                    (N, 1, Some(990)),
                ],
                Some(0),
            ),
            // Team zero wins with both bots on it, so the order in which
            // seats fell decides.
            leg(
                team_label(Pairing::Mixed),
                EvaluationLeg::Forward,
                &[
                    (N, 0, None),
                    (O, 0, Some(200)),
                    (O, 1, Some(800)),
                    (N, 1, Some(300)),
                ],
                Some(0),
            ),
            leg(
                team_label(Pairing::Mixed),
                EvaluationLeg::Swapped,
                &[
                    (O, 0, Some(100)),
                    (N, 0, None),
                    (N, 1, Some(400)),
                    (O, 1, Some(900)),
                ],
                Some(0),
            ),
            leg(
                team_label(Pairing::Baseline),
                EvaluationLeg::Single,
                &[(O, 0, None), (O, 0, None), (O, 1, None), (O, 1, None)],
                None,
            ),
        ];
        let report = build_report(&rows).unwrap();

        assert_eq!(
            (
                report.head_to_head_legs,
                report.mixed_legs,
                report.baseline_legs
            ),
            (2, 2, 1)
        );
        let overall = &report.groups[0];
        assert_eq!(
            (overall.mode, overall.group.as_str()),
            (MatchMode::Teams, "overall")
        );
        assert_eq!(overall.head_to_head.pairs.split, 1);
        assert_eq!(overall.mixed.pairs.new_wins_both, 1);
        let share = overall.mixed.new_share.as_ref().unwrap();
        assert_eq!((share.new_wins, share.old_wins), (2, 0));
        assert_eq!(overall.mixed.legs.decided, 2);
        assert_eq!(overall.baseline.decided, 0);

        let [new, old] = overall.controllers.as_slice() else {
            panic!("two controllers: {:?}", overall.controllers);
        };
        assert_eq!(
            new.placement,
            Some(Placement {
                seat_legs: 4,
                mean_place: 2.0,
                median_survival_ticks: 1_000,
            })
        );
        let old = old.placement.as_ref().unwrap();
        assert_eq!((old.mean_place, old.median_survival_ticks), (3.0, 800));
        let text = report.render();
        assert!(text.contains("mixed: pairs"), "{text}");
        assert!(text.contains("placement in mixed legs"), "{text}");
    }

    fn ffa_label(map: &str, run: u64) -> MatrixLabel {
        label(map, MapFamily::Open, run, Pairing::Mixed)
    }

    #[test]
    fn free_for_all_legs_go_to_the_bot_whose_seats_outlast_the_other() {
        let rows = vec![
            // Survivors tie for first and the fallen tie behind them.
            leg(
                ffa_label("four", 0),
                EvaluationLeg::Forward,
                &[
                    (N, 0, None),
                    (O, 1, None),
                    (N, 2, Some(100)),
                    (O, 3, Some(100)),
                ],
                None,
            ),
            // The old bot's tied seats sit between the new bot's first and
            // last: each bot's seats outlast the other's equally often.
            leg(
                ffa_label("four", 0),
                EvaluationLeg::Swapped,
                &[
                    (N, 0, None),
                    (N, 1, Some(100)),
                    (O, 2, Some(150)),
                    (O, 3, Some(150)),
                ],
                None,
            ),
            // Unequal sides: one old seat outlasting two new ones wins.
            leg(
                ffa_label("three", 0),
                EvaluationLeg::Forward,
                &[(N, 0, Some(200)), (O, 1, None), (N, 2, Some(100))],
                Some(1),
            ),
            leg(
                ffa_label("three", 0),
                EvaluationLeg::Swapped,
                &[(O, 0, None), (N, 1, Some(50)), (O, 2, Some(60))],
                None,
            ),
        ];
        let mut stalled = leg(
            ffa_label("three", 1),
            EvaluationLeg::Forward,
            &[(N, 0, None), (O, 1, Some(10)), (N, 2, None)],
            None,
        );
        stalled.termination = Termination::StallLoop;
        let rows = [
            rows,
            vec![
                stalled,
                leg(
                    ffa_label("three", 1),
                    EvaluationLeg::Swapped,
                    &[(O, 0, Some(10)), (N, 1, None), (O, 2, Some(10))],
                    Some(1),
                ),
            ],
        ]
        .concat();
        let report = build_report(&rows).unwrap();

        let overall = &report.groups[0];
        assert_eq!(overall.mode, MatchMode::FreeForAll);
        assert_eq!(
            overall.mixed.pairs,
            PairTally {
                new_wins_both: 0,
                split: 0,
                old_wins_both: 1,
                undecided: 2,
            }
        );
        let share = overall.mixed.new_share.as_ref().unwrap();
        assert_eq!((share.new_wins, share.old_wins), (1, 2));
        assert_eq!(overall.mixed.legs.stall_loops, 1);
    }

    #[test]
    fn malformed_compared_legs_are_refused() {
        let four = |pairing, seats: [(EvaluationControllerKind, u8); 4]| {
            let seats = seats.map(|(controller, team)| (controller, team, None));
            [EvaluationLeg::Forward, EvaluationLeg::Swapped]
                .map(|leg_kind| leg(team_label(pairing), leg_kind, &seats, None))
                .to_vec()
        };
        let cases = [
            (
                "one team and oxide-bot on the other",
                four(Pairing::HeadToHead, [(N, 0), (O, 0), (N, 1), (O, 1)]),
            ),
            (
                "needs oxide-opponent and oxide-bot",
                four(Pairing::Mixed, [(O, 0), (O, 0), (O, 1), (O, 1)]),
            ),
            (
                "neither a duel",
                four(Pairing::Mixed, [(N, 0), (O, 0), (N, 0), (O, 1)]),
            ),
        ];
        for (expected, rows) in cases {
            let error = format!("{:#}", build_report(&rows).unwrap_err());
            assert!(error.contains(expected), "{expected}: {error}");
        }

        let mut rows = four(Pairing::Mixed, [(N, 0), (O, 0), (O, 1), (N, 1)]);
        rows.pop();
        let error = build_report(&rows).unwrap_err().to_string();
        assert!(error.contains("incomplete"), "{error}");
        let mut rows = four(Pairing::Mixed, [(N, 0), (O, 0), (O, 1), (N, 1)]);
        rows.push(rows[1].clone());
        let error = build_report(&rows).unwrap_err().to_string();
        assert!(error.contains("repeats"), "{error}");
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
    fn diagnostics_count_only_the_rows_that_record_them() {
        let mut rows = pair("a", MapFamily::Severed, 0, [NEW, OLD]);
        rows.push(baseline(
            label("a", MapFamily::Severed, 0, Pairing::Baseline),
            false,
        ));
        let severed = Deliveries {
            delivered: 90,
            lost: 180,
            undelivered: 900,
        };
        rows[0].evidence[0].failures.idle_army = Some(FailureTally {
            incidents: 2,
            examples: Vec::new(),
        });
        rows[0].evidence[0].deliveries = Some(severed);
        rows[0].evidence[1].failures.idle_army = Some(FailureTally::default());
        rows[0].evidence[1].deliveries = Some(Deliveries::default());
        let report = build_report(&rows).unwrap();

        let [new, old] = report.groups[0].controllers.as_slice() else {
            panic!("two controllers: {:?}", report.groups[0].controllers);
        };
        assert_eq!(
            (new.idle_army, new.idle_army_seat_legs, new.seat_legs),
            (2, 1, 2)
        );
        assert_eq!(
            (old.idle_army, old.idle_army_seat_legs, old.seat_legs),
            (0, 1, 4)
        );
        assert_eq!((new.deliveries, new.deliveries_seat_legs), (severed, 1));
        let text = report.render();
        assert!(text.contains("2 (1/2 seat-legs)"), "{text}");
        assert!(text.contains("0 (1/4 seat-legs)"), "{text}");
        assert!(text.contains("90/180/900 (1/2)"), "{text}");

        let unmeasured = build_report(&pair("a", MapFamily::Open, 0, [NEW, OLD]))
            .unwrap()
            .render();
        assert!(!unmeasured.contains("severed ground"), "{unmeasured}");
    }

    #[test]
    fn reactivity_sums_over_the_rows_that_record_it_and_renders_overall() {
        let mut rows = pair("a", MapFamily::Open, 0, [NEW, OLD]);
        let found = SeatReactivity {
            ground_defense: crate::bot_eval::Reactions {
                arose: 4,
                answered: 3,
                missed: 1,
                answer_ticks: 90,
                ..Default::default()
            },
            withdrawal: Some(crate::bot_eval::Reactions {
                arose: 2,
                answered: 2,
                answer_ticks: 400,
                ..Default::default()
            }),
            target_switches: Some(1),
            ..Default::default()
        };
        rows[0].evidence[0].reactivity = Some(found.clone());
        rows[1].evidence[1].reactivity = Some(found);
        let report = build_report(&rows).unwrap();
        let new = report.groups[0]
            .controllers
            .iter()
            .find(|tally| tally.controller == EvaluationControllerKind::Opponent)
            .unwrap();
        assert_eq!(new.reactivity_seat_legs, 2);
        let summed = new.reactivity.as_ref().unwrap();
        assert_eq!(
            (summed.ground_defense.arose, summed.ground_defense.answered),
            (8, 6)
        );
        let text = report.render();
        assert!(text.contains("reactivity, overall"), "{text}");
        assert!(
            text.lines().any(|line| line.contains("ground defense")
                && line.contains("75%")
                && line.contains(" 30")),
            "{text}"
        );
        assert!(text.contains("withdrawal"), "{text}");
        assert!(text.contains("target switches per 10k ticks"), "{text}");

        let unmeasured = build_report(&pair("a", MapFamily::Open, 0, [NEW, OLD]))
            .unwrap()
            .render();
        assert!(!unmeasured.contains("reactivity"), "{unmeasured}");
    }

    #[test]
    fn rows_recorded_before_the_reactivity_detectors_still_load() {
        let evidence: ScoredEvidence = serde_json::from_value(serde_json::json!({
            "seat": 0,
            "eliminated_at": null,
            "failures": {
                "repeated_orders": {"incidents": 0, "examples": []},
                "abandoned_sites": {"incidents": 0, "examples": []},
                "starved_production": {"incidents": 0, "examples": []},
            },
            "income": [],
        }))
        .unwrap();
        assert!(evidence.reactivity.is_none());
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

    #[test]
    fn ledgers_pool_by_controller_and_worth_compares_head_to_head_pairs() {
        let ledger = |worth: u64, sentinels: u64| crate::ledger::SeatLedger {
            units: [(
                "sentinel".to_owned(),
                crate::ledger::KindLedger {
                    built: sentinels,
                    paid: 90 * sentinels,
                    ..Default::default()
                },
            )]
            .into(),
            worth: vec![worth; 6],
            ..Default::default()
        };
        let mut rows = Vec::new();
        for (leg, run) in [(EvaluationLeg::Forward, 0), (EvaluationLeg::Swapped, 0)] {
            let mut row = duel(
                label("skirmish", MapFamily::Open, run, Pairing::HeadToHead),
                leg,
                Some(N),
            );
            let new = usize::from(leg == EvaluationLeg::Swapped);
            row.evidence[new].ledger = Some(ledger(600, 3));
            row.evidence[1 - new].ledger = Some(ledger(400, 2));
            row.evidence[new].attacks = Some(AttackCalibration::default());
            rows.push(row);
        }
        rows.extend([EvaluationLeg::Forward, EvaluationLeg::Swapped].map(|leg| {
            duel(
                label("skirmish", MapFamily::Open, 1, Pairing::HeadToHead),
                leg,
                None,
            )
        }));
        let report = build_report(&rows).unwrap();
        let overall = report
            .groups
            .iter()
            .find(|group| group.group == "overall")
            .unwrap();
        let tally = |controller| {
            overall
                .controllers
                .iter()
                .find(|tally| tally.controller == controller)
                .unwrap()
        };
        let (new, old) = (tally(N), tally(O));
        assert_eq!(
            (new.ledger.seats, new.seat_legs),
            (2, 4),
            "rows without a ledger are unmeasured"
        );
        assert_eq!(new.ledger.units["sentinel"].built, 6);
        assert_eq!(old.ledger.units["sentinel"].built, 4);
        assert!(new.attacks.is_some() && old.attacks.is_none());
        assert_eq!(
            new.worth
                .iter()
                .map(|share| (share.tick, share.pairs))
                .collect::<Vec<_>>(),
            crate::ledger::WORTH_TICKS.map(|tick| (tick, 1)),
            "a match that ended sooner carries its final worth"
        );
        assert!(
            new.worth
                .iter()
                .all(|share| (share.mean - 0.6).abs() < 1e-9)
        );
        assert!((old.worth[0].mean - 0.4).abs() < 1e-9);
        let rendered = report.render();
        assert!(
            rendered.contains("its side's share of net worth"),
            "{rendered}"
        );
        assert!(rendered.contains("sentinel"), "{rendered}");
    }
}
