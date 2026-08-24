//! The ground truth on screen: terrain tiles (fog-ruled), the fog
//! veil itself, and battle scars.

use super::*;

/// Fog of war from the local player's perspective: unexplored is void,
/// explored-but-unseen is dimmed.
pub(crate) fn draw_fog(game: &Game) {
    let vision = game.my_vision();
    let (min, max) = visible_tiles(game);
    for y in min.y..max.y {
        for x in min.x..max.x {
            let tile = TilePos::new(x, y);
            let cover = if !vision.explored(tile) {
                FOG_UNEXPLORED
            } else if !vision.visible(tile) {
                FOG_EXPLORED
            } else {
                continue;
            };
            // Exact shared edges: translucent rects that overlap draw
            // double-dark seams, so each tile ends where the next begins.
            let a = game.camera.to_screen(vec2(x as f32, y as f32)).floor();
            let b = game
                .camera
                .to_screen(vec2((x + 1) as f32, (y + 1) as f32))
                .floor();
            draw_rectangle(a.x, a.y, b.x - a.x, b.y - a.y, cover);
        }
    }
}

/// Fog-honest peak connectivity. An explored barrier cannot disclose that
/// its wall continues into an unexplored neighbor merely through edge art.
fn pit_neighbor_mask(game: &Game, pos: TilePos) -> u8 {
    [(0, -1, 1), (1, 0, 2), (0, 1, 4), (-1, 0, 8)]
        .into_iter()
        .fold(0, |mask, (dx, dy, bit)| {
            let neighbor = pos.offset(dx, dy);
            let known = game.all_seeing() || game.my_vision().explored(neighbor);
            let connected = known
                && game
                    .state
                    .map()
                    .tile(neighbor)
                    .is_some_and(|tile| tile.terrain == oxide_sim::map::Terrain::Pit);
            if connected { mask | bit } else { mask }
        })
}

