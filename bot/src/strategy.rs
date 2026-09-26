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
use super::navigation::commands::{self as routing, RouteProjection, production_spawn_doorstep};
use super::observation::{Observation, UnitObs};
use super::orient::Orientation;
use super::profile::ResolvedProfile;
use super::resources::{
    ProducerEgress, ProducerLaneReservations, ProductionAccess, ResourceSnapshot,
    count_paid_queued_ready_with_access, paid_queued_ready_occurrences_with_access,
};
#[cfg(test)]
use crate::observation::ObservationData;
use crate::production::ProductionPlan;
use crate::query_work::QueryPurpose;
use chassis::Tick;
use chassis::fx::{Fx, HALF, Vec2Fx};
use chassis::grid::TilePos;
use core::cmp::Reverse;
use oxide_sim::ids::{BuildingId, PlayerId, Target, UnitId};
use oxide_sim::scenario::BotStance;
use oxide_sim::stats::{BuildingKind, Domain, QUEUE_CAP, Role, UnitKind, WeaponStats};
use std::collections::BTreeMap;

mod campaign_routes;
pub(super) mod force_package;
use campaign_routes::CampaignRoutes;

use force_package::{
    ConnectedForcePackage, ConnectedForcePackageOptions, ConnectedTargetEvidence, ForceFamily,
    ForcePackageRejection, NormalizedCapability, PreparationConstraints, ProductionEvidence,
    ProviderDemand, ProviderDemandTranche, building_value, current_target_cluster,
    derive_connected_force_package_options_for_cluster, refine_provider_demands, strike_capability,
    suppression_capability, target_cluster_air_defense,
};

/// A connected-map combined-arms operation is an expensive second front, not
/// an opening build order. Keep a real fighting roster online before reserving
/// scouts, artillery, and strike aircraft so a seeded specialty cannot hollow
/// out the ordinary line that protects the economy.
const CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER: usize = 12;
/// A connected operation may use only completed production that can finish its
/// whole requested package inside this immutable preparation window.
const CONNECTED_PREPARATION_HORIZON: Tick = 2_400;

#[cfg(test)]
thread_local! {
    static AIRWORKS_PACKAGE_DERIVATIONS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn airworks_package_derivations() -> usize {
    AIRWORKS_PACKAGE_DERIVATIONS.with(core::cell::Cell::get)
}

/// Shared preparation horizon used by connected proposal derivation and the
/// coordinator's joint resource projection.
pub(crate) const fn connected_preparation_horizon() -> Tick {
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
    planning: Option<&'a crate::planning::PlanningWork>,
    minimum_only: bool,
    campaign_routes: Option<&'a CampaignRoutes<'a>>,
    orientation: Orientation,
    public_map: Option<&'a PublicMapBriefing>,
    resources: &'a ConnectedProductionResources,
    preferred_artillery: &'a [UnitId],
    protected_current_scrap: u32,
    preparation: PreparationConstraints,
}

#[derive(Debug, Clone, Copy)]
struct ConnectedRouteContext<'a> {
    campaign_routes: Option<&'a CampaignRoutes<'a>>,
    unavailable_paid: &'a [(BuildingId, UnitKind, usize)],
    intel: &'a StrategicIntelligence,
    home: TilePos,
    target: TilePos,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
}

impl<'a> ConnectedRouteContext<'a> {
    fn with_navigation<T>(
        self,
        obs: &'a Observation,
        use_routes: impl FnOnce(&CampaignRoutes<'a>) -> T,
    ) -> T {
        if let Some(routes) = self.campaign_routes {
            use_routes(routes)
        } else {
            use_routes(&CampaignRoutes::new(
                obs,
                self.intel,
                self.public_map,
                self.orientation,
            ))
        }
    }

    fn staging(self, obs: &'a Observation) -> Option<TilePos> {
        self.with_navigation(obs, |routes| routes.staging(self.home, self.target))
    }
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// The kind of operation the planner is running. A remembered objective is
/// only watched until current sight admits it as an island or connected
/// assault; only an assault sizes and reserves a strike force.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum AirPlan {
    Reacquire(ReacquirePlan),
    Island(IslandPlan),
    Connected(Box<ConnectedPlan>),
}

/// Scout-only watch over a remembered objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ReacquirePlan {
    admitted_at: Tick,
    assembly_timeout: Tick,
    terrain: ReacquireTerrain,
}

/// Known ground connectivity of a remembered objective when it was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum ReacquireTerrain {
    Severed,
    Connected,
}

/// Massed-air assault on a ground-severed objective, sized once at admission.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct IslandPlan {
    admitted_at: Tick,
    desired_strike_aircraft: usize,
    desired_screen: usize,
    screen: Vec<UnitId>,
    assembly_timeout: Tick,
    dispatch: AirDispatch,
}

/// Combined-arms assault on a ground-connected cluster sized by its package.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ConnectedPlan {
    admitted_at: Tick,
    scope: TilePos,
    package: ConnectedForcePackage,
    paid_production: Vec<ConnectedPurchase>,
    dispatch: AirDispatch,
}

/// Last tactical orders issued by an assault, used to avoid reissuing them.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct AirDispatch {
    suppression: Option<SuppressionDispatch>,
    strike: Option<AirStrikeDispatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum AirStrikeDispatch {
    Attack { target: BuildingId, anchor: TilePos },
    AttackMove(TilePos),
}

impl AirPlan {
    fn remembered_connected(obs: &Observation) -> Self {
        Self::Reacquire(ReacquirePlan {
            admitted_at: obs.tick,
            assembly_timeout: CONNECTED_PREPARATION_HORIZON,
            terrain: ReacquireTerrain::Connected,
        })
    }

    #[cfg(test)]
    fn island(profile: &ResolvedProfile, obs: &Observation) -> Self {
        Self::Island(IslandPlan::new(
            profile,
            obs,
            StrategicProductionContext::empty(),
        ))
    }

    fn admitted_at(&self) -> Tick {
        match self {
            Self::Reacquire(plan) => plan.admitted_at,
            Self::Island(plan) => plan.admitted_at,
            Self::Connected(plan) => plan.admitted_at,
        }
    }

    fn airborne(&self) -> bool {
        matches!(
            self,
            Self::Island(_)
                | Self::Reacquire(ReacquirePlan {
                    terrain: ReacquireTerrain::Severed,
                    ..
                })
        )
    }

    fn package(&self) -> Option<&ConnectedForcePackage> {
        match self {
            Self::Connected(plan) => Some(&plan.package),
            Self::Reacquire(_) | Self::Island(_) => None,
        }
    }

    fn screen(&self) -> &[UnitId] {
        match self {
            Self::Island(plan) => &plan.screen,
            Self::Reacquire(_) | Self::Connected(_) => &[],
        }
    }

    fn screen_mut(&mut self) -> Option<&mut Vec<UnitId>> {
        match self {
            Self::Island(plan) => Some(&mut plan.screen),
            Self::Reacquire(_) | Self::Connected(_) => None,
        }
    }

    fn desired_artillery(&self) -> usize {
        self.package()
            .map_or(0, |package| demand_count(&package.suppression))
    }

    fn desired_strike_aircraft(&self) -> usize {
        match self {
            Self::Reacquire(_) => 0,
            Self::Island(plan) => plan.desired_strike_aircraft,
            Self::Connected(plan) => demand_count(&plan.package.strike),
        }
    }

    fn desired_screen(&self) -> usize {
        match self {
            Self::Island(plan) => plan.desired_screen,
            Self::Reacquire(_) | Self::Connected(_) => 0,
        }
    }

    /// A connected package is always installed on its derivation tick, so its
    /// window runs from that observation to the fixed preparation deadline.
    fn assembly_timeout(&self) -> Tick {
        match self {
            Self::Reacquire(plan) => plan.assembly_timeout,
            Self::Island(plan) => plan.assembly_timeout,
            Self::Connected(plan) => plan
                .package
                .preparation_deadline
                .saturating_sub(plan.package.derived_at),
        }
    }

    fn dispatch(&self) -> Option<&AirDispatch> {
        match self {
            Self::Reacquire(_) => None,
            Self::Island(plan) => Some(&plan.dispatch),
            Self::Connected(plan) => Some(&plan.dispatch),
        }
    }

    fn dispatch_mut(&mut self) -> Option<&mut AirDispatch> {
        match self {
            Self::Reacquire(_) => None,
            Self::Island(plan) => Some(&mut plan.dispatch),
            Self::Connected(plan) => Some(&mut plan.dispatch),
        }
    }

    fn suppression_dispatch(&self) -> Option<&SuppressionDispatch> {
        self.dispatch()
            .and_then(|dispatch| dispatch.suppression.as_ref())
    }

    fn strike_dispatch(&self) -> Option<AirStrikeDispatch> {
        self.dispatch().and_then(|dispatch| dispatch.strike)
    }

    fn set_suppression_dispatch(&mut self, suppression: Option<SuppressionDispatch>) {
        if let Some(dispatch) = self.dispatch_mut() {
            dispatch.suppression = suppression;
        }
    }

    fn set_strike_dispatch(&mut self, strike: Option<AirStrikeDispatch>) {
        if let Some(dispatch) = self.dispatch_mut() {
            dispatch.strike = strike;
        }
    }

    #[cfg(test)]
    fn island_mut(&mut self) -> &mut IslandPlan {
        match self {
            Self::Island(plan) => plan,
            Self::Reacquire(_) | Self::Connected(_) => panic!("expected an island plan"),
        }
    }

    #[cfg(test)]
    fn connected_mut(&mut self) -> &mut ConnectedPlan {
        match self {
            Self::Connected(plan) => plan,
            Self::Reacquire(_) | Self::Island(_) => panic!("expected a connected plan"),
        }
    }

    #[cfg(test)]
    fn package_mut(&mut self) -> Option<&mut ConnectedForcePackage> {
        match self {
            Self::Connected(plan) => Some(&mut plan.package),
            Self::Reacquire(_) | Self::Island(_) => None,
        }
    }
}

impl ReacquirePlan {
    /// Watches a severed objective with the patience its island assault
    /// would be granted if current sight admitted it now.
    fn severed(island: &IslandPlan) -> Self {
        Self {
            admitted_at: island.admitted_at,
            assembly_timeout: island.assembly_timeout,
            terrain: ReacquireTerrain::Severed,
        }
    }
}

impl ConnectedPlan {
    fn new(package: ConnectedForcePackage, admitted_at: Tick, scope: TilePos) -> Self {
        Self {
            admitted_at,
            scope,
            package,
            paid_production: Vec::new(),
            dispatch: AirDispatch::default(),
        }
    }
}

impl IslandPlan {
    fn new(
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
            desired_strike_aircraft,
            desired_screen,
            screen: Vec::new(),
            assembly_timeout,
            dispatch: AirDispatch::default(),
        }
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
    derive_connected_package(profile, obs, intel, home, target, unavailable, context).map(
        |package| {
            AirPlan::Connected(Box::new(ConnectedPlan::new(
                package,
                obs.tick,
                target.anchor,
            )))
        },
    )
}

fn connected_plan_options(
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    target: &BuildingContact,
    unavailable: &[UnitId],
    context: ConnectedPlanningContext<'_>,
) -> Result<(Vec<ConnectedPlan>, bool), ConnectedPlanRejection> {
    let packages =
        derive_connected_package_options(profile, obs, intel, home, target, unavailable, context)?;
    Ok((
        std::iter::once(packages.minimum)
            .chain(packages.marginal)
            .map(|package| ConnectedPlan::new(package, obs.tick, target.anchor))
            .collect(),
        packages.refinement_pending,
    ))
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
    // The package must refuse the same paid queue work as the resources it
    // is derived against, or it can lean on an occurrence the lowered claims
    // will not be allowed to take.
    let route = ConnectedRouteContext {
        campaign_routes: context.campaign_routes,
        unavailable_paid: context.resources.access.paid_exclusions(),
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
    if context.minimum_only {
        return Ok(packages);
    }
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
    let derive = if context.minimum_only {
        force_package::derive_connected_minimum_for_cluster
    } else {
        derive_connected_force_package_options_for_cluster
    };
    let mut packages = derive(
        profile,
        obs,
        intel,
        ConnectedTargetEvidence {
            primary: target,
            cluster: &cluster,
        },
        ProductionEvidence::with_planning(&context.resources.snapshot, &access, context.planning),
        &unavailable,
        preparation,
    )
    .map_err(|reason| ConnectedPlanRejection::Package {
        reason,
        protected_current_scrap: context.protected_current_scrap,
        protected_forecast_scrap,
    })?;
    let mut campaign_routes = BTreeMap::new();
    let mut package_has_routes = |package: &ConnectedForcePackage| {
        let roster: Vec<_> = package
            .suppression
            .iter()
            .map(|demand| (demand.kind, demand.count))
            .collect();
        *campaign_routes.entry(roster).or_insert_with(|| {
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
        })
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
    op: &AirOperation,
    package: &ConnectedForcePackage,
    resources: &ConnectedProductionResources,
    obs: &Observation,
) -> ConnectedOffenseClaims {
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
        units: member_reservations(op, &[], obs),
        paid_providers: connected_paid_provider_claims(op, package, resources, obs),
        provider_jobs,
    }
}

fn connected_paid_provider_claims(
    op: &AirOperation,
    package: &ConnectedForcePackage,
    resources: &ConnectedProductionResources,
    obs: &Observation,
) -> Vec<ConnectedPaidProvider> {
    let mut needed = BTreeMap::<UnitKind, usize>::new();
    for demand in &package.provider_priority {
        let count = needed.entry(demand.kind).or_default();
        *count = count.saturating_add(demand.count);
    }
    for id in op
        .scout
        .into_iter()
        .chain(op.artillery.iter().copied())
        .chain(op.strike_aircraft.iter().copied())
    {
        if let Some(member) = unit(obs, id)
            && let Some(count) = needed.get_mut(&member.kind)
        {
            *count = count.saturating_sub(1);
        }
    }
    for provider in &package.funded_providers {
        if let Some(count) = needed.get_mut(&provider.kind) {
            *count = count.saturating_sub(1);
        }
    }

    let mut paid = Vec::new();
    for (kind, count) in needed {
        let producers = paid_queued_ready_occurrences_with_access(
            &resources.snapshot,
            kind,
            package.preparation_deadline,
            &resources.access,
        );
        debug_assert!(
            producers.len() >= count,
            "a derived connected package must retain every paid provider it used"
        );
        paid.extend(
            producers
                .into_iter()
                .take(count)
                .map(|(producer, occurrence)| ConnectedPaidProvider {
                    producer,
                    kind,
                    occurrence,
                }),
        );
    }
    paid.sort_unstable();
    paid
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
        || !multiset_contains(&scaled.paid_providers, &minimum.paid_providers)
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
        paid_providers: multiset_difference(&scaled.paid_providers, &minimum.paid_providers),
        provider_jobs: scaled.provider_jobs[minimum.provider_jobs.len()..].to_vec(),
    })
}

