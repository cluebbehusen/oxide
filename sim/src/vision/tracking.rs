//! Continuous association of observations, never of concealed entities.

use super::{Sighting, Vision};
use crate::{ContactId, PlayerId, State, Tick, UnitId};
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};

/// One tick's reported position in a continuous contact history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactSample {
    /// Observation tick.
    pub tick: Tick,
    /// Reported tile, including while directly visible.
    pub tile: TilePos,
}

/// A team-local mobile contact. Identity exists only during true sight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactTrack {
    /// Continuous observation identity, unrelated to entity ids.
    pub id: ContactId,
    /// Current reported tile.
    pub tile: TilePos,
    /// Identified unit, present only while currently visible.
    pub visible_unit: Option<UnitId>,
    /// At most one second of consecutive observations, oldest first.
    pub history: Vec<ContactSample>,
}

impl ContactTrack {
    /// Fixed-point velocity estimated exclusively from reported movement.
    pub fn velocity(&self) -> Vec2Fx {
        let Some(first) = self.history.first() else {
            return Vec2Fx::ZERO;
        };
        let Some(last) = self.history.last() else {
            return Vec2Fx::ZERO;
        };
        let elapsed = last.tick.saturating_sub(first.tick);
        if elapsed == 0 {
            return Vec2Fx::ZERO;
        }
        let velocity = (last.tile.center() - first.tile.center()) / Fx::from_num(elapsed);
        let limit = movement_bound();
        if velocity.length() > limit {
            Vec2Fx::ZERO.move_toward(velocity, limit)
        } else {
            velocity
        }
    }
}

