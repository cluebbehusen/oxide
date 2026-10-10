//! Golden-image regression tests.
//!
//! The CPU renderer in `oxide-kit` is bit-deterministic, so these compare
//! PNG bytes exactly, with no tolerance. When an intentional
//! sim or renderer change moves the pixels:
//!
//! 1. `BLESS=1 cargo test -p oxide-driver` to regenerate,
//! 2. *look at* the regenerated PNGs in `driver/tests/goldens/`,
//! 3. commit them together with the change and say why.

use chassis::grid::TilePos;
use oxide_kit::render;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, ScenarioMode, UnitSpec};
use oxide_sim::{
    BuildingKind, Command, PlayerCommand, PlayerId, Scenario, State, Target, UnitId, UnitKind,
};
use std::path::PathBuf;

fn golden_check(name: &str, state: &oxide_sim::State) {
    let actual = render::png_bytes(state).unwrap();
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens")
        .join(format!("{name}.png"));

    if std::env::var_os("BLESS").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &actual).unwrap();
        eprintln!("blessed {}", golden.display());
        return;
    }
    let expected = std::fs::read(&golden).unwrap_or_else(|_| {
        panic!(
            "missing golden {} — run `BLESS=1 cargo test -p oxide-driver` and commit it",
            golden.display()
        )
    });
    if expected != actual {
        let actual_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target")
            .join(format!("golden-actual-{name}.png"));
        std::fs::create_dir_all(actual_path.parent().unwrap()).unwrap();
        std::fs::write(&actual_path, &actual).unwrap();
        panic!(
            "golden mismatch for {name}: inspect {} vs {}, re-bless if the change is intended",
            golden.display(),
            actual_path.display()
        );
    }
}

#[test]
fn skirmish_opening_matches_golden() {
    let state = Scenario::skirmish().build().unwrap();
    golden_check("skirmish-t0", &state);
}

// ---------------------------------------------------------------------------
// The showcase: one state holding everything the CPU renderer draws.
// ---------------------------------------------------------------------------
//
// The skirmish goldens only ever exercise ground, rock, full nodes and
// healthy machines. This scenario is built in test code, never under
// `scenarios/`, which ships to players and is read by the hash fixtures,
// the integrity tests and the map gates. It is driven through a scripted
// program until the final state carries every branch of
// `kit/src/render.rs`: every terrain, rubble, scrap full and rich and
// worked down past half, a wreck tile, standing and half-built structures
// of every kind, damaged machines of every kind, a laden harvester, and
// three hostile seats in distinct colours.
//
// `showcase_covers_every_rendered_feature` is the readable half of the
// contract: it names each of those, so a script that quietly stops
// producing one fails with a sentence rather than a pixel diff.

/// The showcase playfield. Anchor `1` is seat 0's north-west base beside
/// the worked node, `2` seat 1's south-east construction yard, `3` seat
/// 2's south-west corner, a third seat hostile to both.
const SHOWCASE_MAP: [&str; 30] = [
    "################################################",
    "#..............................................#",
    "#.1............................................#",
    "#..............................................#",
    "#....s.........................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..,,,,,..####..^^^^^..........................#",
    "#..,,,,,..####..^^^^^...s.S....................#",
    "#..,,,,,..####..^^^^^..........................#",
    "#..............................................#",
    "#................................~~~~~.........#",
    "#................................~~~~~.........#",
    "#................................~~~~~.........#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#..............................................#",
    "#.3........................................2...#",
    "#..............................................#",
    "#..............................................#",
    "################################################",
];

/// The node the harvest crew works down past half.
const WORKED_NODE: TilePos = TilePos { x: 5, y: 4 };

/// Ticks of scripted fighting before the survivors disengage. Long
/// enough for the ordinary roster's first volley; the Avalanches wait
/// separately for their slower turn and projectile travel.
const FIGHT_TICKS: u64 = 24;

/// Total ticks. Sized so the worked node lands in the renderer's
/// depleted tint without mining out.
const SHOWCASE_TICKS: u64 = 1200;

/// The crew steps off the node before the picture is taken — eight
/// harvesters ringing a tile would hide the very thing they mined.
const CREW_STEPS_OFF: u64 = SHOWCASE_TICKS - 30;

