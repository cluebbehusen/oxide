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
    /// Tier-two line brawler: an upgunned sentinel-class hull. The
    /// frontline that lets tier two fight as a wall, not a clinic.
    Warden,
    /// Armored mobile welder: field sustain for long pushes. No harvest
    /// gear — its torch is the whole job.
    Tender,
    /// Tier-two super-harvester: digs faster, hauls triple, and stands
    /// works up at twice the pace. The juiciest raid target alive.
    Excavator,
    /// Ferrous scout flyer: fast, unarmed, far-sighted.
    Kestrel,
    /// Cupric scout flyer: faster still, frailer still.
    Gnat,
    /// Ferrous heavy interceptor: the bomber's escort and its answer.
    Shrike,
    /// Cupric heavy interceptor: lighter, quicker, hungrier.
    Sylph,
    /// Ferrous strategic bomber: one enormous bomb per pass, flown on a
    /// committed attack run — it cannot stop and strafe.
    Condor,
    /// Cupric carpet bomber: a stick of six small bombs laid along its
    /// flight line each pass.
    Moth,
    /// Tier-three assault walker: a slow siege-breaking wall of a
    /// machine. Shared roster.
    Breaker,
    /// Tier-three rocket battery: extreme-reach indirect saturation with
    /// a blind ring at its feet. Shared roster.
    Avalanche,
    /// Air transport: an unarmed lifter with a four-point sling rack.
    /// Cargo rides sealed — it fights nothing, sees nothing, and dies
    /// with the airframe. Shared roster.
    Skyhook,
    /// A walking demolition charge: presses to its ordered target and
    /// detonates — enormous against structures, modest splash against
    /// machines, always fatal to itself. Shared roster.
    Sapper,
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
    /// Field workshop: an unarmed aura that welds own wounded machines —
    /// ground and air alike — inside its ring, billed per hp from the
    /// owner's bank at repair pricing.
    RepairBay,
    /// A restored strip-mining machine from the old rush. Rebuilt only
    /// on a map-authored derelict frame, it grinds the deep seams for
    /// the strongest income in the game — and the frame outlives every
    /// destruction, so the ground it stands on is contested forever.
    Extractor,
    /// Air production hall: every flyer trains here. Committing to the
    /// sky is a visible, snipeable investment.
    Airworks,
    /// The tier-three works: trains the heaviest machines and gates the
    /// deepest upgrades. Expensive, slow, and worth killing.
    Crucible,
    /// A cheap standing wall segment: blocks ground movement and
    /// nothing else. Terrain you can buy.
    Barricade,
    /// A bare scrap drop-off pad: shortens haul lines without granting
    /// production, sight, or victory weight.
    ScrapDepot,
    /// A buried demolition charge — the game's only stealth. Invisible
    /// to enemies until a scout flies close or a Deep Array's ring
    /// covers it; detonates under hostile ground machines.
    ScuttleCharge,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Closest engagement distance. Targets inside this radius cannot be
    /// selected, giving long-range emplacements an explicit close-pressure
    /// counter.
    pub minimum_range: Fx,
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
    /// rock block direct shots is skipped.
    pub indirect: bool,
    /// Bombs released per trigger pull, laid in a line along the
    /// shooter's heading through the aim point (spacing
    /// [`BOMB_SALVO_SPACING`]). 1 for every conventional weapon; only
    /// turn-limited bombers carry sticks.
    pub salvo: u8,
    /// The shot is a real projectile: a Shell entity travels to a fixed
    /// fire-time aim point and resolves on arrival. Artillery may lead an
    /// existing path before launch, but the shell is never guided and a
    /// later course change can dodge it. Hitscan when false.
    pub projectile: bool,
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
    /// Building kinds the owner must have COMPLETED before training this
    /// unit — the tech tree's production gate, identical for humans and
    /// bots. Empty means the producer alone decides.
    pub requires: &'static [BuildingKind],
    /// Whether this machine carries a welding torch: eligibility for the
    /// Repair and RepairUnit crews (and construction labor rides with
    /// `harvest` or a torch).
    pub welder: bool,
    /// Construction work applied per adjacent tick (1 for everyone but
    /// the Excavator).
    pub build_rate: u32,
    /// The machine IS its own warhead: an ordered attack ends with the
    /// unit pressing to contact and detonating (the SAPPER_* constants
    /// govern the blast). Grants attack legality without weapons.
    pub demolition: bool,
    /// Room this machine occupies aboard a transport. 0 means it can
    /// never be carried — every flyer, and the transport itself.
    pub transport_size: u8,
    /// Total cargo room this machine offers as a carrier. 0 for
    /// everything that is not a transport.
    pub transport_capacity: u8,
    /// Maximum compass steps (of 256) this unit may turn per tick.
    /// 0 means turning is free — the unit is not flight-committed. A
    /// nonzero rate makes the unit fly heading-first: it steers on a
    /// bounded arc, attacks on passes, and releases bombs only into its
    /// forward cone.
    pub turn_rate: u8,
}

impl UnitStats {
    /// The ring inside which a turn-limited flier accepts a waypoint or
    /// goal. It is the aircraft's own turn radius
    /// (`speed * 256 / (2*pi*turn_rate)`, with `256/(2*pi)` as the
    /// literal `40.75`) plus [`BOMBER_ACCEPT_SLACK`] — anything smaller
    /// is an orbit the aircraft can fly forever without ever crossing
    /// the ring. Only meaningful when `turn_rate > 0`.
    pub fn turn_acceptance(&self) -> Fx {
        debug_assert!(self.turn_rate > 0);
        self.speed * Fx::lit("40.75") / Fx::from_num(i64::from(self.turn_rate))
            + BOMBER_ACCEPT_SLACK
    }
}

