use super::ObservationData as Observation;
use super::*;
use crate::command::{Command, PlayerCommand};
use crate::scenario::{Scenario, UnitSpec};
use crate::stats::UnitKind;
use chassis::grid::TilePos;

#[test]
fn paid_provisional_sites_are_marked_for_their_team_and_hidden_from_enemies() {
    for allied in [false, true] {
        let mut scenario = Scenario::skirmish();
        if allied {
            for player in &mut scenario.players {
                player.team = Some(0);
            }
            let mut opponent = scenario.players[1].clone();
            opponent.team = Some(1);
            scenario.players.push(opponent);
            scenario.map[1].replace_range(1..2, "3");
        }
        let mut state = scenario.build().unwrap();
        let worker = state
            .units
            .iter()
            .find(|u| u.player == PlayerId(0) && u.kind.stats().harvest.is_some())
            .unwrap()
            .id;
        let anchor = state.unit(worker).unwrap().tile();
        let kind = BuildingKind::Turret;
        let site = state.place_provisional_site(PlayerId(0), kind, anchor);
        state.unit_mut(worker).unwrap().order = Order::Found { kind, anchor };
        let own = ObservationData::fog_honest(&state, PlayerId(0));
        assert_eq!(own.version, OBSERVATION_VERSION);
        assert!(
            own.my_buildings
                .iter()
                .find(|b| b.id == site)
                .unwrap()
                .provisional
        );
        let observed_worker = own.my_units.iter().find(|u| u.id == worker).unwrap();
        assert_eq!(observed_worker.site, Some(site));
        assert_eq!(observed_worker.founding, None);
        let roundtrip: Observation =
            serde_json::from_value(serde_json::to_value(&own).unwrap()).unwrap();
        assert_eq!(roundtrip, own);
        let other = ObservationData::fog_honest(&state, PlayerId(1));
        assert!(!other.enemy_buildings.iter().any(|b| b.id == site));
        assert_eq!(
            other
                .ally_buildings
                .iter()
                .any(|b| b.id == site && b.provisional),
            allied
        );
    }
}

fn transport_scenario() -> Scenario {
    let mut scenario = Scenario::skirmish();
    scenario.name = "observation-transport".into();
    scenario.units = vec![
        UnitSpec {
            player: 0,
            kind: UnitKind::Skyhook,
            x: 8,
            y: 5,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 7,
            y: 5,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 9,
            y: 5,
        },
        UnitSpec {
            player: 0,
            kind: UnitKind::Lancer,
            x: 8,
            y: 6,
        },
    ];
    scenario
}

#[test]
fn exact_repair_targets_are_owner_only_and_round_trip() {
    let mut state = Scenario::skirmish().build().unwrap();
    let worker = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(0))
        .unwrap()
        .id;
    let patient = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0))
        .unwrap()
        .id;
    state.unit_mut(worker).unwrap().order = Order::Repair { building: patient };
    for own in [
        ObservationData::fog_honest(&state, PlayerId(0)),
        ObservationData::omniscient(&state, PlayerId(0)),
    ] {
        assert_eq!(own.repair_target(worker), Some(Target::Building(patient)));
        assert_eq!(own.version, OBSERVATION_VERSION);
        let mut missing_targets = serde_json::to_value(&own).unwrap();
        missing_targets
            .as_object_mut()
            .unwrap()
            .remove("my_repair_targets");
        assert!(serde_json::from_value::<Observation>(missing_targets).is_err());
        assert_eq!(
            serde_json::from_str::<Observation>(&serde_json::to_string(&own).unwrap()).unwrap(),
            own
        );
    }
    for other in [
        ObservationData::fog_honest(&state, PlayerId(1)),
        ObservationData::omniscient(&state, PlayerId(1)),
    ] {
        assert_eq!(other.repair_target(worker), None);
        assert!(!other.enemy_units.iter().any(|unit| unit.repairing));
    }
    state.unit_mut(worker).unwrap().order = Order::Idle;
    assert_eq!(
        ObservationData::fog_honest(&state, PlayerId(0)).repair_target(worker),
        None
    );
}

