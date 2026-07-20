//! Layer-2 tests: headless scenarios asserting game behavior through the
//! public API only — build a scenario, feed tick-stamped commands, watch
//! events and state. No renderer anywhere near this file.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::{
    BuildingId, Command, Event, Faction, GameResult, Order, PlayerCommand, PlayerId, Scenario,
    State, Target, UnitId, UnitKind,
};

/// A small arena: two Foundries in opposite corners, open ground between.
fn arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "test-arena".into(),
        seed: 42,
        map: vec![
            "################".into(),
            "#1.............#".into(),
            "#..............#".into(),
            "#.....##.......#".into(),
            "#.....##...s...#".into(),
            "#..........s...#".into(),
            "#............2.#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                scrap: 200,
                bot: false,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                scrap: 200,
                bot: false,
            },
        ],
        units,
    }
}

fn unit(player: u8, kind: UnitKind, x: i32, y: i32) -> UnitSpec {
    UnitSpec { player, kind, x, y }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

/// Runs until `stop` returns true or `max_ticks` elapse, collecting every
/// event. Panics if the condition never holds — behavior tests should state
/// exactly how long something may take.
fn run_until(
    state: &mut State,
    max_ticks: u64,
    mut stop: impl FnMut(&State, &[Event]) -> bool,
) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..max_ticks {
        let report = state.tick(&[]);
        let done = stop(state, &report.events);
        all.extend(report.events);
        if done {
            return all;
        }
    }
    panic!("condition not reached within {max_ticks} ticks");
}

#[test]
fn move_command_walks_unit_to_goal_then_idles() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 2)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let goal = TilePos::new(13, 2);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal,
        },
    )]);
    run_until(&mut state, 200, |s, _| {
        let u = s.unit(mover).unwrap();
        u.tile() == goal && u.order == Order::Idle
    });
}

#[test]
fn move_routes_around_rock() {
    // Goal sits directly behind the 2x2 rock at (6,3)-(7,4).
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 4)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let goal = TilePos::new(9, 4);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal,
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        s.unit(mover).unwrap().tile() == goal
    });
}

#[test]
fn move_goal_on_rock_snaps_to_nearby_ground() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 2)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal: TilePos::new(6, 3), // rock
        },
    )]);
    run_until(&mut state, 300, |s, _| {
        let u = s.unit(mover).unwrap();
        u.order == Order::Idle && u.tile().chebyshev(TilePos::new(6, 3)) <= 1
    });
}

#[test]
fn harvester_gathers_and_deposits() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 3, 2)])
        .build()
        .unwrap();
    let worker = state.units()[0].id;
    let scrap_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: TilePos::new(11, 4),
        },
    )]);
    // Walk there (~10 tiles), extract 10 scrap (100 ticks), walk home, drop.
    let events = run_until(&mut state, 600, |s, _| {
        s.player(PlayerId(0)).scrap > scrap_before
    });
    assert!(events.iter().any(
        |e| matches!(e, Event::ScrapDeposited { player: PlayerId(0), amount } if *amount > 0)
    ));
    // Still on the job: the node isn't empty, so back to work.
    assert!(matches!(
        state.unit(worker).unwrap().order,
        Order::Harvest { .. }
    ));
}

#[test]
fn attack_command_kills_and_reports() {
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 4, 6),
        unit(1, UnitKind::Harvester, 6, 6),
    ])
    .build()
    .unwrap();
    let (attacker, victim) = (state.units()[0].id, state.units()[1].id);
    // The first hit can land on the command tick itself, so keep its events.
    let mut events = state
        .tick(&[cmd(
            0,
            Command::Attack {
                units: vec![attacker],
                target: Target::Unit(victim),
            },
        )])
        .events;
    // 60 hp / 10 damage at 1 hit per second → 6 hits, ~101 ticks + travel.
    events.extend(run_until(&mut state, 200, |s, _| s.unit(victim).is_none()));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == victim))
    );
    assert!(
        events
            .iter()
            .filter(|e| matches!(e, Event::AttackHit { .. }))
            .count()
            >= 6
    );
}

