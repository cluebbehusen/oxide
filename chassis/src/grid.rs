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
        self.width > 0
            && self.height > 0
            && self.cells.len() == (self.width as usize) * (self.height as usize)
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
            cells: vec![fill; (width as usize) * (height as usize)],
        }
    }

    /// Builds a grid from row-major cells. Panics if the cell count does not
    /// match the dimensions.
    pub fn from_cells(width: i32, height: i32, cells: Vec<T>) -> Self {
        assert!(width > 0 && height > 0, "grid dimensions must be positive");
        assert_eq!(
            cells.len(),
            (width as usize) * (height as usize),
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
        (pos.y as usize) * (self.width as usize) + (pos.x as usize)
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
        let base = (y as usize) * (self.width as usize);
        self.cells[base + x0 as usize..=base + x1 as usize].fill(value);
    }

    /// Row `y` as a slice, or `None` out of range — the bulk-read
    /// counterpart of [`Grid::fill_row_span`].
    pub fn row(&self, y: i32) -> Option<&[T]> {
        if y < 0 || y >= self.height {
            return None;
        }
        let base = (y as usize) * (self.width as usize);
        Some(&self.cells[base..base + self.width as usize])
    }

    /// Row `y` as a mutable slice, or `None` out of range.
    pub fn row_mut(&mut self, y: i32) -> Option<&mut [T]> {
        if y < 0 || y >= self.height {
            return None;
        }
        let base = (y as usize) * (self.width as usize);
        Some(&mut self.cells[base..base + self.width as usize])
    }

    pub fn fill(&mut self, value: T)
    where
        T: Clone,
    {
        self.cells.fill(value);
    }

    /// Iterates all cells with their positions, row-major (a deterministic
    /// order).
    pub fn iter(&self) -> impl Iterator<Item = (TilePos, &T)> {
        self.cells.iter().enumerate().map(|(i, cell)| {
            let x = (i % (self.width as usize)) as i32;
            let y = (i / (self.width as usize)) as i32;
            (TilePos::new(x, y), cell)
        })
    }

    /// Iterates all cells mutably with their positions, row-major.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (TilePos, &mut T)> {
        let width = self.width as usize;
        self.cells.iter_mut().enumerate().map(move |(i, cell)| {
            let x = (i % width) as i32;
            let y = (i / width) as i32;
            (TilePos::new(x, y), cell)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_and_containing_are_inverse_on_tile_centers() {
        for pos in [TilePos::new(0, 0), TilePos::new(3, 7), TilePos::new(41, 2)] {
            assert_eq!(TilePos::containing(pos.center()), pos);
        }
    }

    #[test]
    fn containing_floors_negative_coordinates() {
        let just_negative = Vec2Fx::new(Fx::lit("-0.25"), Fx::lit("-1.5"));
        assert_eq!(TilePos::containing(just_negative), TilePos::new(-1, -2));
    }

    #[test]
    fn grid_bounds_and_indexing() {
        let mut grid = Grid::new(4, 3, 0u8);
        assert!(grid.in_bounds(TilePos::new(3, 2)));
        assert!(!grid.in_bounds(TilePos::new(4, 0)));
        assert!(!grid.in_bounds(TilePos::new(0, -1)));
        *grid.get_mut(TilePos::new(2, 1)).unwrap() = 9;
        assert_eq!(grid.get(TilePos::new(2, 1)), Some(&9));
        assert_eq!(grid.get(TilePos::new(-1, 0)), None);
    }

    #[test]
    fn iter_is_row_major() {
        let grid = Grid::from_cells(2, 2, vec![10, 11, 12, 13]);
        let order: Vec<_> = grid.iter().map(|(p, &v)| (p.x, p.y, v)).collect();
        assert_eq!(order, vec![(0, 0, 10), (1, 0, 11), (0, 1, 12), (1, 1, 13)]);
    }
}
