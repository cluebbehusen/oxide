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
        let earned = u32::try_from(
            (u64::from(bank) + u64::from(previous.spent)).saturating_sub(u64::from(previous.bank)),
        )
        .unwrap_or(u32::MAX);
        let rate = u64::from(earned) * TICKS_PER_MINUTE / (tick - previous.tick);
        let rate = u32::try_from(rate).unwrap_or(u32::MAX);
        self.per_minute = u32::try_from((3 * u64::from(self.per_minute) + u64::from(rate)) / 4)
            .expect("an average of u32 rates fits in u32");
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
mod tests;
