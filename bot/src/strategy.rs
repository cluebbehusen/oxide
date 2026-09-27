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
use super::experience::{Outcome, OutcomeReason};
use super::intelligence::{
    AirDefenseAssessment, AirDefenseEvidence, AirDefenseSource, BuildingContact, ContactEvidence,
    StrategicIntelligence,
};
use super::navigation::commands::{self as routing, RouteProjection, production_spawn_doorstep};
use super::observation::{Observation, UnitObs};
use super::orient::Orientation;
use super::profile::ResolvedProfile;
use super::resources::{
    PaidQueueClaim, ProducerLaneReservations, ProductionAccess, ResourceSnapshot,
    count_paid_queued_ready_with_access, paid_queued_ready_occurrences_with_access,
};
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
#[cfg(test)]
pub(crate) mod fixtures;
pub(super) mod force_package;
mod geometry;
mod roster;
use campaign_routes::CampaignRoutes;
use geometry::*;
use roster::*;

use force_package::{
    ConnectedForcePackage, ConnectedForcePackageOptions, ConnectedTargetEvidence, ForceFamily,
    ForcePackageRejection, NormalizedCapability, PreparationConstraints, ProductionEvidence,
    ProviderDemand, ProviderDemandTranche, building_value, current_aa_contact,
    current_target_cluster, derive_connected_force_package_options_for_cluster, eligible_producers,
    refine_provider_demands, strike_capability, suppression_capability, target_cluster_air_defense,
};

/// A connected-map combined-arms operation is an expensive second front, not
/// an opening build order. Keep a real fighting roster online before reserving
/// scouts, artillery, and strike aircraft so a seeded specialty cannot hollow
/// out the ordinary line that protects the economy.
const CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER: usize = 12;
/// A connected operation may use only completed production that can finish its
/// whole requested package inside this immutable preparation window. The
/// coordinator's joint resource projection shares it.
pub(crate) const CONNECTED_PREPARATION_HORIZON: Tick = 2_400;

#[cfg(test)]
thread_local! {
    static AIRWORKS_PACKAGE_DERIVATIONS: core::cell::Cell<usize> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn airworks_package_derivations() -> usize {
    AIRWORKS_PACKAGE_DERIVATIONS.with(core::cell::Cell::get)
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

/// Production resources and fixed deadline one package derivation sizes
/// against.
#[derive(Debug, Clone, Copy)]
struct PackageBasis<'a> {
    /// Frozen identity of the admitted operation a revision resizes.
    committed: Option<crate::allocation::ConnectedOffenseKey>,
    resources: &'a ConnectedProductionResources,
    deadline: Tick,
}

impl PackageBasis<'_> {
    fn preparation(self, context: FreshConnectedDerivationContext<'_>) -> PreparationConstraints {
        PreparationConstraints {
            deadline: self.deadline,
            decision_cadence: context.ev.tuning.cadence,
            protected_forecast_scrap: context.inputs.reserve.forecast,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ConnectedRouteContext<'a> {
    campaign_routes: Option<&'a CampaignRoutes<'a>>,
    unavailable_paid: &'a [PaidQueueClaim],
    intel: &'a StrategicIntelligence,
    home: TilePos,
    target: TilePos,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Orientation,
}

impl<'a> ConnectedRouteContext<'a> {
    fn new(
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

    fn with_routes(self, campaign_routes: Option<&'a CampaignRoutes<'a>>) -> Self {
        Self {
            campaign_routes,
            ..self
        }
    }

    fn excluding_paid(self, unavailable_paid: &'a [PaidQueueClaim]) -> Self {
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
    fn from_candidates(
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

    fn from_package_snapshot_after_current_reserve(
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
    commitment: ConnectedCommitment,
    /// Sticky member the operation stages, scouts, and strikes first. It moves
    /// only during preparation, and only when it is no longer in current sight.
    focus: TilePos,
    package: ConnectedForcePackage,
    paid_production: Vec<ConnectedPurchase>,
    dispatch: AirDispatch,
}

/// Identity and target set a connected operation commits to at admission.
/// Revisions resize the package against these members but never change them;
/// a member leaves the operation only when current sight finds it gone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct ConnectedCommitment {
    player: PlayerId,
    primary: BuildingId,
    primary_kind: BuildingKind,
    /// Primary anchor at admission.
    scope: TilePos,
    /// Canonical `(y, x)` anchors of every admitted member, including `scope`.
    anchors: Vec<TilePos>,
    minimum_capability: NormalizedCapability,
    admitted_at: Tick,
    deadline: Tick,
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

    fn admitted_at(&self) -> Tick {
        match self {
            Self::Reacquire(plan) => plan.admitted_at,
            Self::Island(plan) => plan.admitted_at,
            Self::Connected(plan) => plan.commitment.admitted_at,
        }
    }

    fn connected(&self) -> Option<&ConnectedPlan> {
        match self {
            Self::Connected(plan) => Some(plan),
            Self::Reacquire(_) | Self::Island(_) => None,
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

    /// Assembly patience for an operation started at `started_at`. A
    /// connected window ends at its committed deadline, so revisions cannot
    /// shorten it.
    fn assembly_timeout(&self, started_at: Tick) -> Tick {
        match self {
            Self::Reacquire(plan) => plan.assembly_timeout,
            Self::Island(plan) => plan.assembly_timeout,
            Self::Connected(plan) => plan.commitment.deadline.saturating_sub(started_at),
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
    fn new(commitment: ConnectedCommitment, package: ConnectedForcePackage) -> Self {
        Self {
            focus: commitment.scope,
            commitment,
            package,
            paid_production: Vec::new(),
            dispatch: AirDispatch::default(),
        }
    }

    /// Moves a focus that has left current sight to the best member still in
    /// it. With no member in sight, the previous focus is kept.
    fn refocus(&mut self, intel: &StrategicIntelligence) {
        if let Some(best) = best_current_member(&self.commitment, self.focus, intel) {
            self.focus = best.anchor;
        }
    }
}

impl ConnectedCommitment {
    fn admit(
        primary: &BuildingContact,
        package: &ConnectedForcePackage,
        admitted_at: Tick,
    ) -> Self {
        Self {
            player: primary.player,
            primary: primary
                .id
                .expect("an admitted connected objective is current"),
            primary_kind: primary.kind,
            scope: primary.anchor,
            anchors: package.target_anchors.clone(),
            minimum_capability: package.minimum_capability,
            admitted_at,
            deadline: package.preparation_deadline,
        }
    }

    fn key(&self) -> crate::allocation::ConnectedOffenseKey {
        crate::allocation::ConnectedOffenseKey {
            objective: self.primary,
            anchor: self.scope,
        }
    }

    fn contains(&self, anchor: TilePos) -> bool {
        self.anchors
            .binary_search_by_key(&(anchor.y, anchor.x), |member| (member.y, member.x))
            .is_ok()
    }

    /// Admitted members not yet observed gone. Out of sight, a member stays
    /// live on its remembered contact.
    fn live_members<'a>(&self, intel: &'a StrategicIntelligence) -> Vec<&'a BuildingContact> {
        intel
            .buildings()
            .iter()
            .filter(|contact| {
                contact.player == self.player
                    && self.contains(contact.anchor)
                    && contact.built
                    && contact.hp > 0
                    && building_value(contact.kind) > 0
            })
            .collect()
    }

    /// Live members whose evidence can size a revision: current sight, or a
    /// remembered contact that still has positive confidence.
    fn sized_members<'a>(
        &self,
        intel: &'a StrategicIntelligence,
        now: Tick,
    ) -> Vec<&'a BuildingContact> {
        sized_target_contacts_at_anchors(intel, self.player, &self.anchors, now)
    }
}

impl IslandPlan {
    fn new(profile: &ResolvedProfile, obs: &Observation, lanes: ProducerLanes<'_>) -> Self {
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
                let same_think_work = lanes
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
                let reserved_delay = lanes
                    .reservations
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

fn derive_connected_package_options(
    context: FreshConnectedDerivationContext<'_>,
    basis: PackageBasis<'_>,
    target: &BuildingContact,
) -> Result<ConnectedForcePackageOptions, ConnectedPlanRejection> {
    let AirEvidence {
        obs,
        intel,
        home,
        public_map,
        ..
    } = context.ev;
    if known_ground_connection(
        obs,
        home,
        target.anchor,
        target.kind.base_stats().size,
        public_map,
    ) != Some(true)
    {
        return Err(ConnectedPlanRejection::DisconnectedGroundRoute);
    }
    // The package must refuse the same paid queue work as the resources it
    // is derived against, or it can lean on an occurrence the lowered claims
    // will not be allowed to take.
    let route = context
        .ev
        .route(target.anchor)
        .with_routes(Some(context.campaign_routes))
        .excluding_paid(basis.resources.access.paid_exclusions());
    let mut selected = connected_target_subset(obs, intel, target, &[target.anchor]);
    let mut packages =
        derive_connected_package_options_for_targets(context, basis, target, &selected, route)?;
    if context.minimum_only {
        return Ok(packages);
    }
    for anchor in &basis.resources.targets.growth_order {
        let mut proposed_anchors = selected.target_anchors.clone();
        proposed_anchors.push(*anchor);
        let proposed = connected_target_subset(obs, intel, target, &proposed_anchors);
        if let Ok(proposed_packages) =
            derive_connected_package_options_for_targets(context, basis, target, &proposed, route)
        {
            selected = proposed;
            packages = proposed_packages;
        }
    }
    Ok(packages)
}

fn derive_connected_package_options_for_targets(
    context: FreshConnectedDerivationContext<'_>,
    basis: PackageBasis<'_>,
    target: &BuildingContact,
    targets: &ConnectedTargetSelection,
    route: ConnectedRouteContext<'_>,
) -> Result<ConnectedForcePackageOptions, ConnectedPlanRejection> {
    let AirEvidence { obs, intel, .. } = context.ev;
    let access = connected_production_access(obs, targets, &basis.resources.snapshot, route);
    let unavailable =
        connected_provider_unavailable(obs, targets, context.inputs.unavailable, route);
    let preparation = basis.preparation(context);
    let protected_forecast_scrap = preparation.protected_forecast_scrap.min(
        basis
            .resources
            .snapshot
            .forecast()
            .income_through(preparation.deadline)
            .amount(),
    );
    let cluster =
        sized_target_contacts_at_anchors(intel, target.player, &targets.target_anchors, obs.tick);
    let mut packages = derive_connected_force_package_options_for_cluster(
        context.ev,
        ConnectedTargetEvidence {
            primary: target,
            cluster: &cluster,
            committed: basis.committed,
        },
        ProductionEvidence::with_planning(
            &basis.resources.snapshot,
            &access,
            Some(context.inputs.planning),
        ),
        &unavailable,
        preparation,
        context.minimum_only,
    )
    .map_err(|reason| ConnectedPlanRejection::Package {
        reason,
        protected_current_scrap: context.inputs.reserve.current,
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
    obs: &Observation,
    intel: &StrategicIntelligence,
    target: &BuildingContact,
    anchors: &[TilePos],
) -> ConnectedTargetSelection {
    let cluster = sized_target_contacts_at_anchors(intel, target.player, anchors, obs.tick);
    let mut target_anchors: Vec<_> = cluster.iter().map(|contact| contact.anchor).collect();
    target_anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
    target_anchors.dedup();
    ConnectedTargetSelection {
        target_anchors,
        suppression_targets: current_cluster_suppression_needs(intel, &cluster).targets,
        growth_order: Vec::new(),
    }
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
            eligible_producers: eligible_producers(
                &resources.snapshot,
                &resources.access,
                provider.kind,
                Some(package.preparation_deadline),
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
        units: AirRoster::from(op).live_members(&[], obs),
        paid_providers: connected_paid_provider_claims(op, package, resources, obs),
        provider_jobs,
    }
}

fn connected_paid_provider_claims(
    op: &AirOperation,
    package: &ConnectedForcePackage,
    resources: &ConnectedProductionResources,
    obs: &Observation,
) -> Vec<PaidQueueClaim> {
    let mut needed = BTreeMap::<UnitKind, usize>::new();
    for demand in &package.provider_priority {
        let count = needed.entry(demand.kind).or_default();
        *count = count.saturating_add(demand.count);
    }
    for id in op.members() {
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
                .map(|(producer, occurrence)| PaidQueueClaim {
                    producer,
                    kind,
                    occurrence,
                }),
        );
    }
    paid.sort_unstable();
    paid
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
    /// The recovery an admitted operation enters when its revision fails
    /// this way.
    pub(crate) fn recovery_reason(self) -> AirRecoveryReason {
        match self {
            Self::DisconnectedGroundRoute | Self::UnreachableGroupStaging { .. } => {
                AirRecoveryReason::UnreachableStaging
            }
            Self::Package {
                reason: ForcePackageRejection::UntargetableCurrentAirDefense { .. },
                ..
            } => AirRecoveryReason::NewAirDefense,
            Self::Package {
                reason: ForcePackageRejection::TargetNotActionable,
                ..
            } => AirRecoveryReason::ObjectiveLost,
            Self::InsufficientStandingForce { .. } | Self::Package { .. } => {
                AirRecoveryReason::PreparationInfeasible
            }
        }
    }

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
    /// Last known target owner. A connected operation keeps the owner of the
    /// cluster it committed to.
    pub target_player: PlayerId,
    /// Last known target kind. A connected operation keeps its admitted
    /// primary's kind.
    pub target_kind: BuildingKind,
    /// Stable target footprint anchor. A connected operation keeps its
    /// admitted primary's anchor while its tactics may focus another member.
    pub target: TilePos,
    /// Last live id, used only with current evidence. A connected operation
    /// keeps its admitted primary's id.
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

    fn members(&self) -> impl Iterator<Item = UnitId> + '_ {
        AirRoster::from(self).members()
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
        let mut ids: Vec<_> = AirRoster::from(self).members().collect();
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
    ev: AirEvidence<'a>,
    /// Procurement whose `unavailable` units are every unit the operation
    /// may not enlist this decision.
    procurement: AirProcurement<'a>,
    landing_sites: &'a [TilePos],
    connected_resources: Option<ConnectedProductionResources>,
}

/// Exact transport objective and landing envelope offered to the air planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LiftSupportRequest {
    pub player: PlayerId,
    pub target: TilePos,
    pub planned_drops: Vec<TilePos>,
}

/// Same-observation evidence every air-planner call in one decision reads.
#[derive(Clone, Copy)]
pub(crate) struct AirEvidence<'a> {
    pub(crate) profile: &'a ResolvedProfile,
    pub(crate) tuning: DifficultyTuning,
    pub(crate) obs: &'a Observation,
    pub(crate) intel: &'a StrategicIntelligence,
    pub(crate) home: TilePos,
    pub(crate) public_map: Option<&'a PublicMapBriefing>,
    pub(crate) orientation: Orientation,
}

impl<'a> AirEvidence<'a> {
    fn route(self, target: TilePos) -> ConnectedRouteContext<'a> {
        ConnectedRouteContext::new(
            self.intel,
            self.public_map,
            self.orientation,
            self.home,
            target,
        )
    }
}

/// Current scrap, and forecast scrap through the preparation window, that
/// earlier owners already hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CapitalReserve {
    pub(crate) current: u32,
    pub(crate) forecast: u32,
}

/// Allocation evidence for deriving one connected-operation proposal.
#[derive(Clone, Copy)]
pub(crate) struct ConnectedInputs<'a> {
    pub(crate) planning: &'a crate::planning::PlanningWork,
    pub(crate) resources: &'a ResourceSnapshot,
    /// Units other owners hold.
    pub(crate) unavailable: &'a [UnitId],
    /// Paid queue occurrences other programs own.
    pub(crate) paid_exclusions: &'a [PaidQueueClaim],
    pub(crate) reserve: CapitalReserve,
}

/// Producer work already accepted ahead of the air planner this decision.
#[derive(Clone, Copy)]
pub(crate) struct ProducerLanes<'a> {
    pub(crate) prior_intents: &'a [Intent],
    pub(crate) reservations: &'a ProducerLaneReservations,
}

impl ProducerLanes<'static> {
    pub(crate) fn empty() -> Self {
        Self {
            prior_intents: &[],
            reservations: ProducerLaneReservations::empty(),
        }
    }
}

/// Allocation evidence for recruiting and buying an air operation's members.
#[derive(Clone, Copy)]
pub(crate) struct AirProcurement<'a> {
    pub(crate) planning: &'a crate::planning::PlanningWork,
    /// Units other owners hold.
    pub(crate) unavailable: &'a [UnitId],
    /// Paid queue occurrences other programs own.
    pub(crate) paid_exclusions: &'a [PaidQueueClaim],
    pub(crate) reserve: CapitalReserve,
    pub(crate) lanes: ProducerLanes<'a>,
    /// Whether the operation may buy members. The lifecycle also begins a
    /// new operation only when it may.
    pub(crate) allow: bool,
}

