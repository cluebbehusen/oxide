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
        .filter(|unit| unit.hp > 0 && unit.kind.crash_profile().is_some())
        .map(|unit| (unit.id, unit.pos))
        .collect()
}

pub(super) fn remember_motion(state: &mut State, before: &[(UnitId, Vec2Fx)]) {
    for (unit, &(id, pos)) in state
        .units
        .iter_mut()
        .filter(|unit| unit.hp > 0 && unit.kind.crash_profile().is_some())
        .zip(before)
    {
        debug_assert_eq!(unit.id, id);
        let delta = unit.pos - pos;
        let speed = unit.kind.stats().speed;
        unit.air_motion = if unit.domain() == Domain::Ground {
            Vec2Fx::ZERO
        } else if delta.length_sq() > speed * speed {
            delta * (speed / (sqrt(delta.length_sq()) + Fx::lit("0.000001")))
        } else {
            delta
        };
        if unit.kind == crate::UnitKind::Skyhook && unit.air_motion != Vec2Fx::ZERO {
            unit.heading = super::flight::heading_of(unit.air_motion);
        }
    }
}

pub(super) fn schedule(state: &mut State) {
    for unit in &state.units {
        if unit.hp != 0 || unit.domain() != Domain::Air || unit.kind.crash_profile().is_none() {
            continue;
        }
        let coast = unit.air_motion * Fx::from_num(AIRCRAFT_CRASH_TICKS) * Fx::lit("0.8");
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
        let profile = crash
            .kind
            .crash_profile()
            .expect("scheduled large aircraft");
        let radius_sq = profile.radius * profile.radius;
        let team = state.players[usize::from(crash.player.0)].team;
        for unit in &mut state.units {
            if unit.domain() == Domain::Ground
                && state.players[usize::from(unit.player.0)].team != team
                && unit.pos.dist_sq(crash.impact) <= radius_sq
            {
                unit.hp = unit.hp.saturating_sub(profile.damage);
            }
        }
        for building in &mut state.buildings {
            if state.players[usize::from(building.player.0)].team != team
                && building
                    .closest_point_to(crash.impact)
                    .dist_sq(crash.impact)
                    <= radius_sq
            {
                building.hp = building.hp.saturating_sub(profile.damage);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildingKind, PlayerId, Scenario, UnitKind};

    fn arena() -> State {
        let mut scenario = Scenario::skirmish();
        scenario.map = vec![".".repeat(48); 32];
        scenario.map[2].replace_range(2..3, "1");
        scenario.map[2].replace_range(42..43, "2");
        scenario.units.clear();
        scenario.buildings.clear();
        scenario.build().unwrap()
    }

    fn at(x: i32, y: i32) -> Vec2Fx {
        TilePos::new(x, y).center()
    }

    fn casualty(state: &mut State, kind: UnitKind, motion: Vec2Fx) -> UnitId {
        let id = state.spawn_unit(PlayerId(0), kind, at(20, 16));
        let unit = state.units.iter_mut().find(|unit| unit.id == id).unwrap();
        unit.air_motion = motion;
        unit.hp = 0;
        id
    }

    #[test]
    fn crash_damage_waits_for_contact_and_survives_state_round_trip() {
        for kind in [UnitKind::Condor, UnitKind::Moth, UnitKind::Skyhook] {
            let mut state = arena();
            let target = state.spawn_unit(PlayerId(1), UnitKind::Harvester, at(21, 16));
            let hp = state.unit(target).unwrap().hp;
            let id = casualty(&mut state, kind, Vec2Fx::new(kind.stats().speed, Fx::ZERO));
            state.tick(&[]);
            assert!(state.unit(id).is_none());
            let crash = state.aircraft_crashes[0];
            assert!(crash.impact.x > crash.launch.x);
            assert_eq!(crash.impact.y, crash.launch.y);
            for _ in 1..AIRCRAFT_CRASH_TICKS {
                state.tick(&[]);
                assert_eq!(state.unit(target).unwrap().hp, hp);
            }
            let mut resumed: State =
                serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
            let report = state.tick(&[]);
            assert_eq!(report, resumed.tick(&[]));
            assert_eq!(state.hash(), resumed.hash());
            assert!(report.events.contains(&Event::AircraftImpacted { crash }));
            assert_eq!(
                state.unit(target).map_or(0, |u| u.hp),
                hp.saturating_sub(kind.crash_profile().unwrap().damage)
            );
            assert!(state.aircraft_crashes.is_empty());
            let next_hp = state.unit(target).map_or(0, |u| u.hp);
            state.tick(&[]);
            assert_eq!(state.unit(target).map_or(0, |u| u.hp), next_hp);
        }
    }

    #[test]
    fn crash_blast_uses_current_ground_targets_and_building_footprints() {
        let mut state = arena();
        let friendly = state.spawn_unit(PlayerId(0), UnitKind::Harvester, at(20, 16));
        let air = state.spawn_unit(PlayerId(1), UnitKind::Skyhook, at(20, 16));
        let dodged = state.spawn_unit(PlayerId(1), UnitKind::Harvester, at(20, 16));
        let entered = state.spawn_unit(PlayerId(1), UnitKind::Harvester, at(30, 16));
        let building =
            state.place_building(PlayerId(1), BuildingKind::Foundry, TilePos::new(22, 16));
        let ally_building =
            state.place_building(PlayerId(0), BuildingKind::Foundry, TilePos::new(18, 16));
        let building_hp = state.building(building).unwrap().hp;
        let friendly_hp = state.unit(friendly).unwrap().hp;
        let air_hp = state.unit(air).unwrap().hp;
        let ally_hp = state.building(ally_building).unwrap().hp;
        casualty(&mut state, UnitKind::Condor, Vec2Fx::ZERO);
        schedule(&mut state);
        state.units.iter_mut().find(|u| u.id == dodged).unwrap().pos = at(30, 16);
        state
            .units
            .iter_mut()
            .find(|u| u.id == entered)
            .unwrap()
            .pos = at(20, 16);
        state.tick = AIRCRAFT_CRASH_TICKS;
        land(&mut state, &mut Vec::new());
        assert_eq!(state.unit(friendly).unwrap().hp, friendly_hp);
        assert_eq!(state.unit(air).unwrap().hp, air_hp);
        assert_eq!(
            state.unit(dodged).unwrap().hp,
            UnitKind::Harvester.stats().max_hp
        );
        assert_eq!(
            state.unit(entered).unwrap().hp,
            UnitKind::Harvester.stats().max_hp.saturating_sub(50)
        );
        assert_eq!(state.building(building).unwrap().hp, building_hp - 50);
        assert_eq!(state.building(ally_building).unwrap().hp, ally_hp);
    }

    #[test]
    fn pit_impacts_and_grounded_or_small_casualties_do_not_blast() {
        let mut state = arena();
        let mut scenario = Scenario::skirmish();
        scenario.map = vec![".".repeat(48); 32];
        scenario.map[2].replace_range(2..3, "1");
        scenario.map[2].replace_range(42..43, "2");
        scenario.map[16].replace_range(20..21, "~");
        state.map = scenario.build().unwrap().map;
        let target = state.spawn_unit(PlayerId(1), UnitKind::Harvester, at(21, 16));
        casualty(&mut state, UnitKind::Condor, Vec2Fx::ZERO);
        let parked = casualty(&mut state, UnitKind::Moth, Vec2Fx::ZERO);
        state
            .units
            .iter_mut()
            .find(|u| u.id == parked)
            .unwrap()
            .landed = true;
        casualty(&mut state, UnitKind::Talon, Vec2Fx::ZERO);
        schedule(&mut state);
        assert_eq!(state.aircraft_crashes.len(), 1);
        state.tick = AIRCRAFT_CRASH_TICKS;
        let mut events = Vec::new();
        land(&mut state, &mut events);
        assert_eq!(events.len(), 1);
        assert_eq!(
            state.unit(target).unwrap().hp,
            UnitKind::Harvester.stats().max_hp
        );
    }

    #[test]
    fn pending_crash_does_not_delay_the_last_foundry_result() {
        let mut state = arena();
        let aircraft = casualty(&mut state, UnitKind::Condor, Vec2Fx::ZERO);
        let impact = state.buildings[1].center();
        state
            .units
            .iter_mut()
            .find(|u| u.id == aircraft)
            .unwrap()
            .pos = impact;
        state.buildings[0].hp = 0;
        state.buildings[1].hp = UnitKind::Condor.crash_profile().unwrap().damage;
        state.tick(&[]);
        assert_eq!(state.result, Some(crate::GameResult::Victory { team: 1 }));
        assert!(state.aircraft_crashes.is_empty());
        let hp = state.buildings[0].hp;
        for _ in 0..AIRCRAFT_CRASH_TICKS {
            state.tick(&[]);
            state.validate_invariants().unwrap();
        }
        assert_eq!(state.result, Some(crate::GameResult::Victory { team: 1 }));
        assert_eq!(state.buildings[0].hp, hp);
    }

    #[test]
    fn stored_momentum_follows_actual_motion_and_is_bounded_after_collision() {
        let mut state = arena();
        let id = state.spawn_unit(PlayerId(0), UnitKind::Skyhook, at(20, 16));
        let before = capture_positions(&state);
        remember_motion(&mut state, &before);
        assert_eq!(state.unit(id).unwrap().air_motion, Vec2Fx::ZERO);
        let delta = Vec2Fx::new(Fx::lit("0.04"), Fx::lit("-0.03"));
        state.units.iter_mut().find(|u| u.id == id).unwrap().pos += delta;
        remember_motion(&mut state, &before);
        assert_eq!(state.unit(id).unwrap().air_motion, delta);
        assert_eq!(
            state.unit(id).unwrap().heading,
            super::super::flight::heading_of(delta)
        );
        state.units.iter_mut().find(|u| u.id == id).unwrap().pos += Vec2Fx::new(Fx::ONE, Fx::ONE);
        remember_motion(&mut state, &before);
        state.validate_invariants().unwrap();
        let unit = state.units.iter_mut().find(|u| u.id == id).unwrap();
        unit.kind = UnitKind::Condor;
        unit.landed = true;
        remember_motion(&mut state, &before);
        assert_eq!(state.unit(id).unwrap().air_motion, Vec2Fx::ZERO);
    }

    #[test]
    fn simultaneous_crashes_are_ordered_and_cargo_does_not_schedule_another_blast() {
        let mut state = arena();
        let carrier = casualty(&mut state, UnitKind::Skyhook, Vec2Fx::ZERO);
        let bomber = casualty(&mut state, UnitKind::Condor, Vec2Fx::ZERO);
        let rider = state.spawn_unit(PlayerId(0), UnitKind::Sentinel, at(20, 16));
        let rider = state
            .units
            .remove(state.units.iter().position(|u| u.id == rider).unwrap());
        state
            .units
            .iter_mut()
            .find(|u| u.id == carrier)
            .unwrap()
            .cargo
            .push(rider);
        state.tick(&[]);
        assert_eq!(
            state
                .aircraft_crashes
                .iter()
                .map(|c| c.unit)
                .collect::<Vec<_>>(),
            [carrier, bomber]
        );
        state.validate_invariants().unwrap();
        for _ in 1..AIRCRAFT_CRASH_TICKS {
            state.tick(&[]);
        }
        let impacts: Vec<_> = state
            .tick(&[])
            .events
            .into_iter()
            .filter_map(|event| match event {
                Event::AircraftImpacted { crash } => Some(crash.unit),
                _ => None,
            })
            .collect();
        assert_eq!(impacts, [carrier, bomber]);
    }
}
