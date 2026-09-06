use super::*;
use crate::bot::observation::{BuildingObs, UnitObs};
use crate::ids::{PlayerId, UnitId};
use crate::state::Faction;

fn observation() -> Observation {
    Observation {
        tick: 200,
        scrap: 20_000,
        faction: Faction::Ferrous,
        map_width: 24,
        map_height: 14,
        visible: vec![true; 24 * 14],
        explored: vec![true; 24 * 14],
        ..Observation::default()
    }
}

fn building(id: u32, kind: BuildingKind) -> BuildingObs {
    BuildingObs {
        id: BuildingId(id),
        player: PlayerId(0),
        kind,
        anchor: TilePos::new(2 + i32::try_from(id).unwrap(), 2),
        hp: kind.base_stats().max_hp,
        built: true,
        seen: true,
        tier: 0,
    }
}

fn unit(id: u32, kind: UnitKind) -> UnitObs {
    UnitObs {
        id: UnitId(id),
        player: PlayerId(0),
        kind,
        tile: TilePos::new(3 + i32::try_from(id % 10).unwrap(), 6),
        hp: kind.stats().max_hp,
        idle: true,
        carrying: 0,
        harvesting: None,
        cargo: 0,
        site: None,
        salvaging: None,
        founding: None,
        repairing: false,
        grounded: false,
    }
}

fn add_building(obs: &mut Observation, id: u32, kind: BuildingKind, queue: Vec<UnitKind>) {
    obs.my_buildings.push(building(id, kind));
    obs.my_queues.push(queue);
}

#[test]
fn core_status_counts_only_unreserved_line_hulls_and_exact_pipeline_work() {
    let mut obs = observation();
    let mut wounded = unit(10, UnitKind::Sentinel);
    wounded.hp /= 2;
    let reserved = unit(11, UnitKind::Warden);
    let ordinary = unit(12, UnitKind::Breaker);
    obs.my_units.extend([
        wounded.clone(),
        reserved.clone(),
        ordinary.clone(),
        unit(13, UnitKind::Lancer),
        unit(14, UnitKind::Bombard),
        unit(15, UnitKind::Tender),
    ]);
    add_building(
        &mut obs,
        1,
        BuildingKind::Foundry,
        vec![UnitKind::Sentinel, UnitKind::Harvester],
    );
    add_building(
        &mut obs,
        2,
        BuildingKind::Fabricator,
        vec![UnitKind::Warden, UnitKind::Tender],
    );
    let intents = [
        Intent::TrainAt {
            building: BuildingId(1),
            kind: UnitKind::Sentinel,
        },
        Intent::TrainAt {
            building: BuildingId(2),
            kind: UnitKind::Lancer,
        },
    ];

    let expected = crate::bot::executive::unit_strength(&wounded)
        .saturating_add(crate::bot::executive::unit_strength(&ordinary))
        .saturating_add(full_ground_strength(UnitKind::Sentinel))
        .saturating_add(full_ground_strength(UnitKind::Warden))
        .saturating_add(full_ground_strength(UnitKind::Sentinel));
    let status = combat_core_status(&obs, &[reserved.id], &intents, 100);

    assert_eq!(status.projected_strength, expected);
    assert_eq!(status.missing_strength, status.target_strength - expected);
    assert_eq!(
        status.missing_scrap,
        u32::try_from(
            status
                .missing_strength
                .div_ceil(full_ground_strength(UnitKind::Sentinel))
        )
        .unwrap()
        .saturating_mul(UnitKind::Sentinel.stats().cost)
    );
    assert_eq!(
        combat_core_status(&obs, &[], &intents, 100).projected_strength,
        expected.saturating_add(crate::bot::executive::unit_strength(&reserved))
    );
}

#[test]
fn core_status_reports_empty_exact_and_saturated_boundaries() {
    let mut obs = observation();
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
    let sentinel_cost = UnitKind::Sentinel.stats().cost;

    assert_eq!(
        combat_core_status(&obs, &[], &[], 0),
        CombatCoreStatus {
            projected_strength: 0,
            target_strength: 0,
            missing_strength: 0,
            missing_scrap: 0,
            ready: true,
        }
    );

    obs.my_units.push(unit(10, UnitKind::Sentinel));
    let short = combat_core_status(&obs, &[], &[], 2);
    assert_eq!(short.projected_strength, sentinel_strength);
    assert_eq!(short.target_strength, sentinel_strength * 2);
    assert_eq!(short.missing_strength, sentinel_strength);
    assert_eq!(short.missing_scrap, sentinel_cost);
    assert!(!short.ready);

    add_building(&mut obs, 1, BuildingKind::Foundry, vec![UnitKind::Sentinel]);
    let exact = combat_core_status(&obs, &[], &[], 2);
    assert_eq!(exact.projected_strength, exact.target_strength);
    assert_eq!(exact.missing_strength, 0);
    assert_eq!(exact.missing_scrap, 0);
    assert!(exact.ready);

    let saturated = combat_core_status(&observation(), &[], &[], u64::MAX);
    assert_eq!(saturated.target_strength, u64::MAX);
    assert_eq!(saturated.missing_scrap, u32::MAX);
    assert!(!saturated.ready);
}

