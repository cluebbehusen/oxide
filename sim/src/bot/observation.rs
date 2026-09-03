//! What a bot may know.
//!
//! An [`Observation`] is the only input a bot policy receives. It is
//! serializable and buildable two ways: [`Observation::omniscient`] exposes
//! the whole state for focused tests, while [`Observation::fog_honest`]
//! filters through the player's own vision:
//! visible enemies live, remembered buildings as ghosts, remembered scrap
//! amounts, and anonymous team-shared salvage warnings, nothing else. The two
//! produce the *same shape* — a policy cannot tell which world it lives in,
//! only how much of it it sees.
//!
//! Fog-honesty is enforced by explicit filtering, not trust:
//! `Vision` alone is not a safe boundary (the `State` behind it exposes
//! every player's economy), so the builder touches enemy state only
//! through visibility checks and vision memory, and a regression test
//! pins the guarantee that unseen enemy activity cannot change a single
//! serialized byte of a fog-honest observation.

use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::state::{Faction, Order, State};
use crate::stats::{BuildingKind, Domain, UnitKind};
use chassis::Tick;
use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};

/// Schema version for serialized observation snapshots. Increment it when
/// fields or their meaning change so tools can reject incompatible data.
/// Version 9 added current visibility. Version 10 exposes whether an own unit's
/// current program is voluntary paid repair work. Version 11 exposes the
/// team's anonymous, bounded salvage-danger incidents. Version 12 exposes
/// whether an airframe is parked on the ground. Version 13 exposes an own
/// Harvester's current work node without revealing allied or enemy orders.
/// Version 14 exposes which own units have queued or looping programs. Version
/// 15 exposes exact owner-visible progress for the front of each training
/// queue.
pub const OBSERVATION_VERSION: u32 = 15;

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
    /// The salvage node this unit is currently harvesting, if any (own units
    /// only; always `None` for allies and enemies).
    #[serde(default)]
    pub harvesting: Option<TilePos>,
    /// Sling room its riders occupy (own transports; zero otherwise).
    #[serde(default)]
    pub cargo: u8,
    /// The construction site this unit is building, if any (own units
    /// only; always `None` for enemy observations).
    pub site: Option<BuildingId>,
    /// The building this unit is stripping, if any (own units only —
    /// the repair channel reads it to keep the two verbs off one
    /// target; enemy work orders stay opaque).
    pub salvaging: Option<BuildingId>,
    /// The deferred claim this unit is walking out to, if any (own
    /// units only): the promised kind and footprint anchor of a live
    /// [`Order::Found`]. A walking founder is spoken for — the site
    /// audit waits on it and the labor choosers keep off it — and no
    /// site exists to carry an id until the claim lands.
    pub founding: Option<(BuildingKind, TilePos)>,
    /// Whether the current program is voluntary building or unit repair (own
    /// units only; always false for allies and enemies).
    #[serde(default)]
    pub repairing: bool,
    /// Whether the airframe is parked on the ground. Unlike `idle` or
    /// `cargo` this is a physical fact anyone in sight can see, so it is
    /// reported for allies and enemies too. Always false for kinds that
    /// cannot land.
    #[serde(default)]
    pub grounded: bool,
}

