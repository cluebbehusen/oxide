use super::*;
#[cfg(test)]
use crate::observation::ObservationData;
use crate::observation::{BuildingObs, UnitObs};
use oxide_sim::command::{Command, PlayerCommand};
use oxide_sim::event::Event;
use oxide_sim::ids::PlayerId;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::state::{ExtractorIncome, Faction};

const ME: PlayerId = PlayerId(0);

fn unit(id: u32, kind: UnitKind) -> UnitObs {
    crate::test_support::unit(id, ME, kind, TilePos::new(id as i32, 1))
}

fn building(id: u32, kind: BuildingKind, anchor: TilePos, built: bool) -> BuildingObs {
    BuildingObs {
        built,
        ..crate::test_support::building(id, ME, kind, anchor)
    }
}

fn observation(scrap: u32) -> Observation {
    Observation::from_data(ObservationData {
        me: ME,
        scrap,
        faction: Faction::Ferrous,
        map_width: 80,
        map_height: 60,
        visible: vec![true; 80 * 60],
        explored: vec![true; 80 * 60],
        ..crate::test_support::observation_data()
    })
}

fn scenario_players(scrap: u32) -> Vec<PlayerSpec> {
    vec![
        PlayerSpec {
            name: "Ferrous".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
        PlayerSpec {
            name: "Cupric".into(),
            faction: Faction::Cupric,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
    ]
}

fn bordered_ground(width: usize, height: usize) -> Vec<Vec<char>> {
    let mut tiles = vec![vec!['.'; width]; height];
    tiles[0].fill('#');
    tiles[height - 1].fill('#');
    for row in &mut tiles {
        row[0] = '#';
        row[width - 1] = '#';
    }
    tiles
}

fn map_rows(tiles: Vec<Vec<char>>) -> Vec<String> {
    tiles
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

fn income_parity_state(
    support_owner: u8,
    support_anchor: TilePos,
    support_built: bool,
) -> oxide_sim::State {
    let extractor = TilePos::new(20, 5);
    let mut tiles = bordered_ground(42, 22);
    tiles[1][1] = '1';
    tiles[18][38] = '2';
    tiles[extractor.y as usize][extractor.x as usize] = 'E';
    let scenario = oxide_sim::Scenario {
        mode: Default::default(),
        name: "resource-forecast-parity".into(),
        seed: 17,
        map: map_rows(tiles),
        players: scenario_players(0),
        units: vec![UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 5,
            y: 5,
        }],
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Extractor,
                x: extractor.x,
                y: extractor.y,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Reclaimer,
                x: 4,
                y: 14,
            },
            BuildingSpec {
                player: support_owner,
                kind: BuildingKind::Foundry,
                x: support_anchor.x,
                y: support_anchor.y,
            },
        ],
        meta: None,
    };
    let mut state = scenario.build().expect("forecast parity scenario builds");
    if !support_built {
        crate::test_support::edit_buildings(&mut state, |buildings| {
            let site = buildings
                .iter_mut()
                .find(|building| {
                    building.player == ME
                        && building.kind == BuildingKind::Foundry
                        && building.anchor == support_anchor
                })
                .unwrap();
            site.built = false;
            site.progress = 1;
        });
    }
    crate::test_support::set_tick(&mut state, oxide_sim::stats::FOUNDRY_DRIP_START_TICK - 2);
    state
}

fn blocked_egress_state() -> (oxide_sim::State, BuildingId, BuildingId) {
    let producer_anchor = TilePos::new(11, 9);
    let door = TilePos::new(10, 9);
    let mut tiles = bordered_ground(26, 20);
    tiles[1][1] = '1';
    tiles[16][22] = '2';
    for tile in oxide_sim::geometry::rect_adjacent_tiles(
        producer_anchor,
        BuildingKind::Fabricator.base_stats().size,
    ) {
        if tile != door {
            tiles[tile.y as usize][tile.x as usize] = '#';
        }
    }
    let scenario = oxide_sim::Scenario {
        mode: Default::default(),
        name: "blocked-producer-egress".into(),
        seed: 23,
        map: map_rows(tiles),
        players: scenario_players(1_000),
        units: Vec::new(),
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Fabricator,
                x: producer_anchor.x,
                y: producer_anchor.y,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Turret,
                x: door.x,
                y: door.y,
            },
        ],
        meta: None,
    };
    let mut state = scenario.build().expect("blocked-egress scenario builds");
    let producer = state
        .buildings()
        .iter()
        .find(|building| building.player == ME && building.kind == BuildingKind::Fabricator)
        .expect("the Fabricator stands")
        .id;
    let blocker = state
        .buildings()
        .iter()
        .find(|building| building.player != ME && building.kind == BuildingKind::Turret)
        .expect("the hostile doorstep blocker stands")
        .id;
    crate::test_support::edit_building(&mut state, producer, |building| {
        building.queue.push_back(UnitKind::Lancer);
        building.progress = UnitKind::Lancer.stats().train_ticks - 1;
    });
    (state, producer, blocker)
}

