//! The attack mission: when a known enemy building's local defense is
//! beatable now by the free army, above a stance-bounded minimum, the army
//! gathers near home, travels, fights, withdraws from a losing fight, and
//! recovers to go again or disband. A free Tender joins while it regroups,
//! welds its wounded, and follows it; against known defenses free Sappers
//! join too, and each blows up the nearest defense once the fight begins.

use super::Scratch;
use super::raid::{aim, blast};
use super::support::{patient, weld};
use super::{
    AttackPhase, MISSION_CAP, Mission, Missions, Objective, Task, UNIT_CAP, approach, hits, hunt,
    insert, mine, run, standing, value,
};
use crate::composition::{self, Role};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled, footprint_centre, gap, ring};
use crate::map::MapModel;
use crate::memory::Memory;
use crate::profile::ResolvedProfile;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData, UnitObs};
use oxide_sim::scenario::{BotDifficulty, BotStance};
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingKind, PlayerId, UnitId, UnitKind};

/// Tiles between a member and an enemy that make contact.
const CONTACT_TILES: i32 = 8;

/// Tiles around a target inside which known enemies defend it.
const DEFENSE_TILES: i32 = 10;

/// Ground distance from the seat's start, in tenths of a tile, inside which
/// the army gathers.
const RALLY_REACH: u16 = 80;

/// Tiles around the start searched for the rally point.
const RALLY_SEARCH: i32 = 10;

/// Ticks without an attack that lower the margin one step.
const WAIT_STEP: u64 = 2_400;

/// Per-mille margin one waiting step removes, and the floor it stops at.
const WAIT_RELIEF: u64 = 250;
const MARGIN_FLOOR: u64 = 1_000;

/// Health, per mille, under which a member leaves between fights, and at or
/// over which a unit joins.
const WOUNDED: u32 = 350;
pub(super) const FIT: u32 = 500;

const GATHER_TICKS: u64 = 1_200;
const TRAVEL_TICKS: u64 = 3_600;
const ENGAGE_TICKS: u64 = 3_600;
const WITHDRAW_TICKS: u64 = 1_200;
const RECOVER_TICKS: u64 = 1_200;

/// Ticks an attack's `phase` should end within.
pub(super) fn timeout(phase: AttackPhase) -> u64 {
    match phase {
        AttackPhase::Gather => GATHER_TICKS,
        AttackPhase::Travel => TRAVEL_TICKS,
        AttackPhase::Engage { .. } => ENGAGE_TICKS,
        AttackPhase::Withdraw => WITHDRAW_TICKS,
        AttackPhase::Recover => RECOVER_TICKS,
    }
}

/// A building to attack and where the army goes to reach it.
#[derive(Clone, Copy)]
struct Target {
    owner: PlayerId,
    building: BuildingKind,
    anchor: TilePos,
    approach: TilePos,
    score: u64,
}

impl Target {
    fn objective(self) -> Objective {
        Objective {
            owner: self.owner,
            building: self.building,
            anchor: self.anchor,
        }
    }
}

/// What one decision knows about attacking.
struct Plan<'a> {
    observation: &'a ObservationData,
    map: &'a MapModel,
    frame: HomeFrame,
    memory: &'a Memory,
    minimum: u64,
    margin: u64,
    /// Idle free Tenders.
    tenders: Vec<&'a UnitObs>,
    /// Idle free Sappers.
    sappers: Vec<&'a UnitObs>,
    /// The enemy to go after first, when there are several.
    rival: Option<PlayerId>,
    /// Targets another attack holds or this decision gave up on.
    held: Vec<Objective>,
    /// Missing health among members one Tender answers for.
    per_tender: u64,
}

impl Mission {
    /// Moves an attack to `phase` against `target` from `now`, sending its
    /// members to `goal` when one is given.
    fn attack_phase(
        &mut self,
        target: Target,
        phase: AttackPhase,
        now: u64,
        goal: Option<TilePos>,
    ) {
        self.task = Task::Attack {
            target: target.objective(),
            phase,
        };
        self.since = now;
        if let Some(goal) = goal {
            self.goal = goal;
        }
    }
}

