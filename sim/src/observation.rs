//! What a bot may know.
//!
//! An [`ObservationData`] is the only input a bot policy receives. It is
//! serializable and buildable two ways: [`ObservationData::omniscient`] exposes
//! the whole state for focused tests, while [`ObservationData::fog_honest`]
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

use crate::ids::{BuildingId, PlayerId, Target, UnitId};
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
/// queue. Version 16 exposes exact own active repair targets. Version 17 adds
/// owner-only carried identities separately from available units.
/// Version 19 marks provisional building footprints and reports paid deferred
/// construction through `UnitObs::site` instead of `UnitObs::founding`.
/// Version 20 distinguishes explored pits from fire-blocking rock and peaks.
pub const OBSERVATION_VERSION: u32 = 20;

/// An own passenger that remains alive but is unavailable for new assignments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedUnitObs {
    /// Exact transport holding the passenger.
    pub carrier: UnitId,
    /// Passenger identity.
    pub id: UnitId,
    /// Passenger kind, for ordinary replacement valuation.
    pub kind: UnitKind,
    /// Current passenger health.
    pub hp: u32,
}

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
    /// The paid construction site this unit is approaching or building,
    /// including provisional scaffolds (own units only).
    pub site: Option<BuildingId>,
    /// The building this unit is stripping, if any (own units only —
    /// the repair channel reads it to keep the two verbs off one
    /// target; enemy work orders stay opaque).
    pub salvaging: Option<BuildingId>,
    /// A deferred intent without an associated paid site (own units only).
    /// Paid provisional construction is reported through `site`.
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
    /// A paid plan whose ground is not yet verified; it has no physical occupancy.
    pub provisional: bool,
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

/// Serializable player knowledge, independent of derived navigation inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationData {
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
    /// Own carried passengers, ordered by carrier then passenger id. They are
    /// not commandable inventory and reveal no allied or hostile manifests.
    pub my_carried_units: Vec<CarriedUnitObs>,
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
    /// Active repair targets of own workers, sorted by worker id. Allied and
    /// hostile programs remain opaque even when their bodies are visible.
    pub my_repair_targets: Vec<(UnitId, Target)>,
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
    /// Impassable terrain as known (rock, peaks, and pits) — all of it
    /// omnisciently, explored tiles only fog-honestly (terrain is
    /// static, so once seen it is known forever). What placement and
    /// staging decisions steer around; sorted by (y, x).
    pub known_rock: Vec<TilePos>,
    /// Explored pits, also in `known_rock`, which block travel but not fire.
    /// Sorted by (y, x).
    #[serde(default)]
    pub known_pits: Vec<TilePos>,
    /// Derelict Extractor frame anchors discovered through any footprint
    /// tile (all of them, omnisciently). Frames are map facts and never move.
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
    /// Team-local contacts usable by ordinary attack commands.
    #[serde(default)]
    pub contact_tracks: Vec<crate::vision::ContactTrack>,
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

impl ObservationData {
    /// Whether an own unit already has work queued behind its current order or
    /// is running a looping program.
    pub fn has_queued_program(&self, unit: UnitId) -> bool {
        self.my_queued_units.binary_search(&unit).is_ok()
    }

    /// The active own repair program, if the observation contains one.
    pub fn repair_target(&self, unit: UnitId) -> Option<Target> {
        self.my_repair_targets
            .binary_search_by_key(&unit, |(worker, _)| *worker)
            .ok()
            .map(|index| self.my_repair_targets[index].1)
    }

