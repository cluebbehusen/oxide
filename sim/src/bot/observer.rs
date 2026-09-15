//! Optional phase boundaries. Timing and persistence belong to the caller.

/// Coarse controller operations exposed solely for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BotPhase {
    /// Construct the player's observation.
    Observation = 1,
    /// Reconcile existing units, memories, and emergency work.
    Maintenance,
    /// Strategic preparation and residual policy work.
    Strategy,
    /// Cross-domain prepare, resolve, and commit.
    Allocation,
    /// Ground and rank optional defensive construction.
    Defense,
    /// Ground and rank optional economic investment.
    Economy,
    /// Lower funded intentions to ordinary commands.
    Executive,
    /// Rank and price fresh resource expansion.
    Foundry = 21,
    /// Derive ordinary force capabilities and production alternatives.
    StandingForce,
    /// Resolve shared claims and compatible proposal portfolios.
    Portfolio,
    /// Validate combinations of proposed construction footprints.
    Layouts,
    /// Reconcile observation assignments and their paid occurrences.
    Reconnaissance,
    /// Observe repair and protection demand across owned assets.
    Support,
    /// Preserve controller state for atomic allocation rollback.
    Snapshot,
}

/// Deterministic work consumed by the incremental planning services this decision.
/// This does not include synchronous planner or mandatory-validation work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct PlanningWorkStats {
    /// Total allowance for field work and opportunity refinement.
    pub allowance: usize,
    /// Work consumed from that allowance.
    pub spent: usize,
    /// New exact site evaluations; retained-site validation is separate.
    pub new_site_checks: usize,
    /// Field jobs whose distances are not yet complete.
    pub pending_fields: usize,
    /// Completed and unfinished fields retained by this controller.
    pub retained_fields: usize,
}

/// Observational notifications. Implementations must not block or affect game inputs.
pub trait PhaseObserver {
    /// Enter a nested operation.
    fn enter(&self, phase: BotPhase);
    /// Leave that operation, including unwinding.
    fn exit(&self, phase: BotPhase);
    /// Publish completed-decision work counters without influencing scheduling.
    fn planning_work(&self, _work: PlanningWorkStats) {}
}

pub(crate) struct PhaseScope<'a> {
    observer: Option<&'a dyn PhaseObserver>,
    phase: BotPhase,
}
impl<'a> PhaseScope<'a> {
    pub(crate) fn new(observer: Option<&'a dyn PhaseObserver>, phase: BotPhase) -> Self {
        if let Some(observer) = observer {
            observer.enter(phase);
        }
        Self { observer, phase }
    }
}
impl Drop for PhaseScope<'_> {
    fn drop(&mut self) {
        if let Some(observer) = self.observer {
            observer.exit(self.phase);
        }
    }
}
