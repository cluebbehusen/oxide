//! A per-tick spatial index over living units — scratch for the tick
//! pipeline, never a field on [`State`](crate::State): `State`
//! serializes and hashes, so a cached index stored there would either
//! move every hash or poison the derived equality with a skipped field.
//!
//! Entries are `(tile, slot)` pairs sorted by `(y, x, slot)` — exactly
//! the order the collision resolver's bucket list has always used —
//! plus a row-offset table, so a neighborhood query slices each row
//! once and walks it contiguously instead of binary-searching the whole
//! vector per tile. Every window walk yields candidates in the same
//! deterministic order the full scans produced; whoever consumes the
//! index inherits that order, not a new one.

use crate::state::Unit;
use chassis::grid::TilePos;

/// Reusable tile index over the living units of one moment.
///
/// Rebuild at every use point — positions move between pipeline phases,
/// and the collision resolver deliberately snapshots per pass. The
/// buffers survive rebuilds, so one instance threaded through the tick
/// costs its allocations once.
pub(super) struct UnitIndex {
    /// `(tile, slot into the units vec)`, sorted by `(y, x, slot)`.
    entries: Vec<(TilePos, usize)>,
    /// `rows[y]..rows[y + 1]` bounds row `y` within `entries`.
    rows: Vec<u32>,
    /// Row count the offsets were built for.
    height: i32,
}

impl UnitIndex {
    /// An empty index; [`rebuild`](Self::rebuild) before querying.
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            rows: Vec::new(),
            height: 0,
        }
    }

    /// Rebuilds the index over `units` (dead bodies excluded), reusing
    /// the buffers. `height` is the map's row count; every living unit
    /// stands on the map, so rows cover every entry.
    pub(super) fn rebuild(&mut self, units: &[Unit], height: i32) {
        self.height = height;
        self.entries.clear();
        self.entries.extend(
            units
                .iter()
                .enumerate()
                .filter(|(_, u)| u.hp > 0)
                .map(|(slot, u)| (u.tile(), slot)),
        );
        self.entries
            .sort_unstable_by_key(|&(t, slot)| (t.y, t.x, slot));
        self.rows.clear();
        self.rows.reserve(height as usize + 1);
        let mut i = 0usize;
        for y in 0..=height {
            while i < self.entries.len() && self.entries[i].0.y < y {
                i += 1;
            }
            self.rows.push(i as u32);
        }
    }

    /// The entries of row `y` with `x` in `x_min..=x_max`, in ascending
    /// `(x, slot)` order — byte-for-byte the sequence a tile-by-tile
    /// bucket walk over that span produces. Empty for off-map rows.
    pub(super) fn row_span(&self, y: i32, x_min: i32, x_max: i32) -> &[(TilePos, usize)] {
        if y < 0 || y >= self.height {
            return &[];
        }
        let row = &self.entries[self.rows[y as usize] as usize..self.rows[y as usize + 1] as usize];
        let start = row.partition_point(|&(t, _)| t.x < x_min);
        let len = row[start..].partition_point(|&(t, _)| t.x <= x_max);
        &row[start..start + len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Scenario;

    /// Every window the index serves must equal a brute-force filter of
    /// the unit list, entry for entry — the order is load-bearing (the
    /// collision resolver applies corrections in visit order).
    #[test]
    fn row_spans_match_a_full_filter_in_order() {
        let mut state = Scenario::skirmish().build().expect("skirmish builds");
        for _ in 0..90 {
            state.tick(&[]);
        }
        let mut index = UnitIndex::new();
        index.rebuild(&state.units, state.map.height());
        let reference = |y: i32, x_min: i32, x_max: i32| -> Vec<(TilePos, usize)> {
            let mut hits: Vec<(TilePos, usize)> = state
                .units
                .iter()
                .enumerate()
                .filter(|(_, u)| u.hp > 0)
                .map(|(slot, u)| (u.tile(), slot))
                .filter(|(t, _)| t.y == y && t.x >= x_min && t.x <= x_max)
                .collect();
            hits.sort_unstable_by_key(|&(t, slot)| (t.x, slot));
            hits
        };
        let mut nonempty = 0;
        for y in -1..=state.map.height() {
            for x in -1..=state.map.width() {
                let got = index.row_span(y, x - 1, x + 1);
                assert_eq!(got, &reference(y, x - 1, x + 1)[..], "window ({y}, {x})");
                nonempty += usize::from(!got.is_empty());
            }
        }
        assert!(nonempty > 0, "the fixture exercised no occupied windows");
    }
}
