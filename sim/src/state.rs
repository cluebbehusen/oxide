//! The complete game state and its invariants.
//!
//! Everything that affects game outcomes lives in [`State`] and is
//! serializable; [`State::hash`] fingerprints it canonically. Anything not
//! in here (camera, selection, interpolation) is presentation and belongs to
//! the shell.
//!
//! Invariants:
//! - `units` and `buildings` stay sorted by id (ids are assigned
//!   monotonically and entities are only ever appended or `retain`ed).
//! - A dead entity (hp 0) survives at most until the cleanup phase of the
//!   tick that killed it.
//! - `result` is set at most once; once set, ticks are frozen no-ops.

use crate::ids::{BuildingId, PlayerId, Target, UnitId};
use crate::map::{MAX_MAP_EDGE, Map};
use crate::stats::{BuildingKind, UnitKind};
use chassis::Tick;
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;
use chassis::rng::Pcg32;
use serde::{Deserialize, Serialize};

/// A seat's allegiance: which roster it runs, and its sprite tint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Faction {
    /// Rust-orange machines.
    Ferrous,
    /// Patina-teal machines.
    Cupric,
}

/// A participant in the match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    /// Display name.
    pub name: String,
    /// Which roster this seat runs (and its sprite tint).
    pub faction: Faction,
    /// Team index: seats sharing one share vision, never fight each
    /// other, and stand or fall together.
    pub team: u8,
    /// Scrap in the bank.
    pub scrap: u32,
    /// Whether this seat conceded ([`crate::Command::Surrender`]): its
    /// Foundries no longer keep its team in the match and its commands
    /// reject, while its machines play out their brains as remnants.
    /// Defaulted so records that predate the field deserialize.
    #[serde(default)]
    pub resigned: bool,
}

/// How the match ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GameResult {
    /// One team still holds a Foundry.
    Victory {
        /// The surviving team (see [`crate::State::winners`] for its
        /// seats).
        team: u8,
    },
    /// Every team's last Foundry died on the same tick.
    Draw,
}

/// A unit's current intent. The brain phase turns intent into paths,
/// attacks, and extraction; commands only ever set intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "order", rename_all = "snake_case")]
pub enum Order {
    /// Stand around. Combat units auto-acquire targets in aggro range.
    Idle,
    /// Walk to a tile, then go idle.
    Move {
        /// Destination (always passable — commands snap it).
        goal: TilePos,
    },
    /// Mine a node, hauling to the nearest Foundry, until it and its
    /// neighborhood are exhausted.
    Harvest {
        /// The node tile being worked.
        node: TilePos,
    },
    /// Chase and attack one target until it is gone.
    Attack {
        /// The victim.
        target: crate::ids::Target,
        /// Where to resume attack-moving once the victim is gone. `None`
        /// for a plain attack order (absent in old replays, hence the
        /// default).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume: Option<TilePos>,
    },
    /// Walk to an unfinished own site and stand it up (harvesters only).
    Build {
        /// The site under construction.
        site: crate::ids::BuildingId,
    },
    /// Walk adjacent to a damaged own built building and weld it back
    /// toward full (harvesters only; billed per hp welded).
    Repair {
        /// The patient.
        building: crate::ids::BuildingId,
    },
    /// March to a tile, engaging anything encountered on the way — the
    /// stance for actually fighting, as opposed to [`Order::Move`]'s
    /// oblivious walk.
    AttackMove {
        /// Destination (always passable — commands snap it).
        goal: TilePos,
    },
    /// Walk adjacent to an own built building and strip it down for a
    /// partial refund (harvesters only; Foundries refuse). Drains
    /// buffer like damage and resolve after it — fire wins ties, and
    /// fire-forfeited hp refunds nothing.
    Salvage {
        /// The building coming down.
        building: crate::ids::BuildingId,
    },
    /// Walk to remembered ground and claim it on arrival: the deferred
    /// half of a fog-legal build ([`crate::Command::Build`] with
    /// `defer`). Nothing is placed or paid until the founder stands
    /// beside the footprint and re-proves the *strict* placement
    /// predicate on ground it now sees — taken ground stalls the
    /// program instead of leaking what fog hid.
    Found {
        /// What to construct on arrival.
        kind: crate::stats::BuildingKind,
        /// Top-left tile of the claimed footprint.
        anchor: TilePos,
    },
    /// Chase a wounded own ground unit and weld it back toward full
    /// (harvesters only; billed per hp against the patient's cost).
    /// The weld ticks only while welder and patient both stand still
    /// within [`crate::stats::REPAIR_REACH`].
    RepairUnit {
        /// The patient.
        unit: crate::ids::UnitId,
    },
    /// Move to a tile without chasing or stopping, taking only
    /// primary-weapon shots that are already in range and visible.
    /// (Last variant by appending discipline: earlier discriminants
    /// keep their serialized bytes.)
    Advance {
        /// Destination (always passable — commands snap it).
        goal: TilePos,
    },
}

/// A self-acquired fight's tether: where the machine stood when it
/// picked the fight itself, and how much chase it has left. Only idle
/// auto-acquisition and retaliation ever set one — an explicit player
/// attack is a commitment and carries no leash — and any new command
/// clears it. The tether binds *locomotion*, not the trigger: chasing
/// spends patience and respects the radius; standing in range and
/// firing costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Leash {
    /// The station to walk back to when the fight ends or the tether
    /// runs out.
    pub anchor: TilePos,
    /// The warm-blood window: chase ticks the guard may spend BEYOND
    /// the radius, granted only by a joined fight — refreshed to
    /// [`crate::stats::LEASH_PATIENCE`] every time the guard reaches
    /// its firing stance or answers a hit, spent only while chasing
    /// past the radius. A bait that never comes in reach never grants
    /// any: its chaser breaks at the radius line exactly. Inside the
    /// radius the guard fights freely — that ground is its zone.
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub patience: u16,
    /// Ticks left standing at the post before the guard looks for the
    /// next fight; the leash clears when it reaches zero. Nonzero only
    /// while idle — the answer to an enemy dancing at the aggro edge.
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub cooldown: u16,
}

fn is_zero_u16(v: &u16) -> bool {
    *v == 0
}

/// An in-progress walk along an A* path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathFollow {
    /// Final tile, used to detect stale paths when intent changes.
    pub goal: TilePos,
    /// Remaining waypoints from A* (start tile excluded).
    pub waypoints: Vec<TilePos>,
    /// Index of the waypoint currently steered toward.
    pub next: u32,
}

/// A mobile entity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unit {
    /// Stable id; `units` is sorted by it.
    pub id: UnitId,
    /// Owner.
    pub player: PlayerId,
    /// What kind of machine this is.
    pub kind: UnitKind,
    /// World position (tile units).
    pub pos: Vec2Fx,
    /// Current hit points.
    pub hp: u32,
    /// Scrap on board (harvesters only).
    pub carrying: u32,
    /// Ticks until each weapon may fire again, indexed like
    /// `kind.stats().weapons` (unused slots stay zero).
    pub cooldowns: [u32; crate::stats::MAX_WEAPONS],
    /// Order-specific counter (extraction progress).
    pub progress: u32,
    /// Current intent.
    pub order: Order,
    /// Orders waiting behind the active one; completing the active order
    /// pops the front. With [`Unit::looping`] set, the finished order
    /// rotates to the back instead — that cycle is a patrol.
    #[serde(default, skip_serializing_if = "std::collections::VecDeque::is_empty")]
    pub queue: std::collections::VecDeque<Order>,
    /// Whether the queue cycles (patrol) instead of draining.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub looping: bool,
    /// Current walk, if any.
    pub path: Option<PathFollow>,
    /// The tether of a self-acquired fight, if one is live (absent in
    /// old replays, hence the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leash: Option<Leash>,
    /// Ticks spent standing idle with nothing to fight — a unit is a
    /// STATIONED guard (its acquisitions tether) only past
    /// [`crate::stats::LEASH_STATION_TICKS`]. A unit cycling through
    /// idle mid-battle re-acquires unleashed, which is what keeps the
    /// tether from deciding army fights; leashing every idle machine
    /// once collapsed the scripted tier ladder to a seat-parity coin.
    #[serde(default, skip_serializing_if = "is_zero_u16")]
    pub settled: u16,
}