impl UnitObs {
    /// The movement layer this body occupies right now: a grounded
    /// airframe is a ground body for targeting and matchups, whatever its
    /// kind flies as. Routing and procurement keep reading the kind's
    /// domain because the next flight is planned in the air.
    pub fn body_domain(&self) -> Domain {
        if self.grounded {
            Domain::Ground
        } else {
            self.kind.stats().domain
        }
    }
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
    /// Upgrade-ladder rung (0 = base; ghosts report their last-seen
    /// hull, which for now is always the base row).
    #[serde(default)]
    pub tier: u8,
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
    /// Current progress of the front training item per own building, aligned
    /// with `my_buildings` and `my_queues`. Empty queues report zero.
    pub my_queue_progress: Vec<u32>,
    /// Own units whose current order has a queued continuation or loops.
    /// Sorted by id. The continuation itself stays opaque to policy code;
    /// this ownership bit is enough to keep autonomous work from replacing a
    /// player's existing program with a non-queued command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub my_queued_units: Vec<UnitId>,
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
    /// Row-major current team-visibility mask. Unlike `explored`, this may
    /// become false again after a scout leaves. Policies use it to distinguish
    /// fresh negative evidence from remembered terrain.
    pub visible: Vec<bool>,
    /// Row-major fog exploration mask. This is the exact knowledge boundary
    /// for deferred placement: a bot may assume unknown routes continue, but
    /// it may not promise a foundation on a tile its team has never seen.
    pub explored: Vec<bool>,
    /// Scrap nodes as known: `(tile, amount)` — live amounts under the
    /// omniscient builder, remembered amounts under the fog-honest one.
    /// Sorted by (y, x).
    pub known_scrap: Vec<(TilePos, u32)>,
    /// Impassable terrain as known (rock and peaks alike) — all of it
    /// omnisciently, explored tiles only fog-honestly (terrain is
    /// static, so once seen it is known forever). What placement and
    /// staging decisions steer around; sorted by (y, x).
    pub known_rock: Vec<TilePos>,
    /// Derelict Extractor frame anchors on explored ground (all of
    /// them, omnisciently). Frames are map facts and never move.
    #[serde(default)]
    pub known_frames: Vec<TilePos>,
    /// Explored peak terrain, also present in `known_rock`. This separate
    /// subset is what air routing and peak-blocked fire steer around while
    /// ordinary rock remains open sky. Sorted by (y, x).
    pub known_peaks: Vec<TilePos>,
    /// Wreck salvage as known: `(tile, amount)` — live under the
    /// omniscient builder, remembered under the fog-honest one. Sorted
    /// by (y, x).
    pub known_wrecks: Vec<(TilePos, u32)>,
    /// Recent allied damage or loss locations that remain unsafe for
    /// autonomous salvage. These are anonymous team-shared warning tiles,
    /// already bounded and expired by the authoritative vision system.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub salvage_incidents: Vec<TilePos>,
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

/// An empty zero-sized observation: the shared base every test fixture
/// spreads over, so a new field is declared here once instead of fanning
/// out across every hand-built literal. The struct-literal orientation
/// test in `orient` deliberately does not use it — a new field must
/// still declare its seat flip exhaustively there before this default
/// can carry it anywhere else.
#[cfg(test)]
impl Default for Observation {
    fn default() -> Self {
        Self {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 0,
            map_width: 0,
            map_height: 0,
            my_units: Vec::new(),
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            my_queue_progress: Vec::new(),
            my_queued_units: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: Vec::new(),
            explored: Vec::new(),
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: crate::state::Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        }
    }
}

impl Observation {
    /// Whether an own unit already has work queued behind its current order or
    /// is running a looping program.
    pub fn has_queued_program(&self, unit: UnitId) -> bool {
        self.my_queued_units.binary_search(&unit).is_ok()
    }

    /// Exact progress for one own building's front training item. Malformed
    /// alignment or progress outside the front item's legal range yields no
    /// timing evidence.
    pub(crate) fn own_queue_progress(&self, index: usize) -> Option<u32> {
        if self.my_buildings.len() != self.my_queues.len()
            || self.my_queue_progress.len() != self.my_buildings.len()
        {
            return None;
        }
        let progress = *self.my_queue_progress.get(index)?;
        match self.my_queues.get(index)?.first() {
            Some(kind) if progress <= kind.stats().train_ticks => Some(progress),
            None if progress == 0 => Some(0),
            Some(_) | None => None,
        }
    }

    /// Whether `tile` is known impassable terrain — a binary point lookup
    /// into `known_rock`, which is sorted by (y, x) both by row-major
    /// construction and by the orientation re-sort.
    pub fn known_rock_at(&self, tile: TilePos) -> bool {
        self.known_rock
            .binary_search_by_key(&(tile.y, tile.x), |p| (p.y, p.x))
            .is_ok()
    }

    /// Whether `tile` holds a known scrap node — the same sorted point
    /// lookup into `known_scrap`.
    pub fn known_scrap_at(&self, tile: TilePos) -> bool {
        self.known_scrap
            .binary_search_by_key(&(tile.y, tile.x), |(p, _)| (p.y, p.x))
            .is_ok()
    }

    /// Whether `tile` has ever been seen by this seat's team.
    pub fn explored(&self, tile: TilePos) -> bool {
        self.mask_value(&self.explored, tile)
    }

    /// Whether `tile` is in this seat's current team vision.
    pub fn visible(&self, tile: TilePos) -> bool {
        self.mask_value(&self.visible, tile)
    }

