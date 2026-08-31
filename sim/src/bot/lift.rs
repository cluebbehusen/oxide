//! Persistent, fog-honest transport-wave planning.

use super::difficulty::strategic_admission_tick;
use super::executive::{Intent, unit_strength};
use super::intelligence::{BuildingContact, ContactEvidence};
use super::observation::{BuildingObs, Observation, UnitObs};
use super::routing::{self, RouteProjection};
use super::strategy::StrategicDecision;
use super::utility::combat_core_status;
use crate::ids::{BuildingId, PlayerId, UnitId};
use crate::stats::{BuildingKind, Domain, UnitKind};
use chassis::Tick;
use chassis::grid::TilePos;
use core::cmp::Reverse;

const SHALLOW_QUEUE_DEPTH: usize = 2;
const HOME_FLOOR_DIVISOR: u32 = 5;
const MIN_EARLY_PAYLOAD: u32 = 3;
const DROP_ATTEMPTS: u8 = 3;
const BOARDING_GRACE_TRAINS: u64 = 4;
const SUPPORT_GRACE_TRAINS: u64 = 2;
const SUPPORT_TIMEOUT_MIN_CARRIERS: usize = 2;
const SUPPORT_ABORT_MIN_CARRIERS: usize = 3;

/// Coordination signal from an independent air operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LiftAirSupport {
    /// The transport operation may proceed on its own judgment.
    #[default]
    Independent,
    /// A matching air operation is still opening the corridor.
    Suppressing {
        /// Enemy seat whose enclave is being opened.
        player: PlayerId,
        /// Stable objective anchor shared by the two operations.
        target: TilePos,
    },
    /// A matching air operation has released its strike.
    Released {
        /// Enemy seat whose enclave is being struck.
        player: PlayerId,
        /// Stable objective anchor shared by the two operations.
        target: TilePos,
    },
    /// A matching air operation ended without opening the corridor.
    Aborted {
        /// Enemy seat whose enclave could not be opened.
        player: PlayerId,
        /// Stable objective anchor shared by the two operations.
        target: TilePos,
    },
}

/// Persistent phase of a coordinated lift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LiftPhase {
    /// Accumulate a carrier requirement that may grow with the available army.
    Provision,
    /// Move carriers to distinct pickups and board exact manifests.
    Boarding,
    /// Hold the loaded wave while matching suppression is active.
    AwaitSupport,
    /// Fly every ready carrier to its distinct drop slot.
    Landing,
    /// Return empty carriers while landed riders prosecute the objective.
    Recover,
}

/// One carrier's immutable assignment within an active operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiftManifest {
    /// Exact carrier.
    pub carrier: UnitId,
    /// Exact riders assigned to this carrier, in id order.
    pub riders: Vec<UnitId>,
    /// Distinct home-side boarding station.
    pub pickup: TilePos,
    /// Distinct target-side landing station.
    pub drop: TilePos,
    /// Whether the landed survivors received their initial assault order.
    pub attack_issued: bool,
    /// Whether this exact boarding command has already been dispatched.
    pub load_dispatched: bool,
    /// Whether boarding has resolved or failed for this immutable manifest.
    pub boarding_closed: bool,
    /// Number of target-side unload sites attempted.
    pub unload_attempts: u8,
    /// Number of bounded home-side recovery actions attempted.
    pub recovery_attempts: u8,
    /// Whether bounded target-side landing attempts forced a return home.
    pub aborted: bool,
    /// Whether bounded recovery attempts exhausted every useful action.
    pub closed: bool,
}

/// Inspectable persistent state of one transport wave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiftOperation {
    /// Last known owner of the objective.
    pub target_player: PlayerId,
    /// Last known objective id. A remembered ghost may still carry its id.
    pub target_id: BuildingId,
    /// Stable objective footprint anchor.
    pub target: TilePos,
    /// Current operation phase.
    pub phase: LiftPhase,
    /// Tick on which the operation began.
    pub started_at: Tick,
    /// Tick on which the current phase began.
    pub phase_started_at: Tick,
    /// Final tick allowed for provisioning and boarding.
    pub deadline: Tick,
    /// Canonical open tile identifying the frozen home-side pickup component.
    pub pickup_component: TilePos,
    /// Carrier requirement, frozen once exact manifests are assigned.
    pub desired_carriers: usize,
    /// Exact canonical home-side riders owned before carriers finish training.
    pub payload: UnitIdSet,
    /// Sling-room target retained while deterministic replacements exist.
    pub payload_target: u32,
    /// Ground-capable sling room, frozen once exact manifests are assigned.
    pub ground_payload_target: u32,
    /// Canonical target-side landing sites, frozen with exact manifests.
    pub planned_drops: Vec<TilePos>,
    /// Exact disjoint carrier manifests once the full wave is available.
    pub manifests: Vec<LiftManifest>,
    /// Whether the boarding barrier released this wave toward the objective.
    pub launched: bool,
}

/// Sorted, deduplicated unit ids. Readers binary-search the slice, so
/// ordering is a type invariant here rather than a convention every
/// mutation site re-establishes by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitIdSet(Vec<UnitId>);

impl UnitIdSet {
    /// Canonicalizes arbitrary ids into a set.
    pub fn from_ids(mut ids: Vec<UnitId>) -> Self {
        ids.sort_unstable();
        ids.dedup();
        Self(ids)
    }

    /// Inserts in id order; `false` when the id is already present.
    pub fn insert(&mut self, id: UnitId) -> bool {
        match self.0.binary_search(&id) {
            Ok(_) => false,
            Err(index) => {
                self.0.insert(index, id);
                true
            }
        }
    }

    /// Removes and returns the highest id.
    pub fn pop_last(&mut self) -> Option<UnitId> {
        self.0.pop()
    }
}

impl std::ops::Deref for UnitIdSet {
    type Target = [UnitId];

    fn deref(&self) -> &[UnitId] {
        &self.0
    }
}

impl FromIterator<UnitId> for UnitIdSet {
    fn from_iter<I: IntoIterator<Item = UnitId>>(ids: I) -> Self {
        Self::from_ids(ids.into_iter().collect())
    }
}

impl PartialEq<[UnitId]> for UnitIdSet {
    fn eq(&self, other: &[UnitId]) -> bool {
        self.0 == other
    }
}

impl PartialEq<Vec<UnitId>> for UnitIdSet {
    fn eq(&self, other: &Vec<UnitId>) -> bool {
        &self.0 == other
    }
}

impl<const N: usize> PartialEq<[UnitId; N]> for UnitIdSet {
    fn eq(&self, other: &[UnitId; N]) -> bool {
        self.0 == other
    }
}

/// Controller-local owner of a persistent transport wave.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiftPlanner {
    operation: Option<LiftOperation>,
    support_latched: bool,
    support_released: bool,
    assault_waypoints: Vec<TilePos>,
    retry_not_before: Tick,
}

#[derive(Clone, Copy)]
pub(super) struct LiftAdmission<'a> {
    pub(super) allow_new_commitments: bool,
    pub(super) core_reservations: &'a [UnitId],
    pub(super) minimum_core_equivalents: u64,
}

impl LiftPlanner {
    /// Creates an idle lift planner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Active operation for replay diagnostics.
    pub fn operation(&self) -> Option<&LiftOperation> {
        self.operation.as_ref()
    }

    /// Capital held for the first carrier while a remembered objective is
    /// being reacquired. This is a planning hint only: it neither starts an
    /// operation nor claims the prospective payload before current sight.
    pub(super) fn prospective_first_carrier_commitment(
        &self,
        obs: &Observation,
        home: TilePos,
        unavailable: &[UnitId],
        core_reservations: &[UnitId],
        minimum_core_equivalents: u64,
        target: &BuildingContact,
    ) -> u32 {
        if self.operation.is_some()
            || target.evidence != ContactEvidence::Remembered
            || !target.built
            || target.confidence_at(obs.tick) == 0
            || !obs
                .my_buildings
                .iter()
                .any(|building| building.built && building.kind == BuildingKind::Airworks)
            || obs
                .my_queues
                .iter()
                .flatten()
                .any(|kind| *kind == UnitKind::Skyhook)
            || !available_carriers(obs, unavailable).is_empty()
        {
            return 0;
        }
        let Some(plan) = initial_payload_plan_preserving_core(
            obs,
            home,
            unavailable,
            core_reservations,
            minimum_core_equivalents,
        ) else {
            return 0;
        };
        if !ground_disconnection_is_proven(obs, plan.pickup, target) {
            return 0;
        }
        UnitKind::Skyhook.stats().cost
    }

    /// Skyhook factory-time demand remaining beyond the live carrier fleet.
    pub(super) fn remaining_airwork_ticks(&self, obs: &Observation, unavailable: &[UnitId]) -> u64 {
        let Some(operation) = self
            .operation
            .as_ref()
            .filter(|operation| operation.phase == LiftPhase::Provision)
        else {
            return 0;
        };
        let live = available_carriers(obs, unavailable).len();
        let missing = operation.desired_carriers.saturating_sub(live);
        u64::try_from(missing)
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(UnitKind::Skyhook.stats().train_ticks))
    }

    /// Advances the lift using only oriented observation knowledge.
    ///
    /// `unavailable` contains exact units owned by other strategic channels or
    /// by non-transferable executive state. A staged army may deliberately be
    /// omitted because Load lowering removes drafted riders from that army.
    pub fn think(
        &mut self,
        obs: &Observation,
        home: TilePos,
        unavailable: &[UnitId],
        support: LiftAirSupport,
    ) -> StrategicDecision {
        self.think_with_admission(
            obs,
            home,
            unavailable,
            support,
            LiftAdmission {
                allow_new_commitments: true,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        )
    }

    pub(super) fn think_with_admission(
        &mut self,
        obs: &Observation,
        home: TilePos,
        unavailable: &[UnitId],
        support: LiftAirSupport,
        admission: LiftAdmission<'_>,
    ) -> StrategicDecision {
        let LiftAdmission {
            allow_new_commitments,
            core_reservations,
            minimum_core_equivalents,
        } = admission;
        let mut unavailable = unavailable.to_vec();
        unavailable.sort_unstable();
        unavailable.dedup();

        if allow_new_commitments
            && self.operation.is_none()
            && obs.tick >= self.retry_not_before
            && strategic_admission_tick(obs.tick)
            && let Some(plan) = initial_payload_plan_preserving_core(
                obs,
                home,
                &unavailable,
                core_reservations,
                minimum_core_equivalents,
            )
            && let Some(target) = select_target(obs, home, plan.pickup, support)
        {
            let ground_payload_target = plan
                .payload
                .iter()
                .filter_map(|id| unit(obs, *id))
                .filter(|member| can_defend_ground(member))
                .map(|member| u32::from(member.kind.stats().transport_size))
                .sum();
            let built_airworks = obs
                .my_buildings
                .iter()
                .any(|building| building.built && building.kind == BuildingKind::Airworks);
            let enough_existing_carriers =
                available_carriers(obs, &unavailable).len() >= plan.desired_carriers;
            let planned_drops =
                planned_drop_slots(obs, plan.pickup, target.anchor, plan.desired_carriers);
            if planned_drops.len() == plan.desired_carriers
                && (built_airworks || enough_existing_carriers)
            {
                self.support_latched = false;
                self.support_released = false;
                self.assault_waypoints.clear();
                self.operation = Some(LiftOperation {
                    target_player: target.player,
                    target_id: target.id,
                    target: target.anchor,
                    phase: LiftPhase::Provision,
                    started_at: obs.tick,
                    phase_started_at: obs.tick,
                    deadline: provision_deadline(obs, &unavailable, plan.desired_carriers),
                    pickup_component: plan.pickup,
                    desired_carriers: plan.desired_carriers,
                    payload: UnitIdSet::from_ids(plan.payload),
                    payload_target: plan.payload_target,
                    ground_payload_target,
                    planned_drops,
                    manifests: Vec::new(),
                    launched: false,
                });
            }
        }

        let Some(mut operation) = self.operation.take() else {
            return StrategicDecision::default();
        };
        let mut decision = StrategicDecision::default();
        let mut handoff = Vec::new();

        if operation.phase == LiftPhase::Provision
            && !refresh_provision_payload(
                &mut operation,
                obs,
                &unavailable,
                allow_new_commitments,
                core_reservations,
                minimum_core_equivalents,
            )
        {
            enter(&mut operation, LiftPhase::Recover, obs.tick);
        }
        if operation.phase == LiftPhase::Boarding
            && boarding_payload(&operation, obs) < MIN_EARLY_PAYLOAD
        {
            enter(&mut operation, LiftPhase::Recover, obs.tick);
        }
        if matches!(operation.phase, LiftPhase::Provision | LiftPhase::Boarding)
            && obs.tick >= operation.deadline
        {
            enter(&mut operation, LiftPhase::Recover, obs.tick);
        }
        if operation.phase < LiftPhase::Landing && !target_remains_disconnected(obs, &operation) {
            enter(&mut operation, LiftPhase::Recover, obs.tick);
        }
        if operation.phase <= LiftPhase::AwaitSupport {
            let (carrier_lost, released_riders) = close_missing_manifests(&mut operation, obs);
            if carrier_lost {
                if boarding_complete(&operation, obs) {
                    enter(&mut operation, LiftPhase::Landing, obs.tick);
                    operation.launched = true;
                    if !released_riders.is_empty() {
                        decision.intents.push(Intent::StopUnits {
                            units: released_riders,
                        });
                    }
                } else {
                    enter(&mut operation, LiftPhase::Recover, obs.tick);
                }
            }
        }

        let support_directive = operation_support_directive(support, &operation);
        if matches!(operation.phase, LiftPhase::Provision | LiftPhase::Boarding) {
            match support_directive {
                SupportDirective::Hold => {
                    self.support_latched = true;
                    self.support_released = false;
                }
                SupportDirective::Release => {
                    self.support_latched = true;
                    self.support_released = true;
                }
                SupportDirective::Abort => {
                    if operation.phase == LiftPhase::Provision {
                        self.support_latched = false;
                        self.support_released = false;
                        if !target_is_current(obs, &operation) {
                            enter(&mut operation, LiftPhase::Recover, obs.tick);
                        }
                    }
                }
                SupportDirective::Independent | SupportDirective::Unmatched => {}
            }
        }

        match operation.phase {
            LiftPhase::Provision => {
                if allow_new_commitments {
                    provision(&operation, obs, &unavailable, &mut decision);
                }
                if assign_manifests(&mut operation, obs, home, &unavailable) {
                    enter(&mut operation, LiftPhase::Boarding, obs.tick);
                    operation.deadline = operation
                        .deadline
                        .max(obs.tick.saturating_add(boarding_grace()));
                    board(&mut operation, obs, &mut decision);
                }
            }
            LiftPhase::Boarding => {
                board(&mut operation, obs, &mut decision);
                if boarding_complete(&operation, obs) {
                    match support_directive {
                        SupportDirective::Hold => {
                            enter(&mut operation, LiftPhase::AwaitSupport, obs.tick);
                        }
                        SupportDirective::Abort => launch_or_recover(
                            &mut operation,
                            obs,
                            &mut decision,
                            &mut handoff,
                            SUPPORT_ABORT_MIN_CARRIERS,
                        ),
                        SupportDirective::Release => {
                            enter(&mut operation, LiftPhase::Landing, obs.tick);
                            operation.launched = true;
                            land(&mut operation, obs, &mut decision, &mut handoff);
                        }
                        SupportDirective::Independent | SupportDirective::Unmatched => {
                            if self.support_released {
                                enter(&mut operation, LiftPhase::Landing, obs.tick);
                                operation.launched = true;
                                land(&mut operation, obs, &mut decision, &mut handoff);
                            } else if self.support_latched {
                                enter(&mut operation, LiftPhase::AwaitSupport, obs.tick);
                            } else {
                                enter(&mut operation, LiftPhase::Landing, obs.tick);
                                operation.launched = true;
                                land(&mut operation, obs, &mut decision, &mut handoff);
                            }
                        }
                    }
                } else if boarding_resolved(&operation, obs) {
                    enter(&mut operation, LiftPhase::Recover, obs.tick);
                }
            }
            LiftPhase::AwaitSupport => match support_directive {
                SupportDirective::Hold => {
                    if obs.tick.saturating_sub(operation.phase_started_at) >= support_grace() {
                        launch_or_recover(
                            &mut operation,
                            obs,
                            &mut decision,
                            &mut handoff,
                            SUPPORT_TIMEOUT_MIN_CARRIERS,
                        );
                    }
                }
                SupportDirective::Release => {
                    enter(&mut operation, LiftPhase::Landing, obs.tick);
                    operation.launched = true;
                    land(&mut operation, obs, &mut decision, &mut handoff);
                }
                SupportDirective::Abort => {
                    launch_or_recover(
                        &mut operation,
                        obs,
                        &mut decision,
                        &mut handoff,
                        SUPPORT_ABORT_MIN_CARRIERS,
                    );
                }
                SupportDirective::Independent | SupportDirective::Unmatched => {
                    if obs.tick.saturating_sub(operation.phase_started_at) >= support_grace() {
                        launch_or_recover(
                            &mut operation,
                            obs,
                            &mut decision,
                            &mut handoff,
                            SUPPORT_TIMEOUT_MIN_CARRIERS,
                        );
                    }
                }
            },
            LiftPhase::Landing => {
                land(&mut operation, obs, &mut decision, &mut handoff);
                if operation
                    .manifests
                    .iter()
                    .all(|manifest| manifest.closed || carrier_cargo(obs, manifest.carrier) == 0)
                {
                    enter(&mut operation, LiftPhase::Recover, obs.tick);
                }
            }
            LiftPhase::Recover => {
                recover(&mut operation, obs, &mut decision, &mut handoff);
            }
        }

        let assault_complete = if operation.phase == LiftPhase::Recover && operation.launched {
            sustain_landed_assault(
                &operation,
                obs,
                &mut self.assault_waypoints,
                &mut decision,
                &mut handoff,
            )
        } else {
            true
        };

        decision.reservations = reservations(&operation, obs, &unavailable);
        decision.reservations.extend(handoff);
        decision.reservations.sort_unstable();
        decision.reservations.dedup();

        if operation.phase == LiftPhase::Recover
            && recovery_complete(&operation, obs)
            && assault_complete
        {
            if !operation.launched {
                self.retry_not_before = obs.tick.saturating_add(support_grace());
            }
            self.operation = None;
            self.support_latched = false;
            self.support_released = false;
            self.assault_waypoints.clear();
        } else {
            self.operation = Some(operation);
        }
        decision
    }
}