impl UnitStats {
    /// Whether this kind carries any weapon at all.
    pub const fn can_fight(&self) -> bool {
        !self.weapons.is_empty() || self.demolition
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

impl BuildingKind {
    /// The tier ladder for this kind: index by a building's `tier`.
    /// Kinds without upgrades ladder alone at tier zero.
    pub const fn tiers(self) -> &'static [&'static BuildingStats] {
        match self {
            BuildingKind::Turret => &[&TURRET, &HEAVY_TURRET, &BULWARK],
            BuildingKind::FlakTurret => &[&FLAK_TURRET, &BURST_FLAK],
            BuildingKind::Reclaimer => &[&RECLAIMER, &REFINERY],
            BuildingKind::Array => &[&ARRAY, &DEEP_ARRAY],
            BuildingKind::Foundry => &[&FOUNDRY],
            BuildingKind::Fabricator => &[&FABRICATOR],
            BuildingKind::Bastion => &[&BASTION],
            BuildingKind::RepairBay => &[&REPAIR_BAY],
            BuildingKind::Extractor => &[&EXTRACTOR],
            BuildingKind::Airworks => &[&AIRWORKS],
            BuildingKind::Crucible => &[&CRUCIBLE],
            BuildingKind::Barricade => &[&BARRICADE],
            BuildingKind::ScrapDepot => &[&SCRAP_DEPOT],
            BuildingKind::ScuttleCharge => &[&SCUTTLE_CHARGE],
        }
    }

    /// Stats at `tier`, clamped to the ladder's top so a forged tier
    /// can never index past the table (the validator refuses it first).
    pub fn tier_stats(self, tier: u8) -> &'static BuildingStats {
        let tiers = self.tiers();
        tiers[(tier as usize).min(tiers.len() - 1)]
    }

    /// The upgrade that would lift a building at `tier` one rung, if
    /// the ladder continues: the next tier's construction row.
    pub fn upgrade_from(self, tier: u8) -> Option<&'static ConstructionStats> {
        self.tiers()
            .get(tier as usize + 1)
            .and_then(|stats| stats.construction.as_ref())
    }

    /// A display name per tier, so upgraded works read as what they are.
    pub const fn tier_name(self, tier: u8) -> &'static str {
        match (self, tier) {
            (BuildingKind::Turret, 1) => "heavy turret",
            (BuildingKind::Turret, 2) => "bulwark",
            (BuildingKind::FlakTurret, 1) => "burst flak",
            (BuildingKind::Reclaimer, 1) => "refinery",
            (BuildingKind::Array, 1) => "deep array",
            _ => self.name(),
        }
    }
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
    /// Building kinds the owner must have COMPLETED before placing this
    /// one — the tech tree's construction gate, identical for humans
    /// and bots. Empty means always available.
    pub requires: &'static [BuildingKind],
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
    /// Tier-two line brawler (shared).
    Warden,
    /// Mobile welder (shared).
    Tender,
    /// The attack-run bomber.
    Bomber,
    /// Tier-three assault walker (shared).
    Breaker,
    /// Tier-three rocket battery (shared).
    Avalanche,
    /// The air transport (shared).
    Skyhook,
    /// The walking demolition charge (shared).
    Sapper,
    /// Super-harvester (shared).
    Excavator,
    /// Unarmed far-sighted flyer — faction-varied.
    Scout,
    /// Heavy air-superiority flyer — faction-varied.
    Interceptor,
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
            (Role::Warden, _) => UnitKind::Warden,
            (Role::Tender, _) => UnitKind::Tender,
            (Role::Excavator, _) => UnitKind::Excavator,
            (Role::Scout, Faction::Ferrous) => UnitKind::Kestrel,
            (Role::Scout, Faction::Cupric) => UnitKind::Gnat,
            (Role::Interceptor, Faction::Ferrous) => UnitKind::Shrike,
            (Role::Interceptor, Faction::Cupric) => UnitKind::Sylph,
            (Role::Bomber, Faction::Ferrous) => UnitKind::Condor,
            (Role::Bomber, Faction::Cupric) => UnitKind::Moth,
            (Role::Breaker, _) => UnitKind::Breaker,
            (Role::Avalanche, _) => UnitKind::Avalanche,
            (Role::Skyhook, _) => UnitKind::Skyhook,
            (Role::Sapper, _) => UnitKind::Sapper,
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
            UnitKind::Warden
            | UnitKind::Tender
            | UnitKind::Excavator
            | UnitKind::Breaker
            | UnitKind::Avalanche
            | UnitKind::Skyhook
            | UnitKind::Sapper => None,
            UnitKind::Flakhound
            | UnitKind::Buzzard
            | UnitKind::Talon
            | UnitKind::Kestrel
            | UnitKind::Shrike
            | UnitKind::Condor => Some(Faction::Ferrous),
            UnitKind::Stinger
            | UnitKind::Darter
            | UnitKind::Wisp
            | UnitKind::Gnat
            | UnitKind::Sylph
            | UnitKind::Moth => Some(Faction::Cupric),
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
            UnitKind::Warden => "warden",
            UnitKind::Tender => "tender",
            UnitKind::Excavator => "excavator",
            UnitKind::Kestrel => "kestrel",
            UnitKind::Gnat => "gnat",
            UnitKind::Shrike => "shrike",
            UnitKind::Sylph => "sylph",
            UnitKind::Condor => "condor",
            UnitKind::Moth => "moth",
            UnitKind::Breaker => "breaker",
            UnitKind::Avalanche => "avalanche",
            UnitKind::Skyhook => "skyhook",
            UnitKind::Sapper => "sapper",
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
            UnitKind::Warden => Role::Warden,
            UnitKind::Tender => Role::Tender,
            UnitKind::Excavator => Role::Excavator,
            UnitKind::Kestrel | UnitKind::Gnat => Role::Scout,
            UnitKind::Shrike | UnitKind::Sylph => Role::Interceptor,
            UnitKind::Condor | UnitKind::Moth => Role::Bomber,
            UnitKind::Breaker => Role::Breaker,
            UnitKind::Avalanche => Role::Avalanche,
            UnitKind::Skyhook => Role::Skyhook,
            UnitKind::Sapper => Role::Sapper,
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
    requires: &[],
    welder: true,
    build_rate: 1,
    demolition: false,
    transport_size: 1,
    transport_capacity: 0,
    turn_rate: 0,
};

