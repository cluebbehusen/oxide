//! One persistent, fog-honest strategic playbook.
//!
//! This is deliberately not a generic planner. It coordinates reconnaissance,
//! suppression, and an exact strike wing, then brings survivors home. Normal
//! operations use ground artillery; mature island stalemates can instead mass
//! air attackers against visible flak. Persistent membership prevents ordinary
//! drafting from turning either operation into a trickle attack.

use super::difficulty::{DifficultyTuning, strategic_admission_tick};
use super::executive::Intent;
use super::intelligence::{
    AirDefenseAssessment, AirDefenseEvidence, AirDefenseSource, BuildingContact, ContactEvidence,
    StrategicIntelligence,
};
use super::observation::{Observation, UnitObs};
use super::profile::{ResolvedProfile, Specialty};
use super::routing::{self, RouteProjection};
use crate::ids::{BuildingId, PlayerId, Target, UnitId};
use crate::scenario::BotStance;
use crate::stats::{BuildingKind, QUEUE_CAP, Role, UnitKind};
use chassis::Tick;
use chassis::grid::TilePos;
use core::cmp::Reverse;

const STANDARD_BOMBERS: usize = 2;
/// A connected-map combined-arms operation is an expensive second front, not
/// an opening build order. Keep a real fighting roster online before reserving
/// scouts, artillery, and bombers so a seeded specialty cannot hollow out the
/// ordinary line that protects the economy.
const CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER: usize = 12;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct AirPlan {
    suppression: AirSuppression,
    desired_artillery: usize,
    desired_bombers: usize,
    desired_screen: usize,
    screen: Vec<UnitId>,
    assembly_timeout: Tick,
    flak_dispatch: Option<(BuildingId, Vec<UnitId>)>,
    strike_dispatch: Option<AirStrikeDispatch>,
    observed_renewable: usize,
    observed_fighters: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AirStrikeDispatch {
    Attack(BuildingId),
    AttackMove(TilePos),
}

impl AirPlan {
    fn combined(profile: &ResolvedProfile, obs: &Observation) -> Self {
        let siege_leading = siege_leading(profile);
        let artillery = preferred_artillery(profile, obs);
        let desired_artillery = if siege_leading {
            if artillery == UnitKind::Avalanche {
                1
            } else {
                2
            }
        } else {
            1
        };
        let desired_bombers = if siege_leading { 1 } else { STANDARD_BOMBERS };
        let plan = Self {
            suppression: AirSuppression::GroundArtillery,
            desired_artillery,
            desired_bombers,
            desired_screen: 0,
            screen: Vec::new(),
            assembly_timeout: 3_200,
            flak_dispatch: None,
            strike_dispatch: None,
            observed_renewable: 0,
            observed_fighters: 0,
        };
        debug_assert!(plan.desired_artillery + plan.desired_bombers <= 3);
        debug_assert!(combined_combat_cost(&plan, profile, obs) <= combined_combat_ceiling(obs));
        plan
    }

    fn island(profile: &ResolvedProfile, obs: &Observation) -> Self {
        let airworks = completed(obs, BuildingKind::Airworks);
        let renewable = completed(obs, BuildingKind::Extractor)
            .saturating_add(completed(obs, BuildingKind::Reclaimer));
        let fighters = combat_roster(obs);
        let stance_scale = match profile.stance {
            BotStance::Turtle => 0,
            BotStance::Balanced => 1,
            BotStance::Aggressive => 2,
        };
        let desired_bombers = 4usize
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
            .map(|(index, _)| {
                obs.my_queues
                    .get(index)
                    .into_iter()
                    .flatten()
                    .map(|kind| u64::from(kind.stats().train_ticks))
                    .sum::<Tick>()
            })
            .max()
            .unwrap_or(0);
        let requested_training = u64::try_from(desired_bombers)
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
            suppression: AirSuppression::Airborne,
            desired_artillery: 0,
            desired_bombers,
            desired_screen,
            screen: Vec::new(),
            assembly_timeout,
            flak_dispatch: None,
            strike_dispatch: None,
            observed_renewable: renewable,
            observed_fighters: fighters,
        }
    }

    fn airborne(&self) -> bool {
        self.suppression == AirSuppression::Airborne
    }
}

/// A phase of the coordinated air playbook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AirOperationPhase {
    /// Put current sight over the objective.
    Recon,
    /// Recruit or train the exact operation group.
    Assemble,
    /// Let the operation's suppression force remove currently targetable flak.
    SuppressAa,
    /// Re-observe the objective and final approach.
    Verify,
    /// Commit the bomber wing.
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
    /// Last hold (a landing near home) dispatched to the exact bomber wing.
    pub bomber_hold: Option<TilePos>,
    /// Last staging move dispatched to the artillery group. An explicit
    /// artillery attack clears this marker because the staging order no longer
    /// owns the group.
    pub artillery_staging: Option<TilePos>,
    /// Exact assigned Bombard or Avalanche ids, sorted.
    pub artillery: Vec<UnitId>,
    /// Exact assigned faction-bomber ids, sorted.
    pub bombers: Vec<UnitId>,
    /// First issued strike tick.
    pub strike_issued_at: Option<Tick>,
    /// Set only while recovering.
    pub recovery_reason: Option<AirRecoveryReason>,
}

/// Role-preserving survivors held only through the operation cooldown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AirStandby {
    scout: Option<UnitId>,
    artillery: Vec<UnitId>,
    bombers: Vec<UnitId>,
}

impl AirStandby {
    fn from_operation(op: &AirOperation, obs: &Observation) -> Self {
        let mut standby = Self {
            scout: op.scout,
            artillery: op.artillery.clone(),
            bombers: op.bombers.clone(),
        };
        standby.prune(obs);
        standby
    }

    fn prune(&mut self, obs: &Observation) {
        let scout_kind = Role::Scout.unit_for(obs.faction);
        let bomber_kind = Role::Bomber.unit_for(obs.faction);
        self.scout = self
            .scout
            .filter(|id| unit(obs, *id).is_some_and(|member| member.kind == scout_kind));
        self.artillery
            .retain(|id| unit(obs, *id).is_some_and(|member| is_artillery(member.kind)));
        self.bombers
            .retain(|id| unit(obs, *id).is_some_and(|member| member.kind == bomber_kind));
    }

