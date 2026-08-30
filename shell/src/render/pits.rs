//! Pit drops: the quarry boundary's descending sibling. Benches step down
//! from a lit lip into darkness so a cut reads as a lower level of the same
//! quarry rather than a hole punched through the floor.
//!
//! The terraces are a distance field over pit tiles measured in quarter-tile
//! cells, so a wide cut carries several benches across tile boundaries before
//! it vanishes. Only standing ground the player has explored can be a rim:
//! unexplored tiles are unknown, so the cut is assumed to continue under the
//! fog and no step is ever invented along a fog line or read from hidden
//! terrain.

use super::environment::{
    Layer, Material, WAVE_LIFTS, draw_fracture, draw_lip, draw_missing_slab, draw_rebar, hash,
    mixed, rgba, shifted,
};
use crate::game::Game;
use chassis::grid::TilePos;
use macroquad::prelude::*;
use oxide_sim::map::Terrain;

const SALT: u32 = 911;
const CELLS: i32 = 4;
const VIGNETTE_CELLS: i32 = 4;
/// Bench widths vary per block of this many tiles so lips jog along the rim
/// the way the boundary's do per side segment.
const BLOCK_TILES: i32 = 3;
const MAX_BENCH_CELLS: i32 = 3;
const MAX_DEPTH: i32 = DROP.len() as i32 * (1 + MAX_BENCH_CELLS) + VIGNETTE_CELLS;
const VOID: Color = rgba(8, 9, 12);
const GLINT: Color = rgba(22, 24, 34);

/// Steeper than the boundary: half-tile benches between risers, each
/// level darker than the last, so the drop fades to the void within a
/// few tiles of the lip.
const DROP: [Layer; 3] = [
    Layer {
        bench_cells: 2,
        riser: rgba(13, 13, 18),
        top: rgba(27, 27, 32),
        lip: rgba(62, 60, 66),
    },
    Layer {
        bench_cells: 2,
        riser: rgba(10, 10, 14),
        top: rgba(20, 20, 24),
        lip: rgba(44, 43, 48),
    },
    Layer {
        bench_cells: 2,
        riser: rgba(8, 8, 12),
        top: rgba(15, 15, 19),
        lip: rgba(31, 30, 35),
    },
];

/// Bench width per level for one block: the base width, one cell narrower
/// about a third of the time, rarely one wider.
fn bench_widths(block_x: i32, block_y: i32) -> [i32; DROP.len()] {
    let mut widths = [0; DROP.len()];
    for (level, layer) in DROP.iter().enumerate() {
        let token = hash(
            block_x + level as i32 * 11,
            block_y + level as i32 * 7,
            SALT,
        ) % 11;
        let shift = match token {
            0..=3 => -1,
            4 => 1,
            _ => 0,
        };
        widths[level] = (layer.bench_cells + shift).clamp(1, MAX_BENCH_CELLS);
    }
    widths
}

fn material(depth: u8, widths: [i32; DROP.len()]) -> Option<Material> {
    if depth == 0 {
        return None;
    }
    let depth = i32::from(depth);
    let mut cursor = 0;
    for (level, width) in widths.iter().enumerate() {
        cursor += 1;
        if depth <= cursor {
            return Some(Material::Riser(level));
        }
        cursor += width;
        if depth <= cursor {
            return Some(Material::Bench(level));
        }
    }
    Some(if depth <= cursor + VIGNETTE_CELLS {
        Material::Vignette((depth - cursor) as u8)
    } else {
        Material::Void
    })
}

fn cell_color(material: Material, cx: i32, cy: i32) -> Color {
    match material {
        Material::Riser(level) => DROP[level].riser,
        Material::Bench(level) => {
            let phase = (cx.div_euclid(CELLS) - cy.div_euclid(CELLS)).rem_euclid(6) as usize;
            shifted(DROP[level].top, WAVE_LIFTS[phase])
        }
        Material::Vignette(step) => mixed(
            DROP[DROP.len() - 1].top,
            VOID,
            f32::from(step) / (VIGNETTE_CELLS as f32 + 1.0),
        ),
        Material::Void => VOID,
    }
}

/// How one tile enters the distance field: `None` seeds a rim (explored
/// standing ground), `Some(known)` continues the cut, known only when the
/// player has explored that pit tile. Off-map positions are the boundary's
/// highwall, never explored floor, so they continue the cut like fog.
fn seed(terrain: Option<Terrain>, explored: bool) -> Option<bool> {
    match terrain {
        None => Some(false),
        Some(Terrain::Pit) => Some(explored),
        Some(_) if explored => None,
        Some(_) => Some(false),
    }
}

