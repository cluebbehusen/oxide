//! Bounded, fog-honest projections used by cross-domain allocation.

use super::{BuilderResource, ProducerEgress, ProducerLane, ResourceSnapshot};
use crate::ids::{BuildingId, UnitId};
use crate::stats::{Domain, QUEUE_CAP, UnitKind};
use chassis::Tick;

/// Forecast income at the first decision tick on which it may fund a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ForecastAvailability {
    /// Cadence-aligned command tick whose opening bank includes this income.
    pub(crate) available_at: Tick,
    /// Income newly available at this decision boundary.
    pub(crate) amount: u32,
}

/// A producer append projected from the authoritative paid queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProjectedProduction {
    /// First production tick after all earlier FIFO work.
    pub(crate) starts_at: Tick,
    /// Production tick on which the unit can first spawn.
    pub(crate) ready_at: Tick,
}

/// Why an authoritative resource snapshot cannot support a bounded projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanningProjectionError {
    /// A zero decision cadence cannot identify future command boundaries.
    ZeroCadence,
    /// Resource observations used for commands must occur on the bot cadence.
    ObservationOffCadence {
        /// Current observation tick.
        observed_at: Tick,
        /// Configured bot decision cadence.
        cadence: Tick,
    },
    /// The forecast must extend beyond the observation boundary.
    EmptyHorizon {
        /// Current observation tick.
        observed_at: Tick,
        /// Requested inclusive horizon.
        horizon: Tick,
    },
    /// An observed producer queue exceeds the simulation queue capacity.
    QueueBeyondCapacity {
        /// Exact producer.
        producer: BuildingId,
        /// Observed queue length.
        queued: usize,
    },
    /// Owner-visible front progress exceeds the front unit's train time.
    MalformedFrontProgress {
        /// Exact producer.
        producer: BuildingId,
        /// Observed progress.
        progress: u32,
        /// Front unit's complete train time.
        train_ticks: u32,
    },
    /// Queue, cadence, or horizon arithmetic cannot be represented in ticks.
    TickOverflow,
    /// Completed-source forecast income exceeds the simulation scrap type.
    ForecastOverflow {
        /// Last production tick included in the failed sum.
        through: Tick,
    },
    /// A recurring source cannot make progress with a zero payment period.
    ZeroIncomePeriod {
        /// Exact completed income source.
        source: BuildingId,
    },
}

/// Mutable projection of one completed producer's FIFO and queue slots.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProducerPlanningProjection {
    producer: BuildingId,
    observed_at: Tick,
    cadence: Tick,
    production_available_at: Tick,
    last_enqueue_at: Tick,
    slot_available_at: Vec<Tick>,
    trainable: Vec<UnitKind>,
}

impl ProducerPlanningProjection {
    fn from_lane(lane: &ProducerLane, cadence: Tick) -> Result<Self, PlanningProjectionError> {
        if lane.queued.len() > QUEUE_CAP {
            return Err(PlanningProjectionError::QueueBeyondCapacity {
                producer: lane.producer,
                queued: lane.queued.len(),
            });
        }

        let paid_ready = paid_queue_ready_ticks(lane)?;
        let paid_outputs_credible = lane.queued.iter().all(|&kind| credible_egress(lane, kind));
        let mut trainable = UnitKind::ALL
            .into_iter()
            .filter(|&kind| {
                paid_outputs_credible
                    && lane
                        .horizon_timing(&[kind])
                        .is_some_and(|timing| credible(timing.current_egress))
            })
            .collect::<Vec<_>>();
        trainable.sort_unstable();

        let production_available_at = paid_ready.last().map_or(Ok(lane.observed_at), |ready| {
            ready
                .checked_add(1)
                .ok_or(PlanningProjectionError::TickOverflow)
        })?;
        let mut slot_available_at = vec![lane.observed_at; QUEUE_CAP - lane.queued.len()];
        for ready in paid_ready {
            slot_available_at.push(
                next_decision_strictly_after(lane.observed_at, cadence, ready)
                    .ok_or(PlanningProjectionError::TickOverflow)?,
            );
        }
        slot_available_at.sort_unstable();

        Ok(Self {
            producer: lane.producer,
            observed_at: lane.observed_at,
            cadence,
            production_available_at,
            last_enqueue_at: lane.observed_at,
            slot_available_at,
            trainable,
        })
    }

