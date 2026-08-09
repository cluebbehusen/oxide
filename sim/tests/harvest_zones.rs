//! Anchored Harvester work zones: local chaining, wreck cleanup, delivery,
//! queue handoff, and fog-honest retirement.

mod common;

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::stats::{
    BuildingKind, HARVEST_MOBILE_DANGER_MARGIN, HARVEST_RADAR_DANGER_RADIUS, HARVEST_ZONE_RADIUS,
};
use oxide_sim::{Command, Event, Order, PlayerId, State, Target, UnitKind};
use serde_json::json;

use common::{cmd, open_arena_with, run_until, unit};

fn state_with_salvage(
    width: usize,
    sources: &[(TilePos, u32)],
    wrecks: &[(TilePos, u32)],
    units: Vec<oxide_sim::scenario::UnitSpec>,
    buildings: Vec<BuildingSpec>,
) -> State {
    let mut scenario = open_arena_with(width, 12, units, |rows| {
        for &(pos, _) in sources {
            rows[pos.y as usize][pos.x as usize] = 's';
        }
    });
    scenario.buildings = buildings;
    let state = scenario.build().unwrap();
    let mut doc = serde_json::to_value(&state).unwrap();
    let map_width = doc["map"]["grid"]["width"].as_i64().unwrap() as usize;
    for &(pos, scrap) in sources {
        let index = pos.y as usize * map_width + pos.x as usize;
        doc["map"]["grid"]["cells"][index]["scrap"] = json!(scrap);
    }
    for &(pos, wreck) in wrecks {
        let index = pos.y as usize * map_width + pos.x as usize;
        doc["map"]["grid"]["cells"][index]["wreck"] = json!(wreck);
    }
    let mut state: State = serde_json::from_value(doc).unwrap();
    // Reconcile the deliberately edited live amounts into normal player
    // memory before any order is issued. Wreck decay runs on tick zero,
    // so wreck fixtures author one extra piece where their exact count
    // matters only as a positive amount.
    state.tick(&[]);
    state
}

fn set_cargo(mut state: State, worker: oxide_sim::UnitId, carrying: u32) -> State {
    let slot = state
        .units()
        .iter()
        .position(|unit| unit.id == worker)
        .expect("worker exists");
    let mut doc = serde_json::to_value(&state).unwrap();
    doc["units"][slot]["carrying"] = json!(carrying);
    state = serde_json::from_value(doc).unwrap();
    state
}

fn set_building_hp(mut state: State, building: oxide_sim::BuildingId, hp: u32) -> State {
    let slot = state
        .buildings()
        .iter()
        .position(|candidate| candidate.id == building)
        .expect("building exists");
    let mut doc = serde_json::to_value(&state).unwrap();
    doc["buildings"][slot]["hp"] = json!(hp);
    state = serde_json::from_value(doc).unwrap();
    state
}

#[test]
fn the_fixed_anchor_reaches_the_widest_deposit_but_cannot_chain_past_it() {
    let anchor = TilePos::new(5, 5);
    let edge = TilePos::new(anchor.x + HARVEST_ZONE_RADIUS, anchor.y);
    let beyond = edge.offset(1, 0);
    let mut state = state_with_salvage(
        24,
        &[(anchor, 1), (edge, 1), (beyond, 1)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 4, 5),
            // The zone only considers visible or remembered salvage.
            // A harmless air scout establishes honest knowledge of both
            // boundary probes without changing their routes.
            unit(0, UnitKind::Talon, 12, 2),
        ],
        vec![],
    );
    let worker = state.units()[0].id;
    let before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: anchor,
            queue: false,
        },
    )]);
    run_until(&mut state, 2_000, |state, _| {
        let unit = state.unit(worker).unwrap();
        unit.order == Order::Idle && unit.carrying == 0
    });

    assert_eq!(state.map().scrap_at(anchor), 0);
    assert_eq!(state.map().scrap_at(edge), 0);
    assert_eq!(
        state.map().scrap_at(beyond),
        1,
        "a source one tile beyond the fixed radius must not become a new anchor"
    );
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        before + 2,
        "both pieces inside the zone reached the bank"
    );
}

