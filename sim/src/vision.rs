//! Fog of war: per-player visibility.
//!
//! Each player owns two boolean grids. `visible` is recomputed from scratch
//! every tick — the union of vision discs around that player's units and
//! buildings. `explored` only ever accumulates. Vision is radius-based;
//! rocks do not block line of sight (a deliberate simplification, cheap and
//! predictable).
//!
//! Fog is both a presentation surface and the knowledge boundary for built-in
//! opponents. Fog-honest views expose current sight, explored terrain, ghosts,
//! remembered salvage, and anonymous radar contacts; targeted attack commands
//! still require current team sight when the simulation validates them.

use crate::ids::PlayerId;
use crate::state::State;
use crate::stats::{BuildingKind, Domain};
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::{CARDINALS, Grid, TilePos};
use chassis::path::AstarScratch;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, OnceCell, RefCell};

mod tracking;
pub use tracking::{ContactSample, ContactTrack};

/// A remembered enemy building: what its ground looked like the last time
/// this player saw it. Ghosts are beliefs, not facts — the building may be
/// long gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostBuilding {
    /// Building type as last seen.
    pub kind: BuildingKind,
    /// Whose building it was.
    pub owner: PlayerId,
    /// Footprint anchor.
    pub anchor: TilePos,
    /// Hit points at last sighting.
    pub hp: u32,
    /// Whether construction had finished at last sighting — a scouted
    /// scaffold stays a scaffold in memory until seen complete.
    #[serde(
        default = "ghost_built_default",
        skip_serializing_if = "core::clone::Clone::clone"
    )]
    pub built: bool,
}

fn ghost_built_default() -> bool {
    true
}

impl GhostBuilding {
    pub(crate) fn footprint(&self) -> impl Iterator<Item = TilePos> + use<> {
        let (w, h) = self.kind.base_stats().size;
        let anchor = self.anchor;
        (0..h).flat_map(move |dy| (0..w).map(move |dx| anchor.offset(dx, dy)))
    }
}

/// A recent hostile hit remembered by the team that suffered it.
///
/// Only the allied victim's tile is retained. The record deliberately does
/// not identify or locate the attacker, so artillery landing from fog adds
/// caution without turning damage into reconnaissance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SalvageIncident {
    /// Where the allied asset stood when damage landed.
    pub(crate) tile: TilePos,
    /// First tick on which this caution zone no longer applies.
    pub(crate) expires_at: crate::Tick,
}

/// One player's view of the map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vision {
    visible: Grid<bool>,
    explored: Grid<bool>,
    /// Remembered enemy buildings, sorted by (anchor.y, anchor.x, owner) —
    /// a deterministic canonical order like everything else in the state.
    /// The owner is part of the key, not decoration: two hostile seats can
    /// leave memories recorded under the same corner.
    #[serde(default)]
    ghosts: Vec<GhostBuilding>,
    /// Scrap per tile as this player last saw it. Only meaningful where
    /// `explored`; frozen wherever sight is lost, exactly like ghosts.
    remembered_scrap: Grid<u32>,
    /// Wreck salvage per tile as last seen — same freeze-frame rule. Kept
    /// apart from scrap memory because renderers draw them differently
    /// and the harvest brain approaches them differently.
    remembered_wreck: Grid<u32>,
    /// Radar blips: tiles holding a hostile unit or one tile of a hostile
    /// building inside an own built Array's outer ring but outside true
    /// sight. A contact without identity — no kind, no owner, no memory
    /// (rebuilt every tick).
    contacts: Vec<TilePos>,
    #[serde(default)]
    tracking: tracking::Tracking,
    /// Recent tiles where this team saw one of its own assets take damage.
    /// Sorted and deduplicated by (y, x); old snapshots predate the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    salvage_incidents: Vec<SalvageIncident>,
}

impl Vision {
    pub(crate) fn forget_building(
        &mut self,
        owner: PlayerId,
        kind: crate::stats::BuildingKind,
        anchor: TilePos,
    ) {
        self.ghosts
            .retain(|g| g.owner != owner || g.kind != kind || g.anchor != anchor);
    }

    pub(crate) fn new(width: i32, height: i32) -> Self {
        Self {
            visible: Grid::new(width, height, false),
            explored: Grid::new(width, height, false),
            ghosts: Vec::new(),
            remembered_scrap: Grid::new(width, height, 0),
            remembered_wreck: Grid::new(width, height, 0),
            contacts: Vec::new(),
            tracking: tracking::Tracking::default(),
            salvage_incidents: Vec::new(),
        }
    }

    /// Enemy buildings as last observed. Records freeze when sight is lost
    /// or a completed mine becomes concealed, even on visible ground.
    /// Draw live state only for currently observable buildings; otherwise
    /// retain this last-seen marker without exposing current condition.
    pub fn ghosts(&self) -> &[GhostBuilding] {
        &self.ghosts
    }

    /// Whether the deserialized view holds together against the map it
    /// claims to describe — see [`crate::State::validate_invariants`].
    pub fn is_consistent(&self, width: i32, height: i32) -> bool {
        let dims = |w: i32, h: i32, ok: bool| ok && w == width && h == height;
        dims(
            self.visible.width(),
            self.visible.height(),
            self.visible.is_consistent(),
        ) && dims(
            self.explored.width(),
            self.explored.height(),
            self.explored.is_consistent(),
        ) && dims(
            self.remembered_scrap.width(),
            self.remembered_scrap.height(),
            self.remembered_scrap.is_consistent(),
        ) && dims(
            self.remembered_wreck.width(),
            self.remembered_wreck.height(),
            self.remembered_wreck.is_consistent(),
        )
    }

    /// Scrap at `pos` as last seen (zero where never seen or out of
    /// bounds). Renderers should use live amounts on visible ground and
    /// this everywhere else.
    pub fn remembered_scrap(&self, pos: TilePos) -> u32 {
        self.remembered_scrap.get(pos).copied().unwrap_or(0)
    }

    /// Wreck salvage at `pos` as last seen (zero where never seen or out
    /// of bounds). Decay keeps running in the fog — this is a belief.
    pub fn remembered_wreck(&self, pos: TilePos) -> u32 {
        self.remembered_wreck.get(pos).copied().unwrap_or(0)
    }

    /// Radar blips: sorted (y, x), deduplicated, rebuilt every tick.
    pub fn contacts(&self) -> &[TilePos] {
        &self.contacts
    }

    /// Continuous mobile contacts, including identified units in true sight.
    pub fn tracks(&self) -> &[ContactTrack] {
        &self.tracking.tracks
    }

    /// A live observation identity; lost identities never resolve again.
    pub fn track(&self, id: crate::ContactId) -> Option<&ContactTrack> {
        self.tracks()
            .binary_search_by_key(&id, |track| track.id)
            .ok()
            .map(|index| &self.tracks()[index])
    }

    pub(crate) fn tracking_valid(&self, state: &State, player: PlayerId) -> bool {
        self.tracking.valid(self, state, player)
    }

    pub(crate) fn shares_tracking(&self, other: &Self) -> bool {
        self.tracking == other.tracking
    }

    pub(crate) fn minted_contact(&self, id: crate::ContactId) -> bool {
        id.0 < self.tracking.next_id
    }

    pub(crate) fn salvage_incidents(&self) -> &[SalvageIncident] {
        &self.salvage_incidents
    }

