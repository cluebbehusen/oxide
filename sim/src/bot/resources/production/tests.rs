use super::super::{
    BuilderResource, CurrentScrap, ProducerLane, ResourceForecast, UnitResource,
    producer_preceding_ticks,
};
use super::*;
use crate::bot::resources::test_support::{all_producers, count_all_paid_ready};
use crate::stats::BuildingKind;

const OBSERVED_AT: Tick = 100;

fn lane(
    producer: u32,
    building: BuildingKind,
    queued: Vec<UnitKind>,
    trainable: Vec<UnitKind>,
    egress: ProducerEgress,
) -> ProducerLane {
    lane_at(
        OBSERVED_AT,
        producer,
        building,
        queued,
        None,
        trainable,
        egress,
    )
}

fn lane_at(
    observed_at: Tick,
    producer: u32,
    building: BuildingKind,
    queued: Vec<UnitKind>,
    front_progress: Option<u32>,
    trainable: Vec<UnitKind>,
    egress: ProducerEgress,
) -> ProducerLane {
    let (earliest_preceding_ticks, no_block_latest_preceding_ticks) =
        producer_preceding_ticks(&queued, front_progress);
    ProducerLane {
        producer: BuildingId(producer),
        kind: building,
        queued,
        trainable,
        observed_at,
        front_progress,
        earliest_preceding_ticks,
        no_block_latest_preceding_ticks,
        ground_egress: egress,
    }
}

fn snapshot(scrap: u32, producers: Vec<ProducerLane>) -> ResourceSnapshot {
    let producer_slots = producers
        .iter()
        .flat_map(ProducerLane::open_slots)
        .collect();
    ResourceSnapshot {
        current_scrap: CurrentScrap(scrap),
        forecast: ResourceForecast {
            observed_at: OBSERVED_AT,
            income: Vec::new(),
        },
        units: Vec::<UnitResource>::new(),
        owned_buildings: Vec::new(),
        builders: Vec::<BuilderResource>::new(),
        producers,
        producer_slots,
    }
}

fn demand(kind: UnitKind, count: usize) -> ProductionDemand {
    ProductionDemand { kind, count }
}

fn lane_with_horizon_capacity(
    producer: u32,
    capacity: Tick,
    trainable: Vec<UnitKind>,
) -> ProducerLane {
    const HORIZON: Tick = 2_400;
    let preceding = HORIZON - capacity;
    let queued_kind = UnitKind::Condor;
    let queued_ticks = Tick::from(queued_kind.stats().train_ticks);
    let queue_count = preceding.div_ceil(queued_ticks);
    let total_ticks = queue_count * queued_ticks;
    let progress = u32::try_from(total_ticks - preceding).expect("fixture progress fits u32");
    lane_at(
        OBSERVED_AT,
        producer,
        BuildingKind::Airworks,
        vec![queued_kind; usize::try_from(queue_count).expect("fixture queue fits usize")],
        Some(progress),
        trainable,
        ProducerEgress::NotRequired,
    )
}

#[test]
fn speculative_capacity_bounds_never_exclude_a_feasible_mixed_roster() {
    for queued in [0, 2, 4] {
        let resources = snapshot(
            10_000,
            vec![
                lane(
                    1,
                    BuildingKind::Airworks,
                    vec![UnitKind::Buzzard; queued],
                    vec![UnitKind::Buzzard, UnitKind::Condor],
                    ProducerEgress::NotRequired,
                ),
                lane(
                    2,
                    BuildingKind::Airworks,
                    vec![],
                    vec![UnitKind::Buzzard],
                    ProducerEgress::NotRequired,
                ),
            ],
        );
        for duration in [180, 360, 600, 1200] {
            for buzzards in 0..5 {
                for condors in 0..4 {
                    let requested = core::iter::repeat_n(UnitKind::Buzzard, buzzards)
                        .chain(core::iter::repeat_n(UnitKind::Condor, condors))
                        .collect::<Vec<_>>();
                    let deadline = OBSERVED_AT + duration;
                    let possible = production_may_fit_horizon(
                        &resources,
                        &requested,
                        deadline,
                        &all_producers(&resources),
                    );
                    let (exact, _) = complete_horizon_fits(
                        &resources,
                        &requested,
                        deadline,
                        &all_producers(&resources),
                    );
                    assert!(possible || !exact);
                }
            }
        }
    }
}