    fn mask_value(&self, mask: &[bool], tile: TilePos) -> bool {
        if tile.x < 0 || tile.y < 0 || tile.x >= self.map_width || tile.y >= self.map_height {
            return false;
        }
        let Ok(width) = usize::try_from(self.map_width) else {
            return false;
        };
        let Ok(x) = usize::try_from(tile.x) else {
            return false;
        };
        let Ok(y) = usize::try_from(tile.y) else {
            return false;
        };
        y.checked_mul(width)
            .and_then(|row| row.checked_add(x))
            .and_then(|index| mask.get(index))
            .copied()
            .unwrap_or(false)
    }

    /// The complete live view used by focused policy tests.
    pub fn omniscient(state: &State, me: PlayerId) -> Self {
        let mut obs = Self::base(state, me);
        for u in state.units() {
            if u.hp == 0 {
                continue;
            }
            if u.player == me {
                obs.my_units.push(own_unit(u));
                if !u.queue.is_empty() || u.looping {
                    obs.my_queued_units.push(u.id);
                }
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
                obs.my_queue_progress
                    .push(if b.queue.is_empty() { 0 } else { b.progress });
            } else if !state.hostile(me, b.player) {
                obs.ally_buildings.push(BuildingObs {
                    id: b.id,
                    player: b.player,
                    kind: b.kind,
                    anchor: b.anchor,
                    hp: b.hp,
                    built: b.built,
                    seen: true,
                    tier: b.tier,
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
                    tier: b.tier,
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
            if tile.terrain.blocks_ground() {
                obs.known_rock.push(pos);
            }
            if state.map().is_extractor_frame(pos) {
                obs.known_frames.push(pos);
            }
            if tile.terrain.blocks_air() {
                obs.known_peaks.push(pos);
            }
        }
        let tile_count = state.map().iter().count();
        obs.visible = vec![true; tile_count];
        obs.explored = vec![true; tile_count];
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
                if !u.queue.is_empty() || u.looping {
                    obs.my_queued_units.push(u.id);
                }
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
                obs.my_queue_progress
                    .push(if b.queue.is_empty() { 0 } else { b.progress });
            } else if !state.hostile(me, b.player) {
                obs.ally_buildings.push(BuildingObs {
                    id: b.id,
                    player: b.player,
                    kind: b.kind,
                    anchor: b.anchor,
                    hp: b.hp,
                    built: b.built,
                    seen: true,
                    tier: b.tier,
                });
            } else if b.tiles().any(|t| vision.visible(t)) && state.building_apparent(me, b) {
                obs.enemy_buildings.push(BuildingObs {
                    id: b.id,
                    player: b.player,
                    kind: b.kind,
                    anchor: b.anchor,
                    hp: b.hp,
                    built: b.built,
                    seen: true,
                    tier: b.tier,
                });
            }
        }
        // Ghost memories cover ground currently out of sight.
        for ghost in vision.ghosts() {
            let visible_now = {
                let (w, h) = ghost.kind.base_stats().size;
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
                    tier: 0,
                });
            }
        }
        obs.enemy_buildings
            .sort_by_key(|b| (b.anchor.y, b.anchor.x, b.player));
        // Remembered salvage: what this player last saw, everywhere. Rock
        // is static, so explored is knowledge enough.
        // Row slices, the way vision::refresh itself walks: the point
        // accessors re-tested the same fog bits up to seven times per
        // tile, and the per-tile frame test rescanned the frame list
        // for every cell — the frames get their own single pass below.
        for y in 0..state.map().height() {
            let (visible, explored, scrap_mem, wreck_mem) = vision.rows(y).expect("row in range");
            let tiles = state.map().grid().row(y).expect("row in range");
            for (x, tile) in tiles.iter().enumerate() {
                let pos = TilePos::new(x as i32, y);
                let seen = visible[x];
                let known = explored[x];
                obs.visible.push(seen);
                obs.explored.push(known);
                let amount = if seen { tile.scrap } else { scrap_mem[x] };
                if amount > 0 {
                    obs.known_scrap.push((pos, amount));
                }
                let wreck = if seen { tile.wreck } else { wreck_mem[x] };
                if wreck > 0 {
                    obs.known_wrecks.push((pos, wreck));
                }
                if known {
                    if tile.terrain.blocks_ground() {
                        obs.known_rock.push(pos);
                    }
                    if tile.terrain.blocks_air() {
                        obs.known_peaks.push(pos);
                    }
                }
            }
        }
        // Frames are authored in row-major order, the same order the
        // per-tile walk produced them in.
        for frame in state.map().extractor_frames() {
            if vision.explored(*frame) {
                obs.known_frames.push(*frame);
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
            my_queue_progress: Vec::new(),
            my_queued_units: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: Vec::new(),
            explored: Vec::new(),
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: state
                .vision(me)
                .salvage_incidents()
                .iter()
                .filter(|incident| incident.expires_at > state.current_tick())
                .map(|incident| incident.tile)
                .collect(),
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
        harvesting: match u.order {
            Order::Harvest { node, .. } => Some(node),
            _ => None,
        },
        cargo: u.cargo.iter().map(|r| r.kind.stats().transport_size).sum(),
        site: match u.order {
            Order::Build { site } => Some(site),
            _ => None,
        },
        salvaging: match u.order {
            Order::Salvage { building } => Some(building),
            _ => None,
        },
        founding: match u.order {
            Order::Found { kind, anchor } => Some((kind, anchor)),
            _ => None,
        },
        repairing: matches!(u.order, Order::Repair { .. } | Order::RepairUnit { .. }),
        grounded: u.landed,
    }
}

fn enemy_unit(u: &crate::state::Unit) -> UnitObs {
    UnitObs {
        id: u.id,
        player: u.player,
        kind: u.kind,
        tile: u.tile(),
        hp: u.hp,
        idle: false,      // enemy intent is not observable
        carrying: 0,      // nor their cargo manifests
        harvesting: None, // nor their work orders
        cargo: 0,         // a sealed sling shows nothing
        site: None,       // nor their work orders
        salvaging: None,  // ditto
        founding: None,   // ditto
        repairing: false,
        grounded: u.landed,
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
        tier: b.tier,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, PlayerCommand};
    use crate::scenario::{Scenario, UnitSpec};
    use crate::stats::UnitKind;
    use chassis::grid::TilePos;

    fn transport_scenario() -> Scenario {
        let mut scenario = Scenario::skirmish();
        scenario.name = "observation-transport".into();
        scenario.units = vec![
            UnitSpec {
                player: 0,
                kind: UnitKind::Skyhook,
                x: 8,
                y: 5,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Harvester,
                x: 7,
                y: 5,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Sentinel,
                x: 9,
                y: 5,
            },
            UnitSpec {
                player: 0,
                kind: UnitKind::Lancer,
                x: 8,
                y: 6,
            },
        ];
        scenario
    }

    #[test]
    fn owner_training_progress_is_exact_aligned_and_required_by_the_schema() {
        let mut state = Scenario::skirmish()
            .build()
            .expect("the skirmish scenario builds");
        let producer = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0) && building.kind == BuildingKind::Foundry
            })
            .expect("player zero has a Foundry")
            .id;
        let foundry = state
            .building_mut(producer)
            .expect("the Foundry remains live");
        foundry.queue.clear();
        foundry.queue.push_back(UnitKind::Harvester);
        foundry.progress = 17;