fn select_target(
    obs: &Observation,
    home: TilePos,
    pickup: TilePos,
    support: LiftAirSupport,
) -> Option<&BuildingObs> {
    let coordinated = support_target(support).and_then(|(player, target)| {
        obs.enemy_buildings
            .iter()
            .filter(|building| {
                building.built
                    && building.seen
                    && building.player == player
                    && building.anchor == target
                    && disconnected(obs, pickup, building)
            })
            .min_by_key(|building| building.id)
    });
    if coordinated.is_some() {
        return coordinated;
    }

    obs.enemy_buildings
        .iter()
        .filter(|building| building.built && building.seen && disconnected(obs, pickup, building))
        .min_by_key(|building| {
            (
                building.kind != BuildingKind::Foundry,
                building.anchor.manhattan(home),
                building.anchor.y,
                building.anchor.x,
                building.player,
                building.id,
            )
        })
}

fn support_target(support: LiftAirSupport) -> Option<(PlayerId, TilePos)> {
    match support {
        LiftAirSupport::Suppressing { player, target }
        | LiftAirSupport::Released { player, target } => Some((player, target)),
        LiftAirSupport::Independent | LiftAirSupport::Aborted { .. } => None,
    }
}

fn target_remains_disconnected(obs: &Observation, operation: &LiftOperation) -> bool {
    obs.enemy_buildings.iter().any(|building| {
        building.player == operation.target_player
            && building.anchor == operation.target
            && disconnected(obs, operation.pickup_component, building)
    })
}

fn target_is_current(obs: &Observation, operation: &LiftOperation) -> bool {
    obs.enemy_buildings.iter().any(|building| {
        building.id == operation.target_id
            && building.player == operation.target_player
            && building.anchor == operation.target
            && building.seen
    })
}

fn disconnected(obs: &Observation, pickup: TilePos, target: &BuildingObs) -> bool {
    if !routing::ground_open(obs, pickup) {
        return false;
    }
    let mut routes = RouteProjection::known_ground(obs);
    !footprint_ring(target.anchor, target.kind.base_stats().size)
        .into_iter()
        .any(|tile| routes.reaches(pickup, tile))
}

fn ground_disconnection_is_proven(
    obs: &Observation,
    pickup: TilePos,
    target: &BuildingContact,
) -> bool {
    if !routing::ground_open(obs, pickup) {
        return false;
    }
    let mut routes = RouteProjection::new(obs, Domain::Ground);
    let goals: Vec<_> = footprint_ring(target.anchor, target.kind.base_stats().size)
        .into_iter()
        .filter(|tile| routing::ground_open(obs, *tile))
        .collect();
    !goals.is_empty() && !goals.into_iter().any(|goal| routes.reaches(pickup, goal))
}

#[cfg(test)]
fn desired_carriers(obs: &Observation, home: TilePos, unavailable: &[UnitId]) -> usize {
    initial_payload(obs, home, unavailable).2
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PickupPlan {
    pickup: TilePos,
    payload: Vec<UnitId>,
    payload_target: u32,
    desired_carriers: usize,
    strength: u64,
}

#[cfg(test)]
fn initial_payload(
    obs: &Observation,
    home: TilePos,
    unavailable: &[UnitId],
) -> (Vec<UnitId>, u32, usize) {
    initial_payload_plan(obs, home, unavailable).map_or((Vec::new(), 0, 0), |plan| {
        (plan.payload, plan.payload_target, plan.desired_carriers)
    })
}

#[cfg(test)]
fn initial_payload_plan(
    obs: &Observation,
    home: TilePos,
    unavailable: &[UnitId],
) -> Option<PickupPlan> {
    initial_payload_plan_preserving_core(obs, home, unavailable, &[], 0)
}

fn initial_payload_plan_preserving_core(
    obs: &Observation,
    home: TilePos,
    unavailable: &[UnitId],
    core_reservations: &[UnitId],
    minimum_core_equivalents: u64,
) -> Option<PickupPlan> {
    pickup_component_anchors(obs, home)
        .into_iter()
        .filter_map(|pickup| {
            payload_plan_for_component(
                obs,
                pickup,
                unavailable,
                core_reservations,
                minimum_core_equivalents,
            )
        })
        .min_by_key(|plan| {
            (
                Reverse(plan.strength),
                Reverse(plan.payload_target),
                plan.pickup.chebyshev(home),
                plan.pickup.y,
                plan.pickup.x,
            )
        })
}

fn payload_plan_for_component(
    obs: &Observation,
    pickup: TilePos,
    unavailable: &[UnitId],
    core_reservations: &[UnitId],
    minimum_core_equivalents: u64,
) -> Option<PickupPlan> {
    if !routing::ground_open(obs, pickup) {
        return None;
    }
    let candidates = lift_candidates(obs, pickup, unavailable);
    let selected = select_payload(
        obs,
        &candidates,
        usize::MAX,
        core_reservations,
        minimum_core_equivalents,
    );
    let desired = pack(&selected, usize::MAX).len();
    let target = selected
        .iter()
        .map(|unit| u32::from(unit.kind.stats().transport_size))
        .sum();
    if desired == 0 || target < MIN_EARLY_PAYLOAD {
        return None;
    }
    let strength = selected.iter().fold(0u64, |total, unit| {
        total.saturating_add(unit_strength(unit))
    });
    let mut payload: Vec<_> = selected.into_iter().map(|unit| unit.id).collect();
    payload.sort_unstable();
    payload.dedup();
    Some(PickupPlan {
        pickup,
        payload,
        payload_target: target,
        desired_carriers: desired,
        strength,
    })
}

fn provision_deadline(obs: &Observation, unavailable: &[UnitId], desired_carriers: usize) -> Tick {
    let live = available_carriers(obs, unavailable).len();
    let missing = desired_carriers.saturating_sub(live);
    let work = u64::try_from(missing)
        .unwrap_or(u64::MAX)
        .saturating_add(BOARDING_GRACE_TRAINS)
        .saturating_mul(u64::from(UnitKind::Skyhook.stats().train_ticks));
    obs.tick.saturating_add(work)
}

fn lift_candidates<'a>(
    obs: &'a Observation,
    pickup: TilePos,
    unavailable: &[UnitId],
) -> Vec<&'a UnitObs> {
    let mut routes = RouteProjection::known_ground(obs);
    let mut candidates: Vec<_> = obs
        .my_units
        .iter()
        .filter(|unit| {
            let stats = unit.kind.stats();
            stats.domain == Domain::Ground
                && stats.can_fight()
                && stats.transport_size > 0
                && stats.transport_size <= UnitKind::Skyhook.stats().transport_capacity
                && unavailable.binary_search(&unit.id).is_err()
                && routes.unit_reaches(unit, pickup)
        })
        .collect();
    candidates.sort_unstable_by_key(|unit| {
        (
            Reverse(unit_strength(unit)),
            Reverse(unit.kind.stats().transport_size),
            unit.tile.manhattan(pickup),
            unit.id,
        )
    });
    candidates
}

fn select_payload<'a>(
    obs: &'a Observation,
    candidates: &[&'a UnitObs],
    carrier_limit: usize,
    core_reservations: &[UnitId],
    minimum_core_equivalents: u64,
) -> Vec<&'a UnitObs> {
    let (mut remaining, mut ground_remaining) = payload_limits(candidates, carrier_limit);
    let mut selected = Vec::new();
    let mut projected_core_reservations = core_reservations.to_vec();
    projected_core_reservations.sort_unstable();
    projected_core_reservations.dedup();
    for &unit in candidates {
        let size = u32::from(unit.kind.stats().transport_size);
        let ground_capable = can_defend_ground(unit);
        if size > remaining || (ground_capable && size > ground_remaining) {
            continue;
        }
        let insertion = match projected_core_reservations.binary_search(&unit.id) {
            Ok(_) => None,
            Err(index) => {
                projected_core_reservations.insert(index, unit.id);
                Some(index)
            }
        };
        if !combat_core_status(
            obs,
            &projected_core_reservations,
            &[],
            minimum_core_equivalents,
        )
        .ready
        {
            if let Some(index) = insertion {
                projected_core_reservations.remove(index);
            }
            continue;
        }
        selected.push(unit);
        remaining -= size;
        if ground_capable {
            ground_remaining -= size;
        }
    }
    selected
}

fn payload_limits(candidates: &[&UnitObs], carrier_limit: usize) -> (u32, u32) {
    let capacity = u32::from(UnitKind::Skyhook.stats().transport_capacity);
    let total: u32 = candidates
        .iter()
        .map(|unit| u32::from(unit.kind.stats().transport_size))
        .sum();
    if total < MIN_EARLY_PAYLOAD {
        return (0, 0);
    }
    let bulk = total >= capacity.saturating_mul(3);
    let floor = home_floor(total, capacity, bulk);
    let ground_total = candidates
        .iter()
        .filter(|unit| can_defend_ground(unit))
        .map(|unit| u32::from(unit.kind.stats().transport_size))
        .sum();
    let ground_floor = home_floor(ground_total, capacity, bulk);
    let capacity_limit = u32::try_from(carrier_limit)
        .unwrap_or(u32::MAX)
        .saturating_mul(capacity);
    (
        total.saturating_sub(floor).min(capacity_limit),
        ground_total.saturating_sub(ground_floor),
    )
}

fn home_floor(total: u32, capacity: u32, bulk: bool) -> u32 {
    if bulk {
        total.min(capacity.max(total / HOME_FLOOR_DIVISOR))
    } else {
        0
    }
}

fn can_defend_ground(unit: &UnitObs) -> bool {
    let stats = unit.kind.stats();
    stats.can_target(Domain::Ground) || stats.demolition
}

fn pack<'a>(riders: &[&'a UnitObs], carrier_limit: usize) -> Vec<Vec<&'a UnitObs>> {
    let capacity = UnitKind::Skyhook.stats().transport_capacity;
    let mut bins: Vec<(u8, Vec<&UnitObs>)> = Vec::new();
    for &rider in riders {
        let size = rider.kind.stats().transport_size;
        if let Some((room, members)) = bins.iter_mut().find(|(room, _)| size <= *room) {
            *room -= size;
            members.push(rider);
        } else if bins.len() < carrier_limit {
            bins.push((capacity - size, vec![rider]));
        }
    }
    bins.into_iter().map(|(_, riders)| riders).collect()
}

fn boarding_grace() -> Tick {
    BOARDING_GRACE_TRAINS.saturating_mul(u64::from(UnitKind::Skyhook.stats().train_ticks))
}

fn support_grace() -> Tick {
    SUPPORT_GRACE_TRAINS.saturating_mul(u64::from(UnitKind::Skyhook.stats().train_ticks))
}

fn refresh_provision_payload(
    operation: &mut LiftOperation,
    obs: &Observation,
    unavailable: &[UnitId],
    allow_growth: bool,
    core_reservations: &[UnitId],
    minimum_core_equivalents: u64,
) -> bool {
    let pickup = operation.pickup_component;
    if !routing::ground_open(obs, pickup) {
        return false;
    }
    let available = lift_candidates(obs, pickup, unavailable);
    let all_candidates = lift_candidates(obs, pickup, &[]);
    let retained_payload_intact = operation
        .payload
        .iter()
        .all(|id| available.iter().any(|member| member.id == *id));
    let (current_payload_limit, current_ground_payload_limit) =
        payload_limits(&all_candidates, operation.desired_carriers);
    let capacity = u32::from(UnitKind::Skyhook.stats().transport_capacity);
    let available_payload: u32 = available
        .iter()
        .map(|unit| u32::from(unit.kind.stats().transport_size))
        .sum();
    if available_payload < MIN_EARLY_PAYLOAD {
        return false;
    }
    let carrier_capacity = u32::try_from(operation.desired_carriers)
        .unwrap_or(u32::MAX)
        .saturating_mul(capacity);
    let target = if retained_payload_intact {
        operation.payload_target.min(carrier_capacity)
    } else {
        operation
            .payload_target
            .min(current_payload_limit)
            .min(carrier_capacity)
    };
    let ground_target = operation
        .ground_payload_target
        .min(current_ground_payload_limit);

    let mut payload = UnitIdSet::default();
    let mut filled = 0u32;
    let mut ground_filled = 0u32;
    let mut projected_core_reservations: Vec<_> = core_reservations
        .iter()
        .copied()
        .filter(|id| operation.payload.binary_search(id).is_err())
        .collect();
    projected_core_reservations.sort_unstable();
    projected_core_reservations.dedup();
    for id in operation.payload.iter().copied() {
        let Some(member) = available.iter().copied().find(|unit| unit.id == id) else {
            continue;
        };
        let size = u32::from(member.kind.stats().transport_size);
        let ground_capable = can_defend_ground(member);
        if filled.saturating_add(size) <= target
            && (!ground_capable || ground_filled.saturating_add(size) <= ground_target)
        {
            payload.insert(id);
            if let Err(index) = projected_core_reservations.binary_search(&id) {
                projected_core_reservations.insert(index, id);
            }
            filled += size;
            if ground_capable {
                ground_filled += size;
            }
        }
    }
    if allow_growth {
        for member in available {
            if payload.binary_search(&member.id).is_ok() {
                continue;
            }
            let size = u32::from(member.kind.stats().transport_size);
            let ground_capable = can_defend_ground(member);
            if filled.saturating_add(size) <= target
                && (!ground_capable || ground_filled.saturating_add(size) <= ground_target)
            {
                let insertion = match projected_core_reservations.binary_search(&member.id) {
                    Ok(_) => None,
                    Err(index) => {
                        projected_core_reservations.insert(index, member.id);
                        Some(index)
                    }
                };
                if !combat_core_status(
                    obs,
                    &projected_core_reservations,
                    &[],
                    minimum_core_equivalents,
                )
                .ready
                {
                    if let Some(index) = insertion {
                        projected_core_reservations.remove(index);
                    }
                    continue;
                }
                payload.insert(member.id);
                filled += size;
                if ground_capable {
                    ground_filled += size;
                }
            }
        }
    }

    while !payload.is_empty() {
        let members = payload_members(obs, &payload, pickup);
        if pack(&members, usize::MAX).len() <= operation.desired_carriers {
            break;
        }
        payload.pop_last();
    }
    filled = payload
        .iter()
        .filter_map(|id| unit(obs, *id))
        .map(|member| u32::from(member.kind.stats().transport_size))
        .sum();
    if filled < MIN_EARLY_PAYLOAD {
        return false;
    }
    operation.payload = payload;
    true
}

fn payload_members<'a>(
    obs: &'a Observation,
    payload: &[UnitId],
    pickup: TilePos,
) -> Vec<&'a UnitObs> {
    let mut members: Vec<_> = obs
        .my_units
        .iter()
        .filter(|member| payload.binary_search(&member.id).is_ok())
        .collect();
    members.sort_unstable_by_key(|member| {
        (
            Reverse(unit_strength(member)),
            Reverse(member.kind.stats().transport_size),
            member.tile.manhattan(pickup),
            member.id,
        )
    });
    members
}

fn boarding_payload(operation: &LiftOperation, obs: &Observation) -> u32 {
    operation
        .manifests
        .iter()
        .map(|manifest| {
            let field: u32 = manifest
                .riders
                .iter()
                .filter_map(|id| unit(obs, *id))
                .map(|member| u32::from(member.kind.stats().transport_size))
                .sum();
            field.saturating_add(u32::from(carrier_cargo(obs, manifest.carrier)))
        })
        .sum()
}

fn provision(
    operation: &LiftOperation,
    obs: &Observation,
    unavailable: &[UnitId],
    decision: &mut StrategicDecision,
) {
    let live = available_carriers(obs, unavailable).len();
    let queued = obs
        .my_queues
        .iter()
        .flatten()
        .filter(|kind| **kind == UnitKind::Skyhook)
        .count();
    let mut missing = operation
        .desired_carriers
        .saturating_sub(live.saturating_add(queued));
    if missing == 0 {
        return;
    }
    if !obs
        .my_buildings
        .iter()
        .any(|building| building.built && building.kind == BuildingKind::Airworks)
    {
        return;
    }

    let mut bank = obs.scrap;
    let mut added = vec![0usize; obs.my_buildings.len()];
    while missing > 0 {
        let producer = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(index, building)| {
                building.built
                    && building.kind == BuildingKind::Airworks
                    && obs.my_queues.get(*index).is_some_and(|queue| {
                        queue.len().saturating_add(added[*index]) < SHALLOW_QUEUE_DEPTH
                    })
            })
            .min_by_key(|(index, building)| {
                (obs.my_queues[*index].len() + added[*index], building.id)
            })
            .map(|(index, building)| (index, building.id));
        let cost = UnitKind::Skyhook.stats().cost;
        let Some((index, building)) = producer else {
            decision.committed_scrap = decision.committed_scrap.saturating_add(bank.min(cost));
            break;
        };
        if bank < cost {
            decision.committed_scrap = decision.committed_scrap.saturating_add(bank);
            break;
        }
        bank -= cost;
        added[index] += 1;
        missing -= 1;
        decision.committed_scrap = decision.committed_scrap.saturating_add(cost);
        decision.intents.push(Intent::TrainAt {
            building,
            kind: UnitKind::Skyhook,
        });
    }
}

fn available_carriers<'a>(obs: &'a Observation, unavailable: &[UnitId]) -> Vec<&'a UnitObs> {
    let mut carriers: Vec<_> = obs
        .my_units
        .iter()
        .filter(|unit| {
            unit.kind == UnitKind::Skyhook
                && unavailable.binary_search(&unit.id).is_err()
                && unit.cargo == 0
        })
        .collect();
    carriers.sort_unstable_by_key(|unit| unit.id);
    carriers
}

