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
mod tests;
