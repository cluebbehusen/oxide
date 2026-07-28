//! Composition telemetry: what a match's armies were actually made of,
//! cost-weighted — the balance review's measuring stick. The optimizer
//! is a balance probe: a unit the bot spams implicates that unit's
//! tuning, a unit it never fields raises a question (though absence is
//! the weaker signal — some units are merely hard to learn).
//!
//! A record also states the terms it was measured under: how the match
//! ended, whether the tick cap ended it, and the last sample on which
//! anything moved. A capped stalemate's army mix is evidence about a
//! stalemate, not about army choice, and a reader that cannot tell the
//! two apart draws the wrong conclusion from the same number.
//!
//! Buildings ride beside units because the tech tree is a construction
//! decision: a roster that never stands a Fabricator never had the
//! advanced kinds available to choose, and the unit shares alone cannot
//! say which of the two happened.

use crate::runner;
use anyhow::{Result, ensure};
use oxide_sim::{BuildingId, BuildingKind, Faction, GameResult, Scenario, UnitKind};
use std::collections::{BTreeMap, BTreeSet};

/// One match's cost-weighted composition, per seat.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MatchComposition {
    /// Scenario name.
    pub scenario: String,
    /// The map's declared pace label — its size class, and the cohort
    /// key for "does this skew belong to one map class". Empty when the
    /// scenario carries no metadata.
    pub pace: String,
    /// Seed the match ran under.
    pub seed: u64,
    /// Per seat: unit-kind name -> integrated cost-share of that seat's
    /// army over the sampled match (shares sum to 1 per seat that ever
    /// fielded a unit).
    pub seats: Vec<BTreeMap<String, f64>>,
    /// Per seat: Shannon entropy (bits) of that seat's OWN mix. Two
    /// seats each spamming a different kind average to a mix that looks
    /// diverse; only the per-seat figures show the spam.
    pub entropy_bits: Vec<f64>,
    /// Per seat: building-kind name -> distinct buildings of that kind
    /// seen standing built at any sample. A rebuild after a loss counts
    /// twice (ids never repeat) and a site razed before it finished
    /// counts not at all — the number answers "what did this seat
    /// finish", which is what the tech tree gates on.
    pub buildings: Vec<BTreeMap<String, u32>>,
    /// Per seat: the roster it played, lowercased.
    pub factions: Vec<String>,
    /// Ticks the match ran before a result (or the cap).
    pub ticks: u64,
    /// How the match ended; `None` when the tick cap ended it.
    pub result: Option<GameResult>,
    /// Whether the tick cap ended the match. Carried explicitly because
    /// inferring it from `ticks == max_ticks` is wrong on a match
    /// decided on its final tick.
    pub capped: bool,
    /// Winning seats — empty on a draw and on a capped match.
    pub winners: Vec<u8>,
    /// The last sample at which any seat's army value or standing
    /// building count changed. Sample-resolution, so a frozen tail
    /// reads as `ticks - last_progress_tick`.
    pub last_progress_tick: u64,
}

/// Runs one bot-vs-bot match and integrates each seat's army value by
/// kind: every sample adds `cost` for each living unit, so a unit's
/// share reflects both how many were fielded and how long they lived —
/// the honest "what was this army made of" number.
pub fn sample_match(
    scenario: &Scenario,
    max_ticks: u64,
    sample_every: u64,
) -> Result<MatchComposition> {
    let mut bots = oxide_sim::bot::seat_bots(scenario);
    sample_driven(scenario, max_ticks, sample_every, |state| {
        runner::step(state, &mut bots, None);
    })
}

