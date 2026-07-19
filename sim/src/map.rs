//! Terrain: a tile grid parsed from ASCII art.
//!
//! Maps are authored as text on purpose — an agent (or a human) can read a
//! scenario file and see the level. Legend:
//!
//! ```text
//! .    ground
//! #    rock (impassable)
//! s    scrap node (impassable until mined out, then ground)
//! 1-8  Foundry anchor (top-left tile) for player N-1; the tile is ground
//! ```

use crate::ids::PlayerId;
use crate::stats::SCRAP_NODE_AMOUNT;
use chassis::grid::{Grid, TilePos};
use serde::{Deserialize, Serialize};

/// Largest supported map edge, in tiles.
pub const MAX_MAP_EDGE: usize = 256;

/// Base terrain of a tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Terrain {
    /// Walkable.
    Ground,
    /// Never walkable.
    Rock,
}

/// One tile of the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    /// Base terrain.
    pub terrain: Terrain,
    /// Remaining scrap. A ground tile with scrap is a node: impassable and
    /// harvestable until it hits zero.
    pub scrap: u32,
}

/// Errors from [`Map::parse`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MapError {
    /// The map had no rows or empty rows.
    #[error("map is empty")]
    Empty,
    /// The map exceeds the supported envelope. The cap keeps fixed-point
    /// squared distances and neighborhood scans comfortably inside their
    /// numeric ranges.
    #[error("map is {width}x{height}; the supported maximum is {MAX_MAP_EDGE} per side")]
    TooLarge {
        /// Parsed width.
        width: usize,
        /// Parsed height.
        height: usize,
    },
    /// Not all rows share one length.
    #[error("row {row} is {len} tiles wide, expected {expected}")]
    Ragged {
        /// Offending row index.
        row: usize,
        /// Its length.
        len: usize,
        /// Length of row zero.
        expected: usize,
    },
    /// A character outside the legend.
    #[error("unknown map character {c:?} at ({x}, {y})")]
    UnknownChar {
        /// The character.
        c: char,
        /// Column.
        x: i32,
        /// Row.
        y: i32,
    },
    /// The same player digit appeared twice.
    #[error("duplicate foundry anchor for player {0}")]
    DuplicateAnchor(PlayerId),
}

/// The playfield. Owns terrain and scrap amounts; buildings and units live
/// in [`crate::State`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Map {
    grid: Grid<Tile>,
}

impl Map {
    /// Parses ASCII rows into a map plus the Foundry anchors found, sorted
    /// by player.
    pub fn parse(rows: &[impl AsRef<str>]) -> Result<(Self, Vec<(PlayerId, TilePos)>), MapError> {
        if rows.is_empty() || rows[0].as_ref().is_empty() {
            return Err(MapError::Empty);
        }
        let expected = rows[0].as_ref().chars().count();
        if rows.len() > MAX_MAP_EDGE || expected > MAX_MAP_EDGE {
            return Err(MapError::TooLarge {
                width: expected,
                height: rows.len(),
            });
        }
        let mut cells = Vec::with_capacity(rows.len() * expected);
        let mut anchors: Vec<(PlayerId, TilePos)> = Vec::new();

        for (y, row) in rows.iter().enumerate() {
            let row = row.as_ref();
            let len = row.chars().count();
            if len != expected {
                return Err(MapError::Ragged {
                    row: y,
                    len,
                    expected,
                });
            }
            for (x, c) in row.chars().enumerate() {
                let pos = TilePos::new(x as i32, y as i32);
                let tile = match c {
                    '.' => Tile {
                        terrain: Terrain::Ground,
                        scrap: 0,
                    },
                    '#' => Tile {
                        terrain: Terrain::Rock,
                        scrap: 0,
                    },
                    's' => Tile {
                        terrain: Terrain::Ground,
                        scrap: SCRAP_NODE_AMOUNT,
                    },
                    '1'..='8' => {
                        let player = PlayerId(c as u8 - b'1');
                        if anchors.iter().any(|(p, _)| *p == player) {
                            return Err(MapError::DuplicateAnchor(player));
                        }
                        anchors.push((player, pos));
                        Tile {
                            terrain: Terrain::Ground,
                            scrap: 0,
                        }
                    }
                    other => {
                        return Err(MapError::UnknownChar {
                            c: other,
                            x: pos.x,
                            y: pos.y,
                        });
                    }
                };
                cells.push(tile);
            }
        }
        anchors.sort_by_key(|(p, _)| *p);
        Ok((
            Self {
                grid: Grid::from_cells(expected as i32, rows.len() as i32, cells),
            },
            anchors,
        ))
    }