    /// Exact producer represented by this projection.
    pub(crate) const fn producer(&self) -> BuildingId {
        self.producer
    }

    /// Observation boundary from which this lane was projected.
    pub(crate) const fn observed_at(&self) -> Tick {
        self.observed_at
    }

    /// First decision tick with a queue slot for this exact legal unit kind.
    pub(crate) fn earliest_enqueue_tick(&self, kind: UnitKind) -> Option<Tick> {
        self.trainable
            .binary_search(&kind)
            .is_ok()
            .then(|| {
                self.slot_available_at
                    .first()
                    .copied()
                    .map(|slot| slot.max(self.last_enqueue_at))
            })
            .flatten()
    }

    /// Appends one paid job at a real decision boundary and advances FIFO state.
    pub(crate) fn append(
        &mut self,
        kind: UnitKind,
        enqueued_at: Tick,
    ) -> Option<ProjectedProduction> {
        if self.trainable.binary_search(&kind).is_err()
            || !is_decision_tick(self.observed_at, self.cadence, enqueued_at)
            || enqueued_at < self.last_enqueue_at
            || self.slot_available_at.first().copied()? > enqueued_at
        {
            return None;
        }

        self.slot_available_at.remove(0);
        let starts_at = self.production_available_at.max(enqueued_at);
        let ready_at = starts_at
            .checked_add(Tick::from(kind.stats().train_ticks))?
            .checked_sub(1)?;
        self.production_available_at = ready_at.checked_add(1)?;
        self.last_enqueue_at = enqueued_at;
        let slot_returns = next_decision_strictly_after(self.observed_at, self.cadence, ready_at)?;
        let index = self
            .slot_available_at
            .binary_search(&slot_returns)
            .unwrap_or_else(|index| index);
        self.slot_available_at.insert(index, slot_returns);
        Some(ProjectedProduction {
            starts_at,
            ready_at,
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        producer: BuildingId,
        observed_at: Tick,
        cadence: Tick,
        production_available_at: Tick,
        mut slot_available_at: Vec<Tick>,
        mut trainable: Vec<UnitKind>,
    ) -> Result<Self, PlanningProjectionError> {
        validate_cadence(observed_at, cadence)?;
        slot_available_at.sort_unstable();
        trainable.sort_unstable();
        trainable.dedup();
        Ok(Self {
            producer,
            observed_at,
            cadence,
            production_available_at,
            last_enqueue_at: observed_at,
            slot_available_at,
            trainable,
        })
    }
}

/// Resource-owned evidence for one bounded cross-domain allocation pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourcePlanningProjection {
    current_scrap: u32,
    observed_at: Tick,
    horizon: Tick,
    cadence: Tick,
    forecast_income: Vec<ForecastAvailability>,
    units: Vec<UnitId>,
    builders: Vec<BuilderResource>,
    producers: Vec<ProducerPlanningProjection>,
}

#[cfg(test)]
/// Explicit resource evidence used by sibling-module allocation tests.
pub(crate) struct ResourcePlanningFixture {
    /// Spendable bank at the observation boundary.
    pub(crate) current_scrap: u32,
    /// Tick before the next command phase.
    pub(crate) observed_at: Tick,
    /// Inclusive planning horizon.
    pub(crate) horizon: Tick,
    /// Global bot decision cadence.
    pub(crate) cadence: Tick,
    /// Income grouped by first spendable decision tick.
    pub(crate) forecast_income: Vec<ForecastAvailability>,
    /// Exact own-unit roster.
    pub(crate) units: Vec<UnitId>,
    /// Exact construction-capable roster and current obligations.
    pub(crate) builders: Vec<BuilderResource>,
    /// Exact completed producer projections.
    pub(crate) producers: Vec<ProducerPlanningProjection>,
}

impl ResourcePlanningProjection {
    /// Observation boundary shared by every resource in this projection.
    pub(crate) const fn observed_at(&self) -> Tick {
        self.observed_at
    }

    /// Observed spendable bank; forecast income remains separate.
    pub(crate) const fn current_scrap(&self) -> u32 {
        self.current_scrap
    }

    /// Last tick covered by the deliberately bounded projection.
    pub(crate) const fn horizon(&self) -> Tick {
        self.horizon
    }

    /// Whether an exact unit existed in the canonical resource snapshot.
    pub(crate) fn contains_unit(&self, unit: UnitId) -> bool {
        self.units.binary_search(&unit).is_ok()
    }

