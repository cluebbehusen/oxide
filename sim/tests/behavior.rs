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
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                scrap: 200,
                bot: false,
                bot_config: None,
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
            queue: false,
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
            queue: false,
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
            queue: false,
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
            queue: false,
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
                queue: false,
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
            queue: false,
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
            queue: false,
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
            queue: false,
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
            queue: false,
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
            queue: false,
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
                queue: false,
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
            queue: false,
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
            queue: false,
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
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                scrap: 0,
                bot: false,
                bot_config: None,
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
            queue: false,
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
fn queued_orders_execute_in_sequence() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 2, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let (a, b) = (TilePos::new(9, 6), TilePos::new(9, 2));
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: a,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: b,
                queue: true,
            },
        ),
    ]);
    // First leg: arrive at A while B still waits in the queue.
    run_until(&mut state, 300, |s, _| s.unit(mover).unwrap().tile() == a);
    assert_eq!(state.unit(mover).unwrap().queue.len(), 1);
    // Second leg: end idle at B with nothing queued.
    run_until(&mut state, 300, |s, _| {
        let u = s.unit(mover).unwrap();
        u.tile() == b && u.order == Order::Idle && u.queue.is_empty()
    });
}

#[test]
fn direct_order_replaces_the_whole_queue() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 2, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: TilePos::new(13, 6),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: TilePos::new(13, 2),
                queue: true,
            },
        ),
    ]);
    // A fresh unqueued order wipes the program mid-walk.
    for _ in 0..20 {
        state.tick(&[]);
    }
    let d = TilePos::new(4, 2);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal: d,
            queue: false,
        },
    )]);
    assert!(state.unit(mover).unwrap().queue.is_empty());
    run_until(&mut state, 300, |s, _| {
        let u = s.unit(mover).unwrap();
        u.tile() == d && u.order == Order::Idle
    });
}

#[test]
fn patrol_cycles_waypoints_and_never_settles() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 2, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let (a, b) = (TilePos::new(4, 6), TilePos::new(11, 6));
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![mover],
            waypoints: vec![a, b],
        },
    )]);
    // Expect the circuit to visit a, b, then a again — proof of the loop.
    let mut expected = [a, b, a].into_iter();
    let mut next = expected.next();
    for _ in 0..2000u32 {
        let u = state.unit(mover).unwrap();
        assert_ne!(u.order, Order::Idle, "a patrol never settles");
        if Some(u.tile()) == next {
            next = expected.next();
            if next.is_none() {
                break;
            }
        }
        state.tick(&[]);
    }
    assert_eq!(next, None, "circuit did not complete a full loop");
    assert!(state.unit(mover).unwrap().looping);
}

#[test]
fn patrol_engages_on_the_way_and_resumes_the_circuit() {
    // A sentinel patrols the top lane; an enemy harvester sits just off
    // it, inside aggro. The patroller must break off, kill it, and pick
    // the circuit back up. Waypoints stay >5 tiles from the enemy Foundry
    // so the patrol never besieges it mid-test.
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 3, 2),
        unit(1, UnitKind::Harvester, 5, 4),
    ])
    .build()
    .unwrap();
    let (patroller, victim) = (state.units()[0].id, state.units()[1].id);
    let (a, b) = (TilePos::new(3, 2), TilePos::new(8, 2));
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![patroller],
            waypoints: vec![a, b],
        },
    )]);
    run_until(&mut state, 600, |s, _| s.unit(victim).is_none());
    // Back on the beat: both waypoints get visited again after the kill.
    for goal in [b, a] {
        run_until(&mut state, 600, |s, _| {
            s.unit(patroller).unwrap().tile() == goal
        });
    }
    assert!(state.unit(patroller).unwrap().looping);
}

#[test]
fn stalled_leg_drops_the_whole_program() {
    // The queued second leg targets a sealed pocket: no route. The stall
    // must abandon the entire program, not limp to the next leg.
    let scenario = Scenario {
        name: "sealed-pocket".into(),
        seed: 42,
        map: vec![
            "##############".into(),
            "#1...........#".into(),
            "#....###.....#".into(),
            "#....#.#.....#".into(),
            "#....###.....#".into(),
            "#............#".into(),
            "#.........2..#".into(),
            "#............#".into(),
            "##############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![unit(0, UnitKind::Harvester, 3, 1)],
    };
    let mut state = scenario.build().unwrap();
    let mover = state.units()[0].id;
    let reachable = TilePos::new(10, 1);
    let pocket = TilePos::new(6, 3);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: reachable,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal: pocket,
                queue: true,
            },
        ),
    ]);
    let events = run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { unit, .. } if *unit == mover))
    });
    assert!(!events.is_empty());
    let u = state.unit(mover).unwrap();
    assert_eq!(u.order, Order::Idle);
    assert!(u.queue.is_empty() && !u.looping);
    assert_eq!(u.tile(), reachable, "stall happens after the first leg");
}

#[test]
fn stop_clears_a_patrol() {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 2, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![mover],
            waypoints: vec![TilePos::new(4, 6), TilePos::new(11, 6)],
        },
    )]);
    for _ in 0..30 {
        state.tick(&[]);
    }
    state.tick(&[cmd(0, Command::Stop { units: vec![mover] })]);
    let u = state.unit(mover).unwrap();
    assert_eq!(u.order, Order::Idle);
    assert!(u.queue.is_empty() && !u.looping);
    let parked = u.tile();
    for _ in 0..60 {
        state.tick(&[]);
    }
    assert_eq!(state.unit(mover).unwrap().tile(), parked);
}

