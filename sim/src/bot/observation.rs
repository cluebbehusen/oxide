//! What a bot may know.
//!
//! An [`Observation`] is the only input a bot policy receives — versioned,
//! serializable, and buildable two ways: [`Observation::omniscient`] reads
//! the whole state (the classic cheating commander, honestly labeled), and
//! [`Observation::fog_honest`] filters through the player's own vision:
//! visible enemies live, remembered buildings as ghosts, remembered scrap
//! amounts, nothing else. The two produce the *same shape* — a policy
//! cannot tell which world it lives in, only how much of it it sees.
//!
//! Fog-honesty is enforced by explicit filtering, not trust:
//! `Vision` alone is not a safe boundary (the `State` behind it exposes
//! every player's economy), so the builder touches enemy state only
//! through visibility checks and vision memory, and a regression test
//! pins the guarantee that unseen enemy activity cannot change a single
//! serialized byte of a fog-honest observation.

use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::state::{Faction, Order, State};
use crate::stats::{BuildingKind, UnitKind};
use chassis::Tick;
use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};

/// Observation schema version — bump when the shape changes so recorded
/// training data and shipped policies can refuse mismatched worlds.
pub const OBSERVATION_VERSION: u32 = 4;

/// One unit as a bot sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitObs {
    /// Unit id (own units only carry meaning for command lowering; enemy
    /// ids are stable handles for targeting).
    pub id: UnitId,
    /// Owner.
    pub player: PlayerId,
    /// Kind.
    pub kind: UnitKind,
    /// Occupied tile. Deliberately tile- not position-resolution: policy
    /// decisions are macro decisions.
    pub tile: TilePos,
    /// Current hit points.
    pub hp: u32,
    /// Whether the unit is idle (own units only; always false for enemy
    /// observations — intent is not visible from outside).
    pub idle: bool,
    /// Scrap carried (own harvesters; zero otherwise).
    pub carrying: u32,
    /// The construction site this unit is building, if any (own units
    /// only; always `None` for enemy observations).
    pub site: Option<BuildingId>,
}

/// One building as a bot sees it. Enemy entries may be memories: `seen`
/// distinguishes live sight from a ghost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildingObs {
    /// Building id.
    pub id: BuildingId,
    /// Owner.
    pub player: PlayerId,
    /// Kind.
    pub kind: BuildingKind,
    /// Footprint anchor.
    pub anchor: TilePos,
    /// Hit points — for ghosts, as last seen.
    pub hp: u32,
    /// Whether construction had finished — for ghosts, as last seen.
    pub built: bool,
    /// Live sight right now (false = remembered ghost).
    pub seen: bool,
}

/// Everything a policy gets. Same shape for both builders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    /// Schema version ([`OBSERVATION_VERSION`]).
    pub version: u32,
    /// Sim tick this was taken at.
    pub tick: Tick,
    /// Whose eyes these are.
    pub me: PlayerId,
    /// Own bank.
    pub scrap: u32,
    /// Map width in tiles.
    pub map_width: i32,
    /// Map height in tiles.
    pub map_height: i32,
    /// Own units, id order.
    pub my_units: Vec<UnitObs>,
    /// Own buildings (queue lengths matter for production decisions).
    pub my_buildings: Vec<BuildingObs>,
    /// Training queue contents per own building, aligned with
    /// `my_buildings`.
    pub my_queues: Vec<Vec<UnitKind>>,
    /// Teammates' units — always in team sight, never commandable.
    /// Their intent is as opaque as an enemy's: allies coordinate by
    /// position, not telepathy.
    pub ally_units: Vec<UnitObs>,
    /// Teammates' buildings.
    pub ally_buildings: Vec<BuildingObs>,
    /// Enemy units this bot can currently justify knowing about.
    pub enemy_units: Vec<UnitObs>,
    /// Enemy buildings — live where seen, ghosts where remembered.
    pub enemy_buildings: Vec<BuildingObs>,
    /// Scrap nodes as known: `(tile, amount)` — live amounts under the
    /// omniscient builder, remembered amounts under the fog-honest one.
    /// Sorted by (y, x).
    pub known_scrap: Vec<(TilePos, u32)>,
    /// Impassable terrain as known (rock and peaks alike) — all of it
    /// omnisciently, explored tiles only fog-honestly (terrain is
    /// static, so once seen it is known forever). What placement and
    /// staging decisions steer around; sorted by (y, x).
    pub known_rock: Vec<TilePos>,
    /// Wreck salvage as known: `(tile, amount)` — live under the
    /// omniscient builder, remembered under the fog-honest one. Sorted
    /// by (y, x).
    pub known_wrecks: Vec<(TilePos, u32)>,
    /// Radar blips: tiles holding an unidentified hostile contact inside
    /// an Array's outer ring but out of sight. Always empty under the
    /// omniscient builder (it has no unidentified anything). Sorted by
    /// (y, x).
    pub blips: Vec<TilePos>,
    /// The seat's faction — which variants of the varied roles it may
    /// train.
    pub faction: Faction,
    /// Own shells in flight, counted not located — the policy knows
    /// its guns spoke.
    pub my_shells: usize,
    /// Impact tiles of hostile shells this seat can currently justify
    /// seeing (fog-honest: the impact tile must be visible — the same
    /// rule the arc renderer draws by). Sorted by (y, x).
    pub incoming_shells: Vec<TilePos>,
}

