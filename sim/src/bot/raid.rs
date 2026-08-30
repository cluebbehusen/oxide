//! Bounded economy harassment shaped by each scripted identity's guile.
//!
//! Raids use an exact Scuttler group, current sight, explicit loss and time
//! budgets, and a real withdrawal phase. Losing sight in fog is never treated
//! as proof that the objective died.

use super::difficulty::{DifficultyTuning, strategic_admission_tick};
use super::executive::Intent;
use super::observation::{Observation, UnitObs};
use super::profile::ResolvedProfile;
use super::routing::{RouteProjection, first_reachable_group};
use super::strategy::StrategicDecision;
use crate::ids::{BuildingId, PlayerId, Target, UnitId};
use crate::scenario::BotStance;
use crate::stats::{BuildingKind, Domain, UnitKind};
use chassis::Tick;
use chassis::grid::TilePos;

const RAID_GROUP_SIZE: usize = 2;
const HOME_SCREEN_RADIUS: i32 = 8;

/// The active phase of a harassment operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidPhase {
    /// Close on a currently observed weak point.
    Ingress,
    /// Concentrate the exact raiding group on its objective.
    Strike,
    /// Break contact and return surviving raiders home.
    Egress,
}

/// Why a raid broke contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidExitReason {
    /// Current sight over the objective confirmed it was gone.
    Complete,
    /// The objective disappeared into darkness, which proves nothing else.
    LostContact,
    /// The operation spent its allowed losses or health.
    LossBudget,
    /// A visible response made the raid a losing fight.
    EnemyResponse,
    /// The bounded operation ran out of time.
    Timeout,
    /// Explored terrain proved that the exact group cannot reach its order.
    Unreachable,
}

/// The persistent order currently owned by a raid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidDispatch {
    /// Attack-move toward the last observed objective tile.
    Ingress(TilePos),
    /// Attack the stable observed objective.
    Strike(Target),
    /// Return home without acquiring another target.
    Egress(TilePos),
}

/// The exact hostile asset a raid is trying to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RaidObjective {
    /// A visible economic unit.
    Unit {
        /// Stable target id.
        id: UnitId,
        /// Last observed kind.
        kind: UnitKind,
    },
    /// A visible economic or support structure.
    Building {
        /// Stable target id.
        id: BuildingId,
        /// Last observed kind.
        kind: BuildingKind,
    },
}

/// Inspectable persistent state of one bounded raid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaidOperation {
    /// Player whose asset was selected.
    pub target_player: PlayerId,
    /// Stable target identity.
    pub objective: RaidObjective,
    /// Last currently observed target tile.
    pub last_tile: TilePos,
    /// Exact surviving operation members, sorted.
    pub members: Vec<UnitId>,
    /// Group size at commitment, used for the loss budget.
    pub committed_size: usize,
    /// Current operation phase.
    pub phase: RaidPhase,
    /// Tick at which the raid committed.
    pub started_at: Tick,
    /// Tick at which the current phase began.
    pub phase_started_at: Tick,
    /// Set only once the raid begins withdrawing.
    pub exit_reason: Option<RaidExitReason>,
    /// Last operation order sent to the exact group.
    pub dispatch: Option<RaidDispatch>,
}

/// Controller-local owner of a guile raid and its cooldown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RaidPlanner {
    active: Option<RaidOperation>,
    muster: Vec<UnitId>,
    cooldown_until: Tick,
}

impl RaidPlanner {
    /// Creates an idle raid planner.
    pub fn new() -> Self {
        Self::default()
    }

    /// The currently active raid, if any.
    pub fn operation(&self) -> Option<&RaidOperation> {
        self.active.as_ref()
    }

    /// Exact Scuttlers owned while a raid gathers or operates.
    pub fn reservations(&self) -> &[UnitId] {
        self.active
            .as_ref()
            .map_or(self.muster.as_slice(), |operation| {
                operation.members.as_slice()
            })
    }

    /// Earliest tick at which another raid may begin.
    pub fn cooldown_until(&self) -> Tick {
        self.cooldown_until
    }

