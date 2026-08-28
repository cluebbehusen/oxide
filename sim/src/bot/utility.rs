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
use crate::ids::UnitId;
use crate::scenario::BotStance;
use crate::stats::{BuildingKind, Domain, UnitKind};
use chassis::grid::TilePos;

mod combat;
mod construction;
mod danger;
mod economy;
mod production;
mod support;
mod terrain;

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
/// A loss quarantines source anchors beyond the actual sight-check footprint:
/// the source is only one endpoint, and its workers still have to traverse the
/// battlefield around it. The incident danger margin keeps those routes from
/// skimming straight back through the same kill zone.
const CONTESTED_HARVEST_RADIUS: i32 =
    crate::stats::HARVEST_ZONE_RADIUS + crate::stats::HARVEST_INCIDENT_DANGER_RADIUS;
/// One dedicated scout can positively clear the complete suspected work zone.
const CONTESTED_RECON_RADIUS: i32 = crate::stats::HARVEST_ZONE_RADIUS;
/// Require an uninterrupted clear look before reopening a work zone. A scout
/// merely passing between recurring raider sweeps is not useful evidence that
/// the route is safe again.
const CONTESTED_CLEAR_CONFIRM_TICKS: u64 = crate::stats::HARVEST_INCIDENT_MEMORY_TICKS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContestedHarvestRegion {
    center: TilePos,
    last_evidence: u64,
    clear_since: Option<u64>,
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

#[derive(Clone, Copy)]
struct ProductionContext<'a> {
    home: TilePos,
    claims: ConstructionClaims<'a>,
    outstanding_air_production_ticks: Option<u64>,
}

struct AdvancedConstructionContext<'a> {
    home: TilePos,
    player_facing: bool,
    builders: &'a [&'a UnitObs],
    reserved: &'a [UnitId],
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
            air_wing: AIR_WING,
            bomber_target: 2,
            siege_target: 2,
            support_target: 1,
            raider_target: 4,
            turret_cap: TURRET_CAP,
            flak_cap: FLAK_CAP,
            reclaimer_cap: RECLAIMER_CAP,
            mine_cap: MINE_CAP,
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
            air_wing: AIR_WING,
            bomber_target: 2,
            siege_target: 2,
            support_target: 1,
            raider_target: 4,
            turret_cap: TURRET_CAP,
            flak_cap: FLAK_CAP,
            reclaimer_cap: RECLAIMER_CAP,
            mine_cap: MINE_CAP,
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
    /// Extractors, upgrades, expansions, transports, and mines.
    pub fn overseer() -> Self {
        Self {
            deep_tech: true,
            extractors: true,
            upgrades: true,
            expansion: true,
            ferry: true,
            mines: true,
            harvester_target: 5,
            ..Self::full()
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
    /// Set when a harvester died on this watch; cleared when a turret
    /// stands (not when the command is emitted — commands can bounce).
    raided: bool,
    /// Turret count at the last think.
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
    /// A ground scout proved that reconnaissance needs an aircraft.
    /// This keeps one scout-role flyer alive or queued so lower-priority
    /// purchases cannot strand an island seat behind its own shoreline.
    air_scout_needed: bool,
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
    /// Whether enemy air has ever been sighted — the sky stays suspect
    /// afterward.
    seen_air: bool,
    /// Riders sent to board the profile-free Overseer's ferry on its last
    /// Load. The player-facing controller owns transport waves in its
    /// persistent strategic planner instead.
    ferry_boarding: Vec<UnitId>,
    /// Player-facing controller memory for work regions where allied losses
    /// made anonymous salvage unsafe. Authoritative incident warnings seed
    /// this bounded ledger; elapsed time alone never proves a mobile threat
    /// left, so only fresh clear sight releases a region.
    contested_harvest_regions: Vec<ContestedHarvestRegion>,
    /// Workers already sent out of a contested work region. This avoids
    /// replacing the same escape route every think while still retrying a
    /// bounced evacuation once the unit becomes idle.
    evacuating_workers: Vec<UnitId>,
}

struct ThinkContext<'a> {
    armies: &'a [Army],
    enlisted: &'a [UnitId],
    reserved: &'a [UnitId],
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
}

