//! Read-only views of sim state, shaped for reading rather than simulating.
//!
//! The sim's own serialization is exact (fixed-point bits) and therefore
//! unreadable; these views trade exactness for legibility — positions as
//! floats, the map as ASCII with entities overlaid. Anything that needs
//! exactness should use the state hash, not a view.

use oxide_sim::{Building, Faction, GameResult, Order, State, Unit, UnitKind};
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
}