impl Unit {
    /// The tile this unit currently occupies.
    pub fn tile(&self) -> TilePos {
        TilePos::containing(self.pos)
    }

    /// Ends the active order cleanly: a looping program rotates it to the
    /// back (patrol), a plain queue drains, an empty queue idles.
    pub(crate) fn advance_queue(&mut self) {
        let finished = std::mem::replace(&mut self.order, Order::Idle);
        if self.looping {
            self.queue.push_back(finished);
        }
        match self.queue.pop_front() {
            Some(next) => self.order = next,
            None => self.looping = false,
        }
        self.path = None;
        self.progress = 0;
    }

    /// Abandons the whole program: a stalled or overridden order never
    /// half-continues its queue.
    pub(crate) fn clear_program(&mut self) {
        self.order = Order::Idle;
        self.queue.clear();
        self.looping = false;
        self.path = None;
        self.progress = 0;
    }
}

/// A static entity occupying a rectangle of tiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Building {
    /// Stable id; `buildings` is sorted by it.
    pub id: BuildingId,
    /// Owner.
    pub player: PlayerId,
    /// What kind of building.
    pub kind: BuildingKind,
    /// Top-left tile of the footprint.
    pub anchor: TilePos,
    /// Current hit points.
    pub hp: u32,
    /// Units waiting to be produced, front first.
    pub queue: std::collections::VecDeque<UnitKind>,
    /// Ticks of progress on `queue[0]`.
    pub progress: u32,
    /// Where finished units report: harvesters mine a rallied scrap node,
    /// combat units attack-move there, everyone else walks. `None` means
    /// stand at the doorstep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rally: Option<TilePos>,
    /// Whether construction has finished. Sites (`false`) block ground and
    /// take damage but don't see, fight, or produce.
    #[serde(
        default = "default_true",
        skip_serializing_if = "core::clone::Clone::clone"
    )]
    pub built: bool,
    /// Ticks until this building may fire again (turrets).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub cooldown: u32,
    /// Total hp drained from this building by salvage work — the
    /// cumulative ledger refund crediting reads, so truncation never
    /// drifts across intervals. (Skipped at zero: a building never
    /// salvaged serializes exactly as it did before the field existed.)
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub salvage_drained: u32,
    /// Scrap already credited against `salvage_drained`'s target.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub salvage_credited: u32,
    /// Set when salvage — not fire — took the last hp: cleanup removes
    /// the building without wreck or a destruction event.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub salvaged: bool,
}

fn default_true() -> bool {
    true
}

fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

impl Building {
    /// Iterates the footprint tiles row-major.
    pub fn tiles(&self) -> impl Iterator<Item = TilePos> + use<> {
        let (w, h) = self.kind.stats().size;
        let anchor = self.anchor;
        (0..h).flat_map(move |dy| (0..w).map(move |dx| anchor.offset(dx, dy)))
    }

    /// Whether `pos` lies inside the footprint.
    pub fn contains(&self, pos: TilePos) -> bool {
        let (w, h) = self.kind.stats().size;
        pos.x >= self.anchor.x
            && pos.y >= self.anchor.y
            && pos.x < self.anchor.x + w
            && pos.y < self.anchor.y + h
    }

    /// Center of the footprint in world coordinates.
    pub fn center(&self) -> Vec2Fx {
        let (w, h) = self.kind.stats().size;
        let far = self.anchor.offset(w - 1, h - 1);
        (self.anchor.center() + far.center()) * chassis::fx::HALF
    }

    /// The point of the footprint rectangle closest to `from` — what range
    /// checks measure against, so big buildings don't get phantom reach.
    pub fn closest_point_to(&self, from: Vec2Fx) -> Vec2Fx {
        let (w, h) = self.kind.stats().size;
        let min = self.anchor.center() - Vec2Fx::new(chassis::fx::HALF, chassis::fx::HALF);
        let max = min + Vec2Fx::new(chassis::fx::Fx::from_num(w), chassis::fx::Fx::from_num(h));
        Vec2Fx::new(from.x.clamp(min.x, max.x), from.y.clamp(min.y, max.y))
    }
}

/// The whole world. See module docs for invariants.
///
/// Every field is crate-private: the only way anything outside the sim can
/// change a `State` is [`State::tick`] with tick-stamped commands. That is
/// the architecture's core promise, and here the compiler enforces it
/// rather than a comment. Read access goes through the accessor methods
/// below.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct State {
    pub(crate) tick: Tick,
    pub(crate) rng: Pcg32,
    pub(crate) map: Map,
    pub(crate) players: Vec<Player>,
    pub(crate) vision: Vec<crate::vision::Vision>,
    pub(crate) units: Vec<Unit>,
    pub(crate) buildings: Vec<Building>,
    pub(crate) shells: Vec<Shell>,
    pub(crate) result: Option<GameResult>,
    next_unit_id: u32,
    next_building_id: u32,
}

impl State {
    /// Assembles a state from parts; [`crate::Scenario::build`] is the public
    /// entry point.
    pub(crate) fn assemble(map: Map, players: Vec<Player>, seed: u64) -> Self {
        let vision = players
            .iter()
            .map(|_| crate::vision::Vision::new(map.width(), map.height()))
            .collect();
        Self {
            tick: 0,
            rng: Pcg32::new(seed, 0),
            map,
            players,
            vision,
            units: Vec::new(),
            buildings: Vec::new(),
            shells: Vec::new(),
            result: None,
            next_unit_id: 0,
            next_building_id: 0,
        }
    }

    /// Canonical fingerprint of the entire state. Two states with equal
    /// hashes evolved from the same inputs are the same state.
    pub fn hash(&self) -> u64 {
        chassis::hash::state_hash(self)
    }

    /// Ticks elapsed since scenario start. (The mutating step is
    /// [`State::tick`]; this is the counter it advances.)
    pub fn current_tick(&self) -> Tick {
        self.tick
    }

    /// Whether an active seat can currently receive recurring automatic
    /// scrap that it can spend. This includes a completed Reclaimer, the
    /// late Foundry baseline once its start boundary is reached, and the
    /// faster Foundry recovery for a stranded harvest line.
    ///
    /// A resigned seat or one without a completed Foundry cannot turn
    /// autonomous remnant income into a recovered economy, so neither is
    /// reported as active here.
    pub fn recovery_income_active(&self, player: PlayerId) -> bool {
        if self.player(player).resigned
            || !self.buildings.iter().any(|building| {
                building.player == player
                    && building.hp > 0
                    && building.built
                    && building.kind == BuildingKind::Foundry
            })
        {
            return false;
        }

        let reclaimer = self.buildings.iter().any(|building| {
            building.player == player
                && building.hp > 0
                && building.built
                && building.kind == BuildingKind::Reclaimer
        });
        let baseline = self.tick.saturating_add(1) >= crate::stats::FOUNDRY_BASELINE_START_TICK;
        let emergency = self.player(player).scrap < UnitKind::Harvester.stats().cost
            && self.harvester_recovery_needed(player);
        reclaimer || baseline || emergency
    }

