//! What the seat could invest in, how much it wants each, and the next
//! purchase toward it. The list is recomputed every decision.

use crate::expansion;
use crate::frame::{HomeFrame, footprint_centre};
use crate::map::MapModel;
use crate::memory::Memory;
use crate::profile::PersonalityTraits;
use chassis::grid::TilePos;
use oxide_sim::observation::ObservationData;
use oxide_sim::{BuildingId, BuildingKind, PlayerId};
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
    /// Another producer of a kind whose producers are all busy.
    Capacity(BuildingKind),
    /// Another Reclaimer.
    Reclaimer,
    /// Upgrading this Reclaimer to a Refinery.
    Refinery(BuildingId),
    /// A Foundry at this expansion site of the map model.
    Expansion(u16),
    /// An Extractor on this frame.
    Extractor(TilePos),
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
    pub(crate) map: &'a MapModel,
    pub(crate) memory: &'a Memory,
    pub(crate) traits: PersonalityTraits,
    /// Harvesters as a per-mille share of those wanted.
    pub(crate) saturation: u32,
    /// Estimated scrap per minute.
    pub(crate) income: u32,
    /// Per-mille share of the scrap around the start already mined.
    pub(crate) depletion: u32,
    /// What unmet army needs add to buildings the seat lacks.
    pub(crate) pull: Vec<(BuildingKind, u32)>,
}

/// Every investment the seat wants at all, most wanted first.
pub(crate) fn candidates(situation: &Situation<'_>) -> Vec<Candidate> {
    let observation = situation.observation;
    let traits = situation.traits;
    let tick = u32::try_from(observation.tick).unwrap_or(u32::MAX);
    let saturated = situation.saturation >= 750;
    let owned = |kind| owned(observation, kind);
    let pull = |kind: BuildingKind| {
        situation
            .pull
            .iter()
            .filter(|(pulled, _)| *pulled == kind)
            .map(|(_, amount)| *amount)
            .sum::<u32>()
    };
    let mut list = Vec::new();
    if owned(BuildingKind::Fabricator) == 0 {
        let score = 300 * u32::from(saturated) + (tick / 6).min(600);
        let score = score + pull(BuildingKind::Fabricator);
        list.push((Investment::Tech(BuildingKind::Fabricator), score));
    }
    if owned(BuildingKind::Airworks) == 0 {
        let score = u32::from(saturated) * (150 + 4 * u32::from(traits.air)) + (tick / 24).min(300);
        let score = score + pull(BuildingKind::Airworks);
        list.push((Investment::Tech(BuildingKind::Airworks), score));
    }
    if owned(BuildingKind::Crucible) == 0 {
        let full = 150 + 3 * u32::from(traits.greed.max(traits.siege));
        let score = full * situation.income.min(360) / 360 + pull(BuildingKind::Crucible);
        list.push((Investment::Tech(BuildingKind::Crucible), score));
    }
    for kind in [BuildingKind::Fabricator, BuildingKind::Airworks] {
        if busy(situation, kind) {
            list.push((
                Investment::Capacity(kind),
                400 + 2 * u32::from(traits.greed),
            ));
        }
    }
    let growth = expansion::candidates(
        observation,
        situation.map,
        situation.memory,
        traits.greed,
        situation.depletion,
    );
    let stranded = !growth
        .iter()
        .any(|(investment, _)| matches!(investment, Investment::Expansion(_)));
    list.extend(growth);
    if tick >= 2_400 {
        let base = 150
            + 3 * u32::from(traits.greed)
            + situation.depletion * 400 / 1_000
            + 300 * u32::from(stranded);
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
    // Equal scores go to the investment nearest home in the seat's frame, so
    // mirrored seats pick mirrored sites and frames.
    let frame = HomeFrame::of(observation, situation.map);
    list.sort_by_key(|candidate| {
        let rank = frame.zip(location(
            situation.map,
            observation.me,
            candidate.investment,
        ));
        (
            Reverse(candidate.score),
            rank.map(|(frame, centre)| frame.rank(frame.home, centre)),
        )
    });
    list
}

/// Where a located investment would stand, as a footprint centre: an
/// expansion at the first anchor `me` could build it on.
fn location(map: &MapModel, me: PlayerId, investment: Investment) -> Option<(i64, i64)> {
    match investment {
        Investment::Extractor(frame) => Some(footprint_centre(BuildingKind::Extractor, frame)),
        Investment::Expansion(site) => map
            .sites()
            .get(usize::from(site))
            .and_then(|site| expansion::anchors(map, me, site).first().copied())
            .map(|anchor| footprint_centre(BuildingKind::Foundry, anchor)),
        Investment::Tech(_)
        | Investment::Capacity(_)
        | Investment::Reclaimer
        | Investment::Refinery(_) => None,
    }
}

/// The next purchase toward `investment` and its price, or `None` while a
/// prerequisite is still being built or the investment is gone.
pub(crate) fn step(observation: &ObservationData, investment: Investment) -> Option<(Step, u32)> {
    match investment {
        Investment::Tech(kind) | Investment::Capacity(kind) => build_step(observation, kind, 3),
        Investment::Reclaimer => build_step(observation, BuildingKind::Reclaimer, 3),
        Investment::Expansion(_) => build_step(observation, BuildingKind::Foundry, 3),
        Investment::Extractor(_) => build_step(observation, BuildingKind::Extractor, 3),
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
        (Investment::Tech(kind) | Investment::Capacity(kind), Step::Build(built)) => kind == built,
        (Investment::Reclaimer, Step::Build(built)) => built == BuildingKind::Reclaimer,
        (Investment::Refinery(id), Step::Upgrade(upgraded)) => id == upgraded,
        (Investment::Expansion(_), Step::Build(built)) => built == BuildingKind::Foundry,
        (Investment::Extractor(_), Step::Build(built)) => built == BuildingKind::Extractor,
        _ => false,
    }
}

/// Where a building step toward `investment` may go: the expansion site's
/// anchors on the seat's home ground for its Foundry, the frame for an
/// Extractor, and otherwise the seat's home spots.
pub(crate) fn anchors(
    map: &MapModel,
    observation: &ObservationData,
    investment: Investment,
    kind: BuildingKind,
) -> Vec<TilePos> {
    match (investment, kind) {
        (Investment::Expansion(site), BuildingKind::Foundry) => map
            .sites()
            .get(usize::from(site))
            .map_or_else(Vec::new, |site| {
                expansion::anchors(map, observation.me, site)
            }),
        (Investment::Extractor(frame), BuildingKind::Extractor) => vec![frame],
        _ => map.spots(observation.me).to_vec(),
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

/// Whether every built producer of `kind` was busy when the decision began
/// and income could keep one more working at 300 a minute each. A producer
/// already being built answers the need.
fn busy(situation: &Situation<'_>, kind: BuildingKind) -> bool {
    let observation = situation.observation;
    let producers: Vec<bool> = observation
        .my_buildings
        .iter()
        .zip(&observation.my_queues)
        .filter(|(building, _)| building.kind == kind)
        .map(|(building, queue)| building.built && !queue.is_empty())
        .collect();
    !producers.is_empty()
        && producers.iter().all(|busy| *busy)
        && situation.income >= 300 * (producers.len() as u32 + 1)
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
