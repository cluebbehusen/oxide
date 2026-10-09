//! Raids: a few free raiders of one kind go after an enemy harvest line or a
//! lightly defended building while no defense is under way, and come back
//! before they are caught. Scuttlers, and ground-attack aircraft too few for
//! a strike, harry a harvest line: an enemy Extractor, or a Foundry its
//! Harvesters haul to. Sappers blow up a valuable building. A raided target
//! is skipped for a while, which spaces the raids out.

use super::Scratch;
use super::air::{self, Hazard};
use super::attack::{FIT, defense, fortified, healthy, margin, minimum, striking};
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
use oxide_sim::scenario::BotStance;
use oxide_sim::{AttackTarget, BuildingKind, Command, RememberedBuilding, UnitId, UnitKind};
use std::cmp::Reverse;

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
    ground: &'a [Hazard],
    air: &'a [Hazard],
    /// What offense leaves at home.
    reserve: [u64; 2],
    /// Targets another raid holds or this decision is done with.
    held: Vec<Objective>,
    /// Per mille of a target's known guard a raid must bring.
    margin: u64,
    /// The most known guard a Scuttler raid takes on: a line guarded beyond
    /// it is an attack's work, not a raid's.
    light: u64,
}

/// The most known guard a Scuttler raid takes on, as a share of the
/// stance's attack minimum: a stance limit on what counts as lightly
/// defended. Scuttlers sent at a real guard die before they reach the
/// workers.
fn light(stance: BotStance) -> u64 {
    minimum(stance) / 4
}

impl Missions {
    /// Advances every raid under way, then, while no defense is under way,
    /// sends more free raiders at enemy harvest lines or lightly defended
    /// buildings no raid holds. Returns the targets raids are done with, so
    /// the seat leaves them for a while.
    pub(crate) fn raid(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        profile: &ResolvedProfile,
        memory: &Memory,
        scratch: &Scratch,
        ledger: &mut Ledger,
    ) -> Vec<(BuildingKind, TilePos)> {
        let frame = scratch.frame;
        let raiding = self
            .list
            .iter()
            .any(|mission| matches!(mission.task, Task::Raid { .. }));
        let ready = self
            .free(observation, ledger)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .any(|unit| raider(unit.kind).is_some());
        if !raiding && !ready {
            return Vec::new();
        }
        let raiding_kind: fn(&Task) -> bool = |task| matches!(task, Task::Raid { .. });
        let mut foray = Foray {
            observation,
            map,
            frame,
            memory,
            ground: &scratch.ground,
            air: &scratch.air,
            reserve: scratch.reserve,
            held: Vec::new(),
            margin: margin(profile.difficulty),
            light: light(profile.stance),
        };
        let mut done: Vec<Objective> = Vec::new();
        for id in self.ids(raiding_kind) {
            let Some(index) = self.index_of(id) else {
                continue;
            };
            foray.held = self.held(raiding_kind, Some(id), &done);
            if let Some(target) = self.advance_raid(index, &foray, ledger) {
                done.push(target);
            }
        }
        loop {
            foray.held = self.held(raiding_kind, None, &done);
            if !self.form_raid(&foray, minimum(profile.stance), ledger) {
                break;
            }
        }
        done.into_iter()
            .map(|target| (target.building, target.anchor))
            .collect()
    }

    /// Scuttlers a raid on the least guarded known harvest line not raided
    /// lately and not under known guns needs: its known guard by the margin,
    /// at least one. Zero while no such line is known.
    pub(crate) fn raid_squad(
        observation: &ObservationData,
        map: &MapModel,
        profile: &ResolvedProfile,
        memory: &Memory,
        scratch: &Scratch,
    ) -> usize {
        let foray = Foray {
            observation,
            map,
            frame: scratch.frame,
            memory,
            ground: &scratch.ground,
            air: &scratch.air,
            reserve: scratch.reserve,
            held: Vec::new(),
            margin: margin(profile.difficulty),
            light: light(profile.stance),
        };
        foray
            .target(Raider::Scuttler, u64::MAX)
            .map_or(0, |(_, _, need)| {
                usize::try_from(need.div_ceil(u64::from(UnitKind::Scuttler.stats().cost)))
                    .unwrap_or(usize::MAX)
            })
    }

