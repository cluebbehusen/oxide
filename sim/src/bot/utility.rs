//! The utility policy: decision channels over an [`Observation`].
//!
//! The policy is a set
//! of independent **channels** — economy, production, construction,
//! scouting, army command — each contributing its best intents per think
//! under one shared scrap budget. Channels don't compete for a single
//! winning action (a commander harvests, trains, builds, and fights in
//! the same breath); the budget is what keeps them honest with each
//! other.
//!
//! The army channel is the anti-trickle core: fighters are drafted into
//! a staging army every think and the army is only ever committed as a
//! body, once it reaches the size the dials demand. Everything after the
//! push — contact, the withdraw call, pullbacks — belongs to the
//! [`super::Executive`].
//!
//! Deterministic given (dials, observation, executive): every selection
//! orders by an explicit key ending in an id or (y, x).

use super::difficulty::{DifficultyTuning, strategic_admission_tick};
use super::executive::{Army, ArmyState, Intent};
use super::intelligence::{BuildingContact, UnitContact};
use super::observation::{BuildingObs, Observation, UnitObs};
use super::profile::ResolvedProfile;
use super::routing::{self, RouteProjection};
use super::{PublicMapBriefing, StartingFoundry};
use crate::ids::{PlayerId, UnitId};
use crate::scenario::BotStance;
use crate::stats::{BuildingKind, Domain, UnitKind};
use chassis::grid::TilePos;
use std::collections::{BTreeSet, VecDeque};

mod combat;
mod construction;
mod danger;
mod defense;
mod economy;
mod production;
mod sensor;
mod support;
mod terrain;

pub(in crate::bot) use production::{CombatCoreStatus, combat_core_status};

/// How far from home an enemy unit counts as an intruder (Chebyshev).
const DEFENSE_RADIUS: i32 = 8;
/// Ticks between scout refreshes toward a known enemy base, and the
/// window inside which that intel still counts as fresh.
const SCOUT_REFRESH: u64 = 1800;
/// A failed solo overflight may be attempted again after two ordinary recon
/// intervals. This is long enough to prevent a replacement conveyor while
/// keeping disconnected maps strategically live.
const SOLO_SCOUT_RETRY_TICKS: u64 = SCOUT_REFRESH * 2;
/// Require a stable quiet interval before a timed retry. A transient gap
/// between hostile sightings is not evidence that another overflight differs
/// from the one which just failed.
const SOLO_SCOUT_QUIET_TICKS: u64 = SCOUT_REFRESH / 6;
/// Most turrets the policy will pay for in answer to raids.
const TURRET_CAP: usize = 2;
/// Scrap kept banked past a Fabricator's price before teching — the
/// fighting reserve that keeps the sentinel drip alive.
const TECH_RESERVE: u32 = 70;
/// A production queue this shallow accepts another order; deeper queues
/// hoard scrap in trains that cannot be redirected. The strategic air
/// and lift planners keep their own equal constants for their queues.
const SHALLOW_QUEUE_DEPTH: usize = 2;
/// Most flak turrets the policy will pay for against an air threat.
const FLAK_CAP: usize = 2;
/// Most Reclaimers the policy will run at once.
const RECLAIMER_CAP: usize = 3;
/// Ground-attack wings gathered before an air raid launches.
const AIR_WING: usize = 3;
/// How far around home the policy counts remaining salvage (Chebyshev)
/// when judging whether the patches are running dry.
const HOME_SALVAGE_RADIUS: i32 = 14;
/// Below this much known salvage near home, Reclaimers earn their keep.
const SALVAGE_LOW: u32 = 250;
/// Recurring scrap per minute the player-facing policy wants behind each
/// completed producer before adding another Reclaimer. This is an economic
/// demand signal rather than a controller-only building ceiling.
const PASSIVE_INCOME_PER_PRODUCER: u32 = 120;
/// Known anti-air within this range of a raid target scrubs the raid.
const RAID_AA_RADIUS: i32 = 6;
/// A salvage field farther than this (Chebyshev) from every own
/// Foundry counts as an unserved frontier worth an expansion.
const EXPANSION_RADIUS: i32 = 12;
/// Idle ground fighters gathered before the ferry loads a lift.
const FERRY_SQUAD: usize = 3;

fn is_air_threat(unit: &UnitObs) -> bool {
    unit.kind.stats().domain == Domain::Air && unit.kind.role() != crate::stats::Role::Scout
}

fn is_mobile_support_patient(unit: &UnitObs) -> bool {
    let stats = unit.kind.stats();
    stats.domain == Domain::Ground
        && stats.can_fight()
        && unit.hp.saturating_mul(4) < stats.max_hp.saturating_mul(3)
}
/// Persistent quarantine covers the anonymous incident's actual danger area.
/// Route projection rejects paths through it separately, so widening the
/// source radius would suppress unrelated work without adding route safety.
const CONTESTED_HARVEST_RADIUS: i32 = crate::stats::HARVEST_INCIDENT_DANGER_RADIUS;
/// Every eligible contested-region scout has enough vision to cover this
/// complete square from the exact incident tile.
const CONTESTED_RECON_RADIUS: i32 = crate::stats::HARVEST_INCIDENT_DANGER_RADIUS;
/// A recovery sweep must cover the exact danger area within one incident
/// memory window. Piecemeal sightings accumulated over a whole match are not
/// current enough to reopen a worker route.
const CONTESTED_RECON_SWEEP_TICKS: u64 = crate::stats::HARVEST_INCIDENT_MEMORY_TICKS;
/// A scout recalled by fresh danger waits before attempting the same region.
const CONTESTED_RECON_RETRY_TICKS: u64 = 3 * crate::TICKS_PER_SECOND as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContestedHarvestRegion {
    center: TilePos,
    last_evidence: u64,
    sweep_started_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HarvesterWatch {
    id: UnitId,
    tile: TilePos,
    hp: u32,
    source: Option<TilePos>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContestedRecon {
    region: TilePos,
    target: TilePos,
}

impl ContestedRecon {
    const fn at(region: TilePos) -> Self {
        Self {
            region,
            target: region,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RetreatingContestedScout {
    unit: UnitId,
    order_dispatched: bool,
}

/// Inputs retained by the profile-free Overseer's frozen shuttle channel.
struct FerryClaims<'a> {
    enlisted: &'a [UnitId],
    player_facing: bool,
}

#[derive(Clone, Copy)]
struct ConstructionClaims<'a> {
    player_facing: bool,
    enlisted: &'a [UnitId],
    reserved: &'a [UnitId],
}

/// How a spending gate treats the reserve it must leave untouched.
/// `Ordinary` applies the gate's own baseline (usually `TECH_RESERVE`);
/// `Exact(n)` replaces that baseline outright, so `Exact(0)` holds back
/// nothing at all — the two zeros are different decisions and the type
/// forces the caller to say which one it means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reserve {
    Ordinary,
    Exact(u32),
}

impl Reserve {
    /// The scrap this reserve actually withholds at a gate whose own
    /// baseline is `ordinary`.
    const fn amount(self, ordinary: u32) -> u32 {
        match self {
            Self::Ordinary => ordinary,
            Self::Exact(scrap) => scrap,
        }
    }

    const fn is_exact(self) -> bool {
        matches!(self, Self::Exact(_))
    }
}

#[derive(Clone, Copy)]
struct ConstructionContext<'a> {
    home: TilePos,
    claims: ConstructionClaims<'a>,
    combat_core_exclusions: &'a [UnitId],
    unit_contacts: Option<&'a [UnitContact]>,
    building_contacts: Option<&'a [BuildingContact]>,
    unavailable_builders: &'a [UnitId],
    public_map: Option<&'a PublicMapBriefing>,
    scope: ConstructionScope,
    voluntary_scrap_guard: Reserve,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConstructionScope {
    Full,
    OpeningCore {
        ground_emergency: bool,
        air_emergency: bool,
    },
}

impl<'a> ConstructionContext<'a> {
    const fn new(home: TilePos, claims: ConstructionClaims<'a>) -> Self {
        Self {
            home,
            claims,
            combat_core_exclusions: claims.reserved,
            unit_contacts: None,
            building_contacts: None,
            unavailable_builders: &[],
            public_map: None,
            scope: ConstructionScope::Full,
            voluntary_scrap_guard: Reserve::Ordinary,
        }
    }

    const fn with_combat_core_exclusions(mut self, exclusions: &'a [UnitId]) -> Self {
        self.combat_core_exclusions = exclusions;
        self
    }

    const fn with_intelligence(
        mut self,
        unit_contacts: Option<&'a [UnitContact]>,
        building_contacts: Option<&'a [BuildingContact]>,
    ) -> Self {
        self.unit_contacts = unit_contacts;
        self.building_contacts = building_contacts;
        self
    }

    const fn excluding_builders(mut self, unavailable_builders: &'a [UnitId]) -> Self {
        self.unavailable_builders = unavailable_builders;
        self
    }

    const fn with_public_map(mut self, public_map: Option<&'a PublicMapBriefing>) -> Self {
        self.public_map = public_map;
        self
    }

    const fn during_opening_core(mut self, ground_emergency: bool, air_emergency: bool) -> Self {
        self.scope = ConstructionScope::OpeningCore {
            ground_emergency,
            air_emergency,
        };
        self
    }

    const fn with_voluntary_scrap_guard(mut self, guard: Reserve) -> Self {
        self.voluntary_scrap_guard = guard;
        self
    }
}

#[derive(Clone, Copy)]
struct ProductionContext<'a> {
    home: TilePos,
    claims: ConstructionClaims<'a>,
    combat_core_exclusions: &'a [UnitId],
    outstanding_air_production_ticks: Option<u64>,
    unit_contacts: Option<&'a [UnitContact]>,
    building_contacts: Option<&'a [BuildingContact]>,
    public_map: Option<&'a PublicMapBriefing>,
    voluntary_scrap_guard: Reserve,
}

impl<'a> ProductionContext<'a> {
    const fn new(
        home: TilePos,
        claims: ConstructionClaims<'a>,
        outstanding_air_production_ticks: Option<u64>,
    ) -> Self {
        Self {
            home,
            claims,
            combat_core_exclusions: claims.reserved,
            outstanding_air_production_ticks,
            unit_contacts: None,
            building_contacts: None,
            public_map: None,
            voluntary_scrap_guard: Reserve::Ordinary,
        }
    }

    const fn with_combat_core_exclusions(mut self, exclusions: &'a [UnitId]) -> Self {
        self.combat_core_exclusions = exclusions;
        self
    }

    const fn with_intelligence(
        mut self,
        unit_contacts: Option<&'a [UnitContact]>,
        building_contacts: Option<&'a [BuildingContact]>,
    ) -> Self {
        self.unit_contacts = unit_contacts;
        self.building_contacts = building_contacts;
        self
    }

    const fn with_public_map(mut self, public_map: Option<&'a PublicMapBriefing>) -> Self {
        self.public_map = public_map;
        self
    }

    const fn with_voluntary_scrap_guard(mut self, guard: Reserve) -> Self {
        self.voluntary_scrap_guard = guard;
        self
    }
}

struct AdvancedConstructionContext<'a> {
    home: TilePos,
    player_facing: bool,
    builders: &'a [&'a UnitObs],
    combat_core_exclusions: &'a [UnitId],
    unit_contacts: Option<&'a [UnitContact]>,
    building_contacts: Option<&'a [BuildingContact]>,
    voluntary_scrap_guard: Reserve,
}

struct ExtractorClaimContext<'a> {
    home: TilePos,
    builders: &'a [&'a UnitObs],
    unit_contacts: Option<&'a [UnitContact]>,
    building_contacts: Option<&'a [BuildingContact]>,
}

struct FoundryClaimContext<'a> {
    home: TilePos,
    projected_foundries: &'a [TilePos],
    builders: &'a [&'a UnitObs],
    support_extractors: bool,
    ordinary_frontiers: bool,
    unit_contacts: Option<&'a [UnitContact]>,
    building_contacts: Option<&'a [BuildingContact]>,
}

#[derive(Clone, Copy)]
struct OpeningClaimContext<'a> {
    dials: &'a Dials,
    home: TilePos,
    unit_contacts: Option<&'a [UnitContact]>,
    building_contacts: Option<&'a [BuildingContact]>,
    public_map: Option<&'a PublicMapBriefing>,
}
/// Most Scuttle Charges the lane-mining arm keeps in the ground.
const MINE_CAP: usize = 3;
const ADAPTIVE_HARVESTER_BOOTSTRAP: u32 = 4;
/// How far out from home (per axis) the mining arm centers its field
/// along the approach.
const MINE_LEAN: i32 = 5;
/// Static assets the bot may liquidate when its economy is exhausted,
/// ordered from least to most strategically costly.
const SALVAGE_PRIORITY: [BuildingKind; 6] = [
    BuildingKind::Turret,
    BuildingKind::FlakTurret,
    BuildingKind::Array,
    BuildingKind::Bastion,
    BuildingKind::Reclaimer,
    BuildingKind::RepairBay,
];

/// The policy's tunable considerations. The fairness rule is that
/// dials change *thinking* — never income, vision, or combat math.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dials {
    /// Think every N ticks.
    pub cadence: u64,
    /// Harvesters eventually wanted alive or queued. Adaptive identities share
    /// a four-worker bootstrap before renewable income lets this appetite vary.
    pub harvester_target: u32,
    /// Fighters gathered before an army is committed.
    pub army_size: u32,
    /// Ordinary ground strength required before voluntary capital spending,
    /// measured in full-health Sentinel equivalents. Profile-free policies
    /// leave this at zero to preserve their existing build order.
    pub minimum_core_equivalents: u32,
    /// Ground-attack flyers gathered before an ordinary harassment sortie.
    pub air_wing: usize,
    /// Bombers kept alive or queued once the late-tech gate stands.
    pub bomber_target: usize,
    /// Mobile artillery kept alive or queued.
    pub siege_target: usize,
    /// Ceiling on Tenders kept alive or queued. One is the baseline; each
    /// additional Tender requires a distinct reachable wounded combatant.
    pub support_target: usize,
    /// Fast ground raiders kept alive or queued.
    pub raider_target: usize,
    /// Maximum ordinary defensive turrets.
    pub turret_cap: usize,
    /// Maximum anti-air emplacements.
    pub flak_cap: usize,
    /// Maximum late-economy Reclaimers.
    pub reclaimer_cap: usize,
    /// Maximum defensive minefield charges.
    pub mine_cap: usize,
    /// Maximum lane-shaping Barricades. Zero preserves the frozen QA policy's
    /// historical repertoire.
    pub barricade_cap: usize,
    /// Maximum Foundries, including the starting base.
    pub foundry_cap: usize,
    /// Use the player-facing multi-factory composition scheduler.
    pub adaptive_composition: bool,
    /// Most discretionary production candidates serviced per think.
    pub discretionary_slots: usize,
    /// Fixed difficulty estimate scale for own ground strength, in
    /// ten-thousandths. Easier rungs are deliberately conservative;
    /// personality never changes this value.
    pub own_strength_scale: u16,
    /// Estimate scale for observed hostile strength, in ten-thousandths.
    /// Player-facing rungs use the same exact hostile observation; custom and
    /// QA policies retain the dial for focused probes.
    pub enemy_strength_scale: u16,
    /// Ticks for which the largest recently observed hostile ground force
    /// remains available to strategic planning. The voluntary attack gate
    /// consumes only the shared short-lived portion of this memory.
    pub opponent_force_memory: u64,
    /// Coordinate an engaged ground army onto one legal target.
    pub coordinated_focus: bool,
    /// Coordinate overlapping static defenses onto one visible threat.
    pub coordinated_defense_focus: bool,
    /// Build a Fabricator and use the advanced roster.
    pub tech: bool,
    /// Answer harvester raids with turrets.
    pub turret_response: bool,
    /// Keep a scout sweeping the map (pointless without fog-honesty).
    pub scouting: bool,
    /// Observe through own vision instead of omnisciently.
    pub fog_honest: bool,
    /// Answer air threats: anti-air crawlers and flak turrets.
    pub aa_response: bool,
    /// Raise an Array once teched — the eyes for blips and long guns.
    pub radar: bool,
    /// Build Reclaimers when the patches near home run dry.
    pub reclaimers: bool,
    /// Weld wounded buildings instead of watching them rust.
    pub repair: bool,
    /// Fly ground-attack wings at the enemy economy.
    pub air_harass: bool,
    /// Liquidate static defense when the war outlives the economy.
    pub salvage: bool,
    /// Climb the full tree: Airworks after the Fabricator, Crucible
    /// after that, and tier-three metal once the Crucible stands.
    pub deep_tech: bool,
    /// Restore derelict Extractor frames when known and affordable.
    pub extractors: bool,
    /// Lift Reclaimers and Turrets one rung when the bank runs rich.
    pub upgrades: bool,
    /// Raise expansion Foundries toward unserved salvage frontiers.
    pub expansion: bool,
    /// Run a Skyhook shuttle at a known enemy base no ground route
    /// reaches: buy the lifter, load a squad, drop it on their shore.
    pub ferry: bool,
    /// Bury Scuttle Charges along the ground approach once raided or
    /// once the enemy's road home is known.
    pub mines: bool,
}

fn immediate_harvester_target(dials: &Dials) -> u32 {
    if dials.adaptive_composition {
        dials.harvester_target.min(ADAPTIVE_HARVESTER_BOOTSTRAP)
    } else {
        dials.harvester_target
    }
}

impl Dials {
    /// The player-facing rules-based opponent. Keep this as its own
    /// literal so later balance work can tune the opponent without
    /// changing the Overseer QA anchor.
    pub fn balanced() -> Self {
        Self {
            cadence: 8,
            harvester_target: 5,
            army_size: 5,
            minimum_core_equivalents: 0,
            air_wing: AIR_WING,
            bomber_target: 2,
            siege_target: 2,
            support_target: 1,
            raider_target: 4,
            turret_cap: TURRET_CAP,
            flak_cap: FLAK_CAP,
            reclaimer_cap: RECLAIMER_CAP,
            mine_cap: MINE_CAP,
            barricade_cap: 0,
            foundry_cap: 3,
            adaptive_composition: false,
            discretionary_slots: 1,
            own_strength_scale: 10_000,
            enemy_strength_scale: 10_000,
            opponent_force_memory: 0,
            coordinated_focus: true,
            coordinated_defense_focus: false,
            tech: true,
            turret_response: true,
            scouting: true,
            fog_honest: true,
            aa_response: true,
            radar: true,
            reclaimers: true,
            repair: true,
            air_harass: true,
            salvage: true,
            deep_tech: true,
            extractors: true,
            upgrades: true,
            expansion: true,
            ferry: true,
            mines: true,
        }
    }

    /// The core channel set used by focused policy tests. Later strategic
    /// channels stay off so each test can enable them deliberately.
    pub fn full() -> Self {
        Self {
            cadence: 8,
            harvester_target: 4,
            army_size: 5,
            minimum_core_equivalents: 0,
            air_wing: AIR_WING,
            bomber_target: 2,
            siege_target: 2,
            support_target: 1,
            raider_target: 4,
            turret_cap: TURRET_CAP,
            flak_cap: FLAK_CAP,
            reclaimer_cap: RECLAIMER_CAP,
            mine_cap: MINE_CAP,
            barricade_cap: 0,
            foundry_cap: 3,
            adaptive_composition: false,
            discretionary_slots: 1,
            own_strength_scale: 10_000,
            enemy_strength_scale: 10_000,
            opponent_force_memory: 0,
            coordinated_focus: true,
            coordinated_defense_focus: false,
            tech: true,
            turret_response: true,
            scouting: true,
            fog_honest: true,
            aa_response: true,
            radar: true,
            reclaimers: true,
            repair: true,
            air_harass: true,
            salvage: true,
            deep_tech: false,
            extractors: false,
            upgrades: false,
            expansion: false,
            ferry: false,
            mines: false,
        }
    }

    /// The stable QA controller's full strategic surface: deep tech,
    /// Extractors, upgrades, expansions, transports, and mines. Every field
    /// is spelled out because these values anchor the blessed state-hash
    /// fixtures; inheriting from another constructor would let a test-fixture
    /// edit silently redefine the frozen yardstick.
    pub fn overseer() -> Self {
        Self {
            cadence: 8,
            harvester_target: 5,
            army_size: 5,
            minimum_core_equivalents: 0,
            air_wing: AIR_WING,
            bomber_target: 2,
            siege_target: 2,
            support_target: 1,
            raider_target: 4,
            turret_cap: TURRET_CAP,
            flak_cap: FLAK_CAP,
            reclaimer_cap: RECLAIMER_CAP,
            mine_cap: MINE_CAP,
            barricade_cap: 0,
            foundry_cap: 3,
            adaptive_composition: false,
            discretionary_slots: 1,
            own_strength_scale: 10_000,
            enemy_strength_scale: 10_000,
            opponent_force_memory: 0,
            coordinated_focus: true,
            coordinated_defense_focus: false,
            tech: true,
            turret_response: true,
            scouting: true,
            fog_honest: true,
            aa_response: true,
            radar: true,
            reclaimers: true,
            repair: true,
            air_harass: true,
            salvage: true,
            deep_tech: true,
            extractors: true,
            upgrades: true,
            expansion: true,
            ferry: true,
            mines: true,
        }
    }

    /// The full legal strategy surface shaped by one player-facing identity.
    /// Trait scores redistribute priorities under a fixed budget; they never
    /// alter costs, prerequisites, information, or combat rules.
    pub fn scripted(profile: &ResolvedProfile, tuning: DifficultyTuning) -> Self {
        let traits = profile.traits;
        let stance_harvesters: u32 = match profile.stance {
            BotStance::Turtle => 6,
            BotStance::Balanced => 5,
            BotStance::Aggressive => 4,
        };
        let stance_army: u32 = match profile.stance {
            BotStance::Turtle => 7,
            BotStance::Balanced => 5,
            BotStance::Aggressive => 4,
        };
        let greed_adjustment = i32::from(traits.greed) / 25 - 2;
        let harvester_target = (stance_harvesters as i32 + greed_adjustment).clamp(4, 7) as u32;

        Self {
            cadence: tuning.cadence,
            harvester_target,
            army_size: stance_army,
            minimum_core_equivalents: tuning.minimum_core_equivalents,
            air_wing: (5usize.saturating_sub(usize::from(traits.air) / 25)).clamp(2, 4),
            bomber_target: (1 + usize::from(traits.air) / 30).clamp(1, 4),
            siege_target: 1
                + usize::from(traits.siege >= 45)
                + usize::from(traits.siege >= 60)
                + usize::from(traits.siege >= 75),
            support_target: 1
                + usize::from(traits.support >= 50)
                + usize::from(traits.support >= 65),
            // Guile changes how often a small raid forms and how jealously it
            // preserves its force, not how much combat strength it removes
            // from the ordinary army channel.
            raider_target: 2,
            turret_cap: (1 + usize::from(traits.fortification) / 25).clamp(1, 4),
            flak_cap: (1 + usize::from(traits.support) / 35).clamp(1, 3),
            reclaimer_cap: (1 + usize::from(traits.greed) / 25).clamp(1, 4),
            mine_cap: (1 + (usize::from(traits.fortification) + usize::from(traits.guile)) / 50)
                .clamp(1, 5),
            barricade_cap: usize::from(traits.fortification >= 55)
                + usize::from(traits.fortification >= 75)
                + usize::from(traits.fortification >= 90),
            foundry_cap: (1 + usize::from(traits.greed) / 25).clamp(2, 4),
            adaptive_composition: true,
            discretionary_slots: tuning.production_slots,
            own_strength_scale: tuning
                .underestimate_own(10_000)
                .try_into()
                .expect("bounded strength scale fits u16"),
            enemy_strength_scale: 10_000,
            opponent_force_memory: tuning.opponent_force_memory,
            coordinated_focus: tuning.coordinated_focus,
            coordinated_defense_focus: tuning.coordinated_defense_focus,
            ..Self::balanced()
        }
    }
}

