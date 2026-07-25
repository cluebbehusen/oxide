//! Peak terrain: the mountain that blocks everything. Ground can't walk
//! it, air can't fly it, direct fire can't cross it in any domain
//! pairing, and artillery arcs break on the ridge. Vision deliberately
//! ignores it — cover is a firing rule, not a stealth system.

use chassis::grid::TilePos;
use oxide_sim::map::Terrain;
use oxide_sim::scenario::{PlayerSpec, UnitSpec};
use oxide_sim::stats::Domain;
use oxide_sim::{
    Command, Event, Faction, PlayerCommand, PlayerId, Scenario, State, Target, UnitKind,
};

fn players() -> Vec<PlayerSpec> {
    vec![
        PlayerSpec {
            name: "Ferrous".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap: 300,
            bot: false,
            bot_config: None,
        },
        PlayerSpec {
            name: "Cupric".into(),
            faction: Faction::Cupric,
            team: None,
            scrap: 300,
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

/// A ridge wall at x=12. `gap` opens one corridor at y=2; without it the
/// two halves share a map and nothing else. Both foundries sit west —
/// scenario validation requires the seats ground-connected, so the east
/// side hosts only orphaned units.
fn ridge(gap: bool, units: Vec<UnitSpec>) -> Scenario {
    // The border is rock — which air crosses freely — so the ridge
    // claims its border cells too or the sky routes around the ends.
    let wall_row = |row: &str| row.to_string();
    let mut map = vec![wall_row("############^###########")];
    for y in 1..11 {
        let mut row = String::from("#");
        for x in 1..23 {
            row.push(match (x, y) {
                (1, 1) => '1',
                (1, 9) => '2',
                (12, 2) if gap => '.',
                (12, _) => '^',
                _ => '.',
            });
        }
        row.push('#');
        map.push(row);
    }
    map.push(wall_row("############^###########"));
    Scenario {
        name: "ridge".into(),
        seed: 9,
        map,
        players: players(),
        units,
        buildings: Vec::new(),
        meta: None,
    }
}

fn run(state: &mut State, ticks: u64) -> Vec<Event> {
    let mut all = Vec::new();
    for _ in 0..ticks {
        all.extend(state.tick(&[]).events);
    }
    all
}

#[test]
fn the_caret_parses_renders_and_refuses_both_domains() {
    let state = ridge(false, vec![]).build().unwrap();
    let peak = TilePos::new(12, 5);
    assert_eq!(state.map().tile(peak).unwrap().terrain, Terrain::Peak);
    assert!(!state.passable(peak), "no ground stands on a mountain");
    assert!(
        !state.passable_for(Domain::Air, peak),
        "a mountain owns its column of sky"
    );
    let rows = state.map().ascii_rows();
    assert_eq!(
        rows[5].chars().nth(12),
        Some('^'),
        "the glyph round-trips through ascii_rows"
    );
}

#[test]
fn no_route_crosses_a_full_ridge() {
    let mut state = ridge(
        false,
        vec![
            unit(0, UnitKind::Scuttler, 8, 5),
            unit(0, UnitKind::Wisp, 9, 7),
        ],
    )
    .build()
    .unwrap();
    let (walker, flyer) = (state.units()[0].id, state.units()[1].id);
    state.tick(&[
        cmd(
            0,
            Command::Move {
                units: vec![walker],
                goal: TilePos::new(18, 5),
                queue: false,
            },
        ),
        cmd(
            0,
            Command::Move {
                units: vec![flyer],
                goal: TilePos::new(18, 7),
                queue: false,
            },
        ),
    ]);
    run(&mut state, 400);
    assert!(
        state.unit(walker).unwrap().tile().x < 12,
        "the ground half stays ground-half"
    );
    assert!(
        state.unit(flyer).unwrap().tile().x < 12,
        "the sky is walled too"
    );
}

#[test]
fn air_reroutes_through_the_gap() {
    let mut state = ridge(true, vec![unit(0, UnitKind::Wisp, 9, 8)])
        .build()
        .unwrap();
    let flyer = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![flyer],
            goal: TilePos::new(18, 8),
            queue: false,
        },
    )]);
    for _ in 0..400 {
        state.tick(&[]);
        let tile = state.unit(flyer).unwrap().tile();
        assert_ne!(
            state.map().tile(tile).unwrap().terrain,
            Terrain::Peak,
            "the detour never overflies the ridge"
        );
    }
    assert_eq!(
        state.unit(flyer).unwrap().tile(),
        TilePos::new(18, 8),
        "the flyer detoured through the gap and landed the goal"
    );
}

