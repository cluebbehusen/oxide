//! Integer-only marginal economic returns. These quotes never fund commands.

use crate::stats::UnitKind;

const BASE_HORIZON_TICKS: u64 = 3_600;
const GREED_HORIZON_TICKS: u64 = 36;
const SURPLUS_HORIZON_TICKS: u64 = 6;
const SURPLUS_HORIZON_CAP: u32 = 300;

pub(in crate::bot) fn investment_horizon(greed: u8, surplus: u32) -> u64 {
    BASE_HORIZON_TICKS
        .saturating_add(u64::from(greed).saturating_mul(GREED_HORIZON_TICKS))
        .saturating_add(
            u64::from(surplus.min(SURPLUS_HORIZON_CAP)).saturating_mul(SURPLUS_HORIZON_TICKS),
        )
}

/// A source is finite even when several workers can reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HarvestWork {
    pub(super) amount: u64,
    pub(super) positions: usize,
    pub(super) haul_cost: u32,
}

/// Readiness includes training and the initial journey to the work region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WorkerService {
    pub(super) kind: UnitKind,
    pub(super) ready_after: u64,
}

impl WorkerService {
    fn output(self, work: HarvestWork, horizon: u64) -> u64 {
        let stats = self.kind.stats();
        let Some(harvest) = stats.harvest else {
            return 0;
        };
        let load = u64::from(harvest.capacity);
        if load == 0 {
            return 0;
        }
        let travel = travel_ticks(self.kind, work.haul_cost).saturating_mul(2);
        let cycle = travel
            .saturating_add(load.saturating_mul(u64::from(harvest.ticks_per_scrap)))
            .max(1);
        let cycles = horizon.saturating_sub(self.ready_after) / cycle;
        cycles.saturating_mul(load).min(work.amount)
    }
}

/// Route costs use ten for an axial tile and fourteen for a diagonal tile.
pub(super) fn travel_ticks(kind: UnitKind, route_cost: u32) -> u64 {
    let speed = kind.stats().speed.to_bits();
    if speed <= 0 {
        return u64::MAX;
    }
    let distance = u128::from(route_cost) << 32;
    let ticks = distance.div_ceil((speed as u128).saturating_mul(10));
    u64::try_from(ticks).unwrap_or(u64::MAX)
}

pub(super) fn harvest_output(work: HarvestWork, workers: &[WorkerService], horizon: u64) -> u64 {
    let mut outputs = workers
        .iter()
        .map(|worker| worker.output(work, horizon))
        .collect::<Vec<_>>();
    outputs.sort_unstable_by(|a, b| b.cmp(a));
    outputs
        .into_iter()
        .take(work.positions)
        .fold(0, u64::saturating_add)
        .min(work.amount)
}

pub(super) fn marginal_worker_return(
    work: HarvestWork,
    workers: &[WorkerService],
    candidate: WorkerService,
    horizon: u64,
) -> u64 {
    let before = harvest_output(work, workers, horizon);
    let mut after = workers.to_vec();
    after.push(candidate);
    harvest_output(work, &after, horizon).saturating_sub(before)
}

/// Eventual supply limits new investment; it is separate from forecast credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RecurringReturn {
    pub(super) horizon: u64,
    pub(super) ready_after: u64,
    pub(super) old_period: Option<u64>,
    pub(super) new_period: u64,
    pub(super) unmet_demand: u64,
}

impl RecurringReturn {
    pub(super) fn marginal(self) -> u64 {
        if self.new_period == 0 {
            return 0;
        }
        let after = self.horizon.saturating_sub(self.ready_after) / self.new_period;
        let before = self
            .old_period
            .filter(|period| *period > 0)
            .map_or(0, |period| self.horizon / period);
        after.saturating_sub(before).min(self.unmet_demand)
    }
}

/// New infrastructure earns value only for useful work existing lanes miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CapacityReturn {
    pub(super) horizon: u64,
    pub(super) ready_after: u64,
    pub(super) train_ticks: u64,
    pub(super) demanded_units: u64,
    pub(super) existing_units: u64,
}