impl Missions {
    /// Advances every attack under way, then launches another while the free
    /// army beyond the home reserve can beat the known defense of a target no
    /// attack holds. Returns the targets given up on.
    pub(crate) fn attack(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        profile: &ResolvedProfile,
        memory: &Memory,
        scratch: &Scratch,
        ledger: &mut Ledger,
    ) -> Vec<(BuildingKind, TilePos)> {
        let now = observation.tick;
        let attacking: fn(&Task) -> bool = |task| matches!(task, Task::Attack { .. });
        let mut plan = Plan {
            observation,
            map,
            frame: scratch.frame,
            memory,
            minimum: minimum(profile.stance),
            margin: margin(profile.difficulty),
            tenders: Vec::new(),
            sappers: Vec::new(),
            rival: scratch.rival,
            held: Vec::new(),
            per_tender: super::support::per_tender(profile.traits.support),
        };
        let mut given_up: Vec<Objective> = Vec::new();
        for id in self.ids(attacking) {
            let Some(index) = self.index_of(id) else {
                continue;
            };
            let fit = self.refresh(&mut plan, scratch, ledger);
            plan.held = self.held(attacking, Some(id), &given_up);
            if let Some(failed) = self.advance(index, &plan, &fit, ledger) {
                given_up.push(failed);
            }
        }

        let mut fit = self.refresh(&mut plan, scratch, ledger);
        let free: u64 = fit.iter().map(|unit| value(unit)).sum();
        let mut producers = observation
            .my_buildings
            .iter()
            .zip(&observation.my_queues)
            .filter(|(building, _)| {
                building.built && !building.kind.base_stats().produces.is_empty()
            })
            .peekable();
        let idle = producers.peek().is_some() && producers.all(|(_, queue)| queue.is_empty());
        if free >= plan.minimum || idle {
            self.waiting.get_or_insert(now);
        } else {
            self.waiting = None;
        }
        let waited = self.waiting.map_or(0, |since| (now - since) / WAIT_STEP);
        plan.margin = plan
            .margin
            .saturating_sub(waited * WAIT_RELIEF)
            .max(MARGIN_FLOOR);
        loop {
            plan.held = self.held(attacking, None, &given_up);
            if !self.launch(&plan, &fit, ledger) {
                break;
            }
            fit = self.refresh(&mut plan, scratch, ledger);
        }
        given_up
            .into_iter()
            .map(|target| (target.building, target.anchor))
            .collect()
    }

    /// Sappers the seat's attacks want: one for each known enemy defense
    /// around the targets of attacks under way and around the best target no
    /// attack holds yet.
    pub(crate) fn sappers_wanted(
        &self,
        observation: &ObservationData,
        map: &MapModel,
        profile: &ResolvedProfile,
        memory: &Memory,
        scratch: &Scratch,
    ) -> usize {
        let attacking: fn(&Task) -> bool = |task| matches!(task, Task::Attack { .. });
        let held = self.held(attacking, None, &[]);
        let plan = Plan {
            observation,
            map,
            frame: scratch.frame,
            memory,
            minimum: minimum(profile.stance),
            margin: margin(profile.difficulty),
            tenders: Vec::new(),
            sappers: Vec::new(),
            rival: scratch.rival,
            held: held.clone(),
            per_tender: super::support::per_tender(profile.traits.support),
        };
        // Remembered buildings share one id, so a defense is told apart by
        // where it stands.
        let mut defenses: Vec<_> = held
            .iter()
            .filter_map(|target| plan.target(target.owner, target.building, target.anchor))
            .chain(plan.best(None))
            .flat_map(|target| {
                plan.defenses(target)
                    .map(|building| (building.player, building.kind, building.anchor))
            })
            .collect();
        defenses.sort_unstable();
        defenses.dedup();
        defenses.len()
    }

