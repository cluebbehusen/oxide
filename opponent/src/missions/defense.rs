//! The defend mission: every threatened Foundry recruits free units that can
//! hit its threats and sends them at the threat nearest it, then lets them go
//! once the threat has been gone a while.

use super::{
    DefendPhase, MISSION_CAP, Mission, Missions, Task, UNIT_CAP, hits, hunt, insert, mine, value,
};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled, footprint_centre, gap, ring};
use crate::map::MapModel;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData, UnitObs};
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingKind, UnitId};

/// Empty tiles between an enemy and an own building inside which the enemy
/// threatens it, unless its weapon reaches further.
const THREAT_GAP: i32 = 8;

/// Ticks a defense waits without a threat before letting its units go.
const QUIET_TICKS: u64 = 120;

/// Ticks a defense may stay engaged before evaluation calls it stuck.
const ENGAGE_TICKS: u64 = 3_600;

/// Tiles a threat may move before its defenders are sent after it again.
const RETARGET_TILES: i32 = 3;

/// Ticks a defense's `phase` should end within.
pub(super) fn timeout(phase: DefendPhase) -> u64 {
    match phase {
        DefendPhase::Engage { .. } => ENGAGE_TICKS,
        DefendPhase::Recover => QUIET_TICKS,
    }
}

impl Missions {
    /// Answers every threatened Foundry: recruits free units that can hit its
    /// threats until, in each domain it is attacked from, they outweigh the
    /// attackers by half again, and sends them at the threat nearest the
    /// Foundry. A defense that is only recovering lends its units, as does an
    /// attack not yet fighting. Returns whether any defense stayed short.
    ///
    /// Only threats standing on or beside the Foundry's ground count. One
    /// across water or a chasm is left to production: chasing it would stall
    /// every defender, and a defense short forever would never save for the
    /// tech that answers it.
    pub(crate) fn defend(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        frame: HomeFrame,
        ledger: &mut Ledger,
    ) -> bool {
        let now = observation.tick;
        let groups = threats(observation, map, frame);
        for mission in &mut self.list {
            let Task::Defend { asset, phase } = &mut mission.task else {
                continue;
            };
            let threatened = groups.iter().any(|(foundry, _)| foundry.id == *asset);
            if let DefendPhase::Engage { focus } = *phase
                && !threatened
            {
                // A focus order chases its target; without a fresh Hunt the
                // members would follow a retreating enemy out of the base.
                if focus.is_some() {
                    ledger.order(hunt(mission.units.clone(), mission.goal));
                }
                *phase = DefendPhase::Recover;
                mission.since = now;
            }
        }
        self.list.retain(|mission| {
            !matches!(
                mission.task,
                Task::Defend {
                    phase: DefendPhase::Recover,
                    ..
                }
            ) || now < mission.since + QUIET_TICKS
        });

        let mut free = self.available(observation, true);
        let mut short = false;
        for (foundry, threats) in &groups {
            let centre = footprint_centre(foundry.kind, foundry.anchor);
            let nearest = |threats: &mut dyn Iterator<Item = &&UnitObs>| {
                threats
                    .min_by_key(|threat| frame.rank(centre, doubled(threat.tile)))
                    .map(|threat| threat.tile)
            };
            let component = map.component(foundry.anchor);
            let grounded = nearest(
                &mut threats
                    .iter()
                    .filter(|threat| threat.body_domain() == Domain::Ground),
            );
            let Some(goal) = grounded.or_else(|| {
                let flyer = nearest(&mut threats.iter())?;
                guard(observation, map, frame, component, flyer)
            }) else {
                continue;
            };
            let need = [Domain::Ground, Domain::Air].map(|domain| {
                threats
                    .iter()
                    .filter(|threat| threat.body_domain() == domain)
                    .map(|threat| value(threat))
                    .sum::<u64>()
                    * 3
                    / 2
            });
            let index = self.list.iter().position(
                |mission| matches!(mission.task, Task::Defend { asset, .. } if asset == foundry.id),
            );
            let members = index.map_or(&[][..], |index| &self.list[index].units);
            let mut have = [Domain::Ground, Domain::Air].map(|domain| {
                members
                    .iter()
                    .filter_map(|id| mine(observation, *id))
                    .filter(|unit| hits(unit, domain))
                    .map(value)
                    .sum::<u64>()
            });
            let room = UNIT_CAP - members.len();
            let mut candidates: Vec<&UnitObs> = free
                .iter()
                .filter_map(|id| mine(observation, *id))
                .filter(|unit| !members.contains(&unit.id))
                .filter(|unit| can_hit_any(unit, threats))
                .filter(|unit| {
                    unit.kind.stats().domain == Domain::Air || map.component(unit.tile) == component
                })
                .collect();
            candidates.sort_by_key(|unit| (frame.rank(centre, doubled(unit.tile)), unit.id));
            let wanted = |have: &[u64; 2], unit: &UnitObs| {
                [Domain::Ground, Domain::Air]
                    .into_iter()
                    .enumerate()
                    .any(|(index, domain)| have[index] < need[index] && hits(unit, domain))
            };
            let mut recruits = Vec::new();
            for unit in candidates {
                if recruits.len() == room || (have[0] >= need[0] && have[1] >= need[1]) {
                    break;
                }
                if !wanted(&have, unit) {
                    continue;
                }
                for (index, domain) in [Domain::Ground, Domain::Air].into_iter().enumerate() {
                    if hits(unit, domain) {
                        have[index] += value(unit);
                    }
                }
                recruits.push(unit.id);
            }
            short |= have[0] < need[0] || have[1] < need[1];

            let Some(index) = index else {
                if recruits.is_empty() || self.list.len() >= MISSION_CAP {
                    continue;
                }
                recruits.sort_unstable();
                if ledger.order(hunt(recruits.clone(), goal)) {
                    take(&mut free, &recruits);
                    self.release(&recruits);
                    self.list.push(Mission {
                        id: self.next,
                        since: now,
                        units: recruits,
                        goal,
                        task: Task::Defend {
                            asset: foundry.id,
                            phase: DefendPhase::Engage { focus: None },
                        },
                    });
                    self.next += 1;
                }
                continue;
            };
            let mission = &mut self.list[index];
            let Task::Defend { phase, .. } = &mut mission.task else {
                unreachable!("a defend mission's task");
            };
            let recovering = *phase == DefendPhase::Recover;
            // Chasing a threat off the Foundry's ground, such as a flyer over a
            // chasm, would stall every defender each time it moved.
            let resend = recovering
                || (mission.goal.chebyshev(goal) > RETARGET_TILES
                    && map.component(goal) == component);
            let mut sent = recruits.clone();
            if resend {
                sent.extend_from_slice(&mission.units);
            }
            if sent.is_empty() {
                continue;
            }
            sent.sort_unstable();
            if ledger.order(hunt(sent, goal)) {
                for id in &recruits {
                    insert(&mut mission.units, *id);
                }
                take(&mut free, &recruits);
                if resend {
                    mission.goal = goal;
                }
                if recovering {
                    *phase = DefendPhase::Engage { focus: None };
                    mission.since = now;
                }
                self.release_from_others(index, &recruits);
            }
        }
        short
    }
}

