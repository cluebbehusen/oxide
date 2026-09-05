//! Exact economic actions derived from finite work and unmet capability demand.

use super::defense::DefenseThinkContext;
use super::economic_value::{
    CapacityReturn, RecurringReturn, WorkerService, investment_horizon, travel_ticks,
};
use super::*;
use crate::bot::allocation::{
    Confidence, ExecutionSafety, ProposalCase, StrategicValue, TimeToImpact, Urgency,
};
use crate::bot::orient::Orientation;
use crate::bot::resources::ProducerEgress;
use crate::bot::standing_force::{CapabilityDemand, ServiceRouting};
use serde::Serialize;

/// Canonical identity of an economic action, also exposed in decision traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct EconomicInvestment {
    pub(in crate::bot) key: EconomicInvestmentKey,
    pub(in crate::bot) builder: Option<UnitId>,
    pub(in crate::bot) cost: u32,
    pub(in crate::bot) current_capital: u32,
    pub(in crate::bot) observed_at: u64,
    pub(in crate::bot) ready_at: u64,
    pub(in crate::bot) deadline: u64,
    pub(in crate::bot) case: ProposalCase,
    pub(in crate::bot) benefit: u64,
    pub(in crate::bot) personality: u8,
    pub(in crate::bot) foregone_income: Vec<crate::bot::allocation::ForecastClaim>,
}

impl EconomicInvestment {
    pub(in crate::bot) fn build(&self) -> Option<(BuildingKind, TilePos, UnitId)> {
        match self.key {
            EconomicInvestmentKey::Build { kind, anchor } => Some((kind, anchor, self.builder?)),
            _ => None,
        }
    }

    pub(in crate::bot) fn intent(&self) -> Intent {
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
pub(in crate::bot) struct EconomicInvestmentContext<'a> {
    pub(in crate::bot) obs: &'a Observation,
    pub(in crate::bot) resources: &'a ResourceSnapshot,
    pub(in crate::bot) profile: &'a ResolvedProfile,
    pub(in crate::bot) briefing: &'a PublicMapBriefing,
    pub(in crate::bot) orientation: Orientation,
    pub(in crate::bot) unavailable: &'a [UnitId],
    pub(in crate::bot) demands: &'a [CapabilityDemand],
    pub(in crate::bot) unit_contacts: &'a [UnitContact],
    pub(in crate::bot) building_contacts: &'a [BuildingContact],
    pub(in crate::bot) cadence: u64,
    pub(in crate::bot) protected_scrap: u32,
    pub(in crate::bot) air_work: &'a [AirCapacityDemand],
}

#[derive(Debug, Clone, Copy)]
pub(in crate::bot) struct AirCapacityDemand {
    pub(in crate::bot) work_ticks: u64,
    pub(in crate::bot) deadline: u64,
    pub(in crate::bot) kind: UnitKind,
    pub(in crate::bot) service: crate::bot::allocation::StandingForceServiceKey,
}

impl UtilityPolicy {
    pub(in crate::bot) fn fresh_capacity_foundry_investment(
        &self,
        dials: &Dials,
        context: EconomicInvestmentContext<'_>,
        foundry_context: construction::FreshFoundryProposalContext<'_>,
    ) -> Option<construction::FreshFoundryInvestment> {
        let obs = context.obs;
        if !dials.expansion
            || self.foundry_saving.is_some()
            || foundry_context.available_builders.is_empty()
            || !obs
                .my_buildings
                .iter()
                .any(|building| building.kind == BuildingKind::Fabricator && building.built)
            || !context.demands.iter().any(|demand| {
                BuildingKind::Foundry
                    .base_stats()
                    .produces
                    .contains(&demand.kind)
                    && demand
                        .units_needed()
                        .saturating_mul(u64::from(demand.kind.stats().cost))
                        >= u64::from(
                            BuildingKind::Foundry
                                .base_stats()
                                .construction
                                .unwrap()
                                .cost,
                        )
            })
            || Self::projected_foundries(obs).1 != 0
        {
            return None;
        }
        let economy = expansion_economy(
            dials,
            obs,
            foundry_context.current_scrap,
            Reserve::Exact(foundry_context.protected_reserve),
        );
        let horizon = economy.horizon_ticks();
        let delay = economy.build_ticks.saturating_add(funding_delay(
            &context,
            economy.foundry_cost,
            obs.tick.saturating_add(horizon),
        ));
        let mut infrastructure = InfrastructureContext {
            obs,
            resources: context.resources,
            demands: context.demands,
            routes: ServiceRouting::new(obs, Some(context.briefing), Some(context.orientation)),
            briefing: context.briefing,
            orientation: context.orientation,
            air_work: &[],
        };
        let opportunities = obs
            .my_buildings
            .iter()
            .filter(|building| building.built && building.kind == BuildingKind::Foundry)
            .filter_map(|home| self.placement_near(obs, BuildingKind::Foundry, home.anchor))
            .filter_map(|anchor| {
                let value = infrastructure_benefit(
                    &mut infrastructure,
                    BuildingKind::Foundry,
                    anchor,
                    horizon,
                    delay,
                );
                (value >= u64::from(economy.foundry_cost))
                    .then(|| expansion::FoundryOpportunity::capacity_only(anchor, value, economy))
            })
            .collect::<Vec<_>>();
        if opportunities.is_empty() {
            return None;
        }
        self.fresh_foundry_with_opportunities(
            dials,
            obs,
            context.resources,
            foundry_context,
            Some(expansion::rank_foundry_opportunities(opportunities)),
        )
    }

