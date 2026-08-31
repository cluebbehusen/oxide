//! Derelict Extractor frames: the contestable economy. The map authors
//! 2x2 frames ('E'); an Extractor rebuilds only there, nothing else may
//! pave a frame over, the machine earns a fixed remote yield that nearby
//! Foundry support improves, and its death re-derelicts the frame for
//! whoever holds the ground next.

use chassis::grid::TilePos;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::{
    BuildingKind, EXTRACTOR_REMOTE_INCOME_PER_MINUTE, EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE,
};
use oxide_sim::{
    Command, Event, ExtractorIncome, Faction, PlayerCommand, PlayerId, Scenario, State, UnitKind,
};

fn players(scrap: u32) -> Vec<PlayerSpec> {
    vec![
        PlayerSpec {
            name: "Ferrous".into(),
            faction: Faction::Ferrous,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
        PlayerSpec {
            name: "Cupric".into(),
            faction: Faction::Cupric,
            team: None,
            scrap,
            bot: false,
            bot_config: None,
        },
    ]
}

/// A 20x10 arena with one frame mid-field ('E' anchor at (9, 4)).
fn arena(scrap: u32, units: Vec<UnitSpec>, buildings: Vec<BuildingSpec>) -> Scenario {
    Scenario {
        name: "extractor-arena".into(),
        seed: 7,
        map: vec![
            "####################".into(),
            "#1.................#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#........E.........#".into(),
            "#..................#".into(),
            "#..................#".into(),
            "#...............2..#".into(),
            "#..................#".into(),
            "####################".into(),
        ],
        players: players(scrap),
        units,
        buildings,
        meta: None,
    }
}

fn cmd(player: u8, command: Command) -> PlayerCommand {
    PlayerCommand {
        player: PlayerId(player),
        command,
    }
}

fn build(
    player: u8,
    builder: oxide_sim::UnitId,
    kind: BuildingKind,
    anchor: TilePos,
) -> PlayerCommand {
    cmd(
        player,
        Command::Build {
            units: vec![builder],
            kind,
            anchor,
            queue: false,
            defer: false,
        },
    )
}

fn harvester(player: u8, x: i32, y: i32) -> UnitSpec {
    UnitSpec {
        player,
        kind: UnitKind::Harvester,
        x,
        y,
    }
}

const FRAME: TilePos = TilePos { x: 9, y: 4 };

const FOG_FRAME: TilePos = TilePos { x: 16, y: 4 };

const SUPPORT_FRAME: TilePos = TilePos { x: 20, y: 5 };
const SUPPORT_FOUNDRY: TilePos = TilePos { x: 11, y: 5 };

fn support_arena(
    scrap: u32,
    mut units: Vec<UnitSpec>,
    mut buildings: Vec<BuildingSpec>,
) -> Scenario {
    let width = 42usize;
    let height = 22usize;
    let mut tiles = vec![vec!['.'; width]; height];
    tiles[0].fill('#');
    tiles[height - 1].fill('#');
    for row in &mut tiles {
        row[0] = '#';
        row[width - 1] = '#';
    }
    tiles[1][1] = '1';
    tiles[18][38] = '2';
    tiles[SUPPORT_FRAME.y as usize][SUPPORT_FRAME.x as usize] = 'E';

    units.push(harvester(0, 10, 4));
    buildings.insert(
        0,
        BuildingSpec {
            player: 0,
            kind: BuildingKind::Extractor,
            x: SUPPORT_FRAME.x,
            y: SUPPORT_FRAME.y,
        },
    );
    Scenario {
        name: "extractor-support-arena".into(),
        seed: 17,
        map: tiles
            .into_iter()
            .map(|row| row.into_iter().collect())
            .collect(),
        players: players(scrap),
        units,
        buildings,
        meta: None,
    }
}

fn building(player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingSpec {
    BuildingSpec {
        player,
        kind,
        x: anchor.x,
        y: anchor.y,
    }
}

fn extractor_id(state: &State) -> oxide_sim::BuildingId {
    state
        .buildings()
        .iter()
        .find(|building| building.player == PlayerId(0) && building.kind == BuildingKind::Extractor)
        .expect("the support arena starts with an Extractor")
        .id
}

fn earnings_over(mut state: State, start: u64, ticks: u64) -> u32 {
    if start != state.current_tick() {
        let mut value = serde_json::to_value(&state).unwrap();
        value["tick"] = serde_json::json!(start);
        state = serde_json::from_value(value).unwrap();
    }
    let before = state.player(PlayerId(0)).scrap;
    for _ in 0..ticks {
        state.tick(&[]);
    }
    state.player(PlayerId(0)).scrap - before
}

/// A wider field where the frame begins outside seat zero's Foundry sight.
fn fog_arena(units: Vec<UnitSpec>) -> Scenario {
    Scenario {
        name: "extractor-fog-arena".into(),
        seed: 11,
        map: vec![
            "################################".into(),
            "#1..........................2..#".into(),
            "#..............................#".into(),
            "#..............................#".into(),
            "#...............E..............#".into(),
            "#..............................#".into(),
            "#..............................#".into(),
            "#..............................#".into(),
            "#..............................#".into(),
            "################################".into(),
        ],
        players: players(1_000),
        units,
        buildings: vec![],
        meta: None,
    }
}

#[test]
fn the_frame_parses_stays_walkable_and_renders() {
    let state = arena(100, vec![], vec![]).build().unwrap();
    assert_eq!(state.map().extractor_frames(), &[FRAME]);
    assert!(state.map().is_extractor_frame(FRAME));
    assert!(state.map().tile_in_extractor_frame(TilePos::new(10, 5)));
    assert!(!state.map().tile_in_extractor_frame(TilePos::new(11, 4)));
    // A collapsed hulk is rubble you can drive over.
    for dy in 0..2 {
        for dx in 0..2 {
            assert!(state.passable(FRAME.offset(dx, dy)));
        }
    }
    let rows = state.map().ascii_rows();
    assert_eq!(rows[4].chars().nth(9), Some('E'));
}

#[test]
fn a_known_frame_snaps_every_visible_tile_without_revealing_an_unknown_one() {
    let unseen = fog_arena(vec![]).build().unwrap();
    let bottom_right = FOG_FRAME.offset(1, 1);
    assert_eq!(
        unseen.canonical_build_anchor(PlayerId(0), BuildingKind::Extractor, bottom_right),
        bottom_right,
        "an undiscovered frame must not pull the cursor toward itself"
    );

    let known = fog_arena(vec![harvester(0, FOG_FRAME.x - 2, FOG_FRAME.y)])
        .build()
        .unwrap();
    for dy in 0..2 {
        for dx in 0..2 {
            assert_eq!(
                known.canonical_build_anchor(
                    PlayerId(0),
                    BuildingKind::Extractor,
                    FOG_FRAME.offset(dx, dy),
                ),
                FOG_FRAME,
                "every tile painted as one frame snaps to its authored anchor"
            );
        }
    }
    assert_eq!(
        known.canonical_build_anchor(PlayerId(0), BuildingKind::Turret, bottom_right),
        bottom_right,
        "ordinary buildings keep ordinary tile anchors"
    );
}

#[test]
fn placement_reasons_do_not_disclose_frames_in_unexplored_ground() {
    let state = fog_arena(vec![]).build().unwrap();
    let unknown_elsewhere = TilePos::new(23, 4);

    for anchor in [FOG_FRAME, FOG_FRAME.offset(1, 1), unknown_elsewhere] {
        assert_eq!(
            state.place_refusal(PlayerId(0), BuildingKind::Extractor, anchor),
            Some(oxide_sim::PlaceRefusal::Fog),
            "strict placement says only that the ground is unseen at {anchor:?}"
        );
        assert_eq!(
            state.place_intent_refusal(PlayerId(0), BuildingKind::Extractor, anchor),
            Some(oxide_sim::PlaceRefusal::Fog),
            "deferred placement says only that the ground is unexplored at {anchor:?}"
        );
    }
    assert_eq!(
        state.place_refusal(PlayerId(0), BuildingKind::Turret, FOG_FRAME),
        Some(oxide_sim::PlaceRefusal::Fog),
        "strict placement cannot reveal that a normal footprint overlaps the hidden frame"
    );
    assert_eq!(
        state.place_intent_refusal(PlayerId(0), BuildingKind::Turret, FOG_FRAME),
        Some(oxide_sim::PlaceRefusal::Fog),
        "deferred placement cannot reveal that a normal footprint overlaps the hidden frame"
    );
}

#[test]
fn an_unseen_enemy_claim_does_not_replace_the_remembered_frame() {
    let mut state = fog_arena(vec![
        harvester(0, FOG_FRAME.x - 2, FOG_FRAME.y),
        harvester(1, FOG_FRAME.x + 3, FOG_FRAME.y),
    ])
    .build()
    .unwrap();
    let scout = state.units()[0].id;
    let enemy_builder = state.units()[1].id;
    assert!(state.vision(PlayerId(0)).explored(FOG_FRAME));
    assert!(state.vision(PlayerId(0)).visible(FOG_FRAME));

    state.tick(&[cmd(
        0,
        Command::Move {
            units: vec![scout],
            goal: TilePos::new(3, 7),
            queue: false,
        },
    )]);
    for _ in 0..1_000 {
        if !state.vision(PlayerId(0)).visible(FOG_FRAME) {
            break;
        }
        state.tick(&[]);
    }
    assert!(state.vision(PlayerId(0)).explored(FOG_FRAME));
    assert!(!state.vision(PlayerId(0)).visible(FOG_FRAME));
    assert!(
        state
            .vision(PlayerId(0))
            .ghosts()
            .iter()
            .all(|ghost| ghost.anchor != FOG_FRAME)
    );

    state.tick(&[build(1, enemy_builder, BuildingKind::Extractor, FOG_FRAME)]);
    assert!(
        state
            .buildings()
            .iter()
            .any(|building| building.player == PlayerId(1) && building.anchor == FOG_FRAME),
        "the hidden opponent really claimed the frame"
    );
    assert!(
        !state.extractor_frame_claim_known(PlayerId(0), FOG_FRAME),
        "an unobserved hostile claim must not erase the frame from memory"
    );
}

#[test]
fn an_extractor_stands_only_on_its_frame_and_nothing_paves_one() {
    let mut state = arena(1_000, vec![harvester(0, 5, 4)], vec![])
        .build()
        .unwrap();
    let builder = state.units()[0].id;

    // Off-frame: refused.
    let report = state.tick(&[build(
        0,
        builder,
        BuildingKind::Extractor,
        TilePos::new(3, 3),
    )]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "an extractor away from any frame is refused"
    );

    // A turret overlapping the frame: refused — the ground stays
    // contestable forever.
    let report = state.tick(&[build(0, builder, BuildingKind::Turret, TilePos::new(10, 5))]);
    assert!(
        report
            .events
            .iter()
            .any(|e| matches!(e, Event::CommandRejected { .. })),
        "nothing may pave over a frame"
    );

    // On the frame: the restoration site stands.
    state.tick(&[build(0, builder, BuildingKind::Extractor, FRAME)]);
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::Extractor && b.anchor == FRAME && !b.built),
        "the restoration site stands on the frame"
    );
}

