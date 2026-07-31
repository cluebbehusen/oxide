//! Combat: engagement, retaliation, cover, matchups — behavior suite, public API only.

mod common;

use chassis::grid::TilePos;
use oxide_sim::scenario::BuildingSpec;
use oxide_sim::{BuildingKind, Command, Event, Order, Scenario, Target, UnitId, UnitKind};

use common::*;

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
        buildings: Vec::new(),
        meta: None,
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
fn unqueued_advance_replaces_an_acquired_attack_on_the_command_tick() {
    let mut state = open_arena(
        22,
        12,
        vec![
            unit(0, UnitKind::Sentinel, 4, 4),
            unit(1, UnitKind::Harvester, 6, 4),
        ],
    )
    .build()
    .unwrap();
    let (mover, target) = (state.units()[0].id, state.units()[1].id);

    state.tick(&[]);
    assert_eq!(
        state.unit(mover).unwrap().order,
        Order::Attack {
            target: Target::Unit(target),
            resume: None,
        },
        "test premise: idle acquisition installed a chase"
    );

    let goal = TilePos::new(16, 4);
    state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![mover],
            goal,
            queue: false,
        },
    )]);

    let mover = state.unit(mover).unwrap();
    assert_eq!(mover.order, Order::Advance { goal });
    assert!(
        mover.path.is_some(),
        "the replacement starts routing immediately"
    );
    assert!(mover.queue.is_empty());
}

#[test]
fn advance_moves_and_fires_without_replacing_its_route() {
    let mut state = open_arena(
        20,
        12,
        vec![
            unit(0, UnitKind::Sentinel, 4, 4),
            unit(1, UnitKind::Harvester, 6, 4),
        ],
    )
    .build()
    .unwrap();
    let (mover, target) = (state.units()[0].id, state.units()[1].id);
    let before_pos = state.unit(mover).unwrap().pos;
    let before_hp = state.unit(target).unwrap().hp;
    let goal = TilePos::new(14, 4);

    let report = state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![mover],
            goal,
            queue: false,
        },
    )]);

    let mover = state.unit(mover).unwrap();
    assert!(mover.pos != before_pos, "the shot must not stop movement");
    assert_eq!(mover.order, Order::Advance { goal });
    assert!(
        mover.path.is_some(),
        "the advance route must survive firing"
    );
    assert!(state.unit(target).unwrap().hp < before_hp);
    assert!(
        report.events.iter().any(
            |event| matches!(event, Event::AttackHit { attacker, .. } if *attacker == mover.id)
        )
    );
}

#[test]
fn advance_chooses_equal_range_targets_by_id() {
    let mut state = open_arena(
        20,
        12,
        vec![
            unit(0, UnitKind::Sentinel, 4, 4),
            unit(1, UnitKind::Harvester, 6, 3),
            unit(1, UnitKind::Harvester, 6, 5),
        ],
    )
    .build()
    .unwrap();
    let (mover, lower, higher) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let full = UnitKind::Harvester.stats().max_hp;

    state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![mover],
            goal: TilePos::new(14, 4),
            queue: false,
        },
    )]);

    assert!(
        state.unit(lower).unwrap().hp < full,
        "the lower id wins an exact distance tie"
    );
    assert_eq!(
        state.unit(higher).unwrap().hp,
        full,
        "one ready primary weapon takes one deterministic shot"
    );
}

#[test]
fn advance_does_not_fire_a_secondary_weapon() {
    let mut state = open_arena(
        20,
        12,
        vec![
            unit(0, UnitKind::Sentinel, 4, 4),
            unit(1, UnitKind::Wisp, 6, 4),
        ],
    )
    .build()
    .unwrap();
    let (mover, flyer) = (state.units()[0].id, state.units()[1].id);
    let before = state.unit(flyer).unwrap().hp;

    let report = state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![mover],
            goal: TilePos::new(14, 4),
            queue: false,
        },
    )]);

    assert_eq!(
        state.unit(flyer).unwrap().hp,
        before,
        "Advance promises primary-weapon pot-shots, not sidearm fire"
    );
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::AttackHit { attacker, .. } if *attacker == mover)),
        "the Sentinel's anti-air secondary stays quiet"
    );
    assert!(matches!(
        state.unit(mover).unwrap().order,
        Order::Advance { .. }
    ));
}