/// Channel-based scripted policy. Its memory is bot-local and legitimate
/// (a bot is a command source, not sim state): harvest blacklists, raid
/// memory, and the scout rotation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UtilityPolicy {
    /// Exact placement-egress answers for the current known blocking layout.
    /// Construction changes far less often than the bot thinks; retaining this
    /// derived data keeps a fair full-component safety check out of the hot
    /// path without changing which placements are legal.
    ground_egress_cache: std::cell::RefCell<Option<terrain::GroundEgressCache>>,
    /// Lazily materialized, immutable worker-danger surface for the latest
    /// effective fog-honest threat layout.
    harvest_danger_cache: std::cell::RefCell<danger::HarvestDangerCache>,
    /// Largest hostile ground force observed within the difficulty's
    /// strategic memory window. Its exact old position may be stale; voluntary
    /// attack timing consumes only the common recent portion of this fact.
    opponent_force_peak: Option<(u64, u64)>,
    /// Harvest assignments from the last think: worker, node, and where
    /// the worker stood when sent. A unit idle again right after being
    /// sent AND still standing where it started bounced off an
    /// unreachable node; an idle unit that moved (or was re-tasked by
    /// the scout press mid-walk) proves nothing about the node.
    last_sent: Vec<(UnitId, TilePos, TilePos)>,
    /// Nodes that bounced a harvester back.
    dead_nodes: Vec<TilePos>,
    /// Harvester count at the last think; a drop means raiders.
    harvesters_seen: usize,
    /// Bank reading at the last think and the last tick it grew — the
    /// starvation clock behind the desperation endgame. A bank that has
    /// not grown in eighty seconds is a dead economy whatever its
    /// level: rich seats freeze too, hoarding a reserve no income will
    /// ever top up.
    bank_seen: u32,
    bank_grew_at: u64,
    desperate: bool,
    /// Under desperation, two different route questions about home's
    /// mirror — the blind guess at the enemy base a symmetric quarry
    /// offers. `desperate_march` is the optimistic terrain preflight;
    /// the player-facing army gate separately requires an explored route
    /// before issuing the march. Liquidating the capital fund uses the
    /// explored-route check in `desperate_road` directly. The optimistic
    /// preflight would treat any unexplored gulf as passable forever, and a
    /// seat that releases its savings on that hope buys infantry against a
    /// strait until the map dies. No known road means island war — protect
    /// the fund and climb to the sky.
    desperate_march: bool,
    desperate_road: bool,
    /// Set when a harvester died on this watch. The player-facing controller
    /// keeps the latch until its configured Turret line is actually built;
    /// the profile-free QA controller retains its legacy one-site reset.
    raided: bool,
    /// Turret-site count at the last profile-free think.
    turrets_seen: usize,
    /// Build commands dispatched last think, by anchor — one that never
    /// appeared was rejected by ground truth the observation lacks
    /// (an unseen unit in the footprint, say); blacklist the anchor.
    pending_sites: Vec<TilePos>,
    /// Anchors the sim refused.
    dead_anchors: Vec<TilePos>,
    /// The designated scout, held only mid-sweep (released between
    /// sweeps so the draft can have it back).
    scout: Option<UnitId>,
    /// Which leg of the search sweep the scout is on.
    scout_leg: u32,
    /// The last scout order: unit, starting tile, and destination. An
    /// idle ground unit still at the start is direct no-route testimony.
    scout_dispatch: Option<(UnitId, TilePos, TilePos)>,
    /// A ground unit currently probing an authored hostile start. If dynamic
    /// danger recalls this unit, the same public prior must not draft another
    /// Harvester; reconnaissance escalates to a dedicated flyer instead.
    public_start_ground_scout: Option<UnitId>,
    /// Current authored-start reconnaissance cannot use a ground route. This
    /// is recomputed from public terrain and current resource knowledge rather
    /// than retained as evidence that the route is permanently severed.
    public_start_air_scout_needed: bool,
    /// Current contested reconnaissance has no eligible ordinary body. This is
    /// recomputed as the roster changes; it does not prove a ground route failed.
    contested_recon_air_scout_needed: bool,
    /// A dispatched ground look failed or became unsafe, proving that its next
    /// attempt needs an aircraft. This durable evidence remains independent of
    /// the recomputable authored-start prior above.
    persistent_air_scout_needed: bool,
    /// A dispatched dedicated scout died before completing its solo look.
    /// Do not fund the same suicide conveyor until genuinely current enemy
    /// sight changes the information state; remembered ghosts are not new
    /// evidence.
    solo_air_scout_suspended: bool,
    /// First tick of uninterrupted absence of actionable enemy sight after a
    /// solo scout loss.
    solo_air_scout_dark_since: Option<u64>,
    /// Earliest tick a quiet map may fund one more solo overflight.
    solo_air_scout_retry_at: u64,
    /// Tick when a scout was last sent toward a known enemy base. Dispatch
    /// cadence is not evidence that the destination was actually observed.
    scout_sent_at: u64,
    /// Tick of the last confirmed current sight of an enemy Foundry.
    scouted_at: u64,
    /// Authored hostile starts whose complete Foundry footprint has since been
    /// seen empty. These are retired public priors, not destroyed-building
    /// intelligence; live and remembered contacts remain in the observation.
    cleared_hostile_starts: Vec<PlayerId>,
    /// Whether enemy air has ever been sighted — the sky stays suspect
    /// afterward.
    seen_air: bool,
    /// Riders sent to board the profile-free Overseer's ferry on its last
    /// Load. The player-facing controller owns transport waves in its
    /// persistent strategic planner instead.
    ferry_boarding: Vec<UnitId>,
    /// Player-facing controller memory for work regions where allied losses
    /// tied to a current or last-observed worker made anonymous salvage unsafe.
    /// Elapsed time alone never proves a mobile threat left, so only fresh
    /// clear sight releases a region.
    contested_harvest_regions: Vec<ContestedHarvestRegion>,
    /// Exact in-bounds danger-area cells observed during a clean recovery
    /// sweep, keyed by their canonical region center. Coverage accumulates
    /// only while no current or remembered danger intersects the region.
    contested_harvest_clear_tiles: BTreeSet<(TilePos, TilePos)>,
    /// Current and previous worker positions associate anonymous allied-loss
    /// incidents with a real harvest route without revealing the attacker.
    harvester_watch: Vec<HarvesterWatch>,
    /// Region centers whose warning or fog-honest threat projection currently
    /// makes reconnaissance itself unsafe.
    contested_recon_blocked: BTreeSet<TilePos>,
    /// Scout currently assigned to a specific recovery region.
    contested_scout: Option<(UnitId, TilePos)>,
    /// A recovery scout recalled by fresh danger remains protected from every
    /// other channel until it is observed back in the safe home area.
    retreating_contested_scout: Option<RetreatingContestedScout>,
    /// Earliest tick after a failed recovery may be attempted again. A
    /// recalled survivor starts this cooldown only after reaching home.
    contested_recon_retry_at: u64,
    /// Workers already sent out of a contested work region. This avoids
    /// replacing the same escape route every think while still retrying a
    /// bounced evacuation once the unit becomes idle.
    evacuating_workers: Vec<UnitId>,
}

struct ThinkContext<'a> {
    armies: &'a [Army],
    enlisted: &'a [UnitId],
    reserved: &'a [UnitId],
    combat_core_exclusions: &'a [UnitId],
    outstanding_air_production_ticks: Option<u64>,
    prelude: Vec<Intent>,
    mode: PolicyMode<'a>,
}

#[derive(Clone, Copy)]
struct PolicyMode<'a> {
    player_facing: bool,
    admit_voluntary_macro: bool,
    unit_contacts: Option<&'a [UnitContact]>,
    building_contacts: Option<&'a [BuildingContact]>,
    /// Immutable authored priors. Never reinterpret these as current contacts.
    public_map: Option<&'a PublicMapBriefing>,
}

pub(super) struct StrategicUtilityContext<'a> {
    reserved: &'a [UnitId],
    combat_core_exclusions: &'a [UnitId],
    unit_contacts: &'a [UnitContact],
    building_contacts: &'a [BuildingContact],
    public_map: &'a PublicMapBriefing,
    outstanding_air_production_ticks: Option<u64>,
    prelude: Vec<Intent>,
}

impl<'a> StrategicUtilityContext<'a> {
    pub(super) fn new(
        reserved: &'a [UnitId],
        unit_contacts: &'a [UnitContact],
        building_contacts: &'a [BuildingContact],
        public_map: &'a PublicMapBriefing,
        prelude: Vec<Intent>,
    ) -> Self {
        Self {
            reserved,
            combat_core_exclusions: reserved,
            unit_contacts,
            building_contacts,
            public_map,
            outstanding_air_production_ticks: None,
            prelude,
        }
    }

    pub(super) const fn with_combat_core_exclusions(mut self, exclusions: &'a [UnitId]) -> Self {
        self.combat_core_exclusions = exclusions;
        self
    }

    /// Supplies the work still owed by one active, justified strategic air
    /// plan. The utility layer uses this only to buy ordinary production
    /// capacity; `None` keeps speculative or inactive plans from raising
    /// factories on their own.
    pub(super) fn with_outstanding_air_production_ticks(mut self, ticks: u64) -> Self {
        self.outstanding_air_production_ticks = Some(ticks);
        self
    }
}

impl UtilityPolicy {
    /// Fresh policy, no memory.
    pub fn new() -> Self {
        Self::default()
    }

    fn air_scout_needed(&self) -> bool {
        self.public_start_air_scout_needed
            || self.contested_recon_air_scout_needed
            || self.persistent_air_scout_needed
    }

    /// Public hostile starts that have not received current negative evidence.
    ///
    /// The briefing remains immutable. Controller-local evidence suppresses a
    /// stale recon prior without manufacturing a dynamic enemy contact.
    pub(super) fn uncleared_hostile_starts(
        &self,
        public_map: &PublicMapBriefing,
        me: PlayerId,
    ) -> Vec<StartingFoundry> {
        public_map
            .hostile_starting_foundries(me)
            .filter(|start| {
                self.cleared_hostile_starts
                    .binary_search(&start.player)
                    .is_err()
            })
            .copied()
            .collect()
    }

    fn has_honest_ground_objective(
        &self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        public_map: Option<&PublicMapBriefing>,
    ) -> bool {
        if self.ordinary_ground_has_work(dials, obs, home) {
            return true;
        }
        let Some(briefing) = public_map else {
            return false;
        };
        let reaches =
            |anchor, size| Self::public_ground_terrain_reaches(briefing, obs, home, anchor, size);
        self.uncleared_hostile_starts(briefing, obs.me)
            .into_iter()
            .any(|start| reaches(start.anchor, BuildingKind::Foundry.base_stats().size))
            || obs.enemy_buildings.iter().any(|building| {
                building.hp > 0
                    && reaches(
                        building.anchor,
                        building.kind.tier_stats(building.tier).size,
                    )
            })
            || obs.enemy_units.iter().any(|unit| {
                unit.hp > 0
                    && unit.kind.stats().domain == Domain::Ground
                    && reaches(unit.tile, (1, 1))
            })
    }