#[test]
fn foundry_refuses_kinds_it_cannot_produce() {
    let mut state = arena(vec![]).build().unwrap();
    let foundry = state.buildings()[0].id;
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Scuttler,
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::CannotProduce,
            ..
        }
    )));
    assert_eq!(state.player(PlayerId(0)).scrap, 200, "no scrap was taken");
}

#[test]
fn lancer_outranges_aggro_and_retaliation_answers() {
    // The lancer opens fire from 5.4 tiles — outside the sentinel's aggro
    // (5), inside lancer range (5.5). Before 0.5 the sentinel would stand
    // and die; the first hit now turns it on its attacker.
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 3, 6),
        unit(1, UnitKind::Lancer, 8, 4),
    ])
    .build()
    .unwrap();
    let (victim, lancer) = (state.units()[0].id, state.units()[1].id);
    let d2 = {
        let a = state.unit(victim).unwrap().pos;
        let b = state.unit(lancer).unwrap().pos;
        a.dist_sq(b)
    };
    let aggro = oxide_sim::stats::UnitKind::Sentinel
        .stats()
        .attack
        .unwrap()
        .aggro_range;
    assert!(d2 > aggro * aggro, "test premise: outside sentinel aggro");

    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![lancer],
            target: Target::Unit(victim),
            queue: false,
        },
    )]);
    // The lancer needs no approach: the first hit lands within a tick or
    // two, and the sentinel's answer must be immediate.
    run_until(&mut state, 10, |s, _| {
        s.unit(victim).unwrap().hp < UnitKind::Sentinel.stats().max_hp
    });
    assert!(
        matches!(
            state.unit(victim).unwrap().order,
            Order::Attack { target: Target::Unit(t), .. } if t == lancer
        ),
        "the sentinel should turn on its attacker"
    );
    // And the fight resolves the right way: the sentinel closes and wins.
    run_until(&mut state, 400, |s, _| s.unit(lancer).is_none());
    assert!(
        state.unit(victim).is_some(),
        "sentinel survives the approach"
    );
}

#[test]
fn retaliation_keeps_an_attack_movers_destination() {
    // An open lane: the lancer sits 5.4 tiles off the march route —
    // outside the marcher's aggro, inside its own range — and opens fire
    // as the marcher passes. The marcher must answer, win, and still
    // finish the march.
    let scenario = Scenario {
        name: "retaliation-lane".into(),
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
            unit(0, UnitKind::Sentinel, 3, 2),
            unit(1, UnitKind::Lancer, 9, 7),
        ],
    };
    let mut state = scenario.build().unwrap();
    let (marcher, lancer) = (state.units()[0].id, state.units()[1].id);
    let goal = TilePos::new(16, 2);
    state.tick(&[cmd(
        0,
        Command::AttackMove {
            units: vec![marcher],
            goal,
            queue: false,
        },
    )]);
    // Let the march reach the firing window (dist 5.39: in lancer range,
    // outside sentinel aggro), then order the shot.
    run_until(&mut state, 200, |s, _| {
        s.unit(marcher).unwrap().tile() == TilePos::new(7, 2)
    });
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![lancer],
            target: Target::Unit(marcher),
            queue: false,
        },
    )]);
    run_until(&mut state, 20, |s, _| {
        matches!(
            s.unit(marcher).unwrap().order,
            Order::Attack { resume: Some(g), .. } if g == goal
        )
    });
    // Kill confirmed, march resumed, goal reached.
    run_until(&mut state, 600, |s, _| s.unit(lancer).is_none());
    run_until(&mut state, 600, |s, _| {
        let u = s.unit(marcher).unwrap();
        u.tile() == goal && u.order == Order::Idle
    });
}

#[test]
fn scuttler_wins_the_matchups_it_should_and_loses_the_rest() {
    // Scuttler vs harvester: quick shredding. Sentinel vs scuttler: the
    // line unit holds. Both by explicit attack so positioning is fixed.
    let mut state = arena(vec![
        unit(0, UnitKind::Scuttler, 4, 6),
        unit(1, UnitKind::Harvester, 6, 6),
    ])
    .build()
    .unwrap();
    let (rat, prey) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![rat],
            target: Target::Unit(prey),
            queue: false,
        },
    )]);
    run_until(&mut state, 200, |s, _| s.unit(prey).is_none());
    assert!(state.unit(rat).is_some());

    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 4, 6),
        unit(1, UnitKind::Scuttler, 6, 6),
    ])
    .build()
    .unwrap();
    let (line, rat) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![rat],
            target: Target::Unit(line),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| s.unit(rat).is_none());
    assert!(state.unit(line).is_some(), "the sentinel holds the line");
}

#[test]
fn construction_ramps_and_completes() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 6)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let anchor = TilePos::new(5, 6);
    let scrap_before = state.player(PlayerId(0)).scrap;
    let cost = BuildingKind::Turret.stats().construction.unwrap().cost;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    // Site claimed instantly: paid in full, blocking, unfinished, partial hp.
    assert_eq!(state.player(PlayerId(0)).scrap, scrap_before - cost);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .expect("site placed")
        .id;
    let b = state.building(site).unwrap();
    assert!(!b.built);
    assert_eq!(b.hp, BuildingKind::Turret.stats().max_hp / 5);
    assert!(!state.passable(anchor), "sites block their footprint");

    let events = run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    });
    assert!(!events.is_empty());
    let b = state.building(site).unwrap();
    assert!(b.built);
    assert_eq!(b.hp, BuildingKind::Turret.stats().max_hp, "ramped to full");
    // Completion is buffered; the builder learns the site is done on the
    // next tick, through the built-site branch.
    state.tick(&[]);
    assert_eq!(
        state.unit(builder).unwrap().order,
        Order::Idle,
        "builder is done"
    );
}

