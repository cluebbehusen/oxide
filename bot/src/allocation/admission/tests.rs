use super::*;
use crate::test_support::operations::*;
use oxide_sim::UnitKind;

#[test]
fn retained_ownership_excludes_live_members_without_importing_absent_cargo() {
    let restored = [UnitId(9), UnitId(4), UnitId(9), UnitId(12)];
    let mut observation = test_island_observation();
    observation.my_units.extend([
        test_unit(4, UnitKind::Sentinel, TEST_HOME),
        test_unit(9, UnitKind::Sentinel, TEST_HOME),
    ]);
    observation.my_units.sort_unstable_by_key(|unit| unit.id);

    assert_eq!(
        retained_reservations(Vec::new(), &restored, &observation),
        [UnitId(4), UnitId(9)],
        "an empty decision preserves live ownership without importing absent cargo or losses"
    );
}