    fn reservations(&self) -> Vec<UnitId> {
        let mut ids: Vec<_> = self
            .scout
            .into_iter()
            .chain(self.artillery.iter().copied())
            .chain(self.bombers.iter().copied())
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
    enlisted: &'a [UnitId],
    landing_sites: &'a [TilePos],
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

    #[cfg(test)]
    fn air_plan(&self) -> Option<&AirPlan> {
        self.air.as_ref().map(|active| &active.plan)
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
        let live_bombers = op
            .bombers
            .iter()
            .filter(|id| unit(obs, **id).is_some_and(|member| member.kind == bomber_kind))
            .count();
        let missing_scout = 1usize.saturating_sub(live_scout);
        if !op.assault_admitted {
            return training_ticks(missing_scout, scout_kind);
        }
        let missing_screen = plan.desired_screen.saturating_sub(live_screen);
        let missing_bombers = plan.desired_bombers.saturating_sub(live_bombers);
        training_ticks(missing_scout, scout_kind)
            .saturating_add(training_ticks(missing_screen, screen_kind))
            .saturating_add(training_ticks(missing_bombers, bomber_kind))
    }

    /// Advances the air playbook using only the supplied oriented knowledge.
    pub fn think(
        &mut self,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &Observation,
        intel: &StrategicIntelligence,
        home: TilePos,
        enlisted: &[UnitId],
    ) -> StrategicDecision {
        self.think_with_lift_support(
            profile,
            tuning,
            obs,
            intel,
            home,
            StrategicCoordination {
                enlisted,
                lift_support: None,
                allow_new_operation: true,
            },
        )
    }

    pub(super) fn think_with_lift_support(
        &mut self,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        obs: &Observation,
        intel: &StrategicIntelligence,
        home: TilePos,
        coordination: StrategicCoordination<'_>,
    ) -> StrategicDecision {
        let StrategicCoordination {
            enlisted,
            lift_support,
            allow_new_operation,
        } = coordination;
        self.terminal_outcome = None;
        self.standby.prune(obs);
        if intel.observed_at() != Some(obs.tick) {
            return StrategicDecision {
                reservations: self.standby.reservations(),
                ..StrategicDecision::default()
            };
        }
        if self.air.is_none() {
            if !allow_new_operation {
                return StrategicDecision {
                    reservations: self.standby.reservations(),
                    ..StrategicDecision::default()
                };
            }
            if obs.tick < self.cooldown_until {
                return StrategicDecision {
                    reservations: self.standby.reservations(),
                    ..StrategicDecision::default()
                };
            }
            let island_target = if let Some(request) = lift_support {
                exact_wealthy_island_target(profile, obs, home, intel, request)
            } else {
                select_wealthy_island_target(profile, obs, home, intel)
            };
            let target = if lift_support.is_some() {
                island_target
            } else {
                island_target.or_else(|| select_target(intel, obs.tick, tuning.tactical_memory))
            };
            let Some(target) = target else {
                self.standby = AirStandby::default();
                return StrategicDecision::default();
            };
            let plan = if island_target.is_some() {
                Some(AirPlan::island(profile, obs))
            } else if eligible(profile)
                && ready_to_prepare(profile, obs)
                && combat_roster(obs) >= CONNECTED_OPERATION_MINIMUM_COMBAT_ROSTER
            {
                Some(AirPlan::combined(profile, obs))
            } else {
                None
            };
            let Some(plan) = plan else {
                self.standby = AirStandby::default();
                return StrategicDecision::default();
            };
            if !strategic_admission_tick(obs.tick) {
                return StrategicDecision {
                    reservations: self.standby.reservations(),
                    ..StrategicDecision::default()
                };
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
                bomber_hold: None,
                artillery_staging: None,
                artillery: if assault_admitted {
                    standby.artillery
                } else {
                    Vec::new()
                },
                bombers: if assault_admitted {
                    standby.bombers
                } else {
                    Vec::new()
                },
                strike_issued_at: None,
                recovery_reason: None,
            };
            self.air = Some(ActiveAirOperation { op, plan });
        }
        let Some(ActiveAirOperation { mut op, mut plan }) = self.air.take() else {
            return StrategicDecision::default();
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
            op.assault_admitted = true;
            op.started_at = obs.tick;
            op.phase_started_at = obs.tick;
            plan = if wealthy_island_target(profile, obs, home, current_target) {
                AirPlan::island(profile, obs)
            } else {
                AirPlan::combined(profile, obs)
            };
        }
        if !began_in_recovery {
            abort_if_needed(&mut op, &plan, profile, obs, intel);
        }

        let mut out = StrategicDecision::default();
        let landing_sites: Vec<_> = lift_support
            .filter(|request| request.player == op.target_player && request.target == op.target)
            .map_or_else(Vec::new, |request| request.planned_drops.clone());
        let context = AirPlanningContext {
            profile,
            tuning,
            obs,
            intel,
            home,
            enlisted,
            landing_sites: &landing_sites,
        };
        match op.phase {
            AirOperationPhase::Recon if !op.assault_admitted => {
                remembered_recon(&mut op, &context, &mut out)
            }
            AirOperationPhase::Recon => recon(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::Assemble => assemble(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::SuppressAa => suppress(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::Verify => verify(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::Strike => strike(&mut op, &mut plan, &context, &mut out),
            AirOperationPhase::Recover => {}
        }
        if !allow_new_operation {
            out.intents
                .retain(|intent| !matches!(intent, Intent::TrainAt { .. }));
            out.committed_scrap = 0;
        }
        if op.phase == AirOperationPhase::Recover {
            if !began_in_recovery {
                self.cooldown_until = obs.tick.saturating_add(cooldown(profile, tuning));
            }
            let survivors = reservations(&op, &plan, obs);
            let returning = routing::routable_command_subset(obs, &survivors, home);
            release_unroutable(&mut op, &mut plan, &survivors, &returning);
            // The transition into Recover owns the one return order. Move is
            // persistent; replacing it every think is redundant and can keep
            // turn-limited aircraft from settling on their accepted goal.
            if !began_in_recovery && !returning.is_empty() {
                out.intents.push(Intent::MoveUnits {
                    units: returning,
                    goal: home,
                });
            }
        }
        out.reservations = reservations(&op, &plan, obs);
        // Lowering fans a mixed-domain group over distinct snapped goals, and
        // bounded-turn aircraft may stop within their movement acceptance
        // radius. Once a previously dispatched return has become terminal for
        // every survivor, `idle` is the simulation's authoritative completion
        // signal; comparing every tile to the original shared goal invents a
        // stricter geometry and holds the operation forever.
        let settled = began_in_recovery
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
        out
    }
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
    if !dispatch_scout(op, obs, context.intel, context.landing_sites, out) {
        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        return;
    }
    schedule(obs, &[(scout_kind, usize::from(op.scout.is_none()))], out);
}

fn recon(
    op: &mut AirOperation,
    plan: &mut AirPlan,
    context: &AirPlanningContext<'_>,
    out: &mut StrategicDecision,
) {
    let AirPlanningContext {
        profile,
        tuning,
        obs,
        intel,
        enlisted,
        landing_sites,
        ..
    } = context;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let bomber_kind = Role::Bomber.unit_for(obs.faction);
    let previous_scout = op.scout;
    op.scout = op
        .scout
        .filter(|id| unit(obs, *id).is_some())
        .or_else(|| available(obs, enlisted, |k| k == scout_kind).next());
    if op.scout != previous_scout {
        op.scout_dispatch = None;
        if op.scout.is_some() {
            op.phase_started_at = obs.tick;
        }
    }
    assign_exact(
        &mut op.artillery,
        plan.desired_artillery,
        obs,
        enlisted,
        is_artillery,
    );
    assign_exact(
        &mut op.bombers,
        plan.desired_bombers,
        obs,
        enlisted,
        |kind| kind == bomber_kind,
    );
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    assign_exact(
        &mut plan.screen,
        plan.desired_screen,
        obs,
        enlisted,
        |kind| kind == screen_kind,
    );
    if !dispatch_scout(op, obs, intel, landing_sites, out) {
        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        return;
    }
    schedule_missing_members(op, plan, profile, obs, scout_kind, bomber_kind, out);
    if op.scout_dispatch.is_some()
        && target_seen(op, obs)
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
        profile,
        obs,
        intel,
        home,
        enlisted,
        landing_sites,
        ..
    } = context;
    let scout_kind = Role::Scout.unit_for(obs.faction);
    let bomber_kind = Role::Bomber.unit_for(obs.faction);
    let previous_scout = op.scout;
    op.scout = op
        .scout
        .filter(|id| unit(obs, *id).is_some())
        .or_else(|| available(obs, enlisted, |k| k == scout_kind).next());
    if op.scout != previous_scout {
        op.scout_dispatch = None;
    }
    assign_exact(
        &mut op.artillery,
        plan.desired_artillery,
        obs,
        enlisted,
        is_artillery,
    );
    assign_exact(
        &mut op.bombers,
        plan.desired_bombers,
        obs,
        enlisted,
        |kind| kind == bomber_kind,
    );
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    assign_exact(
        &mut plan.screen,
        plan.desired_screen,
        obs,
        enlisted,
        |kind| kind == screen_kind,
    );
    schedule_missing_members(op, plan, profile, obs, scout_kind, bomber_kind, out);
    if op.scout.is_some()
        && op.artillery.len() == plan.desired_artillery
        && op.bombers.len() == plan.desired_bombers
        && plan.screen.len() == plan.desired_screen
    {
        if plan.airborne() {
            if !dispatch_scout(op, obs, intel, landing_sites, out) {
                recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                return;
            }
            enter(op, AirOperationPhase::SuppressAa, obs.tick);
            hold_air_strike(op, plan, obs, *home, out);
            return;
        }
        let Some(staging) = artillery_staging(op, obs, *home) else {
            recover(op, AirRecoveryReason::UnreachableStaging, obs.tick);
            return;
        };
        let staging = match staging {
            ArtilleryStaging::NeedsRecon(goal) => {
                if !dispatch_scout_to(op, obs, goal, out) {
                    recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                    return;
                }
                hold_bombers(op, obs, *home, out);
                return;
            }
            ArtilleryStaging::Ready(staging) => staging,
        };
        if !dispatch_scout(op, obs, intel, landing_sites, out) {
            recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
            return;
        }
        enter(op, AirOperationPhase::SuppressAa, obs.tick);
        stage_artillery(op, staging, out);
        hold_bombers(op, obs, *home, out);
    }
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
    let aa = intel.air_defense_at(op.target);
    let flak = if plan.airborne() {
        targetable_corridor_flak(intel, home, op.target, landing_sites)
    } else {
        targetable_flak(&aa)
    };
    if let Some(flak) = flak {
        if elapsed(op.phase_started_at, obs.tick) >= tuning.reaction_delay {
            let units = if plan.airborne() {
                air_strike_members(op, plan, obs)
            } else {
                op.artillery.clone()
            };
            if !plan.airborne() || plan.flak_dispatch.as_ref() != Some(&(flak, units.clone())) {
                out.intents.push(Intent::AttackUnits {
                    units: units.clone(),
                    target: Target::Building(flak),
                });
                if plan.airborne() {
                    // The suppression attack displaced the prior home hold.
                    // Clearing it lets Verify issue a fresh regroup order
                    // before the bombers commit to the primary objective.
                    op.bomber_hold = None;
                    plan.flak_dispatch = Some((flak, units));
                    plan.strike_dispatch = None;
                }
            }
            if !plan.airborne() {
                op.artillery_staging = None;
            }
        }
        let scouting = if plan.airborne() {
            dispatch_scout(op, obs, intel, landing_sites, out)
        } else {
            scout_and_hold(op, plan, obs, intel, home, &[], out)
        };
        if !scouting {
            out.intents.clear();
            recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        }
    } else {
        plan.flak_dispatch = None;
        if plan.airborne() {
            match airborne_corridor_status(op, plan, obs, intel, home, landing_sites) {
                AirborneCorridorStatus::Defended => {
                    recover(op, AirRecoveryReason::NewAirDefense, obs.tick);
                }
                AirborneCorridorStatus::Clear => {
                    enter(op, AirOperationPhase::Verify, obs.tick);
                    if !scout_and_hold(op, plan, obs, intel, home, landing_sites, out) {
                        out.intents.clear();
                        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                    }
                }
                AirborneCorridorStatus::NeedsRecon => {
                    if !scout_and_hold(op, plan, obs, intel, home, landing_sites, out) {
                        out.intents.clear();
                        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                    }
                }
            }
            return;
        }
        match aa.evidence() {
            AirDefenseEvidence::CurrentCoverage => {
                recover(op, AirRecoveryReason::NewAirDefense, obs.tick)
            }
            AirDefenseEvidence::VisibleWithoutKnownCoverage
                if corridor_clear(intel, home, op.target, &[]) =>
            {
                enter(op, AirOperationPhase::Verify, obs.tick);
                if !scout_and_hold(op, plan, obs, intel, home, &[], out) {
                    out.intents.clear();
                    recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                }
            }
            AirDefenseEvidence::RememberedCoverage
            | AirDefenseEvidence::Unknown
            | AirDefenseEvidence::VisibleWithoutKnownCoverage => {
                if !scout_and_hold(op, plan, obs, intel, home, &[], out) {
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
    let aa = intel.air_defense_at(op.target);
    let flak = if plan.airborne() {
        targetable_corridor_flak(intel, home, op.target, landing_sites)
    } else {
        targetable_flak(&aa)
    };
    if flak.is_some() {
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
                if !scout_and_hold(op, plan, obs, intel, home, landing_sites, out) {
                    out.intents.clear();
                    recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                }
            }
        }
        return;
    }
    match aa.evidence() {
        AirDefenseEvidence::CurrentCoverage => {
            recover(op, AirRecoveryReason::NewAirDefense, obs.tick)
        }
        AirDefenseEvidence::VisibleWithoutKnownCoverage
            if corridor_clear(intel, home, op.target, &[])
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
            if !scout_and_hold(op, plan, obs, intel, home, &[], out) {
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
    let flak = if plan.airborne() {
        targetable_corridor_flak(intel, home, op.target, landing_sites)
    } else if op.artillery.iter().any(|id| unit(obs, *id).is_some()) {
        targetable_flak(&intel.air_defense_at(op.target))
    } else {
        None
    };
    if flak.is_some() {
        enter(op, AirOperationPhase::SuppressAa, obs.tick);
        suppress(op, plan, context, out);
        return;
    }
    let staging = if plan.airborne() {
        None
    } else {
        let Some(staging) = artillery_staging(op, obs, home) else {
            recover(op, AirRecoveryReason::UnreachableStaging, obs.tick);
            return;
        };
        match staging {
            ArtilleryStaging::NeedsRecon(goal) => {
                if !dispatch_scout_to(op, obs, goal, out) {
                    recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
                    return;
                }
                hold_bombers(op, obs, home, out);
                return;
            }
            ArtilleryStaging::Ready(staging) => Some(staging),
        }
    };
    let corridor_clear = if plan.airborne() {
        airborne_corridor_status(op, plan, obs, intel, home, landing_sites)
            == AirborneCorridorStatus::Clear
    } else {
        corridor_clear(intel, home, op.target, landing_sites)
    };
    if !corridor_clear {
        recover(op, AirRecoveryReason::NewAirDefense, obs.tick);
        return;
    }
    let mut air_routes = RouteProjection::new(obs, crate::stats::Domain::Air);
    let attackers = air_strike_members(op, plan, obs);
    if !air_routes.group_reaches_command_goal(&attackers, op.target) {
        recover(op, AirRecoveryReason::UnreachableAirRoute, obs.tick);
        return;
    }
    if let Some(target) = live_strike_target(op, intel) {
        if let Some(id) = target.id {
            dispatch_air_strike(plan, obs, &attackers, AirStrikeDispatch::Attack(id), out);
        }
        op.strike_issued_at.get_or_insert(obs.tick);
    } else if target_visible(op, obs) {
        if op
            .strike_issued_at
            .is_some_and(|tick| elapsed(tick, obs.tick) >= tuning.reaction_delay.max(20))
        {
            recover(op, AirRecoveryReason::Complete, obs.tick);
            return;
        }
        dispatch_air_strike(
            plan,
            obs,
            &attackers,
            AirStrikeDispatch::AttackMove(op.target),
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
        AirStrikeDispatch::Attack(target) => Intent::AttackUnits {
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
    if elapsed(op.started_at, obs.tick) >= operation_timeout(profile, plan)
        || (!waiting_for_recon_scout
            && elapsed(op.phase_started_at, obs.tick) >= phase_timeout(op.phase, plan))
    {
        recover(op, AirRecoveryReason::Timeout, obs.tick);
        return;
    }
    if !plan.airborne()
        && op.phase < AirOperationPhase::Strike
        && intel.buildings().iter().any(|building| {
            building.player == op.target_player
                && building.anchor == op.target
                && building.evidence == ContactEvidence::Remembered
                && building
                    .last_seen
                    .is_none_or(|seen| elapsed(seen, obs.tick) > ACTIVE_OPERATION_TARGET_MEMORY)
        })
    {
        recover(op, AirRecoveryReason::StaleIntelligence, obs.tick);
        return;
    }
    let lost_required_force = if plan.airborne() {
        let live_bombers = op
            .bombers
            .iter()
            .filter(|id| unit(obs, **id).is_some())
            .count();
        live_bombers < plan.desired_bombers.div_ceil(2).max(1)
    } else {
        op.artillery.iter().all(|id| unit(obs, *id).is_none())
            || op
                .bombers
                .iter()
                .filter(|id| unit(obs, **id).is_some())
                .count()
                < plan.desired_bombers
    };
    if op.phase >= AirOperationPhase::SuppressAa
        && (op.scout.is_some_and(|id| unit(obs, id).is_none()) || lost_required_force)
    {
        recover(op, AirRecoveryReason::RequiredUnitLost, obs.tick);
        return;
    }
    let target_is_current = intel.buildings().iter().any(|building| {
        building.player == op.target_player
            && building.anchor == op.target
            && building.evidence == ContactEvidence::Current
    });
    if op.phase < AirOperationPhase::Strike && target_visible(op, obs) && !target_is_current {
        recover(op, AirRecoveryReason::ObjectiveLost, obs.tick);
    }
}

/// Schedules in demand order, spreading equal-load work across producers. If
/// the next otherwise-trainable member is unaffordable or all its queues are
/// full, the available bank is claimed so ordinary production cannot skim it.
fn schedule(obs: &Observation, demands: &[(UnitKind, usize)], out: &mut StrategicDecision) {
    let mut bank = obs.scrap;
    let mut added = vec![0usize; obs.my_buildings.len()];
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
                            queue.len().saturating_add(added[*index]) < depth
                        })
                })
                .min_by_key(|(index, building)| {
                    (obs.my_queues[*index].len() + added[*index], building.id)
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
            added[index] += 1;
            out.committed_scrap += cost;
            out.intents.push(Intent::TrainAt { building, kind });
        }
    }
}

fn select_target(
    intel: &StrategicIntelligence,
    now: Tick,
    tactical_memory: Tick,
) -> Option<&BuildingContact> {
    intel
        .buildings()
        .iter()
        .filter(|b| {
            b.built
                && value(b.kind) > 0
                && b.confidence_at(now) > 0
                && (b.evidence == ContactEvidence::Current
                    || b.last_seen
                        .is_some_and(|seen| elapsed(seen, now) <= tactical_memory))
        })
        .min_by_key(|b| {
            (
                b.evidence != ContactEvidence::Current,
                Reverse(value(b.kind)),
                Reverse(b.confidence_at(now)),
                b.anchor.y,
                b.anchor.x,
                b.player,
                b.kind,
            )
        })
}

fn select_wealthy_island_target<'a>(
    profile: &ResolvedProfile,
    obs: &Observation,
    home: TilePos,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    intel
        .buildings()
        .iter()
        .filter(|target| {
            target.built
                && value(target.kind) > 0
                && wealthy_island_target(profile, obs, home, target)
        })
        .min_by_key(|target| {
            (
                target.evidence != ContactEvidence::Current,
                Reverse(value(target.kind)),
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
) -> Option<&'a BuildingContact> {
    intel.buildings().iter().find(|target| {
        target.player == request.player
            && target.anchor == request.target
            && target.built
            && value(target.kind) > 0
            && wealthy_island_target(profile, obs, home, target)
    })
}

fn live_strike_target<'a>(
    op: &AirOperation,
    intel: &'a StrategicIntelligence,
) -> Option<&'a BuildingContact> {
    intel
        .buildings()
        .iter()
        .filter(|b| {
            b.evidence == ContactEvidence::Current
                && b.id.is_some()
                && b.built
                && b.player == op.target_player
                && b.anchor.manhattan(op.target) <= 4
                && value(b.kind) > 0
        })
        .min_by_key(|b| {
            (
                b.anchor != op.target,
                Reverse(value(b.kind)),
                b.anchor.y,
                b.anchor.x,
                b.id,
            )
        })
}

fn value(kind: BuildingKind) -> u8 {
    match kind {
        BuildingKind::Crucible => 10,
        BuildingKind::Airworks => 9,
        BuildingKind::Fabricator => 8,
        BuildingKind::Foundry => 7,
        BuildingKind::Extractor => 6,
        BuildingKind::Bastion | BuildingKind::RepairBay => 5,
        BuildingKind::Reclaimer | BuildingKind::Array => 4,
        BuildingKind::Turret => 2,
        BuildingKind::FlakTurret | BuildingKind::Barricade | BuildingKind::ScuttleCharge => 0,
    }
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

fn reservations(op: &AirOperation, plan: &AirPlan, obs: &Observation) -> Vec<UnitId> {
    let mut ids: Vec<_> = op
        .scout
        .into_iter()
        .chain(op.artillery.iter().copied())
        .chain(op.bombers.iter().copied())
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
    op.bombers.retain(keep);
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

fn combined_combat_cost(plan: &AirPlan, profile: &ResolvedProfile, obs: &Observation) -> u32 {
    let artillery = preferred_artillery(profile, obs).stats().cost;
    let bomber = Role::Bomber.unit_for(obs.faction).stats().cost;
    artillery
        .saturating_mul(plan.desired_artillery as u32)
        .saturating_add(bomber.saturating_mul(plan.desired_bombers as u32))
}

fn combined_combat_ceiling(obs: &Observation) -> u32 {
    Role::Bomber
        .unit_for(obs.faction)
        .stats()
        .cost
        .saturating_mul(STANDARD_BOMBERS as u32)
        .saturating_add(UnitKind::Bombard.stats().cost)
}

fn schedule_missing_members(
    op: &AirOperation,
    plan: &AirPlan,
    profile: &ResolvedProfile,
    obs: &Observation,
    scout_kind: UnitKind,
    bomber_kind: UnitKind,
    out: &mut StrategicDecision,
) {
    let missing_scout =
        1usize.saturating_sub(usize::from(op.scout.is_some()) + queued(obs, |k| k == scout_kind));
    let missing_artillery = plan
        .desired_artillery
        .saturating_sub(op.artillery.len() + queued(obs, is_artillery));
    let missing_bombers = plan
        .desired_bombers
        .saturating_sub(op.bombers.len() + queued(obs, |k| k == bomber_kind));
    let screen_kind = Role::AirGround.unit_for(obs.faction);
    let missing_screen = plan
        .desired_screen
        .saturating_sub(plan.screen.len() + queued(obs, |kind| kind == screen_kind));
    let demands = if plan.airborne() {
        [
            (scout_kind, missing_scout),
            (screen_kind, missing_screen),
            (bomber_kind, missing_bombers),
        ]
    } else {
        [
            (scout_kind, missing_scout),
            (preferred_artillery(profile, obs), missing_artillery),
            (bomber_kind, missing_bombers),
        ]
    };
    schedule(obs, &demands, out);
}

fn ready_to_prepare(profile: &ResolvedProfile, obs: &Observation) -> bool {
    let kinds = [
        Role::Scout.unit_for(obs.faction),
        preferred_artillery(profile, obs),
        Role::Bomber.unit_for(obs.faction),
    ];
    kinds
        .into_iter()
        .all(|kind| requirements_met(obs, kind) && has_producer(obs, kind))
}

fn scout_and_hold(
    op: &mut AirOperation,
    plan: &AirPlan,
    obs: &Observation,
    intel: &StrategicIntelligence,
    home: TilePos,
    landing_sites: &[TilePos],
    out: &mut StrategicDecision,
) -> bool {
    if !dispatch_scout(op, obs, intel, landing_sites, out) {
        return false;
    }
    hold_air_strike(op, plan, obs, home, out);
    true
}

fn dispatch_scout(
    op: &mut AirOperation,
    obs: &Observation,
    intel: &StrategicIntelligence,
    landing_sites: &[TilePos],
    out: &mut StrategicDecision,
) -> bool {
    let Some(goal) = scout_goal(op, obs, intel, landing_sites) else {
        return false;
    };
    dispatch_scout_to(op, obs, goal, out)
}

fn dispatch_scout_to(
    op: &mut AirOperation,
    obs: &Observation,
    goal: TilePos,
    out: &mut StrategicDecision,
) -> bool {
    let Some(scout) = op.scout else {
        return true;
    };
    let Some(member) = unit(obs, scout) else {
        return false;
    };
    let mut air_routes = RouteProjection::new(obs, crate::stats::Domain::Air);
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
    landing_sites: &[TilePos],
) -> Option<TilePos> {
    let vision = Role::Scout
        .unit_for(obs.faction)
        .stats()
        .vision
        .saturating_sub(1);
    let current = op
        .scout
        .and_then(|id| unit(obs, id))
        .map_or(op.target, |scout| scout.tile);
    let focus = flight_objectives(op.target, landing_sites)
        .into_iter()
        .find(|objective| {
            approach(current, *objective).any(|tile| {
                intel.air_defense_at(tile).evidence()
                    != AirDefenseEvidence::VisibleWithoutKnownCoverage
            })
        })
        .unwrap_or(op.target);
    let mut routes = RouteProjection::new(obs, crate::stats::Domain::Air);
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

/// Parks the bomber wing on the landing pad. A landed bomber is idle, so
/// the later strike dispatch lifts it off exactly like a person clicking an
/// attack on a parked aircraft.
fn hold_bombers(
    op: &mut AirOperation,
    obs: &Observation,
    home: TilePos,
    out: &mut StrategicDecision,
) {
    let pad = landing_pad(obs, home).unwrap_or(home);
    if !op.bombers.is_empty() && op.bomber_hold != Some(pad) {
        out.intents.push(Intent::MoveUnits {
            units: op.bombers.clone(),
            goal: pad,
        });
        op.bomber_hold = Some(pad);
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
        hold_bombers(op, obs, home, out);
        return;
    }
    let mut units = op.bombers.clone();
    units.extend(plan.screen.iter().copied());
    units.sort_unstable();
    units.dedup();
    let pad = landing_pad(obs, home).unwrap_or(home);
    if units.is_empty() || op.bomber_hold == Some(pad) {
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
    op.bomber_hold = Some(pad);
}

fn air_strike_members(op: &AirOperation, plan: &AirPlan, obs: &Observation) -> Vec<UnitId> {
    let mut units: Vec<_> = op
        .bombers
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

fn target_seen(op: &AirOperation, obs: &Observation) -> bool {
    obs.enemy_buildings
        .iter()
        .any(|b| b.seen && b.player == op.target_player && b.anchor == op.target)
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

fn unit(obs: &Observation, id: UnitId) -> Option<&UnitObs> {
    obs.my_units
        .binary_search_by_key(&id, |member| member.id)
        .ok()
        .map(|index| &obs.my_units[index])
}

fn eligible(profile: &ResolvedProfile) -> bool {
    let stance = match profile.stance {
        BotStance::Turtle => -12,
        BotStance::Balanced => 0,
        BotStance::Aggressive => 12,
    };
    let air_identity = matches!(profile.primary, Specialty::Air)
        || matches!(profile.secondary, Specialty::Air)
        || profile.traits.air >= 65;
    let siege_identity = matches!(profile.primary, Specialty::Siege)
        || matches!(profile.secondary, Specialty::Siege)
        || profile.traits.siege >= 65;
    (air_identity || siege_identity)
        && profile.traits.siege >= 40
        && i16::from(profile.traits.air) + i16::from(profile.traits.siege) + stance >= 100
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
        && known_ground_disconnected(obs, home, target.anchor, target.kind.base_stats().size)
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
) -> bool {
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
        .collect();
    let goals: Vec<_> = crate::tick::rect_adjacent_tiles(target, target_size)
        .filter(|tile| routing::ground_open(obs, *tile))
        .collect();
    if starts.is_empty() || goals.is_empty() {
        return false;
    }
    let mut routes = RouteProjection::known_ground(obs);
    !starts
        .iter()
        .any(|start| goals.iter().any(|goal| routes.reaches(*start, *goal)))
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

fn staging(home: TilePos, target: TilePos) -> TilePos {
    TilePos::new(
        home.x + (target.x - home.x) / 3,
        home.y + (target.y - home.y) / 3,
    )
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
) -> Option<ArtilleryStaging> {
    let ideal = staging(home, op.target);
    let mut routes = RouteProjection::new(obs, crate::stats::Domain::Ground);
    for radius in 0i32..=3 {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) != radius {
                    continue;
                }
                let candidate = ideal.offset(dx, dy);
                if !routing::ground_open(obs, candidate) {
                    continue;
                }
                if op.artillery.iter().all(|id| {
                    unit(obs, *id).is_some_and(|member| routes.unit_reaches(member, candidate))
                }) {
                    return Some(if obs.explored(candidate) {
                        ArtilleryStaging::Ready(candidate)
                    } else {
                        ArtilleryStaging::NeedsRecon(candidate)
                    });
                }
            }
        }
    }
    None
}

fn elapsed(start: Tick, now: Tick) -> Tick {
    now.saturating_sub(start)
}

fn enter(op: &mut AirOperation, phase: AirOperationPhase, now: Tick) {
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
    use crate::scenario::{BotConfig, BotDifficulty};
    use crate::state::Faction;

    const HOME: TilePos = TilePos::new(3, 10);
    const TARGET: TilePos = TilePos::new(24, 10);

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
            bomber_hold: None,
            artillery_staging: None,
            artillery: vec![UnitId(2)],
            bombers: vec![UnitId(3), UnitId(4)],
            strike_issued_at: None,
            recovery_reason: None,
        }
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

    fn explore(obs: &mut Observation, tile: TilePos) {
        let index = usize::try_from(tile.y * obs.map_width + tile.x).unwrap();
        obs.explored[index] = true;
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
                plan: AirPlan::combined(&profile(), &observation),
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
        operation.bombers = (100..110).map(UnitId).collect();
        let mut plan = AirPlan::island(&profile(), &battle);
        plan.desired_bombers = 10;
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
        }
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
                },
            ),
            StrategicDecision::default()
        );
        assert!(blocked.air_operation().is_none());

        let mut battle = obs(100);
        see_approach(&mut battle);
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
        assert!(standby.iter().all(|unit| retry.bombers.contains(unit)));
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

        let mut first = StrategicDecision::default();
        assert!(dispatch_scout(
            &mut operation,
            &observation,
            &intel,
            &[],
            &mut first
        ));
        let first_goal = match first.intents.as_slice() {
            [Intent::MoveUnits { units, goal }] if units == &[UnitId(1)] => *goal,
            intents => panic!("expected one scout dispatch, got {intents:?}"),
        };

        let mut repeated = StrategicDecision::default();
        assert!(dispatch_scout(
            &mut operation,
            &observation,
            &intel,
            &[],
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
            &observation,
            &intel,
            &[],
            &mut changed
        ));
        assert!(matches!(
            changed.intents.as_slice(),
            [Intent::MoveUnits { units, goal }]
                if units == &[UnitId(1)] && *goal != first_goal
        ));
    }

    #[test]
    fn bomber_hold_is_dispatched_once_until_home_changes() {
        let observation = obs(100);
        let mut operation = operation(AirOperationPhase::SuppressAa, 100);
        let pad = landing_pad(&observation, HOME).expect("open ground rings the home anchor");
        assert_eq!(
            pad,
            HOME.offset(-2, -2),
            "the pad is the first ring-two tile by (y, x)"
        );

        let mut first = StrategicDecision::default();
        hold_bombers(&mut operation, &observation, HOME, &mut first);
        assert_eq!(
            first.intents,
            [Intent::MoveUnits {
                units: vec![UnitId(3), UnitId(4)],
                goal: pad,
            }]
        );
        assert_eq!(operation.bomber_hold, Some(pad));

        let mut repeated = StrategicDecision::default();
        hold_bombers(&mut operation, &observation, HOME, &mut repeated);
        assert!(
            repeated.intents.is_empty(),
            "the stable hold remains authoritative"
        );

        let replacement_home = HOME.offset(2, 1);
        let replacement_pad = landing_pad(&observation, replacement_home).unwrap();
        assert_ne!(replacement_pad, pad);
        let mut redirected = StrategicDecision::default();
        hold_bombers(
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
        hold_bombers(
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
        assert_eq!(operation.bomber_hold, Some(pad));

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
        let mut plan = AirPlan::combined(&identity, &suppression_observation);
        let context = AirPlanningContext {
            profile: &identity,
            tuning: DifficultyTuning::for_level(BotDifficulty::Prime),
            obs: &suppression_observation,
            intel: &intelligence,
            home: HOME,
            enlisted: &[],
            landing_sites: &[],
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
        assert!(matches!(
            training.intents.as_slice(),
            [Intent::TrainAt {
                building: BuildingId(10),
                kind: UnitKind::Kestrel,
            }]
        ));

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

        planner.think(&profile(), tuning, &reacquired, &intelligence, HOME, &[]);

        assert_eq!(
            planner.air_operation().unwrap().phase,
            AirOperationPhase::Assemble,
            "Prime may react immediately only after the required scout has a real dispatch"
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
        obs.my_units.truncate(1);
        obs.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), false),
            building(11, 0, BuildingKind::Fabricator, TilePos::new(5, 2), false),
            building(12, 0, BuildingKind::Airworks, TilePos::new(2, 5), false),
            building(13, 0, BuildingKind::Airworks, TilePos::new(5, 5), false),
            building(14, 0, BuildingKind::Crucible, TilePos::new(8, 5), false),
        ];
        obs.my_queues = vec![Vec::new(); 5];
        let full = UnitKind::Bombard.stats().cost + UnitKind::Condor.stats().cost * 2;
        obs.scrap = full;
        let intel = knowledge(&obs);
        let mut planner = with_operation(AirOperationPhase::Assemble, 200);
        let out = think(&mut planner, &obs, &intel);
        assert_eq!(out.committed_scrap, full);
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
        let intel = knowledge(&obs);
        let mut planner = with_operation(AirOperationPhase::Assemble, 200);
        let partial = think(&mut planner, &obs, &intel);
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

        let goal = scout_goal(&operation, &obs, &intel, &[]).expect("safe spotting tile");
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
    fn bomber_loss_aborts_recovers_survivors_and_starts_cooldown() {
        let mut battle = obs(300);
        battle.my_units.retain(|unit| unit.id != UnitId(4));
        let intel = knowledge(&battle);
        let mut planner = with_operation(AirOperationPhase::Strike, 300);
        let out = think(&mut planner, &battle, &intel);
        let op = planner.air_operation().unwrap();
        assert_eq!(op.phase, AirOperationPhase::Recover);
        assert_eq!(
            op.recovery_reason,
            Some(AirRecoveryReason::RequiredUnitLost)
        );
        assert!(planner.cooldown_until() > 300);
        assert!(out.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(1), UnitId(2), UnitId(3)],
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
        settled.scrap = UnitKind::Condor.stats().cost;
        let mut planner = with_operation(AirOperationPhase::Recover, settled.tick);
        planner.cooldown_until = 500;
        let operation = planner.air_op_mut().unwrap();
        operation.artillery = vec![UnitId(2), UnitId(5)];
        operation.bombers = vec![UnitId(3)];
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
        let retried = think(&mut planner, &settled, &intel);
        let operation = planner.air_operation().expect("a new operation starts");
        assert_eq!(operation.scout, Some(UnitId(1)));
        assert_eq!(operation.artillery, [UnitId(2), UnitId(5)]);
        assert_eq!(operation.bombers, [UnitId(3)]);
        assert!(retried.intents.contains(&Intent::TrainAt {
            building: BuildingId(11),
            kind: UnitKind::Condor,
        }));
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
            artillery_staging(operation, &observation, HOME),
            Some(ArtilleryStaging::Ready(staging_goal)),
            "the artillery is already across the peak wall on explored staging ground"
        );
        assert_eq!(
            scout_goal(operation, &observation, &intelligence, &[]),
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
    fn a_non_air_identity_does_not_run_the_combined_air_playbook() {
        let observation = obs(100);
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

        assert_eq!(decision, StrategicDecision::default());
        assert!(planner.air_operation().is_none());
    }

    #[test]
    fn resolved_siege_identity_enters_the_playbook_with_substituted_composition() {
        let mut observation = obs(120);
        observation
            .my_units
            .extend((5..=13).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
        observation.my_units.sort_unstable_by_key(|unit| unit.id);
        observation.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), false),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), false),
            building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), false),
        ];
        observation.my_queues = vec![Vec::new(); observation.my_buildings.len()];
        observation.scrap = 10_000;
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
        assert_eq!(
            (siege_plan.desired_artillery, siege_plan.desired_bombers),
            (1, 1),
            "the available Avalanche consumes the two-light-artillery allocation"
        );
        assert_eq!(
            preferred_artillery(&siege, &observation),
            UnitKind::Avalanche
        );

        let mut low_planner = StrategicPlanner::new();
        let low_decision = low_planner.think(
            &low_siege,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            &observation,
            &intel,
            HOME,
            &[],
        );
        assert_eq!(low_decision, StrategicDecision::default());
        assert!(low_planner.air_operation().is_none());
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
        assert!(eligible(&identity));

        let mut immature = obs(120);
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
        assert!(ready_to_prepare(&identity, &immature));
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
            admitted.intents.iter().any(|intent| matches!(
                intent,
                Intent::TrainAt {
                    kind: UnitKind::Bombard,
                    ..
                }
            )),
            "the mature roster should admit the identity's missing suppression piece: {admitted:?}"
        );
        assert!(admitted.committed_scrap >= UnitKind::Bombard.stats().cost);
    }

    #[test]
    fn air_and_siege_leading_compositions_share_one_combat_ceiling() {
        let mut observation = obs(100);
        observation.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), false),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), false),
            building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), false),
        ];
        observation.my_queues = vec![Vec::new(); observation.my_buildings.len()];
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

        let siege_plan = AirPlan::combined(&siege, &observation);
        let air_plan = AirPlan::combined(&air, &observation);

        assert_eq!(
            (siege_plan.desired_artillery, siege_plan.desired_bombers),
            (1, 1)
        );
        assert_eq!(
            (air_plan.desired_artillery, air_plan.desired_bombers),
            (1, 2)
        );
        for (profile, plan) in [(&siege, &siege_plan), (&air, &air_plan)] {
            assert!(plan.desired_artillery + plan.desired_bombers <= 3);
            assert!(
                combined_combat_cost(plan, profile, &observation)
                    <= combined_combat_ceiling(&observation)
            );
        }
    }

    #[test]
    fn the_authored_single_bomber_siege_wing_survives_attrition_checks_and_strikes() {
        let identity = ResolvedProfile::resolve(BotConfig::scripted(
            BotDifficulty::Prime,
            BotStance::Balanced,
            1_616_207,
        ));
        assert_eq!(
            (identity.primary, identity.secondary),
            (Specialty::Siege, Specialty::Support),
            "the replay-derived identity must retain its connected Siege plan"
        );
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let mut battle = obs(300);
        battle.visible.fill(true);
        battle.explored.fill(true);
        battle
            .my_units
            .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
        battle.my_units.sort_unstable_by_key(|unit| unit.id);

        let mut operation = operation(AirOperationPhase::SuppressAa, battle.tick);
        operation.artillery = vec![UnitId(2), UnitId(5)];
        operation.bombers = vec![UnitId(3)];
        let plan = AirPlan::combined(&identity, &battle);
        assert_eq!((plan.desired_artillery, plan.desired_bombers), (2, 1));
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

        let suppression = planner.think(&identity, tuning, &battle, &intelligence, HOME, &[]);
        let operation = planner
            .air_operation()
            .expect("the complete one-bomber wing remains active");
        assert_eq!(operation.phase, AirOperationPhase::Verify);
        assert_eq!(operation.recovery_reason, None);
        assert!(suppression.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));

        battle.tick += tuning.cadence;
        intelligence.update(&battle);
        let strike = planner.think(&identity, tuning, &battle, &intelligence, HOME, &[]);
        assert!(strike.intents.contains(&Intent::AttackUnits {
            units: vec![UnitId(3)],
            target: Target::Building(BuildingId(80)),
        }));
        let operation = planner
            .air_operation()
            .expect("the connected strike remains inspectable");
        assert_eq!(operation.phase, AirOperationPhase::Strike);
        assert_eq!(operation.recovery_reason, None);

        battle.my_units.retain(|unit| unit.id != UnitId(3));
        battle.tick += tuning.cadence;
        intelligence.update(&battle);
        planner.think(&identity, tuning, &battle, &intelligence, HOME, &[]);
        let operation = planner
            .air_operation()
            .expect("the attrited strike remains observable during recovery");
        assert_eq!(operation.phase, AirOperationPhase::Recover);
        assert_eq!(
            operation.recovery_reason,
            Some(AirRecoveryReason::RequiredUnitLost)
        );
    }

    #[test]
    fn secondary_siege_waits_for_two_bombards_when_an_avalanche_is_unavailable() {
        let mut identity = profile();
        identity.primary = Specialty::Support;
        identity.secondary = Specialty::Siege;
        identity.traits.air = 60;
        identity.traits.siege = 60;

        let mut battle = obs(300);
        battle.explored.fill(true);
        let mut operation = operation(AirOperationPhase::Assemble, battle.tick);
        operation.artillery = vec![UnitId(2)];
        operation.bombers = vec![UnitId(3)];
        let plan = AirPlan::combined(&identity, &battle);
        assert_eq!(preferred_artillery(&identity, &battle), UnitKind::Bombard);
        assert_eq!(plan.desired_artillery, 2);
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
        let mut intel = knowledge(&battle);

        let waiting = planner.think(&identity, tuning, &battle, &intel, HOME, &[]);

        let operation = planner.air_operation().expect("assembly remains active");
        assert_eq!(operation.phase, AirOperationPhase::Assemble);
        assert_eq!(operation.artillery, [UnitId(2)]);
        assert!(waiting.intents.iter().all(|intent| !matches!(
            intent,
            Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
        )));

        battle
            .my_units
            .push(own(5, UnitKind::Bombard, TilePos::new(9, 10)));
        battle.my_units.sort_unstable_by_key(|unit| unit.id);
        battle.tick += 1;
        intel.update(&battle);
        let assembled = planner.think(&identity, tuning, &battle, &intel, HOME, &[]);

        let operation = planner.air_operation().expect("operation advances");
        assert_eq!(operation.phase, AirOperationPhase::SuppressAa);
        assert_eq!(operation.artillery, [UnitId(2), UnitId(5)]);
        assert!(assembled.intents.contains(&Intent::MoveUnits {
            units: vec![UnitId(2), UnitId(5)],
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
        assert!(plan.desired_bombers >= 4);
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
    fn partial_scattering_knowledge_treats_the_missing_ground_route_as_an_air_problem() {
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

        assert!(wealthy_island_target(
            &profile(),
            &observation,
            HOME,
            &target,
        ));

        for x in HOME.x + 2..TARGET.x {
            explore(&mut observation, TilePos::new(x, HOME.y));
        }
        assert!(
            !wealthy_island_target(&profile(), &observation, HOME, &target),
            "a fully explored open road keeps the ordinary ground war available"
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
        assert!(operation.bombers.is_empty());
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
        assert!(operation.bombers.is_empty());
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
            assert!(ghost_operation.artillery.is_empty(), "{difficulty:?}");
            assert!(ghost_operation.bombers.is_empty(), "{difficulty:?}");
            assert_eq!(recon.reservations, [UnitId(1)], "{difficulty:?}");
            assert_eq!(recon.committed_scrap, 0, "{difficulty:?}");

            let current = wealthy_island_obs(5_016, 1);
            intel.update(&current);
            planner.think(&identity, tuning, &current, &intel, HOME, &[]);
            let admitted = planner.air_operation().unwrap();
            assert!(admitted.assault_admitted, "{difficulty:?}");
            assert_eq!(admitted.started_at, 5_016, "{difficulty:?}");
            let plan = planner.air_plan().unwrap();
            snapshots.push((
                admitted.artillery.clone(),
                admitted.bombers.clone(),
                plan.desired_artillery,
                plan.desired_bombers,
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
        assert!(plan.desired_bombers <= STANDARD_BOMBERS);
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
        assert_eq!(plan.desired_bombers, 6);
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
            "two existing bombers contribute to the current six-bomber wing"
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
            assert_eq!(plan.desired_bombers, expected.desired_bombers);
            assert_eq!(plan.desired_screen, expected.desired_screen);
            assert_eq!(plan.assembly_timeout, expected.assembly_timeout);
            snapshots.push((plan.desired_bombers, plan.desired_screen));
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
        assert!(current.desired_bombers > frozen.desired_bombers);
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
        assert!(current.desired_bombers < frozen.desired_bombers);
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
        for offset in 0..plan.desired_bombers.saturating_sub(operation.bombers.len()) {
            let id = 100 + u32::try_from(offset).unwrap();
            battle
                .my_units
                .push(own(id, bomber_kind, TilePos::new(4, 6)));
            operation.bombers.push(UnitId(id));
        }
        for offset in 0..plan.desired_screen {
            let id = 200 + u32::try_from(offset).unwrap();
            battle
                .my_units
                .push(own(id, screen_kind, TilePos::new(5, 6)));
            plan.screen.push(UnitId(id));
        }
        battle.my_units.sort_unstable_by_key(|unit| unit.id);
        let frozen_bombers = plan.desired_bombers;
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
        assert!(unconstrained.desired_bombers > frozen_bombers);
        assert!(unconstrained.desired_screen > frozen_screen);
        let intel = knowledge(&battle);

        think(&mut planner, &battle, &intel);

        let committed = planner
            .air_plan()
            .expect("the committed operation retains its frozen plan");
        assert_eq!(committed.desired_bombers, frozen_bombers);
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
        plan.desired_bombers = 2;
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
            plan.desired_bombers = 2;
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
                    plan: AirPlan::combined(&profile(), &observation),
                }),
                standby: AirStandby::default(),
                cooldown_until: 0,
                terminal_outcome: None,
            }
        };

        let mut current = obs(400);
        see_approach(&mut current);
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
        plan.desired_bombers = 2;
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
        planner.air_op_mut().unwrap().bombers.clear();
        planner.air_plan_mut().unwrap().screen.clear();
        let intel = knowledge(&battle);

        let assembly = think(&mut planner, &battle, &intel);
        let frozen = planner.air_operation().unwrap();
        assert_eq!(frozen.phase, AirOperationPhase::SuppressAa);
        assert_eq!(frozen.bombers.len(), 10);
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
        flak_planner.air_op_mut().unwrap().bombers.clear();
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
        for phase in [AirOperationPhase::SuppressAa, AirOperationPhase::Verify] {
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
        plan.desired_bombers = 2;
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
        plan.desired_bombers = 2;
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
        plan.desired_bombers = 2;
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
        plan.desired_bombers = 2;
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
        plan.desired_bombers = 2;
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
        plan.desired_bombers = 2;
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
        observation
            .my_units
            .extend((100..=108).map(|id| own(id, UnitKind::Sentinel, TilePos::new(7, 10))));
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

        observation.my_buildings = vec![
            building(10, 0, BuildingKind::Fabricator, TilePos::new(2, 2), true),
            building(11, 0, BuildingKind::Airworks, TilePos::new(5, 2), true),
            building(12, 0, BuildingKind::Crucible, TilePos::new(8, 2), true),
        ];
        observation.my_queues = vec![Vec::new(); 3];
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
                bombers: vec![UnitId(3), UnitId(4)],
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
        plan.desired_bombers = 2;
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
        plan.desired_bombers = 2;
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
        plan.desired_bombers = 2;
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
    fn visible_mobile_aa_aborts_both_suppression_and_verification() {
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
                .expect("recovery remains observable");

            assert_eq!(operation.phase, AirOperationPhase::Recover);
            assert_eq!(
                operation.recovery_reason,
                Some(AirRecoveryReason::NewAirDefense)
            );
            assert!(decision.intents.iter().all(|intent| !matches!(
                intent,
                Intent::AttackUnits { .. } | Intent::AttackMoveUnits { .. }
            )));
        }
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
            plan.desired_bombers = 2;
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
            plan.desired_bombers = 2;
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
        plan.desired_bombers = 2;
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
        plan.desired_bombers = 2;
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
        let plan = AirPlan::combined(&identity, &observation);
        let mut operation = operation(AirOperationPhase::Assemble, observation.tick);
        operation.artillery.clear();
        let mut decision = StrategicDecision::default();

        schedule_missing_members(
            &operation,
            &plan,
            &identity,
            &observation,
            UnitKind::Kestrel,
            UnitKind::Condor,
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
        ));

        observation.scrap = bomber_bank - 1;
        assert!(
            !wealthy_island_target(&profile(), &observation, HOME, &target),
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
        assert!(wealthy_island_target(&high, &early, HOME, &early_target));
        assert!(
            !wealthy_island_target(&low, &early, HOME, &early_target),
            "the lower-air identity keeps the operation but prepares it longer"
        );

        let later = wealthy_island_obs(earliest(&low), 1);
        let later_target = knowledge(&later).buildings()[0].clone();
        assert!(wealthy_island_target(&low, &later, HOME, &later_target));
        assert!(wealthy_island_target(&high, &later, HOME, &later_target));

        let low_plan = AirPlan::island(&low, &later);
        let high_plan = AirPlan::island(&high, &later);
        assert_eq!(high_plan.desired_bombers, low_plan.desired_bombers + 1);
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
        ));
        assert!(!wealthy_island_target(&turtle, &observation, HOME, &target,));

        let aggressive_plan = AirPlan::island(&aggressive, &observation);
        let turtle_plan = AirPlan::island(&turtle, &observation);
        assert_eq!(
            aggressive_plan.desired_bombers,
            turtle_plan.desired_bombers + 2
        );
        assert!(aggressive_plan.assembly_timeout > turtle_plan.assembly_timeout);

        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        assert!(cooldown(&turtle, tuning) > cooldown(&aggressive, tuning));

        let mut aggressive_threshold = aggressive;
        aggressive_threshold.traits.air = 60;
        aggressive_threshold.traits.siege = 40;
        let mut turtle_threshold = aggressive_threshold;
        turtle_threshold.stance = BotStance::Turtle;
        assert!(eligible(&aggressive_threshold));
        assert!(!eligible(&turtle_threshold));

        let mut later = observation.clone();
        later.tick = later.tick.saturating_add(500);
        assert!(wealthy_island_target(&turtle, &later, HOME, &target,));
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