#[test]
fn owner_training_progress_is_exact_aligned_and_required_by_the_schema() {
    let mut state = Scenario::skirmish()
        .build()
        .expect("the skirmish scenario builds");
    let producer = state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Foundry)
        .expect("player zero has a Foundry")
        .id;
    let foundry = state
        .building_mut(producer)
        .expect("the Foundry remains live");
    foundry.queue.clear();
    foundry.queue.push_back(UnitKind::Harvester);
    foundry.phase = crate::state::BuildingPhase::Built { training: 17 };

    for observation in [
        ObservationData::fog_honest(&state, PlayerId(0)),
        ObservationData::omniscient(&state, PlayerId(0)),
    ] {
        let index = observation
            .my_buildings
            .iter()
            .position(|building| building.id == producer)
            .expect("the owner observes its producer");
        assert_eq!(observation.my_queues[index], vec![UnitKind::Harvester]);
        assert_eq!(observation.my_queue_progress[index], 17);
        assert_eq!(observation.own_queue_progress(index), Some(17));
        assert_eq!(observation.my_buildings.len(), observation.my_queues.len());
        assert_eq!(
            observation.my_buildings.len(),
            observation.my_queue_progress.len()
        );

        let encoded = serde_json::to_value(&observation).expect("the observation serializes");
        assert_eq!(
            serde_json::from_value::<Observation>(encoded.clone())
                .expect("the current observation schema round-trips"),
            observation
        );
        let mut obsolete = encoded;
        obsolete
            .as_object_mut()
            .expect("an observation serializes as an object")
            .remove("my_queue_progress");
        assert!(
            serde_json::from_value::<Observation>(obsolete).is_err(),
            "an older snapshot must not silently invent exact progress"
        );
    }
}

#[test]
fn hostile_training_progress_never_enters_an_observation() {
    let control = Scenario::skirmish()
        .build()
        .expect("the skirmish scenario builds");
    let mut variant = control.clone();
    let hostile_producer = variant
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(1) && building.kind == BuildingKind::Foundry)
        .expect("player one has a Foundry")
        .id;
    let foundry = variant
        .building_mut(hostile_producer)
        .expect("the hostile Foundry remains live");
    foundry.queue.push_back(UnitKind::Harvester);
    foundry.phase = crate::state::BuildingPhase::Built { training: 31 };

    assert_eq!(
        ObservationData::fog_honest(&control, PlayerId(0)),
        ObservationData::fog_honest(&variant, PlayerId(0)),
        "fog-honest policy input must not expose hostile production intent"
    );
    assert_eq!(
        ObservationData::omniscient(&control, PlayerId(0)),
        ObservationData::omniscient(&variant, PlayerId(0)),
        "even the complete test view keeps hostile production private"
    );
}

#[test]
fn owner_observation_reports_exact_sling_occupancy_without_leaking_the_manifest() {
    let mut state = transport_scenario()
        .build()
        .expect("the transport scenario builds");
    let skyhook = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Skyhook)
        .expect("the Skyhook exists")
        .id;
    let riders: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.id != skyhook)
        .map(|unit| unit.id)
        .collect();
    let expected_occupancy: u8 = state
        .units()
        .iter()
        .filter(|unit| unit.id != skyhook)
        .map(|unit| unit.kind.stats().transport_size)
        .sum();
    assert_eq!(expected_occupancy, 4, "the authored squad fills the sling");

    state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Load {
            units: riders,
            transport: skyhook,
            queue: false,
        },
    }]);
    for _ in 0..200 {
        if state
            .unit(skyhook)
            .is_some_and(|transport| transport.cargo.len() == 3)
        {
            break;
        }
        state.tick(&[]);
    }
    assert_eq!(
        state
            .unit(skyhook)
            .expect("the loaded Skyhook remains alive")
            .cargo
            .len(),
        3,
        "the whole mixed-size squad boards"
    );

    for observation in [
        ObservationData::fog_honest(&state, PlayerId(0)),
        ObservationData::omniscient(&state, PlayerId(0)),
    ] {
        let transport = observation
            .my_units
            .iter()
            .find(|unit| unit.id == skyhook)
            .expect("the owner observes its Skyhook");
        assert_eq!(transport.cargo, expected_occupancy);
        assert_eq!(observation.my_carried_units.len(), 3);
        for passenger in &observation.my_carried_units {
            assert_eq!(passenger.carrier, skyhook);
            assert!(
                !observation
                    .my_units
                    .iter()
                    .any(|unit| unit.id == passenger.id)
            );
            let actual = state
                .unit(skyhook)
                .unwrap()
                .cargo
                .iter()
                .find(|unit| unit.id == passenger.id)
                .unwrap();
            assert_eq!((passenger.kind, passenger.hp), (actual.kind, actual.hp));
        }
        assert_eq!(
            serde_json::from_str::<Observation>(&serde_json::to_string(&observation).unwrap())
                .unwrap(),
            observation
        );
    }

    let opponent = ObservationData::omniscient(&state, PlayerId(1));
    assert!(opponent.my_carried_units.is_empty());
    assert!(
        ObservationData::fog_honest(&state, PlayerId(1))
            .my_carried_units
            .is_empty()
    );
    let transport = opponent
        .enemy_units
        .iter()
        .find(|unit| unit.id == skyhook)
        .expect("the complete test view includes the hostile Skyhook");
    assert_eq!(
        transport.cargo, 0,
        "an opponent may see the airframe, but not its sealed manifest"
    );
}

