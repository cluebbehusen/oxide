//! Read-only views of sim state, shaped for reading rather than simulating.
//!
//! The sim's own serialization is exact (fixed-point bits) and therefore
//! unreadable; these views trade exactness for legibility — positions as
//! floats, the map as ASCII with entities overlaid. Anything that needs
//! exactness should use the state hash, not a view.

use chassis::grid::TilePos;
use oxide_sim::{Building, Faction, GameResult, Order, PlayerId, State, Unit, UnitKind};
use serde::{Deserialize, Serialize};

/// Which sections [`StateView`] should include. Map defaults off — it is by
/// far the largest section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
    /// Sprite tint.
    pub faction: Faction,
    /// Normalized team id — the only way a debug client can tell allies
    /// apart (factions repeat across teams) or map a victory's team back
    /// to its seats.
    pub team: u8,
    /// Banked scrap.
    pub scrap: u32,
    /// Living units.
    pub units: usize,
    /// Standing buildings.
    pub buildings: usize,
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
    /// Current intent, as the sim's own tagged serialization.
    pub order: Order,
    /// Orders waiting behind the active one, in execution order — agents
    /// verify queues and patrol circuits from this, not just a count.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue: Vec<Order>,
    /// Whether the queue loops (a patrol circuit).
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub patrolling: bool,
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
    /// Production queue, front first.
    pub queue: Vec<UnitKind>,
    /// Ticks until `queue[0]` finishes (absent when idle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticks_remaining: Option<u32>,
    /// Rally tile `[x, y]`, if set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rally: Option<[i32; 2]>,
    /// Whether construction has finished.
    #[serde(
        default = "default_true",
        skip_serializing_if = "core::clone::Clone::clone"
    )]
    pub built: bool,
    /// Construction or training progress ticks.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub progress: u32,
}

/// The world as one seat honestly knows it — the fog-safe counterpart to
/// the omniscient [`StateView`]. Everything here reads the sim's own
/// [`oxide_sim::Vision`], so the view can never leak what fog hides: live
/// entities appear only under current sight, memories carry no more than
/// the seat last saw, and radar contacts are bare tiles.
///
/// Both servers (the shell and the headless session) build this through
/// [`FogView::capture`], so live and headless answers cannot drift.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FogView {
    /// Current tick. Deliberately no hash — a partial view has no
    /// canonical fingerprint; exactness stays with [`StateView`].
    pub tick: u64,
    /// The seat this knowledge belongs to.
    pub player: u8,
    /// One row per map row, one char per tile: `' '` never seen, `'.'`
    /// explored but currently dark, `'*'` visible right now.
    pub mask: Vec<String>,
    /// Own and allied units always (team sight is standing); hostile
    /// units only while their tile is visible.
    pub units: Vec<UnitView>,
    /// Own and allied buildings always; hostile buildings only while
    /// some footprint tile is visible (their ghost record below mirrors
    /// live state at the same time — that is the sim's refresh rule).
    pub buildings: Vec<BuildingView>,
    /// Enemy buildings as last seen: frozen memories once sight is lost.
    pub ghosts: Vec<GhostView>,
    /// Remembered scrap amounts on explored ground (nonzero only). Live
    /// amounts on visible tiles, beliefs elsewhere — the sim remembers
    /// them the same way.
    pub scrap: Vec<RememberedTileView>,
    /// Remembered wreck salvage, same treatment as scrap.
    pub wrecks: Vec<RememberedTileView>,
    /// Radar blips: bare tiles, no kind, no owner — detection without
    /// identification, exactly what the Array's outer ring grants.
    pub contacts: Vec<[i32; 2]>,
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
        let mut mask = Vec::with_capacity(height as usize);
        let mut scrap = Vec::new();
        let mut wrecks = Vec::new();
        for y in 0..height {
            let mut row = String::with_capacity(width as usize);
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
            mask,
            units: state
                .units()
                .iter()
                .filter(|u| !state.hostile(player, u.player) || vision.visible(u.tile()))
                .map(unit_view)
                .collect(),
            buildings: state
                .buildings()
                .iter()
                .filter(|b| {
                    !state.hostile(player, b.player) || b.tiles().any(|t| vision.visible(t))
                })
                .map(building_view)
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
        }
    }
}

