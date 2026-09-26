use super::super::super::intelligence::StrategicIntelligence;
use super::super::super::observation::{BuildingObs, UnitObs};
use super::super::super::profile::{PersonalityTraits, Specialty};
use super::super::super::resources::ResourceSnapshot;
use super::*;
use crate::resources::test_support::all_producers;
use oxide_sim::ids::{BuildingId, PlayerId};
use oxide_sim::scenario::{BotConfig, BotDifficulty, BotStance};
use oxide_sim::state::Faction;
use oxide_sim::stats::QUEUE_CAP;

const TEST_DECISION_CADENCE: Tick = 12;

fn constraints(deadline: Tick, protected_forecast_scrap: u32) -> PreparationConstraints {
    PreparationConstraints {
        deadline,
        decision_cadence: TEST_DECISION_CADENCE,
        protected_forecast_scrap,
    }
}

fn profile(air: u8, siege: u8) -> ResolvedProfile {
    profile_for(BotDifficulty::Prime, BotStance::Balanced, air, siege)
}

fn profile_for(
    difficulty: BotDifficulty,
    stance: BotStance,
    air: u8,
    siege: u8,
) -> ResolvedProfile {
    ResolvedProfile {
        difficulty,
        stance,
        personality_seed: 7,
        primary: if air > siege {
            Specialty::Air
        } else {
            Specialty::Siege
        },
        secondary: if air > siege {
            Specialty::Siege
        } else {
            Specialty::Air
        },
        traits: PersonalityTraits {
            air,
            siege,
            support: 50,
            fortification: 50,
            greed: 50,
            guile: 50,
        },
    }
}

fn observation(scrap: u32) -> Observation {
    Observation::from_data(ObservationData {
        tick: 24,
        scrap,
        map_width: 30,
        map_height: 30,
        visible: vec![true; 900],
        explored: vec![true; 900],
        ..crate::test_support::observation_data()
    })
}

fn building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
    crate::test_support::building(id, PlayerId(player), kind, anchor)
}

fn unit(id: u32, player: u8, kind: UnitKind, tile: TilePos) -> UnitObs {
    crate::test_support::unit(id, PlayerId(player), kind, tile)
}

fn add_producer(
    observation: &mut Observation,
    id: u32,
    kind: BuildingKind,
    anchor: TilePos,
    queue: Vec<UnitKind>,
) {
    let player = observation.me.0;
    observation
        .my_buildings
        .push(building(id, player, kind, anchor));
    observation.my_queues.push(queue);
}

fn add_owned_building(
    observation: &mut Observation,
    id: u32,
    kind: BuildingKind,
    anchor: TilePos,
    built: bool,
) {
    let mut owned = building(id, observation.me.0, kind, anchor);
    owned.built = built;
    observation.my_buildings.push(owned);
    observation.my_queues.push(Vec::new());
}

fn add_baseline_tech(observation: &mut Observation) {
    add_producer(
        observation,
        10,
        BuildingKind::Foundry,
        TilePos::new(2, 2),
        Vec::new(),
    );
    add_producer(
        observation,
        11,
        BuildingKind::Fabricator,
        TilePos::new(5, 2),
        Vec::new(),
    );
    add_producer(
        observation,
        12,
        BuildingKind::Airworks,
        TilePos::new(8, 2),
        Vec::new(),
    );
}

fn add_complete_tech(observation: &mut Observation) {
    add_baseline_tech(observation);
    add_producer(
        observation,
        13,
        BuildingKind::Crucible,
        TilePos::new(11, 2),
        Vec::new(),
    );
}

fn refine_roster(
    obs: &Observation,
    roster: &[(UnitKind, usize)],
    deadline: Tick,
    planning: &PlanningWork,
) -> Progress<()> {
    let resources = ResourceSnapshot::from_observation(obs);
    let demands: Vec<_> = roster
        .iter()
        .map(|&(kind, count)| ProviderDemandTranche {
            priority: ProviderPriority::Minimum,
            family: match kind.role() {
                Role::Scout => ForceFamily::Recon,
                Role::Bombard => ForceFamily::Suppression,
                _ => ForceFamily::Strike,
            },
            kind,
            count,
        })
        .collect();
    refine_provider_demands(
        ProductionEvidence::with_planning(&resources, &all_producers(&resources), Some(planning)),
        &demands,
        obs.tick,
        constraints(deadline, 0),
        ConnectedOffenseKey {
            objective: BuildingId(99),
            anchor: TilePos::new(25, 25),
        },
    )
}

#[test]
fn missing_planning_continuation_defers_even_a_feasible_purchase() {
    let mut obs = observation(10_000);
    add_baseline_tech(&mut obs);
    let resources = ResourceSnapshot::from_observation(&obs);
    let demands = [ProviderDemandTranche {
        priority: ProviderPriority::Minimum,
        family: ForceFamily::Strike,
        kind: UnitKind::Sentinel,
        count: 1,
    }];
    let result = refine_provider_demands(
        ProductionEvidence::with_planning(&resources, &all_producers(&resources), None),
        &demands,
        obs.tick,
        constraints(5_000, 0),
        ConnectedOffenseKey {
            objective: BuildingId(99),
            anchor: TilePos::new(25, 25),
        },
    );
    assert_eq!(result, Progress::Deferred);
}

#[test]
fn shared_forecast_rejects_overfull_air_group_despite_spare_ground_capacity() {
    let mut obs = observation(100_000);
    obs.tick = 120;
    obs.faction = Faction::Cupric;
    for id in 0..16 {
        add_producer(
            &mut obs,
            id + 1,
            BuildingKind::Airworks,
            TilePos::new(2 + (id % 4) as i32 * 6, 2 + (id / 4) as i32 * 6),
            vec![],
        );
    }
    add_producer(
        &mut obs,
        20,
        BuildingKind::Fabricator,
        TilePos::new(26, 26),
        vec![],
    );
    let deadline = obs.tick + 2400;
    let overfull = [
        (UnitKind::Gnat, 1),
        (UnitKind::Darter, 256),
        (UnitKind::Bombard, 1),
    ];
    let work = PlanningWork::default();
    assert_eq!(
        refine_roster(&obs, &overfull, deadline, &work),
        Progress::ProvenInfeasible
    );
    assert!(
        (1..16_384).contains(&work.spent()),
        "the producer-subset bound must reject without packing the roster"
    );
    let zero = PlanningWork::with_allowance(0);
    assert_eq!(
        refine_roster(&obs, &overfull, deadline, &zero),
        Progress::Deferred
    );
    assert_eq!(zero.spent(), 0);
    let near_capacity = [
        (UnitKind::Gnat, 1),
        (UnitKind::Darter, 255),
        (UnitKind::Bombard, 1),
    ];
    let work = PlanningWork::default();
    assert_ne!(
        refine_roster(&obs, &near_capacity, deadline, &work),
        Progress::ProvenInfeasible,
        "a finite allowance may defer a feasible large roster, never reject it"
    );
}

#[test]
fn shared_forecast_reassigns_scout_to_leave_the_only_bomber_lane() {
    for (faction, scout, bomber, queued) in [
        (
            Faction::Ferrous,
            UnitKind::Kestrel,
            UnitKind::Condor,
            UnitKind::Talon,
        ),
        (
            Faction::Cupric,
            UnitKind::Gnat,
            UnitKind::Moth,
            UnitKind::Wisp,
        ),
    ] {
        let mut obs = observation(10_000);
        obs.tick = 120;
        obs.faction = faction;
        add_complete_tech(&mut obs);
        add_producer(
            &mut obs,
            14,
            BuildingKind::Airworks,
            TilePos::new(14, 2),
            vec![queued],
        );
        let deadline = obs.tick + Tick::from(bomber.stats().train_ticks) + 1;
        assert_eq!(
            refine_roster(
                &obs,
                &[(scout, 1), (bomber, 1)],
                deadline,
                &PlanningWork::default()
            ),
            Progress::Ready(())
        );
    }
}

#[test]
fn shared_forecast_matches_independent_funded_oracle_on_small_rosters() {
    for (queued, scrap, restricted) in [
        (0, 10_000, false),
        (2, 10_000, false),
        (4, 10_000, false),
        (0, 250, false),
        (2, 10_000, true),
        (4, 250, true),
    ] {
        let mut obs = observation(scrap);
        obs.tick = 120;
        add_complete_tech(&mut obs);
        obs.my_queues[2] = vec![UnitKind::Buzzard; queued];
        add_producer(
            &mut obs,
            14,
            BuildingKind::Airworks,
            TilePos::new(14, 2),
            vec![],
        );
        let resources = ResourceSnapshot::from_observation(&obs);
        let access = if restricted {
            ProductionAccess::restricted_kinds(vec![
                (BuildingId(12), UnitKind::Buzzard),
                (BuildingId(12), UnitKind::Condor),
                (BuildingId(14), UnitKind::Buzzard),
            ])
        } else {
            all_producers(&resources)
        };
        for horizon in [179, 180, 299, 300, 600] {
            for buzzards in 0..=2 {
                for condors in 0..=2 {
                    let demands = [
                        ProviderDemandTranche {
                            priority: ProviderPriority::Minimum,
                            family: ForceFamily::Strike,
                            kind: UnitKind::Buzzard,
                            count: buzzards,
                        },
                        ProviderDemandTranche {
                            priority: ProviderPriority::Minimum,
                            family: ForceFamily::Strike,
                            kind: UnitKind::Condor,
                            count: condors,
                        },
                    ];
                    let deadline = obs.tick + horizon;
                    let expected = provider_demands_fit_funded_horizon(
                        &resources,
                        &demands,
                        obs.tick,
                        constraints(deadline, 0),
                        &access,
                    );
                    let actual = refine_provider_demands(
                        ProductionEvidence::with_planning(
                            &resources,
                            &access,
                            Some(&PlanningWork::default()),
                        ),
                        &demands,
                        obs.tick,
                        constraints(deadline, 0),
                        ConnectedOffenseKey {
                            objective: BuildingId(99),
                            anchor: TilePos::new(25, 25),
                        },
                    );
                    assert_eq!(
                        actual,
                        if expected {
                            Progress::Ready(())
                        } else {
                            Progress::ProvenInfeasible
                        },
                        "queued={queued}, scrap={scrap}, restricted={restricted}, horizon={horizon}, buzzards={buzzards}, condors={condors}"
                    );
                }
            }
        }
    }
}

#[test]
fn shared_forecast_defers_without_rejecting_and_resumes_the_minimum() {
    let mut obs = observation(10_000);
    obs.tick = 0;
    add_complete_tech(&mut obs);
    let (mut intelligence, _) = intelligence_with_target(&mut obs, 4);
    let zero = PlanningWork::with_allowance(0);
    let work = PlanningWork::with_allowance(4_096);
    let mut cloned = work.clone();
    let derive = |obs: &Observation, intel: &StrategicIntelligence, planning: &PlanningWork| {
        let target = intel
            .buildings()
            .iter()
            .find(|contact| contact.kind == BuildingKind::Crucible)
            .unwrap();
        let resources = ResourceSnapshot::from_observation(obs);
        derive_connected_minimum_for_cluster(
            &profile(50, 50),
            obs,
            intel,
            ConnectedTargetEvidence {
                primary: target,
                cluster: &[target],
                committed: None,
            },
            ProductionEvidence::with_planning(
                &resources,
                &all_producers(&resources),
                Some(planning),
            ),
            &[],
            constraints(2_500, 0),
        )
    };
    for _ in 0..2 {
        assert_eq!(
            derive(&obs, &intelligence, &zero),
            Err(ForcePackageRejection::Deferred)
        );
        assert_eq!(zero.spent(), 0);
    }
    let mut ready = None;
    for tick in (0..120).step_by(12) {
        obs.tick = tick;
        intelligence.update(&obs);
        let result = derive(&obs, &intelligence, &work);
        assert_eq!(result, derive(&obs, &intelligence, &cloned));
        assert!(work.spent() <= 4_096);
        match result {
            Ok(package) => {
                ready = Some(package.minimum);
                break;
            }
            Err(ForcePackageRejection::Deferred) => {}
            Err(reason) => panic!("a pending funded minimum was rejected: {reason:?}"),
        }
        cloned = work.clone();
    }
    let ready = ready.expect("the minimum finishes before its pending work expires");
    let resources = ResourceSnapshot::from_observation(&obs);
    assert!(funded_providers_fit(
        &resources,
        &ready.funded_providers,
        ready.preparation_deadline,
        &all_producers(&resources)
    ));
}

#[test]
fn runtime_composition_leaves_admission_work_and_only_offers_funded_rosters() {
    let mut offered_growth = false;
    for scrap in [700, 1600, 10000] {
        let mut obs = observation(scrap);
        obs.tick = 0;
        add_complete_tech(&mut obs);
        add_valuable_cluster(&mut obs);
        let (intel, target) = intelligence_with_target(&mut obs, 4);
        let resources = ResourceSnapshot::from_observation(&obs);
        let work = PlanningWork::default();
        let options = derive_connected_force_package_options_for_cluster(
            &profile(50, 50),
            &obs,
            &intel,
            ConnectedTargetEvidence {
                primary: &target,
                cluster: &intel.buildings().iter().collect::<Vec<_>>(),
                committed: None,
            },
            ProductionEvidence::with_planning(&resources, &all_producers(&resources), Some(&work)),
            &[],
            constraints(2500, 0),
        )
        .expect("the completed production base admits a funded package");
        offered_growth |= !options.marginal.is_empty();
        assert!(work.spent() <= 96_000);
        for package in std::iter::once(options.minimum).chain(options.marginal) {
            assert!(funded_providers_fit(
                &resources,
                &package.funded_providers,
                package.preparation_deadline,
                &all_producers(&resources)
            ));
        }
    }
    assert!(offered_growth);
}

#[test]
fn investment_witness_only_derives_a_complete_minimum() {
    let mut observation = observation(10_000);
    add_complete_tech(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 4);
    let resources = ResourceSnapshot::from_observation(&observation);
    let minimum = derive_connected_minimum_for_cluster(
        &profile(50, 50),
        &observation,
        &intelligence,
        ConnectedTargetEvidence {
            primary: &target,
            cluster: &[&target],
            committed: None,
        },
        ProductionEvidence::with_planning(
            &resources,
            &all_producers(&resources),
            Some(&PlanningWork::default()),
        ),
        &[],
        constraints(2_500, 0),
    )
    .expect("a funded minimum is available");
    assert!(minimum.marginal.is_empty());
    for family in ForceFamily::ALL {
        assert!(
            minimum.minimum.chosen_capability.for_family(family)
                >= minimum.minimum.minimum_capability.for_family(family)
        );
    }
    assert!(
        minimum
            .minimum
            .provider_priority
            .iter()
            .all(|provider| provider.priority == ProviderPriority::Minimum)
    );
    assert!(provider_demands_fit_funded_horizon(
        &resources,
        &minimum.minimum.provider_priority,
        observation.tick,
        constraints(2_500, 0),
        &all_producers(&resources),
    ));
}

