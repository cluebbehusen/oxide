//! One persistent, fog-honest strategic playbook.
//!
//! This is deliberately not a generic planner. It coordinates reconnaissance,
//! suppression, and an opportunity-scaled strike package, freezes exact members
//! at tactical commitment, then brings survivors home. Normal operations use
//! ground artillery; mature island stalemates can instead mass air attackers
//! against visible flak. Persistent membership prevents ordinary drafting from
//! turning either operation into a trickle attack.

use super::briefing::PublicMapBriefing;
use super::difficulty::{DifficultyTuning, strategic_admission_tick};
use super::executive::Intent;
use super::intelligence::{
    AirDefenseAssessment, AirDefenseEvidence, AirDefenseSource, BuildingContact, ContactEvidence,
    StrategicIntelligence,
};
use super::observation::{Observation, UnitObs};
use super::orient::Orientation;
use super::profile::{ResolvedProfile, Specialty};
use super::resources::{
    ProducerEgress, ProducerLaneReservations, ProductionAccess, ProductionDemand, ResourceSnapshot,
    count_paid_queued_ready_with_access, plan_production_with_access,
    production_demands_fit_horizon_with_access,
};
use super::routing::{self, RouteProjection};
use crate::ids::{BuildingId, PlayerId, Target, UnitId};
use crate::scenario::BotStance;
use crate::stats::{BuildingKind, Domain, QUEUE_CAP, Role, UnitKind, WeaponStats};
use chassis::Tick;
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::TilePos;
use core::cmp::Reverse;
use std::collections::VecDeque;

pub(super) mod force_package;

use force_package::{
    ConnectedForcePackage, ConnectedForcePackageOptions, ConnectedTargetEvidence, ForceFamily,
    ForcePackageRejection, NormalizedCapability, PreparationConstraints, ProductionEvidence,
    ProviderDemand, ProviderDemandTranche, building_value, current_target_cluster,
    derive_connected_force_package_options_for_cluster, provider_demands_fit_funded_horizon,
    strike_capability, suppression_capability, target_cluster_air_defense,
};

/// A connected-map combined-arms operation is an expensive second front, not
/// an opening build order. Keep a real fighting roster online before reserving
/// scouts, artillery, and strike aircraft so a seeded specialty cannot hollow
/// out the ordinary line that protects the economy.
const CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER: usize = 12;
/// A connected operation may use only completed production that can finish its
/// whole requested package inside this immutable preparation window.
const CONNECTED_PREPARATION_HORIZON: Tick = 2_400;

/// Shared preparation horizon used by connected proposal derivation and the
/// coordinator's joint resource projection.
pub(in crate::bot) const fn connected_preparation_horizon() -> Tick {
    CONNECTED_PREPARATION_HORIZON
}
const ISLAND_OPERATION_EARLIEST_TICK: Tick = 3_600;
const STRATEGIC_AIR_QUEUE_DEPTH: usize = 2;
const APPROACH_TILES: i32 = 3;
/// Once a paid operation owns units and factory capital, every difficulty gets
/// the same bounded opportunity to reacquire its objective. Longer tactical
/// memory remains useful when selecting an uncommitted target, but must not
/// make a higher rung hoard committed assets longer after sight is lost.
const ACTIVE_OPERATION_TARGET_MEMORY: Tick = 540;
const MOBILE_AA_EXPOSURE_TICKS: u64 = 200;
const MOBILE_AA_SURVIVAL_MARGIN: u64 = 2;
const DEDICATED_MOBILE_AA_WEIGHT: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AirSuppression {
    GroundArtillery,
    Airborne,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AirborneCorridorStatus {
    Clear,
    NeedsRecon,
    Defended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClusterAirDefense {
    has_targets: bool,
    targetable: Option<Target>,
    evidence: AirDefenseEvidence,
}

#[derive(Debug, Clone, Copy)]
struct ConnectedPlanningContext<'a> {
    orientation: Orientation,
    public_map: Option<&'a PublicMapBriefing>,
    resources: &'a ConnectedProductionResources,
    preferred_artillery: &'a [UnitId],
    protected_current_scrap: u32,
    preparation: PreparationConstraints,
}

#[derive(Debug, Clone, Copy)]
struct ConnectedRouteContext<'a> {
    intel: &'a StrategicIntelligence,
    home: TilePos,
    target: TilePos,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectedProductionResources {
    snapshot: ResourceSnapshot,
    access: ProductionAccess,
    targets: ConnectedTargetSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectedTargetSelection {
    target_anchors: Vec<TilePos>,
    suppression_targets: Vec<Target>,
    growth_order: Vec<TilePos>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct SuppressionOrigin {
    tile: TilePos,
    kind: UnitKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SuppressionEngagement {
    target: Target,
    firing_stands: Vec<(UnitId, TilePos)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SuppressionDispatch {
    Position {
        target: Target,
        assignments: Vec<(UnitId, TilePos)>,
    },
    Attack {
        target: Target,
        units: Vec<UnitId>,
    },
}

impl ConnectedProductionResources {
    #[cfg(test)]
    fn from_observation(
        obs: &Observation,
        target: &BuildingContact,
        unavailable: &[UnitId],
        route: ConnectedRouteContext<'_>,
    ) -> Self {
        Self::from_observation_after_current_reserve(obs, target, unavailable, route, 0)
    }

    #[cfg(test)]
    fn from_observation_after_current_reserve(
        obs: &Observation,
        target: &BuildingContact,
        unavailable: &[UnitId],
        route: ConnectedRouteContext<'_>,
        current_reserve: u32,
    ) -> Self {
        let snapshot = ResourceSnapshot::from_observation(obs);
        Self::from_snapshot_after_current_reserve(
            obs,
            target,
            unavailable,
            route,
            &snapshot,
            current_reserve,
        )
    }

    fn from_snapshot_after_current_reserve(
        obs: &Observation,
        target: &BuildingContact,
        unavailable: &[UnitId],
        route: ConnectedRouteContext<'_>,
        snapshot: &ResourceSnapshot,
        current_reserve: u32,
    ) -> Self {
        let snapshot = snapshot.after_current_reserve(current_reserve);
        let targets = connected_target_selection(obs, target, unavailable, route);
        let access = connected_production_access(obs, &targets, &snapshot, route);
        Self {
            snapshot,
            access,
            targets,
        }
    }

    fn from_package_after_current_reserve(
        obs: &Observation,
        target_player: PlayerId,
        package: &ConnectedForcePackage,
        route: ConnectedRouteContext<'_>,
        current_reserve: u32,
    ) -> Self {
        let snapshot = ResourceSnapshot::from_observation(obs);
        Self::from_package_snapshot_after_current_reserve(
            obs,
            target_player,
            package,
            route,
            &snapshot,
            current_reserve,
        )
    }

    fn from_package_snapshot_after_current_reserve(
        obs: &Observation,
        target_player: PlayerId,
        package: &ConnectedForcePackage,
        route: ConnectedRouteContext<'_>,
        snapshot: &ResourceSnapshot,
        current_reserve: u32,
    ) -> Self {
        let snapshot = snapshot.after_current_reserve(current_reserve);
        let cluster =
            current_target_contacts_at_anchors(route.intel, target_player, &package.target_anchors);
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

    fn from_revision_after_current_reserve(
        obs: &Observation,
        target: &BuildingContact,
        package: &ConnectedForcePackage,
        route: ConnectedRouteContext<'_>,
        current_reserve: u32,
    ) -> Self {
        let snapshot = ResourceSnapshot::from_observation(obs);
        Self::from_revision_snapshot_after_current_reserve(
            obs,
            target,
            package,
            route,
            &snapshot,
            current_reserve,
        )
    }

    fn from_revision_snapshot_after_current_reserve(
        obs: &Observation,
        target: &BuildingContact,
        package: &ConnectedForcePackage,
        route: ConnectedRouteContext<'_>,
        snapshot: &ResourceSnapshot,
        current_reserve: u32,
    ) -> Self {
        let snapshot = snapshot.after_current_reserve(current_reserve);
        let cluster =
            current_target_contacts_at_anchors(route.intel, target.player, &package.target_anchors);
        let mut target_anchors: Vec<_> = cluster.iter().map(|contact| contact.anchor).collect();
        target_anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
        target_anchors.dedup();
        let targets = ConnectedTargetSelection {
            growth_order: target_anchors
                .iter()
                .copied()
                .filter(|anchor| *anchor != target.anchor)
                .collect(),
            target_anchors,
            suppression_targets: current_cluster_suppression_needs(route.intel, &cluster).targets,
        };
        let access = connected_production_access(obs, &targets, &snapshot, route);
        Self {
            snapshot,
            access,
            targets,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AirPlan {
    admitted_at: Tick,
    suppression: AirSuppression,
    desired_artillery: usize,
    desired_strike_aircraft: usize,
    desired_screen: usize,
    screen: Vec<UnitId>,
    assembly_timeout: Tick,
    suppression_dispatch: Option<SuppressionDispatch>,
    strike_dispatch: Option<AirStrikeDispatch>,
    observed_renewable: usize,
    observed_fighters: usize,
    connected_scope_anchor: Option<TilePos>,
    connected_package: Option<ConnectedForcePackage>,
    /// Provider request ordinals whose exact allocator-owned TrainAt command
    /// was emitted by a successful allocation pass.
    issued_connected_providers: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AirStrikeDispatch {
    Attack { target: BuildingId, anchor: TilePos },
    AttackMove(TilePos),
}

impl AirPlan {
    fn connected(package: ConnectedForcePackage, observed_at: Tick, scope_anchor: TilePos) -> Self {
        let desired_artillery = demand_count(&package.suppression);
        let desired_strike_aircraft = demand_count(&package.strike);
        Self {
            admitted_at: observed_at,
            suppression: AirSuppression::GroundArtillery,
            desired_artillery,
            desired_strike_aircraft,
            desired_screen: 0,
            screen: Vec::new(),
            assembly_timeout: package.preparation_deadline.saturating_sub(observed_at),
            suppression_dispatch: None,
            strike_dispatch: None,
            observed_renewable: 0,
            observed_fighters: 0,
            connected_scope_anchor: Some(scope_anchor),
            connected_package: Some(package),
            issued_connected_providers: Vec::new(),
        }
    }

    fn remembered_connected(obs: &Observation) -> Self {
        Self {
            admitted_at: obs.tick,
            suppression: AirSuppression::GroundArtillery,
            desired_artillery: 0,
            desired_strike_aircraft: 0,
            desired_screen: 0,
            screen: Vec::new(),
            assembly_timeout: CONNECTED_PREPARATION_HORIZON,
            suppression_dispatch: None,
            strike_dispatch: None,
            observed_renewable: 0,
            observed_fighters: 0,
            connected_scope_anchor: None,
            connected_package: None,
            issued_connected_providers: Vec::new(),
        }
    }

    #[cfg(test)]
    fn island(profile: &ResolvedProfile, obs: &Observation) -> Self {
        Self::island_with_production(profile, obs, StrategicProductionContext::empty())
    }

    fn island_with_production(
        profile: &ResolvedProfile,
        obs: &Observation,
        production: StrategicProductionContext<'_>,
    ) -> Self {
        let airworks = completed(obs, BuildingKind::Airworks);
        let renewable = completed(obs, BuildingKind::Extractor)
            .saturating_add(completed(obs, BuildingKind::Reclaimer));
        let fighters = combat_roster(obs);
        let stance_scale = match profile.stance {
            BotStance::Turtle => 0,
            BotStance::Balanced => 1,
            BotStance::Aggressive => 2,
        };
        let desired_strike_aircraft = 4usize
            .saturating_add(renewable / 2)
            .saturating_add(fighters / 20)
            .saturating_add(usize::from(profile.traits.air >= 60))
            .saturating_add(stance_scale);
        let desired_screen = 2usize
            .saturating_add(renewable / 3)
            .saturating_add(fighters / 40)
            .saturating_add(usize::from(profile.traits.air >= 50))
            .saturating_add(usize::from(profile.traits.guile >= 65));
        let airworks_u64 = u64::try_from(airworks).expect("the map fits in addressable memory");
        let queued_delay = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, building)| building.built && building.kind == BuildingKind::Airworks)
            .map(|(index, building)| {
                let observed_work = obs
                    .my_queues
                    .get(index)
                    .into_iter()
                    .flatten()
                    .map(|kind| u64::from(kind.stats().train_ticks))
                    .sum::<Tick>();
                let same_think_work = production
                    .prior_intents
                    .iter()
                    .filter_map(|intent| match intent {
                        Intent::TrainAt {
                            building: producer,
                            kind,
                        } if *producer == building.id => Some(u64::from(kind.stats().train_ticks)),
                        _ => None,
                    })
                    .sum::<Tick>();
                let immediate_delay = observed_work.saturating_add(same_think_work);
                let reserved_delay = production
                    .lane_reservations
                    .latest_ready_at(building.id)
                    .map_or(0, |ready_at| ready_at.saturating_sub(obs.tick));
                immediate_delay.max(reserved_delay)
            })
            .max()
            .unwrap_or(0);
        let requested_training = u64::try_from(desired_strike_aircraft)
            .expect("the roster fits in addressable memory")
            .saturating_mul(u64::from(
                Role::Bomber.unit_for(obs.faction).stats().train_ticks,
            ))
            .saturating_add(
                u64::try_from(desired_screen)
                    .expect("the roster fits in addressable memory")
                    .saturating_mul(u64::from(
                        Role::AirGround.unit_for(obs.faction).stats().train_ticks,
                    )),
            )
            .saturating_add(u64::from(
                Role::Scout.unit_for(obs.faction).stats().train_ticks,
            ));
        let assembly_timeout = 900u64
            .saturating_add(queued_delay)
            .saturating_add(requested_training.div_ceil(airworks_u64));
        Self {
            admitted_at: obs.tick,
            suppression: AirSuppression::Airborne,
            desired_artillery: 0,
            desired_strike_aircraft,
            desired_screen,
            screen: Vec::new(),
            assembly_timeout,
            suppression_dispatch: None,
            strike_dispatch: None,
            observed_renewable: renewable,
            observed_fighters: fighters,
            connected_scope_anchor: None,
            connected_package: None,
            issued_connected_providers: Vec::new(),
        }
    }

    fn airborne(&self) -> bool {
        self.suppression == AirSuppression::Airborne
    }
}

fn demand_count(demands: &[ProviderDemand]) -> usize {
    demands
        .iter()
        .map(|demand| demand.count)
        .fold(0usize, usize::saturating_add)
}

#[cfg(test)]
fn connected_plan(
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    target: &BuildingContact,
    unavailable: &[UnitId],
    context: ConnectedPlanningContext<'_>,
) -> Result<AirPlan, ConnectedPlanRejection> {
    derive_connected_package(profile, obs, intel, home, target, unavailable, context)
        .map(|package| AirPlan::connected(package, obs.tick, target.anchor))
}

fn connected_plan_options(
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    target: &BuildingContact,
    unavailable: &[UnitId],
    context: ConnectedPlanningContext<'_>,
) -> Result<Vec<AirPlan>, ConnectedPlanRejection> {
    let packages =
        derive_connected_package_options(profile, obs, intel, home, target, unavailable, context)?;
    Ok(std::iter::once(packages.minimum)
        .chain(packages.marginal)
        .map(|package| AirPlan::connected(package, obs.tick, target.anchor))
        .collect())
}

#[cfg(test)]
fn derive_connected_package(
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    target: &BuildingContact,
    unavailable: &[UnitId],
    context: ConnectedPlanningContext<'_>,
) -> Result<ConnectedForcePackage, ConnectedPlanRejection> {
    derive_connected_package_options(profile, obs, intel, home, target, unavailable, context)
        .map(ConnectedForcePackageOptions::into_largest)
}

fn derive_connected_package_options(
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    target: &BuildingContact,
    unavailable: &[UnitId],
    context: ConnectedPlanningContext<'_>,
) -> Result<ConnectedForcePackageOptions, ConnectedPlanRejection> {
    if !known_ground_connected(
        obs,
        home,
        target.anchor,
        target.kind.base_stats().size,
        context.public_map,
    ) {
        return Err(ConnectedPlanRejection::DisconnectedGroundRoute);
    }
    let route = ConnectedRouteContext {
        intel,
        home,
        target: target.anchor,
        public_map: context.public_map,
        orientation: context.orientation,
    };
    let mut selected = connected_target_subset(intel, target, &[target.anchor]);
    let mut packages = derive_connected_package_options_for_targets(
        profile,
        obs,
        target,
        unavailable,
        &selected,
        route,
        context,
    )?;
    for anchor in &context.resources.targets.growth_order {
        let mut proposed_anchors = selected.target_anchors.clone();
        proposed_anchors.push(*anchor);
        let proposed = connected_target_subset(intel, target, &proposed_anchors);
        if let Ok(proposed_packages) = derive_connected_package_options_for_targets(
            profile,
            obs,
            target,
            unavailable,
            &proposed,
            route,
            context,
        ) {
            selected = proposed;
            packages = proposed_packages;
        }
    }
    Ok(packages)
}

fn derive_connected_package_options_for_targets(
    profile: &ResolvedProfile,
    obs: &Observation,
    target: &BuildingContact,
    unavailable: &[UnitId],
    targets: &ConnectedTargetSelection,
    route: ConnectedRouteContext<'_>,
    context: ConnectedPlanningContext<'_>,
) -> Result<ConnectedForcePackageOptions, ConnectedPlanRejection> {
    let intel = route.intel;
    let access = connected_production_access(obs, targets, &context.resources.snapshot, route);
    let unavailable = connected_provider_unavailable(obs, targets, unavailable, route);
    let preparation = context.preparation;
    let protected_forecast_scrap = preparation.protected_forecast_scrap.min(
        context
            .resources
            .snapshot
            .forecast()
            .income_through(preparation.deadline)
            .amount(),
    );
    let cluster = selected_current_target_cluster(intel, target, &targets.target_anchors);
    let mut packages = derive_connected_force_package_options_for_cluster(
        profile,
        obs,
        intel,
        ConnectedTargetEvidence {
            primary: target,
            cluster: &cluster,
        },
        ProductionEvidence::new(&context.resources.snapshot, &access),
        &unavailable,
        preparation,
    )
    .map_err(|reason| ConnectedPlanRejection::Package {
        reason,
        protected_current_scrap: context.protected_current_scrap,
        protected_forecast_scrap,
    })?;
    let package_has_routes = |package: &ConnectedForcePackage| {
        connected_artillery_group_has_staging(
            obs,
            route,
            &package.suppression,
            context.preferred_artillery,
            &unavailable,
        ) && connected_suppression_roster_has_firing_assignments(
            obs,
            route,
            &package.suppression,
            &targets.suppression_targets,
        )
    };
    if !package_has_routes(&packages.minimum) {
        return Err(ConnectedPlanRejection::UnreachableGroupStaging {
            requested: demand_count(&packages.minimum.suppression),
        });
    }
    packages.marginal.truncate(
        packages
            .marginal
            .iter()
            .take_while(|package| package_has_routes(package))
            .count(),
    );
    Ok(packages)
}

fn connected_target_subset(
    intel: &StrategicIntelligence,
    target: &BuildingContact,
    anchors: &[TilePos],
) -> ConnectedTargetSelection {
    let cluster = selected_current_target_cluster(intel, target, anchors);
    let mut target_anchors: Vec<_> = cluster.iter().map(|contact| contact.anchor).collect();
    target_anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
    target_anchors.dedup();
    ConnectedTargetSelection {
        target_anchors,
        suppression_targets: current_cluster_suppression_needs(intel, &cluster).targets,
        growth_order: Vec::new(),
    }
}

fn excluding_owned(unavailable: &[UnitId], owned: &[UnitId]) -> Vec<UnitId> {
    let mut owned = owned.to_vec();
    owned.sort_unstable();
    owned.dedup();
    let mut external: Vec<_> = unavailable
        .iter()
        .copied()
        .filter(|id| owned.binary_search(id).is_err())
        .collect();
    external.sort_unstable();
    external.dedup();
    external
}

fn connected_proposal_claims(
    active: &ActiveAirOperation,
    resources: &ConnectedProductionResources,
    obs: &Observation,
) -> ConnectedOffenseClaims {
    let package = active
        .plan
        .connected_package
        .as_ref()
        .expect("a connected proposal carries a connected package");
    let provider_jobs = package
        .funded_providers
        .iter()
        .map(|provider| ConnectedProviderJob {
            kind: provider.kind,
            enqueue_not_before: provider.command_tick,
            ready_before: package.preparation_deadline,
            eligible_producers: eligible_connected_producers(
                resources,
                provider.kind,
                package.preparation_deadline,
            ),
        })
        .collect::<Vec<_>>();
    debug_assert!(
        provider_jobs
            .iter()
            .all(|job| !job.eligible_producers.is_empty()),
        "package funding requires at least one exact preflighted producer per job"
    );
    ConnectedOffenseClaims {
        units: reservations(&active.op, &active.plan, obs),
        provider_jobs,
    }
}

fn eligible_connected_producers(
    resources: &ConnectedProductionResources,
    kind: UnitKind,
    deadline: Tick,
) -> Vec<BuildingId> {
    resources
        .snapshot
        .producers()
        .iter()
        .filter(|lane| resources.access.allows(lane.producer, kind))
        .filter(|lane| {
            lane.horizon_timing(&[kind]).is_some_and(|timing| {
                timing.no_block_latest_ready_tick < deadline
                    && matches!(
                        timing.current_egress,
                        ProducerEgress::NotRequired | ProducerEgress::Open
                    )
            })
        })
        .map(|lane| lane.producer)
        .collect()
}

fn claim_additions(
    minimum: &ConnectedOffenseClaims,
    scaled: &ConnectedOffenseClaims,
) -> Option<ConnectedOffenseClaims> {
    if !minimum
        .units
        .iter()
        .all(|unit| scaled.units.binary_search(unit).is_ok())
        || !scaled.provider_jobs.starts_with(&minimum.provider_jobs)
    {
        return None;
    }
    Some(ConnectedOffenseClaims {
        units: scaled
            .units
            .iter()
            .copied()
            .filter(|unit| minimum.units.binary_search(unit).is_err())
            .collect(),
        provider_jobs: scaled.provider_jobs[minimum.provider_jobs.len()..].to_vec(),
    })
}

/// A phase of the coordinated air playbook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AirOperationPhase {
    /// Put current sight over the objective.
    Recon,
    /// Recruit or train the exact operation group.
    Assemble,
    /// Let the operation's suppression force remove current ground-targetable
    /// anti-air.
    SuppressAa,
    /// Re-observe the objective and final approach.
    Verify,
    /// Commit the strike aircraft.
    Strike,
    /// Withdraw surviving operation members.
    Recover,
}

/// Why an operation entered recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirRecoveryReason {
    /// Current sight confirmed the strike's objective was gone.
    Complete,
    /// A required assigned unit died.
    RequiredUnitLost,
    /// Current sight found anti-air the playbook could not suppress.
    NewAirDefense,
    /// A phase or the complete operation exceeded its patience.
    Timeout,
    /// Current sight disproved the target before the strike.
    ObjectiveLost,
    /// The remembered objective aged beyond the active-operation horizon.
    StaleIntelligence,
    /// No honestly plausible ground route reaches a staging tile near the
    /// operation's intended artillery line.
    UnreachableStaging,
    /// Known peak terrain seals the required air route.
    UnreachableAirRoute,
    /// The observed economy and completed producers cannot field the minimum
    /// connected package before its fixed preparation deadline.
    PreparationInfeasible,
}

/// Why a currently considered connected operation could not be admitted or
/// revised. This value is returned only with the current think; it never
/// becomes controller memory or simulation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnectedPlanRejection {
    InsufficientStandingForce {
        current: usize,
        required: usize,
    },
    DisconnectedGroundRoute,
    UnreachableGroupStaging {
        requested: usize,
    },
    Package {
        reason: ForcePackageRejection,
        protected_current_scrap: u32,
        protected_forecast_scrap: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RejectedConnectedCandidate {
    pub(super) target: BuildingContact,
    pub(super) reason: ConnectedPlanRejection,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StrategicThinkResult {
    pub(super) decision: StrategicDecision,
    pub(super) rejected_connected_candidate: Option<RejectedConnectedCandidate>,
}

impl StrategicThinkResult {
    fn from_decision(decision: StrategicDecision) -> Self {
        Self {
            decision,
            rejected_connected_candidate: None,
        }
    }
}

fn recovery_for_rejection(rejection: ConnectedPlanRejection) -> AirRecoveryReason {
    match rejection {
        ConnectedPlanRejection::DisconnectedGroundRoute
        | ConnectedPlanRejection::UnreachableGroupStaging { .. } => {
            AirRecoveryReason::UnreachableStaging
        }
        ConnectedPlanRejection::Package {
            reason: ForcePackageRejection::UntargetableCurrentAirDefense { .. },
            ..
        } => AirRecoveryReason::NewAirDefense,
        ConnectedPlanRejection::Package {
            reason: ForcePackageRejection::TargetNotActionable,
            ..
        } => AirRecoveryReason::ObjectiveLost,
        ConnectedPlanRejection::InsufficientStandingForce { .. }
        | ConnectedPlanRejection::Package { .. } => AirRecoveryReason::PreparationInfeasible,
    }
}

/// One-think terminal signal for a coordinated lift targeting the same base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AirOperationOutcome {
    Released { player: PlayerId, target: TilePos },
    Aborted { player: PlayerId, target: TilePos },
}

/// Inspectable persistent state of the active operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirOperation {
    /// Last known target owner.
    pub target_player: PlayerId,
    /// Last known target kind.
    pub target_kind: BuildingKind,
    /// Stable target footprint anchor.
    pub target: TilePos,
    /// Last live id, used only with current evidence.
    pub target_id: Option<BuildingId>,
    /// Whether current sight has admitted non-scout spending and reservations.
    pub assault_admitted: bool,
    /// Current playbook phase.
    pub phase: AirOperationPhase,
    /// Start tick of the operation.
    pub started_at: Tick,
    /// Start tick of the current phase.
    pub phase_started_at: Tick,
    /// Exact assigned scout.
    pub scout: Option<UnitId>,
    /// Last scout and destination dispatched by this operation.
    pub scout_dispatch: Option<(UnitId, TilePos)>,
    /// Last hold near home dispatched to the exact strike aircraft.
    pub strike_hold: Option<TilePos>,
    /// Last staging move dispatched to the artillery group. An explicit
    /// artillery attack clears this marker because the staging order no longer
    /// owns the group.
    pub artillery_staging: Option<TilePos>,
    /// Exact assigned Bombard or Avalanche ids, sorted.
    pub artillery: Vec<UnitId>,
    /// Exact assigned ground-strike aircraft ids, sorted.
    pub strike_aircraft: Vec<UnitId>,
    /// First issued strike tick.
    pub strike_issued_at: Option<Tick>,
    /// First tick on which exact package membership crossed the tactical
    /// commitment boundary. Recovery never erases this history.
    pub membership_frozen_at: Option<Tick>,
    /// Set only while recovering.
    pub recovery_reason: Option<AirRecoveryReason>,
}

/// Role-preserving survivors held only through the operation cooldown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AirStandby {
    scout: Option<UnitId>,
    artillery: Vec<UnitId>,
    strike_aircraft: Vec<UnitId>,
}

impl AirStandby {
    fn from_operation(op: &AirOperation, obs: &Observation) -> Self {
        let mut standby = Self {
            scout: op.scout,
            artillery: op.artillery.clone(),
            strike_aircraft: op.strike_aircraft.clone(),
        };
        standby.prune(obs);
        standby
    }

    fn prune(&mut self, obs: &Observation) {
        let scout_kind = Role::Scout.unit_for(obs.faction);
        self.scout = self
            .scout
            .filter(|id| unit(obs, *id).is_some_and(|member| member.kind == scout_kind));
        self.artillery
            .retain(|id| unit(obs, *id).is_some_and(|member| is_artillery(member.kind)));
        self.strike_aircraft.retain(|id| {
            unit(obs, *id).is_some_and(|member| is_strike_aircraft(member.kind, obs.faction))
        });
    }

    fn reservations(&self) -> Vec<UnitId> {
        let mut ids: Vec<_> = self
            .scout
            .into_iter()
            .chain(self.artillery.iter().copied())
            .chain(self.strike_aircraft.iter().copied())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

/// One strategic think's ordered requests and resource claims.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrategicDecision {
    /// Ordered intents; suppression precedes bomber holds.
    pub intents: Vec<Intent>,
    /// Canonical exact-unit claims for the executive.
    pub reservations: Vec<UnitId>,
    /// Scrap spent now or banked for the next trainable missing member.
    pub committed_scrap: u32,
}

struct AirPlanningContext<'a> {
    profile: &'a ResolvedProfile,
    tuning: DifficultyTuning,
    obs: &'a Observation,
    intel: &'a StrategicIntelligence,
    home: TilePos,
    orientation: Orientation,
    public_map: Option<&'a PublicMapBriefing>,
    enlisted: &'a [UnitId],
    landing_sites: &'a [TilePos],
    connected_resources: Option<ConnectedProductionResources>,
    production: StrategicProductionContext<'a>,
    protected_current_scrap: u32,
    protected_forecast_scrap: u32,
}

#[derive(Clone, Copy)]
struct StrategicProductionContext<'a> {
    prior_intents: &'a [Intent],
    lane_reservations: &'a ProducerLaneReservations,
}

impl StrategicProductionContext<'static> {
    fn empty() -> Self {
        Self {
            prior_intents: &[],
            lane_reservations: ProducerLaneReservations::empty(),
        }
    }
}

/// Exact transport objective and landing envelope offered to the air planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LiftSupportRequest {
    pub player: PlayerId,
    pub target: TilePos,
    pub planned_drops: Vec<TilePos>,
}

#[derive(Clone, Copy)]
pub(super) struct StrategicCoordination<'a> {
    pub enlisted: &'a [UnitId],
    pub lift_support: Option<&'a LiftSupportRequest>,
    pub allow_new_operation: bool,
    pub protected_current_scrap: u32,
    pub protected_forecast_scrap: u32,
    pub public_map: Option<&'a PublicMapBriefing>,
    pub orientation: Orientation,
}

/// A live operation and the plan it was admitted under. They exist only
/// together; holding them as one value makes the half-set state — which
/// a fallback once papered over by silently substituting a combined
/// plan for a possibly-island one — unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveAirOperation {
    op: AirOperation,
    plan: AirPlan,
}

/// Stable owner identity shared by a connected proposal and every exact
/// producer assignment returned for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) struct ConnectedOffenseIdentity {
    objective: BuildingId,
    anchor: TilePos,
}

impl ConnectedOffenseIdentity {
    pub(in crate::bot) const fn new(objective: BuildingId, anchor: TilePos) -> Self {
        Self { objective, anchor }
    }

    pub(in crate::bot) const fn objective(self) -> BuildingId {
        self.objective
    }

    pub(in crate::bot) const fn anchor(self) -> TilePos {
        self.anchor
    }
}

/// Exact queue-time evidence selected jointly with every other accepted
/// investment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) struct ConnectedProducerTiming {
    enqueued_at: Tick,
    starts_at: Tick,
    ready_at: Tick,
    ready_before: Tick,
}

impl ConnectedProducerTiming {
    pub(in crate::bot) const fn new(
        enqueued_at: Tick,
        starts_at: Tick,
        ready_at: Tick,
        ready_before: Tick,
    ) -> Self {
        Self {
            enqueued_at,
            starts_at,
            ready_at,
            ready_before,
        }
    }

    pub(in crate::bot) const fn enqueued_at(self) -> Tick {
        self.enqueued_at
    }

    pub(in crate::bot) const fn starts_at(self) -> Tick {
        self.starts_at
    }

    pub(in crate::bot) const fn ready_at(self) -> Tick {
        self.ready_at
    }

    pub(in crate::bot) const fn ready_before(self) -> Tick {
        self.ready_before
    }
}

/// Observation-relative division of one provider's exact cost between the
/// current bank and completed-source income.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) struct ConnectedProducerFunding {
    current_scrap: u32,
    forecast_scrap: u32,
}

impl ConnectedProducerFunding {
    pub(in crate::bot) const fn new(current_scrap: u32, forecast_scrap: u32) -> Self {
        Self {
            current_scrap,
            forecast_scrap,
        }
    }

    pub(in crate::bot) const fn current_scrap(self) -> u32 {
        self.current_scrap
    }

    pub(in crate::bot) const fn forecast_scrap(self) -> u32 {
        self.forecast_scrap
    }
}

/// One exact allocator-selected producer job bound to a connected package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) struct ConnectedProducerAssignment {
    owner: ConnectedOffenseIdentity,
    request_ordinal: usize,
    producer: BuildingId,
    kind: UnitKind,
    timing: ConnectedProducerTiming,
    funding: ConnectedProducerFunding,
}

impl ConnectedProducerAssignment {
    pub(in crate::bot) const fn new(
        owner: ConnectedOffenseIdentity,
        request_ordinal: usize,
        producer: BuildingId,
        kind: UnitKind,
        timing: ConnectedProducerTiming,
        funding: ConnectedProducerFunding,
    ) -> Self {
        Self {
            owner,
            request_ordinal,
            producer,
            kind,
            timing,
            funding,
        }
    }

    pub(in crate::bot) const fn request_ordinal(self) -> usize {
        self.request_ordinal
    }

    pub(in crate::bot) const fn producer(self) -> BuildingId {
        self.producer
    }

    pub(in crate::bot) const fn kind(self) -> UnitKind {
        self.kind
    }

    pub(in crate::bot) const fn timing(self) -> ConnectedProducerTiming {
        self.timing
    }

    pub(in crate::bot) const fn funding(self) -> ConnectedProducerFunding {
        self.funding
    }
}

/// One unpaid provider retained by an exact connected-offense proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct ConnectedProviderJob {
    kind: UnitKind,
    enqueue_not_before: Tick,
    ready_before: Tick,
    eligible_producers: Vec<BuildingId>,
}

impl ConnectedProviderJob {
    /// Concrete provider selected by the offense domain.
    pub(in crate::bot) const fn kind(&self) -> UnitKind {
        self.kind
    }

    /// First command boundary at which the package's funding evidence permits
    /// this provider.
    pub(in crate::bot) const fn enqueue_not_before(&self) -> Tick {
        self.enqueue_not_before
    }

    /// Immutable preparation deadline shared by the complete package.
    pub(in crate::bot) const fn ready_before(&self) -> Tick {
        self.ready_before
    }

    /// Exact completed producers that passed both production and route
    /// preflight for this provider.
    pub(in crate::bot) fn eligible_producers(&self) -> &[BuildingId] {
        &self.eligible_producers
    }

    #[cfg(test)]
    pub(in crate::bot) fn fixture(
        kind: UnitKind,
        enqueue_not_before: Tick,
        ready_before: Tick,
        eligible_producers: Vec<BuildingId>,
    ) -> Self {
        Self {
            kind,
            enqueue_not_before,
            ready_before,
            eligible_producers,
        }
    }
}

/// Atomic shared claims for one exact connected package.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(in crate::bot) struct ConnectedOffenseClaims {
    units: Vec<UnitId>,
    provider_jobs: Vec<ConnectedProviderJob>,
}

impl ConnectedOffenseClaims {
    /// Exact live providers reserved by this package.
    pub(in crate::bot) fn units(&self) -> &[UnitId] {
        &self.units
    }

    /// Exact unpaid production requests retained by this package.
    pub(in crate::bot) fn provider_jobs(&self) -> &[ConnectedProviderJob] {
        &self.provider_jobs
    }

    #[cfg(test)]
    pub(in crate::bot) fn fixture(
        mut units: Vec<UnitId>,
        provider_jobs: Vec<ConnectedProviderJob>,
    ) -> Self {
        units.sort_unstable();
        units.dedup();
        Self {
            units,
            provider_jobs,
        }
    }
}

/// How quickly the currently observed connected opportunity warrants action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) enum ConnectedUrgency {
    Developmental,
    Timely,
    Pressing,
}

/// Quality of the evidence supporting a connected operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) enum ConnectedConfidence {
    Prior,
    Supported,
    Current,
}

/// Strategic consequence of successfully prosecuting the retained cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) enum ConnectedStrategicValue {
    Incremental,
    Material,
    Decisive,
}

/// Time until the retained minimum can begin affecting the objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) enum ConnectedTimeToImpact {
    Patient,
    Near,
    Immediate,
}

/// Confidence that the retained minimum can execute against known defenses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) enum ConnectedExecutionSafety {
    Speculative,
    Managed,
    Secure,
}

/// Named, fog-honest comparison case for one exact connected opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) struct ConnectedOpportunityCase {
    urgency: ConnectedUrgency,
    confidence: ConnectedConfidence,
    value: ConnectedStrategicValue,
    time_to_impact: ConnectedTimeToImpact,
    safety: ConnectedExecutionSafety,
}

impl ConnectedOpportunityCase {
    pub(in crate::bot) const fn urgency(self) -> ConnectedUrgency {
        self.urgency
    }

    pub(in crate::bot) const fn confidence(self) -> ConnectedConfidence {
        self.confidence
    }

    pub(in crate::bot) const fn value(self) -> ConnectedStrategicValue {
        self.value
    }

    pub(in crate::bot) const fn time_to_impact(self) -> ConnectedTimeToImpact {
        self.time_to_impact
    }

    pub(in crate::bot) const fn safety(self) -> ConnectedExecutionSafety {
        self.safety
    }

    #[cfg(test)]
    pub(in crate::bot) const fn fixture(
        urgency: ConnectedUrgency,
        confidence: ConnectedConfidence,
        value: ConnectedStrategicValue,
        time_to_impact: ConnectedTimeToImpact,
        safety: ConnectedExecutionSafety,
    ) -> Self {
        Self {
            urgency,
            confidence,
            value,
            time_to_impact,
            safety,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectedProposalVariant {
    active: ActiveAirOperation,
    claims: ConnectedOffenseClaims,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConnectedProposalOrigin {
    Idle {
        air: Option<ActiveAirOperation>,
        standby: AirStandby,
    },
    Remembered {
        active: ActiveAirOperation,
    },
    Active {
        active: ActiveAirOperation,
    },
}

/// One deterministic cumulative addition above an accepted connected minimum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct ConnectedMarginalVariant {
    variant_index: usize,
    additions: ConnectedOffenseClaims,
}

impl ConnectedMarginalVariant {
    /// Cumulative claims added above the common minimum.
    pub(in crate::bot) const fn additions(&self) -> &ConnectedOffenseClaims {
        &self.additions
    }
}

/// One re-derived provider request paired with the persistent identity it
/// would retain if this active revision commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct ConnectedRevisionProviderJob {
    proposal_ordinal: usize,
    binding_ordinal: usize,
    request: ConnectedProviderJob,
    retained: Option<ConnectedProducerAssignment>,
}

impl ConnectedRevisionProviderJob {
    /// Position in the re-derived package's canonical provider order.
    #[cfg(test)]
    pub(in crate::bot) const fn proposal_ordinal(&self) -> usize {
        self.proposal_ordinal
    }

    /// Stable operation-local identity to bind when the request is accepted.
    pub(in crate::bot) const fn binding_ordinal(&self) -> usize {
        self.binding_ordinal
    }

    /// Current package evidence for the provider request.
    pub(in crate::bot) const fn request(&self) -> &ConnectedProviderJob {
        &self.request
    }

    /// Previously accepted exact schedule retained by this request.
    pub(in crate::bot) const fn retained(&self) -> Option<ConnectedProducerAssignment> {
        self.retained
    }
}

/// Pure provider-schedule delta for one pre-freeze active revision.
///
/// The required variant is the smallest cumulative package that still
/// contains every accepted provider the current opportunity can use. Jobs
/// above it remain optional scale. Previously accepted jobs absent from the
/// complete new ladder are explicit prunes rather than silently rebound work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct ConnectedRevisionProviderDelta {
    required_variant_index: usize,
    jobs: Vec<ConnectedRevisionProviderJob>,
    pruned: Vec<ConnectedProducerAssignment>,
}

impl ConnectedRevisionProviderDelta {
    /// Smallest cumulative proposal variant that retains every matched job.
    pub(in crate::bot) const fn required_variant_index(&self) -> usize {
        self.required_variant_index
    }

    /// Canonical jobs for the largest currently useful package.
    pub(in crate::bot) fn jobs(&self) -> &[ConnectedRevisionProviderJob] {
        &self.jobs
    }

    /// Proposal ordinals in the order the shared scheduler must replay them.
    /// Previously accepted fixed lanes precede new flexible work and are
    /// chronological within each producer, while the package itself keeps its
    /// canonical role order.
    pub(in crate::bot) fn allocation_order(&self, count: usize) -> Vec<usize> {
        let mut order = (0..count.min(self.jobs.len())).collect::<Vec<_>>();
        order.sort_unstable_by_key(|&proposal_ordinal| {
            let job = &self.jobs[proposal_ordinal];
            if let Some(retained) = job.retained {
                let timing = retained.timing();
                (
                    0,
                    retained.producer().0,
                    timing.starts_at(),
                    timing.enqueued_at(),
                    timing.ready_at(),
                    job.binding_ordinal,
                )
            } else {
                (
                    1,
                    0,
                    job.request.enqueue_not_before,
                    job.request.enqueue_not_before,
                    job.request.ready_before,
                    job.proposal_ordinal,
                )
            }
        });
        order
    }

    /// Accepted unpaid jobs that current evidence no longer requests.
    #[cfg(test)]
    pub(in crate::bot) fn pruned(&self) -> &[ConnectedProducerAssignment] {
        &self.pruned
    }
}

/// Pure, exact connected-offense proposal submitted for cross-domain
/// adjudication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct FreshConnectedProposal {
    origin: ConnectedProposalOrigin,
    target: BuildingContact,
    targets: ConnectedTargetSelection,
    variants: Vec<ConnectedProposalVariant>,
    marginal: Vec<ConnectedMarginalVariant>,
    selected_variant: usize,
    protected_current_scrap: u32,
    protected_forecast_scrap: u32,
    case: ConnectedOpportunityCase,
}

impl FreshConnectedProposal {
    pub(in crate::bot) fn revises_active_operation(&self) -> bool {
        matches!(self.origin, ConnectedProposalOrigin::Active { .. })
    }

    /// Reconciles an active operation's unpaid exact producer schedule with
    /// this proposal's current cumulative package ladder.
    pub(in crate::bot) fn active_revision_provider_delta(
        &self,
    ) -> Option<ConnectedRevisionProviderDelta> {
        let ConnectedProposalOrigin::Active { active } = &self.origin else {
            return None;
        };
        let package = active.plan.connected_package.as_ref()?;
        let assignments = package.producer_assignments.as_deref()?;
        let variant_jobs = self
            .variants
            .iter()
            .map(|variant| variant.claims.provider_jobs.as_slice())
            .collect::<Vec<_>>();
        Some(reconcile_connected_revision_provider_jobs(
            self.identity(),
            assignments,
            &active.plan.issued_connected_providers,
            &variant_jobs,
        ))
    }

    fn retain_active_revision_provider_floor(&mut self) {
        let Some(delta) = self.active_revision_provider_delta() else {
            return;
        };
        let required = delta.required_variant_index();
        if required == 0 {
            return;
        }
        self.variants.drain(..required);
        self.selected_variant = 0;
        let minimum = self.variants[0].claims.clone();
        self.marginal = self
            .variants
            .iter()
            .enumerate()
            .skip(1)
            .map(|(variant_index, variant)| ConnectedMarginalVariant {
                variant_index,
                additions: claim_additions(&minimum, &variant.claims)
                    .expect("promoted revision variants remain cumulative"),
            })
            .collect();
    }

    /// Stable identity retained by every minimum and marginal variant.
    pub(in crate::bot) fn identity(&self) -> ConnectedOffenseIdentity {
        ConnectedOffenseIdentity::new(self.objective(), self.anchor())
    }

    /// Exact current objective selected by domain ranking.
    pub(in crate::bot) fn objective(&self) -> BuildingId {
        self.target
            .id
            .expect("an admitted current objective has an exact building id")
    }

    /// Row-major anchor paired with the exact objective id.
    pub(in crate::bot) const fn anchor(&self) -> TilePos {
        self.target.anchor
    }

    /// Fixed deadline shared by the minimum and all marginal variants.
    pub(in crate::bot) fn deadline(&self) -> Tick {
        self.variants[0]
            .active
            .plan
            .connected_package
            .as_ref()
            .expect("a connected proposal carries a connected package")
            .preparation_deadline
    }

    /// Original strategic admission tick retained across remembered
    /// reconnaissance and fresh cross-domain assault adjudication.
    pub(in crate::bot) fn accepted_at(&self) -> Tick {
        self.variants[0].active.plan.admitted_at
    }

    /// Named evidence and consequence bands used by cross-domain ranking.
    pub(in crate::bot) const fn case(&self) -> ConnectedOpportunityCase {
        self.case
    }

    /// Shared claims of the independently admissible common minimum.
    pub(in crate::bot) fn minimum_claims(&self) -> &ConnectedOffenseClaims {
        &self.variants[0].claims
    }

    /// Complete claims for the minimum or selected cumulative scale variant.
    pub(in crate::bot) fn selected_claims(&self) -> &ConnectedOffenseClaims {
        &self.variants[self.selected_variant].claims
    }

    /// Deterministic cumulative additions above the common minimum. Every row
    /// preserves all earlier claims.
    pub(in crate::bot) fn marginal_variants(&self) -> &[ConnectedMarginalVariant] {
        &self.marginal
    }

    /// Selects one exact marginal variant previously returned by
    /// [`Self::marginal_variants`].
    pub(in crate::bot) fn select_marginal(&mut self, marginal: &ConnectedMarginalVariant) -> bool {
        if self.marginal.get(marginal.variant_index.saturating_sub(1)) != Some(marginal) {
            return false;
        }
        self.selected_variant = marginal.variant_index;
        true
    }

    /// Binds the allocator's exact producer schedule to the already-selected
    /// package variant. Commitment and runtime lowering consume this retained
    /// schedule without selecting another target, package, or producer.
    pub(in crate::bot) fn bind_producer_assignments(
        &mut self,
        mut assignments: Vec<ConnectedProducerAssignment>,
    ) -> Result<(), ConnectedProducerBindingError> {
        let identity = self.identity();
        let expected_ordinals = self
            .active_revision_provider_delta()
            .map(|delta| {
                delta.jobs()[..self.selected_claims().provider_jobs.len()]
                    .iter()
                    .map(ConnectedRevisionProviderJob::binding_ordinal)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| (0..self.selected_claims().provider_jobs.len()).collect());
        let selected = &mut self.variants[self.selected_variant];
        let package = selected
            .active
            .plan
            .connected_package
            .as_mut()
            .expect("a connected proposal carries a connected package");
        if package.producer_assignments.is_some() {
            return Err(ConnectedProducerBindingError::AlreadyBound);
        }
        let jobs = &selected.claims.provider_jobs;
        if assignments.len() != jobs.len() {
            return Err(ConnectedProducerBindingError::JobCount {
                expected: jobs.len(),
                actual: assignments.len(),
            });
        }
        for (position, (&request_ordinal, job)) in expected_ordinals.iter().zip(jobs).enumerate() {
            let Some(assignment) = assignments
                .iter()
                .find(|assignment| assignment.request_ordinal == request_ordinal)
            else {
                return Err(ConnectedProducerBindingError::RequestOrdinal {
                    expected: request_ordinal,
                    actual: assignments[position].request_ordinal,
                });
            };
            if assignment.owner != identity {
                return Err(ConnectedProducerBindingError::Owner { request_ordinal });
            }
            if assignment.request_ordinal != request_ordinal {
                return Err(ConnectedProducerBindingError::RequestOrdinal {
                    expected: request_ordinal,
                    actual: assignment.request_ordinal,
                });
            }
            if assignment.kind != job.kind {
                return Err(ConnectedProducerBindingError::Kind { request_ordinal });
            }
            if !job.eligible_producers.contains(&assignment.producer) {
                return Err(ConnectedProducerBindingError::Producer { request_ordinal });
            }
            let timing = assignment.timing;
            let expected_ready = timing
                .starts_at
                .checked_add(Tick::from(assignment.kind.stats().train_ticks))
                .and_then(|tick| tick.checked_sub(1));
            if timing.enqueued_at < job.enqueue_not_before
                || timing.starts_at < timing.enqueued_at
                || expected_ready != Some(timing.ready_at)
                || timing.ready_at >= timing.ready_before
                || timing.ready_before != job.ready_before
            {
                return Err(ConnectedProducerBindingError::Timing { request_ordinal });
            }
            if assignment
                .funding
                .current_scrap
                .checked_add(assignment.funding.forecast_scrap)
                != Some(assignment.kind.stats().cost)
            {
                return Err(ConnectedProducerBindingError::Funding { request_ordinal });
            }
        }
        assignments.sort_unstable_by_key(|assignment| assignment.request_ordinal);
        package.producer_assignments = Some(assignments);
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::bot) fn fixture(fixture: FreshConnectedProposalFixture) -> Self {
        let FreshConnectedProposalFixture {
            objective,
            anchor,
            deadline,
            case,
            minimum_claims,
            marginal_additions,
            protected_current_scrap,
            protected_forecast_scrap,
        } = fixture;
        let derived_at = deadline.saturating_sub(1);
        let package = ConnectedForcePackage {
            derived_at,
            preparation_deadline: deadline,
            target_anchors: vec![anchor],
            recon: Vec::new(),
            suppression: Vec::new(),
            strike: Vec::new(),
            provider_priority: Vec::new(),
            funded_providers: Vec::new(),
            producer_assignments: None,
            minimum_capability: NormalizedCapability {
                recon: 0,
                suppression: 0,
                strike: 0,
            },
            useful_capability: NormalizedCapability {
                recon: 0,
                suppression: 0,
                strike: 0,
            },
            useful_bombing: 0,
            target_value: 0,
            current_scrap: 0,
            observed_aa_firepower: 0,
            suppressible_aa_firepower: 0,
            forecast_scrap: 0,
            chosen_capability: NormalizedCapability {
                recon: 0,
                suppression: 0,
                strike: 0,
            },
            chosen_bombing: 0,
        };
        let plan = AirPlan::connected(package, derived_at, anchor);
        let op = AirOperation {
            target_player: PlayerId(1),
            target_kind: BuildingKind::Crucible,
            target: anchor,
            target_id: Some(objective),
            assault_admitted: true,
            phase: AirOperationPhase::Recon,
            started_at: derived_at,
            phase_started_at: derived_at,
            scout: None,
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery: Vec::new(),
            strike_aircraft: Vec::new(),
            strike_issued_at: None,
            membership_frozen_at: None,
            recovery_reason: None,
        };
        let active = ActiveAirOperation { op, plan };
        let mut cumulative = minimum_claims.clone();
        let mut variants = vec![ConnectedProposalVariant {
            active: active.clone(),
            claims: minimum_claims,
        }];
        let mut marginal = Vec::with_capacity(marginal_additions.len());
        for (offset, additions) in marginal_additions.into_iter().enumerate() {
            cumulative.units.extend(additions.units.iter().copied());
            cumulative.units.sort_unstable();
            cumulative.units.dedup();
            cumulative
                .provider_jobs
                .extend(additions.provider_jobs.iter().cloned());
            variants.push(ConnectedProposalVariant {
                active: active.clone(),
                claims: cumulative.clone(),
            });
            marginal.push(ConnectedMarginalVariant {
                variant_index: offset + 1,
                additions,
            });
        }
        Self {
            origin: ConnectedProposalOrigin::Idle {
                air: None,
                standby: AirStandby::default(),
            },
            target: BuildingContact {
                id: Some(objective),
                player: PlayerId(1),
                kind: BuildingKind::Crucible,
                anchor,
                hp: BuildingKind::Crucible.base_stats().max_hp,
                built: true,
                tier: 0,
                last_seen: Some(derived_at),
                evidence: ContactEvidence::Current,
            },
            targets: ConnectedTargetSelection {
                target_anchors: vec![anchor],
                suppression_targets: Vec::new(),
                growth_order: Vec::new(),
            },
            variants,
            marginal,
            selected_variant: 0,
            protected_current_scrap,
            protected_forecast_scrap,
            case,
        }
    }

    #[cfg(test)]
    pub(in crate::bot) fn into_active_revision_fixture(mut self) -> Self {
        let identity = self.identity();
        let jobs = self.variants[0].claims.provider_jobs.clone();
        let mut lane_ready = Vec::<(BuildingId, Tick)>::new();
        let assignments = jobs
            .iter()
            .enumerate()
            .map(|(request_ordinal, job)| {
                let producer = job.eligible_producers[0];
                let lane_index = lane_ready
                    .iter()
                    .position(|(candidate, _)| *candidate == producer)
                    .unwrap_or_else(|| {
                        lane_ready.push((producer, job.enqueue_not_before));
                        lane_ready.len() - 1
                    });
                let starts_at = lane_ready[lane_index].1.max(job.enqueue_not_before);
                let ready_at = starts_at
                    .saturating_add(Tick::from(job.kind.stats().train_ticks))
                    .saturating_sub(1);
                lane_ready[lane_index].1 = ready_at.saturating_add(1);
                ConnectedProducerAssignment::new(
                    identity,
                    request_ordinal,
                    producer,
                    job.kind,
                    ConnectedProducerTiming::new(
                        job.enqueue_not_before,
                        starts_at,
                        ready_at,
                        job.ready_before,
                    ),
                    ConnectedProducerFunding::new(job.kind.stats().cost, 0),
                )
            })
            .collect();
        let mut active = self.variants[0].active.clone();
        active
            .plan
            .connected_package
            .as_mut()
            .expect("the active revision fixture retains a connected package")
            .producer_assignments = Some(assignments);
        self.origin = ConnectedProposalOrigin::Active { active };
        self
    }
}

fn reconcile_connected_revision_provider_jobs(
    identity: ConnectedOffenseIdentity,
    assignments: &[ConnectedProducerAssignment],
    issued: &[usize],
    variant_jobs: &[&[ConnectedProviderJob]],
) -> ConnectedRevisionProviderDelta {
    let largest = variant_jobs
        .last()
        .copied()
        .expect("a connected proposal retains at least one package variant");
    debug_assert!(variant_jobs.iter().all(|jobs| largest.starts_with(jobs)));

    let mut outstanding = assignments
        .iter()
        .copied()
        .filter(|assignment| issued.binary_search(&assignment.request_ordinal).is_err())
        .collect::<Vec<_>>();
    outstanding.sort_unstable_by_key(|assignment| assignment.request_ordinal);
    let mut next_binding_ordinal = assignments
        .iter()
        .map(|assignment| assignment.request_ordinal)
        .max()
        .map_or(0, |ordinal| {
            ordinal
                .checked_add(1)
                .expect("an operation cannot exhaust provider request ordinals")
        });
    let mut highest_retained_proposal_ordinal = None;
    let mut jobs = Vec::with_capacity(largest.len());
    for (proposal_ordinal, request) in largest.iter().cloned().enumerate() {
        let retained_index = outstanding
            .iter()
            .position(|assignment| assignment.kind == request.kind);
        let (binding_ordinal, retained) = if let Some(index) = retained_index {
            let mut assignment = outstanding.remove(index);
            assignment.owner = identity;
            highest_retained_proposal_ordinal = Some(proposal_ordinal);
            (assignment.request_ordinal, Some(assignment))
        } else {
            let ordinal = next_binding_ordinal;
            next_binding_ordinal = next_binding_ordinal
                .checked_add(1)
                .expect("an operation cannot exhaust provider request ordinals");
            (ordinal, None)
        };
        jobs.push(ConnectedRevisionProviderJob {
            proposal_ordinal,
            binding_ordinal,
            request,
            retained,
        });
    }
    let required_variant_index = highest_retained_proposal_ordinal.map_or(0, |highest| {
        variant_jobs
            .iter()
            .position(|jobs| jobs.len() > highest)
            .expect("the largest package contains every matched provider")
    });
    ConnectedRevisionProviderDelta {
        required_variant_index,
        jobs,
        pruned: outstanding,
    }
}

#[cfg(test)]
pub(in crate::bot) struct FreshConnectedProposalFixture {
    pub(in crate::bot) objective: BuildingId,
    pub(in crate::bot) anchor: TilePos,
    pub(in crate::bot) deadline: Tick,
    pub(in crate::bot) case: ConnectedOpportunityCase,
    pub(in crate::bot) minimum_claims: ConnectedOffenseClaims,
    pub(in crate::bot) marginal_additions: Vec<ConnectedOffenseClaims>,
    pub(in crate::bot) protected_current_scrap: u32,
    pub(in crate::bot) protected_forecast_scrap: u32,
}

/// Why an accepted connected proposal could not be installed exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) enum ConnectedProposalCommitError {
    /// Planner state changed after the pure proposal was derived.
    StalePlanner,
    /// A previously admitted connected assault already owns the planner.
    ExistingAssault,
    /// The selected package was not bound to the allocator's exact producer
    /// schedule before commitment.
    UnboundProducerSchedule,
}

/// Why an allocator schedule cannot be bound to the retained proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) enum ConnectedProducerBindingError {
    AlreadyBound,
    StaleActiveOperation,
    JobCount { expected: usize, actual: usize },
    Owner { request_ordinal: usize },
    RequestOrdinal { expected: usize, actual: usize },
    Kind { request_ordinal: usize },
    Producer { request_ordinal: usize },
    Timing { request_ordinal: usize },
    Funding { request_ordinal: usize },
}

/// Mandatory continuation imported into the next allocation pass for an
/// already-admitted connected operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::bot) struct ActiveConnectedObligation {
    identity: ConnectedOffenseIdentity,
    accepted_at: Tick,
    deadline: Tick,
    units: Vec<UnitId>,
    provider_jobs: Vec<ConnectedProducerAssignment>,
}

impl ActiveConnectedObligation {
    pub(in crate::bot) const fn identity(&self) -> ConnectedOffenseIdentity {
        self.identity
    }

    pub(in crate::bot) const fn accepted_at(&self) -> Tick {
        self.accepted_at
    }

    pub(in crate::bot) const fn deadline(&self) -> Tick {
        self.deadline
    }

    pub(in crate::bot) fn units(&self) -> &[UnitId] {
        &self.units
    }

    pub(in crate::bot) fn provider_jobs(&self) -> &[ConnectedProducerAssignment] {
        &self.provider_jobs
    }

    /// Whether every unpaid immutable provider assignment still matches the
    /// current completed producer lanes and an unmissed command boundary.
    #[cfg(test)]
    pub(in crate::bot) fn producer_schedule_is_executable(
        &self,
        resources: &ResourceSnapshot,
        cadence: Tick,
        observed_at: Tick,
    ) -> bool {
        bound_provider_schedule_is_executable(
            &self.provider_jobs,
            resources,
            self.deadline,
            cadence,
            observed_at,
        )
    }
}

#[derive(Clone, Copy)]
pub(in crate::bot) struct StrategicThinkContext<'a> {
    profile: &'a ResolvedProfile,
    tuning: DifficultyTuning,
    obs: &'a Observation,
    intel: &'a StrategicIntelligence,
    home: TilePos,
    coordination: StrategicCoordination<'a>,
    production: StrategicProductionContext<'a>,
}

impl<'a> StrategicThinkContext<'a> {
    pub(in crate::bot) fn new(
        profile: &'a ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &'a Observation,
        intel: &'a StrategicIntelligence,
        home: TilePos,
        coordination: StrategicCoordination<'a>,
    ) -> Self {
        Self {
            profile,
            tuning,
            obs,
            intel,
            home,
            coordination,
            production: StrategicProductionContext::empty(),
        }
    }

    pub(in crate::bot) fn with_producer_lanes(
        mut self,
        prior_intents: &'a [Intent],
        lane_reservations: &'a ProducerLaneReservations,
    ) -> Self {
        self.production = StrategicProductionContext {
            prior_intents,
            lane_reservations,
        };
        self
    }
}

/// Complete same-observation evidence for one pure connected-offense proposal.
#[derive(Clone, Copy)]
pub(in crate::bot) struct FreshConnectedProposalRequest<'a> {
    profile: &'a ResolvedProfile,
    tuning: DifficultyTuning,
    obs: &'a Observation,
    resource_snapshot: &'a ResourceSnapshot,
    intel: &'a StrategicIntelligence,
    home: TilePos,
    coordination: StrategicCoordination<'a>,
}

impl<'a> FreshConnectedProposalRequest<'a> {
    pub(in crate::bot) const fn new(
        profile: &'a ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &'a Observation,
        resource_snapshot: &'a ResourceSnapshot,
        intel: &'a StrategicIntelligence,
        home: TilePos,
        coordination: StrategicCoordination<'a>,
    ) -> Self {
        Self {
            profile,
            tuning,
            obs,
            resource_snapshot,
            intel,
            home,
            coordination,
        }
    }
}

#[derive(Clone, Copy)]
struct FreshConnectedDerivationContext<'a> {
    profile: &'a ResolvedProfile,
    tuning: DifficultyTuning,
    obs: &'a Observation,
    resource_snapshot: &'a ResourceSnapshot,
    intel: &'a StrategicIntelligence,
    home: TilePos,
    coordination: StrategicCoordination<'a>,
    unavailable: &'a [UnitId],
    preferred_artillery: &'a [UnitId],
}

fn connected_opportunity_case(
    observed_at: Tick,
    intel: &StrategicIntelligence,
    target: &BuildingContact,
    targets: &ConnectedTargetSelection,
    minimum: &ConnectedProposalVariant,
) -> ConnectedOpportunityCase {
    let cluster = selected_current_target_cluster(intel, target, &targets.target_anchors);
    let contains = |kind| cluster.iter().any(|contact| contact.kind == kind);
    let urgency = if contains(BuildingKind::Foundry) {
        ConnectedUrgency::Pressing
    } else if [
        BuildingKind::Airworks,
        BuildingKind::Fabricator,
        BuildingKind::Crucible,
    ]
    .into_iter()
    .any(contains)
    {
        ConnectedUrgency::Timely
    } else {
        ConnectedUrgency::Developmental
    };
    let confidence = match target.evidence {
        ContactEvidence::Current => ConnectedConfidence::Current,
        ContactEvidence::Remembered if target.last_seen.is_some() => ConnectedConfidence::Supported,
        ContactEvidence::Remembered => ConnectedConfidence::Prior,
    };
    let value = if contains(BuildingKind::Foundry) {
        ConnectedStrategicValue::Decisive
    } else if cluster.len() > 1
        || cluster
            .iter()
            .any(|contact| building_value(contact.kind) >= 4)
    {
        ConnectedStrategicValue::Material
    } else {
        ConnectedStrategicValue::Incremental
    };
    let time_to_impact = if minimum.claims.provider_jobs.is_empty() {
        ConnectedTimeToImpact::Immediate
    } else if minimum
        .claims
        .provider_jobs
        .iter()
        .any(|job| job.enqueue_not_before > observed_at)
    {
        ConnectedTimeToImpact::Patient
    } else {
        ConnectedTimeToImpact::Near
    };
    let package = minimum
        .active
        .plan
        .connected_package
        .as_ref()
        .expect("a connected proposal carries a connected package");
    let meets_known_opportunity = package.chosen_capability.suppression
        >= package.useful_capability.suppression
        && package.chosen_capability.strike >= package.useful_capability.strike
        && package.chosen_bombing >= package.useful_bombing;
    let safety = if !meets_known_opportunity {
        ConnectedExecutionSafety::Speculative
    } else if package.observed_aa_firepower == 0 {
        ConnectedExecutionSafety::Secure
    } else {
        ConnectedExecutionSafety::Managed
    };
    ConnectedOpportunityCase {
        urgency,
        confidence,
        value,
        time_to_impact,
        safety,
    }
}

fn derive_fresh_connected_proposal(
    context: FreshConnectedDerivationContext<'_>,
    target: &BuildingContact,
    origin: ConnectedProposalOrigin,
) -> Result<FreshConnectedProposal, ConnectedPlanRejection> {
    let route = ConnectedRouteContext {
        intel: context.intel,
        home: context.home,
        target: target.anchor,
        public_map: context.coordination.public_map,
        orientation: context.coordination.orientation,
    };
    let initial_resources = ConnectedProductionResources::from_snapshot_after_current_reserve(
        context.obs,
        target,
        context.unavailable,
        route,
        context.resource_snapshot,
        context.coordination.protected_current_scrap,
    );
    derive_connected_proposal_with_resources(
        context,
        target,
        origin,
        initial_resources,
        context
            .obs
            .tick
            .saturating_add(CONNECTED_PREPARATION_HORIZON),
    )
}

fn derive_connected_proposal_with_resources(
    context: FreshConnectedDerivationContext<'_>,
    target: &BuildingContact,
    origin: ConnectedProposalOrigin,
    initial_resources: ConnectedProductionResources,
    preparation_deadline: Tick,
) -> Result<FreshConnectedProposal, ConnectedPlanRejection> {
    let FreshConnectedDerivationContext {
        profile,
        tuning,
        obs,
        resource_snapshot,
        intel,
        home,
        coordination,
        unavailable,
        preferred_artillery,
    } = context;
    let route = ConnectedRouteContext {
        intel,
        home,
        target: target.anchor,
        public_map: coordination.public_map,
        orientation: coordination.orientation,
    };
    let mut plans = connected_plan_options(
        profile,
        obs,
        intel,
        home,
        target,
        unavailable,
        ConnectedPlanningContext {
            orientation: coordination.orientation,
            public_map: coordination.public_map,
            resources: &initial_resources,
            preferred_artillery,
            protected_current_scrap: coordination.protected_current_scrap,
            preparation: PreparationConstraints {
                deadline: preparation_deadline,
                decision_cadence: tuning.cadence,
                protected_forecast_scrap: coordination.protected_forecast_scrap,
            },
        },
    )?;
    let minimum_package = plans[0]
        .connected_package
        .as_ref()
        .expect("connected plan options contain connected packages");
    let resources = ConnectedProductionResources::from_package_snapshot_after_current_reserve(
        obs,
        target.player,
        minimum_package,
        route,
        resource_snapshot,
        coordination.protected_current_scrap,
    );
    let prior_admitted_at = match &origin {
        ConnectedProposalOrigin::Idle { .. } => obs.tick,
        ConnectedProposalOrigin::Remembered { active }
        | ConnectedProposalOrigin::Active { active } => active.plan.admitted_at,
    };
    let route_unavailable =
        connected_provider_unavailable(obs, &resources.targets, unavailable, route);
    let mut variants = Vec::with_capacity(plans.len());
    for mut plan in plans.drain(..) {
        plan.admitted_at = prior_admitted_at;
        if let ConnectedProposalOrigin::Active { active } = &origin {
            plan.connected_scope_anchor = active.plan.connected_scope_anchor;
            plan.issued_connected_providers = active.plan.issued_connected_providers.clone();
        }
        let mut op = connected_proposal_operation(&origin, target, obs.tick);
        let package = plan
            .connected_package
            .as_ref()
            .expect("connected plan options contain connected packages");
        let previous_scout = op.scout;
        let previous_artillery = op.artillery.clone();
        let previous_strike_aircraft = op.strike_aircraft.clone();
        let mut scouts = op.scout.into_iter().collect::<Vec<_>>();
        assign_provider_demands(&mut scouts, &package.recon, obs, &route_unavailable);
        op.scout = scouts.into_iter().next();
        assign_artillery(&mut op.artillery, &plan, obs, &route_unavailable);
        assign_strike_aircraft(&mut op.strike_aircraft, &plan, obs, &route_unavailable);
        invalidate_reassigned_member_orders(
            &mut op,
            previous_scout,
            &previous_artillery,
            &previous_strike_aircraft,
        );
        let active = ActiveAirOperation { op, plan };
        let claims = connected_proposal_claims(&active, &resources, obs);
        variants.push(ConnectedProposalVariant { active, claims });
    }
    let minimum_claims = variants[0].claims.clone();
    let marginal = variants
        .iter()
        .enumerate()
        .skip(1)
        .map(|(variant_index, variant)| ConnectedMarginalVariant {
            variant_index,
            additions: claim_additions(&minimum_claims, &variant.claims)
                .expect("marginal package variants only add to their exact minimum"),
        })
        .collect();
    let case =
        connected_opportunity_case(obs.tick, intel, target, &resources.targets, &variants[0]);
    Ok(FreshConnectedProposal {
        origin,
        target: target.clone(),
        targets: resources.targets.clone(),
        variants,
        marginal,
        selected_variant: 0,
        protected_current_scrap: coordination.protected_current_scrap,
        protected_forecast_scrap: coordination.protected_forecast_scrap,
        case,
    })
}

fn invalidate_reassigned_member_orders(
    op: &mut AirOperation,
    previous_scout: Option<UnitId>,
    previous_artillery: &[UnitId],
    previous_strike_aircraft: &[UnitId],
) {
    if op.scout != previous_scout {
        op.scout_dispatch = None;
    }
    if op.artillery != previous_artillery {
        op.artillery_staging = None;
    }
    if op.strike_aircraft != previous_strike_aircraft {
        op.strike_hold = None;
    }
}

fn connected_proposal_operation(
    origin: &ConnectedProposalOrigin,
    target: &BuildingContact,
    admitted_at: Tick,
) -> AirOperation {
    match origin {
        ConnectedProposalOrigin::Idle { standby, .. } => AirOperation {
            target_player: target.player,
            target_kind: target.kind,
            target: target.anchor,
            target_id: target.id,
            assault_admitted: true,
            phase: AirOperationPhase::Recon,
            started_at: admitted_at,
            phase_started_at: admitted_at,
            scout: standby.scout,
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery: standby.artillery.clone(),
            strike_aircraft: standby.strike_aircraft.clone(),
            strike_issued_at: None,
            membership_frozen_at: None,
            recovery_reason: None,
        },
        ConnectedProposalOrigin::Remembered { active } => {
            let mut op = active.op.clone();
            op.target_player = target.player;
            op.target_kind = target.kind;
            op.target = target.anchor;
            op.target_id = target.id;
            op.assault_admitted = true;
            op.started_at = admitted_at;
            op.phase_started_at = admitted_at;
            op
        }
        ConnectedProposalOrigin::Active { active } => {
            let mut op = active.op.clone();
            op.target_player = target.player;
            op.target_kind = target.kind;
            op.target = target.anchor;
            op.target_id = target.id;
            op
        }
    }
}

/// Fog-honest evidence for the connected force package's current revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectedPackageDiagnostics {
    pub(super) admitted_at: Tick,
    pub(super) derived_at: Tick,
    pub(super) preparation_deadline: Tick,
    pub(super) target_anchors: Vec<TilePos>,
    pub(super) target_value: u64,
    pub(super) current_scrap: u32,
    pub(super) forecast_scrap: u32,
    pub(super) minimum_capability: [u64; 3],
    pub(super) useful_capability: [u64; 3],
    pub(super) chosen_capability: [u64; 3],
    pub(super) useful_bombing: u64,
    pub(super) chosen_bombing: u64,
    pub(super) recon: Vec<(UnitKind, usize)>,
    pub(super) suppression: Vec<(UnitKind, usize)>,
    pub(super) strike: Vec<(UnitKind, usize)>,
    pub(super) observed_aa_firepower: u64,
    pub(super) suppressible_aa_firepower: u64,
}

/// Controller-local owner of the active operation and its cooldown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StrategicPlanner {
    air: Option<ActiveAirOperation>,
    standby: AirStandby,
    cooldown_until: Tick,
    terminal_outcome: Option<AirOperationOutcome>,
}

impl StrategicPlanner {
    /// Creates an idle planner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Active operation for replay diagnostics.
    pub fn air_operation(&self) -> Option<&AirOperation> {
        self.air.as_ref().map(|active| &active.op)
    }

    /// Immutable admission tick for resource-priority comparisons. The public
    /// operation's timeout clock may restart when reconnaissance becomes an
    /// assault, but its place in the commitment order does not.
    pub(super) fn air_admitted_at(&self) -> Option<Tick> {
        self.air.as_ref().map(|active| active.plan.admitted_at)
    }

    /// Connected-package evidence for opt-in decision traces.
    pub(super) fn connected_package_diagnostics(&self) -> Option<ConnectedPackageDiagnostics> {
        let active = self.air.as_ref()?;
        let package = active.plan.connected_package.as_ref()?;
        Some(ConnectedPackageDiagnostics {
            admitted_at: active.plan.admitted_at,
            derived_at: package.derived_at,
            preparation_deadline: package.preparation_deadline,
            target_anchors: package.target_anchors.clone(),
            target_value: package.target_value,
            current_scrap: package.current_scrap,
            forecast_scrap: package.forecast_scrap,
            minimum_capability: capability_components(package.minimum_capability),
            useful_capability: capability_components(package.useful_capability),
            chosen_capability: capability_components(package.chosen_capability),
            useful_bombing: package.useful_bombing,
            chosen_bombing: package.chosen_bombing,
            recon: demand_components(&package.recon),
            suppression: demand_components(&package.suppression),
            strike: demand_components(&package.strike),
            observed_aa_firepower: package.observed_aa_firepower,
            suppressible_aa_firepower: package.suppressible_aa_firepower,
        })
    }

    #[cfg(test)]
    fn air_plan(&self) -> Option<&AirPlan> {
        self.air.as_ref().map(|active| &active.plan)
    }

    #[cfg(test)]
    pub(super) fn air_assembly_timeout(&self) -> Option<Tick> {
        self.air.as_ref().map(|active| active.plan.assembly_timeout)
    }

    #[cfg(test)]
    fn air_op_mut(&mut self) -> Option<&mut AirOperation> {
        self.air.as_mut().map(|active| &mut active.op)
    }

    #[cfg(test)]
    fn air_plan_mut(&mut self) -> Option<&mut AirPlan> {
        self.air.as_mut().map(|active| &mut active.plan)
    }

    /// Earliest tick at which another operation may start.
    pub fn cooldown_until(&self) -> Tick {
        self.cooldown_until
    }

    pub(super) fn terminal_outcome(&self) -> Option<AirOperationOutcome> {
        self.terminal_outcome
    }

    pub(in crate::bot) fn has_active_island_operation(&self) -> bool {
        self.air.as_ref().is_some_and(|active| {
            active.op.assault_admitted && active.plan.connected_package.is_none()
        })
    }

    /// Advances one already-admitted island assault without admitting a new
    /// operation. Returning `None` leaves every other strategic lifecycle on
    /// the normal post-allocation path.
    pub(in crate::bot) fn stage_active_island(
        &mut self,
        context: StrategicThinkContext<'_>,
    ) -> Option<StrategicThinkResult> {
        let active = self.air.as_ref()?;
        if !active.op.assault_admitted || active.plan.connected_package.is_some() {
            return None;
        }
        Some(self.think_after_connected_adjudication(context))
    }

    /// Proposes one exact common-minimum connected assault without mutating
    /// planner state. Island admission and an already-admitted assault remain
    /// on the ordinary lifecycle path.
    pub(in crate::bot) fn fresh_connected_minimum_proposal(
        &self,
        request: FreshConnectedProposalRequest<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let FreshConnectedProposalRequest {
            profile,
            tuning,
            obs,
            resource_snapshot,
            intel,
            home,
            coordination,
        } = request;
        if intel.observed_at() != Some(obs.tick)
            || !coordination.allow_new_operation
            || !strategic_admission_tick(obs.tick)
            || obs.tick < self.cooldown_until
            || coordination.lift_support.is_some()
        {
            return Ok(None);
        }

        if let Some(active) = &self.air {
            if active.op.assault_admitted {
                return Ok(None);
            }
            let mut refreshed = active.clone();
            refresh_target(&mut refreshed.op, intel);
            let Some(target) = current_target_contact(&refreshed.op, intel) else {
                return Ok(None);
            };
            if wealthy_island_target(profile, obs, home, target, coordination.public_map) {
                return Ok(None);
            }
            let owned = reservations(&refreshed.op, &refreshed.plan, obs);
            let unavailable = excluding_owned(coordination.enlisted, &owned);
            return derive_fresh_connected_proposal(
                FreshConnectedDerivationContext {
                    profile,
                    tuning,
                    obs,
                    resource_snapshot,
                    intel,
                    home,
                    coordination,
                    unavailable: &unavailable,
                    preferred_artillery: &refreshed.op.artillery,
                },
                target,
                ConnectedProposalOrigin::Remembered {
                    active: active.clone(),
                },
            )
            .map(Some)
            .map_err(|reason| RejectedConnectedCandidate {
                target: target.clone(),
                reason,
            });
        }

        if select_wealthy_island_target(profile, obs, home, intel, coordination.public_map)
            .is_some()
        {
            return Ok(None);
        }
        let current = select_target_candidates(intel, obs.tick, tuning.tactical_memory)
            .into_iter()
            .filter(|target| target.evidence == ContactEvidence::Current)
            .collect::<Vec<_>>();
        let Some(first) = current.first().copied() else {
            return Ok(None);
        };
        let combat_roster = combat_roster(obs);
        if combat_roster < CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER {
            return Err(RejectedConnectedCandidate {
                target: first.clone(),
                reason: ConnectedPlanRejection::InsufficientStandingForce {
                    current: combat_roster,
                    required: CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER,
                },
            });
        }

        let mut standby = self.standby.clone();
        standby.prune(obs);
        let unavailable = excluding_owned(coordination.enlisted, &standby.reservations());
        let origin = ConnectedProposalOrigin::Idle {
            air: self.air.clone(),
            standby: self.standby.clone(),
        };
        let mut first_rejection = None;
        for target in current {
            match derive_fresh_connected_proposal(
                FreshConnectedDerivationContext {
                    profile,
                    tuning,
                    obs,
                    resource_snapshot,
                    intel,
                    home,
                    coordination,
                    unavailable: &unavailable,
                    preferred_artillery: &standby.artillery,
                },
                target,
                origin.clone(),
            ) {
                Ok(proposal) => return Ok(Some(proposal)),
                Err(reason) if first_rejection.is_none() => {
                    first_rejection = Some(RejectedConnectedCandidate {
                        target: target.clone(),
                        reason,
                    });
                }
                Err(_) => {}
            }
        }
        Err(first_rejection.expect("at least one current target was considered"))
    }

    /// Re-derives one admitted connected operation from current evidence while
    /// its membership remains revisable. The fixed preparation deadline and
    /// original target-cluster scope are retained.
    pub(in crate::bot) fn active_connected_revision_proposal(
        &self,
        request: FreshConnectedProposalRequest<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let FreshConnectedProposalRequest {
            profile,
            tuning,
            obs,
            resource_snapshot,
            intel,
            home,
            coordination,
        } = request;
        if intel.observed_at() != Some(obs.tick) {
            return Ok(None);
        }
        let Some(active) = self.air.as_ref() else {
            return Ok(None);
        };
        let Some(package) = active.plan.connected_package.as_ref() else {
            return Ok(None);
        };
        if !active.op.assault_admitted
            || active.op.phase > AirOperationPhase::Assemble
            || active.op.membership_frozen_at.is_some()
            || package.derived_at >= obs.tick
            || obs.tick > package.preparation_deadline
        {
            return Ok(None);
        }
        let Some(target) = current_package_revision_target(&active.op, &active.plan, intel) else {
            return Ok(None);
        };
        let owned = reservations(&active.op, &active.plan, obs);
        let unavailable = excluding_owned(coordination.enlisted, &owned);
        let route = ConnectedRouteContext {
            intel,
            home,
            target: target.anchor,
            public_map: coordination.public_map,
            orientation: coordination.orientation,
        };
        let scope_anchor = active
            .plan
            .connected_scope_anchor
            .expect("an admitted connected package retains its original scope");
        let initial_resources = if target.anchor == scope_anchor {
            ConnectedProductionResources::from_snapshot_after_current_reserve(
                obs,
                target,
                &unavailable,
                route,
                resource_snapshot,
                coordination.protected_current_scrap,
            )
        } else {
            ConnectedProductionResources::from_revision_snapshot_after_current_reserve(
                obs,
                target,
                package,
                route,
                resource_snapshot,
                coordination.protected_current_scrap,
            )
        };
        let mut proposal = derive_connected_proposal_with_resources(
            FreshConnectedDerivationContext {
                profile,
                tuning,
                obs,
                resource_snapshot,
                intel,
                home,
                coordination,
                unavailable: &unavailable,
                preferred_artillery: &active.op.artillery,
            },
            target,
            ConnectedProposalOrigin::Active {
                active: active.clone(),
            },
            initial_resources,
            package.preparation_deadline,
        )
        .map_err(|reason| RejectedConnectedCandidate {
            target: target.clone(),
            reason,
        })?;
        proposal.retain_active_revision_provider_floor();
        Ok(Some(proposal))
    }

    /// Moves an admitted connected operation into bounded recovery after its
    /// current revision can no longer field the shared minimum.
    pub(in crate::bot) fn reject_active_connected_revision(
        &mut self,
        rejection: ConnectedPlanRejection,
        observed_at: Tick,
    ) {
        let active = self
            .air
            .as_mut()
            .expect("a rejected active revision belongs to an admitted operation");
        recover(
            &mut active.op,
            recovery_for_rejection(rejection),
            observed_at,
        );
    }

    /// Installs the exact proposal selected by cross-domain adjudication. No
    /// observation is accepted here, so commitment cannot rerank its target,
    /// rebuild its package, or change its producer basis.
    pub(in crate::bot) fn commit_connected_proposal(
        &mut self,
        proposal: FreshConnectedProposal,
    ) -> Result<(), ConnectedProposalCommitError> {
        let revises_active = proposal.revises_active_operation();
        if !revises_active
            && self
                .air
                .as_ref()
                .is_some_and(|active| active.op.assault_admitted)
        {
            return Err(ConnectedProposalCommitError::ExistingAssault);
        }
        let origin_matches = match &proposal.origin {
            ConnectedProposalOrigin::Idle { air, standby } => {
                &self.air == air && &self.standby == standby
            }
            ConnectedProposalOrigin::Remembered { active } => self.air.as_ref() == Some(active),
            ConnectedProposalOrigin::Active { active } => self.air.as_ref() == Some(active),
        };
        if !origin_matches {
            return Err(ConnectedProposalCommitError::StalePlanner);
        }
        let selected = proposal
            .variants
            .into_iter()
            .nth(proposal.selected_variant)
            .expect("a selected proposal variant came from its retained ladder");
        if selected
            .active
            .plan
            .connected_package
            .as_ref()
            .is_some_and(|package| package.producer_assignments.is_none())
        {
            return Err(ConnectedProposalCommitError::UnboundProducerSchedule);
        }
        self.air = Some(selected.active);
        if !revises_active {
            self.standby = AirStandby::default();
        }
        self.terminal_outcome = None;
        Ok(())
    }

    /// Exact live and unpaid claims owned by an admitted connected operation.
    /// The producer jobs retain the allocator-selected lane and deadline. A job
    /// remains unpaid through its command tick and leaves the next pass only
    /// after that exact accepted command boundary.
    pub(in crate::bot) fn active_connected_obligation(
        &self,
        obs: &Observation,
    ) -> Option<ActiveConnectedObligation> {
        let active = self.air.as_ref()?;
        if !active.op.assault_admitted {
            return None;
        }
        let package = active.plan.connected_package.as_ref()?;
        package.producer_assignments.as_ref()?;
        let objective = active.op.target_id?;
        let provider_jobs = if active.op.phase <= AirOperationPhase::Assemble {
            remaining_bound_provider_assignments(&active.plan)
        } else {
            Vec::new()
        };
        Some(ActiveConnectedObligation {
            identity: ConnectedOffenseIdentity::new(objective, active.op.target),
            accepted_at: active.plan.admitted_at,
            deadline: package.preparation_deadline,
            units: reservations(&active.op, &active.plan, obs),
            provider_jobs,
        })
    }

    /// Cancels a connected purchase that became due during emergency economy
    /// recovery and returns every still-routable member immediately.
    pub(in crate::bot) fn recover_due_connected_for_economy_emergency(
        &mut self,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &Observation,
        home: TilePos,
        public_map: Option<&PublicMapBriefing>,
        orientation: Orientation,
    ) -> Option<StrategicDecision> {
        let has_due_provider = self.air.as_ref().is_some_and(|active| {
            active.op.phase <= AirOperationPhase::Assemble
                && active.plan.connected_package.is_some()
                && remaining_bound_provider_assignments(&active.plan)
                    .iter()
                    .any(|assignment| assignment.timing.enqueued_at <= obs.tick)
        });
        if !has_due_provider {
            return None;
        }

        let ActiveAirOperation { mut op, mut plan } = self
            .air
            .take()
            .expect("a due connected provider belongs to one active operation");
        recover(&mut op, AirRecoveryReason::PreparationInfeasible, obs.tick);
        self.cooldown_until = obs.tick.saturating_add(cooldown(profile, tuning));
        let mut out = StrategicDecision::default();
        reconcile_recovery_return(
            &mut op,
            &mut plan,
            RecoveryReturnContext {
                obs,
                home,
                public_map,
                orientation,
                issue_order: true,
            },
            &mut out,
        );
        out.reservations = reservations(&op, &plan, obs);
        if out.reservations.is_empty() {
            self.terminal_outcome = Some(air_operation_outcome(&op));
        } else {
            self.air = Some(ActiveAirOperation { op, plan });
        }
        Some(out)
    }

    /// Enters recovery when shared allocation can no longer retain an active
    /// connected schedule. The Brain's post-allocation strategy pass observes
    /// this same tick and owns the one return-home order.
    pub(in crate::bot) fn recover_unfundable_active_connected(&mut self, observed_at: Tick) {
        let active = self
            .air
            .as_mut()
            .expect("an active connected obligation can only come from its planner");
        debug_assert!(active.op.assault_admitted);
        debug_assert!(active.plan.connected_package.is_some());
        recover(
            &mut active.op,
            AirRecoveryReason::PreparationInfeasible,
            observed_at,
        );
    }

    /// Records provider commands that the allocator committed on this tick.
    /// Their exact `TrainAt` intents are lowered from the shared producer
    /// schedule, so the domain planner must neither emit nor retry them.
    pub(in crate::bot) fn mark_current_connected_providers_issued(&mut self, observed_at: Tick) {
        let Some(active) = self.air.as_mut() else {
            return;
        };
        let Some(assignments) = active
            .plan
            .connected_package
            .as_ref()
            .and_then(|package| package.producer_assignments.as_ref())
        else {
            return;
        };
        active
            .plan
            .issued_connected_providers
            .extend(assignments.iter().filter_map(|assignment| {
                (assignment.timing.enqueued_at == observed_at).then_some(assignment.request_ordinal)
            }));
        active.plan.issued_connected_providers.sort_unstable();
        active.plan.issued_connected_providers.dedup();
    }

    /// Refreshes only the observation-relative funding split of an active
    /// operation's still-unpaid jobs. The accepted job identity, producer,
    /// timing, and deadline remain immutable.
    pub(in crate::bot) fn refresh_active_connected_funding(
        &mut self,
        obligation: &ActiveConnectedObligation,
        assignments: &[ConnectedProducerAssignment],
    ) -> Result<(), ConnectedProducerBindingError> {
        let Some(active) = self.air.as_ref() else {
            return Err(ConnectedProducerBindingError::StaleActiveOperation);
        };
        let Some(package) = active.plan.connected_package.as_ref() else {
            return Err(ConnectedProducerBindingError::StaleActiveOperation);
        };
        let Some(bound) = package.producer_assignments.as_ref() else {
            return Err(ConnectedProducerBindingError::StaleActiveOperation);
        };
        if !active.op.assault_admitted
            || active.op.target_id != Some(obligation.identity.objective)
            || active.op.target != obligation.identity.anchor
            || active.plan.admitted_at != obligation.accepted_at
            || package.preparation_deadline != obligation.deadline
        {
            return Err(ConnectedProducerBindingError::StaleActiveOperation);
        }
        if assignments.len() != obligation.provider_jobs.len() {
            return Err(ConnectedProducerBindingError::JobCount {
                expected: obligation.provider_jobs.len(),
                actual: assignments.len(),
            });
        }
        for (expected, assignment) in obligation.provider_jobs.iter().zip(assignments) {
            let request_ordinal = expected.request_ordinal;
            if bound
                .iter()
                .find(|candidate| candidate.request_ordinal == request_ordinal)
                != Some(expected)
            {
                return Err(ConnectedProducerBindingError::StaleActiveOperation);
            }
            if assignment.owner != expected.owner {
                return Err(ConnectedProducerBindingError::Owner { request_ordinal });
            }
            if assignment.request_ordinal != request_ordinal {
                return Err(ConnectedProducerBindingError::RequestOrdinal {
                    expected: request_ordinal,
                    actual: assignment.request_ordinal,
                });
            }
            if assignment.kind != expected.kind {
                return Err(ConnectedProducerBindingError::Kind { request_ordinal });
            }
            if assignment.producer != expected.producer {
                return Err(ConnectedProducerBindingError::Producer { request_ordinal });
            }
            if assignment.timing != expected.timing {
                return Err(ConnectedProducerBindingError::Timing { request_ordinal });
            }
            if assignment
                .funding
                .current_scrap
                .checked_add(assignment.funding.forecast_scrap)
                != Some(assignment.kind.stats().cost)
            {
                return Err(ConnectedProducerBindingError::Funding { request_ordinal });
            }
        }

        let bound = self
            .air
            .as_mut()
            .and_then(|active| active.plan.connected_package.as_mut())
            .and_then(|package| package.producer_assignments.as_mut())
            .expect("the validated active operation still owns its exact schedule");
        for assignment in assignments {
            bound
                .iter_mut()
                .find(|candidate| candidate.request_ordinal == assignment.request_ordinal)
                .expect("the validated assignment ordinal remains bound")
                .funding = assignment.funding;
        }
        Ok(())
    }

    /// Airworks training time still required by the current operation roster.
    /// Queued units remain factory work and therefore still contribute to the
    /// capacity signal; only completed members reduce it.
    pub(super) fn remaining_airwork_ticks(&self, obs: &Observation) -> Tick {
        let Some(ActiveAirOperation { op, plan }) = &self.air else {
            return 0;
        };
        if op.phase > AirOperationPhase::Assemble {
            return 0;
        }
        if let Some(package) = &plan.connected_package {
            return package
                .recon
                .iter()
                .chain(&package.strike)
                .filter(|demand| demand.kind.stats().domain == crate::stats::Domain::Air)
                .map(|demand| {
                    let live = op
                        .scout
                        .into_iter()
                        .chain(op.strike_aircraft.iter().copied())
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
            op.scout
                .is_some_and(|id| unit(obs, id).is_some_and(|member| member.kind == scout_kind)),
        );
        let live_screen = plan
            .screen
            .iter()
            .filter(|id| unit(obs, **id).is_some_and(|member| member.kind == screen_kind))
            .count();
        let live_strike_aircraft = op
            .strike_aircraft
            .iter()
            .filter(|id| unit(obs, **id).is_some_and(|member| member.kind == bomber_kind))
            .count();
        let missing_scout = 1usize.saturating_sub(live_scout);
        if !op.assault_admitted {
            return remaining_training_ticks(obs, missing_scout, scout_kind);
        }
        let missing_screen = plan.desired_screen.saturating_sub(live_screen);
        let missing_strike_aircraft = plan
            .desired_strike_aircraft
            .saturating_sub(live_strike_aircraft);
        remaining_training_ticks(obs, missing_scout, scout_kind)
            .saturating_add(remaining_training_ticks(obs, missing_screen, screen_kind))
            .saturating_add(remaining_training_ticks(
                obs,
                missing_strike_aircraft,
                bomber_kind,
            ))
    }

    /// Runs the ordinary tactical lifecycle after the coordinator has already
    /// accepted or rejected the fresh connected-offense proposal for this
    /// observation. Island and remembered reconnaissance behavior is unchanged.
    pub(in crate::bot) fn think_after_connected_adjudication(
        &mut self,
        context: StrategicThinkContext<'_>,
    ) -> StrategicThinkResult {
        self.think_after_connected_adjudication_inner(context)
    }

    fn think_after_connected_adjudication_inner(
        &mut self,
        context: StrategicThinkContext<'_>,
    ) -> StrategicThinkResult {
        let StrategicThinkContext {
            profile,
            tuning,
            obs,
            intel,
            home,
            coordination,
            production,
        } = context;
        let StrategicCoordination {
            enlisted,
            lift_support,
            allow_new_operation,
            protected_current_scrap,
            protected_forecast_scrap,
            public_map,
            orientation,
        } = coordination;
        self.terminal_outcome = None;
        self.standby.prune(obs);
        if intel.observed_at() != Some(obs.tick) {
            return StrategicThinkResult::from_decision(StrategicDecision {
                reservations: self.standby.reservations(),
                ..StrategicDecision::default()
            });
        }
        let mut connected_resources = None;
        if self.air.is_none() {
            if !allow_new_operation {
                return StrategicThinkResult::from_decision(StrategicDecision {
                    reservations: self.standby.reservations(),
                    ..StrategicDecision::default()
                });
            }
            if obs.tick < self.cooldown_until {
                return StrategicThinkResult::from_decision(StrategicDecision {
                    reservations: self.standby.reservations(),
                    ..StrategicDecision::default()
                });
            }
            let island_target = if let Some(request) = lift_support {
                exact_wealthy_island_target(profile, obs, home, intel, request, public_map)
            } else {
                select_wealthy_island_target(profile, obs, home, intel, public_map)
            };
            let selected = if let Some(target) = island_target {
                Some((
                    target,
                    AirPlan::island_with_production(profile, obs, production),
                ))
            } else if lift_support.is_some() {
                None
            } else {
                let candidates = select_target_candidates(intel, obs.tick, tuning.tactical_memory);
                let current: Vec<_> = candidates
                    .iter()
                    .copied()
                    .filter(|target| target.evidence == ContactEvidence::Current)
                    .collect();
                if !current.is_empty() {
                    None
                } else if combat_roster(obs) >= CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER
                    && ready_to_reconnoiter(obs)
                {
                    candidates
                        .first()
                        .copied()
                        .map(|target| (target, AirPlan::remembered_connected(obs)))
                } else {
                    None
                }
            };
            let Some((target, plan)) = selected else {
                self.standby = AirStandby::default();
                return StrategicThinkResult::default();
            };
            if !strategic_admission_tick(obs.tick) {
                return StrategicThinkResult::from_decision(StrategicDecision {
                    reservations: self.standby.reservations(),
                    ..StrategicDecision::default()
                });
            }
            let standby = core::mem::take(&mut self.standby);
            let assault_admitted = target.evidence == ContactEvidence::Current;
            let op = AirOperation {
                target_player: target.player,
                target_kind: target.kind,
                target: target.anchor,
                target_id: target.id,
                assault_admitted,
                phase: AirOperationPhase::Recon,
                started_at: obs.tick,
                phase_started_at: obs.tick,
                scout: standby.scout,
                scout_dispatch: None,
                strike_hold: None,
                artillery_staging: None,
                artillery: if assault_admitted {
                    standby.artillery
                } else {
                    Vec::new()
                },
                strike_aircraft: if assault_admitted {
                    standby.strike_aircraft
                } else {
                    Vec::new()
                },
                strike_issued_at: None,
                membership_frozen_at: None,
                recovery_reason: None,
            };
            self.air = Some(ActiveAirOperation { op, plan });
        }
        let Some(ActiveAirOperation { mut op, mut plan }) = self.air.take() else {
            return StrategicThinkResult::default();
        };
        plan.screen.retain(|id| {
            unit(obs, *id)
                .is_some_and(|member| member.kind == Role::AirGround.unit_for(obs.faction))
        });
        let began_in_recovery = op.phase == AirOperationPhase::Recover;
        refresh_target(&mut op, intel);
        if !op.assault_admitted
            && strategic_admission_tick(obs.tick)
            && let Some(current_target) = current_target_contact(&op, intel)
        {
            let admitted_at = plan.admitted_at;
            if wealthy_island_target(profile, obs, home, current_target, public_map) {
                let mut admitted = AirPlan::island_with_production(profile, obs, production);
                op.assault_admitted = true;
                op.started_at = obs.tick;
                op.phase_started_at = obs.tick;
                admitted.admitted_at = admitted_at;
                plan = admitted;
            }
        }
        if op.assault_admitted
            && op.phase <= AirOperationPhase::Assemble
            && let Some(deadline) = plan
                .connected_package
                .as_ref()
                .filter(|package| package.derived_at < obs.tick)
                .map(|package| package.preparation_deadline)
            && obs.tick <= deadline
            && let Some(current_target) = current_package_revision_target(&op, &plan, intel)
        {
            let connected_package = plan
                .connected_package
                .as_ref()
                .expect("connected revision retains its admitted package")
                .clone();
            let resources = connected_resources.get_or_insert_with(|| {
                ConnectedProductionResources::from_revision_after_current_reserve(
                    obs,
                    current_target,
                    &connected_package,
                    ConnectedRouteContext {
                        intel,
                        home,
                        target: current_target.anchor,
                        public_map,
                        orientation,
                    },
                    protected_current_scrap,
                )
            });
            rebase_adjudicated_connected_target(
                &mut op,
                &mut plan,
                current_target,
                &resources.targets.target_anchors,
            );
        }
        if !began_in_recovery && op.phase != AirOperationPhase::Recover {
            abort_if_needed(&mut op, &plan, profile, obs, intel);
        }

        let mut out = StrategicDecision::default();
        let landing_sites: Vec<_> = lift_support
            .filter(|request| request.player == op.target_player && request.target == op.target)
            .map_or_else(Vec::new, |request| request.planned_drops.clone());
        let external_enlisted = excluding_owned(enlisted, &reservations(&op, &plan, obs));
        if let Some(package) = plan.connected_package.as_ref()
            && op.phase <= AirOperationPhase::Assemble
        {
            let route = ConnectedRouteContext {
                intel,
                home,
                target: op.target,
                public_map,
                orientation,
            };
            connected_resources.get_or_insert_with(|| {
                ConnectedProductionResources::from_package_after_current_reserve(
                    obs,
                    op.target_player,
                    package,
                    route,
                    protected_current_scrap,
                )
            });
        }
        let context = AirPlanningContext {
            profile,
            tuning,
            obs,
            intel,
            home,
            orientation,
            public_map,
            enlisted: &external_enlisted,
            landing_sites: &landing_sites,
            connected_resources,
            production,
            protected_current_scrap,
            protected_forecast_scrap,
        };
        match op.phase {
            AirOperationPhase::Recon if !op.assault_admitted => {
                remembered_recon(&mut op, &plan, &context, &mut out)
            }
            AirOperationPhase::Recon => recon(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::Assemble => assemble(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::SuppressAa => suppress(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::Verify => verify(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::Strike => strike(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::Recover => {}
        }
        let preparation_expired = plan.connected_package.as_ref().is_some_and(|package| {
            op.phase <= AirOperationPhase::Assemble
                && obs.tick >= package.preparation_deadline
                && !assembly_complete(&op, &plan)
        });
        if preparation_expired {
            out.intents.clear();
            out.committed_scrap = 0;
            recover(&mut op, AirRecoveryReason::Timeout, obs.tick);
        }
        if !allow_new_operation {
            out.intents
                .retain(|intent| !matches!(intent, Intent::TrainAt { .. }));
            out.committed_scrap = 0;
        }
        let recovery_entered_this_tick = op.phase == AirOperationPhase::Recover
            && (!began_in_recovery || op.phase_started_at == obs.tick);
        if op.phase == AirOperationPhase::Recover {
            if recovery_entered_this_tick {
                self.cooldown_until = obs.tick.saturating_add(cooldown(profile, tuning));
            }
            reconcile_recovery_return(
                &mut op,
                &mut plan,
                RecoveryReturnContext {
                    obs,
                    home,
                    public_map,
                    orientation,
                    issue_order: recovery_entered_this_tick,
                },
                &mut out,
            );
        }
        out.reservations = reservations(&op, &plan, obs);
        // Lowering fans a mixed-domain group over distinct snapped goals, and
        // bounded-turn aircraft may stop within their movement acceptance
        // radius. Once a previously dispatched return has become terminal for
        // every survivor, `idle` is the simulation's authoritative completion
        // signal; comparing every tile to the original shared goal invents a
        // stricter geometry and holds the operation forever.
        let settled = began_in_recovery
            && !recovery_entered_this_tick
            && out
                .reservations
                .iter()
                .all(|id| unit(obs, *id).is_some_and(|member| member.idle));
        let recovered = op.phase == AirOperationPhase::Recover
            && (out.reservations.is_empty()
                || settled
                || elapsed(op.phase_started_at, obs.tick) >= 500);
        if settled && !out.reservations.is_empty() && reusable_survivors(op.recovery_reason) {
            self.standby = AirStandby::from_operation(&op, obs);
        } else if recovered {
            self.terminal_outcome = Some(air_operation_outcome(&op));
        } else {
            self.air = Some(ActiveAirOperation { op, plan });
        }
        StrategicThinkResult {
            decision: out,
            rejected_connected_candidate: None,
        }
    }
}

struct RecoveryReturnContext<'a> {
    obs: &'a Observation,
    home: TilePos,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
    issue_order: bool,
}

fn reconcile_recovery_return(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: RecoveryReturnContext<'_>,
    out: &mut StrategicDecision,
) {
    let RecoveryReturnContext {
        obs,
        home,
        public_map,
        orientation,
        issue_order,
    } = context;
    let survivors = reservations(op, plan, obs);
    let returning = if plan.connected_package.is_some() {
        connected_public_map(plan, public_map).map_or_else(
            || {
                routing::routable_command_subset_with_orientation(
                    obs,
                    &survivors,
                    home,
                    orientation,
                )
            },
            |map| {
                routing::routable_command_subset_with_public_terrain_and_orientation(
                    obs,
                    map,
                    &survivors,
                    home,
                    orientation,
                )
            },
        )
    } else {
        routing::routable_command_subset(obs, &survivors, home)
    };
    release_unroutable(op, plan, &survivors, &returning);
    if issue_order && !returning.is_empty() {
        out.intents.push(Intent::MoveUnits {
            units: returning,
            goal: home,
        });
    }
}

fn capability_components(capability: NormalizedCapability) -> [u64; 3] {
    [capability.recon, capability.suppression, capability.strike]
}

fn demand_components(demands: &[ProviderDemand]) -> Vec<(UnitKind, usize)> {
    demands
        .iter()
        .map(|demand| (demand.kind, demand.count))
        .collect()
}

fn air_operation_outcome(op: &AirOperation) -> AirOperationOutcome {
    if op.recovery_reason == Some(AirRecoveryReason::Complete) {
        AirOperationOutcome::Released {
            player: op.target_player,
            target: op.target,
        }
    } else {
        AirOperationOutcome::Aborted {
            player: op.target_player,
            target: op.target,
        }
    }
}

fn remembered_recon(
    op: &mut AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) {
    let obs = context.obs;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let previous_scout = op.scout;
    op.scout = op
        .scout
        .filter(|id| unit(obs, *id).is_some())
        .or_else(|| available(obs, context.enlisted, |kind| kind == scout_kind).next());
    if op.scout != previous_scout {
        op.scout_dispatch = None;
        if op.scout.is_some() {
            op.phase_started_at = obs.tick;
        }
    }
    if !dispatch_scout(
        op,
        plan,
        obs,
        context.intel,
        context.landing_sites,
        connected_public_map(plan, context.public_map),
        out,
    ) {
        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        return;
    }
    schedule(
        context,
        &[(scout_kind, usize::from(op.scout.is_none()))],
        out,
    );
}

fn recon(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) {
    let AirPlanningContext {
        tuning,
        obs,
        intel,
        enlisted,
        landing_sites,
        ..
    } = context;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let public_map = connected_public_map(plan, context.public_map);
    let route_unavailable = if let Some(resources) = context.connected_resources.as_ref() {
        connected_provider_unavailable(
            obs,
            &resources.targets,
            &[],
            ConnectedRouteContext {
                intel,
                home: context.home,
                target: op.target,
                public_map: context.public_map,
                orientation: context.orientation,
            },
        )
    } else {
        Vec::new()
    };
    let unavailable = merged_unavailable(enlisted, &route_unavailable);
    let previous_scout = op.scout;
    let previous_artillery = op.artillery.clone();
    let previous_strike_aircraft = op.strike_aircraft.clone();
    op.scout = op
        .scout
        .filter(|id| {
            unit(obs, *id).is_some_and(|member| member.kind == scout_kind)
                && !unavailable.contains(id)
        })
        .or_else(|| available(obs, &unavailable, |k| k == scout_kind).next());
    if op.scout != previous_scout {
        op.scout_dispatch = None;
        if op.scout.is_some() {
            op.phase_started_at = obs.tick;
        }
    }
    assign_artillery(&mut op.artillery, plan, obs, &unavailable);
    assign_strike_aircraft(&mut op.strike_aircraft, plan, obs, &unavailable);
    if previous_strike_aircraft != op.strike_aircraft {
        op.strike_hold = None;
    }
    if previous_scout
        .is_some_and(|id| unit(obs, id).is_some() && route_unavailable.binary_search(&id).is_ok())
        && op.scout.is_none()
        || previous_strike_aircraft
            .iter()
            .any(|id| unit(obs, *id).is_some() && route_unavailable.binary_search(id).is_ok())
            && op.strike_aircraft.len() < plan.desired_strike_aircraft
    {
        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        return;
    }
    if previous_artillery
        .iter()
        .any(|id| unit(obs, *id).is_some() && route_unavailable.binary_search(id).is_ok())
        && op.artillery.len() < plan.desired_artillery
    {
        recover(op, AirRecoveryReason::UnreachableStaging, obs.tick);
        return;
    }
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    assign_exact(
        &mut plan.screen,
        plan.desired_screen,
        obs,
        enlisted,
        |kind| kind == screen_kind,
    );
    if !connected_package_is_feasible(op, plan, context) {
        recover(op, AirRecoveryReason::PreparationInfeasible, obs.tick);
        return;
    }
    if !dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out) {
        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        return;
    }
    schedule_missing_members(op, plan, context, scout_kind, out);
    if plan.connected_package.is_some() {
        hold_strike_aircraft(op, obs, context.home, out);
    }
    if op.scout_dispatch.is_some()
        && target_seen(op, plan, obs)
        && elapsed(op.phase_started_at, obs.tick) >= tuning.reaction_delay
    {
        enter(op, AirOperationPhase::Assemble, obs.tick);
    }
}

fn assemble(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) {
    let AirPlanningContext {
        obs,
        intel,
        home,
        enlisted,
        landing_sites,
        ..
    } = context;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let public_map = connected_public_map(plan, context.public_map);
    let route_unavailable = if let Some(resources) = context.connected_resources.as_ref() {
        connected_provider_unavailable(
            obs,
            &resources.targets,
            &[],
            ConnectedRouteContext {
                intel,
                home: *home,
                target: op.target,
                public_map: context.public_map,
                orientation: context.orientation,
            },
        )
    } else {
        Vec::new()
    };
    let unavailable = merged_unavailable(enlisted, &route_unavailable);
    let previous_scout = op.scout;
    let previous_artillery = op.artillery.clone();
    let previous_strike_aircraft = op.strike_aircraft.clone();
    op.scout = op
        .scout
        .filter(|id| {
            unit(obs, *id).is_some_and(|member| member.kind == scout_kind)
                && !unavailable.contains(id)
        })
        .or_else(|| available(obs, &unavailable, |k| k == scout_kind).next());
    if op.scout != previous_scout {
        op.scout_dispatch = None;
    }
    assign_artillery(&mut op.artillery, plan, obs, &unavailable);
    assign_strike_aircraft(&mut op.strike_aircraft, plan, obs, &unavailable);
    if previous_strike_aircraft != op.strike_aircraft {
        op.strike_hold = None;
    }
    if previous_scout
        .is_some_and(|id| unit(obs, id).is_some() && route_unavailable.binary_search(&id).is_ok())
        && op.scout.is_none()
        || previous_strike_aircraft
            .iter()
            .any(|id| unit(obs, *id).is_some() && route_unavailable.binary_search(id).is_ok())
            && op.strike_aircraft.len() < plan.desired_strike_aircraft
    {
        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        return;
    }
    if previous_artillery
        .iter()
        .any(|id| unit(obs, *id).is_some() && route_unavailable.binary_search(id).is_ok())
        && op.artillery.len() < plan.desired_artillery
    {
        recover(op, AirRecoveryReason::UnreachableStaging, obs.tick);
        return;
    }
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    assign_exact(
        &mut plan.screen,
        plan.desired_screen,
        obs,
        enlisted,
        |kind| kind == screen_kind,
    );
    if !connected_package_is_feasible(op, plan, context) {
        recover(op, AirRecoveryReason::PreparationInfeasible, obs.tick);
        return;
    }
    schedule_missing_members(op, plan, context, scout_kind, out);
    let complete = assembly_complete(op, plan);
    if plan.connected_package.is_some() && !complete {
        hold_strike_aircraft(op, obs, *home, out);
    }
    if complete {
        if plan.airborne() {
            if !dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out) {
                recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                return;
            }
            enter(op, AirOperationPhase::SuppressAa, obs.tick);
            hold_air_strike(op, plan, obs, *home, out);
            return;
        }
        let objective = operation_objective_anchor(op, plan, intel);
        let Some(staging) = artillery_staging(
            op,
            obs,
            *home,
            objective,
            context.public_map,
            context.orientation,
        ) else {
            recover(op, AirRecoveryReason::UnreachableStaging, obs.tick);
            return;
        };
        let staging = match staging {
            ArtilleryStaging::NeedsRecon(goal) => {
                if !dispatch_scout_to(op, obs, goal, public_map, out) {
                    recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                    return;
                }
                hold_strike_aircraft(op, obs, *home, out);
                return;
            }
            ArtilleryStaging::Ready(staging) => staging,
        };
        if !dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out) {
            recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
            return;
        }
        enter(op, AirOperationPhase::SuppressAa, obs.tick);
        stage_artillery(op, staging, out);
        hold_strike_aircraft(op, obs, *home, out);
    }
}

fn assembly_complete(op: &AirOperation, plan: &AirPlan) -> bool {
    op.scout.is_some()
        && op.artillery.len() == plan.desired_artillery
        && op.strike_aircraft.len() == plan.desired_strike_aircraft
        && plan.screen.len() == plan.desired_screen
}

fn suppress(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) {
    let tuning = context.tuning;
    let obs = context.obs;
    let intel = context.intel;
    let home = context.home;
    let landing_sites = context.landing_sites;
    let public_map = connected_public_map(plan, context.public_map);
    let cluster_aa = (!plan.airborne()).then(|| cluster_air_defense(op, plan, intel));
    let connected_engagement = if plan.airborne() {
        None
    } else {
        prosecutable_cluster_air_defense_target(
            op,
            plan,
            obs,
            intel,
            public_map,
            context.orientation,
        )
    };
    let air_defense = if plan.airborne() {
        targetable_corridor_flak(intel, home, op.target, landing_sites).map(Target::Building)
    } else {
        connected_engagement
            .as_ref()
            .map(|engagement| engagement.target)
    };
    if let Some(air_defense) = air_defense {
        if elapsed(op.phase_started_at, obs.tick) >= tuning.reaction_delay {
            let units = if plan.airborne() {
                air_strike_members(op, plan, obs)
            } else {
                op.artillery.clone()
            };
            let firing_stands = connected_engagement
                .as_ref()
                .map(|engagement| engagement.firing_stands.clone())
                .unwrap_or_default();
            let positioned = plan.airborne()
                || firing_stands
                    .iter()
                    .all(|(id, stand)| unit(obs, *id).is_some_and(|member| member.tile == *stand));
            if !positioned {
                let dispatch = SuppressionDispatch::Position {
                    target: air_defense,
                    assignments: firing_stands.clone(),
                };
                let changed = plan.suppression_dispatch.as_ref() != Some(&dispatch);
                for (id, goal) in firing_stands {
                    if unit(obs, id)
                        .is_some_and(|member| member.tile != goal && (changed || member.idle))
                    {
                        out.intents.push(Intent::MoveUnits {
                            units: vec![id],
                            goal,
                        });
                    }
                }
                plan.suppression_dispatch = Some(dispatch);
                op.artillery_staging = None;
            } else {
                let dispatch = SuppressionDispatch::Attack {
                    target: air_defense,
                    units: units.clone(),
                };
                let repeat_refused_ground_order = !plan.airborne()
                    && units
                        .iter()
                        .any(|id| unit(obs, *id).is_some_and(|member| member.idle));
                if plan.suppression_dispatch.as_ref() != Some(&dispatch)
                    || repeat_refused_ground_order
                {
                    out.intents.push(Intent::AttackUnits {
                        units: units.clone(),
                        target: air_defense,
                    });
                }
                if plan.airborne() {
                    // The suppression attack displaced the prior home hold.
                    // Clearing it lets Verify issue a fresh regroup order
                    // before the strike aircraft commit to the primary objective.
                    op.strike_hold = None;
                    plan.strike_dispatch = None;
                } else {
                    op.artillery_staging = None;
                }
                plan.suppression_dispatch = Some(dispatch);
            }
        }
        let scouting = if plan.airborne() {
            dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out)
        } else {
            scout_and_hold(op, plan, context, &[], out)
        };
        if !scouting {
            out.intents.clear();
            recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        }
    } else {
        plan.suppression_dispatch = None;
        if plan.airborne() {
            match airborne_corridor_status(op, plan, obs, intel, home, landing_sites) {
                AirborneCorridorStatus::Defended => {
                    recover(op, AirRecoveryReason::NewAirDefense, obs.tick);
                }
                AirborneCorridorStatus::Clear => {
                    enter(op, AirOperationPhase::Verify, obs.tick);
                    if !scout_and_hold(op, plan, context, landing_sites, out) {
                        out.intents.clear();
                        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                    }
                }
                AirborneCorridorStatus::NeedsRecon => {
                    if !scout_and_hold(op, plan, context, landing_sites, out) {
                        out.intents.clear();
                        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                    }
                }
            }
            return;
        }
        match cluster_aa
            .expect("connected suppression has a cluster assessment")
            .evidence
        {
            AirDefenseEvidence::CurrentCoverage => {
                recover(op, AirRecoveryReason::NewAirDefense, obs.tick)
            }
            AirDefenseEvidence::VisibleWithoutKnownCoverage
                if corridor_clear(intel, home, connected_strike_anchor(op, plan, intel), &[]) =>
            {
                enter(op, AirOperationPhase::Verify, obs.tick);
                if !scout_and_hold(op, plan, context, &[], out) {
                    out.intents.clear();
                    recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                }
            }
            AirDefenseEvidence::RememberedCoverage
            | AirDefenseEvidence::Unknown
            | AirDefenseEvidence::VisibleWithoutKnownCoverage => {
                if !scout_and_hold(op, plan, context, &[], out) {
                    out.intents.clear();
                    recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                }
            }
        }
    }
}

fn verify(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) {
    let tuning = context.tuning;
    let obs = context.obs;
    let intel = context.intel;
    let home = context.home;
    let landing_sites = context.landing_sites;
    let cluster_aa = (!plan.airborne()).then(|| cluster_air_defense(op, plan, intel));
    let air_defense = if plan.airborne() {
        targetable_corridor_flak(intel, home, op.target, landing_sites).map(Target::Building)
    } else {
        cluster_aa.and_then(|assessment| assessment.targetable)
    };
    if air_defense.is_some() {
        enter(op, AirOperationPhase::SuppressAa, obs.tick);
        suppress(op, plan, context, out);
        return;
    }
    if plan.airborne() {
        match airborne_corridor_status(op, plan, obs, intel, home, landing_sites) {
            AirborneCorridorStatus::Defended => {
                recover(op, AirRecoveryReason::NewAirDefense, obs.tick);
            }
            AirborneCorridorStatus::Clear
                if elapsed(op.phase_started_at, obs.tick)
                    >= tuning
                        .reaction_delay
                        .saturating_add(tuning.commitment_hesitation) =>
            {
                enter(op, AirOperationPhase::Strike, obs.tick);
                strike(op, plan, context, out);
            }
            AirborneCorridorStatus::Clear | AirborneCorridorStatus::NeedsRecon => {
                if !scout_and_hold(op, plan, context, landing_sites, out) {
                    out.intents.clear();
                    recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                }
            }
        }
        return;
    }
    match cluster_aa
        .expect("connected verification has a cluster assessment")
        .evidence
    {
        AirDefenseEvidence::CurrentCoverage => {
            recover(op, AirRecoveryReason::NewAirDefense, obs.tick)
        }
        AirDefenseEvidence::VisibleWithoutKnownCoverage
            if corridor_clear(intel, home, connected_strike_anchor(op, plan, intel), &[])
                && elapsed(op.phase_started_at, obs.tick)
                    >= tuning
                        .reaction_delay
                        .saturating_add(tuning.commitment_hesitation) =>
        {
            enter(op, AirOperationPhase::Strike, obs.tick);
            strike(op, plan, context, out);
        }
        AirDefenseEvidence::RememberedCoverage
        | AirDefenseEvidence::Unknown
        | AirDefenseEvidence::VisibleWithoutKnownCoverage => {
            if !scout_and_hold(op, plan, context, &[], out) {
                out.intents.clear();
                recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
            }
        }
    }
}

fn strike(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) {
    let tuning = context.tuning;
    let obs = context.obs;
    let intel = context.intel;
    let home = context.home;
    let landing_sites = context.landing_sites;
    let public_map = connected_public_map(plan, context.public_map);
    let cluster_aa = (!plan.airborne()).then(|| cluster_air_defense(op, plan, intel));
    let air_defense = if plan.airborne() {
        targetable_corridor_flak(intel, home, op.target, landing_sites).map(Target::Building)
    } else {
        cluster_aa.and_then(|assessment| assessment.targetable)
    };
    let connected_cluster_needs_clearance = cluster_aa.is_some_and(|assessment| {
        assessment.has_targets && assessment.evidence == AirDefenseEvidence::CurrentCoverage
    });
    if air_defense.is_some() || connected_cluster_needs_clearance {
        enter(op, AirOperationPhase::SuppressAa, obs.tick);
        suppress(op, plan, context, out);
        return;
    }
    let live_target = live_strike_target(op, plan, intel);
    let strike_anchor = operation_objective_anchor(op, plan, intel);
    let staging = if plan.airborne() {
        None
    } else {
        let Some(staging) = artillery_staging(
            op,
            obs,
            home,
            strike_anchor,
            context.public_map,
            context.orientation,
        ) else {
            recover(op, AirRecoveryReason::UnreachableStaging, obs.tick);
            return;
        };
        match staging {
            ArtilleryStaging::NeedsRecon(goal) => {
                if !dispatch_scout_to(op, obs, goal, public_map, out) {
                    recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                    return;
                }
                hold_strike_aircraft(op, obs, home, out);
                return;
            }
            ArtilleryStaging::Ready(staging) => Some(staging),
        }
    };
    let corridor_clear = if plan.airborne() {
        airborne_corridor_status(op, plan, obs, intel, home, landing_sites)
            == AirborneCorridorStatus::Clear
    } else {
        corridor_clear(intel, home, strike_anchor, landing_sites)
    };
    if !corridor_clear {
        recover(op, AirRecoveryReason::NewAirDefense, obs.tick);
        return;
    }
    let attackers = air_strike_members(op, plan, obs);
    if let Some(target) = live_target {
        if let Some(id) = target.id {
            let mut air_routes =
                operation_route_projection(plan, obs, Domain::Air, public_map, context.orientation);
            if !exact_attack_group_reaches(&mut air_routes, obs, &attackers, target.anchor) {
                recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                return;
            }
            dispatch_air_strike(
                plan,
                obs,
                &attackers,
                AirStrikeDispatch::Attack {
                    target: id,
                    anchor: target.anchor,
                },
                out,
            );
        }
        op.strike_issued_at.get_or_insert(obs.tick);
    } else if operation_objective_cleared(op, plan, obs, intel) {
        if op
            .strike_issued_at
            .is_some_and(|tick| elapsed(tick, obs.tick) >= tuning.reaction_delay.max(20))
        {
            recover(op, AirRecoveryReason::Complete, obs.tick);
            return;
        }
        let mut air_routes =
            operation_route_projection(plan, obs, Domain::Air, public_map, context.orientation);
        let cleared_anchor = last_strike_anchor(plan).unwrap_or(strike_anchor);
        if !air_routes.group_reaches_command_goal(&attackers, cleared_anchor) {
            recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
            return;
        }
        dispatch_air_strike(
            plan,
            obs,
            &attackers,
            AirStrikeDispatch::AttackMove(cleared_anchor),
            out,
        );
        op.strike_issued_at.get_or_insert(obs.tick);
    }
    if let Some(staging) = staging {
        stage_artillery(op, staging, out);
    }
}

fn dispatch_air_strike(
    plan: &mut AirPlan,
    obs: &Observation,
    attackers: &[UnitId],
    dispatch: AirStrikeDispatch,
    out: &mut StrategicDecision,
) {
    let units = if plan.strike_dispatch == Some(dispatch) {
        attackers
            .iter()
            .copied()
            .filter(|id| unit(obs, *id).is_some_and(|member| member.idle))
            .collect()
    } else {
        attackers.to_vec()
    };
    plan.strike_dispatch = Some(dispatch);
    if units.is_empty() {
        return;
    }
    out.intents.push(match dispatch {
        AirStrikeDispatch::Attack { target, .. } => Intent::AttackUnits {
            units,
            target: Target::Building(target),
        },
        AirStrikeDispatch::AttackMove(goal) => Intent::AttackMoveUnits { units, goal },
    });
}

fn abort_if_needed(
    op: &mut AirOperation,
    plan: &AirPlan,
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
) {
    if op.phase <= AirOperationPhase::Assemble
        && op
            .scout_dispatch
            .is_some_and(|(scout, _)| unit(obs, scout).is_none())
    {
        recover(op, AirRecoveryReason::RequiredUnitLost, obs.tick);
        return;
    }
    let waiting_for_recon_scout =
        op.phase == AirOperationPhase::Recon && op.scout.is_none_or(|id| unit(obs, id).is_none());
    let connected_preparation =
        plan.connected_package.is_some() && op.phase <= AirOperationPhase::Assemble;
    if elapsed(op.started_at, obs.tick) >= operation_timeout(profile, plan)
        || (!connected_preparation
            && !waiting_for_recon_scout
            && elapsed(op.phase_started_at, obs.tick) >= phase_timeout(op.phase, plan))
    {
        recover(op, AirRecoveryReason::Timeout, obs.tick);
        return;
    }
    if !plan.airborne()
        && op.phase < AirOperationPhase::Strike
        && operation_objective_is_stale(op, plan, obs.tick, intel)
    {
        recover(op, AirRecoveryReason::StaleIntelligence, obs.tick);
        return;
    }
    let lost_required_force = if let Some(package) = &plan.connected_package {
        let live_suppression = op
            .artillery
            .iter()
            .filter_map(|id| unit(obs, *id))
            .map(|member| suppression_capability(member.kind, obs.faction))
            .fold(0_u64, u64::saturating_add);
        let live_strike = op
            .strike_aircraft
            .iter()
            .filter_map(|id| unit(obs, *id))
            .map(|member| strike_capability(member.kind, obs.faction))
            .fold(0_u64, u64::saturating_add);
        live_suppression < package.minimum_capability.suppression
            || live_strike < package.minimum_capability.strike
    } else if plan.airborne() {
        let live_strike_aircraft = op
            .strike_aircraft
            .iter()
            .filter(|id| unit(obs, **id).is_some())
            .count();
        live_strike_aircraft < plan.desired_strike_aircraft.div_ceil(2).max(1)
    } else {
        op.artillery.iter().all(|id| unit(obs, *id).is_none())
            || op
                .strike_aircraft
                .iter()
                .filter(|id| unit(obs, **id).is_some())
                .count()
                < plan.desired_strike_aircraft
    };
    if op.membership_frozen_at.is_some()
        && (op.scout.is_some_and(|id| unit(obs, id).is_none()) || lost_required_force)
    {
        recover(op, AirRecoveryReason::RequiredUnitLost, obs.tick);
        return;
    }
    if op.phase < AirOperationPhase::Strike && operation_objective_cleared(op, plan, obs, intel) {
        recover(op, AirRecoveryReason::ObjectiveLost, obs.tick);
    }
}

/// Schedules in demand order, spreading equal-load work across producers. If
/// the next otherwise-trainable member is unaffordable or all its queues are
/// full, the available bank is claimed so ordinary production cannot skim it.
fn schedule(
    context: &AirPlanningContext<'_>,
    demands: &[(UnitKind, usize)],
    out: &mut StrategicDecision,
) {
    let obs = context.obs;
    let mut bank = obs.scrap.saturating_sub(context.protected_current_scrap);
    let mut staged = vec![Vec::<UnitKind>::new(); obs.my_buildings.len()];
    for intent in context.production.prior_intents {
        let Intent::TrainAt { building, kind } = intent else {
            continue;
        };
        if let Some(index) = obs
            .my_buildings
            .iter()
            .position(|candidate| candidate.id == *building)
        {
            staged[index].push(*kind);
        }
    }
    'demands: for &(kind, count) in demands {
        if !requirements_met(obs, kind) || !has_producer(obs, kind) {
            continue;
        }
        for _ in 0..count {
            let cost = kind.stats().cost;
            let producer = obs
                .my_buildings
                .iter()
                .enumerate()
                .filter(|(index, building)| {
                    building.built
                        && building.kind.base_stats().produces.contains(&kind)
                        && obs.my_queues.get(*index).is_some_and(|queue| {
                            let depth = if building.kind == BuildingKind::Airworks {
                                STRATEGIC_AIR_QUEUE_DEPTH
                            } else {
                                QUEUE_CAP
                            };
                            queue.len().saturating_add(staged[*index].len()) < depth
                        })
                        && context
                            .production
                            .lane_reservations
                            .allows_raw_immediate_append(building.id, &staged[*index], kind)
                })
                .min_by_key(|(index, building)| {
                    (
                        obs.my_queues[*index].len() + staged[*index].len(),
                        building.id,
                    )
                })
                .map(|(index, building)| (index, building.id));
            if bank < cost || producer.is_none() {
                out.committed_scrap = out.committed_scrap.saturating_add(bank.min(cost));
                break 'demands;
            }
            let Some((index, building)) = producer else {
                break 'demands;
            };
            bank -= cost;
            staged[index].push(kind);
            out.committed_scrap += cost;
            out.intents.push(Intent::TrainAt { building, kind });
        }
    }
}

#[cfg(test)]
fn select_target(
    intel: &StrategicIntelligence,
    now: Tick,
    tactical_memory: Tick,
) -> Option<&BuildingContact> {
    select_target_candidates(intel, now, tactical_memory)
        .into_iter()
        .next()
}

fn select_target_candidates(
    intel: &StrategicIntelligence,
    now: Tick,
    tactical_memory: Tick,
) -> Vec<&BuildingContact> {
    let mut candidates: Vec<_> = intel
        .buildings()
        .iter()
        .filter(|b| {
            b.built
                && building_value(b.kind) > 0
                && b.confidence_at(now) > 0
                && (b.evidence == ContactEvidence::Current
                    || b.last_seen
                        .is_some_and(|seen| elapsed(seen, now) <= tactical_memory))
        })
        .collect();
    candidates.sort_unstable_by_key(|b| {
        (
            b.evidence != ContactEvidence::Current,
            Reverse(building_value(b.kind)),
            Reverse(b.confidence_at(now)),
            b.anchor.y,
            b.anchor.x,
            b.player,
            b.kind,
        )
    });
    candidates
}

fn select_wealthy_island_target<'a>(
    profile: &ResolvedProfile,
    obs: &Observation,
    home: TilePos,
    intel: &'a StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
) -> Option<&'a BuildingContact> {
    intel
        .buildings()
        .iter()
        .filter(|target| {
            target.built
                && building_value(target.kind) > 0
                && wealthy_island_target(profile, obs, home, target, public_map)
        })
        .min_by_key(|target| {
            (
                target.evidence != ContactEvidence::Current,
                Reverse(building_value(target.kind)),
                target.anchor.y,
                target.anchor.x,
                target.player,
                target.kind,
            )
        })
}

fn exact_wealthy_island_target<'a>(
    profile: &ResolvedProfile,
    obs: &Observation,
    home: TilePos,
    intel: &'a StrategicIntelligence,
    request: &LiftSupportRequest,
    public_map: Option<&PublicMapBriefing>,
) -> Option<&'a BuildingContact> {
    intel.buildings().iter().find(|target| {
        target.player == request.player
            && target.anchor == request.target
            && target.built
            && building_value(target.kind) > 0
            && wealthy_island_target(profile, obs, home, target, public_map)
    })
}

fn live_strike_target<'a>(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    operation_target_cluster(op, plan, intel)
        .into_iter()
        .filter(|building| building.evidence == ContactEvidence::Current && building.id.is_some())
        .min_by_key(|building| operation_target_key(op, building))
}

fn operation_target_key(
    op: &AirOperation,
    building: &BuildingContact,
) -> (bool, Reverse<u32>, i32, i32, Option<BuildingId>) {
    (
        building.anchor != op.target,
        Reverse(u32::from(building_value(building.kind))),
        building.anchor.y,
        building.anchor.x,
        building.id,
    )
}

fn refresh_target(op: &mut AirOperation, intel: &StrategicIntelligence) {
    if let Some(target) = intel.buildings().iter().find(|b| {
        b.player == op.target_player
            && b.anchor == op.target
            && b.evidence == ContactEvidence::Current
    }) {
        op.target_kind = target.kind;
        op.target_id = target.id;
    }
}

fn rebase_adjudicated_connected_target(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    target: &BuildingContact,
    surviving_anchors: &[TilePos],
) {
    let objective = target
        .id
        .expect("a current connected revision target has an exact id");
    let identity = ConnectedOffenseIdentity::new(objective, target.anchor);
    op.target_kind = target.kind;
    op.target = target.anchor;
    op.target_id = Some(objective);
    let package = plan
        .connected_package
        .as_mut()
        .expect("an adjudicated connected operation retains its package");
    package.target_anchors = surviving_anchors.to_vec();
    if let Some(assignments) = package.producer_assignments.as_mut() {
        for assignment in assignments {
            assignment.owner = identity;
        }
    }
}

fn prosecutable_cluster_air_defense_target(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
    orientation: Orientation,
) -> Option<SuppressionEngagement> {
    let mut targets = Vec::new();
    let cluster = operation_target_cluster(op, plan, intel);
    for source in target_cluster_air_defense(intel, &cluster).sources {
        if source.evidence == ContactEvidence::Current
            && let Some(target) = current_air_defense_target(intel, source.source)
        {
            targets.push((source.source, target));
        }
    }
    targets.sort_unstable_by_key(|(source, _)| *source);
    targets.dedup_by_key(|(source, _)| *source);

    targets.into_iter().find_map(|(_, target)| {
        artillery_firing_assignments(obs, intel, &op.artillery, target, public_map, orientation)
            .map(|firing_stands| SuppressionEngagement {
                target,
                firing_stands,
            })
    })
}

fn artillery_firing_assignments(
    obs: &Observation,
    intel: &StrategicIntelligence,
    artillery: &[UnitId],
    target: Target,
    public_map: Option<&PublicMapBriefing>,
    orientation: Orientation,
) -> Option<Vec<(UnitId, TilePos)>> {
    if artillery.is_empty() {
        return None;
    }
    let mut members = artillery.to_vec();
    members.sort_unstable();
    members.dedup();
    let origins: Option<Vec<_>> = members
        .iter()
        .map(|id| {
            unit(obs, *id).map(|member| SuppressionOrigin {
                tile: member.tile,
                kind: member.kind,
            })
        })
        .collect();
    let firing_stands =
        suppression_firing_assignment(obs, intel, &origins?, target, public_map, orientation)?;
    Some(members.into_iter().zip(firing_stands).collect())
}

fn suppression_firing_assignment(
    obs: &Observation,
    intel: &StrategicIntelligence,
    origins: &[SuppressionOrigin],
    target: Target,
    public_map: Option<&PublicMapBriefing>,
    orientation: Orientation,
) -> Option<Vec<TilePos>> {
    if origins.is_empty() {
        return None;
    }
    let mut routes =
        route_projection_with_orientation(obs, Domain::Ground, public_map, orientation);
    let stand_options: Vec<Vec<TilePos>> = origins
        .iter()
        .map(|origin| {
            suppression_firing_stands(&mut routes, obs, *origin, target, intel, public_map)
                .collect()
        })
        .collect();
    if stand_options.iter().any(Vec::is_empty) {
        return None;
    }

    let mut stands: Vec<_> = stand_options.iter().flatten().copied().collect();
    stands.sort_unstable_by_key(|stand| (stand.y, stand.x));
    stands.dedup();
    let options: Vec<Vec<usize>> = stand_options
        .iter()
        .map(|member_options| {
            member_options
                .iter()
                .map(|stand| {
                    stands
                        .binary_search_by_key(&(stand.y, stand.x), |candidate| {
                            (candidate.y, candidate.x)
                        })
                        .expect("every firing option came from the canonical stand set")
                })
                .collect()
        })
        .collect();
    let mut owner_by_stand = vec![None; stands.len()];
    for member in 0..origins.len() {
        let mut visited = vec![false; stands.len()];
        if !augment_suppression_assignment(member, &options, &mut visited, &mut owner_by_stand) {
            return None;
        }
    }
    let mut assigned = vec![None; origins.len()];
    for (stand, owner) in owner_by_stand.into_iter().enumerate() {
        if let Some(member) = owner {
            assigned[member] = Some(stands[stand]);
        }
    }
    assigned.into_iter().collect()
}

fn augment_suppression_assignment(
    member: usize,
    options: &[Vec<usize>],
    visited: &mut [bool],
    owner_by_stand: &mut [Option<usize>],
) -> bool {
    for &stand in &options[member] {
        if visited[stand] {
            continue;
        }
        visited[stand] = true;
        if owner_by_stand[stand].is_none_or(|owner| {
            augment_suppression_assignment(owner, options, visited, owner_by_stand)
        }) {
            owner_by_stand[stand] = Some(member);
            return true;
        }
    }
    false
}

fn exact_attack_group_reaches(
    routes: &mut RouteProjection<'_>,
    obs: &Observation,
    units: &[UnitId],
    target: TilePos,
) -> bool {
    !units.is_empty()
        && units
            .iter()
            .all(|id| unit(obs, *id).is_some_and(|member| routes.unit_reaches(member, target)))
}

fn cluster_air_defense(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &StrategicIntelligence,
) -> ClusterAirDefense {
    let cluster = operation_target_cluster(op, plan, intel);
    let has_targets = !cluster.is_empty();
    let assessment = target_cluster_air_defense(intel, &cluster);
    let mut current_coverage = false;
    let mut remembered_coverage = false;
    let mut targetable = Vec::new();

    for source in assessment.sources {
        match source.evidence {
            ContactEvidence::Current => {
                let Some(target) = current_air_defense_target(intel, source.source) else {
                    if current_air_defense_is_operational(intel, source.source) {
                        current_coverage = true;
                    }
                    continue;
                };
                current_coverage = true;
                targetable.push((source.source, target));
            }
            ContactEvidence::Remembered if source.confidence > 0 => {
                remembered_coverage = true;
            }
            ContactEvidence::Remembered => {}
        }
    }

    targetable.sort_unstable_by_key(|(source, _)| *source);
    targetable.dedup_by_key(|(source, _)| *source);
    let evidence = if current_coverage {
        AirDefenseEvidence::CurrentCoverage
    } else if remembered_coverage {
        AirDefenseEvidence::RememberedCoverage
    } else if assessment.all_target_tiles_visible {
        AirDefenseEvidence::VisibleWithoutKnownCoverage
    } else {
        AirDefenseEvidence::Unknown
    };

    ClusterAirDefense {
        has_targets,
        targetable: targetable.first().map(|(_, target)| *target),
        evidence,
    }
}

fn operation_target_cluster<'a>(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &'a StrategicIntelligence,
) -> Vec<&'a BuildingContact> {
    let Some(package) = plan.connected_package.as_ref() else {
        return current_target_cluster(intel, op.target_player, op.target);
    };
    frozen_connected_target_contacts(op, package, intel)
}

fn frozen_connected_target_contacts<'a>(
    op: &AirOperation,
    package: &ConnectedForcePackage,
    intel: &'a StrategicIntelligence,
) -> Vec<&'a BuildingContact> {
    intel
        .buildings()
        .iter()
        .filter(|contact| {
            contact.player == op.target_player
                && package.target_anchors.contains(&contact.anchor)
                && contact.built
                && contact.hp > 0
                && building_value(contact.kind) > 0
        })
        .collect()
}

fn operation_objective_anchor(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &StrategicIntelligence,
) -> TilePos {
    live_strike_target(op, plan, intel)
        .or_else(|| {
            plan.connected_package.as_ref().and_then(|package| {
                frozen_connected_target_contacts(op, package, intel)
                    .into_iter()
                    .min_by_key(|contact| operation_target_key(op, contact))
            })
        })
        .map_or_else(
            || last_strike_anchor(plan).unwrap_or(op.target),
            |contact| contact.anchor,
        )
}

fn last_strike_anchor(plan: &AirPlan) -> Option<TilePos> {
    match plan.strike_dispatch {
        Some(AirStrikeDispatch::Attack { anchor, .. })
        | Some(AirStrikeDispatch::AttackMove(anchor)) => Some(anchor),
        None => None,
    }
}

fn operation_objective_is_stale(
    op: &AirOperation,
    plan: &AirPlan,
    now: Tick,
    intel: &StrategicIntelligence,
) -> bool {
    let Some(package) = plan.connected_package.as_ref() else {
        return intel.buildings().iter().any(|building| {
            building.player == op.target_player
                && building.anchor == op.target
                && building.evidence == ContactEvidence::Remembered
                && building
                    .last_seen
                    .is_none_or(|seen| elapsed(seen, now) > ACTIVE_OPERATION_TARGET_MEMORY)
        });
    };
    let contacts = frozen_connected_target_contacts(op, package, intel);
    !contacts.is_empty()
        && contacts
            .iter()
            .all(|building| building.evidence == ContactEvidence::Remembered)
        && contacts.iter().all(|building| {
            building
                .last_seen
                .is_none_or(|seen| elapsed(seen, now) > ACTIVE_OPERATION_TARGET_MEMORY)
        })
}

fn operation_objective_cleared(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> bool {
    let Some(package) = plan.connected_package.as_ref() else {
        let target_is_current = intel.buildings().iter().any(|building| {
            building.player == op.target_player
                && building.anchor == op.target
                && building.evidence == ContactEvidence::Current
        });
        return target_visible(op, obs) && !target_is_current;
    };
    frozen_connected_target_contacts(op, package, intel).is_empty()
        && package
            .target_anchors
            .iter()
            .all(|anchor| obs.visible(*anchor))
}

fn connected_strike_anchor(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &StrategicIntelligence,
) -> TilePos {
    operation_objective_anchor(op, plan, intel)
}

fn current_air_defense_target(
    intel: &StrategicIntelligence,
    source: AirDefenseSource,
) -> Option<Target> {
    match source {
        AirDefenseSource::Unit { id, kind, tile } => intel
            .units()
            .iter()
            .find(|contact| {
                contact.id == id
                    && contact.kind == kind
                    && contact.tile == tile
                    && contact.evidence == ContactEvidence::Current
                    && contact.hp > 0
            })
            .filter(|contact| contact.body_domain() == Domain::Ground)
            .map(|_| Target::Unit(id)),
        AirDefenseSource::Building {
            id: Some(id),
            player,
            kind,
            anchor,
        } if intel.buildings().iter().any(|contact| {
            contact.id == Some(id)
                && contact.player == player
                && contact.kind == kind
                && contact.anchor == anchor
                && contact.evidence == ContactEvidence::Current
                && contact.built
                && contact.hp > 0
        }) =>
        {
            Some(Target::Building(id))
        }
        AirDefenseSource::Building { .. } => None,
    }
}

fn current_air_defense_is_operational(
    intel: &StrategicIntelligence,
    source: AirDefenseSource,
) -> bool {
    match source {
        AirDefenseSource::Unit { id, kind, tile } => intel.units().iter().any(|contact| {
            contact.id == id
                && contact.kind == kind
                && contact.tile == tile
                && contact.evidence == ContactEvidence::Current
                && contact.hp > 0
        }),
        AirDefenseSource::Building {
            id: Some(id),
            player,
            kind,
            anchor,
        } => intel.buildings().iter().any(|contact| {
            contact.id == Some(id)
                && contact.player == player
                && contact.kind == kind
                && contact.anchor == anchor
                && contact.evidence == ContactEvidence::Current
                && contact.built
                && contact.hp > 0
        }),
        AirDefenseSource::Building { id: None, .. } => false,
    }
}

fn targetable_flak(aa: &AirDefenseAssessment) -> Option<BuildingId> {
    aa.sources.iter().find_map(|source| {
        if source.evidence == ContactEvidence::Current
            && let AirDefenseSource::Building {
                id: Some(id),
                kind: BuildingKind::FlakTurret,
                ..
            } = source.source
        {
            Some(id)
        } else {
            None
        }
    })
}

fn targetable_corridor_flak(
    intel: &StrategicIntelligence,
    home: TilePos,
    target: TilePos,
    landing_sites: &[TilePos],
) -> Option<BuildingId> {
    flight_objectives(target, landing_sites)
        .into_iter()
        .flat_map(|objective| flight_corridor(home, objective))
        .find_map(|tile| targetable_flak(&intel.air_defense_at(tile)))
}

fn corridor_clear(
    intel: &StrategicIntelligence,
    home: TilePos,
    target: TilePos,
    landing_sites: &[TilePos],
) -> bool {
    flight_objectives(target, landing_sites)
        .into_iter()
        .all(|objective| {
            let known_route_is_clear = flight_corridor(home, objective).into_iter().all(|tile| {
                matches!(
                    intel.air_defense_at(tile).evidence(),
                    AirDefenseEvidence::VisibleWithoutKnownCoverage | AirDefenseEvidence::Unknown
                )
            });
            known_route_is_clear
                && approach(home, objective).all(|tile| {
                    intel.air_defense_at(tile).evidence()
                        == AirDefenseEvidence::VisibleWithoutKnownCoverage
                })
        })
}

fn flight_objectives(target: TilePos, landing_sites: &[TilePos]) -> Vec<TilePos> {
    let mut objectives = vec![target];
    objectives.extend_from_slice(landing_sites);
    objectives.sort_unstable_by_key(|tile| (tile.y, tile.x));
    objectives.dedup();
    objectives
}

fn airborne_corridor_status(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    landing_sites: &[TilePos],
) -> AirborneCorridorStatus {
    let objectives = flight_objectives(op.target, landing_sites);
    let mut mobile_sources = std::collections::BTreeMap::new();

    for assessment in objectives
        .iter()
        .flat_map(|objective| flight_corridor(home, *objective))
        .map(|tile| intel.air_defense_at(tile))
    {
        for source in assessment
            .sources
            .iter()
            .filter(|source| source.evidence == ContactEvidence::Current)
        {
            match source.source {
                AirDefenseSource::Building { .. } => return AirborneCorridorStatus::Defended,
                AirDefenseSource::Unit { id, kind, .. } => {
                    mobile_sources
                        .entry(id)
                        .and_modify(|entry: &mut (UnitKind, u32)| {
                            entry.1 = entry.1.max(source.firepower_per_100_ticks);
                        })
                        .or_insert((kind, source.firepower_per_100_ticks));
                }
            }
        }
    }

    let wing_hp = air_strike_members(op, plan, obs)
        .into_iter()
        .filter_map(|id| unit(obs, id))
        .fold(0u64, |total, member| {
            total.saturating_add(u64::from(member.hp))
        });
    let mobile_firepower = mobile_sources
        .values()
        .fold(0u64, |total, (kind, firepower)| {
            let weight = if kind.role() == Role::AntiAir {
                DEDICATED_MOBILE_AA_WEIGHT
            } else {
                1
            };
            total.saturating_add(u64::from(*firepower).saturating_mul(weight))
        });
    let projected_damage = mobile_firepower
        .saturating_mul(MOBILE_AA_EXPOSURE_TICKS)
        .div_ceil(100);
    if projected_damage.saturating_mul(MOBILE_AA_SURVIVAL_MARGIN) > wing_hp {
        return AirborneCorridorStatus::Defended;
    }

    let route_is_clear = |assessment: &AirDefenseAssessment| match assessment.evidence() {
        AirDefenseEvidence::VisibleWithoutKnownCoverage | AirDefenseEvidence::Unknown => true,
        AirDefenseEvidence::CurrentCoverage => current_mobile_coverage_is_fresh(assessment),
        AirDefenseEvidence::RememberedCoverage => false,
    };
    let approach_is_clear = |assessment: &AirDefenseAssessment| {
        assessment.target_visible
            && match assessment.evidence() {
                AirDefenseEvidence::VisibleWithoutKnownCoverage => true,
                AirDefenseEvidence::CurrentCoverage => current_mobile_coverage_is_fresh(assessment),
                AirDefenseEvidence::RememberedCoverage | AirDefenseEvidence::Unknown => false,
            }
    };
    let clear = objectives.into_iter().all(|objective| {
        flight_corridor(home, objective)
            .into_iter()
            .map(|tile| intel.air_defense_at(tile))
            .all(|assessment| route_is_clear(&assessment))
            && approach(home, objective)
                .map(|tile| intel.air_defense_at(tile))
                .all(|assessment| approach_is_clear(&assessment))
    });
    if clear {
        AirborneCorridorStatus::Clear
    } else {
        AirborneCorridorStatus::NeedsRecon
    }
}

fn current_mobile_coverage_is_fresh(assessment: &AirDefenseAssessment) -> bool {
    assessment
        .sources
        .iter()
        .all(|source| match source.evidence {
            ContactEvidence::Current => matches!(source.source, AirDefenseSource::Unit { .. }),
            ContactEvidence::Remembered => source.confidence == 0,
        })
}

fn flight_corridor(home: TilePos, target: TilePos) -> Vec<TilePos> {
    let mut tiles = Vec::new();
    let mut current = home;
    let dx = (target.x - home.x).abs();
    let step_x = (target.x - home.x).signum();
    let dy = -(target.y - home.y).abs();
    let step_y = (target.y - home.y).signum();
    let mut error = dx + dy;
    loop {
        tiles.push(current);
        if current == target {
            break;
        }
        let twice_error = error.saturating_mul(2);
        if twice_error >= dy {
            error += dy;
            current.x += step_x;
        }
        if twice_error <= dx {
            error += dx;
            current.y += step_y;
        }
    }
    tiles
}

fn approach(home: TilePos, target: TilePos) -> impl Iterator<Item = TilePos> {
    let dx = (home.x - target.x).signum();
    let dy = (home.y - target.y).signum();
    (0..=APPROACH_TILES).map(move |step| target.offset(dx * step, dy * step))
}

fn merged_unavailable(first: &[UnitId], second: &[UnitId]) -> Vec<UnitId> {
    let mut merged = first.to_vec();
    merged.extend_from_slice(second);
    merged.sort_unstable();
    merged.dedup();
    merged
}

fn connected_provider_unavailable<'a>(
    obs: &'a Observation,
    targets: &ConnectedTargetSelection,
    unavailable: &[UnitId],
    route: ConnectedRouteContext<'a>,
) -> Vec<UnitId> {
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let staging = connected_artillery_staging_goal(obs, route.home, route.target, route.public_map);
    let mut ground_routes =
        route_projection_with_orientation(obs, Domain::Ground, route.public_map, route.orientation);
    let mut air_routes =
        route_projection_with_orientation(obs, Domain::Air, route.public_map, route.orientation);
    let mut excluded = unavailable.to_vec();
    excluded.extend(obs.my_units.iter().filter_map(|member| {
        let compatible = if is_artillery(member.kind) {
            staging.is_some_and(|goal| {
                ground_routes.ground_command_reaches(member.tile, goal)
                    && suppression_targets_reachable(
                        &mut ground_routes,
                        obs,
                        SuppressionOrigin {
                            tile: member.tile,
                            kind: member.kind,
                        },
                        &targets.suppression_targets,
                        route.intel,
                        route.public_map,
                    )
            })
        } else if member.kind == scout_kind || is_strike_aircraft(member.kind, obs.faction) {
            targets
                .target_anchors
                .iter()
                .all(|anchor| air_routes.unit_reaches(member, *anchor))
        } else {
            true
        };
        (!compatible).then_some(member.id)
    }));
    excluded.sort_unstable();
    excluded.dedup();
    excluded
}

fn connected_production_access<'a>(
    obs: &'a Observation,
    targets: &ConnectedTargetSelection,
    resources: &ResourceSnapshot,
    route: ConnectedRouteContext<'a>,
) -> ProductionAccess {
    let staging = connected_artillery_staging_goal(obs, route.home, route.target, route.public_map);
    let mut ground_routes =
        route_projection_with_orientation(obs, Domain::Ground, route.public_map, route.orientation);
    let mut air_routes =
        route_projection_with_orientation(obs, Domain::Air, route.public_map, route.orientation);
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
        let mut trainable = completed_producer_trainable_kinds(obs, producer);
        trainable.sort_unstable();
        trainable.dedup();
        let mut paid = obs
            .my_queues
            .get(producer_index)
            .cloned()
            .unwrap_or_default();
        paid.sort_unstable();
        paid.dedup();
        let mut candidates = trainable.clone();
        candidates.extend_from_slice(&paid);
        candidates.sort_unstable();
        candidates.dedup();

        for kind in candidates {
            let accessible = match kind.stats().domain {
                Domain::Ground if is_artillery(kind) => staging.is_some_and(|staging| {
                    production_spawn_doorstep(obs, producer, route.public_map, route.orientation)
                        .is_some_and(|spawn| {
                            ground_routes.ground_command_reaches(spawn, staging)
                                && suppression_targets_reachable(
                                    &mut ground_routes,
                                    obs,
                                    SuppressionOrigin { tile: spawn, kind },
                                    &targets.suppression_targets,
                                    route.intel,
                                    route.public_map,
                                )
                        })
                }),
                Domain::Air
                    if kind == Role::Scout.unit_for(obs.faction)
                        || is_strike_aircraft(kind, obs.faction) =>
                {
                    let size = producer.kind.tier_stats(producer.tier).size;
                    let spawn = producer.anchor.offset(size.0 / 2, size.1 / 2);
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
}

fn connected_target_selection<'a>(
    obs: &'a Observation,
    target: &BuildingContact,
    unavailable: &[UnitId],
    route: ConnectedRouteContext<'a>,
) -> ConnectedTargetSelection {
    let mut candidates = current_target_cluster(route.intel, target.player, target.anchor);
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
    let staging =
        connected_artillery_staging_goal(obs, route.home, target.anchor, route.public_map);
    let mut air_routes =
        route_projection_with_orientation(obs, Domain::Air, route.public_map, route.orientation);
    let mut ground_routes =
        route_projection_with_orientation(obs, Domain::Ground, route.public_map, route.orientation);
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
            connected_family_reaches_all(&mut air_routes, &recon_origins, &proposed_anchors)
                && connected_family_reaches_all(
                    &mut air_routes,
                    &strike_origins,
                    &proposed_anchors,
                );
        let suppression_reachable = proposed_suppression.is_empty()
            || staging.is_some_and(|staging| {
                suppression_origins.iter().any(|origin| {
                    ground_routes.ground_command_reaches(origin.tile, staging)
                        && suppression_targets_reachable(
                            &mut ground_routes,
                            obs,
                            *origin,
                            &proposed_suppression,
                            route.intel,
                            route.public_map,
                        )
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
}

#[derive(Debug, Default)]
struct CurrentSuppressionNeeds {
    targets: Vec<Target>,
    has_untargetable_current: bool,
}

fn current_cluster_suppression_needs(
    intel: &StrategicIntelligence,
    cluster: &[&BuildingContact],
) -> CurrentSuppressionNeeds {
    let mut needs = CurrentSuppressionNeeds::default();
    for source in target_cluster_air_defense(intel, cluster).sources {
        if source.evidence != ContactEvidence::Current
            || !current_air_defense_is_operational(intel, source.source)
        {
            continue;
        }
        if let Some(target) = current_air_defense_target(intel, source.source) {
            needs.targets.push(target);
        } else {
            needs.has_untargetable_current = true;
        }
    }
    needs.targets.sort_unstable();
    needs.targets.dedup();
    needs
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
            .map(|producer| {
                let size = producer.kind.tier_stats(producer.tier).size;
                producer.anchor.offset(size.0 / 2, size.1 / 2)
            }),
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
        let spawn = production_spawn_doorstep(obs, producer, public_map, orientation);
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
    producer: &super::observation::BuildingObs,
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
    producer: &super::observation::BuildingObs,
    accepts: impl Fn(UnitKind) -> bool,
) -> bool {
    completed_producer_trainable_kinds(obs, producer)
        .into_iter()
        .any(accepts)
}

fn connected_family_reaches_all(
    routes: &mut RouteProjection<'_>,
    origins: &[TilePos],
    targets: &[TilePos],
) -> bool {
    origins.iter().any(|origin| {
        targets
            .iter()
            .all(|target| routes.reaches(*origin, *target))
    })
}

fn suppression_targets_reachable(
    routes: &mut RouteProjection<'_>,
    obs: &Observation,
    origin: SuppressionOrigin,
    targets: &[Target],
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    targets.iter().all(|target| {
        suppression_firing_stands(routes, obs, origin, *target, intel, public_map)
            .next()
            .is_some()
    })
}

#[derive(Debug, Clone, Copy)]
enum SuppressionTargetGeometry {
    Unit(TilePos),
    Building { anchor: TilePos, size: (i32, i32) },
}

impl SuppressionTargetGeometry {
    fn tile_bounds(self) -> (TilePos, TilePos) {
        match self {
            Self::Unit(tile) => (tile, tile),
            Self::Building {
                anchor,
                size: (width, height),
            } => (anchor, anchor.offset(width - 1, height - 1)),
        }
    }

    fn aim_point(self, shooter: Vec2Fx) -> Vec2Fx {
        match self {
            Self::Unit(tile) => tile.center(),
            Self::Building {
                anchor,
                size: (width, height),
            } => {
                let min = anchor.center() - Vec2Fx::new(HALF, HALF);
                let max = min + Vec2Fx::new(Fx::from_num(width), Fx::from_num(height));
                Vec2Fx::new(shooter.x.clamp(min.x, max.x), shooter.y.clamp(min.y, max.y))
            }
        }
    }
}

fn suppression_target_geometry(
    intel: &StrategicIntelligence,
    target: Target,
) -> Option<SuppressionTargetGeometry> {
    match target {
        Target::Unit(id) => intel
            .units()
            .iter()
            .find(|contact| {
                contact.id == id
                    && contact.evidence == ContactEvidence::Current
                    && contact.hp > 0
                    && contact.body_domain() == Domain::Ground
            })
            .map(|contact| SuppressionTargetGeometry::Unit(contact.tile)),
        Target::Building(id) => intel
            .buildings()
            .iter()
            .find(|contact| {
                contact.id == Some(id)
                    && contact.evidence == ContactEvidence::Current
                    && contact.built
                    && contact.hp > 0
            })
            .map(|contact| SuppressionTargetGeometry::Building {
                anchor: contact.anchor,
                size: contact.kind.tier_stats(contact.tier).size,
            }),
    }
}

fn suppression_weapon(kind: UnitKind) -> Option<&'static WeaponStats> {
    if !is_artillery(kind) {
        return None;
    }
    kind.stats()
        .weapons
        .iter()
        .find(|weapon| weapon.targets.covers(Domain::Ground))
}

fn suppression_firing_stands(
    routes: &mut RouteProjection<'_>,
    obs: &Observation,
    origin: SuppressionOrigin,
    target: Target,
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
) -> impl Iterator<Item = TilePos> {
    let mut stands = Vec::new();
    let Some(weapon) = suppression_weapon(origin.kind) else {
        return stands.into_iter();
    };
    let Some(geometry) = suppression_target_geometry(intel, target) else {
        return stands.into_iter();
    };
    let (near, far) = geometry.tile_bounds();
    let radius = weapon.range.ceil().to_num::<i32>();
    for y in near.y.saturating_sub(radius)..=far.y.saturating_add(radius) {
        for x in near.x.saturating_sub(radius)..=far.x.saturating_add(radius) {
            let stand = TilePos::new(x, y);
            if !public_ground_open(obs, stand, public_map)
                || !routes.ground_command_reaches(origin.tile, stand)
                || !suppression_shot_is_legal(obs, public_map, weapon, stand, geometry)
            {
                continue;
            }
            stands.push(stand);
        }
    }
    stands.sort_unstable_by_key(|stand| (stand.chebyshev(origin.tile), stand.y, stand.x));
    stands.into_iter()
}

fn suppression_shot_is_legal(
    obs: &Observation,
    public_map: Option<&PublicMapBriefing>,
    weapon: &WeaponStats,
    stand: TilePos,
    target: SuppressionTargetGeometry,
) -> bool {
    let shooter = stand.center();
    let aim = target.aim_point(shooter);
    let distance_sq = shooter.dist_sq(aim);
    if distance_sq > weapon.range * weapon.range
        || distance_sq < weapon.minimum_range * weapon.minimum_range
    {
        return false;
    }
    let shot_open = |tile| suppression_shot_tile_open(obs, public_map, weapon, tile);
    shot_open(TilePos::containing(aim)) && !chassis::path::line_blocked(shooter, aim, shot_open)
}

fn suppression_shot_tile_open(
    obs: &Observation,
    public_map: Option<&PublicMapBriefing>,
    weapon: &WeaponStats,
    tile: TilePos,
) -> bool {
    if !(0..obs.map_width).contains(&tile.x) || !(0..obs.map_height).contains(&tile.y) {
        return false;
    }
    if let Some(map) = public_map {
        return map.terrain_at(tile).is_some_and(|terrain| {
            !terrain.blocks_all_fire() && (weapon.indirect || !terrain.blocks_direct_fire())
        });
    }
    if obs
        .known_peaks
        .binary_search_by_key(&(tile.y, tile.x), |peak| (peak.y, peak.x))
        .is_ok()
    {
        return false;
    }
    weapon.indirect || !obs.known_rock_at(tile)
}

fn selected_current_target_cluster<'a>(
    intel: &'a StrategicIntelligence,
    original: &BuildingContact,
    anchors: &[TilePos],
) -> Vec<&'a BuildingContact> {
    current_target_contacts_at_anchors(intel, original.player, anchors)
}

fn current_target_contacts_at_anchors<'a>(
    intel: &'a StrategicIntelligence,
    player: PlayerId,
    anchors: &[TilePos],
) -> Vec<&'a BuildingContact> {
    intel
        .buildings()
        .iter()
        .filter(|contact| {
            contact.player == player
                && anchors.contains(&contact.anchor)
                && contact.evidence == ContactEvidence::Current
                && contact.built
                && contact.hp > 0
                && building_value(contact.kind) > 0
        })
        .collect()
}

fn production_spawn_doorstep(
    obs: &Observation,
    producer: &super::observation::BuildingObs,
    public_map: Option<&PublicMapBriefing>,
    orientation: Orientation,
) -> Option<TilePos> {
    let size = producer.kind.tier_stats(producer.tier).size;
    let world_anchor = orientation.anchor(producer.anchor, size);
    let map_size = (obs.map_width, obs.map_height);
    crate::tick::rect_adjacent_tiles(world_anchor, size)
        .map(|world_tile| (world_tile, orientation.tile(world_tile)))
        .filter(|(_, oriented_tile)| public_ground_open(obs, *oriented_tile, public_map))
        .min_by_key(|(world_tile, _)| {
            crate::tick::spawn_doorstep_key(map_size, world_anchor, size, *world_tile)
        })
        .map(|(_, oriented_tile)| oriented_tile)
}

fn available<'a>(
    obs: &'a Observation,
    enlisted: &'a [UnitId],
    accepts: impl Fn(UnitKind) -> bool + 'a,
) -> impl Iterator<Item = UnitId> + 'a {
    obs.my_units
        .iter()
        .filter(move |member| accepts(member.kind) && !enlisted.contains(&member.id))
        .map(|member| member.id)
}

fn assign_exact(
    assigned: &mut Vec<UnitId>,
    desired: usize,
    obs: &Observation,
    enlisted: &[UnitId],
    accepts: impl Fn(UnitKind) -> bool,
) {
    assigned.retain(|id| unit(obs, *id).is_some_and(|member| accepts(member.kind)));
    for member in &obs.my_units {
        if assigned.len() >= desired {
            break;
        }
        if accepts(member.kind) && !enlisted.contains(&member.id) && !assigned.contains(&member.id)
        {
            assigned.push(member.id);
        }
    }
    assigned.sort_unstable();
}

fn assign_artillery(
    assigned: &mut Vec<UnitId>,
    plan: &AirPlan,
    obs: &Observation,
    enlisted: &[UnitId],
) {
    if let Some(package) = &plan.connected_package {
        assign_provider_demands(assigned, &package.suppression, obs, enlisted);
    } else {
        assign_exact(
            assigned,
            plan.desired_artillery,
            obs,
            enlisted,
            is_artillery,
        );
    }
}

fn assign_strike_aircraft(
    assigned: &mut Vec<UnitId>,
    plan: &AirPlan,
    obs: &Observation,
    enlisted: &[UnitId],
) {
    if let Some(package) = &plan.connected_package {
        assign_provider_demands(assigned, &package.strike, obs, enlisted);
    } else {
        let bomber = Role::Bomber.unit_for(obs.faction);
        assign_exact(
            assigned,
            plan.desired_strike_aircraft,
            obs,
            enlisted,
            |kind| kind == bomber,
        );
    }
}

fn assign_provider_demands(
    assigned: &mut Vec<UnitId>,
    demands: &[ProviderDemand],
    obs: &Observation,
    enlisted: &[UnitId],
) {
    let mut selected = Vec::new();
    for demand in demands {
        selected.extend(
            assigned
                .iter()
                .copied()
                .filter(|id| {
                    !enlisted.contains(id)
                        && unit(obs, *id).is_some_and(|member| member.kind == demand.kind)
                })
                .take(demand.count),
        );
        let have = selected
            .iter()
            .filter(|id| unit(obs, **id).is_some_and(|member| member.kind == demand.kind))
            .count();
        let mut have = have;
        for member in &obs.my_units {
            if have >= demand.count {
                break;
            }
            if member.kind == demand.kind
                && !enlisted.contains(&member.id)
                && !selected.contains(&member.id)
            {
                selected.push(member.id);
                have += 1;
            }
        }
    }
    selected.sort_unstable();
    selected.dedup();
    *assigned = selected;
}

fn reservations(op: &AirOperation, plan: &AirPlan, obs: &Observation) -> Vec<UnitId> {
    let mut ids: Vec<_> = op
        .scout
        .into_iter()
        .chain(op.artillery.iter().copied())
        .chain(op.strike_aircraft.iter().copied())
        .chain(plan.screen.iter().copied())
        .filter(|id| unit(obs, *id).is_some())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn release_unroutable(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    survivors: &[UnitId],
    returning: &[UnitId],
) {
    let keep = |id: &UnitId| !survivors.contains(id) || returning.contains(id);
    op.scout = op.scout.filter(keep);
    op.artillery.retain(keep);
    op.strike_aircraft.retain(keep);
    plan.screen.retain(keep);
}

fn reusable_survivors(reason: Option<AirRecoveryReason>) -> bool {
    matches!(
        reason,
        Some(
            AirRecoveryReason::Complete
                | AirRecoveryReason::RequiredUnitLost
                | AirRecoveryReason::Timeout
        )
    )
}

fn queued(obs: &Observation, accepts: impl Fn(UnitKind) -> bool) -> usize {
    obs.my_queues
        .iter()
        .flatten()
        .filter(|kind| accepts(**kind))
        .count()
}

fn training_ticks(count: usize, kind: UnitKind) -> Tick {
    u64::try_from(count)
        .expect("the roster fits in addressable memory")
        .saturating_mul(u64::from(kind.stats().train_ticks))
}

fn remaining_training_ticks(obs: &Observation, count: usize, kind: UnitKind) -> Tick {
    let mut front_progress: Vec<_> = obs
        .my_buildings
        .iter()
        .enumerate()
        .filter_map(|(index, building)| {
            (obs.my_queues.get(index)?.first() == Some(&kind))
                .then(|| obs.own_queue_progress(index))
                .flatten()
                .map(|progress| {
                    let remaining = kind.stats().train_ticks.saturating_sub(progress).max(1);
                    let completed = kind.stats().train_ticks.saturating_sub(remaining);
                    (Reverse(completed), building.id)
                })
        })
        .collect();
    front_progress.sort_unstable();
    let completed_ticks = front_progress
        .into_iter()
        .take(count)
        .map(|(Reverse(progress), _)| Tick::from(progress))
        .fold(0, Tick::saturating_add);
    training_ticks(count, kind).saturating_sub(completed_ticks)
}

fn requirements_met(obs: &Observation, kind: UnitKind) -> bool {
    kind.stats().requires.iter().all(|required| {
        obs.my_buildings
            .iter()
            .any(|building| building.built && building.kind == *required)
    })
}

fn has_producer(obs: &Observation, kind: UnitKind) -> bool {
    obs.my_buildings
        .iter()
        .any(|building| building.built && building.kind.base_stats().produces.contains(&kind))
}

fn preferred_artillery(profile: &ResolvedProfile, obs: &Observation) -> UnitKind {
    if siege_leading(profile)
        && profile.traits.siege >= 68
        && requirements_met(obs, UnitKind::Avalanche)
        && has_producer(obs, UnitKind::Avalanche)
    {
        UnitKind::Avalanche
    } else {
        UnitKind::Bombard
    }
}

fn siege_leading(profile: &ResolvedProfile) -> bool {
    matches!(profile.primary, Specialty::Siege)
        || (matches!(profile.secondary, Specialty::Siege)
            && !matches!(profile.primary, Specialty::Air))
}

fn schedule_missing_members(
    op: &AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    scout_kind: UnitKind,
    out: &mut StrategicDecision,
) {
    let AirPlanningContext { profile, obs, .. } = context;
    if let Some(package) = &plan.connected_package {
        if package.producer_assignments.is_some() {
            let remaining = remaining_bound_provider_assignments(plan);
            out.committed_scrap = out.committed_scrap.saturating_add(
                remaining.iter().fold(0_u32, |total, assignment| {
                    total.saturating_add(assignment.funding.current_scrap)
                }),
            );
            for assignment in remaining {
                if assignment.timing.enqueued_at <= obs.tick {
                    out.intents.push(Intent::TrainAt {
                        building: assignment.producer,
                        kind: assignment.kind,
                    });
                    plan.issued_connected_providers
                        .push(assignment.request_ordinal);
                }
            }
            plan.issued_connected_providers.sort_unstable();
            plan.issued_connected_providers.dedup();
            return;
        }
        let connected_resources = context
            .connected_resources
            .as_ref()
            .expect("connected preparation has one observation-bound resource view");
        let resources = &connected_resources.snapshot;
        let production_access = &connected_resources.access;
        let demands = missing_package_demands(
            package,
            op,
            obs,
            resources,
            package.preparation_deadline,
            production_access,
        );
        let demands: Vec<_> = demands
            .into_iter()
            .map(|demand| ProductionDemand {
                kind: demand.kind,
                count: demand.count,
            })
            .collect();
        let schedule = plan_production_with_access(
            resources,
            &demands,
            package.preparation_deadline,
            resources.current_scrap().amount(),
            production_access,
        );
        let next_pending_cost = schedule.next_unfunded_cost;
        out.committed_scrap = out
            .committed_scrap
            .saturating_add(schedule.spent)
            .saturating_add(schedule.deferred_scrap);
        out.intents
            .extend(schedule.appends.into_iter().map(|append| Intent::TrainAt {
                building: append.producer,
                kind: append.kind,
            }));
        if let Some(next_cost) = next_pending_cost {
            out.committed_scrap = out.committed_scrap.saturating_add(
                obs.scrap
                    .saturating_sub(schedule.spent)
                    .saturating_sub(schedule.deferred_scrap)
                    .min(next_cost),
            );
        }
        return;
    }

    let bomber_kind = Role::Bomber.unit_for(obs.faction);
    let missing_scout =
        1usize.saturating_sub(usize::from(op.scout.is_some()) + queued(obs, |k| k == scout_kind));
    let missing_artillery = plan
        .desired_artillery
        .saturating_sub(op.artillery.len() + queued(obs, is_artillery));
    let missing_strike_aircraft = plan
        .desired_strike_aircraft
        .saturating_sub(op.strike_aircraft.len() + queued(obs, |k| k == bomber_kind));
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    let missing_screen = plan
        .desired_screen
        .saturating_sub(plan.screen.len() + queued(obs, |kind| kind == screen_kind));
    let demands = if plan.airborne() {
        [
            (scout_kind, missing_scout),
            (screen_kind, missing_screen),
            (bomber_kind, missing_strike_aircraft),
        ]
    } else {
        [
            (scout_kind, missing_scout),
            (preferred_artillery(profile, obs), missing_artillery),
            (bomber_kind, missing_strike_aircraft),
        ]
    };
    schedule(context, &demands, out);
}

fn connected_package_is_feasible(
    op: &AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
) -> bool {
    let Some(package) = &plan.connected_package else {
        return true;
    };
    if context.obs.tick >= package.preparation_deadline {
        return true;
    }
    let resources = context
        .connected_resources
        .as_ref()
        .expect("connected preparation has one observation-bound resource view");
    if package.producer_assignments.is_some() {
        let assignments = remaining_bound_provider_assignments(plan);
        if !assignments.is_empty() {
            return bound_provider_schedule_is_executable(
                &assignments,
                &resources.snapshot,
                package.preparation_deadline,
                context.tuning.cadence,
                context.obs.tick,
            );
        }
    }
    let outstanding = missing_package_demands(
        package,
        op,
        context.obs,
        &resources.snapshot,
        package.preparation_deadline,
        &resources.access,
    );
    let production_demands = outstanding
        .iter()
        .map(|demand| ProductionDemand {
            kind: demand.kind,
            count: demand.count,
        })
        .collect::<Vec<_>>();
    production_demands_fit_horizon_with_access(
        &resources.snapshot,
        &production_demands,
        package.preparation_deadline,
        &resources.access,
    ) && provider_demands_fit_funded_horizon(
        &resources.snapshot,
        &outstanding,
        context.obs.tick,
        PreparationConstraints {
            deadline: package.preparation_deadline,
            decision_cadence: context.tuning.cadence,
            protected_forecast_scrap: context.protected_forecast_scrap,
        },
        &resources.access,
    )
}

fn missing_package_demands(
    package: &ConnectedForcePackage,
    op: &AirOperation,
    obs: &Observation,
    resources: &ResourceSnapshot,
    deadline: Tick,
    production_access: &ProductionAccess,
) -> Vec<ProviderDemandTranche> {
    let mut available = Vec::<(ForceFamily, UnitKind, usize)>::new();
    let mut missing = Vec::new();
    for demand in &package.provider_priority {
        let assigned = match demand.family {
            ForceFamily::Recon => op.scout.as_slice(),
            ForceFamily::Suppression => op.artillery.as_slice(),
            ForceFamily::Strike => op.strike_aircraft.as_slice(),
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

fn remaining_bound_provider_assignments(plan: &AirPlan) -> Vec<ConnectedProducerAssignment> {
    let Some(package) = plan.connected_package.as_ref() else {
        return Vec::new();
    };
    let Some(assignments) = package.producer_assignments.as_ref() else {
        return Vec::new();
    };
    assignments
        .iter()
        .filter(|assignment| {
            plan.issued_connected_providers
                .binary_search(&assignment.request_ordinal)
                .is_err()
        })
        .copied()
        .collect()
}

fn bound_provider_schedule_is_executable(
    assignments: &[ConnectedProducerAssignment],
    resources: &ResourceSnapshot,
    deadline: Tick,
    cadence: Tick,
    observed_at: Tick,
) -> bool {
    if assignments.is_empty() {
        return true;
    }
    if assignments
        .iter()
        .any(|assignment| assignment.timing.enqueued_at < observed_at)
    {
        return false;
    }
    let Ok(projection) = resources.planning_projection(deadline, cadence) else {
        return false;
    };
    let mut producers = projection.producers().to_vec();
    let mut scheduled = assignments.iter().collect::<Vec<_>>();
    scheduled.sort_unstable_by_key(|assignment| {
        (
            assignment.producer,
            assignment.timing.starts_at,
            assignment.timing.ready_at,
            assignment.request_ordinal,
        )
    });
    for assignment in scheduled {
        let Some(index) = producers
            .binary_search_by_key(&assignment.producer, |producer| producer.producer())
            .ok()
        else {
            return false;
        };
        let Some(projected) =
            producers[index].append(assignment.kind, assignment.timing.enqueued_at)
        else {
            return false;
        };
        if projected.starts_at != assignment.timing.starts_at
            || projected.ready_at != assignment.timing.ready_at
            || assignment.timing.ready_before != deadline
            || projected.ready_at >= deadline
        {
            return false;
        }
    }
    true
}

fn ready_to_reconnoiter(obs: &Observation) -> bool {
    let scout = Role::Scout.unit_for(obs.faction);
    obs.my_units.iter().any(|unit| unit.kind == scout)
        || queued(obs, |kind| kind == scout) > 0
        || (requirements_met(obs, scout) && has_producer(obs, scout))
}

fn scout_and_hold(
    op: &mut AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
    landing_sites: &[TilePos],
    out: &mut StrategicDecision,
) -> bool {
    let public_map = connected_public_map(plan, context.public_map);
    let focus = if plan.connected_package.is_some() {
        connected_scout_focus(op, plan, context.obs, context.intel)
    } else {
        op.target
    };
    if !dispatch_scout_toward(
        op,
        context.obs,
        context.intel,
        focus,
        landing_sites,
        public_map,
        out,
    ) {
        return false;
    }
    hold_air_strike(op, plan, context.obs, context.home, out);
    true
}

fn dispatch_scout(
    op: &mut AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
    out: &mut StrategicDecision,
) -> bool {
    let target = if plan.connected_package.is_some() {
        connected_scout_focus(op, plan, obs, intel)
    } else {
        op.target
    };
    dispatch_scout_toward(op, obs, intel, target, landing_sites, public_map, out)
}

fn connected_scout_focus(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> TilePos {
    let Some(package) = plan.connected_package.as_ref() else {
        return op.target;
    };
    let mut contacts = frozen_connected_target_contacts(op, package, intel);
    contacts.sort_unstable_by_key(|contact| {
        (contact.anchor.y, contact.anchor.x, contact.id, contact.kind)
    });
    for contact in contacts {
        let (width, height) = contact.kind.tier_stats(contact.tier).size;
        for dy in 0..height {
            for dx in 0..width {
                let tile = contact.anchor.offset(dx, dy);
                if !obs.visible(tile) {
                    return tile;
                }
            }
        }
    }
    operation_objective_anchor(op, plan, intel)
}

fn dispatch_scout_toward(
    op: &mut AirOperation,
    obs: &Observation,
    intel: &StrategicIntelligence,
    target: TilePos,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
    out: &mut StrategicDecision,
) -> bool {
    let Some(goal) = scout_goal(op, obs, intel, target, landing_sites, public_map) else {
        return false;
    };
    dispatch_scout_to(op, obs, goal, public_map, out)
}

fn dispatch_scout_to(
    op: &mut AirOperation,
    obs: &Observation,
    goal: TilePos,
    public_map: Option<&PublicMapBriefing>,
    out: &mut StrategicDecision,
) -> bool {
    let Some(scout) = op.scout else {
        return true;
    };
    let Some(member) = unit(obs, scout) else {
        return false;
    };
    let mut air_routes = route_projection(obs, Domain::Air, public_map);
    if !air_routes.unit_reaches(member, goal) {
        return false;
    }
    if op.scout_dispatch == Some((scout, goal)) {
        return !member.idle || member.tile.chebyshev(goal) <= 1;
    }
    op.scout_dispatch = Some((scout, goal));
    if !member.idle || member.tile.chebyshev(goal) > 1 {
        out.intents.push(Intent::MoveUnits {
            units: vec![scout],
            goal,
        });
    }
    true
}

fn scout_goal(
    op: &AirOperation,
    obs: &Observation,
    intel: &StrategicIntelligence,
    target: TilePos,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    let vision = Role::Scout
        .unit_for(obs.faction)
        .stats()
        .vision
        .saturating_sub(1);
    let current = op
        .scout
        .and_then(|id| unit(obs, id))
        .map_or(target, |scout| scout.tile);
    let focus = flight_objectives(target, landing_sites)
        .into_iter()
        .find(|objective| {
            approach(current, *objective).any(|tile| {
                intel.air_defense_at(tile).evidence()
                    != AirDefenseEvidence::VisibleWithoutKnownCoverage
            })
        })
        .unwrap_or(target);
    let mut routes = route_projection(obs, Domain::Air, public_map);
    let radius_sq = vision.saturating_mul(vision);
    (focus.y - vision..=focus.y + vision)
        .flat_map(|y| (focus.x - vision..=focus.x + vision).map(move |x| TilePos::new(x, y)))
        .filter(|tile| {
            (0..obs.map_width).contains(&tile.x) && (0..obs.map_height).contains(&tile.y) && {
                let dx = tile.x - focus.x;
                let dy = tile.y - focus.y;
                dx.saturating_mul(dx) + dy.saturating_mul(dy) <= radius_sq
            }
        })
        .filter(|tile| routes.reaches(current, *tile))
        .min_by_key(|tile| {
            let evidence = match intel.air_defense_at(*tile).evidence() {
                AirDefenseEvidence::VisibleWithoutKnownCoverage => 0,
                AirDefenseEvidence::Unknown => 1,
                AirDefenseEvidence::RememberedCoverage => 2,
                AirDefenseEvidence::CurrentCoverage => 3,
            };
            (
                evidence,
                tile.chebyshev(current),
                tile.chebyshev(focus),
                tile.y,
                tile.x,
            )
        })
}

/// Nearest and farthest Chebyshev rings around the home anchor searched for
/// a landing pad. Ring 1 is skipped so the pad never hugs the Foundry
/// doorstep that production spawns and harvest traffic use.
const LANDING_PAD_RINGS: core::ops::RangeInclusive<i32> = 2..=6;

/// A parking tile for the held wing: the first tile by ring, then (y, x),
/// around the home anchor that is on the map, not known impassable, and
/// not under an own or allied footprint. The sim still snaps a landing to
/// landable ground, so this only has to be a sensible, stable choice.
fn landing_pad(obs: &Observation, home: TilePos) -> Option<TilePos> {
    let footprints: Vec<(TilePos, (i32, i32))> = obs
        .my_buildings
        .iter()
        .chain(obs.ally_buildings.iter())
        .map(|building| (building.anchor, building.kind.base_stats().size))
        .collect();
    let under_footprint = |tile: TilePos| {
        footprints.iter().any(|(anchor, (width, height))| {
            tile.x >= anchor.x
                && tile.x < anchor.x + width
                && tile.y >= anchor.y
                && tile.y < anchor.y + height
        })
    };
    let in_bounds = |tile: TilePos| {
        tile.x >= 0 && tile.y >= 0 && tile.x < obs.map_width && tile.y < obs.map_height
    };
    LANDING_PAD_RINGS
        .flat_map(|ring| {
            (-ring..=ring).flat_map(move |dy| {
                (-ring..=ring)
                    .filter(move |dx| dx.abs().max(dy.abs()) == ring)
                    .map(move |dx| home.offset(dx, dy))
            })
        })
        .find(|tile| in_bounds(*tile) && !obs.known_rock_at(*tile) && !under_footprint(*tile))
}

/// Parks the strike aircraft on the landing pad. A landed aircraft is idle, so
/// the later strike dispatch lifts it off exactly like a person clicking an
/// attack on a parked aircraft.
fn hold_strike_aircraft(
    op: &mut AirOperation,
    obs: &Observation,
    home: TilePos,
    out: &mut StrategicDecision,
) {
    let pad = landing_pad(obs, home).unwrap_or(home);
    if !op.strike_aircraft.is_empty() && op.strike_hold != Some(pad) {
        out.intents.push(Intent::MoveUnits {
            units: op.strike_aircraft.clone(),
            goal: pad,
        });
        op.strike_hold = Some(pad);
    }
}

fn hold_air_strike(
    op: &mut AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    home: TilePos,
    out: &mut StrategicDecision,
) {
    if !plan.airborne() {
        hold_strike_aircraft(op, obs, home, out);
        return;
    }
    let mut units = op.strike_aircraft.clone();
    units.extend(plan.screen.iter().copied());
    units.sort_unstable();
    units.dedup();
    let pad = landing_pad(obs, home).unwrap_or(home);
    if units.is_empty() || op.strike_hold == Some(pad) {
        return;
    }
    // Turn-limited kinds set down on the pad at the end of their move; the
    // screen holds airborne over home.
    let (landing, circling): (Vec<UnitId>, Vec<UnitId>) = units
        .into_iter()
        .partition(|id| unit(obs, *id).is_some_and(|member| member.kind.stats().turn_rate > 0));
    if !landing.is_empty() {
        out.intents.push(Intent::MoveUnits {
            units: landing,
            goal: pad,
        });
    }
    if !circling.is_empty() {
        out.intents.push(Intent::MoveUnits {
            units: circling,
            goal: home,
        });
    }
    op.strike_hold = Some(pad);
}

fn air_strike_members(op: &AirOperation, plan: &AirPlan, obs: &Observation) -> Vec<UnitId> {
    let mut units: Vec<_> = op
        .strike_aircraft
        .iter()
        .chain(plan.screen.iter())
        .copied()
        .filter(|id| unit(obs, *id).is_some())
        .collect();
    units.sort_unstable();
    units.dedup();
    units
}

fn stage_artillery(op: &mut AirOperation, staging: TilePos, out: &mut StrategicDecision) {
    if !op.artillery.is_empty() && op.artillery_staging != Some(staging) {
        out.intents.push(Intent::MoveUnits {
            units: op.artillery.clone(),
            goal: staging,
        });
        op.artillery_staging = Some(staging);
    }
}

fn target_seen(op: &AirOperation, plan: &AirPlan, obs: &Observation) -> bool {
    let package = plan.connected_package.as_ref();
    obs.enemy_buildings.iter().any(|building| {
        building.seen
            && building.player == op.target_player
            && package.map_or(building.anchor == op.target, |package| {
                package.target_anchors.contains(&building.anchor)
            })
    })
}

fn current_target_contact<'a>(
    op: &AirOperation,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    intel.buildings().iter().find(|building| {
        building.player == op.target_player
            && building.kind == op.target_kind
            && building.anchor == op.target
            && building.evidence == ContactEvidence::Current
    })
}

fn current_package_revision_target<'a>(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    let Some(package) = plan.connected_package.as_ref() else {
        return current_target_contact(op, intel);
    };
    frozen_connected_target_contacts(op, package, intel)
        .into_iter()
        .filter(|building| building.evidence == ContactEvidence::Current)
        .min_by_key(|building| operation_target_key(op, building))
}

fn target_visible(op: &AirOperation, obs: &Observation) -> bool {
    let (width, height) = op.target_kind.base_stats().size;
    (0..height).any(|dy| (0..width).any(|dx| obs.visible(op.target.offset(dx, dy))))
}

fn unit(obs: &Observation, id: UnitId) -> Option<&UnitObs> {
    obs.my_units
        .binary_search_by_key(&id, |member| member.id)
        .ok()
        .map(|index| &obs.my_units[index])
}

fn completed(obs: &Observation, kind: BuildingKind) -> usize {
    obs.my_buildings
        .iter()
        .filter(|building| building.built && building.kind == kind)
        .count()
}

fn combat_roster(obs: &Observation) -> usize {
    obs.my_units
        .iter()
        .filter(|unit| !unit.kind.stats().weapons.is_empty())
        .count()
}

fn wealthy_island_target(
    profile: &ResolvedProfile,
    obs: &Observation,
    home: TilePos,
    target: &BuildingContact,
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    if completed(obs, BuildingKind::Airworks) == 0
        || completed(obs, BuildingKind::Crucible) == 0
        || !ready_for_airborne_strike(obs)
    {
        return false;
    }
    let stance_delay: Tick = match profile.stance {
        BotStance::Turtle => 500,
        BotStance::Balanced => 250,
        BotStance::Aggressive => 0,
    };
    let personality_delay = u64::from(100u8.saturating_sub(profile.traits.air)) * 8;
    if obs.tick
        < ISLAND_OPERATION_EARLIEST_TICK
            .saturating_add(stance_delay)
            .saturating_add(personality_delay)
    {
        return false;
    }
    let renewable = completed(obs, BuildingKind::Extractor)
        .saturating_add(completed(obs, BuildingKind::Reclaimer));
    let developed_economy = renewable >= 2
        || completed(obs, BuildingKind::Foundry) >= 2
        || obs.scrap
            >= Role::Bomber
                .unit_for(obs.faction)
                .stats()
                .cost
                .saturating_mul(4);
    developed_economy
        && combat_roster(obs) >= 12
        && known_ground_disconnected(
            obs,
            home,
            target.anchor,
            target.kind.base_stats().size,
            public_map,
        )
}

fn ready_for_airborne_strike(obs: &Observation) -> bool {
    [
        Role::Scout.unit_for(obs.faction),
        Role::AirGround.unit_for(obs.faction),
        Role::Bomber.unit_for(obs.faction),
    ]
    .into_iter()
    .all(|kind| requirements_met(obs, kind) && has_producer(obs, kind))
}

fn known_ground_disconnected(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    target_size: (i32, i32),
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    known_ground_connection(obs, home, target, target_size, public_map) == Some(false)
}

fn known_ground_connected(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    target_size: (i32, i32),
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    known_ground_connection(obs, home, target, target_size, public_map) == Some(true)
}

fn known_ground_connection(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    target_size: (i32, i32),
    public_map: Option<&PublicMapBriefing>,
) -> Option<bool> {
    let home_size = obs
        .my_buildings
        .iter()
        .find(|building| {
            building.built && building.kind == BuildingKind::Foundry && building.anchor == home
        })
        .map_or(BuildingKind::Foundry.base_stats().size, |building| {
            building.kind.base_stats().size
        });
    let starts: Vec<_> = crate::tick::rect_adjacent_tiles(home, home_size)
        .filter(|tile| routing::ground_open(obs, *tile))
        .filter(|tile| {
            public_map.is_none_or(|map| {
                map.terrain_at(*tile)
                    .is_some_and(|terrain| !terrain.blocks_ground())
            })
        })
        .collect();
    let goals: Vec<_> = crate::tick::rect_adjacent_tiles(target, target_size)
        .filter(|tile| routing::ground_open(obs, *tile))
        .filter(|tile| {
            public_map.is_none_or(|map| {
                map.terrain_at(*tile)
                    .is_some_and(|terrain| !terrain.blocks_ground())
            })
        })
        .collect();
    if starts.is_empty() || goals.is_empty() {
        return None;
    }

    if let Some(public_map) = public_map {
        if public_map.map_width() != obs.map_width || public_map.map_height() != obs.map_height {
            return None;
        }
        return Some(public_ground_connected(public_map, &starts, &goals));
    }

    let mut optimistic = RouteProjection::new(obs, Domain::Ground);
    if !starts
        .iter()
        .any(|start| goals.iter().any(|goal| optimistic.reaches(*start, *goal)))
    {
        return Some(false);
    }

    let mut routes = RouteProjection::known_ground(obs);
    starts
        .iter()
        .any(|start| goals.iter().any(|goal| routes.reaches(*start, *goal)))
        .then_some(true)
}

fn public_ground_connected(
    public_map: &PublicMapBriefing,
    starts: &[TilePos],
    goals: &[TilePos],
) -> bool {
    let cells = usize::try_from(public_map.map_width())
        .ok()
        .and_then(|width| {
            usize::try_from(public_map.map_height())
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .unwrap_or(0);
    let mut visited = vec![false; cells];
    let mut frontier = VecDeque::new();
    for start in starts {
        let index = (start.y * public_map.map_width() + start.x) as usize;
        if !visited[index] {
            visited[index] = true;
            frontier.push_back(*start);
        }
    }
    while let Some(tile) = frontier.pop_front() {
        if goals.contains(&tile) {
            return true;
        }
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let next = tile.offset(dx, dy);
            let Some(index) = public_map
                .terrain_at(next)
                .filter(|terrain| !terrain.blocks_ground())
                .map(|_| (next.y * public_map.map_width() + next.x) as usize)
            else {
                continue;
            };
            if !visited[index] {
                visited[index] = true;
                frontier.push_back(next);
            }
        }
    }
    false
}

fn operation_timeout(profile: &ResolvedProfile, plan: &AirPlan) -> Tick {
    3_200 + plan.assembly_timeout + u64::from(100u8.saturating_sub(profile.traits.air)) * 4
}

fn phase_timeout(phase: AirOperationPhase, plan: &AirPlan) -> Tick {
    match phase {
        AirOperationPhase::Recon => 900,
        AirOperationPhase::Assemble => plan.assembly_timeout,
        AirOperationPhase::SuppressAa => 1_400,
        AirOperationPhase::Verify => 900,
        AirOperationPhase::Strike => 1_200,
        AirOperationPhase::Recover => 500,
    }
}

fn cooldown(profile: &ResolvedProfile, tuning: DifficultyTuning) -> Tick {
    let base: Tick = match profile.stance {
        BotStance::Turtle => 900,
        BotStance::Balanced => 700,
        BotStance::Aggressive => 500,
    };
    base + u64::from(100u8.saturating_sub(profile.traits.air)) * 3 + tuning.commitment_hesitation
}

fn is_artillery(kind: UnitKind) -> bool {
    matches!(kind, UnitKind::Bombard | UnitKind::Avalanche)
}

fn is_strike_aircraft(kind: UnitKind, faction: crate::state::Faction) -> bool {
    kind == Role::AirGround.unit_for(faction) || kind == Role::Bomber.unit_for(faction)
}

fn staging(home: TilePos, target: TilePos) -> TilePos {
    TilePos::new(
        home.x + (target.x - home.x) / 3,
        home.y + (target.y - home.y) / 3,
    )
}

fn artillery_staging_candidates(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> Vec<TilePos> {
    let ideal = staging(home, target);
    let mut candidates = Vec::new();
    for radius in 0i32..=3 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) != radius {
                    continue;
                }
                let candidate = ideal.offset(dx, dy);
                if public_ground_open(obs, candidate, public_map) {
                    candidates.push(candidate);
                }
            }
        }
    }
    candidates
}

fn connected_artillery_staging_goal(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    let home_size = obs
        .my_buildings
        .iter()
        .find(|building| {
            building.built && building.kind == BuildingKind::Foundry && building.anchor == home
        })
        .map_or(BuildingKind::Foundry.base_stats().size, |building| {
            building.kind.base_stats().size
        });
    let starts: Vec<_> = crate::tick::rect_adjacent_tiles(home, home_size)
        .filter(|tile| public_ground_open(obs, *tile, public_map))
        .collect();
    let mut routes = route_projection(obs, Domain::Ground, public_map);
    artillery_staging_candidates(obs, home, target, public_map)
        .into_iter()
        .find(|candidate| {
            starts
                .iter()
                .any(|start| routes.reaches(*start, *candidate))
        })
}

/// Proves that the exact demanded artillery count can accept its eventual
/// authoritative group spread from the same component used by provider
/// admission. Live artillery and every eligible producer doorstep are already
/// required to reach `source_staging`, so this covers both existing and future
/// members without guessing where a not-yet-trained unit will stand.
fn connected_artillery_group_has_staging(
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
    let Some(source_staging) =
        connected_artillery_staging_goal(obs, route.home, route.target, route.public_map)
    else {
        return false;
    };
    let mut routes =
        route_projection_with_orientation(obs, Domain::Ground, route.public_map, route.orientation);
    let exact_live = exact_live_provider_group(obs, demands, preferred, unavailable);
    artillery_staging_candidates(obs, route.home, route.target, route.public_map)
        .into_iter()
        .any(|candidate| {
            artillery_group_reaches_staging(
                &mut routes,
                source_staging,
                candidate,
                count,
                exact_live.as_deref(),
            )
        })
}

fn connected_suppression_roster_has_firing_assignments(
    obs: &Observation,
    route: ConnectedRouteContext<'_>,
    demands: &[ProviderDemand],
    targets: &[Target],
) -> bool {
    if targets.is_empty() {
        return true;
    }
    let Some(staging) =
        connected_artillery_staging_goal(obs, route.home, route.target, route.public_map)
    else {
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
        && targets.iter().all(|target| {
            suppression_firing_assignment(
                obs,
                route.intel,
                &origins,
                *target,
                route.public_map,
                route.orientation,
            )
            .is_some()
        })
}

fn artillery_group_reaches_staging(
    routes: &mut RouteProjection<'_>,
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

fn exact_live_provider_group(
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtilleryStaging {
    Ready(TilePos),
    NeedsRecon(TilePos),
}

/// A nearby staging tile every assigned artillery unit can reach through the
/// bot's optimistic known ground. The raw one-third point can fall inside a
/// mapped gulf; searching only its local ring preserves the intended line and
/// aborts when the operation actually requires a ferry. An unexplored candidate
/// must be reconnoitered before a ground command treats that optimism as fact.
fn artillery_staging(
    op: &AirOperation,
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    public_map: Option<&PublicMapBriefing>,
    orientation: Orientation,
) -> Option<ArtilleryStaging> {
    let mut routes =
        route_projection_with_orientation(obs, Domain::Ground, public_map, orientation);
    for candidate in artillery_staging_candidates(obs, home, target, public_map) {
        if routes.group_reaches_command_goal(&op.artillery, candidate) {
            return Some(if obs.explored(candidate) {
                ArtilleryStaging::Ready(candidate)
            } else {
                ArtilleryStaging::NeedsRecon(candidate)
            });
        }
    }
    None
}

fn connected_public_map<'a>(
    plan: &AirPlan,
    public_map: Option<&'a PublicMapBriefing>,
) -> Option<&'a PublicMapBriefing> {
    if plan.airborne() { None } else { public_map }
}

fn route_projection<'a>(
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
) -> RouteProjection<'a> {
    public_map.map_or_else(
        || RouteProjection::new(obs, domain),
        |map| RouteProjection::with_public_terrain(obs, domain, map),
    )
}

fn route_projection_with_orientation<'a>(
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
) -> RouteProjection<'a> {
    public_map.map_or_else(
        || RouteProjection::with_orientation(obs, domain, orientation),
        |map| RouteProjection::with_public_terrain_and_orientation(obs, domain, map, orientation),
    )
}

fn operation_route_projection<'a>(
    plan: &AirPlan,
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
) -> RouteProjection<'a> {
    if plan.connected_package.is_some() {
        route_projection_with_orientation(obs, domain, public_map, orientation)
    } else {
        route_projection(obs, domain, public_map)
    }
}

fn public_ground_open(
    obs: &Observation,
    tile: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    routing::ground_open(obs, tile)
        && public_map.is_none_or(|map| {
            map.terrain_at(tile)
                .is_some_and(|terrain| !terrain.blocks_ground())
        })
}

fn elapsed(start: Tick, now: Tick) -> Tick {
    now.saturating_sub(start)
}

fn enter(op: &mut AirOperation, phase: AirOperationPhase, now: Tick) {
    if phase == AirOperationPhase::SuppressAa {
        op.membership_frozen_at.get_or_insert(now);
    }
    op.phase = phase;
    op.phase_started_at = now;
}

fn recover(op: &mut AirOperation, reason: AirRecoveryReason, now: Tick) {
    enter(op, AirOperationPhase::Recover, now);
    op.recovery_reason = Some(reason);
    op.scout_dispatch = None;
}

#[cfg(test)]
mod tests {
    use super::super::observation::BuildingObs;
    use super::super::profile::{PersonalityTraits, Specialty};
    use super::*;
    use crate::map::Terrain;
    use crate::scenario::{BotConfig, BotDifficulty};
    use crate::state::Faction;

    const HOME: TilePos = TilePos::new(3, 10);
    const TARGET: TilePos = TilePos::new(24, 10);

    trait AdjudicatedStrategicThink {
        fn think(
            &mut self,
            profile: &ResolvedProfile,
            tuning: DifficultyTuning,
            obs: &Observation,
            intel: &StrategicIntelligence,
            home: TilePos,
            enlisted: &[UnitId],
        ) -> StrategicDecision;

        fn think_with_lift_support(
            &mut self,
            profile: &ResolvedProfile,
            tuning: DifficultyTuning,
            obs: &Observation,
            intel: &StrategicIntelligence,
            home: TilePos,
            coordination: StrategicCoordination<'_>,
        ) -> StrategicDecision;

        fn think_through_adjudication(
            &mut self,
            profile: &ResolvedProfile,
            tuning: DifficultyTuning,
            obs: &Observation,
            intel: &StrategicIntelligence,
            home: TilePos,
            coordination: StrategicCoordination<'_>,
        ) -> StrategicThinkResult;
    }

    impl AdjudicatedStrategicThink for StrategicPlanner {
        fn think(
            &mut self,
            profile: &ResolvedProfile,
            tuning: DifficultyTuning,
            obs: &Observation,
            intel: &StrategicIntelligence,
            home: TilePos,
            enlisted: &[UnitId],
        ) -> StrategicDecision {
            <Self as AdjudicatedStrategicThink>::think_through_adjudication(
                self,
                profile,
                tuning,
                obs,
                intel,
                home,
                StrategicCoordination {
                    enlisted,
                    lift_support: None,
                    allow_new_operation: true,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                    public_map: None,
                    orientation: Orientation::for_home(obs, home),
                },
            )
            .decision
        }

        fn think_with_lift_support(
            &mut self,
            profile: &ResolvedProfile,
            tuning: DifficultyTuning,
            obs: &Observation,
            intel: &StrategicIntelligence,
            home: TilePos,
            coordination: StrategicCoordination<'_>,
        ) -> StrategicDecision {
            <Self as AdjudicatedStrategicThink>::think_through_adjudication(
                self,
                profile,
                tuning,
                obs,
                intel,
                home,
                coordination,
            )
            .decision
        }

        fn think_through_adjudication(
            &mut self,
            profile: &ResolvedProfile,
            tuning: DifficultyTuning,
            obs: &Observation,
            intel: &StrategicIntelligence,
            home: TilePos,
            coordination: StrategicCoordination<'_>,
        ) -> StrategicThinkResult {
            let resources = ResourceSnapshot::from_observation(obs);
            let request = FreshConnectedProposalRequest::new(
                profile,
                tuning,
                obs,
                &resources,
                intel,
                home,
                coordination,
            );
            let rejected_connected_candidate =
                match self.active_connected_revision_proposal(request) {
                    Ok(Some(mut proposal)) => {
                        if let Some(richest) = proposal.marginal_variants().last().cloned() {
                            assert!(proposal.select_marginal(&richest));
                        }
                        commit_test_connected_proposal(self, proposal);
                        None
                    }
                    Err(rejected) => {
                        self.reject_active_connected_revision(rejected.reason, obs.tick);
                        Some(rejected)
                    }
                    Ok(None) => match self.fresh_connected_minimum_proposal(
                        FreshConnectedProposalRequest::new(
                            profile,
                            tuning,
                            obs,
                            &resources,
                            intel,
                            home,
                            coordination,
                        ),
                    ) {
                        Ok(Some(proposal)) => {
                            commit_test_connected_proposal(self, proposal);
                            None
                        }
                        Ok(None) => None,
                        Err(rejected) => Some(rejected),
                    },
                };
            let mut result = self.think_after_connected_adjudication(StrategicThinkContext::new(
                profile,
                tuning,
                obs,
                intel,
                home,
                coordination,
            ));
            result.rejected_connected_candidate = rejected_connected_candidate;
            result
        }
    }

    fn profile() -> ResolvedProfile {
        ResolvedProfile {
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Balanced,
            personality_seed: 7,
            primary: Specialty::Air,
            secondary: Specialty::Siege,
            traits: PersonalityTraits {
                air: 70,
                siege: 60,
                support: 45,
                fortification: 35,
                greed: 45,
                guile: 45,
            },
        }
    }

    fn planning_context<'a>(
        identity: &'a ResolvedProfile,
        observation: &'a Observation,
        intelligence: &'a StrategicIntelligence,
    ) -> AirPlanningContext<'a> {
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.anchor == TARGET)
            .expect("the fixture has a current strategic target");
        AirPlanningContext {
            profile: identity,
            tuning: DifficultyTuning::for_level(identity.difficulty),
            obs: observation,
            intel: intelligence,
            home: HOME,
            orientation: test_orientation(),
            public_map: None,
            enlisted: &[],
            landing_sites: &[],
            connected_resources: Some(ConnectedProductionResources::from_observation(
                observation,
                target,
                &[],
                ConnectedRouteContext {
                    intel: intelligence,
                    home: HOME,
                    target: target.anchor,
                    public_map: None,
                    orientation: test_orientation(),
                },
            )),
            production: StrategicProductionContext::empty(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        }
    }

    fn obs(tick: Tick) -> Observation {
        Observation {
            tick,
            map_width: 32,
            map_height: 20,
            my_units: vec![
                own(1, UnitKind::Kestrel, TilePos::new(22, 10)),
                own(2, UnitKind::Bombard, TilePos::new(10, 10)),
                own(3, UnitKind::Condor, TilePos::new(4, 9)),
                own(4, UnitKind::Condor, TilePos::new(4, 11)),
            ],
            enemy_buildings: vec![building(80, 1, BuildingKind::Crucible, TARGET, true)],
            visible: vec![false; 32 * 20],
            explored: vec![false; 32 * 20],
            ..Observation::default()
        }
    }

    fn own(id: u32, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
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
        seen: bool,
    ) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(player),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen,
            tier: 0,
        }
    }

    fn operation(phase: AirOperationPhase, tick: Tick) -> AirOperation {
        AirOperation {
            target_player: PlayerId(1),
            target_kind: BuildingKind::Crucible,
            target: TARGET,
            target_id: Some(BuildingId(80)),
            assault_admitted: true,
            phase,
            started_at: tick - 50,
            phase_started_at: tick - 50,
            scout: Some(UnitId(1)),
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery: vec![UnitId(2)],
            strike_aircraft: vec![UnitId(3), UnitId(4)],
            strike_issued_at: None,
            membership_frozen_at: matches!(
                phase,
                AirOperationPhase::SuppressAa
                    | AirOperationPhase::Verify
                    | AirOperationPhase::Strike
            )
            .then_some(tick),
            recovery_reason: None,
        }
    }

    fn connected_test_plan(observation: &Observation) -> AirPlan {
        let faction = observation.faction;
        let scout = Role::Scout.unit_for(faction);
        let strike = Role::Bomber.unit_for(faction);
        AirPlan::connected(
            ConnectedForcePackage {
                derived_at: observation.tick,
                preparation_deadline: observation
                    .tick
                    .saturating_add(CONNECTED_PREPARATION_HORIZON),
                target_anchors: vec![TARGET],
                recon: vec![ProviderDemand {
                    kind: scout,
                    count: 1,
                }],
                suppression: vec![ProviderDemand {
                    kind: UnitKind::Bombard,
                    count: 1,
                }],
                strike: vec![ProviderDemand {
                    kind: strike,
                    count: 2,
                }],
                provider_priority: vec![
                    force_package::ProviderDemandTranche {
                        priority: force_package::ProviderPriority::Minimum,
                        family: ForceFamily::Recon,
                        kind: scout,
                        count: 1,
                    },
                    force_package::ProviderDemandTranche {
                        priority: force_package::ProviderPriority::Minimum,
                        family: ForceFamily::Suppression,
                        kind: UnitKind::Bombard,
                        count: 1,
                    },
                    force_package::ProviderDemandTranche {
                        priority: force_package::ProviderPriority::Minimum,
                        family: ForceFamily::Strike,
                        kind: strike,
                        count: 1,
                    },
                    force_package::ProviderDemandTranche {
                        priority: force_package::ProviderPriority::Marginal,
                        family: ForceFamily::Strike,
                        kind: strike,
                        count: 1,
                    },
                ],
                funded_providers: Vec::new(),
                producer_assignments: None,
                minimum_capability: NormalizedCapability {
                    recon: 1_000,
                    suppression: suppression_capability(UnitKind::Bombard, faction),
                    strike: strike_capability(strike, faction),
                },
                useful_capability: NormalizedCapability {
                    recon: 1_000,
                    suppression: suppression_capability(UnitKind::Bombard, faction),
                    strike: strike_capability(strike, faction).saturating_mul(2),
                },
                useful_bombing: 0,
                target_value: 1,
                current_scrap: observation.scrap,
                observed_aa_firepower: 0,
                suppressible_aa_firepower: 0,
                forecast_scrap: 0,
                chosen_capability: NormalizedCapability {
                    recon: 1_000,
                    suppression: suppression_capability(UnitKind::Bombard, faction),
                    strike: strike_capability(strike, faction).saturating_mul(2),
                },
                chosen_bombing: 0,
            },
            observation.tick,
            TARGET,
        )
    }

    fn derived_connected_test_plan(
        identity: &ResolvedProfile,
        observation: &Observation,
    ) -> Option<AirPlan> {
        let intelligence = knowledge(observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|building| building.evidence == ContactEvidence::Current)?;
        let resources = ConnectedProductionResources::from_observation(
            observation,
            target,
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home: HOME,
                target: target.anchor,
                public_map: None,
                orientation: test_orientation(),
            },
        );
        connected_plan(
            identity,
            observation,
            &intelligence,
            HOME,
            target,
            &[],
            ConnectedPlanningContext {
                orientation: test_orientation(),
                public_map: None,
                resources: &resources,
                preferred_artillery: &[],
                protected_current_scrap: 0,
                preparation: PreparationConstraints {
                    deadline: observation
                        .tick
                        .saturating_add(CONNECTED_PREPARATION_HORIZON),
                    decision_cadence: DifficultyTuning::for_level(identity.difficulty).cadence,
                    protected_forecast_scrap: 0,
                },
            },
        )
        .ok()
    }

    fn see_approach(obs: &mut Observation) {
        see_approach_to(obs, TARGET);
    }

    fn see_approach_to(obs: &mut Observation, target: TilePos) {
        for tile in approach(HOME, target) {
            let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
            obs.visible[index] = true;
            obs.explored[index] = true;
        }
    }

    fn see_building_footprint(obs: &mut Observation, anchor: TilePos, kind: BuildingKind) {
        let (width, height) = kind.base_stats().size;
        for dy in 0..height {
            for dx in 0..width {
                let tile = anchor.offset(dx, dy);
                let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
                obs.visible[index] = true;
                obs.explored[index] = true;
            }
        }
    }

    fn explore(obs: &mut Observation, tile: TilePos) {
        let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
        obs.explored[index] = true;
    }

    fn public_map_with_terrain(
        observation: &Observation,
        terrain: impl IntoIterator<Item = (TilePos, Terrain)>,
    ) -> PublicMapBriefing {
        let mut non_ground_terrain: Vec<_> = terrain.into_iter().collect();
        non_ground_terrain.sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
        PublicMapBriefing {
            map_width: observation.map_width,
            map_height: observation.map_height,
            starting_foundries: Vec::new(),
            teams: vec![None, None],
            non_ground_terrain,
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        }
    }

    fn movement_cap_serpentine_map(observation: &Observation) -> PublicMapBriefing {
        assert_eq!((observation.map_width, observation.map_height), (256, 256));
        let width = observation.map_width;
        public_map_with_terrain(
            observation,
            (0..observation.map_height).flat_map(|y| {
                (0..width).filter_map(move |x| {
                    let tile = TilePos::new(x, y);
                    let open = y % 2 == 0
                        || (y % 4 == 1 && x == width - 1)
                        || (y % 4 == 3 && x == 0)
                        || tile == TilePos::new(255, 255);
                    (!open).then_some((tile, Terrain::Pit))
                })
            }),
        )
    }

    fn knowledge(obs: &Observation) -> StrategicIntelligence {
        let mut intel = StrategicIntelligence::new();
        intel.update(obs);
        intel
    }

    fn with_operation(phase: AirOperationPhase, tick: Tick) -> StrategicPlanner {
        let observation = obs(tick);
        StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation(phase, tick),
                plan: connected_test_plan(&observation),
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        }
    }

    fn wealthy_airborne_operation(
        phase: AirOperationPhase,
        mobile_aa: UnitKind,
        mobile_aa_count: usize,
    ) -> (Observation, StrategicPlanner) {
        let mut battle = wealthy_island_obs(5_000, 2);
        battle.faction = Faction::Cupric;
        battle.my_units = vec![own(1, UnitKind::Gnat, TARGET.offset(-2, 0))];
        battle
            .my_units
            .extend((0..10).map(|index| own(100 + index, UnitKind::Moth, HOME.offset(0, 2))));
        battle
            .my_units
            .extend((0..5).map(|index| own(200 + index, UnitKind::Darter, HOME.offset(1, 2))));
        battle.enemy_units.extend((0..mobile_aa_count).map(|index| {
            let mut unit = own(
                300 + u32::try_from(index).unwrap(),
                mobile_aa,
                TARGET.offset(-2, i32::try_from(index % 3).unwrap() - 1),
            );
            unit.player = PlayerId(1);
            unit
        }));
        battle.my_units.sort_unstable_by_key(|unit| unit.id);
        battle.enemy_units.sort_unstable_by_key(|unit| unit.id);
        see_approach(&mut battle);

        let mut operation = operation(phase, battle.tick);
        operation.artillery.clear();
        operation.scout = Some(UnitId(1));
        operation.strike_aircraft = (100..110).map(UnitId).collect();
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 10;
        plan.desired_screen = 5;
        plan.screen = (200..205).map(UnitId).collect();
        let planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        (battle, planner)
    }

    fn wealthy_island_obs(tick: Tick, airworks: usize) -> Observation {
        let mut observation = obs(tick);
        observation.scrap = 50_000;
        observation
            .my_units
            .extend((5..=16).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
        observation.my_buildings = vec![
            building(20, 0, BuildingKind::Foundry, HOME, true),
            building(21, 0, BuildingKind::Reclaimer, TilePos::new(5, 4), true),
            building(22, 0, BuildingKind::Reclaimer, TilePos::new(7, 4), true),
        ];
        observation.my_buildings.extend((0..airworks).map(|index| {
            building(
                23 + u32::try_from(index).unwrap(),
                0,
                BuildingKind::Airworks,
                TilePos::new(3 + i32::try_from(index).unwrap() * 3, 15),
                true,
            )
        }));
        observation.my_buildings.push(building(
            30,
            0,
            BuildingKind::Crucible,
            TilePos::new(10, 15),
            true,
        ));
        observation.my_queues = vec![Vec::new(); observation.my_buildings.len()];
        observation.known_rock = (0..observation.map_height)
            .map(|y| TilePos::new(16, y))
            .collect();
        observation
    }

    fn developed_connected_obs(tick: Tick) -> Observation {
        let mut observation = obs(tick);
        observation.visible.fill(true);
        observation.explored.fill(true);
        observation.scrap = 10_000;
        observation
            .my_units
            .extend((5..=13).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        observation.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
            building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
        ];
        observation.my_queues = vec![Vec::new(); observation.my_buildings.len()];
        observation
    }

    fn production_hungry_connected_obs(tick: Tick, scrap: u32) -> Observation {
        let mut observation = developed_connected_obs(tick);
        observation.my_units.retain(|unit| {
            !matches!(
                unit.kind,
                UnitKind::Kestrel | UnitKind::Bombard | UnitKind::Condor
            )
        });
        observation
            .my_units
            .extend((14..=17).map(|id| own(id, UnitKind::Sentinel, TilePos::new(8, 10))));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        observation.scrap = scrap;
        assert!(combat_roster(&observation) >= CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER);
        observation
    }

    fn add_renewable_economy(observation: &mut Observation, count: usize) {
        for index in 0..count {
            observation.my_buildings.push(building(
                1_000 + u32::try_from(index).unwrap(),
                0,
                BuildingKind::Reclaimer,
                TilePos::new(
                    1 + i32::try_from(index % 4).unwrap() * 3,
                    1 + i32::try_from(index / 4).unwrap() * 3,
                ),
                true,
            ));
            observation.my_queues.push(Vec::new());
        }
    }

    fn think(
        planner: &mut StrategicPlanner,
        obs: &Observation,
        intel: &StrategicIntelligence,
    ) -> StrategicDecision {
        planner.think(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            obs,
            intel,
            HOME,
            &[],
        )
    }

    fn coordination(lift_support: Option<&LiftSupportRequest>) -> StrategicCoordination<'_> {
        StrategicCoordination {
            enlisted: &[],
            lift_support,
            allow_new_operation: true,
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
            public_map: None,
            orientation: test_orientation(),
        }
    }

    fn selected_connected_assignments(
        proposal: &FreshConnectedProposal,
    ) -> Vec<ConnectedProducerAssignment> {
        let identity = proposal.identity();
        let jobs = proposal.variants[proposal.selected_variant]
            .claims
            .provider_jobs
            .clone();
        let mut lane_ready = Vec::<(BuildingId, Tick)>::new();
        jobs.iter()
            .enumerate()
            .map(|(request_ordinal, job)| {
                let producer = job.eligible_producers[0];
                let lane_index = lane_ready
                    .iter()
                    .position(|(candidate, _)| *candidate == producer)
                    .unwrap_or_else(|| {
                        lane_ready.push((producer, job.enqueue_not_before));
                        lane_ready.len() - 1
                    });
                let starts_at = lane_ready[lane_index].1.max(job.enqueue_not_before);
                let ready_at = starts_at
                    .saturating_add(Tick::from(job.kind.stats().train_ticks))
                    .saturating_sub(1);
                assert!(ready_at < job.ready_before);
                lane_ready[lane_index].1 = ready_at.saturating_add(1);
                ConnectedProducerAssignment::new(
                    identity,
                    request_ordinal,
                    producer,
                    job.kind,
                    ConnectedProducerTiming::new(
                        job.enqueue_not_before,
                        starts_at,
                        ready_at,
                        job.ready_before,
                    ),
                    ConnectedProducerFunding::new(job.kind.stats().cost, 0),
                )
            })
            .collect::<Vec<_>>()
    }

    fn revision_job(kind: UnitKind) -> ConnectedProviderJob {
        ConnectedProviderJob {
            kind,
            enqueue_not_before: 120,
            ready_before: 2_400,
            eligible_producers: vec![BuildingId(10), BuildingId(11)],
        }
    }

    fn revision_assignment(
        owner: ConnectedOffenseIdentity,
        request_ordinal: usize,
        producer: u32,
        kind: UnitKind,
    ) -> ConnectedProducerAssignment {
        let starts_at = 120 + Tick::try_from(request_ordinal).unwrap() * 200;
        ConnectedProducerAssignment::new(
            owner,
            request_ordinal,
            BuildingId(producer),
            kind,
            ConnectedProducerTiming::new(
                starts_at,
                starts_at,
                starts_at + Tick::from(kind.stats().train_ticks) - 1,
                2_400,
            ),
            ConnectedProducerFunding::new(kind.stats().cost, 0),
        )
    }

    #[test]
    fn active_revision_delta_retains_duplicate_kinds_and_never_reuses_issued_ordinals() {
        let old_identity = ConnectedOffenseIdentity::new(BuildingId(80), TARGET);
        let new_identity = ConnectedOffenseIdentity::new(BuildingId(81), TARGET.offset(1, 0));
        let assignments = vec![
            revision_assignment(old_identity, 2, 10, UnitKind::Buzzard),
            revision_assignment(old_identity, 5, 11, UnitKind::Bombard),
            revision_assignment(old_identity, 7, 12, UnitKind::Buzzard),
        ];
        let minimum = vec![revision_job(UnitKind::Buzzard)];
        let middle = vec![
            revision_job(UnitKind::Buzzard),
            revision_job(UnitKind::Condor),
        ];
        let largest = vec![
            revision_job(UnitKind::Buzzard),
            revision_job(UnitKind::Condor),
            revision_job(UnitKind::Buzzard),
        ];
        let variants = [minimum.as_slice(), middle.as_slice(), largest.as_slice()];

        let delta =
            reconcile_connected_revision_provider_jobs(new_identity, &assignments, &[5], &variants);

        assert_eq!(delta.required_variant_index(), 2);
        assert!(delta.pruned().is_empty());
        assert_eq!(
            delta
                .jobs()
                .iter()
                .map(|job| (
                    job.proposal_ordinal(),
                    job.binding_ordinal(),
                    job.request().kind(),
                    job.retained().map(|assignment| assignment.producer()),
                ))
                .collect::<Vec<_>>(),
            vec![
                (0, 2, UnitKind::Buzzard, Some(BuildingId(10))),
                (1, 8, UnitKind::Condor, None),
                (2, 7, UnitKind::Buzzard, Some(BuildingId(12))),
            ]
        );
        assert!(
            delta
                .jobs()
                .iter()
                .filter_map(|job| job.retained())
                .all(|assignment| assignment.owner == new_identity)
        );
    }

    #[test]
    fn active_revision_delta_prunes_only_absent_work_and_keeps_growth_separate() {
        let identity = ConnectedOffenseIdentity::new(BuildingId(80), TARGET);
        let assignments = vec![
            revision_assignment(identity, 0, 10, UnitKind::Kestrel),
            revision_assignment(identity, 1, 11, UnitKind::Bombard),
            revision_assignment(identity, 2, 12, UnitKind::Condor),
        ];
        let minimum = vec![revision_job(UnitKind::Kestrel)];
        let retained_scale = vec![
            revision_job(UnitKind::Kestrel),
            revision_job(UnitKind::Condor),
        ];
        let useful_scale = vec![
            revision_job(UnitKind::Kestrel),
            revision_job(UnitKind::Condor),
            revision_job(UnitKind::Buzzard),
        ];
        let variants = [
            minimum.as_slice(),
            retained_scale.as_slice(),
            useful_scale.as_slice(),
        ];

        let delta =
            reconcile_connected_revision_provider_jobs(identity, &assignments, &[], &variants);

        assert_eq!(delta.required_variant_index(), 1);
        assert_eq!(
            delta
                .pruned()
                .iter()
                .map(|assignment| assignment.request_ordinal())
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            delta
                .jobs()
                .iter()
                .map(|job| (job.binding_ordinal(), job.retained().is_some()))
                .collect::<Vec<_>>(),
            vec![(0, true), (2, true), (3, false)]
        );
    }

    #[test]
    fn active_revision_delta_keeps_prefix_identity_when_useful_scale_grows() {
        let identity = ConnectedOffenseIdentity::new(BuildingId(80), TARGET);
        let assignments = vec![revision_assignment(identity, 4, 10, UnitKind::Kestrel)];
        let minimum = vec![revision_job(UnitKind::Kestrel)];
        let middle = vec![
            revision_job(UnitKind::Kestrel),
            revision_job(UnitKind::Bombard),
        ];
        let richer = vec![
            revision_job(UnitKind::Kestrel),
            revision_job(UnitKind::Bombard),
            revision_job(UnitKind::Condor),
        ];
        let shorter = [minimum.as_slice(), middle.as_slice()];
        let longer = [minimum.as_slice(), middle.as_slice(), richer.as_slice()];

        let before =
            reconcile_connected_revision_provider_jobs(identity, &assignments, &[], &shorter);
        let after =
            reconcile_connected_revision_provider_jobs(identity, &assignments, &[], &longer);

        assert_eq!(
            before.required_variant_index(),
            after.required_variant_index()
        );
        assert_eq!(before.jobs(), &after.jobs()[..before.jobs().len()]);
        assert_eq!(
            after.jobs()[2].binding_ordinal(),
            before.jobs()[1].binding_ordinal() + 1
        );
    }

    #[test]
    fn active_revision_replays_fixed_lanes_chronologically_without_reordering_the_package() {
        let identity = ConnectedOffenseIdentity::new(BuildingId(80), TARGET);
        let mut assignments = (1..=4)
            .map(|ordinal| revision_assignment(identity, ordinal, 10, UnitKind::Buzzard))
            .collect::<Vec<_>>();
        assignments[3].timing = ConnectedProducerTiming::new(620, 620, 799, 2_400);
        let jobs = (0..4)
            .map(|_| revision_job(UnitKind::Buzzard))
            .collect::<Vec<_>>();
        let variants = [jobs.as_slice()];

        let delta =
            reconcile_connected_revision_provider_jobs(identity, &assignments, &[], &variants);

        assert_eq!(
            delta
                .jobs()
                .iter()
                .map(ConnectedRevisionProviderJob::binding_ordinal)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4],
            "the package keeps deterministic kind-occurrence identity"
        );
        assert_eq!(
            delta.allocation_order(4),
            vec![0, 1, 3, 2],
            "the accepted lane must replay by start time, not stable ordinal"
        );
    }

    #[test]
    fn active_revision_invalidates_orders_for_every_reassigned_provider_role() {
        let mut operation = operation(AirOperationPhase::Recon, 120);
        operation.scout_dispatch = Some((operation.scout.unwrap(), TARGET));
        operation.artillery_staging = Some(HOME);
        operation.strike_hold = Some(HOME);
        let previous_scout = operation.scout;
        let previous_artillery = operation.artillery.clone();
        let previous_strike_aircraft = operation.strike_aircraft.clone();
        operation.scout = Some(UnitId(10));
        operation.artillery = vec![UnitId(11)];
        operation.strike_aircraft = vec![UnitId(12)];

        invalidate_reassigned_member_orders(
            &mut operation,
            previous_scout,
            &previous_artillery,
            &previous_strike_aircraft,
        );

        assert_eq!(operation.scout_dispatch, None);
        assert_eq!(operation.artillery_staging, None);
        assert_eq!(operation.strike_hold, None);
    }

    #[test]
    fn active_revision_preserves_orders_when_provider_membership_is_unchanged() {
        let mut operation = operation(AirOperationPhase::Recon, 120);
        let scout_dispatch = Some((operation.scout.unwrap(), TARGET));
        let artillery_staging = Some(HOME);
        let strike_hold = Some(HOME);
        operation.scout_dispatch = scout_dispatch;
        operation.artillery_staging = artillery_staging;
        operation.strike_hold = strike_hold;
        let previous_scout = operation.scout;
        let previous_artillery = operation.artillery.clone();
        let previous_strike_aircraft = operation.strike_aircraft.clone();

        invalidate_reassigned_member_orders(
            &mut operation,
            previous_scout,
            &previous_artillery,
            &previous_strike_aircraft,
        );

        assert_eq!(operation.scout_dispatch, scout_dispatch);
        assert_eq!(operation.artillery_staging, artillery_staging);
        assert_eq!(operation.strike_hold, strike_hold);
    }

    fn commit_test_connected_proposal(
        planner: &mut StrategicPlanner,
        mut proposal: FreshConnectedProposal,
    ) {
        let assignments = selected_connected_assignments(&proposal);
        proposal
            .bind_producer_assignments(assignments)
            .expect("the fixture binds the proposed package's exact jobs");
        planner
            .commit_connected_proposal(proposal)
            .expect("the unchanged planner accepts its own proposal");
    }

    fn test_orientation() -> Orientation {
        Orientation::for_home(&obs(0), TilePos::new(0, 0))
    }

    #[test]
    fn output_and_persistence_are_deterministic() {
        let mut obs = obs(100);
        see_approach(&mut obs);
        let intel = knowledge(&obs);
        let mut first = with_operation(AirOperationPhase::Verify, 100);
        let mut second = first.clone();
        assert_eq!(
            think(&mut first, &obs, &intel),
            think(&mut second, &obs, &intel)
        );
        assert_eq!(first, second);
    }

    #[test]
    fn active_operation_keeps_members_already_claimed_by_the_coordinator() {
        let mut observation = obs(100);
        see_approach(&mut observation);
        let intelligence = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Recon, observation.tick);
        let owned = planner
            .air
            .as_ref()
            .map(|active| reservations(&active.op, &active.plan, &observation))
            .expect("the fixture has an active operation");

        let decision = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            StrategicCoordination {
                enlisted: &owned,
                ..coordination(None)
            },
        );

        let operation = planner
            .air_operation()
            .expect("the active operation remains owned");
        assert_eq!(operation.scout, Some(UnitId(1)));
        assert_eq!(operation.artillery, [UnitId(2)]);
        assert_eq!(operation.strike_aircraft, [UnitId(3), UnitId(4)]);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Kestrel | UnitKind::Bombard | UnitKind::Condor,
                ..
            }
        )));
    }

    #[test]
    fn provider_visible_on_the_deadline_can_complete_assembly() {
        let mut observation = obs(2_500);
        see_approach(&mut observation);
        observation.explored.fill(true);
        let intelligence = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Assemble, observation.tick);
        let active = planner.air.as_mut().expect("the fixture has an operation");
        active.op.strike_aircraft.pop();
        let package = active
            .plan
            .connected_package
            .as_mut()
            .expect("the fixture has a connected package");
        package.derived_at = observation.tick - 1;
        package.preparation_deadline = observation.tick;
        active.plan.admitted_at = observation.tick - CONNECTED_PREPARATION_HORIZON;
        active.plan.assembly_timeout = CONNECTED_PREPARATION_HORIZON;
        active.op.started_at = observation.tick - CONNECTED_PREPARATION_HORIZON;
        active.op.phase_started_at = observation.tick - CONNECTED_PREPARATION_HORIZON;

        let decision = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination(None),
        );

        let operation = planner
            .air_operation()
            .expect("the complete package crosses the commitment boundary");
        assert_eq!(operation.phase, AirOperationPhase::SuppressAa);
        assert_eq!(operation.membership_frozen_at, Some(observation.tick));
        assert_eq!(operation.strike_aircraft, [UnitId(3), UnitId(4)]);
        assert_ne!(operation.recovery_reason, Some(AirRecoveryReason::Timeout));
        assert!(decision.reservations.contains(&UnitId(4)));
    }

    #[test]
    fn provider_first_visible_on_the_deadline_survives_the_recon_transition() {
        let mut observation = obs(2_500);
        see_approach(&mut observation);
        observation.explored.fill(true);
        let mut intelligence = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Recon, observation.tick);
        let active = planner.air.as_mut().expect("the fixture has an operation");
        active.op.strike_aircraft.pop();
        let package = active
            .plan
            .connected_package
            .as_mut()
            .expect("the fixture has a connected package");
        package.derived_at = observation.tick - 1;
        package.preparation_deadline = observation.tick;
        active.plan.admitted_at = observation.tick - CONNECTED_PREPARATION_HORIZON;
        active.plan.assembly_timeout = CONNECTED_PREPARATION_HORIZON;
        active.op.started_at = observation.tick - CONNECTED_PREPARATION_HORIZON;
        active.op.phase_started_at = observation.tick - CONNECTED_PREPARATION_HORIZON;

        let deadline_decision = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination(None),
        );

        let operation = planner
            .air_operation()
            .expect("a complete deadline roster is retained after reconnaissance");
        assert_eq!(operation.phase, AirOperationPhase::Assemble);
        assert_eq!(operation.strike_aircraft, [UnitId(3), UnitId(4)]);
        assert_ne!(operation.recovery_reason, Some(AirRecoveryReason::Timeout));
        assert!(deadline_decision.reservations.contains(&UnitId(4)));

        observation.tick += DifficultyTuning::for_level(BotDifficulty::Prime).cadence;
        intelligence.update(&observation);
        let committed = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination(None),
        );

        let operation = planner
            .air_operation()
            .expect("the ready roster crosses commitment on the next decision boundary");
        assert_eq!(operation.phase, AirOperationPhase::SuppressAa);
        assert_eq!(operation.membership_frozen_at, Some(observation.tick));
        assert!(committed.reservations.contains(&UnitId(4)));
    }

    #[test]
    fn suppression_commitment_tick_is_recorded_once_and_survives_recovery() {
        let mut operation = operation(AirOperationPhase::Assemble, 100);
        assert_eq!(operation.membership_frozen_at, None);

        enter(&mut operation, AirOperationPhase::SuppressAa, 120);
        assert_eq!(operation.membership_frozen_at, Some(120));
        enter(&mut operation, AirOperationPhase::Verify, 130);
        enter(&mut operation, AirOperationPhase::SuppressAa, 140);
        assert_eq!(operation.membership_frozen_at, Some(120));

        recover(&mut operation, AirRecoveryReason::Timeout, 150);
        assert_eq!(operation.membership_frozen_at, Some(120));
    }

    #[test]
    fn closed_admission_blocks_a_new_air_plan_but_an_active_plan_reaches_strike() {
        let eligible = wealthy_island_obs(
            super::super::difficulty::strategic_admission_at_or_after(5_000),
            2,
        );
        let eligible_intelligence = knowledge(&eligible);
        let mut blocked = StrategicPlanner::new();

        assert_eq!(
            blocked.think_with_lift_support(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &eligible,
                &eligible_intelligence,
                HOME,
                StrategicCoordination {
                    enlisted: &[],
                    lift_support: None,
                    allow_new_operation: false,
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                    public_map: None,
                    orientation: test_orientation(),
                },
            ),
            StrategicDecision::default()
        );
        assert!(blocked.air_operation().is_none());

        let mut battle = obs(100);
        see_approach(&mut battle);
        see_building_footprint(&mut battle, TARGET, BuildingKind::Crucible);
        let intelligence = knowledge(&battle);
        let mut active = with_operation(AirOperationPhase::Verify, battle.tick);
        let continued = active.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intelligence,
            HOME,
            StrategicCoordination {
                enlisted: &[],
                lift_support: None,
                allow_new_operation: false,
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
                public_map: None,
                orientation: test_orientation(),
            },
        );

        assert_eq!(
            active.air_operation().map(|operation| operation.phase),
            Some(AirOperationPhase::Strike)
        );
        assert_eq!(continued.committed_scrap, 0);
        assert_eq!(
            continued.reservations,
            [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
        );
        assert!(
            continued
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
        );
        let mut incomplete = obs(200);
        incomplete.scrap = 50_000;
        incomplete.my_units.retain(|unit| unit.id == UnitId(1));
        incomplete.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
            building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
        ];
        incomplete.my_queues = vec![Vec::new(); incomplete.my_buildings.len()];
        see_approach(&mut incomplete);
        let incomplete_intelligence = knowledge(&incomplete);
        let mut assembling = with_operation(AirOperationPhase::Assemble, incomplete.tick);
        let held = assembling.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &incomplete,
            &incomplete_intelligence,
            HOME,
            StrategicCoordination {
                enlisted: &[],
                lift_support: None,
                allow_new_operation: false,
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
                public_map: None,
                orientation: test_orientation(),
            },
        );
        assert_eq!(held.committed_scrap, 0);
        assert!(
            held.intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. })),
            "an active incomplete operation cannot replenish while spending is closed: {held:?}"
        );
        assert!(assembling.air_operation().is_some());

        let mut damaged = obs(300);
        damaged
            .my_units
            .retain(|unit| !matches!(unit.id, UnitId(3) | UnitId(4)));
        let damaged_intelligence = knowledge(&damaged);
        let mut recovering = with_operation(AirOperationPhase::SuppressAa, damaged.tick);
        let retreat = recovering.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &damaged,
            &damaged_intelligence,
            HOME,
            StrategicCoordination {
                enlisted: &[],
                lift_support: None,
                allow_new_operation: false,
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
                public_map: None,
                orientation: test_orientation(),
            },
        );
        assert!(
            retreat
                .intents
                .iter()
                .any(|intent| matches!(intent, Intent::MoveUnits { .. })),
            "closing purchases must preserve the active operation's recovery order: {retreat:?}"
        );
    }

    #[test]
    fn losing_a_dispatched_recon_scout_recovers_without_claiming_a_replacement() {
        let mut observation = obs(100);
        observation.my_units.remove(0);
        observation
            .my_units
            .push(own(5, UnitKind::Kestrel, TilePos::new(5, 10)));
        let intel = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Recon, 100);
        planner.air_op_mut().unwrap().scout_dispatch = Some((UnitId(1), TARGET));

        let decision = think(&mut planner, &observation, &intel);

        let operation = planner
            .air_operation()
            .expect("the lost-scout operation withdraws before closing");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::RequiredUnitLost)
        );
        assert!(planner.cooldown_until() > observation.tick);
        assert_eq!(decision.reservations, [UnitId(2), UnitId(3), UnitId(4)]);
        assert!(!decision.reservations.contains(&UnitId(5)));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, .. } if units.contains(&UnitId(5))
        )));
    }

    #[test]
    fn losing_a_dispatched_recon_scout_releases_its_factory_bank() {
        let mut observation = obs(100);
        observation.my_units.remove(0);
        observation.my_buildings = vec![building(
            10,
            0,
            BuildingKind::Airworks,
            TilePos::new(2, 2),
            true,
        )];
        observation.my_queues = vec![Vec::new()];
        observation.scrap = UnitKind::Kestrel.stats().cost;
        let intel = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Recon, 100);
        planner.air_op_mut().unwrap().scout_dispatch = Some((UnitId(1), TARGET));

        let decision = think(&mut planner, &observation, &intel);

        let operation = planner
            .air_operation()
            .expect("the lost-scout operation withdraws before closing");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::RequiredUnitLost)
        );
        assert_eq!(decision.committed_scrap, 0);
        assert!(
            decision
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
        );
    }

    #[test]
    fn a_naturally_dispatched_scout_loss_completes_recovery_before_using_a_replacement() {
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut observation = wealthy_island_obs(
            super::super::difficulty::strategic_admission_at_or_after(5_000),
            1,
        );
        observation.my_units[0].tile = HOME;
        observation
            .my_units
            .push(own(17, UnitKind::Kestrel, HOME.offset(1, 0)));
        let mut intelligence = knowledge(&observation);
        let mut planner = StrategicPlanner::new();

        let dispatched = think(&mut planner, &observation, &intelligence);
        let operation = planner
            .air_operation()
            .expect("the wealthy island starts a real air operation");
        assert_eq!(operation.phase, AirOperationPhase::Assemble);
        assert_eq!(operation.scout, Some(UnitId(1)));
        assert!(operation.scout_dispatch.is_some());
        assert!(dispatched.intents.iter().any(|intent| matches!(
            intent,
            Intent::MoveUnits { units, .. } if units == &[UnitId(1)]
        )));
        assert!(
            dispatched.committed_scrap > 0,
            "the live operation must own a real factory bank before the loss"
        );

        observation.tick += 1;
        observation.my_units.retain(|unit| unit.id != UnitId(1));
        intelligence.update(&observation);
        let failed = think(&mut planner, &observation, &intelligence);
        let operation = planner
            .air_operation()
            .expect("the failed operation remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::RequiredUnitLost)
        );
        assert_eq!(failed.committed_scrap, 0);
        assert!(!failed.reservations.contains(&UnitId(17)));
        assert_eq!(
            failed
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::MoveUnits { units, goal }
                        if units == &failed.reservations && *goal == HOME
                ))
                .count(),
            1,
            "surviving claimed units receive one recovery order"
        );
        assert!(failed.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, .. } if units.contains(&UnitId(17))
        )));
        assert!(failed.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Kestrel,
                ..
            }
        )));
        let cooldown_until = planner.cooldown_until();
        assert_eq!(
            cooldown_until,
            observation
                .tick
                .saturating_add(cooldown(&profile(), tuning))
        );

        observation.tick += 1;
        intelligence.update(&observation);
        let recovered = think(&mut planner, &observation, &intelligence);
        assert!(
            recovered.intents.is_empty(),
            "the return order is sent once"
        );
        assert!(planner.air_operation().is_none());
        assert_eq!(planner.terminal_outcome(), None);
        let standby = planner.standby.reservations();
        assert!(!standby.is_empty());
        assert!(!standby.contains(&UnitId(17)));

        observation.tick += 1;
        intelligence.update(&observation);
        let cooling_down = think(&mut planner, &observation, &intelligence);
        assert!(cooling_down.intents.is_empty());
        assert_eq!(cooling_down.reservations, standby);
        assert!(planner.air_operation().is_none());
        assert_eq!(planner.terminal_outcome(), None);

        observation.tick =
            super::super::difficulty::strategic_admission_at_or_after(cooldown_until);
        assert!(observation.tick >= cooldown_until);
        assert!(
            observation.tick
                < cooldown_until
                    .saturating_add(super::super::difficulty::STRATEGIC_ADMISSION_CADENCE)
        );
        intelligence.update(&observation);
        let retried = think(&mut planner, &observation, &intelligence);
        let retry = planner
            .air_operation()
            .expect("a fresh operation may use the replacement after cooldown");
        assert_eq!(retry.scout, Some(UnitId(17)));
        assert!(retry.scout_dispatch.is_some());
        assert!(
            standby
                .iter()
                .all(|unit| retry.strike_aircraft.contains(unit))
        );
        assert!(retried.intents.iter().any(|intent| matches!(
            intent,
            Intent::MoveUnits { units, .. } if units == &[UnitId(17)]
        )));
    }

    #[test]
    fn scout_dispatch_retargets_only_when_its_safe_goal_changes() {
        let mut observation = obs(100);
        observation.my_units[0].tile = TilePos::new(4, 10);
        observation.my_units[0].idle = false;
        let intel = knowledge(&observation);
        let mut operation = operation(AirOperationPhase::Recon, 100);
        let plan = AirPlan::remembered_connected(&observation);

        let mut first = StrategicDecision::default();
        assert!(dispatch_scout(
            &mut operation,
            &plan,
            &observation,
            &intel,
            &[],
            None,
            &mut first
        ));
        let first_goal = match first.intents.as_slice() {
            [Intent::MoveUnits { units, goal }] if units == &[UnitId(1)] => *goal,
            intents => panic!("expected one scout dispatch, got {intents:?}"),
        };

        let mut repeated = StrategicDecision::default();
        assert!(dispatch_scout(
            &mut operation,
            &plan,
            &observation,
            &intel,
            &[],
            None,
            &mut repeated
        ));
        assert!(
            repeated.intents.is_empty(),
            "an identical in-flight order remains authoritative"
        );

        operation.target = TilePos::new(24, 16);
        let mut changed = StrategicDecision::default();
        assert!(dispatch_scout(
            &mut operation,
            &plan,
            &observation,
            &intel,
            &[],
            None,
            &mut changed
        ));
        assert!(matches!(
            changed.intents.as_slice(),
            [Intent::MoveUnits { units, goal }]
                if units == &[UnitId(1)] && *goal != first_goal
        ));
    }

    #[test]
    fn strike_hold_is_dispatched_once_until_home_changes() {
        let observation = obs(100);
        let mut operation = operation(AirOperationPhase::SuppressAa, 100);
        let pad = landing_pad(&observation, HOME).expect("open ground rings the home anchor");
        assert_eq!(
            pad,
            HOME.offset(-2, -2),
            "the pad is the first ring-two tile by (y, x)"
        );

        let mut first = StrategicDecision::default();
        hold_strike_aircraft(&mut operation, &observation, HOME, &mut first);
        assert_eq!(
            first.intents,
            [Intent::MoveUnits {
                units: vec![UnitId(3), UnitId(4)],
                goal: pad,
            }]
        );
        assert_eq!(operation.strike_hold, Some(pad));

        let mut repeated = StrategicDecision::default();
        hold_strike_aircraft(&mut operation, &observation, HOME, &mut repeated);
        assert!(
            repeated.intents.is_empty(),
            "the stable hold remains authoritative"
        );

        let replacement_home = HOME.offset(2, 1);
        let replacement_pad = landing_pad(&observation, replacement_home).unwrap();
        assert_ne!(replacement_pad, pad);
        let mut redirected = StrategicDecision::default();
        hold_strike_aircraft(
            &mut operation,
            &observation,
            replacement_home,
            &mut redirected,
        );
        assert_eq!(
            redirected.intents,
            [Intent::MoveUnits {
                units: vec![UnitId(3), UnitId(4)],
                goal: replacement_pad,
            }]
        );

        let mut replacement_repeated = StrategicDecision::default();
        hold_strike_aircraft(
            &mut operation,
            &observation,
            replacement_home,
            &mut replacement_repeated,
        );
        assert!(replacement_repeated.intents.is_empty());
    }

    #[test]
    fn landing_pad_skips_footprints_known_rock_and_the_map_edge() {
        let mut observation = obs(100);
        let foundry_size = BuildingKind::Foundry.base_stats().size;
        let inside = |tile: TilePos| {
            tile.x >= HOME.x
                && tile.x < HOME.x + foundry_size.0
                && tile.y >= HOME.y
                && tile.y < HOME.y + foundry_size.1
        };
        observation.my_buildings = vec![building(20, 0, BuildingKind::Foundry, HOME, true)];
        observation.my_queues = vec![Vec::new()];
        // Rock every ring-two tile outside the footprint so the only
        // ring-two candidates left are under the Foundry itself.
        observation.known_rock = (-2..=2)
            .flat_map(|dy| (-2..=2).map(move |dx| HOME.offset(dx, dy)))
            .filter(|tile| tile.chebyshev(HOME) == 2 && !inside(*tile))
            .collect();
        observation.known_rock.sort_by_key(|tile| (tile.y, tile.x));

        let pad = landing_pad(&observation, HOME).expect("ring three is open");
        assert!(!inside(pad), "the pad must never sit under the Foundry");
        assert!(!observation.known_rock_at(pad));
        assert_eq!(pad.chebyshev(HOME), 3);
        assert_eq!(pad, HOME.offset(-3, -3));

        let corner = TilePos::new(1, 1);
        let corner_pad = landing_pad(&obs(100), corner).expect("the corner has open ground");
        assert!(corner_pad.x >= 0 && corner_pad.y >= 0);
        assert_eq!(corner_pad, TilePos::new(3, 0));

        let mut sealed = obs(100);
        sealed.known_rock = (0..sealed.map_height)
            .flat_map(|y| (0..sealed.map_width).map(move |x| TilePos::new(x, y)))
            .collect();
        assert_eq!(landing_pad(&sealed, HOME), None);
    }

    #[test]
    fn airborne_hold_lands_the_bombers_and_keeps_a_fixed_wing_screen_over_home() {
        let mut observation = wealthy_island_obs(100, 1);
        observation
            .my_units
            .push(own(30, UnitKind::Buzzard, TilePos::new(5, 8)));
        let mut plan = AirPlan::island(&profile(), &observation);
        plan.screen = vec![UnitId(30)];
        let mut operation = operation(AirOperationPhase::SuppressAa, 100);
        let pad = landing_pad(&observation, HOME).unwrap();

        let mut held = StrategicDecision::default();
        hold_air_strike(&mut operation, &plan, &observation, HOME, &mut held);
        assert_eq!(
            held.intents,
            [
                Intent::MoveUnits {
                    units: vec![UnitId(3), UnitId(4)],
                    goal: pad,
                },
                Intent::MoveUnits {
                    units: vec![UnitId(30)],
                    goal: HOME,
                },
            ]
        );
        assert_eq!(operation.strike_hold, Some(pad));

        let mut repeated = StrategicDecision::default();
        hold_air_strike(&mut operation, &plan, &observation, HOME, &mut repeated);
        assert!(repeated.intents.is_empty());
    }

    #[test]
    fn artillery_staging_is_dispatched_once_until_the_goal_or_mission_changes() {
        let mut operation = operation(AirOperationPhase::Strike, 100);
        let first_goal = TilePos::new(12, 10);

        let mut first = StrategicDecision::default();
        stage_artillery(&mut operation, first_goal, &mut first);
        assert_eq!(
            first.intents,
            [Intent::MoveUnits {
                units: vec![UnitId(2)],
                goal: first_goal,
            }]
        );

        let mut repeated = StrategicDecision::default();
        stage_artillery(&mut operation, first_goal, &mut repeated);
        assert!(
            repeated.intents.is_empty(),
            "the stable staging move remains authoritative"
        );

        let replacement_goal = first_goal.offset(1, -2);
        let mut redirected = StrategicDecision::default();
        stage_artillery(&mut operation, replacement_goal, &mut redirected);
        assert_eq!(
            redirected.intents,
            [Intent::MoveUnits {
                units: vec![UnitId(2)],
                goal: replacement_goal,
            }]
        );

        let mut replacement_repeated = StrategicDecision::default();
        stage_artillery(&mut operation, replacement_goal, &mut replacement_repeated);
        assert!(replacement_repeated.intents.is_empty());

        let mut suppression_observation = obs(100);
        suppression_observation.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(20, 10),
            true,
        ));
        let intelligence = knowledge(&suppression_observation);
        let mut suppression = StrategicDecision::default();
        let identity = profile();
        let mut plan = connected_test_plan(&suppression_observation);
        let context = AirPlanningContext {
            profile: &identity,
            tuning: DifficultyTuning::for_level(BotDifficulty::Prime),
            obs: &suppression_observation,
            intel: &intelligence,
            home: HOME,
            orientation: test_orientation(),
            public_map: None,
            enlisted: &[],
            landing_sites: &[],
            connected_resources: None,
            production: StrategicProductionContext::empty(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        };
        suppress(&mut operation, &mut plan, &context, &mut suppression);
        assert!(suppression.intents.iter().any(|intent| matches!(
            intent,
            Intent::AttackUnits {
                units,
                target: Target::Building(BuildingId(81)),
            } if units == &[UnitId(2)]
        )));

        let mut restaged = StrategicDecision::default();
        stage_artillery(&mut operation, replacement_goal, &mut restaged);
        assert_eq!(
            restaged.intents,
            [Intent::MoveUnits {
                units: vec![UnitId(2)],
                goal: replacement_goal,
            }],
            "an artillery attack replaces the prior staging order"
        );

        let mut restaged_repeated = StrategicDecision::default();
        stage_artillery(&mut operation, replacement_goal, &mut restaged_repeated);
        assert!(restaged_repeated.intents.is_empty());
    }

    #[test]
    fn a_late_recon_scout_receives_a_fresh_flight_window() {
        let seen = obs(990);
        let mut intel = knowledge(&seen);
        let mut waiting = obs(1_000);
        waiting.enemy_buildings[0].seen = false;
        waiting.my_units.remove(0);
        waiting.my_buildings = vec![building(
            10,
            0,
            BuildingKind::Airworks,
            TilePos::new(2, 2),
            true,
        )];
        waiting.my_queues = vec![vec![UnitKind::Kestrel]];
        intel.update(&waiting);
        let mut planner = with_operation(AirOperationPhase::Recon, 100);
        let operation = planner.air_op_mut().unwrap();
        operation.started_at = 100;
        operation.phase_started_at = 100;

        let before_scout = think(&mut planner, &waiting, &intel);
        assert!(before_scout.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, .. } if units.contains(&UnitId(1))
        )));
        assert_eq!(
            planner.air_operation().unwrap().phase,
            AirOperationPhase::Recon
        );

        waiting.tick = 1_006;
        waiting
            .my_units
            .push(own(5, UnitKind::Kestrel, TilePos::new(4, 10)));
        waiting.my_queues[0].clear();
        intel.update(&waiting);
        let assigned = think(&mut planner, &waiting, &intel);
        let operation = planner.air_operation().unwrap();
        assert_eq!(operation.phase, AirOperationPhase::Recon);
        assert_eq!(operation.phase_started_at, waiting.tick);
        assert_eq!(operation.scout, Some(UnitId(5)));
        assert!(assigned.intents.iter().any(|intent| matches!(
            intent,
            Intent::MoveUnits { units, .. } if units == &[UnitId(5)]
        )));
    }

    #[test]
    fn current_sight_cannot_skip_recon_while_the_required_scout_trains() {
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut current = obs(240);
        current.explored.fill(true);
        current.my_units.retain(|unit| unit.id != UnitId(1));
        current.my_buildings = vec![building(
            10,
            0,
            BuildingKind::Airworks,
            TilePos::new(2, 2),
            true,
        )];
        current.my_queues = vec![Vec::new()];
        current.scrap = UnitKind::Kestrel.stats().cost;
        let mut intelligence = knowledge(&current);
        let mut planner = with_operation(AirOperationPhase::Recon, current.tick);
        let operation = planner.air_op_mut().unwrap();
        operation.scout = None;
        operation.scout_dispatch = None;
        operation.phase_started_at = current.tick.saturating_sub(tuning.reaction_delay + 1);

        let training = planner.think(&profile(), tuning, &current, &intelligence, HOME, &[]);

        let operation = planner.air_operation().unwrap();
        assert_eq!(operation.phase, AirOperationPhase::Recon);
        assert_eq!(operation.scout, None);
        assert_eq!(operation.scout_dispatch, None);
        assert_eq!(training.committed_scrap, UnitKind::Kestrel.stats().cost);
        assert_eq!(
            training
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::TrainAt { .. }))
                .cloned()
                .collect::<Vec<_>>(),
            [Intent::TrainAt {
                building: BuildingId(10),
                kind: UnitKind::Kestrel,
            }],
            "the operation may hold its strike force while the required scout trains"
        );

        let mut hidden = current;
        hidden.tick += 12;
        hidden.enemy_buildings[0].seen = false;
        hidden.my_queues[0] = vec![UnitKind::Kestrel];
        hidden.scrap = 0;
        intelligence.update(&hidden);

        let waiting = planner.think(&profile(), tuning, &hidden, &intelligence, HOME, &[]);

        let operation = planner.air_operation().unwrap();
        assert_eq!(operation.phase, AirOperationPhase::Recon);
        assert_eq!(operation.scout, None);
        assert_eq!(operation.scout_dispatch, None);
        assert!(waiting.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Kestrel,
                ..
            }
        )));

        let mut ready = hidden;
        ready.tick += 12;
        ready.my_queues[0].clear();
        ready
            .my_units
            .push(own(5, UnitKind::Kestrel, TilePos::new(4, 10)));
        ready.my_units.sort_unstable_by_key(|unit| unit.id);
        intelligence.update(&ready);

        let dispatch = planner.think(&profile(), tuning, &ready, &intelligence, HOME, &[]);

        let operation = planner.air_operation().unwrap();
        assert_eq!(operation.phase, AirOperationPhase::Recon);
        assert_eq!(operation.scout, Some(UnitId(5)));
        assert!(operation.scout_dispatch.is_some());
        assert!(dispatch.intents.iter().any(|intent| matches!(
            intent,
            Intent::MoveUnits { units, .. } if units == &[UnitId(5)]
        )));

        let mut reacquired = ready;
        reacquired.tick += 1;
        reacquired.enemy_buildings[0].seen = true;
        reacquired
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(5))
            .unwrap()
            .idle = false;
        intelligence.update(&reacquired);

        let reacquired_result = planner.think_through_adjudication(
            &profile(),
            tuning,
            &reacquired,
            &intelligence,
            HOME,
            coordination(None),
        );

        assert_eq!(
            planner.air_operation().unwrap().phase,
            AirOperationPhase::Assemble,
            "Prime may react immediately only after the required scout has a real dispatch; rejection={:?}",
            reacquired_result.rejected_connected_candidate,
        );
    }

    #[test]
    fn adjacent_difficulties_cannot_advance_recon_without_a_dispatched_scout() {
        for pair in BotDifficulty::ALL.windows(2) {
            let &[lower, higher] = pair else {
                unreachable!();
            };
            for difficulty in [lower, higher] {
                let tuning = DifficultyTuning::for_level(difficulty);
                let mut observation = obs(500);
                observation.my_units.retain(|unit| unit.id != UnitId(1));
                observation.my_buildings = vec![building(
                    10,
                    0,
                    BuildingKind::Airworks,
                    TilePos::new(2, 2),
                    true,
                )];
                observation.my_queues = vec![vec![UnitKind::Kestrel]];
                let intelligence = knowledge(&observation);
                let mut planner = with_operation(AirOperationPhase::Recon, observation.tick);
                let operation = planner.air_op_mut().unwrap();
                operation.scout = None;
                operation.scout_dispatch = None;
                operation.phase_started_at =
                    observation.tick.saturating_sub(tuning.reaction_delay + 1);
                let mut identity = profile();
                identity.difficulty = difficulty;

                planner.think(&identity, tuning, &observation, &intelligence, HOME, &[]);

                let operation = planner.air_operation().unwrap();
                assert_eq!(
                    operation.phase,
                    AirOperationPhase::Recon,
                    "{difficulty:?} advanced after its full reaction delay without a scout dispatch"
                );
                assert_eq!(operation.scout, None);
                assert_eq!(operation.scout_dispatch, None);
            }
        }
    }

    #[test]
    fn current_flak_is_suppressed_before_bombers_and_ids_are_reserved() {
        let mut obs = obs(100);
        obs.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(20, 10),
            true,
        ));
        let intel = knowledge(&obs);
        let mut planner = with_operation(AirOperationPhase::SuppressAa, 100);
        let out = think(&mut planner, &obs, &intel);
        assert_eq!(
            out.intents[0],
            Intent::AttackUnits {
                units: vec![UnitId(2)],
                target: Target::Building(BuildingId(81)),
            }
        );
        assert!(!out.intents.iter().any(
            |intent| matches!(intent, Intent::AttackUnits { units, .. } if units.contains(&UnitId(3)))
        ));
        assert_eq!(
            out.reservations,
            [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
        );
    }

    #[test]
    fn connected_verification_does_not_treat_unfinished_flak_as_operational() {
        let flak_anchor = TilePos::new(20, 10);
        let mut construction = obs(100);
        see_approach(&mut construction);
        let dark_approach = approach(HOME, TARGET)
            .find(|tile| *tile != TARGET && *tile != flak_anchor)
            .expect("the test route has an approach tile to reacquire");
        let dark_index =
            usize::try_from(dark_approach.y * construction.map_width + dark_approach.x).unwrap();
        construction.visible[dark_index] = false;
        let mut flak = building(81, 1, BuildingKind::FlakTurret, flak_anchor, true);
        flak.built = false;
        construction.enemy_buildings.push(flak);

        let mut intel = knowledge(&construction);
        assert!(
            intel
                .buildings()
                .iter()
                .any(|building| building.id == Some(BuildingId(81)) && !building.built),
            "the observed construction remains available as ordinary intelligence"
        );
        let mut planner = with_operation(AirOperationPhase::Verify, construction.tick);
        let while_unfinished = think(&mut planner, &construction, &intel);

        let operation = planner
            .air_operation()
            .expect("the operation remains active");
        assert_eq!(operation.phase, AirOperationPhase::Verify);
        assert_eq!(operation.recovery_reason, None);
        assert!(while_unfinished.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(81)),
                ..
            }
        )));

        let mut completed = construction;
        completed.tick += 1;
        completed
            .enemy_buildings
            .iter_mut()
            .find(|building| building.id == BuildingId(81))
            .expect("the Flak construction remains in sight")
            .built = true;
        intel.update(&completed);
        let after_completion = think(&mut planner, &completed, &intel);

        let operation = planner
            .air_operation()
            .expect("suppression retains the operation");
        assert_eq!(operation.phase, AirOperationPhase::SuppressAa);
        assert_eq!(operation.recovery_reason, None);
        assert!(after_completion.intents.iter().any(|intent| matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(81)),
                ..
            }
        )));
    }

    #[test]
    fn connected_strike_resumes_artillery_suppression_when_flak_completes() {
        let flak_anchor = TilePos::new(20, 10);
        let mut construction = obs(100);
        see_approach(&mut construction);
        explore(&mut construction, staging(HOME, TARGET));
        let mut flak = building(81, 1, BuildingKind::FlakTurret, flak_anchor, true);
        flak.built = false;
        construction.enemy_buildings.push(flak);

        let mut intel = knowledge(&construction);
        let mut planner = with_operation(AirOperationPhase::Strike, construction.tick);
        let while_unfinished = think(&mut planner, &construction, &intel);

        let operation = planner.air_operation().expect("the strike remains active");
        assert_eq!(operation.phase, AirOperationPhase::Strike);
        assert_eq!(operation.recovery_reason, None);
        assert!(while_unfinished.intents.iter().any(|intent| matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));

        let mut completed = construction;
        completed.tick += 1;
        completed
            .enemy_buildings
            .iter_mut()
            .find(|building| building.id == BuildingId(81))
            .expect("the Flak construction remains in sight")
            .built = true;
        intel.update(&completed);
        let after_completion = think(&mut planner, &completed, &intel);

        let operation = planner
            .air_operation()
            .expect("suppression retains the operation");
        assert_eq!(operation.phase, AirOperationPhase::SuppressAa);
        assert_eq!(operation.recovery_reason, None);
        assert!(after_completion.intents.iter().any(|intent| matches!(
            intent,
            Intent::AttackUnits {
                units,
                target: Target::Building(BuildingId(81)),
            } if units.contains(&UnitId(2))
        )));
        assert!(after_completion.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));
    }

    #[test]
    fn remembered_flak_is_not_mistaken_for_destroyed_flak() {
        let mut seen = obs(100);
        seen.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(20, 10),
            true,
        ));
        let mut intel = knowledge(&seen);
        let mut hidden = obs(101);
        hidden.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, TARGET, false),
            building(
                999,
                1,
                BuildingKind::FlakTurret,
                TilePos::new(20, 10),
                false,
            ),
        ];
        intel.update(&hidden);
        let mut planner = with_operation(AirOperationPhase::SuppressAa, 101);
        let out = think(&mut planner, &hidden, &intel);
        assert_eq!(
            planner.air_operation().unwrap().phase,
            AirOperationPhase::SuppressAa
        );
        assert!(
            !out.intents
                .iter()
                .any(|intent| matches!(intent, Intent::AttackUnits { .. }))
        );
    }

    #[test]
    fn assembly_spreads_work_respects_affordability_and_banks_partial_scrap() {
        let mut obs = obs(200);
        obs.visible.fill(true);
        obs.explored.fill(true);
        obs.my_units.truncate(1);
        obs.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), false),
            building(11, 0, BuildingKind::Fabricator, TilePos::new(5, 2), false),
            building(12, 0, BuildingKind::Airworks, TilePos::new(2, 5), false),
            building(13, 0, BuildingKind::Airworks, TilePos::new(5, 5), false),
            building(14, 0, BuildingKind::Crucible, TilePos::new(8, 5), false),
        ];
        obs.my_queues = vec![Vec::new(); 5];
        let mut plan = connected_test_plan(&obs);
        let mut operation = operation(AirOperationPhase::Assemble, obs.tick);
        operation.artillery.clear();
        operation.strike_aircraft.clear();
        let identity = profile();
        let intelligence = knowledge(&obs);
        let full = UnitKind::Bombard.stats().cost + UnitKind::Condor.stats().cost * 2;
        obs.scrap = full;
        let mut out = StrategicDecision::default();
        schedule_missing_members(
            &operation,
            &mut plan,
            &planning_context(&identity, &obs, &intelligence),
            UnitKind::Kestrel,
            &mut out,
        );
        assert_eq!(out.committed_scrap, full, "{out:?}");
        let bomber_factories: Vec<_> = out
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::TrainAt {
                    building,
                    kind: UnitKind::Condor,
                } => Some(*building),
                _ => None,
            })
            .collect();
        assert_eq!(bomber_factories, [BuildingId(12), BuildingId(13)]);

        obs.scrap = UnitKind::Bombard.stats().cost + 17;
        let mut partial = StrategicDecision::default();
        schedule_missing_members(
            &operation,
            &mut plan,
            &planning_context(&identity, &obs, &intelligence),
            UnitKind::Kestrel,
            &mut partial,
        );
        assert_eq!(partial.committed_scrap, obs.scrap);
        assert_eq!(
            partial
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::TrainAt { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn a_full_operation_queue_holds_the_next_provider_cost_until_a_slot_opens() {
        let mut observation = obs(200);
        observation.visible.fill(true);
        observation.explored.fill(true);
        observation.scrap = UnitKind::Bombard.stats().cost;
        observation.my_buildings = vec![building(
            10,
            0,
            BuildingKind::Fabricator,
            TilePos::new(2, 2),
            true,
        )];
        observation.my_queues = vec![vec![UnitKind::Lancer; QUEUE_CAP]];
        let mut plan = connected_test_plan(&observation);
        let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
        operation.artillery.clear();
        let identity = profile();
        let intelligence = knowledge(&observation);
        let mut decision = StrategicDecision::default();

        schedule_missing_members(
            &operation,
            &mut plan,
            &planning_context(&identity, &observation, &intelligence),
            UnitKind::Kestrel,
            &mut decision,
        );

        assert!(
            decision.intents.is_empty(),
            "a future slot is feasibility evidence, not a current append"
        );
        assert_eq!(
            decision.committed_scrap,
            UnitKind::Bombard.stats().cost,
            "the operation must keep its next provider affordable while the paid queue drains"
        );
    }

    #[test]
    fn a_slot_blocked_provider_keeps_its_capital_while_excess_uses_an_open_lane() {
        let mut observation = obs(200);
        observation.visible.fill(true);
        observation.explored.fill(true);
        observation.scrap = UnitKind::Bombard.stats().cost;
        observation.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
        ];
        observation.my_queues = vec![vec![UnitKind::Lancer; QUEUE_CAP], Vec::new()];
        let mut plan = connected_test_plan(&observation);
        let package = plan
            .connected_package
            .as_mut()
            .expect("connected test plan has a package");
        package.strike = vec![ProviderDemand {
            kind: UnitKind::Buzzard,
            count: 1,
        }];
        package.provider_priority = vec![
            force_package::ProviderDemandTranche {
                priority: force_package::ProviderPriority::Minimum,
                family: ForceFamily::Recon,
                kind: UnitKind::Kestrel,
                count: 1,
            },
            force_package::ProviderDemandTranche {
                priority: force_package::ProviderPriority::Minimum,
                family: ForceFamily::Suppression,
                kind: UnitKind::Bombard,
                count: 1,
            },
            force_package::ProviderDemandTranche {
                priority: force_package::ProviderPriority::Minimum,
                family: ForceFamily::Strike,
                kind: UnitKind::Buzzard,
                count: 1,
            },
        ];
        let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
        operation.artillery.clear();
        operation.strike_aircraft.clear();
        let identity = profile();
        let intelligence = knowledge(&observation);
        let mut decision = StrategicDecision::default();

        schedule_missing_members(
            &operation,
            &mut plan,
            &planning_context(&identity, &observation, &intelligence),
            UnitKind::Kestrel,
            &mut decision,
        );

        assert!(
            decision.intents.is_empty(),
            "the later open Airworks cannot spend capital assigned to the next Bombard"
        );
        assert_eq!(decision.committed_scrap, UnitKind::Bombard.stats().cost);

        observation.scrap = UnitKind::Bombard
            .stats()
            .cost
            .saturating_add(UnitKind::Buzzard.stats().cost);
        let mut parallel = StrategicDecision::default();
        schedule_missing_members(
            &operation,
            &mut plan,
            &planning_context(&identity, &observation, &intelligence),
            UnitKind::Kestrel,
            &mut parallel,
        );

        assert_eq!(
            parallel.intents,
            vec![Intent::TrainAt {
                building: BuildingId(11),
                kind: UnitKind::Buzzard,
            }]
        );
        assert_eq!(parallel.committed_scrap, observation.scrap);
    }

    #[test]
    fn current_bank_funds_the_whole_minimum_before_forecast_funded_marginal_work() {
        let mut identity = profile();
        identity.primary = Specialty::Siege;
        identity.secondary = Specialty::Air;
        identity.traits.air = 10;
        identity.traits.siege = 90;

        let minimum_scrap = UnitKind::Kestrel
            .stats()
            .cost
            .saturating_add(UnitKind::Bombard.stats().cost)
            .saturating_add(UnitKind::Buzzard.stats().cost);
        let mut observation = developed_connected_obs(100);
        observation.scrap = minimum_scrap;
        observation.my_units.clear();
        observation.enemy_buildings = vec![
            building(80, 1, BuildingKind::Turret, TARGET, true),
            building(81, 1, BuildingKind::FlakTurret, TARGET.offset(-1, 0), true),
            building(82, 1, BuildingKind::FlakTurret, TARGET.offset(1, 0), true),
        ];
        let mut reclaimer = building(20, 0, BuildingKind::Reclaimer, TilePos::new(11, 2), true);
        reclaimer.tier = 1;
        observation.my_buildings.push(reclaimer);
        observation.my_queues.push(Vec::new());

        let intelligence = knowledge(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.kind == BuildingKind::Turret)
            .expect("current strategic target");
        let resources = ConnectedProductionResources::from_observation(
            &observation,
            target,
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home: HOME,
                target: target.anchor,
                public_map: None,
                orientation: test_orientation(),
            },
        );
        let mut plan = connected_plan(
            &identity,
            &observation,
            &intelligence,
            HOME,
            target,
            &[],
            ConnectedPlanningContext {
                orientation: test_orientation(),
                public_map: None,
                resources: &resources,
                preferred_artillery: &[],
                protected_current_scrap: 0,
                preparation: PreparationConstraints {
                    deadline: 8_000,
                    decision_cadence: DifficultyTuning::for_level(identity.difficulty).cadence,
                    protected_forecast_scrap: 0,
                },
            },
        )
        .expect("forecast income can fund marginal suppression after the minimum package");
        let package = plan
            .connected_package
            .as_ref()
            .expect("connected plan carries its force package");
        let minimum: Vec<_> = package
            .provider_priority
            .iter()
            .filter(|tranche| tranche.priority == force_package::ProviderPriority::Minimum)
            .copied()
            .collect();
        assert_eq!(
            minimum
                .iter()
                .map(|tranche| tranche.family)
                .collect::<Vec<_>>(),
            [
                ForceFamily::Recon,
                ForceFamily::Suppression,
                ForceFamily::Strike,
            ]
        );
        assert!(package.provider_priority.iter().any(|tranche| {
            tranche.priority == force_package::ProviderPriority::Marginal
                && tranche.family == ForceFamily::Suppression
        }));

        let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
        operation.scout = None;
        operation.artillery.clear();
        operation.strike_aircraft.clear();
        let mut decision = StrategicDecision::default();
        schedule_missing_members(
            &operation,
            &mut plan,
            &planning_context(&identity, &observation, &intelligence),
            UnitKind::Kestrel,
            &mut decision,
        );

        let scheduled: Vec<_> = decision
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::TrainAt { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert_eq!(
            scheduled,
            minimum
                .iter()
                .flat_map(|tranche| core::iter::repeat_n(tranche.kind, tranche.count))
                .collect::<Vec<_>>()
        );
        assert_eq!(decision.committed_scrap, minimum_scrap);
    }

    #[test]
    fn scout_holds_outside_known_flak_while_keeping_the_objective_in_sight() {
        let mut obs = obs(250);
        obs.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(20, 10),
            true,
        ));
        let intel = knowledge(&obs);
        let operation = operation(AirOperationPhase::SuppressAa, 250);

        let goal = scout_goal(&operation, &obs, &intel, operation.target, &[], None)
            .expect("safe spotting tile");
        assert_ne!(
            goal, operation.target,
            "the scout must not orbit over the bomb target"
        );
        assert_ne!(
            intel.air_defense_at(goal).evidence(),
            AirDefenseEvidence::CurrentCoverage,
            "a known safe spotting tile exists outside the flak envelope"
        );
        let dx = goal.x - operation.target.x;
        let dy = goal.y - operation.target.y;
        let sight = UnitKind::Kestrel.stats().vision - 1;
        assert!(dx * dx + dy * dy <= sight * sight);
    }

    #[test]
    fn optional_strike_loss_continues_until_minimum_capability_is_lost() {
        let mut battle = obs(300);
        battle.my_units.retain(|unit| unit.id != UnitId(4));
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Strike, 300);
        let continued = think(&mut planner, &battle, &intel);
        let op = planner.air_operation().unwrap();
        assert_eq!(op.phase, AirOperationPhase::Strike);
        assert_eq!(op.recovery_reason, None);
        assert_eq!(continued.reservations, [UnitId(1), UnitId(2), UnitId(3)]);

        battle.tick += 1;
        battle.my_units.retain(|unit| unit.id != UnitId(3));
        let intel = knowledge(&battle);
        let out = think(&mut planner, &battle, &intel);
        let op = planner.air_operation().unwrap();
        assert_eq!(op.phase, AirOperationPhase::Recover);
        assert_eq!(
            op.recovery_reason,
            Some(AirRecoveryReason::RequiredUnitLost)
        );
        assert!(planner.cooldown_until() > battle.tick);
        assert!(out.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(1), UnitId(2)],
            goal: HOME,
        }));

        let late = 10_000;
        let obs = obs(late);
        let intel = knowledge(&obs);
        let mut timed_out = with_operation(AirOperationPhase::Recon, late);
        timed_out.air_op_mut().unwrap().started_at = 0;
        think(&mut timed_out, &obs, &intel);
        let op = timed_out.air_operation().unwrap();
        assert_eq!(op.phase, AirOperationPhase::Recover);
        assert_eq!(op.recovery_reason, Some(AirRecoveryReason::Timeout));
    }

    #[test]
    fn recovery_trusts_terminal_move_orders_instead_of_recalling_every_think() {
        let mut settled = obs(408);
        settled.faction = Faction::Cupric;
        settled.my_units = vec![
            own(1, UnitKind::Gnat, TilePos::new(3, 10)),
            own(2, UnitKind::Bombard, TilePos::new(4, 10)),
            own(3, UnitKind::Moth, TilePos::new(1, 8)),
            own(4, UnitKind::Moth, TilePos::new(2, 8)),
        ];
        let intel = knowledge(&settled);
        let mut planner = with_operation(AirOperationPhase::Recover, settled.tick);
        planner.cooldown_until = 1_000;
        planner.air_op_mut().unwrap().recovery_reason = Some(AirRecoveryReason::Complete);

        let decision = think(&mut planner, &settled, &intel);

        assert!(planner.air_operation().is_none());
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { goal, .. } if *goal == HOME
        )));
        assert_eq!(
            decision.reservations,
            [UnitId(1), UnitId(2), UnitId(3), UnitId(4)],
            "the completion cadence retains ownership until utility has finished"
        );

        settled.tick = 999;
        let intel = knowledge(&settled);
        let next = think(&mut planner, &settled, &intel);
        assert_eq!(
            next.reservations,
            [UnitId(1), UnitId(2), UnitId(3), UnitId(4)],
            "settled operation members remain owned through the cooldown"
        );

        settled.tick = 1_000;
        let intel = knowledge(&settled);
        let released = think(&mut planner, &settled, &intel);
        assert!(released.reservations.is_empty());
    }

    #[test]
    fn a_dropped_failed_operation_exposes_one_terminal_abort_signal() {
        let observation = obs(400);
        let intel = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Recover, 400);
        planner.air_op_mut().unwrap().recovery_reason = Some(AirRecoveryReason::NewAirDefense);

        think(&mut planner, &observation, &intel);

        assert!(planner.air_operation().is_none());
        assert_eq!(
            planner.terminal_outcome(),
            Some(AirOperationOutcome::Aborted {
                player: PlayerId(1),
                target: TARGET,
            })
        );

        think(&mut planner, &observation, &intel);
        assert_eq!(planner.terminal_outcome(), None);
    }

    #[test]
    fn the_next_operation_reuses_its_standby_roles_before_training_replacements() {
        let mut settled = obs(408);
        settled.my_units.retain(|unit| unit.id != UnitId(4));
        settled
            .my_units
            .push(own(5, UnitKind::Bombard, TilePos::new(5, 10)));
        settled
            .my_units
            .extend((100..=108).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
        settled.my_units.sort_unstable_by_key(|unit| unit.id);
        settled.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
            building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
        ];
        settled.my_queues = vec![Vec::new(); 3];
        settled.visible.fill(true);
        settled.explored.fill(true);
        settled.scrap = 10_000;
        let mut planner = with_operation(AirOperationPhase::Recover, settled.tick);
        planner.cooldown_until = 500;
        let operation = planner.air_op_mut().unwrap();
        operation.artillery = vec![UnitId(2), UnitId(5)];
        operation.strike_aircraft = vec![UnitId(3)];
        operation.recovery_reason = Some(AirRecoveryReason::Timeout);
        let intel = knowledge(&settled);

        let recovered = think(&mut planner, &settled, &intel);
        assert!(planner.air_operation().is_none());
        assert_eq!(
            recovered.reservations,
            [UnitId(1), UnitId(2), UnitId(3), UnitId(5)]
        );

        settled.tick = 504;
        let intel = knowledge(&settled);
        assert!(
            derived_connected_test_plan(&profile(), &settled).is_some(),
            "the recovered force and current bank can field a retry"
        );
        let retried = think(&mut planner, &settled, &intel);
        let operation = planner.air_operation().expect("a new operation starts");
        assert_eq!(operation.scout, Some(UnitId(1)));
        assert_eq!(operation.artillery, [UnitId(2)]);
        assert!(
            !retried.reservations.contains(&UnitId(5)),
            "standby units beyond the target's useful suppression demand return to the standing force"
        );
        assert_eq!(operation.strike_aircraft, [UnitId(3)]);
        let retry_package = planner
            .air_plan()
            .and_then(|plan| plan.connected_package.as_ref())
            .expect("the retry retains its selected force package");
        let selected_strike = retry_package
            .strike
            .iter()
            .map(|demand| demand.count)
            .sum::<usize>();
        let scheduled_strike = retried
            .intents
            .iter()
            .filter(|intent| {
                matches!(
                    intent,
                    Intent::TrainAt { kind, .. }
                        if retry_package.strike.iter().any(|demand| demand.kind == *kind)
                )
            })
            .count();
        assert_eq!(
            scheduled_strike,
            selected_strike.saturating_sub(operation.strike_aircraft.len()),
            "lowering trains only the selected strike demand not already held in standby"
        );
        assert!(retried.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Bombard,
                ..
            }
        )));
    }

    #[test]
    fn standby_releases_when_no_operation_target_survives_the_cooldown() {
        let mut settled = obs(408);
        let mut planner = with_operation(AirOperationPhase::Recover, settled.tick);
        planner.cooldown_until = 500;
        planner.air_op_mut().unwrap().recovery_reason = Some(AirRecoveryReason::Complete);
        let intel = knowledge(&settled);
        think(&mut planner, &settled, &intel);

        settled.tick = 504;
        settled.enemy_buildings.clear();
        let intel = knowledge(&settled);
        let released = think(&mut planner, &settled, &intel);

        assert!(planner.air_operation().is_none());
        assert!(released.reservations.is_empty());
    }

    #[test]
    fn recovery_keeps_moving_survivors_without_replacing_their_return_order() {
        let mut returning = obs(408);
        returning.my_units[0].idle = false;
        let intel = knowledge(&returning);
        let mut planner = with_operation(AirOperationPhase::Recover, returning.tick);

        let decision = think(&mut planner, &returning, &intel);

        assert!(planner.air_operation().is_some());
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { goal, .. } if *goal == HOME
        )));
    }

    #[test]
    fn assembly_reconnoiters_an_unexplored_staging_line_before_moving_artillery() {
        let mut observation = obs(300);
        observation
            .my_units
            .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
        let intel = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Assemble, 300);

        let decision = think(&mut planner, &observation, &intel);
        let ideal = staging(HOME, TARGET);

        let operation = planner.air_operation().expect("assembly remains active");
        assert_eq!(operation.phase, AirOperationPhase::Assemble);
        assert_eq!(operation.scout_dispatch, Some((UnitId(1), ideal)));
        assert!(decision.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(1)],
            goal: ideal,
        }));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, .. }
                if units.contains(&UnitId(2)) || units.contains(&UnitId(5))
        )));
    }

    #[test]
    fn public_terrain_skips_a_sealed_staging_tile_for_a_reachable_alternative() {
        let mut observation = obs(300);
        observation.my_units[1].tile = HOME.offset(4, 0);
        let ideal = staging(HOME, TARGET);
        let sealed_ring = (-1..=1).flat_map(|dy| {
            (-1..=1)
                .filter(move |dx| *dx != 0 || dy != 0)
                .map(move |dx| (ideal.offset(dx, dy), Terrain::Peak))
        });
        let public_map = public_map_with_terrain(&observation, sealed_ring);
        let operation = operation(AirOperationPhase::Assemble, observation.tick);

        assert_eq!(
            connected_artillery_staging_goal(&observation, HOME, TARGET, None),
            Some(ideal),
            "fog alone makes the enclosed ideal look reachable"
        );
        let public_goal =
            connected_artillery_staging_goal(&observation, HOME, TARGET, Some(&public_map))
                .expect("a later public-terrain-safe staging candidate exists");
        assert_ne!(public_goal, ideal);
        assert_eq!(
            artillery_staging(
                &operation,
                &observation,
                HOME,
                TARGET,
                Some(&public_map),
                test_orientation(),
            ),
            Some(ArtilleryStaging::NeedsRecon(public_goal))
        );
    }

    #[test]
    fn artillery_staging_validates_the_exact_spread_assigned_by_the_group_command() {
        let mut observation = obs(300);
        observation.explored.fill(true);
        observation.my_units[1].tile = TilePos::new(14, 10);
        observation
            .my_units
            .push(own(5, UnitKind::Bombard, TilePos::new(14, 11)));
        let ideal = staging(HOME, TARGET);
        let isolated_spread_goal = ideal.offset(1, 1);
        observation.known_rock = [
            isolated_spread_goal.offset(-1, 0),
            isolated_spread_goal.offset(1, 0),
            isolated_spread_goal.offset(0, -1),
            isolated_spread_goal.offset(0, 1),
        ]
        .into_iter()
        .collect();
        observation
            .known_rock
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
        operation.artillery.push(UnitId(5));

        let mut center_only = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            None,
            test_orientation(),
        );
        assert!(operation.artillery.iter().all(|id| {
            unit(&observation, *id).is_some_and(|member| center_only.unit_reaches(member, ideal))
        }));
        assert!(
            !center_only.group_reaches_command_goal(&operation.artillery, ideal),
            "the eastward approach assigns the isolated south-east spread tile"
        );

        let alternate = artillery_staging(
            &operation,
            &observation,
            HOME,
            TARGET,
            None,
            test_orientation(),
        )
        .expect("a later staging candidate accepts the complete spread");
        let ArtilleryStaging::Ready(alternate) = alternate else {
            panic!("the explored fixture must return a ready staging goal");
        };
        assert_ne!(alternate, ideal);
        let mut exact = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            None,
            test_orientation(),
        );
        assert!(exact.group_reaches_command_goal(&operation.artillery, alternate));
    }

    #[test]
    fn connected_admission_rejects_a_group_larger_than_the_reachable_staging_spread() {
        let home = TilePos::new(0, 0);
        let target = TilePos::new(6, 0);
        let source_staging = staging(home, target);
        let remote_open = TilePos::new(5, 0);
        let mut observation = obs(300);
        observation.map_width = 12;
        observation.map_height = 8;
        observation.visible = vec![true; 12 * 8];
        observation.explored = vec![true; 12 * 8];
        observation.my_units = vec![
            own(2, UnitKind::Bombard, source_staging),
            own(5, UnitKind::Bombard, source_staging),
        ];
        observation.my_buildings = vec![building(20, 0, BuildingKind::Foundry, home, true)];
        observation.my_queues = vec![Vec::new()];
        observation.enemy_buildings = vec![building(80, 1, BuildingKind::Crucible, target, true)];
        observation.known_rock = (0..observation.map_height)
            .flat_map(|y| {
                (0..observation.map_width).filter_map(move |x| {
                    let tile = TilePos::new(x, y);
                    (tile != source_staging && tile != remote_open).then_some(tile)
                })
            })
            .collect();
        let public_map = public_map_with_terrain(&observation, []);
        let orientation = Orientation::for_home(&observation, home);
        let intelligence = knowledge(&observation);
        let route = ConnectedRouteContext {
            intel: &intelligence,
            home,
            target,
            public_map: Some(&public_map),
            orientation,
        };

        assert_eq!(
            connected_artillery_staging_goal(&observation, home, target, Some(&public_map)),
            Some(source_staging)
        );
        let mut individual = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&public_map),
            orientation,
        );
        assert!(
            observation
                .my_units
                .iter()
                .all(|member| { individual.unit_reaches(member, source_staging) })
        );
        assert!(connected_artillery_group_has_staging(
            &observation,
            route,
            &[ProviderDemand {
                kind: UnitKind::Bombard,
                count: 1,
            }],
            &[],
            &[],
        ));
        assert!(
            !connected_artillery_group_has_staging(
                &observation,
                route,
                &[ProviderDemand {
                    kind: UnitKind::Bombard,
                    count: 2,
                }],
                &[],
                &[],
            ),
            "individual center reachability cannot admit a spread slot in another component"
        );
    }

    #[test]
    fn suppression_preflight_requires_one_reachable_firing_stand_per_provider() {
        let origin = TilePos::new(0, 7);
        let first_stand = TilePos::new(1, 7);
        let second_stand = TilePos::new(2, 7);
        let target_anchor = TilePos::new(11, 7);
        let mut observation = obs(300);
        observation.map_width = 14;
        observation.map_height = 14;
        observation.visible = vec![true; 14 * 14];
        observation.explored = vec![true; 14 * 14];
        observation.my_units.clear();
        observation.enemy_buildings = vec![building(
            81,
            1,
            BuildingKind::FlakTurret,
            target_anchor,
            true,
        )];
        let intelligence = knowledge(&observation);
        let target = Target::Building(BuildingId(81));
        let origins = vec![
            SuppressionOrigin {
                tile: origin,
                kind: UnitKind::Bombard,
            },
            SuppressionOrigin {
                tile: origin,
                kind: UnitKind::Bombard,
            },
        ];
        let terrain_with = |open: Vec<TilePos>| {
            (0..observation.map_height).flat_map(move |y| {
                let open = open.clone();
                (0..observation.map_width).filter_map(move |x| {
                    let tile = TilePos::new(x, y);
                    (!open.contains(&tile)).then_some((tile, Terrain::Pit))
                })
            })
        };

        let one_stand_map =
            public_map_with_terrain(&observation, terrain_with(vec![origin, first_stand]));
        assert_eq!(
            suppression_firing_assignment(
                &observation,
                &intelligence,
                &origins[..1],
                target,
                Some(&one_stand_map),
                test_orientation(),
            ),
            Some(vec![first_stand])
        );
        assert_eq!(
            suppression_firing_assignment(
                &observation,
                &intelligence,
                &origins,
                target,
                Some(&one_stand_map),
                test_orientation(),
            ),
            None,
            "two providers cannot be admitted against one legal firing tile"
        );

        let two_stand_map = public_map_with_terrain(
            &observation,
            terrain_with(vec![origin, first_stand, second_stand]),
        );
        let assigned = suppression_firing_assignment(
            &observation,
            &intelligence,
            &origins,
            target,
            Some(&two_stand_map),
            test_orientation(),
        )
        .expect("two connected legal firing tiles admit both providers");
        assert_eq!(assigned.len(), 2);
        assert_ne!(assigned[0], assigned[1]);
        assert!(assigned.iter().all(|stand| {
            let mut routes = route_projection_with_orientation(
                &observation,
                Domain::Ground,
                Some(&two_stand_map),
                test_orientation(),
            );
            suppression_firing_stands(
                &mut routes,
                &observation,
                origins[0],
                target,
                &intelligence,
                Some(&two_stand_map),
            )
            .any(|candidate| candidate == *stand)
        }));
    }

    #[test]
    fn suppression_assignment_backtracks_for_a_constrained_provider() {
        let options = vec![vec![0, 1], vec![0]];
        let mut owner_by_stand = vec![None; 2];
        let mut first_visited = vec![false; 2];
        assert!(augment_suppression_assignment(
            0,
            &options,
            &mut first_visited,
            &mut owner_by_stand,
        ));
        let mut second_visited = vec![false; 2];
        assert!(augment_suppression_assignment(
            1,
            &options,
            &mut second_visited,
            &mut owner_by_stand,
        ));
        assert_eq!(owner_by_stand, vec![Some(1), Some(0)]);
    }

    #[test]
    fn authoritative_spread_preflight_is_scoped_to_connected_operations() {
        let mut observation = obs(300);
        let goal = TilePos::new(12, 10);
        observation.my_units[2].tile = goal.offset(4, 0);
        observation.my_units[3].tile = goal.offset(4, 1);
        let isolated_reversed_slot = goal.offset(1, 1);
        observation.known_peaks = [
            isolated_reversed_slot.offset(-1, 0),
            isolated_reversed_slot.offset(1, 0),
            isolated_reversed_slot.offset(0, -1),
            isolated_reversed_slot.offset(0, 1),
        ]
        .into_iter()
        .collect();
        observation
            .known_peaks
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        observation.my_buildings = vec![building(
            20,
            0,
            BuildingKind::Airworks,
            TilePos::new(2, 2),
            true,
        )];
        observation.my_queues = vec![Vec::new()];
        let attackers = [UnitId(3), UnitId(4)];
        let orientation = test_orientation();

        let island = AirPlan::island(&profile(), &observation);
        let mut island_routes =
            operation_route_projection(&island, &observation, Domain::Air, None, orientation);
        assert!(
            island_routes.group_reaches_command_goal(&attackers, goal),
            "the pre-existing island operation keeps its legacy forward spread"
        );

        let connected = connected_test_plan(&observation);
        let mut connected_routes =
            operation_route_projection(&connected, &observation, Domain::Air, None, orientation);
        assert!(
            !connected_routes.group_reaches_command_goal(&attackers, goal),
            "the migrated connected operation preflights the authoritative reverse spread"
        );
    }

    #[test]
    fn connected_admission_uses_the_authoritative_spread_for_an_exact_live_group() {
        let home = TilePos::new(0, 2);
        let target = TilePos::new(6, 2);
        let source_staging = staging(home, target);
        let mut observation = obs(300);
        observation.map_width = 12;
        observation.map_height = 8;
        observation.visible = vec![true; 12 * 8];
        observation.explored = vec![true; 12 * 8];
        observation.my_units = vec![
            own(2, UnitKind::Bombard, source_staging.offset(-1, -1)),
            own(5, UnitKind::Bombard, source_staging.offset(0, -1)),
        ];
        observation.my_buildings = vec![building(20, 0, BuildingKind::Foundry, home, true)];
        observation.my_queues = vec![Vec::new()];
        observation.enemy_buildings = vec![building(80, 1, BuildingKind::Crucible, target, true)];
        let open = [
            source_staging,
            source_staging.offset(-1, -1),
            source_staging.offset(0, -1),
            source_staging.offset(1, 1),
        ];
        observation.known_rock = (0..observation.map_height)
            .flat_map(|y| {
                (0..observation.map_width).filter_map(move |x| {
                    let tile = TilePos::new(x, y);
                    (!open.contains(&tile)).then_some(tile)
                })
            })
            .collect();
        let public_map = public_map_with_terrain(&observation, []);
        let orientation = Orientation::for_home(&observation, home);
        let intelligence = knowledge(&observation);
        let route = ConnectedRouteContext {
            intel: &intelligence,
            home,
            target,
            public_map: Some(&public_map),
            orientation,
        };
        let exact_ids = [UnitId(2), UnitId(5)];
        let mut routes = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&public_map),
            orientation,
        );
        assert!(routes.group_reaches_command_goal(&exact_ids, source_staging));
        assert!(
            !artillery_group_reaches_staging(&mut routes, source_staging, source_staging, 2, None,),
            "the unused reverse scan reaches a sealed south-east slot"
        );

        assert!(connected_artillery_group_has_staging(
            &observation,
            route,
            &[ProviderDemand {
                kind: UnitKind::Bombard,
                count: 2,
            }],
            &[],
            &[],
        ));
        let future_demand = [ProviderDemand {
            kind: UnitKind::Bombard,
            count: 3,
        }];
        assert!(exact_live_provider_group(&observation, &future_demand, &[], &[]).is_none());
        assert!(
            !artillery_group_reaches_staging(&mut routes, source_staging, source_staging, 3, None,),
            "a group with a future member still tests both possible spread scans"
        );
    }

    #[test]
    fn assembly_aborts_before_moving_the_force_when_unexplored_staging_is_air_inaccessible() {
        let mut observation = obs(300);
        observation.my_units[0].tile = HOME;
        observation.known_peaks = (0..observation.map_height)
            .map(|y| TilePos::new(8, y))
            .collect();
        let intel = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Assemble, observation.tick);

        let decision = think(&mut planner, &observation, &intel);
        let operation = planner
            .air_operation()
            .expect("the failed assembly remains observable during recovery");

        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if (units.contains(&UnitId(2))
                    || units.contains(&UnitId(3))
                    || units.contains(&UnitId(4)))
                    && *goal != HOME
        )));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn ready_artillery_staging_still_recovers_when_the_final_scout_route_is_sealed() {
        let mut observation = obs(300);
        observation.my_units[0].tile = HOME;
        let staging_goal = staging(HOME, TARGET);
        explore(&mut observation, staging_goal);
        observation.known_peaks = (0..observation.map_height)
            .map(|y| TilePos::new(8, y))
            .collect();
        let mut intelligence = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Assemble, observation.tick);

        let operation = planner.air_operation().expect("assembly is active");
        assert_eq!(
            artillery_staging(
                operation,
                &observation,
                HOME,
                TARGET,
                None,
                test_orientation(),
            ),
            Some(ArtilleryStaging::Ready(staging_goal)),
            "the artillery is already across the peak wall on explored staging ground"
        );
        assert_eq!(
            scout_goal(
                operation,
                &observation,
                &intelligence,
                operation.target,
                &[],
                None,
            ),
            None,
            "the home-side scout has no admissible route into the target's vision envelope"
        );

        let failed = think(&mut planner, &observation, &intelligence);
        let operation = planner
            .air_operation()
            .expect("the failed operation remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(planner.cooldown_until() > observation.tick);
        assert!(failed.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if units.contains(&UnitId(2)) && *goal == staging_goal
        )));
        assert!(failed.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));

        observation.tick += 1;
        intelligence.update(&observation);
        think(&mut planner, &observation, &intelligence);
        assert!(planner.air_operation().is_none());
        assert_eq!(
            planner.terminal_outcome(),
            Some(AirOperationOutcome::Aborted {
                player: PlayerId(1),
                target: TARGET,
            })
        );

        observation.tick += 1;
        assert!(observation.tick < planner.cooldown_until());
        intelligence.update(&observation);
        let cooling_down = think(&mut planner, &observation, &intelligence);
        assert!(planner.air_operation().is_none());
        assert!(cooling_down.reservations.is_empty());
        assert!(cooling_down.intents.is_empty());
        assert_eq!(planner.terminal_outcome(), None);
    }

    #[test]
    fn connected_strike_refuses_an_air_route_blocked_only_in_the_public_briefing() {
        let mut observation = obs(300);
        see_approach(&mut observation);
        explore(&mut observation, staging(HOME, TARGET));
        let intelligence = knowledge(&observation);
        let public_map = public_map_with_terrain(
            &observation,
            (0..observation.map_height).map(|y| (TilePos::new(16, y), Terrain::Peak)),
        );

        let mut optimistic = with_operation(AirOperationPhase::Strike, observation.tick);
        let optimistic_decision = think(&mut optimistic, &observation, &intelligence);
        assert!(optimistic_decision.intents.iter().any(|intent| matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));

        let mut guarded = with_operation(AirOperationPhase::Strike, observation.tick);
        let guarded_decision = guarded
            .think_through_adjudication(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &observation,
                &intelligence,
                HOME,
                StrategicCoordination {
                    public_map: Some(&public_map),
                    ..coordination(None)
                },
            )
            .decision;
        let operation = guarded
            .air_operation()
            .expect("the refused strike remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(guarded_decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn scouted_water_aborts_staging_without_issuing_an_artillery_move() {
        let mut observation = obs(300);
        observation
            .my_units
            .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
        let ideal = staging(HOME, TARGET);
        let intel = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Assemble, 300);
        think(&mut planner, &observation, &intel);

        observation.tick += 1;
        observation.explored.fill(true);
        observation.known_rock = (0..observation.map_height)
            .flat_map(|y| (ideal.x - 3..=ideal.x + 3).map(move |x| TilePos::new(x, y)))
            .collect();
        let intel = knowledge(&observation);
        let decision = think(&mut planner, &observation, &intel);

        let operation = planner
            .air_operation()
            .expect("recovery remains observable");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableStaging)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if (units.contains(&UnitId(2)) || units.contains(&UnitId(5))) && *goal != HOME
        )));
    }

    #[test]
    fn explored_connected_staging_still_dispatches_the_artillery_group() {
        let mut observation = obs(300);
        observation.explored.fill(true);
        observation
            .my_units
            .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
        let intel = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Assemble, 300);

        let decision = think(&mut planner, &observation, &intel);

        assert_eq!(
            planner.air_operation().expect("operation continues").phase,
            AirOperationPhase::SuppressAa
        );
        assert!(decision.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(2)],
            goal: staging(HOME, TARGET),
        }));
    }

    #[test]
    fn artillery_operation_aborts_when_its_local_staging_line_is_known_severed() {
        let mut observation = obs(300);
        observation
            .my_units
            .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
        let ideal = staging(HOME, TARGET);
        observation.known_rock = (0..observation.map_height)
            .flat_map(|y| (ideal.x - 3..=ideal.x + 3).map(move |x| TilePos::new(x, y)))
            .collect();
        let intel = knowledge(&observation);
        let mut planner = with_operation(AirOperationPhase::Assemble, 300);

        let decision = think(&mut planner, &observation, &intel);

        let operation = planner.air_operation().unwrap();
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableStaging)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if (units.contains(&UnitId(2)) || units.contains(&UnitId(5))) && *goal != HOME
        )));

        observation.tick += 1;
        let intel = knowledge(&observation);
        think(&mut planner, &observation, &intel);
        assert!(planner.air_operation().is_none());

        observation.tick += 1;
        let intel = knowledge(&observation);
        let released = think(&mut planner, &observation, &intel);
        assert!(
            released.reservations.is_empty(),
            "a structurally unreachable objective must not hoard its roster through cooldown"
        );
    }

    #[test]
    fn known_peak_wall_aborts_scout_ingress_but_a_gap_restores_it() {
        let mut sealed = obs(300);
        sealed.my_units[0].tile = TilePos::new(4, 10);
        sealed.known_peaks = (0..sealed.map_height).map(|y| TilePos::new(8, y)).collect();
        let intel = knowledge(&sealed);
        let mut planner = with_operation(AirOperationPhase::Recon, 300);

        think(&mut planner, &sealed, &intel);
        assert_eq!(
            planner
                .air_operation()
                .and_then(|operation| operation.recovery_reason),
            Some(AirRecoveryReason::UnreachableAirRoute)
        );

        let mut open = sealed;
        open.known_peaks.retain(|tile| tile.y != 10);
        let intel = knowledge(&open);
        let mut planner = with_operation(AirOperationPhase::Recon, 300);
        let decision = think(&mut planner, &open, &intel);
        assert!(decision.intents.iter().any(|intent| matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if units == &[UnitId(1)] && goal.x > 8
        )));
    }

    #[test]
    fn known_peak_wall_blocks_bomber_commitment() {
        let mut battle = obs(400);
        see_approach(&mut battle);
        explore(&mut battle, staging(HOME, TARGET));
        battle.known_peaks = (0..battle.map_height)
            .map(|y| TilePos::new(16, y))
            .collect();
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Strike, 400);

        let decision = think(&mut planner, &battle, &intel);

        assert_eq!(
            planner
                .air_operation()
                .and_then(|operation| operation.recovery_reason),
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { units, .. } | Intent::AttackMoveUnits { units, .. }
                if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
        )));
    }

    #[test]
    fn strike_aborts_before_bomber_commitment_when_staging_recon_loses_its_air_route() {
        let mut battle = obs(400);
        battle.my_units[0].tile = HOME;
        battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Strike, battle.tick);

        let decision = think(&mut planner, &battle, &intel);
        let operation = planner
            .air_operation()
            .expect("the refused strike remains observable during recovery");

        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert_eq!(operation.strike_issued_at, None);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn recovery_releases_a_survivor_stranded_behind_known_peaks() {
        let mut battle = obs(400);
        battle.known_peaks = (0..battle.map_height)
            .map(|y| TilePos::new(16, y))
            .collect();
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Recover, 400);

        let decision = think(&mut planner, &battle, &intel);

        assert!(!decision.reservations.contains(&UnitId(1)));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, .. } if units.contains(&UnitId(1))
        )));
    }

    #[test]
    fn connected_recovery_releases_a_survivor_stranded_by_public_peaks() {
        let battle = obs(400);
        let public_map = public_map_with_terrain(
            &battle,
            (0..battle.map_height).map(|y| (TilePos::new(16, y), Terrain::Peak)),
        );
        let intel = knowledge(&battle);
        let identity = profile();
        let mut planner = with_operation(AirOperationPhase::Recover, battle.tick);

        let decision = planner.think_with_lift_support(
            &identity,
            DifficultyTuning::for_level(identity.difficulty),
            &battle,
            &intel,
            HOME,
            StrategicCoordination {
                public_map: Some(&public_map),
                ..coordination(None)
            },
        );

        assert!(!decision.reservations.contains(&UnitId(1)));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, .. } if units.contains(&UnitId(1))
        )));
    }

    #[test]
    fn attack_move_fallback_requires_current_corridor_sight() {
        let mut visible = obs(400);
        visible.enemy_buildings.clear();
        see_approach(&mut visible);
        explore(&mut visible, staging(HOME, TARGET));
        let intel = knowledge(&visible);
        let mut planner = with_operation(AirOperationPhase::Strike, 400);
        assert!(
            think(&mut planner, &visible, &intel)
                .intents
                .contains(&Intent::AttackMoveUnits {
                    units: vec![UnitId(3), UnitId(4)],
                    goal: TARGET,
                })
        );

        let mut dark = visible;
        dark.visible.fill(false);
        let intel = knowledge(&dark);
        let mut planner = with_operation(AirOperationPhase::Strike, 400);
        assert!(
            !think(&mut planner, &dark, &intel)
                .intents
                .iter()
                .any(|intent| matches!(intent, Intent::AttackMoveUnits { .. }))
        );
    }

    #[test]
    fn a_non_air_identity_retains_the_connected_operation_repertoire() {
        let mut observation = developed_connected_obs(120);
        observation.my_units.retain(|unit| unit.id != UnitId(2));
        observation
            .my_units
            .push(own(14, UnitKind::Sentinel, TilePos::new(8, 10)));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        let intel = knowledge(&observation);
        let mut identity = profile();
        identity.primary = Specialty::Support;
        identity.secondary = Specialty::Greed;
        identity.traits.air = 48;
        identity.traits.siege = 52;
        let mut planner = StrategicPlanner::new();

        let decision = planner.think(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            &[],
        );

        assert!(planner.air_operation().is_some());
        assert!(planner.air_plan().is_some_and(|plan| {
            plan.connected_package.as_ref().is_some_and(|package| {
                !package.recon.is_empty()
                    && !package.suppression.is_empty()
                    && !package.strike.is_empty()
            })
        }));
        assert!(
            !decision.reservations.is_empty() || !decision.intents.is_empty(),
            "the personality may change emphasis but cannot gate the operation"
        );
    }

    #[test]
    fn resolved_siege_identity_enters_the_playbook_with_a_complete_repertoire() {
        let mut observation = developed_connected_obs(120);
        observation.my_units.retain(|unit| unit.id != UnitId(2));
        observation
            .my_units
            .push(own(14, UnitKind::Sentinel, TilePos::new(8, 10)));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        let intel = knowledge(&observation);
        let siege = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_043,
        ));
        let low_siege = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_042,
        ));
        assert_eq!(
            (siege.primary, siege.secondary),
            (Specialty::Siege, Specialty::Guile)
        );
        assert_eq!(
            (low_siege.primary, low_siege.secondary),
            (Specialty::Support, Specialty::Greed)
        );

        let mut siege_planner = StrategicPlanner::new();
        siege_planner.think(
            &siege,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            &[],
        );
        let siege_plan = siege_planner
            .air_plan()
            .expect("the resolved Siege identity enters the connected playbook");
        assert!(
            siege_plan
                .connected_package
                .as_ref()
                .is_some_and(|package| {
                    !package.recon.is_empty()
                        && !package.suppression.is_empty()
                        && !package.strike.is_empty()
                })
        );

        let mut low_planner = StrategicPlanner::new();
        low_planner.think(
            &low_siege,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            &[],
        );
        assert!(
            low_planner.air_operation().is_some(),
            "specialty changes marginal emphasis, not access to the playbook"
        );
    }

    #[test]
    fn connected_combined_operation_waits_for_a_mature_fighting_roster() {
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Standard,
            BotStance::Balanced,
            1_616_301,
        ));
        assert_eq!(
            (identity.primary, identity.secondary),
            (Specialty::Guile, Specialty::Siege),
            "the replay-shaped identity must qualify for the combined playbook"
        );
        let mut immature = obs(120);
        immature.visible.fill(true);
        immature.explored.fill(true);
        immature.scrap = 10_000;
        immature.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), false),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), false),
            building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), false),
        ];
        immature.my_queues = vec![Vec::new(); immature.my_buildings.len()];
        immature
            .my_units
            .extend((5..=12).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
        immature.my_units.sort_unstable_by_key(|unit| unit.id);
        assert!(derived_connected_test_plan(&identity, &immature).is_some());
        assert_eq!(combat_roster(&immature), 11);

        let tuning = DifficultyTuning::for_level(BotDifficulty::Standard);
        let mut planner = StrategicPlanner::new();
        let immature_intelligence = knowledge(&immature);
        let held = planner.think(
            &identity,
            tuning,
            &immature,
            &immature_intelligence,
            HOME,
            &[],
        );

        assert_eq!(held, StrategicDecision::default());
        assert!(planner.air_operation().is_none());

        let mut mature = immature;
        mature.tick = 144;
        mature
            .my_units
            .push(own(13, UnitKind::Sentinel, TilePos::new(8, 10)));
        mature.my_units.sort_unstable_by_key(|unit| unit.id);
        assert_eq!(combat_roster(&mature), 12);
        let mature_intelligence = knowledge(&mature);
        let admitted = planner.think(&identity, tuning, &mature, &mature_intelligence, HOME, &[]);

        assert!(planner.air_operation().is_some());
        assert!(
            !admitted.reservations.is_empty() || !admitted.intents.is_empty(),
            "the mature roster should admit a complete connected package: {admitted:?}"
        );
    }

    #[test]
    fn connected_admission_tries_a_reachable_current_target_after_the_best_is_cut_off() {
        let mut battle = developed_connected_obs(120);
        let reachable = TilePos::new(12, 10);
        battle.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, TARGET, true),
            building(81, 1, BuildingKind::Foundry, reachable, true),
        ];
        let public_map = public_map_with_terrain(
            &battle,
            (0..battle.map_height).map(|y| (TilePos::new(18, y), Terrain::Peak)),
        );
        let intelligence = knowledge(&battle);
        let candidates = select_target_candidates(&intelligence, battle.tick, u64::MAX);
        assert_eq!(
            candidates.first().map(|target| target.anchor),
            Some(TARGET),
            "the disconnected Crucible remains the highest-value candidate"
        );

        let mut planner = StrategicPlanner::new();
        let result = planner.think_through_adjudication(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intelligence,
            HOME,
            StrategicCoordination {
                public_map: Some(&public_map),
                ..coordination(None)
            },
        );

        assert_eq!(
            planner.air_operation().map(|operation| operation.target),
            Some(reachable)
        );
        assert!(result.rejected_connected_candidate.is_none());
    }

    #[test]
    fn precommit_package_rebase_preserves_every_surviving_frozen_target() {
        let admitted_at = 120;
        let left_survivor = TARGET.offset(-3, 0);
        let right_survivor = TARGET.offset(3, 0);
        let mut initial = developed_connected_obs(admitted_at);
        initial.scrap = 50_000;
        initial.enemy_buildings.extend([
            building(81, 1, BuildingKind::Turret, left_survivor, true),
            building(82, 1, BuildingKind::Turret, right_survivor, true),
        ]);
        initial
            .enemy_buildings
            .sort_unstable_by_key(|building| building.id);
        let mut intelligence = knowledge(&initial);
        let initial_target = intelligence
            .buildings()
            .iter()
            .find(|building| building.anchor == TARGET)
            .expect("the original target is current");
        let initial_resources = ConnectedProductionResources::from_observation(
            &initial,
            initial_target,
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home: HOME,
                target: TARGET,
                public_map: None,
                orientation: test_orientation(),
            },
        );
        assert_eq!(
            initial_resources.targets.target_anchors,
            vec![left_survivor, TARGET, right_survivor]
        );
        assert_eq!(
            initial_resources.targets.growth_order,
            vec![left_survivor, right_survivor]
        );
        let identity = profile();
        let full_package = derive_connected_package_options_for_targets(
            &identity,
            &initial,
            initial_target,
            &[],
            &initial_resources.targets,
            ConnectedRouteContext {
                intel: &intelligence,
                home: HOME,
                target: TARGET,
                public_map: None,
                orientation: test_orientation(),
            },
            ConnectedPlanningContext {
                orientation: test_orientation(),
                public_map: None,
                resources: &initial_resources,
                preferred_artillery: &[],
                protected_current_scrap: 0,
                preparation: PreparationConstraints {
                    deadline: initial.tick.saturating_add(CONNECTED_PREPARATION_HORIZON),
                    decision_cadence: DifficultyTuning::for_level(identity.difficulty).cadence,
                    protected_forecast_scrap: 0,
                },
            },
        );
        let plan = AirPlan::connected(
            full_package
                .map(ConnectedForcePackageOptions::into_largest)
                .unwrap_or_else(|reason| {
                    panic!("the complete initial package is feasible: {reason:?}")
                }),
            admitted_at,
            TARGET,
        );
        assert_eq!(
            plan.connected_package
                .as_ref()
                .expect("the fixture begins with a connected package")
                .target_anchors,
            vec![left_survivor, TARGET, right_survivor]
        );
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation(AirOperationPhase::Assemble, admitted_at),
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };

        let mut after_destruction = initial;
        after_destruction.tick += 12;
        after_destruction
            .enemy_buildings
            .retain(|building| building.anchor != TARGET);
        let tempting_new_target = TARGET.offset(-3, 3);
        after_destruction.enemy_buildings.push(building(
            79,
            1,
            BuildingKind::Foundry,
            tempting_new_target,
            true,
        ));
        after_destruction
            .enemy_buildings
            .sort_unstable_by_key(|building| building.id);
        intelligence.update(&after_destruction);
        let decision = think(&mut planner, &after_destruction, &intelligence);

        let active = planner
            .air
            .as_ref()
            .expect("the surviving admitted target keeps preparation active");
        assert_ne!(active.op.phase, AirOperationPhase::Recover);
        assert_eq!(active.op.recovery_reason, None);
        assert_eq!(active.op.target, left_survivor);
        assert_eq!(active.op.target_kind, BuildingKind::Turret);
        assert_eq!(active.op.target_id, Some(BuildingId(81)));
        let revised = active
            .plan
            .connected_package
            .as_ref()
            .expect("the surviving target retains a connected package");
        assert_eq!(revised.derived_at, after_destruction.tick);
        assert_eq!(revised.target_anchors, vec![left_survivor, right_survivor]);
        assert!(
            !revised.target_anchors.contains(&tempting_new_target),
            "revision may prune the frozen set but cannot regrow it around the new primary"
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn adjudicated_precommit_rebase_preserves_exact_package_and_surviving_targets() {
        let admitted_at = 120;
        let left_survivor = TARGET.offset(-3, 0);
        let right_survivor = TARGET.offset(3, 0);
        let tempting_new_target = TARGET.offset(-3, 3);
        let mut initial = production_hungry_connected_obs(admitted_at, 50_000);
        initial.enemy_buildings.extend([
            building(81, 1, BuildingKind::Turret, left_survivor, true),
            building(82, 1, BuildingKind::Turret, right_survivor, true),
        ]);
        initial
            .enemy_buildings
            .sort_unstable_by_key(|building| building.id);
        let mut intelligence = knowledge(&initial);
        let resources = ResourceSnapshot::from_observation(&initial);
        let mut proposal = StrategicPlanner::new()
            .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &initial,
                &resources,
                &intelligence,
                HOME,
                coordination(None),
            ))
            .expect("the complete current cluster is admissible")
            .expect("the current cluster produces an exact proposal");
        let richest = proposal.marginal_variants().last().cloned();
        if let Some(richest) = richest.as_ref() {
            assert!(proposal.select_marginal(richest));
        }
        assert_eq!(
            proposal.variants[proposal.selected_variant]
                .active
                .plan
                .connected_package
                .as_ref()
                .expect("the proposal owns an exact package")
                .target_anchors,
            vec![left_survivor, TARGET, right_survivor]
        );
        let assignments = selected_connected_assignments(&proposal);
        proposal
            .bind_producer_assignments(assignments)
            .expect("the selected package binds its exact producer schedule");
        let mut expected_package = proposal.variants[proposal.selected_variant]
            .active
            .plan
            .connected_package
            .clone()
            .expect("the selected proposal retains its exact package");
        let mut planner = StrategicPlanner::new();
        planner
            .commit_connected_proposal(proposal)
            .expect("the exact connected proposal commits");
        let _ = planner.think_after_connected_adjudication(StrategicThinkContext::new(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &initial,
            &intelligence,
            HOME,
            StrategicCoordination {
                allow_new_operation: false,
                ..coordination(None)
            },
        ));

        let mut after_destruction = initial;
        after_destruction.tick += 12;
        after_destruction
            .enemy_buildings
            .retain(|building| building.anchor != TARGET);
        after_destruction.enemy_buildings.push(building(
            79,
            1,
            BuildingKind::Foundry,
            tempting_new_target,
            true,
        ));
        after_destruction
            .enemy_buildings
            .sort_unstable_by_key(|building| building.id);
        intelligence.update(&after_destruction);
        let decision = planner.think_after_connected_adjudication(StrategicThinkContext::new(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &after_destruction,
            &intelligence,
            HOME,
            StrategicCoordination {
                allow_new_operation: false,
                ..coordination(None)
            },
        ));

        let rebased_identity = ConnectedOffenseIdentity::new(BuildingId(81), left_survivor);
        expected_package.target_anchors = vec![left_survivor, right_survivor];
        for assignment in expected_package
            .producer_assignments
            .as_mut()
            .expect("the committed package retains its bound producer schedule")
        {
            assignment.owner = rebased_identity;
        }
        let active = planner
            .air
            .as_ref()
            .expect("the surviving frozen targets keep preparation active");
        assert_ne!(active.op.phase, AirOperationPhase::Recover);
        assert_eq!(active.op.recovery_reason, None);
        assert_eq!(active.op.target, left_survivor);
        assert_eq!(active.op.target_kind, BuildingKind::Turret);
        assert_eq!(active.op.target_id, Some(BuildingId(81)));
        assert_eq!(
            active.plan.connected_package.as_ref(),
            Some(&expected_package),
            "adjudicated continuation may prune destroyed targets and rebind owner identity, but cannot rerank or rederive its exact package"
        );
        assert!(
            !expected_package
                .target_anchors
                .contains(&tempting_new_target),
            "revision cannot admit a newly observed target near the rebased primary"
        );
        let obligation = planner
            .active_connected_obligation(&after_destruction)
            .expect("the rebased operation retains its persistent obligation");
        assert_eq!(obligation.identity(), rebased_identity);
        assert!(
            obligation
                .provider_jobs()
                .iter()
                .all(|assignment| assignment.owner == rebased_identity)
        );
        assert!(decision.decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn fresh_connected_proposal_is_pure_repeatable_and_keeps_one_minimum_basis() {
        let battle = production_hungry_connected_obs(120, 10_000);
        let intelligence = knowledge(&battle);
        let resources = ResourceSnapshot::from_observation(&battle);
        let planner = StrategicPlanner::new();
        let before = planner.clone();
        let propose = || {
            planner
                .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                    &profile(),
                    DifficultyTuning::for_level(BotDifficulty::Prime),
                    &battle,
                    &resources,
                    &intelligence,
                    HOME,
                    coordination(None),
                ))
                .expect("the current connected opportunity is admissible")
                .expect("the current connected opportunity produces a proposal")
        };

        let first = propose();
        let second = propose();

        assert_eq!(first, second);
        assert_eq!(
            planner, before,
            "proposal derivation must not mutate planner state"
        );
        assert_eq!(first.objective(), BuildingId(80));
        assert_eq!(first.anchor(), TARGET);
        assert_eq!(
            first.deadline(),
            battle.tick.saturating_add(connected_preparation_horizon())
        );
        let minimum = &first.variants[0];
        let minimum_package = minimum
            .active
            .plan
            .connected_package
            .as_ref()
            .expect("every proposal variant has a connected package");
        assert!(!minimum.claims.provider_jobs.is_empty());
        for variant in &first.variants {
            let package = variant
                .active
                .plan
                .connected_package
                .as_ref()
                .expect("every proposal variant has a connected package");
            assert_eq!(package.target_anchors, minimum_package.target_anchors);
            assert_eq!(
                package.preparation_deadline,
                minimum_package.preparation_deadline
            );
            assert!(
                package
                    .provider_priority
                    .starts_with(&minimum_package.provider_priority)
            );
            assert!(variant.claims.units.starts_with(&minimum.claims.units));
            assert!(
                variant
                    .claims
                    .provider_jobs
                    .starts_with(&minimum.claims.provider_jobs)
            );
        }
    }

    #[test]
    fn reacquired_remembered_target_requires_fresh_connected_adjudication() {
        let reveal_ground_route = |observation: &mut Observation| {
            observation.known_rock.clear();
            observation.my_buildings.push(building(
                40,
                0,
                BuildingKind::Fabricator,
                TilePos::new(12, 2),
                true,
            ));
            observation.my_queues.push(Vec::new());
            for x in HOME.x + 2..TARGET.x {
                explore(observation, TilePos::new(x, HOME.y));
            }
        };
        let first = {
            let mut observation = wealthy_island_obs(4_800, 1);
            reveal_ground_route(&mut observation);
            observation
        };
        let mut intelligence = knowledge(&first);
        let mut hidden = wealthy_island_obs(4_992, 1);
        reveal_ground_route(&mut hidden);
        hidden.enemy_buildings[0].seen = false;
        hidden.my_units[0].tile = HOME;
        intelligence.update(&hidden);
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_045,
        ));
        let tuning = DifficultyTuning::for_level(identity.difficulty);
        let mut planner = StrategicPlanner::new();
        planner.think(&identity, tuning, &hidden, &intelligence, HOME, &[]);
        assert!(planner.air_operation().is_some_and(|operation| {
            !operation.assault_admitted && operation.target_id == Some(BuildingId(80))
        }));

        let mut current = wealthy_island_obs(5_016, 1);
        reveal_ground_route(&mut current);
        intelligence.update(&current);
        let resources = ResourceSnapshot::from_observation(&current);
        let before_proposal = planner.clone();
        let mut proposal = planner
            .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                &identity,
                tuning,
                &current,
                &resources,
                &intelligence,
                HOME,
                coordination(None),
            ))
            .expect("the reacquired connected objective is admissible")
            .expect("reacquisition produces a fresh proposal");
        assert_eq!(proposal.accepted_at(), hidden.tick);
        assert_eq!(planner, before_proposal);

        let mut without_acceptance = planner.clone();
        without_acceptance.think_after_connected_adjudication(StrategicThinkContext::new(
            &identity,
            tuning,
            &current,
            &intelligence,
            HOME,
            coordination(None),
        ));
        assert!(without_acceptance.air_operation().is_some_and(|operation| {
            !operation.assault_admitted && operation.target_id == Some(BuildingId(80))
        }));

        let assignments = selected_connected_assignments(&proposal);
        proposal
            .bind_producer_assignments(assignments)
            .expect("the accepted proposal binds its exact production schedule");
        planner
            .commit_connected_proposal(proposal)
            .expect("the unchanged remembered origin accepts the proposal");
        assert!(planner.air_operation().is_some_and(|operation| {
            operation.assault_admitted && operation.target_id == Some(BuildingId(80))
        }));
        assert_eq!(planner.air_admitted_at(), Some(hidden.tick));
        assert_eq!(
            planner
                .active_connected_obligation(&current)
                .expect("the promoted assault exports its persistent claims")
                .accepted_at(),
            hidden.tick,
            "persistent allocation must retain reconnaissance admission rather than promotion time"
        );
    }

    #[test]
    fn fresh_connected_proposal_uses_the_coordinators_exact_resource_snapshot() {
        let battle = production_hungry_connected_obs(120, 10_000);
        let intelligence = knowledge(&battle);
        let mut unfunded_evidence = battle.clone();
        unfunded_evidence.scrap = 0;
        let resources = ResourceSnapshot::from_observation(&unfunded_evidence);

        let result = StrategicPlanner::new().fresh_connected_minimum_proposal(
            FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                coordination(None),
            ),
        );

        assert!(
            result.is_err(),
            "strategy must not reconstruct the rich bank from Observation behind the coordinator"
        );
    }

    #[test]
    fn fresh_connected_proposal_falls_back_without_committing_the_rejected_target() {
        let mut battle = production_hungry_connected_obs(120, 10_000);
        let reachable = TilePos::new(12, 10);
        battle.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, TARGET, true),
            building(81, 1, BuildingKind::Foundry, reachable, true),
        ];
        let public_map = public_map_with_terrain(
            &battle,
            (0..battle.map_height).map(|y| (TilePos::new(18, y), Terrain::Peak)),
        );
        let intelligence = knowledge(&battle);
        let resources = ResourceSnapshot::from_observation(&battle);
        let planner = StrategicPlanner::new();
        let before = planner.clone();

        let proposal = planner
            .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                StrategicCoordination {
                    public_map: Some(&public_map),
                    ..coordination(None)
                },
            ))
            .expect("the lower-ranked reachable target remains admissible")
            .expect("the lower-ranked reachable target produces a proposal");

        assert_eq!(proposal.objective(), BuildingId(81));
        assert_eq!(proposal.anchor(), reachable);
        assert_eq!(planner, before);
    }

    #[test]
    fn connected_proposal_uses_completed_income_without_double_counting_provider_costs() {
        let mut battle = production_hungry_connected_obs(120, 0);
        for (id, anchor) in [(20, TilePos::new(2, 15)), (21, TilePos::new(8, 15))] {
            battle
                .my_buildings
                .push(building(id, 0, BuildingKind::Extractor, anchor, true));
            battle.my_queues.push(Vec::new());
        }
        let intelligence = knowledge(&battle);
        let resources = ResourceSnapshot::from_observation(&battle);
        let proposal = StrategicPlanner::new()
            .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                coordination(None),
            ))
            .expect("completed Extractors make the minimum forecast-feasible")
            .expect("the forecast-funded operation produces a proposal");
        let minimum = &proposal.variants[0];
        let package = minimum
            .active
            .plan
            .connected_package
            .as_ref()
            .expect("the proposal retains its exact package");

        assert_eq!(
            minimum.claims.provider_jobs.len(),
            package.funded_providers.len()
        );
        assert!(
            minimum
                .claims
                .provider_jobs
                .iter()
                .any(|job| job.enqueue_not_before > battle.tick),
            "zero current bank requires at least one forecast-funded command boundary"
        );
        assert_eq!(
            minimum
                .claims
                .provider_jobs
                .iter()
                .map(|job| job.kind.stats().cost)
                .sum::<u32>(),
            package
                .funded_providers
                .iter()
                .map(|provider| provider.kind.stats().cost)
                .sum::<u32>(),
            "every unpaid provider cost appears exactly once in the shared claim surface"
        );
        for (job, funded) in minimum
            .claims
            .provider_jobs
            .iter()
            .zip(&package.funded_providers)
        {
            assert_eq!(
                (job.kind, job.enqueue_not_before),
                (funded.kind, funded.command_tick)
            );
        }
    }

    #[test]
    fn protected_current_scrap_monotonically_reduces_connected_scaling() {
        let mut rich = production_hungry_connected_obs(120, 10_000);
        rich.enemy_buildings.extend([
            building(81, 1, BuildingKind::Foundry, TARGET.offset(-2, -2), true),
            building(82, 1, BuildingKind::Airworks, TARGET.offset(-2, 2), true),
            building(83, 1, BuildingKind::FlakTurret, TARGET.offset(-4, 0), true),
        ]);
        let intelligence = knowledge(&rich);
        let rich_resources = ResourceSnapshot::from_observation(&rich);
        let rich_proposal = StrategicPlanner::new()
            .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &rich,
                &rich_resources,
                &intelligence,
                HOME,
                coordination(None),
            ))
            .expect("the rich opportunity is admissible")
            .expect("the rich opportunity produces a proposal");
        let richest_cost = rich_proposal
            .variants
            .last()
            .expect("the minimum is always present")
            .claims
            .provider_jobs
            .iter()
            .map(|job| job.kind.stats().cost)
            .sum::<u32>();
        let minimum_cost = rich_proposal
            .minimum_claims()
            .provider_jobs()
            .iter()
            .map(|job| job.kind().stats().cost)
            .sum::<u32>();
        assert!(richest_cost > minimum_cost);

        rich.scrap = richest_cost;
        let exact_resources = ResourceSnapshot::from_observation(&rich);
        let derive_with_reserve = |reserve| {
            StrategicPlanner::new().fresh_connected_minimum_proposal(
                FreshConnectedProposalRequest::new(
                    &profile(),
                    DifficultyTuning::for_level(BotDifficulty::Prime),
                    &rich,
                    &exact_resources,
                    &intelligence,
                    HOME,
                    StrategicCoordination {
                        protected_current_scrap: reserve,
                        ..coordination(None)
                    },
                ),
            )
        };
        let unreserved = derive_with_reserve(0)
            .expect("the exact full-package bank is admissible")
            .expect("the exact full-package bank produces a proposal");
        let minimum_only = derive_with_reserve(richest_cost - minimum_cost)
            .expect("protecting only optional capital preserves the minimum")
            .expect("the retained minimum still produces a proposal");

        assert_eq!(minimum_only.minimum_claims(), unreserved.minimum_claims());
        assert!(minimum_only.variants.len() < unreserved.variants.len());
        assert_eq!(minimum_only.variants.len(), 1);
        assert!(derive_with_reserve(richest_cost).is_err());
    }

    #[test]
    fn adjudicated_connected_operation_keeps_exact_target_producers_and_obligations() {
        let mut battle = production_hungry_connected_obs(120, 10_000);
        battle.my_buildings.push(building(
            13,
            0,
            BuildingKind::Airworks,
            TilePos::new(12, 2),
            true,
        ));
        battle.my_queues.push(Vec::new());
        let mut intelligence = knowledge(&battle);
        let resources = ResourceSnapshot::from_observation(&battle);
        let mut proposal = StrategicPlanner::new()
            .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                coordination(None),
            ))
            .expect("the production-heavy connected operation is admissible")
            .expect("the operation produces an exact proposal");
        let mut assignments = selected_connected_assignments(&proposal);
        let alternate_index = proposal.variants[proposal.selected_variant]
            .claims
            .provider_jobs
            .iter()
            .position(|job| job.eligible_producers.len() > 1)
            .expect("the two Airworks give one provider an alternate exact lane");
        let alternate_producer = *proposal.variants[proposal.selected_variant]
            .claims
            .provider_jobs[alternate_index]
            .eligible_producers
            .last()
            .expect("the selected job has an alternate producer");
        assignments[alternate_index].producer = alternate_producer;
        let alternate_timing = assignments[alternate_index].timing;
        let delayed_by = 12;
        assignments[alternate_index].timing = ConnectedProducerTiming::new(
            alternate_timing.enqueued_at.saturating_add(delayed_by),
            alternate_timing.starts_at.saturating_add(delayed_by),
            alternate_timing.ready_at.saturating_add(delayed_by),
            alternate_timing.ready_before,
        );
        assignments[alternate_index].funding =
            ConnectedProducerFunding::new(0, assignments[alternate_index].kind.stats().cost);

        let mut unbound_planner = StrategicPlanner::new();
        assert_eq!(
            unbound_planner.commit_connected_proposal(proposal.clone()),
            Err(ConnectedProposalCommitError::UnboundProducerSchedule)
        );
        assert_eq!(unbound_planner, StrategicPlanner::new());

        proposal
            .bind_producer_assignments(assignments.clone())
            .expect("the allocator-selected alternate lane is valid");
        let identity = proposal.identity();
        let deadline = proposal.deadline();
        let mut planner = StrategicPlanner::new();
        planner
            .commit_connected_proposal(proposal)
            .expect("the bound proposal commits without recomputation");

        let obligation = planner
            .active_connected_obligation(&battle)
            .expect("the accepted operation becomes a mandatory obligation");
        assert_eq!(obligation.identity(), identity);
        assert_eq!(obligation.accepted_at(), battle.tick);
        assert_eq!(obligation.deadline(), deadline);
        assert_eq!(obligation.provider_jobs(), assignments);

        let mut reattributed = assignments.clone();
        reattributed[alternate_index].funding =
            ConnectedProducerFunding::new(reattributed[alternate_index].kind.stats().cost, 0);
        planner
            .refresh_active_connected_funding(&obligation, &reattributed)
            .expect("matured forecast funding may become current without moving the job");
        assignments = reattributed;
        let refreshed_obligation = planner
            .active_connected_obligation(&battle)
            .expect("the refreshed operation remains a mandatory obligation");
        assert_eq!(refreshed_obligation.provider_jobs(), assignments);

        let before_invalid_refresh = planner.clone();
        let mut retimed = assignments.clone();
        retimed[alternate_index].timing.enqueued_at = retimed[alternate_index]
            .timing
            .enqueued_at
            .saturating_sub(1);
        assert_eq!(
            planner.refresh_active_connected_funding(&refreshed_obligation, &retimed),
            Err(ConnectedProducerBindingError::Timing {
                request_ordinal: alternate_index,
            })
        );
        assert_eq!(
            planner, before_invalid_refresh,
            "a funding refresh cannot move an accepted legal job to another legal tick"
        );

        let mut same_tick_queue = battle.clone();
        let first = assignments[0];
        same_tick_queue.tick = first.timing.enqueued_at;
        let producer_index = same_tick_queue
            .my_buildings
            .iter()
            .position(|building| building.id == first.producer)
            .expect("the bound producer remains in the observation");
        same_tick_queue.my_queues[producer_index].push(first.kind);
        let same_tick_jobs = planner
            .active_connected_obligation(&same_tick_queue)
            .expect("the command tick is still unpaid before commands execute")
            .provider_jobs()
            .to_vec();
        assert_eq!(
            same_tick_jobs, assignments,
            "an unrelated same-kind queue cannot consume an exact connected job"
        );
        assert!(same_tick_jobs.contains(&first));

        let active = planner.air.as_ref().expect("the operation is committed");
        let alternate = assignments[alternate_index];
        let exact_intent = Intent::TrainAt {
            building: alternate_producer,
            kind: alternate.kind,
        };
        let mut early_observation = battle.clone();
        early_observation.tick = alternate.timing.enqueued_at.saturating_sub(1);
        let early_intelligence = knowledge(&early_observation);
        let identity_profile = profile();
        let mut early = StrategicDecision::default();
        let mut early_plan = active.plan.clone();
        schedule_missing_members(
            &active.op,
            &mut early_plan,
            &planning_context(&identity_profile, &early_observation, &early_intelligence),
            UnitKind::Kestrel,
            &mut early,
        );
        assert!(
            !early.intents.contains(&exact_intent),
            "an earlier legal lane slot must not move the accepted enqueue tick: {early:?}"
        );

        let mut scheduled_observation = battle.clone();
        scheduled_observation.tick = alternate.timing.enqueued_at;
        let scheduled_intelligence = knowledge(&scheduled_observation);
        let mut scheduled = StrategicDecision::default();
        let operation = planner
            .air
            .as_ref()
            .expect("the operation is committed")
            .op
            .clone();
        let active = planner.air.as_mut().expect("the operation is committed");
        schedule_missing_members(
            &operation,
            &mut active.plan,
            &planning_context(
                &identity_profile,
                &scheduled_observation,
                &scheduled_intelligence,
            ),
            UnitKind::Kestrel,
            &mut scheduled,
        );
        assert!(
            scheduled.intents.contains(&exact_intent),
            "a due provider must lower through its allocator-selected producer: {scheduled:?}"
        );
        let remaining_after_issue = planner
            .active_connected_obligation(&scheduled_observation)
            .expect("the operation remains admitted after issuing one provider")
            .provider_jobs()
            .to_vec();
        assert!(
            remaining_after_issue
                .iter()
                .all(|assignment| assignment.timing.enqueued_at > scheduled_observation.tick),
            "issued provider commands leave the persistent unpaid schedule"
        );
        let committed_package = planner
            .air
            .as_ref()
            .and_then(|active| active.plan.connected_package.clone())
            .expect("the exact remaining package schedule stays committed");

        let preferred = TilePos::new(14, 4);
        battle
            .enemy_buildings
            .push(building(79, 1, BuildingKind::Crucible, preferred, true));
        intelligence.update(&battle);
        assert_eq!(
            select_target_candidates(&intelligence, battle.tick, u64::MAX)
                .first()
                .and_then(|target| target.id),
            Some(BuildingId(79)),
            "the new contact would win a fresh target ranking"
        );
        planner.think_after_connected_adjudication(StrategicThinkContext::new(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intelligence,
            HOME,
            StrategicCoordination {
                allow_new_operation: false,
                ..coordination(None)
            },
        ));
        let active = planner
            .air
            .as_ref()
            .expect("fresh arbitration being closed does not stop mandatory continuation");
        assert_eq!(active.op.target_id, Some(BuildingId(80)));
        assert_eq!(
            active.plan.connected_package.as_ref(),
            Some(&committed_package),
            "continuation cannot rerank or rederive the accepted package"
        );
        assert!(
            planner
                .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                    &profile(),
                    DifficultyTuning::for_level(BotDifficulty::Prime),
                    &battle,
                    &resources,
                    &intelligence,
                    HOME,
                    coordination(None),
                ))
                .expect("an active operation is not a proposal error")
                .is_none(),
            "an active admitted operation bypasses fresh arbitration"
        );
    }

    #[test]
    fn rolled_back_connected_provider_commands_retry_until_their_transaction_commits() {
        fn issue_due_providers(
            planner: &mut StrategicPlanner,
            observation: &Observation,
            identity_profile: &ResolvedProfile,
        ) -> StrategicDecision {
            let intelligence = knowledge(observation);
            let operation = planner
                .air
                .as_ref()
                .expect("the connected operation remains active")
                .op
                .clone();
            let active = planner
                .air
                .as_mut()
                .expect("the connected operation remains active");
            let mut decision = StrategicDecision::default();
            schedule_missing_members(
                &operation,
                &mut active.plan,
                &planning_context(identity_profile, observation, &intelligence),
                UnitKind::Kestrel,
                &mut decision,
            );
            decision
        }

        let base = production_hungry_connected_obs(120, 10_000);
        let intelligence = knowledge(&base);
        let resources = ResourceSnapshot::from_observation(&base);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut proposal = StrategicPlanner::new()
            .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                &profile(),
                tuning,
                &base,
                &resources,
                &intelligence,
                HOME,
                coordination(None),
            ))
            .expect("the connected opportunity is admissible")
            .expect("the connected opportunity has provider work");
        let assignments = selected_connected_assignments(&proposal);
        proposal
            .bind_producer_assignments(assignments.clone())
            .expect("the fixture binds its allocator-selected schedule");
        let mut planner = StrategicPlanner::new();
        planner
            .commit_connected_proposal(proposal)
            .expect("the bound connected operation commits");
        let before_failed_transaction = planner.clone();
        let first_due = assignments
            .iter()
            .map(|assignment| assignment.timing.enqueued_at)
            .min()
            .expect("the connected package retains at least one provider job");
        let expected_at = |tick| {
            assignments
                .iter()
                .filter(|assignment| assignment.timing.enqueued_at <= tick)
                .map(|assignment| Intent::TrainAt {
                    building: assignment.producer,
                    kind: assignment.kind,
                })
                .collect::<Vec<_>>()
        };
        let identity_profile = profile();
        let mut due_observation = base.clone();
        due_observation.tick = first_due;

        let failed_issue = issue_due_providers(&mut planner, &due_observation, &identity_profile);
        assert_eq!(failed_issue.intents, expected_at(first_due));
        assert!(
            planner
                .active_connected_obligation(&due_observation)
                .expect("issuing commands does not end the active operation")
                .provider_jobs()
                .iter()
                .all(|assignment| assignment.timing.enqueued_at > first_due),
            "the speculative planner records due provider ordinals as issued"
        );

        planner = before_failed_transaction;
        let retry_tick = first_due.saturating_add(tuning.cadence);
        let mut retry_observation = base;
        retry_observation.tick = retry_tick;
        let retry = issue_due_providers(&mut planner, &retry_observation, &identity_profile);
        assert_eq!(
            retry.intents,
            expected_at(retry_tick),
            "restoring the transaction snapshot must make every still-due exact provider command retryable"
        );
        assert!(
            failed_issue
                .intents
                .iter()
                .all(|intent| retry.intents.contains(intent)),
            "a command suppressed by rollback cannot age out before the next cadence"
        );

        let after_commit = issue_due_providers(&mut planner, &retry_observation, &identity_profile);
        assert!(
            after_commit.intents.is_empty(),
            "without another rollback, an accepted exact provider command emits only once"
        );
    }

    #[test]
    fn connected_producer_binding_rejects_a_schedule_from_another_objective() {
        let battle = production_hungry_connected_obs(120, 10_000);
        let intelligence = knowledge(&battle);
        let resources = ResourceSnapshot::from_observation(&battle);
        let mut proposal = StrategicPlanner::new()
            .fresh_connected_minimum_proposal(FreshConnectedProposalRequest::new(
                &profile(),
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &resources,
                &intelligence,
                HOME,
                coordination(None),
            ))
            .expect("the connected operation is admissible")
            .expect("the operation has unpaid jobs");
        let mut assignments = selected_connected_assignments(&proposal);
        assignments[0].owner = ConnectedOffenseIdentity::new(BuildingId(999), TARGET);

        assert_eq!(
            proposal.bind_producer_assignments(assignments),
            Err(ConnectedProducerBindingError::Owner { request_ordinal: 0 })
        );
    }

    #[test]
    fn connected_admission_reports_the_best_current_target_when_every_candidate_is_rejected() {
        let mut battle = developed_connected_obs(120);
        let lower_value = TARGET.offset(0, 5);
        battle.map_height = 26;
        battle.visible = vec![true; usize::try_from(battle.map_width * battle.map_height).unwrap()];
        battle.explored = battle.visible.clone();
        battle.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, TARGET, true),
            building(81, 1, BuildingKind::Foundry, lower_value, true),
        ];
        let public_map = public_map_with_terrain(
            &battle,
            (0..battle.map_height).map(|y| (TilePos::new(18, y), Terrain::Peak)),
        );
        let intelligence = knowledge(&battle);
        let mut planner = StrategicPlanner::new();

        let result = planner.think_through_adjudication(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intelligence,
            HOME,
            StrategicCoordination {
                public_map: Some(&public_map),
                ..coordination(None)
            },
        );

        let rejected = result
            .rejected_connected_candidate
            .expect("the best rejected candidate remains diagnostic evidence");
        assert_eq!(rejected.target.anchor, TARGET);
        assert_eq!(
            rejected.reason,
            ConnectedPlanRejection::DisconnectedGroundRoute
        );
        assert!(planner.air_operation().is_none());
    }

    #[test]
    fn current_connected_candidate_reports_the_standing_force_gate_only_at_admission() {
        let mut on_boundary = obs(120);
        see_approach(&mut on_boundary);
        let current = combat_roster(&on_boundary);
        assert!(current < CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER);
        let intelligence = knowledge(&on_boundary);
        let mut planner = StrategicPlanner::new();

        let rejected = planner.think_through_adjudication(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &on_boundary,
            &intelligence,
            HOME,
            coordination(None),
        );

        assert_eq!(rejected.decision, StrategicDecision::default());
        assert_eq!(
            rejected.rejected_connected_candidate,
            Some(RejectedConnectedCandidate {
                target: intelligence.buildings()[0].clone(),
                reason: ConnectedPlanRejection::InsufficientStandingForce {
                    current,
                    required: CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER,
                },
            })
        );
        assert!(planner.air_operation().is_none());

        let mut off_boundary = on_boundary;
        off_boundary.tick = 121;
        let intelligence = knowledge(&off_boundary);
        let mut planner = StrategicPlanner::new();
        let not_considered = planner.think_through_adjudication(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &off_boundary,
            &intelligence,
            HOME,
            coordination(None),
        );
        assert!(not_considered.rejected_connected_candidate.is_none());
        assert!(planner.air_operation().is_none());
    }

    #[test]
    fn no_target_is_idle_without_fabricating_a_connected_rejection() {
        let mut observation = developed_connected_obs(120);
        observation.enemy_buildings.clear();
        let intelligence = knowledge(&observation);
        let mut planner = StrategicPlanner::new();

        let result = planner.think_through_adjudication(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination(None),
        );

        assert_eq!(result, StrategicThinkResult::default());
        assert!(planner.air_operation().is_none());
    }

    #[test]
    fn current_connected_candidate_reports_a_disconnected_ground_route() {
        let mut observation = developed_connected_obs(120);
        observation.known_rock = (0..observation.map_height)
            .map(|y| TilePos::new(16, y))
            .collect();
        let intelligence = knowledge(&observation);
        let target = intelligence.buildings()[0].clone();
        let mut planner = StrategicPlanner::new();

        let result = planner.think_through_adjudication(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination(None),
        );

        assert_eq!(
            result.rejected_connected_candidate,
            Some(RejectedConnectedCandidate {
                target,
                reason: ConnectedPlanRejection::DisconnectedGroundRoute,
            })
        );
        assert!(planner.air_operation().is_none());
    }

    #[test]
    fn current_connected_candidate_reports_a_missing_completed_provider() {
        let mut observation = developed_connected_obs(120);
        observation.my_buildings.clear();
        observation.my_queues.clear();
        let intelligence = knowledge(&observation);
        let target = intelligence.buildings()[0].clone();
        let mut planner = StrategicPlanner::new();
        let enlisted = [UnitId(1), UnitId(2), UnitId(3), UnitId(4)];

        let result = planner.think_through_adjudication(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            StrategicCoordination {
                enlisted: &enlisted,
                ..coordination(None)
            },
        );

        assert_eq!(
            result.rejected_connected_candidate,
            Some(RejectedConnectedCandidate {
                target,
                reason: ConnectedPlanRejection::Package {
                    reason: ForcePackageRejection::MissingCompletedProviderCapability {
                        family: force_package::ForceFamily::Recon,
                    },
                    protected_current_scrap: 0,
                    protected_forecast_scrap: 0,
                },
            })
        );
        assert!(planner.air_operation().is_none());
    }

    #[test]
    fn connected_production_rejects_a_route_beyond_the_movement_search_cap() {
        let mut observation = obs(120);
        observation.map_width = 256;
        observation.map_height = 256;
        observation.visible = vec![true; 256 * 256];
        observation.explored = observation.visible.clone();
        observation.my_units.clear();
        observation.my_buildings = vec![building(
            10,
            0,
            BuildingKind::Fabricator,
            TilePos::new(253, 253),
            true,
        )];
        observation.my_queues = vec![Vec::new()];
        observation.enemy_buildings.clear();
        let open = |tile: TilePos| {
            tile.y % 2 == 0
                || (tile.y % 4 == 1 && tile.x == observation.map_width - 1)
                || (tile.y % 4 == 3 && tile.x == 0)
                || tile == TilePos::new(255, 255)
        };
        let public_map = public_map_with_terrain(
            &observation,
            (0..observation.map_height).flat_map(|y| {
                (0..observation.map_width).filter_map(move |x| {
                    let tile = TilePos::new(x, y);
                    (!open(tile)).then_some((tile, Terrain::Pit))
                })
            }),
        );
        let home = TilePos::new(0, 84);
        let target = TilePos::new(3, 84);
        let orientation = Orientation::for_home(&observation, home);
        assert!(orientation.is_identity());
        let intelligence = knowledge(&observation);
        let targets = ConnectedTargetSelection {
            target_anchors: vec![target],
            suppression_targets: Vec::new(),
            growth_order: Vec::new(),
        };
        let resources = ResourceSnapshot::from_observation(&observation);
        let producer = &observation.my_buildings[0];
        let spawn =
            production_spawn_doorstep(&observation, producer, Some(&public_map), orientation)
                .expect("the Fabricator has an open south-east doorstep");
        let staging =
            connected_artillery_staging_goal(&observation, home, target, Some(&public_map))
                .expect("the Foundry-side staging tile is in the same component");
        let mut projected = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&public_map),
            orientation,
        );
        assert!(
            projected.reaches(spawn, staging),
            "the uncapped component flood sees the complete serpentine"
        );
        assert!(
            !projected.ground_command_reaches(spawn, staging),
            "the movement command cannot traverse that component within its bounded A* search"
        );

        let access = connected_production_access(
            &observation,
            &targets,
            &resources,
            ConnectedRouteContext {
                intel: &intelligence,
                home,
                target,
                public_map: Some(&public_map),
                orientation,
            },
        );

        assert!(
            !access.allows(BuildingId(10), UnitKind::Bombard),
            "an operation must not buy artillery whose authoritative route will exhaust"
        );
    }

    #[test]
    fn connected_operation_excludes_live_artillery_beyond_the_movement_search_cap() {
        let mut observation = obs(120);
        observation.map_width = 256;
        observation.map_height = 256;
        observation.visible = vec![true; 256 * 256];
        observation.explored = observation.visible.clone();
        observation.my_units = vec![own(2, UnitKind::Bombard, TilePos::new(255, 255))];
        observation.my_buildings.clear();
        observation.my_queues.clear();
        observation.enemy_buildings.clear();
        let public_map = movement_cap_serpentine_map(&observation);
        let intelligence = knowledge(&observation);
        let home = TilePos::new(0, 84);
        let target = TilePos::new(3, 84);
        let staging =
            connected_artillery_staging_goal(&observation, home, target, Some(&public_map))
                .expect("the Foundry-side staging tile is in the same component");
        let mut routes = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&public_map),
            test_orientation(),
        );
        assert!(
            routes.reaches(observation.my_units[0].tile, staging),
            "the uncapped component flood sees the complete serpentine"
        );
        assert!(
            !routes.ground_command_reaches(observation.my_units[0].tile, staging),
            "the live Bombard's eventual movement command exhausts its bounded search"
        );

        let unavailable = connected_provider_unavailable(
            &observation,
            &ConnectedTargetSelection {
                target_anchors: vec![target],
                suppression_targets: Vec::new(),
                growth_order: Vec::new(),
            },
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home,
                target,
                public_map: Some(&public_map),
                orientation: test_orientation(),
            },
        );

        assert_eq!(
            unavailable,
            vec![UnitId(2)],
            "a live provider must not be admitted when its staging command will exhaust"
        );
    }

    #[test]
    fn connected_cluster_rejects_suppression_whose_staging_route_exceeds_the_command_cap() {
        let primary = TilePos::new(240, 251);
        let secondary = TilePos::new(244, 251);
        let flak = TilePos::new(249, 251);
        let home = TilePos::new(0, 0);
        let mut observation = obs(120);
        observation.map_width = 256;
        observation.map_height = 256;
        observation.visible = vec![true; 256 * 256];
        observation.explored = observation.visible.clone();
        observation.my_units = vec![
            own(1, UnitKind::Kestrel, TilePos::new(255, 254)),
            own(2, UnitKind::Bombard, TilePos::new(255, 255)),
            own(3, UnitKind::Condor, TilePos::new(254, 254)),
        ];
        observation.my_buildings.clear();
        observation.my_queues.clear();
        observation.enemy_buildings = vec![
            building(80, 1, BuildingKind::Turret, primary, true),
            building(82, 1, BuildingKind::Turret, secondary, true),
            building(81, 1, BuildingKind::FlakTurret, flak, true),
        ];
        let mut public_map = movement_cap_serpentine_map(&observation);
        public_map
            .non_ground_terrain
            .retain(|(tile, _)| *tile != primary && *tile != secondary && *tile != flak);
        let intelligence = knowledge(&observation);
        assert_eq!(targetable_flak(&intelligence.air_defense_at(primary)), None);
        assert_eq!(
            targetable_flak(&intelligence.air_defense_at(secondary)),
            Some(BuildingId(81))
        );
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.anchor == primary)
            .expect("the primary target is current");
        let staging =
            connected_artillery_staging_goal(&observation, home, primary, Some(&public_map))
                .expect("the public component contains a staging tile");
        let origin = SuppressionOrigin {
            tile: TilePos::new(255, 255),
            kind: UnitKind::Bombard,
        };
        let mut routes = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&public_map),
            test_orientation(),
        );
        assert!(
            routes.reaches(origin.tile, staging),
            "the uncapped component flood sees the complete serpentine"
        );
        assert!(
            !routes.ground_command_reaches(origin.tile, staging),
            "the suppression provider cannot execute its staging command"
        );
        assert!(
            suppression_targets_reachable(
                &mut routes,
                &observation,
                origin,
                &[Target::Building(BuildingId(81))],
                &intelligence,
                Some(&public_map),
            ),
            "the nearby firing stand is reachable, isolating the staging-leg rejection"
        );

        let selection = connected_target_selection(
            &observation,
            target,
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home,
                target: primary,
                public_map: Some(&public_map),
                orientation: test_orientation(),
            },
        );

        assert_eq!(selection.target_anchors, vec![primary]);
        assert!(selection.suppression_targets.is_empty());
        assert!(selection.growth_order.is_empty());
    }

    #[test]
    fn suppression_rejects_a_firing_stand_beyond_the_movement_search_cap() {
        let mut observation = obs(120);
        observation.map_width = 256;
        observation.map_height = 256;
        observation.visible = vec![true; 256 * 256];
        observation.explored = observation.visible.clone();
        observation.known_rock.clear();
        observation.enemy_buildings = vec![building(
            81,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(250, 84),
            true,
        )];
        let public_map = movement_cap_serpentine_map(&observation);
        let intelligence = knowledge(&observation);
        let target = Target::Building(BuildingId(81));
        let local_origin = SuppressionOrigin {
            tile: TilePos::new(255, 84),
            kind: UnitKind::Bombard,
        };
        let mut local_routes = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&public_map),
            test_orientation(),
        );
        let stand = suppression_firing_stands(
            &mut local_routes,
            &observation,
            local_origin,
            target,
            &intelligence,
            Some(&public_map),
        )
        .next()
        .expect("the nearby Bombard has a legal firing stand");
        let remote_origin = SuppressionOrigin {
            tile: TilePos::new(255, 255),
            kind: UnitKind::Bombard,
        };
        let mut component_routes = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&public_map),
            test_orientation(),
        );
        assert!(
            component_routes.reaches(remote_origin.tile, stand),
            "the uncapped component flood sees the complete serpentine"
        );
        assert!(
            !component_routes.ground_command_reaches(remote_origin.tile, stand),
            "the authoritative ground command exhausts its bounded search"
        );
        let mut actual_routes = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&public_map),
            test_orientation(),
        );
        assert_eq!(
            suppression_firing_stands(
                &mut actual_routes,
                &observation,
                remote_origin,
                target,
                &intelligence,
                Some(&public_map),
            )
            .next(),
            None,
            "suppression must not admit a stand its eventual movement command cannot reach"
        );
    }

    #[test]
    fn connected_package_uses_only_producers_that_can_reach_the_operation() {
        let mut observation = developed_connected_obs(120);
        observation
            .my_units
            .retain(|unit| unit.kind != UnitKind::Bombard);
        observation.my_buildings[0].anchor = TilePos::new(6, 3);
        observation.my_buildings[1].anchor = TilePos::new(3, 14);
        observation.my_buildings[2].kind = BuildingKind::Fabricator;
        observation.my_buildings[2].anchor = TilePos::new(10, 3);

        let isolated = &observation.my_buildings[0];
        let isolated_size = isolated.kind.tier_stats(isolated.tier).size;
        let isolated_spawn = crate::tick::rect_adjacent_tiles(isolated.anchor, isolated_size)
            .min_by_key(|tile| {
                crate::tick::spawn_doorstep_key(
                    (observation.map_width, observation.map_height),
                    isolated.anchor,
                    isolated_size,
                    *tile,
                )
            })
            .expect("the producer has a spawn ring");
        observation.known_rock.extend(
            crate::tick::rect_adjacent_tiles(isolated.anchor, isolated_size)
                .filter(|tile| *tile != isolated_spawn),
        );
        observation.known_rock.extend(
            (-1..=1)
                .flat_map(|dy| (-1..=1).map(move |dx| isolated_spawn.offset(dx, dy)))
                .filter(|tile| *tile != isolated_spawn)
                .filter(|tile| {
                    tile.x < isolated.anchor.x
                        || tile.x >= isolated.anchor.x + isolated_size.0
                        || tile.y < isolated.anchor.y
                        || tile.y >= isolated.anchor.y + isolated_size.1
                }),
        );
        observation
            .known_rock
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        observation.known_rock.dedup();

        let resources = ResourceSnapshot::from_observation(&observation);
        let isolated_timing = resources
            .producers()
            .iter()
            .find(|lane| lane.producer == BuildingId(10))
            .and_then(|lane| lane.production_timing(&[UnitKind::Bombard]))
            .expect("the isolated producer has a locally open spawn tile");
        assert_eq!(
            isolated_timing.current_egress,
            super::super::resources::ProducerEgress::Open
        );

        let intelligence = knowledge(&observation);
        let target = intelligence.buildings()[0].clone();
        let identity = profile();
        let connected_resources = ConnectedProductionResources::from_observation(
            &observation,
            &target,
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home: HOME,
                target: target.anchor,
                public_map: None,
                orientation: test_orientation(),
            },
        );
        let mut plan = connected_plan(
            &identity,
            &observation,
            &intelligence,
            HOME,
            &target,
            &[],
            ConnectedPlanningContext {
                orientation: test_orientation(),
                public_map: None,
                resources: &connected_resources,
                preferred_artillery: &[],
                protected_current_scrap: 0,
                preparation: PreparationConstraints {
                    deadline: observation.tick + CONNECTED_PREPARATION_HORIZON,
                    decision_cadence: DifficultyTuning::for_level(BotDifficulty::Prime).cadence,
                    protected_forecast_scrap: 0,
                },
            },
        )
        .expect("the reachable Fabricator can field the suppression provider");
        let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
        operation.artillery.clear();
        let mut decision = StrategicDecision::default();

        schedule_missing_members(
            &operation,
            &mut plan,
            &planning_context(&identity, &observation, &intelligence),
            UnitKind::Kestrel,
            &mut decision,
        );

        assert!(
            decision.intents.contains(&Intent::TrainAt {
                building: BuildingId(12),
                kind: UnitKind::Bombard,
            }),
            "reachable producer was not selected: plan={plan:?}; decision={decision:?}"
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                building: BuildingId(10),
                ..
            }
        )));
    }

    #[test]
    fn connected_package_excludes_publicly_stranded_units_and_producers() {
        let mut observation = developed_connected_obs(120);
        observation.visible.fill(false);
        observation.explored.fill(false);
        see_approach(&mut observation);
        observation.my_units[1].tile = TilePos::new(5, 5);
        observation.my_buildings[0].anchor = TilePos::new(2, 2);
        observation.my_buildings[1].anchor = TilePos::new(10, 2);
        observation.my_buildings[2].kind = BuildingKind::Fabricator;
        observation.my_buildings[2].anchor = TilePos::new(14, 2);
        let reachable_anchor = observation.my_buildings[2].anchor;
        let reachable_size = observation.my_buildings[2]
            .kind
            .tier_stats(observation.my_buildings[2].tier)
            .size;
        for tile in crate::tick::rect_adjacent_tiles(reachable_anchor, reachable_size) {
            explore(&mut observation, tile);
            let index = usize::try_from(tile.y * observation.map_width + tile.x).unwrap();
            observation.visible[index] = true;
        }

        let horizontal = (0..=7).flat_map(|x| {
            [
                (TilePos::new(x, 0), Terrain::Peak),
                (TilePos::new(x, 7), Terrain::Peak),
            ]
        });
        let vertical = (1..7).flat_map(|y| {
            [
                (TilePos::new(0, y), Terrain::Peak),
                (TilePos::new(7, y), Terrain::Peak),
            ]
        });
        let public_map = public_map_with_terrain(&observation, horizontal.chain(vertical));
        let resources = ResourceSnapshot::from_observation(&observation);
        let intelligence = knowledge(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.anchor == TARGET)
            .expect("current target");
        let optimistic_route = ConnectedRouteContext {
            intel: &intelligence,
            home: HOME,
            target: TARGET,
            public_map: None,
            orientation: test_orientation(),
        };
        let public_route = ConnectedRouteContext {
            public_map: Some(&public_map),
            ..optimistic_route
        };
        let optimistic_targets =
            connected_target_selection(&observation, target, &[], optimistic_route);
        let public_targets = connected_target_selection(&observation, target, &[], public_route);

        let optimistic = connected_provider_unavailable(
            &observation,
            &optimistic_targets,
            &[],
            optimistic_route,
        );
        let public =
            connected_provider_unavailable(&observation, &public_targets, &[], public_route);
        assert!(!optimistic.contains(&UnitId(2)));
        assert!(public.contains(&UnitId(2)));

        let access_without_briefing = connected_production_access(
            &observation,
            &optimistic_targets,
            &resources,
            optimistic_route,
        );
        let public_access =
            connected_production_access(&observation, &public_targets, &resources, public_route);
        assert!(access_without_briefing.allows(BuildingId(10), UnitKind::Bombard));
        assert!(!public_access.allows(BuildingId(10), UnitKind::Bombard));
        assert!(public_access.allows(BuildingId(12), UnitKind::Bombard));
        let reachable_timing = resources
            .producers()
            .iter()
            .find(|lane| lane.producer == BuildingId(12))
            .and_then(|lane| lane.production_timing(&[UnitKind::Bombard]))
            .expect("the reachable producer can train a Bombard");
        assert_eq!(
            reachable_timing.current_egress,
            super::super::resources::ProducerEgress::Open
        );

        let schedule = plan_production_with_access(
            &resources,
            &[ProductionDemand {
                kind: UnitKind::Bombard,
                count: 1,
            }],
            observation.tick + CONNECTED_PREPARATION_HORIZON,
            observation.scrap,
            &public_access,
        );
        assert_eq!(
            schedule.appends.len(),
            1,
            "unexpected schedule: {schedule:?}"
        );
        assert_eq!(schedule.appends[0].producer, BuildingId(12));
        assert_eq!(schedule.appends[0].kind, UnitKind::Bombard);
    }

    #[test]
    fn connected_cluster_uses_air_routes_without_treating_ground_pits_as_a_barrier() {
        let secondary = TARGET.offset(0, 4);
        let mut observation = developed_connected_obs(120);
        observation.enemy_buildings = vec![
            building(80, 1, BuildingKind::Foundry, TARGET, true),
            building(81, 1, BuildingKind::Crucible, secondary, true),
        ];
        let ground_barrier =
            (0..observation.map_width).map(|x| (TilePos::new(x, 12), Terrain::Pit));
        let pit_map = public_map_with_terrain(&observation, ground_barrier);
        let intelligence = knowledge(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.anchor == TARGET)
            .expect("current primary target");
        assert!(!known_ground_connected(
            &observation,
            HOME,
            secondary,
            BuildingKind::Crucible.base_stats().size,
            Some(&pit_map),
        ));

        let pit_selection = connected_target_selection(
            &observation,
            target,
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home: HOME,
                target: TARGET,
                public_map: Some(&pit_map),
                orientation: test_orientation(),
            },
        );
        assert_eq!(pit_selection.target_anchors, vec![TARGET, secondary]);

        let air_barrier = (0..observation.map_width).map(|x| (TilePos::new(x, 12), Terrain::Peak));
        let peak_map = public_map_with_terrain(&observation, air_barrier);
        let peak_selection = connected_target_selection(
            &observation,
            target,
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home: HOME,
                target: TARGET,
                public_map: Some(&peak_map),
                orientation: test_orientation(),
            },
        );
        assert_eq!(peak_selection.target_anchors, vec![TARGET]);

        let peak_resources = ConnectedProductionResources::from_observation(
            &observation,
            target,
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home: HOME,
                target: TARGET,
                public_map: Some(&peak_map),
                orientation: test_orientation(),
            },
        );
        let peak_plan = connected_plan(
            &profile(),
            &observation,
            &intelligence,
            HOME,
            target,
            &[],
            ConnectedPlanningContext {
                orientation: test_orientation(),
                public_map: Some(&peak_map),
                resources: &peak_resources,
                preferred_artillery: &[],
                protected_current_scrap: 0,
                preparation: PreparationConstraints {
                    deadline: observation.tick + CONNECTED_PREPARATION_HORIZON,
                    decision_cadence: 12,
                    protected_forecast_scrap: 0,
                },
            },
        )
        .expect("an inaccessible secondary target must not reject the viable primary");
        assert_eq!(
            peak_plan
                .connected_package
                .as_ref()
                .expect("connected plan")
                .target_anchors,
            vec![TARGET]
        );

        let mut after_primary = observation.clone();
        after_primary
            .enemy_buildings
            .retain(|building| building.anchor == secondary);
        let later_intelligence = knowledge(&after_primary);
        let operation = operation(AirOperationPhase::Strike, after_primary.tick);
        let mut pit_plan = connected_test_plan(&after_primary);
        pit_plan
            .connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = pit_selection.target_anchors;
        assert_eq!(
            live_strike_target(&operation, &pit_plan, &later_intelligence)
                .map(|contact| contact.anchor),
            Some(secondary)
        );
        pit_plan
            .connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = peak_selection.target_anchors;
        assert_eq!(
            live_strike_target(&operation, &pit_plan, &later_intelligence),
            None,
            "a target excluded at admission must not re-enter tactical selection"
        );
    }

    #[test]
    fn connected_operation_survives_the_primary_and_completes_at_the_remaining_anchor() {
        let secondary = TARGET.offset(3, 0);
        let mut battle = obs(5_000);
        battle.visible.fill(true);
        battle.explored.fill(true);
        battle.enemy_buildings = vec![building(81, 1, BuildingKind::Airworks, secondary, true)];
        let mut intelligence = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Verify, battle.tick);
        planner
            .air
            .as_mut()
            .expect("active operation")
            .plan
            .connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = vec![TARGET, secondary];

        let attack = think(&mut planner, &battle, &intelligence);
        assert!(attack.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(81)),
        }));
        assert!(attack.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(2)],
            goal: staging(HOME, secondary),
        }));
        assert!(attack.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if units == &[UnitId(2)] && *goal == staging(HOME, TARGET)
        )));
        assert_eq!(
            planner.air_operation().expect("operation continues").phase,
            AirOperationPhase::Strike
        );

        battle.tick += 12;
        battle.enemy_buildings.clear();
        intelligence.update(&battle);
        let follow_through = think(&mut planner, &battle, &intelligence);
        assert!(follow_through.intents.contains(&Intent::AttackMoveUnits {
            units: vec![UnitId(3), UnitId(4)],
            goal: secondary,
        }));
        assert!(follow_through.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackMoveUnits { goal, .. } if *goal == TARGET
        )));

        battle.tick += 20;
        intelligence.update(&battle);
        let completed = think(&mut planner, &battle, &intelligence);
        assert!(completed.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
        let operation = planner
            .air_operation()
            .expect("completion recovery remains observable");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(operation.recovery_reason, Some(AirRecoveryReason::Complete));
    }

    #[test]
    fn artillery_minimum_range_is_part_of_suppression_access() {
        let flak_anchor = TilePos::new(12, 10);
        let origin = TilePos::new(10, 10);
        let mut battle = obs(120);
        battle.visible.fill(true);
        battle.explored.fill(true);
        battle.enemy_buildings = vec![building(81, 1, BuildingKind::FlakTurret, flak_anchor, true)];
        let sealed = (-1..=1)
            .flat_map(|dy| (-1..=1).map(move |dx| origin.offset(dx, dy)))
            .filter(|tile| *tile != origin)
            .map(|tile| (tile, Terrain::Pit));
        let public_map = public_map_with_terrain(&battle, sealed);
        let intelligence = knowledge(&battle);
        let target = Target::Building(BuildingId(81));

        let mut bombard_routes = route_projection_with_orientation(
            &battle,
            Domain::Ground,
            Some(&public_map),
            test_orientation(),
        );
        assert_eq!(
            suppression_firing_stands(
                &mut bombard_routes,
                &battle,
                SuppressionOrigin {
                    tile: origin,
                    kind: UnitKind::Bombard,
                },
                target,
                &intelligence,
                Some(&public_map),
            )
            .next(),
            Some(origin),
            "Bombard may fire over the surrounding Pit"
        );

        let mut avalanche_routes = route_projection_with_orientation(
            &battle,
            Domain::Ground,
            Some(&public_map),
            test_orientation(),
        );
        assert_eq!(
            suppression_firing_stands(
                &mut avalanche_routes,
                &battle,
                SuppressionOrigin {
                    tile: origin,
                    kind: UnitKind::Avalanche,
                },
                target,
                &intelligence,
                Some(&public_map),
            )
            .next(),
            None,
            "Avalanche cannot use the same pocket inside its minimum range"
        );
    }

    #[test]
    fn reserved_sole_suppression_provider_cannot_inflate_the_target_cluster() {
        let primary = TilePos::new(20, 20);
        let secondary = TilePos::new(24, 20);
        let flak = TilePos::new(29, 20);
        let mut battle = obs(400);
        battle.map_width = 40;
        battle.map_height = 30;
        battle.visible = vec![true; 40 * 30];
        battle.explored = vec![true; 40 * 30];
        battle.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, primary, true),
            building(82, 1, BuildingKind::Airworks, secondary, true),
            building(81, 1, BuildingKind::FlakTurret, flak, true),
        ];
        let intelligence = knowledge(&battle);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.anchor == primary)
            .expect("current primary target");
        let route = ConnectedRouteContext {
            intel: &intelligence,
            home: HOME,
            target: primary,
            public_map: None,
            orientation: test_orientation(),
        };

        let available = connected_target_selection(&battle, target, &[], route);
        assert_eq!(available.target_anchors, vec![primary, secondary]);
        assert_eq!(
            available.suppression_targets,
            vec![Target::Building(BuildingId(81))]
        );

        let reserved = connected_target_selection(&battle, target, &[UnitId(2)], route);
        assert_eq!(reserved.target_anchors, vec![primary]);
        assert!(reserved.suppression_targets.is_empty());
    }

    #[test]
    fn optional_cluster_target_is_dropped_when_its_only_provider_misses_the_deadline() {
        let primary = TARGET;
        let secondary = TARGET.offset(4, 0);
        let flak = TARGET.offset(9, 0);
        let mut battle = developed_connected_obs(400);
        battle.map_width = 40;
        battle.map_height = 24;
        battle.visible = vec![true; 40 * 24];
        battle.explored = vec![true; 40 * 24];
        battle.my_units[1] = own(2, UnitKind::Avalanche, TilePos::new(10, 10));
        battle.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, primary, true),
            building(82, 1, BuildingKind::Turret, secondary, true),
            building(81, 1, BuildingKind::FlakTurret, flak, true),
        ];
        battle.my_queues[0] = vec![UnitKind::Lancer; QUEUE_CAP];

        // Two offset Peak walls leave a bent corridor into a pocket inside
        // the Avalanche's blind ring. A Bombard can prosecute the Flak from
        // the pocket, while the live Avalanche cannot fire through either
        // wall from the main component.
        let terrain = (0..battle.map_height).flat_map(|y| {
            (0..battle.map_width).filter_map(move |x| {
                let open = x <= 28
                    || (x == 29 && y == 15)
                    || (x == 30 && (12..=15).contains(&y))
                    || (x == 31 && y == 12)
                    || ((32..=35).contains(&x) && (9..=12).contains(&y));
                (!open).then_some((TilePos::new(x, y), Terrain::Peak))
            })
        });
        let public_map = public_map_with_terrain(&battle, terrain);
        let intelligence = knowledge(&battle);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.anchor == primary)
            .expect("current primary target");
        let route = ConnectedRouteContext {
            intel: &intelligence,
            home: HOME,
            target: primary,
            public_map: Some(&public_map),
            orientation: test_orientation(),
        };
        let resources = ConnectedProductionResources::from_observation(&battle, target, &[], route);
        assert_eq!(resources.targets.target_anchors, vec![primary, secondary]);

        let mut open_lane = battle.clone();
        open_lane.my_queues[0].clear();
        let open_resources =
            ConnectedProductionResources::from_observation(&open_lane, target, &[], route);
        let open_plan = connected_plan(
            &profile(),
            &open_lane,
            &intelligence,
            HOME,
            target,
            &[],
            ConnectedPlanningContext {
                orientation: test_orientation(),
                public_map: Some(&public_map),
                resources: &open_resources,
                preferred_artillery: &[],
                protected_current_scrap: 0,
                preparation: PreparationConstraints {
                    deadline: battle.tick + 400,
                    decision_cadence: 12,
                    protected_forecast_scrap: 0,
                },
            },
        )
        .expect("an open Bombard lane can cover the optional target in time");
        assert_eq!(
            open_plan
                .connected_package
                .expect("connected package")
                .target_anchors,
            vec![primary, secondary]
        );

        let plan = connected_plan(
            &profile(),
            &battle,
            &intelligence,
            HOME,
            target,
            &[],
            ConnectedPlanningContext {
                orientation: test_orientation(),
                public_map: Some(&public_map),
                resources: &resources,
                preferred_artillery: &[],
                protected_current_scrap: 0,
                preparation: PreparationConstraints {
                    deadline: battle.tick + 400,
                    decision_cadence: 12,
                    protected_forecast_scrap: 0,
                },
            },
        )
        .expect("the live primary-only package remains feasible");
        assert_eq!(
            plan.connected_package
                .expect("connected package")
                .target_anchors,
            vec![primary],
            "a route-only optional target must not enlarge a package whose only covering producer cannot finish before the fixed deadline"
        );
    }

    #[test]
    fn connected_suppression_uses_an_indirect_firing_stand_beyond_a_pit_ring() {
        let flak_anchor = TARGET.offset(-4, 0);
        let mut observation = developed_connected_obs(120);
        observation
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(2))
            .expect("fixture artillery")
            .tile = TilePos::new(5, 10);
        observation.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, TARGET, true),
            building(81, 1, BuildingKind::FlakTurret, flak_anchor, true),
        ];
        let sealed = crate::tick::rect_adjacent_tiles(
            flak_anchor,
            BuildingKind::FlakTurret.base_stats().size,
        )
        .map(|tile| (tile, Terrain::Pit));
        let public_map = public_map_with_terrain(&observation, sealed);
        let mut intelligence = knowledge(&observation);
        let target = intelligence
            .buildings()
            .iter()
            .find(|contact| contact.anchor == TARGET)
            .expect("current primary target");
        let staging =
            connected_artillery_staging_goal(&observation, HOME, TARGET, Some(&public_map))
                .expect("generic staging remains reachable");
        let bombard = observation
            .my_units
            .iter()
            .find(|unit| unit.kind == UnitKind::Bombard)
            .expect("fixture artillery");
        let mut ground_routes = route_projection_with_orientation(
            &observation,
            Domain::Ground,
            Some(&public_map),
            test_orientation(),
        );
        assert!(ground_routes.unit_reaches(bombard, staging));

        let resources = ConnectedProductionResources::from_observation(
            &observation,
            target,
            &[],
            ConnectedRouteContext {
                intel: &intelligence,
                home: HOME,
                target: TARGET,
                public_map: Some(&public_map),
                orientation: test_orientation(),
            },
        );
        assert_eq!(
            resources.targets.suppression_targets,
            vec![Target::Building(BuildingId(81))]
        );
        assert!(
            resources.access.allows(BuildingId(10), UnitKind::Bombard),
            "a producer may supply artillery that can reach an indirect-fire stand"
        );

        connected_plan(
            &profile(),
            &observation,
            &intelligence,
            HOME,
            target,
            &[],
            ConnectedPlanningContext {
                orientation: test_orientation(),
                public_map: Some(&public_map),
                resources: &resources,
                preferred_artillery: &[],
                protected_current_scrap: 0,
                preparation: PreparationConstraints {
                    deadline: observation.tick + CONNECTED_PREPARATION_HORIZON,
                    decision_cadence: 12,
                    protected_forecast_scrap: 0,
                },
            },
        )
        .expect("Pit blocks movement but not an indirect shell");

        let mut planner = with_operation(AirOperationPhase::SuppressAa, observation.tick);
        let active = planner.air.as_mut().expect("active operation");
        active
            .plan
            .connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = vec![TARGET];
        let mut coordination = coordination(None);
        coordination.public_map = Some(&public_map);
        let positioning = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination,
        );
        let firing_stand = positioning
            .intents
            .iter()
            .find_map(|intent| match intent {
                Intent::MoveUnits { units, goal } if units == &[UnitId(2)] => Some(*goal),
                _ => None,
            })
            .expect("artillery moves to its exact firing stand");
        assert!(
            !crate::tick::rect_adjacent_tiles(
                flak_anchor,
                BuildingKind::FlakTurret.base_stats().size,
            )
            .any(|tile| tile == firing_stand),
            "the firing stand is not an unreachable footprint-adjacent tile"
        );
        assert!(positioning.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(81)),
                ..
            }
        )));

        observation.tick += 12;
        observation
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(2))
            .expect("fixture artillery")
            .tile = firing_stand;
        observation
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(1))
            .expect("fixture scout")
            .idle = false;
        intelligence.update(&observation);
        let attack = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intelligence,
            HOME,
            coordination,
        );
        assert!(
            attack.intents.contains(&Intent::AttackUnits {
                units: vec![UnitId(2)],
                target: Target::Building(BuildingId(81)),
            }),
            "{attack:?}; operation={:?}",
            planner.air_operation()
        );
    }

    #[test]
    fn connected_no_aa_evidence_requires_visibility_over_the_full_footprint() {
        let mut observation = obs(120);
        let anchor_index = usize::try_from(TARGET.y * observation.map_width + TARGET.x).unwrap();
        observation.visible[anchor_index] = true;
        let intelligence = knowledge(&observation);
        let operation = operation(AirOperationPhase::Verify, observation.tick);
        let plan = connected_test_plan(&observation);
        assert_eq!(
            cluster_air_defense(&operation, &plan, &intelligence).evidence,
            AirDefenseEvidence::Unknown
        );

        let mut fully_visible = observation;
        let (width, height) = BuildingKind::Crucible.base_stats().size;
        for dy in 0..height {
            for dx in 0..width {
                let tile = TARGET.offset(dx, dy);
                let index = usize::try_from(tile.y * fully_visible.map_width + tile.x).unwrap();
                fully_visible.visible[index] = true;
            }
        }
        let intelligence = knowledge(&fully_visible);
        assert_eq!(
            cluster_air_defense(&operation, &plan, &intelligence).evidence,
            AirDefenseEvidence::VisibleWithoutKnownCoverage
        );
    }

    #[test]
    fn connected_verify_keeps_a_remembered_selected_anchor_in_aa_clearance() {
        let primary = TilePos::new(20, 20);
        let secondary = TilePos::new(24, 20);
        let flak = TilePos::new(29, 20);
        let mut battle = obs(400);
        battle.map_width = 40;
        battle.map_height = 30;
        battle.visible = vec![true; 40 * 30];
        battle.explored = vec![true; 40 * 30];
        battle.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, primary, true),
            building(82, 1, BuildingKind::Airworks, secondary, true),
            building(81, 1, BuildingKind::FlakTurret, flak, true),
        ];
        let mut intelligence = knowledge(&battle);

        let mut hidden = battle;
        hidden.tick += 12;
        hidden.visible.fill(false);
        see_approach_to(&mut hidden, primary);
        see_building_footprint(&mut hidden, primary, BuildingKind::Crucible);
        for building in &mut hidden.enemy_buildings {
            building.seen = building.anchor == primary;
        }
        intelligence.update(&hidden);
        assert!(intelligence.buildings().iter().any(|contact| {
            contact.anchor == secondary && contact.evidence == ContactEvidence::Remembered
        }));

        let mut operation = operation(AirOperationPhase::Verify, hidden.tick);
        operation.target = primary;
        operation.target_id = Some(BuildingId(80));
        let mut plan = connected_test_plan(&hidden);
        plan.connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = vec![primary, secondary];
        assert_eq!(
            cluster_air_defense(&operation, &plan, &intelligence).evidence,
            AirDefenseEvidence::RememberedCoverage
        );
        assert_eq!(
            connected_scout_focus(&operation, &plan, &hidden, &intelligence),
            secondary
        );

        let identity = profile();
        let mut decision = StrategicDecision::default();
        verify(
            &mut operation,
            &mut plan,
            &AirPlanningContext {
                profile: &identity,
                tuning: DifficultyTuning::for_level(identity.difficulty),
                obs: &hidden,
                intel: &intelligence,
                home: HOME,
                orientation: test_orientation(),
                public_map: None,
                enlisted: &[],
                landing_sites: &[],
                connected_resources: None,
                production: StrategicProductionContext::empty(),
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
            },
            &mut decision,
        );
        assert_eq!(operation.phase, AirOperationPhase::Verify);
        assert!(operation.scout_dispatch.is_some());
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));

        let mut cleared = hidden;
        cleared.tick += 100;
        cleared
            .enemy_buildings
            .retain(|building| building.anchor == primary);
        see_building_footprint(&mut cleared, secondary, BuildingKind::Airworks);
        see_building_footprint(&mut cleared, flak, BuildingKind::FlakTurret);
        intelligence.update(&cleared);
        assert_eq!(
            cluster_air_defense(&operation, &plan, &intelligence).evidence,
            AirDefenseEvidence::VisibleWithoutKnownCoverage
        );

        let mut decision = StrategicDecision::default();
        verify(
            &mut operation,
            &mut plan,
            &AirPlanningContext {
                profile: &identity,
                tuning: DifficultyTuning::for_level(identity.difficulty),
                obs: &cleared,
                intel: &intelligence,
                home: HOME,
                orientation: test_orientation(),
                public_map: None,
                enlisted: &[],
                landing_sites: &[],
                connected_resources: None,
                production: StrategicProductionContext::empty(),
                protected_current_scrap: 0,
                protected_forecast_scrap: 0,
            },
            &mut decision,
        );
        assert_eq!(operation.phase, AirOperationPhase::Strike);
    }

    #[test]
    fn connected_verify_scouts_every_selected_footprint_before_accepting_negative_aa_evidence() {
        let secondary = TARGET.offset(0, 4);
        let mut observation = obs(120);
        observation
            .enemy_buildings
            .push(building(81, 1, BuildingKind::Crucible, secondary, true));
        observation.explored.fill(true);
        see_approach(&mut observation);
        see_approach_to(&mut observation, secondary);
        see_building_footprint(&mut observation, TARGET, BuildingKind::Crucible);
        see_building_footprint(&mut observation, secondary, BuildingKind::Crucible);
        let far_edge = secondary.offset(1, 1);
        let far_index = usize::try_from(far_edge.y * observation.map_width + far_edge.x).unwrap();
        observation.visible[far_index] = false;
        let mut intelligence = knowledge(&observation);
        let mut operation = operation(AirOperationPhase::Verify, observation.tick);
        let mut plan = connected_test_plan(&observation);
        plan.connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = vec![TARGET, secondary];
        let identity = profile();
        let context = AirPlanningContext {
            profile: &identity,
            tuning: DifficultyTuning::for_level(identity.difficulty),
            obs: &observation,
            intel: &intelligence,
            home: HOME,
            orientation: test_orientation(),
            public_map: None,
            enlisted: &[],
            landing_sites: &[],
            connected_resources: None,
            production: StrategicProductionContext::empty(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        };
        let mut decision = StrategicDecision::default();

        verify(&mut operation, &mut plan, &context, &mut decision);

        assert_eq!(
            connected_scout_focus(&operation, &plan, &observation, &intelligence),
            far_edge
        );
        let (_, scout_goal) = operation
            .scout_dispatch
            .expect("the scout is sent to clear the remaining footprint tile");
        let scout_vision = Role::Scout
            .unit_for(observation.faction)
            .stats()
            .vision
            .saturating_sub(1);
        let dx = scout_goal.x - far_edge.x;
        let dy = scout_goal.y - far_edge.y;
        assert!(dx.saturating_mul(dx) + dy.saturating_mul(dy) <= scout_vision * scout_vision);
        assert_eq!(operation.phase, AirOperationPhase::Verify);

        observation.tick += 12;
        observation.visible[far_index] = true;
        intelligence.update(&observation);
        let context = AirPlanningContext {
            profile: &identity,
            tuning: DifficultyTuning::for_level(identity.difficulty),
            obs: &observation,
            intel: &intelligence,
            home: HOME,
            orientation: test_orientation(),
            public_map: None,
            enlisted: &[],
            landing_sites: &[],
            connected_resources: None,
            production: StrategicProductionContext::empty(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        };
        let mut cleared = StrategicDecision::default();
        verify(&mut operation, &mut plan, &context, &mut cleared);

        assert_eq!(operation.phase, AirOperationPhase::Strike);
    }

    #[test]
    fn connected_verify_checks_the_selected_secondary_approach_before_striking() {
        let secondary = TARGET.offset(0, 4);
        let mut observation = obs(120);
        observation.enemy_buildings =
            vec![building(81, 1, BuildingKind::Crucible, secondary, true)];
        see_approach(&mut observation);
        let (width, height) = BuildingKind::Crucible.base_stats().size;
        for dy in 0..height {
            for dx in 0..width {
                let tile = secondary.offset(dx, dy);
                let index = usize::try_from(tile.y * observation.map_width + tile.x).unwrap();
                observation.visible[index] = true;
                observation.explored[index] = true;
            }
        }
        let intelligence = knowledge(&observation);
        assert!(corridor_clear(&intelligence, HOME, TARGET, &[]));
        assert!(!corridor_clear(&intelligence, HOME, secondary, &[]));

        let identity = profile();
        let mut operation = operation(AirOperationPhase::Verify, observation.tick);
        let mut plan = connected_test_plan(&observation);
        plan.connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = vec![TARGET, secondary];
        let context = AirPlanningContext {
            profile: &identity,
            tuning: DifficultyTuning::for_level(identity.difficulty),
            obs: &observation,
            intel: &intelligence,
            home: HOME,
            orientation: test_orientation(),
            public_map: None,
            enlisted: &[],
            landing_sites: &[],
            connected_resources: None,
            production: StrategicProductionContext::empty(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        };
        let mut decision = StrategicDecision::default();
        verify(&mut operation, &mut plan, &context, &mut decision);

        assert_eq!(operation.phase, AirOperationPhase::Verify);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(81)),
                ..
            }
        )));
        assert!(operation.scout_dispatch.is_some());
    }

    fn assert_axis_orientation_preserves_producer_doorstep(home: TilePos) {
        let world = developed_connected_obs(120);
        let orientation = Orientation::for_home(&world, home);
        let producer = world
            .my_buildings
            .iter()
            .find(|building| building.id == BuildingId(10))
            .expect("the fixture has a Fabricator");
        let size = producer.kind.tier_stats(producer.tier).size;
        let expected = crate::tick::rect_adjacent_tiles(producer.anchor, size)
            .filter(|tile| public_ground_open(&world, *tile, None))
            .min_by_key(|tile| {
                crate::tick::spawn_doorstep_key(
                    (world.map_width, world.map_height),
                    producer.anchor,
                    size,
                    *tile,
                )
            })
            .expect("the world-frame producer has an open doorstep");

        let oriented = orientation.observe(&world);
        let oriented_producer = oriented
            .my_buildings
            .iter()
            .find(|building| building.id == BuildingId(10))
            .expect("orientation preserves the producer");
        let actual = production_spawn_doorstep(&oriented, oriented_producer, None, orientation)
            .expect("the oriented producer has an open doorstep");

        assert_eq!(orientation.tile(actual), expected);
    }

    #[test]
    fn x_only_orientation_preserves_the_authoritative_producer_doorstep() {
        assert_axis_orientation_preserves_producer_doorstep(TilePos::new(27, 2));
    }

    #[test]
    fn y_only_orientation_preserves_the_authoritative_producer_doorstep() {
        assert_axis_orientation_preserves_producer_doorstep(TilePos::new(2, 17));
    }

    #[test]
    fn centered_producer_preserves_the_authoritative_world_order_doorstep() {
        let mut world = developed_connected_obs(120);
        let (size, anchor) = {
            let producer = world
                .my_buildings
                .iter_mut()
                .find(|building| building.id == BuildingId(10))
                .expect("the fixture has a Fabricator");
            let size = producer.kind.tier_stats(producer.tier).size;
            let anchor = TilePos::new(
                (world.map_width - size.0) / 2,
                (world.map_height - size.1) / 2,
            );
            producer.anchor = anchor;
            (size, anchor)
        };
        let expected = crate::tick::rect_adjacent_tiles(anchor, size)
            .filter(|tile| public_ground_open(&world, *tile, None))
            .min_by_key(|tile| {
                crate::tick::spawn_doorstep_key(
                    (world.map_width, world.map_height),
                    anchor,
                    size,
                    *tile,
                )
            })
            .expect("the world-frame producer has an open doorstep");

        let orientation = Orientation::for_home(
            &world,
            TilePos::new(world.map_width - 1, world.map_height - 1),
        );
        let oriented = orientation.observe(&world);
        let oriented_producer = oriented
            .my_buildings
            .iter()
            .find(|building| building.id == BuildingId(10))
            .expect("orientation preserves the producer");
        let actual = production_spawn_doorstep(&oriented, oriented_producer, None, orientation)
            .expect("the oriented producer has an open doorstep");

        assert_eq!(
            orientation.tile(actual),
            expected,
            "a zero radial key must retain the authoritative world-frame row-major tie"
        );
    }

    #[test]
    fn precommit_rederivation_reports_and_recovers_from_untargetable_air_defense() {
        let mut battle = obs(301);
        battle.explored.fill(true);
        see_approach(&mut battle);
        let mut talon = own(90, UnitKind::Talon, TARGET.offset(-3, 0));
        talon.player = PlayerId(1);
        battle.enemy_units.push(talon);
        let intelligence = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Assemble, 300);

        let result = planner.think_through_adjudication(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intelligence,
            HOME,
            coordination(None),
        );

        assert!(
            matches!(
                result.rejected_connected_candidate,
                Some(RejectedConnectedCandidate {
                    reason: ConnectedPlanRejection::Package {
                        reason: ForcePackageRejection::UntargetableCurrentAirDefense { .. },
                        ..
                    },
                    ..
                })
            ),
            "unexpected rejection: {:?}",
            result.rejected_connected_candidate
        );
        assert_eq!(
            planner
                .air_operation()
                .and_then(|operation| operation.recovery_reason),
            Some(AirRecoveryReason::NewAirDefense)
        );
        assert!(result.decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn current_air_defense_first_seen_on_the_deadline_prevents_stale_force_freeze() {
        let mut battle = obs(2_500);
        battle.explored.fill(true);
        see_approach(&mut battle);
        let mut talon = own(90, UnitKind::Talon, TARGET.offset(-3, 0));
        talon.player = PlayerId(1);
        battle.enemy_units.push(talon);
        let intelligence = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Assemble, battle.tick);
        let active = planner.air.as_mut().expect("the fixture has an operation");
        let package = active
            .plan
            .connected_package
            .as_mut()
            .expect("the fixture has a connected package");
        package.derived_at = battle.tick - 1;
        package.preparation_deadline = battle.tick;

        let result = planner.think_through_adjudication(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intelligence,
            HOME,
            coordination(None),
        );

        assert!(
            matches!(
                result.rejected_connected_candidate,
                Some(RejectedConnectedCandidate {
                    reason: ConnectedPlanRejection::Package {
                        reason: ForcePackageRejection::UntargetableCurrentAirDefense { .. },
                        ..
                    },
                    ..
                })
            ),
            "deadline evidence did not reject the stale package: {:?}",
            result.rejected_connected_candidate
        );
        let operation = planner
            .air_operation()
            .expect("the rejected operation remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::NewAirDefense)
        );
        assert_eq!(operation.membership_frozen_at, None);
        assert!(result.decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn retained_connected_operation_grows_before_freeze_and_is_immutable_afterward() {
        let admitted_at = 120;
        let mut planner = with_operation(AirOperationPhase::Assemble, admitted_at);
        let initial_package = planner
            .air_plan()
            .and_then(|plan| plan.connected_package.clone())
            .expect("the fixture begins with an admitted connected package");
        let fixed_deadline = initial_package.preparation_deadline;

        let mut richer = developed_connected_obs(admitted_at + 12);
        richer.scrap = 50_000;
        richer.enemy_buildings.extend([
            building(81, 1, BuildingKind::Foundry, TARGET.offset(-2, -2), true),
            building(82, 1, BuildingKind::Airworks, TARGET.offset(-2, 2), true),
            building(83, 1, BuildingKind::Fabricator, TARGET.offset(-4, 0), true),
            building(84, 1, BuildingKind::Extractor, TARGET.offset(0, -4), true),
        ]);
        let mut intelligence = knowledge(&richer);

        think(&mut planner, &richer, &intelligence);

        let revised_package = planner
            .air_plan()
            .and_then(|plan| plan.connected_package.clone())
            .expect("the retained operation keeps its connected package");
        assert_eq!(revised_package.derived_at, richer.tick);
        assert_eq!(revised_package.preparation_deadline, fixed_deadline);
        assert!(revised_package.target_value > initial_package.target_value);
        assert!(
            demand_count(&revised_package.suppression) + demand_count(&revised_package.strike)
                > demand_count(&initial_package.suppression)
                    + demand_count(&initial_package.strike),
            "current richer evidence should grow the retained package before commitment"
        );

        let active = planner.air.as_mut().expect("the operation remains active");
        active.op.phase = AirOperationPhase::SuppressAa;
        active.op.phase_started_at = richer.tick;
        active.op.membership_frozen_at = Some(richer.tick);
        let frozen_package = active.plan.connected_package.clone();
        let frozen_members = (
            active.op.scout,
            active.op.artillery.clone(),
            active.op.strike_aircraft.clone(),
        );

        richer.tick += 12;
        richer.enemy_buildings.extend([
            building(85, 1, BuildingKind::Foundry, TARGET.offset(2, -2), true),
            building(86, 1, BuildingKind::Crucible, TARGET.offset(2, 2), true),
        ]);
        intelligence.update(&richer);
        think(&mut planner, &richer, &intelligence);

        let active = planner
            .air
            .as_ref()
            .expect("the frozen operation remains active");
        assert_eq!(active.plan.connected_package, frozen_package);
        assert_eq!(
            (
                active.op.scout,
                active.op.artillery.clone(),
                active.op.strike_aircraft.clone(),
            ),
            frozen_members,
            "post-commit evidence cannot rewrite the exact force membership"
        );
    }

    #[test]
    fn rich_targets_scale_connected_packages_beyond_the_old_fixed_cohort() {
        let mut observation = developed_connected_obs(120);
        observation.scrap = 50_000;
        observation.enemy_buildings.extend([
            building(81, 1, BuildingKind::Foundry, TARGET.offset(-2, -2), true),
            building(82, 1, BuildingKind::Airworks, TARGET.offset(-2, 2), true),
            building(83, 1, BuildingKind::FlakTurret, TARGET.offset(-4, 0), true),
        ]);
        let siege = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_043,
        ));
        let air = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_045,
        ));

        let siege_plan = derived_connected_test_plan(&siege, &observation)
            .expect("the developed economy can field a connected package");
        let air_plan = derived_connected_test_plan(&air, &observation)
            .expect("the developed economy can field a connected package");

        for plan in [&siege_plan, &air_plan] {
            let package = plan
                .connected_package
                .as_ref()
                .expect("connected admission owns an explicit package");
            assert!(!package.recon.is_empty());
            assert!(!package.suppression.is_empty());
            assert!(!package.strike.is_empty());
            assert!(
                plan.desired_artillery + plan.desired_strike_aircraft > 3,
                "a rich defended cluster should justify more than the removed fixed cohort: {package:?}"
            );
        }
    }

    #[test]
    fn admitted_connected_package_recovers_when_its_suppression_producers_are_lost() {
        let mut observation = developed_connected_obs(120);
        let mut intelligence = knowledge(&observation);
        let mut planner = StrategicPlanner::new();

        think(&mut planner, &observation, &intelligence);
        assert!(planner.air_operation().is_some_and(|operation| {
            operation.phase <= AirOperationPhase::Assemble && operation.recovery_reason.is_none()
        }));

        observation.tick += 1;
        observation.my_units.retain(|unit| !is_artillery(unit.kind));
        observation.my_buildings = vec![building(
            11,
            0,
            BuildingKind::Airworks,
            TilePos::new(5, 2),
            true,
        )];
        observation.my_queues = vec![Vec::new()];
        intelligence.update(&observation);

        let failed = think(&mut planner, &observation, &intelligence);
        let operation = planner
            .air_operation()
            .expect("the failed preparation remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::PreparationInfeasible)
        );
        assert!(
            failed
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
        );
    }

    #[test]
    fn hidden_connected_target_revalidates_current_funding_evidence() {
        let mut initial = obs(120);
        initial.visible.fill(true);
        initial.explored.fill(true);
        initial.scrap = 0;
        initial
            .my_units
            .retain(|unit| matches!(unit.kind, UnitKind::Kestrel | UnitKind::Bombard));
        initial.my_buildings = vec![
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
            building(12, 0, BuildingKind::Extractor, TilePos::new(2, 2), true),
        ];
        initial.my_queues = vec![Vec::new(); initial.my_buildings.len()];

        let plan = derived_connected_test_plan(&profile(), &initial)
            .expect("the completed Extractor forecast funds the admitted minimum");
        let package = plan
            .connected_package
            .as_ref()
            .expect("the fixture derives a connected package");
        assert_eq!(package.current_scrap, 0);
        assert!(package.forecast_scrap > 0);

        let mut operation = operation(AirOperationPhase::Assemble, initial.tick);
        operation.strike_aircraft.clear();
        let template = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let run = |retain_extractor: bool, current_scrap: u32| {
            let mut hidden = initial.clone();
            hidden.tick += 12;
            hidden.scrap = current_scrap;
            hidden.visible.fill(false);
            hidden.enemy_buildings[0].seen = false;
            if !retain_extractor {
                let retained: Vec<_> = hidden
                    .my_buildings
                    .drain(..)
                    .zip(hidden.my_queues.drain(..))
                    .filter(|(building, _)| building.kind != BuildingKind::Extractor)
                    .collect();
                (hidden.my_buildings, hidden.my_queues) = retained.into_iter().unzip();
            }
            let mut intelligence = knowledge(&initial);
            intelligence.update(&hidden);
            assert!(intelligence.buildings().iter().any(|building| {
                building.anchor == TARGET && building.evidence == ContactEvidence::Remembered
            }));
            let mut planner = template.clone();
            let decision = think(&mut planner, &hidden, &intelligence);
            (planner, decision)
        };

        let (forecast_funded, _) = run(true, 0);
        assert!(forecast_funded.air_operation().is_some_and(|operation| {
            operation.phase == AirOperationPhase::Assemble && operation.recovery_reason.is_none()
        }));

        let (bank_funded, _) = run(false, 10_000);
        assert!(bank_funded.air_operation().is_some_and(|operation| {
            operation.phase == AirOperationPhase::Assemble && operation.recovery_reason.is_none()
        }));

        let (unfunded, decision) = run(false, 0);
        let operation = unfunded
            .air_operation()
            .expect("the failed preparation remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::PreparationInfeasible)
        );
        assert!(
            decision
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
        );
        assert_eq!(decision.committed_scrap, 0);
    }

    #[test]
    fn hidden_target_revalidates_the_full_scaled_package_funding() {
        let initial = developed_connected_obs(120);
        let mut intelligence = knowledge(&initial);
        let mut hidden = initial.clone();
        hidden.tick += 12;
        hidden.scrap = 0;
        hidden.visible.fill(false);
        hidden.enemy_buildings[0].seen = false;
        hidden.my_units.retain(|unit| unit.id != UnitId(4));
        intelligence.update(&hidden);

        let plan = connected_test_plan(&initial);
        let mut operation = operation(AirOperationPhase::Recon, initial.tick);
        operation.strike_aircraft = vec![UnitId(3)];
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };

        let decision = think(&mut planner, &hidden, &intelligence);

        let operation = planner
            .air_operation()
            .expect("the infeasible scaled package remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::PreparationInfeasible)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Condor,
                ..
            }
        )));
        assert_eq!(decision.committed_scrap, 0);
    }

    #[test]
    fn stale_target_does_not_let_marginal_providers_bypass_a_lost_minimum() {
        let first = developed_connected_obs(120);
        let mut intelligence = knowledge(&first);
        let mut hidden = first.clone();
        hidden.tick += 1;
        hidden.visible.fill(false);
        hidden.enemy_buildings[0].seen = false;
        let retained: Vec<_> = hidden
            .my_buildings
            .drain(..)
            .zip(hidden.my_queues.drain(..))
            .filter(|(building, _)| building.kind != BuildingKind::Crucible)
            .collect();
        (hidden.my_buildings, hidden.my_queues) = retained.into_iter().unzip();
        hidden.my_units.retain(|unit| {
            !matches!(
                unit.kind,
                UnitKind::Bombard | UnitKind::Avalanche | UnitKind::Buzzard | UnitKind::Condor
            )
        });
        intelligence.update(&hidden);
        assert!(intelligence.buildings().iter().any(|building| {
            building.anchor == TARGET && building.evidence == ContactEvidence::Remembered
        }));
        assert!(has_producer(&hidden, UnitKind::Bombard));
        assert!(has_producer(&hidden, UnitKind::Buzzard));
        assert!(!has_producer(&hidden, UnitKind::Avalanche));
        assert!(!requirements_met(&hidden, UnitKind::Condor));

        let mut plan = connected_test_plan(&first);
        let package = plan
            .connected_package
            .as_mut()
            .expect("the admitted connected plan carries its package");
        package.suppression = vec![
            ProviderDemand {
                kind: UnitKind::Avalanche,
                count: 1,
            },
            ProviderDemand {
                kind: UnitKind::Bombard,
                count: 1,
            },
        ];
        package.strike = vec![
            ProviderDemand {
                kind: UnitKind::Condor,
                count: 1,
            },
            ProviderDemand {
                kind: UnitKind::Buzzard,
                count: 1,
            },
        ];
        package.provider_priority = vec![
            force_package::ProviderDemandTranche {
                priority: force_package::ProviderPriority::Minimum,
                family: ForceFamily::Recon,
                kind: UnitKind::Kestrel,
                count: 1,
            },
            force_package::ProviderDemandTranche {
                priority: force_package::ProviderPriority::Minimum,
                family: ForceFamily::Suppression,
                kind: UnitKind::Avalanche,
                count: 1,
            },
            force_package::ProviderDemandTranche {
                priority: force_package::ProviderPriority::Minimum,
                family: ForceFamily::Strike,
                kind: UnitKind::Condor,
                count: 1,
            },
            force_package::ProviderDemandTranche {
                priority: force_package::ProviderPriority::Marginal,
                family: ForceFamily::Suppression,
                kind: UnitKind::Bombard,
                count: 1,
            },
            force_package::ProviderDemandTranche {
                priority: force_package::ProviderPriority::Marginal,
                family: ForceFamily::Strike,
                kind: UnitKind::Buzzard,
                count: 1,
            },
        ];
        plan.desired_artillery = 2;
        plan.desired_strike_aircraft = 2;

        let mut operation = operation(AirOperationPhase::Recon, hidden.tick);
        operation.artillery.clear();
        operation.strike_aircraft.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };

        let decision = think(&mut planner, &hidden, &intelligence);

        let operation = planner
            .air_operation()
            .expect("an infeasible admitted package enters observable recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::PreparationInfeasible)
        );
        assert!(
            decision
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. })),
            "an impossible minimum must block every later provider tranche: {decision:?}"
        );
        assert_eq!(decision.committed_scrap, 0);
    }

    #[test]
    fn paid_front_queue_survives_prerequisite_loss_but_new_work_does_not() {
        fn run(reobserved_after: u32, paid: bool) -> (StrategicDecision, StrategicPlanner) {
            let admitted_at = 100;
            let deadline = 1_100;
            let mut initial = obs(admitted_at);
            initial.visible.fill(true);
            initial.explored.fill(true);
            initial.scrap = UnitKind::Condor.stats().cost;
            initial
                .my_units
                .retain(|unit| matches!(unit.kind, UnitKind::Kestrel | UnitKind::Bombard));
            initial.my_buildings = vec![
                building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
                building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
            ];
            initial.my_queues = vec![Vec::new(), Vec::new()];
            initial.my_queue_progress = vec![0, 0];

            let mut plan = connected_test_plan(&initial);
            let package = plan
                .connected_package
                .as_mut()
                .expect("the admitted plan carries its exact package");
            package.preparation_deadline = deadline;
            package.strike = vec![ProviderDemand {
                kind: UnitKind::Condor,
                count: 1,
            }];
            package.provider_priority = vec![
                ProviderDemandTranche {
                    priority: force_package::ProviderPriority::Minimum,
                    family: ForceFamily::Recon,
                    kind: UnitKind::Kestrel,
                    count: 1,
                },
                ProviderDemandTranche {
                    priority: force_package::ProviderPriority::Minimum,
                    family: ForceFamily::Suppression,
                    kind: UnitKind::Bombard,
                    count: 1,
                },
                ProviderDemandTranche {
                    priority: force_package::ProviderPriority::Minimum,
                    family: ForceFamily::Strike,
                    kind: UnitKind::Condor,
                    count: 1,
                },
            ];
            let strike = strike_capability(UnitKind::Condor, initial.faction);
            package.minimum_capability.strike = strike;
            package.useful_capability.strike = strike;
            package.chosen_capability.strike = strike;
            plan.desired_strike_aircraft = 1;
            plan.assembly_timeout = deadline - admitted_at;

            let mut operation = operation(AirOperationPhase::Assemble, admitted_at);
            operation.strike_aircraft.clear();
            let mut planner = StrategicPlanner {
                air: Some(ActiveAirOperation {
                    op: operation,
                    plan,
                }),
                standby: AirStandby::default(),
                cooldown_until: 0,
                terminal_outcome: None,
            };
            let mut intelligence = knowledge(&initial);

            let commissioned = think(&mut planner, &initial, &intelligence);
            assert!(commissioned.intents.contains(&Intent::TrainAt {
                building: BuildingId(11),
                kind: UnitKind::Condor,
            }));

            let mut later = initial;
            later.tick = admitted_at + Tick::from(reobserved_after);
            later.scrap = if paid {
                0
            } else {
                UnitKind::Condor.stats().cost
            };
            later.visible.fill(false);
            later.enemy_buildings[0].seen = false;
            later.my_buildings.truncate(1);
            later.my_queues.truncate(1);
            later.my_queue_progress.truncate(1);
            if paid {
                later.my_queues[0] = vec![UnitKind::Condor];
                later.my_queue_progress[0] = reobserved_after;
            }
            intelligence.update(&later);

            let decision = think(&mut planner, &later, &intelligence);
            (decision, planner)
        }

        for reobserved_after in [12, 24, 60] {
            let (first_decision, first_planner) = run(reobserved_after, true);
            let (second_decision, second_planner) = run(reobserved_after, true);
            assert_eq!(first_decision, second_decision);
            assert_eq!(first_planner, second_planner);

            let operation = first_planner
                .air_operation()
                .expect("the paid provider keeps the operation active");
            assert_eq!(operation.phase, AirOperationPhase::Assemble);
            assert_eq!(operation.recovery_reason, None);
            assert!(first_decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::TrainAt {
                    kind: UnitKind::Condor,
                    ..
                }
            )));
            assert_eq!(first_decision.committed_scrap, 0);
        }

        let (decision, planner) = run(12, false);
        let operation = planner
            .air_operation()
            .expect("the infeasible operation remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::PreparationInfeasible)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Condor,
                ..
            }
        )));
    }

    #[test]
    fn admitted_connected_package_recovers_when_ground_production_becomes_blocked() {
        let mut observation = developed_connected_obs(120);
        let mut intelligence = knowledge(&observation);
        let mut planner = StrategicPlanner::new();

        think(&mut planner, &observation, &intelligence);
        assert!(planner.air_operation().is_some_and(|operation| {
            operation.phase <= AirOperationPhase::Assemble && operation.recovery_reason.is_none()
        }));

        observation.tick += 1;
        observation.my_units.retain(|unit| !is_artillery(unit.kind));
        observation.known_scrap = observation
            .my_buildings
            .iter()
            .filter(|building| {
                matches!(
                    building.kind,
                    BuildingKind::Fabricator | BuildingKind::Crucible
                )
            })
            .flat_map(|building| {
                crate::tick::rect_adjacent_tiles(
                    building.anchor,
                    building.kind.tier_stats(building.tier).size,
                )
            })
            .map(|tile| (tile, 1))
            .collect();
        observation
            .known_scrap
            .sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
        observation.known_scrap.dedup_by_key(|(tile, _)| *tile);
        intelligence.update(&observation);

        think(&mut planner, &observation, &intelligence);
        let operation = planner
            .air_operation()
            .expect("the failed preparation remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::PreparationInfeasible)
        );
    }

    #[test]
    fn connected_preparation_skips_stranded_low_id_providers() {
        let mut observation = obs(300);
        observation.visible.fill(true);
        observation.explored.fill(true);
        observation.scrap = 0;
        observation.my_buildings.clear();
        observation.my_queues.clear();
        observation.my_units = vec![
            own(1, UnitKind::Kestrel, TilePos::new(2, 2)),
            own(2, UnitKind::Bombard, TilePos::new(28, 17)),
            own(3, UnitKind::Condor, TilePos::new(2, 2)),
            own(4, UnitKind::Condor, TilePos::new(2, 2)),
            own(11, UnitKind::Kestrel, TilePos::new(8, 10)),
            own(12, UnitKind::Bombard, TilePos::new(9, 10)),
        ];
        observation
            .my_units
            .extend((13..=24).map(|id| own(id, UnitKind::Condor, TilePos::new(8, 10))));
        let air_pocket = [
            TilePos::new(1, 2),
            TilePos::new(2, 1),
            TilePos::new(2, 3),
            TilePos::new(3, 2),
        ];
        let ground_pocket = [
            TilePos::new(27, 17),
            TilePos::new(28, 16),
            TilePos::new(28, 18),
            TilePos::new(29, 17),
        ];
        observation.known_peaks = air_pocket.to_vec();
        observation
            .known_peaks
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        observation.known_rock = air_pocket.into_iter().chain(ground_pocket).collect();
        observation
            .known_rock
            .sort_unstable_by_key(|tile| (tile.y, tile.x));

        let intelligence = knowledge(&observation);
        let mut operation = operation(AirOperationPhase::Recon, observation.tick);
        operation.scout = None;
        operation.artillery.clear();
        operation.strike_aircraft.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan: connected_test_plan(&observation),
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };

        let decision = think(&mut planner, &observation, &intelligence);
        let operation = planner
            .air_operation()
            .expect("reachable replacements keep preparation active");
        assert_ne!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(operation.scout, Some(UnitId(11)));
        assert_eq!(operation.artillery, [UnitId(12)]);
        assert!(!operation.strike_aircraft.is_empty());
        assert!(operation.strike_aircraft.iter().all(|id| id.0 >= 13));
        assert!(
            [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
                .iter()
                .all(|id| !decision.reservations.contains(id))
        );
    }

    #[test]
    fn committed_connected_operation_freezes_exact_members() {
        let mut battle = obs(300);
        battle.visible.fill(true);
        battle.explored.fill(true);
        battle.my_units.extend([
            own(5, UnitKind::Bombard, TilePos::new(9, 10)),
            own(6, UnitKind::Condor, TilePos::new(4, 12)),
        ]);
        battle.my_units.sort_unstable_by_key(|unit| unit.id);

        let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
        operation.artillery = vec![UnitId(2)];
        operation.strike_aircraft = vec![UnitId(3), UnitId(4)];
        let plan = connected_test_plan(&battle);
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let mut intelligence = knowledge(&battle);

        let suppression = think(&mut planner, &battle, &intelligence);
        let operation = planner
            .air_operation()
            .expect("the committed package remains active");
        assert_eq!(operation.phase, AirOperationPhase::Verify);
        assert_eq!(
            suppression.reservations,
            [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
        );
        assert!(!suppression.reservations.contains(&UnitId(5)));
        assert!(!suppression.reservations.contains(&UnitId(6)));

        battle.tick += 1;
        intelligence.update(&battle);
        let strike = think(&mut planner, &battle, &intelligence);
        assert!(strike.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(80)),
        }));
        assert_eq!(
            planner.air_operation().unwrap().strike_aircraft,
            [UnitId(3), UnitId(4)]
        );
    }

    #[test]
    fn connected_operation_does_not_wait_for_an_arbitrary_second_bombard() {
        let mut identity = profile();
        identity.primary = Specialty::Support;
        identity.secondary = Specialty::Siege;
        identity.traits.air = 60;
        identity.traits.siege = 60;

        let mut battle = obs(300);
        battle.explored.fill(true);
        let mut operation = operation(AirOperationPhase::Assemble, battle.tick);
        operation.artillery = vec![UnitId(2)];
        operation.strike_aircraft = vec![UnitId(3), UnitId(4)];
        let plan = derived_connected_test_plan(&identity, &battle)
            .expect("the observed force can field a connected package");
        assert_eq!(preferred_artillery(&identity, &battle), UnitKind::Bombard);
        assert_eq!(plan.desired_artillery, 1);
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let intel = knowledge(&battle);

        let waiting = planner.think(&identity, tuning, &battle, &intel, HOME, &[]);

        let operation = planner.air_operation().expect("operation advances");
        assert_eq!(operation.phase, AirOperationPhase::SuppressAa);
        assert_eq!(operation.artillery, [UnitId(2)]);
        assert!(waiting.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(2)],
            goal: staging(HOME, TARGET),
        }));
    }

    #[test]
    fn a_wealthy_scattering_like_bot_starts_airborne_bombers_without_lift_support() {
        let observation = wealthy_island_obs(5_016, 1);
        let intel = knowledge(&observation);
        let mut identity = profile();
        identity.primary = Specialty::Support;
        identity.secondary = Specialty::Greed;
        identity.traits.air = 48;
        identity.traits.siege = 20;
        let mut planner = StrategicPlanner::new();

        planner.think_with_lift_support(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            coordination(None),
        );

        assert_eq!(
            planner
                .air_operation()
                .expect("the disconnected enclave independently starts an air operation")
                .target,
            TARGET
        );
        let plan = planner.air_plan().expect("the operation owns a plan");
        assert_eq!(plan.suppression, AirSuppression::Airborne);
        assert!(plan.desired_strike_aircraft >= 4);
        assert_eq!(plan.desired_artillery, 0);
    }

    #[test]
    fn a_target_without_a_known_ground_doorstep_cannot_trigger_the_uncapped_island_plan() {
        let mut observation = wealthy_island_obs(5_016, 1);
        observation
            .known_rock
            .extend(crate::tick::rect_adjacent_tiles(
                TARGET,
                BuildingKind::Crucible.base_stats().size,
            ));
        observation
            .known_rock
            .sort_unstable_by_key(|tile| (tile.y, tile.x));
        observation.known_rock.dedup();
        let intel = knowledge(&observation);
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_045,
        ));
        assert_eq!(identity.primary, Specialty::Air);
        let mut planner = StrategicPlanner::new();

        let decision = planner.think(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            &[],
        );

        assert_eq!(decision, StrategicDecision::default());
        assert!(
            planner.air_operation().is_none(),
            "an objective with no known ground doorstep must not start either assault playbook"
        );
        assert!(planner.air_plan().is_none());
    }

    #[test]
    fn a_preexisting_lift_makes_the_second_starting_air_operation_inherit_its_exact_objective() {
        let mut observation = wealthy_island_obs(5_016, 1);
        let lift_target = TilePos::new(24, 15);
        observation
            .enemy_buildings
            .push(building(81, 1, BuildingKind::Foundry, lift_target, true));
        let intel = knowledge(&observation);
        let request = LiftSupportRequest {
            player: PlayerId(1),
            target: lift_target,
            planned_drops: vec![TilePos::new(22, 14), TilePos::new(23, 14)],
        };
        let mut planner = StrategicPlanner::new();

        planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            coordination(Some(&request)),
        );

        let operation = planner
            .air_operation()
            .expect("the lift's exact objective starts the matching air operation");
        assert_eq!(operation.target_player, request.player);
        assert_eq!(operation.target, request.target);
        assert_eq!(operation.target_id, Some(BuildingId(81)));
        assert_eq!(
            planner.air_plan().map(|plan| plan.suppression),
            Some(AirSuppression::Airborne)
        );
    }

    #[test]
    fn partial_ground_knowledge_stays_unknown_without_a_public_briefing() {
        let mut observation = wealthy_island_obs(5_000, 1);
        observation.known_rock.clear();
        for tile in
            crate::tick::rect_adjacent_tiles(HOME, BuildingKind::Foundry.base_stats().size).chain(
                crate::tick::rect_adjacent_tiles(TARGET, BuildingKind::Crucible.base_stats().size),
            )
        {
            explore(&mut observation, tile);
        }
        let target = BuildingContact {
            player: PlayerId(1),
            kind: BuildingKind::Crucible,
            anchor: TARGET,
            hp: BuildingKind::Crucible.base_stats().max_hp,
            tier: 0,
            built: true,
            id: Some(BuildingId(80)),
            evidence: ContactEvidence::Current,
            last_seen: Some(observation.tick),
        };

        assert_eq!(
            known_ground_connection(
                &observation,
                HOME,
                TARGET,
                BuildingKind::Crucible.base_stats().size,
                None,
            ),
            None,
            "an optimistic route through unexplored ground is not proof of either connection state"
        );
        assert!(!wealthy_island_target(
            &profile(),
            &observation,
            HOME,
            &target,
            None,
        ));

        let open_public_map = public_map_with_terrain(&observation, []);
        assert_eq!(
            known_ground_connection(
                &observation,
                HOME,
                TARGET,
                BuildingKind::Crucible.base_stats().size,
                Some(&open_public_map),
            ),
            Some(true),
            "the public map proves both endpoints share one ground component"
        );
        let divided_public_map = public_map_with_terrain(
            &observation,
            (0..observation.map_height).map(|y| (TilePos::new(16, y), Terrain::Peak)),
        );
        assert_eq!(
            known_ground_connection(
                &observation,
                HOME,
                TARGET,
                BuildingKind::Crucible.base_stats().size,
                Some(&divided_public_map),
            ),
            Some(false),
        );
        assert!(wealthy_island_target(
            &profile(),
            &observation,
            HOME,
            &target,
            Some(&divided_public_map),
        ));

        for x in HOME.x + 2..TARGET.x {
            explore(&mut observation, TilePos::new(x, HOME.y));
        }
        assert_eq!(
            known_ground_connection(
                &observation,
                HOME,
                TARGET,
                BuildingKind::Crucible.base_stats().size,
                None,
            ),
            Some(true),
        );
        assert!(
            !wealthy_island_target(&profile(), &observation, HOME, &target, None),
            "a fully explored open road keeps the ordinary ground war available"
        );
    }

    #[test]
    fn an_unexplored_remembered_target_on_the_public_home_landmass_uses_connected_recon() {
        let mut first_sighting = wealthy_island_obs(4_992, 1);
        first_sighting.known_rock.clear();
        let mut intel = knowledge(&first_sighting);

        let mut hidden = wealthy_island_obs(5_016, 1);
        hidden.known_rock.clear();
        hidden.enemy_buildings[0].seen = false;
        assert!(hidden.explored.iter().all(|explored| !explored));
        intel.update(&hidden);
        let public_map = public_map_with_terrain(&hidden, []);
        let mut planner = StrategicPlanner::new();

        planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &hidden,
            &intel,
            HOME,
            StrategicCoordination {
                public_map: Some(&public_map),
                ..coordination(None)
            },
        );

        let operation = planner
            .air_operation()
            .expect("the remembered connected objective remains eligible for reconnaissance");
        assert!(!operation.assault_admitted);
        assert_eq!(operation.target, TARGET);
        assert_eq!(
            planner.air_plan().map(|plan| plan.suppression),
            Some(AirSuppression::GroundArtillery),
            "publicly connected terrain must not enter the wealthy island doctrine"
        );
    }

    #[test]
    fn a_remembered_high_value_target_cannot_hide_a_current_island_objective() {
        let mut first_sighting = wealthy_island_obs(4_999, 1);
        let mut intel = knowledge(&first_sighting);
        let island_foundry = TARGET.offset(0, 5);
        first_sighting.tick = 5_016;
        first_sighting.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, TARGET, false),
            building(81, 1, BuildingKind::Foundry, island_foundry, true),
        ];
        intel.update(&first_sighting);
        let mut identity = profile();
        identity.primary = Specialty::Support;
        identity.secondary = Specialty::Greed;
        identity.traits.air = 48;
        identity.traits.siege = 20;
        let mut planner = StrategicPlanner::new();

        planner.think(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &first_sighting,
            &intel,
            HOME,
            &[],
        );

        let operation = planner
            .air_operation()
            .expect("the current disconnected objective starts an air operation");
        assert_eq!(operation.target_id, Some(BuildingId(81)));
        assert_eq!(operation.target, island_foundry);
        assert_eq!(
            planner.air_plan().map(|plan| plan.suppression),
            Some(AirSuppression::Airborne)
        );
    }

    #[test]
    fn a_wealthy_island_bot_reconnoiters_a_stale_building_ghost() {
        let first_sighting = wealthy_island_obs(100, 1);
        let mut intel = knowledge(&first_sighting);
        let mut hidden = wealthy_island_obs(10_008, 1);
        hidden.enemy_buildings[0].seen = false;
        hidden.my_units[0].tile = HOME;
        intel.update(&hidden);
        let mut identity = profile();
        identity.primary = Specialty::Support;
        identity.secondary = Specialty::Greed;
        identity.traits.air = 48;
        identity.traits.siege = 20;
        let mut planner = StrategicPlanner::new();

        let decision = planner.think(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &hidden,
            &intel,
            HOME,
            &[],
        );

        let operation = planner
            .air_operation()
            .expect("a persistent building ghost warrants honest reconnaissance");
        assert_eq!(operation.phase, AirOperationPhase::Recon);
        assert!(!operation.assault_admitted);
        assert_eq!(operation.target, TARGET);
        assert!(decision.intents.iter().any(|intent| matches!(
            intent,
            Intent::MoveUnits { units, .. } if units == &[UnitId(1)]
        )));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
        assert!(operation.artillery.is_empty());
        assert!(operation.strike_aircraft.is_empty());
        assert_eq!(decision.reservations, [UnitId(1)]);
        assert_eq!(decision.committed_scrap, 0);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt { kind, .. } if *kind != UnitKind::Kestrel
        )));
    }

    #[test]
    fn remembered_recon_claims_only_the_missing_scouts_capacity() {
        let first_sighting = wealthy_island_obs(4_800, 1);
        let mut intel = knowledge(&first_sighting);
        let mut ghost = wealthy_island_obs(4_992, 1);
        ghost.enemy_buildings[0].seen = false;
        ghost.my_units.retain(|unit| unit.kind != UnitKind::Kestrel);
        intel.update(&ghost);
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_045,
        ));
        assert_eq!(identity.primary, Specialty::Air);
        let mut planner = StrategicPlanner::new();

        let decision = planner.think(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &ghost,
            &intel,
            HOME,
            &[],
        );

        let operation = planner
            .air_operation()
            .expect("the remembered objective admits scout-only reconnaissance");
        assert!(!operation.assault_admitted);
        assert_eq!(operation.scout, None);
        assert!(operation.artillery.is_empty());
        assert!(operation.strike_aircraft.is_empty());
        assert_eq!(
            planner.remaining_airwork_ticks(&ghost),
            u64::from(UnitKind::Kestrel.stats().train_ticks)
        );
        assert_eq!(decision.committed_scrap, UnitKind::Kestrel.stats().cost);
        assert!(matches!(
            decision.intents.as_slice(),
            [Intent::TrainAt {
                kind: UnitKind::Kestrel,
                ..
            }]
        ));
    }

    #[test]
    fn remembered_recon_gives_a_late_scout_a_fresh_flight_window() {
        let first_sighting = wealthy_island_obs(4_800, 1);
        let mut intel = knowledge(&first_sighting);
        let mut ghost = wealthy_island_obs(4_992, 1);
        ghost.enemy_buildings[0].seen = false;
        ghost.my_units.retain(|unit| unit.kind != UnitKind::Kestrel);
        intel.update(&ghost);
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_045,
        ));
        let mut tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        tuning.tactical_memory = 5_000;
        let mut planner = StrategicPlanner::new();

        planner.think(&identity, tuning, &ghost, &intel, HOME, &[]);
        let admitted_at = planner
            .air_operation()
            .expect("the remembered objective begins scout-only reconnaissance")
            .phase_started_at;
        assert_eq!(admitted_at, ghost.tick);

        let mut scout_ready = ghost;
        scout_ready.tick = admitted_at
            + phase_timeout(
                AirOperationPhase::Recon,
                &AirPlan::island(&identity, &scout_ready),
            )
            - 12;
        scout_ready.my_units.push(own(99, UnitKind::Kestrel, HOME));
        scout_ready.my_units.sort_unstable_by_key(|unit| unit.id);
        intel.update(&scout_ready);
        let dispatch = planner.think(&identity, tuning, &scout_ready, &intel, HOME, &[]);

        let operation = planner
            .air_operation()
            .expect("the late scout must extend reconnaissance instead of timing out");
        assert_eq!(operation.phase, AirOperationPhase::Recon);
        assert_eq!(operation.scout, Some(UnitId(99)));
        assert_eq!(operation.phase_started_at, scout_ready.tick);
        assert_eq!(operation.recovery_reason, None);
        assert!(dispatch.intents.iter().any(|intent| matches!(
            intent,
            Intent::MoveUnits { units, .. } if units == &[UnitId(99)]
        )));

        let assigned_at = scout_ready.tick;
        let mut after_old_deadline = scout_ready;
        after_old_deadline.tick = admitted_at
            + phase_timeout(
                AirOperationPhase::Recon,
                &AirPlan::island(&identity, &after_old_deadline),
            )
            + 12;
        let scout = after_old_deadline
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(99))
            .expect("the assigned scout remains alive");
        scout.idle = false;
        scout.tile = HOME.offset(1, 0);
        intel.update(&after_old_deadline);
        let after = planner.think(&identity, tuning, &after_old_deadline, &intel, HOME, &[]);

        let operation = planner
            .air_operation()
            .expect("the reset window must remain active past the original deadline");
        assert_eq!(operation.phase, AirOperationPhase::Recon);
        assert_eq!(operation.scout, Some(UnitId(99)));
        assert_eq!(operation.phase_started_at, assigned_at);
        assert_eq!(operation.recovery_reason, None);
        assert!(
            after.intents.is_empty(),
            "the accepted flight should persist"
        );
    }

    #[test]
    fn remembered_recon_aborts_when_the_scout_cannot_cross_known_peaks() {
        let first_sighting = wealthy_island_obs(4_800, 1);
        let mut intel = knowledge(&first_sighting);
        let mut ghost = wealthy_island_obs(4_992, 1);
        ghost.enemy_buildings[0].seen = false;
        ghost.my_units[0].tile = HOME;
        ghost.known_peaks = (0..ghost.map_height).map(|y| TilePos::new(8, y)).collect();
        intel.update(&ghost);
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_045,
        ));
        assert_eq!(identity.primary, Specialty::Air);
        let mut planner = StrategicPlanner::new();

        let decision = planner.think(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &ghost,
            &intel,
            HOME,
            &[],
        );

        let operation = planner
            .air_operation()
            .expect("the failed reconnaissance remains observable for one think");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
        assert_eq!(decision.committed_scrap, 0);
    }

    #[test]
    fn remembered_connected_recon_respects_publicly_known_peaks_before_sighting_them() {
        let first_sighting = developed_connected_obs(96);
        let mut intel = knowledge(&first_sighting);
        let mut ghost = developed_connected_obs(120);
        ghost.scrap = 0;
        ghost.visible.fill(false);
        ghost.explored.fill(false);
        ghost.enemy_buildings[0].seen = false;
        ghost.my_units[0].tile = HOME;
        intel.update(&ghost);
        let public_map = public_map_with_terrain(
            &ghost,
            (0..ghost.map_height).map(|y| (TilePos::new(8, y), Terrain::Peak)),
        );
        let identity = profile();
        let mut planner = StrategicPlanner::new();

        let decision = planner.think_with_lift_support(
            &identity,
            DifficultyTuning::for_level(identity.difficulty),
            &ghost,
            &intel,
            HOME,
            StrategicCoordination {
                public_map: Some(&public_map),
                ..coordination(None)
            },
        );

        let operation = planner
            .air_operation()
            .expect("the refused reconnaissance remains observable during recovery");
        assert!(!operation.assault_admitted);
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { goal, .. } if goal.x > 8
        )));
    }

    #[test]
    fn every_difficulty_waits_for_shared_current_sight_before_claiming_an_assault() {
        let mut snapshots = Vec::new();
        for difficulty in BotDifficulty::ALL {
            let tuning = DifficultyTuning::for_level(difficulty);
            let identity = ResolvedProfile::resolve(BotConfig::scripted(
                difficulty,
                BotStance::Balanced,
                20_045,
            ));
            assert_eq!(identity.primary, Specialty::Air);
            let first = wealthy_island_obs(4_800, 1);
            let mut intel = knowledge(&first);
            let mut ghost = wealthy_island_obs(4_992, 1);
            ghost.enemy_buildings[0].seen = false;
            ghost.my_units[0].tile = HOME;
            intel.update(&ghost);
            let mut planner = StrategicPlanner::new();

            let recon = planner.think(&identity, tuning, &ghost, &intel, HOME, &[]);
            let ghost_operation = planner.air_operation().unwrap();
            assert!(!ghost_operation.assault_admitted, "{difficulty:?}");
            let admitted_at = planner
                .air_admitted_at()
                .expect("remembered reconnaissance owns an admission tick");
            assert_eq!(admitted_at, ghost.tick, "{difficulty:?}");
            assert!(ghost_operation.artillery.is_empty(), "{difficulty:?}");
            assert!(ghost_operation.strike_aircraft.is_empty(), "{difficulty:?}");
            assert_eq!(recon.reservations, [UnitId(1)], "{difficulty:?}");
            assert_eq!(recon.committed_scrap, 0, "{difficulty:?}");

            let current = wealthy_island_obs(5_016, 1);
            intel.update(&current);
            planner.think(&identity, tuning, &current, &intel, HOME, &[]);
            let admitted = planner.air_operation().unwrap();
            assert!(admitted.assault_admitted, "{difficulty:?}");
            assert_eq!(admitted.started_at, 5_016, "{difficulty:?}");
            assert_eq!(
                planner.air_admitted_at(),
                Some(admitted_at),
                "reacquiring the target must not reorder the operation behind later commitments"
            );
            let plan = planner.air_plan().unwrap();
            snapshots.push((
                admitted.artillery.clone(),
                admitted.strike_aircraft.clone(),
                plan.desired_artillery,
                plan.desired_strike_aircraft,
                plan.desired_screen,
            ));
        }

        assert!(snapshots.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn remembered_connected_target_admits_the_normal_combined_plan_when_reacquired() {
        let reveal_ground_route = |observation: &mut Observation| {
            observation.known_rock.clear();
            observation.my_buildings.push(building(
                40,
                0,
                BuildingKind::Fabricator,
                TilePos::new(12, 2),
                true,
            ));
            observation.my_queues.push(Vec::new());
            for x in HOME.x + 2..TARGET.x {
                explore(observation, TilePos::new(x, HOME.y));
            }
        };
        let first = {
            let mut observation = wealthy_island_obs(4_800, 1);
            reveal_ground_route(&mut observation);
            observation
        };
        let mut intel = knowledge(&first);
        let mut ghost = wealthy_island_obs(4_992, 1);
        reveal_ground_route(&mut ghost);
        ghost.enemy_buildings[0].seen = false;
        ghost.my_units[0].tile = HOME;
        intel.update(&ghost);
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_045,
        ));
        assert_eq!(identity.primary, Specialty::Air);
        let mut planner = StrategicPlanner::new();

        let recon = planner.think(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &ghost,
            &intel,
            HOME,
            &[],
        );
        let operation = planner
            .air_operation()
            .expect("the remembered connected objective starts scout-only reconnaissance");
        assert!(!operation.assault_admitted);
        assert_eq!(recon.reservations, [UnitId(1)]);
        assert_eq!(
            planner.air_plan().map(|plan| plan.suppression),
            Some(AirSuppression::GroundArtillery)
        );

        let mut current = wealthy_island_obs(5_016, 1);
        reveal_ground_route(&mut current);
        intel.update(&current);
        planner.think(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &current,
            &intel,
            HOME,
            &[],
        );

        let admitted = planner
            .air_operation()
            .expect("current sight admits the connected assault");
        assert!(admitted.assault_admitted);
        assert_eq!(admitted.started_at, current.tick);
        let plan = planner.air_plan().expect("the assault owns a plan");
        assert_eq!(plan.suppression, AirSuppression::GroundArtillery);
        assert!(plan.desired_artillery > 0);
        assert_eq!(plan.desired_screen, 0);
        assert!(plan.connected_package.is_some());
    }

    #[test]
    fn a_large_island_wave_schedules_screen_then_bombers_across_airworks() {
        let observation = wealthy_island_obs(5_016, 3);
        let intel = knowledge(&observation);
        let mut identity = profile();
        identity.primary = Specialty::Support;
        identity.secondary = Specialty::Greed;
        identity.traits.air = 48;
        identity.traits.siege = 20;
        let mut planner = StrategicPlanner::new();

        let decision = planner.think(
            &identity,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            &[],
        );

        let plan = planner.air_plan().expect("the operation owns a plan");
        assert_eq!(plan.desired_strike_aircraft, 6);
        assert_eq!(plan.desired_screen, 2);
        let training: Vec<_> = decision
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::TrainAt { building, kind } => Some((*building, *kind)),
                _ => None,
            })
            .collect();
        assert_eq!(
            &training[..2],
            &[
                (BuildingId(23), UnitKind::Buzzard),
                (BuildingId(24), UnitKind::Buzzard),
            ],
            "the cheap suppression screen is available before the long bomber batch"
        );
        assert_eq!(
            training
                .iter()
                .filter(|(_, kind)| *kind == UnitKind::Condor)
                .count(),
            4,
            "two existing strike aircraft contribute to the current six-aircraft wing"
        );
        for airworks in [BuildingId(23), BuildingId(24), BuildingId(25)] {
            assert_eq!(
                training
                    .iter()
                    .filter(|(building, _)| *building == airworks)
                    .count(),
                2,
                "equal queue loads must spread deterministically"
            );
        }
        assert_eq!(
            planner.remaining_airwork_ticks(&observation),
            3_560,
            "capacity planning sees exact unfinished screen and bomber training time"
        );
        let mut queued = observation.clone();
        queued.my_queues[3] = vec![UnitKind::Buzzard, UnitKind::Condor];
        queued.my_queues[4] = vec![UnitKind::Buzzard, UnitKind::Condor];
        queued.my_queues[5] = vec![UnitKind::Condor, UnitKind::Condor];
        assert_eq!(
            planner.remaining_airwork_ticks(&queued),
            3_560,
            "queued aircraft remain real factory work until they complete"
        );
        assert!(
            plan.assembly_timeout >= 2_660,
            "the deadline covers the faction's exact scheduled production time"
        );
    }

    #[test]
    fn connected_capacity_uses_exact_paid_front_progress() {
        let mut observation = obs(5_016);
        observation.my_buildings = vec![
            building(10, 0, BuildingKind::Airworks, TilePos::new(2, 2), true),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
        ];
        observation.my_queues = vec![vec![UnitKind::Condor], vec![UnitKind::Condor]];
        observation.my_queue_progress = vec![UnitKind::Condor.stats().train_ticks, 1];
        let mut plan = connected_test_plan(&observation);
        let package = plan
            .connected_package
            .as_mut()
            .expect("the operation owns a connected package");
        package.recon.clear();
        package.strike = vec![ProviderDemand {
            kind: UnitKind::Condor,
            count: 6,
        }];
        let planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation(AirOperationPhase::Assemble, observation.tick),
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };

        assert_eq!(
            planner.remaining_airwork_ticks(&observation),
            2_400,
            "a ready-but-not-yet-spawned front still needs one authoritative production tick"
        );
        observation.my_queue_progress.fill(0);
        assert_eq!(
            planner.remaining_airwork_ticks(&observation),
            3_200,
            "without visible progress all four missing bombers retain their full training work"
        );
    }

    #[test]
    fn every_difficulty_freezes_the_same_growing_air_roster_at_shared_admission() {
        let mut snapshots = Vec::new();
        for difficulty in BotDifficulty::ALL {
            let tuning = DifficultyTuning::for_level(difficulty);
            let identity = ResolvedProfile::resolve(BotConfig::scripted(
                difficulty,
                BotStance::Balanced,
                20_045,
            ));
            assert!(matches!(identity.primary, Specialty::Air));
            let mut planner = StrategicPlanner::new();
            let mut shared = wealthy_island_obs(5_016, 1);
            let mut intel = None;
            for tick in 4_993_u64..=5_016 {
                if !tick.is_multiple_of(tuning.cadence) {
                    continue;
                }
                shared = wealthy_island_obs(tick, 1);
                add_renewable_economy(&mut shared, usize::try_from((tick - 4_992) / 3).unwrap());
                let current_intel = intel.get_or_insert_with(|| knowledge(&shared));
                if current_intel.observed_at() != Some(shared.tick) {
                    current_intel.update(&shared);
                }
                planner.think(&identity, tuning, &shared, current_intel, HOME, &[]);
                if tick < 5_016 {
                    assert!(
                        planner.air_operation().is_none(),
                        "{difficulty:?} at {tick}"
                    );
                }
            }
            let expected = AirPlan::island(&identity, &shared);
            let operation = planner.air_operation().unwrap();
            let plan = planner.air_plan().unwrap();

            assert_eq!(operation.started_at, 5_016, "{difficulty:?}");
            assert_eq!(
                plan.desired_strike_aircraft,
                expected.desired_strike_aircraft
            );
            assert_eq!(plan.desired_screen, expected.desired_screen);
            assert_eq!(plan.assembly_timeout, expected.assembly_timeout);
            snapshots.push((plan.desired_strike_aircraft, plan.desired_screen));
        }

        assert!(snapshots.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn fighter_reinforcements_do_not_inflate_an_admitted_airborne_plan() {
        let mut reinforced = wealthy_island_obs(5_016, 1);
        let identity = profile();
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut intel = knowledge(&reinforced);
        let mut planner = StrategicPlanner::new();
        planner.think(&identity, tuning, &reinforced, &intel, HOME, &[]);
        let frozen = planner.air_plan().cloned().unwrap();

        reinforced.my_units.extend((100..=139).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(6 + i32::try_from(id % 8).unwrap(), 6),
            )
        }));
        reinforced.my_units.sort_unstable_by_key(|unit| unit.id);
        reinforced.tick += tuning.cadence;
        intel.update(&reinforced);
        planner.think(&identity, tuning, &reinforced, &intel, HOME, &[]);

        let current = AirPlan::island(&identity, &reinforced);
        assert!(current.desired_strike_aircraft > frozen.desired_strike_aircraft);
        assert!(current.desired_screen > frozen.desired_screen);
        assert_eq!(planner.air_plan().unwrap(), &frozen);
    }

    #[test]
    fn admitted_airborne_plan_does_not_shrink_after_economy_and_roster_losses() {
        let mut wealthy = wealthy_island_obs(5_016, 1);
        add_renewable_economy(&mut wealthy, 12);
        wealthy.my_units.extend((100..=159).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(6 + i32::try_from(id % 8).unwrap(), 6),
            )
        }));
        wealthy.my_units.sort_unstable_by_key(|unit| unit.id);
        let identity = profile();
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut intel = knowledge(&wealthy);
        let mut planner = StrategicPlanner::new();
        planner.think(&identity, tuning, &wealthy, &intel, HOME, &[]);
        let frozen = planner.air_plan().cloned().unwrap();

        let depleted = wealthy_island_obs(5_019, 1);
        let current = AirPlan::island(&identity, &depleted);
        assert!(current.observed_renewable < frozen.observed_renewable);
        assert!(current.observed_fighters < frozen.observed_fighters);
        assert!(current.desired_strike_aircraft < frozen.desired_strike_aircraft);
        assert!(current.desired_screen < frozen.desired_screen);

        intel.update(&depleted);
        planner.think(&identity, tuning, &depleted, &intel, HOME, &[]);

        assert_eq!(planner.air_plan().unwrap(), &frozen);
    }

    #[test]
    fn suppression_commitment_freezes_air_force_targets_despite_a_later_surge() {
        let mut battle = wealthy_island_obs(5_000, 1);
        let mut plan = AirPlan::island(&profile(), &battle);
        let bomber_kind = Role::Bomber.unit_for(battle.faction);
        let screen_kind = Role::AirGround.unit_for(battle.faction);
        let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
        for offset in 0..plan
            .desired_strike_aircraft
            .saturating_sub(operation.strike_aircraft.len())
        {
            let id = 100 + u32::try_from(offset).unwrap();
            battle
                .my_units
                .push(own(id, bomber_kind, TilePos::new(4, 6)));
            operation.strike_aircraft.push(UnitId(id));
        }
        for offset in 0..plan.desired_screen {
            let id = 200 + u32::try_from(offset).unwrap();
            battle
                .my_units
                .push(own(id, screen_kind, TilePos::new(5, 6)));
            plan.screen.push(UnitId(id));
        }
        battle.my_units.sort_unstable_by_key(|unit| unit.id);
        let frozen_strike_aircraft = plan.desired_strike_aircraft;
        let frozen_screen = plan.desired_screen;
        let frozen_renewable = plan.observed_renewable;
        let frozen_fighters = plan.observed_fighters;
        let frozen_timeout = plan.assembly_timeout;
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };

        add_renewable_economy(&mut battle, 12);
        battle.my_units.extend((300..=379).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(6 + i32::try_from(id % 8).unwrap(), 7),
            )
        }));
        battle.my_units.sort_unstable_by_key(|unit| unit.id);
        battle.tick += 1;
        let unconstrained = AirPlan::island(&profile(), &battle);
        assert!(unconstrained.desired_strike_aircraft > frozen_strike_aircraft);
        assert!(unconstrained.desired_screen > frozen_screen);
        let intel = knowledge(&battle);

        think(&mut planner, &battle, &intel);

        let committed = planner
            .air_plan()
            .expect("the committed operation retains its frozen plan");
        assert_eq!(committed.desired_strike_aircraft, frozen_strike_aircraft);
        assert_eq!(committed.desired_screen, frozen_screen);
        assert_eq!(committed.observed_renewable, frozen_renewable);
        assert_eq!(committed.observed_fighters, frozen_fighters);
        assert_eq!(committed.assembly_timeout, frozen_timeout);
    }

    #[test]
    fn airborne_suppression_hits_visible_flak_then_waits_for_fresh_clearance() {
        let mut battle = wealthy_island_obs(5_000, 2);
        see_approach(&mut battle);
        battle.my_units[0].idle = false;
        battle
            .my_units
            .push(own(30, UnitKind::Buzzard, TilePos::new(5, 8)));
        battle
            .my_units
            .push(own(31, UnitKind::Buzzard, TilePos::new(5, 12)));
        battle.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(20, 10),
            true,
        ));
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.screen = vec![UnitId(30), UnitId(31)];
        let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
        operation.artillery.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let intel = knowledge(&battle);

        let suppression = think(&mut planner, &battle, &intel);
        assert!(suppression.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4), UnitId(30), UnitId(31)],
            target: Target::Building(BuildingId(81)),
        }));
        assert!(suppression.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));

        battle.tick += 1;
        let intel = knowledge(&battle);
        let repeated = think(&mut planner, &battle, &intel);
        assert!(
            repeated.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits {
                    target: Target::Building(BuildingId(81)),
                    ..
                }
            )),
            "an unchanged suppression target must not reset bomber egress"
        );

        battle.tick += 1;
        battle
            .enemy_buildings
            .retain(|building| building.id != BuildingId(81));
        let intel = knowledge(&battle);
        let verification = think(&mut planner, &battle, &intel);
        assert_eq!(
            planner.air_operation().map(|operation| operation.phase),
            Some(AirOperationPhase::Verify),
            "decision={verification:?}, cooldown={}",
            planner.cooldown_until()
        );
        assert!(verification.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));
        assert!(verification.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(3), UnitId(4)],
            goal: landing_pad(&battle, HOME).unwrap(),
        }));
        assert!(verification.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(30), UnitId(31)],
            goal: HOME,
        }));

        battle
            .my_units
            .retain(|unit| !matches!(unit.id, UnitId(4) | UnitId(31)));
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        battle.tick = battle
            .tick
            .saturating_add(tuning.reaction_delay)
            .saturating_add(tuning.commitment_hesitation)
            .saturating_add(1);
        let intel = knowledge(&battle);
        let strike = think(&mut planner, &battle, &intel);
        assert!(strike.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(30)],
            target: Target::Building(BuildingId(80)),
        }));
    }

    #[test]
    fn airborne_corridor_actions_distinguish_current_remembered_and_absent_static_aa() {
        let flak_anchor = TilePos::new(12, 10);
        let planner_for = |observation: &Observation| {
            let mut operation = operation(AirOperationPhase::SuppressAa, observation.tick);
            operation.artillery.clear();
            let mut plan = AirPlan::island(&profile(), observation);
            plan.desired_strike_aircraft = 2;
            plan.desired_screen = 0;
            plan.screen.clear();
            StrategicPlanner {
                air: Some(ActiveAirOperation {
                    op: operation,
                    plan,
                }),
                standby: AirStandby::default(),
                cooldown_until: 0,
                terminal_outcome: None,
            }
        };
        let status = |planner: &StrategicPlanner,
                      observation: &Observation,
                      intelligence: &StrategicIntelligence| {
            airborne_corridor_status(
                &planner.air.as_ref().unwrap().op,
                planner.air_plan().unwrap(),
                observation,
                intelligence,
                HOME,
                &[],
            )
        };

        let mut current = wealthy_island_obs(5_000, 2);
        see_approach(&mut current);
        current
            .enemy_buildings
            .push(building(81, 1, BuildingKind::FlakTurret, flak_anchor, true));
        let mut intelligence = knowledge(&current);
        let mut current_planner = planner_for(&current);
        assert_eq!(
            status(&current_planner, &current, &intelligence),
            AirborneCorridorStatus::Defended
        );
        let current_decision = think(&mut current_planner, &current, &intelligence);
        assert_eq!(
            current_planner.air_operation().unwrap().phase,
            AirOperationPhase::SuppressAa
        );
        assert!(current_decision.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(81)),
        }));
        assert!(current_decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));

        let mut remembered = current.clone();
        remembered.tick += 1;
        remembered
            .enemy_buildings
            .iter_mut()
            .find(|building| building.id == BuildingId(81))
            .unwrap()
            .seen = false;
        let (flak_width, flak_height) = BuildingKind::FlakTurret.base_stats().size;
        for dy in 0..flak_height {
            for dx in 0..flak_width {
                let tile = flak_anchor.offset(dx, dy);
                let index = usize::try_from(tile.y * remembered.map_width + tile.x).unwrap();
                remembered.visible[index] = false;
            }
        }
        intelligence.update(&remembered);
        let mut remembered_planner = planner_for(&remembered);
        assert_eq!(
            status(&remembered_planner, &remembered, &intelligence),
            AirborneCorridorStatus::NeedsRecon
        );
        let remembered_decision = think(&mut remembered_planner, &remembered, &intelligence);
        assert_eq!(
            remembered_planner.air_operation().unwrap().phase,
            AirOperationPhase::SuppressAa
        );
        assert!(
            remembered_planner
                .air_operation()
                .unwrap()
                .scout_dispatch
                .is_some(),
            "remembered static AA must trigger a fog-honest look instead of a blind commitment"
        );
        assert!(remembered_decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));

        let mut absent = remembered.clone();
        absent.tick += 1;
        absent
            .enemy_buildings
            .retain(|building| building.id != BuildingId(81));
        for dy in 0..flak_height {
            for dx in 0..flak_width {
                let tile = flak_anchor.offset(dx, dy);
                let index = usize::try_from(tile.y * absent.map_width + tile.x).unwrap();
                absent.visible[index] = true;
                absent.explored[index] = true;
            }
        }
        intelligence.update(&absent);
        let mut absent_planner = planner_for(&absent);
        assert_eq!(
            status(&absent_planner, &absent, &intelligence),
            AirborneCorridorStatus::Clear
        );
        let absent_decision = think(&mut absent_planner, &absent, &intelligence);
        assert_eq!(
            absent_planner.air_operation().unwrap().phase,
            AirOperationPhase::Verify,
            "only fresh negative evidence may advance the operation"
        );
        assert!(absent_decision.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(3), UnitId(4)],
            goal: landing_pad(&absent, HOME).unwrap(),
        }));
        assert!(absent_decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn ground_suppression_requires_fresh_negative_evidence_across_the_air_corridor() {
        let flak_anchor = TilePos::new(12, 5);
        let planner_for = |tick| {
            let observation = obs(tick);
            StrategicPlanner {
                air: Some(ActiveAirOperation {
                    op: operation(AirOperationPhase::SuppressAa, tick),
                    plan: connected_test_plan(&observation),
                }),
                standby: AirStandby::default(),
                cooldown_until: 0,
                terminal_outcome: None,
            }
        };

        let mut current = obs(400);
        see_approach(&mut current);
        see_building_footprint(&mut current, TARGET, BuildingKind::Crucible);
        explore(&mut current, staging(HOME, TARGET));
        current
            .enemy_buildings
            .push(building(81, 1, BuildingKind::FlakTurret, flak_anchor, true));
        let mut intel = knowledge(&current);
        assert_eq!(
            targetable_corridor_flak(&intel, HOME, TARGET, &[]),
            Some(BuildingId(81)),
            "the off-line emplacement must cover the midflight corridor"
        );
        let mut current_planner = planner_for(current.tick);
        let current_decision = think(&mut current_planner, &current, &intel);
        assert_eq!(
            current_planner.air_operation().unwrap().phase,
            AirOperationPhase::SuppressAa
        );
        assert!(
            current_planner
                .air_operation()
                .unwrap()
                .scout_dispatch
                .is_some(),
            "AA outside artillery's target area must hold the wing for reconnaissance"
        );
        assert!(current_decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { units, .. } | Intent::AttackMoveUnits { units, .. }
                if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
        )));
        assert!(current_decision.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(3), UnitId(4)],
            goal: landing_pad(&current, HOME).unwrap(),
        }));

        let mut remembered = current.clone();
        remembered.tick += 1;
        remembered
            .enemy_buildings
            .iter_mut()
            .find(|building| building.id == BuildingId(81))
            .expect("the observed Flak remains as a ghost")
            .seen = false;
        let flak_index =
            usize::try_from(flak_anchor.y * remembered.map_width + flak_anchor.x).unwrap();
        remembered.visible[flak_index] = false;
        intel.update(&remembered);
        assert!(!corridor_clear(&intel, HOME, TARGET, &[]));
        let mut remembered_planner = planner_for(remembered.tick);
        let remembered_decision = think(&mut remembered_planner, &remembered, &intel);
        assert_eq!(
            remembered_planner.air_operation().unwrap().phase,
            AirOperationPhase::SuppressAa
        );
        assert!(
            remembered_planner
                .air_operation()
                .unwrap()
                .scout_dispatch
                .is_some(),
            "remembered corridor AA must be reconnoitered"
        );
        assert!(remembered_decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { units, .. } | Intent::AttackMoveUnits { units, .. }
                if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
        )));

        let mut absent = remembered.clone();
        absent.tick += 1;
        absent
            .enemy_buildings
            .retain(|building| building.id != BuildingId(81));
        absent.visible[flak_index] = true;
        absent.explored[flak_index] = true;
        intel.update(&absent);
        assert!(corridor_clear(&intel, HOME, TARGET, &[]));
        let mut absent_planner = planner_for(absent.tick);
        let absent_decision = think(&mut absent_planner, &absent, &intel);
        assert_eq!(
            absent_planner.air_operation().unwrap().phase,
            AirOperationPhase::Verify,
            "fresh negative evidence may advance the combined operation"
        );
        assert!(absent_decision.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(3), UnitId(4)],
            goal: landing_pad(&absent, HOME).unwrap(),
        }));
    }

    #[test]
    fn expired_static_aa_memory_does_not_override_current_mobile_corridor_evidence() {
        let flak_anchor = TilePos::new(12, 5);
        let (mut first, mut planner) =
            wealthy_airborne_operation(AirOperationPhase::SuppressAa, UnitKind::Sentinel, 1);
        first.enemy_units[0].tile = TilePos::new(12, 10);
        first
            .enemy_buildings
            .push(building(81, 1, BuildingKind::FlakTurret, flak_anchor, true));
        let mut intel = knowledge(&first);

        let mut stale = first;
        stale.tick += 4_000;
        stale
            .enemy_buildings
            .iter_mut()
            .find(|building| building.id == BuildingId(81))
            .expect("the static source remains represented as a ghost")
            .seen = false;
        let flak_index = usize::try_from(flak_anchor.y * stale.map_width + flak_anchor.x).unwrap();
        stale.visible[flak_index] = false;
        intel.update(&stale);
        let operation = planner
            .air_op_mut()
            .expect("the test operation remains active");
        operation.started_at = stale.tick - 50;
        operation.phase_started_at = stale.tick - 50;

        let assessment = intel.air_defense_at(TilePos::new(12, 10));
        assert!(assessment.sources.iter().any(|source| {
            matches!(source.source, AirDefenseSource::Unit { .. })
                && source.evidence == ContactEvidence::Current
        }));
        assert!(assessment.sources.iter().any(|source| {
            matches!(source.source, AirDefenseSource::Building { .. })
                && source.evidence == ContactEvidence::Remembered
                && source.confidence == 0
        }));
        assert_eq!(
            airborne_corridor_status(
                &planner.air.as_ref().unwrap().op,
                planner.air_plan().unwrap(),
                &stale,
                &intel,
                HOME,
                &[],
            ),
            AirborneCorridorStatus::Clear,
            "expired static memory must not turn a currently observed mobile threat into stale uncertainty"
        );

        let decision = think(&mut planner, &stale, &intel);
        assert_eq!(
            planner.air_operation().unwrap().phase,
            AirOperationPhase::Verify
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(81)),
                ..
            }
        )));
    }

    #[test]
    fn airborne_suppression_does_not_ignore_visible_midflight_flak() {
        let mut battle = wealthy_island_obs(5_000, 2);
        see_approach(&mut battle);
        battle.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(12, 10),
            true,
        ));
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
        operation.artillery.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let intel = knowledge(&battle);

        let decision = think(&mut planner, &battle, &intel);

        assert!(decision.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(81)),
        }));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));
    }

    #[test]
    fn an_assembled_wealthy_wave_accepts_mobile_aa_but_still_suppresses_current_flak() {
        let (battle, mut planner) =
            wealthy_airborne_operation(AirOperationPhase::Assemble, UnitKind::Sentinel, 12);
        planner.air_op_mut().unwrap().strike_aircraft.clear();
        planner.air_plan_mut().unwrap().screen.clear();
        let intel = knowledge(&battle);

        let assembly = think(&mut planner, &battle, &intel);
        let frozen = planner.air_operation().unwrap();
        assert_eq!(frozen.phase, AirOperationPhase::SuppressAa);
        assert_eq!(frozen.strike_aircraft.len(), 10);
        assert_eq!(planner.air_plan().unwrap().screen.len(), 5);
        let expected: Vec<_> = std::iter::once(UnitId(1))
            .chain((100..110).chain(200..205).map(UnitId))
            .collect();
        assert_eq!(assembly.reservations, expected);
        let scout_goal = frozen
            .scout_dispatch
            .expect("assembly dispatches the operation scout")
            .1;

        let mut battle = battle;
        let scout = battle
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(1))
            .unwrap();
        scout.tile = scout_goal;
        scout.idle = false;
        battle.tick += 1;
        let intel = knowledge(&battle);
        let verification = think(&mut planner, &battle, &intel);
        assert_eq!(
            planner.air_operation().map(|operation| operation.phase),
            Some(AirOperationPhase::Verify),
            "weak mobile AA should not cancel the frozen 10-bomber/5-screen wave: {verification:?}; {:?}",
            planner.air_operation()
        );

        battle.tick += 1;
        let intel = knowledge(&battle);
        let strike = think(&mut planner, &battle, &intel);
        let expected: Vec<_> = (100..110).chain(200..205).map(UnitId).collect();
        assert_eq!(
            planner.air_operation().map(|operation| operation.phase),
            Some(AirOperationPhase::Strike)
        );
        assert!(strike.intents.contains(&Intent::AttackUnits {
            units: expected,
            target: Target::Building(BuildingId(80)),
        }));

        let (mut defended, mut flak_planner) =
            wealthy_airborne_operation(AirOperationPhase::Assemble, UnitKind::Sentinel, 12);
        flak_planner.air_op_mut().unwrap().strike_aircraft.clear();
        flak_planner.air_plan_mut().unwrap().screen.clear();
        defended.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TARGET.offset(-4, 0),
            true,
        ));
        let intel = knowledge(&defended);
        let assembly = think(&mut flak_planner, &defended, &intel);
        assert_eq!(
            flak_planner.air_operation().unwrap().phase,
            AirOperationPhase::SuppressAa
        );
        let expected_reservations: Vec<_> = std::iter::once(UnitId(1))
            .chain((100..110).chain(200..205).map(UnitId))
            .collect();
        assert_eq!(assembly.reservations, expected_reservations);
        let scout_goal = flak_planner
            .air_operation()
            .unwrap()
            .scout_dispatch
            .expect("assembly dispatches the operation scout")
            .1;
        let scout = defended
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(1))
            .unwrap();
        scout.tile = scout_goal;
        scout.idle = false;

        defended.tick += 1;
        let intel = knowledge(&defended);
        let suppression = think(&mut flak_planner, &defended, &intel);
        assert_eq!(
            flak_planner.air_operation().unwrap().phase,
            AirOperationPhase::SuppressAa,
            "current Flak remains a hard gate even for the full wealthy wing"
        );
        assert!(suppression.intents.contains(&Intent::AttackUnits {
            units: (100..110).chain(200..205).map(UnitId).collect(),
            target: Target::Building(BuildingId(81)),
        }));
        assert!(suppression.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));
    }

    #[test]
    fn a_wealthy_airborne_wave_rejects_overwhelming_mobile_aa_before_and_after_suppression() {
        for phase in [
            AirOperationPhase::SuppressAa,
            AirOperationPhase::Verify,
            AirOperationPhase::Strike,
        ] {
            let (battle, mut planner) = wealthy_airborne_operation(phase, UnitKind::Flakhound, 7);
            let intel = knowledge(&battle);

            let decision = think(&mut planner, &battle, &intel);
            let operation = planner
                .air_operation()
                .expect("recovery remains observable for one think");

            assert_eq!(operation.phase, AirOperationPhase::Recover, "{phase:?}");
            assert_eq!(
                operation.recovery_reason,
                Some(AirRecoveryReason::NewAirDefense),
                "{phase:?}"
            );
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
            )));
        }
    }

    #[test]
    fn shared_air_support_clears_and_reconnoiters_the_frozen_drop_envelope_before_release() {
        let mut battle = wealthy_island_obs(5_000, 1);
        see_approach(&mut battle);
        battle.my_units[0].tile = HOME;
        let drop = TilePos::new(24, 15);
        let flak = TilePos::new(24, 19);
        battle
            .enemy_buildings
            .push(building(81, 1, BuildingKind::FlakTurret, flak, true));
        let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
        operation.artillery.clear();
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        plan.screen.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let request = LiftSupportRequest {
            player: PlayerId(1),
            target: TARGET,
            planned_drops: vec![drop],
        };
        let mut intel = knowledge(&battle);
        assert_eq!(
            targetable_corridor_flak(&intel, HOME, TARGET, &[]),
            None,
            "the flak does not cover the bomber objective's direct corridor"
        );
        assert_eq!(
            targetable_corridor_flak(&intel, HOME, TARGET, &[drop]),
            Some(BuildingId(81)),
            "the same flak does cover the transport's actual drop envelope"
        );

        let suppression = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination(Some(&request)),
        );
        assert_eq!(
            planner.air_operation().unwrap().phase,
            AirOperationPhase::SuppressAa
        );
        assert!(suppression.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(81)),
        }));
        let scout_goal = planner.air_operation().unwrap().scout_dispatch.unwrap().1;
        assert!(suppression.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(1)],
            goal: scout_goal,
        }));
        assert!(suppression.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));

        battle.my_units[0].tile = scout_goal;
        battle.my_units[0].idle = false;
        battle.tick += 1;
        battle
            .enemy_buildings
            .retain(|building| building.id != BuildingId(81));
        intel.update(&battle);
        let searching = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination(Some(&request)),
        );
        assert_eq!(
            planner.air_operation().unwrap().phase,
            AirOperationPhase::SuppressAa,
            "destroying flak in fog is not enough to release the transports"
        );
        assert_eq!(
            planner.air_operation().unwrap().scout_dispatch,
            Some((UnitId(1), scout_goal)),
            "the original scouting order remains authoritative while the drop is dark"
        );
        assert!(searching.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));

        see_approach_to(&mut battle, drop);
        let flak_index = usize::try_from(flak.y * battle.map_width + flak.x).unwrap();
        battle.visible[flak_index] = true;
        battle.explored[flak_index] = true;
        battle.tick += 1;
        intel.update(&battle);
        planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination(Some(&request)),
        );
        assert_eq!(
            planner.air_operation().unwrap().phase,
            AirOperationPhase::Verify,
            "current sight must clear both the objective and frozen drop approach"
        );

        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        battle.tick = battle
            .tick
            .saturating_add(tuning.reaction_delay)
            .saturating_add(tuning.commitment_hesitation);
        intel.update(&battle);
        let released = planner.think_with_lift_support(
            &profile(),
            tuning,
            &battle,
            &intel,
            HOME,
            coordination(Some(&request)),
        );
        assert_eq!(
            planner.air_operation().unwrap().phase,
            AirOperationPhase::Strike
        );
        assert!(released.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(80)),
        }));
    }

    #[test]
    fn island_bombers_can_strike_without_a_screen_or_transport() {
        let mut battle = wealthy_island_obs(5_000, 1);
        see_approach(&mut battle);
        assert!(
            battle
                .my_units
                .iter()
                .all(|unit| unit.kind != UnitKind::Skyhook)
        );
        let mut operation = operation(AirOperationPhase::Strike, battle.tick);
        operation.artillery.clear();
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        plan.screen.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let intel = knowledge(&battle);

        let decision = think(&mut planner, &battle, &intel);

        assert!(decision.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(80)),
        }));
    }

    #[test]
    fn an_active_bomber_strike_is_not_reissued_but_an_idle_member_rejoins() {
        let mut battle = wealthy_island_obs(5_000, 1);
        see_approach(&mut battle);
        let mut operation = operation(AirOperationPhase::Strike, battle.tick);
        operation.artillery.clear();
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        plan.screen.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };

        let first = think(&mut planner, &battle, &knowledge(&battle));
        assert!(first.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(80)),
        }));

        for bomber in battle
            .my_units
            .iter_mut()
            .filter(|unit| matches!(unit.id, UnitId(3) | UnitId(4)))
        {
            bomber.idle = false;
        }
        battle.tick += 6;
        let in_flight = think(&mut planner, &battle, &knowledge(&battle));
        assert!(
            in_flight.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits {
                    target: Target::Building(BuildingId(80)),
                    ..
                }
            )),
            "the authoritative in-flight attack must not be restaged every think"
        );

        battle
            .my_units
            .iter_mut()
            .find(|unit| unit.id == UnitId(3))
            .unwrap()
            .idle = true;
        battle.tick += 6;
        let retry = think(&mut planner, &battle, &knowledge(&battle));
        assert!(retry.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3)],
            target: Target::Building(BuildingId(80)),
        }));
    }

    #[test]
    fn a_strike_keeps_its_exact_attack_until_current_sight_loses_the_target() {
        let mut battle = wealthy_island_obs(5_000, 1);
        see_approach(&mut battle);
        let mut operation = operation(AirOperationPhase::Strike, battle.tick);
        operation.artillery.clear();
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        plan.screen.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let mut intel = knowledge(&battle);

        let first = think(&mut planner, &battle, &intel);
        assert!(first.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(80)),
        }));
        assert!(
            first
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::AttackMoveUnits { .. }))
        );

        battle.tick += 1;
        intel.update(&battle);
        let still_current = think(&mut planner, &battle, &intel);
        assert!(still_current.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(80)),
        }));
        assert!(
            still_current
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::AttackMoveUnits { .. }))
        );

        battle.enemy_buildings.clear();
        battle.tick += 1;
        intel.update(&battle);
        let target_lost = think(&mut planner, &battle, &intel);
        assert!(target_lost.intents.contains(&Intent::AttackMoveUnits {
            units: vec![UnitId(3), UnitId(4)],
            goal: TARGET,
        }));
        assert!(target_lost.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));
    }

    #[test]
    fn an_opportunistic_strike_does_not_move_the_shared_coordination_anchor() {
        let mut battle = wealthy_island_obs(5_000, 1);
        see_approach(&mut battle);
        battle.enemy_buildings = vec![building(
            81,
            1,
            BuildingKind::Airworks,
            TARGET.offset(-3, 0),
            true,
        )];
        let mut operation = operation(AirOperationPhase::Strike, battle.tick);
        operation.artillery.clear();
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        plan.screen.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let request = LiftSupportRequest {
            player: PlayerId(1),
            target: TARGET,
            planned_drops: Vec::new(),
        };
        let intel = knowledge(&battle);

        let decision = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination(Some(&request)),
        );

        assert!(decision.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(81)),
        }));
        let operation = planner.air_operation().unwrap();
        assert_eq!(operation.target_player, request.player);
        assert_eq!(operation.target, request.target);
        assert_eq!(operation.target_id, Some(BuildingId(80)));
        assert_eq!(operation.target_kind, BuildingKind::Crucible);
    }

    #[test]
    fn an_exact_strike_validates_the_selected_cluster_target_not_the_operation_anchor() {
        let mut battle = obs(5_000);
        see_approach(&mut battle);
        let secondary = TARGET.offset(3, 0);
        see_approach_to(&mut battle, secondary);
        see_building_footprint(&mut battle, secondary, BuildingKind::Airworks);
        battle.explored.fill(true);
        battle.enemy_buildings = vec![building(81, 1, BuildingKind::Airworks, secondary, true)];
        let public_map = public_map_with_terrain(
            &battle,
            (0..battle.map_height).map(|y| (TilePos::new(26, y), Terrain::Peak)),
        );
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Strike, battle.tick);
        let active = planner.air.as_mut().expect("active operation");
        active
            .plan
            .connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = vec![TARGET, secondary];
        assert_eq!(
            live_strike_target(&active.op, &active.plan, &intel).map(|target| target.anchor),
            Some(secondary)
        );
        let mut coordination = coordination(None);
        coordination.public_map = Some(&public_map);

        let decision = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination,
        );

        let operation = planner
            .air_operation()
            .expect("recovery remains observable");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(81)),
                ..
            }
        )));
    }

    #[test]
    fn exact_attacks_do_not_require_attack_move_spread_slots() {
        let mut battle = obs(5_000);
        battle.my_units = (0..4)
            .map(|index| {
                own(
                    10 + index,
                    UnitKind::Condor,
                    TilePos::new(4 + i32::try_from(index).unwrap(), TARGET.y),
                )
            })
            .collect();
        let public_map = public_map_with_terrain(
            &battle,
            [TARGET.y - 1, TARGET.y + 1].into_iter().flat_map(|y| {
                (0..battle.map_width).map(move |x| (TilePos::new(x, y), Terrain::Peak))
            }),
        );
        let attackers: Vec<_> = battle.my_units.iter().map(|unit| unit.id).collect();
        let mut exact = route_projection_with_orientation(
            &battle,
            Domain::Air,
            Some(&public_map),
            test_orientation(),
        );
        assert!(exact_attack_group_reaches(
            &mut exact, &battle, &attackers, TARGET
        ));

        let mut spread = route_projection_with_orientation(
            &battle,
            Domain::Air,
            Some(&public_map),
            test_orientation(),
        );
        assert!(!spread.group_reaches_command_goal(&attackers, TARGET));
    }

    #[test]
    fn surviving_screen_cannot_hide_the_loss_of_an_airborne_bomber_force() {
        let mut battle = wealthy_island_obs(5_000, 2);
        battle
            .my_units
            .retain(|unit| !matches!(unit.id, UnitId(3) | UnitId(4)));
        battle
            .my_units
            .push(own(30, UnitKind::Buzzard, TilePos::new(5, 8)));
        battle
            .my_units
            .push(own(31, UnitKind::Buzzard, TilePos::new(5, 12)));
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.screen = vec![UnitId(30), UnitId(31)];
        let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
        operation.artillery.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let intel = knowledge(&battle);

        let decision = think(&mut planner, &battle, &intel);

        let operation = planner.air_operation().expect("recovery remains visible");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::RequiredUnitLost)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { units, .. } | Intent::AttackMoveUnits { units, .. }
                if units.contains(&UnitId(30)) || units.contains(&UnitId(31))
        )));
    }

    #[test]
    fn an_air_identity_waits_for_shared_legal_producers_before_committing() {
        let mut observation = obs(120);
        observation.my_units.retain(|unit| unit.id == UnitId(1));
        observation
            .my_units
            .extend((100..=111).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        assert_eq!(
            combat_roster(&observation),
            CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER
        );
        let intel = knowledge(&observation);
        let mut planner = StrategicPlanner::new();

        planner.think(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            &[],
        );
        assert!(planner.air_operation().is_none());

        observation.visible.fill(true);
        observation.explored.fill(true);
        observation.scrap = 10_000;
        observation.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
            building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
        ];
        observation.my_queues = vec![Vec::new(); 3];
        let intel = knowledge(&observation);
        planner.think(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            &[],
        );
        assert!(planner.air_operation().is_some());
    }

    #[test]
    fn stale_intelligence_preserves_standby_without_issuing_commands() {
        let observation = obs(101);
        let stale_intel = knowledge(&obs(100));
        let mut planner = StrategicPlanner {
            air: None,
            standby: AirStandby {
                scout: Some(UnitId(1)),
                artillery: vec![UnitId(2)],
                strike_aircraft: vec![UnitId(3), UnitId(4)],
            },
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let before = planner.clone();

        let decision = think(&mut planner, &observation, &stale_intel);

        assert!(decision.intents.is_empty());
        assert_eq!(
            decision.reservations,
            [UnitId(1), UnitId(2), UnitId(3), UnitId(4)]
        );
        assert_eq!(planner, before, "stale knowledge must not advance a plan");
    }

    #[test]
    fn airborne_assembly_fails_closed_when_known_peaks_seal_scout_ingress() {
        let mut battle = wealthy_island_obs(5_000, 1);
        battle.my_units[0].tile = TilePos::new(4, 10);
        battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        let mut operation = operation(AirOperationPhase::Assemble, battle.tick);
        operation.artillery.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let intel = knowledge(&battle);

        let decision = think(&mut planner, &battle, &intel);

        let operation = planner
            .air_operation()
            .expect("recovery remains observable");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn a_new_airborne_operation_releases_its_roster_when_scout_ingress_is_sealed() {
        let mut battle = wealthy_island_obs(5_016, 1);
        battle.my_units[0].tile = HOME;
        battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
        let mut intel = knowledge(&battle);
        let mut planner = StrategicPlanner::new();

        let refused = think(&mut planner, &battle, &intel);

        let operation = planner
            .air_operation()
            .expect("the failed ingress remains observable through recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(refused.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));

        battle.tick += 1;
        intel.update(&battle);
        think(&mut planner, &battle, &intel);
        assert!(planner.air_operation().is_none());

        battle.tick += 1;
        intel.update(&battle);
        let cooldown = think(&mut planner, &battle, &intel);
        assert!(planner.air_operation().is_none());
        assert!(cooldown.reservations.is_empty());
        assert!(cooldown.intents.is_empty());
    }

    #[test]
    fn airborne_assembly_fills_an_undispatched_scout_slot_then_launches_the_complete_wave() {
        let mut battle = wealthy_island_obs(5_000, 1);
        battle.my_units[0].tile = HOME;
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        let mut operation = operation(AirOperationPhase::Assemble, battle.tick);
        operation.scout = Some(UnitId(99));
        operation.scout_dispatch = None;
        operation.artillery.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let intel = knowledge(&battle);

        let decision = think(&mut planner, &battle, &intel);

        let operation = planner.air_operation().expect("operation continues");
        assert_eq!(operation.phase, AirOperationPhase::SuppressAa);
        assert_eq!(operation.scout, Some(UnitId(1)));
        assert!(decision.intents.iter().any(|intent| matches!(
            intent,
            Intent::MoveUnits { units, goal }
                if units == &[UnitId(1)] && *goal != TARGET
        )));
        assert!(decision.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(3), UnitId(4)],
            goal: landing_pad(&battle, HOME).unwrap(),
        }));
        assert_eq!(
            planner.remaining_airwork_ticks(&battle),
            0,
            "a launched operation no longer reserves speculative Airworks capacity"
        );
    }

    #[test]
    fn losing_a_dispatched_scout_during_assembly_recovers_without_replacement() {
        let mut battle = wealthy_island_obs(5_000, 1);
        battle.my_units[0].tile = HOME;
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        let mut operation = operation(AirOperationPhase::Assemble, battle.tick);
        operation.scout = Some(UnitId(99));
        operation.scout_dispatch = Some((UnitId(99), TARGET));
        operation.artillery.clear();
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let intel = knowledge(&battle);

        let decision = think(&mut planner, &battle, &intel);

        let operation = planner
            .air_operation()
            .expect("the failed assembly remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::RequiredUnitLost)
        );
        assert_eq!(operation.scout_dispatch, None);
        assert!(planner.cooldown_until() > battle.tick);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::MoveUnits { units, .. } if units.contains(&UnitId(1))
        )));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Kestrel,
                ..
            }
        )));
    }

    #[test]
    fn losing_the_scout_after_assembly_aborts_before_any_bomber_commitment() {
        let mut battle = obs(300);
        battle.my_units.retain(|unit| unit.id != UnitId(1));
        let intel = knowledge(&battle);

        for phase in [
            AirOperationPhase::SuppressAa,
            AirOperationPhase::Verify,
            AirOperationPhase::Strike,
        ] {
            let mut planner = with_operation(phase, battle.tick);
            let decision = think(&mut planner, &battle, &intel);
            let operation = planner
                .air_operation()
                .expect("the failed operation remains observable during recovery");

            assert_eq!(operation.phase, AirOperationPhase::Recover);
            assert_eq!(
                operation.recovery_reason,
                Some(AirRecoveryReason::RequiredUnitLost)
            );
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
            )));
        }
    }

    #[test]
    fn visible_ground_mobile_aa_is_suppressed_before_bombers_commit() {
        let mut battle = obs(300);
        let mut flakhound = own(90, UnitKind::Flakhound, TARGET.offset(-3, 0));
        flakhound.player = PlayerId(1);
        battle.enemy_units.push(flakhound);
        let intel = knowledge(&battle);

        for phase in [AirOperationPhase::SuppressAa, AirOperationPhase::Verify] {
            let mut planner = with_operation(phase, battle.tick);
            let decision = think(&mut planner, &battle, &intel);
            let operation = planner
                .air_operation()
                .expect("suppression retains the operation");

            assert_eq!(operation.phase, AirOperationPhase::SuppressAa);
            assert_eq!(operation.recovery_reason, None);
            let firing_stand = decision
                .intents
                .iter()
                .find_map(|intent| match intent {
                    Intent::MoveUnits { units, goal } if units == &[UnitId(2)] => Some(*goal),
                    _ => None,
                })
                .expect("artillery positions before attacking mobile AA");
            let mut routes = route_projection(&battle, Domain::Ground, None);
            assert!(
                suppression_firing_stands(
                    &mut routes,
                    &battle,
                    SuppressionOrigin {
                        tile: battle.my_units[1].tile,
                        kind: UnitKind::Bombard,
                    },
                    Target::Unit(UnitId(90)),
                    &intel,
                    None,
                )
                .any(|stand| stand == firing_stand)
            );
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent, Intent::AttackUnits { units, .. } if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
            )));
        }
    }

    #[test]
    fn visible_landed_air_aa_is_suppressed_as_a_ground_target() {
        let mut battle = obs(300);
        let mut talon = own(90, UnitKind::Talon, TARGET.offset(-3, 0));
        talon.player = PlayerId(1);
        talon.grounded = true;
        battle.enemy_units.push(talon);
        let intel = knowledge(&battle);

        for phase in [AirOperationPhase::SuppressAa, AirOperationPhase::Verify] {
            let mut planner = with_operation(phase, battle.tick);
            let decision = think(&mut planner, &battle, &intel);
            let operation = planner
                .air_operation()
                .expect("suppression retains the operation");

            assert_eq!(operation.phase, AirOperationPhase::SuppressAa);
            assert_eq!(operation.recovery_reason, None);
            let firing_stand = decision
                .intents
                .iter()
                .find_map(|intent| match intent {
                    Intent::MoveUnits { units, goal } if units == &[UnitId(2)] => Some(*goal),
                    _ => None,
                })
                .expect("artillery positions before attacking landed AA");
            let mut routes = route_projection(&battle, Domain::Ground, None);
            assert!(
                suppression_firing_stands(
                    &mut routes,
                    &battle,
                    SuppressionOrigin {
                        tile: battle.my_units[1].tile,
                        kind: UnitKind::Bombard,
                    },
                    Target::Unit(UnitId(90)),
                    &intel,
                    None,
                )
                .any(|stand| stand == firing_stand)
            );
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent, Intent::AttackUnits { units, .. } if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
            )));
        }
    }

    #[test]
    fn connected_operation_recovers_from_airborne_aa_that_artillery_cannot_suppress() {
        let mut battle = obs(300);
        let mut talon = own(90, UnitKind::Talon, TARGET.offset(-3, 0));
        talon.player = PlayerId(1);
        battle.enemy_units.push(talon);
        let intel = knowledge(&battle);

        for phase in [
            AirOperationPhase::SuppressAa,
            AirOperationPhase::Verify,
            AirOperationPhase::Strike,
        ] {
            let mut planner = with_operation(phase, battle.tick);
            let decision = think(&mut planner, &battle, &intel);
            let operation = planner
                .air_operation()
                .expect("recovery remains observable");

            assert_eq!(operation.phase, AirOperationPhase::Recover, "{phase:?}");
            assert_eq!(
                operation.recovery_reason,
                Some(AirRecoveryReason::NewAirDefense),
                "{phase:?}"
            );
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
            )));
        }
    }

    #[test]
    fn connected_phases_suppress_aa_covering_a_secondary_cluster_target() {
        let primary = TilePos::new(20, 20);
        let secondary = TilePos::new(24, 20);
        let flak = TilePos::new(29, 20);
        let mut battle = obs(400);
        battle.map_width = 40;
        battle.map_height = 30;
        battle.visible = vec![true; 40 * 30];
        battle.explored = vec![true; 40 * 30];
        battle.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, primary, true),
            building(82, 1, BuildingKind::Airworks, secondary, true),
            building(81, 1, BuildingKind::FlakTurret, flak, true),
        ];
        let intel = knowledge(&battle);
        assert_eq!(targetable_flak(&intel.air_defense_at(primary)), None);
        assert_eq!(
            targetable_flak(&intel.air_defense_at(secondary)),
            Some(BuildingId(81))
        );

        for phase in [
            AirOperationPhase::SuppressAa,
            AirOperationPhase::Verify,
            AirOperationPhase::Strike,
        ] {
            let mut planner = with_operation(phase, battle.tick);
            let active = planner.air.as_mut().expect("active operation");
            active.op.target = primary;
            active.op.target_id = Some(BuildingId(80));
            active
                .plan
                .connected_package
                .as_mut()
                .expect("connected package")
                .target_anchors = vec![primary, secondary];
            let decision = think(&mut planner, &battle, &intel);
            let operation = planner
                .air_operation()
                .expect("suppression retains the operation");

            assert_eq!(operation.phase, AirOperationPhase::SuppressAa, "{phase:?}");
            assert_eq!(operation.recovery_reason, None, "{phase:?}");
            let firing_stand = decision
                .intents
                .iter()
                .find_map(|intent| match intent {
                    Intent::MoveUnits { units, goal } if units == &[UnitId(2)] => Some(*goal),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{phase:?}: {decision:?}"));
            let mut routes = route_projection(&battle, Domain::Ground, None);
            assert!(
                suppression_firing_stands(
                    &mut routes,
                    &battle,
                    SuppressionOrigin {
                        tile: battle.my_units[1].tile,
                        kind: UnitKind::Bombard,
                    },
                    Target::Building(BuildingId(81)),
                    &intel,
                    None,
                )
                .any(|stand| stand == firing_stand)
            );
            assert!(
                decision.intents.iter().all(|intent| !matches!(
                    intent,
                    Intent::AttackUnits {
                        units,
                        target: Target::Building(BuildingId(80) | BuildingId(82)),
                    } if units.contains(&UnitId(3)) || units.contains(&UnitId(4))
                )),
                "{phase:?}: {decision:?}"
            );
        }
    }

    #[test]
    fn connected_selection_excludes_secondary_aa_sealed_by_peaks() {
        let primary = TilePos::new(20, 20);
        let secondary = TilePos::new(24, 20);
        let flak = TilePos::new(29, 20);
        let mut battle = obs(400);
        battle.map_width = 40;
        battle.map_height = 30;
        battle.visible = vec![true; 40 * 30];
        battle.explored = vec![true; 40 * 30];
        battle.enemy_buildings = vec![
            building(80, 1, BuildingKind::Crucible, primary, true),
            building(82, 1, BuildingKind::Airworks, secondary, true),
            building(81, 1, BuildingKind::FlakTurret, flak, true),
        ];
        let public_map = public_map_with_terrain(
            &battle,
            crate::tick::rect_adjacent_tiles(flak, BuildingKind::FlakTurret.base_stats().size)
                .map(|tile| (tile, Terrain::Peak)),
        );
        let intel = knowledge(&battle);
        let target = intel
            .buildings()
            .iter()
            .find(|contact| contact.anchor == primary)
            .expect("current primary target");
        let selection = connected_target_selection(
            &battle,
            target,
            &[],
            ConnectedRouteContext {
                intel: &intel,
                home: HOME,
                target: primary,
                public_map: Some(&public_map),
                orientation: test_orientation(),
            },
        );
        assert_eq!(selection.target_anchors, vec![primary]);
        let mut planner = with_operation(AirOperationPhase::SuppressAa, battle.tick);
        let active = planner.air.as_mut().expect("active operation");
        active.op.target = primary;
        active.op.target_id = Some(BuildingId(80));
        active
            .plan
            .connected_package
            .as_mut()
            .expect("connected package")
            .target_anchors = selection.target_anchors;
        let mut coordination = coordination(None);
        coordination.public_map = Some(&public_map);

        let decision = planner.think_with_lift_support(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &battle,
            &intel,
            HOME,
            coordination,
        );

        let operation = planner.air_operation().expect("operation remains active");
        assert_ne!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(operation.recovery_reason, None);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(81)),
                ..
            }
        )));
    }

    #[test]
    fn suppression_cancels_its_attack_when_scout_ingress_is_impossible() {
        let mut battle = obs(300);
        battle.my_units[0].tile = TilePos::new(4, 10);
        battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
        battle.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TARGET.offset(-4, 0),
            true,
        ));
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::SuppressAa, battle.tick);

        let decision = think(&mut planner, &battle, &intel);

        let operation = planner
            .air_operation()
            .expect("recovery remains observable");
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(
            decision
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::AttackUnits { .. })),
            "artillery must not fire after the spotter route is disproved"
        );
    }

    #[test]
    fn suppression_without_flak_still_fails_closed_when_scout_ingress_is_sealed() {
        for target_currently_visible in [false, true] {
            let mut battle = obs(300);
            battle.my_units[0].tile = TilePos::new(4, 10);
            battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
            if target_currently_visible {
                see_approach(&mut battle);
            }
            let intel = knowledge(&battle);
            let mut planner = with_operation(AirOperationPhase::SuppressAa, battle.tick);

            let decision = think(&mut planner, &battle, &intel);
            let operation = planner
                .air_operation()
                .expect("the failed operation remains observable during recovery");

            assert_eq!(operation.phase, AirOperationPhase::Recover);
            assert_eq!(
                operation.recovery_reason,
                Some(AirRecoveryReason::UnreachableAirRoute)
            );
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
            )));
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::MoveUnits { units, goal }
                    if units.iter().any(|id| matches!(id, UnitId(3) | UnitId(4)))
                        && *goal != HOME
            )));
        }
    }

    #[test]
    fn airborne_suppression_aborts_from_clear_and_uncertain_corridors_when_recon_is_sealed() {
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_045,
        ));
        assert_eq!(identity.primary, Specialty::Air);

        for approach_visible in [false, true] {
            let mut battle = wealthy_island_obs(5_000, 1);
            battle.my_units[0].tile = HOME;
            battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
            if approach_visible {
                see_approach(&mut battle);
            }
            let intel = knowledge(&battle);
            let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
            operation.artillery.clear();
            let mut plan = AirPlan::island(&identity, &battle);
            plan.desired_strike_aircraft = 2;
            plan.desired_screen = 0;
            plan.screen.clear();
            let mut planner = StrategicPlanner {
                air: Some(ActiveAirOperation {
                    op: operation,
                    plan,
                }),
                standby: AirStandby::default(),
                cooldown_until: 0,
                terminal_outcome: None,
            };

            let decision = planner.think(
                &identity,
                DifficultyTuning::for_level(BotDifficulty::Prime),
                &battle,
                &intel,
                HOME,
                &[],
            );
            let operation = planner
                .air_operation()
                .expect("the failed suppression remains observable during recovery");

            assert_eq!(operation.phase, AirOperationPhase::Recover);
            assert_eq!(
                operation.recovery_reason,
                Some(AirRecoveryReason::UnreachableAirRoute),
                "approach_visible={approach_visible}"
            );
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
            )));
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::MoveUnits { units, goal }
                    if units.iter().any(|id| matches!(id, UnitId(3) | UnitId(4)))
                        && *goal != HOME
            )));
        }
    }

    #[test]
    fn airborne_verification_never_commits_while_its_recon_route_is_sealed() {
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Standard,
            BotStance::Balanced,
            20_045,
        ));
        assert_eq!(identity.primary, Specialty::Air);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Standard);

        for approach_visible in [false, true] {
            let mut battle = wealthy_island_obs(5_000, 1);
            battle.my_units[0].tile = HOME;
            battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
            if approach_visible {
                see_approach(&mut battle);
            }
            let intel = knowledge(&battle);
            let mut operation = operation(AirOperationPhase::Verify, battle.tick);
            operation.artillery.clear();
            let mut plan = AirPlan::island(&identity, &battle);
            plan.desired_strike_aircraft = 2;
            plan.desired_screen = 0;
            plan.screen.clear();
            let mut planner = StrategicPlanner {
                air: Some(ActiveAirOperation {
                    op: operation,
                    plan,
                }),
                standby: AirStandby::default(),
                cooldown_until: 0,
                terminal_outcome: None,
            };

            let decision = planner.think(&identity, tuning, &battle, &intel, HOME, &[]);
            let operation = planner
                .air_operation()
                .expect("the refused verification remains observable during recovery");

            assert_eq!(operation.phase, AirOperationPhase::Recover);
            assert_eq!(
                operation.recovery_reason,
                Some(AirRecoveryReason::UnreachableAirRoute),
                "approach_visible={approach_visible}"
            );
            assert_eq!(operation.strike_issued_at, None);
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
            )));
        }
    }

    #[test]
    fn suppression_observes_reaction_delay_before_firing_on_new_flak() {
        let mut battle = obs(300);
        battle.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TARGET.offset(-4, 0),
            true,
        ));
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::SuppressAa, battle.tick);
        planner.air_op_mut().unwrap().phase_started_at = battle.tick;

        let decision = planner.think(
            &profile(),
            DifficultyTuning::for_level(BotDifficulty::Standard),
            &battle,
            &intel,
            HOME,
            &[],
        );

        assert_eq!(
            planner.air_operation().expect("operation waits").phase,
            AirOperationPhase::SuppressAa
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(81)),
                ..
            }
        )));
    }

    #[test]
    fn verification_reenters_suppression_when_flak_returns() {
        let mut battle = obs(300);
        battle.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TARGET.offset(-4, 0),
            true,
        ));
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Verify, battle.tick);

        let decision = think(&mut planner, &battle, &intel);

        assert_eq!(
            planner.air_operation().expect("operation continues").phase,
            AirOperationPhase::SuppressAa
        );
        assert!(decision.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(2)],
            target: Target::Building(BuildingId(81)),
        }));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));
    }

    #[test]
    fn verification_clears_held_orders_when_scout_ingress_becomes_impossible() {
        let seen = obs(299);
        let mut intel = knowledge(&seen);
        let mut battle = obs(300);
        battle.my_units[0].tile = TilePos::new(4, 10);
        battle.known_peaks = (0..battle.map_height).map(|y| TilePos::new(8, y)).collect();
        battle.enemy_buildings[0].seen = false;
        intel.update(&battle);
        let mut planner = with_operation(AirOperationPhase::Verify, battle.tick);

        let decision = think(&mut planner, &battle, &intel);

        let operation = planner
            .air_operation()
            .expect("recovery remains observable");
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableAirRoute)
        );
        assert!(
            decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::MoveUnits { units, goal }
                    if units.iter().any(|id| matches!(id, UnitId(3) | UnitId(4)))
                        && *goal != HOME
            )),
            "a failed reconnaissance route must not leave a strike hold behind"
        );
    }

    #[test]
    fn an_airborne_strike_returns_to_suppression_when_corridor_flak_appears() {
        let mut battle = wealthy_island_obs(5_000, 1);
        see_approach(&mut battle);
        battle.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::FlakTurret,
            TilePos::new(12, 10),
            true,
        ));
        let mut operation = operation(AirOperationPhase::Strike, battle.tick);
        operation.artillery.clear();
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };
        let intel = knowledge(&battle);

        let decision = think(&mut planner, &battle, &intel);

        assert_eq!(
            planner.air_operation().expect("operation continues").phase,
            AirOperationPhase::SuppressAa
        );
        assert!(decision.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3), UnitId(4)],
            target: Target::Building(BuildingId(81)),
        }));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits {
                target: Target::Building(BuildingId(80)),
                ..
            }
        )));
    }

    #[test]
    fn destroyed_target_completes_the_strike_and_releases_a_waiting_lift() {
        let mut battle = wealthy_island_obs(5_000, 1);
        battle.enemy_buildings.clear();
        see_approach(&mut battle);
        let mut operation = operation(AirOperationPhase::Strike, battle.tick);
        operation.artillery.clear();
        operation.strike_issued_at = Some(battle.tick - 20);
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_strike_aircraft = 2;
        plan.desired_screen = 0;
        let mut planner = StrategicPlanner {
            air: Some(ActiveAirOperation {
                op: operation,
                plan,
            }),
            standby: AirStandby::default(),
            cooldown_until: 0,
            terminal_outcome: None,
        };

        let completion = think(&mut planner, &battle, &knowledge(&battle));
        let operation = planner
            .air_operation()
            .expect("the survivors receive one return order");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(operation.recovery_reason, Some(AirRecoveryReason::Complete));
        assert!(completion.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));

        battle.tick += 1;
        battle.my_units.clear();
        let released = think(&mut planner, &battle, &knowledge(&battle));
        assert!(released.reservations.is_empty());
        assert!(planner.air_operation().is_none());
        assert_eq!(
            planner.terminal_outcome(),
            Some(AirOperationOutcome::Released {
                player: PlayerId(1),
                target: TARGET,
            })
        );

        battle.tick += 1;
        think(&mut planner, &battle, &knowledge(&battle));
        assert_eq!(
            planner.terminal_outcome(),
            None,
            "the handoff signal is emitted for exactly one think"
        );
    }

    #[test]
    fn a_visible_missing_objective_aborts_before_the_bombers_commit() {
        let mut battle = obs(400);
        battle.enemy_buildings.clear();
        see_approach(&mut battle);
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Verify, battle.tick);

        let decision = think(&mut planner, &battle, &intel);

        let operation = planner
            .air_operation()
            .expect("recovery remains observable");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::ObjectiveLost)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn a_strike_aborts_when_its_previously_viable_staging_area_is_severed() {
        let mut battle = obs(400);
        let ideal = staging(HOME, TARGET);
        battle.known_rock = (0..battle.map_height)
            .flat_map(|y| (ideal.x - 3..=ideal.x + 3).map(move |x| TilePos::new(x, y)))
            .collect();
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Strike, battle.tick);

        let decision = think(&mut planner, &battle, &intel);

        let operation = planner
            .air_operation()
            .expect("recovery remains observable");
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::UnreachableStaging)
        );
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));
    }

    #[test]
    fn a_siege_identity_trains_available_avalanches_for_its_operation() {
        let mut observation = obs(300);
        observation.visible.fill(true);
        observation.explored.fill(true);
        observation.my_units.retain(|unit| unit.id != UnitId(2));
        observation.my_buildings = vec![building(
            30,
            0,
            BuildingKind::Crucible,
            TilePos::new(10, 15),
            true,
        )];
        observation.my_queues = vec![Vec::new()];
        observation.scrap = UnitKind::Avalanche.stats().cost;
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            20_043,
        ));
        let mut plan = derived_connected_test_plan(&identity, &observation)
            .expect("the completed Crucible can supply the package");
        let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
        operation.artillery.clear();
        let mut decision = StrategicDecision::default();
        let intelligence = knowledge(&observation);

        schedule_missing_members(
            &operation,
            &mut plan,
            &planning_context(&identity, &observation, &intelligence),
            UnitKind::Kestrel,
            &mut decision,
        );

        assert!(decision.intents.contains(&Intent::TrainAt {
            building: BuildingId(30),
            kind: UnitKind::Avalanche,
        }));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Bombard,
                ..
            }
        )));
        assert_eq!(plan.desired_artillery, 1);
    }

    #[test]
    fn each_developed_economy_path_can_fund_an_island_operation() {
        let mut observation = wealthy_island_obs(5_000, 1);
        observation
            .my_buildings
            .retain(|building| building.kind != BuildingKind::Reclaimer);
        observation.scrap = 0;
        observation.my_buildings.push(building(
            31,
            0,
            BuildingKind::Foundry,
            TilePos::new(8, 2),
            true,
        ));
        let target = knowledge(&observation).buildings()[0].clone();

        assert!(wealthy_island_target(
            &profile(),
            &observation,
            HOME,
            &target,
            None,
        ));

        observation
            .my_buildings
            .retain(|building| building.id != BuildingId(31));
        let bomber_bank = UnitKind::Condor.stats().cost.saturating_mul(4);
        observation.scrap = bomber_bank;
        assert!(wealthy_island_target(
            &profile(),
            &observation,
            HOME,
            &target,
            None,
        ));

        observation.scrap = bomber_bank - 1;
        assert!(
            !wealthy_island_target(&profile(), &observation, HOME, &target, None),
            "one Foundry, no renewable income, and an underfunded bank is not a mature economy"
        );
    }

    #[test]
    fn every_difficulty_uses_its_exact_tactical_memory_boundary_monotonically() {
        let seen = obs(100);
        let mut intel = knowledge(&seen);
        let mut hidden = obs(101);
        hidden.enemy_buildings[0].seen = false;
        intel.update(&hidden);

        for difficulty in BotDifficulty::ALL {
            let memory = DifficultyTuning::for_level(difficulty).tactical_memory;
            let remembered = select_target(&intel, 100 + memory, memory)
                .expect("evidence remains actionable through the authored boundary");
            assert_eq!(remembered.id, Some(BuildingId(80)), "{difficulty:?}");
            assert_eq!(
                remembered.evidence,
                ContactEvidence::Remembered,
                "{difficulty:?}"
            );
            assert!(
                select_target(&intel, 100 + memory + 1, memory).is_none(),
                "{difficulty:?} retained evidence beyond its tactical-memory limit"
            );
        }

        for pair in BotDifficulty::ALL.windows(2) {
            let lower = DifficultyTuning::for_level(pair[0]);
            let higher = DifficultyTuning::for_level(pair[1]);
            let now = 100 + lower.tactical_memory + 1;
            assert!(
                select_target(&intel, now, lower.tactical_memory).is_none(),
                "{:?} should have forgotten at the adjacent-rung probe",
                pair[0]
            );
            assert!(
                select_target(&intel, now, higher.tactical_memory).is_some(),
                "{:?} should retain what {:?} has just forgotten",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn current_target_outranks_a_more_valuable_remembered_target() {
        let seen = obs(100);
        let mut intel = knowledge(&seen);
        let mut later = obs(400);
        later.enemy_buildings[0].seen = false;
        later.enemy_buildings.push(building(
            81,
            1,
            BuildingKind::Reclaimer,
            TilePos::new(18, 5),
            true,
        ));
        intel.update(&later);

        for difficulty in BotDifficulty::ALL {
            let tuning = DifficultyTuning::for_level(difficulty);
            let selected = select_target(&intel, later.tick, tuning.tactical_memory)
                .expect("a current economic target is actionable");
            assert_eq!(selected.id, Some(BuildingId(81)), "{difficulty:?}");
            assert_eq!(selected.evidence, ContactEvidence::Current);
        }
    }

    #[test]
    fn strategic_air_targeting_spends_its_attention_on_the_highest_value_infrastructure() {
        let mut seen = obs(100);
        let priorities = [
            BuildingKind::Crucible,
            BuildingKind::Airworks,
            BuildingKind::Fabricator,
            BuildingKind::Foundry,
            BuildingKind::Extractor,
            BuildingKind::Bastion,
            BuildingKind::Reclaimer,
            BuildingKind::Turret,
        ];
        seen.enemy_buildings = priorities
            .iter()
            .copied()
            .chain([BuildingKind::FlakTurret])
            .enumerate()
            .map(|(index, kind)| {
                let offset = u32::try_from(index).expect("the fixed target fixture fits in u32");
                building(
                    80 + offset,
                    1,
                    kind,
                    TilePos::new(
                        10 + i32::try_from(index).expect("the fixed target fixture fits in i32"),
                        5,
                    ),
                    true,
                )
            })
            .collect();

        for expected in priorities {
            let intel = knowledge(&seen);
            let selected = select_target(&intel, seen.tick, u64::MAX)
                .expect("at least one strategic target remains");
            assert_eq!(selected.kind, expected);
            seen.enemy_buildings
                .retain(|building| building.kind != expected);
        }

        let intel = knowledge(&seen);
        assert!(
            select_target(&intel, seen.tick, u64::MAX).is_none(),
            "static anti-air alone is a suppression problem, not an air-operation objective"
        );
    }

    #[test]
    fn resolved_air_personalities_change_island_timing_and_wing_size_without_removing_it() {
        let low =
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 19).resolve_profile();
        let high =
            BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1).resolve_profile();
        assert_eq!(
            (low.primary, low.secondary),
            (Specialty::Fortification, Specialty::Air)
        );
        assert_eq!(
            (high.primary, high.secondary),
            (Specialty::Fortification, Specialty::Air)
        );
        assert_eq!((low.traits.air, low.traits.guile), (48, 47));
        assert_eq!((high.traits.air, high.traits.guile), (61, 37));

        let earliest = |identity: &ResolvedProfile| {
            ISLAND_OPERATION_EARLIEST_TICK
                .saturating_add(250)
                .saturating_add(u64::from(100 - identity.traits.air) * 8)
        };
        let early = wealthy_island_obs(earliest(&high), 1);
        let early_target = knowledge(&early).buildings()[0].clone();
        assert!(wealthy_island_target(
            &high,
            &early,
            HOME,
            &early_target,
            None,
        ));
        assert!(
            !wealthy_island_target(&low, &early, HOME, &early_target, None),
            "the lower-air identity keeps the operation but prepares it longer"
        );

        let later = wealthy_island_obs(earliest(&low), 1);
        let later_target = knowledge(&later).buildings()[0].clone();
        assert!(wealthy_island_target(
            &low,
            &later,
            HOME,
            &later_target,
            None,
        ));
        assert!(wealthy_island_target(
            &high,
            &later,
            HOME,
            &later_target,
            None,
        ));

        let low_plan = AirPlan::island(&low, &later);
        let high_plan = AirPlan::island(&high, &later);
        assert_eq!(
            high_plan.desired_strike_aircraft,
            low_plan.desired_strike_aircraft + 1
        );
        assert_eq!(high_plan.desired_screen, low_plan.desired_screen + 1);
    }

    #[test]
    fn stance_changes_island_timing_force_size_and_retry_cadence() {
        let personality_delay = u64::from(100u8.saturating_sub(profile().traits.air)) * 8;
        let observation = wealthy_island_obs(
            ISLAND_OPERATION_EARLIEST_TICK.saturating_add(personality_delay),
            1,
        );
        let target = knowledge(&observation).buildings()[0].clone();
        let mut aggressive = profile();
        aggressive.stance = BotStance::Aggressive;
        let mut turtle = profile();
        turtle.stance = BotStance::Turtle;

        assert!(wealthy_island_target(
            &aggressive,
            &observation,
            HOME,
            &target,
            None,
        ));
        assert!(!wealthy_island_target(
            &turtle,
            &observation,
            HOME,
            &target,
            None,
        ));

        let aggressive_plan = AirPlan::island(&aggressive, &observation);
        let turtle_plan = AirPlan::island(&turtle, &observation);
        assert_eq!(
            aggressive_plan.desired_strike_aircraft,
            turtle_plan.desired_strike_aircraft + 2
        );
        assert!(aggressive_plan.assembly_timeout > turtle_plan.assembly_timeout);

        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        assert!(cooldown(&turtle, tuning) > cooldown(&aggressive, tuning));

        let mut later = observation.clone();
        later.tick = later.tick.saturating_add(500);
        assert!(wealthy_island_target(&turtle, &later, HOME, &target, None,));
    }

    #[test]
    fn active_paid_operations_share_one_stale_target_boundary_across_difficulties() {
        let seen = obs(100);
        let mut intel = knowledge(&seen);
        let boundary = seen.tick + ACTIVE_OPERATION_TARGET_MEMORY;
        let mut hidden = obs(boundary);
        hidden.enemy_buildings = vec![building(80, 1, BuildingKind::Crucible, TARGET, false)];
        intel.update(&hidden);
        let mut cases = BotDifficulty::ALL.map(|difficulty| {
            let mut identity = profile();
            identity.difficulty = difficulty;
            (
                difficulty,
                identity,
                with_operation(AirOperationPhase::Recon, boundary),
            )
        });

        for (difficulty, identity, planner) in &mut cases {
            let retained = planner.think(
                identity,
                DifficultyTuning::for_level(*difficulty),
                &hidden,
                &intel,
                HOME,
                &[],
            );
            let operation = planner.air_operation().expect("the boundary is inclusive");
            assert_eq!(operation.phase, AirOperationPhase::Recon, "{difficulty:?}");
            assert_eq!(operation.recovery_reason, None, "{difficulty:?}");
            assert_eq!(
                retained.reservations,
                [UnitId(1), UnitId(2), UnitId(3), UnitId(4)],
                "{difficulty:?}"
            );
        }

        hidden.tick += 1;
        intel.update(&hidden);
        for (difficulty, identity, planner) in &mut cases {
            let recovered = planner.think(
                identity,
                DifficultyTuning::for_level(*difficulty),
                &hidden,
                &intel,
                HOME,
                &[],
            );
            let operation = planner
                .air_operation()
                .expect("survivors receive one recovery order before release");
            assert_eq!(
                operation.phase,
                AirOperationPhase::Recover,
                "{difficulty:?}"
            );
            assert_eq!(
                operation.recovery_reason,
                Some(AirRecoveryReason::StaleIntelligence),
                "{difficulty:?}"
            );
            assert_eq!(
                recovered.committed_scrap, 0,
                "{difficulty:?} must release its factory bank on the boundary"
            );
        }
    }

    #[test]
    fn prime_can_select_a_remembered_target_after_veteran_forgets_it() {
        let seen = obs(100);
        let mut intel = knowledge(&seen);
        let veteran = DifficultyTuning::for_level(BotDifficulty::Veteran);
        let prime = DifficultyTuning::for_level(BotDifficulty::Prime);
        let now = seen.tick + veteran.tactical_memory + 1;
        let mut hidden = obs(now);
        hidden.enemy_buildings[0].seen = false;
        intel.update(&hidden);

        assert_eq!(veteran.tactical_memory, ACTIVE_OPERATION_TARGET_MEMORY);
        assert!(select_target(&intel, now, veteran.tactical_memory).is_none());
        let selected = select_target(&intel, now, prime.tactical_memory)
            .expect("Prime's longer selection memory remains useful before commitment");
        assert_eq!(selected.id, Some(BuildingId(80)));
        assert_eq!(selected.evidence, ContactEvidence::Remembered);
    }
}
