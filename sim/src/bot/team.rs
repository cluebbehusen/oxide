//! Bounded relief for a teammate's pressured Foundry.
//!
//! A relief operation owns an exact ground group from commitment through its
//! return home. It reacts only to hostiles in current team vision, protects a
//! home-defense floor, and breaks contact once the visible emergency ends or
//! the operation spends its loss, health, or time budget.

use super::difficulty::{DifficultyTuning, strategic_admission_tick};
use super::executive::Intent;
use super::observation::{BuildingObs, Observation, UnitObs};
use super::profile::ResolvedProfile;
use super::routing::{RouteProjection, first_reachable_group_where};
use super::strategy::StrategicDecision;
use super::utility::combat_core_status;
use crate::ids::{BuildingId, PlayerId, Target, UnitId};
use crate::stats::{BuildingKind, Domain};
use chassis::Tick;
use chassis::grid::TilePos;
use core::cmp::Reverse;

const PRESSURE_RADIUS: i32 = 8;
const PRESSURE_CREDIBILITY: Tick = crate::TICKS_PER_SECOND as Tick;
const ARRIVAL_RADIUS: i32 = 4;
const RETURN_RADIUS: i32 = 3;
const MIN_RELIEF_GROUP: usize = 2;
const WITHDRAWAL_TIMEOUT: Tick = 400;

/// The active phase of an allied-base relief operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamReliefPhase {
    /// Marching the exact relief group toward the allied Foundry.
    Deploying,
    /// Holding the threatened ground while hostile pressure remains visible.
    Holding,
    /// Breaking contact and returning surviving members home.
    Withdrawing,
}

/// Why a relief operation broke contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamReliefExitReason {
    /// No hostile ground pressure remains visible around the allied Foundry.
    PressureEnded,
    /// The relief group lost more machines than its commitment budget allows.
    LossBudget,
    /// Surviving machines spent their bounded hull budget.
    HealthBudget,
    /// The allied Foundry no longer stands.
    FoundryLost,
    /// The bounded operation ran out of time.
    Timeout,
    /// Explored terrain proved that the exact group cannot reach its order.
    Unreachable,
}

/// The persistent order currently owned by a relief operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamReliefDispatch {
    /// Attack-move toward the allied base.
    Outbound(TilePos),
    /// Focus a currently visible threat near the allied base.
    Threat(Target),
    /// Return home without seeking another fight.
    Return(TilePos),
}

/// Inspectable persistent state of one team-relief operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamReliefOperation {
    /// Teammate whose Foundry requested relief.
    pub ally: PlayerId,
    /// Stable allied Foundry identity.
    pub foundry: BuildingId,
    /// Stable Foundry anchor used as the relief and return-from-contact point.
    pub anchor: TilePos,
    /// Exact surviving operation members, sorted by id.
    pub members: Vec<UnitId>,
    /// Exact fighters left home at commitment, sorted by id. They remain
    /// available to ordinary home-defense policy after the relief launches.
    pub home_defenders: Vec<UnitId>,
    /// Group size at commitment, used for the loss budget.
    pub committed_size: usize,
    /// Total full hull of the committed group, used for the health budget.
    pub committed_max_hp: u32,
    /// Current operation phase.
    pub phase: TeamReliefPhase,
    /// Tick at which the group committed.
    pub started_at: Tick,
    /// Tick at which the current phase began.
    pub phase_started_at: Tick,
    /// Set only once the group begins withdrawing.
    pub exit_reason: Option<TeamReliefExitReason>,
    /// Last operation order sent to the exact group.
    pub dispatch: Option<TeamReliefDispatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PressureWatch {
    foundry: BuildingId,
    first_seen_at: Tick,
    relief: TeamReliefOperation,
}

/// Controller-local owner of team-pressure evidence, relief, and cooldown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamReliefPlanner {
    active: Option<TeamReliefOperation>,
    watch: Option<PressureWatch>,
    cooldown_until: Tick,
}

#[derive(Clone, Copy)]
pub(super) struct TeamReliefAdmission<'a> {
    pub(super) additionally_reserved: &'a [UnitId],
    pub(super) allow_new_operation: bool,
    pub(super) core_reservations: &'a [UnitId],
    pub(super) minimum_core_equivalents: u64,
}

#[derive(Clone, Copy)]
struct ReliefContext<'a> {
    profile: &'a ResolvedProfile,
    tuning: DifficultyTuning,
    obs: &'a Observation,
    home: TilePos,
    enlisted: &'a [UnitId],
    additionally_reserved: &'a [UnitId],
    core_reservations: &'a [UnitId],
    minimum_core_equivalents: u64,
}

impl TeamReliefPlanner {
    /// Creates an idle team-relief planner.
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently active relief operation, if any.
    pub fn operation(&self) -> Option<&TeamReliefOperation> {
        self.active.as_ref()
    }

    /// Exact units owned by an active relief or its credibility watch.
    #[cfg(test)]
    pub(super) fn reservations(&self) -> Vec<UnitId> {
        self.active.as_ref().map_or_else(
            || {
                self.watch
                    .as_ref()
                    .map_or_else(Vec::new, |watch| pending_reservations(&watch.relief))
            },
            |operation| operation.members.clone(),
        )
    }

    /// Exact fighters that would be absent from the home combat core if the
    /// current relief assignment proceeds. Pending home defenders remain
    /// reserved from other planners but still count as the screen they are
    /// explicitly staying behind to provide.
    pub(super) fn core_reservations(&self) -> Vec<UnitId> {
        self.active.as_ref().map_or_else(
            || {
                self.watch
                    .as_ref()
                    .map_or_else(Vec::new, |watch| watch.relief.members.clone())
            },
            |operation| operation.members.clone(),
        )
    }

    /// Earliest tick at which another relief operation may begin.
    pub fn cooldown_until(&self) -> Tick {
        self.cooldown_until
    }