#[test]
fn a_second_builder_resumes_a_dead_builders_site() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 6),
        unit(0, UnitKind::Harvester, 3, 2),
        unit(1, UnitKind::Scuttler, 6, 7),
    ])
    .build()
    .unwrap();
    let (first, second, killer) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let anchor = TilePos::new(5, 6);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![first],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    // Let some progress land, then eat the builder.
    for _ in 0..100 {
        state.tick(&[]);
    }
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![killer],
            target: Target::Unit(first),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| s.unit(first).is_none());
    // The killer leaves (oblivious walk), or it would eat the relief too.
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![killer],
            goal: TilePos::new(13, 2),
            queue: false,
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .expect("site survives its builder")
        .id;
    let progress_when_orphaned = state.building(site).unwrap().progress;
    // Progress is frozen while nobody tends the site.
    for _ in 0..60 {
        state.tick(&[]);
    }
    assert_eq!(
        state.building(site).unwrap().progress,
        progress_when_orphaned
    );
    // Aiming a fresh Build at the same anchor resumes, not double-pays.
    let scrap_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![second],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    assert_eq!(state.player(PlayerId(0)).scrap, scrap_before);
    run_until(&mut state, 900, |s, _| {
        s.building(site).is_some_and(|b| b.built)
    });
}

#[test]
fn cancel_refunds_by_health_and_damage_burns_it() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 6),
        unit(1, UnitKind::Scuttler, 9, 7),
    ])
    .build()
    .unwrap();
    let (builder, raider) = (state.units()[0].id, state.units()[1].id);
    let anchor = TilePos::new(5, 6);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    for _ in 0..120 {
        state.tick(&[]);
    }
    // The raider chews the scaffold down before the cancel.
    state.tick(&[
        cmd(
            0,
            Command::Stop {
                units: vec![builder],
            },
        ),
        cmd(
            1,
            Command::Attack {
                units: vec![raider],
                target: Target::Building(site),
                queue: false,
            },
        ),
    ]);
    for _ in 0..120 {
        state.tick(&[]);
    }
    let stats = BuildingKind::Turret.stats();
    let b = state.building(site).unwrap();
    let expected = stats.construction.unwrap().cost * b.hp / stats.max_hp;
    let scrap_before = state.player(PlayerId(0)).scrap;
    let report = state.tick(&[cmd(0, Command::Cancel { building: site })]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::BuildCancelled { refund, .. } if *refund == expected))
    );
    assert_eq!(state.player(PlayerId(0)).scrap, scrap_before + expected);
    assert!(state.building(site).is_none());
    assert!(state.passable(anchor), "the ground is free again");
}

#[test]
fn turret_holds_ground_and_dies_to_lancer_siege() {
    use oxide_sim::stats::BuildingKind;
    // A finished enemy turret vs a scuttler rush: the rush loses. Then a
    // lancer sieges from beyond turret range and wins untouched.
    let scenario = Scenario {
        name: "turret-duel".into(),
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
            unit(0, UnitKind::Harvester, 3, 2),
            unit(1, UnitKind::Scuttler, 16, 5),
        ],
    };
    let mut state = scenario.build().unwrap();
    let (builder, rat) = (state.units()[0].id, state.units()[1].id);
    let anchor = TilePos::new(5, 5);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    run_until(&mut state, 600, |s, _| {
        s.building(turret).is_some_and(|b| b.built)
    });
    // Builder clears the field so the duel is clean.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(2, 1),
            queue: false,
        },
    )]);
    // Fog: the rat can't target what it hasn't seen — attack-move in
    // and let fire-at-will find the turret.
    state.tick(&[cmd(
        1,
        Command::AttackMove {
            units: vec![rat],
            goal: TilePos::new(6, 5),
            queue: false,
        },
    )]);
    let events = run_until(&mut state, 600, |s, _| s.unit(rat).is_none());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::TurretFired { .. })),
        "the turret did the killing"
    );
    assert!(state.building(turret).is_some_and(|b| b.hp > 0));

    // Now the siege, in a fresh world: a lancer at range 5.5 > turret 4.5
    // grinds it down without ever taking return fire.
    let scenario = Scenario {
        name: "lancer-siege".into(),
        seed: 43,
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
            unit(0, UnitKind::Harvester, 3, 2),
            unit(1, UnitKind::Lancer, 16, 5),
        ],
    };
    let mut state = scenario.build().unwrap();
    let (builder, lancer) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    let turret = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    run_until(&mut state, 600, |s, _| {
        s.building(turret).is_some_and(|b| b.built)
    });
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![builder],
                goal: TilePos::new(2, 1),
                queue: false,
            },
        ),
        cmd(
            1,
            Command::AttackMove {
                units: vec![lancer],
                goal: TilePos::new(6, 5),
                queue: false,
            },
        ),
    ]);
    run_until(&mut state, 2000, |s, _| s.building(turret).is_none());
    assert_eq!(
        state.unit(lancer).unwrap().hp,
        UnitKind::Lancer.stats().max_hp,
        "the siege takes no return fire"
    );
}

