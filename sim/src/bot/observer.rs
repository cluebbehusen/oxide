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
/// Observational notifications. Implementations must not block or affect game inputs.
pub trait PhaseObserver {
    /// Enter a nested operation.
    fn enter(&self, phase: BotPhase);
    /// Leave that operation, including unwinding.
    fn exit(&self, phase: BotPhase);
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
