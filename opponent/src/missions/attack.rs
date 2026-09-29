//! The attack mission: when a known enemy building's local defense is
//! beatable now by the free army, above a stance-bounded minimum, the army
//! gathers near home, travels, fights, withdraws from a losing fight, and
//! recovers to go again or disband.

use super::{
    MISSION_CAP, Mission, MissionKind, Missions, Phase, UNIT_CAP, hits, hunt, insert, mine, value,
};
use crate::composition::{self, Role};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled, gap};
use crate::map::{MapModel, UNREACHABLE};
use crate::memory::Memory;
use crate::profile::ResolvedProfile;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData, UnitObs};
use oxide_sim::scenario::{BotDifficulty, BotStance};
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingKind, Command, PlayerId, UnitId};

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
const FIT: u32 = 500;

const GATHER_TICKS: u64 = 1_200;
const TRAVEL_TICKS: u64 = 3_600;
const ENGAGE_TICKS: u64 = 3_600;
const WITHDRAW_TICKS: u64 = 1_200;
const RECOVER_TICKS: u64 = 1_200;

/// Ticks an attack's `phase` should end within.
pub(super) fn timeout(phase: Phase) -> u64 {
    match phase {
        Phase::Gather => GATHER_TICKS,
        Phase::Travel => TRAVEL_TICKS,
        Phase::Engage => ENGAGE_TICKS,
        Phase::Withdraw => WITHDRAW_TICKS,
        Phase::Recover => RECOVER_TICKS,
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
    fn kind(self) -> MissionKind {
        MissionKind::Attack {
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
}

impl Missions {
    /// Launches an attack when the free army can beat a target's known
    /// defense now, or advances the one under way.
    pub(crate) fn attack(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        frame: HomeFrame,
        profile: &ResolvedProfile,
        memory: &mut Memory,
        ledger: &mut Ledger,
    ) {
        let now = observation.tick;
        let minimum = minimum(profile.stance);
        let owned = self.owned();
        let fit: Vec<&UnitObs> = observation
            .my_units
            .iter()
            .filter(|unit| owned.binary_search(&unit.id).is_err() && eligible(unit, FIT))
            .collect();
        let index = self
            .list
            .iter()
            .position(|mission| matches!(mission.kind, MissionKind::Attack { .. }));
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
        if index.is_some() || !(free >= minimum || idle) {
            self.waiting = None;
        } else {
            self.waiting.get_or_insert(now);
        }
        let waited = self.waiting.map_or(0, |since| (now - since) / WAIT_STEP);
        let margin = margin(profile.difficulty)
            .saturating_sub(waited * WAIT_RELIEF)
            .max(MARGIN_FLOOR);
        let plan = Plan {
            observation,
            map,
            frame,
            memory,
            minimum,
            margin,
        };
        match index {
            None => self.launch(&plan, &fit, ledger),
            Some(index) => {
                if let Some(failed) = self.advance(index, &plan, &fit, ledger) {
                    memory.abandon(failed.0, failed.1, now);
                }
            }
        }
    }

    fn launch(&mut self, plan: &Plan<'_>, fit: &[&UnitObs], ledger: &mut Ledger) {
        if self.list.len() >= MISSION_CAP {
            return;
        }
        let Some(target) = plan.best(None) else {
            return;
        };
        let Some(rally) = plan.rally(target.owner) else {
            return;
        };
        let need = plan.need(target);
        let component = plan.map.component(target.approach);
        let fit: Vec<&UnitObs> = fit
            .iter()
            .copied()
            .filter(|unit| reaches(plan.map, unit, component))
            .collect();
        if fit.iter().map(|unit| striking(unit)).sum::<u64>() < need {
            return;
        }
        let recruits = recruit(plan.frame, &fit, rally, need, UNIT_CAP);
        if ledger.order(hunt(recruits.clone(), rally)) {
            self.list.push(Mission {
                id: self.next,
                kind: target.kind(),
                phase: Phase::Gather,
                since: plan.observation.tick,
                units: recruits,
                goal: rally,
            });
            self.next += 1;
            self.waiting = None;
        }
    }

    /// Moves the attack at `index` through its phases. Returns the target's
    /// kind and anchor when the attack gave up on it.
    fn advance(
        &mut self,
        index: usize,
        plan: &Plan<'_>,
        fit: &[&UnitObs],
        ledger: &mut Ledger,
    ) -> Option<(BuildingKind, TilePos)> {
        let observation = plan.observation;
        let now = observation.tick;
        let mission = &self.list[index];
        let MissionKind::Attack {
            owner,
            building,
            anchor,
        } = mission.kind
        else {
            unreachable!("the attack mission's kind");
        };
        let rally = plan.rally(owner)?;
        let phase = mission.phase;

        if phase != Phase::Engage {
            let wounded: Vec<UnitId> = mission
                .units
                .iter()
                .copied()
                .filter(|id| mine(observation, *id).is_some_and(|unit| !healthy(unit, WOUNDED)))
                .collect();
            if !wounded.is_empty() && ledger.order(run(wounded.clone(), rally)) {
                self.release(&wounded);
                if !self
                    .list
                    .iter()
                    .any(|mission| matches!(mission.kind, MissionKind::Attack { .. }))
                {
                    return None;
                }
            }
        }
        let index = self
            .list
            .iter()
            .position(|mission| matches!(mission.kind, MissionKind::Attack { .. }))?;

        let current = plan.target(owner, building, anchor);
        let target = if matches!(phase, Phase::Gather | Phase::Recover) {
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
            if matches!(phase, Phase::Gather | Phase::Recover) {
                self.list.remove(index);
                return None;
            }
            let mission = &mut self.list[index];
            if ledger.order(run(mission.units.clone(), rally)) {
                mission.phase = Phase::Recover;
                mission.since = now;
                mission.goal = rally;
            }
            return None;
        };
        let need = plan.need(target);
        let component = plan.map.component(target.approach);
        let mission = &mut self.list[index];
        mission.kind = target.kind();
        let members: Vec<&UnitObs> = mission
            .units
            .iter()
            .filter_map(|id| mine(observation, *id))
            .collect();
        let all_idle = members.iter().all(|unit| unit.idle);
        let strength: u64 = members.iter().map(|unit| value(unit)).sum();
        let age = now - mission.since;

        match phase {
            Phase::Gather | Phase::Recover => {
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
                let ready = striking_strength >= need;
                if !recruited && (all_idle || age >= timeout(phase)) && ready {
                    if ledger.order(hunt(mission.units.clone(), target.approach)) {
                        mission.phase = Phase::Travel;
                        mission.since = now;
                        mission.goal = target.approach;
                    }
                } else if age >= timeout(phase) {
                    self.list.remove(index);
                }
                None
            }
            Phase::Travel => {
                let arrived = members
                    .iter()
                    .any(|unit| unit.tile.chebyshev(target.approach) <= CONTACT_TILES);
                if arrived || contact(observation, &members) {
                    mission.phase = Phase::Engage;
                    mission.since = now;
                    return None;
                }
                if (all_idle || age >= TRAVEL_TICKS)
                    && ledger.order(run(mission.units.clone(), rally))
                {
                    mission.phase = Phase::Recover;
                    mission.since = now;
                    mission.goal = rally;
                    return Some((target.building, target.anchor));
                }
                None
            }
            Phase::Engage => {
                if strength < plan.opposition(&members) {
                    if ledger.order(run(mission.units.clone(), rally)) {
                        mission.phase = Phase::Withdraw;
                        mission.since = now;
                        mission.goal = rally;
                        return Some((target.building, target.anchor));
                    }
                    return None;
                }
                if !all_idle && age < ENGAGE_TICKS {
                    return None;
                }
                let standing = plan.standing(target);
                let next = plan.best(Some(target)).filter(|next| {
                    !standing
                        && members.iter().map(|unit| striking(unit)).sum::<u64>()
                            >= plan.need(*next)
                });
                if let Some(next) = next {
                    if ledger.order(hunt(mission.units.clone(), next.approach)) {
                        mission.kind = next.kind();
                        mission.phase = Phase::Travel;
                        mission.since = now;
                        mission.goal = next.approach;
                    }
                    return None;
                }
                if ledger.order(run(mission.units.clone(), rally)) {
                    mission.phase = Phase::Recover;
                    mission.since = now;
                    mission.goal = rally;
                    if standing {
                        return Some((target.building, target.anchor));
                    }
                }
                None
            }
            Phase::Withdraw => {
                if all_idle || age >= WITHDRAW_TICKS {
                    mission.phase = Phase::Recover;
                    mission.since = now;
                }
                None
            }
        }
    }
}

impl Plan<'_> {
    /// The best target other than `skip`, or `None`. Known enemy buildings
    /// come first; with none, hostile starts are presumed held.
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
        pick(known).or_else(|| {
            pick(
                self.map
                    .hostiles(self.observation.me)
                    .filter_map(|owner| {
                        let anchor = self.map.start(owner)?;
                        self.target(owner, BuildingKind::Foundry, anchor)
                    })
                    .collect(),
            )
        })
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

    /// Whether `target` may still stand: it is known, or its ground is out of
    /// sight.
    fn standing(&self, target: Target) -> bool {
        let known = self.observation.enemy_buildings.iter().any(|building| {
            (building.player, building.kind, building.anchor)
                == (target.owner, target.building, target.anchor)
        });
        let (width, height) = target.building.base_stats().size;
        let seen = (0..height)
            .any(|dy| (0..width).any(|dx| self.observation.visible(target.anchor.offset(dx, dy))));
        known || !seen
    }

    /// Army value the attack on `target` needs: its known local defense
    /// times the margin, and never under the stance minimum.
    fn need(&self, target: Target) -> u64 {
        let now = self.observation.tick;
        let units: u64 = self
            .memory
            .units()
            .iter()
            .filter(|unit| !unit.kind.stats().weapons.is_empty())
            .filter(|unit| unit.tile.chebyshev(target.approach) <= DEFENSE_TILES)
            .map(|unit| u64::from(unit.kind.stats().cost) * u64::from(unit.confidence(now)) / 1_000)
            .sum();
        let buildings: u64 = self
            .observation
            .enemy_buildings
            .iter()
            .filter(|building| {
                building
                    .kind
                    .base_stats()
                    .weapons
                    .iter()
                    .any(|weapon| weapon.targets.ground)
            })
            .filter(|building| {
                gap(
                    building.anchor,
                    building.kind.base_stats().size,
                    target.approach,
                    (1, 1),
                ) < DEFENSE_TILES
            })
            .map(building_value)
            .sum();
        ((units + buildings) * self.margin / 1_000).max(self.minimum)
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
            .map(|unit| u64::from(unit.kind.stats().cost) * u64::from(unit.confidence(now)) / 1_000)
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

/// The tile beside the footprint of `building` at `anchor` nearest the
/// seat's start by ground, or `None` if no ground route reaches it.
fn approach(
    map: &MapModel,
    me: PlayerId,
    frame: HomeFrame,
    building: BuildingKind,
    anchor: TilePos,
) -> Option<TilePos> {
    let (width, height) = building.base_stats().size;
    (-1..=height)
        .flat_map(|dy| (-1..=width).map(move |dx| (dx, dy)))
        .filter(|(dx, dy)| !(0..width).contains(dx) || !(0..height).contains(dy))
        .map(|(dx, dy)| anchor.offset(dx, dy))
        .filter(|tile| map.distance(me, *tile) != UNREACHABLE)
        .min_by_key(|tile| {
            (
                map.distance(me, *tile),
                frame.rank(frame.home, doubled(*tile)),
            )
        })
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

/// A known building's price, discounted by its missing health.
fn building_value(building: &BuildingObs) -> u64 {
    let stats = building.kind.base_stats();
    let cost = stats
        .construction
        .as_ref()
        .map_or(0, |construction| construction.cost);
    u64::from(cost) * u64::from(building.hp) / u64::from(stats.max_hp.max(1))
}

/// Units nearest `rally` first until their value reaches `need`, at most
/// `room` of them, by id.
fn recruit(
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
fn striking(unit: &UnitObs) -> u64 {
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

fn healthy(unit: &UnitObs, health: u32) -> bool {
    u64::from(unit.hp) * 1_000 >= u64::from(unit.kind.stats().max_hp) * u64::from(health)
}

/// Whether `unit` can get to a target whose approach lies in `component`.
fn reaches(map: &MapModel, unit: &UnitObs, component: Option<u32>) -> bool {
    unit.kind.stats().domain == Domain::Air || map.component(unit.tile) == component
}

/// Army value under which the seat does not attack.
fn minimum(stance: BotStance) -> u64 {
    match stance {
        BotStance::Turtle => 800,
        BotStance::Balanced => 600,
        BotStance::Aggressive => 450,
    }
}

/// Per mille of a target's known defense the army must bring.
fn margin(difficulty: BotDifficulty) -> u64 {
    match difficulty {
        BotDifficulty::Scrapheap => 2_500,
        BotDifficulty::Standard => 2_000,
        BotDifficulty::Veteran => 1_750,
        BotDifficulty::Prime => 1_500,
    }
}

fn run(units: Vec<UnitId>, goal: TilePos) -> Command {
    Command::Run {
        units,
        goal,
        queue: false,
    }
}
