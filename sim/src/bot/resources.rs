//! Fog-honest resources and exact, deterministic planning commitments.
//!
//! [`ResourceSnapshot`] is immutable evidence from one [`Observation`].
//! [`CommitmentLedger`] owns only same-think planning claims; it neither mutates
//! the simulation nor persists across observations.

use super::observation::{BuildingObs, Observation, UnitObs};
use crate::ids::{BuildingId, UnitId};
use crate::stats::{BuildingKind, QUEUE_CAP, UnitKind};
use chassis::Tick;
use chassis::grid::TilePos;

mod ledger;

pub(crate) use ledger::*;

/// Scrap present in the player's bank at the observation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CurrentScrap(u32);

impl CurrentScrap {
    /// The observed bank balance.
    pub(crate) const fn amount(self) -> u32 {
        self.0
    }
}

/// Scrap expected from completed recurring-income sources by a deadline.
///
/// This is deliberately a different type from [`CurrentScrap`], and the
/// commitment ledger has no API that can add it to the spendable bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ForecastScrap(u32);

impl ForecastScrap {
    /// The forecast amount, for proposal comparisons only.
    pub(crate) const fn amount(self) -> u32 {
        self.0
    }
}

/// The completed source responsible for one recurring-income stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RecurringIncomeKind {
    /// A base-tier Reclaimer.
    Reclaimer,
    /// An upgraded Reclaimer.
    Refinery,
    /// A completed Foundry after its warm-up.
    Foundry,
    /// A completed Extractor without Foundry support.
    RemoteExtractor,
    /// A completed Extractor supported by a nearby completed Foundry.
    SupportedExtractor,
}

/// One deterministic recurring-income cadence known at the observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecurringIncomeStream {
    /// Exact completed building producing the income.
    pub(crate) source: BuildingId,
    /// Why this source pays at this cadence.
    pub(crate) kind: RecurringIncomeKind,
    /// Scrap credited at each payment.
    pub(crate) amount: u32,
    /// Ticks between payments.
    pub(crate) period: Tick,
    /// First authoritative tick at or after the observation that pays.
    pub(crate) first_payment_tick: Tick,
}

impl RecurringIncomeStream {
    fn income_through(self, deadline: Tick) -> u32 {
        if deadline < self.first_payment_tick {
            return 0;
        }
        let payments = deadline
            .saturating_sub(self.first_payment_tick)
            .checked_div(self.period)
            .unwrap_or(0)
            .saturating_add(1);
        u32::try_from(payments)
            .unwrap_or(u32::MAX)
            .saturating_mul(self.amount)
    }
}

/// Conservative future income from sources that are completed now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceForecast {
    observed_at: Tick,
    income: Vec<RecurringIncomeStream>,
}

impl ResourceForecast {
    /// The observation tick from which this forecast starts.
    #[cfg(test)]
    pub(crate) const fn observed_at(&self) -> Tick {
        self.observed_at
    }

    /// Completed recurring-income streams in building-id order.
    #[cfg(test)]
    pub(crate) fn income_streams(&self) -> &[RecurringIncomeStream] {
        &self.income
    }

    /// Income expected through `deadline`, excluding the current bank.
    pub(crate) fn income_through(&self, deadline: Tick) -> ForecastScrap {
        if deadline < self.observed_at {
            return ForecastScrap(0);
        }
        ForecastScrap(
            self.income
                .iter()
                .copied()
                .map(|stream| stream.income_through(deadline))
                .fold(0, u32::saturating_add),
        )
    }
}

/// One own unit that may be claimed by an admitted plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnitResource {
    /// Exact unit id.
    pub(crate) id: UnitId,
    /// Current unit kind.
    pub(crate) kind: UnitKind,
}

/// Existing non-preemptible work attached to a construction-capable unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuilderObligation {
    /// Raising an already-paid site.
    Build(BuildingId),
    /// Walking out an unpaid deferred foundation.
    Found {
        /// Kind promised by the deferred order.
        kind: BuildingKind,
        /// Exact promised footprint anchor.
        anchor: TilePos,
    },
    /// Dismantling a completed building.
    Salvage(BuildingId),
    /// Performing voluntary paid repair.
    Repair,
    /// Carrying a queued continuation or looping program whose contents stay
    /// outside the policy observation surface.
    Queued,
}