    /// The free army beyond the home reserve fit to fight, farthest from home
    /// first, and the idle free Tenders and Sappers into `plan`, as earlier
    /// missions of this decision left them.
    fn refresh<'a>(
        &self,
        plan: &mut Plan<'a>,
        scratch: &Scratch,
        ledger: &Ledger,
    ) -> Vec<&'a UnitObs> {
        let (observation, map) = (plan.observation, plan.map);
        let free: Vec<&UnitObs> = self
            .free(observation, ledger)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .collect();
        let idle = |kind: UnitKind| -> Vec<&'a UnitObs> {
            free.iter()
                .copied()
                .filter(|unit| unit.kind == kind && unit.idle)
                .collect()
        };
        plan.tenders = idle(UnitKind::Tender);
        plan.sappers = idle(UnitKind::Sapper);
        let fit: Vec<&UnitObs> = free
            .iter()
            .copied()
            .filter(|unit| eligible(unit, FIT))
            .collect();
        self.spare(observation, map, scratch.reserve)
            .outermost(map, scratch.frame, fit)
    }

    /// Launches an attack on the best target no other attack holds when the
    /// free army can beat its known defense. Returns whether one launched.
    fn launch(&mut self, plan: &Plan<'_>, fit: &[&UnitObs], ledger: &mut Ledger) -> bool {
        if self.list.len() >= MISSION_CAP {
            return false;
        }
        let Some(target) = plan.best(None) else {
            return false;
        };
        let Some(rally) = plan.rally(target.owner) else {
            return false;
        };
        let need = plan.need(target);
        let component = plan.map.component(target.approach);
        let fit: Vec<&UnitObs> = fit
            .iter()
            .copied()
            .filter(|unit| reaches(plan.map, unit, component))
            .collect();
        if fit.iter().map(|unit| striking(unit)).sum::<u64>() < need {
            return false;
        }
        let recruits = recruit(plan.frame, &fit, rally, need, UNIT_CAP);
        if recruits.is_empty() || !ledger.order(hunt(recruits.clone(), rally)) {
            return false;
        }
        self.list.push(Mission {
            id: self.next,
            since: plan.observation.tick,
            units: recruits,
            goal: rally,
            task: Task::Attack {
                target: target.objective(),
                phase: AttackPhase::Gather,
            },
        });
        self.next += 1;
        self.waiting = None;
        true
    }

    /// Moves the attack at `index` through its phases. Returns the target
    /// when the attack gave up on it.
    fn advance(
        &mut self,
        index: usize,
        plan: &Plan<'_>,
        fit: &[&UnitObs],
        ledger: &mut Ledger,
    ) -> Option<Objective> {
        let observation = plan.observation;
        let now = observation.tick;
        let mission = &self.list[index];
        let id = mission.id;
        let Task::Attack {
            target:
                Objective {
                    owner,
                    building,
                    anchor,
                },
            phase,
        } = mission.task
        else {
            unreachable!("the attack mission's task");
        };
        let rally = plan.rally(owner)?;
        let engaged = matches!(phase, AttackPhase::Engage { .. });
        let regrouping = matches!(phase, AttackPhase::Gather | AttackPhase::Recover);

        if !engaged {
            let wounded: Vec<UnitId> = mission
                .units
                .iter()
                .copied()
                .filter(|id| mine(observation, *id).is_some_and(|unit| !healthy(unit, WOUNDED)))
                .collect();
            if !wounded.is_empty() && ledger.order(run(wounded.clone(), rally)) {
                self.release(&wounded);
            }
        }
        let index = self.index_of(id)?;

        let current = plan.target(owner, building, anchor);
        let target = if regrouping {
            let current =
                current.filter(|_| !plan.memory.abandoned(building, anchor, observation.tick));
            match (current, plan.best(None)) {
                (Some(current), Some(best)) if best.score * 4 < current.score * 5 => Some(current),
                (_, best) => best,
            }
        } else {
            current
        };
        let Some(target) = target else {
            if regrouping {
                self.list.remove(index);
                return None;
            }
            let mission = &mut self.list[index];
            if ledger.order(run(mission.units.clone(), rally)) {
                if let Task::Attack { phase, .. } = &mut mission.task {
                    *phase = AttackPhase::Recover;
                }
                mission.since = now;
                mission.goal = rally;
            }
            return None;
        };
        let need = plan.need(target);
        let component = plan.map.component(target.approach);
        let mission = &mut self.list[index];
        mission.task = Task::Attack {
            target: target.objective(),
            phase,
        };
        let members: Vec<&UnitObs> = mission
            .units
            .iter()
            .filter_map(|id| mine(observation, *id))
            .collect();
        let all_idle = members
            .iter()
            .filter(|unit| !matches!(unit.kind, UnitKind::Tender | UnitKind::Sapper))
            .all(|unit| unit.idle);
        let strength: u64 = members
            .iter()
            .filter(|unit| !unit.kind.stats().weapons.is_empty())
            .map(|unit| value(unit))
            .sum();
        let age = now - mission.since;

        match phase {
            AttackPhase::Gather | AttackPhase::Recover => {
                let room = UNIT_CAP - mission.units.len();
                let fit: Vec<&UnitObs> = fit
                    .iter()
                    .copied()
                    .filter(|unit| reaches(plan.map, unit, component))
                    .collect();
                let striking_strength: u64 = members.iter().map(|unit| striking(unit)).sum();
                let recruits = recruit(
                    plan.frame,
                    &fit,
                    rally,
                    need.saturating_sub(striking_strength),
                    room,
                );
                let recruited = !recruits.is_empty() && ledger.order(hunt(recruits.clone(), rally));
                if recruited {
                    for id in &recruits {
                        insert(&mut mission.units, *id);
                    }
                }
                let tending = members
                    .iter()
                    .filter(|unit| unit.kind == UnitKind::Tender)
                    .count();
                let wanted =
                    (super::support::wounds(members.iter().copied()) / plan.per_tender).max(1);
                let room = (wanted as usize)
                    .saturating_sub(tending)
                    .min(UNIT_CAP.saturating_sub(mission.units.len()));
                let tenders = plan.nearest_tenders(rally, component, room);
                if !tenders.is_empty() && ledger.order(run(tenders.clone(), rally)) {
                    for tender in tenders {
                        insert(&mut mission.units, tender);
                    }
                }
                let sapping = members
                    .iter()
                    .filter(|unit| unit.kind == UnitKind::Sapper)
                    .count();
                // A Sapper for each known defense around the target.
                let room = plan
                    .defenses(target)
                    .count()
                    .saturating_sub(sapping)
                    .min(UNIT_CAP.saturating_sub(mission.units.len()));
                let sappers = plan.sappers(target, rally, component, room);
                if !sappers.is_empty() && ledger.order(run(sappers.clone(), rally)) {
                    for sapper in sappers {
                        insert(&mut mission.units, sapper);
                    }
                }
                let ready = striking_strength >= need;
                if !recruited && (all_idle || age >= timeout(phase)) && ready {
                    if ledger.order(hunt(mission.units.clone(), target.approach)) {
                        mission.attack_phase(
                            target,
                            AttackPhase::Travel,
                            now,
                            Some(target.approach),
                        );
                    }
                } else if age >= timeout(phase) {
                    self.list.remove(index);
                } else {
                    for tender in members
                        .iter()
                        .filter(|unit| unit.kind == UnitKind::Tender && unit.idle)
                    {
                        if ledger.spendable() < crate::workers::WELD_FLOOR {
                            break;
                        }
                        if let Some(patient) = patient(plan.map, plan.frame, tender, &members) {
                            ledger.order(weld(tender.id, patient));
                        }
                    }
                }
                None
            }
            AttackPhase::Travel => {
                let arrived = members
                    .iter()
                    .any(|unit| unit.tile.chebyshev(target.approach) <= CONTACT_TILES);
                if arrived || contact(observation, &members) {
                    mission.attack_phase(target, AttackPhase::Engage { focus: None }, now, None);
                    return None;
                }
                if (all_idle || age >= TRAVEL_TICKS)
                    && ledger.order(run(mission.units.clone(), rally))
                {
                    mission.attack_phase(target, AttackPhase::Recover, now, Some(rally));
                    return Some(target.objective());
                }
                None
            }
            AttackPhase::Engage { .. } => {
                // Sappers sent at a defense keep that order whatever the rest
                // of the army is told next.
                let mut blasting = Vec::new();
                for sapper in members
                    .iter()
                    .filter(|unit| unit.kind == UnitKind::Sapper && unit.idle)
                {
                    if let Some(defense) = plan.nearest_defense(target, sapper)
                        && ledger.order(blast(vec![sapper.id], aim(defense)))
                    {
                        blasting.push(sapper.id);
                    }
                }
                let army: Vec<UnitId> = mission
                    .units
                    .iter()
                    .copied()
                    .filter(|id| !blasting.contains(id))
                    .collect();
                if strength < plan.opposition(&members) {
                    if ledger.order(run(army.clone(), rally)) {
                        mission.attack_phase(target, AttackPhase::Withdraw, now, Some(rally));
                        return Some(target.objective());
                    }
                    return None;
                }
                if !all_idle && age < ENGAGE_TICKS {
                    return None;
                }
                let standing = standing(observation, target.objective());
                let next = plan.best(Some(target)).filter(|next| {
                    !standing
                        && members.iter().map(|unit| striking(unit)).sum::<u64>()
                            >= plan.need(*next)
                });
                if let Some(next) = next {
                    if ledger.order(hunt(army, next.approach)) {
                        mission.attack_phase(next, AttackPhase::Travel, now, Some(next.approach));
                    }
                    return None;
                }
                if ledger.order(run(army, rally)) {
                    mission.attack_phase(target, AttackPhase::Recover, now, Some(rally));
                    if standing {
                        return Some(target.objective());
                    }
                }
                None
            }
            AttackPhase::Withdraw => {
                if all_idle || age >= WITHDRAW_TICKS {
                    mission.attack_phase(target, AttackPhase::Recover, now, None);
                }
                None
            }
        }
    }
}