/// Like [`sample_match`], but the caller drives each tick — how a
/// candidate weights artifact (not yet embedded) gets probed.
pub fn sample_driven(
    scenario: &Scenario,
    max_ticks: u64,
    sample_every: u64,
    mut tick_fn: impl FnMut(&mut oxide_sim::State),
) -> Result<MatchComposition> {
    ensure!(sample_every > 0, "sample stride must be greater than zero");
    let mut state = scenario.build()?;
    let seats = scenario.players.len();
    let mut acc: Vec<BTreeMap<UnitKind, u64>> = vec![BTreeMap::new(); seats];
    let mut standing: Vec<BTreeSet<(BuildingKind, BuildingId)>> = vec![BTreeSet::new(); seats];
    let mut previous: Vec<(u64, usize)> = vec![(0, 0); seats];
    let mut last_progress_tick = 0;
    let mut ran = 0;
    for tick in 0..max_ticks {
        tick_fn(&mut state);
        ran = tick + 1;
        if tick % sample_every == 0 {
            let mut live: Vec<(u64, usize)> = vec![(0, 0); seats];
            for unit in state.units() {
                let seat = unit.player.0 as usize;
                if seat < seats {
                    let cost = u64::from(unit.kind.stats().cost);
                    *acc[seat].entry(unit.kind).or_default() += cost;
                    live[seat].0 += cost;
                }
            }
            for building in state.buildings() {
                let seat = building.player.0 as usize;
                if seat < seats {
                    live[seat].1 += 1;
                    if building.built {
                        standing[seat].insert((building.kind, building.id));
                    }
                }
            }
            if live != previous {
                last_progress_tick = ran;
                previous = live;
            }
        }
        if state.result().is_some() {
            break;
        }
    }
    let shares: Vec<BTreeMap<String, f64>> = acc
        .into_iter()
        .map(|kinds| {
            let total: u64 = kinds.values().sum();
            kinds
                .into_iter()
                .map(|(kind, value)| {
                    (
                        kind.name().to_string(),
                        if total == 0 {
                            0.0
                        } else {
                            value as f64 / total as f64
                        },
                    )
                })
                .collect()
        })
        .collect();
    let entropy_bits = shares.iter().map(|seat| entropy(seat.values())).collect();
    let buildings = standing
        .into_iter()
        .map(|kinds| {
            let mut counts: BTreeMap<String, u32> = BTreeMap::new();
            for (kind, _) in kinds {
                *counts.entry(kind.name().to_string()).or_default() += 1;
            }
            counts
        })
        .collect();
    let result = state.result();
    Ok(MatchComposition {
        scenario: scenario.name.clone(),
        pace: scenario
            .meta
            .as_ref()
            .map_or_else(String::new, |meta| meta.pace.clone()),
        seed: scenario.seed,
        seats: shares,
        entropy_bits,
        buildings,
        factions: state
            .players()
            .iter()
            .map(|p| faction_name(p.faction).to_string())
            .collect(),
        ticks: ran,
        result,
        capped: result.is_none(),
        winners: state.winners().iter().map(|p| p.0).collect(),
        last_progress_tick,
    })
}

/// The roster label the cohort keys speak.
fn faction_name(faction: Faction) -> &'static str {
    match faction {
        Faction::Ferrous => "ferrous",
        Faction::Cupric => "cupric",
    }
}

/// Shannon entropy (bits) of a share series. Zero shares contribute
/// nothing; the series is assumed normalized.
fn entropy<'a>(shares: impl Iterator<Item = &'a f64>) -> f64 {
    -shares
        .filter(|&&p| p > 0.0)
        .map(|p| p * p.log2())
        .sum::<f64>()
}

/// Nearest-rank quantile over an already-sorted series.
fn quantile(sorted: &[f64], num: usize, den: usize) -> f64 {
    sorted[(sorted.len() * num / den).min(sorted.len() - 1)]
}

/// How the per-seat entropies are spread. The mean of a cohort's seats
/// answers "was there variety on average"; `p10` answers "did anyone
/// spam", which is the question a mean cannot be made to answer.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntropySpread {
    /// Mean per-seat entropy (bits).
    pub mean: f64,
    /// Tenth-percentile seat — the spam detector.
    pub p10: f64,
    /// First-quartile seat.
    pub p25: f64,
    /// Median seat.
    pub median: f64,
}

