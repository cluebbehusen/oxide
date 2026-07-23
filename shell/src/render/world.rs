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

pub(crate) fn draw_tiles(game: &Game, sprites: &Sprites) {
    let zoom = game.camera.zoom;
    let size = zoom.ceil() + 1.0; // slight overlap kills seam hairlines
    let tint = theme_tint(
        game.scenario
            .meta
            .as_ref()
            .map(|m| m.theme.as_str())
            .unwrap_or(""),
    );
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
            // Ground dressing: rubble tiles always get wreckage; plain
            // ground gets a sparse scatter (~4%) of the smaller decals.
            let decal = if tile.cosmetic == 1 {
                Some(sprites.decal(3))
            } else if tile.terrain == oxide_sim::map::Terrain::Ground && h.is_multiple_of(23) {
                Some(sprites.decal(h / 23 % 3))
            } else {
                None
            };
            if let Some(source) = decal {
                draw_texture_ex(
                    sprites.texture(),
                    screen.x.floor(),
                    screen.y.floor(),
                    tint,
                    DrawTextureParams {
                        dest_size: Some(vec2(size, size)),
                        source: Some(source),
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
            let pos = TilePos::new(x, y);
            let scrap = if game.all_seeing() || game.my_vision().visible(pos) {
                tile.scrap
            } else {
                game.my_vision().remembered_scrap(pos)
            };
            // Wrecks follow the same sight rule; a live node or rock
            // outranks the junk visually.
            let wreck = if game.all_seeing() || game.my_vision().visible(pos) {
                tile.wreck
            } else {
                game.my_vision().remembered_wreck(pos)
            };
            let (overlay, flip) = match (tile.terrain, scrap) {
                (oxide_sim::map::Terrain::Rock, _) => (Some(sprites.rock(h % 4)), h % 7 < 3),
                (oxide_sim::map::Terrain::Peak, _) => {
                    // Connectivity picks the art: interior wall tiles
                    // read as solid rock, the skyline row carries the
                    // crests, and connected edges share a fixed profile
                    // so ridges join without seams. Never flipped —
                    // a flip would break those joins.
                    let peaky = |dx: i32, dy: i32| {
                        let pos = TilePos::new(x + dx, y + dy);
                        // An unexplored neighbor is unknown, not absent:
                        // reading its live terrain would let a known
                        // peak's edge art disclose whether the ridge
                        // continues under fog.
                        (game.all_seeing() || game.my_vision().explored(pos))
                            && game
                                .state
                                .map()
                                .tile(pos)
                                .is_some_and(|t| t.terrain == oxide_sim::map::Terrain::Peak)
                    };
                    let source = if peaky(0, -1) {
                        sprites.peak_body(h % 2)
                    } else if !peaky(-1, 0) && !peaky(1, 0) && !peaky(0, 1) {
                        sprites.peak_lone(h % 2)
                    } else {
                        sprites.peak_sky(peaky(-1, 0), peaky(1, 0), h % 2)
                    };
                    (Some(source), false)
                }
                (_, 0) if wreck > 0 => (Some(sprites.wreck_pile()), h % 5 < 2),
                (_, 0) => (None, false),
                (_, s) => (Some(sprites.scrap(s, SCRAP_NODE_AMOUNT)), false),
            };
            if let Some(source) = overlay {
                draw_texture_ex(
                    sprites.texture(),
                    screen.x.floor(),
                    screen.y.floor(),
                    tint,
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