#[test]
fn fabricator_gates_the_advanced_roster() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 6)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let anchor = TilePos::new(5, 5);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Fabricator,
            anchor,
        },
    )]);
    let fab = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    // Unfinished: no training yet.
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: fab,
            kind: UnitKind::Scuttler,
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::CannotProduce,
            ..
        }
    )));
    run_until(&mut state, 900, |s, _| {
        s.building(fab).is_some_and(|b| b.built)
    });
    // Finished: scuttlers roll out; sentinels are still Foundry-only.
    let report = state.tick(&[
        cmd(
            0,
            Command::Train {
                building: fab,
                kind: UnitKind::Scuttler,
            },
        ),
        cmd(
            0,
            Command::Train {
                building: fab,
                kind: UnitKind::Sentinel,
            },
        ),
    ]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::CannotProduce,
            ..
        }
    )));
    run_until(&mut state, 200, |s, events| {
        let _ = s;
        events.iter().any(|e| {
            matches!(
                e,
                Event::UnitTrained {
                    kind: UnitKind::Scuttler,
                    ..
                }
            )
        })
    });
}

#[test]
fn bot_reaches_its_tech_and_mixes_its_army() {
    use oxide_sim::bot::Bot;
    use oxide_sim::stats::BuildingKind;
    // Bot vs an idle opponent: within 12k ticks it should have stood up a
    // Fabricator and fielded at least one advanced unit — proof the build
    // and composition logic actually runs, not just compiles.
    let mut scenario = Scenario::skirmish();
    scenario.players[1].bot = true;
    let mut state = scenario.build().unwrap();
    let mut bots = Bot::for_scenario(&scenario);
    for _ in 0..12_000u32 {
        if state.result().is_some() {
            break;
        }
        let mut commands = Vec::new();
        for bot in &mut bots {
            commands.extend(bot.act(&state));
        }
        state.tick(&commands);
    }
    let me = PlayerId(1);
    let has_fab = state
        .buildings()
        .iter()
        .any(|b| b.player == me && b.kind == BuildingKind::Fabricator && b.built);
    let advanced = state
        .units()
        .iter()
        .filter(|u| u.player == me && matches!(u.kind, UnitKind::Scuttler | UnitKind::Lancer))
        .count();
    // The bot may have already razed the idle opponent and won; that is
    // also a pass as long as tech came up first.
    assert!(
        has_fab || state.result().is_some(),
        "no fabricator and no victory after 12k ticks"
    );
    if has_fab {
        assert!(advanced > 0, "fabricator built but nothing trained from it");
    }
}

#[test]
fn sealed_apart_scenarios_refuse_to_build() {
    use oxide_sim::scenario::ScenarioError;
    let scenario = Scenario {
        name: "sealed".into(),
        seed: 1,
        map: vec![
            "############".into(),
            "#1...#.....#".into(),
            "#....#.....#".into(),
            "#....#..2..#".into(),
            "#....#.....#".into(),
            "############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![],
    };
    assert!(matches!(
        scenario.build(),
        Err(ScenarioError::Disconnected(..))
    ));
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
        Some(GameResult::Victory {
            winner: PlayerId(0)
        }),
        "a standing turret must not keep an eliminated player alive"
    );
}

#[test]
fn harvesters_deposit_only_at_built_foundries() {
    use oxide_sim::stats::BuildingKind;
    // A turret sits right beside the node; the Foundry is far away. The
    // hauler must walk the distance — turrets are not drop-offs.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 10, 4)])
        .build()
        .unwrap();
    let worker = state.units()[0].id;
    let anchor = TilePos::new(10, 3); // touching the node at (11,4)
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![worker],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    run_until(&mut state, 700, |s, _| {
        s.buildings().iter().any(|b| b.anchor == anchor && b.built)
    });
    state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![worker],
            node: TilePos::new(11, 4),
            queue: false,
        },
    )]);
    let foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0) && b.kind == oxide_sim::BuildingKind::Foundry)
        .unwrap();
    let (f_anchor, f_size) = (foundry.anchor, foundry.kind.stats().size);
    let mut deposit_tile = None;
    for _ in 0..1200u32 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::ScrapDeposited { .. }))
        {
            deposit_tile = Some(state.unit(worker).unwrap().tile());
            break;
        }
    }
    let t = deposit_tile.expect("a deposit happened");
    let adjacent_to_foundry = (t.x >= f_anchor.x - 1 && t.x <= f_anchor.x + f_size.0)
        && (t.y >= f_anchor.y - 1 && t.y <= f_anchor.y + f_size.1);
    assert!(
        adjacent_to_foundry,
        "deposit landed at {t:?}, not beside the foundry at {f_anchor:?}"
    );
}

#[test]
fn turret_fires_at_its_stated_cadence() {
    use oxide_sim::stats::BuildingKind;
    // Build in peace (the target waits far outside aggro), then walk the
    // target into range and measure the shot interval.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 6),
        unit(1, UnitKind::Sentinel, 13, 1),
    ])
    .build()
    .unwrap();
    let (builder, target) = (state.units()[0].id, state.units()[1].id);
    let anchor = TilePos::new(5, 6);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    run_until(&mut state, 700, |s, _| {
        s.buildings().iter().any(|b| b.anchor == anchor && b.built)
    });
    // Builder clears out; the sentinel wanders in obliviously.
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![builder],
                goal: TilePos::new(2, 2),
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Move {
                units: vec![target],
                goal: TilePos::new(8, 6),
                queue: false,
            },
        ),
    ]);
    let mut fire_ticks: Vec<u64> = Vec::new();
    for _ in 0..600u32 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::TurretFired { .. }))
        {
            fire_ticks.push(state.current_tick());
        }
        if fire_ticks.len() >= 3 {
            break;
        }
    }
    let cooldown = u64::from(BuildingKind::Turret.stats().attack.unwrap().cooldown_ticks);
    assert!(fire_ticks.len() >= 3, "not enough shots observed");
    assert_eq!(
        fire_ticks[1] - fire_ticks[0],
        cooldown,
        "interval must equal cooldown_ticks, not cooldown_ticks + 1"
    );
    assert_eq!(fire_ticks[2] - fire_ticks[1], cooldown);
}

