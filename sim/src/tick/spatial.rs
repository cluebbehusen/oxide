//! A per-tick spatial index over living units — scratch for the tick
//! pipeline, never a field on [`State`](crate::State): `State`
//! serializes and hashes, so a cached index stored there would either
//! move every hash or poison the derived equality with a skipped field.
//!
//! Entries are `(tile, slot)` pairs in `(y, x, slot)` order — exactly the
//! order the collision resolver's bucket list has always used — plus an
//! offset table over the occupied rectangle, so a neighborhood query
//! slices each row once and walks it contiguously. Every window walk yields
//! candidates in the same deterministic order the full scans produced;
//! whoever consumes the index inherits that order, not a new one.

use crate::State;
use crate::state::{Building, Unit};
use chassis::grid::{TilePos, as_index, cell_count};

/// Reusable tile index over the living units of one moment.
///
/// Rebuild at every use point — positions move between pipeline phases,
/// and the collision resolver deliberately snapshots once per tick. The
/// buffers survive rebuilds, so one instance threaded through the tick
/// costs its allocations once.
pub(super) struct UnitIndex {
    /// `(tile, slot into the units vec)`, in `(y, x, slot)` order.
    entries: Vec<(TilePos, usize)>,
    /// The same pairs in slot order, before the sort.
    unordered: Vec<(TilePos, usize)>,
    /// `starts[b]..starts[b + 1]` bounds bucket `b` within `entries`: one
    /// bucket per tile or per row of the occupied rectangle, row-major.
    starts: Vec<u32>,
    layout: Layout,
    /// Lowest occupied row and column.
    origin: TilePos,
    /// Columns in the occupied rectangle; zero when no body lives.
    width: i32,
    /// Rows in the occupied rectangle; zero when no body lives.
    height: i32,
    /// Which teams own bodies and buildings where, if [`survey`](Self::survey)
    /// ran since the last rebuild.
    presence: Option<Presence>,
}

/// How [`UnitIndex::starts`] buckets the entries.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Layout {
    /// One bucket per tile, so a row lookup is two reads.
    Tiles,
    /// One bucket per row, `(x, slot)`-sorted within it, so a row lookup is
    /// two binary searches. Chosen when the occupied rectangle holds more
    /// than [`TILES_PER_BODY`] tiles per body, which keeps the table, and
    /// the work of rebuilding it, proportional to the bodies.
    Rows,
}

/// Most occupied-rectangle tiles per body the per-tile layout may cost.
/// Crowded late games sit well under it; a few units spread over a large
/// map, or a body far outside the map, sit far over.
const TILES_PER_BODY: usize = 64;

/// Edge of a presence cell, in tiles.
const CELL: i32 = 8;

/// Which teams own a body or a building in each coarse cell, as one bit per
/// team. Bodies outside the map count in the nearest border cell, so a
/// window that reaches past the map still finds them.
struct Presence {
    columns: i32,
    rows: i32,
    cells: Vec<Teams>,
    /// The first building id placed after the survey.
    next_building: u32,
}

#[derive(Clone, Copy, Default)]
struct Teams {
    bodies: u32,
    buildings: u32,
}

/// Whether a body or a building of another team may lie near a tile.
pub(super) struct HostilesNear {
    pub(super) bodies: bool,
    pub(super) buildings: bool,
}

impl Presence {
    fn cell(&self, tile: TilePos) -> (usize, usize) {
        let column = tile.x.div_euclid(CELL).clamp(0, self.columns - 1);
        let row = tile.y.div_euclid(CELL).clamp(0, self.rows - 1);
        (as_index(column), as_index(row))
    }
}

fn team_bit(team: u8) -> Option<u32> {
    1u32.checked_shl(u32::from(team))
}

impl UnitIndex {
    /// An empty index; [`rebuild`](Self::rebuild) before querying.
    pub(super) fn new() -> Self {
        Self {
            entries: Vec::new(),
            unordered: Vec::new(),
            starts: Vec::new(),
            layout: Layout::Rows,
            origin: TilePos::new(0, 0),
            width: 0,
            height: 0,
            presence: None,
        }
    }