/// Chebyshev cell distance from every pit cell to the nearest explored
/// standing cell, over the visible tiles plus enough margin that every
/// drawn cell sees its true nearest rim.
struct PitField {
    origin: TilePos,
    width: i32,
    height: i32,
    depth: Vec<u8>,
    /// Explored pit tiles: the only ones the pass draws.
    known: Vec<bool>,
}

impl PitField {
    fn build(game: &Game, min: TilePos, max: TilePos) -> Self {
        let pad = (MAX_DEPTH + CELLS - 1) / CELLS + 1;
        let origin = TilePos::new(min.x - pad, min.y - pad);
        let width = max.x - min.x + 2 * pad;
        let height = max.y - min.y + 2 * pad;
        let (cw, ch) = (width * CELLS, height * CELLS);
        let mut depth = vec![0u8; (cw * ch).max(0) as usize];
        let mut known = vec![false; (width * height).max(0) as usize];
        let map = game.state.map();
        let vision = game.my_vision();
        let all_seeing = game.all_seeing();
        for ty in 0..height {
            for tx in 0..width {
                let pos = TilePos::new(origin.x + tx, origin.y + ty);
                let terrain = map.tile(pos).map(|tile| tile.terrain);
                let explored = all_seeing || vision.explored(pos);
                let Some(known_pit) = seed(terrain, explored) else {
                    continue;
                };
                known[(ty * width + tx) as usize] = known_pit;
                for sy in 0..CELLS {
                    let row = ((ty * CELLS + sy) * cw + tx * CELLS) as usize;
                    depth[row..row + CELLS as usize].fill(u8::MAX);
                }
            }
        }
        let mut field = Self {
            origin,
            width,
            height,
            depth,
            known,
        };
        field.relax(cw, ch, false);
        field.relax(cw, ch, true);
        field
    }

    /// One chamfer pass with unit weights; forward then backward yields the
    /// exact chessboard distance.
    fn relax(&mut self, cw: i32, ch: i32, backward: bool) {
        let sign = if backward { -1 } else { 1 };
        let neighbors =
            [(-1, -1), (0, -1), (1, -1), (-1, 0)].map(|(dx, dy)| (dx * sign, dy * sign));
        for step_y in 0..ch {
            let y = if backward { ch - 1 - step_y } else { step_y };
            for step_x in 0..cw {
                let x = if backward { cw - 1 - step_x } else { step_x };
                let index = (y * cw + x) as usize;
                if self.depth[index] == 0 {
                    continue;
                }
                let nearest = neighbors
                    .iter()
                    .map(|(dx, dy)| self.cell(x + dx, y + dy))
                    .min()
                    .unwrap_or(u8::MAX);
                self.depth[index] = self.depth[index].min(nearest.saturating_add(1));
            }
        }
    }

    /// Cells beyond the window are unknown, never a rim.
    fn cell(&self, x: i32, y: i32) -> u8 {
        let (cw, ch) = (self.width * CELLS, self.height * CELLS);
        if x < 0 || y < 0 || x >= cw || y >= ch {
            u8::MAX
        } else {
            self.depth[(y * cw + x) as usize]
        }
    }

    fn known_pit(&self, pos: TilePos) -> bool {
        let (tx, ty) = (pos.x - self.origin.x, pos.y - self.origin.y);
        tx >= 0
            && ty >= 0
            && tx < self.width
            && ty < self.height
            && self.known[(ty * self.width + tx) as usize]
    }

    /// Depth of a cell addressed by absolute tile and sub-cell; the sub-cell
    /// may step outside the tile.
    fn at(&self, pos: TilePos, sx: i32, sy: i32) -> u8 {
        self.cell(
            (pos.x - self.origin.x) * CELLS + sx,
            (pos.y - self.origin.y) * CELLS + sy,
        )
    }

    fn material_at(&self, pos: TilePos, sx: i32, sy: i32) -> Option<Material> {
        let tile_x = (pos.x * CELLS + sx).div_euclid(CELLS);
        let tile_y = (pos.y * CELLS + sy).div_euclid(CELLS);
        let widths = bench_widths(
            tile_x.div_euclid(BLOCK_TILES),
            tile_y.div_euclid(BLOCK_TILES),
        );
        material(self.at(pos, sx, sy), widths)
    }
}

