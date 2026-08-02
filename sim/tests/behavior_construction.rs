//! Construction: sites, builders, refunds, placement — behavior suite, public API only.

mod common;

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::{Command, Event, Order, PlayerId, Scenario, State, Target, UnitId, UnitKind};

use common::*;

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
            queue: false,
            defer: false,
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
            queue: false,
            defer: false,
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
            queue: false,
            defer: false,
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
            queue: false,
            defer: false,
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
        buildings: Vec::new(),
        meta: None,
    };
    assert!(matches!(
        scenario.build(),
        Err(ScenarioError::Disconnected(..))
    ));
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
            queue: false,
            defer: false,
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
            queue: false,
            defer: false,
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
            queue: false,
            defer: false,
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
            queue: false,
            defer: false,
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
        buildings: Vec::new(),
        meta: None,
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
            queue: false,
            defer: false,
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
                queue: false,
                defer: false,
            },
        )]);
        if extra_builder {
            state.tick(&[cmd(
                0,
                Command::Build {
                    units: vec![ids[1]],
                    kind: BuildingKind::Turret,
                    anchor,
                    queue: false,
                    defer: false,
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
        buildings: Vec::new(),
        meta: None,
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
            queue: false,
            defer: false,
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
        buildings: Vec::new(),
        meta: None,
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
            queue: false,
            defer: false,
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
            queue: false,
            defer: false,
        },
    )]);
    let _ = a;
    let b = pristine.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(3, 4),
            queue: false,
            defer: false,
        },
    )]);
    let _ = b;
    assert_eq!(with_reject.hash(), pristine.hash());
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
            queue: false,
            defer: false,
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
                    queue: false,
                    defer: false,
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
        buildings: Vec::new(),
        meta: None,
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
                    queue: false,
                    defer: false,
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

#[test]
fn queued_builds_chain_one_builder_through_two_sites() {
    // Shift-placement's promise: pay and claim both sites NOW, walk
    // and build them in order. One builder, two turrets, one gesture.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 3, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: oxide_sim::stats::BuildingKind::Turret,
            anchor: TilePos::new(3, 4),
            queue: false,
            defer: false,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: oxide_sim::stats::BuildingKind::FlakTurret,
            anchor: TilePos::new(9, 2),
            queue: true,
            defer: false,
        },
    )]);
    // Both sites exist immediately (pay-and-claim at command time) and
    // both prices are gone from the bank.
    assert_eq!(state.buildings().len(), 4, "two foundries + two sites");
    assert_eq!(state.player(PlayerId(0)).scrap, 200 - 100 - 90);
    assert!(matches!(
        state.unit(builder).unwrap().order,
        Order::Build { .. }
    ));
    run_until(&mut state, 2_000, |s, _| {
        s.buildings()
            .iter()
            .filter(|b| b.player == PlayerId(0) && b.built)
            .count()
            == 3 // foundry + both towers
    });
}

#[test]
fn a_queued_build_whose_site_died_pops_silently_and_the_program_survives() {
    // A queued leg whose site vanished is a finished job, not a stall:
    // the rest of the program must survive (OrderStalled clears whole
    // programs, which is exactly wrong here).
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 3, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: oxide_sim::stats::BuildingKind::Turret,
            anchor: TilePos::new(3, 4),
            queue: false,
            defer: false,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: oxide_sim::stats::BuildingKind::FlakTurret,
            anchor: TilePos::new(9, 2),
            queue: true,
            defer: false,
        },
    )]);
    let walk_home = TilePos::new(8, 6);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: walk_home,
            queue: true,
        },
    )]);
    // Cancel the queued flak site while the turret is still going up.
    let flak = state
        .buildings()
        .iter()
        .find(|b| b.kind == oxide_sim::stats::BuildingKind::FlakTurret)
        .unwrap()
        .id;
    state.tick(&[cmd(0, Command::Cancel { building: flak })]);
    let events = run_until(&mut state, 2_000, |s, _| {
        let u = s.unit(builder).unwrap();
        u.tile() == walk_home && u.order == Order::Idle
    });
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { unit, .. } if *unit == builder)),
        "a vanished queued site must pop silently, never stall the program"
    );
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == oxide_sim::stats::BuildingKind::Turret && b.built),
        "the first leg still finished"
    );
}

#[test]
fn a_full_order_queue_refuses_placement_with_nothing_spent() {
    // The old code paid for the site and discarded the assignment
    // result; a builder whose program is full must reject the whole
    // command with the site retracted and the bank untouched.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 3, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    // Fill the program in ONE tick — commands executed across ticks
    // would drain as fast as they queue (short legs complete). The
    // current order is a long march so nothing pops before the probe.
    let mut fill = vec![cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(13, 6),
            queue: false,
        },
    )];
    for i in 0..oxide_sim::stats::ORDER_QUEUE_CAP {
        fill.push(cmd(
            0,
            Command::Move {
                units: vec![builder],
                goal: TilePos::new(12 + (i % 2) as i32, 6),
                queue: true,
            },
        ));
    }
    state.tick(&fill);
    let bank = state.player(PlayerId(0)).scrap;
    let sites_before = state.buildings().len();
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: oxide_sim::stats::BuildingKind::Turret,
            anchor: TilePos::new(3, 4),
            queue: true,
            defer: false,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::QueueFull,
                ..
            }
        )),
        "a full program refuses the build"
    );
    assert_eq!(state.player(PlayerId(0)).scrap, bank, "nothing spent");
    assert_eq!(state.buildings().len(), sites_before, "no orphan site");
}