fn resource_observation(scrap: u32) -> Observation {
    let mut obs = observation(scrap);
    obs.my_units = vec![
        unit(3, UnitKind::Sentinel),
        unit(1, UnitKind::Harvester),
        unit(2, UnitKind::Excavator),
    ];
    obs.my_buildings = vec![
        building(8, BuildingKind::Foundry, TilePos::new(3, 3), true),
        building(4, BuildingKind::Fabricator, TilePos::new(8, 3), true),
    ];
    obs.my_queues = vec![vec![UnitKind::Sentinel], Vec::new()];
    obs
}

fn run_until_trained(state: &mut oxide_sim::State, producer: BuildingId, kind: UnitKind) -> Tick {
    let command = PlayerCommand {
        player: ME,
        command: Command::Train {
            building: producer,
            kind,
        },
    };
    let mut next = Some(command);
    loop {
        let commands = next.as_slice();
        let report = state.tick(commands);
        next = None;
        if report.events.iter().any(|event| {
            matches!(
                event,
                Event::UnitTrained {
                    building,
                    kind: trained,
                    ..
                } if *building == producer && *trained == kind
            )
        }) {
            return report.tick;
        }
    }
}

#[test]
fn current_bank_and_completed_income_forecast_remain_distinct() {
    let mut obs = observation(40);
    obs.tick = oxide_sim::stats::FOUNDRY_DRIP_START_TICK;
    obs.my_buildings = vec![
        building(1, BuildingKind::Foundry, TilePos::new(2, 2), true),
        building(2, BuildingKind::Reclaimer, TilePos::new(8, 2), true),
        building(3, BuildingKind::Extractor, TilePos::new(10, 2), true),
        building(4, BuildingKind::Reclaimer, TilePos::new(12, 2), false),
    ];
    obs.my_queues = vec![Vec::new(); 4];

    let resources = ResourceSnapshot::from_observation(&obs);
    let future = resources
        .forecast()
        .income_through(obs.tick.saturating_add(300));
    assert_eq!(resources.current_scrap().amount(), 40);
    assert!(
        future.amount() > 40,
        "completed income should be meaningful"
    );
    assert_eq!(resources.forecast().income_streams().len(), 3);
}

#[test]
fn income_forecast_uses_completed_sources_and_exact_cadences() {
    let mut obs = observation(0);
    obs.tick = 2_405;
    let mut refinery = building(9, BuildingKind::Reclaimer, TilePos::new(40, 3), true);
    refinery.tier = 1;
    obs.my_buildings = vec![
        building(7, BuildingKind::Extractor, TilePos::new(30, 3), true),
        building(2, BuildingKind::Foundry, TilePos::new(2, 3), true),
        building(5, BuildingKind::Extractor, TilePos::new(7, 3), true),
        refinery,
    ];
    obs.my_queues = vec![Vec::new(); 4];

    let forecast = ResourceSnapshot::from_observation(&obs).forecast;
    assert_eq!(forecast.observed_at(), 2_405);
    assert_eq!(
        forecast
            .income_streams()
            .iter()
            .map(|stream| (stream.source, stream.kind, stream.first_payment_tick))
            .collect::<Vec<_>>(),
        vec![
            (BuildingId(2), RecurringIncomeKind::Foundry, 2_459),
            (
                BuildingId(5),
                RecurringIncomeKind::SupportedExtractor,
                2_419,
            ),
            (BuildingId(7), RecurringIncomeKind::RemoteExtractor, 2_409,),
            (BuildingId(9), RecurringIncomeKind::Refinery, 2_410),
        ]
    );
    assert_eq!(forecast.income_through(2_408).amount(), 0);
    assert_eq!(forecast.income_through(2_409).amount(), 1);
    assert_eq!(forecast.income_through(2_419).amount(), 6);
}