    /// Rebuilds the index over `units` (dead bodies excluded), reusing the
    /// buffers. The rectangle spans the occupied coordinate range, including
    /// bodies beyond a map border, which [`State`]'s accepted coordinate
    /// envelope permits.
    pub(super) fn rebuild(&mut self, units: &[Unit]) {
        self.presence = None;
        self.unordered.clear();
        self.unordered.extend(
            units
                .iter()
                .enumerate()
                .filter(|(_, u)| u.hp > 0)
                .map(|(slot, u)| (u.tile(), slot)),
        );
        self.entries.clear();
        self.starts.clear();
        let Some(&(first, _)) = self.unordered.first() else {
            self.width = 0;
            self.height = 0;
            return;
        };
        let (mut low, mut high) = (first, first);
        for &(tile, _) in &self.unordered {
            low = TilePos::new(low.x.min(tile.x), low.y.min(tile.y));
            high = TilePos::new(high.x.max(tile.x), high.y.max(tile.y));
        }
        self.origin = low;
        self.width = high.x - low.x + 1;
        self.height = high.y - low.y + 1;

        let area = cell_count(self.width, self.height);
        self.layout = if area <= TILES_PER_BODY * self.unordered.len() {
            Layout::Tiles
        } else {
            Layout::Rows
        };
        let buckets = match self.layout {
            Layout::Tiles => area,
            Layout::Rows => as_index(self.height),
        };

        // Counting sort: tally each bucket, accumulate to each bucket's end,
        // then place entries in reverse slot order, stepping each bucket's
        // cursor back to its start. Slot order survives within a bucket.
        self.starts.resize(buckets + 1, 0);
        for &(tile, _) in &self.unordered {
            let at = self.bucket(tile);
            self.starts[at] += 1;
        }
        let mut end = 0;
        for start in &mut self.starts {
            end += *start;
            *start = end;
        }
        self.entries.resize(self.unordered.len(), (first, 0));
        for &(tile, slot) in self.unordered.iter().rev() {
            let at = self.bucket(tile);
            self.starts[at] -= 1;
            self.entries[self.starts[at] as usize] = (tile, slot);
        }
        if self.layout == Layout::Rows {
            for row in self.starts.windows(2) {
                self.entries[row[0] as usize..row[1] as usize]
                    .sort_unstable_by_key(|&(tile, slot)| (tile.x, slot));
            }
        }
    }

    /// The bucket holding an occupied-rectangle tile.
    fn bucket(&self, tile: TilePos) -> usize {
        let row = as_index(tile.y - self.origin.y);
        match self.layout {
            Layout::Tiles => row * as_index(self.width) + as_index(tile.x - self.origin.x),
            Layout::Rows => row,
        }
    }

    /// Records which teams own the indexed bodies and `state`'s buildings in
    /// each coarse cell, for [`hostiles_near`](Self::hostiles_near). Bodies
    /// come from the last rebuild, so the survey answers for the same moment.
    pub(super) fn survey(&mut self, state: &State) {
        self.presence = None;
        let team_bits: Option<Vec<u32>> = state
            .players()
            .iter()
            .map(|player| team_bit(player.team))
            .collect();
        let Some(team_bits) = team_bits else {
            return;
        };
        let columns = state.map.width().div_euclid(CELL).max(0) + 1;
        let rows = state.map.height().div_euclid(CELL).max(0) + 1;
        let stride = as_index(columns);
        let mut presence = Presence {
            columns,
            rows,
            cells: vec![Teams::default(); cell_count(columns, rows)],
            next_building: state.buildings().last().map_or(0, |b| b.id.0 + 1),
        };
        for &(tile, slot) in &self.entries {
            let (column, row) = presence.cell(tile);
            presence.cells[row * stride + column].bodies |=
                team_bits[state.units[slot].player.0 as usize];
        }
        for building in state.buildings() {
            let (width, height) = building.stats().size;
            let (low_column, low_row) = presence.cell(building.anchor);
            let (high_column, high_row) =
                presence.cell(building.anchor.offset(width - 1, height - 1));
            for row in low_row..=high_row {
                for column in low_column..=high_column {
                    presence.cells[row * stride + column].buildings |=
                        team_bits[building.player.0 as usize];
                }
            }
        }
        self.presence = Some(presence);
    }