/// One construction-capable own unit and any work that already owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BuilderResource {
    /// Exact unit id.
    pub(crate) id: UnitId,
    /// Current unit kind.
    pub(crate) kind: UnitKind,
    /// Existing non-preemptible work, if any.
    pub(crate) obligation: Option<BuilderObligation>,
}

/// Whether a construction-capable unit is free of every observed program that
/// a newly admitted exact plan must not preempt.
pub(crate) fn builder_is_free(obs: &Observation, unit: &UnitObs) -> bool {
    unit.kind.stats().harvest.is_some() && builder_obligation(obs, unit).is_none()
}

fn builder_obligation(obs: &Observation, unit: &UnitObs) -> Option<BuilderObligation> {
    unit.site
        .map(BuilderObligation::Build)
        .or_else(|| {
            unit.founding
                .map(|(kind, anchor)| BuilderObligation::Found { kind, anchor })
        })
        .or_else(|| unit.salvaging.map(BuilderObligation::Salvage))
        .or(unit.repairing.then_some(BuilderObligation::Repair))
        .or(obs
            .has_queued_program(unit.id)
            .then_some(BuilderObligation::Queued))
}

/// Exact construction work protected across planning and intent lowering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BuilderLease {
    builder: UnitId,
    kind: BuildingKind,
    anchor: TilePos,
}

impl BuilderLease {
    /// Creates an exact lease for one builder and one construction command.
    pub(crate) const fn new(builder: UnitId, kind: BuildingKind, anchor: TilePos) -> Self {
        Self {
            builder,
            kind,
            anchor,
        }
    }

    /// The exact leased builder.
    pub(crate) const fn builder(self) -> UnitId {
        self.builder
    }

    /// Exact building kind the lease permits.
    pub(crate) const fn kind(self) -> BuildingKind {
        self.kind
    }

    /// Exact footprint anchor the lease permits.
    pub(crate) const fn anchor(self) -> TilePos {
        self.anchor
    }

    /// Whether this is the one construction command permitted to consume it.
    pub(crate) fn permits(self, builder: UnitId, kind: BuildingKind, anchor: TilePos) -> bool {
        self.builder == builder && self.kind == kind && self.anchor == anchor
    }
}

/// One exact queue position currently open on a completed producer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProducerSlot {
    /// Exact producer.
    pub(crate) producer: BuildingId,
    /// Zero-based position in its authoritative queue.
    pub(crate) queue_index: u8,
}

/// What the current observation proves about a producer's spawn egress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProducerEgress {
    /// This kind spawns without using the ground ring.
    NotRequired,
    /// At least one currently known ring tile can accept the unit.
    Open,
    /// Every ring tile is currently known blocked.
    Blocked,
    /// No current open tile is known, but unseen non-static ground prevents
    /// certainty.
    Unknown,
}

/// Honest timing evidence for one append-only production claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProductionTiming {
    /// Earliest tick the unit can be ready, assuming prior current work is as
    /// advanced as the observation permits.
    pub(crate) earliest_ready_tick: Tick,
    /// Latest ready tick if the hidden front-item progress is zero and every
    /// ground spawn has an open doorstep when needed. This is a conditional
    /// no-block bound, not a promise that egress will remain available.
    pub(crate) no_block_latest_ready_tick: Tick,
    /// Current egress evidence. This is not a promise that a ground unit will
    /// spawn by any finite deadline.
    pub(crate) current_egress: ProducerEgress,
}

/// Completed production capacity and honest queue-timing bounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProducerLane {
    /// Exact completed producer.
    pub(crate) producer: BuildingId,
    /// Producer kind at the observation.
    pub(crate) kind: BuildingKind,
    /// Units already paid and queued, in queue order.
    queued: Vec<UnitKind>,
    /// Unit kinds legal from this producer with the currently completed tech.
    trainable: Vec<UnitKind>,
    /// Observation tick whose command and production phases come next.
    observed_at: Tick,
    /// Fewest production-phase executions consumed before an appended unit,
    /// accounting for an observed front item that may already be complete.
    earliest_preceding_ticks: Option<Tick>,
    /// Most production-phase executions consumed by current queue work if the
    /// hidden front-item progress is zero and every spawn can leave promptly.
    no_block_latest_preceding_ticks: Option<Tick>,
    /// Current evidence for the ground spawn ring.
    ground_egress: ProducerEgress,
}

