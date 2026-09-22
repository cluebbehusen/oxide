//! Mine construction and placement must expose only the issuing team's knowledge.

mod common;

use chassis::grid::TilePos;
use common::simulation::*;
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::{BuildingKind, Command, Event, Order, PlaceRefusal, PlayerId, State, UnitKind};

const SITE: TilePos = TilePos::new(15, 8);

fn arena_scenario(mined: bool) -> oxide_sim::Scenario {
    let mut scenario = open_arena(
        36,
        20,
        vec![
            unit(0, UnitKind::Harvester, 5, 8),
            unit(0, UnitKind::Harvester, 14, 8),
            unit(0, UnitKind::Gnat, 4, 15),
        ],
    );
    scenario.players[0].scrap = 1000;
    if mined {
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::ScuttleCharge,
            x: SITE.x,
            y: SITE.y,
        });
    }
    scenario
}

fn arena(mined: bool) -> State {
    arena_scenario(mined).build().unwrap()
}

fn build(state: &mut State, defer: bool) -> oxide_sim::TickReport {
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![state.units()[0].id],
            kind: BuildingKind::Barricade,
            anchor: SITE,
            queue: false,
            defer,
        },
    )])
}

fn roundtrip(state: &State) {
    state.validate_invariants().unwrap();
    let copy: State = serde_json::from_str(&serde_json::to_string(state).unwrap()).unwrap();
    assert_eq!(state.hash(), copy.hash());
}

#[test]
fn hidden_mine_and_empty_ground_accept_identical_remote_placement() {
    for defer in [false, true] {
        let mut empty = arena(false);
        let mut mined = arena(true);
        for state in [&empty, &mined] {
            assert!(state.vision(PlayerId(0)).visible(SITE));
            assert_eq!(
                state.place_intent_refusal(PlayerId(0), BuildingKind::Barricade, SITE),
                None
            );
            assert_eq!(
                state.place_refusal(PlayerId(0), BuildingKind::Barricade, SITE),
                None
            );
        }
        assert_eq!(
            build(&mut empty, defer).events,
            build(&mut mined, defer).events
        );
        assert_eq!(
            empty.player(PlayerId(0)).scrap,
            mined.player(PlayerId(0)).scrap
        );
        assert_eq!(empty.units()[0].path, mined.units()[0].path);
        if !defer {
            assert_eq!(
                mined
                    .buildings()
                    .iter()
                    .filter(|b| b.contains(SITE))
                    .count(),
                2
            );
            assert_eq!(
                mined.place_intent_refusal(PlayerId(0), BuildingKind::Barricade, SITE),
                Some(PlaceRefusal::Building)
            );
        }
        roundtrip(&mined);
    }
}

#[test]
fn first_work_detonates_the_mine_and_destroys_the_scaffold() {
    for defer in [false, true] {
        let mut state = arena(true);
        let mut events = build(&mut state, defer).events;
        for _ in 0..400 {
            if events
                .iter()
                .any(|e| matches!(e, Event::ChargeDetonated { .. }))
            {
                break;
            }
            events.extend(state.tick(&[]).events);
            roundtrip(&state);
        }
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, Event::ChargeDetonated { .. }))
                .count(),
            1
        );
        assert!(!events.iter().any(|e| matches!(
            e,
            Event::OrderStalled { .. } | Event::BuildingCompleted { .. }
        )));
        assert!(!state.buildings().iter().any(|b| b.contains(SITE)));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::UnitDied { unit, .. } if unit.0 == 1))
        );
    }
}