/// Allocation verdicts the post-adjudication lifecycle acts on.
pub(crate) struct ThinkInputs<'a> {
    pub(crate) procurement: AirProcurement<'a>,
    /// Units another owner claimed after allocation; the operation waits
    /// rather than acting through them.
    pub(crate) claimed_elsewhere: &'a [UnitId],
    pub(crate) lift_support: Option<&'a LiftSupportRequest>,
    /// Excludes every unit the operation does not already own.
    pub(crate) owned_only: bool,
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
    /// The representative operation fields, the focus, and the sized package
    /// must all stay inside the frozen commitment. The package's demand drives
    /// per-provider job expansion and the paid ledger is walked and cloned
    /// during planning, so both are bounded by the map area rather than
    /// trusted from the checkpoint.
    fn valid_checkpoint(&self, op: &AirOperation, map: &PublicMapBriefing, tick: Tick) -> bool {
        let area = usize::try_from(map.map_width())
            .unwrap_or(0)
            .saturating_mul(usize::try_from(map.map_height()).unwrap_or(0));
        let commitment = &self.commitment;
        let package = &self.package;
        commitment.valid_checkpoint(map)
            && (op.target_player, op.target, op.target_id, op.target_kind)
                == (
                    commitment.player,
                    commitment.scope,
                    Some(commitment.primary),
                    commitment.primary_kind,
                )
            && commitment.contains(self.focus)
            && canonical_anchors(&package.target_anchors)
            && package
                .target_anchors
                .iter()
                .all(|anchor| commitment.contains(*anchor))
            && package.preparation_deadline == commitment.deadline
            && package.minimum_capability == commitment.minimum_capability
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
            && self.paid_production.len() <= area
            && self.paid_production.iter().all(|purchase| {
                purchase.issued_at <= purchase.ready_at && purchase.issued_at <= tick
            })
    }
}

impl ConnectedCommitment {
    /// Every member lies within the admission cluster radius of the scope,
    /// which also bounds the member walks each think performs.
    fn valid_checkpoint(&self, map: &PublicMapBriefing) -> bool {
        canonical_anchors(&self.anchors)
            && self.contains(self.scope)
            && self.anchors.iter().all(|anchor| {
                on_map(map, *anchor) && force_package::within_target_cluster(self.scope, *anchor)
            })
    }
}

fn canonical_anchors(anchors: &[TilePos]) -> bool {
    !anchors.is_empty()
        && anchors
            .windows(2)
            .all(|pair| (pair[0].y, pair[0].x) < (pair[1].y, pair[1].x))
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
}

/// Atomic shared claims for one exact connected package.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ConnectedOffenseClaims {
    units: Vec<UnitId>,
    paid_providers: Vec<PaidQueueClaim>,
    provider_jobs: Vec<ConnectedProviderJob>,
}

impl ConnectedOffenseClaims {
    /// Exact live providers reserved by this package.
    pub(crate) fn units(&self) -> &[UnitId] {
        &self.units
    }

    /// Exact paid queue occurrences that satisfy this package's demand.
    pub(crate) fn paid_providers(&self) -> &[PaidQueueClaim] {
        &self.paid_providers
    }

    /// Exact unpaid production requests retained by this package.
    pub(crate) fn provider_jobs(&self) -> &[ConnectedProviderJob] {
        &self.provider_jobs
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
    variants: Vec<ConnectedProposalVariant>,
    marginal: Vec<ConnectedMarginalVariant>,
    selected_variant: usize,
    case: ConnectedOpportunityCase,
}

impl FreshConnectedProposal {
    pub(crate) fn revises_active_operation(&self) -> bool {
        matches!(self.origin, ConnectedProposalOrigin::Active { .. })
    }

    /// Identity every variant commits to. A fresh proposal takes it from the
    /// objective selected by domain ranking; a revision keeps the identity its
    /// operation was admitted under.
    pub(crate) fn identity(&self) -> crate::allocation::ConnectedOffenseKey {
        self.variants[0].plan.commitment.key()
    }

    /// Fixed deadline shared by the minimum and all marginal variants.
    pub(crate) fn deadline(&self) -> Tick {
        self.variants[0].plan.package.preparation_deadline
    }

    /// Original strategic admission tick retained across remembered
    /// reconnaissance and fresh cross-domain assault adjudication.
    pub(crate) fn accepted_at(&self) -> Tick {
        self.variants[0].plan.commitment.admitted_at
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
        AirRoster::from(self).live_members(&self.screen, obs)
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

#[derive(Clone, Copy)]
struct AirRoster<'a> {
    scout: Option<UnitId>,
    artillery: &'a [UnitId],
    strike_aircraft: &'a [UnitId],
}

impl<'a> AirRoster<'a> {
    fn members(self) -> impl Iterator<Item = UnitId> + 'a {
        self.scout
            .into_iter()
            .chain(self.artillery.iter().copied())
            .chain(self.strike_aircraft.iter().copied())
    }