#[test]
fn advance_respects_cover_without_diverting_around_it() {
    let mut state = open_arena_with(
        20,
        12,
        vec![
            unit(0, UnitKind::Sentinel, 4, 4),
            unit(1, UnitKind::Harvester, 6, 4),
        ],
        |rows| rows[4][5] = '#',
    )
    .build()
    .unwrap();
    let (mover, target) = (state.units()[0].id, state.units()[1].id);
    let before = state.unit(target).unwrap().hp;
    let goal = TilePos::new(4, 8);

    let report = state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![mover],
            goal,
            queue: false,
        },
    )]);

    assert_eq!(
        state.unit(target).unwrap().hp,
        before,
        "direct primary fire cannot pass through rock"
    );
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::AttackHit { attacker, .. } if *attacker == mover))
    );
    assert_eq!(
        state.unit(mover).unwrap().order,
        Order::Advance { goal },
        "cover cannot turn the route into a chase"
    );
}

#[test]
fn advance_projectiles_launch_unguided_without_stopping() {
    let mut state = open_arena(
        24,
        14,
        vec![
            unit(0, UnitKind::Bombard, 4, 5),
            unit(0, UnitKind::Sentinel, 9, 7),
            unit(1, UnitKind::Harvester, 11, 5),
        ],
    )
    .build()
    .unwrap();
    let (bombard, victim) = (state.units()[0].id, state.units()[2].id);
    let before_pos = state.unit(bombard).unwrap().pos;
    let before_hp = state.unit(victim).unwrap().hp;
    assert!(
        state.can_see(oxide_sim::PlayerId(0), state.unit(victim).unwrap().tile()),
        "the allied spotter makes the launch legal"
    );

    let report = state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![bombard],
            goal: TilePos::new(17, 5),
            queue: false,
        },
    )]);

    assert!(
        report.events.iter().any(|event| matches!(
            event,
            Event::ShellLaunched {
                shooter: Target::Unit(id),
                ..
            } if *id == bombard
        )),
        "the Bombard's primary uses the ordinary projectile pipeline"
    );
    assert_eq!(
        state.unit(victim).unwrap().hp,
        before_hp,
        "an unguided shell does not deal launch-tick damage"
    );
    assert_ne!(
        state.unit(bombard).unwrap().pos,
        before_pos,
        "launching does not stop the Advance"
    );
    assert!(matches!(
        state.unit(bombard).unwrap().order,
        Order::Advance { .. }
    ));
}

#[test]
fn advance_never_fires_into_fog() {
    let mut state = open_arena(
        26,
        14,
        vec![
            unit(0, UnitKind::Bombard, 4, 5),
            unit(1, UnitKind::Harvester, 12, 5),
        ],
    )
    .build()
    .unwrap();
    let (bombard, unseen) = (state.units()[0].id, state.units()[1].id);
    assert!(
        !state.can_see(oxide_sim::PlayerId(0), state.unit(unseen).unwrap().tile()),
        "the victim starts inside weapon range but outside all allied sight"
    );
    let before_hp = state.unit(unseen).unwrap().hp;

    let report = state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![bombard],
            goal: TilePos::new(18, 5),
            queue: false,
        },
    )]);

    assert_eq!(state.unit(unseen).unwrap().hp, before_hp);
    assert!(
        !report.events.iter().any(|event| matches!(
            event,
            Event::ShellLaunched {
                shooter: Target::Unit(id),
                ..
            } if *id == bombard
        )),
        "weapon range is not free vision"
    );
    assert!(matches!(
        state.unit(bombard).unwrap().order,
        Order::Advance { .. }
    ));
}

#[test]
fn advance_never_chases_an_out_of_range_bystander() {
    let mut state = open_arena(
        22,
        13,
        vec![
            unit(0, UnitKind::Sentinel, 3, 3),
            unit(1, UnitKind::Harvester, 8, 7),
        ],
    )
    .build()
    .unwrap();
    let (mover, bystander) = (state.units()[0].id, state.units()[1].id);
    let before_hp = state.unit(bystander).unwrap().hp;
    let goal = TilePos::new(17, 3);
    state.tick(&[cmd(
        0,
        Command::Advance {
            units: vec![mover],
            goal,
            queue: false,
        },
    )]);

    run_until(&mut state, 400, |state, _| {
        let mover = state.unit(mover).unwrap();
        mover.tile() == goal && mover.order == Order::Idle
    });
    assert_eq!(state.unit(bystander).unwrap().hp, before_hp);
}

