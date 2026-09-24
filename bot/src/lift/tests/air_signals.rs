use super::*;
use crate::allocation::lift_air_support as air_support;
use crate::strategy::AirOperationOutcome;
use crate::test_support::operations::*;

#[test]
fn terminal_air_signals_release_or_recover_a_boarding_complete_lift() {
    let (obs, planner, manifest) = boarding_complete_lift();

    let mut released = planner.clone();
    let released_decision = released.think_unrestricted(
        &obs,
        TEST_HOME,
        &[],
        air_support(
            None,
            Some(AirOperationOutcome::Released {
                player: PlayerId(1),
                target: TEST_TARGET,
            }),
        ),
    );
    let released_operation = released.operation().expect("released lift remains active");
    assert_eq!(released_operation.phase, LiftPhase::Landing);
    assert!(released_operation.launched);
    assert!(released_decision.intents.contains(&Intent::Unload {
        transport: manifest.carrier,
        at: manifest.drop,
    }));

    let mut aborted = planner;
    let aborted_decision = aborted.think_unrestricted(
        &obs,
        TEST_HOME,
        &[],
        air_support(
            None,
            Some(AirOperationOutcome::Aborted {
                player: PlayerId(1),
                target: TEST_TARGET,
            }),
        ),
    );
    let aborted_operation = aborted.operation().expect("loaded carrier must recover");
    assert_eq!(aborted_operation.phase, LiftPhase::Recover);
    assert!(!aborted_operation.launched);
    assert!(
        aborted_decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::Unload { at, .. } if *at == manifest.drop
        )),
        "an aborted corridor must not become an independent target-side launch"
    );
}

fn boarding_complete_lift() -> (Observation, LiftPlanner, crate::lift::LiftManifest) {
    let mut obs = test_island_observation();
    obs.my_units.extend(
        (1..=3).map(|id| test_unit(id, UnitKind::Sentinel, TilePos::new(8 + id as i32, 8))),
    );
    obs.my_units
        .push(test_unit(900, UnitKind::Skyhook, TEST_HOME.offset(0, 8)));
    obs.my_units.sort_unstable_by_key(|unit| unit.id);

    let mut planner = LiftPlanner::new();
    planner.think_unrestricted(&obs, TEST_HOME, &[], LiftAirSupport::Independent);
    let manifest = planner
        .operation()
        .expect("the lift enters boarding")
        .manifests[0]
        .clone();
    obs.my_units
        .iter_mut()
        .find(|unit| unit.id == manifest.carrier)
        .expect("the assigned carrier is observable")
        .tile = manifest.pickup;
    obs.tick += 1;
    planner.think_unrestricted(&obs, TEST_HOME, &[], LiftAirSupport::Independent);
    obs.my_units
        .retain(|unit| !manifest.riders.contains(&unit.id));
    obs.my_units
        .iter_mut()
        .find(|unit| unit.id == manifest.carrier)
        .expect("the assigned carrier survives boarding")
        .cargo = 3;
    obs.tick += 1;
    (obs, planner, manifest)
}