#[test]
fn resuming_a_site_sends_every_hand() {
    // Builders stack; a resume command commits the whole crew, and the
    // site rises roughly three times as fast under three welders.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 3, 2),
        unit(0, UnitKind::Harvester, 4, 2),
        unit(0, UnitKind::Harvester, 5, 2),
    ])
    .build()
    .unwrap();
    let crew: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![crew[0]],
            kind: oxide_sim::stats::BuildingKind::Turret,
            anchor: TilePos::new(3, 4),
            queue: false,
            defer: false,
        },
    )]);
    // Resume at the existing site with the full crew: everyone joins.
    state.tick(&[cmd(
        0,
        Command::Build {
            units: crew.clone(),
            kind: oxide_sim::stats::BuildingKind::Turret,
            anchor: TilePos::new(3, 4),
            queue: false,
            defer: false,
        },
    )]);
    for id in &crew {
        assert!(
            matches!(state.unit(*id).unwrap().order, Order::Build { .. }),
            "every hand took the order"
        );
    }
    run_until(&mut state, 600, |s, _| {
        s.buildings()
            .iter()
            .any(|b| b.kind == oxide_sim::stats::BuildingKind::Turret && b.built)
    });
}

#[test]
fn place_refusal_names_the_actual_blocker() {
    use oxide_sim::PlaceRefusal;
    use oxide_sim::stats::BuildingKind;
    let state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 6),
        unit(1, UnitKind::Scuttler, 5, 5),
    ])
    .build()
    .unwrap();
    let p = PlayerId(0);
    let k = BuildingKind::Turret;
    let refusal = |x, y| state.place_refusal(p, k, TilePos::new(x, y));
    // The harvester's own tile: friendly machines make way, so the
    // ground is buildable.
    assert_eq!(refusal(4, 6), None);
    // A visible ENEMY machine denies its ground.
    assert_eq!(refusal(5, 5), Some(PlaceRefusal::Unit));
    // Open visible ground: allowed, and the predicate is literally the
    // same answer with the reason thrown away.
    assert_eq!(refusal(5, 6), None);
    assert!(state.can_place(p, k, TilePos::new(5, 6)));
    // Visible rock.
    assert_eq!(refusal(6, 3), Some(PlaceRefusal::Terrain));
    // Own Foundry footprint.
    assert_eq!(refusal(1, 1), Some(PlaceRefusal::Building));
    // The enemy Foundry's ground is fogged — and fog must win before
    // the building underneath can leak through the reason.
    assert_eq!(refusal(13, 6), Some(PlaceRefusal::Fog));
    // Scenario-only kinds are never placeable.
    assert_eq!(
        state.place_refusal(p, BuildingKind::Foundry, TilePos::new(5, 6)),
        Some(PlaceRefusal::NotConstructible)
    );
}

#[test]
fn a_builder_founds_a_building_under_its_own_feet() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 5, 6)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let anchor = TilePos::new(5, 6); // the builder's own tile
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
    assert!(
        state.buildings().iter().any(|b| b.anchor == anchor),
        "the site claims the builder's own ground"
    );
    assert!(matches!(
        state.unit(builder).unwrap().order,
        Order::Build { .. }
    ));
    // The builder WALKS off the claimed ground — the position series is
    // continuous, never the old instant relocation (a 1+ tile jump
    // inside one tick).
    let step_cap = chassis::fx::Fx::lit("0.5");
    let mut prev = state.unit(builder).unwrap().pos;
    let mut off_at = None;
    for t in 0..40u32 {
        state.tick(&[]);
        let u = state.unit(builder).unwrap();
        assert!(
            u.pos.dist(prev) <= step_cap,
            "the builder teleported at tick {t} instead of walking"
        );
        prev = u.pos;
        if off_at.is_none() && u.tile() != anchor {
            off_at = Some(t);
        }
    }
    assert!(off_at.is_some(), "the builder never stepped off its site");
    // And the build completes from the doorstep it walked to.
    let events = run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    });
    assert!(!events.is_empty(), "the under-feet site stands up");
}

#[test]
fn friendly_machines_make_way_for_foundations() {
    use oxide_sim::stats::BuildingKind;
    // A 2x2 Fabricator with the builder on one footprint tile and a
    // second OWN machine on another: both make way — by WALKING off
    // the claimed ground (the eviction pre-pass routes the pathless
    // sentinel out; the builder's own approach routes it to a
    // doorstep), positions continuous the whole way.
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 5, 6),
        unit(0, UnitKind::Sentinel, 6, 7),
    ])
    .build()
    .unwrap();
    let builder = state.units()[0].id;
    let sentinel = state.units()[1].id;
    let anchor = TilePos::new(5, 6);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Fabricator,
            anchor,
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        state.buildings().iter().any(|b| b.anchor == anchor),
        "friendly machines no longer deny the site"
    );
    let step_cap = chassis::fx::Fx::lit("0.5");
    let mut prev: Vec<_> = [builder, sentinel]
        .iter()
        .map(|id| state.unit(*id).unwrap().pos)
        .collect();
    for t in 0..60u32 {
        state.tick(&[]);
        for (i, id) in [builder, sentinel].iter().enumerate() {
            let now = state.unit(*id).unwrap().pos;
            assert!(
                now.dist(prev[i]) <= step_cap,
                "a displaced machine teleported at tick {t} instead of walking"
            );
            prev[i] = now;
        }
    }
    let (w, h) = BuildingKind::Fabricator.stats().size;
    let inside =
        |t: TilePos| t.x >= anchor.x && t.x < anchor.x + w && t.y >= anchor.y && t.y < anchor.y + h;
    assert!(
        state.units().iter().all(|u| !inside(u.tile())),
        "everyone walked off the claimed footprint"
    );
}

