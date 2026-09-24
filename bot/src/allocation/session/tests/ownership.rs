use super::*;
use crate::allocation::lift_air_support as air_support;

#[test]
fn prior_operations_share_one_canonical_ownership_ledger() {
    use crate::strategy::AirOperation;

    let air = AirOperation {
        target_player: PlayerId(1),
        target_kind: BuildingKind::Foundry,
        target: TilePos::new(20, 8),
        target_id: Some(BuildingId(3)),
        stage: crate::strategy::AirStage::Assemble,
        started_at: 100,
        phase_started_at: 120,
        scout: Some(UnitId(8)),
        scout_dispatch: None,
        strike_hold: None,
        artillery_staging: None,
        artillery: vec![UnitId(7), UnitId(9)],
        strike_aircraft: vec![UnitId(10), UnitId(11)],
        strike_issued_at: None,
        membership_frozen_at: None,
    };

    assert_eq!(
        prior_planner_claims(
            &[UnitId(1), UnitId(8)],
            air.scout
                .into_iter()
                .chain(air.artillery.iter().copied())
                .chain(air.strike_aircraft.iter().copied()),
            &[UnitId(5), UnitId(7)],
            &[UnitId(3), UnitId(5)],
            None,
        ),
        [
            UnitId(1),
            UnitId(3),
            UnitId(5),
            UnitId(7),
            UnitId(8),
            UnitId(9),
            UnitId(10),
            UnitId(11),
        ]
    );

    assert_eq!(
        air_support(Some(&air), None),
        LiftAirSupport::Suppressing {
            player: PlayerId(1),
            target: TilePos::new(20, 8),
        }
    );
    let mut ghost_recon = air.clone();
    ghost_recon.stage = crate::strategy::AirStage::Watching;
    assert_eq!(
        air_support(Some(&ghost_recon), None),
        LiftAirSupport::Independent
    );
    let mut released = air.clone();
    released.stage = crate::strategy::AirStage::Strike;
    assert_eq!(
        air_support(Some(&released), None),
        LiftAirSupport::Released {
            player: PlayerId(1),
            target: TilePos::new(20, 8),
        }
    );
    released.stage = crate::strategy::AirStage::Recover {
        reason: crate::strategy::AirRecoveryReason::Complete,
        assault_admitted: true,
    };
    assert!(matches!(
        air_support(Some(&released), None),
        LiftAirSupport::Released { .. }
    ));
    released.stage = crate::strategy::AirStage::Recover {
        reason: crate::strategy::AirRecoveryReason::NewAirDefense,
        assault_admitted: true,
    };
    assert!(matches!(
        air_support(Some(&released), None),
        LiftAirSupport::Aborted { .. }
    ));
    assert_eq!(
        air_support(
            None,
            Some(AirOperationOutcome::Released {
                player: PlayerId(1),
                target: TilePos::new(20, 8),
            }),
        ),
        LiftAirSupport::Released {
            player: PlayerId(1),
            target: TilePos::new(20, 8),
        }
    );
    assert_eq!(
        air_support(
            None,
            Some(AirOperationOutcome::Aborted {
                player: PlayerId(1),
                target: TilePos::new(20, 8),
            }),
        ),
        LiftAirSupport::Aborted {
            player: PlayerId(1),
            target: TilePos::new(20, 8),
        }
    );
}

#[test]
fn lift_ownership_keeps_landed_assault_riders_in_the_prior_planner_ledger() {
    use crate::lift::{LiftManifest, LiftOperation, LiftPhase, UnitIdSet};

    let lift = LiftOperation {
        target_player: PlayerId(1),
        target_id: BuildingId(9),
        target: TilePos::new(20, 8),
        phase: LiftPhase::Landing,
        started_at: 100,
        phase_started_at: 120,
        deadline: 2_000,
        pickup_component: TilePos::new(5, 5),
        desired_carriers: 2,
        payload: UnitIdSet::from_ids(vec![UnitId(2), UnitId(3), UnitId(4), UnitId(5)]),
        payload_target: 4,
        ground_payload_target: 4,
        planned_drops: vec![TilePos::new(18, 7), TilePos::new(19, 7)],
        manifests: vec![
            LiftManifest {
                carrier: UnitId(20),
                riders: vec![UnitId(2), UnitId(3)],
                pickup: TilePos::new(5, 5),
                drop: TilePos::new(18, 7),
                attack_issued: false,
                load_dispatched: true,
                boarding_closed: true,
                unload_attempts: 1,
                recovery_attempts: 0,
                aborted: false,
                closed: false,
            },
            LiftManifest {
                carrier: UnitId(21),
                riders: vec![UnitId(4), UnitId(5)],
                pickup: TilePos::new(6, 5),
                drop: TilePos::new(19, 7),
                attack_issued: true,
                load_dispatched: true,
                boarding_closed: true,
                unload_attempts: 1,
                recovery_attempts: 0,
                aborted: false,
                closed: true,
            },
        ],
        launched: true,
        producer_assignments: Vec::new(),
        issued_producers: Vec::new(),
    };

    assert_eq!(
        prior_planner_claims(&[UnitId(1)], [], &[], &[], Some(&lift)),
        [
            UnitId(1),
            UnitId(2),
            UnitId(3),
            UnitId(4),
            UnitId(5),
            UnitId(20),
        ],
        "landed riders remain operation-owned after their carrier closes"
    );

    let mut provisioning = lift;
    provisioning.phase = LiftPhase::Provision;
    provisioning.manifests.clear();
    provisioning.payload = UnitIdSet::from_ids(vec![UnitId(6), UnitId(7), UnitId(8)]);
    assert_eq!(
        prior_planner_claims(&[], [], &[], &[], Some(&provisioning)),
        [UnitId(6), UnitId(7), UnitId(8)],
        "the exact payload stays owned while its carriers are still training"
    );
}