    /// Advances team conduct from current fog-honest knowledge.
    pub fn think(
        &mut self,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &Observation,
        home: TilePos,
        enlisted: &[UnitId],
        additionally_reserved: &[UnitId],
    ) -> StrategicDecision {
        self.think_with_admission(
            profile,
            tuning,
            obs,
            home,
            enlisted,
            TeamReliefAdmission {
                additionally_reserved,
                allow_new_operation: true,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        )
    }

    pub(super) fn think_with_admission(
        &mut self,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &Observation,
        home: TilePos,
        enlisted: &[UnitId],
        admission: TeamReliefAdmission<'_>,
    ) -> StrategicDecision {
        let TeamReliefAdmission {
            additionally_reserved,
            allow_new_operation,
            core_reservations,
            minimum_core_equivalents,
        } = admission;
        let mut routes = RouteProjection::new(obs, Domain::Ground);
        let context = ReliefContext {
            profile,
            tuning,
            obs,
            home,
            enlisted,
            additionally_reserved,
            core_reservations,
            minimum_core_equivalents,
        };
        if self.active.is_none() {
            self.observe_pressure(&context, &mut routes, allow_new_operation);
        }
        let Some(mut relief) = self.active.take() else {
            return StrategicDecision {
                reservations: self
                    .watch
                    .as_ref()
                    .map_or_else(Vec::new, |watch| pending_reservations(&watch.relief)),
                ..StrategicDecision::default()
            };
        };

        relief.members.retain(|id| own_unit(obs, *id).is_some());
        relief
            .home_defenders
            .retain(|id| own_unit(obs, *id).is_some());
        if relief.phase != TeamReliefPhase::Withdrawing {
            if !allied_foundry_stands(obs, &relief) {
                withdraw(&mut relief, TeamReliefExitReason::FoundryLost, obs.tick);
            } else if loss_budget_spent(&relief) {
                withdraw(&mut relief, TeamReliefExitReason::LossBudget, obs.tick);
            } else if health_budget_spent(obs, &relief) {
                withdraw(&mut relief, TeamReliefExitReason::HealthBudget, obs.tick);
            } else if obs.tick.saturating_sub(relief.started_at) >= operation_timeout(profile) {
                withdraw(&mut relief, TeamReliefExitReason::Timeout, obs.tick);
            } else if visible_pressure(obs, relief.anchor).is_empty() {
                withdraw(&mut relief, TeamReliefExitReason::PressureEnded, obs.tick);
            } else if !routes.group_reaches_command_goal(&relief.members, relief.anchor) {
                withdraw(&mut relief, TeamReliefExitReason::Unreachable, obs.tick);
            }
        }

        let mut decision = StrategicDecision::default();
        let already_home = relief.members.iter().all(|id| {
            own_unit(obs, *id).is_some_and(|unit| unit.tile.chebyshev(home) <= RETURN_RADIUS)
        });
        let return_route = relief.phase != TeamReliefPhase::Withdrawing
            || relief.members.is_empty()
            || already_home
            || routes.group_reaches_command_goal(&relief.members, home);
        match relief.phase {
            TeamReliefPhase::Deploying => {
                let dispatch = TeamReliefDispatch::Outbound(relief.anchor);
                if relief.dispatch != Some(dispatch) {
                    decision.intents.push(Intent::AttackMoveUnits {
                        units: relief.members.clone(),
                        goal: relief.anchor,
                    });
                    relief.dispatch = Some(dispatch);
                }
                if relief.phase == TeamReliefPhase::Deploying
                    && relief.members.iter().any(|id| {
                        own_unit(obs, *id).is_some_and(|unit| {
                            unit.tile.chebyshev(relief.anchor) <= ARRIVAL_RADIUS
                        })
                    })
                {
                    relief.phase = TeamReliefPhase::Holding;
                    relief.phase_started_at = obs.tick;
                }
            }
            TeamReliefPhase::Holding => {
                let threat = visible_pressure(obs, relief.anchor)
                    .into_iter()
                    .min_by_key(|unit| {
                        (
                            relief
                                .members
                                .iter()
                                .filter_map(|id| own_unit(obs, *id))
                                .map(|member| member.tile.chebyshev(unit.tile))
                                .min()
                                .unwrap_or(i32::MAX),
                            unit.id,
                        )
                    })
                    .map(|unit| Target::Unit(unit.id));
                if let Some(target) = threat {
                    let dispatch = TeamReliefDispatch::Threat(target);
                    if relief.dispatch != Some(dispatch) {
                        decision.intents.push(Intent::AttackUnits {
                            units: relief.members.clone(),
                            target,
                        });
                        relief.dispatch = Some(dispatch);
                    }
                }
            }
            TeamReliefPhase::Withdrawing => {
                if !relief.members.is_empty() && !already_home && return_route {
                    let dispatch = TeamReliefDispatch::Return(home);
                    if relief.dispatch != Some(dispatch) {
                        decision.intents.push(Intent::MoveUnits {
                            units: relief.members.clone(),
                            goal: home,
                        });
                        relief.dispatch = Some(dispatch);
                    }
                }
            }
        }
        decision.reservations = relief.members.clone();

        let returned = relief.phase == TeamReliefPhase::Withdrawing
            && (relief.members.is_empty()
                || already_home
                || !return_route
                || obs.tick.saturating_sub(relief.phase_started_at) >= WITHDRAWAL_TIMEOUT);
        if returned {
            self.cooldown_until = obs.tick.saturating_add(cooldown(profile, tuning));
        } else {
            self.active = Some(relief);
        }
        decision
    }

    fn observe_pressure(
        &mut self,
        context: &ReliefContext<'_>,
        routes: &mut RouteProjection<'_>,
        allow_new_operation: bool,
    ) {
        let ReliefContext {
            profile,
            tuning,
            obs,
            ..
        } = *context;
        if obs.tick < self.cooldown_until || !eligible(profile) {
            self.watch = None;
            return;
        }

        if self.watch.is_none() {
            if !allow_new_operation || !strategic_admission_tick(obs.tick) {
                return;
            }
            let Some(relief) = candidate_relief(context, routes) else {
                return;
            };
            self.watch = Some(PressureWatch {
                foundry: relief.foundry,
                first_seen_at: obs.tick,
                relief,
            });
        }

        let still_pressured = self.watch.as_ref().is_some_and(|watch| {
            obs.ally_buildings.iter().any(|building| {
                building.id == watch.foundry
                    && building.built
                    && building.hp > 0
                    && building.seen
                    && !visible_pressure(obs, building.anchor).is_empty()
            })
        });
        if !still_pressured {
            self.watch = None;
            return;
        }

        let refresh = self
            .watch
            .as_ref()
            .is_some_and(|watch| !pending_assignment_is_available(context, &watch.relief));
        if refresh {
            if !allow_new_operation {
                self.watch = None;
                return;
            }
            let watch = self
                .watch
                .take()
                .expect("the pending relief was just inspected");
            let replacement = obs
                .ally_buildings
                .iter()
                .find(|building| building.id == watch.foundry)
                .and_then(|foundry| begin(context, foundry, routes));
            let Some(mut relief) = replacement else {
                return;
            };
            relief.started_at = watch.relief.started_at;
            relief.phase_started_at = watch.relief.phase_started_at;
            self.watch = Some(PressureWatch {
                foundry: watch.foundry,
                first_seen_at: watch.first_seen_at,
                relief,
            });
        }

        let ready = self.watch.as_ref().is_some_and(|watch| {
            obs.tick.saturating_sub(watch.first_seen_at) >= pressure_response_delay(tuning)
        });
        if ready && allow_new_operation {
            self.active = self.watch.take().map(|watch| watch.relief);
        }
    }
}

fn begin(
    context: &ReliefContext<'_>,
    foundry: &BuildingObs,
    routes: &mut RouteProjection<'_>,
) -> Option<TeamReliefOperation> {
    let ReliefContext {
        profile,
        tuning: _,
        obs,
        home,
        enlisted,
        additionally_reserved,
        core_reservations,
        minimum_core_equivalents,
    } = *context;
    let mut available: Vec<_> = obs
        .my_units
        .iter()
        .filter(|unit| {
            unit.idle
                && is_relief_fighter(unit)
                && healthy_enough_for_relief(unit)
                && !enlisted.contains(&unit.id)
                && !additionally_reserved.contains(&unit.id)
        })
        .collect();
    available.sort_by_key(|unit| (unit.tile.chebyshev(home), unit.id));
    let home_floor = home_defense_floor(profile);
    if available.len() < home_floor + MIN_RELIEF_GROUP {
        return None;
    }
    let mut sendable = available.split_off(home_floor);
    let mut home_defenders: Vec<_> = available.iter().map(|unit| unit.id).collect();
    home_defenders.sort_unstable();
    sendable.sort_by_key(|unit| {
        (
            unit.tile.chebyshev(foundry.anchor),
            relief_role_priority(unit),
            unit.id,
        )
    });
    let desired = desired_group_size(profile).min(sendable.len());
    let candidates: Vec<_> = sendable.iter().map(|unit| unit.id).collect();
    let members = (MIN_RELIEF_GROUP..=desired).rev().find_map(|size| {
        first_reachable_group_where(routes, &candidates, size, foundry.anchor, |members| {
            let mut projected_reservations = core_reservations.to_vec();
            projected_reservations.extend_from_slice(members);
            projected_reservations.sort_unstable();
            projected_reservations.dedup();
            combat_core_status(obs, &projected_reservations, &[], minimum_core_equivalents).ready
        })
    })?;
    let committed_max_hp = members.iter().fold(0_u32, |total, id| {
        total.saturating_add(
            own_unit(obs, *id)
                .map(|unit| unit.kind.stats().max_hp)
                .unwrap_or(0),
        )
    });
    let committed_size = members.len();
    Some(TeamReliefOperation {
        ally: foundry.player,
        foundry: foundry.id,
        anchor: foundry.anchor,
        members,
        home_defenders,
        committed_size,
        committed_max_hp,
        phase: TeamReliefPhase::Deploying,
        started_at: obs.tick,
        phase_started_at: obs.tick,
        exit_reason: None,
        dispatch: None,
    })
}

fn eligible(profile: &ResolvedProfile) -> bool {
    profile.traits.support >= 58 || profile.traits.fortification >= 58
}

fn candidate_relief(
    context: &ReliefContext<'_>,
    routes: &mut RouteProjection<'_>,
) -> Option<TeamReliefOperation> {
    let obs = context.obs;
    let home = context.home;
    let mut candidates: Vec<_> = obs
        .ally_buildings
        .iter()
        .filter(|building| {
            building.kind == BuildingKind::Foundry
                && building.built
                && building.hp > 0
                && building.seen
                && !visible_pressure(obs, building.anchor).is_empty()
        })
        .collect();
    candidates.sort_by_key(|building| {
        let pressure = visible_pressure(obs, building.anchor);
        let strength = pressure.iter().fold(0_u64, |total, unit| {
            total.saturating_add(visible_strength(unit))
        });
        (
            Reverse(strength),
            Reverse(pressure.len()),
            building.anchor.chebyshev(home),
            building.anchor.y,
            building.anchor.x,
            building.id,
        )
    });
    candidates
        .into_iter()
        .find_map(|foundry| begin(context, foundry, routes))
}

fn visible_pressure(obs: &Observation, anchor: TilePos) -> Vec<&UnitObs> {
    let mut pressure: Vec<_> = obs
        .enemy_units
        .iter()
        .filter(|unit| {
            obs.visible(unit.tile)
                && unit.tile.chebyshev(anchor) <= PRESSURE_RADIUS
                && unit.body_domain() == Domain::Ground
                && unit.kind.stats().can_fight()
                && (unit.kind.stats().can_target(Domain::Ground) || unit.kind.stats().demolition)
        })
        .collect();
    pressure.sort_by_key(|unit| unit.id);
    pressure
}

fn visible_strength(unit: &UnitObs) -> u64 {
    let stats = unit.kind.stats();
    u64::from(stats.cost.max(1)).saturating_mul(u64::from(unit.hp)) / u64::from(stats.max_hp.max(1))
}

fn allied_foundry_stands(obs: &Observation, relief: &TeamReliefOperation) -> bool {
    obs.ally_buildings.iter().any(|building| {
        building.id == relief.foundry
            && building.player == relief.ally
            && building.kind == BuildingKind::Foundry
            && building.anchor == relief.anchor
            && building.built
            && building.hp > 0
    })
}

fn own_unit(obs: &Observation, id: UnitId) -> Option<&UnitObs> {
    obs.my_units.iter().find(|unit| unit.id == id)
}

fn is_relief_fighter(unit: &UnitObs) -> bool {
    let stats = unit.kind.stats();
    stats.domain == Domain::Ground
        && stats.can_fight()
        && stats.can_target(Domain::Ground)
        && !stats.demolition
}

fn healthy_enough_for_relief(unit: &UnitObs) -> bool {
    unit.hp.saturating_mul(100) >= unit.kind.stats().max_hp.saturating_mul(60)
}

fn relief_role_priority(unit: &UnitObs) -> u8 {
    match unit.kind {
        crate::stats::UnitKind::Warden
        | crate::stats::UnitKind::Breaker
        | crate::stats::UnitKind::Sentinel => 0,
        crate::stats::UnitKind::Scuttler => 1,
        crate::stats::UnitKind::Lancer | crate::stats::UnitKind::Bombard => 2,
        crate::stats::UnitKind::Avalanche => 3,
        _ => 4,
    }
}

fn desired_group_size(profile: &ResolvedProfile) -> usize {
    let orientation = u16::from(profile.traits.support) + u16::from(profile.traits.fortification);
    if orientation >= 155 {
        4
    } else if orientation >= 125 {
        3
    } else {
        2
    }
}

fn home_defense_floor(profile: &ResolvedProfile) -> usize {
    if profile.traits.fortification >= 75 {
        3
    } else {
        2
    }
}

fn pressure_response_delay(tuning: DifficultyTuning) -> Tick {
    PRESSURE_CREDIBILITY.saturating_add(tuning.reaction_delay)
}

fn pending_reservations(relief: &TeamReliefOperation) -> Vec<UnitId> {
    let mut reservations = relief.members.clone();
    reservations.extend_from_slice(&relief.home_defenders);
    reservations.sort_unstable();
    reservations.dedup();
    reservations
}

fn pending_assignment_is_available(
    context: &ReliefContext<'_>,
    relief: &TeamReliefOperation,
) -> bool {
    let available = |id: &UnitId| {
        own_unit(context.obs, *id).is_some_and(|unit| {
            unit.idle
                && is_relief_fighter(unit)
                && healthy_enough_for_relief(unit)
                && !context.enlisted.contains(id)
                && !context.additionally_reserved.contains(id)
        })
    };
    relief.members.iter().all(available) && relief.home_defenders.iter().all(available)
}

fn loss_budget_spent(relief: &TeamReliefOperation) -> bool {
    relief.members.len().saturating_mul(3) < relief.committed_size.saturating_mul(2)
}

fn health_budget_spent(obs: &Observation, relief: &TeamReliefOperation) -> bool {
    let current_hp = relief.members.iter().fold(0_u32, |total, id| {
        total.saturating_add(own_unit(obs, *id).map_or(0, |unit| unit.hp))
    });
    let a_member_is_critical = relief.members.iter().any(|id| {
        own_unit(obs, *id).is_some_and(|unit| {
            unit.hp.saturating_mul(100) < unit.kind.stats().max_hp.saturating_mul(25)
        })
    });
    a_member_is_critical
        || current_hp.saturating_mul(100) < relief.committed_max_hp.saturating_mul(55)
}

fn withdraw(relief: &mut TeamReliefOperation, reason: TeamReliefExitReason, now: Tick) {
    relief.phase = TeamReliefPhase::Withdrawing;
    relief.phase_started_at = now;
    relief.exit_reason = Some(reason);
}

fn operation_timeout(profile: &ResolvedProfile) -> Tick {
    700 + u64::from(profile.traits.support.max(profile.traits.fortification)) * 3
}

fn cooldown(profile: &ResolvedProfile, tuning: DifficultyTuning) -> Tick {
    500 + u64::from(100_u8.saturating_sub(profile.traits.support.max(profile.traits.fortification)))
        * 4
        + tuning.commitment_hesitation
}

#[cfg(test)]
mod tests {
    use super::super::observation::OBSERVATION_VERSION;
    use super::super::profile::{PersonalityTraits, Specialty};
    use super::*;
    use crate::ids::BuildingId;
    use crate::scenario::{BotConfig, BotDifficulty, BotStance, PlayerSpec, Scenario, UnitSpec};
    use crate::state::Faction;
    use crate::stats::UnitKind;