#[test]
fn a_dry_node_adopts_a_neighboring_wreck_inside_the_same_contract() {
    let anchor = TilePos::new(6, 5);
    let wreck = TilePos::new(8, 5);
    let mut state = state_with_salvage(
        22,
        &[(anchor, 1)],
        &[(wreck, 4)],
        vec![unit(0, UnitKind::Harvester, 5, 5)],
        vec![],
    );
    let worker = state.units()[0].id;
    let before = state.map().wreck_at(wreck);
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: anchor,
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |state, _| {
        state.map().wreck_at(wreck) < before
    });

    let Order::Harvest {
        node,
        anchor: work_anchor,
        retiring,
    } = state.unit(worker).unwrap().order
    else {
        panic!("the local cleanup should still be the active work contract")
    };
    assert_eq!(node, wreck);
    assert_eq!(work_anchor, Some(anchor));
    assert!(!retiring);
}

#[test]
fn equally_reachable_sources_prefer_the_more_valuable_salvage() {
    let anchor = TilePos::new(9, 5);
    let low = TilePos::new(7, 5);
    let high = TilePos::new(11, 5);
    let mut state = state_with_salvage(
        24,
        &[(anchor, 1)],
        &[(low, 3), (high, 7)],
        vec![unit(0, UnitKind::Harvester, 9, 4)],
        vec![],
    );
    let worker = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: anchor,
            queue: false,
        },
    )]);
    run_until(&mut state, 100, |state, _| {
        matches!(
            state.unit(worker).unwrap().order,
            Order::Harvest { node, .. } if node != anchor
        )
    });
    assert!(
        matches!(
            state.unit(worker).unwrap().order,
            Order::Harvest { node, .. } if node == high
        ),
        "value breaks an equal route-and-distance tie deterministically"
    );
}

#[test]
fn a_delivery_returns_to_the_same_zone_while_salvage_remains() {
    let source = TilePos::new(9, 5);
    let mut state = state_with_salvage(
        24,
        &[(source, 12)],
        &[],
        vec![unit(0, UnitKind::Harvester, 8, 5)],
        vec![],
    );
    let worker = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: source,
            queue: false,
        },
    )]);
    run_until(&mut state, 800, |_, events| {
        events
            .iter()
            .any(|event| matches!(event, Event::ScrapDeposited { .. }))
    });
    assert!(matches!(
        state.unit(worker).unwrap().order,
        Order::Harvest {
            node,
            anchor: Some(anchor),
            retiring: false,
        } if node == source && anchor == source
    ));
    assert_eq!(state.unit(worker).unwrap().carrying, 0);

    run_until(&mut state, 500, |state, _| {
        state.unit(worker).unwrap().carrying > 0
    });
    assert!(
        state.map().scrap_at(source) < 2,
        "the worker returned after depositing instead of retiring at home"
    );
}

#[test]
fn retirement_deposits_then_advances_one_queued_order_at_the_foundry() {
    let source = TilePos::new(9, 5);
    let goal = TilePos::new(18, 5);
    let mut state = state_with_salvage(
        24,
        &[(source, 1)],
        &[],
        vec![unit(0, UnitKind::Harvester, 8, 5)],
        vec![],
    );
    let worker = state.units()[0].id;
    state.tick(&[
        cmd(
            0,
            Command::Harvest {
                units: vec![worker],
                node: source,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![worker],
                goal,
                queue: true,
            },
        ),
    ]);
    run_until(
        &mut state,
        1_000,
        |state, _| matches!(state.unit(worker).unwrap().order, Order::Move { goal: g } if g == goal),
    );
    let unit = state.unit(worker).unwrap();
    assert_eq!(unit.carrying, 0, "the queued leg starts only after deposit");
    assert!(
        unit.queue.is_empty(),
        "the queued leg was popped exactly once"
    );
    let tile = unit.tile();
    let at_doorstep = (0..=3).contains(&tile.x)
        && (0..=3).contains(&tile.y)
        && !((1..=2).contains(&tile.x) && (1..=2).contains(&tile.y));
    assert!(
        at_doorstep,
        "the handoff happens at the Foundry doorstep, got {tile:?}"
    );

    state.tick(&[]);
    assert!(
        matches!(state.unit(worker).unwrap().order, Order::Move { goal: g } if g == goal),
        "retirement cannot pop the newly active order a second time"
    );
}

