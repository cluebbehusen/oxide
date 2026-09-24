//! Exact economic actions derived from finite work and unmet capability demand.

use super::economic_value::{CapacityReturn, RecurringReturn, WorkerService, investment_horizon};
use super::*;
use crate::allocation::{
    Confidence, ExecutionSafety, ProposalCase, StrategicValue, TimeToImpact, Urgency,
};
use crate::intelligence::StrategicIntelligence;
use crate::navigation::service::ServiceRoutes;
use crate::navigation::travel::travel_ticks;
#[cfg(test)]
use crate::observation::ObservationData;
use crate::orient::Orientation;
use crate::query_work::QueryPurpose;
use crate::resources::ProducerEgress;
use crate::standing_force::CapabilityDemand;
use serde::Serialize;

/// Canonical identity of an economic action, also exposed in decision traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum EconomicInvestmentKey {
    /// One worker trained at a specific completed producer.
    Train {
        /// Worker kind to train.
        kind: UnitKind,
        /// Exact completed producer.
        producer: BuildingId,
        /// Canonical safe-work component.
        service: TilePos,
    },
    /// One foundation at an exact footprint.
    Build {
        /// Kind of foundation.
        kind: BuildingKind,
        /// Exact footprint anchor.
        anchor: TilePos,
    },
    /// One irreversible self-refit of an owned building.
    Upgrade {
        /// Exact owned building.
        building: BuildingId,
        /// Target upgrade tier.
        tier: u8,
    },
}

impl Ord for EconomicInvestmentKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn key(action: EconomicInvestmentKey) -> (u8, u32, i32, i32, u32) {
            match action {
                EconomicInvestmentKey::Train {
                    kind,
                    producer,
                    service,
                } => (0, kind as u32, service.y, service.x, producer.0),
                EconomicInvestmentKey::Build { kind, anchor } => {
                    (1, kind as u32, anchor.y, anchor.x, 0)
                }
                EconomicInvestmentKey::Upgrade { building, tier } => {
                    (2, u32::from(tier), 0, 0, building.0)
                }
            }
        }
        key(*self).cmp(&key(*other))
    }
}

impl PartialOrd for EconomicInvestmentKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct EconomicInvestment {
    pub(crate) key: EconomicInvestmentKey,
    pub(crate) builder: Option<UnitId>,
    pub(crate) cost: u32,
    pub(crate) valuation_cost: u32,
    pub(crate) current_capital: u32,
    pub(crate) observed_at: u64,
    pub(crate) ready_at: u64,
    pub(crate) deadline: u64,
    pub(crate) fund_by: u64,
    pub(crate) case: ProposalCase,
    pub(crate) benefit: u64,
    pub(crate) personality: u8,
    pub(crate) foregone_income: Vec<crate::allocation::ForecastClaim>,
}

impl EconomicInvestment {
    pub(crate) fn build(&self) -> Option<(BuildingKind, TilePos, UnitId)> {
        match self.key {
            EconomicInvestmentKey::Build { kind, anchor } => Some((kind, anchor, self.builder?)),
            _ => None,
        }
    }

