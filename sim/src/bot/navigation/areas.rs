//! Linear-time unions of candidate neighborhoods on a bounded grid.

use chassis::grid::TilePos;

pub(in crate::bot) fn square_neighborhoods(
    width: i32,
    height: i32,
    seeds: impl IntoIterator<Item = TilePos>,
    radius: i32,
) -> Vec<TilePos> {
    if width <= 0 || height <= 0 || radius < 0 {
        return Vec::new();
    }
    let stride = width as usize + 1;
    let mut corners = vec![0_i64; stride * (height as usize + 1)];
    for seed in seeds {
        let left = (i64::from(seed.x) - i64::from(radius)).clamp(0, i64::from(width)) as usize;
        let right = (i64::from(seed.x) + i64::from(radius) + 1).clamp(0, i64::from(width)) as usize;
        let top = (i64::from(seed.y) - i64::from(radius)).clamp(0, i64::from(height)) as usize;
        let bottom =
            (i64::from(seed.y) + i64::from(radius) + 1).clamp(0, i64::from(height)) as usize;
        if left == right || top == bottom {
            continue;
        }
        corners[top * stride + left] += 1;
        corners[top * stride + right] -= 1;
        corners[bottom * stride + left] -= 1;
        corners[bottom * stride + right] += 1;
    }
    let mut vertical = vec![0_i64; width as usize];
    let mut tiles = Vec::new();
    for y in 0..height as usize {
        let mut coverage = 0;
        for (x, column) in vertical.iter_mut().enumerate() {
            *column += corners[y * stride + x];
            coverage += *column;
            if coverage > 0 {
                tiles.push(TilePos::new(x as i32, y as i32));
            }
        }
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neighborhood_union_matches_scalar_distance_for_overlaps_edges_and_duplicates() {
        for mask in 0..512 {
            let mut seeds: Vec<_> = (0..9)
                .filter(|bit| mask & (1 << bit) != 0)
                .map(|bit| TilePos::new(bit % 3 - 1, bit / 3 - 1))
                .collect();
            seeds.extend(seeds.clone());
            seeds.extend([TilePos::new(i32::MIN, 0), TilePos::new(i32::MAX, 0)]);
            for radius in 0..5 {
                let expected: Vec<_> = (0..4)
                    .flat_map(|y| (0..5).map(move |x| TilePos::new(x, y)))
                    .filter(|tile| {
                        seeds.iter().any(|seed| {
                            (i64::from(tile.x) - i64::from(seed.x))
                                .abs()
                                .max((i64::from(tile.y) - i64::from(seed.y)).abs())
                                <= i64::from(radius)
                        })
                    })
                    .collect();
                assert_eq!(
                    square_neighborhoods(5, 4, seeds.iter().copied(), radius),
                    expected
                );
            }
        }
        assert!(square_neighborhoods(0, 4, [TilePos::new(0, 0)], 2).is_empty());
        assert!(square_neighborhoods(4, 4, [TilePos::new(0, 0)], -1).is_empty());
    }
}