#[test]
fn discovering_a_mine_cancels_travelling_and_queued_crews_with_a_full_refund() {
    for defer in [false, true] {
        let mut state = arena(true);
        let worker = state.units()[0].id;
        let scout = state.units()[2].id;
        let bank = state.player(PlayerId(0)).scrap;
        build(&mut state, defer);
        let next = TilePos::new(4, 10);
        state.tick(&[
            cmd(
                0,
                Command::Move {
                    units: vec![worker],
                    goal: next,
                    queue: true,
                },
            ),
            cmd(
                0,
                Command::Move {
                    units: vec![scout],
                    goal: SITE,
                    queue: false,
                },
            ),
        ]);
        let mut refunded = 0;
        for _ in 0..100 {
            let report = state.tick(&[]);
            for event in report.events {
                assert!(!matches!(event, Event::ChargeDetonated { .. }));
                if let Event::BuildCancelled { refund, .. } = event {
                    refunded += refund;
                }
            }
            if state
                .units()
                .iter()
                .all(|u| !matches!(u.order, Order::Build { .. } | Order::Found { .. }))
            {
                break;
            }
        }
        assert_eq!(refunded, 40);
        assert_eq!(state.player(PlayerId(0)).scrap, bank);
        assert_eq!(
            state.unit(worker).unwrap().order,
            Order::Move { goal: next }
        );
        assert!(
            !state
                .buildings()
                .iter()
                .any(|b| b.player == PlayerId(0) && b.contains(SITE))
        );
        roundtrip(&state);
    }
}

fn raise_enemy_mine() -> (State, oxide_sim::BuildingId) {
    let mut scenario = open_arena(
        36,
        20,
        vec![
            unit(0, UnitKind::Harvester, 14, 6),
            unit(1, UnitKind::Harvester, 16, 8),
            unit(0, UnitKind::Gnat, 4, 15),
        ],
    );
    scenario.players[0].scrap = 1000;
    scenario.players[1].scrap = 1000;
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Fabricator,
        x: 25,
        y: 12,
    });
    let mut state = scenario.build().unwrap();
    let builder = state.units()[1].id;
    let report = state.tick(&[cmd(
        1,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::ScuttleCharge,
            anchor: SITE,
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. }))
    );
    let mine = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::ScuttleCharge)
        .unwrap()
        .id;
    (state, mine)
}

#[test]
fn unfinished_mines_are_visible_and_block_placement_then_leave_frozen_memory() {
    let (mut state, mine) = raise_enemy_mine();
    let viewer = PlayerId(0);
    assert!(!state.building(mine).unwrap().built);
    assert!(state.building_apparent(viewer, state.building(mine).unwrap()));
    assert_eq!(
        state.place_refusal(viewer, BuildingKind::Barricade, SITE),
        Some(PlaceRefusal::Building)
    );
    let mut last_seen = *state
        .vision(viewer)
        .ghosts()
        .iter()
        .find(|g| g.anchor == SITE)
        .unwrap();
    while !state.building(mine).unwrap().built {
        last_seen = *state
            .vision(viewer)
            .ghosts()
            .iter()
            .find(|g| g.anchor == SITE)
            .unwrap();
        state.tick(&[]);
    }
    assert!(!state.building_apparent(viewer, state.building(mine).unwrap()));
    assert_eq!(
        *state
            .vision(viewer)
            .ghosts()
            .iter()
            .find(|g| g.anchor == SITE)
            .unwrap(),
        last_seen
    );
    let obs = oxide_bot::Observation::fog_honest(&state, viewer);
    let remembered = obs
        .enemy_buildings
        .iter()
        .find(|b| b.anchor == SITE)
        .unwrap();
    assert!(!remembered.seen);
    assert_eq!(remembered.hp, last_seen.hp);
    let mut intelligence = oxide_bot::StrategicIntelligence::new();
    intelligence.update(&obs);
    assert!(intelligence.buildings().iter().any(|b| b.anchor == SITE));
    let target = oxide_sim::AttackTarget::RememberedBuilding(oxide_sim::RememberedBuilding {
        owner: PlayerId(1),
        building_kind: BuildingKind::ScuttleCharge,
        anchor: SITE,
    });
    assert!(state.attack_view(viewer, target).unwrap().entity.is_none());
    assert_eq!(
        state.place_refusal(viewer, BuildingKind::Barricade, SITE),
        Some(PlaceRefusal::Building)
    );
    roundtrip(&state);

    // Identical knowledge must not reveal whether the concealed mine survived.
    let mut absent = serde_json::to_value(&state).unwrap();
    absent["buildings"]
        .as_array_mut()
        .unwrap()
        .retain(|b| b["id"] != serde_json::json!(mine));
    let mut absent: State = serde_json::from_value(absent).unwrap();
    for _ in 0..5 {
        state.tick(&[]);
        absent.tick(&[]);
    }
    assert_eq!(
        state.vision(viewer).ghosts(),
        absent.vision(viewer).ghosts()
    );
    assert_eq!(
        state.place_intent_refusal(viewer, BuildingKind::Barricade, SITE),
        absent.place_intent_refusal(viewer, BuildingKind::Barricade, SITE)
    );
    let scout = state.units()[2].id;
    let command = cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: SITE,
            queue: false,
        },
    );
    state.tick(std::slice::from_ref(&command));
    absent.tick(&[command]);
    for _ in 0..100 {
        state.tick(&[]);
        absent.tick(&[]);
        if state.building_apparent(viewer, state.building(mine).unwrap()) {
            break;
        }
    }
    assert!(
        state
            .vision(viewer)
            .ghosts()
            .iter()
            .any(|g| g.anchor == SITE && g.built)
    );
    assert!(
        !absent
            .vision(viewer)
            .ghosts()
            .iter()
            .any(|g| g.anchor == SITE)
    );
    roundtrip(&state);
    roundtrip(&absent);
}

