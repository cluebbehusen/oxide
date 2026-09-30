//! The lift mission: when ground reaches no enemy target, carriers that exist
//! take a payload that exists to a landing on the target's island, set it
//! down, and the landed units fight. The mission owns only its actual
//! carriers and passengers. Production keeps a stock of carriers, and nothing
//! waits on a carrier that is not built.

use super::Scratch;
use super::air::{self, Hazard};
use super::attack::{FIT, defense, healthy, margin, minimum, striking};
use super::{
    LiftPhase, MISSION_CAP, Mission, Missions, Objective, Task, approach, hunt, mine, run, standing,
};
use crate::composition::{self, Role};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, centre_distance, doubled, footprint_centre, gap, ring};
use crate::map::MapModel;
use crate::memory::Memory;
use crate::profile::ResolvedProfile;
use chassis::grid::TilePos;
use oxide_sim::observation::{ObservationData, UnitObs};
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingKind, Command, UnitId, UnitKind};

const LOAD_TICKS: u64 = 1_200;
const FLY_TICKS: u64 = 2_400;
const FIGHT_TICKS: u64 = 3_600;

/// Tiles around a target's footprint searched for a landing.
const LANDING_REACH: i32 = 8;

/// Empty tiles a landing would best leave between itself and the target.
const LANDING_GAP: i32 = 4;

/// Tiles around the landing over which unloaded units spread.
const SPREAD: i32 = 4;

/// Tiles a carrier looks around for open ground to wait over.
const CLEARING: i32 = 4;

/// Tiles around a clearing kept free of other aircraft: a flier that meets
/// one idling beside its goal stops short of it.
const AIRSPACE: i32 = 2;

/// Ticks a lift's `phase` should end within.
pub(super) fn timeout(phase: LiftPhase) -> u64 {
    match phase {
        LiftPhase::Load => LOAD_TICKS,
        LiftPhase::Fly => FLY_TICKS,
        LiftPhase::Fight { .. } => FIGHT_TICKS,
    }
}

/// A place to fly a payload to.
#[derive(Clone, Copy)]
struct Drop {
    target: Objective,
    landing: TilePos,
}

/// The lift under way, as this decision sees it.
struct Flight<'a> {
    index: usize,
    target: Objective,
    landing: TilePos,
    age: u64,
    carriers: Vec<&'a UnitObs>,
    /// Members on the ground: riders waiting to board, or set down.
    grounded: Vec<&'a UnitObs>,
    /// Members aboard the carriers.
    aboard: usize,
    /// Their value against ground.
    loaded: u64,
}

/// What one decision knows about lifting.
struct Lifting<'a> {
    observation: &'a ObservationData,
    map: &'a MapModel,
    frame: HomeFrame,
    memory: &'a Memory,
    profile: &'a ResolvedProfile,
    home: u32,
    /// Known fire against aircraft, which carriers must avoid.
    air: &'a [Hazard],
    /// Known fire against ground, which a landing avoids.
    ground: &'a [Hazard],
    objectives: &'a [Objective],
    severed: bool,
}

impl Missions {
    /// Forms a lift when ground reaches no enemy target and the carriers and
    /// payload the seat has can carry enough, or advances the one under way.
    /// Returns a target the lift gave up on.
    pub(crate) fn lift(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        profile: &ResolvedProfile,
        memory: &Memory,
        scratch: &Scratch,
        ledger: &mut Ledger,
    ) -> Option<(BuildingKind, TilePos)> {
        let frame = scratch.frame;
        let home = map
            .start(observation.me)
            .and_then(|start| map.component(start))?;
        let lifting = Lifting {
            observation,
            map,
            frame,
            memory,
            profile,
            home,
            air: &scratch.air,
            ground: &scratch.ground,
            objectives: &scratch.objectives,
            severed: scratch.severed,
        };
        match self
            .list
            .iter()
            .position(|mission| matches!(mission.task, Task::Lift { .. }))
        {
            None => {
                self.form(&lifting, ledger);
                None
            }
            Some(index) => self.advance_lift(index, &lifting, ledger),
        }
    }