    const HOME: TilePos = TilePos::new(3, 10);
    const ALLY_BASE: TilePos = TilePos::new(24, 10);

    fn profile() -> ResolvedProfile {
        ResolvedProfile {
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Balanced,
            personality_seed: 11,
            primary: Specialty::Support,
            secondary: Specialty::Fortification,
            traits: PersonalityTraits {
                air: 35,
                siege: 35,
                support: 70,
                fortification: 65,
                greed: 45,
                guile: 50,
            },
        }
    }

    fn resolved_profile_where(
        stance: BotStance,
        description: &str,
        predicate: impl Fn(&ResolvedProfile) -> bool,
    ) -> ResolvedProfile {
        (0..10_000)
            .map(|seed| {
                crate::scenario::BotConfig::scripted(BotDifficulty::Prime, stance, seed)
                    .resolve_profile()
            })
            .find(predicate)
            .unwrap_or_else(|| panic!("no {stance:?} profile matched {description}"))
    }

    fn tuning() -> DifficultyTuning {
        DifficultyTuning::for_level(BotDifficulty::Prime)
    }

    fn observation(tick: Tick) -> Observation {
        let tick = super::super::difficulty::strategic_admission_at_or_after(tick);
        let mut visible = vec![false; 40 * 24];
        for y in 0..24 {
            for x in 16..34 {
                visible[y * 40 + x] = true;
            }
        }
        Observation {
            version: OBSERVATION_VERSION,
            tick,
            me: PlayerId(0),
            scrap: 0,
            map_width: 40,
            map_height: 24,
            my_units: Vec::new(),
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: vec![building(20, PlayerId(1), ALLY_BASE)],
            enemy_units: vec![unit(
                90,
                PlayerId(2),
                UnitKind::Sentinel,
                TilePos::new(27, 10),
                false,
            )],
            enemy_buildings: Vec::new(),
            visible,
            explored: vec![true; 40 * 24],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        }
    }

