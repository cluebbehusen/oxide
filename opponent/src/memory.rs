//! What the seat remembers between decisions: enemy units it has seen, with
//! confidence that fades until they are seen again; building footprints it
//! failed to claim, so it tries somewhere else for a while; enemy buildings
//! it gave up attacking, so it attacks something else for a while; enemy
//! buildings it raided, so the next raid goes elsewhere; when it last saw
//! each of its scouting points; and its own units whose orders stalled for
//! want of a route, so they sit out orders for a while. Enemy buildings need
//! no other memory here: the observation keeps their ghosts.

use crate::events::OwnEvent;
use chassis::grid::TilePos;
use oxide_sim::observation::ObservationData;
use oxide_sim::{BuildingKind, StallReason, UnitId, UnitKind};
use serde::{Deserialize, Serialize};

/// Ticks a failed footprint stays skipped.
const FAILURE_TICKS: u64 = 3_600;

/// Failures remembered at once; the oldest is forgotten first.
const FAILURE_CAP: usize = 16;

/// Ticks an enemy unit is remembered after it was last seen.
const UNIT_TICKS: u64 = 600;

/// Enemy units remembered at once, the stalest forgotten first: a computation
/// bound that normal play stays under, since units seen more than
/// `UNIT_TICKS` ago are forgotten anyway.
const UNIT_CAP: usize = 4_096;

/// Ticks an own unit whose order stalled for want of a route sits out orders,
/// unless it moves off where it stopped first.
const STUCK_TICKS: u64 = 600;

/// Stuck own units remembered at once, the oldest forgotten first: a
/// computation bound that normal play stays under.
const STUCK_CAP: usize = 1_024;

/// The seat's memory: enemy units by id, failures oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Memory {
    units: Vec<SeenUnit>,
    failures: Vec<Failure>,
    /// Attack targets given up on, oldest first.
    abandoned: Vec<Failure>,
    /// Raid targets raided, oldest first. Kept apart from abandoned targets
    /// so a raid leaves the target to larger missions.
    #[serde(default)]
    raided: Vec<Failure>,
    /// Tick each scouting point was last in sight, by point; empty before
    /// the first decision.
    scouted: Vec<u64>,
    /// Own units whose orders stalled for want of a route, by id.
    #[serde(default)]
    stuck: Vec<Stuck>,
}

