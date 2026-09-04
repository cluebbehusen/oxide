//! Fog-honest demand for independently useful standing-force production.
//!
//! Persistent operations own scouts, strike aircraft, transports, raiders,
//! and their exact cohorts. This module answers a narrower question: what one
//! unit would improve the ordinary force now, without turning an idle factory
//! into a reason to buy something.

use core::cmp::Reverse;
use std::collections::BTreeMap;

use super::DifficultyTuning;
use super::PublicMapBriefing;
use super::allocation::{
    Confidence, ExecutionSafety, ProposalCase, StandingForceKey, StandingForceServiceKey,
    StrategicValue, TimeToImpact, Urgency,
};
use super::executive::{full_ground_strength, ground_strength, weapon_burst_dps100};
use super::intelligence::{ContactEvidence, StrategicIntelligence};
use super::observation::Observation;
use super::orient::Orientation;
use super::profile::{ResolvedProfile, Specialty};
use super::resources::{ProducerEgress, ResourceSnapshot};
use super::routing::{RouteProjection, production_spawn_doorstep};
use crate::ids::{BuildingId, UnitId};
use crate::stats::{BuildingKind, Domain, Role, UnitKind};
use chassis::Tick;
use chassis::grid::TilePos;

/// One exact production commitment already owned by a strategic planner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct StandingProductionCommitment {
    producer: BuildingId,
    kind: UnitKind,
}

impl StandingProductionCommitment {
    /// Excludes one matching already-paid queue item from standing inventory.
    ///
    /// Repeating the same `(producer, kind)` commitment excludes that many
    /// matching queue occurrences in canonical queue order.
    pub(crate) const fn paid(producer: BuildingId, kind: UnitKind) -> Self {
        Self { producer, kind }
    }
}

/// External facts owned by planners rather than ordinary production.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StandingForceContext<'a> {
    excluded_units: &'a [UnitId],
    committed_production: &'a [StandingProductionCommitment],
    home: StandingGroundTarget,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Option<Orientation>,
    force_projection_targets: &'a [StandingGroundTarget],
    expansion_security: Option<ExpansionSecurityNeed>,
    minimum_residual_scrap: u32,
}

impl<'a> StandingForceContext<'a> {
    /// Creates a context with exact operation ownership and ground-routing facts.
    pub(crate) const fn new(
        excluded_units: &'a [UnitId],
        committed_production: &'a [StandingProductionCommitment],
    ) -> Self {
        Self {
            excluded_units,
            committed_production,
            home: StandingGroundTarget::Point(TilePos::new(0, 0)),
            public_map: None,
            orientation: None,
            force_projection_targets: &[],
            expansion_security: None,
            minimum_residual_scrap: 0,
        }
    }

    /// Adds the home defense location, public terrain, and useful ground work.
    pub(crate) const fn with_ground_routing(
        mut self,
        home: StandingGroundTarget,
        public_map: Option<&'a PublicMapBriefing>,
        force_projection_targets: &'a [StandingGroundTarget],
        orientation: Option<Orientation>,
    ) -> Self {
        self.home = home;
        self.public_map = public_map;
        self.force_projection_targets = force_projection_targets;
        self.orientation = orientation;
        self
    }

    /// Adds the location and absolute ordinary-strength target for an expansion.
    pub(crate) const fn with_expansion_security(
        mut self,
        target: StandingGroundTarget,
        target_strength: u64,
    ) -> Self {
        self.expansion_security = Some(ExpansionSecurityNeed {
            target,
            target_strength,
        });
        self
    }

    /// Preserves current capital required by an unmigrated residual policy.
    pub(crate) const fn with_minimum_residual_scrap(mut self, amount: u32) -> Self {
        self.minimum_residual_scrap = amount;
        self
    }
}

/// One location that an ordinary ground reinforcement can materially serve.
pub(crate) type StandingGroundTarget = StandingForceServiceKey;

#[derive(Debug, Clone, Copy)]
struct ExpansionSecurityNeed {
    target: StandingGroundTarget,
    target_strength: u64,
}

/// Why ordinary production currently wants one more unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StandingForceReason {
    /// Restore the difficulty's non-negotiable ordinary fighting screen.
    CoreRecovery,
    /// Supply the exact missing protection assessed for an expansion.
    ExpansionSecurity,
    /// Protect paid construction whose value is already exposed on the map.
    InvestedCapitalSecurity,
    /// Answer currently seen or honestly remembered hostile ground strength.
    GroundPressure,
    /// Answer currently seen or honestly remembered hostile aircraft.
    AirDefense,
    /// Add standoff capability against known fixed defenses.
    SiegePressure,
    /// Add mobile repair capacity for reachable wounded combatants.
    WoundedSupport,
    /// Grow a useful ordinary force while a reachable objective remains.
    ForceProjection,
}

/// One independently useful standing-force purchase or bounded accumulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StandingForceProposal {
    observed_at: Tick,
    ready_before: Tick,
    kind: UnitKind,
    service: StandingForceServiceKey,
    reason: StandingForceReason,
    specialty: Specialty,
    personality_emphasis: u8,
    case: ProposalCase,
    eligible_producers: Vec<BuildingId>,
    minimum_residual_scrap: u32,
    funding: StandingForceFunding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StandingForceFunding {
    Immediate,
    Accumulate {
        through: Tick,
        current_scrap: u32,
        forecast_scrap: u32,
    },
}

#[cfg(test)]
pub(crate) struct StandingForceFixture {
    pub(crate) observed_at: Tick,
    pub(crate) ready_before: Tick,
    pub(crate) kind: UnitKind,
    pub(crate) reason: StandingForceReason,
    pub(crate) specialty: Specialty,
    pub(crate) personality_emphasis: u8,
    pub(crate) case: ProposalCase,
    pub(crate) eligible_producers: Vec<BuildingId>,
}

impl StandingForceProposal {
    /// Stable unit-and-service identity for this standing-force opportunity.
    pub(crate) const fn key(&self) -> StandingForceKey {
        StandingForceKey {
            kind: self.kind,
            service: self.service,
        }
    }

    /// Unit kind requested by this opportunity.
    pub(crate) const fn key_kind(&self) -> UnitKind {
        self.key().kind
    }

    /// Cross-domain comparison bands derived from the demand's evidence.
    pub(crate) const fn case(&self) -> ProposalCase {
        self.case
    }

    /// Personality axis relevant to this exact standing-force alternative.
    #[cfg(test)]
    pub(crate) const fn specialty(&self) -> Specialty {
        self.specialty
    }

    /// Resolved positive emphasis for this exact alternative.
    pub(crate) const fn personality_emphasis(&self) -> u8 {
        self.personality_emphasis
    }

    /// Concrete need this request answers.
    #[cfg(test)]
    pub(in crate::bot) const fn reason(&self) -> StandingForceReason {
        self.reason
    }

    /// Observation tick on which this alternative was derived.
    pub(crate) const fn observed_at(&self) -> Tick {
        self.observed_at
    }

    /// Strict readiness deadline that every reported eligible lane can meet.
    pub(crate) const fn ready_before(&self) -> Tick {
        self.ready_before
    }

    /// Canonical completed producers that can satisfy the shallow request.
    pub(crate) fn eligible_producers(&self) -> &[BuildingId] {
        &self.eligible_producers
    }

    /// Current bank that must survive this immediate purchase.
    pub(crate) const fn minimum_residual_scrap(&self) -> u32 {
        self.minimum_residual_scrap
    }

    /// Capital-only bounded wait that competes in shared allocation without
    /// making its future provider an enqueue-now command.
    pub(crate) const fn accumulation(&self) -> Option<(Tick, u32, u32)> {
        match self.funding {
            StandingForceFunding::Immediate => None,
            StandingForceFunding::Accumulate {
                through,
                current_scrap,
                forecast_scrap,
            } => Some((through, current_scrap, forecast_scrap)),
        }
    }