#[test]
fn observed_cancellation_of_an_unfinished_mine_clears_its_memory() {
    let (mut state, mine) = raise_enemy_mine();
    state.tick(&[cmd(1, Command::Cancel { building: mine })]);
    assert!(
        !state
            .vision(PlayerId(0))
            .ghosts()
            .iter()
            .any(|g| g.anchor == SITE)
    );
}

#[test]
fn manual_cancellation_of_a_hidden_mine_overlap_preserves_the_mine() {
    let mut state = arena(true);
    build(&mut state, false);
    let site = state
        .buildings_at(SITE)
        .find(|b| b.player == PlayerId(0))
        .unwrap()
        .id;
    let report = state.tick(&[cmd(0, Command::Cancel { building: site })]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::BuildCancelled { refund: 40, .. }))
    );
    assert!(state.passable(SITE));
    assert_eq!(state.buildings_at(SITE).count(), 1);
    assert!(state.vision(PlayerId(0)).ghosts().is_empty());
    roundtrip(&state);
}

#[test]
fn an_adjacent_builders_first_command_tick_uses_normal_lethal_blast_damage() {
    let mut state = arena(true);
    let builder = state.units()[1].id;
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Barricade,
            anchor: SITE,
            queue: false,
            defer: false,
        },
    )]);
    assert_eq!(
        report
            .events
            .iter()
            .filter(|e| matches!(e, Event::ChargeDetonated { .. }))
            .count(),
        1
    );
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e,Event::UnitDied{unit,..} if *unit==builder))
    );
    assert!(!state.buildings().iter().any(|b| b.contains(SITE)));
    roundtrip(&state);
}

#[test]
fn multiple_mines_and_builders_resolve_each_trigger_once() {
    let mut scenario = open_arena(
        36,
        20,
        vec![
            unit(0, UnitKind::Harvester, 14, 8),
            unit(0, UnitKind::Harvester, 17, 8),
        ],
    );
    scenario.players[0].scrap = 1000;
    for x in [15, 16] {
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::ScuttleCharge,
            x,
            y: 8,
        });
    }
    let mut state = scenario.build().unwrap();
    let crew = state.units().iter().map(|u| u.id).collect();
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: crew,
            kind: BuildingKind::Fabricator,
            anchor: SITE,
            queue: false,
            defer: false,
        },
    )]);
    assert_eq!(
        report
            .events
            .iter()
            .filter(|e| matches!(e, Event::ChargeDetonated { .. }))
            .count(),
        2
    );
    assert_eq!(
        report
            .events
            .iter()
            .filter(|e| matches!(
                e,
                Event::BuildingDestroyed {
                    player: PlayerId(0),
                    ..
                }
            ))
            .count(),
        1
    );
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    );
    roundtrip(&state);
}

