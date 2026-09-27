//! Production resources, reachability, and the paid-production ledger of
//! connected operations.

use super::*;

#[derive(Debug, Clone, Copy)]
pub(super) struct ConnectedRouteContext<'a> {
    pub(super) campaign_routes: Option<&'a CampaignRoutes<'a>>,
    pub(super) unavailable_paid: &'a [PaidQueueClaim],
    pub(super) intel: &'a StrategicIntelligence,
    pub(super) home: TilePos,
    pub(super) target: TilePos,
    pub(super) public_map: Option<&'a PublicMapBriefing>,
    pub(super) orientation: Orientation,
}

impl<'a> ConnectedRouteContext<'a> {
    pub(super) fn new(
        intel: &'a StrategicIntelligence,
        public_map: Option<&'a PublicMapBriefing>,
        orientation: Orientation,
        home: TilePos,
        target: TilePos,
    ) -> Self {
        Self {
            campaign_routes: None,
            unavailable_paid: &[],
            intel,
            home,
            target,
            public_map,
            orientation,
        }
    }

    pub(super) fn with_routes(self, campaign_routes: Option<&'a CampaignRoutes<'a>>) -> Self {
        Self {
            campaign_routes,
            ..self
        }
    }

    pub(super) fn excluding_paid(self, unavailable_paid: &'a [PaidQueueClaim]) -> Self {
        Self {
            unavailable_paid,
            ..self
        }
    }

    /// Runs `use_routes` with this context bound to shared campaign routes,
    /// building them for this call when the context has none.
    fn with_navigation<T>(
        self,
        obs: &'a Observation,
        use_routes: impl FnOnce(ConnectedRouteContext<'_>, &CampaignRoutes<'_>) -> T,
    ) -> T {
        if let Some(routes) = self.campaign_routes {
            use_routes(self, routes)
        } else {
            let routes = CampaignRoutes::new(obs, self.intel, self.public_map, self.orientation);
            use_routes(self.with_routes(Some(&routes)), &routes)
        }
    }