    pub(crate) fn remember_salvage_incident(&mut self, tile: TilePos, expires_at: crate::Tick) {
        let key = (tile.y, tile.x);
        match self
            .salvage_incidents
            .binary_search_by_key(&key, |incident| (incident.tile.y, incident.tile.x))
        {
            Ok(index) => {
                self.salvage_incidents[index].expires_at =
                    self.salvage_incidents[index].expires_at.max(expires_at);
            }
            Err(mut index) => {
                if self.salvage_incidents.len() == crate::stats::HARVEST_INCIDENT_CAP {
                    let evict = self
                        .salvage_incidents
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, incident)| {
                            (incident.expires_at, incident.tile.y, incident.tile.x)
                        })
                        .map(|(index, _)| index)
                        .expect("a full incident table is nonempty");
                    self.salvage_incidents.remove(evict);
                    index = self
                        .salvage_incidents
                        .binary_search_by_key(&key, |incident| (incident.tile.y, incident.tile.x))
                        .unwrap_or_else(|index| index);
                }
                self.salvage_incidents
                    .insert(index, SalvageIncident { tile, expires_at });
            }
        }
    }

    fn prune_salvage_incidents(&mut self, tick: crate::Tick) {
        self.salvage_incidents
            .retain(|incident| incident.expires_at > tick);
    }

    /// Whether the player currently sees `pos`.
    pub fn visible(&self, pos: TilePos) -> bool {
        self.visible.get(pos).copied().unwrap_or(false)
    }

    /// Drops one tile from current sight until the next vision refresh.
    #[cfg(test)]
    pub(crate) fn conceal(&mut self, pos: TilePos) {
        if let Some(visible) = self.visible.get_mut(pos) {
            *visible = false;
        }
    }

    /// Whether the player has ever seen `pos`.
    pub fn explored(&self, pos: TilePos) -> bool {
        self.explored.get(pos).copied().unwrap_or(false)
    }

    /// Copies `src` into `self` byte-for-byte while reusing this
    /// view's grid and vector allocations — the team-sight shortcut
    /// clones a whole view per teammate per tick, and a plain
    /// clone-assign reallocated four map-sized grids each time. The
    /// exhaustive destructure makes a future field a compile error
    /// here instead of silently stale team sight.
    pub(crate) fn copy_from(&mut self, src: &Vision) {
        let Vision {
            visible,
            explored,
            ghosts,
            remembered_scrap,
            remembered_wreck,
            contacts,
            tracking,
            salvage_incidents,
        } = self;
        visible.copy_from(&src.visible);
        explored.copy_from(&src.explored);
        ghosts.clone_from(&src.ghosts);
        remembered_scrap.copy_from(&src.remembered_scrap);
        remembered_wreck.copy_from(&src.remembered_wreck);
        contacts.clone_from(&src.contacts);
        tracking.clone_from(&src.tracking);
        salvage_incidents.clone_from(&src.salvage_incidents);
    }

    /// Row slices for the observation builder's full-map walk — the
    /// same sequential access `refresh` itself uses, instead of four
    /// bounds-checked point lookups per tile.
    pub(crate) fn rows(&self, y: i32) -> Option<VisionRows<'_>> {
        Some((
            self.visible.row(y)?,
            self.explored.row(y)?,
            self.remembered_scrap.row(y)?,
            self.remembered_wreck.row(y)?,
        ))
    }

    fn stamp_disc(&mut self, center: TilePos, radius: i32, coverage: &mut RowCoverage) {
        let spans = disc_spans(radius);
        for dy in -radius..=radius {
            let span = spans[dy.unsigned_abs() as usize];
            let y = center.y + dy;
            self.visible
                .fill_row_span(y, center.x - span, center.x + span, true);
            coverage.cover(y, center.x - span, center.x + span);
        }
    }

    /// Stamps the union of discs centered on every tile of a `w`x`h`
    /// footprint — the rectangle's Minkowski sum with the sight disc,
    /// written row by row. Cell-identical to stamping each footprint
    /// tile separately, without visiting the overlap four times.
    fn stamp_rect(
        &mut self,
        anchor: TilePos,
        w: i32,
        h: i32,
        radius: i32,
        coverage: &mut RowCoverage,
    ) {
        let spans = disc_spans(radius);
        for dy in -radius..(h + radius) {
            let vdist = (-dy).max(dy - (h - 1)).max(0);
            let span = spans[vdist as usize];
            let y = anchor.y + dy;
            self.visible
                .fill_row_span(y, anchor.x - span, anchor.x + (w - 1) + span, true);
            coverage.cover(y, anchor.x - span, anchor.x + (w - 1) + span);
        }
    }
}

/// Per-refresh record of which cells this team's sight stamps could have
/// touched: one bounding x-span per row. The memory-reconciliation walk
/// visits only these spans instead of the whole map. A bounding span may
/// include cells between two disjoint discs that are not actually visible —
/// the walk re-checks `visible` per cell, so coverage only ever bounds the
/// scan, never widens what counts as seen.
struct RowCoverage {
    /// `(min_x, max_x)` per row, clamped to the grid; `min > max` = untouched.
    bounds: Vec<(i32, i32)>,
    width: i32,
}

impl RowCoverage {
    fn new(width: i32, height: i32) -> Self {
        Self {
            bounds: vec![(i32::MAX, i32::MIN); height.max(0) as usize],
            width,
        }
    }

    fn reset(&mut self) {
        self.bounds.fill((i32::MAX, i32::MIN));
    }

    fn cover(&mut self, y: i32, x0: i32, x1: i32) {
        let Some(entry) = usize::try_from(y).ok().and_then(|y| self.bounds.get_mut(y)) else {
            return;
        };
        let x0 = x0.max(0);
        let x1 = x1.min(self.width - 1);
        if x0 > x1 {
            return;
        }
        entry.0 = entry.0.min(x0);
        entry.1 = entry.1.max(x1);
    }
}

/// Whether the shared fog-honest view for `viewer` justifies treating a
/// ground salvage tile as dangerous.
///
/// This is the one threat-knowledge funnel used by autonomous Harvest
/// work. Live mobile enemies are consulted only when the viewer's
/// current team vision contains their tile, and nearby friendly ground
/// firepower can screen equal or weaker pressure. Static weapons come
/// from the vision's own building memories, and unidentified radar
/// contacts matter only in a tight local ring. Recent allied impact sites
/// remain as anonymous caution zones for a short cooldown; nothing here
/// remembers a mobile enemy's identity or position after sight is lost, or
/// reads an unseen live building.
#[derive(Debug, Clone, Copy)]
struct MobileGroundPressure {
    pos: Vec2Fx,
    reach_sq: Fx,
    strength: u64,
    hostile: bool,
}

#[derive(Debug, Clone, Copy)]
struct StaticGroundPressure {
    anchor: TilePos,
    size: (i32, i32),
    reach_sq: Fx,
}

/// One row of a view's four per-tile grids, in (visible, explored,
/// remembered scrap, remembered wreck) order — the observation
/// builder's bulk-read unit.
pub(crate) type VisionRows<'a> = (&'a [bool], &'a [bool], &'a [u32], &'a [u32]);

/// Memo-lane bits for [`GroundSalvageDanger::lanes`].
mod lane {
    /// Tile sits inside some incident danger ring (stamped at capture).
    pub const INCIDENT_NEAR: u8 = 1 << 0;
    /// The contains verdict has been computed.
    pub const CONTAINS_SET: u8 = 1 << 1;
    /// The memoized contains verdict.
    pub const CONTAINS: u8 = 1 << 2;
    /// The observed-contains verdict has been computed.
    pub const OBSERVED_SET: u8 = 1 << 3;
    /// The memoized observed-contains verdict.
    pub const OBSERVED: u8 = 1 << 4;
    /// The known-ground verdict has been computed.
    pub const GROUND_SET: u8 = 1 << 5;
    /// The memoized known-ground verdict.
    pub const GROUND: u8 = 1 << 6;
}

/// One player's immutable, fog-honest salvage-danger snapshot for the
/// brain phase. Capturing once makes every A* predicate a walk over compact
/// threat records instead of repeatedly rescanning the full game state.
pub(crate) struct GroundSalvageDanger {
    width: i32,
    height: i32,
    contacts: Vec<TilePos>,
    incidents: Vec<TilePos>,
    mobile: Vec<MobileGroundPressure>,
    statics: Vec<StaticGroundPressure>,
    building_blocks: Vec<Vec<(i32, i32)>>,
    /// Per coarse cell, the threat records whose reach could cover a tile
    /// in it; built from the captured lists on the first probe.
    threat_cells: OnceCell<ThreatCells>,
    /// One byte of memo lanes per tile, replacing three separate
    /// tables and their `RefCell` borrow bookkeeping — the A*
    /// predicates probe these once per neighbor, and a `Cell` read is
    /// a plain load. The incident-near stamp is set at capture (the
    /// incident rule depends on the mover's origin, so only the
    /// outside-every-ring case caches); the contains and observed
    /// verdicts memoize on first probe; known-ground memoizes except
    /// its volatile arm (a visible tile with live scrap), which is
    /// served uncached exactly as before.
    lanes: Vec<Cell<u8>>,
    path_scratch: RefCell<AstarScratch>,
    /// What failed safe searches from outside every envelope reached this
    /// phase; see [`Self::safe_route_impossible`].
    safe_proofs: RefCell<Vec<SafeProof>>,
    #[cfg(test)]
    route_searches: Cell<usize>,
}