#[test]
fn discovery_removes_every_queued_crew_commitment_without_reclaiming_the_site() {
    for defer in [false, true] {
        let mut state = arena(true);
        let crew: Vec<_> = state
            .units()
            .iter()
            .filter(|u| u.kind == UnitKind::Harvester)
            .map(|u| u.id)
            .collect();
        let scout = state.units()[2].id;
        let goal = TilePos::new(5, 15);
        state.tick(&[cmd(
            0,
            Command::Move {
                units: crew.clone(),
                goal,
                queue: false,
            },
        )]);
        let original_orders: Vec<_> = crew
            .iter()
            .map(|&id| state.unit(id).unwrap().order)
            .collect();
        state.tick(&[
            cmd(
                0,
                Command::Build {
                    units: crew.clone(),
                    kind: BuildingKind::Barricade,
                    anchor: SITE,
                    queue: true,
                    defer,
                },
            ),
            cmd(
                0,
                Command::Move {
                    units: vec![scout],
                    goal: SITE,
                    queue: false,
                },
            ),
        ]);
        assert!(
            state
                .unit(crew[0])
                .unwrap()
                .queue
                .iter()
                .any(|o| matches!(o, Order::Build { .. } | Order::Found { .. }))
        );
        let mut refunds = 0;
        for _ in 0..40 {
            for e in state.tick(&[]).events {
                if let Event::BuildCancelled { refund, .. } = e {
                    refunds += refund;
                }
                assert!(!matches!(e, Event::ChargeDetonated { .. }));
            }
        }
        assert_eq!(refunds, 40);
        for (id, order) in crew.into_iter().zip(original_orders) {
            let u = state.unit(id).unwrap();
            assert!(u.queue.is_empty());
            assert_eq!(u.order, order);
        }
        for _ in 0..200 {
            state.tick(&[]);
        }
        assert!(
            !state
                .buildings()
                .iter()
                .any(|b| b.player == PlayerId(0) && b.contains(SITE))
        );
        roundtrip(&state);
    }
}

#[test]
fn construction_trips_a_mine_even_when_the_first_work_rounds_to_zero_hp() {
    let mut scenario = arena_scenario(true);
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Fabricator,
        x: 6,
        y: 2,
    });
    let mut state = scenario.build().unwrap();
    let builder = state.units()[1].id;
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::ScuttleCharge,
            anchor: SITE,
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. }))
    );
    assert_eq!(
        report
            .events
            .iter()
            .filter(|e| matches!(e, Event::ChargeDetonated { .. }))
            .count(),
        1
    );
    assert!(!state.buildings().iter().any(|b| b.contains(SITE)));
    assert_eq!(state.player(PlayerId(0)).scrap, 970);
    roundtrip(&state);
}