#[test]
fn income_forecast_matches_authoritative_support_and_payment_boundaries() {
    let cases = [
        (
            "exact support radius",
            0,
            TilePos::new(11, 5),
            true,
            ExtractorIncome::Supported,
        ),
        (
            "one tile outside support",
            0,
            TilePos::new(10, 5),
            true,
            ExtractorIncome::Remote,
        ),
        (
            "wrong-owner foundry",
            1,
            TilePos::new(11, 5),
            true,
            ExtractorIncome::Remote,
        ),
        (
            "unfinished own foundry",
            0,
            TilePos::new(11, 5),
            false,
            ExtractorIncome::Remote,
        ),
    ];

    for (case, owner, anchor, built, expected_income) in cases {
        let mut state = income_parity_state(owner, anchor, built);
        let extractor = state
            .buildings()
            .iter()
            .find(|building| building.player == ME && building.kind == BuildingKind::Extractor)
            .expect("the authored Extractor stands")
            .id;
        assert_eq!(
            state.extractor_income(extractor),
            Some(expected_income),
            "{case}"
        );

        let resources = ResourceSnapshot::from_observation(&Observation::omniscient(&state, ME));
        let stream = resources
            .forecast()
            .income_streams()
            .iter()
            .find(|stream| stream.source == extractor)
            .expect("the completed Extractor contributes a forecast stream");
        let expected_kind = if expected_income == ExtractorIncome::Supported {
            RecurringIncomeKind::SupportedExtractor
        } else {
            RecurringIncomeKind::RemoteExtractor
        };
        assert_eq!(stream.kind, expected_kind, "{case}");

        let starting_scrap = state.player(ME).scrap;
        let observed_at = state.current_tick();
        for deadline in [observed_at, observed_at + 1, observed_at + 2] {
            while state.current_tick() <= deadline {
                state.tick(&[]);
            }
            assert_eq!(
                state.player(ME).scrap - starting_scrap,
                resources.forecast().income_through(deadline).amount(),
                "{case} through tick {deadline}"
            );
        }
    }
}

#[test]
fn resources_and_claims_use_canonical_id_and_row_major_order() {
    let resources = ResourceSnapshot::from_observation(&resource_observation(200));
    assert_eq!(
        resources
            .units()
            .iter()
            .map(|unit| unit.id)
            .collect::<Vec<_>>(),
        vec![UnitId(1), UnitId(2), UnitId(3)]
    );
    assert_eq!(
        resources
            .builders()
            .iter()
            .map(|unit| unit.id)
            .collect::<Vec<_>>(),
        vec![UnitId(1), UnitId(2)]
    );
    assert_eq!(
        resources
            .producers()
            .iter()
            .map(|lane| lane.producer)
            .collect::<Vec<_>>(),
        vec![BuildingId(4), BuildingId(8)]
    );
}

#[test]
fn site_footprints_use_overflow_safe_row_major_geometry() {
    let edge = SiteFootprint::new(TilePos::new(i32::MAX, i32::MIN), (2, 2));
    assert!(edge.overlaps(edge));
    let a = SiteFootprint::new(TilePos::new(1, 8), (1, 1));
    let b = SiteFootprint::new(TilePos::new(9, 2), (1, 1));
    assert!(!a.overlaps(b));
    assert!(b < a);
    assert!(!a.overlaps(SiteFootprint::new(TilePos::new(2, 8), (1, 1))));
}

