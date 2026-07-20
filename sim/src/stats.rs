//! Unit and building kinds, their stats, and global tuning constants.
//!
//! All game balance lives in this one file. Stats are `const` tables —
//! changing a number here changes sim behavior, so expect regression hashes
//! to move and re-bless deliberately (see AGENTS.md).

use chassis::fx::Fx;
use serde::{Deserialize, Serialize};

/// Every trainable unit type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    /// Gathers scrap from nodes and hauls it to a Foundry.
    Harvester,
    /// The line combat unit: short-ranged, sturdy, expendable.
    Sentinel,
    /// Fast, cheap, fragile raider: a contact-range shredder that eats
    /// harvest lines and dies to anything that fights back in time.
    Scuttler,
    /// Slow long-range artillery: outranges everything (including its own
    /// aggro), melts to anything that reaches it.
    Lancer,
}

/// Every building type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildingKind {
    /// HQ, unit factory, and scrap drop-off. Lose all of them, lose the game.
    Foundry,
}

/// Combat parameters for units that can fight.
#[derive(Debug, Clone, Copy)]
pub struct AttackStats {
    /// Hit points removed per hit.
    pub damage: u32,
    /// Maximum engagement distance, in tiles (center to closest point).
    pub range: Fx,
    /// Ticks between hits.
    pub cooldown_ticks: u32,
    /// Idle units acquire targets inside this radius on their own.
    pub aggro_range: Fx,
}

/// Gathering parameters for units that can harvest.
#[derive(Debug, Clone, Copy)]
pub struct HarvestStats {
    /// Scrap carried before a delivery trip is forced.
    pub capacity: u32,
    /// Ticks of standing at a node to extract one scrap.
    pub ticks_per_scrap: u32,
}

/// Static parameters of a unit kind.
#[derive(Debug, Clone, Copy)]
pub struct UnitStats {
    /// Hit points at spawn.
    pub max_hp: u32,
    /// Movement speed in tiles per tick.
    pub speed: Fx,
    /// Collision radius in tiles (separation only; units never hard-block).
    pub radius: Fx,
    /// Scrap price.
    pub cost: u32,
    /// Foundry queue time.
    pub train_ticks: u32,
    /// Present iff the unit can fight.
    pub attack: Option<AttackStats>,
    /// Present iff the unit can gather.
    pub harvest: Option<HarvestStats>,
    /// Fog-of-war reveal radius, in tiles.
    pub vision: i32,
}

/// Static parameters of a building kind.
#[derive(Debug, Clone, Copy)]
pub struct BuildingStats {
    /// Hit points at placement.
    pub max_hp: u32,
    /// Footprint in tiles (width, height), anchored top-left.
    pub size: (i32, i32),
    /// Fog-of-war reveal radius, in tiles (from each footprint tile).
    pub vision: i32,
    /// What this building can train. Empty for non-producers.
    pub produces: &'static [UnitKind],
}

const HARVESTER: UnitStats = UnitStats {
    max_hp: 60,
    speed: Fx::lit("0.125"), // 2.5 tiles/s at 20 tps
    radius: Fx::lit("0.3"),
    cost: 50,
    train_ticks: 100, // 5 s
    attack: None,
    harvest: Some(HarvestStats {
        capacity: 10,
        ticks_per_scrap: 10, // 2 scrap/s while extracting
    }),
    vision: 6,
};

const SENTINEL: UnitStats = UnitStats {
    max_hp: 100,
    speed: Fx::lit("0.11"), // 2.2 tiles/s — armies are slightly outrun by harvesters
    radius: Fx::lit("0.35"),
    cost: 75,
    train_ticks: 160, // 8 s
    attack: Some(AttackStats {
        damage: 10,
        range: Fx::lit("2.5"),
        cooldown_ticks: 20, // 1 hit/s
        aggro_range: Fx::lit("5"),
    }),
    harvest: None,
    vision: 7, // strictly wider than aggro, so acquired targets are seen
};