    /// Canonical ids of the members and `screen` still alive in `obs`.
    fn live_members(self, screen: &[UnitId], obs: &Observation) -> Vec<UnitId> {
        let mut ids: Vec<_> = self
            .members()
            .chain(screen.iter().copied())
            .filter(|id| unit(obs, *id).is_some())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
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

impl<'a> From<&'a AirMembership> for AirRoster<'a> {
    fn from(membership: &'a AirMembership) -> Self {
        Self {
            scout: membership.scout,
            artillery: &membership.artillery,
            strike_aircraft: &membership.strike_aircraft,
        }
    }
}

impl<'a> From<&'a AirStandby> for AirRoster<'a> {
    fn from(standby: &'a AirStandby) -> Self {
        Self {
            scout: standby.scout,
            artillery: &standby.artillery,
            strike_aircraft: &standby.strike_aircraft,
        }
    }
}

/// Mandatory continuation imported into the next allocation pass for an
/// already-admitted connected operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveConnectedObligation {
    pub(crate) membership: AirMembership,
    identity: crate::allocation::ConnectedOffenseKey,
    accepted_at: Tick,
    deadline: Tick,
    units: Vec<UnitId>,
    provider_jobs: Vec<ConnectedProviderJob>,
}

impl ActiveConnectedObligation {
    pub(crate) const fn identity(&self) -> crate::allocation::ConnectedOffenseKey {
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
struct FreshConnectedDerivationContext<'a> {
    ev: AirEvidence<'a>,
    inputs: ConnectedInputs<'a>,
    minimum_only: bool,
    campaign_routes: &'a CampaignRoutes<'a>,
    preferred_artillery: &'a [UnitId],
}

impl<'a> FreshConnectedDerivationContext<'a> {
    fn route(self, target: TilePos) -> ConnectedRouteContext<'a> {
        self.ev
            .route(target)
            .with_routes(Some(self.campaign_routes))
            .excluding_paid(self.inputs.paid_exclusions)
    }
}