/// Collects unit specs while handing back the id each one will get:
/// `Scenario::build` spawns them in list order, so the index *is* the id.
#[derive(Default)]
struct Roster {
    units: Vec<UnitSpec>,
}

impl Roster {
    fn add(&mut self, player: u8, kind: UnitKind, x: i32, y: i32) -> UnitId {
        let id = UnitId(u32::try_from(self.units.len()).unwrap());
        self.units.push(UnitSpec { player, kind, x, y });
        id
    }
}

fn seat(name: &str, scrap: u32) -> PlayerSpec {
    PlayerSpec {
        name: name.into(),
        team: None,
        scrap,
        bot: false,
        bot_config: None,
    }
}

/// Every unit the showcase places, named so the command script reads.
struct Cast {
    /// Seat 0's harvest crew.
    crew: Vec<UnitId>,
    /// Seat 1's site-founding harvester.
    builder: UnitId,
    /// Seat 0's battle line, west to east.
    west: Vec<UnitId>,
    /// Seat 1's battle line, west to east.
    east: Vec<UnitId>,
    /// Seat 2's lone machine, hostile to both other seats.
    interloper: UnitId,
    /// The two Bombards, shelling each other across open ground.
    guns: (UnitId, UnitId),
    /// Seat 0's tier-three annex: Condor, Flakhound wounder, Breaker.
    annex_w: Vec<UnitId>,
    /// Seat 1's tier-three annex: Condor, Flakhound wounder, Breaker.
    annex_e: Vec<UnitId>,
    /// The Avalanche pair, trading one volley across their blind rings.
    avalanches: (UnitId, UnitId),
}