    fn staging(self, obs: &'a Observation) -> Option<TilePos> {
        self.with_navigation(obs, |_, routes| routes.staging(self.home, self.target))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectedProductionResources {
    pub(super) snapshot: ResourceSnapshot,
    pub(super) access: ProductionAccess,
    pub(super) targets: ConnectedTargetSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectedTargetSelection {
    pub(super) target_anchors: Vec<TilePos>,
    pub(super) suppression_targets: Vec<Target>,
    pub(super) growth_order: Vec<TilePos>,
}

impl ConnectedProductionResources {
    pub(super) fn from_candidates(
        obs: &Observation,
        target: &BuildingContact,
        candidates: Vec<&BuildingContact>,
        unavailable: &[UnitId],
        route: ConnectedRouteContext<'_>,
        snapshot: &ResourceSnapshot,
        current_reserve: u32,
    ) -> Self {
        let snapshot = snapshot.after_current_reserve(current_reserve);
        let targets = connected_target_selection(obs, target, candidates, unavailable, route);
        let access = connected_production_access(obs, &targets, &snapshot, route);
        Self {
            snapshot,
            access,
            targets,
        }
    }

    pub(super) fn from_package_snapshot_after_current_reserve(
        obs: &Observation,
        target_player: PlayerId,
        package: &ConnectedForcePackage,
        route: ConnectedRouteContext<'_>,
        snapshot: &ResourceSnapshot,
        current_reserve: u32,
    ) -> Self {
        let snapshot = snapshot.after_current_reserve(current_reserve);
        let cluster = sized_target_contacts_at_anchors(
            route.intel,
            target_player,
            &package.target_anchors,
            obs.tick,
        );
        let targets = ConnectedTargetSelection {
            target_anchors: package.target_anchors.clone(),
            suppression_targets: current_cluster_suppression_needs(route.intel, &cluster).targets,
            growth_order: Vec::new(),
        };
        let access = connected_production_access(obs, &targets, &snapshot, route);
        Self {
            snapshot,
            access,
            targets,
        }
    }
}

impl StrategicPlanner {
    pub(crate) fn reconnaissance_paid_claims(
        &self,
        obs: &Observation,
        resources: &ResourceSnapshot,
        unavailable: &[PaidQueueClaim],
    ) -> Vec<PaidQueueClaim> {
        let Some(active) = self.air.as_ref().filter(|active| {
            active.op.scout.is_none()
                && matches!(
                    active.op.phase(),
                    AirOperationPhase::Recon | AirOperationPhase::Assemble
                )
        }) else {
            return Vec::new();
        };
        let deadline = self.air_capacity_deadline().unwrap_or(active.op.started_at);
        let kind = Role::Scout.unit_for(obs.faction);
        resources
            .producers()
            .iter()
            .flat_map(|lane| {
                lane.queued_readiness()
                    .filter(move |(queued, _)| *queued == kind)
                    .enumerate()
                    .filter_map(move |(occurrence, (_, ready_at))| {
                        let claim = PaidQueueClaim {
                            producer: lane.producer,
                            kind,
                            occurrence,
                        };
                        (ready_at < deadline && !unavailable.contains(&claim))
                            .then_some((ready_at, claim))
                    })
            })
            .min()
            .map(|(_, claim)| claim)
            .into_iter()
            .collect()
    }

    /// Paid purchases still supplying the retained force.
    pub(crate) fn paid_connected_production(&self) -> &[ConnectedPurchase] {
        self.air
            .as_ref()
            .filter(|active| {
                active.op.assault_admitted() && active.op.phase() <= AirOperationPhase::Assemble
            })
            .and_then(|active| active.plan.connected())
            .map_or(&[], |plan| &plan.paid_production)
    }

    /// Completed history leaves the ledger so it cannot capture later
    /// ordinary queue items.
    pub(super) fn prune_paid_production(&mut self, obs: &Observation) {
        let Some(active) = self.air.as_mut().filter(|active| {
            active.op.assault_admitted() && active.op.phase() <= AirOperationPhase::Assemble
        }) else {
            return;
        };
        let mut missing = connected_provider_shortfall(active, obs);
        let mut queued = observed_queue_multiplicity(obs);
        let AirPlan::Connected(plan) = &mut active.plan else {
            return;
        };
        plan.paid_production.retain_mut(|purchase| {
            let Some(needed) = missing.get_mut(&purchase.kind).filter(|count| **count > 0) else {
                return false;
            };
            let count = queued
                .get_mut(&(purchase.producer, purchase.kind))
                .filter(|count| **count > 0);
            let blocked = producer_queue_is_blocked(obs, purchase.producer);
            purchase.delayed |= count.is_some() && obs.tick >= purchase.ready_at && blocked;
            let owned = (purchase.issued_at == obs.tick || count.is_some())
                && (obs.tick <= purchase.ready_at || blocked || purchase.delayed);
            if owned {
                *needed -= 1;
                if let Some(count) = count {
                    *count -= 1;
                }
            }
            owned
        });
    }

    /// Only purchases emitted now cross from forecast evidence into ownership.
    pub(super) fn record_connected_purchases(
        &mut self,
        schedule: &[crate::allocation::ScheduledProducerJob],
        observed_at: Tick,
    ) {
        use crate::allocation::{ClaimOwner, ConnectedOffenseKey, ObligationKey, ProposalKey};
        let Some(ActiveAirOperation {
            plan: AirPlan::Connected(plan),
            ..
        }) = self.air.as_mut()
        else {
            return;
        };
        let owner = plan.commitment.key();
        for job in schedule.iter().filter(|job| job.enqueued_at == observed_at) {
            let key = match job.owner {
                ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(key)) => key,
                ClaimOwner::Obligation {
                    key: ObligationKey::ConnectedOffense { objective, anchor },
                    ..
                } => ConnectedOffenseKey { objective, anchor },
                _ => continue,
            };
            if key == owner {
                plan.paid_production.push(ConnectedPurchase {
                    producer: job.producer,
                    kind: job.kind,
                    issued_at: observed_at,
                    ready_at: job.ready_at,
                    delayed: false,
                });
            }
        }
    }

    /// Airworks training time still required by the current operation roster.
    /// Queued units remain factory work and therefore still contribute to the
    /// capacity signal; only completed members reduce it.
    pub(crate) fn remaining_airwork_ticks(
        &self,
        obs: &Observation,
        proposed: Option<&AirMembership>,
    ) -> Tick {
        let Some(ActiveAirOperation { op, plan }) = &self.air else {
            return 0;
        };
        if op.phase() > AirOperationPhase::Assemble {
            return 0;
        }
        let (scout, strike_aircraft, screen) = proposed.map_or(
            (op.scout, op.strike_aircraft.as_slice(), plan.screen()),
            |members| {
                (
                    members.scout,
                    members.strike_aircraft.as_slice(),
                    members.screen.as_slice(),
                )
            },
        );
        if let Some(package) = plan.package() {
            return package
                .recon
                .iter()
                .chain(&package.strike)
                .filter(|demand| demand.kind.stats().domain == oxide_sim::stats::Domain::Air)
                .map(|demand| {
                    let live = scout
                        .into_iter()
                        .chain(strike_aircraft.iter().copied())
                        .filter(|id| {
                            unit(obs, *id).is_some_and(|member| member.kind == demand.kind)
                        })
                        .count();
                    remaining_training_ticks(obs, demand.count.saturating_sub(live), demand.kind)
                })
                .fold(0, Tick::saturating_add);
        }
        let scout_kind = Role::Scout.unit_for(obs.faction);
        let screen_kind = Role::AirGround.unit_for(obs.faction);
        let bomber_kind = Role::Bomber.unit_for(obs.faction);
        let live_scout = usize::from(
            scout.is_some_and(|id| unit(obs, id).is_some_and(|member| member.kind == scout_kind)),
        );
        let live_screen = screen
            .iter()
            .filter(|id| unit(obs, **id).is_some_and(|member| member.kind == screen_kind))
            .count();
        let live_strike_aircraft = strike_aircraft
            .iter()
            .filter(|id| unit(obs, **id).is_some_and(|member| member.kind == bomber_kind))
            .count();
        let missing_scout = 1usize.saturating_sub(live_scout);
        if !op.assault_admitted() {
            return remaining_training_ticks(obs, missing_scout, scout_kind);
        }
        let missing_screen = plan.desired_screen().saturating_sub(live_screen);
        let missing_strike_aircraft = plan
            .desired_strike_aircraft()
            .saturating_sub(live_strike_aircraft);
        remaining_training_ticks(obs, missing_scout, scout_kind)
            .saturating_add(remaining_training_ticks(obs, missing_screen, screen_kind))
            .saturating_add(remaining_training_ticks(
                obs,
                missing_strike_aircraft,
                bomber_kind,
            ))
    }
}

pub(super) fn connected_provider_unavailable<'a>(
    obs: &'a Observation,
    targets: &ConnectedTargetSelection,
    unavailable: &[UnitId],
    route: ConnectedRouteContext<'a>,
) -> Vec<UnitId> {
    let mut excluded = unavailable.to_vec();
    excluded.sort_unstable();
    excluded.dedup();
    let candidates: Vec<_> = obs
        .my_units
        .iter()
        .filter(|member| excluded.binary_search(&member.id).is_err())
        .collect();
    if candidates.is_empty() {
        return excluded;
    }
    route.with_navigation(obs, |route, navigation| {
        let scout_kind = Role::Scout.unit_for(obs.faction);
        let staging = navigation.staging(route.home, route.target);
        let ground_routes = navigation.ground();
        let air_routes = navigation.air();
        excluded.extend(candidates.into_iter().filter_map(|member| {
            let compatible = if is_artillery(member.kind) {
                let origin = SuppressionOrigin {
                    tile: member.tile,
                    kind: member.kind,
                };
                staging.is_some_and(|goal| {
                    ground_routes.ground_command_reaches(member.tile, goal)
                        && targets
                            .suppression_targets
                            .iter()
                            .all(|target| navigation.reaches(origin, *target))
                })
            } else if member.kind == scout_kind || is_strike_aircraft(member.kind, obs.faction) {
                targets.target_anchors.iter().all(|anchor| {
                    member.kind.stats().domain == Domain::Air
                        && air_routes.reaches(member.tile, *anchor)
                })
            } else {
                true
            };
            (!compatible).then_some(member.id)
        }));
        excluded.sort_unstable();
        excluded.dedup();
        excluded
    })
}

pub(super) fn connected_production_access<'a>(
    obs: &'a Observation,
    targets: &ConnectedTargetSelection,
    resources: &ResourceSnapshot,
    route: ConnectedRouteContext<'a>,
) -> ProductionAccess {
    route.with_navigation(obs, |route, navigation| {
        let staging = navigation.staging(route.home, route.target);
        let ground_routes = navigation.ground();
        let air_routes = navigation.air();
        let mut allowed = Vec::new();
        let mut paid_allowed = Vec::new();

        for lane in resources.producers() {
            let Some((producer_index, producer)) = obs
                .my_buildings
                .iter()
                .enumerate()
                .find(|(_, building)| building.id == lane.producer)
            else {
                continue;
            };
            let trainable = lane.trainable();
            let mut paid = obs
                .my_queues
                .get(producer_index)
                .cloned()
                .unwrap_or_default();
            paid.sort_unstable();
            paid.dedup();
            let mut candidates = trainable.to_vec();
            candidates.extend_from_slice(&paid);
            candidates.sort_unstable();
            candidates.dedup();

            for kind in candidates {
                let accessible = match kind.stats().domain {
                    Domain::Ground if is_artillery(kind) => staging.is_some_and(|staging| {
                        production_spawn_doorstep(
                            QueryPurpose::AirOperation,
                            obs,
                            producer,
                            route.public_map,
                            Some(route.orientation),
                        )
                        .is_some_and(|spawn| {
                            let origin = SuppressionOrigin { tile: spawn, kind };
                            ground_routes.ground_command_reaches(spawn, staging)
                                && targets
                                    .suppression_targets
                                    .iter()
                                    .all(|target| navigation.reaches(origin, *target))
                        })
                    }),
                    Domain::Air
                        if kind == Role::Scout.unit_for(obs.faction)
                            || is_strike_aircraft(kind, obs.faction) =>
                    {
                        let spawn = routing::air_production_spawn_tile(producer, None);
                        targets
                            .target_anchors
                            .iter()
                            .all(|anchor| air_routes.reaches(spawn, *anchor))
                    }
                    Domain::Ground | Domain::Air => false,
                };
                if accessible {
                    if trainable.binary_search(&kind).is_ok() {
                        allowed.push((producer.id, kind));
                    }
                    if paid.binary_search(&kind).is_ok() {
                        paid_allowed.push((producer.id, kind));
                    }
                }
            }
        }

        ProductionAccess::restricted_kinds_with_paid(allowed, paid_allowed)
            .excluding_paid(route.unavailable_paid)
    })
}

