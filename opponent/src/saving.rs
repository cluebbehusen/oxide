//! One optional saving target: the investment the seat is putting scrap aside
//! for, and how much of its bank is protected from ordinary spending.

use crate::investments::{self, ADOPT, Candidate, Investment, Step};
use crate::memory::Memory;
use chassis::grid::TilePos;
use oxide_sim::observation::ObservationData;
use serde::{Deserialize, Serialize};

/// The seat's saving state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Saving {
    /// Scrap held back from ordinary spending, never above the bank.
    protected: u32,
    target: Option<Target>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    investment: Investment,
    /// A purchase the next decision must find in the world.
    attempt: Option<Attempt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    step: Step,
    anchor: TilePos,
    at: u64,
}

impl Saving {
    /// Scrap held back from ordinary spending.
    pub(crate) fn protected(&self) -> u32 {
        self.protected
    }

    /// The investment being saved for.
    pub(crate) fn investment(&self) -> Option<Investment> {
        self.target.map(|target| target.investment)
    }

    /// Checks the previous purchase, keeps or replaces the target, and grows
    /// the protected share by `share` per mille of `earned`.
    ///
    /// A purchase missing from the world was rejected, cancelled or refunded:
    /// the target stays, its footprint is remembered as failed, and the
    /// protected amount is recomputed from the bank.
    pub(crate) fn settle(
        &mut self,
        observation: &ObservationData,
        candidates: &[Candidate],
        share: u32,
        earned: u32,
        memory: &mut Memory,
    ) {
        let bank = observation.scrap;
        let mut refund = false;
        if let Some(target) = &mut self.target
            && let Some(attempt) = target.attempt.take()
        {
            if confirmed(observation, attempt) {
                if investments::completes(target.investment, attempt.step) {
                    self.target = None;
                }
            } else {
                if let Step::Build(kind) = attempt.step {
                    memory.fail(kind, attempt.anchor, observation.tick);
                }
                refund = true;
            }
        }
        let current = self.target.and_then(|target| {
            candidates
                .iter()
                .find(|candidate| candidate.investment == target.investment)
        });
        let best = candidates.first().filter(|best| best.score >= ADOPT);
        let switch = match (current, best) {
            (None, None) => {
                self.target = None;
                self.protected = 0;
                return;
            }
            (_, None) => false,
            (None, Some(_)) => true,
            (Some(current), Some(best)) => {
                best.score >= current.score + (current.score / 4).max(150)
            }
        };
        if switch && let Some(best) = best {
            self.target = Some(Target {
                investment: best.investment,
                attempt: None,
            });
        }
        let Some((_, price)) = self
            .target
            .and_then(|target| investments::step(observation, target.investment))
        else {
            self.protected = self.protected.min(bank);
            return;
        };
        let growth = if switch && current.is_none() {
            share * bank / 1_000
        } else {
            share * earned / 1_000
        };
        self.protected = if refund {
            price.min(bank)
        } else {
            (self.protected + growth).min(price).min(bank)
        };
    }

    /// Records a purchase toward the target for the next decision to check.
    /// The purchase spent what was protected for it.
    pub(crate) fn attempted(&mut self, step: Step, anchor: TilePos, at: u64) {
        self.protected = 0;
        if let Some(target) = &mut self.target {
            target.attempt = Some(Attempt { step, anchor, at });
        }
    }

    /// Caps the protected amount by what the decision left uncommitted.
    pub(crate) fn keep_at_most(&mut self, available: u32) {
        self.protected = self.protected.min(available);
    }

    /// Rejects a restored target that could not have been recorded by `now`.
    pub(crate) fn validate(&self, now: u64) -> Result<(), String> {
        let impossible = self
            .target
            .and_then(|target| target.attempt)
            .is_some_and(|attempt| attempt.at > now);
        if impossible {
            return Err("checkpoint saving target is from the future".into());
        }
        Ok(())
    }
}

/// Whether a purchase shows up in the world: the building or its site at the
/// anchor, a worker's claim on it, or the upgraded building.
fn confirmed(observation: &ObservationData, attempt: Attempt) -> bool {
    match attempt.step {
        Step::Build(kind) => {
            observation
                .my_buildings
                .iter()
                .any(|building| building.kind == kind && building.anchor == attempt.anchor)
                || observation
                    .my_units
                    .iter()
                    .any(|unit| unit.founding == Some((kind, attempt.anchor)))
        }
        Step::Upgrade(id) => observation
            .my_buildings
            .iter()
            .any(|building| building.id == id && building.tier > 0),
    }
}