pub(super) struct StrategicUtilityContext<'a> {
    reserved: &'a [UnitId],
    unit_contacts: &'a [UnitContact],
    building_contacts: &'a [BuildingContact],
    outstanding_air_production_ticks: Option<u64>,
    prelude: Vec<Intent>,
}

impl<'a> StrategicUtilityContext<'a> {
    pub(super) fn new(
        reserved: &'a [UnitId],
        unit_contacts: &'a [UnitContact],
        building_contacts: &'a [BuildingContact],
        prelude: Vec<Intent>,
    ) -> Self {
        Self {
            reserved,
            unit_contacts,
            building_contacts,
            outstanding_air_production_ticks: None,
            prelude,
        }
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
            Self::claim_explicit_intent_units(intent, &mut claimed);
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
                    let size = kind.base_stats().size;
                    let (width, height) = size;
                    let defer = (0..height)
                        .any(|dy| (0..width).any(|dx| !obs.visible(anchor.offset(dx, dy))));
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
                    candidates.sort_unstable_by_key(|unit| (unit.tile.manhattan(anchor), unit.id));
                    let builder = candidates.into_iter().find(|unit| {
                        crate::bot::routing::build_command_path_avoids(
                            obs,
                            unit,
                            anchor,
                            size,
                            defer,
                            |tile| {
                                self.harvest_location_contested(tile)
                                    || danger
                                        .as_ref()
                                        .expect("an implicit Build prepared worker danger")
                                        .contains(tile)
                            },
                        )
                    });
                    if let Some(builder) = builder {
                        claimed.push(builder.id);
                        accepted_builds.push((kind, anchor));
                        bound.push(Intent::BuildWith {
                            builder: builder.id,
                            kind,
                            anchor,
                        });
                    }
                }
                intent @ Intent::BuildWith { kind, anchor, .. } => {
                    if self
                        .preserves_ground_producer_egress_prepared(&accepted_builds, (kind, anchor))
                    {
                        accepted_builds.push((kind, anchor));
                        bound.push(intent);
                    }
                }
                intent => bound.push(intent),
            }
        }
        *intents = bound;
    }

    fn claim_explicit_intent_units(intent: &Intent, claimed: &mut Vec<UnitId>) {
        match intent {
            Intent::MoveUnits { units, .. }
            | Intent::AttackMoveUnits { units, .. }
            | Intent::AttackUnits { units, .. }
            | Intent::StopUnits { units } => claimed.extend(units.iter().copied()),
            Intent::RepairUnits { welders, .. } => claimed.extend(welders.iter().copied()),
            Intent::AssignHarvest { unit, .. } | Intent::Scout { unit, .. } => {
                claimed.push(*unit);
            }
            Intent::BuildWith { builder, .. } => claimed.push(*builder),
            Intent::Load { transport, riders } => {
                claimed.push(*transport);
                claimed.extend(riders.iter().copied());
            }
            Intent::Unload { transport, .. } => claimed.push(*transport),
            Intent::TrainAt { .. }
            | Intent::Build { .. }
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
        Self::deferred_claims(obs)
            .into_iter()
            .map(|(kind, _)| {
                kind.base_stats()
                    .construction
                    .map_or(0, |construction| construction.cost)
            })
            .fold(0, u32::saturating_add)
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
                outstanding_air_production_ticks: None,
                prelude: Vec::new(),
                mode: PolicyMode {
                    player_facing: false,
                    admit_voluntary_macro: true,
                    unit_contacts: None,
                    building_contacts: None,
                },
            },
        )
    }

    /// One player-facing think after higher-level playbooks have claimed their
    /// ordered intents, without controller-level strategic intelligence.
    pub fn think_with_prelude(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        enlisted: &[UnitId],
        reserved: &[UnitId],
        prelude: Vec<Intent>,
    ) -> Vec<Intent> {
        self.think_inner(
            dials,
            obs,
            ThinkContext {
                armies,
                enlisted,
                reserved,
                outstanding_air_production_ticks: None,
                prelude,
                mode: PolicyMode {
                    player_facing: true,
                    admit_voluntary_macro: strategic_admission_tick(obs.tick),
                    unit_contacts: None,
                    building_contacts: None,
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
                outstanding_air_production_ticks: context.outstanding_air_production_ticks,
                prelude: context.prelude,
                mode: PolicyMode {
                    player_facing: true,
                    admit_voluntary_macro: strategic_admission_tick(obs.tick),
                    unit_contacts: Some(context.unit_contacts),
                    building_contacts: Some(context.building_contacts),
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
            outstanding_air_production_ticks,
            prelude,
            mode,
        } = context;
        let player_facing = mode.player_facing;
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
            self.refresh_contested_harvest_regions(obs, mode.unit_contacts, mode.building_contacts);
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
        self.audit_raids(obs);

        let construction_commitment = Self::deferred_construction_commitment(obs);
        let mut spendable = obs.clone();
        if player_facing {
            spendable.scrap = spendable.scrap.saturating_sub(construction_commitment);
        }
        let obs = &spendable;
        let mut budget = obs.scrap;

        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .count();
        let contested_recon = player_facing
            .then(|| self.contested_recon_target(home_tile))
            .flatten();
        if player_facing
            && dials.scouting
            && (harvesters >= immediate_harvester_target(dials) as usize
                || contested_recon.is_some())
        {
            // Exact scout ownership precedes every implicit utility claim.
            let mut unavailable = enlisted.to_vec();
            unavailable.extend_from_slice(reserved);
            unavailable.sort_unstable();
            unavailable.dedup();
            self.scouting(obs, home_tile, contested_recon, &unavailable, &mut intents);
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
                home_tile,
                construction_claims,
                &mut construction_budget,
                &mut planned_construction,
            );
            budget = construction_budget;
        }
        if outstanding_air_production_ticks.is_some() {
            self.production_with_air_demand(
                dials,
                obs,
                ProductionContext {
                    home: home_tile,
                    claims: construction_claims,
                    outstanding_air_production_ticks,
                },
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
        } else {
            self.construction(
                dials,
                obs,
                home_tile,
                construction_claims,
                &mut budget,
                &mut intents,
            );
        }
        let construction_promised = construction_commitment > 0
            || intents.iter().any(|intent| match intent {
                Intent::Build { kind, anchor } => {
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
            });
        if player_facing && construction_promised {
            // A building crew's voluntary repair program may be preempted so
            // its promised construction can proceed. Dedicated Tenders keep
            // welding while the bank has scrap beyond the deferred claim, but
            // must stop before repair can consume the claim itself.
            let repairers: Vec<UnitId> = obs
                .my_units
                .iter()
                .filter(|unit| {
                    unit.repairing && (unit.kind.stats().harvest.is_some() || obs.scrap == 0)
                })
                .map(|unit| unit.id)
                .collect();
            if !repairers.is_empty() {
                let before_build = intents
                    .iter()
                    .position(|intent| matches!(intent, Intent::Build { .. }))
                    .unwrap_or(0);
                intents.insert(before_build, Intent::StopUnits { units: repairers });
            }
        } else {
            self.repairs(dials, obs, mode, &mut budget, &mut intents);
        }
        self.mobile_support(dials, obs, player_facing, &mut intents);
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
        self.air_raid(dials, obs, home_tile, enlisted, reserved, &mut intents);
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
        for &incident in &obs.salvage_incidents {
            if let Some(region) = self
                .contested_harvest_regions
                .iter_mut()
                .filter(|region| {
                    region.center.chebyshev(incident)
                        <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
                })
                .min_by_key(|region| {
                    (
                        region.center.chebyshev(incident),
                        region.center.y,
                        region.center.x,
                    )
                })
            {
                region.last_evidence = obs.tick;
                region.clear_since = None;
            } else {
                self.contested_harvest_regions.push(ContestedHarvestRegion {
                    center: incident,
                    last_evidence: obs.tick,
                    clear_since: None,
                });
            }
        }

        let danger = (!self.contested_harvest_regions.is_empty())
            .then(|| self.harvest_danger_projection(obs, unit_contacts, building_contacts));
        for region in &mut self.contested_harvest_regions {
            let active_incident = obs.salvage_incidents.iter().any(|incident| {
                incident.chebyshev(region.center) <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
            });
            let currently_clear = Self::harvest_region_currently_clear(
                obs,
                region.center,
                danger
                    .as_deref()
                    .expect("a contested region prepared worker danger"),
            );
            if active_incident || !currently_clear {
                region.clear_since = None;
                if !currently_clear {
                    region.last_evidence = obs.tick;
                }
            } else {
                region.clear_since.get_or_insert(obs.tick);
            }
        }
        self.contested_harvest_regions.retain(|region| {
            region.clear_since.is_none_or(|clear_since| {
                obs.tick.saturating_sub(clear_since) < CONTESTED_CLEAR_CONFIRM_TICKS
            })
        });
        while self.contested_harvest_regions.len() > crate::stats::HARVEST_INCIDENT_CAP {
            let evict = self
                .contested_harvest_regions
                .iter()
                .enumerate()
                .min_by_key(|(_, region)| (region.last_evidence, region.center.y, region.center.x))
                .map(|(index, _)| index)
                .expect("an over-cap contested-region ledger is nonempty");
            self.contested_harvest_regions.remove(evict);
        }
        self.contested_harvest_regions
            .sort_by_key(|region| (region.center.y, region.center.x));
    }

    fn harvest_region_currently_clear(
        obs: &Observation,
        center: TilePos,
        danger: &danger::HarvestDangerProjection,
    ) -> bool {
        let radius = CONTESTED_RECON_RADIUS;
        let whole_region_visible = (-radius..=radius).all(|dy| {
            (-radius..=radius).all(|dx| {
                let tile = center.offset(dx, dy);
                tile.x < 0
                    || tile.y < 0
                    || tile.x >= obs.map_width
                    || tile.y >= obs.map_height
                    || obs.visible(tile)
            })
        });
        whole_region_visible && !danger.contains_with_margin(center, radius)
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

    fn contested_recon_target(&self, home: TilePos) -> Option<TilePos> {
        self.contested_harvest_regions
            .iter()
            .map(|region| {
                (
                    region.center.chebyshev(home),
                    region.center.y,
                    region.center.x,
                )
            })
            .min()
            .map(|(_, y, x)| TilePos::new(x, y))
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
                self.scout = None;
                self.scout_dispatch = None;
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
        let mut routes = RouteProjection::known_ground(obs);
        let max_radius = obs.map_width.max(obs.map_height).max(0);
        for radius in 0..=max_radius {
            let mut best = None;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dy.abs()) != radius {
                        continue;
                    }
                    let tile = home.offset(dx, dy);
                    if !routing::ground_open(obs, tile)
                        || !obs.explored(tile)
                        || !self.evacuation_standing_area_safe(obs, tile, danger)
                        || !routes.unit_reaches(worker, tile)
                    {
                        continue;
                    }
                    let key = (worker.tile.manhattan(tile), tile.y, tile.x);
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

    /// A shrinking harvest line means raiders; remember until a turret
    /// actually stands.
    fn audit_raids(&mut self, obs: &Observation) {
        let harvesters = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().harvest.is_some())
            .count();
        if harvesters < self.harvesters_seen {
            self.raided = true;
        }
        self.harvesters_seen = harvesters;
        let turrets = obs
            .my_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Turret)
            .count();
        if turrets > self.turrets_seen {
            self.raided = false;
        }
        self.turrets_seen = turrets;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::Executive;
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION, Observation, UnitObs};
    use crate::bot::{PersonalityTraits, Specialty};
    use crate::ids::{BuildingId, PlayerId, UnitId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
    use crate::{Command, PlayerCommand};

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

    fn harvester(id: u32, founding: Option<(BuildingKind, TilePos)>) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind: UnitKind::Harvester,
            tile: TilePos::new(5, 5),
            hp: UnitKind::Harvester.stats().max_hp,
            idle: founding.is_none(),
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding,
            repairing: false,
        }
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
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
                StrategicUtilityContext::new(&[], &[], &[], Vec::new()),
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
                StrategicUtilityContext::new(&[], &[], &[], Vec::new()),
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
    fn builder_binding_respects_explicit_claims_before_and_after_a_build() {
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
            builder: UnitId(3),
            kind: BuildingKind::Turret,
            anchor,
        };

        for (mut intents, expected) in [
            (
                vec![explicit.clone(), implicit.clone()],
                vec![explicit.clone(), bound.clone()],
            ),
            (
                vec![implicit.clone(), explicit.clone()],
                vec![bound.clone(), explicit.clone()],
            ),
        ] {
            policy.bind_player_facing_builders(&obs, &[], &[], &[], &[], &mut intents);
            assert_eq!(intents, expected);
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
    fn interrupted_clear_sight_restarts_the_full_contested_region_timer() {
        let center = TilePos::new(16, 10);
        let obscured_tile = center.offset(CONTESTED_RECON_RADIUS, 0);
        let mut obs = obs_with(Vec::new());
        obs.tick = 100;
        obs.salvage_incidents = vec![center];
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert_eq!(
            policy.contested_harvest_regions,
            vec![ContestedHarvestRegion {
                center,
                last_evidence: 100,
                clear_since: None,
            }]
        );

        obs.salvage_incidents.clear();
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        let first_clear_since = obs.tick;
        assert_eq!(
            policy.contested_harvest_regions[0].clear_since,
            Some(first_clear_since)
        );

        obs.tick = first_clear_since + CONTESTED_CLEAR_CONFIRM_TICKS - 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert_eq!(policy.contested_harvest_regions.len(), 1);

        set_visible(&mut obs, obscured_tile, false);
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert_eq!(policy.contested_harvest_regions[0].clear_since, None);
        assert_eq!(
            policy.contested_harvest_regions[0].last_evidence, obs.tick,
            "one unseen in-bounds tile must renew uncertainty instead of completing the old timer"
        );

        set_visible(&mut obs, obscured_tile, true);
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        let restarted_at = obs.tick;
        assert_eq!(
            policy.contested_harvest_regions[0].clear_since,
            Some(restarted_at)
        );

        obs.tick = restarted_at + CONTESTED_CLEAR_CONFIRM_TICKS - 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert_eq!(
            policy.contested_harvest_regions.len(),
            1,
            "the almost-complete timer from before the sight break must not carry over"
        );

        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(
            policy.contested_harvest_regions.is_empty(),
            "a complete uninterrupted confirmation interval should reopen the region"
        );
    }

    #[test]
    fn overlapping_incidents_coalesce_and_renew_one_canonical_region_through_policy_think() {
        let left = TilePos::new(10, 10);
        let right = TilePos::new(16, 10);
        let midpoint = TilePos::new(13, 10);
        assert!(
            left.chebyshev(right) > crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
                && left.chebyshev(midpoint) == right.chebyshev(midpoint)
                && left.chebyshev(midpoint) <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
        );
        let mut snapshots = Vec::new();

        for initial in [vec![right, left], vec![left, right]] {
            let mut observation = obs_with(Vec::new());
            observation.my_buildings = vec![standing_building(
                1,
                BuildingKind::Foundry,
                TilePos::new(3, 10),
            )];
            observation.my_queues = vec![Vec::new()];
            observation.tick = 100;
            observation.salvage_incidents = initial;
            let mut policy = UtilityPolicy::new();

            policy.think_with_prelude(&Dials::full(), &observation, &[], &[], &[], Vec::new());
            assert_eq!(policy.contested_harvest_regions.len(), 2);

            observation.tick = 200;
            observation.salvage_incidents = vec![midpoint];
            policy.think_with_prelude(&Dials::full(), &observation, &[], &[], &[], Vec::new());
            snapshots.push(policy.contested_harvest_regions.clone());
        }

        let expected = vec![
            ContestedHarvestRegion {
                center: left,
                last_evidence: 200,
                clear_since: None,
            },
            ContestedHarvestRegion {
                center: right,
                last_evidence: 100,
                clear_since: None,
            },
        ];
        assert_eq!(snapshots, vec![expected.clone(), expected]);
    }

    #[test]
    fn an_edge_region_clears_after_every_in_bounds_tile_stays_visible() {
        let center = TilePos::new(0, 0);
        let mut obs = obs_with(Vec::new());
        obs.map_width = CONTESTED_RECON_RADIUS + 1;
        obs.map_height = CONTESTED_RECON_RADIUS + 1;
        let cell_count = usize::try_from(obs.map_width * obs.map_height)
            .expect("test map dimensions are positive");
        obs.visible = vec![true; cell_count];
        obs.explored = vec![true; cell_count];
        obs.tick = 50;
        obs.salvage_incidents = vec![center];
        let mut policy = UtilityPolicy::new();

        policy.refresh_contested_harvest_regions(&obs, None, None);
        obs.salvage_incidents.clear();
        obs.tick += 1;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert_eq!(
            policy.contested_harvest_regions[0].clear_since,
            Some(obs.tick),
            "off-map cells around a corner incident must not count as hidden"
        );

        obs.tick += CONTESTED_CLEAR_CONFIRM_TICKS;
        policy.refresh_contested_harvest_regions(&obs, None, None);
        assert!(
            policy.contested_harvest_regions.is_empty(),
            "continuous sight over the complete in-bounds corner should clear the warning"
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
                clear_since: None,
            },
            ContestedHarvestRegion {
                center: tied_second,
                last_evidence: 5,
                clear_since: None,
            },
            ContestedHarvestRegion {
                center: tied_first,
                last_evidence: 5,
                clear_since: None,
            },
        ];
        policy
            .contested_harvest_regions
            .extend((0..crate::stats::HARVEST_INCIDENT_CAP - 1).map(|index| {
                ContestedHarvestRegion {
                    center: TilePos::new(100 + i32::try_from(index).unwrap() * 10, 20),
                    last_evidence: 6,
                    clear_since: None,
                }
            }));
        assert_eq!(
            policy.contested_harvest_regions.len(),
            crate::stats::HARVEST_INCIDENT_CAP + 2
        );

        let mut obs = obs_with(Vec::new());
        obs.map_width = 1_024;
        obs.map_height = 64;
        obs.visible = vec![true; 1_024 * 64];
        obs.explored = vec![true; 1_024 * 64];
        obs.tick = 100;
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
        let inactive = StrategicUtilityContext::new(&[], &units, &buildings, Vec::new());
        assert_eq!(inactive.outstanding_air_production_ticks, None);

        let active = StrategicUtilityContext::new(&[], &units, &buildings, Vec::new())
            .with_outstanding_air_production_ticks(4_801);
        assert_eq!(active.outstanding_air_production_ticks, Some(4_801));
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

        let intents = UtilityPolicy::new().think_with_prelude(
            &Dials::full(),
            &obs,
            &[],
            &[],
            &[],
            Vec::new(),
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
                .think_with_prelude(&Dials::full(), &eliminated, &[], &[], &[], Vec::new(),)
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
            TilePos::new(2, 2),
            ConstructionClaims {
                player_facing: false,
                enlisted: &[],
                reserved: &[],
            },
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
                home,
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
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
    fn difficulty_extends_only_the_residual_production_after_a_mandatory_support_build() {
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
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

        let mut train_prefixes = Vec::new();
        for difficulty in BotDifficulty::ALL {
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 20_042).resolve_profile();
            let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
            let intents =
                UtilityPolicy::new().think_with_prelude(&dials, &obs, &[], &[], &[], Vec::new());
            assert!(
                intents.iter().any(|intent| matches!(
                    intent,
                    Intent::Build {
                        kind: BuildingKind::RepairBay,
                        ..
                    }
                )),
                "{difficulty:?} lost the actionable support commitment: {intents:?}"
            );
            assert!(
                intents
                    .iter()
                    .filter(|intent| matches!(intent, Intent::Build { .. }))
                    .count()
                    == 1,
                "one construction channel may spend per think: {intents:?}"
            );
            assert!(
                intents
                    .iter()
                    .all(|intent| !matches!(intent, Intent::Upgrade { .. }))
            );
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
            train_prefixes.push(trains);
        }

        for pair in train_prefixes.windows(2) {
            assert_eq!(
                pair[0].as_slice(),
                &pair[1][..pair[0].len()],
                "higher attention changed the lower rung's production prefix"
            );
        }
        assert!(
            train_prefixes.first().unwrap().len() < train_prefixes.last().unwrap().len(),
            "the fixture must exercise the expanded discretionary attention budget"
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
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing,
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
            UtilityPolicy::new().think_with_prelude(&dials, world, &[], &[], &[], Vec::new())
        };

        obs.scrap = foundry_cost + UnitKind::Sentinel.stats().cost + 1_000;
        let intents = run(&obs);
        assert!(intents.contains(&Intent::StopUnits {
            units: vec![UnitId(2)],
        }));
        assert!(
            intents.iter().all(|intent| !matches!(
                intent,
                Intent::StopUnits { units } if units.contains(&UnitId(3))
            )),
            "an active Tender is not a construction crew and must keep welding"
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
        assert!(lean.iter().all(|intent| !matches!(
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
        assert_only_expected_dials_change(&low, &high, |candidate, expected| {
            candidate.turret_cap = expected.turret_cap;
            candidate.mine_cap = expected.mine_cap;
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
                home,
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
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