/// Aggregates seat compositions across matches: mean cost-share per
/// kind, plus the Shannon entropy of the mean mix (log base 2) — a
/// one-number spam detector. A roster collapsed onto one kind scores
/// 0; an even spread over 8 kinds scores 3.
///
/// The entropy of the mean is the historical series and stays the
/// headline; it is also the number that hides a seat, so
/// [`EntropySpread`] rides beside it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Aggregate {
    /// Mean cost-share per kind name across every sampled seat.
    pub mean_share: BTreeMap<String, f64>,
    /// Shannon entropy (bits) of the mean mix.
    pub entropy_bits: f64,
    /// The per-seat entropy distribution; `None` when no seat in the
    /// cohort ever fielded a unit.
    pub seat_entropy: Option<EntropySpread>,
    /// Mean count of finished buildings per seat, by kind.
    pub mean_buildings: BTreeMap<String, f64>,
    /// Share of seats that finished at least one of that kind — the
    /// tech-climb reading a mean count blurs.
    pub seats_with_building: BTreeMap<String, f64>,
    /// Seats aggregated.
    pub seats: usize,
    /// Matches contributing at least one seat.
    pub matches: usize,
    /// Contributing matches that reached a result.
    pub decided: usize,
    /// Contributing matches the tick cap ended.
    pub capped: usize,
}

/// Folds match compositions into one mean mix.
pub fn aggregate(matches: &[MatchComposition]) -> Aggregate {
    aggregate_where(matches, |_, _| true)
}

/// Folds the seats a predicate keeps. A seat that never fielded a unit
/// is skipped whatever the predicate says — it has no mix to report —
/// and a match contributes to the counts only through the seats kept.
pub fn aggregate_where(
    matches: &[MatchComposition],
    keep: impl Fn(&MatchComposition, usize) -> bool,
) -> Aggregate {
    let mut sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut building_sum: BTreeMap<String, u32> = BTreeMap::new();
    let mut building_seats: BTreeMap<String, usize> = BTreeMap::new();
    let mut entropies: Vec<f64> = Vec::new();
    let mut seats = 0usize;
    let (mut played, mut decided, mut capped) = (0usize, 0usize, 0usize);
    for m in matches {
        let mut contributed = false;
        for (seat, mix) in m.seats.iter().enumerate() {
            if mix.is_empty() || !keep(m, seat) {
                continue;
            }
            contributed = true;
            seats += 1;
            for (kind, share) in mix {
                *sum.entry(kind.clone()).or_default() += share;
            }
            entropies.push(m.entropy_bits.get(seat).copied().unwrap_or_default());
            for (kind, count) in m.buildings.get(seat).into_iter().flatten() {
                *building_sum.entry(kind.clone()).or_default() += count;
                *building_seats.entry(kind.clone()).or_default() += 1;
            }
        }
        if contributed {
            played += 1;
            if m.capped {
                capped += 1;
            } else {
                decided += 1;
            }
        }
    }
    let per_seat = |v: f64| if seats == 0 { 0.0 } else { v / seats as f64 };
    let mean_share: BTreeMap<String, f64> =
        sum.into_iter().map(|(k, v)| (k, per_seat(v))).collect();
    let entropy_bits = entropy(mean_share.values());
    entropies.sort_by(f64::total_cmp);
    let seat_entropy = (!entropies.is_empty()).then(|| EntropySpread {
        mean: entropies.iter().sum::<f64>() / entropies.len() as f64,
        p10: quantile(&entropies, 1, 10),
        p25: quantile(&entropies, 1, 4),
        median: quantile(&entropies, 1, 2),
    });
    Aggregate {
        mean_share,
        entropy_bits,
        seat_entropy,
        mean_buildings: building_sum
            .into_iter()
            .map(|(k, v)| (k, per_seat(f64::from(v))))
            .collect(),
        seats_with_building: building_seats
            .into_iter()
            .map(|(k, v)| (k, per_seat(v as f64)))
            .collect(),
        seats,
        matches: played,
        decided,
        capped,
    }
}

/// Splits the seats into cohorts by a key, then aggregates each. A seat
/// keyed `None` lands in no cohort; an empty cohort is not reported.
pub fn aggregate_by(
    matches: &[MatchComposition],
    key: impl Fn(&MatchComposition, usize) -> Option<String>,
) -> BTreeMap<String, Aggregate> {
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for m in matches {
        for seat in 0..m.seats.len() {
            keys.extend(key(m, seat));
        }
    }
    keys.into_iter()
        .map(|k| {
            let agg = aggregate_where(matches, |m, seat| key(m, seat).as_deref() == Some(&k));
            (k, agg)
        })
        .collect()
}