fn peak_neighbor_mask(game: &Game, pos: TilePos) -> u8 {
    [(0, -1, 1), (1, 0, 2), (0, 1, 4), (-1, 0, 8)]
        .into_iter()
        .fold(0, |mask, (dx, dy, bit)| {
            let neighbor = pos.offset(dx, dy);
            let known = game.all_seeing() || game.my_vision().explored(neighbor);
            let connected = known
                && game
                    .state
                    .map()
                    .tile(neighbor)
                    .is_some_and(|tile| tile.terrain == oxide_sim::map::Terrain::Peak);
            if connected { mask | bit } else { mask }
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ThemePropPlacement {
    variant: usize,
    quarter_turns: u8,
}

impl ThemePropPlacement {
    fn rotation(self) -> f32 {
        // The first prop bank is made of flat surface marks. The second bank
        // contains raised objects with a baked world-space highlight and
        // shadow, so rotating those objects also rotates their lighting.
        let turns = if self.variant < 3 {
            self.quarter_turns
        } else {
            0
        };
        f32::from(turns) * std::f32::consts::FRAC_PI_2
    }
}

const ONE_TILE_ROCK_COUNT: usize = 14;
const MULTI_ROCK_FOOTPRINTS: [(i32, i32); 9] = [
    (2, 1),
    (2, 1),
    (2, 1),
    (2, 1),
    (2, 1),
    (3, 1),
    (3, 1),
    (3, 1),
    (3, 1),
];
const GROUND_BLOCKER_FOOTPRINTS: [(i32, i32); 9] = [
    (2, 2),
    (2, 1),
    (3, 2),
    (2, 2),
    (3, 2),
    (3, 2),
    (3, 1),
    (2, 2),
    (2, 2),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObstacleArt {
    Rock(usize),
    Industrial(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ObstaclePlacement {
    anchor: TilePos,
    footprint: (i32, i32),
    art: ObstacleArt,
}

impl ObstaclePlacement {
    fn covers(self, pos: TilePos) -> bool {
        pos.x >= self.anchor.x
            && pos.y >= self.anchor.y
            && pos.x < self.anchor.x + self.footprint.0
            && pos.y < self.anchor.y + self.footprint.1
    }
}

fn coordinate_hash(x: i32, y: i32, salt: u32) -> u32 {
    let mut hash = 2_166_136_261u32;
    for word in [x as u32, y as u32, salt] {
        for byte in word.to_le_bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(16_777_619);
        }
    }
    hash
}

fn placement_for_group(
    group: TilePos,
    mut known_rock: impl FnMut(TilePos) -> bool,
) -> Option<ObstaclePlacement> {
    let hash = coordinate_hash(group.x, group.y, 0x5155_4152);
    // Most rock stays as individual outcrops; selected 3x2 cells occasionally
    // resolve into one approved cluster or abandoned machine footprint.
    if !hash.is_multiple_of(3) {
        return None;
    }

    let industrial_first = (hash / 3).is_multiple_of(2);
    for family in 0..2 {
        let industrial = if family == 0 {
            industrial_first
        } else {
            !industrial_first
        };
        let footprints = if industrial {
            &GROUND_BLOCKER_FOOTPRINTS
        } else {
            &MULTI_ROCK_FOOTPRINTS
        };
        let start = (hash as usize / 11 + family * 5) % footprints.len();
        for step in 0..footprints.len() {
            let variant = (start + step) % footprints.len();
            let footprint = footprints[variant];
            let x_slack = 3 - footprint.0;
            let y_slack = 2 - footprint.1;
            let anchor = TilePos::new(
                group.x + ((hash / 37 + step as u32) % (x_slack as u32 + 1)) as i32,
                group.y + ((hash / 71 + step as u32) % (y_slack as u32 + 1)) as i32,
            );
            let fits = (0..footprint.1)
                .all(|dy| (0..footprint.0).all(|dx| known_rock(anchor.offset(dx, dy))));
            if fits {
                return Some(ObstaclePlacement {
                    anchor,
                    footprint,
                    art: if industrial {
                        ObstacleArt::Industrial(variant)
                    } else {
                        ObstacleArt::Rock(ONE_TILE_ROCK_COUNT + variant)
                    },
                });
            }
        }
    }
    None
}

fn group_origin(pos: TilePos) -> TilePos {
    TilePos::new(pos.x.div_euclid(3) * 3, pos.y.div_euclid(2) * 2)
}

fn visible_obstacle(game: &Game, group: TilePos) -> Option<ObstaclePlacement> {
    placement_for_group(group, |pos| {
        let known = game.all_seeing() || game.my_vision().explored(pos);
        known
            && game
                .state
                .map()
                .tile(pos)
                .is_some_and(|tile| tile.terrain == oxide_sim::map::Terrain::Rock)
    })
}

fn theme_code(theme: &str) -> Option<u32> {
    // Stable layout salts, chosen so each shipped theme exercises its complete
    // prop row without changing the shared density rule.
    match theme {
        "rusted-yard" => Some(1),
        "cold-circuitry" => Some(2),
        "quarry-dust" => Some(25),
        "basalt" => Some(4),
        "slag" => Some(5),
        "verdigris" => Some(6),
        _ => None,
    }
}

/// Sparse dressing picked from a coordinate and its 180-degree partner.
/// Both halves therefore choose the same art; the far half records a half-turn
/// so rotatable surface marks preserve the shipped maps' visual symmetry.
fn symmetric_theme_prop(
    theme: &str,
    pos: TilePos,
    width: i32,
    height: i32,
) -> Option<ThemePropPlacement> {
    let theme = theme_code(theme)?;
    if width <= 0 || height <= 0 {
        return None;
    }
    let mirror = TilePos::new(width - 1 - pos.x, height - 1 - pos.y);
    // A directional one-tile mark cannot be its own 180-degree partner.
    if pos == mirror {
        return None;
    }
    let (canonical, mirrored) = if (pos.y, pos.x) <= (mirror.y, mirror.x) {
        (pos, false)
    } else {
        (mirror, true)
    };
    // FNV-1a over fixed-width words: stable across platforms and Rust
    // versions, unlike DefaultHasher. Dimensions keep two differently sized
    // maps from laying down the same visible stamp.
    let mut hash = 2_166_136_261u32;
    for word in [
        theme,
        canonical.x as u32,
        canonical.y as u32,
        width as u32,
        height as u32,
    ] {
        for byte in word.to_le_bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(16_777_619);
        }
    }
    // One residue in eleven gives open ground enough history to establish a
    // map identity without turning every tile into visual noise. Residue five
    // keeps even the smallest shipped arenas above the density floor.
    if hash % 11 != 5 {
        return None;
    }
    let variant = (hash / 11 % 13) as usize;
    let base_turns = (hash / (11 * 13) % 4) as u8;
    Some(ThemePropPlacement {
        variant,
        quarter_turns: if mirrored {
            (base_turns + 2) % 4
        } else {
            base_turns
        },
    })
}

fn authored_tile(rows: &[String], pos: TilePos) -> Option<u8> {
    if pos.x < 0 || pos.y < 0 {
        return None;
    }
    rows.get(pos.y as usize)?
        .as_bytes()
        .get(pos.x as usize)
        .copied()
}

/// Props only paint authored plain-ground tiles. A 3x3 terrain patch keeps
/// rock edges and one-tile passes visually clean; salvage counts as eventual
/// open ground so an unseen node cannot change the dressing on its explored
/// neighbor. A wider digit scan leaves the starting base apron quiet.
/// Consulting authored marks rather than live scrap/wreck state also keeps
/// the dressing stable for the entire match.
fn safe_theme_prop_tile(rows: &[String], pos: TilePos) -> bool {
    if authored_tile(rows, pos) != Some(b'.') {
        return false;
    }
    for dy in -1..=1 {
        for dx in -1..=1 {
            if !matches!(
                authored_tile(rows, pos.offset(dx, dy)),
                Some(b'.' | b's' | b'S')
            ) {
                return false;
            }
        }
    }
    for dy in -4..=4 {
        for dx in -4..=4 {
            if authored_tile(rows, pos.offset(dx, dy)).is_some_and(|c| c.is_ascii_digit()) {
                return false;
            }
        }
    }
    true
}

fn symmetric_safe_theme_prop_tile(rows: &[String], pos: TilePos) -> bool {
    let height = rows.len() as i32;
    let width = rows.first().map_or(0, |row| row.len()) as i32;
    let mirror = TilePos::new(width - 1 - pos.x, height - 1 - pos.y);
    safe_theme_prop_tile(rows, pos) && safe_theme_prop_tile(rows, mirror)
}

pub(crate) fn draw_tiles(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    let size = zoom.ceil() + 1.0; // slight overlap kills seam hairlines
    let theme = game
        .scenario
        .meta
        .as_ref()
        .map(|m| m.theme.as_str())
        .unwrap_or("");
    let tint = theme_tint(theme);
    let themed = theme_code(theme).is_some();
    let map_width = game.state.map().width();
    let map_height = game.state.map().height();
    let (min, max) = visible_tiles(game);
    for y in min.y..max.y {
        for x in min.x..max.x {
            let Some(tile) = game.state.map().tile(TilePos::new(x, y)) else {
                continue;
            };
            let screen = game.camera.to_screen(vec2(x as f32, y as f32));
            // Position hashes drive all variety: deterministic, no state.
            let h = (x.wrapping_mul(31).wrapping_add(y.wrapping_mul(17))) as usize;
            let variant = h % 6;
            draw_texture_ex(
                sprites.texture(),
                screen.x.floor(),
                screen.y.floor(),
                tint,
                DrawTextureParams {
                    dest_size: Some(vec2(size, size)),
                    source: Some(sprites.ground(variant)),
                    ..Default::default()
                },
            );
            // Bottomless pit: edge-autotiled void, terraced where the
            // rim meets standing ground and continuous where the cut
            // carries on — the peak barrier's sibling.
            if tile.terrain == oxide_sim::map::Terrain::Pit {
                let mask = pit_neighbor_mask(game, TilePos::new(x, y));
                let source = sprites.pit_edge(mask, h % 2);
                draw_texture_ex(
                    sprites.texture(),
                    screen.x.floor(),
                    screen.y.floor(),
                    WHITE,
                    DrawTextureParams {
                        dest_size: Some(vec2(size, size)),
                        source: Some(source),
                        ..Default::default()
                    },
                );
            }
            // Ground dressing stays under resources, entities, and the fog
            // veil. Static sprites use no wall clock, so reduced-motion mode
            // needs no alternate path.
            let pos = TilePos::new(x, y);
            let prop_candidate =
                if tile.cosmetic == 1 || tile.terrain != oxide_sim::map::Terrain::Ground {
                    None
                } else {
                    symmetric_theme_prop(theme, pos, map_width, map_height)
                };
            let dressing = if tile.cosmetic == 1 {
                Some((sprites.decal(3), 0.0, tint))
            } else if let Some(placement) = prop_candidate {
                if symmetric_safe_theme_prop_tile(&game.scenario.map, pos) {
                    let source = if placement.variant < 3 {
                        sprites.theme_prop(theme, placement.variant)
                    } else {
                        Some(sprites.field_debris(placement.variant - 3))
                    };
                    source.map(|source| (source, placement.rotation(), WHITE))
                } else {
                    None
                }
            } else if !themed
                && tile.terrain == oxide_sim::map::Terrain::Ground
                && h.is_multiple_of(23)
            {
                Some((sprites.decal(h / 23 % 3), 0.0, tint))
            } else {
                None
            };
            if let Some((source, rotation, dressing_tint)) = dressing {
                draw_texture_ex(
                    sprites.texture(),
                    screen.x.floor(),
                    screen.y.floor(),
                    dressing_tint,
                    DrawTextureParams {
                        dest_size: Some(vec2(size, size)),
                        source: Some(source),
                        rotation,
                        ..Default::default()
                    },
                );
            }
            // Rocks cast a soft skirt onto neighboring ground.
            if tile.terrain == oxide_sim::map::Terrain::Ground {
                for (dx, dy, rotation) in [
                    (0, -1, 0.0f32),
                    (1, 0, std::f32::consts::FRAC_PI_2),
                    (0, 1, std::f32::consts::PI),
                    (-1, 0, 3.0 * std::f32::consts::FRAC_PI_2),
                ] {
                    let neighbor = TilePos::new(x + dx, y + dy);
                    let rocky = game
                        .state
                        .map()
                        .tile(neighbor)
                        .is_some_and(|t| t.terrain == oxide_sim::map::Terrain::Rock);
                    if rocky {
                        draw_texture_ex(
                            sprites.texture(),
                            screen.x.floor(),
                            screen.y.floor(),
                            tint,
                            DrawTextureParams {
                                dest_size: Some(vec2(size, size)),
                                source: Some(sprites.rock_skirt()),
                                rotation,
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            // Scrap draws at its live amount only in sight; unseen ground
            // shows what the player remembers (frozen, like ghosts).
            let seen_now = game.all_seeing() || game.my_vision().visible(pos);
            let scrap = if seen_now {
                tile.scrap
            } else {
                game.my_vision().remembered_scrap(pos)
            };
            // Wrecks follow the same sight rule; a live node or rock
            // outranks the junk visually.
            let wreck = if seen_now {
                tile.wreck
            } else {
                game.my_vision().remembered_wreck(pos)
            };
            // Stamp what is on show; fade what is only remembered. The
            // map stays bounded: only tiles carrying salvage enter it.
            let mem_fade = if scrap > 0 || wreck > 0 {
                let key = (x, y);
                if seen_now {
                    game.last_seen.borrow_mut().insert(key, game.fx_time());
                    1.0
                } else {
                    let age = {
                        let mut seen = game.last_seen.borrow_mut();
                        let stamp = *seen.entry(key).or_insert_with(|| game.fx_time());
                        game.fx_time() - stamp
                    };
                    1.0 - super::staleness_fade(age)
                }
            } else {
                1.0
            };
            let (overlay, flip) = match (tile.terrain, scrap) {
                (oxide_sim::map::Terrain::Rock, _)
                    if visible_obstacle(game, group_origin(pos))
                        .is_some_and(|placement| placement.covers(pos)) =>
                {
                    (None, false)
                }
                (oxide_sim::map::Terrain::Rock, _) => {
                    (Some(sprites.rock(h % ONE_TILE_ROCK_COUNT)), h % 7 < 3)
                }
                (oxide_sim::map::Terrain::Peak, _) => (
                    Some(sprites.peak_barrier(peak_neighbor_mask(game, pos), h % 2)),
                    false,
                ),
                // The void was already painted under the dressing pass.
                (oxide_sim::map::Terrain::Pit, _) => (None, false),
                (_, 0) if wreck > 0 => (Some(sprites.wreck_pile()), h % 5 < 2),
                (_, 0) => (None, false),
                (_, s) => (Some(sprites.scrap(s, SCRAP_NODE_AMOUNT)), false),
            };
            if let Some(source) = overlay {
                let overlay_tint = if mem_fade < 1.0 {
                    Color::new(tint.r, tint.g, tint.b, tint.a * mem_fade)
                } else {
                    tint
                };
                draw_texture_ex(
                    sprites.texture(),
                    screen.x.floor(),
                    screen.y.floor(),
                    overlay_tint,
                    DrawTextureParams {
                        dest_size: Some(vec2(size, size)),
                        source: Some(source),
                        flip_x: flip,
                        ..Default::default()
                    },
                );
            }
        }
    }

    let start = group_origin(min);
    for y in (start.y..max.y).step_by(2) {
        for x in (start.x..max.x).step_by(3) {
            let Some(placement) = visible_obstacle(game, TilePos::new(x, y)) else {
                continue;
            };
            let source = match placement.art {
                ObstacleArt::Rock(variant) => sprites.rock(variant),
                ObstacleArt::Industrial(variant) => sprites.ground_blocker(variant),
            };
            let screen = game
                .camera
                .to_screen(vec2(placement.anchor.x as f32, placement.anchor.y as f32));
            draw_texture_ex(
                sprites.texture(),
                screen.x.floor(),
                screen.y.floor(),
                WHITE,
                DrawTextureParams {
                    dest_size: Some(vec2(
                        placement.footprint.0 as f32 * zoom + 1.0,
                        placement.footprint.1 as f32 * zoom + 1.0,
                    )),
                    source: Some(source),
                    ..Default::default()
                },
            );
        }
    }
}

/// Unclaimed derelict Extractor frames, drawn as part of the map: a
/// collapsed 2x2 machine bed that says "rebuild here". A standing
/// building on the anchor covers its frame; unexplored ground hides it
/// like any other terrain fact.
pub(crate) fn draw_extractor_frames(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    for &frame in game.state.map().extractor_frames() {
        let known = game.all_seeing()
            || (0..2).any(|dy| (0..2).any(|dx| game.my_vision().explored(frame.offset(dx, dy))));
        if !known {
            continue;
        }
        let claimed = if game.all_seeing() {
            game.state
                .buildings()
                .iter()
                .any(|building| building.hp > 0 && building.anchor == frame)
        } else {
            game.state.extractor_frame_claim_known(game.human, frame)
        };
        if claimed {
            continue;
        }
        let screen = game.camera.to_screen(vec2(frame.x as f32, frame.y as f32));
        draw_texture_ex(
            sprites.texture(),
            screen.x.floor(),
            screen.y.floor(),
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(zoom * 2.0 + 1.0, zoom * 2.0 + 1.0)),
                source: Some(sprites.extractor_frame()),
                ..Default::default()
            },
        );
    }
}

/// Battle scars: scorch decals where buildings died, fading over ~20s.
pub(crate) fn draw_scorches(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    for (at, age) in &game.scorches {
        let alpha = (1.0 - age / 20.0).clamp(0.0, 1.0) * 0.85;
        let size = zoom * 2.4;
        let screen = game.camera.to_screen(*at);
        draw_texture_ex(
            sprites.texture(),
            screen.x - size * 0.5,
            screen.y - size * 0.5,
            Color::new(1.0, 1.0, 1.0, alpha),
            DrawTextureParams {
                dest_size: Some(vec2(size, size)),
                source: Some(sprites.scorch()),
                ..Default::default()
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    const THEMES: [&str; 6] = [
        "rusted-yard",
        "cold-circuitry",
        "quarry-dust",
        "basalt",
        "slag",
        "verdigris",
    ];

    #[test]
    fn theme_prop_selection_is_a_180_degree_pair() {
        for theme in THEMES {
            for height in [31, 32] {
                for width in [47, 48] {
                    for y in 0..height {
                        for x in 0..width {
                            let pos = TilePos::new(x, y);
                            let mirror = TilePos::new(width - 1 - x, height - 1 - y);
                            let a = symmetric_theme_prop(theme, pos, width, height);
                            let b = symmetric_theme_prop(theme, mirror, width, height);
                            assert_eq!(a.map(|p| p.variant), b.map(|p| p.variant));
                            if pos != mirror
                                && let (Some(a), Some(b)) = (a, b)
                            {
                                assert_eq!((a.quarter_turns + 2) % 4, b.quarter_turns);
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(
            symmetric_theme_prop("unknown", TilePos::new(4, 4), 9, 9),
            None
        );
        assert_eq!(
            symmetric_theme_prop("basalt", TilePos::new(4, 4), 9, 9),
            None
        );
    }

    #[test]
    fn raised_theme_props_keep_world_space_lighting_upright() {
        for quarter_turns in 0..4 {
            for variant in 0..3 {
                let placement = ThemePropPlacement {
                    variant,
                    quarter_turns,
                };
                assert_eq!(
                    placement.rotation(),
                    f32::from(quarter_turns) * std::f32::consts::FRAC_PI_2
                );
            }
            for variant in 3..13 {
                let placement = ThemePropPlacement {
                    variant,
                    quarter_turns,
                };
                assert_eq!(placement.rotation(), 0.0);
            }
        }
    }

    #[test]
    fn safe_prop_tiles_are_open_and_clear_of_semantic_map_marks() {
        let rows: Vec<String> = [
            "#################",
            "#1..............#",
            "#...............#",
            "#...............#",
            "#...............#",
            "#...............#",
            "#...............#",
            "#...........#...#",
            "#...............#",
            "#...............#",
            "#.......s.......#",
            "#...............#",
            "#...............#",
            "#........,......#",
            "#...............#",
            "#...............#",
            "#################",
        ]
        .map(str::to_string)
        .to_vec();
        assert!(safe_theme_prop_tile(&rows, TilePos::new(7, 7)));
        assert!(!safe_theme_prop_tile(&rows, TilePos::new(1, 1)));
        assert!(!safe_theme_prop_tile(&rows, TilePos::new(4, 4)));
        assert!(!safe_theme_prop_tile(&rows, TilePos::new(12, 7)));
        assert!(!safe_theme_prop_tile(&rows, TilePos::new(11, 7)));
        assert!(!safe_theme_prop_tile(&rows, TilePos::new(8, 10)));
        assert!(
            safe_theme_prop_tile(&rows, TilePos::new(7, 9)),
            "hidden salvage must not suppress a neighboring prop"
        );
        assert!(!safe_theme_prop_tile(&rows, TilePos::new(9, 13)));
    }

    #[test]
    fn every_shipped_map_gets_symmetric_safe_theme_dressing() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
        let mut paths: Vec<_> = std::fs::read_dir(&root)
            .unwrap_or_else(|err| panic!("reading {}: {err}", root.display()))
            .map(|entry| entry.expect("scenario directory entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        let mut seen_themes = BTreeSet::new();
        let mut variants_by_theme = BTreeMap::<String, BTreeSet<usize>>::new();
        let mut all_variants = BTreeSet::new();
        let mut total_safe = 0;
        let mut total_props = 0;
        for path in paths {
            let scenario = oxide_sim::Scenario::load(&path)
                .unwrap_or_else(|err| panic!("loading {}: {err}", path.display()));
            let theme = scenario
                .meta
                .as_ref()
                .map(|meta| meta.theme.as_str())
                .unwrap_or("");
            assert!(
                theme_code(theme).is_some(),
                "{} has no generated theme-prop row",
                path.display()
            );
            seen_themes.insert(theme.to_string());
            let height = scenario.map.len() as i32;
            let width = scenario.map.first().expect("map row").len() as i32;
            let mut count = 0;
            let mut safe_count = 0;
            let mut variants = BTreeSet::new();
            for y in 0..height {
                for x in 0..width {
                    let pos = TilePos::new(x, y);
                    let mirror = TilePos::new(width - 1 - x, height - 1 - y);
                    assert_eq!(
                        symmetric_safe_theme_prop_tile(&scenario.map, pos),
                        symmetric_safe_theme_prop_tile(&scenario.map, mirror),
                        "{} safe dressing mask is not symmetric at ({x}, {y})",
                        path.display()
                    );
                    if !symmetric_safe_theme_prop_tile(&scenario.map, pos) {
                        continue;
                    }
                    safe_count += 1;
                    let Some(prop) = symmetric_theme_prop(theme, pos, width, height) else {
                        continue;
                    };
                    count += 1;
                    variants.insert(prop.variant);
                    assert_eq!(authored_tile(&scenario.map, pos), Some(b'.'));
                    let partner = symmetric_theme_prop(theme, mirror, width, height)
                        .expect("symmetric partner");
                    assert_eq!(prop.variant, partner.variant);
                    if pos != mirror {
                        assert_eq!((prop.quarter_turns + 2) % 4, partner.quarter_turns);
                    }
                }
            }
            assert!(
                count * 20 >= safe_count,
                "{} dresses fewer than one in twenty safe tiles ({count}/{safe_count})",
                path.display()
            );
            assert!(
                count * 5 <= safe_count,
                "{} dresses more than one in five safe tiles ({count}/{safe_count})",
                path.display()
            );
            assert!(
                variants.len() >= 3,
                "{} only uses theme-prop variants {variants:?}",
                path.display()
            );
            if safe_count >= 400 {
                assert!(
                    variants.len() >= 8,
                    "{} has room for, but uses too little of the environment library: {variants:?}",
                    path.display()
                );
            }
            variants_by_theme
                .entry(theme.to_string())
                .or_default()
                .extend(variants.iter().copied());
            all_variants.extend(variants);
            total_safe += safe_count;
            total_props += count;
        }
        assert!(
            total_props * 14 >= total_safe && total_props * 8 <= total_safe,
            "shipped maps should average roughly one prop per eleven safe tiles: \
             {total_props}/{total_safe}"
        );
        assert_eq!(
            seen_themes,
            THEMES.map(str::to_string).into_iter().collect()
        );
        for theme in THEMES {
            assert!(
                variants_by_theme
                    .get(theme)
                    .is_some_and(|variants| variants.len() >= 8),
                "{theme} exercises too little of the shared environment library: {:?}",
                variants_by_theme.get(theme)
            );
        }
        assert_eq!(
            all_variants,
            (0..13).collect(),
            "the shipped map set does not exercise the complete environment library"
        );
    }

    #[test]
    fn obstacle_groups_never_cover_non_rock_or_overlap_neighbor_groups() {
        let rows = [
            ".........",
            ".#######.",
            ".#######.",
            ".#######.",
            ".#######.",
            ".........",
        ];
        let is_rock = |pos: TilePos| {
            pos.x >= 0
                && pos.y >= 0
                && rows
                    .get(pos.y as usize)
                    .and_then(|row| row.as_bytes().get(pos.x as usize))
                    == Some(&b'#')
        };
        let mut covered = BTreeSet::new();
        for y in (0..rows.len() as i32).step_by(2) {
            for x in (0..rows[0].len() as i32).step_by(3) {
                let Some(placement) = placement_for_group(TilePos::new(x, y), is_rock) else {
                    continue;
                };
                for dy in 0..placement.footprint.1 {
                    for dx in 0..placement.footprint.0 {
                        let pos = placement.anchor.offset(dx, dy);
                        assert!(is_rock(pos));
                        assert!(covered.insert((pos.x, pos.y)), "overlap at {pos:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn incomplete_fog_knowledge_falls_back_to_individual_rocks() {
        for group_y in (0..20).step_by(2) {
            for group_x in (0..30).step_by(3) {
                let group = TilePos::new(group_x, group_y);
                let Some(_) = placement_for_group(group, |_| true) else {
                    continue;
                };
                assert_eq!(
                    placement_for_group(group, |pos| pos == group),
                    None,
                    "a multi-tile silhouette must not disclose its hidden continuation"
                );
                return;
            }
        }
        panic!("test grid did not find a decorated obstacle group");
    }

    #[test]
    fn shipped_maps_exercise_every_approved_multi_tile_obstacle() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scenarios");
        let mut paths: Vec<_> = std::fs::read_dir(&root)
            .expect("scenario directory")
            .map(|entry| entry.expect("scenario entry").path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        paths.sort();
        let mut rocks = BTreeSet::new();
        let mut industrial = BTreeSet::new();
        for path in paths {
            let scenario = oxide_sim::Scenario::load(&path)
                .unwrap_or_else(|error| panic!("loading {}: {error}", path.display()));
            let height = scenario.map.len() as i32;
            let width = scenario.map.first().expect("scenario row").len() as i32;
            let authored_rock =
                |pos: TilePos| authored_tile(&scenario.map, pos).is_some_and(|cell| cell == b'#');
            for y in (0..height).step_by(2) {
                for x in (0..width).step_by(3) {
                    let Some(placement) = placement_for_group(TilePos::new(x, y), authored_rock)
                    else {
                        continue;
                    };
                    match placement.art {
                        ObstacleArt::Rock(variant) => {
                            rocks.insert(variant);
                        }
                        ObstacleArt::Industrial(variant) => {
                            industrial.insert(variant);
                        }
                    }
                }
            }
        }
        assert_eq!(
            rocks,
            (ONE_TILE_ROCK_COUNT..ONE_TILE_ROCK_COUNT + MULTI_ROCK_FOOTPRINTS.len()).collect()
        );
        assert_eq!(industrial, (0..GROUND_BLOCKER_FOOTPRINTS.len()).collect());
    }
}