    #[cfg(test)]
    pub(crate) const fn with_minimum_residual_scrap(mut self, amount: u32) -> Self {
        self.minimum_residual_scrap = amount;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_accumulation(mut self, through: Tick, current_scrap: u32) -> Self {
        let current_scrap = current_scrap.min(self.kind.stats().cost);
        self.funding = StandingForceFunding::Accumulate {
            through,
            current_scrap,
            forecast_scrap: self.kind.stats().cost.saturating_sub(current_scrap),
        };
        self
    }

    #[cfg(test)]
    pub(crate) fn fixture(fixture: StandingForceFixture) -> Self {
        let StandingForceFixture {
            observed_at,
            ready_before,
            kind,
            reason,
            specialty,
            personality_emphasis,
            case,
            eligible_producers,
        } = fixture;
        Self {
            observed_at,
            ready_before,
            kind,
            service: StandingForceServiceKey::point(TilePos::new(0, 0)),
            reason,
            specialty,
            personality_emphasis,
            case,
            eligible_producers,
            minimum_residual_scrap: 0,
            funding: StandingForceFunding::Immediate,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EvidenceStrength {
    current: u64,
    remembered: u64,
}

impl EvidenceStrength {
    fn add(&mut self, evidence: ContactEvidence, confidence: u16, strength: u64) {
        match evidence {
            ContactEvidence::Current => self.current = self.current.saturating_add(strength),
            ContactEvidence::Remembered => {
                let weighted = u128::from(strength) * u128::from(confidence) / 1_000;
                self.remembered = self
                    .remembered
                    .saturating_add(u64::try_from(weighted).unwrap_or(u64::MAX));
            }
        }
    }

    const fn total(self) -> u64 {
        self.current.saturating_add(self.remembered)
    }

    const fn strongest_evidence(self) -> Option<ContactEvidence> {
        if self.current > 0 {
            Some(ContactEvidence::Current)
        } else if self.remembered > 0 {
            Some(ContactEvidence::Remembered)
        } else {
            None
        }
    }

    fn add_summary(&mut self, other: Self) {
        self.current = self.current.saturating_add(other.current);
        self.remembered = self.remembered.saturating_add(other.remembered);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ThreatSummary {
    ground: EvidenceStrength,
    air: EvidenceStrength,
    defenses: EvidenceStrength,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Inventory {
    line_bodies: u32,
    line_strength: u64,
    anti_air_strength: u64,
    siege_strength: u64,
    tenders: u32,
}

impl Inventory {
    fn account(&mut self, kind: UnitKind, hp: u32) {
        self.anti_air_strength =
            self.anti_air_strength
                .saturating_add(combat_strength(kind, hp, Domain::Air));
        match kind.role() {
            Role::Sentinel | Role::Warden | Role::Breaker => {
                self.line_bodies = self.line_bodies.saturating_add(1);
                self.line_strength = self.line_strength.saturating_add(ground_strength(kind, hp));
            }
            Role::AntiAir | Role::AirAir | Role::Interceptor => {}
            Role::Lancer | Role::Bombard | Role::Avalanche => {
                self.siege_strength =
                    self.siege_strength
                        .saturating_add(combat_strength(kind, hp, Domain::Ground));
            }
            Role::Tender => self.tenders = self.tenders.saturating_add(1),
            Role::Harvester
            | Role::Scuttler
            | Role::AirGround
            | Role::Excavator
            | Role::Scout
            | Role::Bomber
            | Role::Skyhook
            | Role::Sapper => {}
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum InventoryOrigin {
    AirUnit(TilePos),
    AirProducer(BuildingId),
    GroundUnit(TilePos),
    GroundProducer(BuildingId),
}

impl InventoryOrigin {
    const fn domain(self) -> Domain {
        match self {
            Self::AirUnit(_) | Self::AirProducer(_) => Domain::Air,
            Self::GroundUnit(_) | Self::GroundProducer(_) => Domain::Ground,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct InventoryMember {
    kind: UnitKind,
    hp: u32,
    origin: InventoryOrigin,
}

#[derive(Debug, Clone)]
struct InventoryRoster {
    members: Vec<InventoryMember>,
}

impl InventoryRoster {
    fn from_observation(obs: &Observation, context: StandingForceContext<'_>) -> Self {
        let mut members = obs
            .my_units
            .iter()
            .filter(|unit| !context.excluded_units.contains(&unit.id))
            .map(|unit| InventoryMember {
                kind: unit.kind,
                hp: unit.hp,
                origin: if unit.kind.stats().domain == Domain::Air {
                    InventoryOrigin::AirUnit(unit.tile)
                } else {
                    InventoryOrigin::GroundUnit(unit.tile)
                },
            })
            .collect::<Vec<_>>();
        let mut unconsumed_paid = context.committed_production.iter().copied().fold(
            BTreeMap::new(),
            |mut counts, commitment| {
                let count = counts
                    .entry((commitment.producer, commitment.kind))
                    .or_insert(0_u32);
                *count = count.saturating_add(1);
                counts
            },
        );
        for (building_index, queue) in obs.my_queues.iter().enumerate() {
            let Some(producer) = obs.my_buildings.get(building_index) else {
                continue;
            };
            for &kind in queue {
                if let Some(remaining) = unconsumed_paid
                    .get_mut(&(producer.id, kind))
                    .filter(|remaining| **remaining > 0)
                {
                    *remaining -= 1;
                    continue;
                }
                members.push(InventoryMember {
                    kind,
                    hp: kind.stats().max_hp,
                    origin: if kind.stats().domain == Domain::Air {
                        InventoryOrigin::AirProducer(producer.id)
                    } else {
                        InventoryOrigin::GroundProducer(producer.id)
                    },
                });
            }
        }
        Self { members }
    }

    #[cfg(test)]
    fn all(&self) -> Inventory {
        let mut inventory = Inventory::default();
        for member in &self.members {
            inventory.account(member.kind, member.hp);
        }
        inventory
    }
}

#[derive(Debug, Default)]
struct ComponentInventory {
    ground: Option<Vec<ComponentInventoryMember>>,
    air: Option<Vec<ComponentInventoryMember>>,
}

#[derive(Debug)]
struct ComponentInventoryMember {
    kind: UnitKind,
    hp: u32,
    components: Vec<usize>,
}

impl ComponentInventory {
    fn serviceable(
        &mut self,
        domain: Domain,
        targets: &[StandingGroundTarget],
        roster: &InventoryRoster,
        routing: &mut ServiceRouting<'_>,
    ) -> Inventory {
        self.ensure(domain, roster, routing);
        let component_ids = routing.components_for_targets(domain, targets);
        let members = match domain {
            Domain::Ground => self.ground.as_ref(),
            Domain::Air => self.air.as_ref(),
        }
        .expect("the requested movement domain was indexed");
        let mut total = Inventory::default();
        for member in members {
            if member
                .components
                .iter()
                .any(|component| component_ids.binary_search(component).is_ok())
            {
                total.account(member.kind, member.hp);
            }
        }
        total
    }

    fn ensure(
        &mut self,
        domain: Domain,
        roster: &InventoryRoster,
        routing: &mut ServiceRouting<'_>,
    ) {
        let already_indexed = match domain {
            Domain::Ground => self.ground.is_some(),
            Domain::Air => self.air.is_some(),
        };
        if already_indexed {
            return;
        }

        let mut members = Vec::new();
        for member in &roster.members {
            if member.origin.domain() != domain {
                continue;
            }
            let components = routing.inventory_origin_components(*member);
            if components.is_empty() {
                continue;
            }
            members.push(ComponentInventoryMember {
                kind: member.kind,
                hp: member.hp,
                components,
            });
        }
        match domain {
            Domain::Ground => self.ground = Some(members),
            Domain::Air => self.air = Some(members),
        }
    }
}

#[derive(Debug, Clone)]
struct DemandCandidate {
    observed_at: Tick,
    reason: StandingForceReason,
    specialty: Specialty,
    case: ProposalCase,
    kind: UnitKind,
    service: StandingForceServiceKey,
    eligible_producers: Vec<BuildingId>,
    ready_before: Tick,
    unmet: u32,
    personality: u8,
    provider_value: u128,
    funding: StandingForceFunding,
}

#[derive(Debug, Clone, Copy)]
struct DemandBasis {
    reason: StandingForceReason,
    case: ProposalCase,
    unmet: u32,
}

/// Derives ranked, mutually exclusive current-bank alternatives for the
/// ordinary standing force.
///
/// The caller should retain the first alternative that survives shared
/// allocation. Forecast income may break ties among already-affordable
/// providers, but it never makes an unaffordable request legal. Persistent
/// operations keep ownership of partial strategic cohorts through exact unit
/// and paid-queue exclusions.
pub(crate) fn derive_standing_force_proposals(
    obs: &Observation,
    intelligence: &StrategicIntelligence,
    profile: &ResolvedProfile,
    tuning: DifficultyTuning,
    resources: &ResourceSnapshot,
    context: StandingForceContext<'_>,
) -> Vec<StandingForceProposal> {
    let roster = InventoryRoster::from_observation(obs, context);
    let threats = threat_summary(obs.tick, intelligence);
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
    let core_bodies = tuning.minimum_core_equivalents;
    let core_strength = sentinel_strength.saturating_mul(u64::from(core_bodies));
    let mut candidates = Vec::new();
    let mut routing = ServiceRouting::new(obs, context.public_map, context.orientation);
    let mut component_inventory = ComponentInventory::default();
    let home_targets = [context.home];
    let home_inventory =
        component_inventory.serviceable(Domain::Ground, &home_targets, &roster, &mut routing);

    if home_inventory.line_bodies < core_bodies || home_inventory.line_strength < core_strength {
        let body_shortfall = core_bodies.saturating_sub(home_inventory.line_bodies);
        let strength_shortfall = core_strength.saturating_sub(home_inventory.line_strength);
        candidates.extend(line_candidates(
            obs,
            resources,
            profile,
            &mut routing,
            &home_targets,
            DemandBasis {
                reason: StandingForceReason::CoreRecovery,
                case: ProposalCase {
                    urgency: Urgency::Pressing,
                    confidence: contact_confidence(threats.ground.strongest_evidence()),
                    value: StrategicValue::Decisive,
                    time_to_impact: TimeToImpact::Immediate,
                    safety: ExecutionSafety::Secure,
                },
                unmet: body_shortfall
                    .max(strength_equivalents(strength_shortfall, sentinel_strength)),
            },
            body_shortfall > 0,
        ));
    }

    for invested in demand_components(
        Domain::Ground,
        unfinished_building_demands(obs),
        &mut routing,
    ) {
        let invested_target = core_strength.saturating_add(invested.fixed);
        let invested_inventory = component_inventory.serviceable(
            Domain::Ground,
            &invested.targets,
            &roster,
            &mut routing,
        );
        if invested_target <= invested_inventory.line_strength {
            continue;
        }
        let unmet = strength_equivalents(
            invested_target.saturating_sub(invested_inventory.line_strength),
            sentinel_strength,
        );
        candidates.extend(line_candidates(
            obs,
            resources,
            profile,
            &mut routing,
            &invested.targets,
            DemandBasis {
                reason: StandingForceReason::InvestedCapitalSecurity,
                case: ProposalCase {
                    urgency: Urgency::Timely,
                    confidence: Confidence::Current,
                    value: StrategicValue::Material,
                    time_to_impact: TimeToImpact::Near,
                    safety: ExecutionSafety::Managed,
                },
                unmet,
            },
            true,
        ));
    }
    if let Some(expansion) = context.expansion_security {
        let expansion_targets = [expansion.target];
        let expansion_inventory = component_inventory.serviceable(
            Domain::Ground,
            &expansion_targets,
            &roster,
            &mut routing,
        );
        if expansion.target_strength > expansion_inventory.line_strength {
            let unmet = strength_equivalents(
                expansion
                    .target_strength
                    .saturating_sub(expansion_inventory.line_strength),
                sentinel_strength,
            );
            candidates.extend(line_candidates(
                obs,
                resources,
                profile,
                &mut routing,
                &expansion_targets,
                DemandBasis {
                    reason: StandingForceReason::ExpansionSecurity,
                    case: ProposalCase {
                        urgency: Urgency::Timely,
                        confidence: Confidence::Supported,
                        value: StrategicValue::Material,
                        time_to_impact: TimeToImpact::Near,
                        safety: ExecutionSafety::Managed,
                    },
                    unmet,
                },
                false,
            ));
        }
    }

    for ground in demand_components(
        Domain::Ground,
        hostile_ground_demands(obs.tick, intelligence),
        &mut routing,
    ) {
        let hostile_ground = ground.observed.total();
        let ground_inventory =
            component_inventory.serviceable(Domain::Ground, &ground.targets, &roster, &mut routing);
        let perceived_line_strength = tuning.underestimate_own(ground_inventory.line_strength);
        if hostile_ground <= perceived_line_strength {
            continue;
        }
        let unmet = strength_equivalents(
            hostile_ground.saturating_sub(perceived_line_strength),
            sentinel_strength,
        );
        candidates.extend(line_candidates(
            obs,
            resources,
            profile,
            &mut routing,
            &ground.targets,
            DemandBasis {
                reason: StandingForceReason::GroundPressure,
                case: threat_case(
                    ground.observed.strongest_evidence(),
                    StrategicValue::Material,
                ),
                unmet,
            },
            false,
        ));
    }

    let air_demands = hostile_air_demands(obs.tick, intelligence);
    if !air_demands.is_empty() {
        let home_air_components = routing.components_for_targets(Domain::Air, &home_targets);
        for air in demand_components(Domain::Air, air_demands, &mut routing) {
            if !air
                .route_components
                .iter()
                .any(|component| home_air_components.binary_search(component).is_ok())
            {
                continue;
            }
            let hostile_air = air.observed.total();
            let home_anti_air_strength = home_inventory.anti_air_strength.saturating_add(
                component_inventory
                    .serviceable(Domain::Air, &air.targets, &roster, &mut routing)
                    .anti_air_strength,
            );
            if hostile_air <= home_anti_air_strength {
                continue;
            }
            let baseline = combat_strength(
                Role::AntiAir.unit_for(obs.faction),
                Role::AntiAir.unit_for(obs.faction).stats().max_hp,
                Domain::Air,
            )
            .max(1);
            let missing = hostile_air.saturating_sub(home_anti_air_strength);
            let unmet = strength_equivalents(missing, baseline);
            candidates.extend(air_defense_candidates(
                obs,
                resources,
                profile,
                &mut routing,
                &home_targets,
                &air.targets,
                DemandBasis {
                    reason: StandingForceReason::AirDefense,
                    case: threat_case(air.observed.strongest_evidence(), StrategicValue::Decisive),
                    unmet,
                },
            ));
        }
    }

    for defense in demand_components(
        Domain::Ground,
        hostile_defense_demands(obs.tick, intelligence),
        &mut routing,
    ) {
        let hostile_defenses = defense.observed.total();
        let defense_inventory = component_inventory.serviceable(
            Domain::Ground,
            &defense.targets,
            &roster,
            &mut routing,
        );
        if hostile_defenses <= defense_inventory.siege_strength {
            continue;
        }
        let missing = hostile_defenses.saturating_sub(defense_inventory.siege_strength);
        let unmet = strength_equivalents(missing, full_ground_strength(UnitKind::Lancer).max(1));
        candidates.extend(siege_candidates(
            obs,
            resources,
            profile,
            &mut routing,
            &defense.targets,
            DemandBasis {
                reason: StandingForceReason::SiegePressure,
                case: threat_case(
                    defense.observed.strongest_evidence(),
                    StrategicValue::Material,
                ),
                unmet,
            },
        ));
    }

    let support_work_per_tender = UnitKind::Tender.stats().max_hp.saturating_mul(2);
    for (unmet_support, wounded_targets) in wounded_support_needs(
        obs,
        context.excluded_units,
        resources,
        &roster,
        &mut component_inventory,
        &mut routing,
        support_work_per_tender,
    ) {
        candidates.extend(candidates_for_kinds(
            obs,
            resources,
            profile,
            &mut routing,
            CandidateSpec {
                basis: DemandBasis {
                    reason: StandingForceReason::WoundedSupport,
                    case: ProposalCase {
                        urgency: Urgency::Timely,
                        confidence: Confidence::Current,
                        value: StrategicValue::Material,
                        time_to_impact: TimeToImpact::Near,
                        safety: ExecutionSafety::Secure,
                    },
                    unmet: unmet_support,
                },
                kinds: &[UnitKind::Tender],
                targets: &wounded_targets,
            },
            |_| Specialty::Support,
            |_| 1,
        ));
    }

    let projection_demands = context
        .force_projection_targets
        .iter()
        .copied()
        .map(|target| LocatedDemand::fixed(target, 0))
        .collect();
    for projection in demand_components(Domain::Ground, projection_demands, &mut routing) {
        let projection_inventory = component_inventory.serviceable(
            Domain::Ground,
            &projection.targets,
            &roster,
            &mut routing,
        );
        candidates.extend(force_projection_candidates(
            obs,
            resources,
            profile,
            projection_inventory,
            &mut routing,
            &projection.targets,
        ));
    }

    apply_bounded_provider_accumulation(obs, resources, context, &mut candidates);

    let current_scrap = resources.current_scrap().amount();
    candidates.retain(|candidate| {
        current_scrap.saturating_sub(context.minimum_residual_scrap) >= candidate.kind.stats().cost
            || matches!(candidate.funding, StandingForceFunding::Accumulate { .. })
    });

    let mut best_by_service =
        BTreeMap::<(UnitKind, StandingForceServiceKey), DemandCandidate>::new();
    for candidate in candidates {
        match best_by_service.entry((candidate.kind, candidate.service)) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                if candidate_rank(&candidate) > candidate_rank(entry.get()) {
                    entry.insert(candidate);
                }
            }
        }
    }
    let mut alternatives = best_by_service.into_values().collect::<Vec<_>>();
    alternatives.sort_unstable_by(|left, right| {
        candidate_rank(right)
            .cmp(&candidate_rank(left))
            .then_with(|| demand_candidate_key(left).cmp(&demand_candidate_key(right)))
    });
    alternatives
        .into_iter()
        .map(|candidate| {
            StandingForceProposal::from_candidate(candidate, context.minimum_residual_scrap)
        })
        .collect()
}

fn force_projection_candidates(
    obs: &Observation,
    resources: &ResourceSnapshot,
    profile: &ResolvedProfile,
    inventory: Inventory,
    routing: &mut ServiceRouting<'_>,
    targets: &[StandingGroundTarget],
) -> impl Iterator<Item = DemandCandidate> {
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel).max(1);
    let lancer_strength = full_ground_strength(UnitKind::Lancer).max(1);
    let affordable_depth = resources
        .current_scrap()
        .amount()
        .checked_div(UnitKind::Sentinel.stats().cost.max(1))
        .unwrap_or(0)
        .max(1);
    let case = ProposalCase {
        urgency: Urgency::Developmental,
        confidence: Confidence::Prior,
        value: StrategicValue::Incremental,
        time_to_impact: TimeToImpact::Patient,
        safety: ExecutionSafety::Managed,
    };
    let (line_weight, siege_weight) = match profile.stance {
        crate::scenario::BotStance::Turtle => (120, 20),
        crate::scenario::BotStance::Balanced => (100, 30),
        crate::scenario::BotStance::Aggressive => (80, 50),
    };
    let line_priority = diminishing_force_priority(
        inventory.line_strength,
        sentinel_strength,
        line_weight,
        profile.traits.fortification,
    );
    let siege_priority = diminishing_force_priority(
        inventory.siege_strength,
        lancer_strength,
        siege_weight,
        profile.traits.siege,
    );

    let mut candidates = Vec::with_capacity(2);
    for mut candidate in line_candidates(
        obs,
        resources,
        profile,
        routing,
        targets,
        DemandBasis {
            reason: StandingForceReason::ForceProjection,
            case,
            unmet: affordable_depth,
        },
        false,
    ) {
        candidate.unmet = line_priority;
        candidates.push(candidate);
    }
    for mut candidate in siege_candidates(
        obs,
        resources,
        profile,
        routing,
        targets,
        DemandBasis {
            reason: StandingForceReason::ForceProjection,
            case,
            unmet: affordable_depth,
        },
    ) {
        candidate.unmet = siege_priority;
        candidates.push(candidate);
    }
    candidates.into_iter()
}

fn diminishing_force_priority(
    existing_strength: u64,
    baseline_strength: u64,
    base_weight: u8,
    personality: u8,
) -> u32 {
    let numerator = u128::from(base_weight.saturating_add(personality))
        .saturating_mul(u128::from(baseline_strength))
        .saturating_mul(1_000);
    let denominator = u128::from(existing_strength.saturating_add(baseline_strength));
    u32::try_from(numerator / denominator).unwrap_or(u32::MAX)
}

impl StandingForceProposal {
    fn from_candidate(candidate: DemandCandidate, minimum_residual_scrap: u32) -> Self {
        Self {
            observed_at: candidate.observed_at,
            ready_before: candidate.ready_before,
            kind: candidate.kind,
            service: candidate.service,
            reason: candidate.reason,
            specialty: candidate.specialty,
            personality_emphasis: candidate.personality,
            case: candidate.case,
            eligible_producers: candidate.eligible_producers,
            minimum_residual_scrap,
            funding: candidate.funding,
        }
    }
}

fn threat_summary(now: Tick, intelligence: &StrategicIntelligence) -> ThreatSummary {
    let mut summary = ThreatSummary::default();
    for contact in intelligence.units() {
        if !contact.kind.stats().can_fight() {
            continue;
        }
        let confidence = contact.confidence_at(now);
        match contact.kind.stats().domain {
            Domain::Ground => summary.ground.add(
                contact.evidence,
                confidence,
                ground_strength(contact.kind, contact.hp),
            ),
            Domain::Air => summary.air.add(
                contact.evidence,
                confidence,
                combat_strength(contact.kind, contact.hp, Domain::Ground).max(combat_strength(
                    contact.kind,
                    contact.hp,
                    Domain::Air,
                )),
            ),
        }
    }
    for contact in intelligence.buildings() {
        if contact.built && contact.kind.tier_stats(contact.tier).can_fight() {
            let stats = contact.kind.tier_stats(contact.tier);
            let strength = u64::from(contact.hp).saturating_mul(
                stats
                    .weapons
                    .iter()
                    .filter(|weapon| weapon.targets.covers(Domain::Ground))
                    .map(weapon_burst_dps100)
                    .sum(),
            );
            summary
                .defenses
                .add(contact.evidence, contact.confidence_at(now), strength);
        }
    }
    summary
}

fn combat_strength(kind: UnitKind, hp: u32, target: Domain) -> u64 {
    u64::from(hp).saturating_mul(
        kind.stats()
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.covers(target))
            .map(weapon_burst_dps100)
            .sum(),
    )
}

fn strength_equivalents(missing: u64, sentinel_strength: u64) -> u32 {
    u32::try_from(missing.div_ceil(sentinel_strength)).unwrap_or(u32::MAX)
}

fn line_candidates(
    obs: &Observation,
    resources: &ResourceSnapshot,
    profile: &ResolvedProfile,
    routing: &mut ServiceRouting<'_>,
    targets: &[StandingGroundTarget],
    basis: DemandBasis,
    needs_screen_body: bool,
) -> Vec<DemandCandidate> {
    let kinds = &[UnitKind::Sentinel, UnitKind::Warden, UnitKind::Breaker];
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
    let missing_strength = sentinel_strength.saturating_mul(u64::from(basis.unmet));
    candidates_for_kinds(
        obs,
        resources,
        profile,
        routing,
        CandidateSpec {
            basis,
            kinds,
            targets,
        },
        |_| Specialty::Fortification,
        |kind| {
            let full = full_ground_strength(kind);
            let useful = useful_provider_capacity(full, missing_strength, basis.reason);
            let strength_value = u128::from(useful)
                .saturating_mul(1_000)
                .checked_div(u128::from(kind.stats().cost.max(1)))
                .unwrap_or(0);
            if needs_screen_body {
                strength_value
                    .saturating_add(1_000_000_u128 / u128::from(kind.stats().train_ticks.max(1)))
            } else {
                strength_value
            }
        },
    )
}

fn air_defense_candidates(
    obs: &Observation,
    resources: &ResourceSnapshot,
    profile: &ResolvedProfile,
    routing: &mut ServiceRouting<'_>,
    ground_targets: &[StandingGroundTarget],
    air_targets: &[StandingGroundTarget],
    basis: DemandBasis,
) -> Vec<DemandCandidate> {
    let ground = Role::AntiAir.unit_for(obs.faction);
    let fighter = Role::AirAir.unit_for(obs.faction);
    let interceptor = Role::Interceptor.unit_for(obs.faction);
    let missing = combat_strength(ground, ground.stats().max_hp, Domain::Air)
        .max(1)
        .saturating_mul(u64::from(basis.unmet));
    let specialty = |kind: UnitKind| {
        if kind.stats().domain == Domain::Air {
            Specialty::Air
        } else {
            Specialty::Fortification
        }
    };
    let usefulness = |kind: UnitKind| {
        let useful = combat_strength(kind, kind.stats().max_hp, Domain::Air).min(missing);
        let efficiency =
            u128::from(useful).saturating_mul(1_000) / u128::from(kind.stats().cost.max(1));
        let axis = if kind.stats().domain == Domain::Air {
            profile.traits.air
        } else {
            profile.traits.fortification
        };
        efficiency
            .saturating_mul(100)
            .saturating_add(u128::from(axis).saturating_mul(10_000))
            .saturating_add(u128::from(kind.stats().max_hp))
    };
    let mut candidates = candidates_for_kinds(
        obs,
        resources,
        profile,
        routing,
        CandidateSpec {
            basis,
            kinds: &[ground],
            targets: ground_targets,
        },
        specialty,
        usefulness,
    );
    candidates.extend(candidates_for_kinds(
        obs,
        resources,
        profile,
        routing,
        CandidateSpec {
            basis,
            kinds: &[fighter, interceptor],
            targets: air_targets,
        },
        specialty,
        usefulness,
    ));
    candidates
}

fn siege_candidates(
    obs: &Observation,
    resources: &ResourceSnapshot,
    profile: &ResolvedProfile,
    routing: &mut ServiceRouting<'_>,
    targets: &[StandingGroundTarget],
    basis: DemandBasis,
) -> Vec<DemandCandidate> {
    let missing = full_ground_strength(UnitKind::Lancer)
        .max(1)
        .saturating_mul(u64::from(basis.unmet));
    candidates_for_kinds(
        obs,
        resources,
        profile,
        routing,
        CandidateSpec {
            basis,
            kinds: &[UnitKind::Lancer, UnitKind::Bombard, UnitKind::Avalanche],
            targets,
        },
        |_| Specialty::Siege,
        |kind| siege_provider_value(kind, missing, profile.traits.siege, basis.reason),
    )
}

struct CandidateSpec<'a> {
    basis: DemandBasis,
    kinds: &'a [UnitKind],
    targets: &'a [StandingGroundTarget],
}

fn candidates_for_kinds(
    obs: &Observation,
    resources: &ResourceSnapshot,
    profile: &ResolvedProfile,
    routing: &mut ServiceRouting<'_>,
    spec: CandidateSpec<'_>,
    specialty: impl Fn(UnitKind) -> Specialty,
    usefulness: impl Fn(UnitKind) -> u128,
) -> Vec<DemandCandidate> {
    let CandidateSpec {
        basis,
        kinds,
        targets,
    } = spec;
    let Some(service) = targets.iter().copied().min() else {
        return Vec::new();
    };
    let current_scrap = resources.current_scrap().amount();
    let mut candidates = Vec::new();
    for kind in kinds.iter().copied() {
        let Some((eligible_producers, ready_before)) =
            eligible_producers(obs, resources, kind, routing, targets)
        else {
            continue;
        };
        let forecast = resources.forecast().income_through(ready_before).amount();
        let cushion = current_scrap
            .saturating_sub(kind.stats().cost)
            .saturating_add(forecast);
        let specialty = specialty(kind);
        candidates.push(DemandCandidate {
            observed_at: obs.tick,
            reason: basis.reason,
            specialty,
            case: basis.case,
            kind,
            service,
            eligible_producers,
            ready_before,
            unmet: basis.unmet,
            personality: profile.traits.get(specialty),
            provider_value: usefulness(kind)
                .saturating_mul(2)
                .saturating_add(u128::from(cushion >= kind.stats().cost)),
            funding: StandingForceFunding::Immediate,
        });
    }
    candidates
}

fn eligible_producers(
    _obs: &Observation,
    resources: &ResourceSnapshot,
    kind: UnitKind,
    routing: &mut ServiceRouting<'_>,
    targets: &[StandingGroundTarget],
) -> Option<(Vec<BuildingId>, Tick)> {
    let mut choices = Vec::new();
    for lane in resources.producers() {
        let Some(timing) = lane.production_timing(&[kind]) else {
            continue;
        };
        if !matches!(
            timing.current_egress,
            ProducerEgress::NotRequired | ProducerEgress::Open
        ) {
            continue;
        }
        if !routing.producer_reaches_any(lane.producer, kind, targets) {
            continue;
        }
        let Some(ready_before) = timing.no_block_latest_ready_tick.checked_add(1) else {
            continue;
        };
        choices.push((lane.producer, ready_before));
    }
    let ready_before = choices.iter().map(|(_, ready)| *ready).max()?;
    let mut producers = choices
        .into_iter()
        .map(|(producer, _)| producer)
        .collect::<Vec<_>>();
    producers.sort_unstable();
    producers.dedup();
    Some((producers, ready_before))
}

fn siege_provider_value(
    kind: UnitKind,
    missing: u64,
    siege_emphasis: u8,
    reason: StandingForceReason,
) -> u128 {
    let full = full_ground_strength(kind).max(1);
    let useful = useful_provider_capacity(full, missing, reason);
    let reach = kind
        .stats()
        .max_range_vs(Domain::Ground)
        .map_or(0_u128, |range| u128::from(range.to_bits().unsigned_abs()));
    let baseline_reach = UnitKind::Lancer
        .stats()
        .max_range_vs(Domain::Ground)
        .map_or(1_u128, |range| {
            u128::from(range.to_bits().unsigned_abs()).max(1)
        });
    let base = u128::from(useful).saturating_mul(100);
    let reach_bonus = u128::from(useful)
        .saturating_mul(reach)
        .saturating_mul(u128::from(50_u8.saturating_add(siege_emphasis)))
        .saturating_mul(u128::from(useful))
        / baseline_reach.saturating_mul(u128::from(full)).max(1);
    base.saturating_add(reach_bonus)
}

fn useful_provider_capacity(full: u64, missing: u64, reason: StandingForceReason) -> u64 {
    if reason == StandingForceReason::ForceProjection {
        return full;
    }

    let covered = full.min(missing);
    // Excess durability still has value, but never outweighs more than half
    // the explicit shortfall; this favors a coherent hull without pricing an
    // enormous unit as though all of its overkill served a small need.
    let durable_headroom = full.saturating_sub(covered).min(missing) / 2;
    covered.saturating_add(durable_headroom)
}

/// Adds a capital-only wait beside one need's affordable fallback when
/// completed income can reach a strictly better completed-producer option
/// within the two alternatives' exact production horizons. Shared allocation
/// decides whether the wait or fallback survives alongside other work.
fn apply_bounded_provider_accumulation(
    obs: &Observation,
    resources: &ResourceSnapshot,
    context: StandingForceContext<'_>,
    candidates: &mut [DemandCandidate],
) {
    let current_scrap = resources.current_scrap().amount();
    let affordable = |candidate: &DemandCandidate| {
        current_scrap.saturating_sub(context.minimum_residual_scrap) >= candidate.kind.stats().cost
    };
    let held_current = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| affordable(candidate))
        .filter(|(_, candidate)| {
            !candidates.iter().any(|other| {
                other.reason == candidate.reason
                    && other.service == candidate.service
                    && affordable(other)
                    && candidate_rank(other) > candidate_rank(candidate)
            })
        })
        .filter(|(_, current_best)| {
            current_best.reason != StandingForceReason::CoreRecovery
                && current_best.case.urgency != Urgency::Pressing
        })
        .filter(|(_, current_best)| {
            candidates
                .iter()
                .filter(|candidate| {
                    candidate.reason == current_best.reason
                        && candidate.service == current_best.service
                })
                .filter(|candidate| !affordable(candidate))
                .filter(|candidate| candidate.provider_value > current_best.provider_value)
                .filter(|candidate| candidate_rank(candidate) > candidate_rank(current_best))
                .any(|candidate| {
                    let fallback_delay = current_best.ready_before.saturating_sub(obs.tick);
                    let accumulation_deadline =
                        candidate.ready_before.saturating_add(fallback_delay);
                    current_scrap
                        .saturating_add(
                            resources
                                .forecast()
                                .income_through(accumulation_deadline)
                                .amount(),
                        )
                        .saturating_sub(context.minimum_residual_scrap)
                        >= candidate.kind.stats().cost
                })
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    for current_index in held_current {
        let current_best = &candidates[current_index];
        let selected = candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                candidate.reason == current_best.reason && candidate.service == current_best.service
            })
            .filter(|(_, candidate)| !affordable(candidate))
            .filter(|(_, candidate)| candidate.provider_value > current_best.provider_value)
            .filter(|(_, candidate)| candidate_rank(candidate) > candidate_rank(current_best))
            .filter_map(|(index, candidate)| {
                let fallback_delay = current_best.ready_before.saturating_sub(obs.tick);
                let through = candidate.ready_before.saturating_add(fallback_delay);
                (current_scrap
                    .saturating_add(resources.forecast().income_through(through).amount())
                    .saturating_sub(context.minimum_residual_scrap)
                    >= candidate.kind.stats().cost)
                    .then_some((index, through))
            })
            .min_by(|(left, left_through), (right, right_through)| {
                candidates[*left]
                    .kind
                    .stats()
                    .cost
                    .cmp(&candidates[*right].kind.stats().cost)
                    .then_with(|| left_through.cmp(right_through))
                    .then_with(|| left.cmp(right))
            });
        if let Some((index, through)) = selected {
            let cost = candidates[index].kind.stats().cost;
            let current_scrap = current_scrap
                .saturating_sub(context.minimum_residual_scrap)
                .min(cost);
            candidates[index].funding = StandingForceFunding::Accumulate {
                through,
                current_scrap,
                forecast_scrap: cost.saturating_sub(current_scrap),
            };
        }
    }
}

#[derive(Debug, Default)]
struct RouteComponentIndex {
    representatives: Vec<TilePos>,
    tiles: BTreeMap<TilePos, Option<usize>>,
}

impl RouteComponentIndex {
    fn component(&mut self, routes: &mut RouteProjection<'_>, tile: TilePos) -> Option<usize> {
        if let Some(component) = self.tiles.get(&tile) {
            return *component;
        }
        if !routes.reaches(tile, tile) {
            self.tiles.insert(tile, None);
            return None;
        }
        for (component, representative) in self.representatives.iter().copied().enumerate() {
            if routes.reaches(tile, representative) {
                self.tiles.insert(tile, Some(component));
                return Some(component);
            }
        }
        let component = self.representatives.len();
        self.representatives.push(tile);
        self.tiles.insert(tile, Some(component));
        Some(component)
    }
}

struct ServiceRouting<'a> {
    obs: &'a Observation,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Option<Orientation>,
    ground_routes: RouteProjection<'a>,
    air_routes: Option<RouteProjection<'a>>,
    ground_components: RouteComponentIndex,
    air_components: RouteComponentIndex,
    ground_target_components: BTreeMap<StandingGroundTarget, Vec<usize>>,
    air_target_components: BTreeMap<StandingGroundTarget, Vec<usize>>,
    ground_producer_components: BTreeMap<BuildingId, Option<usize>>,
    air_producer_components: BTreeMap<BuildingId, Option<usize>>,
}

impl<'a> ServiceRouting<'a> {
    fn new(
        obs: &'a Observation,
        public_map: Option<&'a PublicMapBriefing>,
        orientation: Option<Orientation>,
    ) -> Self {
        Self {
            obs,
            public_map,
            orientation,
            ground_routes: service_route_projection(obs, Domain::Ground, public_map, orientation),
            air_routes: None,
            ground_components: RouteComponentIndex::default(),
            air_components: RouteComponentIndex::default(),
            ground_target_components: BTreeMap::new(),
            air_target_components: BTreeMap::new(),
            ground_producer_components: BTreeMap::new(),
            air_producer_components: BTreeMap::new(),
        }
    }

    fn inventory_origin_components(&mut self, member: InventoryMember) -> Vec<usize> {
        match member.origin {
            InventoryOrigin::AirUnit(origin) => self.origin_components(Domain::Air, origin),
            InventoryOrigin::AirProducer(producer) => self
                .producer_component(producer, member.kind)
                .into_iter()
                .collect(),
            InventoryOrigin::GroundUnit(origin) => self.origin_components(Domain::Ground, origin),
            InventoryOrigin::GroundProducer(producer) => self
                .producer_component(producer, member.kind)
                .into_iter()
                .collect(),
        }
    }

    fn producer_reaches_any(
        &mut self,
        producer: BuildingId,
        kind: UnitKind,
        targets: &[StandingGroundTarget],
    ) -> bool {
        let Some(producer_component) = self.producer_component(producer, kind) else {
            return false;
        };
        self.components_for_targets(kind.stats().domain, targets)
            .binary_search(&producer_component)
            .is_ok()
    }

    fn producer_component(&mut self, producer: BuildingId, kind: UnitKind) -> Option<usize> {
        let domain = kind.stats().domain;
        let cached = match domain {
            Domain::Ground => self.ground_producer_components.get(&producer),
            Domain::Air => self.air_producer_components.get(&producer),
        };
        if let Some(component) = cached {
            return *component;
        }
        let building = self
            .obs
            .my_buildings
            .iter()
            .find(|building| building.id == producer && building.built && building.hp > 0)?;
        let origin = match domain {
            Domain::Ground => {
                production_spawn_doorstep(self.obs, building, self.public_map, self.orientation)?
            }
            Domain::Air => air_production_spawn_tile(building, self.orientation),
        };
        let component = self.component(domain, origin);
        match domain {
            Domain::Ground => {
                self.ground_producer_components.insert(producer, component);
            }
            Domain::Air => {
                self.air_producer_components.insert(producer, component);
            }
        }
        component
    }

    fn components_for_targets(
        &mut self,
        domain: Domain,
        targets: &[StandingGroundTarget],
    ) -> Vec<usize> {
        let mut components = Vec::new();
        for target in targets {
            components.extend(self.components_for_target(domain, *target));
        }
        components.sort_unstable();
        components.dedup();
        components
    }

    fn components_for_target(
        &mut self,
        domain: Domain,
        target: StandingGroundTarget,
    ) -> Vec<usize> {
        let cached = match domain {
            Domain::Ground => self.ground_target_components.get(&target),
            Domain::Air => self.air_target_components.get(&target),
        };
        if let Some(components) = cached {
            return components.clone();
        }
        let goals = match (domain, target) {
            (_, StandingGroundTarget::Point(tile)) => vec![tile],
            (Domain::Ground, StandingGroundTarget::Footprint { anchor, size }) => {
                crate::tick::rect_adjacent_tiles(anchor, size).collect()
            }
            (Domain::Air, StandingGroundTarget::Footprint { anchor, size }) => (0..size.1)
                .flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
                .collect(),
        };
        let mut components = goals
            .into_iter()
            .filter_map(|goal| self.component(domain, goal))
            .collect::<Vec<_>>();
        components.sort_unstable();
        components.dedup();
        match domain {
            Domain::Ground => {
                self.ground_target_components
                    .insert(target, components.clone());
            }
            Domain::Air => {
                self.air_target_components
                    .insert(target, components.clone());
            }
        }
        components
    }

    fn origin_components(&mut self, domain: Domain, origin: TilePos) -> Vec<usize> {
        if let Some(component) = self.component(domain, origin) {
            return vec![component];
        }
        let mut components = [(1, 0), (-1, 0), (0, 1), (0, -1)]
            .into_iter()
            .filter_map(|(dx, dy)| self.component(domain, origin.offset(dx, dy)))
            .collect::<Vec<_>>();
        components.sort_unstable();
        components.dedup();
        components
    }

    fn component(&mut self, domain: Domain, tile: TilePos) -> Option<usize> {
        match domain {
            Domain::Ground => self
                .ground_components
                .component(&mut self.ground_routes, tile),
            Domain::Air => {
                if self.air_routes.is_none() {
                    self.air_routes = Some(service_route_projection(
                        self.obs,
                        Domain::Air,
                        self.public_map,
                        self.orientation,
                    ));
                }
                self.air_components.component(
                    self.air_routes
                        .as_mut()
                        .expect("air projection was initialized"),
                    tile,
                )
            }
        }
    }
}

fn service_route_projection<'a>(
    obs: &'a Observation,
    domain: Domain,
    public_map: Option<&'a PublicMapBriefing>,
    orientation: Option<Orientation>,
) -> RouteProjection<'a> {
    match (public_map, orientation) {
        (Some(briefing), Some(orientation)) => {
            RouteProjection::with_public_terrain_and_orientation(obs, domain, briefing, orientation)
        }
        (Some(briefing), None) => RouteProjection::with_public_terrain(obs, domain, briefing),
        (None, Some(orientation)) => RouteProjection::with_orientation(obs, domain, orientation),
        (None, None) => RouteProjection::new(obs, domain),
    }
}

fn air_production_spawn_tile(
    producer: &super::observation::BuildingObs,
    orientation: Option<Orientation>,
) -> TilePos {
    let size = producer.kind.tier_stats(producer.tier).size;
    let world_anchor = orientation.map_or(producer.anchor, |orientation| {
        orientation.anchor(producer.anchor, size)
    });
    let world_spawn = world_anchor.offset(size.0 / 2, size.1 / 2);
    orientation.map_or(world_spawn, |orientation| orientation.tile(world_spawn))
}

#[derive(Debug, Clone, Copy)]
struct LocatedDemand {
    target: StandingGroundTarget,
    fixed: u64,
    observed: Option<(ContactEvidence, u16, u64)>,
}

impl LocatedDemand {
    const fn fixed(target: StandingGroundTarget, amount: u64) -> Self {
        Self {
            target,
            fixed: amount,
            observed: None,
        }
    }

    const fn observed(
        target: StandingGroundTarget,
        evidence: ContactEvidence,
        confidence: u16,
        strength: u64,
    ) -> Self {
        Self {
            target,
            fixed: 0,
            observed: Some((evidence, confidence, strength)),
        }
    }
}

#[derive(Debug, Clone)]
struct DemandComponent {
    route_components: Vec<usize>,
    targets: Vec<StandingGroundTarget>,
    fixed: u64,
    observed: EvidenceStrength,
}

impl DemandComponent {
    fn from_located(route_components: Vec<usize>, demand: LocatedDemand) -> Self {
        let mut component = Self {
            route_components,
            targets: vec![demand.target],
            fixed: demand.fixed,
            observed: EvidenceStrength::default(),
        };
        component.add_observed(demand.observed);
        component
    }

    fn add_located(&mut self, demand: LocatedDemand) {
        self.targets.push(demand.target);
        self.fixed = self.fixed.saturating_add(demand.fixed);
        self.add_observed(demand.observed);
    }

    fn absorb(&mut self, mut other: Self) {
        self.route_components.append(&mut other.route_components);
        self.targets.append(&mut other.targets);
        self.fixed = self.fixed.saturating_add(other.fixed);
        self.observed.add_summary(other.observed);
    }

    fn normalize(&mut self) {
        self.route_components.sort_unstable();
        self.route_components.dedup();
        self.targets.sort_unstable();
        self.targets.dedup();
    }

    fn add_observed(&mut self, observed: Option<(ContactEvidence, u16, u64)>) {
        if let Some((evidence, confidence, strength)) = observed {
            self.observed.add(evidence, confidence, strength);
        }
    }
}

fn demand_components(
    domain: Domain,
    mut demands: Vec<LocatedDemand>,
    routing: &mut ServiceRouting<'_>,
) -> Vec<DemandComponent> {
    demands.sort_unstable_by_key(|demand| demand.target);
    let mut components = Vec::<DemandComponent>::new();
    for demand in demands {
        let route_components = routing.components_for_target(domain, demand.target);
        if route_components.is_empty() {
            continue;
        }
        let matches = components
            .iter()
            .enumerate()
            .filter_map(|(index, component)| {
                component
                    .route_components
                    .iter()
                    .any(|existing| route_components.binary_search(existing).is_ok())
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let Some(&first) = matches.first() else {
            components.push(DemandComponent::from_located(route_components, demand));
            continue;
        };
        components[first].route_components.extend(route_components);
        components[first].add_located(demand);
        for &index in matches.iter().skip(1).rev() {
            let other = components.remove(index);
            components[first].absorb(other);
        }
        components[first].normalize();
    }
    for component in &mut components {
        component.normalize();
    }
    components
}

fn unfinished_building_demands(obs: &Observation) -> Vec<LocatedDemand> {
    obs.my_buildings
        .iter()
        .filter(|building| building.hp > 0 && !building.built)
        .filter_map(|building| {
            let cost = building.kind.base_stats().construction?.cost;
            Some(LocatedDemand::fixed(
                StandingGroundTarget::footprint(
                    building.anchor,
                    building.kind.tier_stats(building.tier).size,
                ),
                full_ground_strength(UnitKind::Sentinel)
                    .saturating_mul(u64::from(cost.div_ceil(UnitKind::Sentinel.stats().cost))),
            ))
        })
        .collect()
}

fn hostile_ground_demands(now: Tick, intelligence: &StrategicIntelligence) -> Vec<LocatedDemand> {
    intelligence
        .units()
        .iter()
        .filter(|contact| {
            contact.hp > 0
                && contact.confidence_at(now) > 0
                && contact.kind.stats().domain == Domain::Ground
                && contact.kind.stats().can_fight()
        })
        .map(|contact| {
            LocatedDemand::observed(
                StandingGroundTarget::point(contact.tile),
                contact.evidence,
                contact.confidence_at(now),
                ground_strength(contact.kind, contact.hp),
            )
        })
        .collect()
}

fn hostile_air_demands(now: Tick, intelligence: &StrategicIntelligence) -> Vec<LocatedDemand> {
    intelligence
        .units()
        .iter()
        .filter(|contact| {
            contact.hp > 0
                && contact.confidence_at(now) > 0
                && contact.kind.stats().domain == Domain::Air
                && contact.kind.stats().can_fight()
        })
        .map(|contact| {
            LocatedDemand::observed(
                StandingGroundTarget::point(contact.tile),
                contact.evidence,
                contact.confidence_at(now),
                combat_strength(contact.kind, contact.hp, Domain::Ground).max(combat_strength(
                    contact.kind,
                    contact.hp,
                    Domain::Air,
                )),
            )
        })
        .collect()
}

fn hostile_defense_demands(now: Tick, intelligence: &StrategicIntelligence) -> Vec<LocatedDemand> {
    intelligence
        .buildings()
        .iter()
        .filter(|contact| {
            contact.hp > 0
                && contact.built
                && contact.confidence_at(now) > 0
                && contact
                    .kind
                    .tier_stats(contact.tier)
                    .weapons
                    .iter()
                    .any(|weapon| weapon.targets.covers(Domain::Ground))
        })
        .map(|contact| {
            let stats = contact.kind.tier_stats(contact.tier);
            LocatedDemand::observed(
                StandingGroundTarget::footprint(contact.anchor, stats.size),
                contact.evidence,
                contact.confidence_at(now),
                u64::from(contact.hp).saturating_mul(
                    stats
                        .weapons
                        .iter()
                        .filter(|weapon| weapon.targets.covers(Domain::Ground))
                        .map(weapon_burst_dps100)
                        .sum(),
                ),
            )
        })
        .collect()
}

fn wounded_support_needs(
    obs: &Observation,
    excluded_units: &[UnitId],
    resources: &ResourceSnapshot,
    roster: &InventoryRoster,
    component_inventory: &mut ComponentInventory,
    routing: &mut ServiceRouting<'_>,
    work_per_tender: u32,
) -> Vec<(u32, Vec<StandingGroundTarget>)> {
    let fabricators = resources
        .producers()
        .iter()
        .filter(|lane| lane.kind == BuildingKind::Fabricator)
        .map(|lane| lane.producer)
        .collect::<Vec<_>>();
    let wounded = obs
        .my_units
        .iter()
        .filter(|unit| !excluded_units.contains(&unit.id))
        .filter(|unit| {
            unit.kind.stats().domain == Domain::Ground
                && unit.kind.stats().can_fight()
                && unit.hp < unit.kind.stats().max_hp
        })
        .map(|unit| {
            LocatedDemand::fixed(
                StandingGroundTarget::point(unit.tile),
                u64::from(unit.kind.stats().max_hp - unit.hp),
            )
        })
        .collect::<Vec<_>>();

    let mut needs = Vec::new();
    for mut component in demand_components(Domain::Ground, wounded, routing) {
        if !fabricators.iter().any(|producer| {
            routing.producer_reaches_any(*producer, UnitKind::Tender, &component.targets)
        }) {
            continue;
        }
        let demand = u32::try_from(component.fixed.div_ceil(u64::from(work_per_tender.max(1))))
            .unwrap_or(u32::MAX);
        let serviceable = component_inventory
            .serviceable(Domain::Ground, &component.targets, roster, routing)
            .tenders;
        let component_unmet = demand.saturating_sub(serviceable);
        if component_unmet == 0 {
            continue;
        }
        component.targets.sort_unstable();
        component.targets.dedup();
        needs.push((component_unmet, component.targets));
    }
    needs.sort_unstable_by_key(|(_, targets)| targets.first().copied());
    needs
}

fn contact_confidence(evidence: Option<ContactEvidence>) -> Confidence {
    match evidence {
        Some(ContactEvidence::Current) => Confidence::Current,
        Some(ContactEvidence::Remembered) => Confidence::Supported,
        None => Confidence::Prior,
    }
}

fn threat_case(evidence: Option<ContactEvidence>, value: StrategicValue) -> ProposalCase {
    let current = evidence == Some(ContactEvidence::Current);
    ProposalCase {
        urgency: if current {
            Urgency::Pressing
        } else {
            Urgency::Timely
        },
        confidence: contact_confidence(evidence),
        value,
        time_to_impact: if current {
            TimeToImpact::Immediate
        } else {
            TimeToImpact::Near
        },
        safety: if current {
            ExecutionSafety::Secure
        } else {
            ExecutionSafety::Managed
        },
    }
}

type CandidateRank = (
    u8,
    u8,
    u8,
    u8,
    u8,
    u32,
    u8,
    u128,
    Reverse<u32>,
    Reverse<u32>,
);

const fn demand_candidate_key(candidate: &DemandCandidate) -> StandingForceKey {
    StandingForceKey {
        kind: candidate.kind,
        service: candidate.service,
    }
}

fn candidate_rank(candidate: &DemandCandidate) -> CandidateRank {
    (
        urgency_rank(candidate.case.urgency),
        confidence_rank(candidate.case.confidence),
        value_rank(candidate.case.value),
        time_to_impact_rank(candidate.case.time_to_impact),
        safety_rank(candidate.case.safety),
        candidate.unmet,
        candidate.personality,
        candidate.provider_value,
        Reverse(candidate.kind.stats().train_ticks),
        Reverse(candidate.kind.stats().cost),
    )
}

const fn urgency_rank(value: Urgency) -> u8 {
    match value {
        Urgency::Developmental => 0,
        Urgency::Timely => 1,
        Urgency::Pressing => 2,
    }
}

const fn confidence_rank(value: Confidence) -> u8 {
    match value {
        Confidence::Prior => 0,
        Confidence::Supported => 1,
        Confidence::Current => 2,
    }
}

const fn value_rank(value: StrategicValue) -> u8 {
    match value {
        StrategicValue::Incremental => 0,
        StrategicValue::Material => 1,
        StrategicValue::Decisive => 2,
    }
}

const fn time_to_impact_rank(value: TimeToImpact) -> u8 {
    match value {
        TimeToImpact::Patient => 0,
        TimeToImpact::Near => 1,
        TimeToImpact::Immediate => 2,
    }
}

const fn safety_rank(value: ExecutionSafety) -> u8 {
    match value {
        ExecutionSafety::Speculative => 0,
        ExecutionSafety::Managed => 1,
        ExecutionSafety::Secure => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::allocation::{
        AllocationPersonality, CrossDomainAllocation, connected_investment_proposal,
        standing_force_investment_proposals,
    };
    use crate::bot::observation::{BuildingObs, UnitObs};
    use crate::bot::profile::PersonalityTraits;
    use crate::bot::strategy::{
        ConnectedConfidence, ConnectedExecutionSafety, ConnectedOffenseClaims,
        ConnectedOpportunityCase, ConnectedProviderJob, ConnectedStrategicValue,
        ConnectedTimeToImpact, ConnectedUrgency, FreshConnectedProposal,
        FreshConnectedProposalFixture,
    };
    use crate::ids::PlayerId;
    use crate::map::Terrain;
    use crate::scenario::{BotDifficulty, BotStance};
    use chassis::grid::TilePos;

    fn observation(scrap: u32) -> Observation {
        Observation {
            tick: 120,
            scrap,
            map_width: 32,
            map_height: 20,
            visible: vec![true; 32 * 20],
            explored: vec![true; 32 * 20],
            ..Observation::default()
        }
    }

    fn public_map(
        obs: &Observation,
        non_ground_terrain: Vec<(TilePos, Terrain)>,
    ) -> PublicMapBriefing {
        PublicMapBriefing {
            map_width: obs.map_width,
            map_height: obs.map_height,
            starting_foundries: Vec::new(),
            teams: vec![None, None],
            non_ground_terrain,
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        }
    }

    fn building(id: u32, kind: BuildingKind) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player: PlayerId(0),
            kind,
            anchor: TilePos::new(2 + i32::try_from(id).unwrap(), 2),
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn add_producer(obs: &mut Observation, id: u32, kind: BuildingKind, queue: Vec<UnitKind>) {
        obs.my_buildings.push(building(id, kind));
        obs.my_queues.push(queue);
        obs.my_queue_progress.push(0);
    }

    fn unit(id: u32, kind: UnitKind) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(0),
            kind,
            tile: TilePos::new(6 + i32::try_from(id % 8).unwrap(), 8),
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

    fn enemy_unit(id: u32, kind: UnitKind) -> UnitObs {
        UnitObs {
            player: PlayerId(1),
            idle: false,
            tile: TilePos::new(22, 10),
            ..unit(id, kind)
        }
    }

    fn enemy_building(id: u32, kind: BuildingKind) -> BuildingObs {
        BuildingObs {
            player: PlayerId(1),
            anchor: TilePos::new(22, 9),
            ..building(id, kind)
        }
    }

    fn profile(air: u8, siege: u8, support: u8, fortification: u8) -> ResolvedProfile {
        ResolvedProfile {
            difficulty: BotDifficulty::Prime,
            stance: BotStance::Balanced,
            personality_seed: 0,
            primary: Specialty::Air,
            secondary: Specialty::Siege,
            traits: PersonalityTraits {
                air,
                siege,
                support,
                fortification,
                greed: 50,
                guile: 50,
            },
        }
    }

    fn prime() -> (ResolvedProfile, DifficultyTuning) {
        (
            profile(50, 50, 50, 50),
            DifficultyTuning::for_level(BotDifficulty::Prime),
        )
    }

    fn fill_core(obs: &mut Observation, count: u32) {
        obs.my_units
            .extend((0..count).map(|id| unit(id + 10, UnitKind::Sentinel)));
    }

    fn add_reclaimers(obs: &mut Observation, first_id: u32, count: u32) {
        for id in first_id..first_id.saturating_add(count) {
            add_producer(obs, id, BuildingKind::Reclaimer, Vec::new());
        }
    }

    fn derive(
        obs: &Observation,
        intelligence: &StrategicIntelligence,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        context: StandingForceContext<'_>,
    ) -> Option<StandingForceProposal> {
        let resources = ResourceSnapshot::from_observation(obs);
        derive_standing_force_proposals(obs, intelligence, profile, tuning, &resources, context)
            .into_iter()
            .next()
    }

    fn derive_all(
        obs: &Observation,
        intelligence: &StrategicIntelligence,
        profile: &ResolvedProfile,
        tuning: DifficultyTuning,
        context: StandingForceContext<'_>,
    ) -> Vec<StandingForceProposal> {
        let resources = ResourceSnapshot::from_observation(obs);
        derive_standing_force_proposals(obs, intelligence, profile, tuning, &resources, context)
    }

    #[test]
    fn live_paid_and_committed_inventory_are_counted_once_while_owned_units_are_excluded() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, vec![UnitKind::Sentinel]);
        obs.my_queues[0].push(UnitKind::Sentinel);
        fill_core(&mut obs, 7);
        let commitments = [StandingProductionCommitment::paid(
            BuildingId(1),
            UnitKind::Sentinel,
        )];
        let (profile, tuning) = prime();
        let intelligence = StrategicIntelligence::new();

        assert_eq!(
            InventoryRoster::from_observation(&obs, StandingForceContext::new(&[], &commitments),)
                .all()
                .line_bodies,
            8,
            "one paid ownership record consumes one of two matching queue occurrences"
        );
        let both_paid = [
            StandingProductionCommitment::paid(BuildingId(1), UnitKind::Sentinel),
            StandingProductionCommitment::paid(BuildingId(1), UnitKind::Sentinel),
        ];
        assert_eq!(
            InventoryRoster::from_observation(&obs, StandingForceContext::new(&[], &both_paid))
                .all()
                .line_bodies,
            7,
            "repeated ownership records preserve exact multiplicity without queue indexes"
        );

        assert!(
            derive(
                &obs,
                &intelligence,
                &profile,
                tuning,
                StandingForceContext::new(&[], &commitments),
            )
            .is_none(),
            "seven live and one unowned paid queue item satisfy the floor; planner-owned work does not"
        );

        let excluded = [UnitId(10)];
        let proposal = derive(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&excluded, &commitments),
        )
        .expect("excluding one operation-owned unit reopens the core demand");
        assert_eq!(proposal.reason(), StandingForceReason::CoreRecovery);
        assert_eq!(proposal.key_kind(), UnitKind::Sentinel);
    }

    #[test]
    fn current_air_contact_becomes_memory_then_clears_under_fresh_negative_evidence() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 8);
        let mut intelligence = StrategicIntelligence::new();
        obs.enemy_units = vec![enemy_unit(90, UnitKind::Moth)];
        intelligence.update(&obs);
        let (profile, tuning) = prime();

        let current = derive(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("a current bomber demands air defense");
        assert_eq!(current.reason(), StandingForceReason::AirDefense);
        assert_eq!(current.case().confidence, Confidence::Current);

        let mut hidden = obs.clone();
        hidden.tick += 120;
        hidden.enemy_units.clear();
        hidden.visible[10 * 32 + 22] = false;
        intelligence.update(&hidden);
        let remembered = derive(
            &hidden,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("recent remembered bomber evidence remains actionable");
        assert_eq!(remembered.reason(), StandingForceReason::AirDefense);
        assert_eq!(remembered.case().confidence, Confidence::Supported);

        let mut clear = hidden.clone();
        clear.tick += 12;
        clear.visible.fill(true);
        intelligence.update(&clear);
        assert!(
            derive(
                &clear,
                &intelligence,
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .is_none(),
            "fresh visibility over the remembered location retires the counter demand"
        );
    }

    #[test]
    fn parked_bomber_creates_the_same_air_defense_alternatives_as_an_airborne_bomber() {
        fn alternatives(grounded: bool) -> BTreeMap<UnitKind, Vec<BuildingId>> {
            let mut obs = observation(2_000);
            add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
            add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
            add_producer(&mut obs, 3, BuildingKind::Airworks, Vec::new());
            add_producer(&mut obs, 4, BuildingKind::Crucible, Vec::new());
            fill_core(&mut obs, 8);
            let mut bomber = enemy_unit(90, UnitKind::Moth);
            bomber.grounded = grounded;
            obs.enemy_units.push(bomber);
            let mut intelligence = StrategicIntelligence::new();
            intelligence.update(&obs);
            let (profile, tuning) = prime();

            derive_all(
                &obs,
                &intelligence,
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .into_iter()
            .filter(|proposal| proposal.reason() == StandingForceReason::AirDefense)
            .map(|proposal| (proposal.key_kind(), proposal.eligible_producers().to_vec()))
            .collect()
        }

        let airborne = alternatives(false);
        let parked = alternatives(true);

        assert_eq!(parked, airborne);
        assert_eq!(
            airborne.keys().copied().collect::<Vec<_>>(),
            vec![UnitKind::Flakhound, UnitKind::Talon, UnitKind::Shrike,],
            "every affordable Ferrous anti-air provider remains available"
        );
        assert_eq!(airborne[&UnitKind::Flakhound], vec![BuildingId(2)]);
        assert_eq!(airborne[&UnitKind::Talon], vec![BuildingId(3)]);
        assert_eq!(airborne[&UnitKind::Shrike], vec![BuildingId(3)]);
    }

    #[test]
    fn parked_aircraft_cannot_make_unreachable_ground_pressure_routable() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 8);

        let mut warden = enemy_unit(90, UnitKind::Warden);
        warden.tile = TilePos::new(22, 10);
        let mut parked_moth = enemy_unit(91, UnitKind::Moth);
        parked_moth.tile = TilePos::new(10, 10);
        parked_moth.grounded = true;
        obs.enemy_units = vec![warden, parked_moth];

        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let public_map = public_map(
            &obs,
            (0..obs.map_height)
                .map(|y| (TilePos::new(16, y), Terrain::Peak))
                .collect(),
        );
        let (profile, tuning) = prime();
        let proposals = derive_all(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::footprint(
                    TilePos::new(3, 2),
                    BuildingKind::Foundry.base_stats().size,
                ),
                Some(&public_map),
                &[],
                None,
            ),
        );

        assert!(
            proposals
                .iter()
                .any(|proposal| proposal.reason() == StandingForceReason::AirDefense),
            "the parked Moth remains an intrinsic air threat"
        );
        assert!(
            proposals
                .iter()
                .all(|proposal| proposal.reason() != StandingForceReason::GroundPressure),
            "a reachable parked aircraft must not lend its temporary ground body to the Warden across the impassable divide: {proposals:?}"
        );
    }

    #[test]
    fn true_ground_combatant_remains_ground_pressure() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 8);
        obs.enemy_units.push(enemy_unit(90, UnitKind::Warden));
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let (profile, tuning) = prime();

        let proposals = derive_all(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        );

        assert!(
            proposals
                .iter()
                .any(|proposal| proposal.reason() == StandingForceReason::GroundPressure)
        );
        assert!(
            proposals
                .iter()
                .all(|proposal| proposal.reason() != StandingForceReason::AirDefense)
        );
    }

    #[test]
    fn wealthy_large_security_gap_uses_useful_completed_high_tech() {
        let mut obs = observation(4_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(&mut obs, 3, BuildingKind::Crucible, Vec::new());
        fill_core(&mut obs, 8);
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_expansion_security(
                StandingGroundTarget::footprint(
                    TilePos::new(18, 8),
                    BuildingKind::Foundry.base_stats().size,
                ),
                full_ground_strength(UnitKind::Sentinel).saturating_mul(48),
            ),
        )
        .expect("a large assessed security gap has useful standing-force demand");
        assert_eq!(proposal.reason(), StandingForceReason::ExpansionSecurity);
        assert_eq!(proposal.key_kind(), UnitKind::Breaker);
        assert_eq!(proposal.eligible_producers(), &[BuildingId(3)]);
    }

    #[test]
    fn wealthy_finite_security_need_values_durable_headroom_without_overbuying_a_breaker() {
        let mut obs = observation(1_500);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(&mut obs, 3, BuildingKind::Crucible, Vec::new());
        fill_core(&mut obs, 8);
        for id in 20..23 {
            let mut site = building(id, BuildingKind::FlakTurret);
            site.built = false;
            site.hp = 1;
            obs.my_buildings.push(site);
            obs.my_queues.push(Vec::new());
            obs.my_queue_progress.push(0);
        }
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("paid exposed sites require a finite amount of line security");

        assert_eq!(
            proposal.reason(),
            StandingForceReason::InvestedCapitalSecurity
        );
        assert_eq!(
            proposal.key_kind(),
            UnitKind::Warden,
            "one durable tier-two body is worth its small excess over three disposable screens"
        );
    }

    #[test]
    fn developmental_force_waits_for_a_better_completed_provider_then_buys_it() {
        let mut obs = observation(UnitKind::Sentinel.stats().cost);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_reclaimers(&mut obs, 10, 9);
        fill_core(&mut obs, 8);
        let mut profile = profile(20, 0, 20, 100);
        profile.stance = BotStance::Turtle;
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let targets = [StandingGroundTarget::point(TilePos::new(22, 10))];
        let context = StandingForceContext::new(&[], &[]).with_ground_routing(
            StandingGroundTarget::point(TilePos::new(3, 2)),
            None,
            &targets,
            None,
        );

        let alternatives = derive_all(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            context,
        );
        let wait = alternatives
            .iter()
            .find(|proposal| proposal.key_kind() == UnitKind::Warden)
            .expect("forecast should expose the bounded higher-tier wait to allocation");
        assert!(wait.accumulation().is_some());
        let fallback = alternatives
            .iter()
            .find(|proposal| proposal.key_kind() == UnitKind::Sentinel)
            .expect("the allocator must retain the affordable fallback as an alternative");
        assert_eq!(fallback.accumulation(), None);

        obs.tick += 1;
        obs.scrap = UnitKind::Warden.stats().cost;
        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            context,
        )
        .expect("the saved current bank can buy the preferred provider normally");
        assert_eq!(proposal.reason(), StandingForceReason::ForceProjection);
        assert_eq!(proposal.key_kind(), UnitKind::Warden);
        assert_eq!(proposal.accumulation(), None);
        assert_eq!(proposal.eligible_producers(), &[BuildingId(2)]);
    }

