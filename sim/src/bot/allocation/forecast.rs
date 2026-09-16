//! Production preflights share the allocator's retained refinement service.

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

pub(in crate::bot) fn refine_obligation(
    capacity: &AllocationCapacity,
    prior: &[ImportedObligation],
    candidate: ImportedObligation,
    planning: &PlanningWork,
) -> Progress<ImportedObligation> {
    match refine_obligations(
        capacity,
        &prior
            .iter()
            .cloned()
            .chain(std::iter::once(candidate.clone()))
            .collect::<Vec<_>>(),
        planning,
    ) {
        Progress::Ready(()) => Progress::Ready(candidate),
        Progress::Deferred => Progress::Deferred,
        Progress::ProvenInfeasible => Progress::ProvenInfeasible,
    }
}

pub(in crate::bot) fn refine_obligations(
    capacity: &AllocationCapacity,
    obligations: &[ImportedObligation],
    planning: &PlanningWork,
) -> Progress<()> {
    let mut obligations: Vec<_> = obligations.iter().collect();
    obligations.sort_by_key(|obligation| obligation.owner());
    let mut claims = ClaimState::default();
    for obligation in obligations {
        let owner = obligation.owner();
        if claims
            .stage(
                capacity,
                owner,
                &obligation.claims,
                FundingPriority::obligation(owner),
            )
            .is_err()
        {
            return Progress::ProvenInfeasible;
        }
    }
    match claims.refine(capacity, &mut |capacity, claims| {
        planning.production(capacity.resources.observed_at(), capacity, claims)
    }) {
        Ok(Progress::Ready(_)) => Progress::Ready(()),
        Ok(Progress::Deferred) => Progress::Deferred,
        Ok(Progress::ProvenInfeasible) | Err(_) => Progress::ProvenInfeasible,
    }
}
