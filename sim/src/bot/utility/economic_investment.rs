//! Exact economic actions derived from finite work and unmet capability demand.

use super::defense::DefenseThinkContext;
use super::economic_value::{
    CapacityReturn, RecurringReturn, WorkerService, investment_horizon, travel_ticks,
};
use super::*;
use crate::bot::allocation::{
    Confidence, ExecutionSafety, ProposalCase, StrategicValue, TimeToImpact, Urgency,
};
use crate::bot::intelligence::StrategicIntelligence;
use crate::bot::navigation::service::ServiceRoutes;
use crate::bot::orient::Orientation;
use crate::bot::query_work::QueryPurpose;
use crate::bot::resources::ProducerEgress;
use crate::bot::standing_force::CapabilityDemand;
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
    pub(in crate::bot) valuation_cost: u32,
    pub(in crate::bot) current_capital: u32,
    pub(in crate::bot) observed_at: u64,
    pub(in crate::bot) ready_at: u64,
    pub(in crate::bot) deadline: u64,
    pub(in crate::bot) fund_by: u64,
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
    pub(in crate::bot) obligations: &'a [crate::bot::allocation::ImportedObligation],
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
            routes: ServiceRoutes::new(
                QueryPurpose::EconomicInvestment,
                obs,
                Some(context.briefing),
                Some(context.orientation),
            ),
            briefing: context.briefing,
            orientation: context.orientation,
            air_work: &[],
            protected_scrap: context.protected_scrap,
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
                )
                .benefit;
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

    pub(in crate::bot) fn commit_economic_investment(
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
        if !core_ready || now >= saving.deadline || now > saving.fund_by {
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
                        valuation_cost: kind.stats().cost,
                        current_capital: kind.stats().cost,
                        observed_at: obs.tick,
                        ready_at: timing.no_block_latest_ready_tick,
                        deadline,
                        fund_by: obs.tick,
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
                            valuation_cost: kind.stats().cost,
                            current_capital: kind.stats().cost,
                            observed_at: obs.tick,
                            ready_at: timing.no_block_latest_ready_tick,
                            deadline,
                            fund_by: obs.tick,
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
        let bootstrap_air = !obs
            .my_buildings
            .iter()
            .any(|building| building.kind == BuildingKind::Airworks)
            && !Self::deferred_claims(obs)
                .iter()
                .any(|(kind, _)| *kind == BuildingKind::Airworks)
            && obs
                .enemy_buildings
                .iter()
                .any(|building| building.seen && building.hp > 0)
            && obs.scrap.saturating_sub(context.protected_scrap)
                >= BuildingKind::Airworks
                    .base_stats()
                    .construction
                    .unwrap()
                    .cost;
        let mut infrastructure = InfrastructureContext {
            obs,
            resources: context.resources,
            demands: context.demands,
            routes: ServiceRoutes::new(
                QueryPurpose::EconomicInvestment,
                obs,
                Some(context.briefing),
                Some(context.orientation),
            ),
            briefing: context.briefing,
            orientation: context.orientation,
            air_work: context.air_work,
            protected_scrap: context.protected_scrap,
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
                        && !(kind == BuildingKind::Airworks
                            && (!context.air_work.is_empty() || bootstrap_air))
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
        if retained.is_none() {
            let mut infrastructure_sites =
                std::collections::BTreeMap::<BuildingKind, Vec<TilePos>>::new();
            for &(kind, anchor) in &possible {
                if kind != BuildingKind::Extractor {
                    infrastructure_sites.entry(kind).or_default().push(anchor);
                }
            }
            let selected = infrastructure_sites
                .into_iter()
                .flat_map(|(kind, mut anchors)| {
                    anchors.sort_by_cached_key(|anchor| {
                        (
                            builders
                                .iter()
                                .map(|builder| builder.tile.manhattan(*anchor))
                                .min()
                                .unwrap_or(i32::MAX),
                            anchor.y,
                            anchor.x,
                        )
                    });
                    self.planning
                        .infrastructure_sites(obs.tick, kind, &anchors)
                        .into_iter()
                        .map(move |anchor| (kind, anchor))
                })
                .collect::<BTreeSet<_>>();
            possible.retain(|&(kind, anchor)| {
                kind == BuildingKind::Extractor || selected.contains(&(kind, anchor))
            });
        }
        let airworks_sites: Vec<_> = possible
            .iter()
            .filter_map(|&(kind, anchor)| (kind == BuildingKind::Airworks).then_some(anchor))
            .collect();
        let placement = super::terrain::PlacementGeometry::new(obs);
        for (kind, anchor) in possible {
            if !placement.valid(self, kind, anchor) {
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
                    crate::bot::query_work::QueryPurpose::EconomicInvestment,
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
            let mut capacity = None;
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
                _ => {
                    let mut value =
                        infrastructure_benefit(&mut infrastructure, kind, anchor, horizon, delay);
                    if kind == BuildingKind::Airworks && bootstrap_air {
                        let mut intelligence = StrategicIntelligence::new();
                        intelligence.update(obs);
                        let home = obs
                            .my_buildings
                            .iter()
                            .filter(|building| {
                                building.kind == BuildingKind::Foundry && building.built
                            })
                            .min_by_key(|building| building.id)
                            .unwrap()
                            .anchor;
                        let candidate = BuildingObs {
                            id: BuildingId(u32::MAX),
                            player: obs.me,
                            kind,
                            anchor,
                            hp: kind.base_stats().max_hp,
                            built: true,
                            provisional: false,
                            tier: 0,
                            seen: true,
                        };
                        let request = crate::bot::strategy::FreshConnectedProposalRequest::new(
                            context.profile,
                            DifficultyTuning::for_level(context.profile.difficulty),
                            obs,
                            context.resources,
                            &intelligence,
                            home,
                            crate::bot::strategy::StrategicCoordination {
                                planning: Some(&self.planning),
                                enlisted: context.unavailable,
                                lift_support: None,
                                allow_new_operation: true,
                                protected_current_scrap: context.protected_scrap,
                                protected_forecast_scrap:
                                    crate::bot::allocation::forecast_reserve_through(
                                        context.obligations,
                                        deadline,
                                    ),
                                public_map: Some(context.briefing),
                                orientation: context.orientation,
                            },
                        );
                        if let Some(benefit) =
                            crate::bot::strategy::prospective_airworks_package_value(
                                request,
                                candidate,
                                &airworks_sites,
                                delay,
                                deadline,
                                context.obligations,
                                &self.planning,
                            )
                            .filter(|benefit| *benefit > value.benefit)
                        {
                            value = InfrastructureReturn {
                                benefit,
                                case: Some(ProposalCase {
                                    urgency: Urgency::Timely,
                                    confidence: Confidence::Supported,
                                    value: StrategicValue::Material,
                                    time_to_impact: TimeToImpact::Patient,
                                    safety: ExecutionSafety::Managed,
                                }),
                            };
                        }
                    }
                    capacity = value.case;
                    value.benefit
                }
            };
            if benefit < u64::from(stats.cost) {
                continue;
            }
            let mut case = if capacity.is_some() {
                economic_case(benefit, stats.cost, delay)
            } else {
                infrastructure_case(&context, kind, benefit, stats.cost, delay)
            };
            if let Some(evidence) = capacity {
                case.confidence = evidence.confidence;
                case.urgency = evidence.urgency;
            }
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
                valuation_cost: stats.cost,
                current_capital: obs
                    .scrap
                    .saturating_sub(context.protected_scrap)
                    .min(stats.cost),
                observed_at: obs.tick,
                ready_at: obs.tick.saturating_add(delay),
                deadline,
                fund_by: retained.map_or_else(
                    || {
                        if funding_delay == 0 {
                            obs.tick
                        } else {
                            obs.tick
                                .saturating_add(funding_delay)
                                .saturating_add(context.cadence.max(1))
                        }
                    },
                    |saving| saving.fund_by,
                ),
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
            let (benefit, defense_evidence) = if building.kind == BuildingKind::Reclaimer {
                (
                    RecurringReturn {
                        horizon: horizon.saturating_sub(funding_delay),
                        ready_after: u64::from(upgrade.build_ticks),
                        old_period: Some(crate::stats::RECLAIMER_PERIOD),
                        new_period: crate::stats::REFINERY_PERIOD,
                        unmet_demand: unmet_income,
                    }
                    .marginal(),
                    None,
                )
            } else {
                let (benefit, evidence) = geometry
                    .get_or_insert_with(|| {
                        DefenseThinkContext::new_oriented(
                            crate::bot::query_work::QueryPurpose::EconomicInvestment,
                            self,
                            obs,
                            context.briefing,
                            context.unit_contacts,
                            context.building_contacts,
                            context.orientation,
                        )
                    })
                    .upgrade_quote(building, horizon.saturating_sub(funding_delay));
                (benefit, Some(evidence))
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
            if let Some(evidence) = defense_evidence {
                use super::defense::DefenseOpportunityEvidence;
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
                valuation_cost: upgrade.cost,
                observed_at: obs.tick,
                current_capital: obs
                    .scrap
                    .saturating_sub(context.protected_scrap)
                    .min(upgrade.cost),
                ready_at: obs.tick.saturating_add(refit_delay),
                deadline,
                fund_by: retained.map_or_else(
                    || {
                        if funding_delay == 0 {
                            obs.tick
                        } else {
                            obs.tick
                                .saturating_add(funding_delay)
                                .saturating_add(context.cadence.max(1))
                        }
                    },
                    |saving| saving.fund_by,
                ),
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
        if retained.is_none() {
            self.value_extractor_developments(context, &mut proposals);
        }
        proposals.sort_unstable_by_key(|proposal| {
            (
                std::cmp::Reverse(
                    proposal.benefit.saturating_mul(1_000)
                        / u64::from(proposal.valuation_cost.max(1)),
                ),
                proposal.key,
            )
        });
        let mut seen = BTreeSet::new();
        proposals.retain(|proposal| seen.insert(proposal.key));
        proposals
    }
}

pub(super) fn funding_delay(
    context: &EconomicInvestmentContext<'_>,
    cost: u32,
    deadline: u64,
) -> u64 {
    let now = context.obs.tick;
    let mut protected = 0u32;
    let mut payments = Vec::new();
    for obligation in context.obligations {
        let claims = &obligation.claims;
        protected = protected.saturating_add(claims.current_scrap());
        payments.extend(
            claims
                .forecast_scrap()
                .iter()
                .chain(claims.foregone_income())
                .map(|claim| (claim.through, claim.amount)),
        );
        if let Some(claim) = claims.deferrable_capital() {
            payments.push((claim.through, claim.amount));
        }
        for job in claims.producer_jobs() {
            let through = job
                .fixed_timing()
                .map_or(job.enqueue_not_before(), |(_, enqueued, _, _)| enqueued);
            if through <= now {
                protected = protected.saturating_add(job.kind().stats().cost);
            } else {
                payments.push((through, job.kind().stats().cost));
            }
        }
    }
    payments.sort_unstable();
    let bank = u64::from(
        context
            .obs
            .scrap
            .saturating_sub(context.protected_scrap.max(protected)),
    );
    let available = |through: u64| {
        bank.saturating_add(u64::from(
            context
                .resources
                .forecast()
                .income_through(through.saturating_sub(1))
                .amount(),
        ))
    };
    // The purchase must also leave every later retained payment fundable.
    // Exact lanes, forecast-only claims, and all other resources are still adjudicated together.
    let feasible = |through| {
        let mut required = u64::from(cost);
        for &(payment_at, amount) in &payments {
            required = required.saturating_add(u64::from(amount));
            if payment_at >= through && required > available(payment_at) {
                return false;
            }
        }
        let due = payments
            .iter()
            .take_while(|(at, _)| *at <= through)
            .map(|(_, amount)| u64::from(*amount))
            .fold(u64::from(cost), u64::saturating_add);
        due <= available(through)
    };
    let mut low = now;
    let mut high = deadline;
    while low < high {
        let mid = low + (high - low) / 2;
        if feasible(mid) {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    low.saturating_sub(now)
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
    routes: ServiceRoutes<'a>,
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
    context: &mut InfrastructureContext<'_>,
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
                Some(crate::bot::navigation::commands::air_production_spawn_tile(
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
                        Some(crate::bot::navigation::commands::air_production_spawn_tile(
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
        let origin = crate::bot::navigation::commands::air_production_spawn_tile(
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
    use super::*;
    use crate::bot::allocation::StandingForceServiceKey;
    use crate::bot::standing_force::StandingForceReason;
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};

    fn building(id: u32, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            provisional: false,
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
            regions: Default::default(),
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
    }

    #[test]
    fn first_airworks_is_valued_as_a_complete_affordable_campaign() {
        let (mut obs, map, profile) = fixture();
        obs.scrap = 1200;
        obs.my_buildings
            .push(building(2, BuildingKind::Fabricator, TilePos::new(8, 5)));
        obs.my_buildings
            .push(building(3, BuildingKind::Crucible, TilePos::new(12, 5)));
        obs.my_queues.resize(obs.my_buildings.len(), Vec::new());
        obs.my_queue_progress.resize(obs.my_buildings.len(), 0);
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
            let dials = Dials::balanced();
            policy.fresh_capacity_foundry_investment(
                &dials,
                EconomicInvestmentContext {
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
        use crate::bot::allocation::*;
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
        let mut geometry = DefenseThinkContext::new_oriented(
            crate::bot::query_work::QueryPurpose::NavigationTest,
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
                    obs.explored[((frame.y + dy) * obs.map_width + frame.x + dx) as usize] = false;
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
        use crate::bot::allocation::*;
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
        let unclaimed = funding_delay(&context, cost, deadline);
        context.obligations = std::slice::from_ref(&job);
        let funded = funding_delay(&context, cost, deadline);
        assert!(
            funded > unclaimed,
            "the old unit payment must delay, rather than lose to, the new purchase"
        );
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
        obligations: &[crate::bot::allocation::ImportedObligation],
    ) -> Vec<EconomicInvestment> {
        let resources = ResourceSnapshot::from_observation(obs);
        UtilityPolicy::new()
            .fresh_economic_investments(EconomicInvestmentContext {
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
        obs.map_width = 160;
        map.map_width = 160;
        obs.visible = vec![true; 160 * 30];
        obs.explored = obs.visible.clone();
        let source = TilePos::new(10, 12);
        obs.known_wrecks = vec![(source, 100_000)];
        obs.my_units[0].tile = TilePos::new(158, 12);
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
        let (far, work) = crate::bot::navigation::work::measure(|| regions(&obs));
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
            kind: crate::stats::Role::Bomber.unit_for(obs.faction),
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
            assert!(policy.economic_retry_at > obs.tick);
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
        let before = crate::bot::strategy::airworks_package_derivations();
        assert!(air_quotes(&obs, &map, &profile, &[]).is_empty());
        assert_eq!(
            crate::bot::strategy::airworks_package_derivations() - before,
            2
        );
    }

    #[test]
    fn bootstrap_airworks_respects_retained_cash_forecast_and_factory_work() {
        use crate::bot::allocation::{
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
        obs.my_queues.resize(obs.my_buildings.len(), vec![]);
        obs.my_queue_progress.resize(obs.my_buildings.len(), 0);
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
            crate::bot::allocation::CrossDomainAllocation::new(&resources, quote.deadline, 12)
                .unwrap();
        retained_only.import(job.clone());
        assert!(
            retained_only
                .resolve(
                    crate::bot::allocation::AllocationPersonality::default(),
                    None
                )
                .is_ok()
        );
        let derivations = crate::bot::strategy::airworks_package_derivations();
        assert!(
            air_quotes_with_obligations(&obs, &map, &profile, &[], std::slice::from_ref(&job))
                .is_empty()
        );
        assert_eq!(
            crate::bot::strategy::airworks_package_derivations(),
            derivations,
            "an Airworks that would break a fixed payment must be rejected before campaign derivation"
        );
        obs.scrap = 1200;
        assert!(
            !air_quotes_with_obligations(&obs, &map, &profile, &[], &[job]).is_empty(),
            "compatible retained work must not suppress a funded campaign"
        );
        assert!(crate::bot::strategy::airworks_package_derivations() > derivations);
    }
}