fn default_true() -> bool {
    true
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
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
    /// Sim crate version.
    pub sim_version: String,
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
    /// Gameplay chrome geometry as [top_bar_h, panel_top, minimap x/y/w/h,
    /// panel_right, orders x/y/w/h] in window pixels — the same
    /// LayoutModel hit-testing reads, so an agent can aim clicks at (or
    /// away from) real chrome. The command band spans only to
    /// `panel_right`; `orders` is the queue dock on the left edge,
    /// zero-sized when absent. Menu modes report `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chrome: Option<[f32; 11]>,
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
                        .map(|(i, p)| PlayerView {
                            id: i as u8,
                            name: p.name.clone(),
                            faction: p.faction,
                            team: p.team,
                            scrap: p.scrap,
                            units: state
                                .units()
                                .iter()
                                .filter(|u| u.player.0 as usize == i)
                                .count(),
                            buildings: state
                                .buildings()
                                .iter()
                                .filter(|b| b.player.0 as usize == i)
                                .count(),
                        })
                        .collect()
                }
            } else {
                Default::default()
            },
            units: if filter.units {
                state.units().iter().map(unit_view).collect()
            } else {
                Default::default()
            },
            buildings: if filter.buildings {
                state.buildings().iter().map(building_view).collect()
            } else {
                Default::default()
            },
            map: filter.map.then(|| ascii_with_entities(state)),
        }
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
        carrying: u.carrying,
        order: u.order,
        queue: u.queue.iter().copied().collect(),
        patrolling: u.looping,
    }
}

fn building_view(b: &Building) -> BuildingView {
    BuildingView {
        id: b.id.0,
        player: b.player.0,
        kind: b.kind,
        anchor: [b.anchor.x, b.anchor.y],
        hp: b.hp,
        queue: b.queue.iter().copied().collect(),
        ticks_remaining: b
            .queue
            .front()
            .map(|kind| kind.stats().train_ticks.saturating_sub(b.progress)),
        rally: b.rally.map(|r| [r.x, r.y]),
        built: b.built,
        progress: b.progress,
    }
}

