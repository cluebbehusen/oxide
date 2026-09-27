//! Observation predicates and exact member assignment for air operation
//! rosters.

use super::*;

pub(super) fn excluding_owned(unavailable: &[UnitId], owned: &[UnitId]) -> Vec<UnitId> {
    let mut owned = owned.to_vec();
    owned.sort_unstable();
    owned.dedup();
    let mut external: Vec<_> = unavailable
        .iter()
        .copied()
        .filter(|id| owned.binary_search(id).is_err())
        .collect();
    external.sort_unstable();
    external.dedup();
    external
}

/// Keeps a live, available scout, otherwise enlists the first available one.
pub(super) fn retained_scout(
    scout: Option<UnitId>,
    obs: &Observation,
    unavailable: &[UnitId],
) -> Option<UnitId> {
    let kind = Role::Scout.unit_for(obs.faction);
    scout
        .filter(|id| {
            unit(obs, *id).is_some_and(|member| member.kind == kind) && !unavailable.contains(id)
        })
        .or_else(|| available(obs, unavailable, |candidate| candidate == kind).next())
}

pub(super) fn remembered_recon_scout(
    op: &AirOperation,
    obs: &Observation,
    enlisted: &[UnitId],
) -> Option<UnitId> {
    let scout_kind = Role::Scout.unit_for(obs.faction);
    op.scout
        .filter(|id| unit(obs, *id).is_some())
        .or_else(|| available(obs, enlisted, |kind| kind == scout_kind).next())
}

pub(super) fn merged_unavailable(first: &[UnitId], second: &[UnitId]) -> Vec<UnitId> {
    let mut merged = first.to_vec();
    merged.extend_from_slice(second);
    merged.sort_unstable();
    merged.dedup();
    merged
}

fn available<'a>(
    obs: &'a Observation,
    enlisted: &'a [UnitId],
    accepts: impl Fn(UnitKind) -> bool + 'a,
) -> impl Iterator<Item = UnitId> + 'a {
    obs.my_units
        .iter()
        .filter(move |member| accepts(member.kind) && !enlisted.contains(&member.id))
        .map(|member| member.id)
}

pub(super) fn assign_exact(
    assigned: &mut Vec<UnitId>,
    desired: usize,
    obs: &Observation,
    enlisted: &[UnitId],
    accepts: impl Fn(UnitKind) -> bool,
) {
    assigned.retain(|id| unit(obs, *id).is_some_and(|member| accepts(member.kind)));
    for member in &obs.my_units {
        if assigned.len() >= desired {
            break;
        }
        if accepts(member.kind) && !enlisted.contains(&member.id) && !assigned.contains(&member.id)
        {
            assigned.push(member.id);
        }
    }
    assigned.sort_unstable();
}

pub(super) fn assign_artillery(
    assigned: &mut Vec<UnitId>,
    plan: &AirPlan,
    obs: &Observation,
    enlisted: &[UnitId],
) {
    if let Some(package) = plan.package() {
        assign_provider_demands(assigned, &package.suppression, obs, enlisted);
    } else {
        // An island assault requests no artillery; this only prunes dead
        // members inherited from standby.
        assign_exact(assigned, 0, obs, enlisted, is_artillery);
    }
}

pub(super) fn assign_strike_aircraft(
    assigned: &mut Vec<UnitId>,
    plan: &AirPlan,
    obs: &Observation,
    enlisted: &[UnitId],
) {
    if let Some(package) = plan.package() {
        assign_provider_demands(assigned, &package.strike, obs, enlisted);
    } else {
        let bomber = Role::Bomber.unit_for(obs.faction);
        assign_exact(
            assigned,
            plan.desired_strike_aircraft(),
            obs,
            enlisted,
            |kind| kind == bomber,
        );
    }
}

