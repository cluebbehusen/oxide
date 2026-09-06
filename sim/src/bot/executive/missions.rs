use super::*;
use chassis::Tick;
use serde::Serialize;

/// Frozen site identity that remains meaningful when fog replaces a live id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ArmyObjective {
    /// Observed identity, absent when only a remembered footprint was available.
    pub id: Option<BuildingId>,
    /// Last observed owner, never inferred from anonymous radar.
    pub player: PlayerId,
    /// Last observed building type.
    pub kind: BuildingKind,
    /// Complete footprint anchor, transformed separately from movement goals.
    pub anchor: TilePos,
}

impl ArmyObjective {
    /// Bind only the information supplied by this observation.
    pub fn from_building(building: &super::super::observation::BuildingObs) -> Self {
        Self {
            id: building.seen.then_some(building.id),
            player: building.player,
            kind: building.kind,
            anchor: building.anchor,
        }
    }

    pub(crate) fn matches(&self, building: &super::super::observation::BuildingObs) -> bool {
        building.player == self.player
            && building.kind == self.kind
            && building.anchor == self.anchor
            && (!building.seen || self.id.is_none_or(|id| id == building.id))
    }

    pub(crate) fn observed_id(&self, obs: &Observation) -> BuildingId {
        obs.enemy_buildings
            .iter()
            .find(|building| building.seen && self.matches(building))
            .map_or(self.id.unwrap_or(BuildingId(u32::MAX)), |building| {
                building.id
            })
    }

    fn same_site(&self, other: &Self) -> bool {
        self.player == other.player
            && self.kind == other.kind
            && self.anchor == other.anchor
            && self
                .id
                .zip(other.id)
                .is_none_or(|(left, right)| left == right)
    }
}

/// A ground body's responsibility, independent of its tactical combat state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ArmyPurpose {
    /// Protect an exact own or allied asset.
    Defend(BuildingId),
    /// Pressure a known enemy objective.
    Pressure(ArmyObjective),
    /// Gather or hold useful uncommitted strength.
    Reserve,
    /// Return without reacquiring a retreating enemy.
    Recover,
}

/// Accepted executive ownership metadata; proposals cannot mutate it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArmyMission {
    /// Stable consumer of this force.
    pub purpose: ArmyPurpose,
    /// Exact movement or service point.
    pub goal: TilePos,
    /// First accepted assignment tick.
    pub accepted_at: Tick,
    /// Fixed useful horizon.
    pub deadline: Tick,
    /// Comparison score frozen at acceptance, used for reassignment hysteresis.
    pub score: u64,
}

/// Exact result of an attempted responsibility or membership change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MissionDisposition {
    /// Validated membership and responsibility were committed.
    Accepted,
    /// The exact assignment was already active.
    Unchanged,
    /// The named body no longer exists.
    MissingArmy,
    /// Tactical catastrophic withdrawal takes precedence.
    EmergencyWithdrawal,
    /// Engaged bodies cannot be redirected into fresh pressure.
    Engaged,
    /// Membership was empty, noncanonical, stale, or failed a staging minimum.
    InvalidMembership,
    /// Another accepted responsibility owns at least one member.
    OwnedElsewhere,
    /// The proposed acceptance or useful horizon is invalid.
    InvalidDeadline,
    /// Exact staging validation refused the proposed reorganization.
    InvalidReorganization,
}

/// Observational lowering receipt, never an input to mission selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MissionDecision {
    /// Existing army, or absent when requesting a new exact body.
    pub army: Option<u32>,
    /// Exact requested members.
    pub members: Vec<UnitId>,
    /// Requested responsibility and fixed deadline.
    pub mission: ArmyMission,
    /// Actual acceptance or rejection boundary.
    pub disposition: MissionDisposition,
}

