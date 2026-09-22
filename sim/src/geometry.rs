//! Canonical movement and production geometry shared by rules and command prediction.
use chassis::grid::TilePos;

/// The ring of tiles surrounding a rectangle, row-major (deterministic).
pub fn rect_adjacent_tiles(anchor: TilePos, size: (i32, i32)) -> impl Iterator<Item = TilePos> {
    let (w, h) = size;
    (-1..=h).flat_map(move |dy| {
        (-1..=w)
            .map(move |dx| anchor.offset(dx, dy))
            .filter(move |t| {
                let inside = t.x > anchor.x - 1
                    && t.x < anchor.x + w
                    && t.y > anchor.y - 1
                    && t.y < anchor.y + h;
                !inside
            })
    })
}

/// Orders production doorsteps in the producer's radial frame around the map.
/// Mirrored producers have negated radial and candidate rays, preserving both
/// dot and cross products and therefore selecting mirrored spawn tiles.
pub fn spawn_doorstep_key(
    map_size: (i32, i32),
    anchor: TilePos,
    size: (i32, i32),
    candidate: TilePos,
) -> (std::cmp::Reverse<i64>, i64) {
    let center_x = i64::from(anchor.x) * 2 + i64::from(size.0);
    let center_y = i64::from(anchor.y) * 2 + i64::from(size.1);
    let radial_x = center_x - i64::from(map_size.0);
    let radial_y = center_y - i64::from(map_size.1);
    let candidate_x = i64::from(candidate.x) * 2 + 1 - center_x;
    let candidate_y = i64::from(candidate.y) * 2 + 1 - center_y;
    let dot = radial_x * candidate_x + radial_y * candidate_y;
    let cross = radial_x * candidate_y - radial_y * candidate_x;
    (std::cmp::Reverse(dot), cross)
}

/// Whether a group command scans snapped and spread goals in the half-turned
/// approach frame. Command execution and fog-honest route projection share
/// this pure calculation so they cannot assign different tiles.
pub fn group_spread_scan_reversed(
    center: TilePos,
    unit_tiles: impl IntoIterator<Item = TilePos>,
    foundry: Option<(TilePos, (i32, i32))>,
    map_size: (i32, i32),
    player: crate::ids::PlayerId,
) -> bool {
    let reversed = |dx: i64, dy: i64| dy < 0 || (dy == 0 && dx < 0);
    for tile in unit_tiles {
        let dx = i64::from(center.x) - i64::from(tile.x);
        let dy = i64::from(center.y) - i64::from(tile.y);
        if dx != 0 || dy != 0 {
            return reversed(dx, dy);
        }
    }

    if let Some((anchor, (width, height))) = foundry {
        let dx = i64::from(center.x) * 2 + 1 - (i64::from(anchor.x) * 2 + i64::from(width));
        let dy = i64::from(center.y) * 2 + 1 - (i64::from(anchor.y) * 2 + i64::from(height));
        if dx != 0 || dy != 0 {
            return reversed(dx, dy);
        }
    }

    let dx = i64::from(center.x) * 2 + 1 - i64::from(map_size.0);
    let dy = i64::from(center.y) * 2 + 1 - i64::from(map_size.1);
    if dx != 0 || dy != 0 {
        return reversed(dx, dy);
    }

    player.0 % 2 == 1
}

/// A doorstep's stable key in the approaching body's local frame.
///
/// The dot and cross products are unchanged by a 180-degree rotation of both
/// the footprint and body. This makes a mirrored approach choose a mirrored
/// candidate instead of inheriting the absolute row-major scan direction.
/// Coordinates are doubled so even-sized footprint centers stay exact.
pub fn rect_approach_key(
    from: TilePos,
    anchor: TilePos,
    size: (i32, i32),
    candidate: TilePos,
) -> (i32, std::cmp::Reverse<i64>, i64) {
    rect_approach_key_from(from, from, anchor, size, candidate)
}

/// Rank a footprint approach using an explicit canonical tie frame.
pub fn rect_approach_key_from(
    from: TilePos,
    approach_from: TilePos,
    anchor: TilePos,
    size: (i32, i32),
    candidate: TilePos,
) -> (i32, std::cmp::Reverse<i64>, i64) {
    let center_x = i64::from(anchor.x) * 2 + i64::from(size.0);
    let center_y = i64::from(anchor.y) * 2 + i64::from(size.1);
    let approach_x = i64::from(approach_from.x) * 2 + 1 - center_x;
    let approach_y = i64::from(approach_from.y) * 2 + 1 - center_y;
    let candidate_x = i64::from(candidate.x) * 2 + 1 - center_x;
    let candidate_y = i64::from(candidate.y) * 2 + 1 - center_y;
    let dot = approach_x * candidate_x + approach_y * candidate_y;
    let cross = approach_x * candidate_y - approach_y * candidate_x;
    (candidate.chebyshev(from), std::cmp::Reverse(dot), cross)
}

/// The command approach frame from immutable map and ownership facts. Command
/// previews use this same tie frame without needing authoritative `State`.
pub fn rect_approach_origin_for_map(
    map_size: (i32, i32),
    player: crate::ids::PlayerId,
    from: TilePos,
    anchor: TilePos,
    size: (i32, i32),
    first_foundry: Option<(TilePos, (i32, i32))>,
) -> TilePos {
    let center_x = i64::from(anchor.x) * 2 + i64::from(size.0);
    let center_y = i64::from(anchor.y) * 2 + i64::from(size.1);
    let from_x = i64::from(from.x) * 2 + 1;
    let from_y = i64::from(from.y) * 2 + 1;
    if from_x != center_x || from_y != center_y {
        return from;
    }

    if let Some((foundry_anchor, foundry_size)) = first_foundry {
        let flip_x =
            i64::from(foundry_anchor.x) * 2 + i64::from(foundry_size.0) >= i64::from(map_size.0);
        let flip_y =
            i64::from(foundry_anchor.y) * 2 + i64::from(foundry_size.1) >= i64::from(map_size.1);
        return foundry_anchor.offset(
            if flip_x { foundry_size.0 - 1 } else { 0 },
            if flip_y { foundry_size.1 - 1 } else { 0 },
        );
    }

    let flip_x = if center_x == i64::from(map_size.0) {
        player.0 % 2 == 1
    } else {
        center_x > i64::from(map_size.0)
    };
    let flip_y = if center_y == i64::from(map_size.1) {
        player.0 % 2 == 1
    } else {
        center_y > i64::from(map_size.1)
    };
    TilePos::new(
        if flip_x { map_size.0 - 1 } else { 0 },
        if flip_y { map_size.1 - 1 } else { 0 },
    )
}
