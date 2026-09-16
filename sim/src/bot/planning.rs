//! Deterministic work allowances shared by nested planning services.

mod alternatives;
mod approaches;
mod fields;
pub(super) mod sites;

use std::cell::{Cell, RefCell};

type CampaignTarget = (
    crate::ids::PlayerId,
    chassis::grid::TilePos,
    Option<crate::ids::BuildingId>,
);
type CampaignAlternatives = std::collections::BTreeMap<
    chassis::grid::TilePos,
    (u64, alternatives::Alternatives<CampaignTarget>),
>;

const DECISION_WORK: usize = 128_000;
const PRODUCTION_RESERVE: usize = DECISION_WORK / 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct PlanningWork {
    tick: Cell<Option<u64>>,
    allowance: usize,
    budget: RefCell<WorkBudget>,
    navigation_spent: Cell<usize>,
    fields: RefCell<fields::FieldPreparation>,
    approaches: RefCell<[approaches::ApproachPreparation; 3]>,
    production: RefCell<crate::bot::allocation::production_work::ProductionWork>,
    sites: RefCell<sites::SiteWork>,
    foundry: RefCell<RankedRotation>,
    campaigns: RefCell<CampaignAlternatives>,
    campaign_sites: RefCell<RankedRotation>,
    campaign_selection: RefCell<(Option<u64>, Vec<chassis::grid::TilePos>)>,
    campaign_checks: Cell<(u64, usize)>,
    site_checks: Cell<usize>,
}

impl Default for PlanningWork {
    fn default() -> Self {
        Self {
            tick: Cell::new(None),
            allowance: DECISION_WORK,
            budget: RefCell::new(WorkBudget::new(DECISION_WORK)),
            navigation_spent: Cell::new(0),
            fields: RefCell::default(),
            approaches: RefCell::default(),
            production: RefCell::default(),
            sites: RefCell::default(),
            foundry: RefCell::default(),
            campaigns: RefCell::default(),
            campaign_sites: RefCell::default(),
            campaign_selection: RefCell::default(),
            campaign_checks: Cell::default(),
            site_checks: Cell::new(0),
        }
    }
}

impl PlanningWork {
    pub(in crate::bot) fn campaign_site_selected(
        &self,
        tick: u64,
        site: chassis::grid::TilePos,
        sites: &[chassis::grid::TilePos],
    ) -> bool {
        let mut selection = self.campaign_selection.borrow_mut();
        if selection.0 != Some(tick) {
            let mut indices = self
                .campaign_sites
                .borrow_mut()
                .indices(tick, sites.len(), 2);
            let campaigns = self.campaigns.borrow();
            if let Some(incumbent) = sites.iter().position(|site| {
                campaigns
                    .get(site)
                    .is_some_and(|(_, targets)| targets.retained(tick).is_some())
            }) && let Some(first) = indices.first_mut()
            {
                *first = incumbent;
            }
            *selection = (
                Some(tick),
                indices.into_iter().map(|index| sites[index]).collect(),
            );
        }
        selection.1.contains(&site)
    }

