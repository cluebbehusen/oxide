use super::*;
use crate::bot::executive::{ArmyId, ArmyMission, ArmyObjective, ArmyPurpose};
use crate::bot::experience::{Doctrine, ExperienceKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct GroundMissionInputs {
    pub(in crate::bot) missions: Vec<(ArmyId, ArmyMission)>,
    pub(in crate::bot) unavailable: Vec<UnitId>,
    pub(in crate::bot) enlisted: Vec<UnitId>,
    pub(in crate::bot) tuning: DifficultyTuning,
    pub(in crate::bot) relief: Option<(BuildingId, Vec<UnitId>)>,
}

impl UtilityPolicy {
    pub(super) fn mission_army(
        &mut self,
        dials: &Dials,
        obs: &Observation,
        armies: &[Army],
        home: TilePos,
        mode: PolicyMode<'_>,
        intents: &mut Vec<Intent>,
    ) {
        let inputs = self
            .ground_inputs
            .as_ref()
            .expect("ground mission context")
            .clone();
        let assessment = self.battlefield.clone();
        let available_armies: Vec<_> = armies
            .iter()
            .map(|army| {
                let mut army = army.clone();
                if army.state == ArmyState::Staging {
                    army.members.retain(|id| !inputs.unavailable.contains(id));
                }
                army
            })
            .filter(|army| !army.members.is_empty())
            .collect();
        let armies = available_armies.as_slice();
        let mut assigned = std::collections::BTreeSet::new();
        let mut away_from_home: std::collections::BTreeSet<_> = inputs
            .missions
            .iter()
            .filter(|(_, mission)| mission.goal.chebyshev(home) > 12 && obs.tick < mission.deadline)
            .map(|(army, _)| *army)
            .collect();
        let mut routes = None;
        let minimum = coherent_attack_size(dials, true);
        let eligible = |army: &&Army| {
            army.state != ArmyState::Withdrawing
                && army
                    .members
                    .iter()
                    .all(|id| !inputs.unavailable.contains(id))
        };
        let prior = |id| {
            inputs
                .missions
                .iter()
                .find(|(army, _)| *army == id)
                .map(|(_, mission)| mission)
        };
        let center = |army: &Army| -> TilePos {
            let members: Vec<_> = obs
                .my_units
                .iter()
                .filter(|unit| army.members.contains(&unit.id))
                .collect();
            if members.is_empty() {
                return army.staging;
            }
            TilePos::new(
                members
                    .iter()
                    .map(|unit| i64::from(unit.tile.x))
                    .sum::<i64>() as i32
                    / members.len() as i32,
                members
                    .iter()
                    .map(|unit| i64::from(unit.tile.y))
                    .sum::<i64>() as i32
                    / members.len() as i32,
            )
        };
        // Service retained missions even on a cadence with no fresh attention.
        for army in armies.iter().filter(eligible) {
            let Some(mission) = prior(army.id) else {
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
                center(army).chebyshev(asset)
                    > mission.goal.chebyshev(asset).saturating_add(2).max(8)
            }) && army.state == ArmyState::Engaging;
            if mission.purpose == ArmyPurpose::Recover {
                if center(army).chebyshev(mission.goal) > 2 && obs.tick < mission.deadline {
                    assigned.insert(army.id);
                    continue;
                }
                if army.state == ArmyState::Staging {
                    intents.push(Intent::AssignArmyMission {
                        army: army.id,
                        members: army.members.clone(),
                        mission: ArmyMission {
                            purpose: ArmyPurpose::Reserve,
                            goal: center(army),
                            accepted_at: obs.tick,
                            deadline: obs.tick.saturating_add(1800),
                            score: 0,
                        },
                    });
                    assigned.insert(army.id);
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
                if self.army_reaches(obs, &mut routes, army, goal, mode.public_map) {
                    intents.push(Intent::AssignArmyMission {
                        army: army.id,
                        members: army.members.clone(),
                        mission: ArmyMission {
                            purpose: ArmyPurpose::Recover,
                            goal,
                            accepted_at: obs.tick,
                            deadline: obs.tick.saturating_add(1800),
                            score: 0,
                        },
                    });
                    assigned.insert(army.id);
                }
            }
        }
        let mut fresh = 0;
        let mut departing_members = std::collections::BTreeSet::new();
        let mut credited = std::collections::BTreeSet::new();
        let mut served_assets = std::collections::BTreeSet::new();
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
                .filter(eligible)
                .filter(|army| !assigned.contains(&army.id))
                .filter(|army| {
                    army.state != ArmyState::Engaging || center(army).chebyshev(goal) <= 8
                })
                .filter(|army| {
                    ground_capable_members(army, obs) >= minimum
                        || crate::bot::executive::locally_overmatches_near(
                            obs,
                            &army.members,
                            goal,
                            8,
                        )
                })
                .collect();
            candidates.sort_unstable_by_key(|army| {
                (
                    !prior(army.id).is_some_and(|mission| {
                        mission.purpose == ArmyPurpose::Defend(pressure.asset)
                    }),
                    center(army).manhattan(goal),
                    crate::bot::executive::marching_strength(army, obs),
                    army.id,
                )
            });
            let external = self
                .support_deployments
                .active
                .iter()
                .filter(|deployment| {
                    !deployment.key.air
                        && deployment.key.asset == crate::ids::Target::Building(pressure.asset)
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
                        .saturating_add(crate::bot::executive::ground_strength(unit.kind, unit.hp));
                }
            }
            for army in candidates {
                if coverage >= required {
                    break;
                }
                let strength = crate::bot::executive::marching_strength(army, obs)
                    .saturating_mul(u64::from(dials.own_strength_scale))
                    / 10_000;
                if strength.saturating_mul(2) < required.saturating_sub(coverage) {
                    continue;
                }
                let same = prior(army.id)
                    .is_some_and(|mission| mission.purpose == ArmyPurpose::Defend(pressure.asset));
                if !same
                    && (!strategic_admission_tick(obs.tick)
                        || fresh >= inputs.tuning.attention_slots)
                {
                    continue;
                }
                if pressure.anchor.chebyshev(home) > 12
                    && center(army).chebyshev(pressure.anchor) > 8
                {
                    let home_strength: u64 = obs
                        .my_units
                        .iter()
                        .filter(|unit| {
                            !army.members.contains(&unit.id)
                                && !inputs.unavailable.contains(&unit.id)
                                && unit.tile.chebyshev(home) <= 12
                                && !armies.iter().any(|body| {
                                    away_from_home.contains(&body.id)
                                        && body.members.contains(&unit.id)
                                })
                        })
                        .map(|unit| crate::bot::executive::ground_strength(unit.kind, unit.hp))
                        .sum();
                    if home_strength
                        < u64::from(dials.minimum_core_equivalents)
                            * crate::bot::executive::full_ground_strength(UnitKind::Sentinel)
                    {
                        continue;
                    }
                }
                if !self.army_reaches(obs, &mut routes, army, goal, mode.public_map) {
                    continue;
                }
                if !same
                    && goal.chebyshev(home) <= 12
                    && army.state == ArmyState::Staging
                    && prior(army.id).is_none_or(|mission| mission.purpose == ArmyPurpose::Reserve)
                    && army.members.len() >= minimum.saturating_mul(2)
                {
                    let mut body: Vec<_> = obs
                        .my_units
                        .iter()
                        .filter(|unit| army.members.contains(&unit.id))
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
                            crate::bot::executive::ground_strength(unit.kind, unit.hp)
                                .saturating_mul(u64::from(dials.own_strength_scale))
                                / 10_000,
                        );
                        members.push(unit.id);
                    }
                    if safe && members.len().saturating_add(minimum) <= army.members.len() {
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
                        assigned.insert(army.id);
                        fresh += 1;
                        continue;
                    }
                }
                coverage = coverage.saturating_add(strength);
                assigned.insert(army.id);
                if goal.chebyshev(home) > 12 {
                    away_from_home.insert(army.id);
                    departing_members.extend(army.members.iter().copied());
                }
                if same {
                    continue;
                }
                fresh += 1;
                intents.push(Intent::AssignArmyMission {
                    army: army.id,
                    members: army.members.clone(),
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
        if strategic_admission_tick(obs.tick) && fresh < inputs.tuning.attention_slots {
            let mut claimed = inputs.unavailable.clone();
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
                    || !crate::bot::executive::locally_overmatches_near(
                        obs,
                        &members,
                        pressure.anchor,
                        8,
                    )
                {
                    continue;
                }
                if let Some(size) = (2..members.len()).find(|size| {
                    crate::bot::executive::locally_overmatches_near(
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
                if !self.army_reaches(obs, &mut routes, &trial, goal, mode.public_map) {
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
                fresh += 1;
                if fresh >= inputs.tuning.attention_slots {
                    break;
                }
            }
        }
        if !mode.admit_voluntary_macro {
            return;
        }

        let mut objectives: Vec<_> = obs
            .enemy_buildings
            .iter()
            .filter(|building| building.hp > 0)
            .collect();
        let experience = self.experience.clone();
        let objective_key = |building: &&BuildingObs, doctrine| {
            let context = ExperienceKey {
                doctrine,
                x: building.anchor.x,
                y: building.anchor.y,
                subject: u64::from(building.id.0),
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
        for army in armies.iter().filter(eligible) {
            if assigned.contains(&army.id) {
                continue;
            }
            if prior(army.id).is_some_and(|mission| {
                matches!(mission.purpose, ArmyPurpose::Defend(_)) && obs.tick < mission.deadline
            }) {
                continue;
            }
            if army.state == ArmyState::Engaging || fresh >= inputs.tuning.attention_slots {
                continue;
            }
            if ground_capable_members(army, obs) < minimum {
                continue;
            }
            let doctrine = crate::bot::experience::ground_doctrine(obs, &army.members);
            let mut ranked = objectives.clone();
            ranked.sort_unstable_by_key(|building| objective_key(building, doctrine));
            for objective in &ranked {
                let current = prior(army.id);
                if current.is_some_and(|mission| {
                    matches!(mission.purpose, ArmyPurpose::Pressure(target) if target.matches(objective))
                        && obs.tick < mission.deadline
                }) {
                    break;
                }
                let goal = objective.anchor;
                let mut deploying = army.clone();
                if center(army).chebyshev(home) <= 12 && goal.chebyshev(home) > 12 {
                    let floor = u64::from(dials.minimum_core_equivalents)
                        * crate::bot::executive::full_ground_strength(UnitKind::Sentinel);
                    let other_home: u64 = obs
                        .my_units
                        .iter()
                        .filter(|unit| {
                            unit.hp > 0
                                && unit.tile.chebyshev(home) <= 12
                                && !army.members.contains(&unit.id)
                                && !inputs.unavailable.contains(&unit.id)
                                && !departing_members.contains(&unit.id)
                                && !armies.iter().any(|body| {
                                    away_from_home.contains(&body.id)
                                        && body.members.contains(&unit.id)
                                })
                        })
                        .map(|unit| crate::bot::executive::ground_strength(unit.kind, unit.hp))
                        .sum();
                    if other_home < floor {
                        if army.state != ArmyState::Staging
                            || current
                                .is_some_and(|mission| mission.purpose != ArmyPurpose::Reserve)
                            || obs
                                .my_units
                                .iter()
                                .filter(|unit| army.members.contains(&unit.id))
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
                                army.members.contains(&unit.id) && unit.tile.chebyshev(home) <= 12
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
                            strength += crate::bot::executive::ground_strength(unit.kind, unit.hp);
                        }
                        if strength < floor
                            || retained < minimum
                            || deploying.members.len() < minimum
                        {
                            continue;
                        }
                    }
                }
                let enemies: u64 = (obs
                    .enemy_units
                    .iter()
                    .filter(|unit| unit.tile.chebyshev(goal) <= 8)
                    .map(crate::bot::executive::unit_strength)
                    .sum::<u64>()
                    + obs
                        .enemy_buildings
                        .iter()
                        .filter(|building| building.anchor.chebyshev(goal) <= 8)
                        .map(|building| objective_building_strength(building, mode, obs.tick))
                        .sum::<u64>())
                .saturating_mul(u64::from(dials.enemy_strength_scale))
                    / 10_000;
                let floor = crate::bot::executive::full_ground_strength(UnitKind::Sentinel)
                    * if self.desperate {
                        1
                    } else if objective.seen {
                        3
                    } else {
                        6
                    };
                let strength = crate::bot::executive::marching_strength(&deploying, obs)
                    .saturating_mul(u64::from(dials.own_strength_scale))
                    / 10_000;
                let margin = if self.desperate {
                    4
                } else {
                    8 - (obs.tick / 4000).min(4)
                };
                if strength.saturating_mul(4) < enemies.max(floor).saturating_mul(margin)
                    || !self.army_reaches(obs, &mut routes, &deploying, goal, mode.public_map)
                {
                    continue;
                }
                let score = strength / (1 + center(army).manhattan(goal) as u64);
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
                if deploying.members != army.members {
                    intents.push(Intent::FormArmyWith {
                        army: None,
                        members: deploying.members.clone(),
                        staging: army.staging,
                        minimum,
                        mission,
                    });
                } else {
                    intents.push(Intent::AssignArmyMission {
                        army: army.id,
                        members: deploying.members.clone(),
                        mission,
                    });
                    if goal.chebyshev(home) > 12 {
                        away_from_home.insert(army.id);
                    }
                }
                if goal.chebyshev(home) > 12 {
                    departing_members.extend(deploying.members);
                }
                fresh += 1;
                assigned.insert(army.id);
                break;
            }
        }

        let staging_army = armies
            .iter()
            .filter(|army| {
                army.state == ArmyState::Staging
                    && army.target.is_none()
                    && !assigned.contains(&army.id)
                    && prior(army.id).is_none_or(|mission| {
                        matches!(mission.purpose, ArmyPurpose::Reserve | ArmyPurpose::Recover)
                    })
            })
            .min_by_key(|army| army.id);
        let rally = self.rally_point(
            obs,
            staging_army,
            objectives.first().map(|building| building.anchor),
            home,
            true,
        );
        let count = staging_army.map_or(0, |army| army.members.len());
        let target = dials
            .army_size
            .max(minimum as u32)
            .max(count.saturating_add(2) as u32) as usize;
        let mut claimed = inputs.unavailable.clone();
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
                source.id != destination.id
                    && source.state == ArmyState::Staging
                    && source.target.is_none()
                    && !assigned.contains(&source.id)
                    && source.staging.chebyshev(rally) <= 2
                    && prior(source.id)
                        .is_none_or(|mission| mission.purpose == ArmyPurpose::Reserve)
            }) {
                if source.members.iter().all(|id| !claimed.contains(id))
                    && self.army_reaches(obs, &mut routes, source, rally, mode.public_map)
                {
                    members.extend_from_slice(&source.members);
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
            if self.army_reaches(obs, &mut routes, &trial, rally, mode.public_map) {
                members = group;
            }
        }
        if !members.is_empty() {
            intents.push(Intent::FormArmyWith {
                army: staging_army.map(|army| army.id),
                members,
                staging: rally,
                mission: staging_army
                    .and_then(|army| prior(army.id))
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
