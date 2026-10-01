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
            match outcome(observation, target.investment, attempt) {
                Outcome::Placed => {
                    if investments::completes(target.investment, attempt.step) {
                        self.target = None;
                    }
                }
                Outcome::Pending => target.attempt = Some(attempt),
                Outcome::Missing => {
                    if let Step::Build(kind) = attempt.step {
                        memory.fail(kind, attempt.anchor, observation.tick);
                    }
                    refund = true;
                }
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
            self.protected.saturating_add(growth).min(price).min(bank)
        };
    }

    /// Whether a purchase toward the target still awaits its site.
    pub(crate) fn pending(&self) -> bool {
        self.target.is_some_and(|target| target.attempt.is_some())
    }

    /// Records a purchase toward the target for the next decision to check.
    /// The purchase spent what was protected for it.
    pub(crate) fn attempted(&mut self, step: Step, anchor: TilePos) {
        self.protected = 0;
        if let Some(target) = &mut self.target {
            target.attempt = Some(Attempt { step, anchor });
        }
    }

    /// Rejects a restored target placed off a map of the given size.
    pub(crate) fn validate(&self, width: i32, height: i32) -> Result<(), String> {
        let on_map = |tile: TilePos| (0..width).contains(&tile.x) && (0..height).contains(&tile.y);
        let off_map = self.target.is_some_and(|target| {
            matches!(
                target.investment,
                Investment::Extractor(anchor) | Investment::Defense { anchor, .. } if !on_map(anchor)
            )
                || target
                    .attempt
                    .is_some_and(|attempt| !on_map(attempt.anchor))
        });
        if off_map {
            return Err("checkpoint saving target is off the map".into());
        }
        Ok(())
    }

    /// Caps the protected amount by what the decision left uncommitted.
    pub(crate) fn keep_at_most(&mut self, available: u32) {
        self.protected = self.protected.min(available);
    }
}

/// What became of a purchase.
enum Outcome {
    /// The building, its physical site, or the upgrade stands, or the unit
    /// is queued.
    Placed,
    /// A provisional scaffold or a worker's claim still waits for its
    /// ground to be confirmed, which can still refund it.
    Pending,
    /// Nothing stands: it was rejected, cancelled or refunded.
    Missing,
}

fn outcome(observation: &ObservationData, investment: Investment, attempt: Attempt) -> Outcome {
    match attempt.step {
        Step::Build(kind) => {
            let site = observation
                .my_buildings
                .iter()
                .find(|building| building.kind == kind && building.anchor == attempt.anchor);
            let claimed = observation
                .my_units
                .iter()
                .any(|unit| unit.founding == Some((kind, attempt.anchor)));
            match site {
                Some(site) if !site.provisional => Outcome::Placed,
                Some(_) => Outcome::Pending,
                None if claimed => Outcome::Pending,
                None => Outcome::Missing,
            }
        }
        Step::Train(kind) => {
            let queued = observation
                .my_buildings
                .iter()
                .zip(&observation.my_queues)
                .any(|(building, queue)| {
                    building.anchor == attempt.anchor && queue.contains(&kind)
                });
            if queued {
                Outcome::Placed
            } else {
                Outcome::Missing
            }
        }
        Step::Upgrade(id) => {
            // The upgrade raises the tier as soon as it is accepted.
            let tier = match investment {
                Investment::Upgrade { building, tier } if building == id => tier,
                _ => 1,
            };
            let upgraded = observation
                .my_buildings
                .iter()
                .any(|building| building.id == id && building.tier >= tier);
            if upgraded {
                Outcome::Placed
            } else {
                Outcome::Missing
            }
        }
    }
}