/// Cohort key: the roster the seat played.
pub fn by_faction(m: &MatchComposition, seat: usize) -> Option<String> {
    m.factions.get(seat).cloned()
}

/// Cohort key: the map's declared size class.
pub fn by_pace(m: &MatchComposition, _seat: usize) -> Option<String> {
    (!m.pace.is_empty()).then(|| m.pace.clone())
}

/// Cohort key: whether the match reached a result or the cap ended it.
pub fn by_outcome(m: &MatchComposition, _seat: usize) -> Option<String> {
    Some(if m.capped { "capped" } else { "decided" }.to_string())
}

/// Cohort key: the map.
pub fn by_scenario(m: &MatchComposition, _seat: usize) -> Option<String> {
    Some(m.scenario.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a record with everything but the mixes at their neutral
    /// values, so a test states only the fact it is about.
    fn record(seats: Vec<BTreeMap<String, f64>>) -> MatchComposition {
        let entropy_bits = seats.iter().map(|s| entropy(s.values())).collect();
        MatchComposition {
            scenario: "x".into(),
            pace: String::new(),
            seed: 0,
            buildings: vec![BTreeMap::new(); seats.len()],
            factions: vec!["ferrous".into(), "cupric".into()],
            entropy_bits,
            seats,
            ticks: 1,
            result: None,
            capped: true,
            winners: Vec::new(),
            last_progress_tick: 0,
        }
    }

    fn mix(shares: &[(&str, f64)]) -> BTreeMap<String, f64> {
        shares.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    #[test]
    fn a_zero_sample_stride_is_rejected() {
        let scenario = Scenario::skirmish();
        let error = sample_driven(&scenario, 10, 0, |state| {
            state.tick(&[]);
        })
        .expect_err("zero would panic at the modulo operation");
        assert_eq!(error.to_string(), "sample stride must be greater than zero");
    }

    #[test]
    fn a_sampled_skirmish_reports_normalized_shares() {
        let mut scenario = Scenario::skirmish();
        for p in scenario.players.iter_mut() {
            p.bot = true;
        }
        let m = sample_match(&scenario, 600, 20).expect("samples");
        assert_eq!(m.seats.len(), 2);
        for seat in &m.seats {
            if seat.is_empty() {
                continue;
            }
            let total: f64 = seat.values().sum();
            assert!((total - 1.0).abs() < 1e-9, "shares sum to one, got {total}");
        }
    }

    /// The terms of the measurement: an undecided match says so, names
    /// no winner, and reports where the last change happened. Every
    /// seat starts with a Foundry, so the tech reading is a real count
    /// from tick one.
    #[test]
    fn a_capped_skirmish_states_its_terms() {
        let mut scenario = Scenario::skirmish();
        for p in scenario.players.iter_mut() {
            p.bot = true;
        }
        let m = sample_match(&scenario, 400, 20).expect("samples");
        assert_eq!(m.ticks, 400);
        assert!(m.capped && m.result.is_none() && m.winners.is_empty());
        assert!(m.last_progress_tick > 0 && m.last_progress_tick <= m.ticks);
        assert_eq!(m.factions, vec!["ferrous", "cupric"]);
        assert_eq!(m.pace, "standard");
        for seat in &m.buildings {
            assert_eq!(seat.get("foundry"), Some(&1), "each seat holds a Foundry");
        }
        assert_eq!(m.entropy_bits.len(), 2);
    }

    #[test]
    fn entropy_zeroes_on_a_one_kind_army_and_grows_with_spread() {
        let one = record(vec![mix(&[("sentinel", 1.0)])]);
        assert!(aggregate(&[one]).entropy_bits.abs() < 1e-9);
        let spread = record(vec![mix(&[
            ("sentinel", 0.25),
            ("scuttler", 0.25),
            ("lancer", 0.25),
            ("bombard", 0.25),
        ])]);
        let agg = aggregate(&[spread]);
        assert!(
            (agg.entropy_bits - 2.0).abs() < 1e-9,
            "even four-way mix is 2 bits"
        );
    }

    /// The finding the mean cannot report: two seats spamming different
    /// kinds average to a mix that looks diverse, and only the per-seat
    /// floor says otherwise.
    #[test]
    fn two_spamming_seats_average_to_a_diverse_looking_mix() {
        let m = record(vec![mix(&[("sentinel", 1.0)]), mix(&[("scuttler", 1.0)])]);
        let agg = aggregate(&[m]);
        assert!(
            (agg.entropy_bits - 1.0).abs() < 1e-9,
            "the mean of two spams reads as a one-bit mix"
        );
        let spread = agg.seat_entropy.expect("two seats fielded units");
        assert_eq!((spread.mean, spread.p10, spread.median), (0.0, 0.0, 0.0));
    }

    /// Seats that never fielded a unit are not seats; a match all of
    /// whose seats are empty contributes nothing at all.
    #[test]
    fn empty_seats_never_reach_the_fold() {
        let agg = aggregate(&[record(vec![BTreeMap::new(), BTreeMap::new()])]);
        assert_eq!((agg.seats, agg.matches, agg.capped), (0, 0, 0));
        assert!(agg.seat_entropy.is_none() && agg.mean_share.is_empty());
    }

    /// Cohorts are seat-level: one match splits across two faction
    /// cohorts and is counted once in each.
    #[test]
    fn faction_cohorts_split_one_match_by_seat() {
        let m = record(vec![
            mix(&[("sentinel", 1.0)]),
            mix(&[("scuttler", 0.5), ("darter", 0.5)]),
        ]);
        let by = aggregate_by(&[m], by_faction);
        assert_eq!(by.len(), 2);
        let ferrous = &by["ferrous"];
        assert_eq!((ferrous.seats, ferrous.matches, ferrous.capped), (1, 1, 1));
        assert_eq!(ferrous.mean_share["sentinel"], 1.0);
        assert!(by["cupric"].entropy_bits > 0.9);
    }

    /// The outcome cohort is what the fun gate reads: a stalemate's mix
    /// is evidence about a stalemate.
    #[test]
    fn the_outcome_cohort_separates_decided_from_capped() {
        let mut decided = record(vec![mix(&[("sentinel", 1.0)])]);
        decided.capped = false;
        decided.result = Some(GameResult::Victory { team: 0 });
        decided.winners = vec![0];
        let capped = record(vec![mix(&[("harvester", 1.0)])]);
        let by = aggregate_by(&[decided, capped], by_outcome);
        assert_eq!(by["decided"].matches, 1);
        assert_eq!(by["decided"].decided, 1);
        assert_eq!(by["decided"].capped, 0);
        assert!(by["decided"].mean_share.contains_key("sentinel"));
        assert_eq!(by["capped"].capped, 1);
        assert!(by["capped"].mean_share.contains_key("harvester"));
    }

    /// Building counts answer two different questions: how many, and
    /// how many seats got there at all.
    #[test]
    fn building_counts_report_both_the_mean_and_the_reach() {
        let mut m = record(vec![mix(&[("sentinel", 1.0)]), mix(&[("sentinel", 1.0)])]);
        m.buildings[0] = [("foundry".to_string(), 1), ("fabricator".to_string(), 3)]
            .into_iter()
            .collect();
        m.buildings[1] = [("foundry".to_string(), 1)].into_iter().collect();
        let agg = aggregate(&[m]);
        assert_eq!(agg.mean_buildings["foundry"], 1.0);
        assert_eq!(agg.mean_buildings["fabricator"], 1.5);
        assert_eq!(agg.seats_with_building["foundry"], 1.0);
        assert_eq!(agg.seats_with_building["fabricator"], 0.5);
    }

    #[test]
    fn quantiles_take_the_nearest_rank() {
        assert_eq!(quantile(&[7.0], 1, 10), 7.0);
        assert_eq!(quantile(&[1.0, 2.0, 3.0, 4.0], 1, 2), 3.0);
        assert_eq!(quantile(&[0.0, 1.0, 2.0, 3.0, 4.0], 1, 10), 0.0);
    }
}
