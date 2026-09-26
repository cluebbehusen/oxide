mod producer_reservations;
use super::super::observation::BuildingObs;
use super::super::profile::{PersonalityTraits, Specialty};
use super::*;
use oxide_sim::map::Terrain;
use oxide_sim::scenario::{BotConfig, BotDifficulty};
use oxide_sim::state::Faction;

fn planner_with_operation(op: AirOperation, plan: AirPlan) -> StrategicPlanner {
    StrategicPlanner {
        air: Some(ActiveAirOperation { op, plan }),
        ..StrategicPlanner::new()
    }
}

const HOME: TilePos = TilePos::new(3, 10);
const TARGET: TilePos = TilePos::new(24, 10);

// These fixtures advance one domain with no competing planners. Active revisions
// receive their richest variant; controller admission is tested through Brain.
impl StrategicPlanner {
    fn think_alone(
        &mut self,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &Observation,
        intel: &StrategicIntelligence,
        home: TilePos,
        enlisted: &[UnitId],
    ) -> StrategicDecision {
        let fixture_planning = crate::planning::PlanningWork::default();

        self.think_alone_with(
            profile,
            tuning,
            obs,
            intel,
            home,
            StrategicCoordination {
                planning: Some(&fixture_planning),
                enlisted,
                lift_support: None,
                allow_new_operation: true,
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
                public_map: None,
                orientation: Orientation::for_home(obs, home),
            },
        )
        .decision
    }

    fn think_alone_with(
        &mut self,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &Observation,
        intel: &StrategicIntelligence,
        home: TilePos,
        coordination: StrategicCoordination<'_>,
    ) -> StrategicThinkResult {
        let resources = ResourceSnapshot::from_observation(obs);
        let request = FreshConnectedProposalRequest::new(
            profile,
            tuning,
            obs,
            &resources,
            intel,
            home,
            coordination,
        );
        let rejected_connected_candidate = match self.active_connected_revision_proposal(request) {
            Ok(Some(mut proposal)) => {
                if let Some(richest) = proposal.marginal_variants().last().cloned() {
                    assert!(proposal.select_marginal(&richest));
                }
                commit_test_connected_proposal(self, proposal);
                None
            }
            Err(rejected) => {
                self.reject_active_connected_revision(rejected.reason, obs.tick);
                Some(rejected)
            }
            Ok(None) => match self.fresh_connected_minimum_proposal(
                &crate::experience::Experience::default(),
                FreshConnectedProposalRequest::new(
                    profile,
                    tuning,
                    obs,
                    &resources,
                    intel,
                    home,
                    coordination,
                ),
            ) {
                Ok(Some(proposal)) => {
                    commit_test_connected_proposal(self, proposal);
                    None
                }
                Ok(None) => None,
                Err(rejected) => Some(rejected),
            },
        };
        let settlement = allocate_connected_in_test(self, request);
        let lanes = settlement
            .as_ref()
            .map_or_else(ProducerLaneReservations::default, |settlement| {
                settlement.producer_lane_reservations().clone()
            });
        let mut intents = Vec::new();
        if let Some(settlement) = &settlement {
            self.record_connected_purchases(settlement.producer_schedule(), obs.tick);
            intents.extend(
                settlement
                    .producer_schedule()
                    .iter()
                    .filter(|job| job.enqueued_at == obs.tick)
                    .map(|job| Intent::TrainAt {
                        building: job.producer,
                        kind: job.kind,
                    }),
            );
        }
        let mut result = self.think_after_connected_adjudication(
            StrategicThinkContext::new(profile, tuning, obs, intel, home, coordination)
                .with_producer_lanes(&intents, &lanes),
        );
        result.decision.intents.extend(intents);
        if let Some(settlement) = &settlement {
            result.decision.reserved_scrap = result.decision.reserved_scrap.saturating_add(
                settlement
                    .producer_schedule()
                    .iter()
                    .filter(|job| job.enqueued_at > obs.tick)
                    .map(|job| job.current_scrap)
                    .sum(),
            );
        }
        result.rejected_connected_candidate = rejected_connected_candidate;
        result
    }
}

fn profile() -> ResolvedProfile {
    ResolvedProfile {
        difficulty: BotDifficulty::Prime,
        stance: BotStance::Balanced,
        personality_seed: 7,
        primary: Specialty::Air,
        secondary: Specialty::Siege,
        traits: PersonalityTraits {
            air: 70,
            siege: 60,
            support: 45,
            fortification: 35,
            greed: 45,
            guile: 45,
        },
    }
}

fn planning_context<'a>(
    fixture_planning: &'a crate::planning::PlanningWork,
    identity: &'a ResolvedProfile,
    observation: &'a Observation,
    intelligence: &'a StrategicIntelligence,
) -> AirPlanningContext<'a> {
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.anchor == TARGET)
        .expect("the fixture has a current strategic target");
    AirPlanningContext {
        allow_procurement: true,
        planning: Some(fixture_planning),
        profile: identity,
        tuning: DifficultyTuning::for_level(identity.difficulty),
        obs: observation,
        intel: intelligence,
        home: HOME,
        orientation: test_orientation(),
        public_map: None,
        enlisted: &[],
        landing_sites: &[],
        connected_resources: Some(ConnectedProductionResources::from_observation(
            observation,
            target,
            &[],
            ConnectedRouteContext {
                campaign_routes: None,
                unavailable_paid: &[],
                intel: intelligence,
                home: HOME,
                target: target.anchor,
                public_map: None,
                orientation: test_orientation(),
            },
        )),
        production: StrategicProductionContext::empty(),
        protected_current_scrap: 0,
        protected_forecast_scrap: 0,
    }
}

fn obs(tick: Tick) -> Observation {
    Observation::from_data(ObservationData {
        tick,
        map_width: 32,
        map_height: 20,
        my_units: vec![
            own(1, UnitKind::Kestrel, TilePos::new(22, 10)),
            own(2, UnitKind::Bombard, TilePos::new(10, 10)),
            own(3, UnitKind::Condor, TilePos::new(4, 9)),
            own(4, UnitKind::Condor, TilePos::new(4, 11)),
        ],
        enemy_buildings: vec![building(80, 1, BuildingKind::Crucible, TARGET, true)],
        visible: vec![false; 32 * 20],
        explored: vec![false; 32 * 20],
        ..crate::test_support::observation_data()
    })
}

fn own(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
    crate::test_support::unit(id, PlayerId(0), kind, tile)
}

fn building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos, seen: bool) -> BuildingObs {
    BuildingObs {
        seen,
        ..crate::test_support::building(id, PlayerId(player), kind, anchor)
    }
}

fn operation(phase: AirOperationPhase, tick: Tick) -> AirOperation {
    AirOperation {
        target_player: PlayerId(1),
        target_kind: BuildingKind::Crucible,
        target: TARGET,
        target_id: Some(BuildingId(80)),
        stage: match phase {
            AirOperationPhase::Recon => AirStage::Recon,
            AirOperationPhase::Assemble => AirStage::Assemble,
            AirOperationPhase::SuppressAa => AirStage::SuppressAa,
            AirOperationPhase::Verify => AirStage::Verify,
            AirOperationPhase::Strike => AirStage::Strike,
            AirOperationPhase::Recover => AirStage::Recover {
                reason: AirRecoveryReason::Timeout,
                assault_admitted: true,
            },
        },
        started_at: tick - 50,
        phase_started_at: tick - 50,
        scout: Some(UnitId(1)),
        scout_dispatch: None,
        strike_hold: None,
        artillery_staging: None,
        artillery: vec![UnitId(2)],
        strike_aircraft: vec![UnitId(3), UnitId(4)],
        strike_issued_at: None,
        membership_frozen_at: matches!(
            phase,
            AirOperationPhase::SuppressAa | AirOperationPhase::Verify | AirOperationPhase::Strike
        )
        .then_some(tick),
    }
}

fn connected_test_plan(observation: &Observation) -> AirPlan {
    let faction = observation.faction;
    let scout = Role::Scout.unit_for(faction);
    let strike = Role::Bomber.unit_for(faction);
    AirPlan::connected(
        ConnectedForcePackage {
            derived_at: observation.tick,
            preparation_deadline: observation
                .tick
                .saturating_add(CONNECTED_PREPARATION_HORIZON),
            target_anchors: vec![TARGET],
            recon: vec![ProviderDemand {
                kind: scout,
                count: 1,
            }],
            suppression: vec![ProviderDemand {
                kind: UnitKind::Bombard,
                count: 1,
            }],
            strike: vec![ProviderDemand {
                kind: strike,
                count: 2,
            }],
            provider_priority: vec![
                force_package::ProviderDemandTranche {
                    priority: force_package::ProviderPriority::Minimum,
                    family: ForceFamily::Recon,
                    kind: scout,
                    count: 1,
                },
                force_package::ProviderDemandTranche {
                    priority: force_package::ProviderPriority::Minimum,
                    family: ForceFamily::Suppression,
                    kind: UnitKind::Bombard,
                    count: 1,
                },
                force_package::ProviderDemandTranche {
                    priority: force_package::ProviderPriority::Minimum,
                    family: ForceFamily::Strike,
                    kind: strike,
                    count: 1,
                },
                force_package::ProviderDemandTranche {
                    priority: force_package::ProviderPriority::Marginal,
                    family: ForceFamily::Strike,
                    kind: strike,
                    count: 1,
                },
            ],
            funded_providers: Vec::new(),
            minimum_capability: NormalizedCapability {
                recon: 1_000,
                suppression: suppression_capability(UnitKind::Bombard, faction),
                strike: strike_capability(strike, faction),
            },
            useful_capability: NormalizedCapability {
                recon: 1_000,
                suppression: suppression_capability(UnitKind::Bombard, faction),
                strike: strike_capability(strike, faction).saturating_mul(2),
            },
            useful_bombing: 0,
            target_value: 1,
            current_scrap: observation.scrap,
            observed_aa_firepower: 0,
            suppressible_aa_firepower: 0,
            forecast_scrap: 0,
            chosen_capability: NormalizedCapability {
                recon: 1_000,
                suppression: suppression_capability(UnitKind::Bombard, faction),
                strike: strike_capability(strike, faction).saturating_mul(2),
            },
            chosen_bombing: 0,
        },
        observation.tick,
        TARGET,
    )
}

fn derived_connected_test_plan(
    identity: &ResolvedProfile,
    observation: &Observation,
) -> Option<AirPlan> {
    let fixture_planning = crate::planning::PlanningWork::default();

    let intelligence = knowledge(observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|building| building.evidence == ContactEvidence::Current)?;
    let resources = ConnectedProductionResources::from_observation(
        observation,
        target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home: HOME,
            target: target.anchor,
            public_map: None,
            orientation: test_orientation(),
        },
    );
    connected_plan(
        identity,
        observation,
        &intelligence,
        HOME,
        target,
        &[],
        ConnectedPlanningContext {
            planning: Some(&fixture_planning),
            minimum_only: false,
            campaign_routes: None,
            orientation: test_orientation(),
            public_map: None,
            resources: &resources,
            preferred_artillery: &[],
            protected_current_scrap: 0,
            preparation: PreparationConstraints {
                deadline: observation
                    .tick
                    .saturating_add(CONNECTED_PREPARATION_HORIZON),
                decision_cadence: DifficultyTuning::for_level(identity.difficulty).cadence,
                protected_forecast_scrap: 0,
            },
        },
    )
    .ok()
}

fn see_approach(obs: &mut Observation) {
    see_approach_to(obs, TARGET);
}

fn see_approach_to(obs: &mut Observation, target: TilePos) {
    for tile in approach(HOME, target) {
        let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
        obs.visible[index] = true;
        obs.explored[index] = true;
    }
}

fn see_building_footprint(obs: &mut Observation, anchor: TilePos, kind: BuildingKind) {
    let (width, height) = kind.base_stats().size;
    for dy in 0..height {
        for dx in 0..width {
            let tile = anchor.offset(dx, dy);
            let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
            obs.visible[index] = true;
            obs.explored[index] = true;
        }
    }
}

fn explore(obs: &mut Observation, tile: TilePos) {
    let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
    obs.explored[index] = true;
}

fn public_map_with_terrain(
    observation: &Observation,
    terrain: impl IntoIterator<Item = (TilePos, Terrain)>,
) -> PublicMapBriefing {
    let mut non_ground_terrain: Vec<_> = terrain.into_iter().collect();
    non_ground_terrain.sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
    PublicMapBriefing {
        regions: Default::default(),
        map_width: observation.map_width,
        map_height: observation.map_height,
        starting_foundries: Vec::new(),
        teams: vec![None, None],
        non_ground_terrain: non_ground_terrain.into(),
        extractor_frames: Vec::new(),
        initial_scrap: Vec::new(),
    }
}

fn movement_cap_serpentine_map(observation: &Observation) -> PublicMapBriefing {
    assert_eq!((observation.map_width, observation.map_height), (256, 256));
    let width = observation.map_width;
    public_map_with_terrain(
        observation,
        (0..observation.map_height).flat_map(|y| {
            (0..observation.map_width).filter_map(move |x| {
                let tile = TilePos::new(x, y);
                let open = y % 2 == 0
                    || (y % 4 == 1 && x == width - 1)
                    || (y % 4 == 3 && x == 0)
                    || tile == TilePos::new(255, 255);
                (!open).then_some((tile, Terrain::Pit))
            })
        }),
    )
}

fn knowledge(obs: &Observation) -> StrategicIntelligence {
    let mut intel = StrategicIntelligence::new();
    intel.update(obs);
    intel
}

fn with_operation(phase: AirOperationPhase, tick: Tick) -> StrategicPlanner {
    let observation = obs(tick);
    planner_with_operation(operation(phase, tick), connected_test_plan(&observation))
}

fn wealthy_airborne_operation(
    phase: AirOperationPhase,
    mobile_aa: UnitKind,
    mobile_aa_count: usize,
) -> (Observation, StrategicPlanner) {
    let mut battle = wealthy_island_obs(5_000, 2);
    battle.faction = Faction::Cupric;
    battle.my_units = vec![own(1, UnitKind::Gnat, TARGET.offset(-2, 0))];
    battle
        .my_units
        .extend((0..10).map(|index| own(100 + index, UnitKind::Moth, HOME.offset(0, 2))));
    battle
        .my_units
        .extend((0..5).map(|index| own(200 + index, UnitKind::Darter, HOME.offset(1, 2))));
    battle.enemy_units.extend((0..mobile_aa_count).map(|index| {
        let mut unit = own(
            300 + u32::try_from(index).unwrap(),
            mobile_aa,
            TARGET.offset(-2, i32::try_from(index % 3).unwrap() - 1),
        );
        unit.player = PlayerId(1);
        unit
    }));
    battle.my_units.sort_unstable_by_key(|unit| unit.id);
    battle.enemy_units.sort_unstable_by_key(|unit| unit.id);
    see_approach(&mut battle);

    let mut operation = operation(phase, battle.tick);
    operation.artillery.clear();
    operation.scout = Some(UnitId(1));
    operation.strike_aircraft = (100..110).map(UnitId).collect();
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 10;
    plan.desired_screen = 5;
    plan.screen = (200..205).map(UnitId).collect();
    let planner = planner_with_operation(operation, plan);
    (battle, planner)
}

fn wealthy_island_obs(tick: Tick, airworks: usize) -> Observation {
    let mut observation = obs(tick);
    observation.scrap = 50_000;
    observation
        .my_units
        .extend((5..=16).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
    observation.my_buildings = vec![
        building(20, 0, BuildingKind::Foundry, HOME, true),
        building(21, 0, BuildingKind::Reclaimer, TilePos::new(5, 4), true),
        building(22, 0, BuildingKind::Reclaimer, TilePos::new(7, 4), true),
    ];
    observation.my_buildings.extend((0..airworks).map(|index| {
        building(
            23 + u32::try_from(index).unwrap(),
            0,
            BuildingKind::Airworks,
            TilePos::new(3 + i32::try_from(index).unwrap() * 3, 15),
            true,
        )
    }));
    observation.my_buildings.push(building(
        30,
        0,
        BuildingKind::Crucible,
        TilePos::new(10, 15),
        true,
    ));
    observation.my_queues = vec![Vec::new(); observation.my_buildings.len()];
    observation.known_rock = (0..observation.map_height)
        .map(|y| TilePos::new(16, y))
        .collect();
    observation
}

fn developed_connected_obs(tick: Tick) -> Observation {
    let mut observation = obs(tick);
    observation.visible.fill(true);
    observation.explored.fill(true);
    observation.scrap = 10_000;
    observation
        .my_units
        .extend((5..=13).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
    observation.my_units.sort_unstable_by_key(|unit| unit.id);
    observation.my_buildings = vec![
        building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
        building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
        building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
    ];
    observation.my_queues = vec![Vec::new(); observation.my_buildings.len()];
    observation
}

fn production_hungry_connected_obs(tick: Tick, scrap: u32) -> Observation {
    let mut observation = developed_connected_obs(tick);
    observation.my_units.retain(|unit| {
        !matches!(
            unit.kind,
            UnitKind::Kestrel | UnitKind::Bombard | UnitKind::Condor
        )
    });
    observation
        .my_units
        .extend((14..=17).map(|id| own(id, UnitKind::Sentinel, TilePos::new(8, 10))));
    observation.my_units.sort_unstable_by_key(|unit| unit.id);
    observation.scrap = scrap;
    assert!(combat_roster(&observation) >= CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER);
    observation
}

fn add_renewable_economy(observation: &mut Observation, count: usize) {
    for index in 0..count {
        observation.my_buildings.push(building(
            1_000 + u32::try_from(index).unwrap(),
            0,
            BuildingKind::Reclaimer,
            TilePos::new(
                1 + i32::try_from(index % 4).unwrap() * 3,
                1 + i32::try_from(index / 4).unwrap() * 3,
            ),
            true,
        ));
        observation.my_queues.push(Vec::new());
    }
}

fn think(
    planner: &mut StrategicPlanner,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> StrategicDecision {
    planner.think_alone(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        obs,
        intel,
        HOME,
        &[],
    )
}

fn coordination<'a>(
    fixture_planning: &'a crate::planning::PlanningWork,
    lift_support: Option<&'a LiftSupportRequest>,
) -> StrategicCoordination<'a> {
    StrategicCoordination {
        planning: Some(fixture_planning),
        enlisted: &[],
        lift_support,
        allow_new_operation: true,
        protected_current_scrap: 0,
        protected_forecast_scrap: 0,
        public_map: None,
        orientation: test_orientation(),
    }
}

#[test]
fn active_revision_invalidates_orders_for_every_reassigned_provider_role() {
    let mut operation = operation(AirOperationPhase::Recon, 120);
    operation.scout_dispatch = Some((operation.scout.unwrap(), TARGET));
    operation.artillery_staging = Some(HOME);
    operation.strike_hold = Some(HOME);
    let previous_scout = operation.scout;
    let previous_artillery = operation.artillery.clone();
    let previous_strike_aircraft = operation.strike_aircraft.clone();
    operation.scout = Some(UnitId(10));
    operation.artillery = vec![UnitId(11)];
    operation.strike_aircraft = vec![UnitId(12)];

    invalidate_reassigned_member_orders(
        &mut operation,
        previous_scout,
        &previous_artillery,
        &previous_strike_aircraft,
    );

    assert_eq!(operation.scout_dispatch, None);
    assert_eq!(operation.artillery_staging, None);
    assert_eq!(operation.strike_hold, None);
}

#[test]
fn active_revision_preserves_orders_when_provider_membership_is_unchanged() {
    let mut operation = operation(AirOperationPhase::Recon, 120);
    let scout_dispatch = Some((operation.scout.unwrap(), TARGET));
    let artillery_staging = Some(HOME);
    let strike_hold = Some(HOME);
    operation.scout_dispatch = scout_dispatch;
    operation.artillery_staging = artillery_staging;
    operation.strike_hold = strike_hold;
    let previous_scout = operation.scout;
    let previous_artillery = operation.artillery.clone();
    let previous_strike_aircraft = operation.strike_aircraft.clone();

    invalidate_reassigned_member_orders(
        &mut operation,
        previous_scout,
        &previous_artillery,
        &previous_strike_aircraft,
    );

    assert_eq!(operation.scout_dispatch, scout_dispatch);
    assert_eq!(operation.artillery_staging, artillery_staging);
    assert_eq!(operation.strike_hold, strike_hold);
}

fn commit_test_connected_proposal(
    planner: &mut StrategicPlanner,
    proposal: FreshConnectedProposal,
) {
    planner.commit_connected(proposal);
}

fn allocate_connected_in_test(
    planner: &mut StrategicPlanner,
    request: FreshConnectedProposalRequest<'_>,
) -> Option<crate::allocation::CrossDomainSettlement> {
    use crate::allocation::{
        AllocationPersonality, CrossDomainAllocation, active_connected_obligation,
    };
    let active = planner.active_connected_obligation(request)?;
    let resources = request
        .resource_snapshot
        .after_current_reserve(request.coordination.protected_current_scrap);
    let mut allocation =
        CrossDomainAllocation::new(&resources, active.deadline(), request.tuning.cadence).ok()?;
    allocation.import(active_connected_obligation(&active));
    allocation
        .resolve(AllocationPersonality::default(), None)
        .ok()
}

fn procure_connected_in_test(
    op: &AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) {
    let mut planner = StrategicPlanner::new();
    planner.air = Some(ActiveAirOperation {
        op: op.clone(),
        plan: plan.clone(),
    });
    let resources = ResourceSnapshot::from_observation(context.obs);
    let request = FreshConnectedProposalRequest::new(
        context.profile,
        context.tuning,
        context.obs,
        &resources,
        context.intel,
        context.home,
        StrategicCoordination {
            planning: context.planning,
            enlisted: context.enlisted,
            lift_support: None,
            allow_new_operation: false,
            protected_current_scrap: context.protected_current_scrap,
            protected_forecast_scrap: context.protected_forecast_scrap,
            public_map: context.public_map,
            orientation: context.orientation,
        },
    );
    if let Some(settlement) = allocate_connected_in_test(&mut planner, request) {
        out.reserved_scrap = settlement
            .producer_schedule()
            .iter()
            .filter(|job| job.enqueued_at > context.obs.tick)
            .map(|job| job.current_scrap)
            .sum();
        out.intents.extend(
            settlement
                .producer_schedule()
                .iter()
                .filter(|job| job.enqueued_at == context.obs.tick)
                .map(|job| Intent::TrainAt {
                    building: job.producer,
                    kind: job.kind,
                }),
        );
    }
}

fn active_obligation(
    planner: &mut StrategicPlanner,
    obs: &Observation,
) -> Option<ActiveConnectedObligation> {
    let fixture_planning = crate::planning::PlanningWork::default();

    let resources = ResourceSnapshot::from_observation(obs);
    let intel = knowledge(obs);
    planner.active_connected_obligation(FreshConnectedProposalRequest::new(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        obs,
        &resources,
        &intel,
        HOME,
        coordination(&fixture_planning, None),
    ))
}

fn settle_active(
    planner: &mut StrategicPlanner,
    obs: &Observation,
) -> Vec<crate::allocation::ScheduledProducerJob> {
    use crate::allocation::{
        AllocationPersonality, CrossDomainAllocation, active_connected_obligation,
    };
    let obligation = active_obligation(planner, obs).unwrap();
    let resources = ResourceSnapshot::from_observation(obs);
    let mut allocation = CrossDomainAllocation::new(&resources, obligation.deadline(), 12).unwrap();
    allocation.import(active_connected_obligation(&obligation));
    allocation
        .resolve(AllocationPersonality::default(), None)
        .unwrap()
        .producer_schedule()
        .to_vec()
}

#[test]
fn paid_connected_queue_keeps_duplicates_and_releases_completed_history() {
    let mut obs = developed_connected_obs(120);
    obs.my_units.retain(|unit| unit.kind == UnitKind::Sentinel);
    obs.my_queue_progress = vec![0; obs.my_buildings.len()];
    let mut planner = with_operation(AirOperationPhase::Recon, obs.tick);
    let active = planner.air.as_mut().unwrap();
    active.op.scout = None;
    active.op.artillery.clear();
    active.op.strike_aircraft.clear();
    active.plan.connected_package.as_mut().unwrap().suppression[0].count = 2;
    let kind = UnitKind::Bombard;
    let ready = obs.tick + Tick::from(kind.stats().train_ticks) - 1;
    let purchases = vec![
        ConnectedPurchase {
            producer: BuildingId(12),
            kind,
            issued_at: obs.tick,
            ready_at: ready,
            delayed: false,
        },
        ConnectedPurchase {
            producer: BuildingId(12),
            kind,
            issued_at: obs.tick,
            ready_at: ready + Tick::from(kind.stats().train_ticks),
            delayed: false,
        },
    ];
    active.plan.paid_connected_production = purchases.clone();
    assert_eq!(planner.paid_connected_production(&obs), purchases);
    obs.my_queues[2] = vec![kind, kind];
    obs.tick = ready;
    obs.my_queue_progress[2] = kind.stats().train_ticks - 1;
    assert_eq!(planner.paid_connected_production(&obs), purchases);
    obs.tick += 1;
    obs.my_queues[2] = vec![kind];
    obs.my_queue_progress[2] = 0;
    assert_eq!(planner.paid_connected_production(&obs), purchases[1..]);
    obs.tick = purchases[1].ready_at + 1;
    assert!(planner.paid_connected_production(&obs).is_empty());
    obs.tick += 12;
    assert!(
        planner.paid_connected_production(&obs).is_empty(),
        "later ordinary work cannot resurrect released ownership"
    );
}

#[test]
fn blocked_paid_connected_queue_remains_owned_after_predicted_completion() {
    let mut obs = developed_connected_obs(120);
    obs.my_units.retain(|unit| unit.kind == UnitKind::Sentinel);
    let mut planner = with_operation(AirOperationPhase::Recon, obs.tick);
    let active = planner.air.as_mut().unwrap();
    active.op.artillery.clear();
    let kind = UnitKind::Bombard;
    active
        .plan
        .paid_connected_production
        .push(ConnectedPurchase {
            producer: BuildingId(12),
            kind,
            issued_at: obs.tick - 12,
            ready_at: obs.tick - 1,
            delayed: false,
        });
    obs.my_queues[2] = vec![kind];
    obs.my_queue_progress = vec![0; obs.my_buildings.len()];
    obs.my_queue_progress[2] = kind.stats().train_ticks;
    assert_eq!(planner.paid_connected_production(&obs).len(), 1);
    obs.tick += 12;
    assert_eq!(planner.paid_connected_production(&obs).len(), 1);
    obs.my_queues[2].clear();
    assert!(planner.paid_connected_production(&obs).is_empty());
    obs.my_queues[2] = vec![kind];
    assert!(planner.paid_connected_production(&obs).is_empty());
}

#[test]
fn unpaid_connected_demand_reassigns_factory_without_extending_deadline() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut obs = production_hungry_connected_obs(120, 10_000);
    obs.my_buildings.push(building(
        13,
        0,
        BuildingKind::Airworks,
        TilePos::new(12, 2),
        true,
    ));
    obs.my_queues.push(Vec::new());
    let mut planner = StrategicPlanner::new();
    let proposal = planner
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &obs,
                &ResourceSnapshot::from_observation(&obs),
                &knowledge(&obs),
                HOME,
                coordination(&fixture_planning, None),
            ),
        )
        .unwrap()
        .unwrap();
    let deadline = proposal.deadline();
    planner.commit_connected(proposal);
    let initial = settle_active(&mut planner, &obs);
    let lost = initial
        .iter()
        .find(|job| job.kind.stats().domain == Domain::Air)
        .unwrap()
        .producer;
    let index = obs
        .my_buildings
        .iter()
        .position(|building| building.id == lost)
        .unwrap();
    obs.my_buildings.remove(index);
    obs.my_queues.remove(index);
    obs.tick += 12;
    let replacement = settle_active(&mut planner, &obs);
    assert!(!replacement.is_empty());
    assert!(replacement.iter().all(|job| job.producer != lost
        && job.ready_before == deadline
        && job.ready_at < deadline));
    assert_eq!(planner.air_admitted_at(), Some(120));
}

#[test]
fn unpaid_connected_demand_can_buy_earlier_and_does_not_expire_after_rollback() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut obs = production_hungry_connected_obs(120, 10_000);
    add_renewable_economy(&mut obs, 1);
    let mut planner = StrategicPlanner::new();
    let proposal = planner
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &obs,
                &ResourceSnapshot::from_observation(&obs),
                &knowledge(&obs),
                HOME,
                coordination(&fixture_planning, None),
            ),
        )
        .unwrap()
        .unwrap();
    planner.commit_connected(proposal);
    let bank = obs.scrap;
    let rich = settle_active(&mut planner, &obs);
    obs.scrap = rich.iter().map(|job| job.kind.stats().cost).sum::<u32>() - 1;
    let forecast = settle_active(&mut planner, &obs);
    assert!(forecast.iter().any(|job| job.enqueued_at > obs.tick));
    obs.scrap = bank;
    let initial = settle_active(&mut planner, &obs);
    assert!(
        initial
            .iter()
            .zip(&forecast)
            .any(|(earlier, old)| earlier.enqueued_at < old.enqueued_at)
    );
    assert!(
        planner.paid_connected_production(&obs).is_empty(),
        "quotations create no paid ownership"
    );
    obs.tick += 12;
    let retry = settle_active(&mut planner, &obs);
    assert!(retry.iter().any(|job| job.enqueued_at == obs.tick));
    assert_eq!(initial.len(), retry.len());
    assert!(
        retry
            .iter()
            .all(|job| job.ready_before == initial[0].ready_before)
    );
    planner.record_connected_purchases(&retry, obs.tick);
    assert_eq!(
        planner.paid_connected_production(&obs).len(),
        retry
            .iter()
            .filter(|job| job.enqueued_at == obs.tick)
            .count()
    );
}
#[test]
fn paid_connected_ownership_survives_revision_and_completion_does_not_repurchase() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut obs = production_hungry_connected_obs(120, 10_000);
    let mut planner = StrategicPlanner::new();
    let identity = profile();
    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    let proposal = planner
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &identity,
                tuning,
                &obs,
                &ResourceSnapshot::from_observation(&obs),
                &knowledge(&obs),
                HOME,
                coordination(&fixture_planning, None),
            ),
        )
        .unwrap()
        .unwrap();
    let deadline = proposal.deadline();
    planner.commit_connected(proposal);
    let schedule = settle_active(&mut planner, &obs);
    planner.record_connected_purchases(&schedule, obs.tick);
    let paid = planner.paid_connected_production(&obs);
    assert!(!paid.is_empty());
    for purchase in &paid {
        let index = obs
            .my_buildings
            .iter()
            .position(|b| b.id == purchase.producer)
            .unwrap();
        obs.my_queues[index].push(purchase.kind);
        obs.scrap -= purchase.kind.stats().cost;
    }
    obs.tick += 12;
    obs.my_queue_progress = obs
        .my_queues
        .iter()
        .map(|queue| if queue.is_empty() { 0 } else { 12 })
        .collect();
    assert_eq!(planner.paid_connected_production(&obs), paid);
    let revision = planner
        .active_connected_revision_proposal(FreshConnectedProposalRequest::new(
            &identity,
            tuning,
            &obs,
            &ResourceSnapshot::from_observation(&obs),
            &knowledge(&obs),
            HOME,
            coordination(&fixture_planning, None),
        ))
        .unwrap()
        .unwrap();
    assert_eq!(revision.deadline(), deadline);
    planner.commit_connected(revision);
    assert_eq!(planner.paid_connected_production(&obs), paid);
    let before = active_obligation(&mut planner, &obs)
        .unwrap()
        .provider_jobs()
        .iter()
        .map(ConnectedProviderJob::kind)
        .collect::<Vec<_>>();
    obs.tick = (paid.iter().map(|purchase| purchase.ready_at).max().unwrap() + 1).div_ceil(12) * 12;
    for (index, purchase) in paid.iter().enumerate() {
        obs.my_units.push(own(
            1000 + u32::try_from(index).unwrap(),
            purchase.kind,
            HOME,
        ));
    }
    obs.my_queues.iter_mut().for_each(Vec::clear);
    obs.my_queue_progress.fill(0);
    let after = active_obligation(&mut planner, &obs).unwrap();
    assert_eq!(after.deadline(), deadline);
    assert_eq!(
        after
            .provider_jobs()
            .iter()
            .map(ConnectedProviderJob::kind)
            .collect::<Vec<_>>(),
        before
    );
    after.membership.apply(&mut planner, obs.tick);
    assert!(planner.paid_connected_production(&obs).is_empty());
    assert!(
        planner
            .recover_unpaid_connected_for_economy_emergency(EconomyEmergencyRecovery {
                profile: &identity,
                tuning,
                obs: &obs,
                home: HOME,
                public_map: None,
                orientation: test_orientation(),
                recon_paid_exclusions: &[],
            })
            .is_none()
    );
}

/// An admitted connected operation with its whole package still unpaid,
/// plus the provider jobs that demand covers.
fn unpaid_connected_operation(obs: &Observation) -> (StrategicPlanner, Vec<ConnectedProviderJob>) {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut planner = StrategicPlanner::new();
    let proposal = planner
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                obs,
                &ResourceSnapshot::from_observation(obs),
                &knowledge(obs),
                HOME,
                coordination(&fixture_planning, None),
            ),
        )
        .unwrap()
        .unwrap();
    let jobs = proposal.minimum_claims().provider_jobs().to_vec();
    planner.commit_connected(proposal);
    assert!(
        planner.paid_connected_production(obs).is_empty(),
        "the fixture buys nothing, so the package is wholly outstanding"
    );
    assert!(!jobs.is_empty(), "the fixture must leave unpaid demand");
    (planner, jobs)
}

/// Queues one already-paid unit per outstanding provider job, so the
/// operation needs no further scrap without owning any of that work.
fn queue_foreign_paid_providers(
    obs: &mut Observation,
    jobs: &[ConnectedProviderJob],
) -> Vec<(BuildingId, UnitKind, usize)> {
    let mut occurrences = Vec::new();
    for job in jobs {
        let producer = job.eligible_producers()[0];
        let index = obs
            .my_buildings
            .iter()
            .position(|building| building.id == producer)
            .unwrap();
        occurrences.push((producer, job.kind(), obs.my_queues[index].len()));
        obs.my_queues[index].push(job.kind());
    }
    occurrences
}

#[test]
fn economy_recovery_recalls_an_operation_that_still_needs_scrap() {
    let obs = production_hungry_connected_obs(120, 10_000);
    let (mut planner, _) = unpaid_connected_operation(&obs);

    assert!(
        planner
            .recover_unpaid_connected_for_economy_emergency(EconomyEmergencyRecovery {
                profile: &profile(),
                tuning: DifficultyTuning::for_level(BotDifficulty::Prime),
                obs: &obs,
                home: HOME,
                public_map: None,
                orientation: test_orientation(),
                recon_paid_exclusions: &[],
            })
            .is_some()
    );
}

#[test]
fn economy_recovery_keeps_an_operation_a_foreign_paid_queue_already_covers() {
    let mut obs = production_hungry_connected_obs(120, 10_000);
    let (mut planner, jobs) = unpaid_connected_operation(&obs);
    queue_foreign_paid_providers(&mut obs, &jobs);

    assert!(
        planner
            .recover_unpaid_connected_for_economy_emergency(EconomyEmergencyRecovery {
                profile: &profile(),
                tuning: DifficultyTuning::for_level(BotDifficulty::Prime),
                obs: &obs,
                home: HOME,
                public_map: None,
                orientation: test_orientation(),
                recon_paid_exclusions: &[],
            })
            .is_none(),
        "queue work another program paid for needs no further scrap"
    );
}

#[test]
fn economy_recovery_refuses_paid_queue_work_reconnaissance_holds() {
    let mut obs = production_hungry_connected_obs(120, 10_000);
    let (mut planner, jobs) = unpaid_connected_operation(&obs);
    let held = queue_foreign_paid_providers(&mut obs, &jobs);

    assert!(
        planner
            .recover_unpaid_connected_for_economy_emergency(EconomyEmergencyRecovery {
                profile: &profile(),
                tuning: DifficultyTuning::for_level(BotDifficulty::Prime),
                obs: &obs,
                home: HOME,
                public_map: None,
                orientation: test_orientation(),
                recon_paid_exclusions: &held,
            })
            .is_some(),
        "an occurrence another claimant owns cannot satisfy this demand"
    );
}

fn test_orientation() -> Orientation {
    Orientation::for_home(&obs(0), TilePos::new(0, 0))
}

#[test]
fn output_and_persistence_are_deterministic() {
    let mut obs = obs(100);
    see_approach(&mut obs);
    let intel = knowledge(&obs);
    let mut first = with_operation(AirOperationPhase::Verify, 100);
    let mut second = first.clone();
    assert_eq!(
        think(&mut first, &obs, &intel),
        think(&mut second, &obs, &intel)
    );
    assert_eq!(first, second);
}

#[test]
fn active_operation_keeps_members_already_claimed_by_the_coordinator() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = obs(100);
    see_approach(&mut observation);
    let intelligence = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Recon, observation.tick);
    let owned = planner
        .air
        .as_ref()
        .map(|active| reservations(&active.op, &active.plan, &observation))
        .expect("the fixture has an active operation");

    let decision = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            StrategicCoordination {
                planning: Some(&fixture_planning),
                enlisted: &owned,
                ..coordination(&fixture_planning, None)
            },
        )
        .decision;

    let operation = planner
        .air_operation()
        .expect("the active operation remains owned");
    assert_eq!(operation.scout, Some(UnitId(1)));
    assert_eq!(operation.artillery, [UnitId(2)]);
    assert_eq!(operation.strike_aircraft, [UnitId(3), UnitId(4)]);
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Kestrel | UnitKind::Bombard | UnitKind::Condor,
            ..
        }
    )));
}

#[test]
fn provider_visible_on_the_deadline_can_complete_assembly() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = obs(2_500);
    see_approach(&mut observation);
    observation.explored.fill(true);
    let intelligence = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Assemble, observation.tick);
    let active = planner.air.as_mut().expect("the fixture has an operation");
    active.op.strike_aircraft.pop();
    let package = active
        .plan
        .connected_package
        .as_mut()
        .expect("the fixture has a connected package");
    package.derived_at = observation.tick - 1;
    package.preparation_deadline = observation.tick;
    active.plan.admitted_at = observation.tick - CONNECTED_PREPARATION_HORIZON;
    active.plan.assembly_timeout = CONNECTED_PREPARATION_HORIZON;
    active.op.started_at = observation.tick - CONNECTED_PREPARATION_HORIZON;
    active.op.phase_started_at = observation.tick - CONNECTED_PREPARATION_HORIZON;

    let decision = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination(&fixture_planning, None),
        )
        .decision;

    let operation = planner
        .air_operation()
        .expect("the complete package crosses the commitment boundary");
    assert_eq!(operation.phase(), AirOperationPhase::SuppressAa);
    assert_eq!(operation.membership_frozen_at, Some(observation.tick));
    assert_eq!(operation.strike_aircraft, [UnitId(3), UnitId(4)]);
    assert_ne!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::Timeout)
    );
    assert!(decision.reservations.contains(&UnitId(4)));
}

#[test]
fn provider_first_visible_on_the_deadline_survives_the_recon_transition() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = obs(2_500);
    see_approach(&mut observation);
    observation.explored.fill(true);
    let mut intelligence = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Recon, observation.tick);
    let active = planner.air.as_mut().expect("the fixture has an operation");
    active.op.strike_aircraft.pop();
    let package = active
        .plan
        .connected_package
        .as_mut()
        .expect("the fixture has a connected package");
    package.derived_at = observation.tick - 1;
    package.preparation_deadline = observation.tick;
    active.plan.admitted_at = observation.tick - CONNECTED_PREPARATION_HORIZON;
    active.plan.assembly_timeout = CONNECTED_PREPARATION_HORIZON;
    active.op.started_at = observation.tick - CONNECTED_PREPARATION_HORIZON;
    active.op.phase_started_at = observation.tick - CONNECTED_PREPARATION_HORIZON;

    let deadline_decision = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination(&fixture_planning, None),
        )
        .decision;

    let operation = planner
        .air_operation()
        .expect("a complete deadline roster is retained after reconnaissance");
    assert_eq!(operation.phase(), AirOperationPhase::Assemble);
    assert_eq!(operation.strike_aircraft, [UnitId(3), UnitId(4)]);
    assert_ne!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::Timeout)
    );
    assert!(deadline_decision.reservations.contains(&UnitId(4)));

    observation.tick += DifficultyTuning::for_level(BotDifficulty::Prime).cadence;
    intelligence.update(&observation);
    let committed = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination(&fixture_planning, None),
        )
        .decision;

    let operation = planner
        .air_operation()
        .expect("the ready roster crosses commitment on the next decision boundary");
    assert_eq!(operation.phase(), AirOperationPhase::SuppressAa);
    assert_eq!(operation.membership_frozen_at, Some(observation.tick));
    assert!(committed.reservations.contains(&UnitId(4)));
}

#[test]
fn suppression_commitment_tick_is_recorded_once_and_survives_recovery() {
    let mut operation = operation(AirOperationPhase::Assemble, 100);
    assert_eq!(operation.membership_frozen_at, None);

    enter(&mut operation, AirStage::SuppressAa, 120);
    assert_eq!(operation.membership_frozen_at, Some(120));
    enter(&mut operation, AirStage::Verify, 130);
    enter(&mut operation, AirStage::SuppressAa, 140);
    assert_eq!(operation.membership_frozen_at, Some(120));

    recover(&mut operation, AirRecoveryReason::Timeout, 150);
    assert_eq!(operation.membership_frozen_at, Some(120));
}

#[test]
fn closed_admission_blocks_a_new_air_plan_but_an_active_plan_reaches_strike() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let eligible = wealthy_island_obs(
        super::super::difficulty::strategic_admission_at_or_after(5_000),
        2,
    );
    let eligible_intelligence = knowledge(&eligible);
    let mut blocked = StrategicPlanner::new();

    assert_eq!(
        blocked
            .think_alone_with(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &eligible,
                &eligible_intelligence,
                HOME,
                StrategicCoordination {
                    planning: Some(&fixture_planning),
                    enlisted: &[],
                    lift_support: None,
                    allow_new_operation: false,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                    public_map: None,
                    orientation: test_orientation(),
                },
            )
            .decision,
        StrategicDecision::default()
    );
    assert!(blocked.air_operation().is_none());

    let mut battle = obs(100);
    see_approach(&mut battle);
    see_building_footprint(&mut battle, TARGET, BuildingKind::Crucible);
    let intelligence = knowledge(&battle);
    let mut active = with_operation(AirOperationPhase::Verify, battle.tick);
    let continued = active
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intelligence,
            HOME,
            StrategicCoordination {
                planning: Some(&fixture_planning),
                enlisted: &[],
                lift_support: None,
                allow_new_operation: false,
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
                public_map: None,
                orientation: test_orientation(),
            },
        )
        .decision;

    assert_eq!(
        active.air_operation().map(|operation| operation.phase()),
        Some(AirOperationPhase::Strike)
    );
    assert_eq!(continued.committed_scrap(), 0);
    assert_eq!(
        continued.reservations,
        [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
    );
    assert!(
        continued
            .intents
            .iter()
            .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
    );
    let mut incomplete = obs(200);
    incomplete.scrap = 50_000;
    incomplete.my_units.retain(|unit| unit.id == UnitId(1));
    incomplete.my_buildings = vec![
        building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
        building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
        building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
    ];
    incomplete.my_queues = vec![Vec::new(); incomplete.my_buildings.len()];
    see_approach(&mut incomplete);
    let incomplete_intelligence = knowledge(&incomplete);
    let mut assembling = with_operation(AirOperationPhase::Assemble, incomplete.tick);
    let held = assembling
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &incomplete,
            &incomplete_intelligence,
            HOME,
            StrategicCoordination {
                planning: Some(&fixture_planning),
                enlisted: &[],
                lift_support: None,
                allow_new_operation: false,
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
                public_map: None,
                orientation: test_orientation(),
            },
        )
        .decision;
    assert_eq!(held.committed_scrap(), 0);
    assert!(
        held.intents
            .iter()
            .all(|intent| !matches!(intent, Intent::TrainAt { .. })),
        "an active incomplete operation cannot replenish while spending is closed: {held:?}"
    );
    assert!(assembling.air_operation().is_some());

    let mut damaged = obs(300);
    damaged
        .my_units
        .retain(|unit| !matches!(unit.id, UnitId(3) | UnitId(4)));
    let damaged_intelligence = knowledge(&damaged);
    let mut recovering = with_operation(AirOperationPhase::SuppressAa, damaged.tick);
    let retreat = recovering
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &damaged,
            &damaged_intelligence,
            HOME,
            StrategicCoordination {
                planning: Some(&fixture_planning),
                enlisted: &[],
                lift_support: None,
                allow_new_operation: false,
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
                public_map: None,
                orientation: test_orientation(),
            },
        )
        .decision;
    assert!(
        retreat
            .intents
            .iter()
            .any(|intent| matches!(intent, Intent::MoveUnits { .. })),
        "closing purchases must preserve the active operation's recovery order: {retreat:?}"
    );
}

#[test]
fn losing_a_dispatched_recon_scout_recovers_without_claiming_a_replacement() {
    let mut observation = obs(108);
    observation.my_units.remove(0);
    observation
        .my_units
        .push(own(5, UnitKind::Kestrel, TilePos::new(5, 10)));
    let intel = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Recon, 108);
    planner.air_op_mut().unwrap().scout_dispatch = Some((UnitId(1), TARGET));

    let decision = think(&mut planner, &observation, &intel);

    let operation = planner
        .air_operation()
        .expect("the lost-scout operation withdraws before closing");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::RequiredUnitLost)
    );
    assert!(planner.cooldown_until > observation.tick);
    assert_eq!(decision.reservations, [UnitId(2), UnitId(3), UnitId(4)]);
    assert!(!decision.reservations.contains(&UnitId(5)));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, .. } if units.contains(&UnitId(5))
    )));
}

#[test]
fn losing_a_dispatched_recon_scout_releases_its_factory_bank() {
    let mut observation = obs(108);
    observation.my_units.remove(0);
    observation.my_buildings = vec![building(
        10,
        0,
        BuildingKind::Airworks,
        TilePos::new(2, 2),
        true,
    )];
    observation.my_queues = vec![Vec::new()];
    observation.scrap = UnitKind::Kestrel.stats().cost;
    let intel = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Recon, 108);
    planner.air_op_mut().unwrap().scout_dispatch = Some((UnitId(1), TARGET));

    let decision = think(&mut planner, &observation, &intel);

    let operation = planner
        .air_operation()
        .expect("the lost-scout operation withdraws before closing");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::RequiredUnitLost)
    );
    assert_eq!(decision.committed_scrap(), 0);
    assert!(
        decision
            .intents
            .iter()
            .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
    );
}

#[test]
fn a_naturally_dispatched_scout_loss_completes_recovery_before_using_a_replacement() {
    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    let mut observation = wealthy_island_obs(
        super::super::difficulty::strategic_admission_at_or_after(5_000),
        1,
    );
    observation.my_units[0].tile = HOME;
    observation
        .my_units
        .push(own(17, UnitKind::Kestrel, HOME.offset(1, 0)));
    let mut intelligence = knowledge(&observation);
    let mut planner = StrategicPlanner::new();

    let dispatched = think(&mut planner, &observation, &intelligence);
    let operation = planner
        .air_operation()
        .expect("the wealthy island starts a real air operation");
    assert_eq!(operation.phase(), AirOperationPhase::Assemble);
    assert_eq!(operation.scout, Some(UnitId(1)));
    assert!(operation.scout_dispatch.is_some());
    assert!(dispatched.intents.iter().any(|intent| matches!(
        intent,
        Intent::MoveUnits { units, .. } if units == &[UnitId(1)]
    )));
    assert!(
        dispatched.committed_scrap() > 0,
        "the live operation must own a real factory bank before the loss"
    );

    observation.tick += 1;
    observation.my_units.retain(|unit| unit.id != UnitId(1));
    intelligence.update(&observation);
    let failed = think(&mut planner, &observation, &intelligence);
    let operation = planner
        .air_operation()
        .expect("the failed operation remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::RequiredUnitLost)
    );
    assert_eq!(failed.committed_scrap(), 0);
    assert!(!failed.reservations.contains(&UnitId(17)));
    assert_eq!(
        failed
            .intents
            .iter()
            .filter(|intent| matches!(
                intent,
                Intent::MoveUnits { units, goal }
                    if units == &failed.reservations && *goal == HOME
            ))
            .count(),
        1,
        "surviving claimed units receive one recovery order"
    );
    assert!(failed.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, .. } if units.contains(&UnitId(17))
    )));
    assert!(failed.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Kestrel,
            ..
        }
    )));
    let cooldown_until = planner.cooldown_until;
    assert_eq!(
        cooldown_until,
        observation
            .tick
            .saturating_add(cooldown(&profile(), tuning))
    );

    observation.tick += 1;
    intelligence.update(&observation);
    let recovered = think(&mut planner, &observation, &intelligence);
    assert!(
        recovered.intents.is_empty(),
        "the return order is sent once"
    );
    assert!(planner.air_operation().is_none());
    assert_eq!(planner.terminal_outcome(), None);
    let standby = planner.standby.reservations();
    assert!(!standby.is_empty());
    assert!(!standby.contains(&UnitId(17)));

    observation.tick += 1;
    intelligence.update(&observation);
    let cooling_down = think(&mut planner, &observation, &intelligence);
    assert!(cooling_down.intents.is_empty());
    assert_eq!(cooling_down.reservations, standby);
    assert!(planner.air_operation().is_none());
    assert_eq!(planner.terminal_outcome(), None);

    observation.tick = super::super::difficulty::strategic_admission_at_or_after(cooldown_until);
    assert!(observation.tick >= cooldown_until);
    assert!(
        observation.tick
            < cooldown_until.saturating_add(super::super::difficulty::STRATEGIC_ADMISSION_CADENCE)
    );
    intelligence.update(&observation);
    let retried = think(&mut planner, &observation, &intelligence);
    let retry = planner
        .air_operation()
        .expect("a fresh operation may use the replacement after cooldown");
    assert_eq!(retry.scout, Some(UnitId(17)));
    assert!(retry.scout_dispatch.is_some());
    assert!(
        standby
            .iter()
            .all(|unit| retry.strike_aircraft.contains(unit))
    );
    assert!(retried.intents.iter().any(|intent| matches!(
        intent,
        Intent::MoveUnits { units, .. } if units == &[UnitId(17)]
    )));
}

#[test]
fn scout_dispatch_retargets_only_when_its_safe_goal_changes() {
    let mut observation = obs(100);
    observation.my_units[0].tile = TilePos::new(4, 10);
    observation.my_units[0].idle = false;
    let intel = knowledge(&observation);
    let mut operation = operation(AirOperationPhase::Recon, 100);
    let plan = AirPlan::remembered_connected(&observation);

    let mut first = StrategicDecision::default();
    assert!(dispatch_scout(
        &mut operation,
        &plan,
        &observation,
        &intel,
        &[],
        None,
        &mut first
    ));
    let first_goal = match first.intents.as_slice() {
        [Intent::MoveUnits { units, goal }] if units == &[UnitId(1)] => *goal,
        intents => panic!("expected one scout dispatch, got {intents:?}"),
    };

    let mut repeated = StrategicDecision::default();
    assert!(dispatch_scout(
        &mut operation,
        &plan,
        &observation,
        &intel,
        &[],
        None,
        &mut repeated
    ));
    assert!(
        repeated.intents.is_empty(),
        "an identical in-flight order remains authoritative"
    );

    operation.target = TilePos::new(24, 16);
    let mut changed = StrategicDecision::default();
    assert!(dispatch_scout(
        &mut operation,
        &plan,
        &observation,
        &intel,
        &[],
        None,
        &mut changed
    ));
    assert!(matches!(
        changed.intents.as_slice(),
        [Intent::MoveUnits { units, goal }]
            if units == &[UnitId(1)] && *goal != first_goal
    ));
}

#[test]
fn strike_hold_is_dispatched_once_until_home_changes() {
    let observation = obs(100);
    let mut operation = operation(AirOperationPhase::SuppressAa, 100);
    let pad = landing_pad(&observation, HOME).expect("open ground rings the home anchor");
    assert_eq!(
        pad,
        HOME.offset(-2, -2),
        "the pad is the first ring-two tile by (y, x)"
    );

    let mut first = StrategicDecision::default();
    hold_strike_aircraft(&mut operation, &observation, HOME, &mut first);
    assert_eq!(
        first.intents,
        [Intent::MoveUnits {
            units: vec![UnitId(3), UnitId(4)],
            goal: pad,
        }]
    );
    assert_eq!(operation.strike_hold, Some(pad));

    let mut repeated = StrategicDecision::default();
    hold_strike_aircraft(&mut operation, &observation, HOME, &mut repeated);
    assert!(
        repeated.intents.is_empty(),
        "the stable hold remains authoritative"
    );

    let replacement_home = HOME.offset(2, 1);
    let replacement_pad = landing_pad(&observation, replacement_home).unwrap();
    assert_ne!(replacement_pad, pad);
    let mut redirected = StrategicDecision::default();
    hold_strike_aircraft(
        &mut operation,
        &observation,
        replacement_home,
        &mut redirected,
    );
    assert_eq!(
        redirected.intents,
        [Intent::MoveUnits {
            units: vec![UnitId(3), UnitId(4)],
            goal: replacement_pad,
        }]
    );

    let mut replacement_repeated = StrategicDecision::default();
    hold_strike_aircraft(
        &mut operation,
        &observation,
        replacement_home,
        &mut replacement_repeated,
    );
    assert!(replacement_repeated.intents.is_empty());
}

#[test]
fn landing_pad_skips_footprints_known_rock_and_the_map_edge() {
    let mut observation = obs(100);
    let foundry_size = BuildingKind::Foundry.base_stats().size;
    let inside = |tile: TilePos| {
        tile.x >= HOME.x
            && tile.x < HOME.x + foundry_size.0
            && tile.y >= HOME.y
            && tile.y < HOME.y + foundry_size.1
    };
    observation.my_buildings = vec![building(20, 0, BuildingKind::Foundry, HOME, true)];
    observation.my_queues = vec![Vec::new()];
    // Rock every ring-two tile outside the footprint so the only
    // ring-two candidates left are under the Foundry itself.
    observation.known_rock = (-2..=2)
        .flat_map(|dy| (-2..=2).map(move |dx| HOME.offset(dx, dy)))
        .filter(|tile| tile.chebyshev(HOME) == 2 && !inside(*tile))
        .collect();
    observation.known_rock.sort_by_key(|tile| (tile.y, tile.x));

    let pad = landing_pad(&observation, HOME).expect("ring three is open");
    assert!(!inside(pad), "the pad must never sit under the Foundry");
    assert!(!observation.known_rock_at(pad));
    assert_eq!(pad.chebyshev(HOME), 3);
    assert_eq!(pad, HOME.offset(-3, -3));

    let corner = TilePos::new(1, 1);
    let corner_pad = landing_pad(&obs(100), corner).expect("the corner has open ground");
    assert!(corner_pad.x >= 0 && corner_pad.y >= 0);
    assert_eq!(corner_pad, TilePos::new(3, 0));

    let mut sealed = obs(100);
    sealed.known_rock = (0..sealed.map_height)
        .flat_map(|y| (0..sealed.map_width).map(move |x| TilePos::new(x, y)))
        .collect();
    assert_eq!(landing_pad(&sealed, HOME), None);
}

#[test]
fn airborne_hold_lands_the_bombers_and_keeps_a_fixed_wing_screen_over_home() {
    let mut observation = wealthy_island_obs(100, 1);
    observation
        .my_units
        .push(own(30, UnitKind::Buzzard, TilePos::new(5, 8)));
    let mut plan = AirPlan::island(&profile(), &observation);
    plan.screen = vec![UnitId(30)];
    let mut operation = operation(AirOperationPhase::SuppressAa, 100);
    let pad = landing_pad(&observation, HOME).unwrap();

    let mut held = StrategicDecision::default();
    hold_air_strike(&mut operation, &plan, &observation, HOME, &mut held);
    assert_eq!(
        held.intents,
        [
            Intent::MoveUnits {
                units: vec![UnitId(3), UnitId(4)],
                goal: pad,
            },
            Intent::MoveUnits {
                units: vec![UnitId(30)],
                goal: HOME,
            },
        ]
    );
    assert_eq!(operation.strike_hold, Some(pad));

    let mut repeated = StrategicDecision::default();
    hold_air_strike(&mut operation, &plan, &observation, HOME, &mut repeated);
    assert!(repeated.intents.is_empty());
}

#[test]
fn artillery_staging_is_dispatched_once_until_the_goal_or_mission_changes() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut operation = operation(AirOperationPhase::Strike, 100);
    let first_goal = TilePos::new(12, 10);

    let mut first = StrategicDecision::default();
    stage_artillery(&mut operation, first_goal, &mut first);
    assert_eq!(
        first.intents,
        [Intent::MoveUnits {
            units: vec![UnitId(2)],
            goal: first_goal,
        }]
    );

    let mut repeated = StrategicDecision::default();
    stage_artillery(&mut operation, first_goal, &mut repeated);
    assert!(
        repeated.intents.is_empty(),
        "the stable staging move remains authoritative"
    );

    let replacement_goal = first_goal.offset(1, -2);
    let mut redirected = StrategicDecision::default();
    stage_artillery(&mut operation, replacement_goal, &mut redirected);
    assert_eq!(
        redirected.intents,
        [Intent::MoveUnits {
            units: vec![UnitId(2)],
            goal: replacement_goal,
        }]
    );

    let mut replacement_repeated = StrategicDecision::default();
    stage_artillery(&mut operation, replacement_goal, &mut replacement_repeated);
    assert!(replacement_repeated.intents.is_empty());

    let mut suppression_observation = obs(100);
    suppression_observation.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(20, 10),
        true,
    ));
    let intelligence = knowledge(&suppression_observation);
    let mut suppression = StrategicDecision::default();
    let identity = profile();
    let mut plan = connected_test_plan(&suppression_observation);
    let context = AirPlanningContext {
        allow_procurement: true,
        planning: Some(&fixture_planning),
        profile: &identity,
        tuning: DifficultyTuning::for_level(BotDifficulty::Prime),
        obs: &suppression_observation,
        intel: &intelligence,
        home: HOME,
        orientation: test_orientation(),
        public_map: None,
        enlisted: &[],
        landing_sites: &[],
        connected_resources: None,
        production: StrategicProductionContext::empty(),
        protected_current_scrap: 0,
        protected_forecast_scrap: 0,
    };
    suppress(&mut operation, &mut plan, &context, &mut suppression);
    assert!(suppression.intents.iter().any(|intent| matches!(
        intent,
        Intent::AttackUnits {
            units,
            target: Target::Building(BuildingId(81)),
        } if units == &[UnitId(2)]
    )));

    let mut restaged = StrategicDecision::default();
    stage_artillery(&mut operation, replacement_goal, &mut restaged);
    assert_eq!(
        restaged.intents,
        [Intent::MoveUnits {
            units: vec![UnitId(2)],
            goal: replacement_goal,
        }],
        "an artillery attack replaces the prior staging order"
    );

    let mut restaged_repeated = StrategicDecision::default();
    stage_artillery(&mut operation, replacement_goal, &mut restaged_repeated);
    assert!(restaged_repeated.intents.is_empty());
}

#[test]
fn a_late_recon_scout_receives_a_fresh_flight_window() {
    let seen = obs(990);
    let mut intel = knowledge(&seen);
    let mut waiting = obs(1_008);
    waiting.enemy_buildings[0].seen = false;
    waiting.my_units.remove(0);
    waiting.my_buildings = vec![building(
        10,
        0,
        BuildingKind::Airworks,
        TilePos::new(2, 2),
        true,
    )];
    waiting.my_queues = vec![vec![UnitKind::Kestrel]];
    intel.update(&waiting);
    let mut planner = with_operation(AirOperationPhase::Recon, 100);
    let operation = planner.air_op_mut().unwrap();
    operation.started_at = 100;
    operation.phase_started_at = 100;

    let before_scout = think(&mut planner, &waiting, &intel);
    assert!(before_scout.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, .. } if units.contains(&UnitId(1))
    )));
    assert_eq!(
        planner.air_operation().unwrap().phase(),
        AirOperationPhase::Recon
    );

    waiting.tick = 1_020;
    waiting
        .my_units
        .push(own(5, UnitKind::Kestrel, TilePos::new(4, 10)));
    waiting.my_queues[0].clear();
    intel.update(&waiting);
    let assigned = think(&mut planner, &waiting, &intel);
    let operation = planner.air_operation().unwrap();
    assert_eq!(operation.phase(), AirOperationPhase::Recon);
    assert_eq!(operation.phase_started_at, waiting.tick);
    assert_eq!(operation.scout, Some(UnitId(5)));
    assert!(assigned.intents.iter().any(|intent| matches!(
        intent,
        Intent::MoveUnits { units, .. } if units == &[UnitId(5)]
    )));
}

#[test]
fn current_sight_cannot_skip_recon_while_the_required_scout_trains() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    let mut current = obs(240);
    current.explored.fill(true);
    current.my_units.retain(|unit| unit.id != UnitId(1));
    current.my_buildings = vec![building(
        10,
        0,
        BuildingKind::Airworks,
        TilePos::new(2, 2),
        true,
    )];
    current.my_queues = vec![Vec::new()];
    current.scrap = UnitKind::Kestrel.stats().cost;
    let mut intelligence = knowledge(&current);
    let mut planner = with_operation(AirOperationPhase::Recon, current.tick);
    let operation = planner.air_op_mut().unwrap();
    operation.scout = None;
    operation.scout_dispatch = None;
    operation.phase_started_at = current.tick.saturating_sub(tuning.reaction_delay + 1);

    let training = planner.think_alone(&profile(), tuning, &current, &intelligence, HOME, &[]);

    let operation = planner.air_operation().unwrap();
    assert_eq!(operation.phase(), AirOperationPhase::Recon);
    assert_eq!(operation.scout, None);
    assert_eq!(operation.scout_dispatch, None);
    assert_eq!(training.committed_scrap(), UnitKind::Kestrel.stats().cost);
    assert_eq!(
        training
            .intents
            .iter()
            .filter(|intent| matches!(intent, Intent::TrainAt { .. }))
            .cloned()
            .collect::<Vec<_>>(),
        [Intent::TrainAt {
            building: BuildingId(10),
            kind: UnitKind::Kestrel,
        }],
        "the operation may hold its strike force while the required scout trains"
    );

    let mut hidden = current;
    hidden.tick += 12;
    hidden.enemy_buildings[0].seen = false;
    hidden.my_queues[0] = vec![UnitKind::Kestrel];
    hidden.scrap = 0;
    intelligence.update(&hidden);

    let waiting = planner.think_alone(&profile(), tuning, &hidden, &intelligence, HOME, &[]);

    let operation = planner.air_operation().unwrap();
    assert_eq!(operation.phase(), AirOperationPhase::Recon);
    assert_eq!(operation.scout, None);
    assert_eq!(operation.scout_dispatch, None);
    assert!(waiting.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Kestrel,
            ..
        }
    )));

    let mut ready = hidden;
    ready.tick += 12;
    ready.my_queues[0].clear();
    ready
        .my_units
        .push(own(5, UnitKind::Kestrel, TilePos::new(4, 10)));
    ready.my_units.sort_unstable_by_key(|unit| unit.id);
    intelligence.update(&ready);

    let dispatch = planner.think_alone(&profile(), tuning, &ready, &intelligence, HOME, &[]);

    let operation = planner.air_operation().unwrap();
    assert_eq!(operation.phase(), AirOperationPhase::Recon);
    assert_eq!(operation.scout, Some(UnitId(5)));
    assert!(operation.scout_dispatch.is_some());
    assert!(dispatch.intents.iter().any(|intent| matches!(
        intent,
        Intent::MoveUnits { units, .. } if units == &[UnitId(5)]
    )));

    let mut reacquired = ready;
    reacquired.tick += 12;
    reacquired.enemy_buildings[0].seen = true;
    reacquired
        .my_units
        .iter_mut()
        .find(|unit| unit.id == UnitId(5))
        .unwrap()
        .idle = false;
    intelligence.update(&reacquired);

    let reacquired_result = planner.think_alone_with(
        &profile(),
        tuning,
        &reacquired,
        &intelligence,
        HOME,
        coordination(&fixture_planning, None),
    );

    assert_eq!(
        planner.air_operation().unwrap().phase(),
        AirOperationPhase::Assemble,
        "Prime may react immediately only after the required scout has a real dispatch; rejection={:?}",
        reacquired_result.rejected_connected_candidate,
    );
}

#[test]
fn adjacent_difficulties_cannot_advance_recon_without_a_dispatched_scout() {
    for pair in BotDifficulty::ALL.windows(2) {
        let &[lower, higher] = pair else {
            unreachable!();
        };
        for difficulty in [lower, higher] {
            let tuning = DifficultyTuning::for_level(difficulty);
            let mut observation = obs(504);
            observation.my_units.retain(|unit| unit.id != UnitId(1));
            observation.my_buildings = vec![building(
                10,
                0,
                BuildingKind::Airworks,
                TilePos::new(2, 2),
                true,
            )];
            observation.my_queues = vec![vec![UnitKind::Kestrel]];
            let intelligence = knowledge(&observation);
            let mut planner = with_operation(AirOperationPhase::Recon, observation.tick);
            let operation = planner.air_op_mut().unwrap();
            operation.scout = None;
            operation.scout_dispatch = None;
            operation.phase_started_at = observation.tick.saturating_sub(tuning.reaction_delay + 1);
            let mut identity = profile();
            identity.difficulty = difficulty;

            planner.think_alone(&identity, tuning, &observation, &intelligence, HOME, &[]);

            let operation = planner.air_operation().unwrap();
            assert_eq!(
                operation.phase(),
                AirOperationPhase::Recon,
                "{difficulty:?} advanced after its full reaction delay without a scout dispatch"
            );
            assert_eq!(operation.scout, None);
            assert_eq!(operation.scout_dispatch, None);
        }
    }
}

#[test]
fn current_flak_is_suppressed_before_bombers_and_ids_are_reserved() {
    let mut obs = obs(100);
    obs.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(20, 10),
        true,
    ));
    let intel = knowledge(&obs);
    let mut planner = with_operation(AirOperationPhase::SuppressAa, 100);
    let out = think(&mut planner, &obs, &intel);
    assert_eq!(
        out.intents[0],
        Intent::AttackUnits {
            units: vec![UnitId(2)],
            target: Target::Building(BuildingId(81)),
        }
    );
    assert!(!out.intents.iter().any(
        |intent| matches!(intent, Intent::AttackUnits { units, .. } if units.contains(&UnitId(3)))
    ));
    assert_eq!(
        out.reservations,
        [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
    );
}

#[test]
fn connected_verification_does_not_treat_unfinished_flak_as_operational() {
    let flak_anchor = TilePos::new(20, 10);
    let mut construction = obs(100);
    see_approach(&mut construction);
    let dark_approach = approach(HOME, TARGET)
        .find(|tile| *tile != TARGET && *tile != flak_anchor)
        .expect("the test route has an approach tile to reacquire");
    let dark_index =
        usize::try_from(dark_approach.y * construction.map_width + dark_approach.x).unwrap();
    construction.visible[dark_index] = false;
    let mut flak = building(81, 1, BuildingKind::FlakTurret, flak_anchor, true);
    flak.built = false;
    construction.enemy_buildings.push(flak);

    let mut intel = knowledge(&construction);
    assert!(
        intel
            .buildings()
            .iter()
            .any(|building| building.id == Some(BuildingId(81)) && !building.built),
        "the observed construction remains available as ordinary intelligence"
    );
    let mut planner = with_operation(AirOperationPhase::Verify, construction.tick);
    let while_unfinished = think(&mut planner, &construction, &intel);

    let operation = planner
        .air_operation()
        .expect("the operation remains active");
    assert_eq!(operation.phase(), AirOperationPhase::Verify);
    assert_eq!(operation.recovery_reason(), None);
    assert!(while_unfinished.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(81)),
            ..
        }
    )));

    let mut completed = construction;
    completed.tick += 1;
    completed
        .enemy_buildings
        .iter_mut()
        .find(|building| building.id == BuildingId(81))
        .expect("the Flak construction remains in sight")
        .built = true;
    intel.update(&completed);
    let after_completion = think(&mut planner, &completed, &intel);

    let operation = planner
        .air_operation()
        .expect("suppression retains the operation");
    assert_eq!(operation.phase(), AirOperationPhase::SuppressAa);
    assert_eq!(operation.recovery_reason(), None);
    assert!(after_completion.intents.iter().any(|intent| matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(81)),
            ..
        }
    )));
}

#[test]
fn connected_strike_resumes_artillery_suppression_when_flak_completes() {
    let flak_anchor = TilePos::new(20, 10);
    let mut construction = obs(100);
    see_approach(&mut construction);
    explore(&mut construction, staging(HOME, TARGET));
    let mut flak = building(81, 1, BuildingKind::FlakTurret, flak_anchor, true);
    flak.built = false;
    construction.enemy_buildings.push(flak);

    let mut intel = knowledge(&construction);
    let mut planner = with_operation(AirOperationPhase::Strike, construction.tick);
    let while_unfinished = think(&mut planner, &construction, &intel);

    let operation = planner.air_operation().expect("the strike remains active");
    assert_eq!(operation.phase(), AirOperationPhase::Strike);
    assert_eq!(operation.recovery_reason(), None);
    assert!(while_unfinished.intents.iter().any(|intent| matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));

    let mut completed = construction;
    completed.tick += 1;
    completed
        .enemy_buildings
        .iter_mut()
        .find(|building| building.id == BuildingId(81))
        .expect("the Flak construction remains in sight")
        .built = true;
    intel.update(&completed);
    let after_completion = think(&mut planner, &completed, &intel);

    let operation = planner
        .air_operation()
        .expect("suppression retains the operation");
    assert_eq!(operation.phase(), AirOperationPhase::SuppressAa);
    assert_eq!(operation.recovery_reason(), None);
    assert!(after_completion.intents.iter().any(|intent| matches!(
        intent,
        Intent::AttackUnits {
            units,
            target: Target::Building(BuildingId(81)),
        } if units.contains(&UnitId(2))
    )));
    assert!(after_completion.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));
}

#[test]
fn remembered_flak_is_not_mistaken_for_destroyed_flak() {
    let mut seen = obs(100);
    seen.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(20, 10),
        true,
    ));
    let mut intel = knowledge(&seen);
    let mut hidden = obs(101);
    hidden.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, TARGET, false),
        building(
            999,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(20, 10),
            false,
        ),
    ];
    intel.update(&hidden);
    let mut planner = with_operation(AirOperationPhase::SuppressAa, 101);
    let out = think(&mut planner, &hidden, &intel);
    assert_eq!(
        planner.air_operation().unwrap().phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(
        !out.intents
            .iter()
            .any(|intent| matches!(intent, Intent::AttackUnits { .. }))
    );
}

#[test]
fn immediate_air_scheduling_preserves_staged_order_depth_and_protected_capital() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = obs(200);
    observation.my_buildings = vec![
        building(7, 0, BuildingKind::Airworks, TilePos::new(2, 2), true),
        building(3, 0, BuildingKind::Airworks, TilePos::new(8, 2), true),
    ];
    observation.my_queues = vec![vec![], vec![]];
    let kind = UnitKind::Kestrel;
    let cost = kind.stats().cost;
    let train = |id| Intent::TrainAt {
        building: BuildingId(id),
        kind,
    };
    let prior = [train(7)];
    let identity = profile();
    let intelligence = knowledge(&observation);
    for (bank, expected, held) in [
        (
            3 * cost + 17,
            vec![train(3), train(3), train(7)],
            3 * cost + 17,
        ),
        (cost + 17, vec![train(3)], cost + 17),
    ] {
        observation.scrap = bank + 53;
        let mut context =
            planning_context(&fixture_planning, &identity, &observation, &intelligence);
        context.production.prior_intents = &prior;
        context.protected_current_scrap = 53;
        let mut out = StrategicDecision::default();
        schedule(&context, &[(kind, 5)]).append_to(&mut out);
        assert_eq!(out.intents, expected);
        assert_eq!(out.committed_scrap(), held);
    }
}

#[test]
fn shared_procurement_spreads_work_and_refuses_an_unfundable_package() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut obs = obs(204);
    obs.visible.fill(true);
    obs.explored.fill(true);
    obs.my_units.truncate(1);
    obs.my_buildings = vec![
        building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), false),
        building(11, 0, BuildingKind::Fabricator, TilePos::new(5, 2), false),
        building(12, 0, BuildingKind::Airworks, TilePos::new(2, 5), false),
        building(13, 0, BuildingKind::Airworks, TilePos::new(5, 5), false),
        building(14, 0, BuildingKind::Crucible, TilePos::new(8, 5), false),
    ];
    obs.my_queues = vec![Vec::new(); 5];
    let plan = connected_test_plan(&obs);
    let mut operation = operation(AirOperationPhase::Assemble, obs.tick);
    operation.artillery.clear();
    operation.strike_aircraft.clear();
    let identity = profile();
    let intelligence = knowledge(&obs);
    let full = UnitKind::Bombard.stats().cost + UnitKind::Condor.stats().cost * 2;
    obs.scrap = full;
    let mut out = StrategicDecision::default();
    procure_connected_in_test(
        &operation,
        &plan,
        &planning_context(&fixture_planning, &identity, &obs, &intelligence),
        &mut out,
    );
    assert_eq!(out.committed_scrap(), full, "{out:?}");
    let bomber_factories: Vec<_> = out
        .intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::TrainAt {
                building,
                kind: UnitKind::Condor,
            } => Some(*building),
            _ => None,
        })
        .collect();
    assert_eq!(bomber_factories, [BuildingId(12), BuildingId(13)]);

    obs.scrap = UnitKind::Bombard.stats().cost + 17;
    let mut partial = StrategicDecision::default();
    procure_connected_in_test(
        &operation,
        &plan,
        &planning_context(&fixture_planning, &identity, &obs, &intelligence),
        &mut partial,
    );
    assert_eq!(partial.committed_scrap(), 0);
    assert!(
        partial.intents.is_empty(),
        "an infeasible retained package must be revised or recovered before buying a fragment"
    );
}

#[test]
fn a_full_operation_queue_holds_the_next_provider_cost_until_a_slot_opens() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = obs(204);
    observation.visible.fill(true);
    observation.explored.fill(true);
    observation.scrap = UnitKind::Bombard.stats().cost;
    observation.my_buildings = vec![building(
        10,
        0,
        BuildingKind::Fabricator,
        TilePos::new(2, 2),
        true,
    )];
    observation.my_queues = vec![vec![UnitKind::Lancer; QUEUE_CAP]];
    observation
        .my_units
        .retain(|unit| unit.kind != UnitKind::Bombard);
    let plan = connected_test_plan(&observation);
    let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
    operation.artillery.clear();
    let identity = profile();
    let intelligence = knowledge(&observation);
    let mut decision = StrategicDecision::default();

    procure_connected_in_test(
        &operation,
        &plan,
        &planning_context(&fixture_planning, &identity, &observation, &intelligence),
        &mut decision,
    );

    assert!(
        decision.intents.is_empty(),
        "a future slot is feasibility evidence, not a current append"
    );
    assert_eq!(
        decision.committed_scrap(),
        UnitKind::Bombard.stats().cost,
        "the operation must keep its next provider affordable while the paid queue drains"
    );
}

#[test]
fn shared_procurement_protects_capital_for_the_complete_blocked_package() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = obs(204);
    observation.visible.fill(true);
    observation.explored.fill(true);
    observation.scrap = UnitKind::Bombard.stats().cost;
    observation.my_buildings = vec![
        building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
        building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
    ];
    observation.my_queues = vec![vec![UnitKind::Lancer; QUEUE_CAP], Vec::new()];
    observation
        .my_units
        .retain(|unit| unit.kind == UnitKind::Kestrel);
    let mut plan = connected_test_plan(&observation);
    let package = plan
        .connected_package
        .as_mut()
        .expect("connected test plan has a package");
    package.strike = vec![ProviderDemand {
        kind: UnitKind::Buzzard,
        count: 1,
    }];
    package.provider_priority = vec![
        force_package::ProviderDemandTranche {
            priority: force_package::ProviderPriority::Minimum,
            family: ForceFamily::Recon,
            kind: UnitKind::Kestrel,
            count: 1,
        },
        force_package::ProviderDemandTranche {
            priority: force_package::ProviderPriority::Minimum,
            family: ForceFamily::Suppression,
            kind: UnitKind::Bombard,
            count: 1,
        },
        force_package::ProviderDemandTranche {
            priority: force_package::ProviderPriority::Minimum,
            family: ForceFamily::Strike,
            kind: UnitKind::Buzzard,
            count: 1,
        },
    ];
    let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
    operation.artillery.clear();
    operation.strike_aircraft.clear();
    let identity = profile();
    let intelligence = knowledge(&observation);
    let mut decision = StrategicDecision::default();

    procure_connected_in_test(
        &operation,
        &plan,
        &planning_context(&fixture_planning, &identity, &observation, &intelligence),
        &mut decision,
    );

    assert!(
        decision.intents.is_empty(),
        "the later open Airworks cannot spend capital assigned to the next Bombard"
    );
    assert_eq!(
        decision.committed_scrap(),
        0,
        "without income the whole package is unfundable"
    );

    observation.scrap = UnitKind::Bombard
        .stats()
        .cost
        .saturating_add(UnitKind::Buzzard.stats().cost);
    let mut parallel = StrategicDecision::default();
    procure_connected_in_test(
        &operation,
        &plan,
        &planning_context(&fixture_planning, &identity, &observation, &intelligence),
        &mut parallel,
    );

    assert!(
        parallel.intents.iter().all(|intent| matches!(
            intent,
            Intent::TrainAt {
                building: BuildingId(11),
                kind: UnitKind::Buzzard
            }
        )),
        "the full Fabricator cannot receive a current append"
    );
    assert_eq!(parallel.committed_scrap(), observation.scrap);
}

#[test]
fn current_bank_funds_the_whole_minimum_before_forecast_funded_marginal_work() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut identity = profile();
    identity.primary = Specialty::Siege;
    identity.secondary = Specialty::Air;
    identity.traits.air = 10;
    identity.traits.siege = 90;

    let minimum_scrap = UnitKind::Kestrel
        .stats()
        .cost
        .saturating_add(UnitKind::Bombard.stats().cost)
        .saturating_add(UnitKind::Buzzard.stats().cost);
    let mut observation = developed_connected_obs(108);
    observation.scrap = minimum_scrap;
    observation.my_units.clear();
    observation.enemy_buildings = vec![
        building(80, 1, BuildingKind::Turret, TARGET, true),
        building(81, 1, BuildingKind::FlakTurret, TARGET.offset(-1, 0), true),
        building(82, 1, BuildingKind::FlakTurret, TARGET.offset(1, 0), true),
    ];
    let mut reclaimer = building(20, 0, BuildingKind::Reclaimer, TilePos::new(11, 2), true);
    reclaimer.tier = 1;
    observation.my_buildings.push(reclaimer);
    observation.my_queues.push(Vec::new());

    let intelligence = knowledge(&observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.kind == BuildingKind::Turret)
        .expect("current strategic target");
    let resources = ConnectedProductionResources::from_observation(
        &observation,
        target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home: HOME,
            target: target.anchor,
            public_map: None,
            orientation: test_orientation(),
        },
    );
    let plan = connected_plan(
        &identity,
        &observation,
        &intelligence,
        HOME,
        target,
        &[],
        ConnectedPlanningContext {
            planning: Some(&fixture_planning),
            minimum_only: false,
            campaign_routes: None,
            orientation: test_orientation(),
            public_map: None,
            resources: &resources,
            preferred_artillery: &[],
            protected_current_scrap: 0,
            preparation: PreparationConstraints {
                deadline: 8_000,
                decision_cadence: DifficultyTuning::for_level(identity.difficulty).cadence,
                protected_forecast_scrap: 0,
            },
        },
    )
    .expect("forecast income can fund marginal suppression after the minimum package");
    let package = plan
        .connected_package
        .as_ref()
        .expect("connected plan carries its force package");
    let minimum: Vec<_> = package
        .provider_priority
        .iter()
        .filter(|tranche| tranche.priority == force_package::ProviderPriority::Minimum)
        .copied()
        .collect();
    assert_eq!(
        minimum
            .iter()
            .map(|tranche| tranche.family)
            .collect::<Vec<_>>(),
        [
            ForceFamily::Recon,
            ForceFamily::Suppression,
            ForceFamily::Strike,
        ]
    );
    assert!(package.provider_priority.iter().any(|tranche| {
        tranche.priority == force_package::ProviderPriority::Marginal
            && tranche.family == ForceFamily::Suppression
    }));

    let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
    operation.scout = None;
    operation.artillery.clear();
    operation.strike_aircraft.clear();
    let mut decision = StrategicDecision::default();
    procure_connected_in_test(
        &operation,
        &plan,
        &planning_context(&fixture_planning, &identity, &observation, &intelligence),
        &mut decision,
    );

    let scheduled: Vec<_> = decision
        .intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::TrainAt { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        scheduled,
        minimum
            .iter()
            .flat_map(|tranche| core::iter::repeat_n(tranche.kind, tranche.count))
            .collect::<Vec<_>>()
    );
    assert_eq!(decision.committed_scrap(), minimum_scrap);
}

#[test]
fn scout_holds_outside_known_flak_while_keeping_the_objective_in_sight() {
    let mut obs = obs(250);
    obs.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(20, 10),
        true,
    ));
    let intel = knowledge(&obs);
    let operation = operation(AirOperationPhase::SuppressAa, 250);

    let goal = scout_goal(&operation, &obs, &intel, operation.target, &[], None)
        .expect("safe spotting tile");
    assert_ne!(
        goal, operation.target,
        "the scout must not orbit over the bomb target"
    );
    assert_ne!(
        intel.air_defense_at(goal).evidence(),
        AirDefenseEvidence::CurrentCoverage,
        "a known safe spotting tile exists outside the flak envelope"
    );
    let dx = goal.x - operation.target.x;
    let dy = goal.y - operation.target.y;
    let sight = UnitKind::Kestrel.stats().vision - 1;
    assert!(dx * dx + dy * dy <= sight * sight);
}

#[test]
fn optional_strike_loss_continues_until_minimum_capability_is_lost() {
    let mut battle = obs(300);
    battle.my_units.retain(|unit| unit.id != UnitId(4));
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Strike, 300);
    let continued = think(&mut planner, &battle, &intel);
    let op = planner.air_operation().unwrap();
    assert_eq!(op.phase(), AirOperationPhase::Strike);
    assert_eq!(op.recovery_reason(), None);
    assert_eq!(continued.reservations, [UnitId(1), UnitId(2), UnitId(3)]);

    battle.tick += 1;
    battle.my_units.retain(|unit| unit.id != UnitId(3));
    let intel = knowledge(&battle);
    let out = think(&mut planner, &battle, &intel);
    let op = planner.air_operation().unwrap();
    assert_eq!(op.phase(), AirOperationPhase::Recover);
    assert_eq!(
        op.recovery_reason(),
        Some(AirRecoveryReason::RequiredUnitLost)
    );
    assert!(planner.cooldown_until > battle.tick);
    assert!(out.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(1), UnitId(2)],
        goal: HOME,
    }));

    let late = 10_000;
    let obs = obs(late);
    let intel = knowledge(&obs);
    let mut timed_out = with_operation(AirOperationPhase::Recon, late);
    timed_out.air_op_mut().unwrap().started_at = 0;
    think(&mut timed_out, &obs, &intel);
    let op = timed_out.air_operation().unwrap();
    assert_eq!(op.phase(), AirOperationPhase::Recover);
    assert_eq!(op.recovery_reason(), Some(AirRecoveryReason::Timeout));
}

#[test]
fn preparation_timeout_does_not_report_an_executed_assault_failure() {
    use crate::experience::{Outcome, OutcomeReason};
    let observation = obs(10_000);
    let intel = knowledge(&observation);
    for phase in [AirOperationPhase::Recon, AirOperationPhase::Assemble] {
        let mut planner = with_operation(phase, observation.tick);
        planner.air_op_mut().unwrap().started_at = 0;
        think(&mut planner, &observation, &intel);
        let report = &planner.outcomes.pending[0];
        assert_eq!(report.outcome, Outcome::Aborted);
        assert_eq!(report.reason, OutcomeReason::Deadline);
        assert_eq!(report.own_lost_value, 0);
        assert!(!report.doctrine_eligible);
    }
}

#[test]
fn recovery_trusts_terminal_move_orders_instead_of_recalling_every_think() {
    let mut settled = obs(408);
    settled.faction = Faction::Cupric;
    settled.my_units = vec![
        own(1, UnitKind::Gnat, TilePos::new(3, 10)),
        own(2, UnitKind::Bombard, TilePos::new(4, 10)),
        own(3, UnitKind::Moth, TilePos::new(1, 8)),
        own(4, UnitKind::Moth, TilePos::new(2, 8)),
    ];
    let intel = knowledge(&settled);
    let mut planner = with_operation(AirOperationPhase::Recover, settled.tick);
    planner.cooldown_until = 1_000;
    planner.air_op_mut().unwrap().stage = AirStage::Recover {
        reason: AirRecoveryReason::Complete,
        assault_admitted: true,
    };

    let decision = think(&mut planner, &settled, &intel);

    assert!(planner.air_operation().is_none());
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { goal, .. } if *goal == HOME
    )));
    assert_eq!(
        decision.reservations,
        [UnitId(1), UnitId(2), UnitId(3), UnitId(4)],
        "the completion cadence retains ownership until utility has finished"
    );

    settled.tick = 999;
    let intel = knowledge(&settled);
    let next = think(&mut planner, &settled, &intel);
    assert_eq!(
        next.reservations,
        [UnitId(1), UnitId(2), UnitId(3), UnitId(4)],
        "settled operation members remain owned through the cooldown"
    );

    settled.tick = 1_000;
    let intel = knowledge(&settled);
    let released = think(&mut planner, &settled, &intel);
    assert!(released.reservations.is_empty());
}

#[test]
fn a_dropped_failed_operation_exposes_one_terminal_abort_signal() {
    let observation = obs(400);
    let intel = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Recover, 400);
    planner.air_op_mut().unwrap().stage = AirStage::Recover {
        reason: AirRecoveryReason::NewAirDefense,
        assault_admitted: true,
    };

    think(&mut planner, &observation, &intel);

    assert!(planner.air_operation().is_none());
    assert_eq!(
        planner.terminal_outcome(),
        Some(AirOperationOutcome::Aborted {
            player: PlayerId(1),
            target: TARGET,
        })
    );

    think(&mut planner, &observation, &intel);
    assert_eq!(planner.terminal_outcome(), None);
}

#[test]
fn the_next_operation_reuses_its_standby_roles_before_training_replacements() {
    let mut settled = obs(408);
    settled.my_units.retain(|unit| unit.id != UnitId(4));
    settled
        .my_units
        .push(own(5, UnitKind::Bombard, TilePos::new(5, 10)));
    settled
        .my_units
        .extend((100..=108).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
    settled.my_units.sort_unstable_by_key(|unit| unit.id);
    settled.my_buildings = vec![
        building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
        building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
        building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
    ];
    settled.my_queues = vec![Vec::new(); 3];
    settled.visible.fill(true);
    settled.explored.fill(true);
    settled.scrap = 10_000;
    let mut planner = with_operation(AirOperationPhase::Recover, settled.tick);
    planner.cooldown_until = 500;
    let operation = planner.air_op_mut().unwrap();
    operation.artillery = vec![UnitId(2), UnitId(5)];
    operation.strike_aircraft = vec![UnitId(3)];
    operation.stage = AirStage::Recover {
        reason: AirRecoveryReason::Timeout,
        assault_admitted: true,
    };
    let intel = knowledge(&settled);

    let recovered = think(&mut planner, &settled, &intel);
    assert!(planner.air_operation().is_none());
    assert_eq!(
        recovered.reservations,
        [UnitId(1), UnitId(2), UnitId(3), UnitId(5)]
    );

    settled.tick = 504;
    let intel = knowledge(&settled);
    assert!(
        derived_connected_test_plan(&profile(), &settled).is_some(),
        "the recovered force and current bank can field a retry"
    );
    let retried = think(&mut planner, &settled, &intel);
    let operation = planner.air_operation().expect("a new operation starts");
    assert_eq!(operation.scout, Some(UnitId(1)));
    assert_eq!(operation.artillery, [UnitId(2)]);
    assert!(
        !retried.reservations.contains(&UnitId(5)),
        "standby units beyond the target's useful suppression demand return to the standing force"
    );
    assert_eq!(operation.strike_aircraft, [UnitId(3)]);
    let retry_package = planner
        .air_plan()
        .and_then(|plan| plan.connected_package.as_ref())
        .expect("the retry retains its selected force package");
    let selected_strike = retry_package
        .strike
        .iter()
        .map(|demand| demand.count)
        .sum::<usize>();
    let scheduled_strike = retried
        .intents
        .iter()
        .filter(|intent| {
            matches!(
                intent,
                Intent::TrainAt { kind, .. }
                    if retry_package.strike.iter().any(|demand| demand.kind == *kind)
            )
        })
        .count();
    assert_eq!(
        scheduled_strike,
        selected_strike.saturating_sub(operation.strike_aircraft.len()),
        "lowering trains only the selected strike demand not already held in standby"
    );
    assert!(retried.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Bombard,
            ..
        }
    )));
}

#[test]
fn standby_releases_when_no_operation_target_survives_the_cooldown() {
    let mut settled = obs(408);
    let mut planner = with_operation(AirOperationPhase::Recover, settled.tick);
    planner.cooldown_until = 500;
    planner.air_op_mut().unwrap().stage = AirStage::Recover {
        reason: AirRecoveryReason::Complete,
        assault_admitted: true,
    };
    let intel = knowledge(&settled);
    think(&mut planner, &settled, &intel);

    settled.tick = 504;
    settled.enemy_buildings.clear();
    let intel = knowledge(&settled);
    let released = think(&mut planner, &settled, &intel);

    assert!(planner.air_operation().is_none());
    assert!(released.reservations.is_empty());
}

#[test]
fn recovery_keeps_moving_survivors_without_replacing_their_return_order() {
    let mut returning = obs(408);
    returning.my_units[0].idle = false;
    let intel = knowledge(&returning);
    let mut planner = with_operation(AirOperationPhase::Recover, returning.tick);

    let decision = think(&mut planner, &returning, &intel);

    assert!(planner.air_operation().is_some());
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { goal, .. } if *goal == HOME
    )));
}

#[test]
fn assembly_reconnoiters_an_unexplored_staging_line_before_moving_artillery() {
    let mut observation = obs(300);
    observation
        .my_units
        .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
    let intel = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Assemble, 300);

    let decision = think(&mut planner, &observation, &intel);
    let ideal = staging(HOME, TARGET);

    let operation = planner.air_operation().expect("assembly remains active");
    assert_eq!(operation.phase(), AirOperationPhase::Assemble);
    assert_eq!(operation.scout_dispatch, Some((UnitId(1), ideal)));
    assert!(decision.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(1)],
        goal: ideal,
    }));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, .. }
            if units.contains(&UnitId(2)) || units.contains(&UnitId(5))
    )));
}

#[test]
fn public_terrain_skips_a_sealed_staging_tile_for_a_reachable_alternative() {
    let mut observation = obs(300);
    observation.my_units[1].tile = HOME.offset(4, 0);
    let ideal = staging(HOME, TARGET);
    let sealed_ring = (-1..=1).flat_map(|dy| {
        (-1..=1)
            .filter(move |dx| *dx != 0 || dy != 0)
            .map(move |dx| (ideal.offset(dx, dy), Terrain::Peak))
    });
    let public_map = public_map_with_terrain(&observation, sealed_ring);
    let operation = operation(AirOperationPhase::Assemble, observation.tick);

    assert_eq!(
        connected_artillery_staging_goal(&observation, HOME, TARGET, None),
        Some(ideal),
        "fog alone makes the enclosed ideal look reachable"
    );
    let public_goal =
        connected_artillery_staging_goal(&observation, HOME, TARGET, Some(&public_map))
            .expect("a later public-terrain-safe staging candidate exists");
    assert_ne!(public_goal, ideal);
    assert_eq!(
        artillery_staging(
            &operation,
            &observation,
            HOME,
            TARGET,
            Some(&public_map),
            test_orientation(),
        ),
        Some(ArtilleryStaging::NeedsRecon(public_goal))
    );
}

#[test]
fn artillery_staging_validates_the_exact_spread_assigned_by_the_group_command() {
    let mut observation = obs(300);
    observation.explored.fill(true);
    observation.my_units[1].tile = TilePos::new(14, 10);
    observation
        .my_units
        .push(own(5, UnitKind::Bombard, TilePos::new(14, 11)));
    let ideal = staging(HOME, TARGET);
    let isolated_spread_goal = ideal.offset(1, 1);
    observation.known_rock = [
        isolated_spread_goal.offset(-1, 0),
        isolated_spread_goal.offset(1, 0),
        isolated_spread_goal.offset(0, -1),
        isolated_spread_goal.offset(0, 1),
    ]
    .into_iter()
    .collect();
    observation
        .known_rock
        .sort_unstable_by_key(|tile| (tile.y, tile.x));
    let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
    operation.artillery.push(UnitId(5));

    let mut center_only =
        route_projection_with_orientation(&observation, Domain::Ground, None, test_orientation());
    assert!(operation.artillery.iter().all(|id| {
        unit(&observation, *id).is_some_and(|member| center_only.unit_reaches(member, ideal))
    }));
    assert!(
        !center_only.group_reaches_command_goal(&operation.artillery, ideal),
        "the eastward approach assigns the isolated south-east spread tile"
    );

    let alternate = artillery_staging(
        &operation,
        &observation,
        HOME,
        TARGET,
        None,
        test_orientation(),
    )
    .expect("a later staging candidate accepts the complete spread");
    let ArtilleryStaging::Ready(alternate) = alternate else {
        panic!("the explored fixture must return a ready staging goal");
    };
    assert_ne!(alternate, ideal);
    let exact =
        route_projection_with_orientation(&observation, Domain::Ground, None, test_orientation());
    assert!(exact.group_reaches_command_goal(&operation.artillery, alternate));
}

#[test]
fn connected_admission_rejects_a_group_larger_than_the_reachable_staging_spread() {
    let home = TilePos::new(0, 0);
    let target = TilePos::new(6, 0);
    let source_staging = staging(home, target);
    let remote_open = TilePos::new(5, 0);
    let mut observation = obs(300);
    observation.map_width = 12;
    observation.map_height = 8;
    observation.visible = vec![true; 12 * 8];
    observation.explored = vec![true; 12 * 8];
    observation.my_units = vec![
        own(2, UnitKind::Bombard, source_staging),
        own(5, UnitKind::Bombard, source_staging),
    ];
    observation.my_buildings = vec![building(20, 0, BuildingKind::Foundry, home, true)];
    observation.my_queues = vec![Vec::new()];
    observation.enemy_buildings = vec![building(80, 1, BuildingKind::Crucible, target, true)];
    observation.known_rock = (0..observation.map_height)
        .flat_map(|y| {
            (0..observation.map_width).filter_map(move |x| {
                let tile = TilePos::new(x, y);
                (tile != source_staging && tile != remote_open).then_some(tile)
            })
        })
        .collect();
    let public_map = public_map_with_terrain(&observation, []);
    let orientation = Orientation::for_home(&observation, home);
    let intelligence = knowledge(&observation);
    let route = ConnectedRouteContext {
        campaign_routes: None,
        unavailable_paid: &[],
        intel: &intelligence,
        home,
        target,
        public_map: Some(&public_map),
        orientation,
    };

    assert_eq!(
        connected_artillery_staging_goal(&observation, home, target, Some(&public_map)),
        Some(source_staging)
    );
    let mut individual = route_projection_with_orientation(
        &observation,
        Domain::Ground,
        Some(&public_map),
        orientation,
    );
    assert!(
        observation
            .my_units
            .iter()
            .all(|member| { individual.unit_reaches(member, source_staging) })
    );
    assert!(connected_artillery_group_has_staging(
        &observation,
        route,
        &[ProviderDemand {
            kind: UnitKind::Bombard,
            count: 1,
        }],
        &[],
        &[],
    ));
    assert!(
        !connected_artillery_group_has_staging(
            &observation,
            route,
            &[ProviderDemand {
                kind: UnitKind::Bombard,
                count: 2,
            }],
            &[],
            &[],
        ),
        "individual center reachability cannot admit a spread slot in another component"
    );
}

#[test]
fn campaign_queries_share_navigation_across_targets_and_producers() {
    let mut observation = obs(300);
    observation.enemy_buildings = vec![
        building(81, 1, BuildingKind::FlakTurret, TilePos::new(15, 7), true),
        building(82, 1, BuildingKind::Foundry, TilePos::new(20, 7), true),
    ];
    let map = public_map_with_terrain(&observation, vec![]);
    let intel = knowledge(&observation);
    let resources = ResourceSnapshot::from_observation(&observation);
    let cached = CampaignRoutes::new(&observation, &intel, Some(&map), test_orientation());
    let evaluate = |cache| {
        intel
            .buildings()
            .iter()
            .map(|target| {
                let route = ConnectedRouteContext {
                    campaign_routes: cache,
                    unavailable_paid: &[],
                    intel: &intel,
                    home: HOME,
                    target: target.anchor,
                    public_map: Some(&map),
                    orientation: test_orientation(),
                };
                let targets = connected_target_selection(&observation, target, &[], route);
                let access = connected_production_access(&observation, &targets, &resources, route);
                let unavailable =
                    connected_provider_unavailable(&observation, &targets, &[], route);
                let staging = connected_artillery_group_has_staging(
                    &observation,
                    route,
                    &[ProviderDemand {
                        kind: UnitKind::Bombard,
                        count: 3,
                    }],
                    &[],
                    &[],
                );
                (targets, access, unavailable, staging)
            })
            .collect::<Vec<_>>()
    };
    let expected = evaluate(None);
    let (actual, cold) = crate::navigation::work::measure(|| evaluate(Some(&cached)));
    assert_eq!(actual, expected);
    assert!(cold.components <= 2, "{cold:?}");
    assert!(
        cold.expanded <= 2 * (observation.map_width * observation.map_height) as usize,
        "{cold:?}"
    );
    let (_, warm) = crate::navigation::work::measure(|| {
        for _ in 0..20 {
            assert_eq!(evaluate(Some(&cached)), expected);
        }
    });
    assert_eq!(warm.expanded, 0, "{warm:?}");
    assert_eq!(warm.components, 0, "{warm:?}");
}

#[test]
fn suppression_batch_preserves_assignments_and_reuses_mixed_roster_queries() {
    let mut observation = obs(300);
    observation.enemy_buildings = vec![building(
        81,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(15, 7),
        true,
    )];
    let target = Target::Building(BuildingId(81));
    let origins = [
        SuppressionOrigin {
            tile: TilePos::new(2, 7),
            kind: UnitKind::Bombard,
        },
        SuppressionOrigin {
            tile: TilePos::new(2, 7),
            kind: UnitKind::Avalanche,
        },
        SuppressionOrigin {
            tile: TilePos::new(3, 7),
            kind: UnitKind::Bombard,
        },
    ];
    for blocked in [false, true] {
        let terrain: Vec<_> = if blocked {
            (0..observation.map_height)
                .flat_map(|y| {
                    (0..observation.map_width).map(move |x| (TilePos::new(x, y), Terrain::Pit))
                })
                .collect()
        } else {
            Vec::new()
        };
        let map = public_map_with_terrain(&observation, terrain);
        let intel = knowledge(&observation);
        let cached = CampaignRoutes::new(&observation, &intel, Some(&map), test_orientation());
        for origin in origins {
            assert_eq!(
                cached.reaches(origin, target),
                !suppression_firing_stands(
                    cached.ground(),
                    &observation,
                    origin,
                    target,
                    &intel,
                    Some(&map)
                )
                .collect::<Vec<_>>()
                .is_empty()
            );
        }
        assert_eq!(
            cached.evaluated_queries(),
            0,
            "reachability must not materialize provider assignments"
        );
        assert_eq!(cached.evaluated_geometry(), 2);
        for roster in [
            vec![],
            vec![origins[0]],
            vec![origins[0]; 4],
            vec![origins[0], origins[1], origins[0], origins[1]],
            origins.to_vec(),
        ] {
            let expected = suppression_firing_assignment(
                &observation,
                &intel,
                &roster,
                target,
                Some(&map),
                test_orientation(),
            );
            assert_eq!(cached.assignment(&roster, target), expected);
            for origin in &roster {
                let routes = route_projection_with_orientation(
                    &observation,
                    Domain::Ground,
                    Some(&map),
                    test_orientation(),
                );
                assert_eq!(
                    cached.reaches(*origin, target),
                    suppression_targets_reachable(
                        &routes,
                        &observation,
                        *origin,
                        &[target],
                        &intel,
                        Some(&map)
                    )
                );
            }
            let cold_queries = cached.evaluated_queries();
            let (_, warm) = crate::navigation::work::measure(|| {
                for _ in 0..20 {
                    assert_eq!(cached.assignment(&roster, target), expected);
                }
            });
            assert_eq!(cached.evaluated_queries(), cold_queries);
            assert_eq!(warm.searches, 0);
        }
        assert_eq!(cached.evaluated_queries(), origins.len());
        assert_eq!(
            cached.evaluated_geometry(),
            2,
            "origins sharing a weapon must share target geometry"
        );
    }
}

#[test]
fn excluded_live_providers_require_no_route_work() {
    let observation = obs(300);
    let intel = knowledge(&observation);
    let unavailable: Vec<_> = observation
        .my_units
        .iter()
        .rev()
        .map(|unit| unit.id)
        .collect();
    let mut expected = unavailable.clone();
    expected.sort_unstable();
    let ((actual, repeated), work) = crate::navigation::work::measure(|| {
        let route = ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intel,
            home: HOME,
            target: TilePos::new(15, 7),
            public_map: None,
            orientation: test_orientation(),
        };
        let targets = ConnectedTargetSelection {
            target_anchors: vec![],
            suppression_targets: vec![],
            growth_order: vec![],
        };
        let actual = connected_provider_unavailable(&observation, &targets, &unavailable, route);
        let mut duplicates = unavailable.clone();
        duplicates.extend_from_slice(&unavailable);
        let repeated = connected_provider_unavailable(&observation, &targets, &duplicates, route);
        (actual, repeated)
    });
    assert_eq!(actual, expected);
    assert_eq!(repeated, expected);
    assert_eq!(work.searches, 0);
}

#[test]
fn suppression_preflight_requires_one_reachable_firing_stand_per_provider() {
    let origin = TilePos::new(0, 7);
    let first_stand = TilePos::new(1, 7);
    let second_stand = TilePos::new(2, 7);
    let target_anchor = TilePos::new(11, 7);
    let mut observation = obs(300);
    observation.map_width = 14;
    observation.map_height = 14;
    observation.visible = vec![true; 14 * 14];
    observation.explored = vec![true; 14 * 14];
    observation.my_units.clear();
    observation.enemy_buildings = vec![building(
        81,
        1,
        BuildingKind::FlakTurret,
        target_anchor,
        true,
    )];
    let intelligence = knowledge(&observation);
    let target = Target::Building(BuildingId(81));
    let origins = vec![
        SuppressionOrigin {
            tile: origin,
            kind: UnitKind::Bombard,
        },
        SuppressionOrigin {
            tile: origin,
            kind: UnitKind::Bombard,
        },
    ];
    let (width, height) = (observation.map_width, observation.map_height);
    let terrain_with = |open: Vec<TilePos>| {
        (0..height).flat_map(move |y| {
            let open = open.clone();
            (0..width).filter_map(move |x| {
                let tile = TilePos::new(x, y);
                (!open.contains(&tile)).then_some((tile, Terrain::Pit))
            })
        })
    };

    let one_stand_map =
        public_map_with_terrain(&observation, terrain_with(vec![origin, first_stand]));
    assert_eq!(
        suppression_firing_assignment(
            &observation,
            &intelligence,
            &origins[..1],
            target,
            Some(&one_stand_map),
            test_orientation(),
        ),
        Some(vec![first_stand])
    );
    assert_eq!(
        suppression_firing_assignment(
            &observation,
            &intelligence,
            &origins,
            target,
            Some(&one_stand_map),
            test_orientation(),
        ),
        None,
        "two providers cannot be admitted against one legal firing tile"
    );

    let two_stand_map = public_map_with_terrain(
        &observation,
        terrain_with(vec![origin, first_stand, second_stand]),
    );
    let assigned = suppression_firing_assignment(
        &observation,
        &intelligence,
        &origins,
        target,
        Some(&two_stand_map),
        test_orientation(),
    )
    .expect("two connected legal firing tiles admit both providers");
    assert_eq!(assigned, vec![second_stand, first_stand]);
    assert!(assigned.iter().all(|stand| {
        let routes = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&two_stand_map),
            test_orientation(),
        );
        suppression_firing_stands(
            &routes,
            &observation,
            origins[0],
            target,
            &intelligence,
            Some(&two_stand_map),
        )
        .any(|candidate| candidate == *stand)
    }));
}

#[test]
fn suppression_assignment_backtracks_for_a_constrained_provider() {
    let options = vec![vec![0, 1], vec![0]];
    let mut owner_by_stand = vec![None; 2];
    let mut first_visited = vec![false; 2];
    assert!(augment_suppression_assignment(
        0,
        &options,
        &mut first_visited,
        &mut owner_by_stand,
    ));
    let mut second_visited = vec![false; 2];
    assert!(augment_suppression_assignment(
        1,
        &options,
        &mut second_visited,
        &mut owner_by_stand,
    ));
    assert_eq!(owner_by_stand, vec![Some(1), Some(0)]);
}

#[test]
fn authoritative_spread_preflight_is_scoped_to_connected_operations() {
    let mut observation = obs(300);
    let goal = TilePos::new(12, 10);
    observation.my_units[2].tile = goal.offset(4, 0);
    observation.my_units[3].tile = goal.offset(4, 1);
    let isolated_reversed_slot = goal.offset(1, 1);
    observation.known_peaks = [
        isolated_reversed_slot.offset(-1, 0),
        isolated_reversed_slot.offset(1, 0),
        isolated_reversed_slot.offset(0, -1),
        isolated_reversed_slot.offset(0, 1),
    ]
    .into_iter()
    .collect();
    observation
        .known_peaks
        .sort_unstable_by_key(|tile| (tile.y, tile.x));
    observation.my_buildings = vec![building(
        20,
        0,
        BuildingKind::Airworks,
        TilePos::new(2, 2),
        true,
    )];
    observation.my_queues = vec![Vec::new()];
    let attackers = [UnitId(3), UnitId(4)];
    let orientation = test_orientation();

    let island = AirPlan::island(&profile(), &observation);
    let island_routes =
        operation_route_projection(&island, &observation, Domain::Air, None, orientation);
    assert!(
        island_routes.group_reaches_command_goal(&attackers, goal),
        "the pre-existing island operation keeps its legacy forward spread"
    );

    let connected = connected_test_plan(&observation);
    let connected_routes =
        operation_route_projection(&connected, &observation, Domain::Air, None, orientation);
    assert!(
        !connected_routes.group_reaches_command_goal(&attackers, goal),
        "the migrated connected operation preflights the authoritative reverse spread"
    );
}

#[test]
fn connected_admission_uses_the_authoritative_spread_for_an_exact_live_group() {
    let home = TilePos::new(0, 2);
    let target = TilePos::new(6, 2);
    let source_staging = staging(home, target);
    let mut observation = obs(300);
    observation.map_width = 12;
    observation.map_height = 8;
    observation.visible = vec![true; 12 * 8];
    observation.explored = vec![true; 12 * 8];
    observation.my_units = vec![
        own(2, UnitKind::Bombard, source_staging.offset(-1, -1)),
        own(5, UnitKind::Bombard, source_staging.offset(0, -1)),
    ];
    observation.my_buildings = vec![building(20, 0, BuildingKind::Foundry, home, true)];
    observation.my_queues = vec![Vec::new()];
    observation.enemy_buildings = vec![building(80, 1, BuildingKind::Crucible, target, true)];
    let open = [
        source_staging,
        source_staging.offset(-1, -1),
        source_staging.offset(0, -1),
        source_staging.offset(1, 1),
    ];
    observation.known_rock = (0..observation.map_height)
        .flat_map(|y| {
            (0..observation.map_width).filter_map(move |x| {
                let tile = TilePos::new(x, y);
                (!open.contains(&tile)).then_some(tile)
            })
        })
        .collect();
    let public_map = public_map_with_terrain(&observation, []);
    let orientation = Orientation::for_home(&observation, home);
    let intelligence = knowledge(&observation);
    let route = ConnectedRouteContext {
        campaign_routes: None,
        unavailable_paid: &[],
        intel: &intelligence,
        home,
        target,
        public_map: Some(&public_map),
        orientation,
    };
    let exact_ids = [UnitId(2), UnitId(5)];
    let routes = route_projection_with_orientation(
        &observation,
        Domain::Ground,
        Some(&public_map),
        orientation,
    );
    assert!(routes.group_reaches_command_goal(&exact_ids, source_staging));
    assert!(
        !artillery_group_reaches_staging(&routes, source_staging, source_staging, 2, None,),
        "the unused reverse scan reaches a sealed south-east slot"
    );

    assert!(connected_artillery_group_has_staging(
        &observation,
        route,
        &[ProviderDemand {
            kind: UnitKind::Bombard,
            count: 2,
        }],
        &[],
        &[],
    ));
    let future_demand = [ProviderDemand {
        kind: UnitKind::Bombard,
        count: 3,
    }];
    assert!(exact_live_provider_group(&observation, &future_demand, &[], &[]).is_none());
    assert!(
        !artillery_group_reaches_staging(&routes, source_staging, source_staging, 3, None,),
        "a group with a future member still tests both possible spread scans"
    );
}

#[test]
fn assembly_aborts_before_moving_the_force_when_unexplored_staging_is_air_inaccessible() {
    let mut observation = obs(300);
    observation.my_units[0].tile = HOME;
    observation.known_peaks = (0..observation.map_height)
        .map(|y| TilePos::new(8, y))
        .collect();
    let intel = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Assemble, observation.tick);

    let decision = think(&mut planner, &observation, &intel);
    let operation = planner
        .air_operation()
        .expect("the failed assembly remains observable during recovery");

    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, goal }
            if (units.contains(&UnitId(2))
                || units.contains(&UnitId(3))
                || units.contains(&UnitId(4)))
                && *goal != HOME
    )));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn ready_artillery_staging_still_recovers_when_the_final_scout_route_is_sealed() {
    let mut observation = obs(300);
    observation.my_units[0].tile = HOME;
    let staging_goal = staging(HOME, TARGET);
    explore(&mut observation, staging_goal);
    observation.known_peaks = (0..observation.map_height)
        .map(|y| TilePos::new(8, y))
        .collect();
    let mut intelligence = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Assemble, observation.tick);

    let operation = planner.air_operation().expect("assembly is active");
    assert_eq!(
        artillery_staging(
            operation,
            &observation,
            HOME,
            TARGET,
            None,
            test_orientation(),
        ),
        Some(ArtilleryStaging::Ready(staging_goal)),
        "the artillery is already across the peak wall on explored staging ground"
    );
    assert_eq!(
        scout_goal(
            operation,
            &observation,
            &intelligence,
            operation.target,
            &[],
            None,
        ),
        None,
        "the home-side scout has no admissible route into the target's vision envelope"
    );

    let failed = think(&mut planner, &observation, &intelligence);
    let operation = planner
        .air_operation()
        .expect("the failed operation remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(planner.cooldown_until > observation.tick);
    assert!(failed.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, goal }
            if units.contains(&UnitId(2)) && *goal == staging_goal
    )));
    assert!(failed.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));

    observation.tick += 1;
    intelligence.update(&observation);
    think(&mut planner, &observation, &intelligence);
    assert!(planner.air_operation().is_none());
    assert_eq!(
        planner.terminal_outcome(),
        Some(AirOperationOutcome::Aborted {
            player: PlayerId(1),
            target: TARGET,
        })
    );

    observation.tick += 1;
    assert!(observation.tick < planner.cooldown_until);
    intelligence.update(&observation);
    let cooling_down = think(&mut planner, &observation, &intelligence);
    assert!(planner.air_operation().is_none());
    assert!(cooling_down.reservations.is_empty());
    assert!(cooling_down.intents.is_empty());
    assert_eq!(planner.terminal_outcome(), None);
}

#[test]
fn connected_strike_refuses_an_air_route_blocked_only_in_the_public_briefing() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = obs(300);
    see_approach(&mut observation);
    explore(&mut observation, staging(HOME, TARGET));
    let intelligence = knowledge(&observation);
    let public_map = public_map_with_terrain(
        &observation,
        (0..observation.map_height).map(|y| (TilePos::new(16, y), Terrain::Peak)),
    );

    let mut optimistic = with_operation(AirOperationPhase::Strike, observation.tick);
    let optimistic_decision = think(&mut optimistic, &observation, &intelligence);
    assert!(optimistic_decision.intents.iter().any(|intent| matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));

    let mut guarded = with_operation(AirOperationPhase::Strike, observation.tick);
    let guarded_decision = guarded
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            StrategicCoordination {
                planning: Some(&fixture_planning),
                public_map: Some(&public_map),
                ..coordination(&fixture_planning, None)
            },
        )
        .decision;
    let operation = guarded
        .air_operation()
        .expect("the refused strike remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(guarded_decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn scouted_water_aborts_staging_without_issuing_an_artillery_move() {
    let mut observation = obs(300);
    observation
        .my_units
        .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
    let ideal = staging(HOME, TARGET);
    let intel = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Assemble, 300);
    think(&mut planner, &observation, &intel);

    observation.tick += 1;
    observation.explored.fill(true);
    observation.known_rock = (0..observation.map_height)
        .flat_map(|y| (ideal.x - 3..=ideal.x + 3).map(move |x| TilePos::new(x, y)))
        .collect();
    let intel = knowledge(&observation);
    let decision = think(&mut planner, &observation, &intel);

    let operation = planner
        .air_operation()
        .expect("recovery remains observable");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableStaging)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, goal }
            if (units.contains(&UnitId(2)) || units.contains(&UnitId(5))) && *goal != HOME
    )));
}

#[test]
fn explored_connected_staging_still_dispatches_the_artillery_group() {
    let mut observation = obs(300);
    observation.explored.fill(true);
    observation
        .my_units
        .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
    let intel = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Assemble, 300);

    let decision = think(&mut planner, &observation, &intel);

    assert_eq!(
        planner
            .air_operation()
            .expect("operation continues")
            .phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(decision.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(2)],
        goal: staging(HOME, TARGET),
    }));
}

#[test]
fn artillery_operation_aborts_when_its_local_staging_line_is_known_severed() {
    let mut observation = obs(300);
    observation
        .my_units
        .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
    let ideal = staging(HOME, TARGET);
    observation.known_rock = (0..observation.map_height)
        .flat_map(|y| (ideal.x - 3..=ideal.x + 3).map(move |x| TilePos::new(x, y)))
        .collect();
    let intel = knowledge(&observation);
    let mut planner = with_operation(AirOperationPhase::Assemble, 300);

    let decision = think(&mut planner, &observation, &intel);

    let operation = planner.air_operation().unwrap();
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableStaging)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, goal }
            if (units.contains(&UnitId(2)) || units.contains(&UnitId(5))) && *goal != HOME
    )));

    observation.tick += 1;
    let intel = knowledge(&observation);
    think(&mut planner, &observation, &intel);
    assert!(planner.air_operation().is_none());

    observation.tick += 1;
    let intel = knowledge(&observation);
    let released = think(&mut planner, &observation, &intel);
    assert!(
        released.reservations.is_empty(),
        "a structurally unreachable objective must not hoard its roster through cooldown"
    );
}

#[test]
fn known_peak_wall_aborts_scout_ingress_but_a_gap_restores_it() {
    let mut sealed = obs(300);
    sealed.my_units[0].tile = TilePos::new(4, 10);
    sealed.known_peaks = (0..sealed.map_height).map(|y| TilePos::new(8, y)).collect();
    let intel = knowledge(&sealed);
    let mut planner = with_operation(AirOperationPhase::Recon, 300);

    think(&mut planner, &sealed, &intel);
    assert_eq!(
        planner
            .air_operation()
            .and_then(|operation| operation.recovery_reason()),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );

    let mut open = sealed;
    open.known_peaks.retain(|tile| tile.y != 10);
    let intel = knowledge(&open);
    let mut planner = with_operation(AirOperationPhase::Recon, 300);
    let decision = think(&mut planner, &open, &intel);
    assert!(decision.intents.iter().any(|intent| matches!(
        intent,
        Intent::MoveUnits { units, goal }
            if units == &[UnitId(1)] && goal.x > 8
    )));
}

#[test]
fn known_peak_wall_blocks_bomber_commitment() {
    let mut battle = obs(400);
    see_approach(&mut battle);
    explore(&mut battle, staging(HOME, TARGET));
    battle.known_peaks = (0..battle.map_height)
        .map(|y| TilePos::new(16, y))
        .collect();
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Strike, 400);

    let decision = think(&mut planner, &battle, &intel);

    assert_eq!(
        planner
            .air_operation()
            .and_then(|operation| operation.recovery_reason()),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { units, .. } | Intent::AttackMoveUnits { units, .. }
            if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
    )));
}

#[test]
fn strike_aborts_before_bomber_commitment_when_staging_recon_loses_its_air_route() {
    let mut battle = obs(400);
    battle.my_units[0].tile = HOME;
    battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Strike, battle.tick);

    let decision = think(&mut planner, &battle, &intel);
    let operation = planner
        .air_operation()
        .expect("the refused strike remains observable during recovery");

    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert_eq!(operation.strike_issued_at, None);
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn recovery_releases_a_survivor_stranded_behind_known_peaks() {
    let mut battle = obs(400);
    battle.known_peaks = (0..battle.map_height)
        .map(|y| TilePos::new(16, y))
        .collect();
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Recover, 400);

    let decision = think(&mut planner, &battle, &intel);

    assert!(!decision.reservations.contains(&UnitId(1)));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, .. } if units.contains(&UnitId(1))
    )));
}

#[test]
fn connected_recovery_releases_a_survivor_stranded_by_public_peaks() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let battle = obs(400);
    let public_map = public_map_with_terrain(
        &battle,
        (0..battle.map_height).map(|y| (TilePos::new(16, y), Terrain::Peak)),
    );
    let intel = knowledge(&battle);
    let identity = profile();
    let mut planner = with_operation(AirOperationPhase::Recover, battle.tick);

    let decision = planner
        .think_alone_with(
            &identity,
            DifficultyTuning::for_level(identity.difficulty),
            &battle,
            &intel,
            HOME,
            StrategicCoordination {
                planning: Some(&fixture_planning),
                public_map: Some(&public_map),
                ..coordination(&fixture_planning, None)
            },
        )
        .decision;

    assert!(!decision.reservations.contains(&UnitId(1)));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, .. } if units.contains(&UnitId(1))
    )));
}

#[test]
fn attack_move_fallback_requires_current_corridor_sight() {
    let mut visible = obs(400);
    visible.enemy_buildings.clear();
    see_approach(&mut visible);
    explore(&mut visible, staging(HOME, TARGET));
    let intel = knowledge(&visible);
    let mut planner = with_operation(AirOperationPhase::Strike, 400);
    assert!(
        think(&mut planner, &visible, &intel)
            .intents
            .contains(&Intent::AttackMoveUnits {
                units: vec![UnitId(3), UnitId(4)],
                goal: TARGET,
            })
    );

    let mut dark = visible;
    dark.visible.fill(false);
    let intel = knowledge(&dark);
    let mut planner = with_operation(AirOperationPhase::Strike, 400);
    assert!(
        !think(&mut planner, &dark, &intel)
            .intents
            .iter()
            .any(|intent| matches!(intent, Intent::AttackMoveUnits { .. }))
    );
}

#[test]
fn a_non_air_identity_retains_the_connected_operation_repertoire() {
    let mut observation = developed_connected_obs(120);
    observation.my_units.retain(|unit| unit.id != UnitId(2));
    observation
        .my_units
        .push(own(14, UnitKind::Sentinel, TilePos::new(8, 10)));
    observation.my_units.sort_unstable_by_key(|unit| unit.id);
    let intel = knowledge(&observation);
    let mut identity = profile();
    identity.primary = Specialty::Support;
    identity.secondary = Specialty::Greed;
    identity.traits.air = 48;
    identity.traits.siege = 52;
    let mut planner = StrategicPlanner::new();

    let decision = planner.think_alone(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intel,
        HOME,
        &[],
    );

    assert!(planner.air_operation().is_some());
    assert!(planner.air_plan().is_some_and(|plan| {
        plan.connected_package.as_ref().is_some_and(|package| {
            !package.recon.is_empty()
                && !package.suppression.is_empty()
                && !package.strike.is_empty()
        })
    }));
    assert!(
        !decision.reservations.is_empty() || !decision.intents.is_empty(),
        "the personality may change emphasis but cannot gate the operation"
    );
}

#[test]
fn resolved_siege_identity_enters_the_playbook_with_a_complete_repertoire() {
    let mut observation = developed_connected_obs(120);
    observation.my_units.retain(|unit| unit.id != UnitId(2));
    observation
        .my_units
        .push(own(14, UnitKind::Sentinel, TilePos::new(8, 10)));
    observation.my_units.sort_unstable_by_key(|unit| unit.id);
    let intel = knowledge(&observation);
    let siege = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_043,
    ));
    let low_siege = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_042,
    ));
    assert_eq!(
        (siege.primary, siege.secondary),
        (Specialty::Siege, Specialty::Guile)
    );
    assert_eq!(
        (low_siege.primary, low_siege.secondary),
        (Specialty::Support, Specialty::Greed)
    );

    let mut siege_planner = StrategicPlanner::new();
    siege_planner.think_alone(
        &siege,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intel,
        HOME,
        &[],
    );
    let siege_plan = siege_planner
        .air_plan()
        .expect("the resolved Siege identity enters the connected playbook");
    assert!(
        siege_plan
            .connected_package
            .as_ref()
            .is_some_and(|package| {
                !package.recon.is_empty()
                    && !package.suppression.is_empty()
                    && !package.strike.is_empty()
            })
    );

    let mut low_planner = StrategicPlanner::new();
    low_planner.think_alone(
        &low_siege,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intel,
        HOME,
        &[],
    );
    assert!(
        low_planner.air_operation().is_some(),
        "specialty changes marginal emphasis, not access to the playbook"
    );
}

#[test]
fn connected_combined_operation_waits_for_a_mature_fighting_roster() {
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Standard,
        BotStance::Balanced,
        1_616_301,
    ));
    assert_eq!(
        (identity.primary, identity.secondary),
        (Specialty::Guile, Specialty::Siege),
        "the replay-shaped identity must qualify for the combined playbook"
    );
    let mut immature = obs(120);
    immature.visible.fill(true);
    immature.explored.fill(true);
    immature.scrap = 10_000;
    immature.my_buildings = vec![
        building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), false),
        building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), false),
        building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), false),
    ];
    immature.my_queues = vec![Vec::new(); immature.my_buildings.len()];
    immature
        .my_units
        .extend((5..=12).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
    immature.my_units.sort_unstable_by_key(|unit| unit.id);
    assert!(derived_connected_test_plan(&identity, &immature).is_some());
    assert_eq!(combat_roster(&immature), 11);

    let tuning = DifficultyTuning::for_level(BotDifficulty::Standard);
    let mut planner = StrategicPlanner::new();
    let immature_intelligence = knowledge(&immature);
    let held = planner.think_alone(
        &identity,
        tuning,
        &immature,
        &immature_intelligence,
        HOME,
        &[],
    );

    assert_eq!(held, StrategicDecision::default());
    assert!(planner.air_operation().is_none());

    let mut mature = immature;
    mature.tick = 144;
    mature
        .my_units
        .push(own(13, UnitKind::Sentinel, TilePos::new(8, 10)));
    mature.my_units.sort_unstable_by_key(|unit| unit.id);
    assert_eq!(combat_roster(&mature), 12);
    let mature_intelligence = knowledge(&mature);
    let admitted = planner.think_alone(&identity, tuning, &mature, &mature_intelligence, HOME, &[]);

    assert!(planner.air_operation().is_some());
    assert!(
        !admitted.reservations.is_empty() || !admitted.intents.is_empty(),
        "the mature roster should admit a complete connected package: {admitted:?}"
    );
}

#[test]
fn connected_admission_tries_a_reachable_current_target_after_the_best_is_cut_off() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = developed_connected_obs(120);
    let reachable = TilePos::new(12, 10);
    battle.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, TARGET, true),
        building(81, 1, BuildingKind::Foundry, reachable, true),
    ];
    let public_map = public_map_with_terrain(
        &battle,
        (0..battle.map_height).map(|y| (TilePos::new(18, y), Terrain::Peak)),
    );
    let intelligence = knowledge(&battle);
    let candidates = select_target_candidates(&intelligence, battle.tick, u64::MAX);
    assert_eq!(
        candidates.first().map(|target| target.anchor),
        Some(TARGET),
        "the disconnected Crucible remains the highest-value candidate"
    );

    let mut planner = StrategicPlanner::new();
    let result = planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &battle,
        &intelligence,
        HOME,
        StrategicCoordination {
            planning: Some(&fixture_planning),
            public_map: Some(&public_map),
            ..coordination(&fixture_planning, None)
        },
    );

    assert_eq!(
        planner.air_operation().map(|operation| operation.target),
        Some(reachable)
    );
    assert!(result.rejected_connected_candidate.is_none());
}

#[test]
fn precommit_package_rebase_preserves_every_surviving_frozen_target() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let admitted_at = 120;
    let left_survivor = TARGET.offset(-3, 0);
    let right_survivor = TARGET.offset(3, 0);
    let mut initial = developed_connected_obs(admitted_at);
    initial.scrap = 50_000;
    initial.enemy_buildings.extend([
        building(81, 1, BuildingKind::Turret, left_survivor, true),
        building(82, 1, BuildingKind::Turret, right_survivor, true),
    ]);
    initial
        .enemy_buildings
        .sort_unstable_by_key(|building| building.id);
    let mut intelligence = knowledge(&initial);
    let initial_target = intelligence
        .buildings()
        .iter()
        .find(|building| building.anchor == TARGET)
        .expect("the original target is current");
    let initial_resources = ConnectedProductionResources::from_observation(
        &initial,
        initial_target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home: HOME,
            target: TARGET,
            public_map: None,
            orientation: test_orientation(),
        },
    );
    assert_eq!(
        initial_resources.targets.target_anchors,
        vec![left_survivor, TARGET, right_survivor]
    );
    assert_eq!(
        initial_resources.targets.growth_order,
        vec![left_survivor, right_survivor]
    );
    let identity = profile();
    let full_package = derive_connected_package_options_for_targets(
        &identity,
        &initial,
        initial_target,
        &[],
        &initial_resources.targets,
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home: HOME,
            target: TARGET,
            public_map: None,
            orientation: test_orientation(),
        },
        ConnectedPlanningContext {
            planning: Some(&fixture_planning),
            minimum_only: false,
            campaign_routes: None,
            orientation: test_orientation(),
            public_map: None,
            resources: &initial_resources,
            preferred_artillery: &[],
            protected_current_scrap: 0,
            preparation: PreparationConstraints {
                deadline: initial.tick.saturating_add(CONNECTED_PREPARATION_HORIZON),
                decision_cadence: DifficultyTuning::for_level(identity.difficulty).cadence,
                protected_forecast_scrap: 0,
            },
        },
    );
    let plan = AirPlan::connected(
        full_package
            .map(ConnectedForcePackageOptions::into_largest)
            .unwrap_or_else(|reason| {
                panic!("the complete initial package is feasible: {reason:?}")
            }),
        admitted_at,
        TARGET,
    );
    assert_eq!(
        plan.connected_package
            .as_ref()
            .expect("the fixture begins with a connected package")
            .target_anchors,
        vec![left_survivor, TARGET, right_survivor]
    );
    let mut planner =
        planner_with_operation(operation(AirOperationPhase::Assemble, admitted_at), plan);

    let mut after_destruction = initial;
    after_destruction.tick += 12;
    after_destruction
        .enemy_buildings
        .retain(|building| building.anchor != TARGET);
    let tempting_new_target = TARGET.offset(-3, 3);
    after_destruction.enemy_buildings.push(building(
        79,
        1,
        BuildingKind::Foundry,
        tempting_new_target,
        true,
    ));
    after_destruction
        .enemy_buildings
        .sort_unstable_by_key(|building| building.id);
    intelligence.update(&after_destruction);
    let decision = think(&mut planner, &after_destruction, &intelligence);

    let active = planner
        .air
        .as_ref()
        .expect("the surviving admitted target keeps preparation active");
    assert_ne!(active.op.phase(), AirOperationPhase::Recover);
    assert_eq!(active.op.recovery_reason(), None);
    assert_eq!(active.op.target, left_survivor);
    assert_eq!(active.op.target_kind, BuildingKind::Turret);
    assert_eq!(active.op.target_id, Some(BuildingId(81)));
    let revised = active
        .plan
        .connected_package
        .as_ref()
        .expect("the surviving target retains a connected package");
    assert_eq!(revised.derived_at, after_destruction.tick);
    assert_eq!(revised.target_anchors, vec![left_survivor, right_survivor]);
    assert!(
        !revised.target_anchors.contains(&tempting_new_target),
        "revision may prune the frozen set but cannot regrow it around the new primary"
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn adjudicated_precommit_rebase_preserves_exact_package_and_surviving_targets() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let admitted_at = 120;
    let left_survivor = TARGET.offset(-3, 0);
    let right_survivor = TARGET.offset(3, 0);
    let tempting_new_target = TARGET.offset(-3, 3);
    let mut initial = production_hungry_connected_obs(admitted_at, 50_000);
    initial.enemy_buildings.extend([
        building(81, 1, BuildingKind::Turret, left_survivor, true),
        building(82, 1, BuildingKind::Turret, right_survivor, true),
    ]);
    initial
        .enemy_buildings
        .sort_unstable_by_key(|building| building.id);
    let mut intelligence = knowledge(&initial);
    let resources = ResourceSnapshot::from_observation(&initial);
    let mut proposal = StrategicPlanner::new()
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &initial,
                &resources,
                &intelligence,
                HOME,
                coordination(&fixture_planning, None),
            ),
        )
        .expect("the complete current cluster is admissible")
        .expect("the current cluster produces an exact proposal");
    let richest = proposal.marginal_variants().last().cloned();
    if let Some(richest) = richest.as_ref() {
        assert!(proposal.select_marginal(richest));
    }
    assert_eq!(
        proposal.variants[proposal.selected_variant]
            .active
            .plan
            .connected_package
            .as_ref()
            .expect("the proposal owns an exact package")
            .target_anchors,
        vec![left_survivor, TARGET, right_survivor]
    );
    let mut expected_package = proposal.variants[proposal.selected_variant]
        .active
        .plan
        .connected_package
        .clone()
        .expect("the selected proposal retains its exact package");
    let mut planner = StrategicPlanner::new();
    planner.commit_connected(proposal);
    let _ = planner.think_after_connected_adjudication(StrategicThinkContext::new(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &initial,
        &intelligence,
        HOME,
        StrategicCoordination {
            planning: Some(&fixture_planning),
            allow_new_operation: false,
            ..coordination(&fixture_planning, None)
        },
    ));

    let mut after_destruction = initial;
    after_destruction.tick += 12;
    after_destruction
        .enemy_buildings
        .retain(|building| building.anchor != TARGET);
    after_destruction.enemy_buildings.push(building(
        79,
        1,
        BuildingKind::Foundry,
        tempting_new_target,
        true,
    ));
    after_destruction
        .enemy_buildings
        .sort_unstable_by_key(|building| building.id);
    intelligence.update(&after_destruction);
    let decision = planner.think_after_connected_adjudication(StrategicThinkContext::new(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &after_destruction,
        &intelligence,
        HOME,
        StrategicCoordination {
            planning: Some(&fixture_planning),
            allow_new_operation: false,
            ..coordination(&fixture_planning, None)
        },
    ));

    let rebased_identity = ConnectedOffenseIdentity::new(BuildingId(81), left_survivor);
    expected_package.target_anchors = vec![left_survivor, right_survivor];

    let active = planner
        .air
        .as_ref()
        .expect("the surviving frozen targets keep preparation active");
    assert_ne!(active.op.phase(), AirOperationPhase::Recover);
    assert_eq!(active.op.recovery_reason(), None);
    assert_eq!(active.op.target, left_survivor);
    assert_eq!(active.op.target_kind, BuildingKind::Turret);
    assert_eq!(active.op.target_id, Some(BuildingId(81)));
    assert_eq!(
        active.plan.connected_package.as_ref(),
        Some(&expected_package),
        "adjudicated continuation may prune destroyed targets and rebind owner identity, but cannot rerank or rederive its exact package"
    );
    assert!(
        !expected_package
            .target_anchors
            .contains(&tempting_new_target),
        "revision cannot admit a newly observed target near the rebased primary"
    );
    let obligation = active_obligation(&mut planner, &after_destruction)
        .expect("the rebased operation retains its persistent obligation");
    assert_eq!(obligation.identity(), rebased_identity);

    assert!(decision.decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn fresh_connected_proposal_is_pure_repeatable_and_keeps_one_minimum_basis() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let battle = production_hungry_connected_obs(120, 10_000);
    let intelligence = knowledge(&battle);
    let resources = ResourceSnapshot::from_observation(&battle);
    let planner = StrategicPlanner::new();
    let before = planner.clone();
    let propose = || {
        planner
            .fresh_connected_minimum_proposal(
                &crate::experience::Experience::default(),
                FreshConnectedProposalRequest::new(
                    &profile(),
                    DifficultyTuning::for_level(BotDifficulty::Prime),
                    &battle,
                    &resources,
                    &intelligence,
                    HOME,
                    coordination(&fixture_planning, None),
                ),
            )
            .expect("the current connected opportunity is admissible")
            .expect("the current connected opportunity produces a proposal")
    };

    let first = propose();
    let second = propose();

    assert_eq!(first, second);
    assert_eq!(
        planner, before,
        "proposal derivation must not mutate planner state"
    );
    assert_eq!(first.objective(), BuildingId(80));
    assert_eq!(first.anchor(), TARGET);
    assert_eq!(
        first.deadline(),
        battle.tick.saturating_add(connected_preparation_horizon())
    );
    let minimum = &first.variants[0];
    let minimum_package = minimum
        .active
        .plan
        .connected_package
        .as_ref()
        .expect("every proposal variant has a connected package");
    assert!(!minimum.claims.provider_jobs.is_empty());
    for variant in &first.variants {
        let package = variant
            .active
            .plan
            .connected_package
            .as_ref()
            .expect("every proposal variant has a connected package");
        assert_eq!(package.target_anchors, minimum_package.target_anchors);
        assert_eq!(
            package.preparation_deadline,
            minimum_package.preparation_deadline
        );
        assert!(
            package
                .provider_priority
                .starts_with(&minimum_package.provider_priority)
        );
        assert!(variant.claims.units.starts_with(&minimum.claims.units));
        assert!(
            variant
                .claims
                .provider_jobs
                .starts_with(&minimum.claims.provider_jobs)
        );
    }
}

#[test]
fn reacquired_remembered_target_requires_fresh_connected_adjudication() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let reveal_ground_route = |observation: &mut Observation| {
        observation.known_rock.clear();
        observation.my_buildings.push(building(
            40,
            0,
            BuildingKind::Fabricator,
            TilePos::new(12, 2),
            true,
        ));
        observation.my_queues.push(Vec::new());
        for x in HOME.x + 2..TARGET.x {
            explore(observation, TilePos::new(x, HOME.y));
        }
    };
    let first = {
        let mut observation = wealthy_island_obs(4_800, 1);
        reveal_ground_route(&mut observation);
        observation
    };
    let mut intelligence = knowledge(&first);
    let mut hidden = wealthy_island_obs(4_992, 1);
    reveal_ground_route(&mut hidden);
    hidden.enemy_buildings[0].seen = false;
    hidden.my_units[0].tile = HOME;
    intelligence.update(&hidden);
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_045,
    ));
    let tuning = DifficultyTuning::for_level(identity.difficulty);
    let mut planner = StrategicPlanner::new();
    planner.think_alone(&identity, tuning, &hidden, &intelligence, HOME, &[]);
    assert!(planner.air_operation().is_some_and(|operation| {
        !operation.assault_admitted() && operation.target_id == Some(BuildingId(80))
    }));

    let mut current = wealthy_island_obs(5_016, 1);
    reveal_ground_route(&mut current);
    intelligence.update(&current);
    let resources = ResourceSnapshot::from_observation(&current);
    let before_proposal = planner.clone();
    let proposal = planner
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &identity,
                tuning,
                &current,
                &resources,
                &intelligence,
                HOME,
                coordination(&fixture_planning, None),
            ),
        )
        .expect("the reacquired connected objective is admissible")
        .expect("reacquisition produces a fresh proposal");
    assert_eq!(proposal.accepted_at(), hidden.tick);
    assert_eq!(planner, before_proposal);

    let mut without_acceptance = planner.clone();
    without_acceptance.think_after_connected_adjudication(StrategicThinkContext::new(
        &identity,
        tuning,
        &current,
        &intelligence,
        HOME,
        coordination(&fixture_planning, None),
    ));
    assert!(without_acceptance.air_operation().is_some_and(|operation| {
        !operation.assault_admitted() && operation.target_id == Some(BuildingId(80))
    }));

    planner.commit_connected(proposal);
    assert!(planner.air_operation().is_some_and(|operation| {
        operation.assault_admitted() && operation.target_id == Some(BuildingId(80))
    }));
    assert_eq!(planner.air_admitted_at(), Some(hidden.tick));
    assert_eq!(
        active_obligation(&mut planner, &current)
            .expect("the promoted assault exports its persistent claims")
            .accepted_at(),
        hidden.tick,
        "persistent allocation must retain reconnaissance admission rather than promotion time"
    );
}

#[test]
fn fresh_connected_proposal_uses_the_coordinators_exact_resource_snapshot() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let battle = production_hungry_connected_obs(120, 10_000);
    let intelligence = knowledge(&battle);
    let mut unfunded_evidence = battle.clone();
    unfunded_evidence.scrap = 0;
    let resources = ResourceSnapshot::from_observation(&unfunded_evidence);

    let result = StrategicPlanner::new().fresh_connected_minimum_proposal(
        &crate::experience::Experience::default(),
        FreshConnectedProposalRequest::new(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &resources,
            &intelligence,
            HOME,
            coordination(&fixture_planning, None),
        ),
    );

    assert!(
        result.is_err(),
        "strategy must not reconstruct the rich bank from Observation behind the coordinator"
    );
}

#[test]
fn connected_scout_credit_keeps_the_unowned_queue_occurrence_identity() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = production_hungry_connected_obs(120, 1000);
    battle
        .my_units
        .retain(|unit| unit.kind != UnitKind::Kestrel);
    let factory = battle
        .my_buildings
        .iter()
        .position(|building| building.kind == BuildingKind::Airworks)
        .unwrap();
    let producer = battle.my_buildings[factory].id;
    battle.my_queues[factory] = vec![UnitKind::Kestrel, UnitKind::Kestrel];
    let intelligence = knowledge(&battle);
    let resources = ResourceSnapshot::from_observation(&battle);
    let proposal = StrategicPlanner::new()
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                coordination(&fixture_planning, None),
            )
            .with_paid_exclusions(&[(producer, UnitKind::Kestrel, 0)]),
        )
        .unwrap()
        .unwrap();
    let scouts: Vec<_> = proposal
        .minimum_claims()
        .paid_providers()
        .iter()
        .filter(|provider| provider.kind() == UnitKind::Kestrel)
        .map(|provider| (provider.producer(), provider.occurrence()))
        .collect();
    assert_eq!(scouts, [(producer, 1)]);
    assert!(
        proposal
            .minimum_claims()
            .provider_jobs()
            .iter()
            .all(|job| job.kind() != UnitKind::Kestrel)
    );
    let mut planner = StrategicPlanner::new();
    planner.air = Some(proposal.variants[0].active.clone());
    assert_eq!(
        planner.reconnaissance_paid_claims(
            &battle,
            &resources,
            &[(producer, UnitKind::Kestrel, 0)]
        ),
        [super::super::allocation::PaidQueueClaim {
            producer,
            kind: UnitKind::Kestrel,
            occurrence: 1
        }]
    );
}

#[test]
fn connected_package_funds_a_scout_when_reconnaissance_holds_the_only_queued_one() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = production_hungry_connected_obs(120, 1000);
    battle
        .my_units
        .retain(|unit| unit.kind != UnitKind::Kestrel);
    let factory = battle
        .my_buildings
        .iter()
        .position(|building| building.kind == BuildingKind::Airworks)
        .unwrap();
    let producer = battle.my_buildings[factory].id;
    battle.my_queues[factory] = vec![UnitKind::Kestrel];
    let intelligence = knowledge(&battle);
    let resources = ResourceSnapshot::from_observation(&battle);
    let proposal = StrategicPlanner::new()
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                coordination(&fixture_planning, None),
            )
            .with_paid_exclusions(&[(producer, UnitKind::Kestrel, 0)]),
        )
        .unwrap()
        .unwrap();
    let claims = proposal.minimum_claims();
    assert!(
        claims
            .paid_providers()
            .iter()
            .all(|provider| provider.kind() != UnitKind::Kestrel),
        "the reconnaissance-held occurrence must not be leaned on"
    );
    assert!(
        claims
            .provider_jobs()
            .iter()
            .any(|job| job.kind() == UnitKind::Kestrel),
        "the package funds its own scout instead"
    );
}

#[test]
fn fresh_connected_proposal_falls_back_without_committing_the_rejected_target() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = production_hungry_connected_obs(120, 10_000);
    let reachable = TilePos::new(12, 10);
    battle.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, TARGET, true),
        building(81, 1, BuildingKind::Foundry, reachable, true),
    ];
    let public_map = public_map_with_terrain(
        &battle,
        (0..battle.map_height).map(|y| (TilePos::new(18, y), Terrain::Peak)),
    );
    let intelligence = knowledge(&battle);
    let resources = ResourceSnapshot::from_observation(&battle);
    let planner = StrategicPlanner::new();
    let before = planner.clone();

    let proposal = planner
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                StrategicCoordination {
                    planning: Some(&fixture_planning),
                    public_map: Some(&public_map),
                    ..coordination(&fixture_planning, None)
                },
            ),
        )
        .expect("the lower-ranked reachable target remains admissible")
        .expect("the lower-ranked reachable target produces a proposal");

    assert_eq!(proposal.objective(), BuildingId(81));
    assert_eq!(proposal.anchor(), reachable);
    assert_eq!(planner, before);
}

#[test]
fn connected_proposal_uses_completed_income_without_double_counting_provider_costs() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = production_hungry_connected_obs(120, 0);
    for (id, anchor) in [(20, TilePos::new(2, 15)), (21, TilePos::new(8, 15))] {
        battle
            .my_buildings
            .push(building(id, 0, BuildingKind::Extractor, anchor, true));
        battle.my_queues.push(Vec::new());
    }
    let intelligence = knowledge(&battle);
    let resources = ResourceSnapshot::from_observation(&battle);
    let proposal = StrategicPlanner::new()
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                coordination(&fixture_planning, None),
            ),
        )
        .expect("completed Extractors make the minimum forecast-feasible")
        .expect("the forecast-funded operation produces a proposal");
    let minimum = &proposal.variants[0];
    let package = minimum
        .active
        .plan
        .connected_package
        .as_ref()
        .expect("the proposal retains its exact package");

    assert_eq!(
        minimum.claims.provider_jobs.len(),
        package.funded_providers.len()
    );
    assert!(
        minimum
            .claims
            .provider_jobs
            .iter()
            .any(|job| job.enqueue_not_before > battle.tick),
        "zero current bank requires at least one forecast-funded command boundary"
    );
    assert_eq!(
        minimum
            .claims
            .provider_jobs
            .iter()
            .map(|job| job.kind.stats().cost)
            .sum::<u32>(),
        package
            .funded_providers
            .iter()
            .map(|provider| provider.kind.stats().cost)
            .sum::<u32>(),
        "every unpaid provider cost appears exactly once in the shared claim surface"
    );
    for (job, funded) in minimum
        .claims
        .provider_jobs
        .iter()
        .zip(&package.funded_providers)
    {
        assert_eq!(
            (job.kind, job.enqueue_not_before),
            (funded.kind, funded.command_tick)
        );
    }
}

#[test]
fn connected_claims_retain_only_the_paid_queue_occurrences_the_package_uses() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = production_hungry_connected_obs(120, 10_000);
    let fabricator = battle
        .my_buildings
        .iter()
        .position(|building| building.id == BuildingId(10))
        .expect("the fixture has one Fabricator");
    battle.my_queues[fabricator] = vec![UnitKind::Bombard, UnitKind::Bombard];
    let intelligence = knowledge(&battle);
    let resources = ResourceSnapshot::from_observation(&battle);

    let proposal = StrategicPlanner::new()
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                coordination(&fixture_planning, None),
            ),
        )
        .expect("the paid Bombard makes the minimum feasible")
        .expect("the current objective produces a connected proposal");

    assert_eq!(
        proposal
            .minimum_claims()
            .paid_providers()
            .iter()
            .map(|provider| (provider.producer(), provider.kind()))
            .collect::<Vec<_>>(),
        vec![(BuildingId(10), UnitKind::Bombard)],
        "the minimum uses one paid Bombard, leaving the second queue occurrence ordinary"
    );
    assert!(
        proposal
            .minimum_claims()
            .provider_jobs()
            .iter()
            .all(|job| job.kind() != UnitKind::Bombard),
        "already-paid work cannot appear again as an unpaid provider claim"
    );
}

#[test]
fn protected_current_scrap_monotonically_reduces_connected_scaling() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut rich = production_hungry_connected_obs(120, 10_000);
    rich.enemy_buildings.extend([
        building(81, 1, BuildingKind::Foundry, TARGET.offset(-2, -2), true),
        building(82, 1, BuildingKind::Airworks, TARGET.offset(-2, 2), true),
        building(83, 1, BuildingKind::FlakTurret, TARGET.offset(-4, 0), true),
    ]);
    let intelligence = knowledge(&rich);
    let rich_resources = ResourceSnapshot::from_observation(&rich);
    let rich_proposal = StrategicPlanner::new()
        .fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &rich,
                &rich_resources,
                &intelligence,
                HOME,
                coordination(&fixture_planning, None),
            ),
        )
        .expect("the rich opportunity is admissible")
        .expect("the rich opportunity produces a proposal");
    let richest_cost = rich_proposal
        .variants
        .last()
        .expect("the minimum is always present")
        .claims
        .provider_jobs
        .iter()
        .map(|job| job.kind.stats().cost)
        .sum::<u32>();
    let minimum_cost = rich_proposal
        .minimum_claims()
        .provider_jobs()
        .iter()
        .map(|job| job.kind().stats().cost)
        .sum::<u32>();
    assert!(richest_cost > minimum_cost);

    rich.scrap = richest_cost;
    let exact_resources = ResourceSnapshot::from_observation(&rich);
    let derive_with_reserve = |reserve| {
        StrategicPlanner::new().fresh_connected_minimum_proposal(
            &crate::experience::Experience::default(),
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &rich,
                &exact_resources,
                &intelligence,
                HOME,
                StrategicCoordination {
                    planning: Some(&fixture_planning),
                    protected_current_scrap: reserve,
                    ..coordination(&fixture_planning, None)
                },
            ),
        )
    };
    let unreserved = derive_with_reserve(0)
        .expect("the exact full-package bank is admissible")
        .expect("the exact full-package bank produces a proposal");
    let minimum_only = derive_with_reserve(richest_cost - minimum_cost)
        .expect("protecting only optional capital preserves the minimum")
        .expect("the retained minimum still produces a proposal");

    assert_eq!(minimum_only.minimum_claims(), unreserved.minimum_claims());
    assert!(minimum_only.variants.len() < unreserved.variants.len());
    assert_eq!(minimum_only.variants.len(), 1);
    assert!(derive_with_reserve(richest_cost).is_err());
}

#[test]
fn connected_admission_reports_the_best_current_target_when_every_candidate_is_rejected() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = developed_connected_obs(120);
    let lower_value = TARGET.offset(0, 5);
    battle.map_height = 26;
    battle.visible = vec![true; usize::try_from(battle.map_width * battle.map_height).unwrap()];
    battle.explored = battle.visible.clone();
    battle.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, TARGET, true),
        building(81, 1, BuildingKind::Foundry, lower_value, true),
    ];
    let public_map = public_map_with_terrain(
        &battle,
        (0..battle.map_height).map(|y| (TilePos::new(18, y), Terrain::Peak)),
    );
    let intelligence = knowledge(&battle);
    let mut planner = StrategicPlanner::new();

    let result = planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &battle,
        &intelligence,
        HOME,
        StrategicCoordination {
            planning: Some(&fixture_planning),
            public_map: Some(&public_map),
            ..coordination(&fixture_planning, None)
        },
    );

    let rejected = result
        .rejected_connected_candidate
        .expect("the best rejected candidate remains diagnostic evidence");
    assert_eq!(rejected.target.anchor, TARGET);
    assert_eq!(
        rejected.reason,
        ConnectedPlanRejection::DisconnectedGroundRoute
    );
    assert!(planner.air_operation().is_none());
}

#[test]
fn current_connected_candidate_reports_the_standing_force_gate_only_at_admission() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut on_boundary = obs(120);
    see_approach(&mut on_boundary);
    let current = combat_roster(&on_boundary);
    assert!(current < CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER);
    let intelligence = knowledge(&on_boundary);
    let mut planner = StrategicPlanner::new();

    let rejected = planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &on_boundary,
        &intelligence,
        HOME,
        coordination(&fixture_planning, None),
    );

    assert_eq!(rejected.decision, StrategicDecision::default());
    assert_eq!(
        rejected.rejected_connected_candidate,
        Some(RejectedConnectedCandidate {
            target: intelligence.buildings()[0].clone(),
            reason: ConnectedPlanRejection::InsufficientStandingForce {
                current,
                required: CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER,
            },
        })
    );
    assert!(planner.air_operation().is_none());

    let mut off_boundary = on_boundary;
    off_boundary.tick = 121;
    let intelligence = knowledge(&off_boundary);
    let mut planner = StrategicPlanner::new();
    let not_considered = planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &off_boundary,
        &intelligence,
        HOME,
        coordination(&fixture_planning, None),
    );
    assert!(not_considered.rejected_connected_candidate.is_none());
    assert!(planner.air_operation().is_none());
}

#[test]
fn no_target_is_idle_without_fabricating_a_connected_rejection() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = developed_connected_obs(120);
    observation.enemy_buildings.clear();
    let intelligence = knowledge(&observation);
    let mut planner = StrategicPlanner::new();

    let result = planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intelligence,
        HOME,
        coordination(&fixture_planning, None),
    );

    assert_eq!(result, StrategicThinkResult::default());
    assert!(planner.air_operation().is_none());
}

#[test]
fn current_connected_candidate_reports_a_disconnected_ground_route() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = developed_connected_obs(120);
    observation.known_rock = (0..observation.map_height)
        .map(|y| TilePos::new(16, y))
        .collect();
    let intelligence = knowledge(&observation);
    let target = intelligence.buildings()[0].clone();
    let mut planner = StrategicPlanner::new();

    let result = planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intelligence,
        HOME,
        coordination(&fixture_planning, None),
    );

    assert_eq!(
        result.rejected_connected_candidate,
        Some(RejectedConnectedCandidate {
            target,
            reason: ConnectedPlanRejection::DisconnectedGroundRoute,
        })
    );
    assert!(planner.air_operation().is_none());
}

#[test]
fn current_connected_candidate_reports_a_missing_completed_provider() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = developed_connected_obs(120);
    observation.my_buildings.clear();
    observation.my_queues.clear();
    let intelligence = knowledge(&observation);
    let target = intelligence.buildings()[0].clone();
    let mut planner = StrategicPlanner::new();
    let enlisted = [UnitId(1), UnitId(2), UnitId(3), UnitId(4)];

    let result = planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intelligence,
        HOME,
        StrategicCoordination {
            planning: Some(&fixture_planning),
            enlisted: &enlisted,
            ..coordination(&fixture_planning, None)
        },
    );

    assert_eq!(
        result.rejected_connected_candidate,
        Some(RejectedConnectedCandidate {
            target,
            reason: ConnectedPlanRejection::Package {
                reason: ForcePackageRejection::MissingCompletedProviderCapability {
                    family: force_package::ForceFamily::Recon,
                },
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
            },
        })
    );
    assert!(planner.air_operation().is_none());
}

#[test]
fn connected_production_rejects_a_route_beyond_the_movement_search_cap() {
    let mut observation = obs(120);
    observation.map_width = 256;
    observation.map_height = 256;
    observation.visible = vec![true; 256 * 256];
    observation.explored = observation.visible.clone();
    observation.my_units.clear();
    observation.my_buildings = vec![building(
        10,
        0,
        BuildingKind::Fabricator,
        TilePos::new(253, 253),
        true,
    )];
    observation.my_queues = vec![Vec::new()];
    observation.enemy_buildings.clear();
    let open = |tile: TilePos| {
        tile.y % 2 == 0
            || (tile.y % 4 == 1 && tile.x == observation.map_width - 1)
            || (tile.y % 4 == 3 && tile.x == 0)
            || tile == TilePos::new(255, 255)
    };
    let public_map = public_map_with_terrain(
        &observation,
        (0..observation.map_height).flat_map(|y| {
            (0..observation.map_width).filter_map(move |x| {
                let tile = TilePos::new(x, y);
                (!open(tile)).then_some((tile, Terrain::Pit))
            })
        }),
    );
    let home = TilePos::new(0, 84);
    let target = TilePos::new(3, 84);
    let orientation = Orientation::for_home(&observation, home);
    assert!(orientation.is_identity());
    let intelligence = knowledge(&observation);
    let targets = ConnectedTargetSelection {
        target_anchors: vec![target],
        suppression_targets: Vec::new(),
        growth_order: Vec::new(),
    };
    let resources = ResourceSnapshot::from_observation(&observation);
    let producer = &observation.my_buildings[0];
    let spawn = production_spawn_doorstep(
        QueryPurpose::NavigationTest,
        &observation,
        producer,
        Some(&public_map),
        Some(orientation),
    )
    .expect("the Fabricator has an open south-east doorstep");
    let staging = connected_artillery_staging_goal(&observation, home, target, Some(&public_map))
        .expect("the Foundry-side staging tile is in the same component");
    let projected = route_projection_with_orientation(
        &observation,
        Domain::Ground,
        Some(&public_map),
        orientation,
    );
    assert!(
        projected.reaches(spawn, staging),
        "the uncapped component flood sees the complete serpentine"
    );
    assert!(
        !projected.ground_command_reaches(spawn, staging),
        "the movement command cannot traverse that component within its bounded A* search"
    );

    let access = connected_production_access(
        &observation,
        &targets,
        &resources,
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home,
            target,
            public_map: Some(&public_map),
            orientation,
        },
    );

    assert!(
        !access.allows(BuildingId(10), UnitKind::Bombard),
        "an operation must not buy artillery whose authoritative route will exhaust"
    );
}

#[test]
fn connected_operation_excludes_live_artillery_beyond_the_movement_search_cap() {
    let mut observation = obs(120);
    observation.map_width = 256;
    observation.map_height = 256;
    observation.visible = vec![true; 256 * 256];
    observation.explored = observation.visible.clone();
    observation.my_units = vec![own(2, UnitKind::Bombard, TilePos::new(255, 255))];
    observation.my_buildings.clear();
    observation.my_queues.clear();
    observation.enemy_buildings.clear();
    let public_map = movement_cap_serpentine_map(&observation);
    let intelligence = knowledge(&observation);
    let home = TilePos::new(0, 84);
    let target = TilePos::new(3, 84);
    let staging = connected_artillery_staging_goal(&observation, home, target, Some(&public_map))
        .expect("the Foundry-side staging tile is in the same component");
    let routes = route_projection_with_orientation(
        &observation,
        Domain::Ground,
        Some(&public_map),
        test_orientation(),
    );
    assert!(
        routes.reaches(observation.my_units[0].tile, staging),
        "the uncapped component flood sees the complete serpentine"
    );
    assert!(
        !routes.ground_command_reaches(observation.my_units[0].tile, staging),
        "the live Bombard's eventual movement command exhausts its bounded search"
    );

    let unavailable = connected_provider_unavailable(
        &observation,
        &ConnectedTargetSelection {
            target_anchors: vec![target],
            suppression_targets: Vec::new(),
            growth_order: Vec::new(),
        },
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home,
            target,
            public_map: Some(&public_map),
            orientation: test_orientation(),
        },
    );

    assert_eq!(
        unavailable,
        vec![UnitId(2)],
        "a live provider must not be admitted when its staging command will exhaust"
    );
}

#[test]
fn connected_cluster_rejects_suppression_whose_staging_route_exceeds_the_command_cap() {
    let primary = TilePos::new(240, 251);
    let secondary = TilePos::new(244, 251);
    let flak = TilePos::new(249, 251);
    let home = TilePos::new(0, 0);
    let mut observation = obs(120);
    observation.map_width = 256;
    observation.map_height = 256;
    observation.visible = vec![true; 256 * 256];
    observation.explored = observation.visible.clone();
    observation.my_units = vec![
        own(1, UnitKind::Kestrel, TilePos::new(255, 254)),
        own(2, UnitKind::Bombard, TilePos::new(255, 255)),
        own(3, UnitKind::Condor, TilePos::new(254, 254)),
    ];
    observation.my_buildings.clear();
    observation.my_queues.clear();
    observation.enemy_buildings = vec![
        building(80, 1, BuildingKind::Turret, primary, true),
        building(82, 1, BuildingKind::Turret, secondary, true),
        building(81, 1, BuildingKind::FlakTurret, flak, true),
    ];
    let mut public_map = movement_cap_serpentine_map(&observation);
    public_map.non_ground_terrain = public_map
        .non_ground_terrain
        .iter()
        .copied()
        .filter(|(tile, _)| *tile != primary && *tile != secondary && *tile != flak)
        .collect();
    let intelligence = knowledge(&observation);
    assert_eq!(targetable_flak(&intelligence.air_defense_at(primary)), None);
    assert_eq!(
        targetable_flak(&intelligence.air_defense_at(secondary)),
        Some(BuildingId(81))
    );
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.anchor == primary)
        .expect("the primary target is current");
    let staging = connected_artillery_staging_goal(&observation, home, primary, Some(&public_map))
        .expect("the public component contains a staging tile");
    let origin = SuppressionOrigin {
        tile: TilePos::new(255, 255),
        kind: UnitKind::Bombard,
    };
    let routes = route_projection_with_orientation(
        &observation,
        Domain::Ground,
        Some(&public_map),
        test_orientation(),
    );
    assert!(
        routes.reaches(origin.tile, staging),
        "the uncapped component flood sees the complete serpentine"
    );
    assert!(
        !routes.ground_command_reaches(origin.tile, staging),
        "the suppression provider cannot execute its staging command"
    );
    assert!(
        suppression_targets_reachable(
            &routes,
            &observation,
            origin,
            &[Target::Building(BuildingId(81))],
            &intelligence,
            Some(&public_map),
        ),
        "the nearby firing stand is reachable, isolating the staging-leg rejection"
    );

    let selection = connected_target_selection(
        &observation,
        target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home,
            target: primary,
            public_map: Some(&public_map),
            orientation: test_orientation(),
        },
    );

    assert_eq!(selection.target_anchors, vec![primary]);
    assert!(selection.suppression_targets.is_empty());
    assert!(selection.growth_order.is_empty());
}

#[test]
fn suppression_rejects_a_firing_stand_beyond_the_movement_search_cap() {
    let mut observation = obs(120);
    observation.map_width = 256;
    observation.map_height = 256;
    observation.visible = vec![true; 256 * 256];
    observation.explored = observation.visible.clone();
    observation.known_rock.clear();
    observation.enemy_buildings = vec![building(
        81,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(250, 84),
        true,
    )];
    let public_map = movement_cap_serpentine_map(&observation);
    let intelligence = knowledge(&observation);
    let target = Target::Building(BuildingId(81));
    let local_origin = SuppressionOrigin {
        tile: TilePos::new(255, 84),
        kind: UnitKind::Bombard,
    };
    let local_routes = route_projection_with_orientation(
        &observation,
        Domain::Ground,
        Some(&public_map),
        test_orientation(),
    );
    let stand = suppression_firing_stands(
        &local_routes,
        &observation,
        local_origin,
        target,
        &intelligence,
        Some(&public_map),
    )
    .next()
    .expect("the nearby Bombard has a legal firing stand");
    let remote_origin = SuppressionOrigin {
        tile: TilePos::new(255, 255),
        kind: UnitKind::Bombard,
    };
    let component_routes = route_projection_with_orientation(
        &observation,
        Domain::Ground,
        Some(&public_map),
        test_orientation(),
    );
    assert!(
        component_routes.reaches(remote_origin.tile, stand),
        "the uncapped component flood sees the complete serpentine"
    );
    assert!(
        !component_routes.ground_command_reaches(remote_origin.tile, stand),
        "the authoritative ground command exhausts its bounded search"
    );
    let actual_routes = route_projection_with_orientation(
        &observation,
        Domain::Ground,
        Some(&public_map),
        test_orientation(),
    );
    assert_eq!(
        suppression_firing_stands(
            &actual_routes,
            &observation,
            remote_origin,
            target,
            &intelligence,
            Some(&public_map),
        )
        .next(),
        None,
        "suppression must not admit a stand its eventual movement command cannot reach"
    );
}

#[test]
fn connected_package_uses_only_producers_that_can_reach_the_operation() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = developed_connected_obs(120);
    observation
        .my_units
        .retain(|unit| unit.kind != UnitKind::Bombard);
    observation.my_buildings[0].anchor = TilePos::new(6, 3);
    observation.my_buildings[1].anchor = TilePos::new(3, 14);
    observation.my_buildings[2].kind = BuildingKind::Fabricator;
    observation.my_buildings[2].anchor = TilePos::new(10, 3);

    let isolated = observation.my_buildings[0].clone();
    let isolated_size = isolated.kind.tier_stats(isolated.tier).size;
    let isolated_spawn = oxide_sim::geometry::rect_adjacent_tiles(isolated.anchor, isolated_size)
        .min_by_key(|tile| {
            oxide_sim::geometry::spawn_doorstep_key(
                (observation.map_width, observation.map_height),
                isolated.anchor,
                isolated_size,
                *tile,
            )
        })
        .expect("the producer has a spawn ring");
    observation.known_rock.extend(
        oxide_sim::geometry::rect_adjacent_tiles(isolated.anchor, isolated_size)
            .filter(|tile| *tile != isolated_spawn),
    );
    observation.known_rock.extend(
        (-1..=1)
            .flat_map(|dy| (-1..=1).map(move |dx| isolated_spawn.offset(dx, dy)))
            .filter(|tile| *tile != isolated_spawn)
            .filter(|tile| {
                tile.x < isolated.anchor.x
                    || tile.x >= isolated.anchor.x + isolated_size.0
                    || tile.y < isolated.anchor.y
                    || tile.y >= isolated.anchor.y + isolated_size.1
            }),
    );
    observation
        .known_rock
        .sort_unstable_by_key(|tile| (tile.y, tile.x));
    observation.known_rock.dedup();

    let resources = ResourceSnapshot::from_observation(&observation);
    let isolated_timing = resources
        .producers()
        .iter()
        .find(|lane| lane.producer == BuildingId(10))
        .and_then(|lane| lane.production_timing(&[UnitKind::Bombard]))
        .expect("the isolated producer has a locally open spawn tile");
    assert_eq!(
        isolated_timing.current_egress,
        super::super::resources::ProducerEgress::Open
    );

    let intelligence = knowledge(&observation);
    let target = intelligence.buildings()[0].clone();
    let identity = profile();
    let connected_resources = ConnectedProductionResources::from_observation(
        &observation,
        &target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home: HOME,
            target: target.anchor,
            public_map: None,
            orientation: test_orientation(),
        },
    );
    let plan = connected_plan(
        &identity,
        &observation,
        &intelligence,
        HOME,
        &target,
        &[],
        ConnectedPlanningContext {
            planning: Some(&fixture_planning),
            minimum_only: false,
            campaign_routes: None,
            orientation: test_orientation(),
            public_map: None,
            resources: &connected_resources,
            preferred_artillery: &[],
            protected_current_scrap: 0,
            preparation: PreparationConstraints {
                deadline: observation.tick + CONNECTED_PREPARATION_HORIZON,
                decision_cadence: DifficultyTuning::for_level(BotDifficulty::Prime).cadence,
                protected_forecast_scrap: 0,
            },
        },
    )
    .expect("the reachable Fabricator can field the suppression provider");
    let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
    operation.artillery.clear();
    let mut decision = StrategicDecision::default();

    procure_connected_in_test(
        &operation,
        &plan,
        &planning_context(&fixture_planning, &identity, &observation, &intelligence),
        &mut decision,
    );

    assert!(
        decision.intents.contains(&Intent::TrainAt {
            building: BuildingId(12),
            kind: UnitKind::Bombard,
        }),
        "reachable producer was not selected: plan={plan:?}; decision={decision:?}"
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            building: BuildingId(10),
            ..
        }
    )));
}

#[test]
fn connected_package_excludes_publicly_stranded_units_and_producers() {
    let mut observation = developed_connected_obs(120);
    observation.visible.fill(false);
    observation.explored.fill(false);
    see_approach(&mut observation);
    observation.my_units[1].tile = TilePos::new(5, 5);
    observation.my_buildings[0].anchor = TilePos::new(2, 2);
    observation.my_buildings[1].anchor = TilePos::new(10, 2);
    observation.my_buildings[2].kind = BuildingKind::Fabricator;
    observation.my_buildings[2].anchor = TilePos::new(14, 2);
    let reachable_anchor = observation.my_buildings[2].anchor;
    let reachable_size = observation.my_buildings[2]
        .kind
        .tier_stats(observation.my_buildings[2].tier)
        .size;
    for tile in oxide_sim::geometry::rect_adjacent_tiles(reachable_anchor, reachable_size) {
        explore(&mut observation, tile);
        let index = usize::try_from(tile.y * observation.map_width + tile.x).unwrap();
        observation.visible[index] = true;
    }

    let horizontal = (0..=7).flat_map(|x| {
        [
            (TilePos::new(x, 0), Terrain::Peak),
            (TilePos::new(x, 7), Terrain::Peak),
        ]
    });
    let vertical = (1..7).flat_map(|y| {
        [
            (TilePos::new(0, y), Terrain::Peak),
            (TilePos::new(7, y), Terrain::Peak),
        ]
    });
    let public_map = public_map_with_terrain(&observation, horizontal.chain(vertical));
    let resources = ResourceSnapshot::from_observation(&observation);
    let intelligence = knowledge(&observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.anchor == TARGET)
        .expect("current target");
    let optimistic_route = ConnectedRouteContext {
        campaign_routes: None,
        unavailable_paid: &[],
        intel: &intelligence,
        home: HOME,
        target: TARGET,
        public_map: None,
        orientation: test_orientation(),
    };
    let public_route = ConnectedRouteContext {
        campaign_routes: None,
        unavailable_paid: &[],
        public_map: Some(&public_map),
        ..optimistic_route
    };
    let optimistic_targets =
        connected_target_selection(&observation, target, &[], optimistic_route);
    let public_targets = connected_target_selection(&observation, target, &[], public_route);

    let optimistic =
        connected_provider_unavailable(&observation, &optimistic_targets, &[], optimistic_route);
    let public = connected_provider_unavailable(&observation, &public_targets, &[], public_route);
    assert!(!optimistic.contains(&UnitId(2)));
    assert!(public.contains(&UnitId(2)));

    let access_without_briefing = connected_production_access(
        &observation,
        &optimistic_targets,
        &resources,
        optimistic_route,
    );
    let public_access =
        connected_production_access(&observation, &public_targets, &resources, public_route);
    assert!(access_without_briefing.allows(BuildingId(10), UnitKind::Bombard));
    assert!(!public_access.allows(BuildingId(10), UnitKind::Bombard));
    assert!(public_access.allows(BuildingId(12), UnitKind::Bombard));
    let reachable_timing = resources
        .producers()
        .iter()
        .find(|lane| lane.producer == BuildingId(12))
        .and_then(|lane| lane.production_timing(&[UnitKind::Bombard]))
        .expect("the reachable producer can train a Bombard");
    assert_eq!(
        reachable_timing.current_egress,
        super::super::resources::ProducerEgress::Open
    );

    let eligible: Vec<_> = resources
        .producers()
        .iter()
        .filter(|lane| public_access.allows(lane.producer, UnitKind::Bombard))
        .map(|lane| lane.producer)
        .collect();
    assert_eq!(eligible, [BuildingId(12)]);
    assert!(crate::resources::production_may_fit_horizon(
        &resources,
        &[UnitKind::Bombard],
        observation.tick + CONNECTED_PREPARATION_HORIZON,
        &public_access,
    ));
}

#[test]
fn connected_cluster_uses_air_routes_without_treating_ground_pits_as_a_barrier() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let secondary = TARGET.offset(0, 4);
    let mut observation = developed_connected_obs(120);
    observation.enemy_buildings = vec![
        building(80, 1, BuildingKind::Foundry, TARGET, true),
        building(81, 1, BuildingKind::Crucible, secondary, true),
    ];
    let ground_barrier = (0..observation.map_width).map(|x| (TilePos::new(x, 12), Terrain::Pit));
    let pit_map = public_map_with_terrain(&observation, ground_barrier);
    let intelligence = knowledge(&observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.anchor == TARGET)
        .expect("current primary target");
    assert!(!known_ground_connected(
        &observation,
        HOME,
        secondary,
        BuildingKind::Crucible.base_stats().size,
        Some(&pit_map),
    ));

    let pit_selection = connected_target_selection(
        &observation,
        target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home: HOME,
            target: TARGET,
            public_map: Some(&pit_map),
            orientation: test_orientation(),
        },
    );
    assert_eq!(pit_selection.target_anchors, vec![TARGET, secondary]);

    let air_barrier = (0..observation.map_width).map(|x| (TilePos::new(x, 12), Terrain::Peak));
    let peak_map = public_map_with_terrain(&observation, air_barrier);
    let peak_selection = connected_target_selection(
        &observation,
        target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home: HOME,
            target: TARGET,
            public_map: Some(&peak_map),
            orientation: test_orientation(),
        },
    );
    assert_eq!(peak_selection.target_anchors, vec![TARGET]);

    let peak_resources = ConnectedProductionResources::from_observation(
        &observation,
        target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home: HOME,
            target: TARGET,
            public_map: Some(&peak_map),
            orientation: test_orientation(),
        },
    );
    let peak_plan = connected_plan(
        &profile(),
        &observation,
        &intelligence,
        HOME,
        target,
        &[],
        ConnectedPlanningContext {
            planning: Some(&fixture_planning),
            minimum_only: false,
            campaign_routes: None,
            orientation: test_orientation(),
            public_map: Some(&peak_map),
            resources: &peak_resources,
            preferred_artillery: &[],
            protected_current_scrap: 0,
            preparation: PreparationConstraints {
                deadline: observation.tick + CONNECTED_PREPARATION_HORIZON,
                decision_cadence: 12,
                protected_forecast_scrap: 0,
            },
        },
    )
    .expect("an inaccessible secondary target must not reject the viable primary");
    assert_eq!(
        peak_plan
            .connected_package
            .as_ref()
            .expect("connected plan")
            .target_anchors,
        vec![TARGET]
    );

    let mut after_primary = observation.clone();
    after_primary
        .enemy_buildings
        .retain(|building| building.anchor == secondary);
    let later_intelligence = knowledge(&after_primary);
    let operation = operation(AirOperationPhase::Strike, after_primary.tick);
    let mut pit_plan = connected_test_plan(&after_primary);
    pit_plan
        .connected_package
        .as_mut()
        .expect("connected package")
        .target_anchors = pit_selection.target_anchors;
    assert_eq!(
        live_strike_target(&operation, &pit_plan, &later_intelligence)
            .map(|contact| contact.anchor),
        Some(secondary)
    );
    pit_plan
        .connected_package
        .as_mut()
        .expect("connected package")
        .target_anchors = peak_selection.target_anchors;
    assert_eq!(
        live_strike_target(&operation, &pit_plan, &later_intelligence),
        None,
        "a target excluded at admission must not re-enter tactical selection"
    );
}

#[test]
fn connected_operation_survives_the_primary_and_completes_at_the_remaining_anchor() {
    let secondary = TARGET.offset(3, 0);
    let mut battle = obs(5_000);
    battle.visible.fill(true);
    battle.explored.fill(true);
    battle.enemy_buildings = vec![building(81, 1, BuildingKind::Airworks, secondary, true)];
    let mut intelligence = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Verify, battle.tick);
    planner
        .air
        .as_mut()
        .expect("active operation")
        .plan
        .connected_package
        .as_mut()
        .expect("connected package")
        .target_anchors = vec![TARGET, secondary];

    let attack = think(&mut planner, &battle, &intelligence);
    assert!(attack.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(81)),
    }));
    assert!(attack.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(2)],
        goal: staging(HOME, secondary),
    }));
    assert!(attack.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, goal }
            if units == &[UnitId(2)] && *goal == staging(HOME, TARGET)
    )));
    assert_eq!(
        planner
            .air_operation()
            .expect("operation continues")
            .phase(),
        AirOperationPhase::Strike
    );

    battle.tick += 12;
    battle.enemy_buildings.clear();
    intelligence.update(&battle);
    let follow_through = think(&mut planner, &battle, &intelligence);
    assert!(follow_through.intents.contains(&Intent::AttackMoveUnits {
        units: vec![UnitId(3), UnitId(4)],
        goal: secondary,
    }));
    assert!(follow_through.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackMoveUnits { goal, .. } if *goal == TARGET
    )));

    battle.tick += 20;
    intelligence.update(&battle);
    let completed = think(&mut planner, &battle, &intelligence);
    assert!(completed.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
    let operation = planner
        .air_operation()
        .expect("completion recovery remains observable");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::Complete)
    );
}

#[test]
fn artillery_minimum_range_is_part_of_suppression_access() {
    let flak_anchor = TilePos::new(12, 10);
    let origin = TilePos::new(10, 10);
    let mut battle = obs(120);
    battle.visible.fill(true);
    battle.explored.fill(true);
    battle.enemy_buildings = vec![building(81, 1, BuildingKind::FlakTurret, flak_anchor, true)];
    let sealed = (-1..=1)
        .flat_map(|dy| (-1..=1).map(move |dx| origin.offset(dx, dy)))
        .filter(|tile| *tile != origin)
        .map(|tile| (tile, Terrain::Pit));
    let public_map = public_map_with_terrain(&battle, sealed);
    let intelligence = knowledge(&battle);
    let target = Target::Building(BuildingId(81));

    let bombard_routes = route_projection_with_orientation(
        &battle,
        Domain::Ground,
        Some(&public_map),
        test_orientation(),
    );
    assert_eq!(
        suppression_firing_stands(
            &bombard_routes,
            &battle,
            SuppressionOrigin {
                tile: origin,
                kind: UnitKind::Bombard,
            },
            target,
            &intelligence,
            Some(&public_map),
        )
        .next(),
        Some(origin),
        "Bombard may fire over the surrounding Pit"
    );

    let avalanche_routes = route_projection_with_orientation(
        &battle,
        Domain::Ground,
        Some(&public_map),
        test_orientation(),
    );
    assert_eq!(
        suppression_firing_stands(
            &avalanche_routes,
            &battle,
            SuppressionOrigin {
                tile: origin,
                kind: UnitKind::Avalanche,
            },
            target,
            &intelligence,
            Some(&public_map),
        )
        .next(),
        None,
        "Avalanche cannot use the same pocket inside its minimum range"
    );
}

#[test]
fn reserved_sole_suppression_provider_cannot_inflate_the_target_cluster() {
    let primary = TilePos::new(20, 20);
    let secondary = TilePos::new(24, 20);
    let flak = TilePos::new(29, 20);
    let mut battle = obs(400);
    battle.map_width = 40;
    battle.map_height = 30;
    battle.visible = vec![true; 40 * 30];
    battle.explored = vec![true; 40 * 30];
    battle.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, primary, true),
        building(82, 1, BuildingKind::Airworks, secondary, true),
        building(81, 1, BuildingKind::FlakTurret, flak, true),
    ];
    let intelligence = knowledge(&battle);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.anchor == primary)
        .expect("current primary target");
    let route = ConnectedRouteContext {
        campaign_routes: None,
        unavailable_paid: &[],
        intel: &intelligence,
        home: HOME,
        target: primary,
        public_map: None,
        orientation: test_orientation(),
    };

    let available = connected_target_selection(&battle, target, &[], route);
    assert_eq!(available.target_anchors, vec![primary, secondary]);
    assert_eq!(
        available.suppression_targets,
        vec![Target::Building(BuildingId(81))]
    );

    let reserved = connected_target_selection(&battle, target, &[UnitId(2)], route);
    assert_eq!(reserved.target_anchors, vec![primary]);
    assert!(reserved.suppression_targets.is_empty());
}

#[test]
fn optional_cluster_target_is_dropped_when_its_only_provider_misses_the_deadline() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let primary = TARGET;
    let secondary = TARGET.offset(4, 0);
    let flak = TARGET.offset(9, 0);
    let mut battle = developed_connected_obs(408);
    battle.map_width = 40;
    battle.map_height = 24;
    battle.visible = vec![true; 40 * 24];
    battle.explored = vec![true; 40 * 24];
    battle.my_units[1] = own(2, UnitKind::Avalanche, TilePos::new(10, 10));
    battle.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, primary, true),
        building(82, 1, BuildingKind::Turret, secondary, true),
        building(81, 1, BuildingKind::FlakTurret, flak, true),
    ];
    battle.my_queues[0] = vec![UnitKind::Lancer; QUEUE_CAP];

    // Two offset Peak walls leave a bent corridor into a pocket inside
    // the Avalanche's blind ring. A Bombard can prosecute the Flak from
    // the pocket, while the live Avalanche cannot fire through either
    // wall from the main component.
    let terrain = (0..battle.map_height).flat_map(|y| {
        (0..battle.map_width).filter_map(move |x| {
            let open = x <= 28
                || (x == 29 && y == 15)
                || (x == 30 && (12..=15).contains(&y))
                || (x == 31 && y == 12)
                || ((32..=35).contains(&x) && (9..=12).contains(&y));
            (!open).then_some((TilePos::new(x, y), Terrain::Peak))
        })
    });
    let public_map = public_map_with_terrain(&battle, terrain);
    let intelligence = knowledge(&battle);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.anchor == primary)
        .expect("current primary target");
    let route = ConnectedRouteContext {
        campaign_routes: None,
        unavailable_paid: &[],
        intel: &intelligence,
        home: HOME,
        target: primary,
        public_map: Some(&public_map),
        orientation: test_orientation(),
    };
    let resources = ConnectedProductionResources::from_observation(&battle, target, &[], route);
    assert_eq!(resources.targets.target_anchors, vec![primary, secondary]);

    let mut open_lane = battle.clone();
    open_lane.my_queues[0].clear();
    let open_resources =
        ConnectedProductionResources::from_observation(&open_lane, target, &[], route);
    let open_plan = connected_plan(
        &profile(),
        &open_lane,
        &intelligence,
        HOME,
        target,
        &[],
        ConnectedPlanningContext {
            planning: Some(&fixture_planning),
            minimum_only: false,
            campaign_routes: None,
            orientation: test_orientation(),
            public_map: Some(&public_map),
            resources: &open_resources,
            preferred_artillery: &[],
            protected_current_scrap: 0,
            preparation: PreparationConstraints {
                deadline: battle.tick + 400,
                decision_cadence: 12,
                protected_forecast_scrap: 0,
            },
        },
    )
    .expect("an open Bombard lane can cover the optional target in time");
    assert_eq!(
        open_plan
            .connected_package
            .expect("connected package")
            .target_anchors,
        vec![primary, secondary]
    );

    let plan = connected_plan(
        &profile(),
        &battle,
        &intelligence,
        HOME,
        target,
        &[],
        ConnectedPlanningContext {
            planning: Some(&fixture_planning),
            minimum_only: false,
            campaign_routes: None,
            orientation: test_orientation(),
            public_map: Some(&public_map),
            resources: &resources,
            preferred_artillery: &[],
            protected_current_scrap: 0,
            preparation: PreparationConstraints {
                deadline: battle.tick + 400,
                decision_cadence: 12,
                protected_forecast_scrap: 0,
            },
        },
    )
    .expect("the live primary-only package remains feasible");
    assert_eq!(
        plan.connected_package
            .expect("connected package")
            .target_anchors,
        vec![primary],
        "a route-only optional target must not enlarge a package whose only covering producer cannot finish before the fixed deadline"
    );
}

#[test]
fn connected_suppression_uses_an_indirect_firing_stand_beyond_a_pit_ring() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let flak_anchor = TARGET.offset(-4, 0);
    let mut observation = developed_connected_obs(120);
    observation
        .my_units
        .iter_mut()
        .find(|unit| unit.id == UnitId(2))
        .expect("fixture artillery")
        .tile = TilePos::new(5, 10);
    observation.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, TARGET, true),
        building(81, 1, BuildingKind::FlakTurret, flak_anchor, true),
    ];
    let sealed = oxide_sim::geometry::rect_adjacent_tiles(
        flak_anchor,
        BuildingKind::FlakTurret.base_stats().size,
    )
    .map(|tile| (tile, Terrain::Pit));
    let public_map = public_map_with_terrain(&observation, sealed);
    let mut intelligence = knowledge(&observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.anchor == TARGET)
        .expect("current primary target");
    let staging = connected_artillery_staging_goal(&observation, HOME, TARGET, Some(&public_map))
        .expect("generic staging remains reachable");
    let bombard = observation
        .my_units
        .iter()
        .find(|unit| unit.kind == UnitKind::Bombard)
        .expect("fixture artillery");
    let mut ground_routes = route_projection_with_orientation(
        &observation,
        Domain::Ground,
        Some(&public_map),
        test_orientation(),
    );
    assert!(ground_routes.unit_reaches(bombard, staging));

    let resources = ConnectedProductionResources::from_observation(
        &observation,
        target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intelligence,
            home: HOME,
            target: TARGET,
            public_map: Some(&public_map),
            orientation: test_orientation(),
        },
    );
    assert_eq!(
        resources.targets.suppression_targets,
        vec![Target::Building(BuildingId(81))]
    );
    assert!(
        resources.access.allows(BuildingId(10), UnitKind::Bombard),
        "a producer may supply artillery that can reach an indirect-fire stand"
    );

    connected_plan(
        &profile(),
        &observation,
        &intelligence,
        HOME,
        target,
        &[],
        ConnectedPlanningContext {
            planning: Some(&fixture_planning),
            minimum_only: false,
            campaign_routes: None,
            orientation: test_orientation(),
            public_map: Some(&public_map),
            resources: &resources,
            preferred_artillery: &[],
            protected_current_scrap: 0,
            preparation: PreparationConstraints {
                deadline: observation.tick + CONNECTED_PREPARATION_HORIZON,
                decision_cadence: 12,
                protected_forecast_scrap: 0,
            },
        },
    )
    .expect("Pit blocks movement but not an indirect shell");

    let mut planner = with_operation(AirOperationPhase::SuppressAa, observation.tick);
    let active = planner.air.as_mut().expect("active operation");
    active
        .plan
        .connected_package
        .as_mut()
        .expect("connected package")
        .target_anchors = vec![TARGET];
    let mut coordination = coordination(&fixture_planning, None);
    coordination.public_map = Some(&public_map);
    let positioning = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination,
        )
        .decision;
    let firing_stand = positioning
        .intents
        .iter()
        .find_map(|intent| match intent {
            Intent::MoveUnits { units, goal } if units == &[UnitId(2)] => Some(*goal),
            _ => None,
        })
        .expect("artillery moves to its exact firing stand");
    assert!(
        !oxide_sim::geometry::rect_adjacent_tiles(
            flak_anchor,
            BuildingKind::FlakTurret.base_stats().size,
        )
        .any(|tile| tile == firing_stand),
        "the firing stand is not an unreachable footprint-adjacent tile"
    );
    assert!(positioning.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(81)),
            ..
        }
    )));

    observation.tick += 12;
    observation
        .my_units
        .iter_mut()
        .find(|unit| unit.id == UnitId(2))
        .expect("fixture artillery")
        .tile = firing_stand;
    observation
        .my_units
        .iter_mut()
        .find(|unit| unit.id == UnitId(1))
        .expect("fixture scout")
        .idle = false;
    intelligence.update(&observation);
    let attack = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination,
        )
        .decision;
    assert!(
        attack.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(2)],
            target: Target::Building(BuildingId(81)),
        }),
        "{attack:?}; operation={:?}",
        planner.air_operation()
    );
}

#[test]
fn connected_no_aa_evidence_requires_visibility_over_the_full_footprint() {
    let mut observation = obs(120);
    let anchor_index = usize::try_from(TARGET.y * observation.map_width + TARGET.x).unwrap();
    observation.visible[anchor_index] = true;
    let intelligence = knowledge(&observation);
    let operation = operation(AirOperationPhase::Verify, observation.tick);
    let plan = connected_test_plan(&observation);
    assert_eq!(
        cluster_air_defense(&operation, &plan, &intelligence).evidence,
        AirDefenseEvidence::Unknown
    );

    let mut fully_visible = observation;
    let (width, height) = BuildingKind::Crucible.base_stats().size;
    for dy in 0..height {
        for dx in 0..width {
            let tile = TARGET.offset(dx, dy);
            let index = usize::try_from(tile.y * fully_visible.map_width + tile.x).unwrap();
            fully_visible.visible[index] = true;
        }
    }
    let intelligence = knowledge(&fully_visible);
    assert_eq!(
        cluster_air_defense(&operation, &plan, &intelligence).evidence,
        AirDefenseEvidence::VisibleWithoutKnownCoverage
    );
}

#[test]
fn connected_verify_keeps_a_remembered_selected_anchor_in_aa_clearance() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let primary = TilePos::new(20, 20);
    let secondary = TilePos::new(24, 20);
    let flak = TilePos::new(29, 20);
    let mut battle = obs(400);
    battle.map_width = 40;
    battle.map_height = 30;
    battle.visible = vec![true; 40 * 30];
    battle.explored = vec![true; 40 * 30];
    battle.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, primary, true),
        building(82, 1, BuildingKind::Airworks, secondary, true),
        building(81, 1, BuildingKind::FlakTurret, flak, true),
    ];
    let mut intelligence = knowledge(&battle);

    let mut hidden = battle;
    hidden.tick += 12;
    hidden.visible.fill(false);
    see_approach_to(&mut hidden, primary);
    see_building_footprint(&mut hidden, primary, BuildingKind::Crucible);
    for building in &mut hidden.enemy_buildings {
        building.seen = building.anchor == primary;
    }
    intelligence.update(&hidden);
    assert!(intelligence.buildings().iter().any(|contact| {
        contact.anchor == secondary && contact.evidence == ContactEvidence::Remembered
    }));

    let mut operation = operation(AirOperationPhase::Verify, hidden.tick);
    operation.target = primary;
    operation.target_id = Some(BuildingId(80));
    let mut plan = connected_test_plan(&hidden);
    plan.connected_package
        .as_mut()
        .expect("connected package")
        .target_anchors = vec![primary, secondary];
    assert_eq!(
        cluster_air_defense(&operation, &plan, &intelligence).evidence,
        AirDefenseEvidence::RememberedCoverage
    );
    assert_eq!(
        connected_scout_focus(&operation, &plan, &hidden, &intelligence),
        secondary
    );

    let identity = profile();
    let mut decision = StrategicDecision::default();
    verify(
        &mut operation,
        &mut plan,
        &AirPlanningContext {
            allow_procurement: true,
            planning: Some(&fixture_planning),
            profile: &identity,
            tuning: DifficultyTuning::for_level(identity.difficulty),
            obs: &hidden,
            intel: &intelligence,
            home: HOME,
            orientation: test_orientation(),
            public_map: None,
            enlisted: &[],
            landing_sites: &[],
            connected_resources: None,
            production: StrategicProductionContext::empty(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        },
        &mut decision,
    );
    assert_eq!(operation.phase(), AirOperationPhase::Verify);
    assert!(operation.scout_dispatch.is_some());
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));

    let mut cleared = hidden;
    cleared.tick += 100;
    cleared
        .enemy_buildings
        .retain(|building| building.anchor == primary);
    see_building_footprint(&mut cleared, secondary, BuildingKind::Airworks);
    see_building_footprint(&mut cleared, flak, BuildingKind::FlakTurret);
    intelligence.update(&cleared);
    assert_eq!(
        cluster_air_defense(&operation, &plan, &intelligence).evidence,
        AirDefenseEvidence::VisibleWithoutKnownCoverage
    );

    let mut decision = StrategicDecision::default();
    verify(
        &mut operation,
        &mut plan,
        &AirPlanningContext {
            allow_procurement: true,
            planning: Some(&fixture_planning),
            profile: &identity,
            tuning: DifficultyTuning::for_level(identity.difficulty),
            obs: &cleared,
            intel: &intelligence,
            home: HOME,
            orientation: test_orientation(),
            public_map: None,
            enlisted: &[],
            landing_sites: &[],
            connected_resources: None,
            production: StrategicProductionContext::empty(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        },
        &mut decision,
    );
    assert_eq!(operation.phase(), AirOperationPhase::Strike);
}

#[test]
fn connected_verify_scouts_every_selected_footprint_before_accepting_negative_aa_evidence() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let secondary = TARGET.offset(0, 4);
    let mut observation = obs(120);
    observation
        .enemy_buildings
        .push(building(81, 1, BuildingKind::Crucible, secondary, true));
    observation.explored.fill(true);
    see_approach(&mut observation);
    see_approach_to(&mut observation, secondary);
    see_building_footprint(&mut observation, TARGET, BuildingKind::Crucible);
    see_building_footprint(&mut observation, secondary, BuildingKind::Crucible);
    let far_edge = secondary.offset(1, 1);
    let far_index = usize::try_from(far_edge.y * observation.map_width + far_edge.x).unwrap();
    observation.visible[far_index] = false;
    let mut intelligence = knowledge(&observation);
    let mut operation = operation(AirOperationPhase::Verify, observation.tick);
    let mut plan = connected_test_plan(&observation);
    plan.connected_package
        .as_mut()
        .expect("connected package")
        .target_anchors = vec![TARGET, secondary];
    let identity = profile();
    let context = AirPlanningContext {
        allow_procurement: true,
        planning: Some(&fixture_planning),
        profile: &identity,
        tuning: DifficultyTuning::for_level(identity.difficulty),
        obs: &observation,
        intel: &intelligence,
        home: HOME,
        orientation: test_orientation(),
        public_map: None,
        enlisted: &[],
        landing_sites: &[],
        connected_resources: None,
        production: StrategicProductionContext::empty(),
        protected_current_scrap: 0,
        protected_forecast_scrap: 0,
    };
    let mut decision = StrategicDecision::default();

    verify(&mut operation, &mut plan, &context, &mut decision);

    assert_eq!(
        connected_scout_focus(&operation, &plan, &observation, &intelligence),
        far_edge
    );
    let (_, scout_goal) = operation
        .scout_dispatch
        .expect("the scout is sent to clear the remaining footprint tile");
    let scout_vision = Role::Scout
        .unit_for(observation.faction)
        .stats()
        .vision
        .saturating_sub(1);
    let dx = scout_goal.x - far_edge.x;
    let dy = scout_goal.y - far_edge.y;
    assert!(dx.saturating_mul(dx) + dy.saturating_mul(dy) <= scout_vision * scout_vision);
    assert_eq!(operation.phase(), AirOperationPhase::Verify);

    observation.tick += 12;
    observation.visible[far_index] = true;
    intelligence.update(&observation);
    let context = AirPlanningContext {
        allow_procurement: true,
        planning: Some(&fixture_planning),
        profile: &identity,
        tuning: DifficultyTuning::for_level(identity.difficulty),
        obs: &observation,
        intel: &intelligence,
        home: HOME,
        orientation: test_orientation(),
        public_map: None,
        enlisted: &[],
        landing_sites: &[],
        connected_resources: None,
        production: StrategicProductionContext::empty(),
        protected_current_scrap: 0,
        protected_forecast_scrap: 0,
    };
    let mut cleared = StrategicDecision::default();
    verify(&mut operation, &mut plan, &context, &mut cleared);

    assert_eq!(operation.phase(), AirOperationPhase::Strike);
}

#[test]
fn connected_verify_checks_the_selected_secondary_approach_before_striking() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let secondary = TARGET.offset(0, 4);
    let mut observation = obs(120);
    observation.enemy_buildings = vec![building(81, 1, BuildingKind::Crucible, secondary, true)];
    see_approach(&mut observation);
    let (width, height) = BuildingKind::Crucible.base_stats().size;
    for dy in 0..height {
        for dx in 0..width {
            let tile = secondary.offset(dx, dy);
            let index = usize::try_from(tile.y * observation.map_width + tile.x).unwrap();
            observation.visible[index] = true;
            observation.explored[index] = true;
        }
    }
    let intelligence = knowledge(&observation);
    assert!(corridor_clear(&intelligence, HOME, TARGET, &[]));
    assert!(!corridor_clear(&intelligence, HOME, secondary, &[]));

    let identity = profile();
    let mut operation = operation(AirOperationPhase::Verify, observation.tick);
    let mut plan = connected_test_plan(&observation);
    plan.connected_package
        .as_mut()
        .expect("connected package")
        .target_anchors = vec![TARGET, secondary];
    let context = AirPlanningContext {
        allow_procurement: true,
        planning: Some(&fixture_planning),
        profile: &identity,
        tuning: DifficultyTuning::for_level(identity.difficulty),
        obs: &observation,
        intel: &intelligence,
        home: HOME,
        orientation: test_orientation(),
        public_map: None,
        enlisted: &[],
        landing_sites: &[],
        connected_resources: None,
        production: StrategicProductionContext::empty(),
        protected_current_scrap: 0,
        protected_forecast_scrap: 0,
    };
    let mut decision = StrategicDecision::default();
    verify(&mut operation, &mut plan, &context, &mut decision);

    assert_eq!(operation.phase(), AirOperationPhase::Verify);
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(81)),
            ..
        }
    )));
    assert!(operation.scout_dispatch.is_some());
}

fn assert_axis_orientation_preserves_producer_doorstep(home: TilePos) {
    let world = developed_connected_obs(120);
    let orientation = Orientation::for_home(&world, home);
    let producer = world
        .my_buildings
        .iter()
        .find(|building| building.id == BuildingId(10))
        .expect("the fixture has a Fabricator");
    let size = producer.kind.tier_stats(producer.tier).size;
    let expected = oxide_sim::geometry::rect_adjacent_tiles(producer.anchor, size)
        .filter(|tile| public_ground_open(&world, *tile, None))
        .min_by_key(|tile| {
            oxide_sim::geometry::spawn_doorstep_key(
                (world.map_width, world.map_height),
                producer.anchor,
                size,
                *tile,
            )
        })
        .expect("the world-frame producer has an open doorstep");

    let oriented = orientation.observe(&world);
    let oriented_producer = oriented
        .my_buildings
        .iter()
        .find(|building| building.id == BuildingId(10))
        .expect("orientation preserves the producer");
    let actual = production_spawn_doorstep(
        QueryPurpose::NavigationTest,
        &oriented,
        oriented_producer,
        None,
        Some(orientation),
    )
    .expect("the oriented producer has an open doorstep");

    assert_eq!(orientation.tile(actual), expected);
}

#[test]
fn x_only_orientation_preserves_the_authoritative_producer_doorstep() {
    assert_axis_orientation_preserves_producer_doorstep(TilePos::new(27, 2));
}

#[test]
fn y_only_orientation_preserves_the_authoritative_producer_doorstep() {
    assert_axis_orientation_preserves_producer_doorstep(TilePos::new(2, 17));
}

#[test]
fn centered_producer_preserves_the_authoritative_world_order_doorstep() {
    let mut world = developed_connected_obs(120);
    let (size, anchor) = {
        let world = &mut *world;
        let producer = world
            .my_buildings
            .iter_mut()
            .find(|building| building.id == BuildingId(10))
            .expect("the fixture has a Fabricator");
        let size = producer.kind.tier_stats(producer.tier).size;
        let anchor = TilePos::new(
            (world.map_width - size.0) / 2,
            (world.map_height - size.1) / 2,
        );
        producer.anchor = anchor;
        (size, anchor)
    };
    let expected = oxide_sim::geometry::rect_adjacent_tiles(anchor, size)
        .filter(|tile| public_ground_open(&world, *tile, None))
        .min_by_key(|tile| {
            oxide_sim::geometry::spawn_doorstep_key(
                (world.map_width, world.map_height),
                anchor,
                size,
                *tile,
            )
        })
        .expect("the world-frame producer has an open doorstep");

    let orientation = Orientation::for_home(
        &world,
        TilePos::new(world.map_width - 1, world.map_height - 1),
    );
    let oriented = orientation.observe(&world);
    let oriented_producer = oriented
        .my_buildings
        .iter()
        .find(|building| building.id == BuildingId(10))
        .expect("orientation preserves the producer");
    let actual = production_spawn_doorstep(
        QueryPurpose::NavigationTest,
        &oriented,
        oriented_producer,
        None,
        Some(orientation),
    )
    .expect("the oriented producer has an open doorstep");

    assert_eq!(
        orientation.tile(actual),
        expected,
        "a zero radial key must retain the authoritative world-frame row-major tie"
    );
}

#[test]
fn precommit_rederivation_reports_and_recovers_from_untargetable_air_defense() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = obs(301);
    battle.explored.fill(true);
    see_approach(&mut battle);
    let mut talon = own(90, UnitKind::Talon, TARGET.offset(-3, 0));
    talon.player = PlayerId(1);
    battle.enemy_units.push(talon);
    let intelligence = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Assemble, 300);

    let result = planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &battle,
        &intelligence,
        HOME,
        coordination(&fixture_planning, None),
    );

    assert!(
        matches!(
            result.rejected_connected_candidate,
            Some(RejectedConnectedCandidate {
                reason: ConnectedPlanRejection::Package {
                    reason: ForcePackageRejection::UntargetableCurrentAirDefense { .. },
                    ..
                },
                ..
            })
        ),
        "unexpected rejection: {:?}",
        result.rejected_connected_candidate
    );
    assert_eq!(
        planner
            .air_operation()
            .and_then(|operation| operation.recovery_reason()),
        Some(AirRecoveryReason::NewAirDefense)
    );
    assert!(result.decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn current_air_defense_first_seen_on_the_deadline_prevents_stale_force_freeze() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = obs(2_500);
    battle.explored.fill(true);
    see_approach(&mut battle);
    let mut talon = own(90, UnitKind::Talon, TARGET.offset(-3, 0));
    talon.player = PlayerId(1);
    battle.enemy_units.push(talon);
    let intelligence = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Assemble, battle.tick);
    let active = planner.air.as_mut().expect("the fixture has an operation");
    let package = active
        .plan
        .connected_package
        .as_mut()
        .expect("the fixture has a connected package");
    package.derived_at = battle.tick - 1;
    package.preparation_deadline = battle.tick;

    let result = planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &battle,
        &intelligence,
        HOME,
        coordination(&fixture_planning, None),
    );

    assert!(
        matches!(
            result.rejected_connected_candidate,
            Some(RejectedConnectedCandidate {
                reason: ConnectedPlanRejection::Package {
                    reason: ForcePackageRejection::UntargetableCurrentAirDefense { .. },
                    ..
                },
                ..
            })
        ),
        "deadline evidence did not reject the stale package: {:?}",
        result.rejected_connected_candidate
    );
    let operation = planner
        .air_operation()
        .expect("the rejected operation remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::NewAirDefense)
    );
    assert_eq!(operation.membership_frozen_at, None);
    assert!(result.decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn retained_connected_operation_grows_before_freeze_and_is_immutable_afterward() {
    let admitted_at = 120;
    let mut planner = with_operation(AirOperationPhase::Assemble, admitted_at);
    let initial_package = planner
        .air_plan()
        .and_then(|plan| plan.connected_package.clone())
        .expect("the fixture begins with an admitted connected package");
    let fixed_deadline = initial_package.preparation_deadline;

    let mut richer = developed_connected_obs(admitted_at + 12);
    richer.scrap = 50_000;
    richer.enemy_buildings.extend([
        building(81, 1, BuildingKind::Foundry, TARGET.offset(-2, -2), true),
        building(82, 1, BuildingKind::Airworks, TARGET.offset(-2, 2), true),
        building(83, 1, BuildingKind::Fabricator, TARGET.offset(-4, 0), true),
        building(84, 1, BuildingKind::Extractor, TARGET.offset(0, -4), true),
    ]);
    let mut intelligence = knowledge(&richer);

    think(&mut planner, &richer, &intelligence);

    let revised_package = planner
        .air_plan()
        .and_then(|plan| plan.connected_package.clone())
        .expect("the retained operation keeps its connected package");
    assert_eq!(revised_package.derived_at, richer.tick);
    assert_eq!(revised_package.preparation_deadline, fixed_deadline);
    assert!(revised_package.target_value > initial_package.target_value);
    assert!(
        demand_count(&revised_package.suppression) + demand_count(&revised_package.strike)
            > demand_count(&initial_package.suppression) + demand_count(&initial_package.strike),
        "current richer evidence should grow the retained package before commitment"
    );

    let active = planner.air.as_mut().expect("the operation remains active");
    active.op.stage = AirStage::SuppressAa;
    active.op.phase_started_at = richer.tick;
    active.op.membership_frozen_at = Some(richer.tick);
    let frozen_package = active.plan.connected_package.clone();
    let frozen_members = (
        active.op.scout,
        active.op.artillery.clone(),
        active.op.strike_aircraft.clone(),
    );

    richer.tick += 12;
    richer.enemy_buildings.extend([
        building(85, 1, BuildingKind::Foundry, TARGET.offset(2, -2), true),
        building(86, 1, BuildingKind::Crucible, TARGET.offset(2, 2), true),
    ]);
    intelligence.update(&richer);
    think(&mut planner, &richer, &intelligence);

    let active = planner
        .air
        .as_ref()
        .expect("the frozen operation remains active");
    assert_eq!(active.plan.connected_package, frozen_package);
    assert_eq!(
        (
            active.op.scout,
            active.op.artillery.clone(),
            active.op.strike_aircraft.clone(),
        ),
        frozen_members,
        "post-commit evidence cannot rewrite the exact force membership"
    );
}

#[test]
fn rich_targets_scale_connected_packages_beyond_the_old_fixed_cohort() {
    let mut observation = developed_connected_obs(120);
    observation.scrap = 50_000;
    observation.enemy_buildings.extend([
        building(81, 1, BuildingKind::Foundry, TARGET.offset(-2, -2), true),
        building(82, 1, BuildingKind::Airworks, TARGET.offset(-2, 2), true),
        building(83, 1, BuildingKind::FlakTurret, TARGET.offset(-4, 0), true),
    ]);
    let siege = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_043,
    ));
    let air = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_045,
    ));

    let siege_plan = derived_connected_test_plan(&siege, &observation)
        .expect("the developed economy can field a connected package");
    let air_plan = derived_connected_test_plan(&air, &observation)
        .expect("the developed economy can field a connected package");

    for plan in [&siege_plan, &air_plan] {
        let package = plan
            .connected_package
            .as_ref()
            .expect("connected admission owns an explicit package");
        assert!(!package.recon.is_empty());
        assert!(!package.suppression.is_empty());
        assert!(!package.strike.is_empty());
        assert!(
            plan.desired_artillery + plan.desired_strike_aircraft > 3,
            "a rich defended cluster should justify more than the removed fixed cohort: {package:?}"
        );
    }
}

#[test]
fn retained_connected_feasibility_defers_without_recovering_but_rejects_lost_producers() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = developed_connected_obs(120);
    observation.scrap = 10_000;
    observation.my_units.clear();
    let intelligence = knowledge(&observation);
    let identity = profile();
    let plan = connected_test_plan(&observation);
    let op = operation(AirOperationPhase::Assemble, observation.tick);
    let zero = crate::planning::PlanningWork::with_allowance(0);
    let mut context = planning_context(&fixture_planning, &identity, &observation, &intelligence);
    context.planning = Some(&zero);
    assert!(!connected_package_is_proven_infeasible(
        &op, &plan, &context
    ));
    assert_eq!(zero.spent(), 0);
    let work = crate::planning::PlanningWork::default();
    context.planning = Some(&work);
    assert!(!connected_package_is_proven_infeasible(
        &op, &plan, &context
    ));
    assert!(work.spent() > 0);

    observation.my_buildings.clear();
    observation.my_queues.clear();
    observation.my_queue_progress.clear();
    let mut context = planning_context(&fixture_planning, &identity, &observation, &intelligence);
    context.planning = Some(&work);
    assert!(connected_package_is_proven_infeasible(&op, &plan, &context));
}

#[test]
fn admitted_connected_package_recovers_when_its_suppression_producers_are_lost() {
    let mut observation = developed_connected_obs(120);
    let mut intelligence = knowledge(&observation);
    let mut planner = StrategicPlanner::new();

    think(&mut planner, &observation, &intelligence);
    assert!(planner.air_operation().is_some_and(|operation| {
        operation.phase() <= AirOperationPhase::Assemble && operation.recovery_reason().is_none()
    }));

    observation.tick += 1;
    observation.my_units.retain(|unit| !is_artillery(unit.kind));
    observation.my_buildings = vec![building(
        11,
        0,
        BuildingKind::Airworks,
        TilePos::new(5, 2),
        true,
    )];
    observation.my_queues = vec![Vec::new()];
    intelligence.update(&observation);

    let failed = think(&mut planner, &observation, &intelligence);
    let operation = planner
        .air_operation()
        .expect("the failed preparation remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::PreparationInfeasible)
    );
    assert!(
        failed
            .intents
            .iter()
            .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
    );
}

#[test]
fn hidden_connected_target_revalidates_current_funding_evidence() {
    let mut initial = obs(120);
    initial.visible.fill(true);
    initial.explored.fill(true);
    initial.scrap = 0;
    initial
        .my_units
        .retain(|unit| matches!(unit.kind, UnitKind::Kestrel | UnitKind::Bombard));
    initial.my_buildings = vec![
        building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
        building(12, 0, BuildingKind::Extractor, TilePos::new(2, 2), true),
    ];
    initial.my_queues = vec![Vec::new(); initial.my_buildings.len()];

    let plan = derived_connected_test_plan(&profile(), &initial)
        .expect("the completed Extractor forecast funds the admitted minimum");
    let package = plan
        .connected_package
        .as_ref()
        .expect("the fixture derives a connected package");
    assert_eq!(package.current_scrap, 0);
    assert!(package.forecast_scrap > 0);

    let mut operation = operation(AirOperationPhase::Assemble, initial.tick);
    operation.strike_aircraft.clear();
    let template = planner_with_operation(operation, plan);
    let run = |retain_extractor: bool, current_scrap: u32| {
        let mut hidden = initial.clone();
        hidden.tick += 12;
        hidden.scrap = current_scrap;
        hidden.visible.fill(false);
        hidden.enemy_buildings[0].seen = false;
        if !retain_extractor {
            let retained: Vec<_> = {
                let hidden = &mut *hidden;
                hidden
                    .my_buildings
                    .drain(..)
                    .zip(hidden.my_queues.drain(..))
                    .filter(|(building, _)| building.kind != BuildingKind::Extractor)
                    .collect()
            };
            (hidden.my_buildings, hidden.my_queues) = retained.into_iter().unzip();
        }
        let mut intelligence = knowledge(&initial);
        intelligence.update(&hidden);
        assert!(intelligence.buildings().iter().any(|building| {
            building.anchor == TARGET && building.evidence == ContactEvidence::Remembered
        }));
        let mut planner = template.clone();
        let decision = think(&mut planner, &hidden, &intelligence);
        (planner, decision)
    };

    let (forecast_funded, _) = run(true, 0);
    assert!(forecast_funded.air_operation().is_some_and(|operation| {
        operation.phase() == AirOperationPhase::Assemble && operation.recovery_reason().is_none()
    }));

    let (bank_funded, _) = run(false, 10_000);
    assert!(bank_funded.air_operation().is_some_and(|operation| {
        operation.phase() == AirOperationPhase::Assemble && operation.recovery_reason().is_none()
    }));

    let (unfunded, decision) = run(false, 0);
    let operation = unfunded
        .air_operation()
        .expect("the failed preparation remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::PreparationInfeasible)
    );
    assert!(
        decision
            .intents
            .iter()
            .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
    );
    assert_eq!(decision.committed_scrap(), 0);
}

#[test]
fn hidden_target_revalidates_the_full_scaled_package_funding() {
    let initial = developed_connected_obs(120);
    let mut intelligence = knowledge(&initial);
    let mut hidden = initial.clone();
    hidden.tick += 12;
    hidden.scrap = 0;
    hidden.visible.fill(false);
    hidden.enemy_buildings[0].seen = false;
    hidden.my_units.retain(|unit| unit.id != UnitId(4));
    intelligence.update(&hidden);

    let plan = connected_test_plan(&initial);
    let mut operation = operation(AirOperationPhase::Recon, initial.tick);
    operation.strike_aircraft = vec![UnitId(3)];
    let mut planner = planner_with_operation(operation, plan);

    let decision = think(&mut planner, &hidden, &intelligence);

    let operation = planner
        .air_operation()
        .expect("the infeasible scaled package remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::PreparationInfeasible)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Condor,
            ..
        }
    )));
    assert_eq!(decision.committed_scrap(), 0);
}

#[test]
fn stale_target_does_not_let_marginal_providers_bypass_a_lost_minimum() {
    let first = developed_connected_obs(120);
    let mut intelligence = knowledge(&first);
    let mut hidden = first.clone();
    hidden.tick += 1;
    hidden.visible.fill(false);
    hidden.enemy_buildings[0].seen = false;
    let retained: Vec<_> = {
        let hidden = &mut *hidden;
        hidden
            .my_buildings
            .drain(..)
            .zip(hidden.my_queues.drain(..))
            .filter(|(building, _)| building.kind != BuildingKind::Crucible)
            .collect()
    };
    (hidden.my_buildings, hidden.my_queues) = retained.into_iter().unzip();
    hidden.my_units.retain(|unit| {
        !matches!(
            unit.kind,
            UnitKind::Bombard | UnitKind::Avalanche | UnitKind::Buzzard | UnitKind::Condor
        )
    });
    intelligence.update(&hidden);
    assert!(intelligence.buildings().iter().any(|building| {
        building.anchor == TARGET && building.evidence == ContactEvidence::Remembered
    }));
    assert!(has_producer(&hidden, UnitKind::Bombard));
    assert!(has_producer(&hidden, UnitKind::Buzzard));
    assert!(!has_producer(&hidden, UnitKind::Avalanche));
    assert!(!requirements_met(&hidden, UnitKind::Condor));

    let mut plan = connected_test_plan(&first);
    let package = plan
        .connected_package
        .as_mut()
        .expect("the admitted connected plan carries its package");
    package.suppression = vec![
        ProviderDemand {
            kind: UnitKind::Avalanche,
            count: 1,
        },
        ProviderDemand {
            kind: UnitKind::Bombard,
            count: 1,
        },
    ];
    package.strike = vec![
        ProviderDemand {
            kind: UnitKind::Condor,
            count: 1,
        },
        ProviderDemand {
            kind: UnitKind::Buzzard,
            count: 1,
        },
    ];
    package.provider_priority = vec![
        force_package::ProviderDemandTranche {
            priority: force_package::ProviderPriority::Minimum,
            family: ForceFamily::Recon,
            kind: UnitKind::Kestrel,
            count: 1,
        },
        force_package::ProviderDemandTranche {
            priority: force_package::ProviderPriority::Minimum,
            family: ForceFamily::Suppression,
            kind: UnitKind::Avalanche,
            count: 1,
        },
        force_package::ProviderDemandTranche {
            priority: force_package::ProviderPriority::Minimum,
            family: ForceFamily::Strike,
            kind: UnitKind::Condor,
            count: 1,
        },
        force_package::ProviderDemandTranche {
            priority: force_package::ProviderPriority::Marginal,
            family: ForceFamily::Suppression,
            kind: UnitKind::Bombard,
            count: 1,
        },
        force_package::ProviderDemandTranche {
            priority: force_package::ProviderPriority::Marginal,
            family: ForceFamily::Strike,
            kind: UnitKind::Buzzard,
            count: 1,
        },
    ];
    plan.desired_artillery = 2;
    plan.desired_strike_aircraft = 2;

    let mut operation = operation(AirOperationPhase::Recon, hidden.tick);
    operation.artillery.clear();
    operation.strike_aircraft.clear();
    let mut planner = planner_with_operation(operation, plan);

    let decision = think(&mut planner, &hidden, &intelligence);

    let operation = planner
        .air_operation()
        .expect("an infeasible admitted package enters observable recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::PreparationInfeasible)
    );
    assert!(
        decision
            .intents
            .iter()
            .all(|intent| !matches!(intent, Intent::TrainAt { .. })),
        "an impossible minimum must block every later provider tranche: {decision:?}"
    );
    assert_eq!(decision.committed_scrap(), 0);
}

#[test]
fn paid_front_queue_survives_prerequisite_loss_but_new_work_does_not() {
    fn run(reobserved_after: u32, paid: bool) -> (StrategicDecision, StrategicPlanner) {
        let admitted_at = 96;
        let deadline = 1_100;
        let mut initial = obs(admitted_at);
        initial.visible.fill(true);
        initial.explored.fill(true);
        initial.scrap = UnitKind::Condor.stats().cost;
        initial
            .my_units
            .retain(|unit| matches!(unit.kind, UnitKind::Kestrel | UnitKind::Bombard));
        initial.my_buildings = vec![
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
            building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
        ];
        initial.my_queues = vec![Vec::new(), Vec::new()];
        initial.my_queue_progress = vec![0, 0];

        let mut plan = connected_test_plan(&initial);
        let package = plan
            .connected_package
            .as_mut()
            .expect("the admitted plan carries its exact package");
        package.preparation_deadline = deadline;
        package.strike = vec![ProviderDemand {
            kind: UnitKind::Condor,
            count: 1,
        }];
        package.provider_priority = vec![
            ProviderDemandTranche {
                priority: force_package::ProviderPriority::Minimum,
                family: ForceFamily::Recon,
                kind: UnitKind::Kestrel,
                count: 1,
            },
            ProviderDemandTranche {
                priority: force_package::ProviderPriority::Minimum,
                family: ForceFamily::Suppression,
                kind: UnitKind::Bombard,
                count: 1,
            },
            ProviderDemandTranche {
                priority: force_package::ProviderPriority::Minimum,
                family: ForceFamily::Strike,
                kind: UnitKind::Condor,
                count: 1,
            },
        ];
        let strike = strike_capability(UnitKind::Condor, initial.faction);
        package.minimum_capability.strike = strike;
        package.useful_capability.strike = strike;
        package.chosen_capability.strike = strike;
        plan.desired_strike_aircraft = 1;
        plan.assembly_timeout = deadline - admitted_at;

        let mut operation = operation(AirOperationPhase::Assemble, admitted_at);
        operation.strike_aircraft.clear();
        let mut planner = planner_with_operation(operation, plan);
        let mut intelligence = knowledge(&initial);

        let commissioned = think(&mut planner, &initial, &intelligence);
        assert!(commissioned.intents.contains(&Intent::TrainAt {
            building: BuildingId(11),
            kind: UnitKind::Condor,
        }));

        let mut later = initial;
        later.tick = admitted_at + Tick::from(reobserved_after);
        later.scrap = if paid {
            0
        } else {
            UnitKind::Condor.stats().cost
        };
        later.visible.fill(false);
        later.enemy_buildings[0].seen = false;
        later.my_buildings.truncate(1);
        later.my_queues.truncate(1);
        later.my_queue_progress.truncate(1);
        if paid {
            later.my_queues[0] = vec![UnitKind::Condor];
            later.my_queue_progress[0] = reobserved_after;
        }
        intelligence.update(&later);

        let decision = think(&mut planner, &later, &intelligence);
        (decision, planner)
    }

    for reobserved_after in [12, 24, 60] {
        let (first_decision, first_planner) = run(reobserved_after, true);
        let (second_decision, second_planner) = run(reobserved_after, true);
        assert_eq!(first_decision, second_decision);
        assert_eq!(first_planner, second_planner);

        let operation = first_planner
            .air_operation()
            .expect("the paid provider keeps the operation active");
        assert_eq!(operation.phase(), AirOperationPhase::Assemble);
        assert_eq!(operation.recovery_reason(), None);
        assert!(first_decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Condor,
                ..
            }
        )));
        assert_eq!(first_decision.committed_scrap(), 0);
    }

    let (decision, planner) = run(12, false);
    let operation = planner
        .air_operation()
        .expect("the infeasible operation remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::PreparationInfeasible)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Condor,
            ..
        }
    )));
}

#[test]
fn admitted_connected_package_recovers_when_ground_production_becomes_blocked() {
    let mut observation = developed_connected_obs(120);
    let mut intelligence = knowledge(&observation);
    let mut planner = StrategicPlanner::new();

    think(&mut planner, &observation, &intelligence);
    assert!(planner.air_operation().is_some_and(|operation| {
        operation.phase() <= AirOperationPhase::Assemble && operation.recovery_reason().is_none()
    }));

    observation.tick += 1;
    observation.my_units.retain(|unit| !is_artillery(unit.kind));
    observation.known_scrap = observation
        .my_buildings
        .iter()
        .filter(|building| {
            matches!(
                building.kind,
                BuildingKind::Fabricator | BuildingKind::Crucible
            )
        })
        .flat_map(|building| {
            oxide_sim::geometry::rect_adjacent_tiles(
                building.anchor,
                building.kind.tier_stats(building.tier).size,
            )
        })
        .map(|tile| (tile, 1))
        .collect();
    observation
        .known_scrap
        .sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
    observation.known_scrap.dedup_by_key(|(tile, _)| *tile);
    intelligence.update(&observation);

    think(&mut planner, &observation, &intelligence);
    let operation = planner
        .air_operation()
        .expect("the failed preparation remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::PreparationInfeasible)
    );
}

#[test]
fn connected_preparation_skips_stranded_low_id_providers() {
    let mut observation = obs(300);
    observation.visible.fill(true);
    observation.explored.fill(true);
    observation.scrap = 0;
    observation.my_buildings.clear();
    observation.my_queues.clear();
    observation.my_units = vec![
        own(1, UnitKind::Kestrel, TilePos::new(2, 2)),
        own(2, UnitKind::Bombard, TilePos::new(28, 17)),
        own(3, UnitKind::Condor, TilePos::new(2, 2)),
        own(4, UnitKind::Condor, TilePos::new(2, 2)),
        own(11, UnitKind::Kestrel, TilePos::new(8, 10)),
        own(12, UnitKind::Bombard, TilePos::new(9, 10)),
    ];
    observation
        .my_units
        .extend((13..=24).map(|id| own(id, UnitKind::Condor, TilePos::new(8, 10))));
    let air_pocket = [
        TilePos::new(1, 2),
        TilePos::new(2, 1),
        TilePos::new(2, 3),
        TilePos::new(3, 2),
    ];
    let ground_pocket = [
        TilePos::new(27, 17),
        TilePos::new(28, 16),
        TilePos::new(28, 18),
        TilePos::new(29, 17),
    ];
    observation.known_peaks = air_pocket.to_vec();
    observation
        .known_peaks
        .sort_unstable_by_key(|tile| (tile.y, tile.x));
    observation.known_rock = air_pocket.into_iter().chain(ground_pocket).collect();
    observation
        .known_rock
        .sort_unstable_by_key(|tile| (tile.y, tile.x));

    let intelligence = knowledge(&observation);
    let mut operation = operation(AirOperationPhase::Recon, observation.tick);
    operation.scout = None;
    operation.artillery.clear();
    operation.strike_aircraft.clear();
    let mut planner = planner_with_operation(operation, connected_test_plan(&observation));

    let decision = think(&mut planner, &observation, &intelligence);
    let operation = planner
        .air_operation()
        .expect("reachable replacements keep preparation active");
    assert_ne!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(operation.scout, Some(UnitId(11)));
    assert_eq!(operation.artillery, [UnitId(12)]);
    assert!(!operation.strike_aircraft.is_empty());
    assert!(operation.strike_aircraft.iter().all(|id| id.0 >= 13));
    assert!(
        [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
            .iter()
            .all(|id| !decision.reservations.contains(id))
    );
}

#[test]
fn committed_connected_operation_freezes_exact_members() {
    let mut battle = obs(300);
    battle.visible.fill(true);
    battle.explored.fill(true);
    battle.my_units.extend([
        own(5, UnitKind::Bombard, TilePos::new(9, 10)),
        own(6, UnitKind::Condor, TilePos::new(4, 12)),
    ]);
    battle.my_units.sort_unstable_by_key(|unit| unit.id);

    let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
    operation.artillery = vec![UnitId(2)];
    operation.strike_aircraft = vec![UnitId(3), UnitId(4)];
    let plan = connected_test_plan(&battle);
    let mut planner = planner_with_operation(operation, plan);
    let mut intelligence = knowledge(&battle);

    let suppression = think(&mut planner, &battle, &intelligence);
    let operation = planner
        .air_operation()
        .expect("the committed package remains active");
    assert_eq!(operation.phase(), AirOperationPhase::Verify);
    assert_eq!(
        suppression.reservations,
        [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
    );
    assert!(!suppression.reservations.contains(&UnitId(5)));
    assert!(!suppression.reservations.contains(&UnitId(6)));

    battle.tick += 1;
    intelligence.update(&battle);
    let strike = think(&mut planner, &battle, &intelligence);
    assert!(strike.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(80)),
    }));
    assert_eq!(
        planner.air_operation().unwrap().strike_aircraft,
        [UnitId(3), UnitId(4)]
    );
}

#[test]
fn connected_operation_does_not_wait_for_an_arbitrary_second_bombard() {
    let mut identity = profile();
    identity.primary = Specialty::Support;
    identity.secondary = Specialty::Siege;
    identity.traits.air = 60;
    identity.traits.siege = 60;

    let mut battle = obs(300);
    battle.explored.fill(true);
    let mut operation = operation(AirOperationPhase::Assemble, battle.tick);
    operation.artillery = vec![UnitId(2)];
    operation.strike_aircraft = vec![UnitId(3), UnitId(4)];
    let plan = derived_connected_test_plan(&identity, &battle)
        .expect("the observed force can field a connected package");
    assert_eq!(preferred_artillery(&identity, &battle), UnitKind::Bombard);
    assert_eq!(plan.desired_artillery, 1);
    let mut planner = planner_with_operation(operation, plan);
    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    let intel = knowledge(&battle);

    let waiting = planner.think_alone(&identity, tuning, &battle, &intel, HOME, &[]);

    let operation = planner.air_operation().expect("operation advances");
    assert_eq!(operation.phase(), AirOperationPhase::SuppressAa);
    assert_eq!(operation.artillery, [UnitId(2)]);
    assert!(waiting.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(2)],
        goal: staging(HOME, TARGET),
    }));
}

#[test]
fn a_wealthy_scattering_like_bot_starts_airborne_bombers_without_lift_support() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let observation = wealthy_island_obs(5_016, 1);
    let intel = knowledge(&observation);
    let mut identity = profile();
    identity.primary = Specialty::Support;
    identity.secondary = Specialty::Greed;
    identity.traits.air = 48;
    identity.traits.siege = 20;
    let mut planner = StrategicPlanner::new();

    planner.think_alone_with(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intel,
        HOME,
        coordination(&fixture_planning, None),
    );

    assert_eq!(
        planner
            .air_operation()
            .expect("the disconnected enclave independently starts an air operation")
            .target,
        TARGET
    );
    let plan = planner.air_plan().expect("the operation owns a plan");
    assert_eq!(plan.suppression, AirSuppression::Airborne);
    assert!(plan.desired_strike_aircraft >= 4);
    assert_eq!(plan.desired_artillery, 0);
}

#[test]
fn a_target_without_a_known_ground_doorstep_cannot_trigger_the_uncapped_island_plan() {
    let mut observation = wealthy_island_obs(5_016, 1);
    observation
        .known_rock
        .extend(oxide_sim::geometry::rect_adjacent_tiles(
            TARGET,
            BuildingKind::Crucible.base_stats().size,
        ));
    observation
        .known_rock
        .sort_unstable_by_key(|tile| (tile.y, tile.x));
    observation.known_rock.dedup();
    let intel = knowledge(&observation);
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_045,
    ));
    assert_eq!(identity.primary, Specialty::Air);
    let mut planner = StrategicPlanner::new();

    let decision = planner.think_alone(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intel,
        HOME,
        &[],
    );

    assert_eq!(decision, StrategicDecision::default());
    assert!(
        planner.air_operation().is_none(),
        "an objective with no known ground doorstep must not start either assault playbook"
    );
    assert!(planner.air_plan().is_none());
}

#[test]
fn a_preexisting_lift_makes_the_second_starting_air_operation_inherit_its_exact_objective() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = wealthy_island_obs(5_016, 1);
    let lift_target = TilePos::new(24, 15);
    observation
        .enemy_buildings
        .push(building(81, 1, BuildingKind::Foundry, lift_target, true));
    let intel = knowledge(&observation);
    let request = LiftSupportRequest {
        player: PlayerId(1),
        target: lift_target,
        planned_drops: vec![TilePos::new(22, 14), TilePos::new(23, 14)],
    };
    let mut planner = StrategicPlanner::new();

    planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intel,
        HOME,
        coordination(&fixture_planning, Some(&request)),
    );

    let operation = planner
        .air_operation()
        .expect("the lift's exact objective starts the matching air operation");
    assert_eq!(operation.target_player, request.player);
    assert_eq!(operation.target, request.target);
    assert_eq!(operation.target_id, Some(BuildingId(81)));
    assert_eq!(
        planner.air_plan().map(|plan| plan.suppression),
        Some(AirSuppression::Airborne)
    );
}

#[test]
fn partial_ground_knowledge_stays_unknown_without_a_public_briefing() {
    let mut observation = wealthy_island_obs(5_000, 1);
    observation.known_rock.clear();
    for tile in
        oxide_sim::geometry::rect_adjacent_tiles(HOME, BuildingKind::Foundry.base_stats().size)
            .chain(oxide_sim::geometry::rect_adjacent_tiles(
                TARGET,
                BuildingKind::Crucible.base_stats().size,
            ))
    {
        explore(&mut observation, tile);
    }
    let target = BuildingContact {
        player: PlayerId(1),
        kind: BuildingKind::Crucible,
        anchor: TARGET,
        hp: BuildingKind::Crucible.base_stats().max_hp,
        tier: 0,
        built: true,
        id: Some(BuildingId(80)),
        evidence: ContactEvidence::Current,
        last_seen: Some(observation.tick),
    };

    assert_eq!(
        known_ground_connection(
            &observation,
            HOME,
            TARGET,
            BuildingKind::Crucible.base_stats().size,
            None,
        ),
        None,
        "an optimistic route through unexplored ground is not proof of either connection state"
    );
    assert!(!wealthy_island_target(
        &profile(),
        &observation,
        HOME,
        &target,
        None,
    ));

    let open_public_map = public_map_with_terrain(&observation, []);
    assert_eq!(
        known_ground_connection(
            &observation,
            HOME,
            TARGET,
            BuildingKind::Crucible.base_stats().size,
            Some(&open_public_map),
        ),
        Some(true),
        "the public map proves both endpoints share one ground component"
    );
    let divided_public_map = public_map_with_terrain(
        &observation,
        (0..observation.map_height).map(|y| (TilePos::new(16, y), Terrain::Peak)),
    );
    assert_eq!(
        known_ground_connection(
            &observation,
            HOME,
            TARGET,
            BuildingKind::Crucible.base_stats().size,
            Some(&divided_public_map),
        ),
        Some(false),
    );
    assert!(wealthy_island_target(
        &profile(),
        &observation,
        HOME,
        &target,
        Some(&divided_public_map),
    ));

    for x in HOME.x + 2..TARGET.x {
        explore(&mut observation, TilePos::new(x, HOME.y));
    }
    assert_eq!(
        known_ground_connection(
            &observation,
            HOME,
            TARGET,
            BuildingKind::Crucible.base_stats().size,
            None,
        ),
        Some(true),
    );
    assert!(
        !wealthy_island_target(&profile(), &observation, HOME, &target, None),
        "a fully explored open road keeps the ordinary ground war available"
    );
}

#[test]
fn an_unexplored_remembered_target_on_the_public_home_landmass_uses_connected_recon() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut first_sighting = wealthy_island_obs(4_992, 1);
    first_sighting.known_rock.clear();
    let mut intel = knowledge(&first_sighting);

    let mut hidden = wealthy_island_obs(5_016, 1);
    hidden.known_rock.clear();
    hidden.enemy_buildings[0].seen = false;
    assert!(hidden.explored.iter().all(|explored| !explored));
    intel.update(&hidden);
    let public_map = public_map_with_terrain(&hidden, []);
    let mut planner = StrategicPlanner::new();

    planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &hidden,
        &intel,
        HOME,
        StrategicCoordination {
            planning: Some(&fixture_planning),
            public_map: Some(&public_map),
            ..coordination(&fixture_planning, None)
        },
    );

    let operation = planner
        .air_operation()
        .expect("the remembered connected objective remains eligible for reconnaissance");
    assert!(!operation.assault_admitted());
    assert_eq!(operation.target, TARGET);
    assert_eq!(
        planner.air_plan().map(|plan| plan.suppression),
        Some(AirSuppression::GroundArtillery),
        "publicly connected terrain must not enter the wealthy island doctrine"
    );
}

#[test]
fn a_remembered_high_value_target_cannot_hide_a_current_island_objective() {
    let mut first_sighting = wealthy_island_obs(4_999, 1);
    let mut intel = knowledge(&first_sighting);
    let island_foundry = TARGET.offset(0, 5);
    first_sighting.tick = 5_016;
    first_sighting.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, TARGET, false),
        building(81, 1, BuildingKind::Foundry, island_foundry, true),
    ];
    intel.update(&first_sighting);
    let mut identity = profile();
    identity.primary = Specialty::Support;
    identity.secondary = Specialty::Greed;
    identity.traits.air = 48;
    identity.traits.siege = 20;
    let mut planner = StrategicPlanner::new();

    planner.think_alone(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &first_sighting,
        &intel,
        HOME,
        &[],
    );

    let operation = planner
        .air_operation()
        .expect("the current disconnected objective starts an air operation");
    assert_eq!(operation.target_id, Some(BuildingId(81)));
    assert_eq!(operation.target, island_foundry);
    assert_eq!(
        planner.air_plan().map(|plan| plan.suppression),
        Some(AirSuppression::Airborne)
    );
}

#[test]
fn a_wealthy_island_bot_reconnoiters_a_stale_building_ghost() {
    let first_sighting = wealthy_island_obs(100, 1);
    let mut intel = knowledge(&first_sighting);
    let mut hidden = wealthy_island_obs(10_008, 1);
    hidden.enemy_buildings[0].seen = false;
    hidden.my_units[0].tile = HOME;
    intel.update(&hidden);
    let mut identity = profile();
    identity.primary = Specialty::Support;
    identity.secondary = Specialty::Greed;
    identity.traits.air = 48;
    identity.traits.siege = 20;
    let mut planner = StrategicPlanner::new();

    let decision = planner.think_alone(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &hidden,
        &intel,
        HOME,
        &[],
    );

    let operation = planner
        .air_operation()
        .expect("a persistent building ghost warrants honest reconnaissance");
    assert_eq!(operation.phase(), AirOperationPhase::Recon);
    assert!(!operation.assault_admitted());
    assert_eq!(operation.target, TARGET);
    assert!(decision.intents.iter().any(|intent| matches!(
        intent,
        Intent::MoveUnits { units, .. } if units == &[UnitId(1)]
    )));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
    assert!(operation.artillery.is_empty());
    assert!(operation.strike_aircraft.is_empty());
    assert_eq!(decision.reservations, [UnitId(1)]);
    assert_eq!(decision.committed_scrap(), 0);
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt { kind, .. } if *kind != UnitKind::Kestrel
    )));
}

#[test]
fn remembered_recon_claims_only_the_missing_scouts_capacity() {
    let first_sighting = wealthy_island_obs(4_800, 1);
    let mut intel = knowledge(&first_sighting);
    let mut ghost = wealthy_island_obs(4_992, 1);
    ghost.enemy_buildings[0].seen = false;
    ghost.my_units.retain(|unit| unit.kind != UnitKind::Kestrel);
    intel.update(&ghost);
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_045,
    ));
    assert_eq!(identity.primary, Specialty::Air);
    let mut planner = StrategicPlanner::new();

    let decision = planner.think_alone(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &ghost,
        &intel,
        HOME,
        &[],
    );

    let operation = planner
        .air_operation()
        .expect("the remembered objective admits scout-only reconnaissance");
    assert!(!operation.assault_admitted());
    assert_eq!(operation.scout, None);
    assert!(operation.artillery.is_empty());
    assert!(operation.strike_aircraft.is_empty());
    assert_eq!(
        planner.remaining_airwork_ticks(&ghost, None),
        u64::from(UnitKind::Kestrel.stats().train_ticks)
    );
    assert_eq!(decision.committed_scrap(), UnitKind::Kestrel.stats().cost);
    assert!(matches!(
        decision.intents.as_slice(),
        [Intent::TrainAt {
            kind: UnitKind::Kestrel,
            ..
        }]
    ));
}

#[test]
fn remembered_recon_gives_a_late_scout_a_fresh_flight_window() {
    let first_sighting = wealthy_island_obs(4_800, 1);
    let mut intel = knowledge(&first_sighting);
    let mut ghost = wealthy_island_obs(4_992, 1);
    ghost.enemy_buildings[0].seen = false;
    ghost.my_units.retain(|unit| unit.kind != UnitKind::Kestrel);
    intel.update(&ghost);
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_045,
    ));
    let mut tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    tuning.tactical_memory = 5_000;
    let mut planner = StrategicPlanner::new();

    planner.think_alone(&identity, tuning, &ghost, &intel, HOME, &[]);
    let admitted_at = planner
        .air_operation()
        .expect("the remembered objective begins scout-only reconnaissance")
        .phase_started_at;
    assert_eq!(admitted_at, ghost.tick);

    let mut scout_ready = ghost;
    scout_ready.tick = admitted_at
        + phase_timeout(
            AirOperationPhase::Recon,
            &AirPlan::island(&identity, &scout_ready),
        )
        - 12;
    scout_ready.my_units.push(own(99, UnitKind::Kestrel, HOME));
    scout_ready.my_units.sort_unstable_by_key(|unit| unit.id);
    intel.update(&scout_ready);
    let dispatch = planner.think_alone(&identity, tuning, &scout_ready, &intel, HOME, &[]);

    let operation = planner
        .air_operation()
        .expect("the late scout must extend reconnaissance instead of timing out");
    assert_eq!(operation.phase(), AirOperationPhase::Recon);
    assert_eq!(operation.scout, Some(UnitId(99)));
    assert_eq!(operation.phase_started_at, scout_ready.tick);
    assert_eq!(operation.recovery_reason(), None);
    assert!(dispatch.intents.iter().any(|intent| matches!(
        intent,
        Intent::MoveUnits { units, .. } if units == &[UnitId(99)]
    )));

    let assigned_at = scout_ready.tick;
    let mut after_old_deadline = scout_ready;
    after_old_deadline.tick = admitted_at
        + phase_timeout(
            AirOperationPhase::Recon,
            &AirPlan::island(&identity, &after_old_deadline),
        )
        + 12;
    let scout = after_old_deadline
        .my_units
        .iter_mut()
        .find(|unit| unit.id == UnitId(99))
        .expect("the assigned scout remains alive");
    scout.idle = false;
    scout.tile = HOME.offset(1, 0);
    intel.update(&after_old_deadline);
    let after = planner.think_alone(&identity, tuning, &after_old_deadline, &intel, HOME, &[]);

    let operation = planner
        .air_operation()
        .expect("the reset window must remain active past the original deadline");
    assert_eq!(operation.phase(), AirOperationPhase::Recon);
    assert_eq!(operation.scout, Some(UnitId(99)));
    assert_eq!(operation.phase_started_at, assigned_at);
    assert_eq!(operation.recovery_reason(), None);
    assert!(
        after.intents.is_empty(),
        "the accepted flight should persist"
    );
}

#[test]
fn remembered_recon_buys_only_the_scout_not_owned_by_another_question() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let first_sighting = wealthy_island_obs(4800, 1);
    let mut intelligence = knowledge(&first_sighting);
    let mut ghost = wealthy_island_obs(4992, 1);
    ghost.enemy_buildings[0].seen = false;
    ghost.my_units.retain(|unit| unit.kind != UnitKind::Kestrel);
    let factory = ghost
        .my_buildings
        .iter()
        .position(|building| building.kind == BuildingKind::Airworks)
        .unwrap();
    let producer = ghost.my_buildings[factory].id;
    ghost.my_queues[factory] = vec![UnitKind::Kestrel];
    intelligence.update(&ghost);
    let identity = profile();
    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    for (foreign, expected) in [(vec![], 0), (vec![(producer, UnitKind::Kestrel, 0)], 1)] {
        let mut planner = StrategicPlanner::new();
        let result = planner.think_after_connected_adjudication(
            StrategicThinkContext::new(
                &identity,
                tuning,
                &ghost,
                &intelligence,
                HOME,
                coordination(&fixture_planning, None),
            )
            .with_paid_exclusions(&foreign),
        );
        assert!(planner.air_operation().is_some());
        assert_eq!(
            result
                .decision
                .intents
                .iter()
                .filter(|intent| {
                    matches!(
                        intent,
                        Intent::TrainAt {
                            kind: UnitKind::Kestrel,
                            ..
                        }
                    )
                })
                .count(),
            expected
        );
    }
}

#[test]
fn remembered_recon_aborts_when_the_scout_cannot_cross_known_peaks() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let first_sighting = wealthy_island_obs(4_800, 1);
    let mut intel = knowledge(&first_sighting);
    let mut ghost = wealthy_island_obs(4_992, 1);
    ghost.enemy_buildings[0].seen = false;
    ghost.my_units[0].tile = HOME;
    ghost.known_peaks = (0..ghost.map_height).map(|y| TilePos::new(8, y)).collect();
    intel.update(&ghost);
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_045,
    ));
    assert_eq!(identity.primary, Specialty::Air);
    let mut planner = StrategicPlanner::new();

    assert!(
        planner
            .prospective_recon_target(StrategicThinkContext::new(
                &identity,
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &ghost,
                &intel,
                HOME,
                coordination(&fixture_planning, None),
            ))
            .is_none(),
        "an unreachable live scout cannot create a phantom carrier floor"
    );

    let decision = planner.think_alone(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &ghost,
        &intel,
        HOME,
        &[],
    );

    let operation = planner
        .air_operation()
        .expect("the failed reconnaissance remains observable for one think");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
    assert_eq!(decision.committed_scrap(), 0);
}

#[test]
fn prospective_recon_releases_the_carrier_floor_past_the_active_memory_boundary() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let seen = obs(100);
    let mut intel = knowledge(&seen);
    let boundary = seen.tick + ACTIVE_OPERATION_TARGET_MEMORY;
    let mut hidden = obs(boundary);
    hidden.enemy_buildings[0].seen = false;
    intel.update(&hidden);
    let identity = profile();
    let tuning = DifficultyTuning::for_level(identity.difficulty);
    let mut op = operation(AirOperationPhase::Recon, boundary);
    op.stage = AirStage::Watching;
    op.started_at = boundary;
    op.phase_started_at = boundary;
    op.artillery.clear();
    op.strike_aircraft.clear();
    let mut planner = planner_with_operation(op, AirPlan::remembered_connected(&hidden));

    assert!(
        planner
            .prospective_recon_target(StrategicThinkContext::new(
                &identity,
                tuning,
                &hidden,
                &intel,
                HOME,
                coordination(&fixture_planning, None),
            ))
            .is_some(),
        "the active-operation memory boundary remains inclusive"
    );

    hidden.tick = hidden.tick.saturating_add(1);
    intel.update(&hidden);
    assert!(
        planner
            .prospective_recon_target(StrategicThinkContext::new(
                &identity,
                tuning,
                &hidden,
                &intel,
                HOME,
                coordination(&fixture_planning, None),
            ))
            .is_none(),
        "expired active Recon cannot create a phantom carrier floor"
    );

    let decision = planner.think_alone(&identity, tuning, &hidden, &intel, HOME, &[]);
    let operation = planner
        .air_operation()
        .expect("the stale operation remains observable for its recovery think");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::StaleIntelligence)
    );
    assert_eq!(decision.committed_scrap(), 0);
}

#[test]
fn prospective_recon_releases_the_carrier_floor_for_a_lost_dispatched_scout() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let seen = obs(100);
    let mut intel = knowledge(&seen);
    let mut hidden = obs(120);
    hidden.enemy_buildings[0].seen = false;
    hidden.my_units.retain(|unit| unit.id != UnitId(1));
    hidden
        .my_units
        .push(own(5, UnitKind::Kestrel, TilePos::new(5, 10)));
    hidden.my_units.sort_unstable_by_key(|unit| unit.id);
    intel.update(&hidden);
    let identity = profile();
    let tuning = DifficultyTuning::for_level(identity.difficulty);
    let mut op = operation(AirOperationPhase::Recon, hidden.tick);
    op.stage = AirStage::Watching;
    op.started_at = hidden.tick;
    op.phase_started_at = hidden.tick;
    op.scout_dispatch = Some((UnitId(1), TARGET));
    op.artillery.clear();
    op.strike_aircraft.clear();
    let mut planner = planner_with_operation(op, AirPlan::remembered_connected(&hidden));

    assert!(
        planner
            .prospective_recon_target(StrategicThinkContext::new(
                &identity,
                tuning,
                &hidden,
                &intel,
                HOME,
                coordination(&fixture_planning, None),
            ))
            .is_none(),
        "a lost dispatched scout cannot reserve carrier capital for its replacement"
    );

    let decision = planner.think_alone(&identity, tuning, &hidden, &intel, HOME, &[]);
    assert_eq!(
        planner.terminal_outcome(),
        Some(AirOperationOutcome::Aborted {
            player: PlayerId(1),
            target: TARGET,
        }),
        "a lost scout with no surviving claims completes recovery immediately"
    );
    assert!(!decision.reservations.contains(&UnitId(5)));
    assert_eq!(decision.committed_scrap(), 0);
}

#[test]
fn remembered_connected_recon_respects_publicly_known_peaks_before_sighting_them() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let first_sighting = developed_connected_obs(96);
    let mut intel = knowledge(&first_sighting);
    let mut ghost = developed_connected_obs(120);
    ghost.scrap = 0;
    ghost.visible.fill(false);
    ghost.explored.fill(false);
    ghost.enemy_buildings[0].seen = false;
    ghost.my_units[0].tile = HOME;
    intel.update(&ghost);
    let public_map = public_map_with_terrain(
        &ghost,
        (0..ghost.map_height).map(|y| (TilePos::new(8, y), Terrain::Peak)),
    );
    let identity = profile();
    let mut planner = StrategicPlanner::new();

    let decision = planner
        .think_alone_with(
            &identity,
            DifficultyTuning::for_level(identity.difficulty),
            &ghost,
            &intel,
            HOME,
            StrategicCoordination {
                planning: Some(&fixture_planning),
                public_map: Some(&public_map),
                ..coordination(&fixture_planning, None)
            },
        )
        .decision;

    let operation = planner
        .air_operation()
        .expect("the refused reconnaissance remains observable during recovery");
    assert!(!operation.assault_admitted());
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { goal, .. } if goal.x > 8
    )));
}

#[test]
fn every_difficulty_waits_for_shared_current_sight_before_claiming_an_assault() {
    let mut snapshots = Vec::new();
    for difficulty in BotDifficulty::ALL {
        let tuning = DifficultyTuning::for_level(difficulty);
        let identity =
            ResolvedProfile::resolve(BotConfig::scripted(difficulty, BotStance::Balanced, 20_045));
        assert_eq!(identity.primary, Specialty::Air);
        let first = wealthy_island_obs(4_800, 1);
        let mut intel = knowledge(&first);
        let mut ghost = wealthy_island_obs(4_992, 1);
        ghost.enemy_buildings[0].seen = false;
        ghost.my_units[0].tile = HOME;
        intel.update(&ghost);
        let mut planner = StrategicPlanner::new();

        let recon = planner.think_alone(&identity, tuning, &ghost, &intel, HOME, &[]);
        let ghost_operation = planner.air_operation().unwrap();
        assert!(!ghost_operation.assault_admitted(), "{difficulty:?}");
        let admitted_at = planner
            .air_admitted_at()
            .expect("remembered reconnaissance owns an admission tick");
        assert_eq!(admitted_at, ghost.tick, "{difficulty:?}");
        assert!(ghost_operation.artillery.is_empty(), "{difficulty:?}");
        assert!(ghost_operation.strike_aircraft.is_empty(), "{difficulty:?}");
        assert_eq!(recon.reservations, [UnitId(1)], "{difficulty:?}");
        assert_eq!(recon.committed_scrap(), 0, "{difficulty:?}");

        let current = wealthy_island_obs(5_016, 1);
        intel.update(&current);
        planner.think_alone(&identity, tuning, &current, &intel, HOME, &[]);
        let admitted = planner.air_operation().unwrap();
        assert!(admitted.assault_admitted(), "{difficulty:?}");
        assert_eq!(admitted.started_at, 5_016, "{difficulty:?}");
        assert_eq!(
            planner.air_admitted_at(),
            Some(admitted_at),
            "reacquiring the target must not reorder the operation behind later commitments"
        );
        let plan = planner.air_plan().unwrap();
        snapshots.push((
            admitted.artillery.clone(),
            admitted.strike_aircraft.clone(),
            plan.desired_artillery,
            plan.desired_strike_aircraft,
            plan.desired_screen,
        ));
    }

    assert!(snapshots.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn remembered_connected_target_admits_the_normal_combined_plan_when_reacquired() {
    let reveal_ground_route = |observation: &mut Observation| {
        observation.known_rock.clear();
        observation.my_buildings.push(building(
            40,
            0,
            BuildingKind::Fabricator,
            TilePos::new(12, 2),
            true,
        ));
        observation.my_queues.push(Vec::new());
        for x in HOME.x + 2..TARGET.x {
            explore(observation, TilePos::new(x, HOME.y));
        }
    };
    let first = {
        let mut observation = wealthy_island_obs(4_800, 1);
        reveal_ground_route(&mut observation);
        observation
    };
    let mut intel = knowledge(&first);
    let mut ghost = wealthy_island_obs(4_992, 1);
    reveal_ground_route(&mut ghost);
    ghost.enemy_buildings[0].seen = false;
    ghost.my_units[0].tile = HOME;
    intel.update(&ghost);
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_045,
    ));
    assert_eq!(identity.primary, Specialty::Air);
    let mut planner = StrategicPlanner::new();

    let recon = planner.think_alone(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &ghost,
        &intel,
        HOME,
        &[],
    );
    let operation = planner
        .air_operation()
        .expect("the remembered connected objective starts scout-only reconnaissance");
    assert!(!operation.assault_admitted());
    assert_eq!(recon.reservations, [UnitId(1)]);
    assert_eq!(
        planner.air_plan().map(|plan| plan.suppression),
        Some(AirSuppression::GroundArtillery)
    );

    let mut current = wealthy_island_obs(5_016, 1);
    reveal_ground_route(&mut current);
    intel.update(&current);
    planner.think_alone(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &current,
        &intel,
        HOME,
        &[],
    );

    let admitted = planner
        .air_operation()
        .expect("current sight admits the connected assault");
    assert!(admitted.assault_admitted());
    assert_eq!(admitted.started_at, current.tick);
    let plan = planner.air_plan().expect("the assault owns a plan");
    assert_eq!(plan.suppression, AirSuppression::GroundArtillery);
    assert!(plan.desired_artillery > 0);
    assert_eq!(plan.desired_screen, 0);
    assert!(plan.connected_package.is_some());
}

#[test]
fn a_large_island_wave_schedules_screen_then_bombers_across_airworks() {
    let observation = wealthy_island_obs(5_016, 3);
    let intel = knowledge(&observation);
    let mut identity = profile();
    identity.primary = Specialty::Support;
    identity.secondary = Specialty::Greed;
    identity.traits.air = 48;
    identity.traits.siege = 20;
    let mut planner = StrategicPlanner::new();

    let decision = planner.think_alone(
        &identity,
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intel,
        HOME,
        &[],
    );

    let plan = planner.air_plan().expect("the operation owns a plan");
    assert_eq!(plan.desired_strike_aircraft, 6);
    assert_eq!(plan.desired_screen, 2);
    let training: Vec<_> = decision
        .intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::TrainAt { building, kind } => Some((*building, *kind)),
            _ => None,
        })
        .collect();
    assert_eq!(
        &training[..2],
        &[
            (BuildingId(23), UnitKind::Buzzard),
            (BuildingId(24), UnitKind::Buzzard),
        ],
        "the cheap suppression screen is available before the long bomber batch"
    );
    assert_eq!(
        training
            .iter()
            .filter(|(_, kind)| *kind == UnitKind::Condor)
            .count(),
        4,
        "two existing strike aircraft contribute to the current six-aircraft wing"
    );
    for airworks in [BuildingId(23), BuildingId(24), BuildingId(25)] {
        assert_eq!(
            training
                .iter()
                .filter(|(building, _)| *building == airworks)
                .count(),
            2,
            "equal queue loads must spread deterministically"
        );
    }
    assert_eq!(
        planner.remaining_airwork_ticks(&observation, None),
        3_560,
        "capacity planning sees exact unfinished screen and bomber training time"
    );
    let mut arriving = observation.clone();
    arriving.my_units.push(own(900, UnitKind::Buzzard, HOME));
    let mut proposed = AirMembership::from_active(planner.air.as_ref().unwrap());
    proposed.screen.push(UnitId(900));
    assert_eq!(
        planner.remaining_airwork_ticks(&arriving, Some(&proposed)),
        3_380
    );
    assert_eq!(planner.remaining_airwork_ticks(&arriving, None), 3_560);
    let mut queued = observation.clone();
    queued.my_queues[3] = vec![UnitKind::Buzzard, UnitKind::Condor];
    queued.my_queues[4] = vec![UnitKind::Buzzard, UnitKind::Condor];
    queued.my_queues[5] = vec![UnitKind::Condor, UnitKind::Condor];
    assert_eq!(
        planner.remaining_airwork_ticks(&queued, None),
        3_560,
        "queued aircraft remain real factory work until they complete"
    );
    assert!(
        plan.assembly_timeout >= 2_660,
        "the deadline covers the faction's exact scheduled production time"
    );
}

#[test]
fn connected_capacity_uses_exact_paid_front_progress() {
    let mut observation = obs(5_016);
    observation.my_buildings = vec![
        building(10, 0, BuildingKind::Airworks, TilePos::new(2, 2), true),
        building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
    ];
    observation.my_queues = vec![vec![UnitKind::Condor], vec![UnitKind::Condor]];
    observation.my_queue_progress = vec![UnitKind::Condor.stats().train_ticks, 1];
    let mut plan = connected_test_plan(&observation);
    let package = plan
        .connected_package
        .as_mut()
        .expect("the operation owns a connected package");
    package.recon.clear();
    package.strike = vec![ProviderDemand {
        kind: UnitKind::Condor,
        count: 6,
    }];
    let planner = planner_with_operation(
        operation(AirOperationPhase::Assemble, observation.tick),
        plan,
    );

    assert_eq!(
        planner.remaining_airwork_ticks(&observation, None),
        2_400,
        "a ready-but-not-yet-spawned front still needs one authoritative production tick"
    );
    observation.my_queue_progress.fill(0);
    assert_eq!(
        planner.remaining_airwork_ticks(&observation, None),
        3_200,
        "without visible progress all four missing bombers retain their full training work"
    );
}

#[test]
fn every_difficulty_freezes_the_same_growing_air_roster_at_shared_admission() {
    let mut snapshots = Vec::new();
    for difficulty in BotDifficulty::ALL {
        let tuning = DifficultyTuning::for_level(difficulty);
        let identity =
            ResolvedProfile::resolve(BotConfig::scripted(difficulty, BotStance::Balanced, 20_045));
        assert!(matches!(identity.primary, Specialty::Air));
        let mut planner = StrategicPlanner::new();
        let mut shared = wealthy_island_obs(5_016, 1);
        let mut intel = None;
        for tick in 4_993_u64..=5_016 {
            if !tick.is_multiple_of(tuning.cadence) {
                continue;
            }
            shared = wealthy_island_obs(tick, 1);
            add_renewable_economy(&mut shared, usize::try_from((tick - 4_992) / 3).unwrap());
            let current_intel = intel.get_or_insert_with(|| knowledge(&shared));
            if current_intel.observed_at() != Some(shared.tick) {
                current_intel.update(&shared);
            }
            planner.think_alone(&identity, tuning, &shared, current_intel, HOME, &[]);
            if tick < 5_016 {
                assert!(
                    planner.air_operation().is_none(),
                    "{difficulty:?} at {tick}"
                );
            }
        }
        let expected = AirPlan::island(&identity, &shared);
        let operation = planner.air_operation().unwrap();
        let plan = planner.air_plan().unwrap();

        assert_eq!(operation.started_at, 5_016, "{difficulty:?}");
        assert_eq!(
            plan.desired_strike_aircraft,
            expected.desired_strike_aircraft
        );
        assert_eq!(plan.desired_screen, expected.desired_screen);
        assert_eq!(plan.assembly_timeout, expected.assembly_timeout);
        snapshots.push((plan.desired_strike_aircraft, plan.desired_screen));
    }

    assert!(snapshots.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn fighter_reinforcements_do_not_inflate_an_admitted_airborne_plan() {
    let mut reinforced = wealthy_island_obs(5_016, 1);
    let identity = profile();
    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    let mut intel = knowledge(&reinforced);
    let mut planner = StrategicPlanner::new();
    planner.think_alone(&identity, tuning, &reinforced, &intel, HOME, &[]);
    let frozen = planner.air_plan().cloned().unwrap();

    reinforced.my_units.extend((100..=139).map(|id| {
        own(
            id,
            UnitKind::Sentinel,
            TilePos::new(6 + i32::try_from(id % 8).unwrap(), 6),
        )
    }));
    reinforced.my_units.sort_unstable_by_key(|unit| unit.id);
    reinforced.tick += tuning.cadence;
    intel.update(&reinforced);
    planner.think_alone(&identity, tuning, &reinforced, &intel, HOME, &[]);

    let current = AirPlan::island(&identity, &reinforced);
    assert!(current.desired_strike_aircraft > frozen.desired_strike_aircraft);
    assert!(current.desired_screen > frozen.desired_screen);
    assert_eq!(planner.air_plan().unwrap(), &frozen);
}

#[test]
fn admitted_airborne_plan_does_not_shrink_after_economy_and_roster_losses() {
    let mut wealthy = wealthy_island_obs(5_016, 1);
    add_renewable_economy(&mut wealthy, 12);
    wealthy.my_units.extend((100..=159).map(|id| {
        own(
            id,
            UnitKind::Sentinel,
            TilePos::new(6 + i32::try_from(id % 8).unwrap(), 6),
        )
    }));
    wealthy.my_units.sort_unstable_by_key(|unit| unit.id);
    let identity = profile();
    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    let mut intel = knowledge(&wealthy);
    let mut planner = StrategicPlanner::new();
    planner.think_alone(&identity, tuning, &wealthy, &intel, HOME, &[]);
    let frozen = planner.air_plan().cloned().unwrap();

    let depleted = wealthy_island_obs(5_019, 1);
    let current = AirPlan::island(&identity, &depleted);
    assert!(current.observed_renewable < frozen.observed_renewable);
    assert!(current.observed_fighters < frozen.observed_fighters);
    assert!(current.desired_strike_aircraft < frozen.desired_strike_aircraft);
    assert!(current.desired_screen < frozen.desired_screen);

    intel.update(&depleted);
    planner.think_alone(&identity, tuning, &depleted, &intel, HOME, &[]);

    assert_eq!(planner.air_plan().unwrap(), &frozen);
}

#[test]
fn suppression_commitment_freezes_air_force_targets_despite_a_later_surge() {
    let mut battle = wealthy_island_obs(5_000, 1);
    let mut plan = AirPlan::island(&profile(), &battle);
    let bomber_kind = Role::Bomber.unit_for(battle.faction);
    let screen_kind = Role::AirGround.unit_for(battle.faction);
    let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
    for offset in 0..plan
        .desired_strike_aircraft
        .saturating_sub(operation.strike_aircraft.len())
    {
        let id = 100 + u32::try_from(offset).unwrap();
        battle
            .my_units
            .push(own(id, bomber_kind, TilePos::new(4, 6)));
        operation.strike_aircraft.push(UnitId(id));
    }
    for offset in 0..plan.desired_screen {
        let id = 200 + u32::try_from(offset).unwrap();
        battle
            .my_units
            .push(own(id, screen_kind, TilePos::new(5, 6)));
        plan.screen.push(UnitId(id));
    }
    battle.my_units.sort_unstable_by_key(|unit| unit.id);
    let frozen_strike_aircraft = plan.desired_strike_aircraft;
    let frozen_screen = plan.desired_screen;
    let frozen_renewable = plan.observed_renewable;
    let frozen_fighters = plan.observed_fighters;
    let frozen_timeout = plan.assembly_timeout;
    let mut planner = planner_with_operation(operation, plan);

    add_renewable_economy(&mut battle, 12);
    battle.my_units.extend((300..=379).map(|id| {
        own(
            id,
            UnitKind::Sentinel,
            TilePos::new(6 + i32::try_from(id % 8).unwrap(), 7),
        )
    }));
    battle.my_units.sort_unstable_by_key(|unit| unit.id);
    battle.tick += 1;
    let unconstrained = AirPlan::island(&profile(), &battle);
    assert!(unconstrained.desired_strike_aircraft > frozen_strike_aircraft);
    assert!(unconstrained.desired_screen > frozen_screen);
    let intel = knowledge(&battle);

    think(&mut planner, &battle, &intel);

    let committed = planner
        .air_plan()
        .expect("the committed operation retains its frozen plan");
    assert_eq!(committed.desired_strike_aircraft, frozen_strike_aircraft);
    assert_eq!(committed.desired_screen, frozen_screen);
    assert_eq!(committed.observed_renewable, frozen_renewable);
    assert_eq!(committed.observed_fighters, frozen_fighters);
    assert_eq!(committed.assembly_timeout, frozen_timeout);
}

#[test]
fn airborne_suppression_hits_visible_flak_then_waits_for_fresh_clearance() {
    let mut battle = wealthy_island_obs(5_000, 2);
    see_approach(&mut battle);
    battle.my_units[0].idle = false;
    battle
        .my_units
        .push(own(30, UnitKind::Buzzard, TilePos::new(5, 8)));
    battle
        .my_units
        .push(own(31, UnitKind::Buzzard, TilePos::new(5, 12)));
    battle.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(20, 10),
        true,
    ));
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.screen = vec![UnitId(30), UnitId(31)];
    let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
    operation.artillery.clear();
    let mut planner = planner_with_operation(operation, plan);
    let intel = knowledge(&battle);

    let suppression = think(&mut planner, &battle, &intel);
    assert!(suppression.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4), UnitId(30), UnitId(31)],
        target: Target::Building(BuildingId(81)),
    }));
    assert!(suppression.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));

    battle.tick += 1;
    let intel = knowledge(&battle);
    let repeated = think(&mut planner, &battle, &intel);
    assert!(
        repeated.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(81)),
                ..
            }
        )),
        "an unchanged suppression target must not reset bomber egress"
    );

    battle.tick += 1;
    battle
        .enemy_buildings
        .retain(|building| building.id != BuildingId(81));
    let intel = knowledge(&battle);
    let verification = think(&mut planner, &battle, &intel);
    assert_eq!(
        planner.air_operation().map(|operation| operation.phase()),
        Some(AirOperationPhase::Verify),
        "decision={verification:?}, cooldown={}",
        planner.cooldown_until
    );
    assert!(verification.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));
    assert!(verification.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(3), UnitId(4)],
        goal: landing_pad(&battle, HOME).unwrap(),
    }));
    assert!(verification.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(30), UnitId(31)],
        goal: HOME,
    }));

    battle
        .my_units
        .retain(|unit| !matches!(unit.id, UnitId(4) | UnitId(31)));
    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    battle.tick = battle
        .tick
        .saturating_add(tuning.reaction_delay)
        .saturating_add(tuning.commitment_hesitation)
        .saturating_add(1);
    let intel = knowledge(&battle);
    let strike = think(&mut planner, &battle, &intel);
    assert!(strike.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(30)],
        target: Target::Building(BuildingId(80)),
    }));
}

#[test]
fn airborne_corridor_actions_distinguish_current_remembered_and_absent_static_aa() {
    let flak_anchor = TilePos::new(12, 10);
    let planner_for = |observation: &Observation| {
        let mut operation = operation(AirOperationPhase::SuppressAa, observation.tick);
        operation.artillery.clear();
        let mut plan = AirPlan::island(&profile(), observation);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        plan.screen.clear();
        planner_with_operation(operation, plan)
    };
    let status = |planner: &StrategicPlanner,
                  observation: &Observation,
                  intelligence: &StrategicIntelligence| {
        airborne_corridor_status(
            &planner.air.as_ref().unwrap().op,
            planner.air_plan().unwrap(),
            observation,
            intelligence,
            HOME,
            &[],
        )
    };

    let mut current = wealthy_island_obs(5_000, 2);
    see_approach(&mut current);
    current
        .enemy_buildings
        .push(building(81, 1, BuildingKind::FlakTurret, flak_anchor, true));
    let mut intelligence = knowledge(&current);
    let mut current_planner = planner_for(&current);
    assert_eq!(
        status(&current_planner, &current, &intelligence),
        AirborneCorridorStatus::Defended
    );
    let current_decision = think(&mut current_planner, &current, &intelligence);
    assert_eq!(
        current_planner.air_operation().unwrap().phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(current_decision.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(81)),
    }));
    assert!(current_decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));

    let mut remembered = current.clone();
    remembered.tick += 1;
    remembered
        .enemy_buildings
        .iter_mut()
        .find(|building| building.id == BuildingId(81))
        .unwrap()
        .seen = false;
    let (flak_width, flak_height) = BuildingKind::FlakTurret.base_stats().size;
    for dy in 0..flak_height {
        for dx in 0..flak_width {
            let tile = flak_anchor.offset(dx, dy);
            let index = usize::try_from(tile.y * remembered.map_width + tile.x).unwrap();
            remembered.visible[index] = false;
        }
    }
    intelligence.update(&remembered);
    let mut remembered_planner = planner_for(&remembered);
    assert_eq!(
        status(&remembered_planner, &remembered, &intelligence),
        AirborneCorridorStatus::NeedsRecon
    );
    let remembered_decision = think(&mut remembered_planner, &remembered, &intelligence);
    assert_eq!(
        remembered_planner.air_operation().unwrap().phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(
        remembered_planner
            .air_operation()
            .unwrap()
            .scout_dispatch
            .is_some(),
        "remembered static AA must trigger a fog-honest look instead of a blind commitment"
    );
    assert!(remembered_decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));

    let mut absent = remembered.clone();
    absent.tick += 1;
    absent
        .enemy_buildings
        .retain(|building| building.id != BuildingId(81));
    for dy in 0..flak_height {
        for dx in 0..flak_width {
            let tile = flak_anchor.offset(dx, dy);
            let index = usize::try_from(tile.y * absent.map_width + tile.x).unwrap();
            absent.visible[index] = true;
            absent.explored[index] = true;
        }
    }
    intelligence.update(&absent);
    let mut absent_planner = planner_for(&absent);
    assert_eq!(
        status(&absent_planner, &absent, &intelligence),
        AirborneCorridorStatus::Clear
    );
    let absent_decision = think(&mut absent_planner, &absent, &intelligence);
    assert_eq!(
        absent_planner.air_operation().unwrap().phase(),
        AirOperationPhase::Verify,
        "only fresh negative evidence may advance the operation"
    );
    assert!(absent_decision.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(3), UnitId(4)],
        goal: landing_pad(&absent, HOME).unwrap(),
    }));
    assert!(absent_decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn ground_suppression_requires_fresh_negative_evidence_across_the_air_corridor() {
    let flak_anchor = TilePos::new(12, 5);
    let planner_for = |tick| {
        let observation = obs(tick);
        planner_with_operation(
            operation(AirOperationPhase::SuppressAa, tick),
            connected_test_plan(&observation),
        )
    };

    let mut current = obs(400);
    see_approach(&mut current);
    see_building_footprint(&mut current, TARGET, BuildingKind::Crucible);
    explore(&mut current, staging(HOME, TARGET));
    current
        .enemy_buildings
        .push(building(81, 1, BuildingKind::FlakTurret, flak_anchor, true));
    let mut intel = knowledge(&current);
    assert_eq!(
        targetable_corridor_flak(&intel, HOME, TARGET, &[]),
        Some(BuildingId(81)),
        "the off-line emplacement must cover the midflight corridor"
    );
    let mut current_planner = planner_for(current.tick);
    let current_decision = think(&mut current_planner, &current, &intel);
    assert_eq!(
        current_planner.air_operation().unwrap().phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(
        current_planner
            .air_operation()
            .unwrap()
            .scout_dispatch
            .is_some(),
        "AA outside artillery's target area must hold the wing for reconnaissance"
    );
    assert!(current_decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { units, .. } | Intent::AttackMoveUnits { units, .. }
            if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
    )));
    assert!(current_decision.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(3), UnitId(4)],
        goal: landing_pad(&current, HOME).unwrap(),
    }));

    let mut remembered = current.clone();
    remembered.tick += 1;
    remembered
        .enemy_buildings
        .iter_mut()
        .find(|building| building.id == BuildingId(81))
        .expect("the observed Flak remains as a ghost")
        .seen = false;
    let flak_index = usize::try_from(flak_anchor.y * remembered.map_width + flak_anchor.x).unwrap();
    remembered.visible[flak_index] = false;
    intel.update(&remembered);
    assert!(!corridor_clear(&intel, HOME, TARGET, &[]));
    let mut remembered_planner = planner_for(remembered.tick);
    let remembered_decision = think(&mut remembered_planner, &remembered, &intel);
    assert_eq!(
        remembered_planner.air_operation().unwrap().phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(
        remembered_planner
            .air_operation()
            .unwrap()
            .scout_dispatch
            .is_some(),
        "remembered corridor AA must be reconnoitered"
    );
    assert!(remembered_decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { units, .. } | Intent::AttackMoveUnits { units, .. }
            if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
    )));

    let mut absent = remembered.clone();
    absent.tick += 1;
    absent
        .enemy_buildings
        .retain(|building| building.id != BuildingId(81));
    absent.visible[flak_index] = true;
    absent.explored[flak_index] = true;
    intel.update(&absent);
    assert!(corridor_clear(&intel, HOME, TARGET, &[]));
    let mut absent_planner = planner_for(absent.tick);
    let absent_decision = think(&mut absent_planner, &absent, &intel);
    assert_eq!(
        absent_planner.air_operation().unwrap().phase(),
        AirOperationPhase::Verify,
        "fresh negative evidence may advance the combined operation"
    );
    assert!(absent_decision.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(3), UnitId(4)],
        goal: landing_pad(&absent, HOME).unwrap(),
    }));
}

#[test]
fn expired_static_aa_memory_does_not_override_current_mobile_corridor_evidence() {
    let flak_anchor = TilePos::new(12, 5);
    let (mut first, mut planner) =
        wealthy_airborne_operation(AirOperationPhase::SuppressAa, UnitKind::Sentinel, 1);
    first.enemy_units[0].tile = TilePos::new(12, 10);
    first
        .enemy_buildings
        .push(building(81, 1, BuildingKind::FlakTurret, flak_anchor, true));
    let mut intel = knowledge(&first);

    let mut stale = first;
    stale.tick += 4_000;
    stale
        .enemy_buildings
        .iter_mut()
        .find(|building| building.id == BuildingId(81))
        .expect("the static source remains represented as a ghost")
        .seen = false;
    let flak_index = usize::try_from(flak_anchor.y * stale.map_width + flak_anchor.x).unwrap();
    stale.visible[flak_index] = false;
    intel.update(&stale);
    let operation = planner
        .air_op_mut()
        .expect("the test operation remains active");
    operation.started_at = stale.tick - 50;
    operation.phase_started_at = stale.tick - 50;

    let assessment = intel.air_defense_at(TilePos::new(12, 10));
    assert!(assessment.sources.iter().any(|source| {
        matches!(source.source, AirDefenseSource::Unit { .. })
            && source.evidence == ContactEvidence::Current
    }));
    assert!(assessment.sources.iter().any(|source| {
        matches!(source.source, AirDefenseSource::Building { .. })
            && source.evidence == ContactEvidence::Remembered
            && source.confidence == 0
    }));
    assert_eq!(
        airborne_corridor_status(
            &planner.air.as_ref().unwrap().op,
            planner.air_plan().unwrap(),
            &stale,
            &intel,
            HOME,
            &[],
        ),
        AirborneCorridorStatus::Clear,
        "expired static memory must not turn a currently observed mobile threat into stale uncertainty"
    );

    let decision = think(&mut planner, &stale, &intel);
    assert_eq!(
        planner.air_operation().unwrap().phase(),
        AirOperationPhase::Verify
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(81)),
            ..
        }
    )));
}

#[test]
fn airborne_suppression_does_not_ignore_visible_midflight_flak() {
    let mut battle = wealthy_island_obs(5_000, 2);
    see_approach(&mut battle);
    battle.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(12, 10),
        true,
    ));
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
    operation.artillery.clear();
    let mut planner = planner_with_operation(operation, plan);
    let intel = knowledge(&battle);

    let decision = think(&mut planner, &battle, &intel);

    assert!(decision.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(81)),
    }));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));
}

#[test]
fn an_assembled_wealthy_wave_accepts_mobile_aa_but_still_suppresses_current_flak() {
    let (battle, mut planner) =
        wealthy_airborne_operation(AirOperationPhase::Assemble, UnitKind::Sentinel, 12);
    planner.air_op_mut().unwrap().strike_aircraft.clear();
    planner.air_plan_mut().unwrap().screen.clear();
    let intel = knowledge(&battle);

    let assembly = think(&mut planner, &battle, &intel);
    let frozen = planner.air_operation().unwrap();
    assert_eq!(frozen.phase(), AirOperationPhase::SuppressAa);
    assert_eq!(frozen.strike_aircraft.len(), 10);
    assert_eq!(planner.air_plan().unwrap().screen.len(), 5);
    let expected: Vec<_> = std::iter::once(UnitId(1))
        .chain((100..110).chain(200..205).map(UnitId))
        .collect();
    assert_eq!(assembly.reservations, expected);
    let scout_goal = frozen
        .scout_dispatch
        .expect("assembly dispatches the operation scout")
        .1;

    let mut battle = battle;
    let scout = battle
        .my_units
        .iter_mut()
        .find(|unit| unit.id == UnitId(1))
        .unwrap();
    scout.tile = scout_goal;
    scout.idle = false;
    battle.tick += 1;
    let intel = knowledge(&battle);
    let verification = think(&mut planner, &battle, &intel);
    assert_eq!(
        planner.air_operation().map(|operation| operation.phase()),
        Some(AirOperationPhase::Verify),
        "weak mobile AA should not cancel the frozen 10-bomber/5-screen wave: {verification:?}; {:?}",
        planner.air_operation()
    );

    battle.tick += 1;
    let intel = knowledge(&battle);
    let strike = think(&mut planner, &battle, &intel);
    let expected: Vec<_> = (100..110).chain(200..205).map(UnitId).collect();
    assert_eq!(
        planner.air_operation().map(|operation| operation.phase()),
        Some(AirOperationPhase::Strike)
    );
    assert!(strike.intents.contains(&Intent::AttackUnits {
        units: expected,
        target: Target::Building(BuildingId(80)),
    }));

    let (mut defended, mut flak_planner) =
        wealthy_airborne_operation(AirOperationPhase::Assemble, UnitKind::Sentinel, 12);
    flak_planner.air_op_mut().unwrap().strike_aircraft.clear();
    flak_planner.air_plan_mut().unwrap().screen.clear();
    defended.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TARGET.offset(-4, 0),
        true,
    ));
    let intel = knowledge(&defended);
    let assembly = think(&mut flak_planner, &defended, &intel);
    assert_eq!(
        flak_planner.air_operation().unwrap().phase(),
        AirOperationPhase::SuppressAa
    );
    let expected_reservations: Vec<_> = std::iter::once(UnitId(1))
        .chain((100..110).chain(200..205).map(UnitId))
        .collect();
    assert_eq!(assembly.reservations, expected_reservations);
    let scout_goal = flak_planner
        .air_operation()
        .unwrap()
        .scout_dispatch
        .expect("assembly dispatches the operation scout")
        .1;
    let scout = defended
        .my_units
        .iter_mut()
        .find(|unit| unit.id == UnitId(1))
        .unwrap();
    scout.tile = scout_goal;
    scout.idle = false;

    defended.tick += 1;
    let intel = knowledge(&defended);
    let suppression = think(&mut flak_planner, &defended, &intel);
    assert_eq!(
        flak_planner.air_operation().unwrap().phase(),
        AirOperationPhase::SuppressAa,
        "current Flak remains a hard gate even for the full wealthy wing"
    );
    assert!(suppression.intents.contains(&Intent::AttackUnits {
        units: (100..110).chain(200..205).map(UnitId).collect(),
        target: Target::Building(BuildingId(81)),
    }));
    assert!(suppression.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));
}

#[test]
fn a_wealthy_airborne_wave_rejects_overwhelming_mobile_aa_before_and_after_suppression() {
    for phase in [
        AirOperationPhase::SuppressAa,
        AirOperationPhase::Verify,
        AirOperationPhase::Strike,
    ] {
        let (battle, mut planner) = wealthy_airborne_operation(phase, UnitKind::Flakhound, 7);
        let intel = knowledge(&battle);

        let decision = think(&mut planner, &battle, &intel);
        let operation = planner
            .air_operation()
            .expect("recovery remains observable for one think");

        assert_eq!(operation.phase(), AirOperationPhase::Recover, "{phase:?}");
        assert_eq!(
            operation.recovery_reason(),
            Some(AirRecoveryReason::NewAirDefense),
            "{phase:?}"
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }
}

#[test]
fn shared_air_support_clears_and_reconnoiters_the_frozen_drop_envelope_before_release() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = wealthy_island_obs(5_000, 1);
    see_approach(&mut battle);
    battle.my_units[0].tile = HOME;
    let drop = TilePos::new(24, 15);
    let flak = TilePos::new(24, 19);
    battle
        .enemy_buildings
        .push(building(81, 1, BuildingKind::FlakTurret, flak, true));
    let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
    operation.artillery.clear();
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    plan.screen.clear();
    let mut planner = planner_with_operation(operation, plan);
    let request = LiftSupportRequest {
        player: PlayerId(1),
        target: TARGET,
        planned_drops: vec![drop],
    };
    let mut intel = knowledge(&battle);
    assert_eq!(
        targetable_corridor_flak(&intel, HOME, TARGET, &[]),
        None,
        "the flak does not cover the bomber objective's direct corridor"
    );
    assert_eq!(
        targetable_corridor_flak(&intel, HOME, TARGET, &[drop]),
        Some(BuildingId(81)),
        "the same flak does cover the transport's actual drop envelope"
    );

    let suppression = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination(&fixture_planning, Some(&request)),
        )
        .decision;
    assert_eq!(
        planner.air_operation().unwrap().phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(suppression.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(81)),
    }));
    let scout_goal = planner.air_operation().unwrap().scout_dispatch.unwrap().1;
    assert!(suppression.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(1)],
        goal: scout_goal,
    }));
    assert!(suppression.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));

    battle.my_units[0].tile = scout_goal;
    battle.my_units[0].idle = false;
    battle.tick += 1;
    battle
        .enemy_buildings
        .retain(|building| building.id != BuildingId(81));
    intel.update(&battle);
    let searching = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination(&fixture_planning, Some(&request)),
        )
        .decision;
    assert_eq!(
        planner.air_operation().unwrap().phase(),
        AirOperationPhase::SuppressAa,
        "destroying flak in fog is not enough to release the transports"
    );
    assert_eq!(
        planner.air_operation().unwrap().scout_dispatch,
        Some((UnitId(1), scout_goal)),
        "the original scouting order remains authoritative while the drop is dark"
    );
    assert!(searching.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));

    see_approach_to(&mut battle, drop);
    let flak_index = usize::try_from(flak.y * battle.map_width + flak.x).unwrap();
    battle.visible[flak_index] = true;
    battle.explored[flak_index] = true;
    battle.tick += 1;
    intel.update(&battle);
    planner.think_alone_with(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &battle,
        &intel,
        HOME,
        coordination(&fixture_planning, Some(&request)),
    );
    assert_eq!(
        planner.air_operation().unwrap().phase(),
        AirOperationPhase::Verify,
        "current sight must clear both the objective and frozen drop approach"
    );

    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    battle.tick = battle
        .tick
        .saturating_add(tuning.reaction_delay)
        .saturating_add(tuning.commitment_hesitation);
    intel.update(&battle);
    let released = planner
        .think_alone_with(
            &profile(),
            tuning,
            &battle,
            &intel,
            HOME,
            coordination(&fixture_planning, Some(&request)),
        )
        .decision;
    assert_eq!(
        planner.air_operation().unwrap().phase(),
        AirOperationPhase::Strike
    );
    assert!(released.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(80)),
    }));
}

#[test]
fn island_bombers_can_strike_without_a_screen_or_transport() {
    let mut battle = wealthy_island_obs(5_000, 1);
    see_approach(&mut battle);
    assert!(
        battle
            .my_units
            .iter()
            .all(|unit| unit.kind != UnitKind::Skyhook)
    );
    let mut operation = operation(AirOperationPhase::Strike, battle.tick);
    operation.artillery.clear();
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    plan.screen.clear();
    let mut planner = planner_with_operation(operation, plan);
    let intel = knowledge(&battle);

    let decision = think(&mut planner, &battle, &intel);

    assert!(decision.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(80)),
    }));
}

#[test]
fn an_active_bomber_strike_is_not_reissued_but_an_idle_member_rejoins() {
    let mut battle = wealthy_island_obs(5_000, 1);
    see_approach(&mut battle);
    let mut operation = operation(AirOperationPhase::Strike, battle.tick);
    operation.artillery.clear();
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    plan.screen.clear();
    let mut planner = planner_with_operation(operation, plan);

    let first = think(&mut planner, &battle, &knowledge(&battle));
    assert!(first.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(80)),
    }));

    planner = crate::checkpoint::round_trip(&planner);
    for bomber in battle
        .my_units
        .iter_mut()
        .filter(|unit| matches!(unit.id, UnitId(3) | UnitId(4)))
    {
        bomber.idle = false;
    }
    battle.tick += 6;
    let in_flight = think(&mut planner, &battle, &knowledge(&battle));
    assert!(
        in_flight.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )),
        "the authoritative in-flight attack must not be restaged every think"
    );

    battle
        .my_units
        .iter_mut()
        .find(|unit| unit.id == UnitId(3))
        .unwrap()
        .idle = true;
    battle.tick += 6;
    let retry = think(&mut planner, &battle, &knowledge(&battle));
    assert!(retry.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3)],
        target: Target::Building(BuildingId(80)),
    }));
}

#[test]
fn a_strike_keeps_its_exact_attack_until_current_sight_loses_the_target() {
    let mut battle = wealthy_island_obs(5_000, 1);
    see_approach(&mut battle);
    let mut operation = operation(AirOperationPhase::Strike, battle.tick);
    operation.artillery.clear();
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    plan.screen.clear();
    let mut planner = planner_with_operation(operation, plan);
    let mut intel = knowledge(&battle);

    let first = think(&mut planner, &battle, &intel);
    assert!(first.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(80)),
    }));
    assert!(
        first
            .intents
            .iter()
            .all(|intent| !matches!(intent, Intent::AttackMoveUnits { .. }))
    );

    battle.tick += 1;
    intel.update(&battle);
    let still_current = think(&mut planner, &battle, &intel);
    assert!(still_current.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(80)),
    }));
    assert!(
        still_current
            .intents
            .iter()
            .all(|intent| !matches!(intent, Intent::AttackMoveUnits { .. }))
    );

    battle.enemy_buildings.clear();
    battle.tick += 1;
    intel.update(&battle);
    let target_lost = think(&mut planner, &battle, &intel);
    assert!(target_lost.intents.contains(&Intent::AttackMoveUnits {
        units: vec![UnitId(3), UnitId(4)],
        goal: TARGET,
    }));
    assert!(target_lost.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));
}

#[test]
fn an_opportunistic_strike_does_not_move_the_shared_coordination_anchor() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = wealthy_island_obs(5_000, 1);
    see_approach(&mut battle);
    battle.enemy_buildings = vec![building(
        81,
        1,
        BuildingKind::Airworks,
        TARGET.offset(-3, 0),
        true,
    )];
    let mut operation = operation(AirOperationPhase::Strike, battle.tick);
    operation.artillery.clear();
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    plan.screen.clear();
    let mut planner = planner_with_operation(operation, plan);
    let request = LiftSupportRequest {
        player: PlayerId(1),
        target: TARGET,
        planned_drops: Vec::new(),
    };
    let intel = knowledge(&battle);

    let decision = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination(&fixture_planning, Some(&request)),
        )
        .decision;

    assert!(decision.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(81)),
    }));
    let operation = planner.air_operation().unwrap();
    assert_eq!(operation.target_player, request.player);
    assert_eq!(operation.target, request.target);
    assert_eq!(operation.target_id, Some(BuildingId(80)));
    assert_eq!(operation.target_kind, BuildingKind::Crucible);
}

#[test]
fn an_exact_strike_validates_the_selected_cluster_target_not_the_operation_anchor() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut battle = obs(5_000);
    see_approach(&mut battle);
    let secondary = TARGET.offset(3, 0);
    see_approach_to(&mut battle, secondary);
    see_building_footprint(&mut battle, secondary, BuildingKind::Airworks);
    battle.explored.fill(true);
    battle.enemy_buildings = vec![building(81, 1, BuildingKind::Airworks, secondary, true)];
    let public_map = public_map_with_terrain(
        &battle,
        (0..battle.map_height).map(|y| (TilePos::new(26, y), Terrain::Peak)),
    );
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Strike, battle.tick);
    let active = planner.air.as_mut().expect("active operation");
    active
        .plan
        .connected_package
        .as_mut()
        .expect("connected package")
        .target_anchors = vec![TARGET, secondary];
    assert_eq!(
        live_strike_target(&active.op, &active.plan, &intel).map(|target| target.anchor),
        Some(secondary)
    );
    let mut coordination = coordination(&fixture_planning, None);
    coordination.public_map = Some(&public_map);

    let decision = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination,
        )
        .decision;

    let operation = planner
        .air_operation()
        .expect("recovery remains observable");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(81)),
            ..
        }
    )));
}

#[test]
fn exact_attacks_do_not_require_attack_move_spread_slots() {
    let mut battle = obs(5_000);
    battle.my_units = (0..4)
        .map(|index| {
            own(
                10 + index,
                UnitKind::Condor,
                TilePos::new(4 + i32::try_from(index).unwrap(), TARGET.y),
            )
        })
        .collect();
    let public_map = public_map_with_terrain(
        &battle,
        [TARGET.y - 1, TARGET.y + 1]
            .into_iter()
            .flat_map(|y| (0..battle.map_width).map(move |x| (TilePos::new(x, y), Terrain::Peak))),
    );
    let attackers: Vec<_> = battle.my_units.iter().map(|unit| unit.id).collect();
    let mut exact = route_projection_with_orientation(
        &battle,
        Domain::Air,
        Some(&public_map),
        test_orientation(),
    );
    assert!(exact_attack_group_reaches(
        &mut exact, &battle, &attackers, TARGET
    ));

    let spread = route_projection_with_orientation(
        &battle,
        Domain::Air,
        Some(&public_map),
        test_orientation(),
    );
    assert!(!spread.group_reaches_command_goal(&attackers, TARGET));
}

#[test]
fn surviving_screen_cannot_hide_the_loss_of_an_airborne_bomber_force() {
    let mut battle = wealthy_island_obs(5_000, 2);
    battle
        .my_units
        .retain(|unit| !matches!(unit.id, UnitId(3) | UnitId(4)));
    battle
        .my_units
        .push(own(30, UnitKind::Buzzard, TilePos::new(5, 8)));
    battle
        .my_units
        .push(own(31, UnitKind::Buzzard, TilePos::new(5, 12)));
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.screen = vec![UnitId(30), UnitId(31)];
    let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
    operation.artillery.clear();
    let mut planner = planner_with_operation(operation, plan);
    let intel = knowledge(&battle);

    let decision = think(&mut planner, &battle, &intel);

    let operation = planner.air_operation().expect("recovery remains visible");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::RequiredUnitLost)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { units, .. } | Intent::AttackMoveUnits { units, .. }
            if units.contains(&UnitId(30)) || units.contains(&UnitId(31))
    )));
}

#[test]
fn an_air_identity_waits_for_shared_legal_producers_before_committing() {
    let mut observation = obs(120);
    observation.my_units.retain(|unit| unit.id == UnitId(1));
    observation
        .my_units
        .extend((100..=111).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
    observation.my_units.sort_unstable_by_key(|unit| unit.id);
    assert_eq!(
        combat_roster(&observation),
        CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER
    );
    let intel = knowledge(&observation);
    let mut planner = StrategicPlanner::new();

    planner.think_alone(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intel,
        HOME,
        &[],
    );
    assert!(planner.air_operation().is_none());

    observation.visible.fill(true);
    observation.explored.fill(true);
    observation.scrap = 10_000;
    observation.my_buildings = vec![
        building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
        building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
        building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
    ];
    observation.my_queues = vec![Vec::new(); 3];
    let intel = knowledge(&observation);
    planner.think_alone(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Prime),
        &observation,
        &intel,
        HOME,
        &[],
    );
    assert!(planner.air_operation().is_some());
}

#[test]
fn stale_intelligence_preserves_standby_without_issuing_commands() {
    let observation = obs(101);
    let stale_intel = knowledge(&obs(100));
    let mut planner = StrategicPlanner {
        standby: AirStandby {
            scout: Some(UnitId(1)),
            artillery: vec![UnitId(2)],
            strike_aircraft: vec![UnitId(3), UnitId(4)],
        },
        ..StrategicPlanner::new()
    };
    let before = planner.clone();

    let decision = think(&mut planner, &observation, &stale_intel);

    assert!(decision.intents.is_empty());
    assert_eq!(
        decision.reservations,
        [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
    );
    assert_eq!(planner, before, "stale knowledge must not advance a plan");
}

#[test]
fn airborne_assembly_fails_closed_when_known_peaks_seal_scout_ingress() {
    let mut battle = wealthy_island_obs(5_000, 1);
    battle.my_units[0].tile = TilePos::new(4, 10);
    battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    let mut operation = operation(AirOperationPhase::Assemble, battle.tick);
    operation.artillery.clear();
    let mut planner = planner_with_operation(operation, plan);
    let intel = knowledge(&battle);

    let decision = think(&mut planner, &battle, &intel);

    let operation = planner
        .air_operation()
        .expect("recovery remains observable");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn a_new_airborne_operation_releases_its_roster_when_scout_ingress_is_sealed() {
    let mut battle = wealthy_island_obs(5_016, 1);
    battle.my_units[0].tile = HOME;
    battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
    let mut intel = knowledge(&battle);
    let mut planner = StrategicPlanner::new();

    let refused = think(&mut planner, &battle, &intel);

    let operation = planner
        .air_operation()
        .expect("the failed ingress remains observable through recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(refused.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));

    battle.tick += 1;
    intel.update(&battle);
    think(&mut planner, &battle, &intel);
    assert!(planner.air_operation().is_none());

    battle.tick += 1;
    intel.update(&battle);
    let cooldown = think(&mut planner, &battle, &intel);
    assert!(planner.air_operation().is_none());
    assert!(cooldown.reservations.is_empty());
    assert!(cooldown.intents.is_empty());
}

#[test]
fn airborne_assembly_fills_an_undispatched_scout_slot_then_launches_the_complete_wave() {
    let mut battle = wealthy_island_obs(5_000, 1);
    battle.my_units[0].tile = HOME;
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    let mut operation = operation(AirOperationPhase::Assemble, battle.tick);
    operation.scout = Some(UnitId(99));
    operation.scout_dispatch = None;
    operation.artillery.clear();
    let mut planner = planner_with_operation(operation, plan);
    let intel = knowledge(&battle);

    let decision = think(&mut planner, &battle, &intel);

    let operation = planner.air_operation().expect("operation continues");
    assert_eq!(operation.phase(), AirOperationPhase::SuppressAa);
    assert_eq!(operation.scout, Some(UnitId(1)));
    assert!(decision.intents.iter().any(|intent| matches!(
        intent,
        Intent::MoveUnits { units, goal }
            if units == &[UnitId(1)] && *goal != TARGET
    )));
    assert!(decision.intents.contains(&Intent::MoveUnits {
        units: vec![UnitId(3), UnitId(4)],
        goal: landing_pad(&battle, HOME).unwrap(),
    }));
    assert_eq!(
        planner.remaining_airwork_ticks(&battle, None),
        0,
        "a launched operation no longer reserves speculative Airworks capacity"
    );
}

#[test]
fn losing_a_dispatched_scout_during_assembly_recovers_without_replacement() {
    let mut battle = wealthy_island_obs(5_000, 1);
    battle.my_units[0].tile = HOME;
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    let mut operation = operation(AirOperationPhase::Assemble, battle.tick);
    operation.scout = Some(UnitId(99));
    operation.scout_dispatch = Some((UnitId(99), TARGET));
    operation.artillery.clear();
    let mut planner = planner_with_operation(operation, plan);
    let intel = knowledge(&battle);

    let decision = think(&mut planner, &battle, &intel);

    let operation = planner
        .air_operation()
        .expect("the failed assembly remains observable during recovery");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::RequiredUnitLost)
    );
    assert_eq!(operation.scout_dispatch, None);
    assert!(planner.cooldown_until > battle.tick);
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::MoveUnits { units, .. } if units.contains(&UnitId(1))
    )));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Kestrel,
            ..
        }
    )));
}

#[test]
fn losing_the_scout_after_assembly_aborts_before_any_bomber_commitment() {
    let mut battle = obs(300);
    battle.my_units.retain(|unit| unit.id != UnitId(1));
    let intel = knowledge(&battle);

    for phase in [
        AirOperationPhase::SuppressAa,
        AirOperationPhase::Verify,
        AirOperationPhase::Strike,
    ] {
        let mut planner = with_operation(phase, battle.tick);
        let decision = think(&mut planner, &battle, &intel);
        let operation = planner
            .air_operation()
            .expect("the failed operation remains observable during recovery");

        assert_eq!(operation.phase(), AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason(),
            Some(AirRecoveryReason::RequiredUnitLost)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }
}

#[test]
fn visible_ground_mobile_aa_is_suppressed_before_bombers_commit() {
    let mut battle = obs(300);
    let mut flakhound = own(90, UnitKind::Flakhound, TARGET.offset(-3, 0));
    flakhound.player = PlayerId(1);
    battle.enemy_units.push(flakhound);
    let intel = knowledge(&battle);

    for phase in [AirOperationPhase::SuppressAa, AirOperationPhase::Verify] {
        let mut planner = with_operation(phase, battle.tick);
        let decision = think(&mut planner, &battle, &intel);
        let operation = planner
            .air_operation()
            .expect("suppression retains the operation");

        assert_eq!(operation.phase(), AirOperationPhase::SuppressAa);
        assert_eq!(operation.recovery_reason(), None);
        let firing_stand = decision
            .intents
            .iter()
            .find_map(|intent| match intent {
                Intent::MoveUnits { units, goal } if units == &[UnitId(2)] => Some(*goal),
                _ => None,
            })
            .expect("artillery positions before attacking mobile AA");
        let routes = route_projection(&battle, Domain::Ground, None);
        assert!(
            suppression_firing_stands(
                &routes,
                &battle,
                SuppressionOrigin {
                    tile: battle.my_units[1].tile,
                    kind: UnitKind::Bombard,
                },
                Target::Unit(UnitId(90)),
                &intel,
                None,
            )
            .any(|stand| stand == firing_stand)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent, Intent::AttackUnits { units, .. } if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
        )));
    }
}

#[test]
fn visible_landed_air_aa_is_suppressed_as_a_ground_target() {
    let mut battle = obs(300);
    let mut talon = own(90, UnitKind::Talon, TARGET.offset(-3, 0));
    talon.player = PlayerId(1);
    talon.grounded = true;
    battle.enemy_units.push(talon);
    let intel = knowledge(&battle);

    for phase in [AirOperationPhase::SuppressAa, AirOperationPhase::Verify] {
        let mut planner = with_operation(phase, battle.tick);
        let decision = think(&mut planner, &battle, &intel);
        let operation = planner
            .air_operation()
            .expect("suppression retains the operation");

        assert_eq!(operation.phase(), AirOperationPhase::SuppressAa);
        assert_eq!(operation.recovery_reason(), None);
        let firing_stand = decision
            .intents
            .iter()
            .find_map(|intent| match intent {
                Intent::MoveUnits { units, goal } if units == &[UnitId(2)] => Some(*goal),
                _ => None,
            })
            .expect("artillery positions before attacking landed AA");
        let routes = route_projection(&battle, Domain::Ground, None);
        assert!(
            suppression_firing_stands(
                &routes,
                &battle,
                SuppressionOrigin {
                    tile: battle.my_units[1].tile,
                    kind: UnitKind::Bombard,
                },
                Target::Unit(UnitId(90)),
                &intel,
                None,
            )
            .any(|stand| stand == firing_stand)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent, Intent::AttackUnits { units, .. } if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
        )));
    }
}

#[test]
fn connected_operation_recovers_from_airborne_aa_that_artillery_cannot_suppress() {
    let mut battle = obs(300);
    let mut talon = own(90, UnitKind::Talon, TARGET.offset(-3, 0));
    talon.player = PlayerId(1);
    battle.enemy_units.push(talon);
    let intel = knowledge(&battle);

    for phase in [
        AirOperationPhase::SuppressAa,
        AirOperationPhase::Verify,
        AirOperationPhase::Strike,
    ] {
        let mut planner = with_operation(phase, battle.tick);
        let decision = think(&mut planner, &battle, &intel);
        let operation = planner
            .air_operation()
            .expect("recovery remains observable");

        assert_eq!(operation.phase(), AirOperationPhase::Recover, "{phase:?}");
        assert_eq!(
            operation.recovery_reason(),
            Some(AirRecoveryReason::NewAirDefense),
            "{phase:?}"
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }
}

#[test]
fn connected_phases_suppress_aa_covering_a_secondary_cluster_target() {
    let primary = TilePos::new(20, 20);
    let secondary = TilePos::new(24, 20);
    let flak = TilePos::new(29, 20);
    let mut battle = obs(400);
    battle.map_width = 40;
    battle.map_height = 30;
    battle.visible = vec![true; 40 * 30];
    battle.explored = vec![true; 40 * 30];
    battle.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, primary, true),
        building(82, 1, BuildingKind::Airworks, secondary, true),
        building(81, 1, BuildingKind::FlakTurret, flak, true),
    ];
    let intel = knowledge(&battle);
    assert_eq!(targetable_flak(&intel.air_defense_at(primary)), None);
    assert_eq!(
        targetable_flak(&intel.air_defense_at(secondary)),
        Some(BuildingId(81))
    );

    for phase in [
        AirOperationPhase::SuppressAa,
        AirOperationPhase::Verify,
        AirOperationPhase::Strike,
    ] {
        let mut planner = with_operation(phase, battle.tick);
        let active = planner.air.as_mut().expect("active operation");
        active.op.target = primary;
        active.op.target_id = Some(BuildingId(80));
        active
            .plan
            .connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = vec![primary, secondary];
        let decision = think(&mut planner, &battle, &intel);
        let operation = planner
            .air_operation()
            .expect("suppression retains the operation");

        assert_eq!(
            operation.phase(),
            AirOperationPhase::SuppressAa,
            "{phase:?}"
        );
        assert_eq!(operation.recovery_reason(), None, "{phase:?}");
        let firing_stand = decision
            .intents
            .iter()
            .find_map(|intent| match intent {
                Intent::MoveUnits { units, goal } if units == &[UnitId(2)] => Some(*goal),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{phase:?}: {decision:?}"));
        let routes = route_projection(&battle, Domain::Ground, None);
        assert!(
            suppression_firing_stands(
                &routes,
                &battle,
                SuppressionOrigin {
                    tile: battle.my_units[1].tile,
                    kind: UnitKind::Bombard,
                },
                Target::Building(BuildingId(81)),
                &intel,
                None,
            )
            .any(|stand| stand == firing_stand)
        );
        assert!(
            decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits {
                    units,
                    target: Target::Building(BuildingId(80) | BuildingId(82)),
                } if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
            )),
            "{phase:?}: {decision:?}"
        );
    }
}

#[test]
fn connected_selection_excludes_secondary_aa_sealed_by_peaks() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let primary = TilePos::new(20, 20);
    let secondary = TilePos::new(24, 20);
    let flak = TilePos::new(29, 20);
    let mut battle = obs(400);
    battle.map_width = 40;
    battle.map_height = 30;
    battle.visible = vec![true; 40 * 30];
    battle.explored = vec![true; 40 * 30];
    battle.enemy_buildings = vec![
        building(80, 1, BuildingKind::Crucible, primary, true),
        building(82, 1, BuildingKind::Airworks, secondary, true),
        building(81, 1, BuildingKind::FlakTurret, flak, true),
    ];
    let public_map = public_map_with_terrain(
        &battle,
        oxide_sim::geometry::rect_adjacent_tiles(flak, BuildingKind::FlakTurret.base_stats().size)
            .map(|tile| (tile, Terrain::Peak)),
    );
    let intel = knowledge(&battle);
    let target = intel
        .buildings()
        .iter()
        .find(|contact| contact.anchor == primary)
        .expect("current primary target");
    let selection = connected_target_selection(
        &battle,
        target,
        &[],
        ConnectedRouteContext {
            campaign_routes: None,
            unavailable_paid: &[],
            intel: &intel,
            home: HOME,
            target: primary,
            public_map: Some(&public_map),
            orientation: test_orientation(),
        },
    );
    assert_eq!(selection.target_anchors, vec![primary]);
    let mut planner = with_operation(AirOperationPhase::SuppressAa, battle.tick);
    let active = planner.air.as_mut().expect("active operation");
    active.op.target = primary;
    active.op.target_id = Some(BuildingId(80));
    active
        .plan
        .connected_package
        .as_mut()
        .expect("connected package")
        .target_anchors = selection.target_anchors;
    let mut coordination = coordination(&fixture_planning, None);
    coordination.public_map = Some(&public_map);

    let decision = planner
        .think_alone_with(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination,
        )
        .decision;

    let operation = planner.air_operation().expect("operation remains active");
    assert_ne!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(operation.recovery_reason(), None);
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(81)),
            ..
        }
    )));
}

#[test]
fn suppression_cancels_its_attack_when_scout_ingress_is_impossible() {
    let mut battle = obs(300);
    battle.my_units[0].tile = TilePos::new(4, 10);
    battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
    battle.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TARGET.offset(-4, 0),
        true,
    ));
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::SuppressAa, battle.tick);

    let decision = think(&mut planner, &battle, &intel);

    let operation = planner
        .air_operation()
        .expect("recovery remains observable");
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(
        decision
            .intents
            .iter()
            .all(|intent| !matches!(intent, Intent::AttackUnits { .. })),
        "artillery must not fire after the spotter route is disproved"
    );
}

#[test]
fn suppression_without_flak_still_fails_closed_when_scout_ingress_is_sealed() {
    for target_currently_visible in [false, true] {
        let mut battle = obs(300);
        battle.my_units[0].tile = TilePos::new(4, 10);
        battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
        if target_currently_visible {
            see_approach(&mut battle);
        }
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::SuppressAa, battle.tick);

        let decision = think(&mut planner, &battle, &intel);
        let operation = planner
            .air_operation()
            .expect("the failed operation remains observable during recovery");

        assert_eq!(operation.phase(), AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason(),
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if units.iter().any(|id| matches!(id, UnitId(3) | UnitId(4)))
                    && *goal != HOME
        )));
    }
}

#[test]
fn airborne_suppression_aborts_from_clear_and_uncertain_corridors_when_recon_is_sealed() {
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_045,
    ));
    assert_eq!(identity.primary, Specialty::Air);

    for approach_visible in [false, true] {
        let mut battle = wealthy_island_obs(5_000, 1);
        battle.my_units[0].tile = HOME;
        battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
        if approach_visible {
            see_approach(&mut battle);
        }
        let intel = knowledge(&battle);
        let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
        operation.artillery.clear();
        let mut plan = AirPlan::island(&identity, &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        plan.screen.clear();
        let mut planner = planner_with_operation(operation, plan);

        let decision = planner.think_alone(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            &[],
        );
        let operation = planner
            .air_operation()
            .expect("the failed suppression remains observable during recovery");

        assert_eq!(operation.phase(), AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason(),
            Some(AirRecoveryReason::UnreachableAirRoute),
            "approach_visible={approach_visible}"
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if units.iter().any(|id| matches!(id, UnitId(3) | UnitId(4)))
                    && *goal != HOME
        )));
    }
}

#[test]
fn airborne_verification_never_commits_while_its_recon_route_is_sealed() {
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Standard,
        BotStance::Balanced,
        20_045,
    ));
    assert_eq!(identity.primary, Specialty::Air);
    let tuning = DifficultyTuning::for_level(BotDifficulty::Standard);

    for approach_visible in [false, true] {
        let mut battle = wealthy_island_obs(5_000, 1);
        battle.my_units[0].tile = HOME;
        battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
        if approach_visible {
            see_approach(&mut battle);
        }
        let intel = knowledge(&battle);
        let mut operation = operation(AirOperationPhase::Verify, battle.tick);
        operation.artillery.clear();
        let mut plan = AirPlan::island(&identity, &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        plan.screen.clear();
        let mut planner = planner_with_operation(operation, plan);

        let decision = planner.think_alone(&identity, tuning, &battle, &intel, HOME, &[]);
        let operation = planner
            .air_operation()
            .expect("the refused verification remains observable during recovery");

        assert_eq!(operation.phase(), AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason(),
            Some(AirRecoveryReason::UnreachableAirRoute),
            "approach_visible={approach_visible}"
        );
        assert_eq!(operation.strike_issued_at, None);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }
}

#[test]
fn suppression_observes_reaction_delay_before_firing_on_new_flak() {
    let mut battle = obs(300);
    battle.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TARGET.offset(-4, 0),
        true,
    ));
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::SuppressAa, battle.tick);
    planner.air_op_mut().unwrap().phase_started_at = battle.tick;

    let decision = planner.think_alone(
        &profile(),
        DifficultyTuning::for_level(BotDifficulty::Standard),
        &battle,
        &intel,
        HOME,
        &[],
    );

    assert_eq!(
        planner.air_operation().expect("operation waits").phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(81)),
            ..
        }
    )));
}

#[test]
fn verification_reenters_suppression_when_flak_returns() {
    let mut battle = obs(300);
    battle.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TARGET.offset(-4, 0),
        true,
    ));
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Verify, battle.tick);

    let decision = think(&mut planner, &battle, &intel);

    assert_eq!(
        planner
            .air_operation()
            .expect("operation continues")
            .phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(decision.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(2)],
        target: Target::Building(BuildingId(81)),
    }));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));
}

#[test]
fn verification_clears_held_orders_when_scout_ingress_becomes_impossible() {
    let seen = obs(299);
    let mut intel = knowledge(&seen);
    let mut battle = obs(300);
    battle.my_units[0].tile = TilePos::new(4, 10);
    battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
    battle.enemy_buildings[0].seen = false;
    intel.update(&battle);
    let mut planner = with_operation(AirOperationPhase::Verify, battle.tick);

    let decision = think(&mut planner, &battle, &intel);

    let operation = planner
        .air_operation()
        .expect("recovery remains observable");
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableAirRoute)
    );
    assert!(
        decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if units.iter().any(|id| matches!(id, UnitId(3) | UnitId(4)))
                    && *goal != HOME
        )),
        "a failed reconnaissance route must not leave a strike hold behind"
    );
}

#[test]
fn an_airborne_strike_returns_to_suppression_when_corridor_flak_appears() {
    let mut battle = wealthy_island_obs(5_000, 1);
    see_approach(&mut battle);
    battle.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(12, 10),
        true,
    ));
    let mut operation = operation(AirOperationPhase::Strike, battle.tick);
    operation.artillery.clear();
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    let mut planner = planner_with_operation(operation, plan);
    let intel = knowledge(&battle);

    let decision = think(&mut planner, &battle, &intel);

    assert_eq!(
        planner
            .air_operation()
            .expect("operation continues")
            .phase(),
        AirOperationPhase::SuppressAa
    );
    assert!(decision.intents.contains(&Intent::AttackUnits {
        units: vec![UnitId(3), UnitId(4)],
        target: Target::Building(BuildingId(81)),
    }));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits {
            target: Target::Building(BuildingId(80)),
            ..
        }
    )));
}

#[test]
fn destroyed_target_completes_the_strike_and_releases_a_waiting_lift() {
    let mut battle = wealthy_island_obs(5_000, 1);
    battle.enemy_buildings.clear();
    see_approach(&mut battle);
    let mut operation = operation(AirOperationPhase::Strike, battle.tick);
    operation.artillery.clear();
    operation.strike_issued_at = Some(battle.tick - 20);
    let mut plan = AirPlan::island(&profile(), &battle);
    plan.desired_strike_aircraft = 2;
    plan.desired_screen = 0;
    let mut planner = planner_with_operation(operation, plan);

    let completion = think(&mut planner, &battle, &knowledge(&battle));
    let operation = planner
        .air_operation()
        .expect("the survivors receive one return order");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::Complete)
    );
    assert!(completion.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));

    battle.tick += 1;
    battle.my_units.clear();
    let released = think(&mut planner, &battle, &knowledge(&battle));
    assert!(released.reservations.is_empty());
    assert!(planner.air_operation().is_none());
    assert_eq!(
        planner.terminal_outcome(),
        Some(AirOperationOutcome::Released {
            player: PlayerId(1),
            target: TARGET,
        })
    );

    battle.tick += 1;
    think(&mut planner, &battle, &knowledge(&battle));
    assert_eq!(
        planner.terminal_outcome(),
        None,
        "the handoff signal is emitted for exactly one think"
    );
}

#[test]
fn a_visible_missing_objective_aborts_before_the_bombers_commit() {
    let mut battle = obs(400);
    battle.enemy_buildings.clear();
    see_approach(&mut battle);
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Verify, battle.tick);

    let decision = think(&mut planner, &battle, &intel);

    let operation = planner
        .air_operation()
        .expect("recovery remains observable");
    assert_eq!(operation.phase(), AirOperationPhase::Recover);
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::ObjectiveLost)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn a_strike_aborts_when_its_previously_viable_staging_area_is_severed() {
    let mut battle = obs(400);
    let ideal = staging(HOME, TARGET);
    battle.known_rock = (0..battle.map_height)
        .flat_map(|y| (ideal.x - 3..=ideal.x + 3).map(move |x| TilePos::new(x, y)))
        .collect();
    let intel = knowledge(&battle);
    let mut planner = with_operation(AirOperationPhase::Strike, battle.tick);

    let decision = think(&mut planner, &battle, &intel);

    let operation = planner
        .air_operation()
        .expect("recovery remains observable");
    assert_eq!(
        operation.recovery_reason(),
        Some(AirRecoveryReason::UnreachableStaging)
    );
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
    )));
}

#[test]
fn a_siege_identity_trains_available_avalanches_for_its_operation() {
    let fixture_planning = crate::planning::PlanningWork::default();

    let mut observation = obs(300);
    observation.visible.fill(true);
    observation.explored.fill(true);
    observation.my_units.retain(|unit| unit.id != UnitId(2));
    observation.my_buildings = vec![building(
        30,
        0,
        BuildingKind::Crucible,
        TilePos::new(10, 15),
        true,
    )];
    observation.my_queues = vec![Vec::new()];
    observation.scrap = UnitKind::Avalanche.stats().cost;
    let identity = ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        20_043,
    ));
    let plan = derived_connected_test_plan(&identity, &observation)
        .expect("the completed Crucible can supply the package");
    let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
    operation.artillery.clear();
    let mut decision = StrategicDecision::default();
    let intelligence = knowledge(&observation);

    procure_connected_in_test(
        &operation,
        &plan,
        &planning_context(&fixture_planning, &identity, &observation, &intelligence),
        &mut decision,
    );

    assert!(decision.intents.contains(&Intent::TrainAt {
        building: BuildingId(30),
        kind: UnitKind::Avalanche,
    }));
    assert!(decision.intents.iter().all(|intent| !matches!(
        intent,
        Intent::TrainAt {
            kind: UnitKind::Bombard,
            ..
        }
    )));
    assert_eq!(plan.desired_artillery, 1);
}

#[test]
fn each_developed_economy_path_can_fund_an_island_operation() {
    let mut observation = wealthy_island_obs(5_000, 1);
    observation
        .my_buildings
        .retain(|building| building.kind != BuildingKind::Reclaimer);
    observation.scrap = 0;
    observation.my_buildings.push(building(
        31,
        0,
        BuildingKind::Foundry,
        TilePos::new(8, 2),
        true,
    ));
    let target = knowledge(&observation).buildings()[0].clone();

    assert!(wealthy_island_target(
        &profile(),
        &observation,
        HOME,
        &target,
        None,
    ));

    observation
        .my_buildings
        .retain(|building| building.id != BuildingId(31));
    let bomber_bank = UnitKind::Condor.stats().cost.saturating_mul(4);
    observation.scrap = bomber_bank;
    assert!(wealthy_island_target(
        &profile(),
        &observation,
        HOME,
        &target,
        None,
    ));

    observation.scrap = bomber_bank - 1;
    assert!(
        !wealthy_island_target(&profile(), &observation, HOME, &target, None),
        "one Foundry, no renewable income, and an underfunded bank is not a mature economy"
    );
}

#[test]
fn every_difficulty_uses_its_exact_tactical_memory_boundary_monotonically() {
    let seen = obs(100);
    let mut intel = knowledge(&seen);
    let mut hidden = obs(101);
    hidden.enemy_buildings[0].seen = false;
    intel.update(&hidden);

    for difficulty in BotDifficulty::ALL {
        let memory = DifficultyTuning::for_level(difficulty).tactical_memory;
        let remembered = select_target(&intel, 100 + memory, memory)
            .expect("evidence remains actionable through the authored boundary");
        assert_eq!(remembered.id, Some(BuildingId(80)), "{difficulty:?}");
        assert_eq!(
            remembered.evidence,
            ContactEvidence::Remembered,
            "{difficulty:?}"
        );
        assert!(
            select_target(&intel, 100 + memory + 1, memory).is_none(),
            "{difficulty:?} retained evidence beyond its tactical-memory limit"
        );
    }

    for pair in BotDifficulty::ALL.windows(2) {
        let lower = DifficultyTuning::for_level(pair[0]);
        let higher = DifficultyTuning::for_level(pair[1]);
        let now = 100 + lower.tactical_memory + 1;
        assert!(
            select_target(&intel, now, lower.tactical_memory).is_none(),
            "{:?} should have forgotten at the adjacent-rung probe",
            pair[0]
        );
        assert!(
            select_target(&intel, now, higher.tactical_memory).is_some(),
            "{:?} should retain what {:?} has just forgotten",
            pair[1],
            pair[0]
        );
    }
}

#[test]
fn current_target_outranks_a_more_valuable_remembered_target() {
    let seen = obs(100);
    let mut intel = knowledge(&seen);
    let mut later = obs(400);
    later.enemy_buildings[0].seen = false;
    later.enemy_buildings.push(building(
        81,
        1,
        BuildingKind::Reclaimer,
        TilePos::new(18, 5),
        true,
    ));
    intel.update(&later);

    for difficulty in BotDifficulty::ALL {
        let tuning = DifficultyTuning::for_level(difficulty);
        let selected = select_target(&intel, later.tick, tuning.tactical_memory)
            .expect("a current economic target is actionable");
        assert_eq!(selected.id, Some(BuildingId(81)), "{difficulty:?}");
        assert_eq!(selected.evidence, ContactEvidence::Current);
    }
}

#[test]
fn strategic_air_targeting_spends_its_attention_on_the_highest_value_infrastructure() {
    let mut seen = obs(100);
    let priorities = [
        BuildingKind::Crucible,
        BuildingKind::Airworks,
        BuildingKind::Fabricator,
        BuildingKind::Foundry,
        BuildingKind::Extractor,
        BuildingKind::Bastion,
        BuildingKind::Reclaimer,
        BuildingKind::Turret,
    ];
    seen.enemy_buildings = priorities
        .iter()
        .copied()
        .chain([BuildingKind::FlakTurret])
        .enumerate()
        .map(|(index, kind)| {
            let offset = u32::try_from(index).expect("the fixed target fixture fits in u32");
            building(
                80 + offset,
                1,
                kind,
                TilePos::new(
                    10 + i32::try_from(index).expect("the fixed target fixture fits in i32"),
                    5,
                ),
                true,
            )
        })
        .collect();

    for expected in priorities {
        let intel = knowledge(&seen);
        let selected = select_target(&intel, seen.tick, u64::MAX)
            .expect("at least one strategic target remains");
        assert_eq!(selected.kind, expected);
        seen.enemy_buildings
            .retain(|building| building.kind != expected);
    }

    let intel = knowledge(&seen);
    assert!(
        select_target(&intel, seen.tick, u64::MAX).is_none(),
        "static anti-air alone is a suppression problem, not an air-operation objective"
    );
}

#[test]
fn resolved_air_personalities_change_island_timing_and_wing_size_without_removing_it() {
    let low = crate::profile::ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        19,
    ));
    let high = crate::profile::ResolvedProfile::resolve(BotConfig::scripted(
        BotDifficulty::Prime,
        BotStance::Balanced,
        1,
    ));
    assert_eq!(
        (low.primary, low.secondary),
        (Specialty::Fortification, Specialty::Air)
    );
    assert_eq!(
        (high.primary, high.secondary),
        (Specialty::Fortification, Specialty::Air)
    );
    assert_eq!((low.traits.air, low.traits.guile), (48, 47));
    assert_eq!((high.traits.air, high.traits.guile), (61, 37));

    let earliest = |identity: &ResolvedProfile| {
        ISLAND_OPERATION_EARLIEST_TICK
            .saturating_add(250)
            .saturating_add(u64::from(100 - identity.traits.air) * 8)
    };
    let early = wealthy_island_obs(earliest(&high), 1);
    let early_target = knowledge(&early).buildings()[0].clone();
    assert!(wealthy_island_target(
        &high,
        &early,
        HOME,
        &early_target,
        None,
    ));
    assert!(
        !wealthy_island_target(&low, &early, HOME, &early_target, None),
        "the lower-air identity keeps the operation but prepares it longer"
    );

    let later = wealthy_island_obs(earliest(&low), 1);
    let later_target = knowledge(&later).buildings()[0].clone();
    assert!(wealthy_island_target(
        &low,
        &later,
        HOME,
        &later_target,
        None,
    ));
    assert!(wealthy_island_target(
        &high,
        &later,
        HOME,
        &later_target,
        None,
    ));

    let low_plan = AirPlan::island(&low, &later);
    let high_plan = AirPlan::island(&high, &later);
    assert_eq!(
        high_plan.desired_strike_aircraft,
        low_plan.desired_strike_aircraft + 1
    );
    assert_eq!(high_plan.desired_screen, low_plan.desired_screen + 1);
}

#[test]
fn stance_changes_island_timing_force_size_and_retry_cadence() {
    let personality_delay = u64::from(100u8.saturating_sub(profile().traits.air)) * 8;
    let observation = wealthy_island_obs(
        ISLAND_OPERATION_EARLIEST_TICK.saturating_add(personality_delay),
        1,
    );
    let target = knowledge(&observation).buildings()[0].clone();
    let mut aggressive = profile();
    aggressive.stance = BotStance::Aggressive;
    let mut turtle = profile();
    turtle.stance = BotStance::Turtle;

    assert!(wealthy_island_target(
        &aggressive,
        &observation,
        HOME,
        &target,
        None,
    ));
    assert!(!wealthy_island_target(
        &turtle,
        &observation,
        HOME,
        &target,
        None,
    ));

    let aggressive_plan = AirPlan::island(&aggressive, &observation);
    let turtle_plan = AirPlan::island(&turtle, &observation);
    assert_eq!(
        aggressive_plan.desired_strike_aircraft,
        turtle_plan.desired_strike_aircraft + 2
    );
    assert!(aggressive_plan.assembly_timeout > turtle_plan.assembly_timeout);

    let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
    assert!(cooldown(&turtle, tuning) > cooldown(&aggressive, tuning));

    let mut later = observation.clone();
    later.tick = later.tick.saturating_add(500);
    assert!(wealthy_island_target(&turtle, &later, HOME, &target, None,));
}

#[test]
fn active_paid_operations_share_one_stale_target_boundary_across_difficulties() {
    let seen = obs(132);
    let mut intel = knowledge(&seen);
    let boundary = seen.tick + ACTIVE_OPERATION_TARGET_MEMORY;
    let mut hidden = obs(boundary);
    hidden.enemy_buildings = vec![building(80, 1, BuildingKind::Crucible, TARGET, false)];
    intel.update(&hidden);
    let mut cases = BotDifficulty::ALL.map(|difficulty| {
        let mut identity = profile();
        identity.difficulty = difficulty;
        (
            difficulty,
            identity,
            with_operation(AirOperationPhase::Recon, boundary),
        )
    });

    for (difficulty, identity, planner) in &mut cases {
        let retained = planner.think_alone(
            identity,
            DifficultyTuning::for_level(*difficulty),
            &hidden,
            &intel,
            HOME,
            &[],
        );
        let operation = planner.air_operation().expect("the boundary is inclusive");
        assert_eq!(
            operation.phase(),
            AirOperationPhase::Recon,
            "{difficulty:?}"
        );
        assert_eq!(operation.recovery_reason(), None, "{difficulty:?}");
        assert_eq!(
            retained.reservations,
            [UnitId(1), UnitId(2), UnitId(3), UnitId(4)],
            "{difficulty:?}"
        );
    }

    hidden.tick += 1;
    intel.update(&hidden);
    for (difficulty, identity, planner) in &mut cases {
        let recovered = planner.think_alone(
            identity,
            DifficultyTuning::for_level(*difficulty),
            &hidden,
            &intel,
            HOME,
            &[],
        );
        let operation = planner
            .air_operation()
            .expect("survivors receive one recovery order before release");
        assert_eq!(
            operation.phase(),
            AirOperationPhase::Recover,
            "{difficulty:?}"
        );
        assert_eq!(
            operation.recovery_reason(),
            Some(AirRecoveryReason::StaleIntelligence),
            "{difficulty:?}"
        );
        assert_eq!(
            recovered.committed_scrap(),
            0,
            "{difficulty:?} must release its factory bank on the boundary"
        );
    }
}

#[test]
fn prime_can_select_a_remembered_target_after_veteran_forgets_it() {
    let seen = obs(100);
    let mut intel = knowledge(&seen);
    let veteran = DifficultyTuning::for_level(BotDifficulty::Veteran);
    let prime = DifficultyTuning::for_level(BotDifficulty::Prime);
    let now = seen.tick + veteran.tactical_memory + 1;
    let mut hidden = obs(now);
    hidden.enemy_buildings[0].seen = false;
    intel.update(&hidden);

    assert_eq!(veteran.tactical_memory, ACTIVE_OPERATION_TARGET_MEMORY);
    assert!(select_target(&intel, now, veteran.tactical_memory).is_none());
    let selected = select_target(&intel, now, prime.tactical_memory)
        .expect("Prime's longer selection memory remains useful before commitment");
    assert_eq!(selected.id, Some(BuildingId(80)));
    assert_eq!(selected.evidence, ContactEvidence::Remembered);
}