fn intelligence_with_target(
    observation: &mut Observation,
    anti_air: usize,
) -> (StrategicIntelligence, BuildingContact) {
    observation.enemy_buildings.push(building(
        100,
        1,
        BuildingKind::Crucible,
        TilePos::new(20, 20),
    ));
    let anchors = [
        TilePos::new(18, 18),
        TilePos::new(20, 18),
        TilePos::new(22, 18),
        TilePos::new(18, 22),
    ];
    for (index, anchor) in anchors.into_iter().take(anti_air).enumerate() {
        observation.enemy_buildings.push(building(
            101 + u32::try_from(index).expect("small fixture"),
            1,
            BuildingKind::FlakTurret,
            anchor,
        ));
    }
    let mut intelligence = StrategicIntelligence::default();
    intelligence.update(observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.kind == BuildingKind::Crucible)
        .expect("current target")
        .clone();
    (intelligence, target)
}

fn derive(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    target: &BuildingContact,
    unavailable: &[UnitId],
    deadline: Tick,
) -> Result<ConnectedForcePackage, ForcePackageRejection> {
    derive_with_forecast_reserve(
        profile,
        observation,
        intelligence,
        target,
        unavailable,
        deadline,
        0,
    )
}

fn derive_settled(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    target: &BuildingContact,
    unavailable: &[UnitId],
    deadline: Tick,
) -> Result<ConnectedForcePackage, ForcePackageRejection> {
    let resources = ResourceSnapshot::from_observation(observation);
    let access = all_producers(&resources);
    let cluster = current_target_cluster(intelligence, target.player, target.anchor);
    let planning = PlanningWork::default();
    loop {
        let spent = planning.spent();
        let options = derive_connected_force_package_options_for_cluster(
            profile,
            observation,
            intelligence,
            ConnectedTargetEvidence {
                primary: target,
                cluster: &cluster,
                committed: None,
            },
            ProductionEvidence::with_planning(&resources, &access, Some(&planning)),
            unavailable,
            constraints(deadline, 0),
        )?;
        if !options.refinement_pending {
            return Ok(options.into_largest());
        }
        if planning.spent() == spent {
            return Err(ForcePackageRejection::Deferred);
        }
    }
}

fn derive_with_forecast_reserve(
    profile: &ResolvedProfile,
    observation: &Observation,
    intelligence: &StrategicIntelligence,
    target: &BuildingContact,
    unavailable: &[UnitId],
    deadline: Tick,
    protected_forecast_scrap: u32,
) -> Result<ConnectedForcePackage, ForcePackageRejection> {
    let resources = ResourceSnapshot::from_observation(observation);
    derive_connected_force_package(
        profile,
        observation,
        intelligence,
        target,
        ProductionEvidence::with_planning(
            &resources,
            &all_producers(&resources),
            Some(&PlanningWork::default()),
        ),
        unavailable,
        constraints(deadline, protected_forecast_scrap),
    )
}

fn demand(demands: &[ProviderDemand], kind: UnitKind) -> usize {
    demands
        .iter()
        .find(|demand| demand.kind == kind)
        .map_or(0, |demand| demand.count)
}

fn realized_capability(
    package: &ConnectedForcePackage,
    family: ForceFamily,
    faction: Faction,
) -> u64 {
    let demands = match family {
        ForceFamily::Recon => &package.recon,
        ForceFamily::Suppression => &package.suppression,
        ForceFamily::Strike => &package.strike,
    };
    demands.iter().fold(0_u64, |total, demand| {
        let count = u64::try_from(demand.count).unwrap_or(u64::MAX);
        total
            .saturating_add(provider_capability(family, demand.kind, faction).saturating_mul(count))
    })
}

fn demanded_scrap(package: &ConnectedForcePackage) -> u32 {
    package
        .recon
        .iter()
        .chain(&package.suppression)
        .chain(&package.strike)
        .fold(0_u32, |total, demand| {
            total.saturating_add(
                demand
                    .kind
                    .stats()
                    .cost
                    .saturating_mul(u32::try_from(demand.count).unwrap_or(u32::MAX)),
            )
        })
}

fn baseline_portfolio_oracle(
    profile: &ResolvedProfile,
    useful: NormalizedCapability,
    scrap: u32,
    faction: Faction,
) -> (usize, usize, PackageCandidateScore) {
    let scout = Role::Scout.unit_for(faction);
    let strike = Role::AirGround.unit_for(faction);
    let scout_cost = scout.stats().cost;
    let suppression_cost = UnitKind::Bombard.stats().cost;
    let strike_cost = strike.stats().cost;
    let mut best: Option<(usize, usize, PackageCandidateScore)> = None;
    for suppression_count in 1..=usize::try_from(scrap / suppression_cost).unwrap_or(0) {
        for strike_count in 1..=usize::try_from(scrap / strike_cost).unwrap_or(0) {
            let cost = scout_cost
                .saturating_add(
                    suppression_cost
                        .saturating_mul(u32::try_from(suppression_count).unwrap_or(u32::MAX)),
                )
                .saturating_add(
                    strike_cost.saturating_mul(u32::try_from(strike_count).unwrap_or(u32::MAX)),
                );
            if cost > scrap {
                continue;
            }
            let suppression = NORMALIZED_PROVIDER
                .saturating_mul(u64::try_from(suppression_count).unwrap_or(u64::MAX));
            let strike =
                NORMALIZED_PROVIDER.saturating_mul(u64::try_from(strike_count).unwrap_or(u64::MAX));
            let capped_suppression = suppression.min(useful.suppression);
            let capped_strike = strike.min(useful.strike);
            let total_useful = NORMALIZED_PROVIDER
                .min(useful.recon)
                .saturating_add(capped_suppression)
                .saturating_add(capped_strike);
            let personality_value = u128::from(capped_suppression)
                .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.siege)))
                .saturating_add(
                    u128::from(capped_strike)
                        .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.air))),
                );
            let raw = NORMALIZED_PROVIDER
                .saturating_add(suppression)
                .saturating_add(strike);
            let score = (total_useful, personality_value, Reverse(cost), raw);
            let candidate = (suppression_count, strike_count, score);
            if best.as_ref().is_none_or(|current| candidate.2 > current.2) {
                best = Some(candidate);
            }
        }
    }
    best.expect("the oracle budget funds the common baseline minimum")
}

fn baseline_aa_fixture(
    scrap: u32,
    target_hp: u32,
    first_flak_hp: u32,
) -> (Observation, StrategicIntelligence, BuildingContact) {
    let mut observation = observation(scrap);
    add_baseline_tech(&mut observation);
    let mut target = building(100, 1, BuildingKind::Foundry, TilePos::new(20, 20));
    target.hp = target_hp;
    let mut first_flak = building(101, 1, BuildingKind::FlakTurret, TilePos::new(18, 18));
    first_flak.hp = first_flak_hp;
    observation.enemy_buildings.extend([
        target,
        first_flak,
        building(102, 1, BuildingKind::FlakTurret, TilePos::new(20, 18)),
    ]);
    observation
        .enemy_units
        .push(unit(200, 1, UnitKind::Stinger, TilePos::new(20, 19)));
    let mut intelligence = StrategicIntelligence::default();
    intelligence.update(&observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.id == Some(BuildingId(100)))
        .expect("current Foundry target")
        .clone();
    (observation, intelligence, target)
}

fn minimum_connected_cost(faction: Faction) -> u32 {
    Role::Scout
        .unit_for(faction)
        .stats()
        .cost
        .saturating_add(UnitKind::Bombard.stats().cost)
        .saturating_add(Role::AirGround.unit_for(faction).stats().cost)
}

fn add_valuable_cluster(observation: &mut Observation) {
    for (index, (kind, anchor)) in [
        (BuildingKind::Foundry, TilePos::new(20, 17)),
        (BuildingKind::Airworks, TilePos::new(17, 20)),
        (BuildingKind::Fabricator, TilePos::new(22, 20)),
        (BuildingKind::Extractor, TilePos::new(20, 22)),
    ]
    .into_iter()
    .enumerate()
    {
        observation.enemy_buildings.push(building(
            150 + u32::try_from(index).expect("small fixture"),
            1,
            kind,
            anchor,
        ));
    }
}

fn add_dense_ground_targets(observation: &mut Observation) {
    for (index, tile) in [
        TilePos::new(18, 20),
        TilePos::new(19, 19),
        TilePos::new(19, 21),
        TilePos::new(20, 18),
        TilePos::new(20, 22),
        TilePos::new(21, 19),
        TilePos::new(21, 22),
        TilePos::new(22, 20),
        TilePos::new(22, 21),
        TilePos::new(23, 20),
    ]
    .into_iter()
    .enumerate()
    {
        observation.enemy_units.push(unit(
            300 + u32::try_from(index).expect("small fixture"),
            1,
            UnitKind::Lancer,
            tile,
        ));
    }
}

#[test]
fn full_salvo_values_moth_above_single_bomb_condor() {
    assert!(ground_firepower(UnitKind::Moth) > ground_firepower(UnitKind::Condor));
    assert!(
        provider_capability(ForceFamily::Strike, UnitKind::Moth, Faction::Cupric)
            > NORMALIZED_PROVIDER
    );
}

#[test]
fn bombing_geometry_uses_the_real_salvo_spacing_and_axis() {
    let observation = observation(0);
    let weapon = UnitKind::Moth
        .stats()
        .weapons
        .first()
        .expect("Moth has a bomb stick");
    let aim = TilePos::new(10, 10).center();
    let horizontal = bomb_salvo_impacts(&observation, aim, 0, weapon);
    let vertical = bomb_salvo_impacts(&observation, aim, 64, weapon);
    assert_eq!(horizontal.len(), usize::from(weapon.salvo));
    assert_eq!(
        horizontal[1].x - horizontal[0].x,
        BOMB_SALVO_SPACING,
        "adjacent impacts use the authoritative stick spacing"
    );
    assert_eq!(horizontal[1].y, horizontal[0].y);
    assert_eq!(vertical[1].x, vertical[0].x);
    assert_eq!(vertical[1].y - vertical[0].y, BOMB_SALVO_SPACING);

    let along_horizontal_axis = TilePos::new(13, 10).center();
    assert!(horizontal.iter().any(|impact| bombing_splash_reaches(
        *impact,
        along_horizontal_axis,
        weapon
    )));
    assert!(vertical.iter().all(|impact| !bombing_splash_reaches(
        *impact,
        along_horizontal_axis,
        weapon
    )));
}

#[test]
fn bombing_opportunity_counts_each_current_victim_once() {
    let mut observation = observation(0);
    observation.faction = Faction::Cupric;
    observation.enemy_buildings.extend([
        building(100, 1, BuildingKind::Crucible, TilePos::new(20, 20)),
        building(101, 1, BuildingKind::ScuttleCharge, TilePos::new(19, 20)),
    ]);
    observation
        .enemy_units
        .push(unit(200, 1, UnitKind::Lancer, TilePos::new(22, 20)));
    let mut intelligence = StrategicIntelligence::default();
    intelligence.update(&observation);
    let cluster = current_target_cluster(&intelligence, PlayerId(1), TilePos::new(20, 20));
    let opportunity =
        current_bombing_opportunity(&observation, &intelligence, &cluster, UnitKind::Moth);
    let vulnerable_hp = u64::from(UnitKind::Lancer.stats().max_hp)
        .saturating_add(u64::from(BuildingKind::ScuttleCharge.base_stats().max_hp));
    let weapon = UnitKind::Moth.stats().weapons[0];
    let salvo_damage = u64::from(weapon.damage) * u64::from(weapon.salvo);

    assert_eq!(
        opportunity.useful,
        normalize_bombing_work(vulnerable_hp, salvo_damage),
        "a victim reachable on several candidate axes contributes its HP once"
    );
    assert!(opportunity.per_provider > 0);
    assert!(opportunity.per_provider <= opportunity.useful);
}

#[test]
fn remembered_ground_contacts_do_not_create_bombing_opportunity() {
    let mut seen = observation(0);
    seen.enemy_buildings.push(building(
        100,
        1,
        BuildingKind::Crucible,
        TilePos::new(20, 20),
    ));
    seen.enemy_units
        .push(unit(200, 1, UnitKind::Lancer, TilePos::new(22, 20)));
    let mut intelligence = StrategicIntelligence::default();
    intelligence.update(&seen);
    let seen_cluster = current_target_cluster(&intelligence, PlayerId(1), TilePos::new(20, 20));
    assert!(
        current_bombing_opportunity(&seen, &intelligence, &seen_cluster, UnitKind::Condor).useful
            > 0
    );

    let mut later = observation(0);
    later.tick = seen.tick + TEST_DECISION_CADENCE;
    later.visible.fill(false);
    later.explored.fill(true);
    later.enemy_buildings.push(building(
        100,
        1,
        BuildingKind::Crucible,
        TilePos::new(20, 20),
    ));
    for tile in [
        TilePos::new(20, 20),
        TilePos::new(21, 20),
        TilePos::new(20, 21),
        TilePos::new(21, 21),
    ] {
        let index = usize::try_from(tile.y * later.map_width + tile.x).expect("visible tile");
        later.visible[index] = true;
    }
    intelligence.update(&later);
    assert_eq!(
        intelligence.units().first().map(|contact| contact.evidence),
        Some(ContactEvidence::Remembered)
    );
    let later_cluster = current_target_cluster(&intelligence, PlayerId(1), TilePos::new(20, 20));
    assert_eq!(
        current_bombing_opportunity(&later, &intelligence, &later_cluster, UnitKind::Condor,),
        BombingOpportunity::default()
    );
}

#[test]
fn suppressible_air_defense_does_not_double_discount_bombing() {
    let mut without_flak = observation(0);
    without_flak.faction = Faction::Cupric;
    without_flak.enemy_buildings.push(building(
        100,
        1,
        BuildingKind::Crucible,
        TilePos::new(20, 20),
    ));
    without_flak
        .enemy_units
        .push(unit(200, 1, UnitKind::Lancer, TilePos::new(22, 20)));
    let mut without_flak_intelligence = StrategicIntelligence::default();
    without_flak_intelligence.update(&without_flak);
    let without_flak_cluster = current_target_cluster(
        &without_flak_intelligence,
        PlayerId(1),
        TilePos::new(20, 20),
    );
    let expected = current_bombing_opportunity(
        &without_flak,
        &without_flak_intelligence,
        &without_flak_cluster,
        UnitKind::Moth,
    );

    let mut with_flak = without_flak;
    with_flak.enemy_buildings.push(building(
        101,
        1,
        BuildingKind::FlakTurret,
        TilePos::new(19, 20),
    ));
    let mut with_flak_intelligence = StrategicIntelligence::default();
    with_flak_intelligence.update(&with_flak);
    let with_flak_cluster =
        current_target_cluster(&with_flak_intelligence, PlayerId(1), TilePos::new(20, 20));

    assert_eq!(
        current_bombing_opportunity(
            &with_flak,
            &with_flak_intelligence,
            &with_flak_cluster,
            UnitKind::Moth,
        ),
        expected,
        "the suppression family owns current ground-targetable anti-air risk"
    );
}