    /// Whether a living Foundry owns neither a live Harvester nor a prepaid
    /// one in a live production queue.
    pub(crate) fn harvester_recovery_needed(&self, player: PlayerId) -> bool {
        !self.player(player).resigned
            && self.buildings.iter().any(|building| {
                building.player == player
                    && building.hp > 0
                    && building.built
                    && building.kind == BuildingKind::Foundry
            })
            && !self.units.iter().any(|unit| {
                unit.player == player && unit.hp > 0 && unit.kind == UnitKind::Harvester
            })
            && !self.buildings.iter().any(|building| {
                building.player == player
                    && building.hp > 0
                    && building.built
                    && building
                        .queue
                        .iter()
                        .any(|kind| *kind == UnitKind::Harvester)
            })
    }

    /// Terrain and scrap.
    pub fn map(&self) -> &Map {
        &self.map
    }

    /// All players, indexed by [`PlayerId`].
    pub fn players(&self) -> &[Player] {
        &self.players
    }

    /// All living units, sorted by id.
    pub fn units(&self) -> &[Unit] {
        &self.units
    }

    /// All standing buildings, sorted by id.
    pub fn buildings(&self) -> &[Building] {
        &self.buildings
    }

    /// The match outcome, once decided.
    pub fn result(&self) -> Option<GameResult> {
        self.result
    }

    /// The player behind `id`. Panics on a foreign id — player ids come from
    /// scenario setup and never dangle.
    pub fn player(&self, id: PlayerId) -> &Player {
        &self.players[id.0 as usize]
    }

    /// Fallible sibling of [`State::player`], for callers holding ids from
    /// outside the sim (protocol traffic, tooling).
    pub fn try_player(&self, id: PlayerId) -> Option<&Player> {
        self.players.get(id.0 as usize)
    }

    /// A player's fog-of-war view. Panics on a foreign id, like
    /// [`State::player`].
    pub fn vision(&self, id: PlayerId) -> &crate::vision::Vision {
        &self.vision[id.0 as usize]
    }

    /// Fallible sibling of [`State::vision`].
    pub fn try_vision(&self, id: PlayerId) -> Option<&crate::vision::Vision> {
        self.vision.get(id.0 as usize)
    }

    /// Checks every structural invariant field-level deserialization alone
    /// cannot. This is the sim's trust boundary: [`State`]'s `Deserialize`
    /// impl calls it, there is no unvalidated constructor, and everything
    /// downstream — the tick pipeline, the renderers, the gym's feature
    /// builder — is entitled to assume the whole checklist below. In
    /// particular the coordinate envelope is what *licenses* the sim's
    /// unchecked tile arithmetic ([`TilePos::offset`],
    /// [`Building::contains`], the neighborhood scans): a coordinate that
    /// got through here cannot overflow them.
    ///
    /// The checklist, in the order it runs:
    /// - Players and result: a non-empty table, addressable by
    ///   [`PlayerId`], with team indices inside it and any victory naming
    ///   a team a player actually carries.
    /// - Map: consistent grid dimensions, within [`MAX_MAP_EDGE`].
    /// - Per-player tables: one vision per seat, its grids sized to the
    ///   map.
    /// - Entity lists: strictly sorted by id, both id counters ahead of
    ///   every live id.
    /// - Units: owner in the table, hp inside `(0, max_hp]`, meters and
    ///   per-weapon cooldowns bounded, queue within
    ///   [`crate::stats::ORDER_QUEUE_CAP`], every coordinate inside the
    ///   envelope, every entity named by an order actually minted.
    /// - Buildings: the same, plus a queue this kind can produce for this
    ///   seat's faction, a coherent salvage ledger, and no live salvage
    ///   marker.
    /// - Shells: coordinates inside the envelope, shooter minted.
    /// - Vision: ghost owners in the table and hostile to the viewer,
    ///   ghosts and contacts in canonical order, all of it inside the
    ///   envelope.
    ///
    /// Two rules are deliberately *permissive*, because tighter ones would
    /// refuse legitimately reachable states. References are checked
    /// against the id counters, never against the live tables: an order
    /// or a shell outliving its subject by a tick is ordinary (brains
    /// re-validate, and a shell's shooter may be dead by impact), while an
    /// id the run never minted is forgery. And the envelopes are
    /// generous sanity boxes rather than map-relative bounds: the bug
    /// being killed is the overflow class, not nonsense geometry, and a
    /// body the separation phase shoved a fraction of a tile past the
    /// border is a state the sim really does produce.
    ///
    /// Every field added to [`State`] or its nested types owes a row here
    /// and a fixture in `sim/tests/state_integrity.rs`.
    ///
    /// Public for tooling that wants to re-check a state it mutated by
    /// hand; the sim itself never calls it inside [`State::tick`].
    pub fn validate_invariants(&self) -> Result<(), StateIntegrityError> {
        use StateIntegrityError as E;

        if self.players.is_empty() {
            return Err(E::NoPlayers);
        }
        // PlayerId is a u8: past 256 seats the ids alias and winners()
        // would name the wrong ones.
        if self.players.len() > usize::from(u8::MAX) + 1 {
            return Err(E::TooManyPlayers);
        }
        let players = self.players.len();
        // Teams normalize to dense ids at scenario build, so a team index
        // is always a seat index too.
        if let Some(i) = self
            .players
            .iter()
            .position(|p| usize::from(p.team) >= players)
        {
            return Err(E::ForeignTeam(PlayerId(i as u8)));
        }
        if let Some(GameResult::Victory { team }) = self.result
            && !self.players.iter().any(|player| player.team == team)
        {
            return Err(E::UnknownVictoryTeam(team));
        }

        // Nested grids: derived Deserialize accepts any cell count, and a
        // short one panics deep inside vision refresh instead of here.
        if !self.map.is_consistent() {
            return Err(E::MalformedMapGrid);
        }
        let (w, h) = (self.map.width(), self.map.height());
        // The parse-time bound, re-applied: the neighborhood scans add
        // unchecked radii to the map dimensions.
        if w > MAX_MAP_EDGE as i32 || h > MAX_MAP_EDGE as i32 {
            return Err(E::MapTooLarge {
                width: w,
                height: h,
            });
        }
        if self.vision.len() != players {
            return Err(E::VisionTableMismatch);
        }
        if self.vision.iter().any(|v| !v.is_consistent(w, h)) {
            return Err(E::MalformedVisionGrid);
        }

        if !self.units.windows(2).all(|a| a[0].id < a[1].id) {
            return Err(E::UnsortedUnits);
        }
        if !self.buildings.windows(2).all(|a| a[0].id < a[1].id) {
            return Err(E::UnsortedBuildings);
        }
        if let Some(u) = self.units.last()
            && u.id.0 >= self.next_unit_id
        {
            return Err(E::StaleUnitCounter);
        }
        if let Some(b) = self.buildings.last()
            && b.id.0 >= self.next_building_id
        {
            return Err(E::StaleBuildingCounter);
        }
        // The counters and the clock increment unchecked in the tick
        // pipeline; a forged extreme is a next-step panic (debug) or a
        // wrap that aliases live ids (release).
        if self.tick > TICK_ENVELOPE {
            return Err(E::TickBeyondEnvelope);
        }
        if self.next_unit_id > ID_COUNTER_ENVELOPE || self.next_building_id > ID_COUNTER_ENVELOPE {
            return Err(E::IdCounterBeyondEnvelope);
        }

        for u in &self.units {
            if usize::from(u.player.0) >= players {
                return Err(E::ForeignUnitOwner(u.id));
            }
            let stats = u.kind.stats();
            if u.hp == 0 || u.hp > stats.max_hp {
                return Err(E::UnitHpOutOfRange(u.id));
            }
            if u.progress > PROGRESS_ENVELOPE {
                return Err(E::UnitProgressOutOfRange(u.id));
            }
            // Slot i belongs to weapon i; slots past the roster stay zero
            // for the machine's whole life.
            if u.cooldowns.iter().enumerate().any(|(i, cd)| {
                *cd > stats
                    .weapons
                    .get(i)
                    .map_or(0, |weapon| weapon.cooldown_ticks)
            }) {
                return Err(E::UnitCooldownOutOfRange(u.id));
            }
            if u.queue.len() > crate::stats::ORDER_QUEUE_CAP {
                return Err(E::OverlongUnitQueue(u.id));
            }
            if !unit_inside_envelope(u) {
                return Err(E::UnitOutsideEnvelope(u.id));
            }
            for target in std::iter::once(&u.order)
                .chain(&u.queue)
                .filter_map(order_reference)
            {
                if !self.minted(target) {
                    return Err(E::UnmintedOrderTarget(u.id));
                }
            }
        }

        for b in &self.buildings {
            if usize::from(b.player.0) >= players {
                return Err(E::ForeignBuildingOwner(b.id));
            }
            let stats = b.kind.stats();
            if !b.built && stats.construction.is_none() {
                return Err(E::UnconstructibleSite(b.id));
            }
            if b.hp == 0 || b.hp > stats.max_hp {
                return Err(E::BuildingHpOutOfRange(b.id));
            }
            if b.progress > PROGRESS_ENVELOPE {
                return Err(E::BuildingProgressOutOfRange(b.id));
            }
            // A building fires its first weapon and nothing else.
            if b.cooldown
                > stats
                    .weapons
                    .first()
                    .map_or(0, |weapon| weapon.cooldown_ticks)
            {
                return Err(E::BuildingCooldownOutOfRange(b.id));
            }
            if b.queue.len() > crate::stats::QUEUE_CAP {
                return Err(E::OverlongBuildingQueue(b.id));
            }
            let faction = self.players[usize::from(b.player.0)].faction;
            if b.queue.iter().any(|kind| {
                !stats.produces.contains(kind) || kind.faction().is_some_and(|f| f != faction)
            }) {
                return Err(E::UnproducibleQueueEntry(b.id));
            }
            // The footprint needs no separate check: sizes are single
            // digits, so an anchor inside the envelope keeps `anchor + size`
            // inside it too.
            if !tile_inside_envelope(b.anchor) || !b.rally.is_none_or(tile_inside_envelope) {
                return Err(E::BuildingOutsideEnvelope(b.id));
            }
            if !salvage_ledger_coherent(b) {
                return Err(E::IncoherentSalvageLedger(b.id));
            }
            if b.salvaged {
                return Err(E::LiveBuildingMarkedSalvaged(b.id));
            }
        }

        for (i, s) in self.shells.iter().enumerate() {
            // Shells carry a seat too: hostile() indexes the player table
            // on impact, so a foreign owner would panic ticks after
            // acceptance.
            if usize::from(s.player.0) >= players {
                return Err(E::ForeignShellOwner(i));
            }
            if !point_inside_envelope(s.launch)
                || !point_inside_envelope(s.impact)
                || s.splash
                    .is_some_and(|r| r < Fx::ZERO || r > Fx::from_num(COORD_ENVELOPE))
            {
                return Err(E::ShellOutsideEnvelope(i));
            }
            if !self.minted(s.shooter) {
                return Err(E::UnmintedShellShooter(i));
            }
        }

        for (i, v) in self.vision.iter().enumerate() {
            let seat = PlayerId(i as u8);
            for ghost in v.ghosts() {
                // Renderers index the player table with this to pick a
                // tint; an owner outside it is a panic, not a wrong color.
                if usize::from(ghost.owner.0) >= players {
                    return Err(E::ForeignGhostOwner(seat));
                }
                if self.players[usize::from(ghost.owner.0)].team == self.players[i].team {
                    return Err(E::FriendlyGhost(seat));
                }
                if !tile_inside_envelope(ghost.anchor) {
                    return Err(E::GhostOutsideEnvelope(seat));
                }
            }
            // The real sort key carries the owner; two seats can hold
            // footprints a memory records under the same corner.
            let ghost_key = |g: &crate::vision::GhostBuilding| (g.anchor.y, g.anchor.x, g.owner);
            if !v
                .ghosts()
                .windows(2)
                .all(|a| ghost_key(&a[0]) <= ghost_key(&a[1]))
            {
                return Err(E::UnsortedGhosts(seat));
            }
            if v.contacts().iter().any(|t| !tile_inside_envelope(*t)) {
                return Err(E::ContactOutsideEnvelope(seat));
            }
            // Blips are sorted and deduplicated every refresh.
            if !v
                .contacts()
                .windows(2)
                .all(|a| (a[0].y, a[0].x) < (a[1].y, a[1].x))
            {
                return Err(E::UnsortedContacts(seat));
            }
        }
        Ok(())
    }

