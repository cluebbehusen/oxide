//! The training interface: a bot whose macro decisions come from
//! outside.
//!
//! A [`GymBot`] is the Phase-A architecture with the policy layer
//! removed and handed to whoever is driving — the PPO trainer over the
//! debug socket's sibling protocol, a scripted test, eventually frozen
//! weights. Chores stay automatic (harvest assignment, orphan-site
//! resume — bookkeeping nobody needs to learn), the executive keeps all
//! its micro (focus fire, withdrawal, pullbacks), and the external
//! policy picks one action from each of three fixed, masked heads per
//! think: production, construction/maintenance, and operations. The
//! executive instantiates all three against one shared budget.
//! Everything runs fog-honest and seat-oriented: a learned policy is
//! honest and seat-symmetric by construction.

use super::executive::{ArmyState, Executive, Intent, LoweringRules};
use super::observation::{Observation, UnitObs};
use super::orient::Orientation;
use super::profile::{PROFILE_COMMITMENT_THRESHOLD, PROFILE_DOCTRINE_THRESHOLD, ProfileFacets};
use super::utility::{Dials, UtilityPolicy};
use crate::command::{Command, PlayerCommand};
use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::state::{Order, State};
use crate::stats::{BuildingKind, Domain, Role, UnitKind};
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::{CARDINALS, DIAGONALS, TilePos};

/// Bump when actions or features change shape — recorded checkpoints
/// and shipped weights must refuse mismatched worlds.
pub const GYM_VERSION: u32 = 8;

/// The global macro menu, partitioned among [`ACTION_HEADS`]. Training
/// slots are role-indexed where the factions differ: one action means
/// "train my anti-air", and the seat's faction resolves which machine
/// that is — one action space, two rosters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Action {
    /// Do nothing beyond chores and executive housekeeping.
    Idle = 0,
    /// Queue a Harvester at the Foundry.
    TrainHarvester = 1,
    /// Queue a Sentinel at the Foundry.
    TrainSentinel = 2,
    /// Queue a Scuttler at the Fabricator.
    TrainScuttler = 3,
    /// Queue a Lancer at the Fabricator.
    TrainLancer = 4,
    /// Queue a Bombard at the Fabricator.
    TrainBombard = 5,
    /// Queue the faction's anti-air crawler at the Fabricator.
    TrainAntiAir = 6,
    /// Queue the faction's ground-attack flyer at the Fabricator.
    TrainAirGround = 7,
    /// Queue the faction's air-superiority flyer at the Fabricator.
    TrainAirAir = 8,
    /// Start a Fabricator near home.
    BuildFabricator = 9,
    /// Start a Turret over the harvest line.
    BuildTurret = 10,
    /// Start a Flak Turret over the harvest line.
    BuildFlak = 11,
    /// Start a Bastion near home.
    BuildBastion = 12,
    /// Start an Array near home.
    BuildArray = 13,
    /// Start a Reclaimer near home.
    BuildReclaimer = 14,
    /// Weld the most-wounded standing building.
    Repair = 15,
    /// Throw the idle air wing at the enemy's work.
    AirRaid = 16,
    /// Draft idle ground fighters into the staging army (or found it).
    FormArmy = 17,
    /// Commit the staging army at the nearest known enemy site.
    Push = 18,
    /// Pull every army back to its rally.
    Recall = 19,
    /// Send a scout now.
    Scout = 20,
    /// Strip the cheapest-and-least-useful own defense for scrap.
    Salvage = 21,
    /// Send a harvester to weld the highest-value own machine wound.
    RepairUnit = 22,
    /// Start a Repair Bay near home.
    BuildRepairBay = 23,
    /// Keep the construction head idle without abandoning a saved plan.
    NoConstruction = 24,
    /// Do nothing in the military-operations head.
    NoOperation = 25,
}

/// Number of actions in [`Action`].
pub const ACTION_COUNT: usize = 26;

/// Global action indices in the production head, in policy order.
pub const PRODUCTION_ACTIONS: [usize; 9] = [0, 1, 2, 3, 4, 5, 6, 7, 8];
/// Global action indices in the construction/maintenance head, in
/// policy order.
pub const CONSTRUCTION_ACTIONS: [usize; 11] = [24, 9, 10, 11, 12, 13, 14, 15, 21, 22, 23];
/// Global action indices in the military-operations head, in policy
/// order.
pub const OPERATION_ACTIONS: [usize; 6] = [25, 16, 17, 18, 19, 20];
/// The three independent policy heads, each expressed in global action
/// indices.
pub const ACTION_HEADS: [&[usize]; 3] = [
    &PRODUCTION_ACTIONS,
    &CONSTRUCTION_ACTIONS,
    &OPERATION_ACTIONS,
];

/// Number of entries in the feature vector.
pub const FEATURE_COUNT: usize = 81;

/// How far (Manhattan tiles) a free harvester may stand from a wounded
/// machine for [`Action::RepairUnit`] to consider it a patient. The
/// wounded rotate to the rear and the welders live on the economy
/// line, so a home-front weld is always in range — while a patient
/// only reachable by a cross-map march never masks the verb on: a
/// wounded machine can walk, and chasing one is not a weld.
pub const REPAIR_UNIT_RADIUS: i32 = 12;

/// Maximum time a saved construction choice or unpaid deferred claim
/// may reserve the bank without becoming a real paid site. Expiry is a
/// deterministic cancel: a saved choice is blocked until it can be paid
/// outright, and a walking founder receives Stop on the next think.
pub const CONSTRUCTION_PLAN_TIMEOUT_TICKS: u64 = 1_200;

/// Capital tech must wait for a minimal home screen. Committing the
/// opening bank before the bot can survive a straight Sentinel rush
/// turns every small map into a deterministic build-order loss.
const FABRICATOR_MIN_HARVESTERS: usize = 4;
const FABRICATOR_MIN_SCREEN_STRENGTH: i64 = 150;

/// A complementary team Industry role should still express its economy lean
/// even when the underlying style does not cross the full doctrine threshold.
const PROFILE_RECLAIMER_THRESHOLD: u32 = 700;
/// The industrial opening compounds one worker beyond the generalist target.
const PROFILE_HARVESTER_TARGET: usize = 5;
/// Vanguard commitment adds two direct-fire bodies to the shipped one-Sentinel
/// opening, then permanently releases production back to the learned policy.
const PROFILE_COMMITMENT_SCREEN_TARGET: usize = 3;
/// The authored Reclaimer milestone uses the same retirement economics as the
/// utility policy: nearby salvage must be low and one fighting purchase stays
/// banked beyond the site's cost.
const PROFILE_HOME_SALVAGE_RADIUS: i32 = 14;
const PROFILE_SALVAGE_LOW: u32 = 450;
const PROFILE_CAPITAL_RESERVE: u32 = 70;

