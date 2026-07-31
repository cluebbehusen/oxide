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

/// The crown sprite for a skyline/lone peak tile, or `None` for an
/// interior wall tile. Connectivity picks the art, and the neighbor
/// probe is fog-honest: an unexplored neighbor is unknown, not absent
/// — reading its live terrain would let a known peak's edge art
/// disclose whether the ridge continues under fog. Never flipped;
/// a flip would break the chained edge profiles.
fn peak_crown(game: &Game, sprites: &Sprites, x: i32, y: i32, h: usize) -> Option<Rect> {
    let peaky = |dx: i32, dy: i32| {
        let pos = TilePos::new(x + dx, y + dy);
        (game.all_seeing() || game.my_vision().explored(pos))
            && game
                .state
                .map()
                .tile(pos)
                .is_some_and(|t| t.terrain == oxide_sim::map::Terrain::Peak)
    };
    if peaky(0, -1) {
        None
    } else if !peaky(-1, 0) && !peaky(1, 0) && !peaky(0, 1) {
        Some(sprites.peak_lone(h % 2))
    } else {
        Some(sprites.peak_sky(peaky(-1, 0), peaky(1, 0), h % 2))
    }
}

/// The skyline pass: crown sprites are 1x1.5 tiles, anchored half a
/// tile ABOVE their own — machines on the tile behind the ridge
/// disappear behind the crests, which is the whole point of a wall
/// that owns its column of sky. Fog-gated on the crown's OWN tile:
/// an unexplored peak draws nothing at all, so its overhang can never
/// leak into a visible neighbor (the fog rects only cover tiles, not
/// sprite footprints).
pub(crate) fn draw_peak_crowns(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    let size = zoom.ceil() + 1.0;
    let tint = theme_tint(
        game.scenario
            .meta
            .as_ref()
            .map(|m| m.theme.as_str())
            .unwrap_or(""),
    );
    let (min, max) = visible_tiles(game);
    // One row of slack above the window: a crown anchored just off
    // the top edge still hangs into view.
    for y in min.y..(max.y + 1) {
        for x in min.x..max.x {
            let pos = TilePos::new(x, y);
            let Some(tile) = game.state.map().tile(pos) else {
                continue;
            };
            if tile.terrain != oxide_sim::map::Terrain::Peak {
                continue;
            }
            if !(game.all_seeing() || game.my_vision().explored(pos)) {
                continue;
            }
            let h = (x.wrapping_mul(31).wrapping_add(y.wrapping_mul(17))) as usize;
            let Some(source) = peak_crown(game, sprites, x, y, h) else {
                continue;
            };
            let screen = game.camera.to_screen(vec2(x as f32, y as f32));
            draw_texture_ex(
                sprites.texture(),
                screen.x.floor(),
                (screen.y - zoom * 0.5).floor(),
                tint,
                DrawTextureParams {
                    dest_size: Some(vec2(size, size * 1.5)),
                    source: Some(source),
                    ..Default::default()
                },
            );
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ThemePropPlacement {
    variant: usize,
    quarter_turns: u8,
}

fn theme_code(theme: &str) -> Option<u32> {
    match theme {
        "rusted-yard" => Some(1),
        "cold-circuitry" => Some(2),
        "quarry-dust" => Some(3),
        "basalt" => Some(4),
        "slag" => Some(5),
        "verdigris" => Some(6),
        _ => None,
    }
}

/// Sparse dressing picked from a coordinate and its 180-degree partner.
/// Both halves therefore choose the same art; the far half adds a half-turn
/// so directional marks preserve the shipped maps' visual symmetry.
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
    // Match the old generic-decal density, but make every mark belong to
    // the map instead of sharing one universal scatter.
    if !hash.is_multiple_of(23) {
        return None;
    }
    let variant = (hash / 23 % 3) as usize;
    let base_turns = (hash / (23 * 3) % 4) as u8;
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
                    sprites.theme_prop(theme, placement.variant).map(|source| {
                        (
                            source,
                            f32::from(placement.quarter_turns) * std::f32::consts::FRAC_PI_2,
                            WHITE,
                        )
                    })
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
                (oxide_sim::map::Terrain::Rock, _) => (Some(sprites.rock(h % 4)), h % 7 < 3),
                (oxide_sim::map::Terrain::Peak, _) => {
                    // Interior wall tiles draw here; the skyline rows
                    // (crowns) draw in their own pass after units, so
                    // their overhang can occlude what stands behind
                    // the ridge. See draw_peak_crowns.
                    if peak_crown(game, sprites, x, y, h).is_some() {
                        (None, false)
                    } else {
                        (Some(sprites.peak_body(h % 2)), false)
                    }
                }
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
    use std::collections::BTreeSet;

    const THEMES: [&str; 6] = [
        "rusted-yard",
        "cold-circuitry",
        "quarry-dust",
        "basalt",
        "slag",
        "verdigris",
    ];

    #[test]
    fn theme_prop_selection_is_a_rotated_180_degree_pair() {
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
                    let Some(prop) = symmetric_theme_prop(theme, pos, width, height) else {
                        continue;
                    };
                    count += 1;
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
                count > 0,
                "{} received no deterministic theme props",
                path.display()
            );
        }
        assert_eq!(
            seen_themes,
            THEMES.map(str::to_string).into_iter().collect()
        );
    }
}
