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
    /// Remembered enemy buildings, sorted by (anchor.y, anchor.x) — a
    /// deterministic canonical order like everything else in the state.
    #[serde(default)]
    ghosts: Vec<GhostBuilding>,
    /// Scrap per tile as this player last saw it. Only meaningful where
    /// `explored`; frozen wherever sight is lost, exactly like ghosts.
    remembered_scrap: Grid<u32>,
}

impl Vision {
    pub(crate) fn new(width: i32, height: i32) -> Self {
        Self {
            visible: Grid::new(width, height, false),
            explored: Grid::new(width, height, false),
            ghosts: Vec::new(),
            remembered_scrap: Grid::new(width, height, 0),
        }
    }

    /// Enemy buildings as this player last saw them. While a building's
    /// ground is visible its record simply mirrors live state; the record
    /// earns the name "ghost" once sight is lost and it freezes. Renderers
    /// should draw live state on visible ground and these everywhere else.
    pub fn ghosts(&self) -> &[GhostBuilding] {
        &self.ghosts
    }

    /// Scrap at `pos` as last seen (zero where never seen or out of
    /// bounds). Renderers should use live amounts on visible ground and
    /// this everywhere else.
    pub fn remembered_scrap(&self, pos: TilePos) -> u32 {
        self.remembered_scrap.get(pos).copied().unwrap_or(0)
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
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
                let pos = center.offset(dx, dy);
                if let Some(cell) = self.visible.get_mut(pos) {
                    *cell = true;
                }
                if let Some(cell) = self.explored.get_mut(pos) {
                    *cell = true;
                }
            }
        }
    }
}

/// Rebuilds every player's `visible` set from their live entities, then
/// reconciles their building memory against what is now in sight.
pub(crate) fn refresh(state: &mut State) {
    let mut vision = std::mem::take(&mut state.vision);
    for (index, view) in vision.iter_mut().enumerate() {
        let player = PlayerId(index as u8);
        view.visible.fill(false);
        for unit in state.units.iter().filter(|u| u.player == player) {
            view.stamp_disc(unit.tile(), unit.kind.stats().vision);
        }
        // Sites don't see: a pile of parts has no sensors.
        for building in state
            .buildings
            .iter()
            .filter(|b| b.player == player && b.built)
        {
            let radius = building.kind.stats().vision;
            for tile in building.tiles() {
                view.stamp_disc(tile, radius);
            }
        }

        // Memory reconciliation. Wherever we have sight, live state is the
        // truth: drop every record on visible ground, then re-record every
        // enemy building actually seen there (fresh hp). A building seen
        // *gone* thus loses its record, and a record on unseen ground
        // freezes at its last sighting.
        let mut ghosts = std::mem::take(&mut view.ghosts);
        ghosts.retain(|ghost| !ghost.footprint().any(|t| view.visible(t)));
        for building in state.buildings.iter().filter(|b| b.player != player) {
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
        // remember the scrap; everywhere else the old number stands.
        for (pos, tile) in state.map.iter() {
            if view.visible(pos)
                && let Some(cell) = view.remembered_scrap.get_mut(pos)
            {
                *cell = tile.scrap;
            }
        }
    }
    state.vision = vision;
}
