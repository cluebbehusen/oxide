//! Unit and building kinds, their stats, and global tuning constants.
//!
//! All game balance lives in this one file. Stats are `const` tables —
//! changing a number here changes sim behavior, so expect regression hashes
//! to move and re-bless deliberately (see AGENTS.md).
//!
//! Combat is a weapons matrix: every kind carries a (possibly empty) list
//! of weapons, each declaring which movement domains it can hit, whether it
//! splashes, and whether it fires indirect (arcing over terrain cover).
//! Units also carry a movement domain of their own — ground units path and
//! collide on the terrain grid, air units fly straight lines above it.

use crate::state::Faction;
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
    /// Slow long-range artillery: outranges everything it can see,
    /// melts to anything that reaches it.
    Lancer,
    /// Heavy siege piece: arcing splash shells that reach beyond its own
    /// eyes — someone else must hold sight on the target. Shared roster.
    Bombard,
    /// Ferrous anti-air crawler: tanky flak platform, blind to ground.
    Flakhound,
    /// Cupric anti-air crawler: cheap and quick, dies to a stiff breeze.
    Stinger,
    /// Ferrous ground-attack flyer: slow, heavy strikes, no answer to air.
    Buzzard,
    /// Cupric ground-attack flyer: fast shallow strafes, no answer to air.
    Darter,
    /// Ferrous air-superiority flyer: sees far, hits only other flyers.
    Talon,
    /// Cupric air-superiority flyer: a swarm wing — fragile, rapid, cheap.
    Wisp,
}

/// Every building type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildingKind {
    /// HQ, unit factory, and scrap drop-off. Lose all of them, lose the game.
    Foundry,
    /// Static defense: fires on its own at anything in range and line of
    /// sight. Holds ground; loses to patient siege.
    Turret,
    /// Second factory: trains the advanced roster. The tech gate.
    Fabricator,
    /// Anti-air emplacement: flak bursts that only ever look up.
    FlakTurret,
    /// Artillery emplacement: arcing splash shells beyond its own sight —
    /// punishes lazy siege lines, but needs a spotter at full reach.
    Bastion,
    /// Radar: a tall mast of true sight, and a wider ring of blips —
    /// contacts without identity that never satisfy a targeted attack.
    Array,
    /// Grinds ambient debris into a scrap trickle. Slow to repay itself;
    /// the reason a match can outlive its scrap patches.
    Reclaimer,
}

/// A movement medium. Ground units path and collide on the terrain grid;
/// air units fly straight lines, ignore terrain, and collide only with
/// each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// Bound to passable terrain.
    Ground,
    /// Above the grid: rock, buildings, and scrap mean nothing.
    Air,
}

/// Which movement domains a weapon can hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DomainMask {
    /// Can hit ground units and buildings.
    pub ground: bool,
    /// Can hit air units.
    pub air: bool,
}

impl DomainMask {
    /// Hits ground only.
    pub const GROUND: DomainMask = DomainMask {
        ground: true,
        air: false,
    };
    /// Hits air only.
    pub const AIR: DomainMask = DomainMask {
        ground: false,
        air: true,
    };
    /// Hits everything.
    pub const BOTH: DomainMask = DomainMask {
        ground: true,
        air: true,
    };

    /// Whether this mask covers the given domain.
    pub const fn covers(self, domain: Domain) -> bool {
        match domain {
            Domain::Ground => self.ground,
            Domain::Air => self.air,
        }
    }
}

/// One weapon: its numbers and its firing rules.
#[derive(Debug, Clone, Copy)]
pub struct WeaponStats {
    /// Hit points removed per hit.
    pub damage: u32,
    /// Maximum engagement distance, in tiles (center to closest point).
    pub range: Fx,
    /// Ticks between hits.
    pub cooldown_ticks: u32,
    /// Which movement domains this weapon can hit. Buildings count as
    /// ground.
    pub targets: DomainMask,
    /// Area damage radius around the impact point. Splash hits enemy
    /// *units* in the weapon's target domains; buildings only ever take
    /// the direct hit.
    pub splash: Option<Fx>,
    /// Indirect fire arcs over terrain: the line-of-sight trace that lets
    /// rock and buildings block direct shots is skipped.
    pub indirect: bool,
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
    /// Factory queue time.
    pub train_ticks: u32,
    /// The medium this unit moves through.
    pub domain: Domain,
    /// Every weapon this unit carries (empty for pacifists). The first
    /// weapon that can cover an ordered target engages it; weapons that
    /// cannot pick their own nearest hostile in their domain.
    pub weapons: &'static [WeaponStats],
    /// Idle units acquire targets inside this radius on their own. Zero
    /// for units with no weapons.
    pub aggro_range: Fx,
    /// Present iff the unit can gather.
    pub harvest: Option<HarvestStats>,
    /// Fog-of-war reveal radius, in tiles.
    pub vision: i32,
}