impl Observation {
    /// The classic cheating commander's view: everything, live.
    pub fn omniscient(state: &State, me: PlayerId) -> Self {
        let mut obs = Self::base(state, me);
        for u in state.units() {
            if u.hp == 0 {
                continue;
            }
            if u.player == me {
                obs.my_units.push(own_unit(u));
            } else if !state.hostile(me, u.player) {
                obs.ally_units.push(enemy_unit(u));
            } else {
                obs.enemy_units.push(enemy_unit(u));
            }
        }
        for b in state.buildings() {
            if b.player == me {
                obs.my_buildings.push(own_building(b));
                obs.my_queues.push(b.queue.iter().copied().collect());
            } else if !state.hostile(me, b.player) {
                obs.ally_buildings.push(BuildingObs {
                    id: b.id,
                    player: b.player,
                    kind: b.kind,
                    anchor: b.anchor,
                    hp: b.hp,
                    built: b.built,
                    seen: true,
                });
            } else {
                obs.enemy_buildings.push(BuildingObs {
                    id: b.id,
                    player: b.player,
                    kind: b.kind,
                    anchor: b.anchor,
                    hp: b.hp,
                    built: b.built,
                    seen: true,
                });
            }
        }
        for (pos, tile) in state.map().iter() {
            if tile.scrap > 0 {
                obs.known_scrap.push((pos, tile.scrap));
            }
            if tile.wreck > 0 {
                obs.known_wrecks.push((pos, tile.wreck));
            }
            if tile.terrain != crate::map::Terrain::Ground {
                obs.known_rock.push(pos);
            }
        }
        obs.my_shells = state.shells().iter().filter(|s| s.player == me).count();
        obs.incoming_shells = state
            .shells()
            .iter()
            .filter(|s| state.hostile(me, s.player))
            .map(|s| TilePos::containing(s.impact))
            .collect();
        obs.incoming_shells.sort_by_key(|p| (p.y, p.x));
        obs
    }