fn showcase_scenario() -> (Scenario, Cast) {
    let mut roster = Roster::default();

    // Seat 0's harvest crew rings the worked node.
    let crew = [
        (4, 3),
        (5, 3),
        (6, 3),
        (4, 4),
        (6, 4),
        (4, 5),
        (5, 5),
        (6, 5),
    ]
    .into_iter()
    .map(|(x, y)| roster.add(0, UnitKind::Harvester, x, y))
    .collect();

    // Seat 1's builder stands in the middle of its construction yard.
    let builder = roster.add(1, UnitKind::Harvester, 36, 25);

    // The battle line: seat 0 north, seat 1 south, two tiles apart. Both
    // seats field the same slot order, so opposite slots always agree on
    // movement domain and weapon coverage — slot 4 is the anti-air, slot 5
    // the ground-attack flyer, slot 6 the air-superiority flyer.
    let line = [
        UnitKind::Harvester,
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Sentinel,
        UnitKind::Flakhound,
        UnitKind::Buzzard,
        UnitKind::Talon,
        UnitKind::Lancer,
        UnitKind::Sentinel,
        UnitKind::Sentinel,
        UnitKind::Sentinel,
        // Interleaved so every victim's shooter stands one column
        // over (2.24 tiles, inside every range).
        UnitKind::Warden,
        UnitKind::Tender,
        UnitKind::Sentinel,
        UnitKind::Excavator,
        UnitKind::Sentinel,
        UnitKind::Kestrel,
        UnitKind::Shrike,
        UnitKind::Flakhound,
    ];
    let west: Vec<UnitId> = line
        .into_iter()
        .enumerate()
        .map(|(i, kind)| roster.add(0, kind, 8 + i32::try_from(i).unwrap(), 20))
        .collect();
    let east: Vec<UnitId> = line
        .into_iter()
        .enumerate()
        .map(|(i, kind)| roster.add(1, kind, 8 + i32::try_from(i).unwrap(), 22))
        .collect();
    let interloper = roster.add(2, UnitKind::Sentinel, 20, 20);

    // Artillery: far enough off the line that the splash reaches only
    // the other gun, close enough that each is its own spotter.
    let gun_west = roster.add(0, UnitKind::Bombard, 29, 20);
    let gun_east = roster.add(1, UnitKind::Bombard, 29, 22);

    // The annex stays northeast of the pit, clear of the march lanes.
    // Avalanches wait separately for their turn and missile exchange.
    // Bombers are victims here, not shooters — a released bomb's 2.2
    // splash would rewrite the carefully bounded wounds around it.
    let condor_w = roster.add(0, UnitKind::Condor, 39, 12);
    let flakhound_annex_e = roster.add(1, UnitKind::Flakhound, 40, 12);
    let flakhound_annex_w = roster.add(0, UnitKind::Flakhound, 43, 12);
    let condor_e = roster.add(1, UnitKind::Condor, 44, 12);
    let breaker_w = roster.add(0, UnitKind::Breaker, 40, 9);
    let breaker_e = roster.add(1, UnitKind::Breaker, 43, 9);
    // Five tiles apart: outside both blind rings, inside both reaches,
    // spotted for each seat by its annex flak sitting four tiles off.
    let avalanche_w = roster.add(0, UnitKind::Avalanche, 39, 16);
    let avalanche_e = roster.add(1, UnitKind::Avalanche, 44, 16);
    // The unarmed slings, each with its own flak wounder, two tiles
    // below the Avalanche exchange (outside its 1.6 splash).
    let skyhook_w = roster.add(0, UnitKind::Skyhook, 39, 18);
    let flakhound_sling_e = roster.add(1, UnitKind::Flakhound, 40, 18);
    let flakhound_sling_w = roster.add(0, UnitKind::Flakhound, 43, 18);
    let skyhook_e = roster.add(1, UnitKind::Skyhook, 44, 18);
    // The sapper pair, each nicked by a line sentinel; sappers have no
    // aggro of their own and stand their wounds passively.
    let sapper_w = roster.add(0, UnitKind::Sapper, 40, 20);
    let sentinel_sapper_e = roster.add(1, UnitKind::Sentinel, 41, 20);
    let sentinel_sapper_w = roster.add(0, UnitKind::Sentinel, 42, 20);
    let sapper_e = roster.add(1, UnitKind::Sapper, 43, 20);

    // Seat 0's standing structures: one of every kind the build palette
    // offers, whole. Its Foundry comes from the map anchor.
    let buildings = vec![
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Turret,
            x: 8,
            y: 2,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::FlakTurret,
            x: 10,
            y: 2,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Array,
            x: 12,
            y: 2,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Reclaimer,
            x: 14,
            y: 2,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Fabricator,
            x: 16,
            y: 2,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Bastion,
            x: 19,
            y: 2,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::RepairBay,
            x: 22,
            y: 2,
        },
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Barricade,
            x: 25,
            y: 2,
        },
        // The owner sees its own buried charge; the omniscient CPU
        // renderer draws it regardless.
        BuildingSpec {
            player: 0,
            kind: BuildingKind::ScuttleCharge,
            x: 29,
            y: 2,
        },
    ];

    let scenario = Scenario {
        mode: ScenarioMode::Match,
        name: "renderer showcase".into(),
        map: SHOWCASE_MAP.iter().map(|r| (*r).to_string()).collect(),
        players: vec![seat("West", 700), seat("East", 2000), seat("South", 100)],
        units: roster.units,
        buildings,
        meta: None,
    };
    (
        scenario,
        Cast {
            crew,
            builder,
            west,
            east,
            interloper,
            guns: (gun_west, gun_east),
            annex_w: vec![
                condor_w,
                flakhound_annex_w,
                breaker_w,
                skyhook_w,
                flakhound_sling_w,
                sapper_w,
                sentinel_sapper_w,
            ],
            annex_e: vec![
                condor_e,
                flakhound_annex_e,
                breaker_e,
                skyhook_e,
                flakhound_sling_e,
                sapper_e,
                sentinel_sapper_e,
            ],
            avalanches: (avalanche_w, avalanche_e),
        },
    )
}

fn attack(player: u8, unit: UnitId, victim: UnitId) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command: Command::Attack {
            units: vec![unit],
            target: Target::Unit(victim).into(),
            queue: false,
        },
    }
}

fn walk(player: u8, units: Vec<UnitId>, x: i32, y: i32) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command: Command::Run {
            units,
            goal: TilePos::new(x, y),
            queue: false,
        },
    }
}

/// Queue the showcase's unfinished sites on the final tick so every site
/// retains its worker commitment without completing before the picture.
const YARD_FOUNDS: u64 = SHOWCASE_TICKS - 1;

