//! Workers (Harvesters and Excavators): how many the seat wants, training
//! them, keeping them out of harm, and sending free ones to build, weld and
//! harvest.

use crate::decision::{Ledger, Producer};
use crate::frame::{HomeFrame, doubled, footprint_centre, gap, ring};
use crate::map::MapModel;
use crate::missions::{Hazard, Scratch};
use crate::profile::ResolvedProfile;
use chassis::fx::Fx;
use chassis::grid::{Grid, TilePos};
use oxide_sim::TICKS_PER_SECOND;
use oxide_sim::ids::Target;
use oxide_sim::observation::{BuildingObs, ObservationData, UnitObs};
use oxide_sim::scenario::BotStance;
use oxide_sim::stats::FOUNDRY_REPAIR_PRICE;
use oxide_sim::{BuildingKind, Command, PlayerId, UnitId, UnitKind};
use std::cmp::Reverse;

/// Per mille of its health a building must miss before workers weld it.
const WELD_DAMAGE: u32 = 100;

/// Scrap the seat keeps spendable before starting a weld, which bills as it
/// goes: a weld started with less soon stalls for want of scrap.
pub(crate) const WELD_FLOOR: u32 = 100;

/// Empty tiles around a building inside which an armed enemy in sight keeps
/// workers from welding it.
const WELD_CLEARANCE: i32 = 12;

/// Empty tiles from an own Foundry inside which a worker counts as home.
const HOME_TILES: i32 = 8;

/// The nodes the seat's Foundries work, each with the Harvesters it wants
/// there, and the worker slots it fills.
pub(crate) struct Staffing {
    worked: Vec<(TilePos, usize)>,
    /// Known nodes with scrap whose route from home avoids known danger, in
    /// `known_scrap` order.
    reachable: Vec<TilePos>,
    crew: usize,
}

impl Staffing {
    /// The worked nodes with the Harvesters each wants.
    #[cfg(test)]
    pub(crate) fn crews(&self) -> &[(TilePos, usize)] {
        &self.worked
    }

    fn wanted(&self) -> usize {
        self.worked.iter().map(|(_, crew)| crew).sum()
    }

    /// Filled worker slots as a per-mille share of those wanted.
    pub(crate) fn saturation(&self) -> u32 {
        let wanted = self.wanted();
        if wanted == 0 {
            return 1_000;
        }
        u32::try_from(self.crew.min(wanted) * 1_000 / wanted)
            .expect("a per-mille share fits in u32")
    }
}

/// Whether `kind` harvests, builds and welds: a Harvester or an Excavator.
pub(crate) fn worker(kind: UnitKind) -> bool {
    kind.stats().harvest.is_some()
}

/// Harvester slots a worker of `kind` fills: its mining rate over a
/// Harvester's, so an Excavator, twice as fast, fills two.
fn slots(kind: UnitKind) -> usize {
    let per_scrap = |kind: UnitKind| kind.stats().harvest.map(|harvest| harvest.ticks_per_scrap);
    match (per_scrap(UnitKind::Harvester), per_scrap(kind)) {
        (Some(harvester), Some(ticks)) => (harvester / ticks.max(1)).max(1) as usize,
        _ => 0,
    }
}