#[test]
fn shared_sight_retires_an_autonomous_retarget_but_not_before_it_is_known() {
    let anchor = TilePos::new(11, 5);
    let fallback = TilePos::new(14, 5);
    let mut state = state_with_salvage(
        34,
        &[(anchor, 1), (fallback, 30)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 10, 5),
            unit(0, UnitKind::Talon, 4, 5),
            unit(1, UnitKind::Bombard, 22, 5),
        ],
        vec![],
    );
    let worker = state.units()[0].id;
    let scout = state.units()[1].id;
    let threat = state.units()[2].id;
    assert!(
        !state
            .vision(PlayerId(0))
            .visible(state.unit(threat).unwrap().tile())
    );
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: anchor,
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |state, _| {
        state.unit(worker).unwrap().carrying > 1
            && matches!(
                state.unit(worker).unwrap().order,
                Order::Harvest {
                    node,
                    anchor: Some(work_anchor),
                    retiring: false,
                } if node == fallback && work_anchor == anchor
            )
    });
    assert!(
        !state
            .vision(PlayerId(0))
            .visible(state.unit(threat).unwrap().tile()),
        "the autonomous retarget was legal only because the Bombard was still hidden"
    );

    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(20, 5),
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |state, _| {
        matches!(
            state.unit(worker).unwrap().order,
            Order::Harvest { retiring: true, .. }
        )
    });
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(4, 5),
            queue: false,
        },
    )]);
    let events = run_until(&mut state, 800, |state, _| {
        state.unit(worker).unwrap().order == Order::Idle
    });
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ScrapDeposited { .. })),
        "sticky retirement brought the partial load home"
    );
    assert!(
        state.map().scrap_at(fallback) > 0,
        "losing sight cannot send the retired worker back into the zone"
    );
}

#[test]
fn a_hidden_artillery_hit_diverts_autonomous_work_without_revealing_the_gun() {
    let (safe, anchor, exposed) = (TilePos::new(8, 5), TilePos::new(13, 5), TilePos::new(15, 5));
    let mut state = state_with_salvage(
        32,
        &[(safe, 20), (anchor, 1), (exposed, 30)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 12, 5),
            unit(1, UnitKind::Bombard, 22, 4),
            unit(1, UnitKind::Harvester, 19, 4),
        ],
        vec![],
    );
    let (worker, bombard) = (state.units()[0].id, state.units()[1].id);
    assert!(
        !state.can_see(PlayerId(0), state.unit(bombard).unwrap().tile()),
        "the victim's team cannot see the artillery"
    );

    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: anchor,
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |state, _| {
        matches!(
            state.unit(worker).unwrap().order,
            Order::Harvest {
                node,
                anchor: Some(work_anchor),
                retiring: false,
            } if node == exposed && work_anchor == anchor
        ) && state.unit(worker).unwrap().path.is_none()
            && state.can_see(PlayerId(1), state.unit(worker).unwrap().tile())
    });
    assert!(
        state.can_see(PlayerId(1), state.unit(worker).unwrap().tile()),
        "the spotter sees the worker without making the gun visible in return"
    );
    // The spotter may have triggered a lead while the worker approached.
    // Stage one stationary shot so this test isolates anonymous danger memory.
    let bombard_slot = state
        .units()
        .iter()
        .position(|unit| unit.id == bombard)
        .expect("bombard exists");
    let worker_slot = state
        .units()
        .iter()
        .position(|unit| unit.id == worker)
        .expect("worker exists");
    let mut doc = serde_json::to_value(&state).unwrap();
    doc["shells"] = json!([]);
    doc["units"][bombard_slot]["cooldowns"][0] = json!(0);
    doc["units"][worker_slot]["carrying"] = json!(0);
    doc["units"][worker_slot]["progress"] = json!(0);
    state = serde_json::from_value(doc).unwrap();
    let before = state.unit(worker).unwrap().hp;
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(worker),
            queue: false,
        },
    )]);
    assert!(
        !state.shells().is_empty(),
        "the hidden gun launched through its spotter"
    );
    state.tick(&[cmd(
        1,
        Command::Stop {
            units: vec![bombard],
        },
    )]);
    run_until(&mut state, 100, |state, events| {
        events
            .iter()
            .any(|event| matches!(event, Event::ShellLanded { .. }))
            && state.unit(worker).unwrap().hp < before
    });
    let exposed_after_hit = state.map().scrap_at(exposed);
    assert!(
        !state.can_see(PlayerId(0), state.unit(bombard).unwrap().tile()),
        "taking damage does not disclose the hidden shooter's tile"
    );

    state.tick(&[]);
    assert!(matches!(
        state.unit(worker).unwrap().order,
        Order::Harvest {
            node,
            anchor: Some(work_anchor),
            retiring: false,
        } if node == safe && work_anchor == anchor
    ));
    assert_eq!(
        state.map().scrap_at(exposed),
        exposed_after_hit,
        "the anonymous incident, not hidden enemy state, diverted the worker"
    );
}