impl<'a> Plan<'a> {
    /// Idle free Sappers nearest `rally`, at most `room`, that can get to a
    /// target whose approach lies in `component`, when known enemy defenses
    /// that can hit ground stand around `target`.
    fn sappers(
        &self,
        target: Target,
        rally: TilePos,
        component: Option<u32>,
        room: usize,
    ) -> Vec<UnitId> {
        if self.defenses(target).next().is_none() {
            return Vec::new();
        }
        let mut sappers: Vec<&UnitObs> = self
            .sappers
            .iter()
            .copied()
            .filter(|unit| reaches(self.map, unit, component))
            .collect();
        sappers.sort_by_key(|unit| (self.frame.rank(doubled(rally), doubled(unit.tile)), unit.id));
        let mut ids: Vec<UnitId> = sappers.iter().take(room).map(|unit| unit.id).collect();
        ids.sort_unstable();
        ids
    }

    /// Known enemy buildings around `target` that can hit ground.
    fn defenses(&self, target: Target) -> impl Iterator<Item = &'a BuildingObs> {
        let size = target.building.base_stats().size;
        self.observation
            .enemy_buildings
            .iter()
            .filter(move |building| {
                building
                    .kind
                    .base_stats()
                    .weapons
                    .iter()
                    .any(|weapon| weapon.targets.ground)
                    && gap(
                        target.anchor,
                        size,
                        building.anchor,
                        building.kind.base_stats().size,
                    ) < DEFENSE_TILES
            })
    }

    /// The known defense around `target` on `sapper`'s ground nearest it,
    /// else the target itself while the seat knows it.
    fn nearest_defense(&self, target: Target, sapper: &UnitObs) -> Option<&'a BuildingObs> {
        let from = doubled(sapper.tile);
        let ground = self.map.component(sapper.tile);
        self.defenses(target)
            .filter(|building| {
                ring(building.anchor, building.kind.base_stats().size)
                    .any(|tile| ground.is_some() && self.map.component(tile) == ground)
            })
            .min_by_key(|building| {
                (
                    self.frame
                        .rank(from, footprint_centre(building.kind, building.anchor)),
                    building.id,
                )
            })
            .or_else(|| {
                self.observation.enemy_buildings.iter().find(|building| {
                    (building.player, building.kind, building.anchor)
                        == (target.owner, target.building, target.anchor)
                })
            })
    }

    /// Idle free Tenders nearest `rally`, at most `room`, that can get to a
    /// target whose approach lies in `component`.
    fn nearest_tenders(&self, rally: TilePos, component: Option<u32>, room: usize) -> Vec<UnitId> {
        let mut tenders: Vec<&UnitObs> = self
            .tenders
            .iter()
            .copied()
            .filter(|unit| reaches(self.map, unit, component))
            .collect();
        tenders.sort_by_key(|unit| (self.frame.rank(doubled(rally), doubled(unit.tile)), unit.id));
        let mut ids: Vec<UnitId> = tenders.iter().take(room).map(|unit| unit.id).collect();
        ids.sort_unstable();
        ids
    }

    /// The best target other than `skip`, or `None`. Known enemy buildings
    /// come first; with none, hostile starts are presumed held.
    /// With several enemies, the rival's targets come first.
    fn best(&self, skip: Option<Target>) -> Option<Target> {
        let now = self.observation.tick;
        let differs = |target: &Target| {
            skip.is_none_or(|skip| {
                (skip.owner, skip.building, skip.anchor)
                    != (target.owner, target.building, target.anchor)
            })
        };
        let pick = |targets: Vec<Target>| {
            targets
                .into_iter()
                .filter(differs)
                .filter(|target| !self.held.contains(&target.objective()))
                .filter(|target| !self.memory.abandoned(target.building, target.anchor, now))
                .max_by_key(|target| {
                    (
                        target.score,
                        std::cmp::Reverse(
                            self.frame.rank(self.frame.home, doubled(target.approach)),
                        ),
                    )
                })
        };
        let known: Vec<Target> = self
            .observation
            .enemy_buildings
            .iter()
            .filter_map(|building| self.target(building.player, building.kind, building.anchor))
            .collect();
        let starts: Vec<Target> = self
            .map
            .hostiles(self.observation.me)
            .filter_map(|owner| {
                let anchor = self.map.start(owner)?;
                self.target(owner, BuildingKind::Foundry, anchor)
            })
            .collect();
        let rival = |targets: &[Target]| -> Vec<Target> {
            targets
                .iter()
                .copied()
                .filter(|target| self.rival.is_none_or(|rival| target.owner == rival))
                .collect()
        };
        pick(rival(&known))
            .or_else(|| pick(rival(&starts)))
            .or_else(|| pick(known))
            .or_else(|| pick(starts))
    }

    /// `building` at `anchor` as a target, if the army can reach it.
    fn target(&self, owner: PlayerId, building: BuildingKind, anchor: TilePos) -> Option<Target> {
        let approach = approach(self.map, self.observation.me, self.frame, building, anchor)?;
        let distance = u64::from(self.map.distance(self.observation.me, approach));
        let cost = building
            .base_stats()
            .construction
            .as_ref()
            .map_or(0, |construction| construction.cost);
        Some(Target {
            owner,
            building,
            anchor,
            approach,
            score: u64::from(cost) * 1_000 / (100 + distance / 10),
        })
    }

    /// Army value the attack on `target` needs: its known local defense
    /// times the margin, and never under the stance minimum.
    fn need(&self, target: Target) -> u64 {
        (defense(self.observation, self.memory, target.approach) * self.margin / 1_000)
            .max(self.minimum)
    }

    /// Value of the armed enemies the seat knows of within contact of the
    /// members: remembered units by confidence, and known armed buildings by
    /// health.
    fn opposition(&self, members: &[&UnitObs]) -> u64 {
        let now = self.observation.tick;
        let near = |tile: TilePos| {
            members
                .iter()
                .any(|unit| unit.tile.chebyshev(tile) <= CONTACT_TILES)
        };
        let units: u64 = self
            .memory
            .units()
            .iter()
            .filter(|unit| !unit.kind.stats().weapons.is_empty() && near(unit.tile))
            .map(|unit| unit.value(now))
            .sum();
        let buildings: u64 = self
            .observation
            .enemy_buildings
            .iter()
            .filter(|building| !building.kind.base_stats().weapons.is_empty())
            .filter(|building| {
                members.iter().any(|unit| {
                    gap(
                        building.anchor,
                        building.kind.base_stats().size,
                        unit.tile,
                        (1, 1),
                    ) < CONTACT_TILES
                })
            })
            .map(building_value)
            .sum();
        units + buildings
    }

    /// Where the army gathers against `owner`: the open tile within reach of
    /// the seat's start that is nearest `owner`'s start by ground.
    fn rally(&self, owner: PlayerId) -> Option<TilePos> {
        let me = self.observation.me;
        let start = self.map.start(me)?;
        let occupied = |tile: TilePos| {
            self.observation.my_buildings.iter().any(|building| {
                gap(
                    building.anchor,
                    building.kind.base_stats().size,
                    tile,
                    (1, 1),
                ) < 0
            })
        };
        (-RALLY_SEARCH..=RALLY_SEARCH + 1)
            .flat_map(|dy| (-RALLY_SEARCH..=RALLY_SEARCH + 1).map(move |dx| start.offset(dx, dy)))
            .filter(|tile| self.map.distance(me, *tile) <= RALLY_REACH)
            .filter(|tile| !occupied(*tile))
            .min_by_key(|tile| {
                (
                    self.map.distance(owner, *tile),
                    self.frame.rank(self.frame.home, doubled(*tile)),
                )
            })
    }
}

