//! Read-only views of sim state, shaped for reading rather than simulating.
//!
//! The sim's own serialization is exact (fixed-point bits) and therefore
//! unreadable; these views trade exactness for legibility — positions as
//! floats, the map as ASCII with entities overlaid. Anything that needs
//! exactness should use the state hash, not a view.

use chassis::grid::TilePos;
use chassis::grid::as_index;
use oxide_sim::{Building, GameResult, Order, PlayerId, State, Unit, UnitKind};
use serde::{Deserialize, Serialize};

/// Which sections [`StateView`] should include. Map defaults off — it is by
/// far the largest section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag selects an independent section"
)]
pub struct StateFilter {
    /// Include player rows.
    pub players: bool,
    /// Include unit rows.
    pub units: bool,
    /// Include building rows.
    pub buildings: bool,
    /// Include the ASCII map.
    pub map: bool,
}

impl Default for StateFilter {
    fn default() -> Self {
        Self {
            players: true,
            units: true,
            buildings: true,
            map: false,
        }
    }
}

/// Snapshot of a running match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateView {
    /// Current tick.
    pub tick: u64,
    /// State fingerprint as hex — compare these, not the float fields.
    pub hash: String,
    /// Set once the match is decided.
    pub result: Option<GameResult>,
    /// Player rows (empty when filtered out).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub players: Vec<PlayerView>,
    /// Unit rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub units: Vec<UnitView>,
    /// Building rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buildings: Vec<BuildingView>,
    /// ASCII map with entities overlaid: terrain per the scenario legend,
    /// buildings as `A`/`B`/… by player, units as `a`/`b`/… by player.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<Vec<String>>,
}

/// One player's public numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerView {
    /// Player index.
    pub id: u8,
    /// Display name.
    pub name: String,
    /// Normalized team id: how a debug client tells allies apart and maps a
    /// victory's team back to its seats.
    pub team: u8,
    /// Banked scrap.
    pub scrap: u32,
    /// Living units.
    pub units: usize,
    /// Standing buildings.
    pub buildings: usize,
    /// Whether this seat has conceded and can no longer issue commands.
    #[serde(default)]
    pub resigned: bool,
    /// The tick this seat lost its last Foundry and site, if it has
    /// (free-for-all placement reads from these).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eliminated_at: Option<u64>,
}

/// One unit, floats-for-reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitView {
    /// Unit id.
    pub id: u32,
    /// Owner index.
    pub player: u8,
    /// Kind.
    pub kind: UnitKind,
    /// World position `[x, y]` (tile units).
    pub pos: [f64; 2],
    /// Occupied tile `[x, y]`.
    pub tile: [i32; 2],
    /// Hit points.
    pub hp: u32,
    /// Scrap on board.
    pub carrying: u32,
    /// Current intent, as the sim's own tagged serialization. `None` when
    /// the fog view redacts a hostile unit's intent; omniscient captures
    /// always fill it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Order>,
    /// Orders waiting behind the active one, in execution order. Empty
    /// whenever `order` is redacted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue: Vec<Order>,
    /// Whether the queue loops (a patrol circuit); `None` when intent
    /// is redacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patrolling: Option<bool>,
    /// Parked on the ground at its tile center. A physical fact rather
    /// than an intent: the airframe draws as a ground body and is hit as
    /// one, so the fog view never redacts it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub landed: bool,
}

/// One building.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildingView {
    /// Building id.
    pub id: u32,
    /// Owner index.
    pub player: u8,
    /// Kind.
    pub kind: oxide_sim::BuildingKind,
    /// Top-left footprint tile `[x, y]`.
    pub anchor: [i32; 2],
    /// Hit points.
    pub hp: u32,
    /// Production queue, front first. `None` means not knowable
    /// through this view (the fog view redacts hostile production);
    /// omniscient captures always fill it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<Vec<UnitKind>>,
    /// Ticks until `queue[0]` finishes (absent when idle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks_remaining: Option<u32>,
    /// Rally tile `[x, y]`, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rally: Option<[i32; 2]>,
    /// Player-designated defense target. The fog view exposes this only for
    /// allied buildings; hostile targeting intent is redacted with rally
    /// and production state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<oxide_sim::AttackTarget>,
    /// Whether construction has finished.
    #[serde(
        default = "default_true",
        skip_serializing_if = "core::clone::Clone::clone"
    )]
    pub built: bool,
    /// Paid blueprint whose footprint is not yet verified.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub provisional: bool,
    /// Construction or training progress ticks.
    #[serde(default, skip_serializing_if = "is_default")]
    pub progress: u32,
    /// Upgrade-ladder rung (0 = base). Visible in every view: a
    /// building's tier shows in its silhouette on the ground.
    #[serde(default, skip_serializing_if = "is_default")]
    pub tier: u8,
}