fn yard_orders(cast: &Cast) -> Vec<PlayerCommand> {
    [
        (BuildingKind::Turret, 34, 24),
        (BuildingKind::FlakTurret, 35, 23),
        (BuildingKind::Array, 37, 23),
        (BuildingKind::Reclaimer, 39, 23),
        (BuildingKind::Fabricator, 32, 25),
        (BuildingKind::Bastion, 38, 25),
        (BuildingKind::RepairBay, 41, 25),
    ]
    .into_iter()
    .map(|(kind, x, y)| PlayerCommand {
        player: PlayerId(1),
        command: Command::Build {
            units: vec![cast.builder],
            kind,
            anchor: TilePos::new(x, y),
            queue: true,
            defer: false,
        },
    })
    .collect()
}

/// The western field kit shares one worker's paid construction queue.
fn field_kit_orders(cast: &Cast) -> Vec<PlayerCommand> {
    [
        (BuildingKind::Barricade, 2, 10),
        (BuildingKind::ScuttleCharge, 6, 10),
    ]
    .into_iter()
    .map(|(kind, x, y)| PlayerCommand {
        player: PlayerId(0),
        command: Command::Build {
            units: vec![cast.crew[2]],
            kind,
            anchor: TilePos::new(x, y),
            queue: true,
            defer: false,
        },
    })
    .collect()
}

/// The opening orders: dig, found, and pair every machine off against one
/// it can actually shoot.
fn opening_orders(cast: &Cast) -> Vec<PlayerCommand> {
    let mut commands = vec![PlayerCommand {
        player: PlayerId(0),
        command: Command::Harvest {
            units: cast.crew.clone(),
            node: WORKED_NODE,
            queue: false,
        },
    }];
    let (w, e) = (&cast.west, &cast.east);
    // Every machine that must show a health bar gets a shooter one slot
    // away that covers its movement domain; the harvesters take fire and
    // answer with nothing. Slot 8 is what the opposing Lancer one-shots.
    commands.extend([
        attack(0, w[1], e[0]),
        attack(0, w[2], e[3]),
        attack(0, w[3], e[2]),
        attack(0, w[4], e[5]),
        attack(0, w[5], e[4]),
        attack(0, w[6], e[6]),
        attack(0, w[7], e[8]),
        attack(0, w[8], e[7]),
        attack(0, w[9], e[9]),
        attack(0, w[10], cast.interloper),
        attack(1, e[1], w[0]),
        attack(1, e[2], w[3]),
        attack(1, e[3], w[2]),
        attack(1, e[4], w[5]),
        attack(1, e[5], w[4]),
        attack(1, e[6], w[6]),
        attack(1, e[7], w[8]),
        attack(1, e[8], w[7]),
        attack(1, e[9], w[9]),
        attack(1, e[10], w[10]),
        attack(2, cast.interloper, w[10]),
        // The line's tail: Wardens trade, the flanking sentinels wound the
        // labor machines, the interceptors clip the scouts, and the
        // anti-air rear clips the interceptors.
        attack(0, w[11], e[11]),
        attack(1, e[11], w[11]),
        attack(0, w[13], e[12]),
        attack(1, e[13], w[12]),
        attack(0, w[15], e[14]),
        attack(1, e[15], w[14]),
        attack(0, w[17], e[16]),
        attack(1, e[17], w[16]),
        attack(0, w[18], e[17]),
        attack(1, e[18], w[17]),
        attack(0, cast.guns.0, cast.guns.1),
        attack(1, cast.guns.1, cast.guns.0),
        // The annex wounds: Breakers trade one 90-point blow, the
        // Avalanches trade one spotter-lit volley, and each seat's flak
        // clips the other's bomber.
        attack(0, cast.annex_w[2], cast.annex_e[2]),
        attack(1, cast.annex_e[2], cast.annex_w[2]),
        attack(0, cast.annex_w[1], cast.annex_e[0]),
        attack(1, cast.annex_e[1], cast.annex_w[0]),
        attack(0, cast.avalanches.0, cast.avalanches.1),
        attack(1, cast.avalanches.1, cast.avalanches.0),
        attack(0, cast.annex_w[4], cast.annex_e[3]),
        attack(1, cast.annex_e[4], cast.annex_w[3]),
        attack(0, cast.annex_w[6], cast.annex_e[5]),
        attack(1, cast.annex_e[6], cast.annex_w[5]),
    ]);
    commands
}

