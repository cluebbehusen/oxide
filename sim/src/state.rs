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

use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::map::Map;
use crate::stats::{BuildingKind, UnitKind};
use chassis::Tick;
use chassis::fx::Vec2Fx;
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
    /// fire-forfeited hp refunds nothing. (Last variant by appending
    /// discipline: earlier discriminants keep their serialized bytes.)
    Salvage {
        /// The building coming down.
        building: crate::ids::BuildingId,
    },
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

    /// Checks the structural invariants field-level deserialization alone
    /// cannot: sorted id lists, id counters ahead of every live id,
    /// per-player tables sized to the player list, entity owners inside
    /// it, and nested grids that hold together. Called automatically by
    /// [`State`]'s `Deserialize` impl, so a snapshot that parses is a
    /// snapshot that holds together; public for tooling that wants to
    /// re-check after mutating one by hand.
    pub fn validate_invariants(&self) -> Result<(), StateIntegrityError> {
        if self.players.is_empty() {
            return Err(StateIntegrityError::NoPlayers);
        }
        if !self.units.windows(2).all(|w| w[0].id < w[1].id) {
            return Err(StateIntegrityError::UnsortedUnits);
        }
        if !self.buildings.windows(2).all(|w| w[0].id < w[1].id) {
            return Err(StateIntegrityError::UnsortedBuildings);
        }
        if let Some(u) = self.units.last()
            && u.id.0 >= self.next_unit_id
        {
            return Err(StateIntegrityError::StaleUnitCounter);
        }
        if let Some(b) = self.buildings.last()
            && b.id.0 >= self.next_building_id
        {
            return Err(StateIntegrityError::StaleBuildingCounter);
        }
        if self.vision.len() != self.players.len() {
            return Err(StateIntegrityError::VisionTableMismatch);
        }
        let players = self.players.len();
        if self.units.iter().any(|u| (u.player.0 as usize) >= players) {
            return Err(StateIntegrityError::ForeignUnitOwner);
        }
        if self
            .buildings
            .iter()
            .any(|b| (b.player.0 as usize) >= players)
        {
            return Err(StateIntegrityError::ForeignBuildingOwner);
        }
        // Shells carry a seat too: hostile() indexes the player table on
        // impact, so a foreign owner would panic ticks after acceptance.
        if self.shells.iter().any(|s| (s.player.0 as usize) >= players) {
            return Err(StateIntegrityError::ForeignShellOwner);
        }
        // Nested grids: derived Deserialize accepts any cell count, and a
        // short one panics deep inside vision refresh instead of here.
        if !self.map.is_consistent() {
            return Err(StateIntegrityError::MalformedMapGrid);
        }
        let (w, h) = (self.map.width(), self.map.height());
        if self.vision.iter().any(|v| !v.is_consistent(w, h)) {
            return Err(StateIntegrityError::MalformedVisionGrid);
        }
        Ok(())
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

    /// Whether `player` may start `kind` at `anchor` right now: every
    /// footprint tile *currently visible* to them, open ground, and free
    /// of buildings and standing units. Visibility (not mere exploration)
    /// is the fog-honest rule — the occupancy checks read live state,
    /// and a red ghost over explored-but-unseen ground would otherwise
    /// leak hidden enemies. One predicate serves command validation and
    /// the shell's placement preview — they must never disagree, which
    /// is why this is literally [`State::place_refusal`] with the
    /// reason thrown away.
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
        // (allies included) never block: the accept path relocates
        // them to the perimeter as the site claims the ground. A flyer
        // passing overhead blocks nothing either way.
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
    fn state_hash_changes_with_content() {
        let mut a = tiny_state();
        let b = tiny_state();
        assert_eq!(a.hash(), b.hash());
        a.spawn_unit(PlayerId(0), UnitKind::Harvester, Vec2Fx::ZERO);
        assert_ne!(a.hash(), b.hash());
    }
}

/// Why a deserialized snapshot was refused. Every variant is a structural
/// contradiction the tick pipeline is entitled to assume away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StateIntegrityError {
    /// The player table is empty.
    #[error("no players")]
    NoPlayers,
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
    /// The vision table does not match the player list.
    #[error("vision table does not match the player list")]
    VisionTableMismatch,
    /// A unit is owned by a player outside the table.
    #[error("unit owned by a player outside the table")]
    ForeignUnitOwner,
    /// A building is owned by a player outside the table.
    #[error("building owned by a player outside the table")]
    ForeignBuildingOwner,
    /// A shell in flight is owned by a player outside the table.
    #[error("shell owned by a player outside the table")]
    ForeignShellOwner,
    /// The map grid's dimensions disagree with its cells.
    #[error("map grid dimensions disagree with its cells")]
    MalformedMapGrid,
    /// A vision grid disagrees with the map dimensions.
    #[error("a vision table disagrees with the map dimensions")]
    MalformedVisionGrid,
}

/// The wire shape of [`State`]: a private mirror that derives the actual
/// field-level `Deserialize`, so the only path from bytes to a `State`
/// runs through [`State::validate_invariants`] — there is no public
/// unvalidated constructor to call by accident. The exhaustive `From`
/// below keeps the mirror honest: if `State` grows or loses a field, this
/// module stops compiling instead of silently desyncing.
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
