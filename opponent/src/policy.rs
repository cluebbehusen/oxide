//! The stub policy: idle Harvesters work the nearest known scrap and idle
//! Foundries keep training.

use crate::trace::Purchase;
use chassis::grid::TilePos;
use oxide_sim::observation::ObservationData;
use oxide_sim::{BuildingKind, Command, PlayerCommand, UnitId, UnitKind};
use std::cmp::Reverse;

const HARVESTERS_PER_FOUNDRY: usize = 6;

/// What one decision emits, with the running total it spent.
#[derive(Default)]
pub(crate) struct Decision {
    pub(crate) commands: Vec<PlayerCommand>,
    pub(crate) spent: u32,
    pub(crate) purchases: Vec<Purchase>,
    pub(crate) unit_orders: u32,
}

pub(crate) fn decide(observation: &ObservationData) -> Decision {
    let mut decision = Decision::default();
    let Some(frame) = HomeFrame::of(observation) else {
        return decision;
    };
    harvest(observation, frame, &mut decision);
    train(observation, &mut decision);
    decision
}

/// Sends each idle Harvester to its nearest known scrap node, one order per node.
fn harvest(observation: &ObservationData, frame: HomeFrame, decision: &mut Decision) {
    let mut assignments: Vec<(TilePos, UnitId)> = observation
        .my_units
        .iter()
        .filter(|unit| unit.idle && unit.kind == UnitKind::Harvester)
        .filter_map(|unit| {
            let from = doubled(unit.tile);
            observation
                .known_scrap
                .iter()
                .map(|(node, _)| *node)
                .min_by_key(|node| frame.rank(from, *node))
                .map(|node| (node, unit.id))
        })
        .collect();
    assignments.sort_by_key(|(node, unit)| (frame.rank(frame.home, *node), *unit));
    for group in assignments.chunk_by(|a, b| a.0 == b.0) {
        decision.commands.push(PlayerCommand {
            player: observation.me,
            command: Command::Harvest {
                units: group.iter().map(|(_, unit)| *unit).collect(),
                node: group[0].0,
                queue: false,
            },
        });
        decision.unit_orders += 1;
    }
}

/// Queues one unit at each idle built Foundry the running total can pay for:
/// Harvesters up to six per Foundry, Sentinels after that.
fn train(observation: &ObservationData, decision: &mut Decision) {
    let foundry = |kind: BuildingKind, built: bool| kind == BuildingKind::Foundry && built;
    let foundries = observation
        .my_buildings
        .iter()
        .filter(|building| foundry(building.kind, building.built))
        .count();
    let mut harvesters = observation
        .my_units
        .iter()
        .filter(|unit| unit.kind == UnitKind::Harvester)
        .count()
        + observation
            .my_queues
            .iter()
            .flatten()
            .filter(|kind| **kind == UnitKind::Harvester)
            .count();
    for (building, queue) in observation.my_buildings.iter().zip(&observation.my_queues) {
        if !foundry(building.kind, building.built) || !queue.is_empty() {
            continue;
        }
        let kind = if harvesters < HARVESTERS_PER_FOUNDRY * foundries {
            UnitKind::Harvester
        } else {
            UnitKind::Sentinel
        };
        let cost = kind.stats().cost;
        if observation.scrap.saturating_sub(decision.spent) < cost {
            continue;
        }
        decision.spent += cost;
        if kind == UnitKind::Harvester {
            harvesters += 1;
        }
        decision.commands.push(PlayerCommand {
            player: observation.me,
            command: Command::Train {
                building: building.id,
                kind,
            },
        });
        decision.purchases.push(Purchase {
            building: building.id,
            kind,
        });
    }
}

/// Ranks targets in the seat's home-relative frame, in doubled coordinates so
/// tile and footprint centres stay integral.
///
/// Nearer targets come first. Equal distances prefer the target further along
/// the ray from the map centre to home, then the one clockwise of it. A
/// half-turn of the map negates both that ray and every offset, preserving
/// both products, so mirrored seats break mirrored ties the same way.
#[derive(Clone, Copy)]
struct HomeFrame {
    home: (i64, i64),
    radial: (i64, i64),
}

impl HomeFrame {
    /// The frame around the seat's earliest built Foundry.
    fn of(observation: &ObservationData) -> Option<Self> {
        let home = observation
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry && building.built)
            .min_by_key(|building| building.id)?;
        let (width, height) = BuildingKind::Foundry.base_stats().size;
        let home = (
            i64::from(home.anchor.x) * 2 + i64::from(width),
            i64::from(home.anchor.y) * 2 + i64::from(height),
        );
        Some(Self {
            home,
            radial: (
                home.0 - i64::from(observation.map_width),
                home.1 - i64::from(observation.map_height),
            ),
        })
    }

    /// A home exactly at the map centre has no ray; the trailing row-major
    /// key then only keeps the order total.
    fn rank(self, from: (i64, i64), to: TilePos) -> (i64, Reverse<i64>, i64, i32, i32) {
        let (target_x, target_y) = doubled(to);
        let (dx, dy) = (target_x - from.0, target_y - from.1);
        let dot = self.radial.0 * dx + self.radial.1 * dy;
        let cross = self.radial.0 * dy - self.radial.1 * dx;
        (dx * dx + dy * dy, Reverse(dot), cross, to.y, to.x)
    }
}

fn doubled(tile: TilePos) -> (i64, i64) {
    (i64::from(tile.x) * 2 + 1, i64::from(tile.y) * 2 + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(home: (i64, i64), map: (i64, i64)) -> HomeFrame {
        HomeFrame {
            home,
            radial: (home.0 - map.0, home.1 - map.1),
        }
    }

    #[test]
    fn home_frame_ranks_nearer_targets_first_and_mirrors_its_ties() {
        let map = (24, 12);
        let rotate = |tile: TilePos| TilePos::new(23 - tile.x, 11 - tile.y);
        let west = frame((8, 12), map);
        let east = frame((40, 12), map);
        let nearest = |frame: HomeFrame, from: TilePos, nodes: &[TilePos]| {
            nodes
                .iter()
                .copied()
                .min_by_key(|node| frame.rank(doubled(from), *node))
                .unwrap()
        };
        let harvester = TilePos::new(7, 6);
        let tied = [TilePos::new(7, 3), TilePos::new(7, 9)];

        assert_eq!(
            nearest(west, harvester, &[tied[0], tied[1], TilePos::new(9, 6)]),
            TilePos::new(9, 6)
        );
        let west_choice = nearest(west, harvester, &tied);
        assert_eq!(west_choice, TilePos::new(7, 9), "not broken row-major");
        assert_eq!(
            nearest(east, rotate(harvester), &tied.map(rotate)),
            rotate(west_choice)
        );
        let centred = frame(map, map);
        assert_eq!(nearest(centred, harvester, &tied), TilePos::new(7, 3));
    }
}