/// The known ground defense around `tile`: remembered armed enemy units by
/// confidence, and known enemy buildings that fire on ground by health.
pub(super) fn defense(observation: &ObservationData, memory: &Memory, tile: TilePos) -> u64 {
    let now = observation.tick;
    let units: u64 = memory
        .units()
        .iter()
        .filter(|unit| !unit.kind.stats().weapons.is_empty())
        .filter(|unit| unit.tile.chebyshev(tile) <= DEFENSE_TILES)
        .map(|unit| unit.value(now))
        .sum();
    let buildings: u64 = observation
        .enemy_buildings
        .iter()
        .filter(|building| guards(building, tile))
        .map(building_value)
        .sum();
    units + buildings
}

/// Whether a known, built enemy building's ground fire reaches `tile`.
pub(super) fn fortified(observation: &ObservationData, tile: TilePos) -> bool {
    observation.enemy_buildings.iter().any(|building| {
        let reach = building
            .kind
            .tier_stats(building.tier)
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.ground)
            .map(|weapon| weapon.range.ceil().to_num::<i32>())
            .max();
        building.built
            && reach.is_some_and(|reach| {
                gap(
                    building.anchor,
                    building.kind.base_stats().size,
                    tile,
                    (1, 1),
                ) < reach
            })
    })
}

