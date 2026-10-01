//! The strike mission: free ground-attack aircraft hit a known or presumed
//! enemy building whose known anti-air they outweigh, gathering beside home,
//! flying around known anti-air, and withdrawing when it outweighs them.
//! Ground need not reach the target.

use super::Scratch;
use super::air::{self, Hazard};
use super::attack::{FIT, healthy, margin, minimum, recruit, striking};
use super::{
    MISSION_CAP, Mission, Missions, Objective, StrikePhase, Task, UNIT_CAP, hunt, mine, run,
    standing,
};
use crate::composition::{self, Role};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, centre_distance, doubled, footprint_centre, ring};
use crate::map::MapModel;
use crate::memory::Memory;
use crate::profile::ResolvedProfile;
use chassis::grid::TilePos;
use oxide_sim::observation::{ObservationData, UnitObs};
use oxide_sim::{BuildingKind, UnitKind};

/// Tiles between a member and the target at which the fight begins.
const CONTACT_TILES: i32 = 6;

const GATHER_TICKS: u64 = 600;
const TRAVEL_TICKS: u64 = 1_800;
const ENGAGE_TICKS: u64 = 2_400;
const WITHDRAW_TICKS: u64 = 1_200;

/// Ticks a strike's `phase` should end within.
pub(super) fn timeout(phase: StrikePhase) -> u64 {
    match phase {
        StrikePhase::Gather => GATHER_TICKS,
        StrikePhase::Travel => TRAVEL_TICKS,
        StrikePhase::Engage { .. } => ENGAGE_TICKS,
        StrikePhase::Withdraw => WITHDRAW_TICKS,
    }
}

/// What one decision knows about striking.
struct Raid<'a> {
    observation: &'a ObservationData,
    map: &'a MapModel,
    frame: HomeFrame,
    memory: &'a Memory,
    minimum: u64,
    margin: u64,
    /// Known fire against aircraft.
    air: &'a [Hazard],
    objectives: &'a [Objective],
    /// What offense leaves at home.
    reserve: [u64; 2],
    /// The enemy to go after first, when there are several.
    rival: Option<oxide_sim::PlayerId>,
    /// Targets another strike holds or this decision gave up on.
    held: Vec<Objective>,
}

impl Missions {
    /// Advances every strike under way, then sends more free ground-attack
    /// aircraft beyond the home reserve at a target no strike holds whose
    /// known anti-air they outweigh. Returns the targets given up on.
    pub(crate) fn strike(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        profile: &ResolvedProfile,
        memory: &Memory,
        scratch: &Scratch,
        ledger: &mut Ledger,
    ) -> Vec<(BuildingKind, TilePos)> {
        let frame = scratch.frame;
        let striking_kind: fn(&Task) -> bool = |task| matches!(task, Task::Strike { .. });
        let mut raid = Raid {
            observation,
            map,
            frame,
            memory,
            minimum: minimum(profile.stance),
            margin: margin(profile.difficulty),
            air: &scratch.air,
            objectives: &scratch.objectives,
            reserve: scratch.reserve,
            rival: scratch.rival,
            held: Vec::new(),
        };
        let mut given_up: Vec<Objective> = Vec::new();
        for id in self.ids(striking_kind) {
            let Some(index) = self.index_of(id) else {
                continue;
            };
            raid.held = self.held(striking_kind, Some(id), &given_up);
            if let Some(target) = self.advance_strike(index, &raid, ledger) {
                given_up.push(target);
            }
        }
        loop {
            raid.held = self.held(striking_kind, None, &given_up);
            if !self.form_strike(&raid, ledger) {
                break;
            }
        }
        given_up
            .into_iter()
            .map(|target| (target.building, target.anchor))
            .collect()
    }

