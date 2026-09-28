//! Persistent air operation state: plans, stages, membership, dispatch
//! memory, and checkpoint validation.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum SuppressionDispatch {
    Position {
        target: Target,
        assignments: Vec<(UnitId, TilePos)>,
    },
    Attack {
        target: Target,
        units: Vec<UnitId>,
    },
}

/// The kind of operation the planner is running. A remembered objective is
/// only watched until current sight admits it as an island or connected
/// assault; only an assault sizes and reserves a strike force.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum AirPlan {
    Reacquire(ReacquirePlan),
    Island(IslandPlan),
    Connected(Box<ConnectedPlan>),
}

/// Scout-only watch over a remembered objective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ReacquirePlan {
    pub(super) admitted_at: Tick,
    pub(super) assembly_timeout: Tick,
    pub(super) terrain: ReacquireTerrain,
}

/// Known ground connectivity of a remembered objective when it was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum ReacquireTerrain {
    Severed,
    Connected,
}

/// Massed-air assault on a ground-severed objective, sized once at admission.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct IslandPlan {
    pub(super) admitted_at: Tick,
    pub(super) desired_strike_aircraft: usize,
    pub(super) desired_screen: usize,
    pub(super) screen: Vec<UnitId>,
    pub(super) assembly_timeout: Tick,
    pub(super) dispatch: AirDispatch,
}

/// Combined-arms assault on a ground-connected cluster sized by its package.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ConnectedPlan {
    pub(super) commitment: ConnectedCommitment,
    /// Sticky member the operation stages, scouts, and strikes first. It moves
    /// only during preparation, and only when it is no longer in current sight.
    pub(super) focus: TilePos,
    pub(super) package: ConnectedForcePackage,
    pub(super) paid_production: Vec<ConnectedPurchase>,
    pub(super) dispatch: AirDispatch,
}

/// Identity and target set a connected operation commits to at admission.
/// Revisions resize the package against these members but never change them;
/// a member leaves the operation only when current sight finds it gone.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ConnectedCommitment {
    pub(super) player: PlayerId,
    pub(super) primary: BuildingId,
    pub(super) primary_kind: BuildingKind,
    /// Primary anchor at admission.
    pub(super) scope: TilePos,
    /// Canonical `(y, x)` anchors of every admitted member, including `scope`.
    pub(super) anchors: Vec<TilePos>,
    pub(super) minimum_capability: NormalizedCapability,
    pub(super) admitted_at: Tick,
    pub(super) deadline: Tick,
}

/// Last tactical orders issued by an assault, used to avoid reissuing them.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct AirDispatch {
    pub(super) suppression: Option<SuppressionDispatch>,
    pub(super) strike: Option<AirStrikeDispatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum AirStrikeDispatch {
    Attack { target: BuildingId, anchor: TilePos },
    Hunt(TilePos),
}

impl AirPlan {
    pub(super) fn remembered_connected(obs: &Observation) -> Self {
        Self::Reacquire(ReacquirePlan {
            admitted_at: obs.tick,
            assembly_timeout: CONNECTED_PREPARATION_HORIZON,
            terrain: ReacquireTerrain::Connected,
        })
    }

    pub(super) fn admitted_at(&self) -> Tick {
        match self {
            Self::Reacquire(plan) => plan.admitted_at,
            Self::Island(plan) => plan.admitted_at,
            Self::Connected(plan) => plan.commitment.admitted_at,
        }
    }

    pub(super) fn connected(&self) -> Option<&ConnectedPlan> {
        match self {
            Self::Connected(plan) => Some(plan),
            Self::Reacquire(_) | Self::Island(_) => None,
        }
    }

    pub(super) fn airborne(&self) -> bool {
        matches!(
            self,
            Self::Island(_)
                | Self::Reacquire(ReacquirePlan {
                    terrain: ReacquireTerrain::Severed,
                    ..
                })
        )
    }

    pub(super) fn package(&self) -> Option<&ConnectedForcePackage> {
        match self {
            Self::Connected(plan) => Some(&plan.package),
            Self::Reacquire(_) | Self::Island(_) => None,
        }
    }

    pub(super) fn screen(&self) -> &[UnitId] {
        match self {
            Self::Island(plan) => &plan.screen,
            Self::Reacquire(_) | Self::Connected(_) => &[],
        }
    }

