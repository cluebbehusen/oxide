//! Actual income against a saturated-economy estimate.
//!
//! Both figures are omniscient QA diagnostics computed by the evaluation loop.
//! They never reach a controller.

use chassis::fx::Fx;
use chassis::grid::TilePos;
use oxide_kit::stats::LiveMatchStats;
use oxide_sim::stats::{
    EXTRACTOR_REMOTE_INCOME_PER_MINUTE, EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE, FOUNDRY_DRIP_PERIOD,
    FOUNDRY_DRIP_START_TICK, RECLAIMER_PERIOD, REFINERY_PERIOD,
};
use oxide_sim::{Building, BuildingKind, Event, ExtractorIncome, PlayerId, State, UnitKind};
use serde::{Deserialize, Serialize};

const TICKS_PER_MINUTE: u32 = oxide_sim::TICKS_PER_SECOND * 60;

/// Ticks at which each seat's income is compared with its saturation estimate.
pub const INCOME_CHECKPOINTS: [u64; 3] = [6_000, 12_000, 24_000];

/// Ticks of actual income measured before each checkpoint: one minute.
pub const INCOME_WINDOW_TICKS: u64 = TICKS_PER_MINUTE as u64;

/// Scrap nodes the estimate assigns to each completed Foundry: its nearest
/// ones that still hold scrap.
pub const NODES_PER_FOUNDRY: usize = 4;

/// Harvesters the estimate assigns to each of those nodes.
pub const HARVESTERS_PER_NODE: u32 = 2;

/// One seat's income at one checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IncomeSample {
    /// The checkpoint tick.
    pub tick: u64,
    /// Scrap earned over the preceding minute: Harvester deliveries plus
    /// Reclaimer, Extractor and Foundry credits.
    pub actual_per_minute: u32,
    /// Estimated scrap per minute of a saturated economy on the seat's
    /// standing works at the checkpoint.
    pub saturation_per_minute: u32,
}

/// Per-leg income memory. Passive credits accumulate as per-minute rates
/// times elapsed ticks, sampled every `period` ticks.
pub(super) struct IncomeTracker {
    stats: LiveMatchStats,
    watched: Vec<bool>,
    passive: Vec<u64>,
    window: Vec<Option<(u32, u64)>>,
    samples: Vec<Vec<IncomeSample>>,
}

impl IncomeTracker {
    pub(super) fn new(state: &State, watched: Vec<bool>) -> Self {
        let seats = watched.len();
        Self {
            stats: LiveMatchStats::new(state),
            watched,
            passive: vec![0; seats],
            window: vec![None; seats],
            samples: vec![Vec::new(); seats],
        }
    }

    /// Consumes one completed tick. `period` must divide every checkpoint and
    /// window start.
    pub(super) fn observe(&mut self, state: &State, events: &[Event], period: u64) {
        self.stats.observe(state, events);
        let now = state.current_tick();
        if !now.is_multiple_of(period) {
            return;
        }
        let window_start = INCOME_CHECKPOINTS.contains(&(now + INCOME_WINDOW_TICKS));
        let checkpoint = INCOME_CHECKPOINTS.contains(&now);
        let collected = (window_start || checkpoint).then(|| self.stats.snapshot(state));
        for seat in 0..self.watched.len() {
            let player = PlayerId::from_index(seat);
            // A seat that is out while its team plays on would add samples
            // of an empty economy.
            if !self.watched[seat] || state.player(player).eliminated_at.is_some() {
                continue;
            }
            self.passive[seat] += u64::from(passive_per_minute(state, player)) * period;
            let Some(collected) = &collected else {
                continue;
            };
            let deposits = collected.players[seat].scrap_collected;
            if checkpoint && let Some((start_deposits, start_passive)) = self.window[seat].take() {
                let passive = (self.passive[seat] - start_passive) / u64::from(TICKS_PER_MINUTE);
                let earned = u64::from(deposits - start_deposits) + passive;
                self.samples[seat].push(IncomeSample {
                    tick: now,
                    actual_per_minute: u32::try_from(
                        earned * u64::from(TICKS_PER_MINUTE) / INCOME_WINDOW_TICKS,
                    )
                    .expect("tick counts fit in u32"),
                    saturation_per_minute: saturation_per_minute(state, player),
                });
            }
            if window_start {
                self.window[seat] = Some((deposits, self.passive[seat]));
            }
        }
    }