    /// Sends the free aircraft beyond the reserve that the best target no
    /// strike holds needs. Returns whether a strike formed.
    fn form_strike(&mut self, raid: &Raid<'_>, ledger: &mut Ledger) -> bool {
        let observation = raid.observation;
        if self.list.len() >= MISSION_CAP {
            return false;
        }
        let fit: Vec<&UnitObs> = self
            .free(observation, ledger)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .filter(|unit| bomber(unit.kind) && healthy(unit, FIT))
            .collect();
        let fit = self
            .spare(observation, raid.map, raid.reserve)
            .outermost(raid.map, raid.frame, fit);
        let strength: u64 = fit.iter().map(|unit| striking(unit)).sum();
        if strength < raid.minimum {
            return false;
        }
        let Some(target) = raid.best(strength, None) else {
            return false;
        };
        let centre = footprint_centre(target.building, target.anchor);
        let Some(rally) = air::pad(observation, raid.map, raid.frame, centre) else {
            return false;
        };
        let recruits = recruit(raid.frame, &fit, rally, raid.need(target), UNIT_CAP);
        if recruits.is_empty() || !ledger.order(run(recruits.clone(), rally)) {
            return false;
        }
        self.list.push(Mission {
            id: self.next,
            since: observation.tick,
            units: recruits,
            goal: rally,
            task: Task::Strike {
                target,
                phase: StrikePhase::Gather,
            },
        });
        self.next += 1;
        true
    }

    /// Moves the strike at `index` through its phases. Returns its target
    /// when it gave up on it.
    fn advance_strike(
        &mut self,
        index: usize,
        raid: &Raid<'_>,
        ledger: &mut Ledger,
    ) -> Option<Objective> {
        let observation = raid.observation;
        let now = observation.tick;
        let mission = &self.list[index];
        let Task::Strike { target, phase } = mission.task else {
            unreachable!("the strike mission's task");
        };
        let members: Vec<&UnitObs> = mission
            .units
            .iter()
            .filter_map(|id| mine(observation, *id))
            .collect();
        let all_idle = members.iter().all(|unit| unit.idle);
        let strength: u64 = members.iter().map(|unit| striking(unit)).sum();
        let age = now - mission.since;
        let centre = footprint_centre(target.building, target.anchor);
        let rally = air::pad(observation, raid.map, raid.frame, centre)?;
        let goal = mission.goal;
        let units = mission.units.clone();
        let lost = target;
        // Gives the target up only once the strike has turned back, so a
        // decision out of orders does not keep refreshing the give-up.
        let withdraw = |missions: &mut Self, ledger: &mut Ledger| {
            let turned = ledger.order(run(units.clone(), rally));
            if turned {
                missions.list[index].set_strike(target, StrikePhase::Withdraw, now, rally);
            }
            turned
        };

        match phase {
            StrikePhase::Gather => {
                if !all_idle && age < GATHER_TICKS {
                    return None;
                }
                if strength < raid.need(target) || !standing(observation, target) {
                    self.list.remove(index);
                    return None;
                }
                if let Some(aim) = self.sortie(index, raid, rally, target, ledger) {
                    self.list[index].set_strike(target, StrikePhase::Travel, now, aim);
                }
                None
            }
            StrikePhase::Travel => {
                let arrived = members
                    .iter()
                    .any(|unit| unit.tile.chebyshev(goal) <= CONTACT_TILES);
                if arrived {
                    let engage = StrikePhase::Engage { focus: None };
                    self.list[index].set_strike(target, engage, now, goal);
                    return None;
                }
                if all_idle || age >= TRAVEL_TICKS || raid.opposition(&members) > strength {
                    return withdraw(self, ledger).then_some(lost);
                }
                None
            }
            StrikePhase::Engage { .. } => {
                if raid.opposition(&members) > strength {
                    return withdraw(self, ledger).then_some(lost);
                }
                if !all_idle && age < ENGAGE_TICKS {
                    return None;
                }
                if standing(observation, target) {
                    return withdraw(self, ledger).then_some(lost);
                }
                match raid.best(strength, Some(target)) {
                    Some(next) => {
                        if let Some(aim) = self.sortie(index, raid, goal, next, ledger) {
                            self.list[index].set_strike(next, StrikePhase::Travel, now, aim);
                        }
                    }
                    None => {
                        withdraw(self, ledger);
                    }
                }
                None
            }
            StrikePhase::Withdraw => {
                if all_idle || age >= WITHDRAW_TICKS {
                    self.list.remove(index);
                }
                None
            }
        }
    }