#[test]
fn advance_ignores_retaliation_and_pacifists_use_plain_move() {
    let mut state = open_arena(
        20,
        12,
        vec![
            unit(0, UnitKind::Sentinel, 4, 4),
            unit(0, UnitKind::Harvester, 4, 6),
            unit(1, UnitKind::Sentinel, 6, 4),
        ],
    )
    .build()
    .unwrap();
    let (guard, worker, attacker) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let goal = TilePos::new(14, 4);
    state.tick(&[
        cmd(
            0,
            Command::Advance {
                units: vec![guard, worker],
                goal,
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Attack {
                units: vec![attacker],
                target: Target::Unit(guard),
                queue: false,
            },
        ),
    ]);

    assert!(matches!(
        state.unit(guard).unwrap().order,
        Order::Advance { .. }
    ));
    assert!(matches!(
        state.unit(worker).unwrap().order,
        Order::Move { .. }
    ));
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
        buildings: Vec::new(),
        meta: None,
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
fn buildings_are_not_cover_only_terrain_is() {
    // The rock test's mirror image: same 2-tile spacing inside range 2.5,
    // but the wall is a building. Buildings block movement, never bullets,
    // so the first shot lands on the command tick from the starting tile.
    let scenario = Scenario {
        name: "building-no-cover".into(),
        seed: 42,
        map: vec![
            "############".into(),
            "#1.........#".into(),
            "#..........#".into(),
            "#..........#".into(),
            "#..........#".into(),
            "#........2.#".into(),
            "#..........#".into(),
            "############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![
            unit(0, UnitKind::Sentinel, 4, 3),
            unit(1, UnitKind::Harvester, 6, 3),
        ],
        buildings: vec![BuildingSpec {
            player: 1,
            kind: BuildingKind::Array,
            x: 5,
            y: 3,
        }],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let (attacker, victim) = (state.units()[0].id, state.units()[1].id);
    let start_tile = state.unit(attacker).unwrap().tile();
    let events = state
        .tick(&[cmd(
            0,
            Command::Attack {
                units: vec![attacker],
                target: Target::Unit(victim),
                queue: false,
            },
        )])
        .events;
    assert!(
        events.iter().any(|e| matches!(e, Event::AttackHit { .. })),
        "the shot must land on the command tick — a building on the line is \
         not cover"
    );
    assert_eq!(
        state.unit(attacker).unwrap().tile(),
        start_tile,
        "no repositioning: the attacker fires from where it stands"
    );
}

#[test]
fn a_turret_fires_past_the_building_flush_against_it() {
    // The playtest complaint: a 1x1 flush against a Turret used to shadow
    // over a quarter of its arc, and the turret path has no repositioning
    // fallback — it just went quiet. Terrain-only cover: the turret fires
    // straight through its neighbor.
    let scenario = Scenario {
        name: "turret-past-neighbor".into(),
        seed: 42,
        map: vec![
            "############".into(),
            "#1.........#".into(),
            "#..........#".into(),
            "#..........#".into(),
            "#..........#".into(),
            "#........2.#".into(),
            "#..........#".into(),
            "############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![unit(1, UnitKind::Harvester, 6, 3)],
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 4,
                y: 3,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Array,
                x: 5,
                y: 3,
            },
        ],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let victim = state.units()[0].id;
    // 60 hp / 12 damage every 25 ticks — dead within ~110 ticks. Before
    // terrain-only cover the turret never fired at all.
    run_until(&mut state, 200, |s, _| s.unit(victim).is_none());
}

#[test]
fn an_unbuilt_site_is_no_sandbag() {
    // Construction claims ground the tick it is placed — but a foundation
    // is a slab, not a wall. Dropping a site on the fire line must not buy
    // instant hard cover.
    let scenario = Scenario {
        name: "site-no-sandbag".into(),
        seed: 42,
        map: vec![
            "############".into(),
            "#1.........#".into(),
            "#..........#".into(),
            "#..........#".into(),
            "#..........#".into(),
            "#........2.#".into(),
            "#..........#".into(),
            "############".into(),
        ],
        players: arena(vec![]).players,
        units: vec![
            unit(0, UnitKind::Sentinel, 4, 3),
            unit(1, UnitKind::Harvester, 6, 3),
            unit(1, UnitKind::Harvester, 5, 5),
        ],
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let (attacker, victim, builder) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let start_tile = state.unit(attacker).unwrap().tile();
    // The site claims the line tile on the command tick.
    state.tick(&[cmd(
        1,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Array,
            anchor: TilePos::new(5, 3),
            queue: false,
            defer: false,
        },
    )]);
    let site = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Array)
        .expect("the site must have been placed");
    assert!(!site.built, "the blocker must be an unbuilt foundation");
    let events = state
        .tick(&[cmd(
            0,
            Command::Attack {
                units: vec![attacker],
                target: Target::Unit(victim),
                queue: false,
            },
        )])
        .events;
    assert!(
        events.iter().any(|e| matches!(e, Event::AttackHit { .. })),
        "the shot must land on the command tick — an unbuilt site is not cover"
    );
    assert_eq!(
        state.unit(attacker).unwrap().tile(),
        start_tile,
        "no repositioning: the attacker fires from where it stands"
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
fn lancer_outranges_aggro_and_retaliation_answers() {
    // The lancer opens fire from 5.4 tiles — outside the bombard's aggro
    // (5), inside rail range (5.5). The bombard is the one chassis that
    // survives a rail hit AND can answer ground (the 0.10 rail one-shots
    // the 60-hp sentinel, so the line unit can no longer star here).
    // The first hit turns the victim on its attacker; the rail wins the
    // duel it opened, but the answer — one arcing shell already in
    // flight — lands after its shooter is dead. Shells outlive shooters.
    let mut state = arena(vec![
        unit(0, UnitKind::Bombard, 3, 6),
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
    let aggro = oxide_sim::stats::UnitKind::Bombard.stats().aggro_range;
    assert!(d2 > aggro * aggro, "test premise: outside bombard aggro");

    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![lancer],
            target: Target::Unit(victim),
            queue: false,
        },
    )]);
    // The lancer needs no approach: the first hit lands within a tick or
    // two, and the bombard's answer must be immediate.
    run_until(&mut state, 10, |s, _| {
        s.unit(victim).unwrap().hp < UnitKind::Bombard.stats().max_hp
    });
    assert!(
        matches!(
            state.unit(victim).unwrap().order,
            Order::Attack { target: Target::Unit(t), .. } if t == lancer
        ),
        "the bombard should turn on its attacker"
    );
    // The rail's second shot kills the bombard...
    run_until(&mut state, 400, |s, _| s.unit(victim).is_none());
    // ...and the posthumous shell still lands: the answer connected.
    run_until(&mut state, 400, |s, _| {
        s.unit(lancer).unwrap().hp < UnitKind::Lancer.stats().max_hp
    });
    assert!(
        state.unit(lancer).is_some(),
        "one shell wounds the rail, it does not kill it"
    );
}

#[test]
fn a_flank_pick_is_lethal_and_the_march_still_arrives() {
    // An open lane: the lancer sits 5.4 tiles off the march route —
    // outside the marchers' aggro, inside its own range — and picks one
    // off as the column passes. Under the 0.10 numbers the rail
    // one-shots the 60-hp sentinel: the pick is an assassination, no
    // answer is possible from a corpse, and the sniper walks away
    // clean. What the ambush must NOT do is stop the army — the
    // rearguard still arrives. (The retaliation contract itself is
    // covered by the bombard tests; fight-then-win-then-resume by
    // attack_move_engages_on_the_way_then_resumes.)
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
            unit(0, UnitKind::Sentinel, 4, 1),
            unit(1, UnitKind::Lancer, 9, 7),
        ],
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let (marcher, rearguard, lancer) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let goal = TilePos::new(16, 2);
    // The rearguard marches the y=1 lane: its closest approach to the
    // lancer is 6.0 tiles — outside inclusive aggro (5) from both sides
    // and outside rail range (5.5), so its journey stays a control.
    // Single-unit orders so each keeps the exact goal (a group order
    // would fan out ring goals and blur the arrival assertion).
    let rear_goal = TilePos::new(16, 1);
    state.tick(&[
        cmd(
            0,
            Command::AttackMove {
                units: vec![marcher],
                goal,
                queue: false,
            },
        ),
        cmd(
            0,
            Command::AttackMove {
                units: vec![rearguard],
                goal: rear_goal,
                queue: false,
            },
        ),
    ]);
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
    // One rail hit is the whole story for a 60-hp marcher.
    run_until(&mut state, 20, |s, _| s.unit(marcher).is_none());
    let sniper = state.unit(lancer).unwrap();
    assert_eq!(
        sniper.hp,
        UnitKind::Lancer.stats().max_hp,
        "a corpse answers nothing — the sniper walks away clean"
    );
    run_until(&mut state, 600, |s, _| {
        let u = s.unit(rearguard).unwrap();
        u.tile() == rear_goal && u.order == Order::Idle
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
        buildings: Vec::new(),
        meta: None,
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
            queue: false,
            defer: false,
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

    // Now the siege, in a fresh world: a lancer at range 5.5 > turret 5.0
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
        buildings: Vec::new(),
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let (builder, lancer) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor,
            queue: false,
            defer: false,
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
            queue: false,
            defer: false,
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
    let cooldown = u64::from(BuildingKind::Turret.stats().weapons[0].cooldown_ticks);
    assert!(fire_ticks.len() >= 3, "not enough shots observed");
    assert_eq!(
        fire_ticks[1] - fire_ticks[0],
        cooldown,
        "interval must equal cooldown_ticks, not cooldown_ticks + 1"
    );
    assert_eq!(fire_ticks[2] - fire_ticks[1], cooldown);
}

#[test]
fn bastion_has_artillery_reach_and_a_real_close_pressure_dead_zone() {
    let bastion_weapon = BuildingKind::Bastion.stats().weapons[0];
    assert_eq!(
        bastion_weapon.range,
        UnitKind::Bombard.stats().weapons[0].range,
        "the emplacement must threaten the same nominal envelope as mobile artillery"
    );
    assert!(
        bastion_weapon.minimum_range > chassis::fx::Fx::ZERO,
        "the long gun needs explicit close-pressure counterplay"
    );

    let scenario = Scenario {
        name: "bastion-dead-zone".into(),
        seed: 44,
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
        units: vec![unit(1, UnitKind::Scuttler, 9, 6)],
        buildings: vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 7,
            y: 5,
        }],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let bastion = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Bastion)
        .unwrap()
        .id;
    let scuttler = state.units()[0].id;
    let scuttler_hp = state.unit(scuttler).unwrap().hp;
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![scuttler],
            target: Target::Building(bastion),
            queue: false,
        },
    )]);
    let events = run_until(&mut state, 1_500, |s, _| s.building(bastion).is_none());
    assert!(
        state.building(bastion).is_none(),
        "an isolated Bastion must fall to pressure established inside its dead zone"
    );
    assert_eq!(
        state.unit(scuttler).unwrap().hp,
        scuttler_hp,
        "the Bastion must not acquire or fire on a target inside its dead zone"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            Event::ShellLaunched {
                shooter: Target::Building(id),
                ..
            } if *id == bastion
        )),
        "minimum range must be enforced at fire time, not merely advertised in stats"
    );
}