    fn public_ground_terrain_reaches(
        public_map: &PublicMapBriefing,
        obs: &Observation,
        home: TilePos,
        target: TilePos,
        target_size: (i32, i32),
    ) -> bool {
        let (width, height) = (public_map.map_width(), public_map.map_height());
        if width != obs.map_width
            || height != obs.map_height
            || width <= 0
            || height <= 0
            || target_size.0 <= 0
            || target_size.1 <= 0
        {
            return false;
        }
        let open = |tile: TilePos| {
            public_map
                .terrain_at(tile)
                .is_some_and(|terrain| !terrain.blocks_ground())
        };
        if !open(home) {
            return false;
        }
        let goal = |tile: TilePos| {
            tile.x >= target.x
                && tile.x < target.x + target_size.0
                && tile.y >= target.y
                && tile.y < target.y + target_size.1
        };
        let cells = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let mut visited = vec![false; cells];
        let index = |tile: TilePos| usize::try_from(tile.y * width + tile.x).ok();
        let Some(home_index) = index(home).filter(|index| *index < visited.len()) else {
            return false;
        };
        visited[home_index] = true;
        let mut frontier = VecDeque::from([home]);
        while let Some(tile) = frontier.pop_front() {
            if goal(tile) {
                return true;
            }
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let next = tile.offset(dx, dy);
                let Some(next_index) = index(next).filter(|index| *index < visited.len()) else {
                    continue;
                };
                if !visited[next_index] && open(next) {
                    visited[next_index] = true;
                    frontier.push_back(next);
                }
            }
        }
        false
    }

    fn shallow_sentinel_reinforcement(obs: &Observation, intents: &[Intent]) -> bool {
        obs.my_buildings
            .iter()
            .enumerate()
            .filter(|(_, building)| building.built && building.kind == BuildingKind::Foundry)
            .any(|(queue_index, building)| {
                let queue = obs
                    .my_queues
                    .get(queue_index)
                    .map_or(&[][..], Vec::as_slice);
                // Production can finish the front item before a deferred
                // founder pays later in the same tick. Only a Sentinel behind
                // that item is guaranteed to remain queued across the phase.
                if queue
                    .iter()
                    .skip(1)
                    .take(1)
                    .any(|kind| *kind == UnitKind::Sentinel)
                {
                    return true;
                }
                let remaining = 2usize.saturating_sub(queue.len().min(2));
                intents
                    .iter()
                    .filter_map(|intent| match intent {
                        Intent::TrainAt {
                            building: producer,
                            kind,
                        } if *producer == building.id => Some(*kind),
                        _ => None,
                    })
                    .take(remaining)
                    .any(|kind| kind == UnitKind::Sentinel)
            })
    }

    fn unpaid_deferred_construction(obs: &Observation, intents: &[Intent]) -> bool {
        intents.iter().any(|intent| match intent {
            Intent::Build { kind, anchor } | Intent::BuildWith { kind, anchor, .. } => {
                let already_paid = obs.my_buildings.iter().any(|building| {
                    building.kind == *kind
                        && building.anchor == *anchor
                        && !building.built
                        && building.tier == 0
                });
                let (width, height) = kind.base_stats().size;
                !already_paid
                    && (0..height)
                        .any(|dy| (0..width).any(|dx| !obs.visible(anchor.offset(dx, dy))))
            }
            _ => false,
        })
    }

    pub(super) fn shallow_sentinel_capital_reserve(
        &self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        public_map: &PublicMapBriefing,
    ) -> u32 {
        if dials.minimum_core_equivalents == 0
            || !self.has_honest_ground_objective(dials, obs, home, Some(public_map))
            || Self::shallow_sentinel_reinforcement(obs, &[])
        {
            return 0;
        }
        UnitKind::Sentinel.stats().cost
    }

    pub(super) fn strategic_opening_bootstrap_reserve(
        &self,
        dials: &Dials,
        obs: &Observation,
        home: TilePos,
        public_map: &PublicMapBriefing,
    ) -> u32 {
        if dials.minimum_core_equivalents == 0 {
            return 0;
        }
        self.opening_bootstrap_reserve(
            dials,
            obs,
            ConstructionContext::new(
                home,
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
            )
            .with_public_map(Some(public_map)),
            &[],
        )
    }

    fn maintain_shallow_sentinel_reinforcement(
        obs: &Observation,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) -> bool {
        if Self::shallow_sentinel_reinforcement(obs, intents) {
            return true;
        }
        let sentinel_cost = UnitKind::Sentinel.stats().cost;
        if *budget < sentinel_cost {
            return false;
        }
        let producer = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, building)| building.built && building.kind == BuildingKind::Foundry)
            .filter_map(|(queue_index, building)| {
                let depth = obs
                    .my_queues
                    .get(queue_index)
                    .map_or(2, Vec::len)
                    .saturating_add(production::planned_at(intents, building.id));
                (depth < 2).then_some((depth, building.id))
            })
            .min();
        let Some((_, building)) = producer else {
            return false;
        };

        *budget -= sentinel_cost;
        let before_capital = intents
            .iter()
            .position(|intent| {
                matches!(
                    intent,
                    Intent::Build { .. } | Intent::BuildWith { .. } | Intent::Upgrade { .. }
                )
            })
            .unwrap_or(intents.len());
        intents.insert(
            before_capital,
            Intent::TrainAt {
                building,
                kind: UnitKind::Sentinel,
            },
        );
        true
    }

    fn construction_channel_spent(intents: &[Intent]) -> bool {
        intents.iter().any(|intent| {
            matches!(
                intent,
                Intent::Build { .. }
                    | Intent::BuildWith { .. }
                    | Intent::CancelSite { .. }
                    | Intent::Upgrade { .. }
            )
        })
    }

    /// Workers whose active escape must outrank every implicit utility claim.
    pub(super) fn worker_safety_reservations(&self) -> &[UnitId] {
        &self.evacuating_workers
    }

    /// Replaces player-facing implicit Build intents with one exact worker
    /// whose ordinary command route stays outside current and remembered
    /// worker danger.
    ///
    /// The simulation does not know the controller's fog-honest incident
    /// memory, so it cannot route around that memory on its own. Binding here
    /// also prevents the next think from evacuating a founder that the prior
    /// think just sent through the same quarantined region.
    pub(super) fn bind_player_facing_builders(
        &self,
        obs: &Observation,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        enlisted: &[UnitId],
        reserved: &[UnitId],
        intents: &mut Vec<Intent>,
    ) {
        let original = std::mem::take(intents);
        let mut claimed = Vec::new();
        for intent in &original {
            Self::claim_non_preemptible_intent_units(intent, &mut claimed);
        }
        let danger = original
            .iter()
            .any(|intent| matches!(intent, Intent::Build { .. }))
            .then(|| {
                self.harvest_danger_projection(obs, Some(unit_contacts), Some(building_contacts))
            });
        if original
            .iter()
            .any(|intent| matches!(intent, Intent::Build { .. } | Intent::BuildWith { .. }))
        {
            self.prepare_ground_producer_egress(obs);
        }
        let mut bound = Vec::with_capacity(original.len());
        let mut accepted_builds = Vec::new();
        for intent in original {
            match intent {
                Intent::Build { kind, anchor } => {
                    if !self
                        .preserves_ground_producer_egress_prepared(&accepted_builds, (kind, anchor))
                    {
                        continue;
                    }
                    let mut candidates: Vec<_> = obs
                        .my_units
                        .iter()
                        .filter(|unit| {
                            unit.kind.stats().harvest.is_some()
                                && unit.site.is_none()
                                && unit.founding.is_none()
                                && !enlisted.contains(&unit.id)
                                && !reserved.contains(&unit.id)
                                && !claimed.contains(&unit.id)
                                && self.scout != Some(unit.id)
                        })
                        .collect();
                    if let Some(builder) = self.safe_implicit_builder(
                        obs,
                        kind,
                        anchor,
                        &mut candidates,
                        danger
                            .as_deref()
                            .expect("an implicit Build prepared worker danger"),
                        None,
                    ) {
                        claimed.push(builder);
                        accepted_builds.push((kind, anchor));
                        Self::insert_build_before_harvest(
                            &mut bound,
                            kind,
                            anchor,
                            Intent::BuildWith {
                                builder,
                                kind,
                                anchor,
                            },
                        );
                    }
                }
                intent @ Intent::BuildWith { kind, anchor, .. } => {
                    if self
                        .preserves_ground_producer_egress_prepared(&accepted_builds, (kind, anchor))
                    {
                        accepted_builds.push((kind, anchor));
                        Self::insert_build_before_harvest(&mut bound, kind, anchor, intent);
                    }
                }
                Intent::AssignHarvest { node, .. }
                    if accepted_builds
                        .iter()
                        .any(|(kind, anchor)| Self::footprint_contains(*kind, *anchor, node)) => {}
                intent => bound.push(intent),
            }
        }
        *intents = bound;
    }

    fn safe_implicit_builder(
        &self,
        obs: &Observation,
        kind: BuildingKind,
        anchor: TilePos,
        candidates: &mut [&UnitObs],
        danger: &danger::HarvestDangerProjection,
        public_map: Option<&PublicMapBriefing>,
    ) -> Option<UnitId> {
        let size = kind.base_stats().size;
        let (width, height) = size;
        let defer = (0..height).any(|dy| (0..width).any(|dx| !obs.visible(anchor.offset(dx, dy))));
        candidates.sort_unstable_by_key(|unit| (unit.tile.manhattan(anchor), unit.id));
        candidates
            .iter()
            .copied()
            .find(|unit| match public_map {
                Some(public_map) => {
                    crate::bot::routing::build_command_path_avoids_with_public_terrain(
                        obs,
                        public_map,
                        unit,
                        anchor,
                        size,
                        defer,
                        |tile| self.harvest_location_contested(tile) || danger.contains(tile),
                    )
                }
                None => crate::bot::routing::build_command_path_avoids(
                    obs,
                    unit,
                    anchor,
                    size,
                    defer,
                    |tile| self.harvest_location_contested(tile) || danger.contains(tile),
                ),
            })
            .map(|unit| unit.id)
    }

    fn insert_build_before_harvest(
        intents: &mut Vec<Intent>,
        kind: BuildingKind,
        anchor: TilePos,
        build: Intent,
    ) {
        intents.retain(|intent| {
            !matches!(
                intent,
                Intent::AssignHarvest { node, .. }
                    if Self::footprint_contains(kind, anchor, *node)
            )
        });
        let before_harvest = intents
            .iter()
            .position(|intent| matches!(intent, Intent::AssignHarvest { .. }))
            .unwrap_or(intents.len());
        intents.insert(before_harvest, build);
    }

    fn footprint_contains(kind: BuildingKind, anchor: TilePos, tile: TilePos) -> bool {
        let (width, height) = kind.base_stats().size;
        tile.x >= anchor.x
            && tile.x < anchor.x + width
            && tile.y >= anchor.y
            && tile.y < anchor.y + height
    }

    fn claim_non_preemptible_intent_units(intent: &Intent, claimed: &mut Vec<UnitId>) {
        match intent {
            Intent::MoveUnits { units, .. }
            | Intent::AttackMoveUnits { units, .. }
            | Intent::AttackUnits { units, .. }
            | Intent::StopUnits { units } => claimed.extend(units.iter().copied()),
            Intent::RepairUnits { welders, .. } => claimed.extend(welders.iter().copied()),
            Intent::Scout { unit, .. } => claimed.push(*unit),
            Intent::BuildWith { builder, .. } => claimed.push(*builder),
            Intent::Load { transport, riders } => {
                claimed.push(*transport);
                claimed.extend(riders.iter().copied());
            }
            Intent::Unload { transport, .. } => claimed.push(*transport),
            Intent::TrainAt { .. }
            | Intent::Build { .. }
            | Intent::CancelSite { .. }
            | Intent::AssignHarvest { .. }
            | Intent::FormArmy { .. }
            | Intent::PushArmy { .. }
            | Intent::Repair { .. }
            | Intent::Salvage { .. }
            | Intent::RaidAir { .. }
            | Intent::Upgrade { .. } => {}
        }
        claimed.sort_unstable();
        claimed.dedup();
    }

    /// Distinct deferred construction promises that have not become paid
    /// sites yet. Several founders may share one logical claim.
    fn deferred_claims(obs: &Observation) -> Vec<(BuildingKind, TilePos)> {
        let mut claims: Vec<(BuildingKind, TilePos)> = obs
            .my_units
            .iter()
            .filter_map(|unit| unit.founding)
            .filter(|(kind, anchor)| {
                !obs.my_buildings
                    .iter()
                    .any(|building| building.kind == *kind && building.anchor == *anchor)
            })
            .collect();
        claims.sort_unstable();
        claims.dedup();
        claims
    }

    fn deferred_claims_commitment(claims: &[(BuildingKind, TilePos)]) -> u32 {
        claims
            .iter()
            .map(|(kind, _)| {
                kind.base_stats()
                    .construction
                    .map_or(0, |construction| construction.cost)
            })
            .fold(0, u32::saturating_add)
    }

    fn deferred_claim_has_safe_founder(
        &self,
        obs: &Observation,
        claim: (BuildingKind, TilePos),
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
        public_map: &PublicMapBriefing,
    ) -> bool {
        let (kind, anchor) = claim;
        let mut founders: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.founding == Some(claim))
            .collect();
        if founders.is_empty() {
            return false;
        }
        let danger = self.harvest_danger_projection(obs, unit_contacts, building_contacts);
        self.safe_implicit_builder(obs, kind, anchor, &mut founders, &danger, Some(public_map))
            .is_some()
    }

    fn safe_starting_home_extractor_claim(
        &self,
        obs: &Observation,
        home: TilePos,
        claim: (BuildingKind, TilePos),
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
        public_map: &PublicMapBriefing,
    ) -> bool {
        let (kind, anchor) = claim;
        let (frame_width, frame_height) = BuildingKind::Extractor.base_stats().size;
        let frame_is_occupied = obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .chain(obs.enemy_buildings.iter())
            .any(|building| {
                let (width, height) = building.kind.tier_stats(building.tier).size;
                building.hp > 0
                    && anchor.x < building.anchor.x + width
                    && anchor.x + frame_width > building.anchor.x
                    && anchor.y < building.anchor.y + height
                    && anchor.y + frame_height > building.anchor.y
            });
        if kind != BuildingKind::Extractor
            || !public_map.extractor_frames().contains(&anchor)
            || !self.player_can_plan_frame_restoration(obs, anchor)
            || !Self::ground_route_known(obs, home, anchor)
            || frame_is_occupied
        {
            return false;
        }
        let Some(starting_home) = public_map
            .starting_foundries()
            .iter()
            .find(|start| start.player == obs.me)
            .map(|start| start.anchor)
        else {
            return false;
        };
        starting_home == home
            && Self::foundry_supports_extractor(starting_home, anchor)
            && obs.my_buildings.iter().any(|building| {
                building.kind == BuildingKind::Foundry
                    && building.anchor == starting_home
                    && building.built
                    && building.hp > 0
            })
            && self.deferred_claim_has_safe_founder(
                obs,
                claim,
                unit_contacts,
                building_contacts,
                public_map,
            )
    }

    fn opening_core_deferred_claims(
        &self,
        obs: &Observation,
        context: OpeningClaimContext<'_>,
        intents: &mut Vec<Intent>,
    ) -> Vec<(BuildingKind, TilePos)> {
        let OpeningClaimContext {
            dials,
            home,
            unit_contacts,
            building_contacts,
            public_map,
        } = context;
        let claims = Self::deferred_claims(obs);
        let Some(public_map) = public_map else {
            let founders: Vec<_> = obs
                .my_units
                .iter()
                .filter(|unit| unit.founding.is_some())
                .map(|unit| unit.id)
                .collect();
            if !founders.is_empty() {
                intents.insert(0, Intent::StopUnits { units: founders });
            }
            return Vec::new();
        };

        let ground_emergency = dials.turret_response
            && self.current_emergency_defense_required(BuildingKind::Turret, obs, public_map);
        let air_emergency = dials.aa_response
            && self.current_emergency_defense_required(BuildingKind::FlakTurret, obs, public_map);
        let mut kept_home_extractor = false;
        let mut kept_ground_emergency = false;
        let mut kept_air_emergency = false;
        let mut retained = Vec::new();
        for claim @ (kind, _) in claims {
            let keep = match kind {
                BuildingKind::Extractor if !kept_home_extractor => {
                    let safe = self.safe_starting_home_extractor_claim(
                        obs,
                        home,
                        claim,
                        unit_contacts,
                        building_contacts,
                        public_map,
                    );
                    kept_home_extractor = safe;
                    safe
                }
                BuildingKind::Turret if ground_emergency && !kept_ground_emergency => {
                    let safe = self
                        .current_emergency_defense_claim_is_useful(kind, claim.1, obs, public_map)
                        && self.deferred_claim_has_safe_founder(
                            obs,
                            claim,
                            unit_contacts,
                            building_contacts,
                            public_map,
                        );
                    kept_ground_emergency = safe;
                    safe
                }
                BuildingKind::FlakTurret if air_emergency && !kept_air_emergency => {
                    let safe = self
                        .current_emergency_defense_claim_is_useful(kind, claim.1, obs, public_map)
                        && self.deferred_claim_has_safe_founder(
                            obs,
                            claim,
                            unit_contacts,
                            building_contacts,
                            public_map,
                        );
                    kept_air_emergency = safe;
                    safe
                }
                _ => false,
            };
            if keep {
                retained.push(claim);
            }
        }

        let stopped: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| {
                unit.founding
                    .is_some_and(|claim| !retained.contains(&claim))
            })
            .map(|unit| unit.id)
            .collect();
        if !stopped.is_empty() {
            intents.insert(0, Intent::StopUnits { units: stopped });
        }
        retained
    }

    fn post_floor_deferred_claims(
        &self,
        obs: &Observation,
        home: TilePos,
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
        public_map: Option<&PublicMapBriefing>,
        intents: &mut Vec<Intent>,
    ) -> Vec<(BuildingKind, TilePos)> {
        let claims = Self::deferred_claims(obs);
        let mut exceptions = Vec::new();
        let mut voluntary = Vec::new();
        let mut kept_home_extractor = false;
        let mut kept_ground_emergency = false;
        let mut kept_air_emergency = false;
        for claim @ (kind, anchor) in claims {
            let exception = public_map.is_some_and(|briefing| match kind {
                BuildingKind::Extractor if !kept_home_extractor => {
                    let keep = self.safe_starting_home_extractor_claim(
                        obs,
                        home,
                        claim,
                        unit_contacts,
                        building_contacts,
                        briefing,
                    );
                    kept_home_extractor = keep;
                    keep
                }
                BuildingKind::Turret if !kept_ground_emergency => {
                    let keep = self
                        .current_emergency_defense_claim_is_useful(kind, anchor, obs, briefing)
                        && self.deferred_claim_has_safe_founder(
                            obs,
                            claim,
                            unit_contacts,
                            building_contacts,
                            briefing,
                        );
                    kept_ground_emergency = keep;
                    keep
                }
                BuildingKind::FlakTurret if !kept_air_emergency => {
                    let keep = self
                        .current_emergency_defense_claim_is_useful(kind, anchor, obs, briefing)
                        && self.deferred_claim_has_safe_founder(
                            obs,
                            claim,
                            unit_contacts,
                            building_contacts,
                            briefing,
                        );
                    kept_air_emergency = keep;
                    keep
                }
                _ => false,
            });
            if exception {
                exceptions.push(claim);
            } else {
                voluntary.push(claim);
            }
        }

        let claim_cost = |kind: BuildingKind| {
            kind.base_stats()
                .construction
                .map_or(0, |construction| construction.cost)
        };
        let mut available = exceptions.iter().fold(obs.scrap, |scrap, (kind, _)| {
            scrap.saturating_sub(claim_cost(*kind))
        });
        let sentinel_reserve = UnitKind::Sentinel.stats().cost;
        let mut retained = exceptions;
        for claim @ (kind, _) in voluntary {
            let cost = claim_cost(kind);
            if available >= cost.saturating_add(sentinel_reserve) {
                available -= cost;
                retained.push(claim);
            }
        }
        retained.sort_unstable();

        let stopped: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| {
                unit.founding
                    .is_some_and(|claim| !retained.contains(&claim))
            })
            .map(|unit| unit.id)
            .collect();
        if !stopped.is_empty() {
            intents.insert(0, Intent::StopUnits { units: stopped });
        }
        retained
    }

    /// Foundry anchors already paid for or promised by deferred founders,
    /// plus the number of promises whose cost is still outstanding.
    fn projected_foundries(obs: &Observation) -> (Vec<TilePos>, usize) {
        let mut anchors: Vec<TilePos> = obs
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .map(|building| building.anchor)
            .collect();
        let pending: Vec<TilePos> = Self::deferred_claims(obs)
            .into_iter()
            .filter_map(|(kind, anchor)| (kind == BuildingKind::Foundry).then_some(anchor))
            .collect();
        let outstanding = pending.len();
        anchors.extend(pending);
        anchors.sort_unstable();
        anchors.dedup();
        (anchors, outstanding)
    }

    /// Scrap promised to deferred construction but not charged until the
    /// founders reach their destinations.
    pub(crate) fn deferred_construction_commitment(obs: &Observation) -> u32 {
        Self::deferred_claims_commitment(&Self::deferred_claims(obs))
    }

    fn projected_count(obs: &Observation, kind: BuildingKind, player_facing: bool) -> usize {
        let standing = obs
            .my_buildings
            .iter()
            .filter(|building| building.kind == kind)
            .count();
        if !player_facing {
            return standing;
        }
        standing
            + Self::deferred_claims(obs)
                .iter()
                .filter(|(pending, _)| *pending == kind)
                .count()
    }

    /// One think: intents for this observation, in lowering order.
    /// `armies` and `enlisted` are the executive's bookkeeping,
    /// pre-oriented by the caller when the policy thinks in flipped space.
    pub fn think(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        enlisted: &[UnitId],
    ) -> Vec<Intent> {
        self.think_inner(
            dials,
            obs,
            ThinkContext {
                armies,
                enlisted,
                reserved: &[],
                combat_core_exclusions: &[],
                outstanding_air_production_ticks: None,
                prelude: Vec::new(),
                mode: PolicyMode {
                    player_facing: false,
                    admit_voluntary_macro: true,
                    unit_contacts: None,
                    building_contacts: None,
                    public_map: None,
                },
            },
        )
    }

    /// One player-facing utility think without controller-level strategic
    /// intelligence or higher-level planner intents.
    pub fn think_player_facing(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        enlisted: &[UnitId],
        reserved: &[UnitId],
        public_map: &PublicMapBriefing,
    ) -> Vec<Intent> {
        self.think_inner(
            dials,
            obs,
            ThinkContext {
                armies,
                enlisted,
                reserved,
                combat_core_exclusions: reserved,
                outstanding_air_production_ticks: None,
                prelude: Vec::new(),
                mode: PolicyMode {
                    player_facing: true,
                    admit_voluntary_macro: strategic_admission_tick(obs.tick),
                    unit_contacts: None,
                    building_contacts: None,
                    public_map: Some(public_map),
                },
            },
        )
    }

    /// The maintained player-facing path, including confidence-bearing
    /// strategic memory for remembered defenses.
    pub(super) fn think_with_intelligence(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        enlisted: &[UnitId],
        context: StrategicUtilityContext<'_>,
    ) -> Vec<Intent> {
        self.think_inner(
            dials,
            obs,
            ThinkContext {
                armies,
                enlisted,
                reserved: context.reserved,
                combat_core_exclusions: context.combat_core_exclusions,
                outstanding_air_production_ticks: context.outstanding_air_production_ticks,
                prelude: context.prelude,
                mode: PolicyMode {
                    player_facing: true,
                    admit_voluntary_macro: strategic_admission_tick(obs.tick),
                    unit_contacts: Some(context.unit_contacts),
                    building_contacts: Some(context.building_contacts),
                    public_map: Some(context.public_map),
                },
            },
        )
    }

    fn think_inner(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        context: ThinkContext<'_>,
    ) -> Vec<Intent> {
        let ThinkContext {
            armies,
            enlisted,
            reserved,
            combat_core_exclusions,
            outstanding_air_production_ticks,
            prelude,
            mode,
        } = context;
        let player_facing = mode.player_facing;
        let strategic_capital_advanced = player_facing
            && prelude.iter().any(|intent| {
                matches!(
                    intent,
                    Intent::TrainAt { .. }
                        | Intent::Build { .. }
                        | Intent::BuildWith { .. }
                        | Intent::Upgrade { .. }
                )
            });
        let mut intents = prelude;
        let Some(home) = obs
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Foundry)
            .min_by_key(|building| (!building.built, building.id))
        else {
            return intents; // eliminated: nothing left to decide
        };
        let home_tile = home.anchor;
        let mirror_site = TilePos::new(
            obs.map_width - 1 - home_tile.x,
            obs.map_height - 1 - home_tile.y,
        );
        if obs.enemy_units.iter().any(is_air_threat) {
            self.seen_air = true;
        }

        if player_facing {
            if let Some(public_map) = mode.public_map {
                self.clear_visible_public_starts(obs, public_map);
            }
            self.refresh_contested_harvest_regions(obs, mode.unit_contacts, mode.building_contacts);
            self.retreat_contested_scout(obs, home_tile, &mut intents);
            self.evacuate_contested_workers(
                obs,
                home_tile,
                mode.unit_contacts,
                mode.building_contacts,
                &mut intents,
            );
        }
        let mut protected = reserved.to_vec();
        if player_facing {
            protected.extend(self.evacuating_workers.iter().copied());
            protected.extend(self.retreating_contested_scout.map(|retreat| retreat.unit));
        }
        protected.sort_unstable();
        protected.dedup();
        let reserved = protected.as_slice();

        // Higher rungs may observe and answer current danger on every authored
        // cadence, but extra observations must not repeatedly resample or
        // advance voluntary macro ledgers. New harvesting, scouting,
        // production, construction, paid sustain, salvage, and ordinary
        // offensive commitments share one admission snapshot across every
        // player-facing difficulty.
        if !mode.admit_voluntary_macro {
            self.army(dials, obs, armies, home_tile, mode, &mut intents);
            return intents;
        }

        if obs.scrap > self.bank_seen || obs.tick == 0 {
            self.bank_grew_at = obs.tick;
        }
        self.bank_seen = obs.scrap;
        // The clock must undercut the liveness gate's stall patience
        // (roughly two thousand ticks): desperation is the designed
        // answer to an economic freeze, so it has to fire before the
        // freeze detector calls the game dead between pushes.
        self.desperate = obs.tick.saturating_sub(self.bank_grew_at) > 1_600;
        if self.desperate {
            self.desperate_march = Self::ground_reaches(obs, home_tile, mirror_site);
            self.desperate_road = Self::ground_route_known(obs, home_tile, mirror_site);
        }
        self.audit_harvests(obs);
        self.audit_sites(obs);
        self.audit_raids(dials, obs, player_facing);

        let has_ground_objective = player_facing
            && dials.minimum_core_equivalents > 0
            && self.has_honest_ground_objective(dials, obs, home_tile, mode.public_map);

        let opening_core_at_start = combat_core_status(
            obs,
            combat_core_exclusions,
            &intents,
            u64::from(dials.minimum_core_equivalents),
        );
        let opening_core_active =
            player_facing && dials.minimum_core_equivalents > 0 && !opening_core_at_start.ready;
        let retained_deferred_claims = if opening_core_active {
            self.opening_core_deferred_claims(
                obs,
                OpeningClaimContext {
                    dials,
                    home: home_tile,
                    unit_contacts: mode.unit_contacts,
                    building_contacts: mode.building_contacts,
                    public_map: mode.public_map,
                },
                &mut intents,
            )
        } else if player_facing
            && dials.minimum_core_equivalents > 0
            && has_ground_objective
            && !Self::shallow_sentinel_reinforcement(obs, &intents)
        {
            self.post_floor_deferred_claims(
                obs,
                home_tile,
                mode.unit_contacts,
                mode.building_contacts,
                mode.public_map,
                &mut intents,
            )
        } else {
            Self::deferred_claims(obs)
        };
        let construction_commitment = Self::deferred_claims_commitment(&retained_deferred_claims);
        let mut spendable = obs.clone();
        if player_facing {
            spendable.scrap = spendable.scrap.saturating_sub(construction_commitment);
            for unit in &mut spendable.my_units {
                if unit
                    .founding
                    .is_some_and(|claim| !retained_deferred_claims.contains(&claim))
                {
                    unit.founding = None;
                }
            }
        }
        let obs = &spendable;
        let mut budget = obs.scrap;

        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .count();
        let contested_recon = player_facing
            .then(|| self.contested_recon_target(obs, home_tile))
            .flatten();
        let scouting_admitted = player_facing
            && dials.scouting
            && (harvesters >= immediate_harvester_target(dials) as usize
                || contested_recon.is_some());
        if scouting_admitted {
            // Exact scout ownership precedes every implicit utility claim.
            let mut unavailable = enlisted.to_vec();
            unavailable.extend_from_slice(reserved);
            unavailable.sort_unstable();
            unavailable.dedup();
            self.scouting_with_public_map(
                obs,
                home_tile,
                contested_recon,
                mode.public_map,
                &unavailable,
                &mut intents,
            );
        } else if player_facing {
            // Production still consumes this recomputable demand when the
            // roster is not yet large enough to dispatch the scouting channel.
            self.contested_recon_air_scout_needed = false;
            self.refresh_public_start_air_scout_demand(obs, home_tile, mode.public_map);
        }
        self.economy(
            obs,
            home_tile,
            player_facing,
            mode.unit_contacts,
            mode.building_contacts,
            &mut intents,
        );
        let construction_claims = ConstructionClaims {
            player_facing,
            enlisted,
            reserved,
        };
        let mut unavailable_builders = Vec::new();
        for intent in &intents {
            Self::claim_non_preemptible_intent_units(intent, &mut unavailable_builders);
        }
        let construction_context = ConstructionContext::new(home_tile, construction_claims)
            .with_combat_core_exclusions(combat_core_exclusions)
            .with_intelligence(mode.unit_contacts, mode.building_contacts)
            .with_public_map(mode.public_map)
            .excluding_builders(&unavailable_builders);
        let manages_opening = player_facing && dials.minimum_core_equivalents > 0;
        let mut opening_core_deficient = false;

        if manages_opening {
            self.construction(
                dials,
                obs,
                construction_context.during_opening_core(dials.turret_response, dials.aa_response),
                &mut budget,
                &mut intents,
            );
            let status = self.opening_core_production(
                dials,
                obs,
                ProductionContext::new(home_tile, construction_claims, None)
                    .with_combat_core_exclusions(combat_core_exclusions)
                    .with_intelligence(mode.unit_contacts, mode.building_contacts)
                    .with_public_map(mode.public_map),
                &mut budget,
                &mut intents,
            );
            // Recovering the last missing equivalent does not reopen every
            // voluntary spending channel in the same decision. Let the paid
            // line order become part of the next observation first, so a
            // later casualty cannot pair its recovery purchase with fresh
            // tech, support, or upgrade spending.
            opening_core_deficient = opening_core_active || !status.ready;
        }

        let opening_bootstrap_reserve = if manages_opening {
            self.opening_bootstrap_reserve(dials, obs, construction_context, &intents)
        } else {
            0
        };
        let opening_bootstrap_active = opening_bootstrap_reserve > 0;
        let paid_site_needs_shallow_reinforcement = has_ground_objective
            && construction_commitment == 0
            && !Self::unpaid_deferred_construction(obs, &intents)
            && obs
                .my_buildings
                .iter()
                .any(|building| !building.built && building.tier == 0);
        if manages_opening
            && !opening_core_deficient
            && !opening_bootstrap_active
            && paid_site_needs_shallow_reinforcement
        {
            Self::maintain_shallow_sentinel_reinforcement(obs, &mut budget, &mut intents);
        }
        let shallow_capital_guard =
            if has_ground_objective && !Self::shallow_sentinel_reinforcement(obs, &intents) {
                UnitKind::Sentinel.stats().cost
            } else {
                0
            };

        if manages_opening && !opening_core_deficient && !opening_bootstrap_active {
            let production_guard = shallow_capital_guard.max(opening_bootstrap_reserve);
            self.production_with_air_demand(
                dials,
                obs,
                ProductionContext::new(
                    home_tile,
                    construction_claims,
                    outstanding_air_production_ticks,
                )
                .with_combat_core_exclusions(combat_core_exclusions)
                .with_intelligence(mode.unit_contacts, mode.building_contacts)
                .with_public_map(mode.public_map)
                .with_voluntary_scrap_guard(Reserve::Exact(production_guard)),
                &mut budget,
                &mut intents,
            );

            if !Self::construction_channel_spent(&intents) {
                self.construction(
                    dials,
                    obs,
                    construction_context
                        .with_voluntary_scrap_guard(Reserve::Exact(production_guard)),
                    &mut budget,
                    &mut intents,
                );
            }
            let capital_advanced = strategic_capital_advanced
                || construction_commitment > 0
                || intents.iter().any(|intent| {
                    matches!(
                        intent,
                        Intent::Build { .. } | Intent::BuildWith { .. } | Intent::Upgrade { .. }
                    )
                });
            let unpaid_deferred_capital =
                construction_commitment > 0 || Self::unpaid_deferred_construction(obs, &intents);
            if has_ground_objective && capital_advanced && !unpaid_deferred_capital {
                Self::maintain_shallow_sentinel_reinforcement(obs, &mut budget, &mut intents);
            }
        } else if !player_facing || dials.minimum_core_equivalents == 0 {
            // Preserve the profile-free controller's frozen ordering, and
            // keep zero-floor policy fixtures useful for testing individual
            // channels without implicitly enabling the player-facing escrow.
            let healthy_home_screen = obs
                .my_units
                .iter()
                .filter(|unit| {
                    let stats = unit.kind.stats();
                    stats.domain == Domain::Ground && stats.can_fight()
                })
                .count()
                >= 3;
            let construction_precedes_discretionary =
                player_facing && healthy_home_screen && outstanding_air_production_ticks.is_none();
            let mut planned_construction = Vec::new();
            if construction_precedes_discretionary {
                let mut construction_budget = budget;
                self.construction(
                    dials,
                    obs,
                    construction_context,
                    &mut construction_budget,
                    &mut planned_construction,
                );
                budget = construction_budget;
            }
            if outstanding_air_production_ticks.is_some() {
                self.production_with_air_demand(
                    dials,
                    obs,
                    ProductionContext::new(
                        home_tile,
                        construction_claims,
                        outstanding_air_production_ticks,
                    )
                    .with_combat_core_exclusions(combat_core_exclusions)
                    .with_intelligence(mode.unit_contacts, mode.building_contacts)
                    .with_public_map(mode.public_map),
                    &mut budget,
                    &mut intents,
                );
            } else {
                self.production(
                    dials,
                    obs,
                    home_tile,
                    construction_claims,
                    &mut budget,
                    &mut intents,
                );
            }
            if construction_precedes_discretionary {
                intents.extend(planned_construction);
            } else if !Self::construction_channel_spent(&intents) {
                self.construction(dials, obs, construction_context, &mut budget, &mut intents);
            }
        }
        let construction_promised =
            construction_commitment > 0 || Self::unpaid_deferred_construction(obs, &intents);
        if player_facing
            && (construction_promised
                || opening_core_deficient
                || opening_bootstrap_active
                || shallow_capital_guard > 0)
        {
            // A promised construction preempts its builders and protects its
            // deferred fund from new repair work. A deficient opening core
            // stops every paid repairer so training can spend the full bank.
            let bound_builders: Vec<UnitId> = intents
                .iter()
                .filter_map(|intent| match intent {
                    Intent::BuildWith { builder, .. } => Some(*builder),
                    _ => None,
                })
                .collect();
            let repairers: Vec<UnitId> = obs
                .my_units
                .iter()
                .filter(|unit| {
                    unit.repairing
                        && !bound_builders.contains(&unit.id)
                        && (opening_core_deficient
                            || opening_bootstrap_active
                            || shallow_capital_guard > 0
                            || unit.kind.stats().harvest.is_some()
                            || obs.scrap == 0)
                })
                .map(|unit| unit.id)
                .collect();
            if !repairers.is_empty() {
                let before_spend = intents
                    .iter()
                    .position(|intent| {
                        matches!(
                            intent,
                            Intent::TrainAt { .. }
                                | Intent::Build { .. }
                                | Intent::BuildWith { .. }
                                | Intent::Upgrade { .. }
                        )
                    })
                    .unwrap_or(0);
                intents.insert(before_spend, Intent::StopUnits { units: repairers });
            }
        } else {
            self.repairs(dials, obs, mode, &mut budget, &mut intents);
        }
        if !opening_core_deficient && !opening_bootstrap_active && shallow_capital_guard == 0 {
            self.mobile_support(dials, obs, player_facing, &mut intents);
        }
        self.salvage(dials, obs, &mut intents);
        if !player_facing && dials.scouting && harvesters >= dials.harvester_target as usize {
            self.scouting(obs, home_tile, None, enlisted, &mut intents);
        }
        // The profile-free ferry gathers before the army channel so its Load
        // claims riders ahead of the draft. Player-facing transport waves are
        // already present in the strategic prelude and reservations.
        let ferry_claims = FerryClaims {
            enlisted,
            player_facing,
        };
        self.ferry(dials, obs, armies, home_tile, ferry_claims, &mut intents);
        self.army(dials, obs, armies, home_tile, mode, &mut intents);
        if !opening_core_deficient {
            self.air_raid(dials, obs, home_tile, enlisted, reserved, &mut intents);
        }
        intents
    }

    /// A harvester sent last think and idle again now bounced off an
    /// unreachable node — never ask twice. Only a node still reporting
    /// value earns the blacklist: a source the harvester honestly
    /// drained reads as empty and needs no entry (the amount filter
    /// already refuses it), and blacklisting it would poison the tile
    /// against every future deposit landing there.
    fn audit_harvests(&mut self, obs: &Observation) {
        for (id, node, sent_from) in std::mem::take(&mut self.last_sent) {
            // Collision separation can nudge a routeless worker one tile
            // from its send point, so exact equality misses a bounce.
            let bounced = obs
                .my_units
                .iter()
                .any(|u| u.id == id && u.idle && u.hp > 0 && u.tile.chebyshev(sent_from) <= 1);
            let still_reports = obs
                .known_scrap
                .iter()
                .chain(obs.known_wrecks.iter())
                .any(|(pos, amount)| *pos == node && *amount > 0);
            if bounced && still_reports && !self.dead_nodes.contains(&node) {
                self.dead_nodes.push(node);
            }
        }
    }

    fn refresh_contested_harvest_regions(
        &mut self,
        obs: &Observation,
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
    ) {
        let current_harvesters: Vec<_> = obs
            .my_units
            .iter()
            .chain(obs.ally_units.iter())
            .filter(|unit| unit.kind.stats().harvest.is_some())
            .map(|unit| HarvesterWatch {
                id: unit.id,
                tile: unit.tile,
                hp: unit.hp,
                source: unit.harvesting,
            })
            .collect();
        let incident_matches_worker = |incident: TilePos| {
            self.harvester_watch
                .iter()
                .filter_map(|previous| {
                    let current = current_harvesters
                        .iter()
                        .find(|current| current.id == previous.id);
                    (current.is_none_or(|current| current.hp < previous.hp))
                        .then_some((previous, current))
                })
                .any(|(previous, current)| {
                    std::iter::once(previous).chain(current).any(|worker| {
                        worker.tile.chebyshev(incident)
                            <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
                            || worker.source.is_some_and(|source| {
                                source.chebyshev(incident)
                                    <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
                            })
                    })
                })
        };
        let worker_incidents: Vec<_> = obs
            .salvage_incidents
            .iter()
            .copied()
            .filter(|incident| incident_matches_worker(*incident))
            .collect();
        self.harvester_watch = current_harvesters;

        for incident in worker_incidents {
            if let Some(region) = self
                .contested_harvest_regions
                .iter_mut()
                .find(|region| region.center == incident)
            {
                region.last_evidence = obs.tick;
                region.sweep_started_at = None;
                self.contested_harvest_clear_tiles
                    .retain(|(center, _)| *center != region.center);
            } else {
                self.contested_harvest_regions.push(ContestedHarvestRegion {
                    center: incident,
                    last_evidence: obs.tick,
                    sweep_started_at: None,
                });
            }
        }

        let danger = (!self.contested_harvest_regions.is_empty())
            .then(|| self.harvest_danger_projection(obs, unit_contacts, building_contacts));
        self.contested_recon_blocked.clear();
        let mut cleared_regions = BTreeSet::new();
        let mut timed_out_regions = BTreeSet::new();
        for region in &mut self.contested_harvest_regions {
            let active_incident = obs.salvage_incidents.iter().any(|incident| {
                incident.chebyshev(region.center) <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
            });
            let danger = danger
                .as_deref()
                .expect("a contested region prepared worker danger");
            let currently_dangerous = danger
                .contains_with_margin(region.center, crate::stats::HARVEST_INCIDENT_DANGER_RADIUS);
            if active_incident || currently_dangerous {
                region.sweep_started_at = None;
                self.contested_recon_blocked.insert(region.center);
                self.contested_harvest_clear_tiles
                    .retain(|(center, _)| *center != region.center);
                if currently_dangerous {
                    region.last_evidence = obs.tick;
                }
            } else {
                if region.sweep_started_at.is_some_and(|started| {
                    obs.tick.saturating_sub(started) > CONTESTED_RECON_SWEEP_TICKS
                }) {
                    region.sweep_started_at = None;
                    self.contested_harvest_clear_tiles
                        .retain(|(center, _)| *center != region.center);
                    self.contested_recon_blocked.insert(region.center);
                    timed_out_regions.insert(region.center);
                    continue;
                }
                let mut observed_any = false;
                for tile in Self::contested_region_tiles(obs, region.center) {
                    if obs.visible(tile)
                        || obs
                            .known_peaks
                            .binary_search_by_key(&(tile.y, tile.x), |peak| (peak.y, peak.x))
                            .is_ok()
                    {
                        observed_any = true;
                        self.contested_harvest_clear_tiles
                            .insert((region.center, tile));
                    }
                }
                if observed_any {
                    region.sweep_started_at.get_or_insert(obs.tick);
                }
                let complete = Self::contested_region_tiles(obs, region.center).all(|tile| {
                    self.contested_harvest_clear_tiles
                        .contains(&(region.center, tile))
                });
                if complete {
                    cleared_regions.insert(region.center);
                }
            }
        }
        if !timed_out_regions.is_empty() {
            self.contested_recon_retry_at = self
                .contested_recon_retry_at
                .max(obs.tick.saturating_add(CONTESTED_RECON_RETRY_TICKS));
        }
        if let Some((scout, region)) = self.contested_scout
            && self.contested_recon_blocked.contains(&region)
        {
            self.recall_contested_scout(scout);
        }
        self.contested_harvest_regions
            .retain(|region| !cleared_regions.contains(&region.center));
        while self.contested_harvest_regions.len() > crate::stats::HARVEST_INCIDENT_CAP {
            let (evict, evicted_center) = self
                .contested_harvest_regions
                .iter()
                .enumerate()
                .min_by_key(|(_, region)| (region.last_evidence, region.center.y, region.center.x))
                .map(|(index, region)| (index, region.center))
                .expect("an over-cap contested-region ledger is nonempty");
            if let Some((scout, region)) = self.contested_scout
                && region == evicted_center
            {
                self.recall_contested_scout(scout);
            }
            self.contested_harvest_regions.remove(evict);
        }
        self.contested_harvest_regions
            .sort_by_key(|region| (region.center.y, region.center.x));
        self.contested_harvest_clear_tiles.retain(|(center, _)| {
            self.contested_harvest_regions
                .binary_search_by_key(&(center.y, center.x), |region| {
                    (region.center.y, region.center.x)
                })
                .is_ok()
        });
        if let Some((scout, region)) = self.contested_scout
            && !self
                .contested_harvest_regions
                .iter()
                .any(|candidate| candidate.center == region)
        {
            self.contested_scout = None;
            if self.scout == Some(scout) {
                self.scout = None;
                self.scout_dispatch = None;
                self.public_start_ground_scout = None;
            }
        }
    }

    fn contested_region_tiles(
        obs: &Observation,
        center: TilePos,
    ) -> impl Iterator<Item = TilePos> + '_ {
        (-CONTESTED_RECON_RADIUS..=CONTESTED_RECON_RADIUS).flat_map(move |dy| {
            (-CONTESTED_RECON_RADIUS..=CONTESTED_RECON_RADIUS)
                .map(move |dx| center.offset(dx, dy))
                .filter(|tile| {
                    tile.x >= 0 && tile.y >= 0 && tile.x < obs.map_width && tile.y < obs.map_height
                })
        })
    }

    fn harvest_location_contested(&self, location: TilePos) -> bool {
        Self::location_in_contested_regions(&self.contested_harvest_regions, location)
    }

    fn repair_patient_unsafe(
        &self,
        building: &BuildingObs,
        danger: &danger::HarvestDangerProjection,
    ) -> bool {
        let (width, height) = building.kind.tier_stats(building.tier).size;
        (-1..=height).any(|dy| {
            (-1..=width).any(|dx| {
                let tile = building.anchor.offset(dx, dy);
                self.harvest_location_contested(tile) || danger.contains(tile)
            })
        })
    }

    fn location_in_contested_regions(
        regions: &[ContestedHarvestRegion],
        location: TilePos,
    ) -> bool {
        regions
            .iter()
            .any(|region| region.center.chebyshev(location) <= CONTESTED_HARVEST_RADIUS)
    }

    fn contested_recon_target(&self, obs: &Observation, home: TilePos) -> Option<ContestedRecon> {
        if obs.tick < self.contested_recon_retry_at || self.retreating_contested_scout.is_some() {
            return None;
        }
        self.contested_harvest_regions
            .iter()
            .filter(|region| !self.contested_recon_blocked.contains(&region.center))
            .map(|region| {
                (
                    region.center.chebyshev(home),
                    region.center.y,
                    region.center.x,
                )
            })
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
            .map(|region| {
                let target = Self::contested_region_tiles(obs, region)
                    .filter(|tile| {
                        !self
                            .contested_harvest_clear_tiles
                            .contains(&(region, *tile))
                    })
                    .min_by_key(|tile| (tile.chebyshev(region), tile.y, tile.x))
                    .unwrap_or(region);
                ContestedRecon { region, target }
            })
    }

    fn retreat_contested_scout(
        &mut self,
        obs: &Observation,
        home: TilePos,
        intents: &mut Vec<Intent>,
    ) {
        let Some(retreat) = self.retreating_contested_scout else {
            return;
        };
        let scout = retreat.unit;
        let Some(unit) = obs.my_units.iter().find(|unit| unit.id == scout) else {
            self.retreating_contested_scout = None;
            self.contested_recon_retry_at = self
                .contested_recon_retry_at
                .max(obs.tick.saturating_add(CONTESTED_RECON_RETRY_TICKS));
            return;
        };
        let goal = self.passable_near(obs, home);
        if unit.tile.chebyshev(goal) <= 1 {
            self.retreating_contested_scout = None;
            self.contested_recon_retry_at = self
                .contested_recon_retry_at
                .max(obs.tick.saturating_add(CONTESTED_RECON_RETRY_TICKS));
        } else if !retreat.order_dispatched || unit.idle {
            intents.push(Intent::MoveUnits {
                units: vec![scout],
                goal,
            });
            self.retreating_contested_scout = Some(RetreatingContestedScout {
                unit: scout,
                order_dispatched: true,
            });
        }
    }

    fn recall_contested_scout(&mut self, scout: UnitId) {
        if !self
            .contested_scout
            .is_some_and(|(assigned, _)| assigned == scout)
        {
            return;
        }
        self.contested_scout = None;
        if self.scout == Some(scout) {
            self.scout = None;
            self.scout_dispatch = None;
            self.public_start_ground_scout = None;
        }
        self.retreating_contested_scout = Some(RetreatingContestedScout {
            unit: scout,
            order_dispatched: false,
        });
    }

    fn evacuate_contested_workers(
        &mut self,
        obs: &Observation,
        home: TilePos,
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
        intents: &mut Vec<Intent>,
    ) {
        let needs_danger = !self.evacuating_workers.is_empty()
            || obs
                .my_units
                .iter()
                .any(|unit| unit.kind.stats().harvest.is_some() && unit.tile.chebyshev(home) > 1);
        if !needs_danger {
            return;
        }
        let danger = self.harvest_danger_projection(obs, unit_contacts, building_contacts);
        let contested_regions = &self.contested_harvest_regions;
        let endangered = |unit: &UnitObs| {
            Self::location_in_contested_regions(contested_regions, unit.tile)
                || danger.contains(unit.tile)
        };
        self.evacuating_workers.retain(|id| {
            obs.my_units.iter().any(|unit| {
                unit.id == *id && unit.kind.stats().harvest.is_some() && endangered(unit)
            })
        });

        let mut evacuations: Vec<(TilePos, Vec<UnitId>)> = Vec::new();
        for unit in obs.my_units.iter().filter(|unit| {
            unit.kind.stats().harvest.is_some() && unit.tile.chebyshev(home) > 1 && endangered(unit)
        }) {
            if (!self.evacuating_workers.contains(&unit.id) || unit.idle)
                && let Some(goal) = self.worker_evacuation_goal(obs, unit, home, &danger)
            {
                if let Some((_, workers)) = evacuations
                    .iter_mut()
                    .find(|(candidate, _)| *candidate == goal)
                {
                    workers.push(unit.id);
                } else {
                    evacuations.push((goal, vec![unit.id]));
                }
                if !self.evacuating_workers.contains(&unit.id) {
                    self.evacuating_workers.push(unit.id);
                }
            }
            if let Some((_, anchor)) = unit.founding {
                self.pending_sites.retain(|pending| *pending != anchor);
            }
            if self.scout == Some(unit.id) {
                let public_start_probe = self.public_start_ground_scout == Some(unit.id);
                self.scout = None;
                self.scout_dispatch = None;
                self.public_start_ground_scout = None;
                if public_start_probe {
                    self.persistent_air_scout_needed = true;
                }
            }
        }
        self.evacuating_workers.sort_unstable();
        self.evacuating_workers.dedup();
        evacuations.sort_unstable_by_key(|(goal, _)| (goal.y, goal.x));
        for (goal, mut workers) in evacuations {
            workers.sort_unstable();
            workers.dedup();
            intents.push(Intent::MoveUnits {
                units: workers,
                goal,
            });
        }
    }

    fn worker_evacuation_goal(
        &self,
        obs: &Observation,
        worker: &UnitObs,
        home: TilePos,
        danger: &danger::HarvestDangerProjection,
    ) -> Option<TilePos> {
        let initial_danger = self.worker_escape_component(obs, worker.tile, danger);
        let mut known_routes = RouteProjection::known_ground(obs);
        let mut safe_routes = RouteProjection::ground_avoiding(obs, |tile| {
            (self.harvest_location_contested(tile) || danger.contains(tile))
                && !initial_danger.contains(&tile)
        });
        let search_origin = if initial_danger.is_empty() {
            home
        } else {
            worker.tile
        };
        let max_radius = obs.map_width.max(obs.map_height).max(0);
        for radius in 0..=max_radius {
            let mut best = None;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dy.abs()) != radius {
                        continue;
                    }
                    let tile = search_origin.offset(dx, dy);
                    if !routing::ground_open(obs, tile)
                        || !obs.explored(tile)
                        || !self.evacuation_standing_area_safe(obs, tile, danger)
                        || !known_routes.unit_reaches(worker, tile)
                        || !safe_routes.unit_reaches(worker, tile)
                        || !safe_routes.direct_line_avoids_blocked(worker.tile, tile)
                        || !safe_routes.command_path_avoids_blocked(worker.tile, tile)
                    {
                        continue;
                    }
                    let key = (
                        worker.tile.manhattan(tile),
                        tile.manhattan(home),
                        tile.y,
                        tile.x,
                    );
                    if best.is_none_or(|(_, current)| key < current) {
                        best = Some((tile, key));
                    }
                }
            }
            if let Some((tile, _)) = best {
                return Some(tile);
            }
        }
        None
    }

    fn worker_escape_component(
        &self,
        obs: &Observation,
        origin: TilePos,
        danger: &danger::HarvestDangerProjection,
    ) -> BTreeSet<TilePos> {
        let unsafe_at = |tile| self.harvest_location_contested(tile) || danger.contains(tile);
        if !routing::ground_open(obs, origin) || !unsafe_at(origin) {
            return BTreeSet::new();
        }
        let mut component = BTreeSet::from([origin]);
        let mut frontier = vec![origin];
        while let Some(tile) = frontier.pop() {
            for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
                let neighbor = tile.offset(dx, dy);
                if routing::ground_open(obs, neighbor)
                    && unsafe_at(neighbor)
                    && component.insert(neighbor)
                {
                    frontier.push(neighbor);
                }
            }
        }
        component
    }

    fn evacuation_standing_area_safe(
        &self,
        obs: &Observation,
        goal: TilePos,
        danger: &danger::HarvestDangerProjection,
    ) -> bool {
        (-1..=1).all(|dy| {
            (-1..=1).all(|dx| {
                let tile = goal.offset(dx, dy);
                !routing::ground_open(obs, tile)
                    || (!self.harvest_location_contested(tile) && !danger.contains(tile))
            })
        })
    }

    #[cfg(test)]
    fn source_has_known_danger(
        obs: &Observation,
        node: TilePos,
        unit_contacts: Option<&[UnitContact]>,
        building_contacts: Option<&[BuildingContact]>,
    ) -> bool {
        danger::direct_location_has_known_danger(obs, node, 0, unit_contacts, building_contacts)
    }

    /// Remember a Harvest command that survived intent lowering long enough
    /// to audit an immediate no-route bounce on the next think.
    pub(super) fn record_dispatched_harvest(
        &mut self,
        obs: &Observation,
        unit: UnitId,
        node: TilePos,
    ) {
        let Some(worker) = obs.my_units.iter().find(|worker| worker.id == unit) else {
            return;
        };
        self.last_sent.retain(|(sent, _, _)| *sent != unit);
        self.last_sent.push((unit, node, worker.tile));
    }

    /// Forget source evidence for workers whose Harvest was replaced by a
    /// later dispatched order. Commands are visited in output order, so a
    /// later Harvest can establish a fresh assignment after this reset.
    pub(super) fn record_dispatched_retask(&mut self, units: &[UnitId]) {
        self.last_sent.retain(|(unit, _, _)| !units.contains(unit));
    }

    /// A site requested last think that never appeared was refused for a
    /// reason the observation can't see; stop asking for that anchor.
    /// A pending deferred found is a site on its way, not a refusal:
    /// the founder pays on arrival, so while one is still walking the
    /// anchor stays pending for a later audit to judge (blacklisting
    /// it would poison ground the claim is about to prove). The
    /// player-facing brain defers claims outside current sight, so a
    /// walking founder remains pending until the ground is actually reached.
    fn audit_sites(&mut self, obs: &Observation) {
        for anchor in std::mem::take(&mut self.pending_sites) {
            let appeared = obs.my_buildings.iter().any(|b| b.anchor == anchor);
            if appeared {
                continue;
            }
            let walking = obs
                .my_units
                .iter()
                .any(|u| u.founding.is_some_and(|(_, a)| a == anchor));
            if walking {
                self.pending_sites.push(anchor);
            } else if !self.dead_anchors.contains(&anchor) {
                self.dead_anchors.push(anchor);
            }
        }
    }

    /// Remember a newly dispatched construction command for next
    /// think's refusal audit. Existing sites are orphan relief, and an
    /// Extractor frame has only one legal anchor, so neither may enter
    /// the site blacklist.
    pub(super) fn record_dispatched_build(
        &mut self,
        obs: &Observation,
        kind: BuildingKind,
        anchor: TilePos,
    ) {
        if kind != BuildingKind::Extractor
            && !obs
                .my_buildings
                .iter()
                .any(|building| building.anchor == anchor)
            && !self.pending_sites.contains(&anchor)
        {
            self.pending_sites.push(anchor);
        }
    }

    /// A shrinking harvest line means raiders. Player-facing identities keep
    /// the response open until every configured Turret is complete; the
    /// profile-free controller preserves its historical one-site reset.
    fn audit_raids(&mut self, dials: &Dials, obs: &Observation, player_facing: bool) {
        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .count();
        if harvesters < self.harvesters_seen {
            self.raided = true;
        }
        self.harvesters_seen = harvesters;
        let turret_sites = obs
            .my_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Turret)
            .count();
        let built_turrets = obs
            .my_buildings
            .iter()
            .filter(|building| building.kind == BuildingKind::Turret && building.built)
            .count();
        let response_satisfied = if player_facing {
            built_turrets >= dials.turret_cap
        } else {
            turret_sites > self.turrets_seen
        };
        if self.raided && response_satisfied {
            self.raided = false;
        }
        self.turrets_seen = turret_sites;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::Executive;
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION, Observation, UnitObs};
    use crate::bot::{PersonalityTraits, Specialty};
    use crate::ids::{BuildingId, PlayerId, UnitId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance, PlayerSpec, UnitSpec};
    use crate::{Command, PlayerCommand, Scenario};

    fn obs_with(units: Vec<UnitObs>) -> Observation {
        Observation {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 0,
            map_width: 32,
            map_height: 20,
            my_units: units,
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: vec![true; 32 * 20],
            explored: vec![true; 32 * 20],
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

    fn public_map(obs: &Observation) -> PublicMapBriefing {
        let width = usize::try_from(obs.map_width).expect("the test map has a positive width");
        let height = usize::try_from(obs.map_height).expect("the test map has a positive height");
        assert!(width >= 2 && height >= 2);
        let mut map = vec![".".repeat(width); height];
        map[0].replace_range(..1, "1");
        PublicMapBriefing::from_scenario(&Scenario {
            name: "utility test map".into(),
            seed: 0,
            map,
            players: vec![PlayerSpec {
                name: "test seat".into(),
                faction: obs.faction,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            }],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        })
        .expect("the focused observation has a matching public map")
    }

    fn public_map_with_home_and_frames(
        obs: &Observation,
        home: TilePos,
        frames: &[TilePos],
    ) -> PublicMapBriefing {
        let width = usize::try_from(obs.map_width).expect("the test map has a positive width");
        let height = usize::try_from(obs.map_height).expect("the test map has a positive height");
        let mut map = vec![vec![b'.'; width]; height];
        map[home.y as usize][home.x as usize] = b'1';
        for frame in frames {
            map[frame.y as usize][frame.x as usize] = b'E';
        }
        PublicMapBriefing::from_scenario(&Scenario {
            name: "utility test map with frames".into(),
            seed: 0,
            map: map
                .into_iter()
                .map(|row| String::from_utf8(row).expect("the fixture map is ASCII"))
                .collect(),
            players: vec![PlayerSpec {
                name: "test seat".into(),
                faction: obs.faction,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            }],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        })
        .expect("the focused observation has a matching framed public map")
    }

    fn corridor_briefing(home: TilePos, enemy: TilePos, blocker: char) -> PublicMapBriefing {
        let mut map = vec![vec![b'#'; 32]; 20];
        for y in home.y..home.y + BuildingKind::Foundry.base_stats().size.1 {
            map[y as usize].fill(b'.');
            if blocker != '.' {
                map[y as usize][16] = blocker as u8;
            }
        }
        map[home.y as usize][home.x as usize] = b'1';
        map[enemy.y as usize][enemy.x as usize] = b'2';
        PublicMapBriefing::from_scenario(&Scenario {
            name: "opening guard corridor".into(),
            seed: 0,
            map: map
                .into_iter()
                .map(|row| String::from_utf8(row).expect("the fixture map is ASCII"))
                .collect(),
            players: vec![
                PlayerSpec {
                    name: "home".into(),
                    faction: crate::Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "enemy".into(),
                    faction: crate::Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        })
        .expect("the corridor briefing builds")
    }

    #[test]
    fn shallow_reinforcement_escape_requires_proven_static_disconnection() {
        let home = TilePos::new(2, 10);
        let enemy_start = TilePos::new(29, 10);
        let mut obs = obs_with(Vec::new());
        obs.visible.fill(false);
        obs.explored.fill(false);
        obs.my_buildings = vec![standing_building(0, BuildingKind::Foundry, home)];
        obs.my_queues = vec![Vec::new()];
        let policy = UtilityPolicy::new();
        let dials = Dials::full();

        let scrap_choke = corridor_briefing(home, enemy_start, 's');
        assert!(!UtilityPolicy::public_start_ground_connected(
            &scrap_choke,
            &obs,
            home,
            enemy_start
        ));
        assert!(policy.has_honest_ground_objective(&dials, &obs, home, Some(&scrap_choke)));

        let mut cleared = policy;
        cleared.cleared_hostile_starts.push(PlayerId(1));
        obs.enemy_buildings.push(BuildingObs {
            id: BuildingId(20),
            player: PlayerId(1),
            kind: BuildingKind::Foundry,
            anchor: enemy_start.offset(-4, 0),
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        assert!(!cleared.ordinary_ground_has_work(&dials, &obs, home));
        assert!(
            cleared.has_honest_ground_objective(&dials, &obs, home, Some(&scrap_choke)),
            "a known expansion across unexplored but public open terrain still needs reinforcement"
        );

        let divided = corridor_briefing(home, enemy_start, '#');
        assert!(
            !cleared.has_honest_ground_objective(&dials, &obs, home, Some(&divided)),
            "only permanent public terrain disconnection may waive the shallow guard"
        );
    }

    fn harvester(id: u32, founding: Option<(BuildingKind, TilePos)>) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile: TilePos::new(5, 5),
            hp: UnitKind::Harvester.stats().max_hp,
            idle: founding.is_none(),
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding,
            repairing: false,
            grounded: false,
        }
    }

    #[test]
    fn a_deficient_core_keeps_only_a_safe_unoccupied_starting_home_extractor_claim() {
        let home = TilePos::new(3, 3);
        let frame = TilePos::new(10, 3);
        let claim = (BuildingKind::Extractor, frame);
        let mut obs = obs_with(vec![UnitObs {
            tile: TilePos::new(7, 6),
            ..harvester(1, Some(claim))
        }]);
        obs.known_frames = vec![frame];
        obs.my_buildings = vec![standing_building(10, BuildingKind::Foundry, home)];
        obs.my_queues = vec![Vec::new()];
        let map = public_map_with_home_and_frames(&obs, home, &[frame]);
        let dials = Dials::full();

        let mut safe_intents = Vec::new();
        let safe = UtilityPolicy::new().opening_core_deferred_claims(
            &obs,
            OpeningClaimContext {
                dials: &dials,
                home,
                unit_contacts: None,
                building_contacts: None,
                public_map: Some(&map),
            },
            &mut safe_intents,
        );
        assert_eq!(safe, [claim]);
        assert!(safe_intents.is_empty());

        let mut occupied = obs.clone();
        let mut blocker = standing_building(11, BuildingKind::RepairBay, frame);
        blocker.player = PlayerId(2);
        occupied.ally_buildings.push(blocker);
        let mut occupied_intents = Vec::new();
        let retained = UtilityPolicy::new().opening_core_deferred_claims(
            &occupied,
            OpeningClaimContext {
                dials: &dials,
                home,
                unit_contacts: None,
                building_contacts: None,
                public_map: Some(&map),
            },
            &mut occupied_intents,
        );
        assert!(retained.is_empty());
        assert_eq!(
            occupied_intents,
            [Intent::StopUnits {
                units: vec![UnitId(1)]
            }]
        );

        let mut contested_policy = UtilityPolicy::new();
        contested_policy.contested_harvest_regions = vec![ContestedHarvestRegion {
            center: frame,
            last_evidence: obs.tick,
            sweep_started_at: None,
        }];
        let mut contested_intents = Vec::new();
        let retained = contested_policy.opening_core_deferred_claims(
            &obs,
            OpeningClaimContext {
                dials: &dials,
                home,
                unit_contacts: None,
                building_contacts: None,
                public_map: Some(&map),
            },
            &mut contested_intents,
        );
        assert!(retained.is_empty());
        assert_eq!(
            contested_intents,
            [Intent::StopUnits {
                units: vec![UnitId(1)]
            }]
        );
    }

    #[test]
    fn a_deficient_core_rejects_stale_deferred_defenses_despite_current_threats() {
        let home = TilePos::new(3, 10);
        let claims = [
            (BuildingKind::Turret, TilePos::new(3, 16)),
            (BuildingKind::Turret, TilePos::new(6, 16)),
            (BuildingKind::FlakTurret, TilePos::new(9, 16)),
            (BuildingKind::FlakTurret, TilePos::new(12, 16)),
        ];
        let mut obs = obs_with(
            claims
                .into_iter()
                .enumerate()
                .map(|(index, claim)| UnitObs {
                    tile: claim.1.offset(0, 2),
                    ..harvester(u32::try_from(index + 1).unwrap(), Some(claim))
                })
                .collect(),
        );
        obs.my_buildings = vec![standing_building(10, BuildingKind::Foundry, home)];
        obs.my_queues = vec![Vec::new()];
        obs.enemy_units = vec![
            fighter(20, PlayerId(1), TilePos::new(9, 10)),
            UnitObs {
                id: UnitId(21),
                player: PlayerId(1),
                kind: UnitKind::Condor,
                tile: TilePos::new(9, 9),
                hp: UnitKind::Condor.stats().max_hp,
                idle: true,
                carrying: 0,
                harvesting: None,
                cargo: 0,
                site: None,
                salvaging: None,
                founding: None,
                repairing: false,
                grounded: false,
            },
        ];
        let map = public_map_with_home_and_frames(&obs, home, &[]);
        let dials = Dials::full();

        let mut current_intents = Vec::new();
        let retained = UtilityPolicy::new().opening_core_deferred_claims(
            &obs,
            OpeningClaimContext {
                dials: &dials,
                home,
                unit_contacts: None,
                building_contacts: None,
                public_map: Some(&map),
            },
            &mut current_intents,
        );
        assert!(retained.is_empty());
        let stopped = current_intents
            .iter()
            .find_map(|intent| match intent {
                Intent::StopUnits { units } => Some(units),
                _ => None,
            })
            .expect("the stale claims are stopped");
        assert_eq!(stopped.len(), 4);

        let mut noncurrent = obs;
        noncurrent.enemy_units.clear();
        noncurrent.blips = vec![TilePos::new(9, 10), TilePos::new(9, 9)];
        let mut noncurrent_intents = Vec::new();
        let retained = UtilityPolicy::new().opening_core_deferred_claims(
            &noncurrent,
            OpeningClaimContext {
                dials: &dials,
                home,
                unit_contacts: None,
                building_contacts: None,
                public_map: Some(&map),
            },
            &mut noncurrent_intents,
        );
        assert!(retained.is_empty());
        assert_eq!(
            noncurrent_intents,
            [Intent::StopUnits {
                units: vec![UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
            }]
        );
    }

    #[test]
    fn a_travelling_capital_claim_requires_a_shallow_bank_after_reinforcement_finishes() {
        let home = TilePos::new(3, 3);
        let claim = (BuildingKind::Fabricator, TilePos::new(9, 3));
        let mut units: Vec<_> = (0..8)
            .map(|id| {
                fighter(
                    id,
                    PlayerId(0),
                    TilePos::new(5 + i32::try_from(id).expect("small fixture id"), 8),
                )
            })
            .collect();
        units.push(UnitObs {
            tile: TilePos::new(6, 4),
            ..harvester(20, Some(claim))
        });
        units.extend((21..24).map(|id| UnitObs {
            tile: TilePos::new(6 + i32::try_from(id - 20).expect("small fixture id"), 5),
            ..harvester(id, None)
        }));
        let mut obs = obs_with(units);
        obs.my_buildings = vec![standing_building(10, BuildingKind::Foundry, home)];
        obs.my_queues = vec![Vec::new()];
        let mut enemy = standing_building(30, BuildingKind::Foundry, TilePos::new(24, 12));
        enemy.player = PlayerId(1);
        obs.enemy_buildings.push(enemy);
        let capital_cost = BuildingKind::Fabricator
            .base_stats()
            .construction
            .expect("Fabricators have a construction price")
            .cost;
        let sentinel_cost = UnitKind::Sentinel.stats().cost;
        let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_201)
            .resolve_profile();
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
        let map = public_map(&obs);

        obs.scrap = capital_cost;
        let paused = UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &map);
        assert!(paused.iter().any(|intent| matches!(
            intent,
            Intent::StopUnits { units } if units == &[UnitId(20)]
        )));
        assert!(paused.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Fabricator,
                ..
            } | Intent::BuildWith {
                kind: BuildingKind::Fabricator,
                ..
            }
        )));

        obs.scrap = capital_cost + sentinel_cost;
        let retained = UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &map);
        assert!(retained.iter().all(
            |intent| !matches!(intent, Intent::StopUnits { units } if units == &[UnitId(20)])
        ));
        assert!(
            retained.iter().all(|intent| !matches!(
                intent,
                Intent::TrainAt { .. }
                    | Intent::Build { .. }
                    | Intent::BuildWith { .. }
                    | Intent::Upgrade { .. }
            )),
            "the exact surviving reserve stays banked until the deferred capital pays: {retained:?}"
        );
    }

    #[test]
    fn a_paid_site_turns_its_shallow_bank_into_reinforcement_before_more_capital() {
        let home = TilePos::new(3, 3);
        let mut units: Vec<_> = (0..8)
            .map(|id| {
                fighter(
                    id,
                    PlayerId(0),
                    TilePos::new(5 + i32::try_from(id).expect("small fixture id"), 8),
                )
            })
            .collect();
        units.extend((20..24).map(|id| harvester(id, None)));
        let builder = units
            .iter_mut()
            .find(|unit| unit.id == UnitId(20))
            .expect("the paid site has a builder");
        builder.idle = false;
        builder.site = Some(BuildingId(13));
        units.sort_unstable_by_key(|unit| unit.id);

        let mut paid_site = standing_building(13, BuildingKind::Turret, TilePos::new(9, 3));
        paid_site.built = false;
        paid_site.hp /= 2;
        let mut obs = obs_with(units);
        obs.my_buildings = vec![
            standing_building(10, BuildingKind::Foundry, home),
            standing_building(11, BuildingKind::Fabricator, TilePos::new(4, 3)),
            standing_building(12, BuildingKind::Airworks, TilePos::new(6, 3)),
            paid_site,
        ];
        obs.my_queues = vec![
            vec![UnitKind::Harvester],
            vec![UnitKind::Lancer, UnitKind::Lancer],
            vec![UnitKind::Kestrel, UnitKind::Kestrel],
            Vec::new(),
        ];
        let mut enemy = standing_building(30, BuildingKind::Foundry, TilePos::new(24, 12));
        enemy.player = PlayerId(1);
        obs.enemy_buildings.push(enemy);
        let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_201)
            .resolve_profile();
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
        let map = public_map(&obs);

        obs.scrap = UnitKind::Sentinel.stats().cost;
        let exact = UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &map);
        assert!(exact.contains(&Intent::TrainAt {
            building: BuildingId(10),
            kind: UnitKind::Sentinel,
        }));
        assert!(exact.iter().all(|intent| !matches!(
            intent,
            Intent::Build { .. } | Intent::BuildWith { .. } | Intent::Upgrade { .. }
        )));

        obs.scrap = 10_000;
        let wealthy = UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &map);
        let reinforcement = wealthy
            .iter()
            .position(|intent| {
                matches!(
                    intent,
                    Intent::TrainAt {
                        building: BuildingId(10),
                        kind: UnitKind::Sentinel,
                    }
                )
            })
            .expect("the paid site must consume the shallow bank");
        let next_capital = wealthy
            .iter()
            .position(|intent| {
                matches!(
                    intent,
                    Intent::Build { .. } | Intent::BuildWith { .. } | Intent::Upgrade { .. }
                )
            })
            .expect("the wealthy fixture can afford another capital project");
        assert!(reinforcement < next_capital, "{wealthy:?}");
    }

    #[test]
    fn shallow_reinforcement_must_survive_the_next_production_phase() {
        let home = TilePos::new(3, 3);
        let second = TilePos::new(18, 3);
        let mut obs = obs_with(Vec::new());
        obs.my_buildings = vec![
            standing_building(10, BuildingKind::Foundry, home),
            standing_building(11, BuildingKind::Foundry, second),
        ];
        obs.my_queues = vec![vec![UnitKind::Sentinel], vec![UnitKind::Sentinel]];

        assert!(
            !UtilityPolicy::shallow_sentinel_reinforcement(&obs, &[]),
            "each Foundry may finish its lone front item in the same production phase"
        );

        obs.my_queues[0] = vec![UnitKind::Harvester, UnitKind::Sentinel];
        assert!(UtilityPolicy::shallow_sentinel_reinforcement(&obs, &[]));

        obs.my_queues[0] = vec![UnitKind::Sentinel, UnitKind::Harvester];
        obs.my_queues[1].clear();
        let planned = [Intent::TrainAt {
            building: BuildingId(11),
            kind: UnitKind::Sentinel,
        }];
        assert!(
            UtilityPolicy::shallow_sentinel_reinforcement(&obs, &planned),
            "a same-think enqueue starts at zero progress and survives this tick"
        );

        let full = [Intent::TrainAt {
            building: BuildingId(10),
            kind: UnitKind::Sentinel,
        }];
        assert!(
            !UtilityPolicy::shallow_sentinel_reinforcement(&obs, &full),
            "a planned Sentinel outside a full two-slot queue is not accepted evidence"
        );
    }

    #[test]
    fn a_front_slot_sentinel_cannot_cover_same_tick_deferred_capital() {
        let home = TilePos::new(2, 8);
        let enemy = TilePos::new(35, 8);
        let claim = TilePos::new(10, 8);
        let founder_tile = claim.offset(-1, 1);
        let mut map = vec![".".repeat(40); 20];
        map[home.y as usize].replace_range(home.x as usize..home.x as usize + 1, "1");
        map[enemy.y as usize].replace_range(enemy.x as usize..enemy.x as usize + 1, "2");
        let mut units = vec![UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: founder_tile.x,
            y: founder_tile.y,
        }];
        units.extend((0..3).map(|offset| UnitSpec {
            player: 0,
            kind: UnitKind::Harvester,
            x: 7 + offset,
            y: 13,
        }));
        units.extend((0..8).map(|offset| UnitSpec {
            player: 0,
            kind: UnitKind::Sentinel,
            x: 7 + offset,
            y: 16,
        }));
        let scenario = Scenario {
            name: "shallow reinforcement production race".into(),
            seed: 0,
            map,
            players: vec![
                PlayerSpec {
                    name: "home".into(),
                    faction: crate::Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "enemy".into(),
                    faction: crate::Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units,
            buildings: Vec::new(),
            meta: None,
        };
        let public_map = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let mut state = scenario.build().unwrap();
        let me = PlayerId(0);
        let founder = state
            .units()
            .iter()
            .find(|unit| unit.player == me && unit.tile() == founder_tile)
            .expect("the deferred founder exists")
            .id;
        state.unit_mut(founder).unwrap().order = crate::state::Order::Found {
            kind: BuildingKind::Fabricator,
            anchor: claim,
        };
        let foundry = state
            .buildings()
            .iter()
            .find(|building| building.player == me && building.kind == BuildingKind::Foundry)
            .expect("the home Foundry exists")
            .id;
        let foundry_state = state.building_mut(foundry).unwrap();
        foundry_state.queue.push_back(UnitKind::Sentinel);
        foundry_state.progress = UnitKind::Sentinel.stats().train_ticks - 1;
        let capital_cost = BuildingKind::Fabricator
            .base_stats()
            .construction
            .expect("Fabricators have a construction price")
            .cost;
        state.player_mut(me).scrap = capital_cost;
        let mut brain = crate::bot::Brain::scripted(
            me,
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_201),
            std::sync::Arc::new(public_map),
        );

        let commands = brain.act(&state);

        assert!(commands.iter().any(|command| matches!(
            &command.command,
            Command::Stop { units } if units == &[founder]
        )));
        state.tick(&commands);
        assert!(
            state
                .buildings()
                .iter()
                .all(|building| building.kind != BuildingKind::Fabricator
                    || building.anchor != claim),
            "the deferred capital must not pay after the only observed shallow Sentinel leaves its queue"
        );
        assert!(
            state.player(me).scrap >= UnitKind::Sentinel.stats().cost,
            "the post-production state must retain an affordable shallow reinforcement"
        );
        assert!(!matches!(
            state.unit(founder).map(|unit| unit.order),
            Some(crate::state::Order::Found { .. })
        ));
    }

    #[test]
    fn ready_core_bootstrap_precedes_scouting_and_other_discretionary_spending() {
        let home = TilePos::new(2, 8);
        let frame = home.offset(6, 0);
        let mut units: Vec<_> = (1..=3).map(|id| harvester(id, None)).collect();
        units.extend((20..28).map(|id| {
            fighter(
                id,
                PlayerId(0),
                home.offset(i32::try_from(id - 20).expect("small fixture id"), 5),
            )
        }));
        let mut obs = obs_with(units);
        obs.scrap = UnitKind::Harvester.stats().cost
            + BuildingKind::Extractor
                .base_stats()
                .construction
                .expect("Extractors have a construction price")
                .cost;
        obs.known_frames = vec![frame];
        obs.my_buildings = vec![
            standing_building(10, BuildingKind::Foundry, home),
            standing_building(11, BuildingKind::Airworks, home.offset(0, -5)),
        ];
        obs.my_queues = vec![Vec::new(), Vec::new()];
        let mut enemy = standing_building(90, BuildingKind::Foundry, TilePos::new(25, 8));
        enemy.player = PlayerId(1);
        obs.enemy_buildings.push(enemy);
        let map = public_map_with_home_and_frames(&obs, home, &[frame]);
        let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_201)
            .resolve_profile();
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
        let scout = crate::stats::Role::Scout.unit_for(obs.faction);
        let mut policy = UtilityPolicy::new();
        policy.persistent_air_scout_needed = true;

        let intents = policy.think_player_facing(&dials, &obs, &[], &[], &[], &map);

        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Harvester,
                ..
            }
        )));
        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::BuildWith {
                kind: BuildingKind::Extractor,
                anchor,
                ..
            } if *anchor == frame
        )));
        assert!(intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt { kind, .. } if *kind == scout
        )));
        let spent = intents.iter().fold(0_u32, |total, intent| {
            total.saturating_add(match intent {
                Intent::TrainAt { kind, .. } => kind.stats().cost,
                Intent::Build { kind, .. } | Intent::BuildWith { kind, .. } => kind
                    .base_stats()
                    .construction
                    .map_or(0, |construction| construction.cost),
                _ => 0,
            })
        });
        assert_eq!(
            spent, obs.scrap,
            "only the two bootstrap obligations spend: {intents:?}"
        );
    }

    #[test]
    fn shallow_capital_guard_stops_paid_worker_repair() {
        let home = TilePos::new(2, 8);
        let mut units: Vec<_> = (1..=4).map(|id| harvester(id, None)).collect();
        units[0].repairing = true;
        units.extend((20..28).map(|id| {
            fighter(
                id,
                PlayerId(0),
                home.offset(i32::try_from(id - 20).expect("small fixture id"), 5),
            )
        }));
        let mut obs = obs_with(units);
        obs.scrap = UnitKind::Sentinel.stats().cost;
        let mut foundry = standing_building(10, BuildingKind::Foundry, home);
        foundry.hp /= 2;
        obs.my_buildings = vec![foundry];
        obs.my_queues = vec![vec![UnitKind::Harvester, UnitKind::Harvester]];
        let mut enemy = standing_building(90, BuildingKind::Foundry, TilePos::new(25, 8));
        enemy.player = PlayerId(1);
        obs.enemy_buildings.push(enemy);
        let map = corridor_briefing(home, TilePos::new(29, 10), '.');
        let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_201)
            .resolve_profile();
        let mut dials =
            Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
        dials.extractors = false;

        let intents = UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &map);

        assert!(intents.iter().any(|intent| matches!(
            intent,
            Intent::StopUnits { units } if units == &[UnitId(1)]
        )));
        assert!(
            intents.iter().all(|intent| !matches!(
                intent,
                Intent::Repair { .. } | Intent::RepairUnits { .. }
            ))
        );
    }

    #[test]
    fn shallow_capital_guard_stops_active_tenders_and_refuses_new_welds() {
        let home = TilePos::new(2, 8);
        let mut units: Vec<_> = (1..=4).map(|id| harvester(id, None)).collect();
        units.extend((20..28).map(|id| {
            fighter(
                id,
                PlayerId(0),
                home.offset(i32::try_from(id - 20).expect("small fixture id"), 5),
            )
        }));
        let tender = |id, tile, idle, repairing| UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Tender,
            tile,
            hp: UnitKind::Tender.stats().max_hp,
            idle,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing,
            grounded: false,
        };
        units.push(tender(30, home.offset(5, 1), false, true));
        units.push(tender(31, home.offset(6, 1), true, false));
        units.push(UnitObs {
            id: UnitId(32),
            player: PlayerId(0),
            kind: UnitKind::Bombard,
            tile: home.offset(7, 1),
            hp: UnitKind::Bombard.stats().max_hp / 4,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        });
        units.sort_unstable_by_key(|unit| unit.id);
        let mut obs = obs_with(units);
        obs.scrap = UnitKind::Sentinel.stats().cost;
        obs.my_buildings = vec![standing_building(10, BuildingKind::Foundry, home)];
        obs.my_queues = vec![vec![UnitKind::Harvester, UnitKind::Harvester]];
        let mut enemy = standing_building(90, BuildingKind::Foundry, TilePos::new(25, 8));
        enemy.player = PlayerId(1);
        obs.enemy_buildings.push(enemy);
        let map = corridor_briefing(home, TilePos::new(29, 10), '.');
        let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 20_042)
            .resolve_profile();
        let mut dials =
            Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
        dials.extractors = false;

        let intents = UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &map);

        let stops: Vec<_> = intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::StopUnits { units } => Some(units.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(stops, [vec![UnitId(30)]]);
        assert!(
            intents
                .iter()
                .all(|intent| !matches!(intent, Intent::RepairUnits { .. }))
        );
    }

    #[test]
    fn legacy_air_harass_waits_for_the_projected_opening_core() {
        let home = TilePos::new(2, 8);
        let target = TilePos::new(20, 8);
        let wing_kind = crate::stats::Role::AirGround.unit_for(crate::Faction::Ferrous);
        let mut units: Vec<_> = (1..=4).map(|id| harvester(id, None)).collect();
        units.extend((10..12).map(|id| UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: wing_kind,
            tile: home.offset(i32::try_from(id - 9).expect("small fixture id"), 4),
            hp: wing_kind.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }));
        let mut obs = obs_with(units);
        obs.my_buildings = vec![standing_building(1, BuildingKind::Foundry, home)];
        obs.my_queues = vec![Vec::new()];
        obs.enemy_units = vec![UnitObs {
            tile: target,
            ..harvester(90, None)
        }];
        obs.enemy_units[0].player = PlayerId(1);
        let map = public_map(&obs);
        let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_201)
            .resolve_profile();
        let mut dials =
            Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
        dials.air_harass = true;
        dials.air_wing = 2;
        dials.extractors = false;

        let blocked = UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &map);
        assert!(
            blocked
                .iter()
                .all(|intent| !matches!(intent, Intent::RaidAir { .. }))
        );

        obs.my_units.extend((20..28).map(|id| {
            fighter(
                id,
                PlayerId(0),
                home.offset(i32::try_from(id - 20).expect("small fixture id"), 5),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        let admitted = UtilityPolicy::new().think_player_facing(&dials, &obs, &[], &[], &[], &map);
        assert!(admitted.contains(&Intent::RaidAir { target }));
    }

    fn fighter(id: u32, player: PlayerId, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player,
            kind: UnitKind::Sentinel,
            tile,
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }
    }

    fn standing_building(id: u32, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(0),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn has_supported_restoration(policy: &UtilityPolicy, obs: &Observation, home: TilePos) -> bool {
        policy
            .supported_frame_restoration_claim(
                obs,
                ConstructionContext::new(
                    home,
                    ConstructionClaims {
                        player_facing: true,
                        enlisted: &[],
                        reserved: &[],
                    },
                ),
            )
            .is_some()
    }

    #[test]
    fn between_shared_boundaries_only_current_danger_may_advance() {
        let home = TilePos::new(5, 5);
        let threat = TilePos::new(11, 5);
        let endangered = TilePos::new(16, 11);
        let mut obs = obs_with(
            (0..6)
                .map(|id| fighter(id, PlayerId(0), home.offset(id as i32 % 3, id as i32 / 3)))
                .chain(std::iter::once(UnitObs {
                    tile: endangered,
                    carrying: 4,
                    ..harvester(10, None)
                }))
                .collect(),
        );
        obs.scrap = 2_000;
        obs.known_scrap = vec![(TilePos::new(19, 11), 500)];
        obs.salvage_incidents = vec![endangered];
        obs.my_buildings = vec![standing_building(0, BuildingKind::Foundry, home)];
        obs.my_queues = vec![Vec::new()];
        obs.enemy_units = vec![fighter(20, PlayerId(1), threat)];
        let army = Army {
            id: crate::bot::ArmyId(0),
            members: (0..6).map(UnitId).collect(),
            state: ArmyState::Staging,
            staging: home,
            target: None,
            focus: None,
            progress: None,
            issued: None,
            bounces: 0,
        };

        for difficulty in [
            BotDifficulty::Standard,
            BotDifficulty::Veteran,
            BotDifficulty::Prime,
        ] {
            let tuning = DifficultyTuning::for_level(difficulty);
            assert!(tuning.cadence < super::super::difficulty::STRATEGIC_ADMISSION_CADENCE);
            obs.tick = tuning.cadence;
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 1_616_201).resolve_profile();
            let dials = Dials::scripted(&profile, tuning);
            let mut policy = UtilityPolicy::new();
            let urgent = policy.think_with_intelligence(
                &dials,
                &obs,
                std::slice::from_ref(&army),
                &army.members,
                StrategicUtilityContext::new(&[], &[], &[], &public_map(&obs), Vec::new()),
            );

            assert!(urgent.iter().any(|intent| matches!(
                intent,
                Intent::MoveUnits { units, .. } if units == &[UnitId(10)]
            )));
            assert!(urgent.iter().any(|intent| matches!(
                intent,
                Intent::PushArmy { army: id, target } if *id == army.id && *target == threat
            )));
            assert!(urgent.iter().any(|intent| matches!(
                intent,
                Intent::FormArmy { size, .. } if *size >= 6
            )));
            assert!(
                urgent.iter().all(|intent| matches!(
                    intent,
                    Intent::MoveUnits { .. } | Intent::PushArmy { .. } | Intent::FormArmy { .. }
                )),
                "{difficulty:?} admitted voluntary macro between shared boundaries: {urgent:?}"
            );
            assert_eq!(policy.bank_seen, 0);
            assert_eq!(policy.bank_grew_at, 0);
            assert!(!policy.desperate);
            assert!(policy.last_sent.is_empty());
            assert!(policy.pending_sites.is_empty());
            assert!(policy.dead_anchors.is_empty());
            assert_eq!(policy.scout, None);
            assert_eq!(policy.scout_leg, 0);
            assert_eq!(policy.scout_sent_at, 0);
            assert_eq!(policy.scouted_at, 0);
            assert_eq!(policy.harvesters_seen, 0);
            assert_eq!(policy.turrets_seen, 0);

            obs.tick = super::super::difficulty::STRATEGIC_ADMISSION_CADENCE;
            let admitted = policy.think_with_intelligence(
                &dials,
                &obs,
                std::slice::from_ref(&army),
                &army.members,
                StrategicUtilityContext::new(&[], &[], &[], &public_map(&obs), Vec::new()),
            );
            assert!(
                admitted.iter().any(|intent| matches!(
                    intent,
                    Intent::TrainAt {
                        kind: UnitKind::Harvester,
                        ..
                    }
                )),
                "{difficulty:?} did not resume voluntary macro at the shared boundary: {admitted:?}"
            );
        }
    }

    fn dials_for_traits(traits: PersonalityTraits) -> Dials {
        let profile = ResolvedProfile {
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Balanced,
            personality_seed: 0xD1A1_5EED,
            primary: Specialty::Air,
            secondary: Specialty::Siege,
            traits,
        };
        Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime))
    }

    fn strategy_surface(dials: &Dials) -> [bool; 16] {
        [
            dials.tech,
            dials.turret_response,
            dials.scouting,
            dials.fog_honest,
            dials.aa_response,
            dials.radar,
            dials.reclaimers,
            dials.repair,
            dials.air_harass,
            dials.salvage,
            dials.deep_tech,
            dials.extractors,
            dials.upgrades,
            dials.expansion,
            dials.ferry,
            dials.mines,
        ]
    }

    fn assert_only_expected_dials_change(
        low: &Dials,
        high: &Dials,
        normalize_expected: impl FnOnce(&mut Dials, &Dials),
    ) {
        assert_eq!(strategy_surface(low), strategy_surface(high));
        assert!(
            strategy_surface(low).into_iter().all(|enabled| enabled),
            "personality may rank a strategy but cannot remove it"
        );
        let mut normalized = high.clone();
        normalize_expected(&mut normalized, low);
        assert_eq!(
            &normalized, low,
            "the trait altered a dial outside its documented signature"
        );
    }

    fn set_visible(obs: &mut Observation, tile: TilePos, visible: bool) {
        assert!(tile.x >= 0 && tile.x < obs.map_width);
        assert!(tile.y >= 0 && tile.y < obs.map_height);
        let width = usize::try_from(obs.map_width).expect("test map width is nonnegative");
        let x = usize::try_from(tile.x).expect("test tile x is nonnegative");
        let y = usize::try_from(tile.y).expect("test tile y is nonnegative");
        obs.visible[y * width + x] = visible;
    }

    fn construction_route_observation(workers: &[(u32, TilePos)]) -> Observation {
        let mut obs = obs_with(
            workers
                .iter()
                .map(|(id, tile)| UnitObs {
                    tile: *tile,
                    ..harvester(*id, None)
                })
                .collect(),
        );
        obs.known_rock = (0..obs.map_height)
            .filter(|y| !matches!(*y, 4 | 16))
            .map(|y| TilePos::new(12, y))
            .collect();
        obs.blips = vec![TilePos::new(12, 4)];
        obs
    }

    fn build_route_is_safe(
        policy: &UtilityPolicy,
        obs: &Observation,
        unit: &UnitObs,
        anchor: TilePos,
    ) -> bool {
        crate::bot::routing::build_command_path_avoids(
            obs,
            unit,
            anchor,
            BuildingKind::Turret.base_stats().size,
            false,
            |tile| {
                policy.harvest_location_contested(tile)
                    || UtilityPolicy::source_has_known_danger(obs, tile, Some(&[]), Some(&[]))
            },
        )
    }

    #[test]
    fn builder_binding_uses_a_farther_worker_whose_exact_path_is_safe() {
        let anchor = TilePos::new(22, 5);
        let obs =
            construction_route_observation(&[(1, TilePos::new(5, 4)), (2, TilePos::new(5, 16))]);
        let policy = UtilityPolicy::new();
        assert!(
            !build_route_is_safe(&policy, &obs, &obs.my_units[0], anchor),
            "the nearer worker's canonical route crosses the upper danger gap"
        );
        assert!(
            build_route_is_safe(&policy, &obs, &obs.my_units[1], anchor),
            "the farther worker has a safe canonical route through the lower gap"
        );
        let mut intents = vec![Intent::Build {
            kind: BuildingKind::Turret,
            anchor,
        }];

        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);

        assert_eq!(
            intents,
            vec![Intent::BuildWith {
                builder: UnitId(2),
                kind: BuildingKind::Turret,
                anchor,
            }]
        );
    }

    #[test]
    fn builder_binding_drops_an_implicit_build_when_every_route_is_dangerous() {
        let anchor = TilePos::new(22, 5);
        let obs = construction_route_observation(&[(1, TilePos::new(5, 4))]);
        let policy = UtilityPolicy::new();
        assert!(!build_route_is_safe(
            &policy,
            &obs,
            &obs.my_units[0],
            anchor
        ));
        let mut intents = vec![Intent::Build {
            kind: BuildingKind::Turret,
            anchor,
        }];

        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);

        assert!(intents.is_empty());
    }

    #[test]
    fn builder_binding_moves_capital_construction_before_harvest_chores() {
        let anchor = TilePos::new(22, 5);
        let obs = construction_route_observation(&[
            (1, TilePos::new(5, 4)),
            (2, TilePos::new(5, 16)),
            (3, TilePos::new(4, 16)),
        ]);
        let policy = UtilityPolicy::new();
        let explicit = Intent::AssignHarvest {
            unit: UnitId(2),
            node: TilePos::new(4, 16),
        };
        let implicit = Intent::Build {
            kind: BuildingKind::Turret,
            anchor,
        };
        let bound = Intent::BuildWith {
            builder: UnitId(2),
            kind: BuildingKind::Turret,
            anchor,
        };

        for mut intents in [
            vec![explicit.clone(), implicit.clone()],
            vec![implicit.clone(), explicit.clone()],
        ] {
            policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);
            assert_eq!(intents, vec![bound.clone(), explicit.clone()]);
        }
    }

    #[test]
    fn builder_binding_removes_harvest_work_consumed_by_the_new_footprint() {
        let anchor = TilePos::new(22, 5);
        let safe_node = TilePos::new(4, 16);
        let obs = construction_route_observation(&[
            (1, TilePos::new(5, 4)),
            (2, TilePos::new(5, 16)),
            (3, TilePos::new(4, 16)),
        ]);
        let policy = UtilityPolicy::new();
        let conflicting_harvest = Intent::AssignHarvest {
            unit: UnitId(3),
            node: anchor,
        };
        let safe_harvest = Intent::AssignHarvest {
            unit: UnitId(1),
            node: safe_node,
        };
        let implicit_build = Intent::Build {
            kind: BuildingKind::Turret,
            anchor,
        };
        let exact_build = Intent::BuildWith {
            builder: UnitId(2),
            kind: BuildingKind::Turret,
            anchor,
        };

        for mut intents in [
            vec![
                conflicting_harvest.clone(),
                implicit_build,
                safe_harvest.clone(),
            ],
            vec![
                exact_build.clone(),
                conflicting_harvest,
                safe_harvest.clone(),
            ],
        ] {
            policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);

            assert_eq!(
                intents,
                vec![exact_build.clone(), safe_harvest.clone()],
                "construction must cancel only chores whose resource disappears under its footprint"
            );
        }
    }

    #[test]
    fn builder_binding_does_not_double_book_a_harvester_committed_to_repairs() {
        let anchor = TilePos::new(22, 5);
        let obs = construction_route_observation(&[
            (1, TilePos::new(5, 4)),
            (2, TilePos::new(5, 16)),
            (3, TilePos::new(4, 16)),
        ]);
        let policy = UtilityPolicy::new();
        let repair = Intent::RepairUnits {
            welders: vec![UnitId(2)],
            target: UnitId(1),
        };
        let build = Intent::Build {
            kind: BuildingKind::Turret,
            anchor,
        };
        let bound = Intent::BuildWith {
            builder: UnitId(3),
            kind: BuildingKind::Turret,
            anchor,
        };

        for (mut intents, expected) in [
            (
                vec![repair.clone(), build.clone()],
                vec![repair.clone(), bound.clone()],
            ),
            (
                vec![build.clone(), repair.clone()],
                vec![bound.clone(), repair.clone()],
            ),
        ] {
            policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);
            assert_eq!(
                intents, expected,
                "repair ownership must be independent of channel ordering"
            );
        }
    }

    #[test]
    fn builder_binding_claims_distinct_workers_for_multiple_builds() {
        let first_anchor = TilePos::new(22, 5);
        let second_anchor = TilePos::new(22, 8);
        let obs = construction_route_observation(&[
            (1, TilePos::new(5, 4)),
            (2, TilePos::new(5, 16)),
            (3, TilePos::new(4, 16)),
        ]);
        let policy = UtilityPolicy::new();
        let mut intents = vec![
            Intent::Build {
                kind: BuildingKind::Turret,
                anchor: first_anchor,
            },
            Intent::Build {
                kind: BuildingKind::Turret,
                anchor: second_anchor,
            },
        ];

        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);

        assert_eq!(
            intents,
            vec![
                Intent::BuildWith {
                    builder: UnitId(2),
                    kind: BuildingKind::Turret,
                    anchor: first_anchor,
                },
                Intent::BuildWith {
                    builder: UnitId(3),
                    kind: BuildingKind::Turret,
                    anchor: second_anchor,
                },
            ]
        );
    }

    #[test]
    fn builder_binding_rejects_the_same_think_build_that_seals_egress() {
        let foundry_anchor = TilePos::new(10, 8);
        let first_gap = TilePos::new(7, 8);
        let second_gap = TilePos::new(14, 9);
        let mut first_worker = harvester(1, None);
        first_worker.tile = TilePos::new(9, 7);
        let mut second_worker = harvester(2, None);
        second_worker.tile = TilePos::new(12, 10);
        let mut obs = obs_with(vec![first_worker, second_worker]);
        obs.my_buildings
            .push(standing_building(0, BuildingKind::Foundry, foundry_anchor));
        for y in 5..=12 {
            for x in 7..=14 {
                let anchor = TilePos::new(x, y);
                let perimeter = matches!(x, 7 | 14) || matches!(y, 5 | 12);
                if perimeter && !matches!(anchor, tile if tile == first_gap || tile == second_gap) {
                    let id = u32::try_from(obs.my_buildings.len()).expect("test wall fits u32");
                    obs.my_buildings
                        .push(standing_building(id, BuildingKind::Reclaimer, anchor));
                }
            }
        }

        let policy = UtilityPolicy::new();
        let first = (BuildingKind::Reclaimer, first_gap);
        let second = (BuildingKind::Reclaimer, second_gap);
        assert!(policy.preserves_ground_producer_egress(&obs, &[], first));
        assert!(policy.preserves_ground_producer_egress(&obs, &[], second));
        assert!(!policy.preserves_ground_producer_egress(&obs, &[first], second));

        let mut intents = vec![
            Intent::Build {
                kind: first.0,
                anchor: first.1,
            },
            Intent::Build {
                kind: second.0,
                anchor: second.1,
            },
        ];
        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);

        assert_eq!(intents.len(), 1);
        assert!(matches!(
            intents[0],
            Intent::BuildWith {
                kind: BuildingKind::Reclaimer,
                anchor,
                ..
            } if anchor == first_gap
        ));

        let mut exact_second = vec![
            Intent::Build {
                kind: first.0,
                anchor: first.1,
            },
            Intent::BuildWith {
                builder: UnitId(2),
                kind: second.0,
                anchor: second.1,
            },
        ];
        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut exact_second);
        assert_eq!(
            exact_second,
            vec![Intent::BuildWith {
                builder: UnitId(1),
                kind: BuildingKind::Reclaimer,
                anchor: first_gap,
            }],
            "an already-bound later build must not bypass the shared egress check"
        );

        let safe = Intent::BuildWith {
            builder: UnitId(2),
            kind: second.0,
            anchor: second.1,
        };
        let mut safe_exact = vec![safe.clone()];
        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut safe_exact);
        assert_eq!(
            safe_exact,
            vec![safe],
            "an individually safe exact build must survive the same egress gate"
        );
    }

    #[test]
    fn recovery_sweep_waits_out_the_incident_then_covers_each_clean_tile_once() {
        let center = TilePos::new(16, 10);
        let mut worker = harvester(1, None);
        worker.tile = center;
        worker.idle = false;
        worker.harvesting = Some(center.offset(1, 0));
        let mut obs = obs_with(vec![worker]);
        obs.visible.fill(false);
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick = 100;
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![center];
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert_eq!(
            policy.contested_harvest_regions,
            vec![ContestedHarvestRegion {
                center,
                last_evidence: 100,
                sweep_started_at: None,
            }]
        );
        assert_eq!(
            policy.contested_recon_target(&obs, TilePos::new(3, 10)),
            None,
            "the authoritative warning must expire before a scout enters the kill zone"
        );

        obs.salvage_incidents.clear();
        obs.tick += crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1;
        obs.known_peaks.push(center);
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert_eq!(
            policy
                .contested_recon_target(&obs, TilePos::new(3, 10))
                .map(|recon| recon.target),
            Some(center.offset(-1, -1)),
            "an unoccupiable peak at the incident center must advance to a deterministic unseen cell"
        );

        let mut looks = 0;
        while let Some(recon) = policy.contested_recon_target(&obs, TilePos::new(3, 10)) {
            assert!(
                looks < (CONTESTED_RECON_RADIUS * 2 + 1).pow(2),
                "the finite danger square must not create an unbounded recon loop"
            );
            set_visible(&mut obs, recon.target, true);
            obs.tick += 1;
            policy.refresh_contested_harvest_regions(&obs, None, None);
            looks += 1;
        }
        assert!(
            policy.contested_harvest_regions.is_empty(),
            "one recent clean sweep should reopen the region without a second cooldown"
        );
        assert!(looks > 1, "partial sight must not clear the region");
    }

    #[test]
    fn an_actual_kestrel_stamps_the_region_and_reopens_harvest_work() {
        let width = 52;
        let height = 20;
        let home = TilePos::new(2, 8);
        let center = TilePos::new(22, 10);
        let mut map = vec![".".repeat(width); height];
        map[usize::try_from(home.y).unwrap()].replace_range(
            usize::try_from(home.x).unwrap()..usize::try_from(home.x + 1).unwrap(),
            "1",
        );
        map[8].replace_range(47..48, "2");
        map[usize::try_from(center.y).unwrap()].replace_range(
            usize::try_from(center.x).unwrap()..usize::try_from(center.x + 1).unwrap(),
            "s",
        );
        let scenario = Scenario {
            name: "contested recovery flight".into(),
            seed: 41,
            map,
            players: vec![
                PlayerSpec {
                    name: "Ferrous".into(),
                    faction: crate::state::Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric".into(),
                    faction: crate::state::Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: vec![
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Harvester,
                    x: 6,
                    y: 11,
                },
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Kestrel,
                    x: 6,
                    y: 8,
                },
            ],
            buildings: Vec::new(),
            meta: None,
        };
        let public_map = PublicMapBriefing::from_scenario(&scenario).unwrap();
        let mut state = scenario.build().unwrap();
        let me = PlayerId(0);
        let harvester = state
            .units()
            .iter()
            .find(|unit| unit.player == me && unit.kind == UnitKind::Harvester)
            .unwrap()
            .id;
        let kestrel = state
            .units()
            .iter()
            .find(|unit| unit.player == me && unit.kind == UnitKind::Kestrel)
            .unwrap()
            .id;
        let starting_tile = state.unit(kestrel).unwrap().tile();
        let opposite_corners = [
            center.offset(-CONTESTED_RECON_RADIUS, -CONTESTED_RECON_RADIUS),
            center.offset(CONTESTED_RECON_RADIUS, CONTESTED_RECON_RADIUS),
        ];
        let initial = Observation::fog_honest(&state, me);
        assert!(
            opposite_corners.iter().all(|tile| !initial.visible(*tile)),
            "the recovery region must begin outside home vision"
        );

        let mut policy = UtilityPolicy::new();
        policy.contested_harvest_regions = vec![ContestedHarvestRegion {
            center,
            last_evidence: 0,
            sweep_started_at: None,
        }];
        let mut executive = Executive::new();
        let mut dials = Dials::full();
        dials.harvester_target = 1;
        dials.tech = false;
        dials.turret_response = false;
        dials.aa_response = false;
        dials.radar = false;
        dials.reclaimers = false;
        dials.repair = false;
        dials.air_harass = false;
        dials.salvage = false;
        let mut moved = false;
        let mut stamped_both_corners = false;
        let mut harvest_dispatched = false;

        for _ in 0..400 {
            let obs = Observation::fog_honest(&state, me);
            moved |= state.unit(kestrel).unwrap().tile() != starting_tile;
            stamped_both_corners |= opposite_corners.iter().all(|tile| obs.visible(*tile));
            let intents = policy.think_player_facing(&dials, &obs, &[], &[], &[], &public_map);
            let commands = executive.apply_with_reservations(me, &obs, &intents, &[]);
            harvest_dispatched |= commands.iter().any(|command| {
                matches!(
                    command.command,
                    Command::Harvest { ref units, node, .. }
                        if units.contains(&harvester) && node == center
                )
            });
            let report = state.tick(&commands);
            assert!(
                report.events.iter().all(|event| !matches!(
                    event,
                    crate::Event::CommandRejected { player, .. } if *player == me
                )),
                "the recovery controller emitted a rejected command: {:?}",
                report.events
            );
            if harvest_dispatched {
                break;
            }
        }

        assert!(moved, "the real Kestrel must leave its home position");
        assert!(
            stamped_both_corners,
            "the moving Kestrel must establish simultaneous current sight across the whole square"
        );
        assert!(
            policy.contested_harvest_regions.is_empty(),
            "complete current sight must clear the persistent quarantine"
        );
        assert!(
            harvest_dispatched,
            "the same policy must reopen the revealed scrap for its real Harvester"
        );
    }

    #[test]
    fn fresh_danger_holds_a_recalled_scout_until_home_then_starts_the_retry_delay() {
        let home = TilePos::new(3, 10);
        let center = TilePos::new(16, 10);
        let mut worker = harvester(1, None);
        worker.tile = center;
        worker.idle = false;
        worker.harvesting = Some(center.offset(1, 0));
        let mut scout = fighter(2, PlayerId(0), center.offset(-2, 0));
        scout.kind = UnitKind::Kestrel;
        scout.hp = UnitKind::Kestrel.stats().max_hp;
        scout.idle = false;
        let mut obs = obs_with(vec![worker, scout]);
        obs.visible.fill(false);
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick = 100;
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![center];
        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1;
        obs.salvage_incidents.clear();
        set_visible(&mut obs, center, true);
        policy.refresh_contested_harvest_regions(&obs, None, None);
        let recon = policy
            .contested_recon_target(&obs, home)
            .expect("the quiet region needs a recovery look");
        policy.scout = Some(UnitId(2));
        policy.scout_dispatch = Some((UnitId(2), home, recon.target));
        policy.contested_scout = Some((UnitId(2), center));

        obs.enemy_units
            .push(fighter(90, PlayerId(1), center.offset(2, 0)));
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);

        assert_eq!(policy.scout, None);
        assert_eq!(
            policy.retreating_contested_scout,
            Some(RetreatingContestedScout {
                unit: UnitId(2),
                order_dispatched: false,
            })
        );
        assert_eq!(policy.contested_recon_target(&obs, home), None);
        assert_eq!(
            policy.contested_recon_retry_at, 0,
            "the regional retry cooldown starts on safe return, not recall"
        );
        let mut intents = Vec::new();
        policy.retreat_contested_scout(&obs, home, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::MoveUnits {
                units: vec![UnitId(2)],
                goal: home,
            }],
            "danger must replace an in-flight recon order with an immediate retreat"
        );

        obs.my_units[1].idle = true;
        obs.tick += CONTESTED_RECON_RETRY_TICKS * 2;
        intents.clear();
        policy.retreat_contested_scout(&obs, home, &mut intents);
        assert_eq!(
            intents,
            vec![Intent::MoveUnits {
                units: vec![UnitId(2)],
                goal: home,
            }],
            "an idle scout still in the field must retry its retreat after any wall-clock delay"
        );
        assert_eq!(
            policy.retreating_contested_scout,
            Some(RetreatingContestedScout {
                unit: UnitId(2),
                order_dispatched: true,
            }),
            "timer expiry and an idle body cannot release a remote recovery scout"
        );

        obs.my_units[1].tile = home.offset(1, 0);
        obs.tick += 1;
        intents.clear();
        policy.retreat_contested_scout(&obs, home, &mut intents);
        assert!(intents.is_empty());
        assert_eq!(policy.retreating_contested_scout, None);
        let retry_at = obs.tick + CONTESTED_RECON_RETRY_TICKS;
        assert_eq!(policy.contested_recon_retry_at, retry_at);

        obs.enemy_units.clear();
        obs.tick = retry_at - 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert_eq!(policy.contested_recon_target(&obs, home), None);
        obs.tick = retry_at;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(
            policy.contested_recon_target(&obs, home).is_some(),
            "the danger abort must delay rather than permanently suppressing reconnaissance"
        );
    }

    #[test]
    fn a_no_progress_recon_target_recalls_and_reserves_its_scout_until_home() {
        let home = TilePos::new(3, 10);
        let center = TilePos::new(16, 10);
        let mut worker = harvester(1, None);
        worker.tile = center;
        worker.idle = false;
        worker.harvesting = Some(center.offset(1, 0));
        let mut scout = fighter(2, PlayerId(0), home.offset(2, 0));
        scout.kind = UnitKind::Kestrel;
        scout.hp = UnitKind::Kestrel.stats().max_hp;
        let mut obs = obs_with(vec![worker, scout]);
        obs.visible.fill(false);
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick = 100;
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![center];
        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick += crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1;
        obs.salvage_incidents.clear();
        set_visible(&mut obs, center, true);
        policy.refresh_contested_harvest_regions(&obs, None, None);
        let sweep_started = policy.contested_harvest_regions[0]
            .sweep_started_at
            .expect("the center sight starts a bounded sweep");
        let recon = policy
            .contested_recon_target(&obs, home)
            .expect("partial coverage has a deterministic next target");

        let mut issued = Vec::new();
        for _ in 0..20 {
            let mut intents = Vec::new();
            policy.scouting_with_public_map(&obs, home, Some(recon), None, &[], &mut intents);
            issued.extend(intents);
            obs.tick += super::super::difficulty::STRATEGIC_ADMISSION_CADENCE;
        }
        assert_eq!(
            issued,
            vec![Intent::Scout {
                unit: UnitId(2),
                to: recon.target,
            }],
            "an accepted but unproductive target gets one command, not one per think"
        );

        obs.tick = sweep_started + CONTESTED_RECON_SWEEP_TICKS + 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert_eq!(policy.scout, None);
        assert_eq!(policy.contested_scout, None);
        assert_eq!(
            policy.retreating_contested_scout,
            Some(RetreatingContestedScout {
                unit: UnitId(2),
                order_dispatched: false,
            })
        );
        assert_eq!(policy.contested_recon_target(&obs, home), None);

        let mut retreat = Vec::new();
        policy.retreat_contested_scout(&obs, home, &mut retreat);
        assert_eq!(
            retreat,
            vec![Intent::MoveUnits {
                units: vec![UnitId(2)],
                goal: home,
            }],
            "timing out an impossible target must replace it with one retreat"
        );
        retreat.clear();
        obs.tick += CONTESTED_RECON_RETRY_TICKS * 2;
        policy.retreat_contested_scout(&obs, home, &mut retreat);
        assert_eq!(
            retreat,
            vec![Intent::MoveUnits {
                units: vec![UnitId(2)],
                goal: home,
            }],
            "an idle remote scout retries the retreat instead of becoming eligible for other work"
        );
        assert!(policy.retreating_contested_scout.is_some());

        obs.my_units[1].tile = home.offset(1, 0);
        obs.tick += 1;
        retreat.clear();
        policy.retreat_contested_scout(&obs, home, &mut retreat);
        assert!(
            retreat.is_empty(),
            "arrival inside the home area completes the existing retreat"
        );
        assert_eq!(policy.retreating_contested_scout, None);
        assert_eq!(
            policy.contested_recon_retry_at,
            obs.tick + CONTESTED_RECON_RETRY_TICKS,
            "the bounded retry delay begins only after the scout is safe"
        );
    }

    #[test]
    fn distinct_incidents_stay_canonical_and_exact_repeats_renew_through_policy_think() {
        let left = TilePos::new(10, 10);
        let right = TilePos::new(16, 10);
        assert!(
            left.chebyshev(right) > crate::stats::HARVEST_INCIDENT_DANGER_RADIUS,
            "the initial incidents must establish two separate regions"
        );
        let mut snapshots = Vec::new();

        for initial in [vec![right, left], vec![left, right]] {
            let mut first = harvester(1, None);
            first.tile = left;
            first.idle = false;
            first.harvesting = Some(left);
            let mut second = harvester(2, None);
            second.tile = right;
            second.idle = false;
            second.harvesting = Some(right);
            let mut observation = obs_with(vec![first, second]);
            observation.visible.fill(false);
            observation.my_buildings = vec![standing_building(
                1,
                BuildingKind::Foundry,
                TilePos::new(3, 10),
            )];
            observation.my_queues = vec![Vec::new()];
            let mut policy = UtilityPolicy::new();
            policy.refresh_contested_harvest_regions(&observation, None, None);

            observation.tick = 100;
            observation.my_units[0].hp -= 1;
            observation.my_units[1].hp -= 1;
            observation.salvage_incidents = initial;
            policy.think_player_facing(
                &Dials::full(),
                &observation,
                &[],
                &[],
                &[],
                &public_map(&observation),
            );
            assert_eq!(policy.contested_harvest_regions.len(), 2);

            observation.tick = 200;
            observation.my_units[0].hp -= 1;
            observation.salvage_incidents = vec![left];
            policy.think_player_facing(
                &Dials::full(),
                &observation,
                &[],
                &[],
                &[],
                &public_map(&observation),
            );
            snapshots.push(policy.contested_harvest_regions.clone());
        }

        let expected = vec![
            ContestedHarvestRegion {
                center: left,
                last_evidence: 200,
                sweep_started_at: None,
            },
            ContestedHarvestRegion {
                center: right,
                last_evidence: 100,
                sweep_started_at: None,
            },
        ];
        assert_eq!(snapshots, vec![expected.clone(), expected]);
    }

    #[test]
    fn overlapping_incidents_preserve_their_full_union_after_warnings_expire() {
        let first = TilePos::new(10, 10);
        let second = TilePos::new(14, 10);
        let first_edge = first.offset(-CONTESTED_HARVEST_RADIUS, 0);
        let second_edge = second.offset(CONTESTED_HARVEST_RADIUS, 0);
        assert_eq!(
            first.chebyshev(second),
            crate::stats::HARVEST_INCIDENT_DANGER_RADIUS,
            "the second incident must overlap the first region without sharing its center"
        );

        let mut worker = harvester(1, None);
        worker.tile = first;
        worker.idle = false;
        worker.harvesting = Some(first);
        let mut obs = obs_with(vec![worker]);
        obs.visible.fill(false);
        let mut policy = UtilityPolicy::new();
        policy.refresh_contested_harvest_regions(&obs, None, None);

        obs.tick = 100;
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![first];
        policy.refresh_contested_harvest_regions(&obs, None, None);

        obs.tick += 1;
        obs.my_units[0].tile = second;
        obs.my_units[0].harvesting = Some(second);
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![second];
        policy.refresh_contested_harvest_regions(&obs, None, None);

        assert_eq!(
            policy.contested_harvest_regions,
            vec![
                ContestedHarvestRegion {
                    center: first,
                    last_evidence: 100,
                    sweep_started_at: None,
                },
                ContestedHarvestRegion {
                    center: second,
                    last_evidence: 101,
                    sweep_started_at: None,
                },
            ],
            "overlap must not discard either incident's distinct danger area"
        );

        obs.salvage_incidents.clear();
        obs.tick += crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);

        assert!(policy.harvest_location_contested(first_edge));
        assert!(policy.harvest_location_contested(second_edge));
        assert!(
            !policy.harvest_location_contested(second_edge.offset(1, 0)),
            "the durable quarantine should preserve the exact union without growing it"
        );

        for tile in UtilityPolicy::contested_region_tiles(&obs, first).collect::<Vec<_>>() {
            set_visible(&mut obs, tile, true);
        }
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);

        assert_eq!(
            policy
                .contested_harvest_regions
                .iter()
                .map(|region| region.center)
                .collect::<Vec<_>>(),
            vec![second],
            "clearing the first square must not clear the overlapping region's unseen outer side"
        );
        assert!(!policy.harvest_location_contested(first_edge));
        assert!(policy.harvest_location_contested(second));
        assert!(policy.harvest_location_contested(second_edge));

        for tile in UtilityPolicy::contested_region_tiles(&obs, second).collect::<Vec<_>>() {
            set_visible(&mut obs, tile, true);
        }
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);

        assert!(policy.contested_harvest_regions.is_empty());
        assert!(!policy.harvest_location_contested(second));
        assert!(!policy.harvest_location_contested(second_edge));
    }

    #[test]
    fn an_edge_region_clears_after_every_in_bounds_tile_is_seen_once() {
        let center = TilePos::new(0, 0);
        let mut worker = harvester(1, None);
        worker.tile = center;
        let mut obs = obs_with(vec![worker]);
        obs.map_width = CONTESTED_RECON_RADIUS + 1;
        obs.map_height = CONTESTED_RECON_RADIUS + 1;
        let cell_count = usize::try_from(obs.map_width * obs.map_height)
            .expect("test map dimensions are positive");
        obs.visible = vec![true; cell_count];
        obs.explored = vec![true; cell_count];
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.tick = 50;
        obs.my_units[0].hp -= 1;
        obs.salvage_incidents = vec![center];
        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.salvage_incidents.clear();
        obs.tick += crate::stats::HARVEST_INCIDENT_MEMORY_TICKS + 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(
            policy.contested_harvest_regions.is_empty(),
            "off-map cells around a corner incident must not become impossible evidence"
        );
    }

    #[test]
    fn contested_region_cap_evicts_oldest_evidence_with_position_ties() {
        let strictly_oldest = TilePos::new(30, 30);
        let tied_first = TilePos::new(8, 3);
        let tied_second = TilePos::new(12, 3);
        let mut policy = UtilityPolicy::new();
        policy.contested_harvest_regions = vec![
            ContestedHarvestRegion {
                center: strictly_oldest,
                last_evidence: 4,
                sweep_started_at: None,
            },
            ContestedHarvestRegion {
                center: tied_second,
                last_evidence: 5,
                sweep_started_at: None,
            },
            ContestedHarvestRegion {
                center: tied_first,
                last_evidence: 5,
                sweep_started_at: None,
            },
        ];
        policy
            .contested_harvest_regions
            .extend((0..crate::stats::HARVEST_INCIDENT_CAP - 1).map(|index| {
                ContestedHarvestRegion {
                    center: TilePos::new(100 + i32::try_from(index).unwrap() * 10, 20),
                    last_evidence: 6,
                    sweep_started_at: None,
                }
            }));
        assert_eq!(
            policy.contested_harvest_regions.len(),
            crate::stats::HARVEST_INCIDENT_CAP + 2
        );

        let mut scout = fighter(50, PlayerId(0), strictly_oldest);
        scout.kind = UnitKind::Kestrel;
        scout.hp = UnitKind::Kestrel.stats().max_hp;
        let mut obs = obs_with(vec![scout]);
        obs.map_width = 1_024;
        obs.map_height = 64;
        obs.visible = vec![false; 1_024 * 64];
        obs.explored = vec![true; 1_024 * 64];
        obs.tick = 100;
        policy.scout = Some(UnitId(50));
        policy.scout_dispatch = Some((UnitId(50), TilePos::new(3, 3), strictly_oldest));
        policy.contested_scout = Some((UnitId(50), strictly_oldest));
        policy.refresh_contested_harvest_regions(&obs, None, None);

        let centers: Vec<_> = policy
            .contested_harvest_regions
            .iter()
            .map(|region| region.center)
            .collect();
        assert_eq!(centers.len(), crate::stats::HARVEST_INCIDENT_CAP);
        assert!(!centers.contains(&strictly_oldest));
        assert!(
            !centers.contains(&tied_first),
            "after the strictly oldest region, equal-age evidence evicts by (y, x)"
        );
        assert!(centers.contains(&tied_second));
        assert_eq!(
            policy.retreating_contested_scout,
            Some(RetreatingContestedScout {
                unit: UnitId(50),
                order_dispatched: false,
            }),
            "evicting a capped recovery region must retain its attached scout for retreat"
        );
        assert!(
            centers
                .windows(2)
                .all(|pair| { (pair[0].y, pair[0].x) <= (pair[1].y, pair[1].x) })
        );
    }

    #[test]
    fn strategic_utility_context_requires_explicit_active_air_work() {
        let units = Vec::new();
        let buildings = Vec::new();
        let observation = obs_with(Vec::new());
        let public_map = public_map(&observation);
        let inactive =
            StrategicUtilityContext::new(&[], &units, &buildings, &public_map, Vec::new());
        assert_eq!(inactive.outstanding_air_production_ticks, None);

        let active = StrategicUtilityContext::new(&[], &units, &buildings, &public_map, Vec::new())
            .with_outstanding_air_production_ticks(4_801);
        assert_eq!(active.outstanding_air_production_ticks, Some(4_801));
    }

    #[test]
    fn active_air_capacity_owns_the_sole_builder_before_frame_restoration() {
        let home = TilePos::new(12, 16);
        let frame = home.offset(8, 0);
        let mut worker = harvester(1, None);
        worker.tile = home.offset(0, 4);
        let mut obs = obs_with(vec![worker]);
        obs.map_width = 48;
        obs.map_height = 32;
        obs.visible = vec![true; 48 * 32];
        obs.explored = obs.visible.clone();
        obs.scrap = 10_000;
        obs.known_frames = vec![frame];
        for (id, kind, anchor) in [
            (10, BuildingKind::Foundry, home),
            (11, BuildingKind::Fabricator, TilePos::new(2, 2)),
            (12, BuildingKind::Airworks, TilePos::new(10, 2)),
            (13, BuildingKind::Crucible, TilePos::new(18, 2)),
        ] {
            obs.my_buildings.push(standing_building(id, kind, anchor));
            obs.my_queues.push(Vec::new());
        }
        let mut dials = Dials::full();
        dials.deep_tech = true;
        dials.extractors = true;
        let mut policy = UtilityPolicy::new();
        assert!(has_supported_restoration(&policy, &obs, home));
        let capacity_site = policy
            .placement_near(&obs, BuildingKind::Airworks, home)
            .expect("the fixture must leave a legal second Airworks footprint");
        obs.my_units[0].tile = capacity_site.offset(-1, 0);
        let danger = policy.harvest_danger_projection(&obs, None, None);
        let mut candidate_builders = vec![&obs.my_units[0]];
        assert_eq!(
            policy.safe_implicit_builder(
                &obs,
                BuildingKind::Airworks,
                capacity_site,
                &mut candidate_builders,
                &danger,
                None,
            ),
            Some(UnitId(1)),
            "the sole worker must have a safe command path to the capacity site"
        );
        let mut production_budget = obs.scrap;
        let mut production_intents = Vec::new();
        policy.production_with_air_demand(
            &dials,
            &obs,
            ProductionContext::new(
                home,
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                Some(u64::from(crate::TICKS_PER_SECOND) * 120 + 1),
            ),
            &mut production_budget,
            &mut production_intents,
        );
        assert!(
            production_intents.iter().any(|intent| matches!(
                intent,
                Intent::BuildWith {
                    builder: UnitId(1),
                    kind: BuildingKind::Airworks,
                    ..
                }
            )),
            "the fixture must have an actionable capacity site: {production_intents:?}"
        );
        let public_map = public_map(&obs);
        let context = StrategicUtilityContext::new(&[], &[], &[], &public_map, Vec::new())
            .with_outstanding_air_production_ticks(u64::from(crate::TICKS_PER_SECOND) * 120 + 1);

        let mut intents = policy.think_with_intelligence(&dials, &obs, &[], &[], context);
        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);

        let builds: Vec<_> = intents
            .iter()
            .filter(|intent| matches!(intent, Intent::Build { .. } | Intent::BuildWith { .. }))
            .collect();
        assert!(
            matches!(
                builds.as_slice(),
                [Intent::BuildWith {
                    builder: UnitId(1),
                    kind: BuildingKind::Airworks,
                    ..
                }]
            ),
            "active capacity must retain the only worker and defer restoration: {intents:?}"
        );
        assert!(intents.iter().all(|intent| !matches!(
            intent,
            Intent::Build {
                kind: BuildingKind::Extractor,
                ..
            } | Intent::BuildWith {
                kind: BuildingKind::Extractor,
                ..
            }
        )));
    }

    #[test]
    fn supported_frame_restoration_preempts_the_same_workers_harvest_chore() {
        let home = TilePos::new(3, 8);
        let frame = home.offset(8, 0);
        let source = home.offset(2, 4);
        let mut worker = harvester(1, None);
        worker.tile = home.offset(1, 4);
        let mut units = vec![worker];
        units.extend((0..3).map(|index| {
            fighter(
                10 + index,
                PlayerId(0),
                home.offset(i32::try_from(index).unwrap(), 5),
            )
        }));
        let mut obs = obs_with(units);
        obs.scrap = BuildingKind::Extractor
            .base_stats()
            .construction
            .expect("Extractors have a construction price")
            .cost;
        obs.known_frames = vec![frame];
        obs.known_scrap = vec![(source, 500)];
        obs.my_buildings
            .push(standing_building(10, BuildingKind::Foundry, home));
        obs.my_queues.push(Vec::new());
        let mut dials = Dials::full();
        dials.extractors = true;
        let mut policy = UtilityPolicy::new();
        assert!(
            has_supported_restoration(&policy, &obs, home),
            "the fixture must offer an ordinary supported-frame restoration"
        );

        let mut intents = policy.think_with_intelligence(
            &dials,
            &obs,
            &[],
            &[],
            StrategicUtilityContext::new(&[], &[], &[], &public_map(&obs), Vec::new()),
        );
        assert!(intents.contains(&Intent::AssignHarvest {
            unit: UnitId(1),
            node: source,
        }));
        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);

        assert!(matches!(
            intents.first(),
            Some(Intent::BuildWith {
                builder: UnitId(1),
                kind: BuildingKind::Extractor,
                anchor,
            }) if *anchor == frame
        ));
        let commands = Executive::new().apply_with_reservations(PlayerId(0), &obs, &intents, &[]);
        assert!(commands.iter().any(|command| matches!(
            command,
            PlayerCommand {
                command: Command::Build {
                units,
                kind: BuildingKind::Extractor,
                anchor,
                ..
            },
            ..
            } if units == &[UnitId(1)] && *anchor == frame
        )));
        assert!(commands.iter().all(|command| !matches!(
            &command.command,
            Command::Harvest { units, .. } if units.contains(&UnitId(1))
        )));
    }

    #[test]
    fn deferred_exact_restoration_preserves_its_capital_and_builder() {
        let home = TilePos::new(3, 8);
        let frame = home.offset(8, 0);
        let mut founder = harvester(1, None);
        founder.tile = frame.offset(-1, 2);
        founder.repairing = true;
        let mut other_repairer = harvester(2, None);
        other_repairer.tile = home.offset(0, 4);
        other_repairer.repairing = true;
        let mut third = harvester(3, None);
        third.tile = home.offset(1, 4);
        let mut fourth = harvester(4, None);
        fourth.tile = home.offset(2, 4);
        let mut units = vec![founder, other_repairer, third, fourth];
        units.extend((0..3).map(|index| {
            fighter(
                20 + index,
                PlayerId(0),
                home.offset(i32::try_from(index).unwrap(), 5),
            )
        }));
        let mut obs = obs_with(units);
        obs.scrap = BuildingKind::Extractor
            .base_stats()
            .construction
            .expect("Extractors have a construction price")
            .cost;
        obs.known_frames = vec![frame];
        let mut foundry = standing_building(10, BuildingKind::Foundry, home);
        foundry.hp -= 10;
        obs.my_buildings.push(foundry);
        obs.my_queues.push(Vec::new());
        let (width, height) = BuildingKind::Extractor.base_stats().size;
        for dy in 0..height {
            for dx in 0..width {
                let tile = frame.offset(dx, dy);
                let index = (tile.y * obs.map_width + tile.x) as usize;
                obs.visible[index] = false;
            }
        }
        let mut dials = Dials::full();
        dials.extractors = true;
        dials.scouting = false;
        let mut policy = UtilityPolicy::new();
        assert!(
            has_supported_restoration(&policy, &obs, home),
            "the hidden but explored home frame must retain its ordinary restoration reserve"
        );

        let mut intents = policy.think_with_intelligence(
            &dials,
            &obs,
            &[],
            &[],
            StrategicUtilityContext::new(&[], &[], &[], &public_map(&obs), Vec::new()),
        );
        policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);
        let commands = Executive::new().apply_with_reservations(PlayerId(0), &obs, &intents, &[]);

        assert!(
            commands.iter().any(|command| matches!(
                &command.command,
                Command::Build {
                    units,
                    kind: BuildingKind::Extractor,
                    anchor,
                    defer: true,
                    ..
                } if units == &[UnitId(1)] && *anchor == frame
            )),
            "the exact deferred build must replace its founder's repair order: {intents:?} -> {commands:?}"
        );
        assert!(
            commands.iter().any(|command| matches!(
                &command.command,
                Command::Stop { units } if units == &[UnitId(2)]
            )),
            "other voluntary repairers must stop before they drain the deferred fund: {intents:?} -> {commands:?}"
        );
        assert!(commands.iter().all(|command| !matches!(
            command.command,
            Command::Repair { .. } | Command::RepairUnit { .. }
        )));
    }

    /// The site audit's deferred-found contract (fog placement Part
    /// B): an anchor whose founder is still walking is a site on its
    /// way — kept pending, never blacklisted — while the same anchor
    /// with no founder and no building is a refusal and earns the
    /// blacklist as it always did.
    #[test]
    fn a_walking_founder_defers_the_site_audits_verdict() {
        let anchor = TilePos::new(9, 4);

        let mut policy = UtilityPolicy::new();
        policy.pending_sites.push(anchor);
        let walking = obs_with(vec![harvester(0, Some((BuildingKind::Turret, anchor)))]);
        policy.audit_sites(&walking);
        assert!(
            policy.dead_anchors.is_empty(),
            "the audit blacklisted an anchor whose founder is still walking"
        );
        assert_eq!(
            policy.pending_sites,
            vec![anchor],
            "a walking claim's anchor must stay pending for a later audit"
        );

        let mut policy = UtilityPolicy::new();
        policy.pending_sites.push(anchor);
        let refused = obs_with(vec![harvester(0, None)]);
        policy.audit_sites(&refused);
        assert_eq!(
            policy.dead_anchors,
            vec![anchor],
            "with no founder and no building, the anchor was refused and \
             must be blacklisted exactly as before"
        );
        assert!(policy.pending_sites.is_empty());
    }

    #[test]
    fn an_unfinished_foundry_site_keeps_orphan_relief_alive() {
        let anchor = TilePos::new(9, 4);
        let mut obs = obs_with(vec![harvester(0, None)]);
        obs.my_buildings.push(BuildingObs {
            id: BuildingId(7),
            player: PlayerId(0),
            kind: BuildingKind::Foundry,
            anchor,
            hp: 1,
            built: false,
            seen: true,
            tier: 0,
        });
        obs.my_queues.push(Vec::new());

        let intents = UtilityPolicy::new().think_player_facing(
            &Dials::full(),
            &obs,
            &[],
            &[],
            &[],
            &public_map(&obs),
        );

        assert!(
            intents.contains(&Intent::Build {
                kind: BuildingKind::Foundry,
                anchor,
            }),
            "the last paid Foundry site must remain repairable while it keeps the seat alive: {intents:?}"
        );

        let eliminated = obs_with(vec![harvester(0, None)]);
        assert!(
            UtilityPolicy::new()
                .think_player_facing(
                    &Dials::full(),
                    &eliminated,
                    &[],
                    &[],
                    &[],
                    &public_map(&eliminated),
                )
                .is_empty(),
            "a seat with no completed or unfinished Foundry remains eliminated"
        );
    }

    /// A founder walking toward one anchor must not shield a different
    /// pending anchor from the audit.
    #[test]
    fn the_founder_shields_only_its_own_anchor() {
        let claimed = TilePos::new(9, 4);
        let refused = TilePos::new(15, 8);
        let mut policy = UtilityPolicy::new();
        policy.pending_sites.push(claimed);
        policy.pending_sites.push(refused);
        let obs = obs_with(vec![harvester(0, Some((BuildingKind::Turret, claimed)))]);
        policy.audit_sites(&obs);
        assert_eq!(policy.pending_sites, vec![claimed]);
        assert_eq!(policy.dead_anchors, vec![refused]);
    }

    #[test]
    fn an_automatic_upgrade_is_not_an_orphaned_site() {
        let mut obs = obs_with(Vec::new());
        let anchor = TilePos::new(9, 4);
        obs.my_buildings.push(BuildingObs {
            id: BuildingId(7),
            player: PlayerId(0),
            kind: BuildingKind::Turret,
            anchor,
            hp: 100,
            built: false,
            seen: true,
            tier: 1,
        });
        obs.my_queues.push(Vec::new());
        let mut policy = UtilityPolicy::new();
        let mut budget = 0;
        let mut intents = Vec::new();

        policy.construction(
            &Dials::full(),
            &obs,
            ConstructionContext::new(
                TilePos::new(2, 2),
                ConstructionClaims {
                    player_facing: false,
                    enlisted: &[],
                    reserved: &[],
                },
            ),
            &mut budget,
            &mut intents,
        );

        assert!(
            intents.iter().all(
                |intent| !matches!(intent, Intent::Build { anchor: site, .. } if *site == anchor)
            ),
            "a self-timed upgrade must not draft an orphan-relief worker"
        );
    }

    #[test]
    fn resolved_support_identity_builds_its_repair_bay_before_the_general_fallback() {
        let home = TilePos::new(3, 3);
        let mut obs = obs_with(vec![harvester(0, None)]);
        for (id, kind, anchor) in [
            (1, BuildingKind::Foundry, home),
            (2, BuildingKind::Fabricator, TilePos::new(8, 3)),
            (3, BuildingKind::Airworks, TilePos::new(13, 3)),
            (4, BuildingKind::Crucible, TilePos::new(18, 3)),
            (5, BuildingKind::Array, TilePos::new(23, 3)),
        ] {
            obs.my_buildings.push(standing_building(id, kind, anchor));
            obs.my_queues.push(Vec::new());
        }
        obs.scrap = 10_000;

        let dials = |seed| {
            let difficulty = BotDifficulty::Prime;
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, seed).resolve_profile();
            let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
            (profile, dials)
        };
        let (high_profile, high) = dials(20_042);
        let (low_profile, low) = dials(20_044);
        assert_eq!(
            (high_profile.primary, high.support_target),
            (Specialty::Support, 3)
        );
        assert_eq!(low.support_target, 1, "premise: {low_profile:?}");

        let construct = |dials: &Dials, world: &Observation| {
            let mut budget = world.scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().construction(
                dials,
                world,
                ConstructionContext::new(
                    home,
                    ConstructionClaims {
                        player_facing: true,
                        enlisted: &[],
                        reserved: &[],
                    },
                ),
                &mut budget,
                &mut intents,
            );
            intents
        };
        assert!(matches!(
            construct(&high, &obs).as_slice(),
            [Intent::Build {
                kind: BuildingKind::RepairBay,
                ..
            }]
        ));
        assert!(
            construct(&low, &obs).iter().all(|intent| !matches!(
                intent,
                Intent::Build {
                    kind: BuildingKind::RepairBay,
                    ..
                }
            )),
            "low Support must not inherit the early identity signature"
        );

        obs.tick = 6_000;
        assert!(matches!(
            construct(&low, &obs).as_slice(),
            [Intent::Build {
                kind: BuildingKind::RepairBay,
                ..
            }]
        ));
    }

    #[test]
    fn each_difficulty_recovers_its_opening_core_before_residual_support_spending() {
        let home = TilePos::new(2, 8);
        let mut obs = obs_with((1..=7).map(|id| harvester(id, None)).collect());
        obs.tick = 6_000;
        obs.my_units.extend((20..=23).map(|id| UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Sentinel,
            tile: home.offset(i32::try_from(id - 20).unwrap(), 4),
            hp: UnitKind::Sentinel.stats().max_hp,
            idle: true,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
            grounded: false,
        }));
        for (id, kind, anchor) in [
            (30, BuildingKind::Foundry, home),
            (31, BuildingKind::Foundry, TilePos::new(25, 8)),
            (32, BuildingKind::Fabricator, TilePos::new(2, 2)),
            (33, BuildingKind::Fabricator, TilePos::new(7, 2)),
            (34, BuildingKind::Airworks, TilePos::new(12, 2)),
            (35, BuildingKind::Airworks, TilePos::new(17, 2)),
            (36, BuildingKind::Crucible, TilePos::new(22, 2)),
            (37, BuildingKind::Crucible, TilePos::new(27, 2)),
            (38, BuildingKind::Array, TilePos::new(14, 13)),
        ] {
            obs.my_buildings.push(standing_building(id, kind, anchor));
            obs.my_queues.push(Vec::new());
        }
        let repair_cost = BuildingKind::RepairBay
            .base_stats()
            .construction
            .expect("Repair Bays have a construction price")
            .cost;
        let residual = UnitKind::Avalanche.stats().cost.saturating_mul(10);
        obs.scrap = repair_cost
            .saturating_add(TECH_RESERVE)
            .saturating_add(residual);

        let mut core_targets = Vec::new();
        for difficulty in BotDifficulty::ALL {
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 20_042).resolve_profile();
            let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
            let intents = UtilityPolicy::new().think_player_facing(
                &dials,
                &obs,
                &[],
                &[],
                &[],
                &public_map(&obs),
            );
            let ready_at_start =
                combat_core_status(&obs, &[], &[], u64::from(dials.minimum_core_equivalents)).ready;
            let support_committed = intents.iter().any(|intent| {
                matches!(
                    intent,
                    Intent::Build {
                        kind: BuildingKind::RepairBay,
                        ..
                    }
                )
            });
            assert_eq!(
                support_committed, ready_at_start,
                "a core-recovery decision cannot also buy optional support: {intents:?}"
            );
            assert!(intents.iter().all(|intent| {
                !matches!(intent, Intent::Upgrade { .. })
                    && (ready_at_start
                        || !matches!(
                            intent,
                            Intent::TrainAt {
                                kind,
                                ..
                            } if *kind != UnitKind::Sentinel
                        ))
            }));
            let trains: Vec<_> = intents
                .iter()
                .filter_map(|intent| match intent {
                    Intent::TrainAt { building, kind } => Some((*building, *kind)),
                    _ => None,
                })
                .collect();
            let spent =
                repair_cost.saturating_add(trains.iter().fold(0_u32, |total, (_, kind)| {
                    total.saturating_add(kind.stats().cost)
                }));
            assert!(
                spent <= obs.scrap,
                "{difficulty:?} overspent {spent}: {intents:?}"
            );
            let core = combat_core_status(
                &obs,
                &[],
                &intents,
                u64::from(dials.minimum_core_equivalents),
            );
            assert!(
                core.ready,
                "{difficulty:?} reached optional support spending below its core floor: {intents:?}"
            );
            core_targets.push(core.target_strength);

            if !ready_at_start {
                let mut next = obs.clone();
                for intent in &intents {
                    let Intent::TrainAt { building, kind } = intent else {
                        continue;
                    };
                    let index = next
                        .my_buildings
                        .iter()
                        .position(|candidate| candidate.id == *building)
                        .expect("the recovery order names a standing producer");
                    next.my_queues[index].push(*kind);
                }
                let continued = UtilityPolicy::new().think_player_facing(
                    &dials,
                    &next,
                    &[],
                    &[],
                    &[],
                    &public_map(&next),
                );
                assert!(
                    continued.iter().any(|intent| matches!(
                        intent,
                        Intent::Build {
                            kind: BuildingKind::RepairBay,
                            ..
                        }
                    )),
                    "{difficulty:?} did not reopen optional support after observing the recovered core: {continued:?}"
                );
            }
        }

        assert!(
            core_targets.windows(2).all(|pair| pair[0] <= pair[1]),
            "higher difficulty must not protect a smaller opening core: {core_targets:?}"
        );
    }

    #[test]
    fn team_watch_home_defenders_still_count_toward_the_opening_core() {
        let home = TilePos::new(2, 8);
        let mut obs = obs_with((100..104).map(|id| harvester(id, None)).collect());
        obs.my_units.extend((1..=8).map(|id| {
            fighter(
                id,
                PlayerId(0),
                home.offset(i32::try_from(id - 1).expect("small fixture id"), 4),
            )
        }));
        obs.my_units.extend((20..=22).map(|id| UnitObs {
            kind: UnitKind::Scuttler,
            hp: UnitKind::Scuttler.stats().max_hp,
            ..fighter(
                id,
                PlayerId(0),
                home.offset(i32::try_from(id - 20).expect("small fixture id"), 6),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.my_buildings = vec![standing_building(30, BuildingKind::Foundry, home)];
        obs.my_queues = vec![Vec::new()];
        obs.scrap = UnitKind::Sentinel.stats().cost;

        let all_team_reservations = [UnitId(1), UnitId(2), UnitId(20), UnitId(21), UnitId(22)];
        let outbound_relief = [UnitId(20), UnitId(21), UnitId(22)];
        assert!(combat_core_status(&obs, &outbound_relief, &[], 8).ready);
        assert!(
            !combat_core_status(&obs, &all_team_reservations, &[], 8).ready,
            "the fixture must detect accidentally subtracting the home watch"
        );

        let mut dials = Dials::full();
        dials.minimum_core_equivalents = 8;
        let map = public_map(&obs);
        let decide = |separate_core_exclusions| {
            let context =
                StrategicUtilityContext::new(&all_team_reservations, &[], &[], &map, Vec::new());
            let context = if separate_core_exclusions {
                context.with_combat_core_exclusions(&outbound_relief)
            } else {
                context
            };
            UtilityPolicy::new().think_with_intelligence(&dials, &obs, &[], &[], context)
        };

        let conflated = decide(false);
        assert!(
            conflated.iter().any(|intent| matches!(
                intent,
                Intent::TrainAt {
                    kind: UnitKind::Sentinel,
                    ..
                }
            )),
            "the control must expose the needless refill: {conflated:?}"
        );
        let separated = decide(true);
        assert!(
            separated.iter().all(|intent| !matches!(
                intent,
                Intent::TrainAt {
                    kind: UnitKind::Sentinel,
                    ..
                }
            )),
            "home defenders stay owned by Team but still satisfy the protected core: {separated:?}"
        );
    }

    #[test]
    fn tender_support_continues_while_harvesters_honor_a_construction_promise() {
        let home = TilePos::new(3, 3);
        let promised = TilePos::new(24, 14);
        let mut founder = harvester(1, Some((BuildingKind::Foundry, promised)));
        founder.tile = TilePos::new(15, 10);
        let mut worker_repairer = harvester(2, None);
        worker_repairer.idle = false;
        worker_repairer.repairing = true;
        let ground_unit = |id, kind: UnitKind, tile, hp, idle, repairing| UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind,
            tile,
            hp,
            idle,
            carrying: 0,
            harvesting: None,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing,
            grounded: false,
        };
        let active_tender = ground_unit(
            3,
            UnitKind::Tender,
            TilePos::new(7, 5),
            UnitKind::Tender.stats().max_hp,
            false,
            true,
        );
        let idle_tender = ground_unit(
            4,
            UnitKind::Tender,
            TilePos::new(8, 5),
            UnitKind::Tender.stats().max_hp,
            true,
            false,
        );
        let wounded = ground_unit(
            5,
            UnitKind::Sentinel,
            TilePos::new(9, 5),
            UnitKind::Sentinel.stats().max_hp / 4,
            false,
            false,
        );
        let mut obs = obs_with(vec![
            founder,
            worker_repairer,
            active_tender,
            idle_tender,
            wounded,
        ]);
        for (id, kind, anchor) in [
            (1, BuildingKind::Foundry, home),
            (2, BuildingKind::Fabricator, TilePos::new(8, 3)),
            (3, BuildingKind::Airworks, TilePos::new(13, 3)),
            (4, BuildingKind::Crucible, TilePos::new(18, 3)),
            (5, BuildingKind::Array, TilePos::new(23, 3)),
            (6, BuildingKind::RepairBay, TilePos::new(3, 8)),
        ] {
            obs.my_buildings.push(standing_building(id, kind, anchor));
            obs.my_queues.push(Vec::new());
        }
        let difficulty = BotDifficulty::Prime;
        let profile =
            BotConfig::scripted(difficulty, BotStance::Balanced, 20_042).resolve_profile();
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
        assert_eq!(
            (profile.primary, dials.support_target),
            (Specialty::Support, 3)
        );
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries have a construction price")
            .cost;
        let run = |world: &Observation| {
            UtilityPolicy::new().think_player_facing(
                &dials,
                world,
                &[],
                &[],
                &[],
                &public_map(world),
            )
        };

        obs.scrap = foundry_cost + UnitKind::Sentinel.stats().cost + 1_000;
        let deficient = run(&obs);
        assert!(
            deficient.iter().any(|intent| matches!(
                intent,
                Intent::StopUnits { units }
                    if units.contains(&UnitId(2)) && units.contains(&UnitId(3))
            )),
            "every voluntary welder must release scrap while the opening core remains deficient: {deficient:?}"
        );
        assert!(
            deficient
                .iter()
                .all(|intent| !matches!(intent, Intent::RepairUnits { .. }))
        );

        let mut ready = obs.clone();
        ready.my_units.extend((6..=7).map(|id| harvester(id, None)));
        ready.my_units.extend((20..28).map(|id| {
            ground_unit(
                id,
                UnitKind::Sentinel,
                home.offset(i32::try_from(id - 20).unwrap(), 5),
                UnitKind::Sentinel.stats().max_hp,
                true,
                false,
            )
        }));
        ready.my_units.sort_unstable_by_key(|unit| unit.id);
        let intents = run(&ready);
        assert!(intents.contains(&Intent::StopUnits {
            units: vec![UnitId(2)],
        }));
        assert!(
            intents.iter().all(|intent| !matches!(
                intent,
                Intent::StopUnits { units } if units.contains(&UnitId(3))
            )),
            "an active Tender may keep welding once the opening core is funded"
        );
        assert!(intents.contains(&Intent::RepairUnits {
            welders: vec![UnitId(4)],
            target: UnitId(5),
        }));

        obs.scrap = foundry_cost + UnitKind::Sentinel.stats().cost - 1;
        let lean = run(&obs);
        assert!(
            lean.iter()
                .all(|intent| !matches!(intent, Intent::RepairUnits { .. })),
            "the construction promise and fighting reserve still bound new Tender work"
        );
        assert!(lean.iter().any(|intent| matches!(
            intent,
            Intent::StopUnits { units } if units.contains(&UnitId(3))
        )));
    }

    #[test]
    fn building_repair_is_one_persistent_program() {
        let me = PlayerId(0);
        let mut state = crate::Scenario::skirmish().build().unwrap();
        let foundry_index = state
            .buildings
            .iter()
            .position(|building| building.player == me && building.kind == BuildingKind::Foundry)
            .expect("the skirmish has a player-zero Foundry");
        let foundry = state.buildings[foundry_index].id;
        state.buildings[foundry_index].hp = BuildingKind::Foundry.base_stats().max_hp / 2;
        state.players[usize::from(me.0)].scrap = 1_000;

        let mut policy = UtilityPolicy::new();
        let dials = Dials::full();
        let mut executive = Executive::new();

        let first_obs = Observation::omniscient(&state, me);
        let mut first_budget = first_obs.scrap;
        let mut first_intents = Vec::new();
        policy.repairs(
            &dials,
            &first_obs,
            PolicyMode {
                player_facing: false,
                admit_voluntary_macro: true,
                unit_contacts: None,
                building_contacts: None,
                public_map: None,
            },
            &mut first_budget,
            &mut first_intents,
        );
        assert_eq!(first_intents, vec![Intent::Repair { building: foundry }]);
        let first_commands = executive.apply(me, &first_obs, &first_intents);
        let welder = match first_commands.as_slice() {
            [
                PlayerCommand {
                    player,
                    command:
                        Command::Repair {
                            units,
                            building,
                            queue: false,
                        },
                },
            ] if *player == me && *building == foundry && units.len() == 1 => units[0],
            other => panic!("expected one building-repair command, got {other:?}"),
        };
        let report = state.tick(&first_commands);
        assert!(
            report
                .events
                .iter()
                .all(|event| !matches!(event, crate::Event::CommandRejected { .. })),
            "the initial repair command must be legal"
        );

        let persistent_obs = Observation::omniscient(&state, me);
        assert!(
            persistent_obs
                .my_units
                .iter()
                .any(|unit| unit.id == welder && unit.repairing),
            "the accepted command must remain visible as a persistent repair program"
        );
        let mut persistent_budget = persistent_obs.scrap;
        let mut persistent_intents = Vec::new();
        policy.repairs(
            &dials,
            &persistent_obs,
            PolicyMode {
                player_facing: false,
                admit_voluntary_macro: true,
                unit_contacts: None,
                building_contacts: None,
                public_map: None,
            },
            &mut persistent_budget,
            &mut persistent_intents,
        );
        assert!(
            persistent_intents.is_empty(),
            "a stable repair program must not emit another intent on the next think"
        );
        assert!(
            executive
                .apply(me, &persistent_obs, &persistent_intents)
                .is_empty()
        );

        state.tick(&[PlayerCommand {
            player: me,
            command: Command::Stop {
                units: vec![welder],
            },
        }]);
        let stopped_obs = Observation::omniscient(&state, me);
        assert!(
            stopped_obs
                .my_units
                .iter()
                .all(|unit| unit.id != welder || !unit.repairing),
            "the explicit stop must end the persistent repair program"
        );
        let mut resumed_budget = stopped_obs.scrap;
        let mut resumed_intents = Vec::new();
        policy.repairs(
            &dials,
            &stopped_obs,
            PolicyMode {
                player_facing: false,
                admit_voluntary_macro: true,
                unit_contacts: None,
                building_contacts: None,
                public_map: None,
            },
            &mut resumed_budget,
            &mut resumed_intents,
        );
        assert_eq!(
            resumed_intents,
            vec![Intent::Repair { building: foundry }],
            "once the old program ends, the still-wounded building may be assigned once again"
        );
    }

    #[test]
    fn foundry_commitment_counts_distinct_unpaid_sites_not_crewmates() {
        let first = TilePos::new(9, 4);
        let second = TilePos::new(15, 7);
        let already_paid = TilePos::new(3, 8);
        let mut obs = obs_with(vec![
            harvester(0, Some((BuildingKind::Foundry, first))),
            harvester(1, Some((BuildingKind::Foundry, first))),
            harvester(2, Some((BuildingKind::Foundry, second))),
            harvester(3, Some((BuildingKind::Foundry, already_paid))),
            harvester(4, Some((BuildingKind::Fabricator, TilePos::new(18, 4)))),
        ]);
        obs.my_buildings.push(BuildingObs {
            id: BuildingId(7),
            player: PlayerId(0),
            kind: BuildingKind::Foundry,
            anchor: already_paid,
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: false,
            seen: true,
            tier: 0,
        });
        obs.my_queues.push(Vec::new());
        let price = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("expansion Foundries have a price")
            .cost;
        let fabricator_price = BuildingKind::Fabricator
            .base_stats()
            .construction
            .expect("Fabricators have a price")
            .cost;

        assert_eq!(
            UtilityPolicy::deferred_construction_commitment(&obs),
            price * 2 + fabricator_price
        );
        assert_eq!(UtilityPolicy::projected_foundries(&obs).1, 2);
    }

    #[test]
    fn each_personality_axis_changes_only_its_documented_dials() {
        let baseline = PersonalityTraits {
            air: 40,
            siege: 40,
            support: 40,
            fortification: 40,
            greed: 40,
            guile: 40,
        };

        let mut low_traits = baseline;
        low_traits.air = 20;
        let mut high_traits = baseline;
        high_traits.air = 80;
        let low = dials_for_traits(low_traits);
        let high = dials_for_traits(high_traits);
        assert_eq!((low.air_wing, high.air_wing), (4, 2));
        assert_eq!((low.bomber_target, high.bomber_target), (1, 3));
        assert_only_expected_dials_change(&low, &high, |candidate, expected| {
            candidate.air_wing = expected.air_wing;
            candidate.bomber_target = expected.bomber_target;
        });

        low_traits = baseline;
        low_traits.siege = 44;
        high_traits = baseline;
        high_traits.siege = 80;
        let low = dials_for_traits(low_traits);
        let high = dials_for_traits(high_traits);
        assert_eq!((low.siege_target, high.siege_target), (1, 4));
        assert_only_expected_dials_change(&low, &high, |candidate, expected| {
            candidate.siege_target = expected.siege_target;
        });

        low_traits = baseline;
        low_traits.support = 34;
        high_traits = baseline;
        high_traits.support = 80;
        let low = dials_for_traits(low_traits);
        let high = dials_for_traits(high_traits);
        assert_eq!((low.support_target, high.support_target), (1, 3));
        assert_eq!((low.flak_cap, high.flak_cap), (1, 3));
        assert_only_expected_dials_change(&low, &high, |candidate, expected| {
            candidate.support_target = expected.support_target;
            candidate.flak_cap = expected.flak_cap;
        });

        low_traits = baseline;
        low_traits.fortification = 24;
        high_traits = baseline;
        high_traits.fortification = 80;
        let low = dials_for_traits(low_traits);
        let high = dials_for_traits(high_traits);
        assert_eq!((low.turret_cap, high.turret_cap), (1, 4));
        assert_eq!((low.mine_cap, high.mine_cap), (2, 3));
        assert_eq!((low.barricade_cap, high.barricade_cap), (0, 2));
        assert_only_expected_dials_change(&low, &high, |candidate, expected| {
            candidate.turret_cap = expected.turret_cap;
            candidate.mine_cap = expected.mine_cap;
            candidate.barricade_cap = expected.barricade_cap;
        });

        low_traits = baseline;
        low_traits.greed = 24;
        high_traits = baseline;
        high_traits.greed = 80;
        let low = dials_for_traits(low_traits);
        let high = dials_for_traits(high_traits);
        assert_eq!((low.harvester_target, high.harvester_target), (4, 6));
        assert_eq!((low.reclaimer_cap, high.reclaimer_cap), (1, 4));
        assert_eq!((low.foundry_cap, high.foundry_cap), (2, 4));
        assert_only_expected_dials_change(&low, &high, |candidate, expected| {
            candidate.harvester_target = expected.harvester_target;
            candidate.reclaimer_cap = expected.reclaimer_cap;
            candidate.foundry_cap = expected.foundry_cap;
        });

        low_traits = baseline;
        low_traits.guile = 19;
        high_traits = baseline;
        high_traits.guile = 80;
        let low = dials_for_traits(low_traits);
        let high = dials_for_traits(high_traits);
        assert_eq!((low.raider_target, high.raider_target), (2, 2));
        assert_eq!((low.mine_cap, high.mine_cap), (2, 3));
        assert_only_expected_dials_change(&low, &high, |candidate, expected| {
            candidate.mine_cap = expected.mine_cap;
        });
    }

    #[test]
    fn resolved_greed_changes_real_expansion_appetite_at_the_foundry_boundary() {
        let high_profile =
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 1_616_304)
                .resolve_profile();
        let low_profile =
            BotConfig::scripted(BotDifficulty::Standard, BotStance::Balanced, 1_616_305)
                .resolve_profile();
        assert_eq!(
            (high_profile.traits.greed, low_profile.traits.greed),
            (64, 40)
        );
        let tuning = DifficultyTuning::for_level(BotDifficulty::Standard);
        let low_dials = Dials::scripted(&low_profile, tuning);
        let high_dials = Dials::scripted(&high_profile, tuning);
        assert_eq!(low_dials.foundry_cap, 2, "premise: {low_profile:?}");
        assert_eq!(high_dials.foundry_cap, 3, "premise: {high_profile:?}");

        let mut obs = obs_with((0..7).map(|id| harvester(id, None)).collect());
        obs.my_units.extend((0..6).map(|index| {
            fighter(
                100 + index,
                PlayerId(0),
                TilePos::new(4 + i32::try_from(index).unwrap(), 12),
            )
        }));
        obs.scrap = 10_000;
        obs.tick = 2_000;
        let home = TilePos::new(2, 8);
        for (id, kind, anchor) in [
            (20, BuildingKind::Foundry, home),
            (21, BuildingKind::Foundry, TilePos::new(14, 8)),
            (22, BuildingKind::Fabricator, TilePos::new(2, 2)),
            (23, BuildingKind::Airworks, TilePos::new(7, 2)),
            (24, BuildingKind::Crucible, TilePos::new(12, 2)),
            (25, BuildingKind::Array, TilePos::new(17, 2)),
            (26, BuildingKind::RepairBay, TilePos::new(22, 2)),
        ] {
            obs.my_buildings.push(standing_building(id, kind, anchor));
            obs.my_queues.push(Vec::new());
        }
        let served_salvage = TilePos::new(5, 15);
        let unserved_frontier = TilePos::new(30, 17);
        obs.known_scrap = vec![(served_salvage, 300), (unserved_frontier, 800)];
        assert!(
            obs.my_buildings
                .iter()
                .filter(|building| building.kind == BuildingKind::Foundry)
                .all(|foundry| foundry.anchor.chebyshev(unserved_frontier) > EXPANSION_RADIUS),
            "premise: the rich salvage lies beyond both current Foundries"
        );

        let decide = |dials: &Dials| {
            let mut budget = obs.scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().construction(
                dials,
                &obs,
                ConstructionContext::new(
                    home,
                    ConstructionClaims {
                        player_facing: true,
                        enlisted: &[],
                        reserved: &[],
                    },
                ),
                &mut budget,
                &mut intents,
            );
            intents
        };
        let low_intents = decide(&low_dials);
        let high_intents = decide(&high_dials);
        assert!(
            low_intents.is_empty(),
            "the low-greed identity has already reached its two-Foundry appetite: {low_intents:?}"
        );
        let [
            Intent::BuildWith {
                kind: BuildingKind::Foundry,
                anchor,
                ..
            },
        ] = high_intents.as_slice()
        else {
            panic!("the high-greed identity should claim the unserved frontier: {high_intents:?}");
        };
        assert!(
            anchor.chebyshev(unserved_frontier) < anchor.chebyshev(served_salvage),
            "the expansion must serve the remote economic objective"
        );

        let high_commands =
            Executive::new().apply_with_reservations(PlayerId(0), &obs, &high_intents, &[]);
        assert!(high_commands.iter().any(|command| matches!(
            command.command,
            Command::Build {
                kind: BuildingKind::Foundry,
                anchor: command_anchor,
                ..
            } if command_anchor == *anchor
        )));
        assert!(
            Executive::new()
                .apply_with_reservations(PlayerId(0), &obs, &low_intents, &[])
                .is_empty(),
            "the lower appetite must not be reintroduced during command lowering"
        );
    }

    #[test]
    fn scripted_identity_changes_bounded_priorities_not_the_strategy_surface() {
        let mut siege_targets = std::collections::BTreeSet::new();
        let mut support_targets = std::collections::BTreeSet::new();
        for stance in BotStance::ALL {
            for seed in 0..2_000 {
                let profile =
                    BotConfig::scripted(BotDifficulty::Prime, stance, seed).resolve_profile();
                let dials =
                    Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
                assert!((4..=7).contains(&dials.harvester_target));
                assert!((2..=4).contains(&dials.air_wing));
                assert!((1..=4).contains(&dials.bomber_target));
                assert!((1..=4).contains(&dials.siege_target));
                assert!((1..=3).contains(&dials.support_target));
                assert_eq!(dials.raider_target, 2);
                assert!((1..=4).contains(&dials.turret_cap));
                assert!((1..=3).contains(&dials.flak_cap));
                assert!((1..=4).contains(&dials.reclaimer_cap));
                assert!((1..=5).contains(&dials.mine_cap));
                assert!(dials.barricade_cap <= 3);
                assert!((2..=4).contains(&dials.foundry_cap));
                assert!(dials.tech && dials.deep_tech && dials.scouting);
                assert!(dials.repair && dials.aa_response && dials.turret_response);
                assert!(dials.expansion && dials.extractors && dials.reclaimers);
                assert!(dials.air_harass && dials.ferry && dials.mines);
                siege_targets.insert(dials.siege_target);
                support_targets.insert(dials.support_target);
            }
        }
        assert_eq!(siege_targets, [1, 2, 3, 4].into_iter().collect());
        assert_eq!(support_targets, [1, 2, 3].into_iter().collect());
    }

    #[test]
    fn difficulty_changes_attention_and_risk_without_redealing_composition() {
        let prime_profile = BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            0x8000_0000_1234_5678,
        )
        .resolve_profile();
        let dials: Vec<_> = BotDifficulty::ALL
            .into_iter()
            .map(|difficulty| {
                Dials::scripted(&prime_profile, DifficultyTuning::for_level(difficulty))
            })
            .collect();
        for pair in dials.windows(2) {
            let [lower, higher] = pair else {
                unreachable!()
            };
            assert!(lower.own_strength_scale <= higher.own_strength_scale);
            assert_eq!(lower.enemy_strength_scale, higher.enemy_strength_scale);
            assert!(lower.opponent_force_memory <= higher.opponent_force_memory);
            assert!(!lower.coordinated_focus || higher.coordinated_focus);
            assert!(!lower.coordinated_defense_focus || higher.coordinated_defense_focus);
        }
        let mut scrapheap = dials[0].clone();
        let prime = &dials[3];
        assert!(scrapheap.cadence > prime.cadence);
        assert!(scrapheap.discretionary_slots < prime.discretionary_slots);
        assert!(scrapheap.own_strength_scale < prime.own_strength_scale);
        assert_eq!(scrapheap.enemy_strength_scale, prime.enemy_strength_scale);
        assert!(scrapheap.opponent_force_memory < prime.opponent_force_memory);
        assert!(!scrapheap.coordinated_focus);
        assert!(prime.coordinated_focus);
        assert!(!scrapheap.coordinated_defense_focus);
        assert!(prime.coordinated_defense_focus);
        scrapheap.cadence = prime.cadence;
        scrapheap.discretionary_slots = prime.discretionary_slots;
        scrapheap.own_strength_scale = prime.own_strength_scale;
        scrapheap.enemy_strength_scale = prime.enemy_strength_scale;
        scrapheap.opponent_force_memory = prime.opponent_force_memory;
        scrapheap.coordinated_focus = prime.coordinated_focus;
        scrapheap.coordinated_defense_focus = prime.coordinated_defense_focus;
        scrapheap.minimum_core_equivalents = prime.minimum_core_equivalents;
        assert_eq!(&scrapheap, prime);
    }

    #[test]
    fn personality_changes_style_but_not_private_strength_estimates() {
        for difficulty in BotDifficulty::ALL {
            let profiles = [1_616_300, 1_616_301].map(|seed| {
                BotConfig::scripted(difficulty, BotStance::Balanced, seed).resolve_profile()
            });
            assert_ne!(
                profiles[0].traits, profiles[1].traits,
                "the fixture needs two distinct {difficulty:?} identities"
            );

            let dials = profiles
                .each_ref()
                .map(|profile| Dials::scripted(profile, DifficultyTuning::for_level(difficulty)));
            assert_eq!(
                dials[0].own_strength_scale, dials[1].own_strength_scale,
                "personality changed {difficulty:?} own-strength competence"
            );
            assert_eq!(
                dials[0].enemy_strength_scale, dials[1].enemy_strength_scale,
                "personality changed {difficulty:?} hostile-strength competence"
            );
        }
    }
}
