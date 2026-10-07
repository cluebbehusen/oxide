//! With several enemies, attacks and strikes go after one of them first.

use super::Scratch;
use super::{Missions, Objective, Task};
use crate::frame::{HomeFrame, centre_distance, footprint_centre, gap};
use crate::map::MapModel;
use crate::profile::PersonalityTraits;
use oxide_sim::observation::ObservationData;
use oxide_sim::{BuildingKind, PlayerId};
use std::cmp::Reverse;

/// Empty tiles from the seat's buildings inside which an enemy army presses
/// it.
const PRESSURE_GAP: i32 = 12;

/// What staying on the current target's owner is worth.
const STEADY: i64 = 300;

/// What each tile of distance to an enemy costs it.
const TILE: i64 = 10;

/// What each economic building an enemy is known to have costs it, per
/// point of guile.
const ECONOMY: i64 = 2;

impl Missions {
    /// The enemy attacks and strikes go after when the seat has several:
    /// the one pressing it hardest, then the nearest, less the army it shows,
    /// with guile favoring a small economy, and a bonus for the owner of the
    /// oldest attack or strike's target so the seat does not flip between
    /// enemies. Equal enemies go to the one whose nearest building ranks
    /// first in the seat's frame, so mirrored seats choose mirrored enemies.
    /// `None` with one enemy or none.
    pub(crate) fn rival(
        &self,
        scratch: &Scratch,
        observation: &ObservationData,
        map: &MapModel,
        traits: PersonalityTraits,
    ) -> Option<PlayerId> {
        let targets: &[Objective] = &scratch.objectives;
        let mut owners: Vec<PlayerId> = targets.iter().map(|target| target.owner).collect();
        owners.sort_unstable();
        owners.dedup();
        if owners.len() < 2 {
            return None;
        }
        let frame = HomeFrame::of(observation, map)?;
        let home = footprint_centre(BuildingKind::Foundry, map.start(observation.me)?);
        let nearest = |owner: PlayerId| {
            targets
                .iter()
                .filter(|target| target.owner == owner)
                .map(|target| footprint_centre(target.building, target.anchor))
                .min_by_key(|centre| {
                    (
                        centre_distance(home, *centre),
                        frame.rank(frame.home, *centre),
                    )
                })
        };
        let current = self.list.iter().find_map(|mission| match mission.task {
            Task::Attack { target, .. } | Task::Strike { target, .. } => Some(target.owner),
            _ => None,
        });
        let score = |owner: PlayerId| -> i64 {
            let armed = observation
                .enemy_units
                .iter()
                .filter(|unit| unit.player == owner && !unit.kind.stats().weapons.is_empty());
            let cost = |kind: oxide_sim::UnitKind| i64::from(kind.stats().cost);
            let presence: i64 = armed.clone().map(|unit| cost(unit.kind)).sum();
            let pressure: i64 = armed
                .filter(|unit| {
                    observation.my_buildings.iter().any(|building| {
                        gap(
                            building.anchor,
                            building.kind.base_stats().size,
                            unit.tile,
                            (1, 1),
                        ) <= PRESSURE_GAP
                    })
                })
                .map(|unit| cost(unit.kind))
                .sum();
            let economy = observation
                .enemy_buildings
                .iter()
                .filter(|building| {
                    building.player == owner
                        && matches!(
                            building.kind,
                            BuildingKind::Foundry
                                | BuildingKind::Extractor
                                | BuildingKind::Reclaimer
                        )
                })
                .count();
            let economy = i64::try_from(economy).expect("building counts fit in i64");
            let distance = nearest(owner).map_or(0, |centre| {
                i64::try_from(centre_distance(home, centre)).unwrap_or(i64::MAX)
            });
            let steady = if current == Some(owner) { STEADY } else { 0 };
            4 * pressure - presence - TILE * distance - ECONOMY * economy * i64::from(traits.guile)
                + steady
        };
        owners.into_iter().max_by_key(|owner| {
            (
                score(*owner),
                nearest(*owner).map(|centre| Reverse(frame.rank(frame.home, centre))),
            )
        })
    }
}