/// Keeps `target` and every other candidate the operation's actual air and
/// suppression tactics can reach.
pub(super) fn connected_target_selection<'a>(
    obs: &'a Observation,
    target: &BuildingContact,
    mut candidates: Vec<&BuildingContact>,
    unavailable: &[UnitId],
    route: ConnectedRouteContext<'a>,
) -> ConnectedTargetSelection {
    route.with_navigation(obs, |route, navigation| {
        candidates.sort_unstable_by_key(|candidate| {
            (
                candidate.anchor != target.anchor,
                Reverse(building_value(candidate.kind)),
                candidate.anchor.y,
                candidate.anchor.x,
                candidate.id,
            )
        });

        let recon_origins = connected_air_origins(obs, unavailable, |kind| {
            kind == Role::Scout.unit_for(obs.faction)
        });
        let strike_origins = connected_air_origins(obs, unavailable, |kind| {
            is_strike_aircraft(kind, obs.faction)
        });
        let suppression_origins =
            connected_suppression_origins(obs, unavailable, route.public_map, route.orientation);
        let staging = navigation.staging(route.home, route.target);
        let air_routes = navigation.air();
        let ground_routes = navigation.ground();
        let mut target_anchors = Vec::new();
        let mut suppression_targets = Vec::new();
        let mut growth_order = Vec::new();

        for candidate in candidates {
            let is_original = candidate.id == target.id && candidate.anchor == target.anchor;
            let defense = current_cluster_suppression_needs(route.intel, &[candidate]);
            let mut proposed_anchors = target_anchors.clone();
            proposed_anchors.push(candidate.anchor);
            proposed_anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
            proposed_anchors.dedup();
            let mut proposed_suppression = suppression_targets.clone();
            proposed_suppression.extend(defense.targets.iter().copied());
            proposed_suppression.sort_unstable();
            proposed_suppression.dedup();

            let air_reachable =
                connected_family_reaches_all(air_routes, &recon_origins, &proposed_anchors)
                    && connected_family_reaches_all(air_routes, &strike_origins, &proposed_anchors);
            let suppression_reachable = proposed_suppression.is_empty()
                || staging.is_some_and(|staging| {
                    suppression_origins.iter().any(|origin| {
                        ground_routes.ground_command_reaches(origin.tile, staging)
                            && proposed_suppression
                                .iter()
                                .all(|target| navigation.reaches(*origin, *target))
                    })
                });
            if !is_original
                && (defense.has_untargetable_current || !air_reachable || !suppression_reachable)
            {
                continue;
            }
            target_anchors = proposed_anchors;
            suppression_targets = proposed_suppression;
            if !is_original {
                growth_order.push(candidate.anchor);
            }
        }

        ConnectedTargetSelection {
            target_anchors,
            suppression_targets,
            growth_order,
        }
    })
}

