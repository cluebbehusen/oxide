//! Current queue, deadline, and funding validation of an explicit schedule.

use super::*;

pub(super) fn validate(
    capacity: &AllocationCapacity,
    claims: &ClaimState,
    rows: &[ScheduledProducerJob],
    elapsed: Tick,
) -> Option<ResolvedClaimState> {
    if rows.len() != claims.producer_jobs.len() {
        return None;
    }
    let mut identities: Vec<_> = rows
        .iter()
        .map(|row| (row.owner, row.request_ordinal))
        .collect();
    identities.sort_unstable();
    if identities.windows(2).any(|pair| pair[0] == pair[1]) {
        return None;
    }
    if rows.iter().any(|row| {
        rows.iter().any(|prior| {
            prior.owner == row.owner
                && prior.request_ordinal < row.request_ordinal
                && prior.enqueued_at > row.enqueued_at
        })
    }) {
        return None;
    }
    let mut producers = capacity.resources.producers().to_vec();
    let mut schedule = Vec::with_capacity(rows.len());
    for row in rows {
        let job = claims
            .producer_jobs
            .iter()
            .find(|job| job.owner == row.owner && job.ordinal == row.request_ordinal)?;
        if job.claim.kind != row.kind {
            return None;
        }
        if job
            .claim
            .access
            .producers()
            .binary_search(&row.producer)
            .is_err()
        {
            return None;
        }
        let lane = producers
            .iter_mut()
            .find(|lane| lane.producer() == row.producer)?;
        let earliest = lane
            .earliest_enqueue_tick(job.claim.kind)?
            .max(job.claim.enqueue_not_before)
            .max(
                schedule
                    .iter()
                    .filter(|prior: &&ScheduledProducerJob| {
                        prior.owner == row.owner && prior.request_ordinal < row.request_ordinal
                    })
                    .map(|prior| prior.enqueued_at)
                    .max()
                    .unwrap_or(0),
            );
        let enqueue = match job.claim.fixed_assignment() {
            Some(fixed) => fixed.enqueued_at,
            None => capacity
                .resources
                .decision_at_or_after(row.enqueued_at.checked_add(elapsed)?.max(earliest))?,
        };
        if enqueue < earliest
            || enqueue > job.claim.enqueue_not_after
            || enqueue > capacity.resources.horizon()
        {
            return None;
        }
        let projected = lane.append(job.claim.kind, enqueue)?;
        if projected.ready_at >= job.claim.ready_before
            || job.claim.fixed_assignment().is_some_and(|fixed| {
                fixed.starts_at != projected.starts_at || fixed.ready_at != projected.ready_at
            })
        {
            return None;
        }
        schedule.push(ScheduledProducerJob {
            enqueued_at: enqueue,
            starts_at: projected.starts_at,
            ready_at: projected.ready_at,
            ready_before: job.claim.ready_before,
            current_scrap: 0,
            forecast_scrap: 0,
            ..*row
        });
    }
    let satisfied = claims.voluntary_scrap_guard_satisfied(capacity, &schedule);
    let minimum_residual_scrap = if satisfied {
        claims.effective_minimum_residual_scrap()
    } else {
        claims
            .minimum_residual_scrap
            .max(claims.voluntary_scrap_guard.amount)
    };
    let basis = JointFundingBasis {
        capacity,
        current_capital: claims.current_scrap,
        minimum_residual_scrap,
        forecast_capital: &claims.forecast_scrap,
        deferrable_capital: &claims.deferrable_capital,
        jobs: &claims.producer_jobs,
    };
    let mut capital_assignments =
        assign_joint_funding(basis, &mut schedule, JointFundingMode::PreferPriority).or_else(
            || {
                assign_joint_funding(
                    basis,
                    &mut schedule,
                    JointFundingMode::PreserveCompatiblePortfolio,
                )
            },
        )?;
    schedule.sort_unstable_by_key(|job| {
        (
            job.enqueued_at,
            job.starts_at,
            job.owner,
            job.request_ordinal,
            job.producer,
        )
    });
    capital_assignments.sort_unstable();
    Some(ResolvedClaimState {
        producer_schedule: schedule,
        capital_assignments,
        voluntary_scrap_guard_satisfied: satisfied,
        search_states: 0,
        memo_hits: 0,
    })
}