fn multiset_contains<T: Ord + Copy>(superset: &[T], subset: &[T]) -> bool {
    multiset_difference(subset, superset).is_empty()
}

fn multiset_difference<T: Ord + Copy>(left: &[T], right: &[T]) -> Vec<T> {
    let mut right_counts = BTreeMap::<T, usize>::new();
    for &item in right {
        let count = right_counts.entry(item).or_default();
        *count = count.saturating_add(1);
    }
    left.iter()
        .copied()
        .filter(|item| {
            let Some(count) = right_counts.get_mut(item).filter(|count| **count > 0) else {
                return true;
            };
            *count -= 1;
            false
        })
        .collect()
}

/// A phase of the coordinated air playbook.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

impl ConnectedPlanRejection {
    pub(crate) fn is_deferred(self) -> bool {
        matches!(
            self,
            Self::Package {
                reason: ForcePackageRejection::Deferred,
                ..
            }
        )
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum AirOperationOutcome {
    Released { player: PlayerId, target: TilePos },
    Aborted { player: PlayerId, target: TilePos },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum AirStage {
    Watching,
    Recon,
    Assemble,
    SuppressAa,
    Verify,
    Strike,
    Recover {
        reason: AirRecoveryReason,
        assault_admitted: bool,
    },
}

/// Inspectable persistent state of the active operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AirOperation {
    /// Last known target owner.
    pub target_player: PlayerId,
    /// Last known target kind.
    pub target_kind: BuildingKind,
    /// Stable target footprint anchor.
    pub target: TilePos,
    /// Last live id, used only with current evidence.
    pub target_id: Option<BuildingId>,
    pub(super) stage: AirStage,
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
}

impl AirOperation {
    /// Current playbook phase, including observation-only reconnaissance.
    pub fn phase(&self) -> AirOperationPhase {
        match self.stage {
            AirStage::Watching | AirStage::Recon => AirOperationPhase::Recon,
            AirStage::Assemble => AirOperationPhase::Assemble,
            AirStage::SuppressAa => AirOperationPhase::SuppressAa,
            AirStage::Verify => AirOperationPhase::Verify,
            AirStage::Strike => AirOperationPhase::Strike,
            AirStage::Recover { .. } => AirOperationPhase::Recover,
        }
    }

    /// Whether current sight has admitted non-scout spending and reservations.
    pub fn assault_admitted(&self) -> bool {
        match self.stage {
            AirStage::Watching => false,
            AirStage::Recover {
                assault_admitted, ..
            } => assault_admitted,
            _ => true,
        }
    }

    /// Why the operation is withdrawing, if it is in recovery.
    pub fn recovery_reason(&self) -> Option<AirRecoveryReason> {
        match self.stage {
            AirStage::Recover { reason, .. } => Some(reason),
            _ => None,
        }
    }

    fn admit_assault(&mut self, now: Tick) {
        self.stage = match self.stage {
            AirStage::Watching => AirStage::Recon,
            AirStage::Recover { reason, .. } => AirStage::Recover {
                reason,
                assault_admitted: true,
            },
            stage => stage,
        };
        self.started_at = now;
        self.phase_started_at = now;
    }
}

/// Role-preserving survivors held only through the operation cooldown.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Current scrap held independently of this decision's production requests.
    pub reserved_scrap: u32,
}

impl StrategicDecision {
    pub(crate) fn production(&self) -> impl Iterator<Item = (BuildingId, UnitKind)> + '_ {
        self.intents.iter().filter_map(|intent| match intent {
            Intent::TrainAt { building, kind } => Some((*building, *kind)),
            _ => None,
        })
    }

    /// Current capital owned by held reserves and exact immediate purchases.
    pub fn committed_scrap(&self) -> u32 {
        self.production()
            .fold(self.reserved_scrap, |total, (_, kind)| {
                total.saturating_add(kind.stats().cost)
            })
    }
}

struct AirPlanningContext<'a> {
    allow_procurement: bool,
    planning: Option<&'a crate::planning::PlanningWork>,
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
    unavailable_paid: &'a [(BuildingId, UnitKind, usize)],
    prior_intents: &'a [Intent],
    lane_reservations: &'a ProducerLaneReservations,
}

impl StrategicProductionContext<'static> {
    fn empty() -> Self {
        Self {
            prior_intents: &[],
            unavailable_paid: &[],
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
    pub planning: Option<&'a crate::planning::PlanningWork>,
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ActiveAirOperation {
    op: AirOperation,
    plan: AirPlan,
}

impl ActiveAirOperation {
    fn valid_checkpoint(&self, map: &PublicMapBriefing, tick: Tick) -> bool {
        let Self { op, plan } = self;
        let kind_matches_stage = match plan {
            AirPlan::Reacquire(_) => !op.assault_admitted() && op.membership_frozen_at.is_none(),
            AirPlan::Island(_) | AirPlan::Connected(_) => op.assault_admitted(),
        };
        let committed_stage = matches!(
            op.stage,
            AirStage::SuppressAa | AirStage::Verify | AirStage::Strike
        );
        plan.admitted_at() <= op.started_at
            && op.started_at <= op.phase_started_at
            && op.phase_started_at <= tick
            && op.membership_frozen_at.is_none_or(|at| at <= tick)
            && op.strike_issued_at.is_none_or(|at| at <= tick)
            && kind_matches_stage
            && (!committed_stage || op.membership_frozen_at.is_some())
            && strictly_increasing(&op.artillery)
            && strictly_increasing(&op.strike_aircraft)
            && strictly_increasing(plan.screen())
            && on_map(map, op.target)
            && op.scout_dispatch.is_none_or(|(_, goal)| on_map(map, goal))
            && op.strike_hold.is_none_or(|tile| on_map(map, tile))
            && op.artillery_staging.is_none_or(|tile| on_map(map, tile))
            && plan
                .dispatch()
                .is_none_or(|dispatch| dispatch.valid_checkpoint(map))
            && match plan {
                AirPlan::Connected(connected) => connected.valid_checkpoint(op, map, tick),
                AirPlan::Reacquire(_) | AirPlan::Island(_) => true,
            }
    }
}

impl AirDispatch {
    fn valid_checkpoint(&self, map: &PublicMapBriefing) -> bool {
        let suppression = match &self.suppression {
            Some(SuppressionDispatch::Position { assignments, .. }) => {
                assignments.iter().all(|(_, stand)| on_map(map, *stand))
            }
            Some(SuppressionDispatch::Attack { .. }) | None => true,
        };
        let strike = match self.strike {
            Some(
                AirStrikeDispatch::Attack { anchor, .. } | AirStrikeDispatch::AttackMove(anchor),
            ) => on_map(map, anchor),
            None => true,
        };
        suppression && strike
    }
}

impl ConnectedPlan {
    /// The package's demand drives per-provider job expansion, so its counts
    /// are bounded by the map area rather than trusted from the checkpoint.
    fn valid_checkpoint(&self, op: &AirOperation, map: &PublicMapBriefing, tick: Tick) -> bool {
        let area = usize::try_from(map.map_width())
            .unwrap_or(0)
            .saturating_mul(usize::try_from(map.map_height()).unwrap_or(0));
        let package = &self.package;
        let anchors = &package.target_anchors;
        op.target_id.is_some()
            && on_map(map, self.scope)
            && !anchors.is_empty()
            && anchors
                .windows(2)
                .all(|pair| (pair[0].y, pair[0].x) < (pair[1].y, pair[1].x))
            && anchors.iter().all(|anchor| on_map(map, *anchor))
            && anchors.contains(&op.target)
            && package.derived_at <= tick
            && package.derived_at <= package.preparation_deadline
            && package.preparation_deadline
                <= package
                    .derived_at
                    .saturating_add(CONNECTED_PREPARATION_HORIZON)
            && [&package.recon, &package.suppression, &package.strike]
                .into_iter()
                .all(|demands| bounded_total(demands.iter().map(|demand| demand.count), area))
            && bounded_total(
                package
                    .provider_priority
                    .iter()
                    .map(|tranche| tranche.count),
                area,
            )
            && package.funded_providers.len() <= area
            && self.paid_production.iter().all(|purchase| {
                purchase.issued_at <= purchase.ready_at && purchase.issued_at <= tick
            })
    }
}

fn bounded_total(counts: impl IntoIterator<Item = usize>, limit: usize) -> bool {
    counts
        .into_iter()
        .try_fold(0usize, usize::checked_add)
        .is_some_and(|total| total <= limit)
}

fn strictly_increasing(ids: &[UnitId]) -> bool {
    ids.windows(2).all(|pair| pair[0] < pair[1])
}

fn on_map(map: &PublicMapBriefing, tile: TilePos) -> bool {
    (0..map.map_width()).contains(&tile.x) && (0..map.map_height()).contains(&tile.y)
}

/// Stable owner identity shared by a connected proposal and every exact
/// producer assignment returned for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConnectedOffenseIdentity {
    objective: BuildingId,
    anchor: TilePos,
}

impl ConnectedOffenseIdentity {
    pub(crate) const fn new(objective: BuildingId, anchor: TilePos) -> Self {
        Self { objective, anchor }
    }

    pub(crate) const fn objective(self) -> BuildingId {
        self.objective
    }

    pub(crate) const fn anchor(self) -> TilePos {
        self.anchor
    }
}

/// A purchase already emitted through shared allocation. Predicted completion
/// only releases ownership after the observed queue can actually advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ConnectedPurchase {
    producer: BuildingId,
    kind: UnitKind,
    issued_at: Tick,
    ready_at: Tick,
    delayed: bool,
}

impl ConnectedPurchase {
    pub(crate) const fn producer(self) -> BuildingId {
        self.producer
    }
    pub(crate) const fn kind(self) -> UnitKind {
        self.kind
    }
}

/// One unpaid provider retained by an exact connected-offense proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectedProviderJob {
    kind: UnitKind,
    enqueue_not_before: Tick,
    ready_before: Tick,
    eligible_producers: Vec<BuildingId>,
}

impl ConnectedProviderJob {
    /// Concrete provider selected by the offense domain.
    pub(crate) const fn kind(&self) -> UnitKind {
        self.kind
    }

    /// First command boundary at which the package's funding evidence permits
    /// this provider.
    pub(crate) const fn enqueue_not_before(&self) -> Tick {
        self.enqueue_not_before
    }

    /// Immutable preparation deadline shared by the complete package.
    pub(crate) const fn ready_before(&self) -> Tick {
        self.ready_before
    }

    /// Exact completed producers that passed both production and route
    /// preflight for this provider.
    pub(crate) fn eligible_producers(&self) -> &[BuildingId] {
        &self.eligible_producers
    }

    #[cfg(test)]
    pub(crate) fn fixture(
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
pub(crate) struct ConnectedOffenseClaims {
    units: Vec<UnitId>,
    paid_providers: Vec<ConnectedPaidProvider>,
    provider_jobs: Vec<ConnectedProviderJob>,
}

/// One exact already-paid queue occurrence used by a connected package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ConnectedPaidProvider {
    producer: BuildingId,
    kind: UnitKind,
    occurrence: usize,
}

impl ConnectedPaidProvider {
    pub(crate) const fn occurrence(self) -> usize {
        self.occurrence
    }
    pub(crate) const fn producer(self) -> BuildingId {
        self.producer
    }

    pub(crate) const fn kind(self) -> UnitKind {
        self.kind
    }
}

impl ConnectedOffenseClaims {
    /// Exact live providers reserved by this package.
    pub(crate) fn units(&self) -> &[UnitId] {
        &self.units
    }

    /// Exact paid queue occurrences that satisfy this package's demand.
    pub(crate) fn paid_providers(&self) -> &[ConnectedPaidProvider] {
        &self.paid_providers
    }

    /// Exact unpaid production requests retained by this package.
    pub(crate) fn provider_jobs(&self) -> &[ConnectedProviderJob] {
        &self.provider_jobs
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        mut units: Vec<UnitId>,
        provider_jobs: Vec<ConnectedProviderJob>,
    ) -> Self {
        units.sort_unstable();
        units.dedup();
        Self {
            units,
            paid_providers: Vec::new(),
            provider_jobs,
        }
    }
}

/// How quickly the currently observed connected opportunity warrants action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedUrgency {
    Developmental,
    Timely,
    Pressing,
}

/// Quality of the evidence supporting a connected operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedConfidence {
    Prior,
    Supported,
    Current,
}

/// Strategic consequence of successfully prosecuting the retained cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedStrategicValue {
    Incremental,
    Material,
    Decisive,
}

/// Time until the retained minimum can begin affecting the objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedTimeToImpact {
    Patient,
    Near,
    Immediate,
}

/// Confidence that the retained minimum can execute against known defenses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectedExecutionSafety {
    Speculative,
    Managed,
    Secure,
}

/// Named, fog-honest comparison case for one exact connected opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConnectedOpportunityCase {
    urgency: ConnectedUrgency,
    confidence: ConnectedConfidence,
    value: ConnectedStrategicValue,
    time_to_impact: ConnectedTimeToImpact,
    safety: ConnectedExecutionSafety,
}

impl ConnectedOpportunityCase {
    pub(crate) const fn urgency(self) -> ConnectedUrgency {
        self.urgency
    }

    pub(crate) const fn confidence(self) -> ConnectedConfidence {
        self.confidence
    }

