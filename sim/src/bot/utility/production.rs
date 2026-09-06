//! Opening combat-core production and accounting.

use super::*;
use crate::bot::executive::full_ground_strength;
use crate::ids::BuildingId;
use crate::stats::Role;

/// Keep opening recovery shallow enough to react when the missing screen has
/// been restored. Standing-force production owns all later combat demand.
const PLANNING_DEPTH: usize = 2;

#[derive(Debug, Clone, Copy)]
struct Producer {
    id: BuildingId,
    depth: usize,
}

/// Exact ordinary-combat strength projected after the intents already emitted
/// during this think.
///
/// `missing_scrap` prices the remaining strength in whole Sentinels, because
/// that is the unit the opening-core pass can add without further tech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) struct CombatCoreStatus {
    pub(in crate::bot) projected_strength: u64,
    pub(in crate::bot) target_strength: u64,
    pub(in crate::bot) missing_strength: u64,
    pub(in crate::bot) missing_scrap: u32,
    pub(in crate::bot) ready: bool,
}

/// Restore the opening ordinary-combat floor with shallow Sentinel orders.
///
/// This is a survival prerequisite, not standing-army policy. Once the floor
/// is ready, the shared allocator decides all voluntary combat production.
pub(super) fn fill_combat_core(
    obs: &Observation,
    reserved: &[UnitId],
    sentinel_equivalent_floor: u64,
    capital_reserve: u32,
    producer_lane_reservations: &ProducerLaneReservations,
    budget: &mut u32,
    intents: &mut Vec<Intent>,
) -> CombatCoreStatus {
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
    fill_combat_core_to_strength(
        obs,
        reserved,
        sentinel_strength.saturating_mul(sentinel_equivalent_floor),
        capital_reserve,
        producer_lane_reservations,
        budget,
        intents,
    )
}

pub(super) fn fill_combat_core_to_strength(
    obs: &Observation,
    reserved: &[UnitId],
    target_strength: u64,
    capital_reserve: u32,
    producer_lane_reservations: &ProducerLaneReservations,
    budget: &mut u32,
    intents: &mut Vec<Intent>,
) -> CombatCoreStatus {
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
    let status = combat_core_status_for_strength(obs, reserved, intents, target_strength);
    if status.ready {
        return status;
    }
    let mut projected_strength = status.projected_strength;

    let mut foundries: Vec<Producer> = obs
        .my_buildings
        .iter()
        .enumerate()
        .filter(|(_, building)| building.built && building.kind == BuildingKind::Foundry)
        .map(|(queue_index, building)| Producer {
            id: building.id,
            depth: obs
                .my_queues
                .get(queue_index)
                .map_or(PLANNING_DEPTH, Vec::len)
                .saturating_add(planned_at(intents, building.id)),
        })
        .collect();
    foundries.sort_by_key(|producer| producer.id);
    if foundries.is_empty() {
        return status;
    }

    let sentinel_cost = UnitKind::Sentinel.stats().cost;
    'depths: for target_depth in 1..=PLANNING_DEPTH {
        for foundry in &mut foundries {
            if projected_strength >= status.target_strength {
                break 'depths;
            }
            if foundry.depth >= target_depth
                || *budget < sentinel_cost.saturating_add(capital_reserve)
            {
                continue;
            }
            let prior_immediate = planned_kinds_at(intents, foundry.id);
            if !producer_lane_reservations.allows_raw_immediate_append(
                foundry.id,
                &prior_immediate,
                UnitKind::Sentinel,
            ) {
                continue;
            }
            *budget -= sentinel_cost;
            intents.push(Intent::TrainAt {
                building: foundry.id,
                kind: UnitKind::Sentinel,
            });
            foundry.depth += 1;
            projected_strength = projected_strength.saturating_add(sentinel_strength);
        }
    }

    combat_core_status_for_strength(obs, reserved, intents, target_strength)
}

/// Measure an explicit Sentinel-equivalent floor without inferring ownership
/// from army bookkeeping. Callers exclude only the exact units committed to a
/// strategic operation; ordinary Executive armies therefore remain part of the
/// available fighting line.
pub(in crate::bot) fn combat_core_status(
    obs: &Observation,
    reserved: &[UnitId],
    intents: &[Intent],
    sentinel_equivalent_floor: u64,
) -> CombatCoreStatus {
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
    let target_strength = sentinel_strength.saturating_mul(sentinel_equivalent_floor);
    combat_core_status_for_strength(obs, reserved, intents, target_strength)
}

pub(super) fn combat_core_status_for_strength(
    obs: &Observation,
    reserved: &[UnitId],
    intents: &[Intent],
    target_strength: u64,
) -> CombatCoreStatus {
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
    let live = obs
        .my_units
        .iter()
        .filter(|unit| !reserved.contains(&unit.id) && ordinary_core_unit(unit.kind))
        .map(crate::bot::executive::unit_strength)
        .sum::<u64>();
    let queued = obs
        .my_queues
        .iter()
        .flatten()
        .copied()
        .filter(|kind| ordinary_core_unit(*kind))
        .map(full_ground_strength)
        .sum::<u64>();
    let planned = intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::TrainAt { kind, .. } if ordinary_core_unit(*kind) => Some(*kind),
            _ => None,
        })
        .map(full_ground_strength)
        .sum::<u64>();
    let projected_strength = live.saturating_add(queued).saturating_add(planned);
    let missing_strength = target_strength.saturating_sub(projected_strength);
    CombatCoreStatus {
        projected_strength,
        target_strength,
        missing_strength,
        missing_scrap: missing_core_scrap(
            missing_strength,
            sentinel_strength,
            UnitKind::Sentinel.stats().cost,
        ),
        ready: missing_strength == 0,
    }
}

fn missing_core_scrap(missing_strength: u64, sentinel_strength: u64, sentinel_cost: u32) -> u32 {
    let missing_sentinels = missing_strength.div_ceil(sentinel_strength);
    u32::try_from(missing_sentinels)
        .unwrap_or(u32::MAX)
        .saturating_mul(sentinel_cost)
}

fn ordinary_core_unit(kind: UnitKind) -> bool {
    matches!(kind.role(), Role::Sentinel | Role::Warden | Role::Breaker)
}

pub(super) fn planned_at(intents: &[Intent], building: BuildingId) -> usize {
    intents
        .iter()
        .filter(|intent| {
            matches!(intent, Intent::TrainAt { building: planned, .. } if *planned == building)
        })
        .count()
}

pub(super) fn planned_kinds_at(intents: &[Intent], building: BuildingId) -> Vec<UnitKind> {
    intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::TrainAt {
                building: planned,
                kind,
            } if *planned == building => Some(*kind),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
