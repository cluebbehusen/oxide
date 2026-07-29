//! The training interface: a bot whose macro decisions come from
//! outside.
//!
//! A [`GymBot`] is the Phase-A architecture with the policy layer
//! removed and handed to whoever is driving — the PPO trainer over the
//! debug socket's sibling protocol, a scripted test, eventually frozen
//! weights. Chores stay automatic (harvest assignment, orphan-site
//! resume — bookkeeping nobody needs to learn), the executive keeps all
//! its micro (focus fire, withdrawal, pullbacks), and the external
//! policy picks **one macro action per think** from a fixed, masked
//! menu the executive instantiates. Everything runs fog-honest and
//! seat-oriented: a learned policy is honest and seat-symmetric by
//! construction.

use super::executive::{ArmyState, Executive, Intent, LoweringRules};
use super::observation::{Observation, UnitObs};
use super::orient::Orientation;
use super::utility::{Dials, UtilityPolicy};
use crate::command::PlayerCommand;
use crate::ids::PlayerId;
use crate::state::State;
use crate::stats::{BuildingKind, Domain, Role, UnitKind};
use chassis::grid::TilePos;

/// Bump when actions or features change shape — recorded checkpoints
/// and shipped weights must refuse mismatched worlds.
pub const GYM_VERSION: u32 = 6;

/// The macro menu, one action per think. Training slots are
/// role-indexed where the factions differ: one action means "train my
/// anti-air", and the seat's faction resolves which machine that is —
/// one action space, two rosters.
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
    /// Send a harvester to weld the deepest-wound own machine.
    RepairUnit = 22,
    /// Start a Repair Bay near home.
    BuildRepairBay = 23,
}

/// Number of actions in [`Action`].
pub const ACTION_COUNT: usize = 24;

/// Number of entries in the feature vector.
pub const FEATURE_COUNT: usize = 65;

