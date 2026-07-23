//! Engine edge cases the behavior suites leave uncovered: a ground chaser
//! that runs out of standing room against a flyer walled inside rock, the
//! sidearm's independence from the chassis' ordered target, the buffered-
//! resolution rule that a same-tick corpse draws no retaliation, two
//! command reject reasons a player can actually trigger, and the radar
//! ring's exact detection boundary. Public API only, like `domains.rs`.

use chassis::grid::TilePos;
use oxide_sim::command::RejectReason;
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::stats::BuildingKind;
use oxide_sim::{
    Command, Event, Faction, Order, PlayerCommand, PlayerId, Scenario, State, Target, UnitKind,
};

fn players() -> Vec<PlayerSpec> {
    vec![
        PlayerSpec {
            name: "Ferrous".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap: 200,
            bot: false,
            bot_config: None,
        },
        PlayerSpec {
            name: "Cupric".into(),
            faction: Faction::Cupric,
            team: None,
            scrap: 200,
            bot: false,
            bot_config: None,
        },
    ]
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

/// The 16x9 arena `domains.rs` uses: a rock block at (6,3)-(7,4) and a
/// two-scrap column at (11,4)-(11,5).
fn arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "edge-arena".into(),
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
        players: players(),
        units,
        meta: None,
    }
}

#[test]
fn a_ground_chaser_stalls_when_no_standing_room_reaches_a_flyer_deep_in_rock() {
    // An 11x11 rock block fills (5,3)-(15,13); its center (10,8) is
    // Chebyshev-5 from every edge, so every tile a ground unit could stand
    // on lies outside both the stand-in scan and the weapon's reach (the
    // nearest footing is 6.0 tiles out, past the Flakhound's 5). A
    // Flakhound ordered onto a Wisp parked there can find neither range nor
    // footing — its order must stall rather than path into the wall or spin
    // forever. The chaser sits in sight of the flyer (vision 7) but out of
    // its aggro/range (5), and the Wisp descends straight down its own
    // column, never within 5 tiles of the chaser — so no mid-flight
    // auto-acquire drags the fight open early.
    let scenario = Scenario {
        name: "walled-flyer".into(),
        seed: 42,
        map: vec![
            "#####################".into(),
            "#1..................#".into(),
            "#...................#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#....###########....#".into(),
            "#.................2.#".into(),
            "#...................#".into(),
            "#####################".into(),
        ],
        players: players(),
        units: vec![
            unit(0, UnitKind::Flakhound, 4, 8),
            unit(1, UnitKind::Wisp, 10, 1),
        ],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let (flak, wisp) = (state.units()[0].id, state.units()[1].id);

    // Fly the wisp into the heart of the rock and let it settle.
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![wisp],
            goal: TilePos::new(10, 8),
            queue: false,
        },
    )]);
    run_until(&mut state, 200, |s, _| {
        s.unit(wisp).unwrap().tile() == TilePos::new(10, 8)
    });
    assert_eq!(
        state.unit(flak).unwrap().order,
        Order::Idle,
        "the chaser never woke on its own during the descent"
    );

    // Now order the impossible chase: no footing within reach, so it stalls
    // on the very tick it takes the order.
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![flak],
            target: Target::Unit(wisp),
            queue: false,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::OrderStalled {
                unit,
                reason: oxide_sim::StallReason::NoFiringPosition,
                ..
            } if *unit == flak
        )),
        "no standing room in reach ends the order in a stall that says so"
    );
    assert_eq!(
        state.unit(flak).unwrap().order,
        Order::Idle,
        "a stalled program drops to idle"
    );
    assert_eq!(
        state.unit(wisp).unwrap().hp,
        UnitKind::Wisp.stats().max_hp,
        "the flyer sits untouched behind the rock"
    );
}