#[test]
fn horizon_timing_reuses_queue_slots_without_opening_them_now() {
    let queued = vec![UnitKind::Lancer; crate::stats::QUEUE_CAP];
    let lane = lane(
        4,
        BuildingKind::Fabricator,
        queued.clone(),
        vec![UnitKind::Bombard],
        ProducerEgress::Open,
    );
    let planned = [UnitKind::Bombard];

    assert!(
        lane.production_timing(&planned).is_none(),
        "a full current queue exposes no command slot"
    );
    assert_eq!(lane.open_slots().count(), 0);

    let timing = lane
        .horizon_timing(&planned)
        .expect("the current queue drains before the later provider is needed");
    let preceding_ticks = queued.iter().fold(0_u64, |ticks, kind| {
        ticks + Tick::from(kind.stats().train_ticks)
    });
    assert_eq!(
        timing.no_block_latest_ready_tick,
        OBSERVED_AT + preceding_ticks + Tick::from(UnitKind::Bombard.stats().train_ticks) - 1
    );
}

#[test]
fn horizon_timing_does_not_cap_lifetime_throughput_at_queue_depth() {
    let lane = lane(
        4,
        BuildingKind::Fabricator,
        Vec::new(),
        vec![UnitKind::Bombard],
        ProducerEgress::Open,
    );
    let planned = vec![UnitKind::Bombard; crate::stats::QUEUE_CAP + 1];

    assert!(
        lane.production_timing(&planned).is_none(),
        "only the current queue remains capped"
    );
    let timing = lane
        .horizon_timing(&planned)
        .expect("a long horizon can use a producer after earlier slots drain");
    assert_eq!(
        timing.no_block_latest_ready_tick,
        OBSERVED_AT
            + u64::try_from(planned.len()).expect("small fixture")
                * Tick::from(UnitKind::Bombard.stats().train_ticks)
            - 1
    );
}

#[test]
fn horizon_feasibility_backtracks_for_a_ferrous_minimum() {
    let resources = snapshot(
        0,
        vec![
            lane(
                1,
                BuildingKind::Airworks,
                Vec::new(),
                vec![UnitKind::Kestrel, UnitKind::Condor],
                ProducerEgress::NotRequired,
            ),
            lane(
                2,
                BuildingKind::Airworks,
                vec![UnitKind::Talon],
                vec![UnitKind::Kestrel, UnitKind::Condor],
                ProducerEgress::NotRequired,
            ),
        ],
    );
    let deadline = OBSERVED_AT + Tick::from(UnitKind::Condor.stats().train_ticks) + 1;

    assert!(production_demands_fit_horizon_with_access(
        &resources,
        &[demand(UnitKind::Kestrel, 1), demand(UnitKind::Condor, 1),],
        deadline,
        &all_producers(&resources),
    ));
}

#[test]
fn horizon_feasibility_backtracks_for_a_cupric_minimum() {
    let resources = snapshot(
        0,
        vec![
            lane(
                1,
                BuildingKind::Airworks,
                Vec::new(),
                vec![UnitKind::Gnat, UnitKind::Moth],
                ProducerEgress::NotRequired,
            ),
            lane(
                2,
                BuildingKind::Airworks,
                vec![UnitKind::Wisp],
                vec![UnitKind::Gnat, UnitKind::Moth],
                ProducerEgress::NotRequired,
            ),
        ],
    );
    let deadline = OBSERVED_AT + Tick::from(UnitKind::Moth.stats().train_ticks) + 1;

    assert!(production_demands_fit_horizon_with_access(
        &resources,
        &[demand(UnitKind::Gnat, 1), demand(UnitKind::Moth, 1)],
        deadline,
        &all_producers(&resources),
    ));
}

