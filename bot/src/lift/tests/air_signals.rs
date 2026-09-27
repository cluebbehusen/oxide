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

#[test]
fn an_emergency_air_recall_with_no_survivors_aborts_the_waiting_lift() {
    use crate::experience::{Outcome, OutcomeReason};
    use crate::strategy::fixtures::CommittedClusterFixture;
    use crate::strategy::{AirOperationPhase, EconomyEmergencyRecovery, StrategicPlanner};
    let (mut obs, mut lifts, manifest) = boarding_complete_lift();
    let mut air = StrategicPlanner::committed_cluster_fixture(CommittedClusterFixture {
        faction: obs.faction,
        primary: (BuildingId(500), BuildingKind::Foundry, TEST_TARGET),
        members: vec![TEST_TARGET],
        phase: AirOperationPhase::Assemble,
        tick: obs.tick,
        scout: UnitId(700),
        artillery: vec![UnitId(701)],
        strike_aircraft: vec![UnitId(702)],
    });
    lifts.think_unrestricted(
        &obs,
        TEST_HOME,
        &[],
        air_support(air.air_operation(), air.terminal_outcome()),
    );
    assert_eq!(
        lifts.operation().unwrap().phase,
        LiftPhase::AwaitSupport,
        "the loaded lift waits on the operation suppressing its target"
    );

    obs.tick += 12;
    let profile = coordinator_profile();
    let recovery = air
        .recover_unpaid_connected_for_economy_emergency(EconomyEmergencyRecovery {
            profile: &profile,
            tuning: DifficultyTuning::for_level(profile.difficulty),
            obs: &obs,
            home: TEST_HOME,
            public_map: None,
            orientation: crate::orient::Orientation::for_home(&obs, TEST_HOME),
            recon_paid_exclusions: &[],
        })
        .expect("an unpaid package yields to the economy emergency");
    assert!(recovery.reservations.is_empty(), "no member survives");

    obs.tick += 12;
    let (work, _) = coordinator_pass(&obs, &mut lifts, &mut air);
    let lift = lifts.operation().expect("the loaded carrier must recover");
    assert_eq!(
        (lift.phase, lift.launched),
        (LiftPhase::Recover, false),
        "the abort reaches the lift instead of leaving it to wait out its grace"
    );
    assert!(
        work.intents.iter().all(|intent| !matches!(
            intent,
            Intent::Unload { at, .. } if *at == manifest.drop
        )),
        "an aborted corridor must not become a target-side launch"
    );
    assert!(
        air.air_operation().is_none(),
        "recovery settles the operation"
    );
    let report = air
        .outcomes()
        .pending
        .last()
        .expect("settlement finishes the operation's episode");
    assert_eq!(
        (report.outcome, report.reason, report.finished_at),
        (Outcome::Invalidated, OutcomeReason::Preempted, obs.tick),
        "an infeasible preparation closes its episode when recovery settles"
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

#[test]
fn a_shared_air_objective_matches_the_enclave_by_owner_and_anchor() {
    let mut planner = recovering_empty_lift();
    planner.support_latched = true;
    let lift = planner.operation().expect("the fixture has a lift").clone();
    let mut air = crate::strategy::AirOperation {
        target_player: lift.target_player,
        target_kind: oxide_sim::stats::BuildingKind::Foundry,
        target: lift.target,
        target_id: Some(BuildingId(lift.target_id.0 + 1)),
        stage: crate::strategy::AirStage::Recon,
        started_at: 0,
        phase_started_at: 0,
        scout: None,
        scout_dispatch: None,
        strike_hold: None,
        artillery_staging: None,
        artillery: Vec::new(),
        strike_aircraft: Vec::new(),
        strike_issued_at: None,
        membership_frozen_at: None,
    };
    assert!(
        planner.shares_air_objective(&air),
        "support directives match by owner and anchor, so credit sharing does too"
    );
    air.target = air.target.offset(1, 0);
    assert!(!planner.shares_air_objective(&air));
}
