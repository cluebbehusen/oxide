//! Tile positions and dense 2D grids.
//!
//! Convention shared by everything downstream: one tile is 1.0 × 1.0 world
//! units, tile `(x, y)` spans `[x, x+1) × [y, y+1)`, and its center sits at
//! `(x + 0.5, y + 0.5)`.

use crate::fx::{Fx, HALF, Vec2Fx};
use serde::{Deserialize, Serialize};

/// An integer tile coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TilePos {
    /// Column, increasing rightward.
    pub x: i32,
    /// Row, increasing downward (screen convention).
    pub y: i32,
}

/// The four cardinal neighbor offsets, in fixed iteration order.
pub const CARDINALS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// The four diagonal neighbor offsets, in fixed iteration order.
pub const DIAGONALS: [(i32, i32); 4] = [(1, 1), (1, -1), (-1, 1), (-1, -1)];

impl TilePos {
    /// Builds a tile position.
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// This tile's center in world coordinates.
    pub fn center(self) -> Vec2Fx {
        Vec2Fx::new(Fx::from_num(self.x) + HALF, Fx::from_num(self.y) + HALF)
    }

    /// The tile containing a world position.
    pub fn containing(pos: Vec2Fx) -> Self {
        // `floor` before converting: Fx-to-int conversion truncates toward
        // zero, which would round negative coordinates the wrong way.
        Self::new(pos.x.floor().to_num(), pos.y.floor().to_num())
    }

    /// Offsets by a delta.
    #[must_use]
    pub const fn offset(self, dx: i32, dy: i32) -> Self {
        Self::new(self.x + dx, self.y + dy)
    }

    /// Chebyshev (king-move) distance to `other`.
    pub fn chebyshev(self, other: Self) -> i32 {
        (self.x - other.x).abs().max((self.y - other.y).abs())
    }

    /// Manhattan distance to `other`.
    pub fn manhattan(self, other: Self) -> i32 {
        (self.x - other.x).abs() + (self.y - other.y).abs()
    }

    /// This tile's row-major index in a grid `width` tiles wide. The tile
    /// must lie inside the grid.
    #[inline]
    pub fn row_major(self, width: i32) -> usize {
        debug_assert!(
            self.x >= 0 && self.y >= 0 && self.x < width,
            "{self} lies outside a grid {width} tiles wide"
        );
        as_index(self.y) * as_index(width) + as_index(self.x)
    }

    /// The tile at row-major `index` in a grid `width` tiles wide.
    #[inline]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "an index into a grid of i32 extents yields i32 coordinates"
    )]
    pub fn from_row_major(index: usize, width: i32) -> Self {
        debug_assert!(width > 0, "a grid {width} tiles wide has no cells");
        let width = as_index(width);
        Self::new((index % width) as i32, (index / width) as i32)
    }
}

/// The number of cells in a `width` x `height` grid; zero when either extent
/// is not positive.
pub fn cell_count(width: i32, height: i32) -> usize {
    as_index(width.max(0)) * as_index(height.max(0))
}

/// A coordinate, extent or distance known to be non-negative, as an index.
#[inline]
pub fn as_index(value: i32) -> usize {
    debug_assert!(value >= 0, "negative index {value}");
    value.cast_unsigned() as usize
}

impl core::fmt::Display for TilePos {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

/// A dense row-major grid indexed by [`TilePos`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grid<T> {
    width: i32,
    height: i32,
    cells: Vec<T>,
}

impl<T> Grid<T> {
    /// Whether the deserialized shape holds together: positive dimensions
    /// and a cell vector of exactly `width x height`. Derived `Deserialize`
    /// can't check this — anything loading grids from untrusted bytes must.
    pub fn is_consistent(&self) -> bool {
        self.width > 0 && self.height > 0 && self.cells.len() == cell_count(self.width, self.height)
    }

    /// Builds a grid filled with clones of `fill`.
    pub fn new(width: i32, height: i32, fill: T) -> Self
    where
        T: Clone,
    {
        assert!(width > 0 && height > 0, "grid dimensions must be positive");
        Self {
            width,
            height,
            cells: vec![fill; cell_count(width, height)],
        }
    }