#[test]
fn fixed_connected_horizon_handles_sixteen_full_airworks_without_exhaustion() {
    let mut producers: Vec<_> = (1..=16)
        .map(|producer| {
            lane(
                producer,
                BuildingKind::Airworks,
                Vec::new(),
                vec![UnitKind::Kestrel, UnitKind::Darter],
                ProducerEgress::NotRequired,
            )
        })
        .collect();
    producers.push(lane(
        20,
        BuildingKind::Fabricator,
        Vec::new(),
        vec![UnitKind::Bombard],
        ProducerEgress::Open,
    ));
    let resources = snapshot(100_000, producers);
    let mut requested = vec![UnitKind::Kestrel];
    requested.extend(core::iter::repeat_n(UnitKind::Darter, 255));
    requested.push(UnitKind::Bombard);
    let deadline = OBSERVED_AT + 2_400;

    assert!(
        complete_horizon_fits(&resources, &requested, deadline, &all_producers(&resources)).0,
        "one scout leaves room for 255 Darters across sixteen lanes, alongside independent ground work"
    );
}

#[test]
fn irregular_airworks_loads_do_not_create_a_hidden_package_cap() {
    let trainable = vec![UnitKind::Kestrel, UnitKind::Buzzard, UnitKind::Condor];
    let resources = snapshot(
        100_000,
        vec![
            lane_at(
                OBSERVED_AT,
                1,
                BuildingKind::Airworks,
                vec![UnitKind::Buzzard, UnitKind::Buzzard],
                Some(19),
                trainable.clone(),
                ProducerEgress::NotRequired,
            ),
            lane_at(
                OBSERVED_AT,
                2,
                BuildingKind::Airworks,
                vec![UnitKind::Kestrel],
                Some(103),
                trainable.clone(),
                ProducerEgress::NotRequired,
            ),
            lane(
                3,
                BuildingKind::Airworks,
                Vec::new(),
                trainable.clone(),
                ProducerEgress::NotRequired,
            ),
            lane_at(
                OBSERVED_AT,
                4,
                BuildingKind::Airworks,
                vec![UnitKind::Condor],
                Some(157),
                trainable,
                ProducerEgress::NotRequired,
            ),
        ],
    );
    let mut requested = vec![UnitKind::Kestrel];
    requested.extend(core::iter::repeat_n(UnitKind::Buzzard, 4));
    requested.extend(core::iter::repeat_n(UnitKind::Condor, 3));
    requested.extend(core::iter::repeat_n(UnitKind::Buzzard, 29));
    let deadline = OBSERVED_AT + 2_400;

    let (result, visited_states) =
        complete_horizon_fits(&resources, &requested, deadline, &all_producers(&resources));
    assert!(
        result,
        "the exact capacity check rejected a feasible package"
    );
    assert!(
        visited_states < 1_000,
        "canonical capacity search visited {visited_states} states"
    );
    let demands = [
        demand(UnitKind::Kestrel, 1),
        demand(UnitKind::Buzzard, 4),
        demand(UnitKind::Condor, 3),
        demand(UnitKind::Buzzard, 29),
    ];
    assert!(production_demands_fit_horizon_with_access(
        &resources,
        &demands,
        deadline,
        &all_producers(&resources),
    ));
}