#[test]
fn foundry_support_uses_footprint_distance_and_requires_the_same_owner() {
    let at_boundary = support_arena(
        0,
        vec![],
        vec![building(0, BuildingKind::Foundry, SUPPORT_FOUNDRY)],
    )
    .build()
    .unwrap();
    let extractor = extractor_id(&at_boundary);
    let foundry = at_boundary
        .buildings()
        .iter()
        .find(|building| {
            building.kind == BuildingKind::Foundry && building.anchor == SUPPORT_FOUNDRY
        })
        .expect("the support Foundry stands")
        .id;
    assert_eq!(
        state_income(&at_boundary, extractor),
        ExtractorIncome::Supported,
        "anchors are nine tiles apart, but the 2x2 footprints are exactly eight apart"
    );
    assert!(at_boundary.extractor_supported_by(extractor, foundry));

    let outside = support_arena(
        0,
        vec![],
        vec![building(
            0,
            BuildingKind::Foundry,
            SUPPORT_FOUNDRY.offset(-1, 0),
        )],
    )
    .build()
    .unwrap();
    let extractor = extractor_id(&outside);
    assert_eq!(state_income(&outside, extractor), ExtractorIncome::Remote);

    let wrong_owner = support_arena(
        0,
        vec![],
        vec![building(1, BuildingKind::Foundry, SUPPORT_FOUNDRY)],
    )
    .build()
    .unwrap();
    let extractor = extractor_id(&wrong_owner);
    let enemy_foundry = wrong_owner
        .buildings()
        .iter()
        .find(|building| {
            building.kind == BuildingKind::Foundry && building.anchor == SUPPORT_FOUNDRY
        })
        .expect("the enemy support candidate stands")
        .id;
    assert_eq!(
        state_income(&wrong_owner, extractor),
        ExtractorIncome::Remote,
        "an enemy forward base cannot develop this owner's claim"
    );
    assert!(!wrong_owner.extractor_supported_by(extractor, enemy_foundry));
}

