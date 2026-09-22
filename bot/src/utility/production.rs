//! Opening combat-core production and accounting.

use super::*;
use crate::executive::full_ground_strength;
use crate::production::ImmediateProduction;
use oxide_sim::stats::Role;

/// Keep opening recovery shallow enough to react when the missing screen has
/// been restored. Standing-force production owns all later combat demand.
const PLANNING_DEPTH: usize = 2;

/// Exact ordinary-combat strength projected after the intents already emitted
/// during this think.
///
/// `missing_scrap` prices the remaining strength in whole Sentinels, because
/// that is the unit the opening-core pass can add without further tech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CombatCoreStatus {
    pub(crate) projected_strength: u64,
    pub(crate) target_strength: u64,
    pub(crate) missing_strength: u64,
    pub(crate) missing_scrap: u32,
    pub(crate) ready: bool,
}

impl CombatCoreStatus {
    /// The candidate must be included in this snapshot's projected strength.
    pub(super) fn can_spare(&self, unit: &UnitObs) -> bool {
        let strength = if ordinary_core_unit(unit.kind) {
            crate::executive::unit_strength(unit)
        } else {
            0
        };
        self.projected_strength.saturating_sub(strength) >= self.target_strength
    }
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

    let mut production = ImmediateProduction::new(obs, producer_lane_reservations, intents);
    let sentinel_cost = UnitKind::Sentinel.stats().cost;
    for target_depth in 1..=PLANNING_DEPTH {
        while projected_strength < status.target_strength
            && *budget >= sentinel_cost.saturating_add(capital_reserve)
        {
            let Some(foundry) = production.lowest_id(UnitKind::Sentinel, target_depth) else {
                break;
            };
            *budget -= sentinel_cost;
            intents.push(production.append(foundry));
            projected_strength = projected_strength.saturating_add(sentinel_strength);
        }
    }

    combat_core_status_for_strength(obs, reserved, intents, target_strength)
}

/// Measure an explicit Sentinel-equivalent floor without inferring ownership
/// from army bookkeeping. Callers exclude only the exact units committed to a
/// strategic operation; ordinary Executive armies therefore remain part of the
/// available fighting line.
pub(crate) fn combat_core_status(
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
        .map(crate::executive::unit_strength)
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

#[cfg(test)]
mod tests;