#[test]
fn a_fogged_flyer_footing_never_leaks_through_the_stall_reason() {
    // The fog-safety rule for stall reasons: NoFiringPosition derives
    // from the victim's footing, so it may only be spoken while the
    // team sees that ground. Here the chase is ordered in sight, the
    // flyer then parks deep inside a rock slab far beyond the chaser's
    // vision — the stall must say NoRoute, because saying
    // NoFiringPosition would tell the player the unseen flyer sits
    // over impassable ground.
    let scenario = Scenario {
        name: "fogged-flyer".into(),
        seed: 42,
        map: vec![
            "##########################".into(),
            "#1.......................#".into(),
            "#........................#".into(),
            "#........................#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#....##################..#".into(),
            "#......................2.#".into(),
            "#........................#".into(),
            "##########################".into(),
        ],
        players: players(),
        units: vec![
            unit(0, UnitKind::Flakhound, 4, 2),
            unit(1, UnitKind::Wisp, 10, 2),
        ],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let (flak, wisp) = (state.units()[0].id, state.units()[1].id);
    assert!(
        state.can_see(PlayerId(0), state.unit(wisp).unwrap().tile()),
        "the chase is ordered against a visible flyer"
    );

    // Both orders land the same tick: the chase begins in sight, and
    // the flyer heads for the heart of the slab, far past vision.
    state.tick(&[
        cmd(
            0,
            Command::Attack {
                units: vec![flak],
                target: Target::Unit(wisp),
                queue: false,
            },
        ),
        cmd(
            1,
            Command::Move {
                units: vec![wisp],
                goal: TilePos::new(14, 11),
                queue: false,
            },
        ),
    ]);

    let mut stalled = None;
    for _ in 0..600 {
        let report = state.tick(&[]);
        if let Some(reason) = report.events.iter().find_map(|e| match e {
            Event::OrderStalled { unit, reason, .. } if *unit == flak => Some(*reason),
            _ => None,
        }) {
            stalled = Some((
                reason,
                state.can_see(PlayerId(0), state.unit(wisp).unwrap().tile()),
            ));
            break;
        }
    }
    let (reason, saw_victim) = stalled.expect("the impossible chase stalls");
    assert!(
        !saw_victim,
        "the scenario must exercise the fogged case: the victim's tile is unseen at the stall"
    );
    assert_eq!(
        reason,
        oxide_sim::StallReason::NoRoute,
        "an unseen victim's footing must not narrow the reason to NoFiringPosition"
    );
}

#[test]
fn a_sidearm_downs_a_flyer_without_pulling_the_main_gun_off_its_order() {
    // A Sentinel ordered onto a ground Scuttler keeps that target while its
    // air sidearm independently works a Darter hovering in reach. The
    // opportunist shot must never re-steer the chassis: the ordered target
    // stays the scuttler even as the darter takes fire.
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 3, 6),
        unit(1, UnitKind::Scuttler, 5, 6),
        unit(1, UnitKind::Darter, 3, 4),
    ])
    .build()
    .unwrap();
    let (sentinel, scuttler, darter) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![sentinel],
            target: Target::Unit(scuttler),
            queue: false,
        },
    )]);

    // Watch a window in which the scuttler is still alive (it dies near
    // tick 60): every tick the chassis must still be aimed at it, and the
    // sky poke must land on the darter at least once.
    let mut air_hits = 0;
    for _ in 0..40 {
        let report = state.tick(&[]);
        for e in &report.events {
            if let Event::AttackHit {
                attacker,
                target: Target::Unit(t),
                ..
            } = e
                && *attacker == sentinel
                && *t == darter
            {
                air_hits += 1;
            }
        }
        assert!(
            matches!(
                state.unit(sentinel).unwrap().order,
                Order::Attack { target: Target::Unit(t), .. } if t == scuttler
            ),
            "the sidearm's war never steals the main gun's order"
        );
    }
    assert!(
        air_hits >= 1,
        "the air sidearm fought its own war against the flyer"
    );
    assert!(
        state.unit(darter).unwrap().hp < UnitKind::Darter.stats().max_hp,
        "the darter took sidearm fire"
    );
}

