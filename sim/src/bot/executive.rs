//! The bot executive: intent in, commands out, armies in between.
//!
//! Policies — scripted or learned — never touch [`crate::PlayerCommand`]
//! directly. They emit [`Intent`]s against an [`super::Observation`], and
//! the executive owns everything an intent leaves unsaid: which harvester
//! builds, which fighters join an army, when a pushing army counts as
//! engaged, and when an engagement has gone wrong enough to withdraw.
//! The army lifecycle (Staging → Pushing → Engaging → Withdrawing) is the
//! anti-trickle machinery: units gather before they march, march together,
//! and leave together when the fight turns.
//!
//! Everything here is deterministic given the observation: member choice,
//! targeting, and state transitions order by explicit keys ending in ids.

use super::observation::{Observation, UnitObs};
use crate::command::{Command, PlayerCommand};
use crate::ids::{PlayerId, UnitId};
use crate::stats::{BuildingKind, UnitKind};
use chassis::grid::TilePos;
use serde::{Deserialize, Serialize};

/// Stable handle for an army within one bot's executive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ArmyId(pub u32);

/// Where an army is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArmyState {
    /// Gathering at the staging point until the policy commits it.
    Staging,
    /// Marching on a target as one body.
    Pushing,
    /// In contact: the executive is scoring the fight every think.
    Engaging,
    /// The fight turned: falling back to the staging point.
    Withdrawing,
}

/// A body of fighters managed as one thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Army {
    /// Handle.
    pub id: ArmyId,
    /// Members, id order. Pruned of the dead every think.
    pub members: Vec<UnitId>,
    /// Lifecycle state.
    pub state: ArmyState,
    /// Where this army gathers and falls back to.
    pub staging: TilePos,
    /// Where it was last sent.
    pub target: Option<TilePos>,
}

/// What a policy may ask for. Intents mutate executive bookkeeping or
/// request commands; they are not commands themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Intent {
    /// Queue a unit at an own factory.
    TrainAt {
        /// The producing building.
        building: crate::ids::BuildingId,
        /// What to build.
        kind: UnitKind,
    },
    /// Start a construction site (the executive picks the builder).
    Build {
        /// What to construct.
        kind: BuildingKind,
        /// Footprint anchor.
        anchor: TilePos,
    },
    /// Create a new army staged at a rally point. Idle fighters not
    /// already enlisted are drafted, nearest first, up to `size`.
    FormArmy {
        /// Gather point.
        staging: TilePos,
        /// How many fighters to draft.
        size: u32,
    },
    /// Commit a staged (or withdrawn) army against a target.
    PushArmy {
        /// Which army.
        army: ArmyId,
        /// Where to attack toward.
        target: TilePos,
    },
    /// Pull an army back to its staging point.
    RecallArmy {
        /// Which army.
        army: ArmyId,
    },
    /// Put a harvester on a node.
    AssignHarvest {
        /// The harvester.
        unit: UnitId,
        /// The node tile.
        node: TilePos,
    },
    /// Send a unit to look at a tile (a plain move — scouts are
    /// expendable, not brave).
    Scout {
        /// The scout.
        unit: UnitId,
        /// Where to look.
        to: TilePos,
    },
}

/// Fraction of max hp below which a member is rotated out of its army to
/// the rear — permanently: nothing heals in this world, and cycling the
/// wounded back in just feeds the grinder.
const PULLBACK_NUM: u32 = 35;
const PULLBACK_DEN: u32 = 100;

/// An engagement is "ours" while own strength × this margin ≥ enemy
/// strength near the fight; hysteresis so armies don't dither.
const WITHDRAW_MARGIN_NUM: u32 = 10;
const WITHDRAW_MARGIN_DEN: u32 = 13; // withdraw below ~77% relative strength
/// Radius (tiles) around the army centroid scored as "the fight".
const ENGAGE_RADIUS: i32 = 8;
/// A pushing army is engaged once enemies are inside this radius.
const CONTACT_RADIUS: i32 = 6;

/// The layer between policies and the sim. One per bot; carries across
/// ticks (armies are memory, legitimately — a bot is a command source,
/// not sim state).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Executive {
    armies: Vec<Army>,
    next_army: u32,
    /// Rear-line members rotated out for good (kept so re-drafts skip
    /// them; pruned when they die).
    rear: Vec<UnitId>,
}

impl Executive {
    /// Fresh, armyless.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read access for policies and tests.
    pub fn armies(&self) -> &[Army] {
        &self.armies
    }