    /// Advances one raid using current fog-honest observation only.
    pub fn think(
        &mut self,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &Observation,
        home: TilePos,
        enlisted: &[UnitId],
        additionally_reserved: &[UnitId],
    ) -> StrategicDecision {
        let mut routes = RouteProjection::new(obs, Domain::Ground);
        if self.active.is_none()
            && obs.tick >= self.cooldown_until
            && strategic_admission_tick(obs.tick)
        {
            self.refresh_muster(obs, enlisted, additionally_reserved);
            if self.muster.len() == RAID_GROUP_SIZE
                && self
                    .muster
                    .iter()
                    .all(|id| own_unit(obs, *id).is_some_and(|unit| unit.idle))
                && home_screen_ready(profile, obs, home, &self.muster)
                && let Some(operation) = begin(obs, &self.muster, &mut routes)
            {
                self.active = Some(operation);
                self.muster.clear();
            }
        }
        let Some(mut raid) = self.active.take() else {
            return StrategicDecision {
                reservations: self.muster.clone(),
                ..StrategicDecision::default()
            };
        };

        raid.members.retain(|id| own_unit(obs, *id).is_some());
        let current = current_objective(obs, &raid);
        if let Some((_, tile)) = current {
            raid.last_tile = tile;
        }

        if raid.phase != RaidPhase::Egress {
            if raid.members.len() < raid.committed_size
                || raid.members.iter().any(|id| {
                    own_unit(obs, *id).is_some_and(|unit| {
                        unit.hp.saturating_mul(100)
                            < unit
                                .kind
                                .stats()
                                .max_hp
                                .saturating_mul(withdrawal_health_percent(profile))
                    })
                })
            {
                withdraw(&mut raid, RaidExitReason::LossBudget, obs.tick);
            } else if obs.tick.saturating_sub(raid.started_at) >= timeout(profile) {
                withdraw(&mut raid, RaidExitReason::Timeout, obs.tick);
            } else if visible_response(obs, raid.last_tile, raid.members.len()) {
                withdraw(&mut raid, RaidExitReason::EnemyResponse, obs.tick);
            } else if current.is_none() {
                let reason = if obs.visible(raid.last_tile) {
                    RaidExitReason::Complete
                } else {
                    RaidExitReason::LostContact
                };
                withdraw(&mut raid, reason, obs.tick);
            } else if !routes.group_reaches_command_goal(&raid.members, raid.last_tile) {
                withdraw(&mut raid, RaidExitReason::Unreachable, obs.tick);
            }
        }

        let mut decision = StrategicDecision::default();
        let already_home = raid
            .members
            .iter()
            .all(|id| own_unit(obs, *id).is_some_and(|unit| unit.tile.chebyshev(home) <= 2));
        let return_route = raid.phase != RaidPhase::Egress
            || raid.members.is_empty()
            || already_home
            || routes.group_reaches_command_goal(&raid.members, home);
        match raid.phase {
            RaidPhase::Ingress => {
                let needs_dispatch = match raid.dispatch {
                    Some(RaidDispatch::Ingress(goal)) => goal.chebyshev(raid.last_tile) > 4,
                    _ => true,
                };
                if needs_dispatch {
                    decision.intents.push(Intent::AttackMoveUnits {
                        units: raid.members.clone(),
                        goal: raid.last_tile,
                    });
                    raid.dispatch = Some(RaidDispatch::Ingress(raid.last_tile));
                }
                let close = raid.members.iter().all(|id| {
                    own_unit(obs, *id).is_some_and(|unit| unit.tile.chebyshev(raid.last_tile) <= 4)
                });
                if close
                    && obs.tick.saturating_sub(raid.phase_started_at)
                        >= tuning.reaction_delay + tuning.commitment_hesitation
                {
                    raid.phase = RaidPhase::Strike;
                    raid.phase_started_at = obs.tick;
                }
            }
            RaidPhase::Strike => {
                if let Some((target, _)) = current {
                    let dispatch = RaidDispatch::Strike(target);
                    if raid.dispatch != Some(dispatch) {
                        decision.intents.push(Intent::AttackUnits {
                            units: raid.members.clone(),
                            target,
                        });
                        raid.dispatch = Some(dispatch);
                    }
                }
            }
            RaidPhase::Egress => {
                if !raid.members.is_empty() && !already_home && return_route {
                    let dispatch = RaidDispatch::Egress(home);
                    if raid.dispatch != Some(dispatch) {
                        decision.intents.push(Intent::MoveUnits {
                            units: raid.members.clone(),
                            goal: home,
                        });
                        raid.dispatch = Some(dispatch);
                    }
                }
            }
        }
        decision.reservations = raid.members.clone();

        let home_safe = raid.phase == RaidPhase::Egress
            && (raid.members.is_empty()
                || already_home
                || !return_route
                || obs.tick.saturating_sub(raid.phase_started_at) >= 500);
        if home_safe {
            self.cooldown_until = obs.tick.saturating_add(cooldown(profile, tuning));
        } else {
            self.active = Some(raid);
        }
        decision
    }

    fn refresh_muster(
        &mut self,
        obs: &Observation,
        enlisted: &[UnitId],
        additionally_reserved: &[UnitId],
    ) {
        self.muster.retain(|id| {
            own_unit(obs, *id).is_some_and(|unit| unit.kind == UnitKind::Scuttler)
                && !enlisted.contains(id)
                && !additionally_reserved.contains(id)
        });
        let mut available: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| {
                unit.kind == UnitKind::Scuttler
                    && unit.idle
                    && !self.muster.contains(&unit.id)
                    && !enlisted.contains(&unit.id)
                    && !additionally_reserved.contains(&unit.id)
            })
            .map(|unit| unit.id)
            .collect();
        available.sort_unstable();
        self.muster.extend(
            available
                .into_iter()
                .take(RAID_GROUP_SIZE.saturating_sub(self.muster.len())),
        );
        self.muster.sort_unstable();
    }
}

fn begin(
    obs: &Observation,
    muster: &[UnitId],
    routes: &mut RouteProjection<'_>,
) -> Option<RaidOperation> {
    if muster.len() != RAID_GROUP_SIZE {
        return None;
    }
    let (target_player, objective, last_tile, members) = target_candidates(obs)
        .into_iter()
        .find_map(|(player, objective, tile)| {
            first_reachable_group(routes, muster, RAID_GROUP_SIZE, tile)
                .map(|members| (player, objective, tile, members))
        })?;
    Some(RaidOperation {
        target_player,
        objective,
        last_tile,
        members,
        committed_size: RAID_GROUP_SIZE,
        phase: RaidPhase::Ingress,
        started_at: obs.tick,
        phase_started_at: obs.tick,
        exit_reason: None,
        dispatch: None,
    })
}

