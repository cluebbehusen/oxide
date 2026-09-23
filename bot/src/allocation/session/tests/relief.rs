use super::*;

#[test]
fn failed_allocation_preserves_pressure_age_through_closed_admission() {
    let home = TilePos::new(3, 10);
    let obs = Observation::from_data(ObservationData {
        tick: 120,
        map_width: 40,
        map_height: 24,
        visible: vec![true; 40 * 24],
        explored: vec![true; 40 * 24],
        ally_buildings: vec![observed_building(
            20,
            1,
            BuildingKind::Foundry,
            TilePos::new(24, 10),
        )],
        enemy_units: vec![crate::test_support::unit(
            90,
            PlayerId(2),
            UnitKind::Sentinel,
            TilePos::new(27, 10),
        )],
        my_units: [
            (1, home),
            (2, home.offset(0, 1)),
            (3, TilePos::new(20, 9)),
            (4, TilePos::new(20, 11)),
            (5, TilePos::new(21, 10)),
            (6, TilePos::new(21, 11)),
        ]
        .map(|(id, tile)| owned_unit(id, UnitKind::Sentinel, tile))
        .into(),
        ..crate::test_support::observation_data()
    });
    let mut setup = SessionProfile::new(prime_profile());
    setup.dials.minimum_core_equivalents = 2;
    let map = connected_briefing(&obs);
    let mut policy = UtilityPolicy::new();
    let mut strategy = StrategicPlanner::new();
    let mut team = TeamReliefPlanner::new();
    let mut lifts = LiftPlanner::new();
    let mut raids = RaidPlanner::new();
    let first_seen = obs.tick;
    let mut paused = obs.clone();
    paused.tick += setup.tuning.cadence;
    paused.my_units.retain(|unit| unit.id == UnitId(1));
    assert!(!combat_core_status(&paused, &[], &[], 2).ready);
    let mut replenished = obs.clone();
    replenished.my_units.retain(|unit| unit.id != UnitId(6));
    for unit in replenished
        .my_units
        .iter_mut()
        .filter(|unit| unit.id != UnitId(1))
    {
        unit.id.0 += 100;
    }
    replenished.tick = crate::difficulty::strategic_admission_at_or_after(
        first_seen + u64::from(oxide_sim::TICKS_PER_SECOND) + setup.tuning.reaction_delay,
    );
    for (step, obs) in [obs, paused, replenished].into_iter().enumerate() {
        let fail = step == 0;
        let snapshots = PlannerSnapshots::capture(&strategy, &team, &lifts, &raids);
        let mut work = advanced(snapshots);
        if fail {
            work.team_decision.intents.push(Intent::TrainAt {
                building: BuildingId(999),
                kind: UnitKind::Sentinel,
            });
        }
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let mut trace = AllocationTrace::default();
        let outcome = AllocationSession::new(
            setup.context(&obs, home, &map, &intelligence),
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                team: &mut team,
                lifts: &mut lifts,
                raids: &mut raids,
            },
            work,
            Some(&mut trace),
        )
        .run();
        assert_eq!(outcome.allocation_ok, !fail, "{trace:?}");
        if step < 2 {
            assert!(team.operation().is_none());
            assert!(team.reservations().is_empty());
            assert!(outcome.team_decision.intents.is_empty());
        } else {
            let relief = team
                .operation()
                .expect("allocation rejection and closed admission must not restart credibility");
            assert_eq!(relief.started_at, first_seen);
            assert!(relief.members.iter().all(|id| !(2..=6).contains(&id.0)));
            assert!(!outcome.team_decision.intents.is_empty());
            assert_eq!(team.core_reservations(), outcome.team_decision.reservations);
        }
    }
}