    /// Exact progress for one own building's front training item. Malformed
    /// alignment or progress outside the front item's legal range yields no
    /// timing evidence.
    pub fn own_queue_progress(&self, index: usize) -> Option<u32> {
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
                obs.my_units.push(own_unit(state, u));
                obs.observe_own_cargo(u);
                if let Some(target) = own_repair_target(&u.order) {
                    obs.my_repair_targets.push((u.id, target));
                }
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
                    provisional: b.provisional,
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
                    provisional: b.provisional,
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
            if tile.terrain == crate::map::Terrain::Pit {
                obs.known_pits.push(pos);
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
                obs.my_units.push(own_unit(state, u));
                obs.observe_own_cargo(u);
                if let Some(target) = own_repair_target(&u.order) {
                    obs.my_repair_targets.push((u.id, target));
                }
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
                    provisional: b.provisional,
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
                    provisional: b.provisional,
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
        // Retained mines can be memories even on currently visible ground.
        for ghost in vision.ghosts() {
            let observed = obs.enemy_buildings.iter().any(|b| {
                b.player == ghost.owner
                    && b.kind == ghost.kind
                    && b.anchor == ghost.anchor
                    && b.seen
            });
            if !observed {
                obs.enemy_buildings.push(BuildingObs {
                    // Ghosts carry no live id contract; the anchor is the
                    // stable handle. Id 0 would collide with a real
                    // building, so use the sentinel ceiling.
                    id: BuildingId(u32::MAX),
                    provisional: false,
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
            obs.visible.extend_from_slice(visible);
            obs.explored.extend_from_slice(explored);
            for ((column, tile), x) in tiles.iter().enumerate().zip(0..) {
                let pos = TilePos::new(x, y);
                let seen = visible[column];
                let known = explored[column];
                let amount = if seen { tile.scrap } else { scrap_mem[column] };
                if amount > 0 {
                    obs.known_scrap.push((pos, amount));
                }
                let wreck = if seen { tile.wreck } else { wreck_mem[column] };
                if wreck > 0 {
                    obs.known_wrecks.push((pos, wreck));
                }
                if known {
                    if tile.terrain.blocks_ground() {
                        obs.known_rock.push(pos);
                    }
                    if tile.terrain == crate::map::Terrain::Pit {
                        obs.known_pits.push(pos);
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
            if (0..2).any(|dy| (0..2).any(|dx| vision.explored(frame.offset(dx, dy)))) {
                obs.known_frames.push(*frame);
            }
        }
        // Blips ride through untouched: tiles only, by construction.
        obs.blips = vision.contacts().to_vec();
        obs.contact_tracks = vision.tracks().to_vec();
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
            my_carried_units: Vec::new(),
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            my_queue_progress: Vec::new(),
            my_queued_units: Vec::new(),
            ally_units: Vec::new(),
            my_repair_targets: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: Vec::new(),
            explored: Vec::new(),
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_pits: Vec::new(),
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
            contact_tracks: Vec::new(),
            faction: state.player(me).faction,
            my_shells: 0,
            incoming_shells: Vec::new(),
        }
    }
}

impl ObservationData {
    fn observe_own_cargo(&mut self, carrier: &crate::state::Unit) {
        let start = self.my_carried_units.len();
        self.my_carried_units
            .extend(carrier.cargo.iter().map(|rider| CarriedUnitObs {
                carrier: carrier.id,
                id: rider.id,
                kind: rider.kind,
                hp: rider.hp,
            }));
        self.my_carried_units[start..].sort_unstable_by_key(|rider| rider.id);
    }
}

fn own_repair_target(order: &Order) -> Option<Target> {
    match order {
        Order::Repair { building } => Some(Target::Building(*building)),
        Order::RepairUnit { unit } => Some(Target::Unit(*unit)),
        _ => None,
    }
}

fn own_unit(state: &State, u: &crate::state::Unit) -> UnitObs {
    let site = match u.order {
        Order::Build { site } => Some(site),
        Order::Found { kind, anchor } => state
            .buildings()
            .iter()
            .find(|b| b.player == u.player && b.kind == kind && b.anchor == anchor && !b.built)
            .map(|b| b.id),
        _ => None,
    };
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
        site,
        salvaging: match u.order {
            Order::Salvage { building } => Some(building),
            _ => None,
        },
        founding: match u.order {
            Order::Found { kind, anchor } if site.is_none() => Some((kind, anchor)),
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
        provisional: b.provisional,
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
mod tests;