    pub(in crate::bot) fn campaign_candidate<T>(
        &self,
        tick: u64,
        site: chassis::grid::TilePos,
        targets: &[CampaignTarget],
        mut evaluate: impl FnMut(CampaignTarget) -> Progress<T>,
    ) -> Progress<T> {
        let mut campaigns = self.campaigns.borrow_mut();
        campaigns.retain(|_, (used_at, _)| tick.saturating_sub(*used_at) < 120);
        if !campaigns.contains_key(&site) && campaigns.len() == 16 {
            let victim = *campaigns
                .iter()
                .min_by_key(|(site, (used, _))| (*used, **site))
                .unwrap()
                .0;
            campaigns.remove(&victim);
        }
        let (used_at, alternatives) = campaigns.entry(site).or_default();
        *used_at = tick;
        alternatives.advance(
            tick,
            targets,
            2,
            |target| {
                let (previous, count) = self.campaign_checks.get();
                self.campaign_checks
                    .set((tick, if previous == tick { count + 1 } else { 1 }));
                evaluate(target)
            },
            |_, _| false,
            || true,
        )
    }

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
            self.navigation_spent.set(0);
            let fields_pending = self.fields.borrow().counts().0 > 0;
            let production_pending = self.production.borrow().counts().0 > 0;
            let approach_pending = self
                .approaches
                .borrow()
                .each_ref()
                .map(|domain| domain.counts().0 > 0);
            let active = usize::from(fields_pending)
                + usize::from(production_pending)
                + approach_pending
                    .into_iter()
                    .filter(|pending| *pending)
                    .count();
            if let Some(share) = (self.allowance / 2).checked_div(active) {
                let mut budget = self.budget.borrow_mut();
                if fields_pending {
                    let before = budget.spent();
                    budget.run_slice(share, |slice| {
                        self.fields.borrow_mut().resume_pending(tick, slice)
                    });
                    self.navigation_spent
                        .set(self.navigation_spent.get() + budget.spent() - before);
                }
                if production_pending {
                    budget.run_slice(share, |slice| {
                        self.production.borrow_mut().resume_pending(tick, slice)
                    });
                }
                for (domain, pending) in self
                    .approaches
                    .borrow_mut()
                    .iter_mut()
                    .zip(approach_pending)
                {
                    if pending {
                        let before = budget.spent();
                        budget.run_slice(share, |slice| domain.resume_pending(tick, slice));
                        self.navigation_spent
                            .set(self.navigation_spent.get() + budget.spent() - before);
                    }
                }
            }
        }
    }

    fn navigation_work<T>(&self, run: impl FnOnce(&mut WorkBudget) -> T) -> T {
        let reserve = (self.allowance / 4).min(PRODUCTION_RESERVE);
        let limit = self
            .allowance
            .saturating_sub(reserve)
            .saturating_sub(self.navigation_spent.get());
        let mut budget = self.budget.borrow_mut();
        let before = budget.spent();
        let limit = limit.min(budget.remaining.saturating_sub(reserve));
        let result = budget.run_slice(limit, run);
        self.navigation_spent
            .set(self.navigation_spent.get() + budget.spent() - before);
        result
    }

    pub(in crate::bot) fn production(
        &self,
        tick: u64,
        capacity: &crate::bot::allocation::AllocationCapacity,
        claims: &crate::bot::allocation::ClaimState,
    ) -> Result<
        Progress<crate::bot::allocation::ResolvedClaimState>,
        crate::bot::allocation::AllocationConflict,
    > {
        self.begin(tick);
        self.production
            .borrow_mut()
            .resolve(capacity, claims, &mut self.budget.borrow_mut())
    }

    pub(in crate::bot) fn production_forecast(
        &self,
        tick: u64,
        capacity: &crate::bot::allocation::AllocationCapacity,
        claims: &crate::bot::allocation::ClaimState,
    ) -> Result<
        Progress<crate::bot::allocation::ResolvedClaimState>,
        crate::bot::allocation::AllocationConflict,
    > {
        self.begin(tick);
        let mut budget = self.budget.borrow_mut();
        let reserve = (self.allowance / 4).min(PRODUCTION_RESERVE);
        let limit = budget.remaining.saturating_sub(reserve);
        budget.run_slice(limit, |slice| {
            self.production
                .borrow_mut()
                .resolve(capacity, claims, slice)
        })
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
        mut evaluate: impl FnMut(chassis::grid::TilePos) -> Option<T>,
        better: impl Fn(&T, &T) -> bool,
    ) -> Progress<T> {
        self.site_progress(
            tick,
            kind,
            anchors,
            |anchor| evaluate(anchor).map_or(Progress::ProvenInfeasible, Progress::Ready),
            better,
        )
    }

    pub(in crate::bot) fn site_progress<T>(
        &self,
        tick: u64,
        kind: crate::stats::BuildingKind,
        anchors: &[chassis::grid::TilePos],
        evaluate: impl FnMut(chassis::grid::TilePos) -> Progress<T>,
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
        let (pending_production, retained_production) = self.production.borrow().counts();
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
            pending_production,
            retained_production,
            campaign_target_checks: if self.tick.get() == Some(self.campaign_checks.get().0) {
                self.campaign_checks.get().1
            } else {
                0
            },
            retained_campaign_sites: self.campaigns.borrow().len(),
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
        self.navigation_work(|budget| {
            self.approaches.borrow_mut()[usize::from(air)].advance(tick, grid, goals, None, budget)
        })
    }

    pub(in crate::bot) fn candidate_route_field(
        &self,
        tick: u64,
        grid: super::navigation::KnownGrid<'_>,
        overlay: Option<super::navigation::BlockedRect>,
        goals: &[chassis::grid::TilePos],
    ) -> Progress<std::sync::Arc<super::navigation::approaches::ApproachField>> {
        self.begin(tick);
        self.navigation_work(|budget| {
            self.approaches.borrow_mut()[2].advance(tick, grid, goals, overlay, budget)
        })
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
        self.navigation_work(|budget| {
            self.fields
                .borrow_mut()
                .advance(tick, map, blocked, sources, budget)
        })
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

    pub(super) fn weighted<T>(&mut self, cost: usize, run: impl FnOnce(&mut Self) -> T) -> T {
        assert!(cost > 0);
        let mut units = Self::new(self.remaining / cost);
        let result = run(&mut units);
        assert!(self.charge(units.spent * cost));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn campaign_site_admission_is_shared_across_calls_and_rotates_past_failures() {
        use chassis::grid::TilePos;
        let work = PlanningWork::default();
        let sites: Vec<_> = (0..32).map(|x| TilePos::new(x, 0)).collect();
        let mut visited = std::collections::BTreeSet::new();
        for tick in 0..32 {
            let selected: Vec<_> = sites
                .iter()
                .copied()
                .filter(|site| work.campaign_site_selected(tick, *site, &sites))
                .collect();
            assert_eq!(selected.len(), 2);
            visited.extend(selected);
        }
        assert_eq!(visited.len(), sites.len());
        let selected: Vec<_> = sites
            .iter()
            .copied()
            .filter(|site| work.campaign_site_selected(33, *site, &sites))
            .collect();
        for site in &sites {
            work.campaign_candidate(33, *site, &[(crate::ids::PlayerId(1), *site, None)], |_| {
                Progress::<()>::Deferred
            });
        }
        assert_eq!(
            sites
                .iter()
                .copied()
                .filter(|site| work.campaign_site_selected(33, *site, &sites))
                .collect::<Vec<_>>(),
            selected,
            "changing target incumbents cannot open more site admissions in the same decision"
        );
    }

    #[test]
    fn campaign_diagnostics_reset_and_site_retention_is_bounded() {
        use chassis::grid::TilePos;
        let work = PlanningWork::default();
        work.begin(0);
        let targets = [
            (crate::ids::PlayerId(1), TilePos::new(10, 10), None),
            (crate::ids::PlayerId(1), TilePos::new(12, 10), None),
            (crate::ids::PlayerId(1), TilePos::new(14, 10), None),
        ];
        for x in 0..64 {
            work.campaign_candidate(0, TilePos::new(x, 0), &targets, |_| {
                Progress::<()>::Deferred
            });
        }
        assert_eq!(work.stats().campaign_target_checks, 128);
        assert_eq!(work.stats().retained_campaign_sites, 16);
        work.begin(12);
        assert_eq!(work.stats().campaign_target_checks, 0);
        let clone = work.clone();
        for candidate in [&work, &clone] {
            assert_eq!(
                candidate
                    .campaign_candidate(12, TilePos::new(63, 0), &targets, |_| Progress::Ready(7)),
                Progress::Ready(7)
            );
        }
        assert_eq!(work, clone);
        assert_eq!(work.stats().campaign_target_checks, 1);
        work.campaign_candidate(144, TilePos::new(63, 0), &targets, |_| {
            Progress::<()>::Deferred
        });
        assert_eq!(work.stats().retained_campaign_sites, 1);
    }

    #[test]
    fn large_navigation_requests_leave_work_for_production() {
        use crate::bot::{
            allocation::{AllocationCapacity, ClaimState},
            navigation::KnownGrid,
            observation::Observation,
            resources::ResourceSnapshot,
        };
        use chassis::grid::TilePos;
        let work = PlanningWork::default();
        let ground = vec![false; 256 * 256];
        let grid = KnownGrid::new(256, 256, &ground).unwrap();
        let goals = [TilePos::new(0, 0)];
        for _ in 0..8 {
            assert_eq!(
                work.approach_field(0, grid, false, &goals),
                Progress::Deferred
            );
        }
        assert_eq!(work.spent(), DECISION_WORK - PRODUCTION_RESERVE);
        let capacity = AllocationCapacity::from_snapshot(
            &ResourceSnapshot::from_observation(&Observation::default()),
            120,
            1,
        )
        .unwrap();
        assert!(matches!(
            work.production_forecast(0, &capacity, &ClaimState::default()),
            Ok(Progress::Deferred)
        ));
        assert_eq!(work.spent(), DECISION_WORK - PRODUCTION_RESERVE);
        assert!(matches!(
            work.production(0, &capacity, &ClaimState::default()),
            Ok(Progress::Ready(_))
        ));
        assert!(work.spent() > DECISION_WORK - PRODUCTION_RESERVE);
        let clone = work.clone();
        for controller in [&work, &clone] {
            assert!(matches!(
                controller.approach_field(12, grid, false, &goals),
                Progress::Ready(_)
            ));
        }
        assert_eq!(work, clone);
    }

    #[test]
    fn weighted_work_cannot_spend_fractional_units_or_refill_its_parent() {
        let mut budget = WorkBudget::new(10);
        budget.weighted(3, |units| {
            assert!(units.charge(3));
            assert!(!units.charge(1));
        });
        assert_eq!(budget.spent(), 9);
        budget.weighted(3, |units| assert!(!units.charge(1)));
        assert_eq!(budget.spent(), 9);
        assert!(budget.charge(1));
    }

    #[test]
    fn active_field_preparation_outlives_idle_expiry_with_small_slices() {
        use crate::bot::{
            PublicMapBriefing,
            navigation::{KnownGrid, public_fields::BlockedGroundLayout},
        };
        use chassis::grid::TilePos;
        let map = PublicMapBriefing::from_scenario(&crate::Scenario::skirmish()).unwrap();
        let blocked = BlockedGroundLayout::from_predicate(&map, |_| false);
        let open = vec![false; 64 * 64];
        let grid = KnownGrid::new(64, 64, &open).unwrap();
        for approach in [false, true] {
            let request = |work: &PlanningWork, tick| {
                if approach {
                    matches!(
                        work.approach_field(tick, grid, false, &[TilePos::new(1, 1)]),
                        Progress::Ready(_)
                    )
                } else {
                    matches!(
                        work.field(tick, &map, &blocked, [TilePos::new(1, 1)]),
                        Progress::Ready(_)
                    )
                }
            };
            let work = PlanningWork::with_allowance(64);
            let completed = (0..4_800).step_by(12).find(|tick| request(&work, *tick));
            assert!(
                completed.is_some_and(|tick| tick > 120),
                "{approach}: {completed:?}"
            );
            let abandoned = PlanningWork::with_allowance(64);
            assert!(!request(&abandoned, 0));
            abandoned.begin(120);
            assert_eq!(abandoned.stats().pending_fields, 0);
            assert_eq!(abandoned.stats().pending_approach_fields, 0);
        }
    }

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