impl Executive {
    pub(super) fn watch_ground_mission(
        &mut self,
        obs: &Observation,
        army: ArmyId,
        mission: &ArmyMission,
    ) {
        use crate::bot::experience::{
            Doctrine, EpisodeId, EpisodeOwner, ExperienceKey, Outcome, OutcomeReason,
        };
        let Some(body) = self.armies.iter().find(|body| body.id == army) else {
            return;
        };
        let journal = self.ground_outcomes.entry(army).or_default();
        if matches!(mission.purpose, ArmyPurpose::Reserve | ArmyPurpose::Recover) {
            let completed_defense = journal.has_progress()
                && journal.context().is_some_and(|context| {
                    context.doctrine == Doctrine::Fortification
                        && obs
                            .my_buildings
                            .iter()
                            .chain(&obs.ally_buildings)
                            .any(|asset| {
                                u64::from(asset.id.0) == context.subject
                                    && asset.hp > 0
                                    && !obs.enemy_units.iter().any(|enemy| {
                                        ground_strength(enemy.kind, enemy.hp) > 0
                                    && (enemy.tile.chebyshev(asset.anchor) <= 8
                                        || crate::bot::utility::ground_weapon_reaches_footprint(
                                            enemy, asset.anchor, asset.kind.base_stats().size))
                                    })
                            })
                        && ((context.y - 4).max(0)..=(context.y + 4).min(obs.map_height - 1)).all(
                            |y| {
                                ((context.x - 4).max(0)..=(context.x + 4).min(obs.map_width - 1))
                                    .all(|x| obs.visible(TilePos::new(x, y)))
                            },
                        )
                });
            journal.finish(
                obs,
                if completed_defense {
                    Outcome::Complete
                } else {
                    Outcome::Invalidated
                },
                if completed_defense {
                    OutcomeReason::ServiceCompleted
                } else {
                    OutcomeReason::Preempted
                },
                if completed_defense { 750 } else { 1000 },
                completed_defense,
            );
            return;
        }
        let (doctrine, subject) = match mission.purpose {
            ArmyPurpose::Pressure(target) => (
                crate::bot::experience::ground_doctrine(obs, &body.members),
                target.id.unwrap_or(BuildingId(u32::MAX)),
            ),
            ArmyPurpose::Defend(id) => (Doctrine::Fortification, id),
            _ => unreachable!(),
        };
        let context = ExperienceKey {
            doctrine,
            x: mission.goal.x,
            y: mission.goal.y,
            subject: u64::from(subject.0),
        };
        journal.watch(
            obs,
            EpisodeId {
                owner: EpisodeOwner::Ground,
                serial: (u64::from(army.0) << 32) | mission.accepted_at,
            },
            context,
            &body.members,
            body.state as u8,
        );
        if let ArmyPurpose::Pressure(target) = mission.purpose {
            journal.bind_objective(target);
            journal.observe_objective(obs, target.observed_id(obs));
        }
        let peers: Vec<_> = self
            .ground_outcomes
            .iter()
            .filter(|(id, _)| {
                match (
                    mission.purpose,
                    self.missions.get(id).map(|mission| mission.purpose),
                ) {
                    (ArmyPurpose::Pressure(target), Some(ArmyPurpose::Pressure(other))) => {
                        target.same_site(&other)
                    }
                    (ArmyPurpose::Defend(target), Some(ArmyPurpose::Defend(other))) => {
                        target == other
                    }
                    _ => false,
                }
            })
            .filter_map(|(id, journal)| {
                journal
                    .concurrent_ground_credit(doctrine)
                    .map(|(credit, doctrine)| (*id, credit, doctrine))
            })
            .collect();
        if let Some(credit) = peers
            .iter()
            .map(|(_, credit, _)| *credit)
            .min_by_key(|credit| (credit.owner == EpisodeOwner::Ground, *credit))
        {
            let ambiguous = peers
                .iter()
                .any(|(_, _, owner_doctrine)| *owner_doctrine != doctrine);
            for (id, _, _) in peers {
                let journal = self
                    .ground_outcomes
                    .get_mut(&id)
                    .expect("observed ground owner");
                journal.share_credit(credit);
                if ambiguous {
                    journal.withhold_doctrine_credit();
                }
            }
        }
    }

    pub(super) fn observe_ground_outcomes(&mut self, obs: &Observation) {
        use crate::bot::experience::{Outcome, OutcomeReason, own_unit_health};
        for (id, journal) in &mut self.ground_outcomes {
            let Some(mission) = self.missions.get(id) else {
                continue;
            };
            if let Some(army) = self.armies.iter().find(|army| army.id == *id) {
                journal.observe_phase(army.state as u8);
                if army.state == ArmyState::Engaging {
                    journal.progress(1);
                }
                if army
                    .members
                    .iter()
                    .all(|id| own_unit_health(obs, *id).is_none_or(|hp| hp == 0))
                {
                    journal.finish(
                        obs,
                        Outcome::Ineffective,
                        OutcomeReason::RequiredUnitLost,
                        1000,
                        true,
                    );
                } else if army.state == ArmyState::Withdrawing {
                    let executed = journal.has_progress() || journal.own_lost_value(obs) > 0;
                    journal.finish(
                        obs,
                        if executed {
                            Outcome::Ineffective
                        } else {
                            Outcome::Aborted
                        },
                        OutcomeReason::UnsafeApproach,
                        1000,
                        executed,
                    );
                } else if let ArmyPurpose::Pressure(target) = mission.purpose
                    && journal.observe_objective(obs, target.observed_id(obs))
                {
                    journal.finish(
                        obs,
                        Outcome::Complete,
                        OutcomeReason::ObjectiveObservedGone,
                        750,
                        false,
                    );
                } else if obs.tick >= mission.deadline {
                    journal.finish(
                        obs,
                        if journal.has_progress() {
                            Outcome::Partial
                        } else {
                            Outcome::Inconclusive
                        },
                        OutcomeReason::Deadline,
                        if journal.has_progress() { 500 } else { 0 },
                        false,
                    );
                }
            } else if journal.all_participants_lost(obs) {
                journal.finish(
                    obs,
                    Outcome::Ineffective,
                    OutcomeReason::RequiredUnitLost,
                    1000,
                    true,
                );
            } else if matches!(mission.purpose, ArmyPurpose::Pressure(target) if journal.observe_objective(obs, target.observed_id(obs)))
            {
                journal.finish(
                    obs,
                    Outcome::Complete,
                    OutcomeReason::ObjectiveObservedGone,
                    750,
                    false,
                );
            } else {
                journal.finish(
                    obs,
                    Outcome::Inconclusive,
                    OutcomeReason::LostContact,
                    0,
                    false,
                );
            }
        }
    }

