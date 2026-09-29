//! Raids: a few free raiders of one kind go after an enemy harvest line or a
//! lightly defended building while no defense is under way, and come back
//! before they are caught. Scuttlers, and ground-attack aircraft too few for
//! a strike, harry a harvest line: an enemy Extractor, or a Foundry its
//! Harvesters haul to. Sappers blow up a valuable building. A raided target
//! is skipped for a while, which spaces the raids out.

use super::air::{self, Hazard};
use super::attack::{FIT, defense, healthy, minimum, striking};
use super::{
    MISSION_CAP, Mission, Missions, Objective, RaidPhase, Task, approach, hunt, mine, run,
    standing, value,
};
use crate::composition::{self, Role};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, centre_distance, doubled, footprint_centre, ring};
use crate::map::MapModel;
use crate::memory::Memory;
use crate::profile::ResolvedProfile;
use chassis::grid::TilePos;

use oxide_sim::observation::{BuildingObs, ObservationData, UnitObs};
use oxide_sim::stats::Domain;
use oxide_sim::{AttackTarget, BuildingKind, Command, RememberedBuilding, UnitId, UnitKind};
use std::cmp::Reverse;

/// Raiders a raid starts with, at least. A Scuttler or bomber raid turns
/// back once it has fewer.
const RAIDERS: usize = 2;

/// Scuttlers or Sappers one raid takes, at most.
const RAIDER_CAP: usize = 4;

/// Health, per mille, under which a Scuttler or bomber raid turns back.
const WOUNDED: u32 = 500;

/// Known defense, in scrap, around a building Sappers go after, at most.
const LIGHT_DEFENSE: u64 = 200;

/// Tiles between a raider and its goal at which the raid strikes.
const CONTACT_TILES: i32 = 6;

const TRAVEL_TICKS: u64 = 2_400;
const STRIKE_TICKS: u64 = 1_200;
const WITHDRAW_TICKS: u64 = 1_200;

/// Ticks a raid's `phase` should end within.
pub(super) fn timeout(phase: RaidPhase) -> u64 {
    match phase {
        RaidPhase::Travel => TRAVEL_TICKS,
        RaidPhase::Strike => STRIKE_TICKS,
        RaidPhase::Withdraw => WITHDRAW_TICKS,
    }
}

/// The raids a unit can join.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Raider {
    Sapper,
    Scuttler,
    Bomber,
}

fn raider(kind: UnitKind) -> Option<Raider> {
    match kind {
        UnitKind::Sapper => Some(Raider::Sapper),
        UnitKind::Scuttler => Some(Raider::Scuttler),
        kind if composition::role(kind) == Some(Role::AirStrike) => Some(Raider::Bomber),
        _ => None,
    }
}

/// What one decision knows about raiding.
struct Foray<'a> {
    observation: &'a ObservationData,
    map: &'a MapModel,
    frame: HomeFrame,
    memory: &'a Memory,
    /// Known fire against ground units, and against aircraft.
    ground: Vec<Hazard>,
    air: Vec<Hazard>,
}

impl Missions {
    /// Sends free raiders at an enemy harvest line or lightly defended
    /// building while no defense is under way, or advances the raid under way.
    /// Returns a target the raid is done with, so the seat leaves it for a
    /// while.
    pub(crate) fn raid(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        frame: HomeFrame,
        profile: &ResolvedProfile,
        memory: &Memory,
        ledger: &mut Ledger,
    ) -> Option<(BuildingKind, TilePos)> {
        let raiding = self
            .list
            .iter()
            .any(|mission| matches!(mission.task, Task::Raid { .. }));
        let free = self.available(observation, false);
        let ready = [Raider::Sapper, Raider::Scuttler, Raider::Bomber]
            .into_iter()
            .any(|kind| {
                free.iter()
                    .filter_map(|id| mine(observation, *id))
                    .filter(|unit| raider(unit.kind) == Some(kind))
                    .count()
                    >= RAIDERS
            });
        if !raiding && !ready {
            return None;
        }
        let foray = Foray {
            observation,
            map,
            frame,
            memory,
            ground: air::hazards(observation, memory, Domain::Ground),
            air: air::hazards(observation, memory, Domain::Air),
        };
        match self
            .list
            .iter()
            .position(|mission| matches!(mission.task, Task::Raid { .. }))
        {
            None => {
                self.form_raid(&foray, minimum(profile.stance), ledger);
                None
            }
            Some(index) => self.advance_raid(index, &foray, ledger),
        }
    }