fn movement_bound() -> Fx {
    crate::stats::UnitKind::ALL
        .iter()
        .map(|kind| kind.stats().speed)
        .max()
        .unwrap_or(Fx::ZERO)
        + crate::stats::COLLISION_MAX_STEP
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(super) struct Tracking {
    pub(super) next_id: u32,
    pub(super) tracks: Vec<ContactTrack>,
}

impl Tracking {
    pub(super) fn refresh(
        &mut self,
        view: &Vision,
        state: &State,
        player: PlayerId,
        sightings: &[Sighting],
    ) {
        let team = state.player(player).team;
        let mut observations: Vec<(TilePos, Option<UnitId>)> = view
            .contacts
            .iter()
            .copied()
            .map(|tile| (tile, None))
            .collect();
        observations.extend(
            sightings
                .iter()
                .filter(|unit| unit.team != team && view.visible(unit.tile))
                .map(|unit| (unit.tile, Some(unit.id))),
        );
        observations.sort_unstable_by_key(|(tile, unit)| (tile.y, tile.x, *unit));
        // Sorted by tile, so a row's observations from `x0` through `x1` are
        // one contiguous run.
        let span = |y: i32, x0: i32, x1: i32| {
            let start = observations.partition_point(|(t, _)| (t.y, t.x) < (y, x0));
            let len = observations[start..].partition_point(|(t, _)| (t.y, t.x) <= (y, x1));
            start..start + len
        };
        let live = |track: &ContactTrack| {
            track
                .history
                .last()
                .is_some_and(|sample| sample.tick.saturating_add(1) >= state.current_tick())
        };
        let mut used = vec![false; self.tracks.len()];
        let mut matches = vec![None; observations.len()];
        // A track following a visible unit takes that unit's observation
        // first when it lies in the track's window: identified pairings sort
        // ahead of every other, and no two of them share a track or an
        // observation, since each visible unit has one of each.
        let mut seen: Vec<(UnitId, usize)> = observations
            .iter()
            .enumerate()
            .filter_map(|(new, &(_, unit))| unit.map(|unit| (unit, new)))
            .collect();
        seen.sort_unstable();
        for (old, track) in self.tracks.iter().enumerate() {
            let Some(unit) = track.visible_unit.filter(|_| live(track)) else {
                continue;
            };
            if let Ok(at) = seen.binary_search_by_key(&unit, |&(unit, _)| unit) {
                let new = seen[at].1;
                if observations[new].0.chebyshev(track.tile) <= 1 {
                    used[old] = true;
                    matches[new] = Some(old);
                }
            }
        }
        let mut candidates = Vec::new();
        for (old, track) in self.tracks.iter().enumerate() {
            if used[old] || !live(track) {
                continue;
            }
            let predicted = track.tile.center() + track.velocity();
            for dy in -1..=1 {
                for new in span(track.tile.y + dy, track.tile.x - 1, track.tile.x + 1) {
                    if matches[new].is_some() {
                        continue;
                    }
                    let (tile, visible) = observations[new];
                    if track.visible_unit.zip(visible).is_some_and(|(a, b)| a != b) {
                        continue;
                    }
                    let identified = track.visible_unit.is_some() && track.visible_unit == visible;
                    candidates.push((
                        !identified,
                        predicted.dist_sq(tile.center()),
                        track.tile.center().dist_sq(tile.center()),
                        track.id,
                        tile.y,
                        tile.x,
                        visible,
                        old,
                        new,
                    ));
                }
            }
        }
        candidates.sort_unstable();
        for (_, _, _, _, _, _, _, old, new) in candidates {
            if !used[old] && matches[new].is_none() {
                used[old] = true;
                matches[new] = Some(old);
            }
        }
        let mut previous: Vec<Option<ContactTrack>> = std::mem::take(&mut self.tracks)
            .into_iter()
            .map(Some)
            .collect();
        let mut tracks = Vec::with_capacity(observations.len());
        for (new, (tile, visible_unit)) in observations.into_iter().enumerate() {
            let mut track = if let Some(old) = matches[new] {
                previous[old].take().expect("a track matches at most once")
            } else {
                let id = ContactId(self.next_id);
                self.next_id += 1;
                ContactTrack {
                    id,
                    tile,
                    visible_unit,
                    history: Vec::new(),
                }
            };
            track.tile = tile;
            track.visible_unit = visible_unit;
            // Samples are consecutive ticks, oldest first, so the ones kept
            // form one run. Refresh may run more than once at a scenario's
            // initial tick, so the current tick may already be recorded.
            let now = state.current_tick();
            let kept = track.history.partition_point(|sample| sample.tick < now);
            track.history.truncate(kept);
            let stale = track
                .history
                .partition_point(|sample| now - sample.tick > u64::from(crate::TICKS_PER_SECOND));
            track.history.drain(..stale);
            track.history.push(ContactSample {
                tick: state.current_tick(),
                tile,
            });
            tracks.push(track);
        }
        tracks.sort_unstable_by_key(|track| track.id);
        self.tracks = tracks;
    }

    pub(super) fn valid(&self, view: &Vision, state: &State, player: PlayerId) -> bool {
        if self.next_id > u32::MAX / 2 || self.tracks.windows(2).any(|w| w[0].id >= w[1].id) {
            return false;
        }
        let inside = |tile: TilePos| {
            tile.x >= 0
                && tile.y >= 0
                && tile.x < state.map().width()
                && tile.y < state.map().height()
        };
        let mut reported = Vec::new();
        let mut visible = Vec::new();
        for track in &self.tracks {
            if track.id.0 >= self.next_id
                || !inside(track.tile)
                || track.history.is_empty()
                || track.history.len() > crate::TICKS_PER_SECOND as usize + 1
                || track.history.last().is_none_or(|sample| {
                    sample.tile != track.tile
                        || (state.result.is_none()
                            && sample.tick.saturating_add(1) < state.current_tick())
                })
                || track.history.iter().any(|sample| {
                    !inside(sample.tile)
                        || sample.tick > state.current_tick()
                        || (state.result.is_none()
                            && state.current_tick() - sample.tick
                                > u64::from(crate::TICKS_PER_SECOND) + 1)
                })
                || track
                    .history
                    .windows(2)
                    .any(|w| w[1].tick != w[0].tick + 1 || w[0].tile.chebyshev(w[1].tile) > 1)
            {
                return false;
            }
            if let Some(id) = track.visible_unit {
                if !state.unit(id).is_some_and(|u| {
                    state.hostile(player, u.player)
                        && view.visible(u.tile())
                        && u.tile() == track.tile
                }) {
                    return false;
                }
                visible.push(id);
            } else {
                if view.visible(track.tile) {
                    return false;
                }
                reported.push(track.tile);
            }
        }
        reported.sort_unstable_by_key(|tile| (tile.y, tile.x));
        visible.sort_unstable();
        // Empty tracking is accepted; the next refresh initializes it.
        self.tracks.is_empty() && self.next_id == 0
            || (reported == view.contacts
                && visible
                    == state
                        .units()
                        .iter()
                        .filter(|u| state.hostile(player, u.player) && view.visible(u.tile()))
                        .map(|u| u.id)
                        .collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests;
