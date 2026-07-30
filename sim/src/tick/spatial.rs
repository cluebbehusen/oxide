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
    /// `rows[y - first_row]..rows[y - first_row + 1]` bounds row `y`
    /// within `entries`.
    rows: Vec<u32>,
    /// Lowest occupied row represented by `rows`.
    first_row: i32,
}

impl UnitIndex {
    /// An empty index; [`rebuild`](Self::rebuild) before querying.
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            rows: Vec::new(),
            first_row: 0,
        }
    }

    /// Rebuilds the index over `units` (dead bodies excluded), reusing the
    /// buffers. Rows span the occupied coordinate range, including bodies
    /// fractionally beyond a map border that [`State`](crate::State)'s
    /// accepted coordinate envelope permits.
    pub(super) fn rebuild(&mut self, units: &[Unit]) {
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

        let Some(&(first, _)) = self.entries.first() else {
            self.first_row = 0;
            return;
        };
        let last_row = self.entries.last().expect("just saw an entry").0.y;
        self.first_row = first.y;
        let row_count = i64::from(last_row) - i64::from(self.first_row) + 1;
        self.rows.reserve(row_count as usize + 1);

        let mut i = 0usize;
        self.rows.push(0);
        for y in self.first_row..=last_row {
            while i < self.entries.len() && self.entries[i].0.y == y {
                i += 1;
            }
            self.rows.push(i as u32);
        }
    }

    /// The entries of row `y` with `x` in `x_min..=x_max`, in ascending
    /// `(x, slot)` order — byte-for-byte the sequence a tile-by-tile
    /// bucket walk over that span produces. Empty for unoccupied rows
    /// outside the index's current coordinate range.
    pub(super) fn row_span(&self, y: i32, x_min: i32, x_max: i32) -> &[(TilePos, usize)] {
        let row_index = i64::from(y) - i64::from(self.first_row);
        if row_index < 0 || row_index as usize + 1 >= self.rows.len() {
            return &[];
        }
        let row_index = row_index as usize;
        let row = &self.entries[self.rows[row_index] as usize..self.rows[row_index + 1] as usize];
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
        let width = state.map.width();
        let height = state.map.height();
        for (slot, tile) in [
            TilePos::new(-1, -1),
            TilePos::new(width, height),
            TilePos::new(-1, 2),
            TilePos::new(width, 3),
        ]
        .into_iter()
        .enumerate()
        {
            state.units[slot].pos = tile.center();
        }
        state
            .validate_invariants()
            .expect("the accepted coordinate envelope includes border rows");

        let mut index = UnitIndex::new();
        index.rebuild(&state.units);
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
        for y in -1..=height {
            for x in -1..=width {
                let got = index.row_span(y, x - 1, x + 1);
                assert_eq!(got, &reference(y, x - 1, x + 1)[..], "window ({y}, {x})");
                nonempty += usize::from(!got.is_empty());
            }
        }
        assert!(nonempty > 0, "the fixture exercised no occupied windows");
        assert_eq!(index.row_span(-1, i32::MIN, i32::MAX).len(), 1);
        assert_eq!(index.row_span(height, i32::MIN, i32::MAX).len(), 1);
    }
}