#[test]
fn artillery_hits_the_scaffold_above_a_concealed_mine() {
    for mined in [false, true] {
        let mut scenario = open_arena_with(
            40,
            24,
            vec![
                unit(0, UnitKind::Harvester, 5, 8),
                unit(0, UnitKind::Harvester, 14, 6),
                unit(2, UnitKind::Bombard, 20, 8),
            ],
            |rows| rows[20][1] = '3',
        );
        let mut third = scenario.players[0].clone();
        third.name = "Third".into();
        scenario.players.push(third);
        scenario.players[0].scrap = 1000;
        if mined {
            scenario.buildings.push(BuildingSpec {
                player: 1,
                kind: BuildingKind::ScuttleCharge,
                x: SITE.x,
                y: SITE.y,
            });
        }
        let mut state = scenario.build().unwrap();
        let worker = state.units()[0].id;
        let gun = state.units()[2].id;
        state.tick(&[
            cmd(
                0,
                Command::Move {
                    units: vec![worker],
                    goal: TilePos::new(3, 21),
                    queue: false,
                },
            ),
            cmd(
                0,
                Command::Build {
                    units: vec![worker],
                    kind: BuildingKind::Barricade,
                    anchor: SITE,
                    queue: true,
                    defer: false,
                },
            ),
        ]);
        let site = state
            .buildings()
            .iter()
            .find(|b| b.kind == BuildingKind::Barricade)
            .unwrap()
            .id;
        assert!(
            state
                .buildings()
                .iter()
                .filter(|b| b.kind.is_stealthy())
                .all(|b| !state.building_apparent(PlayerId(2), b))
        );
        let report = state.tick(&[cmd(
            2,
            Command::Attack {
                units: vec![gun],
                target: oxide_sim::Target::Building(site).into(),
                queue: false,
            },
        )]);
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. }))
        );
        let mut landed = false;
        for _ in 0..180 {
            let before = state.building(site).unwrap().hp;
            let report = state.tick(&[]);
            if report
                .events
                .iter()
                .any(|e| matches!(e, Event::ShellLanded { player, .. } if *player == PlayerId(2)))
            {
                assert_eq!(state.building(site).unwrap().hp, before - 45);
                assert!(!state.buildings().iter().any(|b| b.kind.is_stealthy()));
                assert!(
                    !report
                        .events
                        .iter()
                        .any(|e| matches!(e, Event::ChargeDetonated { .. }))
                );
                landed = true;
                break;
            }
        }
        assert!(landed);
        roundtrip(&state);
    }
}

#[test]
fn a_visible_replacement_by_the_mines_team_clears_its_memory() {
    for (owner, kind, anchor) in [
        (1, BuildingKind::Barricade, SITE),
        (2, BuildingKind::Fabricator, SITE.offset(-1, 0)),
    ] {
        let mut scenario = open_arena_with(
            40,
            24,
            vec![
                unit(0, UnitKind::Harvester, 14, 6),
                unit(1, UnitKind::Harvester, 16, 8),
                unit(2, UnitKind::Harvester, 18, 10),
            ],
            |rows| rows[1][37] = '3',
        );
        scenario.players[1].team = Some(1);
        scenario.players[1].scrap = 1000;
        let mut ally = scenario.players[1].clone();
        ally.name = "Ally".into();
        scenario.players.push(ally);
        scenario.buildings.push(BuildingSpec {
            player: 1,
            kind: BuildingKind::Fabricator,
            x: 25,
            y: 12,
        });
        let mut state = scenario.build().unwrap();
        let builder = state.units()[1].id;
        let replacement_builder = state.units()[owner as usize].id;
        state.tick(&[cmd(
            1,
            Command::Build {
                units: vec![builder],
                kind: BuildingKind::ScuttleCharge,
                anchor: SITE,
                queue: false,
                defer: false,
            },
        )]);
        let mine = state
            .buildings()
            .iter()
            .find(|b| b.kind.is_stealthy())
            .unwrap()
            .id;
        run_until(&mut state, 100, |s, _| s.building(mine).unwrap().built);
        let viewer = PlayerId(0);
        assert!(!state.building_apparent(viewer, state.building(mine).unwrap()));
        let mut intelligence = oxide_bot::StrategicIntelligence::new();
        intelligence.update(&oxide_bot::Observation::fog_honest(&state, viewer));
        state.tick(&[cmd(
            1,
            Command::Salvage {
                units: vec![builder],
                building: mine,
                queue: false,
            },
        )]);
        run_until(&mut state, 200, |s, _| s.building(mine).is_none());
        assert!(
            state
                .vision(viewer)
                .ghosts()
                .iter()
                .any(|g| g.kind.is_stealthy())
        );
        let report = state.tick(&[cmd(
            owner,
            Command::Build {
                units: vec![replacement_builder],
                kind,
                anchor,
                queue: false,
                defer: false,
            },
        )]);
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. }))
        );
        assert!(
            !state
                .vision(viewer)
                .ghosts()
                .iter()
                .any(|g| g.kind.is_stealthy())
        );
        let observation = oxide_bot::Observation::fog_honest(&state, viewer);
        assert!(
            !observation
                .enemy_buildings
                .iter()
                .any(|b| b.kind.is_stealthy())
        );
        intelligence.update(&observation);
        assert!(
            !intelligence
                .buildings()
                .iter()
                .any(|b| b.kind.is_stealthy())
        );
        let contact = intelligence
            .buildings()
            .iter()
            .find(|b| b.anchor == anchor)
            .unwrap();
        assert_eq!(contact.kind, kind);
        assert_eq!(contact.evidence, oxide_bot::ContactEvidence::Current);
        assert!(contact.id.is_some());
        roundtrip(&state);
    }
}