#[test]
fn modular_capacity_rejects_fragmented_partial_eligibility_immediately() {
    let all = vec![UnitKind::Kestrel, UnitKind::Buzzard, UnitKind::Condor];
    let air_ground_and_bomber = vec![UnitKind::Buzzard, UnitKind::Condor];
    let scout_and_air_ground = vec![UnitKind::Kestrel, UnitKind::Buzzard];
    let lanes = |bomber_only_capacity| {
        vec![
            lane_with_horizon_capacity(1, bomber_only_capacity, vec![UnitKind::Condor]),
            lane_with_horizon_capacity(2, 1_327, all.clone()),
            lane_with_horizon_capacity(3, 807, all.clone()),
            lane_with_horizon_capacity(4, 1_834, all.clone()),
            lane_with_horizon_capacity(5, 2_079, all.clone()),
            lane_with_horizon_capacity(6, 1_749, air_ground_and_bomber.clone()),
            lane_with_horizon_capacity(7, 2_056, scout_and_air_ground.clone()),
            lane_with_horizon_capacity(8, 779, scout_and_air_ground.clone()),
        ]
    };
    let requested: Vec<_> = core::iter::repeat_n(UnitKind::Kestrel, 19)
        .chain(core::iter::repeat_n(UnitKind::Buzzard, 15))
        .chain(core::iter::repeat_n(UnitKind::Condor, 8))
        .collect();
    let deadline = OBSERVED_AT + 2_400;

    let fragmented = snapshot(0, lanes(1_390));
    let (result, visited_states) = complete_horizon_fits(
        &fragmented,
        &requested,
        deadline,
        &all_producers(&fragmented),
    );
    assert!(!result);
    assert_eq!(visited_states, 1);

    let relaxed = snapshot(0, lanes(1_790));
    let (result, _) =
        complete_horizon_fits(&relaxed, &requested, deadline, &all_producers(&relaxed));
    assert!(result, "the relaxed neighboring fixture fits");
}

#[test]
fn horizon_feasibility_preserves_deadline_access_and_egress_bounds() {
    let air = snapshot(
        0,
        vec![
            lane(
                1,
                BuildingKind::Airworks,
                Vec::new(),
                vec![UnitKind::Kestrel],
                ProducerEgress::NotRequired,
            ),
            lane(
                2,
                BuildingKind::Airworks,
                vec![UnitKind::Talon],
                vec![UnitKind::Kestrel],
                ProducerEgress::NotRequired,
            ),
        ],
    );
    let ready = OBSERVED_AT + Tick::from(UnitKind::Kestrel.stats().train_ticks) - 1;
    assert!(!production_demands_fit_horizon_with_access(
        &air,
        &[demand(UnitKind::Kestrel, 1)],
        ready,
        &all_producers(&air),
    ));
    assert!(production_demands_fit_horizon_with_access(
        &air,
        &[demand(UnitKind::Kestrel, 1)],
        ready + 1,
        &all_producers(&air),
    ));
    assert!(!production_demands_fit_horizon_with_access(
        &air,
        &[demand(UnitKind::Kestrel, 1)],
        ready + 1,
        &ProductionAccess::restricted_kinds(vec![(BuildingId(2), UnitKind::Kestrel)]),
    ));

    let blocked_ground = snapshot(
        0,
        vec![lane(
            3,
            BuildingKind::Fabricator,
            Vec::new(),
            vec![UnitKind::Bombard],
            ProducerEgress::Blocked,
        )],
    );
    assert!(!production_demands_fit_horizon_with_access(
        &blocked_ground,
        &[demand(UnitKind::Bombard, 1)],
        Tick::MAX,
        &all_producers(&blocked_ground),
    ));
}

#[test]
fn restricted_access_is_specific_to_the_unit_kind_on_one_producer() {
    let producer = BuildingId(7);
    let access = ProductionAccess::restricted_kinds(vec![
        (producer, UnitKind::Bombard),
        (producer, UnitKind::Bombard),
    ]);

    assert!(access.allows(producer, UnitKind::Bombard));
    assert!(!access.allows(producer, UnitKind::Lancer));
    assert!(!access.allows(BuildingId(8), UnitKind::Bombard));
}