    pub(in crate::bot) fn economic_saving(&self) -> Option<&EconomicInvestment> {
        self.economic_saving.as_ref()
    }

    pub(in crate::bot) fn has_economic_foundation(&self) -> bool {
        self.economic_foundation.is_some()
    }

    pub(in crate::bot) fn economic_foundation(&self) -> Option<&EconomicInvestment> {
        self.economic_foundation.as_ref()
    }

    pub(in crate::bot) fn bind_economic_saving_current(&mut self, available: u32) {
        if let Some(saving) = self.economic_saving.as_mut() {
            saving.current_capital = available.min(saving.cost);
        }
    }

    pub(in crate::bot) fn commit_economic_investment(
        &mut self,
        proposal: EconomicInvestment,
        current_funding: u32,
        intents: &mut Vec<Intent>,
    ) {
        if matches!(proposal.key, EconomicInvestmentKey::Train { .. }) {
            return;
        }
        if current_funding >= proposal.cost {
            intents.push(proposal.intent());
            if proposal.build().is_some() {
                self.economic_foundation = Some(proposal);
            }
            self.economic_saving = None;
        } else {
            self.economic_saving = Some(proposal);
        }
    }

    pub(in crate::bot) fn refresh_economic_saving(
        &mut self,
        context: EconomicInvestmentContext<'_>,
        core_ready: bool,
    ) {
        if let Some(plan) = self.economic_foundation.take()
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
                    )
                    && self.deferred_claim_has_safe_founder(
                        context.obs,
                        (kind, anchor),
                        Some(context.unit_contacts),
                        Some(context.building_contacts),
                        context.briefing,
                    );
                if valid {
                    self.economic_foundation = Some(plan);
                } else {
                    self.economic_cancelled_founder = Some((builder, kind, anchor));
                    self.economic_retry_at = context.obs.tick.saturating_add(600);
                }
            }
        }
        let Some(saving) = self.economic_saving.as_ref() else {
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
            self.economic_saving = None;
            return;
        }
        let now = context.obs.tick;
        if !core_ready || now >= saving.deadline {
            self.economic_saving = None;
            self.economic_retry_at = now.saturating_add(600);
            return;
        }
        let original = saving.clone();
        let mut refreshed = self.fresh_economic_investments(context);
        if let Some(mut proposal) = refreshed.pop() {
            proposal.observed_at = original.observed_at;
            self.economic_saving = Some(proposal);
        } else {
            self.economic_saving = None;
            self.economic_retry_at = now.saturating_add(600);
        }
    }

    pub(in crate::bot) fn fresh_economic_investments(
        &self,
        context: EconomicInvestmentContext<'_>,
    ) -> Vec<EconomicInvestment> {
        let obs = context.obs;
        if obs.tick < self.economic_retry_at || self.economic_foundation.is_some() {
            return Vec::new();
        }
        let retained = self.economic_saving.as_ref();
        let horizon = retained.map_or_else(
            || {
                investment_horizon(
                    context.profile.traits.greed,
                    context
                        .resources
                        .current_scrap()
                        .amount()
                        .saturating_sub(context.protected_scrap),
                )
            },
            |saving| saving.deadline.saturating_sub(obs.tick),
        );
        let deadline = obs.tick.saturating_add(horizon);
        let needs_harvest_quote = retained.is_none_or(|saving| match saving.key {
            EconomicInvestmentKey::Build { kind, .. } => kind == BuildingKind::Reclaimer,
            EconomicInvestmentKey::Upgrade { building, .. } => obs
                .my_buildings
                .iter()
                .any(|owned| owned.id == building && owned.kind == BuildingKind::Reclaimer),
            EconomicInvestmentKey::Train { .. } => true,
        });
        let regions = if needs_harvest_quote {
            self.economic_harvest_regions(
                obs,
                context.briefing,
                context.resources,
                context.orientation,
                context.unavailable,
                (context.unit_contacts, context.building_contacts),
            )
        } else {
            Vec::new()
        };
        let mut proposals = Vec::new();
        for region in &regions {
            if retained.is_some() {
                break;
            }
            for lane in context.resources.producers() {
                let Some(distance) = region.producer_distance(lane.producer) else {
                    continue;
                };
                for kind in [UnitKind::Harvester, UnitKind::Excavator] {
                    let Some(timing) = lane.production_timing(&[kind]) else {
                        continue;
                    };
                    if !matches!(
                        timing.current_egress,
                        ProducerEgress::Open | ProducerEgress::NotRequired
                    ) || obs.scrap < kind.stats().cost
                    {
                        continue;
                    }
                    let ready_after = timing
                        .no_block_latest_ready_tick
                        .saturating_sub(obs.tick)
                        .saturating_add(travel_ticks(kind, distance));
                    let benefit = region.marginal(WorkerService { kind, ready_after }, horizon);
                    if benefit < u64::from(kind.stats().cost) {
                        continue;
                    }
                    proposals.push(EconomicInvestment {
                        key: EconomicInvestmentKey::Train {
                            kind,
                            producer: lane.producer,
                            service: region.service,
                        },
                        builder: None,
                        cost: kind.stats().cost,
                        current_capital: kind.stats().cost,
                        observed_at: obs.tick,
                        ready_at: timing.no_block_latest_ready_tick,
                        deadline,
                        case: ProposalCase {
                            urgency: Urgency::Developmental,
                            ..economic_case(benefit, kind.stats().cost, ready_after)
                        },
                        benefit,
                        personality: context.profile.traits.greed,
                        foregone_income: Vec::new(),
                    });
                }
            }
        }
        if retained.is_none() {
            for work in self.orphan_construction_work(
                obs,
                context.briefing,
                context.resources,
                context.orientation,
                context.unavailable,
                (context.unit_contacts, context.building_contacts),
            ) {
                for lane in context.resources.producers() {
                    for kind in [UnitKind::Harvester, UnitKind::Excavator] {
                        let Some(timing) = lane.production_timing(&[kind]) else {
                            continue;
                        };
                        if obs.scrap < kind.stats().cost
                            || !matches!(
                                timing.current_egress,
                                ProducerEgress::Open | ProducerEgress::NotRequired,
                            )
                        {
                            continue;
                        }
                        let ready_after = timing
                            .no_block_latest_ready_tick
                            .saturating_add(1)
                            .saturating_sub(obs.tick);
                        let benefit = work.marginal(
                            lane.producer,
                            WorkerService { kind, ready_after },
                            horizon,
                        );
                        if benefit < u64::from(kind.stats().cost) {
                            continue;
                        }
                        proposals.push(EconomicInvestment {
                            key: EconomicInvestmentKey::Train {
                                kind,
                                producer: lane.producer,
                                service: work.service,
                            },
                            builder: None,
                            cost: kind.stats().cost,
                            current_capital: kind.stats().cost,
                            observed_at: obs.tick,
                            ready_at: timing.no_block_latest_ready_tick,
                            deadline,
                            case: economic_case(benefit, kind.stats().cost, ready_after),
                            benefit,
                            personality: context.profile.traits.greed,
                            foregone_income: Vec::new(),
                        });
                    }
                }
            }
        }
        let demand_scrap = useful_demand_scrap(context.demands);
        let harvesting = regions
            .iter()
            .map(|region| region.current_output(horizon))
            .fold(0, u64::saturating_add);
        let eventual_income = projected_recurring_output(obs, context.resources, horizon);
        let unmet_income = demand_scrap
            .saturating_sub(u64::from(obs.scrap.saturating_sub(context.protected_scrap)))
            .saturating_sub(harvesting)
            .saturating_sub(eventual_income);
        let income_evidence =
            unfunded_income_evidence(context.demands, demand_scrap.saturating_sub(unmet_income));
        let builders = self
            .construction_builders(obs, &[], context.unavailable)
            .into_iter()
            .filter(|unit| {
                builder_is_free(obs, unit) && !self.evacuating_workers.contains(&unit.id)
            })
            .filter(|unit| retained.is_none_or(|saving| saving.builder == Some(unit.id)))
            .collect::<Vec<_>>();
        let mut geometry = None;
        let mut infrastructure = InfrastructureContext {
            obs,
            resources: context.resources,
            demands: context.demands,
            routes: ServiceRouting::new(obs, Some(context.briefing), Some(context.orientation)),
            briefing: context.briefing,
            orientation: context.orientation,
            air_work: context.air_work,
        };
        let have_built = |kind| {
            obs.my_buildings
                .iter()
                .any(|building| building.kind == kind && building.built)
        };
        let mut possible = Vec::new();
        if retained.is_none() {
            for &frame in &obs.known_frames {
                if self.player_can_plan_frame_restoration(obs, frame)
                    && !Self::deferred_claims(obs).contains(&(BuildingKind::Extractor, frame))
                    && !obs
                        .my_buildings
                        .iter()
                        .chain(&obs.enemy_buildings)
                        .any(|building| building.anchor == frame)
                {
                    possible.push((BuildingKind::Extractor, frame));
                }
            }
            for home in obs
                .my_buildings
                .iter()
                .filter(|building| building.built && building.kind == BuildingKind::Foundry)
            {
                for kind in [
                    BuildingKind::Reclaimer,
                    BuildingKind::Fabricator,
                    BuildingKind::Airworks,
                    BuildingKind::Crucible,
                ] {
                    if kind == BuildingKind::Reclaimer && unmet_income == 0 {
                        continue;
                    }
                    if kind != BuildingKind::Reclaimer
                        && !(kind == BuildingKind::Airworks && !context.air_work.is_empty())
                        && !context
                            .demands
                            .iter()
                            .any(|demand| next_infrastructure(obs, demand.kind) == Some(kind))
                        && !context
                            .demands
                            .iter()
                            .any(|demand| kind.base_stats().produces.contains(&demand.kind))
                    {
                        continue;
                    }
                    if let Some(anchor) =
                        self.placement_near_where(obs, kind, home.anchor, |anchor| {
                            self.foundry_saving.as_ref().is_none_or(|saving| {
                                let saved = saving.plan.anchor;
                                let (width, height) = kind.base_stats().size;
                                let (saved_width, saved_height) =
                                    BuildingKind::Foundry.base_stats().size;
                                anchor.x + width <= saved.x
                                    || saved.x + saved_width <= anchor.x
                                    || anchor.y + height <= saved.y
                                    || saved.y + saved_height <= anchor.y
                            })
                        })
                    {
                        possible.push((kind, anchor));
                    }
                }
            }
            possible.sort_unstable_by_key(|(kind, anchor)| (*kind, anchor.y, anchor.x));
            possible.dedup();
        }
        if let Some(saving) = retained {
            possible = saving
                .build()
                .map(|(kind, anchor, _)| (kind, anchor))
                .into_iter()
                .collect();
        }
        let projected_bank = obs
            .scrap
            .saturating_sub(context.protected_scrap)
            .saturating_add(
                context
                    .resources
                    .forecast()
                    .income_through(deadline.saturating_sub(1))
                    .amount(),
            );
        for (kind, anchor) in possible {
            if !self.placement_geometry_valid(obs, kind, anchor) {
                continue;
            }
            let Some(stats) = kind.base_stats().construction else {
                continue;
            };
            if stats.requires.iter().any(|kind| !have_built(*kind))
                || projected_bank < stats.cost
                || builders.is_empty()
            {
                continue;
            }
            let geometry = geometry.get_or_insert_with(|| {
                DefenseThinkContext::new_oriented(
                    self,
                    obs,
                    context.briefing,
                    context.unit_contacts,
                    context.building_contacts,
                    context.orientation,
                )
            });
            if !geometry.resource_access_survives(kind, anchor)
                || !geometry.future_ground_producer_egress_survives(kind, anchor)
            {
                continue;
            }
            let Some(builder) = geometry.safe_implicit_builder(self, kind, anchor, &builders)
            else {
                continue;
            };
            let worker = builders
                .iter()
                .find(|worker| worker.id == builder)
                .expect("the quote binds an eligible worker");
            let Some(distance) = geometry.builder_travel_cost(worker, kind, anchor) else {
                continue;
            };
            let funding_delay = funding_delay(&context, stats.cost, deadline);
            let delay = funding_delay
                .saturating_add(travel_ticks(worker.kind, distance))
                .saturating_add(
                    u64::from(stats.build_ticks)
                        .div_ceil(u64::from(worker.kind.stats().build_rate.max(1))),
                );
            let benefit = match kind {
                BuildingKind::Extractor => {
                    let rate = if Self::frame_has_foundry_support(obs, anchor) {
                        crate::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
                    } else {
                        crate::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE
                    };
                    horizon
                        .saturating_sub(delay)
                        .saturating_mul(u64::from(rate))
                        / (u64::from(crate::TICKS_PER_SECOND) * 60)
                }
                BuildingKind::Reclaimer => RecurringReturn {
                    horizon,
                    ready_after: delay,
                    old_period: None,
                    new_period: crate::stats::RECLAIMER_PERIOD,
                    unmet_demand: unmet_income,
                }
                .marginal(),
                _ => infrastructure_benefit(&mut infrastructure, kind, anchor, horizon, delay),
            };
            if benefit < u64::from(stats.cost) {
                continue;
            }
            let mut case = infrastructure_case(&context, kind, benefit, stats.cost, delay);
            if kind == BuildingKind::Reclaimer
                && let Some(evidence) = income_evidence
            {
                case.confidence = evidence.confidence;
                case.urgency = match evidence.urgency {
                    Urgency::Pressing => Urgency::Timely,
                    urgency => urgency,
                };
            }
            proposals.push(EconomicInvestment {
                key: EconomicInvestmentKey::Build { kind, anchor },
                builder: Some(builder),
                cost: stats.cost,
                current_capital: obs
                    .scrap
                    .saturating_sub(context.protected_scrap)
                    .min(stats.cost),
                observed_at: obs.tick,
                ready_at: obs.tick.saturating_add(delay),
                deadline,
                case,
                benefit,
                personality: context.profile.traits.greed,
                foregone_income: Vec::new(),
            });
        }
        for building in &obs.my_buildings {
            if !building.built {
                continue;
            }
            if retained.is_some_and(|saving| {
                saving.key
                    != (EconomicInvestmentKey::Upgrade {
                        building: building.id,
                        tier: building.tier + 1,
                    })
            }) {
                continue;
            }
            let Some(upgrade) = building.kind.upgrade_from(building.tier) else {
                continue;
            };
            if projected_bank < upgrade.cost
                || upgrade.requires.iter().any(|kind| !have_built(*kind))
                || obs
                    .my_units
                    .iter()
                    .any(|unit| unit.salvaging == Some(building.id))
            {
                continue;
            }
            let funding_delay = funding_delay(&context, upgrade.cost, deadline);
            let refit_delay = funding_delay.saturating_add(u64::from(upgrade.build_ticks));
            let benefit = if building.kind == BuildingKind::Reclaimer {
                RecurringReturn {
                    horizon: horizon.saturating_sub(funding_delay),
                    ready_after: u64::from(upgrade.build_ticks),
                    old_period: Some(crate::stats::RECLAIMER_PERIOD),
                    new_period: crate::stats::REFINERY_PERIOD,
                    unmet_demand: unmet_income,
                }
                .marginal()
            } else {
                geometry
                    .get_or_insert_with(|| {
                        DefenseThinkContext::new_oriented(
                            self,
                            obs,
                            context.briefing,
                            context.unit_contacts,
                            context.building_contacts,
                            context.orientation,
                        )
                    })
                    .upgrade_value(building, horizon.saturating_sub(funding_delay))
            };
            if benefit < u64::from(upgrade.cost) {
                continue;
            }
            let mut case =
                infrastructure_case(&context, building.kind, benefit, upgrade.cost, refit_delay);
            if building.kind == BuildingKind::Reclaimer
                && let Some(evidence) = income_evidence
            {
                case.confidence = evidence.confidence;
                case.urgency = match evidence.urgency {
                    Urgency::Pressing => Urgency::Timely,
                    urgency => urgency,
                };
            }
            if building.kind != BuildingKind::Reclaimer {
                use super::defense::DefenseOpportunityEvidence;
                let evidence = geometry
                    .as_mut()
                    .expect("defense valuation prepares geometry")
                    .upgrade_evidence(building.kind);
                case.confidence = match evidence {
                    DefenseOpportunityEvidence::CurrentArmed
                    | DefenseOpportunityEvidence::CurrentFoothold => Confidence::Current,
                    DefenseOpportunityEvidence::Remembered => Confidence::Supported,
                    DefenseOpportunityEvidence::PublicPrior => Confidence::Prior,
                };
                if evidence == DefenseOpportunityEvidence::PublicPrior {
                    case.urgency = Urgency::Developmental;
                }
            }
            proposals.push(EconomicInvestment {
                key: EconomicInvestmentKey::Upgrade {
                    building: building.id,
                    tier: building.tier + 1,
                },
                builder: None,
                cost: upgrade.cost,
                observed_at: obs.tick,
                current_capital: obs
                    .scrap
                    .saturating_sub(context.protected_scrap)
                    .min(upgrade.cost),
                ready_at: obs.tick.saturating_add(refit_delay),
                deadline,
                case,
                benefit,
                personality: context.profile.traits.greed,
                foregone_income: foregone_income(
                    context.resources,
                    building.id,
                    obs.tick.saturating_add(funding_delay),
                    obs.tick.saturating_add(refit_delay),
                    context.cadence,
                ),
            });
        }
        proposals.sort_unstable_by_key(|proposal| {
            (
                std::cmp::Reverse(
                    proposal.benefit.saturating_mul(1_000) / u64::from(proposal.cost.max(1)),
                ),
                proposal.key,
            )
        });
        let mut seen = BTreeSet::new();
        proposals.retain(|proposal| seen.insert(proposal.key));
        proposals
    }
}