#[test]
fn a_hostile_scaffold_does_not_disprove_a_remembered_mine() {
    let mut scenario = open_arena_with(
        40,
        24,
        vec![
            unit(0, UnitKind::Harvester, 14, 6),
            unit(1, UnitKind::Harvester, 16, 8),
            unit(2, UnitKind::Harvester, 5, 18),
        ],
        |rows| rows[1][37] = '3',
    );
    let mut third = scenario.players[0].clone();
    third.name = "Third".into();
    third.scrap = 1000;
    scenario.players.push(third);
    scenario.players[1].scrap = 1000;
    scenario.buildings.push(BuildingSpec {
        player: 1,
        kind: BuildingKind::Fabricator,
        x: 25,
        y: 12,
    });
    let mut state = scenario.build().unwrap();
    let mine_builder = state.units()[1].id;
    let site_builder = state.units()[2].id;
    state.tick(&[cmd(
        1,
        Command::Build {
            units: vec![mine_builder],
            kind: BuildingKind::ScuttleCharge,
            anchor: SITE,
            queue: false,
            defer: false,
        },
    )]);
    let mine = state
        .buildings()
        .iter()
        .find(|b| b.kind.is_stealthy())
        .unwrap()
        .id;
    run_until(&mut state, 100, |s, _| s.building(mine).unwrap().built);
    let remembered = *state
        .vision(PlayerId(0))
        .ghosts()
        .iter()
        .find(|g| g.anchor == SITE)
        .unwrap();
    state.tick(&[cmd(
        2,
        Command::Move {
            units: vec![site_builder],
            goal: TilePos::new(14, 11),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        s.unit(site_builder).unwrap().order == Order::Idle
    });
    assert!(state.vision(PlayerId(2)).visible(SITE));
    assert!(
        !state
            .vision(PlayerId(2))
            .ghosts()
            .iter()
            .any(|g| g.kind.is_stealthy())
    );
    let report = state.tick(&[cmd(
        2,
        Command::Build {
            units: vec![site_builder],
            kind: BuildingKind::Barricade,
            anchor: SITE,
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. }))
    );
    assert_eq!(state.buildings_at(SITE).count(), 2);
    assert!(state.vision(PlayerId(0)).ghosts().contains(&remembered));
    let obs = oxide_bot::Observation::fog_honest(&state, PlayerId(0));
    assert!(
        obs.enemy_buildings
            .iter()
            .any(|b| b.anchor == SITE && b.kind.is_stealthy() && !b.seen)
    );
    assert!(
        obs.enemy_buildings
            .iter()
            .any(|b| b.anchor == SITE && b.kind == BuildingKind::Barricade && b.seen)
    );
    roundtrip(&state);
}