#[test]
fn foreign_paid_occurrences_remain_in_the_lane_without_supplying_capability() {
    let producer = BuildingId(7);
    let scout = UnitKind::Kestrel;
    let resources = snapshot(
        1000,
        vec![lane(
            producer.0,
            BuildingKind::Airworks,
            vec![scout, UnitKind::Condor, scout],
            vec![scout],
            ProducerEgress::NotRequired,
        )],
    );
    let access = ProductionAccess::restricted_kinds(vec![(producer, scout)])
        .excluding_paid(&[(producer, scout, 0)]);
    assert_eq!(
        paid_queued_ready_occurrences_with_access(&resources, scout, Tick::MAX, &access,),
        [(producer, 1)]
    );
    let first_ready = OBSERVED_AT + u64::from(scout.stats().train_ticks);
    assert_eq!(
        count_paid_queued_ready_with_access(&resources, scout, first_ready, &access),
        0
    );
    let timing = resources.producers()[0].horizon_timing(&[scout]).unwrap();
    assert!(
        timing.no_block_latest_ready_tick
            > first_ready + u64::from(UnitKind::Condor.stats().train_ticks)
    );
}

#[test]
fn paid_queue_access_does_not_authorize_a_new_append() {
    let producer = BuildingId(7);
    let resources = snapshot(
        0,
        vec![lane(
            producer.0,
            BuildingKind::Airworks,
            vec![UnitKind::Condor],
            Vec::new(),
            ProducerEgress::NotRequired,
        )],
    );
    let access = ProductionAccess::restricted_kinds_with_paid(
        Vec::new(),
        vec![(producer, UnitKind::Condor)],
    );

    assert!(!access.allows(producer, UnitKind::Condor));
    assert_eq!(
        count_paid_queued_ready_with_access(&resources, UnitKind::Condor, Tick::MAX, &access),
        1
    );
}

#[test]
fn paid_queue_credit_requires_completion_before_the_deadline_observation() {
    let resources = snapshot(
        0,
        vec![lane(
            4,
            BuildingKind::Fabricator,
            vec![UnitKind::Lancer, UnitKind::Bombard, UnitKind::Bombard],
            vec![UnitKind::Bombard],
            ProducerEgress::Open,
        )],
    );
    let first_bombard_ready = OBSERVED_AT
        + Tick::from(UnitKind::Lancer.stats().train_ticks)
        + Tick::from(UnitKind::Bombard.stats().train_ticks)
        - 1;
    let second_bombard_ready =
        first_bombard_ready + Tick::from(UnitKind::Bombard.stats().train_ticks);

    assert_eq!(
        count_all_paid_ready(&resources, UnitKind::Bombard, first_bombard_ready - 1),
        0
    );
    assert_eq!(
        count_all_paid_ready(&resources, UnitKind::Bombard, first_bombard_ready),
        0,
        "a provider spawned during the deadline tick is absent when the bot decides"
    );
    assert_eq!(
        count_all_paid_ready(&resources, UnitKind::Bombard, first_bombard_ready + 1,),
        1
    );
    assert_eq!(
        count_all_paid_ready(&resources, UnitKind::Bombard, second_bombard_ready),
        1
    );
    assert_eq!(
        count_all_paid_ready(&resources, UnitKind::Bombard, second_bombard_ready + 1,),
        2
    );
    assert_eq!(
        paid_queued_ready_producers_with_access(
            &resources,
            UnitKind::Bombard,
            second_bombard_ready + 1,
            &all_producers(&resources),
        ),
        vec![BuildingId(4), BuildingId(4)],
        "the exact ownership surface preserves same-producer multiplicity"
    );
    assert_eq!(
        count_all_paid_ready(&resources, UnitKind::Avalanche, Tick::MAX),
        0
    );
}

