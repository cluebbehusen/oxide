//! The Crucible's smelter: a standing Crucible melts battlefield wrecks
//! within its ring into scrap, one unit per pulse — the immediate
//! utility that makes the tier-three climb a purchase instead of dead
//! spend. Fuel beyond the ring is not its to take.

mod common;
use common::open_arena;

use chassis::grid::TilePos;
use oxide_sim::scenario::{BuildingSpec, PlayerSpec, UnitSpec};
use oxide_sim::stats::{BuildingKind, CRUCIBLE_SMELT_PERIOD};
use oxide_sim::{Faction, PlayerId, Scenario, State, UnitKind};

/// A walled yard: seat 0 owns a Crucible with a Turret beside it and a
/// second Turret far outside the smelter ring; seat 1's two doomed
/// harvesters stand at each Turret's feet. One parked own harvester
/// keeps the recovery machinery quiet so the bank moves only when the
/// smelter feeds it.
fn yard() -> Scenario {
    Scenario {
        seed: 11,
        players: vec![
            PlayerSpec {
                name: "Ferrous".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "Cupric".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Crucible,
                x: 8,
                y: 4,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 11,
                y: 4,
            },
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Turret,
                x: 25,
                y: 4,
            },
        ],
        ..open_arena(
            30,
            10,
            vec![
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Harvester,
                    x: 4,
                    y: 2,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Harvester,
                    x: 12,
                    y: 5,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Harvester,
                    x: 26,
                    y: 5,
                },
            ],
        )
    }
}

fn wreck_tiles(state: &State) -> Vec<(TilePos, u32)> {
    let mut tiles = Vec::new();
    for y in 0..10 {
        for x in 0..30 {
            let pos = TilePos::new(x, y);
            let amount = state.map().wreck_at(pos);
            if amount > 0 {
                tiles.push((pos, amount));
            }
        }
    }
    tiles
}

#[test]
fn the_smelter_melts_ring_fuel_and_leaves_the_far_field() {
    let mut state = yard().build().unwrap();
    // Let the turrets down both trespassers; their wrecks land at their
    // feet — one inside the smelter ring, one far outside it.
    for _ in 0..400 {
        state.tick(&[]);
        if wreck_tiles(&state).len() >= 2 {
            break;
        }
    }
    let field = wreck_tiles(&state);
    assert_eq!(field.len(), 2, "two kills leave two wreck tiles: {field:?}");
    let (near, near_amount) = field[0];
    let (far, far_amount) = field[1];
    assert!(near.x < 16 && far.x > 20, "one wreck per turret: {field:?}");

    let bank = state.player(PlayerId(0)).scrap;
    // Cross the next several pulse boundaries. Global wreck decay may
    // also step inside the window; the far wreck isolates its rate so
    // the smelter's exact draw stays provable.
    let start = state.current_tick();
    let until = (start / CRUCIBLE_SMELT_PERIOD + 5) * CRUCIBLE_SMELT_PERIOD;
    while state.current_tick() < until {
        state.tick(&[]);
    }
    let pulses = state.player(PlayerId(0)).scrap - bank;
    assert!(
        (4..=5).contains(&pulses),
        "one scrap per pulse crossed, got {pulses}"
    );
    let decay = far_amount - state.map().wreck_at(far);
    assert!(
        decay <= 1,
        "at most one decay step fits the window: {decay}"
    );
    assert_eq!(
        state.map().wreck_at(near),
        near_amount - pulses - decay,
        "the ring wreck fed exactly the scrap credited (plus global decay)"
    );
}

