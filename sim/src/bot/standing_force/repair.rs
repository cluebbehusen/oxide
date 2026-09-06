use super::*;
use crate::ids::Target;

const HORIZON: Tick = 1_800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RepairWork {
    pub(crate) unit: UnitId,
    pub(crate) tile: TilePos,
    pub(crate) missing_hp: u32,
    pub(crate) max_hp: u32,
    pub(crate) cost: u32,
    pub(crate) repair_ticks: u32,
}

impl RepairWork {
    fn ticks(self) -> Tick {
        (u64::from(self.missing_hp) * u64::from(self.repair_ticks))
            .div_ceil(u64::from(self.max_hp.max(1)))
    }

    fn value(self, ticks: Tick) -> u64 {
        let hp = (ticks * u64::from(self.max_hp) / u64::from(self.repair_ticks.max(1)))
            .min(u64::from(self.missing_hp));
        let restored = hp * u64::from(self.cost) / u64::from(self.max_hp.max(1));
        restored.saturating_sub((restored * crate::stats::REPAIR_COST_PERMILLE).div_ceil(1000))
    }
}

pub(super) struct RepairDemand {
    pub(super) targets: Vec<StandingGroundTarget>,
    pub(super) value: u64,
    pub(super) producer: BuildingId,
    pub(super) ready_at: Tick,
}

#[derive(Clone, Copy)]
struct Worker {
    kind: UnitKind,
    tile: TilePos,
    ready: Tick,
    patient: Option<Target>,
}

/// Spend each worker's finite time once, including travel between patients.
fn apply_worker(
    worker: Worker,
    patients: &mut [RepairWork],
    routing: &mut ServiceRouting<'_>,
) -> u64 {
    let mut remaining = HORIZON.saturating_sub(worker.ready);
    let mut origin = worker.tile;
    let mut value = 0_u64;
    for patient in patients {
        if patient.missing_hp == 0
            || worker
                .patient
                .is_some_and(|target| target != Target::Unit(patient.unit))
        {
            continue;
        }
        let Some(travel) = routing.repair_travel(origin, patient.tile, worker.kind) else {
            continue;
        };
        let useful = remaining.saturating_sub(travel).min(patient.ticks());
        if useful == 0 {
            continue;
        }
        value = value.saturating_add(patient.value(useful));
        let hp = (useful * u64::from(patient.max_hp) / u64::from(patient.repair_ticks.max(1)))
            .min(u64::from(patient.missing_hp));
        patient.missing_hp -= u32::try_from(hp).expect("service is capped by patient HP");
        remaining = remaining.saturating_sub(travel).saturating_sub(useful);
        origin = patient.tile;
    }
    value
}