/// The ordinary roster retreats clear of enemy reach while Avalanches
/// finish their separate exchange.
fn disengage(cast: &Cast) -> Vec<PlayerCommand> {
    vec![
        walk(0, cast.west.clone(), 6, 14),
        walk(1, cast.east.clone(), 14, 27),
        // Clear of the west column's march lane, where idle aggro would
        // kill it.
        walk(2, vec![cast.interloper], 36, 6),
        walk(0, cast.annex_w.clone(), 32, 15),
        walk(1, cast.annex_e.clone(), 47, 27),
    ]
}

fn showcase_state() -> State {
    let (scenario, cast) = showcase_scenario();
    let mut state = scenario.build().expect("the showcase scenario is valid");
    let mut staged = serde_json::to_value(&state).unwrap();
    for command in opening_orders(&cast) {
        if let Command::Attack { units, target, .. } = command.command {
            for id in units {
                let unit = state.unit(id).unwrap();
                if unit.kind.stats().domain != oxide_sim::stats::Domain::Ground {
                    continue;
                }
                let aim = state
                    .attack_view(unit.player, target)
                    .expect("showcase target is known")
                    .aim_from(unit.pos)
                    - unit.pos;
                let heading = (0..=255u8)
                    .max_by_key(|&step| {
                        let direction = chassis::compass::dir(step);
                        (
                            direction.x * aim.x + direction.y * aim.y,
                            std::cmp::Reverse(step),
                        )
                    })
                    .unwrap();
                let row = staged["units"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|row| row["id"] == serde_json::json!(id))
                    .unwrap();
                row["heading"] = serde_json::json!(heading);
                if unit.kind.has_ground_turret() {
                    row["turret_heading"] = serde_json::json!(heading);
                }
            }
        }
    }
    state = serde_json::from_value(staged).unwrap();
    let mut avalanches_withdrew = false;
    let mut bombards_withdrew = false;
    let mut crew_withdrew = false;
    for tick in 0..SHOWCASE_TICKS {
        let mut commands = match tick {
            0 => opening_orders(&cast),
            t if t == YARD_FOUNDS => {
                let mut commands = yard_orders(&cast);
                commands.extend(field_kit_orders(&cast));
                commands.push(PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Build {
                        units: vec![cast.crew[1]],
                        kind: BuildingKind::Foundry,
                        anchor: TilePos::new(9, 5),
                        queue: false,
                        defer: false,
                    },
                });

                commands
            }
            t if t == FIGHT_TICKS => disengage(&cast),
            _ => Vec::new(),
        };
        if !crew_withdrew
            && (state.map().scrap_at(WORKED_NODE) <= oxide_sim::stats::SCRAP_NODE_AMOUNT / 3
                || tick == CREW_STEPS_OFF)
        {
            let workers = cast
                .crew
                .iter()
                .copied()
                .filter(|id| {
                    state
                        .unit(*id)
                        .is_some_and(|unit| matches!(unit.order, oxide_sim::Order::Harvest { .. }))
                })
                .collect();
            commands.push(walk(0, workers, 2, 6));
            crew_withdrew = true;
        }
        if !avalanches_withdrew
            && [cast.avalanches.0, cast.avalanches.1].iter().all(|id| {
                state
                    .unit(*id)
                    .is_some_and(|unit| unit.hp < unit.kind.stats().max_hp)
            })
        {
            commands.extend([
                walk(0, vec![cast.avalanches.0], 10, 10),
                walk(1, vec![cast.avalanches.1], 46, 27),
            ]);
            avalanches_withdrew = true;
        }
        if !bombards_withdrew
            && [cast.guns.0, cast.guns.1].iter().all(|id| {
                state
                    .unit(*id)
                    .is_some_and(|unit| unit.hp < unit.kind.stats().max_hp)
            })
        {
            commands.extend([
                walk(0, vec![cast.guns.0], 30, 13),
                walk(1, vec![cast.guns.1], 30, 27),
            ]);
            bombards_withdrew = true;
        }
        state.tick(&commands);
    }
    assert!(avalanches_withdrew, "both Avalanches must trade one volley");
    assert!(bombards_withdrew, "both Bombards must trade one volley");
    state
}

