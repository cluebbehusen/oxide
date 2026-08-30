//! Production quarry boundary: collapsed, dark industrial terraces rising
//! away from the battlefield floor.

use crate::game::Game;
use macroquad::prelude::*;

const SALT: u32 = 347;
pub(super) const WAVE_LIFTS: [i16; 6] = [-4, -2, 0, 1, 2, 4];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BoundaryInsets {
    top: bool,
    right: bool,
    bottom: bool,
    left: bool,
}

impl BoundaryInsets {
    fn from_rows(rows: &[String]) -> Self {
        let width = rows.first().map_or(0, String::len);
        let blocked = |cell: u8| matches!(cell, b'#' | b'^');
        Self {
            top: rows
                .first()
                .is_some_and(|row| !row.is_empty() && row.bytes().all(blocked)),
            right: width > 0
                && rows
                    .iter()
                    .all(|row| row.as_bytes().get(width - 1).copied().is_some_and(blocked)),
            bottom: rows
                .last()
                .is_some_and(|row| !row.is_empty() && row.bytes().all(blocked)),
            left: width > 0
                && rows
                    .iter()
                    .all(|row| row.as_bytes().first().copied().is_some_and(blocked)),
        }
    }
}

#[derive(Clone, Copy)]
struct MapFrame {
    rect: Rect,
    tile: f32,
}