pub(super) fn assign_provider_demands(
    assigned: &mut Vec<UnitId>,
    demands: &[ProviderDemand],
    obs: &Observation,
    enlisted: &[UnitId],
) {
    let mut selected = Vec::new();
    for demand in demands {
        selected.extend(
            assigned
                .iter()
                .copied()
                .filter(|id| {
                    !enlisted.contains(id)
                        && unit(obs, *id).is_some_and(|member| member.kind == demand.kind)
                })
                .take(demand.count),
        );
        let have = selected
            .iter()
            .filter(|id| unit(obs, **id).is_some_and(|member| member.kind == demand.kind))
            .count();
        let mut have = have;
        for member in &obs.my_units {
            if have >= demand.count {
                break;
            }
            if member.kind == demand.kind
                && !enlisted.contains(&member.id)
                && !selected.contains(&member.id)
            {
                selected.push(member.id);
                have += 1;
            }
        }
    }
    selected.sort_unstable();
    selected.dedup();
    *assigned = selected;
}

pub(super) fn reservations(op: &AirOperation, plan: &AirPlan, obs: &Observation) -> Vec<UnitId> {
    AirRoster::from(op).live_members(plan.screen(), obs)
}

pub(super) fn queued(obs: &Observation, accepts: impl Fn(UnitKind) -> bool) -> usize {
    obs.my_queues
        .iter()
        .flatten()
        .filter(|kind| accepts(**kind))
        .count()
}

fn training_ticks(count: usize, kind: UnitKind) -> Tick {
    u64::try_from(count)
        .expect("the roster fits in addressable memory")
        .saturating_mul(u64::from(kind.stats().train_ticks))
}

pub(super) fn remaining_training_ticks(obs: &Observation, count: usize, kind: UnitKind) -> Tick {
    let mut front_progress: Vec<_> = obs
        .my_buildings
        .iter()
        .enumerate()
        .filter_map(|(index, building)| {
            (obs.my_queues.get(index)?.first() == Some(&kind))
                .then(|| obs.own_queue_progress(index))
                .flatten()
                .map(|progress| {
                    let remaining = kind.stats().train_ticks.saturating_sub(progress).max(1);
                    let completed = kind.stats().train_ticks.saturating_sub(remaining);
                    (Reverse(completed), building.id)
                })
        })
        .collect();
    front_progress.sort_unstable();
    let completed_ticks = front_progress
        .into_iter()
        .take(count)
        .map(|(Reverse(progress), _)| Tick::from(progress))
        .fold(0, Tick::saturating_add);
    training_ticks(count, kind).saturating_sub(completed_ticks)
}

pub(super) fn requirements_met(obs: &Observation, kind: UnitKind) -> bool {
    kind.stats().requires.iter().all(|required| {
        obs.my_buildings
            .iter()
            .any(|building| building.built && building.kind == *required)
    })
}

pub(super) fn has_producer(obs: &Observation, kind: UnitKind) -> bool {
    obs.my_buildings
        .iter()
        .any(|building| building.built && building.kind.base_stats().produces.contains(&kind))
}

pub(super) fn ready_to_reconnoiter(obs: &Observation) -> bool {
    let scout = Role::Scout.unit_for(obs.faction);
    obs.my_units.iter().any(|unit| unit.kind == scout)
        || queued(obs, |kind| kind == scout) > 0
        || (requirements_met(obs, scout) && has_producer(obs, scout))
}

pub(super) fn air_strike_members(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
) -> Vec<UnitId> {
    let mut units: Vec<_> = op
        .strike_aircraft
        .iter()
        .chain(plan.screen())
        .copied()
        .filter(|id| unit(obs, *id).is_some())
        .collect();
    units.sort_unstable();
    units.dedup();
    units
}

pub(super) fn unit(obs: &Observation, id: UnitId) -> Option<&UnitObs> {
    obs.my_units
        .binary_search_by_key(&id, |member| member.id)
        .ok()
        .map(|index| &obs.my_units[index])
}

pub(super) fn completed(obs: &Observation, kind: BuildingKind) -> usize {
    obs.my_buildings
        .iter()
        .filter(|building| building.built && building.kind == kind)
        .count()
}

pub(super) fn combat_roster(obs: &Observation) -> usize {
    obs.my_units
        .iter()
        .filter(|unit| !unit.kind.stats().weapons.is_empty())
        .count()
}

pub(super) fn is_artillery(kind: UnitKind) -> bool {
    matches!(kind, UnitKind::Bombard | UnitKind::Avalanche)
}

pub(super) fn is_strike_aircraft(kind: UnitKind, faction: oxide_sim::state::Faction) -> bool {
    kind == Role::AirGround.unit_for(faction) || kind == Role::Bomber.unit_for(faction)
}