#[test]
fn mobile_air_defense_is_suppression_work_not_optional_bombing_value() {
    let mut baseline = observation(0);
    baseline.faction = Faction::Cupric;
    baseline.enemy_buildings.push(building(
        100,
        1,
        BuildingKind::Crucible,
        TilePos::new(20, 20),
    ));
    baseline
        .enemy_units
        .push(unit(200, 1, UnitKind::Lancer, TilePos::new(22, 20)));
    let mut baseline_intelligence = StrategicIntelligence::default();
    baseline_intelligence.update(&baseline);
    let baseline_cluster =
        current_target_cluster(&baseline_intelligence, PlayerId(1), TilePos::new(20, 20));
    let expected = current_bombing_opportunity(
        &baseline,
        &baseline_intelligence,
        &baseline_cluster,
        UnitKind::Moth,
    );

    let mut defended = baseline;
    defended.enemy_units.extend([
        unit(201, 1, UnitKind::Stinger, TilePos::new(20, 19)),
        unit(202, 1, UnitKind::Flakhound, TilePos::new(19, 20)),
    ]);
    defended.enemy_units.sort_unstable_by_key(|unit| unit.id);
    let mut defended_intelligence = StrategicIntelligence::default();
    defended_intelligence.update(&defended);
    let defended_cluster =
        current_target_cluster(&defended_intelligence, PlayerId(1), TilePos::new(20, 20));
    let defense = current_air_defense(&defended_intelligence, &defended_cluster);

    assert_eq!(
        defense.suppressible_mobile_units,
        BTreeSet::from([UnitId(201), UnitId(202)])
    );
    assert_eq!(
        current_bombing_opportunity(
            &defended,
            &defended_intelligence,
            &defended_cluster,
            UnitKind::Moth,
        ),
        expected,
        "mandatory mobile-AA suppression must not also manufacture optional bomber demand"
    );
    assert!(
        expected.useful > 0,
        "the nearby non-AA unit remains bombing value"
    );
}

#[test]
fn extreme_coordinates_do_not_wrap_into_the_target_cluster() {
    assert_eq!(
        manhattan(
            TilePos::new(i32::MIN, i32::MIN),
            TilePos::new(i32::MAX, i32::MAX),
        ),
        u32::MAX
    );
}

#[test]
fn non_actionable_targets_are_rejected() {
    let mut observation = observation(10_000);
    add_complete_tech(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);

    let mut non_actionable = target.clone();
    non_actionable.hp = 0;
    assert_eq!(
        derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &non_actionable,
            &[],
            500,
        ),
        Err(ForcePackageRejection::TargetNotActionable)
    );
}

#[test]
fn deadline_observation_can_revalidate_live_providers_but_cannot_start_new_work() {
    let mut live = observation(0);
    add_complete_tech(&mut live);
    {
        let live = &mut *live;
        live.my_units.extend([
            unit(
                40,
                live.me.0,
                Role::Scout.unit_for(live.faction),
                TilePos::new(7, 7),
            ),
            unit(41, live.me.0, UnitKind::Bombard, TilePos::new(8, 7)),
            unit(
                42,
                live.me.0,
                Role::AirGround.unit_for(live.faction),
                TilePos::new(9, 7),
            ),
        ]);
    }
    let (live_intelligence, live_target) = intelligence_with_target(&mut live, 0);

    let package = derive(
        &profile(50, 50),
        &live,
        &live_intelligence,
        &live_target,
        &[],
        live.tick,
    )
    .expect("the deadline observation may retain providers that already exist");
    assert_eq!(
        package
            .recon
            .iter()
            .map(|demand| demand.count)
            .sum::<usize>(),
        1
    );
    assert_eq!(
        package
            .suppression
            .iter()
            .map(|demand| demand.count)
            .sum::<usize>(),
        1
    );
    assert_eq!(
        package
            .strike
            .iter()
            .map(|demand| demand.count)
            .sum::<usize>(),
        1
    );

    let mut unbuilt = observation(50_000);
    add_complete_tech(&mut unbuilt);
    let (unbuilt_intelligence, unbuilt_target) = intelligence_with_target(&mut unbuilt, 0);
    assert!(
        derive(
            &profile(50, 50),
            &unbuilt,
            &unbuilt_intelligence,
            &unbuilt_target,
            &[],
            unbuilt.tick,
        )
        .is_err(),
        "scrap and idle factories cannot begin work on the deadline observation"
    );
}

#[test]
fn minimum_rejections_distinguish_resources_capability_and_timing() {
    let exact_cost = minimum_connected_cost(Faction::Ferrous);
    let mut underfunded = observation(exact_cost - 1);
    add_complete_tech(&mut underfunded);
    let (underfunded_intelligence, underfunded_target) =
        intelligence_with_target(&mut underfunded, 0);
    assert_eq!(
        derive(
            &profile(50, 50),
            &underfunded,
            &underfunded_intelligence,
            &underfunded_target,
            &[],
            500,
        ),
        Err(ForcePackageRejection::InsufficientResources {
            family: ForceFamily::Strike,
            required_scrap: exact_cost,
            available_scrap: exact_cost - 1,
            deadline_shortfall: 1,
        })
    );

    let mut missing_strike_producer = observation(10_000);
    add_producer(
        &mut missing_strike_producer,
        10,
        BuildingKind::Foundry,
        TilePos::new(2, 2),
        Vec::new(),
    );
    add_producer(
        &mut missing_strike_producer,
        11,
        BuildingKind::Fabricator,
        TilePos::new(5, 2),
        Vec::new(),
    );
    {
        let missing_strike_producer = &mut *missing_strike_producer;
        missing_strike_producer.my_units.push(unit(
            40,
            missing_strike_producer.me.0,
            Role::Scout.unit_for(missing_strike_producer.faction),
            TilePos::new(7, 7),
        ));
    }
    let (missing_intelligence, missing_target) =
        intelligence_with_target(&mut missing_strike_producer, 0);
    assert_eq!(
        derive(
            &profile(50, 50),
            &missing_strike_producer,
            &missing_intelligence,
            &missing_target,
            &[],
            1_000,
        ),
        Err(ForcePackageRejection::MissingCompletedProviderCapability {
            family: ForceFamily::Strike,
        })
    );

    let mut too_late = observation(10_000);
    add_complete_tech(&mut too_late);
    {
        let too_late = &mut *too_late;
        too_late.my_units.push(unit(
            40,
            too_late.me.0,
            Role::Scout.unit_for(too_late.faction),
            TilePos::new(7, 7),
        ));
    }
    {
        let too_late = &mut *too_late;
        too_late.my_units.push(unit(
            41,
            too_late.me.0,
            Role::AirGround.unit_for(too_late.faction),
            TilePos::new(8, 7),
        ));
    }
    let (late_intelligence, late_target) = intelligence_with_target(&mut too_late, 0);
    let deadline = too_late.tick + Tick::from(UnitKind::Bombard.stats().train_ticks) - 1;
    assert_eq!(
        derive(
            &profile(50, 50),
            &too_late,
            &late_intelligence,
            &late_target,
            &[],
            deadline,
        ),
        Err(ForcePackageRejection::PreparationWindowTooShort {
            family: ForceFamily::Suppression,
            observed_at: too_late.tick,
            deadline,
        })
    );
}

#[test]
fn current_air_domain_aa_rejects_a_ground_suppression_package() {
    let mut observation = observation(10_000);
    add_complete_tech(&mut observation);
    let interceptor_tile = TilePos::new(20, 19);
    observation
        .enemy_units
        .push(unit(200, 1, UnitKind::Talon, interceptor_tile));
    let (mut intelligence, target) = intelligence_with_target(&mut observation, 0);

    let rejection = derive(
        &profile(50, 50),
        &observation,
        &intelligence,
        &target,
        &[],
        2_500,
    )
    .expect_err("ground artillery cannot suppress a current airborne interceptor");
    let ForcePackageRejection::UntargetableCurrentAirDefense {
        firepower,
        hit_points,
    } = rejection
    else {
        panic!("unexpected rejection: {rejection:?}");
    };
    assert!(firepower > 0);
    assert_eq!(hit_points, u64::from(UnitKind::Talon.stats().max_hp));

    let mut grounded = observation.clone();
    grounded.enemy_units[0].grounded = true;
    let (grounded_intelligence, grounded_target) = intelligence_with_target(&mut grounded, 0);
    let grounded_package = derive(
        &profile(50, 50),
        &grounded,
        &grounded_intelligence,
        &grounded_target,
        &[],
        2_500,
    )
    .expect("ground artillery can suppress a currently landed interceptor");
    assert!(grounded_package.suppressible_aa_firepower > 0);
    assert_eq!(grounded_package.observed_aa_firepower, firepower);

    let mut later = observation.clone();
    later.tick += 12;
    later.enemy_units.clear();
    let index = usize::try_from(interceptor_tile.y * later.map_width + interceptor_tile.x)
        .expect("fixture index");
    later.visible[index] = false;
    intelligence.update(&later);
    let current_target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.id == target.id)
        .expect("target remains current")
        .clone();
    assert!(
        derive(
            &profile(50, 50),
            &later,
            &intelligence,
            &current_target,
            &[],
            2_500,
        )
        .is_ok(),
        "remembered airborne AA is risk evidence, not a current untargetable blocker"
    );
}

#[test]
fn minimum_package_has_exact_scrap_deadline_and_slot_boundaries() {
    let exact_cost = minimum_connected_cost(Faction::Ferrous);
    let mut exact = observation(exact_cost);
    add_complete_tech(&mut exact);
    let (intelligence, target) = intelligence_with_target(&mut exact, 0);

    let package = derive(&profile(50, 50), &exact, &intelligence, &target, &[], 500)
        .expect("the exact lower-tier minimum is feasible");
    assert_eq!(
        package.recon.iter().map(|item| item.count).sum::<usize>(),
        1
    );
    assert_eq!(
        package
            .suppression
            .iter()
            .map(|item| item.count)
            .sum::<usize>(),
        1
    );
    assert_eq!(
        package.strike.iter().map(|item| item.count).sum::<usize>(),
        1
    );
    assert!(
        package.chosen_capability.strike < package.useful_capability.strike,
        "inability to fund marginal scale must not reject a feasible minimum"
    );

    let mut one_scrap_short = exact.clone();
    one_scrap_short.scrap = exact_cost - 1;
    assert!(
        derive(
            &profile(50, 50),
            &one_scrap_short,
            &intelligence,
            &target,
            &[],
            500,
        )
        .is_err()
    );
    assert!(
        derive(
            &profile(50, 50),
            &exact,
            &intelligence,
            &target,
            &[],
            exact.tick + Tick::from(UnitKind::Bombard.stats().train_ticks) - 2,
        )
        .is_err(),
        "the Bombard cannot finish one tick before its conservative ready boundary"
    );

    let mut no_suppression_slot = exact.clone();
    let fabricator = no_suppression_slot
        .my_buildings
        .iter()
        .position(|building| building.kind == BuildingKind::Fabricator)
        .expect("fabricator");
    no_suppression_slot.my_queues[fabricator] = vec![UnitKind::Lancer; QUEUE_CAP];
    assert!(
        derive(
            &profile(50, 50),
            &no_suppression_slot,
            &intelligence,
            &target,
            &[],
            500,
        )
        .is_err(),
        "a full Fabricator queue is not a production slot"
    );
}

#[test]
fn a_full_queue_can_supply_the_minimum_after_it_drains() {
    let mut observation = observation(10_000);
    add_producer(
        &mut observation,
        10,
        BuildingKind::Foundry,
        TilePos::new(2, 2),
        Vec::new(),
    );
    add_producer(
        &mut observation,
        11,
        BuildingKind::Fabricator,
        TilePos::new(5, 2),
        vec![UnitKind::Lancer; QUEUE_CAP],
    );
    add_producer(
        &mut observation,
        12,
        BuildingKind::Airworks,
        TilePos::new(8, 2),
        Vec::new(),
    );
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);
    let queue_ticks = u64::try_from(QUEUE_CAP).expect("small queue")
        * Tick::from(UnitKind::Lancer.stats().train_ticks);
    let ready_after =
        observation.tick + queue_ticks + Tick::from(UnitKind::Bombard.stats().train_ticks);

    assert!(matches!(
        derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            ready_after - 1,
        ),
        Err(ForcePackageRejection::PreparationWindowTooShort {
            family: ForceFamily::Suppression,
            ..
        })
    ));

    let package = derive(
        &profile(50, 50),
        &observation,
        &intelligence,
        &target,
        &[],
        ready_after,
    )
    .expect("the lane can refill after its paid queue drains");
    assert_eq!(demand(&package.suppression, UnitKind::Bombard), 1);
}

#[test]
fn only_completed_income_can_make_a_forecast_funded_package_feasible() {
    let exact_cost = minimum_connected_cost(Faction::Ferrous);
    let mut unfinished = observation(exact_cost - 1);
    add_complete_tech(&mut unfinished);
    add_owned_building(
        &mut unfinished,
        20,
        BuildingKind::Reclaimer,
        TilePos::new(14, 2),
        false,
    );
    let (intelligence, target) = intelligence_with_target(&mut unfinished, 0);
    assert!(
        derive(
            &profile(50, 50),
            &unfinished,
            &intelligence,
            &target,
            &[],
            500,
        )
        .is_err()
    );

    let mut completed = unfinished.clone();
    completed
        .my_buildings
        .iter_mut()
        .find(|building| building.id == BuildingId(20))
        .expect("reclaimer")
        .built = true;
    let package = derive(
        &profile(50, 50),
        &completed,
        &intelligence,
        &target,
        &[],
        500,
    )
    .expect("completed recurring income funds the final scrap in time");
    assert!(package.forecast_scrap > 0);
}

fn add_completed_income_and_live_support(observation: &mut Observation) {
    add_complete_tech(observation);
    add_owned_building(
        observation,
        20,
        BuildingKind::Reclaimer,
        TilePos::new(14, 2),
        true,
    );
    let observation = &mut **observation;
    observation.my_units.extend([
        unit(
            40,
            observation.me.0,
            Role::Scout.unit_for(observation.faction),
            TilePos::new(7, 7),
        ),
        unit(41, observation.me.0, UnitKind::Bombard, TilePos::new(8, 7)),
    ]);
}

