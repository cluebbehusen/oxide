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
    /// The unit the whole army is concentrating on while engaged.
    /// Spread fire kills nothing; focus deletes one gun at a time.
    pub focus: Option<UnitId>,
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
    /// Draft idle, un-enlisted fighters (nearest first, up to `size`)
    /// into the army staged at this rally point — reinforcing it if one
    /// is already staging there, creating it otherwise. Repeating the
    /// intent every think is how a policy feeds a gathering army without
    /// trickling units into battle.
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
    /// Weld a damaged own building (the executive picks the welder).
    Repair {
        /// The patient.
        building: crate::ids::BuildingId,
    },
    /// Strip an own built building for scrap (the executive picks the
    /// crew; the sim refuses Foundries).
    Salvage {
        /// The building coming down.
        building: crate::ids::BuildingId,
    },
    /// Weld a wounded own machine (the executive picks the welder; the
    /// patient never joins its own crew).
    RepairUnit {
        /// The wounded machine.
        unit: UnitId,
    },
    /// Throw every idle ground-attack flyer at a target — a strike, not
    /// an army: no lifecycle, no withdraw call, just wings and a place.
    RaidAir {
        /// Where the strike flies.
        target: TilePos,
    },
    /// Lift a built own building one tier (the executive picks the
    /// crew, exactly like a repair).
    Upgrade {
        /// The works to lift.
        building: crate::ids::BuildingId,
    },
}

/// Fraction of max hp below which a member is rotated out of its army.
/// A fully healed rear-line veteran becomes draftable again; requiring
/// full health prevents pullback/re-draft oscillation around this line.
const PULLBACK_NUM: u32 = 35;
const PULLBACK_DEN: u32 = 100;

/// Withdraw only from catastrophe: below half the local enemy strength.
/// Nothing in this world outruns its pursuers, so a merely-losing fight
/// finished on the spot costs less than a rout — disengaging under fire
/// is free damage handed to the enemy.
const WITHDRAW_MARGIN_NUM: u32 = 1;
const WITHDRAW_MARGIN_DEN: u32 = 2;
/// Radius (tiles) around the army centroid scored as "the fight".
const ENGAGE_RADIUS: i32 = 8;
/// A pushing army is engaged once enemies are inside this radius.
const CONTACT_RADIUS: i32 = 6;

/// Combat habits a tier can switch off. Fairness note: these change
/// how well the executive fights, never the rules it fights under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doctrine {
    /// Concentrate army fire on one target while engaged.
    pub focus_fire: bool,
    /// Rotate members under 35% hp to the rear between fights.
    pub pullback: bool,
}

impl Default for Doctrine {
    fn default() -> Self {
        Self {
            focus_fire: true,
            pullback: true,
        }
    }
}

/// Per-path lowering rules: the freedoms a command source grants
/// [`Executive::apply_with`] beyond the scripted baseline. The scripted
/// `Brain` tiers are the ladder's anchors and yardsticks — their
/// lowering is frozen at [`LoweringRules::scripted`] so their measured
/// behavior cannot move — while the gym path carries the two
/// amendments that would move them: deferred founding (fog placement
/// Part B) and the Scout-arm claim guard. The guard closes the
/// labor-claims trap (an unconditional Scout replaces the whole
/// program of a machine an earlier intent already bought); it stays
/// off the scripted path because their scouting channel follows its
/// construction claims, and guarding it measurably inverts both
/// ladder gates.
pub struct LoweringRules<'a> {
    /// Judge whether a Build must defer its claim to arrival
    /// ([`crate::Command::Build`]'s `defer`); `None` never defers.
    defer_needed: Option<&'a dyn Fn(BuildingKind, TilePos) -> bool>,
    /// Skip a Scout intent naming a unit an earlier intent claimed
    /// this think.
    scout_honors_claims: bool,
}

impl LoweringRules<'static> {
    /// The frozen baseline the scripted tiers lower under: instant
    /// claims only, Scout unconditional.
    pub fn scripted() -> Self {
        Self {
            defer_needed: None,
            scout_honors_claims: false,
        }
    }
}

