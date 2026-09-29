//! Missions: units the seat commands together for one purpose, in a phase
//! that persists between decisions so repeated scoring does not replan them.
//! Missions own only units that exist; production never works for one.
//!
//! Missions take units in a fixed order each decision: defense first, then
//! lift, then attacks, then scouting. Each takes from what
//! [`Missions::available`] leaves free when it runs.

use crate::frame::{HomeFrame, doubled, ring};
use crate::map::{MapModel, UNREACHABLE};
use chassis::grid::TilePos;
use oxide_sim::observation::{ObservationData, UnitObs};
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingId, BuildingKind, Command, PlayerId, UnitId};
use serde::{Deserialize, Serialize};

mod attack;
mod defense;
mod focus;
mod lift;
mod scouting;

pub(crate) use attack::minimum;
pub(crate) use lift::{carrier, needed as lift_needed, rides};
pub(crate) use scouting::points;

/// Missions the seat runs at once.
const MISSION_CAP: usize = 16;

/// Units one mission holds.
const UNIT_CAP: usize = 256;

/// The seat's missions, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Missions {
    /// The id the next mission takes.
    next: u64,
    list: Vec<Mission>,
    /// Since when an army that could attack has not, or production has sat
    /// idle, with no attack under way.
    waiting: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Mission {
    id: u64,
    /// Tick the current phase began.
    since: u64,
    /// Members, by id.
    units: Vec<UnitId>,
    /// Where the members were last sent.
    goal: TilePos,
    task: Task,
}

/// What a mission is doing, with the phases its kind can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "task", rename_all = "snake_case", deny_unknown_fields)]
enum Task {
    Defend {
        asset: BuildingId,
        phase: DefendPhase,
    },
    Attack {
        target: Objective,
        phase: AttackPhase,
    },
    Scout {
        point: u16,
    },
    Lift {
        target: Objective,
        phase: LiftPhase,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum LiftPhase {
    Load,
    Fly,
    Fight { focus: Option<UnitId> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum DefendPhase {
    Engage { focus: Option<UnitId> },
    Recover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum AttackPhase {
    Gather,
    Travel,
    Engage { focus: Option<UnitId> },
    Withdraw,
    Recover,
}

/// An enemy building a mission goes after.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Objective {
    owner: PlayerId,
    building: BuildingKind,
    anchor: TilePos,
}

/// What a mission is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "mission", rename_all = "snake_case")]
pub enum MissionKind {
    /// Answers enemies threatening the seat's buildings around a Foundry.
    Defend {
        /// The Foundry the threats are nearest.
        asset: BuildingId,
    },
    /// Takes an army to a known or presumed enemy building.
    Attack {
        /// The building's owner.
        owner: PlayerId,
        /// What it is.
        building: BuildingKind,
        /// Its footprint anchor.
        anchor: TilePos,
    },
    /// Looks at a place the seat has not seen for a while.
    Scout {
        /// The place, by its index among the seat's scouting points.
        point: u16,
    },
    /// Flies ground units to an enemy building no ground route reaches.
    Lift {
        /// The building's owner.
        owner: PlayerId,
        /// What it is.
        building: BuildingKind,
        /// Its footprint anchor.
        anchor: TilePos,
    },
}

/// Where a mission stands, as reports see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Assembling at a rally point.
    Gather,
    /// On the way to its target.
    Travel,
    /// Fighting what it was formed for.
    Engage,
    /// Pulling back from a fight it was losing.
    Withdraw,
    /// Its purpose is gone for now; it waits, then either resumes or lets its
    /// units go.
    Recover,
    /// Boarding carriers.
    Load,
    /// Carriers on their way to the landing.
    Fly,
    /// Landed units fighting.
    Fight,
}

/// One mission as a decision left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MissionStatus {
    /// Stable for the mission's life.
    pub id: u64,
    /// What it is for.
    pub kind: MissionKind,
    /// Where it stands.
    pub phase: Phase,
    /// Tick the phase began.
    pub since: u64,
    /// Ticks the phase should end within. A phase much older than this is a
    /// mission that failed to recover.
    pub timeout: u64,
    /// Units it holds.
    pub units: u32,
    /// Where its units were last sent.
    pub goal: TilePos,
}

impl MissionKind {
    /// A short name for reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Defend { .. } => "defend",
            Self::Attack { .. } => "attack",
            Self::Scout { .. } => "scout",
            Self::Lift { .. } => "lift",
        }
    }
}

