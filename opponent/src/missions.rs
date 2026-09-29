//! Missions: units the seat commands together for one purpose, in a phase
//! that persists between decisions so repeated scoring does not replan them.
//! Missions own only units that exist; production never works for one.

use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled, footprint_centre, gap};
use crate::map::MapModel;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData, UnitObs};
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingId, BuildingKind, Command, UnitId};
use serde::{Deserialize, Serialize};

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
}

/// Where a mission stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Fighting what it was formed for.
    Engage,
    /// Its purpose is gone for now; it waits before letting its units go.
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
        }
    }
}

impl Phase {
    /// A short name for reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Engage => "engage",
            Self::Recover => "recover",
        }
    }

    fn timeout(self) -> u64 {
        match self {
            Self::Engage => ENGAGE_TICKS,
            Self::Recover => QUIET_TICKS,
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
                timeout: mission.phase.timeout(),
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
                }
        });
    }

    /// Answers every threatened Foundry: recruits free units that can hit its
    /// threats until they outweigh them by half again, and sends them at the
    /// threat nearest the Foundry. Returns whether any defense stayed short.
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
            let MissionKind::Defend { asset } = mission.kind;
            let threatened = groups.iter().any(|(foundry, _)| foundry.id == asset);
            if !threatened && mission.phase == Phase::Engage {
                mission.phase = Phase::Recover;
                mission.since = now;
            }
        }
        self.list
            .retain(|mission| mission.phase != Phase::Recover || now < mission.since + QUIET_TICKS);

        let mut owned = self.owned();
        let mut short = false;
        for (foundry, threats) in &groups {
            let centre = footprint_centre(foundry.kind, foundry.anchor);
            let goal = threats
                .iter()
                .min_by_key(|threat| frame.rank(centre, doubled(threat.tile)))
                .expect("a group holds a threat")
                .tile;
            let need = threats.iter().map(|threat| value(threat)).sum::<u64>() * 3 / 2;
            let index = self.list.iter().position(
                |mission| matches!(mission.kind, MissionKind::Defend { asset } if asset == foundry.id),
            );
            let members = index.map_or(&[][..], |index| &self.list[index].units);
            let mut have: u64 = members
                .iter()
                .filter_map(|id| mine(observation, *id))
                .map(value)
                .sum();
            let room = UNIT_CAP - members.len();
            let component = map.component(foundry.anchor);
            let mut candidates: Vec<&UnitObs> = observation
                .my_units
                .iter()
                .filter(|unit| owned.binary_search(&unit.id).is_err())
                .filter(|unit| can_hit_any(unit, threats))
                .filter(|unit| {
                    unit.kind.stats().domain == Domain::Air || map.component(unit.tile) == component
                })
                .collect();
            candidates.sort_by_key(|unit| (frame.rank(centre, doubled(unit.tile)), unit.id));
            let mut recruits = Vec::new();
            for unit in candidates.into_iter().take(room) {
                if have >= need {
                    break;
                }
                have += value(unit);
                recruits.push(unit.id);
            }
            short |= have < need;

            let Some(index) = index else {
                if recruits.is_empty() || self.list.len() == MISSION_CAP {
                    continue;
                }
                recruits.sort_unstable();
                if ledger.order(hunt(recruits.clone(), goal)) {
                    for id in &recruits {
                        insert(&mut owned, *id);
                    }
                    self.list.push(Mission {
                        id: self.next,
                        kind: MissionKind::Defend { asset: foundry.id },
                        phase: Phase::Engage,
                        since: now,
                        units: recruits,
                        goal,
                    });
                    self.next += 1;
                }
                continue;
            };
            let mission = &mut self.list[index];
            let resend =
                mission.phase == Phase::Recover || mission.goal.chebyshev(goal) > RETARGET_TILES;
            let mut sent = recruits.clone();
            if resend {
                sent.extend_from_slice(&mission.units);
            }
            if sent.is_empty() {
                continue;
            }
            sent.sort_unstable();
            if ledger.order(hunt(sent, goal)) {
                for id in recruits {
                    insert(&mut mission.units, id);
                    insert(&mut owned, id);
                }
                if resend {
                    mission.goal = goal;
                }
                if mission.phase == Phase::Recover {
                    mission.phase = Phase::Engage;
                    mission.since = now;
                }
            }
        }
        short
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
    pub(crate) fn validate(&self, now: u64, width: i32, height: i32) -> Result<(), String> {
        if self.list.len() > MISSION_CAP {
            return Err("checkpoint holds too many missions".into());
        }
        let ordered = self.list.windows(2).all(|pair| pair[0].id < pair[1].id);
        if !ordered || self.list.last().is_some_and(|last| last.id >= self.next) {
            return Err("checkpoint mission ids are out of order".into());
        }
        for mission in &self.list {
            let sorted = mission.units.windows(2).all(|pair| pair[0] < pair[1]);
            if !sorted || mission.units.is_empty() || mission.units.len() > UNIT_CAP {
                return Err("checkpoint mission units are malformed".into());
            }
            let on_map =
                (0..width).contains(&mission.goal.x) && (0..height).contains(&mission.goal.y);
            if mission.since > now || !on_map {
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

fn hunt(units: Vec<UnitId>, goal: TilePos) -> Command {
    Command::Hunt {
        units,
        goal,
        queue: false,
    }
}