#[test]
fn a_parked_airframe_is_grounded_for_its_owner_and_for_anyone_who_sees_it() {
    let mut scenario = Scenario::skirmish();
    scenario.name = "observation-landing".into();
    scenario.units = vec![
        // Mid-basin, clear of both Foundries: nothing in the Condor's
        // acquisition range, so its move ends in a landing. The
        // Sentinel watches from inside its sight and outside its aggro.
        UnitSpec {
            player: 0,
            kind: UnitKind::Condor,
            x: 20,
            y: 12,
        },
        UnitSpec {
            player: 1,
            kind: UnitKind::Sentinel,
            x: 27,
            y: 12,
        },
    ];
    let mut state = scenario.build().expect("the landing scenario builds");
    let condor = state
        .units()
        .iter()
        .find(|unit| unit.kind == UnitKind::Condor)
        .expect("the Condor exists")
        .id;
    let goal = TilePos::new(20, 12);

    let airborne = ObservationData::omniscient(&state, PlayerId(1));
    let flying = airborne
        .enemy_units
        .iter()
        .find(|unit| unit.id == condor)
        .expect("the complete test view includes the hostile Condor");
    assert!(!flying.grounded);
    assert_eq!(flying.body_domain(), crate::stats::Domain::Air);

    state.tick(&[PlayerCommand {
        player: PlayerId(0),
        command: Command::Run {
            units: vec![condor],
            goal,
            queue: false,
        },
    }]);
    for _ in 0..1_500 {
        if state.unit(condor).is_some_and(|unit| unit.landed) {
            break;
        }
        state.tick(&[]);
    }
    let parked = state.unit(condor).expect("the Condor survives its landing");
    assert!(
        parked.landed,
        "the Condor sets down within the flight budget"
    );
    let tile = parked.tile();
    assert!(
        state.vision(PlayerId(1)).visible(tile),
        "the opponent's Sentinel watches the landing tile"
    );

    for observation in [
        ObservationData::fog_honest(&state, PlayerId(0)),
        ObservationData::omniscient(&state, PlayerId(0)),
    ] {
        let own = observation
            .my_units
            .iter()
            .find(|unit| unit.id == condor)
            .expect("the owner observes its Condor");
        assert!(own.grounded);
        assert_eq!(own.body_domain(), crate::stats::Domain::Ground);
    }
    for observation in [
        ObservationData::fog_honest(&state, PlayerId(1)),
        ObservationData::omniscient(&state, PlayerId(1)),
    ] {
        let seen = observation
            .enemy_units
            .iter()
            .find(|unit| unit.id == condor)
            .expect("the opponent sees the parked Condor");
        assert!(
            seen.grounded,
            "a parked airframe is a physical fact, not private intent"
        );
        assert_eq!(seen.body_domain(), crate::stats::Domain::Ground);
    }
}

