use super::*;
use crate::orient::Orientation;
use crate::resources::ReservedProducerJob;
use crate::test_support::operations::*;
use crate::test_support::{building_spec, home_foundry};

#[test]
fn fresh_island_admission_preserves_future_lift_work_on_the_only_airworks() {
    let mut scenario = foundry_saving_lift_competition_scenario(50_000);
    scenario.name = "fresh island admission preserves future lift production".into();
    let mut state = scenario
        .build()
        .expect("the future-lift island scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);
    let raw = Observation::fog_honest(&state, PlayerId(0));
    let airworks = raw
        .my_buildings
        .iter()
        .find(|building| building.kind == BuildingKind::Airworks)
        .expect("the fixture has one Airworks")
        .id;
    let (planner, result) = admit_island_beside_future_carrier(&scenario, &raw, airworks);
    assert!(
        planner
            .air_operation()
            .is_some_and(|operation| operation.assault_admitted()),
        "the regression must exercise an admitted residual island operation"
    );
    assert!(
        result.decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt { building, .. } if *building == airworks
        )),
        "an immediate island append must not move the accepted future carrier: {:?}",
        result.decision.intents
    );
}

#[test]
fn fresh_island_admission_uses_an_airworks_disjoint_from_future_lift_work() {
    let mut scenario = foundry_saving_lift_competition_scenario(50_000);
    scenario.name = "fresh island admission uses a disjoint Airworks".into();
    scenario
        .buildings
        .push(building_spec(0, BuildingKind::Airworks, 14, 3));
    let mut state = scenario
        .build()
        .expect("the disjoint-Airworks island scenario builds");
    crate::test_support::set_tick(&mut state, 6_000);
    let raw = Observation::fog_honest(&state, PlayerId(0));
    let airworks = raw
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Airworks)
        .map(|building| building.id)
        .collect::<Vec<_>>();
    assert_eq!(airworks.len(), 2);
    let reserved = airworks[0];
    let (_, result) = admit_island_beside_future_carrier(&scenario, &raw, reserved);
    let residual_airwork = result
        .decision
        .intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::TrainAt { building, kind }
                if airworks.contains(building) && *kind != UnitKind::Skyhook =>
            {
                Some((*building, *kind))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        !residual_airwork.is_empty(),
        "the island operation must use the compatible producer instead of being globally blocked: {:?}",
        result.decision.intents
    );
    assert!(
        residual_airwork
            .iter()
            .all(|(producer, _)| *producer != reserved),
        "residual island work must not shift the lift's exact future append: {residual_airwork:?}"
    );
}

fn admit_island_beside_future_carrier(
    scenario: &oxide_sim::Scenario,
    raw: &Observation,
    producer: BuildingId,
) -> (StrategicPlanner, StrategicThinkResult) {
    let home = home_foundry(raw);
    let orientation = Orientation::for_home(raw, home);
    let observed = orientation.observe(raw);
    let public_map = orientation.briefing(
        &PublicMapBriefing::from_scenario(scenario)
            .expect("the island fixture has a public briefing"),
    );
    let mut intelligence = StrategicIntelligence::new();
    intelligence.update(&observed);
    let resources = ResourceSnapshot::from_observation(&observed);
    let deadline = observed.tick.saturating_add(2_400);
    let projection = resources
        .planning_projection(deadline, 12)
        .expect("the future lift horizon is valid");
    let enqueue_at = observed.tick.saturating_add(12);
    let timing = projection
        .producer(producer)
        .expect("the lift owns the sole Airworks")
        .clone()
        .append(UnitKind::Skyhook, enqueue_at)
        .expect("one future carrier fits the empty Airworks");
    let future_lift = ProducerLaneReservations::from_jobs(
        &projection,
        [ReservedProducerJob {
            producer,
            kind: UnitKind::Skyhook,
            enqueued_at: enqueue_at,
            starts_at: timing.starts_at,
            ready_at: timing.ready_at,
            ready_before: deadline,
        }],
    );
    let profile = foundry_competition_profile();
    let mut planner = StrategicPlanner::new();
    let result = planner.think_after_connected_adjudication(
        StrategicThinkContext::new(
            &profile,
            DifficultyTuning::for_level(profile.difficulty),
            &observed,
            &intelligence,
            home,
            StrategicCoordination {
                planning: Some(&crate::planning::PlanningWork::default()),
                enlisted: &[],
                lift_support: None,
                allow_new_operation: true,
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
                public_map: Some(&public_map),
                orientation,
            },
        )
        .with_producer_lanes(&[], &future_lift),
    );
    (planner, result)
}