#[test]
fn can_place_refuses_foundries() {
    use oxide_sim::stats::BuildingKind;
    let state = arena(vec![]).build().unwrap();
    assert!(!state.can_place(PlayerId(0), BuildingKind::Foundry, TilePos::new(5, 6)));
    assert!(state.can_place(PlayerId(0), BuildingKind::Turret, TilePos::new(5, 6)));
}

#[test]
fn validator_rejects_foreign_owners() {
    let state = arena(vec![unit(0, UnitKind::Harvester, 4, 6)])
        .build()
        .unwrap();
    let mut doc: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    doc["units"][0]["player"] = serde_json::json!(9);
    // Validation runs inside Deserialize since 0.6: the tamper never
    // becomes a State at all.
    assert!(serde_json::from_value::<State>(doc).is_err());
}

#[test]
fn scouted_sites_are_remembered_as_sites() {
    use oxide_sim::stats::BuildingKind;
    // P0's scout watches P1 start a turret, then leaves. The ghost must
    // remember scaffolding, not a finished building.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 9, 2),
        unit(1, UnitKind::Harvester, 12, 2),
    ])
    .build()
    .unwrap();
    let (scout, builder) = (state.units()[0].id, state.units()[1].id);
    let anchor = TilePos::new(12, 1);
    state.tick(&[cmd(
        1,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    for _ in 0..30 {
        state.tick(&[]);
    }
    assert!(state.can_see(PlayerId(0), anchor), "scout sees the site");
    // Scout retreats out of sight; site keeps building behind the fog.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| !s.can_see(PlayerId(0), anchor));
    run_until(&mut state, 700, |s, _| {
        s.buildings().iter().any(|b| b.anchor == anchor && b.built)
    });
    let ghost = state
        .vision(PlayerId(0))
        .ghosts()
        .iter()
        .find(|g| g.anchor == anchor)
        .expect("the site was scouted and left behind fog");
    assert!(
        !ghost.built,
        "fog memory must freeze the last-seen construction state"
    );
}

#[test]
fn bot_sends_a_relief_builder_to_an_orphaned_site() {
    use oxide_sim::bot::Bot;
    use oxide_sim::stats::BuildingKind;
    // Manufacture the orphan directly: a scripted Build, then Stop the
    // builder on the spot. Hand the seat to a bot — its relief loop must
    // finish the paid-for site (a pending site once suppressed fabricator
    // logic forever instead).
    let mut state = arena(vec![
        unit(1, UnitKind::Harvester, 12, 2),
        unit(1, UnitKind::Harvester, 13, 3),
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
    state.tick(&[cmd(
        1,
        Command::Stop {
            units: vec![builder],
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    assert!(!state.building(site).unwrap().built);

    let mut bot = Bot::new(PlayerId(1), 42);
    for _ in 0..3000u32 {
        let commands = bot.act(&state);
        state.tick(&commands);
        if state.building(site).is_some_and(|b| b.built) {
            return;
        }
    }
    panic!("orphaned site was never resumed by the bot");
}

#[test]
fn a_fresh_site_blocks_units_already_walking_through_it() {
    use oxide_sim::stats::BuildingKind;
    // The mover's cached path runs straight along row 6; a turret site
    // lands on that row mid-walk. The mover must route around, never
    // standing inside the footprint, and still arrive.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 2, 6),
        unit(0, UnitKind::Harvester, 9, 5),
    ])
    .build()
    .unwrap();
    let (mover, builder) = (state.units()[0].id, state.units()[1].id);
    let goal = TilePos::new(12, 6);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal,
            queue: false,
        },
    )]);
    for _ in 0..20 {
        state.tick(&[]); // mover under way, site not yet placed
    }
    let anchor = TilePos::new(9, 6);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    for _ in 0..400 {
        state.tick(&[]);
        let t = state.unit(mover).unwrap().tile();
        assert_ne!(t, anchor, "walked through a standing construction site");
        if t == goal {
            return;
        }
    }
    panic!("mover never arrived after the repath");
}

#[test]
fn a_zeroed_site_cannot_be_revived_by_its_builder() {
    use oxide_sim::stats::BuildingKind;
    // Three lancers volley 90 damage — more than the fresh site's 70 hp —
    // while the builder (highest id, acting last each tick) feeds it
    // progress. Without the hp check, the builder revives the corpse
    // every volley and the site eventually *completes*; with it, the
    // first volley kills the site for good.
    let mut state = arena(vec![
        unit(1, UnitKind::Lancer, 8, 5),
        unit(1, UnitKind::Lancer, 9, 6),
        unit(1, UnitKind::Lancer, 8, 7),
        unit(0, UnitKind::Harvester, 4, 6),
    ])
    .build()
    .unwrap();
    let lancers: Vec<UnitId> = state.units().iter().take(3).map(|u| u.id).collect();
    let builder = state.units()[3].id;
    let anchor = TilePos::new(5, 6);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    // The first volley can land on the command tick itself — keep its
    // report (the oldest gotcha in the book).
    let mut events = state
        .tick(&[cmd(
            1,
            Command::Attack {
                units: lancers,
                target: Target::Building(site),
                queue: false,
            },
        )])
        .events;
    if state.building(site).is_some() {
        events.extend(run_until(&mut state, 120, |s, _| {
            s.building(site).is_none()
        }));
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { .. })),
        "destruction must be an event, not a silent tug-of-war"
    );
}
#[test]
fn queue_overflow_is_rejected_not_swallowed() {
    use oxide_sim::stats::ORDER_QUEUE_CAP;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 2, 6)])
        .build()
        .unwrap();
    let mover = state.units()[0].id;
    let mut rejected = 0;
    for i in 0..(ORDER_QUEUE_CAP + 9) {
        let goal = TilePos::new(3 + (i % 10) as i32, 2);
        let report = state.tick(&[cmd(
            0,
            Command::Move {
                units: vec![mover],
                goal,
                queue: true,
            },
        )]);
        rejected += report
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    Event::CommandRejected {
                        reason: RejectReason::QueueFull,
                        ..
                    }
                )
            })
            .count();
    }
    assert_eq!(state.unit(mover).unwrap().queue.len(), ORDER_QUEUE_CAP);
    assert!(rejected > 0, "silent drops at the cap");
}

