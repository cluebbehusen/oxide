//! Engine tests for the weapons matrix and movement domains: air flight,
//! domain-filtered targeting, splash, indirect fire, and the fire gate
//! that makes long guns spotter weapons. Headless scenarios through the
//! public API only, like `behavior.rs`.

use chassis::grid::TilePos;
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, Domain, Role};
use oxide_sim::{
    Command, Event, Faction, Order, PlayerCommand, PlayerId, Scenario, State, Target, UnitKind,
};

/// A small arena: two Foundries in opposite corners, a rock block and a
/// scrap column in the middle ground.
fn arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "domain-arena".into(),
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
fn roles_resolve_consistently_per_faction() {
    for role in [
        Role::Harvester,
        Role::Sentinel,
        Role::Scuttler,
        Role::Lancer,
        Role::Bombard,
        Role::AntiAir,
        Role::AirGround,
        Role::AirAir,
    ] {
        for faction in [Faction::Ferrous, Faction::Cupric] {
            let kind = role.unit_for(faction);
            assert_eq!(kind.role(), role, "{kind:?} must map back to its role");
            assert!(
                kind.faction().is_none() || kind.faction() == Some(faction),
                "{kind:?} dealt to the wrong faction"
            );
        }
    }
}

#[test]
fn air_flies_straight_over_rock_and_may_park_on_it() {
    // The rock block spans (6,3)-(7,4); the straight line from (4,4) to
    // (9,4) crosses it. A flyer takes the line; ground must detour.
    let mut state = arena(vec![unit(0, UnitKind::Buzzard, 4, 4)])
        .build()
        .unwrap();
    let flyer = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![flyer],
            goal: TilePos::new(9, 4),
            queue: false,
        },
    )]);
    let mut crossed_rock = false;
    run_until(&mut state, 70, |s, _| {
        let u = s.unit(flyer).unwrap();
        let t = u.tile();
        if t == TilePos::new(6, 4) || t == TilePos::new(7, 4) {
            crossed_rock = true;
        }
        t == TilePos::new(9, 4) && u.order == Order::Idle
    });
    assert!(crossed_rock, "a straight flight must pass over the rock");

    // A rock tile is a legal destination for a flyer.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![flyer],
            goal: TilePos::new(6, 3),
            queue: false,
        },
    )]);
    run_until(&mut state, 80, |s, _| {
        s.unit(flyer).unwrap().tile() == TilePos::new(6, 3)
    });
}

#[test]
fn collision_pairs_only_within_a_domain() {
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 4, 2),
        unit(0, UnitKind::Buzzard, 9, 2),
        unit(0, UnitKind::Wisp, 9, 3),
    ])
    .build()
    .unwrap();
    let (sentinel, buzzard, wisp) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    // Both flyers head for the sentinel's tile.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![buzzard, wisp],
            goal: TilePos::new(4, 2),
            queue: false,
        },
    )]);
    for _ in 0..120 {
        state.tick(&[]);
    }
    let (sp, bp, wp) = (
        state.unit(sentinel).unwrap().pos,
        state.unit(buzzard).unwrap().pos,
        state.unit(wisp).unwrap().pos,
    );
    let ground_air = sp.dist(bp).min(sp.dist(wp));
    let air_air = bp.dist(wp);
    assert!(
        ground_air < chassis::fx::Fx::lit("0.5"),
        "a flyer hovers over ground bodies without touching (dist {ground_air})"
    );
    assert!(
        air_air > chassis::fx::Fx::lit("0.5"),
        "two flyers are solid to each other (dist {air_air})"
    );
}

#[test]
fn weapon_masks_gate_acquisition_both_ways() {
    // A Flakhound parked beside an enemy harvester has nothing to say to
    // it; a Wisp drifting into aggro dies to the same gun. The Wisp
    // (air-to-air only) never answers a ground platform.
    let mut state = arena(vec![
        unit(0, UnitKind::Flakhound, 5, 5),
        unit(1, UnitKind::Harvester, 6, 5),
        unit(1, UnitKind::Wisp, 12, 7),
    ])
    .build()
    .unwrap();
    let (flak, harv, wisp) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    for _ in 0..50 {
        state.tick(&[]);
    }
    assert_eq!(
        state.unit(flak).unwrap().order,
        Order::Idle,
        "ground targets never wake an air-only gun"
    );
    assert_eq!(state.unit(harv).unwrap().hp, 60);

    // Send the wisp overhead: flak acquires and deletes it unanswered.
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![wisp],
            goal: TilePos::new(5, 5),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == wisp))
    });
    let flak = state.unit(flak).unwrap();
    assert_eq!(flak.hp, 120, "an air-to-air wing cannot scratch the ground");
}