    pub(super) fn screen_mut(&mut self) -> Option<&mut Vec<UnitId>> {
        match self {
            Self::Island(plan) => Some(&mut plan.screen),
            Self::Reacquire(_) | Self::Connected(_) => None,
        }
    }

    pub(super) fn desired_artillery(&self) -> usize {
        self.package()
            .map_or(0, |package| demand_count(&package.suppression))
    }

    pub(super) fn desired_strike_aircraft(&self) -> usize {
        match self {
            Self::Reacquire(_) => 0,
            Self::Island(plan) => plan.desired_strike_aircraft,
            Self::Connected(plan) => demand_count(&plan.package.strike),
        }
    }

    pub(super) fn desired_screen(&self) -> usize {
        match self {
            Self::Island(plan) => plan.desired_screen,
            Self::Reacquire(_) | Self::Connected(_) => 0,
        }
    }

    /// Assembly patience for an operation started at `started_at`. A
    /// connected window ends at its committed deadline, so revisions cannot
    /// shorten it.
    pub(super) fn assembly_timeout(&self, started_at: Tick) -> Tick {
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

    pub(super) fn suppression_dispatch(&self) -> Option<&SuppressionDispatch> {
        self.dispatch()
            .and_then(|dispatch| dispatch.suppression.as_ref())
    }

    pub(super) fn strike_dispatch(&self) -> Option<AirStrikeDispatch> {
        self.dispatch().and_then(|dispatch| dispatch.strike)
    }

    pub(super) fn set_suppression_dispatch(&mut self, suppression: Option<SuppressionDispatch>) {
        if let Some(dispatch) = self.dispatch_mut() {
            dispatch.suppression = suppression;
        }
    }

    pub(super) fn set_strike_dispatch(&mut self, strike: Option<AirStrikeDispatch>) {
        if let Some(dispatch) = self.dispatch_mut() {
            dispatch.strike = strike;
        }
    }
}

impl ReacquirePlan {
    /// Watches a severed objective with the patience its island assault
    /// would be granted if current sight admitted it now.
    pub(super) fn severed(island: &IslandPlan) -> Self {
        Self {
            admitted_at: island.admitted_at,
            assembly_timeout: island.assembly_timeout,
            terrain: ReacquireTerrain::Severed,
        }
    }
}

impl ConnectedPlan {
    pub(super) fn new(commitment: ConnectedCommitment, package: ConnectedForcePackage) -> Self {
        Self {
            focus: commitment.scope,
            commitment,
            package,
            paid_production: Vec::new(),
            dispatch: AirDispatch::default(),
        }
    }
}

impl ConnectedCommitment {
    pub(super) fn admit(
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

    pub(super) fn key(&self) -> crate::allocation::ConnectedOffenseKey {
        crate::allocation::ConnectedOffenseKey {
            objective: self.primary,
            anchor: self.scope,
        }
    }

    pub(super) fn contains(&self, anchor: TilePos) -> bool {
        self.anchors
            .binary_search_by_key(&(anchor.y, anchor.x), |member| (member.y, member.x))
            .is_ok()
    }

    /// Admitted members not yet observed gone. Out of sight, a member stays
    /// live on its remembered contact.
    pub(super) fn live_members<'a>(
        &self,
        intel: &'a StrategicIntelligence,
    ) -> Vec<&'a BuildingContact> {
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
}

pub(super) fn demand_count(demands: &[ProviderDemand]) -> usize {
    demands
        .iter()
        .map(|demand| demand.count)
        .fold(0usize, usize::saturating_add)
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

/// One-think terminal signal for a coordinated lift targeting the same base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum AirOperationOutcome {
    Released { player: PlayerId, target: TilePos },
    Aborted { player: PlayerId, target: TilePos },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum AirStage {
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
    pub(crate) stage: AirStage,
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

    pub(super) fn members(&self) -> impl Iterator<Item = UnitId> + '_ {
        AirRoster::from(self).members()
    }

    pub(super) fn admit_assault(&mut self, now: Tick) {
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
pub(super) struct AirStandby {
    pub(super) scout: Option<UnitId>,
    pub(super) artillery: Vec<UnitId>,
    pub(super) strike_aircraft: Vec<UnitId>,
}

impl AirStandby {
    pub(super) fn from_operation(op: &AirOperation, obs: &Observation) -> Self {
        let mut standby = Self {
            scout: op.scout,
            artillery: op.artillery.clone(),
            strike_aircraft: op.strike_aircraft.clone(),
        };
        standby.prune(obs);
        standby
    }

    pub(super) fn prune(&mut self, obs: &Observation) {
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

    pub(super) fn reservations(&self) -> Vec<UnitId> {
        let mut ids: Vec<_> = AirRoster::from(self).members().collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

/// A live operation and the plan it was admitted under. They exist only
/// together; holding them as one value makes the half-set state — which
/// a fallback once papered over by silently substituting a combined
/// plan for a possibly-island one — unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ActiveAirOperation {
    pub(super) op: AirOperation,
    pub(super) plan: AirPlan,
}

impl ActiveAirOperation {
    pub(super) fn valid_checkpoint(&self, map: &PublicMapBriefing, tick: Tick) -> bool {
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
            Some(AirStrikeDispatch::Attack { anchor, .. } | AirStrikeDispatch::Hunt(anchor)) => {
                on_map(map, anchor)
            }
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

pub(super) fn strictly_increasing(ids: &[UnitId]) -> bool {
    ids.windows(2).all(|pair| pair[0] < pair[1])
}

pub(super) fn on_map(map: &PublicMapBriefing, tile: TilePos) -> bool {
    (0..map.map_width()).contains(&tile.x) && (0..map.map_height()).contains(&tile.y)
}

/// A purchase already emitted through shared allocation. Predicted completion
/// only releases ownership after the observed queue can actually advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ConnectedPurchase {
    pub(super) producer: BuildingId,
    pub(super) kind: UnitKind,
    pub(super) issued_at: Tick,
    pub(super) ready_at: Tick,
    pub(super) delayed: bool,
}

impl ConnectedPurchase {
    pub(crate) const fn producer(self) -> BuildingId {
        self.producer
    }
    pub(crate) const fn kind(self) -> UnitKind {
        self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AirMembership {
    pub(super) scout: Option<UnitId>,
    pub(super) artillery: Vec<UnitId>,
    pub(super) strike_aircraft: Vec<UnitId>,
    pub(super) screen: Vec<UnitId>,
}

impl AirMembership {
    pub(super) fn from_active(active: &ActiveAirOperation) -> Self {
        Self {
            scout: active.op.scout,
            artillery: active.op.artillery.clone(),
            strike_aircraft: active.op.strike_aircraft.clone(),
            screen: active.plan.screen().to_vec(),
        }
    }

    pub(super) fn units(&self, obs: &Observation) -> Vec<UnitId> {
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
pub(super) struct AirRoster<'a> {
    pub(super) scout: Option<UnitId>,
    pub(super) artillery: &'a [UnitId],
    pub(super) strike_aircraft: &'a [UnitId],
}

impl<'a> AirRoster<'a> {
    pub(super) fn members(self) -> impl Iterator<Item = UnitId> + 'a {
        self.scout
            .into_iter()
            .chain(self.artillery.iter().copied())
            .chain(self.strike_aircraft.iter().copied())
    }

    /// Canonical ids of the members and `screen` still alive in `obs`.
    pub(super) fn live_members(self, screen: &[UnitId], obs: &Observation) -> Vec<UnitId> {
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

pub(super) fn operation_timeout(
    profile: &ResolvedProfile,
    op: &AirOperation,
    plan: &AirPlan,
) -> Tick {
    3_200u64
        .saturating_add(plan.assembly_timeout(op.started_at))
        .saturating_add(u64::from(100u8.saturating_sub(profile.traits.air)) * 4)
}

pub(super) fn phase_timeout(phase: AirOperationPhase, started_at: Tick, plan: &AirPlan) -> Tick {
    match phase {
        AirOperationPhase::Recon => 900,
        AirOperationPhase::Assemble => plan.assembly_timeout(started_at),
        AirOperationPhase::SuppressAa => 1_400,
        AirOperationPhase::Verify => 900,
        AirOperationPhase::Strike => 1_200,
        AirOperationPhase::Recover => 500,
    }
}

pub(super) fn elapsed(start: Tick, now: Tick) -> Tick {
    now.saturating_sub(start)
}

pub(super) fn enter(op: &mut AirOperation, stage: AirStage, now: Tick) {
    if stage == AirStage::SuppressAa {
        op.membership_frozen_at.get_or_insert(now);
    }
    op.stage = stage;
    op.phase_started_at = now;
}

pub(super) fn recover(op: &mut AirOperation, reason: AirRecoveryReason, now: Tick) {
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
