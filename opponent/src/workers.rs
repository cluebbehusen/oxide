//! Harvesters: how many the seat wants, training them, and sending idle ones
//! to work.

use crate::decision::{Ledger, Producer};
use crate::frame::{HomeFrame, doubled, footprint_centre};
use crate::map::MapModel;
use chassis::grid::TilePos;
use oxide_sim::observation::ObservationData;
use oxide_sim::{BuildingKind, Command, UnitId, UnitKind};

/// Scrap nodes each Foundry's Harvesters work, nearest first.
const NODES_PER_FOUNDRY: usize = 4;

/// Harvesters wanted on each worked node.
const HARVESTERS_PER_NODE: usize = 2;

/// Farthest a worked node may sit from its Foundry's centre, in tenths of a
/// tile of octile distance.
const HAUL_REACH: i64 = 120;

/// The nodes the seat's Foundries work and the Harvesters it has for them.
pub(crate) struct Staffing {
    worked: Vec<TilePos>,
    harvesters: usize,
}

impl Staffing {
    fn wanted(&self) -> usize {
        self.worked.len() * HARVESTERS_PER_NODE
    }

    /// Harvesters as a per-mille share of those wanted.
    pub(crate) fn saturation(&self) -> u32 {
        let wanted = self.wanted();
        if wanted == 0 {
            return 1_000;
        }
        (self.harvesters.min(wanted) * 1_000 / wanted) as u32
    }
}

/// Counts the worked nodes and the Harvesters alive, carried or queued.
pub(crate) fn staffing(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    foundries: &[Producer<'_>],
) -> Staffing {
    Staffing {
        worked: worked_nodes(observation, map, frame, foundries),
        harvesters: harvesters(observation),
    }
}

/// With no Harvester alive or queued, the nearest Foundry queues one even
/// behind other work, from protected scrap if it must.
pub(crate) fn recover(
    observation: &ObservationData,
    foundries: &[Producer<'_>],
    ledger: &mut Ledger,
) {
    if harvesters(observation) > 0 {
        return;
    }
    if let Some(foundry) = foundries.first() {
        ledger.train_urgently(foundry.building.id, UnitKind::Harvester);
    }
}

/// Trains Harvesters at idle Foundries up to two per worked node, sends a
/// Harvester to each unattended construction site, then sends idle
/// Harvesters to the least-worked node they can reach.
pub(crate) fn run(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    foundries: &[Producer<'_>],
    staffing: &Staffing,
    ledger: &mut Ledger,
) {
    let mut count = staffing.harvesters + ledger.queued(UnitKind::Harvester);
    for foundry in foundries {
        if count >= staffing.wanted() {
            break;
        }
        if foundry.idle
            && !ledger.queued_at(foundry.building.id)
            && ledger.train(foundry.building.id, UnitKind::Harvester)
        {
            count += 1;
        }
    }
    resume_sites(observation, map, frame, ledger);
    assign_idle(observation, map, frame, &staffing.worked, ledger);
}

/// The nearest Harvester to `centre` that stands on the same ground as
/// `site`, is not constructing, and has no work from this decision yet.
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
            unit.kind == UnitKind::Harvester
                && unit.site.is_none()
                && unit.founding.is_none()
                && map.component(unit.tile) == Some(ground)
                && !ledger.employs(unit.id)
        })
        .min_by_key(|unit| (frame.rank(centre, doubled(unit.tile)), unit.id))
        .map(|unit| unit.id)
}

/// Sends the nearest free Harvester to every paid base-tier site nobody is
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

/// Harvesters alive, carried or queued.
fn harvesters(observation: &ObservationData) -> usize {
    let alive = observation
        .my_units
        .iter()
        .filter(|unit| unit.kind == UnitKind::Harvester)
        .count();
    let carried = observation
        .my_carried_units
        .iter()
        .filter(|unit| unit.kind == UnitKind::Harvester)
        .count();
    let queued = observation
        .my_queues
        .iter()
        .flatten()
        .filter(|kind| **kind == UnitKind::Harvester)
        .count();
    alive + carried + queued
}

/// The live known nodes each Foundry works: the nearest few its ground can
/// reach within haul range, none counted for two Foundries.
fn worked_nodes(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    foundries: &[Producer<'_>],
) -> Vec<TilePos> {
    let mut worked: Vec<TilePos> = Vec::new();
    for foundry in foundries {
        let Some(component) = map.component(foundry.building.anchor) else {
            continue;
        };
        let centre = footprint_centre(BuildingKind::Foundry, foundry.building.anchor);
        let mut near: Vec<TilePos> = observation
            .known_scrap
            .iter()
            .filter(|(node, amount)| {
                *amount > 0
                    && !worked.contains(node)
                    && octile_tenths(centre, doubled(*node)) <= HAUL_REACH
                    && map.touches(*node, component)
            })
            .map(|(node, _)| *node)
            .collect();
        near.sort_by_key(|node| frame.rank(centre, doubled(*node)));
        worked.extend(near.into_iter().take(NODES_PER_FOUNDRY));
    }
    worked
}

/// Sends each idle Harvester, nearest home first, to the reachable worked
/// node with the fewest Harvesters, or to its nearest reachable known node
/// when none is worked. One order per node.
fn assign_idle(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    worked: &[TilePos],
    ledger: &mut Ledger,
) {
    let mut miners: Vec<(TilePos, usize)> = worked
        .iter()
        .map(|node| {
            let working = observation
                .my_units
                .iter()
                .filter(|unit| unit.harvesting == Some(*node))
                .count();
            (*node, working)
        })
        .collect();
    let mut idle: Vec<_> = observation
        .my_units
        .iter()
        .filter(|unit| unit.idle && unit.kind == UnitKind::Harvester && !ledger.employs(unit.id))
        .collect();
    idle.sort_by_key(|unit| (frame.rank(frame.home, doubled(unit.tile)), unit.id));
    let mut assignments: Vec<(TilePos, UnitId)> = Vec::new();
    for unit in idle {
        let Some(component) = map.component(unit.tile) else {
            continue;
        };
        let from = doubled(unit.tile);
        let least_worked = miners
            .iter_mut()
            .filter(|(node, _)| map.touches(*node, component))
            .min_by_key(|(node, working)| (*working, frame.rank(from, doubled(*node))));
        let node = match least_worked {
            Some((node, working)) => {
                *working += 1;
                *node
            }
            None => {
                let nearest = observation
                    .known_scrap
                    .iter()
                    .filter(|(node, amount)| *amount > 0 && map.touches(*node, component))
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
