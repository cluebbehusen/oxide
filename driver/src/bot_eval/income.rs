//! Actual income against a saturated-economy estimate.
//!
//! Both figures are omniscient QA diagnostics computed by the evaluation loop.
//! They never reach a controller.

use chassis::fx::Fx;
use chassis::grid::TilePos;
use oxide_kit::stats::LiveMatchStats;
use oxide_sim::stats::{
    EXTRACTOR_REMOTE_INCOME_PER_MINUTE, EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE, FOUNDRY_DRIP_PERIOD,
    FOUNDRY_DRIP_START_TICK, HARVEST_ZONE_RADIUS, RECLAIMER_PERIOD, REFINERY_PERIOD,
};
use oxide_sim::{Building, BuildingKind, Event, ExtractorIncome, PlayerId, State, UnitKind};
use serde::{Deserialize, Serialize};

const TICKS_PER_MINUTE: u32 = oxide_sim::TICKS_PER_SECOND * 60;

/// Ticks at which each seat's income is compared with its saturation estimate.
pub const INCOME_CHECKPOINTS: [u64; 3] = [6_000, 12_000, 24_000];

/// Ticks of actual income measured before each checkpoint: one minute.
pub const INCOME_WINDOW_TICKS: u64 = TICKS_PER_MINUTE as u64;