#[test]
fn a_peak_goal_snaps_to_open_sky() {
    let mut state = ridge(true, vec![unit(0, UnitKind::Wisp, 9, 5)])
        .build()
        .unwrap();
    let flyer = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![flyer],
            goal: TilePos::new(12, 5),
            queue: false,
        },
    )]);
    run(&mut state, 200);
    let rest = state.unit(flyer).unwrap().tile();
    assert_ne!(
        state.map().tile(rest).unwrap().terrain,
        Terrain::Peak,
        "the goal snapped off the mountain"
    );
    let (dx, dy) = (rest.x - 12, rest.y - 5);
    assert!(dx * dx + dy * dy <= 2, "but stayed close");
}

#[test]
fn ground_guns_do_not_shoot_through_mountains() {
    let mut state = ridge(
        false,
        vec![
            unit(0, UnitKind::Sentinel, 10, 5),
            unit(1, UnitKind::Scuttler, 14, 5),
        ],
    )
    .build()
    .unwrap();
    let (gun, victim) = (state.units()[0].id, state.units()[1].id);
    let hp = state.unit(victim).unwrap().hp;
    let mut events = state
        .tick(&[cmd(
            0,
            Command::Attack {
                units: vec![gun],
                target: Target::Unit(victim),
                queue: false,
            },
        )])
        .events;
    events.extend(run(&mut state, 200));
    assert!(
        !events.iter().any(|e| matches!(e, Event::AttackHit { .. })),
        "nobody landed a hit across the ridge"
    );
    assert_eq!(state.unit(victim).unwrap().hp, hp);
}

#[test]
fn artillery_arcs_break_on_the_ridge() {
    let mut state = ridge(
        false,
        vec![
            // At x=7 the west foundry's footprint sits outside the
            // bombard's aggro ring; at x=6 a stalled gun auto-acquires
            // it (closest-point distance 4.95) and the test measures
            // the wrong war.
            unit(0, UnitKind::Bombard, 7, 5),
            // Spotter: sees the victim (vision 7 at range 4) so the
            // attack command validates — sight is radius-based and
            // crosses peaks on purpose.
            unit(0, UnitKind::Sentinel, 10, 5),
            unit(1, UnitKind::Scuttler, 14, 5),
        ],
    )
    .build()
    .unwrap();
    let (bombard, victim) = (state.units()[0].id, state.units()[2].id);
    let hp = state.unit(victim).unwrap().hp;
    let mut events = state
        .tick(&[cmd(
            0,
            Command::Attack {
                units: vec![bombard],
                target: Target::Unit(victim),
                queue: false,
            },
        )])
        .events;
    events.extend(run(&mut state, 300));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::ShellLaunched { .. })),
        "no shell arcs over a mountain"
    );
    assert_eq!(state.unit(victim).unwrap().hp, hp);
}

#[test]
fn flak_cannot_burst_through_stone() {
    // Air-involved direct fire traces peaks even though rock would let
    // it pass: the flakhound sees the wisp across the ridge, sits in
    // range, and still holds fire.
    let mut state = ridge(
        false,
        vec![
            unit(0, UnitKind::Flakhound, 10, 5),
            unit(1, UnitKind::Wisp, 14, 5),
        ],
    )
    .build()
    .unwrap();
    let (flak, wisp) = (state.units()[0].id, state.units()[1].id);
    let hp = state.unit(wisp).unwrap().hp;
    let mut events = state
        .tick(&[cmd(
            0,
            Command::Attack {
                units: vec![flak],
                target: Target::Unit(wisp),
                queue: false,
            },
        )])
        .events;
    events.extend(run(&mut state, 200));
    assert!(
        !events.iter().any(|e| matches!(e, Event::AttackHit { .. })),
        "the burst never crossed the ridge"
    );
    assert_eq!(state.unit(wisp).unwrap().hp, hp);
}

#[test]
fn a_ridge_match_stays_bit_identical() {
    let build = || {
        let mut state = ridge(
            true,
            vec![
                unit(0, UnitKind::Scuttler, 8, 3),
                unit(0, UnitKind::Wisp, 9, 8),
                unit(1, UnitKind::Scuttler, 16, 3),
                unit(1, UnitKind::Darter, 15, 8),
            ],
        )
        .build()
        .unwrap();
        let (w, d) = (state.units()[1].id, state.units()[3].id);
        state.tick(&[
            cmd(
                0,
                Command::Move {
                    units: vec![w],
                    goal: TilePos::new(18, 8),
                    queue: false,
                },
            ),
            cmd(
                1,
                Command::Move {
                    units: vec![d],
                    goal: TilePos::new(5, 8),
                    queue: false,
                },
            ),
        ]);
        for _ in 0..300 {
            state.tick(&[]);
        }
        state.hash()
    };
    assert_eq!(build(), build(), "same ridge, same bits");
}