#[test]
fn an_enemy_machine_still_denies_the_ground() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 5, 6),
        unit(1, UnitKind::Scuttler, 6, 7),
    ])
    .build()
    .unwrap();
    let builder = state.units()[0].id;
    let anchor = TilePos::new(5, 6);
    let scrap = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Fabricator,
            anchor,
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        state.buildings().iter().all(|b| b.anchor != anchor),
        "denial-by-standing stays a real mechanic"
    );
    assert_eq!(state.player(PlayerId(0)).scrap, scrap, "nothing spent");
}

#[test]
fn an_allied_machine_makes_way_like_your_own() {
    use oxide_sim::stats::BuildingKind;
    // Two seats on one team: seat 0 builds where seat 1's sentinel
    // stands. The ally steps aside — your teammate's foundation is
    // not an enemy of your parking spot.
    let scenario = oxide_sim::Scenario::from_json(
        &serde_json::json!({
            "name": "Team Yard",
            "seed": 11,
            "players": [
                {"name": "West", "faction": "ferrous", "team": 1, "scrap": 300, "bot": false},
                {"name": "East", "faction": "cupric", "team": 1, "scrap": 0, "bot": true,
                 "bot_config": {"level": "medium"}},
                {"name": "Foe", "faction": "cupric", "scrap": 0, "bot": true,
                 "bot_config": {"level": "medium"}}
            ],
            "map": [
                "####################",
                "#1.................#",
                "#..................#",
                "#..................#",
                "#..................#",
                "#..............2.3.#",
                "#..................#",
                "####################"
            ],
            "units": [
                {"player": 0, "kind": "harvester", "x": 5, "y": 3},
                {"player": 1, "kind": "sentinel", "x": 6, "y": 4}
            ]
        })
        .to_string(),
    )
    .expect("team yard parses");
    let mut state = scenario.build().expect("team yard builds");
    let builder = state.units()[0].id;
    let ally = state.units()[1].id;
    let anchor = TilePos::new(5, 3);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Fabricator,
            anchor,
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        state.buildings().iter().any(|b| b.anchor == anchor),
        "an ally on the footprint does not deny the site"
    );
    // The ally walks off like an own machine would — continuous
    // positions, off the footprint within a few seconds.
    let step_cap = chassis::fx::Fx::lit("0.5");
    let mut prev = state.unit(ally).unwrap().pos;
    for t in 0..60u32 {
        state.tick(&[]);
        let now = state.unit(ally).unwrap().pos;
        assert!(
            now.dist(prev) <= step_cap,
            "the ally teleported at tick {t} instead of walking"
        );
        prev = now;
    }
    let (w, h) = BuildingKind::Fabricator.stats().size;
    let t = state.unit(ally).unwrap().tile();
    assert!(
        !(t.x >= anchor.x && t.x < anchor.x + w && t.y >= anchor.y && t.y < anchor.y + h),
        "the ally walked off the claimed footprint (now at {t:?})"
    );
}

#[test]
fn a_rejected_under_feet_build_leaves_no_trace_on_the_hash() {
    use oxide_sim::stats::BuildingKind;
    // QueueFull is the one rejection that fires after place_site: fill
    // the builder's program to the cap, then order a queued build
    // under its feet. The whole tick must leave the state exactly
    // where an empty tick would - the crew draft and the last-resort
    // perimeter deal run only after the last rejection path, or a
    // refused command would leave a mark on the hash.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 5, 6)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let mut fill = vec![cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(6, 6),
            queue: false,
        },
    )];
    for _ in 0..32 {
        fill.push(cmd(
            0,
            Command::Move {
                units: vec![builder],
                goal: TilePos::new(7, 6),
                queue: true,
            },
        ));
    }
    state.tick(&fill);
    let mut twin = state.clone();
    twin.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(5, 6),
            queue: true,
            defer: false,
        },
    )]);
    assert_eq!(
        state.hash(),
        twin.hash(),
        "a rejected build moved the state hash"
    );
}

#[test]
fn a_walled_in_machine_takes_the_instant_deal() {
    use oxide_sim::stats::BuildingKind;
    // A rock pocket seals one footprint tile: the machine standing
    // there has NO escape route once the site claims the ground, so
    // it takes the last-resort perimeter deal on the command tick —
    // nothing may end up inside a finished building. The builder,
    // whose side is open, walks like anyone else.
    let scenario = Scenario::from_json(
        &serde_json::json!({
            "name": "Pocket Yard",
            "seed": 7,
            "players": [
                {"name": "West", "faction": "ferrous", "scrap": 300, "bot": false},
                {"name": "East", "faction": "cupric", "scrap": 0, "bot": false}
            ],
            "map": [
                "############",
                "#1.........#",
                "#..........#",
                "#......#...#",
                "#......#...#",
                "#....###...#",
                "#........2.#",
                "#..........#",
                "############"
            ],
            "units": [
                {"player": 0, "kind": "harvester", "x": 5, "y": 3},
                {"player": 0, "kind": "sentinel", "x": 6, "y": 4}
            ]
        })
        .to_string(),
    )
    .expect("pocket yard parses");
    let mut state = scenario.build().expect("pocket yard builds");
    let builder = state.units()[0].id;
    let sealed = state.units()[1].id;
    let anchor = TilePos::new(5, 3);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Fabricator,
            anchor,
            queue: false,
            defer: false,
        },
    )]);
    assert!(
        state.buildings().iter().any(|b| b.anchor == anchor),
        "the pocketed footprint still accepts the site"
    );
    let (w, h) = BuildingKind::Fabricator.stats().size;
    let inside =
        |t: TilePos| t.x >= anchor.x && t.x < anchor.x + w && t.y >= anchor.y && t.y < anchor.y + h;
    let t = state.unit(sealed).unwrap().tile();
    assert!(
        !inside(t),
        "the routeless machine was dealt off the footprint immediately (at {t:?})"
    );
    // And the site stands up over the whole affair.
    let events = run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    });
    assert!(!events.is_empty(), "the site completes");
}

