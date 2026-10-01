//! Tenders: free ones weld wounded free units on their ground, and those an
//! attack takes along weld its members while the army regroups. How many a
//! seat or an attack wants follows the missing health they answer for.

use super::{Missions, mine};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, doubled};
use crate::map::MapModel;
use oxide_sim::observation::{ObservationData, UnitObs};
use oxide_sim::stats::Domain;
use oxide_sim::{Command, UnitId, UnitKind};
use std::cmp::Reverse;

/// Per mille of its health a unit must miss before a Tender welds it.
const WELD_DAMAGE: u64 = 100;

/// Scrap of missing health among armed ground units one Tender answers for
/// at middling support.
const WOUNDS_PER_TENDER: u64 = 500;

/// Scrap of missing health among the armed ground units of `units`.
pub(crate) fn wounds<'a>(units: impl Iterator<Item = &'a UnitObs>) -> u64 {
    units
        .filter(|unit| {
            let stats = unit.kind.stats();
            stats.domain == Domain::Ground && !stats.weapons.is_empty()
        })
        .map(|unit| {
            let stats = unit.kind.stats();
            let max = u64::from(stats.max_hp.max(1));
            u64::from(stats.cost) * max.saturating_sub(u64::from(unit.hp)) / max
        })
        .sum()
}

/// Missing health one Tender answers for: less the more the seat leans on
/// support.
pub(crate) fn per_tender(support: u8) -> u64 {
    WOUNDS_PER_TENDER * 1_000 / crate::composition::weight(support)
}

impl Missions {
    /// Has each idle free Tender weld the free wounded ground unit on its
    /// ground missing the most value, no two sent to the same patient in one
    /// decision.
    pub(crate) fn tend(
        &self,
        observation: &ObservationData,
        map: &MapModel,
        frame: HomeFrame,
        ledger: &mut Ledger,
    ) {
        let free: Vec<&UnitObs> = self
            .available(observation, false)
            .into_iter()
            .filter_map(|id| mine(observation, id))
            .collect();
        let mut tended: Vec<UnitId> = Vec::new();
        for tender in free
            .iter()
            .filter(|unit| unit.kind == UnitKind::Tender && unit.idle)
        {
            let patients: Vec<&UnitObs> = free
                .iter()
                .copied()
                .filter(|unit| !tended.contains(&unit.id))
                .collect();
            let Some(patient) = patient(map, frame, tender, &patients) else {
                continue;
            };
            if !ledger.order(weld(tender.id, patient)) {
                return;
            }
            tended.push(patient);
        }
    }
}

/// The wounded ground unit among `patients` on `tender`'s ground missing the
/// most value, nearest first among equals. A Tender never welds itself.
pub(super) fn patient(
    map: &MapModel,
    frame: HomeFrame,
    tender: &UnitObs,
    patients: &[&UnitObs],
) -> Option<UnitId> {
    let ground = map.component(tender.tile)?;
    let from = doubled(tender.tile);
    patients
        .iter()
        .filter(|unit| {
            unit.id != tender.id
                && unit.kind.stats().domain == Domain::Ground
                && map.component(unit.tile) == Some(ground)
        })
        .filter_map(|unit| {
            let stats = unit.kind.stats();
            let (hp, max) = (u64::from(unit.hp), u64::from(stats.max_hp.max(1)));
            (hp * 1_000 <= max * (1_000 - WELD_DAMAGE))
                .then(|| (u64::from(stats.cost) * (max - hp) / max, unit))
        })
        .max_by_key(|(missing, unit)| {
            (
                *missing,
                Reverse(frame.rank(from, doubled(unit.tile))),
                Reverse(unit.id),
            )
        })
        .map(|(_, unit)| unit.id)
}

pub(super) fn weld(tender: UnitId, patient: UnitId) -> Command {
    Command::RepairUnit {
        units: vec![tender],
        target: patient,
        queue: false,
    }
}