#[test]
fn a_patrol_leg_on_the_ridge_snaps_to_open_sky() {
    // Patrol waypoints skip the group-order goal snap, and line_blocked
    // ignores endpoints by design — the route funnel itself must refuse
    // to hand a flyer the mountain.
    let mut state = ridge(true, vec![unit(0, UnitKind::Wisp, 9, 5)])
        .build()
        .unwrap();
    let flyer = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![flyer],
            waypoints: vec![TilePos::new(12, 5), TilePos::new(9, 5)],
        },
    )]);
    for _ in 0..300 {
        state.tick(&[]);
        let tile = state.unit(flyer).unwrap().tile();
        assert_ne!(
            state.map().tile(tile).unwrap().terrain,
            Terrain::Peak,
            "the patrol never parks on the ridge"
        );
    }
}

#[test]
fn a_building_flush_against_the_ridge_is_safe_from_the_far_side() {
    // The aim point for a building is its closest footprint point — an
    // exact edge coordinate that floors into the NEIGHBORING tile. With
    // the footprint flush against the ridge's west face and the gun to
    // the east, that neighbor IS the peak — and the line trace skips
    // endpoint tiles by design, so only an explicit endpoint check
    // refuses the shot (tile 13 between gun and edge is open ground).
    let mut map = vec!["############^###########".to_string()];
    for y in 1..11 {
        let mut row = String::from("#");
        for x in 1..23 {
            row.push(match (x, y) {
                (1, 1) => '1',
                (10, 5) => '2', // footprint (10,5)-(11,6), flush at x=12
                (12, 2) => '.', // ground pass keeps the seats connected
                (12, _) => '^',
                _ => '.',
            });
        }
        row.push('#');
        map.push(row);
    }
    map.push("############^###########".to_string());
    let mut state = Scenario {
        name: "flush".into(),
        seed: 9,
        map,
        players: players(),
        units: vec![unit(0, UnitKind::Lancer, 14, 5)],
        buildings: Vec::new(),
        meta: None,
    }
    .build()
    .unwrap();
    let lancer = state.units()[0].id;
    let west = state
        .buildings()
        .iter()
        .find(|b| b.player == PlayerId(1))
        .unwrap()
        .id;
    let hp = state.building(west).unwrap().hp;
    state.tick(&[cmd(
        0,
        Command::Attack {
            units: vec![lancer],
            target: Target::Building(west),
            queue: false,
        },
    )]);
    // The lancer may legally chase through the northern pass; only
    // shots fired while it still stands east of the ridge would be
    // through-the-mountain shots. Watch exactly that window.
    for _ in 0..400 {
        let east_side = state.unit(lancer).is_some_and(|u| u.tile().x > 12);
        if !east_side {
            break;
        }
        let events = state.tick(&[]).events;
        assert!(
            !events.iter().any(|e| matches!(e, Event::AttackHit { .. })),
            "no shot crosses the ridge from the east side"
        );
        assert_eq!(state.building(west).unwrap().hp, hp);
    }
}

#[test]
fn an_air_patrol_through_the_ridge_keeps_flying_its_legs() {
    // A peak waypoint stored raw once deadlocked the flyer: it reached
    // the route's snapped endpoint, compared against the original peak
    // goal, and repathed to the same tile forever. Lowering must store
    // the snapped goal, so the patrol rotates through both legs.
    let mut state = ridge(true, vec![unit(0, UnitKind::Wisp, 6, 5)])
        .build()
        .unwrap();
    let flyer = state.units()[0].id;
    state.tick(&[cmd(
        0,
        Command::Patrol {
            units: vec![flyer],
            waypoints: vec![TilePos::new(12, 5), TilePos::new(6, 5)],
        },
    )]);
    let mut near_wall = false;
    let mut home_again = false;
    for _ in 0..900 {
        state.tick(&[]);
        let tile = state.unit(flyer).unwrap().tile();
        if (tile.x - 12).abs() <= 1 && (tile.y - 5).abs() <= 1 {
            near_wall = true;
        }
        if near_wall && tile == TilePos::new(6, 5) {
            home_again = true;
            break;
        }
    }
    assert!(near_wall, "the first leg reached the wall's snapped tile");
    assert!(
        home_again,
        "the patrol rotated to its second leg instead of deadlocking"
    );
}