#[test]
fn forecast_income_waits_for_the_next_bot_decision_before_training() {
    let strike = Role::AirGround.unit_for(Faction::Ferrous);
    let mut observation = observation(strike.stats().cost - 1);
    add_completed_income_and_live_support(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);
    let income_period = oxide_sim::stats::RECLAIMER_PERIOD;
    let payment_tick = observation.tick.div_ceil(income_period) * income_period;
    let next_decision = next_decision_tick_after(payment_tick, TEST_DECISION_CADENCE)
        .expect("the fixture has a following cadence");
    let falsely_ready_if_command_preceded_income =
        payment_tick + Tick::from(strike.stats().train_ticks);
    let first_observable = next_decision + Tick::from(strike.stats().train_ticks);

    assert!(
        derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            falsely_ready_if_command_preceded_income,
        )
        .is_err(),
        "income paid after commands on P cannot fund a Train command on P"
    );
    assert!(
        derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            first_observable,
        )
        .is_ok(),
        "the provider can first appear after training starts on the next decision cadence"
    );
}

#[test]
fn forecast_funded_training_can_fit_prime_cadence_but_not_a_slower_cadence() {
    let strike = Role::AirGround.unit_for(Faction::Ferrous);
    let mut observation = observation(strike.stats().cost - 1);
    add_completed_income_and_live_support(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);
    let resources = ResourceSnapshot::from_observation(&observation);
    let access = all_producers(&resources);
    let payment_tick = observation
        .tick
        .div_ceil(oxide_sim::stats::RECLAIMER_PERIOD)
        * oxide_sim::stats::RECLAIMER_PERIOD;
    let prime_cadence =
        crate::difficulty::DifficultyTuning::for_level(BotDifficulty::Prime).cadence;
    assert_eq!(prime_cadence, 12);
    let deadline = next_decision_tick_after(payment_tick, prime_cadence)
        .expect("the fixture has a Prime command cadence")
        + Tick::from(strike.stats().train_ticks);

    let prime = derive_connected_force_package(
        &profile(50, 50),
        &observation,
        &intelligence,
        &target,
        ProductionEvidence::with_planning(&resources, &access, Some(&PlanningWork::default())),
        &[],
        PreparationConstraints {
            deadline,
            decision_cadence: prime_cadence,
            protected_forecast_scrap: 0,
        },
    );
    assert!(
        prime.is_ok(),
        "Prime can issue the forecast-funded Train command soon enough: {prime:?}"
    );

    let slower = derive_connected_force_package(
        &profile(50, 50),
        &observation,
        &intelligence,
        &target,
        ProductionEvidence::with_planning(&resources, &access, Some(&PlanningWork::default())),
        &[],
        PreparationConstraints {
            deadline,
            decision_cadence: 24,
            protected_forecast_scrap: 0,
        },
    );
    assert_eq!(
        slower,
        Err(ForcePackageRejection::PreparationWindowTooShort {
            family: ForceFamily::Strike,
            observed_at: observation.tick,
            deadline,
        }),
        "a 24-tick controller cannot spend that same payment before the fixed deadline"
    );
}

#[test]
fn older_forecast_shortfall_owns_income_before_a_new_package() {
    let strike = Role::AirGround.unit_for(Faction::Ferrous);
    let mut observation = observation(strike.stats().cost - 1);
    add_completed_income_and_live_support(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);
    let income_period = oxide_sim::stats::RECLAIMER_PERIOD;
    let first_payment = observation.tick.div_ceil(income_period) * income_period;
    let first_command = next_decision_tick_after(first_payment, TEST_DECISION_CADENCE)
        .expect("first command cadence");
    let first_deadline = first_command + Tick::from(strike.stats().train_ticks);

    assert!(
        derive_with_forecast_reserve(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            first_deadline,
            0,
        )
        .is_ok(),
        "the first forecast scrap can fund the package when unclaimed"
    );
    assert!(
        derive_with_forecast_reserve(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            first_deadline,
            1,
        )
        .is_err(),
        "an older one-scrap shortfall owns the first payment and cannot be double-promised"
    );

    let second_payment = first_payment + income_period;
    let second_command = next_decision_tick_after(second_payment, TEST_DECISION_CADENCE)
        .expect("second command cadence");
    let later_deadline = second_command + Tick::from(strike.stats().train_ticks);
    assert!(
        derive_with_forecast_reserve(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            later_deadline,
            1,
        )
        .is_ok(),
        "later income remains available after honoring the older promise"
    );
}

#[test]
fn longer_deadline_never_loses_useful_capability() {
    let mut observation = observation(1_460);
    add_complete_tech(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);
    let resources = ResourceSnapshot::from_observation(&observation);

    let early = derive_connected_force_package(
        &profile(50, 50),
        &observation,
        &intelligence,
        &target,
        ProductionEvidence::with_planning(
            &resources,
            &all_producers(&resources),
            Some(&PlanningWork::default()),
        ),
        &[],
        constraints(500, 0),
    )
    .expect("lower tiers fit the early deadline");
    assert_eq!(demand(&early.suppression, UnitKind::Avalanche), 0);
    assert_eq!(demand(&early.strike, UnitKind::Condor), 0);
    assert!(demand(&early.suppression, UnitKind::Bombard) > 0);
    assert!(demand(&early.strike, UnitKind::Buzzard) > 0);

    let late = derive_connected_force_package(
        &profile(50, 50),
        &observation,
        &intelligence,
        &target,
        ProductionEvidence::with_planning(
            &resources,
            &all_producers(&resources),
            Some(&PlanningWork::default()),
        ),
        &[],
        constraints(2_000, 0),
    )
    .expect("the later deadline retains every earlier feasible portfolio");
    assert!(
        capped_useful_objective(
            &profile(50, 50),
            late.useful_capability,
            late.useful_bombing,
            late.chosen_capability,
            late.chosen_bombing,
        ) >= capped_useful_objective(
            &profile(50, 50),
            early.useful_capability,
            early.useful_bombing,
            early.chosen_capability,
            early.chosen_bombing,
        ),
        "a longer preparation window must not weaken the useful package"
    );
}

#[test]
fn wealth_and_completed_throughput_scale_realized_force_monotonically() {
    let mut constrained = observation(minimum_connected_cost(Faction::Ferrous));
    add_complete_tech(&mut constrained);
    add_valuable_cluster(&mut constrained);
    let (constrained_intelligence, constrained_target) =
        intelligence_with_target(&mut constrained, 0);
    let constrained_package = derive_settled(
        &profile(80, 20),
        &constrained,
        &constrained_intelligence,
        &constrained_target,
        &[],
        2_200,
    )
    .expect("minimum package");

    let mut wealthy = constrained.clone();
    wealthy.scrap = 10_000;
    let wealthy_package = derive_settled(
        &profile(80, 20),
        &wealthy,
        &constrained_intelligence,
        &constrained_target,
        &[],
        2_200,
    )
    .expect("wealthy package");
    let constrained_strike = realized_capability(
        &constrained_package,
        ForceFamily::Strike,
        constrained.faction,
    );
    let wealthy_strike =
        realized_capability(&wealthy_package, ForceFamily::Strike, wealthy.faction);
    assert!(wealthy_strike > constrained_strike);
    assert!(
        realized_capability(&wealthy_package, ForceFamily::Suppression, wealthy.faction,)
            >= realized_capability(
                &constrained_package,
                ForceFamily::Suppression,
                constrained.faction,
            )
    );

    add_producer(
        &mut wealthy,
        14,
        BuildingKind::Airworks,
        TilePos::new(14, 2),
        Vec::new(),
    );
    let additional_throughput = derive_settled(
        &profile(80, 20),
        &wealthy,
        &constrained_intelligence,
        &constrained_target,
        &[],
        2_200,
    )
    .expect("second completed Airworks contributes throughput");
    assert!(
        realized_capability(&additional_throughput, ForceFamily::Strike, wealthy.faction,)
            > wealthy_strike
    );
}

#[test]
fn exact_wealth_boundary_cannot_turn_an_admitted_package_into_rejection() {
    let mut scarce = observation(759);
    add_complete_tech(&mut scarce);
    let (intelligence, target) = intelligence_with_target(&mut scarce, 0);
    let scarce_package = derive(
        &profile(50, 50),
        &scarce,
        &intelligence,
        &target,
        &[],
        2_500,
    )
    .expect("759 scrap can fund the complete lower-tier package");

    let mut one_more = scarce.clone();
    one_more.scrap = 760;
    let one_more_package = derive(
        &profile(50, 50),
        &one_more,
        &intelligence,
        &target,
        &[],
        2_500,
    )
    .expect("making one advanced provider affordable cannot reject the package");

    assert!(
        capped_useful_objective(
            &profile(50, 50),
            one_more_package.useful_capability,
            one_more_package.useful_bombing,
            one_more_package.chosen_capability,
            one_more_package.chosen_bombing,
        ) >= capped_useful_objective(
            &profile(50, 50),
            scarce_package.useful_capability,
            scarce_package.useful_bombing,
            scarce_package.chosen_capability,
            scarce_package.chosen_bombing,
        )
    );
}

#[test]
fn dense_valuable_target_selects_new_bombers_for_both_factions() {
    for faction in [Faction::Ferrous, Faction::Cupric] {
        let mut observation = observation(10_000);
        observation.faction = faction;
        add_complete_tech(&mut observation);
        add_producer(
            &mut observation,
            14,
            BuildingKind::Airworks,
            TilePos::new(14, 2),
            Vec::new(),
        );
        add_valuable_cluster(&mut observation);
        add_dense_ground_targets(&mut observation);
        let (intelligence, target) = intelligence_with_target(&mut observation, 0);

        let package = derive(
            &profile(90, 10),
            &observation,
            &intelligence,
            &target,
            &[],
            5_000,
        )
        .expect("a wealthy full-tech package can exploit visible collateral");

        assert!(package.useful_bombing > 0, "{faction:?}: {package:?}");
        assert!(package.chosen_bombing > 0, "{faction:?}: {package:?}");
        assert!(
            demand(&package.strike, Role::Bomber.unit_for(faction)) > 0,
            "{faction:?}: {package:?}"
        );
        assert!(
            demand(&package.strike, Role::AirGround.unit_for(faction)) > 0,
            "the common direct-strike minimum remains present: {package:?}"
        );
    }
}

#[test]
fn dense_target_falls_back_when_bomber_tech_or_time_is_unavailable() {
    for faction in [Faction::Ferrous, Faction::Cupric] {
        let mut baseline = observation(10_000);
        baseline.faction = faction;
        add_baseline_tech(&mut baseline);
        add_dense_ground_targets(&mut baseline);
        let (baseline_intelligence, baseline_target) = intelligence_with_target(&mut baseline, 0);
        let baseline_package = derive(
            &profile(90, 10),
            &baseline,
            &baseline_intelligence,
            &baseline_target,
            &[],
            5_000,
        )
        .expect("a dense target does not require advanced bomber tech");
        assert!(baseline_package.useful_bombing > 0);
        assert_eq!(baseline_package.chosen_bombing, 0);
        assert_eq!(
            demand(&baseline_package.strike, Role::Bomber.unit_for(faction)),
            0
        );
        assert!(demand(&baseline_package.strike, Role::AirGround.unit_for(faction)) > 0);

        let mut rushed = observation(10_000);
        rushed.faction = faction;
        add_complete_tech(&mut rushed);
        add_dense_ground_targets(&mut rushed);
        let (rushed_intelligence, rushed_target) = intelligence_with_target(&mut rushed, 0);
        let rushed_package = derive(
            &profile(90, 10),
            &rushed,
            &rushed_intelligence,
            &rushed_target,
            &[],
            500,
        )
        .expect("the short horizon still admits its lower-tier minimum");
        assert!(rushed_package.useful_bombing > 0);
        assert_eq!(rushed_package.chosen_bombing, 0);
        assert_eq!(
            demand(&rushed_package.strike, Role::Bomber.unit_for(faction)),
            0
        );
        assert!(demand(&rushed_package.strike, Role::AirGround.unit_for(faction)) > 0);
    }
}

#[test]
fn air_personality_changes_a_real_competitive_mix_without_gating_bombers() {
    let mut template = observation(10_000);
    add_complete_tech(&mut template);
    add_producer(
        &mut template,
        14,
        BuildingKind::Airworks,
        TilePos::new(14, 2),
        Vec::new(),
    );
    add_valuable_cluster(&mut template);
    add_dense_ground_targets(&mut template);
    let (intelligence, target) = intelligence_with_target(&mut template, 0);
    let restrained_profile = profile(10, 50);

    let restrained_rich = derive(
        &restrained_profile,
        &template,
        &intelligence,
        &target,
        &[],
        5_000,
    )
    .expect("dense evidence keeps bombing available to an air-light personality");
    assert!(
        demand(&restrained_rich.strike, UnitKind::Condor) > 0,
        "personality must not gate the bomber repertoire: {restrained_rich:?}"
    );

    let air_profile = profile(90, 50);
    let mut witness = None;
    let victim_max_hp = UnitKind::Lancer.stats().max_hp;
    for scrap in (1_000..=1_200).step_by(10) {
        for visible_hp in 2..=victim_max_hp.saturating_mul(2) {
            let mut observation = observation(scrap);
            add_complete_tech(&mut observation);
            observation.enemy_units.extend([
                unit(200, 1, UnitKind::Lancer, TilePos::new(19, 20)),
                unit(201, 1, UnitKind::Lancer, TilePos::new(20, 19)),
            ]);
            observation.enemy_units[0].hp = visible_hp.div_ceil(2);
            observation.enemy_units[1].hp = visible_hp / 2;
            let (intelligence, target) = intelligence_with_target(&mut observation, 0);
            let restrained = derive(
                &restrained_profile,
                &observation,
                &intelligence,
                &target,
                &[],
                5_000,
            )
            .expect("the bounded full-tech fixture remains feasible");
            let air = derive(
                &air_profile,
                &observation,
                &intelligence,
                &target,
                &[],
                5_000,
            )
            .expect("personality cannot change package feasibility");
            let restrained_total = capped_useful_objective(
                &restrained_profile,
                restrained.useful_capability,
                restrained.useful_bombing,
                restrained.chosen_capability,
                restrained.chosen_bombing,
            )
            .0;
            let air_total = capped_useful_objective(
                &air_profile,
                air.useful_capability,
                air.useful_bombing,
                air.chosen_capability,
                air.chosen_bombing,
            )
            .0;
            if restrained_total == air_total && restrained.strike != air.strike {
                witness = Some((restrained, air, restrained_total, air_total));
                break;
            }
        }
        if witness.is_some() {
            break;
        }
    }

    let (restrained, air, restrained_total, air_total) =
        witness.expect("the real provider economy contains a competitive mixed-air choice");
    assert_eq!(restrained.useful_capability, air.useful_capability);
    assert_eq!(restrained.useful_bombing, air.useful_bombing);
    assert_eq!(restrained_total, air_total);
    assert!(restrained.chosen_capability.strike >= restrained.minimum_capability.strike);
    assert!(air.chosen_capability.strike >= air.minimum_capability.strike);
    assert!(demand(&air.strike, UnitKind::Condor) > demand(&restrained.strike, UnitKind::Condor));
    assert!(demand(&air.strike, UnitKind::Buzzard) < demand(&restrained.strike, UnitKind::Buzzard));
}

