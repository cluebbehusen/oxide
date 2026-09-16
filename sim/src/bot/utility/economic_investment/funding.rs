//! Retained payment timing shared by economic prices within one observation.
use super::*;

pub(in crate::bot::utility) struct FundingCalendar<'a> {
    resources: &'a ResourceSnapshot,
    now: u64,
    bank: u64,
    payments: Vec<(u64, u32)>,
}
impl<'a> FundingCalendar<'a> {
    pub(in crate::bot::utility) fn new(context: &EconomicInvestmentContext<'a>) -> Self {
        let now = context.obs.tick;
        let mut protected = 0u32;
        let mut payments = Vec::new();
        for obligation in context.obligations {
            let claims = &obligation.claims;
            protected = protected.saturating_add(claims.current_scrap());
            payments.extend(
                claims
                    .forecast_scrap()
                    .iter()
                    .chain(claims.foregone_income())
                    .map(|claim| (claim.through, claim.amount)),
            );
            if let Some(claim) = claims.deferrable_capital() {
                payments.push((claim.through, claim.amount));
            }
            for job in claims.producer_jobs() {
                let through = job
                    .fixed_timing()
                    .map_or(job.enqueue_not_before(), |(_, enqueued, _, _)| enqueued);
                if through <= now {
                    protected = protected.saturating_add(job.kind().stats().cost);
                } else {
                    payments.push((through, job.kind().stats().cost));
                }
            }
        }
        payments.sort_unstable();
        let bank = u64::from(
            context
                .obs
                .scrap
                .saturating_sub(context.protected_scrap.max(protected)),
        );
        Self {
            resources: context.resources,
            now,
            bank,
            payments,
        }
    }
    pub(in crate::bot::utility) fn delay(&self, cost: u32, deadline: u64) -> u64 {
        let available = |through: u64| {
            self.bank.saturating_add(u64::from(
                self.resources
                    .forecast()
                    .income_through(through.saturating_sub(1))
                    .amount(),
            ))
        };
        // The purchase must also leave every later retained payment fundable.
        // Exact lanes, forecast-only claims, and all other resources are still adjudicated together.
        let feasible = |through| {
            let mut required = u64::from(cost);
            for &(payment_at, amount) in &self.payments {
                required = required.saturating_add(u64::from(amount));
                if payment_at >= through && required > available(payment_at) {
                    return false;
                }
            }
            let due = self
                .payments
                .iter()
                .take_while(|(at, _)| *at <= through)
                .map(|(_, amount)| u64::from(*amount))
                .fold(u64::from(cost), u64::saturating_add);
            due <= available(through)
        };
        let now = self.now;
        let mut low = now;
        let mut high = deadline;
        while low < high {
            let mid = low + (high - low) / 2;
            if feasible(mid) {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        low.saturating_sub(now)
    }
}