#[test]
fn attack_move_engages_on_the_way_then_resumes() {
    // A wider arena than `arena()`: the enemy Foundry must sit outside
    // aggro range of the march route, or the marcher will (correctly)
    // besiege it instead of arriving.
    let scenario = Scenario {
        name: "attack-move-lane".into(),
        seed: 42,
        map: vec![
            "####################".into(),
            "#1.................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#................2.#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: arena(vec![]).players,
        units: vec![
            unit(0, UnitKind::Sentinel, 2, 4),
            unit(1, UnitKind::Harvester, 8, 6),
        ],
    };
    let mut state = scenario.build().unwrap();
    let (marcher, bystander) = (state.units()[0].id, state.units()[1].id);
    // The bystander starts outside aggro (~5.7 tiles) but sits ~2 tiles off
    // the route: the marcher must engage mid-march, kill it, then still
    // arrive and stand down.
    let goal = TilePos::new(12, 4);
    state.tick(&[cmd(
        0,
        Command::AttackMove {
            units: vec![marcher],
            goal,
        },
    )]);
    run_until(&mut state, 400, |s, _| s.unit(bystander).is_none());
    run_until(&mut state, 400, |s, _| {
        let u = s.unit(marcher).unwrap();
        u.tile() == goal && u.order == Order::Idle
    });
}

#[test]
fn attack_move_with_only_harvesters_degrades_to_move() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 2)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let goal = TilePos::new(10, 2);
    state.tick(&[cmd(
        0,
        Command::AttackMove {
            units: vec![mover],
            goal,
        },
    )]);
    assert!(matches!(
        state.unit(mover).unwrap().order,
        Order::Move { .. }
    ));
    run_until(&mut state, 200, |s, _| {
        s.unit(mover).unwrap().tile() == goal
    });
}

#[test]
fn units_ordered_to_one_tile_do_not_stack() {
    // Four harvesters converge on the same goal; collision resolution must
    // keep every pair at least (r_a + r_b) apart once things settle.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 3, 2),
        unit(0, UnitKind::Harvester, 12, 2),
        unit(0, UnitKind::Harvester, 3, 6),
        unit(0, UnitKind::Harvester, 12, 6),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Move {
            units: ids,
            goal: TilePos::new(8, 4),
        },
    )]);
    for _ in 0..300 {
        state.tick(&[]);
    }
    let min_gap = UnitKind::Harvester.stats().radius * 2;
    // Allow a whisker of tolerance: the final relaxation pass may leave a
    // sub-ulp of residual overlap.
    let tolerance = min_gap * chassis::fx::Fx::lit("0.9");
    for (i, a) in state.units().iter().enumerate() {
        for b in state.units().iter().skip(i + 1) {
            assert!(
                a.pos.dist_sq(b.pos) >= tolerance * tolerance,
                "{} and {} overlap: {:?} vs {:?}",
                a.id,
                b.id,
                a.pos,
                b.pos
            );
        }
    }
}

#[test]
fn collision_never_pushes_through_rock() {
    // Two units squeezed against the rock at (6,3)-(7,4): however hard the
    // crowd pushes, nobody ends up inside it.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 5, 3),
        unit(0, UnitKind::Harvester, 5, 4),
        unit(0, UnitKind::Harvester, 4, 3),
        unit(0, UnitKind::Harvester, 4, 4),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Move {
            units: ids,
            goal: TilePos::new(5, 3),
        },
    )]);
    for _ in 0..200 {
        state.tick(&[]);
        for u in state.units() {
            assert!(
                state.map().terrain_passable(u.tile()),
                "{} was pushed into impassable {:?}",
                u.id,
                u.tile()
            );
        }
    }
}

#[test]
fn congested_harvesters_keep_depositing() {
    // Six harvesters on one node, one Foundry: the deadlock regression.
    // Symmetric collision cancellation once froze exactly this setup with
    // full loads at the doorstep. Progress must continue, not just start.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 2),
        unit(0, UnitKind::Harvester, 5, 2),
        unit(0, UnitKind::Harvester, 6, 2),
        unit(0, UnitKind::Harvester, 4, 5),
        unit(0, UnitKind::Harvester, 5, 5),
        unit(0, UnitKind::Harvester, 5, 6),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: ids,
            node: TilePos::new(11, 4),
        },
    )]);

    let mut deposited_first_half = 0u32;
    let mut deposited_second_half = 0u32;
    for tick in 0..3000u64 {
        let report = state.tick(&[]);
        for event in &report.events {
            if let Event::ScrapDeposited { amount, .. } = event {
                if tick < 1500 {
                    deposited_first_half += amount;
                } else {
                    deposited_second_half += amount;
                }
            }
        }
    }
    assert!(
        deposited_first_half >= 60,
        "economy never started: {deposited_first_half}"
    );
    assert!(
        deposited_second_half >= 60,
        "economy stalled after starting: {deposited_second_half}"
    );
}

