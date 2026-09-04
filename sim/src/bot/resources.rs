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
mod planning;
mod production;

pub(crate) use ledger::*;
pub(crate) use planning::*;
pub(crate) use production::*;

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

/// One accepted future append whose exact FIFO position belongs to a planning
/// domain rather than to residual same-think production.
///
/// This is reservation evidence, not observed inventory. Consumers use it to
/// keep the named producer free until the accepted append is due; they must not
/// insert `kind` into an observed queue or count it as an already-paid unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ReservedProducerJob {
    /// Exact producer whose FIFO timing was accepted.
    pub(crate) producer: BuildingId,
    /// Exact unit kind retained from the accepted request.
    pub(crate) kind: UnitKind,
    /// Decision tick on which the append is due.
    pub(crate) enqueued_at: Tick,
    /// First production-phase tick occupied by the job.
    pub(crate) starts_at: Tick,
    /// Tick on which production completes.
    pub(crate) ready_at: Tick,
    /// Fixed strict readiness deadline used during allocation.
    pub(crate) ready_before: Tick,
}

/// Why an accepted producer schedule could not be overlaid on the resource
/// projection from which it was allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProducerLaneReservationError {
    /// The schedule named a producer absent from the projection.
    UnknownProducer {
        /// Exact missing producer.
        producer: BuildingId,
    },
    /// A current-tick append was no longer legal on its accepted producer.
    CurrentAppendUnavailable {
        /// Exact producer.
        producer: BuildingId,
        /// Unit whose append failed.
        kind: UnitKind,
    },
    /// Replaying a current-tick append changed its accepted FIFO timing.
    CurrentTimingMismatch {
        /// Exact producer.
        producer: BuildingId,
        /// Unit whose timing changed.
        kind: UnitKind,
    },
}

/// Accepted future producer work overlaid on an otherwise truthful
/// [`Observation`].
///
/// Residual planners issue only immediate queue appends. On a producer with
/// future work, they may use only the queue prefix that leaves every accepted
/// append's enqueue, start, and ready tick unchanged. Unrelated producers
/// remain available, and observed queues remain the sole source of paid
/// inventory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ProducerLaneReservations {
    baselines: Vec<ProducerPlanningProjection>,
    current_jobs: Vec<ReservedProducerJob>,
    jobs: Vec<ReservedProducerJob>,
}