impl GroundSalvageDanger {
    /// Captures the threat knowledge that stays fixed throughout one brain
    /// phase. Unit damage is buffered and positions move afterward, so the
    /// snapshot is exact for every Harvester decision in that phase.
    pub(crate) fn capture(state: &State, viewer: PlayerId) -> Self {
        let vision = state.vision(viewer);
        let mobile = state
            .units
            .iter()
            .filter_map(|unit| {
                let hostile = state.hostile(viewer, unit.player);
                if hostile && !vision.visible(unit.tile()) {
                    return None;
                }
                let range = ground_weapon_reach(unit.kind.stats().weapons)?
                    + crate::stats::HARVEST_MOBILE_DANGER_MARGIN;
                let stats = unit.kind.stats();
                Some(MobileGroundPressure {
                    pos: unit.pos,
                    reach_sq: range * range,
                    strength: u64::from(stats.cost).saturating_mul(u64::from(unit.hp))
                        / u64::from(stats.max_hp),
                    hostile,
                })
            })
            .collect();
        let statics = vision
            .ghosts()
            .iter()
            .filter(|ghost| ghost.built)
            .filter_map(|ghost| {
                let range = ground_weapon_reach(ghost.kind.base_stats().weapons)?
                    + crate::stats::HARVEST_STATIC_DANGER_MARGIN;
                Some(StaticGroundPressure {
                    anchor: ghost.anchor,
                    size: ghost.kind.base_stats().size,
                    reach_sq: range * range,
                })
            })
            .collect();
        let mut building_blocks = vec![Vec::new(); state.map.height() as usize];
        let viewer_team = state.player(viewer).team;
        for building in state
            .buildings
            .iter()
            .filter(|building| !building.kind.is_stealthy() && !building.provisional)
        {
            if state.player(building.player).team == viewer_team {
                stamp_blocked_rect(
                    &mut building_blocks,
                    state.map.width(),
                    building.anchor,
                    building.stats().size,
                );
            } else {
                // A hostile structure placed during this tick's command
                // phase is not in the previous tick's ghost table yet.
                // Its currently visible tiles are still live truth; its
                // unseen footprint must remain unknown until vision
                // refresh records it.
                for tile in building.tiles().filter(|tile| vision.visible(*tile)) {
                    stamp_blocked_span(
                        &mut building_blocks,
                        state.map.width(),
                        tile.y,
                        tile.x,
                        tile.x,
                    );
                }
            }
        }
        for ghost in vision
            .ghosts()
            .iter()
            .filter(|ghost| !ghost.kind.is_stealthy())
        {
            stamp_blocked_rect(
                &mut building_blocks,
                state.map.width(),
                ghost.anchor,
                ghost.kind.base_stats().size,
            );
        }
        for row in &mut building_blocks {
            merge_spans(row);
        }
        let cell_count = (state.map.width() as usize) * (state.map.height() as usize);
        let incidents: Vec<TilePos> = vision
            .salvage_incidents()
            .iter()
            .filter(|incident| incident.expires_at > state.tick)
            .map(|incident| incident.tile)
            .collect();
        let (width, height) = (state.map.width(), state.map.height());
        let lanes: Vec<Cell<u8>> = std::iter::repeat_with(|| Cell::new(0))
            .take(cell_count)
            .collect();
        let radius = crate::stats::HARVEST_INCIDENT_DANGER_RADIUS;
        for incident in &incidents {
            for y in (incident.y - radius).max(0)..=(incident.y + radius).min(height - 1) {
                for x in (incident.x - radius).max(0)..=(incident.x + radius).min(width - 1) {
                    let cell = &lanes[(y * width + x) as usize];
                    cell.set(cell.get() | lane::INCIDENT_NEAR);
                }
            }
        }
        Self {
            width,
            height,
            contacts: vision.contacts().to_vec(),
            incidents,
            mobile,
            statics,
            building_blocks,
            threat_cells: OnceCell::new(),
            lanes,
            path_scratch: RefCell::new(AstarScratch::default()),
            safe_proofs: RefCell::new(Vec::new()),
            #[cfg(test)]
            route_searches: Cell::new(0),
        }
    }

    /// The threat cells, built on first use: many snapshots are never
    /// probed at all.
    fn threat_cells(&self) -> &ThreatCells {
        self.threat_cells.get_or_init(|| self.index_threats())
    }

    /// Files every threat record under each coarse cell its reach could
    /// cover. Each box is a conservative superset of the tiles whose centers
    /// the record's exact test can accept, so a probe that walks only its
    /// cell's records reaches the same verdict as a walk over all of them.
    fn index_threats(&self) -> ThreatCells {
        let (columns, rows) = (danger_cells(self.width), danger_cells(self.height));
        let radar = crate::stats::HARVEST_RADAR_DANGER_RADIUS;
        let contacts = CellLists::build(
            columns,
            rows,
            self.contacts
                .iter()
                .map(|&contact| (contact.offset(-radar, -radar), contact.offset(radar, radar))),
        );
        let mobile = CellLists::build(
            columns,
            rows,
            self.mobile.iter().map(|pressure| {
                let home = TilePos::containing(pressure.pos);
                let reach = tile_reach(pressure.reach_sq) + 1;
                (home.offset(-reach, -reach), home.offset(reach, reach))
            }),
        );
        let statics = CellLists::build(
            columns,
            rows,
            self.statics.iter().map(|pressure| {
                let reach = tile_reach(pressure.reach_sq);
                (
                    pressure.anchor.offset(-reach - 1, -reach - 1),
                    pressure
                        .anchor
                        .offset(pressure.size.0 + reach, pressure.size.1 + reach),
                )
            }),
        );
        ThreatCells {
            contacts,
            mobile,
            statics,
        }
    }

    /// The coarse cell holding `tile`; off-map tiles share the nearest
    /// border cell, as off-map record boxes do.
    fn cell_of(&self, tile: TilePos) -> usize {
        let columns = danger_cells(self.width);
        let column = tile.x.div_euclid(DANGER_CELL).clamp(0, columns - 1);
        let row = tile
            .y
            .div_euclid(DANGER_CELL)
            .clamp(0, danger_cells(self.height) - 1);
        (row * columns + column) as usize
    }

    /// Whether this snapshot marks one tile as too dangerous for
    /// autonomous salvage work.
    pub(crate) fn contains(&self, source: TilePos) -> bool {
        self.lane_memo(source, lane::CONTAINS_SET, lane::CONTAINS, || {
            self.compute_contains(source)
        })
    }

    /// Whether an autonomous route may traverse `tile` from its current
    /// planning origin. Live threats and radar remain hard barriers. A worker
    /// inside remembered static fire or an incident ring may move laterally
    /// or outward; a worker outside cannot enter either envelope.
    pub(crate) fn route_safe_from(&self, from: TilePos, tile: TilePos) -> bool {
        // One bounds test and one byte load serve both the observed
        // memo and the incident-near stamp.
        let index = self.lane_index(tile);
        let bits = index.map_or(0, |i| self.lanes[i].get());
        let observed = self.observed_at(tile, index, bits);
        if observed && self.mobile_or_radar_contains(tile) {
            return false;
        }
        if observed {
            let from_point = from.center();
            let next_point = tile.center();
            let statics = self.threat_cells().statics.at(self.cell_of(tile));
            if statics
                .iter()
                .map(|&i| &self.statics[i as usize])
                .any(|pressure| {
                    let next_distance =
                        rect_closest_point(pressure.anchor, pressure.size, next_point)
                            .dist_sq(next_point);
                    next_distance <= pressure.reach_sq
                        && next_distance
                            < rect_closest_point(pressure.anchor, pressure.size, from_point)
                                .dist_sq(from_point)
                })
            {
                return false;
            }
        }
        if index.is_some() && bits & lane::INCIDENT_NEAR == 0 {
            return true;
        }
        !self.incidents.iter().any(|incident| {
            let next_distance = incident.chebyshev(tile);
            next_distance <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
                && next_distance < incident.chebyshev(from)
        })
    }

    /// Memoized observed-danger test for one tile, given its lane index
    /// and current lane bits.
    fn observed_at(&self, tile: TilePos, index: Option<usize>, bits: u8) -> bool {
        if let Some(i) = index {
            if bits & lane::OBSERVED_SET != 0 {
                bits & lane::OBSERVED != 0
            } else {
                let value = self.compute_observed_contains(tile);
                let cell = &self.lanes[i];
                cell.set(cell.get() | lane::OBSERVED_SET | if value { lane::OBSERVED } else { 0 });
                value
            }
        } else {
            self.compute_observed_contains(tile)
        }
    }

    /// Whether a building occupies this tile in the viewer's shared
    /// knowledge. The row spans are captured once so every A* expansion
    /// avoids a full building and ghost scan.
    pub(crate) fn known_building_blocked(&self, tile: TilePos) -> bool {
        let Some(row) = usize::try_from(tile.y)
            .ok()
            .and_then(|row| self.building_blocks.get(row))
        else {
            return false;
        };
        let index = row.partition_point(|&(_, end)| end < tile.x);
        row.get(index)
            .is_some_and(|&(start, end)| tile.x >= start && tile.x <= end)
    }

