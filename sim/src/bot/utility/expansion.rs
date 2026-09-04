//! Pure economic and security quotes for player-facing Foundry expansion.

use chassis::grid::TilePos;
use core::cmp::Reverse;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use super::construction::FoundryExpansionPlan;
use crate::bot::PublicMapBriefing;
use crate::bot::executive::{Intent, building_strength, full_ground_strength, ground_strength};
use crate::bot::intelligence::{ContactEvidence, UnitContact};
use crate::bot::observation::Observation;
use crate::ids::UnitId;
use crate::stats::{BuildingKind, Domain, UnitKind};

const BASE_HORIZON_TICKS: u64 = 3_600;
const GREED_HORIZON_TICKS: u64 = 36;
const SURPLUS_HORIZON_TICKS: u64 = 6;
const SURPLUS_HORIZON_CAP: u32 = 300;
const THREAT_MARGIN_PERCENT: u64 = 15;

fn extractor_gain_per_minute() -> u64 {
    u64::from(
        crate::stats::EXTRACTOR_SUPPORTED_INCOME_PER_MINUTE
            .saturating_sub(crate::stats::EXTRACTOR_REMOTE_INCOME_PER_MINUTE),
    )
}

fn foundry_drip_per_minute() -> u64 {
    u64::from(crate::TICKS_PER_SECOND).saturating_mul(60) / crate::stats::FOUNDRY_DRIP_PERIOD
}

/// One currently visible scrap field whose hauling distance changes when the
/// candidate Foundry exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScrapLogistics {
    pub(super) amount: u32,
    pub(super) old_distance: u32,
    pub(super) new_distance: u32,
}

impl ScrapLogistics {
    fn credit(self) -> u64 {
        let saved_distance = self.old_distance.saturating_sub(self.new_distance);
        if saved_distance == 0 || self.old_distance == 0 {
            return 0;
        }

        u64::from(self.amount).saturating_mul(u64::from(saved_distance))
            / u64::from(self.old_distance)
    }
}

/// An exact command-feasible Foundry site and the external economy it would
/// serve. Terrain, fog, ownership, route, danger, and builder legality remain
/// caller responsibilities.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FoundryCandidate {
    pub(super) anchor: TilePos,
    pub(super) newly_supported_completed_extractors: u32,
    pub(super) current_visible_scrap: Vec<ScrapLogistics>,
}

/// Shared inputs to the economic quote for a set of exact candidate sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExpansionEconomy {
    pub(super) greed: u8,
    pub(super) uncommitted_surplus: u32,
    pub(super) foundry_cost: u32,
    pub(super) ticks_per_minute: u64,
    pub(super) now: u64,
    pub(super) build_ticks: u64,
    pub(super) foundry_drip_start: u64,
}

impl ExpansionEconomy {
    pub(super) fn horizon_ticks(self) -> u64 {
        BASE_HORIZON_TICKS
            .saturating_add(u64::from(self.greed).saturating_mul(GREED_HORIZON_TICKS))
            .saturating_add(
                u64::from(self.uncommitted_surplus.min(SURPLUS_HORIZON_CAP))
                    .saturating_mul(SURPLUS_HORIZON_TICKS),
            )
    }

    /// Prices capital that must be committed before the Foundry can be built.
    /// As that protection is purchased, both cash and the remaining commitment
    /// fall together, so the candidate keeps the same effective horizon.
    fn after_security_commitment(mut self, commitment: u32) -> Self {
        self.uncommitted_surplus = self.uncommitted_surplus.saturating_sub(commitment);
        self
    }
}

/// The deterministic payback quote for one exact candidate site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FoundryOpportunity {
    pub(super) anchor: TilePos,
    horizon_ticks: u64,
    pub(super) recurring_gain_per_minute: u64,
    pub(super) current_scrap_credit: u64,
    pub(super) projected_return: u64,
    pub(super) economically_eligible: bool,
    extractor_gain_per_minute: u64,
    has_external_objective: bool,
}

impl FoundryOpportunity {
    #[cfg(test)]
    fn evaluate(candidate: &FoundryCandidate, economy: ExpansionEconomy) -> Self {
        Self::evaluate_objectives(
            candidate.anchor,
            candidate.newly_supported_completed_extractors,
            candidate.current_visible_scrap.iter().copied(),
            economy,
        )
    }

    #[cfg(test)]
    pub(super) fn evaluate_objectives(
        anchor: TilePos,
        newly_supported_completed_extractors: u32,
        current_visible_scrap: impl IntoIterator<Item = ScrapLogistics>,
        economy: ExpansionEconomy,
    ) -> Self {
        Self::quote_objectives(
            anchor,
            newly_supported_completed_extractors,
            current_visible_scrap,
            economy,
        )
        .0
    }

    pub(super) fn admitted_objectives(
        anchor: TilePos,
        newly_supported_completed_extractors: u32,
        ordinary_frontiers: bool,
        current_visible_scrap: impl IntoIterator<Item = ScrapLogistics>,
        economy: ExpansionEconomy,
    ) -> Option<Self> {
        let (opportunity, has_improving_scrap) = Self::quote_objectives(
            anchor,
            newly_supported_completed_extractors,
            current_visible_scrap,
            economy,
        );
        let serves_admitted_objective =
            newly_supported_completed_extractors > 0 || (ordinary_frontiers && has_improving_scrap);
        (serves_admitted_objective && opportunity.economically_eligible).then_some(opportunity)
    }

    fn quote_objectives(
        anchor: TilePos,
        newly_supported_completed_extractors: u32,
        current_visible_scrap: impl IntoIterator<Item = ScrapLogistics>,
        economy: ExpansionEconomy,
    ) -> (Self, bool) {
        let mut current_scrap_credit = 0u64;
        let mut has_improving_scrap = false;
        for scrap in current_visible_scrap {
            current_scrap_credit = current_scrap_credit.saturating_add(scrap.credit());
            has_improving_scrap |= scrap.amount > 0 && scrap.new_distance < scrap.old_distance;
        }
        let has_external_objective =
            newly_supported_completed_extractors > 0 || has_improving_scrap;
        let extractor_gain_per_minute = u64::from(newly_supported_completed_extractors)
            .saturating_mul(extractor_gain_per_minute());
        (
            Self::quote(
                anchor,
                current_scrap_credit,
                extractor_gain_per_minute,
                has_external_objective,
                economy,
            ),
            has_improving_scrap,
        )
    }

    fn quote(
        anchor: TilePos,
        current_scrap_credit: u64,
        extractor_gain_per_minute: u64,
        has_external_objective: bool,
        economy: ExpansionEconomy,
    ) -> Self {
        let horizon_ticks = economy.horizon_ticks();
        let recurring_gain_per_minute =
            extractor_gain_per_minute.saturating_add(if has_external_objective {
                foundry_drip_per_minute()
            } else {
                0
            });
        let horizon_end = economy.now.saturating_add(horizon_ticks);
        let completed_at = economy.now.saturating_add(economy.build_ticks);
        let extractor_ticks = horizon_end.saturating_sub(completed_at);
        let drip_ticks = horizon_end.saturating_sub(completed_at.max(economy.foundry_drip_start));
        let extractor_return = extractor_gain_per_minute
            .saturating_mul(extractor_ticks)
            .checked_div(economy.ticks_per_minute)
            .unwrap_or(0);
        let drip_return = if has_external_objective {
            foundry_drip_per_minute()
                .saturating_mul(drip_ticks)
                .checked_div(economy.ticks_per_minute)
                .unwrap_or(0)
        } else {
            0
        };
        let recurring_return = extractor_return.saturating_add(drip_return);
        let projected_return = recurring_return.saturating_add(current_scrap_credit);

        Self {
            anchor,
            horizon_ticks,
            recurring_gain_per_minute,
            current_scrap_credit,
            projected_return,
            economically_eligible: has_external_objective
                && projected_return >= u64::from(economy.foundry_cost),
            extractor_gain_per_minute,
            has_external_objective,
        }
    }

    fn after_security_commitment(self, economy: ExpansionEconomy, commitment: u32) -> Self {
        Self::quote(
            self.anchor,
            self.current_scrap_credit,
            self.extractor_gain_per_minute,
            self.has_external_objective,
            economy.after_security_commitment(commitment),
        )
    }

    fn surplus(self, foundry_cost: u32) -> u64 {
        self.projected_return
            .saturating_sub(u64::from(foundry_cost))
    }

    fn rank_key(self) -> (Reverse<bool>, Reverse<u64>, Reverse<u64>, i32, i32) {
        (
            Reverse(self.economically_eligible),
            Reverse(self.projected_return),
            Reverse(self.recurring_gain_per_minute),
            self.anchor.y,
            self.anchor.x,
        )
    }
}

/// Quotes every legal candidate and ranks the most valuable one first. Equal
/// values use row-major map order rather than `TilePos`'s `(x, y)` ordering.
#[cfg(test)]
pub(super) fn foundry_opportunities(
    candidates: &[FoundryCandidate],
    economy: ExpansionEconomy,
) -> Vec<FoundryOpportunity> {
    let opportunities = candidates
        .iter()
        .map(|candidate| FoundryOpportunity::evaluate(candidate, economy))
        .collect();
    rank_foundry_opportunities(opportunities)
}

pub(super) fn rank_foundry_opportunities(
    mut opportunities: Vec<FoundryOpportunity>,
) -> Vec<FoundryOpportunity> {
    opportunities.sort_by_key(|opportunity| opportunity.rank_key());
    opportunities
}

/// Strength in the bot's canonical hp-weighted, full-salvo ground-combat
/// currency. Callers construct it through the shared executive strength
/// helpers rather than from unit counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct GroundStrength(u64);

impl GroundStrength {
    const ZERO: Self = Self(0);

    fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

/// Whether a defended anchor already exists or is the candidate under review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoundryRole {
    Existing,
    Candidate,
}

/// One existing Foundry or the exact candidate being assessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DefendedFoundry {
    anchor: TilePos,
    role: FoundryRole,
}

/// A known route from one threat to one Foundry. Omitted routes are treated as
/// unreachable; the caller owns public-terrain and fog-honest route proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ThreatRoute {
    foundry: usize,
    distance: u32,
}

/// Fog-honest strength evidence for a current or remembered ground threat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GroundThreatEvidence {
    Current(GroundStrength),
    Remembered {
        strength: GroundStrength,
        confidence_per_mille: u16,
    },
}

impl GroundThreatEvidence {
    fn weighted_strength(self) -> GroundStrength {
        match self {
            Self::Current(strength) => strength,
            Self::Remembered {
                strength,
                confidence_per_mille,
            } => GroundStrength(
                strength
                    .0
                    .saturating_mul(u64::from(confidence_per_mille.min(1_000)))
                    / 1_000,
            ),
        }
    }
}