/// An own unit whose order stalled for want of a route: where it stopped,
/// and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stuck {
    unit: UnitId,
    tile: TilePos,
    at: u64,
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

    /// Remembers giving up attacking, lifting to or striking `kind` at
    /// `anchor`.
    pub(crate) fn abandon(&mut self, kind: BuildingKind, anchor: TilePos, now: u64) {
        record(&mut self.abandoned, kind, anchor, now);
    }

    /// Whether attacking, lifting to or striking `kind` at `anchor` was recently
    /// given up.
    pub(crate) fn abandoned(&self, kind: BuildingKind, anchor: TilePos, now: u64) -> bool {
        recent(&self.abandoned, kind, anchor, now)
    }

    /// Remembers a raid on `kind` at `anchor`.
    pub(crate) fn raid(&mut self, kind: BuildingKind, anchor: TilePos, now: u64) {
        record(&mut self.raided, kind, anchor, now);
    }

    /// Whether `kind` at `anchor` was recently raided.
    pub(crate) fn raided(&self, kind: BuildingKind, anchor: TilePos, now: u64) -> bool {
        recent(&self.raided, kind, anchor, now)
    }

    /// When each scouting point was last in sight, sized to `points` on first
    /// use.
    pub(crate) fn scouted(&mut self, points: usize) -> &mut [u64] {
        if self.scouted.is_empty() {
            self.scouted = vec![0; points];
        }
        &mut self.scouted
    }

    /// Counts scouting point `point` as seen at `now`.
    pub(crate) fn saw(&mut self, point: usize, now: u64) {
        if let Some(seen) = self.scouted.get_mut(point) {
            *seen = now;
        }
    }

    /// Remembers the own units whose orders stalled for want of a route in
    /// `events`, and forgets those that left where they stopped, are gone, or
    /// sat out [`STUCK_TICKS`].
    pub(crate) fn stalls(&mut self, observation: &ObservationData, events: &[OwnEvent]) {
        let now = observation.tick;
        for event in events {
            if let OwnEvent::OrderStalled {
                unit,
                pos,
                reason: StallReason::NoRoute,
            } = *event
            {
                let stuck = Stuck {
                    unit,
                    tile: TilePos::containing(pos),
                    at: now,
                };
                match self.stuck.binary_search_by_key(&unit, |held| held.unit) {
                    Ok(index) => self.stuck[index] = stuck,
                    Err(index) => self.stuck.insert(index, stuck),
                }
            }
        }
        if self.stuck.is_empty() {
            return;
        }
        let tiles: std::collections::BTreeMap<UnitId, TilePos> = observation
            .my_units
            .iter()
            .map(|unit| (unit.id, unit.tile))
            .collect();
        self.stuck.retain(|stuck| {
            now < stuck.at + STUCK_TICKS && tiles.get(&stuck.unit) == Some(&stuck.tile)
        });
        while self.stuck.len() > STUCK_CAP {
            let oldest = self
                .stuck
                .iter()
                .enumerate()
                .min_by_key(|(_, stuck)| (stuck.at, stuck.unit))
                .map_or(0, |(index, _)| index);
            self.stuck.remove(oldest);
        }
    }

    /// Own units sitting out orders, by id.
    pub(crate) fn stuck(&self) -> Vec<UnitId> {
        self.stuck.iter().map(|stuck| stuck.unit).collect()
    }

    /// Forgets failures old enough to try again.
    pub(crate) fn forget(&mut self, now: u64) {
        self.failures
            .retain(|failure| now < failure.at + FAILURE_TICKS);
        self.abandoned
            .retain(|failure| now < failure.at + FAILURE_TICKS);
        self.raided
            .retain(|failure| now < failure.at + FAILURE_TICKS);
    }

    /// Rejects a restored memory that could not have been recorded by `now`
    /// on a map of the given size with `points` scouting points.
    pub(crate) fn validate(
        &self,
        now: u64,
        width: i32,
        height: i32,
        points: usize,
    ) -> Result<(), String> {
        let on_map = |tile: TilePos| (0..width).contains(&tile.x) && (0..height).contains(&tile.y);
        if !self.units.iter().all(|unit| on_map(unit.tile)) {
            return Err("checkpoint enemy units are off the map".into());
        }
        let sized = self.scouted.is_empty() || self.scouted.len() == points;
        if !sized || self.scouted.iter().any(|tick| *tick > now) {
            return Err("checkpoint scouting memory does not fit the map".into());
        }
        let full = |list: &[Failure]| list.len() > FAILURE_CAP;
        if full(&self.failures)
            || full(&self.abandoned)
            || full(&self.raided)
            || self.units.len() > UNIT_CAP
            || self.stuck.len() > STUCK_CAP
        {
            return Err("checkpoint remembers too much".into());
        }
        let stuck = self
            .stuck
            .windows(2)
            .all(|pair| pair[0].unit < pair[1].unit)
            && self
                .stuck
                .iter()
                .all(|stuck| stuck.at <= now && on_map(stuck.tile));
        if !stuck {
            return Err("checkpoint stuck units are malformed".into());
        }
        let by_id = self.units.windows(2).all(|pair| pair[0].id < pair[1].id);
        if !by_id || self.units.iter().any(|unit| unit.seen > now) {
            return Err("checkpoint enemy units are out of order".into());
        }
        let ordered = |list: &[Failure]| {
            list.windows(2).all(|pair| pair[0].at <= pair[1].at)
                && list.iter().all(|failure| failure.at <= now)
        };
        if !ordered(&self.failures) || !ordered(&self.abandoned) || !ordered(&self.raided) {
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
    /// The unit's price weighted by how sure the seat is it is still there.
    pub(crate) fn value(&self, now: u64) -> u64 {
        u64::from(self.kind.stats().cost) * u64::from(self.confidence(now)) / 1_000
    }

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
        assert_eq!(memory.validate(200, 8, 8, 0), Ok(()));
        assert!(memory.validate(199, 8, 8, 0).is_err());
    }

    #[test]
    fn raids_are_remembered_apart_from_given_up_targets() {
        let mut memory = Memory::default();
        let anchor = TilePos::new(4, 4);
        memory.raid(BuildingKind::Foundry, anchor, 100);
        assert!(memory.raided(BuildingKind::Foundry, anchor, 100));
        assert!(!memory.abandoned(BuildingKind::Foundry, anchor, 100));
        assert_eq!(memory.validate(100, 8, 8, 0), Ok(()));
        assert!(
            memory.validate(99, 8, 8, 0).is_err(),
            "a raid after the checkpoint was taken"
        );
        memory.forget(100 + FAILURE_TICKS);
        assert!(!memory.raided(BuildingKind::Foundry, anchor, 100 + FAILURE_TICKS));

        let older: Memory = serde_json::from_value(serde_json::json!({
            "units": [],
            "failures": [],
            "abandoned": [],
            "scouted": [],
        }))
        .unwrap();
        assert_eq!(older, Memory::default(), "a checkpoint from before raids");
    }
}