#[test]
fn bot_economy_progresses_against_an_idle_opponent() {
    // The exact configuration that froze pre-fix: shipped skirmish, human
    // seat idle, Cupric bot alone. Its bank plus spending must exceed its
    // starting stake — deposits happened — well before 12k ticks.
    let scenario = Scenario::skirmish();
    let mut state = scenario.build().unwrap();
    let mut bots = oxide_sim::bot::Bot::for_scenario(&scenario);
    assert_eq!(bots.len(), 1, "skirmish ships with exactly one bot seat");

    let mut deposited = 0u32;
    for _ in 0..12_000u64 {
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        let report = state.tick(&commands);
        for event in &report.events {
            if let Event::ScrapDeposited {
                player: PlayerId(1),
                amount,
            } = event
            {
                deposited += amount;
            }
        }
        if deposited >= 300 {
            return; // healthy economy, no need to run the rest
        }
    }
    panic!("bot deposited only {deposited} scrap in 12k ticks — economy stalled");
}

#[test]
fn rock_is_cover_until_the_attacker_repositions() {
    // Attacker and victim sit exactly 2 tiles apart — inside range 2.5 —
    // with a 1-thick rock wall between them. Without LOS the first shot
    // would land on the command tick from the starting tile; with it, the
    // attacker must first walk around either end of the wall.
    let scenario = Scenario {
        name: "cover".into(),
        seed: 42,
        map: vec![
            "############".into(),
            "#1.........#".into(),
            "#....#.....#".into(),
            "#....#.....#".into(),
            "#....#.....#".into(),
            "#........2.#".into(),
            "#..........#".into(),
            "############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![
            unit(0, UnitKind::Sentinel, 4, 3),
            unit(1, UnitKind::Harvester, 6, 3),
        ],
    };
    let mut state = scenario.build().unwrap();
    let (attacker, victim) = (state.units()[0].id, state.units()[1].id);
    let start_tile = state.unit(attacker).unwrap().tile();
    let mut first_hit_tile = None;
    let mut events = state
        .tick(&[cmd(
            0,
            Command::Attack {
                units: vec![attacker],
                target: Target::Unit(victim),
            },
        )])
        .events;
    for _ in 0..600 {
        if first_hit_tile.is_none() && events.iter().any(|e| matches!(e, Event::AttackHit { .. })) {
            first_hit_tile = Some(state.unit(attacker).unwrap().tile());
        }
        if state.unit(victim).is_none() {
            break;
        }
        events = state.tick(&[]).events;
    }
    assert!(state.unit(victim).is_none(), "victim must eventually die");
    let hit_tile = first_hit_tile.expect("a hit must have been observed");
    assert_ne!(
        hit_tile, start_tile,
        "the first shot must come from a repositioned tile — firing through \
         the rock means LOS failed"
    );
}

#[test]
fn group_moves_fan_out_over_distinct_tiles() {
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 3, 2),
        unit(0, UnitKind::Harvester, 4, 2),
        unit(0, UnitKind::Harvester, 3, 6),
        unit(0, UnitKind::Harvester, 4, 6),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Move {
            units: ids.clone(),
            goal: TilePos::new(10, 4),
        },
    )]);
    for _ in 0..400 {
        state.tick(&[]);
    }
    // Everyone settled…
    assert!(
        state.units().iter().all(|u| u.order == Order::Idle),
        "group should settle: {:?}",
        state.units().iter().map(|u| u.order).collect::<Vec<_>>()
    );
    // …near the click, on distinct goals (spread), without stacking.
    let mut tiles: Vec<TilePos> = state.units().iter().map(|u| u.tile()).collect();
    for t in &tiles {
        assert!(
            t.chebyshev(TilePos::new(10, 4)) <= 3,
            "unit parked too far from the group goal: {t}"
        );
    }
    tiles.sort_unstable();
    tiles.dedup();
    assert!(
        tiles.len() >= 3,
        "group order must fan out, not stack on one tile"
    );
}

