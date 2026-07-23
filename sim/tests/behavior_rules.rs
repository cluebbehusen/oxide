//! Fog, memory, command validation, and match rules — behavior suite, public API only.

mod common;

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::PlayerSpec;
use oxide_sim::{
    BuildingId, Command, Event, Faction, GameResult, Order, PlayerId, Scenario, Target, UnitId,
    UnitKind,
};

use common::*;

#[test]
fn fog_reveals_persists_and_gates_attacks() {
    // Harvester scouts on both sides: nobody can shoot, so vision changes
    // come purely from walking. Foundries sit in opposite corners; the
    // arena is wide enough that each side starts blind to the other.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 3, 3),
        unit(1, UnitKind::Harvester, 13, 3),
    ])
    .build()
    .unwrap();
    let (scout, quarry) = (state.units()[0].id, state.units()[1].id);
    let quarry_tile = state.unit(quarry).unwrap().tile();
    assert!(
        !state.can_see(PlayerId(0), quarry_tile),
        "enemy corner must start fogged"
    );

    // Attacking through fog is rejected outright.
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![scout],
            target: Target::Unit(quarry),
            queue: false,
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::InvalidTarget,
    }));

    // Scouting toward it brings it into view…
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(9, 3),
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| s.can_see(PlayerId(0), quarry_tile));

    // …and walking home drops visibility but keeps the exploration.
    let home = TilePos::new(2, 6);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: home,
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        s.unit(scout).unwrap().tile() == home
    });
    assert!(!state.can_see(PlayerId(0), quarry_tile));
    assert!(state.vision(PlayerId(0)).explored(quarry_tile));
}

#[test]
fn ghost_memory_survives_unseen_demolition_until_revisited() {
    // Three players: p0 scouts p1's Foundry, walks home, then p2's
    // sentinels (parked next door) demolish it while p0 isn't looking.
    // p0's memory must keep the ghost until the ground is seen again.
    let mut players = arena(vec![]).players;
    players.push(PlayerSpec {
        name: "Third".into(),
        faction: Faction::Ferrous,
        team: None,
        scrap: 0,
        bot: false,
        bot_config: None,
    });
    let scenario = Scenario {
        name: "ghost-lab".into(),
        seed: 7,
        map: vec![
            "################".into(),
            "#1.............#".into(),
            "#..............#".into(),
            "#..............#".into(),
            "#..............#".into(),
            "#..............#".into(),
            "#..............#".into(),
            "#..............#".into(),
            "#..............#".into(),
            "#..............#".into(),
            "#..3.....2.....#".into(),
            "#..............#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players,
        units: vec![
            unit(0, UnitKind::Harvester, 4, 2),
            unit(2, UnitKind::Sentinel, 7, 10),
            unit(2, UnitKind::Sentinel, 7, 11),
        ],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let scout = state.units()[0].id;
    let victim_anchor = TilePos::new(9, 10);
    let me = PlayerId(0);

    // Scout down to see p1's Foundry (harvester vision 6).
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(9, 5),
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        s.vision(me)
            .ghosts()
            .iter()
            .any(|g| g.anchor == victim_anchor)
    });

    // Walk home, out of sight of that corner.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(4, 2),
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        s.unit(scout).unwrap().tile() == TilePos::new(4, 2)
    });

    // p2's sentinels raze the Foundry on their own (auto-acquire).
    run_until(&mut state, 2000, |s, _| {
        !s.buildings().iter().any(|b| b.player == PlayerId(1))
    });
    assert!(
        state
            .vision(me)
            .ghosts()
            .iter()
            .any(|g| g.anchor == victim_anchor && g.owner == PlayerId(1)),
        "p0 didn't see the demolition, so the ghost must persist"
    );

    // Revisit: seeing the empty ground erases the memory.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(9, 5),
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        !s.vision(me)
            .ghosts()
            .iter()
            .any(|g| g.anchor == victim_anchor)
    });
}