fn connected_air_origins(
    obs: &Observation,
    unavailable: &[UnitId],
    accepts: impl Fn(UnitKind) -> bool + Copy,
) -> Vec<TilePos> {
    let mut origins: Vec<_> = obs
        .my_units
        .iter()
        .filter(|unit| unit.hp > 0 && accepts(unit.kind) && !unavailable.contains(&unit.id))
        .map(|unit| unit.tile)
        .collect();
    origins.extend(
        obs.my_buildings
            .iter()
            .filter(|producer| completed_producer_can_train(obs, producer, accepts))
            .map(|producer| routing::air_production_spawn_tile(producer, None)),
    );
    origins.sort_unstable_by_key(|tile| (tile.y, tile.x));
    origins.dedup();
    origins
}

fn connected_suppression_origins<'a>(
    obs: &'a Observation,
    unavailable: &[UnitId],
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
) -> Vec<SuppressionOrigin> {
    let mut origins: Vec<_> = obs
        .my_units
        .iter()
        .filter(|unit| unit.hp > 0 && is_artillery(unit.kind) && !unavailable.contains(&unit.id))
        .map(|unit| SuppressionOrigin {
            tile: unit.tile,
            kind: unit.kind,
        })
        .collect();
    origins.extend(obs.my_buildings.iter().flat_map(|producer| {
        let spawn = production_spawn_doorstep(
            QueryPurpose::AirOperation,
            obs,
            producer,
            public_map,
            Some(orientation),
        );
        completed_producer_trainable_kinds(obs, producer)
            .into_iter()
            .filter(|kind| is_artillery(*kind))
            .filter_map(move |kind| spawn.map(|tile| SuppressionOrigin { tile, kind }))
    }));
    origins.sort_unstable_by_key(|origin| (origin.tile.y, origin.tile.x, origin.kind));
    origins.dedup();
    origins
}