    /// Runs one behavior-identical A* query while reusing this team phase's
    /// allocation storage. Brain phases are sequential, so one scratch arena
    /// serves every Harvester without entering deterministic state.
    pub(crate) fn find_route(
        &self,
        start: TilePos,
        goal: TilePos,
        passable: impl FnMut(TilePos) -> bool,
    ) -> Option<Vec<TilePos>> {
        #[cfg(test)]
        self.route_searches.set(self.route_searches.get() + 1);
        chassis::path::astar_with_scratch(
            self.width,
            self.height,
            start,
            goal,
            passable,
            crate::stats::PATH_EXPANSION_CAP,
            &mut self.path_scratch.borrow_mut(),
        )
    }

    /// Whether `from` lies outside every remembered static envelope and
    /// incident ring. From such an origin [`Self::route_safe_from`] refuses
    /// exactly the tiles [`Self::contains`] marks, so every safe search from
    /// one shares a single passability rule.
    pub(crate) fn outside_every_envelope(&self, from: TilePos) -> bool {
        let point = from.center();
        let in_static = self
            .threat_cells()
            .statics
            .at(self.cell_of(from))
            .iter()
            .map(|&i| &self.statics[i as usize])
            .any(|pressure| {
                rect_closest_point(pressure.anchor, pressure.size, point).dist_sq(point)
                    <= pressure.reach_sq
            });
        let in_ring = self.incidents.iter().any(|incident| {
            incident.chebyshev(from) <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
        });
        !in_static && !in_ring
    }

    /// Keeps what the search that just failed reached, when it explored
    /// its whole component: every later safe search starting inside that
    /// component reaches exactly it. `may_drain` marks closed ground that
    /// could open later in this phase (a visible node with live scrap);
    /// such tiles beside the component are watched, since one opening could
    /// join more ground to it.
    ///
    /// Only a component covering at least an eighth of the map is kept: a
    /// proof costs a walk over the whole map, which a small component's
    /// repeat searches would not repay. That bounds the walk by eight times
    /// the search it follows; and since a search from inside a kept
    /// component never fails into a new proof, at most eight are kept at
    /// once.
    pub(crate) fn record_safe_failure(&self, may_drain: impl Fn(TilePos) -> bool) {
        let scratch = self.path_scratch.borrow();
        let area = self.width as usize * self.height as usize;
        if !scratch.last_search_exhausted() || (scratch.last_expansions() as usize) * 8 < area {
            return;
        }
        let mut reached = vec![0u64; (self.width as usize * self.height as usize).div_ceil(64)];
        let mut watched = Vec::new();
        for y in 0..self.height {
            for x in 0..self.width {
                let tile = TilePos::new(x, y);
                if !scratch.last_search_reached(tile) {
                    continue;
                }
                let i = self.lane_index(tile).expect("in bounds");
                reached[i / 64] |= 1 << (i % 64);
                for (dx, dy) in CARDINALS {
                    let next = tile.offset(dx, dy);
                    if !scratch.last_search_reached(next) && may_drain(next) {
                        watched.push(next);
                    }
                }
            }
        }
        self.safe_proofs
            .borrow_mut()
            .push(SafeProof { reached, watched });
    }

    /// Whether a recorded failure proves that a safe search from `from`, an
    /// origin outside every envelope, cannot reach `goal` (or, with
    /// `allow_goal_only`, any cardinal neighbor of it). A proof applies when
    /// it reached `from`, and lapses once `drained` reports a watched tile
    /// open. Adjacent goals are left to the search, which can step onto
    /// them from a closed origin.
    pub(crate) fn safe_route_impossible(
        &self,
        from: TilePos,
        goal: TilePos,
        allow_goal_only: bool,
        drained: impl Fn(TilePos) -> bool,
    ) -> bool {
        if from.chebyshev(goal) <= 1 {
            return false;
        }
        let mut proofs = self.safe_proofs.borrow_mut();
        proofs.retain(|proof| !proof.watched.iter().any(|&tile| drained(tile)));
        let Some(proof) = proofs
            .iter()
            .find(|proof| self.lane_index(from).is_some_and(|i| proof.reaches(i)))
        else {
            return false;
        };
        let reached = |tile: TilePos| self.lane_index(tile).is_some_and(|i| proof.reaches(i));
        !(reached(goal)
            || (allow_goal_only
                && CARDINALS
                    .into_iter()
                    .any(|(dx, dy)| reached(goal.offset(dx, dy)))))
    }

    #[cfg(test)]
    pub(crate) fn route_search_count(&self) -> usize {
        self.route_searches.get()
    }

    /// Reachability of alternate goals proved by the most recent exhausted
    /// route search. `allow_goal_only` handles a goal-specific exception to
    /// the common passability predicate: an alternate goal can be entered iff
    /// the explored component reaches one of its cardinal neighbors. (A legal
    /// diagonal entry also makes a cardinal companion reachable.) `None`
    /// means the search did not explore the complete component.
    pub(crate) fn last_route_reachability(
        &self,
        goals: &[TilePos],
        allow_goal_only: bool,
    ) -> Option<Vec<bool>> {
        let scratch = self.path_scratch.borrow();
        scratch.last_search_exhausted().then(|| {
            goals
                .iter()
                .map(|goal| {
                    scratch.last_search_reached(*goal)
                        || (allow_goal_only
                            && CARDINALS
                                .into_iter()
                                .any(|(dx, dy)| scratch.last_search_reached(goal.offset(dx, dy))))
                })
                .collect()
        })
    }

    fn compute_contains(&self, source: TilePos) -> bool {
        if self.incidents.iter().any(|incident| {
            incident.chebyshev(source) <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
        }) {
            return true;
        }
        self.compute_observed_contains(source)
    }

    fn compute_observed_contains(&self, source: TilePos) -> bool {
        if self.mobile_or_radar_contains(source) {
            return true;
        }
        let source_point = source.center();
        let statics = self.threat_cells().statics.at(self.cell_of(source));
        statics
            .iter()
            .map(|&i| &self.statics[i as usize])
            .any(|pressure| {
                rect_closest_point(pressure.anchor, pressure.size, source_point)
                    .dist_sq(source_point)
                    <= pressure.reach_sq
            })
    }

    fn mobile_or_radar_contains(&self, source: TilePos) -> bool {
        let cell = self.cell_of(source);
        let cells = self.threat_cells();
        if cells.contacts.at(cell).iter().any(|&i| {
            self.contacts[i as usize].chebyshev(source) <= crate::stats::HARVEST_RADAR_DANGER_RADIUS
        }) {
            return true;
        }
        // Strengths are non-negative, so a saturating sum over any subset
        // holding every in-reach pressure equals the sum over all of them.
        let source_point = source.center();
        let (mut hostile_strength, mut screen_strength) = (0u64, 0u64);
        for pressure in cells
            .mobile
            .at(cell)
            .iter()
            .map(|&i| &self.mobile[i as usize])
        {
            if pressure.pos.dist_sq(source_point) > pressure.reach_sq {
                continue;
            }
            if pressure.hostile {
                hostile_strength = hostile_strength.saturating_add(pressure.strength);
            } else {
                screen_strength = screen_strength.saturating_add(pressure.strength);
            }
        }
        hostile_strength > screen_strength
    }
}

impl GroundSalvageDanger {
    /// Serves the known-ground probe through the per-tile memo. The
    /// closure returns `(open, volatile)`; volatile verdicts are
    /// handed back but never stored, so a visible scrap tile whose
    /// node can deplete mid-phase re-evaluates on every probe while
    /// everything else pays the full walk exactly once.
    pub(crate) fn known_ground_cached(
        &self,
        tile: TilePos,
        compute: impl FnOnce() -> (bool, bool),
    ) -> bool {
        let Some(index) = self.lane_index(tile) else {
            return compute().0;
        };
        let cell = &self.lanes[index];
        let bits = cell.get();
        if bits & lane::GROUND_SET != 0 {
            return bits & lane::GROUND != 0;
        }
        let (open, volatile) = compute();
        if !volatile {
            cell.set(bits | lane::GROUND_SET | if open { lane::GROUND } else { 0 });
        }
        open
    }

    /// The packed index of an in-bounds tile.
    fn lane_index(&self, tile: TilePos) -> Option<usize> {
        ((0..self.width).contains(&tile.x) && (0..self.height).contains(&tile.y))
            .then(|| (tile.y as usize) * (self.width as usize) + tile.x as usize)
    }

