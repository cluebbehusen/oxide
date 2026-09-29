//! Missions: units the seat commands together for one purpose, in a phase
//! that persists between decisions so repeated scoring does not replan them.
//! Missions own only units that exist; production never works for one.

use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled, footprint_centre, gap};
use crate::map::{MapModel, UNREACHABLE};
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData, UnitObs};
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingId, BuildingKind, Command, PlayerId, UnitId};
use serde::{Deserialize, Serialize};

mod attack;
mod focus;
mod scouting;

pub(crate) use scouting::points;

/// Missions the seat runs at once.
const MISSION_CAP: usize = 16;

/// Units one mission holds.
const UNIT_CAP: usize = 256;

/// Empty tiles between an enemy and an own building inside which the enemy
/// threatens it, unless its weapon reaches further.
const THREAT_GAP: i32 = 8;

/// Ticks a defense waits without a threat before letting its units go.
const QUIET_TICKS: u64 = 120;

/// Ticks a defense may stay engaged before evaluation calls it stuck.
const ENGAGE_TICKS: u64 = 3_600;

/// Tiles a threat may move before its defenders are sent after it again.
const RETARGET_TILES: i32 = 3;

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
    kind: MissionKind,
    phase: Phase,
    /// Tick the phase began.
    since: u64,
    /// Members, by id.
    units: Vec<UnitId>,
    /// Where the members were last sent.
    goal: TilePos,
    /// The enemy the members were last told to focus, while engaged.
    focus: Option<UnitId>,
}

/// What a mission is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mission", rename_all = "snake_case", deny_unknown_fields)]
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
}

/// Where a mission stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
        }
    }

    /// Ticks `phase` should end within.
    fn timeout(self, phase: Phase) -> u64 {
        match (self, phase) {
            (Self::Defend { .. }, Phase::Recover) => QUIET_TICKS,
            (Self::Attack { .. }, phase) => attack::timeout(phase),
            (Self::Scout { .. }, _) => scouting::TRAVEL_TICKS,
            (Self::Defend { .. }, _) => ENGAGE_TICKS,
        }
    }

    /// Whether a mission of this kind can be in `phase`.
    fn allows(self, phase: Phase) -> bool {
        match self {
            Self::Defend { .. } => matches!(phase, Phase::Engage | Phase::Recover),
            Self::Attack { .. } => true,
            Self::Scout { .. } => phase == Phase::Travel,
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
        }
    }
}

