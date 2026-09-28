//! What the seat remembers between decisions: building footprints it failed
//! to claim, so it tries somewhere else for a while.

use chassis::grid::TilePos;
use oxide_sim::BuildingKind;
use serde::{Deserialize, Serialize};

/// Ticks a failed footprint stays skipped.
const FAILURE_TICKS: u64 = 3_600;

/// Failures remembered at once; the oldest is forgotten first.
const FAILURE_CAP: usize = 16;

/// The seat's memory, oldest failure first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Memory {
    failures: Vec<Failure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Failure {
    kind: BuildingKind,
    anchor: TilePos,
    at: u64,
}

impl Memory {
    /// Remembers that `kind` could not be claimed at `anchor`.
    pub(crate) fn fail(&mut self, kind: BuildingKind, anchor: TilePos, now: u64) {
        self.failures
            .retain(|failure| (failure.kind, failure.anchor) != (kind, anchor));
        if self.failures.len() == FAILURE_CAP {
            self.failures.remove(0);
        }
        self.failures.push(Failure {
            kind,
            anchor,
            at: now,
        });
    }

    /// Whether `kind` recently failed at `anchor`.
    pub(crate) fn failed(&self, kind: BuildingKind, anchor: TilePos, now: u64) -> bool {
        self.failures.iter().any(|failure| {
            (failure.kind, failure.anchor) == (kind, anchor) && now < failure.at + FAILURE_TICKS
        })
    }

    /// Forgets failures old enough to try again.
    pub(crate) fn forget(&mut self, now: u64) {
        self.failures
            .retain(|failure| now < failure.at + FAILURE_TICKS);
    }

    /// Rejects a restored memory that could not have been recorded by `now`.
    pub(crate) fn validate(&self, now: u64) -> Result<(), String> {
        if self.failures.len() > FAILURE_CAP {
            return Err("checkpoint remembers too many failures".into());
        }
        let ordered = self
            .failures
            .windows(2)
            .all(|pair| pair[0].at <= pair[1].at);
        if !ordered || self.failures.iter().any(|failure| failure.at > now) {
            return Err("checkpoint failures are out of order".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_expire_and_the_oldest_leaves_first() {
        let mut memory = Memory::default();
        let anchor = TilePos::new(4, 4);
        memory.fail(BuildingKind::Fabricator, anchor, 100);
        assert!(memory.failed(BuildingKind::Fabricator, anchor, 100 + FAILURE_TICKS - 1));
        assert!(!memory.failed(BuildingKind::Airworks, anchor, 100));
        assert!(!memory.failed(BuildingKind::Fabricator, anchor, 100 + FAILURE_TICKS));
        memory.forget(100 + FAILURE_TICKS);
        assert_eq!(memory, Memory::default());

        for x in 0..=FAILURE_CAP as i32 {
            memory.fail(BuildingKind::Fabricator, TilePos::new(x, 0), 200);
        }
        assert_eq!(memory.failures.len(), FAILURE_CAP);
        assert!(!memory.failed(BuildingKind::Fabricator, TilePos::new(0, 0), 200));
        assert_eq!(memory.validate(200), Ok(()));
        assert!(memory.validate(199).is_err());
    }
}