#[test]
fn an_own_loss_retires_a_worker_home_before_it_surfaces_idle() {
    let anchor = TilePos::new(13, 5);
    let exposed = TilePos::new(15, 5);
    let struck = TilePos::new(19, 4);
    let mut state = state_with_salvage(
        32,
        &[(anchor, 1), (exposed, 30)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 12, 5),
            unit(1, UnitKind::Bombard, 24, 4),
            unit(1, UnitKind::Harvester, 21, 4),
        ],
        vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: struck.x,
            y: struck.y,
        }],
    );
    let (worker, bombard) = (state.units()[0].id, state.units()[1].id);
    let victim = state
        .buildings()
        .iter()
        .find(|building| building.anchor == struck)
        .unwrap()
        .id;
    state = set_building_hp(state, victim, 40);

    state.tick(&[
        cmd(
            0,
            Command::Harvest {
                units: vec![worker],
                node: anchor,
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Attack {
                units: vec![bombard],
                target: Target::Building(victim),
                queue: false,
            },
        ),
    ]);
    state.tick(&[cmd(
        1,
        Command::Stop {
            units: vec![bombard],
        },
    )]);
    run_until(&mut state, 300, |state, _| {
        matches!(
            state.unit(worker).unwrap().order,
            Order::Harvest { node, .. } if node == exposed
        )
    });
    let impact_events = run_until(&mut state, 100, |state, events| {
        state.building(victim).is_none()
            && events
                .iter()
                .any(|event| matches!(event, Event::BuildingDestroyed { building, .. } if *building == victim))
    });
    assert!(impact_events.iter().any(
        |event| matches!(event, Event::BuildingDestroyed { building, .. } if *building == victim)
    ));
    assert!(
        !state.can_see(PlayerId(0), state.unit(bombard).unwrap().tile()),
        "the loss records its own location without revealing the attacker"
    );

    state.tick(&[]);
    assert!(matches!(
        state.unit(worker).unwrap().order,
        Order::Harvest { retiring: true, .. }
    ));
    let events = run_until(&mut state, 800, |state, _| {
        state.unit(worker).unwrap().order == Order::Idle
    });
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ScrapDeposited { .. }))
    );
    assert!(state.map().scrap_at(exposed) > 0);
    assert!(state.map().wreck_at(struck) > 0);
}