    /// Ids of units already spoken for (army members and the rear line) —
    /// the pool FormArmy and harvest assignment must not double-book.
    pub fn enlisted(&self) -> impl Iterator<Item = UnitId> + '_ {
        self.armies
            .iter()
            .flat_map(|a| a.members.iter().copied())
            .chain(self.rear.iter().copied())
    }

    /// Applies a think's intents, in order, returning the commands they
    /// lower to. Deterministic given (self, obs, intents).
    pub fn apply(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        intents: &[Intent],
    ) -> Vec<PlayerCommand> {
        let mut out = Vec::new();
        for intent in intents {
            match intent {
                Intent::TrainAt { building, kind } => out.push(PlayerCommand {
                    player: me,
                    command: Command::Train {
                        building: *building,
                        kind: *kind,
                    },
                }),
                Intent::Build { kind, anchor } => {
                    if let Some(builder) = self.free_harvester(obs, *anchor) {
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Build {
                                units: vec![builder],
                                kind: *kind,
                                anchor: *anchor,
                            },
                        });
                    }
                }
                Intent::FormArmy { staging, size } => {
                    let draft = self.draft(obs, *staging, *size);
                    if !draft.is_empty() {
                        let id = ArmyId(self.next_army);
                        self.next_army += 1;
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units: draft.clone(),
                                goal: *staging,
                                queue: false,
                            },
                        });
                        self.armies.push(Army {
                            id,
                            members: draft,
                            state: ArmyState::Staging,
                            staging: *staging,
                            target: None,
                        });
                    }
                }
                Intent::PushArmy { army, target } => {
                    if let Some(a) = self.armies.iter_mut().find(|a| a.id == *army)
                        && !a.members.is_empty()
                    {
                        a.state = ArmyState::Pushing;
                        a.target = Some(*target);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units: a.members.clone(),
                                goal: *target,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::RecallArmy { army } => {
                    if let Some(a) = self.armies.iter_mut().find(|a| a.id == *army)
                        && !a.members.is_empty()
                    {
                        a.state = ArmyState::Withdrawing;
                        a.target = None;
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Move {
                                units: a.members.clone(),
                                goal: a.staging,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::AssignHarvest { unit, node } => out.push(PlayerCommand {
                    player: me,
                    command: Command::Harvest {
                        units: vec![*unit],
                        node: *node,
                        queue: false,
                    },
                }),
                Intent::Scout { unit, to } => out.push(PlayerCommand {
                    player: me,
                    command: Command::Move {
                        units: vec![*unit],
                        goal: *to,
                        queue: false,
                    },
                }),
            }
        }
        out
    }

    /// The per-think housekeeping no policy should have to ask for: prune
    /// the dead, rotate the wounded to the rear, advance army states, and
    /// withdraw from fights that have turned. Returns the commands the
    /// transitions demand.
    pub fn maintain(&mut self, me: PlayerId, obs: &Observation) -> Vec<PlayerCommand> {
        let mut out = Vec::new();
        let alive = |id: UnitId| obs.my_units.iter().any(|u| u.id == id);
        self.rear.retain(|id| alive(*id));
        for army in &mut self.armies {
            army.members.retain(|id| alive(*id));

            // Rotate the badly wounded out — permanently.
            let mut pulled: Vec<UnitId> = Vec::new();
            army.members.retain(|id| {
                let Some(u) = obs.my_units.iter().find(|u| u.id == *id) else {
                    return false;
                };
                let max = u.kind.stats().max_hp;
                if u.hp * PULLBACK_DEN < max * PULLBACK_NUM {
                    pulled.push(*id);
                    false
                } else {
                    true
                }
            });
            if !pulled.is_empty() {
                out.push(PlayerCommand {
                    player: me,
                    command: Command::Move {
                        units: pulled.clone(),
                        goal: army.staging,
                        queue: false,
                    },
                });
                self.rear.extend(pulled);
            }
            if army.members.is_empty() {
                continue; // swept below
            }

            let centroid = centroid(&army.members, obs);
            match army.state {
                ArmyState::Pushing => {
                    if enemies_near(obs, centroid, CONTACT_RADIUS) {
                        army.state = ArmyState::Engaging;
                    }
                }
                ArmyState::Engaging => {
                    let (mine, theirs) = local_strength(obs, &army.members, centroid);
                    if theirs == 0 {
                        // Fight's over here; march on if a target remains.
                        army.state = match army.target {
                            Some(_) => ArmyState::Pushing,
                            None => ArmyState::Staging,
                        };
                        if let Some(target) = army.target {
                            out.push(PlayerCommand {
                                player: me,
                                command: Command::AttackMove {
                                    units: army.members.clone(),
                                    goal: target,
                                    queue: false,
                                },
                            });
                        }
                    } else if mine * u64::from(WITHDRAW_MARGIN_DEN)
                        < theirs * u64::from(WITHDRAW_MARGIN_NUM)
                    {
                        // Losing decisively: leave together, obliviously —
                        // an orderly retreat does not stop to trade.
                        army.state = ArmyState::Withdrawing;
                        army.target = None;
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Move {
                                units: army.members.clone(),
                                goal: army.staging,
                                queue: false,
                            },
                        });
                    }
                }
                ArmyState::Withdrawing => {
                    if tiles_within(centroid, army.staging, 2) {
                        army.state = ArmyState::Staging;
                    }
                }
                ArmyState::Staging => {}
            }
        }
        self.armies.retain(|a| !a.members.is_empty());
        out
    }

    /// The nearest own harvester to `anchor` that isn't enlisted, for
    /// construction. Working ones are fair game (the economy re-hires).
    fn free_harvester(&self, obs: &Observation, anchor: TilePos) -> Option<UnitId> {
        let enlisted: Vec<UnitId> = self.enlisted().collect();
        obs.my_units
            .iter()
            .filter(|u| u.kind == UnitKind::Harvester && !enlisted.contains(&u.id))
            .map(|u| (u.tile.manhattan(anchor), u.id))
            .min()
            .map(|(_, id)| id)
    }

    /// Drafts up to `size` un-enlisted fighters, nearest to the staging
    /// point first, ties to the lowest id.
    fn draft(&self, obs: &Observation, staging: TilePos, size: u32) -> Vec<UnitId> {
        let enlisted: Vec<UnitId> = self.enlisted().collect();
        let mut candidates: Vec<(i32, UnitId)> = obs
            .my_units
            .iter()
            .filter(|u| u.kind.stats().attack.is_some() && u.idle && !enlisted.contains(&u.id))
            .map(|u| (u.tile.manhattan(staging), u.id))
            .collect();
        candidates.sort_unstable();
        candidates
            .into_iter()
            .take(size as usize)
            .map(|(_, id)| id)
            .collect()
    }
}

