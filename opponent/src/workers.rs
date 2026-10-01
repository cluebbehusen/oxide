//! Workers (Harvesters and Excavators): how many the seat wants, training
//! them, keeping them out of harm, and sending free ones to build, weld and
//! harvest.

use crate::decision::{Ledger, Producer};
use crate::frame::{HomeFrame, doubled, footprint_centre, gap, ring};
use crate::map::MapModel;
use crate::missions::Hazard;
use crate::profile::ResolvedProfile;
use chassis::fx::Fx;
use chassis::grid::TilePos;
use oxide_sim::TICKS_PER_SECOND;
use oxide_sim::ids::Target;
use oxide_sim::observation::{BuildingObs, ObservationData};
use oxide_sim::scenario::BotStance;
use oxide_sim::stats::FOUNDRY_REPAIR_PRICE;
use oxide_sim::{BuildingKind, Command, UnitId, UnitKind};
use std::cmp::Reverse;

/// Per mille of its health a building must miss before workers weld it.
const WELD_DAMAGE: u32 = 100;

/// Scrap the seat keeps spendable before starting a weld, which bills as it
/// goes.
const WELD_FLOOR: u32 = 100;

/// Empty tiles around a building inside which an armed enemy in sight keeps
/// workers from welding it.
const WELD_CLEARANCE: i32 = 12;

/// Empty tiles from an own Foundry inside which a worker counts as home.
const HOME_TILES: i32 = 8;

/// The nodes the seat's Foundries work, each with the Harvesters it wants
/// there, and the worker slots it fills.
pub(crate) struct Staffing {
    worked: Vec<(TilePos, usize)>,
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
        (self.crew.min(wanted) * 1_000 / wanted) as u32
    }
}

/// Whether `kind` harvests, builds and welds: a Harvester or an Excavator.
pub(crate) fn worker(kind: UnitKind) -> bool {
    kind.stats().harvest.is_some()
}

/// Harvester slots a worker of `kind` fills: an Excavator mines at twice a
/// Harvester's rate.
fn slots(kind: UnitKind) -> usize {
    match kind {
        UnitKind::Excavator => 2,
        kind if worker(kind) => 1,
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
) -> Staffing {
    Staffing {
        worked: worked_nodes(observation, map, frame, profile, foundries),
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
/// counting those alive and queued.
pub(crate) fn train(
    observation: &ObservationData,
    foundries: &[Producer<'_>],
    staffing: &Staffing,
    greed: u8,
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
        if ledger.train(foundry.building.id, kind) {
            count += slots(kind);
        }
    }
}

/// Brings workers away from home back from armed enemies in sight, sends a
/// worker to each unattended construction site, welds a damaged building,
/// then sends idle workers to the least-worked node they can reach clear of
/// known danger.
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
    assign_idle(observation, map, frame, &staffing.worked, hazards, ledger);
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
    let foundries: Vec<&BuildingObs> = observation
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry && building.built)
        .collect();
    if hazards.is_empty() {
        return;
    }
    let size = BuildingKind::Foundry.base_stats().size;
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
        let ground = map.component(unit.tile);
        let from = doubled(unit.tile);
        let refuge = foundries
            .iter()
            .flat_map(|foundry| ring(foundry.anchor, size))
            .filter(|tile| ground.is_some() && map.component(*tile) == ground)
            .min_by_key(|tile| (frame.rank(from, doubled(*tile)), *tile));
        let Some(refuge) = refuge else {
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
            let size = building.kind.base_stats().size;
            let threatened = observation.enemy_units.iter().any(|enemy| {
                !enemy.kind.stats().weapons.is_empty()
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
/// to the nearest built Foundry whose ground reaches it, unless an enemy
/// building or hostile start is as near, and is worked when a Harvester
/// hauling from there repays its price within the seat's horizon.
/// Its crew fills the free tiles around it that its remaining scrap repays,
/// as full as stance and greed make it.
fn worked_nodes(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    profile: &ResolvedProfile,
    foundries: &[Producer<'_>],
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
    // Hostile Foundries known or presumed, and every other known enemy
    // building: a node nearer one of them than any own Foundry is theirs.
    let hostile: Vec<(i64, i64)> = observation
        .enemy_buildings
        .iter()
        .map(|building| footprint_centre(building.kind, building.anchor))
        .chain(
            map.hostiles(observation.me)
                .filter_map(|owner| map.start(owner))
                .map(|start| footprint_centre(BuildingKind::Foundry, start)),
        )
        .collect();
    observation
        .known_scrap
        .iter()
        .filter(|(_, amount)| *amount > 0)
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
            if hostile
                .iter()
                .any(|centre| octile_tenths(*centre, doubled(*node)) <= reach)
            {
                return None;
            }
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
            (places > 0).then(|| (*node, (places * fill).div_ceil(1_000).max(1) as usize))
        })
        .collect()
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
    (base + (i64::from(profile.traits.greed) - 50) * 5).clamp(250, 1_000) as u64
}

/// Free tiles a worker could stand on to work `node`: its neighbours on
/// `component` that no known building or other node covers.
fn room(observation: &ObservationData, map: &MapModel, node: TilePos, component: u32) -> usize {
    let covered = |tile: TilePos| {
        observation
            .my_buildings
            .iter()
            .chain(&observation.enemy_buildings)
            .any(|building| {
                let (width, height) = building.kind.base_stats().size;
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
/// when none is worked, never to a node inside known danger. One order per
/// node.
fn assign_idle(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    worked: &[(TilePos, usize)],
    hazards: &[Hazard],
    ledger: &mut Ledger,
) {
    let safe = |node: &TilePos| {
        let point = doubled(*node);
        !hazards.iter().any(|hazard| hazard.covers(point))
    };
    let mut miners: Vec<(TilePos, i64)> = worked
        .iter()
        .filter(|(node, _)| safe(node))
        .map(|(node, crew)| {
            let working = observation
                .my_units
                .iter()
                .filter(|unit| unit.harvesting == Some(*node))
                .count();
            (*node, *crew as i64 - working as i64)
        })
        .collect();
    let mut idle: Vec<_> = observation
        .my_units
        .iter()
        .filter(|unit| unit.idle && worker(unit.kind) && !ledger.employs(unit.id))
        .collect();
    idle.sort_by_key(|unit| (frame.rank(frame.home, doubled(unit.tile)), unit.id));
    let mut assignments: Vec<(TilePos, UnitId)> = Vec::new();
    for unit in idle {
        let Some(component) = map.component(unit.tile) else {
            continue;
        };
        let from = doubled(unit.tile);
        let most_open = miners
            .iter_mut()
            .filter(|(node, _)| map.touches(*node, component))
            .min_by_key(|(node, open)| (Reverse(*open), frame.rank(from, doubled(*node))));
        let node = match most_open {
            Some((node, open)) => {
                *open -= 1;
                *node
            }
            None => {
                let nearest = observation
                    .known_scrap
                    .iter()
                    .filter(|(node, amount)| {
                        *amount > 0 && map.touches(*node, component) && safe(node)
                    })
                    .map(|(node, _)| *node)
                    .min_by_key(|node| frame.rank(from, doubled(*node)));
                let Some(node) = nearest else {
                    continue;
                };
                node
            }
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
            break;
        }
    }
}

/// Octile distance between two doubled-coordinate points, in tenths of a tile.
fn octile_tenths(a: (i64, i64), b: (i64, i64)) -> i64 {
    let (dx, dy) = ((a.0 - b.0).abs(), (a.1 - b.1).abs());
    (10 * dx.max(dy) + 4 * dx.min(dy)) / 2
}
