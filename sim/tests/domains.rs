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
        ],
        units,
        buildings: Vec::new(),
        meta: None,
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
        // Beyond every scuttler's aggro (5) so the cluster stands its
        // ground through the flight; the Foundry's vision does the
        // spotting.
        unit(0, UnitKind::Bombard, 2, 6),
        // One tile further out than looks necessary: at (7,2) the left
        // scuttler sits 4.5 from the p0 Foundry's wall and marches on
        // it — buildings are ground targets — right out of the blast.
        unit(1, UnitKind::Scuttler, 9, 2),
        unit(1, UnitKind::Scuttler, 8, 2),
        unit(1, UnitKind::Scuttler, 9, 3),
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
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::ShellLaunched { .. })),
        "the shell leaves the gun at once"
    );
    // Flight is real now: the cluster stands (idle scuttlers hold
    // their ground) until the shell arrives.
    let mut died = 0;
    for _ in 0..40 {
        died += state
            .tick(&[])
            .events
            .iter()
            .filter(|e| matches!(e, Event::UnitDied { .. }))
            .count();
    }
    assert_eq!(died, 3, "one shell, three wrecks — that is what splash is");
}

#[test]
fn splash_victims_all_turn_on_the_shooter() {
    let mut state = arena(vec![
        // In the (aggro, vision] window: the sentinels stand through
        // the flight yet see the gun, so their answer is fog-legal.
        unit(0, UnitKind::Bombard, 3, 3),
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
    run_until(&mut state, 30, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::ShellLanded { .. }))
    });
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
            .any(|e| matches!(e, Event::ShellLaunched { .. })),
        "indirect fire ignores the rock between"
    );
    let hp_before = state.unit(victim).unwrap().hp;
    run_until(&mut state, 30, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::ShellLanded { .. }))
    });
    assert!(
        state.unit(victim).is_none_or(|u| u.hp < hp_before),
        "the arc pays off on arrival"
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
        buildings: Vec::new(),
        meta: None,
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
            .any(|e| matches!(e, Event::ShellLaunched { .. })),
        "with a spotter, the shell flies beyond the gun's own sight"
    );
    run_until(&mut state, 40, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::ShellLanded { .. }))
    });
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
    let mut blind_launches = 0;
    for _ in 0..119 {
        let report = state.tick(&[]);
        blind_launches += report
            .events
            .iter()
            .filter(|e| matches!(e, Event::ShellLaunched { .. }))
            .count();
    }
    assert_eq!(blind_launches, 0, "no eyes, no shells");
    assert_eq!(state.unit(victim).unwrap().hp, hp_after_first);

    // The gun is not broken, just blind: once its crawl brings the
    // victim inside its own five tiles of sight, the shelling resumes.
    run_until(&mut state, 400, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::ShellLaunched { .. }))
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
            queue: false,
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
            queue: false,
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
    // A sentinel working a ground target keeps its skyward poke busy
    // against a hovering darter — two weapons, two wars, two independent
    // cooldowns. The ground target is a harvester: at 60 hp a sentinel
    // duel plus darter fire ends before both cooldowns can prove a
    // second cycle, and this test is about the weapons matrix, not
    // survivability.
    let mut state = arena(vec![
        unit(0, UnitKind::Sentinel, 4, 2),
        unit(1, UnitKind::Harvester, 6, 2),
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

#[test]
fn radar_blips_detect_without_identifying_or_authorizing() {
    // Array mast at (4,2): true sight to 9, detection to 16. The enemy
    // harvester at (12,7) sits outside every friendly eye but inside the
    // ring — a blip. Blips are tiles: no kind, no owner, and no license
    // to shoot.
    let mut scenario = arena(vec![
        unit(0, UnitKind::Harvester, 4, 1),
        unit(0, UnitKind::Sentinel, 5, 1),
        unit(1, UnitKind::Harvester, 12, 7),
    ]);
    scenario.players[0].scrap = 200;
    let mut state = scenario.build().unwrap();
    let (builder, fighter, intruder) = (
        state.units()[0].id,
        state.units()[1].id,
        state.units()[2].id,
    );
    state.tick(&[cmd(
        0,
        Command::Build {
            units: vec![builder],
            kind: BuildingKind::Array,
            anchor: TilePos::new(4, 2),
            queue: false,
        },
    )]);
    run_until(&mut state, 500, |_, events| {
        events
            .iter()
            .any(|e| matches!(e, Event::BuildingCompleted { .. }))
    });
    state.tick(&[]);
    let intruder_tile = state.unit(intruder).unwrap().tile();
    assert!(
        state
            .vision(PlayerId(0))
            .contacts()
            .contains(&intruder_tile),
        "the ring detects what sight cannot reach"
    );
    assert!(
        !state.vision(PlayerId(0)).visible(intruder_tile),
        "test premise: the contact is genuinely out of sight"
    );

    // The fog-honest observation carries the blip and nothing more about
    // the contact — the unit itself is absent.
    let obs = oxide_sim::bot::Observation::fog_honest(&state, PlayerId(0));
    assert!(obs.blips.contains(&intruder_tile));
    assert!(
        obs.enemy_units.is_empty(),
        "detection is not identification"
    );

    // And it buys no shot: targeted attacks still need true sight.
    let report = state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![fighter],
            target: Target::Unit(intruder),
            queue: false,
        },
    )]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "a blip is not a target"
    );

    // Walking into true sight converts the blip into a sighting.
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![intruder],
            goal: TilePos::new(6, 2),
            queue: false,
        },
    )]);
    run_until(&mut state, 400, |s, _| {
        let t = s.unit(intruder).unwrap().tile();
        s.vision(PlayerId(0)).visible(t)
    });
    assert!(
        state.vision(PlayerId(0)).contacts().is_empty(),
        "seen contacts are sightings, not blips"
    );
}

