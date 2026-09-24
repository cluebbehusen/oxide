use super::*;

fn relief_observation(home: TilePos) -> Observation {
    Observation::from_data(ObservationData {
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
    })
}

#[test]
fn failed_allocation_preserves_pressure_age_through_closed_admission() {
    let home = TilePos::new(3, 10);
    let obs = relief_observation(home);
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

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let mut trace = AllocationTrace::default();
        let mut session = AllocationSession::new(
            setup.context(&obs, home, &map, &intelligence),
            AllocationParticipants {
                policy: &mut policy,
                strategy: &mut strategy,
                team: &mut team,
                lifts: &mut lifts,
                raids: &mut raids,
            },
            advanced(),
            Some(&mut trace),
        );
        let observed = session.observe_retained_work();
        let mut prepared = session.prepare(observed);
        if fail {
            prepared.coordinator_failure = Some((
                AllocationCoordinatorStageTrace::ObligationCollection,
                AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
            ));
        }
        let resolved = session.resolve(prepared);
        let outcome = session.finish_allocation(resolved);
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
            assert_eq!(team.reservations(), outcome.team_decision.reservations);
        }
    }
}

#[test]
fn accepted_operations_keep_return_orders_and_losses_when_capital_is_rejected() {
    for raid in [false, true] {
        let home = TilePos::new(3, 10);
        let mut obs = relief_observation(home);
        if raid {
            obs.ally_buildings.clear();
            obs.enemy_units = vec![crate::test_support::unit(
                90,
                PlayerId(2),
                UnitKind::Harvester,
                TilePos::new(27, 10),
            )];
            for unit in obs
                .my_units
                .iter_mut()
                .filter(|unit| [UnitId(3), UnitId(4)].contains(&unit.id))
            {
                *unit = owned_unit(unit.id.0, UnitKind::Scuttler, unit.tile);
            }
        }
        let mut setup = SessionProfile::new(prime_profile());
        setup.profile.traits.guile = 80;
        setup.dials.minimum_core_equivalents = 2;
        let map = connected_briefing(&obs);
        let mut policy = UtilityPolicy::new();
        let mut strategy = StrategicPlanner::new();
        let mut team = TeamReliefPlanner::new();
        let mut lifts = LiftPlanner::new();
        let mut raids = RaidPlanner::new();
        let ready = crate::difficulty::strategic_admission_at_or_after(
            obs.tick + u64::from(oxide_sim::TICKS_PER_SECOND) + setup.tuning.reaction_delay,
        );
        for tick in [obs.tick, ready] {
            obs.tick = tick;
            let mut intelligence = StrategicIntelligence::new();
            intelligence.update(&obs);

            let mut work = advanced();
            work.team_decision = team.maintain(&setup.profile, setup.tuning, &obs, home, &[]);
            if raid {
                work.raid_decision =
                    raids.think_with_admission(crate::raid::RaidPlanningContext::new(
                        &setup.profile,
                        setup.tuning,
                        &obs,
                        home,
                        &[],
                        &[],
                    ));
            }
            AllocationSession::new(
                setup.context(&obs, home, &map, &intelligence),
                AllocationParticipants {
                    policy: &mut policy,
                    strategy: &mut strategy,
                    team: &mut team,
                    lifts: &mut lifts,
                    raids: &mut raids,
                },
                work,
                None,
            )
            .run();
        }
        let (started_at, lost) = if raid {
            let operation = raids.operation().expect("raid pair was allocated");
            (operation.started_at, operation.members[0])
        } else {
            let operation = team.operation().expect("credible relief was allocated");
            (operation.started_at, operation.members[0])
        };
        obs.my_units.retain(|unit| unit.id != lost);
        obs.enemy_units.clear();
        setup.dials.minimum_core_equivalents = 100;
        for (step, tick) in [
            ready + setup.tuning.cadence,
            ready + 2 * setup.tuning.cadence,
        ]
        .into_iter()
        .enumerate()
        {
            obs.tick = tick;
            let mut intelligence = StrategicIntelligence::new();
            intelligence.update(&obs);

            let mut work = advanced();
            if raid {
                work.raid_started_at = started_at;
                work.raid_decision = raids.think_with_admission(
                    crate::raid::RaidPlanningContext::new(
                        &setup.profile,
                        setup.tuning,
                        &obs,
                        home,
                        &[],
                        &[],
                    )
                    .with_admission(false),
                );
            } else {
                work.team_started_at = started_at;
                work.team_decision = team.maintain(&setup.profile, setup.tuning, &obs, home, &[]);
            }
            let mut session = AllocationSession::new(
                setup.context(&obs, home, &map, &intelligence),
                AllocationParticipants {
                    policy: &mut policy,
                    strategy: &mut strategy,
                    team: &mut team,
                    lifts: &mut lifts,
                    raids: &mut raids,
                },
                work,
                None,
            );
            let observed = session.observe_retained_work();
            let mut prepared = session.prepare(observed);
            if step == 0 {
                prepared.coordinator_failure = Some((
                    AllocationCoordinatorStageTrace::ObligationCollection,
                    AllocationCoordinatorFailureReasonTrace::ExactDispatchRejected,
                ));
            }
            let resolved = session.resolve(prepared);
            let outcome = session.finish_allocation(resolved);
            assert!(!outcome.allow_new_voluntary_operations);
            assert_eq!(outcome.allocation_ok, step != 0);
            let (members, decision) = if raid {
                let operation = raids.operation().unwrap();
                assert_eq!(operation.phase, crate::raid::RaidPhase::Egress);
                assert_eq!(
                    operation.dispatch,
                    Some(crate::raid::RaidDispatch::Egress(home))
                );
                (&operation.members, &outcome.raid_decision)
            } else {
                let operation = team.operation().unwrap();
                assert_eq!(operation.phase, crate::team::TeamReliefPhase::Withdrawing);
                assert_eq!(
                    operation.dispatch,
                    Some(crate::team::TeamReliefDispatch::Return(home))
                );
                (&operation.members, &outcome.team_decision)
            };
            assert!(!members.contains(&lost));
            if step == 0 {
                assert_eq!(
                    decision.intents,
                    [Intent::MoveUnits {
                        units: members.clone(),
                        goal: home
                    }]
                );
            } else {
                assert!(
                    decision.intents.is_empty(),
                    "accepted return is not reissued"
                );
            }
            let restored_team: TeamReliefPlanner =
                serde_json::from_value(serde_json::to_value(&team).unwrap()).unwrap();
            let restored_raid: RaidPlanner =
                serde_json::from_value(serde_json::to_value(&raids).unwrap()).unwrap();
            assert_eq!(restored_team, team);
            assert_eq!(restored_raid, raids);
            team = restored_team;
            raids = restored_raid;
        }
    }
}