fn connected_opportunity_case(
    observed_at: Tick,
    intel: &StrategicIntelligence,
    target: &BuildingContact,
    targets: &ConnectedTargetSelection,
    minimum: &ConnectedProposalVariant,
) -> ConnectedOpportunityCase {
    let cluster = sized_target_contacts_at_anchors(
        intel,
        target.player,
        &targets.target_anchors,
        observed_at,
    );
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
    ev: AirEvidence<'_>,
    reserve: CapitalReserve,
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
    if !planning.campaign_site_selected(ev.obs.tick, site, candidate_sites) {
        return None;
    }
    let mut prospective = ev.obs.clone();
    let cost = BuildingKind::Airworks.base_stats().construction?.cost;
    let bank = prospective.scrap.saturating_sub(reserve.current);
    let paid_now = bank.min(cost);
    let shortfall = cost - paid_now;
    prospective.scrap = bank - paid_now;
    prospective.my_buildings.push(candidate);
    prospective.my_queues.push(Vec::new());
    prospective.my_queue_progress.push(0);
    let resources = ResourceSnapshot::from_observation(&prospective);
    // The sizing bank excludes current promises; exact allocation imports them itself.
    let restored = reserve
        .current
        .min(current_reserve_at(obligations, prospective.tick))
        .min(ev.obs.scrap - bank);
    prospective.scrap += restored;
    let capacity = AllocationCapacity::from_snapshot(
        &ResourceSnapshot::from_observation(&prospective),
        deadline,
        ev.tuning.cadence,
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
        super::allocation::forecast::refine_obligations(&capacity, &obligations, planning),
        super::planning::Progress::Ready(())
    ) {
        return None;
    }
    prospective.scrap -= restored;
    let unavailable: Vec<_> = prospective.my_units.iter().map(|unit| unit.id).collect();
    let intel = ev.intel.assuming_remembered_buildings(prospective.tick);
    let campaign_routes = CampaignRoutes::new(&prospective, &intel, ev.public_map, ev.orientation);
    let context = FreshConnectedDerivationContext {
        ev: AirEvidence {
            obs: &prospective,
            intel: &intel,
            ..ev
        },
        inputs: ConnectedInputs {
            planning,
            resources: &resources,
            unavailable: &unavailable,
            paid_exclusions: &[],
            reserve: CapitalReserve {
                current: 0,
                forecast: reserve.forecast.saturating_add(shortfall),
            },
        },
        minimum_only: true,
        campaign_routes: &campaign_routes,
        preferred_artillery: &[],
    };
    let deadline = deadline.checked_sub(ready_after)?;
    if deadline <= prospective.tick {
        return None;
    }
    let confidence = |target: &BuildingContact| {
        ev.intel
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
    let distances = ev.public_map.map(|map| map.regions().distances(ev.home));
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
        let derived = derive_connected_proposal_with_resources(
            context,
            target,
            current_target_cluster(&intel, target.player, target.anchor),
            ConnectedProposalOrigin::Idle {
                standby: AirStandby::default(),
            },
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

/// Sizes a connected proposal against `target` with `candidates` as the
/// members its production resources may serve.
fn derive_connected_proposal_with_resources(
    context: FreshConnectedDerivationContext<'_>,
    target: &BuildingContact,
    candidates: Vec<&BuildingContact>,
    origin: ConnectedProposalOrigin,
    deadline: Tick,
) -> Result<FreshConnectedProposal, ConnectedPlanRejection> {
    let FreshConnectedDerivationContext {
        ev: AirEvidence { obs, intel, .. },
        inputs:
            ConnectedInputs {
                resources: resource_snapshot,
                unavailable,
                reserve,
                ..
            },
        ..
    } = context;
    let route = context.route(target.anchor);
    let initial_resources = ConnectedProductionResources::from_candidates(
        obs,
        target,
        candidates,
        unavailable,
        route,
        resource_snapshot,
        reserve.current,
    );
    let committed = match &origin {
        ConnectedProposalOrigin::Active { plan, .. } => Some(plan.commitment.key()),
        ConnectedProposalOrigin::Idle { .. } | ConnectedProposalOrigin::Remembered { .. } => None,
    };
    let packages = derive_connected_package_options(
        context,
        PackageBasis {
            committed,
            resources: &initial_resources,
            deadline,
        },
        target,
    )?;
    if packages.refinement_pending && committed.is_some() {
        return Err(ConnectedPlanRejection::Package {
            reason: ForcePackageRejection::Deferred,
            protected_current_scrap: reserve.current,
            protected_forecast_scrap: reserve.forecast,
        });
    }
    let resources = ConnectedProductionResources::from_package_snapshot_after_current_reserve(
        obs,
        target.player,
        &packages.minimum,
        route,
        resource_snapshot,
        reserve.current,
    );
    let commitment = match &origin {
        ConnectedProposalOrigin::Idle { .. } => {
            ConnectedCommitment::admit(target, &packages.minimum, obs.tick)
        }
        ConnectedProposalOrigin::Remembered { active } => {
            ConnectedCommitment::admit(target, &packages.minimum, active.plan.admitted_at())
        }
        ConnectedProposalOrigin::Active { plan, .. } => plan.commitment.clone(),
    };
    let route_unavailable =
        connected_provider_unavailable(obs, &resources.targets, unavailable, route);
    let mut variants = Vec::with_capacity(packages.marginal.len().saturating_add(1));
    for package in std::iter::once(packages.minimum).chain(packages.marginal) {
        let mut plan = ConnectedPlan::new(commitment.clone(), package);
        // The sizing primary is the focus whenever the focus is in current
        // sight, and otherwise the member the focus moves to.
        plan.focus = target.anchor;
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
        ConnectedProposalOrigin::Active { op, .. } => op.clone(),
    }
}

/// Fog-honest evidence for the connected force package's current revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectedPackageDiagnostics {
    pub(super) admitted_at: Tick,
    pub(super) derived_at: Tick,
    pub(super) preparation_deadline: Tick,
    pub(super) admitted_anchors: Vec<TilePos>,
    pub(super) live_anchors: Vec<TilePos>,
    pub(super) focus: TilePos,
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
    outcomes: super::experience::OutcomeJournal,
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
                    .saturating_add(active.plan.assembly_timeout(active.op.started_at))
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

    /// Committed preparation deadline of an admitted connected operation.
    pub(crate) fn connected_deadline(&self) -> Option<Tick> {
        Some(self.air.as_ref()?.plan.connected()?.commitment.deadline)
    }

    /// Connected-package evidence for opt-in decision traces.
    pub(super) fn connected_package_diagnostics(
        &self,
        intel: &StrategicIntelligence,
    ) -> Option<ConnectedPackageDiagnostics> {
        let connected = self.air.as_ref()?.plan.connected()?;
        let package = &connected.package;
        let mut live_anchors: Vec<_> = connected
            .commitment
            .live_members(intel)
            .into_iter()
            .map(|contact| contact.anchor)
            .collect();
        live_anchors.sort_unstable_by_key(|anchor| (anchor.y, anchor.x));
        live_anchors.dedup();
        Some(ConnectedPackageDiagnostics {
            admitted_at: connected.commitment.admitted_at,
            derived_at: package.derived_at,
            preparation_deadline: package.preparation_deadline,
            admitted_anchors: connected.commitment.anchors.clone(),
            live_anchors,
            focus: connected.focus,
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

    pub(super) fn terminal_outcome(&self) -> Option<AirOperationOutcome> {
        self.terminal_outcome
    }

    pub(crate) fn outcomes_mut(&mut self) -> &mut super::experience::OutcomeJournal {
        &mut self.outcomes
    }

    pub(crate) fn episode_id(&self) -> Option<super::experience::EpisodeId> {
        self.outcomes.episode_id()
    }

    pub(crate) fn owned_units(&self) -> impl Iterator<Item = UnitId> + '_ {
        self.air
            .iter()
            .flat_map(|active| {
                active
                    .op
                    .members()
                    .chain(active.plan.screen().iter().copied())
            })
            .chain(AirRoster::from(&self.standby).members())
    }

    /// Begins one decision's turn: releases dead standby members, refreshes
    /// the operation's target, aborts on current evidence, and settles the
    /// paid ledger. Every later read in the decision sees this state.
    pub(crate) fn observe<'a>(&'a mut self, ev: AirEvidence<'a>) -> AirTurn<'a> {
        let AirEvidence {
            profile,
            obs,
            intel,
            ..
        } = ev;
        self.standby.prune(obs);
        if let Some(active) = &mut self.air {
            refresh_target(&mut active.op, &active.plan, intel);
            if active.op.phase() != AirOperationPhase::Recover {
                abort_if_needed(&mut active.op, &active.plan, profile, obs, intel);
            }
        }
        self.prune_paid_production(obs);
        AirTurn { planner: self, ev }
    }

    fn apply_membership(&mut self, membership: AirMembership, now: Tick) {
        let active = self
            .air
            .as_mut()
            .expect("validated air operation remains active");
        let previous_scout = active.op.scout;
        let previous_artillery = core::mem::replace(&mut active.op.artillery, membership.artillery);
        let previous_strike =
            core::mem::replace(&mut active.op.strike_aircraft, membership.strike_aircraft);
        active.op.scout = membership.scout;
        if let Some(screen) = active.plan.screen_mut() {
            *screen = membership.screen;
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

    /// The remembered objective of an unadmitted reconnaissance watch. Only
    /// such an objective may hold a prospective first carrier's capital.
    pub(crate) fn remembered_recon_target<'i>(
        &self,
        intel: &'i StrategicIntelligence,
    ) -> Option<&'i BuildingContact> {
        self.air
            .as_ref()
            .filter(|active| unadmitted_recon(&active.op))
            .and_then(|active| remembered_objective(&active.op, intel))
    }

    pub(crate) fn has_active_island_operation(&self) -> bool {
        self.air.as_ref().is_some_and(|active| {
            active.op.assault_admitted() && matches!(active.plan, AirPlan::Island(_))
        })
    }

    /// Installs the exact proposal selected by cross-domain adjudication. No
    /// observation is accepted here, so commitment cannot rerank its target,
    /// rebuild its package, or change its producer basis.
    fn commit_connected(&mut self, proposal: FreshConnectedProposal) {
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
    fn prune_paid_production(&mut self, obs: &Observation) {
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

    /// Releases unpaid connected demand during emergency economy recovery
    /// and returns every still-routable member immediately. The operation
    /// stays in recovery even with no survivors: this path returns before Lift
    /// runs, so the next full decision settles it and hands the abort to Lift.
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
        // This path returns before the normal observation would prune.
        self.prune_paid_production(obs);
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

        let ActiveAirOperation { op, plan } = self
            .air
            .as_mut()
            .expect("unpaid connected demand belongs to one active operation");
        recover(op, AirRecoveryReason::PreparationInfeasible, obs.tick);
        self.cooldown_until = obs.tick.saturating_add(cooldown(profile, tuning));
        let mut out = StrategicDecision::default();
        reconcile_recovery_return(
            op,
            plan,
            RecoveryReturnContext {
                obs,
                home,
                public_map,
                orientation,
                issue_order: true,
            },
            &mut out,
        );
        out.reservations = reservations(op, plan, obs);
        Some(out)
    }

    /// Only purchases emitted now cross from forecast evidence into ownership.
    fn record_connected_purchases(
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

    fn think(&mut self, ev: AirEvidence<'_>, inputs: ThinkInputs<'_>) -> StrategicDecision {
        let AirEvidence {
            profile,
            obs,
            intel,
            home,
            public_map,
            ..
        } = ev;
        let mut active = match self.begin_or_resume(ev, &inputs) {
            Ok(active) => active,
            Err(idle) => return idle,
        };
        let objective_gone = self.watch_outcomes(obs, intel, &active);
        let ActiveAirOperation { op, plan } = &mut active;
        if let Some(screen) = plan.screen_mut() {
            screen.retain(|id| {
                unit(obs, *id)
                    .is_some_and(|member| member.kind == Role::AirGround.unit_for(obs.faction))
            });
        }
        let owned = reservations(op, plan, obs);
        if owned.iter().any(|id| inputs.claimed_elsewhere.contains(id)) {
            self.air = Some(active);
            return StrategicDecision {
                reservations: owned,
                ..Default::default()
            };
        }
        let began_in_recovery = op.phase() == AirOperationPhase::Recover;
        refresh_target(op, plan, intel);
        if !op.assault_admitted()
            && strategic_admission_tick(obs.tick)
            && let Some(current_target) = current_target_contact(op, intel)
        {
            let admitted_at = plan.admitted_at();
            if wealthy_island_target(profile, obs, home, current_target, public_map) {
                let mut admitted = IslandPlan::new(profile, obs, inputs.procurement.lanes);
                op.admit_assault(obs.tick);
                admitted.admitted_at = admitted_at;
                *plan = AirPlan::Island(admitted);
            }
        }
        if op.assault_admitted()
            && op.phase() <= AirOperationPhase::Assemble
            && let AirPlan::Connected(connected) = plan
        {
            connected.refocus(intel);
        }
        if !began_in_recovery && op.phase() != AirOperationPhase::Recover {
            abort_if_needed(op, plan, profile, obs, intel);
        }

        let mut out = StrategicDecision::default();
        let landing_sites = landing_sites(inputs.lift_support, op);
        let owned = reservations(op, plan, obs);
        let mut enlisted = excluding_owned(inputs.procurement.unavailable, &owned);
        if inputs.owned_only {
            enlisted.extend(
                obs.my_units
                    .iter()
                    .filter_map(|unit| (!owned.contains(&unit.id)).then_some(unit.id)),
            );
            enlisted.sort_unstable();
            enlisted.dedup();
        }
        let context = planning_context(
            ev,
            AirProcurement {
                unavailable: &enlisted,
                ..inputs.procurement
            },
            op,
            plan,
            &landing_sites,
        );
        let staged = match op.stage {
            AirStage::Watching => remembered_recon(op, plan, &context, &mut out),
            AirStage::Recon => recon(op, plan, &context, &mut out),
            AirStage::Assemble => assemble(op, plan, &context, &mut out),
            AirStage::SuppressAa => suppress(op, plan, &context, &mut out),
            AirStage::Verify => verify(op, plan, &context, &mut out),
            AirStage::Strike => strike(op, plan, &context, &mut out),
            AirStage::Recover { .. } => Ok(()),
        };
        let end = self.finish_stage(
            ev,
            &inputs,
            began_in_recovery,
            staged,
            &mut active,
            &mut out,
        );
        if let Some(reason) = active.op.recovery_reason() {
            let (outcome, reason, confidence, doctrine) =
                recovery_outcome(reason, objective_gone, &active.op, &self.outcomes, obs);
            self.outcomes
                .finish(obs, outcome, reason, confidence, doctrine);
        }
        match end {
            OperationEnd::Settled
                if !out.reservations.is_empty()
                    && reusable_survivors(active.op.recovery_reason()) =>
            {
                self.standby = AirStandby::from_operation(&active.op, obs);
                self.terminal_outcome = Some(air_operation_outcome(&active.op));
            }
            OperationEnd::Settled | OperationEnd::Released => {
                self.terminal_outcome = Some(air_operation_outcome(&active.op));
            }
            OperationEnd::Continue => self.air = Some(active),
        }
        out
    }

    /// The operation this observation continues or begins, or the idle
    /// decision when there is none.
    fn begin_or_resume(
        &mut self,
        ev: AirEvidence<'_>,
        inputs: &ThinkInputs<'_>,
    ) -> Result<ActiveAirOperation, StrategicDecision> {
        self.terminal_outcome = None;
        self.standby.prune(ev.obs);
        let holding = |standby: &AirStandby| StrategicDecision {
            reservations: standby.reservations(),
            ..StrategicDecision::default()
        };
        if ev.intel.observed_at() != Some(ev.obs.tick) {
            return Err(holding(&self.standby));
        }
        if let Some(active) = self.air.take() {
            return Ok(active);
        }
        match self.admission(ev, inputs.procurement.allow, inputs.lift_support) {
            AirAdmission::Hold => Err(holding(&self.standby)),
            AirAdmission::NoTarget => {
                self.standby = AirStandby::default();
                Err(StrategicDecision::default())
            }
            AirAdmission::Begin(selected) => {
                let standby = core::mem::take(&mut self.standby);
                Ok(fresh_air_operation(
                    ev.profile,
                    ev.obs,
                    inputs.procurement.lanes,
                    selected,
                    standby,
                ))
            }
        }
    }

    /// Whether a fresh operation may begin on this observation. Reads only
    /// planner state, so the carrier preview and the lifecycle share it.
    fn admission<'i>(
        &self,
        ev: AirEvidence<'i>,
        allow_new_operation: bool,
        lift_support: Option<&LiftSupportRequest>,
    ) -> AirAdmission<'i> {
        let AirEvidence {
            profile,
            tuning,
            obs,
            intel,
            home,
            public_map,
            ..
        } = ev;
        if !allow_new_operation || obs.tick < self.cooldown_until {
            return AirAdmission::Hold;
        }
        let Some(selected) =
            select_fresh_air_target(profile, tuning, obs, intel, home, lift_support, public_map)
        else {
            return AirAdmission::NoTarget;
        };
        if !strategic_admission_tick(obs.tick) {
            return AirAdmission::Hold;
        }
        AirAdmission::Begin(selected)
    }

    /// Opens or continues the journal episode and reports whether current
    /// sight confirms the objective gone.
    fn watch_outcomes(
        &mut self,
        obs: &Observation,
        intel: &StrategicIntelligence,
        ActiveAirOperation { op, plan }: &ActiveAirOperation,
    ) -> bool {
        use super::experience::{
            Doctrine, EpisodeId, EpisodeOwner, ExperienceKey, ExperienceSubject,
        };
        let members: Vec<_> = op.members().chain(plan.screen().iter().copied()).collect();
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
        match plan {
            AirPlan::Connected(connected) => {
                let commitment = &connected.commitment;
                let live_anchors: Vec<_> = commitment
                    .live_members(intel)
                    .into_iter()
                    .map(|member| member.anchor)
                    .collect();
                self.outcomes.observe_objective(obs, commitment.primary);
                self.outcomes.observe_cluster(
                    obs,
                    commitment.player,
                    &commitment.anchors,
                    &live_anchors,
                )
            }
            AirPlan::Reacquire(_) | AirPlan::Island(_) => op
                .target_id
                .is_some_and(|id| self.outcomes.observe_objective(obs, id)),
        }
    }

    /// Applies the stage verdict, the preparation deadline, and closed
    /// funding, then issues the one return order when recovery begins.
    fn finish_stage(
        &mut self,
        ev: AirEvidence<'_>,
        inputs: &ThinkInputs<'_>,
        began_in_recovery: bool,
        staged: Result<(), AirRecoveryReason>,
        ActiveAirOperation { op, plan }: &mut ActiveAirOperation,
        out: &mut StrategicDecision,
    ) -> OperationEnd {
        let obs = ev.obs;
        if let Err(reason) = staged {
            out.intents.clear();
            recover(op, reason, obs.tick);
        }
        let preparation_expired = plan.package().is_some_and(|package| {
            op.phase() <= AirOperationPhase::Assemble
                && obs.tick >= package.preparation_deadline
                && !assembly_complete(op, plan)
        });
        if preparation_expired {
            out.intents.clear();
            out.reserved_scrap = 0;
            recover(op, AirRecoveryReason::Timeout, obs.tick);
        }
        if !inputs.procurement.allow {
            out.intents
                .retain(|intent| !matches!(intent, Intent::TrainAt { .. }));
            out.reserved_scrap = 0;
        }
        let recovery_entered_this_tick = op.phase() == AirOperationPhase::Recover
            && (!began_in_recovery || op.phase_started_at == obs.tick);
        if op.phase() == AirOperationPhase::Recover {
            if recovery_entered_this_tick {
                self.cooldown_until = obs.tick.saturating_add(cooldown(ev.profile, ev.tuning));
            }
            reconcile_recovery_return(
                op,
                plan,
                RecoveryReturnContext {
                    obs,
                    home: ev.home,
                    public_map: ev.public_map,
                    orientation: ev.orientation,
                    issue_order: recovery_entered_this_tick,
                },
                out,
            );
        }
        out.reservations = reservations(op, plan, obs);
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
        if settled {
            OperationEnd::Settled
        } else if op.phase() == AirOperationPhase::Recover
            && (out.reservations.is_empty() || elapsed(op.phase_started_at, obs.tick) >= 500)
        {
            OperationEnd::Released
        } else {
            OperationEnd::Continue
        }
    }
}

/// Whether the lifecycle may begin a fresh operation this observation.
enum AirAdmission<'i> {
    /// Nothing may begin now; standby members stay reserved.
    Hold,
    /// No target justifies an operation, so standby members are released.
    NoTarget,
    Begin(FreshAirTarget<'i>),
}

/// How one lifecycle pass leaves the active operation.
enum OperationEnd {
    Continue,
    /// Recovery finished: every survivor was lost or released, or it ran out
    /// of time.
    Released,
    /// Every survivor of a dispatched return has come to rest.
    Settled,
}

/// Journal verdict for an operation that entered recovery for `reason`.
fn recovery_outcome(
    reason: AirRecoveryReason,
    objective_gone: bool,
    op: &AirOperation,
    journal: &super::experience::OutcomeJournal,
    obs: &Observation,
) -> (Outcome, OutcomeReason, u16, bool) {
    match reason {
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
                && !journal.has_progress()
                && journal.own_lost_value(obs) == 0 =>
        {
            (Outcome::Aborted, OutcomeReason::Deadline, 1000, false)
        }
        AirRecoveryReason::Timeout => (Outcome::Ineffective, OutcomeReason::Deadline, 750, false),
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
    }
}

/// Planned drops of a Lift waiting on this operation's exact objective.
fn landing_sites(lift_support: Option<&LiftSupportRequest>, op: &AirOperation) -> Vec<TilePos> {
    lift_support
        .filter(|request| request.player == op.target_player && request.target == op.target)
        .map_or_else(Vec::new, |request| request.planned_drops.clone())
}

fn planning_context<'c>(
    ev: AirEvidence<'c>,
    procurement: AirProcurement<'c>,
    op: &AirOperation,
    plan: &AirPlan,
    landing_sites: &'c [TilePos],
) -> AirPlanningContext<'c> {
    let connected_resources = plan
        .connected()
        .filter(|_| op.phase() <= AirOperationPhase::Assemble)
        .map(|connected| {
            ConnectedProductionResources::from_package_snapshot_after_current_reserve(
                ev.obs,
                connected.commitment.player,
                &connected.package,
                ev.route(connected.focus)
                    .excluding_paid(procurement.paid_exclusions),
                &ResourceSnapshot::from_observation(ev.obs),
                procurement.reserve.current,
            )
        });
    AirPlanningContext {
        ev,
        procurement,
        landing_sites,
        connected_resources,
    }
}

/// One decision's exclusive access to the air planner, bound to the evidence
/// it observed. Proposals read the planner; only the named transitions change
/// it, in the order allocation reaches them.
pub(crate) struct AirTurn<'a> {
    planner: &'a mut StrategicPlanner,
    ev: AirEvidence<'a>,
}

impl core::ops::Deref for AirTurn<'_> {
    type Target = StrategicPlanner;

    fn deref(&self) -> &StrategicPlanner {
        self.planner
    }
}