const SENTINEL: UnitStats = UnitStats {
    // 0.10 balance, third pass: 100 hp made the line unit the best
    // value mass in the roster and the optimizer proved it (nine
    // campaign rounds of sentinel floods). At 60 the rail one-shots
    // it, scuttler swarms out-trade it, and turrets drop it in five
    // hits: the sentinel is a screen and a scout, not a war-winner.
    max_hp: 60,
    speed: Fx::lit("0.11"), // 2.2 tiles/s — armies are slightly outrun by harvesters
    radius: Fx::lit("0.35"),
    cost: 90, // 0.10 balance: spam pays — four campaign rounds proved 75 optimal-by-flooding
    // 0.13 balance: 7.5 s, and load-bearing twice over despite the kind
    // being faction-shared. Measured at 160 under the 0.13 economy
    // (yardstick: the since-deleted classic bot): long-haul stalls past
    // the liveness horizon (8,245
    // ticks of zero progress), and the mixed-roster marginal reads
    // ferrous 37.3% [34.9, 39.8] against 48.5% at 150 — Ferrous fields
    // the heavier Sentinel share, so the shared cadence is not
    // faction-neutral in play.
    train_ticks: 150, // 7.5 s
    domain: Domain::Ground,
    weapons: &[
        WeaponStats {
            damage: 10,
            range: Fx::lit("2.5"),
            minimum_range: Fx::ZERO,
            cooldown_ticks: 20, // 1 hit/s
            targets: DomainMask::GROUND,
            splash: None,
            indirect: false,
            salvo: 1,
            projectile: false,
        },
        // A weak skyward poke: the tier-0 reason a pure air ball cannot
        // blank the core army — dedicated anti-air still hard-counters.
        WeaponStats {
            damage: 4,
            range: Fx::lit("3"),
            minimum_range: Fx::ZERO,
            cooldown_ticks: 30,
            targets: DomainMask::AIR,
            splash: None,
            indirect: false,
            salvo: 1,
            projectile: false,
        },
    ],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7, // strictly wider than aggro, so acquired targets are seen
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 1,
    transport_capacity: 0,
    turn_rate: 0,
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
        minimum_range: Fx::ZERO,
        cooldown_ticks: 6, // a gnawing 10 dps
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 6,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 1,
    transport_capacity: 0,
    turn_rate: 0,
};

const LANCER: UnitStats = UnitStats {
    max_hp: 50,
    speed: Fx::lit("0.08"), // 1.6 tiles/s — the army protects it, not vice versa
    radius: Fx::lit("0.35"),
    cost: 110,
    train_ticks: 200, // 10 s
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        // 0.10 balance: at 30 the rail matched sentinel dps at higher
        // cost and lost par to the entire roster — the tech tree's
        // first rung wasn't worth climbing. 60 two-shots a sentinel and
        // one-shots the light roster; siege and air still counter.
        damage: 60,
        range: Fx::lit("5.5"), // beyond aggro: it only uses this on orders
        minimum_range: Fx::ZERO,
        cooldown_ticks: 60, // one heavy shot per 3 s
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 2,
    transport_capacity: 0,
    turn_rate: 0,
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
        minimum_range: Fx::ZERO,
        cooldown_ticks: 100, // one shell per 5 s
        targets: DomainMask::GROUND,
        splash: Some(Fx::lit("1.4")),
        indirect: true,
        salvo: 1,
        projectile: true,
    }],
    aggro_range: Fx::lit("9.5"), // its whole spotter-enabled firing envelope
    harvest: None,
    vision: 5, // it cannot see as far as it shoots — on purpose
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 3,
    transport_capacity: 0,
    turn_rate: 0,
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
        minimum_range: Fx::ZERO,
        cooldown_ticks: 25,
        targets: DomainMask::AIR,
        splash: Some(Fx::lit("1.2")),
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 2,
    transport_capacity: 0,
    turn_rate: 0,
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
        minimum_range: Fx::ZERO,
        cooldown_ticks: 20,
        targets: DomainMask::AIR,
        splash: Some(Fx::lit("1")),
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 1,
    transport_capacity: 0,
    turn_rate: 0,
};

