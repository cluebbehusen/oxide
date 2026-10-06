//! The clear mission: a seat whose ground reaches no enemy sends its free
//! splash bombers together ahead of its lifts, at the enemy anti-air around
//! the building a lift goes after, or the next lift would. It goes at the
//! remembered anti-air there whose known cover it outweighs most easily,
//! sweeps it and moves on to the next, then sweeps the building's
//! surroundings, holding the defenders' attention while the carriers land,
//! and withdraws when the anti-air around it outweighs it.

use super::Scratch;
use super::air::{self, Hazard};
use super::attack::{FIT, healthy, margin, minimum, recruit, striking};
use super::{MISSION_CAP, Mission, Missions, StrikePhase, Task, UNIT_CAP, hunt, mine, run};
use crate::composition::{self, Role};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled};
use crate::map::MapModel;
use crate::memory::Memory;
use crate::profile::ResolvedProfile;
use chassis::grid::TilePos;
use oxide_sim::Command;
use oxide_sim::observation::{ObservationData, UnitObs};
use oxide_sim::stats::Domain;

/// Tiles between a member and the aim at which the sweep begins.
const CONTACT_TILES: i32 = 6;

/// Tiles around another clearance's aim left to it.
const HELD_TILES: i32 = 4;

/// Tiles around a lift's target within which anti-air is cleared for it.
const FOCUS_TILES: i32 = 12;

const GATHER_TICKS: u64 = 600;
const TRAVEL_TICKS: u64 = 1_800;
const ENGAGE_TICKS: u64 = 1_200;
const WITHDRAW_TICKS: u64 = 1_200;

/// Ticks a clearance's `phase` should end within.
pub(super) fn timeout(phase: StrikePhase) -> u64 {
    match phase {
        StrikePhase::Gather => GATHER_TICKS,
        StrikePhase::Travel => TRAVEL_TICKS,
        StrikePhase::Engage { .. } => ENGAGE_TICKS,
        StrikePhase::Withdraw => WITHDRAW_TICKS,
    }
}

/// What one decision knows about clearing anti-air.
struct Sweep<'a> {
    observation: &'a ObservationData,
    map: &'a MapModel,
    frame: HomeFrame,
    memory: &'a Memory,
    minimum: u64,
    margin: u64,
    /// Known fire against aircraft.
    air: &'a [Hazard],
    /// The start's ground.
    home: Option<u32>,
    /// What offense leaves at home.
    reserve: [u64; 2],
    /// Aims another clearance holds.
    held: Vec<TilePos>,
    /// Where the building a lift goes after, or the next lift would, stands.
    focus: Option<TilePos>,
}

impl Missions {
    /// Advances every clearance under way, then, while the seat's ground
    /// reaches no enemy, sends the free ground-attack aircraft beyond the
    /// home reserve at enemy anti-air they outweigh.
    pub(crate) fn clear_anti_air(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        profile: &ResolvedProfile,
        memory: &Memory,
        scratch: &Scratch,
        ledger: &mut Ledger,
    ) {
        let mut sweep = Sweep {
            observation,
            map,
            frame: scratch.frame,
            memory,
            minimum: minimum(profile.stance),
            margin: margin(profile.difficulty),
            air: &scratch.air,
            home: map
                .start(observation.me)
                .and_then(|start| map.component(start)),
            reserve: scratch.reserve,
            held: Vec::new(),
            focus: self
                .lift_focus(observation, map, memory, scratch)
                .map(|target| target.anchor),
        };
        let clearing: fn(&Task) -> bool = |task| matches!(task, Task::Clear { .. });
        for id in self.ids(clearing) {
            let Some(index) = self.index_of(id) else {
                continue;
            };
            sweep.held = self.aims(Some(id));
            self.advance_clear(index, &sweep, ledger);
        }
        if !scratch.severed {
            return;
        }
        sweep.held = self.aims(None);
        self.form_clear(&sweep, ledger);
    }

    /// Bomber strength a clearance ahead of the next lift needs: the known
    /// anti-air around its target by the margin. Nothing while no lift has a
    /// target or the seat's ground reaches an enemy.
    pub(crate) fn clear_need(
        &self,
        observation: &ObservationData,
        map: &MapModel,
        profile: &ResolvedProfile,
        memory: &Memory,
        scratch: &Scratch,
    ) -> u64 {
        if !scratch.severed {
            return 0;
        }
        let sweep = Sweep {
            observation,
            map,
            frame: scratch.frame,
            memory,
            minimum: minimum(profile.stance),
            margin: margin(profile.difficulty),
            air: &scratch.air,
            home: map
                .start(observation.me)
                .and_then(|start| map.component(start)),
            reserve: scratch.reserve,
            held: Vec::new(),
            focus: self
                .lift_focus(observation, map, memory, scratch)
                .map(|target| target.anchor),
        };
        if sweep.focus.is_none() {
            return 0;
        }
        sweep.guard()
    }