#[test]
fn bombing_commitment_is_monotone_with_wealth_and_visible_collateral() {
    for faction in [Faction::Ferrous, Faction::Cupric] {
        let mut moderate = observation(minimum_connected_cost(faction));
        moderate.faction = faction;
        add_complete_tech(&mut moderate);
        add_producer(
            &mut moderate,
            14,
            BuildingKind::Airworks,
            TilePos::new(14, 2),
            Vec::new(),
        );
        add_dense_ground_targets(&mut moderate);
        moderate.enemy_units.truncate(4);
        let (moderate_intelligence, moderate_target) = intelligence_with_target(&mut moderate, 0);
        let scarce = derive(
            &profile(70, 30),
            &moderate,
            &moderate_intelligence,
            &moderate_target,
            &[],
            5_000,
        )
        .expect("the exact common-minimum bank remains admissible");

        let mut wealthy = moderate.clone();
        wealthy.scrap = 10_000;
        let wealthy_package = derive(
            &profile(70, 30),
            &wealthy,
            &moderate_intelligence,
            &moderate_target,
            &[],
            5_000,
        )
        .expect("wealth funds useful bombing without weakening the package");
        assert!(wealthy_package.chosen_bombing >= scarce.chosen_bombing);
        assert!(
            demand(&wealthy_package.strike, Role::Bomber.unit_for(faction))
                >= demand(&scarce.strike, Role::Bomber.unit_for(faction))
        );

        let mut dense = wealthy.clone();
        dense.enemy_units.clear();
        add_dense_ground_targets(&mut dense);
        let (dense_intelligence, dense_target) = intelligence_with_target(&mut dense, 0);
        let dense_package = derive(
            &profile(70, 30),
            &dense,
            &dense_intelligence,
            &dense_target,
            &[],
            5_000,
        )
        .expect("more visible collateral remains an actionable opportunity");
        assert!(dense_package.useful_bombing >= wealthy_package.useful_bombing);
        assert!(dense_package.chosen_bombing >= wealthy_package.chosen_bombing);
        assert!(
            demand(&dense_package.strike, Role::Bomber.unit_for(faction))
                >= demand(&wealthy_package.strike, Role::Bomber.unit_for(faction))
        );
    }
}

#[test]
fn completed_tech_does_not_spend_for_equal_capped_capability() {
    for faction in [Faction::Ferrous, Faction::Cupric] {
        let mut observation = observation(10_000);
        observation.faction = faction;
        add_complete_tech(&mut observation);
        observation.enemy_buildings.push(building(
            100,
            1,
            BuildingKind::Turret,
            TilePos::new(20, 20),
        ));
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.id == Some(BuildingId(100)))
            .expect("current Turret target")
            .clone();

        let package = derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            2_500,
        )
        .expect("the cheap complete package is feasible");

        assert_eq!(package.chosen_capability, package.minimum_capability);
        assert_eq!(package.useful_bombing, 0);
        assert_eq!(package.chosen_bombing, 0);
        assert_eq!(
            package.recon,
            vec![ProviderDemand {
                kind: Role::Scout.unit_for(faction),
                count: 1
            }]
        );
        assert_eq!(
            package.suppression,
            vec![ProviderDemand {
                kind: UnitKind::Bombard,
                count: 1,
            }]
        );
        assert_eq!(
            package.strike,
            vec![ProviderDemand {
                kind: Role::AirGround.unit_for(faction),
                count: 1,
            }]
        );
        assert_eq!(demanded_scrap(&package), minimum_connected_cost(faction));
        assert_eq!(demand(&package.suppression, UnitKind::Avalanche), 0);
        assert_eq!(demand(&package.strike, Role::Bomber.unit_for(faction)), 0);
    }
}

#[test]
fn stronger_aa_cannot_hide_a_better_feasible_baseline_portfolio() {
    let profile = profile(50, 50);
    for (first_flak_hp, expected_suppression_useful) in [(150, 4_050), (177, 4_100)] {
        let (observation, intelligence, target) = baseline_aa_fixture(700, 724, first_flak_hp);
        let package = derive(&profile, &observation, &intelligence, &target, &[], 2_500)
            .expect("the fixed budget admits a complete baseline portfolio");
        assert_eq!(package.useful_capability.strike, 2_050);
        assert_eq!(
            package.useful_capability.suppression,
            expected_suppression_useful
        );
        assert_eq!(demand(&package.suppression, UnitKind::Bombard), 2);
        assert_eq!(demand(&package.strike, UnitKind::Buzzard), 2);
        assert_eq!(demanded_scrap(&package), 700);
        assert_eq!(
            package.chosen_capability,
            NormalizedCapability {
                recon: 1_000,
                suppression: 2_000,
                strike: 2_000,
            }
        );
    }
}

#[test]
fn marginal_deadlines_follow_canonical_funding_for_every_provider() {
    let mut obs = observation(680);
    add_completed_income_and_live_support(&mut obs);
    obs.my_buildings.last_mut().unwrap().tier = 1;
    let resources = ResourceSnapshot::from_observation(&obs);
    let access = all_producers(&resources);
    let profile = profile(10, 90);

    for (deadline, fits) in [(1_608, true), (1_607, false)] {
        let planning = PlanningWork::default();
        let capacity = AllocationCapacity::from_snapshot(&resources, deadline, 12).unwrap();
        let refinement = PackageRefinement {
            planning: &planning,
            capacity: &capacity,
            key: ConnectedOffenseKey {
                objective: BuildingId(99),
                anchor: TilePos::new(25, 25),
            },
        };
        let deferred = Cell::new(false);
        let builder = PackageBuilder {
            faction: obs.faction,
            observed_at: obs.tick,
            deadline,
            decision_cadence: 12,
            current_scrap: obs.scrap,
            protected_forecast_scrap: 0,
            forecast: resources.forecast(),
            resources: &resources,
            committed_scrap: 0,
            production_access: &access,
            refinement: Some(&refinement),
            deferred: &deferred,
            funded_providers: Vec::new(),
            preserved: Vec::new(),
            provider_priority: Vec::new(),
            recon: Vec::new(),
            suppression: Vec::new(),
            strike: Vec::new(),
            capability: NormalizedCapability {
                recon: 0,
                suppression: 0,
                strike: 0,
            },
            bombing_capability_per_provider: 0,
            bombing: 0,
        };
        let prior = builder
            .add_new_kind_variants(
                ForceFamily::Strike,
                ProviderPriority::Marginal,
                UnitKind::Buzzard,
            )
            .unwrap()
            .remove(0);
        let mut successors =
            prior.provider_successors(ForceFamily::Suppression, ProviderPriority::Marginal);
        let index = successors
            .iter()
            .position(|successor| {
                successor
                    .funded_providers
                    .last()
                    .is_some_and(|provider| provider.kind == UnitKind::Avalanche)
            })
            .expect("temporary append order must not discard the preferred provider");
        let mut successor = successors.remove(index);
        assert_eq!(successor.canonicalize_funding(&profile), fits);
        if fits {
            assert_eq!(
                successor.funded_providers,
                vec![
                    FundedProvider {
                        kind: UnitKind::Avalanche,
                        command_tick: 228
                    },
                    FundedProvider {
                        kind: UnitKind::Buzzard,
                        command_tick: 1_428
                    },
                ]
            );
            assert_eq!(
                successor.provider_refinement(&successor.funded_providers),
                Progress::Ready(())
            );
        }
    }
}

#[test]
fn late_funded_preference_cannot_hide_a_feasible_composition() {
    for tick in [24, 120] {
        let (mut obs, _, _) = baseline_aa_fixture(578, 705, 149);
        obs.tick = tick;
        let mut intelligence = StrategicIntelligence::default();
        intelligence.update(&obs);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.id == Some(BuildingId(100)))
            .unwrap();
        let resources = ResourceSnapshot::from_observation(&obs);
        let access = all_producers(&resources);
        let planning = PlanningWork::default();
        let options = derive_connected_force_package_options_for_cluster(
            &profile(73, 41),
            &obs,
            &intelligence,
            ConnectedTargetEvidence {
                primary: target,
                cluster: &[target],
                committed: None,
            },
            ProductionEvidence::with_planning(&resources, &access, Some(&planning)),
            &[],
            constraints(2_500, 0),
        )
        .unwrap();
        assert!(!options.refinement_pending);
        let package = options.into_largest();
        assert_eq!(demand(&package.suppression, UnitKind::Bombard), 1);
        assert_eq!(demand(&package.strike, UnitKind::Buzzard), 2);
        assert!(funded_providers_fit(
            &resources,
            &package.funded_providers,
            2_500,
            &access
        ));
    }
}

#[test]
fn baseline_portfolio_matches_an_independent_count_oracle_at_demand_boundaries() {
    let profile = profile(73, 41);
    for target_hp in [352, 353, 705, 706, 724] {
        for first_flak_hp in [149, 150, 176, 177] {
            for scrap in 380..=900 {
                let (observation, intelligence, target) =
                    baseline_aa_fixture(scrap, target_hp, first_flak_hp);
                let package = derive(&profile, &observation, &intelligence, &target, &[], 2_500)
                    .expect("the sweep starts at the exact common-minimum budget");
                let (suppression_count, strike_count, oracle_score) = baseline_portfolio_oracle(
                    &profile,
                    package.useful_capability,
                    scrap,
                    observation.faction,
                );
                assert_eq!(
                    (
                        demand(&package.suppression, UnitKind::Bombard),
                        demand(&package.strike, UnitKind::Buzzard),
                    ),
                    (suppression_count, strike_count),
                    "target hp {target_hp}, flak hp {first_flak_hp}, scrap {scrap}"
                );
                let chosen_score = (
                    package
                        .chosen_capability
                        .recon
                        .min(package.useful_capability.recon)
                        .saturating_add(
                            package
                                .chosen_capability
                                .suppression
                                .min(package.useful_capability.suppression),
                        )
                        .saturating_add(
                            package
                                .chosen_capability
                                .strike
                                .min(package.useful_capability.strike),
                        ),
                    u128::from(
                        package
                            .chosen_capability
                            .suppression
                            .min(package.useful_capability.suppression),
                    )
                    .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.siege)))
                    .saturating_add(
                        u128::from(
                            package
                                .chosen_capability
                                .strike
                                .min(package.useful_capability.strike),
                        )
                        .saturating_mul(u128::from(100_u16 + u16::from(profile.traits.air))),
                    ),
                    Reverse(demanded_scrap(&package)),
                    package
                        .chosen_capability
                        .recon
                        .saturating_add(package.chosen_capability.suppression)
                        .saturating_add(package.chosen_capability.strike),
                );
                assert_eq!(
                    chosen_score, oracle_score,
                    "target hp {target_hp}, flak hp {first_flak_hp}, scrap {scrap}"
                );
            }
        }
    }
}

#[test]
fn unaffordable_preferred_family_does_not_block_affordable_useful_force() {
    let mut observation = observation(500);
    add_complete_tech(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 1);
    let package = derive(
        &profile(10, 90),
        &observation,
        &intelligence,
        &target,
        &[],
        2_500,
    )
    .expect("the complete minimum plus one affordable marginal provider fits");

    assert_eq!(package.chosen_capability.suppression, 1_000);
    assert_eq!(package.chosen_capability.strike, 2_000);
    assert_eq!(
        package
            .provider_priority
            .iter()
            .find(|tranche| tranche.priority == ProviderPriority::Marginal)
            .map(|tranche| (tranche.family, tranche.kind)),
        Some((ForceFamily::Strike, UnitKind::Buzzard)),
        "the affordable second-choice family must remain available"
    );
}

#[test]
fn completed_advanced_tech_never_weakens_the_selected_package() {
    let mut baseline = observation(1_200);
    add_baseline_tech(&mut baseline);
    add_valuable_cluster(&mut baseline);
    let (intelligence, target) = intelligence_with_target(&mut baseline, 0);
    let baseline_package = derive(
        &profile(70, 30),
        &baseline,
        &intelligence,
        &target,
        &[],
        2_500,
    )
    .expect("baseline production admits a connected package");

    let mut advanced = baseline.clone();
    add_producer(
        &mut advanced,
        13,
        BuildingKind::Crucible,
        TilePos::new(11, 2),
        Vec::new(),
    );
    let advanced_package = derive(
        &profile(70, 30),
        &advanced,
        &intelligence,
        &target,
        &[],
        2_500,
    )
    .expect("completed advanced tech cannot revoke admission");

    assert!(
        capped_useful_objective(
            &profile(70, 30),
            advanced_package.useful_capability,
            advanced_package.useful_bombing,
            advanced_package.chosen_capability,
            advanced_package.chosen_bombing,
        ) >= capped_useful_objective(
            &profile(70, 30),
            baseline_package.useful_capability,
            baseline_package.useful_bombing,
            baseline_package.chosen_capability,
            baseline_package.chosen_bombing,
        )
    );
}

#[test]
fn package_selection_is_deterministic() {
    let mut observation = observation(1_537);
    add_complete_tech(&mut observation);
    add_valuable_cluster(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 1);
    let expected = derive(
        &profile(73, 41),
        &observation,
        &intelligence,
        &target,
        &[],
        2_317,
    );

    for _ in 0..32 {
        assert_eq!(
            derive(
                &profile(73, 41),
                &observation,
                &intelligence,
                &target,
                &[],
                2_317,
            ),
            expected
        );
    }
}

