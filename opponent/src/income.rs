//! Income is not observed. The seat estimates it from the change in its bank
//! since the previous decision plus what that decision spent.

use oxide_sim::TICKS_PER_SECOND;
use serde::{Deserialize, Serialize};

const TICKS_PER_MINUTE: u64 = 60 * TICKS_PER_SECOND as u64;

/// The seat's smoothed income and the decision it is measured from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Income {
    previous: Option<Previous>,
    per_minute: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Previous {
    tick: u64,
    bank: u32,
    spent: u32,
}

impl Income {
    /// Folds in the bank at `tick` and returns the scrap earned since the
    /// previous decision. A rejected command may never have charged what the
    /// seat counted as spent, so a decision after one skips its sample.
    pub(crate) fn observe(&mut self, tick: u64, bank: u32, rejected: bool) -> u32 {
        let Some(previous) = self.previous.filter(|previous| previous.tick < tick) else {
            return 0;
        };
        if rejected {
            return 0;
        }
        let earned = (bank + previous.spent).saturating_sub(previous.bank);
        let rate = u64::from(earned) * TICKS_PER_MINUTE / (tick - previous.tick);
        let rate = u32::try_from(rate).unwrap_or(u32::MAX);
        self.per_minute = ((3 * u64::from(self.per_minute) + u64::from(rate)) / 4) as u32;
        earned
    }

    /// Records the decision the next sample is measured from.
    pub(crate) fn record(&mut self, tick: u64, bank: u32, spent: u32) {
        self.previous = Some(Previous { tick, bank, spent });
    }

    /// Smoothed scrap per minute.
    pub(crate) fn per_minute(&self) -> u32 {
        self.per_minute
    }

    /// Rejects a restored estimate measured after `now`.
    pub(crate) fn validate(&self, now: u64) -> Result<(), String> {
        match self.previous {
            Some(previous) if previous.tick > now || previous.spent > previous.bank => {
                Err("checkpoint income sample is impossible".into())
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn income_counts_spending_and_skips_samples_after_a_rejection() {
        let mut income = Income::default();
        assert_eq!(income.observe(0, 150, false), 0, "no previous decision");
        income.record(0, 150, 100);
        assert_eq!(income.observe(12, 80, false), 30);
        assert_eq!(income.per_minute(), 750);
        income.record(12, 80, 0);
        assert_eq!(income.observe(24, 200, true), 0);
        assert_eq!(income.per_minute(), 750, "unchanged");
        assert_eq!(income.validate(24), Ok(()));
        assert!(income.validate(11).is_err());
    }
}