impl ProducerLaneReservations {
    /// Shared empty overlay for callers outside cross-domain allocation.
    pub(crate) fn empty() -> &'static Self {
        static EMPTY: ProducerLaneReservations = ProducerLaneReservations {
            baselines: Vec::new(),
            current_jobs: Vec::new(),
            jobs: Vec::new(),
        };
        &EMPTY
    }

    /// Retains only jobs whose append is still in the future at this
    /// observation, in canonical producer/timing order.
    pub(crate) fn from_jobs(
        resources: &ResourcePlanningProjection,
        jobs: impl IntoIterator<Item = ReservedProducerJob>,
    ) -> Result<Self, ProducerLaneReservationError> {
        let observed_at = resources.observed_at();
        let mut scheduled: Vec<_> = jobs
            .into_iter()
            .filter(|job| job.enqueued_at >= observed_at)
            .collect();
        scheduled.sort_unstable_by_key(|job| {
            (
                job.producer,
                job.enqueued_at,
                job.starts_at,
                job.ready_at,
                job.kind,
                job.ready_before,
            )
        });
        let jobs = scheduled
            .iter()
            .copied()
            .filter(|job| job.enqueued_at > observed_at)
            .collect::<Vec<_>>();
        let mut producers: Vec<_> = scheduled.iter().map(|job| job.producer).collect();
        producers.sort_unstable();
        producers.dedup();
        let current_jobs = scheduled
            .iter()
            .copied()
            .filter(|job| job.enqueued_at == observed_at)
            .collect::<Vec<_>>();
        let baselines = producers
            .into_iter()
            .map(|producer| {
                let mut baseline = resources
                    .producer(producer)
                    .cloned()
                    .ok_or(ProducerLaneReservationError::UnknownProducer { producer })?;
                for due in scheduled
                    .iter()
                    .filter(|job| job.producer == producer && job.enqueued_at == observed_at)
                {
                    let projected = baseline.append(due.kind, observed_at).ok_or(
                        ProducerLaneReservationError::CurrentAppendUnavailable {
                            producer,
                            kind: due.kind,
                        },
                    )?;
                    if (projected.starts_at, projected.ready_at) != (due.starts_at, due.ready_at) {
                        return Err(ProducerLaneReservationError::CurrentTimingMismatch {
                            producer,
                            kind: due.kind,
                        });
                    }
                }
                Ok(baseline)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            baselines,
            current_jobs,
            jobs,
        })
    }

    /// Whether this exact producer carries accepted work on or after this tick.
    pub(crate) fn reserves(&self, producer: BuildingId) -> bool {
        self.baselines
            .binary_search_by_key(&producer, ProducerPlanningProjection::producer)
            .is_ok()
    }

    /// Whether one append against an already-projected observation preserves
    /// every accepted future append on the same exact producer.
    ///
    /// `prior_immediate` contains only commands added after the accepted
    /// current-tick schedule represented by this baseline.
    pub(crate) fn allows_immediate_append(
        &self,
        producer: BuildingId,
        prior_immediate: &[UnitKind],
        kind: UnitKind,
    ) -> bool {
        if !self.reserves(producer) {
            return true;
        }
        let index = self
            .baselines
            .binary_search_by_key(&producer, ProducerPlanningProjection::producer)
            .expect("a reserved producer retains its baseline projection");
        let mut lane = self.baselines[index].clone();
        let observed_at = lane.observed_at();
        for &planned in prior_immediate {
            if lane.append(planned, observed_at).is_none() {
                return false;
            }
        }
        if lane.append(kind, observed_at).is_none() {
            return false;
        }
        self.jobs
            .iter()
            .filter(|job| job.producer == producer)
            .all(|job| {
                lane.append(job.kind, job.enqueued_at)
                    .is_some_and(|projected| {
                        projected.starts_at == job.starts_at && projected.ready_at == job.ready_at
                    })
            })
    }

    /// Whether one append against the raw observation preserves every accepted
    /// future append on the same exact producer.
    ///
    /// The exact accepted current-tick prefix is already represented by the
    /// baseline. Callers pass all staged producer commands, including that
    /// prefix; only later residual commands are replayed.
    pub(crate) fn allows_raw_immediate_append(
        &self,
        producer: BuildingId,
        prior_immediate: &[UnitKind],
        kind: UnitKind,
    ) -> bool {
        if !self.reserves(producer) {
            return true;
        }
        let represented_current = self
            .current_jobs
            .iter()
            .filter(|job| job.producer == producer)
            .map(|job| job.kind)
            .collect::<Vec<_>>();
        let Some(residual) = prior_immediate.strip_prefix(represented_current.as_slice()) else {
            return false;
        };
        self.allows_immediate_append(producer, residual, kind)
    }

    /// Accepted appends due on this decision tick for one exact producer.
    pub(crate) fn current_job_count(&self, producer: BuildingId) -> usize {
        self.current_jobs
            .iter()
            .filter(|job| job.producer == producer)
            .count()
    }

    /// Accepted appends due on this decision tick for one exact unit kind.
    pub(crate) fn current_kind_count(&self, kind: UnitKind) -> usize {
        self.current_jobs
            .iter()
            .filter(|job| job.kind == kind)
            .count()
    }

    /// Latest completion tick already promised on one exact producer.
    pub(crate) fn latest_ready_at(&self, producer: BuildingId) -> Option<Tick> {
        self.current_jobs
            .iter()
            .chain(&self.jobs)
            .filter(|job| job.producer == producer)
            .map(|job| job.ready_at)
            .max()
    }

    /// Whether the shared allocation retained this exact accepted lane row.
    pub(crate) fn contains_exact_job(&self, job: ReservedProducerJob) -> bool {
        self.current_jobs
            .iter()
            .chain(&self.jobs)
            .any(|reserved| *reserved == job)
    }

    /// Exact future jobs retained by the overlay.
    #[cfg(test)]
    pub(crate) fn jobs(&self) -> &[ReservedProducerJob] {
        &self.jobs
    }

    /// Accepted current-tick appends in their exact producer schedule.
    #[cfg(test)]
    pub(crate) fn current_jobs(&self) -> &[ReservedProducerJob] {
        &self.current_jobs
    }
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
    /// Earliest tick the unit can be ready, using exact owner-visible front
    /// progress when available and otherwise the most favorable honest bound.
    pub(crate) earliest_ready_tick: Tick,
    /// Latest ready tick using exact owner-visible front progress when
    /// available, otherwise assuming it is zero, and assuming every ground
    /// spawn has an open doorstep when needed. This is a conditional no-block
    /// bound, not a promise that egress will remain available.
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
    /// Exact owner-visible progress of the current front item. `None` means
    /// the observation rows were malformed or came from a hand-built fixture,
    /// so deadline claims must retain the old conservative bound.
    front_progress: Option<u32>,
    /// Fewest production-phase executions consumed before an appended unit,
    /// accounting for an observed front item that may already be complete.
    earliest_preceding_ticks: Option<Tick>,
    /// Most production-phase executions consumed by current queue work with
    /// the available progress evidence and prompt spawn egress.
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
        if self.queued.len().saturating_add(planned.len()) > QUEUE_CAP {
            return None;
        }
        self.sequence_timing(planned)
    }

    /// Conservative readiness for a sequence that may refill queue slots as
    /// current work completes during a fixed planning horizon.
    ///
    /// This is feasibility evidence, not permission to enqueue beyond today's
    /// open slots. Command lowering continues to use [`Self::production_timing`]
    /// and the exact slots returned by [`Self::open_slots`].
    pub(crate) fn horizon_timing(&self, planned: &[UnitKind]) -> Option<ProductionTiming> {
        self.sequence_timing(planned)
    }

    /// Number of paid queued units of `kind` conservatively ready before an
    /// operation deadline.
    pub(crate) fn queued_kind_ready_before(&self, kind: UnitKind, ready_before: Tick) -> usize {
        let mut elapsed = 0_u64;
        let mut ready = 0_usize;
        for (index, queued) in self.queued.iter().copied().enumerate() {
            let ticks = if index == 0 {
                self.front_progress.map_or_else(
                    || Tick::from(queued.stats().train_ticks),
                    |progress| {
                        Tick::from(queued.stats().train_ticks.saturating_sub(progress).max(1))
                    },
                )
            } else {
                Tick::from(queued.stats().train_ticks)
            };
            let Some(next_elapsed) = elapsed.checked_add(ticks) else {
                break;
            };
            elapsed = next_elapsed;
            let Some(ready_at) = self
                .observed_at
                .checked_add(elapsed)
                .and_then(|tick| tick.checked_sub(1))
            else {
                break;
            };
            if ready_at >= ready_before {
                break;
            }
            ready += usize::from(queued == kind);
        }
        ready
    }

    fn sequence_timing(&self, planned: &[UnitKind]) -> Option<ProductionTiming> {
        if planned.iter().any(|kind| !self.trainable.contains(kind)) {
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
            .enumerate()
            .filter(|(_, (building, _))| {
                building.player == obs.me
                    && building.built
                    && building.hp > 0
                    && !building.kind.tier_stats(building.tier).produces.is_empty()
            })
            .map(|(index, (building, queue))| {
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
                let front_progress = obs.own_queue_progress(index).filter(|_| !queued.is_empty());
                let (earliest_preceding_ticks, no_block_latest_preceding_ticks) =
                    producer_preceding_ticks(&queued, front_progress);
                ProducerLane {
                    producer: building.id,
                    kind: building.kind,
                    queued,
                    trainable,
                    observed_at: obs.tick,
                    front_progress,
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

    /// Returns the same observed resources after protecting a named share of
    /// the current bank from a planning domain.
    ///
    /// Forecast income and exact actor capacity remain tied to the original
    /// observation. A reserve larger than the bank leaves zero spendable
    /// current scrap; it can never manufacture planning capital.
    pub(crate) fn after_current_reserve(&self, reserve: u32) -> Self {
        let mut available = self.clone();
        available.current_scrap = CurrentScrap(self.current_scrap.0.saturating_sub(reserve));
        available
    }

    /// Conservative forecast from completed income sources.
    pub(crate) const fn forecast(&self) -> &ResourceForecast {
        &self.forecast
    }

    /// Own units in id order.
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

fn producer_preceding_ticks(
    queued: &[UnitKind],
    front_progress: Option<u32>,
) -> (Option<Tick>, Option<Tick>) {
    if queued.is_empty() {
        return (Some(0), Some(0));
    }
    if let Some(progress) = front_progress {
        let front_ticks = queued[0].stats().train_ticks;
        let remaining_front = Tick::from(front_ticks.saturating_sub(progress).max(1));
        let exact = queued
            .iter()
            .skip(1)
            .try_fold(remaining_front, |ticks, kind| {
                ticks.checked_add(Tick::from(kind.stats().train_ticks))
            });
        return (exact, exact);
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

#[cfg(test)]
mod current_reserve_tests {
    use super::*;

    fn snapshot_with_current_scrap(current_scrap: u32) -> ResourceSnapshot {
        ResourceSnapshot {
            current_scrap: CurrentScrap(current_scrap),
            forecast: ResourceForecast {
                observed_at: 40,
                income: vec![RecurringIncomeStream {
                    source: BuildingId(7),
                    kind: RecurringIncomeKind::Foundry,
                    amount: 1,
                    period: 20,
                    first_payment_tick: 59,
                }],
            },
            units: vec![UnitResource {
                id: UnitId(3),
                kind: UnitKind::Harvester,
            }],
            builders: Vec::new(),
            producers: Vec::new(),
            producer_slots: Vec::new(),
        }
    }

    #[test]
    fn current_reserve_changes_only_the_planning_bank() {
        let original = snapshot_with_current_scrap(90);
        let available = original.after_current_reserve(35);

        assert_eq!(original.current_scrap().amount(), 90);
        assert_eq!(available.current_scrap().amount(), 55);
        assert_eq!(available.forecast, original.forecast);
        assert_eq!(available.units, original.units);
        assert_eq!(available.builders, original.builders);
        assert_eq!(available.producers, original.producers);
        assert_eq!(available.producer_slots, original.producer_slots);
    }

    #[test]
    fn current_reserve_saturates_without_increasing_the_bank() {
        let original = snapshot_with_current_scrap(12);

        assert_eq!(
            original.after_current_reserve(0).current_scrap().amount(),
            12
        );
        assert_eq!(
            original.after_current_reserve(50).current_scrap().amount(),
            0
        );
    }

    fn producer_projection() -> ResourcePlanningProjection {
        ResourcePlanningProjection::fixture(ResourcePlanningFixture {
            current_scrap: 500,
            observed_at: 0,
            horizon: 1_000,
            cadence: 1,
            forecast_income: Vec::new(),
            units: Vec::new(),
            builders: Vec::new(),
            producers: vec![
                ProducerPlanningProjection::fixture(
                    BuildingId(7),
                    0,
                    1,
                    0,
                    vec![0; QUEUE_CAP],
                    vec![UnitKind::Sentinel],
                )
                .expect("the producer fixture is valid"),
            ],
        })
        .expect("the resource fixture is valid")
    }

    fn reserved_job(
        producer: BuildingId,
        kind: UnitKind,
        enqueued_at: Tick,
        starts_at: Tick,
        ready_at: Tick,
    ) -> ReservedProducerJob {
        ReservedProducerJob {
            producer,
            kind,
            enqueued_at,
            starts_at,
            ready_at,
            ready_before: ready_at.saturating_add(1),
        }
    }

    #[test]
    fn producer_reservations_reject_an_unknown_accepted_lane_without_panicking() {
        let resources = producer_projection();
        let result = ProducerLaneReservations::from_jobs(
            &resources,
            [reserved_job(
                BuildingId(99),
                UnitKind::Sentinel,
                0,
                0,
                Tick::from(UnitKind::Sentinel.stats().train_ticks).saturating_sub(1),
            )],
        );

        assert_eq!(
            result,
            Err(ProducerLaneReservationError::UnknownProducer {
                producer: BuildingId(99),
            })
        );
    }

    #[test]
    fn producer_reservations_reject_changed_current_timing_without_panicking() {
        let resources = producer_projection();
        let train_ticks = Tick::from(UnitKind::Sentinel.stats().train_ticks);
        let due_now = reserved_job(BuildingId(7), UnitKind::Sentinel, 0, 0, train_ticks);
        let future = reserved_job(
            BuildingId(7),
            UnitKind::Sentinel,
            train_ticks.saturating_add(1),
            train_ticks.saturating_add(1),
            train_ticks.saturating_mul(2),
        );

        assert_eq!(
            ProducerLaneReservations::from_jobs(&resources, [due_now, future]),
            Err(ProducerLaneReservationError::CurrentTimingMismatch {
                producer: BuildingId(7),
                kind: UnitKind::Sentinel,
            })
        );
    }

    #[test]
    fn producer_reservations_recognize_only_exact_current_and_future_jobs() {
        let resources = producer_projection();
        let train_ticks = Tick::from(UnitKind::Sentinel.stats().train_ticks);
        let current = reserved_job(
            BuildingId(7),
            UnitKind::Sentinel,
            0,
            0,
            train_ticks.saturating_sub(1),
        );
        let future = reserved_job(
            BuildingId(7),
            UnitKind::Sentinel,
            train_ticks,
            train_ticks,
            train_ticks.saturating_mul(2).saturating_sub(1),
        );
        let reservations = ProducerLaneReservations::from_jobs(&resources, [current, future])
            .expect("the exact two-job lane is valid");

        assert!(reservations.contains_exact_job(current));
        assert!(reservations.contains_exact_job(future));
        assert!(!reservations.contains_exact_job(ReservedProducerJob {
            ready_at: future.ready_at.saturating_sub(1),
            ..future
        }));
    }
}