    /// Serves one boolean verdict through its memo lane pair; out of
    /// bounds computes uncached, like the tables it replaces.
    fn lane_memo(&self, tile: TilePos, set: u8, value: u8, compute: impl FnOnce() -> bool) -> bool {
        let Some(index) = self.lane_index(tile) else {
            return compute();
        };
        let cell = &self.lanes[index];
        let bits = cell.get();
        if bits & set != 0 {
            return bits & value != 0;
        }
        let verdict = compute();
        cell.set(bits | set | if verdict { value } else { 0 });
        verdict
    }
}

/// Edge of a [`GroundSalvageDanger`] threat cell, in tiles.
const DANGER_CELL: i32 = 8;

fn danger_cells(tiles: i32) -> i32 {
    tiles.div_euclid(DANGER_CELL).max(0) + 1
}

/// The smallest whole tile count at least as long as `sqrt(reach_sq)`.
fn tile_reach(reach_sq: Fx) -> i32 {
    let mut reach = 0;
    while Fx::from_num(reach * reach) < reach_sq {
        reach += 1;
    }
    reach
}

/// One failed safe search's reach: a whole cardinal component, plus the
/// drainable tiles bordering it.
struct SafeProof {
    reached: Vec<u64>,
    watched: Vec<TilePos>,
}

impl SafeProof {
    fn reaches(&self, index: usize) -> bool {
        self.reached[index / 64] >> (index % 64) & 1 == 1
    }
}

/// [`CellLists`] for each kind of threat record.
struct ThreatCells {
    contacts: CellLists,
    mobile: CellLists,
    statics: CellLists,
}

/// Record indices filed per coarse cell, ascending within each cell.
struct CellLists {
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl CellLists {
    /// Files record `i` under every cell its inclusive tile box overlaps,
    /// clamping boxes that run past the map onto the border cells.
    fn build(
        columns: i32,
        rows: i32,
        boxes: impl Iterator<Item = (TilePos, TilePos)> + Clone,
    ) -> Self {
        let cells = |(low, high): (TilePos, TilePos)| {
            let clamp_column = |x: i32| x.div_euclid(DANGER_CELL).clamp(0, columns - 1);
            let clamp_row = |y: i32| y.div_euclid(DANGER_CELL).clamp(0, rows - 1);
            (clamp_row(low.y)..=clamp_row(high.y)).flat_map(move |row| {
                (clamp_column(low.x)..=clamp_column(high.x))
                    .map(move |column| (row * columns + column) as usize)
            })
        };
        let mut starts = vec![0u32; (columns * rows) as usize + 1];
        for cell in boxes.clone().flat_map(cells) {
            starts[cell + 1] += 1;
        }
        for cell in 1..starts.len() {
            starts[cell] += starts[cell - 1];
        }
        let mut cursors = starts.clone();
        let mut items = vec![0u32; starts[starts.len() - 1] as usize];
        for (record, bounds) in boxes.enumerate() {
            for cell in cells(bounds) {
                items[cursors[cell] as usize] = record as u32;
                cursors[cell] += 1;
            }
        }
        Self { starts, items }
    }

    fn at(&self, cell: usize) -> &[u32] {
        &self.items[self.starts[cell] as usize..self.starts[cell + 1] as usize]
    }
}

fn stamp_blocked_rect(
    rows: &mut [Vec<(i32, i32)>],
    map_width: i32,
    anchor: TilePos,
    size: (i32, i32),
) {
    for y in anchor.y..anchor.y + size.1 {
        stamp_blocked_span(rows, map_width, y, anchor.x, anchor.x + size.0 - 1);
    }
}

fn stamp_blocked_span(rows: &mut [Vec<(i32, i32)>], map_width: i32, y: i32, start: i32, end: i32) {
    let Some(row) = usize::try_from(y).ok().and_then(|y| rows.get_mut(y)) else {
        return;
    };
    let start = start.max(0);
    let end = end.min(map_width - 1);
    if start <= end {
        row.push((start, end));
    }
}

fn merge_spans(row: &mut Vec<(i32, i32)>) {
    row.sort_unstable();
    let mut write = 0;
    for read in 0..row.len() {
        let (start, end) = row[read];
        if write > 0 && start <= row[write - 1].1.saturating_add(1) {
            row[write - 1].1 = row[write - 1].1.max(end);
        } else {
            row[write] = (start, end);
            write += 1;
        }
    }
    row.truncate(write);
}

fn ground_weapon_reach(weapons: &[crate::stats::WeaponStats]) -> Option<Fx> {
    weapons
        .iter()
        .filter(|weapon| weapon.targets.covers(Domain::Ground))
        .map(|weapon| weapon.range)
        .max()
}

fn rect_closest_point(anchor: TilePos, size: (i32, i32), from: Vec2Fx) -> Vec2Fx {
    let min = anchor.center() - Vec2Fx::new(HALF, HALF);
    let max = min + Vec2Fx::new(Fx::from_num(size.0), Fx::from_num(size.1));
    Vec2Fx::new(from.x.clamp(min.x, max.x), from.y.clamp(min.y, max.y))
}

/// Horizontal half-spans of a sight disc, per |dy|: `spans[d]` is the
/// widest `dx` with `dx*dx + d*d <= r*r`. Built once per process for
/// every radius the stats can name — integer math, no libm.
fn disc_spans(radius: i32) -> &'static [i32] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<Vec<Vec<i32>>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..=32i32)
            .map(|r| {
                (0..=r)
                    .map(|dy| {
                        let mut span = r;
                        while span * span + dy * dy > r * r {
                            span -= 1;
                        }
                        span
                    })
                    .collect()
            })
            .collect()
    });
    &table[radius as usize]
}

/// Forget a building only when its removal itself was observable.
pub(crate) fn forget_observed_building(state: &mut State, id: crate::BuildingId, detonated: bool) {
    let Some(b) = state.building(id) else {
        return;
    };
    let (owner, kind, anchor) = (b.player, b.kind, b.anchor);
    let viewers: Vec<_> = (0..state.players.len())
        .filter(|&index| {
            let viewer = PlayerId(index as u8);
            b.tiles().any(|t| state.vision(viewer).visible(t))
                && (detonated || state.building_apparent(viewer, b))
        })
        .collect();
    for index in viewers {
        state.vision[index].forget_building(owner, kind, anchor);
    }
}