/// What the accepted allocation settles for the air planner.
pub(crate) enum AirAdjudication {
    Unchanged,
    /// Retained membership the allocation validated.
    Members(AirMembership),
    /// The accepted fresh proposal or active revision.
    Connected(Box<FreshConnectedProposal>),
}

/// The accepted connected proposal, otherwise the membership of the retained
/// connected or island operation the allocation imported.
pub(crate) fn air_adjudication(
    connected: Option<FreshConnectedProposal>,
    active_connected: Option<&ActiveConnectedObligation>,
    island: Option<&IslandPreparation>,
) -> AirAdjudication {
    if let Some(proposal) = connected {
        return AirAdjudication::Connected(Box::new(proposal));
    }
    active_connected
        .map(|active| &active.membership)
        .or_else(|| island.map(|island| &island.membership))
        .map_or(AirAdjudication::Unchanged, |membership| {
            AirAdjudication::Members(membership.clone())
        })
}

impl<'a> AirTurn<'a> {
    /// Lends this turn to a shorter-lived owner such as one allocation pass.
    pub(crate) fn reborrow(&mut self) -> AirTurn<'_> {
        AirTurn {
            planner: self.planner,
            ev: self.ev,
        }
    }

    /// Reconstructs unpaid demand from the retained package and current inventory.
    pub(crate) fn retained_obligation(
        &self,
        resources: &ResourceSnapshot,
        unavailable: &[UnitId],
        paid_exclusions: &[PaidQueueClaim],
    ) -> Option<ActiveConnectedObligation> {
        let active = self
            .air
            .as_ref()
            .filter(|active| active.op.assault_admitted())?;
        let connected = active.plan.connected()?;
        let package = &connected.package;
        let mut membership = AirMembership::from_active(active);
        let obs = self.ev.obs;
        let provider_jobs = if active.op.phase() <= AirOperationPhase::Assemble
            && obs.tick < package.preparation_deadline
            && operation_recovery_reason(
                &active.op,
                &active.plan,
                self.ev.profile,
                obs,
                self.ev.intel,
            )
            .is_none()
        {
            let route = self
                .ev
                .route(connected.focus)
                .excluding_paid(paid_exclusions);
            let resources =
                ConnectedProductionResources::from_package_snapshot_after_current_reserve(
                    obs,
                    connected.commitment.player,
                    package,
                    route,
                    resources,
                    0,
                );
            if active.op.phase() <= AirOperationPhase::Assemble
                && active.op.membership_frozen_at.is_none()
            {
                let owned = reservations(&active.op, &active.plan, obs);
                let unavailable = excluding_owned(unavailable, &owned);
                let mut unavailable =
                    connected_provider_unavailable(obs, &resources.targets, &unavailable, route);
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
                AirRoster::from(&membership),
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
                    eligible_producers: eligible_producers(
                        &resources.snapshot,
                        &resources.access,
                        demand.kind,
                        Some(package.preparation_deadline),
                    ),
                };
                std::iter::repeat_n(job, demand.count)
            })
            .collect()
        } else {
            Vec::new()
        };
        Some(ActiveConnectedObligation {
            identity: connected.commitment.key(),
            accepted_at: connected.commitment.admitted_at,
            deadline: package.preparation_deadline,
            units: membership.units(obs),
            membership,
            provider_jobs,
        })
    }

    pub(crate) fn island_preparation(
        &self,
        procurement: AirProcurement<'_>,
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
        let obs = self.ev.obs;
        let mut membership = AirMembership::from_active(active);
        let mut purchases = ProductionPlan::default();
        if op.phase() <= AirOperationPhase::Assemble {
            let unavailable =
                excluding_owned(procurement.unavailable, &reservations(op, plan, obs));
            let scout = Role::Scout.unit_for(obs.faction);
            membership.scout = retained_scout(membership.scout, obs, &unavailable);
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
                ev: self.ev,
                procurement: AirProcurement {
                    unavailable: &unavailable,
                    ..procurement
                },
                landing_sites: &[],
                connected_resources: None,
            };
            let demands = missing_island_members(
                AirRoster::from(&membership),
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

    /// Re-derives one admitted connected operation from current evidence while
    /// its membership remains revisable. The fixed preparation deadline and
    /// the committed identity and target set are retained; the package is
    /// sized against the committed members only.
    pub(crate) fn connected_revision(
        &self,
        inputs: ConnectedInputs<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let AirEvidence {
            profile,
            obs,
            intel,
            public_map,
            orientation,
            ..
        } = self.ev;
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
            || obs.tick > connected.commitment.deadline
        {
            return Ok(None);
        }
        let Some(target) = best_current_member(&connected.commitment, connected.focus, intel)
        else {
            return Ok(None);
        };
        let owned = reservations(&active.op, &active.plan, obs);
        let unavailable = excluding_owned(inputs.unavailable, &owned);
        let campaign_routes = CampaignRoutes::new(obs, intel, public_map, orientation);
        let context = FreshConnectedDerivationContext {
            ev: self.ev,
            inputs: ConnectedInputs {
                unavailable: &unavailable,
                ..inputs
            },
            minimum_only: false,
            campaign_routes: &campaign_routes,
            preferred_artillery: &active.op.artillery,
        };
        // A revision sizes the admitted members only; it never searches the
        // radius around its current primary again.
        let proposal = derive_connected_proposal_with_resources(
            context,
            target,
            connected.commitment.sized_members(intel, obs.tick),
            ConnectedProposalOrigin::Active {
                op: active.op.clone(),
                plan: connected.clone(),
            },
            connected.commitment.deadline,
        )
        .map_err(|reason| RejectedConnectedCandidate {
            target: target.clone(),
            reason,
        })?;
        // Tactics act on every live committed member, so a revision that could
        // not size one of them keeps the current package instead.
        let sized = connected.commitment.sized_members(intel, obs.tick);
        if !proposal.variants.iter().all(|variant| {
            sized
                .iter()
                .all(|member| variant.plan.package.target_anchors.contains(&member.anchor))
        }) {
            return Ok(None);
        }
        Ok(Some(proposal))
    }

    /// Proposes one exact common-minimum connected assault without mutating
    /// planner state. Island admission and an already-admitted assault remain
    /// on the ordinary lifecycle path.
    pub(crate) fn fresh_connected(
        &self,
        experience: &super::experience::Experience,
        inputs: ConnectedInputs<'_>,
    ) -> Result<Option<FreshConnectedProposal>, RejectedConnectedCandidate> {
        let AirEvidence {
            profile,
            tuning,
            obs,
            intel,
            home,
            public_map,
            orientation,
        } = self.ev;
        if intel.observed_at() != Some(obs.tick)
            || !strategic_admission_tick(obs.tick)
            || obs.tick < self.cooldown_until
        {
            return Ok(None);
        }

        if let Some(active) = &self.air {
            if active.op.assault_admitted() {
                return Ok(None);
            }
            let mut refreshed = active.clone();
            refresh_target(&mut refreshed.op, &refreshed.plan, intel);
            let Some(target) = current_target_contact(&refreshed.op, intel) else {
                return Ok(None);
            };
            if wealthy_island_target(profile, obs, home, target, public_map) {
                return Ok(None);
            }
            let owned = reservations(&refreshed.op, &refreshed.plan, obs);
            let unavailable = excluding_owned(inputs.unavailable, &owned);
            let campaign_routes = CampaignRoutes::new(obs, intel, public_map, orientation);
            return derive_connected_proposal_with_resources(
                FreshConnectedDerivationContext {
                    ev: self.ev,
                    inputs: ConnectedInputs {
                        unavailable: &unavailable,
                        ..inputs
                    },
                    minimum_only: false,
                    campaign_routes: &campaign_routes,
                    preferred_artillery: &refreshed.op.artillery,
                },
                target,
                current_target_cluster(intel, target.player, target.anchor),
                ConnectedProposalOrigin::Remembered {
                    active: active.clone(),
                },
                obs.tick.saturating_add(CONNECTED_PREPARATION_HORIZON),
            )
            .map(Some)
            .map_err(|reason| RejectedConnectedCandidate {
                target: target.clone(),
                reason,
            });
        }

        if select_wealthy_island_target(profile, obs, home, intel, public_map).is_some() {
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
        let unavailable = excluding_owned(inputs.unavailable, &standby.reservations());
        let origin = ConnectedProposalOrigin::Idle {
            standby: self.standby.clone(),
        };
        let mut first_rejection = None;
        let campaign_routes = CampaignRoutes::new(obs, intel, public_map, orientation);
        for target in current {
            match derive_connected_proposal_with_resources(
                FreshConnectedDerivationContext {
                    ev: self.ev,
                    inputs: ConnectedInputs {
                        unavailable: &unavailable,
                        ..inputs
                    },
                    minimum_only: false,
                    campaign_routes: &campaign_routes,
                    preferred_artillery: &standby.artillery,
                },
                target,
                current_target_cluster(intel, target.player, target.anchor),
                origin.clone(),
                obs.tick.saturating_add(CONNECTED_PREPARATION_HORIZON),
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

    /// Returns the exact remembered reconnaissance target that the ordinary
    /// post-allocation lifecycle will retain or admit on this observation.
    /// This preview is read-only so shared allocation can preserve capital
    /// needed by the immediately following Lift handoff.
    pub(crate) fn prospective_recon_target(
        &self,
        unavailable: &[UnitId],
        lift_support: Option<&LiftSupportRequest>,
    ) -> Option<&'a BuildingContact> {
        let AirEvidence {
            profile,
            obs,
            intel,
            public_map,
            ..
        } = self.ev;
        if intel.observed_at() != Some(obs.tick) {
            return None;
        }
        let ActiveAirOperation { mut op, plan } = match &self.air {
            Some(active) if unadmitted_recon(&active.op) => active.clone(),
            // Target selection is skipped when nothing could begin anyway.
            None if strategic_admission_tick(obs.tick) => {
                let AirAdmission::Begin(selected) = self.admission(self.ev, true, lift_support)
                else {
                    return None;
                };
                let mut standby = self.standby.clone();
                standby.prune(obs);
                fresh_air_operation(profile, obs, ProducerLanes::empty(), selected, standby)
            }
            Some(_) | None => return None,
        };
        refresh_target(&mut op, &plan, intel);
        let target = remembered_objective(&op, intel)?;
        if operation_recovery_reason(&op, &plan, profile, obs, intel).is_some() {
            return None;
        }
        let owned = reservations(&op, &plan, obs);
        op.scout = remembered_recon_scout(&op, obs, &excluding_owned(unavailable, &owned));
        reachable_scout_goal(
            &op,
            &plan,
            obs,
            intel,
            &landing_sites(lift_support, &op),
            connected_public_map(&plan, public_map),
        )
        .map(|_| target)
    }

    /// Moves the admitted connected operation into bounded recovery as soon as
    /// preparation proves it cannot continue, so every later read in this
    /// allocation sees the recovery phase. The post-allocation think owns the
    /// one return-home order.
    pub(crate) fn recover_connected(&mut self, reason: AirRecoveryReason) {
        let active = self
            .planner
            .air
            .as_mut()
            .expect("an active connected obligation can only come from its planner");
        debug_assert!(active.op.assault_admitted());
        debug_assert!(matches!(active.plan, AirPlan::Connected(_)));
        recover(&mut active.op, reason, self.ev.obs.tick);
    }

    /// Installs the accepted allocation's verdict. Only purchases emitted now
    /// cross from forecast evidence into the connected ledger.
    pub(crate) fn apply(
        &mut self,
        verdict: AirAdjudication,
        schedule: &[crate::allocation::ScheduledProducerJob],
    ) {
        let now = self.ev.obs.tick;
        match verdict {
            AirAdjudication::Unchanged => {}
            AirAdjudication::Members(membership) => self.planner.apply_membership(membership, now),
            AirAdjudication::Connected(proposal) => self.planner.commit_connected(*proposal),
        }
        self.planner.record_connected_purchases(schedule, now);
    }

    /// Runs the ordinary tactical lifecycle after the coordinator has already
    /// accepted or rejected the fresh connected-offense proposal for this
    /// observation. Island and remembered reconnaissance behavior is unchanged.
    pub(crate) fn think(&mut self, inputs: ThinkInputs<'_>) -> StrategicDecision {
        self.planner.think(self.ev, inputs)
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
    pub(crate) recon_paid_exclusions: &'a [PaidQueueClaim],
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
) -> Result<(), AirRecoveryReason> {
    let obs = context.ev.obs;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let previous_scout = op.scout;
    op.scout = remembered_recon_scout(op, obs, context.procurement.unavailable);
    if op.scout != previous_scout {
        op.scout_dispatch = None;
        if op.scout.is_some() {
            op.phase_started_at = obs.tick;
        }
    }
    dispatch_scout(
        op,
        plan,
        obs,
        context.ev.intel,
        context.landing_sites,
        connected_public_map(plan, context.ev.public_map),
        out,
    )?;
    schedule(
        context,
        &[(
            scout_kind,
            usize::from(op.scout.is_none())
                .saturating_sub(unowned_queued_scouts(context, scout_kind)),
        )],
    )
    .append_to(out);
    Ok(())
}

fn unowned_queued_scouts(context: &AirPlanningContext<'_>, scout: UnitKind) -> usize {
    let prior = context
        .procurement
        .lanes
        .prior_intents
        .iter()
        .filter(|intent| matches!(intent, Intent::TrainAt { kind, .. } if *kind == scout))
        .count();
    let unavailable = context
        .procurement
        .paid_exclusions
        .iter()
        .filter(|claim| claim.kind == scout)
        .count();
    queued(context.ev.obs, |kind| kind == scout)
        .saturating_add(prior)
        .saturating_sub(unavailable)
}

fn reconcile_preparation_members(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
) -> Result<(), AirRecoveryReason> {
    let obs = context.ev.obs;
    let route_unavailable = if let Some(resources) = context.connected_resources.as_ref() {
        connected_provider_unavailable(
            obs,
            &resources.targets,
            &[],
            context.ev.route(preferred_anchor(op, plan)),
        )
    } else {
        Vec::new()
    };
    let unavailable = merged_unavailable(context.procurement.unavailable, &route_unavailable);
    let previous_scout = op.scout;
    let previous_artillery = op.artillery.clone();
    let previous_strike_aircraft = op.strike_aircraft.clone();
    op.scout = retained_scout(op.scout, obs, &unavailable);
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
        return Err(AirRecoveryReason::UnreachableAirRoute);
    }
    if previous_artillery
        .iter()
        .any(|id| unit(obs, *id).is_some() && route_unavailable.binary_search(id).is_ok())
        && op.artillery.len() < plan.desired_artillery()
    {
        return Err(AirRecoveryReason::UnreachableStaging);
    }
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    let desired_screen = plan.desired_screen();
    if let Some(screen) = plan.screen_mut() {
        assign_exact(
            screen,
            desired_screen,
            obs,
            context.procurement.unavailable,
            |kind| kind == screen_kind,
        );
    }
    if connected_package_is_proven_infeasible(op, plan, context) {
        return Err(AirRecoveryReason::PreparationInfeasible);
    }
    Ok(())
}

fn recon(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let AirEvidence {
        tuning, obs, intel, ..
    } = context.ev;
    let landing_sites = context.landing_sites;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let public_map = connected_public_map(plan, context.ev.public_map);
    reconcile_preparation_members(op, plan, context)?;
    dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out)?;
    schedule_missing_members(op, plan, context, scout_kind, out);
    if matches!(plan, AirPlan::Connected(_)) {
        hold_strike_aircraft(op, obs, context.ev.home, out);
    }
    if op.scout_dispatch.is_some()
        && target_seen(op, plan, obs)
        && elapsed(op.phase_started_at, obs.tick) >= tuning.reaction_delay
    {
        enter(op, AirStage::Assemble, obs.tick);
    }
    Ok(())
}

fn assemble(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let AirEvidence {
        obs, intel, home, ..
    } = context.ev;
    let landing_sites = context.landing_sites;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let public_map = connected_public_map(plan, context.ev.public_map);
    reconcile_preparation_members(op, plan, context)?;
    schedule_missing_members(op, plan, context, scout_kind, out);
    let complete = assembly_complete(op, plan);
    if matches!(plan, AirPlan::Connected(_)) && !complete {
        hold_strike_aircraft(op, obs, home, out);
    }
    if complete {
        if plan.airborne() {
            dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out)?;
            enter(op, AirStage::SuppressAa, obs.tick);
            hold_air_strike(op, plan, obs, home, out);
            return Ok(());
        }
        let objective = operation_objective_anchor(op, plan, intel);
        let staging = match artillery_staging(
            op,
            obs,
            home,
            objective,
            context.ev.public_map,
            context.ev.orientation,
        ) {
            None => return Err(AirRecoveryReason::UnreachableStaging),
            Some(ArtilleryStaging::NeedsRecon(goal)) => {
                dispatch_scout_to(op, obs, goal, public_map, out)?;
                hold_strike_aircraft(op, obs, home, out);
                return Ok(());
            }
            Some(ArtilleryStaging::Ready(staging)) => staging,
        };
        dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out)?;
        enter(op, AirStage::SuppressAa, obs.tick);
        stage_artillery(op, staging, out);
        hold_strike_aircraft(op, obs, home, out);
    }
    Ok(())
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
) -> Result<(), AirRecoveryReason> {
    let tuning = context.ev.tuning;
    let obs = context.ev.obs;
    let intel = context.ev.intel;
    let home = context.ev.home;
    let landing_sites = context.landing_sites;
    let public_map = connected_public_map(plan, context.ev.public_map);
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
            context.ev.orientation,
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
        if plan.airborne() {
            dispatch_scout(op, plan, obs, intel, landing_sites, public_map, out)
        } else {
            scout_and_hold(op, plan, context, &[], out)
        }
    } else {
        plan.set_suppression_dispatch(None);
        if plan.airborne() {
            return match airborne_corridor_status(op, plan, obs, intel, home, landing_sites) {
                AirborneCorridorStatus::Defended => Err(AirRecoveryReason::NewAirDefense),
                AirborneCorridorStatus::Clear => {
                    enter(op, AirStage::Verify, obs.tick);
                    scout_and_hold(op, plan, context, landing_sites, out)
                }
                AirborneCorridorStatus::NeedsRecon => {
                    scout_and_hold(op, plan, context, landing_sites, out)
                }
            };
        }
        match cluster_aa
            .expect("connected suppression has a cluster assessment")
            .evidence
        {
            AirDefenseEvidence::CurrentCoverage => Err(AirRecoveryReason::NewAirDefense),
            AirDefenseEvidence::VisibleWithoutKnownCoverage
                if corridor_clear(
                    intel,
                    home,
                    operation_objective_anchor(op, plan, intel),
                    &[],
                ) =>
            {
                enter(op, AirStage::Verify, obs.tick);
                scout_and_hold(op, plan, context, &[], out)
            }
            AirDefenseEvidence::RememberedCoverage
            | AirDefenseEvidence::Unknown
            | AirDefenseEvidence::VisibleWithoutKnownCoverage => {
                scout_and_hold(op, plan, context, &[], out)
            }
        }
    }
}