#[test]
fn owner_observation_exposes_the_active_harvest_node_without_leaking_enemy_orders() {
    let mut state = Scenario::skirmish()
        .build()
        .expect("the skirmish scenario builds");
    let worker = |player| {
        state
            .units()
            .iter()
            .find(|unit| unit.player == player && unit.kind == UnitKind::Harvester)
            .expect("each skirmish seat starts with a Harvester")
            .id
    };
    let known_node = |player| {
        state
            .map()
            .iter()
            .find(|(tile, cell)| cell.scrap > 0 && state.vision(player).visible(*tile))
            .map(|(tile, _)| tile)
            .expect("each skirmish seat starts with visible salvage")
    };
    let p0_worker = worker(PlayerId(0));
    let p1_worker = worker(PlayerId(1));
    let p0_node = known_node(PlayerId(0));
    let p1_node = known_node(PlayerId(1));

    state.tick(&[
        PlayerCommand {
            player: PlayerId(0),
            command: Command::Harvest {
                units: vec![p0_worker],
                node: p0_node,
                queue: false,
            },
        },
        PlayerCommand {
            player: PlayerId(1),
            command: Command::Harvest {
                units: vec![p1_worker],
                node: p1_node,
                queue: false,
            },
        },
    ]);

    for (observer, own_worker, own_node, hostile_worker) in [
        (PlayerId(0), p0_worker, p0_node, p1_worker),
        (PlayerId(1), p1_worker, p1_node, p0_worker),
    ] {
        let observation = ObservationData::omniscient(&state, observer);
        assert_eq!(
            observation
                .my_units
                .iter()
                .find(|unit| unit.id == own_worker)
                .expect("the owner observes its Harvester")
                .harvesting,
            Some(own_node)
        );
        assert_eq!(
            observation
                .enemy_units
                .iter()
                .find(|unit| unit.id == hostile_worker)
                .expect("the complete test view includes the hostile Harvester")
                .harvesting,
            None,
            "an enemy's Harvest order remains private even in a complete test view"
        );
    }
}

#[test]
fn owner_observation_marks_queued_and_looping_programs_without_leaking_them() {
    let mut state = Scenario::skirmish()
        .build()
        .expect("the skirmish scenario builds");
    let own_workers: Vec<_> = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
        .take(2)
        .map(|unit| unit.id)
        .collect();
    let hostile_worker = state
        .units()
        .iter()
        .find(|unit| unit.player == PlayerId(1) && unit.kind == UnitKind::Harvester)
        .expect("the opposing seat starts with a Harvester")
        .id;
    state
        .unit_mut(own_workers[0])
        .expect("the first own Harvester exists")
        .queue
        .push_back(Order::Run {
            goal: TilePos::new(8, 8).into(),
        });
    state
        .unit_mut(own_workers[1])
        .expect("the second own Harvester exists")
        .looping = true;
    state
        .unit_mut(hostile_worker)
        .expect("the hostile Harvester exists")
        .queue
        .push_back(Order::Run {
            goal: TilePos::new(30, 15).into(),
        });

    for observation in [
        ObservationData::fog_honest(&state, PlayerId(0)),
        ObservationData::omniscient(&state, PlayerId(0)),
    ] {
        assert_eq!(observation.my_queued_units, own_workers);
        assert!(observation.has_queued_program(own_workers[0]));
        assert!(observation.has_queued_program(own_workers[1]));
        assert!(!observation.has_queued_program(hostile_worker));
    }
}

#[test]
fn pit_knowledge_is_exploration_limited_and_serialized() {
    let mut scenario = Scenario::skirmish();
    let mut rows = vec![vec!['.'; 40]; 24];
    rows[4][4] = '1';
    rows[18][34] = '2';
    rows[4][10] = '~';
    rows[12][20] = '~';
    scenario.map = rows
        .into_iter()
        .map(|row| row.into_iter().collect())
        .collect();
    scenario.units.clear();
    let state = scenario.build().unwrap();
    let local = TilePos::new(10, 4);
    let hidden = TilePos::new(20, 12);
    let fog = ObservationData::fog_honest(&state, PlayerId(0));
    assert_eq!(fog.known_pits, vec![local]);
    assert!(fog.known_rock_at(local));
    assert!(!fog.known_rock_at(hidden));
    assert_eq!(
        ObservationData::omniscient(&state, PlayerId(0)).known_pits,
        vec![local, hidden]
    );
    let restored: Observation =
        serde_json::from_str(&serde_json::to_string(&fog).unwrap()).unwrap();
    assert_eq!(restored, fog);
}