    /// Strength of the bombers out clearing the anti-air around `target`:
    /// cover a lift there can count on while they hold the defenders'
    /// attention.
    pub(super) fn cover(&self, observation: &ObservationData, target: TilePos) -> u64 {
        self.list
            .iter()
            .filter(|mission| match mission.task {
                Task::Clear { aim, .. } => aim.chebyshev(target) <= FOCUS_TILES,
                _ => false,
            })
            .filter(|mission| mission.holds(observation))
            .flat_map(|mission| mission.units.iter())
            .filter_map(|id| mine(observation, *id))
            .map(|unit| striking(unit))
            .sum()
    }

    /// Where every clearance but `except` is aimed.
    fn aims(&self, except: Option<u64>) -> Vec<TilePos> {
        self.list
            .iter()
            .filter(|mission| Some(mission.id) != except)
            .filter_map(|mission| match mission.task {
                Task::Clear { aim, .. } => Some(aim),
                _ => None,
            })
            .collect()
    }

    /// Sends every free ground-attack aircraft beyond the reserve at the
    /// anti-air they outweigh most easily. Returns whether a clearance formed.
    fn form_clear(&mut self, sweep: &Sweep<'_>, ledger: &mut Ledger) -> bool {
        let observation = sweep.observation;
        if self.list.len() >= MISSION_CAP {
            return false;
        }
        let fit: Vec<&UnitObs> = self
            .free(observation, ledger)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .filter(|unit| bomber(unit) && healthy(unit, FIT))
            .collect();
        let fit = self.spare(observation, sweep.map, sweep.reserve).outermost(
            sweep.map,
            sweep.frame,
            fit,
        );
        let strength: u64 = fit.iter().map(|unit| striking(unit)).sum();
        if strength < sweep.minimum || strength < sweep.guard() {
            return false;
        }
        let Some(aim) = sweep.best(strength, None) else {
            return false;
        };
        let Some(rally) = air::pad(observation, sweep.map, sweep.frame, doubled(aim)) else {
            return false;
        };
        let recruits = recruit(sweep.frame, &fit, rally, u64::MAX, UNIT_CAP);
        if recruits.is_empty() || !ledger.order(run(recruits.clone(), rally)) {
            return false;
        }
        self.list.push(Mission {
            id: self.next,
            since: observation.tick,
            units: recruits,
            goal: rally,
            task: Task::Clear {
                aim,
                phase: StrikePhase::Gather,
            },
        });
        self.next += 1;
        true
    }

    /// Moves the clearance at `index` through its phases.
    fn advance_clear(&mut self, index: usize, sweep: &Sweep<'_>, ledger: &mut Ledger) {
        let observation = sweep.observation;
        let now = observation.tick;
        let mission = &self.list[index];
        let Task::Clear { aim, phase } = mission.task else {
            unreachable!("the clear mission's task");
        };
        let members: Vec<&UnitObs> = mission
            .units
            .iter()
            .filter_map(|id| mine(observation, *id))
            .collect();
        let all_idle = members.iter().all(|unit| unit.idle);
        let strength: u64 = members.iter().map(|unit| striking(unit)).sum();
        let age = now - mission.since;
        let Some(rally) = air::pad(observation, sweep.map, sweep.frame, doubled(aim)) else {
            self.list.remove(index);
            return;
        };
        let goal = mission.goal;
        let units = mission.units.clone();
        let withdraw = |missions: &mut Self, ledger: &mut Ledger| {
            if ledger.order(run(units.clone(), rally)) {
                missions.list[index].set_clear(aim, StrikePhase::Withdraw, now, rally);
            }
        };
        match phase {
            StrikePhase::Gather => {
                if !all_idle && age < GATHER_TICKS {
                    return;
                }
                if strength < sweep.need(aim) {
                    self.list.remove(index);
                    return;
                }
                if self.sweep_at(index, sweep, rally, aim, ledger) {
                    self.list[index].set_clear(aim, StrikePhase::Travel, now, aim);
                }
            }
            StrikePhase::Travel => {
                if members
                    .iter()
                    .any(|unit| unit.tile.chebyshev(goal) <= CONTACT_TILES)
                {
                    let engage = StrikePhase::Engage { focus: None };
                    self.list[index].set_clear(aim, engage, now, goal);
                    return;
                }
                if all_idle || age >= TRAVEL_TICKS || sweep.opposition(&members) > strength {
                    withdraw(self, ledger);
                }
            }
            StrikePhase::Engage { .. } => {
                if sweep.opposition(&members) > strength {
                    withdraw(self, ledger);
                    return;
                }
                if !all_idle && age < ENGAGE_TICKS {
                    return;
                }
                match sweep.best(strength, Some(goal)) {
                    Some(next) => {
                        if self.sweep_at(index, sweep, goal, next, ledger) {
                            self.list[index].set_clear(next, StrikePhase::Travel, now, next);
                        }
                    }
                    None => withdraw(self, ledger),
                }
            }
            StrikePhase::Withdraw => {
                if all_idle || age >= WITHDRAW_TICKS {
                    self.list.remove(index);
                }
            }
        }
    }