#[test]
fn ground_egress_matches_static_tile_blockers_not_unit_collisions_or_charges() {
    let mut obs = observation(0);
    let producer = building(5, BuildingKind::Fabricator, TilePos::new(10, 10), true);
    let door = TilePos::new(9, 10);
    obs.known_rock =
        oxide_sim::geometry::rect_adjacent_tiles(producer.anchor, producer.kind.base_stats().size)
            .filter(|tile| *tile != door)
            .collect();
    obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
    let mut occupant = unit(9, UnitKind::Sentinel);
    occupant.tile = door;
    obs.my_units.push(occupant);
    obs.enemy_buildings
        .push(building(6, BuildingKind::ScuttleCharge, door, true));
    obs.my_buildings.push(producer);
    obs.my_queues.push(Vec::new());

    let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
    assert_eq!(lane.ground_egress, ProducerEgress::Open);

    obs.known_scrap.push((door, 1));
    let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
    assert_eq!(lane.ground_egress, ProducerEgress::Blocked);
}

#[test]
fn formerly_open_ground_egress_becomes_unknown_out_of_sight() {
    let mut obs = observation(0);
    let producer = building(5, BuildingKind::Fabricator, TilePos::new(10, 10), true);
    let door = TilePos::new(9, 10);
    let ring: Vec<_> =
        oxide_sim::geometry::rect_adjacent_tiles(producer.anchor, producer.kind.base_stats().size)
            .collect();
    obs.known_rock = ring.iter().copied().filter(|tile| *tile != door).collect();
    obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
    obs.my_buildings.push(producer);
    obs.my_queues.push(Vec::new());

    let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
    assert_eq!(lane.ground_egress, ProducerEgress::Open);

    for tile in ring {
        let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
        obs.visible[index] = false;
    }
    let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
    assert_eq!(lane.ground_egress, ProducerEgress::Unknown);

    obs.known_rock.push(door);
    obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
    let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
    assert_eq!(lane.ground_egress, ProducerEgress::Blocked);
}

#[test]
fn stale_remembered_blocker_does_not_prove_ground_egress_blocked() {
    let mut obs = observation(0);
    let producer = building(5, BuildingKind::Fabricator, TilePos::new(10, 10), true);
    let door = TilePos::new(9, 10);
    let ring: Vec<_> =
        oxide_sim::geometry::rect_adjacent_tiles(producer.anchor, producer.kind.base_stats().size)
            .collect();
    obs.known_rock = ring.iter().copied().filter(|tile| *tile != door).collect();
    obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
    let mut blocker = building(6, BuildingKind::Turret, door, true);
    blocker.seen = false;
    obs.enemy_buildings.push(blocker);
    obs.my_buildings.push(producer);
    obs.my_queues.push(Vec::new());
    for tile in ring {
        let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
        obs.visible[index] = false;
    }

    let lane = &ResourceSnapshot::from_observation(&obs).producers[0];
    assert_eq!(lane.ground_egress, ProducerEgress::Unknown);
}

#[test]
fn visible_building_blocker_stalls_authoritative_spawn_until_egress_opens() {
    let (mut state, producer, blocker) = blocked_egress_state();
    let resources = ResourceSnapshot::from_observation(&Observation::omniscient(&state, ME));
    let lane = resources
        .producers()
        .iter()
        .find(|lane| lane.producer == producer)
        .unwrap();
    assert_eq!(
        lane.production_timing(&[UnitKind::Lancer])
            .unwrap()
            .current_egress,
        ProducerEgress::Blocked
    );

    let blocked_report = state.tick(&[]);
    assert!(
        blocked_report.events.iter().all(|event| !matches!(
            event,
            Event::UnitTrained { building, .. } if *building == producer
        )),
        "a ready ground unit cannot cross a currently occupied doorstep"
    );
    assert_eq!(state.building(producer).unwrap().queue.len(), 1);

    crate::test_support::edit_buildings(&mut state, |buildings| {
        buildings.retain(|building| building.id != blocker)
    });
    let resources = ResourceSnapshot::from_observation(&Observation::omniscient(&state, ME));
    let lane = resources
        .producers()
        .iter()
        .find(|lane| lane.producer == producer)
        .unwrap();
    assert_eq!(
        lane.production_timing(&[UnitKind::Lancer])
            .unwrap()
            .current_egress,
        ProducerEgress::Open
    );

    let open_report = state.tick(&[]);
    assert!(open_report.events.iter().any(|event| matches!(
        event,
        Event::UnitTrained { building, kind, .. }
            if *building == producer && *kind == UnitKind::Lancer
    )));
}