    fn building(id: u32, player: PlayerId, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player,
            kind: BuildingKind::Foundry,
            anchor,
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn unit(id: u32, player: PlayerId, kind: UnitKind, tile: TilePos, idle: bool) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player,
            kind,
            tile,
            hp: kind.stats().max_hp,
            idle,
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

    fn add_fighters(obs: &mut Observation, specs: &[(u32, TilePos)]) {
        obs.my_units.extend(
            specs
                .iter()
                .map(|(id, tile)| unit(*id, PlayerId(0), UnitKind::Sentinel, *tile, true)),
        );
    }

    fn start_relief(
        planner: &mut TeamReliefPlanner,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &mut Observation,
        enlisted: &[UnitId],
        additionally_reserved: &[UnitId],
    ) -> StrategicDecision {
        let pending = planner.think(profile, tuning, obs, HOME, enlisted, additionally_reserved);
        assert!(pending.intents.is_empty());
        assert!(!pending.reservations.is_empty());
        assert!(planner.operation().is_none());
        obs.tick = obs.tick.saturating_add(pressure_response_delay(tuning));
        planner.think(profile, tuning, obs, HOME, enlisted, additionally_reserved)
    }

    fn active_three_member_relief() -> (Observation, TeamReliefPlanner, Vec<UnitId>) {
        let mut obs = observation(100);
        obs.my_units.extend([
            unit(1, PlayerId(0), UnitKind::Sentinel, TilePos::new(3, 9), true),
            unit(
                2,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(3, 11),
                true,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Scuttler,
                TilePos::new(20, 9),
                true,
            ),
            unit(
                4,
                PlayerId(0),
                UnitKind::Scuttler,
                TilePos::new(20, 11),
                true,
            ),
            unit(5, PlayerId(0), UnitKind::Warden, TilePos::new(21, 10), true),
        ]);
        let mut planner = TeamReliefPlanner::new();
        start_relief(&mut planner, &profile(), tuning(), &mut obs, &[], &[]);
        let members = planner
            .operation()
            .expect("the relief remains active")
            .members
            .clone();
        assert_eq!(members.len(), 3, "the budget fixture commits three units");
        (obs, planner, members)
    }

    #[test]
    fn closed_admission_blocks_new_relief_but_an_active_relief_withdraws() {
        let mut eligible = observation(100);
        add_fighters(
            &mut eligible,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
            ],
        );
        let identity = profile();
        let tuning = tuning();
        let mut blocked = TeamReliefPlanner::new();

        assert_eq!(
            blocked.think_with_admission(
                &identity,
                tuning,
                &eligible,
                HOME,
                &[],
                TeamReliefAdmission {
                    additionally_reserved: &[],
                    allow_new_operation: false,
                    core_reservations: &[],
                    minimum_core_equivalents: 0,
                },
            ),
            StrategicDecision::default()
        );
        assert!(blocked.reservations().is_empty());
        assert!(blocked.operation().is_none());

        let mut watched = TeamReliefPlanner::new();
        let pending = watched.think_with_admission(
            &identity,
            tuning,
            &eligible,
            HOME,
            &[],
            TeamReliefAdmission {
                additionally_reserved: &[],
                allow_new_operation: true,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        );
        assert!(!pending.reservations.is_empty());
        eligible.tick += pressure_response_delay(tuning);
        let held = watched.think_with_admission(
            &identity,
            tuning,
            &eligible,
            HOME,
            &[],
            TeamReliefAdmission {
                additionally_reserved: &[],
                allow_new_operation: false,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        );
        assert_eq!(held.reservations, pending.reservations);
        assert!(held.intents.is_empty());
        assert!(watched.operation().is_none());

        let (mut active_obs, mut active, members) = active_three_member_relief();
        active_obs.tick += 1;
        active_obs.enemy_units.clear();

        let continued = active.think_with_admission(
            &identity,
            tuning,
            &active_obs,
            HOME,
            &[],
            TeamReliefAdmission {
                additionally_reserved: &[],
                allow_new_operation: false,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        );

        assert_eq!(continued.reservations, members);
        assert_eq!(
            continued.intents,
            [Intent::MoveUnits {
                units: members,
                goal: HOME,
            }]
        );
        assert_eq!(
            active.operation().map(|operation| operation.phase),
            Some(TeamReliefPhase::Withdrawing)
        );
    }

    #[test]
    fn protected_prime_core_bounds_the_largest_admissible_relief_group() {
        let decision_for = |fighters| {
            let mut obs = observation(100);
            let specs: Vec<_> = (0..fighters)
                .map(|index| {
                    (
                        1 + index,
                        TilePos::new(
                            3 + i32::try_from(index % 4).unwrap(),
                            12 + i32::try_from(index / 4).unwrap(),
                        ),
                    )
                })
                .collect();
            add_fighters(&mut obs, &specs);
            let mut planner = TeamReliefPlanner::new();
            let decision = planner.think_with_admission(
                &profile(),
                tuning(),
                &obs,
                HOME,
                &[],
                TeamReliefAdmission {
                    additionally_reserved: &[],
                    allow_new_operation: true,
                    core_reservations: &[],
                    minimum_core_equivalents: 8,
                },
            );
            (obs, planner, decision)
        };

        let (_, exact, exact_decision) = decision_for(8);
        assert_eq!(exact_decision, StrategicDecision::default());
        assert!(exact.core_reservations().is_empty());

        for (fighters, expected_members) in [(10, 2), (11, 3)] {
            let (obs, planner, decision) = decision_for(fighters);
            let members = planner.core_reservations();
            assert_eq!(members.len(), expected_members, "{fighters} fighters");
            assert!(!decision.reservations.is_empty(), "{fighters} fighters");
            assert!(
                combat_core_status(&obs, &members, &[], 8).ready,
                "the admitted {fighters}-fighter relief must leave Prime's exact core projected"
            );
        }
    }

    #[test]
    fn relief_skips_a_preferred_core_draining_group_for_a_same_size_alternative() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(19, 8)),
                (6, TilePos::new(19, 12)),
                (7, TilePos::new(18, 9)),
                (8, TilePos::new(18, 11)),
            ],
        );
        obs.my_units.extend([
            unit(
                20,
                PlayerId(0),
                UnitKind::Scuttler,
                TilePos::new(17, 8),
                true,
            ),
            unit(
                21,
                PlayerId(0),
                UnitKind::Scuttler,
                TilePos::new(17, 10),
                true,
            ),
            unit(
                22,
                PlayerId(0),
                UnitKind::Scuttler,
                TilePos::new(17, 12),
                true,
            ),
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);

        let preferred = [UnitId(3), UnitId(4), UnitId(5)];
        let mut routes = RouteProjection::new(&obs, Domain::Ground);
        assert!(routes.group_reaches_command_goal(&preferred, ALLY_BASE));
        assert!(
            !combat_core_status(&obs, &preferred, &[], 8).ready,
            "the route-safe preferred group would consume Prime's protected core"
        );

        let mut planner = TeamReliefPlanner::new();
        let decision = planner.think_with_admission(
            &profile(),
            tuning(),
            &obs,
            HOME,
            &[],
            TeamReliefAdmission {
                additionally_reserved: &[],
                allow_new_operation: true,
                core_reservations: &[],
                minimum_core_equivalents: 8,
            },
        );

        assert_eq!(
            planner.core_reservations(),
            [UnitId(20), UnitId(21), UnitId(22)]
        );
        assert_eq!(
            decision.reservations,
            [UnitId(1), UnitId(2), UnitId(20), UnitId(21), UnitId(22)],
            "the credibility watch keeps its ordinary home screen and the same-size relief group"
        );
        assert!(combat_core_status(&obs, &planner.core_reservations(), &[], 8).ready);
    }

    #[test]
    fn hidden_hostile_cannot_trigger_an_omniscient_relief_response() {
        let mut obs = observation(100);
        let hostile = obs.enemy_units[0].tile;
        obs.visible[hostile.y as usize * 40 + hostile.x as usize] = false;
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
            ],
        );

        let mut planner = TeamReliefPlanner::new();
        let profile = profile();
        let tuning = tuning();
        assert_eq!(
            planner.think(&profile, tuning, &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );
        obs.tick += pressure_response_delay(tuning);
        assert_eq!(
            planner.think(&profile, tuning, &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );
        assert!(planner.operation().is_none());
    }

    #[test]
    fn ground_relief_does_not_march_into_an_air_only_emergency() {
        let mut obs = observation(100);
        obs.enemy_units = vec![unit(
            90,
            PlayerId(2),
            UnitKind::Buzzard,
            TilePos::new(27, 10),
            false,
        )];
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
            ],
        );

        let mut planner = TeamReliefPlanner::new();
        let profile = profile();
        let tuning = tuning();
        assert_eq!(
            planner.think(&profile, tuning, &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );
        obs.tick += pressure_response_delay(tuning);
        assert_eq!(
            planner.think(&profile, tuning, &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );
        assert!(planner.operation().is_none());
    }

    #[test]
    fn visible_sapper_pressure_completes_the_credible_watch_and_relief_handoff() {
        let mut obs = observation(100);
        obs.enemy_units = vec![unit(
            90,
            PlayerId(2),
            UnitKind::Sapper,
            TilePos::new(27, 10),
            false,
        )];
        let sapper = UnitKind::Sapper.stats();
        assert!(sapper.can_fight());
        assert!(!sapper.can_target(Domain::Ground));
        assert!(sapper.demolition);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
            ],
        );
        let identity = profile();
        let tuning = tuning();
        let mut planner = TeamReliefPlanner::new();