/// Screen-space cell edges for one tile, floored so neighbors share exact
/// edges and no hairline opens between cells.
fn cell_edges(game: &Game, pos: TilePos) -> ([f32; 5], [f32; 5]) {
    let mut xs = [0.0; 5];
    let mut ys = [0.0; 5];
    for (i, (x, y)) in xs.iter_mut().zip(ys.iter_mut()).enumerate() {
        let frac = i as f32 / CELLS as f32;
        let screen = game
            .camera
            .to_screen(vec2(pos.x as f32 + frac, pos.y as f32 + frac));
        *x = screen.x.floor();
        *y = screen.y.floor();
    }
    (xs, ys)
}

fn draw_glints(xs: &[f32; 5], ys: &[f32; 5], pos: TilePos, zoom: f32) {
    let value = hash(pos.x, pos.y, SALT + 17);
    if !value.is_multiple_of(3) {
        return;
    }
    let size = (zoom * 0.045).max(1.0);
    let (w, h) = (xs[4] - xs[0], ys[4] - ys[0]);
    for shift in [8, 20] {
        let gx = ((value >> shift) % 14) as f32 + 1.0;
        let gy = ((value >> (shift + 4)) % 14) as f32 + 1.0;
        draw_rectangle(
            xs[0] + w * gx / 16.0,
            ys[0] + h * gy / 16.0,
            size,
            size,
            GLINT,
        );
    }
}

fn draw_fill(game: &Game, field: &PitField, pos: TilePos, zoom: f32) {
    let (xs, ys) = cell_edges(game, pos);
    let color = |sx: i32, sy: i32| {
        let material = field.material_at(pos, sx, sy).unwrap_or(Material::Void);
        cell_color(material, pos.x * CELLS + sx, pos.y * CELLS + sy)
    };
    let all_void = (0..CELLS)
        .flat_map(|sy| (0..CELLS).map(move |sx| (sx, sy)))
        .all(|(sx, sy)| field.material_at(pos, sx, sy) == Some(Material::Void));
    if all_void {
        draw_rectangle(xs[0], ys[0], xs[4] - xs[0], ys[4] - ys[0], VOID);
        draw_glints(&xs, &ys, pos, zoom);
        return;
    }
    for sy in 0..CELLS {
        let mut run_start = 0;
        let mut run_color = color(0, sy);
        for sx in 1..=CELLS {
            let next = (sx < CELLS).then(|| color(sx, sy));
            if next == Some(run_color) {
                continue;
            }
            draw_rectangle(
                xs[run_start as usize],
                ys[sy as usize],
                xs[sx as usize] - xs[run_start as usize],
                ys[sy as usize + 1] - ys[sy as usize],
                run_color,
            );
            if let Some(color) = next {
                run_start = sx;
                run_color = color;
            }
        }
    }
}

fn uniform_bench(field: &PitField, pos: TilePos, sx: i32, sy: i32) -> Option<usize> {
    let Some(Material::Bench(level)) = field.material_at(pos, sx, sy) else {
        return None;
    };
    [(1, 0), (0, 1), (1, 1)]
        .iter()
        .all(|(dx, dy)| field.material_at(pos, sx + dx, sy + dy) == Some(Material::Bench(level)))
        .then_some(level)
}

fn draw_relief(game: &Game, field: &PitField, pos: TilePos, zoom: f32) {
    let (xs, ys) = cell_edges(game, pos);
    let lip = (zoom * 0.055).max(1.0);
    let cell = zoom / CELLS as f32;
    for sy in 0..CELLS {
        for sx in 0..CELLS {
            let depth = field.at(pos, sx, sy);
            let rect = Rect::new(
                xs[sx as usize],
                ys[sy as usize],
                xs[sx as usize + 1] - xs[sx as usize],
                ys[sy as usize + 1] - ys[sy as usize],
            );
            match field.material_at(pos, sx, sy) {
                Some(Material::Riser(level)) => {
                    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                        if field.at(pos, sx + dx, sy + dy) < depth {
                            draw_lip(rect, (dx, dy), lip, DROP[level].lip);
                        }
                    }
                }
                Some(Material::Bench(_)) => {
                    let Some(level) = uniform_bench(field, pos, sx, sy) else {
                        continue;
                    };
                    let value = hash(pos.x * CELLS + sx, pos.y * CELLS + sy, SALT + 503);
                    if value.is_multiple_of(199) {
                        draw_missing_slab(rect, cell, DROP[level].top, value);
                    } else if value % 83 == 3 {
                        draw_fracture(rect, cell, zoom, DROP[level].top, value);
                    } else if value % 211 == 13 {
                        draw_rebar(rect, cell, zoom);
                    }
                }
                Some(Material::Vignette(_) | Material::Void) | None => {}
            }
        }
    }
}

