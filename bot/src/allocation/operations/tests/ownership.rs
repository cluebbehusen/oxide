use super::*;
use crate::executive::{Army, ArmyState};
use crate::test_support::operations::*;

#[test]
fn lift_may_take_idle_staging_armies_without_stealing_active_defenders() {
    use crate::executive::ArmyId;

    let mut obs = test_island_observation();
    let armies = [
        Army::staging(ArmyId(1), vec![UnitId(1), UnitId(2)], TilePos::new(3, 3)),
        Army {
            state: ArmyState::Staging,
            target: Some(TEST_TARGET),
            ..Army::staging(ArmyId(2), vec![UnitId(3)], TilePos::new(4, 4))
        },
        Army {
            state: ArmyState::Staging,
            target: Some(TEST_TARGET),
            ..Army::staging(ArmyId(3), vec![UnitId(5)], TEST_TARGET)
        },
        Army {
            state: ArmyState::Pushing,
            target: Some(TEST_TARGET),
            ..Army::staging(ArmyId(4), vec![UnitId(6)], TilePos::new(4, 4))
        },
    ];

    assert_eq!(
        lift_unavailable(
            &obs,
            &armies,
            &[
                UnitId(1),
                UnitId(2),
                UnitId(3),
                UnitId(4),
                UnitId(5),
                UnitId(6),
            ],
            &[UnitId(2), UnitId(8)],
        ),
        [UnitId(2), UnitId(4), UnitId(5), UnitId(6), UnitId(8)]
    );

    obs.enemy_buildings.clear();
    obs.enemy_units.push(UnitObs {
        player: PlayerId(1),
        ..test_unit(90, UnitKind::Sentinel, TEST_TARGET.offset(1, 0))
    });
    assert_eq!(
        lift_unavailable(
            &obs,
            &armies,
            &[
                UnitId(1),
                UnitId(2),
                UnitId(3),
                UnitId(4),
                UnitId(5),
                UnitId(6),
            ],
            &[UnitId(2), UnitId(8)],
        ),
        [UnitId(2), UnitId(4), UnitId(5), UnitId(6), UnitId(8)],
        "a visible attacker must keep the objective-holding army out of the lift pool"
    );

    obs.enemy_units.clear();
    assert_eq!(
        lift_unavailable(
            &obs,
            &armies,
            &[
                UnitId(1),
                UnitId(2),
                UnitId(3),
                UnitId(4),
                UnitId(5),
                UnitId(6),
            ],
            &[UnitId(2), UnitId(8)],
        ),
        [UnitId(2), UnitId(4), UnitId(6), UnitId(8)],
        "the same idle holding army becomes transferable once its objective is uncontested"
    );
}