/// How far (Manhattan tiles) a free harvester may stand from a wounded
/// machine for [`Action::RepairUnit`] to consider it a patient. The
/// wounded rotate to the rear and the welders live on the economy
/// line, so a home-front weld is always in range — while a patient
/// only reachable by a cross-map march never masks the verb on: a
/// wounded machine can walk, and chasing one is not a weld.
pub const REPAIR_UNIT_RADIUS: i32 = 12;

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
            _ => Action::Idle,
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
        let _ = projected.maintain(self.player, &world, rear);
        let obs = orientation.observe(&world);
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
        ];

        let mut mask = [false; ACTION_COUNT];
        mask[Action::Idle as usize] = true;
        if let Some(h) = home {
            let foundry_open = obs.my_buildings.iter().enumerate().any(|(qi, b)| {
                b.kind == BuildingKind::Foundry && b.built && obs.my_queues[qi].len() < 2
            });
            let fab_open = obs.my_buildings.iter().enumerate().any(|(qi, b)| {
                b.kind == BuildingKind::Fabricator && b.built && obs.my_queues[qi].len() < 2
            });
            let cost = |k: UnitKind| obs.scrap >= k.stats().cost;
            mask[Action::TrainHarvester as usize] = foundry_open && cost(UnitKind::Harvester);
            mask[Action::TrainSentinel as usize] = foundry_open && cost(UnitKind::Sentinel);
            mask[Action::TrainScuttler as usize] = fab_open && cost(UnitKind::Scuttler);
            mask[Action::TrainLancer as usize] = fab_open && cost(UnitKind::Lancer);
            mask[Action::TrainBombard as usize] = fab_open && cost(UnitKind::Bombard);
            // Role slots price the seat's own variant.
            let role_kind = |r: Role| r.unit_for(obs.faction);
            mask[Action::TrainAntiAir as usize] = fab_open && cost(role_kind(Role::AntiAir));
            mask[Action::TrainAirGround as usize] = fab_open && cost(role_kind(Role::AirGround));
            mask[Action::TrainAirAir as usize] = fab_open && cost(role_kind(Role::AirAir));
            let build_cost =
                |k: BuildingKind| k.stats().construction.is_some_and(|c| obs.scrap >= c.cost);
            // A build without a builder lowers to nothing — and worse,
            // the silent no-op gets the anchor blacklisted as refused.
            let builder_free = obs.my_units.iter().any(|u| {
                u.kind == UnitKind::Harvester && u.site.is_none() && !enlisted.contains(&u.id)
            });
            mask[Action::BuildFabricator as usize] = builder_free
                && build_cost(BuildingKind::Fabricator)
                && !obs
                    .my_buildings
                    .iter()
                    .any(|b| b.kind == BuildingKind::Fabricator)
                && self
                    .policy
                    .placement_near(&obs, BuildingKind::Fabricator, h)
                    .is_some();
            mask[Action::BuildTurret as usize] = builder_free
                && build_cost(BuildingKind::Turret)
                && self.policy.nearest_scrap(&obs, h).is_some_and(|node| {
                    self.policy
                        .placement_near(&obs, BuildingKind::Turret, node)
                        .is_some()
                });
            // Flak guards the same ground the Turret does; the pricier
            // emplacements anchor near home like the Fabricator.
            mask[Action::BuildFlak as usize] = builder_free
                && build_cost(BuildingKind::FlakTurret)
                && self.policy.nearest_scrap(&obs, h).is_some_and(|node| {
                    self.policy
                        .placement_near(&obs, BuildingKind::FlakTurret, node)
                        .is_some()
                });
            let near_home = |k: BuildingKind| {
                builder_free && build_cost(k) && self.policy.placement_near(&obs, k, h).is_some()
            };
            mask[Action::BuildBastion as usize] = near_home(BuildingKind::Bastion);
            mask[Action::BuildArray as usize] = near_home(BuildingKind::Array);
            mask[Action::BuildReclaimer as usize] = near_home(BuildingKind::Reclaimer);
            mask[Action::BuildRepairBay as usize] = near_home(BuildingKind::RepairBay);
            // Repair and salvage never share a target: a patient an
            // own crew is stripping is not a patient (the sim evicts
            // the loser anyway; masking keeps the oscillator out of
            // the trained distribution).
            let under_salvage: Vec<crate::ids::BuildingId> =
                obs.my_units.iter().filter_map(|u| u.salvaging).collect();
            mask[Action::Repair as usize] = builder_free
                && obs.scrap > 0
                && obs.my_buildings.iter().any(|b| {
                    b.built && b.hp < b.kind.stats().max_hp && !under_salvage.contains(&b.id)
                });
            mask[Action::Salvage as usize] = builder_free
                && obs
                    .my_buildings
                    .iter()
                    .any(|b| b.built && SALVAGE_PRIORITY.contains(&b.kind));
            // v6: the weld turns on machines. The patient pick carries
            // its own welder check (a free harvester inside the leash),
            // so `builder_free` alone would both under- and over-claim.
            mask[Action::RepairUnit as usize] =
                obs.scrap > 0 && unit_patient(&obs, &enlisted).is_some();
            mask[Action::AirRaid as usize] = enemy_site.is_some()
                && obs.my_units.iter().any(|u| {
                    let stats = u.kind.stats();
                    stats.domain == Domain::Air
                        && stats.can_target(Domain::Ground)
                        && u.idle
                        && !enlisted.contains(&u.id)
                });
            mask[Action::FormArmy as usize] = idle_fighters > 0;
            // Push commits the main army wherever it is in its life:
            // between two decisions a staging army can enter combat,
            // and an action the policy observed as legal must not
            // no-op on that one-cadence transition.
            mask[Action::Push as usize] = !armies.is_empty() && enemy_site.is_some();
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
        }
        Decision { features, mask }
    }

    /// Applies one chosen action (plus chores and housekeeping) and
    /// returns this tick's commands.
    pub fn step(&mut self, state: &State, action: Action) -> Vec<PlayerCommand> {
        let (world, orientation) = self.observe(state);
        let rear = rear_tile(&world);
        let mut commands = self.exec.maintain(self.player, &world, rear);

        let obs = orientation.observe(&world);
        let Some(home) = home_tile(&obs) else {
            return commands; // eliminated
        };
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

        // The chosen action's intent is lowered FIRST: intents claim
        // units in order, so chores must not grab the action's builder
        // or scout out from under it (the AssignHarvest chore skips
        // already-claimed units).
        let mut intents = Vec::new();

        let staging = armies
            .iter()
            .filter(|a| a.state == ArmyState::Staging)
            .min_by_key(|a| a.id);
        let enemy_site = UtilityPolicy::enemy_site(&obs, home);
        match action {
            Action::Idle => {}
            Action::TrainHarvester => self.train(
                &obs,
                BuildingKind::Foundry,
                UnitKind::Harvester,
                &mut intents,
            ),
            Action::TrainSentinel => self.train(
                &obs,
                BuildingKind::Foundry,
                UnitKind::Sentinel,
                &mut intents,
            ),
            Action::TrainScuttler => self.train(
                &obs,
                BuildingKind::Fabricator,
                UnitKind::Scuttler,
                &mut intents,
            ),
            Action::TrainLancer => self.train(
                &obs,
                BuildingKind::Fabricator,
                UnitKind::Lancer,
                &mut intents,
            ),
            Action::TrainBombard => self.train(
                &obs,
                BuildingKind::Fabricator,
                UnitKind::Bombard,
                &mut intents,
            ),
            Action::TrainAntiAir => self.train(
                &obs,
                BuildingKind::Fabricator,
                Role::AntiAir.unit_for(obs.faction),
                &mut intents,
            ),
            Action::TrainAirGround => self.train(
                &obs,
                BuildingKind::Fabricator,
                Role::AirGround.unit_for(obs.faction),
                &mut intents,
            ),
            Action::TrainAirAir => self.train(
                &obs,
                BuildingKind::Fabricator,
                Role::AirAir.unit_for(obs.faction),
                &mut intents,
            ),
            Action::BuildFabricator => {
                if let Some(anchor) =
                    self.policy
                        .placement_near(&obs, BuildingKind::Fabricator, home)
                {
                    self.policy.note_pending_site(anchor);
                    intents.push(Intent::Build {
                        kind: BuildingKind::Fabricator,
                        anchor,
                    });
                }
            }
            Action::BuildTurret | Action::BuildFlak => {
                let kind = if action == Action::BuildTurret {
                    BuildingKind::Turret
                } else {
                    BuildingKind::FlakTurret
                };
                if let Some(anchor) = self
                    .policy
                    .nearest_scrap(&obs, home)
                    .and_then(|node| self.policy.placement_near(&obs, kind, node))
                {
                    self.policy.note_pending_site(anchor);
                    intents.push(Intent::Build { kind, anchor });
                }
            }
            Action::BuildBastion
            | Action::BuildArray
            | Action::BuildReclaimer
            | Action::BuildRepairBay => {
                let kind = match action {
                    Action::BuildBastion => BuildingKind::Bastion,
                    Action::BuildArray => BuildingKind::Array,
                    Action::BuildRepairBay => BuildingKind::RepairBay,
                    _ => BuildingKind::Reclaimer,
                };
                if let Some(anchor) = self.policy.placement_near(&obs, kind, home) {
                    self.policy.note_pending_site(anchor);
                    intents.push(Intent::Build { kind, anchor });
                }
            }
            Action::Repair => {
                // Same pick as the scripted repair channel: deepest
                // wound first, ties toward the map origin then id —
                // skipping anything an own crew is stripping.
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
                }
            }
            Action::Salvage => {
                // Cheapest-and-least-useful first, ties toward the
                // map origin then id — one deterministic pick, like
                // every other lowering.
                let target = obs
                    .my_buildings
                    .iter()
                    .filter(|b| b.built)
                    .filter_map(|b| {
                        SALVAGE_PRIORITY
                            .iter()
                            .position(|k| *k == b.kind)
                            .map(|rank| (rank, b.anchor.y, b.anchor.x, b.id))
                    })
                    .min()
                    .map(|(.., id)| id);
                if let Some(building) = target {
                    intents.push(Intent::Salvage { building });
                }
            }
            Action::AirRaid => {
                // The wing flies at the enemy's work: the nearest known
                // harvester, else the known site. Whether flak waits
                // there is the policy's problem — lowering is total,
                // judgment is what the network is for.
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
                let rally = self.policy.rally_point(&obs, staging, enemy_site, home);
                let members = staging.map_or(0, |a| a.members.len() as u32);
                intents.push(Intent::FormArmy {
                    staging: rally,
                    size: self.dials.army_size.max(members + 2),
                });
            }
            Action::Push => {
                let main = staging.or_else(|| armies.iter().min_by_key(|a| a.id));
                if let (Some(a), Some(target)) = (main, enemy_site) {
                    intents.push(Intent::PushArmy { army: a.id, target });
                }
            }
            Action::Recall => {
                for a in &armies {
                    intents.push(Intent::RecallArmy { army: a.id });
                }
            }
            Action::RepairUnit => {
                // Same pick as the mask promised: deepest wound first,
                // ties toward the map origin then id, leashed to a
                // welder actually nearby.
                if let Some(unit) = unit_patient(&obs, &enlisted) {
                    intents.push(Intent::RepairUnit { unit });
                }
            }
            Action::Scout => {
                self.policy
                    .scouting(&obs, home, &enlisted, true, &mut intents);
            }
        }

        // Chores after the action: idle harvesters to work (with the
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

