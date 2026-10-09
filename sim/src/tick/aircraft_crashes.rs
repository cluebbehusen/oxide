use crate::event::Event;
use crate::ids::UnitId;
use crate::map::Terrain;
use crate::state::{AircraftCrash, State};
use crate::stats::{AIRCRAFT_CRASH_TICKS, Domain};
use chassis::fx::{Fx, Vec2Fx, sqrt};
use chassis::grid::TilePos;

pub(super) fn capture_positions(state: &State) -> Vec<(UnitId, Vec2Fx)> {
    state
        .units
        .iter()
        .filter(|unit| unit.hp > 0 && unit.kind.stats().crash.is_some())
        .map(|unit| (unit.id, unit.pos))
        .collect()
}

pub(super) fn remember_motion(state: &mut State, before: &[(UnitId, Vec2Fx)]) {
    for (unit, &(id, pos)) in state
        .units
        .iter_mut()
        .filter(|unit| unit.hp > 0 && unit.kind.stats().crash.is_some())
        .zip(before)
    {
        debug_assert_eq!(unit.id, id);
        let delta = unit.pos - pos;
        let speed = unit.kind.stats().speed;
        unit.air_motion = if unit.domain() == Domain::Ground {
            Vec2Fx::ZERO
        } else if delta.length_sq() > speed * speed {
            delta * (speed / (sqrt(delta.length_sq()) + const { Fx::lit("0.000001") }))
        } else {
            delta
        };
        if unit
            .kind
            .stats()
            .crash
            .is_some_and(|crash| crash.aligns_to_motion)
            && unit.air_motion != Vec2Fx::ZERO
        {
            unit.heading = chassis::compass::heading_of(unit.air_motion);
        }
    }
}

pub(super) fn schedule(state: &mut State) {
    for unit in &state.units {
        if unit.hp != 0 || unit.domain() != Domain::Air || unit.kind.stats().crash.is_none() {
            continue;
        }
        let coast = unit.air_motion
            * Fx::from_num(AIRCRAFT_CRASH_TICKS)
            * crate::stats::AIRCRAFT_CRASH_COAST;
        state.aircraft_crashes.push(AircraftCrash {
            unit: unit.id,
            player: unit.player,
            kind: unit.kind,
            heading: unit.heading,
            launch: unit.pos,
            impact: state.map.clamp_to_envelope(unit.pos + coast),
            started: state.tick,
            arrival: state.tick + AIRCRAFT_CRASH_TICKS,
        });
    }
}

pub(super) fn land(state: &mut State, events: &mut Vec<Event>) {
    let due = state
        .aircraft_crashes
        .partition_point(|crash| crash.arrival <= state.tick);
    for crash in state.aircraft_crashes.drain(..due) {
        events.push(Event::AircraftImpacted { crash });
        if !state
            .map
            .tile(TilePos::containing(crash.impact))
            .is_some_and(|tile| tile.terrain != Terrain::Pit)
        {
            continue;
        }
        let profile = crash.kind.stats().crash.expect("scheduled large aircraft");
        let radius_sq = profile.radius * profile.radius;
        let team = state.players[usize::from(crash.player.0)].team;
        for unit in &mut state.units {
            if unit.domain() == Domain::Ground
                && state.players[usize::from(unit.player.0)].team != team
                && unit.pos.dist_sq(crash.impact) <= radius_sq
            {
                super::damage::unit(unit, profile.damage, events);
            }
        }
        for building in &mut state.buildings {
            if !building.provisional
                && state.players[usize::from(building.player.0)].team != team
                && building
                    .closest_point_to(crash.impact)
                    .dist_sq(crash.impact)
                    <= radius_sq
            {
                super::damage::building(building, profile.damage, events);
            }
        }
    }
}

#[cfg(test)]
mod tests;