    /// Sends the strike at `index` from `from` at the side of `target`
    /// nearest it, around known anti-air when the straight line crosses it.
    /// Returns where it went.
    fn sortie(
        &mut self,
        index: usize,
        raid: &Raid<'_>,
        from: TilePos,
        target: Objective,
        ledger: &mut Ledger,
    ) -> Option<TilePos> {
        let units = self.list[index].units.clone();
        let aim = ring(target.anchor, target.building.base_stats().size).min_by_key(|tile| {
            (
                tile.chebyshev(from),
                raid.frame.rank(raid.frame.home, doubled(*tile)),
            )
        })?;
        let Some(via) = air::route(raid.observation, raid.frame, raid.air, from, aim) else {
            return ledger.order(hunt(units, aim)).then_some(aim);
        };
        if ledger.room() < 2 {
            return None;
        }
        ledger.order(run(units.clone(), via));
        ledger.order(super::Command::Hunt {
            units,
            goal: aim,
            queue: true,
        });
        Some(aim)
    }
}

impl Mission {
    fn set_strike(&mut self, target: Objective, phase: StrikePhase, now: u64, goal: TilePos) {
        self.task = Task::Strike { target, phase };
        self.since = now;
        self.goal = goal;
    }
}

impl Raid<'_> {
    /// The most valuable target for its distance that `strength` can strike,
    /// other than `skip`: a known enemy building or hostile start not
    /// recently given up and not seen gone, the rival's first when there are
    /// several enemies.
    fn best(&self, strength: u64, skip: Option<Objective>) -> Option<Objective> {
        let rival = |target: &Objective| self.rival.is_none_or(|rival| target.owner == rival);
        self.best_of(strength, skip, rival)
            .or_else(|| self.best_of(strength, skip, |_| true))
    }

    fn best_of(
        &self,
        strength: u64,
        skip: Option<Objective>,
        eligible: impl Fn(&Objective) -> bool,
    ) -> Option<Objective> {
        let observation = self.observation;
        let start = self.map.start(observation.me)?;
        self.objectives
            .iter()
            .copied()
            .filter(|target| eligible(target))
            .filter(|target| Some(*target) != skip && !self.held.contains(target))
            .filter(|target| {
                !self
                    .memory
                    .abandoned(target.building, target.anchor, observation.tick)
            })
            .filter(|target| self.need(*target) <= strength)
            .max_by_key(|target| {
                let cost = target
                    .building
                    .base_stats()
                    .construction
                    .as_ref()
                    .map_or(0, |construction| construction.cost);
                let distance = centre_distance(
                    footprint_centre(BuildingKind::Foundry, start),
                    footprint_centre(target.building, target.anchor),
                );
                (
                    u64::from(cost) * 1_000 / (100 + distance),
                    std::cmp::Reverse(self.frame.rank(
                        self.frame.home,
                        footprint_centre(target.building, target.anchor),
                    )),
                )
            })
    }

    /// Strength a strike on `target` needs: the known anti-air that reaches
    /// over it times the margin, and never under the stance minimum.
    fn need(&self, target: Objective) -> u64 {
        (cover(self.air, target) * self.margin / 1_000).max(self.minimum)
    }

    /// Value of the known anti-air reaching any member.
    fn opposition(&self, members: &[&UnitObs]) -> u64 {
        self.air
            .iter()
            .filter(|hazard| members.iter().any(|unit| hazard.covers(doubled(unit.tile))))
            .map(|hazard| hazard.value)
            .sum()
    }
}

/// Known anti-air reaching over `target`.
fn cover(air: &[Hazard], target: Objective) -> u64 {
    let centre = footprint_centre(target.building, target.anchor);
    air.iter()
        .filter(|hazard| hazard.covers(centre))
        .map(|hazard| hazard.value)
        .sum()
}

/// What a strike needs against the easiest target the seat knows of and has
/// not given up on: the known anti-air reaching over it by the attack margin,
/// at least the stance's minimum. Nothing while it has given up on all.
pub(crate) fn strike_need(
    observation: &ObservationData,
    memory: &Memory,
    profile: &ResolvedProfile,
    scratch: &Scratch,
) -> u64 {
    let margin = margin(profile.difficulty);
    scratch
        .objectives
        .iter()
        .filter(|target| !memory.abandoned(target.building, target.anchor, observation.tick))
        .map(|target| cover(&scratch.air, *target) * margin / 1_000)
        .min()
        .map_or(0, |need| need.max(minimum(profile.stance)))
}

/// Whether `kind` is a ground-attack aircraft a strike takes.
fn bomber(kind: UnitKind) -> bool {
    composition::role(kind) == Some(Role::AirStrike)
}