/// How long a witnessed threat, hit, or own loss keeps nearby salvage
/// suspect. Bot memory is deliberately coarser than vision memory: it
/// remembers danger, not hidden units.
const DANGER_MEMORY_TICKS: u64 = 1_800;
/// A source inside this Chebyshev radius of remembered danger is guarded.
const DANGER_RADIUS: i32 = 7;
/// Nearby danger samples coalesce so a walking hostile does not grow an
/// unbounded breadcrumb trail in bot-local memory.
const DANGER_MERGE_RADIUS: i32 = 2;
/// Bot-local tactical memory stays small even when several contacts walk
/// through vision for the full cooling window.
const MAX_DANGER_MEMORIES: usize = 64;
/// One cheap Foundry screen is the minimum recovery escort.
const RECOVERY_SCREEN_SIZE: u32 = 1;
/// A screen that reaches this close to a guarded source has contested it.
const RECOVERY_SECURE_RADIUS: i32 = 3;
/// A rejected or stalled worker hold is retried on a bounded cadence.
const RECOVERY_HOLD_RETRY_TICKS: u64 = 120;
/// A critically wounded, undefended Foundry under visible pressure cannot
/// plausibly wait out the public fallback-income clock.
const RECOVERY_CONCEDE_HP_NUM: u32 = 1;
const RECOVERY_CONCEDE_HP_DEN: u32 = 4;
const RECOVERY_HOME_DANGER_RADIUS: i32 = 8;
/// Finishing needs a real body, not one expensive machine that happens to
/// score highly in the strength estimate.
const FINISH_MIN_FIGHTERS: usize = 5;
/// The ordinary finish gate is a conservative 3:2 advantage over the
/// strongest justified opposition estimate.
const FINISH_MARGIN_NUM: u64 = 3;
const FINISH_MARGIN_DEN: u64 = 2;
/// Late enough and large enough that continuing to grow is less coherent
/// than making the known enemy answer the army.
const FINISH_LATE_TICK: u64 = 24_000;
const FINISH_LATE_FIGHTERS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DangerMemory {
    tile: TilePos,
    strength: u64,
    seen_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OwnUnitMemory {
    id: UnitId,
    tile: TilePos,
    hp: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecoveryAssignment {
    worker: UnitId,
    source: TilePos,
    issued_at: u64,
    secured_target: bool,
}

fn recovery_ring_blocks(start: TilePos, tile: TilePos, danger: TilePos, radius: i32) -> bool {
    let start_distance = start.chebyshev(danger);
    let next_distance = tile.chebyshev(danger);
    if start_distance <= radius {
        next_distance <= radius && next_distance < start_distance
    } else {
        next_distance <= radius
    }
}

fn recovery_reach_contains(distance: Fx, reach: Fx) -> bool {
    distance <= reach * reach
}

fn recovery_rect_closest_point(anchor: TilePos, size: (i32, i32), from: Vec2Fx) -> Vec2Fx {
    let min = anchor.center() - Vec2Fx::new(HALF, HALF);
    let max = min + Vec2Fx::new(Fx::from_num(size.0), Fx::from_num(size.1));
    Vec2Fx::new(from.x.clamp(min.x, max.x), from.y.clamp(min.y, max.y))
}

fn recovery_building_ground_reach(kind: BuildingKind) -> Option<Fx> {
    kind.stats()
        .weapons
        .iter()
        .filter(|weapon| weapon.targets.covers(Domain::Ground))
        .map(|weapon| weapon.range)
        .max()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryPosture {
    Inactive,
    Saving,
    QueueHarvester,
    QueuePackage,
    Prospect,
    Contest(TilePos),
    Harvest(TilePos),
    Concede,
}

/// One name per feature index, emitted in the gym hello and asserted
/// by the trainer — Rust/Python index skew fails loudly at handshake
/// instead of silently training on shifted columns.
pub const FEATURE_NAMES: [&str; FEATURE_COUNT] = [
    "tick",
    "scrap",
    "my_harvesters",
    "my_sentinels",
    "my_scuttlers",
    "my_lancers",
    "my_turrets_built",
    "fab_built",
    "max_foundry_hp",
    "idle_ground_fighters",
    "armies",
    "staging_army_size",
    "army_state",
    "enemy_harvesters",
    "enemy_sentinels",
    "enemy_scuttlers",
    "enemy_lancers",
    "enemy_buildings",
    "enemy_turrets_built",
    "enemy_foundry_known",
    "my_strength",
    "army_strength",
    "enemy_strength",
    "home_x",
    "home_y",
    "enemy_site_x",
    "enemy_site_y",
    "intel_age",
    "seen_strength",
    "seen_age",
    "seen_x",
    "seen_y",
    "my_bombards",
    "my_antiair",
    "my_airground",
    "my_airair",
    "enemy_bombards",
    "enemy_antiair",
    "enemy_airground",
    "enemy_airair",
    "my_flak_built",
    "my_arrays_built",
    "my_reclaimers_built",
    "my_aa_strength",
    "enemy_aa_strength",
    "blip_count",
    "nearest_blip_x",
    "nearest_blip_y",
    "wreck_count",
    "wreck_value",
    "nearest_wreck_x",
    "nearest_wreck_y",
    "damaged_buildings",
    "repair_deficit",
    "ally_units",
    "ally_strength",
    "ally_foundry_hp",
    "ally_distress",
    "faction",
    "map_w",
    "map_h",
    "incoming_shells",
    "my_shells_in_flight",
    "my_building_value",
    "damaged_unit_value",
    "known_salvage_value",
    "near_home_salvage_value",
    "nearest_salvage_distance",
    "idle_harvesters",
    "carried_scrap",
    "queued_unit_value",
    "construction_site_value",
    "my_unit_health_value",
    "my_building_health_value",
    "my_bastions_built",
    "my_repair_bays_built",
    "my_construction_sites",
    "home_enemy_pressure",
    "nearest_enemy_distance",
    "construction_plan",
    "construction_reserve",
];

/// Salvage's fixed liquidation order: cheapest and least useful
/// first. The Fabricator (the tech gate) and the Foundry (the victory
/// token) are never eligible — humans may sell a Fabricator, but the
/// bot's lowering never picks one: v1's value-ordered list made
/// selling the Fabricator the first legal salvage in every game.
pub const SALVAGE_PRIORITY: [BuildingKind; 6] = [
    BuildingKind::Turret,
    BuildingKind::FlakTurret,
    BuildingKind::Array,
    BuildingKind::Bastion,
    BuildingKind::Reclaimer,
    BuildingKind::RepairBay,
];

impl Action {
    /// Decodes a policy's choice; out-of-range folds to Idle (the
    /// trainer masks, but a harness must never panic on bad input).
    pub fn from_index(i: usize) -> Action {
        match i {
            1 => Action::TrainHarvester,
            2 => Action::TrainSentinel,
            3 => Action::TrainScuttler,
            4 => Action::TrainLancer,
            5 => Action::TrainBombard,
            6 => Action::TrainAntiAir,
            7 => Action::TrainAirGround,
            8 => Action::TrainAirAir,
            9 => Action::BuildFabricator,
            10 => Action::BuildTurret,
            11 => Action::BuildFlak,
            12 => Action::BuildBastion,
            13 => Action::BuildArray,
            14 => Action::BuildReclaimer,
            15 => Action::Repair,
            16 => Action::AirRaid,
            17 => Action::FormArmy,
            18 => Action::Push,
            19 => Action::Recall,
            20 => Action::Scout,
            21 => Action::Salvage,
            22 => Action::RepairUnit,
            23 => Action::BuildRepairBay,
            24 => Action::NoConstruction,
            25 => Action::NoOperation,
            _ => Action::Idle,
        }
    }

    fn production(self) -> bool {
        PRODUCTION_ACTIONS.contains(&(self as usize))
    }

    fn construction(self) -> bool {
        CONSTRUCTION_ACTIONS.contains(&(self as usize))
    }

    fn operation(self) -> bool {
        OPERATION_ACTIONS.contains(&(self as usize))
    }

    fn building(self) -> Option<BuildingKind> {
        match self {
            Action::BuildFabricator => Some(BuildingKind::Fabricator),
            Action::BuildTurret => Some(BuildingKind::Turret),
            Action::BuildFlak => Some(BuildingKind::FlakTurret),
            Action::BuildBastion => Some(BuildingKind::Bastion),
            Action::BuildArray => Some(BuildingKind::Array),
            Action::BuildReclaimer => Some(BuildingKind::Reclaimer),
            Action::BuildRepairBay => Some(BuildingKind::RepairBay),
            _ => None,
        }
    }
}

/// One decision from each independent action head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionPlan {
    /// Production choice. [`Action::Idle`] is this head's no-op.
    pub production: Action,
    /// Construction or maintenance choice.
    pub construction: Action,
    /// Military-operations choice.
    pub operation: Action,
}

impl ActionPlan {
    /// Decodes a wire triple, folding an invalid or wrong-head index to
    /// that head's no-op.
    pub fn from_indices(indices: [usize; 3]) -> Self {
        let production = Action::from_index(indices[0]);
        let construction = Action::from_index(indices[1]);
        let operation = Action::from_index(indices[2]);
        Self {
            production: if production.production() {
                production
            } else {
                Action::Idle
            },
            construction: if construction.construction() {
                construction
            } else {
                Action::NoConstruction
            },
            operation: if operation.operation() {
                operation
            } else {
                Action::NoOperation
            },
        }
    }

    /// Maps one legacy flat action into its head while the other two
    /// heads stay idle.
    pub fn from_action(action: Action) -> Self {
        if action.production() {
            Self {
                production: action,
                ..Self::default()
            }
        } else if action.construction() {
            Self {
                construction: action,
                ..Self::default()
            }
        } else if action.operation() {
            Self {
                operation: action,
                ..Self::default()
            }
        } else {
            Self::default()
        }
    }

    /// Returns the three global action indices in head order.
    pub fn indices(self) -> [usize; 3] {
        [
            self.production as usize,
            self.construction as usize,
            self.operation as usize,
        ]
    }
}

impl Default for ActionPlan {
    fn default() -> Self {
        Self {
            production: Action::Idle,
            construction: Action::NoConstruction,
            operation: Action::NoOperation,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ProfileDoctrineProgress {
    workforce: bool,
    commitment_screen: bool,
    fabricator: bool,
    reclaimer: bool,
    air_ground: bool,
    air_air: bool,
    ground_tech: bool,
    anti_air: bool,
    scuttler: bool,
    bombard: bool,
    turret: bool,
}

/// One externally-driven bot. Same command-source shape as the other
/// bots: everything it does goes through recorded `PlayerCommand`s.
#[derive(Debug, Clone)]
pub struct GymBot {
    player: PlayerId,
    dials: Dials,
    /// Construction-time named strategy. Zero is the raw research path and
    /// must leave the decision surface byte-for-byte unchanged.
    profile_facets: ProfileFacets,
    profile_progress: ProfileDoctrineProgress,
    policy: UtilityPolicy,
    exec: Executive,
    /// Fog memory (bot-local, legitimate): the last enemy army this bot
    /// actually saw. Without it a policy forgets an army the moment it
    /// breaks line of sight — no reactive play survives that amnesia.
    seen_strength: u64,
    seen_at: u64,
    seen_pos: Option<TilePos>,
    planned_build: Option<BuildingKind>,
    planned_since: Option<u64>,
    capital_retry_after: u64,
    build_retry_after: Vec<(BuildingKind, u64)>,
    founding_since: Vec<(BuildingKind, TilePos, u64)>,
    /// Threat samples justified by sight, radar, incoming fire, or damage
    /// to our own machines. Positions stay in world space and are oriented
    /// only while a policy decision is made.
    danger: Vec<DangerMemory>,
    /// Previous own-unit observations, used to turn damage and deaths into
    /// legitimate danger memory without reading an attacker's hidden state.
    own_units_seen: Vec<OwnUnitMemory>,
    memory_tick: Option<u64>,
    /// A broken economy remains under recovery reconciliation until a
    /// replacement Harvester has a safe job.
    recovery_active: bool,
    /// An emitted emergency assignment awaiting confirmation in sim state.
    recovery_assignment: Option<RecoveryAssignment>,
    /// The guarded source currently being contested, in oriented space.
    recovery_target: Option<TilePos>,
    /// Last attempt to hold the replacement worker near home.
    recovery_worker_hold: Option<(UnitId, u64)>,
    /// Avoid restarting the same deliberate liquidation every think.
    recovery_liquidation: Option<BuildingId>,
}

/// What the world looks like at a decision point.
#[derive(Debug, Clone)]
pub struct Decision {
    /// Raw integer features, oriented; the trainer normalizes.
    pub features: [i64; FEATURE_COUNT],
    /// Which actions are legal right now.
    pub mask: [bool; ACTION_COUNT],
}

impl GymBot {
    /// A gym bot for `player`. Fog-honest, full doctrine, standard
    /// dials — the learned policy's job is choosing, not cheating.
    pub fn new(player: PlayerId) -> Self {
        Self::with_cadence(player, Dials::full().cadence)
    }

    /// A gym bot deciding every `cadence` ticks (clamped 4..=64). A
    /// longer stride halves the trainer's credit-assignment horizon;
    /// macro decisions don't need 8-tick resolution.
    pub fn with_cadence(player: PlayerId, cadence: u64) -> Self {
        Self::with_profile_facets(player, cadence, ProfileFacets::ZERO)
    }

    /// A gym bot whose named profile may commit finite opening milestones.
    ///
    /// The five values use the same Rust-authored contract as neural
    /// conditioning. [`ProfileFacets::ZERO`] is exactly the raw research path:
    /// it performs no profile reconciliation at all.
    pub fn with_profile_facets(
        player: PlayerId,
        cadence: u64,
        profile_facets: ProfileFacets,
    ) -> Self {
        Self {
            player,
            dials: Dials {
                cadence: cadence.clamp(4, 64),
                ..Dials::full()
            },
            profile_facets,
            profile_progress: ProfileDoctrineProgress::default(),
            policy: UtilityPolicy::new(),
            exec: Executive::default(),
            seen_strength: 0,
            seen_at: 0,
            seen_pos: None,
            planned_build: None,
            planned_since: None,
            capital_retry_after: 0,
            build_retry_after: Vec::new(),
            founding_since: Vec::new(),
            danger: Vec::new(),
            own_units_seen: Vec::new(),
            memory_tick: None,
            recovery_active: false,
            recovery_assignment: None,
            recovery_target: None,
            recovery_worker_hold: None,
            recovery_liquidation: None,
        }
    }

    /// The think cadence (ticks between decisions).
    pub fn cadence(&self) -> u64 {
        self.dials.cadence
    }

    /// The player this bot drives.
    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// Features and action mask at the current tick, oriented.
    /// Also refreshes the fog memory (idempotent at a given tick).
    ///
    /// The executive's real reconciliation lives in `step` (its
    /// transitions emit commands, which a read path must not). To keep
    /// the observation honest anyway, the decision previews that exact
    /// reconciliation on a throwaway clone — same implementation, no
    /// drift, commands discarded — so features and masks always match
    /// what the subsequent lowering will see.
    pub fn decision(&mut self, state: &State) -> Decision {
        let (world, orientation) = self.observe(state);
        self.refresh_tactical_memory(&world);
        let obs = orientation.observe(&world);
        self.refresh_recovery_assignment(state, &obs, &orientation);
        self.remember(&world);
        let rear = rear_tile(&world);
        let mut projected = self.exec.clone();
        let _ = projected.maintain_repair_capable(self.player, &world, rear);
        self.refresh_founding_claims(&obs);
        self.reconcile_plan(&obs);
        self.refresh_profile_progress(&obs);
        let home = home_tile(&obs);
        let armies: Vec<_> = projected
            .armies()
            .iter()
            .map(|a| orientation.army(a.clone()))
            .collect();
        let enlisted: Vec<_> = projected.enlisted().collect();

        let count = |kind: UnitKind, mine: bool| -> i64 {
            let list = if mine {
                &obs.my_units
            } else {
                &obs.enemy_units
            };
            list.iter().filter(|u| u.kind == kind).count() as i64
        };
        let fab_built = obs
            .my_buildings
            .iter()
            .any(|b| b.kind == BuildingKind::Fabricator && b.built);
        // Ground fighters only: the draft (and therefore FormArmy's
        // meaning) is a ground body — wings belong to the raid action.
        let idle_fighters = obs
            .my_units
            .iter()
            .filter(|u| {
                let stats = u.kind.stats();
                stats.can_fight()
                    && stats.domain == Domain::Ground
                    && u.idle
                    && !enlisted.contains(&u.id)
            })
            .count() as i64;
        let staging = armies
            .iter()
            .filter(|a| a.state == ArmyState::Staging)
            .min_by_key(|a| a.id);
        let army_state = armies.first().map_or(0i64, |a| match a.state {
            ArmyState::Staging => 1,
            ArmyState::Pushing => 2,
            ArmyState::Engaging => 3,
            ArmyState::Withdrawing => 4,
        });
        let enemy_site = home.and_then(|h| UtilityPolicy::enemy_site(&obs, h));
        let home_intruder = home.and_then(|h| nearest_home_intruder(&obs, h));
        let my_strength: i64 = obs
            .my_units
            .iter()
            .map(super::executive::unit_strength)
            .sum::<u64>() as i64
            / 100;
        let army_strength: i64 = staging
            .map(|a| {
                obs.my_units
                    .iter()
                    .filter(|u| a.members.contains(&u.id))
                    .map(super::executive::unit_strength)
                    .sum::<u64>()
            })
            .unwrap_or(0) as i64
            / 100;
        let enemy_strength: i64 = (obs
            .enemy_units
            .iter()
            .map(super::executive::unit_strength)
            .sum::<u64>()
            + obs
                .enemy_buildings
                .iter()
                .map(super::executive::building_strength)
                .sum::<u64>()) as i64
            / 100;
        let intel_age = self.policy.intel_age(obs.tick).min(10_000) as i64;
        let seen_pos = self.seen_pos.map(|p| orientation.tile(p));
        let seen_age: i64 = if self.seen_at == 0 && self.seen_strength == 0 {
            10_000
        } else {
            obs.tick.saturating_sub(self.seen_at).min(10_000) as i64
        };

        let role_count = |role: Role, mine: bool| -> i64 {
            let list = if mine {
                &obs.my_units
            } else {
                &obs.enemy_units
            };
            list.iter().filter(|u| u.kind.role() == role).count() as i64
        };
        let built = |kind: BuildingKind| -> i64 {
            obs.my_buildings
                .iter()
                .filter(|b| b.kind == kind && b.built)
                .count() as i64
        };
        let aa_strength = |mine: bool| -> i64 {
            let list = if mine {
                &obs.my_units
            } else {
                &obs.enemy_units
            };
            (list
                .iter()
                .map(|u| super::executive::strength_vs(u, Domain::Air))
                .sum::<u64>()
                / 100) as i64
        };
        let nearest = |tiles: &mut dyn Iterator<Item = TilePos>| -> Option<TilePos> {
            home.and_then(|h| {
                tiles
                    .map(|t| (t.manhattan(h), t.y, t.x))
                    .min()
                    .map(|(_, y, x)| TilePos::new(x, y))
            })
        };
        let nearest_blip = nearest(&mut obs.blips.iter().copied());
        let nearest_wreck = nearest(&mut obs.known_wrecks.iter().map(|(t, _)| *t));
        let damaged: Vec<_> = obs
            .my_buildings
            .iter()
            .filter(|b| b.built && b.hp < b.kind.stats().max_hp)
            .collect();
        let repair_deficit: i64 = damaged
            .iter()
            .map(|b| i64::from(b.kind.stats().max_hp - b.hp))
            .sum();
        let ally_strength: i64 = (obs
            .ally_units
            .iter()
            .map(super::executive::unit_strength)
            .sum::<u64>()
            / 100) as i64;
        let ally_foundries: Vec<TilePos> = obs
            .ally_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Foundry && b.built)
            .map(|b| b.anchor)
            .collect();
        let ally_foundry_hp = obs
            .ally_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Foundry && b.built)
            .map(|b| i64::from(b.hp))
            .min()
            .unwrap_or(-1);
        let ally_distress = i64::from(obs.enemy_units.iter().any(|u| {
            u.kind.stats().can_fight() && ally_foundries.iter().any(|f| u.tile.chebyshev(*f) <= 8)
        }));
        // Positions ride as relative 0-1000 against the actual map, so
        // no coordinate ever extrapolates past the training range on a
        // bigger field. Orientation composed first: flip-then-scale is
        // 1000-minus-scaled, seat symmetry intact.
        let rel_x = |p: TilePos| i64::from(p.x) * 1000 / i64::from(obs.map_width - 1).max(1);
        let rel_y = |p: TilePos| i64::from(p.y) * 1000 / i64::from(obs.map_height - 1).max(1);
        // Hostile shells about to land on the economy: impact tiles the
        // team currently sees (the observation enforces that), within 8
        // of an own building.
        let incoming_shells = obs
            .incoming_shells
            .iter()
            .filter(|t| obs.my_buildings.iter().any(|b| t.chebyshev(b.anchor) <= 8))
            .count() as i64;
        let known_salvage_value = obs
            .known_scrap
            .iter()
            .chain(&obs.known_wrecks)
            .map(|(_, amount)| i64::from(*amount))
            .sum::<i64>();
        let near_home_salvage_value = home.map_or(0, |home| {
            obs.known_scrap
                .iter()
                .chain(&obs.known_wrecks)
                .filter(|(tile, _)| tile.chebyshev(home) <= 14)
                .map(|(_, amount)| i64::from(*amount))
                .sum()
        });
        let nearest_salvage_distance = home
            .and_then(|home| {
                obs.known_scrap
                    .iter()
                    .chain(&obs.known_wrecks)
                    .filter(|(_, amount)| *amount > 0)
                    .map(|(tile, _)| tile.manhattan(home))
                    .min()
            })
            .map_or(-1, i64::from);
        let idle_harvesters = obs
            .my_units
            .iter()
            .filter(|u| {
                u.kind == UnitKind::Harvester
                    && u.idle
                    && u.site.is_none()
                    && u.founding.is_none()
                    && u.salvaging.is_none()
            })
            .count() as i64;
        let carried_scrap = obs
            .my_units
            .iter()
            .map(|u| i64::from(u.carrying))
            .sum::<i64>();
        let queued_unit_value = obs
            .my_queues
            .iter()
            .flatten()
            .map(|kind| i64::from(kind.stats().cost))
            .sum::<i64>();
        let construction_sites = construction_sites(&obs);
        let construction_site_value = obs
            .my_buildings
            .iter()
            .filter(|building| !building.built)
            .map(|building| feature_price(building.kind))
            .sum::<i64>();
        let my_unit_health_value = obs
            .my_units
            .iter()
            .map(|u| {
                let stats = u.kind.stats();
                i64::from(stats.cost) * i64::from(u.hp) / i64::from(stats.max_hp)
            })
            .sum::<i64>();
        let my_building_health_value = obs
            .my_buildings
            .iter()
            .filter(|b| b.built)
            .map(|b| feature_price(b.kind) * i64::from(b.hp) / i64::from(b.kind.stats().max_hp))
            .sum::<i64>();
        let hostile_fighters_near_home = home.map_or(0, |home| {
            obs.enemy_units
                .iter()
                .filter(|u| u.kind.stats().can_fight() && u.tile.chebyshev(home) <= 12)
                .map(super::executive::unit_strength)
                .sum::<u64>()
                / 100
        }) as i64;
        let nearest_enemy_distance = home
            .and_then(|home| {
                obs.enemy_units
                    .iter()
                    .filter(|u| u.kind.stats().can_fight())
                    .map(|u| u.tile.manhattan(home))
                    .min()
            })
            .map_or(-1, i64::from);
        let construction_plan = self.planned_build.map_or(0, building_plan_code);
        let construction_reserve = i64::from(self.construction_reserve(&obs));

        let features: [i64; FEATURE_COUNT] = [
            obs.tick as i64,
            i64::from(obs.scrap),
            count(UnitKind::Harvester, true),
            count(UnitKind::Sentinel, true),
            count(UnitKind::Scuttler, true),
            count(UnitKind::Lancer, true),
            obs.my_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::Turret && b.built)
                .count() as i64,
            i64::from(fab_built),
            obs.my_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::Foundry)
                .map(|b| i64::from(b.hp))
                .max()
                .unwrap_or(0),
            idle_fighters,
            armies.len() as i64,
            staging.map_or(0, |a| a.members.len() as i64),
            army_state,
            count(UnitKind::Harvester, false),
            count(UnitKind::Sentinel, false),
            count(UnitKind::Scuttler, false),
            count(UnitKind::Lancer, false),
            obs.enemy_buildings.len() as i64,
            obs.enemy_buildings
                .iter()
                .filter(|b| b.kind == BuildingKind::Turret && b.built)
                .count() as i64,
            i64::from(
                obs.enemy_buildings
                    .iter()
                    .any(|b| b.kind == BuildingKind::Foundry),
            ),
            my_strength,
            army_strength,
            enemy_strength,
            home.map_or(-1, &rel_x),
            home.map_or(-1, &rel_y),
            enemy_site.map_or(-1, &rel_x),
            enemy_site.map_or(-1, &rel_y),
            intel_age,
            (self.seen_strength / 100) as i64,
            seen_age,
            seen_pos.map_or(-1, &rel_x),
            seen_pos.map_or(-1, &rel_y),
            role_count(Role::Bombard, true),
            role_count(Role::AntiAir, true),
            role_count(Role::AirGround, true),
            role_count(Role::AirAir, true),
            role_count(Role::Bombard, false),
            role_count(Role::AntiAir, false),
            role_count(Role::AirGround, false),
            role_count(Role::AirAir, false),
            built(BuildingKind::FlakTurret),
            built(BuildingKind::Array),
            built(BuildingKind::Reclaimer),
            aa_strength(true),
            aa_strength(false),
            obs.blips.len() as i64,
            nearest_blip.map_or(-1, &rel_x),
            nearest_blip.map_or(-1, &rel_y),
            obs.known_wrecks.len() as i64,
            obs.known_wrecks
                .iter()
                .map(|(_, v)| i64::from(*v))
                .sum::<i64>(),
            nearest_wreck.map_or(-1, rel_x),
            nearest_wreck.map_or(-1, rel_y),
            damaged.len() as i64,
            repair_deficit,
            obs.ally_units.len() as i64,
            ally_strength,
            ally_foundry_hp,
            ally_distress,
            i64::from(obs.faction == crate::state::Faction::Cupric),
            i64::from(obs.map_width),
            i64::from(obs.map_height),
            incoming_shells,
            obs.my_shells as i64,
            // v5: own standing value in buildings — what Salvage can
            // liquidate, and the potential term that keeps selling a
            // Bastion to buy scuttlers from reading as free reward.
            obs.my_buildings
                .iter()
                .filter(|b| b.built)
                .map(|b| feature_price(b.kind))
                .sum::<i64>(),
            // v6: scrap locked in own ground wounds — what RepairUnit
            // recovers and what a Repair Bay amortizes against.
            // my_strength conflates count with wounds (hp-weighted
            // dps), so without this term a policy cannot tell a small
            // army from a battered one, and the trainer has no
            // fog-safe potential to price the recovered value with —
            // my_building_value's role for Salvage, played for welds.
            obs.my_units
                .iter()
                .filter(|u| u.kind.stats().domain == Domain::Ground && u.hp < u.kind.stats().max_hp)
                .map(|u| {
                    let stats = u.kind.stats();
                    i64::from(stats.cost) * i64::from(stats.max_hp - u.hp) / i64::from(stats.max_hp)
                })
                .sum::<i64>(),
            known_salvage_value,
            near_home_salvage_value,
            nearest_salvage_distance,
            idle_harvesters,
            carried_scrap,
            queued_unit_value,
            construction_site_value,
            my_unit_health_value,
            my_building_health_value,
            built(BuildingKind::Bastion),
            built(BuildingKind::RepairBay),
            construction_sites.len() as i64,
            hostile_fighters_near_home,
            nearest_enemy_distance,
            construction_plan,
            construction_reserve,
        ];

        let mut mask = [false; ACTION_COUNT];
        let mut tactical_reconciliation = false;
        mask[Action::Idle as usize] = true;
        mask[Action::NoConstruction as usize] = true;
        mask[Action::NoOperation as usize] = true;
        if let Some(h) = home {
            let foundry_open = obs.my_buildings.iter().enumerate().any(|(qi, b)| {
                b.kind == BuildingKind::Foundry && b.built && obs.my_queues[qi].len() < 2
            });
            let fab_open = obs.my_buildings.iter().enumerate().any(|(qi, b)| {
                b.kind == BuildingKind::Fabricator && b.built && obs.my_queues[qi].len() < 2
            });
            let reserve = self.construction_reserve(&obs);
            let spendable = obs.scrap.saturating_sub(reserve);
            // Production choices are intentions: an open producer is
            // enough to expose the choice before affordability, and
            // lowering waits until the post-construction bank can pay.
            mask[Action::TrainHarvester as usize] = foundry_open;
            mask[Action::TrainSentinel as usize] = foundry_open;
            mask[Action::TrainScuttler as usize] = fab_open;
            mask[Action::TrainLancer as usize] = fab_open;
            mask[Action::TrainBombard as usize] = fab_open;
            mask[Action::TrainAntiAir as usize] = fab_open;
            mask[Action::TrainAirGround as usize] = fab_open;
            mask[Action::TrainAirAir as usize] = fab_open;
            // Turret, FlakTurret, and Bastion feasibility all consult the
            // same known passability and builder routes; one think builds
            // that pair at most once and shares it across the loop.
            let mut defense_probe = None;
            for action in [
                Action::BuildFabricator,
                Action::BuildTurret,
                Action::BuildFlak,
                Action::BuildBastion,
                Action::BuildArray,
                Action::BuildReclaimer,
                Action::BuildRepairBay,
            ] {
                let kind = action.building().expect("build action names a kind");
                let screen_ready = kind != BuildingKind::Fabricator
                    || (obs
                        .my_units
                        .iter()
                        .filter(|unit| unit.kind == UnitKind::Harvester)
                        .count()
                        >= FABRICATOR_MIN_HARVESTERS
                        && my_strength >= FABRICATOR_MIN_SCREEN_STRENGTH);
                mask[action as usize] = screen_ready
                    && self.can_plan_build(&obs, &enlisted, h, kind, &mut defense_probe);
            }
            // Repair and salvage never share a target: a patient an
            // own crew is stripping is not a patient (the sim evicts
            // the loser anyway; masking keeps the oscillator out of
            // the trained distribution).
            let under_salvage: Vec<crate::ids::BuildingId> =
                obs.my_units.iter().filter_map(|u| u.salvaging).collect();
            mask[Action::Repair as usize] = free_builder(&obs, &enlisted)
                && spendable > 0
                && obs.my_buildings.iter().any(|b| {
                    b.built && b.hp < b.kind.stats().max_hp && !under_salvage.contains(&b.id)
                });
            mask[Action::Salvage as usize] = free_builder(&obs, &enlisted)
                && obs.scrap < UnitKind::Harvester.stats().cost
                && !obs.known_scrap.iter().any(|(_, amount)| *amount > 0)
                && !obs.known_wrecks.iter().any(|(_, amount)| *amount > 0)
                && obs
                    .my_buildings
                    .iter()
                    .any(|b| b.built && SALVAGE_PRIORITY.contains(&b.kind));
            // v6: the weld turns on machines. The patient pick carries
            // its own welder check (a free harvester inside the leash),
            // so `builder_free` alone would both under- and over-claim.
            mask[Action::RepairUnit as usize] =
                home_intruder.is_none() && unit_patient(&obs, &enlisted, spendable).is_some();
            mask[Action::AirRaid as usize] = enemy_site.is_some()
                && obs.my_units.iter().any(|u| {
                    let stats = u.kind.stats();
                    stats.domain == Domain::Air
                        && stats.can_target(Domain::Ground)
                        && u.idle
                        && !enlisted.contains(&u.id)
                });
            mask[Action::FormArmy as usize] = idle_fighters > 0;
            mask[Action::Push as usize] =
                staging.is_some() && (home_intruder.is_some() || enemy_site.is_some());
            // The mask shows Recall when armies are out (what the
            // policy trained with — widening it feeds Recall's untrained
            // logits to the blunder picker and yo-yos low-skill armies);
            // the LOWERING is total, so if an army re-stages between
            // decisions the action still emits, idempotently.
            mask[Action::Recall as usize] = armies
                .iter()
                .any(|a| matches!(a.state, ArmyState::Pushing | ArmyState::Engaging));
            mask[Action::Scout as usize] = obs.my_units.iter().any(|u| {
                !enlisted.contains(&u.id)
                    && u.site.is_none()
                    && (u.kind == UnitKind::Harvester || (u.idle && u.kind.stats().can_fight()))
            });
            // Home defense is executive reconciliation, not a strategic
            // preference. Once an armed intruder reaches the economy,
            // "do nothing", scout, raid, and march past it toward a
            // distant base are not coherent alternatives. A staging
            // army engages it; otherwise idle fighters form a body.
            let defense = home_intruder.and_then(|_| {
                if staging.is_some() {
                    Some(Action::Push)
                } else if idle_fighters > 0 {
                    Some(Action::FormArmy)
                } else {
                    None
                }
            });
            if let Some(defense) = defense {
                tactical_reconciliation = true;
                for action in OPERATION_ACTIONS {
                    mask[action] = action == defense as usize;
                }
            } else if let Some(finish) = self.finish_operation(&obs, &armies, h) {
                tactical_reconciliation = true;
                for action in OPERATION_ACTIONS {
                    mask[action] = action == finish as usize;
                }
            }
        }
        let recovery = self.recovery_posture(&obs, &orientation);
        if recovery != RecoveryPosture::Inactive {
            mask.fill(false);
            let action = match recovery {
                RecoveryPosture::QueueHarvester => Action::TrainHarvester,
                RecoveryPosture::Inactive
                | RecoveryPosture::Saving
                | RecoveryPosture::QueuePackage
                | RecoveryPosture::Prospect
                | RecoveryPosture::Contest(_)
                | RecoveryPosture::Harvest(_)
                | RecoveryPosture::Concede => Action::Idle,
            };
            mask[action as usize] = true;
            mask[Action::NoConstruction as usize] = true;
            mask[Action::NoOperation as usize] = true;
        } else if !tactical_reconciliation && home_intruder.is_none() {
            self.apply_profile_doctrine(&obs, &mut mask);
        }
        Decision { features, mask }
    }

    /// Narrows ordinary legal choices around finite, observable profile
    /// milestones. This is part of the decision surface rather than a hidden
    /// rewrite after inference, so native play and the external gym execute
    /// the same action they selected and train against the same mask.
    fn apply_profile_doctrine(&self, obs: &Observation, mask: &mut [bool; ACTION_COUNT]) {
        if self.profile_facets == ProfileFacets::ZERO || self.planned_build.is_some() {
            return;
        }

        if let Some(action) = self.profile_production_milestone(obs, mask) {
            narrow_head(mask, &PRODUCTION_ACTIONS, action);
        }
        if let Some(action) = self.profile_construction_milestone(obs, mask) {
            narrow_head(mask, &CONSTRUCTION_ACTIONS, action);
        }
    }

    fn profile_production_milestone(
        &self,
        obs: &Observation,
        mask: &[bool; ACTION_COUNT],
    ) -> Option<Action> {
        let facets = self.profile_facets;
        if facets.commitment_bias >= PROFILE_COMMITMENT_THRESHOLD
            && !self.profile_progress.commitment_screen
            && mask[Action::TrainSentinel as usize]
        {
            return Some(Action::TrainSentinel);
        }
        if facets.economy_bias >= PROFILE_RECLAIMER_THRESHOLD
            && !self.profile_progress.workforce
            && mask[Action::TrainHarvester as usize]
        {
            return Some(Action::TrainHarvester);
        }

        let industry =
            facets.economy_bias >= PROFILE_DOCTRINE_THRESHOLD && self.profile_progress.workforce;
        let advanced = facets.air_bias >= PROFILE_DOCTRINE_THRESHOLD
            || facets.siege_bias >= PROFILE_DOCTRINE_THRESHOLD
            || industry;
        if advanced && !self.profile_progress.fabricator {
            if committed_units(obs, UnitKind::Harvester) < FABRICATOR_MIN_HARVESTERS
                && mask[Action::TrainHarvester as usize]
            {
                return Some(Action::TrainHarvester);
            }
            let screen_strength = projected_ground_strength(obs);
            if screen_strength < FABRICATOR_MIN_SCREEN_STRENGTH
                && mask[Action::TrainSentinel as usize]
            {
                return Some(Action::TrainSentinel);
            }
        }

        if facets.air_bias >= PROFILE_DOCTRINE_THRESHOLD {
            for (complete, action) in [
                (self.profile_progress.air_ground, Action::TrainAirGround),
                (self.profile_progress.air_air, Action::TrainAirAir),
                (self.profile_progress.ground_tech, Action::TrainLancer),
            ] {
                if !complete && mask[action as usize] {
                    return Some(action);
                }
            }
        }

        if facets.siege_bias >= PROFILE_DOCTRINE_THRESHOLD
            && !self.profile_progress.bombard
            && mask[Action::TrainBombard as usize]
        {
            return Some(Action::TrainBombard);
        }

        if industry {
            for (complete, action) in [
                (self.profile_progress.scuttler, Action::TrainScuttler),
                (self.profile_progress.anti_air, Action::TrainAntiAir),
                (self.profile_progress.ground_tech, Action::TrainLancer),
            ] {
                if !complete && mask[action as usize] {
                    return Some(action);
                }
            }
        }
        None
    }

    fn profile_construction_milestone(
        &self,
        obs: &Observation,
        mask: &[bool; ACTION_COUNT],
    ) -> Option<Action> {
        let facets = self.profile_facets;
        let industry =
            facets.economy_bias >= PROFILE_DOCTRINE_THRESHOLD && self.profile_progress.workforce;
        let advanced = facets.air_bias >= PROFILE_DOCTRINE_THRESHOLD
            || facets.siege_bias >= PROFILE_DOCTRINE_THRESHOLD
            || industry;
        if facets.economy_bias >= PROFILE_RECLAIMER_THRESHOLD
            && self.profile_progress.workforce
            && !self.profile_progress.reclaimer
            && nearby_salvage(obs) < PROFILE_SALVAGE_LOW
            && affordable_capital(obs, BuildingKind::Reclaimer, PROFILE_CAPITAL_RESERVE)
            && mask[Action::BuildReclaimer as usize]
        {
            return Some(Action::BuildReclaimer);
        }

        if advanced && !self.profile_progress.fabricator && mask[Action::BuildFabricator as usize] {
            return Some(Action::BuildFabricator);
        }

        if facets.support_bias >= PROFILE_DOCTRINE_THRESHOLD
            && !self.profile_progress.turret
            && mask[Action::BuildTurret as usize]
        {
            return Some(Action::BuildTurret);
        }
        None
    }

    fn refresh_profile_progress(&mut self, obs: &Observation) {
        if self.profile_facets == ProfileFacets::ZERO {
            return;
        }
        self.profile_progress.workforce |=
            committed_units(obs, UnitKind::Harvester) >= PROFILE_HARVESTER_TARGET;
        self.profile_progress.commitment_screen |=
            committed_direct_ground_fighters(obs) >= PROFILE_COMMITMENT_SCREEN_TARGET;
        self.profile_progress.fabricator |= committed_buildings(obs, BuildingKind::Fabricator) > 0;
        self.profile_progress.reclaimer |= committed_buildings(obs, BuildingKind::Reclaimer) > 0;
        self.profile_progress.air_ground |=
            committed_units(obs, Role::AirGround.unit_for(obs.faction)) > 0;
        self.profile_progress.air_air |=
            committed_units(obs, Role::AirAir.unit_for(obs.faction)) > 0;
        self.profile_progress.ground_tech |= committed_units(obs, UnitKind::Lancer) > 0;
        self.profile_progress.anti_air |=
            committed_units(obs, Role::AntiAir.unit_for(obs.faction)) > 0;
        self.profile_progress.scuttler |= committed_units(obs, UnitKind::Scuttler) > 0;
        self.profile_progress.bombard |= committed_units(obs, UnitKind::Bombard) > 0;
        self.profile_progress.turret |= committed_buildings(obs, BuildingKind::Turret) > 0;
    }

    /// Applies one legacy flat action while the other two heads stay
    /// idle.
    pub fn step(&mut self, state: &State, action: Action) -> Vec<PlayerCommand> {
        self.step_plan(state, ActionPlan::from_action(action))
    }

    /// Applies one choice from every action head, plus chores and
    /// executive housekeeping, and returns this tick's commands.
    pub fn step_plan(&mut self, state: &State, plan: ActionPlan) -> Vec<PlayerCommand> {
        let (world, orientation) = self.observe(state);
        self.refresh_tactical_memory(&world);
        let obs = orientation.observe(&world);
        self.refresh_recovery_assignment(state, &obs, &orientation);
        self.remember(&world);
        let rear = rear_tile(&world);
        let mut commands = self.exec.maintain_repair_capable(self.player, &world, rear);

        self.refresh_founding_claims(&obs);
        self.reconcile_plan(&obs);
        commands.extend(self.cancel_stale_founding(&obs));
        let Some(home) = home_tile(&obs) else {
            return commands; // eliminated
        };
        let armies: Vec<_> = self
            .exec
            .armies()
            .iter()
            .map(|a| orientation.army(a.clone()))
            .collect();
        let recovery = self.recovery_posture(&obs, &orientation);
        if recovery != RecoveryPosture::Inactive {
            commands.extend(self.apply_recovery(
                state,
                &world,
                &obs,
                &orientation,
                &armies,
                home,
                recovery,
            ));
            return commands;
        }
        let enlisted: Vec<_> = self.exec.enlisted().collect();

        self.policy.audit_harvests(&obs);
        self.policy.audit_sites(&obs);
        self.policy.audit_raids(&obs);

        // Head intents lower before chores and in priority order.
        // Construction owns the first claim on labor and scrap,
        // production spends only above its reservation, then operations
        // claim the remaining units.
        let mut intents = Vec::new();
        let staging = armies
            .iter()
            .filter(|a| a.state == ArmyState::Staging)
            .min_by_key(|a| a.id);
        let enemy_site = UtilityPolicy::enemy_site(&obs, home);
        let home_intruder = nearest_home_intruder(&obs, home);
        let mut plan = plan;
        if home_intruder.is_none()
            && let Some(finish) = self.finish_operation(&obs, &armies, home)
        {
            plan.operation = finish;
        }

        if let Some(kind) = plan.construction.building()
            && self.can_plan_build(&obs, &enlisted, home, kind, &mut None)
        {
            self.set_planned_build(kind, obs.tick);
        }
        let maintenance_selected = matches!(
            plan.construction,
            Action::Repair | Action::Salvage | Action::RepairUnit
        );
        // A saved capital project retains its reservation while a selected
        // maintenance verb uses only the surplus. It must not silently
        // replace the sampled verb once the project becomes affordable:
        // that would teach the policy that RepairUnit means "build".
        let build_spend = if maintenance_selected {
            None
        } else {
            self.try_planned_build(&obs, &enlisted, home, &mut intents)
        };
        let reserve = self
            .unpaid_claim_reserve(&obs)
            .saturating_add(build_spend.unwrap_or_else(|| self.saved_plan_reserve()));
        let mut spendable = obs.scrap.saturating_sub(reserve);

        if build_spend.is_none()
            && !(plan.construction == Action::RepairUnit && home_intruder.is_some())
        {
            let maintenance_spend =
                self.lower_maintenance(&obs, &enlisted, plan.construction, spendable, &mut intents);
            spendable = spendable.saturating_sub(maintenance_spend);
        }
        self.lower_production(&obs, plan.production, spendable, &mut intents);
        self.lower_operation(
            &obs,
            &armies,
            staging,
            &enlisted,
            enemy_site,
            home,
            plan.operation,
            &mut intents,
        );

        // Chores after the heads: idle harvesters to work (with the
        // starvation ladder behind the normal channel — neural bots
        // prospect; the scripted yardstick tiers never do), orphaned
        // sites resumed (paid-for progress must not strand).
        self.policy.economy(&obs, home, &mut intents);
        // The action's Build/Repair/Salvage spends a harvester the
        // executive picks only at lowering time, and Scout's lowering
        // is unconditional: a prospector drawn from that same machine
        // would replace the order the action just paid for. Preview the
        // labor claims in world space (the anchors `apply` will see)
        // and keep the ladder off them.
        let spoken_for = self
            .exec
            .labor_claims(&world, &orientation.emit(intents.clone()));
        self.policy.prospect(&obs, &spoken_for, &mut intents);
        if let Some(site) = obs
            .my_buildings
            .iter()
            .filter(|b| !b.built && !obs.my_units.iter().any(|u| u.site == Some(b.id)))
            .min_by_key(|b| (b.anchor.y, b.anchor.x))
        {
            intents.push(Intent::Build {
                kind: site.kind,
                anchor: site.anchor,
            });
        }

        let intents = orientation.emit(intents);
        // Fog placement Part B — the reclaim-parity rule: the gym bot
        // defers a Build exactly when the human's armed click would
        // (some footprint tile not currently visible; the shell's
        // `build_defer_needed` makes the same judgment), so remembered
        // ground founds a walking claim instead of bouncing off the
        // strict predicate's live-occupancy checks. Own vision is own
        // knowledge, and the emitted intents are world-space like the
        // commands. The gym rules also arm the Scout claim guard; the
        // scripted tiers keep both amendments off — they are the
        // ladder's anchors and must not move.
        let vision = state.vision(self.player);
        let defer_needed = |kind: BuildingKind, anchor: TilePos| {
            let (w, h) = kind.stats().size;
            (0..h).any(|dy| (0..w).any(|dx| !vision.visible(anchor.offset(dx, dy))))
        };
        commands.extend(self.exec.apply_with(
            self.player,
            &world,
            &intents,
            &LoweringRules::gym(&defer_needed),
        ));
        commands
    }

    fn lower_production(
        &self,
        obs: &Observation,
        action: Action,
        spendable: u32,
        intents: &mut Vec<Intent>,
    ) {
        let choice = match action {
            Action::TrainHarvester => Some((BuildingKind::Foundry, UnitKind::Harvester)),
            Action::TrainSentinel => Some((BuildingKind::Foundry, UnitKind::Sentinel)),
            Action::TrainScuttler => Some((BuildingKind::Fabricator, UnitKind::Scuttler)),
            Action::TrainLancer => Some((BuildingKind::Fabricator, UnitKind::Lancer)),
            Action::TrainBombard => Some((BuildingKind::Fabricator, UnitKind::Bombard)),
            Action::TrainAntiAir => Some((
                BuildingKind::Fabricator,
                Role::AntiAir.unit_for(obs.faction),
            )),
            Action::TrainAirGround => Some((
                BuildingKind::Fabricator,
                Role::AirGround.unit_for(obs.faction),
            )),
            Action::TrainAirAir => {
                Some((BuildingKind::Fabricator, Role::AirAir.unit_for(obs.faction)))
            }
            _ => None,
        };
        if let Some((building, kind)) = choice
            && spendable >= kind.stats().cost
        {
            self.train(obs, building, kind, intents);
        }
    }

    fn lower_maintenance(
        &mut self,
        obs: &Observation,
        enlisted: &[crate::ids::UnitId],
        action: Action,
        spendable: u32,
        intents: &mut Vec<Intent>,
    ) -> u32 {
        match action {
            Action::Repair if spendable > 0 => {
                let under_salvage: Vec<crate::ids::BuildingId> =
                    obs.my_units.iter().filter_map(|u| u.salvaging).collect();
                let patient = obs
                    .my_buildings
                    .iter()
                    .filter(|b| {
                        b.built && b.hp < b.kind.stats().max_hp && !under_salvage.contains(&b.id)
                    })
                    .map(|b| {
                        let deficit = b.kind.stats().max_hp - b.hp;
                        (std::cmp::Reverse(deficit), b.anchor.y, b.anchor.x, b.id)
                    })
                    .min()
                    .map(|(.., id)| id);
                if let Some(building) = patient {
                    intents.push(Intent::Repair { building });
                    return 1;
                }
            }
            Action::Salvage => {
                let target = obs
                    .my_buildings
                    .iter()
                    .filter(|b| b.built)
                    .filter_map(|b| {
                        SALVAGE_PRIORITY
                            .iter()
                            .position(|kind| *kind == b.kind)
                            .map(|rank| (rank, b.anchor.y, b.anchor.x, b.id))
                    })
                    .min()
                    .map(|(.., id)| id);
                if let Some(building) = target {
                    intents.push(Intent::Salvage { building });
                }
            }
            Action::RepairUnit => {
                if let Some(unit) = unit_patient(obs, enlisted, spendable) {
                    let kind = obs
                        .my_units
                        .iter()
                        .find(|candidate| candidate.id == unit)
                        .map(|candidate| candidate.kind)
                        .expect("patient came from this observation");
                    intents.push(Intent::RepairUnit { unit });
                    return crate::stats::unit_repair_opening_debit(kind);
                }
            }
            _ => {}
        }
        0
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_operation(
        &mut self,
        obs: &Observation,
        armies: &[super::executive::Army],
        staging: Option<&super::executive::Army>,
        enlisted: &[crate::ids::UnitId],
        enemy_site: Option<TilePos>,
        home: TilePos,
        action: Action,
        intents: &mut Vec<Intent>,
    ) {
        match action {
            Action::AirRaid => {
                let target = obs
                    .enemy_units
                    .iter()
                    .filter(|u| u.kind.stats().harvest.is_some())
                    .map(|u| (u.tile.manhattan(home), u.tile.y, u.tile.x))
                    .min()
                    .map(|(_, y, x)| TilePos::new(x, y))
                    .or(enemy_site);
                if let Some(target) = target {
                    intents.push(Intent::RaidAir { target });
                }
            }
            Action::FormArmy => {
                let rally = self.policy.rally_point(obs, staging, enemy_site, home);
                let members = staging.map_or(0, |army| army.members.len() as u32);
                intents.push(Intent::FormArmy {
                    staging: rally,
                    size: self.dials.army_size.max(members + 2),
                });
            }
            Action::Push => {
                let target = nearest_home_intruder(obs, home).or(enemy_site);
                if let (Some(army), Some(target)) = (staging, target) {
                    intents.push(Intent::PushArmy {
                        army: army.id,
                        target,
                    });
                }
            }
            Action::Recall => {
                for army in armies {
                    intents.push(Intent::RecallArmy { army: army.id });
                }
            }
            Action::Scout => self.policy.scouting(obs, home, enlisted, true, intents),
            _ => {}
        }
    }

    fn reconcile_plan(&mut self, obs: &Observation) {
        self.build_retry_after
            .retain(|(_, retry_after)| obs.tick < *retry_after);
        let Some(kind) = self.planned_build else {
            self.planned_since = None;
            return;
        };
        if committed_buildings(obs, kind) >= building_cap(kind) {
            self.clear_planned_build();
            return;
        }
        if self
            .planned_since
            .is_some_and(|since| obs.tick.saturating_sub(since) >= CONSTRUCTION_PLAN_TIMEOUT_TICKS)
        {
            self.clear_planned_build();
            self.block_capital(obs.tick);
            self.block_build(kind, obs.tick);
        }
    }

    fn set_planned_build(&mut self, kind: BuildingKind, tick: u64) {
        if self.planned_since.is_none() {
            self.planned_since = Some(tick);
        }
        self.planned_build = Some(kind);
        self.build_retry_after
            .retain(|(blocked, _)| *blocked != kind);
    }

    fn clear_planned_build(&mut self) {
        self.planned_build = None;
        self.planned_since = None;
    }

    fn block_build(&mut self, kind: BuildingKind, tick: u64) {
        let retry_after = tick.saturating_add(CONSTRUCTION_PLAN_TIMEOUT_TICKS);
        if let Some((_, existing)) = self
            .build_retry_after
            .iter_mut()
            .find(|(blocked, _)| *blocked == kind)
        {
            *existing = (*existing).max(retry_after);
        } else {
            self.build_retry_after.push((kind, retry_after));
            self.build_retry_after
                .sort_unstable_by_key(|(kind, _)| *kind);
        }
    }

    fn block_capital(&mut self, tick: u64) {
        self.capital_retry_after = self
            .capital_retry_after
            .max(tick.saturating_add(CONSTRUCTION_PLAN_TIMEOUT_TICKS));
    }

    fn saved_plan_reserve(&self) -> u32 {
        self.planned_build
            .and_then(|kind| kind.stats().construction)
            .map_or(0, |construction| construction.cost)
    }

    fn refresh_founding_claims(&mut self, obs: &Observation) {
        let current = founding_claims(obs);
        self.founding_since
            .retain(|(kind, anchor, _)| current.contains(&(*kind, *anchor)));
        for (kind, anchor) in current {
            if !self
                .founding_since
                .iter()
                .any(|(known_kind, known_anchor, _)| *known_kind == kind && *known_anchor == anchor)
            {
                self.founding_since.push((kind, anchor, obs.tick));
            }
        }
        self.founding_since
            .sort_unstable_by_key(|(kind, anchor, _)| (*kind, *anchor));
        let mut stale_kinds: Vec<_> = self
            .founding_since
            .iter()
            .filter(|(_, _, since)| {
                obs.tick.saturating_sub(*since) >= CONSTRUCTION_PLAN_TIMEOUT_TICKS
            })
            .map(|(kind, _, _)| *kind)
            .collect();
        stale_kinds.sort_unstable();
        stale_kinds.dedup();
        if !stale_kinds.is_empty() {
            self.block_capital(obs.tick);
        }
        for kind in stale_kinds {
            self.block_build(kind, obs.tick);
        }
    }

    fn founding_claim_stale(&self, kind: BuildingKind, anchor: TilePos, tick: u64) -> bool {
        self.founding_since
            .iter()
            .find(|(known_kind, known_anchor, _)| *known_kind == kind && *known_anchor == anchor)
            .is_some_and(|(_, _, since)| {
                tick.saturating_sub(*since) >= CONSTRUCTION_PLAN_TIMEOUT_TICKS
            })
    }

    fn unpaid_claim_reserve(&self, obs: &Observation) -> u32 {
        founding_claims(obs)
            .into_iter()
            .filter(|(kind, anchor)| !self.founding_claim_stale(*kind, *anchor, obs.tick))
            .filter_map(|(kind, _)| kind.stats().construction)
            .fold(0u32, |total, construction| {
                total.saturating_add(construction.cost)
            })
    }

    fn construction_reserve(&self, obs: &Observation) -> u32 {
        self.saved_plan_reserve()
            .saturating_add(self.unpaid_claim_reserve(obs))
    }

    fn cancel_stale_founding(&mut self, obs: &Observation) -> Vec<PlayerCommand> {
        let mut units = Vec::new();
        let mut kinds = Vec::new();
        for unit in &obs.my_units {
            let Some((kind, anchor)) = unit.founding else {
                continue;
            };
            if self.founding_claim_stale(kind, anchor, obs.tick) {
                units.push(unit.id);
                kinds.push(kind);
            }
        }
        kinds.sort_unstable();
        kinds.dedup();
        if !kinds.is_empty() {
            self.block_capital(obs.tick);
        }
        for kind in kinds {
            self.block_build(kind, obs.tick);
        }
        if units.is_empty() {
            Vec::new()
        } else {
            vec![PlayerCommand {
                player: self.player,
                command: Command::Stop { units },
            }]
        }
    }

    fn can_plan_build(
        &self,
        obs: &Observation,
        enlisted: &[crate::ids::UnitId],
        home: TilePos,
        kind: BuildingKind,
        defense_probe: &mut Option<(KnownPassability, DefenseBuilderRoutes)>,
    ) -> bool {
        obs.tick >= self.capital_retry_after
            && committed_buildings(obs, kind) < building_cap(kind)
            && self.unpaid_claim_reserve(obs) == 0
            && !self
                .build_retry_after
                .iter()
                .any(|(blocked, retry_after)| *blocked == kind && obs.tick < *retry_after)
            && free_builder(obs, enlisted)
            && self.has_build_anchor(obs, enlisted, home, kind, defense_probe)
    }

    /// `defense_probe` memoizes the known-passability grid and builder-route
    /// Dijkstra for one think: they depend only on the observation and the
    /// enlisted set, both fixed across a think, never on the queried kind.
    fn has_build_anchor(
        &self,
        obs: &Observation,
        enlisted: &[UnitId],
        home: TilePos,
        kind: BuildingKind,
        defense_probe: &mut Option<(KnownPassability, DefenseBuilderRoutes)>,
    ) -> bool {
        match kind {
            BuildingKind::Turret | BuildingKind::FlakTurret | BuildingKind::Bastion => {
                let (_, builders) = defense_probe.get_or_insert_with(|| {
                    let passability = KnownPassability::from_observation(obs);
                    let builders = DefenseBuilderRoutes::measure(obs, enlisted, &passability);
                    (passability, builders)
                });
                defense_foci(obs, home, kind)
                    .into_iter()
                    .chain(std::iter::once(home))
                    .flat_map(|focus| self.policy.placements_near(obs, kind, focus))
                    .any(|anchor| builders.travel_to(anchor, kind).is_some())
            }
            BuildingKind::Foundry => false,
            _ => self.policy.placement_near(obs, kind, home).is_some(),
        }
    }

    fn build_anchor(
        &self,
        obs: &Observation,
        enlisted: &[UnitId],
        home: TilePos,
        kind: BuildingKind,
    ) -> Option<TilePos> {
        match kind {
            BuildingKind::Turret | BuildingKind::FlakTurret | BuildingKind::Bastion => {
                self.defense_anchor(obs, enlisted, home, kind)
            }
            BuildingKind::Foundry => None,
            _ => self.policy.placement_near(obs, kind, home),
        }
    }

    fn defense_anchor(
        &self,
        obs: &Observation,
        enlisted: &[UnitId],
        home: TilePos,
        kind: BuildingKind,
    ) -> Option<TilePos> {
        let mut foci = defense_foci(obs, home, kind);
        foci.sort_unstable_by_key(|tile| (tile.y, tile.x));
        foci.dedup();

        let mut candidates: Vec<_> = foci
            .into_iter()
            .flat_map(|focus| self.policy.placements_near(obs, kind, focus))
            .collect();
        candidates.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
        candidates.dedup();
        let traffic = DefenseTraffic::measure(obs, home, kind);
        let builders = DefenseBuilderRoutes::measure(obs, enlisted, &traffic.passability);
        candidates.retain(|anchor| builders.travel_to(*anchor, kind).is_some());
        if candidates.is_empty() {
            candidates = self.policy.placements_near(obs, kind, home);
            candidates.retain(|anchor| builders.travel_to(*anchor, kind).is_some());
        }
        if candidates.is_empty() {
            return None;
        }
        let metrics: Vec<_> = candidates
            .iter()
            .copied()
            .map(|anchor| DefenseMetrics::measure(obs, home, kind, anchor, &traffic, &builders))
            .collect();
        let bounds = DefenseBounds::from_metrics(&metrics);
        candidates
            .into_iter()
            .zip(metrics)
            .map(|(anchor, metrics)| {
                (
                    std::cmp::Reverse(metrics.score(kind, bounds)),
                    anchor.y,
                    anchor.x,
                    anchor,
                )
            })
            .min()
            .map(|(.., anchor)| anchor)
    }

    fn try_planned_build(
        &mut self,
        obs: &Observation,
        enlisted: &[crate::ids::UnitId],
        home: TilePos,
        intents: &mut Vec<Intent>,
    ) -> Option<u32> {
        let kind = self.planned_build?;
        let construction = kind.stats().construction?;
        if self.unpaid_claim_reserve(obs) > 0
            || obs.scrap < construction.cost
            || !free_builder(obs, enlisted)
            || committed_buildings(obs, kind) >= building_cap(kind)
        {
            return None;
        }
        let anchor = self.build_anchor(obs, enlisted, home, kind)?;
        self.policy.note_pending_site(anchor);
        intents.push(Intent::Build { kind, anchor });
        self.clear_planned_build();
        Some(construction.cost)
    }

    fn train(
        &self,
        obs: &Observation,
        at: BuildingKind,
        kind: UnitKind,
        intents: &mut Vec<Intent>,
    ) {
        if let Some((_, b)) = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(qi, b)| b.kind == at && b.built && obs.my_queues[*qi].len() < 2)
            .min_by_key(|(_, b)| b.id)
        {
            intents.push(Intent::TrainAt {
                building: b.id,
                kind,
            });
        }
    }

    /// Refreshes only from information a human commander could justify:
    /// visible/shared hostile units, radar contacts, visible incoming
    /// impacts, and changes to our own units. Calling `decision` followed
    /// by `step_plan` at the same tick is idempotent.
    fn refresh_tactical_memory(&mut self, world: &Observation) {
        if self.memory_tick == Some(world.tick) {
            return;
        }
        self.danger
            .retain(|memory| world.tick.saturating_sub(memory.seen_at) <= DANGER_MEMORY_TICKS);

        let mut samples: Vec<(TilePos, u64)> = Vec::new();
        for previous in &self.own_units_seen {
            match world.my_units.iter().find(|unit| unit.id == previous.id) {
                Some(unit) if unit.hp < previous.hp => {
                    samples.push((unit.tile, recovery_uncertainty_floor()));
                }
                None => samples.push((previous.tile, recovery_uncertainty_floor())),
                Some(_) => {}
            }
        }
        samples.extend(
            world
                .enemy_units
                .iter()
                .filter(|unit| unit.kind.stats().can_target(Domain::Ground))
                .map(|unit| {
                    (
                        unit.tile,
                        super::executive::unit_strength(unit).max(recovery_uncertainty_floor()),
                    )
                }),
        );
        samples.extend(world.enemy_buildings.iter().filter_map(|building| {
            let strength = super::executive::building_strength(building);
            (strength > 0).then_some((building.anchor, strength.max(recovery_uncertainty_floor())))
        }));
        samples.extend(
            world
                .blips
                .iter()
                .copied()
                .map(|tile| (tile, recovery_uncertainty_floor())),
        );
        samples.extend(
            world
                .incoming_shells
                .iter()
                .copied()
                .map(|tile| (tile, recovery_uncertainty_floor())),
        );
        samples.sort_unstable_by_key(|(tile, strength)| (tile.y, tile.x, *strength));
        for (tile, strength) in samples {
            self.remember_danger(tile, strength, world.tick);
        }

        self.own_units_seen = world
            .my_units
            .iter()
            .map(|unit| OwnUnitMemory {
                id: unit.id,
                tile: unit.tile,
                hp: unit.hp,
            })
            .collect();
        self.memory_tick = Some(world.tick);
    }

    fn remember_danger(&mut self, tile: TilePos, strength: u64, tick: u64) {
        let merge = self
            .danger
            .iter()
            .enumerate()
            .filter(|(_, memory)| memory.tile.chebyshev(tile) <= DANGER_MERGE_RADIUS)
            .map(|(index, memory)| {
                (
                    memory.tile.chebyshev(tile),
                    memory.tile.y,
                    memory.tile.x,
                    index,
                )
            })
            .min()
            .map(|(.., index)| index);
        if let Some(index) = merge {
            let memory = &mut self.danger[index];
            memory.tile = tile;
            memory.strength = memory.strength.max(strength);
            memory.seen_at = tick;
        } else {
            self.danger.push(DangerMemory {
                tile,
                strength,
                seen_at: tick,
            });
        }
        while self.danger.len() > MAX_DANGER_MEMORIES {
            let remove = self
                .danger
                .iter()
                .enumerate()
                .min_by_key(|(_, memory)| {
                    (
                        memory.seen_at,
                        memory.strength,
                        memory.tile.y,
                        memory.tile.x,
                    )
                })
                .map(|(index, _)| index)
                .expect("an over-cap danger ledger is non-empty");
            self.danger.remove(remove);
        }
        self.danger
            .sort_unstable_by_key(|memory| (memory.tile.y, memory.tile.x, memory.seen_at));
    }

    fn danger_strength_at(&self, tick: u64, source: TilePos, orientation: &Orientation) -> u64 {
        self.danger
            .iter()
            .filter(|memory| orientation.tile(memory.tile).chebyshev(source) <= DANGER_RADIUS)
            .map(|memory| {
                let age = tick.saturating_sub(memory.seen_at);
                let remaining = DANGER_MEMORY_TICKS.saturating_sub(age);
                memory.strength.saturating_mul(remaining) / DANGER_MEMORY_TICKS
            })
            .fold(0u64, u64::saturating_add)
    }

    fn recovery_route_is_safe(
        &self,
        obs: &Observation,
        orientation: &Orientation,
        worker: UnitId,
        source: TilePos,
        secured_target: bool,
        observed_path: Option<&crate::state::PathFollow>,
    ) -> bool {
        let Some(start) = obs
            .my_units
            .iter()
            .find(|unit| unit.id == worker)
            .map(|unit| unit.tile)
        else {
            return false;
        };
        let source_is_scrap = obs
            .known_scrap
            .iter()
            .any(|(tile, amount)| *tile == source && *amount > 0);
        let source_is_wreck = obs
            .known_wrecks
            .iter()
            .any(|(tile, amount)| *tile == source && *amount > 0);
        if !source_is_scrap && !source_is_wreck {
            return false;
        }

        let passability = KnownPassability::from_observation(obs);
        let remembered: Vec<_> = self
            .danger
            .iter()
            .filter(|memory| {
                memory.strength > 0
                    && obs.tick.saturating_sub(memory.seen_at) < DANGER_MEMORY_TICKS
                    && (!secured_target
                        || orientation.tile(memory.tile).chebyshev(source) > DANGER_RADIUS)
            })
            .map(|memory| orientation.tile(memory.tile))
            .collect();
        let dangerous = |tile: TilePos| {
            if remembered
                .iter()
                .any(|danger| recovery_ring_blocks(start, tile, *danger, DANGER_RADIUS))
            {
                return true;
            }
            let tile_point = tile.center();
            if obs.enemy_units.iter().any(|unit| {
                unit.kind
                    .stats()
                    .max_range_vs(Domain::Ground)
                    .is_some_and(|range| {
                        let reach = range + crate::stats::HARVEST_MOBILE_DANGER_MARGIN;
                        recovery_reach_contains(
                            recovery_rect_closest_point(unit.tile, (1, 1), tile_point)
                                .dist_sq(tile_point),
                            reach,
                        )
                    })
            }) {
                return true;
            }
            if obs.enemy_buildings.iter().any(|building| {
                building.built
                    && recovery_building_ground_reach(building.kind).is_some_and(|range| {
                        let reach = range + crate::stats::HARVEST_STATIC_DANGER_MARGIN;
                        let size = building.kind.stats().size;
                        recovery_reach_contains(
                            recovery_rect_closest_point(building.anchor, size, tile_point)
                                .dist_sq(tile_point),
                            reach,
                        )
                    })
            }) {
                return true;
            }
            obs.blips
                .iter()
                .any(|danger| danger.chebyshev(tile) <= crate::stats::HARVEST_RADAR_DANGER_RADIUS)
                || obs.incoming_shells.iter().any(|danger| {
                    danger.chebyshev(tile) <= crate::stats::HARVEST_INCIDENT_DANGER_RADIUS
                })
        };
        let known_scrap = |tile: TilePos| {
            obs.known_scrap
                .iter()
                .any(|(known, amount)| *known == tile && *amount > 0)
        };
        let passable =
            |tile: TilePos| passability.route_open(tile) && !known_scrap(tile) && !dangerous(tile);
        if observed_path.is_some_and(|path| {
            path.waypoints
                .iter()
                .skip(path.next as usize)
                .copied()
                .map(|waypoint| orientation.tile(waypoint))
                .any(dangerous)
        }) {
            return false;
        }

        let mut goals = if source_is_scrap {
            CARDINALS
                .into_iter()
                .chain(DIAGONALS)
                .map(|(dx, dy)| source.offset(dx, dy))
                .collect::<Vec<_>>()
        } else {
            vec![source]
        };
        goals.sort_unstable_by_key(|goal| (goal.manhattan(start), goal.y, goal.x));
        goals.into_iter().any(|goal| {
            chassis::path::astar(
                obs.map_width,
                obs.map_height,
                start,
                goal,
                passable,
                crate::stats::PATH_EXPANSION_CAP,
            )
            .is_some()
        })
    }

    fn recovery_posture(
        &mut self,
        obs: &Observation,
        orientation: &Orientation,
    ) -> RecoveryPosture {
        let has_foundry = obs
            .my_buildings
            .iter()
            .any(|building| building.kind == BuildingKind::Foundry && building.built);
        if !has_foundry {
            self.recovery_active = false;
            self.recovery_assignment = None;
            self.recovery_target = None;
            return RecoveryPosture::Inactive;
        }

        let harvesters: Vec<&UnitObs> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Harvester)
            .collect();
        let queued_harvester = obs
            .my_queues
            .iter()
            .flatten()
            .any(|kind| *kind == UnitKind::Harvester);
        if harvesters.is_empty() {
            self.recovery_active = true;
        }
        if !self.recovery_active {
            return RecoveryPosture::Inactive;
        }
        if self.recovery_assignment.is_some() {
            return RecoveryPosture::Saving;
        }
        let Some(home) = home_tile(obs) else {
            return RecoveryPosture::Inactive;
        };
        let mut sources: Vec<TilePos> = obs
            .known_scrap
            .iter()
            .chain(&obs.known_wrecks)
            .filter(|(_, amount)| *amount > 0)
            .map(|(tile, _)| *tile)
            .collect();
        sources.sort_unstable_by_key(|tile| (tile.manhattan(home), tile.y, tile.x));
        sources.dedup();
        let danger = |source: TilePos| {
            let remembered = self.danger_strength_at(obs.tick, source, orientation);
            let static_guard = obs
                .enemy_buildings
                .iter()
                .filter(|building| building.anchor.chebyshev(source) <= DANGER_RADIUS)
                .map(super::executive::building_strength)
                .fold(0u64, u64::saturating_add);
            remembered.saturating_add(static_guard)
        };
        let recovery_worker = recovery_worker(obs);
        let safe = sources.iter().copied().find(|source| {
            danger(*source) == 0
                && recovery_worker.is_none_or(|worker| {
                    self.recovery_route_is_safe(obs, orientation, worker, *source, false, None)
                })
        });

        if !harvesters.is_empty() {
            if let Some(source) = safe {
                return RecoveryPosture::Harvest(source);
            }
            if sources.is_empty() {
                self.recovery_target = None;
                return RecoveryPosture::Prospect;
            }
            let target = self
                .recovery_target
                .filter(|target| sources.contains(target) && danger(*target) > 0)
                .unwrap_or_else(|| {
                    sources
                        .iter()
                        .copied()
                        .min_by_key(|source| {
                            (danger(*source), source.manhattan(home), source.y, source.x)
                        })
                        .expect("the guarded source list is non-empty")
                });
            self.recovery_target = Some(target);
            return RecoveryPosture::Contest(target);
        }

        if sources.is_empty() || safe.is_some() {
            return if queued_harvester {
                RecoveryPosture::Saving
            } else if obs.scrap >= UnitKind::Harvester.stats().cost {
                RecoveryPosture::QueueHarvester
            } else {
                RecoveryPosture::Saving
            };
        }

        let live_screen = recovery_screen_units(obs).next().is_some();
        let queued_screen = recovery_screen_queued(obs);
        if live_screen {
            if queued_harvester {
                RecoveryPosture::Saving
            } else if obs.scrap >= UnitKind::Harvester.stats().cost {
                RecoveryPosture::QueueHarvester
            } else {
                RecoveryPosture::Saving
            }
        } else if queued_screen {
            RecoveryPosture::QueuePackage
        } else if recovery_is_conclusive(obs, home) {
            RecoveryPosture::Concede
        } else {
            // A prepaid naked worker suppresses the public emergency
            // income. QueuePackage cancels it first, then saves for the
            // screen and replacement as one coherent purchase.
            RecoveryPosture::QueuePackage
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_recovery(
        &mut self,
        state: &State,
        world: &Observation,
        obs: &Observation,
        orientation: &Orientation,
        armies: &[super::executive::Army],
        home: TilePos,
        posture: RecoveryPosture,
    ) -> Vec<PlayerCommand> {
        let mut commands = Vec::new();
        let lower = |this: &mut Self, intents: Vec<Intent>| {
            let vision = state.vision(this.player);
            let defer_needed = |kind: BuildingKind, anchor: TilePos| {
                let (w, h) = kind.stats().size;
                (0..h).any(|dy| (0..w).any(|dx| !vision.visible(anchor.offset(dx, dy))))
            };
            this.exec.apply_with(
                this.player,
                world,
                &orientation.emit(intents),
                &LoweringRules::gym(&defer_needed),
            )
        };

        match posture {
            RecoveryPosture::Inactive | RecoveryPosture::Saving => {}
            RecoveryPosture::Concede => commands.push(PlayerCommand {
                player: self.player,
                command: Command::Surrender,
            }),
            RecoveryPosture::QueueHarvester => {
                if let Some(foundry) = open_foundry(obs, 1) {
                    commands.push(PlayerCommand {
                        player: self.player,
                        command: Command::Train {
                            building: foundry,
                            kind: UnitKind::Harvester,
                        },
                    });
                }
            }
            RecoveryPosture::QueuePackage => {
                self.clear_planned_build();
                let (
                    mut cancellations,
                    projected_scrap,
                    screen_queued,
                    worker_queued,
                    foundry_slots,
                ) = self.cancel_for_recovery(obs);
                commands.append(&mut cancellations);
                let live_screen = recovery_screen_units(obs).next().is_some();
                let need_screen = !live_screen && !screen_queued;
                let need_worker = !worker_queued;
                let buy_screen = need_screen && projected_scrap >= UnitKind::Sentinel.stats().cost;
                let after_screen = projected_scrap
                    .saturating_sub(u32::from(buy_screen) * UnitKind::Sentinel.stats().cost);
                let mut buy_worker = need_worker
                    && (!need_screen || buy_screen)
                    && after_screen >= UnitKind::Harvester.stats().cost;
                let mut slots = usize::from(buy_screen) + usize::from(buy_worker);
                let mut foundry = foundry_slots
                    .iter()
                    .find(|(_, available)| *available >= slots)
                    .map(|(building, _)| *building);
                if foundry.is_none() && buy_screen && buy_worker {
                    buy_worker = false;
                    slots = 1;
                    foundry = foundry_slots
                        .iter()
                        .find(|(_, available)| *available >= slots)
                        .map(|(building, _)| *building);
                }
                if let Some(foundry) = foundry {
                    if buy_screen {
                        commands.push(PlayerCommand {
                            player: self.player,
                            command: Command::Train {
                                building: foundry,
                                kind: UnitKind::Sentinel,
                            },
                        });
                    }
                    if buy_worker {
                        commands.push(PlayerCommand {
                            player: self.player,
                            command: Command::Train {
                                building: foundry,
                                kind: UnitKind::Harvester,
                            },
                        });
                    }
                }
            }
            RecoveryPosture::Prospect => {
                self.policy.audit_harvests(obs);
                let mut intents = Vec::new();
                self.policy.prospect(obs, &[], &mut intents);
                commands.extend(lower(self, intents));
            }
            RecoveryPosture::Harvest(source) => {
                if let Some(worker) = recovery_worker(obs) {
                    let assignment = lower(
                        self,
                        vec![Intent::AssignHarvest {
                            unit: worker,
                            node: source,
                        }],
                    );
                    self.remember_recovery_assignment(&assignment, worker, obs.tick, false);
                    commands.extend(assignment);
                }
            }
            RecoveryPosture::Contest(source) => {
                let Some(worker) = recovery_worker(obs) else {
                    return commands;
                };
                let live_screen: Vec<UnitId> = recovery_screen_units(obs).collect();
                if live_screen.is_empty() {
                    let queued_screen = recovery_screen_queued(obs);
                    self.recovery_liquidation = self.recovery_liquidation.filter(|building| {
                        obs.my_units
                            .iter()
                            .any(|unit| unit.salvaging == Some(*building))
                    });
                    let mut liquidating = self.recovery_liquidation.is_some();
                    if !queued_screen {
                        if obs.scrap >= UnitKind::Sentinel.stats().cost
                            && let Some(foundry) = open_foundry(obs, 1)
                        {
                            commands.push(PlayerCommand {
                                player: self.player,
                                command: Command::Train {
                                    building: foundry,
                                    kind: UnitKind::Sentinel,
                                },
                            });
                        } else if self.recovery_liquidation.is_none()
                            && let Some(building) = useful_recovery_liquidation(obs)
                        {
                            commands.extend(lower(self, vec![Intent::Salvage { building }]));
                            self.recovery_liquidation = Some(building);
                            liquidating = true;
                        }
                    }
                    if !liquidating {
                        commands.extend(self.hold_recovery_worker(obs, worker, home, orientation));
                    }
                    return commands;
                }

                // Army state outlives individual roles. A pure-artillery
                // remnant cannot make this ground-worker push safe merely
                // because its executive still says Pushing or Staging.
                let has_direct_screen =
                    |members: &[UnitId]| members.iter().any(|member| live_screen.contains(member));
                let staging = armies
                    .iter()
                    .filter(|army| {
                        army.state == ArmyState::Staging && has_direct_screen(&army.members)
                    })
                    .min_by_key(|army| army.id);
                let contesting = armies.iter().find(|army| {
                    army.target == Some(source)
                        && matches!(army.state, ArmyState::Pushing | ArmyState::Engaging)
                        && has_direct_screen(&army.members)
                });
                let screen_on_source = live_screen.iter().any(|member| {
                    obs.my_units.iter().any(|unit| {
                        unit.id == *member && unit.tile.chebyshev(source) <= RECOVERY_SECURE_RADIUS
                    })
                });
                let secured = screen_on_source
                    && !obs.enemy_units.iter().any(|enemy| {
                        enemy.kind.stats().can_target(Domain::Ground)
                            && enemy.tile.chebyshev(source) <= DANGER_RADIUS
                    })
                    && !obs.enemy_buildings.iter().any(|building| {
                        super::executive::building_strength(building) > 0
                            && building.anchor.chebyshev(source) <= DANGER_RADIUS
                    })
                    && !obs
                        .blips
                        .iter()
                        .any(|blip| blip.chebyshev(source) <= DANGER_RADIUS);
                let route_secured = secured
                    && self.recovery_route_is_safe(obs, orientation, worker, source, true, None);
                if route_secured {
                    let assignment = lower(
                        self,
                        vec![Intent::AssignHarvest {
                            unit: worker,
                            node: source,
                        }],
                    );
                    self.remember_recovery_assignment(&assignment, worker, obs.tick, true);
                    commands.extend(assignment);
                } else {
                    let intent = if contesting.is_some()
                        || armies
                            .iter()
                            .any(|army| army.state == ArmyState::Withdrawing)
                    {
                        None
                    } else if let Some(army) = staging {
                        Some(Intent::PushArmy {
                            army: army.id,
                            target: source,
                        })
                    } else {
                        let rally = self.policy.rally_point(obs, None, Some(source), home);
                        Some(Intent::FormArmy {
                            staging: rally,
                            size: RECOVERY_SCREEN_SIZE,
                        })
                    };
                    if let Some(intent) = intent {
                        commands.extend(lower(self, vec![intent]));
                    }
                    commands.extend(self.hold_recovery_worker(obs, worker, home, orientation));
                }
            }
        }
        commands
    }

    fn remember_recovery_assignment(
        &mut self,
        commands: &[PlayerCommand],
        worker: UnitId,
        tick: u64,
        secured_target: bool,
    ) {
        if let Some(source) = commands.iter().find_map(|command| match &command.command {
            Command::Harvest { units, node, .. } if units.contains(&worker) => Some(*node),
            _ => None,
        }) {
            self.recovery_assignment = Some(RecoveryAssignment {
                worker,
                source,
                issued_at: tick,
                secured_target,
            });
        }
    }

    fn refresh_recovery_assignment(
        &mut self,
        state: &State,
        obs: &Observation,
        orientation: &Orientation,
    ) {
        let Some(assignment) = self.recovery_assignment else {
            return;
        };
        if state.current_tick() <= assignment.issued_at {
            return;
        }
        let working = state.units().iter().find(|unit| {
            unit.id == assignment.worker
                && matches!(
                    unit.order,
                    Order::Harvest {
                        node,
                        anchor,
                        retiring: false,
                    } if anchor.unwrap_or(node) == assignment.source
                )
                && (unit.path.is_some() || unit.progress > 0 || unit.carrying > 0)
        });
        let source = orientation.tile(assignment.source);
        let viable = working.is_some_and(|unit| {
            self.recovery_route_is_safe(
                obs,
                orientation,
                assignment.worker,
                source,
                assignment.secured_target,
                unit.path.as_ref(),
            )
        });
        self.recovery_assignment = None;
        if viable {
            self.danger
                .retain(|memory| memory.tile.chebyshev(assignment.source) > DANGER_RADIUS);
            self.recovery_active = false;
            self.recovery_target = None;
            self.recovery_worker_hold = None;
            self.recovery_liquidation = None;
        }
    }

    fn cancel_for_recovery(
        &mut self,
        obs: &Observation,
    ) -> (
        Vec<PlayerCommand>,
        u32,
        bool,
        bool,
        Vec<(BuildingId, usize)>,
    ) {
        let mut commands = Vec::new();
        let mut projected = obs.scrap;
        let mut retained = vec![0usize; obs.my_buildings.len()];
        let screen_to_keep = obs
            .my_queues
            .iter()
            .enumerate()
            .flat_map(|(queue_index, queue)| {
                queue
                    .iter()
                    .copied()
                    .enumerate()
                    .filter(|(_, kind)| recovery_screen_kind(*kind))
                    .map(move |(index, _)| (queue_index, index))
            })
            .min_by_key(|(queue_index, index)| (*index, obs.my_buildings[*queue_index].id));
        let worker_to_keep = (recovery_screen_units(obs).next().is_some()
            || screen_to_keep.is_some())
        .then(|| {
            obs.my_queues
                .iter()
                .enumerate()
                .flat_map(|(queue_index, queue)| {
                    queue
                        .iter()
                        .copied()
                        .enumerate()
                        .filter(|(_, kind)| *kind == UnitKind::Harvester)
                        .map(move |(index, _)| (queue_index, index))
                })
                .min_by_key(|(queue_index, index)| (*index, obs.my_buildings[*queue_index].id))
        })
        .flatten();

        for (queue_index, building) in obs.my_buildings.iter().enumerate() {
            for (index, kind) in obs.my_queues[queue_index].iter().copied().enumerate().rev() {
                let keep = screen_to_keep == Some((queue_index, index))
                    || worker_to_keep == Some((queue_index, index));
                if keep {
                    retained[queue_index] += 1;
                } else {
                    commands.push(PlayerCommand {
                        player: self.player,
                        command: Command::CancelTrain {
                            building: building.id,
                            index: index as u8,
                        },
                    });
                    projected = projected.saturating_add(kind.stats().cost);
                }
            }
        }

        for building in obs.my_buildings.iter().filter(|building| !building.built) {
            commands.push(PlayerCommand {
                player: self.player,
                command: Command::Cancel {
                    building: building.id,
                },
            });
            if let Some(construction) = building.kind.stats().construction {
                projected = projected
                    .saturating_add(construction.cost * building.hp / building.kind.stats().max_hp);
            }
        }
        let founders: Vec<UnitId> = obs
            .my_units
            .iter()
            .filter(|unit| unit.founding.is_some())
            .map(|unit| unit.id)
            .collect();
        if !founders.is_empty() {
            commands.push(PlayerCommand {
                player: self.player,
                command: Command::Stop { units: founders },
            });
        }
        let foundry_slots = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, building)| building.kind == BuildingKind::Foundry && building.built)
            .map(|(index, building)| {
                (
                    building.id,
                    crate::stats::QUEUE_CAP.saturating_sub(retained[index]),
                )
            })
            .collect();
        (
            commands,
            projected,
            screen_to_keep.is_some(),
            worker_to_keep.is_some(),
            foundry_slots,
        )
    }

    fn hold_recovery_worker(
        &mut self,
        obs: &Observation,
        worker: UnitId,
        home: TilePos,
        orientation: &Orientation,
    ) -> Vec<PlayerCommand> {
        let parked = obs
            .my_units
            .iter()
            .find(|unit| unit.id == worker)
            .is_some_and(|unit| unit.idle && unit.tile.chebyshev(home) <= 3);
        if parked {
            self.recovery_worker_hold = Some((worker, obs.tick));
            return Vec::new();
        }
        if self.recovery_worker_hold.is_some_and(|(held, issued_at)| {
            held == worker && obs.tick.saturating_sub(issued_at) < RECOVERY_HOLD_RETRY_TICKS
        }) {
            return Vec::new();
        }
        self.recovery_worker_hold = Some((worker, obs.tick));
        vec![PlayerCommand {
            player: self.player,
            command: Command::Move {
                units: vec![worker],
                goal: orientation.tile(home),
                queue: false,
            },
        }]
    }

    fn finish_operation(
        &mut self,
        obs: &Observation,
        armies: &[super::executive::Army],
        home: TilePos,
    ) -> Option<Action> {
        let _target = UtilityPolicy::enemy_site(obs, home)?;
        if armies
            .iter()
            .any(|army| army.state == ArmyState::Withdrawing)
        {
            return None;
        }

        let fighters: Vec<&UnitObs> = obs
            .my_units
            .iter()
            .filter(|unit| {
                let stats = unit.kind.stats();
                stats.domain == Domain::Ground && stats.can_target(Domain::Ground)
            })
            .collect();
        let own_strength: u64 = fighters
            .iter()
            .map(|unit| super::executive::unit_strength(unit))
            .sum();
        let known_enemy = obs
            .enemy_units
            .iter()
            .map(super::executive::unit_strength)
            .sum::<u64>()
            + obs
                .enemy_buildings
                .iter()
                .map(super::executive::building_strength)
                .sum::<u64>();
        let remembered =
            if self.seen_at > 0 && obs.tick.saturating_sub(self.seen_at) <= DANGER_MEMORY_TICKS {
                self.seen_strength
            } else {
                0
            };
        let intel_fresh = self.policy.intel_age(obs.tick) <= DANGER_MEMORY_TICKS;
        let uncertainty = recovery_uncertainty_floor() * if intel_fresh { 3 } else { 5 };
        let opposition = known_enemy.max(remembered).max(uncertainty);
        let ordinary_advantage = fighters.len() >= FINISH_MIN_FIGHTERS
            && own_strength.saturating_mul(FINISH_MARGIN_DEN)
                >= opposition.saturating_mul(FINISH_MARGIN_NUM);
        let late_commitment = obs.tick >= FINISH_LATE_TICK
            && fighters.len() >= FINISH_LATE_FIGHTERS
            && own_strength >= opposition;
        if !ordinary_advantage && !late_commitment {
            return None;
        }

        // An army already marching or fighting owns its own lifecycle only
        // while it still has a finishing body. A surviving straggler must
        // not freeze a stronger staged or idle reserve behind it.
        // Lock the learned operation head to a no-op: Recall must not
        // interrupt a justified finish, while reissuing Push would reset
        // paths and weapon opportunities.
        if armies
            .iter()
            .filter(|army| matches!(army.state, ArmyState::Pushing | ArmyState::Engaging))
            .any(|army| {
                fighters
                    .iter()
                    .filter(|unit| army.members.contains(&unit.id))
                    .count()
                    >= FINISH_MIN_FIGHTERS
            })
        {
            return Some(Action::NoOperation);
        }
        let staging = armies
            .iter()
            .filter(|army| army.state == ArmyState::Staging)
            .min_by_key(|army| army.id);
        let staging_strength = staging.map_or(0, |army| {
            obs.my_units
                .iter()
                .filter(|unit| army.members.contains(&unit.id))
                .map(super::executive::unit_strength)
                .sum::<u64>()
        });
        let staging_advantage = staging.is_some_and(|army| {
            army.members.len() >= FINISH_MIN_FIGHTERS
                && staging_strength.saturating_mul(FINISH_MARGIN_DEN)
                    >= opposition.saturating_mul(FINISH_MARGIN_NUM)
        });
        let staging_late = staging.is_some_and(|army| {
            obs.tick >= FINISH_LATE_TICK
                && army.members.len() >= FINISH_LATE_FIGHTERS
                && staging_strength >= opposition
        });
        if staging_advantage || staging_late {
            Some(Action::Push)
        } else if obs.my_units.iter().any(|unit| {
            let stats = unit.kind.stats();
            stats.domain == Domain::Ground
                && stats.can_target(Domain::Ground)
                && unit.idle
                && !self.exec.enlisted().any(|id| id == unit.id)
        }) {
            Some(Action::FormArmy)
        } else {
            Some(Action::NoOperation)
        }
    }

    /// Updates fog memory from a world-space observation: while any
    /// enemy fighter is visible, the remembered army is what's visible
    /// now (strength and centroid tile); the timestamp freezes when
    /// sight is lost.
    fn remember(&mut self, world: &Observation) {
        let fighters: Vec<_> = world
            .enemy_units
            .iter()
            .filter(|u| u.kind.stats().can_fight())
            .collect();
        if fighters.is_empty() {
            return;
        }
        self.seen_strength = fighters
            .iter()
            .map(|u| super::executive::unit_strength(u))
            .sum();
        self.seen_at = world.tick;
        let n = fighters.len() as i64;
        let (sx, sy) = fighters.iter().fold((0i64, 0i64), |(sx, sy), u| {
            (sx + i64::from(u.tile.x), sy + i64::from(u.tile.y))
        });
        self.seen_pos = Some(TilePos::new((sx / n) as i32, (sy / n) as i32));
    }

    fn observe(&self, state: &State) -> (Observation, Orientation) {
        let obs = Observation::fog_honest(state, self.player);
        let home = obs
            .my_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Foundry)
            .min_by_key(|b| b.id)
            .map(|b| b.anchor)
            .unwrap_or(TilePos::new(0, 0));
        let orientation = Orientation::for_home(&obs, home);
        (obs, orientation)
    }
}