impl MapFrame {
    fn from_game(game: &Game) -> Self {
        let origin = game.camera.to_screen(vec2(0.0, 0.0));
        let far = game.camera.to_screen(vec2(
            game.state.map().width() as f32,
            game.state.map().height() as f32,
        ));
        let tile = game.camera.zoom;
        let mut rect = Rect::new(
            origin.x.floor(),
            origin.y.floor(),
            far.x.ceil() - origin.x.floor(),
            far.y.ceil() - origin.y.floor(),
        );
        let insets = BoundaryInsets::from_rows(&game.scenario.map);
        if insets.left {
            rect.x += tile;
            rect.w -= tile;
        }
        if insets.right {
            rect.w -= tile;
        }
        if insets.top {
            rect.y += tile;
            rect.h -= tile;
        }
        if insets.bottom {
            rect.h -= tile;
        }
        Self { rect, tile }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Material {
    Riser(usize),
    Bench(usize),
    Vignette(u8),
    Void,
}

#[derive(Clone, Copy)]
pub(super) struct Layer {
    pub(super) bench_cells: i32,
    pub(super) riser: Color,
    pub(super) top: Color,
    pub(super) lip: Color,
}

pub(super) const fn rgba(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgba(r, g, b, 255)
}

const LAYERS: [Layer; 3] = [
    Layer {
        bench_cells: 4,
        riser: rgba(24, 25, 30),
        top: rgba(52, 52, 56),
        lip: rgba(69, 67, 73),
    },
    Layer {
        bench_cells: 5,
        riser: rgba(28, 28, 34),
        top: rgba(55, 54, 59),
        lip: rgba(74, 71, 77),
    },
    Layer {
        bench_cells: 3,
        riser: rgba(32, 31, 37),
        top: rgba(58, 56, 62),
        lip: rgba(79, 74, 80),
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

struct TerraceField {
    rect: Rect,
    tile: f32,
    cell: f32,
    inner_width: i32,
    inner_height: i32,
}

impl TerraceField {
    fn new(frame: MapFrame) -> Self {
        let cell = frame.tile / 4.0;
        Self {
            rect: frame.rect,
            tile: frame.tile,
            cell,
            inner_width: (frame.rect.w / cell).round() as i32,
            inner_height: (frame.rect.h / cell).round() as i32,
        }
    }

    fn max_depth(&self) -> i32 {
        LAYERS
            .iter()
            .map(|layer| 1 + layer.bench_cells + 1)
            .sum::<i32>()
            + 6
    }

    fn outside(&self, ix: i32, iy: i32) -> bool {
        ix < 0 || iy < 0 || ix >= self.inner_width || iy >= self.inner_height
    }

    fn depth(&self, ix: i32, iy: i32) -> i32 {
        let x = if ix < 0 {
            -ix
        } else if ix >= self.inner_width {
            ix - self.inner_width + 1
        } else {
            0
        };
        let y = if iy < 0 {
            -iy
        } else if iy >= self.inner_height {
            iy - self.inner_height + 1
        } else {
            0
        };
        x.max(y)
    }

    fn side_segment(&self, ix: i32, iy: i32) -> Option<(Side, i32)> {
        let (side, along, length) = if iy < 0 && (0..self.inner_width).contains(&ix) {
            (Side::Top, ix, self.inner_width)
        } else if ix >= self.inner_width && (0..self.inner_height).contains(&iy) {
            (Side::Right, iy, self.inner_height)
        } else if iy >= self.inner_height && (0..self.inner_width).contains(&ix) {
            (Side::Bottom, self.inner_width - 1 - ix, self.inner_width)
        } else if ix < 0 && (0..self.inner_height).contains(&iy) {
            (Side::Left, self.inner_height - 1 - iy, self.inner_height)
        } else {
            return None;
        };
        Some((side, (along * 6 / length.max(1)).clamp(0, 5)))
    }

    fn bench_cells(&self, layer: usize, segment: Option<(Side, i32)>) -> i32 {
        let Some((side, segment)) = segment else {
            return LAYERS[layer].bench_cells;
        };
        let token = hash(
            segment + layer as i32 * 11,
            side as i32 + layer as i32 * 7,
            SALT,
        ) % 11;
        (LAYERS[layer].bench_cells
            + match token {
                0..=3 => -1,
                4 => 1,
                _ => 0,
            })
        .clamp(3, 5)
    }

    fn material(&self, ix: i32, iy: i32) -> Material {
        if !self.outside(ix, iy) {
            return Material::Void;
        }
        let depth = self.depth(ix, iy);
        let segment = self.side_segment(ix, iy);
        let mut cursor = 0;
        for layer in 0..LAYERS.len() {
            cursor += 1;
            if depth <= cursor {
                return Material::Riser(layer);
            }
            cursor += self.bench_cells(layer, segment);
            if depth <= cursor {
                return Material::Bench(layer);
            }
        }
        if depth <= cursor + 4 {
            Material::Vignette((depth - cursor) as u8)
        } else {
            Material::Void
        }
    }

    fn rect(&self, ix: i32, iy: i32) -> Rect {
        Rect::new(
            self.rect.x + ix as f32 * self.cell,
            self.rect.y + iy as f32 * self.cell,
            self.cell,
            self.cell,
        )
    }

    fn visible_ranges(&self) -> (std::ops::Range<i32>, std::ops::Range<i32>) {
        let pad = self.max_depth();
        let left = (((-self.rect.x) / self.cell).floor() as i32 - 1).max(-pad);
        let right = (((screen_width() - self.rect.x) / self.cell).ceil() as i32 + 1)
            .min(self.inner_width + pad);
        let top = (((-self.rect.y) / self.cell).floor() as i32 - 1).max(-pad);
        let bottom = (((screen_height() - self.rect.y) / self.cell).ceil() as i32 + 1)
            .min(self.inner_height + pad);
        (left..right, top..bottom)
    }
}

pub(super) fn shifted(color: Color, amount: i16) -> Color {
    let amount = amount as f32 / 255.0;
    Color::new(
        (color.r + amount).clamp(0.0, 1.0),
        (color.g + amount).clamp(0.0, 1.0),
        (color.b + amount).clamp(0.0, 1.0),
        color.a,
    )
}

pub(super) fn mixed(left: Color, right: Color, amount: f32) -> Color {
    Color::new(
        left.r + (right.r - left.r) * amount,
        left.g + (right.g - left.g) * amount,
        left.b + (right.b - left.b) * amount,
        left.a + (right.a - left.a) * amount,
    )
}

fn cell_color(material: Material, ix: i32, iy: i32) -> Option<Color> {
    match material {
        Material::Riser(layer) => Some(LAYERS[layer].riser),
        Material::Bench(layer) => {
            let phase = (ix.div_euclid(4) - iy.div_euclid(4)).rem_euclid(6) as usize;
            Some(shifted(LAYERS[layer].top, WAVE_LIFTS[phase]))
        }
        Material::Vignette(step) => Some(mixed(
            LAYERS.last().expect("terrace layers").top,
            BLACK,
            (0.25 + f32::from(step) / 4.0 * 0.72).min(0.97),
        )),
        Material::Void => None,
    }
}

pub(super) fn draw_lip(rect: Rect, direction: (i32, i32), thickness: f32, color: Color) {
    match direction {
        (-1, 0) => draw_rectangle(rect.x, rect.y, thickness, rect.h, color),
        (1, 0) => draw_rectangle(
            rect.x + rect.w - thickness,
            rect.y,
            thickness,
            rect.h,
            color,
        ),
        (0, -1) => draw_rectangle(rect.x, rect.y, rect.w, thickness, color),
        (0, 1) => draw_rectangle(
            rect.x,
            rect.y + rect.h - thickness,
            rect.w,
            thickness,
            color,
        ),
        _ => unreachable!("terrace lips are cardinal"),
    }
}

fn uniform_bench(field: &TerraceField, ix: i32, iy: i32) -> Option<usize> {
    let Material::Bench(level) = field.material(ix, iy) else {
        return None;
    };
    (0..2)
        .flat_map(|dy| (0..2).map(move |dx| (dx, dy)))
        .all(|(dx, dy)| field.material(ix + dx, iy + dy) == Material::Bench(level))
        .then_some(level)
}

pub(super) fn draw_missing_slab(rect: Rect, cell: f32, top: Color, value: u32) {
    let (x, y) = if value & 1 == 0 {
        (0.18, 0.42)
    } else {
        (0.52, 0.22)
    };
    let void = rgba(17, 18, 22);
    draw_rectangle(
        rect.x + cell * x,
        rect.y + cell * y,
        cell * 1.2,
        cell * 0.68,
        void,
    );
    draw_rectangle(
        rect.x + cell * (x + if value & 1 == 0 { 0.22 } else { -0.14 }),
        rect.y + cell * (y + 0.68),
        cell * 0.82,
        cell * 0.24,
        void,
    );
    draw_rectangle(
        rect.x + cell * (x + 0.18),
        rect.y + cell * (y + 0.94),
        cell * 0.66,
        cell * 0.12,
        shifted(top, -14),
    );
}

pub(super) fn draw_fracture(rect: Rect, cell: f32, tile: f32, top: Color, value: u32) {
    let points = if value & 1 == 0 {
        [(0.12, 0.72), (0.7, 0.56), (1.02, 1.1), (1.84, 1.38)]
    } else {
        [(0.42, 0.12), (0.58, 0.74), (1.34, 0.96), (1.62, 1.82)]
    };
    for pair in points.windows(2) {
        draw_line(
            rect.x + cell * pair[0].0,
            rect.y + cell * pair[0].1,
            rect.x + cell * pair[1].0,
            rect.y + cell * pair[1].1,
            (tile * 0.035).max(1.0),
            shifted(top, -22),
        );
    }
}

pub(super) fn draw_rebar(rect: Rect, cell: f32, tile: f32) {
    for offset in [0.0, 0.22] {
        for (width, color) in [
            ((tile * 0.04).max(1.0) + 1.0, rgba(48, 38, 39)),
            ((tile * 0.04).max(1.0), rgba(116, 67, 52)),
        ] {
            draw_line(
                rect.x + cell * (0.52 + offset),
                rect.y + cell * 0.36,
                rect.x + cell * (1.42 + offset),
                rect.y + cell * 1.54,
                width,
                color,
            );
        }
    }
}

fn draw_boundary_terraces(frame: MapFrame) {
    let field = TerraceField::new(frame);
    let (x_range, y_range) = field.visible_ranges();
    for iy in y_range.clone() {
        for ix in x_range.clone() {
            let material = field.material(ix, iy);
            let Some(color) = cell_color(material, ix, iy) else {
                continue;
            };
            let rect = field.rect(ix, iy);
            draw_rectangle(rect.x, rect.y, rect.w, rect.h, color);
        }
    }

    for iy in y_range.clone() {
        for ix in x_range.clone() {
            if ix.rem_euclid(4) != 0 || iy.rem_euclid(4) != 0 {
                continue;
            }
            let Some(level) = uniform_bench(&field, ix, iy) else {
                continue;
            };
            let value = hash(ix.div_euclid(4), iy.div_euclid(4), SALT + 503);
            let rect = field.rect(ix, iy);
            if value.is_multiple_of(41) {
                draw_missing_slab(rect, field.cell, LAYERS[level].top, value);
            } else if value % 17 == 3 {
                draw_fracture(rect, field.cell, field.tile, LAYERS[level].top, value);
            } else if value % 43 == 13 {
                draw_rebar(rect, field.cell, field.tile);
            }
        }
    }

    let lip = (frame.tile * 0.055).max(1.0);
    for iy in y_range {
        for ix in x_range.clone() {
            let Material::Bench(level) = field.material(ix, iy) else {
                continue;
            };
            let rect = field.rect(ix, iy);
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                if field.material(ix + dx, iy + dy) == Material::Riser(level) {
                    draw_lip(rect, (dx, dy), lip, LAYERS[level].lip);
                }
            }
        }
    }
}

pub(super) fn hash(x: i32, y: i32, salt: u32) -> u32 {
    let mut value = 2_166_136_261u32;
    for word in [x as u32, y as u32, salt] {
        for byte in word.to_le_bytes() {
            value ^= u32::from(byte);
            value = value.wrapping_mul(16_777_619);
        }
    }
    value
}

pub(super) fn draw_backdrop(_game: &Game) {
    draw_rectangle(0.0, 0.0, screen_width(), screen_height(), rgba(8, 9, 12));
}

pub(super) fn draw_boundary(game: &Game) {
    draw_boundary_terraces(MapFrame::from_game(game));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_authored_walls_are_absorbed_but_open_lanes_stay_playable() {
        let closed = ["#####", "#...#", "#...#", "#####"].map(str::to_string);
        assert_eq!(
            BoundaryInsets::from_rows(&closed),
            BoundaryInsets {
                top: true,
                right: true,
                bottom: true,
                left: true,
            }
        );
        let open = ["##.##", "#...#", "....#", "##.##"].map(str::to_string);
        assert_eq!(
            BoundaryInsets::from_rows(&open),
            BoundaryInsets {
                top: false,
                right: true,
                bottom: false,
                left: false,
            }
        );
    }

    #[test]
    fn terrace_material_never_enters_the_battlefield_rect() {
        let field = TerraceField::new(MapFrame {
            rect: Rect::new(32.0, 48.0, 640.0, 384.0),
            tile: 32.0,
        });
        for iy in 0..field.inner_height {
            for ix in 0..field.inner_width {
                assert_eq!(field.material(ix, iy), Material::Void);
            }
        }
        for layer in LAYERS {
            assert!(layer.bench_cells >= 3);
        }
    }

    #[test]
    fn floor_wave_carries_across_the_terrace_benches() {
        let phases: Vec<_> = (0..6)
            .map(|tile_x| WAVE_LIFTS[(tile_x as usize) % 6])
            .collect();
        assert_eq!(phases, WAVE_LIFTS);
        assert!(phases.iter().any(|lift| *lift < 0));
        assert!(phases.iter().any(|lift| *lift > 0));
    }
}