    /// Forms a lift when every free carrier and rider at home together can
    /// meet the best landing's need, sending as many loads as this decision's
    /// orders allow; the rest board on later decisions.
    fn form(&mut self, lifting: &Lifting<'_>, ledger: &mut Ledger) {
        let observation = lifting.observation;
        if self.list.len() >= MISSION_CAP || !lifting.severed {
            return;
        }
        let loads = self.loads(lifting, ledger);
        let value = loads.iter().map(|(_, _, value)| value).sum::<u64>();
        if loads.is_empty() || value < minimum(lifting.profile.stance) {
            return;
        }
        let Some(drop) = lifting.best_drop() else {
            return;
        };
        let need = lifting.need(drop.landing);
        if value < need {
            return;
        }
        let (mut units, _) = send(loads, need, ledger);
        if units.is_empty() {
            return;
        }
        units.sort_unstable();
        self.list.push(Mission {
            id: self.next,
            since: observation.tick,
            units,
            goal: drop.landing,
            task: Task::Lift {
                target: drop.target,
                phase: LiftPhase::Load,
            },
        });
        self.next += 1;
        self.waiting = None;
    }

    /// The free carriers and riders at home a lift could load now: riders
    /// packed into carriers strongest value per slot first, with each load's
    /// value. Carriers over ground riders cannot stand beside move to a
    /// clearing first.
    fn loads(&self, lifting: &Lifting<'_>, ledger: &mut Ledger) -> Vec<(UnitId, Vec<UnitId>, u64)> {
        let observation = lifting.observation;
        let (map, frame) = (lifting.map, lifting.frame);
        let free: Vec<&UnitObs> = self
            .available(observation, false)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .filter(|unit| map.component(unit.tile) == Some(lifting.home))
            .collect();
        let rank = |unit: &&UnitObs| (frame.rank(frame.home, doubled(unit.tile)), unit.id);
        let (mut carriers, covered): (Vec<&UnitObs>, Vec<&UnitObs>) = free
            .iter()
            .copied()
            .filter(|unit| carrier(unit.kind) && unit.idle && unit.cargo == 0)
            .partition(|unit| lifting.open(unit.tile, lifting.home));
        for unit in covered {
            if let Some(tile) = lifting.clearing(unit) {
                ledger.order(run(vec![unit.id], tile));
            }
        }
        carriers.sort_by_key(rank);
        let mut riders: Vec<&UnitObs> = free
            .iter()
            .copied()
            .filter(|unit| rides(unit.kind) && healthy(unit, FIT))
            .collect();
        // Most value per transport slot first, so a few strong units are not
        // crowded out by many weak ones nearer home.
        riders.sort_by_key(|unit| {
            let slots = u64::from(unit.kind.stats().transport_size.max(1));
            (
                std::cmp::Reverse(striking(unit) * 1_000 / slots),
                rank(unit),
            )
        });
        let rooms: Vec<(UnitId, u8)> = carriers
            .iter()
            .map(|unit| (unit.id, unit.kind.stats().transport_capacity))
            .collect();
        pack(&rooms, &riders)
            .into_iter()
            .map(|(carrier, riders)| {
                let value = riders
                    .iter()
                    .filter_map(|id| mine(observation, *id))
                    .map(striking)
                    .sum();
                (carrier, riders, value)
            })
            .collect()
    }