impl Mission {
    /// Whether the mission keeps its units from a defense: everything but a
    /// recovering defense, and an attack only once it is fighting. A
    /// travelling attack may have met the enemy since the last decision. A
    /// scout keeps its scout.
    fn holds(&self, observation: &ObservationData) -> bool {
        match self.kind {
            MissionKind::Defend { .. } => self.phase != Phase::Recover,
            MissionKind::Attack { .. } => match self.phase {
                Phase::Engage => true,
                Phase::Travel => {
                    let members: Vec<&UnitObs> = self
                        .units
                        .iter()
                        .filter_map(|id| mine(observation, *id))
                        .collect();
                    attack::contact(observation, &members)
                }
                Phase::Gather | Phase::Withdraw | Phase::Recover => false,
            },
            MissionKind::Scout { .. } => true,
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
                kind: mission.kind,
                phase: mission.phase,
                since: mission.since,
                timeout: mission.kind.timeout(mission.phase),
                units: mission.units.len() as u32,
                goal: mission.goal,
            })
            .collect()
    }

    /// Drops members that are gone, and missions left without members or
    /// without the Foundry they defend.
    pub(crate) fn prune(&mut self, observation: &ObservationData) {
        let alive = |id: &UnitId| {
            observation
                .my_units
                .binary_search_by_key(id, |unit| unit.id)
                .is_ok()
        };
        for mission in &mut self.list {
            mission.units.retain(alive);
        }
        self.list.retain(|mission| {
            !mission.units.is_empty()
                && match mission.kind {
                    MissionKind::Defend { asset } => observation
                        .my_buildings
                        .iter()
                        .any(|building| building.id == asset),
                    MissionKind::Attack { .. } | MissionKind::Scout { .. } => true,
                }
        });
    }

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
            let MissionKind::Defend { asset } = mission.kind else {
                continue;
            };
            let threatened = groups.iter().any(|(foundry, _)| foundry.id == asset);
            if !threatened && mission.phase == Phase::Engage {
                // A focus order chases its target; without a fresh Hunt the
                // members would follow a retreating enemy out of the base.
                if mission.focus.take().is_some() {
                    ledger.order(hunt(mission.units.clone(), mission.goal));
                }
                mission.phase = Phase::Recover;
                mission.since = now;
            }
        }
        self.list.retain(|mission| {
            !matches!(mission.kind, MissionKind::Defend { .. })
                || mission.phase != Phase::Recover
                || now < mission.since + QUIET_TICKS
        });

        let mut owned: Vec<UnitId> = self
            .list
            .iter()
            .filter(|mission| mission.holds(observation))
            .flat_map(|mission| mission.units.iter().copied())
            .collect();
        owned.sort_unstable();
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
                |mission| matches!(mission.kind, MissionKind::Defend { asset } if asset == foundry.id),
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
            let mut candidates: Vec<&UnitObs> = observation
                .my_units
                .iter()
                .filter(|unit| owned.binary_search(&unit.id).is_err())
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
                    for id in &recruits {
                        insert(&mut owned, *id);
                    }
                    self.release(&recruits);
                    self.list.push(Mission {
                        id: self.next,
                        kind: MissionKind::Defend { asset: foundry.id },
                        phase: Phase::Engage,
                        since: now,
                        units: recruits,
                        goal,
                        focus: None,
                    });
                    self.next += 1;
                }
                continue;
            };
            let mission = &mut self.list[index];
            // Chasing a threat off the Foundry's ground, such as a flyer over a
            // chasm, would stall every defender each time it moved.
            let resend = mission.phase == Phase::Recover
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
                    insert(&mut owned, *id);
                }
                if resend {
                    mission.goal = goal;
                }
                if mission.phase == Phase::Recover {
                    mission.phase = Phase::Engage;
                    mission.since = now;
                }
                self.release_from_others(index, &recruits);
            }
        }
        short
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

    /// Every unit a mission holds, by id.
    fn owned(&self) -> Vec<UnitId> {
        let mut owned: Vec<UnitId> = self
            .list
            .iter()
            .flat_map(|mission| mission.units.iter().copied())
            .collect();
        owned.sort_unstable();
        owned
    }

    /// Rejects restored missions that could not have been recorded by `now`
    /// on a map of the given size.
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
        let exhausted = self.next == u64::MAX;
        if !ordered || exhausted || self.list.last().is_some_and(|last| last.id >= self.next) {
            return Err("checkpoint mission ids are out of order".into());
        }
        let attacks = self
            .list
            .iter()
            .filter(|mission| matches!(mission.kind, MissionKind::Attack { .. }))
            .count();
        if attacks > 1 || self.waiting.is_some_and(|since| since > now) {
            return Err("checkpoint mission could not have been recorded".into());
        }
        for mission in &self.list {
            if !mission.kind.allows(mission.phase)
                || (mission.focus.is_some() && mission.phase != Phase::Engage)
            {
                return Err("checkpoint mission is in a phase its kind lacks".into());
            }
            let sorted = mission.units.windows(2).all(|pair| pair[0] < pair[1]);
            if !sorted || mission.units.is_empty() || mission.units.len() > UNIT_CAP {
                return Err("checkpoint mission units are malformed".into());
            }
            let on_map =
                |tile: TilePos| (0..width).contains(&tile.x) && (0..height).contains(&tile.y);
            let target_on_map = match mission.kind {
                MissionKind::Attack { anchor, .. } => on_map(anchor),
                MissionKind::Scout { point } => usize::from(point) < points,
                MissionKind::Defend { .. } => true,
            };
            if mission.since > now || !on_map(mission.goal) || !target_on_map {
                return Err("checkpoint mission could not have been recorded".into());
            }
        }
        let owned = self.owned();
        if owned.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("checkpoint missions share a unit".into());
        }
        Ok(())
    }
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
    let (width, height) = building.kind.base_stats().size;
    (-1..=height)
        .flat_map(|dy| (-1..=width).map(move |dx| (dx, dy)))
        .filter(|(dx, dy)| !(0..width).contains(dx) || !(0..height).contains(dy))
        .map(|(dx, dy)| building.anchor.offset(dx, dy))
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