fn completed_producer_trainable_kinds(
    obs: &Observation,
    producer: &crate::observation::BuildingObs,
) -> Vec<UnitKind> {
    if producer.player != obs.me || !producer.built || producer.hp == 0 {
        return Vec::new();
    }
    let completed = |kind: BuildingKind| {
        obs.my_buildings.iter().any(|building| {
            building.player == obs.me && building.kind == kind && building.built && building.hp > 0
        })
    };
    producer
        .kind
        .tier_stats(producer.tier)
        .produces
        .iter()
        .copied()
        .filter(|kind| {
            kind.faction().is_none_or(|faction| faction == obs.faction)
                && kind.stats().requires.iter().copied().all(completed)
        })
        .collect()
}

fn completed_producer_can_train(
    obs: &Observation,
    producer: &crate::observation::BuildingObs,
    accepts: impl Fn(UnitKind) -> bool,
) -> bool {
    completed_producer_trainable_kinds(obs, producer)
        .into_iter()
        .any(accepts)
}

fn connected_family_reaches_all(
    routes: &RouteProjection<'_>,
    origins: &[TilePos],
    targets: &[TilePos],
) -> bool {
    origins.iter().any(|origin| {
        targets
            .iter()
            .all(|target| routes.reaches(*origin, *target))
    })
}

