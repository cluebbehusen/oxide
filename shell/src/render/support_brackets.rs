//! Shared corner geometry for marked economy-support endpoints.

use std::collections::BTreeMap;

use chassis::grid::TilePos;
use macroquad::prelude::{Vec2, vec2};

const NW: u8 = 1;
const NE: u8 = 2;
const SE: u8 = 4;
const SW: u8 = 8;

pub(super) struct Bracket {
    pub corner: TilePos,
    offset: Vec2,
    horizontal: [f32; 2],
    vertical: [f32; 2],
}

impl Bracket {
    pub fn segments(&self, origin: Vec2) -> [[Vec2; 2]; 2] {
        let center = origin + self.offset;
        [
            self.horizontal.map(|x| center + vec2(x, 0.0)),
            self.vertical.map(|y| center + vec2(0.0, y)),
        ]
    }
}

pub(super) fn junctions(
    footprints: impl IntoIterator<Item = (TilePos, (i32, i32))>,
    zoom: f32,
    scale: f32,
) -> Vec<Bracket> {
    let padding = 3.0 * scale;
    let mut corners = BTreeMap::<(i32, i32), (u8, f32)>::new();
    for (min, (width, height)) in footprints {
        let reach = (6.0 * scale).min((width.min(height) as f32 * zoom + 2.0 * padding) * 0.25);
        for (x, y, quadrant) in [
            (min.x, min.y, SE),
            (min.x + width, min.y, SW),
            (min.x + width, min.y + height, NW),
            (min.x, min.y + height, NE),
        ] {
            let entry = corners.entry((y, x)).or_insert((0, reach));
            entry.0 |= quadrant;
            entry.1 = entry.1.min(reach);
        }
    }
    let mut brackets = Vec::with_capacity(corners.len());
    for ((y, x), (quadrants, reach)) in corners {
        let corner = TilePos::new(x, y);
        if quadrants == (NW | SE) || quadrants == (NE | SW) {
            // Opposite corners alone have no shared edge. Keep their Ls apart
            // inside the footprints instead of inventing a four-way junction.
            for quadrant in [NW, NE, SE, SW] {
                if quadrants & quadrant != 0 {
                    brackets.push(bracket(corner, quadrant, reach, -padding));
                }
            }
        } else {
            brackets.push(bracket(corner, quadrants, reach, padding));
        }
    }
    brackets
}

fn bracket(corner: TilePos, quadrants: u8, reach: f32, padding: f32) -> Bracket {
    let left = quadrants & (NW | SW) != 0;
    let right = quadrants & (NE | SE) != 0;
    let up = quadrants & (NW | NE) != 0;
    let down = quadrants & (SW | SE) != 0;
    Bracket {
        corner,
        offset: vec2(
            (i32::from(left) - i32::from(right)) as f32 * padding,
            (i32::from(up) - i32::from(down)) as f32 * padding,
        ),
        horizontal: [
            if left { -reach } else { 0.0 },
            if right { reach } else { 0.0 },
        ],
        vertical: [
            if up { -reach } else { 0.0 },
            if down { reach } else { 0.0 },
        ],
    }
}

#[cfg(test)]
mod tests;