    fn advance_lift(
        &mut self,
        index: usize,
        lifting: &Lifting<'_>,
        ledger: &mut Ledger,
    ) -> Option<(BuildingKind, TilePos)> {
        let observation = lifting.observation;
        let mission = &self.list[index];
        let Task::Lift { target, phase } = mission.task else {
            unreachable!("the lift mission's task");
        };
        let members: Vec<&UnitObs> = mission
            .units
            .iter()
            .filter_map(|id| mine(observation, *id))
            .collect();
        let (carriers, grounded) = members.into_iter().partition(|unit| carrier(unit.kind));
        let aboard: Vec<_> = observation
            .my_carried_units
            .iter()
            .filter(|rider| mission.units.binary_search(&rider.carrier).is_ok())
            .collect();
        let flight = Flight {
            index,
            target,
            landing: mission.goal,
            age: observation.tick - mission.since,
            carriers,
            grounded,
            aboard: aboard.len(),
            loaded: aboard
                .iter()
                .map(|rider| {
                    let stats = rider.kind.stats();
                    if stats.weapons.iter().any(|weapon| weapon.targets.ground) {
                        u64::from(stats.cost) * u64::from(rider.hp) / u64::from(stats.max_hp.max(1))
                    } else {
                        0
                    }
                })
                .sum(),
        };
        match phase {
            LiftPhase::Load => {
                self.board(&flight, lifting, ledger);
                None
            }
            LiftPhase::Fly => {
                self.fly(&flight, lifting, ledger);
                None
            }
            LiftPhase::Fight { .. } => self.fight(&flight, lifting, ledger),
        }
    }

    /// Sends more free carriers and riders while those aboard or walking fall
    /// short of the need, and waits while riders walk to their carriers. Once
    /// none is walking and none was sent, or time runs out, flies with
    /// everyone aboard or at least half the need, and otherwise sets everyone
    /// down and lets them go. A rider that stopped short of its carrier could
    /// not board it and is not sent again.
    fn board(&mut self, flight: &Flight<'_>, lifting: &Lifting<'_>, ledger: &mut Ledger) {
        let waiting = &flight.grounded;
        let need = lifting.need(flight.landing);
        let walking: u64 = waiting
            .iter()
            .filter(|unit| !unit.idle)
            .map(|unit| striking(unit))
            .sum();
        let committed = flight.loaded + walking;
        if committed < need && flight.age < LOAD_TICKS {
            let loads = self.loads(lifting, ledger);
            let (sent, _) = send(loads, need - committed, ledger);
            if !sent.is_empty() {
                let units = &mut self.list[flight.index].units;
                units.extend(sent);
                units.sort_unstable();
                return;
            }
        }
        if waiting.iter().any(|unit| !unit.idle) && flight.age < LOAD_TICKS {
            return;
        }
        let enough = waiting.is_empty() || flight.loaded * 2 >= need;
        if flight.aboard > 0 && enough {
            self.take_off(flight, lifting, ledger);
            return;
        }
        let loaded: Vec<&&UnitObs> = flight
            .carriers
            .iter()
            .filter(|unit| unit.cargo > 0)
            .collect();
        let orders = loaded.len() + usize::from(!waiting.is_empty());
        if (ledger.room() as usize) < orders {
            return;
        }
        for carrier in loaded {
            ledger.order(unload(carrier.id, carrier.tile, false));
        }
        if !waiting.is_empty() {
            ledger.order(Command::Stop {
                units: ids(waiting),
            });
        }
        self.list.remove(flight.index);
    }

    /// Sends every loaded carrier to the landing together in one order, around
    /// known anti-air when the straight line crosses it; each sets its riders
    /// down once there. Riders still walking to a carrier stop, orders
    /// permitting, and are let go.
    fn take_off(&mut self, flight: &Flight<'_>, lifting: &Lifting<'_>, ledger: &mut Ledger) {
        let landing = flight.landing;
        let (loaded, empty): (Vec<&UnitObs>, Vec<&UnitObs>) =
            flight.carriers.iter().partition(|unit| unit.cargo > 0);
        let loaded = ids(&loaded);
        let Some(pad) = lifting.pad(landing) else {
            return;
        };
        let via = lifting.route(pad, landing);
        if ledger.room() < route_orders(via) {
            return;
        }
        fly_to(loaded, via, landing, ledger);
        if !flight.grounded.is_empty() && ledger.room() > 0 {
            ledger.order(Command::Stop {
                units: ids(&flight.grounded),
            });
        }
        let mission = &mut self.list[flight.index];
        if let Task::Lift { phase, .. } = &mut mission.task {
            *phase = LiftPhase::Fly;
        }
        mission.since = lifting.observation.tick;
        let released: Vec<UnitId> = ids(&empty)
            .into_iter()
            .chain(ids(&flight.grounded))
            .collect();
        self.release_units(flight.index, &released);
    }