pub(super) fn connected_provider_shortfall(
    active: &ActiveAirOperation,
    obs: &Observation,
) -> BTreeMap<UnitKind, usize> {
    let Some(package) = active.plan.package() else {
        return BTreeMap::new();
    };
    let mut missing = BTreeMap::new();
    for demand in package
        .recon
        .iter()
        .chain(&package.suppression)
        .chain(&package.strike)
    {
        let count = missing.entry(demand.kind).or_insert(0usize);
        *count = count.saturating_add(demand.count);
    }
    for id in reservations(&active.op, &active.plan, obs) {
        let Some(member) = unit(obs, id) else {
            continue;
        };
        if let Some(count) = missing.get_mut(&member.kind) {
            *count = count.saturating_sub(1);
        }
    }
    missing
}

/// Accepts every observed queue occurrence except those another program holds.
///
/// Emergency economy recovery asks only whether remaining demand still needs
/// scrap, so it does not narrow producers by tactical route the way a package
/// derivation does. Route eligibility decides whether the operation can
/// succeed, and the ordinary preparation checks own that question.
pub(super) fn emergency_paid_queue_access(
    obs: &Observation,
    excluded: &[PaidQueueClaim],
) -> ProductionAccess {
    let paid_allowed = obs
        .my_buildings
        .iter()
        .zip(&obs.my_queues)
        .flat_map(|(producer, queue)| queue.iter().map(|&kind| (producer.id, kind)))
        .collect();
    ProductionAccess::restricted_kinds_with_paid(Vec::new(), paid_allowed).excluding_paid(excluded)
}

fn observed_queue_multiplicity(obs: &Observation) -> BTreeMap<(BuildingId, UnitKind), usize> {
    let mut queued = BTreeMap::new();
    for (producer, queue) in obs.my_buildings.iter().zip(&obs.my_queues) {
        for &kind in queue {
            let count = queued.entry((producer.id, kind)).or_insert(0usize);
            *count = count.saturating_add(1);
        }
    }
    queued
}

fn producer_queue_is_blocked(obs: &Observation, producer: BuildingId) -> bool {
    let Some(index) = obs
        .my_buildings
        .iter()
        .position(|building| building.id == producer)
    else {
        return false;
    };
    let Some(&front) = obs.my_queues.get(index).and_then(|queue| queue.first()) else {
        return false;
    };
    front.stats().domain == Domain::Ground
        && obs
            .my_queue_progress
            .get(index)
            .is_some_and(|progress| *progress >= front.stats().train_ticks)
}