    fn form_raid(&mut self, foray: &Foray<'_>, minimum: u64, ledger: &mut Ledger) {
        let observation = foray.observation;
        let defending = self
            .list
            .iter()
            .any(|mission| matches!(mission.task, Task::Defend { .. }));
        if self.list.len() >= MISSION_CAP || defending {
            return;
        }
        let free: Vec<&UnitObs> = self
            .available(observation, false)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .filter(|unit| unit.idle && healthy(unit, FIT))
            .collect();
        for kind in [Raider::Sapper, Raider::Scuttler, Raider::Bomber] {
            let squad: Vec<&UnitObs> = free
                .iter()
                .copied()
                .filter(|unit| raider(unit.kind) == Some(kind))
                .collect();
            let strength: u64 = squad.iter().map(|unit| value(unit)).sum();
            // Bombers enough for a strike are the strike's.
            let striking: u64 = squad.iter().map(|unit| striking(unit)).sum();
            if squad.len() < RAIDERS || (kind == Raider::Bomber && striking >= minimum) {
                continue;
            }
            let Some((target, goal)) = foray.target(kind, strength) else {
                continue;
            };
            let mut squad = squad;
            squad
                .sort_by_key(|unit| (foray.frame.rank(doubled(goal), doubled(unit.tile)), unit.id));
            if kind != Raider::Bomber {
                squad.truncate(RAIDER_CAP);
            }
            let mut units: Vec<UnitId> = squad.iter().map(|unit| unit.id).collect();
            units.sort_unstable();
            let sent = match kind {
                Raider::Sapper => foray
                    .demolish(target)
                    .is_some_and(|aim| ledger.order(blast(units.clone(), aim))),
                Raider::Scuttler => ledger.order(hunt(units.clone(), goal)),
                Raider::Bomber => foray.fly(units.clone(), goal, ledger),
            };
            if sent {
                self.list.push(Mission {
                    id: self.next,
                    since: observation.tick,
                    units,
                    goal,
                    task: Task::Raid {
                        target,
                        phase: RaidPhase::Travel,
                    },
                });
                self.next += 1;
            }
            return;
        }
    }

    /// Moves the raid at `index` through its phases. Returns its target once
    /// it turns back.
    fn advance_raid(
        &mut self,
        index: usize,
        foray: &Foray<'_>,
        ledger: &mut Ledger,
    ) -> Option<(BuildingKind, TilePos)> {
        let observation = foray.observation;
        let now = observation.tick;
        let mission = &self.list[index];
        let Task::Raid { target, phase } = mission.task else {
            unreachable!("the raid mission's task");
        };
        let members: Vec<&UnitObs> = mission
            .units
            .iter()
            .filter_map(|id| mine(observation, *id))
            .collect();
        let kind = members.iter().find_map(|unit| raider(unit.kind))?;
        let all_idle = members.iter().all(|unit| unit.idle);
        let age = now - mission.since;
        let goal = mission.goal;
        let units = mission.units.clone();
        let centre = footprint_centre(target.building, target.anchor);
        let home = air::pad(observation, foray.map, foray.frame, centre)?;
        // Sappers spend themselves, so only a raid of fighters counts its
        // losses and wounds.
        let spent = kind != Raider::Sapper
            && (members.len() < RAIDERS || members.iter().any(|unit| !healthy(unit, WOUNDED)));
        let strength: u64 = members.iter().map(|unit| value(unit)).sum();
        let outweighed = foray.opposition(kind, &members) > strength;
        let lost = (target.building, target.anchor);
        let withdraw = |missions: &mut Self, ledger: &mut Ledger| {
            let turned = ledger.order(run(units.clone(), home));
            if turned {
                let mission = &mut missions.list[index];
                mission.task = Task::Raid {
                    target,
                    phase: RaidPhase::Withdraw,
                };
                mission.since = now;
                mission.goal = home;
            }
            turned
        };
        match phase {
            RaidPhase::Travel => {
                if spent || outweighed || age >= TRAVEL_TICKS || !standing(observation, target) {
                    return withdraw(self, ledger).then_some(lost);
                }
                let arrived = members
                    .iter()
                    .any(|unit| unit.tile.chebyshev(goal) <= CONTACT_TILES);
                if arrived {
                    let mission = &mut self.list[index];
                    mission.task = Task::Raid {
                        target,
                        phase: RaidPhase::Strike,
                    };
                    mission.since = now;
                }
                None
            }
            RaidPhase::Strike => {
                let done = all_idle || age >= STRIKE_TICKS || !standing(observation, target);
                if spent || outweighed || done {
                    return withdraw(self, ledger).then_some(lost);
                }
                None
            }
            RaidPhase::Withdraw => {
                if all_idle || age >= WITHDRAW_TICKS {
                    self.list.remove(index);
                }
                None
            }
        }
    }
}