/// The world as one seat knows it: the fog-honest debug and agent
/// counterpart to the omniscient [`StateView`]. It reads the sim's own
/// [`oxide_sim::Vision`], the same source as the bots' fog-honest
/// `ObservationData`, so it cannot leak what fog hides: live entities
/// appear only under current sight, memories carry no more than the seat
/// last saw, and radar contacts are bare tiles.
///
/// Every debug session builds this through [`FogView::capture`], so live,
/// playback, and headless answers cannot drift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FogView {
    /// Current tick. There is no hash: a partial view has no canonical
    /// fingerprint, so exactness stays with [`StateView`].
    pub tick: u64,
    /// The seat this knowledge belongs to.
    pub player: u8,
    /// The viewing seat's own economy and status. Other seats' player rows
    /// are absent so no hostile economy leaks.
    pub own_player: Box<PlayerView>,
    /// One row per map row, one char per tile: `' '` never seen, `'.'`
    /// explored but currently dark, `'*'` visible right now.
    pub mask: Vec<String>,
    /// Own and allied units always (team sight is standing); hostile
    /// units only while their tile is visible.
    pub units: Vec<UnitView>,
    /// Own and allied buildings always; hostile buildings only while some
    /// footprint tile is visible and the stealth rule reveals them. Their
    /// ghost records mirror live state while seen.
    pub buildings: Vec<BuildingView>,
    /// Enemy buildings as last seen: frozen memories once sight is lost.
    pub ghosts: Vec<GhostView>,
    /// Remembered scrap amounts on explored ground (nonzero only): live
    /// amounts on visible tiles, beliefs elsewhere.
    pub scrap: Vec<RememberedTileView>,
    /// Remembered wreck salvage, same treatment as scrap.
    pub wrecks: Vec<RememberedTileView>,
    /// Radar blips: bare tiles with no kind or owner.
    pub contacts: Vec<[i32; 2]>,
    /// Continuously observed contacts; entity identity appears only in true sight.
    pub contact_tracks: Vec<oxide_sim::vision::ContactTrack>,
}

/// An enemy building as one seat remembers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostView {
    /// Remembered kind.
    pub kind: oxide_sim::BuildingKind,
    /// Remembered owner index.
    pub owner: u8,
    /// Top-left footprint tile `[x, y]`.
    pub anchor: [i32; 2],
    /// Hit points as last seen.
    pub hp: u32,
    /// Whether it looked finished when last seen.
    pub built: bool,
}

/// A remembered salvage amount at a tile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedTileView {
    /// Tile `[x, y]`.
    pub tile: [i32; 2],
    /// Amount as last seen.
    pub amount: u32,
}

