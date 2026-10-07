//! Compact solid sprite regions for presentation-only surface contact.

use macroquad::prelude::{Image, Rect, Vec2, vec2};

pub(crate) struct SpriteContact {
    regions: Vec<Rect>,
}

impl SpriteContact {
    pub(crate) fn capture(image: &Image, [left, top, width, height]: [u32; 4]) -> Self {
        let mut regions: Vec<Rect> = Vec::new();
        let mut previous: Vec<(u32, u32, usize)> = Vec::new();
        for y in 0..height {
            let solid = |x| {
                let pixel = ((top + y) * u32::from(image.width) + left + x) as usize;
                image.bytes[pixel * 4 + 3] >= 128
            };
            let mut current = Vec::new();
            let mut x = 0;
            while x < width {
                if !solid(x) {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < width && solid(x) {
                    x += 1;
                }
                let index = if let Some(&(_, _, index)) =
                    previous.iter().find(|&&(a, b, _)| a == start && b == x)
                {
                    regions[index].h += 1.0;
                    index
                } else {
                    regions.push(Rect::new(start as f32, y as f32, (x - start) as f32, 1.0));
                    regions.len() - 1
                };
                current.push((start, x, index));
            }
            previous = current;
        }
        for region in &mut regions {
            region.x /= width as f32;
            region.w /= width as f32;
            region.y /= height as f32;
            region.h /= height as f32;
        }
        Self { regions }
    }

    pub(crate) fn contact(&self, from: Vec2, aim: Vec2, origin: Vec2, size: Vec2) -> Option<Vec2> {
        let direction = aim - from;
        let regions = || {
            self.regions.iter().map(|region| {
                let min = origin + vec2(region.x, region.y) * size;
                (min, min + vec2(region.w, region.h) * size)
            })
        };
        let arrival = regions()
            .filter_map(|(min, max)| {
                let mut entry = 0.0_f32;
                let mut exit = f32::INFINITY;
                for axis in 0..2 {
                    if direction[axis].abs() < 1e-6 {
                        if from[axis] < min[axis] || from[axis] > max[axis] {
                            return None;
                        }
                    } else {
                        let a = (min[axis] - from[axis]) / direction[axis];
                        let b = (max[axis] - from[axis]) / direction[axis];
                        entry = entry.max(a.min(b));
                        exit = exit.min(a.max(b));
                    }
                }
                (entry <= exit && direction.length_squared() > 1e-12).then_some(entry)
            })
            .min_by(f32::total_cmp);
        if let Some(arrival) = arrival {
            return Some(from + direction * arrival);
        }
        // A ray grazing the rectangular footprint can miss every solid pixel.
        // Keep that cosmetic hit on the nearest visible surface instead.
        regions()
            .map(|(min, max)| from.clamp(min, max))
            .min_by(|a, b| {
                a.distance_squared(from)
                    .total_cmp(&b.distance_squared(from))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::numeric;
    use crate::numeric::Fit;

    #[test]
    fn cropped_outline_extends_the_shot_through_padding_and_cut_corners() {
        let mut image = Image::gen_image_color(16, 16, macroquad::prelude::BLANK);
        for y in 2..6 {
            for x in 2..6 {
                if x != 2 || y != 2 {
                    image.bytes[((y + 3) * 16 + x + 4) * 4 + 3] = 255;
                }
            }
        }
        image.bytes[(3 * 16 + 4) * 4 + 3] = 127;
        let shape = SpriteContact::capture(&image, [4, 3, 8, 8]);
        for (from, aim, expected) in [
            (vec2(-2., -2.), Vec2::ZERO, vec2(3., 3.)),
            (vec2(-2., 4.), vec2(0., 4.), vec2(2., 4.)),
            (vec2(4., -2.), vec2(4., 0.), vec2(4., 2.)),
            (vec2(10., 4.), vec2(8., 4.), vec2(6., 4.)),
            (vec2(4., 10.), vec2(4., 8.), vec2(4., 6.)),
        ] {
            let origin = vec2(13., 9.);
            let contact = shape
                .contact(origin + from, origin + aim, origin, Vec2::splat(8.))
                .unwrap();
            assert_eq!(contact, origin + expected);
        }
        assert_eq!(
            shape.contact(vec2(-2., 0.), Vec2::ZERO, Vec2::ZERO, Vec2::splat(8.)),
            Some(vec2(2., 3.))
        );
        let empty = SpriteContact::capture(&image, [0, 0, 2, 2]);
        assert!(
            empty
                .contact(Vec2::ZERO, Vec2::ONE, Vec2::ZERO, Vec2::ONE)
                .is_none()
        );
    }

    #[test]
    fn fabricator_reports_reach_solid_pixels_in_idle_work_and_construction_frames() {
        let manifest: std::collections::BTreeMap<String, [u32; 4]> =
            serde_json::from_str(include_str!("../../assets/sprites/atlas.json")).unwrap();
        let load = |name: &str| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../assets/sprites")
                .join(name);
            Image::from_file_with_format(&std::fs::read(path).unwrap(), None).unwrap()
        };
        let first = load("atlas.png");
        let page_height = u32::from(first.height);
        let mut pages = std::collections::BTreeMap::from([(0, first)]);
        let mut count = 0;
        for (name, [x, y, width, height]) in manifest {
            if !name.starts_with("fabricator_") || name.contains("accent") {
                continue;
            }
            let page = y / page_height;
            let image = pages
                .entry(page)
                .or_insert_with(|| load(&format!("atlas_{page}.png")));
            let top = y % page_height;
            let shape = SpriteContact::capture(image, [x, top, width, height]);
            for (from, aim) in [
                (vec2(-1.5, -1.5), Vec2::ZERO),
                (vec2(-1.5, 0.5), vec2(0., 0.5)),
                (vec2(1.5, -1.5), vec2(1.5, 0.)),
                (vec2(3.5, 1.5), vec2(2., 1.5)),
                (vec2(0.5, 3.5), vec2(0.5, 2.)),
            ] {
                let contact = shape
                    .contact(from, aim, Vec2::ZERO, Vec2::splat(2.))
                    .unwrap();
                let pixel = contact / 2. * vec2(width as f32, height as f32);
                let on_solid = (-1..=0).any(|dy| {
                    (-1..=0).any(|dx| {
                        let px = numeric::to_i32(pixel.x.floor()) + dx;
                        let py = numeric::to_i32(pixel.y.floor()) + dy;
                        px >= 0
                            && py >= 0
                            && px < width.fit::<i32>()
                            && py < height.fit::<i32>()
                            && image.bytes[(((top + py.fit::<u32>()) * u32::from(image.width)
                                + x
                                + px.fit::<u32>())
                                * 4
                                + 3) as usize]
                                >= 128
                    })
                });
                assert!(on_solid, "{name}: {from:?} -> {contact:?}");
                if name == "fabricator_cupric" {
                    assert!(
                        contact.distance(aim) > 0.1,
                        "the reported footprint still misses the body"
                    );
                    assert!(
                        (contact - from).perp_dot(aim - from).abs() < 1e-5,
                        "preserve the original firing direction"
                    );
                }
            }
            count += 1;
        }
        assert_eq!(
            count, 22,
            "two factions, each with idle, four work, and six construction frames"
        );
    }
}