/// One independently observed threat group and its reachable Foundries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroundThreat {
    evidence: GroundThreatEvidence,
    routes: Vec<ThreatRoute>,
}

/// Strength supplied by one completed static defense whose ground weapon
/// envelope covers its assigned Foundry. Filtering unfinished, air-only, and
/// non-covering structures is deliberately a caller responsibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CoveringGroundDefense {
    foundry: usize,
    strength: GroundStrength,
}

/// Inputs to the expansion security quote. The final entry need not be the
/// candidate; role-aware assignment makes equal routes prefer existing bases.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpansionSecurityInput {
    foundries: Vec<DefendedFoundry>,
    threats: Vec<GroundThreat>,
    covering_completed_ground_defenses: Vec<CoveringGroundDefense>,
    network_core: GroundStrength,
    available_mobile_strength: GroundStrength,
    sentinel_strength: GroundStrength,
    forward_toward_uncleared_reachable_enemy_start: bool,
}

/// Per-Foundry threat assignment retained for diagnosis and focused tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FoundrySecurityQuote {
    foundry: usize,
    weighted_threat: GroundStrength,
    threat_with_margin: GroundStrength,
    covering_static_strength: GroundStrength,
    uncovered_threat: GroundStrength,
}

/// The complete security requirement for one candidate expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpansionSecurity {
    foundries: Vec<FoundrySecurityQuote>,
    network_core: GroundStrength,
    forward_reserve: GroundStrength,
    required_mobile_strength: GroundStrength,
    available_mobile_strength: GroundStrength,
    missing_mobile_strength: GroundStrength,
    safe: bool,
}

fn add_percent(strength: GroundStrength, percent: u64) -> GroundStrength {
    let scaled =
        u128::from(strength.0).saturating_mul(u128::from(100u64.saturating_add(percent))) / 100;
    GroundStrength(scaled.min(u128::from(u64::MAX)) as u64)
}

fn assigned_foundry(threat: &GroundThreat, foundries: &[DefendedFoundry]) -> Option<usize> {
    threat
        .routes
        .iter()
        .filter(|route| route.foundry < foundries.len())
        .min_by_key(|route| {
            let foundry = foundries[route.foundry];
            (
                route.distance,
                matches!(foundry.role, FoundryRole::Candidate),
                foundry.anchor.y,
                foundry.anchor.x,
                route.foundry,
            )
        })
        .map(|route| route.foundry)
}

/// Computes the mobile force that must remain after adding the candidate. The
/// economic quote never enters this arithmetic, so a lucrative site may ask
/// the bot to prepare more protection but can never make an unsafe force safe.
fn expansion_security(input: &ExpansionSecurityInput) -> ExpansionSecurity {
    let mut weighted_threat = vec![GroundStrength::ZERO; input.foundries.len()];
    for threat in &input.threats {
        if let Some(foundry) = assigned_foundry(threat, &input.foundries) {
            weighted_threat[foundry] =
                weighted_threat[foundry].saturating_add(threat.evidence.weighted_strength());
        }
    }

    let mut covering_static = vec![GroundStrength::ZERO; input.foundries.len()];
    for defense in &input.covering_completed_ground_defenses {
        if let Some(total) = covering_static.get_mut(defense.foundry) {
            *total = total.saturating_add(defense.strength);
        }
    }

    let foundries = weighted_threat
        .into_iter()
        .zip(covering_static)
        .enumerate()
        .map(|(foundry, (weighted_threat, covering_static_strength))| {
            let threat_with_margin = add_percent(weighted_threat, THREAT_MARGIN_PERCENT);
            FoundrySecurityQuote {
                foundry,
                weighted_threat,
                threat_with_margin,
                covering_static_strength,
                uncovered_threat: threat_with_margin.saturating_sub(covering_static_strength),
            }
        })
        .collect::<Vec<_>>();
    let existing_requirement = foundries
        .iter()
        .filter(|quote| input.foundries[quote.foundry].role == FoundryRole::Existing)
        .map(|quote| quote.uncovered_threat)
        .fold(GroundStrength::ZERO, GroundStrength::saturating_add);
    let candidate_requirement = foundries
        .iter()
        .filter(|quote| input.foundries[quote.foundry].role == FoundryRole::Candidate)
        .map(|quote| quote.uncovered_threat)
        .fold(GroundStrength::ZERO, GroundStrength::saturating_add);
    let forward_reserve = if input.forward_toward_uncleared_reachable_enemy_start {
        input.sentinel_strength
    } else {
        GroundStrength::ZERO
    };
    let protected_network = input.network_core.max(existing_requirement);
    let protected_candidate = candidate_requirement.max(forward_reserve);
    let required_mobile_strength = protected_network.saturating_add(protected_candidate);
    let missing_mobile_strength =
        required_mobile_strength.saturating_sub(input.available_mobile_strength);

    ExpansionSecurity {
        foundries,
        network_core: input.network_core,
        forward_reserve,
        required_mobile_strength,
        available_mobile_strength: input.available_mobile_strength,
        missing_mobile_strength,
        safe: missing_mobile_strength == GroundStrength::ZERO,
    }
}

/// Whether an economically worthwhile candidate is rejected, should prepare
/// its missing protection, or can be built now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExpansionDisposition {
    Reject,
    Prepare {
        missing_mobile_strength: GroundStrength,
    },
    Build,
}

fn expansion_disposition(
    opportunity: FoundryOpportunity,
    security: &ExpansionSecurity,
    missing_security_scrap: u32,
    expansion_appetite: u8,
    foundry_cost: u32,
) -> ExpansionDisposition {
    if !opportunity.economically_eligible {
        ExpansionDisposition::Reject
    } else if security.safe {
        ExpansionDisposition::Build
    } else if u64::from(missing_security_scrap)
        > opportunity
            .surplus(foundry_cost)
            .saturating_mul(u64::from(expansion_appetite))
            / 100
    {
        ExpansionDisposition::Reject
    } else {
        ExpansionDisposition::Prepare {
            missing_mobile_strength: security.missing_mobile_strength,
        }
    }
}

/// Fog-honest inputs used to decide whether the first viable expansion in an
/// economically ranked list can be protected.
pub(super) struct ExpansionAssessmentContext<'a> {
    pub(super) obs: &'a Observation,
    pub(super) public_map: &'a PublicMapBriefing,
    pub(super) unit_contacts: &'a [UnitContact],
    pub(super) uncleared_hostile_starts: &'a [crate::bot::StartingFoundry],
    pub(super) combat_core_exclusions: &'a [UnitId],
    pub(super) same_think_intents: &'a [Intent],
    pub(super) minimum_core_equivalents: u32,
    pub(super) own_strength_scale: u16,
    pub(super) economy: ExpansionEconomy,
}

/// The first economically ranked expansion that is either safe now or worth
/// preparing protection for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FoundryExpansionAssessment {
    pub(super) plan: FoundryExpansionPlan,
    security: ExpansionSecurity,
    pub(super) disposition: ExpansionDisposition,
    pub(super) missing_security_scrap: u32,
    /// Actual ordinary-core strength needed for the difficulty's perceived
    /// strength to meet the security quote.
    pub(super) preparation_target_strength: u64,
}

/// A security-priced candidate before an exact safe builder is bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FoundryExpansionQuote {
    opportunity: FoundryOpportunity,
    security: ExpansionSecurity,
    disposition: ExpansionDisposition,
    missing_security_scrap: u32,
    preparation_target_strength: u64,
}

impl FoundryExpansionQuote {
    pub(super) fn anchor(&self) -> TilePos {
        self.opportunity.anchor
    }

