//! Immutable production demand for the existing tactical pair.

use super::*;
use crate::bot::PublicMapBriefing;
use crate::bot::orient::Orientation;
use crate::bot::resources::{ProducerEgress, ResourceSnapshot};
use crate::bot::routing::production_spawn_doorstep;

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
    paid: Vec<(BuildingId, usize)>,
}

impl RaidProcurementRequest {
    pub(crate) fn with_paid_ownership(
        mut self,
        owned: &[crate::bot::standing_force::StandingProductionCommitment],
    ) -> Self {
        let available = self
            .paid
            .iter()
            .map(|(producer, count)| {
                count.saturating_sub(
                    owned
                        .iter()
                        .filter(|commitment| commitment.matches(*producer, UnitKind::Scuttler))
                        .count(),
                )
            })
            .sum::<usize>();
        self.missing = RAID_GROUP_SIZE.saturating_sub(self.members.len().saturating_add(available));
        self
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
        let invalid = self.preparation.as_ref().is_some_and(|request| {
            if obs.tick >= request.deadline {
                return true;
            }
            let Some(target) = current_target(obs, request) else {
                return true;
            };
            let routes = procurement_routes(obs, briefing, orientation);
            let live = request
                .members
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
                .filter_map(|id| obs.my_buildings.iter().find(|building| building.id == *id))
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
        if invalid {
            self.preparation = None;
            if self.active.is_none() {
                self.muster.clear();
            }
            self.cooldown_until = self.cooldown_until.max(obs.tick.saturating_add(300));
        }
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
            let mut home_exclusions = members.clone();
            home_exclusions.extend_from_slice(context.enlisted);
            home_exclusions.extend_from_slice(context.additionally_reserved);
            if !home_screen_ready(context.profile, obs, context.home, &home_exclusions) {
                continue;
            }
            let mut paid = 0_usize;
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
                paid_occurrences.push((
                    lane.producer,
                    lane.queued_kind_ready_before(UnitKind::Scuttler, ready_before),
                ));
                paid = paid.saturating_add(
                    lane.queued_kind_ready_before(UnitKind::Scuttler, ready_before)
                        .saturating_sub(owned),
                );
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
            let missing = RAID_GROUP_SIZE.saturating_sub(members.len().saturating_add(paid));
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
            || request.missing == 0
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