/// Mean member tile (integer division — a macro-scale center).
fn centroid(members: &[UnitId], obs: &Observation) -> TilePos {
    let mut n = 0i32;
    let (mut sx, mut sy) = (0i64, 0i64);
    for u in obs.my_units.iter().filter(|u| members.contains(&u.id)) {
        sx += i64::from(u.tile.x);
        sy += i64::from(u.tile.y);
        n += 1;
    }
    if n == 0 {
        TilePos::new(0, 0)
    } else {
        TilePos::new((sx / i64::from(n)) as i32, (sy / i64::from(n)) as i32)
    }
}

fn enemies_near(obs: &Observation, at: TilePos, radius: i32) -> bool {
    obs.enemy_units
        .iter()
        .any(|u| u.tile.chebyshev(at) <= radius)
}

/// hp-weighted dps sums inside the engagement radius: (mine, theirs).
/// Integer math, deterministic; dps is damage per 100 ticks to stay in
/// integers (cooldowns divide 100 unevenly — close enough for a margin
/// call that carries hysteresis anyway).
fn local_strength(obs: &Observation, members: &[UnitId], at: TilePos) -> (u64, u64) {
    let weight = |u: &UnitObs| -> u64 {
        let Some(atk) = u.kind.stats().attack else {
            return 0;
        };
        let dps100 = u64::from(atk.damage) * 100 / u64::from(atk.cooldown_ticks);
        u64::from(u.hp) * dps100
    };
    let mine: u64 = obs
        .my_units
        .iter()
        .filter(|u| members.contains(&u.id) && u.tile.chebyshev(at) <= ENGAGE_RADIUS)
        .map(weight)
        .sum();
    let theirs: u64 = obs
        .enemy_units
        .iter()
        .filter(|u| u.tile.chebyshev(at) <= ENGAGE_RADIUS)
        .map(weight)
        .sum();
    (mine, theirs)
}

fn tiles_within(a: TilePos, b: TilePos, radius: i32) -> bool {
    a.chebyshev(b) <= radius
}