/// Rebuilds every player's `visible` set from their live entities, then
/// reconciles their building memory against what is now in sight.
pub(crate) fn refresh(state: &mut State) {
    let mut vision = std::mem::take(&mut state.vision);
    let mut coverage = RowCoverage::new(state.map.width(), state.map.height());
    let mut eyes: Vec<(TilePos, i32)> = Vec::new();
    let sightings = Sighting::gather(state);
    for index in 0..vision.len() {
        // Team sight is seat-symmetric by construction: every teammate
        // stamps the same discs, reconciles the same memories, hears
        // the same radar. A later seat on an already-computed team is
        // a byte-for-byte clone — half the refresh on team maps.
        if let Some(src) = (0..index).find(|&j| state.players[j].team == state.players[index].team)
        {
            let (head, tail) = vision.split_at_mut(index);
            tail[0].copy_from(&head[src]);
            continue;
        }
        let view = &mut vision[index];
        view.prune_salvage_incidents(state.tick);
        let my_team = state.players[index].team;
        let allied = |p: PlayerId| state.players[p.0 as usize].team == my_team;
        view.visible.fill(false);
        coverage.reset();
        // Team sight: every teammate's eyes stamp into this view. A disc
        // lies inside any wider one around the same tile, and crowds share
        // tiles, so only the widest disc per tile is stamped.
        eyes.clear();
        eyes.extend(
            sightings
                .iter()
                .filter(|unit| unit.team == my_team)
                .map(|unit| (unit.tile, unit.vision)),
        );
        eyes.sort_unstable_by_key(|&(tile, radius)| (tile.y, tile.x, std::cmp::Reverse(radius)));
        eyes.dedup_by_key(|&mut (tile, _)| tile);
        for &(tile, radius) in &eyes {
            view.stamp_disc(tile, radius, &mut coverage);
        }
        // Sites don't see: a pile of parts has no sensors.
        for building in state
            .buildings
            .iter()
            .filter(|b| allied(b.player) && b.built)
        {
            let (w, h) = building.stats().size;
            view.stamp_rect(
                building.anchor,
                w,
                h,
                building.stats().vision,
                &mut coverage,
            );
        }

        // Memory reconciliation. Wherever we have sight, live state is the
        // truth: drop every record on visible ground, then re-record every
        // enemy building actually seen there (fresh hp). A building seen
        // *gone* thus loses its record, and a record on unseen ground
        // freezes at its last sighting.
        let mut ghosts = std::mem::take(&mut view.ghosts);
        ghosts.retain(|ghost| {
            if !ghost.footprint().any(|t| view.visible(t)) {
                return true;
            }
            // Ordinary sight cannot prove that a buried mine has vanished.
            // A visible replacement from the mine's team proves its removal:
            // that team cannot place over its own charge. Hostile scaffolds
            // can overlap a concealed mine and do not disprove the memory.
            ghost.kind.is_stealthy()
                && !state.charge_detected_at(PlayerId(index as u8), ghost.anchor)
                && !state.buildings.iter().any(|b| {
                    b.contains(ghost.anchor)
                        && !state.hostile(ghost.owner, b.player)
                        && state.building_apparent(PlayerId(index as u8), b)
                })
        });
        for building in state.buildings.iter().filter(|b| !allied(b.player)) {
            // An undetected buried charge never enters memory: sight of
            // its tile alone is not knowledge of it (the one stealth
            // rule; see `State::building_apparent`).
            if !state.building_apparent(PlayerId(index as u8), building) {
                continue;
            }
            if building.tiles().any(|t| view.visible(t)) {
                ghosts.push(GhostBuilding {
                    kind: building.kind,
                    owner: building.player,
                    anchor: building.anchor,
                    hp: building.hp,
                    built: building.built,
                });
            }
        }
        ghosts.sort_unstable_by_key(|g| (g.anchor.y, g.anchor.x, g.owner));
        view.ghosts = ghosts;

        // Freeze-frame the economy the same way: wherever there is sight,
        // remember the salvage; everywhere else the old numbers stand.
        // Row slices, not per-cell lookups, both memories in one walk —
        // and only inside the x-spans this team's stamps could have
        // touched, since nothing outside them became visible this tick.
        // The per-cell `seen` check still decides; the coverage bounds
        // only shrink the walk.
        for y in 0..state.map.height() {
            let (x0, x1) = coverage.bounds[y as usize];
            if x0 > x1 {
                continue;
            }
            let (x0, x1) = (x0 as usize, x1 as usize);
            let visible = &view.visible.row(y).expect("row in range")[x0..=x1];
            let tiles = &state.map.grid().row(y).expect("row in range")[x0..=x1];
            let scrap = &mut view.remembered_scrap.row_mut(y).expect("row in range")[x0..=x1];
            let wreck = &mut view.remembered_wreck.row_mut(y).expect("row in range")[x0..=x1];
            let explored = &mut view.explored.row_mut(y).expect("row in range")[x0..=x1];
            for (x, (&seen, tile)) in visible.iter().zip(tiles).enumerate() {
                if seen {
                    // Explored accumulates here instead of in the sight
                    // stamps: the stamps wrote the same spans to two
                    // grids per row, and this walk already touches
                    // every newly visible cell exactly once.
                    explored[x] = true;
                    scrap[x] = tile.scrap;
                    wreck[x] = tile.wreck;
                }
            }
        }

        // Radar blips: hostile units and buildings inside any own built
        // Array's outer ring, on ground this player cannot actually see. A
        // tile only — detection is not identification, and there is no
        // memory: a contact that leaves the ring is simply gone.
        view.contacts.clear();
        let masts: Vec<TilePos> = state
            .buildings
            .iter()
            .filter(|b| allied(b.player) && b.built && b.kind == BuildingKind::Array)
            .map(|b| b.anchor)
            .collect();
        if !masts.is_empty() {
            let r = crate::stats::RADAR_DETECT_RADIUS;
            let ring_distance = |t: TilePos| {
                masts
                    .iter()
                    .map(|m| {
                        let (dx, dy) = (t.x - m.x, t.y - m.y);
                        dx * dx + dy * dy
                    })
                    .min()
                    .filter(|&d| d <= r * r)
            };
            for unit in sightings.iter().filter(|unit| unit.team != my_team) {
                let t = unit.tile;
                if !view.visible(t) && ring_distance(t).is_some() {
                    view.contacts.push(t);
                }
            }
            // A building returns one blip, like a unit of any size. An
            // undetected charge or hostile provisional site is not apparent
            // and returns nothing.
            let viewer = PlayerId(index as u8);
            for b in state
                .buildings
                .iter()
                .filter(|b| !allied(b.player) && state.building_apparent(viewer, b))
            {
                if b.tiles().any(|t| view.visible(t)) {
                    continue;
                }
                if let Some(t) = radar_return(state, b, ring_distance) {
                    view.contacts.push(t);
                }
            }
            view.contacts.sort_unstable_by_key(|t| (t.y, t.x));
            view.contacts.dedup();
        }
        let mut tracking = std::mem::take(&mut view.tracking);
        tracking.refresh(view, state, PlayerId(index as u8), &sightings);
        view.tracking = tracking;
    }
    state.vision = vision;
}

/// One unit as each view's refresh reads it: gathered once per refresh
/// instead of once per view.
pub(super) struct Sighting {
    pub(super) id: crate::UnitId,
    pub(super) tile: TilePos,
    pub(super) team: u8,
    vision: i32,
}

impl Sighting {
    pub(super) fn gather(state: &State) -> Vec<Self> {
        state
            .units
            .iter()
            .map(|unit| Self {
                id: unit.id,
                tile: unit.tile(),
                team: state.players[unit.player.0 as usize].team,
                vision: unit.kind.stats().vision,
            })
            .collect()
    }
}

/// The footprint tile a building's radar return reports: the one nearest a
/// mast, with distance ties ranked in the footprint's radial map frame. A
/// footprint centered on the map has no radial frame, so its remaining ties
/// rank toward the owner's first Foundry and then by seat parity, the fallback
/// order group spreads use. Mirrored seats therefore report mirrored tiles.
fn radar_return(
    state: &State,
    building: &crate::state::Building,
    ring_distance: impl Fn(TilePos) -> Option<i32>,
) -> Option<TilePos> {
    let map_size = (state.map.width(), state.map.height());
    let (anchor, size) = (building.anchor, building.stats().size);
    let ranked: Vec<_> = building
        .tiles()
        .filter_map(|t| {
            let radial = crate::geometry::spawn_doorstep_key(map_size, anchor, size, t);
            ring_distance(t).map(|d| ((d, radial), t))
        })
        .collect();
    let best = ranked.iter().map(|&(key, _)| key).min()?;
    let tied: Vec<TilePos> = ranked
        .into_iter()
        .filter(|&(key, _)| key == best)
        .map(|(_, t)| t)
        .collect();
    if let [only] = tied[..] {
        return Some(only);
    }
    // Doubled coordinates keep even footprint centers exact.
    let center = (
        i64::from(anchor.x) * 2 + i64::from(size.0),
        i64::from(anchor.y) * 2 + i64::from(size.1),
    );
    let toward_home = state
        .buildings
        .iter()
        .filter(|b| {
            b.player == building.player && !b.provisional && b.kind == BuildingKind::Foundry
        })
        .min_by_key(|b| b.id)
        .map(|foundry| {
            let (w, h) = foundry.stats().size;
            (
                i64::from(foundry.anchor.x) * 2 + i64::from(w) - center.0,
                i64::from(foundry.anchor.y) * 2 + i64::from(h) - center.1,
            )
        })
        .filter(|&ray| ray != (0, 0));
    match toward_home {
        Some((rx, ry)) => tied.into_iter().max_by_key(|t| {
            let (cx, cy) = (
                i64::from(t.x) * 2 + 1 - center.0,
                i64::from(t.y) * 2 + 1 - center.1,
            );
            (rx * cx + ry * cy, rx * cy - ry * cx)
        }),
        None if building.player.0 % 2 == 1 => tied.last().copied(),
        None => tied.first().copied(),
    }
}

/// Initialize pre-contact snapshots from their validated, stored observations.
pub(crate) fn initialize_legacy_tracking(state: &mut State) -> bool {
    let mut vision = std::mem::take(&mut state.vision);
    let mut initialized = false;
    let sightings = Sighting::gather(state);
    for (index, view) in vision.iter_mut().enumerate() {
        if view.tracking.next_id == 0 && view.tracking.tracks.is_empty() {
            let mut tracking = std::mem::take(&mut view.tracking);
            tracking.refresh(view, state, PlayerId(index as u8), &sightings);
            initialized |= tracking.next_id != 0;
            view.tracking = tracking;
        }
    }
    state.vision = vision;
    initialized
}

#[cfg(test)]
mod sight_tests {
    use super::*;
    use crate::{Scenario, UnitKind};