pub(super) fn missing_package_demands(
    package: &ConnectedForcePackage,
    members: AirRoster<'_>,
    obs: &Observation,
    resources: &ResourceSnapshot,
    deadline: Tick,
    production_access: &ProductionAccess,
) -> Vec<ProviderDemandTranche> {
    let mut available = Vec::<(ForceFamily, UnitKind, usize)>::new();
    let mut missing = Vec::new();
    for demand in &package.provider_priority {
        let assigned = match demand.family {
            ForceFamily::Recon => members.scout.as_slice(),
            ForceFamily::Suppression => members.artillery,
            ForceFamily::Strike => members.strike_aircraft,
        };
        let available_index = available
            .iter()
            .position(|(candidate_family, kind, _)| {
                *candidate_family == demand.family && *kind == demand.kind
            })
            .unwrap_or_else(|| {
                let live = assigned
                    .iter()
                    .filter(|id| unit(obs, **id).is_some_and(|member| member.kind == demand.kind))
                    .count();
                let paid = count_paid_queued_ready_with_access(
                    resources,
                    demand.kind,
                    deadline,
                    production_access,
                );
                available.push((demand.family, demand.kind, live.saturating_add(paid)));
                available.len() - 1
            });
        let supplied = demand.count.min(available[available_index].2);
        available[available_index].2 -= supplied;
        let count = demand.count - supplied;
        if count > 0 {
            missing.push(ProviderDemandTranche {
                priority: demand.priority,
                family: demand.family,
                kind: demand.kind,
                count,
            });
        }
    }
    missing
}

/// Proves that the exact demanded artillery count can accept its eventual
/// authoritative group spread from the same component used by provider
/// admission. Live artillery and every eligible producer doorstep are already
/// required to reach `source_staging`, so this covers both existing and future
/// members without guessing where a not-yet-trained unit will stand.
pub(super) fn connected_artillery_group_has_staging(
    obs: &Observation,
    route: ConnectedRouteContext<'_>,
    demands: &[ProviderDemand],
    preferred: &[UnitId],
    unavailable: &[UnitId],
) -> bool {
    let count = demand_count(demands);
    if count == 0 {
        return true;
    }
    let Some(source_staging) = route.staging(obs) else {
        return false;
    };
    route.with_navigation(obs, |_, navigation| {
        let routes = navigation.ground();
        let exact_live = exact_live_provider_group(obs, demands, preferred, unavailable);
        artillery_staging_candidates(obs, route.home, route.target, route.public_map)
            .into_iter()
            .any(|candidate| {
                artillery_group_reaches_staging(
                    routes,
                    source_staging,
                    candidate,
                    count,
                    exact_live.as_deref(),
                )
            })
    })
}

pub(super) fn connected_suppression_roster_has_firing_assignments(
    obs: &Observation,
    route: ConnectedRouteContext<'_>,
    demands: &[ProviderDemand],
    targets: &[Target],
) -> bool {
    if targets.is_empty() {
        return true;
    }
    route.with_navigation(obs, |route, navigation| {
        let Some(staging) = navigation.staging(route.home, route.target) else {
            return false;
        };
        let mut demands = demands.to_vec();
        demands.sort_unstable_by_key(|demand| demand.kind);
        let origins: Vec<_> = demands
            .iter()
            .flat_map(|demand| {
                std::iter::repeat_n(
                    SuppressionOrigin {
                        tile: staging,
                        kind: demand.kind,
                    },
                    demand.count,
                )
            })
            .collect();
        !origins.is_empty()
            && targets
                .iter()
                .all(|target| navigation.assignment(&origins, *target).is_some())
    })
}

pub(super) fn artillery_group_reaches_staging(
    routes: &RouteProjection<'_>,
    source_staging: TilePos,
    candidate: TilePos,
    count: usize,
    exact_live: Option<&[UnitId]>,
) -> bool {
    if let Some(units) = exact_live {
        routes.group_reaches_command_goal(units, candidate)
    } else {
        routes.all_command_spreads_reachable_from(source_staging, candidate, count)
    }
}

pub(super) fn exact_live_provider_group(
    obs: &Observation,
    demands: &[ProviderDemand],
    preferred: &[UnitId],
    unavailable: &[UnitId],
) -> Option<Vec<UnitId>> {
    let mut selected = preferred.to_vec();
    assign_provider_demands(&mut selected, demands, obs, unavailable);
    demands
        .iter()
        .all(|demand| {
            selected
                .iter()
                .filter(|id| unit(obs, **id).is_some_and(|member| member.kind == demand.kind))
                .count()
                >= demand.count
        })
        .then_some(selected)
}