impl<'a> LoweringRules<'a> {
    /// The gym path's rules: `defer_needed` mirrors the judgment the
    /// shell's armed click makes (some footprint tile not currently
    /// visible), and Scout keeps off machines the think already spent.
    pub fn gym(defer_needed: &'a dyn Fn(BuildingKind, TilePos) -> bool) -> Self {
        Self {
            defer_needed: Some(defer_needed),
            scout_honors_claims: true,
        }
    }
}

/// The layer between policies and the sim. One per bot; carries across
/// ticks (armies are memory, legitimately — a bot is a command source,
/// not sim state).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Executive {
    armies: Vec<Army>,
    next_army: u32,
    /// Rear-line members kept out of drafts. Repair-capable policies
    /// release them at full health; frozen scripted paths retain them.
    rear: Vec<UnitId>,
    /// Which combat habits this executive practices.
    doctrine: Doctrine,
}

impl Executive {
    /// Fresh, armyless, full doctrine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fresh with an explicit doctrine (how the difficulty tiers strip
    /// combat habits from the lower rungs).
    pub fn with_doctrine(doctrine: Doctrine) -> Self {
        Self {
            doctrine,
            ..Self::default()
        }
    }

    /// Read access for policies and tests.
    pub fn armies(&self) -> &[Army] {
        &self.armies
    }

    /// Rear-line ids currently held out of the draft.
    pub fn rear(&self) -> &[UnitId] {
        &self.rear
    }

    /// Test-only: force a unit onto the rear line, reproducing the
    /// reservation state the 0.14 deadlock replays captured.
    #[cfg(test)]
    pub(crate) fn hold_rear_for_test(&mut self, id: UnitId) {
        if !self.rear.contains(&id) {
            self.rear.push(id);
            self.rear.sort_unstable();
        }
    }

    /// Releases every rear-held unit whose id satisfies `pick` back to
    /// the draftable pool. Recovery adopts its screens through this:
    /// a wounded fighter parked on the rear line is a real screen the
    /// emergency controller must be able to use, or the reservation and
    /// the recovery suppression deadlock each other (the exact stall
    /// the 0.14 replay forensics pinned).
    pub fn release_rear_where(&mut self, pick: impl Fn(UnitId) -> bool) {
        self.rear.retain(|id| !pick(*id));
    }