/// Harvesters the estimate assigns to each scrap node near a Foundry.
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
            if !self.watched[seat] {
                continue;
            }
            let player = PlayerId(seat as u8);
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
                    actual_per_minute: (earned * u64::from(TICKS_PER_MINUTE) / INCOME_WINDOW_TICKS)
                        as u32,
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
/// [`HARVESTERS_PER_NODE`] Harvesters on every scrap node within the work zone
/// of a completed Foundry, each cycling between the node and the nearest such
/// Foundry in a straight line, plus passive credits.
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
                && building.built
                && building.hp > 0
        })
        .collect();
    let harvester = UnitKind::Harvester.stats();
    let harvest = harvester.harvest.expect("Harvesters harvest");
    let mut nodes: Vec<TilePos> = Vec::new();
    for foundry in &foundries {
        let (width, height) = foundry.stats().size;
        for y in
            foundry.anchor.y - HARVEST_ZONE_RADIUS..foundry.anchor.y + height + HARVEST_ZONE_RADIUS
        {
            for x in foundry.anchor.x - HARVEST_ZONE_RADIUS
                ..foundry.anchor.x + width + HARVEST_ZONE_RADIUS
            {
                let tile = TilePos::new(x, y);
                if state.map().scrap_at(tile) > 0 {
                    nodes.push(tile);
                }
            }
        }
    }
    nodes.sort_unstable_by_key(|tile| (tile.y, tile.x));
    nodes.dedup();
    let extraction = Fx::from_num(harvest.capacity * harvest.ticks_per_scrap);
    let delivered = Fx::from_num(HARVESTERS_PER_NODE * harvest.capacity * TICKS_PER_MINUTE);
    let mut total = Fx::ZERO;
    for node in nodes {
        let center = node.center();
        let Some(distance) = foundries
            .iter()
            .map(|foundry| center.dist(foundry.closest_point_to(center)))
            .min()
        else {
            continue;
        };
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
        .filter(|building| building.player == player && building.built && building.hp > 0)
        .map(|building| match building.kind {
            BuildingKind::Reclaimer if building.tier == 0 => {
                TICKS_PER_MINUTE / RECLAIMER_PERIOD as u32
            }
            BuildingKind::Reclaimer => TICKS_PER_MINUTE / REFINERY_PERIOD as u32,
            BuildingKind::Extractor if !seat.resigned => {
                match state.extractor_income(building.id) {
                    Some(ExtractorIncome::Supported) => EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE,
                    Some(ExtractorIncome::Remote) => EXTRACTOR_REMOTE_INCOME_PER_MINUTE,
                    None => 0,
                }
            }
            BuildingKind::Foundry if drip => TICKS_PER_MINUTE / FOUNDRY_DRIP_PERIOD as u32,
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::Scenario;
    use oxide_sim::scenario::{BuildingSpec, PlayerSpec};
    use oxide_sim::{Faction, stats::HarvestStats};

    fn scenario(map: Vec<String>, buildings: Vec<BuildingSpec>) -> Scenario {
        Scenario {
            mode: Default::default(),
            name: "income".into(),
            seed: 5,
            map,
            players: [Faction::Ferrous, Faction::Cupric]
                .into_iter()
                .map(|faction| PlayerSpec {
                    name: format!("{faction:?}"),
                    faction,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                })
                .collect(),
            units: Vec::new(),
            buildings,
            meta: None,
        }
    }

    fn row(width: usize, marks: &[(usize, char)]) -> String {
        let mut row: Vec<char> = ".".repeat(width).chars().collect();
        for (x, mark) in marks {
            row[*x] = *mark;
        }
        row.into_iter().collect()
    }

    fn node_rate(distance: Fx) -> Fx {
        let HarvestStats {
            capacity,
            ticks_per_scrap,
        } = UnitKind::Harvester.stats().harvest.unwrap();
        Fx::from_num(HARVESTERS_PER_NODE * capacity * TICKS_PER_MINUTE)
            / (Fx::from_num(capacity * ticks_per_scrap)
                + distance * 2 / UnitKind::Harvester.stats().speed)
    }

    #[test]
    fn saturation_counts_zone_nodes_once_at_their_round_trip() {
        // Foundry 1 at (2,2) covers tiles 0..=10 by Chebyshev distance. The
        // node at (5,3) sits 1.5 tiles beyond its east face; the node at
        // (10,2) is 6.5 tiles out; the node at (11,2) lies outside the zone.
        let mut map = vec![".".repeat(30); 14];
        map[2] = row(30, &[(2, '1'), (10, 's'), (11, 's'), (26, '2')]);
        map[3] = row(30, &[(5, 's')]);
        let state = scenario(map, Vec::new()).build().unwrap();
        let expected = (node_rate(Fx::lit("1.5")) + node_rate(Fx::lit("6.5"))).to_num::<u32>();
        assert_eq!(saturation_per_minute(&state, PlayerId(0)), expected);
        assert!(expected > 0);
        assert_eq!(
            passive_per_minute(&state, PlayerId(0)),
            0,
            "the drip has not started at tick zero"
        );
    }

    #[test]
    fn passive_rates_follow_reclaimer_tiers_extractors_and_the_drip() {
        let mut map = vec![".".repeat(30); 14];
        map[2] = row(30, &[(2, '1'), (26, '2')]);
        let buildings = vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 8,
                y: 8,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 12,
                y: 8,
            },
        ];
        let mut value = serde_json::to_value(scenario(map, buildings).build().unwrap()).unwrap();
        let reclaimer = value["buildings"]
            .as_array()
            .unwrap()
            .iter()
            .rposition(|building| building["kind"] == "reclaimer")
            .unwrap();
        value["buildings"][reclaimer]["tier"] = 1.into();
        value["tick"] = FOUNDRY_DRIP_START_TICK.into();
        let state: State = serde_json::from_value(value).unwrap();
        assert_eq!(
            passive_per_minute(&state, PlayerId(0)),
            TICKS_PER_MINUTE / RECLAIMER_PERIOD as u32
                + TICKS_PER_MINUTE / REFINERY_PERIOD as u32
                + TICKS_PER_MINUTE / FOUNDRY_DRIP_PERIOD as u32
        );
        assert_eq!(
            passive_per_minute(&state, PlayerId(1)),
            TICKS_PER_MINUTE / FOUNDRY_DRIP_PERIOD as u32
        );
    }

    #[test]
    fn checkpoints_compare_the_last_minute_with_the_estimate() {
        let mut state = Scenario::skirmish().build().unwrap();
        let mut tracker = IncomeTracker::new(&state, vec![true, false]);
        while state.current_tick() < INCOME_CHECKPOINTS[0] {
            let report = state.tick(&[]);
            tracker.observe(&state, &report.events, 12);
        }
        let samples = tracker.finish();
        assert!(samples[1].is_empty(), "seat one is unwatched");
        let [sample] = samples[0].as_slice() else {
            panic!("one checkpoint was reached: {samples:?}");
        };
        assert_eq!(sample.tick, INCOME_CHECKPOINTS[0]);
        assert_eq!(
            sample.saturation_per_minute,
            saturation_per_minute(&state, PlayerId(0))
        );
        let drip = TICKS_PER_MINUTE / FOUNDRY_DRIP_PERIOD as u32;
        assert!(
            sample.actual_per_minute >= drip,
            "an idle seat still earns its drip: {sample:?}"
        );
    }
}