#[test]
fn empty_queue_starts_this_tick_and_unrepresentable_timing_is_rejected() {
    let mut obs = observation(0);
    obs.tick = 1_000;
    obs.my_buildings = vec![building(
        12,
        BuildingKind::Airworks,
        TilePos::new(20, 5),
        true,
    )];
    obs.my_queues = vec![Vec::new()];
    let resources = ResourceSnapshot::from_observation(&obs);
    let timing = resources.producers[0]
        .production_timing(&[UnitKind::Kestrel])
        .expect("an empty Airworks can start a legal command this tick");
    let ready = obs.tick + Tick::from(UnitKind::Kestrel.stats().train_ticks) - 1;
    assert_eq!(timing.earliest_ready_tick, ready);
    assert_eq!(timing.no_block_latest_ready_tick, ready);
    assert_eq!(timing.current_egress, ProducerEgress::NotRequired);

    obs.tick = Tick::MAX;
    let resources = ResourceSnapshot::from_observation(&obs);
    assert_eq!(
        resources.producers[0].production_timing(&[UnitKind::Kestrel]),
        None
    );
}

#[test]
fn producer_timing_matches_authoritative_owner_visible_front_progress() {
    let mut base = oxide_sim::Scenario::skirmish().build().unwrap();
    crate::test_support::edit_player(&mut base, ME, |item| item.scrap = u32::MAX);
    let producer = base
        .buildings()
        .iter()
        .find(|building| building.player == ME && building.kind == BuildingKind::Foundry)
        .expect("skirmish has a player-zero Foundry")
        .id;
    {
        crate::test_support::edit_building(&mut base, producer, |foundry| {
            foundry.queue.clear();
            foundry.progress = 0;
        });
    }

    let empty_obs = Observation::omniscient(&base, ME);
    let empty_timing = ResourceSnapshot::from_observation(&empty_obs)
        .producers
        .iter()
        .find(|lane| lane.producer == producer)
        .unwrap()
        .production_timing(&[UnitKind::Scuttler])
        .unwrap();
    let empty_actual = run_until_trained(&mut base, producer, UnitKind::Scuttler);
    assert_eq!(empty_timing.earliest_ready_tick, empty_actual);
    assert_eq!(empty_timing.no_block_latest_ready_tick, empty_actual);

    let mut almost_done = oxide_sim::Scenario::skirmish().build().unwrap();
    crate::test_support::edit_player(&mut almost_done, ME, |item| item.scrap = u32::MAX);
    let producer = almost_done
        .buildings()
        .iter()
        .find(|building| building.player == ME && building.kind == BuildingKind::Foundry)
        .unwrap()
        .id;
    {
        crate::test_support::edit_building(&mut almost_done, producer, |foundry| {
            foundry.queue.clear();
            foundry.queue.push_back(UnitKind::Harvester);
            foundry.progress = UnitKind::Harvester.stats().train_ticks - 1;
        });
    }
    let mut just_started = almost_done.clone();
    crate::test_support::edit_building(&mut just_started, producer, |item| item.progress = 0);
    let almost_done_obs = Observation::omniscient(&almost_done, ME);
    let just_started_obs = Observation::omniscient(&just_started, ME);
    assert_ne!(almost_done_obs, just_started_obs);
    let almost_done_timing = ResourceSnapshot::from_observation(&almost_done_obs)
        .producers
        .iter()
        .find(|lane| lane.producer == producer)
        .unwrap()
        .production_timing(&[UnitKind::Sentinel])
        .unwrap();
    let just_started_timing = ResourceSnapshot::from_observation(&just_started_obs)
        .producers
        .iter()
        .find(|lane| lane.producer == producer)
        .unwrap()
        .production_timing(&[UnitKind::Sentinel])
        .unwrap();

    let almost_done_actual = run_until_trained(&mut almost_done, producer, UnitKind::Sentinel);
    let just_started_actual = run_until_trained(&mut just_started, producer, UnitKind::Sentinel);
    assert_eq!(almost_done_timing.earliest_ready_tick, almost_done_actual);
    assert_eq!(
        almost_done_timing.no_block_latest_ready_tick,
        almost_done_actual
    );
    assert_eq!(just_started_timing.earliest_ready_tick, just_started_actual);
    assert_eq!(
        just_started_timing.no_block_latest_ready_tick,
        just_started_actual
    );
}