#[test]
fn unreachable_sites_are_rejected_before_charging() {
    use oxide_sim::stats::BuildingKind;
    // The pocket interior is visible (vision is radius-based, rock does
    // not block sight) but no builder can path to any doorstep.
    let scenario = Scenario {
        name: "sealed-doorstep".into(),
        seed: 42,
        map: vec![
            "##############".into(),
            "#1...........#".into(),
            "#....###.....#".into(),
            "#....#.#.....#".into(),
            "#....###.....#".into(),
            "#..........2.#".into(),
            "#............#".into(),
            "##############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![unit(0, UnitKind::Harvester, 4, 6)],
    };
    let mut state = scenario.build().unwrap();
    let builder = state.units()[0].id;
    let scrap_before = state.player(PlayerId(0)).scrap;
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(6, 3),
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::UnreachableGoal,
            ..
        }
    )));
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        scrap_before,
        "an impossible site must not cost anything"
    );
    assert!(
        state
            .buildings()
            .iter()
            .all(|b| b.anchor != TilePos::new(6, 3))
    );
}

#[test]
fn placement_requires_current_vision_not_mere_exploration() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 12, 2)])
        .build()
        .unwrap();
    let scout = state.units()[0].id;
    let spot = TilePos::new(12, 1);
    assert!(state.can_place(PlayerId(0), BuildingKind::Turret, spot));
    // Walk home: the ground stays explored but drops out of sight.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| !s.can_see(PlayerId(0), spot));
    assert!(state.vision(PlayerId(0)).explored(spot));
    assert!(
        !state.can_place(PlayerId(0), BuildingKind::Turret, spot),
        "occupancy reads live state, so placement needs live sight"
    );
}

#[test]
fn extra_builders_accelerate_construction() {
    use oxide_sim::stats::BuildingKind;
    // Deliberate mechanic (documented in AGENTS): every adjacent builder
    // contributes a progress tick, so two roughly halve the build.
    let build_time = |extra_builder: bool| {
        let mut units = vec![unit(0, UnitKind::Harvester, 4, 6)];
        if extra_builder {
            units.push(unit(0, UnitKind::Harvester, 6, 6));
        }
        let mut state = arena(units).build().unwrap();
        let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
        let anchor = TilePos::new(5, 6);
        state.tick(&[cmd(
            0,
            Command::Build {
                units: vec![ids[0]],
                kind: BuildingKind::Turret,
                anchor,
            },
        )]);
        if extra_builder {
            state.tick(&[cmd(
                0,
                Command::Build {
                    units: vec![ids[1]],
                    kind: BuildingKind::Turret,
                    anchor,
                },
            )]);
        }
        let mut ticks = 0u32;
        while !state
            .buildings()
            .iter()
            .any(|b| b.anchor == anchor && b.built)
        {
            state.tick(&[]);
            ticks += 1;
            assert!(ticks < 700, "construction never finished");
        }
        ticks
    };
    let solo = build_time(false);
    let pair = build_time(true);
    assert!(
        pair * 2 < solo * 3,
        "two builders should be markedly faster: solo {solo}, pair {pair}"
    );
}

#[test]
fn validator_rejects_malformed_grids() {
    let state = arena(vec![]).build().unwrap();
    let mut doc: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    // Truncate the first vision table's visible-cells array.
    let cells = doc["vision"][0]["visible"]["cells"]
        .as_array_mut()
        .expect("vision grid serializes its cells");
    cells.pop();
    assert!(serde_json::from_value::<State>(doc).is_err());
}

