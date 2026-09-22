use super::super::{
    BuilderResource, CurrentScrap, ProducerLane, ResourceForecast, UnitResource,
    producer_preceding_ticks,
};
use super::*;
use crate::resources::test_support::{all_producers, count_all_paid_ready};
use oxide_sim::stats::BuildingKind;

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

fn brute_horizon_fits(
    resources: &ResourceSnapshot,
    requested: &[UnitKind],
    deadline: Tick,
) -> bool {
    fn place(
        resources: &ResourceSnapshot,
        remaining: &[UnitKind],
        deadline: Tick,
        lanes: &mut [Vec<UnitKind>],
    ) -> bool {
        let Some((&kind, later)) = remaining.split_first() else {
            return true;
        };
        for index in 0..lanes.len() {
            lanes[index].push(kind);
            let fits = resources.producers()[index]
                .horizon_timing(&lanes[index])
                .is_some_and(|timing| timing.no_block_latest_ready_tick < deadline);
            if fits && place(resources, later, deadline, lanes) {
                return true;
            }
            lanes[index].pop();
        }
        false
    }
    place(
        resources,
        requested,
        deadline,
        &mut vec![Vec::new(); resources.producers().len()],
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
                    let exact = brute_horizon_fits(&resources, &requested, deadline);
                    assert!(possible || !exact);
                }
            }
        }
    }
}

#[test]
fn horizon_timing_reuses_queue_slots_without_opening_them_now() {
    let queued = vec![UnitKind::Lancer; oxide_sim::stats::QUEUE_CAP];
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
    let planned = vec![UnitKind::Bombard; oxide_sim::stats::QUEUE_CAP + 1];

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
fn modular_capacity_rejects_fragmented_partial_eligibility() {
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
    assert!(!production_may_fit_horizon(
        &fragmented,
        &requested,
        deadline,
        &all_producers(&fragmented)
    ));
    let relaxed = snapshot(0, lanes(1_790));
    assert!(production_may_fit_horizon(
        &relaxed,
        &requested,
        deadline,
        &all_producers(&relaxed)
    ));
}

#[test]
fn capacity_bounds_preserve_deadline_access_and_egress() {
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
    assert!(!production_may_fit_horizon(
        &air,
        &[UnitKind::Kestrel],
        ready,
        &all_producers(&air),
    ));
    assert!(production_may_fit_horizon(
        &air,
        &[UnitKind::Kestrel],
        ready + 1,
        &all_producers(&air),
    ));
    assert!(!production_may_fit_horizon(
        &air,
        &[UnitKind::Kestrel],
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
    assert!(!production_may_fit_horizon(
        &blocked_ground,
        &[UnitKind::Bombard],
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