fn assign_manifests(
    operation: &mut LiftOperation,
    obs: &Observation,
    home: TilePos,
    unavailable: &[UnitId],
) -> bool {
    if !operation.manifests.is_empty() {
        return true;
    }
    let carriers = available_carriers(obs, unavailable);
    if carriers.len() < operation.desired_carriers {
        return false;
    }
    let pickups = pickup_slots(
        obs,
        home,
        operation.pickup_component,
        operation.desired_carriers,
    );
    let mut air = RouteProjection::new(obs, Domain::Air);
    let drops: Vec<_> = operation
        .planned_drops
        .iter()
        .copied()
        .filter(|drop| {
            routing::ground_open(obs, *drop)
                && air.reaches(pickups.first().copied().unwrap_or(home), *drop)
        })
        .collect();
    if pickups.len() < operation.desired_carriers || drops.len() < operation.desired_carriers {
        return false;
    }
    let selected = payload_members(obs, &operation.payload, pickups[0]);
    let groups = pack(&selected, operation.desired_carriers);
    if groups.len() != operation.desired_carriers {
        return false;
    }
    operation.manifests = groups
        .into_iter()
        .enumerate()
        .map(|(index, riders)| LiftManifest {
            carrier: carriers[index].id,
            riders: {
                let mut ids: Vec<_> = riders.into_iter().map(|unit| unit.id).collect();
                ids.sort_unstable();
                ids
            },
            pickup: pickups[index],
            drop: drops[index],
            attack_issued: false,
            load_dispatched: false,
            boarding_closed: false,
            unload_attempts: 0,
            recovery_attempts: 0,
            aborted: false,
            closed: false,
        })
        .collect();
    true
}

fn board(operation: &mut LiftOperation, obs: &Observation, decision: &mut StrategicDecision) {
    for manifest in &mut operation.manifests {
        let Some(carrier) = unit(obs, manifest.carrier) else {
            continue;
        };
        let pending: Vec<_> = manifest
            .riders
            .iter()
            .copied()
            .filter(|id| unit(obs, *id).is_some())
            .collect();
        if pending.is_empty() {
            manifest.boarding_closed = true;
            continue;
        }
        if manifest.boarding_closed || !carrier.idle {
            continue;
        }
        if !terminal_at(carrier, manifest.pickup) {
            decision.intents.push(Intent::MoveUnits {
                units: vec![manifest.carrier],
                goal: manifest.pickup,
            });
        } else if !manifest.load_dispatched {
            decision.intents.push(Intent::Load {
                transport: manifest.carrier,
                riders: pending,
            });
            manifest.load_dispatched = true;
        } else if pending
            .iter()
            .all(|id| unit(obs, *id).is_some_and(|rider| rider.idle))
        {
            manifest.boarding_closed = true;
        }
    }
}

fn boarding_complete(operation: &LiftOperation, obs: &Observation) -> bool {
    let loaded_manifests = operation
        .manifests
        .iter()
        .filter(|manifest| carrier_cargo(obs, manifest.carrier) > 0)
        .count();
    let carrier_quorum = if operation.desired_carriers > 1 {
        operation.manifests.len().div_ceil(2).max(2)
    } else {
        1
    };
    let payload_quorum = operation.payload_target.div_ceil(2).max(MIN_EARLY_PAYLOAD);
    boarding_resolved(operation, obs)
        && loaded_manifests >= carrier_quorum
        && loaded_payload(operation, obs) >= payload_quorum
}

fn boarding_resolved(operation: &LiftOperation, obs: &Observation) -> bool {
    for manifest in &operation.manifests {
        if unit(obs, manifest.carrier).is_none() {
            continue;
        }
        let pending = manifest.riders.iter().any(|id| unit(obs, *id).is_some());
        if pending && !manifest.boarding_closed {
            return false;
        }
    }
    true
}

fn loaded_payload(operation: &LiftOperation, obs: &Observation) -> u32 {
    operation
        .manifests
        .iter()
        .map(|manifest| u32::from(carrier_cargo(obs, manifest.carrier)))
        .sum()
}

/// The one legal phase transition: every entry stamps its start tick,
/// so a phase-dwell read can never see a stale stamp from two phases
/// ago. Direct `.phase =` writes outside this helper are a bug.
fn enter(operation: &mut LiftOperation, phase: LiftPhase, now: Tick) {
    operation.phase = phase;
    operation.phase_started_at = now;
}

