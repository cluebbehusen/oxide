//! Question-owned information work. Knowledge is reconciled independently of admission.

use super::*;
use crate::bot::allocation::{
    ClaimBundle, Confidence, ExecutionSafety, ProducerJobClaim, ProposalCase, StrategicValue,
    TimeToImpact, Urgency,
};
use chassis::{Tick, fx::Fx};
use std::collections::BTreeMap;

const HORIZON: Tick = 1_800;
const LOSS_COOLDOWN: Tick = 3_600;
const QUIET_INTERVAL: Tick = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ReconConsumer {
    HostileStart(PlayerId),
    Objective(PlayerId, BuildingKind),
    Economy,
    HarvestRecovery,
    Defense(BuildingId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ReconQuestionKey {
    pub(crate) consumer: ReconConsumer,
    pub(crate) y: i32,
    pub(crate) x: i32,
}

impl ReconQuestionKey {
    fn new(consumer: ReconConsumer, tile: TilePos) -> Self {
        Self {
            consumer,
            y: tile.y,
            x: tile.x,
        }
    }

    pub(crate) fn tile(self) -> TilePos {
        TilePos::new(self.x, self.y)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconQuestion {
    pub(crate) key: ReconQuestionKey,
    pub(crate) size: (i32, i32),
    pub(crate) evidence_at: Tick,
    pub(crate) confidence: Confidence,
    pub(crate) value: StrategicValue,
    pub(crate) urgency: Urgency,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReconObserver {
    Live(UnitId),
    Queued {
        producer: BuildingId,
        kind: UnitKind,
        occurrence: usize,
        ready_at: Tick,
    },
    Purchase {
        producer: BuildingId,
        kind: UnitKind,
        ready_at: Tick,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconProposal {
    pub(crate) question: ReconQuestion,
    pub(crate) observer: ReconObserver,
    pub(crate) observed_at: Tick,
    pub(crate) deadline: Tick,
    pub(crate) goal: TilePos,
    pub(crate) origin: TilePos,
    pub(crate) arrival_at: Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ReconProposalKey {
    pub(crate) question: ReconQuestionKey,
    pub(crate) unit: Option<UnitId>,
    pub(crate) producer: Option<BuildingId>,
    pub(crate) paid_occurrence: Option<u8>,
}

impl ReconProposal {
    pub(crate) fn funding_deadline(&self) -> Tick {
        let ready_at = match self.observer {
            ReconObserver::Purchase { ready_at, .. } | ReconObserver::Queued { ready_at, .. } => {
                ready_at
            }
            ReconObserver::Live(_) => self.observed_at,
        };
        self.deadline
            .saturating_sub(self.arrival_at.saturating_sub(ready_at))
    }
    pub(crate) fn key(&self) -> ReconProposalKey {
        let (unit, producer) = match self.observer {
            ReconObserver::Live(unit) => (Some(unit), None),
            ReconObserver::Queued { producer, .. } => (None, Some(producer)),
            ReconObserver::Purchase { producer, .. } => (None, Some(producer)),
        };
        ReconProposalKey {
            question: self.question.key,
            unit,
            producer,
            paid_occurrence: match self.observer {
                ReconObserver::Queued { occurrence, .. } => Some(
                    u8::try_from(occurrence)
                        .expect("ordinary queues have fewer than 256 occurrences"),
                ),
                _ => None,
            },
        }
    }
    pub(crate) fn case(&self) -> ProposalCase {
        ProposalCase {
            urgency: self.question.urgency,
            confidence: self.question.confidence,
            value: self.question.value,
            time_to_impact: if self.arrival_at.saturating_sub(self.observed_at) < 600 {
                TimeToImpact::Near
            } else {
                TimeToImpact::Patient
            },
            safety: ExecutionSafety::Managed,
        }
    }

    pub(crate) fn claims(&self) -> ClaimBundle {
        let (units, jobs) = match self.observer {
            ReconObserver::Live(unit) => (vec![unit], vec![]),
            ReconObserver::Queued {
                producer,
                kind,
                occurrence,
                ..
            } => {
                return ClaimBundle::default().with_paid_queue(vec![
                    crate::bot::allocation::PaidQueueClaim {
                        producer,
                        kind,
                        occurrence,
                    },
                ]);
            }
            ReconObserver::Purchase { producer, kind, .. } => (
                vec![],
                vec![ProducerJobClaim::flexible(
                    kind,
                    self.observed_at,
                    self.funding_deadline(),
                    vec![producer],
                )],
            ),
        };
        ClaimBundle::new(0, vec![], vec![], units, vec![], jobs)
            .expect("one observer owns one exact reconnaissance assignment")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReconPhase {
    Preparation,
    Outbound,
    Recall,
}

impl ReconPhase {
    pub(crate) fn trace(self) -> crate::bot::trace::ReconPhaseTrace {
        use crate::bot::trace::ReconPhaseTrace;
        match self {
            Self::Preparation => ReconPhaseTrace::Preparation,
            Self::Outbound => ReconPhaseTrace::Outbound,
            Self::Recall => ReconPhaseTrace::Recall,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconAssignment {
    pub(crate) proposal: ReconProposal,
    pub(crate) unit: Option<UnitId>,
    pub(crate) phase: ReconPhase,
    pub(crate) dispatch: Option<TilePos>,
    known_units: Vec<UnitId>,
    pub(crate) paid_claim: Option<crate::bot::allocation::PaidQueueClaim>,
    pub(crate) unpaid: bool,
    pub(crate) funding: Option<crate::bot::allocation::ScheduledProducerJob>,
}

impl ReconAssignment {
    pub(crate) fn retained_claims(&self, _now: Tick) -> ClaimBundle {
        let jobs = if self.unpaid {
            let ReconObserver::Purchase { producer, kind, .. } = self.proposal.observer else {
                unreachable!("only a purchase awaits funding")
            };
            let funding = self
                .funding
                .expect("an accepted unpaid purchase retains its exact schedule");
            vec![ProducerJobClaim::fixed(
                producer,
                kind,
                funding.enqueued_at,
                funding.starts_at,
                funding.ready_at,
                self.proposal.funding_deadline(),
            )]
        } else {
            vec![]
        };
        ClaimBundle::new(
            0,
            vec![],
            vec![],
            self.unit.into_iter().collect(),
            vec![],
            jobs,
        )
        .expect("one exact observer or one exact producer request")
        .with_paid_queue(self.paid_claim.into_iter().collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconRecovery {
    pub(crate) retry_at: Tick,
    pub(crate) quiet_since: Option<Tick>,
    pub(crate) attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationalReconWork {
    pub(crate) target: TilePos,
    pub(crate) scout: Option<UnitId>,
    pub(crate) goal: Option<TilePos>,
    pub(crate) deadline: Tick,
    pub(crate) paid: Vec<crate::bot::allocation::PaidQueueClaim>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Reconnaissance {
    pub(crate) assignments: BTreeMap<ReconQuestionKey, ReconAssignment>,
    pub(crate) recovery: BTreeMap<ReconQuestionKey, ReconRecovery>,
    questions: BTreeMap<ReconQuestionKey, ReconQuestion>,
    pub(super) observed_at: Option<Tick>,
    pub(crate) needs_air: bool,
    capability_demand: Vec<crate::bot::standing_force::CapabilityDemand>,
    queue_counts: BTreeMap<(BuildingId, UnitKind), usize>,
    previous_units: Vec<UnitId>,
    pub(crate) released: Vec<(ReconQuestionKey, crate::bot::trace::ReconReleaseReason)>,
    pub(crate) operational: Option<OperationalReconWork>,
    pub(crate) covered: Vec<(ReconQuestionKey, ReconObserver, Tick)>,
}

impl Reconnaissance {
    fn reconsider_after_quiet(&mut self, key: ReconQuestionKey, now: Tick) {
        let interval = question_quiet_interval(key);
        self.recovery
            .entry(key)
            .and_modify(|prior| {
                prior.retry_at = prior.retry_at.max(now.saturating_add(interval));
                prior.quiet_since = Some(now);
            })
            .or_insert(ReconRecovery {
                retry_at: now.saturating_add(interval),
                quiet_since: Some(now),
                attempts: 0,
            });
    }
    fn record_loss(&mut self, key: ReconQuestionKey, now: Tick) {
        let attempts = self
            .recovery
            .get(&key)
            .map_or(1, |prior| prior.attempts.saturating_add(1));
        self.recovery.insert(
            key,
            ReconRecovery {
                retry_at: now.saturating_add(if key.consumer == ReconConsumer::HarvestRecovery {
                    CONTESTED_RECON_RETRY_TICKS
                } else {
                    LOSS_COOLDOWN
                }),
                quiet_since: None,
                attempts,
            },
        );
        self.released
            .push((key, crate::bot::trace::ReconReleaseReason::ObserverLost));
    }
    pub(crate) fn release_unpaid(
        &mut self,
        key: ReconQuestionKey,
        now: Tick,
        reason: crate::bot::trace::ReconReleaseReason,
    ) {
        if self.assignments.get(&key).is_some_and(|work| work.unpaid) {
            self.assignments.remove(&key);
            self.released.push((key, reason));
            self.released.sort_by_key(|(key, _)| *key);
            self.reconsider_after_quiet(key, now);
        }
    }
    pub(crate) fn paid_claims(&self) -> Vec<crate::bot::allocation::PaidQueueClaim> {
        self.assignments
            .values()
            .filter_map(|assignment| assignment.paid_claim)
            .collect()
    }
    pub(crate) fn paid_exclusions(&self) -> Vec<(BuildingId, UnitKind, usize)> {
        self.paid_claims()
            .into_iter()
            .map(|claim| (claim.producer, claim.kind, claim.occurrence))
            .collect()
    }
    pub(crate) fn capability_demands(&self) -> &[crate::bot::standing_force::CapabilityDemand] {
        &self.capability_demand
    }
    pub(crate) fn reservations(&self) -> Vec<UnitId> {
        let mut units: Vec<_> = self
            .assignments
            .values()
            .filter_map(|work| work.unit)
            .collect();
        units.sort_unstable();
        units.dedup();
        units
    }
}

fn question_quiet_interval(key: ReconQuestionKey) -> Tick {
    if key.consumer == ReconConsumer::HarvestRecovery {
        CONTESTED_RECON_RETRY_TICKS
    } else {
        QUIET_INTERVAL
    }
}

fn newborn_at_factory(
    context: EconomicInvestmentContext<'_>,
    work: &ReconAssignment,
    unit: &UnitObs,
) -> bool {
    let producer = match work.proposal.observer {
        ReconObserver::Live(_) => return false,
        ReconObserver::Queued { producer, .. } | ReconObserver::Purchase { producer, .. } => {
            producer
        }
    };
    let distance = unit.tile.chebyshev(work.proposal.origin);
    distance <= 1
        && !work.known_units.contains(&unit.id)
        && context
            .obs
            .my_buildings
            .iter()
            .filter(|building| building.id != producer && building.kind == BuildingKind::Airworks)
            .all(|building| {
                unit.tile
                    .chebyshev(crate::bot::standing_force::air_production_spawn_tile(
                        building,
                        Some(context.orientation),
                    ))
                    > distance
            })
}

struct ReconRoutes<'a> {
    ground: RouteProjection<'a>,
    air: RouteProjection<'a>,
    costs: BTreeMap<(UnitKind, TilePos, TilePos), Option<u32>>,
}

impl<'a> ReconRoutes<'a> {
    fn new(
        context: EconomicInvestmentContext<'a>,
        danger: &danger::HarvestDangerProjection,
    ) -> Self {
        let obs = context.obs;
        let mut air_sources = Vec::new();
        for unit in context.unit_contacts {
            if unit.confidence_at(obs.tick) == 0 {
                continue;
            }
            for weapon in unit
                .kind
                .stats()
                .weapons
                .iter()
                .filter(|weapon| weapon.targets.covers(Domain::Air))
            {
                air_sources.push((unit.tile.center(), weapon.range + Fx::from_num(1)));
            }
        }
        for building in context.building_contacts {
            if !building.built || building.confidence_at(obs.tick) == 0 {
                continue;
            }
            let stats = building.kind.tier_stats(building.tier);
            for weapon in stats
                .weapons
                .iter()
                .filter(|weapon| weapon.targets.covers(Domain::Air))
            {
                air_sources.push((
                    building.anchor.center(),
                    weapon.range + Fx::from_num(stats.size.0.max(stats.size.1)),
                ));
            }
        }
        Self {
            costs: BTreeMap::new(),
            ground: RouteProjection::ground_avoiding_with_public_terrain(
                obs,
                context.briefing,
                context.orientation,
                |tile| danger.contains(tile),
            ),
            air: RouteProjection::avoiding_with_public_terrain(
                obs,
                Domain::Air,
                context.briefing,
                context.orientation,
                |tile| {
                    air_sources
                        .iter()
                        .any(|(source, range)| source.dist_sq(tile.center()) <= *range * *range)
                },
            ),
        }
    }

    fn for_kind(&mut self, kind: UnitKind) -> &mut RouteProjection<'a> {
        if kind.stats().domain == Domain::Air {
            &mut self.air
        } else {
            &mut self.ground
        }
    }

    fn arrival(&mut self, now: Tick, from: TilePos, goal: TilePos, kind: UnitKind) -> Option<Tick> {
        let key = (kind, from, goal);
        let cost = if let Some(cost) = self.costs.get(&key) {
            *cost
        } else {
            let cost = self
                .for_kind(kind)
                .safe_command_route_cost(from, goal, false);
            self.costs.insert(key, cost);
            cost
        }?;
        Some(
            now.saturating_add(super::economic_value::travel_ticks(kind, cost))
                .saturating_add(24),
        )
    }

    fn approach(
        &mut self,
        from: TilePos,
        kind: UnitKind,
        question: &ReconQuestion,
    ) -> Option<TilePos> {
        let tile = question.key.tile();
        let sight = (kind.stats().vision - question.size.0.max(question.size.1) - 1).max(1);
        let mut goals: Vec<_> = (-sight..=sight)
            .flat_map(|dy| {
                (-sight..=sight).filter_map(move |dx| {
                    (dx * dx + dy * dy <= sight * sight).then_some(tile.offset(dx, dy))
                })
            })
            .collect();
        goals.sort_by_key(|goal| (goal.manhattan(from), goal.y, goal.x));
        let route = self.for_kind(kind);
        goals.into_iter().find(|goal| {
            route.reaches(from, *goal) && route.direct_line_avoids_blocked(from, *goal)
        })
    }
}

impl UtilityPolicy {
    fn recon_questions(&self, context: EconomicInvestmentContext<'_>) -> Vec<ReconQuestion> {
        let obs = context.obs;
        let map = context.briefing;
        let mut questions = Vec::new();
        let mut add = |consumer, tile, size, confidence, value, urgency| {
            let key = ReconQuestionKey::new(consumer, tile);
            let evidence_at = self
                .reconnaissance
                .questions
                .get(&key)
                .map_or(obs.tick, |prior| prior.evidence_at);
            questions.push(ReconQuestion {
                key,
                size,
                evidence_at,
                confidence,
                value,
                urgency,
            });
        };
        for start in self.uncleared_hostile_starts(map, obs.me) {
            if obs
                .enemy_buildings
                .iter()
                .any(|building| building.anchor == start.anchor)
            {
                continue;
            }
            add(
                ReconConsumer::HostileStart(start.player),
                start.anchor,
                BuildingKind::Foundry.base_stats().size,
                Confidence::Prior,
                StrategicValue::Material,
                Urgency::Timely,
            );
        }
        for building in &obs.enemy_buildings {
            if building.seen
                || context.building_contacts.iter().any(|contact| {
                    contact.anchor == building.anchor
                        && contact.player == building.player
                        && contact
                            .last_seen
                            .is_some_and(|seen| obs.tick.saturating_sub(seen) < HORIZON)
                        && self
                            .reconnaissance
                            .operational
                            .as_ref()
                            .is_none_or(|operation| operation.target != building.anchor)
                })
                || !matches!(
                    building.kind,
                    BuildingKind::Foundry
                        | BuildingKind::Extractor
                        | BuildingKind::Airworks
                        | BuildingKind::Fabricator
                )
            {
                continue;
            }
            add(
                ReconConsumer::Objective(building.player, building.kind),
                building.anchor,
                building.kind.tier_stats(building.tier).size,
                Confidence::Supported,
                StrategicValue::Material,
                Urgency::Timely,
            );
        }
        for frame in map.extractor_frames() {
            let size = BuildingKind::Extractor.base_stats().size;
            let actionable = obs.my_buildings.iter().any(|building| {
                building.kind == BuildingKind::Foundry
                    && building.built
                    && building.anchor.chebyshev(*frame) <= crate::stats::EXTRACTOR_SUPPORT_RADIUS
            });
            if actionable
                && !(0..size.1).all(|dy| (0..size.0).all(|dx| obs.visible(frame.offset(dx, dy))))
            {
                add(
                    ReconConsumer::Economy,
                    *frame,
                    size,
                    Confidence::Prior,
                    StrategicValue::Material,
                    Urgency::Timely,
                );
            }
        }
        let harvesters = obs
            .my_units
            .iter()
            .filter(|unit| {
                unit.hp > 0
                    && unit.kind.stats().harvest.is_some()
                    && !self.evacuating_workers.contains(&unit.id)
            })
            .count();
        if harvesters > 0 {
            let mut sources: BTreeMap<_, _> = map.initial_scrap().iter().copied().collect();
            sources.extend(obs.known_scrap.iter().copied());
            for (source, prior_amount) in sources {
                if obs.visible(source)
                    || prior_amount < UnitKind::Kestrel.stats().cost
                    || (obs.explored(source) && !obs.known_scrap_at(source))
                {
                    continue;
                }
                let useful = obs
                    .my_buildings
                    .iter()
                    .filter(|building| building.built && building.kind.is_drop_off())
                    .any(|base| {
                        let local =
                            source.chebyshev(base.anchor) <= crate::stats::EXTRACTOR_SUPPORT_RADIUS;
                        let known_work = obs
                            .known_scrap
                            .iter()
                            .filter(|(tile, _)| {
                                obs.visible(*tile)
                                    && tile.chebyshev(base.anchor)
                                        <= crate::stats::EXTRACTOR_SUPPORT_RADIUS
                            })
                            .map(|(_, amount)| u64::from(*amount))
                            .sum::<u64>();
                        let harvest = UnitKind::Harvester
                            .stats()
                            .harvest
                            .expect("harvesting role");
                        local
                            && known_work
                                < (HORIZON / u64::from(harvest.ticks_per_scrap)) * harvesters as u64
                    });
                if useful {
                    add(
                        ReconConsumer::Economy,
                        source,
                        (1, 1),
                        if obs.explored(source) {
                            Confidence::Supported
                        } else {
                            Confidence::Prior
                        },
                        StrategicValue::Material,
                        Urgency::Timely,
                    );
                }
            }
        }
        for region in &self.contested_harvest_regions {
            add(
                ReconConsumer::HarvestRecovery,
                region.center,
                (1, 1),
                Confidence::Current,
                StrategicValue::Decisive,
                Urgency::Pressing,
            );
        }
        for blip in &obs.blips {
            if let Some(asset) = obs
                .my_buildings
                .iter()
                .filter(|building| building.built && building.anchor.chebyshev(*blip) <= 16)
                .min_by_key(|building| (building.anchor.manhattan(*blip), building.id))
            {
                add(
                    ReconConsumer::Defense(asset.id),
                    *blip,
                    (1, 1),
                    Confidence::Supported,
                    StrategicValue::Material,
                    Urgency::Pressing,
                );
            }
        }
        questions.sort_by_key(|question| question.key);
        questions.dedup_by_key(|question| question.key);
        let mut footprints = std::collections::BTreeSet::new();
        questions.retain(|question| footprints.insert((question.key.tile(), question.size)));
        questions
    }

    fn recon_answered(&self, obs: &Observation, question: &ReconQuestion) -> bool {
        let tile = question.key.tile();
        if question.key.consumer == ReconConsumer::HarvestRecovery {
            return !self
                .contested_harvest_regions
                .iter()
                .any(|region| region.center == tile);
        }
        (0..question.size.1)
            .all(|dy| (0..question.size.0).all(|dx| obs.visible(tile.offset(dx, dy))))
            || obs
                .enemy_buildings
                .iter()
                .any(|building| building.seen && building.anchor == tile)
    }

    /// Reconcile every retained assignment, even when no discretionary attention remains.
    pub(in crate::bot) fn observe_reconnaissance(
        &mut self,
        context: EconomicInvestmentContext<'_>,
        home: TilePos,
    ) -> Vec<Intent> {
        let obs = context.obs;
        if self.reconnaissance.observed_at == Some(obs.tick) {
            return vec![];
        }
        self.reconnaissance.observed_at = Some(obs.tick);
        self.reconnaissance.released.clear();
        let previous_units = core::mem::replace(
            &mut self.reconnaissance.previous_units,
            obs.my_units.iter().map(|unit| unit.id).collect(),
        );
        self.clear_visible_public_starts(obs, context.briefing);
        self.refresh_contested_harvest_regions(
            obs,
            Some(context.unit_contacts),
            Some(context.building_contacts),
        );
        let questions = self.recon_questions(context);
        self.reconnaissance.questions = questions
            .into_iter()
            .map(|question| (question.key, question))
            .collect();
        let mut queue_counts = BTreeMap::new();
        for lane in context.resources.producers() {
            for (kind, _) in lane.queued_readiness() {
                *queue_counts.entry((lane.producer, kind)).or_insert(0usize) += 1;
            }
        }
        let mut births = BTreeMap::<_, std::collections::BTreeSet<UnitId>>::new();
        for work in self
            .reconnaissance
            .assignments
            .values()
            .filter(|work| !work.unpaid)
        {
            if let Some(claim) = work.paid_claim {
                let found = births.entry((claim.producer, claim.kind)).or_default();
                found.extend(
                    obs.my_units
                        .iter()
                        .filter(|unit| {
                            unit.kind == claim.kind
                                && !previous_units.contains(&unit.id)
                                && newborn_at_factory(context, work, unit)
                        })
                        .map(|unit| unit.id),
                );
            }
        }
        for work in self.reconnaissance.assignments.values_mut() {
            if let Some(claim) = &mut work.paid_claim {
                let key = (claim.producer, claim.kind);
                let before = self
                    .reconnaissance
                    .queue_counts
                    .get(&key)
                    .copied()
                    .unwrap_or(0);
                let now = queue_counts.get(&key).copied().unwrap_or(0);
                let completed = before
                    .saturating_sub(now)
                    .max(births.get(&key).map_or(0, |units| units.len()));
                if claim.occurrence < completed {
                    work.paid_claim = None;
                } else {
                    claim.occurrence -= completed;
                }
            }
        }
        self.reconnaissance.queue_counts = queue_counts;
        for (key, recovery) in &mut self.reconnaissance.recovery {
            let quiet = !obs
                .enemy_units
                .iter()
                .any(|unit| unit.tile.chebyshev(key.tile()) <= 8)
                && !obs
                    .salvage_incidents
                    .iter()
                    .any(|tile| tile.chebyshev(key.tile()) <= CONTESTED_RECON_RADIUS);
            if quiet {
                recovery.quiet_since.get_or_insert(obs.tick);
            } else {
                recovery.quiet_since = None;
            }
        }
        if self.reconnaissance.assignments.is_empty() {
            return vec![];
        }
        let danger = self.harvest_danger_projection(
            obs,
            Some(context.unit_contacts),
            Some(context.building_contacts),
        );
        let mut routes = ReconRoutes::new(context, &danger);
        let mut intents = Vec::new();
        let return_goal = self.passable_near(obs, home);
        let mut assigned = self.reconnaissance.reservations();
        let mut prior: Vec<_> = core::mem::take(&mut self.reconnaissance.assignments)
            .into_iter()
            .collect();
        prior.sort_by_key(|(key, work)| {
            let (producer, ready) = match work.proposal.observer {
                ReconObserver::Live(_) => (None, 0),
                ReconObserver::Queued {
                    producer, ready_at, ..
                }
                | ReconObserver::Purchase {
                    producer, ready_at, ..
                } => (
                    Some(producer),
                    work.funding.map_or(ready_at, |job| job.ready_at),
                ),
            };
            (work.unit.is_some(), ready, producer, *key)
        });
        for (key, mut work) in prior {
            let answered = self.recon_answered(obs, &work.proposal.question);
            let useful = self.reconnaissance.questions.contains_key(&key);
            if work.phase == ReconPhase::Preparation {
                if answered
                    || !useful
                    || obs.tick >= work.proposal.deadline
                    || (work.unpaid && obs.tick >= work.proposal.funding_deadline())
                {
                    let reason = if answered {
                        crate::bot::trace::ReconReleaseReason::Answered
                    } else if !useful {
                        crate::bot::trace::ReconReleaseReason::NoLongerUseful
                    } else {
                        crate::bot::trace::ReconReleaseReason::DeadlineExpired
                    };
                    self.reconnaissance.released.push((key, reason));
                    self.reconnaissance.reconsider_after_quiet(key, obs.tick);
                    continue;
                }
                let (ReconObserver::Purchase {
                    producer,
                    kind,
                    ready_at,
                }
                | ReconObserver::Queued {
                    producer,
                    kind,
                    ready_at,
                    ..
                }) = work.proposal.observer
                else {
                    unreachable!("preparation owns paid production");
                };
                if work.unpaid
                    && !obs.my_buildings.iter().any(|building| {
                        building.id == producer
                            && routes.air.reaches(building.anchor, work.proposal.goal)
                            && routes
                                .air
                                .command_path_avoids_blocked(building.anchor, work.proposal.goal)
                    })
                {
                    self.reconnaissance.released.push((
                        key,
                        crate::bot::trace::ReconReleaseReason::ProducerOrRouteUnavailable,
                    ));
                    self.reconnaissance.reconsider_after_quiet(key, obs.tick);
                    continue;
                }
                if let Some(unit) = obs
                    .my_units
                    .iter()
                    .filter(|unit| {
                        unit.kind == kind
                            && !work.unpaid
                            && work.paid_claim.is_none()
                            && unit.hp > 0
                            && unit.idle
                            && newborn_at_factory(context, &work, unit)
                            && !previous_units.contains(&unit.id)
                            && !assigned.contains(&unit.id)
                            && !context.unavailable.contains(&unit.id)
                    })
                    .min_by_key(|unit| unit.id)
                {
                    work.unit = Some(unit.id);
                    work.paid_claim = None;
                    assigned.push(unit.id);
                    work.phase = if routes
                        .arrival(obs.tick, unit.tile, work.proposal.goal, unit.kind)
                        .is_some_and(|arrival| arrival < work.proposal.deadline)
                    {
                        ReconPhase::Outbound
                    } else {
                        ReconPhase::Recall
                    };
                } else if !work.unpaid && work.paid_claim.is_none() {
                    self.reconnaissance.record_loss(key, obs.tick);
                    continue;
                } else if !obs
                    .my_buildings
                    .iter()
                    .any(|building| building.id == producer)
                    && obs.tick > ready_at
                {
                    self.reconnaissance.released.push((
                        key,
                        crate::bot::trace::ReconReleaseReason::ProducerOrRouteUnavailable,
                    ));
                    self.reconnaissance.reconsider_after_quiet(key, obs.tick);
                    continue;
                }
            }
            if let Some(id) = work.unit {
                let Some(unit) = obs
                    .my_units
                    .iter()
                    .find(|unit| unit.id == id && unit.hp > 0)
                else {
                    self.reconnaissance.record_loss(key, obs.tick);
                    continue;
                };
                if answered
                    || !useful
                    || obs.tick >= work.proposal.deadline
                    || (key.consumer == ReconConsumer::HarvestRecovery
                        && self.contested_recon_blocked.contains(&key.tile()))
                {
                    work.phase = ReconPhase::Recall;
                }
                if work.phase == ReconPhase::Recall {
                    if unit.tile.chebyshev(return_goal) <= 1 {
                        self.reconnaissance.reconsider_after_quiet(key, obs.tick);
                        self.reconnaissance
                            .released
                            .push((key, crate::bot::trace::ReconReleaseReason::SafeReturn));
                        continue;
                    }
                    let goal = return_goal;
                    if (work.dispatch != Some(goal) || unit.idle)
                        && routes
                            .for_kind(unit.kind)
                            .safe_command_route_cost(unit.tile, goal, true)
                            .is_some()
                    {
                        intents.push(Intent::MoveUnits {
                            units: vec![id],
                            goal,
                        });
                        work.dispatch = Some(goal);
                    }
                } else {
                    let mut question = work.proposal.question.clone();
                    if key.consumer == ReconConsumer::HarvestRecovery
                        && let Some(tile) = Self::contested_region_tiles(obs, key.tile())
                            .filter(|tile| {
                                !self
                                    .contested_harvest_clear_tiles
                                    .contains(&(key.tile(), *tile))
                            })
                            .min_by_key(|tile| (tile.chebyshev(unit.tile), tile.y, tile.x))
                    {
                        question.key.x = tile.x;
                        question.key.y = tile.y;
                    }
                    if work.dispatch.is_none()
                        || unit.idle
                        || !routes
                            .for_kind(unit.kind)
                            .reaches(unit.tile, work.dispatch.unwrap_or(work.proposal.goal))
                        || !routes.for_kind(unit.kind).command_path_avoids_blocked(
                            unit.tile,
                            work.dispatch.unwrap_or(work.proposal.goal),
                        )
                    {
                        if let Some(goal) = routes.approach(unit.tile, unit.kind, &question)
                            && routes
                                .for_kind(unit.kind)
                                .command_path_avoids_blocked(unit.tile, goal)
                        {
                            intents.push(Intent::MoveUnits {
                                units: vec![id],
                                goal,
                            });
                            work.dispatch = Some(goal);
                        } else {
                            work.phase = ReconPhase::Recall;
                            intents.push(Intent::StopUnits { units: vec![id] });
                        }
                    }
                }
            }
            self.reconnaissance.assignments.insert(key, work);
        }
        self.reconnaissance.released.sort_by_key(|(key, _)| *key);
        intents
    }

    pub(in crate::bot) fn prepare_reconnaissance(
        &mut self,
        context: EconomicInvestmentContext<'_>,
        tuning: DifficultyTuning,
        minimum_core: u32,
        allow_paid: bool,
        paid_unavailable: &[crate::bot::allocation::PaidQueueClaim],
    ) -> Vec<ReconProposal> {
        self.reconnaissance.needs_air = false;
        self.reconnaissance.capability_demand.clear();
        self.reconnaissance.covered.clear();
        if !strategic_admission_tick(context.obs.tick) {
            return vec![];
        }
        let obs = context.obs;
        let mut questions: Vec<_> = self
            .reconnaissance
            .questions
            .values()
            .filter(|question| {
                !self.reconnaissance.assignments.contains_key(&question.key)
                    && !self.recon_answered(obs, question)
                    && (matches!(question.key.consumer, ReconConsumer::HostileStart(_))
                        || obs.tick.saturating_sub(question.evidence_at) >= tuning.reaction_delay)
                    && self
                        .reconnaissance
                        .recovery
                        .get(&question.key)
                        .is_none_or(|recovery| {
                            obs.tick >= recovery.retry_at
                                && recovery.quiet_since.is_some_and(|quiet| {
                                    obs.tick.saturating_sub(quiet)
                                        >= question_quiet_interval(question.key)
                                })
                        })
                    && !(question.key.consumer == ReconConsumer::HarvestRecovery
                        && self.contested_recon_blocked.contains(&question.key.tile()))
            })
            .cloned()
            .collect();
        questions.sort_by_key(|question| {
            (
                std::cmp::Reverse(question.urgency as u8),
                std::cmp::Reverse(question.value as u8),
                question.key,
            )
        });
        questions.truncate(tuning.attention_slots);
        if questions.is_empty() {
            return vec![];
        }
        let danger = self.harvest_danger_projection(
            obs,
            Some(context.unit_contacts),
            Some(context.building_contacts),
        );
        let mut routes = ReconRoutes::new(context, &danger);
        let owned = self.reconnaissance.reservations();
        let mut proposals = Vec::new();
        for question in questions {
            let deadline = obs.tick.saturating_add(HORIZON);
            let overlapping = self
                .reconnaissance
                .assignments
                .values()
                .find_map(|assignment| {
                    if question.key.consumer == ReconConsumer::HarvestRecovery
                        || assignment.proposal.question.key.consumer
                            == ReconConsumer::HarvestRecovery
                        || assignment.phase == ReconPhase::Recall
                        || assignment.unpaid
                    {
                        return None;
                    }
                    let (observer, kind, origin, ready_at) = if let Some(id) = assignment.unit {
                        let unit = obs
                            .my_units
                            .iter()
                            .find(|unit| unit.id == id && unit.hp > 0)?;
                        (ReconObserver::Live(id), unit.kind, unit.tile, obs.tick)
                    } else {
                        let claim = assignment.paid_claim?;
                        let (_, ready_at) = context
                            .resources
                            .producers()
                            .iter()
                            .find(|lane| lane.producer == claim.producer)?
                            .queued_readiness()
                            .filter(|(kind, _)| *kind == claim.kind)
                            .nth(claim.occurrence)?;
                        (
                            ReconObserver::Queued {
                                producer: claim.producer,
                                kind: claim.kind,
                                occurrence: claim.occurrence,
                                ready_at,
                            },
                            claim.kind,
                            assignment.proposal.origin,
                            ready_at,
                        )
                    };
                    let goal = assignment.proposal.goal;
                    let sight = kind.stats().vision;
                    if !(0..question.size.1).all(|dy| {
                        (0..question.size.0).all(|dx| {
                            let tile = question.key.tile().offset(dx, dy);
                            let dx = tile.x - goal.x;
                            let dy = tile.y - goal.y;
                            dx * dx + dy * dy <= sight * sight
                        })
                    }) {
                        return None;
                    }
                    let arrival = routes.arrival(ready_at, origin, goal, kind)?;
                    (arrival < deadline.min(assignment.proposal.deadline))
                        .then_some((observer, arrival))
                });
            if let Some((observer, arrival)) = overlapping {
                self.reconnaissance
                    .covered
                    .push((question.key, observer, arrival));
                continue;
            }
            if question.key.consumer == ReconConsumer::Economy {
                let size = question.size;
                let accessible = obs
                    .my_units
                    .iter()
                    .filter(|unit| unit.kind.stats().harvest.is_some() && unit.hp > 0)
                    .any(|worker| {
                        crate::tick::rect_adjacent_tiles(question.key.tile(), size)
                            .any(|door| routes.ground.reaches(worker.tile, door))
                    });
                if !accessible {
                    continue;
                }
            }
            if let Some(operation) = self
                .reconnaissance
                .operational
                .as_ref()
                .filter(|operation| {
                    operation.target == question.key.tile() && operation.deadline > obs.tick
                })
            {
                let useful_until = deadline.min(operation.deadline);
                let live = operation.scout.and_then(|id| {
                    obs.my_units
                        .iter()
                        .find(|unit| unit.id == id && unit.hp > 0)
                });
                let coverage = live
                    .and_then(|unit| {
                        let goal = operation.goal?;
                        let sight = unit.kind.stats().vision;
                        let sees = (0..question.size.1).all(|dy| {
                            (0..question.size.0).all(|dx| {
                                let tile = question.key.tile().offset(dx, dy);
                                let dx = tile.x - goal.x;
                                let dy = tile.y - goal.y;
                                dx * dx + dy * dy <= sight * sight
                            })
                        });
                        let arrival = routes.arrival(obs.tick, unit.tile, goal, unit.kind)?;
                        (sees && arrival < useful_until)
                            .then_some((ReconObserver::Live(unit.id), arrival))
                    })
                    .or_else(|| {
                        let kind = crate::stats::Role::Scout.unit_for(obs.faction);
                        for lane in context.resources.producers() {
                            let Some(building) = obs
                                .my_buildings
                                .iter()
                                .find(|building| building.id == lane.producer)
                            else {
                                continue;
                            };
                            for (occurrence, (_, ready_at)) in lane
                                .queued_readiness()
                                .filter(|(queued, _)| *queued == kind)
                                .enumerate()
                            {
                                if !operation.paid.contains(
                                    &crate::bot::allocation::PaidQueueClaim {
                                        producer: lane.producer,
                                        kind,
                                        occurrence,
                                    },
                                ) {
                                    continue;
                                }
                                let Some(goal) = routes.approach(building.anchor, kind, &question)
                                else {
                                    continue;
                                };
                                let Some(arrival) =
                                    routes.arrival(ready_at, building.anchor, goal, kind)
                                else {
                                    continue;
                                };
                                if arrival < useful_until {
                                    return Some((
                                        ReconObserver::Queued {
                                            producer: lane.producer,
                                            kind,
                                            occurrence,
                                            ready_at,
                                        },
                                        arrival,
                                    ));
                                }
                            }
                        }
                        None
                    });
                if let Some((observer, arrival)) = coverage {
                    self.reconnaissance
                        .covered
                        .push((question.key, observer, arrival));
                    continue;
                }
            }
            let mut live: Vec<_> = obs
                .my_units
                .iter()
                .filter(|unit| {
                    unit.hp > 0
                        && unit.idle
                        && unit.site.is_none()
                        && unit.founding.is_none()
                        && !unit.repairing
                        && unit.salvaging.is_none()
                        && !obs.my_queued_units.contains(&unit.id)
                        && !context.unavailable.contains(&unit.id)
                        && !owned.contains(&unit.id)
                        && self
                            .reconnaissance
                            .operational
                            .as_ref()
                            .is_none_or(|operation| operation.scout != Some(unit.id))
                        && self
                            .foundry_saving
                            .as_ref()
                            .is_none_or(|saving| saving.plan.builder != unit.id)
                        && self
                            .economic_saving
                            .as_ref()
                            .is_none_or(|saving| saving.builder != Some(unit.id))
                        && !self.evacuating_workers.contains(&unit.id)
                        && !self
                            .support_work
                            .repairs
                            .iter()
                            .any(|repair| repair.key.worker == unit.id)
                })
                .filter_map(|unit| {
                    combat::utility_scout_preference(
                        unit,
                        question.key.consumer == ReconConsumer::HarvestRecovery,
                    )
                    .map(|preference| (preference, unit))
                })
                .filter(|(_, unit)| {
                    if unit.kind.stats().harvest.is_some() {
                        return obs
                            .my_units
                            .iter()
                            .filter(|other| {
                                other.kind.stats().harvest.is_some() && !owned.contains(&other.id)
                            })
                            .count()
                            > 2;
                    }
                    if unit.kind.stats().weapons.is_empty() {
                        return true;
                    }
                    let mut excluded = context.unavailable.to_vec();
                    excluded.extend_from_slice(&owned);
                    excluded.push(unit.id);
                    combat_core_status(obs, &excluded, &[], u64::from(minimum_core)).ready
                })
                .collect();
            live.sort_by_key(|(preference, unit)| {
                (
                    *preference,
                    unit.tile.manhattan(question.key.tile()),
                    unit.id,
                )
            });
            let selected = live.into_iter().find_map(|(_, unit)| {
                let goal = routes.approach(unit.tile, unit.kind, &question)?;
                let arrival_at = routes.arrival(obs.tick, unit.tile, goal, unit.kind)?;
                (arrival_at < deadline).then_some((unit, goal, arrival_at))
            });
            if let Some((unit, goal, arrival_at)) = selected {
                proposals.push(ReconProposal {
                    question: question.clone(),
                    observer: ReconObserver::Live(unit.id),
                    origin: unit.tile,
                    observed_at: obs.tick,
                    deadline,
                    goal,
                    arrival_at,
                });
            }
            let kind = crate::stats::Role::Scout.unit_for(obs.faction);
            let mut queued = Vec::new();
            for lane in context.resources.producers() {
                let Some(building) = obs
                    .my_buildings
                    .iter()
                    .find(|building| building.id == lane.producer)
                else {
                    continue;
                };
                for (occurrence, (_, ready_at)) in lane
                    .queued_readiness()
                    .filter(|(queued, _)| *queued == kind)
                    .enumerate()
                {
                    let claim = crate::bot::allocation::PaidQueueClaim {
                        producer: lane.producer,
                        kind,
                        occurrence,
                    };
                    if paid_unavailable.contains(&claim)
                        || self.reconnaissance.paid_claims().contains(&claim)
                    {
                        continue;
                    }
                    let origin = crate::bot::standing_force::air_production_spawn_tile(
                        building,
                        Some(context.orientation),
                    );
                    let Some(goal) = routes.approach(origin, kind, &question) else {
                        continue;
                    };
                    let Some(arrival_at) = routes.arrival(ready_at, origin, goal, kind) else {
                        continue;
                    };
                    if arrival_at < deadline {
                        queued.push((
                            arrival_at,
                            lane.producer,
                            occurrence,
                            ready_at,
                            origin,
                            goal,
                        ));
                    }
                }
            }
            queued.sort_by_key(|(arrival_at, producer, occurrence, ..)| {
                (*arrival_at, *producer, *occurrence)
            });
            if let Some((arrival_at, producer, occurrence, ready_at, origin, goal)) =
                queued.first().copied()
            {
                proposals.push(ReconProposal {
                    question: question.clone(),
                    observer: ReconObserver::Queued {
                        producer,
                        kind,
                        occurrence,
                        ready_at,
                    },
                    origin,
                    observed_at: obs.tick,
                    deadline,
                    goal,
                    arrival_at,
                });
                continue;
            }
            if !allow_paid {
                continue;
            }
            let mut producers: Vec<_> = context
                .resources
                .producers()
                .iter()
                .filter_map(|lane| {
                    let timing = lane.production_timing(&[kind])?;
                    let ready_at = timing.no_block_latest_ready_tick;
                    let building = obs
                        .my_buildings
                        .iter()
                        .find(|building| building.id == lane.producer)?;
                    let origin = crate::bot::standing_force::air_production_spawn_tile(
                        building,
                        Some(context.orientation),
                    );
                    let goal = routes.approach(origin, kind, &question)?;
                    let arrival_at = routes.arrival(ready_at, origin, goal, kind)?;
                    (arrival_at < deadline).then_some((
                        arrival_at,
                        lane.producer,
                        ready_at,
                        origin,
                        goal,
                    ))
                })
                .collect();
            producers.sort_by_key(|(arrival_at, producer, ..)| (*arrival_at, *producer));
            if let Some((arrival_at, producer, ready_at, origin, goal)) = producers.first().copied()
            {
                proposals.push(ReconProposal {
                    question,
                    observer: ReconObserver::Purchase {
                        producer,
                        kind,
                        ready_at,
                    },
                    origin,
                    observed_at: obs.tick,
                    deadline,
                    goal,
                    arrival_at,
                });
            } else if selected.is_none() {
                let home = obs
                    .my_buildings
                    .iter()
                    .filter(|building| building.kind == BuildingKind::Foundry)
                    .min_by_key(|building| building.id)
                    .map(|building| building.anchor);
                if let Some(home) = home
                    && let Some(goal) = routes.approach(home, kind, &question)
                    && routes.air.command_path_avoids_blocked(home, goal)
                {
                    self.reconnaissance.needs_air = true;
                    self.reconnaissance.capability_demand.push(
                        crate::bot::standing_force::CapabilityDemand {
                            kind,
                            service: crate::bot::allocation::StandingForceServiceKey::Point(
                                question.key.tile(),
                            ),
                            reason: crate::bot::standing_force::StandingForceReason::Reconnaissance,
                            case: ProposalCase {
                                urgency: question.urgency,
                                confidence: question.confidence,
                                value: question.value,
                                time_to_impact: TimeToImpact::Patient,
                                safety: ExecutionSafety::Managed,
                            },
                            unmet: 1,
                            baseline: kind,
                            provider_value: u128::from(kind.stats().cost),
                        },
                    );
                }
            }
        }
        proposals
    }

    pub(crate) fn commit_reconnaissance(
        &mut self,
        proposal: ReconProposal,
        funding: Option<crate::bot::allocation::ScheduledProducerJob>,
        obs: &Observation,
        intents: &mut Vec<Intent>,
    ) -> bool {
        if proposal.observed_at != obs.tick
            || proposal.deadline <= obs.tick
            || self
                .reconnaissance
                .assignments
                .contains_key(&proposal.question.key)
            || self.reconnaissance.questions.get(&proposal.question.key) != Some(&proposal.question)
        {
            return false;
        }
        if let ReconObserver::Purchase { producer, kind, .. } = proposal.observer
            && funding.is_none_or(|job| {
                job.producer != producer
                    || job.kind != kind
                    || job.ready_at >= proposal.funding_deadline()
            })
        {
            return false;
        }
        let unpaid = funding.is_some_and(|job| job.enqueued_at > obs.tick);
        let (unit, phase, dispatch) = match proposal.observer {
            ReconObserver::Live(id) => {
                if self.reconnaissance.reservations().contains(&id)
                    || !obs.my_units.iter().any(|unit| {
                        unit.id == id
                            && unit.hp > 0
                            && unit.idle
                            && unit.site.is_none()
                            && unit.founding.is_none()
                            && !unit.repairing
                            && unit.salvaging.is_none()
                            && !obs.my_queued_units.contains(&id)
                    })
                {
                    return false;
                }
                intents.push(Intent::MoveUnits {
                    units: vec![id],
                    goal: proposal.goal,
                });
                (Some(id), ReconPhase::Outbound, Some(proposal.goal))
            }
            ReconObserver::Purchase { .. } | ReconObserver::Queued { .. } => {
                (None, ReconPhase::Preparation, None)
            }
        };
        let known_units = obs.my_units.iter().map(|unit| unit.id).collect();
        let paid_claim = match proposal.observer {
            ReconObserver::Live(_) => None,
            ReconObserver::Queued {
                producer,
                kind,
                occurrence,
                ..
            } => Some(crate::bot::allocation::PaidQueueClaim {
                producer,
                kind,
                occurrence,
            }),
            ReconObserver::Purchase { .. } if unpaid => None,
            ReconObserver::Purchase { producer, kind, .. } => {
                let occurrence = obs
                    .my_buildings
                    .iter()
                    .position(|building| building.id == producer)
                    .and_then(|index| obs.my_queues.get(index))
                    .map_or(0, |queue| {
                        queue.iter().filter(|queued| **queued == kind).count()
                    });
                Some(crate::bot::allocation::PaidQueueClaim {
                    producer,
                    kind,
                    occurrence,
                })
            }
        };
        if let ReconObserver::Purchase { producer, kind, .. } = proposal.observer
            && !unpaid
        {
            *self
                .reconnaissance
                .queue_counts
                .entry((producer, kind))
                .or_insert(0) += 1;
        }
        self.reconnaissance.assignments.insert(
            proposal.question.key,
            ReconAssignment {
                proposal,
                unit,
                phase,
                dispatch,
                known_units,
                paid_claim,
                unpaid,
                funding,
            },
        );
        true
    }

    pub(crate) fn bind_reconnaissance_funding(
        &mut self,
        key: ReconQuestionKey,
        job: crate::bot::allocation::ScheduledProducerJob,
        obs: &Observation,
    ) -> bool {
        let Some(work) = self.reconnaissance.assignments.get_mut(&key) else {
            return false;
        };
        let ReconObserver::Purchase { producer, kind, .. } = work.proposal.observer else {
            return false;
        };
        if !work.unpaid
            || job.producer != producer
            || job.kind != kind
            || job.ready_at >= work.proposal.funding_deadline()
        {
            return false;
        }
        work.funding = Some(job);
        if job.enqueued_at == obs.tick {
            work.unpaid = false;
            work.known_units = obs.my_units.iter().map(|unit| unit.id).collect();
            let occurrence = obs
                .my_buildings
                .iter()
                .position(|building| building.id == producer)
                .and_then(|index| obs.my_queues.get(index))
                .map_or(0, |queue| {
                    queue.iter().filter(|queued| **queued == kind).count()
                });
            work.paid_claim = Some(crate::bot::allocation::PaidQueueClaim {
                producer,
                kind,
                occurrence,
            });
            *self
                .reconnaissance
                .queue_counts
                .entry((producer, kind))
                .or_insert(0) += 1;
        }
        true
    }

    pub(crate) fn bind_reconnaissance_queue_order(
        &mut self,
        jobs: &[crate::bot::allocation::ScheduledProducerJob],
        obs: &Observation,
    ) {
        use crate::bot::allocation::{ClaimOwner, ObligationKey, ProposalKey};
        let mut counts = BTreeMap::new();
        for (index, building) in obs.my_buildings.iter().enumerate() {
            for kind in obs.my_queues.get(index).into_iter().flatten() {
                *counts.entry((building.id, *kind)).or_insert(0usize) += 1;
            }
        }
        for job in jobs.iter().filter(|job| job.enqueued_at == obs.tick) {
            let occurrence = counts.entry((job.producer, job.kind)).or_insert(0);
            let key = match job.owner {
                ClaimOwner::Proposal(ProposalKey::Reconnaissance(key)) => Some(key.question),
                ClaimOwner::Obligation {
                    key: ObligationKey::Reconnaissance(key),
                    ..
                } => Some(key),
                _ => None,
            };
            if let Some(work) = key.and_then(|key| self.reconnaissance.assignments.get_mut(&key)) {
                work.paid_claim = Some(crate::bot::allocation::PaidQueueClaim {
                    producer: job.producer,
                    kind: job.kind,
                    occurrence: *occurrence,
                });
            }
            *occurrence += 1;
        }
        self.reconnaissance.queue_counts = counts;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};

    fn fixture() -> (Observation, PublicMapBriefing, ResolvedProfile) {
        let scenario = crate::Scenario::skirmish();
        let mut obs = Observation::omniscient(&scenario.build().unwrap(), PlayerId(0));
        obs.tick = 120;
        obs.scrap = 1_000;
        obs.map_width = 40;
        obs.map_height = 30;
        obs.visible = vec![false; 1200];
        obs.explored = vec![true; 1200];
        obs.known_rock.clear();
        obs.known_peaks.clear();
        obs.known_scrap.clear();
        obs.known_frames.clear();
        obs.enemy_units.clear();
        obs.enemy_buildings.clear();
        obs.blips.clear();
        obs.my_buildings.truncate(1);
        obs.my_buildings[0].anchor = TilePos::new(2, 12);
        obs.my_queues = vec![vec![]];
        obs.my_queue_progress = vec![0];
        let mut scout = obs.my_units[0].clone();
        scout.kind = UnitKind::Kestrel;
        scout.hp = scout.kind.stats().max_hp;
        scout.idle = true;
        scout.tile = TilePos::new(7, 10);
        scout.id = UnitId(100);
        obs.my_units = vec![
            scout.clone(),
            UnitObs {
                id: UnitId(101),
                tile: TilePos::new(7, 20),
                ..scout
            },
        ];
        let map = PublicMapBriefing {
            map_width: 40,
            map_height: 30,
            starting_foundries: vec![
                StartingFoundry {
                    player: PlayerId(1),
                    anchor: TilePos::new(32, 4),
                },
                StartingFoundry {
                    player: PlayerId(2),
                    anchor: TilePos::new(32, 23),
                },
            ],
            teams: vec![None, None, None],
            non_ground_terrain: vec![],
            extractor_frames: vec![],
            initial_scrap: vec![],
        };
        (
            obs,
            map,
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 7).resolve_profile(),
        )
    }

    fn context<'a>(
        obs: &'a Observation,
        map: &'a PublicMapBriefing,
        profile: &'a ResolvedProfile,
        resources: &'a ResourceSnapshot,
    ) -> EconomicInvestmentContext<'a> {
        EconomicInvestmentContext {
            obs,
            resources,
            profile,
            briefing: map,
            orientation: super::super::super::orient::Orientation::for_home(
                obs,
                TilePos::new(2, 12),
            ),
            unavailable: &[],
            demands: &[],
            cadence: 12,
            unit_contacts: &[],
            building_contacts: &[],
            protected_scrap: 0,
            air_work: &[],
        }
    }

    fn proposals(
        policy: &mut UtilityPolicy,
        obs: &Observation,
        map: &PublicMapBriefing,
        profile: &ResolvedProfile,
    ) -> Vec<ReconProposal> {
        let resources = ResourceSnapshot::from_observation(obs);
        let context = context(obs, map, profile, &resources);
        policy.observe_reconnaissance(context, TilePos::new(2, 12));
        policy.prepare_reconnaissance(
            context,
            DifficultyTuning::for_level(profile.difficulty),
            0,
            true,
            &[],
        )
    }

    #[test]
    fn operational_scout_covers_only_its_useful_question_without_transferring_ownership() {
        let (mut obs, map, profile) = fixture();
        let mut policy = UtilityPolicy::new();
        policy.reconnaissance.operational = Some(OperationalReconWork {
            target: TilePos::new(32, 4),
            scout: Some(UnitId(100)),
            goal: Some(TilePos::new(30, 4)),
            deadline: obs.tick + HORIZON,
            paid: vec![],
        });
        let quoted = proposals(&mut policy, &obs, &map, &profile);
        assert_eq!(policy.reconnaissance.covered.len(), 1);
        assert!(
            quoted
                .iter()
                .all(|proposal| proposal.question.key.tile() != TilePos::new(32, 4))
        );
        assert!(
            quoted
                .iter()
                .all(|proposal| proposal.observer != ReconObserver::Live(UnitId(100)))
        );
        assert!(policy.reconnaissance.assignments.is_empty());
        assert!(
            quoted
                .iter()
                .any(|proposal| proposal.question.key.tile() == TilePos::new(32, 23))
        );

        obs.tick += 24;
        policy.reconnaissance.operational.as_mut().unwrap().deadline = obs.tick + 1;
        let quoted = proposals(&mut policy, &obs, &map, &profile);
        assert!(policy.reconnaissance.covered.is_empty());
        assert!(
            quoted
                .iter()
                .any(|proposal| proposal.question.key.tile() == TilePos::new(32, 4)),
            "an observer that cannot arrive in time supplies no useful coverage"
        );
    }

    #[test]
    fn a_ghost_at_an_uncleared_start_is_one_spatial_question() {
        let (mut obs, map, profile) = fixture();
        let mut ghost = obs.my_buildings[0].clone();
        ghost.player = PlayerId(1);
        ghost.anchor = TilePos::new(32, 4);
        ghost.seen = false;
        obs.enemy_buildings.push(ghost);
        let mut policy = UtilityPolicy::new();
        let quoted = proposals(&mut policy, &obs, &map, &profile);
        assert!(
            !quoted.iter().any(|proposal| {
                proposal.question.key.consumer == ReconConsumer::HostileStart(PlayerId(1))
            }),
            "remembered positive evidence must not recreate an unknown start"
        );
        assert_eq!(
            quoted
                .iter()
                .filter(|proposal| proposal.question.key.tile() == TilePos::new(32, 4))
                .count(),
            1
        );
    }

    #[test]
    fn an_owned_sighting_covers_overlapping_but_not_independent_questions() {
        let (mut obs, mut map, profile) = fixture();
        let mut policy = UtilityPolicy::new();
        let first = proposals(&mut policy, &obs, &map, &profile).remove(0);
        assert!(policy.commit_reconnaissance(first, None, &obs, &mut Vec::new()));
        let reservations = policy.reconnaissance.reservations();
        map.starting_foundries.push(StartingFoundry {
            player: PlayerId(3),
            anchor: TilePos::new(32, 5),
        });
        map.teams.push(None);
        obs.tick += 24;
        let quoted = proposals(&mut policy, &obs, &map, &profile);
        assert_eq!(policy.reconnaissance.reservations(), reservations);
        assert!(
            policy
                .reconnaissance
                .covered
                .iter()
                .any(|(key, _, _)| { key.consumer == ReconConsumer::HostileStart(PlayerId(3)) })
        );
        assert!(!quoted.iter().any(|proposal| {
            proposal.question.key.consumer == ReconConsumer::HostileStart(PlayerId(3))
        }));
        assert!(quoted.iter().any(|proposal| {
            proposal.question.key.consumer == ReconConsumer::HostileStart(PlayerId(2))
        }));
    }

    #[test]
    fn recently_answered_objective_is_not_an_immediate_new_purchase() {
        let (mut obs, map, profile) = fixture();
        let mut building = obs.my_buildings[0].clone();
        building.player = PlayerId(1);
        building.anchor = TilePos::new(32, 4);
        building.seen = true;
        obs.enemy_buildings.push(building);
        let mut intelligence = crate::bot::intelligence::StrategicIntelligence::new();
        intelligence.update(&obs);
        let seen_at = obs.tick;
        obs.enemy_buildings[0].seen = false;
        let mut policy = UtilityPolicy::new();
        for (elapsed, expected) in [(24, false), (HORIZON, true)] {
            obs.tick = seen_at + elapsed;
            intelligence.update(&obs);
            let resources = ResourceSnapshot::from_observation(&obs);
            let mut context = context(&obs, &map, &profile, &resources);
            context.building_contacts = intelligence.buildings();
            policy.observe_reconnaissance(context, TilePos::new(2, 12));
            assert_eq!(
                policy.reconnaissance.questions.keys().any(|key| {
                    matches!(key.consumer, ReconConsumer::Objective(PlayerId(1), _))
                }),
                expected
            );
        }
    }

    #[test]
    fn independent_questions_keep_disjoint_observers_and_question_local_loss_recovery() {
        let (mut obs, map, profile) = fixture();
        let mut policy = UtilityPolicy::new();
        let first = proposals(&mut policy, &obs, &map, &profile).remove(0);
        let first_key = first.question.key;
        let ReconObserver::Live(first_unit) = first.observer else {
            panic!("live scout")
        };
        assert!(policy.commit_reconnaissance(first, None, &obs, &mut Vec::new()));
        obs.tick += 24;
        let second = proposals(&mut policy, &obs, &map, &profile).remove(0);
        let second_key = second.question.key;
        let ReconObserver::Live(second_unit) = second.observer else {
            panic!("live scout")
        };
        assert_ne!(first_key, second_key);
        assert_ne!(first_unit, second_unit);
        assert!(policy.commit_reconnaissance(second, None, &obs, &mut Vec::new()));
        assert_eq!(
            policy.reconnaissance.reservations(),
            vec![UnitId(100), UnitId(101)]
        );
        obs.my_units.retain(|unit| unit.id != first_unit);
        obs.tick += 24;
        assert!(proposals(&mut policy, &obs, &map, &profile).is_empty());
        assert!(!policy.reconnaissance.assignments.contains_key(&first_key));
        assert_eq!(
            policy.reconnaissance.assignments[&second_key].unit,
            Some(second_unit)
        );
        assert_eq!(
            policy.reconnaissance.recovery[&first_key].retry_at,
            obs.tick + LOSS_COOLDOWN
        );
        assert!(!policy.reconnaissance.recovery.contains_key(&second_key));
    }

    #[test]
    fn quiet_bounded_recovery_can_repropose_without_new_enemy_sight() {
        let (mut obs, map, profile) = fixture();
        let mut policy = UtilityPolicy::new();
        let first = proposals(&mut policy, &obs, &map, &profile).remove(0);
        let key = first.question.key;
        let ReconObserver::Live(id) = first.observer else {
            unreachable!()
        };
        assert!(policy.commit_reconnaissance(first, None, &obs, &mut Vec::new()));
        obs.my_units.retain(|unit| unit.id != id);
        obs.tick += 24;
        proposals(&mut policy, &obs, &map, &profile);
        let retry_at = policy.reconnaissance.recovery[&key].retry_at;
        obs.tick += 24;
        assert!(
            !proposals(&mut policy, &obs, &map, &profile)
                .iter()
                .any(|proposal| proposal.question.key == key)
        );
        obs.tick = retry_at;
        let retry = proposals(&mut policy, &obs, &map, &profile)
            .into_iter()
            .find(|proposal| proposal.question.key == key)
            .expect("safe still-useful question recovers without fresh sight");
        assert!(obs.enemy_units.is_empty() && obs.enemy_buildings.is_empty());
        assert_eq!(retry.deadline, retry_at + HORIZON);
        assert!(policy.commit_reconnaissance(retry, None, &obs, &mut Vec::new()));
    }

    #[test]
    fn attention_never_cancels_recall_or_extends_a_frozen_deadline() {
        let (mut obs, map, profile) = fixture();
        let mut policy = UtilityPolicy::new();
        let first = proposals(&mut policy, &obs, &map, &profile).remove(0);
        let key = first.question.key;
        let deadline = first.deadline;
        assert!(policy.commit_reconnaissance(first, None, &obs, &mut Vec::new()));
        obs.tick = deadline;
        let resources = ResourceSnapshot::from_observation(&obs);
        let context = context(&obs, &map, &profile, &resources);
        let commands = policy.observe_reconnaissance(context, TilePos::new(2, 12));
        let tuning = DifficultyTuning {
            attention_slots: 0,
            ..DifficultyTuning::for_level(profile.difficulty)
        };
        assert!(
            policy
                .prepare_reconnaissance(context, tuning, 0, true, &[])
                .is_empty()
        );
        assert_eq!(
            policy.reconnaissance.assignments[&key].phase,
            ReconPhase::Recall
        );
        assert_eq!(
            policy.reconnaissance.assignments[&key].proposal.deadline,
            deadline
        );
        assert!(matches!(commands.as_slice(), [Intent::MoveUnits { .. }]));
        assert!(
            policy
                .observe_reconnaissance(context, TilePos::new(2, 12))
                .is_empty(),
            "one lifecycle advancement per observation"
        );
    }

    #[test]
    fn contested_recall_keeps_ownership_until_safe_return_and_requires_complete_coverage() {
        let (mut obs, map, profile) = fixture();
        let center = TilePos::new(20, 15);
        let mut policy = UtilityPolicy::new();
        policy
            .contested_harvest_regions
            .push(ContestedHarvestRegion {
                center,
                last_evidence: obs.tick,
                sweep_started_at: None,
            });
        proposals(&mut policy, &obs, &map, &profile);
        obs.tick += 24;
        let proposal = proposals(&mut policy, &obs, &map, &profile)
            .into_iter()
            .find(|proposal| proposal.question.key.consumer == ReconConsumer::HarvestRecovery)
            .expect("a quiet contested work region is a consequential question");
        let key = proposal.question.key;
        let ReconObserver::Live(id) = proposal.observer else {
            unreachable!()
        };
        assert!(policy.commit_reconnaissance(proposal, None, &obs, &mut vec![]));
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == id)
            .unwrap()
            .tile = center.offset(-2, 0);
        obs.salvage_incidents.push(center);
        obs.tick += 24;
        proposals(&mut policy, &obs, &map, &profile);
        assert_eq!(
            policy.reconnaissance.assignments[&key].phase,
            ReconPhase::Recall
        );
        obs.salvage_incidents.clear();
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == id)
            .unwrap()
            .tile = TilePos::new(12, 14);
        obs.tick += 24;
        proposals(&mut policy, &obs, &map, &profile);
        assert_eq!(policy.reconnaissance.assignments[&key].unit, Some(id));
        assert!(policy.reconnaissance.reservations().contains(&id));
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == id)
            .unwrap()
            .tile = TilePos::new(2, 12);
        obs.tick += 24;
        proposals(&mut policy, &obs, &map, &profile);
        assert!(!policy.reconnaissance.assignments.contains_key(&key));
        assert!(
            policy.harvest_location_contested(center),
            "return is not evidence that the region is safe"
        );
        let tiles: Vec<_> = UtilityPolicy::contested_region_tiles(&obs, center).collect();
        for tile in &tiles {
            obs.visible[(tile.y * obs.map_width + tile.x) as usize] = true;
        }
        let last = *tiles.last().unwrap();
        obs.visible[(last.y * obs.map_width + last.x) as usize] = false;
        obs.tick += 24;
        proposals(&mut policy, &obs, &map, &profile);
        assert!(
            policy.harvest_location_contested(center),
            "partial negative evidence cannot end a sweep"
        );
        obs.visible[(last.y * obs.map_width + last.x) as usize] = true;
        obs.tick += 24;
        proposals(&mut policy, &obs, &map, &profile);
        assert!(!policy.harvest_location_contested(center));
    }

    #[test]
    fn partial_negative_evidence_and_permuted_inputs_preserve_question_identity() {
        let (mut obs, map, profile) = fixture();
        let tile = map.starting_foundries()[0].anchor;
        obs.visible[(tile.y * obs.map_width + tile.x) as usize] = true;
        let mut policy = UtilityPolicy::new();
        let before = proposals(&mut policy, &obs, &map, &profile);
        assert!(
            before
                .iter()
                .any(|proposal| proposal.question.key.tile() == tile)
        );
        obs.my_units.reverse();
        let mut permuted = UtilityPolicy::new();
        assert_eq!(before, proposals(&mut permuted, &obs, &map, &profile));
        let size = BuildingKind::Foundry.base_stats().size;
        for dy in 0..size.1 {
            for dx in 0..size.0 {
                obs.visible[((tile.y + dy) * obs.map_width + tile.x + dx) as usize] = true;
            }
        }
        obs.tick += 24;
        assert!(
            !proposals(&mut policy, &obs, &map, &profile)
                .iter()
                .any(|proposal| proposal.question.key.tile() == tile)
        );
    }

    #[test]
    fn two_questions_own_distinct_paid_occurrences_and_only_the_completed_one_binds() {
        let (mut obs, map, profile) = fixture();
        let scout = obs.my_units[0].clone();
        obs.my_units.clear();
        let factory = BuildingId(10);
        obs.my_buildings.push(BuildingObs {
            id: factory,
            player: obs.me,
            kind: BuildingKind::Airworks,
            anchor: TilePos::new(6, 13),
            hp: BuildingKind::Airworks.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        obs.my_queues
            .push(vec![UnitKind::Kestrel, UnitKind::Kestrel]);
        obs.my_queue_progress.push(0);
        let mut policy = UtilityPolicy::new();
        let first = proposals(&mut policy, &obs, &map, &profile)
            .into_iter()
            .find(|proposal| matches!(proposal.observer, ReconObserver::Queued { .. }))
            .expect("already-paid observer is useful");
        let first_key = first.question.key;
        let ReconObserver::Queued {
            occurrence,
            ready_at,
            ..
        } = first.observer
        else {
            unreachable!()
        };
        assert_eq!(occurrence, 0);
        let mut scout = scout;
        scout.tile = first.origin;
        assert_eq!(first.claims().claimed_capital(), 0);
        assert!(policy.commit_reconnaissance(first, None, &obs, &mut Vec::new()));
        obs.tick += 24;
        let second = proposals(&mut policy, &obs, &map, &profile)
            .into_iter()
            .find(|proposal| matches!(proposal.observer, ReconObserver::Queued { .. }))
            .expect("the second question can use the second occurrence");
        let second_key = second.question.key;
        assert!(matches!(
            second.observer,
            ReconObserver::Queued { occurrence: 1, .. }
        ));
        assert_ne!(first_key, second_key);
        assert!(policy.commit_reconnaissance(second, None, &obs, &mut Vec::new()));
        let mut lost_policy = policy.clone();
        let mut lost_obs = obs.clone();
        lost_obs.tick = ready_at + 1;
        lost_obs.my_queues[1].remove(0);
        let resources = ResourceSnapshot::from_observation(&lost_obs);
        lost_policy.observe_reconnaissance(
            context(&lost_obs, &map, &profile, &resources),
            TilePos::new(2, 12),
        );
        assert!(
            !lost_policy
                .reconnaissance
                .assignments
                .contains_key(&first_key)
        );
        assert_eq!(lost_policy.reconnaissance.recovery[&first_key].attempts, 1);
        lost_obs.tick += u64::from(UnitKind::Kestrel.stats().train_ticks);
        lost_obs.my_queues[1].clear();
        lost_obs.my_units.push(scout.clone());
        let resources = ResourceSnapshot::from_observation(&lost_obs);
        lost_policy.observe_reconnaissance(
            context(&lost_obs, &map, &profile, &resources),
            TilePos::new(2, 12),
        );
        assert_eq!(
            lost_policy.reconnaissance.assignments[&second_key].unit,
            Some(scout.id),
            "a lost first occurrence cannot steal the second question's newborn"
        );
        obs.tick = ready_at + 1;
        obs.my_queues[1].remove(0);
        obs.my_units.push(scout.clone());
        let resources = ResourceSnapshot::from_observation(&obs);
        let mut late_policy = policy.clone();
        late_policy
            .reconnaissance
            .assignments
            .get_mut(&first_key)
            .unwrap()
            .proposal
            .deadline = obs.tick + 1;
        late_policy.observe_reconnaissance(
            context(&obs, &map, &profile, &resources),
            TilePos::new(2, 12),
        );
        let late = &late_policy.reconnaissance.assignments[&first_key];
        assert_eq!(
            late.unit,
            Some(scout.id),
            "late arrival cannot turn a surviving paid observer into a loss"
        );
        assert_eq!(late.phase, ReconPhase::Recall);
        assert_eq!(late.proposal.deadline, obs.tick + 1);
        assert!(!late_policy.reconnaissance.recovery.contains_key(&first_key));
        policy.observe_reconnaissance(
            context(&obs, &map, &profile, &resources),
            TilePos::new(2, 12),
        );
        assert_eq!(
            policy.reconnaissance.assignments[&first_key].unit,
            Some(scout.id)
        );
        assert_eq!(policy.reconnaissance.assignments[&second_key].unit, None);
        assert_eq!(
            policy.reconnaissance.assignments[&second_key]
                .paid_claim
                .unwrap()
                .occurrence,
            0
        );
        assert_eq!(policy.reconnaissance.paid_claims().len(), 1);
        obs.tick += 24;
        let resources = ResourceSnapshot::from_observation(&obs);
        policy.observe_reconnaissance(
            context(&obs, &map, &profile, &resources),
            TilePos::new(2, 12),
        );
        assert_eq!(
            policy.reconnaissance.assignments[&second_key]
                .paid_claim
                .unwrap()
                .occurrence,
            0,
            "an already observed sibling is not another factory completion"
        );
    }

    #[test]
    fn partial_funding_owns_one_frozen_purchase_without_emitting_forecast_commands() {
        use crate::bot::allocation::{
            AllocationCapacity, AllocationPersonality, allocate, imported_obligation,
            reconnaissance_investment_proposal,
        };
        let (mut obs, map, profile) = fixture();
        obs.my_units.clear();
        obs.tick = crate::stats::FOUNDRY_DRIP_START_TICK;
        obs.scrap = UnitKind::Kestrel.stats().cost - 5;
        obs.my_buildings.push(BuildingObs {
            id: BuildingId(10),
            player: obs.me,
            kind: BuildingKind::Airworks,
            anchor: TilePos::new(6, 13),
            hp: BuildingKind::Airworks.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        });
        obs.my_queues.push(vec![]);
        obs.my_queue_progress.push(0);
        let mut policy = UtilityPolicy::new();
        let proposal = proposals(&mut policy, &obs, &map, &profile)
            .into_iter()
            .find(|proposal| matches!(proposal.observer, ReconObserver::Purchase { .. }))
            .unwrap();
        let key = proposal.question.key;
        let resources = ResourceSnapshot::from_observation(&obs);
        let capacity =
            AllocationCapacity::from_snapshot(&resources, proposal.deadline, 12).unwrap();
        let allocation = allocate(
            &capacity,
            vec![],
            vec![reconnaissance_investment_proposal(proposal.clone())],
            AllocationPersonality::default(),
        )
        .unwrap();
        let funding = *allocation.producer_schedule.first().expect(
            "completed Foundry income funds the fixed shortfall before the useful deadline",
        );
        assert!(funding.enqueued_at > obs.tick);
        assert!(funding.forecast_scrap > 0);
        let mut intents = Vec::new();
        assert!(policy.commit_reconnaissance(proposal.clone(), Some(funding), &obs, &mut intents));
        assert!(intents.is_empty(), "a funded forecast is not a command");
        assert!(policy.reconnaissance.assignments[&key].unpaid);
        assert!(policy.reconnaissance.assignments[&key].paid_claim.is_none());
        let mut core_recovery = policy.clone();
        core_recovery.reconnaissance.release_unpaid(
            key,
            obs.tick,
            crate::bot::trace::ReconReleaseReason::CoreRecovery,
        );
        assert!(!core_recovery.reconnaissance.assignments.contains_key(&key));
        assert!(core_recovery.reconnaissance.recovery[&key].retry_at > obs.tick);
        obs.tick = funding.enqueued_at;
        obs.scrap = funding.kind.stats().cost;
        let resources = ResourceSnapshot::from_observation(&obs);
        let capacity =
            AllocationCapacity::from_snapshot(&resources, proposal.deadline, 12).unwrap();
        let work = &policy.reconnaissance.assignments[&key];
        let obligation = imported_obligation(
            crate::bot::allocation::ObligationClass::PersistentPlan,
            proposal.observed_at,
            crate::bot::allocation::ObligationKey::Reconnaissance(key),
            work.retained_claims(obs.tick),
        );
        let allocation = allocate::<()>(
            &capacity,
            vec![obligation],
            vec![],
            AllocationPersonality::default(),
        )
        .unwrap();
        let due = allocation.producer_schedule[0];
        assert_eq!(due.enqueued_at, obs.tick);
        assert_eq!(due.forecast_scrap, 0);
        assert!(policy.bind_reconnaissance_funding(key, due, &obs));
        assert!(!policy.reconnaissance.assignments[&key].unpaid);
        assert!(policy.reconnaissance.assignments[&key].paid_claim.is_some());
        policy.reconnaissance.release_unpaid(
            key,
            obs.tick,
            crate::bot::trace::ReconReleaseReason::CoreRecovery,
        );
        assert!(
            policy.reconnaissance.assignments.contains_key(&key),
            "core recovery never cancels an already-paid observer"
        );
        assert_eq!(
            policy.reconnaissance.assignments[&key].proposal.deadline,
            proposal.deadline
        );
    }
}