    /// Ids of units already spoken for (army members and the rear line) —
    /// the pool FormArmy and harvest assignment must not double-book.
    pub fn enlisted(&self) -> impl Iterator<Item = UnitId> + '_ {
        self.armies
            .iter()
            .flat_map(|a| a.members.iter().copied())
            .chain(self.rear.iter().copied())
    }

    /// Reserves a stranded economy's bank and queues its replacement
    /// Harvester as soon as the reserve is whole.
    ///
    /// `None` means ordinary play. `Some([])` means the seat is still
    /// saving (or its Foundry queue is temporarily full); callers must
    /// skip policy spending for this think. A non-empty vector is the
    /// one emergency Train command. Keeping this below the policies is
    /// deliberate: recovery is an executive safety rule, not something
    /// every scripted and learned policy must rediscover.
    pub(crate) fn harvester_recovery(
        &self,
        me: PlayerId,
        obs: &Observation,
    ) -> Option<Vec<PlayerCommand>> {
        let has_harvester = obs.my_units.iter().any(|u| u.kind == UnitKind::Harvester);
        let queued_harvester = obs
            .my_queues
            .iter()
            .flatten()
            .any(|kind| *kind == UnitKind::Harvester);
        let has_foundry = obs
            .my_buildings
            .iter()
            .any(|b| b.kind == BuildingKind::Foundry && b.built);
        if has_harvester || queued_harvester || !has_foundry {
            return None;
        }

        let mut commands = Vec::new();
        if obs.scrap >= UnitKind::Harvester.stats().cost
            && let Some((_, foundry)) = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(qi, b)| {
                    b.kind == BuildingKind::Foundry
                        && b.built
                        && obs.my_queues[*qi].len() < crate::stats::QUEUE_CAP
                })
                .min_by_key(|(_, b)| b.id)
        {
            commands.push(PlayerCommand {
                player: me,
                command: Command::Train {
                    building: foundry.id,
                    kind: UnitKind::Harvester,
                },
            });
        }
        Some(commands)
    }

    /// Applies a think's intents under the scripted baseline rules —
    /// see [`Executive::apply_with`].
    pub fn apply(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        intents: &[Intent],
    ) -> Vec<PlayerCommand> {
        self.apply_with(me, obs, intents, &LoweringRules::scripted())
    }

    /// Applies a think's intents, in order, returning the commands they
    /// lower to. Deterministic given (self, obs, intents, rules).
    pub fn apply_with(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        intents: &[Intent],
        rules: &LoweringRules,
    ) -> Vec<PlayerCommand> {
        let mut out = Vec::new();
        // Units spoken for by an earlier intent in this same think — keeps
        // a Scout's unit from being drafted by a FormArmy a line later.
        let mut claimed: Vec<UnitId> = Vec::new();
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
                    if let Some(builder) = self.free_harvester(obs, *anchor, &claimed) {
                        claimed.push(builder);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Build {
                                units: vec![builder],
                                kind: *kind,
                                anchor: *anchor,
                                queue: false,
                                defer: rules.defer_needed.is_some_and(|f| f(*kind, *anchor)),
                            },
                        });
                    }
                }
                Intent::FormArmy { staging, size } => {
                    // `size` is a target strength, not an increment: an
                    // army already staging here only drafts the shortfall.
                    let existing = self
                        .armies
                        .iter()
                        .position(|a| a.state == ArmyState::Staging && a.staging == *staging);
                    let want = existing
                        .map(|i| (*size as usize).saturating_sub(self.armies[i].members.len()))
                        .unwrap_or(*size as usize);
                    let draft = self.draft(obs, *staging, want as u32, &claimed);
                    if !draft.is_empty() {
                        claimed.extend(draft.iter().copied());
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units: draft.clone(),
                                goal: *staging,
                                queue: false,
                            },
                        });
                        if let Some(i) = existing {
                            self.armies[i].members.extend(draft);
                            self.armies[i].members.sort_unstable();
                        } else {
                            let id = ArmyId(self.next_army);
                            self.next_army += 1;
                            self.armies.push(Army {
                                id,
                                members: draft,
                                state: ArmyState::Staging,
                                staging: *staging,
                                target: None,
                                focus: None,
                            });
                        }
                    }
                }
                Intent::PushArmy { army, target } => {
                    if let Some(a) = self.armies.iter_mut().find(|a| a.id == *army)
                        && !a.members.is_empty()
                    {
                        a.state = ArmyState::Pushing;
                        a.target = Some(*target);
                        march(me, obs, a, *target, &mut out);
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
                            command: Command::AttackMove {
                                units: a.members.clone(),
                                goal: a.staging,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::AssignHarvest { unit, node } => {
                    // A unit an earlier intent claimed (a chosen builder,
                    // a scout) or one already held in an army/rear line
                    // must not be re-tasked by a chore.
                    if !claimed.contains(unit) && !self.enlisted().any(|id| id == *unit) {
                        claimed.push(*unit);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Harvest {
                                units: vec![*unit],
                                node: *node,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::Scout { unit, to } => {
                    // A plain Move replaces the unit's whole program: a
                    // Scout naming a machine an earlier intent already
                    // bought would orphan a paid site or drop a weld.
                    // Guarded on the gym path only — the scripted
                    // tiers' scouting follows its construction claims.
                    if rules.scout_honors_claims && claimed.contains(unit) {
                        continue;
                    }
                    claimed.push(*unit);
                    out.push(PlayerCommand {
                        player: me,
                        command: Command::Move {
                            units: vec![*unit],
                            goal: *to,
                            queue: false,
                        },
                    });
                }
                Intent::Repair { building } => {
                    let anchor = obs
                        .my_buildings
                        .iter()
                        .find(|b| b.id == *building)
                        .map(|b| b.anchor);
                    if let Some(anchor) = anchor
                        && let Some(welder) = self.free_harvester(obs, anchor, &claimed)
                    {
                        claimed.push(welder);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Repair {
                                units: vec![welder],
                                building: *building,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::Upgrade { building } => {
                    let anchor = obs
                        .my_buildings
                        .iter()
                        .find(|b| b.id == *building)
                        .map(|b| b.anchor);
                    if let Some(anchor) = anchor
                        && let Some(crew) = self.free_harvester(obs, anchor, &claimed)
                    {
                        claimed.push(crew);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::UpgradeBuilding {
                                units: vec![crew],
                                building: *building,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::Salvage { building } => {
                    let anchor = obs
                        .my_buildings
                        .iter()
                        .find(|b| b.id == *building)
                        .map(|b| b.anchor);
                    if let Some(anchor) = anchor
                        && let Some(stripper) = self.free_harvester(obs, anchor, &claimed)
                    {
                        claimed.push(stripper);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Salvage {
                                units: vec![stripper],
                                building: *building,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::RepairUnit { unit } => {
                    let tile = obs.my_units.iter().find(|u| u.id == *unit).map(|u| u.tile);
                    // The patient must not be drafted as its own welder:
                    // the sim strips it from the crew and would reject
                    // the emptied command.
                    let mut barred = claimed.clone();
                    barred.push(*unit);
                    if let Some(tile) = tile
                        && let Some(welder) = self.free_harvester(obs, tile, &barred)
                    {
                        // Rotate an enlisted patient out before later
                        // Push/Recall intents lower. Merely claiming it
                        // protects Scout and FormArmy, but army commands
                        // address their existing membership wholesale
                        // and would replace the weld in the same tick.
                        for army in &mut self.armies {
                            army.members.retain(|member| member != unit);
                        }
                        self.armies.retain(|army| !army.members.is_empty());
                        if !self.rear.contains(unit) {
                            self.rear.push(*unit);
                            self.rear.sort_unstable();
                        }
                        // The patient must stay put for the weld to land.
                        // Reserve it from every later non-army intent too.
                        claimed.push(*unit);
                        claimed.push(welder);
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::Stop { units: vec![*unit] },
                        });
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::RepairUnit {
                                units: vec![welder],
                                target: *unit,
                                queue: false,
                            },
                        });
                    }
                }
                Intent::RaidAir { target } => {
                    let enlisted: Vec<UnitId> = self.enlisted().collect();
                    let wings: Vec<UnitId> = obs
                        .my_units
                        .iter()
                        .filter(|u| {
                            let stats = u.kind.stats();
                            stats.domain == crate::stats::Domain::Air
                                && stats.can_target(crate::stats::Domain::Ground)
                                && u.idle
                                && !enlisted.contains(&u.id)
                                && !claimed.contains(&u.id)
                        })
                        .map(|u| u.id)
                        .collect();
                    if !wings.is_empty() {
                        claimed.extend(wings.iter().copied());
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units: wings,
                                goal: *target,
                                queue: false,
                            },
                        });
                    }
                }
            }
        }
        out
    }

    /// The frozen scripted path's per-think housekeeping. Rear-line
    /// veterans remain reserved even if an external effect heals them.
    pub fn maintain(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        rear: TilePos,
    ) -> Vec<PlayerCommand> {
        self.maintain_with_rejoin(me, obs, rear, false)
    }

    /// Per-think housekeeping for a policy that can deliberately heal
    /// units. A fully healed rear-line veteran returns to the draft pool.
    pub fn maintain_repair_capable(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        rear: TilePos,
    ) -> Vec<PlayerCommand> {
        self.maintain_with_rejoin(me, obs, rear, true)
    }

    /// Prune the dead, rotate the wounded to the rear, advance army
    /// states, and withdraw from fights that have turned. `rear` is
    /// behind the lines, not the army's rally (which may be the fight).
    fn maintain_with_rejoin(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        rear: TilePos,
        rejoin_healed: bool,
    ) -> Vec<PlayerCommand> {
        let mut out = Vec::new();
        let doctrine = self.doctrine;
        let alive = |id: UnitId| obs.my_units.iter().any(|u| u.id == id);
        self.rear.retain(|id| {
            obs.my_units
                .iter()
                .find(|u| u.id == *id)
                .is_some_and(|u| !rejoin_healed || u.hp < u.kind.stats().max_hp)
        });
        for army in &mut self.armies {
            army.members.retain(|id| alive(*id));
            if army.members.is_empty() {
                continue; // swept below
            }
            let in_contact = enemies_near(obs, &army.members, CONTACT_RADIUS);

            // Rotate the badly wounded out, but only between fights.
            // Mid-engagement a wounded machine still deals full damage,
            // and at equal speeds it cannot escape a pursuer anyway;
            // pulling it then just thins the line.
            if doctrine.pullback && !in_contact {
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
                            goal: rear,
                            queue: false,
                        },
                    });
                    self.rear.extend(pulled);
                }
            }
            if army.members.is_empty() {
                continue; // swept below
            }

            let centroid = centroid(&army.members, obs);
            match army.state {
                ArmyState::Staging => {
                    // A staged army can be attacked where it stands — the
                    // fight evaluation must not wait for a push order.
                    if in_contact {
                        army.state = ArmyState::Engaging;
                    }
                }
                ArmyState::Pushing => {
                    if in_contact {
                        army.state = ArmyState::Engaging;
                    } else if let Some(target) = army.target
                        && tiles_within(vanguard_centroid(&army.members, obs), target, 2)
                    {
                        // Arrived and nothing to fight: hold the ground
                        // taken — this rally is the staging point now.
                        army.state = ArmyState::Staging;
                        army.staging = target;
                        army.target = None;
                    }
                }
                ArmyState::Engaging => {
                    let (mine, theirs) = local_strength(obs, &army.members);
                    if theirs == 0 {
                        // Fight's over here; march on if a target remains.
                        army.state = match army.target {
                            Some(_) => ArmyState::Pushing,
                            None => ArmyState::Staging,
                        };
                        army.focus = None;
                        if let Some(target) = army.target {
                            march(me, obs, army, target, &mut out);
                        }
                    } else if mine * u64::from(WITHDRAW_MARGIN_DEN)
                        < theirs * u64::from(WITHDRAW_MARGIN_NUM)
                    {
                        // Losing decisively: leave together, fighting.
                        // Nothing here outruns its pursuers, so an
                        // oblivious Move retreat is shot in the back for
                        // free the whole way home — the attack-move falls
                        // back along the same line but answers fire.
                        army.state = ArmyState::Withdrawing;
                        army.target = None;
                        army.focus = None;
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units: army.members.clone(),
                                goal: army.staging,
                                queue: false,
                            },
                        });
                    } else if doctrine.focus_fire {
                        // Concentrate fire: everyone on the weakest gun
                        // in the fight (ties toward the centroid, then
                        // id). Candidates stay inside contact radius so
                        // the sim's see-the-victim rule holds even for
                        // an omniscient policy. One command per change
                        // of focus — auto-acquire covers the seconds in
                        // between; churning orders every think costs
                        // shots.
                        let members: Vec<&UnitObs> = obs
                            .my_units
                            .iter()
                            .filter(|u| army.members.contains(&u.id))
                            .collect();
                        let near = |t: TilePos| {
                            members
                                .iter()
                                .map(|m| m.tile.chebyshev(t))
                                .min()
                                .unwrap_or(i32::MAX)
                        };
                        let focus = obs
                            .enemy_units
                            .iter()
                            .filter(|u| {
                                near(u.tile) <= CONTACT_RADIUS && u.kind.stats().can_fight()
                            })
                            .map(|u| (u.hp, near(u.tile), u.id))
                            .min()
                            .map(|(.., id)| id);
                        if let Some(target) = focus
                            && army.focus != Some(target)
                        {
                            army.focus = Some(target);
                            out.push(PlayerCommand {
                                player: me,
                                command: Command::Attack {
                                    units: army.members.clone(),
                                    target: crate::ids::Target::Unit(target),
                                    queue: false,
                                },
                            });
                        }
                    }
                }
                ArmyState::Withdrawing => {
                    if tiles_within(centroid, army.staging, 2) {
                        army.state = ArmyState::Staging;
                    }
                }
            }
        }
        self.armies.retain(|a| !a.members.is_empty());
        out
    }

    /// The harvesters [`Self::apply`] would spend lowering `intents` —
    /// same chooser, same order, so a chore appended behind them can
    /// keep off a machine the lowering has already bought. Takes
    /// world-space intents, exactly like `apply`.
    pub(super) fn labor_claims(&self, obs: &Observation, intents: &[Intent]) -> Vec<UnitId> {
        let mut claimed: Vec<UnitId> = Vec::new();
        for intent in intents {
            // The labor intents are the ones whose worker the policy
            // never names; a new one belongs in this list too.
            let (anchor, patient) = match intent {
                Intent::Build { anchor, .. } => (Some(*anchor), None),
                Intent::Repair { building }
                | Intent::Salvage { building }
                | Intent::Upgrade { building } => (
                    obs.my_buildings
                        .iter()
                        .find(|b| b.id == *building)
                        .map(|b| b.anchor),
                    None,
                ),
                Intent::RepairUnit { unit } => (
                    obs.my_units.iter().find(|u| u.id == *unit).map(|u| u.tile),
                    Some(*unit),
                ),
                _ => (None, None),
            };
            let mut barred = claimed.clone();
            barred.extend(patient);
            if let Some(anchor) = anchor
                && let Some(unit) = self.free_harvester(obs, anchor, &barred)
            {
                claimed.push(unit);
            }
        }
        claimed
    }

    /// The nearest own harvester to `anchor` that isn't enlisted or
    /// already claimed this think, for construction. Working ones are
    /// fair game (the economy re-hires).
    fn free_harvester(
        &self,
        obs: &Observation,
        anchor: TilePos,
        claimed: &[UnitId],
    ) -> Option<UnitId> {
        let enlisted: Vec<UnitId> = self.enlisted().collect();
        obs.my_units
            .iter()
            .filter(|u| {
                u.kind == UnitKind::Harvester
                    && u.site.is_none()
                    // A walking founder is as spoken for as a builder
                    // on site: re-tasking it silently drops the
                    // promised claim. Scripted tiers never defer, so
                    // this arm is dead on their path.
                    && u.founding.is_none()
                    && !enlisted.contains(&u.id)
                    && !claimed.contains(&u.id)
            })
            .map(|u| (u.tile.manhattan(anchor), u.id))
            .min()
            .map(|(_, id)| id)
    }

    /// Drafts up to `size` un-enlisted, unclaimed fighters, nearest to
    /// the staging point first, ties to the lowest id.
    fn draft(
        &self,
        obs: &Observation,
        staging: TilePos,
        size: u32,
        claimed: &[UnitId],
    ) -> Vec<UnitId> {
        let enlisted: Vec<UnitId> = self.enlisted().collect();
        let mut candidates: Vec<(i32, UnitId)> = obs
            .my_units
            .iter()
            .filter(|u| {
                let stats = u.kind.stats();
                // Armies are ground bodies: the lifecycle's centroids,
                // standoffs, and focus picks are all ground-shaped, and
                // enlisting wings here would starve the raid channel of
                // the very units it was bought for.
                stats.can_fight()
                    && stats.domain == crate::stats::Domain::Ground
                    && u.idle
                    && !enlisted.contains(&u.id)
                    && !claimed.contains(&u.id)
            })
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

/// A long gun: ordered reach beyond its own eyes. It fires on the
/// team's sight, so it must never lead the march into what it cannot
/// see.
fn is_artillery(u: &UnitObs) -> bool {
    let stats = u.kind.stats();
    stats
        .max_range_vs(crate::stats::Domain::Ground)
        .is_some_and(|r| r > chassis::fx::Fx::from_num(stats.vision))
}

/// How far short of the push target artillery parks — inside its own
/// reach of the target, outside a defending turret's.
const ARTY_STANDOFF: i32 = 7;

/// Marching orders for a push: escorts attack-move onto the target;
/// artillery holds a standoff point pulled back along the line of
/// advance — and without an escort quorum (a third of the army) the
/// guns stay at the staging ground instead. Nobody pushes blind
/// artillery.
fn march(
    me: PlayerId,
    obs: &Observation,
    army: &Army,
    target: TilePos,
    out: &mut Vec<PlayerCommand>,
) {
    let (arty, escorts): (Vec<UnitId>, Vec<UnitId>) = army
        .members
        .iter()
        .partition(|id| obs.my_units.iter().any(|u| u.id == **id && is_artillery(u)));
    if !escorts.is_empty() {
        out.push(PlayerCommand {
            player: me,
            command: Command::AttackMove {
                units: escorts.clone(),
                goal: target,
                queue: false,
            },
        });
    }
    if arty.is_empty() {
        return;
    }
    if escorts.len() * 3 >= army.members.len() {
        let (dx, dy) = (army.staging.x - target.x, army.staging.y - target.y);
        let d = dx.abs().max(dy.abs());
        let stand = if d == 0 {
            target
        } else {
            let pull = ARTY_STANDOFF.min(d);
            TilePos::new(target.x + dx * pull / d, target.y + dy * pull / d)
        };
        out.push(PlayerCommand {
            player: me,
            command: Command::AttackMove {
                units: arty,
                goal: stand,
                queue: false,
            },
        });
    } else {
        out.push(PlayerCommand {
            player: me,
            command: Command::Move {
                units: arty,
                goal: army.staging,
                queue: false,
            },
        });
    }
}

/// The escorts' mean tile — artillery hanging back must not drag the
/// army's sense of "arrived" backward with it. Falls back to the whole
/// body for a pure-artillery force.
fn vanguard_centroid(members: &[UnitId], obs: &Observation) -> TilePos {
    let escorts: Vec<UnitId> = members
        .iter()
        .copied()
        .filter(|id| obs.my_units.iter().any(|u| u.id == *id && !is_artillery(u)))
        .collect();
    if escorts.is_empty() {
        centroid(members, obs)
    } else {
        centroid(&escorts, obs)
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

/// Whether an army counts as fighting: a third of it (at least one
/// member) has an armed enemy inside `radius`. A lone straggler brushing
/// past an enemy is not the army's fight — quorum keeps the state
/// machine from being yanked around by grazing contact.
fn enemies_near(obs: &Observation, members: &[UnitId], radius: i32) -> bool {
    let touched = obs
        .my_units
        .iter()
        .filter(|u| members.contains(&u.id))
        .filter(|m| {
            obs.enemy_units
                .iter()
                .any(|e| mutually_relevant(m, e) && m.tile.chebyshev(e.tile) <= radius)
        })
        .count();
    touched > 0 && touched * 3 >= members.len()
}

/// hp-weighted dps a unit can bring against the given movement domain —
/// the shared coin every fight estimate is priced in. Damage per 100
/// ticks keeps it in integers (cooldowns divide 100 unevenly — close
/// enough for margin calls that carry hysteresis).
pub fn strength_vs(u: &UnitObs, domain: crate::stats::Domain) -> u64 {
    let stats = u.kind.stats();
    let dps100: u64 = stats
        .weapons
        .iter()
        .filter(|w| w.targets.covers(domain))
        .map(|w| u64::from(w.damage) * 100 / u64::from(w.cooldown_ticks))
        .sum();
    u64::from(u.hp) * dps100
}

/// Ground-battle strength — the legacy coin the gym's v2 features are
/// priced in. Weapons that can only look up contribute nothing, so an
/// anti-air escort never inflates a push estimate.
pub fn unit_strength(u: &UnitObs) -> u64 {
    strength_vs(u, crate::stats::Domain::Ground)
}

/// Whether these two would have anything to say to each other in a
/// fight: the enemy is armed and coverable, or it can cover us. Unarmed
/// enemies never constitute a fight (chasing a fleeing harvester is not
/// an engagement), and a flak platform staring at ground infantry is
/// scenery to both sides.
fn mutually_relevant(member: &UnitObs, enemy: &UnitObs) -> bool {
    let m = member.kind.stats();
    let e = enemy.kind.stats();
    (e.can_fight() && m.can_target(e.domain)) || e.can_target(m.domain)
}

/// Same coin for a standing building (turrets; zero for the unarmed).
pub fn building_strength(b: &super::observation::BuildingObs) -> u64 {
    if !b.built {
        return 0;
    }
    let dps100: u64 = b
        .kind
        .base_stats()
        .weapons
        .iter()
        .filter(|w| w.targets.ground)
        .map(|w| u64::from(w.damage) * 100 / u64::from(w.cooldown_ticks))
        .sum();
    u64::from(b.hp) * dps100
}

/// Strength sums for an army's fight: every member counts (the army is
/// the fighting body wherever its parts stand), and the opposition is
/// every enemy within the engagement radius of a member that is itself
/// in contact. Anchoring on fighting members instead of a centroid keeps
/// the estimate stable when the line bends — a mean position can land in
/// empty ground and blind every radius test around it — while stragglers
/// don't sweep distant enemies into the count.
fn local_strength(obs: &Observation, members: &[UnitId]) -> (u64, u64) {
    use crate::stats::Domain;
    let mine_units: Vec<&UnitObs> = obs
        .my_units
        .iter()
        .filter(|u| members.contains(&u.id))
        .collect();
    let engaged: Vec<TilePos> = mine_units
        .iter()
        .filter(|m| {
            obs.enemy_units
                .iter()
                .any(|e| mutually_relevant(m, e) && m.tile.chebyshev(e.tile) <= CONTACT_RADIUS)
        })
        .map(|m| m.tile)
        .collect();
    let opposition: Vec<&UnitObs> = obs
        .enemy_units
        .iter()
        .filter(|e| engaged.iter().any(|m| m.chebyshev(e.tile) <= ENGAGE_RADIUS))
        .collect();
    // Matched pairs: each side is worth what it can actually apply to
    // the domains the other side fields. An interceptor over a pure
    // ground brawl contributes nothing to either column.
    let domains_of = |units: &[&UnitObs]| {
        let ground = units
            .iter()
            .any(|u| u.kind.stats().domain == Domain::Ground);
        let air = units.iter().any(|u| u.kind.stats().domain == Domain::Air);
        (ground, air)
    };
    let (their_ground, their_air) = domains_of(&opposition);
    let (my_ground, my_air) = domains_of(&mine_units);
    let applicable = |u: &UnitObs, ground: bool, air: bool| -> u64 {
        let g = if ground {
            strength_vs(u, Domain::Ground)
        } else {
            0
        };
        let a = if air { strength_vs(u, Domain::Air) } else { 0 };
        g.max(a)
    };
    let mine: u64 = mine_units
        .iter()
        .map(|u| applicable(u, their_ground, their_air))
        .sum();
    let theirs: u64 = opposition
        .iter()
        .map(|u| applicable(u, my_ground, my_air))
        .sum();
    (mine, theirs)
}

fn tiles_within(a: TilePos, b: TilePos, radius: i32) -> bool {
    a.chebyshev(b) <= radius
}
