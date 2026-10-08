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

/// Center of a footprint in world coordinates. An even side puts it on a tile seam.
pub fn footprint_center(anchor: TilePos, size: (i32, i32)) -> chassis::fx::Vec2Fx {
    use chassis::fx::{Fx, HALF, Vec2Fx};
    Vec2Fx::new(
        Fx::from_num(anchor.x) + Fx::from_num(size.0) * HALF,
        Fx::from_num(anchor.y) + Fx::from_num(size.1) * HALF,
    )
}

/// Nearest point on the closed rectangle occupied by a footprint.
pub fn footprint_contact(
    pos: chassis::fx::Vec2Fx,
    anchor: TilePos,
    size: (i32, i32),
) -> chassis::fx::Vec2Fx {
    use chassis::fx::{Fx, HALF, Vec2Fx};
    let min = anchor.center() - Vec2Fx::new(HALF, HALF);
    let max = min + Vec2Fx::new(Fx::from_num(size.0), Fx::from_num(size.1));
    Vec2Fx::new(pos.x.clamp(min.x, max.x), pos.y.clamp(min.y, max.y))
}

/// Center clearance at a work position; the chassis nose may overhang the tile margin.
pub fn work_approach_distance(radius: chassis::fx::Fx) -> chassis::fx::Fx {
    (radius - crate::stats::WORK_FOOTPRINT_OVERHANG).max(chassis::fx::Fx::ZERO)
        + crate::stats::WORK_FOOTPRINT_GAP
}

/// Work position on the chosen doorstep, preserving its approach side.
pub fn work_approach_point(
    goal: TilePos,
    anchor: TilePos,
    size: (i32, i32),
    radius: chassis::fx::Fx,
) -> chassis::fx::Vec2Fx {
    let contact = footprint_contact(goal.center(), anchor, size);
    let outward = goal.center() - contact;
    contact + outward * (work_approach_distance(radius) / outward.length())
}

/// The tile a work position belongs to, seen from a worker at `from` beside
/// a footprint centred on `centre`. A position exactly on the edge between
/// two tiles belongs to the one on the worker's side, and for a worker on
/// that edge itself, to the one a clockwise turn about the centre leads to:
/// flooring alone always takes the tile on the positive side, so mirrored
/// workers would choose unmirrored tiles.
pub fn work_tile(
    entry: chassis::fx::Vec2Fx,
    from: chassis::fx::Vec2Fx,
    centre: chassis::fx::Vec2Fx,
) -> TilePos {
    use chassis::fx::Fx;
    let tile = TilePos::containing(entry);
    let offset = entry - centre;
    // `lead` is the turn's component along the axis and `across` the offset
    // along it, which settles a position level with the centre. Both flip
    // under a half turn, as the tiles either side of the edge swap.
    let back = |at: Fx, from: Fx, lead: Fx, across: Fx| {
        let turned = lead < Fx::ZERO || (lead == Fx::ZERO && across > Fx::ZERO);
        i32::from(at.frac() == Fx::ZERO && (from < at || (from == at && turned)))
    };
    tile.offset(
        -back(entry.x, from.x, -offset.y, offset.x),
        -back(entry.y, from.y, offset.x, offset.y),
    )
}

/// Whether a chassis circle fits the adjacent passable tiles.
pub fn circle_clear(
    point: chassis::fx::Vec2Fx,
    radius: chassis::fx::Fx,
    open: impl Fn(TilePos) -> bool,
) -> bool {
    let tile = TilePos::containing(point);
    let reach = radius.ceil().to_num::<i32>();
    (-reach..=reach).all(|dy| {
        (-reach..=reach).all(|dx| {
            let at = tile.offset(dx, dy);
            open(at) || point.dist_sq(footprint_contact(point, at, (1, 1))) >= radius * radius
        })
    })
}

/// Candidate centers around a work footprint with mirrored face offsets.
pub fn work_positions(
    anchor: TilePos,
    size: (i32, i32),
    clearance: chassis::fx::Fx,
    pitch: chassis::fx::Fx,
) -> Vec<chassis::fx::Vec2Fx> {
    use chassis::fx::{Fx, Vec2Fx};
    let x = Fx::from_num(anchor.x);
    let y = Fx::from_num(anchor.y);
    let w = Fx::from_num(size.0);
    let h = Fx::from_num(size.1);
    let inset = const { Fx::lit("0.02") };
    let mut points = Vec::new();
    for (span, horizontal) in [(w, true), (h, false)] {
        let divisions = (span / pitch).floor().to_num::<i32>().max(1);
        for i in 0..=divisions + i32::from(divisions % 2 != 0) {
            // Mirrored offsets share the same division rounding.
            let offset = if i > divisions {
                span / Fx::from_num(2)
            } else {
                span / Fx::from_num(2)
                    + (span - inset * Fx::from_num(2)) * Fx::from_num(2 * i - divisions)
                        / Fx::from_num(2 * divisions)
            };
            if horizontal {
                points.push(Vec2Fx::new(x + offset, y - clearance));
                points.push(Vec2Fx::new(x + offset, y + h + clearance));
            } else {
                points.push(Vec2Fx::new(x - clearance, y + offset));
                points.push(Vec2Fx::new(x + w + clearance, y + offset));
            }
        }
    }
    points
}

#[cfg(test)]
mod tests;