#[test]
fn idle_sentinel_auto_acquires_intruder() {
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 4, 6),
        unit(1, UnitKind::Harvester, 8, 6), // 4 tiles away, inside aggro 5
    ])
    .build()
    .unwrap();
    let victim = state.units()[1].id;
    // No command at all: the sentinel should pick the fight itself.
    run_until(&mut state, 300, |s, _| s.unit(victim).is_none());
}

#[test]
fn train_costs_scrap_and_spawns_after_build_time() {
    let mut state = arena(vec![]).build().unwrap();
    let foundry = state.buildings()[0].id;
    let before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    )]);
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        before - UnitKind::Harvester.stats().cost,
        "cost is deducted on enqueue"
    );
    let events = run_until(&mut state, 105, |s, _| !s.units().is_empty());
    assert!(events.iter().any(|e| matches!(
        e,
        Event::UnitTrained {
            kind: UnitKind::Harvester,
            player: PlayerId(0),
            ..
        }
    )));
}

#[test]
fn rally_routes_fresh_units_by_role() {
    // A bystander parked in sight of the node: rallies read the owner's
    // remembered scrap, so somebody must have actually seen it.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 8, 4)])
        .build()
        .unwrap();
    let foundry = state.buildings()[0].id;
    // Rally onto the scrap node: fresh harvesters go straight to work.
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
    let scrap_before = state.player(PlayerId(0)).scrap;
    run_until(&mut state, 700, |s, _| {
        s.player(PlayerId(0)).scrap > scrap_before
    });

    // Rally onto open ground: fresh sentinels attack-move there.
    state.tick(&[
        cmd(
            0,
            Command::SetRally {
                building: foundry,
                rally: Some(TilePos::new(10, 6)),
            },
        ),
        cmd(
            0,
            Command::Train {
                building: foundry,
                kind: UnitKind::Sentinel,
            },
        ),
    ]);
    run_until(&mut state, 300, |s, _| {
        s.units().iter().any(|u| {
            u.kind == UnitKind::Sentinel
                && matches!(u.order, Order::AttackMove { .. } | Order::Attack { .. })
        })
    });
}

#[test]
fn rally_on_foreign_building_is_rejected() {
    let mut state = arena(vec![]).build().unwrap();
    let theirs = state.buildings()[1].id;
    let report = state.tick(&[cmd(
        0,
        Command::SetRally {
            building: theirs,
            rally: Some(TilePos::new(5, 5)),
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NotYourBuilding,
    }));
    assert_eq!(state.building(theirs).unwrap().rally, None);
}

#[test]
fn train_rejects_poverty_and_foreign_buildings() {
    let mut state = arena(vec![]).build().unwrap();
    let (mine, theirs) = (state.buildings()[0].id, state.buildings()[1].id);

    // Not my building.
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: theirs,
            kind: UnitKind::Sentinel,
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NotYourBuilding,
    }));

    // Not enough scrap: 200 banked, sentinels cost 75 → third fails.
    for _ in 0..2 {
        let r = state.tick(&[cmd(
            0,
            Command::Train {
                building: mine,
                kind: UnitKind::Sentinel,
            },
        )]);
        assert!(
            !r.events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. }))
        );
    }
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: mine,
            kind: UnitKind::Sentinel,
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(0),
        reason: RejectReason::NotEnoughScrap,
    }));
}

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
        scrap: 0,
        bot: false,
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
            },
            Command::AttackMove {
                units: vec![u],
                goal,
            },
            Command::Harvest {
                units: vec![u],
                node: goal,
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
        scrap: 0,
        bot: false,
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
        },
    )]);
    assert!(report.events.contains(&Event::CommandRejected {
        player: PlayerId(1),
        reason: RejectReason::Eliminated,
    }));
}

