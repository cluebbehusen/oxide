//! Fog of war: per-player visibility.
//!
//! Each player owns two boolean grids. `visible` is recomputed from scratch
//! every tick — the union of vision discs around that player's units and
//! buildings. `explored` only ever accumulates. Vision is radius-based;
//! rocks do not block line of sight (a deliberate simplification, cheap and
//! predictable).
//!
//! What fog *enforces* is deliberately narrow: targeted attack commands
//! require the issuer to see the victim. Everything else — what the shell
//! draws, what a player knows — is presentation reading these grids. The
//! built-in bot reads full state (a classic cheating AI), but the commands
//! it issues still pass the same validation as everyone else's.

use crate::ids::PlayerId;
use crate::state::State;
use crate::stats::{BuildingKind, Domain};
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::{CARDINALS, Grid, TilePos};
use chassis::path::AstarScratch;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

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
    fn footprint(&self) -> impl Iterator<Item = TilePos> + use<> {
        let (w, h) = self.kind.stats().size;
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
    /// Radar blips: tiles holding a hostile unit inside an own built
    /// Array's outer ring but outside true sight. A contact without
    /// identity — no kind, no owner, no memory (rebuilt every tick).
    contacts: Vec<TilePos>,
    /// Recent tiles where this team saw one of its own assets take damage.
    /// Sorted and deduplicated by (y, x); old snapshots predate the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    salvage_incidents: Vec<SalvageIncident>,
}

impl Vision {
    pub(crate) fn new(width: i32, height: i32) -> Self {
        Self {
            visible: Grid::new(width, height, false),
            explored: Grid::new(width, height, false),
            ghosts: Vec::new(),
            remembered_scrap: Grid::new(width, height, 0),
            remembered_wreck: Grid::new(width, height, 0),
            contacts: Vec::new(),
            salvage_incidents: Vec::new(),
        }
    }

    /// Enemy buildings as this player last saw them. While a building's
    /// ground is visible its record simply mirrors live state; the record
    /// earns the name "ghost" once sight is lost and it freezes. Renderers
    /// should draw live state on visible ground and these everywhere else.
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

    /// Whether the player has ever seen `pos`.
    pub fn explored(&self, pos: TilePos) -> bool {
        self.explored.get(pos).copied().unwrap_or(false)
    }

    fn stamp_disc(&mut self, center: TilePos, radius: i32) {
        let spans = disc_spans(radius);
        for dy in -radius..=radius {
            let span = spans[dy.unsigned_abs() as usize];
            let y = center.y + dy;
            self.visible
                .fill_row_span(y, center.x - span, center.x + span, true);
            self.explored
                .fill_row_span(y, center.x - span, center.x + span, true);
        }
    }