    /// Sets riders down as each carrier reaches the landing, sends emptied
    /// carriers home together and frees them, sends landed riders at the
    /// target, brings back carriers still loaded when time runs out, and
    /// fights once no one is aboard.
    fn fly(&mut self, flight: &Flight<'_>, lifting: &Lifting<'_>, ledger: &mut Ledger) {
        let map = lifting.map;
        let island = map.component(flight.target.anchor);
        let hunt_at = lifting.hunt_tile(flight.target, flight.landing);
        let pad = lifting.pad(flight.landing);
        let (loaded, empty): (Vec<&UnitObs>, Vec<&UnitObs>) =
            flight.carriers.iter().partition(|unit| unit.cargo > 0);
        let late = flight.age >= FLY_TICKS;
        for carrier in loaded.iter().filter(|unit| unit.idle) {
            if map.component(carrier.tile) == Some(lifting.home) {
                ledger.order(unload(carrier.id, carrier.tile, false));
            } else if late {
                continue;
            } else if carrier.tile.chebyshev(flight.landing) <= SPREAD {
                ledger.order(unload(carrier.id, flight.landing, false));
            } else {
                ledger.order(run(vec![carrier.id], flight.landing));
            }
        }
        let away: Vec<UnitId> = loaded
            .iter()
            .filter(|unit| map.component(unit.tile) != Some(lifting.home))
            .map(|unit| unit.id)
            .collect();
        if late
            && let (false, Some(pad)) = (away.is_empty(), pad)
            && ledger.order(run(away, pad))
        {
            self.list[flight.index].since = lifting.observation.tick;
        }
        let (resting, leaving): (Vec<&UnitObs>, Vec<&UnitObs>) =
            empty.iter().partition(|unit| unit.idle);
        let mut going: Vec<UnitId> = ids(&leaving);
        if let (false, Some(pad)) = (resting.is_empty(), pad) {
            let via = lifting.route(flight.landing, pad);
            if ledger.room() >= route_orders(via) {
                fly_to(ids(&resting), via, pad, ledger);
                going.extend(ids(&resting));
            }
        }
        let (landed, astray): (Vec<&UnitObs>, Vec<&UnitObs>) = flight
            .grounded
            .iter()
            .partition(|unit| island.is_some() && map.component(unit.tile) == island);
        let delivered = loaded.is_empty() && flight.aboard == 0;
        let idle: Vec<&UnitObs> = landed.iter().copied().filter(|unit| unit.idle).collect();
        if let (false, false, Some(hunt_at)) = (delivered, idle.is_empty(), hunt_at) {
            ledger.order(hunt(ids(&idle), hunt_at));
        }
        let released: Vec<UnitId> = going.into_iter().chain(ids(&astray)).collect();
        if !self.release_units(flight.index, &released) || !delivered {
            return;
        }
        if landed.is_empty() {
            self.list.remove(flight.index);
            return;
        }
        let goal = hunt_at.unwrap_or(flight.landing);
        let mut riders = ids(&landed);
        riders.sort_unstable();
        if ledger.order(hunt(riders, goal)) {
            let mission = &mut self.list[flight.index];
            mission.task = Task::Lift {
                target: flight.target,
                phase: LiftPhase::Fight { focus: None },
            };
            mission.since = lifting.observation.tick;
            mission.goal = goal;
        }
    }

