//! Deterministic work allowances shared by nested planning services.

mod approaches;
mod fields;
pub(super) mod sites;

use std::cell::{Cell, RefCell};

const DECISION_WORK: usize = 128_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct PlanningWork {
    tick: Cell<Option<u64>>,
    allowance: usize,
    budget: RefCell<WorkBudget>,
    fields: RefCell<fields::FieldPreparation>,
    approaches: RefCell<[approaches::ApproachPreparation; 2]>,
    sites: RefCell<sites::SiteWork>,
    foundry: RefCell<RankedRotation>,
    site_checks: Cell<usize>,
}

impl Default for PlanningWork {
    fn default() -> Self {
        Self {
            tick: Cell::new(None),
            allowance: DECISION_WORK,
            budget: RefCell::new(WorkBudget::new(DECISION_WORK)),
            fields: RefCell::default(),
            approaches: RefCell::default(),
            sites: RefCell::default(),
            foundry: RefCell::default(),
            site_checks: Cell::new(0),
        }
    }
}

impl PlanningWork {
    #[cfg(test)]
    pub(in crate::bot) fn with_allowance(allowance: usize) -> Self {
        Self {
            allowance,
            budget: RefCell::new(WorkBudget::new(allowance)),
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(in crate::bot) fn spent(&self) -> usize {
        self.budget.borrow().spent()
    }

    pub(in crate::bot) fn begin(&self, tick: u64) {
        if self.tick.get() != Some(tick) {
            self.tick.set(Some(tick));
            *self.budget.borrow_mut() = WorkBudget::new(self.allowance);
            self.site_checks.set(0);
            let fields_pending = self.fields.borrow().counts().0 > 0;
            let approach_pending = self
                .approaches
                .borrow()
                .each_ref()
                .map(|domain| domain.counts().0 > 0);
            let active = usize::from(fields_pending)
                + approach_pending
                    .into_iter()
                    .filter(|pending| *pending)
                    .count();
            if let Some(share) = (self.allowance / 2).checked_div(active) {
                let mut budget = self.budget.borrow_mut();
                if fields_pending {
                    budget.run_slice(share, |slice| {
                        self.fields.borrow_mut().resume_pending(tick, slice)
                    });
                }
                for (domain, pending) in self
                    .approaches
                    .borrow_mut()
                    .iter_mut()
                    .zip(approach_pending)
                {
                    if pending {
                        budget.run_slice(share, |slice| domain.resume_pending(tick, slice));
                    }
                }
            }
        }
    }

    pub(in crate::bot) fn site_incumbent(
        &self,
        tick: u64,
        kind: crate::stats::BuildingKind,
    ) -> Option<chassis::grid::TilePos> {
        self.sites.borrow().retained(tick, kind)
    }

    pub(in crate::bot) fn clear_site(&self, kind: crate::stats::BuildingKind) {
        self.sites.borrow_mut().clear_incumbent(kind);
    }

    pub(in crate::bot) fn foundry_indices(
        &self,
        tick: u64,
        count: usize,
        limit: usize,
    ) -> Vec<usize> {
        self.foundry.borrow_mut().indices(tick, count, limit)
    }

    pub(in crate::bot) fn site<T>(
        &self,
        tick: u64,
        kind: crate::stats::BuildingKind,
        anchors: &[chassis::grid::TilePos],
        evaluate: impl FnMut(chassis::grid::TilePos) -> Option<T>,
        better: impl Fn(&T, &T) -> bool,
    ) -> Progress<T> {
        self.begin(tick);
        self.sites
            .borrow_mut()
            .advance_ranked(tick, kind, anchors, evaluate, better, || {
                if self.budget.borrow_mut().charge(1) {
                    self.site_checks.set(self.site_checks.get() + 1);
                    true
                } else {
                    false
                }
            })
    }

    pub(in crate::bot) fn stats(&self) -> super::observer::PlanningWorkStats {
        let (pending_fields, retained_fields) = self.fields.borrow().counts();
        let (pending_approach_fields, retained_approach_fields) = self
            .approaches
            .borrow()
            .iter()
            .map(approaches::ApproachPreparation::counts)
            .fold((0, 0), |(pending, retained), (p, r)| {
                (pending + p, retained + r)
            });
        super::observer::PlanningWorkStats {
            allowance: self.allowance,
            spent: self.budget.borrow().spent(),
            new_site_checks: self.site_checks.get(),
            pending_fields,
            retained_fields,
            pending_approach_fields,
            retained_approach_fields,
        }
    }

    pub(in crate::bot) fn approach_field(
        &self,
        tick: u64,
        grid: super::navigation::KnownGrid<'_>,
        air: bool,
        goals: &[chassis::grid::TilePos],
    ) -> Progress<std::sync::Arc<super::navigation::approaches::ApproachField>> {
        self.begin(tick);
        self.approaches.borrow_mut()[usize::from(air)].advance(
            tick,
            grid,
            goals,
            &mut self.budget.borrow_mut(),
        )
    }

    pub(in crate::bot) fn field(
        &self,
        tick: u64,
        map: &crate::bot::PublicMapBriefing,
        blocked: &crate::bot::navigation::public_fields::BlockedGroundLayout,
        sources: impl IntoIterator<Item = chassis::grid::TilePos>,
    ) -> Progress<std::sync::Arc<crate::bot::navigation::public_fields::PublicGroundDistances>>
    {
        self.begin(tick);
        self.fields
            .borrow_mut()
            .advance(tick, map, blocked, sources, &mut self.budget.borrow_mut())
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

    pub(super) fn run_slice<T>(&mut self, limit: usize, run: impl FnOnce(&mut Self) -> T) -> T {
        let mut slice = Self::new(self.remaining.min(limit));
        let result = run(&mut slice);
        assert!(self.charge(slice.spent));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_preparation_and_site_refinement_share_one_allowance() {
        use crate::bot::{PublicMapBriefing, navigation::public_fields::BlockedGroundLayout};
        use crate::stats::BuildingKind;
        use chassis::grid::TilePos;

        let map = PublicMapBriefing::from_scenario(&crate::Scenario::skirmish()).unwrap();
        let blocked = BlockedGroundLayout::from_predicate(&map, |_| false);
        let work = PlanningWork::with_allowance(3);
        let anchors = (0..8).map(|x| TilePos::new(x, 0)).collect::<Vec<_>>();
        let mut visited = Vec::new();
        assert_eq!(
            work.site(
                24,
                BuildingKind::Turret,
                &anchors,
                |anchor| {
                    visited.push(anchor);
                    assert_eq!(
                        work.field(24, &map, &blocked, [TilePos::new(1, 1)]),
                        Progress::Deferred
                    );
                    None::<()>
                },
                |_, _| false
            ),
            Progress::Deferred
        );
        assert_eq!(visited, anchors[..1]);
        assert_eq!(work.spent(), 3);
        assert_eq!(
            work.site(
                24,
                BuildingKind::Array,
                &anchors,
                |_| { panic!("a different role cannot refill the exhausted controller allowance") },
                |_: &(), _| false
            ),
            Progress::Deferred
        );
        let clone = work.clone();
        let next =
            |work: &PlanningWork| work.site(36, BuildingKind::Turret, &anchors, Some, |_, _| false);
        assert_eq!(next(&work), Progress::Ready(anchors[1]));
        assert_eq!(next(&clone), Progress::Ready(anchors[1]));
        assert_eq!(work, clone);
    }

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