#[test]
fn a_sidearm_holds_fire_when_nothing_it_covers_is_in_range() {
    // The inverse: a Sentinel dueling a ground Scuttler with a Wisp sitting
    // six tiles off — seen (vision 7) but outside the air sidearm's three
    // tiles and outside aggro, so it never approaches. The sidearm must
    // stay silent; opportunist fire is range-gated, not a free swing at
    // anything visible.
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 3, 6),
        unit(1, UnitKind::Scuttler, 5, 6),
        unit(1, UnitKind::Wisp, 9, 6),
    ])
    .build()
    .unwrap();
    let (sentinel, scuttler, wisp) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![sentinel],
            target: Target::Unit(scuttler),
            queue: false,
        },
    )]);
    let mut air_hits = 0;
    let mut ground_hits = 0;
    for _ in 0..40 {
        let report = state.tick(&[]);
        for e in &report.events {
            if let Event::AttackHit {
                attacker,
                target: Target::Unit(t),
                ..
            } = e
                && *attacker == sentinel
            {
                if *t == wisp {
                    air_hits += 1;
                } else if *t == scuttler {
                    ground_hits += 1;
                }
            }
        }
    }
    assert!(
        ground_hits >= 1,
        "the sentinel is genuinely engaged (main gun firing)"
    );
    assert_eq!(
        air_hits, 0,
        "an out-of-range flyer draws no sidearm fire, seen or not"
    );
    assert_eq!(
        state.unit(wisp).unwrap().hp,
        UnitKind::Wisp.stats().max_hp,
        "the wisp is untouched"
    );
}

#[test]
fn a_dead_attacker_draws_no_answer_and_the_earliest_survivor_gets_it() {
    // Three Lancers open on a Sentinel from 5.1-5.4 tiles — inside lancer
    // range (5.5), outside the Sentinel's aggro (5), so it stands idle and
    // can only answer through retaliation. The earliest attacker (lowest
    // id) is cut down the same tick by two allied Lancers, so its shot
    // buffers but its body is gone before retaliation resolves. The answer
    // must skip the corpse and land on the earliest attacker that actually
    // survived the volley — not the later one, not nobody. Both Foundries
    // sit far from the victim, or the idle Sentinel would auto-acquire an
    // enemy base inside its aggro and never reach the retaliation branch.
    let scenario = Scenario {
        name: "buffered-answer".into(),
        seed: 42,
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
            "#..............#".into(),
            "#..............#".into(),
            "#..............#".into(),
            "#2.............#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: players(),
        units: vec![
            unit(0, UnitKind::Sentinel, 10, 10), // victim
            unit(1, UnitKind::Lancer, 5, 9),     // A: earliest, dies this tick
            unit(1, UnitKind::Lancer, 5, 11),    // B: earliest survivor
            unit(1, UnitKind::Lancer, 5, 8),     // C: a later survivor
            unit(0, UnitKind::Lancer, 4, 9),     // executioner
            unit(0, UnitKind::Lancer, 4, 10),    // executioner
        ],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let ids: Vec<_> = state.units().iter().map(|u| u.id).collect();
    let (victim, a, b, c, k1, k2) = (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);

    let report = state.tick(&[
        cmd(
            1,
            Command::Attack {
                units: vec![a, b, c],
                target: Target::Unit(victim),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Attack {
                units: vec![k1, k2],
                target: Target::Unit(a),
                queue: false,
            },
        ),
    ]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == a)),
        "the earliest attacker fell on the tick it fired"
    );
    assert!(state.unit(b).is_some() && state.unit(c).is_some());
    assert_eq!(
        state.unit(victim).unwrap().order,
        Order::Attack {
            target: Target::Unit(b),
            resume: None
        },
        "the answer skips the corpse and lands on the earliest survivor"
    );
}

#[test]
fn harvest_on_barren_ground_is_rejected_as_not_a_node() {
    // A tile that holds no scrap, no wreck, and no remembered salvage is not
    // a source — ordering harvesters onto it bounces.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 3, 2)])
        .build()
        .unwrap();
    let harvester = state.units()[0].id;
    let barren = TilePos::new(3, 3);
    assert_eq!(state.map().scrap_at(barren), 0);
    assert_eq!(state.map().wreck_at(barren), 0);
    let report = state.tick(&[cmd(
        0,
        Command::Harvest {
            units: vec![harvester],
            node: barren,
            queue: false,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::NotANode,
                ..
            }
        )),
        "empty ground is not a node"
    );
}

