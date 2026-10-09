//! Ground layers: terrain tiles (fog-ruled), the fog veil, and battle
//! scars.

use super::*;
use crate::numeric;
use crate::numeric::Fit;
use chassis::grid::as_index;

/// Share of a tile's width over which the veil fades into lighter ground.
const FOG_FEATHER: f32 = 0.45;

/// Fog of war from the local player's perspective: unexplored is void,
/// explored-but-unseen is dimmed.
pub(crate) fn draw_fog(game: &crate::game::Scene<'_>) {
    let (lo, hi) = game.presentation.camera.world_rect();
    let min = numeric::tile_at(lo);
    let max = TilePos::new(numeric::to_i32(hi.x.ceil()), numeric::to_i32(hi.y.ceil()));
    // One extra ring so every drawn tile sees all eight neighbors.
    let stride = as_index(max.x - min.x + 2);
    let states: Vec<Fog> = (min.y - 1..=max.y)
        .flat_map(|y| (min.x - 1..=max.x).map(move |x| tile_fog(game, TilePos::new(x, y))))
        .collect();
    let fog = |x: i32, y: i32| states[as_index(y - min.y + 1) * stride + as_index(x - min.x + 1)];
    for y in min.y..max.y {
        for x in min.x..max.x {
            let alpha = fog(x, y).alpha();
            if alpha == 0.0 {
                continue;
            }
            let cover = Color::new(FOG_UNEXPLORED.r, FOG_UNEXPLORED.g, FOG_UNEXPLORED.b, alpha);
            // Exact shared edges: translucent rects that overlap draw
            // double-dark seams, so each tile ends where the next begins.
            let a = game
                .presentation
                .camera
                .to_screen(vec2(x as f32, y as f32))
                .floor();
            let b = game
                .presentation
                .camera
                .to_screen(vec2((x + 1) as f32, (y + 1) as f32))
                .floor();
            draw_rectangle(a.x, a.y, b.x - a.x, b.y - a.y, cover);
        }
    }
    // Feather inward into known ground. Unknown tiles retain their opaque veil.
    for y in min.y..max.y {
        for x in min.x..max.x {
            let neighborhood: [[Fog; 3]; 3] = std::array::from_fn(|row| {
                std::array::from_fn(|col| fog(x + col.fit::<i32>() - 1, y + row.fit::<i32>() - 1))
            });
            let current = neighborhood[1][1];
            if current == Fog::Unexplored
                || neighborhood.iter().flatten().all(|&state| state <= current)
            {
                continue;
            }
            let a = game
                .presentation
                .camera
                .to_screen(vec2(x as f32, y as f32))
                .floor();
            let b = game
                .presentation
                .camera
                .to_screen(vec2((x + 1) as f32, (y + 1) as f32))
                .floor();
            draw_fog_feather(a, b, &neighborhood);
        }
    }
}

/// Samples [`fog_feather_alpha`] on a grid over one tile's screen rect.
fn draw_fog_feather(a: Vec2, b: Vec2, neighborhood: &[[Fog; 3]; 3]) {
    let band = (b - a).max_element() * FOG_FEATHER;
    let steps = numeric::to_usize((band / 6.0).ceil().clamp(1.0, 8.0));
    // Grid lines fall on the feather band boundaries so straight fades
    // interpolate exactly; only the rounded corners are approximated.
    let coords: Vec<f32> = (0..=steps)
        .map(|i| FOG_FEATHER * i as f32 / steps as f32)
        .chain((0..=steps).map(|i| 1.0 - FOG_FEATHER + FOG_FEATHER * i as f32 / steps as f32))
        .collect();
    let side = coords.len();
    let vertices = coords
        .iter()
        .flat_map(|&v| {
            coords.iter().map(move |&u| {
                let p = a + (b - a) * vec2(u, v);
                let alpha = fog_feather_alpha(neighborhood, u, v);
                let color = Color::new(FOG_UNEXPLORED.r, FOG_UNEXPLORED.g, FOG_UNEXPLORED.b, alpha);
                Vertex::new(p.x, p.y, 0.0, 0.0, 0.0, color)
            })
        })
        .collect();
    let indices = (0..side - 1)
        .flat_map(|row| {
            (0..side - 1).flat_map(move |col| {
                let top = (row * side + col).fit::<u16>();
                let bottom = top + side.fit::<u16>();
                [top, top + 1, bottom + 1, top, bottom + 1, bottom]
            })
        })
        .collect();
    draw_mesh(&Mesh {
        vertices,
        indices,
        texture: None,
    });
}

