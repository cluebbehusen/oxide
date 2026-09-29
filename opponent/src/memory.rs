//! What the seat remembers between decisions: enemy units it has seen, with
//! confidence that fades until they are seen again; building footprints it
//! failed to claim, so it tries somewhere else for a while; enemy buildings
//! it gave up attacking, so it attacks something else for a while; and when
//! it last saw each of its scouting points. Enemy buildings need no other
//! memory here: the observation keeps their ghosts.

use chassis::grid::TilePos;
use oxide_sim::observation::ObservationData;
use oxide_sim::{BuildingKind, UnitId, UnitKind};
use serde::{Deserialize, Serialize};

/// Ticks a failed footprint stays skipped.
const FAILURE_TICKS: u64 = 3_600;

/// Failures remembered at once; the oldest is forgotten first.
const FAILURE_CAP: usize = 16;

/// Ticks an enemy unit is remembered after it was last seen.
const UNIT_TICKS: u64 = 600;

/// Enemy units remembered at once; the stalest is forgotten first.
const UNIT_CAP: usize = 128;

/// The seat's memory: enemy units by id, failures oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Memory {
    units: Vec<SeenUnit>,
    failures: Vec<Failure>,
    /// Attack targets given up on, oldest first.
    abandoned: Vec<Failure>,
    /// Tick each scouting point was last in sight, by point; empty before
    /// the first decision.
    scouted: Vec<u64>,
}

/// An enemy unit as last seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeenUnit {
    pub(crate) id: UnitId,
    pub(crate) kind: UnitKind,
    pub(crate) tile: TilePos,
    pub(crate) seen: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Failure {
    kind: BuildingKind,
    anchor: TilePos,
    at: u64,
}

impl Memory {
    /// Refreshes every enemy unit in sight, and forgets those gone stale or
    /// missing from where they were last seen.
    pub(crate) fn observe(&mut self, observation: &ObservationData) {
        let now = observation.tick;
        for unit in &observation.enemy_units {
            let seen = SeenUnit {
                id: unit.id,
                kind: unit.kind,
                tile: unit.tile,
                seen: now,
            };
            match self.units.binary_search_by_key(&unit.id, |known| known.id) {
                Ok(index) => self.units[index] = seen,
                Err(index) => self.units.insert(index, seen),
            }
        }
        self.units.retain(|unit| {
            now < unit.seen + UNIT_TICKS && (unit.seen == now || !observation.visible(unit.tile))
        });
        while self.units.len() > UNIT_CAP {
            let stalest = self
                .units
                .iter()
                .enumerate()
                .min_by_key(|(_, unit)| (unit.seen, unit.id))
                .map(|(index, _)| index)
                .expect("over the cap");
            self.units.remove(stalest);
        }
    }

    /// Remembered enemy units, by id.
    pub(crate) fn units(&self) -> &[SeenUnit] {
        &self.units
    }

    /// Remembers that `kind` could not be claimed at `anchor`.
    pub(crate) fn fail(&mut self, kind: BuildingKind, anchor: TilePos, now: u64) {
        record(&mut self.failures, kind, anchor, now);
    }

    /// Whether `kind` recently failed at `anchor`.
    pub(crate) fn failed(&self, kind: BuildingKind, anchor: TilePos, now: u64) -> bool {
        recent(&self.failures, kind, anchor, now)
    }

    /// Remembers giving up the attack on `kind` at `anchor`.
    pub(crate) fn abandon(&mut self, kind: BuildingKind, anchor: TilePos, now: u64) {
        record(&mut self.abandoned, kind, anchor, now);
    }

    /// Whether the attack on `kind` at `anchor` was recently given up.
    pub(crate) fn abandoned(&self, kind: BuildingKind, anchor: TilePos, now: u64) -> bool {
        recent(&self.abandoned, kind, anchor, now)
    }

    /// When each scouting point was last in sight, sized to `points` on first
    /// use.
    pub(crate) fn scouted(&mut self, points: usize) -> &mut [u64] {
        if self.scouted.is_empty() {
            self.scouted = vec![0; points];
        }
        &mut self.scouted
    }

    /// Forgets failures old enough to try again.
    pub(crate) fn forget(&mut self, now: u64) {
        self.failures
            .retain(|failure| now < failure.at + FAILURE_TICKS);
        self.abandoned
            .retain(|failure| now < failure.at + FAILURE_TICKS);
    }

    /// Rejects a restored memory that could not have been recorded by `now`
    /// for a seat with `points` scouting points.
    pub(crate) fn validate(&self, now: u64, points: usize) -> Result<(), String> {
        let sized = self.scouted.is_empty() || self.scouted.len() == points;
        if !sized || self.scouted.iter().any(|tick| *tick > now) {
            return Err("checkpoint scouting memory does not fit the map".into());
        }
        let full = |list: &[Failure]| list.len() > FAILURE_CAP;
        if full(&self.failures) || full(&self.abandoned) || self.units.len() > UNIT_CAP {
            return Err("checkpoint remembers too much".into());
        }
        let by_id = self.units.windows(2).all(|pair| pair[0].id < pair[1].id);
        if !by_id || self.units.iter().any(|unit| unit.seen > now) {
            return Err("checkpoint enemy units are out of order".into());
        }
        let ordered = |list: &[Failure]| {
            list.windows(2).all(|pair| pair[0].at <= pair[1].at)
                && list.iter().all(|failure| failure.at <= now)
        };
        if !ordered(&self.failures) || !ordered(&self.abandoned) {
            return Err("checkpoint failures are out of order".into());
        }
        Ok(())
    }
}

/// Remembers `kind` at `anchor` in `list`, oldest forgotten first once full.
fn record(list: &mut Vec<Failure>, kind: BuildingKind, anchor: TilePos, now: u64) {
    list.retain(|failure| (failure.kind, failure.anchor) != (kind, anchor));
    if list.len() == FAILURE_CAP {
        list.remove(0);
    }
    list.push(Failure {
        kind,
        anchor,
        at: now,
    });
}

/// Whether `list` recorded `kind` at `anchor` recently.
fn recent(list: &[Failure], kind: BuildingKind, anchor: TilePos, now: u64) -> bool {
    list.iter().any(|failure| {
        (failure.kind, failure.anchor) == (kind, anchor) && now < failure.at + FAILURE_TICKS
    })
}

impl SeenUnit {
    /// Per-mille confidence that the unit is still roughly where and what it
    /// was.
    pub(crate) fn confidence(&self, now: u64) -> u32 {
        let age = now.saturating_sub(self.seen).min(UNIT_TICKS);
        ((UNIT_TICKS - age) * 1_000 / UNIT_TICKS) as u32
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
        assert_eq!(memory.validate(200, 0), Ok(()));
        assert!(memory.validate(199, 0).is_err());
    }
}