#[test]
fn misaligned_queue_progress_falls_back_to_conservative_timing() {
    let mut obs = observation(0);
    obs.tick = 100;
    obs.my_buildings = vec![building(
        12,
        BuildingKind::Airworks,
        TilePos::new(20, 5),
        true,
    )];
    obs.my_queues = vec![vec![UnitKind::Buzzard]];
    obs.my_queue_progress = vec![12, u32::MAX];

    assert_eq!(obs.own_queue_progress(0), None);
    let resources = ResourceSnapshot::from_observation(&obs);
    let timing = resources.producers[0]
        .production_timing(&[UnitKind::Kestrel])
        .expect("the valid queue still defines conservative timing");
    assert_eq!(
        timing.earliest_ready_tick,
        obs.tick + Tick::from(UnitKind::Kestrel.stats().train_ticks),
        "the front item may be one production tick from completion"
    );
    assert_eq!(
        timing.no_block_latest_ready_tick,
        obs.tick
            + Tick::from(UnitKind::Buzzard.stats().train_ticks)
            + Tick::from(UnitKind::Kestrel.stats().train_ticks)
            - 1,
        "misaligned progress cannot create an optimistic deadline promise"
    );
}

#[test]
fn impossible_queue_progress_falls_back_without_crediting_empty_queues() {
    let mut obs = observation(0);
    obs.tick = 100;
    obs.my_buildings = vec![building(
        12,
        BuildingKind::Airworks,
        TilePos::new(20, 5),
        true,
    )];
    obs.my_queues = vec![vec![UnitKind::Buzzard]];
    obs.my_queue_progress = vec![UnitKind::Buzzard.stats().train_ticks + 1];

    assert_eq!(obs.own_queue_progress(0), None);
    let resources = ResourceSnapshot::from_observation(&obs);
    let timing = resources.producers[0]
        .production_timing(&[UnitKind::Kestrel])
        .expect("the valid queue still defines conservative timing");
    assert_eq!(
        timing.earliest_ready_tick,
        obs.tick + Tick::from(UnitKind::Kestrel.stats().train_ticks)
    );
    assert_eq!(
        timing.no_block_latest_ready_tick,
        obs.tick
            + Tick::from(UnitKind::Buzzard.stats().train_ticks)
            + Tick::from(UnitKind::Kestrel.stats().train_ticks)
            - 1,
        "overlong progress cannot create optimistic deadline evidence"
    );

    obs.my_queues[0].clear();
    obs.my_queue_progress[0] = 1;
    assert_eq!(
        obs.own_queue_progress(0),
        None,
        "an empty queue cannot carry production progress"
    );
    obs.my_queue_progress[0] = 0;
    assert_eq!(obs.own_queue_progress(0), Some(0));
}

#[test]
fn unrepresentable_income_cadence_is_not_saturated_to_tick_max() {
    assert_eq!(next_multiple_at_or_after(Tick::MAX, 24), None);
    let mut obs = observation(0);
    obs.tick = Tick::MAX;
    obs.my_buildings = vec![building(
        1,
        BuildingKind::Reclaimer,
        TilePos::new(2, 2),
        true,
    )];
    obs.my_queues = vec![Vec::new()];
    assert!(
        ResourceSnapshot::from_observation(&obs)
            .forecast()
            .income_streams()
            .is_empty()
    );
}

#[test]
fn current_prerequisites_gate_new_appends_without_losing_prepaid_queue_work() {
    let mut obs = observation(0);
    obs.tick = 1_000;
    obs.my_buildings = vec![
        building(12, BuildingKind::Airworks, TilePos::new(20, 5), true),
        building(13, BuildingKind::Crucible, TilePos::new(24, 5), false),
    ];
    obs.my_queues = vec![vec![UnitKind::Condor], Vec::new()];

    let resources = ResourceSnapshot::from_observation(&obs);
    let airworks = resources
        .producers()
        .iter()
        .find(|lane| lane.producer == BuildingId(12))
        .expect("the completed Airworks remains a production lane");
    assert_eq!(airworks.queued(), &[UnitKind::Condor]);
    assert!(!airworks.trainable().contains(&UnitKind::Condor));
    assert!(resources.producer_slots_for(UnitKind::Condor).is_empty());
    obs.my_buildings[1].built = true;
    let resources = ResourceSnapshot::from_observation(&obs);
    let airworks = resources
        .producers()
        .iter()
        .find(|lane| lane.producer == BuildingId(12))
        .unwrap();
    assert_eq!(airworks.queued(), &[UnitKind::Condor]);
    assert!(airworks.trainable().contains(&UnitKind::Condor));
    assert_eq!(
        resources.producer_slots_for(UnitKind::Condor)[0].queue_index,
        1
    );
    assert!(airworks.production_timing(&[UnitKind::Condor]).is_some());
}