pub(crate) fn remaining_work(
    obs: &Observation,
    context: StandingForceContext<'_>,
    resources: &ResourceSnapshot,
    routing: &mut ServiceRouting<'_>,
) -> Vec<RepairWork> {
    let fallback;
    let source = if let Some(work) = context.repair_work {
        work
    } else {
        fallback = obs
            .my_units
            .iter()
            .filter(|unit| {
                unit.hp > 0
                    && unit.hp < unit.kind.stats().max_hp
                    && unit.body_domain() == Domain::Ground
            })
            .map(|unit| RepairWork {
                unit: unit.id,
                tile: unit.tile,
                missing_hp: unit.kind.stats().max_hp - unit.hp,
                max_hp: unit.kind.stats().max_hp,
                cost: unit.kind.stats().cost,
                repair_ticks: unit.kind.stats().train_ticks,
            })
            .collect::<Vec<_>>();
        &fallback
    };
    if source.is_empty() {
        return vec![];
    }
    let mut patients = source.to_vec();
    patients.sort_by_key(|patient| (Reverse(patient.value(patient.ticks())), patient.unit));
    let bays = obs
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::RepairBay)
        .map(|bay| (bay.anchor, bay.built))
        .chain(obs.my_units.iter().filter_map(|worker| {
            worker
                .founding
                .filter(|(kind, _)| *kind == BuildingKind::RepairBay)
                .map(|(_, anchor)| (anchor, false))
        }))
        .collect::<std::collections::BTreeSet<_>>();
    for (anchor, built) in bays {
        let delay = if built {
            0
        } else {
            u64::from(
                BuildingKind::RepairBay
                    .base_stats()
                    .construction
                    .expect("constructible Bay")
                    .build_ticks,
            )
        };
        let capacity = HORIZON.saturating_sub(delay) / crate::stats::REPAIR_BAY_PERIOD
            * u64::from(crate::stats::REPAIR_BAY_STEP);
        for patient in &mut patients {
            let dx = (anchor.x - patient.tile.x)
                .max(patient.tile.x - anchor.x - 1)
                .max(0);
            let dy = (anchor.y - patient.tile.y)
                .max(patient.tile.y - anchor.y - 1)
                .max(0);
            if chassis::fx::Fx::from_num(dx * dx + dy * dy)
                <= crate::stats::REPAIR_BAY_RADIUS * crate::stats::REPAIR_BAY_RADIUS
            {
                patient.missing_hp = patient
                    .missing_hp
                    .saturating_sub(u32::try_from(capacity).unwrap_or(u32::MAX));
            }
        }
    }
    let mut workers = obs
        .my_units
        .iter()
        .filter(|unit| {
            unit.hp > 0
                && unit.kind.stats().welder
                && !obs.my_queued_units.contains(&unit.id)
                && unit.site.is_none()
                && unit.founding.is_none()
                && unit.salvaging.is_none()
                && ((unit.repairing
                    && context
                        .funded_repairers
                        .is_none_or(|workers| workers.contains(&unit.id)))
                    || (unit.idle && !context.excluded_units.contains(&unit.id)))
        })
        .map(|unit| {
            (
                unit.id,
                Worker {
                    kind: unit.kind,
                    tile: unit.tile,
                    ready: 0,
                    patient: obs.repair_target(unit.id),
                },
            )
        })
        .collect::<Vec<_>>();
    workers.sort_by_key(|(id, worker)| (worker.patient.is_none(), *id));
    for (_, worker) in workers {
        apply_worker(worker, &mut patients, routing);
    }
    for lane in resources.producers() {
        let Some(building) = obs
            .my_buildings
            .iter()
            .find(|building| building.id == lane.producer)
        else {
            continue;
        };
        let Some(origin) =
            production_spawn_doorstep(obs, building, context.public_map, context.orientation)
        else {
            continue;
        };
        let owned = context
            .committed_production
            .iter()
            .filter(|commitment| commitment.matches(lane.producer, UnitKind::Tender))
            .count();
        for (_, ready_at) in lane
            .queued_readiness()
            .filter(|(kind, _)| *kind == UnitKind::Tender)
            .skip(owned)
        {
            apply_worker(
                Worker {
                    kind: UnitKind::Tender,
                    tile: origin,
                    ready: ready_at.saturating_sub(obs.tick),
                    patient: None,
                },
                &mut patients,
                routing,
            );
        }
    }
    patients
}

pub(super) fn unmet_work(
    obs: &Observation,
    context: StandingForceContext<'_>,
    resources: &ResourceSnapshot,
    routing: &mut ServiceRouting<'_>,
) -> Vec<RepairDemand> {
    let patients = remaining_work(obs, context, resources, routing);
    if patients.iter().all(|patient| patient.missing_hp == 0) {
        return vec![];
    }
    let mut demands = Vec::new();
    for lane in resources.producers() {
        let Some(timing) = lane.production_timing(&[UnitKind::Tender]) else {
            continue;
        };
        let Some(building) = obs
            .my_buildings
            .iter()
            .find(|building| building.id == lane.producer)
        else {
            continue;
        };
        let Some(origin) =
            production_spawn_doorstep(obs, building, context.public_map, context.orientation)
        else {
            continue;
        };
        let mut remaining = patients.clone();
        let value = apply_worker(
            Worker {
                kind: UnitKind::Tender,
                tile: origin,
                ready: timing.no_block_latest_ready_tick.saturating_sub(obs.tick),
                patient: None,
            },
            &mut remaining,
            routing,
        );
        if value <= u64::from(UnitKind::Tender.stats().cost) {
            continue;
        }
        let mut targets: Vec<_> = patients
            .iter()
            .zip(&remaining)
            .filter(|(before, after)| before.missing_hp > after.missing_hp)
            .map(|(patient, _)| StandingGroundTarget::point(patient.tile))
            .collect();
        targets.sort_unstable();
        targets.dedup();
        demands.push(RepairDemand {
            targets,
            value,
            producer: lane.producer,
            ready_at: timing.no_block_latest_ready_tick,
        });
    }
    demands
}