#[test]
fn ground_only_weapons_cannot_answer_air() {
    let mut state = arena(vec![
        unit(0, UnitKind::Darter, 4, 2),
        unit(1, UnitKind::Scuttler, 6, 2),
    ])
    .build()
    .unwrap();
    let (darter, scuttler) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![darter],
            target: Target::Unit(scuttler),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::UnitDied { unit, .. } if *unit == scuttler))
    });
    assert_eq!(
        state.unit(darter).unwrap().hp,
        UnitKind::Darter.stats().max_hp,
        "the scuttler has no gun that reaches the sky"
    );
}

#[test]
fn splash_kills_the_cluster_in_one_shell() {
    let mut state = arena(vec![
        unit(0, UnitKind::Bombard, 4, 2),
        unit(1, UnitKind::Scuttler, 8, 2),
        unit(1, UnitKind::Scuttler, 7, 2),
        unit(1, UnitKind::Scuttler, 8, 3),
    ])
    .build()
    .unwrap();
    let target = state.units()[1].id;
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![state.units()[0].id],
            target: Target::Unit(target),
            queue: false,
        },
    )]);
    let died = report
        .events
        .iter()
        .filter(|e| matches!(e, Event::UnitDied { .. }))
        .count();
    assert_eq!(died, 3, "one shell, three wrecks — that is what splash is");
}

#[test]
fn splash_victims_all_turn_on_the_shooter() {
    let mut state = arena(vec![
        unit(0, UnitKind::Bombard, 4, 2),
        unit(1, UnitKind::Sentinel, 8, 2),
        unit(1, UnitKind::Sentinel, 9, 2),
    ])
    .build()
    .unwrap();
    let (bombard, s1, s2) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(s1),
            queue: false,
        },
    )]);
    for id in [s1, s2] {
        let u = state.unit(id).unwrap();
        assert!(u.hp < 100, "both sentinels took the shell");
        assert_eq!(
            u.order,
            Order::Attack {
                target: Target::Unit(bombard),
                resume: None
            },
            "every splash victim answers the gun that fired"
        );
    }
}

#[test]
fn indirect_fire_arcs_over_rock() {
    // The rock block (6,3)-(7,4) stands between the gun and its victim;
    // a Lancer would hold fire and approach, the Bombard shells through.
    let mut state = arena(vec![
        unit(0, UnitKind::Bombard, 4, 4),
        unit(1, UnitKind::Harvester, 9, 4),
    ])
    .build()
    .unwrap();
    let (bombard, victim) = (state.units()[0].id, state.units()[1].id);
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(victim),
            queue: false,
        },
    )]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::AttackHit { attacker, .. } if *attacker == bombard)),
        "indirect fire ignores the rock between"
    );
    assert_eq!(
        state.unit(bombard).unwrap().tile(),
        TilePos::new(4, 4),
        "no approach needed — the shell arcs"
    );
}

#[test]
fn long_guns_fire_on_a_spotters_eyes_and_go_quiet_without_them() {
    // A rock wall splits the map; the only way around is the southern
    // gap. The Bombard at (3,4) can reach the harvester at (12,4) —
    // range 9.5, straight-line distance 9 — but sees only 5, and its
    // crawl around the wall keeps it blind for a long time. The scuttler
    // at (10,3) holds the sight line; no other friendly eye reaches (the
    // Foundry's 8 falls short).
    let scenario = Scenario {
        name: "spotter-wall".into(),
        seed: 42,
        map: vec![
            "################".into(),
            "#1......#......#".into(),
            "#.......#......#".into(),
            "#.......#......#".into(),
            "#.......#......#".into(),
            "#.......#......#".into(),
            "#............2.#".into(),
            "#..............#".into(),
            "################".into(),
        ],
        players: arena(vec![]).players,
        units: vec![
            unit(0, UnitKind::Bombard, 3, 4),
            unit(0, UnitKind::Scuttler, 10, 3),
            unit(1, UnitKind::Harvester, 12, 4),
        ],
    };
    let mut state = scenario.build().unwrap();
    let (bombard, spotter, victim) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(victim),
            queue: false,
        },
    )]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::AttackHit { attacker, .. } if *attacker == bombard)),
        "with a spotter, the shell flies beyond the gun's own sight"
    );
    let hp_after_first = state.unit(victim).unwrap().hp;
    assert!(hp_after_first < 60);

    // Recall the spotter. Sight collapses as it rounds the wall; the
    // next shell comes off cooldown at tick ~101, when the blind gun is
    // still crawling the southern detour far outside its own vision — it
    // must hold fire through this whole window.
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![spotter],
            goal: TilePos::new(2, 6),
            queue: false,
        },
    )]);
    let mut blind_hits = 0;
    for _ in 0..119 {
        let report = state.tick(&[]);
        blind_hits += report
            .events
            .iter()
            .filter(|e| matches!(e, Event::AttackHit { attacker, .. } if *attacker == bombard))
            .count();
    }
    assert_eq!(blind_hits, 0, "no eyes, no shells");
    assert_eq!(state.unit(victim).unwrap().hp, hp_after_first);

    // The gun is not broken, just blind: once its crawl brings the
    // victim inside its own five tiles of sight, the shelling resumes.
    run_until(&mut state, 400, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::AttackHit { attacker, .. } if *attacker == bombard))
    });
}