#[test]
fn a_fresh_placement_commits_the_whole_crew() {
    use oxide_sim::stats::BuildingKind;
    // The reclaim-parity rule reaches construction: a fresh Build
    // drafts every accepted harvester — not just the founder — and
    // non-harvesters in the selection are left to their own work.
    // Three hands raise the site markedly faster than one.
    let build_time = |crew: usize| {
        let mut units: Vec<_> = (0..crew)
            .map(|i| unit(0, UnitKind::Harvester, 3 + i as i32, 2))
            .collect();
        units.push(unit(0, UnitKind::Sentinel, 8, 2));
        let mut state = arena(units).build().unwrap();
        let ids: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
        let anchor = TilePos::new(3, 4);
        state.tick(&[cmd(
            0,
            Command::Build {
                units: ids.clone(),
                kind: BuildingKind::Turret,
                anchor,
                queue: false,
                defer: false,
            },
        )]);
        let (hands, sentinel) = ids.split_at(crew);
        for id in hands {
            assert!(
                matches!(state.unit(*id).unwrap().order, Order::Build { .. }),
                "every selected harvester took the order"
            );
        }
        assert_eq!(
            state.unit(sentinel[0]).unwrap().order,
            Order::Idle,
            "the sentinel is not drafted into construction"
        );
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
    let solo = build_time(1);
    let trio = build_time(3);
    assert!(
        trio * 3 < solo * 2,
        "three hands should be markedly faster: solo {solo}, trio {trio}"
    );
}

/// The deferred mode end-to-end: a claim on remembered ground charges
/// nothing and places nothing at accept, hands the founder
/// [`Order::Found`], and founds — site, payment, Build order — only
/// when the founder stands beside ground it can see again.
#[test]
fn a_deferred_build_founds_on_arrival() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 12, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let spot = TilePos::new(12, 1);
    // Walk home: the spot stays explored but drops out of sight.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| !s.can_see(PlayerId(0), spot));
    assert!(state.vision(PlayerId(0)).explored(spot));
    assert!(
        !state.can_place(PlayerId(0), BuildingKind::Turret, spot),
        "the strict predicate still refuses remembered ground"
    );
    assert!(
        state
            .place_intent_refusal(PlayerId(0), BuildingKind::Turret, spot)
            .is_none(),
        "the intent predicate accepts it from memory"
    );

    let scrap_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: spot,
            queue: false,
            defer: true,
        },
    )]);
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        scrap_before,
        "nothing charged at accept"
    );
    assert!(
        state.buildings().iter().all(|b| b.anchor != spot),
        "nothing placed at accept"
    );
    assert_eq!(
        state.unit(builder).unwrap().order,
        Order::Found {
            kind: BuildingKind::Turret,
            anchor: spot
        }
    );

    run_until(&mut state, 600, |s, _| {
        s.buildings().iter().any(|b| b.anchor == spot)
    });
    let cost = BuildingKind::Turret.stats().construction.unwrap().cost;
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        scrap_before - cost,
        "paid in full when the ground was claimed"
    );
    let site = state
        .buildings()
        .iter()
        .find(|b| b.anchor == spot)
        .unwrap()
        .id;
    assert_eq!(state.unit(builder).unwrap().order, Order::Build { site });
    let events = run_until(&mut state, 800, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    });
    assert!(!events.is_empty(), "the deferred site completes normally");
}

fn deferred_founder_fixture() -> (State, UnitId, TilePos) {
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 12, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let spot = TilePos::new(12, 1);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |state, _| {
        !state.can_see(PlayerId(0), spot)
    });
    assert!(state.vision(PlayerId(0)).explored(spot));
    (state, builder, spot)
}

#[test]
fn cancelling_a_queued_deferred_site_preserves_the_surrounding_program() {
    use oxide_sim::stats::BuildingKind;

    let (mut state, builder, spot) = deferred_founder_fixture();
    let later = Order::Move {
        goal: TilePos::new(3, 5),
    };
    let scrap_before = state.player(PlayerId(0)).scrap;
    state.tick(&[
        cmd(
            0,
            Command::Build {
                units: vec![builder],
                kind: BuildingKind::Turret,
                anchor: spot,
                queue: true,
                defer: true,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![builder],
                goal: TilePos::new(3, 5),
                queue: true,
            },
        ),
    ]);
    let active_before = state.unit(builder).unwrap().order;
    assert_eq!(
        state
            .unit(builder)
            .unwrap()
            .queue
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![
            Order::Found {
                kind: BuildingKind::Turret,
                anchor: spot,
            },
            later,
        ]
    );

    let report = state.tick(&[cmd(
        0,
        Command::CancelFound {
            kind: BuildingKind::Turret,
            anchor: spot,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. }))
    );
    let worker = state.unit(builder).unwrap();
    assert_eq!(worker.order, active_before, "the active leg keeps running");
    assert_eq!(
        worker.queue.iter().copied().collect::<Vec<_>>(),
        vec![later],
        "only the addressed promise leaves the queue"
    );
    assert_eq!(state.player(PlayerId(0)).scrap, scrap_before);
    assert!(
        state
            .buildings()
            .iter()
            .all(|building| building.anchor != spot)
    );

    let mut baseline = state.clone();
    let rejected = state.tick(&[cmd(
        0,
        Command::CancelFound {
            kind: BuildingKind::Turret,
            anchor: spot,
        },
    )]);
    baseline.tick(&[]);
    assert!(rejected.events.iter().any(|event| matches!(
        event,
        Event::CommandRejected {
            reason: RejectReason::InvalidTarget,
            ..
        }
    )));
    assert_eq!(
        state.hash(),
        baseline.hash(),
        "a stale logical site cannot edit the next leg"
    );
}