#[test]
fn a_far_radar_contact_does_not_retire_an_unrelated_zone() {
    let source = TilePos::new(13, 5);
    let contact = TilePos::new(20, 5);
    assert!(contact.chebyshev(source) > HARVEST_RADAR_DANGER_RADIUS);
    let mut state = state_with_salvage(
        32,
        &[(source, 20)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 12, 5),
            unit(1, UnitKind::Scuttler, contact.x, contact.y),
        ],
        vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 5,
            y: 5,
        }],
    );
    let worker = state.units()[0].id;
    assert_eq!(state.vision(PlayerId(0)).contacts(), &[contact]);
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: source,
            queue: false,
        },
    )]);
    for _ in 0..25 {
        state.tick(&[]);
    }
    assert!(
        state.unit(worker).unwrap().carrying > 0
            && matches!(
                state.unit(worker).unwrap().order,
                Order::Harvest {
                    retiring: false,
                    ..
                }
            ),
        "a vague contact seven tiles away is not local danger"
    );
}

#[test]
fn an_explicit_anchor_remains_authoritative_inside_a_local_radar_contact() {
    let source = TilePos::new(18, 5);
    let contact = TilePos::new(22, 5);
    assert_eq!(contact.chebyshev(source), HARVEST_RADAR_DANGER_RADIUS);
    let mut state = state_with_salvage(
        34,
        &[(source, 20)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 3, 3),
            unit(1, UnitKind::Scuttler, contact.x, contact.y),
        ],
        vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 9,
            y: 5,
        }],
    );
    let worker = state.units()[0].id;
    assert_eq!(state.vision(PlayerId(0)).contacts(), &[contact]);
    assert!(!state.vision(PlayerId(0)).visible(contact));

    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: source,
            queue: false,
        },
    )]);
    assert!(
        matches!(
            state.unit(worker).unwrap().order,
            Order::Harvest {
                node,
                anchor: Some(work_anchor),
                retiring: false,
            } if node == source && work_anchor == source
        ),
        "danger filters autonomous chaining, not the source the commander explicitly named"
    );
    assert_eq!(state.map().scrap_at(source), 20);
}

#[test]
fn an_explicit_source_avoids_unrelated_known_danger_on_the_way() {
    let source = TilePos::new(24, 5);
    let contact = TilePos::new(13, 5);
    assert!(contact.chebyshev(source) > HARVEST_RADAR_DANGER_RADIUS);
    let mut state = state_with_salvage(
        36,
        &[(source, 20)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 3, 5),
            unit(0, UnitKind::Talon, 24, 4),
            unit(1, UnitKind::Scuttler, contact.x, contact.y),
        ],
        vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 2,
            y: 4,
        }],
    );
    let worker = state.units()[0].id;
    let scout = state.units()[1].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(3, 4),
            queue: false,
        },
    )]);
    run_until(&mut state, 500, |state, _| {
        state.unit(scout).unwrap().order == Order::Idle
    });
    assert_eq!(state.vision(PlayerId(0)).contacts(), &[contact]);
    assert!(!state.vision(PlayerId(0)).visible(contact));
    assert!(!state.vision(PlayerId(0)).visible(source));

    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: source,
            queue: false,
        },
    )]);
    let path = state
        .unit(worker)
        .unwrap()
        .path
        .as_ref()
        .expect("a safe explicit route exists around the radar envelope");
    assert!(
        path.waypoints
            .iter()
            .all(|tile| tile.chebyshev(contact) > HARVEST_RADAR_DANGER_RADIUS),
        "the clicked endpoint stays authoritative without making the route danger-blind: {path:?}"
    );
    assert!(
        path.waypoints.iter().any(|tile| tile.y == 10),
        "the deterministic route leaves the direct lane to clear the contact"
    );
}

