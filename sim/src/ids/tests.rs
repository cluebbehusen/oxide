use super::*;

#[test]
fn owner_local_unit_rank_ignores_sparse_and_mixed_enemy_ids() {
    let units = [
        (UnitId(2), PlayerId(1)),
        (UnitId(4), PlayerId(0)),
        (UnitId(9), PlayerId(1)),
        (UnitId(20), PlayerId(0)),
        (UnitId(31), PlayerId(1)),
    ];

    assert_eq!(owner_local_unit_rank(UnitId(20), PlayerId(0), units), 1);
    assert_eq!(
        owner_local_unit_rank(UnitId(31), PlayerId(1), units.into_iter().rev()),
        2,
        "input iteration order cannot alter the canonical id rank"
    );
}
