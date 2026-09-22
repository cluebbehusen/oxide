use super::*;
use crate::executive::{ArmyId, ArmyMission, ArmyObjective, ArmyPurpose};
use crate::experience::{Doctrine, ExperienceKey, ExperienceSubject};
use crate::query_work::QueryPurpose;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GroundMissionInputs<'a> {
    pub(crate) missions: &'a [(ArmyId, ArmyMission)],
    pub(crate) unavailable: &'a [UnitId],
    pub(crate) enlisted: &'a [UnitId],
    pub(crate) tuning: DifficultyTuning,
    pub(crate) relief: Option<(BuildingId, &'a [UnitId])>,
}

#[derive(Clone, Copy)]
pub(super) struct MissionContext<'a> {
    pub(super) dials: &'a Dials,
    pub(super) home: TilePos,
    pub(super) mode: PolicyMode<'a>,
    pub(super) inputs: GroundMissionInputs<'a>,
}

#[derive(Default)]
struct MissionAssignments<'a> {
    assigned: BTreeSet<ArmyId>,
    away_from_home: BTreeSet<ArmyId>,
    departing_members: BTreeSet<UnitId>,
    fresh: usize,
    routes: Option<crate::navigation::commands::RouteProjection<'a>>,
}

impl<'a> GroundMissionInputs<'a> {
    fn prior(self, id: ArmyId) -> Option<&'a ArmyMission> {
        self.missions
            .iter()
            .find(|(army, _)| *army == id)
            .map(|(_, mission)| mission)
    }
}

struct MissionArmy<'a> {
    body: std::borrow::Cow<'a, Army>,
    center: TilePos,
    ground_capable: usize,
    strength: u64,
}

impl<'a> MissionArmy<'a> {
    fn eligible(&self, inputs: GroundMissionInputs<'_>) -> bool {
        self.body.state != ArmyState::Withdrawing
            && self
                .body
                .members
                .iter()
                .all(|id| !inputs.unavailable.contains(id))
    }

    fn prepare(army: &'a Army, obs: &Observation, unavailable: &[UnitId]) -> Option<Self> {
        let mut body = std::borrow::Cow::Borrowed(army);
        if army.state == ArmyState::Staging
            && army.members.iter().any(|id| unavailable.contains(id))
        {
            body.to_mut().members.retain(|id| !unavailable.contains(id));
        }
        if body.members.is_empty() {
            return None;
        }
        let (mut x, mut y, mut count, mut ground_capable) = (0_i64, 0_i64, 0_i32, 0);
        for unit in obs
            .my_units
            .iter()
            .filter(|unit| body.members.contains(&unit.id))
        {
            x += i64::from(unit.tile.x);
            y += i64::from(unit.tile.y);
            count += 1;
            ground_capable += usize::from(crate::executive::unit_strength(unit) > 0);
        }
        let center = if count == 0 {
            body.staging
        } else {
            TilePos::new(x as i32 / count, y as i32 / count)
        };
        let strength = crate::executive::marching_strength(&body, obs);
        Some(Self {
            body,
            center,
            ground_capable,
            strength,
        })
    }
}

impl UtilityPolicy {
    pub(super) fn approach_defense_strength(
        &self,
        obs: &Observation,
        army: &Army,
        goal: TilePos,
        mode: PolicyMode<'_>,
    ) -> Option<u64> {
        let defenses: Vec<_> = obs
            .enemy_buildings
            .iter()
            .filter(|building| objective_building_strength(building, mode, obs.tick) > 0)
            .collect();
        if defenses.is_empty() {
            return Some(0);
        }
        let leader = obs
            .my_units
            .iter()
            .filter(|unit| unit.hp > 0 && army.members.contains(&unit.id))
            .min_by_key(|unit| unit.id)?;
        let destination = *self.ground_attack_goals(obs, goal, 1)?.first()?;
        let routes = mode.public_map.map_or_else(
            || {
                crate::navigation::commands::RouteProjection::known_ground(
                    QueryPurpose::GroundTargetSelection,
                    obs,
                )
            },
            |map| {
                crate::navigation::commands::RouteProjection::with_public_terrain(
                    QueryPurpose::GroundTargetSelection,
                    obs,
                    Domain::Ground,
                    map,
                )
            },
        );
        let path = routes.command_route(leader.tile, destination)?;
        Some(
            defenses
                .into_iter()
                .filter(|building| {
                    building.anchor.chebyshev(goal) <= 8
                        || path.iter().any(|tile| {
                            crate::executive::threats::building_threatens(
                                obs,
                                building,
                                *tile,
                                Domain::Ground,
                            )
                        })
                })
                .map(|building| objective_building_strength(building, mode, obs.tick))
                .sum(),
        )
    }

