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

/// Cosmetic allegiance — decides sprite tint, nothing else.
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
    /// Sprite tint.
    pub faction: Faction,
    /// Scrap in the bank.
    pub scrap: u32,
}

/// How the match ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GameResult {
    /// One player still has buildings.
    Victory {
        /// The survivor.
        winner: PlayerId,
    },
    /// Everyone's buildings died on the same tick.
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
    /// March to a tile, engaging anything encountered on the way — the
    /// stance for actually fighting, as opposed to [`Order::Move`]'s
    /// oblivious walk.
    AttackMove {
        /// Destination (always passable — commands snap it).
        goal: TilePos,
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
    /// Ticks until the next attack is allowed.
    pub cooldown: u32,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub(crate) tick: Tick,
    pub(crate) rng: Pcg32,
    pub(crate) map: Map,
    pub(crate) players: Vec<Player>,
    pub(crate) vision: Vec<crate::vision::Vision>,
    pub(crate) units: Vec<Unit>,
    pub(crate) buildings: Vec<Building>,
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

    /// A player's fog-of-war view. Panics on a foreign id, like
    /// [`State::player`].
    pub fn vision(&self, id: PlayerId) -> &crate::vision::Vision {
        &self.vision[id.0 as usize]
    }

    /// Whether `player` currently sees `pos`.
    pub fn can_see(&self, player: PlayerId, pos: TilePos) -> bool {
        self.vision(player).visible(pos)
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
            cooldown: 0,
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

    /// Whether `player` may start `kind` at `anchor` right now: every
    /// footprint tile explored by them, open ground, and free of buildings
    /// and standing units. One predicate serves command validation and the
    /// shell's placement preview — they must never disagree.
    pub fn can_place(&self, player: PlayerId, kind: BuildingKind, anchor: TilePos) -> bool {
        let (w, h) = kind.stats().size;
        for dy in 0..h {
            for dx in 0..w {
                let t = anchor.offset(dx, dy);
                if !self.vision(player).explored(t)
                    || !self.map.terrain_passable(t)
                    || self.building_at(t).is_some()
                {
                    return false;
                }
            }
        }
        // Standing machines hold their ground — no foundations under feet.
        !self.units.iter().any(|u| {
            u.hp > 0 && {
                let t = u.tile();
                t.x >= anchor.x && t.x < anchor.x + w && t.y >= anchor.y && t.y < anchor.y + h
            }
        })
    }
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