#[test]
fn package_objective_is_monotone_over_bounded_wealth_horizon_and_tech_sweeps() {
    let profile = profile(73, 41);
    let mut baseline = observation(0);
    add_baseline_tech(&mut baseline);
    add_valuable_cluster(&mut baseline);
    let (intelligence, target) = intelligence_with_target(&mut baseline, 0);
    let mut advanced = baseline.clone();
    add_producer(
        &mut advanced,
        13,
        BuildingKind::Crucible,
        TilePos::new(11, 2),
        Vec::new(),
    );
    let deadlines = [500, 900, 1_300, 1_700, 2_100, 2_500];

    for template in [&baseline, &advanced] {
        for deadline in deadlines {
            let mut previous_objective = None;
            for scrap in 0..=2_500 {
                let mut observation = template.clone();
                observation.scrap = scrap;
                match derive_settled(
                    &profile,
                    &observation,
                    &intelligence,
                    &target,
                    &[],
                    deadline,
                ) {
                    Ok(package) => {
                        let objective = capped_useful_objective(
                            &profile,
                            package.useful_capability,
                            package.useful_bombing,
                            package.chosen_capability,
                            package.chosen_bombing,
                        );
                        if let Some(previous) = previous_objective {
                            assert!(
                                objective >= previous,
                                "scrap {scrap} at deadline {deadline} regressed {previous:?} to {objective:?}"
                            );
                        }
                        previous_objective = Some(objective);
                    }
                    Err(rejection) => assert!(
                        previous_objective.is_none(),
                        "scrap {scrap} at deadline {deadline} revoked admission: {rejection:?}"
                    ),
                }
            }
        }
    }

    for scrap in 0..=2_500 {
        let mut previous_objective = None;
        for deadline in deadlines {
            let mut observation = advanced.clone();
            observation.scrap = scrap;
            match derive_settled(
                &profile,
                &observation,
                &intelligence,
                &target,
                &[],
                deadline,
            ) {
                Ok(package) => {
                    let objective = capped_useful_objective(
                        &profile,
                        package.useful_capability,
                        package.useful_bombing,
                        package.chosen_capability,
                        package.chosen_bombing,
                    );
                    if let Some(previous) = previous_objective {
                        assert!(
                            objective >= previous,
                            "deadline {deadline} with scrap {scrap} regressed {previous:?} to {objective:?}"
                        );
                    }
                    previous_objective = Some(objective);
                }
                Err(rejection) => assert!(
                    previous_objective.is_none(),
                    "deadline {deadline} with scrap {scrap} revoked admission: {rejection:?}"
                ),
            }
        }
    }

    for deadline in deadlines {
        for scrap in 0..=2_500 {
            let mut baseline_observation = baseline.clone();
            baseline_observation.scrap = scrap;
            let Ok(baseline_package) = derive_settled(
                &profile,
                &baseline_observation,
                &intelligence,
                &target,
                &[],
                deadline,
            ) else {
                continue;
            };
            let mut advanced_observation = advanced.clone();
            advanced_observation.scrap = scrap;
            let advanced_package = derive_settled(
                &profile,
                &advanced_observation,
                &intelligence,
                &target,
                &[],
                deadline,
            )
            .unwrap_or_else(|rejection| {
                panic!(
                    "completed tech revoked admission at scrap {scrap}, deadline {deadline}: {rejection:?}"
                )
            });
            let baseline_objective = capped_useful_objective(
                &profile,
                baseline_package.useful_capability,
                baseline_package.useful_bombing,
                baseline_package.chosen_capability,
                baseline_package.chosen_bombing,
            );
            let advanced_objective = capped_useful_objective(
                &profile,
                advanced_package.useful_capability,
                advanced_package.useful_bombing,
                advanced_package.chosen_capability,
                advanced_package.chosen_bombing,
            );
            assert!(
                advanced_objective >= baseline_objective,
                "completed tech regressed scrap {scrap}, deadline {deadline} from {baseline_objective:?} to {advanced_objective:?}"
            );
        }
    }
}

#[test]
fn bounded_wealth_sweeps_cover_personality_and_target_shape_variants() {
    for (air, siege, valuable_cluster, anti_air) in
        [(90, 10, false, 0), (50, 50, true, 0), (10, 90, true, 2)]
    {
        let profile = profile(air, siege);
        let mut template = observation(0);
        add_complete_tech(&mut template);
        if valuable_cluster {
            add_valuable_cluster(&mut template);
        }
        let (intelligence, target) = intelligence_with_target(&mut template, anti_air);

        for deadline in [500, 1_300, 2_500] {
            let mut previous_objective = None;
            for scrap in 0..=2_500 {
                let mut observation = template.clone();
                observation.scrap = scrap;
                match derive_settled(
                    &profile,
                    &observation,
                    &intelligence,
                    &target,
                    &[],
                    deadline,
                ) {
                    Ok(package) => {
                        let objective = capped_useful_objective(
                            &profile,
                            package.useful_capability,
                            package.useful_bombing,
                            package.chosen_capability,
                            package.chosen_bombing,
                        );
                        if let Some(previous) = previous_objective {
                            assert!(
                                objective >= previous,
                                "profile ({air}, {siege}), target ({valuable_cluster}, {anti_air}), scrap {scrap}, deadline {deadline} regressed {previous:?} to {objective:?}"
                            );
                        }
                        previous_objective = Some(objective);
                    }
                    Err(rejection) => assert!(
                        previous_objective.is_none(),
                        "profile ({air}, {siege}), target ({valuable_cluster}, {anti_air}), scrap {scrap}, deadline {deadline} revoked admission: {rejection:?}"
                    ),
                }
            }
        }
    }
}

#[test]
fn one_lane_can_admit_more_than_one_live_queue_of_providers() {
    let mut observation = observation(100_000);
    add_producer(
        &mut observation,
        10,
        BuildingKind::Foundry,
        TilePos::new(2, 2),
        Vec::new(),
    );
    add_producer(
        &mut observation,
        11,
        BuildingKind::Fabricator,
        TilePos::new(5, 2),
        Vec::new(),
    );
    add_producer(
        &mut observation,
        12,
        BuildingKind::Airworks,
        TilePos::new(8, 2),
        Vec::new(),
    );
    let cluster_anchors = (-4_i32..=4).flat_map(|dy| {
        (-4_i32..=4).filter_map(move |dx| {
            (dx.abs().saturating_add(dy.abs()) <= 4 && (dx, dy) != (0, 0))
                .then_some(TilePos::new(20 + dx, 20 + dy))
        })
    });
    for (index, anchor) in cluster_anchors.take(2).enumerate() {
        observation.enemy_buildings.push(building(
            200 + u32::try_from(index).expect("small fixture"),
            1,
            BuildingKind::Foundry,
            anchor,
        ));
    }
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);
    let deadline = 50_000;
    let package = derive_settled(
        &profile(90, 10),
        &observation,
        &intelligence,
        &target,
        &[],
        deadline,
    )
    .expect("a wealthy long-horizon operation can exceed one live queue");
    let buzzards = demand(&package.strike, UnitKind::Buzzard);
    assert!(
        buzzards > QUEUE_CAP,
        "fixture must exercise horizon demand beyond the live queue: {package:?}"
    );
}

#[test]
fn more_valuable_current_targets_never_reduce_strike_demand() {
    let mut single = observation(10_000);
    add_complete_tech(&mut single);
    let (single_intelligence, single_target) = intelligence_with_target(&mut single, 0);
    let single_package = derive(
        &profile(80, 20),
        &single,
        &single_intelligence,
        &single_target,
        &[],
        2_200,
    )
    .expect("single-target package");

    let mut cluster = observation(10_000);
    add_complete_tech(&mut cluster);
    add_valuable_cluster(&mut cluster);
    let (cluster_intelligence, cluster_target) = intelligence_with_target(&mut cluster, 0);
    let cluster_package = derive(
        &profile(80, 20),
        &cluster,
        &cluster_intelligence,
        &cluster_target,
        &[],
        2_200,
    )
    .expect("cluster package");

    assert!(cluster_package.target_value > single_package.target_value);
    assert!(cluster_package.useful_capability.strike > single_package.useful_capability.strike);
    assert!(
        realized_capability(&cluster_package, ForceFamily::Strike, cluster.faction)
            >= realized_capability(&single_package, ForceFamily::Strike, single.faction)
    );
}

#[test]
fn only_current_ground_targetable_aa_adds_suppression_work() {
    let mut undefended = observation(10_000);
    add_complete_tech(&mut undefended);
    let (undefended_intelligence, undefended_target) = intelligence_with_target(&mut undefended, 0);
    let undefended_package = derive(
        &profile(20, 80),
        &undefended,
        &undefended_intelligence,
        &undefended_target,
        &[],
        2_500,
    )
    .expect("undefended package");

    let mut static_defense = observation(10_000);
    add_complete_tech(&mut static_defense);
    let (static_intelligence, static_target) = intelligence_with_target(&mut static_defense, 3);
    let static_package = derive(
        &profile(20, 80),
        &static_defense,
        &static_intelligence,
        &static_target,
        &[],
        2_500,
    )
    .expect("static-AA package");
    assert!(static_package.suppressible_aa_firepower > 0);
    assert!(
        static_package.useful_capability.suppression
            > undefended_package.useful_capability.suppression
    );
    assert!(
        realized_capability(
            &static_package,
            ForceFamily::Suppression,
            static_defense.faction,
        ) > realized_capability(
            &undefended_package,
            ForceFamily::Suppression,
            undefended.faction,
        )
    );

    let mut mobile_defense = observation(10_000);
    add_complete_tech(&mut mobile_defense);
    for (id, kind, tile) in [
        (200, UnitKind::Flakhound, TilePos::new(20, 19)),
        (201, UnitKind::Stinger, TilePos::new(19, 20)),
        (202, UnitKind::Sentinel, TilePos::new(21, 20)),
        (203, UnitKind::Flakhound, TilePos::new(20, 21)),
    ] {
        mobile_defense.enemy_units.push(unit(id, 1, kind, tile));
    }
    let (mobile_intelligence, mobile_target) = intelligence_with_target(&mut mobile_defense, 0);
    let mobile_package = derive(
        &profile(20, 80),
        &mobile_defense,
        &mobile_intelligence,
        &mobile_target,
        &[],
        2_500,
    )
    .expect("mobile-AA package");
    assert!(mobile_package.observed_aa_firepower > 0);
    assert_eq!(
        mobile_package.suppressible_aa_firepower,
        mobile_package.observed_aa_firepower
    );
    assert!(
        mobile_package.useful_capability.suppression
            > undefended_package.useful_capability.suppression
    );
    assert!(
        realized_capability(
            &mobile_package,
            ForceFamily::Suppression,
            mobile_defense.faction,
        ) > realized_capability(
            &undefended_package,
            ForceFamily::Suppression,
            undefended.faction,
        )
    );
    let cluster = current_target_cluster(
        &mobile_intelligence,
        mobile_target.player,
        mobile_target.anchor,
    );
    let defense = current_air_defense(&mobile_intelligence, &cluster);
    let expected_hp = [
        UnitKind::Flakhound,
        UnitKind::Stinger,
        UnitKind::Sentinel,
        UnitKind::Flakhound,
    ]
    .into_iter()
    .map(|kind| u64::from(kind.stats().max_hp))
    .sum::<u64>();
    assert_eq!(defense.suppressible_hp, expected_hp);
    assert_eq!(defense.untargetable_air_firepower, 0);

    let mut unfinished_defense = observation(10_000);
    add_complete_tech(&mut unfinished_defense);
    let mut unfinished = building(201, 1, BuildingKind::FlakTurret, TilePos::new(19, 20));
    unfinished.built = false;
    unfinished_defense.enemy_buildings.push(unfinished);
    let (unfinished_intelligence, unfinished_target) =
        intelligence_with_target(&mut unfinished_defense, 0);
    let unfinished_package = derive(
        &profile(20, 80),
        &unfinished_defense,
        &unfinished_intelligence,
        &unfinished_target,
        &[],
        2_500,
    )
    .expect("unfinished-AA package");
    assert_eq!(unfinished_package.observed_aa_firepower, 0);
    assert_eq!(unfinished_package.suppressible_aa_firepower, 0);

    let mut destroyed_defense = observation(10_000);
    add_complete_tech(&mut destroyed_defense);
    let mut destroyed = building(202, 1, BuildingKind::FlakTurret, TilePos::new(19, 20));
    destroyed.hp = 0;
    destroyed_defense.enemy_buildings.push(destroyed);
    let (destroyed_intelligence, destroyed_target) =
        intelligence_with_target(&mut destroyed_defense, 0);
    let destroyed_package = derive(
        &profile(20, 80),
        &destroyed_defense,
        &destroyed_intelligence,
        &destroyed_target,
        &[],
        2_500,
    )
    .expect("destroyed-AA package");
    assert_eq!(destroyed_package.observed_aa_firepower, 0);
    assert_eq!(destroyed_package.suppressible_aa_firepower, 0);

    let mut remembered_intelligence = static_intelligence;
    let mut after_scout_left = static_defense;
    after_scout_left.tick += 12;
    after_scout_left
        .enemy_buildings
        .retain(|building| building.kind != BuildingKind::FlakTurret);
    for x in 17..=22 {
        for y in 18..=22 {
            let index = usize::try_from(y * after_scout_left.map_width + x).expect("fixture index");
            after_scout_left.visible[index] = false;
        }
    }
    remembered_intelligence.update(&after_scout_left);
    let remembered_target = remembered_intelligence
        .buildings()
        .iter()
        .find(|contact| contact.kind == BuildingKind::Crucible)
        .expect("current target")
        .clone();
    let remembered_package = derive(
        &profile(20, 80),
        &after_scout_left,
        &remembered_intelligence,
        &remembered_target,
        &[],
        2_500,
    )
    .expect("remembered-AA package");
    assert_eq!(remembered_package.observed_aa_firepower, 0);
    assert_eq!(remembered_package.suppressible_aa_firepower, 0);
}

#[test]
fn air_defense_covers_the_complete_target_footprint_and_deduplicates_sources() {
    let mut observation = observation(10_000);
    add_complete_tech(&mut observation);
    let target_anchor = TilePos::new(20, 10);
    let flak_anchor = TilePos::new(26, 11);
    observation.enemy_buildings.extend([
        building(100, 1, BuildingKind::Crucible, target_anchor),
        building(101, 1, BuildingKind::FlakTurret, flak_anchor),
    ]);
    let mut intelligence = StrategicIntelligence::default();
    intelligence.update(&observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.anchor == target_anchor)
        .expect("current target");
    let cluster = vec![target];

    assert!(
        intelligence
            .air_defense_at(target_anchor)
            .sources
            .iter()
            .all(|source| !matches!(
                source.source,
                AirDefenseSource::Building {
                    id: Some(BuildingId(101)),
                    ..
                }
            )),
        "the regression depends on the anchor itself falling outside flak range"
    );
    let assessment = target_cluster_air_defense(&intelligence, &cluster);
    assert!(assessment.all_target_tiles_visible);
    assert_eq!(
        assessment
            .sources
            .iter()
            .filter(|source| matches!(
                source.source,
                AirDefenseSource::Building {
                    id: Some(BuildingId(101)),
                    ..
                }
            ))
            .count(),
        1,
        "one flak source may cover several footprint tiles but must be budgeted once"
    );

    let package = derive(
        &profile(20, 80),
        &observation,
        &intelligence,
        target,
        &[],
        2_500,
    )
    .expect("the visible static defense is suppressible");
    assert!(package.observed_aa_firepower > 0);
    assert_eq!(
        package.observed_aa_firepower,
        package.suppressible_aa_firepower
    );
}