const SCUTTLER: UnitStats = UnitStats {
    max_hp: 40,
    speed: Fx::lit("0.16"), // 3.2 tiles/s — outruns everything
    radius: Fx::lit("0.28"),
    cost: 40,
    train_ticks: 80, // 4 s
    attack: Some(AttackStats {
        damage: 3,
        range: Fx::lit("0.8"), // practically touching
        cooldown_ticks: 6,     // a gnawing 10 dps
        aggro_range: Fx::lit("5"),
    }),
    harvest: None,
    vision: 6,
};

const LANCER: UnitStats = UnitStats {
    max_hp: 50,
    speed: Fx::lit("0.08"), // 1.6 tiles/s — the army protects it, not vice versa
    radius: Fx::lit("0.35"),
    cost: 110,
    train_ticks: 200, // 10 s
    attack: Some(AttackStats {
        damage: 30,
        range: Fx::lit("5.5"), // beyond aggro: it only uses this on orders
        cooldown_ticks: 60,    // one heavy shot per 3 s
        aggro_range: Fx::lit("5"),
    }),
    harvest: None,
    vision: 7,
};

const FOUNDRY: BuildingStats = BuildingStats {
    max_hp: 800,
    size: (2, 2),
    vision: 8,
    produces: &[UnitKind::Harvester, UnitKind::Sentinel],
};

impl UnitKind {
    /// Static stats for this kind.
    pub const fn stats(self) -> &'static UnitStats {
        match self {
            UnitKind::Harvester => &HARVESTER,
            UnitKind::Sentinel => &SENTINEL,
            UnitKind::Scuttler => &SCUTTLER,
            UnitKind::Lancer => &LANCER,
        }
    }
}

impl BuildingKind {
    /// Static stats for this kind.
    pub const fn stats(self) -> &'static BuildingStats {
        match self {
            BuildingKind::Foundry => &FOUNDRY,
        }
    }
}

/// Scrap contained in a freshly parsed node tile.
pub const SCRAP_NODE_AMOUNT: u32 = 400;

/// Scrap in a rich node (the `S` map legend) — a fought-over prize.
pub const RICH_SCRAP_NODE_AMOUNT: u32 = 800;

/// Maximum queued units per Foundry.
pub const QUEUE_CAP: usize = 5;

/// Maximum orders (and patrol waypoints) queued per unit. Bounds what a
/// hostile append stream can make a unit remember.
pub const ORDER_QUEUE_CAP: usize = 32;

/// A* expansion budget per query — bounds worst-case pathfinding work.
pub const PATH_EXPANSION_CAP: u32 = 20_000;

/// When a harvest node runs dry, harvesters look for a replacement within
/// this Chebyshev radius of the old node.
pub const RETARGET_RADIUS: i32 = 10;

/// When a Move command lands on an impassable tile, the goal snaps to the
/// nearest passable tile within this radius (else the command is rejected).
pub const GOAL_SNAP_RADIUS: i32 = 3;

/// Relaxation passes of collision resolution per tick. More passes settle
/// dense crowds faster; each pass is a full pairwise sweep.
pub const COLLISION_ITERATIONS: u32 = 3;

/// How close to a waypoint counts as "reached" when another waypoint
/// follows (final waypoints are still landed exactly). Kills the
/// push-off/re-seek oscillation that made crowds grind.
pub const WAYPOINT_ACCEPT: Fx = Fx::lit("0.35");

/// Within this range of a shared goal, touching an already-arrived
/// neighbor counts as arriving — crowds settle instead of churning on the
/// click point.
pub const ARRIVAL_NEAR: Fx = Fx::lit("1.5");

/// Collision share taken by an anchored unit (extracting or firing from a
/// hold); the mover takes the rest. Passers-by flow around workers.
pub const ANCHORED_PUSH_SHARE: Fx = Fx::lit("0.1");

/// Furthest one collision pass may displace one unit, in tiles. Keeps
/// packed crowds settling smoothly instead of popping apart.
pub const COLLISION_MAX_STEP: Fx = Fx::lit("0.12");
