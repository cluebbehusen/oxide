//! Composition telemetry: what a match's armies were actually made of,
//! cost-weighted — the balance review's measuring stick. The optimizer
//! is a balance probe: a unit the bot spams implicates that unit's
//! tuning, a unit it never fields raises a question (though absence is
//! the weaker signal — some units are merely hard to learn).

use crate::runner;
use anyhow::{Result, ensure};
use oxide_sim::{Scenario, UnitKind};
use std::collections::BTreeMap;

/// One match's cost-weighted composition, per seat.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MatchComposition {
    /// Scenario name.
    pub scenario: String,
    /// Seed the match ran under.
    pub seed: u64,
    /// Per seat: unit-kind name -> integrated cost-share of that seat's
    /// army over the sampled match (shares sum to 1 per seat that ever
    /// fielded a unit).
    pub seats: Vec<BTreeMap<String, f64>>,
    /// Ticks the match ran before a result (or the cap).
    pub ticks: u64,
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
    let mut ran = 0;
    for tick in 0..max_ticks {
        tick_fn(&mut state);
        ran = tick + 1;
        if tick % sample_every == 0 {
            for unit in state.units() {
                let seat = unit.player.0 as usize;
                if seat < seats {
                    *acc[seat].entry(unit.kind).or_default() += u64::from(unit.kind.stats().cost);
                }
            }
        }
        if state.result().is_some() {
            break;
        }
    }
    let seats = acc
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
    Ok(MatchComposition {
        scenario: scenario.name.clone(),
        seed: scenario.seed,
        seats,
        ticks: ran,
    })
}

/// Aggregates seat compositions across matches: mean cost-share per
/// kind, plus the Shannon entropy of the mean mix (log base 2) — a
/// one-number spam detector. A roster collapsed onto one kind scores
/// 0; an even spread over 8 kinds scores 3.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Aggregate {
    /// Mean cost-share per kind name across every sampled seat.
    pub mean_share: BTreeMap<String, f64>,
    /// Shannon entropy (bits) of the mean mix.
    pub entropy_bits: f64,
    /// Seats aggregated.
    pub seats: usize,
}

/// Folds match compositions into one mean mix.
pub fn aggregate(matches: &[MatchComposition]) -> Aggregate {
    let mut sum: BTreeMap<String, f64> = BTreeMap::new();
    let mut seats = 0usize;
    for m in matches {
        for seat in &m.seats {
            if seat.is_empty() {
                continue;
            }
            seats += 1;
            for (kind, share) in seat {
                *sum.entry(kind.clone()).or_default() += share;
            }
        }
    }
    let mean_share: BTreeMap<String, f64> = sum
        .into_iter()
        .map(|(k, v)| (k, if seats == 0 { 0.0 } else { v / seats as f64 }))
        .collect();
    let entropy_bits = -mean_share
        .values()
        .filter(|&&p| p > 0.0)
        .map(|p| p * p.log2())
        .sum::<f64>();
    Aggregate {
        mean_share,
        entropy_bits,
        seats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn entropy_zeroes_on_a_one_kind_army_and_grows_with_spread() {
        let one = MatchComposition {
            scenario: "x".into(),
            seed: 0,
            seats: vec![[("sentinel".to_string(), 1.0)].into_iter().collect()],
            ticks: 1,
        };
        assert!(aggregate(&[one]).entropy_bits.abs() < 1e-9);
        let spread = MatchComposition {
            scenario: "x".into(),
            seed: 0,
            seats: vec![
                [
                    ("sentinel".to_string(), 0.25),
                    ("scuttler".to_string(), 0.25),
                    ("lancer".to_string(), 0.25),
                    ("bombard".to_string(), 0.25),
                ]
                .into_iter()
                .collect(),
            ],
            ticks: 1,
        };
        let agg = aggregate(&[spread]);
        assert!(
            (agg.entropy_bits - 2.0).abs() < 1e-9,
            "even four-way mix is 2 bits"
        );
    }
}