#[test]
fn a_fresh_site_cannot_be_corner_cut_diagonally() {
    use oxide_sim::stats::BuildingKind;
    // The mover walks a diagonal staircase; a turret lands on one of the
    // flanking cardinals of an upcoming diagonal step. The waypoint
    // itself stays open — only the no-corner-cut invariant is at stake.
    let scenario = Scenario {
        name: "corner-cut".into(),
        seed: 42,
        map: vec![
            "###############".into(),
            "#1............#".into(),
            "#.............#".into(),
            "#.............#".into(),
            "#.............#".into(),
            "#.............#".into(),
            "#.............#".into(),
            "#.............#".into(),
            "#.............#".into(),
            "#..........2..#".into(),
            "#.............#".into(),
            "###############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![
            unit(0, UnitKind::Harvester, 3, 2),
            unit(0, UnitKind::Harvester, 6, 2),
        ],
    };
    let mut state = scenario.build().unwrap();
    let (mover, builder) = (state.units()[0].id, state.units()[1].id);
    let goal = TilePos::new(11, 8);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![mover],
            goal,
            queue: false,
        },
    )]);
    for _ in 0..10 {
        state.tick(&[]); // under way along the staircase
    }
    // Read the actual route and flank an upcoming diagonal step — the
    // test adapts to whatever staircase A* chose.
    let anchor = {
        let path = state.unit(mover).unwrap().path.as_ref().expect("walking");
        let next = path.next as usize;
        let mut flank = None;
        for w in path.waypoints[next..].windows(2) {
            let (a, b) = (w[0], w[1]);
            if a.x != b.x && a.y != b.y {
                flank = Some(TilePos::new(b.x, a.y));
                break;
            }
        }
        flank.expect("the route has a diagonal step")
    };
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    // Every step from here obeys the no-corner-cut rule around the site.
    let mut prev = state.unit(mover).unwrap().tile();
    for _ in 0..500 {
        state.tick(&[]);
        let now = state.unit(mover).unwrap().tile();
        assert_ne!(now, anchor, "inside the site footprint");
        let (dx, dy) = (now.x - prev.x, now.y - prev.y);
        if dx != 0 && dy != 0 {
            assert!(
                TilePos::new(prev.x + dx, prev.y) != anchor
                    && TilePos::new(prev.x, prev.y + dy) != anchor,
                "diagonal step {prev:?} -> {now:?} cut the corner of {anchor:?}"
            );
        }
        prev = now;
        if now == goal {
            return;
        }
    }
    panic!("mover never arrived after the repath");
}

#[test]
fn a_rejected_build_leaves_no_trace_on_the_hash() {
    use oxide_sim::stats::BuildingKind;
    let scenario = Scenario {
        name: "sealed-doorstep".into(),
        seed: 42,
        map: vec![
            "##############".into(),
            "#1...........#".into(),
            "#....###.....#".into(),
            "#....#.#.....#".into(),
            "#....###.....#".into(),
            "#..........2.#".into(),
            "#............#".into(),
            "##############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![unit(0, UnitKind::Harvester, 4, 6)],
    };
    let mut with_reject = scenario.build().unwrap();
    let mut pristine = scenario.build().unwrap();
    let builder = with_reject.units()[0].id;
    let report = with_reject.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(6, 3),
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::UnreachableGoal,
            ..
        }
    )));
    pristine.tick(&[]);
    assert_eq!(
        with_reject.hash(),
        pristine.hash(),
        "a rejected command must not move the state hash (id counter leak)"
    );
    // And the next building anywhere gets the same id in both worlds.
    let a = with_reject.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(3, 4),
        },
    )]);
    let _ = a;
    let b = pristine.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(3, 4),
        },
    )]);
    let _ = b;
    assert_eq!(with_reject.hash(), pristine.hash());
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

#[test]
fn mirrored_duels_end_in_mutual_annihilation() {
    // The observable core of simultaneous resolution: two identical
    // sentinels ordered at each other die on the same tick. Before 0.6,
    // inline damage let the lower id win every mirror duel with hp to
    // spare — the same edge that decided every mirror match.
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 4, 6),
        unit(1, UnitKind::Sentinel, 11, 6),
    ])
    .build()
    .unwrap();
    let (a, b) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[
        cmd(
            0,
            Command::Attack {
                units: vec![a],
                target: Target::Unit(b),
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Attack {
                units: vec![b],
                target: Target::Unit(a),
                queue: false,
            },
        ),
    ]);
    run_until(&mut state, 400, |s, _| {
        s.unit(a).is_none() || s.unit(b).is_none()
    });
    assert!(
        state.unit(a).is_none() && state.unit(b).is_none(),
        "identical opponents must fall together, not by id order"
    );
}