        for observation in [
            Observation::fog_honest(&state, PlayerId(0)),
            Observation::omniscient(&state, PlayerId(0)),
        ] {
            let index = observation
                .my_buildings
                .iter()
                .position(|building| building.id == producer)
                .expect("the owner observes its producer");
            assert_eq!(observation.my_queues[index], vec![UnitKind::Harvester]);
            assert_eq!(observation.my_queue_progress[index], 17);
            assert_eq!(observation.own_queue_progress(index), Some(17));
            assert_eq!(observation.my_buildings.len(), observation.my_queues.len());
            assert_eq!(
                observation.my_buildings.len(),
                observation.my_queue_progress.len()
            );

            let encoded = serde_json::to_value(&observation).expect("the observation serializes");
            assert_eq!(
                serde_json::from_value::<Observation>(encoded.clone())
                    .expect("the current observation schema round-trips"),
                observation
            );
            let mut obsolete = encoded;
            obsolete
                .as_object_mut()
                .expect("an observation serializes as an object")
                .remove("my_queue_progress");
            assert!(
                serde_json::from_value::<Observation>(obsolete).is_err(),
                "an older snapshot must not silently invent exact progress"
            );
        }
    }

    #[test]
    fn hostile_training_progress_never_enters_an_observation() {
        let control = Scenario::skirmish()
            .build()
            .expect("the skirmish scenario builds");
        let mut variant = control.clone();
        let hostile_producer = variant
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(1) && building.kind == BuildingKind::Foundry
            })
            .expect("player one has a Foundry")
            .id;
        let foundry = variant
            .building_mut(hostile_producer)
            .expect("the hostile Foundry remains live");
        foundry.queue.push_back(UnitKind::Harvester);
        foundry.progress = 31;

        assert_eq!(
            Observation::fog_honest(&control, PlayerId(0)),
            Observation::fog_honest(&variant, PlayerId(0)),
            "fog-honest policy input must not expose hostile production intent"
        );
        assert_eq!(
            Observation::omniscient(&control, PlayerId(0)),
            Observation::omniscient(&variant, PlayerId(0)),
            "even the complete test view keeps hostile production private"
        );
    }

    #[test]
    fn owner_observation_reports_exact_sling_occupancy_without_leaking_the_manifest() {
        let mut state = transport_scenario()
            .build()
            .expect("the transport scenario builds");
        let skyhook = state
            .units()
            .iter()
            .find(|unit| unit.kind == UnitKind::Skyhook)
            .expect("the Skyhook exists")
            .id;
        let riders: Vec<_> = state
            .units()
            .iter()
            .filter(|unit| unit.id != skyhook)
            .map(|unit| unit.id)
            .collect();
        let expected_occupancy: u8 = state
            .units()
            .iter()
            .filter(|unit| unit.id != skyhook)
            .map(|unit| unit.kind.stats().transport_size)
            .sum();
        assert_eq!(expected_occupancy, 4, "the authored squad fills the sling");

        state.tick(&[PlayerCommand {
            player: PlayerId(0),
            command: Command::Load {
                units: riders,
                transport: skyhook,
                queue: false,
            },
        }]);
        for _ in 0..200 {
            if state
                .unit(skyhook)
                .is_some_and(|transport| transport.cargo.len() == 3)
            {
                break;
            }
            state.tick(&[]);
        }
        assert_eq!(
            state
                .unit(skyhook)
                .expect("the loaded Skyhook remains alive")
                .cargo
                .len(),
            3,
            "the whole mixed-size squad boards"
        );

        for observation in [
            Observation::fog_honest(&state, PlayerId(0)),
            Observation::omniscient(&state, PlayerId(0)),
        ] {
            let transport = observation
                .my_units
                .iter()
                .find(|unit| unit.id == skyhook)
                .expect("the owner observes its Skyhook");
            assert_eq!(transport.cargo, expected_occupancy);
        }

        let opponent = Observation::omniscient(&state, PlayerId(1));
        let transport = opponent
            .enemy_units
            .iter()
            .find(|unit| unit.id == skyhook)
            .expect("the complete test view includes the hostile Skyhook");
        assert_eq!(
            transport.cargo, 0,
            "an opponent may see the airframe, but not its sealed manifest"
        );
    }

    #[test]
    fn a_parked_airframe_is_grounded_for_its_owner_and_for_anyone_who_sees_it() {
        let mut scenario = Scenario::skirmish();
        scenario.name = "observation-landing".into();
        scenario.units = vec![
            // Mid-basin, clear of both Foundries: nothing in the Condor's
            // acquisition range, so its move ends in a landing. The
            // Sentinel watches from inside its sight and outside its aggro.
            UnitSpec {
                player: 0,
                kind: UnitKind::Condor,
                x: 20,
                y: 12,
            },
            UnitSpec {
                player: 1,
                kind: UnitKind::Sentinel,
                x: 14,
                y: 12,
            },
        ];
        let mut state = scenario.build().expect("the landing scenario builds");
        let condor = state
            .units()
            .iter()
            .find(|unit| unit.kind == UnitKind::Condor)
            .expect("the Condor exists")
            .id;
        let goal = TilePos::new(20, 12);

        let airborne = Observation::omniscient(&state, PlayerId(1));
        let flying = airborne
            .enemy_units
            .iter()
            .find(|unit| unit.id == condor)
            .expect("the complete test view includes the hostile Condor");
        assert!(!flying.grounded);
        assert_eq!(flying.body_domain(), crate::stats::Domain::Air);

        state.tick(&[PlayerCommand {
            player: PlayerId(0),
            command: Command::Move {
                units: vec![condor],
                goal,
                queue: false,
            },
        }]);
        for _ in 0..1_500 {
            if state.unit(condor).is_some_and(|unit| unit.landed) {
                break;
            }
            state.tick(&[]);
        }
        let parked = state.unit(condor).expect("the Condor survives its landing");
        assert!(
            parked.landed,
            "the Condor sets down within the flight budget"
        );
        let tile = parked.tile();
        assert!(
            state.vision(PlayerId(1)).visible(tile),
            "the opponent's Sentinel watches the landing tile"
        );

        for observation in [
            Observation::fog_honest(&state, PlayerId(0)),
            Observation::omniscient(&state, PlayerId(0)),
        ] {
            let own = observation
                .my_units
                .iter()
                .find(|unit| unit.id == condor)
                .expect("the owner observes its Condor");
            assert!(own.grounded);
            assert_eq!(own.body_domain(), crate::stats::Domain::Ground);
        }
        for observation in [
            Observation::fog_honest(&state, PlayerId(1)),
            Observation::omniscient(&state, PlayerId(1)),
        ] {
            let seen = observation
                .enemy_units
                .iter()
                .find(|unit| unit.id == condor)
                .expect("the opponent sees the parked Condor");
            assert!(
                seen.grounded,
                "a parked airframe is a physical fact, not private intent"
            );
            assert_eq!(seen.body_domain(), crate::stats::Domain::Ground);
        }
    }

    #[test]
    fn owner_observation_exposes_the_active_harvest_node_without_leaking_enemy_orders() {
        let mut state = Scenario::skirmish()
            .build()
            .expect("the skirmish scenario builds");
        let worker = |player| {
            state
                .units()
                .iter()
                .find(|unit| unit.player == player && unit.kind == UnitKind::Harvester)
                .expect("each skirmish seat starts with a Harvester")
                .id
        };
        let known_node = |player| {
            state
                .map()
                .iter()
                .find(|(tile, cell)| cell.scrap > 0 && state.vision(player).visible(*tile))
                .map(|(tile, _)| tile)
                .expect("each skirmish seat starts with visible salvage")
        };
        let p0_worker = worker(PlayerId(0));
        let p1_worker = worker(PlayerId(1));
        let p0_node = known_node(PlayerId(0));
        let p1_node = known_node(PlayerId(1));

        state.tick(&[
            PlayerCommand {
                player: PlayerId(0),
                command: Command::Harvest {
                    units: vec![p0_worker],
                    node: p0_node,
                    queue: false,
                },
            },
            PlayerCommand {
                player: PlayerId(1),
                command: Command::Harvest {
                    units: vec![p1_worker],
                    node: p1_node,
                    queue: false,
                },
            },
        ]);

        for (observer, own_worker, own_node, hostile_worker) in [
            (PlayerId(0), p0_worker, p0_node, p1_worker),
            (PlayerId(1), p1_worker, p1_node, p0_worker),
        ] {
            let observation = Observation::omniscient(&state, observer);
            assert_eq!(
                observation
                    .my_units
                    .iter()
                    .find(|unit| unit.id == own_worker)
                    .expect("the owner observes its Harvester")
                    .harvesting,
                Some(own_node)
            );
            assert_eq!(
                observation
                    .enemy_units
                    .iter()
                    .find(|unit| unit.id == hostile_worker)
                    .expect("the complete test view includes the hostile Harvester")
                    .harvesting,
                None,
                "an enemy's Harvest order remains private even in a complete test view"
            );
        }
    }

    #[test]
    fn owner_observation_marks_queued_and_looping_programs_without_leaking_them() {
        let mut state = Scenario::skirmish()
            .build()
            .expect("the skirmish scenario builds");
        let own_workers: Vec<_> = state
            .units()
            .iter()
            .filter(|unit| unit.player == PlayerId(0) && unit.kind == UnitKind::Harvester)
            .take(2)
            .map(|unit| unit.id)
            .collect();
        let hostile_worker = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(1) && unit.kind == UnitKind::Harvester)
            .expect("the opposing seat starts with a Harvester")
            .id;
        state
            .unit_mut(own_workers[0])
            .expect("the first own Harvester exists")
            .queue
            .push_back(Order::Move {
                goal: TilePos::new(8, 8),
            });
        state
            .unit_mut(own_workers[1])
            .expect("the second own Harvester exists")
            .looping = true;
        state
            .unit_mut(hostile_worker)
            .expect("the hostile Harvester exists")
            .queue
            .push_back(Order::Move {
                goal: TilePos::new(30, 15),
            });

        for observation in [
            Observation::fog_honest(&state, PlayerId(0)),
            Observation::omniscient(&state, PlayerId(0)),
        ] {
            assert_eq!(observation.my_queued_units, own_workers);
            assert!(observation.has_queued_program(own_workers[0]));
            assert!(observation.has_queued_program(own_workers[1]));
            assert!(!observation.has_queued_program(hostile_worker));
        }
    }
}