fn home_screen_ready(
    profile: &ResolvedProfile,
    obs: &Observation,
    home: TilePos,
    muster: &[UnitId],
) -> bool {
    let sentinel = UnitObs {
        id: UnitId(u32::MAX),
        player: obs.me,
        kind: UnitKind::Sentinel,
        tile: home,
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
    };
    let required =
        super::executive::unit_strength(&sentinel).saturating_mul(home_screen_equivalents(profile));
    let available = obs
        .my_units
        .iter()
        .filter(|unit| !muster.contains(&unit.id))
        .filter(|unit| unit.tile.chebyshev(home) <= HOME_SCREEN_RADIUS)
        .filter(|unit| {
            let stats = unit.kind.stats();
            stats.domain == Domain::Ground && stats.can_fight()
        })
        .map(super::executive::unit_strength)
        .sum::<u64>();
    available >= required
}

fn home_screen_equivalents(profile: &ResolvedProfile) -> u64 {
    let guile_floor = 3u64.saturating_sub(u64::from(profile.traits.guile) / 34);
    match profile.stance {
        BotStance::Turtle => guile_floor.saturating_add(1),
        BotStance::Balanced => guile_floor,
        BotStance::Aggressive => guile_floor.saturating_sub(1).max(1),
    }
}

fn withdrawal_health_percent(profile: &ResolvedProfile) -> u32 {
    35 + u32::from(profile.traits.guile) / 4
}

fn target_candidates(obs: &Observation) -> Vec<(PlayerId, RaidObjective, TilePos)> {
    let mut candidates = Vec::new();
    for unit in &obs.enemy_units {
        let priority = match unit.kind {
            UnitKind::Excavator => 0,
            UnitKind::Harvester => 1,
            _ => continue,
        };
        candidates.push((
            defenders(obs, unit.tile),
            priority,
            unit.tile.y,
            unit.tile.x,
            unit.player,
            RaidObjective::Unit {
                id: unit.id,
                kind: unit.kind,
            },
            unit.tile,
        ));
    }
    for building in obs.enemy_buildings.iter().filter(|building| building.seen) {
        let priority = if !building.built
            && building
                .kind
                .base_stats()
                .construction
                .is_some_and(|construction| construction.cost >= 100)
        {
            0
        } else {
            match building.kind {
                BuildingKind::Extractor => 2,
                BuildingKind::Reclaimer => 3,
                BuildingKind::Airworks | BuildingKind::Crucible => 4,
                BuildingKind::RepairBay | BuildingKind::Array => 5,
                BuildingKind::Fabricator => 6,
                BuildingKind::Foundry
                | BuildingKind::Turret
                | BuildingKind::FlakTurret
                | BuildingKind::Bastion
                | BuildingKind::Barricade
                | BuildingKind::ScuttleCharge => continue,
            }
        };
        candidates.push((
            defenders(obs, building.anchor),
            priority,
            building.anchor.y,
            building.anchor.x,
            building.player,
            RaidObjective::Building {
                id: building.id,
                kind: building.kind,
            },
            building.anchor,
        ));
    }
    candidates.sort_by_key(|candidate| {
        (
            candidate.0,
            candidate.1,
            candidate.2,
            candidate.3,
            candidate.4,
            candidate.5,
        )
    });
    candidates
        .into_iter()
        .map(|(_, _, _, _, player, objective, tile)| (player, objective, tile))
        .collect()
}

fn current_objective(obs: &Observation, raid: &RaidOperation) -> Option<(Target, TilePos)> {
    match raid.objective {
        RaidObjective::Unit { id, kind } => obs
            .enemy_units
            .iter()
            .find(|unit| unit.id == id && unit.player == raid.target_player && unit.kind == kind)
            .map(|unit| (Target::Unit(id), unit.tile)),
        RaidObjective::Building { id, kind } => obs
            .enemy_buildings
            .iter()
            .find(|building| {
                building.seen
                    && building.id == id
                    && building.player == raid.target_player
                    && building.kind == kind
            })
            .map(|building| (Target::Building(id), building.anchor)),
    }
}

fn defenders(obs: &Observation, tile: TilePos) -> usize {
    obs.enemy_units
        .iter()
        .filter(|unit| {
            unit.kind.stats().can_fight()
                && unit.kind.stats().can_target(Domain::Ground)
                && unit.tile.chebyshev(tile) <= 5
        })
        .count()
        + obs
            .enemy_buildings
            .iter()
            .filter(|building| {
                building.seen
                    && building.built
                    && building.kind.base_stats().weapons.iter().any(|weapon| {
                        weapon.targets.covers(Domain::Ground)
                            && building.anchor.chebyshev(tile)
                                <= weapon.range.ceil().to_num::<i32>().saturating_add(2)
                    })
            })
            .count()
}

fn visible_response(obs: &Observation, tile: TilePos, raiders: usize) -> bool {
    defenders(obs, tile) >= raiders.max(1)
}

