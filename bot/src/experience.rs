//! Bounded, attributable experience. Reports come from owners, never from traces.

#[cfg(test)]
use crate::observation::ObservationData;
use chassis::Tick;
use oxide_sim::ids::{BuildingId, UnitId};
use oxide_sim::stats::{BuildingKind, UnitKind};
use serde::Serialize;

use super::observation::Observation;

const EPISODE_LIMIT: usize = 64;
const CONTEXT_LIMIT: usize = 128;
const SCORE_LIMIT: i32 = 1024;
const EPISODE_WEIGHT: i32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
pub(crate) enum Doctrine {
    Pressure,
    Air,
    Siege,
    Expansion,
    Fortification,
    Sustain,
}

impl Doctrine {
    fn index(self) -> usize {
        match self {
            Self::Pressure => 0,
            Self::Air => 1,
            Self::Siege => 2,
            Self::Expansion => 3,
            Self::Fortification => 4,
            Self::Sustain => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
pub(crate) struct EpisodeId {
    pub(crate) owner: EpisodeOwner,
    pub(crate) serial: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
pub(crate) struct ExperienceKey {
    pub(crate) doctrine: Doctrine,
    pub(crate) y: i32,
    pub(crate) x: i32,
    pub(crate) subject: ExperienceSubject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
pub(crate) enum ExperienceSubject {
    Building(Option<BuildingId>),
    Unit(UnitId),
    Construction(BuildingKind),
    Production(UnitKind),
    Upgrade(BuildingId),
    Harvest,
    Reconnaissance,
}

impl ExperienceKey {
    pub(crate) fn construction(kind: BuildingKind, anchor: chassis::grid::TilePos) -> Self {
        Self {
            doctrine: match kind {
                BuildingKind::RepairBay => Doctrine::Sustain,
                BuildingKind::Turret
                | BuildingKind::FlakTurret
                | BuildingKind::Bastion
                | BuildingKind::ScuttleCharge
                | BuildingKind::Barricade
                | BuildingKind::Array => Doctrine::Fortification,
                _ => Doctrine::Expansion,
            },
            x: anchor.x,
            y: anchor.y,
            subject: ExperienceSubject::Construction(kind),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
pub(crate) enum Outcome {
    Complete,
    Partial,
    Aborted,
    Ineffective,
    Invalidated,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub(crate) struct EpisodeReport {
    pub(crate) id: EpisodeId,
    /// Coordinated components share one credit identity, even after handoff.
    pub(crate) credit: EpisodeId,
    pub(crate) context: ExperienceKey,
    /// Frozen observed site, independent of a remembered building's placeholder id.
    pub(crate) objective: Option<super::executive::ArmyObjective>,
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
    pub(crate) fn has_uncertain_losses(&self) -> bool {
        self.outcome == Outcome::Inconclusive
            && matches!(
                self.reason,
                OutcomeReason::LostContact | OutcomeReason::Deadline
            )
            && self.own_lost_value > 0
    }

    fn contextual_rank(&self) -> (u8, u16, Tick, std::cmp::Reverse<EpisodeId>) {
        let quality = match self.outcome {
            Outcome::Complete | Outcome::Ineffective => 3,
            Outcome::Aborted => 2,
            Outcome::Partial => 1,
            Outcome::Inconclusive if self.has_uncertain_losses() => 1,
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
        // Own casualties remain certain even when the objective and attacker are unknown.
        if self.has_uncertain_losses() {
            return -EPISODE_WEIGHT / 2;
        }
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ContextEntry {
    key: ExperienceKey,
    #[serde(deserialize_with = "crate::checkpoint::bounded_vec::<_, _, EPISODE_LIMIT>")]
    contributions: Vec<ContextContribution>,
    updated_at: Tick,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ContextContribution {
    credit: EpisodeId,
    evidence: Evidence,
    doctrine: Option<(Doctrine, Evidence)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Evidence {
    rank: (u8, u16, Tick, std::cmp::Reverse<EpisodeId>),
    score: i32,
}

impl Evidence {
    fn from_report(report: &EpisodeReport) -> Self {
        Self {
            rank: report.contextual_rank(),
            score: report.contribution(),
        }
    }

    fn finished_at(self) -> Tick {
        self.rank.2
    }

    fn reported_by(self, id: EpisodeId) -> bool {
        self.rank.3 == std::cmp::Reverse(id)
    }

    fn current(self, now: Tick, horizon: Tick) -> bool {
        now.saturating_sub(self.finished_at()) < horizon
    }

    fn score_at(self, now: Tick, horizon: Tick) -> i32 {
        decay(self.score, now.saturating_sub(self.finished_at()), horizon)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Experience {
    observed_at: Option<Tick>,
    map: (i32, i32),
    horizon: Tick,
    #[serde(deserialize_with = "crate::checkpoint::bounded_vec::<_, _, EPISODE_LIMIT>")]
    episodes: Vec<EpisodeReport>,
    #[serde(deserialize_with = "crate::checkpoint::bounded_vec::<_, _, CONTEXT_LIMIT>")]
    contexts: Vec<ContextEntry>,
    doctrine_scores: [i16; 6],
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
        self.contexts.retain_mut(|entry| {
            entry.contributions.retain_mut(|contribution| {
                contribution.doctrine = contribution
                    .doctrine
                    .filter(|(_, evidence)| evidence.current(obs.tick, self.horizon));
                contribution.evidence.current(obs.tick, self.horizon)
                    || contribution.doctrine.is_some()
            });
            !entry.contributions.is_empty()
        });
        self.refresh_doctrine();
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
            || self
                .contexts
                .iter()
                .flat_map(|entry| &entry.contributions)
                .any(|value| {
                    value.evidence.reported_by(report.id)
                        || value
                            .doctrine
                            .is_some_and(|(_, evidence)| evidence.reported_by(report.id))
                })
        {
            return;
        }
        report.confidence = report.confidence.min(1000);
        if report.has_uncertain_losses() {
            report.doctrine_eligible = false;
        }
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
        self.record_context(&report, now);
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
        self.refresh_doctrine();
    }

    fn record_context(&mut self, report: &EpisodeReport, now: Tick) {
        let evidence = Evidence::from_report(report);
        let mut key = report.context;
        let mut contribution = ContextContribution {
            credit: report.credit,
            evidence,
            doctrine: None,
        };
        let prior = self.contexts.iter().find_map(|entry| {
            entry
                .contributions
                .iter()
                .find(|value| value.credit == report.credit)
                .map(|value| (entry.key, value.clone()))
        });
        if let Some((prior_key, previous)) = &prior {
            contribution = previous.clone();
            if contribution.evidence.current(now, self.horizon)
                && (contribution.evidence.reported_by(report.id)
                    || contribution.evidence.rank >= evidence.rank)
            {
                key = *prior_key;
            } else {
                contribution.evidence = evidence;
            }
        } else if evidence.rank.0 == 0 {
            return;
        }
        if report.doctrine_eligible
            && report.confidence >= 750
            && evidence.score != 0
            && contribution.doctrine.is_none_or(|(_, prior)| {
                !prior.reported_by(report.id) && evidence.rank > prior.rank
            })
        {
            contribution.doctrine = Some((report.context.doctrine, evidence));
        }
        if prior
            .as_ref()
            .is_some_and(|(prior_key, previous)| *prior_key == key && *previous == contribution)
        {
            return;
        }
        self.contexts.retain_mut(|entry| {
            entry
                .contributions
                .retain(|value| value.credit != report.credit);
            !entry.contributions.is_empty()
        });
        if let Some(entry) = self.contexts.iter_mut().find(|entry| entry.key == key) {
            entry.contributions.push(contribution);
            entry
                .contributions
                .sort_unstable_by_key(|value| (value.evidence.finished_at(), value.credit));
            if entry.contributions.len() > EPISODE_LIMIT {
                let (oldest, _) = entry
                    .contributions
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, value)| {
                        let newest = value.evidence.finished_at().max(
                            value
                                .doctrine
                                .map_or(0, |(_, evidence)| evidence.finished_at()),
                        );
                        (newest, value.credit)
                    })
                    .expect("overfull context has contributions");
                entry.contributions.remove(oldest);
            }
            entry.updated_at = now;
        } else {
            self.contexts.push(ContextEntry {
                key,
                contributions: vec![contribution],
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
                entry
                    .contributions
                    .iter()
                    .fold(0_i32, |score, contribution| {
                        score
                            .saturating_add(decay(
                                contribution.evidence.score,
                                now.saturating_sub(contribution.evidence.finished_at()),
                                self.horizon,
                            ))
                            .clamp(-SCORE_LIMIT, SCORE_LIMIT)
                    }) as i16
            })
    }

    fn refresh_doctrine(&mut self) {
        let now = self.observed_at.unwrap_or(0);
        let mut values = [(0, 0_i64); 6];
        for (doctrine, evidence) in self
            .contexts
            .iter()
            .flat_map(|entry| &entry.contributions)
            .filter_map(|contribution| contribution.doctrine)
        {
            let (count, score) = &mut values[doctrine.index()];
            *count += 1;
            *score = score.saturating_add(i64::from(evidence.score_at(now, self.horizon)));
        }
        self.doctrine_scores = values.map(|(count, score)| {
            if count < 2 {
                0
            } else {
                score.clamp(-i64::from(SCORE_LIMIT), i64::from(SCORE_LIMIT)) as i16
            }
        });
    }

    pub(crate) fn doctrine_score(&self, doctrine: Doctrine) -> i16 {
        self.doctrine_scores[doctrine.index()]
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
    (i128::from(score) * i128::from(horizon - age) / i128::from(horizon)) as i32
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
    use oxide_sim::stats::Role;
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct EpisodeWatch<O = Option<super::observation::BuildingObs>> {
    report: EpisodeReport,
    members: Vec<(UnitId, u32)>,
    finished: bool,
    objective: O,
    doctrine_allowed: bool,
    ground_contact_losses: (u32, bool),
}

impl<O> EpisodeWatch<O> {
    fn map_objective<T>(self, map: impl FnOnce(O) -> T) -> EpisodeWatch<T> {
        EpisodeWatch {
            report: self.report,
            members: self.members,
            finished: self.finished,
            objective: map(self.objective),
            doctrine_allowed: self.doctrine_allowed,
            ground_contact_losses: self.ground_contact_losses,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ObjectiveWatch {
    watch: EpisodeWatch<super::observation::BuildingObs>,
    deadline: Tick,
}

/// Owners explicitly open and finish episodes; disappearance is not a verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct OutcomeJournal {
    watch: Option<EpisodeWatch>,
    #[serde(deserialize_with = "crate::checkpoint::bounded_vec::<_, _, EPISODE_LIMIT>")]
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
        let Some(watch) = self.watch.clone() else {
            return;
        };
        let Some(objective) = watch.objective.clone() else {
            return;
        };
        let mut watch = watch.map_objective(|_| objective);
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
            let baseline = watch.watch.objective.clone();
            let id = baseline.id;
            let lost = watch
                .watch
                .members
                .iter()
                .all(|(id, _)| own_unit_health(obs, *id).is_none_or(|hp| hp == 0));
            let mut journal = Self {
                watch: Some(watch.watch.map_objective(Some)),
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
                    watch: journal
                        .watch
                        .take()
                        .expect("active watch")
                        .map_objective(|_| baseline),
                    deadline: watch.deadline,
                });
            }
            self.pending.extend(journal.pending);
        }
    }
    pub(crate) fn episode_id(&self) -> Option<EpisodeId> {
        self.watch.as_ref().map(|watch| watch.report.credit)
    }

    pub(crate) fn objective(&self) -> Option<super::executive::ArmyObjective> {
        self.watch.as_ref()?.report.objective
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
                .fold(0_u32, |lost, (_, cost)| lost.saturating_add(*cost))
        })
    }

    pub(crate) fn observe_ground_contact(&mut self, obs: &Observation, contact: bool) -> bool {
        let lost = self.own_lost_value(obs);
        let Some(watch) = self.watch.as_mut().filter(|watch| !watch.finished) else {
            return false;
        };
        let (previous_loss, previous_contact) = watch.ground_contact_losses;
        watch.ground_contact_losses = (lost, contact);
        lost > previous_loss && !contact && !previous_contact
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
                    objective: None,
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
                ground_contact_losses: (0, false),
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
        id: oxide_sim::ids::BuildingId,
    ) -> bool {
        let Some(watch) = self.watch.as_mut() else {
            return false;
        };
        if let Some(current) = obs
            .enemy_buildings
            .iter()
            .find(|building| building.id == id && building.seen)
        {
            watch
                .report
                .objective
                .get_or_insert_with(|| super::executive::ArmyObjective::from_building(current));
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

    pub(crate) fn bind_objective(&mut self, objective: super::executive::ArmyObjective) {
        if let Some(watch) = self.watch.as_mut().filter(|watch| !watch.finished) {
            watch.report.objective.get_or_insert(objective);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::observation::CarriedUnitObs;
    use super::*;
    use oxide_sim::stats::UnitKind;

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
                subject: ExperienceSubject::Building(Some(BuildingId(5))),
            },
            objective: None,
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
            &Observation::from_data(ObservationData {
                tick: 100,
                map_width: 32,
                map_height: 32,
                ..crate::test_support::observation_data()
            }),
            12_000,
        );
        memory
    }

    #[test]
    fn restored_extreme_evidence_has_bounded_scores_without_overflow() {
        for score in [i32::MIN, i32::MAX] {
            let mut memory = memory();
            memory.report(report(1));
            memory.report(report(2));
            for contribution in &mut memory.contexts[0].contributions {
                contribution.evidence.score = score;
                contribution.doctrine.as_mut().unwrap().1.score = score;
            }
            let mut memory = crate::checkpoint::round_trip(&memory);
            memory.refresh_doctrine();
            let expected = if score < 0 { -1024 } else { 1024 };
            assert_eq!(memory.contextual_score(report(1).context), expected);
            assert_eq!(memory.doctrine_score(Doctrine::Pressure), expected);
            for horizon in [1, 6000, i64::MAX as u64, u64::MAX] {
                assert_eq!(decay(score, 0, horizon), score);
                assert_eq!(decay(score, horizon, horizon), 0);
            }
            assert_eq!(decay(score, u64::MAX / 2, u64::MAX), score / 2);
        }
    }

    #[test]
    fn restored_learning_collections_respect_runtime_storage_bounds() {
        let mut original = memory();
        original.report(report(1));
        for (field, limit) in [("episodes", EPISODE_LIMIT), ("contexts", CONTEXT_LIMIT)] {
            let mut wire = serde_json::to_value(&original).unwrap();
            let item = wire[field][0].clone();
            wire[field] = serde_json::json!(vec![item.clone(); limit]);
            assert!(serde_json::from_value::<Experience>(wire.clone()).is_ok());
            wire[field].as_array_mut().unwrap().push(item);
            assert!(serde_json::from_value::<Experience>(wire).is_err());
        }
        let mut wire = serde_json::to_value(&original).unwrap();
        let rows = &mut wire["contexts"][0]["contributions"];
        let contribution = rows[0].clone();
        *rows = serde_json::json!(vec![contribution.clone(); EPISODE_LIMIT]);
        assert!(serde_json::from_value::<Experience>(wire.clone()).is_ok());
        wire["contexts"][0]["contributions"]
            .as_array_mut()
            .unwrap()
            .push(contribution);
        assert!(serde_json::from_value::<Experience>(wire).is_err());
    }

    #[test]
    fn restored_casualty_costs_saturate_when_an_episode_finishes() {
        let obs = Observation::from_data(crate::test_support::observation_data());
        let episode = report(1);
        let mut journal = OutcomeJournal::default();
        journal.watch(&obs, episode.id, episode.context, &[], 0);
        journal.watch.as_mut().unwrap().members = vec![
            (UnitId(u32::MAX - 1), u32::MAX),
            (UnitId(u32::MAX), u32::MAX),
        ];
        let mut restored = crate::checkpoint::round_trip(&journal);
        restored.finish(
            &obs,
            Outcome::Ineffective,
            OutcomeReason::RequiredUnitLost,
            1000,
            true,
        );
        assert_eq!(restored.pending[0].own_lost_value, u32::MAX);
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
                &Observation::from_data(ObservationData {
                    tick,
                    map_width: 32,
                    map_height: 32,
                    ..crate::test_support::observation_data()
                }),
                12_000,
            );
        }
        assert_eq!(memory.contextual_score(report(1).context), -128);
        let before = memory.clone();
        memory.observe(
            &Observation::from_data(ObservationData {
                tick: 3000,
                map_width: 32,
                map_height: 32,
                ..crate::test_support::observation_data()
            }),
            12_000,
        );
        assert_eq!(memory, before);
        memory.observe(
            &Observation::from_data(ObservationData {
                tick: 6100,
                map_width: 32,
                map_height: 32,
                ..crate::test_support::observation_data()
            }),
            12_000,
        );
        assert_eq!(memory.contextual_score(report(1).context), 0);
        assert!(memory.episodes.is_empty());
    }

    #[test]
    fn uncertain_objective_retains_local_casualty_evidence_without_doctrine() {
        let mut memory = memory();
        for serial in 1..=2 {
            let mut event = report(serial);
            event.outcome = Outcome::Inconclusive;
            event.reason = OutcomeReason::LostContact;
            event.confidence = 0;
            memory.report(event);
        }
        assert_eq!(memory.contextual_score(report(1).context), -256);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), 0);
        assert!(
            memory
                .episodes
                .iter()
                .all(|episode| !episode.doctrine_eligible)
        );
        let mut empty_followup = memory.episodes[0].clone();
        empty_followup.own_lost_value = 0;
        empty_followup.finished_at += 12;
        assert!(memory.episodes[0].contextual_rank() > empty_followup.contextual_rank());

        let mut event = report(3);
        event.context.x += 1;
        event.outcome = Outcome::Inconclusive;
        event.reason = OutcomeReason::LostContact;
        event.own_lost_value = 0;
        event.confidence = 0;
        let context = event.context;
        memory.report(event);
        assert_eq!(memory.contextual_score(context), 0);
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
    fn later_reports_do_not_renew_older_context_contributions() {
        let mut memory = memory();
        let key = report(1).context;
        memory.report(report(1));
        let mut obs = Observation::from_data(ObservationData {
            tick: 3100,
            map_width: 32,
            map_height: 32,
            ..crate::test_support::observation_data()
        });
        memory.observe(&obs, 6000);
        let mut later = report(2);
        later.finished_at = obs.tick;
        memory.report(later);
        assert_eq!(memory.contextual_score(key), -384);
        obs.tick = 6100;
        memory.observe(&obs, 6000);
        assert_eq!(memory.episodes.len(), 1);
        assert_eq!(memory.contextual_score(key), -128);
        obs.tick = 9100;
        memory.observe(&obs, 6000);
        assert_eq!(memory.contextual_score(key), 0);
        assert!(memory.contexts.is_empty());
    }

    #[test]
    fn replacing_shared_credit_preserves_independent_contribution_ages() {
        let mut memory = memory();
        let mut delivery = report(1);
        delivery.outcome = Outcome::Partial;
        memory.report(delivery.clone());
        memory.report(report(2));
        let mut obs = Observation::from_data(ObservationData {
            tick: 3100,
            map_width: 32,
            map_height: 32,
            ..crate::test_support::observation_data()
        });
        memory.observe(&obs, 6000);
        let mut assault = report(3);
        assault.credit = delivery.credit;
        assault.finished_at = obs.tick;
        memory.report(assault.clone());
        assert_eq!(memory.contextual_score(delivery.context), -384);
        assert_eq!(memory.contexts[0].contributions.len(), 2);
        memory.report(assault);
        assert_eq!(memory.contextual_score(delivery.context), -384);
        obs.tick = 6100;
        memory.observe(&obs, 6000);
        assert_eq!(memory.contextual_score(delivery.context), -128);
        assert_eq!(memory.contexts[0].contributions[0].credit, delivery.credit);
    }

    #[test]
    fn context_contributions_are_bounded_and_counterevidence_breaks_saturation() {
        let mut memory = memory();
        for serial in 0..200 {
            memory.report(report(serial));
        }
        assert_eq!(memory.contexts.len(), 1);
        assert_eq!(memory.contexts[0].contributions.len(), EPISODE_LIMIT);
        assert_eq!(memory.contexts[0].contributions[0].credit.serial, 136);
        assert_eq!(memory.contextual_score(report(0).context), -1024);
        let mut success = report(200);
        success.outcome = Outcome::Complete;
        memory.report(success);
        assert_eq!(memory.contextual_score(report(0).context), -768);
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
            &Observation::from_data(ObservationData {
                tick: 101,
                map_width: 40,
                map_height: 32,
                ..crate::test_support::observation_data()
            }),
            12_000,
        );
        assert!(memory.episodes.is_empty());
        assert_eq!(memory.score(report(1).context), 0);
    }

    #[test]
    fn capacity_eviction_uses_both_evidence_times_without_reordering_scores() {
        for (contextual_tick, doctrine_tick, contextual_score, doctrine_score) in
            [(1, 200, 1024, -128), (200, 1, 768, -133)]
        {
            let mut memory = memory();
            let mut obs = crate::test_support::observation_data();
            obs.map_width = 32;
            obs.map_height = 32;
            obs.tick = 200;
            memory.observe(&Observation::from_data(obs), 6000);
            let mut original = report(1);
            original.finished_at = contextual_tick;
            original.doctrine_eligible = false;
            memory.report(original.clone());
            for serial in 2..=64 {
                let mut other = report(serial);
                other.finished_at = 2;
                other.outcome = Outcome::Complete;
                other.doctrine_eligible = false;
                memory.report(other);
            }
            let mut later = report(100);
            later.credit = original.credit;
            later.finished_at = doctrine_tick;
            later.outcome = Outcome::Partial;
            memory.report(later);
            let mut independent = report(101);
            independent.context.x += 1;
            independent.finished_at = 200;
            memory.report(independent);
            assert_eq!(memory.doctrine_score(Doctrine::Pressure), doctrine_score);
            assert_eq!(memory.contextual_score(original.context), contextual_score);

            let mut overflow = report(65);
            overflow.finished_at = 65;
            overflow.outcome = Outcome::Aborted;
            overflow.reason = OutcomeReason::Preempted;
            overflow.doctrine_eligible = false;
            memory.report(overflow);
            assert_eq!(memory.doctrine_score(Doctrine::Pressure), doctrine_score);
            assert_eq!(memory.contextual_score(original.context), contextual_score);
            let entry = memory
                .contexts
                .iter()
                .find(|entry| entry.key == original.context)
                .unwrap();
            assert_eq!(entry.contributions.len(), EPISODE_LIMIT);
            assert!(
                !entry
                    .contributions
                    .iter()
                    .any(|value| value.credit == report(2).credit)
            );
            assert!(
                entry
                    .contributions
                    .iter()
                    .any(|value| value.credit == report(3).credit)
            );
        }
    }

    #[test]
    fn storage_is_bounded_with_canonical_eviction_and_scores_saturate() {
        let mut memory = memory();
        for serial in 0..200 {
            let mut event = report(serial);
            event.context.subject = ExperienceSubject::Building(Some(BuildingId(serial as u32)));
            memory.report(event);
        }
        assert_eq!(memory.episodes.len(), EPISODE_LIMIT);
        assert_eq!(memory.contexts.len(), CONTEXT_LIMIT);
        assert_eq!(memory.episodes[0].id.serial, 136);
        assert_eq!(
            memory.contexts[0].key.subject,
            ExperienceSubject::Building(Some(BuildingId(72)))
        );
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), -1024);
    }

    #[test]
    fn retained_credit_outlives_unrelated_report_history() {
        let mut memory = memory();
        let original = report(1);
        memory.report(original.clone());
        memory.report(report(2));
        for serial in 3..=66 {
            let mut neutral = report(serial);
            neutral.outcome = Outcome::Invalidated;
            memory.report(neutral);
        }
        assert!(
            !memory
                .episodes()
                .iter()
                .any(|event| event.id == original.id)
        );
        assert_eq!(memory.contextual_score(original.context), -512);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), -512);

        let mut weaker = report(67);
        weaker.credit = original.credit;
        weaker.outcome = Outcome::Partial;
        weaker.context.x += 1;
        memory.report(weaker.clone());
        assert_eq!(memory.contextual_score(original.context), -512);
        assert_eq!(memory.contextual_score(weaker.context), 0);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), -512);

        let mut duplicate = original;
        duplicate.outcome = Outcome::Complete;
        memory.report(duplicate);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), -512);
    }

    #[test]
    fn non_scoring_evidence_still_prevents_a_weaker_followup() {
        let mut memory = memory();
        let mut preempted = report(1);
        preempted.outcome = Outcome::Aborted;
        preempted.reason = OutcomeReason::Preempted;
        memory.report(preempted.clone());
        for serial in 2..=65 {
            let mut neutral = report(serial);
            neutral.outcome = Outcome::Invalidated;
            memory.report(neutral);
        }
        let mut weaker = report(66);
        weaker.credit = preempted.credit;
        weaker.outcome = Outcome::Partial;
        weaker.doctrine_eligible = false;
        memory.report(weaker);
        assert_eq!(memory.contextual_score(preempted.context), 0);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), 0);
    }

    #[test]
    fn stronger_shared_evidence_moves_only_its_credit_after_history_eviction() {
        let mut memory = memory();
        let mut delivery = report(1);
        delivery.outcome = Outcome::Partial;
        delivery.doctrine_eligible = false;
        memory.report(delivery.clone());
        memory.report(report(2));
        for serial in 3..=66 {
            let mut neutral = report(serial);
            neutral.outcome = Outcome::Invalidated;
            memory.report(neutral);
        }
        let mut assault = report(67);
        assault.credit = delivery.credit;
        assault.context.x += 1;
        memory.report(assault.clone());
        assert_eq!(memory.contextual_score(delivery.context), -256);
        assert_eq!(memory.contextual_score(assault.context), -256);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), -512);
        let mut weaker = report(68);
        weaker.credit = delivery.credit;
        weaker.outcome = Outcome::Partial;
        memory.report(weaker);
        assert_eq!(memory.contextual_score(delivery.context), -256);
        assert_eq!(memory.contextual_score(assault.context), -256);
    }

    #[test]
    fn context_and_doctrine_evidence_expire_at_their_own_completion_times() {
        let mut memory = memory();
        let mut original = report(1);
        original.doctrine_eligible = false;
        memory.report(original.clone());
        let mut obs = crate::test_support::observation_data();
        obs.map_width = 32;
        obs.map_height = 32;
        obs.tick = 3100;
        memory.observe(&Observation::from_data(obs.clone()), 6000);
        let mut later = report(2);
        later.credit = original.credit;
        later.finished_at = 3100;
        later.outcome = Outcome::Partial;
        memory.report(later);
        let mut independent = report(3);
        independent.finished_at = 3100;
        memory.report(independent);
        assert_eq!(memory.contextual_score(original.context), -384);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), -128);
        memory = crate::checkpoint::round_trip(&memory);
        obs.tick = 6100;
        memory.observe(&Observation::from_data(obs.clone()), 6000);
        assert_eq!(memory.contextual_score(original.context), -128);
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), -64);
        obs.tick = 9100;
        memory.observe(&Observation::from_data(obs), 6000);
        assert!(memory.contexts.is_empty());
        assert_eq!(memory.doctrine_score(Doctrine::Pressure), 0);
    }

    #[test]
    fn equal_numeric_subjects_do_not_transfer_contextual_credit() {
        let mut memory = memory();
        let event = report(1);
        memory.report(event.clone());
        for subject in [
            ExperienceSubject::Unit(UnitId(5)),
            ExperienceSubject::Construction(BuildingKind::Array),
            ExperienceSubject::Production(UnitKind::Flakhound),
            ExperienceSubject::Upgrade(BuildingId(5)),
            ExperienceSubject::Building(None),
            ExperienceSubject::Harvest,
            ExperienceSubject::Reconnaissance,
        ] {
            assert_eq!(
                memory.contextual_score(ExperienceKey {
                    subject,
                    ..event.context
                }),
                0
            );
        }
        assert_eq!(memory.contextual_score(event.context), -256);
    }

    #[test]
    fn passengers_remain_alive_but_not_available() {
        let obs = Observation::from_data(ObservationData {
            my_carried_units: vec![CarriedUnitObs {
                carrier: UnitId(9),
                id: UnitId(1),
                kind: UnitKind::Sentinel,
                hp: 40,
            }],
            ..crate::test_support::observation_data()
        });
        assert_eq!(own_unit_health(&obs, UnitId(1)), Some(40));
        assert_eq!(own_unit_health(&obs, UnitId(2)), None);
        assert!(obs.my_units.is_empty());
    }

    #[test]
    fn an_owner_finishes_once_and_a_boarded_participant_is_not_a_loss() {
        let mut obs = Observation::from_data(ObservationData {
            tick: 100,
            my_carried_units: vec![CarriedUnitObs {
                carrier: UnitId(9),
                id: UnitId(1),
                kind: UnitKind::Sentinel,
                hp: 100,
            }],
            ..crate::test_support::observation_data()
        });
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
        use chassis::grid::TilePos;
        use oxide_sim::{BuildingId, BuildingKind, PlayerId};
        let mut obs = Observation::from_data(ObservationData {
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
                hp: 1000,
                ..crate::test_support::building(
                    5,
                    PlayerId(1),
                    BuildingKind::Foundry,
                    TilePos::new(5, 5),
                )
            }],
            ..crate::test_support::observation_data()
        });
        let mut delivery = OutcomeJournal::default();
        let mut episode = report(1);
        episode.id.owner = EpisodeOwner::Lift;
        delivery.watch(&obs, episode.id, episode.context, &[UnitId(1)], 1);
        delivery.observe_objective(&obs, BuildingId(5));
        delivery.handoff_objective(&obs, &[UnitId(1)]);
        delivery = crate::checkpoint::round_trip(&delivery);
        let wire = serde_json::to_value(&delivery).unwrap();
        for missing in [false, true] {
            let mut bad = wire.clone();
            let watch = bad["follow_through"][0]["watch"].as_object_mut().unwrap();
            if missing {
                watch.remove("objective");
            } else {
                watch.insert("objective".into(), serde_json::Value::Null);
            }
            assert!(serde_json::from_value::<OutcomeJournal>(bad).is_err());
        }
        let mut bounded = wire;
        let watch = bounded["follow_through"][0].clone();
        bounded["follow_through"] = serde_json::json!(vec![watch.clone(); EPISODE_LIMIT]);
        assert!(serde_json::from_value::<OutcomeJournal>(bounded.clone()).is_ok());
        bounded["follow_through"]
            .as_array_mut()
            .unwrap()
            .push(watch);
        assert!(serde_json::from_value::<OutcomeJournal>(bounded).is_err());
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
        use chassis::grid::TilePos;
        use oxide_sim::{BuildingId, BuildingKind, PlayerId};
        let mut obs = Observation::from_data(ObservationData {
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
                hp: 1000,
                ..crate::test_support::building(
                    5,
                    PlayerId(1),
                    BuildingKind::Foundry,
                    TilePos::new(5, 5),
                )
            }],
            ..crate::test_support::observation_data()
        });
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
        use chassis::grid::TilePos;
        use oxide_sim::{BuildingId, BuildingKind, PlayerId};
        let mut obs = Observation::from_data(ObservationData {
            tick: 100,
            map_width: 20,
            map_height: 20,
            visible: vec![true; 400],
            enemy_buildings: vec![BuildingObs {
                hp: 1000,
                ..crate::test_support::building(
                    1,
                    PlayerId(1),
                    BuildingKind::Foundry,
                    TilePos::new(5, 5),
                )
            }],
            ..crate::test_support::observation_data()
        });
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