#[test]
fn remembered_scrap_freezes_when_sight_is_lost() {
    // p0 scouts the node at (11,4), walks home; p1 mines it unseen. p0's
    // memory must keep the full amount until the ground is re-seen.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(1, UnitKind::Harvester, 13, 3),
    ])
    .build()
    .unwrap();
    let (scout, miner) = (state.units()[0].id, state.units()[1].id);
    let node = TilePos::new(11, 4);
    let me = PlayerId(0);
    let full = oxide_sim::stats::SCRAP_NODE_AMOUNT;

    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(9, 4),
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        s.vision(me).remembered_scrap(node) == full
    });

    let home = TilePos::new(4, 2);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: home,
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        s.unit(scout).unwrap().tile() == home && !s.can_see(me, node)
    });

    // Unseen mining: live amount drops, p0's memory doesn't.
    state.tick(&[cmd(
        1,
        Command::Harvest {
            units: vec![miner],
            node,
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |s, _| s.map().scrap_at(node) < full - 4);
    assert_eq!(
        state.vision(me).remembered_scrap(node),
        full,
        "memory must freeze at the last sighting"
    );

    // Re-scouting reconciles memory with reality.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(9, 4),
            queue: false,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        s.can_see(me, node) && s.vision(me).remembered_scrap(node) == s.map().scrap_at(node)
    });
    assert!(state.vision(me).remembered_scrap(node) < full);
}

#[test]
fn hostile_coordinates_are_rejected_not_panicked() {
    // Extreme i32 goals once overflowed the neighborhood scan's offset
    // arithmetic (a debug-build panic from one malformed debug-socket
    // command). Running this test in a debug profile IS the assertion.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 2)])
        .build()
        .unwrap();
    let u = state.units()[0].id;
    let foundry = state.buildings()[0].id;
    for goal in [
        TilePos::new(i32::MAX, 0),
        TilePos::new(0, i32::MIN),
        TilePos::new(i32::MAX, i32::MAX),
    ] {
        for command in [
            Command::Move {
                units: vec![u],
                goal,
                queue: false,
            },
            Command::AttackMove {
                units: vec![u],
                goal,
                queue: false,
            },
            Command::Harvest {
                units: vec![u],
                node: goal,
                queue: false,
            },
            Command::SetRally {
                building: foundry,
                rally: Some(goal),
            },
        ] {
            let report = state.tick(&[cmd(0, command)]);
            assert!(
                report.events.contains(&Event::CommandRejected {
                    player: PlayerId(0),
                    reason: RejectReason::OutOfBounds,
                }),
                "goal {goal} must be rejected"
            );
        }
    }
    assert_eq!(state.unit(u).unwrap().order, Order::Idle);
}

#[test]
fn eliminated_players_cannot_command_survivors() {
    // Three players; p1's foundry falls while its harvester lives on.
    let mut players = arena(vec![]).players;
    players.push(PlayerSpec {
        name: "Third".into(),
        faction: Faction::Ferrous,
        team: None,
        scrap: 0,
        bot: false,
        bot_config: None,
    });
    let scenario = Scenario {
        name: "elimination".into(),
        seed: 3,
        map: vec![
            "##############".into(),
            "#1...........#".into(),
            "#............#".into(),
            "#..3.....2...#".into(),
            "#............#".into(),
            "#............#".into(),
            "##############".into(),
        ],
        players,
        units: vec![
            unit(1, UnitKind::Harvester, 12, 1),
            unit(2, UnitKind::Sentinel, 7, 3),
            unit(2, UnitKind::Sentinel, 7, 4),
        ],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let survivor = state.units()[0].id;
    // p2's sentinels raze p1's foundry on their own.
    run_until(&mut state, 3000, |s, _| {
        !s.buildings().iter().any(|b| b.player == PlayerId(1))
    });
    assert!(state.result().is_none(), "two players remain — play on");
    let report = state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![survivor],
            goal: TilePos::new(5, 5),
            queue: false,
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(1),
        reason: RejectReason::Eliminated,
    }));
}

#[test]
fn commanding_enemy_units_is_rejected() {
    let mut state = arena(vec![unit(1, UnitKind::Harvester, 8, 6)])
        .build()
        .unwrap();
    let enemy_unit = state.units()[0].id;
    let report = state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![enemy_unit],
            goal: TilePos::new(2, 2),
            queue: false,
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NoValidUnits,
    }));
    assert_eq!(state.unit(enemy_unit).unwrap().order, Order::Idle);
}