impl Phase {
    /// A short name for reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Gather => "gather",
            Self::Travel => "travel",
            Self::Engage => "engage",
            Self::Withdraw => "withdraw",
            Self::Recover => "recover",
            Self::Load => "load",
            Self::Fly => "fly",
            Self::Fight => "fight",
        }
    }
}

impl Task {
    fn kind(self) -> MissionKind {
        match self {
            Self::Defend { asset, .. } => MissionKind::Defend { asset },
            Self::Attack { target, .. } => MissionKind::Attack {
                owner: target.owner,
                building: target.building,
                anchor: target.anchor,
            },
            Self::Scout { point } => MissionKind::Scout { point },
            Self::Lift { target, .. } => MissionKind::Lift {
                owner: target.owner,
                building: target.building,
                anchor: target.anchor,
            },
        }
    }

    fn phase(self) -> Phase {
        match self {
            Self::Defend { phase, .. } => match phase {
                DefendPhase::Engage { .. } => Phase::Engage,
                DefendPhase::Recover => Phase::Recover,
            },
            Self::Attack { phase, .. } => match phase {
                AttackPhase::Gather => Phase::Gather,
                AttackPhase::Travel => Phase::Travel,
                AttackPhase::Engage { .. } => Phase::Engage,
                AttackPhase::Withdraw => Phase::Withdraw,
                AttackPhase::Recover => Phase::Recover,
            },
            Self::Scout { .. } => Phase::Travel,
            Self::Lift { phase, .. } => match phase {
                LiftPhase::Load => Phase::Load,
                LiftPhase::Fly => Phase::Fly,
                LiftPhase::Fight { .. } => Phase::Fight,
            },
        }
    }

    /// Ticks the current phase should end within.
    fn timeout(self) -> u64 {
        match self {
            Self::Defend { phase, .. } => defense::timeout(phase),
            Self::Attack { phase, .. } => attack::timeout(phase),
            Self::Scout { .. } => scouting::TRAVEL_TICKS,
            Self::Lift { phase, .. } => lift::timeout(phase),
        }
    }

    /// The enemy an engaged mission focuses, if it is engaged.
    fn focus(&mut self) -> Option<&mut Option<UnitId>> {
        match self {
            Self::Defend {
                phase: DefendPhase::Engage { focus },
                ..
            }
            | Self::Attack {
                phase: AttackPhase::Engage { focus },
                ..
            }
            | Self::Lift {
                phase: LiftPhase::Fight { focus },
                ..
            } => Some(focus),
            _ => None,
        }
    }
}

impl Mission {
    /// Whether the mission keeps its units from a defense: everything but a
    /// recovering defense, and an attack only once it is fighting. A
    /// travelling attack may have met the enemy since the last decision. A
    /// scout keeps its scout, and a lift its units once it has left the
    /// ground.
    fn holds(&self, observation: &ObservationData) -> bool {
        match self.task {
            Task::Defend { phase, .. } => phase != DefendPhase::Recover,
            Task::Attack { phase, .. } => match phase {
                AttackPhase::Engage { .. } => true,
                AttackPhase::Travel => {
                    let members: Vec<&UnitObs> = self
                        .units
                        .iter()
                        .filter_map(|id| mine(observation, *id))
                        .collect();
                    attack::contact(observation, &members)
                }
                AttackPhase::Gather | AttackPhase::Withdraw | AttackPhase::Recover => false,
            },
            Task::Scout { .. } => true,
            Task::Lift { phase, .. } => phase != LiftPhase::Load,
        }
    }

