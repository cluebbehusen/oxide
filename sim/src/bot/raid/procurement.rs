//! Immutable production demand for the existing tactical pair.

use super::*;
use crate::bot::PublicMapBriefing;
use crate::bot::allocation::{ClaimOwner, PaidQueueClaim, ProposalKey, ScheduledProducerJob};
use crate::bot::orient::Orientation;
use crate::bot::resources::{ProducerEgress, ResourceSnapshot};
use crate::bot::routing::production_spawn_doorstep;
use std::collections::{BTreeMap, BTreeSet};

const PREPARATION_HORIZON: Tick = 1_800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RaidProcurementRequest {
    pub(crate) observed_at: Tick,
    pub(crate) deadline: Tick,
    pub(crate) production_deadline: Tick,
    pub(crate) objective: RaidObjective,
    pub(crate) player: PlayerId,
    pub(crate) tile: TilePos,
    pub(crate) members: Vec<UnitId>,
    pub(crate) newly_claimed: Vec<UnitId>,
    pub(crate) missing: usize,
    pub(crate) eligible_producers: Vec<BuildingId>,
    pub(crate) paid: Vec<PaidQueueClaim>,
    available_paid: Vec<PaidQueueClaim>,
    retained_paid: Vec<PaidQueueClaim>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RaidPaidWork {
    claims: Vec<PaidQueueClaim>,
    counts: BTreeMap<BuildingId, usize>,
    origins: BTreeMap<BuildingId, TilePos>,
    known_units: BTreeSet<UnitId>,
    observed_at: Option<Tick>,
}

impl RaidProcurementRequest {
    pub(crate) fn new_paid_claims(&self) -> Vec<PaidQueueClaim> {
        self.paid
            .iter()
            .copied()
            .filter(|claim| !self.retained_paid.contains(claim))
            .collect()
    }
    pub(crate) fn with_paid_ownership(
        mut self,
        owned: &[crate::bot::standing_force::StandingProductionCommitment],
    ) -> Self {
        self.paid = self
            .available_paid
            .iter()
            .copied()
            .filter(|claim| {
                self.retained_paid.contains(claim)
                    || claim.occurrence
                        >= owned
                            .iter()
                            .filter(|commitment| commitment.matches(claim.producer, claim.kind))
                            .count()
            })
            .collect();
        self.paid
            .truncate(RAID_GROUP_SIZE.saturating_sub(self.members.len()));
        self.missing = RAID_GROUP_SIZE.saturating_sub(self.members.len() + self.paid.len());
        self
    }

    pub(super) fn begin(
        &self,
        obs: &Observation,
        muster: &[UnitId],
        routes: &mut RouteProjection<'_>,
    ) -> Option<RaidOperation> {
        let tile = current_target(obs, self)?;
        if obs.tick >= self.deadline || muster.len() != RAID_GROUP_SIZE {
            return None;
        }
        let members = first_reachable_group(routes, muster, RAID_GROUP_SIZE, tile)?;
        Some(RaidOperation {
            target_player: self.player,
            objective: self.objective,
            last_tile: tile,
            members,
            committed_size: RAID_GROUP_SIZE,
            phase: RaidPhase::Ingress,
            started_at: obs.tick,
            phase_started_at: obs.tick,
            exit_reason: None,
            dispatch: None,
        })
    }
}

impl RaidPlanner {
    pub(crate) fn reconcile_procurement(&mut self, obs: &Observation) {
        self.reconcile_procurement_routes(obs, None, None);
    }

    pub(crate) fn reconcile_procurement_routes(
        &mut self,
        obs: &Observation,
        briefing: Option<&PublicMapBriefing>,
        orientation: Option<Orientation>,
    ) {
        if self.paid_work.observed_at == Some(obs.tick) {
            return;
        }
        self.paid_work.observed_at = Some(obs.tick);
        let lost_paid = self.reconcile_paid(obs, briefing, orientation);
        let invalid = self.preparation.as_ref().is_some_and(|request| {
            if obs.tick >= request.deadline
                || self.muster.iter().any(|id| own_unit(obs, *id).is_none())
            {
                return true;
            }
            let Some(target) = current_target(obs, request) else {
                return true;
            };
            let routes = procurement_routes(obs, briefing, orientation);
            let live = self
                .muster
                .iter()
                .filter_map(|id| own_unit(obs, *id))
                .any(|unit| {
                    routes
                        .prospective_group_route_cost(unit.tile, target, RAID_GROUP_SIZE)
                        .is_some()
                });
            let producer = request
                .eligible_producers
                .iter()
                .copied()
                .chain(self.paid_work.claims.iter().map(|claim| claim.producer))
                .filter_map(|id| obs.my_buildings.iter().find(|building| building.id == id))
                .any(|building| {
                    production_spawn_doorstep(obs, building, briefing, orientation).is_some_and(
                        |origin| {
                            routes
                                .prospective_group_route_cost(origin, target, RAID_GROUP_SIZE)
                                .is_some()
                        },
                    )
                });
            !live && !producer
        });
        if invalid || lost_paid {
            self.preparation = None;
            self.paid_work = RaidPaidWork {
                observed_at: Some(obs.tick),
                ..Default::default()
            };
            if self.active.is_none() {
                self.muster.clear();
            }
            self.cooldown_until = self.cooldown_until.max(obs.tick.saturating_add(300));
        }
    }

    pub(crate) fn paid_claims(&self) -> &[PaidQueueClaim] {
        &self.paid_work.claims
    }

    pub(crate) fn preparation_started_at(&self) -> Option<Tick> {
        self.preparation.as_ref().map(|request| request.observed_at)
    }

    fn reconcile_paid(
        &mut self,
        obs: &Observation,
        briefing: Option<&PublicMapBriefing>,
        orientation: Option<Orientation>,
    ) -> bool {
        if self.paid_work.claims.is_empty() {
            return false;
        }
        let mut lost = false;
        let mut retained = Vec::new();
        for (&producer, &before) in &self.paid_work.counts {
            let Some(index) = obs
                .my_buildings
                .iter()
                .position(|building| building.id == producer && building.hp > 0)
            else {
                lost |= self
                    .paid_work
                    .claims
                    .iter()
                    .any(|claim| claim.producer == producer);
                continue;
            };
            let now = obs.my_queues.get(index).map_or(0, |queue| {
                queue
                    .iter()
                    .filter(|kind| **kind == UnitKind::Scuttler)
                    .count()
            });
            let Some(&origin) = self.paid_work.origins.get(&producer) else {
                lost |= self
                    .paid_work
                    .claims
                    .iter()
                    .any(|claim| claim.producer == producer);
                continue;
            };
            let mut births = obs
                .my_units
                .iter()
                .filter(|unit| {
                    unit.kind == UnitKind::Scuttler
                        && unit.hp > 0
                        && !self.paid_work.known_units.contains(&unit.id)
                        && unit.tile.chebyshev(origin) <= 2
                        && obs
                            .my_buildings
                            .iter()
                            .filter(|building| {
                                building.id != producer
                                    && building
                                        .kind
                                        .base_stats()
                                        .produces
                                        .contains(&UnitKind::Scuttler)
                            })
                            .all(|building| {
                                production_spawn_doorstep(obs, building, briefing, orientation)
                                    .is_none_or(|other| {
                                        unit.tile.chebyshev(other) > unit.tile.chebyshev(origin)
                                    })
                            })
                })
                .map(|unit| unit.id)
                .collect::<Vec<_>>();
            births.sort_unstable();
            let completed = before.saturating_sub(now).max(births.len());
            for claim in self
                .paid_work
                .claims
                .iter()
                .filter(|claim| claim.producer == producer)
            {
                if claim.occurrence < completed {
                    // Missing births cannot shift a later occurrence into this claim.
                    if births.len() == completed {
                        self.muster.push(births[claim.occurrence]);
                    } else {
                        lost = true;
                    }
                } else {
                    retained.push(PaidQueueClaim {
                        occurrence: claim.occurrence - completed,
                        ..*claim
                    });
                }
            }
        }
        self.muster.sort_unstable();
        self.muster.dedup();
        self.paid_work.claims = retained;
        self.observe_paid_inventory(obs, briefing, orientation);
        lost
    }

    fn observe_paid_inventory(
        &mut self,
        obs: &Observation,
        briefing: Option<&PublicMapBriefing>,
        orientation: Option<Orientation>,
    ) {
        self.paid_work.known_units = obs.my_units.iter().map(|unit| unit.id).collect();
        self.paid_work.counts.clear();
        self.paid_work.origins.clear();
        for (index, building) in obs.my_buildings.iter().enumerate() {
            let count = obs.my_queues.get(index).map_or(0, |queue| {
                queue
                    .iter()
                    .filter(|kind| **kind == UnitKind::Scuttler)
                    .count()
            });
            self.paid_work.counts.insert(building.id, count);
            if let Some(origin) = production_spawn_doorstep(obs, building, briefing, orientation) {
                self.paid_work.origins.insert(building.id, origin);
            }
        }
    }

    pub(crate) fn bind_procurement(
        &mut self,
        obs: &Observation,
        jobs: &[ScheduledProducerJob],
        briefing: &PublicMapBriefing,
        orientation: Orientation,
    ) -> bool {
        let Some(request) = self.preparation.as_ref() else {
            return false;
        };
        let owner = ClaimOwner::Proposal(ProposalKey::StandingForce(
            crate::bot::allocation::StandingForceKey {
                kind: UnitKind::Scuttler,
                service: crate::bot::standing_force::StandingGroundTarget::point(request.tile),
            },
        ));
        let mut claims = request.paid.clone();
        self.observe_paid_inventory(obs, Some(briefing), Some(orientation));
        for job in jobs
            .iter()
            .filter(|job| job.enqueued_at == obs.tick && job.kind == UnitKind::Scuttler)
        {
            let occurrence = self.paid_work.counts.entry(job.producer).or_default();
            if job.owner == owner {
                claims.push(PaidQueueClaim {
                    producer: job.producer,
                    kind: job.kind,
                    occurrence: *occurrence,
                });
            }
            *occurrence += 1;
        }
        if claims
            .iter()
            .any(|claim| !self.paid_work.origins.contains_key(&claim.producer))
        {
            return false;
        }
        claims.sort_unstable();
        claims.dedup();
        self.paid_work.claims = claims;
        self.paid_work.observed_at = Some(obs.tick);
        true
    }

    #[cfg(test)]
    pub(in crate::bot) fn procurement_request(
        &self,
        context: RaidPlanningContext<'_>,
        resources: &ResourceSnapshot,
        briefing: Option<&PublicMapBriefing>,
        orientation: Option<Orientation>,
    ) -> Option<RaidProcurementRequest> {
        self.muster_request(context, resources, briefing, orientation)
            .filter(|request| request.missing > 0)
    }

    pub(in crate::bot) fn muster_request(
        &self,
        context: RaidPlanningContext<'_>,
        resources: &ResourceSnapshot,
        briefing: Option<&PublicMapBriefing>,
        orientation: Option<Orientation>,
    ) -> Option<RaidProcurementRequest> {
        let obs = context.obs;
        if !context.allow_new_operation
            || self.active.is_some()
            || obs.tick < self.cooldown_until
            || !strategic_admission_tick(obs.tick)
        {
            return None;
        }
        let deadline = self.preparation.as_ref().map_or_else(
            || obs.tick.saturating_add(PREPARATION_HORIZON),
            |request| request.deadline,
        );
        if obs.tick >= deadline {
            return None;
        }
        let targets = if let Some(request) = &self.preparation {
            let tile = current_target(obs, request)?;
            vec![(request.player, request.objective, tile)]
        } else {
            target_candidates(obs)
        };
        if targets.is_empty() {
            return None;
        }
        let routes = procurement_routes(obs, briefing, orientation);
        for (player, objective, tile) in targets {
            let mut members = obs
                .my_units
                .iter()
                .filter(|unit| {
                    unit.hp > 0
                        && unit.kind == UnitKind::Scuttler
                        && unit.idle
                        && !obs.my_queued_units.contains(&unit.id)
                        && !context.enlisted.contains(&unit.id)
                        && (!context.additionally_reserved.contains(&unit.id)
                            || self.muster.contains(&unit.id))
                        && (self.preparation.is_none() || self.muster.contains(&unit.id))
                        && routes
                            .prospective_group_route_cost(unit.tile, tile, RAID_GROUP_SIZE)
                            .is_some_and(|cost| {
                                obs.tick.saturating_add(raid_travel(cost)) < deadline
                            })
                })
                .map(|unit| unit.id)
                .collect::<Vec<_>>();
            members.sort_unstable();
            members.truncate(RAID_GROUP_SIZE);
            if self.preparation.is_some() && members.len() != self.muster.len() {
                continue;
            }
            let mut home_exclusions = members.clone();
            home_exclusions.extend_from_slice(context.enlisted);
            home_exclusions.extend_from_slice(context.additionally_reserved);
            if !home_screen_ready(context.profile, obs, context.home, &home_exclusions) {
                continue;
            }
            let mut paid_occurrences = Vec::new();
            let mut producers = Vec::new();
            let mut production_deadline = deadline;
            for lane in resources.producers() {
                let Some(building) = obs.my_buildings.iter().find(|b| b.id == lane.producer) else {
                    continue;
                };
                let Some(origin) = production_spawn_doorstep(obs, building, briefing, orientation)
                else {
                    continue;
                };
                let Some(cost) = routes.prospective_group_route_cost(origin, tile, RAID_GROUP_SIZE)
                else {
                    continue;
                };
                let ready_before = deadline.saturating_sub(raid_travel(cost));
                let owned = context
                    .paid_production
                    .iter()
                    .filter(|commitment| commitment.matches(lane.producer, UnitKind::Scuttler))
                    .count();
                for occurrence in 0..lane.queued_kind_ready_before(UnitKind::Scuttler, ready_before)
                {
                    let claim = PaidQueueClaim {
                        producer: lane.producer,
                        kind: UnitKind::Scuttler,
                        occurrence,
                    };
                    if self.paid_work.claims.contains(&claim)
                        || self.preparation.is_none() && occurrence >= owned
                    {
                        paid_occurrences.push(claim);
                    }
                }
                if lane
                    .production_timing(&[UnitKind::Scuttler])
                    .is_some_and(|timing| {
                        timing.current_egress == ProducerEgress::Open
                            && timing.no_block_latest_ready_tick < ready_before
                    })
                {
                    producers.push(lane.producer);
                    production_deadline = production_deadline.min(ready_before);
                }
            }
            let available_paid = paid_occurrences.clone();
            paid_occurrences.truncate(RAID_GROUP_SIZE.saturating_sub(members.len()));
            let missing = RAID_GROUP_SIZE.saturating_sub(members.len() + paid_occurrences.len());
            if missing > 0 && producers.is_empty() {
                continue;
            }
            producers.sort_unstable();
            let newly_claimed = members
                .iter()
                .copied()
                .filter(|id| !self.muster.contains(id))
                .collect();
            return Some(RaidProcurementRequest {
                observed_at: obs.tick,
                deadline,
                production_deadline,
                objective,
                player,
                tile,
                members,
                newly_claimed,
                missing,
                eligible_producers: producers,
                paid: paid_occurrences,
                available_paid,
                retained_paid: self.paid_work.claims.clone(),
            });
        }
        None
    }

    pub(crate) fn commit_procurement(
        &mut self,
        request: RaidProcurementRequest,
        now: Tick,
    ) -> bool {
        if self.active.is_some()
            || request.observed_at != now
            || request.deadline <= now
            || request.missing > RAID_GROUP_SIZE
            || self.preparation.as_ref().is_some_and(|prior| {
                prior.objective != request.objective || prior.deadline != request.deadline
            })
        {
            return false;
        }
        self.muster = request.members.clone();
        self.preparation = Some(request);
        true
    }
}

fn procurement_routes<'a>(
    obs: &'a Observation,
    briefing: Option<&'a PublicMapBriefing>,
    orientation: Option<Orientation>,
) -> RouteProjection<'a> {
    match (briefing, orientation) {
        (Some(map), Some(orientation)) => RouteProjection::with_public_terrain_and_orientation(
            obs,
            Domain::Ground,
            map,
            orientation,
        ),
        (Some(map), None) => RouteProjection::with_public_terrain(obs, Domain::Ground, map),
        (None, Some(orientation)) => {
            RouteProjection::with_orientation(obs, Domain::Ground, orientation)
        }
        _ => RouteProjection::new(obs, Domain::Ground),
    }
}

fn raid_travel(cost: u32) -> Tick {
    let speed = UnitKind::Scuttler.stats().speed.to_bits() as u128;
    u64::try_from((u128::from(cost) << 32).div_ceil(speed * 10)).unwrap_or(u64::MAX)
}

fn current_target(obs: &Observation, request: &RaidProcurementRequest) -> Option<TilePos> {
    match request.objective {
        RaidObjective::Unit { id, kind } => obs
            .enemy_units
            .iter()
            .find(|unit| {
                unit.id == id && unit.kind == kind && unit.player == request.player && unit.hp > 0
            })
            .map(|unit| unit.tile),
        RaidObjective::Building { id, kind } => obs
            .enemy_buildings
            .iter()
            .find(|building| {
                building.id == id
                    && building.kind == kind
                    && building.player == request.player
                    && building.hp > 0
                    && building.seen
            })
            .map(|building| building.anchor),
    }
}