impl UnitStats {
    /// Whether this kind carries any weapon at all.
    pub const fn can_fight(&self) -> bool {
        !self.weapons.is_empty()
    }

    /// Whether any weapon covers the given domain.
    pub fn can_target(&self, domain: Domain) -> bool {
        self.weapons.iter().any(|w| w.targets.covers(domain))
    }

    /// The longest reach of any weapon covering the given domain.
    pub fn max_range_vs(&self, domain: Domain) -> Option<Fx> {
        self.weapons
            .iter()
            .filter(|w| w.targets.covers(domain))
            .map(|w| w.range)
            .max()
    }
}

/// Static parameters of a building kind.
#[derive(Debug, Clone, Copy)]
pub struct BuildingStats {
    /// Hit points when fully built.
    pub max_hp: u32,
    /// Footprint in tiles (width, height), anchored top-left.
    pub size: (i32, i32),
    /// Fog-of-war reveal radius, in tiles (from each footprint tile).
    pub vision: i32,
    /// What this building can train. Empty for non-producers.
    pub produces: &'static [UnitKind],
    /// Every weapon this building fires on its own (turrets have no aggro
    /// dial — their reach is their temper). Empty for civilians.
    pub weapons: &'static [WeaponStats],
    /// Present iff harvesters can build it. `None` marks the kinds only
    /// scenarios place (the Foundry — win conditions stay authored).
    pub construction: Option<ConstructionStats>,
}

impl BuildingStats {
    /// Whether this kind fires on its own.
    pub const fn can_fight(&self) -> bool {
        !self.weapons.is_empty()
    }
}

/// Parameters of a buildable kind.
#[derive(Debug, Clone, Copy)]
pub struct ConstructionStats {
    /// Scrap price, deducted when the site is placed. Cancelling refunds
    /// `cost x hp / max_hp` — you salvage what actually got built, and
    /// enemy fire burns the refund.
    pub cost: u32,
    /// Builder-adjacent ticks from site to standing building.
    pub build_ticks: u32,
}

/// A production role: the slot a unit fills in a roster, independent of
/// which faction's variant fills it. Shared kinds map to themselves; the
/// varied slots resolve per faction through [`Role::unit_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The economy unit.
    Harvester,
    /// The line fighter.
    Sentinel,
    /// The raider.
    Scuttler,
    /// Direct-fire siege.
    Lancer,
    /// Indirect heavy siege.
    Bombard,
    /// The dedicated anti-air ground unit.
    AntiAir,
    /// The ground-attack flyer.
    AirGround,
    /// The air-superiority flyer.
    AirAir,
}

impl Role {
    /// The concrete kind filling this role for a faction.
    pub const fn unit_for(self, faction: Faction) -> UnitKind {
        match (self, faction) {
            (Role::Harvester, _) => UnitKind::Harvester,
            (Role::Sentinel, _) => UnitKind::Sentinel,
            (Role::Scuttler, _) => UnitKind::Scuttler,
            (Role::Lancer, _) => UnitKind::Lancer,
            (Role::Bombard, _) => UnitKind::Bombard,
            (Role::AntiAir, Faction::Ferrous) => UnitKind::Flakhound,
            (Role::AntiAir, Faction::Cupric) => UnitKind::Stinger,
            (Role::AirGround, Faction::Ferrous) => UnitKind::Buzzard,
            (Role::AirGround, Faction::Cupric) => UnitKind::Darter,
            (Role::AirAir, Faction::Ferrous) => UnitKind::Talon,
            (Role::AirAir, Faction::Cupric) => UnitKind::Wisp,
        }
    }
}

impl UnitKind {
    /// The faction whose roster carries this kind; `None` means shared.
    /// Training a faction-bound kind from the other faction's seat is
    /// rejected at command validation.
    pub const fn faction(self) -> Option<Faction> {
        match self {
            UnitKind::Harvester
            | UnitKind::Sentinel
            | UnitKind::Scuttler
            | UnitKind::Lancer
            | UnitKind::Bombard => None,
            UnitKind::Flakhound | UnitKind::Buzzard | UnitKind::Talon => Some(Faction::Ferrous),
            UnitKind::Stinger | UnitKind::Darter | UnitKind::Wisp => Some(Faction::Cupric),
        }
    }