#[test]
fn target_cluster_counts_each_covering_air_defense_source_once() {
    let primary = TilePos::new(20, 20);
    let secondary = TilePos::new(24, 20);
    let shared_source = UnitId(200);
    let secondary_only_source = UnitId(201);
    let mut observation = observation(10_000);
    add_complete_tech(&mut observation);
    observation
        .enemy_buildings
        .push(building(150, 1, BuildingKind::Reclaimer, secondary));
    observation.enemy_units.extend([
        unit(200, 1, UnitKind::Flakhound, TilePos::new(22, 20)),
        unit(201, 1, UnitKind::Flakhound, TilePos::new(28, 20)),
    ]);
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);
    let cluster = current_target_cluster(&intelligence, target.player, target.anchor);
    assert_eq!(cluster.len(), 2);

    let primary_coverage = intelligence.air_defense_at(primary);
    let secondary_coverage = intelligence.air_defense_at(secondary);
    let shared_primary = primary_coverage
        .sources
        .iter()
        .find(|source| {
            matches!(
                source.source,
                AirDefenseSource::Unit { id, .. } if id == shared_source
            )
        })
        .expect("the shared source covers the primary target");
    let shared_secondary = secondary_coverage
        .sources
        .iter()
        .find(|source| {
            matches!(
                source.source,
                AirDefenseSource::Unit { id, .. } if id == shared_source
            )
        })
        .expect("the shared source also covers the secondary target");
    let secondary_only = secondary_coverage
        .sources
        .iter()
        .find(|source| {
            matches!(
                source.source,
                AirDefenseSource::Unit { id, .. } if id == secondary_only_source
            )
        })
        .expect("the second source covers only the secondary target");
    assert!(
        primary_coverage.sources.iter().all(|source| !matches!(
            source.source,
            AirDefenseSource::Unit { id, .. } if id == secondary_only_source
        )),
        "the second source must not cover the primary target"
    );

    let defense = current_air_defense(&intelligence, &cluster);
    assert_eq!(
        defense.total_firepower,
        u64::from(
            shared_primary
                .firepower_per_100_ticks
                .max(shared_secondary.firepower_per_100_ticks)
        ) + u64::from(secondary_only.firepower_per_100_ticks),
        "coverage of both cluster buildings must not count one source twice"
    );
    assert_eq!(
        defense.suppressible_hp,
        u64::from(UnitKind::Flakhound.stats().max_hp) * 2
    );
}

#[test]
fn parallel_advanced_and_lower_tier_lanes_form_a_stronger_mixed_package() {
    let mut observation = observation(10_000);
    add_complete_tech(&mut observation);
    add_producer(
        &mut observation,
        14,
        BuildingKind::Airworks,
        TilePos::new(14, 2),
        Vec::new(),
    );
    add_valuable_cluster(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 3);

    let package = derive(
        &profile(50, 50),
        &observation,
        &intelligence,
        &target,
        &[],
        1_100,
    )
    .expect("wealthy mixed-tier package");
    assert!(demand(&package.suppression, UnitKind::Avalanche) > 0);
    assert!(demand(&package.suppression, UnitKind::Bombard) > 0);
    assert!(demand(&package.strike, UnitKind::Buzzard) > 0);
    assert!(
        package.chosen_capability.suppression
            > suppression_capability(UnitKind::Avalanche, observation.faction,),
        "the complete package benefits from both parallel suppression lanes"
    );
}

#[test]
fn minimum_tranche_precedes_forecast_funded_marginal_suppression() {
    let minimum_scrap = UnitKind::Kestrel
        .stats()
        .cost
        .saturating_add(UnitKind::Avalanche.stats().cost)
        .saturating_add(UnitKind::Condor.stats().cost);
    let mut observation = observation(minimum_scrap);
    add_complete_tech(&mut observation);
    add_owned_building(
        &mut observation,
        14,
        BuildingKind::Reclaimer,
        TilePos::new(14, 2),
        true,
    );
    observation
        .my_buildings
        .last_mut()
        .expect("the refinery was appended")
        .tier = 1;
    observation.enemy_buildings.extend([
        building(100, 1, BuildingKind::Turret, TilePos::new(20, 20)),
        building(101, 1, BuildingKind::FlakTurret, TilePos::new(19, 20)),
        building(102, 1, BuildingKind::FlakTurret, TilePos::new(21, 20)),
    ]);
    let mut intelligence = StrategicIntelligence::default();
    intelligence.update(&observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.kind == BuildingKind::Turret)
        .expect("current target")
        .clone();

    let package = derive(
        &profile(10, 90),
        &observation,
        &intelligence,
        &target,
        &[],
        8_000,
    )
    .expect("forecast income can fund one useful suppression provider");

    let first_marginal = package
        .provider_priority
        .iter()
        .position(|tranche| tranche.priority == ProviderPriority::Marginal)
        .expect("the forecast funds useful marginal suppression");
    assert!(
        package.provider_priority[..first_marginal]
            .iter()
            .all(|tranche| tranche.priority == ProviderPriority::Minimum)
    );
    assert!(
        package.provider_priority[first_marginal..]
            .iter()
            .all(|tranche| tranche.priority == ProviderPriority::Marginal)
    );
    assert_eq!(
        package.provider_priority[..first_marginal]
            .iter()
            .map(|tranche| tranche.family)
            .collect::<Vec<_>>(),
        ForceFamily::ALL
    );
    assert!(
        package.provider_priority[first_marginal..]
            .iter()
            .any(|tranche| tranche.family == ForceFamily::Suppression)
    );
    assert_eq!(package.current_scrap, minimum_scrap);
    assert!(package.forecast_scrap >= UnitKind::Avalanche.stats().cost);
}

#[test]
fn every_marginal_variant_preserves_earlier_provider_identity_and_funding() {
    let mut nontrivial = false;
    for (air, siege) in [(10, 90), (90, 10), (50, 50)] {
        for scrap in [400, 700, 1_000, 1_600] {
            let mut observation = observation(scrap);
            add_baseline_tech(&mut observation);
            add_valuable_cluster(&mut observation);
            let (intelligence, target) = intelligence_with_target(&mut observation, 3);
            let resources = ResourceSnapshot::from_observation(&observation);
            let cluster = intelligence.buildings().iter().collect::<Vec<_>>();
            let Ok(options) = derive_connected_force_package_options_for_cluster(
                &profile(air, siege),
                &observation,
                &intelligence,
                ConnectedTargetEvidence {
                    primary: &target,
                    cluster: &cluster,
                    committed: None,
                },
                ProductionEvidence::with_planning(
                    &resources,
                    &all_producers(&resources),
                    Some(&PlanningWork::default()),
                ),
                &[],
                constraints(5_000, 0),
            ) else {
                continue;
            };
            let variants = std::iter::once(options.minimum)
                .chain(options.marginal)
                .collect::<Vec<_>>();
            nontrivial |= variants.len() >= 4;
            for pair in variants.windows(2) {
                assert!(
                    pair[1]
                        .funded_providers
                        .starts_with(&pair[0].funded_providers),
                    "air {air}, siege {siege}, scrap {scrap}"
                );
                assert!(funded_providers_fit(
                    &resources,
                    &pair[1].funded_providers,
                    5_000,
                    &all_producers(&resources)
                ));
            }
        }
    }
    assert!(nontrivial, "exercise growth beyond one extra provider");
}

#[test]
fn existing_and_paid_lower_tiers_are_not_replaced_by_tier_three() {
    let mut observation = observation(2_000);
    add_complete_tech(&mut observation);
    let airworks = observation
        .my_buildings
        .iter()
        .position(|building| building.kind == BuildingKind::Airworks)
        .expect("airworks");
    observation.my_queues[airworks].push(UnitKind::Buzzard);
    {
        let observation = &mut *observation;
        observation.my_units.push(unit(
            40,
            observation.me.0,
            UnitKind::Bombard,
            TilePos::new(7, 7),
        ));
    }
    let (intelligence, target) = intelligence_with_target(&mut observation, 1);
    let resources = ResourceSnapshot::from_observation(&observation);

    let package = derive_connected_force_package(
        &profile(80, 80),
        &observation,
        &intelligence,
        &target,
        ProductionEvidence::with_planning(
            &resources,
            &all_producers(&resources),
            Some(&PlanningWork::default()),
        ),
        &[],
        constraints(2_500, 0),
    )
    .expect("existing providers plus completed production can assemble");
    assert!(demand(&package.suppression, UnitKind::Bombard) >= 1);
    assert!(demand(&package.strike, UnitKind::Buzzard) >= 1);
    assert_eq!(
        package
            .provider_priority
            .iter()
            .filter(|demand| demand.priority == ProviderPriority::Minimum)
            .map(|demand| (demand.family, demand.kind, demand.count))
            .collect::<Vec<_>>(),
        [
            (ForceFamily::Recon, UnitKind::Kestrel, 1),
            (ForceFamily::Suppression, UnitKind::Bombard, 1),
            (ForceFamily::Strike, UnitKind::Buzzard, 1),
        ],
        "live and already-paid lower-tier providers retain minimum priority"
    );
}

#[test]
fn paid_queue_work_must_spawn_before_the_deadline_observation() {
    let mut observation = observation(0);
    add_complete_tech(&mut observation);
    let airworks = observation
        .my_buildings
        .iter()
        .position(|building| building.kind == BuildingKind::Airworks)
        .expect("airworks");
    observation.my_queues[airworks].push(UnitKind::Buzzard);
    {
        let observation = &mut *observation;
        observation.my_units.push(unit(
            40,
            observation.me.0,
            Role::Scout.unit_for(observation.faction),
            TilePos::new(7, 7),
        ));
    }
    {
        let observation = &mut *observation;
        observation.my_units.push(unit(
            41,
            observation.me.0,
            UnitKind::Bombard,
            TilePos::new(8, 7),
        ));
    }
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);
    let ready_tick = observation.tick + Tick::from(UnitKind::Buzzard.stats().train_ticks) - 1;

    assert!(
        derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            ready_tick - 1,
        )
        .is_err()
    );
    assert!(
        derive(
            &profile(50, 50),
            &observation,
            &intelligence,
            &target,
            &[],
            ready_tick,
        )
        .is_err(),
        "the aircraft spawns after the deadline decision has already run"
    );
    let package = derive(
        &profile(50, 50),
        &observation,
        &intelligence,
        &target,
        &[],
        ready_tick + 1,
    )
    .expect("the paid aircraft exists in the next tick's observation");
    assert_eq!(demand(&package.strike, UnitKind::Buzzard), 1);
}

#[test]
fn blocked_ground_egress_does_not_promise_a_paid_provider() {
    let mut open = observation(0);
    add_complete_tech(&mut open);
    let fabricator = open
        .my_buildings
        .iter()
        .position(|building| building.kind == BuildingKind::Fabricator)
        .expect("fabricator");
    open.my_queues[fabricator].push(UnitKind::Bombard);
    {
        let open = &mut *open;
        open.my_units.push(unit(
            40,
            open.me.0,
            Role::Scout.unit_for(open.faction),
            TilePos::new(7, 7),
        ));
    }
    {
        let open = &mut *open;
        open.my_units.push(unit(
            41,
            open.me.0,
            Role::AirGround.unit_for(open.faction),
            TilePos::new(8, 7),
        ));
    }
    let (intelligence, target) = intelligence_with_target(&mut open, 0);
    let ready_tick = open.tick + Tick::from(UnitKind::Bombard.stats().train_ticks) - 1;
    assert!(
        derive(
            &profile(50, 50),
            &open,
            &intelligence,
            &target,
            &[],
            ready_tick + 1,
        )
        .is_ok(),
        "an open paid Bombard is credible"
    );

    let mut blocked = open.clone();
    let producer = blocked.my_buildings[fabricator].clone();
    blocked.known_scrap.extend(
        oxide_sim::geometry::rect_adjacent_tiles(
            producer.anchor,
            producer.kind.tier_stats(producer.tier).size,
        )
        .map(|tile| (tile, 1)),
    );
    blocked
        .known_scrap
        .sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
    assert!(
        derive(
            &profile(50, 50),
            &blocked,
            &intelligence,
            &target,
            &[],
            ready_tick + 1,
        )
        .is_err(),
        "paid ground work with no observed spawn tile has no finite ready promise"
    );
}

#[test]
fn equivalent_portfolios_receive_canonical_personality_order() {
    let mut observation = observation(2_160);
    add_complete_tech(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 2);
    let resources = ResourceSnapshot::from_observation(&observation);

    let air = derive_connected_force_package(
        &profile(90, 10),
        &observation,
        &intelligence,
        &target,
        ProductionEvidence::with_planning(
            &resources,
            &all_producers(&resources),
            Some(&PlanningWork::default()),
        ),
        &[],
        constraints(2_500, 0),
    )
    .expect("air package");
    let siege = derive_connected_force_package(
        &profile(10, 90),
        &observation,
        &intelligence,
        &target,
        ProductionEvidence::with_planning(
            &resources,
            &all_producers(&resources),
            Some(&PlanningWork::default()),
        ),
        &[],
        constraints(2_500, 0),
    )
    .expect("siege package");

    assert_eq!(air.minimum_capability, siege.minimum_capability);
    assert_eq!(air.recon, siege.recon);
    assert_eq!(air.suppression, siege.suppression);
    assert_eq!(air.strike, siege.strike);
    assert_ne!(air.provider_priority, siege.provider_priority);
    assert_eq!(
        air.provider_priority
            .iter()
            .find(|demand| demand.priority == ProviderPriority::Marginal)
            .map(|demand| demand.family),
        Some(ForceFamily::Strike)
    );
    assert_eq!(
        siege
            .provider_priority
            .iter()
            .find(|demand| demand.priority == ProviderPriority::Marginal)
            .map(|demand| demand.family),
        Some(ForceFamily::Suppression)
    );
}

#[test]
fn every_difficulty_stance_and_personality_retains_the_minimum_repertoire() {
    let exact_cost = minimum_connected_cost(Faction::Ferrous);
    let mut observation = observation(exact_cost);
    add_complete_tech(&mut observation);
    let (intelligence, target) = intelligence_with_target(&mut observation, 0);

    for difficulty in BotDifficulty::ALL {
        for stance in BotStance::ALL {
            for seed in [0, 1, u64::MAX] {
                let profile =
                    ResolvedProfile::resolve(BotConfig::scripted(difficulty, stance, seed));
                let package = derive(&profile, &observation, &intelligence, &target, &[], 500)
                    .unwrap_or_else(|_rejection| {
                        panic!("{difficulty}/{stance}/{seed} lost a provider family")
                    });
                assert!(!package.recon.is_empty());
                assert!(!package.suppression.is_empty());
                assert!(!package.strike.is_empty());
                assert!(
                    realized_capability(&package, ForceFamily::Recon, observation.faction)
                        >= package.minimum_capability.recon
                );
                assert!(
                    realized_capability(&package, ForceFamily::Suppression, observation.faction,)
                        >= package.minimum_capability.suppression
                );
                assert!(
                    realized_capability(&package, ForceFamily::Strike, observation.faction)
                        >= package.minimum_capability.strike
                );
            }
        }
    }
}