#[test]
fn deposits_saturate_a_full_bank() {
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 10, 3)]);
    scenario.players[0].scrap = u32::MAX - 5;
    let mut state = scenario.build().unwrap();
    let worker = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: TilePos::new(11, 4),
        },
    )]);
    run_until(&mut state, 800, |s, _| {
        s.player(PlayerId(0)).scrap == u32::MAX
    });
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
        },
    )]);
    // 800 hp / 30 dps → ~27 s ≈ 540 ticks, plus approach.
    let events = run_until(&mut state, 800, |s, _| s.result().is_some());
    assert_eq!(
        state.result(),
        Some(GameResult::Victory {
            winner: PlayerId(0)
        })
    );
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
    assert_eq!(
        state.result(),
        Some(GameResult::Victory {
            winner: PlayerId(0)
        })
    );
}

#[test]
fn congestion_survives_nonconsecutive_unit_ids() {
    // Doorstep rotation and pair iteration both walk unit ids; this run
    // punches holes in the sequence first. Three mid-id harvesters spawn
    // beside an enemy sentinel and die to its auto-acquire, then the
    // survivors (ids 0, 1, 4, 6, 7) crowd one node — the economy must keep
    // flowing exactly as it does with dense ids.
    let scenario = Scenario {
        name: "id-gaps".into(),
        seed: 42,
        map: vec![
            "########################".into(),
            "#1..........s..........#".into(),
            "#...........s..........#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#......................#".into(),
            "#..................2...#".into(),
            "#......................#".into(),
            "########################".into(),
        ],
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                scrap: 0,
                bot: false,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                scrap: 0,
                bot: false,
            },
        ],
        units: vec![
            unit(0, UnitKind::Harvester, 4, 1),
            unit(0, UnitKind::Harvester, 5, 1),
            unit(0, UnitKind::Harvester, 19, 4), // victim
            unit(0, UnitKind::Harvester, 20, 4), // victim
            unit(0, UnitKind::Harvester, 4, 2),
            unit(0, UnitKind::Harvester, 19, 5), // victim
            unit(0, UnitKind::Harvester, 6, 1),
            unit(0, UnitKind::Harvester, 5, 2),
            unit(1, UnitKind::Sentinel, 20, 5),
        ],
    };
    let mut state = scenario.build().unwrap();

    let mut dead = 0;
    run_until(&mut state, 1500, |_, events| {
        dead += events
            .iter()
            .filter(|e| matches!(e, Event::UnitDied { .. }))
            .count();
        dead == 3
    });
    let survivors: Vec<UnitId> = state
        .units()
        .iter()
        .filter(|u| u.player == PlayerId(0))
        .map(|u| u.id)
        .collect();
    assert_eq!(
        survivors,
        [0, 1, 4, 6, 7].map(UnitId),
        "the wrong harvesters died"
    );

    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: survivors,
            node: TilePos::new(12, 1),
        },
    )]);
    let mut deposited = [0u32; 2];
    for tick in 0..3000u64 {
        let report = state.tick(&[]);
        for event in &report.events {
            if let Event::ScrapDeposited { amount, .. } = event {
                deposited[(tick >= 1500) as usize] += amount;
            }
        }
    }
    assert!(deposited[0] >= 50, "economy never started: {deposited:?}");
    assert!(
        deposited[1] >= 50,
        "economy stalled with gapped ids: {deposited:?}"
    );
}

#[test]
fn dense_stacks_respect_the_per_pass_displacement_cap() {
    // 100 units spawned on one tile. Per-pair clamping once let a unit in
    // k overlaps drift k × COLLISION_MAX_STEP in a single tick (measured
    // 1.8+ tiles); the budget is per unit per pass, so one tick may move
    // nobody farther than COLLISION_ITERATIONS × COLLISION_MAX_STEP.
    let units = (0..100)
        .map(|_| unit(0, UnitKind::Harvester, 8, 3))
        .collect();
    let mut state = arena(units).build().unwrap();
    let before: Vec<_> = state.units().iter().map(|u| (u.id, u.pos)).collect();
    state.tick(&[]);
    let cap = oxide_sim::stats::COLLISION_MAX_STEP * 3; // COLLISION_ITERATIONS
    for (id, start) in before {
        let now = state.unit(id).unwrap().pos;
        let moved = (now - start).length_sq();
        assert!(
            moved <= cap * cap,
            "{id} moved {moved:?}² in one tick (cap {cap:?})"
        );
    }
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
            },
        ),
        cmd(
            1,
            Command::Harvest {
                units: vec![miner],
                node,
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