    /// Lowercase display name.
    pub const fn name(self) -> &'static str {
        match self {
            UnitKind::Harvester => "harvester",
            UnitKind::Sentinel => "sentinel",
            UnitKind::Scuttler => "scuttler",
            UnitKind::Lancer => "lancer",
            UnitKind::Bombard => "bombard",
            UnitKind::Flakhound => "flakhound",
            UnitKind::Stinger => "stinger",
            UnitKind::Buzzard => "buzzard",
            UnitKind::Darter => "darter",
            UnitKind::Talon => "talon",
            UnitKind::Wisp => "wisp",
        }
    }

    /// The role this kind fills in its roster.
    pub const fn role(self) -> Role {
        match self {
            UnitKind::Harvester => Role::Harvester,
            UnitKind::Sentinel => Role::Sentinel,
            UnitKind::Scuttler => Role::Scuttler,
            UnitKind::Lancer => Role::Lancer,
            UnitKind::Bombard => Role::Bombard,
            UnitKind::Flakhound | UnitKind::Stinger => Role::AntiAir,
            UnitKind::Buzzard | UnitKind::Darter => Role::AirGround,
            UnitKind::Talon | UnitKind::Wisp => Role::AirAir,
        }
    }
}

/// The most weapons any kind carries; per-weapon cooldown state is sized
/// by this.
pub const MAX_WEAPONS: usize = 2;

const HARVESTER: UnitStats = UnitStats {
    max_hp: 60,
    speed: Fx::lit("0.125"), // 2.5 tiles/s at 20 tps
    radius: Fx::lit("0.3"),
    cost: 50,
    train_ticks: 100, // 5 s
    domain: Domain::Ground,
    weapons: &[],
    aggro_range: Fx::lit("0"),
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
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        damage: 10,
        range: Fx::lit("2.5"),
        cooldown_ticks: 20, // 1 hit/s
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7, // strictly wider than aggro, so acquired targets are seen
};

const SCUTTLER: UnitStats = UnitStats {
    max_hp: 40,
    speed: Fx::lit("0.16"), // 3.2 tiles/s — outruns everything on the ground
    radius: Fx::lit("0.28"),
    cost: 40,
    train_ticks: 80, // 4 s
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        damage: 3,
        range: Fx::lit("0.8"), // practically touching
        cooldown_ticks: 6,     // a gnawing 10 dps
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 6,
};

const LANCER: UnitStats = UnitStats {
    max_hp: 50,
    speed: Fx::lit("0.08"), // 1.6 tiles/s — the army protects it, not vice versa
    radius: Fx::lit("0.35"),
    cost: 110,
    train_ticks: 200, // 10 s
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        damage: 30,
        range: Fx::lit("5.5"), // beyond aggro: it only uses this on orders
        cooldown_ticks: 60,    // one heavy shot per 3 s
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
};

const BOMBARD: UnitStats = UnitStats {
    max_hp: 80,
    speed: Fx::lit("0.06"), // 1.2 tiles/s — a gun that walks, barely
    radius: Fx::lit("0.4"),
    cost: 200,
    train_ticks: 300, // 15 s
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        damage: 45,
        range: Fx::lit("9.5"), // beyond its own vision: a spotter weapon
        cooldown_ticks: 100,   // one shell per 5 s
        targets: DomainMask::GROUND,
        splash: Some(Fx::lit("1.4")),
        indirect: true,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 5, // it cannot see as far as it shoots — on purpose
};

const FLAKHOUND: UnitStats = UnitStats {
    max_hp: 120,
    speed: Fx::lit("0.10"), // 2.0 tiles/s
    radius: Fx::lit("0.38"),
    cost: 90,
    train_ticks: 180, // 9 s
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        damage: 8,
        range: Fx::lit("5"),
        cooldown_ticks: 25,
        targets: DomainMask::AIR,
        splash: Some(Fx::lit("1.2")),
        indirect: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
};

const STINGER: UnitStats = UnitStats {
    max_hp: 45,
    speed: Fx::lit("0.14"), // 2.8 tiles/s
    radius: Fx::lit("0.28"),
    cost: 45,
    train_ticks: 100, // 5 s
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        damage: 5,
        range: Fx::lit("4.5"),
        cooldown_ticks: 20,
        targets: DomainMask::AIR,
        splash: Some(Fx::lit("1")),
        indirect: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
};

const BUZZARD: UnitStats = UnitStats {
    max_hp: 110,
    speed: Fx::lit("0.10"), // 2.0 tiles/s
    radius: Fx::lit("0.4"),
    cost: 160,
    train_ticks: 240, // 12 s
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 25,
        range: Fx::lit("3"),
        cooldown_ticks: 50,
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
};

const DARTER: UnitStats = UnitStats {
    max_hp: 55,
    speed: Fx::lit("0.17"), // 3.4 tiles/s — the fastest thing in the sky
    radius: Fx::lit("0.3"),
    cost: 90,
    train_ticks: 140, // 7 s
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 8,
        range: Fx::lit("2.5"),
        cooldown_ticks: 15,
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
};

