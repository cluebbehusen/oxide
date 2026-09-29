//! What the seat could invest in, how much it wants each, and the next
//! purchase toward it. The list is recomputed every decision.

use crate::profile::PersonalityTraits;
use oxide_sim::observation::ObservationData;
use oxide_sim::{BuildingId, BuildingKind};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

/// Lowest score worth starting to save for. A target already being saved
/// for stays while it scores at all.
pub(crate) const ADOPT: u32 = 300;

/// A concrete investment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Investment {
    /// A first building of a tech kind.
    Tech(BuildingKind),
    /// Another Reclaimer.
    Reclaimer,
    /// Upgrading this Reclaimer to a Refinery.
    Refinery(BuildingId),
}

/// The next purchase toward an investment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Placing a building.
    Build(BuildingKind),
    /// Upgrading a building.
    Upgrade(BuildingId),
}

/// An investment the seat wants now, with how much.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) investment: Investment,
    pub(crate) score: u32,
}

/// What the scores read besides the observation.
pub(crate) struct Situation<'a> {
    pub(crate) observation: &'a ObservationData,
    pub(crate) traits: PersonalityTraits,
    /// Harvesters as a per-mille share of those wanted.
    pub(crate) saturation: u32,
    /// Estimated scrap per minute.
    pub(crate) income: u32,
    /// Per-mille share of the scrap around the start already mined.
    pub(crate) depletion: u32,
}

/// Every investment the seat wants at all, most wanted first.
pub(crate) fn candidates(situation: &Situation<'_>) -> Vec<Candidate> {
    let observation = situation.observation;
    let traits = situation.traits;
    let tick = u32::try_from(observation.tick).unwrap_or(u32::MAX);
    let saturated = situation.saturation >= 750;
    let owned = |kind| owned(observation, kind);
    let mut list = Vec::new();
    if owned(BuildingKind::Fabricator) == 0 {
        let score = 300 * u32::from(saturated) + (tick / 6).min(600);
        list.push((Investment::Tech(BuildingKind::Fabricator), score));
    }
    if owned(BuildingKind::Airworks) == 0 {
        let score = u32::from(saturated) * (150 + 4 * u32::from(traits.air)) + (tick / 24).min(300);
        list.push((Investment::Tech(BuildingKind::Airworks), score));
    }
    if owned(BuildingKind::Crucible) == 0 {
        let full = 150 + 3 * u32::from(traits.greed.max(traits.siege));
        let score = full * situation.income.min(360) / 360;
        list.push((Investment::Tech(BuildingKind::Crucible), score));
    }
    if tick >= 2_400 {
        let base = 150 + 3 * u32::from(traits.greed) + situation.depletion * 400 / 1_000;
        let reclaimers = u32::try_from(owned(BuildingKind::Reclaimer)).unwrap_or(u32::MAX);
        list.push((
            Investment::Reclaimer,
            base * 1_000 / (1_000 + 400 * reclaimers),
        ));
        let unupgraded = observation.my_buildings.iter().find(|building| {
            building.kind == BuildingKind::Reclaimer && building.built && building.tier == 0
        });
        if let Some(reclaimer) = unupgraded {
            list.push((Investment::Refinery(reclaimer.id), base * 7 / 5));
        }
    }
    let mut list: Vec<Candidate> = list
        .into_iter()
        .filter(|(_, score)| *score > 0)
        .map(|(investment, score)| Candidate { investment, score })
        .collect();
    list.sort_by_key(|candidate| Reverse(candidate.score));
    list
}

/// The next purchase toward `investment` and its price, or `None` while a
/// prerequisite is still being built or the investment is gone.
pub(crate) fn step(observation: &ObservationData, investment: Investment) -> Option<(Step, u32)> {
    match investment {
        Investment::Tech(kind) => build_step(observation, kind, 3),
        Investment::Reclaimer => build_step(observation, BuildingKind::Reclaimer, 3),
        Investment::Refinery(id) => {
            let reclaimer = observation.my_buildings.iter().find(|building| {
                building.id == id && building.kind == BuildingKind::Reclaimer && building.built
            })?;
            let upgrade = reclaimer.kind.upgrade_from(reclaimer.tier)?;
            match upgrade
                .requires
                .iter()
                .find(|required| !built(observation, **required))
            {
                None => Some((Step::Upgrade(id), upgrade.cost)),
                Some(missing) => requirement_step(observation, *missing, 2),
            }
        }
    }
}

/// Whether `step` finishes `investment` rather than a prerequisite of it.
pub(crate) fn completes(investment: Investment, step: Step) -> bool {
    match (investment, step) {
        (Investment::Tech(kind), Step::Build(built)) => kind == built,
        (Investment::Reclaimer, Step::Build(built)) => built == BuildingKind::Reclaimer,
        (Investment::Refinery(id), Step::Upgrade(upgraded)) => id == upgraded,
        _ => false,
    }
}

fn build_step(observation: &ObservationData, kind: BuildingKind, depth: u8) -> Option<(Step, u32)> {
    let construction = kind.base_stats().construction.as_ref()?;
    match construction
        .requires
        .iter()
        .find(|required| !built(observation, **required))
    {
        None => Some((Step::Build(kind), construction.cost)),
        Some(missing) => requirement_step(observation, *missing, depth),
    }
}

/// A missing requirement already under construction is waited for.
fn requirement_step(
    observation: &ObservationData,
    missing: BuildingKind,
    depth: u8,
) -> Option<(Step, u32)> {
    if owned(observation, missing) > 0 || depth <= 1 {
        return None;
    }
    build_step(observation, missing, depth - 1)
}

/// Own buildings of `kind`, built or not.
fn owned(observation: &ObservationData, kind: BuildingKind) -> usize {
    observation
        .my_buildings
        .iter()
        .filter(|building| building.kind == kind)
        .count()
}

fn built(observation: &ObservationData, kind: BuildingKind) -> bool {
    observation
        .my_buildings
        .iter()
        .any(|building| building.kind == kind && building.built)
}