#[test]
fn paid_front_queue_keeps_its_fixed_ready_tick_across_decision_cadences() {
    let deadline = 280;
    let expected_ready = 279;

    for elapsed in [0_u32, 1, 12, 24, 60, 120, 179] {
        let observed_at = OBSERVED_AT + Tick::from(elapsed);
        let producer = || {
            lane_at(
                observed_at,
                4,
                BuildingKind::Airworks,
                vec![UnitKind::Buzzard],
                Some(elapsed),
                vec![UnitKind::Buzzard],
                ProducerEgress::NotRequired,
            )
        };
        let first = snapshot(0, vec![producer()]);
        let second = snapshot(0, vec![producer()]);

        assert_eq!(
            count_all_paid_ready(&first, UnitKind::Buzzard, deadline),
            1,
            "the paid Buzzard lost deadline credit after {elapsed} production ticks"
        );
        assert_eq!(
            count_all_paid_ready(&first, UnitKind::Buzzard, expected_ready),
            0,
            "a unit produced on the deadline tick is not in that observation"
        );
        assert_eq!(
            first, second,
            "identical queue evidence must derive bit-identical resources"
        );

        let timing = first.producers()[0]
            .production_timing(&[UnitKind::Buzzard])
            .expect("one follow-up Buzzard fits the queue");
        assert_eq!(
            timing.earliest_ready_tick,
            expected_ready + Tick::from(UnitKind::Buzzard.stats().train_ticks)
        );
        assert_eq!(
            timing.no_block_latest_ready_tick,
            timing.earliest_ready_tick
        );
    }
}

#[test]
fn paid_queue_credit_uses_only_the_current_queue_and_live_producer() {
    let at_tick_112 = |queued| {
        snapshot(
            0,
            vec![lane_at(
                112,
                4,
                BuildingKind::Airworks,
                queued,
                Some(12),
                vec![UnitKind::Buzzard, UnitKind::Condor],
                ProducerEgress::NotRequired,
            )],
        )
    };
    let commissioned = at_tick_112(vec![UnitKind::Buzzard]);
    let changed_queue = at_tick_112(vec![UnitKind::Condor]);
    let lost_producer = snapshot(0, Vec::new());

    assert_eq!(
        count_all_paid_ready(&commissioned, UnitKind::Buzzard, 280),
        1
    );
    assert_eq!(
        count_all_paid_ready(&changed_queue, UnitKind::Buzzard, 280),
        0,
        "front progress cannot preserve credit after the queued kind changes"
    );
    assert_eq!(
        count_all_paid_ready(&lost_producer, UnitKind::Buzzard, 280),
        0,
        "a destroyed producer cannot preserve credit for its former queue"
    );
}

#[test]
fn paid_ground_queue_credit_requires_known_open_egress() {
    let almost_complete = UnitKind::Bombard.stats().train_ticks - 1;
    let resources = snapshot(
        0,
        vec![
            lane_at(
                OBSERVED_AT,
                1,
                BuildingKind::Fabricator,
                vec![UnitKind::Bombard],
                Some(almost_complete),
                vec![UnitKind::Bombard],
                ProducerEgress::Blocked,
            ),
            lane_at(
                OBSERVED_AT,
                2,
                BuildingKind::Fabricator,
                vec![UnitKind::Bombard],
                Some(almost_complete),
                vec![UnitKind::Bombard],
                ProducerEgress::Unknown,
            ),
            lane_at(
                OBSERVED_AT,
                3,
                BuildingKind::Fabricator,
                vec![UnitKind::Bombard],
                Some(almost_complete),
                vec![UnitKind::Bombard],
                ProducerEgress::Open,
            ),
        ],
    );

    assert_eq!(
        count_all_paid_ready(&resources, UnitKind::Bombard, OBSERVED_AT + 1),
        1,
        "exact progress cannot turn a blocked or unknown doorstep into a deadline promise"
    );
}

#[test]
fn paid_air_queue_credit_does_not_require_ground_egress() {
    let resources = snapshot(
        0,
        vec![lane(
            7,
            BuildingKind::Airworks,
            vec![UnitKind::Buzzard],
            vec![UnitKind::Buzzard],
            ProducerEgress::Unknown,
        )],
    );

    assert_eq!(
        count_all_paid_ready(&resources, UnitKind::Buzzard, Tick::MAX),
        1
    );
}