    /// Landed riders fight the target, then the next target on the same
    /// ground, and are let go when there is none or time runs out. Returns a
    /// target they stood beside without taking.
    fn fight(
        &mut self,
        flight: &Flight<'_>,
        lifting: &Lifting<'_>,
        ledger: &mut Ledger,
    ) -> Option<(BuildingKind, TilePos)> {
        let target = flight.target;
        if flight.age >= FIGHT_TICKS {
            self.list.remove(flight.index);
            return None;
        }
        if !flight.grounded.iter().all(|unit| unit.idle) {
            return None;
        }
        if standing(lifting.observation, target) {
            self.list.remove(flight.index);
            return Some((target.building, target.anchor));
        }
        let (map, observation, frame) = (lifting.map, lifting.observation, lifting.frame);
        let island = map.component(target.anchor);
        let from = flight.landing;
        let next = lifting
            .objectives
            .iter()
            .copied()
            .filter(|objective| *objective != target)
            .filter(|objective| island.is_some() && map.component(objective.anchor) == island)
            .filter(|objective| {
                !lifting
                    .memory
                    .abandoned(objective.building, objective.anchor, observation.tick)
            })
            .filter_map(|objective| Some((objective, lifting.hunt_tile(objective, from)?)))
            .min_by_key(|(_, tile)| (tile.chebyshev(from), frame.rank(frame.home, doubled(*tile))));
        let Some((next, tile)) = next else {
            self.list.remove(flight.index);
            return None;
        };
        let mut riders = ids(&flight.grounded);
        riders.sort_unstable();
        if ledger.order(hunt(riders, tile)) {
            let mission = &mut self.list[flight.index];
            mission.task = Task::Lift {
                target: next,
                phase: LiftPhase::Fight { focus: None },
            };
            mission.since = observation.tick;
            mission.goal = tile;
        }
        None
    }

    /// Takes `units` out of the mission at `index`, dropping it if empty.
    /// Returns whether it remains.
    fn release_units(&mut self, index: usize, units: &[UnitId]) -> bool {
        self.list[index].units.retain(|id| !units.contains(id));
        if self.list[index].units.is_empty() {
            self.list.remove(index);
            return false;
        }
        true
    }
}

impl Lifting<'_> {
    /// The best target no ground route reaches that has a landing, by value
    /// for its distance from home.
    fn best_drop(&self) -> Option<Drop> {
        targets(
            self.objectives,
            self.observation,
            self.map,
            self.frame,
            self.memory,
        )
        .into_iter()
        .find_map(|target| {
            Some(Drop {
                target,
                landing: self.landing(target)?,
            })
        })
    }

    /// Army value a lift to `landing` needs: its known ground defense times
    /// the margin, and never under the stance minimum.
    fn need(&self, landing: TilePos) -> u64 {
        (defense(self.observation, self.memory, landing) * margin(self.profile.difficulty) / 1_000)
            .max(minimum(self.profile.stance))
    }

    /// Where to set a payload down near `target`: known open ground on the
    /// target's island, clear of known fire, about four tiles from it. The
    /// one reachability check: every ground tile riders could be set down on
    /// lies on the target's island, so none land across a chasm.
    fn landing(&self, target: Objective) -> Option<TilePos> {
        let map = self.map;
        let size = target.building.base_stats().size;
        let island = map.component(target.anchor)?;
        let spread_ok = |tile: TilePos| {
            (-SPREAD..=SPREAD).all(|dy| {
                (-SPREAD..=SPREAD).all(|dx| {
                    let near = tile.offset(dx, dy);
                    map.component(near)
                        .is_none_or(|component| component == island)
                })
            })
        };
        (-LANDING_REACH..size.1 + LANDING_REACH)
            .flat_map(|dy| {
                (-LANDING_REACH..size.0 + LANDING_REACH).map(move |dx| target.anchor.offset(dx, dy))
            })
            .filter(|tile| self.open(*tile, island))
            .filter(|tile| gap(target.anchor, size, *tile, (1, 1)) >= 1 && spread_ok(*tile))
            .min_by_key(|tile| {
                let point = doubled(*tile);
                let exposure: u64 = self
                    .air
                    .iter()
                    .chain(self.ground)
                    .filter(|hazard| hazard.covers(point))
                    .map(|hazard| hazard.value)
                    .sum();
                (
                    exposure,
                    (gap(target.anchor, size, *tile, (1, 1)) - LANDING_GAP).abs(),
                    self.frame.rank(self.frame.home, point),
                )
            })
    }

    /// Whether a ground unit could stand on `tile` of `island` as far as the
    /// seat knows: no known building, its own, an ally's or an enemy's, and
    /// no rock or scrap covers it.
    fn open(&self, tile: TilePos, island: u32) -> bool {
        let observation = self.observation;
        self.map.component(tile) == Some(island)
            && !observation.known_scrap_at(tile)
            && !observation.known_rock_at(tile)
            && !observation
                .enemy_buildings
                .iter()
                .chain(&observation.my_buildings)
                .chain(&observation.ally_buildings)
                .any(|building| {
                    gap(
                        building.anchor,
                        building.kind.base_stats().size,
                        tile,
                        (1, 1),
                    ) < 0
                })
    }

    /// The open home tile nearest `carrier`, away from other aircraft, where
    /// riders can reach a carrier now hovering over a building or other
    /// ground they cannot stand on.
    fn clearing(&self, carrier: &UnitObs) -> Option<TilePos> {
        let tile = carrier.tile;
        let aircraft: Vec<TilePos> = self
            .observation
            .my_units
            .iter()
            .filter(|unit| unit.id != carrier.id && unit.kind.stats().domain == Domain::Air)
            .map(|unit| unit.tile)
            .collect();
        (-CLEARING..=CLEARING)
            .flat_map(|dy| (-CLEARING..=CLEARING).map(move |dx| tile.offset(dx, dy)))
            .filter(|near| self.open(*near, self.home))
            .filter(|near| {
                aircraft
                    .iter()
                    .all(|other| other.chebyshev(*near) > AIRSPACE)
            })
            .min_by_key(|near| {
                (
                    near.chebyshev(tile),
                    self.frame.rank(self.frame.home, doubled(*near)),
                )
            })
    }

    fn pad(&self, landing: TilePos) -> Option<TilePos> {
        air::pad(self.observation, self.map, self.frame, doubled(landing))
    }

    fn route(&self, from: TilePos, to: TilePos) -> Option<TilePos> {
        air::route(self.observation, self.frame, self.air, from, to)
    }

    /// The tile beside `target` on its island nearest `from`.
    fn hunt_tile(&self, target: Objective, from: TilePos) -> Option<TilePos> {
        let island = self.map.component(target.anchor)?;
        ring(target.anchor, target.building.base_stats().size)
            .filter(|tile| self.map.component(*tile) == Some(island))
            .min_by_key(|tile| {
                (
                    tile.chebyshev(from),
                    self.frame.rank(self.frame.home, doubled(*tile)),
                )
            })
    }
}