impl Foray<'_> {
    /// Where raiders of `kind` worth `strength` go, and the tile they aim
    /// for: for Sappers the most valuable known enemy building for its
    /// distance with little known defense, for the others the enemy
    /// Extractor or Foundry with the least known defense, nearest first.
    /// Targets recently given up are skipped, and ground raiders need a
    /// ground route.
    fn target(&self, kind: Raider, strength: u64) -> Option<(Objective, TilePos)> {
        let observation = self.observation;
        let now = observation.tick;
        let start = self.map.start(observation.me)?;
        let home = footprint_centre(BuildingKind::Foundry, start);
        let candidates = observation
            .enemy_buildings
            .iter()
            .filter(|building| building.built)
            .map(|building| Objective {
                owner: building.player,
                building: building.kind,
                anchor: building.anchor,
            })
            .filter(|target| !self.memory.abandoned(target.building, target.anchor, now))
            .filter_map(|target| {
                let goal = match kind {
                    Raider::Bomber => {
                        let size = target.building.base_stats().size;
                        ring(target.anchor, size).min_by_key(|tile| {
                            (
                                centre_distance(home, doubled(*tile)),
                                self.frame.rank(self.frame.home, doubled(*tile)),
                            )
                        })?
                    }
                    Raider::Sapper | Raider::Scuttler => approach(
                        self.map,
                        observation.me,
                        self.frame,
                        target.building,
                        target.anchor,
                    )?,
                };
                Some((target, goal))
            });
        let distance = |target: &Objective| {
            centre_distance(home, footprint_centre(target.building, target.anchor))
        };
        let rank = |target: &Objective| {
            self.frame.rank(
                self.frame.home,
                footprint_centre(target.building, target.anchor),
            )
        };
        match kind {
            Raider::Sapper => candidates
                .filter(|(_, goal)| defense(observation, self.memory, *goal) <= LIGHT_DEFENSE)
                .max_by_key(|(target, _)| {
                    let cost = target
                        .building
                        .base_stats()
                        .construction
                        .as_ref()
                        .map_or(0, |construction| construction.cost);
                    (
                        u64::from(cost) * 1_000 / (100 + distance(target)),
                        Reverse(rank(target)),
                    )
                }),
            Raider::Scuttler | Raider::Bomber => candidates
                .filter(|(target, _)| {
                    matches!(
                        target.building,
                        BuildingKind::Extractor | BuildingKind::Foundry
                    )
                })
                .filter_map(|(target, goal)| {
                    let guarded = match kind {
                        Raider::Bomber => {
                            let centre = footprint_centre(target.building, target.anchor);
                            self.air
                                .iter()
                                .filter(|hazard| hazard.covers(centre))
                                .map(|hazard| hazard.value)
                                .sum()
                        }
                        _ => defense(observation, self.memory, goal),
                    };
                    (guarded < strength).then_some((guarded, target, goal))
                })
                .min_by_key(|(guarded, target, _)| (*guarded, distance(target), rank(target)))
                .map(|(_, target, goal)| (target, goal)),
        }
    }

    /// What a Sapper aims at to blow up `target`.
    fn demolish(&self, target: Objective) -> Option<AttackTarget> {
        self.observation
            .enemy_buildings
            .iter()
            .find(|building| {
                (building.player, building.kind, building.anchor)
                    == (target.owner, target.building, target.anchor)
            })
            .map(aim)
    }

    /// Sends aircraft at `goal`, around known anti-air when the straight line
    /// crosses it. Returns whether they went.
    fn fly(&self, units: Vec<UnitId>, goal: TilePos, ledger: &mut Ledger) -> bool {
        let Some(pad) = air::pad(self.observation, self.map, self.frame, doubled(goal)) else {
            return false;
        };
        let Some(via) = air::route(self.observation, self.frame, &self.air, pad, goal) else {
            return ledger.order(hunt(units, goal));
        };
        if ledger.room() < 2 {
            return false;
        }
        ledger.order(run(units.clone(), via));
        ledger.order(Command::Hunt {
            units,
            goal,
            queue: true,
        });
        true
    }

    /// Value of the known fire reaching any member.
    fn opposition(&self, kind: Raider, members: &[&UnitObs]) -> u64 {
        let hazards = match kind {
            Raider::Bomber => &self.air,
            Raider::Sapper | Raider::Scuttler => &self.ground,
        };
        hazards
            .iter()
            .filter(|hazard| members.iter().any(|unit| hazard.covers(doubled(unit.tile))))
            .map(|hazard| hazard.value)
            .sum()
    }
}

/// What to attack to hit `building`: the building in sight, or the
/// footprint the seat remembers.
pub(super) fn aim(building: &BuildingObs) -> AttackTarget {
    if building.seen {
        AttackTarget::Building(building.id)
    } else {
        AttackTarget::RememberedBuilding(RememberedBuilding {
            owner: building.player,
            building_kind: building.kind,
            anchor: building.anchor,
        })
    }
}

pub(super) fn blast(units: Vec<UnitId>, target: AttackTarget) -> Command {
    Command::Attack {
        units,
        target,
        queue: false,
    }
}