/// The wounded rear line's tile: the Foundry, in world coordinates.
fn rear_tile(world: &Observation) -> TilePos {
    world
        .my_buildings
        .iter()
        .filter(|b| b.kind == BuildingKind::Foundry)
        .min_by_key(|b| b.id)
        .map(|b| b.anchor)
        .unwrap_or(TilePos::new(0, 0))
}

/// The weld verb's patient: the own ground machine with the most
/// purchase value recoverable from its wound (air patients refuse in
/// the sim), a free harvester inside [`REPAIR_UNIT_RADIUS`], and another
/// free harvester left for the economy, with enough bank for its first
/// paid weld step; ties toward the map origin then id.
/// Fog-safe: own-state only. Both the mask and the lowering call this,
/// so what the policy observed as legal is what the step emits.
/// Whether a harvester is genuinely free for new labor: not on a site,
/// not walking a deferred founding, not enlisted. The masks and the
/// build lowerings share this one judgment — counting a walking
/// founder promises a verb the executive then refuses, and the silent
/// no-op poisons the pending-site ledger.
fn free_builder(obs: &Observation, enlisted: &[crate::ids::UnitId]) -> bool {
    obs.my_units.iter().any(|u| {
        u.kind == UnitKind::Harvester
            && u.site.is_none()
            && u.founding.is_none()
            && !enlisted.contains(&u.id)
    })
}