impl ProducerLane {
    /// Existing queue in authoritative order.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> &[UnitKind] {
        &self.queued
    }

    /// Unit kinds currently legal from this producer, in declaration order.
    #[cfg(test)]
    pub(crate) fn trainable(&self) -> &[UnitKind] {
        &self.trainable
    }

    /// Open queue positions in increasing queue-index order.
    pub(crate) fn open_slots(&self) -> impl Iterator<Item = ProducerSlot> + '_ {
        (self.queued.len()..QUEUE_CAP).map(|queue_index| ProducerSlot {
            producer: self.producer,
            queue_index: u8::try_from(queue_index).expect("queue capacity fits in u8"),
        })
    }

    /// Earliest readiness and current egress evidence for a proposed sequence.
    pub(crate) fn production_timing(&self, planned: &[UnitKind]) -> Option<ProductionTiming> {
        if self.queued.len().saturating_add(planned.len()) > QUEUE_CAP
            || planned.iter().any(|kind| !self.trainable.contains(kind))
        {
            return None;
        }
        if planned.is_empty() {
            return None;
        }
        let planned_ticks = planned.iter().try_fold(0_u64, |ticks, kind| {
            ticks.checked_add(Tick::from(kind.stats().train_ticks))
        })?;
        let earliest_ready_tick = self.observed_at.checked_add(
            self.earliest_preceding_ticks?
                .checked_add(planned_ticks)?
                .checked_sub(1)?,
        )?;
        let no_block_latest_ready_tick = self.observed_at.checked_add(
            self.no_block_latest_preceding_ticks?
                .checked_add(planned_ticks)?
                .checked_sub(1)?,
        )?;
        let current_egress = planned.last().map_or(ProducerEgress::NotRequired, |kind| {
            if self.kind == BuildingKind::Airworks
                && kind.stats().domain == crate::stats::Domain::Air
            {
                ProducerEgress::NotRequired
            } else {
                self.ground_egress
            }
        });
        Some(ProductionTiming {
            earliest_ready_tick,
            no_block_latest_ready_tick,
            current_egress,
        })
    }
}

/// Current, fog-honest resources visible to one planning pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceSnapshot {
    current_scrap: CurrentScrap,
    forecast: ResourceForecast,
    units: Vec<UnitResource>,
    builders: Vec<BuilderResource>,
    producers: Vec<ProducerLane>,
    producer_slots: Vec<ProducerSlot>,
}

impl ResourceSnapshot {
    /// Builds a snapshot exclusively from the policy's observation surface.
    pub(crate) fn from_observation(obs: &Observation) -> Self {
        let mut units: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.player == obs.me)
            .map(|unit| UnitResource {
                id: unit.id,
                kind: unit.kind,
            })
            .collect();
        units.sort_unstable_by_key(|unit| unit.id);
        units.dedup_by_key(|unit| unit.id);