fn own_unit(obs: &Observation, id: UnitId) -> Option<&UnitObs> {
    obs.my_units
        .binary_search_by_key(&id, |unit| unit.id)
        .ok()
        .map(|index| &obs.my_units[index])
}

fn withdraw(raid: &mut RaidOperation, reason: RaidExitReason, now: Tick) {
    raid.phase = RaidPhase::Egress;
    raid.phase_started_at = now;
    raid.exit_reason = Some(reason);
}

fn timeout(profile: &ResolvedProfile) -> Tick {
    let stance = match profile.stance {
        BotStance::Turtle => 600,
        BotStance::Balanced => 760,
        BotStance::Aggressive => 920,
    };
    stance + u64::from(profile.traits.guile) * 2
}

fn cooldown(profile: &ResolvedProfile, tuning: DifficultyTuning) -> Tick {
    let stance = match profile.stance {
        BotStance::Turtle => 900,
        BotStance::Balanced => 700,
        BotStance::Aggressive => 520,
    };
    stance + u64::from(100 - profile.traits.guile) * 5 + tuning.commitment_hesitation
}

#[cfg(test)]
mod tests {
    use super::super::observation::{BuildingObs, OBSERVATION_VERSION};
    use super::super::profile::{PersonalityTraits, Specialty};
    use super::*;
    use crate::scenario::{BotConfig, BotDifficulty};
    use crate::state::Faction;

    const HOME: TilePos = TilePos::new(2, 8);
    const TARGET: TilePos = TilePos::new(18, 8);

    fn profile(guile: u8) -> ResolvedProfile {
        ResolvedProfile {
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Balanced,
            personality_seed: 7,
            primary: Specialty::Guile,
            secondary: Specialty::Air,
            traits: PersonalityTraits {
                air: 45,
                siege: 45,
                support: 40,
                fortification: 35,
                greed: 50,
                guile,
            },
        }
    }

    fn profile_with_stance(guile: u8, stance: BotStance) -> ResolvedProfile {
        ResolvedProfile {
            stance,
            ..profile(guile)
        }
    }