    pub(super) fn form_exact_army(
        &mut self,
        obs: &Observation,
        destination: Option<ArmyId>,
        members: &[UnitId],
        staging: TilePos,
        minimum: usize,
        unavailable: &[UnitId],
    ) -> Option<ArmyId> {
        if members.is_empty() || minimum < 2 || members.windows(2).any(|pair| pair[0] >= pair[1]) {
            return None;
        }
        if let Some(id) = destination {
            let army = self.armies.iter().find(|army| army.id == id)?;
            if army.state != ArmyState::Staging {
                return None;
            }
            if self.missions.get(&id).is_some_and(|mission| {
                mission.purpose != ArmyPurpose::Reserve || mission.goal.chebyshev(staging) > 2
            }) {
                return None;
            }
        }
        for id in members {
            let unit = obs.my_units.iter().find(|unit| unit.id == *id)?;
            if unit.hp == 0
                || unit.kind.stats().domain != crate::stats::Domain::Ground
                || !unit.kind.stats().can_fight()
                || obs.has_queued_program(*id)
                || unit.founding.is_some()
                || unit.site.is_some()
                || unit.repairing
                || unavailable.contains(id)
                || self.rear.iter().any(|unit| unit.id == *id)
                || self.exhausted_rear.contains(id)
            {
                return None;
            }
            if let Some(source) = self.armies.iter().find(|army| army.members.contains(id)) {
                if obs
                    .enemy_units
                    .iter()
                    .any(|enemy| enemy.hp > 0 && enemy.tile.chebyshev(unit.tile) <= 8)
                {
                    return None;
                }
                if source.state != ArmyState::Staging || Some(source.id) == destination {
                    return None;
                }
                if source.members.iter().any(|id| {
                    obs.my_units
                        .iter()
                        .find(|unit| unit.id == *id)
                        .is_none_or(|unit| unit.tile.chebyshev(source.staging) > 6)
                }) {
                    return None;
                }
                if self
                    .missions
                    .get(&source.id)
                    .is_some_and(|mission| mission.purpose != ArmyPurpose::Reserve)
                {
                    return None;
                }
            } else if !unit.idle {
                return None;
            }
        }
        for source in &self.armies {
            let transferred = source
                .members
                .iter()
                .filter(|id| members.binary_search(id).is_ok())
                .count();
            let remaining = source.members.len() - transferred;
            if transferred > 0 && remaining > 0 && remaining < minimum {
                return None;
            }
        }
        let id = match destination {
            Some(id) => id,
            None => {
                let next = self.next_army.checked_add(1)?;
                let id = ArmyId(self.next_army);
                self.next_army = next;
                self.armies.push(Army {
                    id,
                    members: Vec::new(),
                    state: ArmyState::Staging,
                    staging,
                    target: None,
                    focus: None,
                    progress: None,
                    issued: None,
                    bounces: 0,
                });
                id
            }
        };
        for source in &mut self.armies {
            if source.id != id {
                source
                    .members
                    .retain(|member| members.binary_search(member).is_err());
            }
        }
        let army = self
            .armies
            .iter_mut()
            .find(|army| army.id == id)
            .expect("validated destination");
        army.members.extend_from_slice(members);
        army.members.sort_unstable();
        army.staging = staging;
        self.armies.retain(|army| !army.members.is_empty());
        self.missions
            .retain(|id, _| self.armies.iter().any(|army| army.id == *id));
        Some(id)
    }
}
