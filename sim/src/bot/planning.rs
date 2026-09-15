//! Deterministic work allowances shared by nested planning services.

mod fields;
pub(super) mod sites;

const DECISION_WORK: usize = 128_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct PlanningWork {
    tick: Option<u64>,
    allowance: usize,
    budget: WorkBudget,
    fields: fields::FieldPreparation,
}

impl Default for PlanningWork {
    fn default() -> Self {
        Self {
            tick: None,
            allowance: DECISION_WORK,
            budget: WorkBudget::new(DECISION_WORK),
            fields: fields::FieldPreparation::default(),
        }
    }
}

impl PlanningWork {
    #[cfg(test)]
    pub(in crate::bot) fn with_allowance(allowance: usize) -> Self {
        Self {
            allowance,
            budget: WorkBudget::new(allowance),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(in crate::bot) fn spent(&self) -> usize {
        self.budget.spent()
    }

    pub(in crate::bot) fn begin(&mut self, tick: u64) {
        if self.tick != Some(tick) {
            self.tick = Some(tick);
            self.budget = WorkBudget::new(self.allowance);
        }
    }

    pub(in crate::bot) fn field(
        &mut self,
        tick: u64,
        map: &crate::bot::PublicMapBriefing,
        blocked: &crate::bot::navigation::public_fields::BlockedGroundLayout,
        sources: impl IntoIterator<Item = chassis::grid::TilePos>,
    ) -> Progress<std::sync::Arc<crate::bot::navigation::public_fields::PublicGroundDistances>>
    {
        self.begin(tick);
        self.fields
            .advance(tick, map, blocked, sources, &mut self.budget)
    }
}

/// Refine the strongest estimate plus a rotating remainder on actual requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RankedRotation {
    tick: Option<u64>,
    offset: usize,
    next: usize,
}

impl RankedRotation {
    pub(super) fn indices(&mut self, tick: u64, count: usize, limit: usize) -> Vec<usize> {
        if count <= limit {
            return (0..count).collect();
        }
        if limit == 0 {
            return Vec::new();
        }
        let remaining = count - 1;
        if self.tick != Some(tick) {
            self.tick = Some(tick);
            self.offset = self.next % remaining;
            self.next = (self.offset + limit - 1) % remaining;
        }
        std::iter::once(0)
            .chain((0..limit - 1).map(|index| 1 + (self.offset + index) % remaining))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Progress<T> {
    Ready(T),
    ProvenInfeasible,
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct WorkBudget {
    remaining: usize,
    spent: usize,
}

impl WorkBudget {
    pub(super) const fn new(allowance: usize) -> Self {
        Self {
            remaining: allowance,
            spent: 0,
        }
    }

    pub(super) fn charge(&mut self, amount: usize) -> bool {
        if amount > self.remaining {
            return false;
        }
        self.remaining -= amount;
        self.spent += amount;
        true
    }

    pub(super) const fn spent(&self) -> usize {
        self.spent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranked_refinement_does_not_starve_candidates_when_admissions_skip_ticks() {
        let mut rotation = RankedRotation::default();
        let mut visited = std::collections::BTreeSet::new();
        for tick in [24, 72, 120, 168, 216, 264] {
            let indices = rotation.indices(tick, 7, 2);
            assert_eq!(indices[0], 0);
            assert_eq!(indices.len(), 2);
            assert_eq!(rotation.indices(tick, 7, 2), indices);
            let mut clone = rotation.clone();
            assert_eq!(clone.indices(tick, 7, 2), indices);
            visited.extend(indices);
        }
        assert_eq!(visited, (0..7).collect());
        assert!(rotation.indices(288, 7, 0).is_empty());
        assert_eq!(rotation.indices(288, 1, 2), [0]);
    }

    #[test]
    fn nested_services_cannot_refill_or_overdraw_the_shared_allowance() {
        fn service(budget: &mut WorkBudget) -> bool {
            budget.charge(3)
        }
        let mut budget = WorkBudget::new(5);
        assert!(service(&mut budget));
        assert!(!service(&mut budget));
        assert_eq!(budget.spent(), 3);
        assert!(budget.charge(2));
        assert!(!budget.charge(1));
        assert_eq!(budget.spent(), 5);
    }

    #[test]
    fn zero_and_maximum_allowances_do_not_wrap() {
        let mut empty = WorkBudget::new(0);
        assert!(!empty.charge(1));
        let mut maximum = WorkBudget::new(usize::MAX);
        assert!(maximum.charge(usize::MAX));
        assert!(!maximum.charge(1));
        assert_eq!(maximum.spent(), usize::MAX);
    }
}
