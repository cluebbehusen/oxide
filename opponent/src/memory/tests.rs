use super::*;

#[test]
fn failures_expire_and_the_oldest_leaves_first() {
    let mut memory = Memory::default();
    let anchor = TilePos::new(4, 4);
    memory.fail(BuildingKind::Fabricator, anchor, 100);
    assert!(memory.failed(BuildingKind::Fabricator, anchor, 100 + FAILURE_TICKS - 1));
    assert!(!memory.failed(BuildingKind::Airworks, anchor, 100));
    assert!(!memory.failed(BuildingKind::Fabricator, anchor, 100 + FAILURE_TICKS));
    memory.forget(100 + FAILURE_TICKS);
    assert_eq!(memory, Memory::default());

    for x in 0..=i32::try_from(FAILURE_CAP).unwrap() {
        memory.fail(BuildingKind::Fabricator, TilePos::new(x, 0), 200);
    }
    assert_eq!(memory.failures.len(), FAILURE_CAP);
    assert!(!memory.failed(BuildingKind::Fabricator, TilePos::new(0, 0), 200));
    assert_eq!(memory.validate(200, 8, 8, 0), Ok(()));
    assert!(memory.validate(199, 8, 8, 0).is_err());
}

#[test]
fn raids_are_remembered_apart_from_given_up_targets() {
    let mut memory = Memory::default();
    let anchor = TilePos::new(4, 4);
    memory.raid(BuildingKind::Foundry, anchor, 100);
    assert!(memory.raided(BuildingKind::Foundry, anchor, 100));
    assert!(!memory.abandoned(BuildingKind::Foundry, anchor, 100));
    assert_eq!(memory.validate(100, 8, 8, 0), Ok(()));
    assert!(
        memory.validate(99, 8, 8, 0).is_err(),
        "a raid after the checkpoint was taken"
    );
    memory.forget(100 + FAILURE_TICKS);
    assert!(!memory.raided(BuildingKind::Foundry, anchor, 100 + FAILURE_TICKS));

    let older: Memory = serde_json::from_value(serde_json::json!({
        "units": [],
        "failures": [],
        "abandoned": [],
        "scouted": [],
    }))
    .unwrap();
    assert_eq!(older, Memory::default(), "a checkpoint from before raids");
}

#[test]
fn units_that_cannot_leave_their_ground_are_remembered_longer() {
    use oxide_sim::{PlayerId, Scenario, UnitId};
    let state = Scenario::skirmish().build().unwrap();
    let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
    observation
        .visible
        .iter_mut()
        .for_each(|visible| *visible = false);
    let template = observation.my_units[0].clone();
    observation.enemy_units = [(1, 10), (2, 20)]
        .into_iter()
        .map(|(id, x)| oxide_sim::observation::UnitObs {
            id: UnitId(id),
            kind: UnitKind::Sentinel,
            tile: TilePos::new(x, 10),
            ..template.clone()
        })
        .collect();
    let mut memory = Memory::default();
    let stranded = |unit: &SeenUnit| unit.id == UnitId(2);
    memory.observe_stranded(&observation, stranded);
    observation.enemy_units.clear();
    let remembered =
        |memory: &Memory| -> Vec<UnitId> { memory.units().iter().map(|unit| unit.id).collect() };
    observation.tick = UNIT_TICKS;
    memory.observe_stranded(&observation, stranded);
    assert_eq!(remembered(&memory), [UnitId(2)]);
    observation.tick = STRANDED_TICKS - 1;
    memory.observe_stranded(&observation, stranded);
    assert_eq!(remembered(&memory), [UnitId(2)]);
    observation.tick = STRANDED_TICKS;
    memory.observe_stranded(&observation, stranded);
    assert!(remembered(&memory).is_empty());
}
