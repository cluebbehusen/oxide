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
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: oxide_sim::stats::BuildingKind::FlakTurret,
            anchor: TilePos::new(9, 2),
            queue: true,
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
        },
    )]);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: oxide_sim::stats::BuildingKind::FlakTurret,
            anchor: TilePos::new(9, 2),
            queue: true,
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