fn state_income(state: &State, extractor: oxide_sim::BuildingId) -> ExtractorIncome {
    state
        .extractor_income(extractor)
        .expect("the authored Extractor is completed and living")
}

#[test]
fn extractor_yield_is_fixed_and_foundry_support_does_not_stack() {
    let remote = support_arena(0, vec![], vec![]).build().unwrap();
    let supported = support_arena(
        0,
        vec![],
        vec![building(0, BuildingKind::Foundry, SUPPORT_FOUNDRY)],
    )
    .build()
    .unwrap();
    let twice_supported = support_arena(
        0,
        vec![],
        vec![
            building(0, BuildingKind::Foundry, SUPPORT_FOUNDRY),
            building(0, BuildingKind::Foundry, TilePos::new(20, 14)),
        ],
    )
    .build()
    .unwrap();

    assert_eq!(
        state_income(&remote, extractor_id(&remote)).scrap_per_minute(),
        EXTRACTOR_REMOTE_INCOME_PER_MINUTE
    );
    assert_eq!(
        state_income(&supported, extractor_id(&supported)).scrap_per_minute(),
        EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
    );
    for start in [0, 24_000] {
        assert_eq!(
            earnings_over(remote.clone(), start, 20),
            2,
            "remote yield must remain 120/min at tick {start}"
        );
        assert_eq!(
            earnings_over(supported.clone(), start, 20),
            3,
            "supported yield must remain 180/min at tick {start}"
        );
        assert_eq!(
            earnings_over(twice_supported.clone(), start, 20),
            3,
            "a second nearby Foundry must not stack at tick {start}"
        );
    }
}