    #[test]
    fn eyes_sharing_a_tile_see_what_the_widest_sees() {
        let (narrow, wide) = (UnitKind::Bombard, UnitKind::Kestrel);
        assert!(narrow.stats().vision < wide.stats().vision);
        let visible = |kinds: &[UnitKind]| {
            let mut state = Scenario::skirmish().build().expect("skirmish builds");
            state.units.clear();
            let tile = TilePos::new(state.map.width() / 2, state.map.height() / 2);
            for &kind in kinds {
                state.spawn_unit(PlayerId(0), kind, tile.center());
            }
            state.refresh_vision();
            state.vision(PlayerId(0)).visible.clone()
        };
        let widest = visible(&[wide]);
        assert_eq!(visible(&[narrow, wide]), widest);
        assert_eq!(visible(&[wide, narrow]), widest);
        assert_ne!(visible(&[narrow]), widest);
    }
}

#[cfg(test)]
mod danger_tests {
    use super::*;
    use crate::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
    use crate::{Faction, Scenario, UnitKind};

    fn player(name: &str, faction: Faction, team: Option<u8>) -> PlayerSpec {
        PlayerSpec {
            name: name.into(),
            faction,
            team,
            scrap: 0,
            bot: false,
            bot_config: None,
        }
    }

    fn allied_incident_state() -> State {
        Scenario {
            mode: ScenarioMode::Match,
            name: "allied-incidents".into(),
            seed: 5,
            map: vec![
                "########################".into(),
                "#1.........2........3..#".into(),
                "#......................#".into(),
                "#......................#".into(),
                "#......................#".into(),
                "#......................#".into(),
                "########################".into(),
            ],
            players: vec![
                player("West", Faction::Ferrous, Some(0)),
                player("Center", Faction::Cupric, Some(0)),
                player("East", Faction::Ferrous, Some(1)),
            ],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .unwrap()
    }

    fn screened_source(extra_hostile: bool) -> (State, TilePos) {
        let source = TilePos::new(10, 5);
        let mut units = vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 9,
                y: 5,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 8,
                y: 5,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Sentinel,
                x: 12,
                y: 5,
            },
        ];
        if extra_hostile {
            units.push(UnitSpec {
                player: 1,
                kind: UnitKind::Sentinel,
                x: 12,
                y: 6,
            });
        }
        let state = Scenario {
            mode: ScenarioMode::Match,
            name: "screened-salvage".into(),
            seed: 4,
            map: vec![
                "####################".into(),
                "#1.................#".into(),
                "#..................#".into(),
                "#..................#".into(),
                "#..................#".into(),
                "#..................#".into(),
                "#................2.#".into(),
                "#..................#".into(),
                "####################".into(),
            ],
            players: vec![
                PlayerSpec {
                    name: "Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units,
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .unwrap();
        (state, source)
    }

    #[test]
    fn equal_local_ground_value_screens_a_work_zone() {
        let (state, source) = screened_source(false);
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        assert!(!danger.contains(source));
    }

    #[test]
    fn outmatched_local_ground_value_retires_a_work_zone() {
        let (state, source) = screened_source(true);
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        assert!(danger.contains(source));
    }

    #[test]
    fn cached_danger_queries_match_the_snapshot_predicate() {
        let (state, _) = screened_source(true);
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        for y in -1..=state.map.height() {
            for x in -1..=state.map.width() {
                let tile = TilePos::new(x, y);
                assert_eq!(
                    danger.contains(tile),
                    danger.compute_contains(tile),
                    "{tile}"
                );
                assert_eq!(
                    danger.contains(tile),
                    danger.compute_contains(tile),
                    "cached {tile}"
                );
            }
        }
    }

    #[test]
    fn exhausted_route_cache_preserves_a_goal_only_danger_exception() {
        let (state, _) = screened_source(false);
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        let start = TilePos::new(2, 3);
        let unreachable = TilePos::new(15, 3);
        let exceptional_goal = TilePos::new(10, 3);

        assert!(
            danger
                .find_route(start, unreachable, |tile| tile.x != 10)
                .is_none()
        );
        assert_eq!(
            danger.last_route_reachability(&[exceptional_goal, unreachable], false),
            Some(vec![false, false])
        );
        assert_eq!(
            danger.last_route_reachability(&[exceptional_goal, unreachable], true),
            Some(vec![true, false]),
            "a goal-specific exception is reachable through its explored cardinal neighbor"
        );
        assert!(
            danger
                .find_route(start, exceptional_goal, |tile| {
                    tile == exceptional_goal || tile.x != 10
                })
                .is_some(),
            "the cached exception agrees with a real goal-specific A*"
        );
    }

    #[test]
    fn allied_impact_memory_is_shared_bounded_and_cools_down() {
        let mut state = allied_incident_state();
        let source = TilePos::new(10, 4);
        state.record_salvage_incident(PlayerId(1), source);

        let west = state.vision(PlayerId(0)).salvage_incidents();
        let center = state.vision(PlayerId(1)).salvage_incidents();
        assert_eq!(west, center, "teammates receive one shared memory");
        assert_eq!(west.len(), 1);
        assert_eq!(west[0].tile, source);
        assert_eq!(
            west[0].expires_at,
            state.current_tick() + crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1
        );
        let danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        assert!(
            danger.contains(source),
            "the incident source stays ineligible"
        );
        assert!(
            danger.route_safe_from(source, source.offset(-1, 0)),
            "a worker inside the ring may step outward"
        );
        assert!(
            danger.route_safe_from(source.offset(-2, 0), source.offset(-3, 0)),
            "an outward route may keep leaving the ring"
        );
        assert!(
            !danger.route_safe_from(source.offset(-2, 0), source.offset(-1, 0)),
            "an escape cannot turn back toward the impact"
        );
        assert!(
            !danger.route_safe_from(source.offset(-5, 0), source.offset(-4, 0)),
            "a route originating outside cannot enter the incident ring"
        );
        assert!(
            state.vision(PlayerId(2)).salvage_incidents().is_empty(),
            "the hostile team learns nothing from its victim's memory"
        );

        state.tick = west[0].expires_at;
        assert!(
            !GroundSalvageDanger::capture(&state, PlayerId(0)).contains(source),
            "the incident stops affecting routes exactly at expiry"
        );
        state.refresh_vision();
        assert!(
            state.vision(PlayerId(0)).salvage_incidents().is_empty(),
            "refresh prunes expired state instead of accumulating history"
        );
        assert_eq!(state.vision(PlayerId(0)), state.vision(PlayerId(1)));
    }

    #[test]
    fn incident_memory_coalesces_and_evicts_deterministically() {
        let mut state = allied_incident_state();
        let repeated = TilePos::new(8, 3);
        state.record_salvage_incident(PlayerId(0), repeated);
        let first_expiry = state.vision(PlayerId(0)).salvage_incidents()[0].expires_at;
        state.tick += 7;
        state.record_salvage_incident(PlayerId(1), repeated);
        let incidents = state.vision(PlayerId(0)).salvage_incidents();
        assert_eq!(incidents.len(), 1);
        assert_eq!(incidents[0].expires_at, first_expiry + 7);

        let mut state = allied_incident_state();
        for x in 0..=crate::stats::HARVEST_INCIDENT_CAP {
            state.record_salvage_incident(PlayerId(0), TilePos::new(x as i32, 3));
        }
        let incidents = state.vision(PlayerId(0)).salvage_incidents();
        assert_eq!(incidents.len(), crate::stats::HARVEST_INCIDENT_CAP);
        assert_eq!(
            incidents[0].tile,
            TilePos::new(1, 3),
            "equal-expiry overflow evicts the row-major first site"
        );
        assert!(
            incidents.windows(2).all(|pair| {
                (pair[0].tile.y, pair[0].tile.x) < (pair[1].tile.y, pair[1].tile.x)
            })
        );
    }

    #[test]
    fn indexed_building_knowledge_matches_the_fog_reference() {
        fn reference(state: &State, viewer: PlayerId, tile: TilePos) -> bool {
            let vision = state.vision(viewer);
            if vision.visible(tile) {
                return state
                    .buildings_at(tile)
                    .any(|building| !building.kind.is_stealthy() && !building.provisional);
            }
            let team = state.player(viewer).team;
            state.buildings.iter().any(|building| {
                !building.kind.is_stealthy()
                    && !building.provisional
                    && state.player(building.player).team == team
                    && building.contains(tile)
            }) || vision
                .ghosts()
                .iter()
                .any(|ghost| !ghost.kind.is_stealthy() && ghost.footprint().any(|t| t == tile))
        }

        let (mut state, _) = screened_source(false);
        let visible_site = TilePos::new(10, 4);
        assert!(state.vision(PlayerId(0)).visible(visible_site));
        state.place_building(PlayerId(1), BuildingKind::Turret, visible_site);

        let check = |state: &State| {
            let knowledge = GroundSalvageDanger::capture(state, PlayerId(0));
            for y in 0..state.map.height() {
                for x in 0..state.map.width() {
                    let tile = TilePos::new(x, y);
                    assert_eq!(
                        knowledge.known_building_blocked(tile),
                        reference(state, PlayerId(0), tile),
                        "{tile}"
                    );
                }
            }
        };
        check(&state);

        state.refresh_vision();
        for unit in state
            .units
            .iter_mut()
            .filter(|unit| unit.player == PlayerId(0))
        {
            unit.pos = TilePos::new(2, 2).center();
        }
        state.refresh_vision();
        assert!(!state.vision(PlayerId(0)).visible(visible_site));
        assert!(
            state
                .vision(PlayerId(0))
                .ghosts()
                .iter()
                .any(|ghost| ghost.anchor == visible_site)
        );
        check(&state);
    }

    #[test]
    fn known_building_projection_only_blocks_ground_claiming_footprints() {
        let mut state = allied_incident_state();
        let own_charge = TilePos::new(3, 4);
        let allied_charge = TilePos::new(6, 4);
        let allied_wall = TilePos::new(9, 4);
        let hostile_wall = TilePos::new(13, 4);
        let hostile_charge = TilePos::new(16, 4);
        state.place_building(PlayerId(0), BuildingKind::ScuttleCharge, own_charge);
        state.place_building(PlayerId(1), BuildingKind::ScuttleCharge, allied_charge);
        state.place_building(PlayerId(1), BuildingKind::Barricade, allied_wall);
        state.vision[0].ghosts.extend([
            GhostBuilding {
                kind: BuildingKind::Barricade,
                owner: PlayerId(2),
                anchor: hostile_wall,
                hp: BuildingKind::Barricade.base_stats().max_hp,
                built: true,
            },
            GhostBuilding {
                kind: BuildingKind::ScuttleCharge,
                owner: PlayerId(2),
                anchor: hostile_charge,
                hp: BuildingKind::ScuttleCharge.base_stats().max_hp,
                built: true,
            },
        ]);

        assert!(state.passable(own_charge));
        assert!(state.passable(allied_charge));
        assert!(!state.passable(allied_wall));
        let projection = GroundSalvageDanger::capture(&state, PlayerId(0));
        assert!(!projection.known_building_blocked(own_charge));
        assert!(!projection.known_building_blocked(allied_charge));
        assert!(projection.known_building_blocked(allied_wall));
        assert!(projection.known_building_blocked(hostile_wall));
        assert!(!projection.known_building_blocked(hostile_charge));
    }

    fn static_egress_danger() -> GroundSalvageDanger {
        let (state, _) = screened_source(false);
        let mut danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        danger.mobile.clear();
        danger.statics = vec![StaticGroundPressure {
            anchor: TilePos::new(10, 3),
            size: (2, 2),
            reach_sq: Fx::from_num(64),
        }];
        danger
    }

    #[test]
    fn static_pressure_allows_egress_but_never_approach_or_new_entry() {
        let danger = static_egress_danger();
        let from = TilePos::new(7, 4);
        assert!(
            danger.contains(from),
            "the unsafe source remains ineligible for work"
        );
        assert!(danger.route_safe_from(from, TilePos::new(6, 4)));
        assert!(danger.route_safe_from(from, TilePos::new(7, 3)));
        assert!(!danger.route_safe_from(from, TilePos::new(8, 4)));
        assert!(!danger.route_safe_from(TilePos::new(0, 4), TilePos::new(3, 4)));
        assert!(danger.route_safe_from(from, TilePos::new(0, 4)));
    }

    #[test]
    fn static_egress_cannot_approach_an_overlapping_gun_or_cross_radar_or_mobile_fire() {
        let from = TilePos::new(7, 4);
        let to = TilePos::new(6, 4);
        let mut overlap = static_egress_danger();
        overlap.statics.push(StaticGroundPressure {
            anchor: TilePos::new(0, 3),
            size: (2, 2),
            reach_sq: Fx::from_num(64),
        });
        assert!(!overlap.route_safe_from(from, to));
        let mut radar = static_egress_danger();
        radar.contacts.push(to);
        assert!(!radar.route_safe_from(from, to));
        let mut mobile = static_egress_danger();
        mobile.mobile.push(MobileGroundPressure {
            pos: to.center(),
            reach_sq: Fx::from_num(16),
            strength: 100,
            hostile: true,
        });
        assert!(!mobile.route_safe_from(from, to));
    }

    /// A walk over every threat record, kept as the reference the coarse
    /// cells must reproduce: `(mobile or radar, observed)`.
    fn linear_threats(danger: &GroundSalvageDanger, source: TilePos) -> (bool, bool) {
        let point = source.center();
        let radar = danger
            .contacts
            .iter()
            .any(|contact| contact.chebyshev(source) <= crate::stats::HARVEST_RADAR_DANGER_RADIUS);
        let (mut hostile, mut screen) = (0u64, 0u64);
        for pressure in &danger.mobile {
            if pressure.pos.dist_sq(point) <= pressure.reach_sq {
                if pressure.hostile {
                    hostile = hostile.saturating_add(pressure.strength);
                } else {
                    screen = screen.saturating_add(pressure.strength);
                }
            }
        }
        let mobile = radar || hostile > screen;
        let statics = danger.statics.iter().any(|pressure| {
            rect_closest_point(pressure.anchor, pressure.size, point).dist_sq(point)
                <= pressure.reach_sq
        });
        (mobile, mobile || statics)
    }

    #[test]
    fn threat_cells_agree_with_a_walk_over_every_record() {
        let mut state = Scenario::skirmish().build().expect("skirmish builds");
        for _ in 0..90 {
            state.tick(&[]);
        }
        let mut danger = GroundSalvageDanger::capture(&state, PlayerId(0));
        assert!(danger.incidents.is_empty());
        let (width, height) = (state.map.width(), state.map.height());
        let mut rng = chassis::rng::Pcg32::new(17, 3);
        let mut coordinate = |span: i32| (rng.next_u32() % (span as u32 + 8)) as i32 - 4;
        for _ in 0..160 {
            let (x, y) = (coordinate(width), coordinate(height));
            let reach = Fx::from_num(x.rem_euclid(9) + 1) + Fx::lit("0.3");
            danger.mobile.push(MobileGroundPressure {
                pos: TilePos::new(x, y).center() + Vec2Fx::new(Fx::lit("0.2"), -Fx::lit("0.4")),
                reach_sq: reach * reach,
                strength: (y.rem_euclid(7) as u64 + 1) * 40,
                hostile: x % 2 == 0,
            });
        }
        for _ in 0..40 {
            let (x, y) = (coordinate(width), coordinate(height));
            let reach = Fx::from_num(y.rem_euclid(3) + 1);
            danger.statics.push(StaticGroundPressure {
                anchor: TilePos::new(x, y),
                size: (x.rem_euclid(3) + 1, y.rem_euclid(2) + 1),
                reach_sq: reach * reach,
            });
        }
        for _ in 0..6 {
            let (x, y) = (coordinate(width), coordinate(height));
            danger.contacts.push(TilePos::new(x, y));
        }

        let froms = [
            TilePos::new(0, 0),
            TilePos::new(width / 2, height / 2),
            TilePos::new(width - 1, 3),
        ];
        let (mut dangerous, mut safe) = (0, 0);
        for y in -2..=height + 1 {
            for x in -2..=width + 1 {
                let tile = TilePos::new(x, y);
                let (mobile, observed) = linear_threats(&danger, tile);
                assert_eq!(danger.mobile_or_radar_contains(tile), mobile, "{tile:?}");
                assert_eq!(danger.compute_observed_contains(tile), observed, "{tile:?}");
                for from in froms {
                    let approaches = danger.statics.iter().any(|pressure| {
                        let next =
                            rect_closest_point(pressure.anchor, pressure.size, tile.center())
                                .dist_sq(tile.center());
                        next <= pressure.reach_sq
                            && next
                                < rect_closest_point(pressure.anchor, pressure.size, from.center())
                                    .dist_sq(from.center())
                    });
                    assert_eq!(
                        danger.route_safe_from(from, tile),
                        !(observed && (mobile || approaches)),
                        "{from:?} -> {tile:?}"
                    );
                }
                dangerous += usize::from(observed);
                safe += usize::from(!observed);
            }
        }
        assert!(
            dangerous > 0 && safe > 0,
            "{dangerous} dangerous, {safe} safe"
        );
    }
}