fn funding_delay(context: &EconomicInvestmentContext<'_>, cost: u32, deadline: u64) -> u64 {
    let missing = cost.saturating_sub(context.obs.scrap.saturating_sub(context.protected_scrap));
    if missing == 0 {
        return 0;
    }
    let mut low = context.obs.tick;
    let mut high = deadline;
    while low < high {
        let mid = low + (high - low) / 2;
        if context
            .resources
            .forecast()
            .income_through(mid.saturating_sub(1))
            .amount()
            >= missing
        {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    low.saturating_sub(context.obs.tick)
}

fn foregone_income(
    resources: &ResourceSnapshot,
    source: BuildingId,
    starts_at: u64,
    deadline: u64,
    cadence: u64,
) -> Vec<crate::bot::allocation::ForecastClaim> {
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
            claims.push(crate::bot::allocation::ForecastClaim {
                through: next,
                amount,
            });
        }
        prior = total;
        decision = next;
    }
    claims
}

fn economic_case(benefit: u64, cost: u32, delay: u64) -> ProposalCase {
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
            (BuildingKind::Reclaimer, 0) => remaining / crate::stats::RECLAIMER_PERIOD,
            (BuildingKind::Reclaimer, _) => remaining / crate::stats::REFINERY_PERIOD,
            (BuildingKind::Extractor, _) => {
                let rate = if UtilityPolicy::frame_has_foundry_support(obs, anchor) {
                    crate::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
                } else {
                    crate::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE
                };
                remaining.saturating_mul(u64::from(rate))
                    / (u64::from(crate::TICKS_PER_SECOND) * 60)
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

struct InfrastructureContext<'a> {
    obs: &'a Observation,
    resources: &'a ResourceSnapshot,
    demands: &'a [CapabilityDemand],
    routes: ServiceRouting<'a>,
    briefing: &'a PublicMapBriefing,
    orientation: Orientation,
    air_work: &'a [AirCapacityDemand],
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
    context: &mut InfrastructureContext<'_>,
    kind: BuildingKind,
    anchor: TilePos,
    horizon: u64,
    delay: u64,
) -> u64 {
    let obs = context.obs;
    let candidate = BuildingObs {
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
                Some(crate::bot::standing_force::air_production_spawn_tile(
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
                return Some(
                    units
                        .min(demand.units_needed())
                        .saturating_mul(u64::from(demand.kind.stats().cost))
                        .saturating_sub(chain_cost),
                );
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
                        Some(crate::bot::standing_force::air_production_spawn_tile(
                            &pending,
                            Some(context.orientation),
                        ))
                    } else {
                        routing::production_spawn_doorstep(
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
            Some(
                CapacityReturn {
                    horizon,
                    ready_after: delay,
                    train_ticks: u64::from(demand.kind.stats().train_ticks),
                    demanded_units: demand.units_needed(),
                    existing_units: existing.saturating_add(pending),
                }
                .additional_units()
                .saturating_mul(u64::from(demand.kind.stats().cost)),
            )
        })
        .max()
        .unwrap_or(0);
    if kind != BuildingKind::Airworks {
        return ordinary;
    }
    let origin = crate::bot::standing_force::air_production_spawn_tile(
        &candidate,
        Some(context.orientation),
    );
    let operational = context
        .air_work
        .iter()
        .filter_map(|demand| {
            if !context
                .routes
                .origin_serves(origin, demand.kind, demand.service)
            {
                return None;
            }
            let remaining = demand.deadline.saturating_sub(obs.tick);
            let existing = obs
                .my_buildings
                .iter()
                .filter(|building| building.kind == kind)
                .filter(|building| {
                    context.routes.origin_serves(
                        crate::bot::standing_force::air_production_spawn_tile(
                            building,
                            Some(context.orientation),
                        ),
                        demand.kind,
                        demand.service,
                    )
                })
                .map(|building| {
                    if building.built {
                        remaining
                    } else {
                        remaining.saturating_sub(u64::from(
                            kind.base_stats().construction.unwrap().build_ticks,
                        ))
                    }
                })
                .fold(0, u64::saturating_add);
            let deferred = UtilityPolicy::deferred_claims(obs)
                .into_iter()
                .filter(|(pending, anchor)| {
                    *pending == kind
                        && !obs
                            .my_buildings
                            .iter()
                            .any(|building| building.kind == *pending && building.anchor == *anchor)
                })
                .filter(|(_, anchor)| {
                    context
                        .routes
                        .origin_serves(*anchor, demand.kind, demand.service)
                })
                .map(|_| {
                    remaining.saturating_sub(u64::from(
                        kind.base_stats().construction.unwrap().build_ticks,
                    ))
                })
                .fold(0, u64::saturating_add);
            let existing = existing.saturating_add(deferred);
            let available_work = remaining
                .saturating_sub(delay)
                .min(demand.work_ticks.saturating_sub(existing));
            Some(
                available_work / u64::from(demand.kind.stats().train_ticks.max(1))
                    * u64::from(demand.kind.stats().cost),
            )
        })
        .max()
        .unwrap_or(0);
    ordinary.max(operational)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::allocation::StandingForceServiceKey;
    use crate::bot::standing_force::StandingForceReason;
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};

    fn building(id: u32, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(0),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn worker(id: u32, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile,
            hp: UnitKind::Harvester.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn fixture() -> (Observation, PublicMapBriefing, ResolvedProfile) {
        let obs = Observation {
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
            ..Observation::default()
        };
        let map = PublicMapBriefing {
            map_width: 40,
            map_height: 30,
            starting_foundries: Vec::new(),
            teams: vec![None, None],
            non_ground_terrain: Vec::new(),
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        };
        let profile =
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 7).resolve_profile();
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
        policy.fresh_economic_investments(EconomicInvestmentContext {
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
            let dials = Dials::balanced();
            policy.fresh_capacity_foundry_investment(
                &dials,
                EconomicInvestmentContext {
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
                },
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
        let economy = expansion_economy(&Dials::balanced(), &obs, obs.scrap, Reserve::Exact(0));
        policy.foundry_saving = Some(construction::FoundrySavingCommitment {
            plan: construction::FoundryExpansionPlan {
                anchor: saved,
                builder: UnitId(2),
                opportunity: expansion::FoundryOpportunity::capacity_only(saved, 1_000, economy),
            },
            accepted_at: obs.tick,
            required_scrap: economy.foundry_cost,
            forecast_basis: None,
            blocked_since: None,
        });
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
        assert_eq!(policy.foundry_saving.as_ref().unwrap().plan.anchor, saved);
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
        policy.economic_saving = Some(initial);
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
        assert_eq!(policy.economic_saving(), Some(&quote));
        policy.commit_economic_investment(quote.clone(), quote.cost, &mut intents);
        assert_eq!(intents, vec![quote.intent()]);
        assert!(policy.economic_saving().is_none());
    }

    fn air_quotes(
        obs: &Observation,
        map: &PublicMapBriefing,
        profile: &ResolvedProfile,
        work: &[AirCapacityDemand],
    ) -> Vec<EconomicInvestment> {
        let resources = ResourceSnapshot::from_observation(obs);
        UtilityPolicy::new()
            .fresh_economic_investments(EconomicInvestmentContext {
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
            policy.pending_sites.push(anchor);
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
                policy.economic_cancelled_founder,
                (!paid && !retained).then_some((builder, kind, anchor))
            );
            if retained {
                assert_eq!(
                    policy.economic_foundation.as_ref().unwrap().deadline,
                    quote.deadline
                );
                let mut intents = Vec::new();
                obs.scrap = quote.cost;
                assert!(
                    policy
                        .post_floor_deferred_claims(
                            &obs,
                            TilePos::new(3, 12),
                            Some(&[]),
                            Some(&[]),
                            Some(&map),
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
        for expired in [false, true] {
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
            }
            let paid = building(7, BuildingKind::Reclaimer, TilePos::new(21, 3));
            obs.my_buildings.push(paid.clone());
            let resources = ResourceSnapshot::from_observation(&obs);
            policy.refresh_economic_saving(
                EconomicInvestmentContext {
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
                expired,
            );
            assert!(policy.economic_saving().is_none());
            assert!(policy.economic_retry_at > obs.tick);
            assert!(obs.my_buildings.contains(&paid));
        }
    }
}