/// Counts the worked nodes with their crews and the worker slots filled by
/// workers alive, carried or queued.
pub(crate) fn staffing(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    profile: &ResolvedProfile,
    foundries: &[Producer<'_>],
    scratch: &Scratch,
) -> Staffing {
    let homes: Vec<&BuildingObs> = foundries.iter().map(|foundry| foundry.building).collect();
    let reachable = reachable(observation, map, frame, scratch, &homes);
    Staffing {
        worked: worked_nodes(observation, map, frame, profile, foundries, &reachable),
        reachable,
        crew: crew(observation),
    }
}

/// With no worker alive or queued, the nearest Foundry queues a Harvester
/// even behind other work, from protected scrap if it must.
pub(crate) fn recover(
    observation: &ObservationData,
    foundries: &[Producer<'_>],
    ledger: &mut Ledger,
) {
    if crew(observation) > 0 {
        return;
    }
    if let Some(foundry) = foundries.first() {
        ledger.train_urgently(foundry.building.id, UnitKind::Harvester);
    }
}

/// Trains workers at ready Foundries until the worked nodes' crews are full,
/// counting those alive and queued, spending at most `budget` on them.
pub(crate) fn train(
    observation: &ObservationData,
    foundries: &[Producer<'_>],
    staffing: &Staffing,
    greed: u8,
    mut budget: u64,
    ledger: &mut Ledger,
) {
    let mut count = staffing.crew
        + ledger.queued(UnitKind::Harvester)
        + slots(UnitKind::Excavator) * ledger.queued(UnitKind::Excavator);
    for foundry in foundries {
        if count >= staffing.wanted() {
            break;
        }
        if !foundry.ready || ledger.queued_at(foundry.building.id) {
            continue;
        }
        let kind = if excavate(observation, staffing.wanted() - count, greed, ledger) {
            UnitKind::Excavator
        } else {
            UnitKind::Harvester
        };
        let cost = u64::from(kind.stats().cost);
        if cost > budget {
            break;
        }
        if ledger.train(foundry.building.id, kind) {
            count += slots(kind);
            budget -= cost;
        }
    }
}

/// Brings workers away from home back out of known enemy fire, sends a
/// worker to each unattended construction site, welds damaged buildings,
/// then sends idle workers to the least-worked node whose route and site are
/// clear of known danger.
pub(crate) fn run(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    hazards: &[Hazard],
    staffing: &Staffing,
    ledger: &mut Ledger,
) {
    flee(observation, map, frame, hazards, ledger);
    resume_sites(observation, map, frame, ledger);
    weld(observation, map, frame, hazards, ledger);
    assign_idle(observation, map, frame, staffing, hazards, ledger);
}

/// Whether the next worker is an Excavator: once a Fabricator stands, when
/// two Harvesters' slots are open and the seat can pay with scrap to spare,
/// less to spare the greedier it is.
fn excavate(observation: &ObservationData, open: usize, greed: u8, ledger: &Ledger) -> bool {
    let fabricator = observation
        .my_buildings
        .iter()
        .any(|building| building.kind == BuildingKind::Fabricator && building.built);
    let spare = 2 * (100 - u32::from(greed.min(100)));
    fabricator
        && open >= slots(UnitKind::Excavator)
        && ledger.spendable() >= UnitKind::Excavator.stats().cost + spare
}

/// Sends each harvesting or idle worker away from home that known enemy fire
/// reaches back beside the nearest own Foundry on its ground. An explicit
/// harvest order keeps a worker at its node under fire until it is told
/// otherwise.
fn flee(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    hazards: &[Hazard],
    ledger: &mut Ledger,
) {
    let foundries = built_foundries(observation);
    if hazards.is_empty() {
        return;
    }
    let size = BuildingKind::Foundry.size();
    for unit in &observation.my_units {
        let working = unit.idle || unit.harvesting.is_some();
        if !worker(unit.kind) || !working || ledger.employs(unit.id) {
            continue;
        }
        let threatened = hazards
            .iter()
            .any(|hazard| hazard.covers(doubled(unit.tile)));
        let home = foundries
            .iter()
            .any(|foundry| gap(foundry.anchor, size, unit.tile, (1, 1)) <= HOME_TILES);
        if !threatened || home {
            continue;
        }
        let Some(refuge) = refuge(map, frame, &foundries, unit.tile) else {
            continue;
        };
        if !ledger.order(Command::Run {
            units: vec![unit.id],
            goal: refuge,
            queue: false,
        }) {
            return;
        }
    }
}

/// The seat's built Foundries.
fn built_foundries(observation: &ObservationData) -> Vec<&BuildingObs> {
    observation
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry && building.built)
        .collect()
}

/// The tile beside one of `foundries` on the ground of `tile` nearest it.
fn refuge(
    map: &MapModel,
    frame: HomeFrame,
    foundries: &[&BuildingObs],
    tile: TilePos,
) -> Option<TilePos> {
    let size = BuildingKind::Foundry.size();
    let ground = map.component(tile)?;
    let from = doubled(tile);
    foundries
        .iter()
        .flat_map(|foundry| ring(foundry.anchor, size))
        .filter(|beside| map.component(*beside) == Some(ground))
        .min_by_key(|beside| (frame.rank(from, doubled(*beside)), *beside))
}

/// Sends the nearest free worker to weld each damaged own building nobody
/// welds yet, most missing value first, while no armed enemy in sight stands
/// near it, no known enemy weapon reaches it, and the seat has scrap to pay.
fn weld(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    hazards: &[Hazard],
    ledger: &mut Ledger,
) {
    // Buildings a worker already welds.
    let tended: Vec<oxide_sim::BuildingId> = observation
        .my_units
        .iter()
        .filter(|unit| worker(unit.kind) && unit.repairing)
        .filter_map(|unit| match observation.repair_target(unit.id)? {
            Target::Building(building) => Some(building),
            Target::Unit(_) => None,
        })
        .collect();
    let mut patients: Vec<(u64, (i64, i64), &BuildingObs)> = observation
        .my_buildings
        .iter()
        .filter(|building| building.built && !building.provisional)
        .filter(|building| !tended.contains(&building.id))
        .filter_map(|building| {
            let stats = building.kind.tier_stats(building.tier);
            let (hp, max) = (u64::from(building.hp), u64::from(stats.max_hp.max(1)));
            if hp * 1_000 > max * u64::from(1_000 - WELD_DAMAGE) {
                return None;
            }
            let size = building.kind.size();
            let threatened = observation.enemy_units.iter().any(|enemy| {
                enemy.kind.stats().can_fight()
                    && gap(building.anchor, size, enemy.tile, (1, 1)) <= WELD_CLEARANCE
            });
            let centre = footprint_centre(building.kind, building.anchor);
            if threatened || hazards.iter().any(|hazard| hazard.covers(centre)) {
                return None;
            }
            let price = building
                .kind
                .base_stats()
                .construction
                .as_ref()
                .map_or(FOUNDRY_REPAIR_PRICE, |construction| construction.cost);
            let missing = u64::from(price) * (max - hp) / max;
            Some((missing, centre, building))
        })
        .collect();
    patients.sort_by_key(|(missing, centre, building)| {
        (
            Reverse(*missing),
            frame.rank(frame.home, *centre),
            building.id,
        )
    });
    for (_, centre, building) in patients {
        if ledger.spendable() < WELD_FLOOR {
            return;
        }
        let Some(welder) = builder(observation, map, frame, building.anchor, centre, ledger) else {
            continue;
        };
        if !ledger.order(Command::Repair {
            units: vec![welder],
            building: building.id,
            queue: false,
        }) {
            return;
        }
    }
}

/// The nearest worker to `centre` that stands on the same ground as `site`,
/// is not constructing or welding, and has no work from this decision yet.
pub(crate) fn builder(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    site: TilePos,
    centre: (i64, i64),
    ledger: &Ledger,
) -> Option<UnitId> {
    let ground = map.component(site)?;
    observation
        .my_units
        .iter()
        .filter(|unit| {
            worker(unit.kind)
                && unit.site.is_none()
                && unit.founding.is_none()
                && !unit.repairing
                && map.component(unit.tile) == Some(ground)
                && !ledger.employs(unit.id)
                && !ledger.stuck(unit.id)
        })
        .min_by_key(|unit| (frame.rank(centre, doubled(unit.tile)), unit.id))
        .map(|unit| unit.id)
}

/// Sends the nearest free worker to every paid base-tier site nobody is
/// building. Upgrades rebuild themselves and provisional scaffolds already
/// have their founder.
fn resume_sites(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    ledger: &mut Ledger,
) {
    for site in &observation.my_buildings {
        let attended = observation
            .my_units
            .iter()
            .any(|unit| unit.site == Some(site.id));
        if site.built || site.provisional || site.tier > 0 || attended {
            continue;
        }
        let centre = footprint_centre(site.kind, site.anchor);
        let Some(builder) = builder(observation, map, frame, site.anchor, centre, ledger) else {
            continue;
        };
        let command = Command::Build {
            units: vec![builder],
            kind: site.kind,
            anchor: site.anchor,
            queue: false,
            defer: false,
        };
        if !ledger.order(command) {
            return;
        }
    }
}

/// Worker slots filled by workers alive, carried or queued.
fn crew(observation: &ObservationData) -> usize {
    observation
        .my_units
        .iter()
        .map(|unit| unit.kind)
        .chain(observation.my_carried_units.iter().map(|unit| unit.kind))
        .chain(observation.my_queues.iter().flatten().copied())
        .map(slots)
        .sum()
}

/// The live known nodes the seat works, each with its crew. A node belongs
/// to the nearest built Foundry whose ground reaches it, is worked when its
/// route from home avoids known danger and a Harvester hauling from there
/// repays its price within the seat's horizon. Its crew fills the free tiles
/// around it that its remaining scrap repays, as full as stance and greed
/// make it.
fn worked_nodes(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    profile: &ResolvedProfile,
    foundries: &[Producer<'_>],
    reachable: &[TilePos],
) -> Vec<(TilePos, usize)> {
    let harvester = UnitKind::Harvester.stats();
    let Some(harvest) = harvester.harvest else {
        return Vec::new();
    };
    let horizon = horizon(profile);
    let fill = fill(profile);
    let price = u64::from(harvester.cost.max(1));
    let payback = price.div_ceil(u64::from(harvest.capacity.max(1)));
    let homes: Vec<(u32, (i64, i64), &BuildingObs)> = foundries
        .iter()
        .filter_map(|foundry| {
            let component = map.component(foundry.building.anchor)?;
            let centre = footprint_centre(BuildingKind::Foundry, foundry.building.anchor);
            Some((component, centre, foundry.building))
        })
        .collect();
    observation
        .known_scrap
        .iter()
        .filter(|(node, _)| {
            reachable
                .binary_search_by_key(&(node.y, node.x), |tile| (tile.y, tile.x))
                .is_ok()
        })
        .filter_map(|(node, amount)| {
            let (component, reach) = homes
                .iter()
                .filter(|(component, _, _)| map.touches(*node, *component))
                .map(|(component, centre, building)| {
                    let reach = octile_tenths(*centre, doubled(*node));
                    (
                        (reach, frame.rank(doubled(*node), *centre), building.id),
                        *component,
                    )
                })
                .min()
                .map(|((reach, _, _), component)| (component, reach))?;
            // Ticks for one load: standing at the node, then there and back.
            let trip = Fx::from_num(2 * reach) / (Fx::from_num(10) * harvester.speed);
            let cycle =
                u64::from(harvest.capacity * harvest.ticks_per_scrap) + trip.ceil().to_num::<u64>();
            if payback * cycle > horizon {
                return None;
            }
            let room = room(observation, map, *node, component) as u64;
            let repaid = u64::from(*amount) / price;
            let places = room.min(repaid);
            (places > 0).then(|| {
                let crew = (places * fill).div_ceil(1_000).max(1);
                (*node, usize::try_from(crew).expect("crews fit in usize"))
            })
        })
        .collect()
}

/// Known nodes with scrap whose route from home stays out of known danger:
/// the reach of known enemy ground fire, and the surroundings of enemy
/// Foundries, known or presumed, where their owners' defenders gather.
fn reachable(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    scratch: &Scratch,
    homes: &[&BuildingObs],
) -> Vec<TilePos> {
    let danger = danger(map, scratch);
    observation
        .known_scrap
        .iter()
        .filter(|(_, amount)| *amount > 0)
        .map(|(node, _)| *node)
        .filter(|node| clear_route(map, observation.me, frame, &danger, homes, *node))
        .collect()
}

/// Tiles known enemy ground fire covers or that lie around an enemy Foundry.
fn danger(map: &MapModel, scratch: &Scratch) -> Grid<bool> {
    let (width, height) = map.size();
    let mut danger = Grid::new(width, height, false);
    let mut mark = |tile: TilePos| {
        if let Some(cell) = danger.get_mut(tile) {
            *cell = true;
        }
    };
    for hazard in &scratch.ground {
        hazard.tiles().for_each(&mut mark);
    }
    let (foundry_width, foundry_height) = BuildingKind::Foundry.size();
    for anchor in scratch.enemy_foundries() {
        for dy in -(HOME_TILES + 1)..=foundry_height + HOME_TILES {
            for dx in -(HOME_TILES + 1)..=foundry_width + HOME_TILES {
                mark(anchor.offset(dx, dy));
            }
        }
    }
    danger
}

/// Whether the shortest ground route from the seat's start to `node`, which
/// workers roughly follow, stays out of `danger` until it comes home to one
/// of `homes`. The route is traced back from the node down the start's
/// distance field; a node that field does not reach is judged by itself.
fn clear_route(
    map: &MapModel,
    me: PlayerId,
    frame: HomeFrame,
    danger: &Grid<bool>,
    homes: &[&BuildingObs],
    node: TilePos,
) -> bool {
    let size = BuildingKind::Foundry.size();
    let mut tile = node;
    let mut distance = map.distance(me, tile);
    loop {
        if homes
            .iter()
            .any(|home| gap(home.anchor, size, tile, (1, 1)) <= HOME_TILES)
        {
            return true;
        }
        if danger.get(tile) == Some(&true) {
            return false;
        }
        let from = doubled(tile);
        let open = |tile: TilePos| map.component(tile).is_some();
        // Diagonal steps never cut a corner, as in the distance field. A
        // worker stands on any tile beside the node, so the first step may.
        let legal = |next: &TilePos| {
            let (dx, dy) = (next.x - tile.x, next.y - tile.y);
            tile == node
                || dx == 0
                || dy == 0
                || (open(tile.offset(dx, 0)) && open(tile.offset(0, dy)))
        };
        let next = ring(tile, (1, 1))
            .filter(legal)
            .map(|next| (map.distance(me, next), next))
            .filter(|(closer, _)| *closer < distance)
            .min_by_key(|(closer, next)| (*closer, frame.rank(from, doubled(*next))));
        let Some((closer, next)) = next else {
            return true;
        };
        tile = next;
        distance = closer;
    }
}

/// Ticks within which a Harvester must repay its price for its node to be
/// worked: longer the more a stance invests at home, stretched by greed.
fn horizon(profile: &ResolvedProfile) -> u64 {
    let minutes_tenths: u64 = match profile.stance {
        BotStance::Turtle => 30,
        BotStance::Balanced => 20,
        BotStance::Aggressive => 15,
    };
    let per_minute = u64::from(TICKS_PER_SECOND) * 60;
    minutes_tenths * per_minute * (200 + u64::from(profile.traits.greed)) / 2_000
}

/// Per mille of a node's places the seat fills: all of them for an economic
/// stance, about half for an aggressive one, more the greedier it is.
pub(crate) fn fill(profile: &ResolvedProfile) -> u64 {
    let base: i64 = match profile.stance {
        BotStance::Turtle => 1_000,
        BotStance::Balanced => 750,
        BotStance::Aggressive => 500,
    };
    (base + (i64::from(profile.traits.greed) - 50) * 5)
        .clamp(250, 1_000)
        .cast_unsigned()
}

/// Free tiles a worker could stand on to work `node`: its neighbours on
/// `component` that no known building, the seat's, an ally's or an enemy's,
/// or other node covers.
fn room(observation: &ObservationData, map: &MapModel, node: TilePos, component: u32) -> usize {
    let covered = |tile: TilePos| {
        observation
            .my_buildings
            .iter()
            .chain(&observation.ally_buildings)
            .chain(&observation.enemy_buildings)
            .any(|building| {
                let (width, height) = building.kind.size();
                (building.anchor.x..building.anchor.x + width).contains(&tile.x)
                    && (building.anchor.y..building.anchor.y + height).contains(&tile.y)
            })
            || observation
                .known_scrap
                .binary_search_by_key(&(tile.y, tile.x), |(other, _)| (other.y, other.x))
                .is_ok()
    };
    ring(node, (1, 1))
        .filter(|tile| map.component(*tile) == Some(component) && !covered(*tile))
        .count()
}

/// Sends each idle worker, nearest home first, to the reachable worked
/// node with the most places open, or to its nearest reachable known node
/// when none is worked, never to a node inside known danger or whose route
/// crosses it. A worker harvesting a node whose route has turned dangerous
/// is sent elsewhere the same way, or home when nowhere is left. A worker
/// with nowhere to go that still carries scrap delivers it instead. One
/// order per node.
fn assign_idle(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    staffing: &Staffing,
    hazards: &[Hazard],
    ledger: &mut Ledger,
) {
    let safe = |node: &TilePos| {
        let point = doubled(*node);
        !hazards.iter().any(|hazard| hazard.covers(point))
    };
    let mut miners: Vec<(TilePos, i64)> = staffing
        .worked
        .iter()
        .filter(|(node, _)| safe(node))
        .map(|(node, crew)| {
            let working = observation
                .my_units
                .iter()
                .filter(|unit| unit.harvesting == Some(*node))
                .count();
            (
                *node,
                i64::try_from(*crew).expect("crews fit in i64")
                    - i64::try_from(working).expect("unit counts fit in i64"),
            )
        })
        .collect();
    let stale = |unit: &UnitObs| {
        unit.harvesting.is_some_and(|node| {
            staffing
                .reachable
                .binary_search_by_key(&(node.y, node.x), |tile| (tile.y, tile.x))
                .is_err()
        })
    };
    let mut idle: Vec<_> = observation
        .my_units
        .iter()
        .filter(|unit| {
            (unit.idle || stale(unit))
                && worker(unit.kind)
                && !ledger.employs(unit.id)
                && !ledger.stuck(unit.id)
        })
        .collect();
    idle.sort_by_key(|unit| (frame.rank(frame.home, doubled(unit.tile)), unit.id));
    let mut assignments: Vec<(TilePos, UnitId)> = Vec::new();
    let mut loaded: Vec<&UnitObs> = Vec::new();
    let mut stranded: Vec<&UnitObs> = Vec::new();
    for unit in idle {
        let Some(component) = map.component(unit.tile) else {
            continue;
        };
        let from = doubled(unit.tile);
        let most_open = miners
            .iter_mut()
            .filter(|(node, _)| map.touches(*node, component))
            .min_by_key(|(node, open)| (Reverse(*open), frame.rank(from, doubled(*node))));
        let node = if let Some((node, open)) = most_open {
            *open -= 1;
            *node
        } else {
            let nearest = staffing
                .reachable
                .iter()
                .copied()
                .filter(|node| map.touches(*node, component) && safe(node))
                .min_by_key(|node| frame.rank(from, doubled(*node)));
            let Some(node) = nearest else {
                if unit.carrying > 0 {
                    loaded.push(unit);
                } else if stale(unit) {
                    stranded.push(unit);
                }
                continue;
            };
            node
        };
        assignments.push((node, unit.id));
    }
    assignments.sort_by_key(|(node, unit)| (frame.rank(frame.home, doubled(*node)), *unit));
    for group in assignments.chunk_by(|a, b| a.0 == b.0) {
        let command = Command::Harvest {
            units: group.iter().map(|(_, unit)| *unit).collect(),
            node: group[0].0,
            queue: false,
        };
        if !ledger.order(command) {
            return;
        }
    }
    let foundries = built_foundries(observation);
    // Only a worker with a Foundry on its ground can deliver, and an order
    // in which none can is refused.
    let returning: Vec<UnitId> = loaded
        .iter()
        .filter(|unit| refuge(map, frame, &foundries, unit.tile).is_some())
        .map(|unit| unit.id)
        .collect();
    if !returning.is_empty()
        && !ledger.order(Command::ReturnCargo {
            units: returning,
            foundry: None,
            repair: false,
        })
    {
        return;
    }
    for unit in stranded {
        let Some(refuge) = refuge(map, frame, &foundries, unit.tile) else {
            continue;
        };
        if !ledger.order(Command::Run {
            units: vec![unit.id],
            goal: refuge,
            queue: false,
        }) {
            return;
        }
    }
}

/// Octile distance between two doubled-coordinate points, in tenths of a tile.
fn octile_tenths(a: (i64, i64), b: (i64, i64)) -> i64 {
    let (dx, dy) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
    let doubled = 10 * dx.max(dy) + 4 * dx.min(dy);
    doubled / 2
}