    pub(super) fn mission_army<'a>(
        &mut self,
        obs: &'a Observation,
        armies: &[Army],
        context: MissionContext<'a>,
        intents: &mut Vec<Intent>,
    ) {
        let armies: Vec<_> = armies
            .iter()
            .filter_map(|army| MissionArmy::prepare(army, obs, context.inputs.unavailable))
            .collect();
        let mut assignments = MissionAssignments {
            away_from_home: context
                .inputs
                .missions
                .iter()
                .filter(|(_, mission)| {
                    mission.goal.chebyshev(context.home) > 12 && obs.tick < mission.deadline
                })
                .map(|(army, _)| *army)
                .collect(),
            ..Default::default()
        };
        self.service_retained_missions(obs, &armies, context, &mut assignments, intents);
        self.assign_defense_missions(obs, &armies, context, &mut assignments, intents);
        if context.mode.admit_voluntary_macro {
            self.assign_pressure_and_reserve_missions(
                obs,
                &armies,
                context,
                &mut assignments,
                intents,
            );
        }
    }

    fn service_retained_missions<'a>(
        &mut self,
        obs: &'a Observation,
        armies: &[MissionArmy<'_>],
        context: MissionContext<'a>,
        assignments: &mut MissionAssignments<'a>,
        intents: &mut Vec<Intent>,
    ) {
        let MissionContext {
            mode, inputs, home, ..
        } = context;
        let assessment = mode.evidence.battlefield;
        // Retained missions remain serviced when fresh attention is closed.
        for army in armies.iter().filter(|army| army.eligible(inputs)) {
            let Some(mission) = inputs.prior(army.body.id) else {
                continue;
            };
            let lost_asset = match mission.purpose {
                ArmyPurpose::Defend(asset) => !obs
                    .my_buildings
                    .iter()
                    .chain(&obs.ally_buildings)
                    .any(|building| building.id == asset && building.hp > 0),
                _ => false,
            };
            let pressure_gone = match mission.purpose {
                ArmyPurpose::Defend(asset) => !assessment
                    .pressure
                    .iter()
                    .any(|pressure| pressure.asset == asset),
                _ => false,
            };
            let lost_objective = matches!(mission.purpose, ArmyPurpose::Pressure(target)
                if !obs.enemy_buildings.iter().any(|building| target.matches(building) && building.hp > 0));
            let service = if let ArmyPurpose::Defend(asset) = mission.purpose {
                obs.my_buildings
                    .iter()
                    .chain(&obs.ally_buildings)
                    .find(|building| building.id == asset)
                    .map(|building| building.anchor)
            } else {
                None
            };
            let outside_service = service.is_some_and(|asset| {
                army.center.chebyshev(asset)
                    > mission.goal.chebyshev(asset).saturating_add(2).max(8)
            }) && army.body.state == ArmyState::Engaging;
            if mission.purpose == ArmyPurpose::Recover {
                if army.center.chebyshev(mission.goal) > 2 && obs.tick < mission.deadline {
                    assignments.assigned.insert(army.body.id);
                    continue;
                }
                if army.body.state == ArmyState::Staging {
                    intents.push(Intent::AssignArmyMission {
                        army: army.body.id,
                        members: army.body.members.clone(),
                        mission: ArmyMission {
                            purpose: ArmyPurpose::Reserve,
                            goal: army.center,
                            accepted_at: obs.tick,
                            deadline: obs.tick.saturating_add(1800),
                            score: 0,
                        },
                    });
                    assignments.assigned.insert(army.body.id);
                    continue;
                }
            } else if lost_asset
                || lost_objective
                || pressure_gone
                || outside_service
                || obs.tick >= mission.deadline
            {
                let goal = if lost_asset || matches!(mission.purpose, ArmyPurpose::Pressure(_)) {
                    self.durable_rally_near(obs, home)
                } else {
                    self.durable_rally_near(obs, service.unwrap_or(mission.goal))
                };
                if self.army_reaches(
                    obs,
                    &mut assignments.routes,
                    &army.body,
                    goal,
                    mode.public_map,
                ) {
                    intents.push(Intent::AssignArmyMission {
                        army: army.body.id,
                        members: army.body.members.clone(),
                        mission: ArmyMission {
                            purpose: ArmyPurpose::Recover,
                            goal,
                            accepted_at: obs.tick,
                            deadline: obs.tick.saturating_add(1800),
                            score: 0,
                        },
                    });
                    assignments.assigned.insert(army.body.id);
                }
            }
        }
    }

    fn assign_defense_missions<'a>(
        &mut self,
        obs: &'a Observation,
        armies: &[MissionArmy<'_>],
        context: MissionContext<'a>,
        assignments: &mut MissionAssignments<'a>,
        intents: &mut Vec<Intent>,
    ) {
        let MissionContext {
            mode,
            inputs,
            home,
            dials,
        } = context;
        let assessment = mode.evidence.battlefield;
        let minimum = coherent_attack_size(dials);
        let mut credited = BTreeSet::new();
        let mut served_assets = BTreeSet::new();
        for pressure in &assessment.pressure {
            if pressure.ground == 0
                || obs.tick
                    < pressure
                        .evidence_at
                        .saturating_add(inputs.tuning.reaction_delay)
            {
                continue;
            }
            let goal = obs
                .enemy_units
                .iter()
                .filter(|unit| {
                    pressure.attackers.contains(&unit.id) && unit.body_domain() == Domain::Ground
                })
                .min_by_key(|unit| {
                    (
                        unit.tile.chebyshev(pressure.anchor),
                        unit.tile.y,
                        unit.tile.x,
                        unit.id,
                    )
                })
                .map_or(pressure.anchor, |unit| unit.tile);
            let required = pressure
                .ground
                .saturating_mul(u64::from(dials.enemy_strength_scale))
                / 10_000;
            let mut candidates: Vec<_> = armies
                .iter()
                .filter(|army| army.eligible(inputs))
                .filter(|army| !assignments.assigned.contains(&army.body.id))
                .filter(|army| {
                    army.body.state != ArmyState::Engaging || army.center.chebyshev(goal) <= 8
                })
                .filter(|army| {
                    army.ground_capable >= minimum
                        || crate::executive::locally_overmatches_near(
                            obs,
                            &army.body.members,
                            goal,
                            8,
                        )
                })
                .collect();
            candidates.sort_unstable_by_key(|army| {
                (
                    !inputs.prior(army.body.id).is_some_and(|mission| {
                        mission.purpose == ArmyPurpose::Defend(pressure.asset)
                    }),
                    army.center.manhattan(goal),
                    army.strength,
                    army.body.id,
                )
            });
            let external = self
                .state
                .support_deployments
                .active
                .iter()
                .filter(|deployment| {
                    !deployment.key.air
                        && deployment.key.asset == oxide_sim::ids::Target::Building(pressure.asset)
                })
                .map(|deployment| deployment.unit)
                .chain(
                    inputs
                        .relief
                        .iter()
                        .filter(|(asset, _)| *asset == pressure.asset)
                        .flat_map(|(_, members)| members.iter().copied()),
                );
            let mut coverage = 0_u64;
            for id in external {
                if let Some(unit) = obs
                    .my_units
                    .iter()
                    .find(|unit| unit.id == id && unit.hp > 0 && unit.tile.chebyshev(goal) <= 8)
                    && !inputs.enlisted.contains(&id)
                    && credited.insert(id)
                {
                    coverage = coverage
                        .saturating_add(crate::executive::ground_strength(unit.kind, unit.hp));
                }
            }
            for army in candidates {
                if coverage >= required {
                    break;
                }
                let strength = army
                    .strength
                    .saturating_mul(u64::from(dials.own_strength_scale))
                    / 10_000;
                if strength.saturating_mul(2) < required.saturating_sub(coverage) {
                    continue;
                }
                let same = inputs
                    .prior(army.body.id)
                    .is_some_and(|mission| mission.purpose == ArmyPurpose::Defend(pressure.asset));
                if !same
                    && (!strategic_admission_tick(obs.tick)
                        || assignments.fresh >= inputs.tuning.attention_slots)
                {
                    continue;
                }
                if pressure.anchor.chebyshev(home) > 12
                    && army.center.chebyshev(pressure.anchor) > 8
                {
                    let home_strength: u64 = obs
                        .my_units
                        .iter()
                        .filter(|unit| {
                            !army.body.members.contains(&unit.id)
                                && !inputs.unavailable.contains(&unit.id)
                                && unit.tile.chebyshev(home) <= 12
                                && !armies.iter().any(|body| {
                                    assignments.away_from_home.contains(&body.body.id)
                                        && body.body.members.contains(&unit.id)
                                })
                        })
                        .map(|unit| crate::executive::ground_strength(unit.kind, unit.hp))
                        .sum();
                    if home_strength
                        < u64::from(dials.minimum_core_equivalents)
                            * crate::executive::full_ground_strength(UnitKind::Sentinel)
                    {
                        continue;
                    }
                }
                if !self.army_reaches(
                    obs,
                    &mut assignments.routes,
                    &army.body,
                    goal,
                    mode.public_map,
                ) {
                    continue;
                }
                if !same
                    && goal.chebyshev(home) <= 12
                    && army.body.state == ArmyState::Staging
                    && inputs
                        .prior(army.body.id)
                        .is_none_or(|mission| mission.purpose == ArmyPurpose::Reserve)
                    && army.body.members.len() >= minimum.saturating_mul(2)
                {
                    let mut body: Vec<_> = obs
                        .my_units
                        .iter()
                        .filter(|unit| army.body.members.contains(&unit.id))
                        .collect();
                    let safe = body.iter().all(|unit| {
                        !obs.enemy_units
                            .iter()
                            .any(|enemy| enemy.hp > 0 && enemy.tile.chebyshev(unit.tile) <= 8)
                    });
                    body.sort_unstable_by_key(|unit| (unit.tile.manhattan(goal), unit.id));
                    let mut members = Vec::new();
                    let mut useful = 0_u64;
                    for unit in body {
                        if members.len() >= minimum && useful >= required.saturating_sub(coverage) {
                            break;
                        }
                        useful = useful.saturating_add(
                            crate::executive::ground_strength(unit.kind, unit.hp)
                                .saturating_mul(u64::from(dials.own_strength_scale))
                                / 10_000,
                        );
                        members.push(unit.id);
                    }
                    if safe && members.len().saturating_add(minimum) <= army.body.members.len() {
                        members.sort_unstable();
                        intents.push(Intent::FormArmyWith {
                            army: None,
                            members,
                            staging: goal,
                            minimum,
                            mission: ArmyMission {
                                purpose: ArmyPurpose::Defend(pressure.asset),
                                goal,
                                accepted_at: obs.tick,
                                deadline: obs.tick.saturating_add(1800),
                                score: u64::from(pressure.value).saturating_mul(1024),
                            },
                        });
                        coverage = coverage.saturating_add(useful);
                        assignments.assigned.insert(army.body.id);
                        assignments.fresh += 1;
                        continue;
                    }
                }
                coverage = coverage.saturating_add(strength);
                assignments.assigned.insert(army.body.id);
                if goal.chebyshev(home) > 12 {
                    assignments.away_from_home.insert(army.body.id);
                    assignments
                        .departing_members
                        .extend(army.body.members.iter().copied());
                }
                if same {
                    continue;
                }
                assignments.fresh += 1;
                intents.push(Intent::AssignArmyMission {
                    army: army.body.id,
                    members: army.body.members.clone(),
                    mission: ArmyMission {
                        purpose: ArmyPurpose::Defend(pressure.asset),
                        goal,
                        accepted_at: obs.tick,
                        deadline: obs.tick.saturating_add(1800),
                        score: u64::from(pressure.value).saturating_mul(1024),
                    },
                });
            }
            if coverage >= required {
                served_assets.insert(pressure.asset);
            }
        }
        if strategic_admission_tick(obs.tick) && assignments.fresh < inputs.tuning.attention_slots {
            let mut claimed = inputs.unavailable.to_vec();
            for intent in intents.iter() {
                Self::claim_non_preemptible_intent_units(intent, &mut claimed);
            }
            for pressure in &assessment.pressure {
                if served_assets.contains(&pressure.asset)
                    || pressure.ground == 0
                    || obs.tick
                        < pressure
                            .evidence_at
                            .saturating_add(inputs.tuning.reaction_delay)
                {
                    continue;
                }
                let mut members: Vec<_> = obs
                    .my_units
                    .iter()
                    .filter(|unit| {
                        unit.hp > 0
                            && unit.idle
                            && unit.body_domain() == Domain::Ground
                            && unit.kind.stats().can_fight()
                            && unit.tile.chebyshev(pressure.anchor) <= 8
                            && !inputs.enlisted.contains(&unit.id)
                            && !claimed.contains(&unit.id)
                            && !obs.has_queued_program(unit.id)
                    })
                    .map(|unit| unit.id)
                    .collect();
                members.sort_unstable();
                if members.len() < 2
                    || !crate::executive::locally_overmatches_near(
                        obs,
                        &members,
                        pressure.anchor,
                        8,
                    )
                {
                    continue;
                }
                if let Some(size) = (2..members.len()).find(|size| {
                    crate::executive::locally_overmatches_near(
                        obs,
                        &members[..*size],
                        pressure.anchor,
                        8,
                    )
                }) {
                    members.truncate(size);
                }
                let goal = obs
                    .enemy_units
                    .iter()
                    .filter(|unit| {
                        pressure.attackers.contains(&unit.id)
                            && unit.body_domain() == Domain::Ground
                    })
                    .min_by_key(|unit| (unit.tile.chebyshev(pressure.anchor), unit.id))
                    .map_or(pressure.anchor, |unit| unit.tile);
                let trial = Army {
                    id: ArmyId(0),
                    members: members.clone(),
                    state: ArmyState::Staging,
                    staging: pressure.anchor,
                    target: None,
                    focus: None,
                    progress: None,
                    issued: None,
                    bounces: 0,
                };
                if !self.army_reaches(obs, &mut assignments.routes, &trial, goal, mode.public_map) {
                    continue;
                }
                claimed.extend_from_slice(&members);
                intents.push(Intent::FormArmyWith {
                    army: None,
                    members,
                    staging: pressure.anchor,
                    minimum: 2,
                    mission: ArmyMission {
                        purpose: ArmyPurpose::Defend(pressure.asset),
                        goal,
                        accepted_at: obs.tick,
                        deadline: obs.tick.saturating_add(1800),
                        score: u64::from(pressure.value).saturating_mul(1024),
                    },
                });
                assignments.fresh += 1;
                if assignments.fresh >= inputs.tuning.attention_slots {
                    break;
                }
            }
        }
    }

    fn assign_pressure_and_reserve_missions<'a>(
        &mut self,
        obs: &'a Observation,
        armies: &[MissionArmy<'_>],
        context: MissionContext<'a>,
        assignments: &mut MissionAssignments<'a>,
        intents: &mut Vec<Intent>,
    ) {
        let MissionContext {
            mode,
            inputs,
            home,
            dials,
        } = context;
        let minimum = coherent_attack_size(dials);
        let mut objectives: Vec<_> = obs
            .enemy_buildings
            .iter()
            .filter(|building| building.hp > 0)
            .collect();
        let experience = mode.evidence.experience;
        let objective_key = |building: &&BuildingObs, doctrine| {
            let context = ExperienceKey {
                doctrine,
                x: building.anchor.x,
                y: building.anchor.y,
                subject: ExperienceSubject::Building(Some(building.id)),
            };
            let preference = (1024 + i32::from(experience.score(context)) / 2) as u64;
            let value = building
                .kind
                .tier_stats(building.tier)
                .construction
                .map_or(1, |cost| cost.cost)
                .max(1);
            let score =
                u64::from(value) * preference / (1 + home.manhattan(building.anchor) as u64);
            (
                std::cmp::Reverse(score),
                building.anchor.y,
                building.anchor.x,
                building.id,
            )
        };
        objectives.sort_unstable_by_key(|building| objective_key(building, Doctrine::Pressure));
        for army in armies.iter().filter(|army| army.eligible(inputs)) {
            if assignments.assigned.contains(&army.body.id) {
                continue;
            }
            if inputs.prior(army.body.id).is_some_and(|mission| {
                matches!(mission.purpose, ArmyPurpose::Defend(_)) && obs.tick < mission.deadline
            }) {
                continue;
            }
            if army.body.state == ArmyState::Engaging
                || assignments.fresh >= inputs.tuning.attention_slots
            {
                continue;
            }
            if army.ground_capable < minimum {
                continue;
            }
            let doctrine = crate::experience::ground_doctrine(obs, &army.body.members);
            let mut ranked = objectives.clone();
            ranked.sort_unstable_by_key(|building| objective_key(building, doctrine));
            for objective in &ranked {
                let current = inputs.prior(army.body.id);
                if current.is_some_and(|mission| {
                    matches!(mission.purpose, ArmyPurpose::Pressure(target) if target.matches(objective))
                        && obs.tick < mission.deadline
                }) {
                    break;
                }
                let goal = objective.anchor;
                let mut deploying = army.body.as_ref().clone();
                if army.center.chebyshev(home) <= 12 && goal.chebyshev(home) > 12 {
                    let floor = u64::from(dials.minimum_core_equivalents)
                        * crate::executive::full_ground_strength(UnitKind::Sentinel);
                    let other_home: u64 = obs
                        .my_units
                        .iter()
                        .filter(|unit| {
                            unit.hp > 0
                                && unit.tile.chebyshev(home) <= 12
                                && !army.body.members.contains(&unit.id)
                                && !inputs.unavailable.contains(&unit.id)
                                && !assignments.departing_members.contains(&unit.id)
                                && !armies.iter().any(|body| {
                                    assignments.away_from_home.contains(&body.body.id)
                                        && body.body.members.contains(&unit.id)
                                })
                        })
                        .map(|unit| crate::executive::ground_strength(unit.kind, unit.hp))
                        .sum();
                    if other_home < floor {
                        if army.body.state != ArmyState::Staging
                            || current
                                .is_some_and(|mission| mission.purpose != ArmyPurpose::Reserve)
                            || obs
                                .my_units
                                .iter()
                                .filter(|unit| army.body.members.contains(&unit.id))
                                .any(|unit| {
                                    obs.enemy_units.iter().any(|enemy| {
                                        enemy.hp > 0 && enemy.tile.chebyshev(unit.tile) <= 8
                                    })
                                })
                        {
                            continue;
                        }
                        let mut guards: Vec<_> = obs
                            .my_units
                            .iter()
                            .filter(|unit| {
                                army.body.members.contains(&unit.id)
                                    && unit.tile.chebyshev(home) <= 12
                            })
                            .collect();
                        guards.sort_unstable_by_key(|unit| (unit.tile.manhattan(home), unit.id));
                        let mut retained = 0;
                        let mut strength = other_home;
                        for unit in guards {
                            if retained >= minimum && strength >= floor {
                                break;
                            }
                            deploying.members.retain(|id| *id != unit.id);
                            retained += 1;
                            strength += crate::executive::ground_strength(unit.kind, unit.hp);
                        }
                        if strength < floor
                            || retained < minimum
                            || deploying.members.len() < minimum
                        {
                            continue;
                        }
                    }
                }
                let Some(approach_defenses) =
                    self.approach_defense_strength(obs, &deploying, goal, mode)
                else {
                    continue;
                };
                let enemies: u64 = (obs
                    .enemy_units
                    .iter()
                    .filter(|unit| unit.tile.chebyshev(goal) <= 8)
                    .map(crate::executive::unit_strength)
                    .sum::<u64>()
                    + approach_defenses)
                    .saturating_mul(u64::from(dials.enemy_strength_scale))
                    / 10_000;
                let floor = crate::executive::full_ground_strength(UnitKind::Sentinel)
                    * if self.state.desperate {
                        1
                    } else if objective.seen {
                        3
                    } else {
                        6
                    };
                let strength = crate::executive::marching_strength(&deploying, obs)
                    .saturating_mul(u64::from(dials.own_strength_scale))
                    / 10_000;
                let margin = if self.state.desperate {
                    4
                } else {
                    8 - (obs.tick / 4000).min(4)
                };
                if strength.saturating_mul(4) < enemies.max(floor).saturating_mul(margin)
                    || !self.army_reaches(
                        obs,
                        &mut assignments.routes,
                        &deploying,
                        goal,
                        mode.public_map,
                    )
                {
                    continue;
                }
                let score = strength / (1 + army.center.manhattan(goal) as u64);
                if current.is_some_and(|mission| {
                    matches!(mission.purpose, ArmyPurpose::Pressure(_))
                        && obs.tick < mission.deadline
                        && (obs.tick.saturating_sub(mission.accepted_at) < 300
                            || score.saturating_mul(4) < mission.score.saturating_mul(5))
                }) {
                    continue;
                }
                let mission = ArmyMission {
                    purpose: ArmyPurpose::Pressure(ArmyObjective::from_building(objective)),
                    goal,
                    accepted_at: obs.tick,
                    deadline: obs.tick.saturating_add(1800),
                    score,
                };
                if deploying.members != army.body.members {
                    intents.push(Intent::FormArmyWith {
                        army: None,
                        members: deploying.members.clone(),
                        staging: army.body.staging,
                        minimum,
                        mission,
                    });
                } else {
                    intents.push(Intent::AssignArmyMission {
                        army: army.body.id,
                        members: deploying.members.clone(),
                        mission,
                    });
                    if goal.chebyshev(home) > 12 {
                        assignments.away_from_home.insert(army.body.id);
                    }
                }
                if goal.chebyshev(home) > 12 {
                    assignments.departing_members.extend(deploying.members);
                }
                assignments.fresh += 1;
                assignments.assigned.insert(army.body.id);
                break;
            }
        }

        let staging_army = armies
            .iter()
            .filter(|army| {
                army.body.state == ArmyState::Staging
                    && army.body.target.is_none()
                    && !assignments.assigned.contains(&army.body.id)
                    && inputs.prior(army.body.id).is_none_or(|mission| {
                        matches!(mission.purpose, ArmyPurpose::Reserve | ArmyPurpose::Recover)
                    })
            })
            .min_by_key(|army| army.body.id);
        let rally = self.rally_point(
            obs,
            staging_army.map(|army| army.body.as_ref()),
            objectives.first().map(|building| building.anchor),
            home,
        );
        let count = staging_army.map_or(0, |army| army.body.members.len());
        let target = dials
            .army_size
            .max(minimum as u32)
            .max(count.saturating_add(2) as u32) as usize;
        let mut claimed = inputs.unavailable.to_vec();
        for intent in intents.iter() {
            Self::claim_non_preemptible_intent_units(intent, &mut claimed);
        }
        let mut free: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| {
                unit.hp > 0
                    && unit.idle
                    && unit.kind.stats().domain == Domain::Ground
                    && unit.kind.stats().can_fight()
                    && !inputs.enlisted.contains(&unit.id)
                    && !claimed.contains(&unit.id)
                    && !obs.has_queued_program(unit.id)
            })
            .collect();
        free.sort_unstable_by_key(|unit| (unit.tile.manhattan(rally), unit.id));
        let mut members = Vec::new();
        if let Some(destination) = staging_army {
            for source in armies.iter().filter(|source| {
                source.body.id != destination.body.id
                    && source.body.state == ArmyState::Staging
                    && source.body.target.is_none()
                    && !assignments.assigned.contains(&source.body.id)
                    && source.body.staging.chebyshev(rally) <= 2
                    && inputs
                        .prior(source.body.id)
                        .is_none_or(|mission| mission.purpose == ArmyPurpose::Reserve)
            }) {
                if source.body.members.iter().all(|id| !claimed.contains(id))
                    && self.army_reaches(
                        obs,
                        &mut assignments.routes,
                        &source.body,
                        rally,
                        mode.public_map,
                    )
                {
                    members.extend_from_slice(&source.body.members);
                }
            }
            members.sort_unstable();
            members.dedup();
        }
        for unit in free {
            if members.len() >= target.saturating_sub(count) {
                break;
            }
            let mut group = members.clone();
            group.push(unit.id);
            group.sort_unstable();
            let trial = Army {
                id: ArmyId(0),
                members: group.clone(),
                state: ArmyState::Staging,
                staging: rally,
                target: None,
                focus: None,
                progress: None,
                issued: None,
                bounces: 0,
            };
            if self.army_reaches(obs, &mut assignments.routes, &trial, rally, mode.public_map) {
                members = group;
            }
        }
        if !members.is_empty() {
            intents.push(Intent::FormArmyWith {
                army: staging_army.map(|army| army.body.id),
                members,
                staging: rally,
                mission: staging_army
                    .and_then(|army| inputs.prior(army.body.id))
                    .filter(|mission| {
                        mission.purpose == ArmyPurpose::Reserve
                            && mission.goal == rally
                            && obs.tick < mission.deadline
                    })
                    .cloned()
                    .unwrap_or(ArmyMission {
                        purpose: ArmyPurpose::Reserve,
                        goal: rally,
                        accepted_at: obs.tick,
                        deadline: obs.tick.saturating_add(1800),
                        score: 0,
                    }),
                minimum,
            });
        }
    }
}