impl CapacityReturn {
    pub(super) fn additional_units(self) -> u64 {
        if self.train_ticks == 0 {
            return 0;
        }
        let throughput = self.horizon.saturating_sub(self.ready_after) / self.train_ticks;
        throughput.min(self.demanded_units.saturating_sub(self.existing_units))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(kind: UnitKind, ready_after: u64) -> WorkerService {
        WorkerService { kind, ready_after }
    }

    #[test]
    fn rich_work_remains_useful_beyond_the_old_worker_ceiling() {
        let work = HarvestWork {
            amount: 100_000,
            positions: 20,
            haul_cost: 80,
        };
        let workers = vec![worker(UnitKind::Harvester, 0); 8];
        assert!(
            marginal_worker_return(work, &workers, worker(UnitKind::Harvester, 200), 6_000)
                > u64::from(UnitKind::Harvester.stats().cost)
        );
    }

    #[test]
    fn finite_sources_and_work_positions_bound_total_return() {
        let worker = worker(UnitKind::Harvester, 0);
        let work = HarvestWork {
            amount: 30,
            positions: 8,
            haul_cost: 10,
        };
        assert_eq!(harvest_output(work, &[worker; 8], 6_000), 30);
        assert_eq!(marginal_worker_return(work, &[worker], worker, 6_000), 0);
        let full = HarvestWork {
            amount: 100_000,
            positions: 2,
            haul_cost: 10,
        };
        assert_eq!(marginal_worker_return(full, &[worker; 2], worker, 6_000), 0);
        assert_eq!(
            harvest_output(
                HarvestWork {
                    positions: 0,
                    ..full
                },
                &[worker],
                6_000
            ),
            0
        );
    }

    #[test]
    fn paid_queue_readiness_and_haul_delay_are_not_free_production() {
        let work = HarvestWork {
            amount: 100_000,
            positions: 8,
            haul_cost: 100,
        };
        let live = worker(UnitKind::Harvester, 0);
        let queued = worker(UnitKind::Harvester, 2_000);
        assert!(harvest_output(work, &[live], 3_000) > harvest_output(work, &[queued], 3_000));
        assert_eq!(harvest_output(work, &[queued], 2_000), 0);
        assert!(
            harvest_output(work, &[live], 3_000)
                > harvest_output(
                    HarvestWork {
                        haul_cost: 300,
                        ..work
                    },
                    &[live],
                    3_000
                )
        );
        assert_eq!(travel_ticks(UnitKind::Harvester, 0), 0);
    }

    #[test]
    fn a_specialist_is_priced_by_its_actual_work_not_its_tier() {
        let work = HarvestWork {
            amount: 100_000,
            positions: 8,
            haul_cost: 50,
        };
        let harvester = worker(UnitKind::Harvester, 0);
        let excavator = worker(UnitKind::Excavator, 0);
        assert!(
            harvest_output(work, &[excavator], 6_000) > harvest_output(work, &[harvester], 6_000)
        );
        assert_eq!(
            harvest_output(work, &[worker(UnitKind::Sentinel, 0)], 6_000),
            0
        );
        assert_eq!(
            marginal_worker_return(HarvestWork { amount: 0, ..work }, &[], excavator, 6_000),
            0
        );
    }

    #[test]
    fn refit_return_loses_the_old_income_during_downtime() {
        let quote = RecurringReturn {
            horizon: 6_000,
            ready_after: 300,
            old_period: Some(24),
            new_period: 10,
            unmet_demand: u64::MAX,
        };
        assert_eq!(quote.marginal(), 570 - 250);
        assert_eq!(
            RecurringReturn {
                unmet_demand: 7,
                ..quote
            }
            .marginal(),
            7
        );
        assert_eq!(
            RecurringReturn {
                ready_after: 6_000,
                ..quote
            }
            .marginal(),
            0
        );
        assert_eq!(
            RecurringReturn {
                new_period: 0,
                ..quote
            }
            .marginal(),
            0
        );
        assert_eq!(
            RecurringReturn {
                old_period: None,
                ..quote
            }
            .marginal(),
            570
        );
    }

    #[test]
    fn new_capacity_needs_a_customer_and_time_to_deliver() {
        let quote = CapacityReturn {
            horizon: 1_000,
            ready_after: 400,
            train_ticks: 300,
            demanded_units: 20,
            existing_units: 3,
        };
        assert_eq!(quote.additional_units(), 2);
        assert_eq!(
            CapacityReturn {
                ready_after: 401,
                ..quote
            }
            .additional_units(),
            1
        );
        assert_eq!(
            CapacityReturn {
                existing_units: 20,
                ..quote
            }
            .additional_units(),
            0
        );
        assert_eq!(
            CapacityReturn {
                ready_after: 1_001,
                ..quote
            }
            .additional_units(),
            0
        );
        assert_eq!(
            CapacityReturn {
                train_ticks: 0,
                ..quote
            }
            .additional_units(),
            0
        );
    }

    #[test]
    fn wealth_extends_a_bounded_horizon_without_changing_work() {
        assert_eq!(investment_horizon(0, 0), 3_600);
        assert!(investment_horizon(100, 0) > investment_horizon(0, 0));
        assert!(investment_horizon(100, 300) > investment_horizon(100, 0));
        assert_eq!(
            investment_horizon(100, 300),
            investment_horizon(100, u32::MAX)
        );
        assert!(travel_ticks(UnitKind::Harvester, u32::MAX) > 0);
    }
}
