use super::*;

impl ResourcePlanningFixture {
    pub(crate) fn empty(ticks: std::ops::RangeInclusive<Tick>, cadence: Tick) -> Self {
        Self {
            current_scrap: 0,
            observed_at: *ticks.start(),
            horizon: *ticks.end(),
            cadence,
            forecast_income: Vec::new(),
            units: Vec::new(),
            builders: Vec::new(),
            producers: Vec::new(),
        }
    }
}

/// A domain fixture that admits the snapshot's exact producer/kind pairs.
/// Queue, egress, deadline and capital checks still run in the real scheduler.
pub(crate) fn all_producers(resources: &ResourceSnapshot) -> ProductionAccess {
    let allowed = resources
        .producers()
        .iter()
        .flat_map(|lane| lane.trainable.iter().map(|&kind| (lane.producer, kind)))
        .collect();
    let paid = resources
        .producers()
        .iter()
        .flat_map(|lane| lane.queued.iter().map(|&kind| (lane.producer, kind)))
        .collect();
    ProductionAccess::restricted_kinds_with_paid(allowed, paid)
}

pub(crate) fn count_all_paid_ready(
    resources: &ResourceSnapshot,
    kind: UnitKind,
    deadline: Tick,
) -> usize {
    count_paid_queued_ready_with_access(resources, kind, deadline, &all_producers(resources))
}
