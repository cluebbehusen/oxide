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
use super::utility::{Dials, UtilityPolicy};
use crate::command::{Command, PlayerCommand};
use crate::ids::PlayerId;
use crate::state::State;
use crate::stats::{BuildingKind, Domain, Role, UnitKind};
use chassis::grid::TilePos;

/// Bump when actions or features change shape — recorded checkpoints
/// and shipped weights must refuse mismatched worlds.
pub const GYM_VERSION: u32 = 7;

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

/// One externally-driven bot. Same command-source shape as the other
/// bots: everything it does goes through recorded `PlayerCommand`s.
#[derive(Debug, Clone)]
pub struct GymBot {
    player: PlayerId,
    dials: Dials,
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
        Self {
            player,
            dials: Dials {
                cadence: cadence.clamp(4, 64),
                ..Dials::full()
            },
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
        self.remember(&world);
        let rear = rear_tile(&world);
        let mut projected = self.exec.clone();
        let _ = projected.maintain_repair_capable(self.player, &world, rear);
        let obs = orientation.observe(&world);
        self.refresh_founding_claims(&obs);
        self.reconcile_plan(&obs);
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
            .map(|building| {
                building
                    .kind
                    .stats()
                    .construction
                    .map_or(0, |construction| i64::from(construction.cost))
            })
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
            .filter_map(|b| {
                b.kind.stats().construction.map(|construction| {
                    i64::from(construction.cost) * i64::from(b.hp)
                        / i64::from(b.kind.stats().max_hp)
                })
            })
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
                .map(|b| {
                    b.kind
                        .stats()
                        .construction
                        .map_or(0i64, |c| i64::from(c.cost))
                })
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
                mask[action as usize] =
                    screen_ready && self.can_plan_build(&obs, &enlisted, h, kind);
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
                for action in OPERATION_ACTIONS {
                    mask[action] = action == defense as usize;
                }
            }
        }
        if let Some(recovery) = projected.harvester_recovery(self.player, &obs) {
            mask.fill(false);
            let action = if recovery.is_empty() {
                Action::Idle
            } else {
                Action::TrainHarvester
            };
            mask[action as usize] = true;
            mask[Action::NoConstruction as usize] = true;
            mask[Action::NoOperation as usize] = true;
        }
        Decision { features, mask }
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
        let rear = rear_tile(&world);
        let mut commands = self.exec.maintain_repair_capable(self.player, &world, rear);

        let obs = orientation.observe(&world);
        self.refresh_founding_claims(&obs);
        self.reconcile_plan(&obs);
        commands.extend(self.cancel_stale_founding(&obs));
        let Some(home) = home_tile(&obs) else {
            return commands; // eliminated
        };
        if let Some(recovery) = self.exec.harvester_recovery(self.player, &obs) {
            commands.extend(recovery);
            return commands;
        }
        let armies: Vec<_> = self
            .exec
            .armies()
            .iter()
            .map(|a| orientation.army(a.clone()))
            .collect();
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

        if let Some(kind) = plan.construction.building()
            && self.can_plan_build(&obs, &enlisted, home, kind)
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
    ) -> bool {
        obs.tick >= self.capital_retry_after
            && committed_buildings(obs, kind) < building_cap(kind)
            && self.unpaid_claim_reserve(obs) == 0
            && !self
                .build_retry_after
                .iter()
                .any(|(blocked, retry_after)| *blocked == kind && obs.tick < *retry_after)
            && free_builder(obs, enlisted)
            && self.build_anchor(obs, home, kind).is_some()
    }

    fn build_anchor(
        &self,
        obs: &Observation,
        home: TilePos,
        kind: BuildingKind,
    ) -> Option<TilePos> {
        match kind {
            BuildingKind::Turret | BuildingKind::FlakTurret => self
                .policy
                .nearest_scrap(obs, home)
                .and_then(|node| self.policy.placement_near(obs, kind, node)),
            BuildingKind::Foundry => None,
            _ => self.policy.placement_near(obs, kind, home),
        }
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
        let anchor = self.build_anchor(obs, home, kind)?;
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