    #[test]
    fn developmental_force_uses_the_affordable_provider_when_income_cannot_close_the_gap() {
        let mut obs = observation(UnitKind::Sentinel.stats().cost);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_reclaimers(&mut obs, 10, 1);
        fill_core(&mut obs, 8);
        let profile = profile(20, 10, 20, 100);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let targets = [StandingGroundTarget::point(TilePos::new(22, 10))];

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(3, 2)),
                None,
                &targets,
                None,
            ),
        )
        .expect("a remote future purchase must not turn useful current scrap into a hoard");

        assert_eq!(proposal.reason(), StandingForceReason::ForceProjection);
        assert_eq!(proposal.key_kind(), UnitKind::Sentinel);
    }

    #[test]
    fn core_recovery_and_current_pressure_never_wait_for_a_higher_tier_body() {
        let mut deficient = observation(UnitKind::Sentinel.stats().cost);
        add_producer(&mut deficient, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut deficient, 2, BuildingKind::Fabricator, Vec::new());
        add_reclaimers(&mut deficient, 10, 8);
        fill_core(&mut deficient, 7);
        let (profile, tuning) = prime();

        let recovery = derive(
            &deficient,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("the ordinary floor needs a body now");
        assert_eq!(recovery.reason(), StandingForceReason::CoreRecovery);
        assert_eq!(recovery.key_kind(), UnitKind::Sentinel);

        let mut pressured = deficient;
        pressured.my_units.push(unit(90, UnitKind::Sentinel));
        pressured.enemy_units.push(enemy_unit(91, UnitKind::Warden));
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&pressured);
        let response = derive(
            &pressured,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("a current hostile force requires the useful provider that is affordable now");
        assert_eq!(response.reason(), StandingForceReason::GroundPressure);
        assert_eq!(response.key_kind(), UnitKind::Sentinel);
    }

    #[test]
    fn saving_for_one_need_does_not_suppress_an_affordable_counter_for_another() {
        let mut obs = observation(UnitKind::Flakhound.stats().cost);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_reclaimers(&mut obs, 10, 9);
        fill_core(&mut obs, 8);
        for id in 20..23 {
            let mut site = building(id, BuildingKind::FlakTurret);
            site.built = false;
            site.hp = 1;
            obs.my_buildings.push(site);
            obs.my_queues.push(Vec::new());
            obs.my_queue_progress.push(0);
        }
        obs.enemy_units.push(enemy_unit(90, UnitKind::Moth));
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        obs.tick += 120;
        obs.enemy_units.clear();
        obs.visible[10 * 32 + 22] = false;
        intelligence.update(&obs);
        let profile = profile(10, 10, 10, 100);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);

        let alternatives = derive_all(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        );
        let proposal = alternatives
            .iter()
            .find(|proposal| proposal.reason() == StandingForceReason::AirDefense)
            .expect("line saving must leave a distinct affordable air counter available");

        assert_eq!(proposal.reason(), StandingForceReason::AirDefense);
        assert_eq!(proposal.key_kind(), UnitKind::Flakhound);
        assert_eq!(proposal.accumulation(), None);
    }

    #[test]
    fn large_need_retains_cheaper_fallbacks_for_shared_allocation() {
        let mut obs = observation(4_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(&mut obs, 3, BuildingKind::Crucible, Vec::new());
        fill_core(&mut obs, 8);
        let (profile, tuning) = prime();

        let proposals = derive_all(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_expansion_security(
                StandingGroundTarget::footprint(
                    TilePos::new(18, 8),
                    BuildingKind::Foundry.base_stats().size,
                ),
                full_ground_strength(UnitKind::Sentinel).saturating_mul(48),
            ),
        );
        let line = proposals
            .iter()
            .filter(|proposal| proposal.reason() == StandingForceReason::ExpansionSecurity)
            .map(StandingForceProposal::key_kind)
            .collect::<Vec<_>>();

        assert_eq!(line.first(), Some(&UnitKind::Breaker));
        assert!(line.contains(&UnitKind::Warden));
        assert!(line.contains(&UnitKind::Sentinel));
    }

    #[test]
    fn siege_provider_scale_uses_lancer_bombard_and_avalanche_at_distinct_opportunities() {
        let mut obs = observation(110);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 8);
        obs.enemy_buildings = vec![enemy_building(90, BuildingKind::Turret)];
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let siege_profile = profile(20, 100, 20, 20);

        let cheap = derive_all(
            &obs,
            &intelligence,
            &siege_profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .into_iter()
        .filter(|proposal| proposal.reason() == StandingForceReason::SiegePressure)
        .collect::<Vec<_>>();
        assert_eq!(cheap.len(), 1);
        assert_eq!(cheap[0].key_kind(), UnitKind::Lancer);

        obs.scrap = 400;
        let tier_one = derive_all(
            &obs,
            &intelligence,
            &siege_profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .into_iter()
        .filter(|proposal| proposal.reason() == StandingForceReason::SiegePressure)
        .map(|proposal| proposal.key_kind())
        .collect::<Vec<_>>();
        assert_eq!(tier_one.first(), Some(&UnitKind::Bombard));
        assert!(tier_one.contains(&UnitKind::Lancer));

        obs.scrap = 2_000;
        add_producer(&mut obs, 3, BuildingKind::Crucible, Vec::new());
        let full_tech = derive_all(
            &obs,
            &intelligence,
            &siege_profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .into_iter()
        .filter(|proposal| proposal.reason() == StandingForceReason::SiegePressure)
        .map(|proposal| proposal.key_kind())
        .collect::<Vec<_>>();
        assert_eq!(full_tech.first(), Some(&UnitKind::Avalanche));
        assert!(full_tech.contains(&UnitKind::Bombard));
        assert!(full_tech.contains(&UnitKind::Lancer));
    }

    #[test]
    fn ground_alternatives_use_only_producers_connected_to_their_need() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 20, BuildingKind::Foundry, Vec::new());
        fill_core(&mut obs, 8);
        let wall = (0..obs.map_height)
            .map(|y| (TilePos::new(16, y), Terrain::Pit))
            .collect::<Vec<_>>();
        obs.known_rock = wall.iter().map(|(tile, _)| *tile).collect();
        let briefing = public_map(&obs, wall);
        let (profile, tuning) = prime();

        for (target, expected) in [
            (TilePos::new(8, 10), BuildingId(1)),
            (TilePos::new(24, 10), BuildingId(20)),
        ] {
            let targets = [StandingGroundTarget::point(target)];
            let proposals = derive_all(
                &obs,
                &StrategicIntelligence::new(),
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]).with_ground_routing(
                    StandingGroundTarget::footprint(
                        TilePos::new(3, 2),
                        BuildingKind::Foundry.base_stats().size,
                    ),
                    Some(&briefing),
                    &targets,
                    None,
                ),
            );
            let sentinel = proposals
                .iter()
                .find(|proposal| proposal.key_kind() == UnitKind::Sentinel)
                .expect("a reachable public objective keeps line production useful");
            assert_eq!(sentinel.eligible_producers(), &[expected]);
        }
    }

    #[test]
    fn stranded_line_inventory_does_not_satisfy_the_home_core() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 20, BuildingKind::Foundry, Vec::new());
        fill_core(&mut obs, 8);
        for unit in &mut obs.my_units {
            unit.tile = TilePos::new(22, 10);
        }
        let wall = (0..obs.map_height)
            .map(|y| (TilePos::new(16, y), Terrain::Pit))
            .collect::<Vec<_>>();
        obs.known_rock = wall.iter().map(|(tile, _)| *tile).collect();
        let briefing = public_map(&obs, wall);
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(8, 10)),
                Some(&briefing),
                &[],
                None,
            ),
        )
        .expect("a full line force across an impassable divide cannot defend home");

        assert_eq!(proposal.reason(), StandingForceReason::CoreRecovery);
        assert_eq!(proposal.key_kind(), UnitKind::Sentinel);
        assert_eq!(proposal.eligible_producers(), &[BuildingId(1)]);
    }

    #[test]
    fn line_strength_at_one_hostile_component_does_not_cover_another() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 20, BuildingKind::Foundry, Vec::new());
        fill_core(&mut obs, 8);
        let mut near = enemy_unit(90, UnitKind::Sentinel);
        near.tile = TilePos::new(10, 10);
        obs.enemy_units.push(near);
        obs.enemy_units.push(enemy_unit(91, UnitKind::Sentinel));
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let wall = (0..obs.map_height)
            .map(|y| (TilePos::new(16, y), Terrain::Pit))
            .collect::<Vec<_>>();
        obs.known_rock = wall.iter().map(|(tile, _)| *tile).collect();
        let briefing = public_map(&obs, wall);
        let (profile, tuning) = prime();
        let roster = InventoryRoster::from_observation(&obs, StandingForceContext::new(&[], &[]));
        assert!(
            roster.all().line_strength >= threat_summary(obs.tick, &intelligence).ground.total(),
            "the regression requires union-wide inventory to look sufficient"
        );

        let proposal = derive_all(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(8, 10)),
                Some(&briefing),
                &[],
                None,
            ),
        )
        .into_iter()
        .find(|proposal| proposal.reason() == StandingForceReason::GroundPressure)
        .expect("the undefended hostile component still needs a line response");

        assert_eq!(proposal.eligible_producers(), &[BuildingId(20)]);
    }

    #[test]
    fn paid_sites_in_separate_components_receive_separate_security() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 20, BuildingKind::Foundry, Vec::new());
        fill_core(&mut obs, 16);
        for (id, anchor) in [(30, TilePos::new(8, 9)), (31, TilePos::new(22, 9))] {
            let mut site = building(id, BuildingKind::Turret);
            site.anchor = anchor;
            site.built = false;
            site.hp = 1;
            obs.my_buildings.push(site);
            obs.my_queues.push(Vec::new());
            obs.my_queue_progress.push(0);
        }
        let wall = (0..obs.map_height)
            .map(|y| (TilePos::new(16, y), Terrain::Pit))
            .collect::<Vec<_>>();
        obs.known_rock = wall.iter().map(|(tile, _)| *tile).collect();
        let briefing = public_map(&obs, wall);
        let (profile, tuning) = prime();

        let proposal = derive_all(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(8, 10)),
                Some(&briefing),
                &[],
                None,
            ),
        )
        .into_iter()
        .find(|proposal| proposal.reason() == StandingForceReason::InvestedCapitalSecurity)
        .expect("the unprotected paid site needs security in its own component");

        assert_eq!(proposal.eligible_producers(), &[BuildingId(20)]);
    }

    #[test]
    fn force_projection_grows_the_weaker_reachable_component() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 20, BuildingKind::Foundry, Vec::new());
        fill_core(&mut obs, 8);
        let wall = (0..obs.map_height)
            .map(|y| (TilePos::new(16, y), Terrain::Pit))
            .collect::<Vec<_>>();
        obs.known_rock = wall.iter().map(|(tile, _)| *tile).collect();
        let briefing = public_map(&obs, wall);
        let targets = [
            StandingGroundTarget::point(TilePos::new(10, 10)),
            StandingGroundTarget::point(TilePos::new(22, 10)),
        ];
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(8, 10)),
                Some(&briefing),
                &targets,
                None,
            ),
        )
        .expect("a reachable empty front remains a useful force-projection demand");

        assert_eq!(proposal.reason(), StandingForceReason::ForceProjection);
        assert_eq!(proposal.key_kind(), UnitKind::Sentinel);
        assert_eq!(proposal.eligible_producers(), &[BuildingId(20)]);
    }

    #[test]
    fn connected_lane_conflict_uses_same_kind_alternative_from_independent_front() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(&mut obs, 20, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 8);

        let defense_anchor = TilePos::new(10, 9);
        let mut defense = enemy_building(90, BuildingKind::Turret);
        defense.anchor = defense_anchor;
        obs.enemy_buildings.push(defense);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);

        let wall = (0..obs.map_height)
            .map(|y| (TilePos::new(16, y), Terrain::Pit))
            .collect::<Vec<_>>();
        obs.known_rock = wall.iter().map(|(tile, _)| *tile).collect();
        let briefing = public_map(&obs, wall);
        let projection_target = StandingGroundTarget::point(TilePos::new(22, 10));
        let (profile, tuning) = prime();
        let lancers = derive_all(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(8, 10)),
                Some(&briefing),
                core::slice::from_ref(&projection_target),
                None,
            ),
        )
        .into_iter()
        .filter(|proposal| proposal.key_kind() == UnitKind::Lancer)
        .collect::<Vec<_>>();

        assert_eq!(lancers.len(), 2);
        assert_eq!(lancers[0].reason(), StandingForceReason::SiegePressure);
        assert_eq!(lancers[0].eligible_producers(), &[BuildingId(2)]);
        assert_eq!(lancers[1].reason(), StandingForceReason::ForceProjection);
        assert_eq!(lancers[1].eligible_producers(), &[BuildingId(20)]);

        let deadline = obs.tick + u64::from(UnitKind::Bombard.stats().train_ticks);
        let connected = FreshConnectedProposal::fixture(FreshConnectedProposalFixture {
            objective: BuildingId(700),
            anchor: defense_anchor,
            deadline,
            case: ConnectedOpportunityCase::fixture(
                ConnectedUrgency::Pressing,
                ConnectedConfidence::Current,
                ConnectedStrategicValue::Decisive,
                ConnectedTimeToImpact::Near,
                ConnectedExecutionSafety::Managed,
            ),
            minimum_claims: ConnectedOffenseClaims::fixture(
                Vec::new(),
                vec![ConnectedProviderJob::fixture(
                    UnitKind::Bombard,
                    obs.tick,
                    deadline,
                    vec![BuildingId(2)],
                )],
            ),
            marginal_additions: Vec::new(),
            protected_current_scrap: 0,
            protected_forecast_scrap: 0,
        });
        let resources = ResourceSnapshot::from_observation(&obs);
        let mut allocation = CrossDomainAllocation::new(&resources, deadline, 12)
            .expect("the two producer lanes have a valid bounded forecast");
        allocation.offer(
            connected_investment_proposal(connected)
                .expect("the connected package has exact claims"),
        );
        for proposal in standing_force_investment_proposals(lancers)
            .expect("the route-local standing alternatives have exact claims")
        {
            allocation.offer(proposal);
        }

        let settlement = allocation
            .resolve(AllocationPersonality::default(), None)
            .expect("the independent standing-force lane remains jointly feasible");
        let scheduled = settlement
            .producer_schedule()
            .iter()
            .map(|job| (job.producer, job.kind))
            .collect::<Vec<_>>();
        assert!(
            scheduled.contains(&(BuildingId(2), UnitKind::Bombard)),
            "the selected connected package must retain its preferred lane: {scheduled:?}"
        );
        assert!(
            scheduled.contains(&(BuildingId(20), UnitKind::Lancer)),
            "the compatible route-local standing alternative must survive: {scheduled:?}"
        );
        assert!(
            !scheduled.contains(&(BuildingId(2), UnitKind::Lancer)),
            "the conflicted higher-ranked standing alternative must be rejected: {scheduled:?}"
        );
    }

    #[test]
    fn queued_ground_anti_air_only_satisfies_the_component_its_producer_can_reach() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(
            &mut obs,
            20,
            BuildingKind::Fabricator,
            vec![
                UnitKind::Flakhound,
                UnitKind::Flakhound,
                UnitKind::Flakhound,
            ],
        );
        fill_core(&mut obs, 8);
        obs.enemy_units.push(enemy_unit(90, UnitKind::Moth));
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let wall = (0..obs.map_height)
            .map(|y| (TilePos::new(16, y), Terrain::Pit))
            .collect::<Vec<_>>();
        obs.known_rock = wall.iter().map(|(tile, _)| *tile).collect();
        let briefing = public_map(&obs, wall);
        let (profile, tuning) = prime();
        let roster = InventoryRoster::from_observation(&obs, StandingForceContext::new(&[], &[]));
        assert!(
            roster.all().anti_air_strength >= threat_summary(obs.tick, &intelligence).air.total(),
            "the regression requires global accounting to be falsely sufficient"
        );

        let proposal = derive_all(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(8, 10)),
                Some(&briefing),
                &[],
                None,
            ),
        )
        .into_iter()
        .find(|proposal| {
            proposal.reason() == StandingForceReason::AirDefense
                && proposal.key_kind() == UnitKind::Flakhound
        })
        .expect("remote queued flak cannot cover the home air-defense shortfall");

        assert_eq!(proposal.eligible_producers(), &[BuildingId(2)]);
    }

    #[test]
    fn stranded_siege_inventory_does_not_satisfy_a_reachable_defense_target() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 20, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 8);
        for id in 70..78 {
            let mut lancer = unit(id, UnitKind::Lancer);
            lancer.tile = TilePos::new(8, 10);
            obs.my_units.push(lancer);
        }
        let mut near_defense = enemy_building(90, BuildingKind::Turret);
        near_defense.anchor = TilePos::new(8, 9);
        obs.enemy_buildings.push(near_defense);
        obs.enemy_buildings
            .push(enemy_building(91, BuildingKind::Turret));
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let wall = (0..obs.map_height)
            .map(|y| (TilePos::new(16, y), Terrain::Pit))
            .collect::<Vec<_>>();
        obs.known_rock = wall.iter().map(|(tile, _)| *tile).collect();
        let briefing = public_map(&obs, wall);
        let (profile, tuning) = prime();
        let roster = InventoryRoster::from_observation(&obs, StandingForceContext::new(&[], &[]));
        assert!(
            roster.all().siege_strength >= threat_summary(obs.tick, &intelligence).defenses.total(),
            "the regression requires global siege accounting to be falsely sufficient"
        );

        let proposal = derive_all(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(8, 10)),
                Some(&briefing),
                &[],
                None,
            ),
        )
        .into_iter()
        .find(|proposal| proposal.reason() == StandingForceReason::SiegePressure)
        .expect("siege stranded left of the divide cannot pressure a defense on the right");

        assert_eq!(proposal.eligible_producers(), &[BuildingId(20)]);
    }

    #[test]
    fn air_inventory_and_airworks_roofs_must_share_an_air_component_with_home() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 3, BuildingKind::Airworks, Vec::new());
        add_producer(&mut obs, 20, BuildingKind::Airworks, Vec::new());
        fill_core(&mut obs, 8);
        for id in 70..72 {
            let mut shrike = unit(id, UnitKind::Shrike);
            shrike.tile = TilePos::new(22, 10);
            obs.my_units.push(shrike);
        }
        let mut moth = enemy_unit(90, UnitKind::Moth);
        moth.tile = TilePos::new(10, 10);
        obs.enemy_units.push(moth);
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let wall = (0..obs.map_height)
            .map(|y| (TilePos::new(16, y), Terrain::Peak))
            .collect::<Vec<_>>();
        obs.known_rock = wall.iter().map(|(tile, _)| *tile).collect();
        obs.known_peaks = obs.known_rock.clone();
        let briefing = public_map(&obs, wall);
        let (profile, tuning) = prime();
        let roster = InventoryRoster::from_observation(&obs, StandingForceContext::new(&[], &[]));
        assert!(
            roster.all().anti_air_strength >= threat_summary(obs.tick, &intelligence).air.total(),
            "the regression requires global fighter accounting to be falsely sufficient"
        );

        let air_defense = derive_all(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(8, 10)),
                Some(&briefing),
                &[],
                None,
            ),
        )
        .into_iter()
        .filter(|proposal| {
            proposal.reason() == StandingForceReason::AirDefense
                && proposal.key_kind().stats().domain == Domain::Air
        })
        .collect::<Vec<_>>();

        assert!(
            !air_defense.is_empty(),
            "fighters trapped beyond Peaks cannot defend the home airspace"
        );
        assert!(
            air_defense
                .iter()
                .all(|proposal| { proposal.eligible_producers() == [BuildingId(3)] })
        );
    }

    #[test]
    fn flipped_airworks_inventory_uses_the_authoritative_roof_spawn_tile() {
        let mut world = observation(2_000);
        add_producer(&mut world, 3, BuildingKind::Airworks, Vec::new());
        let orientation = Orientation::for_home(&world, TilePos::new(28, 2));
        assert!(!orientation.is_identity());
        let oriented = orientation.observe(&world);
        let producer = &oriented.my_buildings[0];
        let size = producer.kind.tier_stats(producer.tier).size;
        let world_anchor = orientation.anchor(producer.anchor, size);
        let authoritative = orientation.tile(world_anchor.offset(size.0 / 2, size.1 / 2));

        assert_eq!(
            air_production_spawn_tile(producer, Some(orientation)),
            authoritative
        );
        assert_ne!(
            authoritative,
            producer.anchor.offset(size.0 / 2, size.1 / 2),
            "an even footprint's containing tile changes under a flipped command frame"
        );
    }

    #[test]
    fn ground_alternative_requires_the_authoritative_spawn_doorstep_to_reach_its_need() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        obs.my_buildings[0].anchor = TilePos::new(14, 9);
        fill_core(&mut obs, 8);
        obs.known_rock = vec![
            TilePos::new(12, 9),
            TilePos::new(13, 9),
            TilePos::new(12, 10),
            TilePos::new(12, 11),
            TilePos::new(13, 11),
        ];
        obs.known_rock.sort_by_key(|tile| (tile.y, tile.x));
        let target = TilePos::new(22, 10);
        let producer = &obs.my_buildings[0];

        let spawn = production_spawn_doorstep(&obs, producer, None, None)
            .expect("the canonical outward doorstep remains open");
        assert_eq!(spawn, TilePos::new(13, 10));
        let mut routes = RouteProjection::new(&obs, Domain::Ground);
        assert!(
            !routes.reaches(spawn, target),
            "the authoritative spawn is isolated beside the Fabricator"
        );
        assert!(
            routes.reaches(TilePos::new(16, 10), target),
            "a noncanonical doorstep remains connected and would make any-doorstep admission lie"
        );

        let (profile, tuning) = prime();
        let targets = [StandingGroundTarget::point(target)];
        let proposals = derive_all(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_ground_routing(
                StandingGroundTarget::point(TilePos::new(3, 2)),
                None,
                &targets,
                None,
            ),
        );

        assert!(
            proposals.is_empty(),
            "standing production must not buy a ground unit that will spawn into a disconnected pocket: {proposals:?}"
        );
    }

    #[test]
    fn the_same_security_need_uses_a_tier_one_screen_when_no_higher_provider_exists() {
        let mut obs = observation(4_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        fill_core(&mut obs, 8);
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_expansion_security(
                StandingGroundTarget::footprint(
                    TilePos::new(18, 8),
                    BuildingKind::Foundry.base_stats().size,
                ),
                full_ground_strength(UnitKind::Sentinel).saturating_mul(48),
            ),
        )
        .expect("tier one remains an independently useful legal provider");
        assert_eq!(proposal.key_kind(), UnitKind::Sentinel);
        assert_eq!(proposal.eligible_producers(), &[BuildingId(1)]);
    }

    #[test]
    fn expansion_security_is_real_demand_but_never_an_idle_factory_sink() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        fill_core(&mut obs, 8);
        let (profile, tuning) = prime();
        let intelligence = StrategicIntelligence::new();

        assert!(
            derive(
                &obs,
                &intelligence,
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .is_none()
        );
        let proposal = derive(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]).with_expansion_security(
                StandingGroundTarget::footprint(
                    TilePos::new(18, 8),
                    BuildingKind::Foundry.base_stats().size,
                ),
                full_ground_strength(UnitKind::Sentinel).saturating_mul(9),
            ),
        )
        .expect("the caller's assessed expansion shortfall creates one request");
        assert_eq!(proposal.reason(), StandingForceReason::ExpansionSecurity);
    }

    #[test]
    fn rich_full_tech_keeps_growing_a_varied_force_without_a_count_cap() {
        let mut obs = observation(10_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(&mut obs, 3, BuildingKind::Crucible, Vec::new());
        fill_core(&mut obs, 8);
        let (profile, tuning) = prime();
        let targets = [StandingGroundTarget::point(TilePos::new(22, 10))];
        let context = StandingForceContext::new(&[], &[]).with_ground_routing(
            StandingGroundTarget::point(TilePos::new(3, 2)),
            None,
            &targets,
            None,
        );
        let mut produced = Vec::new();

        for id in 100..120 {
            let proposal = derive(
                &obs,
                &StrategicIntelligence::new(),
                &profile,
                tuning,
                context,
            )
            .expect("a reachable objective keeps marginal standing-force work useful");
            assert_eq!(proposal.reason(), StandingForceReason::ForceProjection);
            produced.push(proposal.key_kind());
            obs.my_units.push(unit(id, proposal.key_kind()));
        }

        assert!(
            produced
                .iter()
                .any(|kind| matches!(kind.role(), Role::Warden | Role::Breaker)),
            "completed line tech must displace a Sentinel-only fallback"
        );
        assert!(
            produced
                .iter()
                .any(|kind| matches!(kind.role(), Role::Lancer | Role::Bombard | Role::Avalanche)),
            "diminishing returns must create a complementary siege component"
        );
    }

    #[test]
    fn unreachable_ground_objective_stops_proactive_ground_production() {
        let mut obs = observation(10_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(&mut obs, 3, BuildingKind::Crucible, Vec::new());
        fill_core(&mut obs, 8);
        let (profile, tuning) = prime();

        assert!(
            derive(
                &obs,
                &StrategicIntelligence::new(),
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .is_none(),
            "idle factories are not evidence that an island needs more ground units"
        );
        assert_eq!(
            derive(
                &obs,
                &StrategicIntelligence::new(),
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]).with_ground_routing(
                    StandingGroundTarget::point(TilePos::new(3, 2)),
                    None,
                    &[StandingGroundTarget::point(TilePos::new(22, 10))],
                    None,
                ),
            )
            .expect("the same completed tech is useful once a ground objective is reachable")
            .reason(),
            StandingForceReason::ForceProjection
        );
    }

    #[test]
    fn paid_construction_creates_cost_derived_security_until_existing_force_covers_it() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 8);
        let mut crucible_site = building(3, BuildingKind::Crucible);
        crucible_site.built = false;
        crucible_site.hp = 1;
        obs.my_buildings.push(crucible_site);
        obs.my_queues.push(Vec::new());
        obs.my_queue_progress.push(0);
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("paid exposed capital creates an ordinary protection demand");
        assert_eq!(
            proposal.reason(),
            StandingForceReason::InvestedCapitalSecurity
        );

        obs.my_units.push(unit(90, UnitKind::Warden));
        assert!(
            derive(
                &obs,
                &StrategicIntelligence::new(),
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .is_none(),
            "existing strength can cover the paid site without a hidden Sentinel reserve"
        );
    }

    #[test]
    fn ground_pressure_compares_strength_and_decays_with_remembered_confidence() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 8);
        obs.enemy_units = vec![enemy_unit(90, UnitKind::Warden)];
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let (profile, tuning) = prime();

        let current = derive(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("one Warden outweighs an eight-Sentinel line by actual combat strength");
        assert_eq!(current.reason(), StandingForceReason::GroundPressure);

        let mut hidden = obs.clone();
        hidden.tick += 480;
        hidden.enemy_units.clear();
        hidden.visible[10 * 32 + 22] = false;
        intelligence.update(&hidden);
        assert!(
            derive(
                &hidden,
                &intelligence,
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .is_none(),
            "stale mobile strength decays below the already-standing line"
        );
    }

    #[test]
    fn queued_work_keeps_the_request_enqueue_tick_at_the_observation_boundary() {
        let mut obs = observation(2_000);
        add_producer(
            &mut obs,
            1,
            BuildingKind::Foundry,
            vec![UnitKind::Harvester],
        );
        obs.my_queue_progress[0] = 50;
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("the deficient core can append behind paid work");
        assert_eq!(proposal.observed_at(), obs.tick);
        assert!(
            proposal.ready_before()
                > obs
                    .tick
                    .saturating_add(Tick::from(UnitKind::Sentinel.stats().train_ticks))
        );
    }

    #[test]
    fn higher_tier_line_units_are_real_screen_bodies_not_sentinel_equivalents() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        fill_core(&mut obs, 7);
        obs.my_units.push(unit(90, UnitKind::Warden));
        let (profile, tuning) = prime();

        assert!(
            derive(
                &obs,
                &StrategicIntelligence::new(),
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .is_none(),
            "a Warden supplies both one physical screen body and its actual stronger value"
        );
    }

    #[test]
    fn difficulty_changes_the_protected_core_without_changing_unit_access() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        fill_core(&mut obs, 5);
        let profile = profile(50, 50, 50, 50);
        let intelligence = StrategicIntelligence::new();

        assert!(
            derive(
                &obs,
                &intelligence,
                &profile,
                DifficultyTuning::for_level(BotDifficulty::Standard),
                StandingForceContext::new(&[], &[]),
            )
            .is_none()
        );
        let prime = derive(
            &obs,
            &intelligence,
            &profile,
            DifficultyTuning::for_level(BotDifficulty::Prime),
            StandingForceContext::new(&[], &[]),
        )
        .expect("Prime protects a deeper fair core");
        assert_eq!(prime.reason(), StandingForceReason::CoreRecovery);
        assert_eq!(prime.key_kind(), UnitKind::Sentinel);
    }

    #[test]
    fn easier_rungs_apply_their_strength_error_to_own_forces_not_hostile_evidence() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 8);
        let mut marginal = enemy_unit(90, UnitKind::Warden);
        marginal.hp = 185;
        obs.enemy_units = vec![marginal];
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let profile = profile(50, 50, 50, 50);

        assert!(
            derive_all(
                &obs,
                &intelligence,
                &profile,
                DifficultyTuning::for_level(BotDifficulty::Scrapheap),
                StandingForceContext::new(&[], &[]),
            )
            .into_iter()
            .any(|proposal| proposal.reason() == StandingForceReason::GroundPressure),
            "underestimating its own line makes the easiest rung spend against marginal pressure"
        );
        assert!(
            derive_all(
                &obs,
                &intelligence,
                &profile,
                DifficultyTuning::for_level(BotDifficulty::Prime),
                StandingForceContext::new(&[], &[]),
            )
            .into_iter()
            .all(|proposal| proposal.reason() != StandingForceReason::GroundPressure),
            "Prime should judge the same exact hostile evidence against its accurate own strength"
        );
    }

    #[test]
    fn reachable_damage_creates_bounded_support_work_and_an_existing_tender_satisfies_it() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 9);
        obs.my_units[0].hp = 1;
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("real reachable damage justifies mobile support");
        assert_eq!(proposal.reason(), StandingForceReason::WoundedSupport);
        assert_eq!(proposal.key_kind(), UnitKind::Tender);

        obs.my_units.push(unit(80, UnitKind::Tender));
        assert!(
            derive(
                &obs,
                &StrategicIntelligence::new(),
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .is_none(),
            "one Tender covers this one bounded repair workload"
        );
    }

    #[test]
    fn a_tender_stranded_in_another_ground_component_does_not_satisfy_reachable_damage() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 9);
        obs.my_units[0].hp = 1;
        let mut stranded = unit(80, UnitKind::Tender);
        stranded.tile = TilePos::new(22, 10);
        obs.my_units.push(stranded);
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(16, y)).collect();
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("a Tender across an impassable wall cannot service the wounded home force");

        assert_eq!(proposal.reason(), StandingForceReason::WoundedSupport);
        assert_eq!(proposal.key_kind(), UnitKind::Tender);
    }

    #[test]
    fn each_wounded_component_needs_its_own_reachable_tender_capacity() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(&mut obs, 20, BuildingKind::Fabricator, Vec::new());
        fill_core(&mut obs, 9);
        obs.my_units[0].hp = 1;
        let mut remote_wounded = unit(80, UnitKind::Sentinel);
        remote_wounded.tile = TilePos::new(22, 10);
        remote_wounded.hp = 1;
        obs.my_units.push(remote_wounded);
        let mut local_tender = unit(81, UnitKind::Tender);
        local_tender.tile = TilePos::new(8, 10);
        obs.my_units.push(local_tender);
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(16, y)).collect();
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &StrategicIntelligence::new(),
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("one local Tender cannot cover a separate wounded component");

        assert_eq!(proposal.reason(), StandingForceReason::WoundedSupport);
        assert_eq!(proposal.key_kind(), UnitKind::Tender);
        assert_eq!(proposal.eligible_producers(), &[BuildingId(20)]);
    }

    #[test]
    fn personality_breaks_an_otherwise_equal_air_defense_choice_without_removing_access() {
        let mut obs = observation(2_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(&mut obs, 3, BuildingKind::Airworks, Vec::new());
        add_producer(&mut obs, 4, BuildingKind::Crucible, Vec::new());
        fill_core(&mut obs, 8);
        obs.enemy_units = vec![enemy_unit(90, UnitKind::Moth)];
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);

        let air = derive(
            &obs,
            &intelligence,
            &profile(95, 50, 50, 10),
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("air-heavy profile still answers the threat");
        let fortified = derive(
            &obs,
            &intelligence,
            &profile(10, 50, 50, 95),
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("fortified profile still answers the threat");
        assert!(matches!(
            air.key_kind().role(),
            Role::AirAir | Role::Interceptor
        ));
        assert_eq!(fortified.key_kind().role(), Role::AntiAir);
        assert_eq!(air.specialty(), Specialty::Air);
        assert_eq!(air.personality_emphasis(), 95);
        assert_eq!(fortified.specialty(), Specialty::Fortification);
        assert_eq!(fortified.personality_emphasis(), 95);
        assert_eq!(air.reason(), StandingForceReason::AirDefense);
        assert_eq!(fortified.reason(), StandingForceReason::AirDefense);
    }

    #[test]
    fn known_defense_creates_siege_demand_but_satisfied_demand_is_silent() {
        let mut obs = observation(3_000);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        add_producer(&mut obs, 3, BuildingKind::Crucible, Vec::new());
        fill_core(&mut obs, 8);
        obs.enemy_buildings = vec![enemy_building(90, BuildingKind::Turret)];
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let (profile, tuning) = prime();

        let proposal = derive(
            &obs,
            &intelligence,
            &profile,
            tuning,
            StandingForceContext::new(&[], &[]),
        )
        .expect("a current fixed defense creates standoff demand");
        assert_eq!(proposal.reason(), StandingForceReason::SiegePressure);
        assert!(matches!(
            proposal.key_kind(),
            UnitKind::Lancer | UnitKind::Bombard | UnitKind::Avalanche
        ));

        obs.my_units.push(unit(80, proposal.key_kind()));
        assert!(
            derive(
                &obs,
                &intelligence,
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .is_none(),
            "matching standoff inventory closes the demand instead of falling back"
        );
    }

    #[test]
    fn forecast_does_not_fund_an_unaffordable_counter() {
        let mut obs = observation(0);
        add_producer(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_producer(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        let mut reclaimer = building(3, BuildingKind::Reclaimer);
        reclaimer.tier = 1;
        obs.my_buildings.push(reclaimer);
        obs.my_queues.push(Vec::new());
        obs.my_queue_progress.push(0);
        fill_core(&mut obs, 8);
        obs.enemy_units = vec![enemy_unit(90, UnitKind::Moth)];
        let mut intelligence = StrategicIntelligence::new();
        intelligence.update(&obs);
        let (profile, tuning) = prime();

        assert!(
            derive(
                &obs,
                &intelligence,
                &profile,
                tuning,
                StandingForceContext::new(&[], &[]),
            )
            .is_none(),
            "completed recurring income is evidence, never current production credit"
        );
    }
}