/// The connected cluster's anti-air assessment and the anti-air target the
/// operation must suppress first, if any.
fn stage_air_defense(
    op: &AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
) -> (Option<ClusterAirDefense>, Option<Target>) {
    if plan.airborne() {
        let flak = targetable_corridor_flak(
            context.ev.intel,
            context.ev.home,
            op.target,
            context.landing_sites,
        );
        (None, flak.map(Target::Building))
    } else {
        let assessment = cluster_air_defense(op, plan, context.ev.intel);
        (Some(assessment), assessment.targetable)
    }
}

fn verify(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let tuning = context.ev.tuning;
    let obs = context.ev.obs;
    let intel = context.ev.intel;
    let home = context.ev.home;
    let landing_sites = context.landing_sites;
    let (cluster_aa, air_defense) = stage_air_defense(op, plan, context);
    if air_defense.is_some() {
        enter(op, AirStage::SuppressAa, obs.tick);
        return suppress(op, plan, context, out);
    }
    if plan.airborne() {
        return match airborne_corridor_status(op, plan, obs, intel, home, landing_sites) {
            AirborneCorridorStatus::Defended => Err(AirRecoveryReason::NewAirDefense),
            AirborneCorridorStatus::Clear
                if elapsed(op.phase_started_at, obs.tick)
                    >= tuning
                        .reaction_delay
                        .saturating_add(tuning.commitment_hesitation) =>
            {
                enter(op, AirStage::Strike, obs.tick);
                strike(op, plan, context, out)
            }
            AirborneCorridorStatus::Clear | AirborneCorridorStatus::NeedsRecon => {
                scout_and_hold(op, plan, context, landing_sites, out)
            }
        };
    }
    match cluster_aa
        .expect("connected verification has a cluster assessment")
        .evidence
    {
        AirDefenseEvidence::CurrentCoverage => Err(AirRecoveryReason::NewAirDefense),
        AirDefenseEvidence::VisibleWithoutKnownCoverage
            if corridor_clear(
                intel,
                home,
                operation_objective_anchor(op, plan, intel),
                &[],
            ) && elapsed(op.phase_started_at, obs.tick)
                >= tuning
                    .reaction_delay
                    .saturating_add(tuning.commitment_hesitation) =>
        {
            enter(op, AirStage::Strike, obs.tick);
            strike(op, plan, context, out)
        }
        AirDefenseEvidence::RememberedCoverage
        | AirDefenseEvidence::Unknown
        | AirDefenseEvidence::VisibleWithoutKnownCoverage => {
            scout_and_hold(op, plan, context, &[], out)
        }
    }
}