#[test]
fn cancelling_an_active_deferred_site_promotes_the_next_leg() {
    use oxide_sim::stats::BuildingKind;

    let (mut state, builder, spot) = deferred_founder_fixture();
    let later = Order::Move {
        goal: TilePos::new(3, 5),
    };
    let scrap_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: spot,
            queue: false,
            defer: true,
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(3, 5),
            queue: true,
        },
    )]);

    state.tick(&[cmd(
        0,
        Command::CancelFound {
            kind: BuildingKind::Turret,
            anchor: spot,
        },
    )]);
    let worker = state.unit(builder).unwrap();
    assert_eq!(worker.order, later);
    assert!(worker.queue.is_empty());
    assert_eq!(state.player(PlayerId(0)).scrap, scrap_before);
    assert!(
        state
            .buildings()
            .iter()
            .all(|building| building.anchor != spot)
    );
}

#[test]
fn cancelling_a_deferred_site_drops_the_whole_builder_crew() {
    use oxide_sim::stats::BuildingKind;

    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 5),
        unit(0, UnitKind::Harvester, 4, 6),
    ])
    .build()
    .unwrap();
    let crew: Vec<_> = state.units().iter().map(|unit| unit.id).collect();
    let anchor = TilePos::new(9, 5);
    let scrap_before = state.player(PlayerId(0)).scrap;

    let placed = state.tick(&[cmd(
        0,
        Command::Build {
            units: crew.clone(),
            kind: BuildingKind::Turret,
            anchor,
            queue: false,
            defer: true,
        },
    )]);
    assert!(
        !placed
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. }))
    );
    assert!(crew.iter().all(|id| matches!(
        state.unit(*id).unwrap().order,
        Order::Found {
            kind: BuildingKind::Turret,
            anchor: found_anchor,
        } if found_anchor == anchor
    )));

    let cancelled = state.tick(&[cmd(
        0,
        Command::CancelFound {
            kind: BuildingKind::Turret,
            anchor,
        },
    )]);
    assert!(
        !cancelled
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. }))
    );
    assert!(crew.iter().all(|id| {
        let worker = state.unit(*id).unwrap();
        matches!(worker.order, Order::Idle) && worker.queue.is_empty()
    }));
    assert_eq!(state.player(PlayerId(0)).scrap, scrap_before);

    for _ in 0..100 {
        state.tick(&[]);
    }
    assert!(
        state
            .buildings()
            .iter()
            .all(|building| building.anchor != anchor),
        "a crewmate cannot resurrect the cancelled logical site"
    );
}

#[test]
fn repeated_pending_cancellation_cannot_retarget_the_next_site() {
    use oxide_sim::stats::BuildingKind;

    let mut state = arena(vec![unit(0, UnitKind::Harvester, 4, 6)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let first = TilePos::new(9, 5);
    let second = TilePos::new(9, 7);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![builder],
                goal: TilePos::new(3, 2),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Build {
                units: vec![builder],
                kind: BuildingKind::Turret,
                anchor: first,
                queue: true,
                defer: true,
            },
        ),
        cmd(
            0,
            Command::Build {
                units: vec![builder],
                kind: BuildingKind::Turret,
                anchor: second,
                queue: true,
                defer: true,
            },
        ),
    ]);
    assert_eq!(
        state
            .unit(builder)
            .unwrap()
            .queue
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![
            Order::Found {
                kind: BuildingKind::Turret,
                anchor: first,
            },
            Order::Found {
                kind: BuildingKind::Turret,
                anchor: second,
            },
        ]
    );

    let report = state.tick(&[
        cmd(
            0,
            Command::CancelFound {
                kind: BuildingKind::Turret,
                anchor: first,
            },
        ),
        cmd(
            0,
            Command::CancelFound {
                kind: BuildingKind::Turret,
                anchor: first,
            },
        ),
    ]);
    assert_eq!(
        report
            .events
            .iter()
            .filter(|event| matches!(
                event,
                Event::CommandRejected {
                    reason: RejectReason::InvalidTarget,
                    ..
                }
            ))
            .count(),
        1,
        "the repeated click becomes stale instead of hitting another site"
    );
    assert_eq!(
        state
            .unit(builder)
            .unwrap()
            .queue
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        vec![Order::Found {
            kind: BuildingKind::Turret,
            anchor: second,
        }]
    );
}