#[test]
fn bastion_opens_fire_beyond_its_dead_zone() {
    let scenario = Scenario {
        name: "bastion-open-fire".into(),
        seed: 45,
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
        units: vec![unit(1, UnitKind::Harvester, 13, 6)],
        buildings: vec![BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 7,
            y: 5,
        }],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let bastion = state
        .buildings()
        .iter()
        .find(|building| building.kind == BuildingKind::Bastion)
        .unwrap()
        .id;
    let target = state.units()[0].id;
    let before = state.unit(target).unwrap().hp;
    let events = run_until(&mut state, 200, |s, _| {
        s.unit(target).is_none_or(|unit| unit.hp < before)
    });
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::ShellLaunched {
                shooter: Target::Building(id),
                ..
            } if *id == bastion
        )),
        "a target outside the dead zone must still be acquired immediately"
    );
    assert!(
        state.unit(target).is_none_or(|unit| unit.hp < before),
        "a stationary target outside the dead zone must take the shell"
    );
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

// A retired sibling of the test below — "two simultaneous beyond-aggro
// attackers, one executed, the victim answers the earliest survivor" —
// died with the 0.10 rail bless: two rail hits (120) now kill every
// chassis that can answer ground, so the two-survivable-shooter volley
// cannot be staged in the real game. The earliest-survivor property
// itself is structural (the busy-guard makes the first processed answer
// stick) and stays exercised by the corpse-skip and interrupt tests.

#[test]
fn retaliation_interrupts_an_attack_on_a_corpse() {
    // The victim auto-acquires the adjacent scuttler during its own brain
    // step; the scuttler dies in the same volley that an out-of-aggro
    // lancer lands on the victim. The victim's attack order points at a
    // corpse — it must interrupt and answer the survivor, not stand mute
    // through the lancer's next cooldown. The victim is a bombard: the
    // one chassis that survives the rail hit and answers ground.
    let mut state = arena(vec![
        unit(1, UnitKind::Scuttler, 5, 6), // id 0: bait, dies this tick
        unit(1, UnitKind::Lancer, 9, 4),   // id 1: the real threat
        unit(0, UnitKind::Lancer, 9, 7),   // id 2: executioner
        unit(0, UnitKind::Lancer, 10, 6),  // id 3: executioner
        unit(0, UnitKind::Bombard, 4, 6),  // id 4: the victim
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