fn strike(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let tuning = context.ev.tuning;
    let obs = context.ev.obs;
    let intel = context.ev.intel;
    let home = context.ev.home;
    let landing_sites = context.landing_sites;
    let public_map = connected_public_map(plan, context.ev.public_map);
    let (cluster_aa, air_defense) = stage_air_defense(op, plan, context);
    let connected_cluster_needs_clearance = cluster_aa.is_some_and(|assessment| {
        assessment.has_targets && assessment.evidence == AirDefenseEvidence::CurrentCoverage
    });
    if air_defense.is_some() || connected_cluster_needs_clearance {
        enter(op, AirStage::SuppressAa, obs.tick);
        return suppress(op, plan, context, out);
    }
    let strike_settled = op
        .strike_issued_at
        .is_some_and(|tick| elapsed(tick, obs.tick) >= tuning.reaction_delay.max(20));
    if matches!(plan, AirPlan::Connected(_))
        && strike_settled
        && operation_objective_cleared(op, plan, obs, intel)
    {
        // Every admitted member was observed gone, so completion needs no
        // renewed sight of their anchors or approach.
        return Err(AirRecoveryReason::Complete);
    }
    let live_target = live_strike_target(op, plan, intel);
    let remembered_target = live_target
        .is_none()
        .then(|| best_remembered_member(plan, intel))
        .flatten();
    let strike_anchor = operation_objective_anchor(op, plan, intel);
    let staging = if plan.airborne() {
        None
    } else {
        match artillery_staging(
            op,
            obs,
            home,
            strike_anchor,
            context.ev.public_map,
            context.ev.orientation,
        ) {
            None => return Err(AirRecoveryReason::UnreachableStaging),
            Some(ArtilleryStaging::NeedsRecon(goal)) => {
                dispatch_scout_to(op, obs, goal, public_map, out)?;
                hold_strike_aircraft(op, obs, home, out);
                return Ok(());
            }
            Some(ArtilleryStaging::Ready(staging)) => Some(staging),
        }
    };
    let corridor_clear = if plan.airborne() {
        airborne_corridor_status(op, plan, obs, intel, home, landing_sites)
            == AirborneCorridorStatus::Clear
    } else if remembered_target.is_some() {
        // Current sight cannot cover the approach to a member that has left
        // it, so only known anti-air along the route blocks reacquisition.
        route_without_known_air_defense(intel, home, strike_anchor)
    } else {
        corridor_clear(intel, home, strike_anchor, landing_sites)
    };
    if !corridor_clear {
        return Err(AirRecoveryReason::NewAirDefense);
    }
    let attackers = air_strike_members(op, plan, obs);
    if let Some(target) = live_target {
        if let Some(id) = target.id {
            let mut air_routes = operation_route_projection(
                plan,
                obs,
                Domain::Air,
                public_map,
                context.ev.orientation,
            );
            if !exact_attack_group_reaches(&mut air_routes, obs, &attackers, target.anchor) {
                return Err(AirRecoveryReason::UnreachableAirRoute);
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
        if strike_settled {
            return Err(AirRecoveryReason::Complete);
        }
        let air_routes =
            operation_route_projection(plan, obs, Domain::Air, public_map, context.ev.orientation);
        let cleared_anchor = last_strike_anchor(plan).unwrap_or(strike_anchor);
        if !air_routes.group_reaches_command_goal(&attackers, cleared_anchor) {
            return Err(AirRecoveryReason::UnreachableAirRoute);
        }
        dispatch_air_strike(
            plan,
            obs,
            &attackers,
            AirStrikeDispatch::AttackMove(cleared_anchor),
            out,
        );
        op.strike_issued_at.get_or_insert(obs.tick);
    } else if let Some(remembered) = remembered_target {
        // Every live member has left current sight. Flying toward the best
        // remembered one reacquires it instead of idling out the phase.
        let air_routes =
            operation_route_projection(plan, obs, Domain::Air, public_map, context.ev.orientation);
        if !air_routes.group_reaches_command_goal(&attackers, remembered.anchor) {
            return Err(AirRecoveryReason::UnreachableAirRoute);
        }
        dispatch_air_strike(
            plan,
            obs,
            &attackers,
            AirStrikeDispatch::AttackMove(remembered.anchor),
            out,
        );
        op.strike_issued_at.get_or_insert(obs.tick);
    }
    if let Some(staging) = staging {
        stage_artillery(op, staging, out);
    }
    Ok(())
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
    if elapsed(op.started_at, obs.tick) >= operation_timeout(profile, op, plan)
        || (!connected_preparation
            && !waiting_for_recon_scout
            && elapsed(op.phase_started_at, obs.tick)
                >= phase_timeout(op.phase(), op.started_at, plan))
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
            let minimum = connected.commitment.minimum_capability;
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
            live_suppression < minimum.suppression || live_strike < minimum.strike
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
    let procurement = context.procurement;
    if !procurement.allow {
        return out;
    }
    let obs = context.ev.obs;
    let mut bank = obs.scrap.saturating_sub(procurement.reserve.current);
    let mut production = super::production::ImmediateProduction::new(
        obs,
        procurement.lanes.reservations,
        procurement.lanes.prior_intents,
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
    lanes: ProducerLanes<'_>,
    selected: FreshAirTarget<'_>,
    standby: AirStandby,
) -> ActiveAirOperation {
    let (target, plan) = match selected {
        FreshAirTarget::Island(target) => {
            let island = IslandPlan::new(profile, obs, lanes);
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
    let preferred = preferred_anchor(op, plan);
    operation_target_cluster(op, plan, intel)
        .into_iter()
        .filter(|building| building.evidence == ContactEvidence::Current && building.id.is_some())
        .min_by_key(|building| operation_target_key(preferred, building))
}

/// The member an operation stages, scouts, and strikes first: a connected
/// operation's focus, otherwise its objective anchor.
fn preferred_anchor(op: &AirOperation, plan: &AirPlan) -> TilePos {
    plan.connected()
        .map_or(op.target, |connected| connected.focus)
}

/// Best currently observed live member, preferring `focus`.
fn best_current_member<'a>(
    commitment: &ConnectedCommitment,
    focus: TilePos,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    commitment
        .live_members(intel)
        .into_iter()
        .filter(|building| building.evidence == ContactEvidence::Current && building.id.is_some())
        .min_by_key(|building| operation_target_key(focus, building))
}

fn operation_target_key(
    preferred: TilePos,
    building: &BuildingContact,
) -> (bool, Reverse<u32>, i32, i32, Option<BuildingId>) {
    (
        building.anchor != preferred,
        Reverse(u32::from(building_value(building.kind))),
        building.anchor.y,
        building.anchor.x,
        building.id,
    )
}

/// Connected operations keep the representative identity they were admitted
/// under; only island and remembered objectives follow current sight.
fn refresh_target(op: &mut AirOperation, plan: &AirPlan, intel: &StrategicIntelligence) {
    if matches!(plan, AirPlan::Connected(_)) {
        return;
    }
    if let Some(target) = intel.buildings().iter().find(|b| {
        b.player == op.target_player
            && b.anchor == op.target
            && b.evidence == ContactEvidence::Current
    }) {
        op.target_kind = target.kind;
        op.target_id = target.id;
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
    let cluster = operation_target_cluster(op, plan, intel);
    let mut targets = target_cluster_air_defense(intel, &cluster)
        .sources
        .into_iter()
        .filter(|source| source.evidence == ContactEvidence::Current)
        .filter_map(|source| current_aa_contact(intel, source.source)?.suppression_target());
    targets.find_map(|target| {
        artillery_firing_assignments(obs, intel, &op.artillery, target, public_map, orientation)
            .map(|firing_stands| SuppressionEngagement {
                target,
                firing_stands,
            })
    })
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
    let mut targetable = None;

    for source in assessment.sources {
        match source.evidence {
            ContactEvidence::Current => {
                if let Some(contact) = current_aa_contact(intel, source.source) {
                    current_coverage = true;
                    targetable = targetable.or_else(|| contact.suppression_target());
                }
            }
            ContactEvidence::Remembered if source.confidence > 0 => {
                remembered_coverage = true;
            }
            ContactEvidence::Remembered => {}
        }
    }

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
        targetable,
        evidence,
    }
}

fn operation_target_cluster<'a>(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &'a StrategicIntelligence,
) -> Vec<&'a BuildingContact> {
    match plan.connected() {
        Some(connected) => connected.commitment.live_members(intel),
        None => current_target_cluster(intel, op.target_player, op.target),
    }
}

fn operation_objective_anchor(
    op: &AirOperation,
    plan: &AirPlan,
    intel: &StrategicIntelligence,
) -> TilePos {
    live_strike_target(op, plan, intel)
        .or_else(|| best_remembered_member(plan, intel))
        .map_or_else(
            || last_strike_anchor(plan).unwrap_or_else(|| preferred_anchor(op, plan)),
            |contact| contact.anchor,
        )
}

/// Best live connected member known only from memory, preferring the focus.
/// Callers use it only when no member is in current sight.
fn best_remembered_member<'a>(
    plan: &AirPlan,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    let connected = plan.connected()?;
    connected
        .commitment
        .live_members(intel)
        .into_iter()
        .filter(|contact| contact.evidence == ContactEvidence::Remembered)
        .min_by_key(|contact| operation_target_key(connected.focus, contact))
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
    let Some(connected) = plan.connected() else {
        return intel.buildings().iter().any(|building| {
            building.player == op.target_player
                && building.anchor == op.target
                && building.evidence == ContactEvidence::Remembered
                && building
                    .last_seen
                    .is_none_or(|seen| elapsed(seen, now) > ACTIVE_OPERATION_TARGET_MEMORY)
        });
    };
    let contacts = connected.commitment.live_members(intel);
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

/// A connected operation is cleared once current sight has found every
/// admitted member gone, whether or not those anchors remain in sight.
fn operation_objective_cleared(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> bool {
    let Some(connected) = plan.connected() else {
        let target_is_current = intel.buildings().iter().any(|building| {
            building.player == op.target_player
                && building.anchor == op.target
                && building.evidence == ContactEvidence::Current
        });
        return target_visible(op, obs) && !target_is_current;
    };
    connected.commitment.live_members(intel).is_empty()
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

fn route_without_known_air_defense(
    intel: &StrategicIntelligence,
    home: TilePos,
    target: TilePos,
) -> bool {
    flight_corridor(home, target)
        .into_iter()
        .chain(approach(home, target))
        .all(|tile| {
            matches!(
                intel.air_defense_at(tile).evidence(),
                AirDefenseEvidence::VisibleWithoutKnownCoverage | AirDefenseEvidence::Unknown
            )
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

fn connected_production_access<'a>(
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
fn connected_target_selection<'a>(
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
        if source.evidence != ContactEvidence::Current {
            continue;
        }
        let Some(contact) = current_aa_contact(intel, source.source) else {
            continue;
        };
        match contact.suppression_target() {
            Some(target) => needs.targets.push(target),
            None => needs.has_untargetable_current = true,
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

/// Live contacts at `anchors` whose evidence can size a package: current sight,
/// or a remembered contact with positive confidence. Anchors selected from a
/// fresh current cluster hold only current contacts.
fn sized_target_contacts_at_anchors<'a>(
    intel: &'a StrategicIntelligence,
    player: PlayerId,
    anchors: &[TilePos],
    now: Tick,
) -> Vec<&'a BuildingContact> {
    intel
        .buildings()
        .iter()
        .filter(|contact| {
            contact.player == player
                && anchors.contains(&contact.anchor)
                && (contact.evidence == ContactEvidence::Current || contact.confidence_at(now) > 0)
                && contact.built
                && contact.hp > 0
                && building_value(contact.kind) > 0
        })
        .collect()
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
fn emergency_paid_queue_access(obs: &Observation, excluded: &[PaidQueueClaim]) -> ProductionAccess {
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
    let obs = context.ev.obs;
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
    let Some(connected) = plan.connected() else {
        return false;
    };
    let package = &connected.package;
    if !context.procurement.allow || context.ev.obs.tick >= package.preparation_deadline {
        return false;
    }
    let resources = context
        .connected_resources
        .as_ref()
        .expect("connected preparation has one observation-bound resource view");
    let outstanding = missing_package_demands(
        package,
        AirRoster::from(op),
        context.ev.obs,
        &resources.snapshot,
        package.preparation_deadline,
        &resources.access,
    );
    matches!(
        refine_provider_demands(
            ProductionEvidence::with_planning(
                &resources.snapshot,
                &resources.access,
                Some(context.procurement.planning)
            ),
            &outstanding,
            context.ev.obs.tick,
            PreparationConstraints {
                deadline: package.preparation_deadline,
                decision_cadence: context.ev.tuning.cadence,
                protected_forecast_scrap: context.procurement.reserve.forecast,
            },
            connected.commitment.key(),
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

fn scout_and_hold(
    op: &mut AirOperation,
    plan: &AirPlan,
    context: &AirPlanningContext<'_>,
    landing_sites: &[TilePos],
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    dispatch_scout(
        op,
        plan,
        context.ev.obs,
        context.ev.intel,
        landing_sites,
        connected_public_map(plan, context.ev.public_map),
        out,
    )?;
    hold_air_strike(op, plan, context.ev.obs, context.ev.home, out);
    Ok(())
}

/// Moves the scout toward its next objective, failing when no air route
/// reaches it.
fn dispatch_scout(
    op: &mut AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    let goal = reachable_scout_goal(op, plan, obs, intel, landing_sites, public_map)
        .ok_or(AirRecoveryReason::UnreachableAirRoute)?;
    issue_scout_dispatch(op, obs, goal, out);
    Ok(())
}

/// The scout's next objective, when a known air route still reaches it.
fn reachable_scout_goal(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    scout_dispatch_goal(op, plan, obs, intel, landing_sites, public_map)
        .filter(|goal| scout_dispatch_is_viable(op, obs, *goal, public_map))
}

fn scout_dispatch_goal(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    public_map: Option<&PublicMapBriefing>,
) -> Option<TilePos> {
    let target = connected_scout_focus(op, plan, obs, intel);
    scout_goal(op, obs, intel, target, landing_sites, public_map)
}

fn connected_scout_focus(
    op: &AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
) -> TilePos {
    let Some(connected) = plan.connected() else {
        return op.target;
    };
    let mut contacts = connected.commitment.live_members(intel);
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

fn dispatch_scout_to(
    op: &mut AirOperation,
    obs: &Observation,
    goal: TilePos,
    public_map: Option<&PublicMapBriefing>,
    out: &mut StrategicDecision,
) -> Result<(), AirRecoveryReason> {
    if !scout_dispatch_is_viable(op, obs, goal, public_map) {
        return Err(AirRecoveryReason::UnreachableAirRoute);
    }
    issue_scout_dispatch(op, obs, goal, out);
    Ok(())
}

fn issue_scout_dispatch(
    op: &mut AirOperation,
    obs: &Observation,
    goal: TilePos,
    out: &mut StrategicDecision,
) {
    let Some(scout) = op.scout else {
        return;
    };
    let member = unit(obs, scout).expect("a viable scout dispatch retains its live unit");
    if op.scout_dispatch == Some((scout, goal)) {
        return;
    }
    op.scout_dispatch = Some((scout, goal));
    if !member.idle || member.tile.chebyshev(goal) > 1 {
        out.intents.push(Intent::MoveUnits {
            units: vec![scout],
            goal,
        });
    }
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
            routing::in_bounds(obs, *tile) && {
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
    obs.enemy_buildings.iter().any(|building| {
        building.seen
            && building.player == op.target_player
            && plan
                .connected()
                .map_or(building.anchor == op.target, |connected| {
                    connected.commitment.contains(building.anchor)
                })
    })
}

fn unadmitted_recon(op: &AirOperation) -> bool {
    op.phase() == AirOperationPhase::Recon && !op.assault_admitted()
}

/// The objective's contact while current sight has yet to reacquire it.
fn remembered_objective<'a>(
    op: &AirOperation,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    intel.buildings().iter().find(|building| {
        building.player == op.target_player
            && building.anchor == op.target
            && building.evidence == ContactEvidence::Remembered
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

fn target_visible(op: &AirOperation, obs: &Observation) -> bool {
    let (width, height) = op.target_kind.base_stats().size;
    (0..height).any(|dy| (0..width).any(|dx| obs.visible(op.target.offset(dx, dy))))
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
        && known_ground_connection(
            obs,
            home,
            target.anchor,
            target.kind.base_stats().size,
            public_map,
        ) == Some(false)
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

fn operation_timeout(profile: &ResolvedProfile, op: &AirOperation, plan: &AirPlan) -> Tick {
    3_200u64
        .saturating_add(plan.assembly_timeout(op.started_at))
        .saturating_add(u64::from(100u8.saturating_sub(profile.traits.air)) * 4)
}

fn phase_timeout(phase: AirOperationPhase, started_at: Tick, plan: &AirPlan) -> Tick {
    match phase {
        AirOperationPhase::Recon => 900,
        AirOperationPhase::Assemble => plan.assembly_timeout(started_at),
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

fn connected_suppression_roster_has_firing_assignments(
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
