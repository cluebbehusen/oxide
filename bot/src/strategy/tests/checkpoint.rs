use super::*;

const TICK: Tick = 200;
const ADMITTED: Tick = TICK - 50;

fn map() -> PublicMapBriefing {
    crate::test_support::briefing(32, 20, [], Vec::new())
}

fn active(planner: &mut StrategicPlanner) -> &mut ActiveAirOperation {
    planner.air.as_mut().expect("the fixture has an operation")
}

fn watching(plan: AirPlan) -> StrategicPlanner {
    let mut op = operation(AirOperationPhase::Recon, TICK);
    op.stage = AirStage::Watching;
    op.artillery.clear();
    op.strike_aircraft.clear();
    planner_with_operation(op, plan)
}

fn remembered_connected() -> StrategicPlanner {
    watching(AirPlan::remembered_connected(&obs(ADMITTED)))
}

fn remembered_island() -> StrategicPlanner {
    let island = IslandPlan::new(
        &profile(),
        &wealthy_island_obs(ADMITTED, 1),
        StrategicProductionContext::empty(),
    );
    watching(AirPlan::Reacquire(ReacquirePlan::severed(&island)))
}

fn island() -> StrategicPlanner {
    let mut plan = AirPlan::island(&profile(), &wealthy_island_obs(ADMITTED, 1));
    let island = plan.island_mut();
    island.screen = vec![UnitId(30), UnitId(31)];
    island.dispatch.strike = Some(AirStrikeDispatch::AttackMove(TARGET));
    let mut op = operation(AirOperationPhase::Strike, TICK);
    op.strike_issued_at = Some(TICK);
    planner_with_operation(op, plan)
}

fn connected() -> StrategicPlanner {
    let mut plan = connected_test_plan(&obs(ADMITTED));
    let connected = plan.connected_mut();
    connected.paid_production = vec![ConnectedPurchase {
        producer: BuildingId(12),
        kind: UnitKind::Bombard,
        issued_at: TICK - 12,
        ready_at: TICK + 100,
        delayed: false,
    }];
    connected.dispatch.suppression = Some(SuppressionDispatch::Position {
        target: Target::Building(BuildingId(81)),
        assignments: vec![(UnitId(2), TilePos::new(18, 10))],
    });
    planner_with_operation(operation(AirOperationPhase::SuppressAa, TICK), plan)
}

fn standing_by() -> StrategicPlanner {
    StrategicPlanner {
        standby: AirStandby {
            scout: Some(UnitId(5)),
            artillery: vec![UnitId(6)],
            strike_aircraft: vec![UnitId(7), UnitId(8)],
        },
        cooldown_until: TICK + 700,
        terminal_outcome: Some(AirOperationOutcome::Released {
            player: PlayerId(1),
            target: TARGET,
        }),
        ..StrategicPlanner::new()
    }
}

type Forge = fn(&mut StrategicPlanner);

fn assert_rejected(fixture: fn() -> StrategicPlanner, cases: &[(&str, Forge)]) {
    let map = map();
    for (name, forge) in cases {
        let mut planner = fixture();
        assert!(
            planner.valid_checkpoint(&map, TICK),
            "{name}: the fixture starts valid"
        );
        forge(&mut planner);
        assert!(!planner.valid_checkpoint(&map, TICK), "{name}");
    }
}

#[test]
fn every_operation_kind_round_trips_as_a_valid_checkpoint() {
    let map = map();
    for planner in [
        StrategicPlanner::new(),
        remembered_connected(),
        remembered_island(),
        island(),
        connected(),
        standing_by(),
    ] {
        assert!(planner.valid_checkpoint(&map, TICK), "{planner:?}");
        let restored = crate::checkpoint::round_trip(&planner);
        assert!(restored.valid_checkpoint(&map, TICK), "{restored:?}");
    }
}

#[test]
fn checkpoint_rejects_operation_clocks_out_of_order_or_in_the_future() {
    assert_rejected(
        connected,
        &[
            ("admission after start", |planner| {
                active(planner).plan.connected_mut().admitted_at = ADMITTED + 1;
            }),
            ("start after phase", |planner| {
                active(planner).op.started_at = ADMITTED + 1;
            }),
            ("future phase", |planner| {
                active(planner).op.phase_started_at = TICK + 1;
            }),
            ("future freeze", |planner| {
                active(planner).op.membership_frozen_at = Some(TICK + 1);
            }),
            ("future strike", |planner| {
                active(planner).op.strike_issued_at = Some(TICK + 1);
            }),
        ],
    );
}

#[test]
fn checkpoint_rejects_an_operation_kind_its_stage_cannot_hold() {
    assert_rejected(
        remembered_connected,
        &[
            ("reacquire in an assault stage", |planner| {
                active(planner).op.stage = AirStage::Recon;
            }),
            ("reacquire after assault admission", |planner| {
                active(planner).op.stage = AirStage::Recover {
                    reason: AirRecoveryReason::Timeout,
                    assault_admitted: true,
                };
            }),
            ("reacquire with frozen membership", |planner| {
                active(planner).op.membership_frozen_at = Some(TICK);
            }),
        ],
    );
    assert_rejected(
        island,
        &[
            ("watching island", |planner| {
                active(planner).op.stage = AirStage::Watching;
            }),
            ("unfrozen strike", |planner| {
                active(planner).op.membership_frozen_at = None;
            }),
        ],
    );
    assert_rejected(
        connected,
        &[("unadmitted recovery", |planner| {
            active(planner).op.stage = AirStage::Recover {
                reason: AirRecoveryReason::Timeout,
                assault_admitted: false,
            };
        })],
    );
}