impl FogView {
    /// Captures what `player` currently knows of `state`.
    pub fn capture(state: &State, player: PlayerId) -> Self {
        let vision = state.vision(player);
        let (width, height) = (state.map().width(), state.map().height());
        let mut mask = Vec::with_capacity(as_index(height));
        let mut scrap = Vec::new();
        let mut wrecks = Vec::new();
        for y in 0..height {
            let mut row = String::with_capacity(as_index(width));
            for x in 0..width {
                let pos = TilePos::new(x, y);
                row.push(if vision.visible(pos) {
                    '*'
                } else if vision.explored(pos) {
                    '.'
                } else {
                    ' '
                });
                if vision.explored(pos) {
                    let remembered = vision.remembered_scrap(pos);
                    if remembered > 0 {
                        scrap.push(RememberedTileView {
                            tile: [x, y],
                            amount: remembered,
                        });
                    }
                    let remembered = vision.remembered_wreck(pos);
                    if remembered > 0 {
                        wrecks.push(RememberedTileView {
                            tile: [x, y],
                            amount: remembered,
                        });
                    }
                }
            }
            mask.push(row);
        }
        Self {
            tick: state.current_tick(),
            player: player.0,
            own_player: Box::new(player_view(state, usize::from(player.0))),
            mask,
            units: state
                .units()
                .iter()
                .filter(|u| !state.hostile(player, u.player) || vision.visible(u.tile()))
                .map(|u| {
                    if state.hostile(player, u.player) {
                        unit_view_redacted(u)
                    } else {
                        unit_view(u)
                    }
                })
                .collect(),
            buildings: state
                .buildings()
                .iter()
                .filter(|b| {
                    // Seeing the ground does not reveal a buried charge:
                    // the stealth rule gates this view as it gates
                    // targeting and ghosts.
                    !state.hostile(player, b.player)
                        || (b.tiles().any(|t| vision.visible(t))
                            && state.building_apparent(player, b))
                })
                .map(|b| {
                    if state.hostile(player, b.player) {
                        building_view_redacted(b)
                    } else {
                        building_view(b)
                    }
                })
                .collect(),
            ghosts: vision
                .ghosts()
                .iter()
                .map(|g| GhostView {
                    kind: g.kind,
                    owner: g.owner.0,
                    anchor: [g.anchor.x, g.anchor.y],
                    hp: g.hp,
                    built: g.built,
                })
                .collect(),
            scrap,
            wrecks,
            contacts: vision.contacts().iter().map(|t| [t.x, t.y]).collect(),
            contact_tracks: vision.tracks().to_vec(),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Serde skip predicate: omits a field that still holds its default value.
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Shell status summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusView {
    /// Current tick.
    pub tick: u64,
    /// Whether the wall clock is stopped.
    pub paused: bool,
    /// Wall-clock speed multiplier.
    pub speed: f64,
    /// Scenario display name.
    pub scenario: String,
    /// [`oxide_sim::SIM_VERSION`] of the running sim.
    pub sim_version: u32,
    /// Match outcome, if decided.
    pub result: Option<GameResult>,
    /// Commands recorded into the session replay so far.
    pub recorded_commands: usize,
}

/// Camera pose and what it can see.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraView {
    /// World point at the viewport center.
    pub center: [f64; 2],
    /// Pixels per world unit.
    pub zoom: f64,
    /// Viewport size in pixels `[w, h]`.
    pub viewport: [f64; 2],
    /// Visible world rectangle `[min_x, min_y, max_x, max_y]`.
    pub world_rect: [f64; 4],
}

/// Snapshot of the shell screen that currently owns input.
///
/// Menu rows use a half-open `visible_range`, so `[2, 7]` means item
/// indices 2 through 6 are currently drawn. Grid screens (the
/// `main_menu` map browser) report the contiguous run of cards on
/// screen the same way, and `[0, 0]` when the window shows none —
/// distinct from `None`, which means the mode has no menu at all.
/// Gameplay has no active menu and reports `None` for the
/// menu-specific fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiView {
    /// Stable snake-case mode name, such as `main_menu` or `playing`.
    pub mode: String,
    /// Active menu heading, when this mode has a menu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Highlighted row index, when this mode has a menu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<usize>,
    /// Every row label, including rows outside the current scroll window.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
    /// Half-open item-index range currently visible on screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible_range: Option<[usize; 2]>,
    /// Row under the pointer (highlight only — never the selection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover: Option<usize>,
    /// Gameplay chrome geometry as [`top_bar_h`, `panel_top`, minimap x/y/w/h,
    /// `panel_right`, orders x/y/w/h] in window pixels, from the same
    /// `LayoutModel` hit-testing reads, so an agent can aim clicks at (or
    /// away from) real chrome. The command band spans only to
    /// `panel_right`; `orders` is the queue dock on the left edge,
    /// zero-sized when absent. Menu modes report `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chrome: Option<[f32; 11]>,
    /// Exact information and action rectangles [x, y, width, height];
    /// `chrome` describes only their enclosing bounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_regions: Option<[[f32; 4]; 2]>,
    /// The top bar's menu button [x, y, width, height]; a click or tap
    /// there opens the pause menu. Absent outside live play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menu_button: Option<[f32; 4]>,
    /// The top bar's clock or PAUSED status [x, y, width, height]; a
    /// click or tap there toggles pause. Absent outside live play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_status: Option<[f32; 4]>,
    /// The control-group column's plate [x, y, width, height] while it
    /// shows above the minimap. Its slots recall and assign groups and
    /// its gaps swallow presses, so it is chrome, not world. Absent
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_column: Option<[f32; 4]>,
}

impl StateView {
    /// Captures a filtered snapshot of `state`.
    pub fn capture(state: &State, filter: StateFilter) -> Self {
        Self {
            tick: state.current_tick(),
            hash: crate::hash_hex(state.hash()),
            result: state.result(),
            players: if filter.players {
                {
                    state
                        .players()
                        .iter()
                        .enumerate()
                        .map(|(i, _)| player_view(state, i))
                        .collect()
                }
            } else {
                Vec::default()
            },
            units: if filter.units {
                state.units().iter().map(unit_view).collect()
            } else {
                Vec::default()
            },
            buildings: if filter.buildings {
                state.buildings().iter().map(building_view).collect()
            } else {
                Vec::default()
            },
            map: filter.map.then(|| ascii_with_entities(state)),
        }
    }
}

fn player_view(state: &State, index: usize) -> PlayerView {
    let player = &state.players()[index];
    PlayerView {
        id: PlayerId::from_index(index).0,
        name: player.name.clone(),
        team: player.team,
        scrap: player.scrap,
        units: state
            .units()
            .iter()
            .filter(|unit| usize::from(unit.player.0) == index)
            .count(),
        buildings: state
            .buildings()
            .iter()
            .filter(|building| usize::from(building.player.0) == index)
            .count(),
        resigned: player.resigned,
        eliminated_at: player.eliminated_at,
    }
}

fn unit_view(u: &Unit) -> UnitView {
    UnitView {
        id: u.id.0,
        player: u.player.0,
        kind: u.kind,
        pos: [u.pos.x.to_num(), u.pos.y.to_num()],
        tile: [u.tile().x, u.tile().y],
        hp: u.hp,
        carrying: u.carrying(),
        order: Some(u.order),
        queue: u.queue.iter().copied().collect(),
        patrolling: Some(u.looping),
        landed: u.landed(),
    }
}

/// A hostile unit as the viewer sees it: body, position, wounds, and
/// visible cargo, but never its orders, queue, or patrol flag.
fn unit_view_redacted(u: &Unit) -> UnitView {
    UnitView {
        order: None,
        queue: Vec::new(),
        patrolling: None,
        ..unit_view(u)
    }
}

fn building_view(b: &Building) -> BuildingView {
    BuildingView {
        id: b.id.0,
        player: b.player.0,
        kind: b.kind,
        anchor: [b.anchor.x, b.anchor.y],
        hp: b.hp,
        queue: Some(b.queue.iter().copied().collect()),
        ticks_remaining: b.queue.front().map(|kind| {
            kind.stats()
                .train_ticks
                .saturating_sub(b.training_progress())
        }),
        rally: b.rally.map(|r| [r.x, r.y]),
        focus: b.focus,
        built: b.built(),
        provisional: b.provisional(),
        progress: b
            .construction_progress()
            .unwrap_or_else(|| b.training_progress()),
        tier: b.tier,
    }
}

/// A hostile building as the viewer sees it: hull, scaffold stage, and
/// wounds, but never its production queue, rally point, or focus target.
fn building_view_redacted(b: &Building) -> BuildingView {
    BuildingView {
        queue: None,
        ticks_remaining: None,
        rally: None,
        focus: None,
        // A scaffold's stage is drawn on every screen; a built
        // producer's meter is training progress no enemy panel shows.
        progress: b.construction_progress().unwrap_or(0),
        ..building_view(b)
    }
}

/// Terrain plus entity overlay: buildings print as `A` + player, units as
/// `a` + player; units win when both claim a tile.
fn ascii_with_entities(state: &State) -> Vec<String> {
    let mut rows: Vec<Vec<char>> = state
        .map()
        .ascii_rows()
        .into_iter()
        .map(|r| r.chars().collect())
        .collect();
    let mut put = |x: i32, y: i32, c: char| {
        if let Some(cell) = rows
            .get_mut(as_index(y))
            .and_then(|row| row.get_mut(as_index(x)))
        {
            *cell = c;
        }
    };
    for b in state.buildings() {
        for t in b.tiles() {
            put(t.x, t.y, (b'A' + b.player.0) as char);
        }
    }
    for u in state.units() {
        let t = u.tile();
        put(t.x, t.y, (b'a' + u.player.0) as char);
    }
    rows.into_iter().map(|r| r.into_iter().collect()).collect()
}

#[cfg(test)]
mod tests;