    fn unit(id: u32, player: u8, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(player),
            kind,
            tile,
            hp: kind.stats().max_hp,
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

    fn building(
        id: u32,
        player: u8,
        kind: BuildingKind,
        anchor: TilePos,
        built: bool,
    ) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(player),
            kind,
            anchor,
            hp: kind.tier_stats(0).max_hp,
            built,
            seen: true,
            tier: 0,
        }
    }

    fn observation(tick: Tick) -> Observation {
        let tick = super::super::difficulty::strategic_admission_at_or_after(tick);
        let mut visible = vec![false; 24 * 16];
        visible[(TARGET.y as usize) * 24 + TARGET.x as usize] = true;
        Observation {
            version: OBSERVATION_VERSION,
            tick,
            me: PlayerId(0),
            scrap: 0,
            map_width: 24,
            map_height: 16,
            my_units: (1..=2)
                .map(|id| unit(id, 0, UnitKind::Scuttler, HOME))
                .chain((10..=13).map(|id| unit(id, 0, UnitKind::Sentinel, HOME)))
                .collect(),
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: vec![unit(80, 1, UnitKind::Harvester, TARGET)],
            enemy_buildings: Vec::new(),
            visible,
            explored: vec![true; 24 * 16],
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

    #[test]
    fn high_guile_commits_the_same_exact_pair_and_leaves_the_screen_home() {
        let obs = observation(200);
        let mut planner = RaidPlanner::new();

        let decision = planner.think(
            &profile(80),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            HOME,
            &[],
            &[],
        );

        assert_eq!(decision.reservations, [UnitId(1), UnitId(2)]);
        assert_eq!(
            decision.intents,
            [Intent::AttackMoveUnits {
                units: decision.reservations.clone(),
                goal: TARGET,
            }]
        );
        assert_eq!(planner.operation().unwrap().committed_size, RAID_GROUP_SIZE);
        assert!(obs.my_units[2..].iter().all(|unit| {
            unit.kind == UnitKind::Sentinel && !decision.reservations.contains(&unit.id)
        }));
    }

    #[test]
    fn partial_muster_is_reserved_until_the_exact_pair_exists() {
        let mut obs = observation(200);
        obs.my_units.retain(|unit| unit.id != UnitId(2));
        let mut planner = RaidPlanner::new();

        let partial = planner.think(
            &profile(80),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            HOME,
            &[],
            &[],
        );
        assert_eq!(partial.reservations, [UnitId(1)]);
        assert!(partial.intents.is_empty());
        assert!(planner.operation().is_none());

        obs.my_units
            .push(unit(2, 0, UnitKind::Scuttler, HOME.offset(1, 0)));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick = super::super::difficulty::next_strategic_admission_tick(obs.tick);
        let complete = planner.think(
            &profile(80),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            HOME,
            &[],
            &[],
        );

        assert_eq!(complete.reservations, [UnitId(1), UnitId(2)]);
        assert!(matches!(
            complete.intents.as_slice(),
            [Intent::AttackMoveUnits { units, goal }]
                if units == &complete.reservations && *goal == TARGET
        ));
        assert!(planner.operation().is_some());
    }

    #[test]
    fn muster_waits_for_the_guile_and_stance_home_strength_floor() {
        let mut obs = observation(200);
        obs.my_units.retain(|unit| unit.kind == UnitKind::Scuttler);
        let turtle = profile_with_stance(0, BotStance::Turtle);
        let mut planner = RaidPlanner::new();

        let waiting = planner.think(
            &turtle,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            HOME,
            &[],
            &[],
        );

        assert_eq!(waiting.reservations, [UnitId(1), UnitId(2)]);
        assert!(waiting.intents.is_empty());
        assert!(planner.operation().is_none());

        obs.my_units
            .extend((10..=13).map(|id| unit(id, 0, UnitKind::Sentinel, HOME)));
        obs.tick = super::super::difficulty::next_strategic_admission_tick(obs.tick);
        let ready = planner.think(
            &turtle,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            HOME,
            &[],
            &[],
        );
        assert!(matches!(
            ready.intents.as_slice(),
            [Intent::AttackMoveUnits { units, .. }] if units == &[UnitId(1), UnitId(2)]
        ));
    }

    #[test]
    fn raid_retargets_only_after_the_objective_moves_materially() {
        let mut obs = observation(200);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut planner = RaidPlanner::new();

        let first = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        assert_eq!(first.intents.len(), 1);

        obs.tick += 1;
        obs.enemy_units[0].tile = TARGET.offset(4, 0);
        let nearby = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        assert!(nearby.intents.is_empty());

        obs.tick += 1;
        obs.enemy_units[0].tile = TARGET.offset(5, 0);
        let moved = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        assert_eq!(
            moved.intents,
            [Intent::AttackMoveUnits {
                units: moved.reservations.clone(),
                goal: TARGET.offset(5, 0),
            }]
        );

        obs.tick += 1;
        let stable = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        assert!(stable.intents.is_empty());
    }

    #[test]
    fn raid_strike_and_egress_orders_are_each_dispatched_once() {
        let mut obs = observation(200);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut planner = RaidPlanner::new();
        planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);

        for unit in &mut obs.my_units {
            unit.tile = TARGET.offset(-3, 0);
        }
        obs.tick += tuning.reaction_delay + tuning.commitment_hesitation;
        let arrival = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        assert!(arrival.intents.is_empty());
        assert_eq!(planner.operation().unwrap().phase, RaidPhase::Strike);

        obs.tick += 1;
        let strike = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        assert_eq!(
            strike.intents,
            [Intent::AttackUnits {
                units: strike.reservations.clone(),
                target: Target::Unit(UnitId(80)),
            }]
        );
        obs.tick += 1;
        assert!(
            planner
                .think(&profile(80), tuning, &obs, HOME, &[], &[])
                .intents
                .is_empty()
        );

        obs.enemy_units
            .extend((81..=83).map(|id| unit(id, 1, UnitKind::Sentinel, TARGET)));
        obs.tick += 1;
        let egress = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        assert!(matches!(
            egress.intents.as_slice(),
            [Intent::MoveUnits { goal, .. }] if *goal == HOME
        ));
        obs.tick += 1;
        assert!(
            planner
                .think(&profile(80), tuning, &obs, HOME, &[], &[])
                .intents
                .is_empty()
        );
    }

    #[test]
    fn every_guile_band_can_launch_the_same_pair_behind_its_home_screen() {
        let obs = observation(200);

        for guile in [0, 40, 68, 80, 100] {
            let mut planner = RaidPlanner::new();
            let decision = planner.think(
                &profile(guile),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &obs,
                HOME,
                &[],
                &[],
            );
            assert_eq!(
                decision.reservations,
                [UnitId(1), UnitId(2)],
                "guile {guile}"
            );
            assert!(
                matches!(decision.intents.as_slice(), [Intent::AttackMoveUnits { units, .. }] if units == &[UnitId(1), UnitId(2)]),
                "guile {guile}: {decision:?}"
            );
            assert_eq!(planner.operation().unwrap().committed_size, RAID_GROUP_SIZE);
        }
    }

    #[test]
    fn every_difficulty_waits_for_the_exact_pair_before_focus_firing() {
        let mut focus_ticks = Vec::new();
        let mut frozen_pairs = Vec::new();
        for difficulty in BotDifficulty::ALL {
            let tuning = DifficultyTuning::for_level(difficulty);
            let identity =
                BotConfig::scripted(difficulty, BotStance::Balanced, 20_024).resolve_profile();
            assert_eq!(identity.primary, Specialty::Guile);
            assert_eq!(identity.traits.guile, 74);
            let mut obs = observation(200);
            let mut planner = RaidPlanner::new();

            planner.think(&identity, tuning, &obs, HOME, &[], &[]);
            let pair = planner.operation().unwrap().members.clone();
            assert_eq!(pair, [UnitId(1), UnitId(2)]);
            frozen_pairs.push(pair);

            let mut focus_tick = None;
            let mut tick = obs.tick.saturating_add(tuning.cadence);
            while tick <= 600 {
                obs.tick = tick;
                if tick >= 240 {
                    obs.my_units[0].tile = TARGET.offset(-2, 0);
                }
                if tick >= 264 {
                    obs.my_units[1].tile = TARGET.offset(-1, 0);
                }
                let decision = planner.think(&identity, tuning, &obs, HOME, &[], &[]);
                let focused = decision
                    .intents
                    .iter()
                    .any(|intent| matches!(intent, Intent::AttackUnits { .. }));
                if tick < 264 {
                    assert!(
                        !focused,
                        "{difficulty:?} focused before exact-pair cohesion"
                    );
                }
                if focused {
                    focus_tick = Some(tick);
                    break;
                }
                tick = tick.saturating_add(tuning.cadence);
            }
            focus_ticks.push(focus_tick.expect("every rung eventually focuses the objective"));
        }

        assert!(frozen_pairs.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(focus_ticks.windows(2).all(|pair| pair[0] >= pair[1]));
    }

    #[test]
    fn visible_response_causes_immediate_group_withdrawal() {
        let mut obs = observation(200);
        let mut planner = RaidPlanner::new();
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        obs.tick += 6;
        for unit in &mut obs.my_units {
            unit.tile = TilePos::new(12, 8);
        }
        obs.enemy_units
            .extend((81..=83).map(|id| unit(id, 1, UnitKind::Sentinel, TARGET)));

        let decision = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);

        assert_eq!(
            planner.operation().unwrap().exit_reason,
            Some(RaidExitReason::EnemyResponse)
        );
        assert_eq!(
            decision.intents,
            [Intent::MoveUnits {
                units: vec![UnitId(1), UnitId(2)],
                goal: HOME,
            }]
        );
    }

    #[test]
    fn high_guile_preserves_the_same_pair_at_a_health_low_guile_accepts() {
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut low_obs = observation(200);
        let mut high_obs = low_obs.clone();
        let low = profile(20);
        let high = profile(80);
        let mut low_planner = RaidPlanner::new();
        let mut high_planner = RaidPlanner::new();
        low_planner.think(&low, tuning, &low_obs, HOME, &[], &[]);
        high_planner.think(&high, tuning, &high_obs, HOME, &[], &[]);

        for obs in [&mut low_obs, &mut high_obs] {
            obs.tick += 1;
            obs.my_units[0].hp = 21;
            obs.my_units[0].tile = TilePos::new(12, 8);
            obs.my_units[1].tile = TilePos::new(12, 9);
        }
        let low_decision = low_planner.think(&low, tuning, &low_obs, HOME, &[], &[]);
        let high_decision = high_planner.think(&high, tuning, &high_obs, HOME, &[], &[]);

        assert_eq!(low_planner.operation().unwrap().phase, RaidPhase::Ingress);
        assert!(low_decision.intents.is_empty());
        assert_eq!(
            high_planner.operation().unwrap().exit_reason,
            Some(RaidExitReason::LossBudget)
        );
        assert!(matches!(
            high_decision.intents.as_slice(),
            [Intent::MoveUnits { units, goal }]
                if units == &[UnitId(1), UnitId(2)] && *goal == HOME
        ));
        assert!(withdrawal_health_percent(&high) > withdrawal_health_percent(&low));
    }

    #[test]
    fn resolved_guile_personalities_both_raid_but_preserve_the_pair_differently() {
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let low =
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 59).resolve_profile();
        let high =
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 12).resolve_profile();
        assert_eq!(low.traits.guile, 33);
        assert_eq!(high.traits.guile, 84);

        let mut low_obs = observation(200);
        let mut high_obs = low_obs.clone();
        let mut low_planner = RaidPlanner::new();
        let mut high_planner = RaidPlanner::new();
        let low_launch = low_planner.think(&low, tuning, &low_obs, HOME, &[], &[]);
        let high_launch = high_planner.think(&high, tuning, &high_obs, HOME, &[], &[]);
        for launch in [&low_launch, &high_launch] {
            assert_eq!(launch.reservations, [UnitId(1), UnitId(2)]);
            assert!(matches!(
                launch.intents.as_slice(),
                [Intent::AttackMoveUnits { units, goal }]
                    if units == &[UnitId(1), UnitId(2)] && *goal == TARGET
            ));
        }

        let low_threshold = withdrawal_health_percent(&low);
        let high_threshold = withdrawal_health_percent(&high);
        assert!(high_threshold > low_threshold);
        let max_hp = UnitKind::Scuttler.stats().max_hp;
        let boundary_hp = max_hp.saturating_mul(low_threshold).saturating_add(99) / 100;
        assert!(boundary_hp.saturating_mul(100) >= max_hp.saturating_mul(low_threshold));
        assert!(boundary_hp.saturating_mul(100) < max_hp.saturating_mul(high_threshold));
        for obs in [&mut low_obs, &mut high_obs] {
            obs.tick += 1;
            obs.my_units[0].hp = boundary_hp;
            obs.my_units[0].tile = TilePos::new(12, 8);
            obs.my_units[1].tile = TilePos::new(12, 9);
        }

        let low_decision = low_planner.think(&low, tuning, &low_obs, HOME, &[], &[]);
        let high_decision = high_planner.think(&high, tuning, &high_obs, HOME, &[], &[]);
        assert_eq!(low_planner.operation().unwrap().phase, RaidPhase::Ingress);
        assert!(low_decision.intents.is_empty());
        assert_eq!(
            high_planner.operation().unwrap().exit_reason,
            Some(RaidExitReason::LossBudget)
        );
        assert!(matches!(
            high_decision.intents.as_slice(),
            [Intent::MoveUnits { units, goal }]
                if units == &[UnitId(1), UnitId(2)] && *goal == HOME
        ));
    }

    #[test]
    fn high_guile_rearms_the_same_pair_sooner_than_low_guile() {
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let remaining = |profile: &ResolvedProfile| {
            let mut obs = observation(200);
            let mut planner = RaidPlanner::new();
            planner.think(profile, tuning, &obs, HOME, &[], &[]);
            obs.tick += 1;
            obs.enemy_units.clear();
            planner.think(profile, tuning, &obs, HOME, &[], &[]);
            planner.cooldown_until().saturating_sub(obs.tick)
        };

        let low = profile(20);
        let high = profile(80);
        assert_eq!(remaining(&low), cooldown(&low, tuning));
        assert_eq!(remaining(&high), cooldown(&high, tuning));
        assert!(remaining(&high) < remaining(&low));
    }

    #[test]
    fn disappearance_in_fog_is_lost_contact_not_success() {
        let mut obs = observation(200);
        let mut planner = RaidPlanner::new();
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        obs.tick += 6;
        for unit in &mut obs.my_units {
            unit.tile = TilePos::new(12, 8);
        }
        obs.enemy_units.clear();
        obs.visible.fill(false);

        planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);

        assert_eq!(
            planner.operation().unwrap().exit_reason,
            Some(RaidExitReason::LostContact)
        );
    }

    #[test]
    fn empty_visible_objective_is_confirmed_complete() {
        let mut obs = observation(200);
        let mut planner = RaidPlanner::new();
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        obs.tick += 6;
        for unit in &mut obs.my_units {
            unit.tile = TilePos::new(12, 8);
        }
        obs.enemy_units.clear();

        planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);

        assert_eq!(
            planner.operation().unwrap().exit_reason,
            Some(RaidExitReason::Complete)
        );
    }

    #[test]
    fn known_island_target_is_refused_but_a_mapped_gap_allows_the_raid() {
        let mut obs = observation(200);
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(10, y)).collect();
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut planner = RaidPlanner::new();

        assert_eq!(
            planner
                .think(&profile(80), tuning, &obs, HOME, &[], &[])
                .intents,
            []
        );
        assert!(planner.operation().is_none());
        assert_eq!(planner.reservations(), [UnitId(1), UnitId(2)]);

        obs.known_rock.retain(|tile| tile.y != TARGET.y);
        let decision = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        assert_eq!(
            decision.intents,
            [Intent::AttackMoveUnits {
                units: vec![UnitId(1), UnitId(2)],
                goal: TARGET,
            }]
        );
    }

    #[test]
    fn raid_chooses_a_reachable_economy_target_over_a_better_island_target() {
        let mut obs = observation(200);
        obs.enemy_units = vec![
            unit(79, 1, UnitKind::Harvester, TilePos::new(7, 8)),
            unit(80, 1, UnitKind::Excavator, TARGET),
        ];
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(10, y)).collect();

        let decision = RaidPlanner::new().think(
            &profile(80),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            HOME,
            &[],
            &[],
        );

        assert!(matches!(
            decision.intents.as_slice(),
            [Intent::AttackMoveUnits { goal, .. }] if *goal == TilePos::new(7, 8)
        ));
    }

    #[test]
    fn newly_mapped_severance_ends_the_raid_without_a_route_storm() {
        let mut obs = observation(200);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut planner = RaidPlanner::new();
        planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);

        obs.tick += 1;
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(10, y)).collect();
        let decision = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);

        assert!(decision.intents.is_empty());
        assert!(planner.operation().is_none());
        assert!(planner.cooldown_until() > obs.tick);
    }

    #[test]
    fn losing_one_committed_raider_sends_the_survivors_home() {
        let mut obs = observation(200);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut planner = RaidPlanner::new();
        planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);

        obs.tick += 1;
        obs.my_units.retain(|unit| unit.id != UnitId(2));
        for unit in &mut obs.my_units {
            unit.tile = TilePos::new(12, 8);
        }
        let decision = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);

        assert_eq!(
            planner.operation().unwrap().exit_reason,
            Some(RaidExitReason::LossBudget)
        );
        assert_eq!(decision.reservations, [UnitId(1)]);
        assert_eq!(
            decision.intents,
            [Intent::MoveUnits {
                units: vec![UnitId(1)],
                goal: HOME,
            }]
        );
    }

    #[test]
    fn a_raid_times_out_at_its_stance_specific_deadline() {
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let turtle = profile_with_stance(80, BotStance::Turtle);
        let aggressive = profile_with_stance(80, BotStance::Aggressive);
        let mut turtle_obs = observation(200);
        let mut aggressive_obs = turtle_obs.clone();
        let mut turtle_planner = RaidPlanner::new();
        let mut aggressive_planner = RaidPlanner::new();
        turtle_planner.think(&turtle, tuning, &turtle_obs, HOME, &[], &[]);
        aggressive_planner.think(&aggressive, tuning, &aggressive_obs, HOME, &[], &[]);

        let turtle_deadline = turtle_obs.tick + timeout(&turtle);
        turtle_obs.tick = turtle_deadline;
        aggressive_obs.tick = turtle_deadline;
        for unit in &mut turtle_obs.my_units {
            unit.tile = TilePos::new(12, 8);
        }
        for unit in &mut aggressive_obs.my_units {
            unit.tile = TilePos::new(12, 8);
        }

        let turtle_decision = turtle_planner.think(&turtle, tuning, &turtle_obs, HOME, &[], &[]);
        let aggressive_decision =
            aggressive_planner.think(&aggressive, tuning, &aggressive_obs, HOME, &[], &[]);

        assert_eq!(
            turtle_planner.operation().unwrap().exit_reason,
            Some(RaidExitReason::Timeout)
        );
        assert!(matches!(
            turtle_decision.intents.as_slice(),
            [Intent::MoveUnits { goal, .. }] if *goal == HOME
        ));
        assert_eq!(
            aggressive_planner.operation().unwrap().phase,
            RaidPhase::Ingress
        );
        assert!(aggressive_decision.intents.is_empty());
        assert!(timeout(&aggressive) > timeout(&turtle));
    }

    #[test]
    fn raid_locks_onto_unfinished_capital_and_tracks_the_building_by_id() {
        let mut obs = observation(200);
        obs.enemy_units.clear();
        obs.enemy_buildings = vec![
            building(80, 1, BuildingKind::Extractor, TilePos::new(10, 4), true),
            building(81, 1, BuildingKind::Airworks, TARGET, false),
        ];
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut planner = RaidPlanner::new();

        let ingress = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        assert_eq!(
            planner.operation().unwrap().objective,
            RaidObjective::Building {
                id: BuildingId(81),
                kind: BuildingKind::Airworks,
            }
        );
        assert!(matches!(
            ingress.intents.as_slice(),
            [Intent::AttackMoveUnits { goal, .. }] if *goal == TARGET
        ));

        obs.enemy_buildings.push(building(
            79,
            1,
            BuildingKind::Crucible,
            TilePos::new(8, 12),
            false,
        ));
        for unit in &mut obs.my_units {
            unit.tile = TARGET.offset(-3, 0);
        }
        obs.tick += 1;
        planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);
        obs.tick += 1;
        let strike = planner.think(&profile(80), tuning, &obs, HOME, &[], &[]);

        assert_eq!(
            strike.intents,
            [Intent::AttackUnits {
                units: strike.reservations.clone(),
                target: Target::Building(BuildingId(81)),
            }]
        );
    }

    #[test]
    fn static_defense_redirects_a_raid_to_an_exposed_economy_target() {
        let mut obs = observation(200);
        obs.enemy_units.clear();
        let exposed = TilePos::new(8, 3);
        obs.enemy_buildings = vec![
            building(80, 1, BuildingKind::Airworks, TARGET, false),
            building(81, 1, BuildingKind::Turret, TARGET.offset(-2, 0), true),
            building(82, 1, BuildingKind::Reclaimer, exposed, true),
        ];

        let decision = RaidPlanner::new().think(
            &profile(80),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            HOME,
            &[],
            &[],
        );

        assert!(matches!(
            decision.intents.as_slice(),
            [Intent::AttackMoveUnits { goal, .. }] if *goal == exposed
        ));
    }

    #[test]
    fn raids_ignore_combat_units_and_prefer_production_over_repair_support() {
        let mut obs = observation(200);
        obs.enemy_units = vec![unit(80, 1, UnitKind::Sentinel, TilePos::new(6, 8))];
        let production = TilePos::new(14, 4);
        obs.enemy_buildings = vec![
            building(81, 1, BuildingKind::RepairBay, TilePos::new(8, 3), true),
            building(82, 1, BuildingKind::Airworks, production, true),
        ];

        let decision = RaidPlanner::new().think(
            &profile(80),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            HOME,
            &[],
            &[],
        );

        assert!(matches!(
            decision.intents.as_slice(),
            [Intent::AttackMoveUnits { goal, .. }] if *goal == production
        ));
    }

    #[test]
    fn an_exposed_fabricator_is_a_valid_last_resort_raid_target() {
        let mut obs = observation(200);
        obs.enemy_units.clear();
        obs.enemy_buildings = vec![building(80, 1, BuildingKind::Fabricator, TARGET, true)];

        let decision = RaidPlanner::new().think(
            &profile(80),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &obs,
            HOME,
            &[],
            &[],
        );

        assert!(matches!(
            decision.intents.as_slice(),
            [Intent::AttackMoveUnits { goal, .. }] if *goal == TARGET
        ));
    }

    #[test]
    fn aggressive_raids_rearm_sooner_after_a_completed_harassment() {
        let tuning = DifficultyTuning::for_level(BotDifficulty::Standard);
        let turtle = profile_with_stance(80, BotStance::Turtle);
        let aggressive = profile_with_stance(80, BotStance::Aggressive);

        let completion_cooldown = |profile: &ResolvedProfile| {
            let mut obs = observation(200);
            let mut planner = RaidPlanner::new();
            planner.think(profile, tuning, &obs, HOME, &[], &[]);
            obs.tick += 1;
            obs.enemy_units.clear();
            planner.think(profile, tuning, &obs, HOME, &[], &[]);
            planner.cooldown_until().saturating_sub(obs.tick)
        };

        assert_eq!(completion_cooldown(&turtle), cooldown(&turtle, tuning));
        assert_eq!(
            completion_cooldown(&aggressive),
            cooldown(&aggressive, tuning)
        );
        assert!(completion_cooldown(&aggressive) < completion_cooldown(&turtle));
    }
}