#[test]
fn an_explicit_source_prefers_a_safe_doorstep_before_a_dangerous_fallback() {
    let source = TilePos::new(19, 5);
    let contact = TilePos::new(14, 5);
    assert_eq!(contact.chebyshev(source), HARVEST_RADAR_DANGER_RADIUS + 1);
    let mut state = state_with_salvage(
        32,
        &[(source, 20)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 3, 5),
            unit(0, UnitKind::Talon, 19, 4),
            unit(1, UnitKind::Scuttler, contact.x, contact.y),
        ],
        vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 2,
            y: 4,
        }],
    );
    let worker = state.units()[0].id;
    let scout = state.units()[1].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(3, 4),
            queue: false,
        },
    )]);
    run_until(&mut state, 500, |state, _| {
        state.unit(scout).unwrap().order == Order::Idle
    });
    assert_eq!(state.vision(PlayerId(0)).contacts(), &[contact]);
    assert!(!state.vision(PlayerId(0)).visible(source));

    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: source,
            queue: false,
        },
    )]);
    let path = state
        .unit(worker)
        .unwrap()
        .path
        .as_ref()
        .expect("the source has safe and dangerous doorsteps");
    assert!(
        path.goal.chebyshev(contact) > HARVEST_RADAR_DANGER_RADIUS,
        "a dangerous near-side doorstep is a fallback, not the first choice: {path:?}"
    );
    assert!(
        path.waypoints
            .iter()
            .all(|tile| tile.chebyshev(contact) > HARVEST_RADAR_DANGER_RADIUS),
        "the safe-first route must stay outside the contact envelope: {path:?}"
    );
}

#[test]
fn a_loaded_delivery_routes_around_known_danger() {
    let source = TilePos::new(24, 5);
    let contact = TilePos::new(13, 5);
    let mut state = state_with_salvage(
        36,
        &[(source, 20)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 23, 5),
            unit(1, UnitKind::Scuttler, contact.x, contact.y),
        ],
        vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 2,
            y: 4,
        }],
    );
    let worker = state.units()[0].id;
    assert_eq!(state.vision(PlayerId(0)).contacts(), &[contact]);
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: source,
            queue: false,
        },
    )]);
    let capacity = UnitKind::Harvester.stats().harvest.unwrap().capacity;
    state = set_cargo(state, worker, capacity);

    state.tick(&[]);
    let path = state
        .unit(worker)
        .unwrap()
        .path
        .as_ref()
        .expect("a safe delivery route exists around the contact");
    assert!(
        path.waypoints
            .iter()
            .all(|tile| tile.chebyshev(contact) > HARVEST_RADAR_DANGER_RADIUS),
        "the homeward leg must use the same danger envelope as outbound work: {path:?}"
    );
    assert!(
        path.waypoints.iter().any(|tile| tile.y == 10),
        "the delivery route leaves the direct lane to clear the contact"
    );
}

#[test]
fn an_unscouted_enemy_building_cannot_bend_a_delivery_route() {
    let source = TilePos::new(24, 5);
    let worker_spec = unit(0, UnitKind::Harvester, 23, 5);
    let clear = state_with_salvage(36, &[(source, 20)], &[], vec![worker_spec], vec![]);
    let obscured = state_with_salvage(
        36,
        &[(source, 20)],
        &[],
        vec![worker_spec],
        vec![BuildingSpec {
            player: 1,
            kind: BuildingKind::Reclaimer,
            x: 12,
            y: 4,
        }],
    );
    assert!(!obscured.vision(PlayerId(0)).visible(TilePos::new(12, 4)));
    assert!(obscured.vision(PlayerId(0)).ghosts().is_empty());

    let prepare = |mut state: State| {
        let worker = state.units()[0].id;
        state.tick(&[cmd(
            0,
            Command::Harvest {
                units: vec![worker],
                node: source,
                queue: false,
            },
        )]);
        let capacity = UnitKind::Harvester.stats().harvest.unwrap().capacity;
        state = set_cargo(state, worker, capacity);
        state.tick(&[]);
        state.unit(worker).unwrap().path.clone()
    };
    assert_eq!(
        prepare(obscured),
        prepare(clear),
        "homeward autonomy may react to remembered buildings, not hidden live truth"
    );
}

