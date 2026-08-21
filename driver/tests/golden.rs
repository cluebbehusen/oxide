//! Golden-image regression tests.
//!
//! The driver's software renderer is bit-deterministic, so these compare
//! PNG bytes exactly — no tolerance thresholds to tune. When an intentional
//! sim or renderer change moves the pixels:
//!
//! 1. `BLESS=1 cargo test -p oxide-driver` to regenerate,
//! 2. *look at* the regenerated PNGs in `driver/tests/goldens/`,
//! 3. commit them together with the change and say why.

use chassis::grid::TilePos;
use oxide_driver::render;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::{
    BuildingKind, Command, Faction, PlayerCommand, PlayerId, Scenario, State, Target, UnitId,
    UnitKind,
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

#[test]
fn skirmish_midgame_matches_golden() {
    // The Overseer — the scripted QA anchor — drives both seats by
    // hand: bot seats proper are inert until the retrained actor
    // ships, and an idle world would be a vacuous midgame picture.
    let mut scenario = Scenario::skirmish();
    for player in &mut scenario.players {
        player.bot = true;
    }
    let mut state = scenario.build().unwrap();
    let mut bots: Vec<oxide_sim::bot::Brain> = (0..scenario.players.len())
        .map(|seat| oxide_sim::bot::Brain::overseer(PlayerId(seat as u8), scenario.seed))
        .collect();
    for _ in 0..1200 {
        let commands: Vec<PlayerCommand> =
            bots.iter_mut().flat_map(|bot| bot.act(&state)).collect();
        state.tick(&commands);
    }
    golden_check("skirmish-t1200", &state);
}

// ---------------------------------------------------------------------------
// The showcase: one state holding everything the CPU renderer draws.
// ---------------------------------------------------------------------------
//
// The skirmish goldens only ever exercise ground, rock, full nodes and
// healthy machines. This scenario is built in test code — never under
// `scenarios/`, which ships to players and is swept by the hash fixtures,
// the liveness gate and the map gates — and driven through a scripted
// program until the final state carries every branch of
// `kit/src/render.rs`: all three terrains, rubble, scrap full and rich and
// worked down past half, a wreck tile, standing and half-built structures
// of every kind, damaged machines of every kind on both rosters, a laden
// harvester, and a same-faction hostile pair.
//
// `showcase_covers_every_rendered_feature` is the readable half of the
// contract: it names each of those, so a script that quietly stops
// producing one fails with a sentence rather than a pixel diff.

/// The showcase playfield. Anchor `1` is seat 0's north-west base beside
/// the worked node, `2` seat 1's south-east construction yard, `3` seat
/// 2's south-west corner — Ferrous like seat 0, and hostile to it.
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
/// enough for one volley from every weapon and the artillery's shells to
/// fly; short enough that only the two machines a Lancer one-shots die.
const FIGHT_TICKS: u64 = 24;

/// Total ticks. Sized so the worked node lands in the renderer's
/// depleted tint without mining out.
const SHOWCASE_TICKS: u64 = 400;

/// The crew steps off the node before the picture is taken — eight
/// harvesters ringing a tile would hide the very thing they mined.
const CREW_STEPS_OFF: u64 = SHOWCASE_TICKS - 30;

/// The buried-charge site is founded just before the picture: at a
/// fifth of 20 hp it survives only a couple of abandonment-decay beats.
const CHARGE_FOUNDS: u64 = SHOWCASE_TICKS - 12;

/// Collects unit specs while handing back the id each one will get:
/// `Scenario::build` spawns them in list order, so the index *is* the id.
#[derive(Default)]
struct Roster {
    units: Vec<UnitSpec>,
}

impl Roster {
    fn add(&mut self, player: u8, kind: UnitKind, x: i32, y: i32) -> UnitId {
        let id = UnitId(self.units.len() as u32);
        self.units.push(UnitSpec { player, kind, x, y });
        id
    }
}

fn seat(name: &str, faction: Faction, scrap: u32) -> PlayerSpec {
    PlayerSpec {
        name: name.into(),
        faction,
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
    /// Seat 2's lone machine: hostile to seat 0 and wearing its colours.
    interloper: UnitId,
    /// The two Bombards, shelling each other across open ground.
    guns: (UnitId, UnitId),
    /// Seat 0's tier-three annex: Condor, Flakhound wounder, Breaker.
    annex_f: Vec<UnitId>,
    /// Seat 1's tier-three annex: Moth, Stinger wounder, Breaker.
    annex_c: Vec<UnitId>,
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
    // rosters take the same slot order, so opposite slots always agree on
    // movement domain and weapon coverage — slot 4 is each roster's
    // anti-air, slot 5 its ground-attack flyer, slot 6 its interceptor.
    let line = |faction| {
        [
            UnitKind::Harvester,
            UnitKind::Sentinel,
            UnitKind::Scuttler,
            UnitKind::Sentinel,
            oxide_sim::stats::Role::AntiAir.unit_for(faction),
            oxide_sim::stats::Role::AirGround.unit_for(faction),
            oxide_sim::stats::Role::AirAir.unit_for(faction),
            UnitKind::Lancer,
            UnitKind::Sentinel,
            UnitKind::Sentinel,
            UnitKind::Sentinel,
            // 0.15 additions, interleaved so every victim's shooter
            // stands one column over (2.24 tiles, inside every range).
            UnitKind::Warden,
            UnitKind::Tender,
            UnitKind::Sentinel,
            UnitKind::Excavator,
            UnitKind::Sentinel,
            oxide_sim::stats::Role::Scout.unit_for(faction),
            oxide_sim::stats::Role::Interceptor.unit_for(faction),
            oxide_sim::stats::Role::AntiAir.unit_for(faction),
        ]
    };
    let west: Vec<UnitId> = line(Faction::Ferrous)
        .into_iter()
        .enumerate()
        .map(|(i, kind)| roster.add(0, kind, 8 + i as i32, 20))
        .collect();
    let east: Vec<UnitId> = line(Faction::Cupric)
        .into_iter()
        .enumerate()
        .map(|(i, kind)| roster.add(1, kind, 8 + i as i32, 22))
        .collect();
    let interloper = roster.add(2, UnitKind::Sentinel, 20, 20);
    // The same-faction-foes pin must not depend on the interloper
    // surviving its cameo among thirty-eight hostiles: a second South
    // Ferrous machine idles in the far corner where nothing ever walks.
    roster.add(2, UnitKind::Harvester, 44, 2);

    // Artillery: far enough off the line that the splash reaches only
    // the other gun, close enough that each is its own spotter.
    let gun_west = roster.add(0, UnitKind::Bombard, 29, 20);
    let gun_east = roster.add(1, UnitKind::Bombard, 29, 22);

    // The tier-three annex, northeast of the pit and clear of every
    // march lane: each new 0.15 kind stands next to (or five tiles
    // from) the thing that wounds it inside the 24-tick fight window.
    // Bombers are victims here, not shooters — a released bomb's 2.2
    // splash would rewrite the carefully bounded wounds around it.
    let condor = roster.add(0, UnitKind::Condor, 39, 12);
    let stinger_annex = roster.add(1, UnitKind::Stinger, 40, 12);
    let flakhound_annex = roster.add(0, UnitKind::Flakhound, 43, 12);
    let moth = roster.add(1, UnitKind::Moth, 44, 12);
    let breaker_w = roster.add(0, UnitKind::Breaker, 40, 9);
    let breaker_e = roster.add(1, UnitKind::Breaker, 43, 9);
    // Five tiles apart: outside both blind rings, inside both reaches,
    // spotted for each seat by its annex flak sitting four tiles off.
    let avalanche_w = roster.add(0, UnitKind::Avalanche, 39, 16);
    let avalanche_e = roster.add(1, UnitKind::Avalanche, 44, 16);
    // The unarmed slings, each with its own flak wounder, two tiles
    // below the Avalanche exchange (outside its 1.6 splash).
    let skyhook_w = roster.add(0, UnitKind::Skyhook, 39, 18);
    let stinger_sling = roster.add(1, UnitKind::Stinger, 40, 18);
    let flakhound_sling = roster.add(0, UnitKind::Flakhound, 43, 18);
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
        name: "renderer showcase".into(),
        seed: 20_130,
        map: SHOWCASE_MAP.iter().map(|r| (*r).to_string()).collect(),
        players: vec![
            seat("West Ferrous", Faction::Ferrous, 700),
            seat("East Cupric", Faction::Cupric, 2000),
            seat("South Ferrous", Faction::Ferrous, 100),
        ],
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
            annex_f: vec![
                condor,
                flakhound_annex,
                breaker_w,
                skyhook_w,
                flakhound_sling,
                sapper_w,
                sentinel_sapper_w,
            ],
            annex_c: vec![
                moth,
                stinger_annex,
                breaker_e,
                skyhook_e,
                stinger_sling,
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
            target: Target::Unit(victim),
            queue: false,
        },
    }
}

fn walk(player: u8, units: Vec<UnitId>, x: i32, y: i32) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command: Command::Move {
            units,
            goal: TilePos::new(x, y),
            queue: false,
        },
    }
}

