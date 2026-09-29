//! Seat-relative tie-breaking. Distances and components are exact and need no
//! orientation; only equal choices need a rule, and that rule must mirror
//! between mirrored seats.

use crate::map::MapModel;
use chassis::grid::TilePos;
use oxide_sim::BuildingKind;
use oxide_sim::observation::ObservationData;
use std::cmp::Reverse;

/// Ranks targets in the seat's home-relative frame, in doubled coordinates so
/// tile and footprint centres stay integral.
///
/// Nearer targets come first. Equal distances prefer the target further along
/// the ray from the map centre to home, then the one clockwise of it. A
/// half-turn of the map negates both that ray and every offset, preserving
/// both products, so mirrored seats break mirrored ties the same way.
#[derive(Clone, Copy)]
pub(crate) struct HomeFrame {
    pub(crate) home: (i64, i64),
    radial: (i64, i64),
}

impl HomeFrame {
    /// The frame around the seat's authored start or, on a map without one,
    /// its earliest built Foundry.
    pub(crate) fn of(observation: &ObservationData, map: &MapModel) -> Option<Self> {
        let anchor = map.start(observation.me).or_else(|| {
            observation
                .my_buildings
                .iter()
                .filter(|building| building.kind == BuildingKind::Foundry && building.built)
                .min_by_key(|building| building.id)
                .map(|building| building.anchor)
        })?;
        Some(Self::at(
            anchor,
            observation.map_width,
            observation.map_height,
        ))
    }

    /// A frame around `home` whose ties follow `radial`, both in doubled
    /// coordinates scaled alike.
    pub(crate) fn around(home: (i64, i64), radial: (i64, i64)) -> Self {
        Self { home, radial }
    }

    /// The frame around a Foundry at `anchor` on a map of the given size.
    pub(crate) fn at(anchor: TilePos, width: i32, height: i32) -> Self {
        let home = footprint_centre(BuildingKind::Foundry, anchor);
        Self {
            home,
            radial: (home.0 - i64::from(width), home.1 - i64::from(height)),
        }
    }

    /// Ranks `to` as seen from `from`, both in doubled coordinates. A home
    /// exactly at the map centre has no ray; the trailing row-major key then
    /// only keeps the order total.
    pub(crate) fn rank(
        self,
        from: (i64, i64),
        to: (i64, i64),
    ) -> (i64, Reverse<i64>, i64, i64, i64) {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let dot = self.radial.0 * dx + self.radial.1 * dy;
        let cross = self.radial.0 * dy - self.radial.1 * dx;
        (dx * dx + dy * dy, Reverse(dot), cross, to.1, to.0)
    }
}

/// Empty tiles between two footprints along the wider axis; negative when
/// they overlap. Measuring between whole footprints keeps it the same for
/// mirrored pairs, which a distance from a top-left anchor would not.
pub(crate) fn gap(a: TilePos, a_size: (i32, i32), b: TilePos, b_size: (i32, i32)) -> i32 {
    let dx = (b.x - (a.x + a_size.0)).max(a.x - (b.x + b_size.0));
    let dy = (b.y - (a.y + a_size.1)).max(a.y - (b.y + b_size.1));
    dx.max(dy)
}

/// The tiles bordering the footprint of `size` at `anchor`.
pub(crate) fn ring(anchor: TilePos, size: (i32, i32)) -> impl Iterator<Item = TilePos> {
    let (width, height) = size;
    (-1..=height)
        .flat_map(move |dy| (-1..=width).map(move |dx| (dx, dy)))
        .filter(move |(dx, dy)| !(0..width).contains(dx) || !(0..height).contains(dy))
        .map(move |(dx, dy)| anchor.offset(dx, dy))
}

/// A tile's centre in doubled coordinates.
pub(crate) fn doubled(tile: TilePos) -> (i64, i64) {
    (i64::from(tile.x) * 2 + 1, i64::from(tile.y) * 2 + 1)
}

/// A building footprint's centre in doubled coordinates.
pub(crate) fn footprint_centre(kind: BuildingKind, anchor: TilePos) -> (i64, i64) {
    let (width, height) = kind.base_stats().size;
    (
        i64::from(anchor.x) * 2 + i64::from(width),
        i64::from(anchor.y) * 2 + i64::from(height),
    )
}

/// Chebyshev distance in tiles, rounded down, between two points in doubled
/// coordinates, such as footprint centres. Unlike anchors, centres keep their
/// distances under a half-turn whatever the footprints' sizes.
pub(crate) fn centre_distance(a: (i64, i64), b: (i64, i64)) -> u64 {
    (a.0.abs_diff(b.0)).max(a.1.abs_diff(b.1)) / 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(home: (i64, i64), map: (i64, i64)) -> HomeFrame {
        HomeFrame {
            home,
            radial: (home.0 - map.0, home.1 - map.1),
        }
    }

    #[test]
    fn home_frame_ranks_nearer_targets_first_and_mirrors_its_ties() {
        let map = (24, 12);
        let rotate = |tile: TilePos| TilePos::new(23 - tile.x, 11 - tile.y);
        let west = frame((8, 12), map);
        let east = frame((40, 12), map);
        let nearest = |frame: HomeFrame, from: TilePos, nodes: &[TilePos]| {
            nodes
                .iter()
                .copied()
                .min_by_key(|node| frame.rank(doubled(from), doubled(*node)))
                .unwrap()
        };
        let harvester = TilePos::new(7, 6);
        let tied = [TilePos::new(7, 3), TilePos::new(7, 9)];

        assert_eq!(
            nearest(west, harvester, &[tied[0], tied[1], TilePos::new(9, 6)]),
            TilePos::new(9, 6)
        );
        let west_choice = nearest(west, harvester, &tied);
        assert_eq!(west_choice, TilePos::new(7, 9), "not broken row-major");
        assert_eq!(
            nearest(east, rotate(harvester), &tied.map(rotate)),
            rotate(west_choice)
        );
        let centred = frame(map, map);
        assert_eq!(nearest(centred, harvester, &tied), TilePos::new(7, 3));
    }
}