    /// Whether a body, and whether a building the survey saw, of a team other
    /// than `team` may cover a tile within `reach` of `home`. Both are true
    /// without a survey.
    pub(super) fn hostiles_near(&self, team: u8, home: TilePos, reach: i32) -> HostilesNear {
        let (Some(presence), Some(own)) = (&self.presence, team_bit(team)) else {
            return HostilesNear {
                bodies: true,
                buildings: true,
            };
        };
        let (low_column, low_row) = presence.cell(home.offset(-reach, -reach));
        let (high_column, high_row) = presence.cell(home.offset(reach, reach));
        let mut teams = Teams::default();
        for row in low_row..=high_row {
            for cell in
                &presence.cells[row * as_index(presence.columns)..][low_column..=high_column]
            {
                teams.bodies |= cell.bodies;
                teams.buildings |= cell.buildings;
            }
        }
        HostilesNear {
            bodies: teams.bodies & !own != 0,
            buildings: teams.buildings & !own != 0,
        }
    }

    /// The buildings placed after the survey, which its cells do not cover;
    /// all of them without a survey. `buildings` is sorted by id.
    pub(super) fn buildings_since_survey<'a>(&self, buildings: &'a [Building]) -> &'a [Building] {
        let Some(presence) = &self.presence else {
            return buildings;
        };
        let first = buildings.partition_point(|b| b.id.0 < presence.next_building);
        &buildings[first..]
    }

    /// The entries of row `y` with `x` in `x_min..=x_max`, in ascending
    /// `(x, slot)` order — byte-for-byte the sequence a tile-by-tile
    /// bucket walk over that span produces. Empty outside the occupied
    /// rectangle.
    pub(super) fn row_span(&self, y: i32, x_min: i32, x_max: i32) -> &[(TilePos, usize)] {
        if y < self.origin.y || y >= self.origin.y + self.height {
            return &[];
        }
        match self.layout {
            Layout::Tiles => {
                let x_min = x_min.max(self.origin.x);
                let x_max = x_max.min(self.origin.x + self.width - 1);
                if x_min > x_max {
                    return &[];
                }
                let first = self.bucket(TilePos::new(x_min, y));
                let last = self.bucket(TilePos::new(x_max, y));
                &self.entries[self.starts[first] as usize..self.starts[last + 1] as usize]
            }
            Layout::Rows => {
                let at = as_index(y - self.origin.y);
                let row = &self.entries[self.starts[at] as usize..self.starts[at + 1] as usize];
                let start = row.partition_point(|&(t, _)| t.x < x_min);
                let len = row[start..].partition_point(|&(t, _)| t.x <= x_max);
                &row[start..start + len]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::Scenario;

    /// Every window the index serves must equal a brute-force filter of
    /// the unit list, entry for entry — the order is load-bearing (the
    /// collision resolver applies corrections in visit order).
    fn assert_row_spans_match_a_full_filter(state: &State, index: &UnitIndex) {
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
        let width = state.map.width();
        let height = state.map.height();
        let mut nonempty = 0;
        for y in -1..=height {
            for x in -1..=width {
                let got = index.row_span(y, x - 1, x + 1);
                assert_eq!(got, &reference(y, x - 1, x + 1)[..], "window ({y}, {x})");
                nonempty += usize::from(!got.is_empty());
            }
            let whole = index.row_span(y, i32::MIN, i32::MAX);
            assert_eq!(whole, &reference(y, i32::MIN, i32::MAX)[..], "row {y}");
        }
        assert!(nonempty > 0, "the fixture exercised no occupied windows");
    }

    /// A state whose bodies sit on both sides of the map, border rows
    /// included, so the occupied rectangle is the whole map and then some.
    fn spread_skirmish() -> State {
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
        state
    }

    #[test]
    fn row_spans_match_a_full_filter_in_either_layout() {
        let mut state = spread_skirmish();
        let mut index = UnitIndex::new();
        index.rebuild(&state.units);
        assert_eq!(index.layout, Layout::Rows, "a sparse map keeps row buckets");
        assert_row_spans_match_a_full_filter(&state, &index);

        // Pack the same bodies into a block across the corner, stacking
        // several per tile, until the rectangle is small enough for tiles.
        for (slot, unit) in state.units.iter_mut().enumerate() {
            let slot = i32::try_from(slot).unwrap() / 2;
            unit.pos = TilePos::new(slot % 3 - 1, slot / 3 - 1).center();
        }
        index.rebuild(&state.units);
        assert_eq!(index.layout, Layout::Tiles, "a crowd gets tile buckets");
        assert_row_spans_match_a_full_filter(&state, &index);
    }

    /// The accepted envelope reaches far past any map; a body out there
    /// must not make the index pay for the empty rectangle between.
    #[test]
    fn bodies_at_the_envelope_corners_keep_the_index_small() {
        let mut state = Scenario::skirmish().build().expect("skirmish builds");
        let near = TilePos::new(-2048, -2048);
        let far = TilePos::new(2047, 2047);
        state.units[0].pos = near.center();
        state.units[1].pos = far.center();
        state
            .validate_invariants()
            .expect("the envelope accepts both corners");

        let mut index = UnitIndex::new();
        index.rebuild(&state.units);
        assert_eq!(index.layout, Layout::Rows);
        assert_eq!(index.starts.len(), 4096 + 1);
        for (slot, tile) in [(0, near), (1, far)] {
            assert_eq!(index.row_span(tile.y, tile.x, tile.x), &[(tile, slot)]);
        }
    }

    /// The survey may over-report but never miss: every hostile body or
    /// building within reach of a tile must raise its flag, or acquisition
    /// would skip a target the full scan finds.
    #[test]
    fn the_survey_never_misses_a_hostile_near_a_tile() {
        let mut state = spread_skirmish();
        let width = state.map.width();
        let height = state.map.height();
        // A footprint across a cell corner must mark every cell it covers.
        let corner = TilePos::new(
            CELL * (width / CELL / 2) - 1,
            CELL * (height / CELL / 2) - 1,
        );
        state.place_site(
            crate::ids::PlayerId(1),
            crate::stats::BuildingKind::Foundry,
            corner,
        );

        let mut index = UnitIndex::new();
        index.rebuild(&state.units);
        index.survey(&state);
        let mut ruled_out = 0;
        for team in [0, 1] {
            for y in -1..=height {
                for x in -1..=width {
                    for reach in [0, 3, 9] {
                        let home = TilePos::new(x, y);
                        let near = |tile: TilePos| {
                            (tile.x - home.x).abs() <= reach && (tile.y - home.y).abs() <= reach
                        };
                        let foreign = |player| state.player(player).team != team;
                        let bodies = state
                            .units
                            .iter()
                            .any(|u| u.hp > 0 && foreign(u.player) && near(u.tile()));
                        let buildings = state
                            .buildings()
                            .iter()
                            .any(|b| foreign(b.player) && b.tiles().any(near));
                        let flagged = index.hostiles_near(team, home, reach);
                        assert!(flagged.bodies || !bodies, "body near {home:?}");
                        assert!(flagged.buildings || !buildings, "building near {home:?}");
                        ruled_out += usize::from(!flagged.bodies && !flagged.buildings);
                    }
                }
            }
        }
        assert!(ruled_out > 0, "the survey never ruled anything out");
    }

    #[test]
    fn buildings_placed_after_the_survey_are_left_to_a_full_scan() {
        let mut state = Scenario::skirmish().build().expect("skirmish builds");
        let mut index = UnitIndex::new();
        index.rebuild(&state.units);
        let all = state.buildings().len();
        assert_eq!(index.buildings_since_survey(state.buildings()).len(), all);
        index.survey(&state);
        assert!(index.buildings_since_survey(state.buildings()).is_empty());

        let anchor = TilePos::new(state.map.width() / 2, state.map.height() / 2);
        let site = state.place_site(
            crate::ids::PlayerId(1),
            crate::stats::BuildingKind::Turret,
            anchor,
        );
        let since: Vec<_> = index
            .buildings_since_survey(state.buildings())
            .iter()
            .map(|b| b.id)
            .collect();
        assert_eq!(since, [site]);

        index.rebuild(&state.units);
        assert_eq!(
            index.buildings_since_survey(state.buildings()).len(),
            all + 1
        );
        let unsurveyed = index.hostiles_near(0, anchor, 0);
        assert!(unsurveyed.bodies && unsurveyed.buildings);
    }
}