pub(super) fn draw_pits(game: &Game) {
    let (min, max) = super::visible_tiles(game);
    if min.x >= max.x || min.y >= max.y {
        return;
    }
    let field = PitField::build(game, min, max);
    let zoom = game.camera.zoom;
    let pit_tiles: Vec<TilePos> = (min.y..max.y)
        .flat_map(|y| (min.x..max.x).map(move |x| TilePos::new(x, y)))
        .filter(|pos| field.known_pit(*pos))
        .collect();
    // Fills first, then relief: a decal that straddles two tiles must not
    // be buried under the neighbor's fill.
    for pos in &pit_tiles {
        draw_fill(game, &field, *pos, zoom);
    }
    for pos in &pit_tiles {
        draw_relief(game, &field, *pos, zoom);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: [i32; 3] = [2, 2, 2];

    #[test]
    fn depth_walks_lip_bench_vignette_void_in_order() {
        let mut seen = Vec::new();
        for depth in 1..=(MAX_DEPTH as u8 + 1) {
            let material = material(depth, BASE).expect("pit cell");
            if seen.last() != Some(&material) {
                seen.push(material);
            }
        }
        assert_eq!(
            seen,
            [
                Material::Riser(0),
                Material::Bench(0),
                Material::Riser(1),
                Material::Bench(1),
                Material::Riser(2),
                Material::Bench(2),
                Material::Vignette(1),
                Material::Vignette(2),
                Material::Vignette(3),
                Material::Vignette(4),
                Material::Void,
            ]
        );
        assert_eq!(material(0, BASE), None);
    }

    #[test]
    fn each_level_descends_toward_the_void() {
        let mut previous: Option<Layer> = None;
        for layer in DROP {
            assert!(layer.riser.r < layer.top.r);
            assert!(layer.top.r < layer.lip.r);
            if let Some(above) = previous {
                assert!(layer.top.r < above.top.r);
                assert!(layer.lip.r < above.lip.r);
            }
            previous = Some(layer);
        }
        assert!(VOID.r < DROP[DROP.len() - 1].top.r);
    }

    #[test]
    fn bench_widths_jog_within_bounds_and_never_exceed_the_padding() {
        let mut distinct = std::collections::BTreeSet::new();
        for by in -8..8 {
            for bx in -8..8 {
                let widths = bench_widths(bx, by);
                assert!(widths.iter().all(|w| (1..=MAX_BENCH_CELLS).contains(w)));
                let depth: i32 = widths.iter().map(|w| 1 + w).sum::<i32>() + VIGNETTE_CELLS;
                assert!(depth <= MAX_DEPTH);
                distinct.insert(widths);
            }
        }
        assert!(distinct.len() > 1, "the rim never jogs");
    }

    fn flat_field(cells: i32) -> PitField {
        PitField {
            origin: TilePos::new(0, 0),
            width: cells / CELLS,
            height: cells / CELLS,
            depth: vec![u8::MAX; (cells * cells) as usize],
            known: vec![true; ((cells / CELLS) * (cells / CELLS)) as usize],
        }
    }

    #[test]
    fn chessboard_distance_is_exact_from_a_flat_field() {
        let mut field = flat_field(12);
        // A single standing cell in the middle; everything else is pit.
        field.depth[6 * 12 + 6] = 0;
        field.relax(12, 12, false);
        field.relax(12, 12, true);
        for y in 0..12i32 {
            for x in 0..12i32 {
                let expected = (x - 6).abs().max((y - 6).abs()) as u8;
                assert_eq!(field.cell(x, y), expected, "cell {x},{y}");
            }
        }
    }

    #[test]
    fn only_explored_standing_ground_seeds_a_rim() {
        assert_eq!(seed(Some(Terrain::Ground), true), None);
        assert_eq!(seed(Some(Terrain::Rock), true), None);
        assert_eq!(seed(Some(Terrain::Peak), true), None);
        assert_eq!(seed(Some(Terrain::Ground), false), Some(false));
        assert_eq!(seed(Some(Terrain::Pit), false), Some(false));
        assert_eq!(seed(Some(Terrain::Pit), true), Some(true));
        assert_eq!(
            seed(None, true),
            Some(false),
            "off-map is highwall, not floor"
        );
    }

    #[test]
    fn unknown_ground_never_becomes_a_rim() {
        // Unexplored tiles enter the field exactly like pit: with no explored
        // standing cell anywhere, every cell stays void.
        let mut field = flat_field(12);
        field.relax(12, 12, false);
        field.relax(12, 12, true);
        assert!(field.depth.iter().all(|depth| *depth == u8::MAX));
    }
}
