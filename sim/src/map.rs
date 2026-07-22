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
use crate::stats::{RICH_SCRAP_NODE_AMOUNT, SCRAP_NODE_AMOUNT};
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
    /// Never walkable (flyable — rock is clutter, not altitude).
    Rock,
    /// A mountain: blocks ground, air, direct fire that involves
    /// aircraft, and artillery arcs — the one terrain that makes
    /// genuinely siege-safe geography.
    Peak,
}

/// One tile of the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tile {
    /// Base terrain.
    pub terrain: Terrain,
    /// Remaining scrap. A ground tile with scrap is a node: impassable and
    /// harvestable until it hits zero.
    pub scrap: u32,
    /// Battlefield salvage left by destroyed machines. Unlike a node it
    /// never blocks movement — harvesters stand *on* the tile to strip it
    /// — and it decays slowly back into the dirt.
    #[serde(default)]
    pub wreck: u32,
    /// Purely visual ground dressing (0 plain, 1 rubble). No gameplay
    /// effect, but part of the map and therefore of the state hash.
    pub cosmetic: u8,
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
                        wreck: 0,
                        cosmetic: 0,
                    },
                    ',' => Tile {
                        terrain: Terrain::Ground,
                        scrap: 0,
                        wreck: 0,
                        cosmetic: 1,
                    },
                    '#' => Tile {
                        terrain: Terrain::Rock,
                        scrap: 0,
                        wreck: 0,
                        cosmetic: 0,
                    },
                    '^' => Tile {
                        terrain: Terrain::Peak,
                        scrap: 0,
                        wreck: 0,
                        cosmetic: 0,
                    },
                    's' => Tile {
                        terrain: Terrain::Ground,
                        scrap: SCRAP_NODE_AMOUNT,
                        wreck: 0,
                        cosmetic: 0,
                    },
                    'S' => Tile {
                        terrain: Terrain::Ground,
                        scrap: RICH_SCRAP_NODE_AMOUNT,
                        wreck: 0,
                        cosmetic: 0,
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
                            wreck: 0,
                            cosmetic: 0,
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

    /// Whether the deserialized grid holds together (see
    /// [`chassis::grid::Grid::is_consistent`]).
    pub fn is_consistent(&self) -> bool {
        self.grid.is_consistent()
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

    /// The raw tile grid — row-slice access for hot scans (vision's
    /// memory reconciliation); everything else should prefer the
    /// per-tile accessors.
    pub(crate) fn grid(&self) -> &Grid<Tile> {
        &self.grid
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

    /// Battlefield salvage at `pos` (zero when out of bounds).
    pub fn wreck_at(&self, pos: TilePos) -> u32 {
        self.grid.get(pos).map_or(0, |t| t.wreck)
    }

    /// Deposits salvage on plain ground. Rock and live nodes swallow it —
    /// a machine downed where nothing can stand leaves nothing to strip.
    pub(crate) fn add_wreck(&mut self, pos: TilePos, amount: u32) {
        if let Some(tile) = self.grid.get_mut(pos)
            && tile.terrain == Terrain::Ground
            && tile.scrap == 0
        {
            tile.wreck = tile.wreck.saturating_add(amount);
        }
    }

    /// Removes one salvage from the wreck at `pos`. Returns the amount
    /// left, or `None` if there was nothing to strip.
    pub fn extract_wreck(&mut self, pos: TilePos) -> Option<u32> {
        let tile = self.grid.get_mut(pos)?;
        if tile.wreck == 0 {
            return None;
        }
        tile.wreck -= 1;
        Some(tile.wreck)
    }

    /// One decay step: every wreck tile loses one salvage. Runs on a
    /// global cadence so battlefield scrap stays a fresh-battle prize
    /// without per-tile timers in the hash.
    pub(crate) fn decay_wrecks(&mut self) {
        for (_, tile) in self.grid.iter_mut() {
            tile.wreck = tile.wreck.saturating_sub(1);
        }
    }

    /// Erases the wreck under a new foundation.
    pub(crate) fn clear_wreck(&mut self, pos: TilePos) {
        if let Some(tile) = self.grid.get_mut(pos) {
            tile.wreck = 0;
        }
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
                (Terrain::Peak, _) => '^',
                // Render-only: wrecks are never authored, so `w` stays out
                // of the parse legend.
                (Terrain::Ground, 0) if tile.wreck > 0 => 'w',
                (Terrain::Ground, 0) if tile.cosmetic == 1 => ',',
                (Terrain::Ground, 0) => '.',
                (Terrain::Ground, s) if s > SCRAP_NODE_AMOUNT => 'S',
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
        let rows = ["#.s", ",S.", "s#."];
        let (map, _) = Map::parse(&rows).unwrap();
        assert_eq!(map.ascii_rows(), rows);
    }
}