/// Veil alpha layered over a known tile's own cover at tile-local
/// `(u, v)`. `neighborhood[row][col]` holds fog states with this tile at
/// the center.
///
/// The veil blends a fade toward hidden ground with a fade toward
/// unexplored ground. Neither depends on the drawing tile's own state, so
/// neighbors agree along every shared edge, including where visible,
/// explored, and unexplored ground meet.
fn fog_feather_alpha(neighborhood: &[[Fog; 3]; 3], u: f32, v: f32) -> f32 {
    let current = neighborhood[1][1].alpha();
    if current >= 1.0 {
        return 0.0;
    }
    let explored = Fog::Explored.alpha();
    let occlusion = explored * fog_reach(neighborhood, u, v, Fog::Explored)
        + (1.0 - explored) * fog_reach(neighborhood, u, v, Fog::Unexplored);
    ((occlusion - current) / (1.0 - current)).max(0.0)
}

/// How strongly tiles at least as fogged as `floor` reach tile-local
/// `(u, v)`: fully on such a tile, fading with distance from one, and
/// rounded off where two such sides meet so the corner does not jut.
fn fog_reach(neighborhood: &[[Fog; 3]; 3], u: f32, v: f32, floor: Fog) -> f32 {
    let fogged = |row: usize, col: usize| neighborhood[row][col] >= floor;
    let gap = |index: usize, offset: f32| match index {
        0 => offset,
        1 => 0.0,
        _ => 1.0 - offset,
    };
    let mut reach = 0.0_f32;
    for row in 0..3 {
        for col in 0..3 {
            if fogged(row, col) {
                let distance = gap(col, u).hypot(gap(row, v));
                reach = reach.max((1.0 - distance / FOG_FEATHER).clamp(0.0, 1.0));
            }
        }
    }
    for (row, col) in [(0, 0), (0, 2), (2, 0), (2, 2)] {
        if fogged(1, col) && fogged(row, 1) {
            let inset = (FOG_FEATHER - gap(col, u))
                .max(0.0)
                .hypot((FOG_FEATHER - gap(row, v)).max(0.0));
            reach = reach.max((inset / FOG_FEATHER).min(1.0));
        }
    }
    reach
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Fog {
    Visible,
    Explored,
    Unexplored,
}

impl Fog {
    fn alpha(self) -> f32 {
        match self {
            Fog::Visible => 0.0,
            Fog::Explored => FOG_EXPLORED.a,
            Fog::Unexplored => 1.0,
        }
    }
}

fn tile_fog(game: &crate::game::Scene<'_>, tile: TilePos) -> Fog {
    let (explored, visible) = if game.state.map().tile(tile).is_some() {
        (
            game.my_vision().explored(tile),
            game.my_vision().visible(tile),
        )
    } else {
        (
            game.presentation.boundary_fog.explored(tile),
            game.presentation.boundary_fog.visible(tile),
        )
    };
    if !explored {
        Fog::Unexplored
    } else if !visible {
        Fog::Explored
    } else {
        Fog::Visible
    }
}

/// Fog-honest peak connectivity. An explored barrier cannot disclose that
/// its wall continues into an unexplored neighbor merely through edge art.
fn peak_neighbor_mask(game: &crate::game::Scene<'_>, pos: TilePos) -> u8 {
    [(0, -1, 1), (1, 0, 2), (0, 1, 4), (-1, 0, 8)]
        .into_iter()
        .fold(0, |mask, (dx, dy, bit)| {
            let neighbor = pos.offset(dx, dy);
            let known = game.presentation.all_seeing() || game.my_vision().explored(neighbor);
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
    for word in [x.cast_unsigned(), y.cast_unsigned(), salt] {
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
    // resolve into one cluster or abandoned machine footprint.
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
                group.x
                    + ((hash / 37 + step.fit::<u32>()) % (x_slack.cast_unsigned() + 1))
                        .cast_signed(),
                group.y
                    + ((hash / 71 + step.fit::<u32>()) % (y_slack.cast_unsigned() + 1))
                        .cast_signed(),
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

fn visible_obstacle(game: &crate::game::Scene<'_>, group: TilePos) -> Option<ObstaclePlacement> {
    placement_for_group(group, |pos| {
        let known = game.presentation.all_seeing() || game.my_vision().explored(pos);
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
        canonical.x.cast_unsigned(),
        canonical.y.cast_unsigned(),
        width.cast_unsigned(),
        height.cast_unsigned(),
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
    rows.get(as_index(pos.y))?
        .as_bytes()
        .get(as_index(pos.x))
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
    let height = rows.len().fit::<i32>();
    let width = rows
        .first()
        .map_or(0, std::string::String::len)
        .fit::<i32>();
    let mirror = TilePos::new(width - 1 - pos.x, height - 1 - pos.y);
    safe_theme_prop_tile(rows, pos) && safe_theme_prop_tile(rows, mirror)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct QuarryDressing {
    variant: usize,
    rotation: f32,
    offset: Vec2,
    scale: f32,
}

fn quarry_dressing(pos: TilePos, width: i32, height: i32, seed: u64) -> Option<QuarryDressing> {
    let mirror = TilePos::new(width - 1 - pos.x, height - 1 - pos.y);
    if width <= 0 || height <= 0 || pos == mirror {
        return None;
    }
    let mirrored = (pos.y, pos.x) > (mirror.y, mirror.x);
    let canonical = if mirrored { mirror } else { pos };
    let salt = (seed & 0xFFFF_FFFF) as u32
        ^ (seed >> 32) as u32
        ^ width.cast_unsigned().rotate_left(9)
        ^ height.cast_unsigned().rotate_left(19);
    let mut token = super::environment::hash(canonical.x, canonical.y, salt);
    token ^= token >> 16;
    token = token.wrapping_mul(0x7feb_352d);
    token ^= token >> 15;
    token = token.wrapping_mul(0x846c_a68b);
    token ^= token >> 16;
    if !token.is_multiple_of(19) {
        return None;
    }
    let direction = if mirrored { -1.0 } else { 1.0 };
    Some(QuarryDressing {
        variant: (token / 19 % 12) as usize,
        rotation: (((token >> 12) % 4) as f32 + if mirrored { 2.0 } else { 0.0 })
            * std::f32::consts::FRAC_PI_2,
        offset: vec2(
            ((token >> 17) % 17) as f32 - 8.0,
            ((token >> 22) % 17) as f32 - 8.0,
        ) * (direction / 64.0),
        scale: 0.72 + ((token >> 27) % 8) as f32 * 0.035,
    })
}

pub(crate) fn draw_tiles(game: &crate::game::Scene<'_>, sprites: &Sprites) {
    let zoom = game.presentation.camera.zoom;
    let size = zoom.ceil() + 1.0; // slight overlap kills seam hairlines
    let theme = game.scenario.meta.as_ref().map_or("", |m| m.theme.as_str());
    let tint = theme_tint(theme);
    let themed = theme_code(theme).is_some();
    let map_width = game.state.map().width();
    // Quarry props vary by map, not by anything in play.
    let quarry_salt = chassis::hash::fnv1a(game.scenario.name.as_bytes());
    let map_height = game.state.map().height();
    let (min, max) = visible_tiles(game);
    for y in min.y..max.y {
        for x in min.x..max.x {
            let Some(tile) = game.state.map().tile(TilePos::new(x, y)) else {
                continue;
            };
            let screen = game.presentation.camera.to_screen(vec2(x as f32, y as f32));
            // Position hashes drive all variety: deterministic, no state.
            let h = x
                .wrapping_mul(31)
                .wrapping_add(y.wrapping_mul(17))
                .cast_unsigned() as usize;
            // The linear hash deliberately steps shades in diagonal waves.
            // Grit and cracks use a mixed hash so no shade repeats one texture;
            // FNV's low bits track coordinate parity, so read its high bits.
            let variant = h % 6;
            let detail = (coordinate_hash(x, y, 0x4752_4e44) >> 16) as usize;
            let next = game
                .presentation
                .camera
                .to_screen(vec2((x + 1) as f32, (y + 1) as f32));
            let tile_size = vec2(
                next.x.floor() - screen.x.floor(),
                next.y.floor() - screen.y.floor(),
            );
            sprites.draw(
                screen.x.floor(),
                screen.y.floor(),
                tint,
                DrawTextureParams {
                    dest_size: Some(tile_size),
                    source: Some(sprites.ground(variant)),
                    ..Default::default()
                },
            );
            let detail_count = sprites.ground_detail_count();
            let orientation = detail / detail_count;
            sprites.draw(
                screen.x.floor(),
                screen.y.floor(),
                tint,
                DrawTextureParams {
                    dest_size: Some(tile_size),
                    source: Some(sprites.ground_detail(detail)),
                    flip_x: orientation & 1 == 1,
                    flip_y: orientation & 2 == 2,
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
            let quarry = sprites.quarry_dressing(0).is_some();
            let placement = quarry
                .then(|| quarry_dressing(pos, map_width, map_height, quarry_salt))
                .flatten()
                .filter(|_| {
                    tile.cosmetic != 1
                        && tile.terrain == oxide_sim::map::Terrain::Ground
                        && symmetric_safe_theme_prop_tile(&game.scenario.map, pos)
                });
            let dressing = if let Some(placement) = placement {
                sprites
                    .quarry_dressing(placement.variant)
                    .map(|source| (source, placement.rotation, tint))
            } else if tile.cosmetic == 1 {
                Some((sprites.decal(3), 0.0, tint))
            } else if quarry {
                None
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
                let (offset, scale) = placement.map_or((Vec2::ZERO, 1.0), |p| (p.offset, p.scale));
                let origin = screen + (Vec2::splat((1.0 - scale) * 0.5) + offset) * zoom;
                sprites.draw(
                    origin.x.floor(),
                    origin.y.floor(),
                    dressing_tint,
                    DrawTextureParams {
                        dest_size: Some(vec2(size * scale, size * scale)),
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
                        sprites.draw(
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
            let seen_now = game.presentation.all_seeing() || game.my_vision().visible(pos);
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
                super::resource_memory_opacity(game, pos)
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
                // Pit terraces are drawn by the environment pass after the tiles.
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
                sprites.draw(
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
                .presentation
                .camera
                .to_screen(vec2(placement.anchor.x as f32, placement.anchor.y as f32));
            sprites.draw(
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
pub(crate) fn draw_extractor_frames(game: &crate::game::Scene<'_>, sprites: &Sprites) {
    let zoom = game.presentation.camera.zoom;
    for &frame in game.state.map().extractor_frames() {
        if !crate::strategic_markers::extractor_frame_visible(game, frame) {
            continue;
        }
        let screen = game
            .presentation
            .camera
            .to_screen(vec2(frame.x as f32, frame.y as f32));
        sprites.draw(
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
pub(crate) fn draw_scorches(game: &crate::game::Scene<'_>, sprites: &Sprites) {
    let zoom = game.presentation.camera.zoom;
    for (at, age) in &game.presentation.scorches {
        let alpha = (1.0 - age / 20.0).clamp(0.0, 1.0) * 0.85;
        let size = zoom * 2.4;
        let screen = game.presentation.camera.to_screen(*at);
        sprites.draw(
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
mod tests;
