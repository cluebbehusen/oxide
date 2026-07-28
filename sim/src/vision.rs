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
use crate::stats::BuildingKind;
use chassis::grid::{Grid, TilePos};
use serde::{Deserialize, Serialize};

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
        // Row slices, not per-cell lookups — this scan runs over the
        // whole map for every team every tick.
        for y in 0..state.map.height() {
            let visible = view.visible.row(y).expect("row in range");
            let tiles = state.map.grid().row(y).expect("row in range");
            let scrap = view.remembered_scrap.row_mut(y).expect("row in range");
            for (x, (&seen, tile)) in visible.iter().zip(tiles).enumerate() {
                if seen {
                    scrap[x] = tile.scrap;
                }
            }
            let visible = view.visible.row(y).expect("row in range");
            let wreck = view.remembered_wreck.row_mut(y).expect("row in range");
            for (x, (&seen, tile)) in visible.iter().zip(tiles).enumerate() {
                if seen {
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