const BUZZARD: UnitStats = UnitStats {
    max_hp: 110,
    speed: Fx::lit("0.10"), // 2.0 tiles/s
    radius: Fx::lit("0.4"),
    // 0.13 balance: at 160 the durable flyer was strictly outclassed by
    // the Darter (more dps at 56% the price) and fell out of play (0.8%
    // of army value). 120 is arena par against a common Sentinel line:
    // the Buzzard keeps more value, the Darter clears faster.
    cost: 120,
    train_ticks: 180, // 9 s
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 25,
        range: Fx::lit("3"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 50,
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 0,
};

const DARTER: UnitStats = UnitStats {
    max_hp: 55,
    speed: Fx::lit("0.17"), // 3.4 tiles/s — the fastest thing in the sky
    radius: Fx::lit("0.3"),
    // 0.13 balance: 90 underpriced the speed — the factorial probe read
    // Ferrous at 21.5% of mixed-roster victories under the old prices.
    // The shipped 100 (with the Sentinel at 150 ticks) measures 48.5%;
    // the harsher 110 probe read 46.9% and was rejected as overshoot.
    cost: 100,
    train_ticks: 150, // 7.5 s
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 8,
        range: Fx::lit("2.5"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 15,
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 0,
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
        minimum_range: Fx::ZERO,
        cooldown_ticks: 25,
        targets: DomainMask::AIR,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 8,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 0,
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
        minimum_range: Fx::ZERO,
        cooldown_ticks: 18,
        targets: DomainMask::AIR,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 8,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 0,
};

const WARDEN: UnitStats = UnitStats {
    // 0.15 balance lab: at 240hp/24dmg the Lancer didn't counter the
    // Warden, it deleted it (cost-normalized arena: 0-550 wipe both
    // seats) while the Lancer also carried 2.6x the damage-per-scrap —
    // so learned play rationally never left tier one. The line brawler
    // now trades into massed rails instead of evaporating; the Lancer
    // keeps the per-cost edge as the dedicated answer.
    max_hp: 260,
    speed: Fx::lit("0.09"),
    radius: Fx::lit("0.45"),
    cost: 280,
    train_ticks: 400,
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        damage: 32,
        range: Fx::lit("3"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 25,
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 7,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 2,
    transport_capacity: 0,
    turn_rate: 0,
};

const TENDER: UnitStats = UnitStats {
    max_hp: 150,
    speed: Fx::lit("0.11"),
    radius: Fx::lit("0.38"),
    cost: 180,
    train_ticks: 300,
    domain: Domain::Ground,
    weapons: &[],
    aggro_range: Fx::ZERO,
    harvest: None,
    vision: 7,
    requires: &[],
    welder: true,
    build_rate: 1,
    demolition: false,
    transport_size: 2,
    transport_capacity: 0,
    turn_rate: 0,
};

const EXCAVATOR: UnitStats = UnitStats {
    max_hp: 160,
    speed: Fx::lit("0.11"),
    radius: Fx::lit("0.42"),
    cost: 200,
    train_ticks: 350,
    domain: Domain::Ground,
    weapons: &[],
    aggro_range: Fx::ZERO,
    harvest: Some(HarvestStats {
        capacity: 30,
        ticks_per_scrap: 5,
    }),
    vision: 6,
    requires: &[BuildingKind::Fabricator],
    welder: true,
    build_rate: 2,
    demolition: false,
    transport_size: 2,
    transport_capacity: 0,
    turn_rate: 0,
};

const KESTREL: UnitStats = UnitStats {
    max_hp: 60,
    speed: Fx::lit("0.2"),
    radius: Fx::lit("0.3"),
    cost: 60,
    train_ticks: 120,
    domain: Domain::Air,
    weapons: &[],
    aggro_range: Fx::ZERO,
    harvest: None,
    vision: 10,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 0,
};

const GNAT: UnitStats = UnitStats {
    max_hp: 45,
    speed: Fx::lit("0.22"),
    radius: Fx::lit("0.26"),
    cost: 50,
    train_ticks: 100,
    domain: Domain::Air,
    weapons: &[],
    aggro_range: Fx::ZERO,
    harvest: None,
    vision: 10,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 0,
};

const SHRIKE: UnitStats = UnitStats {
    max_hp: 160,
    speed: Fx::lit("0.16"),
    radius: Fx::lit("0.38"),
    cost: 260,
    train_ticks: 300,
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 30,
        range: Fx::lit("4"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 30,
        targets: DomainMask::AIR,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("6"),
    harvest: None,
    vision: 8,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 0,
};

const SYLPH: UnitStats = UnitStats {
    max_hp: 100,
    speed: Fx::lit("0.21"),
    radius: Fx::lit("0.3"),
    cost: 200,
    train_ticks: 240,
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 16,
        range: Fx::lit("3.5"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 20,
        targets: DomainMask::AIR,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("6"),
    harvest: None,
    vision: 8,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 0,
};

const CONDOR: UnitStats = UnitStats {
    max_hp: 260,
    speed: Fx::lit("0.11"),
    radius: Fx::lit("0.45"),
    cost: 700,
    train_ticks: 800,
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 100,
        range: Fx::lit("2.5"), // release point, not a standoff gun
        minimum_range: Fx::ZERO,
        cooldown_ticks: 150, // one bomb per pass; the loop IS the reload
        targets: DomainMask::GROUND,
        splash: Some(Fx::lit("2.2")),
        indirect: true,
        salvo: 1,
        projectile: true,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 6,
    requires: &[BuildingKind::Crucible],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 2, // ~2.2-tile turn radius: every run is a commitment
};

const MOTH: UnitStats = UnitStats {
    max_hp: 140,
    speed: Fx::lit("0.15"),
    radius: Fx::lit("0.4"),
    cost: 550,
    train_ticks: 700,
    domain: Domain::Air,
    weapons: &[WeaponStats {
        damage: 25,
        range: Fx::lit("2.5"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 130,
        targets: DomainMask::GROUND,
        splash: Some(Fx::lit("1.2")),
        indirect: true,
        salvo: 6, // the stick, laid along the flight line
        projectile: true,
    }],
    aggro_range: Fx::lit("5"),
    harvest: None,
    vision: 6,
    requires: &[BuildingKind::Crucible],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 0,
    turn_rate: 3, // tighter loops than the Condor, weaker punch
};

const BREAKER: UnitStats = UnitStats {
    // 0.15 balance lab: the tier-crusher coin-flipped cost-equal
    // Lancer mass (verdict flipped on seat swap) — a 900-scrap unit
    // behind a Crucible that trades evenly with tier one is a climb
    // nobody should make. One shell now deletes a rail and its splash
    // punishes the clump; bombers, artillery, and economy remain the
    // honest answers.
    max_hp: 900,
    speed: Fx::lit("0.055"),
    radius: Fx::lit("0.55"),
    cost: 900,
    train_ticks: 1200,
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        damage: 115,
        range: Fx::lit("4.5"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 60,
        targets: DomainMask::GROUND,
        splash: Some(Fx::lit("1.5")),
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    aggro_range: Fx::lit("6"),
    harvest: None,
    vision: 6,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 4,
    transport_capacity: 0,
    turn_rate: 0,
};

const AVALANCHE: UnitStats = UnitStats {
    max_hp: 300,
    speed: Fx::lit("0.045"),
    radius: Fx::lit("0.5"),
    cost: 700,
    train_ticks: 900,
    domain: Domain::Ground,
    weapons: &[WeaponStats {
        // 0.15 balance lab: at 70/140t the superheavy needed TWO
        // seven-second shots per Bombard and lost the cost-normalized
        // artillery duel outright (0-800 both seats) — tier-one
        // artillery obsoleted its own successor. One shell now deletes
        // a Bombard on the drop; rushes inside the blind ring and the
        // sky it cannot answer stay lethal.
        damage: 110,
        range: Fx::lit("14"),        // far past its own eyes: a spotter weapon
        minimum_range: Fx::lit("4"), // blind at its feet — close the gap
        cooldown_ticks: 120,
        targets: DomainMask::GROUND,
        splash: Some(Fx::lit("1.6")),
        indirect: true,
        salvo: 1,
        projectile: true,
    }],
    aggro_range: Fx::lit("14"),
    harvest: None,
    vision: 5,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 4,
    transport_capacity: 0,
    turn_rate: 0,
};

const SKYHOOK: UnitStats = UnitStats {
    max_hp: 200,
    speed: Fx::lit("0.13"),
    radius: Fx::lit("0.45"),
    cost: 250,
    train_ticks: 400,
    domain: Domain::Air,
    weapons: &[],
    aggro_range: Fx::ZERO,
    harvest: None,
    vision: 6,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: false,
    transport_size: 0,
    transport_capacity: 4,
    turn_rate: 0,
};

const SAPPER: UnitStats = UnitStats {
    max_hp: 50,
    speed: Fx::lit("0.15"),
    radius: Fx::lit("0.3"),
    cost: 120,
    train_ticks: 180,
    domain: Domain::Ground,
    weapons: &[],
    aggro_range: Fx::ZERO, // it never picks its own grave
    harvest: None,
    vision: 5,
    requires: &[],
    welder: false,
    build_rate: 1,
    demolition: true,
    transport_size: 1,
    transport_capacity: 0,
    turn_rate: 0,
};

const FOUNDRY: BuildingStats = BuildingStats {
    // 0.10 pacing: doubled so a rush can pressure but rarely close a
    // match in single-digit minutes (measured: +60-75% match length).
    max_hp: 1600,
    size: (2, 2),
    vision: 8,
    produces: &[
        UnitKind::Harvester,
        UnitKind::Sentinel,
        UnitKind::Scuttler,
        UnitKind::Excavator,
    ],
    weapons: &[],
    // 0.15: buildable — the expansion base and the comeback path. Gated
    // on a Fabricator so a proxy Foundry is a committed tech play, and
    // priced so its income drip alone never pays for it (~20 minutes;
    // production, drop-off reach, and survivability are the reasons to
    // build one). Victory counts sites too, so a dying main can be
    // answered by ground already claimed.
    construction: Some(ConstructionStats {
        cost: 400,
        build_ticks: 800,
        requires: &[BuildingKind::Fabricator],
    }),
};

const TURRET: BuildingStats = BuildingStats {
    max_hp: 350,
    size: (1, 1),
    vision: 6,
    produces: &[],
    weapons: &[WeaponStats {
        damage: 12,
        range: Fx::lit("5"), // the bottom rung of the siege ladder
        minimum_range: Fx::ZERO,
        cooldown_ticks: 25,
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    construction: Some(ConstructionStats {
        cost: 100,
        build_ticks: 300, // 15 s of builder attention
        requires: &[],
    }),
};

const FABRICATOR: BuildingStats = BuildingStats {
    max_hp: 500,
    size: (2, 2),
    vision: 6,
    // Both factions' variants are listed; the train gate deals each seat
    // only its own. Order groups the roles for the HUD's slot labels.
    produces: &[
        UnitKind::Lancer,
        UnitKind::Bombard,
        UnitKind::Flakhound,
        UnitKind::Stinger,
        UnitKind::Warden,
        UnitKind::Tender,
        UnitKind::Sapper,
    ],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 120,
        build_ticks: 280, // 14 s — the tech window must fit inside the rush window
        requires: &[],
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
        minimum_range: Fx::ZERO,
        cooldown_ticks: 12,
        targets: DomainMask::AIR,
        splash: Some(Fx::lit("1.2")),
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    construction: Some(ConstructionStats {
        cost: 90,
        build_ticks: 250,
        requires: &[],
    }),
};

const BASTION: BuildingStats = BuildingStats {
    max_hp: 500,
    size: (2, 2),
    vision: 6,
    produces: &[],
    weapons: &[WeaponStats {
        damage: 40,
        range: Fx::lit("9.5"), // artillery parity; full reach needs a spotter
        minimum_range: Fx::lit("2.5"),
        cooldown_ticks: 90,
        targets: DomainMask::GROUND,
        splash: Some(Fx::lit("1.3")),
        indirect: true,
        salvo: 1,
        projectile: true,
    }],
    construction: Some(ConstructionStats {
        cost: 250,
        build_ticks: 500,
        requires: &[],
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
        requires: &[],
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
        requires: &[],
    }),
};

const REPAIR_BAY: BuildingStats = BuildingStats {
    max_hp: 400,
    size: (2, 2),
    vision: 5,
    produces: &[],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 200,
        build_ticks: 350,
        requires: &[],
    }),
};

const AIRWORKS: BuildingStats = BuildingStats {
    max_hp: 500,
    size: (2, 2),
    vision: 6,
    // Both factions' wings are listed; the train gate deals each seat
    // only its own.
    produces: &[
        UnitKind::Buzzard,
        UnitKind::Darter,
        UnitKind::Talon,
        UnitKind::Wisp,
        UnitKind::Kestrel,
        UnitKind::Gnat,
        UnitKind::Shrike,
        UnitKind::Sylph,
        UnitKind::Condor,
        UnitKind::Moth,
        UnitKind::Skyhook,
    ],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 200,
        build_ticks: 350,
        requires: &[BuildingKind::Fabricator],
    }),
};

const CRUCIBLE: BuildingStats = BuildingStats {
    max_hp: 900,
    size: (2, 2),
    vision: 6,
    produces: &[UnitKind::Breaker, UnitKind::Avalanche],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 500,
        build_ticks: 700,
        requires: &[BuildingKind::Fabricator],
    }),
};

const BARRICADE: BuildingStats = BuildingStats {
    max_hp: 400,
    size: (1, 1),
    vision: 1,
    produces: &[],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 40,
        build_ticks: 120,
        requires: &[],
    }),
};

const SCRAP_DEPOT: BuildingStats = BuildingStats {
    max_hp: 300,
    size: (1, 1),
    vision: 3,
    produces: &[],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 80,
        build_ticks: 200,
        requires: &[],
    }),
};

