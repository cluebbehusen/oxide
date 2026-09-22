//! One economic quotation pass with observation-scoped inputs and scratch.
use super::super::construction_checks::ConstructionChecks;
use super::super::defense::DefenseThinkContext;
use super::super::economic_work::HarvestRegion;
use super::*;

pub(crate) struct EconomicQuotes<'a> {
    policy: &'a UtilityPolicy,
    context: EconomicInvestmentContext<'a>,
    routes: Option<ServiceRoutes<'a>>,
    funding: Option<FundingCalendar<'a>>,
}
struct CapitalPreparation<'a> {
    retained: Option<&'a EconomicInvestment>,
    horizon: u64,
    deadline: u64,
    unmet_income: u64,
    income_evidence: Option<ProposalCase>,
    builders: Vec<&'a UnitObs>,
    bootstrap_air: bool,
    projected_bank: u32,
}
impl<'a> EconomicQuotes<'a> {
    pub(super) fn new(policy: &'a UtilityPolicy, context: EconomicInvestmentContext<'a>) -> Self {
        Self {
            policy,
            context,
            routes: None,
            funding: None,
        }
    }

    pub(crate) fn capacity_foundry(
        &mut self,
        dials: &Dials,
        foundry_context: construction::FreshFoundryProposalContext<'_>,
    ) -> Option<construction::FreshFoundryInvestment> {
        let policy = self.policy;
        let context = self.context;

        let obs = context.obs;
        if !dials.expansion
            || policy.state.foundry_saving.is_some()
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
            || UtilityPolicy::projected_foundries(obs).1 != 0
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
        let delay = economy.build_ticks.saturating_add(
            self.funding
                .get_or_insert_with(|| FundingCalendar::new(&context))
                .delay(economy.foundry_cost, obs.tick.saturating_add(horizon)),
        );
        let mut infrastructure = infrastructure_context(
            EconomicInvestmentContext {
                air_work: &[],
                ..context
            },
            self.routes.get_or_insert_with(|| service_routes(context)),
        );
        let opportunities = obs
            .my_buildings
            .iter()
            .filter(|building| building.built && building.kind == BuildingKind::Foundry)
            .filter_map(|home| policy.placement_near(obs, BuildingKind::Foundry, home.anchor))
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
        policy.fresh_foundry_with_opportunities(
            dials,
            obs,
            context.resources,
            foundry_context,
            Some(expansion::rank_foundry_opportunities(opportunities)),
        )
    }