    /// Checkpoint samples by seat; unwatched seats report none.
    pub(super) fn finish(self) -> Vec<Vec<IncomeSample>> {
        self.samples
    }
}

/// Scrap per minute a saturated seat would earn from its standing works:
/// [`HARVESTERS_PER_NODE`] Harvesters on each of the [`NODES_PER_FOUNDRY`]
/// nearest scrap nodes of every completed Foundry, each cycling between node
/// and Foundry in a straight line, plus passive credits. Nodes go to the
/// closest Foundry pairs first, so none is counted twice.
pub fn saturation_per_minute(state: &State, player: PlayerId) -> u32 {
    harvest_per_minute(state, player).saturating_add(passive_per_minute(state, player))
}

fn harvest_per_minute(state: &State, player: PlayerId) -> u32 {
    let foundries: Vec<&Building> = state
        .buildings()
        .iter()
        .filter(|building| {
            building.player == player
                && building.kind == BuildingKind::Foundry
                && building.built()
                && building.hp > 0
        })
        .collect();
    if foundries.is_empty() {
        return 0;
    }
    let map = state.map();
    let mut pairs: Vec<(Fx, usize, i32, i32)> = Vec::new();
    for y in 0..map.height() {
        for x in 0..map.width() {
            let node = TilePos::new(x, y);
            if map.scrap_at(node) == 0 {
                continue;
            }
            let center = node.center();
            for (index, foundry) in foundries.iter().enumerate() {
                let distance = center.dist(foundry.closest_point_to(center));
                pairs.push((distance, index, y, x));
            }
        }
    }
    pairs.sort_unstable();
    let harvester = UnitKind::Harvester.stats();
    let harvest = harvester.harvest.expect("Harvesters harvest");
    let extraction = Fx::from_num(harvest.capacity * harvest.ticks_per_scrap);
    let delivered = Fx::from_num(HARVESTERS_PER_NODE * harvest.capacity * TICKS_PER_MINUTE);
    let mut assigned = vec![0_usize; foundries.len()];
    let mut claimed: Vec<(i32, i32)> = Vec::new();
    let mut total = Fx::ZERO;
    for (distance, index, y, x) in pairs {
        if assigned[index] == NODES_PER_FOUNDRY || claimed.contains(&(y, x)) {
            continue;
        }
        assigned[index] += 1;
        claimed.push((y, x));
        total += delivered / (extraction + distance * 2 / harvester.speed);
    }
    total.to_num::<u32>()
}

/// Scrap per minute credited without deliveries: Reclaimers, Extractors and
/// the Foundry drip once it has started.
fn passive_per_minute(state: &State, player: PlayerId) -> u32 {
    let seat = state.player(player);
    let drip = state.current_tick() >= FOUNDRY_DRIP_START_TICK && !seat.resigned;
    state
        .buildings()
        .iter()
        .filter(|building| building.player == player && building.built() && building.hp > 0)
        .map(|building| match building.kind {
            BuildingKind::Reclaimer if building.tier == 0 => {
                TICKS_PER_MINUTE / u32::try_from(RECLAIMER_PERIOD).expect("periods fit in u32")
            }
            BuildingKind::Reclaimer => {
                TICKS_PER_MINUTE / u32::try_from(REFINERY_PERIOD).expect("periods fit in u32")
            }
            BuildingKind::Extractor if !seat.resigned => {
                match state.extractor_income(building.id) {
                    Some(ExtractorIncome::Supported) => EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE,
                    Some(ExtractorIncome::Remote) => EXTRACTOR_REMOTE_INCOME_PER_MINUTE,
                    None => 0,
                }
            }
            BuildingKind::Foundry if drip => {
                TICKS_PER_MINUTE / u32::try_from(FOUNDRY_DRIP_PERIOD).expect("periods fit in u32")
            }
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests;