    pub(crate) fn intent(&self) -> Intent {
        match self.key {
            EconomicInvestmentKey::Train { kind, producer, .. } => Intent::TrainAt {
                building: producer,
                kind,
            },
            EconomicInvestmentKey::Build { kind, anchor } => Intent::BuildWith {
                builder: self.builder.expect("construction quotes bind a builder"),
                kind,
                anchor,
            },
            EconomicInvestmentKey::Upgrade { building, .. } => Intent::Upgrade { building },
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct EconomicInvestmentContext<'a> {
    pub(crate) evidence: crate::utility::DecisionEvidence<'a>,
    pub(crate) obs: &'a Observation,
    pub(crate) resources: &'a ResourceSnapshot,
    pub(crate) profile: &'a ResolvedProfile,
    pub(crate) briefing: &'a PublicMapBriefing,
    pub(crate) orientation: Orientation,
    pub(crate) unavailable: &'a [UnitId],
    pub(crate) demands: &'a [CapabilityDemand],
    pub(crate) unit_contacts: &'a [UnitContact],
    pub(crate) building_contacts: &'a [BuildingContact],
    pub(crate) cadence: u64,
    pub(crate) protected_scrap: u32,
    pub(crate) obligations: &'a [crate::allocation::ImportedObligation],
    pub(crate) air_work: &'a [AirCapacityDemand],
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct AirCapacityDemand {
    pub(crate) work_ticks: u64,
    pub(crate) deadline: u64,
    pub(crate) kind: UnitKind,
    pub(crate) service: crate::allocation::StandingForceServiceKey,
}

mod funding;
mod quotes;
pub(super) use funding::FundingCalendar;
pub(crate) use quotes::EconomicQuotes;

impl UtilityPolicy {
    pub(crate) fn economic_quotes<'a>(
        &'a self,
        context: EconomicInvestmentContext<'a>,
    ) -> EconomicQuotes<'a> {
        EconomicQuotes::new(self, context)
    }

    pub(crate) fn economic_saving(&self) -> Option<&EconomicInvestment> {
        self.state.economic_saving.as_ref()
    }

    pub(crate) fn has_economic_foundation(&self) -> bool {
        self.state.economic_foundation.is_some()
    }

    pub(crate) fn economic_foundation(&self) -> Option<&EconomicInvestment> {
        self.state.economic_foundation.as_ref()
    }

    pub(crate) fn commit_economic_investment(
        &mut self,
        proposal: EconomicInvestment,
        current_funding: u32,
        intents: &mut Vec<Intent>,
    ) {
        if matches!(proposal.key, EconomicInvestmentKey::Train { .. }) {
            return;
        }
        let mut proposal = proposal;
        proposal.current_capital = current_funding.min(proposal.cost);
        if current_funding >= proposal.cost {
            intents.push(proposal.intent());
            if proposal.build().is_some() {
                self.state.economic_foundation = Some(proposal);
            }
            self.state.economic_saving = None;
        } else {
            self.state.economic_saving = Some(proposal);
        }
    }

    pub(crate) fn refresh_economic_saving(
        &mut self,
        context: EconomicInvestmentContext<'_>,
        core_ready: bool,
    ) {
        if let Some(plan) = self.state.economic_foundation.take()
            && let Some((kind, anchor, builder)) = plan.build()
        {
            let paid = context
                .obs
                .my_buildings
                .iter()
                .any(|building| building.kind == kind && building.anchor == anchor);
            let founding = context
                .obs
                .my_units
                .iter()
                .any(|unit| unit.id == builder && unit.founding == Some((kind, anchor)));
            if !paid && founding {
                let valid = core_ready
                    && context.obs.tick < plan.deadline
                    && self.placement_geometry_valid_except(
                        context.obs,
                        kind,
                        anchor,
                        Some((kind, anchor)),
                        FoundationCancellations::default(),
                    )
                    && self.deferred_claim_has_safe_founder(
                        context.obs,
                        (kind, anchor),
                        Some(context.unit_contacts),
                        Some(context.building_contacts),
                        context.briefing,
                    );
                if valid {
                    self.state.economic_foundation = Some(plan);
                } else {
                    self.state.economic_cancelled_founder = Some((builder, kind, anchor));
                    self.state.economic_retry_at = context.obs.tick.saturating_add(600);
                }
            }
        }
        let Some(saving) = self.state.economic_saving.as_ref() else {
            return;
        };
        let transitioned = saving.build().is_some_and(|(kind, anchor, _)| {
            context
                .obs
                .my_buildings
                .iter()
                .any(|building| building.kind == kind && building.anchor == anchor)
                || Self::deferred_claims(context.obs).contains(&(kind, anchor))
        }) || match saving.key {
            EconomicInvestmentKey::Upgrade { building, tier } => context
                .obs
                .my_buildings
                .iter()
                .any(|owned| owned.id == building && owned.tier >= tier),
            _ => false,
        };
        if transitioned {
            self.state.economic_saving = None;
            return;
        }
        let now = context.obs.tick;
        if !core_ready || now >= saving.deadline || now > saving.fund_by {
            self.state.economic_saving = None;
            self.state.economic_retry_at = now.saturating_add(600);
            return;
        }
        let original = saving.clone();
        let mut refreshed = self.economic_quotes(context).investments();
        if let Some(mut proposal) = refreshed.pop() {
            proposal.observed_at = original.observed_at;
            self.state.economic_saving = Some(proposal);
        } else {
            self.state.economic_saving = None;
            self.state.economic_retry_at = now.saturating_add(600);
        }
    }
}

fn foregone_income(
    resources: &ResourceSnapshot,
    source: BuildingId,
    starts_at: u64,
    deadline: u64,
    cadence: u64,
) -> Vec<crate::allocation::ForecastClaim> {
    let mut claims = Vec::new();
    if cadence == 0 {
        return claims;
    }
    let mut decision = resources.forecast().observed_at();
    let mut prior = 0;
    while decision < deadline
        && let Some(next) = decision.checked_add(cadence)
    {
        let total = resources
            .forecast()
            .source_income_through(source, (next - 1).min(deadline.saturating_sub(1)))
            .amount()
            .saturating_sub(
                resources
                    .forecast()
                    .source_income_through(source, starts_at.saturating_sub(1))
                    .amount(),
            );
        let amount = total.saturating_sub(prior);
        if amount > 0 {
            claims.push(crate::allocation::ForecastClaim {
                through: next,
                amount,
            });
        }
        prior = total;
        decision = next;
    }
    claims
}

pub(super) fn economic_case(benefit: u64, cost: u32, delay: u64) -> ProposalCase {
    ProposalCase {
        urgency: Urgency::Timely,
        confidence: Confidence::Current,
        value: if benefit >= u64::from(cost).saturating_mul(2) {
            StrategicValue::Material
        } else {
            StrategicValue::Incremental
        },
        time_to_impact: if delay <= 600 {
            TimeToImpact::Near
        } else {
            TimeToImpact::Patient
        },
        safety: ExecutionSafety::Managed,
    }
}

fn infrastructure_case(
    context: &EconomicInvestmentContext<'_>,
    kind: BuildingKind,
    benefit: u64,
    cost: u32,
    delay: u64,
) -> ProposalCase {
    let mut case = economic_case(benefit, cost, delay);
    if kind == BuildingKind::Extractor {
        return case;
    }
    if kind == BuildingKind::Airworks && !context.air_work.is_empty() {
        case.confidence = Confidence::Supported;
        return case;
    }
    if let Some(demand) = context
        .demands
        .iter()
        .filter(|demand| {
            kind == BuildingKind::Reclaimer
                || kind.base_stats().produces.contains(&demand.kind)
                || next_infrastructure(context.obs, demand.kind) == Some(kind)
        })
        .max_by_key(|demand| {
            (
                demand.case.confidence as u8,
                demand.case.urgency as u8,
                demand.case.value as u8,
            )
        })
    {
        case.confidence = demand.case.confidence;
        if next_infrastructure(context.obs, demand.kind) == Some(kind) {
            case.value = StrategicValue::Material;
        }
        case.urgency = match demand.case.urgency {
            Urgency::Pressing | Urgency::Timely => Urgency::Timely,
            Urgency::Developmental => Urgency::Developmental,
        };
    }
    case
}

fn useful_demand_scrap(demands: &[CapabilityDemand]) -> u64 {
    let mut services = std::collections::BTreeMap::new();
    for demand in demands {
        let amount = demand
            .units_needed()
            .saturating_mul(u64::from(demand.kind.stats().cost));
        let current = services
            .entry((demand.service, demand.reason as u8))
            .or_insert(0u64);
        *current = (*current).max(amount);
    }
    services.into_values().fold(0, u64::saturating_add)
}

fn unfunded_income_evidence(
    demands: &[CapabilityDemand],
    mut supplied: u64,
) -> Option<ProposalCase> {
    let mut services = std::collections::BTreeMap::new();
    for demand in demands {
        let amount = demand
            .units_needed()
            .saturating_mul(u64::from(demand.kind.stats().cost));
        let entry = services
            .entry((demand.service, demand.reason as u8))
            .or_insert((0, demand.case));
        if amount > entry.0 {
            *entry = (amount, demand.case);
        }
    }
    let mut needs = services.into_values().collect::<Vec<_>>();
    needs.sort_by_key(|(_, case)| {
        (
            std::cmp::Reverse(case.confidence as u8),
            std::cmp::Reverse(case.urgency as u8),
        )
    });
    for (amount, case) in needs {
        if amount > supplied {
            return Some(case);
        }
        supplied -= amount;
    }
    None
}

fn projected_recurring_output(
    obs: &Observation,
    resources: &ResourceSnapshot,
    horizon: u64,
) -> u64 {
    let recurring = |kind: BuildingKind, tier, anchor| {
        let remaining = horizon.saturating_sub(
            kind.tier_stats(tier)
                .construction
                .map_or(0, |construction| u64::from(construction.build_ticks)),
        );
        match (kind, tier) {
            (BuildingKind::Reclaimer, 0) => remaining / oxide_sim::stats::RECLAIMER_PERIOD,
            (BuildingKind::Reclaimer, _) => remaining / oxide_sim::stats::REFINERY_PERIOD,
            (BuildingKind::Extractor, _) => {
                let rate = if UtilityPolicy::frame_has_foundry_support(obs, anchor) {
                    oxide_sim::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
                } else {
                    oxide_sim::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE
                };
                remaining.saturating_mul(u64::from(rate))
                    / (u64::from(oxide_sim::TICKS_PER_SECOND) * 60)
            }
            _ => 0,
        }
    };
    let completed = u64::from(
        resources
            .forecast()
            .income_through(obs.tick.saturating_add(horizon))
            .amount(),
    );
    obs.my_buildings
        .iter()
        .filter(|building| !building.built)
        .map(|building| recurring(building.kind, building.tier, building.anchor))
        .chain(
            UtilityPolicy::deferred_claims(obs)
                .into_iter()
                .filter(|(kind, anchor)| {
                    !obs.my_buildings
                        .iter()
                        .any(|building| building.kind == *kind && building.anchor == *anchor)
                })
                .map(|(kind, anchor)| recurring(kind, 0, anchor)),
        )
        .fold(completed, u64::saturating_add)
}

fn next_infrastructure(obs: &Observation, unit: UnitKind) -> Option<BuildingKind> {
    let producer = BuildingKind::ALL
        .into_iter()
        .find(|kind| kind.base_stats().produces.contains(&unit))?;
    fn missing(obs: &Observation, kind: BuildingKind) -> Option<BuildingKind> {
        if obs
            .my_buildings
            .iter()
            .any(|building| building.kind == kind)
            || UtilityPolicy::deferred_claims(obs)
                .iter()
                .any(|(pending, _)| *pending == kind)
        {
            return None;
        }
        let stats = kind.base_stats().construction?;
        for &requirement in stats.requires {
            if !obs
                .my_buildings
                .iter()
                .any(|building| building.kind == requirement && building.built)
            {
                return missing(obs, requirement);
            }
        }
        Some(kind)
    }
    for &requirement in unit.stats().requires {
        if !obs
            .my_buildings
            .iter()
            .any(|building| building.kind == requirement && building.built)
        {
            return missing(obs, requirement);
        }
    }
    missing(obs, producer)
}

struct InfrastructureContext<'a, 'r> {
    obs: &'a Observation,
    resources: &'a ResourceSnapshot,
    demands: &'a [CapabilityDemand],
    routes: &'r mut ServiceRoutes<'a>,
    briefing: &'a PublicMapBriefing,
    orientation: Orientation,
    air_work: &'a [AirCapacityDemand],
    protected_scrap: u32,
}

#[derive(Default)]
struct InfrastructureReturn {
    benefit: u64,
    case: Option<ProposalCase>,
}

fn capability_chain(obs: &Observation, unit: UnitKind, candidate: BuildingKind) -> (u64, u64) {
    fn visit(
        obs: &Observation,
        kind: BuildingKind,
        candidate: BuildingKind,
        seen: &mut BTreeSet<BuildingKind>,
    ) -> (u64, u64) {
        if kind == candidate
            || !seen.insert(kind)
            || obs
                .my_buildings
                .iter()
                .any(|building| building.kind == kind && building.built)
        {
            return (0, 0);
        }
        let Some(construction) = kind.base_stats().construction else {
            return (0, 0);
        };
        let paid = obs
            .my_buildings
            .iter()
            .any(|building| building.kind == kind);
        let deferred = UtilityPolicy::deferred_claims(obs)
            .iter()
            .any(|(pending, _)| *pending == kind);
        let mut result = (
            if paid || deferred {
                0
            } else {
                u64::from(construction.cost)
            },
            u64::from(construction.build_ticks),
        );
        for &requirement in construction.requires {
            let (cost, delay) = visit(obs, requirement, candidate, seen);
            result.0 = result.0.saturating_add(cost);
            result.1 = result.1.saturating_add(delay);
        }
        result
    }
    let mut seen = BTreeSet::new();
    let mut result = (0u64, 0u64);
    for kind in unit.stats().requires.iter().copied().chain(
        BuildingKind::ALL
            .into_iter()
            .filter(|kind| kind.base_stats().produces.contains(&unit)),
    ) {
        let (cost, delay) = visit(obs, kind, candidate, &mut seen);
        result.0 = result.0.saturating_add(cost);
        result.1 = result.1.saturating_add(delay);
    }
    result
}

fn infrastructure_benefit(
    context: &mut InfrastructureContext<'_, '_>,
    kind: BuildingKind,
    anchor: TilePos,
    horizon: u64,
    delay: u64,
) -> InfrastructureReturn {
    let obs = context.obs;
    let budget = u64::from(obs.scrap.saturating_sub(context.protected_scrap))
        .saturating_add(u64::from(
            context
                .resources
                .forecast()
                .income_through(obs.tick.saturating_add(horizon).saturating_sub(1))
                .amount(),
        ))
        .saturating_sub(u64::from(
            kind.base_stats().construction.map_or(0, |stats| stats.cost),
        ));
    let candidate = BuildingObs {
        provisional: false,
        id: BuildingId(u32::MAX),
        player: obs.me,
        kind,
        anchor,
        hp: kind.base_stats().max_hp,
        built: true,
        tier: 0,
        seen: true,
    };
    let ground_spawn = routing::production_spawn_doorstep(
        QueryPurpose::EconomicInvestment,
        obs,
        &candidate,
        Some(context.briefing),
        Some(context.orientation),
    );
    let ordinary = context
        .demands
        .iter()
        .filter_map(|demand| {
            let spawn = if demand.kind.stats().domain == Domain::Air {
                Some(crate::navigation::commands::air_production_spawn_tile(
                    &candidate,
                    Some(context.orientation),
                ))
            } else {
                ground_spawn
            }?;
            if !context
                .routes
                .origin_serves(spawn, demand.kind, demand.service)
            {
                return None;
            }
            if next_infrastructure(obs, demand.kind) == Some(kind) {
                let (chain_cost, chain_delay) = capability_chain(obs, demand.kind, kind);
                let units = horizon.saturating_sub(delay.saturating_add(chain_delay))
                    / u64::from(demand.kind.stats().train_ticks.max(1));
                let affordable =
                    budget.saturating_sub(chain_cost) / u64::from(demand.kind.stats().cost.max(1));
                return Some((
                    units
                        .min(demand.units_needed())
                        .min(affordable)
                        .saturating_mul(u64::from(demand.kind.stats().cost))
                        .saturating_sub(chain_cost),
                    demand,
                ));
            }
            if !kind.base_stats().produces.contains(&demand.kind) {
                return None;
            }
            let existing = context
                .resources
                .producers()
                .iter()
                .filter(|lane| lane.kind == kind)
                .filter(|lane| {
                    context.routes.producer_reaches_any(
                        lane.producer,
                        demand.kind,
                        &[demand.service],
                    )
                })
                .filter_map(|lane| lane.horizon_timing(&[demand.kind]))
                .filter(|timing| {
                    matches!(
                        timing.current_egress,
                        ProducerEgress::Open | ProducerEgress::NotRequired
                    )
                })
                .map(|timing| {
                    let first = timing.no_block_latest_ready_tick.saturating_sub(obs.tick);
                    if first > horizon {
                        0
                    } else {
                        (horizon - first) / u64::from(demand.kind.stats().train_ticks.max(1)) + 1
                    }
                })
                .fold(0, u64::saturating_add);
            let pending = obs
                .my_buildings
                .iter()
                .filter(|building| building.kind == kind && !building.built)
                .map(|building| building.anchor)
                .chain(
                    UtilityPolicy::deferred_claims(obs)
                        .into_iter()
                        .filter(|(pending, anchor)| {
                            *pending == kind
                                && !obs.my_buildings.iter().any(|building| {
                                    building.kind == kind && building.anchor == *anchor
                                })
                        })
                        .map(|(_, anchor)| anchor),
                )
                .filter(|anchor| {
                    let pending = BuildingObs {
                        anchor: *anchor,
                        ..candidate.clone()
                    };
                    let spawn = if demand.kind.stats().domain == Domain::Air {
                        Some(crate::navigation::commands::air_production_spawn_tile(
                            &pending,
                            Some(context.orientation),
                        ))
                    } else {
                        routing::production_spawn_doorstep(
                            QueryPurpose::EconomicInvestment,
                            obs,
                            &pending,
                            Some(context.briefing),
                            Some(context.orientation),
                        )
                    };
                    spawn.is_some_and(|spawn| {
                        context
                            .routes
                            .origin_serves(spawn, demand.kind, demand.service)
                    })
                })
                .map(|_| {
                    horizon.saturating_sub(u64::from(
                        kind.base_stats().construction.unwrap().build_ticks,
                    )) / u64::from(demand.kind.stats().train_ticks.max(1))
                })
                .fold(0, u64::saturating_add);
            Some((
                CapacityReturn {
                    horizon,
                    ready_after: delay,
                    train_ticks: u64::from(demand.kind.stats().train_ticks),
                    demanded_units: demand
                        .units_needed()
                        .min(budget / u64::from(demand.kind.stats().cost.max(1))),
                    existing_units: existing.saturating_add(pending),
                }
                .additional_units()
                .saturating_mul(u64::from(demand.kind.stats().cost)),
                demand,
            ))
        })
        .filter(|(benefit, _)| *benefit > 0)
        .max_by_key(|(benefit, demand)| {
            (
                *benefit,
                demand.case.confidence as u8,
                demand.case.urgency as u8,
                demand.kind,
                demand.service,
            )
        });
    let ordinary = ordinary.map_or_else(InfrastructureReturn::default, |(benefit, demand)| {
        let mut case = demand.case;
        if case.urgency == Urgency::Pressing {
            case.urgency = Urgency::Timely;
        }
        InfrastructureReturn {
            benefit,
            case: Some(case),
        }
    });
    if kind != BuildingKind::Airworks || context.air_work.is_empty() {
        return ordinary;
    }
    let construction_delay = u64::from(kind.base_stats().construction.unwrap().build_ticks);
    let mut lane = |building: &BuildingObs, ready_after| {
        let origin = crate::navigation::commands::air_production_spawn_tile(
            building,
            Some(context.orientation),
        );
        super::economic_capacity::AirCapacityLane {
            ready_after,
            serves: context
                .air_work
                .iter()
                .map(|demand| {
                    context
                        .routes
                        .origin_serves(origin, demand.kind, demand.service)
                })
                .collect(),
        }
    };
    let candidate_lane = lane(&candidate, delay);
    if !candidate_lane.serves.iter().any(|serves| *serves) {
        return ordinary;
    }
    let mut existing = obs
        .my_buildings
        .iter()
        .filter(|building| building.kind == kind)
        .map(|building| {
            lane(
                building,
                if building.built {
                    0
                } else {
                    construction_delay
                },
            )
        })
        .collect::<Vec<_>>();
    for (pending, anchor) in UtilityPolicy::deferred_claims(obs) {
        if pending == kind
            && !obs
                .my_buildings
                .iter()
                .any(|building| building.kind == pending && building.anchor == anchor)
        {
            existing.push(lane(
                &BuildingObs {
                    anchor,
                    ..candidate.clone()
                },
                construction_delay,
            ));
        }
    }
    let operational = super::economic_capacity::additional_air_capacity_value(
        context.air_work,
        obs.tick,
        &existing,
        &candidate_lane,
    );
    let operational = operational.min(budget);
    if operational > ordinary.benefit {
        let mut case = economic_case(
            operational,
            kind.base_stats().construction.unwrap().cost,
            delay,
        );
        case.confidence = Confidence::Supported;
        InfrastructureReturn {
            benefit: operational,
            case: Some(case),
        }
    } else {
        ordinary
    }
}

#[cfg(test)]
mod tests {
    use super::super::construction_checks::ConstructionChecks;
    use super::*;
    use crate::allocation::StandingForceServiceKey;
    use crate::standing_force::StandingForceReason;
    use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};

