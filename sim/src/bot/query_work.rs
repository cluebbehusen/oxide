//! Opt-in work counters. Attribution never influences planning results.

use super::observer::PhaseObserver;
use std::cell::{Cell, RefCell};

/// Required caller identity at a navigation or planning-query boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(usize)]
pub enum QueryPurpose {
    /// Find a worker's safe evacuation destination.
    EvacuationDestination,
    /// Price fresh economic construction and upgrades.
    EconomicInvestment,
    /// Estimate harvest service and worker returns.
    HarvestValuation,
    /// Assess Extractors and their shared support.
    ExtractorCluster,
    /// Price new Foundry logistics.
    FoundryLogistics,
    /// Check the military safety of an expansion.
    FoundrySecurity,
    /// Assess weapon coverage for a defensive site.
    DefenseCoverage,
    /// Compare routes around a hypothetical Barricade.
    BarricadeDetour,
    /// Assess information coverage and Array placement.
    ArrayPlacement,
    /// Route an army's current mission.
    ArmyMovement,
    /// Route an observer toward a reconnaissance question.
    ReconApproach,
    /// Prepare representative hostile approaches to defended assets.
    DefenseApproaches,
    /// Validate defensive construction candidates.
    DefenseSitePlacement,
    /// Check ground access around construction sites.
    ConstructionAccess,
    /// Preserve producer exits and resource access.
    ConstructionExitSafety,
    /// Select and validate a builder route.
    BuilderRouting,
    /// Assign workers to harvest destinations.
    HarvestDispatch,
    /// Rank reachable ground combat objectives.
    GroundTargetSelection,
    /// Assess the available ground force.
    ForceReadiness,
    /// Maintain and execute an air operation.
    AirOperation,
    /// Maintain and execute a transport operation.
    LiftOperation,
    /// Maintain and execute a raid.
    RaidOperation,
    /// Route repair and protective support.
    SupportRouting,
    /// Validate and lower committed unit orders.
    ExecutiveOrders,
    /// Construct derived player knowledge.
    ObservationProjection,
    /// Direct service regression tests.
    #[cfg(test)]
    NavigationTest,
}

const PURPOSES: &[QueryPurpose] = &[
    QueryPurpose::EvacuationDestination,
    QueryPurpose::EconomicInvestment,
    QueryPurpose::HarvestValuation,
    QueryPurpose::ExtractorCluster,
    QueryPurpose::FoundryLogistics,
    QueryPurpose::FoundrySecurity,
    QueryPurpose::DefenseCoverage,
    QueryPurpose::BarricadeDetour,
    QueryPurpose::ArrayPlacement,
    QueryPurpose::ArmyMovement,
    QueryPurpose::ReconApproach,
    QueryPurpose::DefenseApproaches,
    QueryPurpose::DefenseSitePlacement,
    QueryPurpose::ConstructionAccess,
    QueryPurpose::ConstructionExitSafety,
    QueryPurpose::BuilderRouting,
    QueryPurpose::HarvestDispatch,
    QueryPurpose::GroundTargetSelection,
    QueryPurpose::ForceReadiness,
    QueryPurpose::AirOperation,
    QueryPurpose::LiftOperation,
    QueryPurpose::RaidOperation,
    QueryPurpose::SupportRouting,
    QueryPurpose::ExecutiveOrders,
    QueryPurpose::ObservationProjection,
    #[cfg(test)]
    QueryPurpose::NavigationTest,
];

/// Service operation. Work units are specific to the operation, not time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
#[repr(usize)]
pub enum QueryOperation {
    /// Request a route cost between endpoint sets; work counts requests.
    CostRequest,
    /// Search endpoint-set costs; work counts expanded nodes.
    CostSearch,
    /// Prepare passability; work counts grid cells.
    PrepareSurface,
    /// Request a retained canonical path; work counts requests.
    PathRequest,
    /// Execute A*; work counts expanded nodes.
    PathSearch,
    /// Initialize a distance field; work counts grid cells.
    FieldSetup,
    /// Advance a distance field; work counts queue entries processed.
    FieldAdvance,
    /// Label connectivity; work counts grid cells.
    Components,
    /// Reuse a path, field, or component surface; work counts hits.
    CacheHit,
    /// Query sparse observation passability; work counts checks.
    SparsePassability,
    /// Scan evacuation perimeters; work counts candidate tiles.
    EvacuationCandidates,
    /// Prepare shared weapon coverage facts; work counts distinct approach tiles.
    CoverageSetup,
    /// Score a defensive site; work counts approach samples evaluated.
    CoverageScore,
}
const OPERATIONS: [QueryOperation; 13] = [
    QueryOperation::CostRequest,
    QueryOperation::CostSearch,
    QueryOperation::PrepareSurface,
    QueryOperation::PathRequest,
    QueryOperation::PathSearch,
    QueryOperation::FieldSetup,
    QueryOperation::FieldAdvance,
    QueryOperation::Components,
    QueryOperation::CacheHit,
    QueryOperation::SparsePassability,
    QueryOperation::EvacuationCandidates,
    QueryOperation::CoverageSetup,
    QueryOperation::CoverageScore,
];