/// The tick the construction yard founds its seven scaffolds. Late
/// enough that abandonment decay (one hp per SITE_DECAY_PERIOD) cannot
/// finish off even the frailest site before the picture is taken.
const YARD_FOUNDS: u64 = 200;

/// Seven sites, founded and then abandoned: the builder's order is
/// replaced each time, but the ground is claimed on placement.
fn yard_orders(cast: &Cast) -> Vec<PlayerCommand> {
    [
        (BuildingKind::Turret, 33, 23),
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
            queue: false,
            defer: false,
        },
    })
    .collect()
}

/// The western field kit, founded by a crew harvester and then
/// abandoned with the same shrug as the eastern yard.
fn field_kit_orders(cast: &Cast) -> Vec<PlayerCommand> {
    [(BuildingKind::Barricade, 2, 10)]
        .into_iter()
        .map(|(kind, x, y)| PlayerCommand {
            player: PlayerId(0),
            command: Command::Build {
                units: vec![cast.crew[2]],
                kind,
                anchor: TilePos::new(x, y),
                queue: false,
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
    // Seat 0's Foundry expansion: the 0.15 buildable base, founded by a
    // crew harvester behind the standing Fabricator's tech gate. Its
    // builder stays on the site, so it renders as attended construction.
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
        // The 0.15 roster's wounds: Wardens trade, the flanking
        // sentinels wound the labor machines, the interceptors clip the
        // scouts, and the anti-air rear clips the interceptors.
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
        attack(0, cast.annex_f[2], cast.annex_c[2]),
        attack(1, cast.annex_c[2], cast.annex_f[2]),
        attack(0, cast.annex_f[1], cast.annex_c[0]),
        attack(1, cast.annex_c[1], cast.annex_f[0]),
        attack(0, cast.avalanches.0, cast.avalanches.1),
        attack(1, cast.avalanches.1, cast.avalanches.0),
        attack(0, cast.annex_f[4], cast.annex_c[3]),
        attack(1, cast.annex_c[4], cast.annex_f[3]),
        attack(0, cast.annex_f[6], cast.annex_c[5]),
        attack(1, cast.annex_c[6], cast.annex_f[5]),
    ]);
    commands
}

/// The disengagement: every survivor walks somewhere nothing of another
/// colour can reach, so the long economy tail runs quiet.
fn disengage(cast: &Cast) -> Vec<PlayerCommand> {
    vec![
        walk(0, cast.west.clone(), 6, 14),
        walk(1, cast.east.clone(), 14, 27),
        // Clear of the widened west column's march lane (idle aggro
        // killed it at its old post once the line grew eight slots).
        walk(2, vec![cast.interloper], 36, 6),
        walk(0, vec![cast.guns.0], 30, 13),
        walk(1, vec![cast.guns.1], 30, 27),
        walk(0, cast.annex_f.clone(), 32, 15),
        walk(1, cast.annex_c.clone(), 47, 27),
        walk(0, vec![cast.avalanches.0], 10, 10),
        walk(1, vec![cast.avalanches.1], 46, 27),
    ]
}

fn showcase_state() -> State {
    let (scenario, cast) = showcase_scenario();
    let mut state = scenario.build().expect("the showcase scenario is valid");
    for tick in 0..SHOWCASE_TICKS {
        let commands = match tick {
            0 => opening_orders(&cast),
            // The construction yard founds late and is then abandoned:
            // orphaned scaffolds decay now, and the frailest (the Array,
            // a fifth of 250 hp) must still be standing at the picture.
            t if t == YARD_FOUNDS => {
                let mut commands = yard_orders(&cast);
                commands.extend(field_kit_orders(&cast));
                commands
            }
            t if t == YARD_FOUNDS + 1 => vec![
                PlayerCommand {
                    player: PlayerId(1),
                    command: Command::Stop {
                        units: vec![cast.builder],
                    },
                },
                PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Stop {
                        units: vec![cast.crew[2]],
                    },
                },
            ],
            t if t == FIGHT_TICKS => disengage(&cast),
            t if t == CREW_STEPS_OFF => vec![walk(0, cast.crew.clone(), 2, 6)],
            t if t == CHARGE_FOUNDS => vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::Build {
                    units: vec![cast.crew[2]],
                    kind: BuildingKind::ScuttleCharge,
                    anchor: TilePos::new(6, 10),
                    queue: false,
                    defer: false,
                },
            }],
            t if t == CHARGE_FOUNDS + 1 => vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::Stop {
                    units: vec![cast.crew[2]],
                },
            }],
            _ => Vec::new(),
        };
        state.tick(&commands);
    }
    state
}

