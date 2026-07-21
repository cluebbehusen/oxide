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

use super::executive::{ArmyState, Executive, Intent};
use super::observation::Observation;
use super::orient::Orientation;
use super::utility::{Dials, UtilityPolicy};
use crate::command::PlayerCommand;
use crate::ids::PlayerId;
use crate::state::State;
use crate::stats::{BuildingKind, UnitKind};
use chassis::grid::TilePos;

/// Bump when actions or features change shape — recorded checkpoints
/// and shipped weights must refuse mismatched worlds.
pub const GYM_VERSION: u32 = 2;

/// The macro menu, one action per think.
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
    /// Start a Fabricator near home.
    BuildFabricator = 5,
    /// Start a Turret over the harvest line.
    BuildTurret = 6,
    /// Draft idle fighters into the staging army (or found it).
    FormArmy = 7,
    /// Commit the staging army at the nearest known enemy site.
    Push = 8,
    /// Pull every army back to its rally.
    Recall = 9,
    /// Send a scout now.
    Scout = 10,
}

/// Number of actions in [`Action`].
pub const ACTION_COUNT: usize = 11;

/// Number of entries in the feature vector.
pub const FEATURE_COUNT: usize = 32;

impl Action {
    /// Decodes a policy's choice; out-of-range folds to Idle (the
    /// trainer masks, but a harness must never panic on bad input).
    pub fn from_index(i: usize) -> Action {
        match i {
            1 => Action::TrainHarvester,
            2 => Action::TrainSentinel,
            3 => Action::TrainScuttler,
            4 => Action::TrainLancer,
            5 => Action::BuildFabricator,
            6 => Action::BuildTurret,
            7 => Action::FormArmy,
            8 => Action::Push,
            9 => Action::Recall,
            10 => Action::Scout,
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

    /// Features and action mask at the current tick, oriented. Also
    /// refreshes the fog memory (idempotent at a given tick).
    pub fn decision(&mut self, state: &State) -> Decision {
        let (obs, orientation) = self.observe(state);
        self.remember(&obs);
        let obs = orientation.observe(&obs);
        let home = home_tile(&obs);
        // The executive reconciles in `step` (its transitions emit
        // commands, which a read path must not). But deaths since the
        // last step must not leak into what the policy observes: filter
        // members against the living before deriving any army feature
        // or mask. Lifecycle state can lag one cadence; membership
        // cannot.
        let armies: Vec<_> = self
            .exec
            .armies()
            .iter()
            .map(|a| {
                let mut a = orientation.army(a.clone());
                a.members
                    .retain(|id| obs.my_units.iter().any(|u| u.id == *id));
                a
            })
            .filter(|a| !a.members.is_empty())
            .collect();
        let enlisted: Vec<_> = self.exec.enlisted().collect();

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
        let idle_fighters = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().attack.is_some() && u.idle && !enlisted.contains(&u.id))
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
            home.map_or(-1, |h| i64::from(h.x)),
            home.map_or(-1, |h| i64::from(h.y)),
            enemy_site.map_or(-1, |s| i64::from(s.x)),
            enemy_site.map_or(-1, |s| i64::from(s.y)),
            intel_age,
            (self.seen_strength / 100) as i64,
            seen_age,
            seen_pos.map_or(-1, |p| i64::from(p.x)),
            seen_pos.map_or(-1, |p| i64::from(p.y)),
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
                    && (u.kind == UnitKind::Harvester
                        || (u.idle && u.kind.stats().attack.is_some()))
            });
        }
        Decision { features, mask }
    }

    /// Applies one chosen action (plus chores and housekeeping) and
    /// returns this tick's commands.
    pub fn step(&mut self, state: &State, action: Action) -> Vec<PlayerCommand> {
        let (world, orientation) = self.observe(state);
        let rear = world
            .my_buildings
            .iter()
            .filter(|b| b.kind == BuildingKind::Foundry)
            .min_by_key(|b| b.id)
            .map(|b| b.anchor)
            .unwrap_or(TilePos::new(0, 0));
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
            Action::BuildTurret => {
                if let Some(anchor) = self
                    .policy
                    .nearest_scrap(&obs, home)
                    .and_then(|node| self.policy.placement_near(&obs, BuildingKind::Turret, node))
                {
                    self.policy.note_pending_site(anchor);
                    intents.push(Intent::Build {
                        kind: BuildingKind::Turret,
                        anchor,
                    });
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
            Action::Scout => {
                self.policy
                    .scouting(&obs, home, &enlisted, true, &mut intents);
            }
        }

        // Chores after the action: idle harvesters to work, orphaned
        // sites resumed (paid-for progress must not strand).
        self.policy.economy(&obs, home, &mut intents);
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
        commands.extend(self.exec.apply(self.player, &world, &intents));
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
            .filter(|u| u.kind.stats().attack.is_some())
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

fn home_tile(obs: &Observation) -> Option<TilePos> {
    obs.my_buildings
        .iter()
        .filter(|b| b.kind == BuildingKind::Foundry && b.built)
        .min_by_key(|b| b.id)
        .map(|b| b.anchor)
}
