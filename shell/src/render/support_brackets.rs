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
mod tests {
    use super::*;

    fn at(anchors: &[(i32, i32)], corner: (i32, i32), zoom: f32, scale: f32) -> Vec<Bracket> {
        junctions(
            anchors.iter().map(|&(x, y)| (TilePos::new(x, y), (2, 2))),
            zoom,
            scale,
        )
        .into_iter()
        .filter(|bracket| bracket.corner == TilePos::new(corner.0, corner.1))
        .collect()
    }

    #[test]
    fn isolated_corners_keep_the_existing_padding_and_arms() {
        let brackets = at(&[(0, 0)], (0, 0), 32.0, 1.0);
        assert_eq!(brackets.len(), 1);
        assert_eq!(
            brackets[0].segments(vec2(100.0, 100.0)),
            [
                [vec2(97.0, 97.0), vec2(103.0, 97.0)],
                [vec2(97.0, 97.0), vec2(97.0, 103.0)],
            ]
        );
    }

    #[test]
    fn rows_and_columns_share_single_t_junctions() {
        for (anchors, corner, horizontal, vertical, offset) in [
            (
                [(0, 0), (2, 0), (4, 0)],
                (2, 0),
                [-6.0, 6.0],
                [0.0, 6.0],
                vec2(0.0, -3.0),
            ),
            (
                [(0, 0), (2, 0), (4, 0)],
                (4, 2),
                [-6.0, 6.0],
                [-6.0, 0.0],
                vec2(0.0, 3.0),
            ),
            (
                [(0, 0), (0, 2), (0, 4)],
                (0, 2),
                [0.0, 6.0],
                [-6.0, 6.0],
                vec2(-3.0, 0.0),
            ),
            (
                [(0, 0), (0, 2), (0, 4)],
                (2, 4),
                [-6.0, 0.0],
                [-6.0, 6.0],
                vec2(3.0, 0.0),
            ),
        ] {
            let brackets = at(&anchors, corner, 32.0, 1.0);
            assert_eq!(brackets.len(), 1);
            assert_eq!(brackets[0].horizontal, horizontal);
            assert_eq!(brackets[0].vertical, vertical);
            assert_eq!(brackets[0].offset, offset);
        }
    }

    #[test]
    fn four_buildings_share_one_centered_cross_at_every_zoom_and_scale() {
        for zoom in [8.0, 32.0, 64.0, 96.0] {
            for scale in [1.0, 1.25, 1.5] {
                let brackets = at(&[(0, 0), (2, 0), (0, 2), (2, 2)], (2, 2), zoom, scale);
                assert_eq!(brackets.len(), 1);
                let bracket = &brackets[0];
                assert_eq!(bracket.offset, Vec2::ZERO);
                assert_eq!(bracket.horizontal, bracket.vertical);
                assert_eq!(bracket.horizontal[0], -bracket.horizontal[1]);
                assert!(bracket.horizontal[1] > 0.0);
                assert!(bracket.horizontal[1] <= 6.0 * scale);
                assert!(bracket.horizontal[1] <= (2.0 * zoom + 6.0 * scale) * 0.25);
            }
        }
    }

    #[test]
    fn diagonal_pairs_remain_two_separated_ls() {
        for anchors in [[(0, 0), (2, 2)], [(2, 0), (0, 2)]] {
            let brackets = at(&anchors, (2, 2), 32.0, 1.0);
            assert_eq!(brackets.len(), 2);
            assert_eq!(brackets[0].offset, -brackets[1].offset);
            for bracket in &brackets {
                for [from, to] in bracket.segments(Vec2::ZERO) {
                    assert!(from.x * to.x > 0.0 && from.y * to.y > 0.0);
                }
            }
        }
    }

    #[test]
    fn separated_buildings_do_not_merge_and_input_order_does_not_matter() {
        let anchors = [(0, 0), (3, 0), (0, 3), (3, 3)];
        let geometry = |anchors: &[(i32, i32)]| {
            junctions(
                anchors.iter().map(|&(x, y)| (TilePos::new(x, y), (2, 2))),
                32.0,
                1.0,
            )
            .into_iter()
            .map(|bracket| (bracket.corner, bracket.segments(Vec2::ZERO)))
            .collect::<Vec<_>>()
        };
        let forward = geometry(&anchors);
        assert_eq!(forward.len(), 16);
        let mut reversed = anchors;
        reversed.reverse();
        assert_eq!(forward, geometry(&reversed));
    }

    #[test]
    fn dense_block_draws_each_junction_once_and_empty_selection_draws_nothing() {
        assert!(junctions([], 32.0, 1.0).is_empty());
        let footprints =
            (0..3).flat_map(|y| (0..3).map(move |x| (TilePos::new(x * 2, y * 2), (2, 2))));
        let brackets = junctions(footprints, 32.0, 1.0);
        assert_eq!(brackets.len(), 16);
        let mut counts = [0; 5];
        for bracket in brackets {
            let arms = bracket
                .horizontal
                .into_iter()
                .chain(bracket.vertical)
                .filter(|&length| length != 0.0)
                .count();
            counts[arms] += 1;
        }
        assert_eq!(counts, [0, 0, 4, 8, 4]);
    }
}