#[test]
fn building_a_footprint_over_rock_is_rejected_as_a_bad_site() {
    // The arena's rock block sits at (6,3)-(7,4), in the harvester's sight.
    // A turret cannot stand on rock; the placement bounces as a bad site
    // before any scrap changes hands.
    let mut state = arena(vec![unit(0, UnitKind::Harvester, 5, 3)])
        .build()
        .unwrap();
    let harvester = state.units()[0].id;
    let rock = TilePos::new(6, 3);
    assert!(
        state.can_see(PlayerId(0), rock),
        "test premise: the footprint is visible, so rejection is about the terrain"
    );
    assert!(!state.can_place(PlayerId(0), BuildingKind::Turret, rock));
    let scrap_before = state.players()[0].scrap;
    let report = state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![harvester],
            kind: BuildingKind::Turret,
            anchor: rock,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: RejectReason::BadSite,
                ..
            }
        )),
        "rock is not buildable ground"
    );
    assert_eq!(
        state.players()[0].scrap,
        scrap_before,
        "a refused site charges nothing"
    );
    assert!(
        state.buildings().iter().all(|b| b.anchor != rock),
        "nothing was placed"
    );
}

#[test]
fn radar_detects_at_the_ring_and_goes_quiet_one_tile_beyond() {
    // The Array's detection ring is 16 tiles, measured in squared distance
    // from the anchor: a hostile tile at exactly 16 (dx 16, dy 0 -> 256) is
    // a blip; one tile deeper (dx 16, dy 1 -> 257) is past the ring and
    // invisible. Two idle enemy harvesters straddle that edge, both far
    // outside any friendly true sight.
    let scenario = Scenario {
        name: "radar-edge".into(),
        seed: 42,
        map: vec![
            "######################".into(),
            "#1...................#".into(),
            "#....................#".into(),
            "#....................#".into(),
            "#....................#".into(),
            "#....................#".into(),
            "#................2...#".into(),
            "#....................#".into(),
            "######################".into(),
        ],
        players: players(),
        units: vec![
            unit(0, UnitKind::Harvester, 4, 2),  // builder
            unit(1, UnitKind::Harvester, 20, 4), // on the ring: dx16 dy0
            unit(1, UnitKind::Harvester, 20, 5), // one deeper: dx16 dy1
        ],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let (builder, on_ring, past_ring) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Array,
            anchor: TilePos::new(4, 4),
        },
    )]);
    run_until(&mut state, 500, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    });
    state.tick(&[]);

    let (ring_tile, past_tile) = (
        state.unit(on_ring).unwrap().tile(),
        state.unit(past_ring).unwrap().tile(),
    );
    let view = state.vision(PlayerId(0));
    assert!(
        !view.visible(ring_tile) && !view.visible(past_tile),
        "test premise: both intruders are out of true sight"
    );
    assert!(
        view.contacts().contains(&ring_tile),
        "a tile exactly on the ring is a blip"
    );
    assert!(
        !view.contacts().contains(&past_tile),
        "one tile past the ring falls silent"
    );
}

#[test]
fn a_ground_chaser_flanks_to_a_firing_position_it_can_actually_shoot_from() {
    // A 7x7 rock block: the scan's first passable candidates are the ring-4
    // corners at 5.66 tiles — past the Flakhound's 5 — but the ring's edge
    // tiles sit at 4.0-5.0 and are honest firing positions. The chaser must
    // reject the tempting-but-useless corner, route to a tile it can shoot
    // from, and take the kill. (Before range-aware selection this soft-
    // locked: the chaser parked on the corner forever, out of range.)
    let scenario = Scenario {
        name: "flanked-flyer".into(),
        seed: 42,
        map: vec![
            "################".into(),
            "#1.............#".into(),
            "#..............#".into(),
            "#...#######....#".into(),
            "#...#######....#".into(),
            "#...#######....#".into(),
            "#...#######....#".into(),
            "#...#######....#".into(),
            "#...#######....#".into(),
            "#...#######....#".into(),
            "#..............#".into(),
            "#...........2..#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: players(),
        units: vec![
            unit(0, UnitKind::Flakhound, 1, 6),
            unit(1, UnitKind::Wisp, 7, 1),
        ],
        meta: None,
    };
    let mut state = scenario.build().unwrap();
    let (flak, wisp) = (state.units()[0].id, state.units()[1].id);

    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![wisp],
            goal: TilePos::new(7, 6),
            queue: false,
        },
    )]);
    run_until(&mut state, 200, |s, _| {
        s.unit(wisp).unwrap().tile() == TilePos::new(7, 6)
    });
    assert_eq!(state.unit(flak).unwrap().order, Order::Idle);

    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![flak],
            target: Target::Unit(wisp),
            queue: false,
        },
    )]);
    run_until(&mut state, 600, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == wisp))
    });
}