    pub(crate) const fn value(self) -> ConnectedStrategicValue {
        self.value
    }

    pub(crate) const fn time_to_impact(self) -> ConnectedTimeToImpact {
        self.time_to_impact
    }

    pub(crate) const fn safety(self) -> ConnectedExecutionSafety {
        self.safety
    }

    #[cfg(test)]
    pub(crate) const fn fixture(
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
    op: AirOperation,
    plan: ConnectedPlan,
    claims: ConnectedOffenseClaims,
}

impl ConnectedProposalVariant {
    fn into_active(self) -> ActiveAirOperation {
        ActiveAirOperation {
            op: self.op,
            plan: AirPlan::Connected(Box::new(self.plan)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConnectedProposalOrigin {
    Idle {
        standby: AirStandby,
    },
    Remembered {
        active: ActiveAirOperation,
    },
    Active {
        op: AirOperation,
        plan: Box<ConnectedPlan>,
    },
}

/// One deterministic cumulative addition above an accepted connected minimum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectedMarginalVariant {
    variant_index: usize,
    additions: ConnectedOffenseClaims,
}

impl ConnectedMarginalVariant {
    /// Cumulative claims added above the common minimum.
    pub(crate) const fn additions(&self) -> &ConnectedOffenseClaims {
        &self.additions
    }
}

/// Pure, exact connected-offense proposal submitted for cross-domain
/// adjudication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FreshConnectedProposal {
    origin: ConnectedProposalOrigin,
    target: BuildingContact,
    variants: Vec<ConnectedProposalVariant>,
    marginal: Vec<ConnectedMarginalVariant>,
    selected_variant: usize,
    case: ConnectedOpportunityCase,
}

impl FreshConnectedProposal {
    pub(crate) fn revises_active_operation(&self) -> bool {
        matches!(self.origin, ConnectedProposalOrigin::Active { .. })
    }

    /// Stable identity retained by every minimum and marginal variant.
    pub(crate) fn identity(&self) -> ConnectedOffenseIdentity {
        ConnectedOffenseIdentity::new(self.objective(), self.anchor())
    }

    /// Exact current objective selected by domain ranking.
    pub(crate) fn objective(&self) -> BuildingId {
        self.target
            .id
            .expect("an admitted current objective has an exact building id")
    }

    /// Row-major anchor paired with the exact objective id.
    pub(crate) const fn anchor(&self) -> TilePos {
        self.target.anchor
    }

    /// Fixed deadline shared by the minimum and all marginal variants.
    pub(crate) fn deadline(&self) -> Tick {
        self.variants[0].plan.package.preparation_deadline
    }

    /// Original strategic admission tick retained across remembered
    /// reconnaissance and fresh cross-domain assault adjudication.
    pub(crate) fn accepted_at(&self) -> Tick {
        self.variants[0].plan.admitted_at
    }

    /// Named evidence and consequence bands used by cross-domain ranking.
    pub(crate) const fn case(&self) -> ConnectedOpportunityCase {
        self.case
    }

    /// Shared claims of the independently admissible common minimum.
    pub(crate) fn minimum_claims(&self) -> &ConnectedOffenseClaims {
        &self.variants[0].claims
    }

    /// Deterministic cumulative additions above the common minimum. Every row
    /// preserves all earlier claims.
    pub(crate) fn marginal_variants(&self) -> &[ConnectedMarginalVariant] {
        &self.marginal
    }

    /// Selects one exact marginal variant previously returned by
    /// [`Self::marginal_variants`].
    pub(crate) fn select_marginal(&mut self, marginal: &ConnectedMarginalVariant) -> bool {
        if self.marginal.get(marginal.variant_index.saturating_sub(1)) != Some(marginal) {
            return false;
        }
        self.selected_variant = marginal.variant_index;
        true
    }

    #[cfg(test)]
    pub(crate) fn fixture(fixture: FreshConnectedProposalFixture) -> Self {
        let FreshConnectedProposalFixture {
            objective,
            anchor,
            deadline,
            case,
            minimum_claims,
            marginal_additions,
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
        let plan = ConnectedPlan::new(package, derived_at, anchor);
        let op = AirOperation {
            target_player: PlayerId(1),
            target_kind: BuildingKind::Crucible,
            target: anchor,
            target_id: Some(objective),
            stage: AirStage::Recon,
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
        };
        let mut cumulative = minimum_claims.clone();
        let mut variants = vec![ConnectedProposalVariant {
            op: op.clone(),
            plan: plan.clone(),
            claims: minimum_claims,
        }];
        let mut marginal = Vec::with_capacity(marginal_additions.len());
        for (offset, additions) in marginal_additions.into_iter().enumerate() {
            cumulative.units.extend(additions.units.iter().copied());
            cumulative.units.sort_unstable();
            cumulative.units.dedup();
            cumulative
                .paid_providers
                .extend(additions.paid_providers.iter().copied());
            cumulative.paid_providers.sort_unstable();
            cumulative
                .provider_jobs
                .extend(additions.provider_jobs.iter().cloned());
            variants.push(ConnectedProposalVariant {
                op: op.clone(),
                plan: plan.clone(),
                claims: cumulative.clone(),
            });
            marginal.push(ConnectedMarginalVariant {
                variant_index: offset + 1,
                additions,
            });
        }
        Self {
            origin: ConnectedProposalOrigin::Idle {
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
            variants,
            marginal,
            selected_variant: 0,
            case,
        }
    }

    #[cfg(test)]
    pub(crate) fn into_active_revision_fixture(mut self) -> Self {
        self.origin = ConnectedProposalOrigin::Active {
            op: self.variants[0].op.clone(),
            plan: Box::new(self.variants[0].plan.clone()),
        };
        self
    }
}

#[cfg(test)]
pub(crate) struct FreshConnectedProposalFixture {
    pub(crate) objective: BuildingId,
    pub(crate) anchor: TilePos,
    pub(crate) deadline: Tick,
    pub(crate) case: ConnectedOpportunityCase,
    pub(crate) minimum_claims: ConnectedOffenseClaims,
    pub(crate) marginal_additions: Vec<ConnectedOffenseClaims>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AirMembership {
    scout: Option<UnitId>,
    artillery: Vec<UnitId>,
    strike_aircraft: Vec<UnitId>,
    screen: Vec<UnitId>,
}

impl AirMembership {
    fn from_active(active: &ActiveAirOperation) -> Self {
        Self {
            scout: active.op.scout,
            artillery: active.op.artillery.clone(),
            strike_aircraft: active.op.strike_aircraft.clone(),
            screen: active.plan.screen().to_vec(),
        }
    }

    fn units(&self, obs: &Observation) -> Vec<UnitId> {
        let mut units: Vec<_> = self
            .scout
            .into_iter()
            .chain(self.artillery.iter().copied())
            .chain(self.strike_aircraft.iter().copied())
            .chain(self.screen.iter().copied())
            .filter(|id| unit(obs, *id).is_some())
            .collect();
        units.sort_unstable();
        units.dedup();
        units
    }

    pub(crate) fn apply(self, planner: &mut StrategicPlanner, now: Tick) {
        let active = planner
            .air
            .as_mut()
            .expect("validated air operation remains active");
        let previous_scout = active.op.scout;
        let previous_artillery = core::mem::replace(&mut active.op.artillery, self.artillery);
        let previous_strike =
            core::mem::replace(&mut active.op.strike_aircraft, self.strike_aircraft);
        active.op.scout = self.scout;
        if let Some(screen) = active.plan.screen_mut() {
            *screen = self.screen;
        }
        if previous_scout != active.op.scout
            && active.op.scout.is_some()
            && (matches!(active.plan, AirPlan::Connected(_))
                || active.op.phase() == AirOperationPhase::Recon)
        {
            active.op.phase_started_at = now;
        }
        invalidate_reassigned_member_orders(
            &mut active.op,
            previous_scout,
            &previous_artillery,
            &previous_strike,
        );
    }
}

pub(crate) struct IslandPreparation {
    pub(crate) membership: AirMembership,
    pub(crate) purchases: ProductionPlan,
}

impl IslandPreparation {
    pub(crate) fn units(&self, obs: &Observation) -> Vec<UnitId> {
        self.membership.units(obs)
    }
}

struct AirRoster<'a> {
    scout: Option<UnitId>,
    artillery: &'a [UnitId],
    strike_aircraft: &'a [UnitId],
}

impl<'a> From<&'a AirOperation> for AirRoster<'a> {
    fn from(op: &'a AirOperation) -> Self {
        Self {
            scout: op.scout,
            artillery: &op.artillery,
            strike_aircraft: &op.strike_aircraft,
        }
    }
}

/// Mandatory continuation imported into the next allocation pass for an
/// already-admitted connected operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveConnectedObligation {
    pub(crate) membership: AirMembership,
    identity: ConnectedOffenseIdentity,
    accepted_at: Tick,
    deadline: Tick,
    units: Vec<UnitId>,
    provider_jobs: Vec<ConnectedProviderJob>,
}

impl ActiveConnectedObligation {
    pub(crate) const fn identity(&self) -> ConnectedOffenseIdentity {
        self.identity
    }

    pub(crate) const fn accepted_at(&self) -> Tick {
        self.accepted_at
    }

    pub(crate) const fn deadline(&self) -> Tick {
        self.deadline
    }

    pub(crate) fn units(&self) -> &[UnitId] {
        &self.units
    }

    pub(crate) fn provider_jobs(&self) -> &[ConnectedProviderJob] {
        &self.provider_jobs
    }
}

#[derive(Clone, Copy)]
pub(crate) struct StrategicThinkContext<'a> {
    profile: &'a ResolvedProfile,
    tuning: DifficultyTuning,
    obs: &'a Observation,
    intel: &'a StrategicIntelligence,
    home: TilePos,
    coordination: StrategicCoordination<'a>,
    production: StrategicProductionContext<'a>,
    owned_only: bool,
    claimed_elsewhere: &'a [UnitId],
}

impl<'a> StrategicThinkContext<'a> {
    pub(crate) fn new(
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
            owned_only: false,
            claimed_elsewhere: &[],
        }
    }

    pub(crate) fn with_external_claims(mut self, claims: &'a [UnitId]) -> Self {
        self.claimed_elsewhere = claims;
        self
    }

    pub(crate) fn with_owned_members(mut self, owned_only: bool) -> Self {
        self.owned_only = owned_only;
        self
    }

    pub(crate) fn with_producer_lanes(
        mut self,
        prior_intents: &'a [Intent],
        lane_reservations: &'a ProducerLaneReservations,
    ) -> Self {
        self.production = StrategicProductionContext {
            prior_intents,
            lane_reservations,
            ..self.production
        };
        self
    }

    pub(crate) fn with_paid_exclusions(
        mut self,
        excluded: &'a [(BuildingId, UnitKind, usize)],
    ) -> Self {
        self.production.unavailable_paid = excluded;
        self
    }
}

/// Complete same-observation evidence for one pure connected-offense proposal.
#[derive(Clone, Copy)]
pub(crate) struct FreshConnectedProposalRequest<'a> {
    unavailable_paid: &'a [(BuildingId, UnitKind, usize)],
    profile: &'a ResolvedProfile,
    tuning: DifficultyTuning,
    obs: &'a Observation,
    resource_snapshot: &'a ResourceSnapshot,
    intel: &'a StrategicIntelligence,
    home: TilePos,
    coordination: StrategicCoordination<'a>,
}

impl<'a> FreshConnectedProposalRequest<'a> {
    pub(crate) fn with_paid_exclusions(
        mut self,
        excluded: &'a [(BuildingId, UnitKind, usize)],
    ) -> Self {
        self.unavailable_paid = excluded;
        self
    }
    pub(crate) const fn new(
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
            unavailable_paid: &[],
            intel,
            home,
            coordination,
        }
    }
}

#[derive(Clone, Copy)]
struct FreshConnectedDerivationContext<'a> {
    minimum_only: bool,
    campaign_routes: Option<&'a CampaignRoutes<'a>>,
    unavailable_paid: &'a [(BuildingId, UnitKind, usize)],
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
    let package = &minimum.plan.package;
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

/// Whether a remembered or current structure can anchor a prospective first
/// Airworks campaign.
pub(crate) fn prospective_air_target(contact: &BuildingContact, now: Tick) -> bool {
    contact.built && contact.hp > 0 && contact.confidence_at(now) > 0
}