/// Whether `building` fires on ground and stands around `tile`.
fn guards(building: &BuildingObs, tile: TilePos) -> bool {
    building
        .kind
        .base_stats()
        .weapons
        .iter()
        .any(|weapon| weapon.targets.ground)
        && gap(
            building.anchor,
            building.kind.base_stats().size,
            tile,
            (1, 1),
        ) < DEFENSE_TILES
}

/// Whether any visible armed enemy or seen enemy building is within contact
/// of a member.
pub(super) fn contact(observation: &ObservationData, members: &[&UnitObs]) -> bool {
    let near = |tile: TilePos| {
        members
            .iter()
            .any(|unit| unit.tile.chebyshev(tile) <= CONTACT_TILES)
    };
    observation
        .enemy_units
        .iter()
        .any(|enemy| !enemy.kind.stats().weapons.is_empty() && near(enemy.tile))
        || observation.enemy_buildings.iter().any(|building| {
            building.seen
                && members.iter().any(|unit| {
                    gap(
                        building.anchor,
                        building.kind.base_stats().size,
                        unit.tile,
                        (1, 1),
                    ) < CONTACT_TILES
                })
        })
}

/// A known building's price with every upgrade it reached, discounted by its
/// missing health at that tier.
pub(crate) fn building_value(building: &BuildingObs) -> u64 {
    let tiers = building.kind.tiers();
    let reached = usize::from(building.tier).min(tiers.len() - 1);
    let paid: u64 = tiers[..=reached]
        .iter()
        .filter_map(|stats| stats.construction.as_ref())
        .map(|construction| u64::from(construction.cost))
        .sum();
    let max_hp = building.kind.tier_stats(building.tier).max_hp;
    paid * u64::from(building.hp) / u64::from(max_hp.max(1))
}