    /// Map width in tiles.
    pub fn width(&self) -> i32 {
        self.grid.width()
    }

    /// Map height in tiles.
    pub fn height(&self) -> i32 {
        self.grid.height()
    }

    /// The tile at `pos`, if in bounds.
    pub fn tile(&self, pos: TilePos) -> Option<&Tile> {
        self.grid.get(pos)
    }

    /// Whether terrain alone allows standing on `pos` (buildings are
    /// [`crate::State`]'s concern).
    pub fn terrain_passable(&self, pos: TilePos) -> bool {
        self.grid
            .get(pos)
            .is_some_and(|t| t.terrain == Terrain::Ground && t.scrap == 0)
    }

    /// Remaining scrap at `pos` (zero when out of bounds).
    pub fn scrap_at(&self, pos: TilePos) -> u32 {
        self.grid.get(pos).map_or(0, |t| t.scrap)
    }

    /// Removes one scrap from the node at `pos`. Returns the amount left,
    /// or `None` if there was nothing to extract.
    pub fn extract_scrap(&mut self, pos: TilePos) -> Option<u32> {
        let tile = self.grid.get_mut(pos)?;
        if tile.scrap == 0 {
            return None;
        }
        tile.scrap -= 1;
        Some(tile.scrap)
    }

    /// Iterates all tiles row-major.
    pub fn iter(&self) -> impl Iterator<Item = (TilePos, &Tile)> {
        self.grid.iter()
    }

    /// Renders terrain back to the ASCII legend (buildings not included —
    /// callers overlay entities as needed).
    pub fn ascii_rows(&self) -> Vec<String> {
        let mut rows = vec![String::with_capacity(self.width() as usize); self.height() as usize];
        for (pos, tile) in self.grid.iter() {
            let c = match (tile.terrain, tile.scrap) {
                (Terrain::Rock, _) => '#',
                (Terrain::Ground, 0) => '.',
                (Terrain::Ground, _) => 's',
            };
            rows[pos.y as usize].push(c);
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legend_and_anchors() {
        let (map, anchors) = Map::parse(&["#.s", ".1.", "..2"]).unwrap();
        assert_eq!(map.width(), 3);
        assert_eq!(map.height(), 3);
        assert!(!map.terrain_passable(TilePos::new(0, 0)));
        assert!(map.terrain_passable(TilePos::new(1, 0)));
        assert!(!map.terrain_passable(TilePos::new(2, 0)), "scrap blocks");
        assert_eq!(map.scrap_at(TilePos::new(2, 0)), SCRAP_NODE_AMOUNT);
        // Anchor tiles are plain ground.
        assert!(map.terrain_passable(TilePos::new(1, 1)));
        assert_eq!(
            anchors,
            vec![
                (PlayerId(0), TilePos::new(1, 1)),
                (PlayerId(1), TilePos::new(2, 2)),
            ]
        );
    }

    #[test]
    fn depleted_node_becomes_passable() {
        let (mut map, _) = Map::parse(&["s"]).unwrap();
        let pos = TilePos::new(0, 0);
        for left in (0..SCRAP_NODE_AMOUNT).rev() {
            assert_eq!(map.extract_scrap(pos), Some(left));
        }
        assert_eq!(map.extract_scrap(pos), None);
        assert!(map.terrain_passable(pos));
    }

    #[test]
    fn rejects_ragged_and_unknown() {
        assert!(matches!(
            Map::parse(&["..", "..."]),
            Err(MapError::Ragged { row: 1, .. })
        ));
        assert!(matches!(
            Map::parse(&["..", ".x"]),
            Err(MapError::UnknownChar { c: 'x', x: 1, y: 1 })
        ));
        assert!(matches!(
            Map::parse(&["11"]),
            Err(MapError::DuplicateAnchor(PlayerId(0)))
        ));
    }

    #[test]
    fn ascii_roundtrip() {
        let rows = ["#.s", "...", "s#."];
        let (map, _) = Map::parse(&rows).unwrap();
        assert_eq!(map.ascii_rows(), rows);
    }
}