/// Values a complete, route-serviceable minimum after a proposed first Airworks.
/// The hypothetical producer is confined to sizing; it never becomes an owned claim.
/// Sizing assumes remembered targets still stand as last seen, so each value is
/// discounted by that target's confidence.
#[expect(
    clippy::too_many_arguments,
    reason = "the sizing witness mirrors the exact quote inputs it must respect"
)]
pub(crate) fn prospective_airworks_package_value(
    request: FreshConnectedProposalRequest<'_>,
    candidate: crate::observation::BuildingObs,
    candidate_sites: &[TilePos],
    ready_after: Tick,
    fund_by: Tick,
    deadline: Tick,
    obligations: &[crate::allocation::ImportedObligation],
    planning: &crate::planning::PlanningWork,
) -> Option<u64> {
    use crate::allocation::{
        AllocationCapacity, AllocationPersonality, ClaimBundle, DeferrableCapitalClaim,
        ImportedObligation, ObligationClass, ObligationKey, allocate_requiring_planned,
        connected_investment_proposal, current_reserve_at,
    };
    use crate::planning::Progress;
    let site = candidate.anchor;
    if !planning.campaign_site_selected(request.obs.tick, site, candidate_sites) {
        return None;
    }
    let mut prospective = request.obs.clone();
    let cost = BuildingKind::Airworks.base_stats().construction?.cost;
    let bank = prospective
        .scrap
        .saturating_sub(request.coordination.protected_current_scrap);
    let paid_now = bank.min(cost);
    let shortfall = cost - paid_now;
    prospective.scrap = bank - paid_now;
    prospective.my_buildings.push(candidate);
    prospective.my_queues.push(Vec::new());
    prospective.my_queue_progress.push(0);
    let resources = ResourceSnapshot::from_observation(&prospective);
    // The sizing bank excludes current promises; exact allocation imports them itself.
    let restored = request
        .coordination
        .protected_current_scrap
        .min(current_reserve_at(obligations, prospective.tick))
        .min(request.obs.scrap - bank);
    prospective.scrap += restored;
    let capacity = AllocationCapacity::from_snapshot(
        &ResourceSnapshot::from_observation(&prospective),
        deadline,
        request.tuning.cadence,
    )
    .ok()?;
    // Capital the bank cannot cover now is owed from forecast by the factory's
    // funding deadline, exactly as the proposed saving will be charged.
    let mut obligations = obligations.to_vec();
    if shortfall > 0 {
        obligations.push(ImportedObligation {
            class: ObligationClass::PersistentPlan,
            accepted_at: prospective.tick,
            key: ObligationKey::SavedEconomy(crate::utility::EconomicInvestmentKey::Build {
                kind: BuildingKind::Airworks,
                anchor: site,
            }),
            claims: ClaimBundle::new(
                0,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            )
            .ok()?
            .with_deferrable_capital(DeferrableCapitalClaim {
                through: fund_by,
                amount: shortfall,
            })
            .ok()?,
        });
    }
    // A new campaign cannot make retained obligations fit after buying its factory.
    if !matches!(
        super::allocation::forecast::refine_obligations(
            &capacity,
            &obligations,
            request.coordination.planning?,
        ),
        super::planning::Progress::Ready(())
    ) {
        return None;
    }
    prospective.scrap -= restored;
    let unavailable: Vec<_> = prospective.my_units.iter().map(|unit| unit.id).collect();
    let coordination = StrategicCoordination {
        enlisted: &unavailable,
        protected_current_scrap: 0,
        protected_forecast_scrap: request
            .coordination
            .protected_forecast_scrap
            .saturating_add(shortfall),
        ..request.coordination
    };
    let intel = request
        .intel
        .assuming_remembered_buildings(prospective.tick);
    let campaign_routes = CampaignRoutes::new(
        &prospective,
        &intel,
        coordination.public_map,
        coordination.orientation,
    );
    let context = FreshConnectedDerivationContext {
        minimum_only: true,
        campaign_routes: Some(&campaign_routes),
        unavailable_paid: &[],
        profile: request.profile,
        tuning: request.tuning,
        obs: &prospective,
        resource_snapshot: &resources,
        intel: &intel,
        home: request.home,
        coordination,
        unavailable: &unavailable,
        preferred_artillery: &[],
    };
    let deadline = deadline.checked_sub(ready_after)?;
    if deadline <= prospective.tick {
        return None;
    }
    let confidence = |target: &BuildingContact| {
        request
            .intel
            .buildings()
            .iter()
            .find(|remembered| {
                remembered.player == target.player && remembered.anchor == target.anchor
            })
            .map_or(0, |remembered| remembered.confidence_at(prospective.tick))
    };
    let mut targets: Vec<_> = intel
        .buildings()
        .iter()
        .filter(|target| target.built && target.hp > 0 && confidence(target) > 0)
        .collect();
    let distances = coordination
        .public_map
        .map(|map| map.regions().distances(request.home));
    targets.sort_by_key(|target| {
        (
            std::cmp::Reverse(
                u64::from(target.hp)
                    * u64::from(building_value(target.kind))
                    * u64::from(confidence(target)),
            ),
            distances
                .as_ref()
                .and_then(|distances| distances.estimate(target.anchor))
                .unwrap_or(u32::MAX),
            target.anchor.y,
            target.anchor.x,
            target.id,
        )
    });
    let keys: Vec<_> = targets
        .iter()
        .map(|target| (target.player, target.anchor, target.id))
        .collect();
    let result = planning.campaign_candidate(prospective.tick, site, &keys, |key| {
        let target = targets
            .iter()
            .find(|target| (target.player, target.anchor, target.id) == key)
            .unwrap();
        #[cfg(test)]
        AIRWORKS_PACKAGE_DERIVATIONS.with(|count| count.set(count.get() + 1));
        let route = ConnectedRouteContext {
            campaign_routes: Some(&campaign_routes),
            unavailable_paid: &[],
            intel: &intel,
            home: request.home,
            target: target.anchor,
            public_map: coordination.public_map,
            orientation: coordination.orientation,
        };
        let initial = ConnectedProductionResources::from_snapshot_after_current_reserve(
            &prospective,
            target,
            &unavailable,
            route,
            &resources,
            0,
        );
        let derived = derive_connected_proposal_with_resources(
            context,
            target,
            ConnectedProposalOrigin::Idle {
                standby: AirStandby::default(),
            },
            initial,
            deadline,
        );
        let proposal = match derived {
            Ok(proposal) => proposal,
            Err(ConnectedPlanRejection::Package {
                reason: ForcePackageRejection::Deferred,
                ..
            }) => return Progress::Deferred,
            Err(_) => return Progress::ProvenInfeasible,
        };
        let investment = connected_investment_proposal(proposal.clone());
        match allocate_requiring_planned(
            &capacity,
            obligations.clone(),
            vec![investment.clone()],
            AllocationPersonality::default(),
            investment.key(),
            planning,
        ) {
            Ok(Some(_)) => {}
            Ok(None) => return Progress::Deferred,
            Err(_) => return Progress::ProvenInfeasible,
        }
        let package_cost: u64 = proposal
            .minimum_claims()
            .provider_jobs()
            .iter()
            .map(|job| u64::from(job.kind().stats().cost))
            .sum();
        Progress::Ready(
            package_cost * u64::from(confidence(target))
                / u64::from(crate::intelligence::MAX_CONFIDENCE),
        )
    });
    match result {
        Progress::Ready(value) => Some(value),
        Progress::Deferred | Progress::Exhausted | Progress::ProvenInfeasible => None,
    }
}