/// Units nearest `rally` first until their value reaches `need`, at most
/// `room` of them, by id.
pub(super) fn recruit(
    frame: HomeFrame,
    fit: &[&UnitObs],
    rally: TilePos,
    need: u64,
    room: usize,
) -> Vec<UnitId> {
    let mut sorted = fit.to_vec();
    sorted.sort_by_key(|unit| (frame.rank(doubled(rally), doubled(unit.tile)), unit.id));
    let mut have = 0;
    let mut recruits = Vec::new();
    for unit in sorted.into_iter().take(room) {
        if have >= need {
            break;
        }
        have += striking(unit);
        recruits.push(unit.id);
    }
    recruits.sort_unstable();
    recruits
}

/// What `unit` adds against a building: its value if it can hit ground.
/// Anti-air escorts go along but cannot take the target.
pub(super) fn striking(unit: &UnitObs) -> u64 {
    if hits(unit, Domain::Ground) {
        value(unit)
    } else {
        0
    }
}

/// A line, siege or anti-air unit at `health` per mille or better.
fn eligible(unit: &UnitObs, health: u32) -> bool {
    matches!(
        composition::role(unit.kind),
        Some(Role::Line | Role::Siege | Role::AntiAir)
    ) && healthy(unit, health)
}

pub(super) fn healthy(unit: &UnitObs, health: u32) -> bool {
    u64::from(unit.hp) * 1_000 >= u64::from(unit.kind.stats().max_hp) * u64::from(health)
}

/// Whether `unit` can get to a target whose approach lies in `component`.
fn reaches(map: &MapModel, unit: &UnitObs, component: Option<u32>) -> bool {
    unit.kind.stats().domain == Domain::Air || map.component(unit.tile) == component
}

/// Army value under which the seat does not attack.
pub(crate) fn minimum(stance: BotStance) -> u64 {
    match stance {
        BotStance::Turtle => 800,
        BotStance::Balanced => 600,
        BotStance::Aggressive => 450,
    }
}

/// Per mille of a target's known defense the army must bring.
pub(crate) fn margin(difficulty: BotDifficulty) -> u64 {
    match difficulty {
        BotDifficulty::Scrapheap => 2_500,
        BotDifficulty::Standard => 2_000,
        BotDifficulty::Veteran => 1_750,
        BotDifficulty::Prime => 1_500,
    }
}