fn launch_or_recover(
    operation: &mut LiftOperation,
    obs: &Observation,
    decision: &mut StrategicDecision,
    handoff: &mut Vec<UnitId>,
    minimum_loaded_carriers: usize,
) {
    let loaded_carriers = operation
        .manifests
        .iter()
        .filter(|manifest| carrier_cargo(obs, manifest.carrier) > 0)
        .count();
    if boarding_complete(operation, obs) && loaded_carriers >= minimum_loaded_carriers {
        enter(operation, LiftPhase::Landing, obs.tick);
        operation.launched = true;
        land(operation, obs, decision, handoff);
    } else {
        enter(operation, LiftPhase::Recover, obs.tick);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupportDirective {
    Independent,
    Hold,
    Release,
    Abort,
    Unmatched,
}

fn support_directive(
    support: LiftAirSupport,
    player: PlayerId,
    target: TilePos,
) -> SupportDirective {
    let matches_enclave = |other_player: PlayerId, other_target: TilePos| {
        other_player == player && other_target == target
    };
    match support {
        LiftAirSupport::Independent => SupportDirective::Independent,
        LiftAirSupport::Suppressing {
            player: other_player,
            target: other_target,
        } => {
            if matches_enclave(other_player, other_target) {
                SupportDirective::Hold
            } else {
                SupportDirective::Unmatched
            }
        }
        LiftAirSupport::Released {
            player: other_player,
            target: other_target,
        } => {
            if matches_enclave(other_player, other_target) {
                SupportDirective::Release
            } else {
                SupportDirective::Unmatched
            }
        }
        LiftAirSupport::Aborted {
            player: other_player,
            target: other_target,
        } => {
            if matches_enclave(other_player, other_target) {
                SupportDirective::Abort
            } else {
                SupportDirective::Unmatched
            }
        }
    }
}

fn operation_support_directive(
    support: LiftAirSupport,
    operation: &LiftOperation,
) -> SupportDirective {
    support_directive(support, operation.target_player, operation.target)
}

fn land(
    operation: &mut LiftOperation,
    obs: &Observation,
    decision: &mut StrategicDecision,
    handoff: &mut Vec<UnitId>,
) {
    let target = operation.target;
    let mut claimed_drops: Vec<_> = operation
        .manifests
        .iter()
        .filter(|manifest| !manifest.aborted)
        .map(|manifest| manifest.drop)
        .collect();
    for manifest in &mut operation.manifests {
        let Some(carrier) = unit(obs, manifest.carrier) else {
            if operation.launched && !manifest.aborted {
                issue_assault(manifest, obs, target, decision, handoff);
            }
            manifest.closed = true;
            continue;
        };
        if carrier.cargo > 0 {
            if !carrier.idle || manifest.closed {
                continue;
            }
            if manifest.aborted {
                recover_loaded(manifest, obs, decision);
                continue;
            }
            if manifest.unload_attempts >= DROP_ATTEMPTS {
                issue_assault(manifest, obs, target, decision, handoff);
                manifest.aborted = true;
                recover_loaded(manifest, obs, decision);
                continue;
            }
            if manifest.unload_attempts > 0
                && let Some(alternate) = alternate_drop(obs, carrier.tile, target, &claimed_drops)
            {
                manifest.drop = alternate;
                claimed_drops.push(alternate);
            }
            manifest.unload_attempts += 1;
            decision.intents.push(Intent::Unload {
                transport: manifest.carrier,
                at: manifest.drop,
            });
            continue;
        }
        if operation.launched && !manifest.aborted {
            issue_assault(manifest, obs, target, decision, handoff);
        }
        return_carrier(manifest, carrier, obs, decision);
    }
}

fn recover(
    operation: &mut LiftOperation,
    obs: &Observation,
    decision: &mut StrategicDecision,
    handoff: &mut Vec<UnitId>,
) {
    if !operation.launched {
        cancel_pending_boarding(operation, obs, decision);
    }
    for manifest in &mut operation.manifests {
        let Some(carrier) = unit(obs, manifest.carrier) else {
            if operation.launched && !manifest.aborted {
                issue_assault(manifest, obs, operation.target, decision, handoff);
            }
            manifest.closed = true;
            continue;
        };
        if carrier.cargo > 0 {
            if carrier.idle {
                recover_loaded(manifest, obs, decision);
            }
            continue;
        }
        if operation.launched && !manifest.aborted {
            issue_assault(manifest, obs, operation.target, decision, handoff);
        }
        return_carrier(manifest, carrier, obs, decision);
    }
}

fn cancel_pending_boarding(
    operation: &LiftOperation,
    obs: &Observation,
    decision: &mut StrategicDecision,
) {
    let mut riders: Vec<_> = operation
        .manifests
        .iter()
        .filter(|manifest| manifest.load_dispatched)
        .flat_map(|manifest| manifest.riders.iter().copied())
        .filter(|id| unit(obs, *id).is_some_and(|rider| !rider.idle))
        .collect();
    riders.sort_unstable();
    riders.dedup();
    if !riders.is_empty() {
        decision.intents.push(Intent::StopUnits { units: riders });
    }
}

fn close_missing_manifests(
    operation: &mut LiftOperation,
    obs: &Observation,
) -> (bool, Vec<UnitId>) {
    let mut carrier_lost = false;
    let mut released_riders = Vec::new();
    for manifest in &mut operation.manifests {
        if manifest.closed || unit(obs, manifest.carrier).is_some() {
            continue;
        }
        carrier_lost = true;
        if manifest.load_dispatched {
            released_riders.extend(
                manifest
                    .riders
                    .iter()
                    .copied()
                    .filter(|id| unit(obs, *id).is_some_and(|rider| !rider.idle)),
            );
        }
        manifest.boarding_closed = true;
        manifest.closed = true;
    }
    released_riders.sort_unstable();
    released_riders.dedup();
    (carrier_lost, released_riders)
}

fn issue_assault(
    manifest: &mut LiftManifest,
    obs: &Observation,
    target: TilePos,
    decision: &mut StrategicDecision,
    handoff: &mut Vec<UnitId>,
) {
    if manifest.attack_issued {
        return;
    }
    let mut routes = RouteProjection::known_ground(obs);
    let landed: Vec<_> = manifest
        .riders
        .iter()
        .copied()
        .filter(|id| unit(obs, *id).is_some_and(|rider| routes.unit_reaches(rider, manifest.drop)))
        .collect();
    if landed.is_empty() {
        return;
    }
    handoff.extend(landed.iter().copied());
    decision.intents.push(Intent::AttackMoveUnits {
        units: landed,
        goal: target,
    });
    manifest.attack_issued = true;
}

fn sustain_landed_assault(
    operation: &LiftOperation,
    obs: &Observation,
    assault_waypoints: &mut Vec<TilePos>,
    decision: &mut StrategicDecision,
    reservations: &mut Vec<UnitId>,
) -> bool {
    let survivors = landed_survivors(operation, obs);
    if survivors.is_empty() {
        return true;
    }
    let issued_this_tick = survivors.iter().any(|id| reservations.contains(id));

    if issued_this_tick
        || survivors
            .iter()
            .filter_map(|id| unit(obs, *id))
            .any(|member| !member.idle)
        || exact_target_remains(obs, operation)
    {
        reservations.extend(survivors);
        return false;
    }

    let Some(goal) = followup_assault_goal(obs, operation, &survivors, assault_waypoints) else {
        return true;
    };
    reservations.extend(survivors.iter().copied());
    assault_waypoints.push(goal);
    assault_waypoints.sort_unstable_by_key(|tile| (tile.y, tile.x));
    assault_waypoints.dedup();
    decision.intents.push(Intent::AttackMoveUnits {
        units: survivors,
        goal,
    });
    false
}

fn landed_survivors(operation: &LiftOperation, obs: &Observation) -> Vec<UnitId> {
    let mut routes = RouteProjection::known_ground(obs);
    let mut survivors = Vec::new();
    for manifest in operation
        .manifests
        .iter()
        .filter(|manifest| manifest.attack_issued && !manifest.aborted)
    {
        survivors.extend(manifest.riders.iter().copied().filter(|id| {
            unit(obs, *id).is_some_and(|member| routes.unit_reaches(member, manifest.drop))
        }));
    }
    survivors.sort_unstable();
    survivors.dedup();
    survivors
}

fn exact_target_remains(obs: &Observation, operation: &LiftOperation) -> bool {
    obs.enemy_buildings.iter().any(|building| {
        building.id == operation.target_id
            && building.player == operation.target_player
            && building.anchor == operation.target
    })
}

fn followup_assault_goal(
    obs: &Observation,
    operation: &LiftOperation,
    survivors: &[UnitId],
    attempted: &[TilePos],
) -> Option<TilePos> {
    let mut known_routes = RouteProjection::known_ground(obs);
    if let Some(building) = obs
        .enemy_buildings
        .iter()
        .filter(|building| {
            building.built
                && building.seen
                && !attempted.contains(&building.anchor)
                && known_routes.group_reaches_command_goal(survivors, building.anchor)
        })
        .min_by_key(|building| {
            (
                building.player != operation.target_player,
                building.kind != BuildingKind::Foundry,
                group_distance(obs, survivors, building.anchor),
                building.anchor.y,
                building.anchor.x,
                building.player,
                building.id,
            )
        })
    {
        return Some(building.anchor);
    }

    let mut projected_routes = RouteProjection::new(obs, Domain::Ground);
    (0..obs.map_height)
        .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
        .filter(|tile| {
            !obs.explored(*tile)
                && !attempted.contains(tile)
                && projected_routes.group_reaches_command_goal(survivors, *tile)
        })
        .min_by_key(|tile| (group_distance(obs, survivors, *tile), tile.y, tile.x))
}

fn group_distance(obs: &Observation, members: &[UnitId], goal: TilePos) -> i32 {
    members
        .iter()
        .filter_map(|id| unit(obs, *id))
        .map(|member| member.tile.manhattan(goal))
        .max()
        .unwrap_or(i32::MAX)
}

fn return_carrier(
    manifest: &mut LiftManifest,
    carrier: &UnitObs,
    obs: &Observation,
    decision: &mut StrategicDecision,
) {
    if manifest.closed || !carrier.idle {
        return;
    }
    if terminal_at(carrier, manifest.pickup) {
        manifest.closed = true;
        return;
    }
    let mut air = RouteProjection::new(obs, Domain::Air);
    if manifest.recovery_attempts >= DROP_ATTEMPTS || !air.reaches(carrier.tile, manifest.pickup) {
        manifest.closed = true;
        return;
    }
    manifest.recovery_attempts += 1;
    decision.intents.push(Intent::MoveUnits {
        units: vec![manifest.carrier],
        goal: manifest.pickup,
    });
}

fn reservations(
    operation: &LiftOperation,
    obs: &Observation,
    unavailable: &[UnitId],
) -> Vec<UnitId> {
    let mut ids = Vec::new();
    if operation.manifests.is_empty() {
        if operation.phase == LiftPhase::Provision {
            ids.extend(
                operation
                    .payload
                    .iter()
                    .copied()
                    .filter(|id| unit(obs, *id).is_some()),
            );
            ids.extend(
                obs.my_units
                    .iter()
                    .filter(|unit| {
                        unit.kind == UnitKind::Skyhook
                            && unit.cargo == 0
                            && unavailable.binary_search(&unit.id).is_err()
                    })
                    .map(|unit| unit.id)
                    .take(operation.desired_carriers),
            );
        }
    } else {
        for manifest in &operation.manifests {
            if !manifest.closed && unit(obs, manifest.carrier).is_some() {
                ids.push(manifest.carrier);
            }
            if !manifest.closed && !manifest.attack_issued {
                ids.extend(
                    manifest
                        .riders
                        .iter()
                        .copied()
                        .filter(|id| unit(obs, *id).is_some()),
                );
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn recovery_complete(operation: &LiftOperation, obs: &Observation) -> bool {
    operation.manifests.iter().all(|manifest| {
        manifest.closed
            || unit(obs, manifest.carrier)
                .is_none_or(|carrier| carrier.cargo == 0 && terminal_at(carrier, manifest.pickup))
    })
}

fn terminal_at(unit: &UnitObs, goal: TilePos) -> bool {
    unit.idle && unit.tile.chebyshev(goal) <= 1
}

fn recover_loaded(
    manifest: &mut LiftManifest,
    obs: &Observation,
    decision: &mut StrategicDecision,
) {
    if manifest.recovery_attempts >= DROP_ATTEMPTS {
        manifest.closed = true;
        return;
    }
    let sites = core::iter::once(manifest.pickup)
        .chain(open_slots(obs, manifest.pickup, usize::from(DROP_ATTEMPTS)))
        .collect::<Vec<_>>();
    let at = sites
        .get(usize::from(manifest.recovery_attempts))
        .copied()
        .unwrap_or(manifest.pickup);
    manifest.recovery_attempts += 1;
    decision.intents.push(Intent::Unload {
        transport: manifest.carrier,
        at,
    });
}

fn alternate_drop(
    obs: &Observation,
    from: TilePos,
    target: TilePos,
    claimed: &[TilePos],
) -> Option<TilePos> {
    let mut air = RouteProjection::new(obs, Domain::Air);
    open_slots(obs, target, claimed.len().saturating_add(32))
        .into_iter()
        .find(|tile| !claimed.contains(tile) && air.reaches(from, *tile))
}

fn carrier_cargo(obs: &Observation, carrier: UnitId) -> u8 {
    unit(obs, carrier).map_or(0, |unit| unit.cargo)
}

fn unit(obs: &Observation, id: UnitId) -> Option<&UnitObs> {
    obs.my_units
        .binary_search_by_key(&id, |unit| unit.id)
        .ok()
        .map(|index| &obs.my_units[index])
}

fn landing_slots(obs: &Observation, from: TilePos, target: TilePos, count: usize) -> Vec<TilePos> {
    let mut air = RouteProjection::new(obs, Domain::Air);
    open_slots(obs, target, count.saturating_mul(3))
        .into_iter()
        .filter(|tile| air.reaches(from, *tile))
        .take(count)
        .collect()
}

fn planned_drop_slots(
    obs: &Observation,
    pickup: TilePos,
    target: TilePos,
    count: usize,
) -> Vec<TilePos> {
    landing_slots(obs, pickup, target, count)
}

fn pickup_slots(
    obs: &Observation,
    home: TilePos,
    component: TilePos,
    count: usize,
) -> Vec<TilePos> {
    if !routing::ground_open(obs, component) {
        return Vec::new();
    }
    let map_cells = usize::try_from(obs.map_width)
        .ok()
        .and_then(|width| {
            usize::try_from(obs.map_height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .unwrap_or(0);
    let candidates = open_slots(obs, home, map_cells);
    let mut routes = RouteProjection::known_ground(obs);
    candidates
        .into_iter()
        .filter(|tile| routes.reaches(component, *tile))
        .take(count)
        .collect()
}

fn pickup_component_anchors(obs: &Observation, home: TilePos) -> Vec<TilePos> {
    let radius = BuildingKind::Foundry.base_stats().vision;
    let count = usize::try_from(radius.saturating_mul(2).saturating_add(1))
        .ok()
        .and_then(|width| width.checked_mul(width))
        .unwrap_or(0);
    let candidates = open_slots_within(obs, home, radius, count);
    let mut anchors = Vec::new();
    let mut routes = RouteProjection::known_ground(obs);
    for candidate in candidates {
        if anchors
            .iter()
            .copied()
            .any(|anchor| routes.reaches(anchor, candidate))
        {
            continue;
        }
        anchors.push(candidate);
    }
    anchors
}

fn open_slots(obs: &Observation, center: TilePos, count: usize) -> Vec<TilePos> {
    open_slots_within(obs, center, obs.map_width.max(obs.map_height).max(1), count)
}

fn open_slots_within(
    obs: &Observation,
    center: TilePos,
    radius: i32,
    count: usize,
) -> Vec<TilePos> {
    let mut slots = Vec::with_capacity(count);
    for r in 1..=radius {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let tile = center.offset(dx, dy);
                if routing::ground_open(obs, tile) {
                    slots.push(tile);
                    if slots.len() == count {
                        return slots;
                    }
                }
            }
        }
    }
    slots
}

fn footprint_ring(anchor: TilePos, size: (i32, i32)) -> Vec<TilePos> {
    let (width, height) = size;
    let mut ring = Vec::new();
    for x in anchor.x - 1..=anchor.x + width {
        ring.push(TilePos::new(x, anchor.y - 1));
        ring.push(TilePos::new(x, anchor.y + height));
    }
    for y in anchor.y..anchor.y + height {
        ring.push(TilePos::new(anchor.x - 1, y));
        ring.push(TilePos::new(anchor.x + width, y));
    }
    ring.sort_unstable_by_key(|tile| (tile.y, tile.x));
    ring.dedup();
    ring
}

#[cfg(test)]
mod tests {
    use super::super::difficulty::DifficultyTuning;
    use super::*;
    use crate::bot::intelligence::{BuildingContact, ContactEvidence};
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION};
    use crate::scenario::BotDifficulty;
    use crate::state::Faction;

    const HOME: TilePos = TilePos::new(5, 15);
    const TARGET: TilePos = TilePos::new(50, 15);

    #[test]
    fn remembered_proven_island_holds_only_missing_first_carrier_capital() {
        let mut obs = island_obs();
        obs.tick = 100;
        add_fighters(&mut obs, 3);
        obs.my_buildings
            .push(building(2, 0, BuildingKind::Airworks, HOME.offset(4, -4)));
        obs.my_queues.push(Vec::new());
        let target = remembered_target(0);
        let planner = LiftPlanner::new();

        assert_eq!(
            planner.prospective_first_carrier_commitment(&obs, HOME, &[], &[], 0, &target),
            UnitKind::Skyhook.stats().cost
        );
        assert!(planner.operation().is_none());

        obs.my_queues
            .last_mut()
            .expect("the Airworks has a matching queue")
            .push(UnitKind::Skyhook);
        assert_eq!(
            planner.prospective_first_carrier_commitment(&obs, HOME, &[], &[], 0, &target),
            0,
            "an already-paid queued carrier releases the prospective escrow"
        );
        assert!(
            planner.operation().is_none(),
            "prospective accounting must not start or mutate a lift"
        );

        obs.my_queues
            .last_mut()
            .expect("the Airworks has a matching queue")
            .clear();
        assert_eq!(
            planner.prospective_first_carrier_commitment(&obs, HOME, &[], &[], 0, &target),
            UnitKind::Skyhook.stats().cost,
            "removing the queued carrier restores exactly one carrier's escrow"
        );

        let mut current = target.clone();
        current.evidence = ContactEvidence::Current;
        assert_eq!(
            planner.prospective_first_carrier_commitment(&obs, HOME, &[], &[], 0, &current),
            0,
            "current sight belongs to ordinary lift admission"
        );
    }

    #[test]
    fn reachable_unknown_and_expired_targets_do_not_hold_carrier_capital() {
        let mut obs = island_obs();
        obs.tick = 100;
        add_fighters(&mut obs, 3);
        obs.my_buildings
            .push(building(2, 0, BuildingKind::Airworks, HOME.offset(4, -4)));
        obs.my_queues.push(Vec::new());
        let target = remembered_target(0);
        let planner = LiftPlanner::new();

        let mut reachable = obs.clone();
        reachable.known_rock.clear();
        assert_eq!(
            planner.prospective_first_carrier_commitment(&reachable, HOME, &[], &[], 0, &target,),
            0,
            "a known open ground route needs no transport escrow"
        );

        let mut unknown = reachable;
        unknown.explored.fill(false);
        assert_eq!(
            planner.prospective_first_carrier_commitment(&unknown, HOME, &[], &[], 0, &target),
            0,
            "unknown terrain remains optimistically traversable"
        );

        let mut expired = obs;
        expired.tick = 3_601;
        assert_eq!(
            planner.prospective_first_carrier_commitment(&expired, HOME, &[], &[], 0, &target),
            0,
            "expired building memory cannot bank carrier capital"
        );
    }

    #[test]
    fn one_early_lift_is_valid_but_a_large_bank_of_fighters_scales_without_a_cap() {
        let mut early = island_obs();
        add_fighters(&mut early, 3);
        assert_eq!(desired_carriers(&early, HOME, &[]), 1);

        let mut wealthy = island_obs();
        add_fighters(&mut wealthy, 230);
        assert_eq!(
            desired_carriers(&wealthy, HOME, &[]),
            46,
            "230 one-slot fighters keep 20% home and demand 46 full Skyhooks"
        );
    }

    #[test]
    fn protected_prime_core_bounds_the_largest_admissible_lift_payload() {
        let plan_for = |fighters| {
            let mut obs = island_obs();
            add_fighters(&mut obs, fighters);
            let plan = initial_payload_plan_preserving_core(&obs, HOME, &[], &[], 8);
            (obs, plan)
        };

        let (_, exact) = plan_for(8);
        assert!(
            exact.is_none(),
            "Prime's exact opening core cannot supply the minimum lift payload"
        );

        for (fighters, payload, carriers) in [(12, 4, 1), (20, 12, 3)] {
            let (obs, plan) = plan_for(fighters);
            let plan = plan.expect("the surplus above Prime's core admits a lift");
            assert_eq!(plan.payload.len(), payload, "{fighters} fighters");
            assert_eq!(plan.payload_target, payload as u32, "{fighters} fighters");
            assert_eq!(plan.desired_carriers, carriers, "{fighters} fighters");
            assert!(
                combat_core_status(&obs, &plan.payload, &[], 8).ready,
                "the admitted {fighters}-fighter payload must leave Prime's exact core projected"
            );
        }
    }

    #[test]
    fn provisioning_loss_cannot_refill_a_lift_from_the_protected_home_core() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 200, Vec::new());
        let admission = LiftAdmission {
            allow_new_commitments: true,
            core_reservations: &[],
            minimum_core_equivalents: 8,
        };
        let mut planner = LiftPlanner::new();

        planner.think_with_admission(&obs, HOME, &[], LiftAirSupport::Independent, admission);
        let initial_payload = planner
            .operation()
            .expect("the four-unit surplus starts a lift")
            .payload
            .clone();
        assert_eq!(initial_payload.len(), 4);

        let lost = initial_payload[0];
        obs.my_units.retain(|unit| unit.id != lost);
        obs.tick += 1;
        planner.think_with_admission(&obs, HOME, &[], LiftAirSupport::Independent, admission);

        let replacement_payload = &planner
            .operation()
            .expect("the surviving lift remains active")
            .payload;
        assert_eq!(replacement_payload.len(), 3);
        assert!(
            combat_core_status(&obs, replacement_payload, &[], 8).ready,
            "the operation may keep its surviving riders but cannot draft a replacement from the exact home core"
        );
    }

    #[test]
    fn a_small_mixed_early_lift_is_not_starved_by_the_home_reserve() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 2);
        add_flakhounds(&mut obs, 100, 1);

        let (payload, payload_target, carriers) = initial_payload(&obs, HOME, &[]);

        assert_eq!(payload, [UnitId(1), UnitId(2), UnitId(100)]);
        assert_eq!(payload_target, 4);
        assert_eq!(carriers, 1);
    }

    #[test]
    fn a_mixed_bulk_lift_keeps_ground_capable_fighters_at_home() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_flakhounds(&mut obs, 100, 12);

        let (payload, payload_target, carriers) = initial_payload(&obs, HOME, &[]);
        let home_ground_room = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().can_target(Domain::Ground))
            .filter(|unit| payload.binary_search(&unit.id).is_err())
            .map(|unit| u32::from(unit.kind.stats().transport_size))
            .sum::<u32>();

        assert_eq!(home_ground_room, 4);
        assert_eq!(payload_target, 28);
        assert_eq!(carriers, 7);
        assert!(payload.iter().any(|id| id.0 >= 100));
    }

    #[test]
    fn a_wealthy_mixed_army_still_forms_an_uncapped_bulk_wave() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 230);
        add_flakhounds(&mut obs, 1_000, 230);

        let (payload, payload_target, carriers) = initial_payload(&obs, HOME, &[]);
        let home_ground_room = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().can_target(Domain::Ground))
            .filter(|unit| payload.binary_search(&unit.id).is_err())
            .map(|unit| u32::from(unit.kind.stats().transport_size))
            .sum::<u32>();

        assert_eq!(home_ground_room, 46);
        assert_eq!(payload_target, 552);
        assert_eq!(carriers, 138);
    }

    #[test]
    fn pickup_selection_uses_the_strongest_near_home_component_not_the_first_open_tile() {
        let obs = split_staging_obs(5, 20);
        let naive = open_slots(&obs, HOME, 1)[0];
        let naive_candidates = lift_candidates(&obs, naive, &[]);
        let plan = initial_payload_plan(&obs, HOME, &[])
            .expect("the larger home-side component supports a lift");

        assert_eq!(
            naive_candidates.len(),
            5,
            "the old anchor sees only the pocket"
        );
        assert_eq!(plan.payload.len(), 16, "four of twenty fighters stay home");
        assert!(
            plan.payload.iter().all(|id| id.0 >= 100),
            "the payload must come from the stronger component: {:?}",
            plan.payload
        );
        let mut routes = RouteProjection::known_ground(&obs);
        assert!(!routes.reaches(naive, plan.pickup));
        assert!(
            plan.payload
                .iter()
                .all(|id| { routes.unit_reaches(unit(&obs, *id).unwrap(), plan.pickup) })
        );
    }

    #[test]
    fn equal_pickup_components_use_distance_then_coordinates_as_canonical_ties() {
        let mut obs = island_obs();
        obs.my_buildings.clear();
        obs.my_queues.clear();
        obs.my_units.clear();
        let home = TilePos::new(10, 15);
        obs.known_rock
            .extend((0..obs.map_height).map(|y| TilePos::new(home.x, y)));
        obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
        obs.known_rock.dedup();
        obs.my_units.extend((1..=8).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(
                    7 + i32::try_from(id % 2).unwrap(),
                    10 + i32::try_from(id / 2).unwrap(),
                ),
            )
        }));
        obs.my_units.extend((101..=108).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(
                    12 + i32::try_from(id % 2).unwrap(),
                    10 + i32::try_from(id % 5).unwrap(),
                ),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);

        let plan = initial_payload_plan(&obs, home, &[])
            .expect("both symmetric components support an equal lift");

        assert_eq!(plan.pickup, TilePos::new(9, 14));
        assert_eq!(plan.payload, (1..=8).map(UnitId).collect::<Vec<_>>());
    }

    #[test]
    fn shared_admission_freezes_one_reachable_pickup_component() {
        let mut obs = split_staging_obs(5, 20);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let initial = planner
            .operation()
            .expect("the stronger component starts provisioning")
            .clone();
        assert_eq!(initial.phase, LiftPhase::Provision);
        assert_eq!(initial.desired_carriers, 4);

        obs.my_units.extend((200..208).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(
                    2 + i32::try_from(id % 2).unwrap(),
                    8 + i32::try_from(id % 12).unwrap(),
                ),
            )
        }));
        obs.my_units.extend((300..340).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(
                    8 + i32::try_from(id % 4).unwrap(),
                    8 + i32::try_from(id % 12).unwrap(),
                ),
            )
        }));
        obs.my_units.extend((900..906).map(|id| {
            own(
                id,
                UnitKind::Skyhook,
                HOME.offset(i32::try_from(id - 900).unwrap(), 8),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;

        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let frozen = planner
            .operation()
            .expect("the grown wave assigns exact manifests")
            .clone();
        assert_eq!(frozen.phase, LiftPhase::Boarding);
        assert_eq!(frozen.pickup_component, initial.pickup_component);
        assert_eq!(frozen.desired_carriers, initial.desired_carriers);
        assert!(
            frozen
                .payload
                .iter()
                .all(|id| (100..120).contains(&id.0) || (200..208).contains(&id.0)),
            "a larger remote roster cannot switch the operation's component"
        );
        assert_eq!(frozen.manifests.len(), frozen.desired_carriers);
        let mut routes = RouteProjection::known_ground(&obs);
        for manifest in &frozen.manifests {
            assert!(routes.reaches(frozen.pickup_component, manifest.pickup));
            for rider in &manifest.riders {
                assert!(
                    routes.unit_reaches(unit(&obs, *rider).unwrap(), manifest.pickup),
                    "rider {rider} cannot reach its frozen pickup {:?}",
                    manifest.pickup
                );
            }
        }

        obs.my_units.extend((400..450).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(
                    9 + i32::try_from(id % 3).unwrap(),
                    6 + i32::try_from(id % 16).unwrap(),
                ),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let retained = planner.operation().expect("boarding remains active");
        assert_eq!(retained.pickup_component, frozen.pickup_component);
        assert_eq!(retained.payload, frozen.payload);
        assert_eq!(retained.manifests, frozen.manifests);
    }

    #[test]
    fn provisioning_rebalances_if_the_reserved_ground_screen_is_lost() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_flakhounds(&mut obs, 100, 12);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();

        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let original_payload = planner.operation().unwrap().payload.clone();
        let reserved_ground: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().can_target(Domain::Ground))
            .filter(|unit| original_payload.binary_search(&unit.id).is_err())
            .map(|unit| unit.id)
            .collect();
        assert_eq!(reserved_ground.len(), 4);

        obs.my_units
            .retain(|unit| reserved_ground.binary_search(&unit.id).is_err());
        obs.tick += 1;
        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().unwrap();
        let home_ground_room = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().can_target(Domain::Ground))
            .filter(|unit| operation.payload.binary_search(&unit.id).is_err())
            .map(|unit| u32::from(unit.kind.stats().transport_size))
            .sum::<u32>();

        assert_eq!(operation.phase, LiftPhase::Provision);
        assert_eq!(home_ground_room, 4);
        assert_eq!(decision.reservations, *operation.payload);
    }

    #[test]
    fn connected_objectives_do_not_start_a_lift() {
        let mut obs = island_obs();
        obs.known_rock.clear();
        add_fighters(&mut obs, 12);

        let mut planner = LiftPlanner::new();
        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_none());
        assert_eq!(decision, StrategicDecision::default());
    }

    #[test]
    fn a_blocked_pickup_cannot_make_an_enemy_look_disconnected() {
        let mut obs = island_obs();
        let pickup = HOME.offset(0, -2);
        let target = obs.enemy_buildings[0].clone();
        assert!(
            disconnected(&obs, pickup, &target),
            "the open home-side pickup and enemy Foundry begin in separate components"
        );

        obs.known_rock.push(pickup);
        obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
        assert!(
            !disconnected(&obs, pickup, &target),
            "a blocked pickup is not a component and must fail closed"
        );
    }

    #[test]
    fn a_lift_without_home_side_staging_ground_never_commits_a_wave() {
        let mut obs = island_obs();
        obs.known_rock = (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .collect();
        add_fighters(&mut obs, 12);
        obs.my_units.push(own(900, UnitKind::Skyhook, HOME));

        let mut planner = LiftPlanner::new();
        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert_eq!(desired_carriers(&obs, HOME, &[]), 0);
        assert!(pickup_component_anchors(&obs, HOME).is_empty());
        assert!(planner.operation().is_none());
        assert_eq!(decision, StrategicDecision::default());
    }

    #[test]
    fn losing_home_side_staging_ground_cancels_an_unlaunched_wave() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Provision);

        obs.known_rock = (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .collect();
        obs.tick += 1;
        let cancelled = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_none());
        assert!(planner.retry_not_before > obs.tick);
        assert!(cancelled.intents.is_empty());
        assert!(cancelled.reservations.is_empty());
    }

    #[test]
    fn remembered_objectives_do_not_start_repeated_lift_operations() {
        let mut obs = island_obs();
        obs.enemy_buildings[0].seen = false;
        add_fighters(&mut obs, 12);
        obs.my_units.push(own(900, UnitKind::Skyhook, HOME));

        let mut planner = LiftPlanner::new();
        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_none());
        assert_eq!(decision, StrategicDecision::default());
    }

    #[test]
    fn matching_suppression_cannot_reserve_an_assault_against_a_remembered_objective() {
        let mut obs = island_obs();
        obs.enemy_buildings[0].seen = false;
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();

        let decision = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        assert!(planner.operation().is_none());
        assert!(!planner.support_latched);
        assert_eq!(decision, StrategicDecision::default());
    }

    #[test]
    fn support_selects_only_its_exact_objective_and_otherwise_leaves_selection_independent() {
        let mut obs = island_obs();
        let supported = TARGET.offset(0, 8);
        obs.enemy_buildings
            .push(building(501, 1, BuildingKind::Foundry, supported));
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());

        let mut coordinated = LiftPlanner::new();
        coordinated.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: supported,
            },
        );
        assert_eq!(coordinated.operation().unwrap().target, supported);
        assert!(coordinated.support_latched);

        let mut unmatched = LiftPlanner::new();
        unmatched.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET.offset(0, 1),
            },
        );
        assert_eq!(unmatched.operation().unwrap().target, TARGET);
        assert!(!unmatched.support_latched);
    }

    #[test]
    fn provisioning_spreads_a_large_wave_across_shallow_airworks_queues() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 20);
        add_airworks(&mut obs, 10, Vec::new());
        add_airworks(&mut obs, 11, Vec::new());
        obs.scrap = 1_000;

        let mut planner = LiftPlanner::new();
        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let orders: Vec<_> = decision
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::TrainAt {
                    building,
                    kind: UnitKind::Skyhook,
                } => Some(*building),
                _ => None,
            })
            .collect();

        assert_eq!(planner.operation().unwrap().desired_carriers, 4);
        assert_eq!(
            orders,
            [
                BuildingId(10),
                BuildingId(11),
                BuildingId(10),
                BuildingId(11)
            ]
        );
        assert_eq!(decision.committed_scrap, 1_000);
    }

    #[test]
    fn provisioning_counts_live_and_queued_carriers_before_buying_more() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 20);
        obs.my_units.push(own(900, UnitKind::Skyhook, HOME));
        let mut occupied = own(901, UnitKind::Skyhook, HOME.offset(1, 0));
        occupied.cargo = 4;
        obs.my_units.push(occupied);
        add_airworks(&mut obs, 10, vec![UnitKind::Skyhook]);
        add_airworks(&mut obs, 11, Vec::new());
        obs.scrap = 1_000;

        let mut planner = LiftPlanner::new();
        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert_eq!(
            decision
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::TrainAt {
                        kind: UnitKind::Skyhook,
                        ..
                    }
                ))
                .count(),
            2
        );
        assert_eq!(decision.committed_scrap, 500);
    }

    #[test]
    fn full_airworks_queues_hold_only_the_next_missing_carriers_cost() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 20);
        add_airworks(&mut obs, 10, vec![UnitKind::Skyhook; SHALLOW_QUEUE_DEPTH]);
        obs.scrap = 1_000;

        let mut planner = LiftPlanner::new();
        let waiting = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert_eq!(planner.operation().unwrap().desired_carriers, 4);
        assert!(waiting.intents.is_empty());
        assert_eq!(waiting.committed_scrap, UnitKind::Skyhook.stats().cost);
    }

    #[test]
    fn a_missing_airworks_does_not_starve_the_tech_path() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        obs.scrap = 900;

        let mut planner = LiftPlanner::new();
        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(decision.intents.is_empty());
        assert_eq!(decision.committed_scrap, 0);
        assert!(decision.reservations.is_empty());
        assert!(planner.operation().is_none());
    }

    #[test]
    fn losing_the_airworks_mid_provision_does_not_reserve_phantom_spending() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());
        obs.scrap = 900;
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Provision);

        let airworks_index = obs
            .my_buildings
            .iter()
            .position(|building| building.kind == BuildingKind::Airworks)
            .unwrap();
        obs.my_buildings.remove(airworks_index);
        obs.my_queues.remove(airworks_index);
        obs.tick += 1;
        let waiting = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Provision);
        assert!(waiting.intents.is_empty());
        assert_eq!(waiting.committed_scrap, 0);
        assert!(!waiting.reservations.is_empty());
    }

    #[test]
    fn provision_immediately_reserves_a_canonical_payload() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());
        let mut first = LiftPlanner::new();
        let mut second = LiftPlanner::new();

        let first_decision = first.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let second_decision = second.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = first.operation().unwrap();

        assert_eq!(operation.phase, LiftPhase::Provision);
        assert_eq!(operation.desired_carriers, 2);
        assert_eq!(operation.payload_target, 8);
        assert_eq!(operation.payload.len(), 8);
        assert_eq!(operation.planned_drops.len(), operation.desired_carriers);
        assert!(
            operation
                .planned_drops
                .iter()
                .enumerate()
                .all(|(index, drop)| !operation.planned_drops[..index].contains(drop))
        );
        assert!(operation.payload.windows(2).all(|ids| ids[0] < ids[1]));
        assert_eq!(first_decision.reservations, *operation.payload);
        assert_eq!(first_decision, second_decision);
        assert_eq!(first, second);
    }

    #[test]
    fn closed_admission_blocks_a_new_lift_but_an_active_lift_starts_boarding() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());
        obs.scrap = 10_000;
        let mut blocked = LiftPlanner::new();

        assert_eq!(
            blocked.think_with_admission(
                &obs,
                HOME,
                &[],
                LiftAirSupport::Independent,
                LiftAdmission {
                    allow_new_commitments: false,
                    core_reservations: &[],
                    minimum_core_equivalents: 0,
                },
            ),
            StrategicDecision::default()
        );
        assert!(blocked.operation().is_none());

        obs.scrap = 0;
        let mut active = LiftPlanner::new();
        active.think_with_admission(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: true,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        );
        assert_eq!(
            active.operation().map(|operation| operation.phase),
            Some(LiftPhase::Provision)
        );

        obs.scrap = UnitKind::Skyhook.stats().cost;
        obs.tick += 1;
        let paused = active.think_with_admission(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: false,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        );
        assert_eq!(
            active.operation().map(|operation| operation.phase),
            Some(LiftPhase::Provision)
        );
        assert_eq!(paused.committed_scrap, 0);
        assert!(
            paused
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
        );

        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME),
            own(901, UnitKind::Skyhook, HOME.offset(1, 0)),
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;
        let continued = active.think_with_admission(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Independent,
            LiftAdmission {
                allow_new_commitments: false,
                core_reservations: &[],
                minimum_core_equivalents: 0,
            },
        );

        let operation = active
            .operation()
            .expect("closed admission preserves the in-flight lift");
        assert_eq!(operation.phase, LiftPhase::Boarding);
        assert_eq!(operation.manifests.len(), operation.desired_carriers);
        assert_eq!(continued.committed_scrap, 0);
        assert!(
            continued
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::TrainAt { .. }))
        );
        assert!(
            continued
                .intents
                .iter()
                .any(|intent| matches!(intent, Intent::MoveUnits { .. } | Intent::Load { .. }))
        );
    }

    #[test]
    fn every_difficulty_freezes_the_same_growing_roster_at_shared_admission() {
        let mut snapshots = Vec::new();
        for difficulty in BotDifficulty::ALL {
            let tuning = DifficultyTuning::for_level(difficulty);
            let mut planner = LiftPlanner::new();
            let mut decision = StrategicDecision::default();
            let mut shared = island_obs();
            for tick in (tuning.cadence..=24).step_by(tuning.cadence as usize) {
                shared = island_obs();
                shared.tick = tick;
                add_fighters(&mut shared, u32::try_from(4 + tick / 2).unwrap());
                add_airworks(&mut shared, 10, Vec::new());
                decision = planner.think(&shared, HOME, &[], LiftAirSupport::Independent);
                if tick < 24 {
                    assert!(planner.operation().is_none(), "{difficulty:?} at {tick}");
                }
            }
            let expected = initial_payload(&shared, HOME, &[]);
            let operation = planner.operation().unwrap();

            assert_eq!(operation.phase, LiftPhase::Provision, "{difficulty:?}");
            assert_eq!(operation.desired_carriers, expected.2, "{difficulty:?}");
            assert_eq!(operation.payload_target, expected.1, "{difficulty:?}");
            assert_eq!(
                operation
                    .payload
                    .iter()
                    .filter_map(|id| unit(&shared, *id))
                    .map(|member| u32::from(member.kind.stats().transport_size))
                    .sum::<u32>(),
                expected.1,
                "{difficulty:?}"
            );
            assert_eq!(decision.reservations, *operation.payload, "{difficulty:?}");
            assert!(operation.manifests.is_empty(), "{difficulty:?}");
            snapshots.push((operation.desired_carriers, operation.payload.clone()));
        }

        assert!(snapshots.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[test]
    fn provision_keeps_its_feasible_wave_when_growth_has_too_few_landing_slots() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());

        let pickup = initial_payload_plan(&obs, HOME, &[])
            .expect("the home roster has a legal pickup component")
            .pickup;
        let drops = [TARGET.offset(-1, -1), TARGET.offset(0, -1)];
        let mut air_corridor = Vec::new();
        let mut cursor = pickup;
        air_corridor.push(cursor);
        while cursor.x != drops[0].x {
            cursor = cursor.offset((drops[0].x - cursor.x).signum(), 0);
            air_corridor.push(cursor);
        }
        while cursor.y != drops[0].y {
            cursor = cursor.offset(0, (drops[0].y - cursor.y).signum());
            air_corridor.push(cursor);
        }
        air_corridor.push(drops[1]);
        air_corridor.sort_unstable_by_key(|tile| (tile.y, tile.x));
        air_corridor.dedup();

        obs.known_peaks = (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .filter(|tile| {
                air_corridor
                    .binary_search_by_key(&(tile.y, tile.x), |tile| (tile.y, tile.x))
                    .is_err()
            })
            .collect();
        obs.known_rock.extend(
            air_corridor
                .iter()
                .copied()
                .filter(|tile| *tile != pickup && !drops.contains(tile)),
        );
        obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
        obs.known_rock.dedup();

        assert_eq!(planned_drop_slots(&obs, pickup, TARGET, 2), drops);
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let smaller = planner
            .operation()
            .expect("the feasible wave starts")
            .clone();
        assert_eq!(smaller.phase, LiftPhase::Provision);
        assert_eq!(smaller.desired_carriers, 2);

        obs.my_units.extend((100..=107).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(8 + i32::try_from(id % 8).unwrap(), 10),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;
        assert!(initial_payload(&obs, HOME, &[]).2 > smaller.desired_carriers);
        assert_eq!(
            planned_drop_slots(&obs, pickup, TARGET, initial_payload(&obs, HOME, &[]).2,),
            drops,
            "the larger roster still has only the original two honest landing slots"
        );

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let retained = planner
            .operation()
            .expect("failed growth must not discard the feasible wave");
        assert_eq!(retained.phase, LiftPhase::Provision);
        assert_eq!(retained.desired_carriers, smaller.desired_carriers);
        assert_eq!(retained.payload, smaller.payload);
        assert_eq!(retained.payload_target, smaller.payload_target);
        assert_eq!(retained.planned_drops, smaller.planned_drops);
        assert!(retained.manifests.is_empty());
        assert_eq!(decision.reservations, *retained.payload);
    }

    #[test]
    fn boarding_freezes_carriers_payload_drop_envelope_and_manifests() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME.offset(0, 8)),
            own(901, UnitKind::Skyhook, HOME.offset(1, 8)),
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        let mut planner = LiftPlanner::new();

        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let frozen = planner
            .operation()
            .expect("the available carriers commit exact manifests")
            .clone();
        assert_eq!(frozen.phase, LiftPhase::Boarding);
        assert_eq!(frozen.desired_carriers, 2);
        assert_eq!(frozen.manifests.len(), 2);

        obs.my_units.extend((100..=179).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(8 + i32::try_from(id % 12).unwrap(), 8),
            )
        }));
        obs.my_units.extend([
            own(902, UnitKind::Skyhook, HOME.offset(2, 8)),
            own(903, UnitKind::Skyhook, HOME.offset(3, 8)),
            own(904, UnitKind::Skyhook, HOME.offset(4, 8)),
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;
        assert!(initial_payload(&obs, HOME, &[]).2 > frozen.desired_carriers);

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let committed = planner
            .operation()
            .expect("boarding continues with the committed wave");
        assert_eq!(committed.phase, LiftPhase::Boarding);
        assert_eq!(committed.desired_carriers, frozen.desired_carriers);
        assert_eq!(committed.payload, frozen.payload);
        assert_eq!(committed.payload_target, frozen.payload_target);
        assert_eq!(
            committed.ground_payload_target,
            frozen.ground_payload_target
        );
        assert_eq!(committed.planned_drops, frozen.planned_drops);
        assert_eq!(committed.manifests, frozen.manifests);
        assert!(
            decision
                .reservations
                .iter()
                .all(|id| frozen.payload.contains(id) || matches!(id.0, 900 | 901)),
            "later riders and carriers must remain available to other plans"
        );
    }

    #[test]
    fn provision_refills_losses_without_collapsing_the_frozen_wave() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let original = planner.operation().unwrap().payload.clone();
        let lost = original[0];

        obs.my_units.retain(|unit| unit.id != lost);
        obs.tick += 1;
        let refilled = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().unwrap();
        assert_eq!(operation.desired_carriers, 2);
        assert_eq!(operation.payload.len(), 8);
        assert!(!operation.payload.contains(&lost));
        assert!(
            original
                .iter()
                .filter(|id| **id != lost)
                .all(|id| operation.payload.contains(id))
        );
        assert_eq!(refilled.reservations, *operation.payload);

        let survivors: Vec<_> = operation.payload.iter().copied().take(3).collect();
        obs.my_units.retain(|unit| survivors.contains(&unit.id));
        obs.tick += 1;
        let waiting = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::Provision);
        assert_eq!(operation.desired_carriers, 2);
        assert_eq!(operation.payload, survivors);
        assert_eq!(operation.payload_target, 8);
        assert!(operation.manifests.is_empty());
        assert_eq!(waiting.reservations, survivors);

        obs.my_units.retain(|unit| unit.id != survivors[0]);
        obs.tick += 1;
        let aborted = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(planner.operation().is_none());
        assert!(aborted.reservations.is_empty());
        assert!(aborted.intents.is_empty());
    }

    #[test]
    fn an_unpackable_replacement_roster_below_the_frozen_floor_aborts_provisioning() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 3);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let frozen = planner.operation().expect("the early lift begins").clone();
        assert_eq!(frozen.phase, LiftPhase::Provision);
        assert_eq!(frozen.desired_carriers, 1);
        assert_eq!(frozen.payload_target, 3);
        assert_eq!(frozen.ground_payload_target, 3);

        obs.my_units
            .retain(|unit| !frozen.payload.contains(&unit.id));
        add_flakhounds(&mut obs, 100, 6);
        let available_room: u32 = lift_candidates(&obs, frozen.pickup_component, &[])
            .into_iter()
            .map(|unit| u32::from(unit.kind.stats().transport_size))
            .sum();
        assert_eq!(UnitKind::Flakhound.stats().transport_size, 2);
        assert_eq!(available_room, 12);
        obs.tick += 1;

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_none());
        assert!(planner.retry_not_before > obs.tick);
        assert!(decision.intents.is_empty());
        assert!(decision.reservations.is_empty());
    }

    #[test]
    fn a_frozen_drop_envelope_is_not_recomputed_when_a_site_becomes_blocked() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let planned = planner.operation().unwrap().planned_drops.clone();

        obs.enemy_buildings
            .push(building(600, 1, BuildingKind::Barricade, planned[0]));
        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME.offset(0, 8)),
            own(901, UnitKind::Skyhook, HOME.offset(1, 8)),
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;

        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().expect("the wave keeps provisioning");
        assert_eq!(operation.phase, LiftPhase::Provision);
        assert_eq!(operation.planned_drops, planned);
        assert!(operation.manifests.is_empty());
    }

    #[test]
    fn replacement_roster_is_trimmed_to_the_frozen_carrier_capacity() {
        let mut obs = island_obs();
        obs.my_units = vec![
            own(9, UnitKind::Bombard, HOME.offset(3, 0)),
            own(10, UnitKind::Bombard, HOME.offset(4, 0)),
            own(11, UnitKind::Lancer, HOME.offset(5, 0)),
            own(900, UnitKind::Skyhook, HOME.offset(0, 8)),
            own(901, UnitKind::Skyhook, HOME.offset(1, 8)),
        ];
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        let mut planner = LiftPlanner {
            operation: Some(LiftOperation {
                target_player: PlayerId(1),
                target_id: BuildingId(500),
                target: TARGET,
                phase: LiftPhase::Provision,
                started_at: obs.tick,
                phase_started_at: obs.tick,
                deadline: obs.tick.saturating_add(boarding_grace()),
                pickup_component: initial_payload_plan(&obs, HOME, &[])
                    .expect("the replacement roster has a pickup component")
                    .pickup,
                desired_carriers: 2,
                payload: (1..=8).map(UnitId).collect(),
                payload_target: 8,
                ground_payload_target: 8,
                planned_drops: planned_drop_slots(&obs, HOME, TARGET, 2),
                manifests: Vec::new(),
                launched: false,
            }),
            support_latched: false,
            support_released: false,
            assault_waypoints: Vec::new(),
            retry_not_before: 0,
        };

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().expect("the trimmed wave proceeds");

        assert_eq!(operation.phase, LiftPhase::Boarding);
        assert_eq!(operation.payload, [UnitId(9), UnitId(10)]);
        assert_eq!(operation.manifests.len(), 2);
        assert!(operation.manifests.iter().all(|manifest| {
            manifest.riders.len() == 1 && matches!(manifest.riders[0], UnitId(9) | UnitId(10))
        }));
        assert!(!decision.reservations.contains(&UnitId(11)));
    }

    #[test]
    fn attrition_cannot_collapse_a_frozen_two_carrier_wave_into_one_manifest() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 3);
        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME.offset(0, 8)),
            own(901, UnitKind::Skyhook, HOME.offset(1, 8)),
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        let mut planner = LiftPlanner {
            operation: Some(LiftOperation {
                target_player: PlayerId(1),
                target_id: BuildingId(500),
                target: TARGET,
                phase: LiftPhase::Provision,
                started_at: obs.tick,
                phase_started_at: obs.tick,
                deadline: obs.tick.saturating_add(boarding_grace()),
                pickup_component: initial_payload_plan(&obs, HOME, &[])
                    .expect("the surviving roster has a pickup component")
                    .pickup,
                desired_carriers: 2,
                payload: (1..=8).map(UnitId).collect(),
                payload_target: 8,
                ground_payload_target: 8,
                planned_drops: planned_drop_slots(&obs, HOME, TARGET, 2),
                manifests: Vec::new(),
                launched: false,
            }),
            support_latched: false,
            support_released: false,
            assault_waypoints: Vec::new(),
            retry_not_before: 0,
        };

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().expect("the frozen wave keeps waiting");

        assert_eq!(operation.phase, LiftPhase::Provision);
        assert_eq!(operation.desired_carriers, 2);
        assert_eq!(operation.payload, [UnitId(1), UnitId(2), UnitId(3)]);
        assert!(operation.manifests.is_empty());
        assert!(
            decision
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Load { .. } | Intent::Unload { .. }))
        );
    }

    #[test]
    fn provision_deadline_scales_with_outstanding_carrier_training() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 8);
        obs.my_units.push(own(900, UnitKind::Skyhook, HOME));
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();

        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().unwrap();
        let train_ticks = u64::from(UnitKind::Skyhook.stats().train_ticks);
        assert_eq!(operation.desired_carriers, 2);
        assert_eq!(
            operation.deadline,
            (1 + BOARDING_GRACE_TRAINS) * train_ticks
        );

        obs.tick = operation.deadline;
        let timed_out = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(planner.operation().is_none());
        assert!(timed_out.intents.is_empty());
        assert!(timed_out.reservations.is_empty());
    }

    #[test]
    fn remaining_airwork_ticks_keeps_queued_work_in_the_capacity_signal() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 20);
        obs.my_units.push(own(900, UnitKind::Skyhook, HOME));
        let mut occupied = own(901, UnitKind::Skyhook, HOME.offset(1, 0));
        occupied.cargo = 4;
        obs.my_units.push(occupied);
        add_airworks(&mut obs, 10, vec![UnitKind::Skyhook]);
        let mut planner = LiftPlanner::new();

        assert_eq!(planner.remaining_airwork_ticks(&obs, &[]), 0);

        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert_eq!(planner.operation().unwrap().desired_carriers, 4);
        assert_eq!(
            planner.remaining_airwork_ticks(&obs, &[]),
            3 * u64::from(UnitKind::Skyhook.stats().train_ticks),
            "only the empty live carrier reduces demand; queued and occupied carriers remain work"
        );
    }

    #[test]
    fn carrier_reservations_and_capacity_share_one_eligibility_boundary() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 20);
        let mut occupied = own(800, UnitKind::Skyhook, HOME);
        occupied.cargo = 4;
        obs.my_units.extend([
            occupied,
            own(801, UnitKind::Skyhook, HOME.offset(1, 0)),
            own(802, UnitKind::Skyhook, HOME.offset(2, 0)),
            own(803, UnitKind::Skyhook, HOME.offset(3, 0)),
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        add_airworks(&mut obs, 10, Vec::new());
        obs.scrap = 1_000;
        let unavailable = [UnitId(801)];
        let mut planner = LiftPlanner::new();

        let decision = planner.think(&obs, HOME, &unavailable, LiftAirSupport::Independent);

        assert_eq!(planner.operation().unwrap().desired_carriers, 4);
        assert!(!decision.reservations.contains(&UnitId(800)));
        assert!(!decision.reservations.contains(&UnitId(801)));
        assert!(decision.reservations.contains(&UnitId(802)));
        assert!(decision.reservations.contains(&UnitId(803)));
        assert_eq!(
            planner.remaining_airwork_ticks(&obs, &unavailable),
            2 * u64::from(UnitKind::Skyhook.stats().train_ticks)
        );
        assert_eq!(
            decision
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::TrainAt {
                        kind: UnitKind::Skyhook,
                        ..
                    }
                ))
                .count(),
            2
        );
    }

    #[test]
    fn provisioning_never_reserves_a_carrier_owned_by_another_plan() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME),
            own(901, UnitKind::Skyhook, HOME.offset(1, 0)),
        ]);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();

        let decision = planner.think(&obs, HOME, &[UnitId(900)], LiftAirSupport::Independent);

        assert!(!decision.reservations.contains(&UnitId(900)));
        assert!(decision.reservations.contains(&UnitId(901)));
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Provision);
    }

    #[test]
    fn a_wave_does_not_start_without_a_complete_frozen_route_plan() {
        let mut obs = island_obs();
        let only_open_tile = HOME.offset(5, 0);
        obs.known_rock = (0..obs.map_height)
            .flat_map(|y| (0..obs.map_width).map(move |x| TilePos::new(x, y)))
            .filter(|tile| *tile != only_open_tile)
            .collect();
        add_fighters(&mut obs, 20);
        for fighter in &mut obs.my_units {
            fighter.tile = only_open_tile;
        }
        obs.my_units
            .extend((900..904).map(|id| own(id, UnitKind::Skyhook, only_open_tile)));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);

        let mut planner = LiftPlanner::new();
        let waiting = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(planner.operation().is_none());
        assert!(
            waiting
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Load { .. }))
        );
        assert!(waiting.reservations.is_empty());
    }

    #[test]
    fn an_idle_carrier_adjacent_to_its_accepted_goal_is_not_moved_again() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 3);
        obs.my_units
            .push(own(900, UnitKind::Skyhook, HOME.offset(0, 8)));
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let manifest = planner.operation().unwrap().manifests[0].clone();
        let carrier = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap();
        carrier.tile = manifest.pickup.offset(1, 0);
        carrier.idle = true;

        obs.tick += 1;
        let board_once = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(board_once.intents.contains(&Intent::Load {
            transport: manifest.carrier,
            riders: manifest.riders.clone(),
        }));
        assert!(
            board_once
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::MoveUnits { .. }))
        );

        for rider in &mut obs.my_units {
            if manifest.riders.contains(&rider.id) {
                rider.idle = false;
            }
        }
        obs.tick += 1;
        let walking = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(
            walking.intents.is_empty(),
            "Load and Move are both persistent"
        );

        planner.operation.as_mut().unwrap().phase = LiftPhase::Recover;
        obs.tick += 1;
        let recovered = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(planner.operation().is_none());
        assert!(recovered.intents.contains(&Intent::StopUnits {
            units: manifest.riders.clone(),
        }));
        assert!(
            recovered
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::MoveUnits { .. }))
        );
    }

    #[test]
    fn partial_boarding_below_the_payload_floor_recovers_without_launching() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 3);
        obs.my_units
            .push(own(900, UnitKind::Skyhook, HOME.offset(0, 8)));
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let manifest = planner.operation().unwrap().manifests[0].clone();
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap()
            .tile = manifest.pickup;
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        obs.my_units
            .retain(|unit| !manifest.riders.contains(&unit.id));
        let carrier = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap();
        carrier.cargo = 2;
        obs.tick += 1;
        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Recover);
        assert!(!planner.operation().unwrap().launched);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::Unload { at, .. } if *at == manifest.drop
        )));
    }

    #[test]
    fn one_loaded_carrier_cannot_turn_a_four_carrier_wave_into_a_trickle() {
        let (obs, mut planner, manifests) = resolved_four_carrier_wave(1);

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::Recover);
        assert!(!operation.launched);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::Unload { at, .. } if manifests.iter().any(|manifest| manifest.drop == *at)
        )));
    }

    #[test]
    fn half_of_a_four_carrier_wave_is_the_smallest_launch_quorum() {
        let (obs, mut planner, manifests) = resolved_four_carrier_wave(2);

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::Landing);
        assert!(operation.launched);
        let target_unloads: Vec<_> = decision
            .intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::Unload { transport, at }
                    if manifests.iter().any(|manifest| manifest.drop == *at) =>
                {
                    Some(*transport)
                }
                _ => None,
            })
            .collect();
        assert_eq!(target_unloads.len(), 2);
    }

    #[test]
    fn assault_handoff_excludes_a_failed_rider_still_alive_at_home() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 4);
        obs.my_units
            .push(own(900, UnitKind::Skyhook, HOME.offset(0, 8)));
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let manifest = planner.operation().unwrap().manifests[0].clone();
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap()
            .tile = manifest.pickup;
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        let failed = *manifest.riders.last().unwrap();
        let transported: Vec<_> = manifest
            .riders
            .iter()
            .copied()
            .filter(|id| *id != failed)
            .collect();
        obs.my_units.retain(|unit| !transported.contains(&unit.id));
        let carrier = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap();
        carrier.cargo = 3;
        obs.tick += 1;
        let launch = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Landing);
        assert!(launch.intents.contains(&Intent::Unload {
            transport: manifest.carrier,
            at: manifest.drop,
        }));

        let carrier = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap();
        carrier.cargo = 0;
        carrier.tile = manifest.drop;
        for id in &transported {
            obs.my_units
                .push(own(id.0, UnitKind::Sentinel, manifest.drop));
        }
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;
        let handoff = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(handoff.intents.contains(&Intent::AttackMoveUnits {
            units: transported.clone(),
            goal: TARGET,
        }));
        assert!(handoff.intents.iter().all(|intent| match intent {
            Intent::AttackMoveUnits { units, .. } => !units.contains(&failed),
            _ => true,
        }));
        assert!(!handoff.reservations.contains(&failed));
    }

    #[test]
    fn manifests_are_disjoint_and_the_loaded_wave_obeys_one_shared_barrier() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME.offset(0, -2)),
            own(901, UnitKind::Skyhook, HOME.offset(1, -2)),
        ]);
        let original_riders: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind == UnitKind::Sentinel)
            .cloned()
            .collect();

        let mut planner = LiftPlanner::new();
        let initial = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().unwrap().clone();
        assert_eq!(operation.phase, LiftPhase::Boarding);
        assert_eq!(operation.manifests.len(), 2);
        assert_ne!(operation.manifests[0].pickup, operation.manifests[1].pickup);
        assert_ne!(operation.manifests[0].drop, operation.manifests[1].drop);
        let mut assigned: Vec<_> = operation
            .manifests
            .iter()
            .flat_map(|manifest| manifest.riders.iter().copied())
            .collect();
        let assigned_len = assigned.len();
        assigned.sort_unstable();
        assigned.dedup();
        assert_eq!(
            assigned.len(),
            assigned_len,
            "manifests may not share riders"
        );

        for manifest in &operation.manifests {
            let carrier = obs
                .my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap();
            carrier.tile = manifest.pickup;
        }
        obs.tick += 1;
        let boarding = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let loads = initial
            .intents
            .iter()
            .chain(&boarding.intents)
            .filter(|intent| matches!(intent, Intent::Load { .. }))
            .count();
        assert_eq!(loads, 2);

        for rider in &mut obs.my_units {
            if assigned.contains(&rider.id) {
                rider.idle = false;
            }
        }
        obs.tick += 1;
        let walking = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(
            walking
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Load { .. })),
            "one persistent Load must own riders while they walk"
        );

        obs.my_units.retain(|unit| !assigned.contains(&unit.id));
        for manifest in &operation.manifests {
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap()
                .cargo = 4;
        }
        obs.tick += 1;
        let held = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::AwaitSupport);
        assert!(
            held.intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Unload { .. }))
        );

        obs.tick += 1;
        let released = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Released {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Landing);
        assert_eq!(
            released
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::Unload { .. }))
                .count(),
            2
        );

        for manifest in &operation.manifests {
            let carrier = obs
                .my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap();
            carrier.cargo = 0;
            carrier.tile = manifest.drop;
        }
        for mut rider in original_riders {
            if let Some(manifest) = operation
                .manifests
                .iter()
                .find(|manifest| manifest.riders.contains(&rider.id))
            {
                rider.tile = manifest.drop;
                obs.my_units.push(rider);
            }
        }
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;
        let assault = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Recover);
        assert_eq!(
            assault
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::AttackMoveUnits { .. }))
                .count(),
            2
        );
        assert!(assigned.iter().all(|id| assault.reservations.contains(id)));

        for manifest in &operation.manifests {
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap()
                .idle = false;
        }
        obs.tick += 1;
        let no_repeat = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(
            no_repeat
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::AttackMoveUnits { .. }))
        );
        assert!(
            assigned
                .iter()
                .all(|id| no_repeat.reservations.contains(id))
        );

        for manifest in &operation.manifests {
            let carrier = obs
                .my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap();
            carrier.tile = manifest.pickup;
            carrier.idle = true;
        }
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(planner.operation().is_some());
    }

    #[test]
    fn every_manifest_uses_a_distinct_pickup_in_the_riders_ground_component() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 16);
        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME),
            own(901, UnitKind::Skyhook, HOME.offset(1, 0)),
            own(902, UnitKind::Skyhook, HOME.offset(2, 0)),
        ]);
        let isolated = TilePos::new(4, 16);
        obs.known_rock.extend([
            TilePos::new(3, 16),
            TilePos::new(5, 16),
            TilePos::new(4, 17),
        ]);
        obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
        obs.known_rock.dedup();

        let naive = open_slots(&obs, HOME, 3);
        assert_eq!(naive[2], isolated);
        let mut routes = RouteProjection::known_ground(&obs);
        assert!(!routes.reaches(naive[0], naive[2]));

        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().unwrap();

        assert_eq!(operation.desired_carriers, 3);
        assert_eq!(operation.manifests.len(), 3);
        let mut pickups: Vec<_> = operation
            .manifests
            .iter()
            .map(|manifest| manifest.pickup)
            .collect();
        pickups.sort_unstable_by_key(|tile| (tile.y, tile.x));
        pickups.dedup();
        assert_eq!(pickups.len(), 3);
        assert!(!pickups.contains(&isolated));

        let mut routes = RouteProjection::known_ground(&obs);
        for manifest in &operation.manifests {
            for rider in &manifest.riders {
                let rider = unit(&obs, *rider).unwrap();
                assert!(
                    routes.unit_reaches(rider, manifest.pickup),
                    "{} cannot reach {:?}",
                    rider.id,
                    manifest.pickup
                );
            }
        }
    }

    #[test]
    fn a_carrier_lost_during_boarding_aborts_the_wave_instead_of_freezing() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME),
            own(901, UnitKind::Skyhook, HOME.offset(1, 0)),
        ]);
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner.operation().unwrap().clone();
        for manifest in &operation.manifests {
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap()
                .tile = manifest.pickup;
        }
        obs.my_units.retain(|unit| unit.id != UnitId(900));

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_none());
        assert!(planner.retry_not_before > obs.tick);
        assert!(!decision.reservations.contains(&UnitId(900)));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackMoveUnits { .. } | Intent::Load { .. } | Intent::Unload { .. }
        )));
    }

    #[test]
    fn a_carrier_lost_during_partial_boarding_sends_the_loaded_survivor_home() {
        let (mut obs, mut planner, manifests) = resolved_carrier_wave(12, 2, 1);
        let loaded = &manifests[0];
        let lost = &manifests[1];
        assert!(carrier_cargo(&obs, loaded.carrier) >= MIN_EARLY_PAYLOAD as u8);
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Boarding);

        obs.my_units.retain(|unit| unit.id != lost.carrier);
        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        let operation = planner
            .operation()
            .expect("the loaded survivor still needs a recovery unload");
        assert_eq!(operation.phase, LiftPhase::Recover);
        assert!(!operation.launched);
        assert_eq!(operation.manifests.len(), manifests.len());
        assert!(
            operation
                .manifests
                .iter()
                .find(|manifest| manifest.carrier == lost.carrier)
                .is_some_and(|manifest| manifest.closed)
        );
        assert_eq!(
            decision.intents,
            [Intent::Unload {
                transport: loaded.carrier,
                at: loaded.pickup,
            }]
        );
        assert_eq!(decision.reservations, [loaded.carrier]);
        assert!(!decision.reservations.contains(&lost.carrier));
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )));
    }

    #[test]
    fn a_frozen_bulk_wave_launches_surviving_manifests_after_one_carrier_is_lost() {
        let (mut obs, mut planner, manifests) = resolved_carrier_wave(15, 3, 2);
        let surviving = &manifests[..2];
        let lost = &manifests[2];
        for rider in &lost.riders {
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == *rider)
                .unwrap()
                .idle = false;
        }
        obs.my_units.retain(|unit| unit.id != lost.carrier);

        let launch = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        let operation = planner
            .operation()
            .expect("the surviving carriers remain active through landing");
        assert_eq!(operation.phase, LiftPhase::Landing);
        assert!(operation.launched);
        assert!(
            operation
                .manifests
                .iter()
                .find(|manifest| manifest.carrier == lost.carrier)
                .is_some_and(|manifest| manifest.closed)
        );
        assert!(launch.intents.contains(&Intent::StopUnits {
            units: lost.riders.clone(),
        }));
        assert_eq!(
            launch
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::Unload { transport, at }
                        if surviving.iter().any(|manifest| {
                            manifest.carrier == *transport && manifest.drop == *at
                        })
                ))
                .count(),
            2
        );
        assert!(!launch.reservations.contains(&lost.carrier));
        assert!(
            lost.riders
                .iter()
                .all(|rider| !launch.reservations.contains(rider))
        );
        assert!(launch.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )));

        for manifest in surviving {
            {
                let carrier = obs
                    .my_units
                    .iter_mut()
                    .find(|unit| unit.id == manifest.carrier)
                    .unwrap();
                carrier.cargo = 0;
                carrier.tile = manifest.drop;
            }
            for rider in &manifest.riders {
                obs.my_units
                    .push(own(rider.0, UnitKind::Sentinel, manifest.drop));
            }
        }
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;
        let landed = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Recover);
        assert_eq!(
            landed
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::AttackMoveUnits { goal, .. } if *goal == TARGET))
                .count(),
            2
        );
        assert!(landed.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )));

        for manifest in surviving {
            let carrier = obs
                .my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap();
            carrier.tile = manifest.pickup;
            carrier.idle = true;
        }
        obs.tick += 1;
        let recovered = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(planner.operation().is_some());
        assert!(!recovered.reservations.contains(&lost.carrier));
        assert!(surviving.iter().all(|manifest| {
            manifest
                .riders
                .iter()
                .all(|rider| recovered.reservations.contains(rider))
        }));
        assert!(
            lost.riders
                .iter()
                .all(|rider| !recovered.reservations.contains(rider))
        );
        assert!(recovered.intents.iter().all(|intent| !matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )));

        obs.enemy_buildings.clear();
        obs.tick += 1;
        let idle = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(planner.operation().is_none());
        assert!(idle.reservations.is_empty());
        assert!(idle.intents.is_empty());
    }

    #[test]
    fn target_loss_before_launch_returns_loaded_riders_without_attacking_a_ghost() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 3);
        obs.my_units.push(own(900, UnitKind::Skyhook, HOME));
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let manifest = planner.operation().unwrap().manifests[0].clone();
        obs.my_units
            .retain(|unit| !manifest.riders.contains(&unit.id));
        let carrier = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap();
        carrier.cargo = 3;
        carrier.tile = manifest.drop;
        obs.enemy_buildings.clear();

        let returning = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Recover);
        assert!(returning.intents.iter().any(|intent| matches!(
            intent,
            Intent::Unload {
                transport: UnitId(900),
                at,
            } if *at == manifest.pickup
        )));
        assert!(
            returning
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::AttackMoveUnits { .. }))
        );

        let carrier = obs
            .my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap();
        carrier.cargo = 0;
        carrier.tile = manifest.pickup;
        for id in &manifest.riders {
            obs.my_units
                .push(own(id.0, UnitKind::Sentinel, manifest.pickup));
        }
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        let released = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(planner.operation().is_none());
        assert!(
            released
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::AttackMoveUnits { .. }))
        );
    }

    #[test]
    fn a_known_unreachable_empty_carrier_does_not_hold_recovery_open() {
        let mut obs = island_obs();
        obs.known_peaks = (0..obs.map_height).map(|y| TilePos::new(32, y)).collect();
        obs.my_units
            .push(own(900, UnitKind::Skyhook, TARGET.offset(-2, 0)));
        let mut planner = recovering_empty_lift();

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_none());
        assert!(decision.intents.is_empty());
        assert!(decision.reservations.is_empty());
    }

    #[test]
    fn a_refused_empty_carrier_return_is_bounded() {
        let mut obs = island_obs();
        obs.my_units
            .push(own(900, UnitKind::Skyhook, TARGET.offset(-2, 0)));
        let mut planner = recovering_empty_lift();
        let mut moves = 0usize;

        for _ in 0..=usize::from(DROP_ATTEMPTS) {
            let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
            moves += decision
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::MoveUnits { .. }))
                .count();
            obs.tick += 1;
        }

        assert_eq!(moves, usize::from(DROP_ATTEMPTS));
        assert!(planner.operation().is_none());
    }

    #[test]
    fn a_stranded_carrier_tries_distinct_drops_then_bounds_its_recovery() {
        let mut obs = island_obs();
        let mut carrier = own(900, UnitKind::Skyhook, TARGET.offset(-2, -2));
        carrier.cargo = 3;
        obs.my_units.push(carrier);
        let initial_drop = TARGET.offset(-2, -2);
        let mut planner = LiftPlanner {
            operation: Some(LiftOperation {
                target_player: PlayerId(1),
                target_id: BuildingId(500),
                target: TARGET,
                phase: LiftPhase::Landing,
                started_at: 0,
                phase_started_at: 0,
                deadline: boarding_grace(),
                pickup_component: HOME.offset(0, -2),
                desired_carriers: 1,
                payload: UnitIdSet::from_ids(vec![UnitId(1), UnitId(2), UnitId(3)]),
                payload_target: 3,
                ground_payload_target: 3,
                planned_drops: vec![initial_drop],
                manifests: vec![LiftManifest {
                    carrier: UnitId(900),
                    riders: vec![UnitId(1), UnitId(2), UnitId(3)],
                    pickup: HOME.offset(0, -2),
                    drop: initial_drop,
                    attack_issued: false,
                    load_dispatched: true,
                    boarding_closed: true,
                    unload_attempts: 0,
                    recovery_attempts: 0,
                    aborted: false,
                    closed: false,
                }],
                launched: true,
            }),
            support_latched: false,
            support_released: false,
            assault_waypoints: Vec::new(),
            retry_not_before: 0,
        };
        let mut target_attempts = Vec::new();
        let mut all_unloads = 0usize;

        for _ in 0..=usize::from(DROP_ATTEMPTS * 2) {
            let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
            for intent in decision.intents {
                if let Intent::Unload { at, .. } = intent {
                    all_unloads += 1;
                    if target_attempts.len() < usize::from(DROP_ATTEMPTS) {
                        target_attempts.push(at);
                    }
                }
            }
            obs.tick += 1;
        }

        let unique: std::collections::BTreeSet<_> = target_attempts.iter().copied().collect();
        assert_eq!(target_attempts.len(), usize::from(DROP_ATTEMPTS));
        assert_eq!(unique.len(), target_attempts.len());
        assert_eq!(all_unloads, usize::from(DROP_ATTEMPTS * 2));
        assert!(planner.operation().is_none());
    }

    #[test]
    fn a_carrier_destroyed_after_drop_hands_surviving_riders_to_the_assault() {
        let mut obs = island_obs();
        obs.my_units
            .push(own(1, UnitKind::Sentinel, TARGET.offset(-2, -2)));
        let mut planner = LiftPlanner {
            operation: Some(LiftOperation {
                target_player: PlayerId(1),
                target_id: BuildingId(500),
                target: TARGET,
                phase: LiftPhase::Landing,
                started_at: 0,
                phase_started_at: 0,
                deadline: boarding_grace(),
                pickup_component: HOME.offset(0, -2),
                desired_carriers: 1,
                payload: UnitIdSet::from_ids(vec![UnitId(1)]),
                payload_target: 1,
                ground_payload_target: 1,
                planned_drops: vec![TARGET.offset(-2, -2)],
                manifests: vec![LiftManifest {
                    carrier: UnitId(900),
                    riders: vec![UnitId(1)],
                    pickup: HOME.offset(0, -2),
                    drop: TARGET.offset(-2, -2),
                    attack_issued: false,
                    load_dispatched: true,
                    boarding_closed: true,
                    unload_attempts: 1,
                    recovery_attempts: 0,
                    aborted: false,
                    closed: false,
                }],
                launched: true,
            }),
            support_latched: false,
            support_released: false,
            assault_waypoints: Vec::new(),
            retry_not_before: 0,
        };

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_some());
        assert!(decision.intents.contains(&Intent::AttackMoveUnits {
            units: vec![UnitId(1)],
            goal: TARGET,
        }));
        assert!(decision.reservations.contains(&UnitId(1)));
    }

    #[test]
    fn a_carrier_lost_during_recovery_still_hands_off_landed_survivors() {
        let (mut obs, mut planner, manifest) = loaded_single_lift();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(planner.operation().unwrap().launched);

        obs.my_units.retain(|unit| unit.id != manifest.carrier);
        obs.my_units.extend(
            manifest
                .riders
                .iter()
                .map(|id| own(id.0, UnitKind::Sentinel, manifest.drop)),
        );
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        planner.operation.as_mut().unwrap().phase = LiftPhase::Recover;
        obs.tick += 1;

        let recovered = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_some());
        assert!(recovered.intents.contains(&Intent::AttackMoveUnits {
            units: manifest.riders.clone(),
            goal: TARGET,
        }));
        assert!(
            manifest
                .riders
                .iter()
                .all(|id| recovered.reservations.contains(id))
        );
    }

    #[test]
    fn landed_survivors_attack_a_current_replacement_on_their_component() {
        let (mut obs, planner, riders, _) = recovering_landed_assault();
        obs.enemy_buildings.clear();
        let replacement = TilePos::new(44, 12);
        let farther = TilePos::new(46, 22);
        let mut unseen = building(501, 1, BuildingKind::Foundry, TilePos::new(49, 14));
        unseen.seen = false;
        obs.enemy_buildings.extend([
            building(504, 1, BuildingKind::Reclaimer, farther),
            building(502, 1, BuildingKind::Reclaimer, replacement),
            building(503, 1, BuildingKind::Reclaimer, HOME.offset(8, -8)),
            unseen,
        ]);
        let mut reversed_obs = obs.clone();
        reversed_obs.enemy_buildings.reverse();
        let mut first = planner.clone();
        let mut second = planner;

        let first_decision = first.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let second_decision = second.think(&reversed_obs, HOME, &[], LiftAirSupport::Independent);

        assert_eq!(first_decision, second_decision);
        assert_eq!(first, second, "input order cannot choose the replacement");
        assert!(first.operation().is_some());
        assert!(first_decision.intents.contains(&Intent::AttackMoveUnits {
            units: riders.clone(),
            goal: replacement,
        }));
        assert!(
            riders
                .iter()
                .all(|rider| first_decision.reservations.contains(rider))
        );
        assert!(first_decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackMoveUnits { goal, .. }
                if *goal == HOME.offset(8, -8) || *goal == TilePos::new(49, 14)
        )));
    }

    #[test]
    fn targetless_landed_survivors_explore_distinct_fog_honest_waypoints() {
        let (mut obs, mut planner, riders, drop) = recovering_landed_assault();
        obs.enemy_buildings.clear();
        let first_goal = drop.offset(-1, 0);
        let second_goal = drop.offset(0, -2);
        set_explored(&mut obs, first_goal, false);
        set_explored(&mut obs, second_goal, false);

        let first = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_some());
        assert!(first.intents.contains(&Intent::AttackMoveUnits {
            units: riders.clone(),
            goal: first_goal,
        }));
        assert!(
            riders
                .iter()
                .all(|rider| first.reservations.contains(rider))
        );

        for rider in &mut obs.my_units {
            if riders.contains(&rider.id) {
                rider.tile = first_goal;
                rider.idle = true;
            }
        }
        set_explored(&mut obs, first_goal, true);
        obs.tick += 1;
        let second = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(second.intents.contains(&Intent::AttackMoveUnits {
            units: riders.clone(),
            goal: second_goal,
        }));
        assert!(second.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackMoveUnits { goal, .. } if *goal == first_goal
        )));
        assert!(
            riders
                .iter()
                .all(|rider| second.reservations.contains(rider))
        );
    }

    #[test]
    fn moving_landed_survivors_remain_owned_without_replacing_their_order() {
        let (mut obs, mut planner, riders, drop) = recovering_landed_assault();
        obs.enemy_buildings.clear();
        set_explored(&mut obs, drop.offset(-1, 0), false);
        for rider in &mut obs.my_units {
            if riders.contains(&rider.id) {
                rider.idle = false;
            }
        }

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_some());
        assert!(decision.intents.is_empty());
        assert!(
            riders
                .iter()
                .all(|rider| decision.reservations.contains(rider))
        );
    }

    #[test]
    fn a_fully_explored_targetless_shore_releases_landed_survivors() {
        let (mut obs, mut planner, riders, _) = recovering_landed_assault();
        obs.enemy_buildings.clear();
        let mut unseen = building(501, 1, BuildingKind::Foundry, TARGET.offset(-4, -4));
        unseen.seen = false;
        obs.enemy_buildings.extend([
            unseen,
            building(502, 1, BuildingKind::Reclaimer, HOME.offset(8, -8)),
        ]);

        let decision = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert!(planner.operation().is_none());
        assert!(decision.intents.is_empty());
        assert!(
            riders
                .iter()
                .all(|rider| !decision.reservations.contains(rider))
        );
    }

    #[test]
    fn a_latched_suppression_requires_a_matching_release() {
        let (mut obs, mut planner, manifest) = loaded_single_lift();
        let held = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        assert_eq!(planner.operation().unwrap().phase, LiftPhase::AwaitSupport);
        assert!(
            held.intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Unload { .. }))
        );

        obs.tick += 1;
        let missing = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::AwaitSupport);
        assert!(missing.intents.is_empty());

        obs.tick += 1;
        let mismatched = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Released {
                player: PlayerId(1),
                target: TARGET.offset(0, 1),
            },
        );
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::AwaitSupport);
        assert!(mismatched.intents.is_empty());

        obs.tick += 1;
        let released = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Released {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Landing);
        assert!(released.intents.contains(&Intent::Unload {
            transport: manifest.carrier,
            at: manifest.drop,
        }));
    }

    #[test]
    fn missing_or_unrelated_support_cannot_hold_a_latched_small_wave_forever() {
        let unsupported = [
            LiftAirSupport::Independent,
            LiftAirSupport::Suppressing {
                player: PlayerId(2),
                target: TARGET,
            },
        ];

        for support in unsupported {
            let (mut obs, mut planner, manifest) = loaded_single_lift();
            planner.think(
                &obs,
                HOME,
                &[],
                LiftAirSupport::Suppressing {
                    player: PlayerId(1),
                    target: TARGET,
                },
            );
            let waiting_since = planner.operation().unwrap().phase_started_at;
            assert_eq!(planner.operation().unwrap().phase, LiftPhase::AwaitSupport);

            obs.tick = waiting_since.saturating_add(support_grace());
            let timed_out = planner.think(&obs, HOME, &[], support);
            let operation = planner
                .operation()
                .expect("loaded recovery needs a later unload at home");

            assert_eq!(operation.phase, LiftPhase::Recover);
            assert!(!operation.launched);
            assert!(timed_out.intents.iter().all(|intent| !matches!(
                intent,
                Intent::Unload { at, .. } if *at == manifest.drop
            )));
        }
    }

    #[test]
    fn unrelated_air_operations_cannot_hold_or_abort_a_ready_lift() {
        let (obs, planner, manifest) = loaded_single_lift();
        let unrelated = [
            LiftAirSupport::Suppressing {
                player: PlayerId(2),
                target: TARGET,
            },
            LiftAirSupport::Aborted {
                player: PlayerId(2),
                target: TARGET,
            },
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET.offset(0, 1),
            },
            LiftAirSupport::Aborted {
                player: PlayerId(1),
                target: TARGET.offset(0, 1),
            },
        ];

        for support in unrelated {
            let mut independent = planner.clone();
            let decision = independent.think(&obs, HOME, &[], support);
            let operation = independent.operation().unwrap();
            assert_eq!(operation.phase, LiftPhase::Landing);
            assert!(operation.launched);
            assert!(decision.intents.contains(&Intent::Unload {
                transport: manifest.carrier,
                at: manifest.drop,
            }));
        }
    }

    #[test]
    fn a_matching_release_launches_a_ready_wave_immediately() {
        let (obs, mut planner, manifest) = loaded_single_lift();

        let released = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Released {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::Landing);
        assert!(operation.launched);
        assert!(released.intents.contains(&Intent::Unload {
            transport: manifest.carrier,
            at: manifest.drop,
        }));
    }

    #[test]
    fn a_busy_carrier_does_not_receive_repeated_unload_orders() {
        let (mut obs, mut planner, manifest) = loaded_single_lift();
        let launched = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(launched.intents.contains(&Intent::Unload {
            transport: manifest.carrier,
            at: manifest.drop,
        }));

        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap()
            .idle = false;
        obs.tick += 1;
        let in_flight = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Landing);
        assert!(
            in_flight
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Unload { .. }))
        );
    }

    #[test]
    fn a_release_seen_during_provision_survives_until_the_wave_is_ready() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();

        planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Provision);
        assert!(planner.support_latched);

        obs.tick += 1;
        planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Released {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        assert!(planner.support_released);

        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME),
            own(901, UnitKind::Skyhook, HOME.offset(1, 0)),
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let manifests = planner.operation().unwrap().manifests.clone();
        assert_eq!(manifests.len(), 2);

        for manifest in &manifests {
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap()
                .tile = manifest.pickup;
        }
        for rider in &mut obs.my_units {
            if manifests
                .iter()
                .any(|manifest| manifest.riders.contains(&rider.id))
            {
                rider.idle = false;
            }
        }
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        let riders: Vec<_> = manifests
            .iter()
            .flat_map(|manifest| manifest.riders.iter().copied())
            .collect();
        obs.my_units.retain(|unit| !riders.contains(&unit.id));
        for manifest in &manifests {
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap()
                .cargo = 4;
        }
        obs.tick += 1;
        let launch = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Landing);
        assert_eq!(
            launch
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::Unload { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn an_air_abort_does_not_cancel_a_current_independently_viable_bulk_lift() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 16);
        add_airworks(&mut obs, 10, Vec::new());
        obs.scrap = 500;
        let mut planner = LiftPlanner::new();

        planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        obs.tick += 1;
        let independent = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Aborted {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        let operation = planner
            .operation()
            .expect("current sight lets the bulk wave continue without air support");
        assert_eq!(operation.phase, LiftPhase::Provision);
        assert!(operation.desired_carriers >= 3);
        assert!(!planner.support_latched);
        assert!(!planner.support_released);
        assert_eq!(independent.reservations, *operation.payload);
        assert!(independent.intents.iter().any(|intent| matches!(
            intent,
            Intent::TrainAt {
                kind: UnitKind::Skyhook,
                ..
            }
        )));

        let desired_carriers = u32::try_from(operation.desired_carriers).unwrap();
        obs.my_units.extend((900..900 + desired_carriers).map(|id| {
            own(
                id,
                UnitKind::Skyhook,
                HOME.offset(i32::try_from(id - 900).unwrap(), 8),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let manifests = planner
            .operation()
            .expect("the independent operation assigns the completed carriers")
            .manifests
            .clone();
        assert_eq!(manifests.len(), usize::try_from(desired_carriers).unwrap());

        for manifest in &manifests {
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap()
                .tile = manifest.pickup;
        }
        obs.tick += 1;
        let boarding = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(
            boarding
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::Load { .. }))
                .count(),
            manifests.len()
        );

        for manifest in &manifests {
            let cargo = manifest
                .riders
                .iter()
                .filter_map(|id| unit(&obs, *id))
                .map(|rider| rider.kind.stats().transport_size)
                .sum();
            obs.my_units
                .retain(|unit| !manifest.riders.contains(&unit.id));
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap()
                .cargo = cargo;
        }
        obs.tick += 1;
        let launched = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let operation = planner
            .operation()
            .expect("the independently viable wave remains active in flight");
        assert_eq!(operation.phase, LiftPhase::Landing);
        assert!(operation.launched);
        assert_eq!(
            launched
                .intents
                .iter()
                .filter(|intent| matches!(intent, Intent::Unload { .. }))
                .count(),
            manifests.len()
        );
    }

    #[test]
    fn remembered_support_waits_for_current_sight_before_admitting_a_lift() {
        let mut obs = island_obs();
        obs.enemy_buildings[0].seen = false;
        add_fighters(&mut obs, 16);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();

        let remembered = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        assert!(planner.operation().is_none());
        assert_eq!(remembered, StrategicDecision::default());

        obs.enemy_buildings[0].seen = true;
        obs.tick = super::super::difficulty::next_strategic_admission_tick(obs.tick);
        let admitted = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        assert!(planner.support_latched);
        assert!(planner.operation().unwrap().desired_carriers >= 3);
        assert!(!admitted.reservations.is_empty());
    }

    #[test]
    fn a_matching_abort_releases_a_provisioning_wave_after_current_sight_is_lost() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 16);
        add_airworks(&mut obs, 10, Vec::new());
        let mut planner = LiftPlanner::new();

        let preparing = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        let operation = planner
            .operation()
            .expect("current support admits the disconnected lift");
        assert_eq!(operation.phase, LiftPhase::Provision);
        assert!(planner.support_latched);
        assert!(!preparing.reservations.is_empty());

        obs.tick += 1;
        obs.enemy_buildings[0].seen = false;
        let aborted = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Aborted {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        assert!(planner.operation().is_none());
        assert!(!planner.support_latched);
        assert!(!planner.support_released);
        assert!(planner.retry_not_before > obs.tick);
        assert_eq!(aborted, StrategicDecision::default());
    }

    #[test]
    fn a_matching_support_abort_recovers_a_small_wave_without_launching() {
        let (mut obs, mut planner, manifest) = loaded_single_lift();
        planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        obs.tick += 1;
        let aborted = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Aborted {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::Recover);
        assert!(!operation.launched);
        assert!(aborted.intents.iter().all(|intent| !matches!(
            intent,
            Intent::Unload { at, .. } if *at == manifest.drop
        )));

        obs.tick += 1;
        let recovering = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert!(recovering.intents.contains(&Intent::Unload {
            transport: manifest.carrier,
            at: manifest.pickup,
        }));
    }

    #[test]
    fn a_matching_support_abort_recovers_when_only_two_of_three_carriers_loaded() {
        let (obs, mut planner, manifests) = resolved_carrier_wave(15, 3, 2);

        let decision = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Aborted {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        let operation = planner.operation().unwrap();
        assert_eq!(operation.desired_carriers, 3);
        assert_eq!(operation.phase, LiftPhase::Recover);
        assert!(!operation.launched);
        assert!(decision.intents.iter().all(|intent| !matches!(
            intent,
            Intent::Unload { at, .. }
                if manifests.iter().any(|manifest| manifest.drop == *at)
        )));
    }

    #[test]
    fn a_matching_support_abort_launches_a_prepared_bulk_wave_independently() {
        let (obs, mut planner, manifests) = resolved_carrier_wave(15, 3, 3);

        let decision = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Aborted {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        let operation = planner.operation().unwrap();
        assert_eq!(operation.desired_carriers, 3);
        assert_eq!(operation.phase, LiftPhase::Landing);
        assert!(operation.launched);
        assert_eq!(
            decision
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::Unload { at, .. }
                        if manifests.iter().any(|manifest| manifest.drop == *at)
                ))
                .count(),
            3
        );
    }

    #[test]
    fn a_matching_support_abort_still_recovers_a_two_carrier_wave() {
        let (mut obs, mut planner, manifests) = resolved_carrier_wave(12, 2, 2);

        let aborted = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Aborted {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        let operation = planner.operation().unwrap();
        assert_eq!(operation.desired_carriers, 2);
        assert_eq!(operation.phase, LiftPhase::Recover);
        assert!(!operation.launched);
        assert!(
            aborted
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Unload { .. }))
        );

        obs.tick += 1;
        let recovering = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(
            recovering
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::Unload { at, .. }
                        if manifests.iter().any(|manifest| manifest.pickup == *at)
                ))
                .count(),
            2
        );
        assert!(recovering.intents.iter().all(|intent| !matches!(
            intent,
            Intent::Unload { at, .. }
                if manifests.iter().any(|manifest| manifest.drop == *at)
        )));
    }

    #[test]
    fn support_timeout_launches_a_fully_boarded_two_carrier_wave_at_the_deadline() {
        let (mut obs, mut planner, manifests) = resolved_carrier_wave(12, 2, 2);
        planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        let waiting_since = planner.operation().unwrap().phase_started_at;
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::AwaitSupport);

        obs.tick = waiting_since
            .saturating_add(support_grace())
            .saturating_sub(1);
        let before_deadline = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::AwaitSupport);
        assert_eq!(operation.phase_started_at, waiting_since);
        assert!(
            before_deadline
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Unload { .. }))
        );

        obs.tick = waiting_since.saturating_add(support_grace());
        let launched = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::Landing);
        assert!(operation.launched);
        assert_eq!(
            launched
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::Unload { at, .. }
                        if manifests.iter().any(|manifest| manifest.drop == *at)
                ))
                .count(),
            2
        );
        assert!(launched.intents.iter().all(|intent| !matches!(
            intent, Intent::Unload { at, .. }
                if manifests.iter().any(|manifest| manifest.pickup == *at)
        )));
    }

    #[test]
    fn support_timeout_recovers_when_only_one_of_two_carriers_remains_loaded() {
        let (mut obs, mut planner, manifests) = resolved_carrier_wave(12, 2, 2);
        planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        let waiting_since = planner.operation().unwrap().phase_started_at;
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::AwaitSupport);

        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifests[1].carrier)
            .unwrap()
            .cargo = 0;
        obs.tick = waiting_since.saturating_add(support_grace());
        let timed_out = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );

        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::Recover);
        assert!(!operation.launched);
        assert!(
            timed_out
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Unload { .. }))
        );

        obs.tick += 1;
        let recovering = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(
            recovering
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::Unload { transport, at }
                        if *transport == manifests[0].carrier && *at == manifests[0].pickup
                ))
                .count(),
            1
        );
        assert!(recovering.intents.iter().all(|intent| !matches!(
            intent,
            Intent::Unload { at, .. }
                if manifests.iter().any(|manifest| manifest.drop == *at)
        )));
    }

    #[test]
    fn continuous_support_hold_releases_a_prepared_bulk_wave_at_the_original_deadline() {
        let (mut obs, mut planner, manifests) = resolved_carrier_wave(15, 3, 3);
        planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        let waiting_since = planner.operation().unwrap().phase_started_at;
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::AwaitSupport);

        obs.tick = waiting_since
            .saturating_add(support_grace())
            .saturating_sub(1);
        planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::AwaitSupport);
        assert_eq!(operation.phase_started_at, waiting_since);

        obs.tick = waiting_since.saturating_add(support_grace());
        let released = planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::Landing);
        assert!(operation.launched);
        assert_eq!(
            released
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::Unload { at, .. }
                        if manifests.iter().any(|manifest| manifest.drop == *at)
                ))
                .count(),
            3
        );
    }

    #[test]
    fn a_bulk_wave_that_never_latched_still_launches_independently() {
        let (obs, mut planner, manifests) = resolved_carrier_wave(15, 3, 3);
        assert!(!planner.support_latched);

        let launched = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);

        let operation = planner.operation().unwrap();
        assert_eq!(operation.phase, LiftPhase::Landing);
        assert!(operation.launched);
        assert_eq!(
            launched
                .intents
                .iter()
                .filter(|intent| matches!(
                    intent,
                    Intent::Unload { at, .. }
                        if manifests.iter().any(|manifest| manifest.drop == *at)
                ))
                .count(),
            3
        );
    }

    #[test]
    fn suppression_seen_during_boarding_cannot_disappear_into_an_independent_launch() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 3);
        obs.my_units
            .push(own(900, UnitKind::Skyhook, HOME.offset(0, 8)));
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let manifest = planner.operation().unwrap().manifests[0].clone();
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap()
            .tile = manifest.pickup;
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        for rider in &mut obs.my_units {
            if manifest.riders.contains(&rider.id) {
                rider.idle = false;
            }
        }

        obs.tick += 1;
        planner.think(
            &obs,
            HOME,
            &[],
            LiftAirSupport::Suppressing {
                player: PlayerId(1),
                target: TARGET,
            },
        );
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::Boarding);

        obs.my_units
            .retain(|unit| !manifest.riders.contains(&unit.id));
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap()
            .cargo = 3;
        obs.tick += 1;
        let missing = planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        assert_eq!(planner.operation().unwrap().phase, LiftPhase::AwaitSupport);
        assert!(
            missing
                .intents
                .iter()
                .all(|intent| !matches!(intent, Intent::Unload { .. }))
        );
    }

    #[test]
    fn unavailable_fighters_do_not_inflate_or_enter_the_wave() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 20);
        let unavailable: Vec<_> = (1..=12).map(UnitId).collect();

        assert_eq!(desired_carriers(&obs, HOME, &unavailable), 2);
    }

    #[test]
    fn equal_inputs_produce_identical_decisions_and_persistent_state() {
        let mut obs = island_obs();
        add_fighters(&mut obs, 12);
        obs.my_units.extend([
            own(900, UnitKind::Skyhook, HOME),
            own(901, UnitKind::Skyhook, HOME.offset(1, 0)),
        ]);
        let mut first = LiftPlanner::new();
        let mut second = LiftPlanner::new();

        assert_eq!(
            first.think(&obs, HOME, &[], LiftAirSupport::Independent),
            second.think(&obs, HOME, &[], LiftAirSupport::Independent)
        );
        assert_eq!(first, second);
    }

    fn island_obs() -> Observation {
        let mut obs = Observation {
            version: OBSERVATION_VERSION,
            tick: 0,
            me: PlayerId(0),
            scrap: 0,
            map_width: 64,
            map_height: 32,
            my_units: Vec::new(),
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: vec![building(500, 1, BuildingKind::Foundry, TARGET)],
            visible: vec![true; 64 * 32],
            explored: vec![true; 64 * 32],
            known_scrap: Vec::new(),
            known_rock: (0..32).map(|y| TilePos::new(32, y)).collect(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
        };
        obs.my_buildings
            .push(building(1, 0, BuildingKind::Foundry, HOME.offset(-1, -1)));
        obs.my_queues.push(Vec::new());
        obs
    }

    fn add_fighters(obs: &mut Observation, count: u32) {
        obs.my_units.extend((1..=count).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(8 + (id % 12) as i32, 8 + ((id / 12) % 12) as i32),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
    }

    fn split_staging_obs(pocket_count: u32, strong_count: u32) -> Observation {
        let mut obs = island_obs();
        obs.known_rock
            .extend((0..obs.map_height).map(|y| TilePos::new(HOME.x, y)));
        obs.known_rock.sort_unstable_by_key(|tile| (tile.y, tile.x));
        obs.known_rock.dedup();
        obs.my_units.extend((1..=pocket_count).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(
                    8 + i32::try_from(id % 3).unwrap(),
                    8 + i32::try_from(id % 12).unwrap(),
                ),
            )
        }));
        obs.my_units.extend((100..100 + strong_count).map(|id| {
            own(
                id,
                UnitKind::Sentinel,
                TilePos::new(
                    1 + i32::try_from(id % 3).unwrap(),
                    8 + i32::try_from(id % 12).unwrap(),
                ),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        obs
    }

    fn add_flakhounds(obs: &mut Observation, first_id: u32, count: u32) {
        obs.my_units.extend((first_id..first_id + count).map(|id| {
            own(
                id,
                UnitKind::Flakhound,
                TilePos::new(8 + (id % 12) as i32, 20 + ((id / 12) % 8) as i32),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
    }

    fn add_airworks(obs: &mut Observation, id: u32, queue: Vec<UnitKind>) {
        obs.my_buildings.push(building(
            id,
            0,
            BuildingKind::Airworks,
            TilePos::new(10 + id as i32 % 4, 4),
        ));
        obs.my_queues.push(queue);
    }

    fn loaded_single_lift() -> (Observation, LiftPlanner, LiftManifest) {
        let mut obs = island_obs();
        add_fighters(&mut obs, 3);
        obs.my_units
            .push(own(900, UnitKind::Skyhook, HOME.offset(0, 8)));
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let manifest = planner.operation().unwrap().manifests[0].clone();
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap()
            .tile = manifest.pickup;
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        obs.my_units
            .retain(|unit| !manifest.riders.contains(&unit.id));
        obs.my_units
            .iter_mut()
            .find(|unit| unit.id == manifest.carrier)
            .unwrap()
            .cargo = 3;
        obs.tick += 1;
        (obs, planner, manifest)
    }

    fn recovering_empty_lift() -> LiftPlanner {
        LiftPlanner {
            operation: Some(LiftOperation {
                target_player: PlayerId(1),
                target_id: BuildingId(500),
                target: TARGET,
                phase: LiftPhase::Recover,
                started_at: 0,
                phase_started_at: 0,
                deadline: boarding_grace(),
                pickup_component: HOME.offset(0, -2),
                desired_carriers: 1,
                payload: UnitIdSet::default(),
                payload_target: 0,
                ground_payload_target: 0,
                planned_drops: vec![TARGET.offset(-2, 0)],
                manifests: vec![LiftManifest {
                    carrier: UnitId(900),
                    riders: Vec::new(),
                    pickup: HOME.offset(0, -2),
                    drop: TARGET.offset(-2, 0),
                    attack_issued: false,
                    load_dispatched: false,
                    boarding_closed: true,
                    unload_attempts: 0,
                    recovery_attempts: 0,
                    aborted: true,
                    closed: false,
                }],
                launched: true,
            }),
            support_latched: false,
            support_released: false,
            assault_waypoints: Vec::new(),
            retry_not_before: 0,
        }
    }

    fn recovering_landed_assault() -> (Observation, LiftPlanner, Vec<UnitId>, TilePos) {
        let mut obs = island_obs();
        let pickup = HOME.offset(0, -2);
        let drop = TARGET.offset(-2, -2);
        let riders: Vec<_> = (1..=3).map(UnitId).collect();
        obs.my_units.push(own(900, UnitKind::Skyhook, pickup));
        obs.my_units
            .extend(riders.iter().map(|id| own(id.0, UnitKind::Sentinel, drop)));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        let planner = LiftPlanner {
            operation: Some(LiftOperation {
                target_player: PlayerId(1),
                target_id: BuildingId(500),
                target: TARGET,
                phase: LiftPhase::Recover,
                started_at: 0,
                phase_started_at: 0,
                deadline: boarding_grace(),
                pickup_component: pickup,
                desired_carriers: 1,
                payload: UnitIdSet::from_ids(riders.clone()),
                payload_target: 3,
                ground_payload_target: 3,
                planned_drops: vec![drop],
                manifests: vec![LiftManifest {
                    carrier: UnitId(900),
                    riders: riders.clone(),
                    pickup,
                    drop,
                    attack_issued: true,
                    load_dispatched: true,
                    boarding_closed: true,
                    unload_attempts: 1,
                    recovery_attempts: 0,
                    aborted: false,
                    closed: false,
                }],
                launched: true,
            }),
            support_latched: false,
            support_released: false,
            assault_waypoints: Vec::new(),
            retry_not_before: 0,
        };
        (obs, planner, riders, drop)
    }

    fn set_explored(obs: &mut Observation, tile: TilePos, explored: bool) {
        let width = usize::try_from(obs.map_width).unwrap();
        let index = usize::try_from(tile.y).unwrap() * width + usize::try_from(tile.x).unwrap();
        obs.explored[index] = explored;
        obs.visible[index] = explored;
    }

    fn resolved_four_carrier_wave(
        loaded_manifests: usize,
    ) -> (Observation, LiftPlanner, Vec<LiftManifest>) {
        resolved_carrier_wave(20, 4, loaded_manifests)
    }

    fn resolved_carrier_wave(
        fighter_count: u32,
        carrier_count: u32,
        loaded_manifests: usize,
    ) -> (Observation, LiftPlanner, Vec<LiftManifest>) {
        let mut obs = island_obs();
        add_fighters(&mut obs, fighter_count);
        obs.my_units.extend((900..900 + carrier_count).map(|id| {
            own(
                id,
                UnitKind::Skyhook,
                HOME.offset(i32::try_from(id - 900).unwrap(), 8),
            )
        }));
        obs.my_units.sort_unstable_by_key(|unit| unit.id);
        let mut planner = LiftPlanner::new();
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        let manifests = planner.operation().unwrap().manifests.clone();
        assert_eq!(manifests.len(), usize::try_from(carrier_count).unwrap());

        for manifest in &manifests {
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap()
                .tile = manifest.pickup;
        }
        obs.tick += 1;
        planner.think(&obs, HOME, &[], LiftAirSupport::Independent);
        for manifest in manifests.iter().take(loaded_manifests) {
            let cargo = manifest
                .riders
                .iter()
                .filter_map(|id| unit(&obs, *id))
                .map(|rider| rider.kind.stats().transport_size)
                .sum();
            obs.my_units
                .retain(|unit| !manifest.riders.contains(&unit.id));
            obs.my_units
                .iter_mut()
                .find(|unit| unit.id == manifest.carrier)
                .unwrap()
                .cargo = cargo;
        }
        for manifest in &mut planner.operation.as_mut().unwrap().manifests {
            manifest.boarding_closed = true;
        }
        obs.tick += 1;
        (obs, planner, manifests)
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

    fn building(id: u32, player: u8, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(player),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn remembered_target(last_seen: Tick) -> BuildingContact {
        BuildingContact {
            id: Some(BuildingId(500)),
            player: PlayerId(1),
            kind: BuildingKind::Foundry,
            anchor: TARGET,
            hp: BuildingKind::Foundry.base_stats().max_hp,
            built: true,
            tier: 0,
            last_seen: Some(last_seen),
            evidence: ContactEvidence::Remembered,
        }
    }
}