#[test]
fn completed_producers_and_live_queues_define_deterministic_capacity() {
    let mut obs = observation(0);
    obs.tick = 1_000;
    let mut cupric_only = building(12, BuildingKind::Airworks, TilePos::new(20, 5), true);
    cupric_only.tier = 0;
    obs.my_buildings = vec![
        building(20, BuildingKind::Foundry, TilePos::new(2, 2), true),
        building(5, BuildingKind::Fabricator, TilePos::new(8, 2), false),
        cupric_only,
        building(8, BuildingKind::Fabricator, TilePos::new(8, 8), true),
    ];
    obs.my_queues = vec![
        vec![UnitKind::Harvester, UnitKind::Sentinel],
        Vec::new(),
        vec![UnitKind::Kestrel],
        vec![UnitKind::Lancer; QUEUE_CAP],
    ];

    let resources = ResourceSnapshot::from_observation(&obs);
    assert_eq!(
        resources
            .producers()
            .iter()
            .map(|lane| lane.producer)
            .collect::<Vec<_>>(),
        vec![BuildingId(8), BuildingId(12), BuildingId(20)]
    );
    assert_eq!(
        resources.producer_slots().len(),
        (QUEUE_CAP - 1) + (QUEUE_CAP - 2)
    );
    assert_eq!(
        resources.producer_slots()[0],
        ProducerSlot {
            producer: BuildingId(12),
            queue_index: 1
        }
    );
    assert!(
        resources
            .producer_slots()
            .iter()
            .all(|slot| slot.producer != BuildingId(5) && slot.producer != BuildingId(8))
    );
    assert!(
        resources.producer_slots_for(UnitKind::Moth).is_empty(),
        "a Ferrous seat cannot train Cupric air"
    );
    assert_eq!(
        resources.producer_slots_for(UnitKind::Kestrel).len(),
        QUEUE_CAP - 1
    );
    assert!(
        resources
            .producers()
            .iter()
            .find(|lane| lane.producer == BuildingId(12))
            .unwrap()
            .trainable()
            .contains(&UnitKind::Kestrel)
    );

    let foundry = resources
        .producers()
        .iter()
        .find(|lane| lane.producer == BuildingId(20))
        .unwrap();
    assert_eq!(foundry.queued(), &[UnitKind::Harvester, UnitKind::Sentinel]);
    let timing = foundry
        .production_timing(&[UnitKind::Scuttler])
        .expect("the Foundry has one open legal queue position");
    assert_eq!(
        timing.earliest_ready_tick,
        obs.tick
            + Tick::from(UnitKind::Sentinel.stats().train_ticks)
            + Tick::from(UnitKind::Scuttler.stats().train_ticks),
        "the unknown front item may be one tick from completion"
    );
    assert_eq!(
        timing.no_block_latest_ready_tick,
        obs.tick
            + Tick::from(UnitKind::Harvester.stats().train_ticks)
            + Tick::from(UnitKind::Sentinel.stats().train_ticks)
            + Tick::from(UnitKind::Scuttler.stats().train_ticks)
            - 1,
        "with no egress delay the unknown front item may have zero progress"
    );
    assert_eq!(timing.current_egress, ProducerEgress::Open);
    assert_eq!(
        foundry.production_timing(&[UnitKind::Sentinel; QUEUE_CAP - 1]),
        None,
        "the plan cannot exceed real queue capacity"
    );
    assert_eq!(
        foundry.production_timing(&[UnitKind::Lancer]),
        None,
        "the plan must use this producer's legal roster"
    );
}