#[test]
fn cancelling_a_paid_queued_site_removes_only_its_build_leg() {
    use oxide_sim::stats::BuildingKind;

    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 4, 6)]);
    scenario.players[0].scrap = 1000;
    let mut state = scenario.build().unwrap();
    let builder = state.units()[0].id;
    let first_anchor = TilePos::new(5, 6);
    let second_anchor = TilePos::new(8, 6);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: first_anchor,
            queue: false,
            defer: false,
        },
    )]);
    let first = state
        .buildings()
        .iter()
        .find(|building| building.anchor == first_anchor)
        .unwrap()
        .id;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: second_anchor,
            queue: true,
            defer: false,
        },
    )]);
    let second = state
        .buildings()
        .iter()
        .find(|building| building.anchor == second_anchor)
        .unwrap()
        .id;
    let later = Order::Move {
        goal: TilePos::new(3, 2),
    };
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(3, 2),
            queue: true,
        },
    )]);
    let second_site = state.building(second).unwrap();
    let stats = second_site.kind.stats();
    let expected_refund = stats.construction.unwrap().cost * second_site.hp / stats.max_hp;
    let scrap_before = state.player(PlayerId(0)).scrap;

    state.tick(&[cmd(0, Command::Cancel { building: second })]);

    let worker = state.unit(builder).unwrap();
    assert_eq!(worker.order, Order::Build { site: first });
    assert_eq!(
        worker.queue.iter().copied().collect::<Vec<_>>(),
        vec![later],
        "the later leg survives the cancelled paid site"
    );
    assert!(state.building(first).is_some());
    assert!(state.building(second).is_none());
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        scrap_before + expected_refund
    );
}

/// Reissuing a deferred claim with replacement semantics must be accepted:
/// the selected founder's old claim is the program being replaced, not a
/// competing reservation.
#[test]
fn reissuing_a_deferred_build_ignores_the_selected_founders_claim() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 12, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let spot = TilePos::new(12, 1);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| !s.can_see(PlayerId(0), spot));
    assert!(state.vision(PlayerId(0)).explored(spot));

    let deferred = |queue| Command::Build {
        units: vec![builder],
        kind: BuildingKind::Turret,
        anchor: spot,
        queue,
        defer: true,
    };
    let first = state.tick(&[cmd(0, deferred(false))]);
    assert!(
        !first
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. }))
    );
    let queued = state.tick(&[cmd(0, deferred(true))]);
    assert!(queued.events.iter().any(|event| matches!(
        event,
        Event::CommandRejected {
            reason: RejectReason::BadSite,
            ..
        }
    )));
    assert!(
        state.unit(builder).unwrap().queue.is_empty(),
        "a queued reissue preserves the current claim instead of duplicating it"
    );

    let second = state.tick(&[cmd(0, deferred(false))]);
    assert!(
        !second
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. })),
        "a replacement command must not collide with the claim it replaces"
    );
    assert_eq!(
        state.unit(builder).unwrap().order,
        Order::Found {
            kind: BuildingKind::Turret,
            anchor: spot,
        }
    );
}

/// Retargeting a replacement command may overlap the selected founder's old
/// footprint because that old claim disappears when the new program lands.
#[test]
fn retargeting_a_deferred_build_ignores_the_replaced_footprint() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 12, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let old_spot = TilePos::new(12, 1);
    let new_spot = TilePos::new(13, 1);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        !s.can_see(PlayerId(0), old_spot) && !s.can_see(PlayerId(0), new_spot)
    });
    assert!(state.vision(PlayerId(0)).explored(new_spot));

    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Fabricator,
            anchor: old_spot,
            queue: false,
            defer: true,
        },
    )]);
    assert_eq!(
        state.unit(builder).unwrap().order,
        Order::Found {
            kind: BuildingKind::Fabricator,
            anchor: old_spot,
        }
    );
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Fabricator,
            anchor: new_spot,
            queue: false,
            defer: true,
        },
    )]);
    assert!(
        !report
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandRejected { .. })),
        "the selected founder's overlapping old claim is being replaced"
    );
    assert_eq!(
        state.unit(builder).unwrap().order,
        Order::Found {
            kind: BuildingKind::Fabricator,
            anchor: new_spot,
        }
    );
}

/// Claims owned by harvesters outside the replacement selection remain real
/// reservations and must continue to protect their footprints.
#[test]
fn a_deferred_build_respects_an_unselected_founders_claim() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 12, 2),
        unit(0, UnitKind::Harvester, 13, 2),
    ])
    .build()
    .unwrap();
    let first = state.units()[0].id;
    let second = state.units()[1].id;
    let old_spot = TilePos::new(12, 1);
    let overlapping_spot = TilePos::new(13, 1);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![first, second],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        !s.can_see(PlayerId(0), old_spot) && !s.can_see(PlayerId(0), overlapping_spot)
    });

    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![first],
            kind: BuildingKind::Fabricator,
            anchor: old_spot,
            queue: false,
            defer: true,
        },
    )]);
    assert_eq!(
        state.unit(first).unwrap().order,
        Order::Found {
            kind: BuildingKind::Fabricator,
            anchor: old_spot,
        }
    );
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![second],
            kind: BuildingKind::Fabricator,
            anchor: overlapping_spot,
            queue: false,
            defer: true,
        },
    )]);
    assert!(report.events.iter().any(|event| matches!(
        event,
        Event::CommandRejected {
            reason: RejectReason::BadSite,
            ..
        }
    )));
    assert!(
        !matches!(
            state.unit(second).unwrap().order,
            Order::Found {
                kind: BuildingKind::Fabricator,
                anchor,
            } if anchor == overlapping_spot
        ),
        "an unselected founder's claim must not be replaced"
    );
}