#[test]
fn splash_hits_the_unseen_but_reveals_nothing() {
    // The fire gate governs choosing a victim; a shell in flight chooses
    // nothing. An unseen bystander in the blast takes damage silently:
    // no event names it, and it never chases a shooter it cannot see.
    // The spotter's disc (vision 6) grazes the victim at distance
    // sqrt(32) but misses the bystander one tile deeper at sqrt(41);
    // the gun itself (vision 5, range 9.5) sits exactly 9 tiles out.
    let mut scenario = arena(vec![
        unit(0, UnitKind::Bombard, 4, 5),
        unit(0, UnitKind::Harvester, 9, 1),
        unit(1, UnitKind::Harvester, 13, 5),
        unit(1, UnitKind::Sentinel, 13, 6),
    ]);
    scenario.map = vec![
        "################".into(),
        "#1......#....2.#".into(),
        "#.......#......#".into(),
        "#.......#......#".into(),
        "#.......#......#".into(),
        "#.......#......#".into(),
        "#..............#".into(),
        "#..............#".into(),
        "################".into(),
    ];
    let mut state = scenario.build().unwrap();
    let (bombard, victim, bystander) = (
        state.units()[0].id,
        state.units()[2].id,
        state.units()[3].id,
    );
    // Premise: the aimed victim is seen (spotter), the bystander is not.
    assert!(state.can_see(PlayerId(0), state.unit(victim).unwrap().tile()));
    assert!(!state.can_see(PlayerId(0), state.unit(bystander).unwrap().tile()));
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![bombard],
            target: Target::Unit(victim),
            queue: false,
        },
    )]);
    let mut named_bystander = false;
    let mut landed = false;
    for _ in 0..40 {
        let report = state.tick(&[]);
        landed |= report
            .events
            .iter()
            .any(|e| matches!(e, Event::ShellLanded { .. }));
        named_bystander |= report.events.iter().any(
            |e| matches!(e, Event::AttackHit { target: Target::Unit(u), .. } if *u == bystander),
        );
        if landed {
            break;
        }
    }
    assert!(landed, "the shell arrived");
    assert!(
        state.unit(bystander).unwrap().hp < UnitKind::Sentinel.stats().max_hp,
        "the blast does not check papers"
    );
    assert!(!named_bystander, "no event names the unseen bystander");
    state.tick(&[]);
    assert_eq!(
        state.unit(bystander).unwrap().order,
        Order::Idle,
        "it cannot chase a shooter it never saw"
    );
}

#[test]
fn ground_anti_air_reaches_a_flyer_parked_over_rock() {
    // The wisp hovers over the rock block; the flakhound cannot stand
    // there — it must take the nearest standable tile and shoot from
    // range instead of stalling forever while the flyer sits immune.
    // Geometry matters: the flakhound sits in sight of the rock
    // (vision 7) but out of range and aggro (5), and the wisp's whole
    // descent stays outside aggro too — so no mid-flight auto-acquire
    // drags the chaser into range before the wisp parks. The kill then
    // requires an approach toward a tile no ground unit can stand on.
    let mut state = arena(vec![
        unit(0, UnitKind::Flakhound, 13, 2),
        unit(1, UnitKind::Wisp, 7, 1),
    ])
    .build()
    .unwrap();
    let (flak, wisp) = (state.units()[0].id, state.units()[1].id);
    // Park the wisp on the rock at (6,3)-(7,4).
    state.tick(&[cmd(
        1,
        Command::Move {
            units: vec![wisp],
            goal: TilePos::new(7, 4),
            queue: false,
        },
    )]);
    run_until(&mut state, 200, |s, _| {
        s.unit(wisp).unwrap().tile() == TilePos::new(7, 4)
    });
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

#[test]
fn air_units_may_start_on_ground_no_walker_could() {
    // Spawn validation runs in the unit's own movement domain: a flyer may
    // open the match hovering over rock, exactly where play could take it
    // one tick later; walkers still need open ground.
    let state = arena(vec![unit(1, UnitKind::Wisp, 7, 4)]).build().unwrap();
    assert_eq!(state.units()[0].tile(), TilePos::new(7, 4));
    assert!(
        arena(vec![unit(0, UnitKind::Scuttler, 7, 4)])
            .build()
            .is_err(),
        "rock still rejects a walker's spawn"
    );
}
