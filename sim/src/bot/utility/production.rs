//! Opening combat-core production and accounting.

use super::*;
use crate::bot::executive::full_ground_strength;
use crate::ids::BuildingId;
use crate::stats::Role;
use std::cmp::Reverse;

/// Keep opening recovery shallow enough to react when the missing screen has
/// been restored. Standing-force production owns all later combat demand.
const PLANNING_DEPTH: usize = 2;
const ALLY_DISCOUNT: usize = 2;

#[derive(Debug, Clone, Copy)]
struct Producer {
    id: BuildingId,
    depth: usize,
}

#[derive(Debug, Clone, Copy)]
struct ResidualFoundryDemand {
    kind: UnitKind,
    minimum_owned: usize,
    target: usize,
    own_floor: usize,
    discount_allies: bool,
    order: u8,
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

/// Fill the Foundry roles that do not belong to standing-force allocation.
///
/// The shared allocator owns every ordinary combat and specialist purchase.
/// This residual pass retains only economy growth after renewable income,
/// tier-two workers, and the bounded raider roster. It spends current scrap
/// left by allocation and never moves an accepted producer schedule.
pub(super) fn fill_residual_foundry_roles(
    dials: &Dials,
    obs: &Observation,
    capital_reserve: u32,
    producer_lane_reservations: &ProducerLaneReservations,
    budget: &mut u32,
    intents: &mut Vec<Intent>,
) {
    if !dials.adaptive_composition {
        return;
    }

    let mut foundries: Vec<_> = obs
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
    foundries.sort_unstable_by_key(|producer| producer.id);

    // Breadth before depth keeps multiple Foundries useful while retaining a
    // shallow, reconsiderable queue. The finite role targets, current bank,
    // and accepted producer lanes are the only limits on this pass.
    for target_depth in 1..=PLANNING_DEPTH {
        for foundry in &mut foundries {
            if foundry.depth >= target_depth {
                continue;
            }
            let Some(demand) =
                best_residual_foundry_demand(dials, obs, capital_reserve, *budget, intents)
            else {
                continue;
            };
            let prior_immediate = planned_kinds_at(intents, foundry.id);
            if !producer_lane_reservations.allows_raw_immediate_append(
                foundry.id,
                &prior_immediate,
                demand.kind,
            ) {
                continue;
            }
            *budget -= demand.kind.stats().cost;
            intents.push(Intent::TrainAt {
                building: foundry.id,
                kind: demand.kind,
            });
            foundry.depth += 1;
        }
    }
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

fn best_residual_foundry_demand(
    dials: &Dials,
    obs: &Observation,
    capital_reserve: u32,
    budget: u32,
    intents: &[Intent],
) -> Option<ResidualFoundryDemand> {
    residual_foundry_demands(dials, obs)
        .into_iter()
        .filter(|demand| {
            BuildingKind::Foundry
                .base_stats()
                .produces
                .contains(&demand.kind)
                && demand.kind.stats().requires.iter().all(|required| {
                    obs.my_buildings
                        .iter()
                        .any(|building| building.built && building.kind == *required)
                })
        })
        .filter(|demand| {
            let own = own_role_count(obs, intents, demand.kind.role());
            let allied = if demand.discount_allies {
                ally_role_count(obs, demand.kind.role()) / ALLY_DISCOUNT
            } else {
                0
            };
            own >= demand.minimum_owned
                && (own < demand.own_floor || own.saturating_add(allied) < demand.target)
        })
        .filter(|demand| {
            let fighting_reserve = if demand.kind == UnitKind::Harvester {
                0
            } else {
                UnitKind::Sentinel.stats().cost
            };
            budget
                >= demand
                    .kind
                    .stats()
                    .cost
                    .saturating_add(capital_reserve)
                    .saturating_add(fighting_reserve)
        })
        .min_by_key(|demand| {
            let own = own_role_count(obs, intents, demand.kind.role());
            let allied = if demand.discount_allies {
                ally_role_count(obs, demand.kind.role()) / ALLY_DISCOUNT
            } else {
                0
            };
            let effective = own.saturating_add(allied);
            (
                u8::from(own >= demand.own_floor),
                effective.saturating_mul(1_000) / demand.target.max(1),
                Reverse(demand.target),
                demand.order,
                demand.kind,
            )
        })
}

fn residual_foundry_demands(dials: &Dials, obs: &Observation) -> Vec<ResidualFoundryDemand> {
    let mut demands = vec![ResidualFoundryDemand {
        kind: Role::Scuttler.unit_for(obs.faction),
        minimum_owned: 0,
        target: dials.raider_target,
        own_floor: 1,
        discount_allies: true,
        order: 20,
    }];
    let bootstrap_workers = immediate_harvester_target(dials) as usize;
    let workers = dials.harvester_target as usize;
    if workers > bootstrap_workers && renewable_economy_stands(obs) {
        demands.push(ResidualFoundryDemand {
            kind: Role::Harvester.unit_for(obs.faction),
            minimum_owned: bootstrap_workers,
            target: workers,
            own_floor: 0,
            discount_allies: false,
            order: 5,
        });
    }
    if dials.tech {
        demands.push(ResidualFoundryDemand {
            kind: Role::Excavator.unit_for(obs.faction),
            minimum_owned: 0,
            target: workers.saturating_sub(3).div_ceil(2).max(1),
            own_floor: 1,
            discount_allies: false,
            order: 10,
        });
    }
    demands
}

fn renewable_economy_stands(obs: &Observation) -> bool {
    obs.my_buildings.iter().any(|building| {
        building.built
            && matches!(
                building.kind,
                BuildingKind::Extractor | BuildingKind::Reclaimer
            )
    })
}

fn own_role_count(obs: &Observation, intents: &[Intent], role: Role) -> usize {
    obs.my_units
        .iter()
        .filter(|unit| unit.kind.role() == role)
        .count()
        + obs
            .my_queues
            .iter()
            .flatten()
            .filter(|kind| kind.role() == role)
            .count()
        + intents
            .iter()
            .filter(|intent| matches!(intent, Intent::TrainAt { kind, .. } if kind.role() == role))
            .count()
}

fn ally_role_count(obs: &Observation, role: Role) -> usize {
    obs.ally_units
        .iter()
        .filter(|unit| unit.kind.role() == role)
        .count()
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