#[test]
fn an_unfinished_foundry_does_not_support_until_construction_completes() {
    let mut state = support_arena(
        1_000,
        vec![],
        vec![building(0, BuildingKind::Fabricator, TilePos::new(5, 5))],
    )
    .build()
    .unwrap();
    let extractor = extractor_id(&state);
    let builder = state.units()[0].id;

    state.tick(&[build(0, builder, BuildingKind::Foundry, SUPPORT_FOUNDRY)]);
    let expansion = state
        .buildings()
        .iter()
        .find(|building| {
            building.kind == BuildingKind::Foundry && building.anchor == SUPPORT_FOUNDRY
        })
        .expect("the legal expansion command places a site");
    assert!(!expansion.built);
    let expansion = expansion.id;
    assert_eq!(state_income(&state, extractor), ExtractorIncome::Remote);

    for _ in 0..1_000 {
        if state
            .building(expansion)
            .is_some_and(|building| building.built)
        {
            break;
        }
        state.tick(&[]);
    }
    assert!(
        state
            .building(expansion)
            .is_some_and(|building| building.built),
        "the adjacent Harvester completes the expansion"
    );
    assert_eq!(state_income(&state, extractor), ExtractorIncome::Supported);
}

#[test]
fn destroying_the_supporting_foundry_returns_the_extractor_to_remote_yield() {
    let attackers = (0..3)
        .map(|offset| UnitSpec {
            player: 1,
            kind: UnitKind::Avalanche,
            x: 25,
            y: 4 + offset,
        })
        .chain(std::iter::once(UnitSpec {
            player: 1,
            kind: UnitKind::Wisp,
            x: 15,
            y: 5,
        }))
        .collect();
    let mut state = support_arena(
        0,
        attackers,
        vec![building(0, BuildingKind::Foundry, SUPPORT_FOUNDRY)],
    )
    .build()
    .unwrap();
    let extractor = extractor_id(&state);
    let expansion = state
        .buildings()
        .iter()
        .find(|building| {
            building.kind == BuildingKind::Foundry && building.anchor == SUPPORT_FOUNDRY
        })
        .unwrap()
        .id;
    let artillery = state
        .units()
        .iter()
        .filter(|unit| unit.player == PlayerId(1) && unit.kind == UnitKind::Avalanche)
        .map(|unit| unit.id)
        .collect();
    assert_eq!(state_income(&state, extractor), ExtractorIncome::Supported);

    state.tick(&[cmd(
        1,
        Command::Attack {
            units: artillery,
            target: oxide_sim::Target::Building(expansion),
            queue: false,
        },
    )]);
    for _ in 0..2_000 {
        if state.building(expansion).is_none() {
            break;
        }
        state.tick(&[]);
    }
    assert!(
        state.building(expansion).is_none(),
        "sustained artillery destroys the support Foundry"
    );
    assert!(
        state.building(extractor).is_some(),
        "the Extractor survives"
    );
    assert_eq!(state_income(&state, extractor), ExtractorIncome::Remote);
}