    /// Whether an exact builder exists and, for fresh work, is currently free.
    pub(crate) fn contains_builder(&self, unit: UnitId, require_free: bool) -> bool {
        self.builders
            .binary_search_by_key(&unit, |builder| builder.id)
            .ok()
            .is_some_and(|index| !require_free || self.builders[index].obligation.is_none())
    }

    /// Canonical producer projection for one exact completed building.
    pub(crate) fn producer(&self, producer: BuildingId) -> Option<&ProducerPlanningProjection> {
        self.producers
            .binary_search_by_key(&producer, ProducerPlanningProjection::producer)
            .ok()
            .map(|index| &self.producers[index])
    }

    /// Canonical mutable producer bases for the exact joint search.
    pub(crate) fn producers(&self) -> &[ProducerPlanningProjection] {
        &self.producers
    }

    /// Future decision boundaries at which new income becomes spendable.
    pub(crate) fn forecast_income(&self) -> &[ForecastAvailability] {
        &self.forecast_income
    }

    /// Completed-source income spendable by one command tick.
    pub(crate) fn forecast_through(&self, through: Tick) -> u64 {
        self.forecast_income
            .iter()
            .take_while(|income| income.available_at <= through)
            .map(|income| u64::from(income.amount))
            .sum()
    }

    /// First cadence-aligned decision tick at or after a lower bound.
    pub(crate) fn decision_at_or_after(&self, tick: Tick) -> Option<Tick> {
        next_decision_at_or_after(self.observed_at, self.cadence, tick)
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        fixture: ResourcePlanningFixture,
    ) -> Result<Self, PlanningProjectionError> {
        let ResourcePlanningFixture {
            current_scrap,
            observed_at,
            horizon,
            cadence,
            mut forecast_income,
            mut units,
            mut builders,
            mut producers,
        } = fixture;
        validate_cadence(observed_at, cadence)?;
        if horizon <= observed_at {
            return Err(PlanningProjectionError::EmptyHorizon {
                observed_at,
                horizon,
            });
        }
        forecast_income.sort_unstable_by_key(|income| income.available_at);
        units.sort_unstable();
        builders.sort_unstable_by_key(|builder| builder.id);
        producers.sort_unstable_by_key(ProducerPlanningProjection::producer);
        Ok(Self {
            current_scrap,
            observed_at,
            horizon,
            cadence,
            forecast_income,
            units,
            builders,
            producers,
        })
    }
}

impl ResourceSnapshot {
    /// Projects only resource facts needed by one bounded allocation pass.
    pub(crate) fn planning_projection(
        &self,
        horizon: Tick,
        cadence: Tick,
    ) -> Result<ResourcePlanningProjection, PlanningProjectionError> {
        let observed_at = self.forecast.observed_at;
        validate_cadence(observed_at, cadence)?;
        if horizon <= observed_at {
            return Err(PlanningProjectionError::EmptyHorizon {
                observed_at,
                horizon,
            });
        }

        let forecast_income = decision_income(self, horizon, cadence)?;
        let producers = self
            .producers
            .iter()
            .map(|lane| ProducerPlanningProjection::from_lane(lane, cadence))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ResourcePlanningProjection {
            current_scrap: self.current_scrap.amount(),
            observed_at,
            horizon,
            cadence,
            forecast_income,
            units: self.units.iter().map(|unit| unit.id).collect(),
            builders: self.builders.clone(),
            producers,
        })
    }
}

fn paid_queue_ready_ticks(lane: &ProducerLane) -> Result<Vec<Tick>, PlanningProjectionError> {
    let Some(&front) = lane.queued.first() else {
        return Ok(Vec::new());
    };
    let progress = lane.front_progress.unwrap_or(0);
    if progress > front.stats().train_ticks {
        return Err(PlanningProjectionError::MalformedFrontProgress {
            producer: lane.producer,
            progress,
            train_ticks: front.stats().train_ticks,
        });
    }
    let remaining = Tick::from(front.stats().train_ticks.saturating_sub(progress).max(1));
    let mut ready = lane
        .observed_at
        .checked_add(remaining)
        .and_then(|tick| tick.checked_sub(1))
        .ok_or(PlanningProjectionError::TickOverflow)?;
    let mut ready_ticks = vec![ready];
    for kind in lane.queued.iter().skip(1) {
        ready = ready
            .checked_add(Tick::from(kind.stats().train_ticks))
            .ok_or(PlanningProjectionError::TickOverflow)?;
        ready_ticks.push(ready);
    }
    Ok(ready_ticks)
}