    /// The fair view: own side in full, the enemy only as this player's
    /// vision can currently justify — live where visible, ghost memory
    /// where remembered, absent where never seen.
    pub fn fog_honest(state: &State, me: PlayerId) -> Self {
        let mut obs = Self::base(state, me);
        let vision = state.vision(me);
        for u in state.units() {
            if u.hp == 0 {
                continue;
            }
            if u.player == me {
                obs.my_units.push(own_unit(u));
            } else if !state.hostile(me, u.player) {
                // Teammates stamp this player's vision, so they are
                // always in sight by construction.
                obs.ally_units.push(enemy_unit(u));
            } else if vision.visible(u.tile()) {
                obs.enemy_units.push(enemy_unit(u));
            }
        }
        for b in state.buildings() {
            if b.player == me {
                obs.my_buildings.push(own_building(b));
                obs.my_queues.push(b.queue.iter().copied().collect());
            } else if !state.hostile(me, b.player) {
                obs.ally_buildings.push(BuildingObs {
                    id: b.id,
                    player: b.player,
                    kind: b.kind,
                    anchor: b.anchor,
                    hp: b.hp,
                    built: b.built,
                    seen: true,
                });
            } else if b.tiles().any(|t| vision.visible(t)) {
                obs.enemy_buildings.push(BuildingObs {
                    id: b.id,
                    player: b.player,
                    kind: b.kind,
                    anchor: b.anchor,
                    hp: b.hp,
                    built: b.built,
                    seen: true,
                });
            }
        }
        // Ghost memories cover ground currently out of sight.
        for ghost in vision.ghosts() {
            let visible_now = {
                let (w, h) = ghost.kind.stats().size;
                (0..h)
                    .flat_map(|dy| (0..w).map(move |dx| ghost.anchor.offset(dx, dy)))
                    .any(|t| vision.visible(t))
            };
            if !visible_now {
                obs.enemy_buildings.push(BuildingObs {
                    // Ghosts carry no live id contract; the anchor is the
                    // stable handle. Id 0 would collide with a real
                    // building, so use the sentinel ceiling.
                    id: BuildingId(u32::MAX),
                    player: ghost.owner,
                    kind: ghost.kind,
                    anchor: ghost.anchor,
                    hp: ghost.hp,
                    built: ghost.built,
                    seen: false,
                });
            }
        }
        obs.enemy_buildings
            .sort_by_key(|b| (b.anchor.y, b.anchor.x, b.player));
        // Remembered salvage: what this player last saw, everywhere. Rock
        // is static, so explored is knowledge enough.
        for (pos, tile) in state.map().iter() {
            let amount = if vision.visible(pos) {
                state.map().scrap_at(pos)
            } else {
                vision.remembered_scrap(pos)
            };
            if amount > 0 {
                obs.known_scrap.push((pos, amount));
            }
            let wreck = if vision.visible(pos) {
                state.map().wreck_at(pos)
            } else {
                vision.remembered_wreck(pos)
            };
            if wreck > 0 {
                obs.known_wrecks.push((pos, wreck));
            }
            if tile.terrain != crate::map::Terrain::Ground && vision.explored(pos) {
                obs.known_rock.push(pos);
            }
        }
        // Blips ride through untouched: tiles only, by construction.
        obs.blips = vision.contacts().to_vec();
        obs.my_shells = state.shells().iter().filter(|s| s.player == me).count();
        obs.incoming_shells = state
            .shells()
            .iter()
            .filter(|s| state.hostile(me, s.player))
            .map(|s| TilePos::containing(s.impact))
            .filter(|t| vision.visible(*t))
            .collect();
        obs.incoming_shells.sort_by_key(|p| (p.y, p.x));
        obs
    }

    fn base(state: &State, me: PlayerId) -> Self {
        Self {
            version: OBSERVATION_VERSION,
            tick: state.current_tick(),
            me,
            scrap: state.player(me).scrap,
            map_width: state.map().width(),
            map_height: state.map().height(),
            my_units: Vec::new(),
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_wrecks: Vec::new(),
            blips: Vec::new(),
            faction: state.player(me).faction,
            my_shells: 0,
            incoming_shells: Vec::new(),
        }
    }
}

fn own_unit(u: &crate::state::Unit) -> UnitObs {
    UnitObs {
        id: u.id,
        player: u.player,
        kind: u.kind,
        tile: u.tile(),
        hp: u.hp,
        idle: u.order == Order::Idle,
        carrying: u.carrying,
        site: match u.order {
            Order::Build { site } => Some(site),
            _ => None,
        },
    }
}

fn enemy_unit(u: &crate::state::Unit) -> UnitObs {
    UnitObs {
        id: u.id,
        player: u.player,
        kind: u.kind,
        tile: u.tile(),
        hp: u.hp,
        idle: false, // enemy intent is not observable
        carrying: 0, // nor their cargo manifests
        site: None,  // nor their work orders
    }
}

fn own_building(b: &crate::state::Building) -> BuildingObs {
    BuildingObs {
        id: b.id,
        player: b.player,
        kind: b.kind,
        anchor: b.anchor,
        hp: b.hp,
        built: b.built,
        seen: true,
    }
}