    pub(super) fn bind(self, builder: UnitId) -> FoundryExpansionAssessment {
        FoundryExpansionAssessment {
            plan: FoundryExpansionPlan {
                anchor: self.opportunity.anchor,
                builder,
                opportunity: self.opportunity,
            },
            security: self.security,
            disposition: self.disposition,
            missing_security_scrap: self.missing_security_scrap,
            preparation_target_strength: self.preparation_target_strength,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KnownGroundThreat {
    tile: TilePos,
    evidence: GroundThreatEvidence,
    source_rank: (bool, u64, i32, i32),
}

impl KnownGroundThreat {
    fn from_contact(contact: &UnitContact, now: u64) -> Option<Self> {
        let strength = GroundStrength(ground_strength(contact.kind, contact.hp));
        if contact.kind.stats().domain != Domain::Ground || strength == GroundStrength::ZERO {
            return None;
        }
        let current = contact.evidence == ContactEvidence::Current;
        Some(Self {
            tile: contact.tile,
            evidence: if current {
                GroundThreatEvidence::Current(strength)
            } else {
                GroundThreatEvidence::Remembered {
                    strength,
                    confidence_per_mille: contact.confidence_at(now),
                }
            },
            source_rank: (current, contact.last_seen, -contact.tile.y, -contact.tile.x),
        })
    }

    fn from_current(unit: &crate::bot::observation::UnitObs, now: u64) -> Option<Self> {
        let strength = GroundStrength(crate::bot::executive::unit_strength(unit));
        if unit.kind.stats().domain != Domain::Ground || strength == GroundStrength::ZERO {
            return None;
        }
        Some(Self {
            tile: unit.tile,
            evidence: GroundThreatEvidence::Current(strength),
            source_rank: (true, now, -unit.tile.y, -unit.tile.x),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PublicGroundDistances {
    width: i32,
    height: i32,
    distances: Vec<u32>,
}

/// Exact dynamic ground exclusions for expansion logistics routing.
///
/// The dense membership form makes every Dijkstra edge check constant-time,
/// while equality remains an exact invalidation key for both projected danger
/// and the policy's independently retained contested-work regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BlockedGroundLayout {
    width: i32,
    height: i32,
    blocked: Vec<bool>,
}

impl BlockedGroundLayout {
    pub(super) fn from_predicate(
        public_map: &PublicMapBriefing,
        mut blocked: impl FnMut(TilePos) -> bool,
    ) -> Self {
        let width = public_map.map_width();
        let height = public_map.map_height();
        let cells = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let blocked = (0..cells)
            .map(|index| {
                let index = i32::try_from(index).unwrap_or(i32::MAX);
                let tile = if width > 0 {
                    TilePos::new(index % width, index / width)
                } else {
                    TilePos::new(-1, -1)
                };
                blocked(tile)
            })
            .collect();
        Self {
            width,
            height,
            blocked,
        }
    }

    pub(super) fn contains(&self, tile: TilePos) -> bool {
        PublicGroundDistances::index_for(self.width, self.height, tile)
            .and_then(|index| self.blocked.get(index))
            .copied()
            .unwrap_or(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DangerAwareDistanceGeneration {
    blocked: BlockedGroundLayout,
    fields: BTreeMap<TilePos, Arc<PublicGroundDistances>>,
}

/// Bounded routing memoization for the expansion planner.
///
/// Dynamic logistics fields retain only the sources present in the latest
/// exact danger generation. Terrain-only threat fields likewise retain only
/// currently known threat positions. Authored start fields are permanent for
/// one briefing and therefore bounded by its starting-seat count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ExpansionRoutingCache {
    public_map: Option<PublicMapBriefing>,
    danger_aware: Option<DangerAwareDistanceGeneration>,
    threat_fields: BTreeMap<TilePos, Arc<PublicGroundDistances>>,
    start_fields: BTreeMap<TilePos, Arc<PublicGroundDistances>>,
    #[cfg(test)]
    builds: ExpansionRoutingBuilds,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct ExpansionRoutingBuilds {
    pub(super) danger_aware: usize,
    pub(super) threats: usize,
    pub(super) starts: usize,
}

impl ExpansionRoutingCache {
    fn prepare_map(&mut self, public_map: &PublicMapBriefing) {
        if self
            .public_map
            .as_ref()
            .is_some_and(|cached| cached == public_map)
        {
            return;
        }
        self.public_map = Some(public_map.clone());
        self.danger_aware = None;
        self.threat_fields.clear();
        self.start_fields.clear();
    }

    pub(super) fn danger_aware_fields(
        &mut self,
        public_map: &PublicMapBriefing,
        blocked: BlockedGroundLayout,
        sources: impl IntoIterator<Item = TilePos>,
    ) -> Vec<(TilePos, Arc<PublicGroundDistances>)> {
        self.prepare_map(public_map);
        let mut sources = sources.into_iter().collect::<Vec<_>>();
        sources.sort_unstable_by_key(|tile| (tile.y, tile.x));
        sources.dedup();
        if self
            .danger_aware
            .as_ref()
            .is_none_or(|generation| generation.blocked != blocked)
        {
            self.danger_aware = Some(DangerAwareDistanceGeneration {
                blocked,
                fields: BTreeMap::new(),
            });
        }
        let generation = self
            .danger_aware
            .as_mut()
            .expect("a danger-aware generation was prepared");
        generation.fields.retain(|source, _| {
            sources
                .binary_search_by_key(&(source.y, source.x), |tile| (tile.y, tile.x))
                .is_ok()
        });
        for &source in &sources {
            generation.fields.entry(source).or_insert_with(|| {
                #[cfg(test)]
                {
                    self.builds.danger_aware += 1;
                }
                Arc::new(PublicGroundDistances::from_sources_avoiding(
                    public_map,
                    [source],
                    |tile| generation.blocked.contains(tile),
                ))
            });
        }
        sources
            .into_iter()
            .filter_map(|source| {
                generation
                    .fields
                    .get(&source)
                    .map(|field| (source, Arc::clone(field)))
            })
            .collect()
    }

    fn threat_fields(
        &mut self,
        public_map: &PublicMapBriefing,
        sources: impl IntoIterator<Item = TilePos>,
    ) -> BTreeMap<TilePos, Arc<PublicGroundDistances>> {
        self.prepare_map(public_map);
        let mut sources = sources.into_iter().collect::<Vec<_>>();
        sources.sort_unstable_by_key(|tile| (tile.y, tile.x));
        sources.dedup();
        self.threat_fields.retain(|source, _| {
            sources
                .binary_search_by_key(&(source.y, source.x), |tile| (tile.y, tile.x))
                .is_ok()
        });
        for &source in &sources {
            self.threat_fields.entry(source).or_insert_with(|| {
                #[cfg(test)]
                {
                    self.builds.threats += 1;
                }
                Arc::new(PublicGroundDistances::from_sources(public_map, [source]))
            });
        }
        self.threat_fields.clone()
    }

    fn start_fields(
        &mut self,
        public_map: &PublicMapBriefing,
        starts: &[crate::bot::StartingFoundry],
    ) -> Vec<Arc<PublicGroundDistances>> {
        self.prepare_map(public_map);
        starts
            .iter()
            .map(|start| {
                Arc::clone(self.start_fields.entry(start.anchor).or_insert_with(|| {
                    #[cfg(test)]
                    {
                        self.builds.starts += 1;
                    }
                    Arc::new(PublicGroundDistances::from_sources(
                        public_map,
                        foundry_footprint_tiles(start.anchor),
                    ))
                }))
            })
            .collect()
    }

    #[cfg(test)]
    pub(super) fn build_count(&self) -> ExpansionRoutingBuilds {
        self.builds
    }

    #[cfg(test)]
    pub(super) fn retained_field_counts(&self) -> (usize, usize, usize) {
        (
            self.danger_aware
                .as_ref()
                .map_or(0, |generation| generation.fields.len()),
            self.threat_fields.len(),
            self.start_fields.len(),
        )
    }
}

impl PublicGroundDistances {
    pub(super) fn from_sources(
        public_map: &PublicMapBriefing,
        sources: impl IntoIterator<Item = TilePos>,
    ) -> Self {
        Self::from_sources_avoiding(public_map, sources, |_| false)
    }

    pub(super) fn from_sources_avoiding(
        public_map: &PublicMapBriefing,
        sources: impl IntoIterator<Item = TilePos>,
        mut blocked: impl FnMut(TilePos) -> bool,
    ) -> Self {
        let width = public_map.map_width();
        let height = public_map.map_height();
        let cells = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let mut distances = vec![u32::MAX; cells];
        const MAX_STEP_COST: usize = 14;
        let mut frontier = (0..=MAX_STEP_COST)
            .map(|_| VecDeque::new())
            .collect::<Vec<VecDeque<(u32, TilePos)>>>();
        let mut queued = 0usize;
        for source in sources {
            if !Self::ground_open(public_map, source) || blocked(source) {
                continue;
            }
            let Some(index) = Self::index_for(width, height, source) else {
                continue;
            };
            if distances[index] == 0 {
                continue;
            }
            distances[index] = 0;
            frontier[0].push_back((0, source));
            queued += 1;
        }

        let mut current_distance = 0u32;
        while queued > 0 {
            let bucket_index = usize::try_from(
                current_distance % u32::try_from(MAX_STEP_COST + 1).expect("small bucket count"),
            )
            .expect("bucket index fits usize");
            let Some(&(distance, current)) = frontier[bucket_index].front() else {
                current_distance = current_distance.saturating_add(1);
                continue;
            };
            if distance > current_distance {
                current_distance = current_distance.saturating_add(1);
                continue;
            }
            frontier[bucket_index].pop_front();
            queued -= 1;
            let Some(current_index) = Self::index_for(width, height, current) else {
                continue;
            };
            if distances[current_index] != distance {
                continue;
            }
            for (dx, dy, step) in [
                (-1, 0, 10),
                (1, 0, 10),
                (0, -1, 10),
                (0, 1, 10),
                (-1, -1, 14),
                (1, -1, 14),
                (-1, 1, 14),
                (1, 1, 14),
            ] {
                let next = current.offset(dx, dy);
                if !Self::ground_open(public_map, next)
                    || blocked(next)
                    || (dx != 0
                        && dy != 0
                        && (!Self::ground_open(public_map, current.offset(dx, 0))
                            || blocked(current.offset(dx, 0))
                            || !Self::ground_open(public_map, current.offset(0, dy))
                            || blocked(current.offset(0, dy))))
                {
                    continue;
                }
                let Some(next_index) = Self::index_for(width, height, next) else {
                    continue;
                };
                let next_distance = distance.saturating_add(step);
                if next_distance < distances[next_index] {
                    distances[next_index] = next_distance;
                    let bucket = usize::try_from(
                        next_distance
                            % u32::try_from(MAX_STEP_COST + 1).expect("small bucket count"),
                    )
                    .expect("bucket index fits usize");
                    frontier[bucket].push_back((next_distance, next));
                    queued += 1;
                }
            }
        }

        Self {
            width,
            height,
            distances,
        }
    }

    fn ground_open(public_map: &PublicMapBriefing, tile: TilePos) -> bool {
        public_map
            .terrain_at(tile)
            .is_some_and(|terrain| !terrain.blocks_ground())
    }

    fn index_for(width: i32, height: i32, tile: TilePos) -> Option<usize> {
        if tile.x < 0 || tile.y < 0 || tile.x >= width || tile.y >= height {
            return None;
        }
        usize::try_from(tile.y)
            .ok()?
            .checked_mul(usize::try_from(width).ok()?)?
            .checked_add(usize::try_from(tile.x).ok()?)
    }

    pub(super) fn footprint_distance(&self, anchor: TilePos, size: (i32, i32)) -> Option<u32> {
        (0..size.1)
            .flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
            .filter_map(|tile| {
                Self::index_for(self.width, self.height, tile)
                    .and_then(|index| self.distances.get(index).copied())
                    .filter(|distance| *distance != u32::MAX)
            })
            .min()
    }
}

fn scale_strength(strength: GroundStrength, scale: u16) -> GroundStrength {
    GroundStrength(
        (u128::from(strength.0) * u128::from(scale) / 10_000).min(u128::from(u64::MAX)) as u64,
    )
}

fn actual_strength_for_perceived(required: GroundStrength, scale: u16) -> u64 {
    if required == GroundStrength::ZERO {
        return 0;
    }
    if scale == 0 {
        return u64::MAX;
    }
    let numerator = u128::from(required.0).saturating_mul(10_000);
    let scaled = numerator.div_ceil(u128::from(scale));
    scaled.min(u128::from(u64::MAX)) as u64
}

fn missing_security_scrap(
    missing: GroundStrength,
    perceived_sentinel_strength: GroundStrength,
) -> u32 {
    if missing == GroundStrength::ZERO {
        return 0;
    }
    if perceived_sentinel_strength == GroundStrength::ZERO {
        return u32::MAX;
    }
    let sentinels = missing.0.div_ceil(perceived_sentinel_strength.0);
    u32::try_from(sentinels)
        .unwrap_or(u32::MAX)
        .saturating_mul(UnitKind::Sentinel.stats().cost)
}

fn foundry_footprint_tiles(anchor: TilePos) -> impl Iterator<Item = TilePos> {
    let size = BuildingKind::Foundry.base_stats().size;
    (0..size.1).flat_map(move |dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
}

fn known_ground_threats(
    obs: &Observation,
    contacts: &[UnitContact],
) -> BTreeMap<UnitId, KnownGroundThreat> {
    let mut threats = BTreeMap::new();
    for contact in contacts {
        let Some(candidate) = KnownGroundThreat::from_contact(contact, obs.tick) else {
            continue;
        };
        match threats.entry(contact.id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut entry)
                if candidate.source_rank > entry.get().source_rank =>
            {
                entry.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(_) => {}
        }
    }
    for unit in &obs.enemy_units {
        if let Some(current) = KnownGroundThreat::from_current(unit, obs.tick) {
            threats.insert(unit.id, current);
        }
    }
    threats
}

fn existing_foundries(obs: &Observation) -> Vec<DefendedFoundry> {
    let mut foundries = obs
        .my_buildings
        .iter()
        .filter(|building| building.hp > 0 && building.kind == BuildingKind::Foundry)
        .map(|building| (building.anchor, building.id))
        .collect::<Vec<_>>();
    foundries.sort_unstable_by_key(|(anchor, id)| (anchor.y, anchor.x, *id));
    foundries
        .into_iter()
        .map(|(anchor, _)| DefendedFoundry {
            anchor,
            role: FoundryRole::Existing,
        })
        .collect()
}

fn defended_foundries(existing: &[DefendedFoundry], candidate: TilePos) -> Vec<DefendedFoundry> {
    existing
        .iter()
        .copied()
        .chain(std::iter::once(DefendedFoundry {
            anchor: candidate,
            role: FoundryRole::Candidate,
        }))
        .collect()
}

struct RoutedGroundThreat {
    evidence: GroundThreatEvidence,
    distances: Arc<PublicGroundDistances>,
}

fn ground_threat_distance_fields(
    public_map: &PublicMapBriefing,
    known: &BTreeMap<UnitId, KnownGroundThreat>,
    routing_cache: &mut ExpansionRoutingCache,
) -> Vec<RoutedGroundThreat> {
    let fields = routing_cache.threat_fields(public_map, known.values().map(|threat| threat.tile));
    known
        .values()
        .filter_map(|threat| {
            Some(RoutedGroundThreat {
                evidence: threat.evidence,
                distances: Arc::clone(fields.get(&threat.tile)?),
            })
        })
        .collect()
}

fn routed_ground_threats(
    routed: &[RoutedGroundThreat],
    foundries: &[DefendedFoundry],
) -> Vec<GroundThreat> {
    let foundry_size = BuildingKind::Foundry.base_stats().size;
    routed
        .iter()
        .map(|threat| {
            let routes = foundries
                .iter()
                .enumerate()
                .filter_map(|(foundry, defended)| {
                    threat
                        .distances
                        .footprint_distance(defended.anchor, foundry_size)
                        .map(|distance| ThreatRoute { foundry, distance })
                })
                .collect();
            GroundThreat {
                evidence: threat.evidence,
                routes,
            }
        })
        .collect()
}

fn hostile_start_distance_fields(
    public_map: &PublicMapBriefing,
    starts: &[crate::bot::StartingFoundry],
    routing_cache: &mut ExpansionRoutingCache,
) -> Vec<Arc<PublicGroundDistances>> {
    routing_cache.start_fields(public_map, starts)
}

fn candidate_advances_toward_uncleared_start(
    start_distances: &[Arc<PublicGroundDistances>],
    foundries: &[DefendedFoundry],
) -> bool {
    let foundry_size = BuildingKind::Foundry.base_stats().size;
    let Some(candidate_index) = foundries
        .iter()
        .position(|foundry| foundry.role == FoundryRole::Candidate)
    else {
        return false;
    };
    start_distances.iter().any(|distances| {
        let Some(candidate_distance) =
            distances.footprint_distance(foundries[candidate_index].anchor, foundry_size)
        else {
            return false;
        };
        foundries
            .iter()
            .enumerate()
            .filter(|(_, foundry)| foundry.role == FoundryRole::Existing)
            .filter_map(|(_, foundry)| distances.footprint_distance(foundry.anchor, foundry_size))
            .all(|existing_distance| candidate_distance < existing_distance)
    })
}

fn assigned_covering_defenses(
    obs: &Observation,
    public_map: &PublicMapBriefing,
    foundries: &[DefendedFoundry],
    preliminary: &ExpansionSecurity,
    own_strength_scale: u16,
) -> Vec<CoveringGroundDefense> {
    let foundry_size = BuildingKind::Foundry.base_stats().size;
    let mut remaining = preliminary
        .foundries
        .iter()
        .map(|quote| quote.threat_with_margin)
        .collect::<Vec<_>>();
    let mut defenses = obs
        .my_buildings
        .iter()
        .filter_map(|defense| {
            let strength = scale_strength(
                GroundStrength(building_strength(defense)),
                own_strength_scale,
            );
            if strength == GroundStrength::ZERO {
                return None;
            }
            let covers = foundries
                .iter()
                .enumerate()
                .filter_map(|(index, foundry)| {
                    super::defense::completed_ground_defense_covers_asset(
                        obs,
                        public_map,
                        defense,
                        foundry.anchor,
                        foundry_size,
                    )
                    .then_some(index)
                })
                .collect::<Vec<_>>();
            (!covers.is_empty()).then_some((defense, strength, covers))
        })
        .collect::<Vec<_>>();
    defenses.sort_unstable_by_key(|(defense, strength, covers)| {
        (
            covers.len(),
            Reverse(strength.0),
            defense.anchor.y,
            defense.anchor.x,
            defense.id,
        )
    });

    let mut assigned = Vec::new();
    for (defense, strength, covers) in defenses {
        let Some(foundry) = covers.into_iter().min_by_key(|index| {
            let foundry = foundries[*index];
            (
                Reverse(remaining[*index].0.min(strength.0)),
                matches!(foundry.role, FoundryRole::Candidate),
                defense.anchor.chebyshev(foundry.anchor),
                foundry.anchor.y,
                foundry.anchor.x,
                *index,
            )
        }) else {
            continue;
        };
        remaining[foundry] = remaining[foundry].saturating_sub(strength);
        assigned.push(CoveringGroundDefense { foundry, strength });
    }
    assigned
}

struct ExpansionSecurityWorld {
    existing_foundries: Vec<DefendedFoundry>,
    threats: Vec<RoutedGroundThreat>,
    hostile_start_distances: Vec<Arc<PublicGroundDistances>>,
    network_core: GroundStrength,
    available_mobile_strength: GroundStrength,
    sentinel_strength: GroundStrength,
}

impl ExpansionSecurityWorld {
    fn observe(
        context: &ExpansionAssessmentContext<'_>,
        routing_cache: &mut ExpansionRoutingCache,
    ) -> Self {
        let sentinel_strength = GroundStrength(full_ground_strength(UnitKind::Sentinel));
        let network_core = GroundStrength(
            sentinel_strength
                .0
                .saturating_mul(u64::from(context.minimum_core_equivalents)),
        );
        let projected_core = super::production::combat_core_status_for_strength(
            context.obs,
            context.combat_core_exclusions,
            context.same_think_intents,
            0,
        )
        .projected_strength;
        let known = known_ground_threats(context.obs, context.unit_contacts);
        Self {
            existing_foundries: existing_foundries(context.obs),
            threats: ground_threat_distance_fields(context.public_map, &known, routing_cache),
            hostile_start_distances: hostile_start_distance_fields(
                context.public_map,
                context.uncleared_hostile_starts,
                routing_cache,
            ),
            network_core,
            available_mobile_strength: scale_strength(
                GroundStrength(projected_core),
                context.own_strength_scale,
            ),
            sentinel_strength,
        }
    }
}

fn security_for_anchor(
    anchor: TilePos,
    context: &ExpansionAssessmentContext<'_>,
    world: &ExpansionSecurityWorld,
) -> (ExpansionSecurity, u32, u64) {
    let foundries = defended_foundries(&world.existing_foundries, anchor);
    let threats = routed_ground_threats(&world.threats, &foundries);
    let forward_toward_uncleared_reachable_enemy_start =
        candidate_advances_toward_uncleared_start(&world.hostile_start_distances, &foundries);
    let mut input = ExpansionSecurityInput {
        foundries,
        threats,
        covering_completed_ground_defenses: Vec::new(),
        network_core: world.network_core,
        available_mobile_strength: world.available_mobile_strength,
        sentinel_strength: world.sentinel_strength,
        forward_toward_uncleared_reachable_enemy_start,
    };
    let preliminary = expansion_security(&input);
    input.covering_completed_ground_defenses = assigned_covering_defenses(
        context.obs,
        context.public_map,
        &input.foundries,
        &preliminary,
        context.own_strength_scale,
    );
    let security = expansion_security(&input);
    let perceived_sentinel_strength =
        scale_strength(world.sentinel_strength, context.own_strength_scale);
    let missing_scrap = missing_security_scrap(
        security.missing_mobile_strength,
        perceived_sentinel_strength,
    );
    let target = actual_strength_for_perceived(
        security.required_mobile_strength,
        context.own_strength_scale,
    );
    (security, missing_scrap, target)
}

/// Reprices each candidate after its own missing protection, then restores the
/// deterministic economic order. Builder routing follows that order until an
/// exact viable worker and route are found.
#[cfg(test)]
pub(super) fn quote_foundry_expansions(
    opportunities: Vec<FoundryOpportunity>,
    context: &ExpansionAssessmentContext<'_>,
) -> Vec<FoundryExpansionQuote> {
    let mut routing_cache = ExpansionRoutingCache::default();
    quote_foundry_expansions_cached(opportunities, context, &mut routing_cache)
}

pub(super) fn quote_foundry_expansions_cached(
    opportunities: Vec<FoundryOpportunity>,
    context: &ExpansionAssessmentContext<'_>,
    routing_cache: &mut ExpansionRoutingCache,
) -> Vec<FoundryExpansionQuote> {
    let world = ExpansionSecurityWorld::observe(context, routing_cache);
    let mut quotes = opportunities
        .into_iter()
        .filter_map(|opportunity| {
            let (security, missing_security_scrap, preparation_target_strength) =
                security_for_anchor(opportunity.anchor, context, &world);
            let opportunity =
                opportunity.after_security_commitment(context.economy, missing_security_scrap);
            let disposition = expansion_disposition(
                opportunity,
                &security,
                missing_security_scrap,
                context.economy.greed,
                context.economy.foundry_cost,
            );
            (disposition != ExpansionDisposition::Reject).then_some(FoundryExpansionQuote {
                opportunity,
                security,
                disposition,
                missing_security_scrap,
                preparation_target_strength,
            })
        })
        .collect::<Vec<_>>();
    quotes.sort_by_key(|quote| quote.opportunity.rank_key());
    quotes
}

/// Rechecks current protection for one already-admitted expansion without
/// rediscovering or repricing its frozen economic opportunity.
pub(super) fn assess_retained_foundry(
    opportunity: FoundryOpportunity,
    builder: UnitId,
    context: &ExpansionAssessmentContext<'_>,
    routing_cache: &mut ExpansionRoutingCache,
) -> FoundryExpansionAssessment {
    let world = ExpansionSecurityWorld::observe(context, routing_cache);
    let (security, missing_security_scrap, preparation_target_strength) =
        security_for_anchor(opportunity.anchor, context, &world);
    let disposition = expansion_disposition(
        opportunity,
        &security,
        missing_security_scrap,
        context.economy.greed,
        context.economy.foundry_cost,
    );
    FoundryExpansionQuote {
        opportunity,
        security,
        disposition,
        missing_security_scrap,
        preparation_target_strength,
    }
    .bind(builder)
}

/// Test adapter that binds prebuilt plans after candidate-local security
/// reprices their economic order.
#[cfg(test)]
pub(super) fn assess_foundry_expansions(
    plans: Vec<FoundryExpansionPlan>,
    context: ExpansionAssessmentContext<'_>,
) -> Option<FoundryExpansionAssessment> {
    let opportunities = plans.iter().map(|plan| plan.opportunity).collect();
    for quote in quote_foundry_expansions(opportunities, &context) {
        if let Some(plan) = plans.iter().find(|plan| plan.anchor == quote.anchor()) {
            return Some(quote.bind(plan.builder));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKS_PER_MINUTE: u64 = 1_200;
    const FOUNDRY_COST: u32 = 300;

    fn economy(greed: u8, uncommitted_surplus: u32) -> ExpansionEconomy {
        ExpansionEconomy {
            greed,
            uncommitted_surplus,
            foundry_cost: FOUNDRY_COST,
            ticks_per_minute: TICKS_PER_MINUTE,
            now: 0,
            build_ticks: 600,
            foundry_drip_start: 0,
        }
    }

    fn candidate(anchor: TilePos, extractors: u32, scrap: Vec<ScrapLogistics>) -> FoundryCandidate {
        FoundryCandidate {
            anchor,
            newly_supported_completed_extractors: extractors,
            current_visible_scrap: scrap,
        }
    }

    #[test]
    fn horizon_uses_greed_and_only_the_first_three_hundred_surplus() {
        assert_eq!(economy(0, 0).horizon_ticks(), 3_600);
        assert_eq!(economy(50, 100).horizon_ticks(), 6_000);
        assert_eq!(economy(100, 300).horizon_ticks(), 9_000);
        assert_eq!(economy(100, u32::MAX).horizon_ticks(), 9_000);
    }

    #[test]
    fn security_preparation_cannot_shorten_its_own_effective_horizon() {
        let wealthy = ExpansionEconomy {
            greed: 77,
            uncommitted_surplus: 192,
            foundry_cost: FOUNDRY_COST,
            ticks_per_minute: TICKS_PER_MINUTE,
            now: 0,
            build_ticks: 600,
            foundry_drip_start: 2_400,
        };
        let opportunity = FoundryOpportunity::evaluate(
            &candidate(
                TilePos::new(8, 3),
                4,
                vec![ScrapLogistics {
                    amount: 249,
                    old_distance: 1,
                    new_distance: 0,
                }],
            ),
            wealthy,
        );
        let before_purchase = opportunity.after_security_commitment(wealthy, 900);

        let after_two_sentinels = ExpansionEconomy {
            uncommitted_surplus: 12,
            ..wealthy
        };
        let repriced = opportunity.after_security_commitment(after_two_sentinels, 720);

        assert_eq!(before_purchase, repriced);
        assert_eq!(before_purchase.projected_return, 1_469);
        assert_eq!(
            before_purchase.surplus(FOUNDRY_COST) * 77 / 100,
            900,
            "the exact ten-Sentinel project remains affordable after each depth-two queue fill"
        );
    }

    #[test]
    fn security_cost_is_removed_from_wealth_before_preparation_is_admitted() {
        let economy = ExpansionEconomy {
            greed: 77,
            uncommitted_surplus: 192,
            foundry_cost: FOUNDRY_COST,
            ticks_per_minute: TICKS_PER_MINUTE,
            now: 0,
            build_ticks: 600,
            foundry_drip_start: 2_400,
        };
        let opportunity =
            FoundryOpportunity::evaluate(&candidate(TilePos::new(8, 3), 4, Vec::new()), economy)
                .after_security_commitment(economy, 900);
        let mut input = security_input();
        input.available_mobile_strength = GroundStrength(0);
        let security = expansion_security(&input);

        assert_eq!(opportunity.projected_return, 1_220);
        assert_eq!(
            expansion_disposition(opportunity, &security, 900, 77, FOUNDRY_COST),
            ExpansionDisposition::Reject,
            "cash promised to the ten-Sentinel screen is not still counted as wealth"
        );
    }

    #[test]
    fn scrap_credit_prices_only_the_distance_actually_saved() {
        let quote = FoundryOpportunity::evaluate(
            &candidate(
                TilePos::new(4, 7),
                0,
                vec![
                    ScrapLogistics {
                        amount: 100,
                        old_distance: 10,
                        new_distance: 4,
                    },
                    ScrapLogistics {
                        amount: 90,
                        old_distance: 5,
                        new_distance: 8,
                    },
                    ScrapLogistics {
                        amount: u32::MAX,
                        old_distance: 0,
                        new_distance: 0,
                    },
                ],
            ),
            economy(0, 0),
        );

        assert_eq!(quote.current_scrap_credit, 60);
        assert_eq!(quote.recurring_gain_per_minute, foundry_drip_per_minute());
        assert_eq!(quote.projected_return, 110);
        assert!(!quote.economically_eligible);
    }

    #[test]
    fn extractor_candidate_keeps_scrap_credit_when_scrap_only_frontiers_are_closed() {
        let anchor = TilePos::new(8, 8);
        let scraps = vec![
            ScrapLogistics {
                amount: 600,
                old_distance: 20,
                new_distance: 5,
            },
            ScrapLogistics {
                amount: 90,
                old_distance: 10,
                new_distance: 12,
            },
        ];
        let materialized =
            FoundryOpportunity::evaluate(&candidate(anchor, 1, scraps.clone()), economy(50, 0));

        let streamed = FoundryOpportunity::admitted_objectives(
            anchor,
            1,
            false,
            scraps.clone(),
            economy(50, 0),
        )
        .expect("Extractor support admits the candidate without generic frontier expansion");

        assert_eq!(streamed, materialized);
        assert_eq!(streamed.current_scrap_credit, 450);
        assert!(
            FoundryOpportunity::admitted_objectives(anchor, 0, false, scraps, economy(50, 0),)
                .is_none(),
            "the same scrap cannot independently admit a frontier while that channel is closed"
        );
    }

    #[test]
    fn foundry_drip_cannot_justify_an_empty_foundry() {
        let quote = FoundryOpportunity::evaluate(
            &candidate(TilePos::new(3, 3), 0, Vec::new()),
            economy(100, u32::MAX),
        );

        assert_eq!(quote.recurring_gain_per_minute, 0);
        assert_eq!(quote.projected_return, 0);
        assert!(!quote.economically_eligible);
    }

    #[test]
    fn completed_extractor_gain_and_drip_cover_the_foundry_cost() {
        let quote = FoundryOpportunity::evaluate(
            &candidate(TilePos::new(8, 8), 1, Vec::new()),
            economy(50, 0),
        );

        assert_eq!(quote.horizon_ticks, 5_400);
        assert_eq!(quote.recurring_gain_per_minute, 80);
        assert_eq!(quote.projected_return, 320);
        assert!(quote.economically_eligible);
    }

    #[test]
    fn construction_and_delayed_drip_do_not_earn_before_the_foundry_exists() {
        let quote = FoundryOpportunity::evaluate(
            &candidate(TilePos::new(8, 8), 1, Vec::new()),
            ExpansionEconomy {
                now: 1_000,
                build_ticks: 600,
                foundry_drip_start: 4_000,
                ..economy(50, 0)
            },
        );

        assert_eq!(quote.horizon_ticks, 5_400);
        assert_eq!(quote.projected_return, 280);
        assert!(!quote.economically_eligible);
    }

    #[test]
    fn all_candidates_are_ranked_by_value_then_row_major_position() {
        let candidates = vec![
            candidate(TilePos::new(9, 1), 1, Vec::new()),
            candidate(TilePos::new(2, 7), 3, Vec::new()),
            candidate(TilePos::new(4, 1), 1, Vec::new()),
            candidate(TilePos::new(0, 0), 0, Vec::new()),
        ];

        let ranked = foundry_opportunities(&candidates, economy(50, 0));

        assert_eq!(ranked[0].anchor, TilePos::new(2, 7));
        assert_eq!(ranked[1].anchor, TilePos::new(4, 1));
        assert_eq!(ranked[2].anchor, TilePos::new(9, 1));
        assert_eq!(ranked[3].anchor, TilePos::new(0, 0));
    }

    #[test]
    fn zero_tick_minute_is_rejected_without_panicking() {
        let quote = FoundryOpportunity::evaluate(
            &candidate(TilePos::new(1, 1), u32::MAX, Vec::new()),
            ExpansionEconomy {
                ticks_per_minute: 0,
                ..economy(u8::MAX, u32::MAX)
            },
        );

        assert_eq!(quote.projected_return, 0);
        assert!(!quote.economically_eligible);
    }

    fn foundry(anchor: TilePos, role: FoundryRole) -> DefendedFoundry {
        DefendedFoundry { anchor, role }
    }

    fn current(strength: u64, routes: &[(usize, u32)]) -> GroundThreat {
        GroundThreat {
            evidence: GroundThreatEvidence::Current(GroundStrength(strength)),
            routes: routes
                .iter()
                .map(|&(foundry, distance)| ThreatRoute { foundry, distance })
                .collect(),
        }
    }

    fn security_input() -> ExpansionSecurityInput {
        ExpansionSecurityInput {
            foundries: vec![
                foundry(TilePos::new(1, 1), FoundryRole::Existing),
                foundry(TilePos::new(9, 1), FoundryRole::Candidate),
            ],
            threats: Vec::new(),
            covering_completed_ground_defenses: Vec::new(),
            network_core: GroundStrength(1_000),
            available_mobile_strength: GroundStrength(1_000),
            sentinel_strength: GroundStrength(200),
            forward_toward_uncleared_reachable_enemy_start: false,
        }
    }

    #[test]
    fn equal_routes_prefer_an_existing_foundry_over_the_candidate() {
        let mut input = security_input();
        input.threats = vec![current(1_000, &[(1, 5), (0, 5)])];

        let quote = expansion_security(&input);

        assert_eq!(quote.foundries[0].weighted_threat, GroundStrength(1_000));
        assert_eq!(quote.foundries[1].weighted_threat, GroundStrength::ZERO);
    }

    #[test]
    fn assignment_is_independent_of_route_input_order() {
        let mut first = security_input();
        first
            .foundries
            .insert(1, foundry(TilePos::new(5, 1), FoundryRole::Existing));
        first.threats = vec![current(300, &[(2, 9), (1, 4), (0, 4)])];
        let mut second = first.clone();
        second.threats[0].routes.reverse();

        let first_quote = expansion_security(&first);
        let second_quote = expansion_security(&second);

        assert_eq!(first_quote, second_quote);
        assert_eq!(
            first_quote.foundries[0].weighted_threat,
            GroundStrength(300)
        );
    }

    #[test]
    fn remembered_threat_is_confidence_weighted_and_clamped() {
        let mut input = security_input();
        input.threats = vec![
            GroundThreat {
                evidence: GroundThreatEvidence::Remembered {
                    strength: GroundStrength(1_000),
                    confidence_per_mille: 400,
                },
                routes: vec![ThreatRoute {
                    foundry: 1,
                    distance: 2,
                }],
            },
            GroundThreat {
                evidence: GroundThreatEvidence::Remembered {
                    strength: GroundStrength(100),
                    confidence_per_mille: u16::MAX,
                },
                routes: vec![ThreatRoute {
                    foundry: 1,
                    distance: 3,
                }],
            },
        ];

        let quote = expansion_security(&input);

        assert_eq!(quote.foundries[1].weighted_threat, GroundStrength(500));
        assert_eq!(quote.foundries[1].threat_with_margin, GroundStrength(575));
    }

    #[test]
    fn only_declared_covering_static_strength_offsets_local_threat() {
        let mut input = security_input();
        input.threats = vec![current(1_000, &[(1, 1)])];
        input.covering_completed_ground_defenses = vec![
            CoveringGroundDefense {
                foundry: 1,
                strength: GroundStrength(900),
            },
            CoveringGroundDefense {
                foundry: 0,
                strength: GroundStrength(400),
            },
            CoveringGroundDefense {
                foundry: usize::MAX,
                strength: GroundStrength(u64::MAX),
            },
        ];
        input.available_mobile_strength = GroundStrength(1_250);

        let quote = expansion_security(&input);

        assert_eq!(quote.foundries[1].threat_with_margin, GroundStrength(1_150));
        assert_eq!(quote.foundries[1].uncovered_threat, GroundStrength(250));
        assert_eq!(quote.required_mobile_strength, GroundStrength(1_250));
        assert!(quote.safe);
    }

    #[test]
    fn network_core_cannot_be_replaced_by_static_defense() {
        let mut input = security_input();
        input.covering_completed_ground_defenses = vec![CoveringGroundDefense {
            foundry: 0,
            strength: GroundStrength(u64::MAX),
        }];
        input.available_mobile_strength = GroundStrength(999);

        let quote = expansion_security(&input);

        assert_eq!(quote.required_mobile_strength, GroundStrength(1_000));
        assert_eq!(quote.missing_mobile_strength, GroundStrength(1));
        assert!(!quote.safe);
    }

    #[test]
    fn existing_threat_below_the_network_core_does_not_stack_with_the_floor() {
        let mut input = security_input();
        input.threats = vec![current(400, &[(0, 1)])];

        let quote = expansion_security(&input);

        assert_eq!(quote.foundries[0].threat_with_margin, GroundStrength(460));
        assert_eq!(quote.network_core, GroundStrength(1_000));
        assert_eq!(quote.required_mobile_strength, GroundStrength(1_000));
        assert!(quote.safe);
    }

    #[test]
    fn forward_candidate_adds_exactly_one_sentinel_equivalent() {
        let mut input = security_input();
        input.forward_toward_uncleared_reachable_enemy_start = true;
        input.available_mobile_strength = GroundStrength(1_199);

        let short = expansion_security(&input);
        assert_eq!(short.forward_reserve, GroundStrength(200));
        assert_eq!(short.required_mobile_strength, GroundStrength(1_200));
        assert_eq!(short.missing_mobile_strength, GroundStrength(1));

        input.available_mobile_strength = GroundStrength(1_200);
        assert!(expansion_security(&input).safe);
    }

    #[test]
    fn candidate_threat_below_the_forward_reserve_does_not_stack_with_it() {
        let mut input = security_input();
        input.threats = vec![current(100, &[(1, 1)])];
        input.forward_toward_uncleared_reachable_enemy_start = true;
        input.available_mobile_strength = GroundStrength(1_200);

        let quote = expansion_security(&input);

        assert_eq!(quote.foundries[1].threat_with_margin, GroundStrength(115));
        assert_eq!(quote.forward_reserve, GroundStrength(200));
        assert_eq!(quote.required_mobile_strength, GroundStrength(1_200));
        assert!(quote.safe);
    }

    #[test]
    fn disconnected_threat_does_not_burden_any_foundry() {
        let mut input = security_input();
        input.threats = vec![current(900, &[(usize::MAX, 0)])];

        let quote = expansion_security(&input);

        assert_eq!(quote.required_mobile_strength, input.network_core);
        assert!(quote.safe);
    }

    #[test]
    fn arithmetic_saturates_at_extreme_strengths() {
        let mut input = security_input();
        input.network_core = GroundStrength(u64::MAX - 5);
        input.available_mobile_strength = GroundStrength(u64::MAX);
        input.threats = vec![current(u64::MAX, &[(1, 1)])];
        input.forward_toward_uncleared_reachable_enemy_start = true;

        let quote = expansion_security(&input);

        assert_eq!(
            quote.foundries[1].threat_with_margin,
            GroundStrength(u64::MAX)
        );
        assert_eq!(quote.required_mobile_strength, GroundStrength(u64::MAX));
        assert!(quote.safe);
    }

    #[test]
    fn economic_value_can_request_preparation_but_cannot_waive_safety() {
        let valuable = FoundryOpportunity::evaluate(
            &candidate(TilePos::new(9, 1), 3, Vec::new()),
            economy(100, 300),
        );
        let mut input = security_input();
        input.available_mobile_strength = GroundStrength(800);
        let unsafe_quote = expansion_security(&input);

        assert_eq!(
            expansion_disposition(valuable, &unsafe_quote, 40, 100, FOUNDRY_COST),
            ExpansionDisposition::Prepare {
                missing_mobile_strength: GroundStrength(200),
            }
        );

        input.available_mobile_strength = GroundStrength(1_000);
        let safe_quote = expansion_security(&input);
        assert_eq!(
            expansion_disposition(valuable, &safe_quote, 0, 100, FOUNDRY_COST),
            ExpansionDisposition::Build
        );
    }

    #[test]
    fn marginal_value_does_not_fund_disproportionate_security() {
        let marginal = FoundryOpportunity {
            anchor: TilePos::new(9, 1),
            horizon_ticks: 4_000,
            recurring_gain_per_minute: 80,
            current_scrap_credit: 0,
            projected_return: 320,
            economically_eligible: true,
            extractor_gain_per_minute: 60,
            has_external_objective: true,
        };
        let mut input = security_input();
        input.available_mobile_strength = GroundStrength(800);
        let unsafe_quote = expansion_security(&input);

        assert_eq!(
            expansion_disposition(marginal, &unsafe_quote, 21, 100, FOUNDRY_COST),
            ExpansionDisposition::Reject
        );
        assert_eq!(
            expansion_disposition(marginal, &unsafe_quote, 20, 100, FOUNDRY_COST),
            ExpansionDisposition::Prepare {
                missing_mobile_strength: GroundStrength(200),
            }
        );
    }

    #[test]
    fn expansion_appetite_controls_whether_the_same_security_cost_is_affordable() {
        let opportunity = FoundryOpportunity {
            anchor: TilePos::new(9, 1),
            horizon_ticks: 4_000,
            recurring_gain_per_minute: 80,
            current_scrap_credit: 0,
            projected_return: 500,
            economically_eligible: true,
            extractor_gain_per_minute: 60,
            has_external_objective: true,
        };
        let mut input = security_input();
        input.available_mobile_strength = GroundStrength(800);
        let security = expansion_security(&input);

        assert_eq!(
            expansion_disposition(opportunity, &security, 100, 49, FOUNDRY_COST),
            ExpansionDisposition::Reject
        );
        assert_eq!(
            expansion_disposition(opportunity, &security, 100, 50, FOUNDRY_COST),
            ExpansionDisposition::Prepare {
                missing_mobile_strength: GroundStrength(200),
            }
        );
    }

    #[test]
    fn uneconomic_candidate_is_rejected_even_when_safe() {
        let empty = FoundryOpportunity::evaluate(
            &candidate(TilePos::new(9, 1), 0, Vec::new()),
            economy(100, 300),
        );
        let safe_quote = expansion_security(&security_input());

        assert_eq!(
            expansion_disposition(empty, &safe_quote, 0, 100, FOUNDRY_COST),
            ExpansionDisposition::Reject
        );
    }

    fn briefing(
        width: i32,
        height: i32,
        walls: impl IntoIterator<Item = TilePos>,
        starts: Vec<crate::bot::StartingFoundry>,
    ) -> PublicMapBriefing {
        let mut non_ground_terrain = walls
            .into_iter()
            .map(|tile| (tile, crate::map::Terrain::Rock))
            .collect::<Vec<_>>();
        non_ground_terrain.sort_unstable_by_key(|(tile, _)| (tile.y, tile.x));
        PublicMapBriefing {
            map_width: width,
            map_height: height,
            starting_foundries: starts,
            teams: vec![Some(0), Some(1)],
            non_ground_terrain,
            extractor_frames: Vec::new(),
            initial_scrap: Vec::new(),
        }
    }

    fn reference_ground_distances(
        public_map: &PublicMapBriefing,
        sources: impl IntoIterator<Item = TilePos>,
        blocked: &BlockedGroundLayout,
    ) -> PublicGroundDistances {
        use std::collections::BinaryHeap;

        let width = public_map.map_width();
        let height = public_map.map_height();
        let cells = usize::try_from(width * height).expect("small test map");
        let mut distances = vec![u32::MAX; cells];
        let mut frontier = BinaryHeap::new();
        for source in sources {
            if !PublicGroundDistances::ground_open(public_map, source) || blocked.contains(source) {
                continue;
            }
            let Some(index) = PublicGroundDistances::index_for(width, height, source) else {
                continue;
            };
            if distances[index] == 0 {
                continue;
            }
            distances[index] = 0;
            frontier.push(Reverse((0u32, source.y, source.x)));
        }
        while let Some(Reverse((distance, y, x))) = frontier.pop() {
            let current = TilePos::new(x, y);
            let Some(current_index) = PublicGroundDistances::index_for(width, height, current)
            else {
                continue;
            };
            if distances[current_index] != distance {
                continue;
            }
            for (dx, dy, step) in [
                (-1, 0, 10),
                (1, 0, 10),
                (0, -1, 10),
                (0, 1, 10),
                (-1, -1, 14),
                (1, -1, 14),
                (-1, 1, 14),
                (1, 1, 14),
            ] {
                let next = current.offset(dx, dy);
                if !PublicGroundDistances::ground_open(public_map, next)
                    || blocked.contains(next)
                    || (dx != 0
                        && dy != 0
                        && (!PublicGroundDistances::ground_open(public_map, current.offset(dx, 0))
                            || blocked.contains(current.offset(dx, 0))
                            || !PublicGroundDistances::ground_open(
                                public_map,
                                current.offset(0, dy),
                            )
                            || blocked.contains(current.offset(0, dy))))
                {
                    continue;
                }
                let Some(next_index) = PublicGroundDistances::index_for(width, height, next) else {
                    continue;
                };
                let next_distance = distance.saturating_add(step);
                if next_distance < distances[next_index] {
                    distances[next_index] = next_distance;
                    frontier.push(Reverse((next_distance, next.y, next.x)));
                }
            }
        }
        PublicGroundDistances {
            width,
            height,
            distances,
        }
    }

    #[test]
    fn bounded_bucket_routes_match_the_reference_dijkstra_exactly() {
        let walls = (0..9)
            .filter(|y| !matches!(y, 2 | 7))
            .map(|y| TilePos::new(6, y))
            .chain([TilePos::new(3, 4), TilePos::new(4, 3)]);
        let public_map = briefing(13, 9, walls, Vec::new());
        let blocked = BlockedGroundLayout::from_predicate(&public_map, |tile| {
            matches!((tile.x, tile.y), (8, 2) | (8, 3) | (9, 3))
        });
        let sources = [
            TilePos::new(1, 1),
            TilePos::new(11, 7),
            TilePos::new(1, 1),
            TilePos::new(-1, -1),
        ];

        let actual = PublicGroundDistances::from_sources_avoiding(&public_map, sources, |tile| {
            blocked.contains(tile)
        });
        let reference = reference_ground_distances(&public_map, sources, &blocked);

        assert_eq!(actual, reference);
    }

    #[test]
    fn routing_cache_reuses_exact_generations_and_keeps_dynamic_fields_bounded() {
        let public_map = briefing(18, 10, [], Vec::new());
        let clear = BlockedGroundLayout::from_predicate(&public_map, |_| false);
        let mut cache = ExpansionRoutingCache::default();
        let first = cache.danger_aware_fields(
            &public_map,
            clear.clone(),
            [TilePos::new(3, 3), TilePos::new(14, 7)],
        );
        assert_eq!(cache.build_count().danger_aware, 2);
        assert_eq!(cache.retained_field_counts().0, 2);

        let repeated = cache.danger_aware_fields(
            &public_map,
            clear,
            [TilePos::new(14, 7), TilePos::new(3, 3), TilePos::new(3, 3)],
        );
        assert_eq!(cache.build_count().danger_aware, 2);
        assert_eq!(first.len(), repeated.len());
        assert!(
            first
                .iter()
                .zip(&repeated)
                .all(|(left, right)| { left.0 == right.0 && Arc::ptr_eq(&left.1, &right.1) })
        );

        cache.danger_aware_fields(
            &public_map,
            BlockedGroundLayout::from_predicate(&public_map, |tile| tile == TilePos::new(9, 5)),
            [TilePos::new(14, 7)],
        );
        assert_eq!(cache.build_count().danger_aware, 3);
        assert_eq!(cache.retained_field_counts().0, 1);
    }

    #[test]
    fn row_major_source_retention_reuses_nonmonotone_coordinates() {
        let public_map = briefing(18, 10, [], Vec::new());
        let clear = BlockedGroundLayout::from_predicate(&public_map, |_| false);
        let sources = [TilePos::new(14, 2), TilePos::new(3, 7)];
        let mut cache = ExpansionRoutingCache::default();

        cache.danger_aware_fields(&public_map, clear.clone(), sources);
        cache.threat_fields(&public_map, sources);
        let before = cache.build_count();
        cache.danger_aware_fields(&public_map, clear, sources.into_iter().rev());
        cache.threat_fields(&public_map, sources.into_iter().rev());

        assert_eq!(cache.build_count(), before);
        assert_eq!(cache.retained_field_counts(), (2, 2, 0));
    }

    #[test]
    fn dynamic_danger_never_invalidates_terrain_only_threat_or_start_fields() {
        let starts = vec![
            crate::bot::StartingFoundry {
                player: crate::ids::PlayerId(0),
                anchor: TilePos::new(1, 1),
            },
            crate::bot::StartingFoundry {
                player: crate::ids::PlayerId(1),
                anchor: TilePos::new(14, 6),
            },
        ];
        let public_map = briefing(18, 10, [], starts.clone());
        let mut cache = ExpansionRoutingCache::default();
        cache.threat_fields(&public_map, [TilePos::new(4, 4), TilePos::new(12, 4)]);
        cache.start_fields(&public_map, &starts);
        let before = cache.build_count();

        for blocked in [TilePos::new(7, 4), TilePos::new(8, 4)] {
            cache.danger_aware_fields(
                &public_map,
                BlockedGroundLayout::from_predicate(&public_map, |tile| tile == blocked),
                [TilePos::new(5, 4)],
            );
            cache.threat_fields(&public_map, [TilePos::new(12, 4), TilePos::new(4, 4)]);
            cache.start_fields(&public_map, &starts[1..]);
        }

        let after = cache.build_count();
        assert_eq!(after.threats, before.threats);
        assert_eq!(after.starts, before.starts);
        assert_eq!(cache.retained_field_counts(), (1, 2, 2));
    }

    #[test]
    fn routing_cache_replaces_departed_threats_and_resets_for_any_map_change() {
        let public_map = briefing(18, 10, [], Vec::new());
        let mut cache = ExpansionRoutingCache::default();
        cache.threat_fields(&public_map, [TilePos::new(3, 3), TilePos::new(9, 3)]);
        cache.threat_fields(&public_map, [TilePos::new(9, 3), TilePos::new(14, 3)]);
        assert_eq!(cache.build_count().threats, 3);
        assert_eq!(cache.retained_field_counts().1, 2);

        let changed_map = briefing(18, 10, [TilePos::new(8, 5)], Vec::new());
        cache.threat_fields(&changed_map, [TilePos::new(9, 3)]);
        assert_eq!(cache.build_count().threats, 4);
        assert_eq!(cache.retained_field_counts(), (0, 1, 0));
    }

    fn building(
        id: u32,
        player: u8,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> crate::bot::observation::BuildingObs {
        crate::bot::observation::BuildingObs {
            id: crate::ids::BuildingId(id),
            player: crate::ids::PlayerId(player),
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn unit(
        id: u32,
        player: u8,
        kind: UnitKind,
        tile: TilePos,
        hp: u32,
    ) -> crate::bot::observation::UnitObs {
        crate::bot::observation::UnitObs {
            id: UnitId(id),
            player: crate::ids::PlayerId(player),
            kind,
            tile,
            hp,
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

    fn observed_map(width: i32, height: i32, home: TilePos) -> Observation {
        let cells = usize::try_from(width * height).expect("small test map");
        Observation {
            map_width: width,
            map_height: height,
            my_buildings: vec![building(0, 0, BuildingKind::Foundry, home)],
            my_queues: vec![Vec::new()],
            visible: vec![true; cells],
            explored: vec![true; cells],
            ..Observation::default()
        }
    }

    fn expansion_plan(anchor: TilePos, current_scrap_credit: u64) -> FoundryExpansionPlan {
        FoundryExpansionPlan {
            anchor,
            builder: UnitId(99),
            opportunity: FoundryOpportunity::quote(
                anchor,
                current_scrap_credit,
                60,
                true,
                economy(100, 0),
            ),
        }
    }

    fn assessment_context<'a>(
        obs: &'a Observation,
        public_map: &'a PublicMapBriefing,
        contacts: &'a [UnitContact],
        starts: &'a [crate::bot::StartingFoundry],
        minimum_core_equivalents: u32,
        own_strength_scale: u16,
    ) -> ExpansionAssessmentContext<'a> {
        ExpansionAssessmentContext {
            obs,
            public_map,
            unit_contacts: contacts,
            uncleared_hostile_starts: starts,
            combat_core_exclusions: &[],
            same_think_intents: &[],
            minimum_core_equivalents,
            own_strength_scale,
            economy: economy(100, 0),
        }
    }

    #[test]
    fn security_rejection_falls_through_to_the_next_economic_plan() {
        let obs = observed_map(20, 8, TilePos::new(2, 3));
        let enemy_start = crate::bot::StartingFoundry {
            player: crate::ids::PlayerId(1),
            anchor: TilePos::new(16, 3),
        };
        let starts = [enemy_start];
        let public_map = briefing(20, 8, [], starts.to_vec());

        let mut context = assessment_context(&obs, &public_map, &[], &starts, 0, 10_000);
        context.economy = economy(0, 0);
        let assessment = assess_foundry_expansions(
            vec![
                expansion_plan(TilePos::new(9, 3), 100),
                expansion_plan(TilePos::new(0, 3), 600),
            ],
            context,
        )
        .expect("the rear plan is safe");

        assert_eq!(assessment.plan.anchor, TilePos::new(0, 3));
        assert_eq!(assessment.disposition, ExpansionDisposition::Build);
        assert_eq!(assessment.security.forward_reserve, GroundStrength::ZERO);
    }

    #[test]
    fn candidate_local_security_cost_can_reverse_the_economic_order() {
        let obs = observed_map(20, 8, TilePos::new(2, 3));
        let starts = [crate::bot::StartingFoundry {
            player: crate::ids::PlayerId(1),
            anchor: TilePos::new(16, 3),
        }];
        let public_map = briefing(20, 8, [], starts.to_vec());
        let pricing = economy(100, 300);
        let plan = |anchor, current_scrap_credit| FoundryExpansionPlan {
            anchor,
            builder: UnitId(99),
            opportunity: FoundryOpportunity::quote(anchor, current_scrap_credit, 60, true, pricing),
        };
        let forward = plan(TilePos::new(9, 3), 400);
        let rear = plan(TilePos::new(0, 3), 390);
        assert!(
            forward.opportunity.projected_return > rear.opportunity.projected_return,
            "the forward site must begin as the better raw economic opportunity"
        );

        let mut context = assessment_context(&obs, &public_map, &[], &starts, 0, 10_000);
        context.economy = pricing;
        let quotes =
            quote_foundry_expansions(vec![forward.opportunity, rear.opportunity], &context);

        assert_eq!(quotes.len(), 2);
        assert_eq!(quotes[0].anchor(), rear.anchor);
        assert_eq!(quotes[0].disposition, ExpansionDisposition::Build);
        assert_eq!(quotes[1].anchor(), forward.anchor);
        assert_eq!(
            quotes[1].disposition,
            ExpansionDisposition::Prepare {
                missing_mobile_strength: GroundStrength(full_ground_strength(UnitKind::Sentinel)),
            }
        );
        assert!(
            quotes[0].opportunity.projected_return > quotes[1].opportunity.projected_return,
            "charging the forward site's screen must deterministically rerank the survivors"
        );
    }

    #[test]
    fn current_observation_overrides_the_same_remembered_unit_contact() {
        let mut obs = observed_map(16, 8, TilePos::new(1, 3));
        obs.tick = 300;
        obs.enemy_units = vec![unit(7, 1, UnitKind::Sentinel, TilePos::new(11, 3), 30)];
        let contacts = [UnitContact {
            id: UnitId(7),
            player: crate::ids::PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: TilePos::new(3, 3),
            hp: 60,
            grounded: false,
            last_seen: 0,
            evidence: ContactEvidence::Remembered,
        }];
        let public_map = briefing(16, 8, [], Vec::new());

        let assessment = assess_foundry_expansions(
            vec![expansion_plan(TilePos::new(8, 3), 2_000)],
            assessment_context(&obs, &public_map, &contacts, &[], 0, 10_000),
        )
        .expect("the valuable plan can prepare defense");

        assert_eq!(
            assessment.security.foundries[1].weighted_threat,
            GroundStrength(ground_strength(UnitKind::Sentinel, 30))
        );
    }

    #[test]
    fn paid_unfinished_foundry_remains_a_defended_existing_asset() {
        let mut obs = observed_map(20, 8, TilePos::new(2, 3));
        let mut paid_site = building(1, 0, BuildingKind::Foundry, TilePos::new(8, 3));
        paid_site.built = false;
        paid_site.hp /= 2;
        obs.my_buildings.push(paid_site);
        obs.my_queues.push(Vec::new());
        obs.enemy_units = vec![unit(
            7,
            1,
            UnitKind::Sentinel,
            TilePos::new(11, 3),
            UnitKind::Sentinel.stats().max_hp,
        )];
        let public_map = briefing(20, 8, [], Vec::new());

        let assessment = assess_foundry_expansions(
            vec![expansion_plan(TilePos::new(15, 3), 2_000)],
            assessment_context(&obs, &public_map, &[], &[], 0, 10_000),
        )
        .expect("the valuable plan can prepare its network defense");

        assert_eq!(assessment.security.foundries.len(), 3);
        assert_eq!(
            assessment.security.foundries[1].weighted_threat,
            GroundStrength(full_ground_strength(UnitKind::Sentinel)),
            "the nearer paid site must retain the threat rather than disappearing from security"
        );
        assert_eq!(
            assessment.security.foundries[2].weighted_threat,
            GroundStrength::ZERO
        );
    }

    #[test]
    fn own_strength_error_prices_enough_real_units_to_clear_the_floor() {
        let mut obs = observed_map(12, 8, TilePos::new(3, 3));
        obs.my_units = vec![unit(
            1,
            0,
            UnitKind::Sentinel,
            TilePos::new(4, 2),
            UnitKind::Sentinel.stats().max_hp,
        )];
        let public_map = briefing(12, 8, [], Vec::new());
        let sentinel_strength = full_ground_strength(UnitKind::Sentinel);

        let assessment = assess_foundry_expansions(
            vec![expansion_plan(TilePos::new(0, 3), 1_000)],
            assessment_context(&obs, &public_map, &[], &[], 1, 9_400),
        )
        .expect("the plan can afford one additional screen unit");

        assert_eq!(
            assessment.disposition,
            ExpansionDisposition::Prepare {
                missing_mobile_strength: GroundStrength(
                    sentinel_strength - sentinel_strength * 9_400 / 10_000,
                ),
            }
        );
        assert_eq!(
            assessment.missing_security_scrap,
            UnitKind::Sentinel.stats().cost
        );
        assert!(assessment.preparation_target_strength > sentinel_strength);
    }

    #[test]
    fn assessment_excludes_reserved_core_but_counts_queued_and_same_think_core() {
        let mut obs = observed_map(12, 8, TilePos::new(3, 3));
        obs.my_units = (1..=3)
            .map(|id| {
                unit(
                    id,
                    0,
                    UnitKind::Sentinel,
                    TilePos::new(3 + i32::try_from(id).expect("small id"), 2),
                    UnitKind::Sentinel.stats().max_hp,
                )
            })
            .collect();
        obs.my_queues[0].push(UnitKind::Sentinel);
        let exclusions = [UnitId(3)];
        let same_think = [Intent::TrainAt {
            building: crate::ids::BuildingId(0),
            kind: UnitKind::Sentinel,
        }];
        let public_map = briefing(12, 8, [], Vec::new());
        let mut context = assessment_context(&obs, &public_map, &[], &[], 4, 10_000);
        context.combat_core_exclusions = &exclusions;
        context.same_think_intents = &same_think;

        let assessment =
            assess_foundry_expansions(vec![expansion_plan(TilePos::new(0, 3), 1_000)], context)
                .expect("two live, one queued, and one planned Sentinel meet the floor");
        let sentinel_strength = full_ground_strength(UnitKind::Sentinel);

        assert_eq!(
            assessment.security.available_mobile_strength,
            GroundStrength(sentinel_strength * 4)
        );
        assert_eq!(assessment.disposition, ExpansionDisposition::Build);
    }

    #[test]
    fn one_covering_defense_is_never_credited_to_two_foundries() {
        let mut obs = observed_map(15, 8, TilePos::new(2, 3));
        obs.my_buildings
            .push(building(1, 0, BuildingKind::Turret, TilePos::new(5, 3)));
        obs.my_queues.push(Vec::new());
        obs.enemy_units = vec![unit(
            8,
            1,
            UnitKind::Sentinel,
            TilePos::new(11, 3),
            UnitKind::Sentinel.stats().max_hp,
        )];
        let public_map = briefing(15, 8, [], Vec::new());

        let assessment = assess_foundry_expansions(
            vec![expansion_plan(TilePos::new(8, 3), 1_000)],
            assessment_context(&obs, &public_map, &[], &[], 0, 10_000),
        )
        .expect("the turret makes the candidate safe");
        let credited = assessment
            .security
            .foundries
            .iter()
            .map(|quote| quote.covering_static_strength.0)
            .sum::<u64>();

        assert_eq!(credited, building_strength(&obs.my_buildings[1]));
        assert_eq!(assessment.disposition, ExpansionDisposition::Build);
    }

    #[test]
    fn invalid_static_defenses_do_not_secure_an_expansion() {
        let cases = [
            (
                "unfinished ground defense",
                BuildingKind::Turret,
                TilePos::new(5, 3),
                false,
            ),
            (
                "ground-incompatible flak",
                BuildingKind::FlakTurret,
                TilePos::new(5, 3),
                true,
            ),
            (
                "out-of-envelope ground defense",
                BuildingKind::Turret,
                TilePos::new(18, 3),
                true,
            ),
        ];

        for (label, kind, anchor, built) in cases {
            let mut obs = observed_map(20, 8, TilePos::new(2, 3));
            let mut defense = building(1, 0, kind, anchor);
            defense.built = built;
            obs.my_buildings.push(defense);
            obs.my_queues.push(Vec::new());
            obs.enemy_units = vec![unit(
                8,
                1,
                UnitKind::Sentinel,
                TilePos::new(11, 3),
                UnitKind::Sentinel.stats().max_hp,
            )];
            let public_map = briefing(20, 8, [], Vec::new());

            let assessment = assess_foundry_expansions(
                vec![expansion_plan(TilePos::new(8, 3), 1_000)],
                assessment_context(&obs, &public_map, &[], &[], 0, 10_000),
            )
            .unwrap_or_else(|| panic!("{label} must not erase a valuable opportunity"));

            assert_eq!(
                assessment.security.foundries[1].covering_static_strength,
                GroundStrength::ZERO,
                "{label}"
            );
            assert!(
                matches!(assessment.disposition, ExpansionDisposition::Prepare { .. }),
                "{label} must leave the candidate in preparation"
            );
        }
    }

    #[test]
    fn constrained_defenses_are_assigned_before_flexible_ones_regardless_of_id() {
        let existing = DefendedFoundry {
            anchor: TilePos::new(2, 3),
            role: FoundryRole::Existing,
        };
        let candidate = DefendedFoundry {
            anchor: TilePos::new(8, 3),
            role: FoundryRole::Candidate,
        };
        let foundries = [existing, candidate];
        let preliminary = expansion_security(&ExpansionSecurityInput {
            foundries: foundries.to_vec(),
            threats: vec![
                GroundThreat {
                    evidence: GroundThreatEvidence::Current(GroundStrength(1_000)),
                    routes: vec![ThreatRoute {
                        foundry: 0,
                        distance: 0,
                    }],
                },
                GroundThreat {
                    evidence: GroundThreatEvidence::Current(GroundStrength(1_000)),
                    routes: vec![ThreatRoute {
                        foundry: 1,
                        distance: 0,
                    }],
                },
            ],
            covering_completed_ground_defenses: Vec::new(),
            network_core: GroundStrength::ZERO,
            available_mobile_strength: GroundStrength::ZERO,
            sentinel_strength: GroundStrength::ZERO,
            forward_toward_uncleared_reachable_enemy_start: false,
        });
        let public_map = briefing(16, 9, [], Vec::new());

        let assigned_strengths = |flexible_id, constrained_id| {
            let mut obs = observed_map(16, 9, existing.anchor);
            obs.my_buildings.extend([
                building(flexible_id, 0, BuildingKind::Turret, TilePos::new(5, 3)),
                building(constrained_id, 0, BuildingKind::Turret, TilePos::new(0, 3)),
            ]);
            obs.my_buildings
                .sort_unstable_by_key(|building| building.id);
            let mut totals = [0u64; 2];
            for defense in
                assigned_covering_defenses(&obs, &public_map, &foundries, &preliminary, 10_000)
            {
                totals[defense.foundry] =
                    totals[defense.foundry].saturating_add(defense.strength.0);
            }
            totals
        };

        let flexible_first = assigned_strengths(1, 2);
        let constrained_first = assigned_strengths(2, 1);
        assert_eq!(flexible_first, constrained_first);
        assert!(
            flexible_first.into_iter().all(|strength| strength > 0),
            "both bases need assigned coverage, got {flexible_first:?}"
        );
    }

    #[test]
    fn a_static_wall_prevents_a_geometrically_forward_candidate_from_reserving_a_screen() {
        let obs = observed_map(16, 8, TilePos::new(2, 3));
        let enemy_start = crate::bot::StartingFoundry {
            player: crate::ids::PlayerId(1),
            anchor: TilePos::new(12, 3),
        };
        let starts = [enemy_start];
        let wall = (0..8).map(|y| TilePos::new(8, y));
        let public_map = briefing(16, 8, wall, starts.to_vec());

        let assessment = assess_foundry_expansions(
            vec![expansion_plan(TilePos::new(6, 3), 1_000)],
            assessment_context(&obs, &public_map, &[], &starts, 0, 10_000),
        )
        .expect("a rear-network candidate remains safe");

        assert_eq!(assessment.security.forward_reserve, GroundStrength::ZERO);
        assert_eq!(assessment.disposition, ExpansionDisposition::Build);
    }
}