fn rich_parallel_suppression_fixture(staggered: bool) -> Observation {
    let mut observation = observation(1_000_000);
    add_producer(
        &mut observation,
        10,
        BuildingKind::Foundry,
        TilePos::new(2, 2),
        Vec::new(),
    );
    for index in 0..12_u32 {
        add_producer(
            &mut observation,
            20 + index,
            BuildingKind::Fabricator,
            TilePos::new(2 + 3 * (index % 8) as i32, 5 + 3 * (index / 8) as i32),
            staggered
                .then_some(vec![UnitKind::Lancer])
                .unwrap_or_default(),
        );
    }
    add_producer(
        &mut observation,
        40,
        BuildingKind::Airworks,
        TilePos::new(2, 14),
        Vec::new(),
    );
    if staggered {
        observation.my_queue_progress = vec![0; observation.my_buildings.len()];
        {
            let observation = &mut *observation;
            for (building, progress) in observation
                .my_buildings
                .iter()
                .zip(&mut observation.my_queue_progress)
            {
                if building.kind == BuildingKind::Fabricator {
                    *progress = (building.id.0 - 20) * 10;
                }
            }
        }
    }
    observation
        .enemy_buildings
        .push(building(200, 1, BuildingKind::Turret, TilePos::new(20, 20)));
    let anti_air_tiles = [
        TilePos::new(20, 15),
        TilePos::new(17, 17),
        TilePos::new(18, 17),
        TilePos::new(19, 17),
        TilePos::new(20, 17),
        TilePos::new(21, 17),
        TilePos::new(22, 17),
        TilePos::new(23, 17),
        TilePos::new(17, 18),
        TilePos::new(18, 18),
        TilePos::new(19, 18),
        TilePos::new(20, 18),
    ];
    for (index, tile) in anti_air_tiles.into_iter().enumerate() {
        observation.enemy_units.push(unit(
            300 + u32::try_from(index).expect("small fixture"),
            1,
            UnitKind::Flakhound,
            tile,
        ));
    }
    observation
}

fn derive_rich_parallel_suppression(staggered: bool) -> ConnectedForcePackage {
    let observation = rich_parallel_suppression_fixture(staggered);
    let mut intelligence = StrategicIntelligence::default();
    intelligence.update(&observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.id == Some(BuildingId(200)))
        .expect("current target")
        .clone();
    derive_settled(
        &profile(50, 50),
        &observation,
        &intelligence,
        &target,
        &[],
        2_500,
    )
    .expect("the rich late-game package fits its fixed deadline")
}

#[test]
fn dense_portfolio_reaches_a_proved_alternative_across_checkpoint_boundaries() {
    for rolling in [false, true] {
        let mut obs = observation(100_000);
        add_producer(
            &mut obs,
            10,
            BuildingKind::Foundry,
            TilePos::new(2, 2),
            vec![],
        );
        add_producer(
            &mut obs,
            11,
            BuildingKind::Crucible,
            TilePos::new(8, 2),
            vec![],
        );
        for i in 0..8 {
            add_producer(
                &mut obs,
                20 + i,
                BuildingKind::Fabricator,
                TilePos::new(2 + 3 * i as i32, 6),
                vec![],
            );
        }
        add_producer(
            &mut obs,
            80,
            BuildingKind::Airworks,
            TilePos::new(2, 20),
            vec![],
        );
        let (mut intel, _) = intelligence_with_target(&mut obs, 4);
        for i in 0..24 {
            obs.enemy_units.push(unit(
                300 + i,
                1,
                UnitKind::Flakhound,
                TilePos::new(18 + (i % 8) as i32, 16 + (i / 8) as i32),
            ));
        }
        let planning = PlanningWork::default();
        let mut restored = planning.clone();
        let mut ready = None;
        for decision in 0..32 {
            obs.tick = 24 + decision * 12;
            intel.update(&obs);
            let target = intel
                .buildings()
                .iter()
                .find(|b| b.id == Some(BuildingId(100)))
                .unwrap();
            let cluster: Vec<_> = intel.buildings().iter().collect();
            let resources = ResourceSnapshot::from_observation(&obs);
            let access = all_producers(&resources);
            let derive = |work| {
                derive_connected_force_package_options_for_cluster(
                    &profile(10, 90),
                    &obs,
                    &intel,
                    ConnectedTargetEvidence {
                        primary: target,
                        cluster: &cluster,
                        committed: None,
                    },
                    ProductionEvidence::with_planning(&resources, &access, Some(work)),
                    &[],
                    constraints(if rolling { obs.tick + 1320 } else { 1344 }, 0),
                )
            };
            let options = derive(&planning);
            assert_eq!(options, derive(&restored));
            assert_eq!(planning, restored);
            assert!(planning.spent() <= 128_000);
            let mut bytes = Vec::new();
            ciborium::into_writer(&restored, &mut bytes).unwrap();
            restored = ciborium::from_reader(bytes.as_slice()).unwrap();
            if options
                .as_ref()
                .is_ok_and(|options| !options.refinement_pending && !options.marginal.is_empty())
            {
                ready = Some(decision);
                break;
            }
        }
        assert!(
            ready.is_some(),
            "rolling={rolling}: feasible alternatives were starved"
        );
    }
}

#[test]
fn large_feasible_roster_finishes_within_the_production_work_limit() {
    for staggered in [false, true] {
        let obs = rich_parallel_suppression_fixture(staggered);
        let resources = ResourceSnapshot::from_observation(&obs);
        let access = all_producers(&resources);
        let providers: Vec<_> = [UnitKind::Kestrel, UnitKind::Bombard, UnitKind::Buzzard]
            .into_iter()
            .chain(core::iter::repeat_n(UnitKind::Bombard, 11))
            .map(|kind| FundedProvider {
                kind,
                command_tick: obs.tick,
            })
            .collect();
        assert!(funded_providers_fit(&resources, &providers, 2_500, &access));
        let capacity =
            AllocationCapacity::from_snapshot(&resources, 2_500, TEST_DECISION_CADENCE).unwrap();
        let planning = PlanningWork::default();
        let refinement = PackageRefinement {
            planning: &planning,
            capacity: &capacity,
            key: ConnectedOffenseKey {
                objective: BuildingId(200),
                anchor: TilePos::new(20, 20),
            },
        };
        let mut result = Progress::Deferred;
        for _ in 0..4 {
            result = refinement.refine(&resources, &access, &providers, 2_500);
            if result != Progress::Deferred {
                break;
            }
        }
        assert_eq!(
            result,
            Progress::Ready(()),
            "staggered={staggered}, work={}",
            planning.spent()
        );
    }
}

#[test]
fn equivalent_late_game_lanes_do_not_create_a_permutation_search() {
    let first = derive_rich_parallel_suppression(false);
    let second = derive_rich_parallel_suppression(false);

    assert_eq!(first, second);
    assert_eq!(demand(&first.suppression, UnitKind::Bombard), 12);
    assert!(first.chosen_capability.suppression >= first.useful_capability.suppression);
}

#[test]
fn staggered_late_game_lanes_do_not_create_an_assignment_search() {
    let first = derive_rich_parallel_suppression(true);
    let second = derive_rich_parallel_suppression(true);

    assert_eq!(first, second);
    assert_eq!(demand(&first.suppression, UnitKind::Bombard), 12);
    assert!(first.chosen_capability.suppression >= first.useful_capability.suppression);
}

#[test]
fn selected_funding_order_matches_canonical_lowering_priority() {
    let mut observation = observation(700);
    add_producer(
        &mut observation,
        10,
        BuildingKind::Foundry,
        TilePos::new(2, 2),
        Vec::new(),
    );
    add_producer(
        &mut observation,
        12,
        BuildingKind::Airworks,
        TilePos::new(8, 2),
        Vec::new(),
    );
    add_producer(
        &mut observation,
        13,
        BuildingKind::Crucible,
        TilePos::new(11, 2),
        Vec::new(),
    );
    let airworks = observation
        .my_buildings
        .iter()
        .position(|building| building.kind == BuildingKind::Airworks)
        .expect("airworks");
    observation.my_queues[airworks] = vec![UnitKind::Skyhook, UnitKind::Skyhook, UnitKind::Talon];
    add_owned_building(
        &mut observation,
        20,
        BuildingKind::Reclaimer,
        TilePos::new(14, 2),
        true,
    );
    observation
        .my_buildings
        .last_mut()
        .expect("the refinery was appended")
        .tier = 1;
    {
        let observation = &mut *observation;
        observation.my_units.extend([
            unit(40, observation.me.0, UnitKind::Kestrel, TilePos::new(7, 7)),
            unit(41, observation.me.0, UnitKind::Bombard, TilePos::new(8, 7)),
            unit(42, observation.me.0, UnitKind::Buzzard, TilePos::new(9, 7)),
        ]);
    }
    observation.enemy_buildings.push(building(
        100,
        1,
        BuildingKind::Fabricator,
        TilePos::new(20, 20),
    ));
    observation.enemy_units.extend([
        unit(101, 1, UnitKind::Flakhound, TilePos::new(20, 15)),
        unit(102, 1, UnitKind::Flakhound, TilePos::new(19, 17)),
    ]);
    let mut intelligence = StrategicIntelligence::default();
    intelligence.update(&observation);
    let target = intelligence
        .buildings()
        .iter()
        .find(|contact| contact.id == Some(BuildingId(100)))
        .expect("current target")
        .clone();

    let package = derive(
        &profile(90, 10),
        &observation,
        &intelligence,
        &target,
        &[],
        1_476,
    )
    .expect("a canonically fundable package exists");

    assert!(package.provider_priority.windows(2).all(|pair| {
        provider_priority_rank(&profile(90, 10), observation.faction, pair[0])
            <= provider_priority_rank(&profile(90, 10), observation.faction, pair[1])
    }));
    assert!(
        package.provider_priority.contains(&ProviderDemandTranche {
            priority: ProviderPriority::Minimum,
            family: ForceFamily::Suppression,
            kind: UnitKind::Bombard,
            count: 1,
        }),
        "the already-owned suppression minimum stays useful"
    );
    let resources = ResourceSnapshot::from_observation(&observation);
    assert!(funded_providers_fit(
        &resources,
        &package.funded_providers,
        1_476,
        &all_producers(&resources),
    ));
    let funding = ProviderFundingEvidence {
        observed_at: observation.tick,
        current_scrap: observation.scrap,
        forecast: resources.forecast(),
        constraints: constraints(1_476, 0),
    };
    let mut spent = 0_u64;
    for provider in &package.funded_providers {
        spent += u64::from(provider.kind.stats().cost);
        assert!(spent <= u64::from(funding.available_scrap_at(provider.command_tick)));
    }

    let mut fully_funded = observation.clone();
    fully_funded.scrap = 820;
    let resources = ResourceSnapshot::from_observation(&fully_funded);
    let fully_funded_package = derive_connected_force_package(
        &profile(90, 10),
        &fully_funded,
        &intelligence,
        &target,
        ProductionEvidence::with_planning(
            &resources,
            &all_producers(&resources),
            Some(&PlanningWork::default()),
        ),
        &[],
        constraints(1_476, 0),
    )
    .expect("the neighboring canonical portfolio is fully funded now");
    assert!(
        fully_funded_package
            .provider_priority
            .contains(&ProviderDemandTranche {
                priority: ProviderPriority::Marginal,
                family: ForceFamily::Suppression,
                kind: UnitKind::Avalanche,
                count: 1,
            })
    );
}

fn brute_funded_lane_schedule_fits(
    lanes: &[FundedLane],
    providers: &[FundedProvider],
    deadline: Tick,
) -> bool {
    fn assign(
        lanes: &[FundedLane],
        providers: &[FundedProvider],
        deadline: Tick,
        provider_index: usize,
        available_ticks: &mut [Tick],
    ) -> bool {
        let Some(provider) = providers.get(provider_index).copied() else {
            return true;
        };
        let duration = Tick::from(provider.kind.stats().train_ticks);
        for lane_index in 0..lanes.len() {
            if lanes[lane_index]
                .eligible_kinds
                .binary_search(&provider.kind)
                .is_err()
            {
                continue;
            }
            let previous = available_ticks[lane_index];
            let Some(next) = previous.max(provider.command_tick).checked_add(duration) else {
                continue;
            };
            if next > deadline {
                continue;
            }
            available_ticks[lane_index] = next;
            if assign(
                lanes,
                providers,
                deadline,
                provider_index + 1,
                available_ticks,
            ) {
                return true;
            }
            available_ticks[lane_index] = previous;
        }
        false
    }

    let mut available_ticks: Vec<_> = lanes.iter().map(|lane| lane.available_tick).collect();
    assign(lanes, providers, deadline, 0, &mut available_ticks)
}

#[test]
fn funded_lane_compression_matches_a_concrete_small_oracle() {
    let kinds = [UnitKind::Kestrel, UnitKind::Buzzard];
    let lane_variants = [
        (100, vec![kinds[0]]),
        (160, vec![kinds[0]]),
        (100, vec![kinds[1]]),
        (160, vec![kinds[1]]),
        (100, kinds.to_vec()),
        (160, kinds.to_vec()),
    ];
    let deadline = 520;
    for lane_count in 1..=3_u32 {
        let topology_count = lane_variants.len().pow(lane_count);
        for mut topology in 0..topology_count {
            let mut lanes = Vec::new();
            for _ in 0..lane_count {
                let variant = topology % lane_variants.len();
                topology /= lane_variants.len();
                let (available_tick, eligible_kinds) = &lane_variants[variant];
                lanes.push(FundedLane {
                    eligible_kinds: eligible_kinds.clone(),
                    available_tick: *available_tick,
                });
            }
            for provider_count in 0..=4_usize {
                for kind_bits in 0..(1_usize << provider_count) {
                    for late_count in 0..=provider_count {
                        let providers: Vec<_> = (0..provider_count)
                            .map(|index| FundedProvider {
                                kind: kinds[(kind_bits >> index) & 1],
                                command_tick: if index < provider_count - late_count {
                                    100
                                } else {
                                    220
                                },
                            })
                            .collect();
                        let expected =
                            brute_funded_lane_schedule_fits(&lanes, &providers, deadline);
                        let actual = funded_lane_schedule_fits(lanes.clone(), &providers, deadline);
                        assert_eq!(actual, expected, "lanes={lanes:?}, providers={providers:?}");
                    }
                }
            }
        }
    }
}