    /// Sends the first raider kind with a squad beyond the reserve at a target
    /// no raid holds. Returns whether a raid formed.
    fn form_raid(&mut self, foray: &Foray<'_>, minimum: u64, ledger: &mut Ledger) -> bool {
        let observation = foray.observation;
        let defending = self
            .list
            .iter()
            .any(|mission| matches!(mission.task, Task::Defend { .. }));
        if self.list.len() >= MISSION_CAP || defending {
            return false;
        }
        let free: Vec<&UnitObs> = self
            .free(observation, ledger)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .filter(|unit| unit.idle && healthy(unit, FIT))
            .collect();
        let spare = self.spare(observation, foray.map, foray.reserve);
        for kind in [Raider::Sapper, Raider::Scuttler, Raider::Bomber] {
            let squad: Vec<&UnitObs> = free
                .iter()
                .copied()
                .filter(|unit| raider(unit.kind) == Some(kind))
                .collect();
            let squad = spare.clone().outermost(foray.map, foray.frame, squad);
            let strength: u64 = squad.iter().map(|unit| value(unit)).sum();
            // Bombers enough for a strike are the strike's.
            let striking: u64 = squad.iter().map(|unit| striking(unit)).sum();
            if squad.is_empty() || (kind == Raider::Bomber && striking >= minimum) {
                continue;
            }
            let Some((target, goal, need)) = foray.target(kind, strength) else {
                continue;
            };
            // The raiders nearest the goal until they are worth its need.
            let mut squad = squad;
            squad
                .sort_by_key(|unit| (foray.frame.rank(doubled(goal), doubled(unit.tile)), unit.id));
            let mut have = 0;
            let mut units: Vec<UnitId> = Vec::new();
            for unit in squad {
                if have >= need {
                    break;
                }
                have += value(unit);
                units.push(unit.id);
            }
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
            return sent;
        }
        false
    }

    /// Moves the raid at `index` through its phases. Returns its target once
    /// it turns back.
    fn advance_raid(
        &mut self,
        index: usize,
        foray: &Foray<'_>,
        ledger: &mut Ledger,
    ) -> Option<Objective> {
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
        let spent = kind != Raider::Sapper && members.iter().any(|unit| !healthy(unit, WOUNDED));
        let strength: u64 = members.iter().map(|unit| value(unit)).sum();
        let outweighed = foray.opposition(kind, &members) > strength;
        let lost = target;
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
                // Sappers at the target blow it up whatever stands there.
                let outweighed = outweighed && kind != Raider::Sapper;
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
    /// Scuttlers leave lines that known enemy guns cover. Targets recently
    /// raided are skipped, and ground raiders need a ground route.
    fn target(&self, kind: Raider, strength: u64) -> Option<(Objective, TilePos, u64)> {
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
            .filter(|target| !self.held.contains(target))
            .filter(|target| !self.memory.raided(target.building, target.anchor, now))
            .filter_map(|target| {
                let goal = match kind {
                    Raider::Bomber => {
                        let size = target.building.size();
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
                .filter_map(|(target, goal)| {
                    // Enough blasts for what is left of the building.
                    let hp = observation
                        .enemy_buildings
                        .iter()
                        .find(|building| {
                            (building.player, building.kind, building.anchor)
                                == (target.owner, target.building, target.anchor)
                        })
                        .map_or(1, |building| building.hp);
                    let sapper = UnitKind::Sapper.stats();
                    let blast = sapper
                        .demolition
                        .map_or(1, |charge| charge.structure_damage);
                    let blasts = u64::from(hp.div_ceil(blast).max(1));
                    let need = blasts * u64::from(sapper.cost);
                    (need <= strength).then_some((target, goal, need))
                })
                .max_by_key(|(target, _, _)| {
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
                // A line under known guns is an attack's work, not a raid's.
                .filter(|(_, goal)| kind != Raider::Scuttler || !fortified(observation, *goal))
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
                    let need = (guarded * self.margin / 1_000).max(1);
                    (need <= strength && (kind != Raider::Scuttler || guarded <= self.light))
                        .then_some((guarded, target, goal, need))
                })
                .min_by_key(|(guarded, target, _, _)| (*guarded, distance(target), rank(target)))
                .map(|(_, target, goal, need)| (target, goal, need)),
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
        let Some(via) = air::route(self.observation, self.frame, self.air, pad, goal) else {
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