/// One nonempty purpose/operation pair from an observed decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct QueryWork {
    /// Caller that requested this work.
    pub purpose: QueryPurpose,
    /// Operation measured at the shared service boundary.
    pub operation: QueryOperation,
    /// Calls, including separate continuation slices.
    pub requests: u64,
    /// Operation-specific work, as documented by `QueryOperation`.
    pub work: u64,
}

struct State {
    counts: [[(u64, u64); OPERATIONS.len()]; PURPOSES.len()],
}
impl Default for State {
    fn default() -> Self {
        Self {
            counts: [[(0, 0); OPERATIONS.len()]; PURPOSES.len()],
        }
    }
}
thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static STATE: RefCell<State> = RefCell::default();
}

pub(super) fn record(purpose: QueryPurpose, operation: QueryOperation, work: usize) {
    if ENABLED.get() {
        STATE.with_borrow_mut(|state| {
            let counts = &mut state.counts[purpose as usize][operation as usize];
            counts.0 = counts.0.saturating_add(1);
            counts.1 = counts.1.saturating_add(work as u64);
        });
    }
}

pub(super) struct Capture<'a> {
    previous: Option<State>,
    was_enabled: bool,
    observer: Option<&'a dyn PhaseObserver>,
    thread: std::marker::PhantomData<*const ()>,
}
impl<'a> Capture<'a> {
    pub(super) fn new(observer: Option<&'a dyn PhaseObserver>) -> Self {
        let observer = observer.filter(|observer| observer.collect_query_work());
        let was_enabled = ENABLED.get();
        let previous = observer.map(|_| STATE.with_borrow_mut(std::mem::take));
        ENABLED.set(observer.is_some());
        Self {
            previous,
            was_enabled,
            observer,
            thread: std::marker::PhantomData,
        }
    }
}
impl Drop for Capture<'_> {
    fn drop(&mut self) {
        ENABLED.set(self.was_enabled);
        if let Some(previous) = self.previous.take() {
            let completed = STATE.with_borrow_mut(|state| std::mem::replace(state, previous));
            let mut rows = Vec::new();
            for &purpose in PURPOSES {
                for operation in OPERATIONS {
                    let (requests, work) = completed.counts[purpose as usize][operation as usize];
                    if requests != 0 {
                        rows.push(QueryWork {
                            purpose,
                            operation,
                            requests,
                            work,
                        });
                    }
                }
            }
            self.observer.unwrap().query_work(&rows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::observer::BotPhase;

    #[derive(Default)]
    struct Observer(RefCell<Vec<QueryWork>>);
    impl PhaseObserver for Observer {
        fn enter(&self, _: BotPhase) {}
        fn exit(&self, _: BotPhase) {}
        fn collect_query_work(&self) -> bool {
            true
        }
        fn query_work(&self, rows: &[QueryWork]) {
            self.0.borrow_mut().extend_from_slice(rows);
        }
    }

    #[test]
    fn shared_cache_hits_belong_to_the_requesting_caller() {
        use crate::bot::navigation::{
            KnownGrid,
            paths::{CacheClass, PathBoard, PathQueries},
            search::Search,
        };
        use chassis::grid::TilePos;

        let blocked = vec![false; 64];
        let cache = RefCell::new(PathQueries::default());
        let first = PathBoard {
            query_purpose: QueryPurpose::FoundryLogistics,
            grid: KnownGrid::new(8, 8, &blocked).unwrap(),
            class: CacheClass::Ground,
            cache: &cache,
        };
        let second = PathBoard {
            query_purpose: QueryPurpose::DefenseApproaches,
            ..first
        };
        let observer = Observer::default();
        {
            let _capture = Capture::new(Some(&observer));
            let mut search = Search::default();
            let start = TilePos::new(0, 0);
            let goal = TilePos::new(5, 5);
            let path = first.path(start, goal, None, &mut search).unwrap();
            assert_eq!(second.path(start, goal, None, &mut search), Some(path));
        }
        let rows = observer.0.borrow();
        assert!(
            rows.iter()
                .any(|row| row.purpose == QueryPurpose::FoundryLogistics
                    && row.operation == QueryOperation::PathSearch
                    && row.work > 0)
        );
        assert!(
            rows.iter()
                .any(|row| row.purpose == QueryPurpose::DefenseApproaches
                    && row.operation == QueryOperation::CacheHit
                    && row.requests == 1)
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.purpose == QueryPurpose::DefenseApproaches
                    && row.operation == QueryOperation::PathSearch)
        );
    }

    #[test]
    fn cloned_deferred_jobs_preserve_their_caller_when_resumed() {
        use crate::bot::{
            PublicMapBriefing,
            navigation::{KnownGrid, public_fields::BlockedGroundLayout},
            planning::{PlanningWork, Progress},
        };
        use chassis::grid::TilePos;

        let map = PublicMapBriefing::from_scenario(&crate::Scenario::skirmish()).unwrap();
        let blocked = BlockedGroundLayout::from_predicate(&map, |_| false);
        let surface = vec![false; 64];
        let grid = KnownGrid::new(8, 8, &surface).unwrap();
        for approach in [false, true] {
            let work = PlanningWork::with_allowance(if approach {
                64
            } else {
                (map.map_width * map.map_height) as usize
            });
            let purpose = if approach {
                QueryPurpose::DefenseApproaches
            } else {
                QueryPurpose::FoundryLogistics
            };
            let pending = if approach {
                matches!(
                    work.approach_field(purpose, 0, grid, false, &[TilePos::new(1, 1)]),
                    Progress::Deferred
                )
            } else {
                matches!(
                    work.field(purpose, 0, &map, &blocked, [TilePos::new(1, 1)]),
                    Progress::Deferred
                )
            };
            assert!(pending);
            let resumed = work.clone();
            let observer = Observer::default();
            {
                let _capture = Capture::new(Some(&observer));
                for tick in (12..120).step_by(12) {
                    resumed.begin(tick);
                    if resumed.stats().pending_fields + resumed.stats().pending_approach_fields == 0
                    {
                        break;
                    }
                }
            }
            assert_eq!(
                resumed.stats().pending_fields + resumed.stats().pending_approach_fields,
                0
            );
            let rows = observer.0.borrow();
            assert!(
                rows.iter()
                    .any(|row| row.operation == QueryOperation::FieldAdvance && row.work > 0)
            );
            assert!(rows.iter().all(|row| row.purpose == purpose), "{rows:?}");
        }
    }

    #[test]
    fn explicit_purposes_and_disabled_nested_capture_do_not_leak() {
        let observer = Observer::default();
        {
            let _capture = Capture::new(Some(&observer));
            record(
                QueryPurpose::EconomicInvestment,
                QueryOperation::PathSearch,
                7,
            );
            let _ = std::panic::catch_unwind(|| {
                record(
                    QueryPurpose::FoundrySecurity,
                    QueryOperation::PathSearch,
                    11,
                );
                panic!("exercise scope unwinding");
            });
            {
                let _disabled = Capture::new(None);
                record(
                    QueryPurpose::EconomicInvestment,
                    QueryOperation::PathSearch,
                    100,
                );
            }
            record(
                QueryPurpose::EconomicInvestment,
                QueryOperation::PathSearch,
                3,
            );
        }
        let rows = observer.0.borrow();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].purpose, rows[0].requests, rows[0].work),
            (QueryPurpose::EconomicInvestment, 2, 10)
        );
        assert_eq!(
            (rows[1].purpose, rows[1].requests, rows[1].work),
            (QueryPurpose::FoundrySecurity, 1, 11)
        );
        assert!(!ENABLED.get());
    }

    #[test]
    fn threads_have_independent_attribution() {
        let handles = [7, 13].map(|work| {
            std::thread::spawn(move || {
                let observer = Observer::default();
                {
                    let _capture = Capture::new(Some(&observer));
                    record(
                        QueryPurpose::NavigationTest,
                        QueryOperation::PathSearch,
                        work,
                    );
                }
                observer.0.into_inner()
            })
        });
        for (handle, expected) in handles.into_iter().zip([7, 13]) {
            assert_eq!(handle.join().unwrap()[0].work, expected);
        }
    }

    #[test]
    fn observing_queries_does_not_change_controller_commands() {
        use crate::{
            Scenario,
            bot::seat_bots,
            scenario::{BotConfig, BotDifficulty, BotStance},
        };
        let mut scenario = Scenario::skirmish();
        for (seat, player) in scenario.players.iter_mut().enumerate() {
            player.bot = true;
            player.bot_config = Some(BotConfig::scripted(
                BotDifficulty::Prime,
                BotStance::Balanced,
                seat as u64 + 7,
            ));
        }
        let mut state = scenario.build().unwrap();
        let mut ordinary = seat_bots(&scenario).unwrap();
        let mut observed = ordinary.clone();
        let observer = Observer::default();
        for _ in 0..240 {
            let expected = ordinary
                .iter_mut()
                .flat_map(|bot| bot.act(&state))
                .collect::<Vec<_>>();
            let actual = observed
                .iter_mut()
                .flat_map(|bot| bot.act_observed(&state, &observer))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected);
            state.tick(&actual);
        }
        assert!(!observer.0.borrow().is_empty());
    }
}
