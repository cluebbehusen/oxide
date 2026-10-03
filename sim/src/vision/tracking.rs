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
            match track.visible_unit {
                Some(id) => {
                    if !state.unit(id).is_some_and(|u| {
                        state.hostile(player, u.player)
                            && view.visible(u.tile())
                            && u.tile() == track.tile
                    }) {
                        return false;
                    }
                    visible.push(id);
                }
                None => {
                    if view.visible(track.tile) {
                        return false;
                    }
                    reported.push(track.tile);
                }
            }
        }
        reported.sort_unstable_by_key(|tile| (tile.y, tile.x));
        visible.sort_unstable();
        // Empty tracking accepts old snapshots; the next refresh initializes it.
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
mod tests {
    use super::*;

    /// The previous refresh, kept verbatim as the reference: every
    /// candidate pairing generated and sorted.
    fn sorted_refresh(this: &mut Tracking, view: &Vision, state: &State, player: PlayerId) {
        let mut observations: Vec<(TilePos, Option<UnitId>)> = view
            .contacts
            .iter()
            .copied()
            .map(|tile| (tile, None))
            .collect();
        observations.extend(
            state
                .units()
                .iter()
                .filter(|u| state.hostile(player, u.player) && view.visible(u.tile()))
                .map(|u| (u.tile(), Some(u.id))),
        );
        observations.sort_unstable_by_key(|(tile, unit)| (tile.y, tile.x, *unit));
        // Sorted by tile, so a row's observations from `x0` through `x1` are
        // one contiguous run.
        let span = |y: i32, x0: i32, x1: i32| {
            let start = observations.partition_point(|(t, _)| (t.y, t.x) < (y, x0));
            let len = observations[start..].partition_point(|(t, _)| (t.y, t.x) <= (y, x1));
            start..start + len
        };
        let mut candidates = Vec::new();
        for (old, track) in this.tracks.iter().enumerate() {
            if track
                .history
                .last()
                .is_none_or(|sample| sample.tick.saturating_add(1) < state.current_tick())
            {
                continue;
            }
            let predicted = track.tile.center() + track.velocity();
            for dy in -1..=1 {
                for new in span(track.tile.y + dy, track.tile.x - 1, track.tile.x + 1) {
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
        let mut used = vec![false; this.tracks.len()];
        let mut matches = vec![None; observations.len()];
        for (_, _, _, _, _, _, _, old, new) in candidates {
            if !used[old] && matches[new].is_none() {
                used[old] = true;
                matches[new] = Some(old);
            }
        }
        let mut previous: Vec<Option<ContactTrack>> = std::mem::take(&mut this.tracks)
            .into_iter()
            .map(Some)
            .collect();
        let mut tracks = Vec::with_capacity(observations.len());
        for (new, (tile, visible_unit)) in observations.into_iter().enumerate() {
            let mut track = if let Some(old) = matches[new] {
                previous[old].take().expect("a track matches at most once")
            } else {
                let id = ContactId(this.next_id);
                this.next_id += 1;
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
        this.tracks = tracks;
    }

    #[test]
    fn a_new_blip_beside_a_seen_unit_leaves_that_unit_its_track() {
        let mut state = crate::Scenario::skirmish()
            .build()
            .expect("skirmish builds");
        state.units.clear();
        let tile = TilePos::new(10, 10);
        let enemy = state.spawn_unit(PlayerId(1), crate::UnitKind::Sentinel, tile.center());
        let mut view = Vision::new(state.map().width(), state.map().height());
        *view.visible.get_mut(tile).expect("on the map") = true;
        let refresh = |tracking: &mut Tracking, view: &Vision, state: &State| {
            let mut sorted = tracking.clone();
            tracking.refresh(view, state, PlayerId(0), &Sighting::gather(state));
            sorted_refresh(&mut sorted, view, state, PlayerId(0));
            assert_eq!(*tracking, sorted);
        };
        let mut tracking = Tracking::default();
        refresh(&mut tracking, &view, &state);
        state.tick += 1;
        view.contacts = vec![tile.offset(1, 0)];
        refresh(&mut tracking, &view, &state);
        assert_eq!(tracking.tracks.len(), 2);
        assert!(
            tracking
                .tracks
                .iter()
                .any(|track| track.id == ContactId(0) && track.visible_unit == Some(enemy))
        );
    }

    #[test]
    fn direct_pairing_matches_sorting_every_candidate() {
        use crate::command::{Command, PlayerCommand};
        use crate::scenario::{BuildingSpec, PlayerSpec, Scenario, UnitSpec};
        use crate::stats::BuildingKind;
        use crate::stats::UnitKind;
        let seat = |name: &str, faction| PlayerSpec {
            name: name.into(),
            faction,
            team: None,
            scrap: 0,
            bot: false,
            bot_config: None,
        };
        let army = |player: u8, x: i32| {
            (0..12).map(move |i| UnitSpec {
                player,
                kind: [UnitKind::Sentinel, UnitKind::Scuttler, UnitKind::Lancer][i % 3],
                x: x + (i as i32 % 3),
                y: 1 + i as i32 / 3 * 2,
            })
        };
        let mut state = Scenario {
            mode: Default::default(),
            name: "converging-armies".into(),
            seed: 11,
            map: (0..13)
                .map(|y| match y {
                    1 => ".1".to_owned() + &".".repeat(29),
                    9 => ".".repeat(27) + "2" + "...",
                    _ => ".".repeat(31),
                })
                .collect(),
            players: vec![
                seat("West", crate::Faction::Ferrous),
                seat("East", crate::Faction::Cupric),
            ],
            units: army(0, 6).chain(army(1, 22)).collect(),
            // West hears East's army on radar before seeing it, so radar
            // contacts take the sorted pairing path.
            buildings: vec![BuildingSpec {
                player: 0,
                kind: BuildingKind::Array,
                x: 2,
                y: 10,
            }],
            meta: None,
        }
        .build()
        .expect("arena builds");
        let hunt = |state: &State, player: u8, x: i32| PlayerCommand {
            player: PlayerId(player),
            command: Command::Hunt {
                units: state
                    .units()
                    .iter()
                    .filter(|u| u.player == PlayerId(player))
                    .map(|u| u.id)
                    .collect(),
                goal: TilePos::new(x, 6),
                queue: false,
            },
        };
        let orders = [hunt(&state, 0, 28), hunt(&state, 1, 2)];
        state.tick(&orders);
        let (mut matched, mut heard) = (0, 0);
        for _ in 0..300 {
            state.tick(&[]);
            let sightings = Sighting::gather(&state);
            for (index, view) in state.vision.iter().enumerate() {
                let player = PlayerId(index as u8);
                let (mut direct, mut sorted) = (view.tracking.clone(), view.tracking.clone());
                direct.refresh(view, &state, player, &sightings);
                sorted_refresh(&mut sorted, view, &state, player);
                assert_eq!(direct, sorted, "seat {index} at tick {}", state.tick);
                matched += direct.tracks.len();
                heard += view.contacts.len();
            }
        }
        assert!(
            matched > 1000 && heard > 100,
            "the armies never came into sight ({matched}) or radar ({heard})"
        );
    }

    fn observe(tracking: &mut Tracking, state: &mut State, positions: &[(i32, i32)]) {
        let mut view = Vision::new(state.map().width(), state.map().height());
        view.contacts = positions.iter().map(|&(x, y)| TilePos::new(x, y)).collect();
        view.contacts.sort_unstable_by_key(|tile| (tile.y, tile.x));
        tracking.refresh(&view, state, PlayerId(0), &Sighting::gather(state));
        state.tick += 1;
    }

    #[test]
    fn crossing_and_merged_observations_never_resurrect_retired_ids() {
        let mut state = crate::Scenario::skirmish().build().unwrap();
        let mut tracks = Tracking::default();
        observe(&mut tracks, &mut state, &[(8, 8), (10, 8)]);
        let original: Vec<_> = tracks.tracks.iter().map(|t| t.id).collect();
        observe(&mut tracks, &mut state, &[(9, 8)]);
        assert_eq!(tracks.tracks.len(), 1);
        let survivor = tracks.tracks[0].id;
        let retired = *original.iter().find(|&&id| id != survivor).unwrap();
        observe(&mut tracks, &mut state, &[(8, 8), (10, 8)]);
        assert!(tracks.tracks.iter().any(|t| t.id == survivor));
        assert!(!tracks.tracks.iter().any(|t| t.id == retired));
        let latest = tracks.next_id;
        observe(&mut tracks, &mut state, &[]);
        observe(&mut tracks, &mut state, &[(8, 8)]);
        assert_eq!(tracks.tracks[0].id, ContactId(latest));
        assert_eq!(tracks.tracks[0].velocity(), Vec2Fx::ZERO);
    }

    #[test]
    fn movement_history_is_bounded_and_stationary_samples_remove_old_velocity() {
        let mut state = crate::Scenario::skirmish().build().unwrap();
        let mut tracks = Tracking::default();
        for x in [8, 8, 8, 8, 9, 9, 9, 9, 10] {
            observe(&mut tracks, &mut state, &[(x, 8)]);
        }
        assert!(tracks.tracks[0].velocity().x > Fx::ZERO);
        assert!(tracks.tracks[0].velocity().length() <= movement_bound());
        for _ in 0..25 {
            observe(&mut tracks, &mut state, &[(10, 8)]);
        }
        assert_eq!(tracks.tracks[0].history.len(), 21);
        assert_eq!(tracks.tracks[0].velocity(), Vec2Fx::ZERO);
    }

    #[test]
    fn detection_order_does_not_change_association() {
        let mut state = crate::Scenario::skirmish().build().unwrap();
        let mut left = Tracking::default();
        let mut right = Tracking::default();
        observe(&mut left, &mut state, &[(8, 8), (10, 8)]);
        state.tick -= 1;
        observe(&mut right, &mut state, &[(10, 8), (8, 8)]);
        assert_eq!(left, right);
        observe(&mut left, &mut state, &[(9, 8), (10, 9)]);
        state.tick -= 1;
        observe(&mut right, &mut state, &[(10, 9), (9, 8)]);
        assert_eq!(left, right);
    }
}