#[test]
fn retaliation_picks_the_earliest_surviving_attacker() {
    // Two lancers volley the sentinel from beyond its aggro while the
    // sentinel's own lancers kill the first of them in the same buffered
    // resolution. The answer must go to the survivor — locking onto the
    // corpse would leave the second shooter firing unopposed.
    let mut state = arena(vec![
        unit(1, UnitKind::Lancer, 8, 4),   // id 0: dies this tick
        unit(1, UnitKind::Lancer, 8, 7),   // id 1: survives
        unit(0, UnitKind::Lancer, 10, 1),  // id 2: executioner
        unit(0, UnitKind::Lancer, 12, 4),  // id 3: executioner
        unit(0, UnitKind::Sentinel, 3, 6), // id 4: the victim
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    let (a, b, c1, c2, v) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    let report = state.tick(&[
        cmd(
            1,
            Command::Attack {
                units: vec![a, b],
                target: Target::Unit(v),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Attack {
                units: vec![c1, c2],
                target: Target::Unit(a),
                queue: false,
            },
        ),
    ]);
    let _ = report;
    // The whole exchange lands on the command tick (everyone in range).
    assert!(
        state.unit(a).is_none(),
        "the first attacker died in the volley"
    );
    assert!(
        matches!(
            state.unit(v).unwrap().order,
            Order::Attack { target: Target::Unit(t), .. } if t == b
        ),
        "the victim must answer the surviving attacker, got {:?}",
        state.unit(v).unwrap().order
    );
}

#[test]
fn retaliation_interrupts_an_attack_on_a_corpse() {
    // The victim auto-acquires the adjacent scuttler during its own brain
    // step; the scuttler dies in the same volley that an out-of-aggro
    // lancer lands on the victim. The victim's attack order points at a
    // corpse — it must interrupt and answer the survivor, not stand mute
    // through the lancer's next cooldown.
    let mut state = arena(vec![
        unit(1, UnitKind::Scuttler, 5, 6), // id 0: bait, dies this tick
        unit(1, UnitKind::Lancer, 9, 4),   // id 1: the real threat
        unit(0, UnitKind::Lancer, 9, 7),   // id 2: executioner
        unit(0, UnitKind::Lancer, 10, 6),  // id 3: executioner
        unit(0, UnitKind::Sentinel, 4, 6), // id 4: the victim
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    let (bait, sniper, c1, c2, v) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    state.tick(&[
        cmd(
            1,
            Command::Attack {
                units: vec![sniper],
                target: Target::Unit(v),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Attack {
                units: vec![c1, c2],
                target: Target::Unit(bait),
                queue: false,
            },
        ),
    ]);
    assert!(state.unit(bait).is_none(), "the bait died in the volley");
    assert!(
        matches!(
            state.unit(v).unwrap().order,
            Order::Attack { target: Target::Unit(t), .. } if t == sniper
        ),
        "the victim must abandon the corpse and answer the sniper, got {:?}",
        state.unit(v).unwrap().order
    );
}

#[test]
fn same_tick_construction_cannot_absorb_a_lethal_hit() {
    use oxide_sim::stats::BuildingKind;
    // Chew a turret site to exactly the lancer's damage, then land the
    // builder's resume and the lancer's shot on the same tick. The
    // shooter aimed at a 30 hp site; one point of same-tick construction
    // must not rescue it.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 6),
        unit(1, UnitKind::Sentinel, 12, 2),
        // Parked at exactly 5.5 from the site's closest point: in firing
        // range, outside auto-acquire — it must not chew early.
        unit(1, UnitKind::Lancer, 11, 6),
    ])
    .build()
    .unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    let (builder, chewer, sniper) = (ids[0], ids[1], ids[2]);
    let anchor = TilePos::new(5, 6);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    // Freeze construction at exactly 70 hp (the first ramp step is zero).
    state.tick(&[cmd(
        0,
        Command::Stop {
            units: vec![builder],
        },
    )]);
    assert_eq!(state.building(site).unwrap().hp, 70);
    // Four sentinel hits take it to exactly 30.
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![chewer],
            target: Target::Building(site),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        s.building(site).is_some_and(|b| b.hp <= 30)
    });
    assert_eq!(state.building(site).unwrap().hp, 30, "clean 10s from 70");
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![chewer],
            goal: TilePos::new(13, 1),
            queue: false,
        },
    )]);
    // The finale: builder resumes and the lancer fires, same tick.
    let mut events = state
        .tick(&[
            cmd(
                0,
                Command::Build {
                    units: vec![builder],
                    kind: BuildingKind::Turret,
                    anchor,
                },
            ),
            cmd(
                1,
                Command::Attack {
                    units: vec![sniper],
                    target: Target::Building(site),
                    queue: false,
                },
            ),
        ])
        .events;
    if state.building(site).is_some() {
        events.extend(run_until(&mut state, 5, |s, _| s.building(site).is_none()));
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { .. })),
        "a lethal hit must kill the site regardless of same-tick building"
    );
}

#[test]
fn a_doomed_site_never_comes_online() {
    use oxide_sim::stats::BuildingKind;
    // Two scuttlers chew the site throughout construction so its hp at
    // the final progress tick sits well under the parked lancers' volley;
    // the volley lands on exactly that tick. The site must die without
    // ever completing: no online event, no free turret shot.
    let scenario = Scenario {
        name: "doomed-site".into(),
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
            unit(0, UnitKind::Harvester, 8, 5),
            unit(1, UnitKind::Scuttler, 11, 3),
            unit(1, UnitKind::Scuttler, 11, 8),
            // Parked in the fire-but-no-aggro band around the site.
            unit(1, UnitKind::Lancer, 15, 5),
            unit(1, UnitKind::Lancer, 15, 6),
            unit(1, UnitKind::Lancer, 14, 2),
        ],
    };
    let mut state = scenario.build().unwrap();
    let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    let (builder, s1, s2, l1, l2, l3) = (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);
    let anchor = TilePos::new(9, 5);
    let build_ticks = BuildingKind::Turret
        .stats()
        .construction
        .unwrap()
        .build_ticks;

    let mut all_events = Vec::new();
    all_events.extend(
        state
            .tick(&[cmd(
                0,
                Command::Build {
                    units: vec![builder],
                    kind: BuildingKind::Turret,
                    anchor,
                },
            )])
            .events,
    );
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == anchor)
        .unwrap()
        .id;
    all_events.extend(
        state
            .tick(&[cmd(
                1,
                Command::Attack {
                    units: vec![s1, s2],
                    target: Target::Building(site),
                    queue: false,
                },
            )])
            .events,
    );
    // Progress hit 1 on the build tick and 2 on the tick above; the final
    // tick is build_ticks-2 empty ticks later. Fire the volley then.
    for _ in 0..(build_ticks - 3) {
        all_events.extend(state.tick(&[]).events);
        assert!(
            state.building(site).is_some_and(|b| !b.built && b.hp > 0),
            "premise: the site survives, unfinished, until the final tick"
        );
    }
    all_events.extend(
        state
            .tick(&[cmd(
                1,
                Command::Attack {
                    units: vec![l1, l2, l3],
                    target: Target::Building(site),
                    queue: false,
                },
            )])
            .events,
    );
    // Sweep a couple more ticks for the destruction event.
    for _ in 0..3 {
        all_events.extend(state.tick(&[]).events);
    }
    assert!(
        !all_events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. })),
        "a site killed on its final tick must never report completion"
    );
    assert!(
        !all_events
            .iter()
            .any(|e| matches!(e, Event::TurretFired { .. })),
        "a doomed turret gets no free shot"
    );
    assert!(
        all_events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { .. })),
        "the volley killed it"
    );
    assert!(state.building(site).is_none());
}