/// The weld verb's patient: the deepest-wound own ground machine (air
/// patients refuse in the sim) with a free harvester inside
/// [`REPAIR_UNIT_RADIUS`], ties toward the map origin then id — the
/// building repair channel's pick discipline pointed at machines.
/// Fog-safe: own-state only. Both the mask and the lowering call this,
/// so what the policy observed as legal is what the step emits.
fn unit_patient(obs: &Observation, enlisted: &[crate::ids::UnitId]) -> Option<crate::ids::UnitId> {
    let welder_near = |patient: &UnitObs| {
        obs.my_units.iter().any(|u| {
            u.kind == UnitKind::Harvester
                && u.site.is_none()
                && u.id != patient.id
                && !enlisted.contains(&u.id)
                && u.tile.manhattan(patient.tile) <= REPAIR_UNIT_RADIUS
        })
    };
    obs.my_units
        .iter()
        .filter(|u| {
            let stats = u.kind.stats();
            stats.domain == Domain::Ground && u.hp < stats.max_hp && welder_near(u)
        })
        .map(|u| {
            let deficit = u.kind.stats().max_hp - u.hp;
            (std::cmp::Reverse(deficit), u.tile.y, u.tile.x, u.id)
        })
        .min()
        .map(|(.., id)| id)
}

fn home_tile(obs: &Observation) -> Option<TilePos> {
    obs.my_buildings
        .iter()
        .filter(|b| b.kind == BuildingKind::Foundry && b.built)
        .min_by_key(|b| b.id)
        .map(|b| b.anchor)
}