        let mut builders: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.player == obs.me && unit.kind.stats().harvest.is_some())
            .map(|unit| BuilderResource {
                id: unit.id,
                kind: unit.kind,
                obligation: builder_obligation(obs, unit),
            })
            .collect();
        builders.sort_unstable_by_key(|builder| builder.id);
        builders.dedup_by_key(|builder| builder.id);

        let completed_kinds: Vec<_> = obs
            .my_buildings
            .iter()
            .filter(|building| building.player == obs.me && building.built && building.hp > 0)
            .map(|building| building.kind)
            .collect();
        let mut producers: Vec<_> = obs
            .my_buildings
            .iter()
            .zip(&obs.my_queues)
            .filter(|(building, _)| {
                building.player == obs.me
                    && building.built
                    && building.hp > 0
                    && !building.kind.tier_stats(building.tier).produces.is_empty()
            })
            .map(|(building, queue)| {
                let stats = building.kind.tier_stats(building.tier);
                let trainable = UnitKind::ALL
                    .into_iter()
                    .filter(|kind| stats.produces.contains(kind))
                    .filter(|kind| kind.faction().is_none_or(|faction| faction == obs.faction))
                    .filter(|kind| {
                        kind.stats()
                            .requires
                            .iter()
                            .all(|required| completed_kinds.contains(required))
                    })
                    .collect();
                let queued = queue.clone();
                let (earliest_preceding_ticks, no_block_latest_preceding_ticks) =
                    producer_preceding_ticks(&queued);
                ProducerLane {
                    producer: building.id,
                    kind: building.kind,
                    queued,
                    trainable,
                    observed_at: obs.tick,
                    earliest_preceding_ticks,
                    no_block_latest_preceding_ticks,
                    ground_egress: ground_producer_egress(obs, building),
                }
            })
            .collect();
        producers.sort_unstable_by_key(|lane| lane.producer);
        producers.dedup_by_key(|lane| lane.producer);
        let producer_slots = producers
            .iter()
            .flat_map(ProducerLane::open_slots)
            .collect();

        Self {
            current_scrap: CurrentScrap(obs.scrap),
            forecast: forecast_from_observation(obs),
            units,
            builders,
            producers,
            producer_slots,
        }
    }

    /// Current spendable bank, distinct from all forecast income.
    pub(crate) const fn current_scrap(&self) -> CurrentScrap {
        self.current_scrap
    }

    /// Conservative forecast from completed income sources.
    pub(crate) const fn forecast(&self) -> &ResourceForecast {
        &self.forecast
    }

    /// Own units in id order.
    #[cfg(test)]
    pub(crate) fn units(&self) -> &[UnitResource] {
        &self.units
    }

    /// Construction-capable own units in id order.
    pub(crate) fn builders(&self) -> &[BuilderResource] {
        &self.builders
    }

    /// Completed production lanes in building-id order.
    pub(crate) fn producers(&self) -> &[ProducerLane] {
        &self.producers
    }

    /// Every currently open queue position, ordered by producer then index.
    pub(crate) fn producer_slots(&self) -> &[ProducerSlot] {
        &self.producer_slots
    }

    /// Open queue positions on producers that can currently train `kind`.
    #[cfg(test)]
    pub(crate) fn producer_slots_for(&self, kind: UnitKind) -> Vec<ProducerSlot> {
        self.producers
            .iter()
            .filter(|lane| lane.trainable.contains(&kind))
            .flat_map(ProducerLane::open_slots)
            .collect()
    }
}

fn producer_preceding_ticks(queued: &[UnitKind]) -> (Option<Tick>, Option<Tick>) {
    if queued.is_empty() {
        return (Some(0), Some(0));
    }
    let earliest = queued.iter().skip(1).try_fold(1_u64, |ticks, kind| {
        ticks.checked_add(Tick::from(kind.stats().train_ticks))
    });
    let no_block_latest = queued.iter().try_fold(0_u64, |ticks, kind| {
        ticks.checked_add(Tick::from(kind.stats().train_ticks))
    });
    (earliest, no_block_latest)
}

fn ground_producer_egress(obs: &Observation, producer: &BuildingObs) -> ProducerEgress {
    let building_covers = |tile: TilePos| {
        obs.my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .chain(obs.enemy_buildings.iter())
            .filter(|building| !building.kind.is_stealthy())
            .any(|building| {
                let size = building.kind.tier_stats(building.tier).size;
                tile.x >= building.anchor.x
                    && tile.x < building.anchor.x.saturating_add(size.0)
                    && tile.y >= building.anchor.y
                    && tile.y < building.anchor.y.saturating_add(size.1)
            })
    };
    let mut unknown = false;
    for tile in crate::tick::rect_adjacent_tiles(
        producer.anchor,
        producer.kind.tier_stats(producer.tier).size,
    ) {
        if tile.x < 0 || tile.y < 0 || tile.x >= obs.map_width || tile.y >= obs.map_height {
            continue;
        }
        if obs.known_rock_at(tile) {
            continue;
        }
        if !obs.visible(tile) {
            unknown = true;
            continue;
        }
        if !obs.known_scrap_at(tile) && !building_covers(tile) {
            return ProducerEgress::Open;
        }
    }
    if unknown {
        ProducerEgress::Unknown
    } else {
        ProducerEgress::Blocked
    }
}