fn credible_egress(lane: &ProducerLane, kind: UnitKind) -> bool {
    if lane.kind == crate::stats::BuildingKind::Airworks && kind.stats().domain == Domain::Air {
        true
    } else {
        credible(lane.ground_egress)
    }
}

fn credible(egress: ProducerEgress) -> bool {
    matches!(egress, ProducerEgress::NotRequired | ProducerEgress::Open)
}

fn decision_income(
    resources: &ResourceSnapshot,
    horizon: Tick,
    cadence: Tick,
) -> Result<Vec<ForecastAvailability>, PlanningProjectionError> {
    let observed_at = resources.forecast.observed_at;
    let mut decision = observed_at
        .checked_add(cadence)
        .ok_or(PlanningProjectionError::TickOverflow)?;
    let mut prior = 0_u32;
    let mut result = Vec::new();
    while decision <= horizon {
        let through = decision
            .checked_sub(1)
            .ok_or(PlanningProjectionError::TickOverflow)?;
        let available = checked_income_through(resources, through)?;
        let amount = available.saturating_sub(prior);
        if amount > 0 {
            result.push(ForecastAvailability {
                available_at: decision,
                amount,
            });
        }
        prior = available;
        let Some(next) = decision.checked_add(cadence) else {
            if decision < horizon {
                return Err(PlanningProjectionError::TickOverflow);
            }
            break;
        };
        decision = next;
    }
    Ok(result)
}

fn checked_income_through(
    resources: &ResourceSnapshot,
    through: Tick,
) -> Result<u32, PlanningProjectionError> {
    resources
        .forecast
        .income
        .iter()
        .try_fold(0_u32, |sum, stream| {
            if stream.period == 0 {
                return Err(PlanningProjectionError::ZeroIncomePeriod {
                    source: stream.source,
                });
            }
            if through < stream.first_payment_tick {
                return Ok(sum);
            }
            let payments = through
                .saturating_sub(stream.first_payment_tick)
                .checked_div(stream.period)
                .expect("the zero period was rejected")
                .saturating_add(1);
            let income = u32::try_from(payments)
                .ok()
                .and_then(|payments| payments.checked_mul(stream.amount))
                .ok_or(PlanningProjectionError::ForecastOverflow { through })?;
            sum.checked_add(income)
                .ok_or(PlanningProjectionError::ForecastOverflow { through })
        })
}

fn is_decision_tick(observed_at: Tick, cadence: Tick, tick: Tick) -> bool {
    cadence != 0 && tick >= observed_at && tick.is_multiple_of(cadence)
}

fn next_decision_at_or_after(observed_at: Tick, cadence: Tick, tick: Tick) -> Option<Tick> {
    if cadence == 0 || !observed_at.is_multiple_of(cadence) {
        return None;
    }
    if tick <= observed_at {
        return Some(observed_at);
    }
    tick.div_ceil(cadence).checked_mul(cadence)
}

fn next_decision_strictly_after(observed_at: Tick, cadence: Tick, tick: Tick) -> Option<Tick> {
    next_decision_at_or_after(observed_at, cadence, tick.checked_add(1)?)
}