fn derive_fresh_connected_proposal(
    context: FreshConnectedDerivationContext<'_>,
    target: &BuildingContact,
    origin: ConnectedProposalOrigin,
) -> Result<FreshConnectedProposal, ConnectedPlanRejection> {
    let local_routes;
    let context = if context.campaign_routes.is_some() {
        context
    } else {
        local_routes = CampaignRoutes::new(
            context.obs,
            context.intel,
            context.coordination.public_map,
            context.coordination.orientation,
        );
        FreshConnectedDerivationContext {
            campaign_routes: Some(&local_routes),
            ..context
        }
    };
    let route = ConnectedRouteContext {
        campaign_routes: context.campaign_routes,
        unavailable_paid: context.unavailable_paid,
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
        minimum_only,
        campaign_routes,
        unavailable_paid,
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
    let local_routes;
    let campaign_routes = Some(match campaign_routes {
        Some(routes) => routes,
        None => {
            local_routes = CampaignRoutes::new(
                obs,
                intel,
                coordination.public_map,
                coordination.orientation,
            );
            &local_routes
        }
    });
    let route = ConnectedRouteContext {
        campaign_routes,
        unavailable_paid,
        intel,
        home,
        target: target.anchor,
        public_map: coordination.public_map,
        orientation: coordination.orientation,
    };
    let (mut plans, refinement_pending) = connected_plan_options(
        profile,
        obs,
        intel,
        home,
        target,
        unavailable,
        ConnectedPlanningContext {
            planning: coordination.planning,
            minimum_only,
            campaign_routes,
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
    if refinement_pending && matches!(origin, ConnectedProposalOrigin::Active { .. }) {
        return Err(ConnectedPlanRejection::Package {
            reason: ForcePackageRejection::Deferred,
            protected_current_scrap: coordination.protected_current_scrap,
            protected_forecast_scrap: coordination.protected_forecast_scrap,
        });
    }
    let resources = ConnectedProductionResources::from_package_snapshot_after_current_reserve(
        obs,
        target.player,
        &plans[0].package,
        route,
        resource_snapshot,
        coordination.protected_current_scrap,
    );
    let prior_admitted_at = match &origin {
        ConnectedProposalOrigin::Idle { .. } => obs.tick,
        ConnectedProposalOrigin::Remembered { active } => active.plan.admitted_at(),
        ConnectedProposalOrigin::Active { plan, .. } => plan.admitted_at,
    };
    let route_unavailable =
        connected_provider_unavailable(obs, &resources.targets, unavailable, route);
    let mut variants = Vec::with_capacity(plans.len());
    for mut plan in plans.drain(..) {
        plan.admitted_at = prior_admitted_at;
        if let ConnectedProposalOrigin::Active { plan: active, .. } = &origin {
            plan.scope = active.scope;
        }
        let mut op = connected_proposal_operation(&origin, target, obs.tick);
        let package = &plan.package;
        let previous_scout = op.scout;
        let previous_artillery = op.artillery.clone();
        let previous_strike_aircraft = op.strike_aircraft.clone();
        let mut scouts = op.scout.into_iter().collect::<Vec<_>>();
        assign_provider_demands(&mut scouts, &package.recon, obs, &route_unavailable);
        op.scout = scouts.into_iter().next();
        assign_provider_demands(
            &mut op.artillery,
            &package.suppression,
            obs,
            &route_unavailable,
        );
        assign_provider_demands(
            &mut op.strike_aircraft,
            &package.strike,
            obs,
            &route_unavailable,
        );
        invalidate_reassigned_member_orders(
            &mut op,
            previous_scout,
            &previous_artillery,
            &previous_strike_aircraft,
        );
        let claims = connected_proposal_claims(&op, package, &resources, obs);
        variants.push(ConnectedProposalVariant { op, plan, claims });
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
        variants,
        marginal,
        selected_variant: 0,
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
            stage: AirStage::Recon,
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
        },
        ConnectedProposalOrigin::Remembered { active } => {
            let mut op = active.op.clone();
            op.target_player = target.player;
            op.target_kind = target.kind;
            op.target = target.anchor;
            op.target_id = target.id;
            op.admit_assault(admitted_at);
            op
        }
        ConnectedProposalOrigin::Active { op, .. } => {
            let mut op = op.clone();
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
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StrategicPlanner {
    pub(crate) outcomes: super::experience::OutcomeJournal,
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

    /// Rejects restored state that could panic, cause unbounded work, or
    /// break an ordering later passes rely on. A forged value that only
    /// changes play, such as a cooldown, is accepted.
    pub(crate) fn valid_checkpoint(&self, map: &PublicMapBriefing, tick: Tick) -> bool {
        let mut members: Vec<_> = self.owned_units().collect();
        members.sort_unstable();
        members.windows(2).all(|pair| pair[0] != pair[1])
            && strictly_increasing(&self.standby.artillery)
            && strictly_increasing(&self.standby.strike_aircraft)
            && self.terminal_outcome.is_none_or(|outcome| match outcome {
                AirOperationOutcome::Released { target, .. }
                | AirOperationOutcome::Aborted { target, .. } => on_map(map, target),
            })
            && self
                .air
                .as_ref()
                .is_none_or(|active| active.valid_checkpoint(map, tick))
    }

    /// Scout-only watch over a remembered connected objective.
    #[cfg(test)]
    pub(crate) fn remembered_watch_fixture(
        target_player: PlayerId,
        target: TilePos,
        tick: Tick,
    ) -> Self {
        let op = AirOperation {
            target_player,
            target_kind: BuildingKind::Foundry,
            target,
            target_id: None,
            stage: AirStage::Watching,
            started_at: tick,
            phase_started_at: tick,
            scout: None,
            scout_dispatch: None,
            strike_hold: None,
            artillery_staging: None,
            artillery: Vec::new(),
            strike_aircraft: Vec::new(),
            strike_issued_at: None,
            membership_frozen_at: None,
        };
        let plan = AirPlan::Reacquire(ReacquirePlan {
            admitted_at: tick,
            assembly_timeout: CONNECTED_PREPARATION_HORIZON,
            terrain: ReacquireTerrain::Connected,
        });
        Self {
            air: Some(ActiveAirOperation { op, plan }),
            ..Self::new()
        }
    }

    /// Active operation for replay diagnostics.
    pub fn air_operation(&self) -> Option<&AirOperation> {
        self.air.as_ref().map(|active| &active.op)
    }

    pub(crate) fn air_capacity_deadline(&self) -> Option<Tick> {
        let active = self.air.as_ref()?;
        Some(active.plan.package().map_or_else(
            || {
                active
                    .op
                    .started_at
                    .saturating_add(active.plan.assembly_timeout())
            },
            |package| package.preparation_deadline,
        ))
    }

    /// Immutable admission tick for resource-priority comparisons. The public
    /// operation's timeout clock may restart when reconnaissance becomes an
    /// assault, but its place in the commitment order does not.
    pub(super) fn air_admitted_at(&self) -> Option<Tick> {
        self.air.as_ref().map(|active| active.plan.admitted_at())
    }

    /// Connected-package evidence for opt-in decision traces.
    pub(super) fn connected_package_diagnostics(&self) -> Option<ConnectedPackageDiagnostics> {
        let active = self.air.as_ref()?;
        let package = active.plan.package()?;
        Some(ConnectedPackageDiagnostics {
            admitted_at: active.plan.admitted_at(),
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
        self.air
            .as_ref()
            .map(|active| active.plan.assembly_timeout())
    }

    #[cfg(test)]
    fn air_op_mut(&mut self) -> Option<&mut AirOperation> {
        self.air.as_mut().map(|active| &mut active.op)
    }

    #[cfg(test)]
    fn air_plan_mut(&mut self) -> Option<&mut AirPlan> {
        self.air.as_mut().map(|active| &mut active.plan)
    }

    pub(super) fn terminal_outcome(&self) -> Option<AirOperationOutcome> {
        self.terminal_outcome
    }

    pub(crate) fn owned_units(&self) -> impl Iterator<Item = UnitId> + '_ {
        self.air
            .iter()
            .flat_map(|active| {
                active
                    .op
                    .scout
                    .into_iter()
                    .chain(active.op.artillery.iter().copied())
                    .chain(active.op.strike_aircraft.iter().copied())
                    .chain(active.plan.screen().iter().copied())
            })
            .chain(self.standby.scout)
            .chain(self.standby.artillery.iter().copied())
            .chain(self.standby.strike_aircraft.iter().copied())
    }

    pub(crate) fn observe_operation(
        &mut self,
        profile: &ResolvedProfile,
        obs: &Observation,
        intel: &StrategicIntelligence,
    ) {
        self.standby.prune(obs);
        if let Some(active) = &mut self.air {
            refresh_target(&mut active.op, intel);
            if active.op.phase() != AirOperationPhase::Recover {
                abort_if_needed(&mut active.op, &active.plan, profile, obs, intel);
            }
        }
    }

    pub(crate) fn has_active_island_operation(&self) -> bool {
        self.air.as_ref().is_some_and(|active| {
            active.op.assault_admitted() && matches!(active.plan, AirPlan::Island(_))
        })
    }

    pub(crate) fn prepare_active_island(
        &self,
        context: StrategicThinkContext<'_>,
    ) -> Option<IslandPreparation> {
        let active = self.air.as_ref()?;
        let AirPlan::Island(island) = &active.plan else {
            return None;
        };
        if !active.op.assault_admitted() {
            return None;
        }
        let op = &active.op;
        let plan = &active.plan;
        let obs = context.obs;
        let mut membership = AirMembership::from_active(active);
        let mut purchases = ProductionPlan::default();
        if op.phase() <= AirOperationPhase::Assemble {
            let unavailable =
                excluding_owned(context.coordination.enlisted, &reservations(op, plan, obs));
            let scout = Role::Scout.unit_for(obs.faction);
            membership.scout = membership
                .scout
                .filter(|id| {
                    unit(obs, *id).is_some_and(|member| member.kind == scout)
                        && !unavailable.contains(id)
                })
                .or_else(|| available(obs, &unavailable, |kind| kind == scout).next());
            assign_artillery(&mut membership.artillery, plan, obs, &unavailable);
            assign_strike_aircraft(&mut membership.strike_aircraft, plan, obs, &unavailable);
            assign_exact(
                &mut membership.screen,
                island.desired_screen,
                obs,
                &unavailable,
                |kind| kind == Role::AirGround.unit_for(obs.faction),
            );
            let planning = AirPlanningContext {
                allow_procurement: context.coordination.allow_new_operation,
                planning: context.coordination.planning,
                tuning: context.tuning,
                obs,
                intel: context.intel,
                home: context.home,
                orientation: context.coordination.orientation,
                public_map: context.coordination.public_map,
                enlisted: &unavailable,
                landing_sites: &[],
                connected_resources: None,
                production: context.production,
                protected_current_scrap: context.coordination.protected_current_scrap,
                protected_forecast_scrap: context.coordination.protected_forecast_scrap,
            };
            let demands = missing_island_members(
                AirRoster {
                    scout: membership.scout,
                    artillery: &membership.artillery,
                    strike_aircraft: &membership.strike_aircraft,
                },
                membership.screen.len(),
                island,
                &planning,
                scout,
            );
            purchases = schedule(&planning, &demands);
        }
        Some(IslandPreparation {
            membership,
            purchases,
        })
    }

    /// Proposes one exact common-minimum connected assault without mutating
    /// planner state. Island admission and an already-admitted assault remain
    /// on the ordinary lifecycle path.
    pub(crate) fn fresh_connected_minimum_proposal(
        &self,
        experience: &super::experience::Experience,
        request: FreshConnectedProposalRequest<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let FreshConnectedProposalRequest {
            unavailable_paid,
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
            if active.op.assault_admitted() {
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
                    minimum_only: false,
                    campaign_routes: None,
                    unavailable_paid,
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
        let mut current = select_target_candidates(intel, obs.tick, tuning.tactical_memory)
            .into_iter()
            .filter(|target| target.evidence == ContactEvidence::Current)
            .collect::<Vec<_>>();
        current.sort_unstable_by_key(|target| {
            let context = super::experience::ExperienceKey {
                doctrine: super::experience::Doctrine::Air,
                x: target.anchor.x,
                y: target.anchor.y,
                subject: super::experience::ExperienceSubject::Building(target.id),
            };
            let preference = (1024 + i32::from(experience.score(context)) / 2) as u64;
            (
                Reverse(u64::from(building_value(target.kind)) * preference),
                Reverse(target.confidence_at(obs.tick)),
                target.anchor.y,
                target.anchor.x,
                target.player,
                target.kind,
            )
        });
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
            standby: self.standby.clone(),
        };
        let mut first_rejection = None;
        let campaign_routes = CampaignRoutes::new(
            obs,
            intel,
            coordination.public_map,
            coordination.orientation,
        );
        for target in current {
            match derive_fresh_connected_proposal(
                FreshConnectedDerivationContext {
                    minimum_only: false,
                    campaign_routes: Some(&campaign_routes),
                    unavailable_paid,
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
    pub(crate) fn active_connected_revision_proposal(
        &self,
        request: FreshConnectedProposalRequest<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let FreshConnectedProposalRequest {
            unavailable_paid,
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
        let AirPlan::Connected(connected) = &active.plan else {
            return Ok(None);
        };
        let package = &connected.package;
        if !active.op.assault_admitted()
            || active.op.phase() > AirOperationPhase::Assemble
            || active.op.membership_frozen_at.is_some()
            || package.derived_at >= obs.tick
            || operation_recovery_reason(&active.op, &active.plan, profile, obs, intel).is_some()
            || obs.tick > package.preparation_deadline
        {
            return Ok(None);
        }
        let Some(target) = current_package_revision_target(&active.op, &active.plan, intel) else {
            return Ok(None);
        };
        let owned = reservations(&active.op, &active.plan, obs);
        let unavailable = excluding_owned(coordination.enlisted, &owned);
        let campaign_routes = CampaignRoutes::new(
            obs,
            intel,
            coordination.public_map,
            coordination.orientation,
        );
        let route = ConnectedRouteContext {
            campaign_routes: Some(&campaign_routes),
            unavailable_paid,
            intel,
            home,
            target: target.anchor,
            public_map: coordination.public_map,
            orientation: coordination.orientation,
        };
        let initial_resources = if target.anchor == connected.scope {
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
        let proposal = derive_connected_proposal_with_resources(
            FreshConnectedDerivationContext {
                minimum_only: false,
                campaign_routes: Some(&campaign_routes),
                unavailable_paid,
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
                op: active.op.clone(),
                plan: connected.clone(),
            },
            initial_resources,
            package.preparation_deadline,
        )
        .map_err(|reason| RejectedConnectedCandidate {
            target: target.clone(),
            reason,
        })?;
        Ok(Some(proposal))
    }

    /// Moves an admitted connected operation into bounded recovery after its
    /// current revision can no longer field the shared minimum.
    pub(crate) fn reject_active_connected_revision(
        &mut self,
        rejection: ConnectedPlanRejection,
        observed_at: Tick,
    ) {
        if rejection.is_deferred() {
            return;
        }
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
    pub(crate) fn commit_connected(&mut self, proposal: FreshConnectedProposal) {
        let revises_active = proposal.revises_active_operation();
        let paid = match &proposal.origin {
            ConnectedProposalOrigin::Active { plan, .. } => plan.paid_production.clone(),
            _ => Vec::new(),
        };
        let mut selected = proposal
            .variants
            .into_iter()
            .nth(proposal.selected_variant)
            .expect("a selected proposal variant came from its retained ladder");
        selected.plan.paid_production = paid;
        self.air = Some(selected.into_active());
        if !revises_active {
            self.standby = AirStandby::default();
        }
        self.terminal_outcome = None;
    }

    /// Reconstructs unpaid demand from the retained package and current inventory.
    pub(crate) fn active_connected_obligation(
        &self,
        request: FreshConnectedProposalRequest<'_>,
    ) -> Option<ActiveConnectedObligation> {
        let active = self
            .air
            .as_ref()
            .filter(|active| active.op.assault_admitted())?;
        let package = active.plan.package()?;
        let mut membership = AirMembership::from_active(active);
        let obs = request.obs;
        let provider_jobs = if active.op.phase() <= AirOperationPhase::Assemble
            && obs.tick < package.preparation_deadline
            && operation_recovery_reason(
                &active.op,
                &active.plan,
                request.profile,
                obs,
                request.intel,
            )
            .is_none()
        {
            let resources =
                ConnectedProductionResources::from_package_snapshot_after_current_reserve(
                    obs,
                    active.op.target_player,
                    package,
                    ConnectedRouteContext {
                        campaign_routes: None,
                        unavailable_paid: request.unavailable_paid,
                        intel: request.intel,
                        home: request.home,
                        target: active.op.target,
                        public_map: request.coordination.public_map,
                        orientation: request.coordination.orientation,
                    },
                    request.resource_snapshot,
                    0,
                );
            if active.op.phase() <= AirOperationPhase::Assemble
                && active.op.membership_frozen_at.is_none()
            {
                let owned = reservations(&active.op, &active.plan, obs);
                let unavailable = excluding_owned(request.coordination.enlisted, &owned);
                let mut unavailable = connected_provider_unavailable(
                    obs,
                    &resources.targets,
                    &unavailable,
                    ConnectedRouteContext {
                        campaign_routes: None,
                        unavailable_paid: request.unavailable_paid,
                        intel: request.intel,
                        home: request.home,
                        target: active.op.target,
                        public_map: request.coordination.public_map,
                        orientation: request.coordination.orientation,
                    },
                );
                unavailable.retain(|id| !owned.contains(id));
                if membership.scout.is_none() && active.op.scout_dispatch.is_none() {
                    let mut scouts = Vec::new();
                    assign_provider_demands(&mut scouts, &package.recon, obs, &unavailable);
                    membership.scout = scouts.into_iter().next();
                }
                assign_artillery(&mut membership.artillery, &active.plan, obs, &unavailable);
                assign_strike_aircraft(
                    &mut membership.strike_aircraft,
                    &active.plan,
                    obs,
                    &unavailable,
                );
            }
            missing_package_demands(
                package,
                AirRoster {
                    scout: membership.scout,
                    artillery: &membership.artillery,
                    strike_aircraft: &membership.strike_aircraft,
                },
                obs,
                &resources.snapshot,
                package.preparation_deadline,
                &resources.access,
            )
            .into_iter()
            .flat_map(|demand| {
                let job = ConnectedProviderJob {
                    kind: demand.kind,
                    enqueue_not_before: obs.tick,
                    ready_before: package.preparation_deadline,
                    eligible_producers: eligible_connected_producers(
                        &resources,
                        demand.kind,
                        package.preparation_deadline,
                    ),
                };
                std::iter::repeat_n(job, demand.count)
            })
            .collect()
        } else {
            Vec::new()
        };
        Some(ActiveConnectedObligation {
            identity: ConnectedOffenseIdentity::new(active.op.target_id?, active.op.target),
            accepted_at: active.plan.admitted_at(),
            deadline: package.preparation_deadline,
            units: membership.units(obs),
            membership,
            provider_jobs,
        })
    }

    pub(crate) fn reconnaissance_paid_claims(
        &self,
        obs: &Observation,
        resources: &ResourceSnapshot,
        unavailable: &[(BuildingId, UnitKind, usize)],
    ) -> Vec<super::allocation::PaidQueueClaim> {
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
                        (ready_at < deadline
                            && !unavailable.contains(&(lane.producer, kind, occurrence)))
                        .then_some((ready_at, lane.producer, occurrence))
                    })
            })
            .min()
            .map(
                |(_, producer, occurrence)| super::allocation::PaidQueueClaim {
                    producer,
                    kind,
                    occurrence,
                },
            )
            .into_iter()
            .collect()
    }

    /// Paid purchases still supplying the retained force. Completed history
    /// leaves the ledger so it cannot capture later ordinary queue items.
    pub(crate) fn paid_connected_production(
        &mut self,
        obs: &Observation,
    ) -> Vec<ConnectedPurchase> {
        let Some(active) = self.air.as_mut().filter(|active| {
            active.op.assault_admitted() && active.op.phase() <= AirOperationPhase::Assemble
        }) else {
            return Vec::new();
        };
        let mut missing = connected_provider_shortfall(active, obs);
        let mut queued = observed_queue_multiplicity(obs);
        let AirPlan::Connected(plan) = &mut active.plan else {
            return Vec::new();
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
        plan.paid_production.clone()
    }

    /// Releases unpaid connected demand during emergency economy recovery
    /// and returns every still-routable member immediately.
    pub(crate) fn recover_unpaid_connected_for_economy_emergency(
        &mut self,
        context: EconomyEmergencyRecovery<'_>,
    ) -> Option<StrategicDecision> {
        let EconomyEmergencyRecovery {
            profile,
            tuning,
            obs,
            home,
            public_map,
            orientation,
            recon_paid_exclusions,
        } = context;
        // Prunes completed purchases so they cannot claim later ordinary queue
        // work. This path returns before the normal pass would do it.
        self.paid_connected_production(obs);
        let has_unpaid_provider = self.air.as_ref().is_some_and(|active| {
            if active.op.phase() > AirOperationPhase::Assemble {
                return false;
            }
            let Some(package) = active.plan.package() else {
                return false;
            };
            let snapshot = ResourceSnapshot::from_observation(obs);
            let access = emergency_paid_queue_access(obs, recon_paid_exclusions);
            // Demand a paid queue can still deliver costs no further scrap,
            // whichever program bought it. The obligation path credits the same
            // occurrences, so recalling the operation over them would abort an
            // attack that needs no money.
            connected_provider_shortfall(active, obs)
                .into_iter()
                .any(|(kind, count)| {
                    count
                        > count_paid_queued_ready_with_access(
                            &snapshot,
                            kind,
                            package.preparation_deadline,
                            &access,
                        )
                })
        });
        if !has_unpaid_provider {
            return None;
        }

        let ActiveAirOperation { mut op, mut plan } = self
            .air
            .take()
            .expect("unpaid connected demand belongs to one active operation");
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
    pub(crate) fn recover_unfundable_active_connected(&mut self, observed_at: Tick) {
        let active = self
            .air
            .as_mut()
            .expect("an active connected obligation can only come from its planner");
        debug_assert!(active.op.assault_admitted());
        debug_assert!(matches!(active.plan, AirPlan::Connected(_)));
        recover(
            &mut active.op,
            AirRecoveryReason::PreparationInfeasible,
            observed_at,
        );
    }

    /// Only purchases emitted now cross from forecast evidence into ownership.
    pub(crate) fn record_connected_purchases(
        &mut self,
        schedule: &[crate::allocation::ScheduledProducerJob],
        observed_at: Tick,
    ) {
        use crate::allocation::{ClaimOwner, ObligationKey, ProposalKey};
        let Some(ActiveAirOperation {
            op,
            plan: AirPlan::Connected(plan),
        }) = self.air.as_mut()
        else {
            return;
        };
        for job in schedule.iter().filter(|job| job.enqueued_at == observed_at) {
            let identity = match job.owner {
                ClaimOwner::Proposal(ProposalKey::ConnectedOffenseMinimum(key)) => {
                    (key.objective, key.anchor)
                }
                ClaimOwner::Obligation {
                    key: ObligationKey::ConnectedOffense { objective, anchor },
                    ..
                } => (objective, anchor),
                _ => continue,
            };
            if (op.target_id, op.target) == (Some(identity.0), identity.1) {
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
    pub(super) fn remaining_airwork_ticks(
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

    /// Runs the ordinary tactical lifecycle after the coordinator has already
    /// accepted or rejected the fresh connected-offense proposal for this
    /// observation. Island and remembered reconnaissance behavior is unchanged.
    pub(crate) fn think_after_connected_adjudication(
        &mut self,
        context: StrategicThinkContext<'_>,
    ) -> StrategicThinkResult {
        self.think_after_connected_adjudication_inner(context)
    }

    /// Returns the exact remembered reconnaissance target that the ordinary
    /// post-allocation lifecycle will retain or admit on this observation.
    /// This preview is read-only so shared allocation can preserve capital
    /// needed by the immediately following Lift handoff.
    pub(crate) fn prospective_recon_target<'a>(
        &self,
        context: StrategicThinkContext<'a>,
    ) -> Option<&'a BuildingContact> {
        let StrategicThinkContext {
            profile,
            tuning,
            obs,
            intel,
            home,
            coordination,
            production,
            ..
        } = context;
        if intel.observed_at() != Some(obs.tick) || !coordination.allow_new_operation {
            return None;
        }
        let active = if let Some(active) = self.air.as_ref() {
            if active.op.phase() != AirOperationPhase::Recon || active.op.assault_admitted() {
                return None;
            }
            active.clone()
        } else {
            if obs.tick < self.cooldown_until || !strategic_admission_tick(obs.tick) {
                return None;
            }
            let selected = select_fresh_air_target(
                profile,
                tuning,
                obs,
                intel,
                home,
                coordination.lift_support,
                coordination.public_map,
            )?;
            let mut standby = self.standby.clone();
            standby.prune(obs);
            fresh_air_operation(profile, obs, production, selected, standby)
        };
        let ActiveAirOperation { mut op, plan } = active;
        refresh_target(&mut op, intel);
        let target = intel.buildings().iter().find(|target| {
            target.player == op.target_player
                && target.anchor == op.target
                && target.evidence == ContactEvidence::Remembered
        })?;
        if operation_recovery_reason(&op, &plan, profile, obs, intel).is_some() {
            return None;
        }
        let owned = reservations(&op, &plan, obs);
        let unavailable = excluding_owned(coordination.enlisted, &owned);
        op.scout = remembered_recon_scout(&op, obs, &unavailable);
        let landing_sites: Vec<_> = coordination
            .lift_support
            .filter(|request| request.player == op.target_player && request.target == op.target)
            .map_or_else(Vec::new, |request| request.planned_drops.clone());
        remembered_recon_route_is_viable(
            &op,
            &plan,
            obs,
            intel,
            &landing_sites,
            connected_public_map(&plan, coordination.public_map),
        )
        .then_some(target)
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
            owned_only,
            claimed_elsewhere,
        } = context;
        let StrategicCoordination {
            planning,
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
            let Some(selected) = select_fresh_air_target(
                profile,
                tuning,
                obs,
                intel,
                home,
                lift_support,
                public_map,
            ) else {
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
            self.air = Some(fresh_air_operation(
                profile, obs, production, selected, standby,
            ));
        }
        let Some(ActiveAirOperation { mut op, mut plan }) = self.air.take() else {
            return StrategicThinkResult::default();
        };
        use super::experience::{
            Doctrine, EpisodeId, EpisodeOwner, ExperienceKey, ExperienceSubject, Outcome,
            OutcomeReason,
        };
        let members: Vec<_> = op
            .scout
            .into_iter()
            .chain(op.artillery.iter().copied())
            .chain(op.strike_aircraft.iter().copied())
            .chain(plan.screen().iter().copied())
            .collect();
        self.outcomes.watch(
            obs,
            EpisodeId {
                owner: EpisodeOwner::Air,
                serial: plan.admitted_at(),
            },
            ExperienceKey {
                doctrine: Doctrine::Air,
                y: op.target.y,
                x: op.target.x,
                subject: ExperienceSubject::Building(op.target_id),
            },
            &members,
            op.phase() as u8,
        );
        let objective_gone = op
            .target_id
            .is_some_and(|id| self.outcomes.observe_objective(obs, id));
        if let Some(screen) = plan.screen_mut() {
            screen.retain(|id| {
                unit(obs, *id)
                    .is_some_and(|member| member.kind == Role::AirGround.unit_for(obs.faction))
            });
        }
        if reservations(&op, &plan, obs)
            .iter()
            .any(|id| claimed_elsewhere.contains(id))
        {
            let decision = StrategicDecision {
                reservations: reservations(&op, &plan, obs),
                ..Default::default()
            };
            self.air = Some(ActiveAirOperation { op, plan });
            return StrategicThinkResult::from_decision(decision);
        }
        let began_in_recovery = op.phase() == AirOperationPhase::Recover;
        refresh_target(&mut op, intel);
        if !op.assault_admitted()
            && strategic_admission_tick(obs.tick)
            && let Some(current_target) = current_target_contact(&op, intel)
        {
            let admitted_at = plan.admitted_at();
            if wealthy_island_target(profile, obs, home, current_target, public_map) {
                let mut admitted = IslandPlan::new(profile, obs, production);
                op.admit_assault(obs.tick);
                admitted.admitted_at = admitted_at;
                plan = AirPlan::Island(admitted);
            }
        }
        if op.assault_admitted()
            && op.phase() <= AirOperationPhase::Assemble
            && let AirPlan::Connected(connected) = &mut plan
            && connected.package.derived_at < obs.tick
            && obs.tick <= connected.package.preparation_deadline
            && let Some(current_target) = connected_revision_target(&op, &connected.package, intel)
        {
            let resources = connected_resources.get_or_insert_with(|| {
                ConnectedProductionResources::from_revision_after_current_reserve(
                    obs,
                    current_target,
                    &connected.package,
                    ConnectedRouteContext {
                        campaign_routes: None,
                        unavailable_paid: production.unavailable_paid,
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
                connected,
                current_target,
                &resources.targets.target_anchors,
            );
        }
        if !began_in_recovery && op.phase() != AirOperationPhase::Recover {
            abort_if_needed(&mut op, &plan, profile, obs, intel);
        }

        let mut out = StrategicDecision::default();
        let landing_sites: Vec<_> = lift_support
            .filter(|request| request.player == op.target_player && request.target == op.target)
            .map_or_else(Vec::new, |request| request.planned_drops.clone());
        let owned = reservations(&op, &plan, obs);
        let mut external_enlisted = excluding_owned(enlisted, &owned);
        if owned_only {
            external_enlisted.extend(
                obs.my_units
                    .iter()
                    .filter_map(|unit| (!owned.contains(&unit.id)).then_some(unit.id)),
            );
            external_enlisted.sort_unstable();
            external_enlisted.dedup();
        }
        if let Some(package) = plan.package()
            && op.phase() <= AirOperationPhase::Assemble
        {
            let route = ConnectedRouteContext {
                campaign_routes: None,
                unavailable_paid: production.unavailable_paid,
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
            allow_procurement: allow_new_operation,
            planning,
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
        match op.stage {
            AirStage::Watching => remembered_recon(&mut op, &plan, &context, &mut out),
            AirStage::Recon => recon(&mut op, &mut plan, &context, &mut out),
            AirStage::Assemble => assemble(&mut op, &mut plan, &context, &mut out),
            AirStage::SuppressAa => suppress(&mut op, &mut plan, &context, &mut out),
            AirStage::Verify => verify(&mut op, &mut plan, &context, &mut out),
            AirStage::Strike => strike(&mut op, &mut plan, &context, &mut out),
            AirStage::Recover { .. } => {}
        }
        let preparation_expired = plan.package().is_some_and(|package| {
            op.phase() <= AirOperationPhase::Assemble
                && obs.tick >= package.preparation_deadline
                && !assembly_complete(&op, &plan)
        });
        if preparation_expired {
            out.intents.clear();
            out.reserved_scrap = 0;
            recover(&mut op, AirRecoveryReason::Timeout, obs.tick);
        }
        if !allow_new_operation {
            out.intents
                .retain(|intent| !matches!(intent, Intent::TrainAt { .. }));
            out.reserved_scrap = 0;
        }
        let recovery_entered_this_tick = op.phase() == AirOperationPhase::Recover
            && (!began_in_recovery || op.phase_started_at == obs.tick);
        if op.phase() == AirOperationPhase::Recover {
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
        let recovered = op.phase() == AirOperationPhase::Recover
            && (out.reservations.is_empty()
                || settled
                || elapsed(op.phase_started_at, obs.tick) >= 500);
        if let Some(reason) = op.recovery_reason() {
            let (outcome, reason, confidence, doctrine) = match reason {
                AirRecoveryReason::Complete if objective_gone => (
                    Outcome::Complete,
                    OutcomeReason::ObjectiveObservedGone,
                    750,
                    false,
                ),
                AirRecoveryReason::RequiredUnitLost => (
                    Outcome::Ineffective,
                    OutcomeReason::RequiredUnitLost,
                    1000,
                    op.membership_frozen_at.is_some(),
                ),
                AirRecoveryReason::NewAirDefense => (
                    Outcome::Aborted,
                    OutcomeReason::ObservedCounter,
                    1000,
                    false,
                ),
                AirRecoveryReason::Timeout
                    if op.membership_frozen_at.is_none()
                        && !self.outcomes.has_progress()
                        && self.outcomes.own_lost_value(obs) == 0 =>
                {
                    (Outcome::Aborted, OutcomeReason::Deadline, 1000, false)
                }
                AirRecoveryReason::Timeout => {
                    (Outcome::Ineffective, OutcomeReason::Deadline, 750, false)
                }
                AirRecoveryReason::Complete
                | AirRecoveryReason::ObjectiveLost
                | AirRecoveryReason::StaleIntelligence => {
                    (Outcome::Inconclusive, OutcomeReason::LostContact, 0, false)
                }
                AirRecoveryReason::UnreachableStaging | AirRecoveryReason::UnreachableAirRoute => {
                    (Outcome::Aborted, OutcomeReason::BlockedRoute, 1000, false)
                }
                AirRecoveryReason::PreparationInfeasible => {
                    (Outcome::Invalidated, OutcomeReason::Preempted, 1000, false)
                }
            };
            self.outcomes
                .finish(obs, outcome, reason, confidence, doctrine);
        }
        if settled && !out.reservations.is_empty() && reusable_survivors(op.recovery_reason()) {
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

/// Same-observation inputs for one emergency economy recovery decision.
pub(crate) struct EconomyEmergencyRecovery<'a> {
    pub(crate) profile: &'a ResolvedProfile,
    pub(crate) tuning: DifficultyTuning,
    pub(crate) obs: &'a Observation,
    pub(crate) home: TilePos,
    pub(crate) public_map: Option<&'a PublicMapBriefing>,
    pub(crate) orientation: Orientation,
    /// Queue occurrences the reconnaissance program already holds. This path
    /// returns before shared allocation runs, so it receives them directly
    /// instead of reading an obligation view.
    pub(crate) recon_paid_exclusions: &'a [(BuildingId, UnitKind, usize)],
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
    let returning = if matches!(plan, AirPlan::Connected(_)) {
        connected_public_map(plan, public_map).map_or_else(
            || {
                routing::routable_command_subset_with_orientation(
                    crate::query_work::QueryPurpose::AirOperation,
                    obs,
                    &survivors,
                    home,
                    orientation,
                )
            },
            |map| {
                routing::routable_command_subset_with_public_terrain_and_orientation(
                    crate::query_work::QueryPurpose::AirOperation,
                    obs,
                    map,
                    &survivors,
                    home,
                    orientation,
                )
            },
        )
    } else {
        routing::routable_command_subset(
            crate::query_work::QueryPurpose::AirOperation,
            obs,
            &survivors,
            home,
        )
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
    if op.recovery_reason() == Some(AirRecoveryReason::Complete) {
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
    op.scout = remembered_recon_scout(op, obs, context.enlisted);
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
        &[(
            scout_kind,
            usize::from(op.scout.is_none())
                .saturating_sub(unowned_queued_scouts(context, scout_kind)),
        )],
    )
    .append_to(out);
}

fn unowned_queued_scouts(context: &AirPlanningContext<'_>, scout: UnitKind) -> usize {
    let prior = context
        .production
        .prior_intents
        .iter()
        .filter(|intent| matches!(intent, Intent::TrainAt { kind, .. } if *kind == scout))
        .count();
    let unavailable = context
        .production
        .unavailable_paid
        .iter()
        .filter(|(_, kind, _)| *kind == scout)
        .count();
    queued(context.obs, |kind| kind == scout)
        .saturating_add(prior)
        .saturating_sub(unavailable)
}

fn remembered_recon_scout(
    op: &AirOperation,
    obs: &Observation,
    enlisted: &[UnitId],
) -> Option<UnitId> {
    let scout_kind = Role::Scout.unit_for(obs.faction);
    op.scout
        .filter(|id| unit(obs, *id).is_some())
        .or_else(|| available(obs, enlisted, |kind| kind == scout_kind).next())
}

fn remembered_recon_route_is_viable(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    scout_dispatch_goal(op, plan, obs, intel, landing_sites, public_map)
        .is_some_and(|goal| scout_dispatch_is_viable(op, obs, goal, public_map))
}

fn reconcile_preparation_members(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
) -> bool {
    let obs = context.obs;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let route_unavailable = if let Some(resources) = context.connected_resources.as_ref() {
        connected_provider_unavailable(
            obs,
            &resources.targets,
            &[],
            ConnectedRouteContext {
                campaign_routes: None,
                unavailable_paid: &[],
                intel: context.intel,
                home: context.home,
                target: op.target,
                public_map: context.public_map,
                orientation: context.orientation,
            },
        )
    } else {
        Vec::new()
    };
    let unavailable = merged_unavailable(context.enlisted, &route_unavailable);
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
        if op.scout.is_some() && op.phase() == AirOperationPhase::Recon {
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
            && op.strike_aircraft.len() < plan.desired_strike_aircraft()
    {
        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        return false;
    }
    if previous_artillery
        .iter()
        .any(|id| unit(obs, *id).is_some() && route_unavailable.binary_search(id).is_ok())
        && op.artillery.len() < plan.desired_artillery()
    {
        recover(op, AirRecoveryReason::UnreachableStaging, obs.tick);
        return false;
    }
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    let desired_screen = plan.desired_screen();
    if let Some(screen) = plan.screen_mut() {
        assign_exact(screen, desired_screen, obs, context.enlisted, |kind| {
            kind == screen_kind
        });
    }
    if connected_package_is_proven_infeasible(op, plan, context) {
        recover(op, AirRecoveryReason::PreparationInfeasible, obs.tick);
        return false;
    }
    true
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
        landing_sites,
        ..
    } = context;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let public_map = connected_public_map(plan, context.public_map);
    if !reconcile_preparation_members(op, plan, context) {
        return;
    }
    if !dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out) {
        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        return;
    }
    schedule_missing_members(op, plan, context, scout_kind, out);
    if matches!(plan, AirPlan::Connected(_)) {
        hold_strike_aircraft(op, obs, context.home, out);
    }
    if op.scout_dispatch.is_some()
        && target_seen(op, plan, obs)
        && elapsed(op.phase_started_at, obs.tick) >= tuning.reaction_delay
    {
        enter(op, AirStage::Assemble, obs.tick);
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
        landing_sites,
        ..
    } = context;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let public_map = connected_public_map(plan, context.public_map);
    if !reconcile_preparation_members(op, plan, context) {
        return;
    }
    schedule_missing_members(op, plan, context, scout_kind, out);
    let complete = assembly_complete(op, plan);
    if matches!(plan, AirPlan::Connected(_)) && !complete {
        hold_strike_aircraft(op, obs, *home, out);
    }
    if complete {
        if plan.airborne() {
            if !dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out) {
                recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                return;
            }
            enter(op, AirStage::SuppressAa, obs.tick);
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
        enter(op, AirStage::SuppressAa, obs.tick);
        stage_artillery(op, staging, out);
        hold_strike_aircraft(op, obs, *home, out);
    }
}

fn assembly_complete(op: &AirOperation, plan: &AirPlan) -> bool {
    op.scout.is_some()
        && op.artillery.len() == plan.desired_artillery()
        && op.strike_aircraft.len() == plan.desired_strike_aircraft()
        && plan.screen().len() == plan.desired_screen()
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
                let changed = plan.suppression_dispatch() != Some(&dispatch);
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
                plan.set_suppression_dispatch(Some(dispatch));
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
                if plan.suppression_dispatch() != Some(&dispatch) || repeat_refused_ground_order {
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
                    plan.set_strike_dispatch(None);
                } else {
                    op.artillery_staging = None;
                }
                plan.set_suppression_dispatch(Some(dispatch));
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
        plan.set_suppression_dispatch(None);
        if plan.airborne() {
            match airborne_corridor_status(op, plan, obs, intel, home, landing_sites) {
                AirborneCorridorStatus::Defended => {
                    recover(op, AirRecoveryReason::NewAirDefense, obs.tick);
                }
                AirborneCorridorStatus::Clear => {
                    enter(op, AirStage::Verify, obs.tick);
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
                enter(op, AirStage::Verify, obs.tick);
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
        enter(op, AirStage::SuppressAa, obs.tick);
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
                enter(op, AirStage::Strike, obs.tick);
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
            enter(op, AirStage::Strike, obs.tick);
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
        enter(op, AirStage::SuppressAa, obs.tick);
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
        let air_routes =
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
    let units = if plan.strike_dispatch() == Some(dispatch) {
        attackers
            .iter()
            .copied()
            .filter(|id| unit(obs, *id).is_some_and(|member| member.idle))
            .collect()
    } else {
        attackers.to_vec()
    };
    plan.set_strike_dispatch(Some(dispatch));
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
    if let Some(reason) = operation_recovery_reason(op, plan, profile, obs, intel) {
        recover(op, reason, obs.tick);
    }
}

fn operation_recovery_reason(
    op: &AirOperation,
    plan: &AirPlan,
    profile: &ResolvedProfile,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> Option<AirRecoveryReason> {
    if op.phase() <= AirOperationPhase::Assemble
        && op
            .scout_dispatch
            .is_some_and(|(scout, _)| unit(obs, scout).is_none())
    {
        return Some(AirRecoveryReason::RequiredUnitLost);
    }
    let waiting_for_recon_scout =
        op.phase() == AirOperationPhase::Recon && op.scout.is_none_or(|id| unit(obs, id).is_none());
    let connected_preparation =
        matches!(plan, AirPlan::Connected(_)) && op.phase() <= AirOperationPhase::Assemble;
    if elapsed(op.started_at, obs.tick) >= operation_timeout(profile, plan)
        || (!connected_preparation
            && !waiting_for_recon_scout
            && elapsed(op.phase_started_at, obs.tick) >= phase_timeout(op.phase(), plan))
    {
        return Some(AirRecoveryReason::Timeout);
    }
    if !plan.airborne()
        && op.phase() < AirOperationPhase::Strike
        && operation_objective_is_stale(op, plan, obs.tick, intel)
    {
        return Some(AirRecoveryReason::StaleIntelligence);
    }
    let lost_required_force = match plan {
        AirPlan::Connected(connected) => {
            let package = &connected.package;
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
        }
        AirPlan::Island(island) => {
            let live_strike_aircraft = op
                .strike_aircraft
                .iter()
                .filter(|id| unit(obs, **id).is_some())
                .count();
            live_strike_aircraft < island.desired_strike_aircraft.div_ceil(2).max(1)
        }
        AirPlan::Reacquire(_) => false,
    };
    if op.membership_frozen_at.is_some()
        && (op.scout.is_some_and(|id| unit(obs, id).is_none()) || lost_required_force)
    {
        return Some(AirRecoveryReason::RequiredUnitLost);
    }
    if op.phase() < AirOperationPhase::Strike && operation_objective_cleared(op, plan, obs, intel) {
        return Some(AirRecoveryReason::ObjectiveLost);
    }
    None
}

/// Schedules in demand order, spreading equal-load work across producers. If
/// the next otherwise-trainable member is unaffordable or all its queues are
/// full, the available bank is claimed so ordinary production cannot skim it.
fn schedule(context: &AirPlanningContext<'_>, demands: &[(UnitKind, usize)]) -> ProductionPlan {
    let mut out = ProductionPlan::default();
    if !context.allow_procurement {
        return out;
    }
    let obs = context.obs;
    let mut bank = obs.scrap.saturating_sub(context.protected_current_scrap);
    let mut production = super::production::ImmediateProduction::new(
        obs,
        context.production.lane_reservations,
        context.production.prior_intents,
    );
    'demands: for &(kind, count) in demands {
        if !requirements_met(obs, kind) || !has_producer(obs, kind) {
            continue;
        }
        for _ in 0..count {
            let cost = kind.stats().cost;
            let producer = production
                .available(kind, |building| {
                    if building == BuildingKind::Airworks {
                        STRATEGIC_AIR_QUEUE_DEPTH
                    } else {
                        QUEUE_CAP
                    }
                })
                .min_by_key(|producer| (producer.depth, producer.id));
            if bank < cost || producer.is_none() {
                out.reserved_scrap = out.reserved_scrap.saturating_add(bank.min(cost));
                break 'demands;
            }
            let Some(producer) = producer else {
                break 'demands;
            };
            bank -= cost;
            out.purchases.push(production.append_purchase(producer));
        }
    }
    out
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreshAirTarget<'a> {
    Island(&'a BuildingContact),
    Remembered(&'a BuildingContact),
}

fn fresh_air_operation(
    profile: &ResolvedProfile,
    obs: &Observation,
    production: StrategicProductionContext<'_>,
    selected: FreshAirTarget<'_>,
    standby: AirStandby,
) -> ActiveAirOperation {
    let (target, plan) = match selected {
        FreshAirTarget::Island(target) => {
            let island = IslandPlan::new(profile, obs, production);
            let plan = if target.evidence == ContactEvidence::Current {
                AirPlan::Island(island)
            } else {
                AirPlan::Reacquire(ReacquirePlan::severed(&island))
            };
            (target, plan)
        }
        FreshAirTarget::Remembered(target) => (target, AirPlan::remembered_connected(obs)),
    };
    let assault_admitted = !matches!(plan, AirPlan::Reacquire(_));
    let op = AirOperation {
        target_player: target.player,
        target_kind: target.kind,
        target: target.anchor,
        target_id: target.id,
        stage: if assault_admitted {
            AirStage::Recon
        } else {
            AirStage::Watching
        },
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
    };
    ActiveAirOperation { op, plan }
}

fn select_fresh_air_target<'a>(
    profile: &ResolvedProfile,
    tuning: DifficultyTuning,
    obs: &Observation,
    intel: &'a StrategicIntelligence,
    home: TilePos,
    lift_support: Option<&LiftSupportRequest>,
    public_map: Option<&PublicMapBriefing>,
) -> Option<FreshAirTarget<'a>> {
    let island_target = if let Some(request) = lift_support {
        exact_wealthy_island_target(profile, obs, home, intel, request, public_map)
    } else {
        select_wealthy_island_target(profile, obs, home, intel, public_map)
    };
    if let Some(target) = island_target {
        return Some(FreshAirTarget::Island(target));
    }
    if lift_support.is_some() {
        return None;
    }
    let candidates = select_target_candidates(intel, obs.tick, tuning.tactical_memory);
    if candidates
        .iter()
        .any(|target| target.evidence == ContactEvidence::Current)
        || combat_roster(obs) < CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER
        || !ready_to_reconnoiter(obs)
    {
        return None;
    }
    candidates.first().copied().map(FreshAirTarget::Remembered)
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
    plan: &mut ConnectedPlan,
    target: &BuildingContact,
    surviving_anchors: &[TilePos],
) {
    let objective = target
        .id
        .expect("a current connected revision target has an exact id");
    op.target_kind = target.kind;
    op.target = target.anchor;
    op.target_id = Some(objective);
    plan.package.target_anchors = surviving_anchors.to_vec();
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
    let routes = route_projection_with_orientation(obs, Domain::Ground, public_map, orientation);
    if origins.iter().all(|origin| *origin == origins[0]) {
        let mut stands: Vec<_> =
            suppression_firing_stands(&routes, obs, origins[0], target, intel, public_map)
                .take(origins.len())
                .collect();
        if stands.len() != origins.len() {
            return None;
        }
        // With identical options, each augmenting member displaces the earlier
        // members by one stand. Preserve that exact assignment order directly.
        stands.reverse();
        return Some(stands);
    }
    let mut by_origin = BTreeMap::new();
    let stand_options: Vec<Vec<TilePos>> = origins
        .iter()
        .map(|origin| {
            by_origin
                .entry(*origin)
                .or_insert_with(|| {
                    suppression_firing_stands(&routes, obs, *origin, target, intel, public_map)
                        .collect::<Vec<_>>()
                })
                .clone()
        })
        .collect();
    assign_suppression_stands(origins.len(), stand_options)
}

fn assign_suppression_stands(
    member_count: usize,
    stand_options: Vec<Vec<TilePos>>,
) -> Option<Vec<TilePos>> {
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
    for member in 0..member_count {
        let mut visited = vec![false; stands.len()];
        if !augment_suppression_assignment(member, &options, &mut visited, &mut owner_by_stand) {
            return None;
        }
    }
    let mut assigned = vec![None; member_count];
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
                    if force_package::current_operational_aa_source(intel, source.source) {
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
    let Some(package) = plan.package() else {
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
            plan.package().and_then(|package| {
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
    match plan.strike_dispatch() {
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
    let Some(package) = plan.package() else {
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
    let Some(package) = plan.package() else {
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
    route.with_navigation(obs, |navigation| {
        let scout_kind = Role::Scout.unit_for(obs.faction);
        let staging = navigation.staging(route.home, route.target);
        let route = ConnectedRouteContext {
            campaign_routes: Some(navigation),
            ..route
        };
        let ground_routes = navigation.ground();
        let air_routes = navigation.air();
        excluded.extend(candidates.into_iter().filter_map(|member| {
            let compatible = if is_artillery(member.kind) {
                staging.is_some_and(|goal| {
                    ground_routes.ground_command_reaches(member.tile, goal)
                        && suppression_targets_reachable_in_context(
                            ground_routes,
                            obs,
                            SuppressionOrigin {
                                tile: member.tile,
                                kind: member.kind,
                            },
                            &targets.suppression_targets,
                            route,
                        )
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

fn connected_production_access<'a>(
    obs: &'a Observation,
    targets: &ConnectedTargetSelection,
    resources: &ResourceSnapshot,
    route: ConnectedRouteContext<'a>,
) -> ProductionAccess {
    route.with_navigation(obs, |navigation| {
        let staging = navigation.staging(route.home, route.target);
        let route = ConnectedRouteContext {
            campaign_routes: Some(navigation),
            ..route
        };
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
                        production_spawn_doorstep(
                            QueryPurpose::AirOperation,
                            obs,
                            producer,
                            route.public_map,
                            Some(route.orientation),
                        )
                        .is_some_and(|spawn| {
                            ground_routes.ground_command_reaches(spawn, staging)
                                && suppression_targets_reachable_in_context(
                                    ground_routes,
                                    obs,
                                    SuppressionOrigin { tile: spawn, kind },
                                    &targets.suppression_targets,
                                    route,
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
            .excluding_paid(route.unavailable_paid)
    })
}

fn connected_target_selection<'a>(
    obs: &'a Observation,
    target: &BuildingContact,
    unavailable: &[UnitId],
    route: ConnectedRouteContext<'a>,
) -> ConnectedTargetSelection {
    route.with_navigation(obs, |navigation| {
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
        let staging = navigation.staging(route.home, route.target);
        let air_routes = navigation.air();
        let route = ConnectedRouteContext {
            campaign_routes: Some(navigation),
            ..route
        };
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
                            && suppression_targets_reachable_in_context(
                                ground_routes,
                                obs,
                                *origin,
                                &proposed_suppression,
                                route,
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
    })
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
            || !force_package::current_operational_aa_source(intel, source.source)
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

fn suppression_targets_reachable_in_context(
    routes: &RouteProjection<'_>,
    obs: &Observation,
    origin: SuppressionOrigin,
    targets: &[Target],
    route: ConnectedRouteContext<'_>,
) -> bool {
    if let Some(cached) = route.campaign_routes {
        targets.iter().all(|target| cached.reaches(origin, *target))
    } else {
        suppression_targets_reachable(routes, obs, origin, targets, route.intel, route.public_map)
    }
}

fn suppression_targets_reachable(
    routes: &RouteProjection<'_>,
    obs: &Observation,
    origin: SuppressionOrigin,
    targets: &[Target],
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    targets.iter().all(|target| {
        legal_suppression_stands(obs, origin, *target, intel, public_map)
            .into_iter()
            .any(|stand| routes.ground_command_reaches(origin.tile, stand))
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
    routes: &RouteProjection<'_>,
    obs: &Observation,
    origin: SuppressionOrigin,
    target: Target,
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
) -> impl Iterator<Item = TilePos> {
    let mut stands = legal_suppression_stands(obs, origin, target, intel, public_map);
    stands.retain(|stand| routes.ground_command_reaches(origin.tile, *stand));
    stands.into_iter()
}

fn legal_suppression_stands(
    obs: &Observation,
    origin: SuppressionOrigin,
    target: Target,
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
) -> Vec<TilePos> {
    let mut stands = legal_suppression_tiles(obs, origin.kind, target, intel, public_map, |tile| {
        public_ground_open(obs, tile, public_map)
    });
    stands.sort_unstable_by_key(|stand| (stand.chebyshev(origin.tile), stand.y, stand.x));
    stands
}

fn legal_suppression_tiles(
    obs: &Observation,
    kind: UnitKind,
    target: Target,
    intel: &StrategicIntelligence,
    public_map: Option<&PublicMapBriefing>,
    open: impl Fn(TilePos) -> bool,
) -> Vec<TilePos> {
    let mut stands = Vec::new();
    let Some(weapon) = suppression_weapon(kind) else {
        return stands;
    };
    let Some(geometry) = suppression_target_geometry(intel, target) else {
        return stands;
    };
    let (near, far) = geometry.tile_bounds();
    let radius = weapon.range.ceil().to_num::<i32>();
    for y in near.y.saturating_sub(radius)..=far.y.saturating_add(radius) {
        for x in near.x.saturating_sub(radius)..=far.x.saturating_add(radius) {
            let stand = TilePos::new(x, y);
            if !open(stand) || !suppression_shot_is_legal(obs, public_map, weapon, stand, geometry)
            {
                continue;
            }
            stands.push(stand);
        }
    }
    stands
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
    if let Some(package) = plan.package() {
        assign_provider_demands(assigned, &package.suppression, obs, enlisted);
    } else {
        // An island assault requests no artillery; this only prunes dead
        // members inherited from standby.
        assign_exact(assigned, 0, obs, enlisted, is_artillery);
    }
}

fn assign_strike_aircraft(
    assigned: &mut Vec<UnitId>,
    plan: &AirPlan,
    obs: &Observation,
    enlisted: &[UnitId],
) {
    if let Some(package) = plan.package() {
        assign_provider_demands(assigned, &package.strike, obs, enlisted);
    } else {
        let bomber = Role::Bomber.unit_for(obs.faction);
        assign_exact(
            assigned,
            plan.desired_strike_aircraft(),
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
    member_reservations(op, plan.screen(), obs)
}

fn member_reservations(op: &AirOperation, screen: &[UnitId], obs: &Observation) -> Vec<UnitId> {
    let mut ids: Vec<_> = op
        .scout
        .into_iter()
        .chain(op.artillery.iter().copied())
        .chain(op.strike_aircraft.iter().copied())
        .chain(screen.iter().copied())
        .filter(|id| unit(obs, *id).is_some())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn connected_provider_shortfall(
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
fn emergency_paid_queue_access(
    obs: &Observation,
    excluded: &[(BuildingId, UnitKind, usize)],
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
    if let Some(screen) = plan.screen_mut() {
        screen.retain(keep);
    }
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

fn schedule_missing_members(
    op: &AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
    scout_kind: UnitKind,
    out: &mut StrategicDecision,
) {
    if let AirPlan::Island(island) = plan {
        schedule(
            context,
            &missing_island_members(
                AirRoster::from(op),
                island.screen.len(),
                island,
                context,
                scout_kind,
            ),
        )
        .append_to(out);
    }
}

fn missing_island_members(
    members: AirRoster<'_>,
    screen_count: usize,
    plan: &IslandPlan,
    context: &AirPlanningContext<'_>,
    scout_kind: UnitKind,
) -> [(UnitKind, usize); 3] {
    let obs = context.obs;
    let bomber_kind = Role::Bomber.unit_for(obs.faction);
    let missing_scout = 1usize.saturating_sub(
        usize::from(members.scout.is_some()) + unowned_queued_scouts(context, scout_kind),
    );
    let missing_strike_aircraft = plan
        .desired_strike_aircraft
        .saturating_sub(members.strike_aircraft.len() + queued(obs, |k| k == bomber_kind));
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    let missing_screen = plan
        .desired_screen
        .saturating_sub(screen_count + queued(obs, |kind| kind == screen_kind));
    [
        (scout_kind, missing_scout),
        (screen_kind, missing_screen),
        (bomber_kind, missing_strike_aircraft),
    ]
}

fn connected_package_is_proven_infeasible(
    op: &AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
) -> bool {
    let Some(package) = plan.package() else {
        return false;
    };
    if !context.allow_procurement || context.obs.tick >= package.preparation_deadline {
        return false;
    }
    let resources = context
        .connected_resources
        .as_ref()
        .expect("connected preparation has one observation-bound resource view");
    let outstanding = missing_package_demands(
        package,
        AirRoster::from(op),
        context.obs,
        &resources.snapshot,
        package.preparation_deadline,
        &resources.access,
    );
    matches!(
        refine_provider_demands(
            ProductionEvidence::with_planning(
                &resources.snapshot,
                &resources.access,
                context.planning
            ),
            &outstanding,
            context.obs.tick,
            PreparationConstraints {
                deadline: package.preparation_deadline,
                decision_cadence: context.tuning.cadence,
                protected_forecast_scrap: context.protected_forecast_scrap,
            },
            crate::allocation::ConnectedOffenseKey {
                objective: op
                    .target_id
                    .expect("a connected package retains its objective"),
                anchor: op.target,
            },
        ),
        crate::planning::Progress::ProvenInfeasible
    )
}

fn missing_package_demands(
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
    let focus = if matches!(plan, AirPlan::Connected(_)) {
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
    let Some(goal) = scout_dispatch_goal(op, plan, obs, intel, landing_sites, public_map) else {
        return false;
    };
    dispatch_scout_to(op, obs, goal, public_map, out)
}

fn scout_dispatch_goal(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    let target = if matches!(plan, AirPlan::Connected(_)) {
        connected_scout_focus(op, plan, obs, intel)
    } else {
        op.target
    };
    scout_goal(op, obs, intel, target, landing_sites, public_map)
}

fn connected_scout_focus(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> TilePos {
    let Some(package) = plan.package() else {
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
    if !scout_dispatch_is_viable(op, obs, goal, public_map) {
        return false;
    }
    let Some(scout) = op.scout else {
        return true;
    };
    let member = unit(obs, scout).expect("a viable scout dispatch retains its live unit");
    if op.scout_dispatch == Some((scout, goal)) {
        return true;
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

fn scout_dispatch_is_viable(
    op: &AirOperation,
    obs: &Observation,
    goal: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> bool {
    let Some(scout) = op.scout else {
        return true;
    };
    let Some(member) = unit(obs, scout) else {
        return false;
    };
    let mut air_routes = route_projection(obs, Domain::Air, public_map);
    air_routes.unit_reaches(member, goal)
        && (op.scout_dispatch != Some((scout, goal))
            || !member.idle
            || member.tile.chebyshev(goal) <= 1)
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
    let routes = route_projection(obs, Domain::Air, public_map);
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
    units.extend(plan.screen().iter().copied());
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
        .chain(plan.screen())
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
    let package = plan.package();
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
    let Some(package) = plan.package() else {
        return current_target_contact(op, intel);
    };
    connected_revision_target(op, package, intel)
}

fn connected_revision_target<'a>(
    op: &AirOperation,
    package: &ConnectedForcePackage,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
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
    let starts: Vec<_> = oxide_sim::geometry::rect_adjacent_tiles(home, home_size)
        .filter(|tile| routing::ground_open(QueryPurpose::AirOperation, obs, *tile))
        .filter(|tile| {
            public_map.is_none_or(|map| {
                map.terrain_at(*tile)
                    .is_some_and(|terrain| !terrain.blocks_ground())
            })
        })
        .collect();
    let goals: Vec<_> = oxide_sim::geometry::rect_adjacent_tiles(target, target_size)
        .filter(|tile| routing::ground_open(QueryPurpose::AirOperation, obs, *tile))
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
        return Some(public_ground_connected(public_map, &starts, &goals));
    }

    let optimistic = RouteProjection::new(QueryPurpose::AirOperation, obs, Domain::Ground);
    if !starts
        .iter()
        .any(|start| goals.iter().any(|goal| optimistic.reaches(*start, *goal)))
    {
        return Some(false);
    }

    let routes = RouteProjection::known_ground(QueryPurpose::AirOperation, obs);
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
    public_map.regions().connects(starts, goals)
}

fn operation_timeout(profile: &ResolvedProfile, plan: &AirPlan) -> Tick {
    3_200u64
        .saturating_add(plan.assembly_timeout())
        .saturating_add(u64::from(100u8.saturating_sub(profile.traits.air)) * 4)
}

fn phase_timeout(phase: AirOperationPhase, plan: &AirPlan) -> Tick {
    match phase {
        AirOperationPhase::Recon => 900,
        AirOperationPhase::Assemble => plan.assembly_timeout(),
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

fn is_strike_aircraft(kind: UnitKind, faction: oxide_sim::state::Faction) -> bool {
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

#[cfg(test)]
fn connected_artillery_staging_goal(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    let routes = route_projection(obs, Domain::Ground, public_map);
    artillery_staging_with_routes(obs, home, target, public_map, &routes)
}

fn artillery_staging_with_routes(
    obs: &Observation,
    home: TilePos,
    target: TilePos,
    public_map: Option<&PublicMapBriefing>,
    routes: &RouteProjection<'_>,
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
    let starts: Vec<_> = oxide_sim::geometry::rect_adjacent_tiles(home, home_size)
        .filter(|tile| public_ground_open(obs, *tile, public_map))
        .collect();
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
    let Some(source_staging) = route.staging(obs) else {
        return false;
    };
    route.with_navigation(obs, |navigation| {
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

fn connected_suppression_roster_has_firing_assignments(
    obs: &Observation,
    route: ConnectedRouteContext<'_>,
    demands: &[ProviderDemand],
    targets: &[Target],
) -> bool {
    if targets.is_empty() {
        return true;
    }
    let Some(staging) = route.staging(obs) else {
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
            if let Some(cached) = route.campaign_routes {
                cached.assignment(&origins, *target)
            } else {
                suppression_firing_assignment(
                    obs,
                    route.intel,
                    &origins,
                    *target,
                    route.public_map,
                    route.orientation,
                )
            }
            .is_some()
        })
}

fn artillery_group_reaches_staging(
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
    let routes = route_projection_with_orientation(obs, Domain::Ground, public_map, orientation);
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
        || RouteProjection::new(QueryPurpose::AirOperation, obs, domain),
        |map| RouteProjection::with_public_terrain(QueryPurpose::AirOperation, obs, domain, map),
    )
}

fn route_projection_with_orientation<'a>(
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
) -> RouteProjection<'a> {
    public_map.map_or_else(
        || RouteProjection::with_orientation(QueryPurpose::AirOperation, obs, domain, orientation),
        |map| {
            RouteProjection::with_public_terrain_and_orientation(
                QueryPurpose::AirOperation,
                obs,
                domain,
                map,
                orientation,
            )
        },
    )
}

fn operation_route_projection<'a>(
    plan: &AirPlan,
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
) -> RouteProjection<'a> {
    if matches!(plan, AirPlan::Connected(_)) {
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
    routing::ground_open(QueryPurpose::AirOperation, obs, tile)
        && public_map.is_none_or(|map| {
            map.terrain_at(tile)
                .is_some_and(|terrain| !terrain.blocks_ground())
        })
}

fn elapsed(start: Tick, now: Tick) -> Tick {
    now.saturating_sub(start)
}

fn enter(op: &mut AirOperation, stage: AirStage, now: Tick) {
    if stage == AirStage::SuppressAa {
        op.membership_frozen_at.get_or_insert(now);
    }
    op.stage = stage;
    op.phase_started_at = now;
}

fn recover(op: &mut AirOperation, reason: AirRecoveryReason, now: Tick) {
    enter(
        op,
        AirStage::Recover {
            reason,
            assault_admitted: op.assault_admitted(),
        },
        now,
    );
    op.scout_dispatch = None;
}

#[cfg(test)]
mod tests;
