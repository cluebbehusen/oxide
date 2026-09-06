//! Bounded, attributable experience. Reports come from owners, never from traces.

use crate::ids::UnitId;
use chassis::Tick;
use serde::Serialize;

use super::observation::Observation;

const EPISODE_LIMIT: usize = 64;
const CONTEXT_LIMIT: usize = 128;
const SCORE_LIMIT: i32 = 1024;
const EPISODE_WEIGHT: i32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) enum Doctrine {
    Pressure,
    Air,
    Siege,
    Expansion,
    Fortification,
    Sustain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) enum EpisodeOwner {
    Ground,
    Air,
    Lift,
    Raid,
    Relief,
    Reconnaissance,
    Support,
    Harvest,
    Construction,
    LiftAssault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct EpisodeId {
    pub(crate) owner: EpisodeOwner,
    pub(crate) serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct ExperienceKey {
    pub(crate) doctrine: Doctrine,
    pub(crate) y: i32,
    pub(crate) x: i32,
    /// Domain-owned objective, counter, or service identity.
    pub(crate) subject: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum Outcome {
    Complete,
    Partial,
    Aborted,
    Ineffective,
    Invalidated,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) enum OutcomeReason {
    ObjectiveObservedGone,
    ServiceCompleted,
    ObservedProgress,
    RequiredUnitLost,
    ObservedCounter,
    UnsafeApproach,
    BlockedRoute,
    Deadline,
    Preempted,
    LostContact,
    FundingUnavailable,
    SiteOccupied,
    ResourceExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct EpisodeReport {
    pub(crate) id: EpisodeId,
    /// Coordinated components share one credit identity, even after handoff.
    pub(crate) credit: EpisodeId,
    pub(crate) context: ExperienceKey,
    pub(crate) started_at: Tick,
    pub(crate) finished_at: Tick,
    pub(crate) participants: Vec<UnitId>,
    pub(crate) phase: u8,
    pub(crate) outcome: Outcome,
    pub(crate) reason: OutcomeReason,
    pub(crate) observed_progress: u32,
    pub(crate) own_lost_value: u32,
    /// Strength of attribution in thousandths, not certainty about hidden state.
    pub(crate) confidence: u16,
    pub(crate) doctrine_eligible: bool,
}

impl EpisodeReport {
    fn contextual_rank(&self) -> (u8, u16, Tick, std::cmp::Reverse<EpisodeId>) {
        let quality = match self.outcome {
            Outcome::Complete | Outcome::Ineffective => 3,
            Outcome::Aborted => 2,
            Outcome::Partial => 1,
            Outcome::Invalidated | Outcome::Inconclusive => 0,
        };
        (
            quality,
            self.confidence,
            self.finished_at,
            std::cmp::Reverse(self.id),
        )
    }

    fn contribution(&self) -> i32 {
        let magnitude = EPISODE_WEIGHT * i32::from(self.confidence.min(1000)) / 1000;
        match self.outcome {
            Outcome::Complete => magnitude,
            Outcome::Partial => magnitude / 2,
            Outcome::Ineffective => -magnitude,
            Outcome::Aborted
                if matches!(
                    self.reason,
                    OutcomeReason::UnsafeApproach
                        | OutcomeReason::BlockedRoute
                        | OutcomeReason::ObservedCounter
                ) =>
            {
                -magnitude
            }
            Outcome::Aborted | Outcome::Invalidated | Outcome::Inconclusive => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContextEntry {
    key: ExperienceKey,
    score: i32,
    updated_at: Tick,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Experience {
    observed_at: Option<Tick>,
    map: (i32, i32),
    horizon: Tick,
    episodes: Vec<EpisodeReport>,
    contexts: Vec<ContextEntry>,
}

impl Experience {
    pub(crate) fn observe(&mut self, obs: &Observation, memory: Tick) {
        if self.map != (obs.map_width, obs.map_height) {
            *self = Self::default();
            self.map = (obs.map_width, obs.map_height);
        }
        if self.observed_at.is_some_and(|tick| tick > obs.tick) {
            return;
        }
        self.observed_at = Some(obs.tick);
        self.horizon = memory.min(6000);
        self.episodes
            .retain(|report| obs.tick.saturating_sub(report.finished_at) < self.horizon);
        self.contexts
            .retain(|entry| obs.tick.saturating_sub(entry.updated_at) < self.horizon);
    }

    /// Idempotent owner report; a shared credit can affect learning only once.
    pub(crate) fn report(&mut self, mut report: EpisodeReport) {
        let Some(now) = self.observed_at else { return };
        if report.finished_at > now
            || report.started_at > report.finished_at
            || now.saturating_sub(report.finished_at) >= self.horizon
            || self
                .episodes
                .iter()
                .any(|existing| existing.id == report.id)
        {
            return;
        }
        report.confidence = report.confidence.min(1000);
        report.participants.sort_unstable();
        report.participants.dedup();
        if matches!(report.credit.owner, EpisodeOwner::Air | EpisodeOwner::Lift)
            && matches!(
                report.id.owner,
                EpisodeOwner::Ground | EpisodeOwner::LiftAssault
            )
        {
            report.context.doctrine = Doctrine::Air;
        }
        let prior_credit = self
            .episodes
            .iter()
            .filter(|existing| existing.credit == report.credit)
            .max_by_key(|existing| existing.contextual_rank())
            .cloned();
        let prior_doctrine = self
            .episodes
            .iter()
            .filter(|existing| {
                existing.credit == report.credit
                    && existing.doctrine_eligible
                    && existing.confidence >= 750
                    && existing.contribution() != 0
            })
            .map(EpisodeReport::contextual_rank)
            .max();
        if report.doctrine_eligible && report.confidence >= 750 && report.contribution() != 0 {
            if prior_doctrine.is_none_or(|prior| report.contextual_rank() > prior) {
                for existing in self
                    .episodes
                    .iter_mut()
                    .filter(|existing| existing.credit == report.credit)
                {
                    existing.doctrine_eligible = false;
                }
            } else {
                report.doctrine_eligible = false;
            }
        }
        if prior_credit
            .as_ref()
            .is_none_or(|prior| report.contextual_rank() > prior.contextual_rank())
        {
            if let Some(prior) = prior_credit
                && self.contexts.iter().any(|entry| entry.key == prior.context)
            {
                let delta = decay(
                    prior.contribution(),
                    now.saturating_sub(prior.finished_at),
                    self.horizon,
                );
                self.adjust_context(prior.context, -delta, now);
            }
            let delta = decay(
                report.contribution(),
                now.saturating_sub(report.finished_at),
                self.horizon,
            );
            self.adjust_context(report.context, delta, now);
        }
        self.episodes.push(report);
        self.episodes
            .sort_unstable_by_key(|report| (report.finished_at, report.id));
        if self.episodes.len() > EPISODE_LIMIT {
            self.episodes.remove(0);
        }
        self.contexts
            .sort_unstable_by_key(|entry| (entry.updated_at, entry.key));
        while self.contexts.len() > CONTEXT_LIMIT {
            self.contexts.remove(0);
        }
    }

    fn adjust_context(&mut self, key: ExperienceKey, delta: i32, now: Tick) {
        if delta == 0 {
            return;
        }
        if let Some(entry) = self.contexts.iter_mut().find(|entry| entry.key == key) {
            entry.score = (decay(
                entry.score,
                now.saturating_sub(entry.updated_at),
                self.horizon,
            ) + delta)
                .clamp(-SCORE_LIMIT, SCORE_LIMIT);
            entry.updated_at = now;
        } else {
            self.contexts.push(ContextEntry {
                key,
                score: delta,
                updated_at: now,
            });
        }
    }

    pub(crate) fn contextual_score(&self, key: ExperienceKey) -> i16 {
        let now = self.observed_at.unwrap_or(0);
        self.contexts
            .iter()
            .find(|entry| entry.key == key)
            .map_or(0, |entry| {
                decay(
                    entry.score,
                    now.saturating_sub(entry.updated_at),
                    self.horizon,
                ) as i16
            })
    }

    pub(crate) fn doctrine_score(&self, doctrine: Doctrine) -> i16 {
        let now = self.observed_at.unwrap_or(0);
        let qualified = self.episodes.iter().filter(|report| {
            report.context.doctrine == doctrine
                && report.doctrine_eligible
                && report.confidence >= 750
                && report.contribution() != 0
        });
        let (count, score) = qualified.fold((0, 0_i32), |(count, score), report| {
            (
                count + 1,
                score
                    + decay(
                        report.contribution(),
                        now.saturating_sub(report.finished_at),
                        self.horizon,
                    ),
            )
        });
        if count < 2 {
            0
        } else {
            score.clamp(-SCORE_LIMIT, SCORE_LIMIT) as i16
        }
    }

    pub(crate) fn score(&self, key: ExperienceKey) -> i16 {
        (i32::from(self.contextual_score(key)) + i32::from(self.doctrine_score(key.doctrine)))
            .clamp(-SCORE_LIMIT, SCORE_LIMIT) as i16
    }

    pub(crate) fn episodes(&self) -> &[EpisodeReport] {
        &self.episodes
    }

    pub(crate) fn trace(&self) -> super::trace::ExperienceTrace {
        super::trace::ExperienceTrace {
            horizon: self.horizon,
            episodes: self.episodes().to_vec(),
            contexts: self
                .contexts
                .iter()
                .map(|entry| (entry.key, self.contextual_score(entry.key)))
                .collect(),
            doctrine: [
                Doctrine::Pressure,
                Doctrine::Air,
                Doctrine::Siege,
                Doctrine::Expansion,
                Doctrine::Fortification,
                Doctrine::Sustain,
            ]
            .into_iter()
            .map(|doctrine| (doctrine, self.doctrine_score(doctrine)))
            .collect(),
        }
    }
}

fn decay(score: i32, age: Tick, horizon: Tick) -> i32 {
    if horizon == 0 || age >= horizon {
        return 0;
    }
    (i64::from(score) * (horizon - age) as i64 / horizon as i64) as i32
}

/// Presence includes sealed own cargo, but never grants availability to it.
pub(crate) fn own_unit_health(obs: &Observation, id: UnitId) -> Option<u32> {
    obs.my_units
        .iter()
        .find(|unit| unit.id == id)
        .map(|unit| unit.hp)
        .or_else(|| {
            obs.my_carried_units
                .iter()
                .find(|unit| unit.id == id)
                .map(|unit| unit.hp)
        })
}

pub(crate) fn ground_doctrine(obs: &Observation, members: &[UnitId]) -> Doctrine {
    use crate::stats::Role;
    let (siege, total) = obs
        .my_units
        .iter()
        .filter(|unit| members.contains(&unit.id))
        .fold((0_u64, 0_u64), |(siege, total), unit| {
            let cost = u64::from(unit.kind.stats().cost);
            let specialized = matches!(
                unit.kind.role(),
                Role::Lancer | Role::Bombard | Role::Avalanche
            );
            (siege + if specialized { cost } else { 0 }, total + cost)
        });
    if total > 0 && siege.saturating_mul(2) >= total {
        Doctrine::Siege
    } else {
        Doctrine::Pressure
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EpisodeWatch {
    report: EpisodeReport,
    members: Vec<(UnitId, u32)>,
    finished: bool,
    objective: Option<super::observation::BuildingObs>,
    doctrine_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObjectiveWatch {
    watch: EpisodeWatch,
    deadline: Tick,
}

/// Owners explicitly open and finish episodes; disappearance is not a verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OutcomeJournal {
    watch: Option<EpisodeWatch>,
    follow_through: Vec<ObjectiveWatch>,
    pub(crate) pending: Vec<EpisodeReport>,
}

impl OutcomeJournal {
    pub(crate) fn link_handoff(&mut self, source: &Self) {
        let Some(watch) = self.watch.as_mut().filter(|watch| !watch.finished) else {
            return;
        };
        let mut matching = source.follow_through.iter().filter(|prior| {
            prior.watch.report.context.subject == watch.report.context.subject
                && prior
                    .watch
                    .members
                    .iter()
                    .any(|(id, _)| watch.members.iter().any(|(member, _)| member == id))
        });
        let Some(first) = matching.next() else { return };
        let credit = first.watch.report.credit;
        if matching.all(|prior| prior.watch.report.credit == credit) {
            watch.report.credit = credit;
        } else {
            watch.doctrine_allowed = false;
        }
    }

    pub(crate) fn handoff_objective(&mut self, obs: &Observation, participants: &[UnitId]) {
        let Some(mut watch) = self.watch.clone().filter(|watch| watch.objective.is_some()) else {
            return;
        };
        watch.report.id.owner = EpisodeOwner::LiftAssault;
        if self
            .follow_through
            .iter()
            .any(|prior| prior.watch.report.id == watch.report.id)
        {
            return;
        }
        watch.members.retain(|(id, _)| participants.contains(id));
        if watch.members.is_empty() {
            return;
        }
        watch.report.started_at = obs.tick;
        watch.report.phase = 0;
        watch.report.observed_progress = 0;
        watch.finished = false;
        self.follow_through.push(ObjectiveWatch {
            watch,
            deadline: obs.tick.saturating_add(1800),
        });
        self.follow_through
            .sort_unstable_by_key(|watch| (watch.deadline, watch.watch.report.id));
        if self.follow_through.len() > EPISODE_LIMIT {
            self.follow_through.remove(0);
        }
    }

    pub(crate) fn observe_follow_through(&mut self, obs: &Observation) {
        for watch in std::mem::take(&mut self.follow_through) {
            let id = watch
                .watch
                .objective
                .as_ref()
                .expect("objective watch has a baseline")
                .id;
            let lost = watch
                .watch
                .members
                .iter()
                .all(|(id, _)| own_unit_health(obs, *id).is_none_or(|hp| hp == 0));
            let mut journal = Self {
                watch: Some(watch.watch),
                ..Self::default()
            };
            if journal.observe_objective(obs, id) {
                journal.finish(
                    obs,
                    Outcome::Complete,
                    OutcomeReason::ObjectiveObservedGone,
                    750,
                    false,
                );
            } else if lost {
                journal.finish(
                    obs,
                    Outcome::Ineffective,
                    OutcomeReason::RequiredUnitLost,
                    1000,
                    true,
                );
            } else if obs.tick >= watch.deadline {
                let progress = journal
                    .watch
                    .as_ref()
                    .expect("active watch")
                    .report
                    .observed_progress;
                journal.finish(
                    obs,
                    if progress > 0 {
                        Outcome::Partial
                    } else {
                        Outcome::Inconclusive
                    },
                    OutcomeReason::Deadline,
                    500,
                    false,
                );
            } else {
                self.follow_through.push(ObjectiveWatch {
                    watch: journal.watch.take().expect("active watch"),
                    deadline: watch.deadline,
                });
            }
            self.pending.extend(journal.pending);
        }
    }
    pub(crate) fn episode_id(&self) -> Option<EpisodeId> {
        self.watch.as_ref().map(|watch| watch.report.credit)
    }

    pub(crate) fn all_participants_lost(&self, obs: &Observation) -> bool {
        self.watch.as_ref().is_some_and(|watch| {
            !watch.members.is_empty()
                && watch
                    .members
                    .iter()
                    .all(|(id, _)| own_unit_health(obs, *id).is_none_or(|hp| hp == 0))
        })
    }

    fn attributable_objective_progress(&self, obs: &Observation) -> bool {
        let Some(watch) = self
            .watch
            .as_ref()
            .filter(|watch| watch.report.observed_progress > 0)
        else {
            return false;
        };
        let Some(objective) = watch.objective.as_ref() else {
            return false;
        };
        let nearby = |tile: chassis::grid::TilePos| tile.chebyshev(objective.anchor) <= 8;
        watch.members.iter().any(|(id, _)| {
            obs.my_units.iter().any(|unit| {
                unit.id == *id && unit.hp > 0 && unit.kind.stats().can_fight() && nearby(unit.tile)
            })
        }) && !obs.my_units.iter().any(|unit| {
            unit.hp > 0
                && unit.kind.stats().can_fight()
                && nearby(unit.tile)
                && !watch.members.iter().any(|(id, _)| *id == unit.id)
        }) && !obs.ally_units.iter().chain(&obs.enemy_units).any(|unit| {
            unit.player != objective.player
                && unit.hp > 0
                && unit.kind.stats().can_fight()
                && nearby(unit.tile)
        })
    }

    pub(crate) fn share_credit(&mut self, credit: EpisodeId) {
        let id = self.watch.as_ref().map(|watch| watch.report.id);
        if let Some(watch) = self.watch.as_mut() {
            watch.report.credit = credit;
        }
        for report in self
            .pending
            .iter_mut()
            .filter(|report| Some(report.id) == id)
        {
            report.credit = credit;
        }
        for follow in &mut self.follow_through {
            if id.is_some_and(|id| follow.watch.report.id.serial == id.serial) {
                follow.watch.report.credit = credit;
            }
        }
    }

    pub(crate) fn progress(&mut self, amount: u32) {
        if let Some(watch) = self.watch.as_mut().filter(|watch| !watch.finished) {
            watch.report.observed_progress = watch.report.observed_progress.max(amount);
        }
    }

    pub(crate) fn has_progress(&self) -> bool {
        self.watch
            .as_ref()
            .is_some_and(|watch| watch.report.observed_progress > 0)
    }

    pub(crate) fn observe_phase(&mut self, phase: u8) {
        if let Some(watch) = self.watch.as_mut().filter(|watch| !watch.finished) {
            watch.report.phase = phase;
        }
    }

    pub(crate) fn own_lost_value(&self, obs: &Observation) -> u32 {
        self.watch.as_ref().map_or(0, |watch| {
            watch
                .members
                .iter()
                .filter(|(id, _)| own_unit_health(obs, *id).is_none_or(|hp| hp == 0))
                .map(|(_, cost)| cost)
                .sum()
        })
    }

    pub(crate) fn context(&self) -> Option<ExperienceKey> {
        self.watch.as_ref().map(|watch| watch.report.context)
    }

    pub(crate) fn concurrent_ground_credit(
        &self,
        doctrine: Doctrine,
    ) -> Option<(EpisodeId, Doctrine)> {
        let watch = self.watch.as_ref().filter(|watch| !watch.finished)?;
        let prior = watch.report.context;
        let pressure = |doctrine| matches!(doctrine, Doctrine::Pressure | Doctrine::Siege);
        (watch.report.id.owner == EpisodeOwner::Ground
            && (prior.doctrine == doctrine || (pressure(prior.doctrine) && pressure(doctrine))))
        .then_some((watch.report.credit, prior.doctrine))
    }

    pub(crate) fn withhold_doctrine_credit(&mut self) {
        if let Some(watch) = &mut self.watch {
            watch.doctrine_allowed = false;
        }
    }

    pub(crate) fn watch(
        &mut self,
        obs: &Observation,
        id: EpisodeId,
        context: ExperienceKey,
        members: &[UnitId],
        phase: u8,
    ) {
        if self
            .watch
            .as_ref()
            .is_none_or(|watch| watch.report.id != id)
        {
            self.finish(
                obs,
                Outcome::Invalidated,
                OutcomeReason::Preempted,
                1000,
                false,
            );
            self.watch = Some(EpisodeWatch {
                report: EpisodeReport {
                    id,
                    credit: id,
                    context,
                    started_at: obs.tick,
                    finished_at: obs.tick,
                    participants: Vec::new(),
                    phase,
                    outcome: Outcome::Inconclusive,
                    reason: OutcomeReason::LostContact,
                    observed_progress: 0,
                    own_lost_value: 0,
                    confidence: 0,
                    doctrine_eligible: false,
                },
                members: Vec::new(),
                finished: false,
                objective: None,
                doctrine_allowed: true,
            });
        }
        let watch = self.watch.as_mut().expect("opened episode");
        if watch.finished {
            return;
        }
        watch.report.phase = phase;
        for id in members {
            if watch.members.iter().any(|(member, _)| member == id) {
                continue;
            }
            let kind = obs
                .my_units
                .iter()
                .find(|unit| unit.id == *id)
                .map(|unit| unit.kind)
                .or_else(|| {
                    obs.my_carried_units
                        .iter()
                        .find(|unit| unit.id == *id)
                        .map(|unit| unit.kind)
                });
            if let Some(kind) = kind {
                watch.members.push((*id, kind.stats().cost));
            }
        }
        watch.members.sort_unstable_by_key(|(id, _)| *id);
    }

    pub(crate) fn finish(
        &mut self,
        obs: &Observation,
        outcome: Outcome,
        reason: OutcomeReason,
        confidence: u16,
        doctrine_eligible: bool,
    ) {
        let own_lost_value = self.own_lost_value(obs);
        let carried_handoff = reason == OutcomeReason::RequiredUnitLost
            && own_lost_value == 0
            && self.watch.as_ref().is_some_and(|watch| {
                watch.members.iter().any(|(id, _)| {
                    obs.my_carried_units
                        .iter()
                        .any(|passenger| passenger.id == *id)
                })
            });
        let (outcome, reason, doctrine_eligible) = if carried_handoff {
            (Outcome::Invalidated, OutcomeReason::Preempted, false)
        } else {
            (outcome, reason, doctrine_eligible)
        };
        let doctrine_eligible = doctrine_eligible
            || (matches!(outcome, Outcome::Complete | Outcome::Partial)
                && self.attributable_objective_progress(obs));
        let Some(watch) = self.watch.as_mut() else {
            return;
        };
        if watch.finished {
            return;
        }
        watch.finished = true;
        watch.report.finished_at = obs.tick;
        watch.report.outcome = outcome;
        watch.report.reason = reason;
        watch.report.confidence = confidence;
        watch.report.doctrine_eligible = doctrine_eligible && watch.doctrine_allowed;
        watch.report.participants = watch.members.iter().map(|(id, _)| *id).collect();
        watch.report.own_lost_value = own_lost_value;
        self.pending.push(watch.report.clone());
    }

    pub(crate) fn observe_objective(
        &mut self,
        obs: &Observation,
        id: crate::ids::BuildingId,
    ) -> bool {
        let Some(watch) = self.watch.as_mut() else {
            return false;
        };
        if let Some(current) = obs
            .enemy_buildings
            .iter()
            .find(|building| building.id == id && building.seen)
        {
            let baseline = watch.objective.get_or_insert_with(|| current.clone());
            watch.report.observed_progress = watch
                .report
                .observed_progress
                .max(baseline.hp.saturating_sub(current.hp));
            return false;
        }
        watch.objective.as_ref().is_some_and(|baseline| {
            let size = baseline.kind.base_stats().size;
            (0..size.1).all(|dy| (0..size.0).all(|dx| obs.visible(baseline.anchor.offset(dx, dy))))
                && !obs
                    .enemy_buildings
                    .iter()
                    .any(|building| building.id == baseline.id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::observation::CarriedUnitObs;
    use super::*;
    use crate::stats::UnitKind;

    fn report(serial: u64) -> EpisodeReport {
        let id = EpisodeId {
            owner: EpisodeOwner::Ground,
            serial,
        };
        EpisodeReport {
            id,
            credit: id,
            context: ExperienceKey {
                doctrine: Doctrine::Pressure,
                y: 3,
                x: 4,
                subject: 5,
            },
            started_at: 0,
            finished_at: 100,
            participants: vec![UnitId(3), UnitId(1), UnitId(3)],
            phase: 1,
            outcome: Outcome::Ineffective,
            reason: OutcomeReason::ObservedCounter,
            observed_progress: 0,
            own_lost_value: 60,
            confidence: 1000,
            doctrine_eligible: true,
        }
    }

    fn memory() -> Experience {
        let mut memory = Experience::default();
        memory.observe(
            &Observation {
                tick: 100,
                map_width: 32,
                map_height: 32,
                ..Default::default()
            },
            12_000,
        );
        memory
    }

    #[test]
    fn one_failed_route_is_local_and_duplicate_reports_do_not_train() {
        let mut memory = memory();
        let event = report(1);
        memory.report(event.clone());
        memory.report(event.clone());
        assert_eq!(memory.contextual_score(event.context), -256);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), 0);
        assert_eq!(memory.episodes.len(), 1);
        assert_eq!(memory.episodes[0].participants, vec![UnitId(1), UnitId(3)]);
        memory.report(report(2));
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), -512);
        assert_eq!(memory.score(event.context), -1024);
    }

    #[test]
    fn an_assault_result_supersedes_delivery_credit_without_multiplying_it() {
        let mut memory = memory();
        let mut delivery = report(1);
        delivery.id.owner = EpisodeOwner::Lift;
        delivery.credit = delivery.id;
        delivery.context.doctrine = Doctrine::Air;
        delivery.outcome = Outcome::Partial;
        delivery.confidence = 750;
        delivery.doctrine_eligible = false;
        memory.report(delivery.clone());
        assert_eq!(memory.contextual_score(delivery.context), 96);
        let mut assault = report(2);
        assault.credit = delivery.credit;
        memory.report(assault.clone());
        assert_eq!(memory.contextual_score(delivery.context), -256);
        assert_eq!(memory.doctrine_score(Doctrine::Air), 0);
        let mut duplicate = assault;
        duplicate.id.owner = EpisodeOwner::LiftAssault;
        memory.report(duplicate);
        assert_eq!(memory.contextual_score(delivery.context), -256);
        assert_eq!(memory.doctrine_score(Doctrine::Air), 0);
        let mut independent = report(3);
        independent.context.doctrine = Doctrine::Air;
        memory.report(independent);
        assert_eq!(memory.doctrine_score(Doctrine::Air), -512);
    }

    #[test]
    fn shared_handoff_does_not_multiply_credit() {
        let mut memory = memory();
        memory.report(report(1));
        let mut handoff = report(2);
        handoff.credit = report(1).credit;
        handoff.context.doctrine = Doctrine::Air;
        memory.report(handoff.clone());
        assert_eq!(memory.contextual_score(handoff.context), 0);
        assert_eq!(memory.doctrine_score(Doctrine::Air), 0);
        assert_eq!(memory.episodes.len(), 2);
    }

    #[test]
    fn decay_is_linear_and_observation_does_not_compound_it() {
        let mut memory = memory();
        memory.report(report(1));
        for tick in [100, 400, 3100] {
            memory.observe(
                &Observation {
                    tick,
                    map_width: 32,
                    map_height: 32,
                    ..Default::default()
                },
                12_000,
            );
        }
        assert_eq!(memory.contextual_score(report(1).context), -128);
        let before = memory.clone();
        memory.observe(
            &Observation {
                tick: 3000,
                map_width: 32,
                map_height: 32,
                ..Default::default()
            },
            12_000,
        );
        assert_eq!(memory, before);
        memory.observe(
            &Observation {
                tick: 6100,
                map_width: 32,
                map_height: 32,
                ..Default::default()
            },
            12_000,
        );
        assert_eq!(memory.contextual_score(report(1).context), 0);
        assert!(memory.episodes.is_empty());
    }

    #[test]
    fn uncertainty_and_preemption_do_not_teach_failure() {
        for outcome in [
            Outcome::Invalidated,
            Outcome::Inconclusive,
            Outcome::Aborted,
        ] {
            let mut memory = memory();
            let mut event = report(1);
            event.outcome = outcome;
            event.reason = OutcomeReason::Preempted;
            memory.report(event);
            assert_eq!(memory.score(report(1).context), 0);
        }
        let mut memory = memory();
        let mut partial = report(1);
        partial.outcome = Outcome::Partial;
        partial.confidence = 500;
        memory.report(partial);
        assert_eq!(memory.contextual_score(report(1).context), 64);
    }

    #[test]
    fn counterevidence_recovers_and_maps_reset() {
        let mut memory = memory();
        memory.report(report(1));
        memory.report(report(2));
        for serial in 3..7 {
            let mut success = report(serial);
            success.outcome = Outcome::Complete;
            memory.report(success);
        }
        assert!(memory.score(report(1).context) > 0);
        memory.observe(
            &Observation {
                tick: 101,
                map_width: 40,
                map_height: 32,
                ..Default::default()
            },
            12_000,
        );
        assert!(memory.episodes.is_empty());
        assert_eq!(memory.score(report(1).context), 0);
    }

    #[test]
    fn storage_is_bounded_with_canonical_eviction_and_scores_saturate() {
        let mut memory = memory();
        for serial in 0..200 {
            let mut event = report(serial);
            event.context.subject = serial;
            memory.report(event);
        }
        assert_eq!(memory.episodes.len(), EPISODE_LIMIT);
        assert_eq!(memory.contexts.len(), CONTEXT_LIMIT);
        assert_eq!(memory.episodes[0].id.serial, 136);
        assert_eq!(memory.contexts[0].key.subject, 72);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), -1024);
    }

    #[test]
    fn passengers_remain_alive_but_not_available() {
        let obs = Observation {
            my_carried_units: vec![CarriedUnitObs {
                carrier: UnitId(9),
                id: UnitId(1),
                kind: UnitKind::Sentinel,
                hp: 40,
            }],
            ..Default::default()
        };
        assert_eq!(own_unit_health(&obs, UnitId(1)), Some(40));
        assert_eq!(own_unit_health(&obs, UnitId(2)), None);
        assert!(obs.my_units.is_empty());
    }

    #[test]
    fn an_owner_finishes_once_and_a_boarded_participant_is_not_a_loss() {
        let mut obs = Observation {
            tick: 100,
            my_carried_units: vec![CarriedUnitObs {
                carrier: UnitId(9),
                id: UnitId(1),
                kind: UnitKind::Sentinel,
                hp: 100,
            }],
            ..Observation::default()
        };
        let episode = report(1);
        let mut journal = OutcomeJournal::default();
        journal.watch(&obs, episode.id, episode.context, &[UnitId(1)], 1);
        obs.tick = 124;
        journal.finish(
            &obs,
            Outcome::Partial,
            OutcomeReason::ObservedProgress,
            500,
            false,
        );
        assert_eq!(journal.pending[0].own_lost_value, 0);
        assert_eq!(journal.pending[0].participants, [UnitId(1)]);
        let mut preempted = OutcomeJournal::default();
        preempted.watch(&obs, episode.id, episode.context, &[UnitId(1)], 1);
        preempted.finish(
            &obs,
            Outcome::Ineffective,
            OutcomeReason::RequiredUnitLost,
            1000,
            true,
        );
        assert_eq!(preempted.pending[0].outcome, Outcome::Invalidated);
        assert_eq!(preempted.pending[0].reason, OutcomeReason::Preempted);
        assert_eq!(preempted.pending[0].own_lost_value, 0);
        assert!(!preempted.pending[0].doctrine_eligible);
        journal.pending.clear();
        for tick in [148, 172, 196] {
            obs.tick = tick;
            journal.watch(&obs, episode.id, episode.context, &[UnitId(1)], 2);
            journal.finish(
                &obs,
                Outcome::Complete,
                OutcomeReason::ServiceCompleted,
                1000,
                true,
            );
            assert!(journal.pending.is_empty());
        }
    }

    #[test]
    fn delivery_watch_links_ground_credit_without_claiming_or_inventing_success() {
        use crate::{BuildingId, BuildingKind, PlayerId};
        use chassis::grid::TilePos;
        let mut obs = Observation {
            tick: 100,
            map_width: 20,
            map_height: 20,
            visible: vec![true; 400],
            my_carried_units: vec![CarriedUnitObs {
                carrier: UnitId(9),
                id: UnitId(1),
                kind: UnitKind::Sentinel,
                hp: 40,
            }],
            enemy_buildings: vec![super::super::observation::BuildingObs {
                id: BuildingId(5),
                player: PlayerId(1),
                kind: BuildingKind::Foundry,
                anchor: TilePos::new(5, 5),
                hp: 1000,
                built: true,
                seen: true,
                tier: 0,
            }],
            ..Observation::default()
        };
        let mut delivery = OutcomeJournal::default();
        let mut episode = report(1);
        episode.id.owner = EpisodeOwner::Lift;
        delivery.watch(&obs, episode.id, episode.context, &[UnitId(1)], 1);
        delivery.observe_objective(&obs, BuildingId(5));
        delivery.handoff_objective(&obs, &[UnitId(1)]);
        delivery.finish(
            &obs,
            Outcome::Partial,
            OutcomeReason::ObservedProgress,
            750,
            false,
        );
        let mut ground = OutcomeJournal::default();
        ground.watch(&obs, report(2).id, episode.context, &[UnitId(1)], 1);
        ground.link_handoff(&delivery);
        assert_eq!(ground.episode_id(), Some(episode.id));
        delivery.pending.clear();
        obs.tick = 200;
        delivery.observe_follow_through(&obs);
        assert!(delivery.pending.is_empty());
        assert!(!delivery.all_participants_lost(&obs));
        obs.my_carried_units.clear();
        obs.tick = 224;
        delivery.observe_follow_through(&obs);
        assert_eq!(delivery.pending.len(), 1);
        assert_eq!(delivery.pending[0].outcome, Outcome::Ineffective);
        assert_eq!(delivery.pending[0].credit, episode.id);
        assert!(delivery.follow_through.is_empty());
        let mut experience = Experience::default();
        experience.observe(&obs, 6000);
        experience.report(delivery.pending[0].clone());
        ground.finish(
            &obs,
            Outcome::Ineffective,
            OutcomeReason::RequiredUnitLost,
            1000,
            true,
        );
        experience.report(ground.pending[0].clone());
        assert_eq!(
            experience.contextual_score(ExperienceKey {
                doctrine: Doctrine::Air,
                ..episode.context
            }),
            -256
        );
        assert_eq!(experience.doctrine_score(Doctrine::Pressure), 0);
    }

    #[test]
    fn an_unresolved_handoff_expires_inconclusively_and_preserves_new_work() {
        use crate::{BuildingId, BuildingKind, PlayerId};
        use chassis::grid::TilePos;
        let mut obs = Observation {
            tick: 100,
            map_width: 20,
            map_height: 20,
            visible: vec![true; 400],
            my_carried_units: vec![CarriedUnitObs {
                carrier: UnitId(9),
                id: UnitId(1),
                kind: UnitKind::Sentinel,
                hp: 40,
            }],
            enemy_buildings: vec![super::super::observation::BuildingObs {
                id: BuildingId(5),
                player: PlayerId(1),
                kind: BuildingKind::Foundry,
                anchor: TilePos::new(5, 5),
                hp: 1000,
                built: true,
                seen: true,
                tier: 0,
            }],
            ..Observation::default()
        };
        let mut journal = OutcomeJournal::default();
        journal.watch(&obs, report(1).id, report(1).context, &[UnitId(1)], 1);
        journal.observe_objective(&obs, BuildingId(5));
        journal.handoff_objective(&obs, &[UnitId(1)]);
        journal.watch(&obs, report(2).id, report(2).context, &[UnitId(1)], 0);
        assert_eq!(journal.pending[0].outcome, Outcome::Invalidated);
        journal.pending.clear();
        obs.tick = 1900;
        obs.visible.fill(false);
        obs.enemy_buildings.clear();
        journal.observe_follow_through(&obs);
        assert_eq!(journal.pending.len(), 1);
        assert_eq!(journal.pending[0].outcome, Outcome::Inconclusive);
        assert_eq!(journal.pending[0].own_lost_value, 0);
        assert_eq!(journal.episode_id(), Some(report(2).id));
        assert!(journal.follow_through.is_empty());
        journal.observe_follow_through(&obs);
        assert_eq!(journal.pending.len(), 1);
    }

    #[test]
    fn objective_progress_requires_current_sight_and_complete_negative_footprint() {
        use super::super::observation::BuildingObs;
        use crate::{BuildingId, BuildingKind, PlayerId};
        use chassis::grid::TilePos;
        let mut obs = Observation {
            tick: 100,
            map_width: 20,
            map_height: 20,
            visible: vec![true; 400],
            enemy_buildings: vec![BuildingObs {
                id: BuildingId(1),
                player: PlayerId(1),
                kind: BuildingKind::Foundry,
                anchor: TilePos::new(5, 5),
                hp: 1000,
                built: true,
                seen: true,
                tier: 0,
            }],
            ..Observation::default()
        };
        let mut journal = OutcomeJournal::default();
        let episode = report(1);
        journal.watch(&obs, episode.id, episode.context, &[], 1);
        assert!(!journal.observe_objective(&obs, BuildingId(1)));
        obs.enemy_buildings[0].hp = 800;
        assert!(!journal.observe_objective(&obs, BuildingId(1)));
        assert_eq!(
            journal.watch.as_ref().unwrap().report.observed_progress,
            200
        );
        obs.enemy_buildings.clear();
        obs.visible[6 * 20 + 6] = false;
        assert!(!journal.observe_objective(&obs, BuildingId(1)));
        obs.visible[6 * 20 + 6] = true;
        assert!(journal.observe_objective(&obs, BuildingId(1)));
    }
}