    /// Builds a grid from row-major cells. Panics if the cell count does not
    /// match the dimensions.
    pub fn from_cells(width: i32, height: i32, cells: Vec<T>) -> Self {
        assert!(width > 0 && height > 0, "grid dimensions must be positive");
        assert_eq!(
            cells.len(),
            cell_count(width, height),
            "cell count must equal width * height"
        );
        Self {
            width,
            height,
            cells,
        }
    }

    /// Grid width in tiles.
    pub fn width(&self) -> i32 {
        self.width
    }

    /// Grid height in tiles.
    pub fn height(&self) -> i32 {
        self.height
    }

    /// Whether `pos` lies inside the grid.
    pub fn in_bounds(&self, pos: TilePos) -> bool {
        pos.x >= 0 && pos.y >= 0 && pos.x < self.width && pos.y < self.height
    }

    fn index(&self, pos: TilePos) -> usize {
        pos.row_major(self.width)
    }

    /// The cell at `pos`, or `None` when out of bounds.
    pub fn get(&self, pos: TilePos) -> Option<&T> {
        self.in_bounds(pos).then(|| &self.cells[self.index(pos)])
    }

    /// Mutable access to the cell at `pos`, or `None` when out of bounds.
    pub fn get_mut(&mut self, pos: TilePos) -> Option<&mut T> {
        self.in_bounds(pos)
            .then(|| self.index(pos))
            .map(|i| &mut self.cells[i])
    }

    /// Overwrites every cell with clones of `value`.
    /// Fills `[x0, x1]` on row `y` with `value`, clamped to the grid;
    /// fully out-of-range spans are a no-op. The bulk write behind
    /// sight-disc stamping — one slice fill instead of per-cell lookups.
    pub fn fill_row_span(&mut self, y: i32, x0: i32, x1: i32, value: T)
    where
        T: Clone,
    {
        if y < 0 || y >= self.height {
            return;
        }
        let x0 = x0.max(0);
        let x1 = x1.min(self.width - 1);
        if x0 > x1 {
            return;
        }
        let first = self.index(TilePos::new(x0, y));
        let last = self.index(TilePos::new(x1, y));
        self.cells[first..=last].fill(value);
    }

    /// Copies `other` into `self`, reusing this grid's allocation when
    /// capacities allow — `Vec::clone_from` keeps the buffer where a
    /// plain clone-assign reallocates. Dimensions follow the source.
    pub fn copy_from(&mut self, other: &Grid<T>)
    where
        T: Clone,
    {
        self.width = other.width;
        self.height = other.height;
        self.cells.clone_from(&other.cells);
    }

    /// Row `y` as a slice, or `None` out of range — the bulk-read
    /// counterpart of [`Grid::fill_row_span`].
    pub fn row(&self, y: i32) -> Option<&[T]> {
        if y < 0 || y >= self.height {
            return None;
        }
        let first = self.index(TilePos::new(0, y));
        let last = self.index(TilePos::new(self.width - 1, y));
        Some(&self.cells[first..=last])
    }

    /// Row `y` as a mutable slice, or `None` out of range.
    pub fn row_mut(&mut self, y: i32) -> Option<&mut [T]> {
        if y < 0 || y >= self.height {
            return None;
        }
        let first = self.index(TilePos::new(0, y));
        let last = self.index(TilePos::new(self.width - 1, y));
        Some(&mut self.cells[first..=last])
    }

    /// Sets every cell to `value`.
    pub fn fill(&mut self, value: T)
    where
        T: Clone,
    {
        self.cells.fill(value);
    }

    /// Iterates all cells with their positions, row-major (a deterministic
    /// order).
    pub fn iter(&self) -> impl Iterator<Item = (TilePos, &T)> {
        self.cells
            .iter()
            .enumerate()
            .map(|(i, cell)| (TilePos::from_row_major(i, self.width), cell))
    }

    /// Iterates all cells mutably with their positions, row-major.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (TilePos, &mut T)> {
        let width = self.width;
        self.cells
            .iter_mut()
            .enumerate()
            .map(move |(i, cell)| (TilePos::from_row_major(i, width), cell))
    }
}

#[cfg(test)]
mod tests;