/// What the golden is *for*. Every branch `kit/src/render.rs` can take
/// is named here, so a showcase that quietly stops covering one fails
/// with a sentence instead of a silent pixel match.
#[test]
fn showcase_covers_every_rendered_feature() {
    use oxide_sim::map::Terrain;
    let state = showcase_state();
    let tiles = || state.map().iter().map(|(_, t)| t);

    assert!(
        state.result().is_none(),
        "the showcase pictures a live match; a decided one freezes production"
    );
    for terrain in [Terrain::Ground, Terrain::Rock, Terrain::Peak, Terrain::Pit] {
        assert!(
            tiles().any(|t| t.terrain == terrain),
            "no {terrain:?} tile on the showcase map"
        );
    }
    assert!(
        tiles().any(|t| t.cosmetic == 1),
        "no rubble: the cosmetic-ground branch goes unrendered"
    );
    assert!(
        tiles().any(|t| t.scrap > oxide_sim::stats::SCRAP_NODE_AMOUNT),
        "no rich node"
    );
    assert!(
        tiles().any(|t| t.scrap == oxide_sim::stats::SCRAP_NODE_AMOUNT),
        "no untouched node"
    );
    let worked = state.map().scrap_at(WORKED_NODE);
    assert!(
        worked > 0 && worked * 2 <= oxide_sim::stats::SCRAP_NODE_AMOUNT,
        "the worked node holds {worked}, outside the renderer's depleted tint"
    );
    assert!(
        tiles().any(|t| t.wreck > 0),
        "no wreck salvage on the field"
    );

    for kind in [
        BuildingKind::Foundry,
        BuildingKind::Turret,
        BuildingKind::Fabricator,
        BuildingKind::FlakTurret,
        BuildingKind::Bastion,
        BuildingKind::Array,
        BuildingKind::Reclaimer,
        BuildingKind::RepairBay,
        BuildingKind::Barricade,
        BuildingKind::ScuttleCharge,
    ] {
        assert!(
            state
                .buildings()
                .iter()
                .any(|b| b.kind == kind && b.built()),
            "no standing {kind:?}"
        );
        // Anything constructible must show a site form, the Foundry
        // included.
        assert_eq!(
            kind.base_stats().construction.is_some(),
            state
                .buildings()
                .iter()
                .any(|b| b.kind == kind && !b.built()),
            "{kind:?}'s scaffolding coverage disagrees with whether it can be built"
        );
    }

    for kind in UnitKind::ALL {
        assert!(
            state
                .units()
                .iter()
                .any(|u| u.kind == kind && u.hp < kind.stats().max_hp),
            "no wounded {kind:?} to draw a health bar for"
        );
    }
    assert!(
        state
            .units()
            .iter()
            .any(|u| u.kind == UnitKind::Harvester && u.carrying() > 0),
        "no laden harvester: the carried-scrap dot goes unrendered"
    );

    // Three mutually hostile seats, one past the first two palette
    // entries: each seat's standing Foundry must paint in its own colour.
    let picture = render::render_state(&state);
    let tile = picture.width() / u32::try_from(state.map().width()).unwrap();
    let seats = [PlayerId(0), PlayerId(1), PlayerId(2)];
    let fills = seats.map(|seat| {
        let foundry = state
            .buildings()
            .iter()
            .find(|b| b.player == seat && b.kind == BuildingKind::Foundry && b.built())
            .unwrap_or_else(|| panic!("{seat:?} has no standing Foundry to sample"));
        let centre = |anchor: i32| (u32::try_from(anchor).unwrap() + 1) * tile;
        picture
            .pixel(centre(foundry.anchor.x), centre(foundry.anchor.y))
            .unwrap()
    });
    for (i, a) in seats.iter().enumerate() {
        for (j, b) in seats.iter().enumerate().skip(i + 1) {
            assert!(state.hostile(*a, *b), "{a:?} and {b:?} must be foes");
            assert_ne!(
                fills[i], fills[j],
                "{a:?} and {b:?} paint in the same colour"
            );
        }
    }
}

#[test]
fn showcase_matches_golden() {
    golden_check("showcase", &showcase_state());
}