const SCUTTLE_CHARGE: BuildingStats = BuildingStats {
    max_hp: 20,
    size: (1, 1),
    vision: 1,
    produces: &[],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 30,
        build_ticks: 60,
        requires: &[BuildingKind::Fabricator],
    }),
};

const EXTRACTOR: BuildingStats = BuildingStats {
    max_hp: 600,
    size: (2, 2),
    vision: 4,
    produces: &[],
    weapons: &[],
    // Cheap to restore, brutal to hold: the price buys the strongest
    // income in the game on ground everyone can read from the map.
    construction: Some(ConstructionStats {
        cost: 100,
        build_ticks: 300,
        requires: &[],
    }),
};

// ---- Upgrade tiers ----------------------------------------------------
//
// Each upgradeable kind carries an array of tier structs; a building's
// `tier` indexes it. A tier's `construction` row is the price of the
// UPGRADE that produced it (tier 0 keeps the ordinary build price), so
// repair pricing and refund logic read the tier they are welding.
// `BuildingStats.upgrade` names the next tier's row where one exists.

const HEAVY_TURRET: BuildingStats = BuildingStats {
    max_hp: 500,
    size: (1, 1),
    vision: 6,
    produces: &[],
    weapons: &[WeaponStats {
        damage: 20,
        range: Fx::lit("6"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 25,
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    construction: Some(ConstructionStats {
        cost: 150,
        build_ticks: 300,
        requires: &[BuildingKind::Fabricator],
    }),
};

const BULWARK: BuildingStats = BuildingStats {
    max_hp: 900,
    size: (1, 1),
    vision: 7,
    produces: &[],
    weapons: &[WeaponStats {
        damage: 60,
        range: Fx::lit("7.5"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 50,
        targets: DomainMask::GROUND,
        splash: None,
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    construction: Some(ConstructionStats {
        cost: 300,
        build_ticks: 500,
        requires: &[BuildingKind::Crucible],
    }),
};

const BURST_FLAK: BuildingStats = BuildingStats {
    max_hp: 400,
    size: (1, 1),
    vision: 7,
    produces: &[],
    weapons: &[WeaponStats {
        damage: 12,
        range: Fx::lit("6"),
        minimum_range: Fx::ZERO,
        cooldown_ticks: 10,
        targets: DomainMask::AIR,
        splash: Some(Fx::lit("1.5")),
        indirect: false,
        salvo: 1,
        projectile: false,
    }],
    construction: Some(ConstructionStats {
        cost: 120,
        build_ticks: 250,
        requires: &[BuildingKind::Fabricator],
    }),
};

const REFINERY: BuildingStats = BuildingStats {
    max_hp: 400,
    size: (1, 1),
    vision: 4,
    produces: &[],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 150,
        build_ticks: 300,
        requires: &[BuildingKind::Fabricator],
    }),
};

const DEEP_ARRAY: BuildingStats = BuildingStats {
    max_hp: 300,
    size: (1, 1),
    vision: 11,
    produces: &[],
    weapons: &[],
    construction: Some(ConstructionStats {
        cost: 150,
        build_ticks: 300,
        requires: &[BuildingKind::Fabricator],
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
            UnitKind::Warden => &WARDEN,
            UnitKind::Tender => &TENDER,
            UnitKind::Excavator => &EXCAVATOR,
            UnitKind::Kestrel => &KESTREL,
            UnitKind::Gnat => &GNAT,
            UnitKind::Shrike => &SHRIKE,
            UnitKind::Sylph => &SYLPH,
            UnitKind::Condor => &CONDOR,
            UnitKind::Moth => &MOTH,
            UnitKind::Breaker => &BREAKER,
            UnitKind::Avalanche => &AVALANCHE,
            UnitKind::Skyhook => &SKYHOOK,
            UnitKind::Sapper => &SAPPER,
        }
    }

    /// Whether this unit can serve as the ground escort in a stranded
    /// economy's recovery package.
    pub(crate) fn is_recovery_screen(self) -> bool {
        let stats = self.stats();
        stats.domain == Domain::Ground
            && stats
                .weapons
                .iter()
                .any(|weapon| weapon.targets.covers(Domain::Ground) && !weapon.projectile)
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
            BuildingKind::RepairBay => "repair bay",
            BuildingKind::Extractor => "extractor",
            BuildingKind::Airworks => "airworks",
            BuildingKind::Crucible => "crucible",
            BuildingKind::Barricade => "barricade",
            BuildingKind::ScrapDepot => "scrap depot",
            BuildingKind::ScuttleCharge => "scuttle charge",
        }
    }

    /// Tier-zero stats for this kind. Most callers want a live
    /// building's [`crate::state::Building::stats`], which follows the
    /// upgrade ladder; this base row is for costs, footprints, and
    /// other tier-invariant questions.
    pub const fn base_stats(self) -> &'static BuildingStats {
        match self {
            BuildingKind::Foundry => &FOUNDRY,
            BuildingKind::Turret => &TURRET,
            BuildingKind::Fabricator => &FABRICATOR,
            BuildingKind::FlakTurret => &FLAK_TURRET,
            BuildingKind::Bastion => &BASTION,
            BuildingKind::Array => &ARRAY,
            BuildingKind::Reclaimer => &RECLAIMER,
            BuildingKind::RepairBay => &REPAIR_BAY,
            BuildingKind::Extractor => &EXTRACTOR,
            BuildingKind::Airworks => &AIRWORKS,
            BuildingKind::Crucible => &CRUCIBLE,
            BuildingKind::Barricade => &BARRICADE,
            BuildingKind::ScrapDepot => &SCRAP_DEPOT,
            BuildingKind::ScuttleCharge => &SCUTTLE_CHARGE,
        }
    }

    /// Whether harvesters can deliver their cargo here. The one funnel
    /// every drop-off decision consults — deliveries, retirement homes,
    /// and route planning alike.
    pub const fn is_drop_off(self) -> bool {
        matches!(self, BuildingKind::Foundry | BuildingKind::ScrapDepot)
    }

    /// Whether this kind hides from enemies until actively detected
    /// (see `State::building_apparent`). The Scuttle Charge is the
    /// game's only stealth.
    pub const fn is_stealthy(self) -> bool {
        matches!(self, BuildingKind::ScuttleCharge)
    }
}

/// Scrap contained in a freshly parsed node tile.
pub const SCRAP_NODE_AMOUNT: u32 = 400;

/// Fraction of a destroyed machine's price left on the field as wreck
/// salvage: losing an army literally feeds the other side's harvesters.
pub const WRECK_VALUE_NUM: u32 = 45;
/// Denominator of the wreck-value fraction.
pub const WRECK_VALUE_DEN: u32 = 100;

/// The Foundry is never bought, so its wreck value is authored: a prize
/// worth fighting over where a base used to stand.
pub const FOUNDRY_WRECK_VALUE: u32 = 300;

/// Ticks between global wreck-decay steps (every wreck tile loses one
/// salvage per step). Battlefield scrap is a prize that outlives the
/// battle — worth a deliberate trip minutes later — but never a
/// permanent bank.
pub const WRECK_DECAY_TICKS: u64 = 300;

/// Outer detection ring of the Array, in tiles: hostile units inside it
/// but out of true sight appear as blips — a tile, no kind, no owner.
/// Blips never satisfy targeted-attack visibility.
pub const RADAR_DETECT_RADIUS: i32 = 16;

/// Shell flight speed in tiles per tick. A full-range 9.5-tile lob takes
/// about 32 ticks: path-aware aim catches a straight commitment, while a
/// reacting Scuttler can change course by 4+ tiles before impact.
pub const SHELL_SPEED: Fx = Fx::lit("0.30");

/// Ticks per scrap credited by each built Reclaimer. At this rate the
/// building repays its own price in roughly three minutes — insurance and
/// a stalemate valve, never an opening.
pub const RECLAIMER_PERIOD: u64 = 24;

/// Ticks per emergency scrap credited by a surviving Foundry after its
/// owner's last Harvester is gone. Each real deposit arms one finite
/// recovery entitlement; spending or cancelling that package cannot refill
/// it.
pub const FOUNDRY_RECOVERY_PERIOD: u64 = 10;

/// Maximum symmetric emergency entitlement available to a stranded seat:
/// one cheap screen plus its replacement Harvester. A seat with a paid
/// ground screen captures only the Harvester-sized deficit.
pub const FOUNDRY_RECOVERY_RESERVE: u32 = SENTINEL.cost + HARVESTER.cost;

/// Release gate for turn-limited bombers: the target must sit inside
/// the forward cone, `dot(heading, to_target) >= |to_target| * CONE`.
/// 0.92 is a half-angle of about 23 degrees — wide enough that a clean
/// pass releases, narrow enough that a bomber circling its target must
/// straighten out before the bay opens.
pub const BOMBER_CONE_DOT: Fx = Fx::lit("0.92");

/// Distance between consecutive bombs of a stick along the flight line.
pub const BOMB_SALVO_SPACING: Fx = Fx::lit("0.8");

/// Acceptance slack added to a turn-limited flier's computed turn
/// radius: the ring inside which a waypoint or goal counts as reached.
/// The radius itself must dominate — an acceptance ring smaller than
/// the turn radius is an orbit trap the aircraft can circle forever.
pub const BOMBER_ACCEPT_SLACK: Fx = Fx::lit("0.4");

/// How close a boarding machine must stand to its transport before the
/// sling takes it.
pub const LOAD_REACH: Fx = Fx::lit("1.5");

/// Ring-scan radius when a transport sets its cargo down: the farthest
/// tile from the drop point a disgorged machine may appear on.
pub const UNLOAD_SCAN_RADIUS: i32 = 4;

/// A hostile ground machine inside this radius of a buried charge sets
/// it off.
pub const CHARGE_TRIGGER_RADIUS: Fx = Fx::lit("0.8");

/// Damage a detonating charge deals to every hostile ground machine in
/// its blast ring.
pub const CHARGE_DAMAGE: u32 = 60;

/// The charge's blast ring.
pub const CHARGE_BLAST_RADIUS: Fx = Fx::lit("1.5");

/// A scout-role flyer within this many tiles reveals buried charges to
/// its team.
pub const CHARGE_SCOUT_DETECT_RADIUS: i32 = 4;

/// A built Deep Array (Array tier 1) reveals buried charges anywhere
/// inside its radar ring (euclidean, like radar contacts).
pub const CHARGE_ARRAY_DETECT_RADIUS: i32 = 22;

/// A Sapper reaching contact with its ordered target detonates: this
/// lands on a building target directly...
pub const SAPPER_STRUCTURE_DAMAGE: u32 = 250;

/// ...while every hostile ground machine in the blast ring (the
/// building's occupants aside) takes the splash.
pub const SAPPER_SPLASH_DAMAGE: u32 = 60;

/// The Sapper's blast ring.
pub const SAPPER_BLAST_RADIUS: Fx = Fx::lit("1.5");

/// How close the Sapper must press to its target before the charge
/// fires (measured to the target's closest point).
pub const SAPPER_CONTACT_RANGE: Fx = Fx::lit("0.9");

/// Ticks per scrap smelted by each standing, completed Foundry — the
/// transparent income floor.
///
/// This is the economy's guarantee: exhausted nodes, lost Reclaimers,
/// and camped salvage can make progress slow, but never leave a seat
/// with no income at all. Credit is per Foundry so expansion bases are
/// worth their keep, but the rate is tuned so income alone never pays
/// for one (20/min against a 400 cost: production, drop-off reach,
/// and survivability are the reasons to expand). Watched in training
/// telemetry for foundry-farm degeneracy; the fallback design is a
/// flat per-player floor at this same period.
pub const FOUNDRY_DRIP_PERIOD: u64 = 60;

/// First completed tick eligible for the drip: a two-minute warm-up.
/// The floor exists for mid- and late-game lockouts; openings stay
/// exactly as tuned without it. Measured (against the since-deleted
/// 0.14 scripted bots): a from-tick-zero drip handed an omniscient
/// anchor a decisive edge over a fog-honest brain (13/40 -> passing)
/// purely on perfectly-converted
/// early free scrap — the floor should never be an opening build order.
pub const FOUNDRY_DRIP_START_TICK: u64 = 2_400;

/// Ticks per scrap ground by a tier-one Reclaimer (the Refinery) — two
/// and a half times the base drum, the roadmap's "improved Reclaimer".
pub const REFINERY_PERIOD: u64 = 10;

/// Extractor yield: `(first eligible completed tick, scrap, per ticks)`
/// rows, later rows superseding earlier ones. The escalation is the
/// visible late-game pressure rule: map control compounds, so turtling
/// on the drip against a seat holding restored Extractors is a legible
/// death spiral rather than a stalemate. Base 120 scrap/min, +50% from
/// ten minutes, doubled from twenty.
pub const EXTRACTOR_YIELD_SCHEDULE: [(u64, u32, u64); 3] =
    [(0, 1, 10), (12_000, 3, 20), (24_000, 2, 10)];

/// Ticks between decay steps on an unattended construction site (one hp
/// per step, applied while no own harvest-capable machine stands beside
/// the footprint). Sites count for survival exactly like standing
/// Foundries, so abandoned scaffolds must rust away rather than keep a
/// beaten seat technically alive forever — and an untended half-built
/// anything is a melting asset, not a free land claim.
pub const SITE_DECAY_PERIOD: u64 = 8;

/// Per-mille of a building's cost billed per hp welded (against max_hp).
/// The three economy verbs price strictly build > repair > salvage:
/// welding always costs more than salvage refunds, so repair-then-salvage
/// strictly loses scrap, and a full re-ramp costs ~68% of the price —
/// cheaper than replacing it, never free, and a real sustain tax under
/// fire (the 0.11 repricing; the old flat tick-trickle accidentally
/// charged 25-47% and would have made salvage refunds a printer).
pub const REPAIR_COST_PERMILLE: u64 = 850;

pub(crate) fn unit_repair_debit(kind: UnitKind, progress: u32) -> u32 {
    let stats = kind.stats();
    let owed = |ticks: u32| {
        let welded = u64::from(stats.max_hp) * u64::from(ticks) / u64::from(stats.train_ticks);
        welded * u64::from(stats.cost) * REPAIR_COST_PERMILLE / u64::from(stats.max_hp)
    };
    u32::try_from(owed(progress + 1).div_ceil(1000) - owed(progress).div_ceil(1000))
        .expect("one unit-repair tick debit fits u32")
}

pub(crate) fn unit_repair_opening_debit(kind: UnitKind) -> u32 {
    let stats = kind.stats();
    let first_weld_tick = stats.train_ticks.div_ceil(stats.max_hp);
    unit_repair_debit(kind, first_weld_tick - 1)
}

/// Per-mille of a building's cost refunded per hp drained by salvage
/// (against max_hp). A full-health salvage banks exactly cost*800/1000.
pub const SALVAGE_REFUND_PERMILLE: u64 = 800;

/// How close a welder must stand to a wounded machine for the torch to
/// hold, in tiles between body centers — body contact, a hair over the
/// widest radius pair, and well under any weapon's reach. Unit welds
/// have no footprint to be adjacent to; this is their adjacency.
pub const REPAIR_REACH: Fx = Fx::lit("1.2");

/// Reach of the Repair Bay's welding aura, in tiles from the nearest
/// point of its footprint — a base ring, not battlefield cover: shorter
/// than every siege weapon's reach, so the counter to a healed defense
/// is standing outside it.
pub const REPAIR_BAY_RADIUS: Fx = Fx::lit("4.0");

/// Ticks between Repair Bay aura pulses. With [`REPAIR_BAY_STEP`] this
/// sets the sustain rate per patient: 1 hp / 8 ticks — around a quarter
/// of one Turret's damage rate, so an aura never out-heals focused
/// fire; its value is breadth (every wounded machine in the ring heals
/// at once) and never needing a harvester's torch time.
pub const REPAIR_BAY_PERIOD: u64 = 8;

/// Hp each aura pulse offers each patient in the ring.
pub const REPAIR_BAY_STEP: u32 = 1;

/// Welding ramp for the Foundry, which has no construction stats to
/// borrow one from.
pub const FOUNDRY_REPAIR_TICKS: u32 = 400;

/// Billing basis for Foundry repair, which has no purchase cost to
/// price against. Chosen so a full re-ramp runs ~68 scrap — pricier
/// than the pre-0.11 flat trickle's 40 (the sustain tax is intended)
/// without making the victory token unhealable in a siege.
pub const FOUNDRY_REPAIR_PRICE: u32 = 100;

/// Scrap in a rich node (the `S` map legend) — a fought-over prize.
pub const RICH_SCRAP_NODE_AMOUNT: u32 = 800;

/// Maximum queued units per Foundry.
pub const QUEUE_CAP: usize = 8;

/// Maximum orders (and patrol waypoints) queued per unit. Bounds what a
/// hostile append stream can make a unit remember.
pub const ORDER_QUEUE_CAP: usize = 32;

/// A* expansion budget per query — bounds worst-case pathfinding work.
pub const PATH_EXPANSION_CAP: u32 = 20_000;

/// Chebyshev radius of a Harvester's work zone around the source the
/// player clicked. Seven spans the widest deliberately connected deposit
/// on the shipped 0.13 map shelf (the grand team-map center fields) while
/// a fixed anchor prevents hop-by-hop drift into another patch.
pub const HARVEST_ZONE_RADIUS: i32 = 7;

/// A radar blip only makes salvage unsafe when it is this close to a
/// candidate source. Contacts carry no identity or range, so a distant
/// blip must not retire an otherwise healthy work zone.
pub const HARVEST_RADAR_DANGER_RADIUS: i32 = 4;

/// How long an observed hit on an allied asset keeps its location unsafe
/// for autonomous salvage work. The memory contains only the ally's impact
/// tile, never the hidden attacker's identity or position.
pub const HARVEST_INCIDENT_MEMORY_TICKS: crate::Tick = 15 * crate::TICKS_PER_SECOND as crate::Tick;

/// Radius around a recent allied impact or loss that autonomous Harvest
/// treats as unsafe while the incident memory is live.
pub const HARVEST_INCIDENT_DANGER_RADIUS: i32 = 4;

/// Maximum recent allied impact sites retained per team. Incidents at one
/// tile coalesce, and the oldest expiry is evicted first at the ceiling.
pub const HARVEST_INCIDENT_CAP: usize = 64;

/// Mobile ground threats are treated as dangerous this many tiles beyond
/// their current weapon reach. It gives a visible raider's approach time
/// weight while sight is live; incident memory is the separate, deliberately
/// less informative signal that remains after sight is lost.
pub const HARVEST_MOBILE_DANGER_MARGIN: Fx = Fx::lit("3");

/// Remembered hostile emplacements are static, so their conservative
/// danger margin can stay tighter than a mobile threat's.
pub const HARVEST_STATIC_DANGER_MARGIN: Fx = Fx::lit("1");

/// When a Move command lands on an impassable tile, the goal snaps to the
/// nearest passable tile within this radius (else the command is rejected).
pub const GOAL_SNAP_RADIUS: i32 = 3;

/// How far the footprint-eviction pre-pass ring-scans for a walkable
/// escape tile. Any real escape starts on an adjacent open tile (A*
/// cannot leave a fully sealed one), so the reach only pads for
/// corner-cut geometry around the footprint.
pub const EVICT_SCAN_RADIUS: i32 = 3;

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

/// Furthest collision resolution may displace one unit in a whole tick,
/// across every relaxation pass. This stays below the visible-jolt limit
/// while leaving enough separation headroom for dense armies to flow.
pub const COLLISION_MAX_STEP: Fx = Fx::lit("0.155");

/// The slide blend for a MOVING unit's collision correction: instead
/// of a pure push along the contact normal (which a head-on pair's
/// path following exactly undoes — the measured permanent freeze at
/// 0.700 separation), a mover's correction is
/// `RADIAL_SHARE * away + LATERAL_SHARE * sideways`, the sideways
/// half picked toward the mover's own travel. Both constants are
/// exactly representable in Q32.32 and their squares sum to
/// 0.98828125 < 1, so the blended direction never exceeds unit
/// length and [`COLLISION_MAX_STEP`] keeps meaning what it says.
/// The radial share must stay well below the closing rate's half or
/// the freeze returns; the lateral share is what converts a grind
/// into a pass-by.
pub const SLIDE_RADIAL_SHARE: Fx = Fx::lit("0.5");
/// See [`SLIDE_RADIAL_SHARE`].
pub const SLIDE_LATERAL_SHARE: Fx = Fx::lit("0.859375");

/// How far from its anchor a self-acquired chase may reach before the
/// guard breaks off and walks home, in tiles. MUST stay >= the
/// Bombard's 9.5 weapon range: a shorter tether would let siege
/// pieces shell a guard that turns back before ever answering
/// (pinned by `retaliation_can_still_reach_a_bombard`).
pub const LEASH_RADIUS: Fx = Fx::lit("10");

/// The warm-blood window, in ticks (3 s): how long a self-acquired
/// chase may continue BEYOND the leash radius after the fight was
/// actually joined (a shot fired or answered — each refreshes the
/// window). Roughly 7 tiles of followthrough: enough to finish a
/// wounded runner rotating to the rear, nothing like the door to a
/// cross-map dive. A bait that never came in reach grants none, so
/// the kited guard breaks at the radius line exactly.
pub const LEASH_PATIENCE: u16 = 60;

/// Ticks a returned guard stands at its post before re-acquiring
/// (3 s). Without it, an enemy dancing at the aggro edge strips a
/// picket in an endless acquire/return cycle.
pub const LEASH_REACQUIRE_COOLDOWN: u16 = 60;

/// Ticks of standing idle before a machine counts as STATIONED — only
/// a stationed machine's self-acquired fights tether. A unit cycling
/// through idle mid-battle (its target fell, the next is a tick away)
/// re-acquires unleashed: tethering those turned army fights into
/// seat-parity coin flips (measured against the since-deleted scripted
/// tier ladder, which it collapsed).
pub const LEASH_STATION_TICKS: u16 = 40;