    /// Stamps the union of discs centered on every tile of a `w`x`h`
    /// footprint — the rectangle's Minkowski sum with the sight disc,
    /// written row by row. Cell-identical to stamping each footprint
    /// tile separately, without visiting the overlap four times.
    fn stamp_rect(&mut self, anchor: TilePos, w: i32, h: i32, radius: i32) {
        let spans = disc_spans(radius);
        for dy in -radius..(h + radius) {
            let vdist = (-dy).max(dy - (h - 1)).max(0);
            let span = spans[vdist as usize];
            let y = anchor.y + dy;
            self.visible
                .fill_row_span(y, anchor.x - span, anchor.x + (w - 1) + span, true);
            self.explored
                .fill_row_span(y, anchor.x - span, anchor.x + (w - 1) + span, true);
        }
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
    cache: RefCell<Vec<Option<bool>>>,
    observed_cache: RefCell<Vec<Option<bool>>>,
    path_scratch: RefCell<AstarScratch>,
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
                let range = ground_weapon_reach(ghost.kind.stats().weapons)?
                    + crate::stats::HARVEST_STATIC_DANGER_MARGIN;
                Some(StaticGroundPressure {
                    anchor: ghost.anchor,
                    size: ghost.kind.stats().size,
                    reach_sq: range * range,
                })
            })
            .collect();
        let mut building_blocks = vec![Vec::new(); state.map.height() as usize];
        let viewer_team = state.player(viewer).team;
        for building in &state.buildings {
            if state.player(building.player).team == viewer_team {
                stamp_blocked_rect(
                    &mut building_blocks,
                    state.map.width(),
                    building.anchor,
                    building.kind.stats().size,
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
        for ghost in vision.ghosts() {
            stamp_blocked_rect(
                &mut building_blocks,
                state.map.width(),
                ghost.anchor,
                ghost.kind.stats().size,
            );
        }
        for row in &mut building_blocks {
            merge_spans(row);
        }
        let cell_count = (state.map.width() as usize) * (state.map.height() as usize);
        Self {
            width: state.map.width(),
            height: state.map.height(),
            contacts: vision.contacts().to_vec(),
            incidents: vision
                .salvage_incidents()
                .iter()
                .filter(|incident| incident.expires_at > state.tick)
                .map(|incident| incident.tile)
                .collect(),
            mobile,
            statics,
            building_blocks,
            cache: RefCell::new(vec![None; cell_count]),
            observed_cache: RefCell::new(vec![None; cell_count]),
            path_scratch: RefCell::new(AstarScratch::default()),
        }
    }

    /// Whether this snapshot marks one tile as too dangerous for
    /// autonomous salvage work.
    pub(crate) fn contains(&self, source: TilePos) -> bool {
        cached_tile_predicate(&self.cache, self.width, self.height, source, || {
            self.compute_contains(source)
        })
    }

    /// Whether an autonomous route may traverse `tile` from its current
    /// planning origin. Live threats, radar, and static fire remain hard
    /// barriers. A worker already inside an anonymous incident ring may
    /// move laterally or outward, but never closer to that impact; a worker
    /// outside cannot enter it.
    pub(crate) fn route_safe_from(&self, from: TilePos, tile: TilePos) -> bool {
        if cached_tile_predicate(&self.observed_cache, self.width, self.height, tile, || {
            self.compute_observed_contains(tile)
        }) {
            return false;
        }
        !self.incidents.iter().any(|incident| {
            let next_distance = incident.chebyshev(tile);
            next_distance <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
                && next_distance < incident.chebyshev(from)
        })
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
        if self
            .contacts
            .iter()
            .any(|contact| contact.chebyshev(source) <= crate::stats::HARVEST_RADAR_DANGER_RADIUS)
        {
            return true;
        }
        let source_point = source.center();
        let (mut hostile_strength, mut screen_strength) = (0u64, 0u64);
        for pressure in &self.mobile {
            if pressure.pos.dist_sq(source_point) > pressure.reach_sq {
                continue;
            }
            if pressure.hostile {
                hostile_strength = hostile_strength.saturating_add(pressure.strength);
            } else {
                screen_strength = screen_strength.saturating_add(pressure.strength);
            }
        }
        if hostile_strength > screen_strength {
            return true;
        }
        self.statics.iter().any(|pressure| {
            rect_closest_point(pressure.anchor, pressure.size, source_point).dist_sq(source_point)
                <= pressure.reach_sq
        })
    }
}

fn cached_tile_predicate(
    cache: &RefCell<Vec<Option<bool>>>,
    width: i32,
    height: i32,
    tile: TilePos,
    compute: impl FnOnce() -> bool,
) -> bool {
    if tile.x < 0 || tile.y < 0 || tile.x >= width || tile.y >= height {
        return compute();
    }
    let index = (tile.y as usize) * (width as usize) + tile.x as usize;
    let cached = cache.borrow()[index];
    if let Some(value) = cached {
        return value;
    }
    let value = compute();
    cache.borrow_mut()[index] = Some(value);
    value
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

/// Rebuilds every player's `visible` set from their live entities, then
/// reconciles their building memory against what is now in sight.
pub(crate) fn refresh(state: &mut State) {
    let mut vision = std::mem::take(&mut state.vision);
    for index in 0..vision.len() {
        // Team sight is seat-symmetric by construction: every teammate
        // stamps the same discs, reconciles the same memories, hears
        // the same radar. A later seat on an already-computed team is
        // a byte-for-byte clone — half the refresh on team maps.
        if let Some(src) = (0..index).find(|&j| state.players[j].team == state.players[index].team)
        {
            vision[index] = vision[src].clone();
            continue;
        }
        let view = &mut vision[index];
        view.prune_salvage_incidents(state.tick);
        let my_team = state.players[index].team;
        let allied = |p: PlayerId| state.players[p.0 as usize].team == my_team;
        view.visible.fill(false);
        // Team sight: every teammate's eyes stamp into this view.
        for unit in state.units.iter().filter(|u| allied(u.player)) {
            view.stamp_disc(unit.tile(), unit.kind.stats().vision);
        }
        // Sites don't see: a pile of parts has no sensors.
        for building in state
            .buildings
            .iter()
            .filter(|b| allied(b.player) && b.built)
        {
            let (w, h) = building.kind.stats().size;
            view.stamp_rect(building.anchor, w, h, building.kind.stats().vision);
        }

        // Memory reconciliation. Wherever we have sight, live state is the
        // truth: drop every record on visible ground, then re-record every
        // enemy building actually seen there (fresh hp). A building seen
        // *gone* thus loses its record, and a record on unseen ground
        // freezes at its last sighting.
        let mut ghosts = std::mem::take(&mut view.ghosts);
        ghosts.retain(|ghost| !ghost.footprint().any(|t| view.visible(t)));
        for building in state.buildings.iter().filter(|b| !allied(b.player)) {
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
        // Row slices, not per-cell lookups, and both memories in one
        // walk — this scan runs over the whole map for every team every
        // tick, so it gets to run exactly once.
        for y in 0..state.map.height() {
            let visible = view.visible.row(y).expect("row in range");
            let tiles = state.map.grid().row(y).expect("row in range");
            let scrap = view.remembered_scrap.row_mut(y).expect("row in range");
            let wreck = view.remembered_wreck.row_mut(y).expect("row in range");
            for (x, (&seen, tile)) in visible.iter().zip(tiles).enumerate() {
                if seen {
                    scrap[x] = tile.scrap;
                    wreck[x] = tile.wreck;
                }
            }
        }

        // Radar blips: hostile units inside any own built Array's outer
        // ring, on ground this player cannot actually see. A tile only —
        // detection is not identification, and there is no memory: a
        // contact that leaves the ring is simply gone.
        view.contacts.clear();
        let masts: Vec<TilePos> = state
            .buildings
            .iter()
            .filter(|b| allied(b.player) && b.built && b.kind == BuildingKind::Array)
            .map(|b| b.anchor)
            .collect();
        if !masts.is_empty() {
            let r = crate::stats::RADAR_DETECT_RADIUS;
            for u in state.units.iter().filter(|u| !allied(u.player)) {
                let t = u.tile();
                if view.visible(t) {
                    continue;
                }
                let detected = masts.iter().any(|m| {
                    let (dx, dy) = (t.x - m.x, t.y - m.y);
                    dx * dx + dy * dy <= r * r
                });
                if detected {
                    view.contacts.push(t);
                }
            }
            view.contacts.sort_unstable_by_key(|t| (t.y, t.x));
            view.contacts.dedup();
        }
    }
    state.vision = vision;
}

#[cfg(test)]
mod danger_tests {
    use super::*;
    use crate::scenario::{PlayerSpec, UnitSpec};
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
                return state.building_at(tile).is_some();
            }
            let team = state.player(viewer).team;
            state.buildings.iter().any(|building| {
                state.player(building.player).team == team && building.contains(tile)
            }) || vision
                .ghosts()
                .iter()
                .any(|ghost| ghost.footprint().any(|t| t == tile))
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
}