    /// Sends the clearance at `index` from `from` to `aim`, around known
    /// anti-air when the straight line crosses it. Returns whether it went.
    fn sweep_at(
        &mut self,
        index: usize,
        sweep: &Sweep<'_>,
        from: TilePos,
        aim: TilePos,
        ledger: &mut Ledger,
    ) -> bool {
        let units = self.list[index].units.clone();
        let others: Vec<Hazard> = sweep
            .air
            .iter()
            .copied()
            .filter(|hazard| !hazard.covers(doubled(aim)))
            .collect();
        let Some(via) = air::route(sweep.observation, sweep.frame, &others, from, aim) else {
            return ledger.order(hunt(units, aim));
        };
        if ledger.room() < 2 {
            return false;
        }
        ledger.order(run(units.clone(), via));
        ledger.order(Command::Hunt {
            units,
            goal: aim,
            queue: true,
        })
    }
}

impl Mission {
    fn set_clear(&mut self, aim: TilePos, phase: StrikePhase, now: u64, goal: TilePos) {
        self.task = Task::Clear { aim, phase };
        self.since = now;
        self.goal = goal;
    }
}

impl Sweep<'_> {
    /// Where `strength` goes next around the lift's target: the remembered
    /// enemy anti-air there on ground the seat cannot walk to that it
    /// outweighs most easily, and once none is left, the target itself.
    /// Never near `skip` or an aim another clearance holds.
    fn best(&self, strength: u64, skip: Option<TilePos>) -> Option<TilePos> {
        let focus = self.focus?;
        let near = |a: TilePos, b: TilePos| a.chebyshev(b) <= HELD_TILES;
        let anti_air: Vec<TilePos> = self
            .memory
            .units()
            .iter()
            .filter(|unit| {
                unit.kind.stats().domain == Domain::Ground
                    && composition::role(unit.kind) == Some(Role::AntiAir)
            })
            .map(|unit| unit.tile)
            .filter(|tile| tile.chebyshev(focus) <= FOCUS_TILES)
            .filter(|tile| {
                let ground = self.map.component(*tile);
                ground.is_some() && ground != self.home
            })
            .collect();
        let aims = if anti_air.is_empty() {
            vec![focus]
        } else {
            anti_air
        };
        aims.into_iter()
            .filter(|tile| skip.is_none_or(|skip| !near(*tile, skip)))
            .filter(|tile| !self.held.iter().any(|aim| near(*tile, *aim)))
            .map(|tile| (self.need(tile), tile))
            .filter(|(need, _)| *need <= strength)
            .min_by_key(|(need, tile)| (*need, self.frame.rank(self.frame.home, doubled(*tile))))
            .map(|(_, tile)| tile)
    }

    /// Strength a clearance needs to go at all: the known anti-air reaching
    /// over the lift's target or any anti-air around it, each counted once,
    /// times the margin, so the bombers go in together rather than one at a
    /// time.
    fn guard(&self) -> u64 {
        let Some(focus) = self.focus else {
            return u64::MAX;
        };
        let points: Vec<(i64, i64)> = self
            .memory
            .units()
            .iter()
            .filter(|unit| {
                unit.kind.stats().domain == Domain::Ground
                    && composition::role(unit.kind) == Some(Role::AntiAir)
                    && unit.tile.chebyshev(focus) <= FOCUS_TILES
            })
            .map(|unit| doubled(unit.tile))
            .chain(std::iter::once(doubled(focus)))
            .collect();
        let cover: u64 = self
            .air
            .iter()
            .filter(|hazard| points.iter().any(|point| hazard.covers(*point)))
            .map(|hazard| hazard.value)
            .sum();
        (cover * self.margin / 1_000).max(self.minimum)
    }

    /// Strength a sweep at `aim` needs: the known anti-air reaching over it
    /// times the margin, and never under the stance minimum.
    fn need(&self, aim: TilePos) -> u64 {
        let cover: u64 = self
            .air
            .iter()
            .filter(|hazard| hazard.covers(doubled(aim)))
            .map(|hazard| hazard.value)
            .sum();
        (cover * self.margin / 1_000).max(self.minimum)
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

/// Whether `unit` is a splash bomber, the ground-attack aircraft a
/// clearance takes: one blast hits a clump of anti-air.
fn bomber(unit: &UnitObs) -> bool {
    composition::role(unit.kind) == Some(Role::AirStrike) && splash(unit.kind)
}

/// Whether `kind`'s ground weapon hits around where it lands.
pub(super) fn splash(kind: oxide_sim::UnitKind) -> bool {
    kind.stats()
        .weapons
        .iter()
        .any(|weapon| weapon.targets.ground && weapon.splash.is_some())
}