        let watching = planner.think(&identity, tuning, &obs, HOME, &[], &[]);
        assert!(watching.intents.is_empty());
        assert_eq!(
            watching.reservations,
            [UnitId(1), UnitId(2), UnitId(3), UnitId(4), UnitId(5)],
            "the credibility window protects both the exact relief group and home floor"
        );
        assert!(planner.operation().is_none());

        obs.tick = obs.tick.saturating_add(pressure_response_delay(tuning));
        let outbound = planner.think(&identity, tuning, &obs, HOME, &[], &[]);
        let operation = planner
            .operation()
            .expect("credible pressure starts relief");
        assert_eq!(operation.members, [UnitId(3), UnitId(4), UnitId(5)]);
        assert_eq!(operation.home_defenders, [UnitId(1), UnitId(2)]);
        assert_eq!(operation.committed_size, 3);
        assert_eq!(operation.phase, TeamReliefPhase::Holding);
        let members = operation.members.clone();
        assert_eq!(outbound.reservations, members);
        assert_eq!(
            outbound.intents,
            [Intent::AttackMoveUnits {
                units: members.clone(),
                goal: ALLY_BASE,
            }]
        );

        obs.tick += 1;
        let focus = planner.think(&identity, tuning, &obs, HOME, &[], &[]);
        assert_eq!(focus.reservations, members);
        assert_eq!(
            focus.intents,
            [Intent::AttackUnits {
                units: members,
                target: Target::Unit(UnitId(90)),
            }]
        );
    }

    #[test]
    fn a_pending_relief_refreshes_away_from_reserved_wounded_and_dead_members() {
        #[derive(Clone, Copy, Debug)]
        enum Disruption {
            Reserved,
            Wounded,
            Dead,
        }

        for disruption in [Disruption::Reserved, Disruption::Wounded, Disruption::Dead] {
            let mut obs = observation(100);
            add_fighters(
                &mut obs,
                &[
                    (1, TilePos::new(3, 9)),
                    (2, TilePos::new(3, 11)),
                    (3, TilePos::new(22, 10)),
                    (4, TilePos::new(21, 10)),
                    (5, TilePos::new(20, 10)),
                    (6, TilePos::new(19, 10)),
                ],
            );
            let identity = profile();
            let tuning = tuning();
            let mut planner = TeamReliefPlanner::new();

            let pending = planner.think(&identity, tuning, &obs, HOME, &[], &[]);
            assert_eq!(
                pending.reservations,
                [UnitId(1), UnitId(2), UnitId(3), UnitId(4), UnitId(5)]
            );
            assert_eq!(planner.reservations(), pending.reservations);
            let watch = planner.watch.as_ref().expect("pressure starts a watch");
            let first_seen_at = watch.first_seen_at;
            let started_at = watch.relief.started_at;
            assert_eq!(watch.relief.members, [UnitId(3), UnitId(4), UnitId(5)]);

            let newly_reserved = if matches!(disruption, Disruption::Reserved) {
                vec![UnitId(3)]
            } else {
                Vec::new()
            };
            match disruption {
                Disruption::Reserved => {}
                Disruption::Wounded => {
                    let member = obs
                        .my_units
                        .iter_mut()
                        .find(|unit| unit.id == UnitId(3))
                        .expect("the watched member exists");
                    member.hp = member.kind.stats().max_hp / 2;
                }
                Disruption::Dead => obs.my_units.retain(|unit| unit.id != UnitId(3)),
            }

            obs.tick = first_seen_at + pressure_response_delay(tuning) - 1;
            let refreshed = planner.think(&identity, tuning, &obs, HOME, &[], &newly_reserved);
            assert!(refreshed.intents.is_empty(), "{disruption:?}");
            assert_eq!(
                refreshed.reservations,
                [UnitId(1), UnitId(2), UnitId(4), UnitId(5), UnitId(6)],
                "{disruption:?}"
            );
            assert_eq!(planner.reservations(), refreshed.reservations);
            assert!(
                refreshed
                    .reservations
                    .windows(2)
                    .all(|pair| pair[0] < pair[1]),
                "{disruption:?} produced duplicate or noncanonical claims"
            );
            let refreshed_watch = planner.watch.as_ref().expect("replacement group is viable");
            assert_eq!(refreshed_watch.first_seen_at, first_seen_at);
            assert_eq!(refreshed_watch.relief.started_at, started_at);
            assert_eq!(
                refreshed_watch.relief.members,
                [UnitId(4), UnitId(5), UnitId(6)]
            );

            obs.tick = first_seen_at + pressure_response_delay(tuning);
            let committed = planner.think(&identity, tuning, &obs, HOME, &[], &newly_reserved);
            let operation = planner
                .operation()
                .expect("refreshed relief commits on time");
            assert_eq!(operation.members, [UnitId(4), UnitId(5), UnitId(6)]);
            assert_eq!(committed.reservations, operation.members);
            assert!(committed.reservations.binary_search(&UnitId(3)).is_err());

            if matches!(disruption, Disruption::Reserved) {
                let mut intents = vec![Intent::MoveUnits {
                    units: newly_reserved.clone(),
                    goal: HOME,
                }];
                intents.extend(committed.intents.clone());
                let mut reservations = newly_reserved.clone();
                reservations.extend(committed.reservations.iter().copied());
                reservations.sort_unstable();
                reservations.dedup();
                let commands = super::super::executive::Executive::default()
                    .apply_with_reservations(PlayerId(0), &obs, &intents, &reservations);
                let mut commanded = Vec::new();
                for command in &commands {
                    match &command.command {
                        crate::Command::Move { units, .. }
                        | crate::Command::AttackMove { units, .. }
                        | crate::Command::Attack { units, .. } => {
                            commanded.extend(units.iter().copied());
                        }
                        _ => {}
                    }
                }
                let commanded_count = commanded.len();
                commanded.sort_unstable();
                commanded.dedup();
                assert_eq!(commands.len(), 2);
                assert_eq!(commanded.len(), commanded_count);
                assert_eq!(commanded, [UnitId(3), UnitId(4), UnitId(5), UnitId(6)]);

                let mut state = relief_command_state();
                let report = state.tick(&commands);
                assert!(
                    report
                        .events
                        .iter()
                        .all(|event| !matches!(event, crate::Event::CommandRejected { .. })),
                    "the disjoint operation batch must be accepted: {:?}",
                    report.events
                );
            }
        }
    }

    #[test]
    fn a_pending_relief_cancels_when_no_healthy_unclaimed_quorum_remains() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(22, 10)),
                (4, TilePos::new(21, 10)),
                (5, TilePos::new(20, 10)),
            ],
        );
        let identity = profile();
        let tuning = tuning();
        let mut planner = TeamReliefPlanner::new();
        let pending = planner.think(&identity, tuning, &obs, HOME, &[], &[]);
        assert_eq!(planner.reservations(), pending.reservations);

        let first_seen_at = planner
            .watch
            .as_ref()
            .expect("pressure starts a watch")
            .first_seen_at;
        for id in [UnitId(3), UnitId(4)] {
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == id)
                .expect("the watched member exists")
                .hp = 1;
        }
        obs.tick = first_seen_at + pressure_response_delay(tuning);

        assert_eq!(
            planner.think(&identity, tuning, &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );
        assert!(planner.watch.is_none());
        assert!(planner.operation().is_none());
        assert!(planner.reservations().is_empty());
    }

    fn relief_command_state() -> crate::State {
        let width = 40usize;
        let height = 24usize;
        let mut map = vec![format!("#{}#", ".".repeat(width - 2)); height];
        map[0] = "#".repeat(width);
        map[height - 1] = "#".repeat(width);
        for (x, y, marker) in [(2usize, 2usize, b'1'), (35usize, 20usize, b'2')] {
            let mut row = map[y].as_bytes().to_vec();
            row[x] = marker;
            map[y] = String::from_utf8(row).expect("the authored row is ASCII");
        }
        Scenario {
            name: "team relief command acceptance".into(),
            seed: 19,
            map,
            players: vec![
                PlayerSpec {
                    name: "Ferrous".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "Cupric".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                },
            ],
            units: (0..7)
                .map(|offset| UnitSpec {
                    player: 0,
                    kind: UnitKind::Sentinel,
                    x: 4 + offset,
                    y: 10,
                })
                .collect(),
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("the command-acceptance scenario builds")
    }

    #[test]
    fn relief_reserves_one_exact_sorted_group_and_protects_other_claims() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (9, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (8, TilePos::new(19, 9)),
                (5, TilePos::new(20, 10)),
                (7, TilePos::new(21, 11)),
                (4, TilePos::new(22, 10)),
                (6, TilePos::new(23, 11)),
            ],
        );
        obs.my_units.push(unit(
            3,
            PlayerId(0),
            UnitKind::Sentinel,
            TilePos::new(23, 9),
            false,
        ));
        obs.my_units.push(unit(
            1,
            PlayerId(0),
            UnitKind::Buzzard,
            TilePos::new(23, 10),
            true,
        ));

        let mut planner = TeamReliefPlanner::new();
        let decision = start_relief(
            &mut planner,
            &profile(),
            tuning(),
            &mut obs,
            &[UnitId(6)],
            &[UnitId(4)],
        );

        assert_eq!(decision.reservations, vec![UnitId(5), UnitId(7), UnitId(8)]);
        let operation = planner.operation().expect("relief remains active");
        assert_eq!(operation.members, vec![UnitId(5), UnitId(7), UnitId(8)]);
        assert_eq!(operation.home_defenders, vec![UnitId(2), UnitId(9)]);
        assert_eq!(
            decision.intents,
            vec![Intent::AttackMoveUnits {
                units: operation.members.clone(),
                goal: ALLY_BASE,
            }]
        );
    }

    #[test]
    fn relief_focuses_a_distant_base_threat_once_after_arrival() {
        let mut obs = observation(100);
        obs.enemy_units[0].tile = TilePos::new(32, 10);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(23, 9)),
                (4, TilePos::new(24, 11)),
                (5, TilePos::new(25, 10)),
            ],
        );
        let mut planner = TeamReliefPlanner::new();

        let outbound = start_relief(&mut planner, &profile(), tuning(), &mut obs, &[], &[]);
        assert!(matches!(
            outbound.intents.as_slice(),
            [Intent::AttackMoveUnits { goal, .. }] if *goal == ALLY_BASE
        ));
        assert_eq!(
            planner.operation().map(|operation| operation.phase),
            Some(TeamReliefPhase::Holding)
        );
        let members = planner
            .operation()
            .expect("relief remains active")
            .members
            .clone();

        obs.tick += 1;
        let focus = planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);
        assert_eq!(
            focus.intents,
            [Intent::AttackUnits {
                units: members,
                target: Target::Unit(UnitId(90)),
            }]
        );

        obs.tick += 1;
        let stable = planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);
        assert!(stable.intents.is_empty());
        assert_eq!(stable.reservations, focus.reservations);
    }

    #[test]
    fn home_defense_floor_refuses_to_strip_the_last_fighters() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
            ],
        );

        let mut planner = TeamReliefPlanner::new();
        let profile = profile();
        let tuning = tuning();
        assert_eq!(
            planner.think(&profile, tuning, &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );
        obs.tick += pressure_response_delay(tuning);
        assert_eq!(
            planner.think(&profile, tuning, &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );
        assert!(planner.operation().is_none());
    }

    #[test]
    fn resolved_fortification_controls_the_exact_home_defense_floor() {
        let low_profile =
            resolved_profile_where(BotStance::Turtle, "fortification below 75", |profile| {
                profile.traits.fortification < 75
            });
        let high_profile =
            resolved_profile_where(BotStance::Turtle, "fortification at least 75", |profile| {
                profile.traits.fortification >= 75
            });
        assert!(eligible(&low_profile), "premise: {low_profile:?}");
        assert!(eligible(&high_profile), "premise: {high_profile:?}");

        let begin_with = |profile: &ResolvedProfile| {
            let mut obs = observation(100);
            add_fighters(
                &mut obs,
                &[
                    (1, TilePos::new(3, 9)),
                    (2, TilePos::new(3, 11)),
                    (3, TilePos::new(4, 10)),
                    (4, TilePos::new(19, 9)),
                    (5, TilePos::new(19, 11)),
                    (6, TilePos::new(20, 9)),
                    (7, TilePos::new(20, 11)),
                    (8, TilePos::new(21, 9)),
                    (9, TilePos::new(21, 11)),
                ],
            );
            let mut planner = TeamReliefPlanner::new();
            let decision = start_relief(
                &mut planner,
                profile,
                DifficultyTuning::for_level(profile.difficulty),
                &mut obs,
                &[],
                &[],
            );
            let operation = planner.operation().expect("relief remains active");
            assert!(matches!(
                decision.intents.as_slice(),
                [Intent::AttackMoveUnits { goal, .. }] if *goal == ALLY_BASE
            ));
            operation.home_defenders.clone()
        };

        assert_eq!(
            begin_with(&low_profile),
            vec![UnitId(1), UnitId(2)],
            "the ordinary fortifier keeps the shared two-fighter floor"
        );
        assert_eq!(
            begin_with(&high_profile),
            vec![UnitId(1), UnitId(2), UnitId(3)],
            "a fortification specialist must visibly retain one more exact home defender"
        );
    }

    #[test]
    fn wounded_or_already_claimed_fighters_do_not_pad_a_relief_group() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
                (6, TilePos::new(22, 10)),
            ],
        );
        let wounded = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(3))
            .expect("wounded fighter exists");
        wounded.hp = wounded.kind.stats().max_hp / 2;

        let mut planner = TeamReliefPlanner::new();
        let profile = profile();
        let tuning = tuning();
        assert_eq!(
            planner.think(&profile, tuning, &obs, HOME, &[UnitId(4)], &[UnitId(5)],),
            StrategicDecision::default()
        );
        obs.tick += pressure_response_delay(tuning);
        let decision = planner.think(&profile, tuning, &obs, HOME, &[UnitId(4)], &[UnitId(5)]);

        assert!(decision.intents.is_empty());
        assert!(decision.reservations.is_empty());
        assert!(planner.operation().is_none());
    }

    #[test]
    fn pressure_ending_withdraws_the_reserved_survivors_home() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
            ],
        );
        let mut planner = TeamReliefPlanner::new();
        let started = start_relief(&mut planner, &profile(), tuning(), &mut obs, &[], &[]);
        let committed = planner
            .operation()
            .expect("relief remains active")
            .members
            .clone();
        assert_eq!(
            planner.operation().map(|operation| operation.phase),
            Some(TeamReliefPhase::Holding),
            "the no-pressure transition is tested from an established hold"
        );

        obs.tick += 1;
        obs.enemy_units.clear();
        let withdrawal = planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);
        assert_eq!(withdrawal.reservations, started.reservations);
        assert_eq!(
            withdrawal.intents,
            vec![Intent::MoveUnits {
                units: committed,
                goal: HOME,
            }]
        );
        let operation = planner.operation().expect("return remains persistent");
        assert_eq!(operation.phase, TeamReliefPhase::Withdrawing);
        assert_eq!(
            operation.exit_reason,
            Some(TeamReliefExitReason::PressureEnded)
        );
    }

    #[test]
    fn loss_budget_withdraws_only_after_more_than_one_third_is_lost() {
        let (mut obs, mut planner, committed) = active_three_member_relief();
        assert_eq!(committed, vec![UnitId(3), UnitId(4), UnitId(5)]);

        obs.my_units.retain(|unit| unit.id != UnitId(3));
        obs.tick += 1;
        planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);
        let boundary = planner
            .operation()
            .expect("losing exactly one third stays inside the loss budget");
        assert_eq!(boundary.members, vec![UnitId(4), UnitId(5)]);
        assert_eq!(boundary.phase, TeamReliefPhase::Holding);
        assert_eq!(boundary.exit_reason, None);

        obs.my_units.retain(|unit| unit.id != UnitId(4));
        obs.tick += 1;
        assert!(
            !health_budget_spent(
                &obs,
                planner
                    .operation()
                    .expect("the relief is active before the second loss"),
            ),
            "the Warden's surviving hull isolates the member-count exit"
        );
        let withdrawal = planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);

        assert_eq!(withdrawal.reservations, vec![UnitId(5)]);
        assert_eq!(
            withdrawal.intents,
            vec![Intent::MoveUnits {
                units: vec![UnitId(5)],
                goal: HOME,
            }]
        );
        let operation = planner.operation().expect("the survivor returns home");
        assert_eq!(operation.phase, TeamReliefPhase::Withdrawing);
        assert_eq!(
            operation.exit_reason,
            Some(TeamReliefExitReason::LossBudget)
        );
    }

    #[test]
    fn relief_times_out_at_the_exact_deadline_while_pressure_remains() {
        let (mut obs, mut planner, committed) = active_three_member_relief();
        let profile = profile();
        let tuning = tuning();
        let started_at = planner
            .operation()
            .expect("the relief remains active")
            .started_at;
        let deadline = started_at.saturating_add(operation_timeout(&profile));
        obs.tick = deadline - 1;
        planner.think(&profile, tuning, &obs, HOME, &[], &[]);
        let before_deadline = planner
            .operation()
            .expect("sustained pressure keeps relief active before its deadline");
        assert_eq!(before_deadline.phase, TeamReliefPhase::Holding);
        assert_eq!(before_deadline.exit_reason, None);

        obs.tick = deadline;
        let withdrawal = planner.think(&profile, tuning, &obs, HOME, &[], &[]);

        assert_eq!(withdrawal.reservations, committed);
        assert_eq!(
            withdrawal.intents,
            vec![Intent::MoveUnits {
                units: committed,
                goal: HOME,
            }]
        );
        let operation = planner
            .operation()
            .expect("the timed-out group returns home");
        assert_eq!(operation.phase, TeamReliefPhase::Withdrawing);
        assert_eq!(operation.phase_started_at, deadline);
        assert_eq!(operation.exit_reason, Some(TeamReliefExitReason::Timeout));
    }

    #[test]
    fn spent_hull_budget_withdraws_even_while_pressure_remains() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
            ],
        );
        let mut planner = TeamReliefPlanner::new();
        start_relief(&mut planner, &profile(), tuning(), &mut obs, &[], &[]);
        let committed = planner
            .operation()
            .expect("relief remains active")
            .members
            .clone();
        for unit in &mut obs.my_units {
            if committed.contains(&unit.id) {
                unit.hp = unit.kind.stats().max_hp / 5;
            }
        }
        obs.tick += 1;

        let withdrawal = planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);
        assert_eq!(
            withdrawal.intents,
            vec![Intent::MoveUnits {
                units: committed,
                goal: HOME,
            }]
        );
        assert_eq!(
            planner
                .operation()
                .and_then(|operation| operation.exit_reason),
            Some(TeamReliefExitReason::HealthBudget)
        );
    }

    #[test]
    fn losing_the_allied_foundry_withdraws_without_substituting_another_base() {
        let mut obs = observation(100);
        obs.ally_buildings
            .push(building(21, PlayerId(1), TilePos::new(30, 18)));
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
            ],
        );
        let mut planner = TeamReliefPlanner::new();
        start_relief(&mut planner, &profile(), tuning(), &mut obs, &[], &[]);
        let committed = planner
            .operation()
            .expect("relief remains active")
            .members
            .clone();
        obs.tick += 1;
        obs.ally_buildings
            .retain(|building| building.id != BuildingId(20));

        let withdrawal = planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);
        assert_eq!(
            withdrawal.intents,
            vec![Intent::MoveUnits {
                units: committed,
                goal: HOME,
            }]
        );
        assert_eq!(
            planner
                .operation()
                .and_then(|operation| operation.exit_reason),
            Some(TeamReliefExitReason::FoundryLost)
        );
    }

    #[test]
    fn a_completed_withdrawal_starts_a_real_cooldown() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
            ],
        );
        let mut planner = TeamReliefPlanner::new();
        let started = start_relief(&mut planner, &profile(), tuning(), &mut obs, &[], &[]);
        let committed = planner
            .operation()
            .expect("relief remains active")
            .members
            .clone();

        obs.tick += 1;
        obs.enemy_units.clear();
        planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);
        for unit in &mut obs.my_units {
            if committed.contains(&unit.id) {
                unit.tile = HOME;
            }
        }
        obs.tick += 1;
        let returned = planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);
        assert_eq!(returned.reservations, started.reservations);
        assert!(planner.operation().is_none());
        assert!(planner.cooldown_until() > obs.tick);

        obs.enemy_units.push(unit(
            91,
            PlayerId(2),
            UnitKind::Sentinel,
            TilePos::new(27, 10),
            false,
        ));
        assert_eq!(
            planner.think(&profile(), tuning(), &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );
    }

    #[test]
    fn selection_is_identical_under_noncanonical_input_order() {
        let mut canonical = observation(100);
        canonical
            .ally_buildings
            .push(building(10, PlayerId(1), TilePos::new(24, 18)));
        canonical.enemy_units.push(unit(
            80,
            PlayerId(2),
            UnitKind::Warden,
            TilePos::new(26, 18),
            false,
        ));
        canonical.visible[18 * 40 + 26] = true;
        add_fighters(
            &mut canonical,
            &[
                (7, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (9, TilePos::new(19, 9)),
                (4, TilePos::new(20, 10)),
                (6, TilePos::new(21, 11)),
                (3, TilePos::new(22, 10)),
            ],
        );
        let mut shuffled = canonical.clone();
        shuffled.my_units.reverse();
        shuffled.ally_buildings.reverse();
        shuffled.enemy_units.reverse();

        let decide = |source: &Observation| {
            let mut obs = source.clone();
            start_relief(
                &mut TeamReliefPlanner::new(),
                &profile(),
                tuning(),
                &mut obs,
                &[],
                &[],
            )
        };
        assert_eq!(decide(&canonical), decide(&shuffled));
    }

    #[test]
    fn mixed_relief_ranking_and_four_member_target_ignore_specialty_order() {
        let mut support_first = profile();
        support_first.primary = Specialty::Support;
        support_first.secondary = Specialty::Fortification;
        support_first.traits.support = 80;
        support_first.traits.fortification = 75;
        let mut fortification_first = support_first;
        fortification_first.primary = Specialty::Fortification;
        fortification_first.secondary = Specialty::Support;
        assert_eq!(desired_group_size(&support_first), 4);
        assert_eq!(home_defense_floor(&support_first), 3);

        let mut roster = observation(100);
        roster.my_units = vec![
            unit(1, PlayerId(0), UnitKind::Sentinel, TilePos::new(3, 9), true),
            unit(2, PlayerId(0), UnitKind::Sentinel, HOME, true),
            unit(
                3,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(3, 11),
                true,
            ),
            unit(
                40,
                PlayerId(0),
                UnitKind::Avalanche,
                TilePos::new(18, 10),
                true,
            ),
            unit(
                50,
                PlayerId(0),
                UnitKind::Bombard,
                TilePos::new(18, 10),
                true,
            ),
            unit(
                60,
                PlayerId(0),
                UnitKind::Lancer,
                TilePos::new(18, 10),
                true,
            ),
            unit(
                70,
                PlayerId(0),
                UnitKind::Scuttler,
                TilePos::new(18, 10),
                true,
            ),
            unit(
                80,
                PlayerId(0),
                UnitKind::Warden,
                TilePos::new(18, 10),
                true,
            ),
        ];
        let expected_members = vec![UnitId(50), UnitId(60), UnitId(70), UnitId(80)];
        let expected_home = vec![UnitId(1), UnitId(2), UnitId(3)];
        let expected_hull = [
            UnitKind::Bombard,
            UnitKind::Lancer,
            UnitKind::Scuttler,
            UnitKind::Warden,
        ]
        .into_iter()
        .fold(0_u32, |total, kind| {
            total.saturating_add(kind.stats().max_hp)
        });
        let expected_started_at = roster.tick;

        let decide = |profile: &ResolvedProfile, source: &Observation| {
            let mut obs = source.clone();
            let mut planner = TeamReliefPlanner::new();
            let decision = start_relief(
                &mut planner,
                profile,
                DifficultyTuning::for_level(profile.difficulty),
                &mut obs,
                &[],
                &[],
            );
            (decision, planner.operation().cloned())
        };
        let expected_decision = StrategicDecision {
            intents: vec![Intent::AttackMoveUnits {
                units: expected_members.clone(),
                goal: ALLY_BASE,
            }],
            reservations: expected_members.clone(),
            ..StrategicDecision::default()
        };
        let expected_operation = TeamReliefOperation {
            ally: PlayerId(1),
            foundry: BuildingId(20),
            anchor: ALLY_BASE,
            members: expected_members,
            home_defenders: expected_home,
            committed_size: 4,
            committed_max_hp: expected_hull,
            phase: TeamReliefPhase::Deploying,
            started_at: expected_started_at,
            phase_started_at: expected_started_at,
            exit_reason: None,
            dispatch: Some(TeamReliefDispatch::Outbound(ALLY_BASE)),
        };

        assert_eq!(
            decide(&support_first, &roster),
            (expected_decision.clone(), Some(expected_operation.clone())),
            "equal-distance relief candidates must prefer line, raid, then ordinary artillery roles and leave the Avalanche out of the four-member cohort"
        );
        assert_eq!(
            decide(&fortification_first, &roster),
            (expected_decision.clone(), Some(expected_operation.clone())),
            "swapping the same Support and Fortification specialties must not redeal relief ownership"
        );
        roster.my_units.reverse();
        assert_eq!(
            decide(&fortification_first, &roster),
            (expected_decision, Some(expected_operation)),
            "roster storage order must not change the exact mixed relief group or command"
        );
    }

    #[test]
    fn every_difficulty_requires_shared_credibility_before_its_own_latency() {
        let mut frozen_groups = Vec::new();
        let mut response_ticks = Vec::new();
        for difficulty in [
            BotDifficulty::Scrapheap,
            BotDifficulty::Standard,
            BotDifficulty::Veteran,
            BotDifficulty::Prime,
        ] {
            let mut obs = observation(100);
            add_fighters(
                &mut obs,
                &[
                    (1, TilePos::new(3, 9)),
                    (2, TilePos::new(3, 11)),
                    (3, TilePos::new(20, 9)),
                    (4, TilePos::new(20, 11)),
                    (5, TilePos::new(21, 10)),
                ],
            );
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 20_042).resolve_profile();
            assert!(eligible(&profile), "{difficulty:?}: {profile:?}");
            let tuning = DifficultyTuning::for_level(difficulty);
            let response_delay = pressure_response_delay(tuning);
            let mut planner = TeamReliefPlanner::new();

            let pending = planner.think(&profile, tuning, &obs, HOME, &[], &[]);
            assert!(
                pending.intents.is_empty(),
                "{difficulty:?} must first observe the pressure"
            );
            assert!(!pending.reservations.is_empty());
            let frozen = planner.watch.as_ref().unwrap().relief.clone();
            add_fighters(
                &mut obs,
                &[
                    (20, TilePos::new(20, 8)),
                    (21, TilePos::new(21, 8)),
                    (22, TilePos::new(22, 8)),
                ],
            );
            obs.tick += response_delay - 1;
            let still_pending = planner.think(&profile, tuning, &obs, HOME, &[], &[]);
            assert!(
                still_pending.intents.is_empty(),
                "{difficulty:?} acted before credibility plus reaction latency"
            );
            assert_eq!(still_pending.reservations, pending.reservations);
            obs.tick += 1;
            let committed = planner.think(&profile, tuning, &obs, HOME, &[], &[]);
            assert!(
                !committed.intents.is_empty(),
                "{difficulty:?} did not act once sustained pressure was actionable"
            );
            let operation = planner.operation().unwrap();
            assert_eq!(operation.members, frozen.members, "{difficulty:?}");
            assert_eq!(
                operation.home_defenders, frozen.home_defenders,
                "{difficulty:?}"
            );
            frozen_groups.push((operation.members.clone(), operation.home_defenders.clone()));
            response_ticks.push(obs.tick);
        }
        assert!(frozen_groups.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(response_ticks.windows(2).all(|pair| pair[0] >= pair[1]));
    }

    #[test]
    fn a_transient_pressure_resets_primes_credibility_watch() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 9)),
                (2, TilePos::new(3, 11)),
                (3, TilePos::new(20, 9)),
                (4, TilePos::new(20, 11)),
                (5, TilePos::new(21, 10)),
            ],
        );
        let profile = profile();
        let tuning = tuning();
        let mut planner = TeamReliefPlanner::new();

        let first_pending = planner.think(&profile, tuning, &obs, HOME, &[], &[]);
        assert!(first_pending.intents.is_empty());
        assert!(!first_pending.reservations.is_empty());
        obs.tick += PRESSURE_CREDIBILITY - 1;
        obs.enemy_units.clear();
        assert_eq!(
            planner.think(&profile, tuning, &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );

        obs.tick += 1;
        obs.enemy_units.push(unit(
            91,
            PlayerId(2),
            UnitKind::Sentinel,
            TilePos::new(27, 10),
            false,
        ));
        obs.tick = super::super::difficulty::strategic_admission_at_or_after(obs.tick);
        let second_pending = planner.think(&profile, tuning, &obs, HOME, &[], &[]);
        assert!(second_pending.intents.is_empty());
        assert_eq!(second_pending.reservations, first_pending.reservations);
        obs.tick += PRESSURE_CREDIBILITY - 1;
        let still_pending = planner.think(&profile, tuning, &obs, HOME, &[], &[]);
        assert!(
            still_pending.intents.is_empty(),
            "the second sighting must earn a fresh credibility window"
        );
        assert_eq!(still_pending.reservations, second_pending.reservations);
        obs.tick += 1;
        assert!(
            !planner
                .think(&profile, tuning, &obs, HOME, &[], &[])
                .intents
                .is_empty()
        );
    }

    #[test]
    fn difficulty_does_not_change_relief_force_or_home_defenders() {
        let mut expected_members = None;
        for difficulty in [
            BotDifficulty::Scrapheap,
            BotDifficulty::Standard,
            BotDifficulty::Veteran,
            BotDifficulty::Prime,
        ] {
            let mut obs = observation(100);
            add_fighters(
                &mut obs,
                &[
                    (1, TilePos::new(3, 9)),
                    (2, TilePos::new(3, 11)),
                    (3, TilePos::new(4, 10)),
                    (4, TilePos::new(20, 9)),
                    (5, TilePos::new(20, 11)),
                    (6, TilePos::new(21, 10)),
                    (7, TilePos::new(22, 10)),
                ],
            );
            let profile =
                BotConfig::scripted(difficulty, BotStance::Balanced, 20_042).resolve_profile();
            assert!(eligible(&profile), "{difficulty:?}: {profile:?}");
            let tuning = DifficultyTuning::for_level(difficulty);
            let mut planner = TeamReliefPlanner::new();
            let decision = start_relief(&mut planner, &profile, tuning, &mut obs, &[], &[]);
            let operation = planner.operation().expect("relief remains active");

            assert_eq!(decision.reservations, operation.members);
            assert!(
                operation
                    .home_defenders
                    .iter()
                    .all(|id| !decision.reservations.contains(id)),
                "{difficulty:?} must release the home floor to ordinary defense"
            );
            if let Some(expected) = &expected_members {
                assert_eq!(
                    &operation.members, expected,
                    "difficulty must alter response timing, not force allocation"
                );
            } else {
                expected_members = Some(operation.members.clone());
            }
        }
    }

    #[test]
    fn known_severance_refuses_ground_relief_but_a_gap_restores_it() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 8)),
                (2, TilePos::new(3, 12)),
                (3, TilePos::new(5, 8)),
                (4, TilePos::new(5, 10)),
                (5, TilePos::new(5, 12)),
            ],
        );
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(14, y)).collect();
        let mut planner = TeamReliefPlanner::new();

        assert_eq!(
            planner.think(&profile(), tuning(), &obs, HOME, &[], &[]),
            StrategicDecision::default()
        );
        assert!(planner.operation().is_none());

        obs.known_rock.retain(|tile| tile.y != 10);
        let decision = start_relief(&mut planner, &profile(), tuning(), &mut obs, &[], &[]);
        assert!(matches!(
            decision.intents.as_slice(),
            [Intent::AttackMoveUnits { goal, .. }] if *goal == ALLY_BASE
        ));
    }

    #[test]
    fn newly_mapped_severance_releases_relief_without_repeating_orders() {
        let mut obs = observation(100);
        add_fighters(
            &mut obs,
            &[
                (1, TilePos::new(3, 8)),
                (2, TilePos::new(3, 12)),
                (3, TilePos::new(5, 8)),
                (4, TilePos::new(5, 10)),
                (5, TilePos::new(5, 12)),
            ],
        );
        let mut planner = TeamReliefPlanner::new();
        start_relief(&mut planner, &profile(), tuning(), &mut obs, &[], &[]);

        obs.tick += 1;
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(14, y)).collect();
        let decision = planner.think(&profile(), tuning(), &obs, HOME, &[], &[]);

        assert!(decision.intents.is_empty());
        assert!(planner.operation().is_none());
        assert!(planner.cooldown_until() > obs.tick);
    }
}