    fn building(id: u32, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        crate::test_support::building(id, PlayerId(0), kind, anchor)
    }

    fn worker(id: u32, tile: TilePos) -> UnitObs {
        crate::test_support::unit(id, PlayerId(0), UnitKind::Harvester, tile)
    }

    fn fixture() -> (Observation, PublicMapBriefing, ResolvedProfile) {
        let obs = Observation::from_data(ObservationData {
            tick: 120,
            scrap: 1_000,
            map_width: 40,
            map_height: 30,
            visible: vec![true; 1_200],
            explored: vec![true; 1_200],
            my_buildings: vec![building(1, BuildingKind::Foundry, TilePos::new(3, 12))],
            my_units: vec![worker(1, TilePos::new(8, 12))],
            my_queues: vec![Vec::new()],
            my_queue_progress: vec![0],
            ..crate::test_support::observation_data()
        });
        let map = PublicMapBriefing {
            regions: Default::default(),
            map_width: 40,
            map_height: 30,
            starting_foundries: Vec::new(),
            teams: vec![None, None],
            non_ground_terrain: Vec::new(),
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        };
        let profile = crate::profile::ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            7,
        ));
        (obs, map, profile)
    }

    fn demand(kind: UnitKind, unmet: u32) -> CapabilityDemand {
        CapabilityDemand {
            kind,
            service: StandingForceServiceKey::point(TilePos::new(30, 12)),
            reason: StandingForceReason::GroundPressure,
            case: economic_case(1_000, 100, 100),
            unmet,
            baseline: UnitKind::Sentinel,
            provider_value: 1,
        }
    }

    fn quotes(
        policy: &UtilityPolicy,
        obs: &Observation,
        map: &PublicMapBriefing,
        profile: &ResolvedProfile,
        demands: &[CapabilityDemand],
    ) -> Vec<EconomicInvestment> {
        let resources = ResourceSnapshot::from_observation(obs);
        policy
            .economic_quotes(EconomicInvestmentContext {
                evidence: Default::default(),
                obligations: &[],
                obs,
                resources: &resources,
                briefing: map,
                profile,
                orientation: Orientation::for_home(obs, TilePos::new(3, 12)),
                unavailable: &[],
                demands,
                unit_contacts: &[],
                building_contacts: &[],
                cadence: 12,
                protected_scrap: 0,
                air_work: &[],
            })
            .investments()
    }

    #[test]
    fn first_airworks_is_valued_as_a_complete_affordable_campaign() {
        let (mut obs, map, profile) = fixture();
        obs.scrap = 1200;
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(8, 5)));
        obs.my_buildings
            .push(building(3, BuildingKind::Crucible, TilePos::new(12, 5)));
        {
            let obs = &mut *obs;
            obs.my_queues.resize(obs.my_buildings.len(), Vec::new());
        }
        {
            let obs = &mut *obs;
            obs.my_queue_progress.resize(obs.my_buildings.len(), 0);
        }
        let mut target = building(90, BuildingKind::Foundry, TilePos::new(30, 12));
        target.player = PlayerId(1);
        obs.enemy_buildings.push(target);
        let offered = air_quotes(&obs, &map, &profile, &[]);
        assert!(
            !offered.is_empty(),
            "a serviceable scout, suppression and strike minimum should justify its first Airworks"
        );
        assert!(
            offered
                .iter()
                .all(|quote| quote.case.confidence == Confidence::Supported)
        );
        obs.scrap = 200;
        assert!(
            air_quotes(&obs, &map, &profile, &[]).is_empty(),
            "the building alone does not fund a campaign"
        );
        obs.scrap = 1200;
        obs.enemy_buildings[0].seen = false;
        assert!(
            air_quotes(&obs, &map, &profile, &[]).is_empty(),
            "remembered targets require new reconnaissance before this investment"
        );
        obs.enemy_buildings[0].seen = true;
        obs.my_buildings
            .push(building(4, BuildingKind::Airworks, TilePos::new(16, 5)));
        obs.my_queues.push(Vec::new());
        obs.my_queue_progress.push(0);
        assert!(
            air_quotes(&obs, &map, &profile, &[]).is_empty(),
            "bootstrap value cannot buy redundant factories"
        );
    }

    #[test]
    fn worker_quotes_require_visible_finite_work_not_a_roster_quota() {
        let (mut obs, map, profile) = fixture();
        let policy = UtilityPolicy::new();
        assert!(quotes(&policy, &obs, &map, &profile, &[]).is_empty());
        obs.known_wrecks = vec![
            (TilePos::new(12, 12), 50_000),
            (TilePos::new(13, 12), 50_000),
        ];
        assert!(
            quotes(&policy, &obs, &map, &profile, &[])
                .iter()
                .any(|quote| matches!(
                    quote.key,
                    EconomicInvestmentKey::Train {
                        kind: UnitKind::Harvester,
                        ..
                    }
                ))
        );
        obs.visible[12 * 40 + 12] = false;
        obs.visible[12 * 40 + 13] = false;
        assert!(quotes(&policy, &obs, &map, &profile, &[]).is_empty());
    }

    #[test]
    fn funded_current_work_cannot_lend_confidence_to_speculative_income_growth() {
        let current = demand(UnitKind::Tender, 1);
        let mut prior = demand(UnitKind::Sentinel, 100);
        prior.reason = StandingForceReason::ForceProjection;
        prior.case.confidence = Confidence::Prior;
        prior.case.urgency = Urgency::Developmental;
        let current_cost = current.units_needed() * u64::from(current.kind.stats().cost);
        let needs = [current, prior];
        assert_eq!(
            unfunded_income_evidence(&needs, 0).unwrap().confidence,
            Confidence::Current
        );
        let evidence = unfunded_income_evidence(&needs, current_cost).unwrap();
        assert_eq!(evidence.confidence, Confidence::Prior);
        assert_eq!(evidence.urgency, Urgency::Developmental);
        assert!(unfunded_income_evidence(&needs, useful_demand_scrap(&needs)).is_none());
    }

    #[test]
    fn foundry_capacity_reuses_expansion_admission_without_a_remote_resource_site() {
        let (mut obs, map, profile) = fixture();
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(3, 3)));
        obs.my_queues.push(Vec::new());
        obs.my_queue_progress.push(0);
        for index in 0..8 {
            let mut defender = worker(10 + index, TilePos::new(5 + index as i32, 16));
            defender.kind = UnitKind::Sentinel;
            obs.my_units.push(defender);
        }
        let policy = UtilityPolicy::new();
        let quote = |obs: &Observation, unmet| {
            let resources = ResourceSnapshot::from_observation(obs);
            let demands = [demand(UnitKind::Sentinel, unmet)];
            let dials = Dials::default();
            policy
                .economic_quotes(EconomicInvestmentContext {
                    evidence: Default::default(),
                    obligations: &[],
                    obs,
                    resources: &resources,
                    profile: &profile,
                    briefing: &map,
                    orientation: Orientation::for_home(obs, TilePos::new(3, 12)),
                    unavailable: &[],
                    demands: &demands,
                    unit_contacts: &[],
                    building_contacts: &[],
                    cadence: 12,
                    protected_scrap: 0,
                    air_work: &[],
                })
                .capacity_foundry(
                    &dials,
                    construction::FreshFoundryProposalContext {
                        home: TilePos::new(3, 12),
                        available_builders: &[UnitId(1)],
                        combat_core_exclusions: &[],
                        unit_contacts: &[],
                        building_contacts: &[],
                        public_map: &map,
                        same_think_intents: &[],
                        current_scrap: obs.scrap,
                        protected_reserve: 0,
                    },
                )
        };
        assert!(
            quote(&obs, 1_000).is_none(),
            "existing capacity covers the affordable army"
        );
        obs.scrap = 50_000;
        assert!(
            matches!(
                quote(&obs, 1_000),
                Some(construction::FreshFoundryInvestment::Ready(_))
            ),
            "useful demand beyond completed throughput can fund local Foundry capacity"
        );
        assert!(
            quote(&obs, 1).is_none(),
            "idle capacity is not an economic opportunity"
        );
        let mut pending = building(3, BuildingKind::Foundry, TilePos::new(15, 3));
        pending.built = false;
        obs.my_buildings.push(pending);
        assert!(
            quote(&obs, 1_000).is_none(),
            "a paid expansion retains ownership of the capacity channel"
        );
    }

    #[test]
    fn shared_economic_preparation_preserves_capacity_and_investment_quotes() {
        let (mut obs, map, profile) = fixture();
        obs.scrap = 50_000;
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(3, 3)));
        obs.my_queues.push(Vec::new());
        obs.my_queue_progress.push(0);
        for index in 0..8 {
            let mut defender = worker(10 + index, TilePos::new(5 + index as i32, 16));
            defender.kind = UnitKind::Sentinel;
            obs.my_units.push(defender);
        }
        let resources = ResourceSnapshot::from_observation(&obs);
        let demands = [demand(UnitKind::Sentinel, 1_000)];
        let context = EconomicInvestmentContext {
            evidence: Default::default(),
            obs: &obs,
            resources: &resources,
            profile: &profile,
            briefing: &map,
            orientation: Orientation::for_home(&obs, TilePos::new(3, 12)),
            unavailable: &[],
            demands: &demands,
            unit_contacts: &[],
            building_contacts: &[],
            cadence: 12,
            protected_scrap: 0,
            obligations: &[],
            air_work: &[],
        };
        let foundry = construction::FreshFoundryProposalContext {
            home: TilePos::new(3, 12),
            available_builders: &[UnitId(1)],
            combat_core_exclusions: &[],
            unit_contacts: &[],
            building_contacts: &[],
            public_map: &map,
            same_think_intents: &[],
            current_scrap: obs.scrap,
            protected_reserve: 0,
        };
        let dials = Dials::default();
        let shared_policy = UtilityPolicy::new();
        let separate_policy = shared_policy.clone();
        let expected_foundry = separate_policy
            .economic_quotes(context)
            .capacity_foundry(&dials, foundry);
        let expected = separate_policy.economic_quotes(context).investments();
        assert!(expected_foundry.is_some());
        assert!(!expected.is_empty());
        let mut shared = shared_policy.economic_quotes(context);
        assert_eq!(shared.capacity_foundry(&dials, foundry), expected_foundry);
        assert_eq!(shared.investments(), expected);
    }

    #[test]
    fn orphaned_paid_work_values_replacement_labor_without_a_worker_quota() {
        let (mut obs, map, profile) = fixture();
        obs.my_units.clear();
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(3, 3)));
        let mut orphan = building(3, BuildingKind::Crucible, TilePos::new(15, 12));
        orphan.built = false;
        obs.my_buildings.push(orphan.clone());
        obs.my_queues.resize(3, Vec::new());
        obs.my_queue_progress.resize(3, 0);
        let policy = UtilityPolicy::new();
        let worker_quotes = |obs: &Observation| {
            quotes(&policy, obs, &map, &profile, &[])
                .into_iter()
                .filter(|quote| matches!(quote.key, EconomicInvestmentKey::Train { .. }))
                .collect::<Vec<_>>()
        };
        let replacements = worker_quotes(&obs);
        assert!(replacements.iter().any(|quote| matches!(quote.key,
            EconomicInvestmentKey::Train { kind: UnitKind::Excavator, service, .. } if service == orphan.anchor
        )), "paid construction can justify a specialist even with no harvest sources");
        obs.my_queues[1].push(UnitKind::Excavator);
        assert!(
            worker_quotes(&obs).is_empty(),
            "one paid replacement covers the same backlog"
        );
        obs.my_queues[1].clear();
        obs.my_units.push(worker(1, TilePos::new(14, 12)));
        assert!(
            worker_quotes(&obs).is_empty(),
            "available local labor erases replacement value"
        );
        obs.my_units.clear();
        obs.my_buildings[2].tier = 1;
        assert!(
            worker_quotes(&obs).is_empty(),
            "self-timed refits do not demand construction labor"
        );
    }

    #[test]
    fn pending_prerequisite_stops_duplicate_technology_purchase() {
        let (mut obs, _, _) = fixture();
        assert_eq!(
            next_infrastructure(&obs, UnitKind::Warden),
            Some(BuildingKind::Fabricator)
        );
        let mut pending = building(2, BuildingKind::Fabricator, TilePos::new(12, 4));
        pending.built = false;
        pending.hp = 1;
        obs.my_buildings.push(pending);
        assert_eq!(next_infrastructure(&obs, UnitKind::Warden), None);
        obs.my_buildings[1].built = true;
        assert_eq!(next_infrastructure(&obs, UnitKind::Warden), None);
    }

    #[test]
    fn renewable_income_scales_past_legacy_caps_only_for_unfunded_useful_work() {
        let (mut obs, map, profile) = fixture();
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(3, 3)));
        obs.my_queues.push(Vec::new());
        for index in 0..6 {
            obs.my_buildings.push(building(
                10 + index,
                BuildingKind::Reclaimer,
                TilePos::new(12 + index as i32 * 3, 3),
            ));
            obs.my_queues.push(Vec::new());
        }
        let needs = [demand(UnitKind::Warden, 1_000)];
        let alternatives = quotes(&UtilityPolicy::new(), &obs, &map, &profile, &needs);
        assert!(
            alternatives.iter().any(|quote| matches!(
                quote.key,
                EconomicInvestmentKey::Build {
                    kind: BuildingKind::Reclaimer,
                    ..
                }
            )),
            "ordinary demand may exceed a six-Reclaimer economy: {alternatives:?}"
        );
        assert!(
            quotes(&UtilityPolicy::new(), &obs, &map, &profile, &[])
                .iter()
                .all(|quote| !matches!(
                    quote.key,
                    EconomicInvestmentKey::Build {
                        kind: BuildingKind::Reclaimer,
                        ..
                    }
                ))
        );
        obs.scrap = 1_000_000;
        assert!(
            quotes(&UtilityPolicy::new(), &obs, &map, &profile, &needs)
                .iter()
                .all(|quote| !matches!(
                    quote.key,
                    EconomicInvestmentKey::Build {
                        kind: BuildingKind::Reclaimer,
                        ..
                    }
                )),
            "already funded work is not an income shortfall"
        );
    }

    #[test]
    fn surplus_factories_do_not_value_unaffordable_throughput_or_borrow_confidence() {
        let (mut obs, map, profile) = fixture();
        for (id, anchor) in [
            (2, TilePos::new(8, 3)),
            (3, TilePos::new(14, 3)),
            (4, TilePos::new(20, 3)),
            (5, TilePos::new(26, 3)),
        ] {
            obs.my_buildings
                .push(building(id, BuildingKind::Fabricator, anchor));
            obs.my_queues.push(Vec::new());
            obs.my_queue_progress.push(0);
        }
        obs.my_buildings
            .push(building(6, BuildingKind::Crucible, TilePos::new(8, 22)));
        obs.my_queues.push(Vec::new());
        obs.my_queue_progress.push(0);
        let mut siege = demand(UnitKind::Bombard, 1_000);
        siege.case.confidence = Confidence::Prior;
        siege.case.urgency = Urgency::Developmental;
        let mut covered = demand(UnitKind::Warden, 1);
        covered.case.confidence = Confidence::Current;
        covered.case.urgency = Urgency::Pressing;
        let demands = [siege, covered];
        let factory = |quote: &&EconomicInvestment| {
            matches!(
                quote.key,
                EconomicInvestmentKey::Build {
                    kind: BuildingKind::Fabricator,
                    ..
                }
            )
        };
        obs.scrap = 282;
        assert!(
            !quotes(&UtilityPolicy::new(), &obs, &map, &profile, &demands)
                .iter()
                .any(|quote| factory(&quote))
        );
        obs.scrap = 50_000;
        let rich = quotes(&UtilityPolicy::new(), &obs, &map, &profile, &demands);
        let quote = rich
            .iter()
            .find(factory)
            .expect("funded demand can exhaust existing throughput");
        assert_eq!(quote.case.confidence, Confidence::Prior);
        assert_eq!(quote.case.urgency, Urgency::Developmental);
    }

    #[test]
    fn infrastructure_refinement_rotates_across_real_factory_sites() {
        let (mut obs, map, profile) = fixture();
        obs.scrap = 10_000;
        for (id, anchor) in [
            (2, TilePos::new(15, 4)),
            (3, TilePos::new(27, 10)),
            (4, TilePos::new(14, 23)),
        ] {
            obs.my_buildings
                .push(building(id, BuildingKind::Foundry, anchor));
            obs.my_queues.push(Vec::new());
            obs.my_queue_progress.push(0);
        }
        let policy = UtilityPolicy::new();
        let demands = [demand(UnitKind::Warden, 100)];
        let mut visited = BTreeSet::new();
        for tick in [120, 192, 264, 336] {
            obs.tick = tick;
            let candidates = quotes(&policy, &obs, &map, &profile, &demands);
            let anchors = candidates
                .iter()
                .filter_map(|quote| match quote.key {
                    EconomicInvestmentKey::Build {
                        kind: BuildingKind::Fabricator,
                        anchor,
                    } => Some(anchor),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            assert_eq!(
                anchors.len(),
                2,
                "each think must refine a bounded pair of viable sites"
            );
            visited.extend(anchors);
            assert_eq!(
                quotes(&policy, &obs, &map, &profile, &demands),
                candidates,
                "repeated evaluation must not advance the rotation within a decision"
            );
        }
        assert_eq!(
            visited.len(),
            4,
            "skipped ticks must not starve any factory location"
        );
    }

    #[test]
    fn finite_capability_demand_prices_the_next_real_prerequisite() {
        let (obs, map, profile) = fixture();
        let quotes = quotes(
            &UtilityPolicy::new(),
            &obs,
            &map,
            &profile,
            &[demand(UnitKind::Warden, 100)],
        );
        let quote = quotes
            .iter()
            .find(|quote| {
                matches!(
                    quote.key,
                    EconomicInvestmentKey::Build {
                        kind: BuildingKind::Fabricator,
                        ..
                    }
                )
            })
            .expect("useful missing ground capability must be able to fund its prerequisite");
        assert_eq!(quote.builder, Some(UnitId(1)));
        assert!(quote.ready_at > obs.tick);
    }

    #[test]
    fn saved_foundry_footprint_does_not_hide_an_alternative_technology_site() {
        let (mut obs, map, profile) = fixture();
        obs.my_units.push(worker(2, TilePos::new(3, 20)));
        let demands = [demand(UnitKind::Warden, 100)];
        let mut policy = UtilityPolicy::new();
        let fabricator = |quotes: Vec<EconomicInvestment>| {
            quotes.into_iter().find_map(|quote| match quote.key {
                EconomicInvestmentKey::Build {
                    kind: BuildingKind::Fabricator,
                    anchor,
                } => Some(anchor),
                _ => None,
            })
        };
        let saved = fabricator(quotes(&policy, &obs, &map, &profile, &demands))
            .expect("the unclaimed map offers a technology site");
        let economy = expansion_economy(&Dials::default(), &obs, obs.scrap, Reserve::Exact(0));
        policy.commit_adjudicated_foundry(
            FreshFoundryProposal::fixture(
                saved,
                UnitId(2),
                economy.foundry_cost,
                0,
                0,
                obs.tick + 1_000,
                FoundryOpportunityCase::fixture(
                    FoundryUrgency::Developmental,
                    FoundryConfidence::Supported,
                    FoundryStrategicValue::Incremental,
                    FoundryTimeToImpact::Near,
                    FoundryExecutionSafety::Secure,
                ),
            ),
            obs.tick,
            &mut Vec::new(),
        );
        let alternative = fabricator(quotes(&policy, &obs, &map, &profile, &demands))
            .expect("a saved site must not repeatedly veto every technology proposal");
        let (width, height) = BuildingKind::Fabricator.base_stats().size;
        let (saved_width, saved_height) = BuildingKind::Foundry.base_stats().size;
        assert!(
            alternative.x + width <= saved.x
                || saved.x + saved_width <= alternative.x
                || alternative.y + height <= saved.y
                || saved.y + saved_height <= alternative.y
        );
        assert_eq!(
            policy.state.foundry_saving.as_ref().unwrap().plan.anchor,
            saved
        );
    }

    #[test]
    fn refit_withholds_only_income_that_would_arrive_while_offline() {
        let (mut obs, _, _) = fixture();
        obs.my_buildings
            .push(building(2, BuildingKind::Reclaimer, TilePos::new(12, 4)));
        let resources = ResourceSnapshot::from_observation(&obs);
        let end = obs.tick + 127;
        let lost = foregone_income(&resources, BuildingId(2), obs.tick, end, 12);
        assert_eq!(
            lost.iter().map(|claim| claim.amount).sum::<u32>(),
            resources
                .forecast()
                .source_income_through(BuildingId(2), end - 1)
                .amount()
        );
        assert!(lost.iter().all(|claim| claim.through <= end + 12));
        assert!(foregone_income(&resources, BuildingId(99), obs.tick, end, 12).is_empty());
        let delayed = foregone_income(&resources, BuildingId(2), obs.tick + 60, end, 12);
        assert_eq!(
            delayed.iter().map(|claim| claim.amount).sum::<u32>(),
            resources
                .forecast()
                .source_income_through(BuildingId(2), end - 1)
                .amount()
                - resources
                    .forecast()
                    .source_income_through(BuildingId(2), obs.tick + 59)
                    .amount()
        );
    }

    #[test]
    fn retained_frame_saving_rechecks_occupation_and_exact_builder() {
        let (mut obs, map, profile) = fixture();
        let anchor = TilePos::new(12, 12);
        obs.known_frames.push(anchor);
        let initial = quotes(&UtilityPolicy::new(), &obs, &map, &profile, &[])
            .into_iter()
            .find(|quote| {
                quote.key
                    == (EconomicInvestmentKey::Build {
                        kind: BuildingKind::Extractor,
                        anchor,
                    })
            })
            .expect("a safe supported frame pays for its restoration");
        let mut policy = UtilityPolicy::new();
        policy.state.economic_saving = Some(initial);
        let retained = quotes(&policy, &obs, &map, &profile, &[]);
        assert_eq!(retained.len(), 1);
        assert_eq!(
            retained[0].deadline,
            policy.economic_saving().unwrap().deadline
        );
        let mut occupied = obs.clone();
        let mut enemy = building(99, BuildingKind::Extractor, anchor);
        enemy.player = PlayerId(1);
        enemy.seen = false;
        occupied.enemy_buildings.push(enemy.clone());
        assert!(quotes(&policy, &occupied, &map, &profile, &[]).is_empty());
        occupied.enemy_buildings.clear();
        occupied.ally_buildings.push(enemy);
        assert!(quotes(&policy, &occupied, &map, &profile, &[]).is_empty());
        obs.my_units[0].id = UnitId(2);
        assert!(
            quotes(&policy, &obs, &map, &profile, &[]).is_empty(),
            "saving must not silently transfer to a replacement builder"
        );
    }

    #[test]
    fn capability_chain_deducts_unpaid_prerequisites_and_counts_paid_delay() {
        let (mut obs, _, _) = fixture();
        let (cost, delay) = capability_chain(&obs, UnitKind::Avalanche, BuildingKind::Fabricator);
        assert!(
            cost >= u64::from(
                BuildingKind::Crucible
                    .base_stats()
                    .construction
                    .unwrap()
                    .cost
            )
        );
        assert!(
            delay
                >= u64::from(
                    BuildingKind::Crucible
                        .base_stats()
                        .construction
                        .unwrap()
                        .build_ticks
                )
        );
        let mut crucible = building(2, BuildingKind::Crucible, TilePos::new(20, 4));
        crucible.built = false;
        obs.my_buildings.push(crucible);
        let (paid_cost, paid_delay) =
            capability_chain(&obs, UnitKind::Avalanche, BuildingKind::Fabricator);
        assert!(paid_cost < cost);
        assert_eq!(paid_delay, delay);
        obs.my_buildings.last_mut().unwrap().built = true;
        assert!(capability_chain(&obs, UnitKind::Avalanche, BuildingKind::Fabricator).1 < delay);
    }

    #[test]
    fn partial_funding_never_emits_a_purchase_or_changes_its_identity() {
        let (obs, map, profile) = fixture();
        let mut policy = UtilityPolicy::new();
        let quote = quotes(
            &policy,
            &obs,
            &map,
            &profile,
            &[demand(UnitKind::Warden, 100)],
        )
        .into_iter()
        .find(|quote| {
            matches!(
                quote.key,
                EconomicInvestmentKey::Build {
                    kind: BuildingKind::Fabricator,
                    ..
                }
            )
        })
        .unwrap();
        let mut intents = Vec::new();
        policy.commit_economic_investment(quote.clone(), quote.cost - 1, &mut intents);
        assert!(intents.is_empty());
        assert_eq!(
            policy.economic_saving(),
            Some(&EconomicInvestment {
                current_capital: quote.cost - 1,
                ..quote.clone()
            })
        );
        policy.commit_economic_investment(quote.clone(), quote.cost, &mut intents);
        assert_eq!(intents, vec![quote.intent()]);
        assert!(policy.economic_saving().is_none());
    }

    #[test]
    fn economic_funding_matures_despite_repeated_unit_requests() {
        use crate::allocation::*;
        let (mut obs, map, profile) = fixture();
        obs.scrap = 40;
        for id in 2..6 {
            obs.my_buildings.push(building(
                id,
                BuildingKind::Reclaimer,
                TilePos::new(3 + (id as i32 - 2) * 5, 3),
            ));
            obs.my_queues.push(vec![]);
            obs.my_queue_progress.push(0);
        }
        let initial = ResourceSnapshot::from_observation(&obs);
        let demands = [demand(UnitKind::Warden, 100)];
        let mut policy = UtilityPolicy::new();
        let quote = quotes(&policy, &obs, &map, &profile, &demands)
            .into_iter()
            .find(|quote| {
                matches!(
                    quote.key,
                    EconomicInvestmentKey::Build {
                        kind: BuildingKind::Fabricator,
                        ..
                    }
                )
            })
            .unwrap();
        assert!(quote.fund_by < quote.deadline / 2);
        let owner = ClaimOwner::Obligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: quote.observed_at,
            key: ObligationKey::SavedEconomy(quote.key),
        };
        policy.commit_economic_investment(quote.clone(), obs.scrap, &mut vec![]);
        let mut purchased = false;
        let mut spent = 0;
        for tick in (obs.tick..=quote.fund_by + 12).step_by(12) {
            obs.tick = tick;
            obs.scrap = 40
                + initial
                    .forecast()
                    .income_through(tick.saturating_sub(1))
                    .amount()
                - spent;
            let resources = ResourceSnapshot::from_observation(&obs);
            policy.refresh_economic_saving(
                EconomicInvestmentContext {
                    evidence: Default::default(),
                    obs: &obs,
                    resources: &resources,
                    profile: &profile,
                    briefing: &map,
                    orientation: Orientation::for_home(&obs, TilePos::new(3, 12)),
                    unavailable: &[],
                    demands: &demands,
                    unit_contacts: &[],
                    building_contacts: &[],
                    cadence: 12,
                    protected_scrap: 0,
                    obligations: &[],
                    air_work: &[],
                },
                true,
            );
            let saved = policy
                .economic_saving()
                .expect("the original funding window remains viable")
                .clone();
            assert_eq!(
                (saved.fund_by, saved.deadline),
                (quote.fund_by, quote.deadline)
            );
            let mut allocation =
                CrossDomainAllocation::new(&resources, quote.deadline, 12).unwrap();
            allocation.import(ImportedObligation {
                class: ObligationClass::PersistentPlan,
                accepted_at: quote.observed_at,
                key: ObligationKey::SavedEconomy(quote.key),
                claims: economic_investment_claims(&saved).unwrap(),
            });
            let mut production = quote.clone();
            production.key = EconomicInvestmentKey::Train {
                kind: UnitKind::Sentinel,
                producer: BuildingId(1),
                service: TilePos::new(30, 12),
            };
            production.observed_at = tick;
            production.ready_at = tick + u64::from(UnitKind::Sentinel.stats().train_ticks);
            production.cost = UnitKind::Sentinel.stats().cost;
            allocation.offer(economic_investment_proposal(production).unwrap());
            let settlement = allocation
                .resolve(AllocationPersonality::default(), None)
                .unwrap();
            let current = settlement.capital_assignment(owner).unwrap().current_scrap;
            let mut intents = vec![];
            policy.commit_economic_investment(saved, current, &mut intents);
            if !intents.is_empty() {
                assert_eq!(intents, vec![quote.intent()]);
                assert!(obs.scrap >= quote.cost);
                purchased = true;
                break;
            }
            spent += settlement
                .producer_schedule()
                .iter()
                .filter(|job| job.enqueued_at == tick)
                .map(|job| job.kind.stats().cost)
                .sum::<u32>();
        }
        assert!(
            purchased,
            "ordinary production cannot continually defer the accepted purchase"
        );
    }

    #[test]
    fn extractor_cluster_prices_shared_support_before_the_first_restore() {
        let (mut obs, mut map, profile) = fixture();
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(3, 3)));
        obs.my_queues.push(vec![]);
        obs.my_queue_progress.push(0);
        let cluster = [
            TilePos::new(26, 3),
            TilePos::new(28, 3),
            TilePos::new(30, 3),
        ];
        obs.known_frames = cluster.to_vec();
        obs.known_frames.push(TilePos::new(8, 26));
        map.extractor_frames = obs.known_frames.clone();
        let policy = UtilityPolicy::new();
        let mut geometry = ConstructionChecks::new(
            crate::query_work::QueryPurpose::NavigationTest,
            &policy,
            &obs,
            &map,
            &[],
            &[],
            Orientation::for_home(&obs, TilePos::new(3, 12)),
        );
        let mut projected_worker = obs.my_units[0].clone();
        let initial_distance = geometry
            .builder_travel_cost(&projected_worker, BuildingKind::Extractor, cluster[0])
            .unwrap();
        projected_worker.tile = cluster[0].offset(-1, 0);
        let local_distance = geometry
            .builder_travel_cost(&projected_worker, BuildingKind::Extractor, cluster[0])
            .unwrap();
        assert!(
            local_distance < initial_distance,
            "later construction steps must use the builder's projected location"
        );
        drop(geometry);
        for id in 10..30 {
            let mut unit = worker(id, TilePos::new(18, 12));
            unit.kind = UnitKind::Sentinel;
            unit.hp = unit.kind.stats().max_hp;
            obs.my_units.push(unit);
        }
        let candidates = quotes(&UtilityPolicy::new(), &obs, &map, &profile, &[]);
        let first = candidates
            .iter()
            .find(|quote| quote.build().is_some())
            .unwrap();
        assert!(
            matches!(first.key, EconomicInvestmentKey::Build { kind: BuildingKind::Extractor, anchor } if cluster.contains(&anchor)),
            "{candidates:#?}"
        );
        assert!(
            first.valuation_cost
                >= 3 * first.cost
                    + BuildingKind::Foundry
                        .base_stats()
                        .construction
                        .unwrap()
                        .cost,
            "the cluster must include every restoration and its shared Foundry: {first:#?}"
        );
        assert_eq!(
            first.cost,
            BuildingKind::Extractor
                .base_stats()
                .construction
                .unwrap()
                .cost
        );
        let cluster_value = first.benefit;
        for frame in cluster.iter().skip(1) {
            for dy in 0..2 {
                for dx in 0..2 {
                    {
                        let obs = &mut *obs;
                        obs.explored[((frame.y + dy) * obs.map_width + frame.x + dx) as usize] =
                            false;
                    }
                }
            }
        }
        let hidden = quotes(&UtilityPolicy::new(), &obs, &map, &profile, &[]);
        assert!(
            hidden
                .iter()
                .all(|quote| quote.valuation_cost == quote.cost)
        );
        assert!(hidden.iter().all(|quote| quote.benefit < cluster_value));
        obs.explored.fill(true);
        for id in 40..43 {
            let next = quotes(&UtilityPolicy::new(), &obs, &map, &profile, &[])
                .into_iter()
                .find(|quote| quote.build().is_some())
                .unwrap();
            let (kind, anchor, _) = next.build().unwrap();
            assert_eq!(kind, BuildingKind::Extractor);
            assert!(
                cluster.contains(&anchor),
                "the cluster must remain useful as its earlier steps complete: {next:#?}"
            );
            obs.my_buildings.push(building(id, kind, anchor));
            obs.my_queues.push(vec![]);
            obs.my_queue_progress.push(0);
        }
        let policy = UtilityPolicy::new();
        let resources = ResourceSnapshot::from_observation(&obs);
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(profile.difficulty));
        let support = policy
            .fresh_foundry_investment(
                &dials,
                &obs,
                &resources,
                construction::FreshFoundryProposalContext {
                    home: TilePos::new(3, 12),
                    available_builders: &[UnitId(1)],
                    combat_core_exclusions: &[],
                    unit_contacts: &[],
                    building_contacts: &[],
                    public_map: &map,
                    same_think_intents: &[],
                    current_scrap: obs.scrap,
                    protected_reserve: 0,
                },
            )
            .expect("completed cluster must justify shared support");
        let construction::FreshFoundryInvestment::Ready(support) = support else {
            panic!("the cluster has enough protection");
        };
        assert!(
            cluster
                .iter()
                .all(|frame| UtilityPolicy::foundry_supports_extractor(support.anchor(), *frame))
        );
    }

    #[test]
    fn economic_purchase_prices_existing_fixed_payments_before_choosing_its_deadline() {
        use crate::allocation::*;
        let (mut obs, map, profile) = fixture();
        obs.scrap = 40;
        obs.my_buildings
            .push(building(2, BuildingKind::Reclaimer, TilePos::new(3, 3)));
        obs.my_queues.push(vec![]);
        obs.my_queue_progress.push(0);
        let resources = ResourceSnapshot::from_observation(&obs);
        let enqueue = obs.tick + 1_500;
        let ready = enqueue + u64::from(UnitKind::Sentinel.stats().train_ticks) - 1;
        let job = ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: obs.tick - 12,
            key: ObligationKey::OpeningCore { sequence: 1 },
            claims: ClaimBundle::new(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::fixed(
                    BuildingId(1),
                    UnitKind::Sentinel,
                    enqueue,
                    enqueue,
                    ready,
                    ready + 1,
                )],
            )
            .unwrap(),
        };
        let mut context = EconomicInvestmentContext {
            evidence: Default::default(),
            obs: &obs,
            resources: &resources,
            profile: &profile,
            briefing: &map,
            orientation: Orientation::for_home(&obs, TilePos::new(3, 12)),
            unavailable: &[],
            demands: &[],
            unit_contacts: &[],
            building_contacts: &[],
            cadence: 12,
            protected_scrap: 0,
            obligations: &[],
            air_work: &[],
        };
        let cost = 120;
        let deadline = obs.tick + 5_000;
        let unclaimed = FundingCalendar::new(&context).delay(cost, deadline);
        context.obligations = std::slice::from_ref(&job);
        let calendar = FundingCalendar::new(&context);
        let funded = calendar.delay(cost, deadline);
        assert!(
            funded > unclaimed,
            "the old unit payment must delay, rather than lose to, the new purchase"
        );
        for price in [0, 40, 120, 250] {
            for limit in [0, 1, 100, 1_500, 5_000] {
                let expected = (0..=limit)
                    .find(|delay| {
                        let purchase_at = obs.tick + delay;
                        let available = |at: u64| {
                            u64::from(obs.scrap)
                                + u64::from(
                                    resources
                                        .forecast()
                                        .income_through(at.saturating_sub(1))
                                        .amount(),
                                )
                        };
                        let job_cost = u64::from(UnitKind::Sentinel.stats().cost);
                        available(purchase_at)
                            >= u64::from(price) + if enqueue <= purchase_at { job_cost } else { 0 }
                            && available(enqueue)
                                >= job_cost
                                    + if purchase_at <= enqueue {
                                        u64::from(price)
                                    } else {
                                        0
                                    }
                    })
                    .unwrap_or(limit);
                assert_eq!(
                    calendar.delay(price, obs.tick + limit),
                    expected,
                    "price={price}, horizon={limit}"
                );
            }
        }
        let mut wealthy = obs.clone();
        wealthy.scrap = 1_000;
        let wealthy_resources = ResourceSnapshot::from_observation(&wealthy);
        let renewed = FundingCalendar::new(&EconomicInvestmentContext {
            obs: &wealthy,
            resources: &wealthy_resources,
            ..context
        });
        assert_eq!(renewed.delay(cost, deadline), 0);
        assert_eq!(calendar.delay(cost, deadline), funded);
        let mut allocation = CrossDomainAllocation::new(&resources, deadline, 12).unwrap();
        allocation.import(job);
        allocation.import(ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: obs.tick,
            key: ObligationKey::OpeningCore { sequence: 2 },
            claims: ClaimBundle::new(0, vec![], vec![], vec![], vec![], vec![])
                .unwrap()
                .with_deferrable_capital(DeferrableCapitalClaim {
                    through: obs.tick + funded + 12,
                    amount: cost,
                })
                .unwrap(),
        });
        let settlement = allocation
            .resolve(AllocationPersonality::default(), None)
            .unwrap();
        let retained = &settlement.producer_schedule()[0];
        assert_eq!(
            (retained.enqueued_at, retained.starts_at, retained.ready_at),
            (enqueue, enqueue, ready)
        );
    }

    fn air_quotes(
        obs: &Observation,
        map: &PublicMapBriefing,
        profile: &ResolvedProfile,
        work: &[AirCapacityDemand],
    ) -> Vec<EconomicInvestment> {
        air_quotes_with_obligations(obs, map, profile, work, &[])
    }

    fn air_quotes_with_obligations(
        obs: &Observation,
        map: &PublicMapBriefing,
        profile: &ResolvedProfile,
        work: &[AirCapacityDemand],
        obligations: &[crate::allocation::ImportedObligation],
    ) -> Vec<EconomicInvestment> {
        let resources = ResourceSnapshot::from_observation(obs);
        UtilityPolicy::new()
            .economic_quotes(EconomicInvestmentContext {
                evidence: Default::default(),
                obligations,
                obs,
                resources: &resources,
                profile,
                briefing: map,
                orientation: Orientation::for_home(obs, TilePos::new(3, 12)),
                unavailable: &[],
                demands: &[],
                unit_contacts: &[],
                building_contacts: &[],
                cadence: 12,
                protected_scrap: 0,
                air_work: work,
            })
            .investments()
            .into_iter()
            .filter(|quote| {
                matches!(
                    quote.key,
                    EconomicInvestmentKey::Build {
                        kind: BuildingKind::Airworks,
                        ..
                    }
                )
            })
            .collect()
    }

    #[test]
    fn live_worker_arrival_preserves_useful_local_harvest_capacity() {
        let (mut obs, mut map, _) = fixture();
        obs.map_width = 240;
        map.map_width = 240;
        obs.visible = vec![true; 240 * 30];
        obs.explored = obs.visible.clone();
        let source = TilePos::new(10, 12);
        obs.known_wrecks = vec![(source, 100_000)];
        obs.my_units[0].tile = TilePos::new(238, 12);
        let regions = |obs: &Observation| {
            UtilityPolicy::new().economic_harvest_regions(
                obs,
                &map,
                &ResourceSnapshot::from_observation(obs),
                Orientation::for_home(obs, TilePos::new(3, 12)),
                &[],
                (&[], &[]),
            )
        };
        let (far, work) = crate::navigation::work::measure(|| regions(&obs));
        assert_eq!(
            work.paths, 0,
            "worker valuation must not enumerate command paths"
        );
        assert_eq!(far.len(), 1);
        assert!(far[0].workers[0].ready_after > 1_000);
        let local = WorkerService {
            kind: UnitKind::Harvester,
            ready_after: 150,
        };
        assert!(
            far[0].marginal(local, 6_000) > u64::from(UnitKind::Harvester.stats().cost),
            "work={:?}, workers={:?}, marginal={}",
            far[0].work,
            far[0].workers,
            far[0].marginal(local, 6_000),
        );
        obs.my_units[0].tile = source;
        let near = regions(&obs);
        assert_eq!(near[0].workers[0].ready_after, 0);
        assert_eq!(near[0].marginal(local, 6_000), 0);
        obs.visible.fill(false);
        assert!(
            regions(&obs).is_empty(),
            "remembered amounts are not live work"
        );
    }

    #[test]
    fn simultaneous_air_operations_share_existing_factory_time() {
        let (mut obs, map, profile) = fixture();
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(3, 3)));
        obs.my_buildings
            .push(building(3, BuildingKind::Airworks, TilePos::new(15, 3)));
        obs.my_queues.resize(3, Vec::new());
        obs.my_queue_progress.resize(3, 0);
        let demand = AirCapacityDemand {
            work_ticks: 3_000,
            deadline: obs.tick + 3_000,
            kind: UnitKind::Skyhook,
            service: StandingForceServiceKey::point(TilePos::new(30, 12)),
        };
        let bomber = AirCapacityDemand {
            kind: oxide_sim::stats::Role::Bomber.unit_for(obs.faction),
            ..demand
        };
        assert!(air_quotes(&obs, &map, &profile, &[demand]).is_empty());
        assert!(air_quotes(&obs, &map, &profile, &[bomber]).is_empty());
        assert!(!air_quotes(&obs, &map, &profile, &[bomber, demand]).is_empty());
        obs.my_buildings
            .push(building(4, BuildingKind::Airworks, TilePos::new(23, 3)));
        obs.my_queues.push(Vec::new());
        obs.my_queue_progress.push(0);
        assert!(air_quotes(&obs, &map, &profile, &[bomber, demand]).is_empty());
    }

    #[test]
    fn air_capacity_prices_real_work_and_deadlines_without_a_crucible_gate() {
        let (mut obs, map, profile) = fixture();
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(3, 3)));
        obs.my_buildings
            .push(building(3, BuildingKind::Airworks, TilePos::new(15, 3)));
        obs.my_queues.resize(3, Vec::new());
        obs.my_queue_progress.resize(3, 0);
        let mut demand = AirCapacityDemand {
            work_ticks: 9_000,
            deadline: obs.tick + 3_000,
            kind: UnitKind::Skyhook,
            service: StandingForceServiceKey::point(TilePos::new(30, 12)),
        };
        assert!(air_quotes(&obs, &map, &profile, &[]).is_empty());
        assert!(
            !air_quotes(&obs, &map, &profile, &[demand]).is_empty(),
            "ordinary Fabricator prerequisites suffice when another Airworks can complete useful work"
        );
        demand.work_ticks = 3_000;
        assert!(
            air_quotes(&obs, &map, &profile, &[demand]).is_empty(),
            "a completed producer that can meet the deadline erases the capacity demand"
        );
        demand.work_ticks = 9_000;
        demand.deadline = obs.tick + 1;
        assert!(
            air_quotes(&obs, &map, &profile, &[demand]).is_empty(),
            "capacity completing after the operation deadline has no value"
        );
    }

    #[test]
    fn paid_and_uniquely_deferred_airworks_count_as_eventual_supply_once() {
        let (mut obs, map, profile) = fixture();
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(3, 3)));
        let mut pending = building(3, BuildingKind::Airworks, TilePos::new(15, 3));
        pending.built = false;
        pending.hp = 1;
        obs.my_buildings.push(pending);
        obs.my_queues.resize(3, Vec::new());
        obs.my_queue_progress.resize(3, 0);
        let work = [AirCapacityDemand {
            work_ticks: 2_000,
            deadline: obs.tick + 3_000,
            kind: UnitKind::Skyhook,
            service: StandingForceServiceKey::point(TilePos::new(30, 12)),
        }];
        assert!(air_quotes(&obs, &map, &profile, &work).is_empty());
        obs.my_buildings.pop();
        obs.my_queues.pop();
        obs.my_queue_progress.pop();
        assert!(!air_quotes(&obs, &map, &profile, &work).is_empty());
        let mut founder = worker(2, TilePos::new(14, 3));
        founder.founding = Some((BuildingKind::Airworks, TilePos::new(15, 3)));
        obs.my_units.push(founder.clone());
        founder.id = UnitId(3);
        obs.my_units.push(founder);
        assert!(air_quotes(&obs, &map, &profile, &work).is_empty());
        let resources = ResourceSnapshot::from_observation(&obs);
        assert_eq!(
            resources.producers().len(),
            2,
            "deferred eventual capacity never becomes a spendable producer lane"
        );
    }

    #[test]
    fn dispatched_foundation_keeps_its_deadline_and_releases_only_unpaid_work() {
        for (expired, paid, core_ready) in [
            (false, false, true),
            (true, false, true),
            (false, false, false),
            (true, true, false),
        ] {
            let (mut obs, map, profile) = fixture();
            let demands = [demand(UnitKind::Warden, 100)];
            let mut policy = UtilityPolicy::new();
            let quote = quotes(&policy, &obs, &map, &profile, &demands)
                .into_iter()
                .find(|quote| {
                    matches!(
                        quote.key,
                        EconomicInvestmentKey::Build {
                            kind: BuildingKind::Fabricator,
                            ..
                        }
                    )
                })
                .unwrap();
            let (kind, anchor, builder) = quote.build().unwrap();
            policy.commit_economic_investment(quote.clone(), quote.cost, &mut Vec::new());
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == builder)
                .unwrap()
                .founding = Some((kind, anchor));
            obs.tick = if expired {
                quote.deadline
            } else {
                obs.tick + 24
            };
            if paid {
                let mut foundation = building(7, kind, anchor);
                foundation.built = false;
                obs.my_buildings.push(foundation);
            }
            let resources = ResourceSnapshot::from_observation(&obs);
            policy.refresh_economic_saving(
                EconomicInvestmentContext {
                    evidence: Default::default(),
                    obligations: &[],
                    obs: &obs,
                    resources: &resources,
                    profile: &profile,
                    briefing: &map,
                    orientation: Orientation::for_home(&obs, TilePos::new(3, 12)),
                    unavailable: &[],
                    demands: &demands,
                    unit_contacts: &[],
                    building_contacts: &[],
                    cadence: 12,
                    protected_scrap: 0,
                    air_work: &[],
                },
                core_ready,
            );
            let retained = !expired && !paid && core_ready;
            assert_eq!(policy.has_economic_foundation(), retained);
            assert_eq!(
                policy.state.economic_cancelled_founder,
                (!paid && !retained).then_some((builder, kind, anchor))
            );
            if retained {
                assert_eq!(
                    policy.state.economic_foundation.as_ref().unwrap().deadline,
                    quote.deadline
                );
                let mut intents = Vec::new();
                obs.scrap = quote.cost;
                assert!(
                    policy
                        .post_floor_deferred_claims(
                            &obs,
                            obs.scrap,
                            DeferredClaimContext {
                                home: TilePos::new(3, 12),
                                unit_contacts: Some(&[]),
                                building_contacts: Some(&[]),
                                public_map: Some(&map)
                            },
                            &mut intents,
                        )
                        .contains(&(kind, anchor))
                );
                assert!(
                    intents.is_empty(),
                    "the shallow guard cannot revoke accepted capital"
                );
                assert!(quotes(&policy, &obs, &map, &profile, &demands).is_empty());
            }
        }
    }

    #[test]
    fn core_loss_and_deadline_expiry_release_only_the_unpaid_economic_plan() {
        for (expired, missed_funding) in [(false, false), (true, false), (false, true)] {
            let (mut obs, map, profile) = fixture();
            let demands = [demand(UnitKind::Warden, 100)];
            let mut policy = UtilityPolicy::new();
            let quote = quotes(&policy, &obs, &map, &profile, &demands)
                .into_iter()
                .find(|quote| {
                    matches!(
                        quote.key,
                        EconomicInvestmentKey::Build {
                            kind: BuildingKind::Fabricator,
                            ..
                        }
                    )
                })
                .unwrap();
            policy.commit_economic_investment(quote.clone(), 0, &mut Vec::new());
            if expired {
                obs.tick = quote.deadline;
            } else if missed_funding {
                obs.tick = quote.fund_by + 1;
            }
            let paid = building(7, BuildingKind::Reclaimer, TilePos::new(21, 3));
            obs.my_buildings.push(paid.clone());
            let resources = ResourceSnapshot::from_observation(&obs);
            policy.refresh_economic_saving(
                EconomicInvestmentContext {
                    evidence: Default::default(),
                    obligations: &[],
                    obs: &obs,
                    resources: &resources,
                    profile: &profile,
                    briefing: &map,
                    orientation: Orientation::for_home(&obs, TilePos::new(3, 12)),
                    unavailable: &[],
                    demands: &demands,
                    unit_contacts: &[],
                    building_contacts: &[],
                    cadence: 12,
                    protected_scrap: 0,
                    air_work: &[],
                },
                expired || missed_funding,
            );
            assert!(policy.economic_saving().is_none());
            assert!(policy.state.economic_retry_at > obs.tick);
            assert!(obs.my_buildings.contains(&paid));
        }
    }

    #[test]
    fn bootstrap_airworks_bounds_target_derivation_when_every_campaign_is_unfunded() {
        let (mut obs, map, profile) = fixture();
        obs.scrap = BuildingKind::Airworks
            .base_stats()
            .construction
            .unwrap()
            .cost;
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(8, 5)));
        obs.my_queues.push(Vec::new());
        obs.my_queue_progress.push(0);
        for (index, anchor) in [
            TilePos::new(20, 3),
            TilePos::new(30, 3),
            TilePos::new(20, 13),
            TilePos::new(30, 13),
            TilePos::new(20, 23),
            TilePos::new(30, 23),
        ]
        .into_iter()
        .enumerate()
        {
            let mut target = building(90 + index as u32, BuildingKind::Foundry, anchor);
            target.player = PlayerId(1);
            obs.enemy_buildings.push(target);
        }
        let before = crate::strategy::airworks_package_derivations();
        assert!(air_quotes(&obs, &map, &profile, &[]).is_empty());
        assert_eq!(crate::strategy::airworks_package_derivations() - before, 2);
    }

    #[test]
    fn bootstrap_airworks_respects_retained_cash_forecast_and_factory_work() {
        use crate::allocation::{
            ClaimBundle, ForecastClaim, ImportedObligation, ObligationClass, ObligationKey,
            ProducerJobClaim,
        };
        let (mut obs, map, profile) = fixture();
        obs.scrap = 300;
        obs.my_buildings.extend([
            building(2, BuildingKind::Fabricator, TilePos::new(8, 5)),
            building(3, BuildingKind::Crucible, TilePos::new(12, 5)),
            building(4, BuildingKind::Reclaimer, TilePos::new(2, 23)),
        ]);
        {
            let obs = &mut *obs;
            obs.my_queues.resize(obs.my_buildings.len(), vec![]);
        }
        {
            let obs = &mut *obs;
            obs.my_queue_progress.resize(obs.my_buildings.len(), 0);
        }
        let mut target = building(90, BuildingKind::Foundry, TilePos::new(30, 12));
        target.player = PlayerId(1);
        obs.enemy_buildings.push(target);
        let quotes = air_quotes(&obs, &map, &profile, &[]);
        let quote = quotes
            .first()
            .expect("unclaimed completed income funds the minimum");
        let resources = ResourceSnapshot::from_observation(&obs);
        let forecast = resources.forecast().income_through(quote.deadline).amount();
        let future = ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: obs.tick,
            key: ObligationKey::OpeningCore { sequence: 0 },
            claims: ClaimBundle::new(
                0,
                vec![ForecastClaim {
                    through: quote.deadline,
                    amount: forecast,
                }],
                vec![],
                vec![],
                vec![],
                vec![],
            )
            .unwrap(),
        };
        assert!(air_quotes_with_obligations(&obs, &map, &profile, &[], &[future]).is_empty());
        let kind = UnitKind::Warden;
        let enqueue = obs.tick + 12;
        let job = ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: obs.tick,
            key: ObligationKey::OpeningCore { sequence: 1 },
            claims: ClaimBundle::new(
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![ProducerJobClaim::fixed(
                    BuildingId(2),
                    kind,
                    enqueue,
                    enqueue,
                    enqueue + u64::from(kind.stats().train_ticks) - 1,
                    quote.deadline,
                )],
            )
            .unwrap(),
        };
        let mut retained_only =
            crate::allocation::CrossDomainAllocation::new(&resources, quote.deadline, 12).unwrap();
        retained_only.import(job.clone());
        assert!(
            retained_only
                .resolve(crate::allocation::AllocationPersonality::default(), None)
                .is_ok()
        );
        let derivations = crate::strategy::airworks_package_derivations();
        assert!(
            air_quotes_with_obligations(&obs, &map, &profile, &[], std::slice::from_ref(&job))
                .is_empty()
        );
        assert_eq!(
            crate::strategy::airworks_package_derivations(),
            derivations,
            "an Airworks that would break a fixed payment must be rejected before campaign derivation"
        );
        obs.scrap = 1200;
        assert!(
            !air_quotes_with_obligations(&obs, &map, &profile, &[], &[job]).is_empty(),
            "compatible retained work must not suppress a funded campaign"
        );
        assert!(crate::strategy::airworks_package_derivations() > derivations);
    }
}
