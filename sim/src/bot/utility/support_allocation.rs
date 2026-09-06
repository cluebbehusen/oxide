//! Exact repair work and its cadence-bounded ordinary spending.

use super::*;
use crate::bot::allocation::{
    ClaimBundle, Confidence, ExecutionSafety, ProposalCase, StrategicValue, TimeToImpact, Urgency,
};
use crate::bot::trace::{RepairProgramTrace, SupportLifecycleReason, SupportLifecycleTrace};
use crate::ids::Target;
use chassis::Tick;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SupportKey {
    pub(crate) patient: Target,
    pub(crate) worker: UnitId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepairAssignment {
    pub(crate) key: SupportKey,
    pub(crate) accepted_at: Tick,
    pub(crate) funded_until: Tick,
    pub(crate) debit: u32,
    pub(crate) case: ProposalCase,
}

impl RepairAssignment {
    pub(in crate::bot) fn trace(&self) -> RepairProgramTrace {
        RepairProgramTrace {
            worker: self.key.worker,
            patient: self.key.patient,
            accepted_at: self.accepted_at,
            funded_until: self.funded_until,
            debit: self.debit,
        }
    }

    fn lifecycle(&self, tick: Tick, reason: SupportLifecycleReason) -> SupportLifecycleTrace {
        SupportLifecycleTrace {
            tick,
            program: self.trace(),
            reason,
        }
    }
    pub(crate) fn claims(&self, already_working: bool) -> ClaimBundle {
        // Observed construction-capable repairers already enter the ledger as
        // occupied builders. Their renewal reserves only the additional debit.
        let claims = ClaimBundle::new(
            self.debit,
            vec![],
            vec![],
            if already_working {
                vec![]
            } else {
                vec![self.key.worker]
            },
            vec![],
            vec![],
        )
        .expect("one exact repair actor and current-only debit are canonical");
        match self.key.patient {
            Target::Building(building) => claims.with_building(building),
            Target::Unit(_) => claims,
        }
    }

    pub(crate) fn intent(&self) -> Intent {
        match self.key.patient {
            Target::Unit(target) => Intent::RepairUnits {
                welders: vec![self.key.worker],
                target,
            },
            Target::Building(building) => Intent::RepairWith {
                worker: self.key.worker,
                building,
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SupportWork {
    pub(crate) repairs: Vec<RepairAssignment>,
    pub(crate) lifecycle: Vec<SupportLifecycleTrace>,
    evidence: std::collections::BTreeMap<Target, Tick>,
    observed_at: Option<Tick>,
}

#[derive(Clone)]
pub(in crate::bot) struct SupportWorkSnapshot {
    patients: Vec<Patient>,
    danger: std::sync::Arc<danger::HarvestDangerProjection>,
    pub(in crate::bot) protection: Vec<super::support_deployment::ProtectionRequest>,
}

impl SupportWorkSnapshot {
    pub(in crate::bot) fn unit_work(&self) -> Vec<crate::bot::standing_force::RepairWork> {
        self.patients
            .iter()
            .filter_map(|patient| {
                let Target::Unit(unit) = patient.target else {
                    return None;
                };
                Some(crate::bot::standing_force::RepairWork {
                    unit,
                    tile: patient.tile,
                    missing_hp: patient.missing,
                    max_hp: patient.max_hp,
                    cost: patient.basis,
                    repair_ticks: patient.ticks,
                })
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
struct Patient {
    target: Target,
    tile: TilePos,
    size: (i32, i32),
    missing: u32,
    hp: u32,
    max_hp: u32,
    basis: u32,
    value_basis: u32,
    ramp: u32,
    ticks: u32,
}

impl Patient {
    fn reserve(self, cadence: Tick) -> u32 {
        // A meter's phase is private. Bound both cumulative integer-HP and
        // milli-scrap rounding across the entire renewal interval, not each tick.
        let hp =
            (u128::from(self.ramp) * u128::from(cadence)).div_ceil(u128::from(self.ticks.max(1)));
        let debit = (hp * u128::from(self.basis) * u128::from(crate::stats::REPAIR_COST_PERMILLE))
            .div_ceil(u128::from(self.max_hp.max(1)) * 1000)
            .saturating_add(1);
        u32::try_from(debit).unwrap_or(u32::MAX)
    }
}

impl UtilityPolicy {
    pub(in crate::bot) fn observe_support_work(
        &mut self,
        snapshot: &SupportWorkSnapshot,
        now: Tick,
    ) {
        if self.support_work.observed_at == Some(now) {
            return;
        }
        self.support_work.observed_at = Some(now);
        self.support_work.evidence.retain(|target, _| {
            snapshot
                .patients
                .iter()
                .any(|patient| patient.target == *target)
        });
        for patient in &snapshot.patients {
            self.support_work
                .evidence
                .entry(patient.target)
                .or_insert(now);
        }
    }

    pub(in crate::bot) fn discretionary_support_work(
        &self,
        snapshot: &SupportWorkSnapshot,
        now: Tick,
        tuning: DifficultyTuning,
    ) -> SupportWorkSnapshot {
        SupportWorkSnapshot {
            patients: snapshot
                .patients
                .iter()
                .filter(|patient| {
                    self.support_work
                        .evidence
                        .get(&patient.target)
                        .is_some_and(|first_seen| {
                            now.saturating_sub(*first_seen) >= tuning.reaction_delay
                        })
                })
                .take(tuning.attention_slots)
                .copied()
                .collect(),
            danger: snapshot.danger.clone(),
            protection: snapshot.protection.clone(),
        }
    }

    pub(in crate::bot) fn support_work_snapshot(
        &self,
        context: EconomicInvestmentContext<'_>,
    ) -> SupportWorkSnapshot {
        let danger = self.harvest_danger_projection(
            context.obs,
            Some(context.unit_contacts),
            Some(context.building_contacts),
        );
        let patients = self.repair_patients(context.obs, &danger);
        SupportWorkSnapshot {
            patients,
            danger,
            protection: super::support_deployment::protection_requests(context),
        }
    }

    #[cfg(test)]
    fn fresh_repair_assignments(
        &self,
        context: EconomicInvestmentContext<'_>,
    ) -> Vec<RepairAssignment> {
        self.prepared_repair_assignments(context, &self.support_work_snapshot(context))
    }

    #[cfg(test)]
    fn fresh_repair_bays(&self, context: EconomicInvestmentContext<'_>) -> Vec<EconomicInvestment> {
        self.prepared_repair_bays(context, &self.support_work_snapshot(context))
    }

    #[cfg(test)]
    fn renew_repair_assignments(
        &mut self,
        context: EconomicInvestmentContext<'_>,
        available: u32,
        allow_repair: bool,
    ) -> Vec<RepairAssignment> {
        self.renew_prepared_repairs(
            context,
            &self.support_work_snapshot(context),
            available,
            allow_repair,
        )
    }
    #[cfg(test)]
    pub(super) fn admit_test_repair(
        &mut self,
        obs: &Observation,
        map: &PublicMapBriefing,
        worker: UnitId,
        patient: Target,
        intents: &mut Vec<Intent>,
    ) {
        let resources = ResourceSnapshot::from_observation(obs);
        let profile = crate::scenario::BotConfig::scripted(
            crate::scenario::BotDifficulty::Prime,
            crate::scenario::BotStance::Balanced,
            7,
        )
        .resolve_profile();
        let mut unavailable = Vec::new();
        for intent in intents.iter() {
            Self::claim_non_preemptible_intent_units(intent, &mut unavailable);
        }
        let candidates = self.fresh_repair_assignments(EconomicInvestmentContext {
            obs,
            resources: &resources,
            profile: &profile,
            briefing: map,
            orientation: super::super::orient::Orientation::for_home(
                obs,
                obs.my_buildings[0].anchor,
            ),
            unavailable: &unavailable,
            demands: &[],
            unit_contacts: &[],
            building_contacts: &[],
            cadence: 24,
            protected_scrap: 0,
            air_work: &[],
        });
        let proposal = candidates.iter()
            .find(|proposal| proposal.key.patient == patient && proposal.key.worker == worker)
            .cloned()
            .unwrap_or_else(|| panic!("expected {worker:?} -> {patient:?}; candidates {candidates:?}; workers {:?}; unavailable {unavailable:?}", obs.my_units));
        assert!(proposal.debit <= obs.scrap);
        assert!(self.commit_repair_assignment(proposal, obs, intents));
    }

    pub(super) fn stop_unfunded_repairs(&self, obs: &Observation, intents: &mut Vec<Intent>) {
        let mut preempted = Vec::new();
        for intent in intents.iter() {
            Self::claim_non_preemptible_intent_units(intent, &mut preempted);
        }
        let repairers = obs
            .my_units
            .iter()
            .filter(|unit| {
                unit.repairing
                    && !self.repair_is_funded(unit.id, obs.tick)
                    && !preempted.contains(&unit.id)
            })
            .map(|unit| unit.id)
            .collect::<Vec<_>>();
        if !repairers.is_empty() {
            intents.insert(0, Intent::StopUnits { units: repairers });
        }
    }

    pub(in crate::bot) fn prepared_repair_bays(
        &self,
        context: EconomicInvestmentContext<'_>,
        snapshot: &SupportWorkSnapshot,
    ) -> Vec<EconomicInvestment> {
        let obs = context.obs;
        let kind = BuildingKind::RepairBay;
        let stats = kind
            .base_stats()
            .construction
            .expect("a Bay is constructible");
        if obs.scrap < stats.cost
            || stats.requires.iter().any(|required| {
                !obs.my_buildings
                    .iter()
                    .any(|building| building.kind == *required && building.built)
            })
        {
            return Vec::new();
        }
        let patients = &snapshot.patients;
        if patients
            .iter()
            .map(|patient| {
                u64::from(patient.missing) * u64::from(patient.value_basis)
                    / u64::from(patient.max_hp.max(1))
            })
            .sum::<u64>()
            <= u64::from(stats.cost)
        {
            return Vec::new();
        }
        let builders = obs
            .my_units
            .iter()
            .filter(|unit| builder_is_free(obs, unit) && !context.unavailable.contains(&unit.id))
            .collect::<Vec<_>>();
        if builders.is_empty() {
            return Vec::new();
        }
        let covered = |anchor: TilePos, patient: &Patient| {
            let dx = (anchor.x - patient.tile.x)
                .max(patient.tile.x - anchor.x - 1)
                .max(0);
            let dy = (anchor.y - patient.tile.y)
                .max(patient.tile.y - anchor.y - 1)
                .max(0);
            let distance = chassis::fx::Fx::from_num(dx * dx + dy * dy);
            distance <= crate::stats::REPAIR_BAY_RADIUS * crate::stats::REPAIR_BAY_RADIUS
        };
        let existing = obs
            .my_buildings
            .iter()
            .filter(|building| building.kind == kind)
            .map(|building| {
                (
                    building.anchor,
                    if building.built {
                        0
                    } else {
                        u64::from(stats.build_ticks)
                    },
                )
            })
            .chain(obs.my_units.iter().filter_map(|worker| {
                worker
                    .founding
                    .filter(|(founding, _)| *founding == kind)
                    .map(|(_, anchor)| (anchor, u64::from(stats.build_ticks)))
            }))
            .collect::<std::collections::BTreeSet<_>>();
        let work = snapshot.unit_work();
        let funded_repairers: Vec<_> = self
            .support_work
            .repairs
            .iter()
            .map(|repair| repair.key.worker)
            .collect();
        let support_context =
            crate::bot::standing_force::StandingForceContext::new(context.unavailable, &[])
                .with_ground_routing(
                    crate::bot::standing_force::StandingGroundTarget::point(TilePos::new(0, 0)),
                    Some(context.briefing),
                    &[],
                    Some(context.orientation),
                )
                .with_repair_work(&work)
                .with_funded_repairers(&funded_repairers);
        let mut service_routes = crate::bot::standing_force::ServiceRouting::new(
            obs,
            Some(context.briefing),
            Some(context.orientation),
        );
        let unit_remaining: std::collections::BTreeMap<_, _> =
            crate::bot::standing_force::remaining_repair_work(
                obs,
                support_context,
                context.resources,
                &mut service_routes,
            )
            .into_iter()
            .map(|patient| (patient.unit, patient.missing_hp))
            .collect();
        let mut sites = Vec::new();
        for patient in patients {
            if let Some(anchor) = self
                .placement_near_where(obs, kind, patient.tile, |anchor| covered(anchor, patient))
            {
                sites.push(anchor);
            }
        }
        sites.sort_by_key(|tile| (tile.y, tile.x));
        sites.dedup();
        let mut geometry = super::defense::DefenseThinkContext::new_oriented(
            self,
            obs,
            context.briefing,
            context.unit_contacts,
            context.building_contacts,
            context.orientation,
        );
        let mut proposals = Vec::new();
        for anchor in sites {
            if !geometry.resource_access_survives(kind, anchor)
                || !geometry.future_ground_producer_egress_survives(kind, anchor)
            {
                continue;
            }
            let Some(builder) = geometry.safe_implicit_builder(self, kind, anchor, &builders)
            else {
                continue;
            };
            let worker = builders.iter().find(|worker| worker.id == builder).unwrap();
            let Some(distance) = geometry.builder_travel_cost(worker, kind, anchor) else {
                continue;
            };
            let delay = super::economic_value::travel_ticks(worker.kind, distance).saturating_add(
                u64::from(stats.build_ticks)
                    .div_ceil(u64::from(worker.kind.stats().build_rate.max(1))),
            );
            let service = 1_800_u64.saturating_sub(delay) / crate::stats::REPAIR_BAY_PERIOD
                * u64::from(crate::stats::REPAIR_BAY_STEP);
            let benefit = patients
                .iter()
                .filter(|patient| covered(anchor, patient))
                .map(|patient| {
                    let prior = existing
                        .iter()
                        .filter(|(anchor, _)| covered(*anchor, patient))
                        .map(|(_, delay)| {
                            1_800_u64.saturating_sub(*delay) / crate::stats::REPAIR_BAY_PERIOD
                                * u64::from(crate::stats::REPAIR_BAY_STEP)
                        })
                        .sum::<u64>();
                    let assigned = self
                        .support_work
                        .repairs
                        .iter()
                        .any(|repair| repair.key.patient == patient.target);
                    let remaining = if let Target::Unit(unit) = patient.target {
                        u64::from(unit_remaining.get(&unit).copied().unwrap_or(0))
                    } else {
                        let worker_service = if assigned {
                            1_800_u64 * u64::from(patient.ramp) / u64::from(patient.ticks.max(1))
                        } else {
                            0
                        };
                        u64::from(patient.missing).saturating_sub(prior + worker_service)
                    };
                    let restored = remaining.min(service) * u64::from(patient.value_basis)
                        / u64::from(patient.max_hp.max(1));
                    let repair_price = remaining.min(service)
                        * u64::from(patient.basis)
                        * crate::stats::REPAIR_COST_PERMILLE;
                    restored.saturating_sub(
                        repair_price.div_ceil(u64::from(patient.max_hp.max(1)) * 1000),
                    )
                })
                .sum::<u64>();
            if benefit <= u64::from(stats.cost) {
                continue;
            }
            proposals.push(EconomicInvestment {
                key: EconomicInvestmentKey::Build { kind, anchor },
                builder: Some(builder),
                cost: stats.cost,
                current_capital: stats.cost,
                observed_at: obs.tick,
                ready_at: obs.tick.saturating_add(delay),
                deadline: obs.tick.saturating_add(1_800),
                case: ProposalCase {
                    urgency: Urgency::Timely,
                    confidence: Confidence::Current,
                    value: if benefit > u64::from(stats.cost) * 2 {
                        StrategicValue::Decisive
                    } else {
                        StrategicValue::Material
                    },
                    time_to_impact: TimeToImpact::Near,
                    safety: ExecutionSafety::Secure,
                },
                benefit,
                personality: context.profile.traits.support,
                foregone_income: vec![],
            });
        }
        proposals.sort_by_key(|proposal| {
            (
                std::cmp::Reverse(proposal.benefit),
                proposal.ready_at,
                proposal.key,
            )
        });
        proposals
    }

    pub(in crate::bot) fn renew_prepared_repairs(
        &mut self,
        context: EconomicInvestmentContext<'_>,
        snapshot: &SupportWorkSnapshot,
        available: u32,
        allow_repair: bool,
    ) -> Vec<RepairAssignment> {
        self.support_work.lifecycle.clear();
        if self.support_work.repairs.is_empty() {
            return Vec::new();
        }
        if !allow_repair || available == 0 {
            self.support_work
                .lifecycle
                .extend(self.support_work.repairs.iter().map(|repair| {
                    repair.lifecycle(
                        context.obs.tick,
                        if allow_repair {
                            SupportLifecycleReason::Unfunded
                        } else {
                            SupportLifecycleReason::CoreRecovery
                        },
                    )
                }));
            self.support_work.repairs.clear();
            return Vec::new();
        }
        let obs = context.obs;
        let danger = &snapshot.danger;
        let patients = &snapshot.patients;
        let mut routes = routing::RouteProjection::ground_avoiding_with_public_terrain(
            obs,
            context.briefing,
            context.orientation,
            |tile| danger.contains(tile) || self.harvest_location_contested(tile),
        );
        let mut available = available;
        let mut retained = Vec::new();
        for repair in &self.support_work.repairs {
            let Some(patient) = patients
                .iter()
                .find(|patient| patient.target == repair.key.patient)
            else {
                self.support_work
                    .lifecycle
                    .push(repair.lifecycle(obs.tick, SupportLifecycleReason::PatientUnavailable));
                continue;
            };
            let Some(worker) = obs
                .my_units
                .iter()
                .find(|worker| worker.id == repair.key.worker && worker.hp > 0)
            else {
                self.support_work
                    .lifecycle
                    .push(repair.lifecycle(obs.tick, SupportLifecycleReason::WorkerUnavailable));
                continue;
            };
            if obs.repair_target(worker.id) != Some(patient.target)
                || obs.my_queued_units.contains(&worker.id)
                || context.unavailable.contains(&worker.id)
            {
                self.support_work
                    .lifecycle
                    .push(repair.lifecycle(obs.tick, SupportLifecycleReason::Preempted));
                continue;
            }
            if danger.contains(worker.tile) {
                self.support_work
                    .lifecycle
                    .push(repair.lifecycle(obs.tick, SupportLifecycleReason::UnsafeApproach));
                continue;
            }
            let reaches = match patient.target {
                Target::Unit(_) => {
                    routes.unit_reaches(worker, patient.tile)
                        && routes.command_path_avoids_blocked(worker.tile, patient.tile)
                }
                Target::Building(_) => {
                    routing::unit_reaches_build_site_via(
                        &mut routes,
                        worker,
                        patient.tile,
                        patient.size,
                    ) && self.repair_building_route_is_safe(context, snapshot, worker, patient)
                }
            };
            let debit = patient.reserve(context.cadence);
            if !reaches || debit > available {
                self.support_work.lifecycle.push(repair.lifecycle(
                    obs.tick,
                    if reaches {
                        SupportLifecycleReason::Unfunded
                    } else {
                        SupportLifecycleReason::UnsafeApproach
                    },
                ));
                continue;
            }
            available -= debit;
            let mut funded = repair.clone();
            funded.debit = debit;
            funded.funded_until = obs.tick.saturating_add(context.cadence);
            self.support_work
                .lifecycle
                .push(funded.lifecycle(obs.tick, SupportLifecycleReason::Renewed));
            retained.push(funded);
        }
        self.support_work.repairs = retained.clone();
        retained
    }

    pub(crate) fn commit_repair_assignment(
        &mut self,
        assignment: RepairAssignment,
        obs: &Observation,
        intents: &mut Vec<Intent>,
    ) -> bool {
        if assignment.accepted_at != obs.tick
            || assignment.funded_until <= obs.tick
            || self.support_work.repairs.iter().any(|work| {
                work.key.worker == assignment.key.worker
                    || work.key.patient == assignment.key.patient
            })
            || !obs.my_units.iter().any(|worker| {
                worker.id == assignment.key.worker
                    && worker.hp > 0
                    && worker.idle
                    && worker.player == obs.me
                    && !obs.my_queued_units.contains(&worker.id)
                    && match assignment.key.patient {
                        Target::Unit(id) => {
                            worker.kind.stats().welder
                                && id != worker.id
                                && obs.my_units.iter().any(|patient| {
                                    patient.id == id
                                        && patient.hp > 0
                                        && patient.hp < patient.kind.stats().max_hp
                                        && patient.body_domain() == Domain::Ground
                                })
                        }
                        Target::Building(id) => {
                            worker.kind.stats().harvest.is_some()
                                && obs.my_buildings.iter().any(|patient| {
                                    patient.id == id
                                        && patient.built
                                        && patient.hp > 0
                                        && patient.hp < patient.kind.tier_stats(patient.tier).max_hp
                                })
                        }
                    }
            })
        {
            return false;
        }
        intents.push(assignment.intent());
        self.support_work
            .lifecycle
            .push(assignment.lifecycle(obs.tick, SupportLifecycleReason::Accepted));
        self.support_work.repairs.push(assignment);
        self.support_work.repairs.sort_by_key(|repair| repair.key);
        true
    }

    pub(crate) fn repair_is_funded(&self, worker: UnitId, tick: Tick) -> bool {
        self.support_work
            .repairs
            .iter()
            .any(|repair| repair.key.worker == worker && repair.funded_until > tick)
    }

    fn repair_patients(
        &self,
        obs: &Observation,
        danger: &danger::HarvestDangerProjection,
    ) -> Vec<Patient> {
        let mut patients = Vec::new();
        for unit in &obs.my_units {
            let stats = unit.kind.stats();
            if unit.player != obs.me
                || unit.hp == 0
                || unit.hp >= stats.max_hp
                || unit.body_domain() != Domain::Ground
                || danger.contains(unit.tile)
                || self.harvest_location_contested(unit.tile)
            {
                continue;
            }
            patients.push(Patient {
                target: Target::Unit(unit.id),
                tile: unit.tile,
                size: (1, 1),
                missing: stats.max_hp - unit.hp,
                hp: unit.hp,
                max_hp: stats.max_hp,
                basis: stats.cost,
                value_basis: stats.cost,
                ramp: stats.max_hp,
                ticks: stats.train_ticks,
            });
        }
        for building in &obs.my_buildings {
            let stats = building.kind.tier_stats(building.tier);
            if building.player != obs.me
                || !building.built
                || building.hp == 0
                || building.hp >= stats.max_hp
                || self.repair_patient_unsafe(building, danger)
                || obs
                    .my_units
                    .iter()
                    .any(|unit| unit.salvaging == Some(building.id))
                || self.economic_saving.as_ref().is_some_and(|saving| {
                    matches!(saving.key,
                    EconomicInvestmentKey::Upgrade { building: id, .. } if id == building.id)
                })
            {
                continue;
            }
            let (ticks, basis) = if building.kind == BuildingKind::Foundry {
                (
                    crate::stats::FOUNDRY_REPAIR_TICKS,
                    crate::stats::FOUNDRY_REPAIR_PRICE,
                )
            } else {
                let Some(construction) = stats.construction else {
                    continue;
                };
                (construction.build_ticks, construction.cost)
            };
            patients.push(Patient {
                target: Target::Building(building.id),
                tile: building.anchor,
                size: stats.size,
                missing: stats.max_hp - building.hp,
                hp: building.hp,
                max_hp: stats.max_hp,
                basis,
                value_basis: stats
                    .construction
                    .map_or(basis, |construction| construction.cost),
                ramp: stats.max_hp - stats.max_hp / 5,
                ticks,
            });
        }
        patients.sort_by_key(|patient| {
            (
                std::cmp::Reverse(
                    u64::from(patient.missing) * u64::from(patient.value_basis)
                        / u64::from(patient.max_hp.max(1)),
                ),
                patient.target,
            )
        });
        patients
    }

    pub(in crate::bot) fn prepared_repair_assignments(
        &self,
        context: EconomicInvestmentContext<'_>,
        snapshot: &SupportWorkSnapshot,
    ) -> Vec<RepairAssignment> {
        let obs = context.obs;
        let danger = &snapshot.danger;
        let patients = &snapshot.patients;
        if patients.is_empty() {
            return Vec::new();
        }
        let mut routes = routing::RouteProjection::ground_avoiding_with_public_terrain(
            obs,
            context.briefing,
            context.orientation,
            |tile| danger.contains(tile) || self.harvest_location_contested(tile),
        );
        let mut proposals = Vec::new();
        for patient in patients {
            if self
                .support_work
                .repairs
                .iter()
                .any(|repair| repair.key.patient == patient.target)
            {
                continue;
            }
            let worker = obs
                .my_units
                .iter()
                .filter(|worker| {
                    worker.player == obs.me
                        && worker.hp > 0
                        && worker.idle
                        && !context.unavailable.contains(&worker.id)
                        && !obs.my_queued_units.contains(&worker.id)
                        && worker.site.is_none()
                        && worker.founding.is_none()
                        && worker.salvaging.is_none()
                        && !worker.repairing
                        && Target::Unit(worker.id) != patient.target
                        && match patient.target {
                            Target::Unit(_) => worker.kind.stats().welder,
                            Target::Building(_) => worker.kind.stats().harvest.is_some(),
                        }
                })
                .filter(|worker| match patient.target {
                    Target::Unit(_) => {
                        routes.unit_reaches(worker, patient.tile)
                            && routes.direct_line_avoids_blocked(worker.tile, patient.tile)
                    }
                    Target::Building(_) => routing::unit_reaches_build_site_via(
                        &mut routes,
                        worker,
                        patient.tile,
                        patient.size,
                    ),
                })
                .min_by_key(|worker| (worker.tile.manhattan(patient.tile), worker.id));
            let Some(worker) = worker else { continue };
            let safe = match patient.target {
                Target::Unit(_) => routes.command_path_avoids_blocked(worker.tile, patient.tile),
                Target::Building(_) => {
                    self.repair_building_route_is_safe(context, snapshot, worker, patient)
                }
            };
            if !safe {
                continue;
            }
            proposals.push(RepairAssignment {
                key: SupportKey {
                    worker: worker.id,
                    patient: patient.target,
                },
                accepted_at: obs.tick,
                funded_until: obs.tick.saturating_add(context.cadence),
                debit: patient.reserve(context.cadence),
                case: ProposalCase {
                    urgency: if patient.hp < patient.max_hp / 2 {
                        Urgency::Pressing
                    } else {
                        Urgency::Timely
                    },
                    confidence: Confidence::Current,
                    value: StrategicValue::Material,
                    time_to_impact: TimeToImpact::Near,
                    safety: ExecutionSafety::Secure,
                },
            });
        }
        proposals
    }

    fn repair_building_route_is_safe(
        &self,
        context: EconomicInvestmentContext<'_>,
        snapshot: &SupportWorkSnapshot,
        worker: &UnitObs,
        patient: &Patient,
    ) -> bool {
        routing::build_command_path_avoids_with_public_terrain_and_orientation(
            context.obs,
            context.briefing,
            worker,
            routing::BuildCommandTarget {
                anchor: patient.tile,
                size: patient.size,
                defer: false,
            },
            context.orientation,
            |tile| snapshot.danger.contains(tile) || self.harvest_location_contested(tile),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::orient::Orientation;
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};

    fn fixture() -> (
        Observation,
        PublicMapBriefing,
        crate::bot::profile::ResolvedProfile,
    ) {
        let scenario = crate::Scenario::skirmish();
        let mut obs = Observation::omniscient(&scenario.build().unwrap(), PlayerId(0));
        obs.tick = 120;
        obs.scrap = 1_000;
        obs.enemy_units.clear();
        obs.enemy_buildings.clear();
        obs.blips.clear();
        for building in &mut obs.my_buildings {
            building.hp = building.kind.tier_stats(building.tier).max_hp;
        }
        (
            obs,
            PublicMapBriefing::from_scenario(&scenario).unwrap(),
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 7).resolve_profile(),
        )
    }

    fn context<'a>(
        obs: &'a Observation,
        map: &'a PublicMapBriefing,
        profile: &'a crate::bot::profile::ResolvedProfile,
        resources: &'a ResourceSnapshot,
    ) -> EconomicInvestmentContext<'a> {
        EconomicInvestmentContext {
            obs,
            resources,
            profile,
            briefing: map,
            orientation: Orientation::for_home(obs, obs.my_buildings[0].anchor),
            unavailable: &[],
            demands: &[],
            unit_contacts: &[],
            building_contacts: &[],
            cadence: 12,
            protected_scrap: 0,
            air_work: &[],
        }
    }

    #[test]
    fn bay_admission_values_local_damage_and_delayed_overlapping_service() {
        let (mut obs, mut map, mut profile) = fixture();
        obs.visible.fill(true);
        obs.explored.fill(true);
        obs.known_rock.clear();
        obs.known_peaks.clear();
        obs.known_frames.clear();
        obs.known_scrap.clear();
        map.non_ground_terrain.clear();
        map.extractor_frames.clear();
        map.initial_scrap.clear();
        obs.my_units.truncate(1);
        obs.my_units[0].tile = TilePos::new(8, 9);
        let prototype = obs.my_units[0].clone();
        for index in 0..8 {
            obs.my_units.push(UnitObs {
                id: UnitId(100 + index),
                kind: UnitKind::Avalanche,
                hp: 100,
                tile: TilePos::new(12 + index as i32 % 4, 10 + index as i32 / 4),
                ..prototype.clone()
            });
        }
        profile.traits.support = 0;
        let policy = UtilityPolicy::new();
        let quote = |obs: &Observation| {
            let resources = ResourceSnapshot::from_observation(obs);
            policy.fresh_repair_bays(context(obs, &map, &profile, &resources))
        };
        let proposals = quote(&obs);
        assert!(
            !proposals.is_empty(),
            "finite local damage can justify a Bay without a personality gate"
        );
        let EconomicInvestmentKey::Build { anchor, .. } = proposals[0].key else {
            unreachable!()
        };
        let mut covered = obs.clone();
        covered.my_buildings.push(BuildingObs {
            id: BuildingId(100),
            player: obs.me,
            kind: BuildingKind::RepairBay,
            anchor,
            hp: BuildingKind::RepairBay.base_stats().max_hp,
            built: false,
            seen: true,
            tier: 0,
        });
        covered.my_queues.push(vec![]);
        covered.my_queue_progress.push(0);
        assert!(
            quote(&covered).is_empty(),
            "pending local healing reduces marginal return before it is live"
        );
        covered.my_buildings.last_mut().unwrap().anchor = TilePos::new(30, 20);
        assert!(
            !quote(&covered).is_empty(),
            "a remote Bay neither supplies local service nor imposes a global cap"
        );
    }

    #[test]
    fn healthy_own_roster_has_no_repair_or_bay_alternative() {
        let (obs, map, profile) = fixture();
        let resources = ResourceSnapshot::from_observation(&obs);
        let policy = UtilityPolicy::new();
        assert!(
            policy
                .fresh_repair_assignments(context(&obs, &map, &profile, &resources))
                .is_empty()
        );
        assert!(
            policy
                .fresh_repair_bays(context(&obs, &map, &profile, &resources))
                .is_empty()
        );
    }

    #[test]
    fn exact_repair_renews_without_reissuing_and_stops_when_unfunded() {
        let (mut obs, map, profile) = fixture();
        obs.my_buildings[0].hp /= 2;
        let resources = ResourceSnapshot::from_observation(&obs);
        let mut policy = UtilityPolicy::new();
        let proposal = policy
            .fresh_repair_assignments(context(&obs, &map, &profile, &resources))
            .remove(0);
        let key = proposal.key;
        let mut intents = Vec::new();
        assert!(policy.commit_repair_assignment(proposal.clone(), &obs, &mut intents));
        assert_eq!(intents, vec![proposal.intent()]);
        assert_eq!(
            policy.support_work.lifecycle[0].reason,
            SupportLifecycleReason::Accepted
        );
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == key.worker)
            .unwrap()
            .repairing = true;
        obs.my_repair_targets = vec![(key.worker, key.patient)];
        obs.tick += 12;
        let resources = ResourceSnapshot::from_observation(&obs);
        let renewed = policy.renew_repair_assignments(
            context(&obs, &map, &profile, &resources),
            proposal.debit,
            true,
        );
        assert_eq!(renewed.len(), 1);
        assert_eq!(renewed[0].key, key);
        assert_eq!(renewed[0].accepted_at, proposal.accepted_at);
        assert_eq!(renewed[0].funded_until, obs.tick + 12);
        assert_eq!(
            policy.support_work.lifecycle[0].reason,
            SupportLifecycleReason::Renewed
        );
        let mut commands = Vec::new();
        policy.stop_unfunded_repairs(&obs, &mut commands);
        assert!(commands.is_empty());
        assert!(
            policy
                .renew_repair_assignments(context(&obs, &map, &profile, &resources), 0, true)
                .is_empty()
        );
        policy.stop_unfunded_repairs(&obs, &mut commands);
        assert_eq!(
            policy.support_work.lifecycle[0].reason,
            SupportLifecycleReason::Unfunded
        );
        assert_eq!(
            commands,
            vec![Intent::StopUnits {
                units: vec![key.worker]
            }]
        );
    }

    #[test]
    fn changed_patient_releases_only_its_exact_repair_ownership() {
        let (mut obs, map, profile) = fixture();
        obs.my_buildings[0].hp /= 2;
        let resources = ResourceSnapshot::from_observation(&obs);
        let mut policy = UtilityPolicy::new();
        let proposal = policy
            .fresh_repair_assignments(context(&obs, &map, &profile, &resources))
            .remove(0);
        let key = proposal.key;
        assert!(policy.commit_repair_assignment(proposal, &obs, &mut Vec::new()));
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == key.worker)
            .unwrap()
            .repairing = true;
        obs.my_repair_targets = vec![(key.worker, Target::Building(BuildingId(999)))];
        let resources = ResourceSnapshot::from_observation(&obs);
        assert!(
            policy
                .renew_repair_assignments(context(&obs, &map, &profile, &resources), 1_000, true)
                .is_empty()
        );
    }

    #[test]
    fn repair_reserve_bounds_every_meter_phase_over_a_decision_interval() {
        for kind in [
            UnitKind::Sentinel,
            UnitKind::Bombard,
            UnitKind::Tender,
            UnitKind::Condor,
        ] {
            let stats = kind.stats();
            let patient = Patient {
                target: Target::Unit(UnitId(1)),
                tile: TilePos::new(0, 0),
                size: (1, 1),
                missing: stats.max_hp,
                hp: 1,
                max_hp: stats.max_hp,
                basis: stats.cost,
                value_basis: stats.cost,
                ramp: stats.max_hp,
                ticks: stats.train_ticks,
            };
            for cadence in [1, 6, 12, 24] {
                for phase in 0..stats.train_ticks.saturating_mul(2) {
                    let actual = (phase..phase + cadence)
                        .map(|tick| crate::stats::unit_repair_debit(kind, tick))
                        .sum::<u32>();
                    assert!(
                        actual <= patient.reserve(u64::from(cadence)),
                        "{kind:?} phase {phase} cadence {cadence}: {actual}"
                    );
                }
            }
        }
    }
}