/// Removes `taken` from the sorted `free`.
fn take(free: &mut Vec<UnitId>, taken: &[UnitId]) {
    free.retain(|id| !taken.contains(id));
}

/// Visible enemies that could hit the seat's buildings, grouped by the built
/// Foundry each is nearest, home-nearest Foundry first. An enemy joins only
/// if it stands on or beside that Foundry's ground.
fn threats<'a>(
    observation: &'a ObservationData,
    map: &MapModel,
    frame: HomeFrame,
) -> Vec<(&'a BuildingObs, Vec<&'a UnitObs>)> {
    let mut foundries: Vec<&BuildingObs> = observation
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry && building.built)
        .collect();
    foundries.sort_by_key(|foundry| {
        (
            frame.rank(frame.home, footprint_centre(foundry.kind, foundry.anchor)),
            foundry.id,
        )
    });
    let mut groups: Vec<(&BuildingObs, Vec<&UnitObs>)> = foundries
        .iter()
        .map(|foundry| (*foundry, Vec::new()))
        .collect();
    for enemy in &observation.enemy_units {
        let Some(reach) = ground_reach(enemy) else {
            continue;
        };
        let near = observation.my_buildings.iter().any(|building| {
            gap(
                building.anchor,
                building.kind.base_stats().size,
                enemy.tile,
                (1, 1),
            ) < reach
        });
        if !near {
            continue;
        }
        let nearest = (0..foundries.len()).min_by_key(|index| {
            let foundry = foundries[*index];
            let size = foundry.kind.base_stats().size;
            (
                gap(foundry.anchor, size, enemy.tile, (1, 1)),
                frame.rank(
                    doubled(enemy.tile),
                    footprint_centre(foundry.kind, foundry.anchor),
                ),
            )
        });
        let Some(index) = nearest else {
            continue;
        };
        let reachable = map
            .component(foundries[index].anchor)
            .is_some_and(|component| {
                map.component(enemy.tile) == Some(component) || map.touches(enemy.tile, component)
            });
        if reachable {
            groups[index].1.push(enemy);
        }
    }
    groups.retain(|(_, threats)| !threats.is_empty());
    groups
}

/// Where ground defenders wait out an air raid: beside the seat's building
/// on the Foundry's ground nearest `flyer`, on the tile nearest it. Chasing a
/// flyer's shadow only sends them to tiles they cannot stand on.
fn guard(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    component: Option<u32>,
    flyer: TilePos,
) -> Option<TilePos> {
    component?;
    let building = observation
        .my_buildings
        .iter()
        .filter(|building| map.component(building.anchor) == component)
        .min_by_key(|building| {
            (
                gap(
                    building.anchor,
                    building.kind.base_stats().size,
                    flyer,
                    (1, 1),
                ),
                frame.rank(
                    doubled(flyer),
                    footprint_centre(building.kind, building.anchor),
                ),
            )
        })?;
    ring(building.anchor, building.kind.base_stats().size)
        .filter(|tile| map.component(*tile) == component)
        .min_by_key(|tile| frame.rank(doubled(flyer), doubled(*tile)))
}

/// Empty tiles inside which `enemy` threatens a building: the threat gap, or
/// its longest reach against ground if longer. `None` when it cannot hit
/// ground at all.
fn ground_reach(enemy: &UnitObs) -> Option<i32> {
    enemy
        .kind
        .stats()
        .weapons
        .iter()
        .filter(|weapon| weapon.targets.ground)
        .map(|weapon| weapon.range.ceil().to_num::<i32>().max(THREAT_GAP))
        .max()
}

/// Whether `unit` has a weapon for any of `threats`.
fn can_hit_any(unit: &UnitObs, threats: &[&UnitObs]) -> bool {
    unit.kind.stats().weapons.iter().any(|weapon| {
        threats
            .iter()
            .any(|threat| weapon.targets.covers(threat.body_domain()))
    })
}