    /// Whether this run ever handed out `target`'s id — the permissive
    /// reference rule. Dangling is fine; unminted is forgery.
    fn minted(&self, target: Target) -> bool {
        match target {
            Target::Unit(id) => id.0 < self.next_unit_id,
            Target::Building(id) => id.0 < self.next_building_id,
        }
    }

    /// Whether `player` currently sees `pos`.
    pub fn can_see(&self, player: PlayerId, pos: TilePos) -> bool {
        self.vision(player).visible(pos)
    }

    /// Whether two seats are enemies. Every combat, targeting, and
    /// detection decision routes through this — teammates are never
    /// valid victims, and a seat is never hostile to itself.
    pub fn hostile(&self, a: PlayerId, b: PlayerId) -> bool {
        self.players[a.0 as usize].team != self.players[b.0 as usize].team
    }

    /// Shells currently in flight, in launch order.
    pub fn shells(&self) -> &[Shell] {
        &self.shells
    }

    /// The seats on the winning team, in id order — empty until a
    /// victory is declared.
    pub fn winners(&self) -> Vec<PlayerId> {
        match self.result {
            Some(GameResult::Victory { team }) => (0..self.players.len())
                .filter(|&i| self.players[i].team == team)
                .map(|i| PlayerId(i as u8))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Rebuilds every player's visible set; runs each tick and once at
    /// scenario build so tick 0 already has sight.
    pub(crate) fn refresh_vision(&mut self) {
        crate::vision::refresh(self);
    }

    /// Mutable access to a player.
    pub(crate) fn player_mut(&mut self, id: PlayerId) -> &mut Player {
        &mut self.players[id.0 as usize]
    }

    /// Looks up a living unit.
    pub fn unit(&self, id: UnitId) -> Option<&Unit> {
        self.units
            .binary_search_by_key(&id, |u| u.id)
            .ok()
            .map(|i| &self.units[i])
    }

    /// Mutable lookup of a living unit.
    pub(crate) fn unit_mut(&mut self, id: UnitId) -> Option<&mut Unit> {
        self.units
            .binary_search_by_key(&id, |u| u.id)
            .ok()
            .map(|i| &mut self.units[i])
    }

    /// Looks up a standing building.
    pub fn building(&self, id: BuildingId) -> Option<&Building> {
        self.buildings
            .binary_search_by_key(&id, |b| b.id)
            .ok()
            .map(|i| &self.buildings[i])
    }

    /// Mutable lookup of a standing building.
    pub(crate) fn building_mut(&mut self, id: BuildingId) -> Option<&mut Building> {
        self.buildings
            .binary_search_by_key(&id, |b| b.id)
            .ok()
            .map(|i| &mut self.buildings[i])
    }

    /// The building whose footprint covers `pos`, if any.
    pub fn building_at(&self, pos: TilePos) -> Option<&Building> {
        self.buildings.iter().find(|b| b.contains(pos))
    }

    /// Whether a unit may stand on `pos`: ground terrain, no live scrap, no
    /// building. Units never block tiles — overlap is resolved by the
    /// separation phase instead.
    pub fn passable(&self, pos: TilePos) -> bool {
        self.map.terrain_passable(pos) && self.building_at(pos).is_none()
    }

    /// Whether a unit of the given movement domain may stand on `pos`.
    /// Ground units need open terrain and no building; air units need the
    /// map itself minus peaks — rock, scrap, and roofs mean nothing up
    /// there, but a mountain owns its column of sky.
    pub fn passable_for(&self, domain: crate::stats::Domain, pos: TilePos) -> bool {
        match domain {
            crate::stats::Domain::Ground => self.passable(pos),
            crate::stats::Domain::Air => self
                .map
                .tile(pos)
                .is_some_and(|t| t.terrain != crate::map::Terrain::Peak),
        }
    }

    /// Spawns a unit at full health. Position is the caller's problem to
    /// validate.
    pub(crate) fn spawn_unit(&mut self, player: PlayerId, kind: UnitKind, pos: Vec2Fx) -> UnitId {
        let id = UnitId(self.next_unit_id);
        self.next_unit_id += 1;
        self.units.push(Unit {
            id,
            player,
            kind,
            pos,
            hp: kind.stats().max_hp,
            carrying: 0,
            cooldowns: [0; crate::stats::MAX_WEAPONS],
            progress: 0,
            order: Order::Idle,
            queue: std::collections::VecDeque::new(),
            looping: false,
            path: None,
            leash: None,
            settled: 0,
        });
        id
    }

    /// Places a building at full health. Footprint validity is the caller's
    /// problem.
    pub(crate) fn place_building(
        &mut self,
        player: PlayerId,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> BuildingId {
        let id = BuildingId(self.next_building_id);
        self.next_building_id += 1;
        self.buildings.push(Building {
            id,
            player,
            kind,
            anchor,
            hp: kind.stats().max_hp,
            queue: std::collections::VecDeque::new(),
            progress: 0,
            rally: None,
            built: true,
            cooldown: 0,
            salvage_drained: 0,
            salvage_credited: 0,
            salvaged: false,
        });
        id
    }

    /// Claims ground for a construction site: blocks the footprint at once
    /// but starts at a fifth of its hit points, unfinished. Site validity
    /// is checked by [`State::can_place`] at the command layer.
    pub(crate) fn place_site(
        &mut self,
        player: PlayerId,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> BuildingId {
        let id = self.place_building(player, kind, anchor);
        let b = self.building_mut(id).expect("just placed");
        b.built = false;
        b.hp = kind.stats().max_hp / 5;
        id
    }

    /// Undoes a just-placed site completely, id counter included — for
    /// validation paths that must leave no trace on rejection (a rejected
    /// command must not move the state hash).
    pub(crate) fn retract_site(&mut self, id: BuildingId) {
        debug_assert_eq!(
            id.0 + 1,
            self.next_building_id,
            "only the newest site retracts"
        );
        self.buildings.retain(|b| b.id != id);
        self.next_building_id = id.0;
    }

    /// Whether `player` may claim `kind` at `anchor` *this instant*:
    /// every footprint tile currently visible to them, open ground, and
    /// free of buildings and standing units. The real invariant is
    /// narrower than visibility: a placement verdict may only read facts
    /// the issuer knows — static terrain, own memory, own and allied
    /// entities. Requiring current sight is how THIS predicate earns the
    /// right to read live occupancy (`building_at`, the hostile-unit
    /// scan); [`State::place_intent_refusal`] earns it differently, by
    /// answering from memory and re-checking here at arrival. This is
    /// literally [`State::place_refusal`] with the reason thrown away,
    /// and it stays the final word on every actual ground claim —
    /// instant builds, bot builds, and the deferred founder's arrival
    /// all resolve through it.
    pub fn can_place(&self, player: PlayerId, kind: BuildingKind, anchor: TilePos) -> bool {
        self.place_refusal(player, kind, anchor).is_none()
    }

    /// Why a placement is refused, or `None` when it is allowed — the
    /// toast's vocabulary. The first blocking reason in footprint scan
    /// order wins; every check is fog-safe by construction (it reads
    /// only what `player` currently sees, exactly like the predicate).
    pub fn place_refusal(
        &self,
        player: PlayerId,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> Option<PlaceRefusal> {
        if kind.stats().construction.is_none() {
            return Some(PlaceRefusal::NotConstructible);
        }
        let (w, h) = kind.stats().size;
        for dy in 0..h {
            for dx in 0..w {
                let t = anchor.offset(dx, dy);
                if !self.vision(player).visible(t) {
                    return Some(PlaceRefusal::Fog);
                }
                if !self.map.terrain_passable(t) {
                    return Some(PlaceRefusal::Terrain);
                }
                if self.building_at(t).is_some() {
                    return Some(PlaceRefusal::Building);
                }
            }
        }
        // Hostile machines hold their ground — standing on a tile
        // denies it to the enemy's foundations. Friendly machines
        // (allies included) never block: they walk off as the site
        // claims the ground (only a routeless body is dealt to the
        // perimeter instantly). A flyer passing overhead blocks
        // nothing either way.
        let hostile_in_footprint = self.units.iter().any(|u| {
            u.hp > 0
                && self.hostile(player, u.player)
                && u.kind.stats().domain == crate::stats::Domain::Ground
                && {
                    let t = u.tile();
                    t.x >= anchor.x && t.x < anchor.x + w && t.y >= anchor.y && t.y < anchor.y + h
                }
        });
        hostile_in_footprint.then_some(PlaceRefusal::Unit)
    }

    /// Whether `player` may *intend* to build `kind` at `anchor` — the
    /// deferred sibling of [`State::place_refusal`], serving
    /// [`crate::Command::Build`]'s `defer` mode and the shell's ghost
    /// on remembered ground. Per footprint tile: a currently visible
    /// tile takes the strict predicate's live checks verbatim; an
    /// explored-but-unseen tile is judged ONLY on what the issuer
    /// knows — static terrain (immutable after parse), remembered
    /// scrap (conservative: nodes only shrink), remembered enemy
    /// buildings, live own/allied buildings (team-internal facts),
    /// and the issuer's own pending [`Order::Found`] claims; a
    /// never-explored tile refuses as [`PlaceRefusal::Fog`]. Live
    /// hostile units and unremembered enemy buildings on unseen ground
    /// are deliberately unreadable here — two states differing only in
    /// what fog hides return identical verdicts, so the amber ghost
    /// can never be a hidden-enemy detector. The arrival re-check
    /// through the strict predicate is what catches the collisions
    /// memory cannot (an allied scaffold on unseen ground included).
    pub fn place_intent_refusal(
        &self,
        player: PlayerId,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> Option<PlaceRefusal> {
        self.place_intent_refusal_replacing(player, kind, anchor, &[])
    }

    /// The intent verdict for a non-queued build that replaces the programs
    /// of `units`. Claims belonging to live own harvesters in that selection
    /// leave with those programs and therefore do not reserve ground against
    /// the replacement. Claims from every other unit remain blockers.
    pub fn place_intent_refusal_replacing(
        &self,
        player: PlayerId,
        kind: BuildingKind,
        anchor: TilePos,
        units: &[UnitId],
    ) -> Option<PlaceRefusal> {
        if kind.stats().construction.is_none() {
            return Some(PlaceRefusal::NotConstructible);
        }
        let vision = self.vision(player);
        let my_team = self.players[player.0 as usize].team;
        let (w, h) = kind.stats().size;
        let covers = |a: TilePos, size: (i32, i32), t: TilePos| {
            t.x >= a.x && t.x < a.x + size.0 && t.y >= a.y && t.y < a.y + size.1
        };
        for dy in 0..h {
            for dx in 0..w {
                let t = anchor.offset(dx, dy);
                if vision.visible(t) {
                    if !self.map.terrain_passable(t) {
                        return Some(PlaceRefusal::Terrain);
                    }
                    if self.building_at(t).is_some() {
                        return Some(PlaceRefusal::Building);
                    }
                    continue;
                }
                if !vision.explored(t) {
                    return Some(PlaceRefusal::Fog);
                }
                let terrain = self.map.tile(t).map(|tile| tile.terrain);
                if terrain != Some(crate::map::Terrain::Ground) || vision.remembered_scrap(t) > 0 {
                    return Some(PlaceRefusal::Terrain);
                }
                let ghosted = vision
                    .ghosts()
                    .iter()
                    .any(|g| covers(g.anchor, g.kind.stats().size, t));
                let allied_building = self.buildings.iter().any(|b| {
                    self.players[b.player.0 as usize].team == my_team
                        && covers(b.anchor, b.kind.stats().size, t)
                });
                if ghosted || allied_building {
                    return Some(PlaceRefusal::Building);
                }
            }
        }
        // The issuer's own outstanding claims: two deferred founds may
        // not promise the same ground (checked over the whole footprint
        // so a visible/unseen mix cannot slip a double claim through).
        let claimed = self.units.iter().any(|u| {
            u.player == player
                && u.hp > 0
                && !(u.kind.stats().harvest.is_some() && units.contains(&u.id))
                && std::iter::once(&u.order).chain(u.queue.iter()).any(|o| {
                    matches!(o, Order::Found { kind: k, anchor: a }
                    if (0..h).any(|dy| (0..w).any(|dx| {
                        covers(*a, k.stats().size, anchor.offset(dx, dy))
                    })))
                })
        });
        if claimed {
            return Some(PlaceRefusal::Building);
        }
        // Hostile machines deny only ground the issuer can SEE them
        // holding — exactly the strict rule, restricted to visible
        // footprint tiles.
        let hostile_in_sight = self.units.iter().any(|u| {
            u.hp > 0
                && self.hostile(player, u.player)
                && u.kind.stats().domain == crate::stats::Domain::Ground
                && {
                    let t = u.tile();
                    covers(anchor, (w, h), t) && vision.visible(t)
                }
        });
        hostile_in_sight.then_some(PlaceRefusal::Unit)
    }
}

/// How far outside the map a coordinate may sit before a snapshot is
/// refused, in tiles: eight times the largest legal map edge. Deliberately
/// a generous sanity box rather than a map-relative bound — see
/// [`State::validate_invariants`] for why. Every offset the sim adds to a
/// coordinate (footprint sizes, ring scans, vision spans) is smaller than
/// one map edge, so nothing inside this box can overflow the unchecked
/// arithmetic downstream, and the squared distances it feeds stay far
/// inside [`Fx`]'s integer range.
const COORD_ENVELOPE: i32 = 8 * MAX_MAP_EDGE as i32;

/// Ceiling on the tick meters a snapshot may carry ([`Unit::progress`],
/// [`Building::progress`]). Construction, repair, and salvage all price a
/// step as `ramp * (meter + 1) / ramp_ticks` in `u32`; an unbounded meter
/// overflows that product. This bound keeps it inside the type for every
/// shipped building and unit and sits far above any meter a match of
/// playable length reaches. The live weld/salvage meters saturate just
/// short of here (the economy brain's `metered` read), so a torch held
/// on one job for millions of ticks keeps billing at its marginal rate
/// instead of walking the product past `u32`.
pub(crate) const PROGRESS_ENVELOPE: u32 = 1 << 21;

/// Ceiling on a building's cumulative salvage ledger. Repairing a
/// half-stripped building and stripping it again legitimately drains more
/// hp than it ever had, so the ledger has no semantic bound — only this
/// one, which keeps the running total clear of a `u32` wrap.
const SALVAGE_LEDGER_CEILING: u32 = u32::MAX / 2;

/// Ceiling on a snapshot's tick. `State::tick` increments unchecked;
/// a forged `u64::MAX` panics on the very next step in a debug build
/// and wraps in release. Half the type is ~14 trillion years at 20
/// ticks/s — no honest record gets near it.
const TICK_ENVELOPE: u64 = u64::MAX / 2;

/// Ceiling on the id counters. Spawning increments unchecked, and a
/// wrapped counter would alias live ids and break the sorted-id
/// invariant this same validator enforces. Half the type leaves two
/// billion spawns of headroom.
const ID_COUNTER_ENVELOPE: u32 = u32::MAX / 2;

/// Whether a tile coordinate sits inside [`COORD_ENVELOPE`].
fn tile_inside_envelope(t: TilePos) -> bool {
    t.x >= -COORD_ENVELOPE
        && t.x <= COORD_ENVELOPE
        && t.y >= -COORD_ENVELOPE
        && t.y <= COORD_ENVELOPE
}

/// Whether a world position sits inside [`COORD_ENVELOPE`].
fn point_inside_envelope(p: Vec2Fx) -> bool {
    let (lo, hi) = (Fx::from_num(-COORD_ENVELOPE), Fx::from_num(COORD_ENVELOPE));
    p.x >= lo && p.x <= hi && p.y >= lo && p.y <= hi
}

/// Whether every coordinate an order names sits inside the envelope. The
/// match is exhaustive on purpose: a new [`Order`] variant carrying a tile
/// must decide its row here rather than slip through a catch-all.
fn order_inside_envelope(order: &Order) -> bool {
    match order {
        Order::Idle
        | Order::Build { .. }
        | Order::Repair { .. }
        | Order::Salvage { .. }
        | Order::RepairUnit { .. } => true,
        Order::Move { goal } | Order::AttackMove { goal } | Order::Advance { goal } => {
            tile_inside_envelope(*goal)
        }
        Order::Harvest { node } => tile_inside_envelope(*node),
        Order::Attack { resume, .. } => resume.is_none_or(tile_inside_envelope),
        Order::Found { anchor, .. } => tile_inside_envelope(*anchor),
    }
}

/// The entity an order names, if any — exhaustive for the same reason as
/// [`order_inside_envelope`].
fn order_reference(order: &Order) -> Option<Target> {
    match order {
        Order::Idle
        | Order::Move { .. }
        | Order::Harvest { .. }
        | Order::AttackMove { .. }
        | Order::Advance { .. }
        | Order::Found { .. } => None,
        Order::Attack { target, .. } => Some(*target),
        Order::Build { site } => Some(Target::Building(*site)),
        Order::Repair { building } | Order::Salvage { building } => {
            Some(Target::Building(*building))
        }
        Order::RepairUnit { unit } => Some(Target::Unit(*unit)),
    }
}

/// Whether every coordinate a unit carries — body, orders, walk, tether —
/// sits inside the envelope.
fn unit_inside_envelope(u: &Unit) -> bool {
    point_inside_envelope(u.pos)
        && order_inside_envelope(&u.order)
        && u.queue.iter().all(order_inside_envelope)
        && u.leash.is_none_or(|l| tile_inside_envelope(l.anchor))
        && u.path.as_ref().is_none_or(|p| {
            tile_inside_envelope(p.goal) && p.waypoints.iter().copied().all(tile_inside_envelope)
        })
}

/// Whether a building's salvage ledger is a coherent record of the hp
/// stripped from it: crediting reads the cumulative drain through one
/// exact formula, so any other pairing is a forgery — and one that would
/// underflow the `u32` subtraction the next drain performs.
fn salvage_ledger_coherent(b: &Building) -> bool {
    if b.salvage_drained > SALVAGE_LEDGER_CEILING {
        return false;
    }
    let stats = b.kind.stats();
    let basis = stats.construction.map_or(0, |c| c.cost);
    let target =
        u64::from(b.salvage_drained) * u64::from(basis) * crate::stats::SALVAGE_REFUND_PERMILLE
            / (1000 * u64::from(stats.max_hp));
    u64::from(b.salvage_credited) == target
}

/// Why [`State::place_refusal`] said no. Own-state facts only — no
/// variant may ever derive from what fog hides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceRefusal {
    /// The kind is scenario-authored, never player-buildable.
    NotConstructible,
    /// A footprint tile is not currently visible.
    Fog,
    /// A footprint tile is impassable ground.
    Terrain,
    /// A building already holds a footprint tile.
    Building,
    /// A hostile machine holds a footprint tile (friendly machines
    /// make way instead of blocking).
    Unit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chassis::fx::Fx;

    fn tiny_state() -> State {
        let (map, _) = Map::parse(&["....", "....", "....", "...."]).unwrap();
        State::assemble(
            map,
            vec![Player {
                name: "p".into(),
                faction: Faction::Ferrous,
                team: 0,
                scrap: 0,
                resigned: false,
            }],
            7,
        )
    }

    #[test]
    fn unit_lookup_by_id_uses_sorted_order() {
        let mut state = tiny_state();
        let a = state.spawn_unit(
            PlayerId(0),
            UnitKind::Harvester,
            TilePos::new(0, 0).center(),
        );
        let b = state.spawn_unit(PlayerId(0), UnitKind::Sentinel, TilePos::new(1, 1).center());
        assert_eq!(state.unit(a).unwrap().kind, UnitKind::Harvester);
        assert_eq!(state.unit(b).unwrap().kind, UnitKind::Sentinel);
        assert_eq!(state.unit(UnitId(99)), None);
    }

    #[test]
    fn building_blocks_passability() {
        let mut state = tiny_state();
        state.place_building(PlayerId(0), BuildingKind::Foundry, TilePos::new(1, 1));
        assert!(state.passable(TilePos::new(0, 0)));
        for pos in [(1, 1), (2, 1), (1, 2), (2, 2)] {
            assert!(!state.passable(TilePos::new(pos.0, pos.1)));
        }
        assert!(state.passable(TilePos::new(3, 3)));
    }

    #[test]
    fn building_geometry() {
        let mut state = tiny_state();
        let id = state.place_building(PlayerId(0), BuildingKind::Foundry, TilePos::new(1, 1));
        let b = state.building(id).unwrap();
        assert_eq!(b.center(), Vec2Fx::new(Fx::from_num(2), Fx::from_num(2)));
        // A point due west clamps to the footprint's west face.
        let probe = Vec2Fx::new(Fx::ZERO, Fx::from_num(2));
        assert_eq!(
            b.closest_point_to(probe),
            Vec2Fx::new(Fx::from_num(1), Fx::from_num(2))
        );
        assert_eq!(b.tiles().count(), 4);
    }

    #[test]
    fn the_progress_ceiling_keeps_the_construction_ramp_in_u32() {
        // Construction, repair, and salvage all price one tick of work as
        // `ramp * (meter + 1) / ramp_ticks` in u32. The ceiling is only
        // worth anything if that product still fits at the ceiling.
        const KINDS: [BuildingKind; 7] = [
            BuildingKind::Foundry,
            BuildingKind::Turret,
            BuildingKind::Fabricator,
            BuildingKind::FlakTurret,
            BuildingKind::Bastion,
            BuildingKind::Array,
            BuildingKind::Reclaimer,
        ];
        for kind in KINDS {
            let stats = kind.stats();
            let ramp = u64::from(stats.max_hp - stats.max_hp / 5);
            assert!(
                ramp * (u64::from(PROGRESS_ENVELOPE) + 1) <= u64::from(u32::MAX),
                "{}: the ramp math must stay inside u32 at the ceiling",
                kind.name()
            );
        }
        // Unit welds ramp over the full max_hp — same product, same
        // ceiling, same obligation for every machine on the roster.
        const UNIT_KINDS: [UnitKind; 11] = [
            UnitKind::Harvester,
            UnitKind::Sentinel,
            UnitKind::Scuttler,
            UnitKind::Lancer,
            UnitKind::Bombard,
            UnitKind::Flakhound,
            UnitKind::Stinger,
            UnitKind::Buzzard,
            UnitKind::Darter,
            UnitKind::Talon,
            UnitKind::Wisp,
        ];
        for kind in UNIT_KINDS {
            let ramp = u64::from(kind.stats().max_hp);
            assert!(
                ramp * (u64::from(PROGRESS_ENVELOPE) + 1) <= u64::from(u32::MAX),
                "{}: the unit weld ramp must stay inside u32 at the ceiling",
                kind.name()
            );
        }
    }

    #[test]
    fn the_coordinate_envelope_admits_every_legal_map_and_refuses_the_extremes() {
        assert!(tile_inside_envelope(TilePos::new(
            MAX_MAP_EDGE as i32,
            MAX_MAP_EDGE as i32
        )));
        assert!(tile_inside_envelope(TilePos::new(-1, -1)));
        assert!(!tile_inside_envelope(TilePos::new(i32::MAX, 0)));
        assert!(!tile_inside_envelope(TilePos::new(0, i32::MIN)));
        assert!(point_inside_envelope(
            TilePos::new(MAX_MAP_EDGE as i32, 0).center()
        ));
        assert!(!point_inside_envelope(Vec2Fx::new(
            Fx::from_bits(i64::MAX),
            Fx::ZERO
        )));
    }

    #[test]
    fn state_hash_changes_with_content() {
        let mut a = tiny_state();
        let b = tiny_state();
        assert_eq!(a.hash(), b.hash());
        a.spawn_unit(PlayerId(0), UnitKind::Harvester, Vec2Fx::ZERO);
        assert_ne!(a.hash(), b.hash());
    }
}

/// Why a deserialized snapshot was refused. Every variant is a structural
/// contradiction the tick pipeline is entitled to assume away, and every
/// one names the entity that broke it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StateIntegrityError {
    /// The player table is empty.
    #[error("no players")]
    NoPlayers,
    /// The player table is longer than [`PlayerId`] can address.
    #[error("more players than a player id can address")]
    TooManyPlayers,
    /// A player sits on a team index no seat carries.
    #[error("player {0} sits on a team outside the table")]
    ForeignTeam(PlayerId),
    /// A victory names a team no player carries.
    #[error("victory names team {0}, which no player carries")]
    UnknownVictoryTeam(u8),
    /// The map grid's dimensions disagree with its cells.
    #[error("map grid dimensions disagree with its cells")]
    MalformedMapGrid,
    /// The map is larger than the supported maximum per side.
    #[error("map is {width}x{height}; the supported maximum is {MAX_MAP_EDGE} per side")]
    MapTooLarge {
        /// Columns claimed by the grid.
        width: i32,
        /// Rows claimed by the grid.
        height: i32,
    },
    /// The vision table does not match the player list.
    #[error("vision table does not match the player list")]
    VisionTableMismatch,
    /// A vision grid disagrees with the map dimensions.
    #[error("a vision table disagrees with the map dimensions")]
    MalformedVisionGrid,
    /// Units are not strictly sorted by id.
    #[error("units not strictly sorted by id")]
    UnsortedUnits,
    /// Buildings are not strictly sorted by id.
    #[error("buildings not strictly sorted by id")]
    UnsortedBuildings,
    /// The unit id counter sits behind a live unit.
    #[error("unit id counter behind a live unit")]
    StaleUnitCounter,
    /// The building id counter sits behind a live building.
    #[error("building id counter behind a live building")]
    StaleBuildingCounter,
    /// The tick is past the envelope the pipeline's unchecked
    /// increment tolerates.
    #[error("tick beyond the sanity envelope")]
    TickBeyondEnvelope,
    /// An id counter is past the envelope spawning tolerates.
    #[error("an id counter is beyond the sanity envelope")]
    IdCounterBeyondEnvelope,
    /// A unit is owned by a player outside the table.
    #[error("unit {0} is owned by a player outside the table")]
    ForeignUnitOwner(UnitId),
    /// A unit's hit points sit outside `(0, max_hp]`.
    #[error("unit {0} carries hit points its kind cannot hold")]
    UnitHpOutOfRange(UnitId),
    /// A unit's work meter is past the ceiling.
    #[error("unit {0} carries a work meter past the ceiling")]
    UnitProgressOutOfRange(UnitId),
    /// A unit's weapon cooldown is longer than that weapon's period, or a
    /// slot past its roster is armed.
    #[error("unit {0} carries a cooldown no weapon of its kind sets")]
    UnitCooldownOutOfRange(UnitId),
    /// A unit's order queue is longer than [`crate::stats::ORDER_QUEUE_CAP`].
    #[error("unit {0} queues more orders than the cap allows")]
    OverlongUnitQueue(UnitId),
    /// A unit's body, order, walk, or tether names a coordinate outside
    /// the sanity envelope.
    #[error("unit {0} names a coordinate outside the envelope")]
    UnitOutsideEnvelope(UnitId),
    /// A unit's order names an entity id this run never handed out.
    #[error("unit {0} is ordered against an id the run never minted")]
    UnmintedOrderTarget(UnitId),
    /// A building is owned by a player outside the table.
    #[error("building {0} is owned by a player outside the table")]
    ForeignBuildingOwner(BuildingId),
    /// An unfinished building kind has no construction definition.
    #[error("building {0} is unfinished but its kind cannot be constructed")]
    UnconstructibleSite(BuildingId),
    /// A building's hit points sit outside `(0, max_hp]`.
    #[error("building {0} carries hit points its kind cannot hold")]
    BuildingHpOutOfRange(BuildingId),
    /// A building's progress meter is past the ceiling.
    #[error("building {0} carries a progress meter past the ceiling")]
    BuildingProgressOutOfRange(BuildingId),
    /// A building's cooldown is longer than its weapon's period.
    #[error("building {0} carries a cooldown its weapon never sets")]
    BuildingCooldownOutOfRange(BuildingId),
    /// A building's production queue is longer than
    /// [`crate::stats::QUEUE_CAP`].
    #[error("building {0} queues more units than the cap allows")]
    OverlongBuildingQueue(BuildingId),
    /// A building queues a unit its kind cannot train, or one belonging to
    /// the other faction's roster.
    #[error("building {0} queues a unit it could never train")]
    UnproducibleQueueEntry(BuildingId),
    /// A building's anchor or rally point sits outside the sanity
    /// envelope.
    #[error("building {0} names a coordinate outside the envelope")]
    BuildingOutsideEnvelope(BuildingId),
    /// A building's salvage ledger is not a coherent record of the hp
    /// stripped from it.
    #[error("building {0} carries an incoherent salvage ledger")]
    IncoherentSalvageLedger(BuildingId),
    /// A live building carries the transient marker cleanup uses to
    /// distinguish a completed salvage from combat destruction.
    #[error("building {0} is still live but marked salvaged")]
    LiveBuildingMarkedSalvaged(BuildingId),
    /// A shell in flight is owned by a player outside the table.
    #[error("shell {0} is owned by a player outside the table")]
    ForeignShellOwner(usize),
    /// A shell's launch, impact, or splash radius is outside the sanity
    /// envelope.
    #[error("shell {0} names a coordinate outside the envelope")]
    ShellOutsideEnvelope(usize),
    /// A shell was fired by an entity id this run never handed out.
    #[error("shell {0} was fired by an id the run never minted")]
    UnmintedShellShooter(usize),
    /// A remembered building is owned by a player outside the table.
    #[error("player {0} remembers a building owned outside the table")]
    ForeignGhostOwner(PlayerId),
    /// A player remembers a building belonging to their own team; ghosts
    /// are memories of the enemy.
    #[error("player {0} remembers a building of their own team")]
    FriendlyGhost(PlayerId),
    /// A remembered building's anchor sits outside the sanity envelope.
    #[error("player {0} remembers a building outside the envelope")]
    GhostOutsideEnvelope(PlayerId),
    /// A player's remembered buildings are not in canonical order.
    #[error("player {0} remembers buildings out of canonical order")]
    UnsortedGhosts(PlayerId),
    /// A radar contact sits outside the sanity envelope.
    #[error("player {0} holds a radar contact outside the envelope")]
    ContactOutsideEnvelope(PlayerId),
    /// A player's radar contacts are not sorted and deduplicated.
    #[error("player {0} holds radar contacts out of canonical order")]
    UnsortedContacts(PlayerId),
}

/// A shell in flight: launched at the victim's fire-time position,
/// unguided from that instant ("a shell in flight chooses nothing" —
/// literal since 0.9), resolving on its arrival tick against whatever
/// stands there. Outlives its shooter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shell {
    /// Who fired it (may be dead by impact; retaliation copes).
    pub shooter: crate::ids::Target,
    /// The firing seat.
    pub player: crate::ids::PlayerId,
    /// Where it launched, for presentation.
    pub launch: Vec2Fx,
    /// Where it will land — fixed at fire time.
    pub impact: Vec2Fx,
    /// The tick it resolves on.
    pub arrival: Tick,
    /// Damage on the direct hit.
    pub damage: u32,
    /// Which movement domains the splash covers.
    pub targets: crate::stats::DomainMask,
    /// Splash radius, if the weapon splashes.
    pub splash: Option<chassis::fx::Fx>,
}

/// The wire shape of [`State`]: a private mirror that derives the actual
/// field-level `Deserialize`, so the only path from bytes to a `State`
/// runs through `State::validate_invariants` — there is no public
/// unvalidated constructor to call by accident. The exhaustive `From`
/// below keeps the mirror honest: if `State` grows or loses a field, this
/// module stops compiling instead of silently desyncing.
#[derive(Deserialize)]
#[serde(rename = "State")]
struct StateWire {
    tick: Tick,
    rng: Pcg32,
    map: Map,
    players: Vec<Player>,
    vision: Vec<crate::vision::Vision>,
    units: Vec<Unit>,
    buildings: Vec<Building>,
    shells: Vec<Shell>,
    result: Option<GameResult>,
    next_unit_id: u32,
    next_building_id: u32,
}

impl From<StateWire> for State {
    fn from(w: StateWire) -> Self {
        // Every field named on both sides: drift breaks the build.
        let StateWire {
            tick,
            rng,
            map,
            players,
            vision,
            units,
            buildings,
            shells,
            result,
            next_unit_id,
            next_building_id,
        } = w;
        State {
            tick,
            rng,
            map,
            players,
            vision,
            units,
            buildings,
            shells,
            result,
            next_unit_id,
            next_building_id,
        }
    }
}

impl<'de> Deserialize<'de> for State {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let state: State = StateWire::deserialize(deserializer)?.into();
        state
            .validate_invariants()
            .map_err(serde::de::Error::custom)?;
        Ok(state)
    }
}