    /// The target of an attack or lift lost while committed to it.
    fn lost_target(&self) -> Option<(BuildingKind, TilePos)> {
        match self.task {
            Task::Attack {
                target,
                phase: AttackPhase::Travel | AttackPhase::Engage { .. },
            }
            | Task::Lift {
                target,
                phase: LiftPhase::Fly | LiftPhase::Fight { .. },
            } => Some((target.building, target.anchor)),
            _ => None,
        }
    }
}

impl Missions {
    /// Every mission, by id.
    pub(crate) fn statuses(&self) -> Vec<MissionStatus> {
        self.list
            .iter()
            .map(|mission| MissionStatus {
                id: mission.id,
                kind: mission.task.kind(),
                phase: mission.task.phase(),
                since: mission.since,
                timeout: mission.task.timeout(),
                units: mission.units.len() as u32,
                goal: mission.goal,
            })
            .collect()
    }

    /// Drops members that are gone, and missions left without members or
    /// without the Foundry they defend. Units aboard a carrier are alive.
    /// Returns the targets of attacks and lifts wiped out while committed, so
    /// the seat tries something else for a while.
    pub(crate) fn prune(&mut self, observation: &ObservationData) -> Vec<(BuildingKind, TilePos)> {
        let mut carried: Vec<UnitId> = observation
            .my_carried_units
            .iter()
            .map(|unit| unit.id)
            .collect();
        carried.sort_unstable();
        let alive = |id: &UnitId| {
            observation
                .my_units
                .binary_search_by_key(id, |unit| unit.id)
                .is_ok()
                || carried.binary_search(id).is_ok()
        };
        for mission in &mut self.list {
            mission.units.retain(alive);
        }
        let lost = self
            .list
            .iter()
            .filter(|mission| mission.units.is_empty())
            .filter_map(Mission::lost_target)
            .collect();
        self.list.retain(|mission| {
            !mission.units.is_empty()
                && match mission.task {
                    Task::Defend { asset, .. } => observation
                        .my_buildings
                        .iter()
                        .any(|building| building.id == asset),
                    Task::Attack { .. } | Task::Scout { .. } | Task::Lift { .. } => true,
                }
        });
        lost
    }

    /// Units no mission holds, by id. A defense may also `borrow` units that
    /// a recovering defense or an attack out of contact would lend it.
    fn available(&self, observation: &ObservationData, borrow: bool) -> Vec<UnitId> {
        let mut owned: Vec<UnitId> = self
            .list
            .iter()
            .filter(|mission| !borrow || mission.holds(observation))
            .flat_map(|mission| mission.units.iter().copied())
            .collect();
        owned.sort_unstable();
        observation
            .my_units
            .iter()
            .map(|unit| unit.id)
            .filter(|id| owned.binary_search(id).is_err())
            .collect()
    }

    /// Takes `units` out of every mission, dropping missions left empty.
    fn release(&mut self, units: &[UnitId]) {
        for mission in &mut self.list {
            mission.units.retain(|id| !units.contains(id));
        }
        self.list.retain(|mission| !mission.units.is_empty());
    }

    /// Takes `units` out of every mission but the one at `keep`.
    fn release_from_others(&mut self, keep: usize, units: &[UnitId]) {
        let id = self.list[keep].id;
        for mission in self.list.iter_mut().filter(|mission| mission.id != id) {
            mission.units.retain(|unit| !units.contains(unit));
        }
        self.list.retain(|mission| !mission.units.is_empty());
    }