#[test]
fn the_contest_cycle_re_derelicts_and_rebuilds() {
    let mut scenario = arena(
        1_000,
        vec![
            harvester(1, 12, 4),
            UnitSpec {
                player: 1,
                kind: UnitKind::Sentinel,
                x: 12,
                y: 5,
            },
        ],
        vec![],
    );
    scenario.buildings.push(BuildingSpec {
        player: 0,
        kind: BuildingKind::Extractor,
        x: FRAME.x,
        y: FRAME.y,
    });
    let mut state = scenario.build().unwrap();
    let machine = state
        .buildings()
        .iter()
        .find(|b| b.kind == BuildingKind::Extractor)
        .unwrap()
        .id;
    let (rebuilder, raider) = (state.units()[0].id, state.units()[1].id);

    // The raider grinds the restored machine down.
    state.tick(&[cmd(
        1,
        Command::Attack {
            units: vec![raider],
            target: oxide_sim::Target::Building(machine),
            queue: false,
        },
    )]);
    let mut fell = false;
    for _ in 0..4_000 {
        let report = state.tick(&[]);
        if report
            .events
            .iter()
            .any(|e| matches!(e, Event::BuildingDestroyed { building, .. } if *building == machine))
        {
            fell = true;
            break;
        }
    }
    assert!(fell, "the machine falls to sustained fire");
    assert!(
        state.map().is_extractor_frame(FRAME),
        "the frame outlives the machine"
    );

    // The other side rebuilds on the same frame and owns the yield.
    state.tick(&[build(1, rebuilder, BuildingKind::Extractor, FRAME)]);
    assert!(
        state
            .buildings()
            .iter()
            .any(|b| b.kind == BuildingKind::Extractor
                && b.anchor == FRAME
                && b.player == PlayerId(1)),
        "the ground changes hands"
    );
}