#[test]
fn destroying_the_last_foundry_wins_and_freezes() {
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 10, 6),
        unit(0, UnitKind::Sentinel, 11, 6),
        unit(0, UnitKind::Sentinel, 10, 5),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    let enemy_foundry: BuildingId = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1))
        .unwrap()
        .id;
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: ids,
            target: Target::Building(enemy_foundry),
            queue: false,
        },
    )]);
    // 800 hp / 30 dps → ~27 s ≈ 540 ticks, plus approach.
    let events = run_until(&mut state, 800, |s, _| s.result().is_some());
    assert_eq!(state.result(), Some(GameResult::Victory { team: 0 }));
    assert!(events.iter().any(|e| matches!(e, Event::GameOver { .. })));

    // Frozen: ticks advance, nothing else changes. (State fields are
    // private now, so compare the world piecewise instead of patching the
    // tick counter and hashing.)
    let tick_before = state.current_tick();
    let units_before = state.units().to_vec();
    let buildings_before = state.buildings().to_vec();
    let players_before = state.players().to_vec();
    state.tick(&[]);
    assert_eq!(state.current_tick(), tick_before + 1);
    assert_eq!(state.units(), units_before.as_slice());
    assert_eq!(state.buildings(), buildings_before.as_slice());
    assert_eq!(state.players(), players_before.as_slice());
    assert_eq!(state.result(), Some(GameResult::Victory { team: 0 }));
}

#[test]
fn rally_on_unexplored_scrap_does_not_probe_the_map() {
    // The arena node at (11,4) sits outside the Foundry's vision and no
    // unit has ever seen it. A rally there must read as plain ground —
    // a harvesting newborn would leak that hidden scrap exists.
    let mut state = arena(vec![]).build().unwrap();
    let foundry = state.buildings()[0].id;
    assert!(
        !state.vision(PlayerId(0)).explored(TilePos::new(11, 4)),
        "test premise: the rally tile must be unexplored"
    );
    state.tick(&[
        cmd(
            0,
            Command::SetRally {
                building: foundry,
                rally: Some(TilePos::new(11, 4)),
            },
        ),
        cmd(
            0,
            Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        ),
    ]);
    let events = run_until(&mut state, 200, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitTrained { .. }))
    });
    let newborn = events
        .iter()
        .find_map(|e| match e {
            Event::UnitTrained { unit, .. } => Some(*unit),
            _ => None,
        })
        .unwrap();
    assert!(
        matches!(state.unit(newborn).unwrap().order, Order::Move { .. }),
        "newborn should walk to unexplored ground, not clairvoyantly harvest"
    );
}

#[test]
fn rally_trusts_remembered_scrap_even_when_it_is_stale() {
    // Player 0 scouts the node, loses sight, and player 1 mines it dry.
    // The rally still believes the memory: the newborn honestly walks out
    // to harvest and will discover the truth on arrival.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 8, 4), // scout, sees (11,4)
        unit(1, UnitKind::Harvester, 12, 5),
    ])
    .build()
    .unwrap();
    let (scout, miner) = (state.units()[0].id, state.units()[1].id);
    let node = TilePos::new(11, 4);
    assert!(state.vision(PlayerId(0)).remembered_scrap(node) > 0);
    // Scout retreats out of sight; the enemy strips the node bare.
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![scout],
                goal: TilePos::new(2, 3),
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Harvest {
                units: vec![miner],
                node,
                queue: false,
            },
        ),
    ]);
    run_until(&mut state, 12_000, |s, _| s.map().scrap_at(node) == 0);
    assert!(
        state.vision(PlayerId(0)).remembered_scrap(node) > 0,
        "memory must have frozen before depletion"
    );

    let foundry = state.buildings()[0].id;
    state.tick(&[
        cmd(
            0,
            Command::SetRally {
                building: foundry,
                rally: Some(node),
            },
        ),
        cmd(
            0,
            Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        ),
    ]);
    let events = run_until(&mut state, 200, |_, events| {
        events.iter().any(|e| {
            matches!(
                e,
                Event::UnitTrained {
                    player: PlayerId(0),
                    ..
                }
            )
        })
    });
    let newborn = events
        .iter()
        .find_map(|e| match e {
            Event::UnitTrained {
                unit,
                player: PlayerId(0),
                ..
            } => Some(*unit),
            _ => None,
        })
        .unwrap();
    // The rally honored the memory and issued Harvest. (The harvest brain
    // may already have retargeted a neighboring node — its depleted-node
    // replacement scan is a separate, order-wide behavior — but under the
    // old live-map rule the newborn would have gotten a plain Move.)
    assert!(
        matches!(state.unit(newborn).unwrap().order, Order::Harvest { .. }),
        "stale belief should be acted on honestly, not silently corrected"
    );
}