/// Ground honestly taken while the founder walked: the arrival re-check
/// discovers the blocker with the founder's own eyes, drops the program
/// with the fog-safe stall, and never charges a coin.
#[test]
fn a_deferred_claim_on_taken_ground_stalls_without_spending() {
    use oxide_sim::event::StallReason;
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 12, 2),
        unit(1, UnitKind::Harvester, 13, 3),
    ])
    .build()
    .unwrap();
    let (founder, rival) = (state.units()[0].id, state.units()[1].id);
    let spot = TilePos::new(12, 1);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![founder],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| !s.can_see(PlayerId(0), spot));
    // The rival claims the corner while nobody watches.
    state.tick(&[cmd(
        1,
        Command::Build {
            units: vec![rival],
            kind: BuildingKind::Turret,
            anchor: spot,
            queue: false,
            defer: false,
        },
    )]);
    assert!(state.buildings().iter().any(|b| b.anchor == spot));

    let scrap_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![founder],
            kind: BuildingKind::Turret,
            anchor: spot,
            queue: false,
            defer: true,
        },
    )]);
    assert_eq!(
        state.unit(founder).unwrap().order,
        Order::Found {
            kind: BuildingKind::Turret,
            anchor: spot
        },
        "memory knows nothing of the rival's site, so the intent stands"
    );
    let events = run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { .. }))
    });
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::OrderStalled {
                unit,
                reason: StallReason::GroundTaken,
                ..
            } if *unit == founder
        )),
        "the arrival re-check names the taken ground"
    );
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        scrap_before,
        "a claim that never landed cost nothing"
    );
    assert_eq!(
        state.unit(founder).unwrap().order,
        Order::Idle,
        "the program dropped"
    );
    assert_eq!(
        state
            .buildings()
            .iter()
            .filter(|b| b.anchor == spot)
            .count(),
        1,
        "only the rival's site stands"
    );
}

/// Stop is the cancel: with nothing placed and nothing paid at accept,
/// abandoning a pending found needs no refund machinery at all.
#[test]
fn a_stopped_pending_found_spends_nothing() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 12, 2)])
        .build()
        .unwrap();
    let builder = state.units()[0].id;
    let spot = TilePos::new(12, 1);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| !s.can_see(PlayerId(0), spot));
    let scrap_before = state.player(PlayerId(0)).scrap;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: spot,
            queue: false,
            defer: true,
        },
    )]);
    state.tick(&[]);
    state.tick(&[cmd(
        0,
        Command::Stop {
            units: vec![builder],
        },
    )]);
    for _ in 0..300 {
        state.tick(&[]);
    }
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        scrap_before,
        "no charge ever landed"
    );
    assert!(
        state.buildings().iter().all(|b| b.anchor != spot),
        "no site ever appeared"
    );
}

/// A rejected deferred build (never-explored footprint) must leave no
/// trace on the hash, like every other rejected command.
#[test]
fn a_rejected_deferred_build_leaves_no_trace_on_the_hash() {
    use oxide_sim::stats::BuildingKind;
    let scenario = arena(vec![unit(0, UnitKind::Harvester, 4, 6)]);
    let mut with_reject = scenario.build().unwrap();
    let mut pristine = scenario.build().unwrap();
    let builder = with_reject.units()[0].id;
    let unscouted = TilePos::new(12, 1);
    assert!(!with_reject.vision(PlayerId(0)).explored(unscouted));
    let report = with_reject.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: unscouted,
            queue: false,
            defer: true,
        },
    )]);
    assert!(report.events.iter().any(|e| matches!(
        e,
        Event::CommandRejected {
            reason: RejectReason::BadSite,
            ..
        }
    )));
    pristine.tick(&[]);
    assert_eq!(
        with_reject.hash(),
        pristine.hash(),
        "a rejected deferred build must not move the state hash"
    );
}

/// The information boundary itself: two worlds differing ONLY in what
/// the issuer's fog hides must return the same intent verdict for every
/// anchor on the map — the amber ghost can never be a hidden-enemy
/// detector.
#[test]
fn intent_verdicts_ignore_what_fog_hides() {
    use oxide_sim::stats::BuildingKind;
    let build = |ambush: bool| {
        let mut units = vec![unit(0, UnitKind::Harvester, 30, 4)];
        if ambush {
            // A hostile squad starting on never-explored ground, far
            // from the scout's path.
            units.push(unit(1, UnitKind::Scuttler, 34, 15));
            units.push(unit(1, UnitKind::Scuttler, 35, 15));
        }
        open_arena(40, 20, units).build().unwrap()
    };
    let mut plain = build(false);
    let mut ambushed = build(true);
    let spot = TilePos::new(30, 3);
    // March the scout home in both worlds; identical command stream.
    for state in [&mut plain, &mut ambushed] {
        let scout = state.units()[0].id;
        state.tick(&[cmd(
            0,
            Command::Move {
                units: vec![scout],
                goal: TilePos::new(3, 5),
                queue: false,
            },
        )]);
        run_until(state, 800, |s, _| !s.can_see(PlayerId(0), spot));
        assert!(state.vision(PlayerId(0)).explored(spot));
    }
    // Only now does the squad creep onto the remembered ground — the
    // issuer never sees it arrive.
    let squad: Vec<UnitId> = ambushed
        .units()
        .iter()
        .filter(|u| u.player == PlayerId(1))
        .map(|u| u.id)
        .collect();
    ambushed.tick(&[cmd(
        1,
        Command::Move {
            units: squad.clone(),
            goal: spot,
            queue: false,
        },
    )]);
    run_until(&mut ambushed, 800, |s, _| {
        squad
            .iter()
            .all(|id| s.unit(*id).unwrap().tile().manhattan(spot) <= 2)
    });
    // Keep the plain world's clock even with the ambushed one so the
    // only difference left IS the hidden squad.
    while plain.current_tick() < ambushed.current_tick() {
        plain.tick(&[]);
    }
    for id in &squad {
        let t = ambushed.unit(*id).unwrap().tile();
        assert!(
            !ambushed.can_see(PlayerId(0), t),
            "the squad must stay hidden for the property to mean anything"
        );
    }
    // The hidden squad must not tilt a single verdict anywhere.
    for y in 0..plain.map().height() {
        for x in 0..plain.map().width() {
            let anchor = TilePos::new(x, y);
            for kind in [BuildingKind::Turret, BuildingKind::Fabricator] {
                let a = plain.place_intent_refusal(PlayerId(0), kind, anchor);
                let b = ambushed.place_intent_refusal(PlayerId(0), kind, anchor);
                assert_eq!(
                    a, b,
                    "verdict for {kind:?} at {anchor:?} leaked hidden enemies"
                );
            }
        }
    }
}