fn nearest_home_intruder(obs: &Observation, home: TilePos) -> Option<TilePos> {
    obs.enemy_units
        .iter()
        .filter(|unit| unit.kind.stats().can_fight() && unit.tile.chebyshev(home) <= 12)
        .map(|unit| (unit.tile.chebyshev(home), unit.tile.y, unit.tile.x, unit.id))
        .min()
        .map(|(_, y, x, _)| TilePos::new(x, y))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Point2 {
    x: i32,
    y: i32,
}

impl Point2 {
    fn tile(tile: TilePos) -> Self {
        Self {
            x: tile.x.saturating_mul(2).saturating_add(1),
            y: tile.y.saturating_mul(2).saturating_add(1),
        }
    }

    fn building(anchor: TilePos, kind: BuildingKind) -> Self {
        let (width, height) = kind.stats().size;
        Self {
            x: anchor.x.saturating_mul(2).saturating_add(width),
            y: anchor.y.saturating_mul(2).saturating_add(height),
        }
    }

    fn chebyshev(self, other: Self) -> i32 {
        (self.x - other.x).abs().max((self.y - other.y).abs())
    }

    fn within_reach(self, other: Self, minimum_reach: i32, reach: i32) -> bool {
        let dx = i64::from(self.x) - i64::from(other.x);
        let dy = i64::from(self.y) - i64::from(other.y);
        let distance_sq = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
        let minimum = i64::from(minimum_reach);
        let maximum = i64::from(reach);
        distance_sq >= minimum.saturating_mul(minimum)
            && distance_sq <= maximum.saturating_mul(maximum)
    }

    fn as_tile(self) -> TilePos {
        TilePos::new(self.x.div_euclid(2), self.y.div_euclid(2))
    }

    fn as_vec(self) -> chassis::fx::Vec2Fx {
        let half = chassis::fx::HALF;
        chassis::fx::Vec2Fx::new(
            chassis::fx::Fx::from_num(self.x) * half,
            chassis::fx::Fx::from_num(self.y) * half,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownPassability {
    width: i32,
    height: i32,
    terrain_open: Vec<bool>,
    air_open: Vec<bool>,
    route_open: Vec<bool>,
}

impl KnownPassability {
    fn from_observation(obs: &Observation) -> Self {
        let width = obs.map_width.max(0);
        let height = obs.map_height.max(0);
        let len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let mut terrain_open = vec![true; len];
        for &tile in &obs.known_rock {
            if let Some(index) = Self::index_for(width, height, tile) {
                terrain_open[index] = false;
            }
        }
        let mut air_open = vec![true; len];
        for &tile in &obs.known_peaks {
            if let Some(index) = Self::index_for(width, height, tile) {
                air_open[index] = false;
            }
        }
        let mut route_open = terrain_open.clone();
        for &(tile, amount) in &obs.known_scrap {
            if amount > 0
                && let Some(index) = Self::index_for(width, height, tile)
            {
                route_open[index] = false;
            }
        }
        for building in obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .chain(obs.enemy_buildings.iter())
        {
            let (building_width, building_height) = building.kind.stats().size;
            for dy in 0..building_height {
                for dx in 0..building_width {
                    let tile = building.anchor.offset(dx, dy);
                    if let Some(index) = Self::index_for(width, height, tile) {
                        route_open[index] = false;
                    }
                }
            }
        }
        Self {
            width,
            height,
            terrain_open,
            air_open,
            route_open,
        }
    }

    fn index_for(width: i32, height: i32, tile: TilePos) -> Option<usize> {
        if tile.x < 0 || tile.y < 0 || tile.x >= width || tile.y >= height {
            return None;
        }
        let width = usize::try_from(width).ok()?;
        let x = usize::try_from(tile.x).ok()?;
        let y = usize::try_from(tile.y).ok()?;
        y.checked_mul(width)?.checked_add(x)
    }

    fn index(&self, tile: TilePos) -> Option<usize> {
        Self::index_for(self.width, self.height, tile)
    }

    fn route_open(&self, tile: TilePos) -> bool {
        self.index(tile)
            .and_then(|index| self.route_open.get(index))
            .copied()
            .unwrap_or(false)
    }

    fn terrain_open(&self, tile: TilePos) -> bool {
        self.index(tile)
            .and_then(|index| self.terrain_open.get(index))
            .copied()
            .unwrap_or(false)
    }

    fn air_open(&self, tile: TilePos) -> bool {
        self.index(tile)
            .and_then(|index| self.air_open.get(index))
            .copied()
            .unwrap_or(false)
    }

    fn target_open(&self, kind: BuildingKind, tile: TilePos) -> bool {
        if kind == BuildingKind::FlakTurret {
            self.air_open(tile)
        } else {
            self.terrain_open(tile)
        }
    }

    fn fire_clear(&self, kind: BuildingKind, from: Point2, to: Point2) -> bool {
        let open = |tile| {
            if kind == BuildingKind::Turret {
                self.terrain_open(tile)
            } else {
                self.air_open(tile)
            }
        };
        open(to.as_tile()) && !chassis::path::line_blocked(from.as_vec(), to.as_vec(), open)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DefenseBuilderRoutes {
    width: i32,
    height: i32,
    travel: Vec<u32>,
}

impl DefenseBuilderRoutes {
    fn measure(obs: &Observation, enlisted: &[UnitId], passability: &KnownPassability) -> Self {
        let mut travel = vec![u32::MAX; passability.route_open.len()];
        let mut open = std::collections::BinaryHeap::new();
        let known_open = |tile: TilePos| obs.explored(tile) && passability.route_open(tile);
        for unit in obs.my_units.iter().filter(|unit| {
            unit.kind == UnitKind::Harvester
                && unit.site.is_none()
                && unit.founding.is_none()
                && !enlisted.contains(&unit.id)
        }) {
            let Some(index) = passability.index(unit.tile) else {
                continue;
            };
            if known_open(unit.tile) && travel[index] != 0 {
                travel[index] = 0;
                open.push(std::cmp::Reverse((0u32, index)));
            }
        }

        while let Some(std::cmp::Reverse((distance, current_index))) = open.pop() {
            if travel[current_index] != distance {
                continue;
            }
            let Ok(width) = usize::try_from(passability.width) else {
                break;
            };
            let current = TilePos::new(
                i32::try_from(current_index % width).unwrap_or(0),
                i32::try_from(current_index / width).unwrap_or(0),
            );
            let mut visit = |next: TilePos, step: u32| {
                let Some(index) = passability.index(next) else {
                    return;
                };
                let candidate = distance.saturating_add(step);
                if known_open(next) && candidate < travel[index] {
                    travel[index] = candidate;
                    open.push(std::cmp::Reverse((candidate, index)));
                }
            };
            for (dx, dy) in CARDINALS {
                visit(current.offset(dx, dy), 10);
            }
            for (dx, dy) in DIAGONALS {
                let next = current.offset(dx, dy);
                if known_open(current.offset(dx, 0)) && known_open(current.offset(0, dy)) {
                    visit(next, 14);
                }
            }
        }

        Self {
            width: passability.width,
            height: passability.height,
            travel,
        }
    }

    fn travel_to(&self, anchor: TilePos, kind: BuildingKind) -> Option<u32> {
        let (width, height) = kind.stats().size;
        (-1..=width)
            .flat_map(|dx| (-1..=height).map(move |dy| (dx, dy)))
            .filter(|(dx, dy)| !(0..width).contains(dx) || !(0..height).contains(dy))
            .filter_map(|(dx, dy)| {
                KnownPassability::index_for(self.width, self.height, anchor.offset(dx, dy))
                    .and_then(|index| self.travel.get(index))
                    .copied()
                    .filter(|distance| *distance != u32::MAX)
            })
            .min()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DefenseApproach {
    value: i64,
    routes: Vec<Vec<TilePos>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DefenseTraffic {
    approaches: Vec<DefenseApproach>,
    passability: KnownPassability,
    kind: BuildingKind,
    ground_routes: bool,
}

// Sharing the primary is expensive enough to prefer a real parallel lane,
// while the explicit stretch ceiling keeps a remote map-edge tour from
// masquerading as a practical bypass.
const DEFENSE_ALTERNATE_OVERLAP_COST: u32 = 40;
const DEFENSE_ALTERNATE_MAX_STRETCH: u32 = 2;
// The caller budgets roughly two tile expansions per map cell. A
// resource-constrained search can retain several nondominated labels at one
// cell, so give that same spatial budget room for a small Pareto frontier.
const DEFENSE_ALTERNATE_LABEL_BUDGET_FACTOR: u32 = 4;

impl DefenseTraffic {
    fn measure(obs: &Observation, home: TilePos, kind: BuildingKind) -> Self {
        let passability = KnownPassability::from_observation(obs);
        let mut targets: Vec<_> = protected_points(obs, kind)
            .into_iter()
            .map(|(point, value)| (point.as_tile(), value))
            .collect();
        targets.push((home, 1_800));
        targets.sort_unstable_by_key(|(tile, value)| (tile.y, tile.x, std::cmp::Reverse(*value)));
        targets.dedup_by_key(|(tile, _)| *tile);
        targets.sort_unstable_by_key(|(tile, value)| {
            (
                std::cmp::Reverse(*value),
                tile.manhattan(home),
                tile.y,
                tile.x,
            )
        });
        targets.truncate(4);

        let mut sources: Vec<_> = defense_threats(obs, kind)
            .into_iter()
            .map(|(point, value)| (point.as_tile(), value.clamp(1_000, 12_000)))
            .collect();
        let far_x = (obs.map_width - 2).max(0);
        let far_y = (obs.map_height - 2).max(0);
        sources.extend([
            (TilePos::new(far_x, obs.map_height / 4), 500),
            (TilePos::new(far_x, obs.map_height / 2), 500),
            (TilePos::new(far_x, obs.map_height * 3 / 4), 500),
            (TilePos::new(obs.map_width / 4, far_y), 500),
            (TilePos::new(obs.map_width / 2, far_y), 500),
            (TilePos::new(obs.map_width * 3 / 4, far_y), 500),
        ]);
        sources.sort_unstable_by_key(|(tile, value)| (tile.y, tile.x, std::cmp::Reverse(*value)));
        sources.dedup_by_key(|(tile, _)| *tile);
        sources.sort_unstable_by_key(|(tile, value)| {
            (
                std::cmp::Reverse(*value),
                std::cmp::Reverse(tile.manhattan(home)),
                tile.y,
                tile.x,
            )
        });
        sources.truncate(10);

        let max_expansions = u32::try_from(
            i64::from(obs.map_width)
                .saturating_mul(i64::from(obs.map_height))
                .saturating_mul(2),
        )
        .unwrap_or(u32::MAX);
        let ground_routes = kind != BuildingKind::FlakTurret;
        let mut approaches = Vec::new();
        for (target, target_value) in targets {
            for (source, source_value) in &sources {
                let routes = if ground_routes {
                    let Some(target) = nearest_known_route_tile(&passability, target) else {
                        continue;
                    };
                    let Some(source) = nearest_known_route_tile(&passability, *source) else {
                        continue;
                    };
                    let Some(mut primary) = chassis::path::astar(
                        obs.map_width,
                        obs.map_height,
                        source,
                        target,
                        |tile| passability.route_open(tile),
                        max_expansions,
                    ) else {
                        continue;
                    };
                    primary.insert(0, source);
                    let mut routes = vec![primary.clone()];
                    if let Some(alternate) = alternate_ground_route(
                        &passability,
                        source,
                        target,
                        &primary,
                        max_expansions,
                    ) && alternate != primary
                    {
                        routes.push(alternate);
                    }
                    routes
                } else {
                    let Some(route) =
                        known_air_route(&passability, *source, target, max_expansions)
                    else {
                        continue;
                    };
                    vec![route]
                };
                approaches.push(DefenseApproach {
                    value: target_value.saturating_add(*source_value),
                    routes,
                });
            }
        }
        Self {
            approaches,
            passability,
            kind,
            ground_routes,
        }
    }

    fn score_at(&self, center: Point2, minimum_reach: i32, reach: i32) -> (i64, i64, i64) {
        let mut traffic = 0i64;
        let mut choke = 0i64;
        let mut bypass = 0i64;
        for approach in &self.approaches {
            let mut coverage_sum = 0i64;
            let mut choke_sum = 0i64;
            let mut minimum_coverage = i64::MAX;
            for route in &approach.routes {
                let mut covered = 0i64;
                let mut constrained = 0i64;
                for tile in route {
                    if !center.within_reach(Point2::tile(*tile), minimum_reach, reach) {
                        continue;
                    }
                    if !self
                        .passability
                        .fire_clear(self.kind, center, Point2::tile(*tile))
                    {
                        continue;
                    }
                    covered += 1;
                    if self.ground_routes {
                        let exits = chassis::grid::CARDINALS
                            .into_iter()
                            .filter(|(dx, dy)| self.passability.route_open(tile.offset(*dx, *dy)))
                            .count();
                        constrained += i64::try_from(4usize.saturating_sub(exits)).unwrap_or(4);
                    }
                }
                let length = i64::try_from(route.len()).unwrap_or(i64::MAX).max(1);
                let fraction = covered.saturating_mul(1_000) / length;
                coverage_sum = coverage_sum.saturating_add(fraction);
                minimum_coverage = minimum_coverage.min(fraction);
                if self.ground_routes {
                    choke_sum =
                        choke_sum.saturating_add(constrained.saturating_mul(1_000) / length);
                }
            }
            let route_count = i64::try_from(approach.routes.len())
                .unwrap_or(i64::MAX)
                .max(1);
            traffic =
                traffic.saturating_add((coverage_sum / route_count).saturating_mul(approach.value));
            if self.ground_routes {
                choke =
                    choke.saturating_add((choke_sum / route_count).saturating_mul(approach.value));
                bypass = bypass.saturating_add(minimum_coverage.saturating_mul(approach.value));
            }
        }
        (traffic, choke, bypass)
    }
}

fn straight_air_route(start: TilePos, end: TilePos) -> Vec<TilePos> {
    let mut route = Vec::new();
    let mut x = i64::from(start.x);
    let mut y = i64::from(start.y);
    let end_x = i64::from(end.x);
    let end_y = i64::from(end.y);
    let dx = (end_x - x).abs();
    let step_x = if x < end_x { 1 } else { -1 };
    let dy = -(end_y - y).abs();
    let step_y = if y < end_y { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        route.push(TilePos::new(
            i32::try_from(x).unwrap_or(start.x),
            i32::try_from(y).unwrap_or(start.y),
        ));
        if x == end_x && y == end_y {
            break;
        }
        let doubled = error.saturating_mul(2);
        if doubled >= dy {
            error = error.saturating_add(dy);
            x += step_x;
        }
        if doubled <= dx {
            error = error.saturating_add(dx);
            y += step_y;
        }
    }
    route
}

fn known_air_route(
    passability: &KnownPassability,
    start: TilePos,
    goal: TilePos,
    max_expansions: u32,
) -> Option<Vec<TilePos>> {
    let start = nearest_known_air_tile(passability, start)?;
    let goal = nearest_known_air_tile(passability, goal)?;
    let straight = straight_air_route(start, goal);
    if passability.fire_clear(
        BuildingKind::FlakTurret,
        Point2::tile(start),
        Point2::tile(goal),
    ) {
        return Some(straight);
    }
    let mut route = chassis::path::astar(
        passability.width,
        passability.height,
        start,
        goal,
        |tile| passability.air_open(tile),
        max_expansions,
    )?;
    route.insert(0, start);
    Some(route)
}

fn alternate_ground_route(
    passability: &KnownPassability,
    start: TilePos,
    goal: TilePos,
    primary: &[TilePos],
    max_expansions: u32,
) -> Option<Vec<TilePos>> {
    #[derive(Clone, Copy)]
    struct Label {
        score: u32,
        overlap: u32,
        travel: u32,
        tile_index: usize,
        previous: Option<usize>,
        active: bool,
    }

    let start_index = passability.index(start)?;
    let goal_index = passability.index(goal)?;
    let maximum_travel =
        defense_route_travel(primary)?.saturating_mul(DEFENSE_ALTERNATE_MAX_STRETCH);
    let mut primary_interior = vec![false; passability.route_open.len()];
    for &tile in primary.iter().skip(1).take(primary.len().saturating_sub(2)) {
        if let Some(index) = passability.index(tile) {
            primary_interior[index] = true;
        }
    }

    let mut labels = vec![Label {
        score: 0,
        overlap: 0,
        travel: 0,
        tile_index: start_index,
        previous: None,
        active: true,
    }];
    let mut labels_at = vec![Vec::new(); passability.route_open.len()];
    labels_at[start_index].push(0);
    let mut open = std::collections::BinaryHeap::new();
    open.push(std::cmp::Reverse((0u32, 0u32, 0u32, start_index, 0usize)));

    let label_expansion_budget =
        max_expansions.saturating_mul(DEFENSE_ALTERNATE_LABEL_BUDGET_FACTOR);
    let mut expansions = 0u32;
    while let Some(std::cmp::Reverse((score, overlap, travel, current_index, label_index))) =
        open.pop()
    {
        let label = labels[label_index];
        if !label.active
            || (label.score, label.overlap, label.travel, label.tile_index)
                != (score, overlap, travel, current_index)
        {
            continue;
        }
        if current_index == goal_index {
            let width = usize::try_from(passability.width).ok()?;
            let mut route = Vec::new();
            let mut cursor = Some(label_index);
            while let Some(index) = cursor {
                let label = labels[index];
                route.push(TilePos::new(
                    i32::try_from(label.tile_index % width).ok()?,
                    i32::try_from(label.tile_index / width).ok()?,
                ));
                cursor = label.previous;
            }
            route.reverse();
            return Some(route);
        }
        expansions = expansions.saturating_add(1);
        if expansions > label_expansion_budget {
            return None;
        }

        let width = usize::try_from(passability.width).ok()?;
        let current = TilePos::new(
            i32::try_from(current_index % width).ok()?,
            i32::try_from(current_index / width).ok()?,
        );
        let mut visit = |next: TilePos, step: u32| {
            let Some(next_index) = passability.index(next) else {
                return;
            };
            let overlap_step = u32::from(primary_interior[next_index]);
            let candidate_travel = travel.saturating_add(step);
            if candidate_travel > maximum_travel {
                return;
            }
            let candidate = (
                score
                    .saturating_add(step)
                    .saturating_add(overlap_step.saturating_mul(DEFENSE_ALTERNATE_OVERLAP_COST)),
                overlap.saturating_add(overlap_step),
                candidate_travel,
            );
            let existing = labels_at[next_index].clone();
            if existing.iter().any(|index| {
                let label = labels[*index];
                label.active
                    && label.travel <= candidate.2
                    && (label.score < candidate.0
                        || (label.score == candidate.0 && label.overlap <= candidate.1))
            }) {
                return;
            }
            for index in existing {
                let label = &mut labels[index];
                if label.active
                    && candidate.2 <= label.travel
                    && (candidate.0 < label.score
                        || (candidate.0 == label.score && candidate.1 <= label.overlap))
                {
                    label.active = false;
                }
            }
            labels_at[next_index].retain(|index| labels[*index].active);
            let next_label = labels.len();
            labels.push(Label {
                score: candidate.0,
                overlap: candidate.1,
                travel: candidate.2,
                tile_index: next_index,
                previous: Some(label_index),
                active: true,
            });
            labels_at[next_index].push(next_label);
            open.push(std::cmp::Reverse((
                candidate.0,
                candidate.1,
                candidate.2,
                next_index,
                next_label,
            )));
        };

        for (dx, dy) in chassis::grid::CARDINALS {
            let next = current.offset(dx, dy);
            if passability.route_open(next) {
                visit(next, 10);
            }
        }
        for (dx, dy) in chassis::grid::DIAGONALS {
            let next = current.offset(dx, dy);
            if passability.route_open(next)
                && passability.route_open(current.offset(dx, 0))
                && passability.route_open(current.offset(0, dy))
            {
                visit(next, 14);
            }
        }
    }
    None
}

fn defense_route_travel(route: &[TilePos]) -> Option<u32> {
    route.windows(2).try_fold(0u32, |travel, pair| {
        let dx = (pair[0].x - pair[1].x).abs();
        let dy = (pair[0].y - pair[1].y).abs();
        let step = match (dx, dy) {
            (1, 1) => 14,
            (1, 0) | (0, 1) => 10,
            _ => return None,
        };
        Some(travel.saturating_add(step))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DefenseMetrics {
    threat: i64,
    protected_value: i64,
    coverage: i64,
    traffic: i64,
    choke: i64,
    bypass: i64,
    vision: i64,
    spacing: i64,
    congestion: i64,
    builder_safety: i64,
    builder_travel: i64,
}

impl DefenseMetrics {
    fn measure(
        obs: &Observation,
        home: TilePos,
        kind: BuildingKind,
        anchor: TilePos,
        traffic: &DefenseTraffic,
        builders: &DefenseBuilderRoutes,
    ) -> Self {
        let center = Point2::building(anchor, kind);
        let home = Point2::tile(home);
        let (minimum_reach, reach) = defense_reach2(kind);
        let protected = protected_points(obs, kind);
        let threats = defense_threats(obs, kind);
        let (traffic_score, choke, bypass) = traffic.score_at(center, minimum_reach, reach);

        let threat = threats
            .iter()
            .map(|(point, value)| {
                value.saturating_mul(i64::from(64 - center.chebyshev(*point).min(64)))
            })
            .sum();
        let protected_value = protected
            .iter()
            .map(|(point, value)| {
                value.saturating_mul(32) / i64::from(8i32.saturating_add(center.chebyshev(*point)))
            })
            .sum();

        let protected_coverage: i64 = protected
            .iter()
            .filter(|(point, _)| {
                center.within_reach(*point, minimum_reach, reach)
                    && traffic.passability.fire_clear(kind, center, *point)
            })
            .map(|(_, value)| value / 50)
            .sum();
        let approach_coverage: i64 = threats
            .iter()
            .filter(|(point, _)| {
                center.within_reach(*point, minimum_reach, reach)
                    && traffic.passability.fire_clear(kind, center, *point)
                    && home.chebyshev(center) < home.chebyshev(*point)
                    && home
                        .chebyshev(center)
                        .saturating_add(center.chebyshev(*point))
                        <= home.chebyshev(*point).saturating_add(4)
            })
            .map(|(_, value)| value / 50)
            .sum();
        let open_coverage =
            known_open_coverage(&traffic.passability, center, minimum_reach, reach, kind);

        let vision = obs
            .my_units
            .iter()
            .chain(obs.ally_units.iter())
            .map(|unit| {
                let sight = unit.kind.stats().vision.saturating_mul(2);
                if center.chebyshev(Point2::tile(unit.tile)) <= sight {
                    i64::from(sight).saturating_mul(50)
                } else {
                    0
                }
            })
            .chain(
                obs.my_buildings
                    .iter()
                    .chain(obs.ally_buildings.iter())
                    .filter(|building| building.built)
                    .map(|building| {
                        let sight = building.kind.stats().vision.saturating_mul(2);
                        if center.chebyshev(Point2::building(building.anchor, building.kind))
                            <= sight
                        {
                            let array_bonus = if building.kind == BuildingKind::Array {
                                3
                            } else {
                                1
                            };
                            i64::from(sight)
                                .saturating_mul(50)
                                .saturating_mul(array_bonus)
                        } else {
                            0
                        }
                    }),
            )
            .sum();

        let spacing = obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .filter(|building| {
                matches!(
                    building.kind,
                    BuildingKind::Turret | BuildingKind::FlakTurret | BuildingKind::Bastion
                )
            })
            .map(|building| center.chebyshev(Point2::building(building.anchor, building.kind)))
            .min()
            .unwrap_or(24)
            .min(24);

        let congestion = obs
            .my_units
            .iter()
            .chain(obs.ally_units.iter())
            .filter(|unit| center.chebyshev(Point2::tile(unit.tile)) <= 6)
            .count()
            .saturating_add(
                obs.my_buildings
                    .iter()
                    .chain(obs.ally_buildings.iter())
                    .filter(|building| {
                        center.chebyshev(Point2::building(building.anchor, building.kind)) <= 6
                    })
                    .count()
                    .saturating_mul(2),
            );
        let builder_safety = threats
            .iter()
            .map(|(point, _)| center.chebyshev(*point))
            .min()
            .unwrap_or(64)
            .min(64);

        let builder_travel = builders.travel_to(anchor, kind).unwrap_or(u32::MAX);

        Self {
            threat,
            protected_value,
            coverage: protected_coverage
                .saturating_add(approach_coverage)
                .saturating_add(open_coverage),
            traffic: traffic_score,
            choke,
            bypass,
            vision,
            spacing: i64::from(spacing),
            congestion: i64::try_from(congestion).unwrap_or(i64::MAX),
            builder_safety: i64::from(builder_safety),
            builder_travel: i64::from(builder_travel),
        }
    }

    fn score(self, kind: BuildingKind, bounds: DefenseBounds) -> i64 {
        let values = [
            normalized(self.threat, bounds.threat, false),
            normalized(self.protected_value, bounds.protected_value, false),
            normalized(self.coverage, bounds.coverage, false),
            normalized(self.traffic, bounds.traffic, false),
            normalized(self.choke, bounds.choke, false),
            normalized(self.bypass, bounds.bypass, false),
            normalized(self.vision, bounds.vision, false),
            normalized(self.spacing, bounds.spacing, false),
            normalized(self.congestion, bounds.congestion, true),
            normalized(self.builder_safety, bounds.builder_safety, false),
            normalized(self.builder_travel, bounds.builder_travel, true),
        ];
        let weights = match kind {
            BuildingKind::Turret => [16, 18, 10, 15, 12, 10, 3, 8, 3, 3, 2],
            BuildingKind::FlakTurret => [40, 17, 8, 14, 0, 0, 5, 7, 3, 4, 2],
            BuildingKind::Bastion => [15, 12, 14, 14, 12, 12, 10, 5, 2, 2, 2],
            _ => [0; 11],
        };
        values
            .into_iter()
            .zip(weights)
            .map(|(value, weight)| value.saturating_mul(weight))
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DefenseBounds {
    threat: (i64, i64),
    protected_value: (i64, i64),
    coverage: (i64, i64),
    traffic: (i64, i64),
    choke: (i64, i64),
    bypass: (i64, i64),
    vision: (i64, i64),
    spacing: (i64, i64),
    congestion: (i64, i64),
    builder_safety: (i64, i64),
    builder_travel: (i64, i64),
}

impl DefenseBounds {
    fn from_metrics(metrics: &[DefenseMetrics]) -> Self {
        let bounds = |values: fn(DefenseMetrics) -> i64| {
            metrics
                .iter()
                .copied()
                .map(values)
                .fold((i64::MAX, i64::MIN), |(low, high), value| {
                    (low.min(value), high.max(value))
                })
        };
        Self {
            threat: bounds(|metric| metric.threat),
            protected_value: bounds(|metric| metric.protected_value),
            coverage: bounds(|metric| metric.coverage),
            traffic: bounds(|metric| metric.traffic),
            choke: bounds(|metric| metric.choke),
            bypass: bounds(|metric| metric.bypass),
            vision: bounds(|metric| metric.vision),
            spacing: bounds(|metric| metric.spacing),
            congestion: bounds(|metric| metric.congestion),
            builder_safety: bounds(|metric| metric.builder_safety),
            builder_travel: bounds(|metric| metric.builder_travel),
        }
    }
}

fn normalized(value: i64, (low, high): (i64, i64), inverse: bool) -> i64 {
    if low == high {
        return 0;
    }
    let offset = if inverse {
        high.saturating_sub(value)
    } else {
        value.saturating_sub(low)
    };
    offset.saturating_mul(1_000) / high.saturating_sub(low)
}

fn defense_reach2(kind: BuildingKind) -> (i32, i32) {
    kind.stats().weapons.first().map_or((0, 0), |weapon| {
        let doubled = chassis::fx::Fx::from_num(2);
        (
            (weapon.minimum_range * doubled).floor().to_num(),
            (weapon.range * doubled).floor().to_num(),
        )
    })
}

fn defense_foci(obs: &Observation, home: TilePos, kind: BuildingKind) -> Vec<TilePos> {
    let mut foci = Vec::new();
    let mut salvage: Vec<_> = obs
        .known_scrap
        .iter()
        .chain(obs.known_wrecks.iter())
        .filter(|(_, amount)| *amount > 0)
        .map(|(tile, _)| (tile.manhattan(home), tile.y, tile.x, *tile))
        .collect();
    salvage.sort_unstable();

    match kind {
        BuildingKind::Turret | BuildingKind::FlakTurret => {
            foci.extend(salvage.into_iter().take(8).map(|(.., tile)| tile));
            if foci.is_empty() {
                foci.push(home);
                foci.extend(
                    obs.my_units
                        .iter()
                        .filter(|unit| unit.kind == UnitKind::Harvester)
                        .take(8)
                        .map(|unit| unit.tile),
                );
            }
        }
        BuildingKind::Bastion => {
            foci.push(home);
            foci.extend(
                obs.my_buildings
                    .iter()
                    .filter(|building| building.built && !building.kind.stats().can_fight())
                    .take(8)
                    .map(|building| building.anchor),
            );
            foci.extend(salvage.into_iter().take(4).map(|(.., tile)| {
                TilePos::new(
                    home.x + (tile.x - home.x) / 3,
                    home.y + (tile.y - home.y) / 3,
                )
            }));
        }
        _ => {}
    }

    let mut threats: Vec<_> = defense_threats(obs, kind)
        .into_iter()
        .map(|(point, value)| {
            let tile = TilePos::new((point.x - 1) / 2, (point.y - 1) / 2);
            (
                std::cmp::Reverse(value),
                tile.manhattan(home),
                tile.y,
                tile.x,
                tile,
            )
        })
        .collect();
    threats.sort_unstable();
    for (.., threat) in threats.into_iter().take(8) {
        foci.push(threat);
        foci.push(TilePos::new(
            home.x + (threat.x - home.x) / 2,
            home.y + (threat.y - home.y) / 2,
        ));
    }
    if foci.is_empty() {
        foci.push(home);
    }
    foci
}

fn defense_threats(obs: &Observation, kind: BuildingKind) -> Vec<(Point2, i64)> {
    let target_domain = match kind {
        BuildingKind::FlakTurret => Domain::Air,
        _ => Domain::Ground,
    };
    let mut threats: Vec<_> = obs
        .enemy_units
        .iter()
        .filter(|unit| unit.kind.stats().domain == target_domain && unit.kind.stats().can_fight())
        .map(|unit| {
            (
                Point2::tile(unit.tile),
                i64::try_from(super::executive::unit_strength(unit))
                    .unwrap_or(i64::MAX)
                    .max(1),
            )
        })
        .collect();

    threats.extend(obs.blips.iter().map(|tile| (Point2::tile(*tile), 1_000)));
    if target_domain == Domain::Ground {
        threats.extend(
            obs.incoming_shells
                .iter()
                .map(|tile| (Point2::tile(*tile), 1_500)),
        );
        threats.extend(
            obs.enemy_buildings
                .iter()
                .filter(|building| {
                    building
                        .kind
                        .stats()
                        .weapons
                        .iter()
                        .any(|weapon| weapon.targets.covers(Domain::Ground))
                })
                .map(|building| {
                    (
                        Point2::building(building.anchor, building.kind),
                        i64::from(building.hp).max(1),
                    )
                }),
        );
    }
    threats
}

/// A building's price for the value-aggregate features (standing stock,
/// health value, site value). The Foundry prices as ZERO even now that
/// it is purchasable: the shipped v8 policy trained when Foundry
/// construction did not exist, and these aggregates must keep that
/// input distribution until the gym v9 feature redesign re-prices the
/// world coherently for the retrain.
fn feature_price(kind: BuildingKind) -> i64 {
    if kind == BuildingKind::Foundry {
        return 0;
    }
    kind.stats()
        .construction
        .map_or(0, |construction| i64::from(construction.cost))
}

fn protected_points(obs: &Observation, kind: BuildingKind) -> Vec<(Point2, i64)> {
    let building_value = |building: &super::observation::BuildingObs| {
        let base: i64 = match building.kind {
            BuildingKind::Foundry => 1_800,
            BuildingKind::Fabricator => 1_200,
            BuildingKind::Reclaimer => 900,
            BuildingKind::RepairBay => 900,
            BuildingKind::Array => 700,
            BuildingKind::Bastion => 450,
            BuildingKind::Turret | BuildingKind::FlakTurret => 350,
        };
        if building.built { base } else { base / 2 }
    };
    let mut protected: Vec<_> = obs
        .my_buildings
        .iter()
        .chain(obs.ally_buildings.iter())
        .map(|building| {
            (
                Point2::building(building.anchor, building.kind),
                building_value(building),
            )
        })
        .collect();
    protected.extend(
        obs.my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Harvester)
            .map(|unit| {
                (
                    Point2::tile(unit.tile),
                    1_000i64.saturating_add(i64::from(unit.carrying).saturating_mul(20)),
                )
            }),
    );

    let salvage_base = match kind {
        BuildingKind::Turret | BuildingKind::FlakTurret => 5_000,
        BuildingKind::Bastion => 700,
        _ => 0,
    };
    protected.extend(
        obs.known_scrap
            .iter()
            .chain(obs.known_wrecks.iter())
            .filter(|(_, amount)| *amount > 0)
            .map(|(tile, amount)| {
                (
                    Point2::tile(*tile),
                    salvage_base + i64::from((*amount).min(800)),
                )
            }),
    );
    protected
}

fn nearest_known_route_tile(passability: &KnownPassability, wanted: TilePos) -> Option<TilePos> {
    for radius in 0i32..=4 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) == radius {
                    let tile = wanted.offset(dx, dy);
                    if passability.route_open(tile) {
                        return Some(tile);
                    }
                }
            }
        }
    }
    None
}

fn nearest_known_air_tile(passability: &KnownPassability, wanted: TilePos) -> Option<TilePos> {
    for radius in 0i32..=4 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) == radius {
                    let tile = wanted.offset(dx, dy);
                    if passability.air_open(tile) {
                        return Some(tile);
                    }
                }
            }
        }
    }
    None
}

fn known_open_coverage(
    passability: &KnownPassability,
    center: Point2,
    minimum_reach: i32,
    reach: i32,
    kind: BuildingKind,
) -> i64 {
    let min_x = ((center.x - reach - 1) / 2).max(0);
    let max_x = ((center.x + reach - 1) / 2).min(passability.width - 1);
    let min_y = ((center.y - reach - 1) / 2).max(0);
    let max_y = ((center.y + reach - 1) / 2).min(passability.height - 1);
    let mut open = 0i64;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let tile = TilePos::new(x, y);
            if center.within_reach(Point2::tile(tile), minimum_reach, reach)
                && passability.target_open(kind, tile)
                && passability.fire_clear(kind, center, Point2::tile(tile))
            {
                open += 1;
            }
        }
    }
    open
}

fn building_cap(kind: BuildingKind) -> usize {
    match kind {
        BuildingKind::Fabricator => 1,
        BuildingKind::Turret | BuildingKind::FlakTurret | BuildingKind::Bastion => 2,
        BuildingKind::Array | BuildingKind::RepairBay => 1,
        BuildingKind::Reclaimer => 2,
        BuildingKind::Foundry => 0,
    }
}

fn building_plan_code(kind: BuildingKind) -> i64 {
    match kind {
        BuildingKind::Fabricator => 1,
        BuildingKind::Turret => 2,
        BuildingKind::FlakTurret => 3,
        BuildingKind::Bastion => 4,
        BuildingKind::Array => 5,
        BuildingKind::Reclaimer => 6,
        BuildingKind::RepairBay => 7,
        BuildingKind::Foundry => 0,
    }
}

fn founding_claims(obs: &Observation) -> Vec<(BuildingKind, TilePos)> {
    let mut claims: Vec<_> = obs
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

fn construction_sites(obs: &Observation) -> Vec<(BuildingKind, TilePos)> {
    let mut sites: Vec<_> = obs
        .my_buildings
        .iter()
        .filter(|building| !building.built)
        .map(|building| (building.kind, building.anchor))
        .collect();
    sites.extend(founding_claims(obs));
    sites.sort_unstable();
    sites.dedup();
    sites
}

fn committed_buildings(obs: &Observation, kind: BuildingKind) -> usize {
    obs.my_buildings
        .iter()
        .filter(|building| building.kind == kind)
        .count()
        + founding_claims(obs)
            .iter()
            .filter(|(claim, _)| *claim == kind)
            .count()
}

fn committed_units(obs: &Observation, kind: UnitKind) -> usize {
    obs.my_units.iter().filter(|unit| unit.kind == kind).count()
        + obs
            .my_queues
            .iter()
            .flatten()
            .filter(|queued| **queued == kind)
            .count()
}

fn committed_direct_ground_fighters(obs: &Observation) -> usize {
    obs.my_units
        .iter()
        .filter(|unit| unit.kind.is_recovery_screen())
        .count()
        + obs
            .my_queues
            .iter()
            .flatten()
            .filter(|kind| kind.is_recovery_screen())
            .count()
}

fn projected_ground_strength(obs: &Observation) -> i64 {
    let live = obs
        .my_units
        .iter()
        .map(super::executive::unit_strength)
        .sum::<u64>();
    let queued = obs
        .my_queues
        .iter()
        .flatten()
        .map(|kind| {
            let stats = kind.stats();
            let dps100 = stats
                .weapons
                .iter()
                .filter(|weapon| weapon.targets.covers(Domain::Ground))
                .map(|weapon| u64::from(weapon.damage) * 100 / u64::from(weapon.cooldown_ticks))
                .sum::<u64>();
            u64::from(stats.max_hp) * dps100
        })
        .sum::<u64>();
    i64::try_from(live.saturating_add(queued) / 100).unwrap_or(i64::MAX)
}

fn nearby_salvage(obs: &Observation) -> u32 {
    let Some(home) = home_tile(obs) else {
        return 0;
    };
    obs.known_scrap
        .iter()
        .chain(&obs.known_wrecks)
        .filter(|(tile, _)| tile.chebyshev(home) <= PROFILE_HOME_SALVAGE_RADIUS)
        .fold(0u32, |total, (_, amount)| total.saturating_add(*amount))
}

fn affordable_capital(obs: &Observation, kind: BuildingKind, reserve: u32) -> bool {
    kind.stats()
        .construction
        .is_some_and(|construction| obs.scrap >= construction.cost.saturating_add(reserve))
}

fn narrow_head(mask: &mut [bool; ACTION_COUNT], head: &[usize], action: Action) {
    debug_assert!(head.contains(&(action as usize)));
    debug_assert!(mask[action as usize]);
    for candidate in head {
        mask[*candidate] = *candidate == action as usize;
    }
}

fn unit_patient(
    obs: &Observation,
    enlisted: &[crate::ids::UnitId],
    spendable: u32,
) -> Option<crate::ids::UnitId> {
    let welder_near = |patient: &UnitObs| {
        let mut available = obs.my_units.iter().filter(|u| {
            u.kind == UnitKind::Harvester
                && u.site.is_none()
                && u.founding.is_none()
                && u.id != patient.id
                && !enlisted.contains(&u.id)
        });
        available.clone().count() >= 2
            && available.any(|u| u.tile.manhattan(patient.tile) <= REPAIR_UNIT_RADIUS)
    };
    obs.my_units
        .iter()
        .filter(|u| {
            let stats = u.kind.stats();
            stats.domain == Domain::Ground
                && u.hp < stats.max_hp
                && spendable >= crate::stats::unit_repair_opening_debit(u.kind)
                && welder_near(u)
        })
        .min_by(|a, b| {
            let a_stats = a.kind.stats();
            let b_stats = b.kind.stats();
            let a_deficit = a_stats.max_hp - a.hp;
            let b_deficit = b_stats.max_hp - b.hp;
            // Compare cost*deficit/max exactly by cross multiplication.
            // Repair pricing's common 850-permille factor cannot change
            // this order.
            let a_value_at_b_scale =
                u64::from(a_stats.cost) * u64::from(a_deficit) * u64::from(b_stats.max_hp);
            let b_value_at_a_scale =
                u64::from(b_stats.cost) * u64::from(b_deficit) * u64::from(a_stats.max_hp);
            b_value_at_a_scale
                .cmp(&a_value_at_b_scale)
                .then_with(|| (a.tile.y, a.tile.x, a.id).cmp(&(b.tile.y, b.tile.x, b.id)))
        })
        .map(|u| u.id)
}

fn home_tile(obs: &Observation) -> Option<TilePos> {
    obs.my_buildings
        .iter()
        .filter(|b| b.kind == BuildingKind::Foundry && b.built)
        .min_by_key(|b| b.id)
        .map(|b| b.anchor)
}

fn recovery_uncertainty_floor() -> u64 {
    let sentinel = UnitKind::Sentinel.stats();
    let weapon = sentinel
        .weapons
        .iter()
        .find(|weapon| weapon.targets.covers(Domain::Ground))
        .expect("the recovery screen can fight ground");
    u64::from(sentinel.max_hp) * (u64::from(weapon.damage) * 100 / u64::from(weapon.cooldown_ticks))
}

fn recovery_screen_kind(kind: UnitKind) -> bool {
    kind.is_recovery_screen()
}

fn recovery_screen_units(obs: &Observation) -> impl Iterator<Item = UnitId> + '_ {
    obs.my_units
        .iter()
        .filter(|unit| recovery_screen_kind(unit.kind))
        .map(|unit| unit.id)
}

fn recovery_screen_queued(obs: &Observation) -> bool {
    obs.my_queues
        .iter()
        .flatten()
        .any(|kind| recovery_screen_kind(*kind))
}

fn recovery_worker(obs: &Observation) -> Option<UnitId> {
    obs.my_units
        .iter()
        .filter(|unit| {
            unit.kind == UnitKind::Harvester
                && unit.site.is_none()
                && unit.founding.is_none()
                && unit.salvaging.is_none()
        })
        .map(|unit| unit.id)
        .min()
}

fn recovery_is_conclusive(obs: &Observation, home: TilePos) -> bool {
    let foundry = obs
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry && building.built)
        .min_by_key(|building| building.id);
    let Some(foundry) = foundry else {
        return false;
    };
    let critically_wounded = foundry.hp.saturating_mul(RECOVERY_CONCEDE_HP_DEN)
        <= foundry
            .kind
            .stats()
            .max_hp
            .saturating_mul(RECOVERY_CONCEDE_HP_NUM);
    let visible_pressure = obs.enemy_units.iter().any(|unit| {
        unit.kind.stats().can_target(Domain::Ground)
            && unit.tile.chebyshev(home) <= RECOVERY_HOME_DANGER_RADIUS
    });
    let ally_can_intervene = obs
        .ally_units
        .iter()
        .any(|unit| unit.kind.stats().can_target(Domain::Ground))
        || obs
            .ally_buildings
            .iter()
            .any(|building| building.kind == BuildingKind::Foundry && building.built);
    let refundable_commitment = obs.my_buildings.iter().any(|building| !building.built)
        || obs.my_queues.iter().flatten().next().is_some();
    let salvageable_asset = obs
        .my_buildings
        .iter()
        .any(|building| building.built && SALVAGE_PRIORITY.contains(&building.kind));
    critically_wounded
        && visible_pressure
        && !ally_can_intervene
        && !refundable_commitment
        && !salvageable_asset
        && obs.scrap < UnitKind::Sentinel.stats().cost
}

fn open_foundry(obs: &Observation, slots: usize) -> Option<BuildingId> {
    obs.my_buildings
        .iter()
        .enumerate()
        .filter(|(queue_index, building)| {
            building.kind == BuildingKind::Foundry
                && building.built
                && obs.my_queues[*queue_index].len().saturating_add(slots)
                    <= crate::stats::QUEUE_CAP
        })
        .map(|(_, building)| building.id)
        .min()
}

fn useful_recovery_liquidation(obs: &Observation) -> Option<BuildingId> {
    let active: Vec<BuildingId> = obs
        .my_units
        .iter()
        .filter_map(|unit| unit.salvaging)
        .collect();
    obs.my_buildings
        .iter()
        .filter(|building| building.built && !active.contains(&building.id))
        .filter_map(|building| {
            let rank = SALVAGE_PRIORITY
                .iter()
                .position(|kind| *kind == building.kind)?;
            let construction = building.kind.stats().construction?;
            let refund = u64::from(construction.cost)
                * u64::from(building.hp)
                * crate::stats::SALVAGE_REFUND_PERMILLE
                / (1000 * u64::from(building.kind.stats().max_hp));
            (u64::from(obs.scrap).saturating_add(refund)
                >= u64::from(UnitKind::Sentinel.stats().cost))
            .then_some((rank, building.anchor.y, building.anchor.x, building.id))
        })
        .min()
        .map(|(.., building)| building)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn danger_memory_is_bounded_and_cools_deterministically() {
        let mut bot = GymBot::new(PlayerId(0));
        for tick in 0..100 {
            bot.remember_danger(
                TilePos::new(i32::try_from(tick * 3).unwrap(), 0),
                recovery_uncertainty_floor(),
                tick,
            );
        }
        assert_eq!(bot.danger.len(), MAX_DANGER_MEMORIES);
        assert_eq!(
            bot.danger.iter().map(|memory| memory.seen_at).min(),
            Some(100 - MAX_DANGER_MEMORIES as u64)
        );

        let state = crate::Scenario::skirmish().build().unwrap();
        let (_, orientation) = bot.observe(&state);
        let source = TilePos::new(7, 2);
        let strength = recovery_uncertainty_floor();
        bot.danger.clear();
        bot.remember_danger(source, strength, 0);
        assert_eq!(bot.danger_strength_at(0, source, &orientation), strength);
        assert_eq!(
            bot.danger_strength_at(DANGER_MEMORY_TICKS / 2, source, &orientation),
            strength / 2
        );
        assert_eq!(
            bot.danger_strength_at(DANGER_MEMORY_TICKS, source, &orientation),
            0
        );
    }

    #[test]
    fn recovery_danger_allows_escape_without_allowing_entry() {
        let danger = TilePos::new(10, 10);
        let inside = TilePos::new(5, 10);
        assert!(!recovery_ring_blocks(
            inside,
            TilePos::new(4, 10),
            danger,
            DANGER_RADIUS
        ));
        assert!(!recovery_ring_blocks(
            inside,
            TilePos::new(5, 11),
            danger,
            DANGER_RADIUS
        ));
        assert!(recovery_ring_blocks(
            inside,
            TilePos::new(6, 10),
            danger,
            DANGER_RADIUS
        ));
        assert!(recovery_ring_blocks(
            TilePos::new(2, 10),
            TilePos::new(3, 10),
            danger,
            DANGER_RADIUS
        ));
    }

    #[test]
    fn recovery_route_uses_visible_siege_weapon_reach() {
        let danger = TilePos::new(0, 0);
        let worker = TilePos::new(2, 0);
        let bombard_tile = TilePos::new(10, 0);
        assert!(bombard_tile.chebyshev(danger) > DANGER_RADIUS);
        let bombard_reach = UnitKind::Bombard
            .stats()
            .max_range_vs(Domain::Ground)
            .unwrap()
            + crate::stats::HARVEST_MOBILE_DANGER_MARGIN;
        assert!(recovery_reach_contains(
            danger.center().dist_sq(bombard_tile.center()),
            bombard_reach,
        ));
        assert!(
            danger.center().dist_sq(worker.center())
                < danger.center().dist_sq(bombard_tile.center()),
            "moving outward from inside visible siege reach is still unsafe"
        );

        let bastion = BuildingKind::Bastion;
        let bastion_anchor = TilePos::new(0, 0);
        let bastion_tile = TilePos::new(10, 0);
        assert!(bastion_tile.chebyshev(bastion_anchor) > DANGER_RADIUS);
        let bastion_reach = recovery_building_ground_reach(bastion).unwrap()
            + crate::stats::HARVEST_STATIC_DANGER_MARGIN;
        let size = bastion.stats().size;
        let tile_point = bastion_tile.center();
        assert!(recovery_reach_contains(
            recovery_rect_closest_point(bastion_anchor, size, tile_point).dist_sq(tile_point),
            bastion_reach,
        ));
    }

    #[test]
    fn defense_reach_is_derived_from_the_weapon_role() {
        assert_eq!(defense_reach2(BuildingKind::Turret), (0, 10));
        assert_eq!(defense_reach2(BuildingKind::FlakTurret), (0, 11));
        assert_eq!(
            defense_reach2(BuildingKind::Bastion),
            (5, 19),
            "the Bastion placement annulus must track its close-pressure dead zone and \
             artillery-parity reach"
        );
    }

    #[test]
    fn defense_range_uses_euclidean_weapon_reach() {
        let center = Point2::tile(TilePos::new(2, 2));
        assert!(center.within_reach(Point2::tile(TilePos::new(7, 2)), 0, 10));
        assert!(
            !center.within_reach(Point2::tile(TilePos::new(6, 6)), 0, 10),
            "the corner of the old Chebyshev square lies beyond a five-tile weapon radius"
        );
        assert!(
            !center.within_reach(Point2::tile(TilePos::new(3, 2)), 5, 19),
            "the Bastion's minimum range is an annulus, not a square"
        );
    }

    #[test]
    fn defense_routes_sample_the_other_twin_lane() {
        let rows = [
            "###############",
            "#......#......#",
            "#.............#",
            "#......#......#",
            "#......#......#",
            "#......#......#",
            "#.............#",
            "#......#......#",
            "###############",
        ];
        let width = i32::try_from(rows[0].len()).unwrap();
        let height = i32::try_from(rows.len()).unwrap();
        let route_open: Vec<_> = rows
            .iter()
            .flat_map(|row| row.bytes().map(|tile| tile != b'#'))
            .collect();
        let passability = KnownPassability {
            width,
            height,
            terrain_open: route_open.clone(),
            air_open: vec![true; route_open.len()],
            route_open,
        };
        let start = TilePos::new(13, 4);
        let goal = TilePos::new(1, 4);
        let mut primary = chassis::path::astar(
            width,
            height,
            start,
            goal,
            |tile| passability.route_open(tile),
            u32::try_from(width * height * 2).unwrap(),
        )
        .unwrap();
        primary.insert(0, start);
        let alternate = alternate_ground_route(
            &passability,
            start,
            goal,
            &primary,
            u32::try_from(width * height * 2).unwrap(),
        )
        .unwrap();

        let upper_gap = TilePos::new(7, 2);
        let lower_gap = TilePos::new(7, 6);
        assert!(primary.contains(&upper_gap) ^ primary.contains(&lower_gap));
        assert!(alternate.contains(&upper_gap) ^ alternate.contains(&lower_gap));
        assert_ne!(
            primary.contains(&upper_gap),
            alternate.contains(&upper_gap),
            "the alternate must sample the parallel lane, not retrace the deterministic primary"
        );
    }

    #[test]
    fn defense_routes_reject_a_remote_detour_as_a_bypass() {
        let width = 7;
        let height = 9;
        let mut route_open = vec![false; usize::try_from(width * height).unwrap()];
        let mut open = |tile| {
            let index = KnownPassability::index_for(width, height, tile).unwrap();
            route_open[index] = true;
        };
        for x in 1..=5 {
            open(TilePos::new(x, 1));
            open(TilePos::new(x, 7));
        }
        for y in 1..=7 {
            open(TilePos::new(1, y));
            open(TilePos::new(5, y));
        }
        let passability = KnownPassability {
            width,
            height,
            terrain_open: route_open.clone(),
            air_open: vec![true; route_open.len()],
            route_open,
        };
        let start = TilePos::new(1, 1);
        let goal = TilePos::new(5, 1);
        let primary: Vec<_> = (1..=5).map(|x| TilePos::new(x, 1)).collect();
        let chosen = alternate_ground_route(
            &passability,
            start,
            goal,
            &primary,
            u32::try_from(width * height * 2).unwrap(),
        )
        .unwrap();

        assert_eq!(defense_route_travel(&primary), Some(40));
        assert_eq!(chosen, primary);
        assert!(
            !chosen.contains(&TilePos::new(3, 7)),
            "a four-times-longer shelf-edge loop is not a useful battlefield bypass"
        );
    }

    #[test]
    fn defense_routes_keep_a_shorter_pareto_prefix() {
        let rows = ["......", ".##...", "..#.#.", "......", "#..#..", "..##.#"];
        let width = i32::try_from(rows[0].len()).unwrap();
        let height = i32::try_from(rows.len()).unwrap();
        let route_open: Vec<_> = rows
            .iter()
            .flat_map(|row| row.bytes().map(|tile| tile != b'#'))
            .collect();
        let passability = KnownPassability {
            width,
            height,
            terrain_open: route_open.clone(),
            air_open: vec![true; route_open.len()],
            route_open,
        };
        let start = TilePos::new(0, 3);
        let goal = TilePos::new(5, 3);
        let primary: Vec<_> = (0..=5).map(|x| TilePos::new(x, 3)).collect();
        let alternate = alternate_ground_route(
            &passability,
            start,
            goal,
            &primary,
            u32::try_from(width * height * 4).unwrap(),
        )
        .expect("a feasible alternate remains below twice the primary travel");

        assert_ne!(alternate, primary);
        assert!(
            defense_route_travel(&alternate).unwrap()
                <= defense_route_travel(&primary).unwrap() * DEFENSE_ALTERNATE_MAX_STRETCH
        );
    }

    #[test]
    fn defense_route_budget_counts_pareto_labels() {
        let rows = [
            "....##..", ".PPP....", "P.#P#...", "P#.P..##", "S#.P.#PG", "#.#P#.P#", "##.P##P.",
            "#..PPPP#",
        ];
        let width = i32::try_from(rows[0].len()).unwrap();
        let height = i32::try_from(rows.len()).unwrap();
        let route_open: Vec<_> = rows
            .iter()
            .flat_map(|row| row.bytes().map(|tile| tile != b'#'))
            .collect();
        let passability = KnownPassability {
            width,
            height,
            terrain_open: route_open.clone(),
            air_open: vec![true; route_open.len()],
            route_open,
        };
        let start = TilePos::new(0, 4);
        let goal = TilePos::new(7, 4);
        let primary = [
            (0, 4),
            (0, 3),
            (0, 2),
            (1, 1),
            (2, 1),
            (3, 1),
            (3, 2),
            (3, 3),
            (3, 4),
            (3, 5),
            (3, 6),
            (3, 7),
            (4, 7),
            (5, 7),
            (6, 7),
            (6, 6),
            (6, 5),
            (6, 4),
            (7, 4),
        ]
        .map(|(x, y)| TilePos::new(x, y));
        let alternate = alternate_ground_route(
            &passability,
            start,
            goal,
            &primary,
            u32::try_from(width * height * 2).unwrap(),
        )
        .expect("the label frontier must not exhaust a tile-sized search budget");

        assert_ne!(alternate, primary);
        assert!(
            defense_route_travel(&alternate).unwrap()
                <= defense_route_travel(&primary).unwrap() * DEFENSE_ALTERNATE_MAX_STRETCH
        );
    }

    #[test]
    fn known_scrap_blocks_ground_routes_but_not_fire_or_airspace() {
        let state = crate::Scenario::skirmish().build().unwrap();
        let obs = Observation::omniscient(&state, PlayerId(0));
        let node = obs
            .known_scrap
            .iter()
            .find_map(|(tile, amount)| (*amount > 0).then_some(*tile))
            .expect("the skirmish fixture carries salvage");
        let passability = KnownPassability::from_observation(&obs);
        assert!(!passability.route_open(node));
        assert!(passability.terrain_open(node));
        assert!(passability.air_open(node));
    }

    #[test]
    fn bypass_score_uses_the_less_covered_route_variant() {
        let passability = KnownPassability {
            width: 5,
            height: 5,
            terrain_open: vec![true; 25],
            air_open: vec![true; 25],
            route_open: vec![true; 25],
        };
        let traffic = DefenseTraffic {
            approaches: vec![DefenseApproach {
                value: 100,
                routes: vec![
                    (0..5).map(|x| TilePos::new(x, 1)).collect(),
                    (0..5).map(|x| TilePos::new(x, 3)).collect(),
                ],
            }],
            passability,
            kind: BuildingKind::Turret,
            ground_routes: true,
        };
        let (_, _, one_lane) = traffic.score_at(Point2::tile(TilePos::new(2, 1)), 0, 2);
        let (_, _, both_lanes) = traffic.score_at(Point2::tile(TilePos::new(2, 2)), 0, 2);
        assert_eq!(one_lane, 0);
        assert!(
            both_lanes > one_lane,
            "bypass value comes from the route an emplacement covers least"
        );
    }

    #[test]
    fn flak_coverage_counts_airspace_over_known_ground_barriers() {
        let mut terrain_open = vec![true; 25];
        terrain_open[0] = false;
        let passability = KnownPassability {
            width: 5,
            height: 5,
            route_open: terrain_open.clone(),
            terrain_open,
            air_open: vec![true; 25],
        };
        let center = Point2::tile(TilePos::new(2, 2));
        assert_eq!(
            known_open_coverage(&passability, center, 0, 10, BuildingKind::Turret),
            24
        );
        assert_eq!(
            known_open_coverage(&passability, center, 0, 10, BuildingKind::FlakTurret,),
            25,
            "anti-air coverage includes flyable space over known ground barriers"
        );
        let air_traffic = DefenseTraffic {
            approaches: vec![DefenseApproach {
                value: 100,
                routes: vec![straight_air_route(TilePos::new(0, 0), TilePos::new(4, 4))],
            }],
            passability,
            kind: BuildingKind::FlakTurret,
            ground_routes: false,
        };
        let (traffic, choke, bypass) = air_traffic.score_at(center, 0, 10);
        assert!(traffic > 0);
        assert_eq!((choke, bypass), (0, 0));
    }

    #[test]
    fn peak_ridges_block_every_defense_role_and_air_ingress() {
        let width = 7;
        let height = 5;
        let mut terrain_open = vec![true; usize::try_from(width * height).unwrap()];
        let mut air_open = terrain_open.clone();
        for y in 0..height - 1 {
            let index = KnownPassability::index_for(width, height, TilePos::new(3, y)).unwrap();
            terrain_open[index] = false;
            air_open[index] = false;
        }
        let passability = KnownPassability {
            width,
            height,
            route_open: terrain_open.clone(),
            terrain_open,
            air_open,
        };
        let west = Point2::tile(TilePos::new(1, 2));
        let east = Point2::tile(TilePos::new(5, 2));
        for kind in [
            BuildingKind::Turret,
            BuildingKind::FlakTurret,
            BuildingKind::Bastion,
        ] {
            assert!(
                !passability.fire_clear(kind, west, east),
                "{kind:?} must not receive coverage through a peak ridge"
            );
        }

        let route = known_air_route(
            &passability,
            west.as_tile(),
            east.as_tile(),
            u32::try_from(width * height * 2).unwrap(),
        )
        .expect("the ridge leaves one flyable gap");
        assert!(route.contains(&TilePos::new(3, height - 1)));
        assert!(route.iter().all(|tile| passability.air_open(*tile)));

        let mut rock_only = passability.clone();
        rock_only.air_open.fill(true);
        assert!(!rock_only.fire_clear(BuildingKind::Turret, west, east));
        assert!(rock_only.fire_clear(BuildingKind::FlakTurret, west, east));
        assert!(rock_only.fire_clear(BuildingKind::Bastion, west, east));
    }

    #[test]
    fn defense_scoring_rewards_builder_safety_and_low_congestion() {
        let bounds = DefenseBounds {
            threat: (0, 0),
            protected_value: (0, 0),
            coverage: (0, 0),
            traffic: (0, 0),
            choke: (0, 0),
            bypass: (0, 0),
            vision: (0, 0),
            spacing: (0, 0),
            congestion: (0, 10),
            builder_safety: (0, 10),
            builder_travel: (0, 0),
        };
        let exposed = DefenseMetrics {
            threat: 0,
            protected_value: 0,
            coverage: 0,
            traffic: 0,
            choke: 0,
            bypass: 0,
            vision: 0,
            spacing: 0,
            congestion: 10,
            builder_safety: 0,
            builder_travel: 0,
        };
        let safe = DefenseMetrics {
            congestion: 0,
            builder_safety: 10,
            ..exposed
        };
        for kind in [
            BuildingKind::Turret,
            BuildingKind::FlakTurret,
            BuildingKind::Bastion,
        ] {
            assert!(
                safe.score(kind, bounds) > exposed.score(kind, bounds),
                "{kind:?} placement must not treat a crowded firing lane or an exposed builder \
                 as free"
            );
        }
    }
}