/// Every kind, on every roster that can field it.
fn every_kind_and_faction() -> impl Iterator<Item = (UnitKind, Faction)> {
    const KINDS: [UnitKind; 24] = [
        UnitKind::Harvester,
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Lancer,
        UnitKind::Bombard,
        UnitKind::Flakhound,
        UnitKind::Stinger,
        UnitKind::Buzzard,
        UnitKind::Darter,
        UnitKind::Talon,
        UnitKind::Wisp,
        UnitKind::Warden,
        UnitKind::Tender,
        UnitKind::Excavator,
        UnitKind::Kestrel,
        UnitKind::Gnat,
        UnitKind::Shrike,
        UnitKind::Sylph,
        UnitKind::Condor,
        UnitKind::Moth,
        UnitKind::Breaker,
        UnitKind::Avalanche,
        UnitKind::Skyhook,
        UnitKind::Sapper,
    ];
    KINDS.into_iter().flat_map(|kind| {
        [Faction::Ferrous, Faction::Cupric]
            .into_iter()
            .filter(move |f| kind.faction().is_none_or(|bound| bound == *f))
            .map(move |f| (kind, f))
    })
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
            state.buildings().iter().any(|b| b.kind == kind && b.built),
            "no standing {kind:?}"
        );
        // Anything constructible must show a site form — the Foundry
        // included, now that expansions are buildable.
        assert_eq!(
            kind.base_stats().construction.is_some(),
            state.buildings().iter().any(|b| b.kind == kind && !b.built),
            "{kind:?}'s scaffolding coverage disagrees with whether it can be built"
        );
    }

    for (kind, faction) in every_kind_and_faction() {
        assert!(
            state.units().iter().any(|u| {
                u.kind == kind
                    && state.player(u.player).faction == faction
                    && u.hp < kind.stats().max_hp
            }),
            "no wounded {faction:?} {kind:?} to draw a health bar for"
        );
    }
    assert!(
        state
            .units()
            .iter()
            .any(|u| u.kind == UnitKind::Harvester && u.carrying > 0),
        "no laden harvester: the carried-scrap dot goes unrendered"
    );

    // Two seats on one roster, at each other's throats: the software
    // renderer paints allegiance by faction alone, and this is the pair
    // that pins it.
    let same_faction_foes = state.units().iter().any(|a| {
        state.units().iter().any(|b| {
            state.hostile(a.player, b.player)
                && state.player(a.player).faction == state.player(b.player).faction
        })
    });
    assert!(same_faction_foes, "no same-faction hostile pair on the map");
}

#[test]
fn showcase_matches_golden() {
    golden_check("showcase", &showcase_state());
}
