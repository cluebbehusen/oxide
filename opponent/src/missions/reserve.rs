//! The home reserve: army value against ground and against aircraft that
//! offense leaves at home while an enemy could come for it, bounded by
//! stance. Defense takes every unit regardless; the reserve only limits what
//! attacks, strikes, raids and lifts take away.

use super::attack::{building_value, minimum};
use super::{Missions, Task, carrier, hits, mine, value};
use crate::frame::{HomeFrame, doubled};
use crate::map::{MapModel, UNREACHABLE};
use crate::memory::{Memory, SeenUnit};
use oxide_sim::observation::{ObservationData, UnitObs};
use oxide_sim::scenario::BotStance;
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingKind, PlayerId};
use std::cmp::Reverse;

/// The body domains a reserve covers, in the order its values keep.
const DOMAINS: [Domain; 2] = [Domain::Ground, Domain::Air];

/// Value against each domain the seat keeps home from offense, before the
/// units already out defending count: nothing while no enemy could reach
/// home that way, else the stance's floor or its share of the known enemy
/// army that could, whichever is more, less the static defenses on the
/// start's ground that already cover it.
pub(super) fn reserve(
    observation: &ObservationData,
    map: &MapModel,
    memory: &Memory,
    stance: BotStance,
) -> [u64; 2] {
    let me = observation.me;
    let now = observation.tick;
    let home = map.start(me).and_then(|start| map.component(start));
    let (floor, share) = match stance {
        BotStance::Turtle => (minimum(stance), 1_500),
        BotStance::Balanced => (minimum(stance) / 2, 1_000),
        BotStance::Aggressive => (0, 500),
    };
    let armed = |unit: &&SeenUnit| !unit.kind.stats().weapons.is_empty();
    let airworks = observation
        .enemy_buildings
        .iter()
        .any(|building| building.kind == BuildingKind::Airworks);
    let walks = map
        .start(me)
        .is_some_and(|start| map.hostile_distance(me, start) != UNREACHABLE);
    let reach = [
        walks || airworks || memory.units().iter().any(|unit| carrier(unit.kind)),
        airworks
            || memory
                .units()
                .iter()
                .filter(armed)
                .any(|unit| unit.kind.stats().domain == Domain::Air),
    ];
    std::array::from_fn(|index| {
        if !reach[index] {
            return 0;
        }
        let domain = DOMAINS[index];
        let threat = threat(map, memory, me, now, domain);
        let standing: u64 = observation
            .my_buildings
            .iter()
            .filter(|building| building.built && map.component(building.anchor) == home)
            .filter(|building| {
                building
                    .kind
                    .base_stats()
                    .weapons
                    .iter()
                    .any(|weapon| weapon.targets.covers(domain))
            })
            .map(building_value)
            .sum();
        floor.max(threat * share / 1_000).saturating_sub(standing)
    })
}

/// Value of the armed enemy units of `domain` the seat remembers that could
/// come at home: aircraft anywhere, ground units only on ground connected to
/// its start.
pub(crate) fn threat(
    map: &MapModel,
    memory: &Memory,
    me: PlayerId,
    now: u64,
    domain: Domain,
) -> u64 {
    memory
        .units()
        .iter()
        .filter(|unit| !unit.kind.stats().weapons.is_empty())
        .filter(|unit| unit.kind.stats().domain == domain)
        .filter(|unit| domain == Domain::Air || map.distance(me, unit.tile) != UNREACHABLE)
        .map(|unit| unit.value(now))
        .sum()
}

/// What free units at home may still take away on offense: their value
/// against each domain beyond what the reserve keeps.
#[derive(Clone)]
pub(super) struct Spare {
    home: Option<u32>,
    left: [u64; 2],
}

impl Spare {
    /// Whether `unit` may leave, charging its value against every domain it
    /// hits. A unit away from home keeps nothing there, so it may always go.
    fn take(&mut self, map: &MapModel, unit: &UnitObs) -> bool {
        if !at_home(map, self.home, unit) {
            return true;
        }
        let cost = value(unit);
        let charged = DOMAINS.map(|domain| hits(unit, domain));
        if charged
            .iter()
            .zip(self.left)
            .any(|(charged, left)| *charged && cost > left)
        {
            return false;
        }
        for (charged, left) in charged.iter().zip(&mut self.left) {
            if *charged {
                *left -= cost;
            }
        }
        true
    }

    /// Those of `units` that may leave, farthest from home first, so that
    /// the ones kept are those nearest it.
    pub(super) fn outermost<'a>(
        &mut self,
        map: &MapModel,
        frame: HomeFrame,
        mut units: Vec<&'a UnitObs>,
    ) -> Vec<&'a UnitObs> {
        units.sort_by_key(|unit| (Reverse(frame.rank(frame.home, doubled(unit.tile))), unit.id));
        self.trim(map, units)
    }

    /// Those of `units` that may leave, taken in order.
    pub(super) fn trim<'a>(&mut self, map: &MapModel, units: Vec<&'a UnitObs>) -> Vec<&'a UnitObs> {
        units
            .into_iter()
            .filter(|unit| self.take(map, unit))
            .collect()
    }

    /// A lift's payload of value against ground, cut to what may leave.
    fn payload(&self, value: u64) -> u64 {
        value.min(self.left[0])
    }
}

impl Missions {
    /// What free units at home may take away on offense now: their value
    /// against each domain beyond the part of `reserve` the units out
    /// defending do not already cover.
    pub(super) fn spare(
        &self,
        observation: &ObservationData,
        map: &MapModel,
        reserve: [u64; 2],
    ) -> Spare {
        let home = map
            .start(observation.me)
            .and_then(|start| map.component(start));
        let worth = |units: &[&UnitObs]| {
            DOMAINS.map(|domain| {
                units
                    .iter()
                    .filter(|unit| hits(unit, domain))
                    .map(|unit| value(unit))
                    .sum::<u64>()
            })
        };
        let defending: Vec<&UnitObs> = self
            .list
            .iter()
            .filter(|mission| matches!(mission.task, Task::Defend { .. }))
            .flat_map(|mission| &mission.units)
            .filter_map(|id| mine(observation, *id))
            .collect();
        let free: Vec<&UnitObs> = self
            .available(observation, false)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .filter(|unit| at_home(map, home, unit))
            .collect();
        let (defending, free) = (worth(&defending), worth(&free));
        Spare {
            home,
            left: std::array::from_fn(|index| {
                free[index].saturating_sub(reserve[index].saturating_sub(defending[index]))
            }),
        }
    }

    /// The home `payload` a lift could take, cut to what the reserve lets
    /// leave.
    pub(super) fn liftable(
        &self,
        observation: &ObservationData,
        map: &MapModel,
        reserve: [u64; 2],
        payload: u64,
    ) -> u64 {
        self.spare(observation, map, reserve).payload(payload)
    }
}

/// Whether `unit` counts as at home: in the air, or on the start's ground.
fn at_home(map: &MapModel, home: Option<u32>, unit: &UnitObs) -> bool {
    unit.kind.stats().domain == Domain::Air || (home.is_some() && map.component(unit.tile) == home)
}