const TALON: UnitStats = UnitStats {
    max_hp: 90,
    speed: Fx::lit("0.14"), // 2.8 tiles/s
    radius: Fx::lit("0.35"),
    cost: 110,
    train_ticks: 180, // 9 s
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 14,
        range: Fx::lit("3.5"),
        cooldown_ticks: 25,
        targets: DomainMask::AIR,
        splash: None,
        indirect: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 8,
};

const WISP: UnitStats = UnitStats {
    max_hp: 50,
    speed: Fx::lit("0.19"), // 3.8 tiles/s
    radius: Fx::lit("0.28"),
    cost: 70,
    train_ticks: 120, // 6 s
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 8,
        range: Fx::lit("3"),
        cooldown_ticks: 18,
        targets: DomainMask::AIR,
        splash: None,
        indirect: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 8,
};

const FOUNDRY: BuildingStats = BuildingStats {
    max_hp: 800,
    size: (2, 2),
    vision: 8,
    produces: &[UnitKind::Harvester, UnitKind::Sentinel],
    weapons: &[],
    construction: None,
};

const TURRET: BuildingStats = BuildingStats {
    max_hp: 350,
    size: (1, 1),
    vision: 6,
    produces: &[],
    weapons: &[WeaponStats {
        damage: 12,
        range: Fx::lit("4.5"), // the bottom rung of the siege ladder
        cooldown_ticks: 25,
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
    }],
    construction: Some(ConstructionStats {
        cost: 100,
        build_ticks: 300, // 15 s of builder attention
    }),
};

const FABRICATOR: BuildingStats = BuildingStats {
    max_hp: 500,
    size: (2, 2),
    vision: 6,
    produces: &[UnitKind::Scuttler, UnitKind::Lancer],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 150,
        build_ticks: 400, // 20 s
    }),
};

const FLAK_TURRET: BuildingStats = BuildingStats {
    max_hp: 300,
    size: (1, 1),
    vision: 7,
    produces: &[],
    weapons: &[WeaponStats {
        damage: 7,
        range: Fx::lit("5.5"),
        cooldown_ticks: 12,
        targets: DomainMask::AIR,
        splash: Some(Fx::lit("1.2")),
        indirect: false,
    }],
    construction: Some(ConstructionStats {
        cost: 90,
        build_ticks: 250,
    }),
};

const BASTION: BuildingStats = BuildingStats {
    max_hp: 500,
    size: (2, 2),
    vision: 6,
    produces: &[],
    weapons: &[WeaponStats {
        damage: 40,
        range: Fx::lit("7.5"), // beyond its own sight: full reach needs a spotter
        cooldown_ticks: 90,
        targets: DomainMask::GROUND,
        splash: Some(Fx::lit("1.3")),
        indirect: true,
    }],
    construction: Some(ConstructionStats {
        cost: 250,
        build_ticks: 500,
    }),
};

const ARRAY: BuildingStats = BuildingStats {
    max_hp: 250,
    size: (1, 1),
    vision: 9, // the inner ring: true sight
    produces: &[],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 120,
        build_ticks: 300,
    }),
};

const RECLAIMER: BuildingStats = BuildingStats {
    max_hp: 300,
    size: (1, 1),
    vision: 4,
    produces: &[],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 150,
        build_ticks: 350,
    }),
};

impl UnitKind {
    /// Static stats for this kind.
    pub const fn stats(self) -> &'static UnitStats {
        match self {
            UnitKind::Harvester => &HARVESTER,
            UnitKind::Sentinel => &SENTINEL,
            UnitKind::Scuttler => &SCUTTLER,
            UnitKind::Lancer => &LANCER,
            UnitKind::Bombard => &BOMBARD,
            UnitKind::Flakhound => &FLAKHOUND,
            UnitKind::Stinger => &STINGER,
            UnitKind::Buzzard => &BUZZARD,
            UnitKind::Darter => &DARTER,
            UnitKind::Talon => &TALON,
            UnitKind::Wisp => &WISP,
        }
    }
}

impl BuildingKind {
    /// Lowercase display name.
    pub const fn name(self) -> &'static str {
        match self {
            BuildingKind::Foundry => "foundry",
            BuildingKind::Turret => "turret",
            BuildingKind::Fabricator => "fabricator",
            BuildingKind::FlakTurret => "flak turret",
            BuildingKind::Bastion => "bastion",
            BuildingKind::Array => "array",
            BuildingKind::Reclaimer => "reclaimer",
        }
    }

    /// Static stats for this kind.
    pub const fn stats(self) -> &'static BuildingStats {
        match self {
            BuildingKind::Foundry => &FOUNDRY,
            BuildingKind::Turret => &TURRET,
            BuildingKind::Fabricator => &FABRICATOR,
            BuildingKind::FlakTurret => &FLAK_TURRET,
            BuildingKind::Bastion => &BASTION,
            BuildingKind::Array => &ARRAY,
            BuildingKind::Reclaimer => &RECLAIMER,
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