fn forecast_from_observation(obs: &Observation) -> ResourceForecast {
    let built_foundries: Vec<_> = obs
        .my_buildings
        .iter()
        .filter(|building| {
            building.player == obs.me
                && building.hp > 0
                && building.built
                && building.kind == BuildingKind::Foundry
        })
        .map(|building| building.anchor)
        .collect();
    let mut income: Vec<_> = obs
        .my_buildings
        .iter()
        .filter(|building| building.player == obs.me && building.hp > 0 && building.built)
        .filter_map(|building| {
            let (kind, amount, period, first_payment_tick) = match building.kind {
                BuildingKind::Reclaimer if building.tier == 0 => (
                    RecurringIncomeKind::Reclaimer,
                    1,
                    crate::stats::RECLAIMER_PERIOD,
                    next_multiple_at_or_after(obs.tick, crate::stats::RECLAIMER_PERIOD)?,
                ),
                BuildingKind::Reclaimer => (
                    RecurringIncomeKind::Refinery,
                    1,
                    crate::stats::REFINERY_PERIOD,
                    next_multiple_at_or_after(obs.tick, crate::stats::REFINERY_PERIOD)?,
                ),
                BuildingKind::Foundry => {
                    let completed_tick = next_multiple_at_or_after(
                        obs.tick
                            .checked_add(1)?
                            .max(crate::stats::FOUNDRY_DRIP_START_TICK),
                        crate::stats::FOUNDRY_DRIP_PERIOD,
                    )?;
                    (
                        RecurringIncomeKind::Foundry,
                        1,
                        crate::stats::FOUNDRY_DRIP_PERIOD,
                        completed_tick.checked_sub(1)?,
                    )
                }
                BuildingKind::Extractor => {
                    let supported = built_foundries
                        .iter()
                        .copied()
                        .any(|foundry| foundry_supports_extractor(foundry, building.anchor));
                    let (kind, (amount, period)) = if supported {
                        (
                            RecurringIncomeKind::SupportedExtractor,
                            crate::stats::EXTRACTOR_SUPPORTED_YIELD,
                        )
                    } else {
                        (
                            RecurringIncomeKind::RemoteExtractor,
                            crate::stats::EXTRACTOR_REMOTE_YIELD,
                        )
                    };
                    let completed_tick =
                        next_multiple_at_or_after(obs.tick.checked_add(1)?, period)?;
                    (kind, amount, period, completed_tick.checked_sub(1)?)
                }
                _ => return None,
            };
            Some(RecurringIncomeStream {
                source: building.id,
                kind,
                amount,
                period,
                first_payment_tick,
            })
        })
        .collect();
    income.sort_unstable_by_key(|stream| stream.source);
    income.dedup_by_key(|stream| stream.source);
    ResourceForecast {
        observed_at: obs.tick,
        income,
    }
}

fn next_multiple_at_or_after(tick: Tick, period: Tick) -> Option<Tick> {
    if period == 0 {
        return None;
    }
    let remainder = tick % period;
    if remainder == 0 {
        Some(tick)
    } else {
        tick.checked_add(period - remainder)
    }
}

fn foundry_supports_extractor(foundry: TilePos, extractor: TilePos) -> bool {
    fn axis_distance(a: i32, a_len: i32, b: i32, b_len: i32) -> i32 {
        let a_far = a.saturating_add(a_len).saturating_sub(1);
        let b_far = b.saturating_add(b_len).saturating_sub(1);
        a.saturating_sub(b_far).max(b.saturating_sub(a_far)).max(0)
    }

    let foundry_size = BuildingKind::Foundry.base_stats().size;
    let extractor_size = BuildingKind::Extractor.base_stats().size;
    axis_distance(foundry.x, foundry_size.0, extractor.x, extractor_size.0).max(axis_distance(
        foundry.y,
        foundry_size.1,
        extractor.y,
        extractor_size.1,
    )) <= crate::stats::EXTRACTOR_SUPPORT_RADIUS
}
