//! Combat: engagement, retaliation, cover, matchups — behavior suite, public API only.

mod common;

use chassis::grid::TilePos;
use oxide_sim::{Command, Event, Order, Scenario, Target, UnitId, UnitKind};

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
    let aggro = oxide_sim::stats::UnitKind::Sentinel.stats().aggro_range;
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
        meta: None,
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