/// Issues `loads` in order while the decision has orders and those sent so
/// far fall short of `need`, returning the carriers and riders sent and
/// their value.
fn send(
    loads: Vec<(UnitId, Vec<UnitId>, u64)>,
    need: u64,
    ledger: &mut Ledger,
) -> (Vec<UnitId>, u64) {
    let mut units = Vec::new();
    let mut value = 0;
    for (carrier, riders, worth) in loads {
        if value >= need
            || !ledger.order(Command::Load {
                units: riders.clone(),
                transport: carrier,
                queue: false,
            })
        {
            break;
        }
        units.push(carrier);
        units.extend(riders);
        value += worth;
    }
    (units, value)
}

/// Orders [`fly_to`] issues.
fn route_orders(via: Option<TilePos>) -> u32 {
    1 + u32::from(via.is_some())
}

/// Sends `carriers` to `goal` in one group order, or around known anti-air by
/// `via` first in two.
fn fly_to(carriers: Vec<UnitId>, via: Option<TilePos>, goal: TilePos, ledger: &mut Ledger) {
    match via {
        Some(via) => {
            ledger.order(run(carriers.clone(), via));
            ledger.order(Command::Run {
                units: carriers,
                goal,
                queue: true,
            });
        }
        None => {
            ledger.order(run(carriers, goal));
        }
    }
}