#[test]
fn mirrored_crucibles_smelt_mirrored_wreck_tiles() {
    // Two wrecks at equal reach of each crucible, one above and one below,
    // so only the tie-break decides which tile feeds first. Mirrored seats
    // must burn mirrored tiles, not whichever an absolute scan meets first.
    let (width, height) = (30usize, 10usize);
    let mut rows = vec![vec!['.'; width]; height];
    for row in rows.iter_mut() {
        row[0] = '#';
        row[width - 1] = '#';
    }
    rows[0] = vec!['#'; width];
    rows[height - 1] = vec!['#'; width];
    rows[1][1] = '1';
    rows[height - 3][width - 3] = '2';
    let (cw, ch) = BuildingKind::Crucible.base_stats().size;
    let left_anchor = TilePos::new(6, 4);
    let right_anchor = TilePos::new(
        width as i32 - cw - left_anchor.x,
        height as i32 - ch - left_anchor.y,
    );
    let scenario = Scenario {
        mode: Default::default(),
        name: "mirrored-smelters".into(),
        seed: 3,
        map: rows.into_iter().map(|r| r.into_iter().collect()).collect(),
        players: vec![
            PlayerSpec {
                name: "West".into(),
                faction: Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
            PlayerSpec {
                name: "East".into(),
                faction: Faction::Cupric,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            },
        ],
        units: Vec::new(),
        buildings: vec![
            BuildingSpec {
                player: 0,
                kind: BuildingKind::Crucible,
                x: left_anchor.x,
                y: left_anchor.y,
            },
            BuildingSpec {
                player: 1,
                kind: BuildingKind::Crucible,
                x: right_anchor.x,
                y: right_anchor.y,
            },
        ],
        meta: None,
    };
    let state = scenario.build().unwrap();
    let mirror =
        |tile: TilePos| TilePos::new(width as i32 - 1 - tile.x, height as i32 - 1 - tile.y);
    let above = TilePos::new(left_anchor.x, left_anchor.y - 2);
    let below = TilePos::new(left_anchor.x, left_anchor.y + ch + 1);
    let mut doc = serde_json::to_value(&state).unwrap();
    for tile in [above, below, mirror(above), mirror(below)] {
        let index = tile.y as usize * width + tile.x as usize;
        doc["map"]["grid"]["cells"][index]["wreck"] = serde_json::json!(5);
    }
    let mut state: State = serde_json::from_value(doc).unwrap();
    // One pulse: a second would take the other, now richer, tile on both
    // sides and hide which tile each crucible burned first.
    assert!(state.current_tick().is_multiple_of(CRUCIBLE_SMELT_PERIOD));
    state.tick(&[]);
    assert_eq!(
        state.player(PlayerId(0)).scrap,
        state.player(PlayerId(1)).scrap
    );
    assert!(
        state.player(PlayerId(0)).scrap > 0,
        "the smelters never fed"
    );
    for tile in [above, below] {
        assert_eq!(
            state.map().wreck_at(tile),
            state.map().wreck_at(mirror(tile)),
            "mirrored crucibles burned different tiles: {:?} vs {:?}",
            wreck_tiles(&state),
            tile
        );
    }
}

#[test]
fn a_centered_crucible_smelts_in_its_owners_home_frame() {
    // A crucible on the exact map center is its own mirror image, so the
    // owner's home side has to orient the tie-break: the same crucible
    // owned from the other Foundry burns the mirrored tile.
    let (width, height) = (30usize, 10usize);
    let anchor = TilePos::new(14, 4);
    let build = |owner: u8| {
        let mut rows = vec![vec!['.'; width]; height];
        for row in rows.iter_mut() {
            row[0] = '#';
            row[width - 1] = '#';
        }
        rows[0] = vec!['#'; width];
        rows[height - 1] = vec!['#'; width];
        rows[1][1] = '1';
        rows[height - 3][width - 3] = '2';
        let scenario = Scenario {
            mode: Default::default(),
            name: "centered-smelter".into(),
            seed: 3,
            map: rows.into_iter().map(|r| r.into_iter().collect()).collect(),
            players: vec![
                PlayerSpec {
                    name: "West".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "East".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: Vec::new(),
            buildings: vec![BuildingSpec {
                player: owner,
                kind: BuildingKind::Crucible,
                x: anchor.x,
                y: anchor.y,
            }],
            meta: None,
        };
        scenario.build().unwrap()
    };
    let mirror =
        |tile: TilePos| TilePos::new(width as i32 - 1 - tile.x, height as i32 - 1 - tile.y);
    let (cw, ch) = BuildingKind::Crucible.base_stats().size;
    assert_eq!(
        (anchor.x * 2 + cw, anchor.y * 2 + ch),
        (width as i32, height as i32)
    );
    let above = TilePos::new(anchor.x, anchor.y - 2);
    let below = mirror(above);
    let burn = |owner: u8| {
        let state = build(owner);
        let mut doc = serde_json::to_value(&state).unwrap();
        for tile in [above, below] {
            let index = tile.y as usize * width + tile.x as usize;
            doc["map"]["grid"]["cells"][index]["wreck"] = serde_json::json!(5);
        }
        let mut state: State = serde_json::from_value(doc).unwrap();
        assert!(state.current_tick().is_multiple_of(CRUCIBLE_SMELT_PERIOD));
        state.tick(&[]);
        assert!(
            state.player(PlayerId(owner)).scrap > 0,
            "the smelter never fed"
        );
        (state.map().wreck_at(above), state.map().wreck_at(below))
    };
    let west = burn(0);
    let east = burn(1);
    assert_ne!(west.0, west.1, "one tile burned first");
    assert_eq!(
        (west.0, west.1),
        (east.1, east.0),
        "the eastern owner must burn the mirrored tile first"
    );
}