fn validate_cadence(observed_at: Tick, cadence: Tick) -> Result<(), PlanningProjectionError> {
    if cadence == 0 {
        return Err(PlanningProjectionError::ZeroCadence);
    }
    if !observed_at.is_multiple_of(cadence) {
        return Err(PlanningProjectionError::ObservationOffCadence {
            observed_at,
            cadence,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::BuildingKind;

    fn lane(
        producer: u32,
        observed_at: Tick,
        queued: Vec<UnitKind>,
        front_progress: Option<u32>,
        trainable: Vec<UnitKind>,
        ground_egress: ProducerEgress,
    ) -> ProducerLane {
        let (earliest_preceding_ticks, no_block_latest_preceding_ticks) =
            super::super::producer_preceding_ticks(&queued, front_progress);
        ProducerLane {
            producer: BuildingId(producer),
            kind: BuildingKind::Foundry,
            queued,
            trainable,
            observed_at,
            front_progress,
            earliest_preceding_ticks,
            no_block_latest_preceding_ticks,
            ground_egress,
        }
    }

    fn snapshot(
        observed_at: Tick,
        income: Vec<super::super::RecurringIncomeStream>,
        producers: Vec<ProducerLane>,
    ) -> ResourceSnapshot {
        ResourceSnapshot {
            current_scrap: super::super::CurrentScrap(500),
            forecast: super::super::ResourceForecast {
                observed_at,
                income,
            },
            units: Vec::new(),
            builders: Vec::new(),
            producers,
            producer_slots: Vec::new(),
        }
    }

    fn income(
        first_payment_tick: Tick,
        period: Tick,
        amount: u32,
    ) -> super::super::RecurringIncomeStream {
        super::super::RecurringIncomeStream {
            source: BuildingId(90),
            kind: super::super::RecurringIncomeKind::Foundry,
            amount,
            period,
            first_payment_tick,
        }
    }

    #[test]
    fn append_now_enters_an_open_slot_behind_paid_fifo_work() {
        let observed_at = 12;
        let paid = UnitKind::Harvester;
        let fresh = UnitKind::Sentinel;
        let mut projection = ProducerPlanningProjection::from_lane(
            &lane(
                7,
                observed_at,
                vec![paid],
                Some(0),
                vec![paid, fresh],
                ProducerEgress::Open,
            ),
            12,
        )
        .expect("the owner-visible paid queue is valid");

        assert_eq!(projection.earliest_enqueue_tick(fresh), Some(observed_at));
        let appended = projection
            .append(fresh, observed_at)
            .expect("an open slot accepts a command now");
        let paid_ready = observed_at + Tick::from(paid.stats().train_ticks) - 1;
        assert_eq!(appended.starts_at, paid_ready + 1);
        assert_eq!(
            appended.ready_at,
            paid_ready + Tick::from(fresh.stats().train_ticks)
        );
    }

    #[test]
    fn missing_front_progress_uses_the_conservative_full_train_bound() {
        let observed_at = 12;
        let kind = UnitKind::Sentinel;
        let mut projection = ProducerPlanningProjection::from_lane(
            &lane(
                7,
                observed_at,
                vec![kind],
                None,
                vec![kind],
                ProducerEgress::Open,
            ),
            12,
        )
        .expect("missing progress retains a conservative no-block bound");

        let appended = projection
            .append(kind, observed_at)
            .expect("the paid queue leaves open append capacity");
        assert_eq!(
            appended.starts_at,
            observed_at + Tick::from(kind.stats().train_ticks)
        );
    }

    #[test]
    fn a_completion_on_a_decision_tick_reopens_its_slot_next_cadence() {
        let observed_at = 12;
        let kind = UnitKind::Sentinel;
        let mut projection = ProducerPlanningProjection::from_lane(
            &lane(
                7,
                observed_at,
                vec![kind; QUEUE_CAP],
                Some(kind.stats().train_ticks),
                vec![kind],
                ProducerEgress::Open,
            ),
            12,
        )
        .expect("the full paid queue is valid");

        assert_eq!(projection.earliest_enqueue_tick(kind), Some(24));
        assert!(projection.append(kind, observed_at).is_none());
        assert!(projection.append(kind, 24).is_some());
    }

    #[test]
    fn multiple_open_slots_accept_same_tick_payments_but_fifo_the_work() {
        let kind = UnitKind::Sentinel;
        let mut projection = ProducerPlanningProjection::from_lane(
            &lane(
                7,
                0,
                vec![kind; QUEUE_CAP - 2],
                Some(0),
                vec![kind],
                ProducerEgress::Open,
            ),
            12,
        )
        .expect("two queue slots are currently open");

        let first = projection.append(kind, 0).expect("first open slot");
        let second = projection.append(kind, 0).expect("second open slot");
        assert_eq!(second.starts_at, first.ready_at + 1);
        assert!(projection.append(kind, 0).is_none());
    }

    #[test]
    fn delayed_enqueue_leaves_an_idle_lane_idle_until_the_command_tick() {
        let kind = UnitKind::Sentinel;
        let mut projection = ProducerPlanningProjection::from_lane(
            &lane(7, 0, Vec::new(), None, vec![kind], ProducerEgress::Open),
            12,
        )
        .expect("the empty lane is valid");

        let appended = projection
            .append(kind, 24)
            .expect("the future command is cadence aligned");
        assert_eq!(appended.starts_at, 24);
        assert_eq!(
            appended.ready_at,
            24 + Tick::from(kind.stats().train_ticks) - 1
        );
        assert_eq!(projection.earliest_enqueue_tick(kind), Some(24));
        assert!(projection.append(kind, 12).is_none());
    }

    #[test]
    fn income_on_a_decision_tick_is_not_spendable_until_the_next_cadence() {
        let resources = snapshot(0, vec![income(12, 100, 25)], Vec::new());
        let projection = resources
            .planning_projection(36, 12)
            .expect("the bounded income projection is valid");

        assert_eq!(projection.forecast_through(12), 0);
        assert_eq!(projection.forecast_through(23), 0);
        assert_eq!(projection.forecast_through(24), 25);
        assert_eq!(
            projection.forecast_income(),
            &[ForecastAvailability {
                available_at: 24,
                amount: 25,
            }]
        );
    }

    #[test]
    fn cadence_is_global_and_projection_horizons_are_bounded() {
        let resources = snapshot(13, Vec::new(), Vec::new());
        assert_eq!(
            resources.planning_projection(36, 12),
            Err(PlanningProjectionError::ObservationOffCadence {
                observed_at: 13,
                cadence: 12,
            })
        );
        let resources = snapshot(12, Vec::new(), Vec::new());
        assert_eq!(
            resources.planning_projection(36, 0),
            Err(PlanningProjectionError::ZeroCadence)
        );
        assert_eq!(
            resources.planning_projection(12, 12),
            Err(PlanningProjectionError::EmptyHorizon {
                observed_at: 12,
                horizon: 12,
            })
        );
        assert_eq!(next_decision_at_or_after(12, 12, 25), Some(36));
        assert_eq!(next_decision_at_or_after(13, 12, 25), None);
    }

    #[test]
    fn malformed_paid_work_is_rejected_before_projection() {
        let kind = UnitKind::Sentinel;
        let malformed = lane(
            7,
            0,
            vec![kind],
            Some(kind.stats().train_ticks + 1),
            vec![kind],
            ProducerEgress::Open,
        );
        let resources = snapshot(0, Vec::new(), vec![malformed]);
        assert_eq!(
            resources.planning_projection(100, 1),
            Err(PlanningProjectionError::MalformedFrontProgress {
                producer: BuildingId(7),
                progress: kind.stats().train_ticks + 1,
                train_ticks: kind.stats().train_ticks,
            })
        );
    }

    #[test]
    fn blocked_or_unknown_ground_egress_cannot_back_a_deadline_claim() {
        let kind = UnitKind::Sentinel;
        for egress in [ProducerEgress::Blocked, ProducerEgress::Unknown] {
            let resources = snapshot(
                0,
                Vec::new(),
                vec![lane(7, 0, Vec::new(), None, vec![kind], egress)],
            );
            let projection = resources
                .planning_projection(100, 1)
                .expect("uncertain egress is representable but not usable");
            assert_eq!(
                projection
                    .producer(BuildingId(7))
                    .and_then(|lane| lane.earliest_enqueue_tick(kind)),
                None
            );
        }
    }

    #[test]
    fn malformed_capacity_and_forecast_arithmetic_fail_explicitly() {
        let kind = UnitKind::Sentinel;
        let overfull = lane(
            7,
            0,
            vec![kind; QUEUE_CAP + 1],
            Some(0),
            vec![kind],
            ProducerEgress::Open,
        );
        assert_eq!(
            snapshot(0, Vec::new(), vec![overfull]).planning_projection(100, 1),
            Err(PlanningProjectionError::QueueBeyondCapacity {
                producer: BuildingId(7),
                queued: QUEUE_CAP + 1,
            })
        );
        assert_eq!(
            snapshot(0, vec![income(0, 0, 1)], Vec::new()).planning_projection(2, 1),
            Err(PlanningProjectionError::ZeroIncomePeriod {
                source: BuildingId(90),
            })
        );
        let overflowing_queue = lane(
            7,
            Tick::MAX - 1,
            vec![kind],
            Some(0),
            vec![kind],
            ProducerEgress::Open,
        );
        assert_eq!(
            snapshot(Tick::MAX - 1, Vec::new(), vec![overflowing_queue])
                .planning_projection(Tick::MAX, 1),
            Err(PlanningProjectionError::TickOverflow)
        );
    }
}