/// Carriers the seat wants for its next lift: enough to carry what the best
/// landing needs, or the stance minimum while none is known, at the value per
/// slot of the riders at home, but no more than those riders fill. At least
/// one.
pub(crate) fn carriers_wanted(
    observation: &ObservationData,
    map: &MapModel,
    profile: &ResolvedProfile,
    memory: &Memory,
    scratch: &Scratch,
) -> u64 {
    let frame = scratch.frame;
    let (value, slots) = scratch.payload;
    let line = UnitKind::Sentinel.stats();
    let per_slot = value
        .checked_div(slots)
        .unwrap_or(u64::from(line.cost) / u64::from(line.transport_size.max(1)))
        .max(1);
    // The landing lies a few tiles from its target, so the defense around the
    // target stands in for it without searching for one.
    let need = targets(&scratch.objectives, observation, map, frame, memory)
        .first()
        .map_or(0, |target| {
            defense(observation, memory, target.anchor) * margin(profile.difficulty) / 1_000
        })
        .max(minimum(profile.stance));
    let capacity = u64::from(UnitKind::Skyhook.stats().transport_capacity).max(1);
    need.div_ceil(per_slot)
        .min(slots)
        .max(capacity)
        .div_ceil(capacity)
}

/// The targets no ground route reaches and not given up on, most valuable for
/// their distance from home first.
fn targets(
    objectives: &[Objective],
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    memory: &Memory,
) -> Vec<Objective> {
    let Some(start) = map.start(observation.me) else {
        return Vec::new();
    };
    let mut ranked = objectives
        .iter()
        .copied()
        .filter(|objective| {
            approach(
                map,
                observation.me,
                frame,
                objective.building,
                objective.anchor,
            )
            .is_none()
        })
        .filter(|objective| {
            !memory.abandoned(objective.building, objective.anchor, observation.tick)
        })
        .map(|target| {
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
            let score = u64::from(cost) * 1_000 / (100 + distance);
            let rank = frame.rank(frame.home, footprint_centre(target.building, target.anchor));
            ((std::cmp::Reverse(score), rank), target)
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(order, _)| *order);
    ranked.into_iter().map(|(_, target)| target).collect()
}

/// Assigns `riders` in order to the first carrier with room, each carrier
/// given as its id and remaining room. Carriers left empty are omitted.
fn pack(carriers: &[(UnitId, u8)], riders: &[&UnitObs]) -> Vec<(UnitId, Vec<UnitId>)> {
    let mut rooms: Vec<(UnitId, u8, Vec<UnitId>)> = carriers
        .iter()
        .map(|(id, room)| (*id, *room, Vec::new()))
        .collect();
    for rider in riders {
        let size = rider.kind.stats().transport_size;
        if let Some(slot) = rooms.iter_mut().find(|(_, room, _)| *room >= size) {
            slot.1 -= size;
            slot.2.push(rider.id);
        }
    }
    rooms
        .into_iter()
        .filter(|(_, _, riders)| !riders.is_empty())
        .map(|(id, _, mut riders)| {
            riders.sort_unstable();
            (id, riders)
        })
        .collect()
}

/// Whether `kind` carries others.
pub(crate) fn carrier(kind: UnitKind) -> bool {
    kind.stats().transport_capacity > 0
}

/// The value against ground and the transport slots of the units at home a
/// lift could take now.
pub(crate) fn payload(observation: &ObservationData, map: &MapModel) -> (u64, u64) {
    let home = map
        .start(observation.me)
        .and_then(|start| map.component(start));
    observation
        .my_units
        .iter()
        .filter(|unit| rides(unit.kind) && healthy(unit, FIT) && map.component(unit.tile) == home)
        .fold((0, 0), |(value, slots), unit| {
            (
                value + striking(unit),
                slots + u64::from(unit.kind.stats().transport_size),
            )
        })
}

/// Whether `kind` is a line or siege unit a carrier can take.
fn rides(kind: UnitKind) -> bool {
    matches!(composition::role(kind), Some(Role::Line | Role::Siege))
        && kind.stats().transport_size > 0
}

fn ids(units: &[&UnitObs]) -> Vec<UnitId> {
    units.iter().map(|unit| unit.id).collect()
}

fn unload(transport: UnitId, at: TilePos, queue: bool) -> Command {
    Command::Unload {
        transport,
        at,
        queue,
    }
}