#[test]
fn losing_the_last_foundry_ends_the_match_despite_other_buildings() {
    use oxide_sim::stats::BuildingKind;
    // Player 1 stands up a turret, then loses its Foundry. The turret
    // must not keep it in the game — survival means a Foundry.
    let mut state = arena(vec![
        unit(1, UnitKind::Harvester, 12, 2),
        unit(0, UnitKind::Sentinel, 4, 6),
        unit(0, UnitKind::Sentinel, 5, 7),
        unit(0, UnitKind::Sentinel, 4, 7),
    ])
    .build()
    .unwrap();
    let builder = state.units()[0].id;
    let anchor = TilePos::new(12, 1);
    state.tick(&[cmd(
        1,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    run_until(&mut state, 700, |s, _| {
        s.buildings().iter().any(|b| b.anchor == anchor && b.built)
    });
    // Raze the foundry (attack-move onto it; fire-at-will besieges).
    let attackers: Vec<UnitId> = state
        .units()
        .iter()
        .filter(|u| u.player == PlayerId(0) && u.kind == UnitKind::Sentinel)
        .map(|u| u.id)
        .collect();
    let foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1))
        .filter(|b| b.kind == oxide_sim::BuildingKind::Foundry)
        .unwrap()
        .id;
    state.tick(&[cmd(
        0,
        Command::AttackMove {
            units: attackers,
            goal: TilePos::new(13, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 4000, |s, _| s.building(foundry).is_none());
    assert_eq!(
        state.result(),
        Some(GameResult::Victory { team: 0 }),
        "a standing turret must not keep an eliminated player alive"
    );
}

#[test]
#[ignore]
fn probe_mirror_divergence() {
    use oxide_sim::bot::Bot;
    // Perfect mirror premise: symmetric map, same-tick thinking is NOT
    // possible via public API (cadence is seat-staggered), so drive BOTH
    // seats with the same bot logic manually mirrored... instead: run the
    // real bots and report the first tick where unit multisets stop being
    // 180-degree mirror images (position+kind+hp).
    let mut scenario = Scenario::skirmish();
    scenario.seed = 424242; // both thresholds roll 6
    scenario.players[0].bot = true;
    scenario.players[1].bot = true;
    let mut state = scenario.build().unwrap();
    let mut bots = Bot::for_scenario(&scenario);
    let (w, h) = (state.map().width(), state.map().height());
    for tick in 0..30_000u32 {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        state.tick(&commands);
        // Mirror check: every p0 unit must have a p1 twin at the mirrored
        // position with the same kind and hp.
        let mut p1_units: Vec<(i64, i64, oxide_sim::UnitKind, u32)> = state
            .units()
            .iter()
            .filter(|u| u.player == PlayerId(1))
            .map(|u| {
                (
                    (chassis::fx::Fx::from_num(w) - u.pos.x).to_bits(),
                    (chassis::fx::Fx::from_num(h) - u.pos.y).to_bits(),
                    u.kind,
                    u.hp,
                )
            })
            .collect();
        let mut asym = Vec::new();
        for u in state.units().iter().filter(|u| u.player == PlayerId(0)) {
            let key = (u.pos.x.to_bits(), u.pos.y.to_bits(), u.kind, u.hp);
            if let Some(i) = p1_units.iter().position(|t| *t == key) {
                p1_units.swap_remove(i);
            } else {
                asym.push((u.id, u.kind, u.tile(), u.hp));
            }
        }
        if !asym.is_empty() || !p1_units.is_empty() {
            println!("FIRST DIVERGENCE at tick {tick}");
            println!("unmatched p0: {asym:?}");
            println!("unmatched p1 (mirrored keys left): {}", p1_units.len());
            for u in state.units() {
                println!(
                    "  u{} p{} {:?} {:?} hp{} order {:?}",
                    u.id.0,
                    u.player.0,
                    u.kind,
                    u.tile(),
                    u.hp,
                    u.order
                );
            }
            return;
        }
        if state.result().is_some() {
            println!("game ended at {tick} while still symmetric??");
            return;
        }
    }
    println!("fully symmetric for 30k ticks");
}