#[test]
fn checkpoint_rejects_members_that_are_unsorted_or_owned_twice() {
    assert_rejected(
        island,
        &[
            ("unsorted strike aircraft", |planner| {
                active(planner).op.strike_aircraft.reverse();
            }),
            ("duplicate artillery", |planner| {
                active(planner).op.artillery.push(UnitId(2));
            }),
            ("unsorted screen", |planner| {
                active(planner).plan.island_mut().screen.reverse();
            }),
            ("scout also strikes", |planner| {
                active(planner).op.scout = Some(UnitId(3));
            }),
            ("screen also strikes", |planner| {
                active(planner).plan.island_mut().screen = vec![UnitId(4), UnitId(30)];
            }),
            ("standby also serves", |planner| {
                planner.standby.artillery = vec![UnitId(2)];
            }),
        ],
    );
    assert_rejected(
        standing_by,
        &[("unsorted standby", |planner| {
            planner.standby.strike_aircraft.reverse();
        })],
    );
}

#[test]
fn checkpoint_rejects_stored_tiles_off_the_map() {
    assert_rejected(
        island,
        &[
            ("target", |planner| {
                active(planner).op.target = TilePos::new(-1, 10)
            }),
            ("scout goal", |planner| {
                active(planner).op.scout_dispatch = Some((UnitId(1), TilePos::new(5, 20)));
            }),
            ("strike hold", |planner| {
                active(planner).op.strike_hold = Some(TilePos::new(32, 0));
            }),
            ("artillery staging", |planner| {
                active(planner).op.artillery_staging = Some(TilePos::new(0, -1));
            }),
            ("strike dispatch", |planner| {
                active(planner).plan.island_mut().dispatch.strike =
                    Some(AirStrikeDispatch::Attack {
                        target: BuildingId(80),
                        anchor: TilePos::new(40, 10),
                    });
            }),
        ],
    );
    assert_rejected(
        connected,
        &[
            ("suppression stand", |planner| {
                active(planner).plan.connected_mut().dispatch.suppression =
                    Some(SuppressionDispatch::Position {
                        target: Target::Building(BuildingId(81)),
                        assignments: vec![(UnitId(2), TilePos::new(18, 20))],
                    });
            }),
            ("scope", |planner| {
                active(planner).plan.connected_mut().scope = TilePos::new(32, 10);
            }),
        ],
    );
    assert_rejected(
        standing_by,
        &[("terminal target", |planner| {
            planner.terminal_outcome = Some(AirOperationOutcome::Aborted {
                player: PlayerId(1),
                target: TilePos::new(32, 10),
            });
        })],
    );
}

#[test]
fn checkpoint_rejects_a_connected_package_without_a_bounded_canonical_objective() {
    assert_rejected(
        connected,
        &[
            ("no objective id", |planner| {
                active(planner).op.target_id = None
            }),
            ("no anchors", |planner| {
                active(planner)
                    .plan
                    .connected_mut()
                    .package
                    .target_anchors
                    .clear();
            }),
            ("unsorted anchors", |planner| {
                active(planner).plan.connected_mut().package.target_anchors =
                    vec![TARGET, TARGET.offset(-2, 0)];
            }),
            ("duplicate anchors", |planner| {
                active(planner).plan.connected_mut().package.target_anchors = vec![TARGET, TARGET];
            }),
            ("objective outside anchors", |planner| {
                active(planner).plan.connected_mut().package.target_anchors =
                    vec![TARGET.offset(-2, 0)];
            }),
            ("future derivation", |planner| {
                let package = &mut active(planner).plan.connected_mut().package;
                package.derived_at = TICK + 1;
            }),
            ("deadline before derivation", |planner| {
                let package = &mut active(planner).plan.connected_mut().package;
                package.preparation_deadline = package.derived_at - 1;
            }),
            ("deadline beyond the horizon", |planner| {
                let package = &mut active(planner).plan.connected_mut().package;
                package.preparation_deadline =
                    package.derived_at + CONNECTED_PREPARATION_HORIZON + 1;
            }),
            ("demand larger than the map", |planner| {
                active(planner).plan.connected_mut().package.strike[0].count = 32 * 20 + 1;
            }),
            ("overflowing tranches", |planner| {
                let tranches = &mut active(planner)
                    .plan
                    .connected_mut()
                    .package
                    .provider_priority;
                tranches[0].count = usize::MAX;
            }),
            ("too many funded providers", |planner| {
                active(planner)
                    .plan
                    .connected_mut()
                    .package
                    .funded_providers = vec![
                    force_package::FundedProvider {
                        kind: UnitKind::Bombard,
                        command_tick: TICK,
                    };
                    32 * 20 + 1
                ];
            }),
        ],
    );
}

#[test]
fn checkpoint_rejects_paid_production_issued_after_its_readiness_or_the_checkpoint() {
    assert_rejected(
        connected,
        &[
            ("ready before issue", |planner| {
                let purchase = &mut active(planner).plan.connected_mut().paid_production[0];
                purchase.ready_at = purchase.issued_at - 1;
            }),
            ("issued in the future", |planner| {
                let purchase = &mut active(planner).plan.connected_mut().paid_production[0];
                purchase.issued_at = TICK + 1;
                purchase.ready_at = TICK + 200;
            }),
        ],
    );
}