    pub(crate) fn investments(&mut self) -> Vec<EconomicInvestment> {
        let policy = self.policy;
        let context = self.context;

        let obs = context.obs;
        if obs.tick < policy.state.economic_retry_at || policy.state.economic_foundation.is_some() {
            return Vec::new();
        }
        let retained = policy.state.economic_saving.as_ref();
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
            policy.economic_harvest_regions(
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
        self.quote_workers(retained, horizon, deadline, &regions, &mut proposals);
        let capital = self.prepare_capital(retained, horizon, deadline, &regions);
        let possible = self.construction_sites(&capital);
        let mut geometry = None;
        self.quote_construction(&capital, possible, &mut geometry, &mut proposals);
        self.quote_upgrades(&capital, &mut proposals);
        if retained.is_none() {
            policy.value_extractor_developments(
                context,
                &mut geometry,
                self.funding
                    .get_or_insert_with(|| FundingCalendar::new(&context)),
                &mut proposals,
            );
        }
        rank_quotes(&mut proposals);
        proposals
    }

    fn quote_workers(
        &self,
        retained: Option<&EconomicInvestment>,
        horizon: u64,
        deadline: u64,
        regions: &[HarvestRegion],
        proposals: &mut Vec<EconomicInvestment>,
    ) {
        let policy = self.policy;
        let context = self.context;
        let obs = context.obs;
        for region in regions {
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
            for work in policy.orphan_construction_work(
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
    }

    fn prepare_capital(
        &mut self,
        retained: Option<&'a EconomicInvestment>,
        horizon: u64,
        deadline: u64,
        regions: &[HarvestRegion],
    ) -> CapitalPreparation<'a> {
        let policy = self.policy;
        let context = self.context;
        let obs = context.obs;
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
        let builders = policy
            .construction_builders(obs, &[], context.unavailable)
            .into_iter()
            .filter(|unit| {
                builder_is_free(obs, unit) && !policy.state.evacuating_workers.contains(&unit.id)
            })
            .filter(|unit| retained.is_none_or(|saving| saving.builder == Some(unit.id)))
            .collect::<Vec<_>>();
        let bootstrap_air = !obs
            .my_buildings
            .iter()
            .any(|building| building.kind == BuildingKind::Airworks)
            && !UtilityPolicy::deferred_claims(obs)
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
        self.routes.get_or_insert_with(|| service_routes(context));
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
        CapitalPreparation {
            retained,
            horizon,
            deadline,
            unmet_income,
            income_evidence,
            builders,
            bootstrap_air,
            projected_bank,
        }
    }

    fn construction_sites(&self, capital: &CapitalPreparation<'_>) -> Vec<(BuildingKind, TilePos)> {
        let policy = self.policy;
        let context = self.context;
        let obs = context.obs;
        let retained = capital.retained;
        let unmet_income = capital.unmet_income;
        let builders = &capital.builders;
        let bootstrap_air = capital.bootstrap_air;

        let mut possible = Vec::new();
        if retained.is_none() {
            for &frame in &obs.known_frames {
                if policy.player_can_plan_frame_restoration(obs, frame)
                    && !UtilityPolicy::deferred_claims(obs)
                        .contains(&(BuildingKind::Extractor, frame))
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
                        policy.placement_near_where(obs, kind, home.anchor, |anchor| {
                            policy.state.foundry_saving.as_ref().is_none_or(|saving| {
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
                    policy
                        .planning
                        .infrastructure_sites(obs.tick, kind, &anchors)
                        .into_iter()
                        .map(move |anchor| (kind, anchor))
                })
                .collect::<BTreeSet<_>>();
            possible.retain(|&(kind, anchor)| {
                kind == BuildingKind::Extractor || selected.contains(&(kind, anchor))
            });
        }
        possible
    }

    fn quote_construction(
        &mut self,
        capital: &CapitalPreparation<'a>,
        possible: Vec<(BuildingKind, TilePos)>,
        geometry: &mut Option<ConstructionChecks<'a>>,
        proposals: &mut Vec<EconomicInvestment>,
    ) {
        let policy = self.policy;
        let context = self.context;
        let obs = context.obs;
        let retained = capital.retained;
        let horizon = capital.horizon;
        let deadline = capital.deadline;
        let unmet_income = capital.unmet_income;
        let income_evidence = capital.income_evidence;
        let builders = &capital.builders;
        let bootstrap_air = capital.bootstrap_air;
        let projected_bank = capital.projected_bank;

        let funding = self
            .funding
            .get_or_insert_with(|| FundingCalendar::new(&context));
        let mut infrastructure = infrastructure_context(
            context,
            self.routes.get_or_insert_with(|| service_routes(context)),
        );
        let have_built = |kind| {
            obs.my_buildings
                .iter()
                .any(|building| building.kind == kind && building.built)
        };
        let airworks_sites: Vec<_> = possible
            .iter()
            .filter_map(|&(kind, anchor)| (kind == BuildingKind::Airworks).then_some(anchor))
            .collect();
        let placement = super::terrain::PlacementGeometry::new(obs);
        for (kind, anchor) in possible {
            if !placement.valid(policy, kind, anchor) {
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
                ConstructionChecks::new(
                    crate::query_work::QueryPurpose::EconomicInvestment,
                    policy,
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
            let Some(builder) = geometry.safe_implicit_builder(kind, anchor, builders) else {
                continue;
            };
            let worker = builders
                .iter()
                .find(|worker| worker.id == builder)
                .expect("the quote binds an eligible worker");
            let Some(distance) = geometry.builder_travel_cost(worker, kind, anchor) else {
                continue;
            };
            let funding_delay = funding.delay(stats.cost, deadline);
            let delay = funding_delay
                .saturating_add(travel_ticks(worker.kind, distance))
                .saturating_add(
                    u64::from(stats.build_ticks)
                        .div_ceil(u64::from(worker.kind.stats().build_rate.max(1))),
                );
            let mut capacity = None;
            let benefit = match kind {
                BuildingKind::Extractor => {
                    let rate = if UtilityPolicy::frame_has_foundry_support(obs, anchor) {
                        oxide_sim::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
                    } else {
                        oxide_sim::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE
                    };
                    horizon
                        .saturating_sub(delay)
                        .saturating_mul(u64::from(rate))
                        / (u64::from(oxide_sim::TICKS_PER_SECOND) * 60)
                }
                BuildingKind::Reclaimer => RecurringReturn {
                    horizon,
                    ready_after: delay,
                    old_period: None,
                    new_period: oxide_sim::stats::RECLAIMER_PERIOD,
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
                        let request = crate::strategy::FreshConnectedProposalRequest::new(
                            context.profile,
                            DifficultyTuning::for_level(context.profile.difficulty),
                            obs,
                            context.resources,
                            &intelligence,
                            home,
                            crate::strategy::StrategicCoordination {
                                planning: Some(&policy.planning),
                                enlisted: context.unavailable,
                                lift_support: None,
                                allow_new_operation: true,
                                protected_current_scrap: context.protected_scrap,
                                protected_forecast_scrap:
                                    crate::allocation::forecast_reserve_through(
                                        context.obligations,
                                        deadline,
                                    ),
                                public_map: Some(context.briefing),
                                orientation: context.orientation,
                            },
                        );
                        if let Some(benefit) = crate::strategy::prospective_airworks_package_value(
                            request,
                            candidate,
                            &airworks_sites,
                            delay,
                            deadline,
                            context.obligations,
                            &policy.planning,
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
    }

    fn quote_upgrades(
        &mut self,
        capital: &CapitalPreparation<'a>,
        proposals: &mut Vec<EconomicInvestment>,
    ) {
        let mut geometry = None;
        let policy = self.policy;
        let context = self.context;
        let obs = context.obs;
        let retained = capital.retained;
        let horizon = capital.horizon;
        let deadline = capital.deadline;
        let unmet_income = capital.unmet_income;
        let income_evidence = capital.income_evidence;
        let projected_bank = capital.projected_bank;

        let funding = self
            .funding
            .get_or_insert_with(|| FundingCalendar::new(&context));
        let have_built = |kind| {
            obs.my_buildings
                .iter()
                .any(|building| building.kind == kind && building.built)
        };
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
            let funding_delay = funding.delay(upgrade.cost, deadline);
            let refit_delay = funding_delay.saturating_add(u64::from(upgrade.build_ticks));
            let (benefit, defense_evidence) = if building.kind == BuildingKind::Reclaimer {
                (
                    RecurringReturn {
                        horizon: horizon.saturating_sub(funding_delay),
                        ready_after: u64::from(upgrade.build_ticks),
                        old_period: Some(oxide_sim::stats::RECLAIMER_PERIOD),
                        new_period: oxide_sim::stats::REFINERY_PERIOD,
                        unmet_demand: unmet_income,
                    }
                    .marginal(),
                    None,
                )
            } else {
                let (benefit, evidence) = geometry
                    .get_or_insert_with(|| {
                        DefenseThinkContext::new_oriented(
                            crate::query_work::QueryPurpose::EconomicInvestment,
                            policy,
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
    }
}
fn rank_quotes(proposals: &mut Vec<EconomicInvestment>) {
    proposals.sort_unstable_by_key(|proposal| {
        (
            std::cmp::Reverse(
                proposal.benefit.saturating_mul(1_000) / u64::from(proposal.valuation_cost.max(1)),
            ),
            proposal.key,
        )
    });
    let mut seen = BTreeSet::new();
    proposals.retain(|proposal| seen.insert(proposal.key));
}
fn service_routes(context: EconomicInvestmentContext<'_>) -> ServiceRoutes<'_> {
    ServiceRoutes::new(
        QueryPurpose::EconomicInvestment,
        context.obs,
        Some(context.briefing),
        Some(context.orientation),
    )
}
fn infrastructure_context<'a, 'r>(
    context: EconomicInvestmentContext<'a>,
    routes: &'r mut ServiceRoutes<'a>,
) -> InfrastructureContext<'a, 'r> {
    InfrastructureContext {
        obs: context.obs,
        resources: context.resources,
        demands: context.demands,
        routes,
        briefing: context.briefing,
        orientation: context.orientation,
        air_work: context.air_work,
        protected_scrap: context.protected_scrap,
    }
}
