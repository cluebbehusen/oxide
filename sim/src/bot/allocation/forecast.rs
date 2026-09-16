//! Campaign forecasts use the allocator's retained production service.

use super::*;
use crate::bot::planning::{PlanningWork, Progress};

pub(in crate::bot) fn refine(
    capacity: &AllocationCapacity,
    key: ConnectedOffenseKey,
    jobs: Vec<ProducerJobClaim>,
    planning: &PlanningWork,
) -> Progress<()> {
    if jobs.is_empty() {
        return Progress::Ready(());
    }
    let owner = ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(key));
    let claims = ClaimState {
        producer_jobs: jobs
            .into_iter()
            .enumerate()
            .map(|(ordinal, claim)| OwnedProducerJob {
                claim,
                owner,
                ordinal,
                funding_priority: FundingPriority::fresh_proposal(owner, 0),
            })
            .collect(),
        ..ClaimState::default()
    };
    match planning.production(capacity.resources.observed_at(), capacity, &claims) {
        Ok(Progress::Ready(_)) => Progress::Ready(()),
        Ok(Progress::Deferred) => Progress::Deferred,
        Ok(Progress::ProvenInfeasible) | Err(_) => Progress::ProvenInfeasible,
    }
}