#[test]
fn opening_recovery_fills_factories_breadth_first_in_canonical_order() {
    let mut obs = observation();
    obs.my_units.push(unit(10, UnitKind::Sentinel));
    add_building(&mut obs, 9, BuildingKind::Foundry, Vec::new());
    add_building(&mut obs, 3, BuildingKind::Foundry, Vec::new());
    let target = full_ground_strength(UnitKind::Sentinel).saturating_mul(3);
    let mut budget = UnitKind::Sentinel.stats().cost.saturating_mul(3);
    let mut intents = Vec::new();

    let status = fill_combat_core_to_strength(
        &obs,
        &[],
        target,
        0,
        ProducerLaneReservations::empty(),
        &mut budget,
        &mut intents,
    );

    assert!(status.ready);
    assert_eq!(
        intents,
        vec![
            Intent::TrainAt {
                building: BuildingId(3),
                kind: UnitKind::Sentinel,
            },
            Intent::TrainAt {
                building: BuildingId(9),
                kind: UnitKind::Sentinel,
            },
        ]
    );
    assert_eq!(budget, UnitKind::Sentinel.stats().cost);
}

#[test]
fn opening_recovery_preserves_capital_and_waits_for_queue_capacity() {
    let mut obs = observation();
    add_building(
        &mut obs,
        1,
        BuildingKind::Foundry,
        vec![UnitKind::Harvester; PLANNING_DEPTH],
    );
    let sentinel_cost = UnitKind::Sentinel.stats().cost;
    let capital = 73;
    let target = full_ground_strength(UnitKind::Sentinel);
    let mut budget = sentinel_cost.saturating_add(capital);
    let mut intents = Vec::new();

    let blocked = fill_combat_core_to_strength(
        &obs,
        &[],
        target,
        capital,
        ProducerLaneReservations::empty(),
        &mut budget,
        &mut intents,
    );
    assert!(!blocked.ready);
    assert!(intents.is_empty());
    assert_eq!(budget, sentinel_cost + capital);

    obs.my_queues[0].pop();
    let ready = fill_combat_core_to_strength(
        &obs,
        &[],
        target,
        capital,
        ProducerLaneReservations::empty(),
        &mut budget,
        &mut intents,
    );
    assert!(ready.ready);
    assert_eq!(
        intents,
        vec![Intent::TrainAt {
            building: BuildingId(1),
            kind: UnitKind::Sentinel,
        }]
    );
    assert_eq!(budget, capital);
}

#[test]
fn opening_recovery_counts_prior_same_think_orders_once() {
    let mut obs = observation();
    add_building(&mut obs, 1, BuildingKind::Foundry, Vec::new());
    add_building(&mut obs, 2, BuildingKind::Foundry, Vec::new());
    let mut intents = vec![Intent::TrainAt {
        building: BuildingId(1),
        kind: UnitKind::Sentinel,
    }];
    let target = full_ground_strength(UnitKind::Sentinel).saturating_mul(2);
    let mut budget = UnitKind::Sentinel.stats().cost.saturating_mul(2);

    let status = fill_combat_core_to_strength(
        &obs,
        &[],
        target,
        0,
        ProducerLaneReservations::empty(),
        &mut budget,
        &mut intents,
    );

    assert!(status.ready);
    assert_eq!(
        intents,
        vec![
            Intent::TrainAt {
                building: BuildingId(1),
                kind: UnitKind::Sentinel,
            },
            Intent::TrainAt {
                building: BuildingId(2),
                kind: UnitKind::Sentinel,
            },
        ]
    );
    assert_eq!(budget, UnitKind::Sentinel.stats().cost);
    assert_eq!(planned_at(&intents, BuildingId(1)), 1);
    assert_eq!(
        planned_kinds_at(&intents, BuildingId(2)),
        [UnitKind::Sentinel]
    );
}

#[test]
fn every_line_hull_uses_the_same_live_and_queued_strength_currency() {
    for kind in [UnitKind::Sentinel, UnitKind::Warden, UnitKind::Breaker] {
        assert!(ordinary_core_unit(kind));
        assert_eq!(
            full_ground_strength(kind),
            crate::bot::executive::unit_strength(&unit(10, kind))
        );
    }
    for kind in [
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Bombard,
        UnitKind::Avalanche,
        UnitKind::Tender,
    ] {
        assert!(!ordinary_core_unit(kind));
    }
}