#[test]
fn attack_orders_on_uncoverable_targets_walk_instead() {
    // A Flakhound ordered onto a ground unit has no gun for the job: it
    // walks to the area like a pacifist would, instead of pretending.
    let mut state = arena(vec![
        unit(0, UnitKind::Flakhound, 4, 2),
        unit(0, UnitKind::Scuttler, 11, 2),
        unit(1, UnitKind::Harvester, 12, 2),
    ])
    .build()
    .unwrap();
    let (flak, harv) = (state.units()[0].id, state.units()[2].id);
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![flak],
            target: Target::Unit(harv),
            queue: false,
        },
    )]);
    assert!(
        matches!(state.unit(flak).unwrap().order, Order::Move { .. }),
        "no covering weapon lowers the attack to a walk"
    );
}

#[test]
fn hovering_machines_do_not_block_foundations() {
    let mut state = arena(vec![
        unit(0, UnitKind::Harvester, 4, 5),
        unit(0, UnitKind::Darter, 5, 5),
    ])
    .build()
    .unwrap();
    let builder = state.units()[0].id;
    assert_eq!(UnitKind::Darter.stats().domain, Domain::Air);
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Turret,
            anchor: TilePos::new(5, 5),
        },
    )]);
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::Turret && b.anchor == TilePos::new(5, 5)),
        "a flyer overhead is not a foundation problem"
    );
}

#[test]
fn training_is_gated_to_the_seats_faction() {
    let mut scenario = arena(vec![unit(0, UnitKind::Harvester, 4, 2)]);
    scenario.players[0].scrap = 600; // fabricator + one of each test kind
    let mut state = scenario.build().unwrap();
    let builder = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Fabricator,
            anchor: TilePos::new(5, 1),
        },
    )]);
    run_until(&mut state, 500, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    });
    let fab = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Fabricator)
        .unwrap()
        .id;

    // Ferrous may not train the Cupric anti-air variant...
    let report = state.tick(&[cmd(
        0,
        Command::Train {
            building: fab,
            kind: UnitKind::Stinger,
        },
    )]);
    assert!(
        report.events.iter().any(|e| matches!(
            e,
            Event::CommandRejected {
                reason: oxide_sim::command::RejectReason::WrongFaction,
                ..
            }
        )),
        "cross-faction training must bounce"
    );

    // ...but its own variant and the shared Bombard queue fine.
    for kind in [UnitKind::Flakhound, UnitKind::Bombard] {
        let report = state.tick(&[cmd(
            0,
            Command::Train {
                building: fab,
                kind,
            },
        )]);
        assert!(
            !report
                .events
                .iter()
                .any(|e| matches!(e, Event::CommandRejected { .. })),
            "{kind:?} belongs to (or is shared with) Ferrous"
        );
    }
}

#[test]
fn the_sidearm_fights_its_own_war_alongside_the_main_gun() {
    // A sentinel slugging it out on the ground keeps its skyward poke
    // busy against a hovering darter — two weapons, two wars, two
    // independent cooldowns.
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 4, 2),
        unit(1, UnitKind::Sentinel, 6, 2),
        unit(1, UnitKind::Darter, 5, 2),
    ])
    .build()
    .unwrap();
    let (mine, foe, darter) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![mine],
            target: Target::Unit(foe),
            queue: false,
        },
    )]);
    let (mut ground_hits, mut air_hits) = (0, 0);
    for _ in 0..100 {
        let report = state.tick(&[]);
        for e in &report.events {
            if let Event::AttackHit {
                attacker, target, ..
            } = e
                && *attacker == mine
            {
                match target {
                    Target::Unit(uid) if *uid == foe => ground_hits += 1,
                    Target::Unit(uid) if *uid == darter => air_hits += 1,
                    _ => {}
                }
            }
        }
        if ground_hits >= 2 && air_hits >= 2 {
            break;
        }
    }
    assert!(
        ground_hits >= 2 && air_hits >= 2,
        "both weapons must cycle (ground {ground_hits}, air {air_hits})"
    );
}