#[test]
fn a_remembered_armed_structure_blocks_an_autonomous_retarget_after_sight_is_lost() {
    let anchor = TilePos::new(13, 5);
    let fallback = TilePos::new(18, 5);
    let turret = TilePos::new(20, 5);
    let mut state = state_with_salvage(
        34,
        &[(anchor, 1), (fallback, 20)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 12, 5),
            unit(0, UnitKind::Talon, 18, 4),
        ],
        vec![BuildingSpec {
            player: 1,
            kind: BuildingKind::Turret,
            x: turret.x,
            y: turret.y,
        }],
    );
    let worker = state.units()[0].id;
    let scout = state.units()[1].id;
    assert!(
        state
            .vision(PlayerId(0))
            .ghosts()
            .iter()
            .any(|ghost| ghost.anchor == turret)
    );

    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(3, 4),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |state, _| {
        state.unit(scout).unwrap().order == Order::Idle
    });
    assert!(!state.vision(PlayerId(0)).visible(turret));
    assert_eq!(
        state.vision(PlayerId(0)).remembered_scrap(fallback),
        20,
        "the fallback remains legitimate frozen salvage memory"
    );

    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: anchor,
            queue: false,
        },
    )]);
    run_until(&mut state, 200, |state, _| {
        matches!(
            state.unit(worker).unwrap().order,
            Order::Harvest {
                node,
                anchor: Some(work_anchor),
                retiring: true,
            } if node == anchor && work_anchor == anchor
        )
    });
    assert_eq!(
        state.map().scrap_at(fallback),
        20,
        "static danger memory keeps the autonomous fallback out of the work contract"
    );
}

#[test]
fn an_autonomous_retarget_routes_around_known_danger() {
    let anchor = TilePos::new(10, 5);
    let fallback = TilePos::new(17, 5);
    let threat_tile = TilePos::new(13, 10);
    let mut state = state_with_salvage(
        36,
        &[(anchor, 1), (fallback, 20)],
        &[],
        vec![
            unit(0, UnitKind::Harvester, 9, 5),
            unit(0, UnitKind::Talon, 17, 4),
            unit(1, UnitKind::Sentinel, threat_tile.x, threat_tile.y),
        ],
        vec![],
    );
    let worker = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: anchor,
            queue: false,
        },
    )]);
    run_until(&mut state, 100, |state, _| {
        matches!(
            state.unit(worker).unwrap().order,
            Order::Harvest {
                node,
                anchor: Some(work_anchor),
                retiring: false,
            } if node == fallback && work_anchor == anchor
        ) && state.unit(worker).unwrap().path.is_some()
    });
    let path = state
        .unit(worker)
        .unwrap()
        .path
        .as_ref()
        .expect("a safe detour exists around the radar envelope");
    let threat_reach = UnitKind::Sentinel.stats().weapons[0].range + HARVEST_MOBILE_DANGER_MARGIN;
    assert!(
        path.waypoints
            .iter()
            .all(|tile| tile.center().dist_sq(threat_tile.center()) > threat_reach * threat_reach),
        "every autonomous waypoint avoids the visible threat envelope: {path:?}"
    );
    assert!(
        path.waypoints
            .iter()
            .any(|tile| (tile.y - threat_tile.y).abs() >= 6),
        "the deterministic route leaves the direct lane to clear the threat"
    );
}

#[test]
fn never_seen_live_salvage_is_not_a_command_oracle() {
    let hidden = TilePos::new(18, 5);
    let mut state = state_with_salvage(
        28,
        &[(hidden, 5)],
        &[],
        vec![unit(0, UnitKind::Harvester, 3, 3)],
        vec![],
    );
    let worker = state.units()[0].id;
    assert!(!state.vision(PlayerId(0)).explored(hidden));
    assert_eq!(state.map().scrap_at(hidden), 5);

    let report = state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: hidden,
            queue: false,
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NotANode,
    }));
    assert_eq!(state.unit(worker).unwrap().order, Order::Idle);
}

#[test]
fn legacy_harvest_orders_default_the_anchor_to_their_current_node() {
    let order: Order = serde_json::from_value(json!({
        "order": "harvest",
        "node": {"x": 7, "y": 5}
    }))
    .unwrap();
    assert_eq!(
        order,
        Order::Harvest {
            node: TilePos::new(7, 5),
            anchor: None,
            retiring: false,
        }
    );
}