/// Terrain plus entity overlay: buildings print as `A` + player, units as
/// `a` + player (units win when both would claim a tile — they're what
/// moves).
fn ascii_with_entities(state: &State) -> Vec<String> {
    let mut rows: Vec<Vec<char>> = state
        .map()
        .ascii_rows()
        .into_iter()
        .map(|r| r.chars().collect())
        .collect();
    let mut put = |x: i32, y: i32, c: char| {
        if let Some(cell) = rows
            .get_mut(y as usize)
            .and_then(|row| row.get_mut(x as usize))
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
mod tests {
    use super::*;

    #[test]
    fn capture_respects_filters_and_overlays_entities() {
        let state = oxide_sim::Scenario::skirmish().build().unwrap();
        let full = StateView::capture(
            &state,
            StateFilter {
                map: true,
                ..StateFilter::default()
            },
        );
        assert_eq!(full.players.len(), 2);
        assert_eq!(full.units.len(), 8);
        assert_eq!(full.buildings.len(), 2);
        let map = full.map.as_ref().unwrap();
        let flat: String = map.concat();
        assert!(
            flat.contains('A') && flat.contains('B'),
            "both foundries drawn"
        );
        assert!(
            flat.contains('a') && flat.contains('b'),
            "both armies drawn"
        );

        let slim = StateView::capture(&state, StateFilter::default());
        assert!(slim.map.is_none());
        assert!(!slim.hash.is_empty() && slim.hash.starts_with("0x"));
        assert_eq!(slim.hash, full.hash, "views never perturb state");
    }

    #[test]
    fn the_fog_view_reports_only_what_the_seat_has_seen() {
        let state = oxide_sim::Scenario::skirmish().build().unwrap();
        let fog = FogView::capture(&state, PlayerId(0));
        let omniscient = StateView::capture(&state, StateFilter::default());

        // At tick zero the enemy base is dark: the honest view carries
        // strictly less than the omniscient one.
        assert!(fog.units.len() < omniscient.units.len());
        assert!(fog.buildings.len() < omniscient.buildings.len());
        assert!(fog.ghosts.is_empty(), "nothing hostile has been seen yet");

        let mask_at = |tile: [i32; 2]| {
            fog.mask[tile[1] as usize]
                .chars()
                .nth(tile[0] as usize)
                .expect("tile inside the mask")
        };
        let flat: String = fog.mask.concat();
        assert!(
            flat.contains('*') && flat.contains(' '),
            "the opening view has both sight and darkness"
        );
        for unit in fog.units.iter().filter(|u| u.player != 0) {
            assert_eq!(
                mask_at(unit.tile),
                '*',
                "a reported hostile must sit on visible ground"
            );
        }
        for entry in fog.scrap.iter().chain(&fog.wrecks) {
            assert_ne!(
                mask_at(entry.tile),
                ' ',
                "remembered salvage never leaks from unexplored ground"
            );
        }
    }

    #[test]
    fn unit_view_exposes_queued_orders() {
        let mut state = oxide_sim::Scenario::skirmish().build().unwrap();
        let mover = state.units()[0].id;
        state.tick(&[oxide_sim::PlayerCommand {
            player: oxide_sim::PlayerId(0),
            command: oxide_sim::Command::Patrol {
                units: vec![mover],
                waypoints: vec![
                    chassis::grid::TilePos::new(10, 10),
                    chassis::grid::TilePos::new(14, 6),
                    chassis::grid::TilePos::new(8, 12),
                ],
            },
        }]);
        let view = StateView::capture(&state, StateFilter::default());
        let u = view.units.iter().find(|u| u.id == mover.0).unwrap();
        assert!(u.patrolling);
        assert_eq!(u.queue.len(), 2, "the remaining circuit legs are visible");
    }

    #[test]
    fn building_view_reports_remaining_train_time() {
        let mut state = oxide_sim::Scenario::skirmish().build().unwrap();
        let foundry = state.buildings()[0].id;
        state.tick(&[oxide_sim::PlayerCommand {
            player: oxide_sim::PlayerId(0),
            command: oxide_sim::Command::Train {
                building: foundry,
                kind: UnitKind::Harvester,
            },
        }]);
        let view = StateView::capture(&state, StateFilter::default());
        let b = view.buildings.iter().find(|b| b.id == foundry.0).unwrap();
        assert_eq!(b.queue, vec![UnitKind::Harvester]);
        let remaining = b.ticks_remaining.unwrap();
        assert!(remaining > 0 && remaining <= UnitKind::Harvester.stats().train_ticks);
    }

    #[test]
    fn the_overlaid_debug_map_is_a_picture_not_a_parseable_scenario() {
        let state = oxide_sim::Scenario::skirmish().build().unwrap();
        let view = StateView::capture(
            &state,
            StateFilter {
                map: true,
                ..StateFilter::default()
            },
        );
        let rows = view.map.expect("map requested");
        let flat: String = rows.concat();
        assert!(
            flat.contains('A') || flat.contains('a'),
            "the overlay paints entities as letters"
        );
        // Those entity letters are not in the terrain legend, so the debug
        // map never round-trips back through the scenario parser — the same
        // reason the render-only wreck glyph stays out of authorable maps.
        match oxide_sim::map::Map::parse(&rows) {
            Err(oxide_sim::map::MapError::UnknownChar { c, .. }) => {
                assert!(
                    c.is_ascii_alphabetic(),
                    "rejected on an entity glyph, got {c:?}"
                );
            }
            other => panic!("expected the overlay to be unparseable terrain, got {other:?}"),
        }
    }
}