/// A whole crew defers together: the lowest-id arrival founds exactly
/// one site and every other hand joins the same build.
#[test]
fn a_deferred_crew_founds_once_and_stacks() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 12, 2),
        unit(0, UnitKind::Harvester, 11, 2),
    ])
    .build()
    .unwrap();
    let crew: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    let spot = TilePos::new(12, 1);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: crew.clone(),
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| !s.can_see(PlayerId(0), spot));
    state.tick(&[cmd(
        0,
        Command::Build {
            units: crew.clone(),
            kind: BuildingKind::Turret,
            anchor: spot,
            queue: false,
            defer: true,
        },
    )]);
    for id in &crew {
        assert!(matches!(
            state.unit(*id).unwrap().order,
            Order::Found { .. }
        ));
    }
    run_until(&mut state, 600, |s, _| {
        s.buildings().iter().any(|b| b.anchor == spot)
    });
    run_until(&mut state, 200, |s, _| {
        crew.iter()
            .all(|id| matches!(s.unit(*id).map(|u| u.order), Some(Order::Build { .. })))
    });
    assert_eq!(
        state
            .buildings()
            .iter()
            .filter(|b| b.anchor == spot)
            .count(),
        1,
        "one claim, however many hands"
    );
}

/// A crewmate arriving after the crew already FINISHED the building is
/// done, not stalled: its founding succeeded by other hands, and
/// reading its own standing building as taken ground would mislabel
/// success as failure.
#[test]
fn a_late_crewmate_finds_its_building_finished_and_calls_it_done() {
    use oxide_sim::stats::BuildingKind;
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 12, 2),
        // The straggler starts far enough back that the founder finishes
        // the cheap turret before it arrives.
        unit(0, UnitKind::Harvester, 2, 6),
    ])
    .build()
    .unwrap();
    let crew: Vec<UnitId> = state.units().iter().map(|u| u.id).collect();
    let straggler = crew[1];
    let spot = TilePos::new(12, 1);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: crew.clone(),
            kind: BuildingKind::Turret,
            anchor: spot,
            queue: false,
            defer: true,
        },
    )]);
    run_until(&mut state, 2_000, |s, _| {
        s.buildings().iter().any(|b| b.anchor == spot && b.built)
    });
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.anchor == spot && b.built),
        "the founder must finish before the straggler arrives for this test to bite"
    );
    let events = run_until(&mut state, 600, |s, _| {
        s.unit(straggler)
            .is_some_and(|u| matches!(u.order, Order::Idle))
    });
    assert!(
        matches!(state.unit(straggler).unwrap().order, Order::Idle),
        "the late crewmate ends done, not stalled"
    );
    assert!(
        !events.iter().any(|e| matches!(
            e,
            Event::OrderStalled { unit, .. } if *unit == straggler
        )),
        "success by other hands is not a stall"
    );
}

/// A bank that ran dry before arrival stalls the founder with the
/// existing own-state reason — affordability is judged when the ground
/// is claimed, not when the intent is spoken.
#[test]
fn a_broke_founder_stalls_on_arrival() {
    use oxide_sim::event::StallReason;
    use oxide_sim::stats::BuildingKind;
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 12, 2)]);
    let cost = BuildingKind::Turret.stats().construction.unwrap().cost;
    scenario.players[0].scrap = cost;
    let mut state = scenario.build().unwrap();
    let builder = state.units()[0].id;
    let spot = TilePos::new(12, 1);
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![builder],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| !s.can_see(PlayerId(0), spot));
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: spot,
            queue: false,
            defer: true,
        },
    )]);
    // Drain the bank while the founder walks: train a unit at the
    // Foundry (paid on enqueue).
    let foundry = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(0))
        .unwrap()
        .id;
    state.tick(&[cmd(
        0,
        Command::Train {
            building: foundry,
            kind: UnitKind::Harvester,
        },
    )]);
    assert!(state.player(PlayerId(0)).scrap < cost);
    let events = run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::OrderStalled { .. }))
    });
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::OrderStalled {
                unit,
                reason: StallReason::InsufficientScrap,
                ..
            } if *unit == builder
        )),
        "the broke founder stalls with the existing reason"
    );
    assert!(state.buildings().iter().all(|b| b.anchor != spot));
}