    /// Rejects restored missions that could not have been recorded by `now`
    /// on a map of the given size with `points` scouting points.
    pub(crate) fn validate(
        &self,
        now: u64,
        width: i32,
        height: i32,
        points: usize,
    ) -> Result<(), String> {
        if self.list.len() > MISSION_CAP {
            return Err("checkpoint holds too many missions".into());
        }
        let ordered = self.list.windows(2).all(|pair| pair[0].id < pair[1].id);
        // Missions form a few at a time, so a counter far ahead of the tick
        // was not recorded by this seat and could overflow.
        let exhausted = self.next > (now + 1).saturating_mul(MISSION_CAP as u64);
        if !ordered || exhausted || self.list.last().is_some_and(|last| last.id >= self.next) {
            return Err("checkpoint mission ids are out of order".into());
        }
        let count = |kind: fn(&Task) -> bool| {
            self.list
                .iter()
                .filter(|mission| kind(&mission.task))
                .count()
        };
        let attacks = count(|task| matches!(task, Task::Attack { .. }));
        let lifts = count(|task| matches!(task, Task::Lift { .. }));
        if attacks > 1 || lifts > 1 || self.waiting.is_some_and(|since| since > now) {
            return Err("checkpoint mission could not have been recorded".into());
        }
        let on_map = |tile: TilePos| (0..width).contains(&tile.x) && (0..height).contains(&tile.y);
        for mission in &self.list {
            let sorted = mission.units.windows(2).all(|pair| pair[0] < pair[1]);
            if !sorted || mission.units.is_empty() || mission.units.len() > UNIT_CAP {
                return Err("checkpoint mission units are malformed".into());
            }
            let target_on_map = match mission.task {
                Task::Attack { target, .. } | Task::Lift { target, .. } => on_map(target.anchor),
                Task::Scout { point } => usize::from(point) < points,
                Task::Defend { .. } => true,
            };
            if mission.since > now || !on_map(mission.goal) || !target_on_map {
                return Err("checkpoint mission could not have been recorded".into());
            }
        }
        let mut owned: Vec<UnitId> = self
            .list
            .iter()
            .flat_map(|mission| mission.units.iter().copied())
            .collect();
        owned.sort_unstable();
        if owned.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("checkpoint missions share a unit".into());
        }
        Ok(())
    }
}

/// Whether `unit` has a weapon for `domain`.
fn hits(unit: &UnitObs, domain: Domain) -> bool {
    unit.kind
        .stats()
        .weapons
        .iter()
        .any(|weapon| weapon.targets.covers(domain))
}

/// A unit's price, discounted by its missing health.
fn value(unit: &UnitObs) -> u64 {
    let stats = unit.kind.stats();
    u64::from(stats.cost) * u64::from(unit.hp) / u64::from(stats.max_hp.max(1))
}

fn mine(observation: &ObservationData, id: UnitId) -> Option<&UnitObs> {
    observation
        .my_units
        .binary_search_by_key(&id, |unit| unit.id)
        .ok()
        .map(|index| &observation.my_units[index])
}

fn insert(ids: &mut Vec<UnitId>, id: UnitId) {
    if let Err(index) = ids.binary_search(&id) {
        ids.insert(index, id);
    }
}

/// The tile beside the footprint of `building` at `anchor` nearest the
/// seat's start by ground, or `None` if no ground route reaches it.
fn approach(
    map: &MapModel,
    me: PlayerId,
    frame: HomeFrame,
    building: BuildingKind,
    anchor: TilePos,
) -> Option<TilePos> {
    ring(anchor, building.base_stats().size)
        .filter(|tile| map.distance(me, *tile) != UNREACHABLE)
        .min_by_key(|tile| {
            (
                map.distance(me, *tile),
                frame.rank(frame.home, doubled(*tile)),
            )
        })
}

fn run(units: Vec<UnitId>, goal: TilePos) -> Command {
    Command::Run {
        units,
        goal,
        queue: false,
    }
}

fn hunt(units: Vec<UnitId>, goal: TilePos) -> Command {
    Command::Hunt {
        units,
        goal,
        queue: false,
    }
}
