//! Fog-honest valuation and approach-lane scoring for static defenses.

use super::*;
use crate::bot::intelligence::ContactEvidence;
use crate::bot::{Orientation, PublicMapBriefing, StartingFoundry};
use crate::map::Terrain;
use crate::stats::{
    CHARGE_TRIGGER_RADIUS, PATH_EXPANSION_CAP, SAPPER_CONTACT_RANGE, SCRAP_NODE_AMOUNT, WeaponStats,
};
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::{CARDINALS, DIAGONALS};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

const INTERCEPTION_DEPTH: usize = 8;
const MAX_BARRICADE_DETOUR_COST: u32 = 40;
const MAX_BARRICADE_RESOURCE_DETOUR_COST: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DefenseDomain {
    Ground,
    Air,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefenseEvidenceScope {
    #[cfg(test)]
    Strategic,
    CurrentEmergency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefenseSiteSearch {
    Best,
    #[cfg(test)]
    Any,
}

#[derive(Clone, Copy)]
struct DefenseEvidence<'a> {
    unit_contacts: &'a [UnitContact],
    building_contacts: &'a [BuildingContact],
    scope: DefenseEvidenceScope,
    search: DefenseSiteSearch,
}

impl<'a> DefenseEvidence<'a> {
    #[cfg(test)]
    const fn strategic(
        unit_contacts: &'a [UnitContact],
        building_contacts: &'a [BuildingContact],
    ) -> Self {
        Self {
            unit_contacts,
            building_contacts,
            scope: DefenseEvidenceScope::Strategic,
            search: DefenseSiteSearch::Best,
        }
    }

    #[cfg(test)]
    const fn strategic_existence(
        unit_contacts: &'a [UnitContact],
        building_contacts: &'a [BuildingContact],
    ) -> Self {
        Self {
            unit_contacts,
            building_contacts,
            scope: DefenseEvidenceScope::Strategic,
            search: DefenseSiteSearch::Any,
        }
    }

    const fn current_emergency(
        unit_contacts: &'a [UnitContact],
        building_contacts: &'a [BuildingContact],
    ) -> Self {
        Self {
            unit_contacts,
            building_contacts,
            scope: DefenseEvidenceScope::CurrentEmergency,
            search: DefenseSiteSearch::Best,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DefenseProfile {
    kind: BuildingKind,
    domain: DefenseDomain,
    candidate_reach: i32,
}

impl DefenseProfile {
    fn for_kind(kind: BuildingKind) -> Option<Self> {
        match kind {
            BuildingKind::Turret => Some(Self {
                kind,
                domain: DefenseDomain::Ground,
                candidate_reach: 5,
            }),
            BuildingKind::Bastion => Some(Self {
                kind,
                domain: DefenseDomain::Ground,
                candidate_reach: 9,
            }),
            BuildingKind::FlakTurret => Some(Self {
                kind,
                domain: DefenseDomain::Air,
                candidate_reach: 6,
            }),
            BuildingKind::ScuttleCharge => Some(Self {
                kind,
                domain: DefenseDomain::Ground,
                candidate_reach: 0,
            }),
            BuildingKind::Barricade => Some(Self {
                kind,
                domain: DefenseDomain::Ground,
                candidate_reach: 0,
            }),
            _ => None,
        }
    }

    fn footprint(self, anchor: TilePos) -> PlacementFootprint {
        PlacementFootprint {
            anchor,
            size: self.kind.base_stats().size,
            blocks_ground: !self.kind.is_stealthy(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PlacementFootprint {
    anchor: TilePos,
    size: (i32, i32),
    blocks_ground: bool,
}

struct FutureGroundProducerEgress {
    footprint: PlacementFootprint,
    witnesses: Vec<TilePos>,
}

/// The observation-derived half of defense-site selection: the terrain
/// grid, the asset ledger, and the uncleared starts it was scored
/// against. One construction think walks up to five defense rungs, and
/// every rung derives this identically from the same observation — the
/// asset ledger's per-cluster access routing is the most expensive
/// pathfinding in a planning think, so the ladder builds it once and
/// shares it.
pub(super) struct DefenseGrounding<'a> {
    public_starts: Vec<StartingFoundry>,
    ground: GroundKnowledge<'a>,
    assets: Vec<DefendedAsset>,
    future_ground_producers: Vec<FutureGroundProducerEgress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BuilderSafetyKey {
    builder: UnitId,
    anchor: TilePos,
    size: (i32, i32),
    defer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct BuilderTravelKey {
    builder: UnitId,
    placement: PlacementFootprint,
}

#[derive(Clone, Copy)]
struct BuilderSafetyContext<'a> {
    obs: &'a Observation,
    briefing: &'a PublicMapBriefing,
    danger: &'a super::danger::HarvestDangerProjection,
    orientation: Option<Orientation>,
}

#[derive(Clone, Copy)]
struct CombinedLayoutContext<'a> {
    obs: &'a Observation,
    briefing: &'a PublicMapBriefing,
    unit_contacts: &'a [UnitContact],
    building_contacts: &'a [BuildingContact],
    orientation: Option<Orientation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ResourceAccessKey {
    placement: PlacementFootprint,
    max_detour: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ReinforcementRouteKey {
    unit: UnitId,
    defense: BuildingKind,
    anchor: TilePos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BaselineEndpointRoute {
    path: Option<Vec<TilePos>>,
    exhausted: bool,
}

#[derive(Default)]
struct DefenseEvaluationCache {
    builder_safety: BTreeMap<BuilderSafetyKey, bool>,
    builder_travel: BTreeMap<BuilderTravelKey, Option<u32>>,
    future_producer_egress: BTreeMap<PlacementFootprint, bool>,
    resource_access: BTreeMap<ResourceAccessKey, bool>,
    reinforcement_travel: BTreeMap<ReinforcementRouteKey, Option<u32>>,
    supported_assets: BTreeMap<PlacementFootprint, BTreeSet<usize>>,
    rerouted_approaches: BTreeMap<(DefenseDomain, PlacementFootprint), Option<Vec<Approach>>>,
    baseline_endpoint_routes: BTreeMap<(DefenseDomain, TilePos, TilePos), BaselineEndpointRoute>,
    #[cfg(test)]
    stats: DefenseThinkCacheStats,
}

struct StrategicLaneProjection<'a> {
    origins: Vec<ThreatOrigin>,
    approaches: Vec<Approach>,
    evidence: DefenseOpportunityEvidence,
    existing: Vec<&'a BuildingObs>,
    planned: Vec<PlannedDefense>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CandidateThreatSummary {
    evidence: DefenseOpportunityEvidence,
    evidence_count: usize,
    threat_arrival_ticks: Option<u64>,
}

/// One immutable observation boundary plus exact derived-data caches shared by
/// every voluntary defensive role in a single allocation think.
pub(super) struct DefenseThinkContext<'a> {
    obs: &'a Observation,
    briefing: &'a PublicMapBriefing,
    unit_contacts: &'a [UnitContact],
    building_contacts: &'a [BuildingContact],
    grounding: DefenseGrounding<'a>,
    future_egress_orientation: Option<Orientation>,
    danger: Arc<super::danger::HarvestDangerProjection>,
    ground_projection: Option<Option<StrategicLaneProjection<'a>>>,
    air_projection: Option<Option<StrategicLaneProjection<'a>>>,
    evaluation: DefenseEvaluationCache,
    #[cfg(test)]
    projection_builds: [usize; 2],
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct DefenseThinkCacheStats {
    pub(super) ground_projection_builds: usize,
    pub(super) air_projection_builds: usize,
    pub(super) builder_safety_builds: usize,
    pub(super) builder_safety_hits: usize,
    pub(super) builder_travel_builds: usize,
    pub(super) builder_travel_hits: usize,
    pub(super) future_producer_baseline_builds: usize,
    pub(super) future_producer_egress_builds: usize,
    pub(super) future_producer_egress_hits: usize,
    pub(super) resource_access_builds: usize,
    pub(super) resource_access_hits: usize,
    pub(super) reinforcement_route_builds: usize,
    pub(super) reinforcement_route_hits: usize,
    pub(super) supported_assets_builds: usize,
    pub(super) supported_assets_hits: usize,
    pub(super) rerouted_approach_builds: usize,
    pub(super) rerouted_approach_hits: usize,
}

impl<'a> DefenseGrounding<'a> {
    pub(super) fn new(
        policy: &UtilityPolicy,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
    ) -> Self {
        Self::new_inner(policy, obs, briefing, None)
    }

    fn new_inner(
        policy: &UtilityPolicy,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        future_egress_orientation: Option<Orientation>,
    ) -> Self {
        let public_starts = policy.uncleared_hostile_starts(briefing, obs.me);
        let ground = GroundKnowledge::new(obs, briefing, &public_starts);
        let assets = defended_assets(policy, obs, &ground);
        let future_ground_producers =
            future_egress_orientation.map_or_else(Vec::new, |orientation| {
                observed_future_ground_producers(obs)
                    .into_iter()
                    .map(|footprint| {
                        future_ground_producer_egress_certificate(
                            &ground,
                            Some(orientation),
                            footprint,
                        )
                    })
                    .collect()
            });
        Self {
            public_starts,
            ground,
            assets,
            future_ground_producers,
        }
    }

    fn resource_access_survives(
        &self,
        placement: PlacementFootprint,
        max_detour: Option<u32>,
    ) -> bool {
        scrap_access_survives(&self.ground, &self.assets, placement, max_detour)
    }

    fn builder_travel_cost(
        &self,
        builder: &UnitObs,
        placement: PlacementFootprint,
        orientation: Option<Orientation>,
    ) -> Option<u32> {
        let defer = (0..placement.size.1).any(|dy| {
            (0..placement.size.0)
                .any(|dx| !self.ground.obs.visible(placement.anchor.offset(dx, dy)))
        });
        match orientation {
            Some(orientation) => {
                routing::build_command_path_cost_with_public_terrain_and_orientation(
                    self.ground.obs,
                    self.ground.briefing,
                    builder,
                    placement.anchor,
                    placement.size,
                    defer,
                    orientation,
                )
            }
            None => routing::build_command_path_cost_with_public_terrain(
                self.ground.obs,
                self.ground.briefing,
                builder,
                placement.anchor,
                placement.size,
                defer,
            ),
        }
    }
}

impl<'a> DefenseThinkContext<'a> {
    #[cfg(test)]
    pub(super) fn new(
        policy: &UtilityPolicy,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        unit_contacts: &'a [UnitContact],
        building_contacts: &'a [BuildingContact],
    ) -> Self {
        Self::new_inner(
            policy,
            obs,
            briefing,
            unit_contacts,
            building_contacts,
            None,
        )
    }

    pub(super) fn new_oriented(
        policy: &UtilityPolicy,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        unit_contacts: &'a [UnitContact],
        building_contacts: &'a [BuildingContact],
        orientation: Orientation,
    ) -> Self {
        Self::new_inner(
            policy,
            obs,
            briefing,
            unit_contacts,
            building_contacts,
            Some(orientation),
        )
    }

    fn new_inner(
        policy: &UtilityPolicy,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        unit_contacts: &'a [UnitContact],
        building_contacts: &'a [BuildingContact],
        future_egress_orientation: Option<Orientation>,
    ) -> Self {
        Self {
            obs,
            briefing,
            unit_contacts,
            building_contacts,
            grounding: DefenseGrounding::new_inner(
                policy,
                obs,
                briefing,
                future_egress_orientation,
            ),
            future_egress_orientation,
            danger: policy.harvest_danger_projection(
                obs,
                Some(unit_contacts),
                Some(building_contacts),
            ),
            ground_projection: None,
            air_projection: None,
            evaluation: DefenseEvaluationCache::default(),
            #[cfg(test)]
            projection_builds: [0; 2],
        }
    }

    pub(super) fn worker_start_is_safe(&self, tile: TilePos) -> bool {
        !self.danger.contains(tile)
    }

    pub(super) fn resource_access_survives(&mut self, kind: BuildingKind, anchor: TilePos) -> bool {
        let placement = DefenseProfile::for_kind(kind).map_or(
            PlacementFootprint {
                anchor,
                size: kind.base_stats().size,
                blocks_ground: !kind.is_stealthy(),
            },
            |profile| profile.footprint(anchor),
        );
        cached_resource_access_survives(&self.grounding, &mut self.evaluation, placement, None)
    }

    pub(super) fn future_ground_producer_egress_survives(
        &mut self,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> bool {
        let Some(orientation) = self.future_egress_orientation else {
            return true;
        };
        let placement = DefenseProfile::for_kind(kind).map_or(
            PlacementFootprint {
                anchor,
                size: kind.base_stats().size,
                blocks_ground: !kind.is_stealthy(),
            },
            |profile| profile.footprint(anchor),
        );
        cached_future_ground_producer_egress_survives(
            &self.grounding,
            orientation,
            &mut self.evaluation,
            placement,
        )
    }

    pub(super) fn builder_travel_cost(
        &mut self,
        builder: &UnitObs,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> Option<u32> {
        let placement = DefenseProfile::for_kind(kind).map_or(
            PlacementFootprint {
                anchor,
                size: kind.base_stats().size,
                blocks_ground: !kind.is_stealthy(),
            },
            |profile| profile.footprint(anchor),
        );
        cached_builder_travel_cost(
            &self.grounding,
            &mut self.evaluation,
            builder,
            placement,
            self.future_egress_orientation,
        )
    }

    pub(super) fn safe_implicit_builder(
        &mut self,
        policy: &UtilityPolicy,
        kind: BuildingKind,
        anchor: TilePos,
        builders: &[&UnitObs],
    ) -> Option<UnitId> {
        cached_safe_implicit_builder(
            policy,
            BuilderSafetyContext {
                obs: self.obs,
                briefing: self.briefing,
                danger: &self.danger,
                orientation: self.future_egress_orientation,
            },
            &mut self.evaluation,
            kind,
            anchor,
            builders,
        )
    }

    pub(super) const fn observation(&self) -> &'a Observation {
        self.obs
    }

    pub(super) const fn briefing(&self) -> &'a PublicMapBriefing {
        self.briefing
    }

    pub(super) const fn unit_contacts(&self) -> &'a [UnitContact] {
        self.unit_contacts
    }

    pub(super) const fn building_contacts(&self) -> &'a [BuildingContact] {
        self.building_contacts
    }

    pub(super) fn upgrade_evidence(&mut self, kind: BuildingKind) -> DefenseOpportunityEvidence {
        let Some(profile) = DefenseProfile::for_kind(kind) else {
            return DefenseOpportunityEvidence::PublicPrior;
        };
        self.ensure_projection(profile.domain);
        let projection = match profile.domain {
            DefenseDomain::Ground => self.ground_projection.as_ref().and_then(Option::as_ref),
            DefenseDomain::Air => self.air_projection.as_ref().and_then(Option::as_ref),
        };
        projection.map_or(DefenseOpportunityEvidence::PublicPrior, |projection| {
            projection.evidence
        })
    }

    pub(super) fn upgrade_value(&mut self, building: &BuildingObs, horizon: u64) -> u64 {
        let Some(upgrade) = building.kind.upgrade_from(building.tier) else {
            return 0;
        };
        if building.kind == BuildingKind::Array {
            return self.array_upgrade_value(building, horizon, u64::from(upgrade.build_ticks));
        }
        let Some(profile) = DefenseProfile::for_kind(building.kind) else {
            return 0;
        };
        self.ensure_projection(profile.domain);
        let projection = match profile.domain {
            DefenseDomain::Ground => self.ground_projection.as_ref().and_then(Option::as_ref),
            DefenseDomain::Air => self.air_projection.as_ref().and_then(Option::as_ref),
        };
        let Some(projection) = projection else {
            return 0;
        };
        let mut upgraded = building.clone();
        upgraded.tier += 1;
        let dps = |building: &BuildingObs| {
            building
                .kind
                .tier_stats(building.tier)
                .weapons
                .iter()
                .filter(|weapon| weapon_targets_domain(weapon, profile.domain))
                .map(|weapon| {
                    u64::from(weapon.damage)
                        .saturating_mul(u64::from(weapon.salvo))
                        .saturating_mul(100)
                        / u64::from(weapon.cooldown_ticks.max(1))
                })
                .fold(0, u64::saturating_add)
        };
        let old_dps = dps(building);
        let new_dps = dps(&upgraded);
        if new_dps == 0 {
            return 0;
        }
        let active_ticks = horizon.saturating_sub(u64::from(upgrade.build_ticks));
        let mut value = 0u64;
        for (index, asset) in self.grounding.assets.iter().enumerate() {
            let mut marginal = 0;
            for approach in projection
                .approaches
                .iter()
                .filter(|approach| approach.asset == index)
            {
                let currently_covered = approach.path.iter().any(|tile| {
                    building_covers(self.obs, self.briefing, building, *tile, profile.domain)
                });
                // The self-refit cannot retreat or be cancelled. Do not remove
                // a needed firing position while a current attacker is near it.
                if currently_covered
                    && projection.evidence == DefenseOpportunityEvidence::CurrentArmed
                    && approach.path.len() <= DEFENSE_RADIUS as usize
                {
                    return 0;
                }
                for &tile in &approach.path {
                    if !building_covers(self.obs, self.briefing, &upgraded, tile, profile.domain) {
                        continue;
                    }
                    let before =
                        if building_covers(self.obs, self.briefing, building, tile, profile.domain)
                        {
                            old_dps
                        } else {
                            0
                        };
                    let gain = new_dps
                        .saturating_mul(active_ticks)
                        .saturating_sub(before.saturating_mul(horizon));
                    let redundancy = projection
                        .existing
                        .iter()
                        .filter(|other| {
                            other.id != building.id
                                && building_covers(
                                    self.obs,
                                    self.briefing,
                                    other,
                                    tile,
                                    profile.domain,
                                )
                        })
                        .count() as u64;
                    let covered_value = u64::from(asset.value)
                        .saturating_mul(u64::from(UnitKind::Sentinel.stats().cost));
                    marginal = marginal.max(
                        covered_value.saturating_mul(gain)
                            / new_dps
                                .saturating_mul(horizon)
                                .saturating_mul(redundancy.saturating_add(1))
                                .max(1),
                    );
                }
            }
            value = value.saturating_add(marginal);
        }
        value.saturating_mul(u64::from(building.hp))
            / u64::from(building.kind.tier_stats(building.tier).max_hp.max(1))
    }

    fn array_upgrade_value(&self, building: &BuildingObs, horizon: u64, offline: u64) -> u64 {
        let covers = |array: &BuildingObs, target: TilePos, tier: u8| {
            let radius = if tier == 0 {
                crate::stats::CHARGE_BASE_ARRAY_DETECT_RADIUS
            } else {
                crate::stats::CHARGE_ARRAY_DETECT_RADIUS
            };
            let dx = i64::from(target.x) - i64::from(array.anchor.x);
            let dy = i64::from(target.y) - i64::from(array.anchor.y);
            dx * dx + dy * dy <= i64::from(radius) * i64::from(radius)
        };
        let radius = crate::stats::CHARGE_ARRAY_DETECT_RADIUS;
        let mut before = 0u64;
        let mut after = 0u64;
        for y in (building.anchor.y - radius).max(0)
            ..=(building.anchor.y + radius).min(self.obs.map_height - 1)
        {
            for x in (building.anchor.x - radius).max(0)
                ..=(building.anchor.x + radius).min(self.obs.map_width - 1)
            {
                let tile = TilePos::new(x, y);
                if self
                    .briefing
                    .terrain_at(tile)
                    .is_none_or(|terrain| terrain.blocks_ground())
                {
                    continue;
                }
                if self
                    .obs
                    .my_buildings
                    .iter()
                    .chain(&self.obs.ally_buildings)
                    .any(|other| {
                        other.id != building.id
                            && other.built
                            && other.kind == BuildingKind::Array
                            && covers(other, tile, other.tier)
                    })
                {
                    continue;
                }
                before += u64::from(covers(building, tile, building.tier));
                after += u64::from(covers(building, tile, building.tier + 1));
            }
        }
        // Both tiers have the same radar radius. Only novel, usable buried-charge
        // detection earns value; the refit also loses the existing coverage.
        after
            .saturating_mul(horizon.saturating_sub(offline))
            .saturating_sub(before.saturating_mul(horizon))
            / horizon.max(1)
    }

    pub(super) fn reinforcement_travel_cost(
        &mut self,
        unit: &UnitObs,
        defense: BuildingKind,
        anchor: TilePos,
    ) -> Option<u32> {
        let key = ReinforcementRouteKey {
            unit: unit.id,
            defense,
            anchor,
        };
        if let Some(cost) = self.evaluation.reinforcement_travel.get(&key).copied() {
            #[cfg(test)]
            {
                self.evaluation.stats.reinforcement_route_hits += 1;
            }
            return cost;
        }
        let footprint = PlacementFootprint {
            anchor,
            size: defense.base_stats().size,
            blocks_ground: !defense.is_stealthy(),
        };
        let movement_domain = if unit.kind.stats().domain == Domain::Air {
            DefenseDomain::Air
        } else {
            DefenseDomain::Ground
        };
        let goals = if movement_domain == DefenseDomain::Air {
            vec![anchor]
        } else {
            building_doorsteps(&self.grounding.ground, anchor, footprint.size)
        };
        let candidate = (movement_domain == DefenseDomain::Ground).then_some(footprint);
        let cost = shortest_path_between(
            &self.grounding.ground,
            &[unit.tile],
            &goals,
            candidate,
            movement_domain,
        )
        .map(|(_, _, path)| path_cost(&path));
        self.evaluation.reinforcement_travel.insert(key, cost);
        #[cfg(test)]
        {
            self.evaluation.stats.reinforcement_route_builds += 1;
        }
        cost
    }

    fn ensure_projection(&mut self, domain: DefenseDomain) {
        let slot = match domain {
            DefenseDomain::Ground => &mut self.ground_projection,
            DefenseDomain::Air => &mut self.air_projection,
        };
        if slot.is_none() {
            *slot = Some(strategic_lane_projection(
                self.obs,
                self.unit_contacts,
                self.building_contacts,
                &self.grounding,
                domain,
            ));
            #[cfg(test)]
            {
                self.projection_builds[match domain {
                    DefenseDomain::Ground => 0,
                    DefenseDomain::Air => 1,
                }] += 1;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn cache_stats(&self) -> DefenseThinkCacheStats {
        DefenseThinkCacheStats {
            ground_projection_builds: self.projection_builds[0],
            air_projection_builds: self.projection_builds[1],
            future_producer_baseline_builds: self.grounding.future_ground_producers.len(),
            ..self.evaluation.stats
        }
    }
}

#[cfg(test)]
pub(super) struct ResourceAccessGuard<'a> {
    ground: GroundKnowledge<'a>,
    assets: Vec<DefendedAsset>,
}

#[cfg(test)]
impl<'a> ResourceAccessGuard<'a> {
    pub(super) fn new(
        policy: &UtilityPolicy,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
    ) -> Self {
        let public_starts = policy.uncleared_hostile_starts(briefing, obs.me);
        let ground = GroundKnowledge::new(obs, briefing, &public_starts);
        let assets = defended_assets(policy, obs, &ground);
        Self { ground, assets }
    }

    pub(super) fn survives(&self, kind: BuildingKind, anchor: TilePos) -> bool {
        scrap_access_survives(
            &self.ground,
            &self.assets,
            PlacementFootprint {
                anchor,
                size: kind.base_stats().size,
                blocks_ground: !kind.is_stealthy(),
            },
            None,
        )
    }

    pub(super) fn builder_travel_cost(
        &self,
        builder: &UnitObs,
        kind: BuildingKind,
        anchor: TilePos,
    ) -> Option<u32> {
        let placement = PlacementFootprint {
            anchor,
            size: kind.base_stats().size,
            blocks_ground: !kind.is_stealthy(),
        };
        let defer = (0..placement.size.1).any(|dy| {
            (0..placement.size.0)
                .any(|dx| !self.ground.obs.visible(placement.anchor.offset(dx, dy)))
        });
        routing::build_command_path_cost_with_public_terrain(
            self.ground.obs,
            self.ground.briefing,
            builder,
            anchor,
            placement.size,
            defer,
        )
    }
}

fn cached_safe_implicit_builder(
    policy: &UtilityPolicy,
    context: BuilderSafetyContext<'_>,
    cache: &mut DefenseEvaluationCache,
    kind: BuildingKind,
    anchor: TilePos,
    builders: &[&UnitObs],
) -> Option<UnitId> {
    let size = kind.base_stats().size;
    let defer =
        (0..size.1).any(|dy| (0..size.0).any(|dx| !context.obs.visible(anchor.offset(dx, dy))));
    let mut ordered = builders.to_vec();
    ordered.sort_unstable_by_key(|builder| (builder.tile.manhattan(anchor), builder.id));
    ordered.into_iter().find_map(|builder| {
        let key = BuilderSafetyKey {
            builder: builder.id,
            anchor,
            size,
            defer,
        };
        let safe = if let Some(safe) = cache.builder_safety.get(&key).copied() {
            #[cfg(test)]
            {
                cache.stats.builder_safety_hits += 1;
            }
            safe
        } else {
            let blocked =
                |tile| policy.harvest_location_contested(tile) || context.danger.contains(tile);
            let safe = match context.orientation {
                Some(orientation) => {
                    routing::build_command_path_avoids_with_public_terrain_and_orientation(
                        context.obs,
                        context.briefing,
                        builder,
                        routing::BuildCommandTarget {
                            anchor,
                            size,
                            defer,
                        },
                        orientation,
                        blocked,
                    )
                }
                None => routing::build_command_path_avoids_with_public_terrain(
                    context.obs,
                    context.briefing,
                    builder,
                    anchor,
                    size,
                    defer,
                    blocked,
                ),
            };
            cache.builder_safety.insert(key, safe);
            #[cfg(test)]
            {
                cache.stats.builder_safety_builds += 1;
            }
            safe
        };
        safe.then_some(builder.id)
    })
}

fn cached_builder_travel_cost(
    grounding: &DefenseGrounding<'_>,
    cache: &mut DefenseEvaluationCache,
    builder: &UnitObs,
    placement: PlacementFootprint,
    orientation: Option<Orientation>,
) -> Option<u32> {
    let key = BuilderTravelKey {
        builder: builder.id,
        placement,
    };
    if let Some(cost) = cache.builder_travel.get(&key).copied() {
        #[cfg(test)]
        {
            cache.stats.builder_travel_hits += 1;
        }
        return cost;
    }
    let cost = grounding.builder_travel_cost(builder, placement, orientation);
    cache.builder_travel.insert(key, cost);
    #[cfg(test)]
    {
        cache.stats.builder_travel_builds += 1;
    }
    cost
}

fn cached_future_ground_producer_egress_survives(
    grounding: &DefenseGrounding<'_>,
    orientation: Orientation,
    cache: &mut DefenseEvaluationCache,
    placement: PlacementFootprint,
) -> bool {
    if !placement.blocks_ground || grounding.future_ground_producers.is_empty() {
        return true;
    }
    if let Some(survives) = cache.future_producer_egress.get(&placement).copied() {
        #[cfg(test)]
        {
            cache.stats.future_producer_egress_hits += 1;
        }
        return survives;
    }
    let survives = grounding.future_ground_producers.iter().all(|producer| {
        future_ground_producer_keeps_egress(
            &grounding.ground,
            Some(placement),
            Some(orientation),
            producer,
        )
    });
    cache.future_producer_egress.insert(placement, survives);
    #[cfg(test)]
    {
        cache.stats.future_producer_egress_builds += 1;
    }
    survives
}

fn cached_resource_access_survives(
    grounding: &DefenseGrounding<'_>,
    cache: &mut DefenseEvaluationCache,
    placement: PlacementFootprint,
    max_detour: Option<u32>,
) -> bool {
    let key = ResourceAccessKey {
        placement,
        max_detour,
    };
    if let Some(survives) = cache.resource_access.get(&key).copied() {
        #[cfg(test)]
        {
            cache.stats.resource_access_hits += 1;
        }
        return survives;
    }
    let survives = grounding.resource_access_survives(placement, max_detour);
    cache.resource_access.insert(key, survives);
    #[cfg(test)]
    {
        cache.stats.resource_access_builds += 1;
    }
    survives
}

impl PlacementFootprint {
    fn blocks(self, tile: TilePos) -> bool {
        self.blocks_ground && footprint_contains(self.anchor, self.size, tile)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DefendedAsset {
    value: u32,
    shape: AssetShape,
    access: Option<AccessRoute>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AssetShape {
    Building {
        anchor: TilePos,
        size: (i32, i32),
    },
    Scrap {
        tiles: Vec<TilePos>,
        work_tiles: Vec<TilePos>,
    },
}

impl AssetShape {
    fn approach_tiles(&self, ground: &GroundKnowledge<'_>, domain: DefenseDomain) -> Vec<TilePos> {
        match (self, domain) {
            (Self::Building { anchor, size }, DefenseDomain::Ground) => sorted_tiles(
                crate::tick::rect_adjacent_tiles(*anchor, *size)
                    .filter(|tile| ground.open(*tile, None, domain)),
            ),
            (Self::Building { anchor, size }, DefenseDomain::Air) => {
                footprint_tiles(*anchor, *size)
                    .into_iter()
                    .filter(|tile| ground.open(*tile, None, domain))
                    .collect()
            }
            (Self::Scrap { work_tiles, .. }, DefenseDomain::Ground) => work_tiles.clone(),
            (Self::Scrap { tiles, .. }, DefenseDomain::Air) => tiles.clone(),
        }
    }

    fn candidate_seeds(&self) -> Vec<TilePos> {
        match self {
            Self::Building { anchor, size } => {
                sorted_tiles(crate::tick::rect_adjacent_tiles(*anchor, *size))
            }
            Self::Scrap { work_tiles, .. } => work_tiles.clone(),
        }
    }

    fn aim_point(&self, shooter: Vec2Fx, route_goal: TilePos) -> Vec2Fx {
        match self {
            Self::Building { anchor, size } => closest_point_to_footprint(*anchor, *size, shooter),
            Self::Scrap { .. } => route_goal.center(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccessRoute {
    foundry: TilePos,
    work_tiles: Vec<TilePos>,
    path: Vec<TilePos>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ThreatCapability {
    Foothold,
    Mobile(UnitKind),
    StaticDefense { kind: BuildingKind, tier: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ThreatOrigin {
    anchor: TilePos,
    size: Option<(i32, i32)>,
    capability: ThreatCapability,
    tie: u32,
}

impl ThreatOrigin {
    #[cfg(test)]
    fn mobile_kind(self) -> Option<UnitKind> {
        match self.capability {
            ThreatCapability::Mobile(kind) => Some(kind),
            ThreatCapability::Foothold | ThreatCapability::StaticDefense { .. } => None,
        }
    }

    fn approach_tiles(self, ground: &GroundKnowledge<'_>, domain: DefenseDomain) -> Vec<TilePos> {
        match (self.size, domain) {
            (Some(size), DefenseDomain::Ground) => sorted_tiles(
                crate::tick::rect_adjacent_tiles(self.anchor, size)
                    .filter(|tile| ground.open(*tile, None, domain)),
            ),
            (Some(size), DefenseDomain::Air) => footprint_tiles(self.anchor, size)
                .into_iter()
                .filter(|tile| ground.open(*tile, None, domain))
                .collect(),
            (None, _) => vec![self.anchor],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Approach {
    asset: usize,
    source: ThreatOrigin,
    goal: TilePos,
    path: Vec<TilePos>,
    baseline_cost: u32,
    disrupted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Coverage {
    new: u32,
    reinforced: u32,
    unplanned_new: u32,
    unplanned_reinforced: u32,
    interception: u32,
    protected_value: u32,
    planned_overlap: u32,
    blind_exposure: u32,
    spotted_reach: u32,
    redundant: u32,
    lateral: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Candidate {
    anchor: TilePos,
    builder_travel: u32,
    coverage: Coverage,
    threat_distance: i32,
}

/// Fog-honest evidence tier which selected a voluntary defense opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) enum DefenseOpportunityEvidence {
    /// A currently observed armed attacker or hostile weapon emplacement.
    CurrentArmed,
    /// A currently observed production or economic foothold.
    CurrentFoothold,
    /// A still-credible remembered attacker or foothold.
    Remembered,
    /// Only an uncleared authored hostile start supports the opportunity.
    PublicPrior,
}

/// Exact scorer output retained by the voluntary-investment layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StrategicDefenseQuote {
    pub(super) placement: DefensePlacement,
    pub(super) uncovered_value: u32,
    pub(super) reinforced_value: u32,
    pub(super) builder_travel_cost: u32,
    pub(super) evidence: DefenseOpportunityEvidence,
    pub(super) evidence_count: usize,
    pub(super) threat_arrival_ticks: Option<u64>,
    pub(super) blind_exposure: u32,
}

impl Candidate {
    fn key(self, profile: DefenseProfile) -> impl Ord {
        let posture = if profile.kind == BuildingKind::Bastion {
            self.threat_distance
        } else {
            -self.threat_distance
        };
        (
            (
                self.coverage.unplanned_new,
                self.coverage.unplanned_reinforced,
            ),
            self.coverage.new,
            self.coverage.reinforced,
            self.coverage.protected_value,
            Reverse(self.coverage.planned_overlap),
            Reverse(self.coverage.redundant),
            Reverse(self.coverage.blind_exposure),
            self.coverage.spotted_reach,
            self.coverage.interception,
            Reverse(self.builder_travel),
            posture,
            (
                Reverse(self.coverage.lateral),
                Reverse(self.anchor.y),
                Reverse(self.anchor.x),
            ),
        )
    }
}

/// Exact defense placement whose worker has already passed the same public-
/// terrain and current-danger route checks used to rank the site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DefensePlacement {
    pub(super) anchor: TilePos,
    pub(super) builder: UnitId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlannedDefense {
    profile: DefenseProfile,
    anchor: TilePos,
    tier: u8,
}

struct GroundKnowledge<'a> {
    obs: &'a Observation,
    briefing: &'a PublicMapBriefing,
    terrain: Vec<Terrain>,
    ground_blocked: Vec<bool>,
    scrap: BTreeMap<TilePos, u32>,
}

impl<'a> GroundKnowledge<'a> {
    fn new(
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        public_starts: &[StartingFoundry],
    ) -> Self {
        let mut scrap: BTreeMap<_, _> = briefing.initial_scrap().iter().copied().collect();
        for tile in sorted_tiles(scrap.keys().copied()) {
            if obs.explored(tile) {
                let amount = obs
                    .known_scrap
                    .binary_search_by_key(&(tile.y, tile.x), |(known, _)| (known.y, known.x))
                    .ok()
                    .map_or(0, |index| obs.known_scrap[index].1);
                if amount == 0 {
                    scrap.remove(&tile);
                } else {
                    scrap.insert(tile, amount);
                }
            }
        }
        for (tile, amount) in &obs.known_scrap {
            if *amount > 0 {
                scrap.insert(*tile, *amount);
            }
        }
        let area = usize::try_from(obs.map_width)
            .ok()
            .and_then(|width| {
                usize::try_from(obs.map_height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .unwrap_or(0);
        let mut terrain = vec![Terrain::Ground; area];
        for (tile, authored) in briefing.non_ground_terrain() {
            if let Some(index) = tile_index(obs.map_width, obs.map_height, *tile) {
                terrain[index] = *authored;
            }
        }
        let mut ground_blocked: Vec<_> = terrain
            .iter()
            .map(|terrain| terrain.blocks_ground())
            .collect();
        for tile in scrap.keys().copied() {
            if let Some(index) = tile_index(obs.map_width, obs.map_height, tile) {
                ground_blocked[index] = true;
            }
        }
        for (anchor, size) in obs
            .my_buildings
            .iter()
            .chain(obs.ally_buildings.iter())
            .chain(obs.enemy_buildings.iter())
            .filter(|building| building.hp > 0 && !building.kind.is_stealthy())
            .map(|building| {
                (
                    building.anchor,
                    building.kind.tier_stats(building.tier).size,
                )
            })
            .chain(obs.my_units.iter().filter_map(|unit| {
                let (kind, anchor) = unit.founding?;
                (!kind.is_stealthy()).then_some((anchor, kind.base_stats().size))
            }))
            .chain(
                public_starts
                    .iter()
                    .map(|start| (start.anchor, BuildingKind::Foundry.base_stats().size)),
            )
        {
            for tile in footprint_tiles(anchor, size) {
                if let Some(index) = tile_index(obs.map_width, obs.map_height, tile) {
                    ground_blocked[index] = true;
                }
            }
        }
        Self {
            obs,
            briefing,
            terrain,
            ground_blocked,
            scrap,
        }
    }

    fn open(
        &self,
        tile: TilePos,
        candidate: Option<PlacementFootprint>,
        domain: DefenseDomain,
    ) -> bool {
        let Some(index) = tile_index(self.obs.map_width, self.obs.map_height, tile) else {
            return false;
        };
        match domain {
            DefenseDomain::Ground => {
                !self.ground_blocked[index]
                    && candidate.is_none_or(|placement| !placement.blocks(tile))
            }
            DefenseDomain::Air => !self.terrain[index].blocks_air(),
        }
    }
}

fn tile_index(width: i32, height: i32, tile: TilePos) -> Option<usize> {
    (tile.x >= 0 && tile.y >= 0 && tile.x < width && tile.y < height)
        .then(|| tile.y as usize * width as usize + tile.x as usize)
}

fn observed_future_ground_producers(obs: &Observation) -> Vec<PlacementFootprint> {
    let mut producers: Vec<_> = obs
        .my_buildings
        .iter()
        .filter(|building| {
            building.hp > 0
                && !building.built
                && building
                    .kind
                    .tier_stats(building.tier)
                    .produces
                    .iter()
                    .any(|unit| unit.stats().domain == Domain::Ground)
        })
        .map(|building| PlacementFootprint {
            anchor: building.anchor,
            size: building.kind.tier_stats(building.tier).size,
            blocks_ground: !building.kind.is_stealthy(),
        })
        .chain(obs.my_units.iter().filter_map(|unit| {
            let (kind, anchor) = unit.founding?;
            kind.base_stats()
                .produces
                .iter()
                .any(|unit| unit.stats().domain == Domain::Ground)
                .then_some(PlacementFootprint {
                    anchor,
                    size: kind.base_stats().size,
                    blocks_ground: !kind.is_stealthy(),
                })
        }))
        .collect();
    producers.sort_unstable();
    producers.dedup();
    producers
}

fn future_ground_producers_keep_egress(
    baseline: &GroundKnowledge<'_>,
    combined: &GroundKnowledge<'_>,
    orientation: Option<Orientation>,
    builds: &[(BuildingKind, TilePos)],
) -> bool {
    let mut producers = observed_future_ground_producers(baseline.obs);
    producers.extend(builds.iter().copied().filter_map(|(kind, anchor)| {
        kind.base_stats()
            .produces
            .iter()
            .any(|unit| unit.stats().domain == Domain::Ground)
            .then_some(PlacementFootprint {
                anchor,
                size: kind.base_stats().size,
                blocks_ground: !kind.is_stealthy(),
            })
    }));
    producers.sort_unstable();
    producers.dedup();
    producers.into_iter().all(|footprint| {
        let producer = future_ground_producer_egress_certificate(baseline, orientation, footprint);
        future_ground_producer_keeps_egress(combined, None, orientation, &producer)
    })
}

fn planned_ground_producer_spawn_doorstep(
    ground: &GroundKnowledge<'_>,
    footprint: PlacementFootprint,
    candidate: Option<PlacementFootprint>,
    orientation: Option<Orientation>,
) -> Option<TilePos> {
    routing::production_spawn_doorstep_for_open_tiles(
        (ground.obs.map_width, ground.obs.map_height),
        footprint.anchor,
        footprint.size,
        orientation,
        |policy_tile| ground.open(policy_tile, candidate, DefenseDomain::Ground),
    )
}

fn future_ground_producer_egress_certificate(
    baseline: &GroundKnowledge<'_>,
    orientation: Option<Orientation>,
    footprint: PlacementFootprint,
) -> FutureGroundProducerEgress {
    let Some(start) =
        planned_ground_producer_spawn_doorstep(baseline, footprint, Some(footprint), orientation)
    else {
        return FutureGroundProducerEgress {
            footprint,
            witnesses: Vec::new(),
        };
    };

    let width = baseline.obs.map_width;
    let height = baseline.obs.map_height;
    let cells = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .unwrap_or(0);
    let mut reachable = vec![false; cells];
    let start_index = tile_index(width, height, start).expect("an open doorstep is in bounds");
    reachable[start_index] = true;
    let mut frontier = VecDeque::from([start]);
    while let Some(tile) = frontier.pop_front() {
        for (dx, dy) in CARDINALS {
            let next = tile.offset(dx, dy);
            let Some(index) = tile_index(width, height, next) else {
                continue;
            };
            if !reachable[index] && baseline.open(next, Some(footprint), DefenseDomain::Ground) {
                reachable[index] = true;
                frontier.push_back(next);
            }
        }
    }
    let mut witnesses: Vec<_> = reachable
        .iter()
        .enumerate()
        .filter(|(_, reachable)| **reachable)
        .map(|(index, _)| TilePos::new(index as i32 % width, index as i32 / width))
        .collect();
    witnesses
        .sort_unstable_by_key(|tile| (Reverse(tile.chebyshev(footprint.anchor)), tile.y, tile.x));
    FutureGroundProducerEgress {
        footprint,
        witnesses,
    }
}

fn future_ground_producer_keeps_egress(
    combined: &GroundKnowledge<'_>,
    combined_candidate: Option<PlacementFootprint>,
    orientation: Option<Orientation>,
    producer: &FutureGroundProducerEgress,
) -> bool {
    let Some(witness) = producer
        .witnesses
        .iter()
        .copied()
        .find(|tile| combined.open(*tile, combined_candidate, DefenseDomain::Ground))
    else {
        return false;
    };
    let Some(spawn) = planned_ground_producer_spawn_doorstep(
        combined,
        producer.footprint,
        combined_candidate,
        orientation,
    ) else {
        return false;
    };
    shortest_path_between(
        combined,
        &[spawn],
        &[witness],
        combined_candidate,
        DefenseDomain::Ground,
    )
    .is_some()
}

impl UtilityPolicy {
    /// Verifies a frozen set of construction footprints as one layout, without
    /// reranking any proposal. This is the pairwise allocator preflight for
    /// independently selected Foundry and defense opportunities.
    #[cfg(test)]
    pub(in crate::bot) fn combined_build_layout_is_safe(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        builds: &[(BuildingKind, TilePos)],
    ) -> bool {
        self.combined_build_layout_is_safe_inner(
            CombinedLayoutContext {
                obs,
                briefing,
                unit_contacts: &[],
                building_contacts: &[],
                orientation: None,
            },
            builds,
            &[],
        )
    }

    /// The same combined-layout preflight, additionally preserving each exact
    /// selected builder's route to its own footprint.
    pub(in crate::bot) fn combined_build_layout_with_builders_is_safe(
        &self,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        orientation: Orientation,
        builds: &[(BuildingKind, TilePos, UnitId)],
    ) -> bool {
        let footprints: Vec<_> = builds
            .iter()
            .map(|(kind, anchor, _)| (*kind, *anchor))
            .collect();
        self.combined_build_layout_is_safe_inner(
            CombinedLayoutContext {
                obs,
                briefing,
                unit_contacts,
                building_contacts,
                orientation: Some(orientation),
            },
            &footprints,
            builds,
        )
    }

    fn combined_build_layout_is_safe_inner(
        &self,
        context: CombinedLayoutContext<'_>,
        builds: &[(BuildingKind, TilePos)],
        exact_builders: &[(BuildingKind, TilePos, UnitId)],
    ) -> bool {
        let CombinedLayoutContext {
            obs,
            briefing,
            unit_contacts,
            building_contacts,
            orientation,
        } = context;
        if builds.is_empty() {
            return true;
        }
        self.prepare_ground_producer_egress(obs);
        if builds.iter().any(|(kind, anchor)| {
            kind.base_stats().construction.is_none()
                || !self.placement_valid_prepared(obs, *kind, *anchor)
        }) {
            return false;
        }
        let sites: Vec<_> = builds
            .iter()
            .map(|(kind, anchor)| PlacementFootprint {
                anchor: *anchor,
                size: kind.base_stats().size,
                blocks_ground: !kind.is_stealthy(),
            })
            .collect();
        if sites.iter().enumerate().any(|(index, site)| {
            sites
                .iter()
                .skip(index + 1)
                .any(|other| footprints_overlap(*site, *other))
        }) {
            return false;
        }
        let blocking_builds: Vec<_> = builds
            .iter()
            .copied()
            .filter(|(kind, _)| !kind.is_stealthy())
            .collect();
        if let Some((candidate, accepted)) = blocking_builds.split_last()
            && !self.preserves_ground_producer_egress_prepared(accepted, *candidate)
        {
            return false;
        }

        let public_starts = self.uncleared_hostile_starts(briefing, obs.me);
        let baseline_ground = GroundKnowledge::new(obs, briefing, &public_starts);
        let assets = defended_assets(self, obs, &baseline_ground);
        let mut combined_ground = GroundKnowledge::new(obs, briefing, &public_starts);
        for site in &sites {
            if !site.blocks_ground {
                continue;
            }
            for tile in footprint_tiles(site.anchor, site.size) {
                let Some(index) = tile_index(obs.map_width, obs.map_height, tile) else {
                    return false;
                };
                combined_ground.ground_blocked[index] = true;
            }
        }
        if !future_ground_producers_keep_egress(
            &baseline_ground,
            &combined_ground,
            orientation,
            builds,
        ) {
            return false;
        }
        if assets.iter().any(|asset| {
            let Some(access) = &asset.access else {
                return false;
            };
            shortest_path_between(
                &combined_ground,
                &building_doorsteps(
                    &combined_ground,
                    access.foundry,
                    BuildingKind::Foundry.base_stats().size,
                ),
                &access.work_tiles,
                None,
                DefenseDomain::Ground,
            )
            .is_none()
        }) {
            return false;
        }
        if !exact_builders.is_empty() && orientation.is_none() {
            return false;
        }
        let danger =
            self.harvest_danger_projection(obs, Some(unit_contacts), Some(building_contacts));
        exact_builders.iter().all(|(kind, anchor, builder)| {
            let Some(builder) = obs
                .my_units
                .iter()
                .find(|candidate| candidate.id == *builder)
            else {
                return false;
            };
            if !combined_ground.open(builder.tile, None, DefenseDomain::Ground) {
                return false;
            }
            let size = kind.base_stats().size;
            let defer =
                (0..size.1).any(|dy| (0..size.0).any(|dx| !obs.visible(anchor.offset(dx, dy))));
            routing::build_command_path_avoids_with_public_terrain_and_blockers_and_orientation(
                obs,
                briefing,
                builder,
                routing::BuildCommandTarget {
                    anchor: *anchor,
                    size,
                    defer,
                },
                orientation.expect("exact combined builder checks require a command orientation"),
                |tile| {
                    sites.iter().any(|site| {
                        site.blocks_ground
                            && (site.anchor != *anchor || site.size != size)
                            && site.blocks(tile)
                    })
                },
                |tile| {
                    (*kind == BuildingKind::Foundry && !obs.explored(tile))
                        || self.harvest_location_contested(tile)
                        || danger.contains(tile)
                },
            )
        })
    }

    pub(super) fn current_emergency_defense_required(
        &self,
        kind: BuildingKind,
        obs: &Observation,
        briefing: &PublicMapBriefing,
    ) -> bool {
        let Some(profile) = DefenseProfile::for_kind(kind) else {
            return false;
        };
        if obs
            .my_buildings
            .iter()
            .any(|building| building.kind == kind && building.hp > 0)
        {
            return false;
        }
        // Threat origins read only enemy state and visibility, which the
        // founding-cleared clone below leaves untouched, so the common
        // no-threat think skips the clone and the defended-asset routing.
        let origins = emergency_threat_origins(obs, profile.domain);
        if origins.is_empty() {
            return false;
        }
        let mut unclaimed = obs.clone();
        for unit in &mut unclaimed.my_units {
            unit.founding = None;
        }
        let public_starts = self.uncleared_hostile_starts(briefing, obs.me);
        let ground = GroundKnowledge::new(&unclaimed, briefing, &public_starts);
        let assets = defended_assets(self, &unclaimed, &ground);
        if assets.is_empty() {
            return false;
        }
        approaches(&ground, &origins, &assets, None, profile.domain)
            .into_iter()
            .any(|approach| approach.path.len().saturating_sub(1) <= DEFENSE_RADIUS as usize)
    }

    pub(super) fn current_emergency_defense_claim_is_useful(
        &self,
        kind: BuildingKind,
        anchor: TilePos,
        obs: &Observation,
        briefing: &PublicMapBriefing,
    ) -> bool {
        let Some(profile) = DefenseProfile::for_kind(kind) else {
            return false;
        };
        if obs
            .my_buildings
            .iter()
            .any(|building| building.kind == kind && building.hp > 0)
        {
            return false;
        }
        let mut unclaimed = obs.clone();
        for unit in &mut unclaimed.my_units {
            unit.founding = None;
        }
        if self.first_valid_placement(&unclaimed, kind, [anchor]) != Some(anchor) {
            return false;
        }
        let public_starts = self.uncleared_hostile_starts(briefing, obs.me);
        let ground = GroundKnowledge::new(&unclaimed, briefing, &public_starts);
        let assets = defended_assets(self, &unclaimed, &ground);
        if assets.is_empty() {
            return false;
        }
        let origins = emergency_threat_origins(&unclaimed, profile.domain);
        let approaches = approaches(&ground, &origins, &assets, None, profile.domain)
            .into_iter()
            .filter(|approach| approach.path.len().saturating_sub(1) <= DEFENSE_RADIUS as usize)
            .collect::<Vec<_>>();
        if origins.is_empty() || approaches.is_empty() {
            return false;
        }
        let placement = profile.footprint(anchor);
        if !scrap_access_survives(&ground, &assets, placement, None) {
            return false;
        }
        let Some(candidate_approaches) = operationally_supported_approaches(
            &ground,
            &assets,
            &approaches,
            placement,
            profile.domain,
            None,
        ) else {
            return false;
        };
        let existing = existing_defenses(&unclaimed, profile.domain);
        let planned = planned_defenses(&unclaimed, profile.domain);
        let coverage = score_coverage(
            &CoverageContext {
                obs: &unclaimed,
                briefing,
                assets: &assets,
                approaches: &candidate_approaches,
                existing: &existing,
                planned: &planned,
            },
            profile,
            anchor,
        );
        coverage.new > 0 || coverage.reinforced > 0
    }

    #[cfg(test)]
    pub(super) fn strategic_defense_site(
        &self,
        kind: BuildingKind,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
    ) -> Option<TilePos> {
        if builders.is_empty() {
            return None;
        }
        self.strategic_defense_site_grounded(
            kind,
            obs,
            briefing,
            unit_contacts,
            building_contacts,
            builders,
            &mut None,
        )
    }

    /// [`Self::strategic_defense_site`] against a caller-held grounding
    /// slot, for the rung ladder that asks about several building kinds in
    /// one think. The grounding is built on the first rung that can use
    /// it: a rung with no builder cannot select a site, so it must not pay
    /// for the asset ledger's routing either.
    #[expect(clippy::too_many_arguments, reason = "mirrors strategic_defense_site")]
    #[cfg(test)]
    pub(super) fn strategic_defense_site_grounded<'a>(
        &self,
        kind: BuildingKind,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
        grounding: &mut Option<DefenseGrounding<'a>>,
    ) -> Option<TilePos> {
        DefenseProfile::for_kind(kind)?;
        if builders.is_empty() {
            return None;
        }
        self.strategic_defense_quote_with_evidence(
            kind,
            obs,
            briefing,
            builders,
            DefenseEvidence::strategic(unit_contacts, building_contacts),
            grounding.get_or_insert_with(|| DefenseGrounding::new(self, obs, briefing)),
        )
        .map(|quote| quote.placement.anchor)
    }

    /// Answers the exact strategic placement predicate without ranking sites
    /// whose location the caller will discard.
    #[expect(clippy::too_many_arguments, reason = "mirrors strategic_defense_site")]
    #[cfg(test)]
    pub(super) fn strategic_defense_site_exists_grounded<'a>(
        &self,
        kind: BuildingKind,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
        grounding: &mut Option<DefenseGrounding<'a>>,
    ) -> bool {
        if DefenseProfile::for_kind(kind).is_none() || builders.is_empty() {
            return false;
        }
        self.strategic_defense_quote_with_evidence(
            kind,
            obs,
            briefing,
            builders,
            DefenseEvidence::strategic_existence(unit_contacts, building_contacts),
            grounding.get_or_insert_with(|| DefenseGrounding::new(self, obs, briefing)),
        )
        .is_some()
    }

    pub(super) fn emergency_defense_placement(
        &self,
        kind: BuildingKind,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
    ) -> Option<DefensePlacement> {
        if builders.is_empty() {
            return None;
        }
        self.strategic_defense_quote_with_evidence(
            kind,
            obs,
            briefing,
            builders,
            DefenseEvidence::current_emergency(unit_contacts, building_contacts),
            &DefenseGrounding::new(self, obs, briefing),
        )
        .map(|quote| quote.placement)
    }

    /// Ranks one voluntary role and retains the exact placement evidence used
    /// to compare it with unlike strategic investments.
    #[expect(clippy::too_many_arguments, reason = "mirrors strategic_defense_site")]
    #[cfg(test)]
    pub(super) fn strategic_defense_quote_grounded<'a>(
        &self,
        kind: BuildingKind,
        obs: &'a Observation,
        briefing: &'a PublicMapBriefing,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
        grounding: &mut Option<DefenseGrounding<'a>>,
    ) -> Option<StrategicDefenseQuote> {
        DefenseProfile::for_kind(kind)?;
        if builders.is_empty() {
            return None;
        }
        self.strategic_defense_quote_with_evidence(
            kind,
            obs,
            briefing,
            builders,
            DefenseEvidence::strategic(unit_contacts, building_contacts),
            grounding.get_or_insert_with(|| DefenseGrounding::new(self, obs, briefing)),
        )
    }

    pub(super) fn strategic_defense_quote_in_context(
        &self,
        kind: BuildingKind,
        builders: &[&UnitObs],
        context: &mut DefenseThinkContext<'_>,
    ) -> Option<StrategicDefenseQuote> {
        let profile = DefenseProfile::for_kind(kind)?;
        if builders.is_empty() {
            return None;
        }
        context.ensure_projection(profile.domain);
        let projection = match profile.domain {
            DefenseDomain::Ground => context.ground_projection.as_ref()?.as_ref()?,
            DefenseDomain::Air => context.air_projection.as_ref()?.as_ref()?,
        };
        strategic_defense_quote_from_projection(
            self,
            kind,
            context.obs,
            context.briefing,
            builders,
            &context.grounding,
            projection,
            &context.danger,
            DefenseSiteSearch::Best,
            context.future_egress_orientation,
            &mut context.evaluation,
        )
    }

    fn strategic_defense_quote_with_evidence(
        &self,
        kind: BuildingKind,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        builders: &[&UnitObs],
        evidence: DefenseEvidence<'_>,
        grounding: &DefenseGrounding<'_>,
    ) -> Option<StrategicDefenseQuote> {
        let DefenseEvidence {
            unit_contacts,
            building_contacts,
            scope,
            search,
        } = evidence;
        let profile = DefenseProfile::for_kind(kind)?;
        if builders.is_empty() {
            return None;
        }
        let projection = if scope == DefenseEvidenceScope::CurrentEmergency {
            emergency_lane_projection(obs, grounding, profile.domain)?
        } else {
            strategic_lane_projection(
                obs,
                unit_contacts,
                building_contacts,
                grounding,
                profile.domain,
            )?
        };
        let danger =
            self.harvest_danger_projection(obs, Some(unit_contacts), Some(building_contacts));
        strategic_defense_quote_from_projection(
            self,
            kind,
            obs,
            briefing,
            builders,
            grounding,
            &projection,
            &danger,
            search,
            None,
            &mut DefenseEvaluationCache::default(),
        )
    }
}

fn strategic_lane_projection<'a>(
    obs: &'a Observation,
    unit_contacts: &[UnitContact],
    building_contacts: &[BuildingContact],
    grounding: &DefenseGrounding<'a>,
    domain: DefenseDomain,
) -> Option<StrategicLaneProjection<'a>> {
    if grounding.assets.is_empty() {
        return None;
    }
    let (origins, approaches, evidence) = threat_origin_tiers(
        obs,
        unit_contacts,
        building_contacts,
        &grounding.public_starts,
        domain,
    )
    .into_iter()
    .enumerate()
    .filter(|(_, origins)| !origins.is_empty())
    .find_map(|(tier, origins)| {
        let approaches = approaches(&grounding.ground, &origins, &grounding.assets, None, domain);
        let evidence = match tier {
            0 => DefenseOpportunityEvidence::CurrentArmed,
            1 if origins.iter().any(|origin| {
                matches!(origin.capability, ThreatCapability::StaticDefense { .. })
            }) =>
            {
                DefenseOpportunityEvidence::CurrentArmed
            }
            1 => DefenseOpportunityEvidence::CurrentFoothold,
            2 => DefenseOpportunityEvidence::Remembered,
            3 => DefenseOpportunityEvidence::PublicPrior,
            _ => unreachable!("the strategic evidence ladder has four tiers"),
        };
        (!approaches.is_empty()).then_some((origins, approaches, evidence))
    })?;
    Some(StrategicLaneProjection {
        origins,
        approaches,
        evidence,
        existing: existing_defenses(obs, domain),
        planned: planned_defenses(obs, domain),
    })
}

fn emergency_lane_projection<'a>(
    obs: &'a Observation,
    grounding: &DefenseGrounding<'a>,
    domain: DefenseDomain,
) -> Option<StrategicLaneProjection<'a>> {
    if grounding.assets.is_empty() {
        return None;
    }
    let origins = emergency_threat_origins(obs, domain);
    let approaches = approaches(&grounding.ground, &origins, &grounding.assets, None, domain)
        .into_iter()
        .filter(|approach| approach.path.len().saturating_sub(1) <= DEFENSE_RADIUS as usize)
        .collect::<Vec<_>>();
    if origins.is_empty() || approaches.is_empty() {
        return None;
    }
    Some(StrategicLaneProjection {
        origins,
        approaches,
        evidence: DefenseOpportunityEvidence::CurrentArmed,
        existing: existing_defenses(obs, domain),
        planned: planned_defenses(obs, domain),
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "one frozen defense-scoring boundary"
)]
fn strategic_defense_quote_from_projection(
    policy: &UtilityPolicy,
    kind: BuildingKind,
    obs: &Observation,
    briefing: &PublicMapBriefing,
    builders: &[&UnitObs],
    grounding: &DefenseGrounding<'_>,
    projection: &StrategicLaneProjection<'_>,
    danger: &super::danger::HarvestDangerProjection,
    search: DefenseSiteSearch,
    future_egress_orientation: Option<Orientation>,
    cache: &mut DefenseEvaluationCache,
) -> Option<StrategicDefenseQuote> {
    let profile = DefenseProfile::for_kind(kind)?;
    let ground = &grounding.ground;
    let assets = &grounding.assets;
    let origins = &projection.origins;
    let approaches = &projection.approaches;

    let mut candidate_tiles = BTreeSet::new();
    for approach in approaches {
        for seed in approach
            .path
            .iter()
            .rev()
            .take(INTERCEPTION_DEPTH + 1)
            .copied()
            .chain(assets[approach.asset].shape.candidate_seeds())
        {
            for dy in -profile.candidate_reach..=profile.candidate_reach {
                for dx in -profile.candidate_reach..=profile.candidate_reach {
                    if dx.abs().max(dy.abs()) <= profile.candidate_reach {
                        candidate_tiles.insert(seed.offset(dx, dy));
                    }
                }
            }
        }
    }
    let mut candidate_tiles: Vec<_> = candidate_tiles.into_iter().collect();
    candidate_tiles.sort_by_key(|tile| (tile.y, tile.x));

    policy.prepare_ground_producer_egress(obs);
    let candidates = candidate_tiles.into_iter().filter_map(|anchor| {
        if !policy.placement_valid_prepared(obs, kind, anchor) {
            return None;
        }
        let placement = profile.footprint(anchor);
        if future_egress_orientation.is_some_and(|orientation| {
            !cached_future_ground_producer_egress_survives(grounding, orientation, cache, placement)
        }) {
            return None;
        }
        let mut ordered_builders = builders.to_vec();
        ordered_builders
            .sort_unstable_by_key(|builder| (builder.tile.manhattan(anchor), builder.id));
        let (builder, builder_travel) = ordered_builders.into_iter().find_map(|builder| {
            let builder_travel = cached_builder_travel_cost(
                grounding,
                cache,
                builder,
                placement,
                future_egress_orientation,
            )?;
            let safe = cached_safe_implicit_builder(
                policy,
                BuilderSafetyContext {
                    obs,
                    briefing,
                    danger,
                    orientation: future_egress_orientation,
                },
                cache,
                kind,
                anchor,
                &[builder],
            ) == Some(builder.id);
            safe.then_some((builder.id, builder_travel))
        })?;
        let resource_detour_limit =
            (profile.kind == BuildingKind::Barricade).then_some(MAX_BARRICADE_RESOURCE_DETOUR_COST);
        if !cached_resource_access_survives(grounding, cache, placement, resource_detour_limit) {
            return None;
        }
        let candidate_approaches = cached_operationally_supported_approaches(
            ground,
            assets,
            approaches,
            placement,
            profile.domain,
            (profile.kind == BuildingKind::Barricade).then_some(MAX_BARRICADE_DETOUR_COST),
            cache,
        )?;
        let coverage = score_coverage(
            &CoverageContext {
                obs,
                briefing,
                assets,
                approaches: &candidate_approaches,
                existing: &projection.existing,
                planned: &projection.planned,
            },
            profile,
            anchor,
        );
        if coverage.new == 0 && coverage.reinforced == 0 {
            return None;
        }
        let threat_distance = origins
            .iter()
            .map(|origin| origin.anchor.manhattan(anchor))
            .min()
            .unwrap_or(i32::MAX);
        Some((
            Candidate {
                anchor,
                builder_travel,
                coverage,
                threat_distance,
            },
            builder,
        ))
    });
    let selected = match search {
        DefenseSiteSearch::Best => candidates.max_by_key(|(candidate, _)| candidate.key(profile)),
        #[cfg(test)]
        DefenseSiteSearch::Any => candidates.take(1).next(),
    };
    let (candidate, builder) = selected?;
    let placement = profile.footprint(candidate.anchor);
    let candidate_approaches = cached_operationally_supported_approaches(
        ground,
        assets,
        approaches,
        placement,
        profile.domain,
        (profile.kind == BuildingKind::Barricade).then_some(MAX_BARRICADE_DETOUR_COST),
        cache,
    )?;
    let threat = candidate_threat_summary(
        obs,
        briefing,
        &projection.existing,
        &projection.planned,
        profile,
        candidate.anchor,
        &candidate_approaches,
        projection.evidence,
    )?;
    Some(StrategicDefenseQuote {
        placement: DefensePlacement {
            anchor: candidate.anchor,
            builder,
        },
        uncovered_value: candidate.coverage.unplanned_new,
        reinforced_value: candidate.coverage.unplanned_reinforced,
        builder_travel_cost: candidate.builder_travel,
        evidence: threat.evidence,
        evidence_count: threat.evidence_count,
        threat_arrival_ticks: threat.threat_arrival_ticks,
        blind_exposure: candidate.coverage.blind_exposure,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "summarizes one frozen defense candidate against its scoring context"
)]
fn candidate_threat_summary(
    obs: &Observation,
    briefing: &PublicMapBriefing,
    existing: &[&BuildingObs],
    planned: &[PlannedDefense],
    profile: DefenseProfile,
    candidate: TilePos,
    approaches: &[Approach],
    projection_evidence: DefenseOpportunityEvidence,
) -> Option<CandidateThreatSummary> {
    let mut sources = BTreeSet::new();
    let mut threat_arrival_ticks = None;
    for approach in approaches.iter().filter(|approach| {
        approach_contributes_marginal_protection(
            obs, briefing, existing, planned, profile, candidate, approach,
        )
    }) {
        sources.insert(approach.source);
        if let Some(arrival) = approach_threat_arrival_ticks(approach) {
            threat_arrival_ticks =
                Some(threat_arrival_ticks.map_or(arrival, |earliest: u64| earliest.min(arrival)));
        }
    }
    if sources.is_empty() {
        return None;
    }
    let evidence = match projection_evidence {
        DefenseOpportunityEvidence::CurrentArmed | DefenseOpportunityEvidence::CurrentFoothold => {
            if sources.iter().any(|source| {
                matches!(
                    source.capability,
                    ThreatCapability::Mobile(_) | ThreatCapability::StaticDefense { .. }
                )
            }) {
                DefenseOpportunityEvidence::CurrentArmed
            } else {
                DefenseOpportunityEvidence::CurrentFoothold
            }
        }
        DefenseOpportunityEvidence::Remembered => DefenseOpportunityEvidence::Remembered,
        DefenseOpportunityEvidence::PublicPrior => DefenseOpportunityEvidence::PublicPrior,
    };
    Some(CandidateThreatSummary {
        evidence,
        evidence_count: sources.len(),
        threat_arrival_ticks,
    })
}

fn approach_contributes_marginal_protection(
    obs: &Observation,
    briefing: &PublicMapBriefing,
    existing: &[&BuildingObs],
    planned: &[PlannedDefense],
    profile: DefenseProfile,
    candidate: TilePos,
    approach: &Approach,
) -> bool {
    if profile.kind == BuildingKind::Barricade {
        return approach.disrupted && path_cost(&approach.path) > approach.baseline_cost;
    }
    approach.path.iter().copied().any(|tile| {
        candidate_covers(obs, briefing, profile, candidate, tile)
            && !planned
                .iter()
                .any(|defense| planned_defense_covers(obs, briefing, defense, tile))
            && existing
                .iter()
                .filter(|building| building_covers(obs, briefing, building, tile, profile.domain))
                .take(2)
                .count()
                <= 1
    })
}

fn footprints_overlap(first: PlacementFootprint, second: PlacementFootprint) -> bool {
    first.anchor.x < second.anchor.x + second.size.0
        && second.anchor.x < first.anchor.x + first.size.0
        && first.anchor.y < second.anchor.y + second.size.1
        && second.anchor.y < first.anchor.y + first.size.1
}

fn approach_threat_arrival_ticks(approach: &Approach) -> Option<u64> {
    match approach.source.capability {
        ThreatCapability::Mobile(kind) => {
            Some(travel_ticks(approach.baseline_cost, kind.stats().speed))
        }
        ThreatCapability::StaticDefense { .. } => Some(0),
        ThreatCapability::Foothold => None,
    }
}

pub(super) fn travel_ticks(path_cost_tenths: u32, speed: Fx) -> u64 {
    if path_cost_tenths == 0 {
        return 0;
    }
    let distance = Fx::from_num(path_cost_tenths) / Fx::from_num(10);
    (distance / speed).ceil().to_num::<u64>()
}

fn defended_assets(
    policy: &UtilityPolicy,
    obs: &Observation,
    ground: &GroundKnowledge<'_>,
) -> Vec<DefendedAsset> {
    let foundry_count = obs
        .my_buildings
        .iter()
        .filter(|building| building.hp > 0 && building.kind == BuildingKind::Foundry)
        .count();
    let foundries: Vec<_> = obs
        .my_buildings
        .iter()
        .filter(|building| {
            building.hp > 0 && building.built && building.kind == BuildingKind::Foundry
        })
        .map(|building| building.anchor)
        .collect();
    let mut assets = Vec::new();
    for (index, building) in obs.my_buildings.iter().enumerate() {
        if building.hp == 0 {
            continue;
        }
        let base = match building.kind {
            BuildingKind::Foundry if foundry_count == 1 => 16,
            BuildingKind::Foundry => 12,
            BuildingKind::Crucible => 10,
            BuildingKind::Airworks => 9,
            BuildingKind::Fabricator => 8,
            BuildingKind::Extractor
                if foundries.iter().any(|foundry| {
                    UtilityPolicy::foundry_supports_extractor(*foundry, building.anchor)
                }) =>
            {
                8
            }
            BuildingKind::Extractor => 6,
            BuildingKind::Bastion | BuildingKind::Reclaimer => 5,
            BuildingKind::FlakTurret | BuildingKind::RepairBay => 4,
            BuildingKind::Array => 3,
            BuildingKind::Turret | BuildingKind::Barricade | BuildingKind::ScuttleCharge => 0,
        };
        if base == 0 {
            continue;
        }
        let active = building.built
            || building.tier > 0
            || obs
                .my_units
                .iter()
                .any(|unit| unit.site == Some(building.id));
        if !active {
            continue;
        }
        let queue_scrap = obs.my_queues.get(index).map_or(0, |queue| {
            queue
                .iter()
                .fold(0u32, |total, kind| total.saturating_add(kind.stats().cost))
        });
        let queue_bonus = queue_scrap.saturating_add(199) / 200;
        let full_value = base + u32::from(building.tier) + queue_bonus.min(4);
        let value = if building.built || building.tier > 0 {
            full_value
        } else {
            full_value.div_ceil(2)
        };
        assets.push(DefendedAsset {
            value,
            shape: AssetShape::Building {
                anchor: building.anchor,
                size: building.kind.tier_stats(building.tier).size,
            },
            access: None,
        });
    }

    assets.extend(scrap_assets(policy, ground, &foundries));
    assets.sort_by_key(asset_sort_key);
    assets
}

fn asset_sort_key(asset: &DefendedAsset) -> (i32, i32, u8) {
    match &asset.shape {
        AssetShape::Building { anchor, .. } => (anchor.y, anchor.x, 0),
        AssetShape::Scrap { tiles, .. } => {
            let first = tiles
                .first()
                .copied()
                .unwrap_or(TilePos::new(i32::MAX, i32::MAX));
            (first.y, first.x, 1)
        }
    }
}

fn scrap_assets(
    policy: &UtilityPolicy,
    ground: &GroundKnowledge<'_>,
    foundries: &[TilePos],
) -> Vec<DefendedAsset> {
    let mut remaining: BTreeSet<_> = ground
        .scrap
        .iter()
        .filter(|(tile, amount)| **amount > 0 && !policy.dead_nodes.contains(tile))
        .map(|(tile, _)| *tile)
        .collect();
    let mut clusters = Vec::new();
    while let Some(seed) = remaining
        .iter()
        .min_by_key(|tile| (tile.y, tile.x))
        .copied()
    {
        remaining.remove(&seed);
        let mut open = VecDeque::from([seed]);
        let mut tiles = Vec::new();
        while let Some(tile) = open.pop_front() {
            tiles.push(tile);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if (dx != 0 || dy != 0) && remaining.remove(&tile.offset(dx, dy)) {
                        open.push_back(tile.offset(dx, dy));
                    }
                }
            }
        }
        tiles.sort_by_key(|tile| (tile.y, tile.x));
        let work_tiles = scrap_work_tiles(ground, &tiles, None);
        if !resource_region_is_active(ground.obs, &tiles) {
            continue;
        }
        let support = foundries
            .iter()
            .filter(|foundry| {
                let own_distance = tiles
                    .iter()
                    .map(|tile| tile.chebyshev(**foundry))
                    .min()
                    .unwrap_or(i32::MAX);
                own_distance <= HOME_SALVAGE_RADIUS
            })
            .filter_map(|foundry| {
                shortest_path_between(
                    ground,
                    &building_doorsteps(ground, *foundry, BuildingKind::Foundry.base_stats().size),
                    &work_tiles,
                    None,
                    DefenseDomain::Ground,
                )
                .map(|(_, _goal, path)| AccessRoute {
                    foundry: *foundry,
                    work_tiles: work_tiles.clone(),
                    path,
                })
            })
            .min_by_key(|route| {
                (
                    route.path.len(),
                    route.foundry.y,
                    route.foundry.x,
                    route.path.clone(),
                )
            });
        let Some(access) = support else { continue };
        let total = tiles.iter().fold(0u32, |sum, tile| {
            sum.saturating_add(ground.scrap.get(tile).copied().unwrap_or(0))
        });
        let value = total.div_ceil(SCRAP_NODE_AMOUNT).min(8);
        if value > 0 {
            clusters.push(DefendedAsset {
                value,
                shape: AssetShape::Scrap { tiles, work_tiles },
                access: Some(access),
            });
        }
    }
    clusters
}

fn resource_region_is_active(obs: &Observation, resource_tiles: &[TilePos]) -> bool {
    obs.my_units.iter().any(|unit| {
        unit.kind.stats().harvest.is_some()
            && unit.harvesting.is_some_and(|target| {
                resource_tiles
                    .binary_search_by_key(&(target.y, target.x), |tile| (tile.y, tile.x))
                    .is_ok()
            })
    })
}

fn emergency_threat_origins(obs: &Observation, domain: DefenseDomain) -> Vec<ThreatOrigin> {
    let mut origins: Vec<_> = obs
        .enemy_units
        .iter()
        .filter(|unit| obs.visible(unit.tile))
        .filter(|unit| match domain {
            DefenseDomain::Ground => {
                unit.kind.stats().domain == Domain::Ground && unit_threatens_ground(unit.kind)
            }
            DefenseDomain::Air => {
                unit.kind.stats().domain == Domain::Air && unit_threatens_ground(unit.kind)
            }
        })
        .map(|unit| ThreatOrigin {
            anchor: unit.tile,
            size: None,
            capability: ThreatCapability::Mobile(unit.kind),
            tie: unit.id.0,
        })
        .collect();
    if domain == DefenseDomain::Ground {
        origins.extend(
            obs.enemy_buildings
                .iter()
                .filter(|building| building.seen && building.built && building.hp > 0)
                .filter(|building| {
                    building
                        .kind
                        .tier_stats(building.tier)
                        .weapons
                        .iter()
                        .any(|weapon| weapon.targets.ground)
                })
                .map(|building| ThreatOrigin {
                    anchor: building.anchor,
                    size: Some(building.kind.tier_stats(building.tier).size),
                    capability: ThreatCapability::StaticDefense {
                        kind: building.kind,
                        tier: building.tier,
                    },
                    tie: building.id.0,
                }),
        );
    }
    canonical_origins(&mut origins);
    origins
}

fn threat_origin_tiers(
    obs: &Observation,
    unit_contacts: &[UnitContact],
    building_contacts: &[BuildingContact],
    public_starts: &[StartingFoundry],
    domain: DefenseDomain,
) -> [Vec<ThreatOrigin>; 4] {
    let mut current_units: Vec<_> = obs
        .enemy_units
        .iter()
        .filter(|unit| match domain {
            DefenseDomain::Ground => ground_attacker(unit.kind),
            DefenseDomain::Air => air_attacker(unit.kind),
        })
        .map(|unit| ThreatOrigin {
            anchor: unit.tile,
            size: None,
            capability: ThreatCapability::Mobile(unit.kind),
            tie: unit.id.0,
        })
        .collect();
    canonical_origins(&mut current_units);
    let mut current_buildings: Vec<_> = obs
        .enemy_buildings
        .iter()
        .filter(|building| building.seen && building.built && building.hp > 0)
        .filter(|building| threat_building(building.kind, building.tier, domain))
        .map(|building| ThreatOrigin {
            anchor: building.anchor,
            size: Some(building.kind.tier_stats(building.tier).size),
            capability: building_threat_capability(building.kind, building.tier, domain),
            tie: building.id.0,
        })
        .collect();
    canonical_origins(&mut current_buildings);
    let mut remembered: Vec<_> = unit_contacts
        .iter()
        .filter(|contact| {
            contact.evidence == ContactEvidence::Remembered
                && contact.confidence_at(obs.tick) > 0
                && contact.hp > 0
                && match domain {
                    DefenseDomain::Ground => ground_attacker(contact.kind),
                    DefenseDomain::Air => air_attacker(contact.kind),
                }
        })
        .map(|contact| ThreatOrigin {
            anchor: contact.tile,
            size: None,
            capability: ThreatCapability::Mobile(contact.kind),
            tie: contact.id.0,
        })
        .collect();
    remembered.extend(
        building_contacts
            .iter()
            .filter(|contact| {
                contact.evidence == ContactEvidence::Remembered
                    && contact.confidence_at(obs.tick) > 0
                    && contact.built
                    && contact.hp > 0
                    && threat_building(contact.kind, contact.tier, domain)
            })
            .map(|contact| ThreatOrigin {
                anchor: contact.anchor,
                size: Some(contact.kind.tier_stats(contact.tier).size),
                capability: building_threat_capability(contact.kind, contact.tier, domain),
                tie: contact.id.map_or(u32::MAX, |id| id.0),
            }),
    );
    canonical_origins(&mut remembered);
    let mut starts: Vec<_> = public_starts
        .iter()
        .map(|start| ThreatOrigin {
            anchor: start.anchor,
            size: Some(BuildingKind::Foundry.base_stats().size),
            capability: ThreatCapability::Foothold,
            tie: u32::from(start.player.0),
        })
        .collect();
    canonical_origins(&mut starts);
    [current_units, current_buildings, remembered, starts]
}

fn canonical_origins(origins: &mut Vec<ThreatOrigin>) {
    origins.sort_by_key(|origin| (origin.anchor.y, origin.anchor.x, origin.tie));
    origins.dedup();
}

fn ground_attacker(kind: UnitKind) -> bool {
    let stats = kind.stats();
    stats.domain == Domain::Ground
        && (kind == UnitKind::Sapper || stats.weapons.iter().any(|weapon| weapon.targets.ground))
}

fn air_attacker(kind: UnitKind) -> bool {
    let stats = kind.stats();
    stats.domain == Domain::Air && (stats.transport_capacity > 0 || unit_threatens_ground(kind))
}

fn threat_building(kind: BuildingKind, tier: u8, domain: DefenseDomain) -> bool {
    building_is_foothold(kind, domain)
        || domain == DefenseDomain::Ground
            && kind
                .tier_stats(tier)
                .weapons
                .iter()
                .any(|weapon| weapon.targets.ground)
}

pub(super) fn unit_threatens_ground(kind: UnitKind) -> bool {
    kind == UnitKind::Sapper
        || kind
            .stats()
            .weapons
            .iter()
            .any(|weapon| weapon.targets.ground)
}

fn building_is_foothold(kind: BuildingKind, domain: DefenseDomain) -> bool {
    match domain {
        DefenseDomain::Ground => {
            kind == BuildingKind::Foundry
                || kind == BuildingKind::Extractor
                || !kind.base_stats().produces.is_empty()
        }
        DefenseDomain::Air => kind == BuildingKind::Airworks,
    }
}

fn building_threat_capability(
    kind: BuildingKind,
    tier: u8,
    domain: DefenseDomain,
) -> ThreatCapability {
    if kind
        .tier_stats(tier)
        .weapons
        .iter()
        .any(|weapon| weapon_targets_domain(weapon, domain))
    {
        ThreatCapability::StaticDefense { kind, tier }
    } else {
        ThreatCapability::Foothold
    }
}

fn approaches(
    ground: &GroundKnowledge<'_>,
    origins: &[ThreatOrigin],
    assets: &[DefendedAsset],
    candidate: Option<PlacementFootprint>,
    domain: DefenseDomain,
) -> Vec<Approach> {
    assets
        .iter()
        .enumerate()
        .flat_map(|(asset, defended)| {
            let goals = defended.shape.approach_tiles(ground, domain);
            origins.iter().filter_map(move |source| {
                approach_path(ground, *source, &defended.shape, &goals, candidate, domain).map(
                    |(goal, path)| Approach {
                        asset,
                        source: *source,
                        goal,
                        baseline_cost: path_cost(&path),
                        path,
                        disrupted: false,
                    },
                )
            })
        })
        .collect()
}

fn approach_path(
    ground: &GroundKnowledge<'_>,
    source: ThreatOrigin,
    asset: &AssetShape,
    goals: &[TilePos],
    candidate: Option<PlacementFootprint>,
    domain: DefenseDomain,
) -> Option<(TilePos, Vec<TilePos>)> {
    if let ThreatCapability::StaticDefense { kind, tier } = source.capability {
        return static_defense_attack(kind, tier, source.anchor, asset, ground.briefing, goals)
            .map(|(goal, source_tile)| (goal, vec![source_tile]));
    }

    if let ThreatCapability::Mobile(kind) = source.capability
        && domain == DefenseDomain::Ground
    {
        if let Some(goal) = goals
            .iter()
            .copied()
            .filter(|goal| {
                mobile_threat_can_attack_asset(kind, asset, ground.briefing, *goal, source.anchor)
            })
            .min_by_key(|goal| (goal.y, goal.x))
        {
            return Some((goal, vec![source.anchor]));
        }

        let retreat_goal = goals
            .iter()
            .copied()
            .filter(|goal| mobile_threat_inside_minimum_range(kind, asset, *goal, source.anchor))
            .min_by_key(|goal| {
                let aim = asset.aim_point(source.anchor.center(), *goal);
                (source.anchor.center().dist_sq(aim), goal.y, goal.x)
            });
        if let Some(goal) = retreat_goal {
            return retreat_to_mobile_firing_stand(ground, source, asset, goal, candidate)
                .map(|path| (goal, path));
        }
    }

    shortest_path_between(
        ground,
        &source.approach_tiles(ground, domain),
        goals,
        candidate,
        domain,
    )
    .map(|(_, goal, path)| {
        let path = mobile_ground_standoff(source, asset, ground.briefing, goal, path, domain);
        (goal, path)
    })
}

fn approach_path_cached(
    ground: &GroundKnowledge<'_>,
    source: ThreatOrigin,
    asset: &AssetShape,
    goals: &[TilePos],
    candidate: PlacementFootprint,
    domain: DefenseDomain,
    baseline_endpoint_routes: &mut BTreeMap<
        (DefenseDomain, TilePos, TilePos),
        BaselineEndpointRoute,
    >,
) -> Option<(TilePos, Vec<TilePos>)> {
    if let ThreatCapability::StaticDefense { kind, tier } = source.capability {
        return static_defense_attack(kind, tier, source.anchor, asset, ground.briefing, goals)
            .map(|(goal, source_tile)| (goal, vec![source_tile]));
    }

    if let ThreatCapability::Mobile(kind) = source.capability
        && domain == DefenseDomain::Ground
    {
        if let Some(goal) = goals
            .iter()
            .copied()
            .filter(|goal| {
                mobile_threat_can_attack_asset(kind, asset, ground.briefing, *goal, source.anchor)
            })
            .min_by_key(|goal| (goal.y, goal.x))
        {
            return Some((goal, vec![source.anchor]));
        }

        let retreat_goal = goals
            .iter()
            .copied()
            .filter(|goal| mobile_threat_inside_minimum_range(kind, asset, *goal, source.anchor))
            .min_by_key(|goal| {
                let aim = asset.aim_point(source.anchor.center(), *goal);
                (source.anchor.center().dist_sq(aim), goal.y, goal.x)
            });
        if let Some(goal) = retreat_goal {
            return retreat_to_mobile_firing_stand(ground, source, asset, goal, Some(candidate))
                .map(|path| (goal, path));
        }
    }

    shortest_path_between_cached(
        ground,
        &source.approach_tiles(ground, domain),
        goals,
        candidate,
        domain,
        baseline_endpoint_routes,
    )
    .map(|(_, goal, path)| {
        let path = mobile_ground_standoff(source, asset, ground.briefing, goal, path, domain);
        (goal, path)
    })
}

fn static_defense_attack(
    kind: BuildingKind,
    tier: u8,
    anchor: TilePos,
    asset: &AssetShape,
    briefing: &PublicMapBriefing,
    goals: &[TilePos],
) -> Option<(TilePos, TilePos)> {
    let stats = kind.tier_stats(tier);
    let shooter = footprint_center(anchor, stats.size);
    let goal = goals
        .iter()
        .copied()
        .filter(|goal| {
            let aim = asset.aim_point(shooter, *goal);
            stats
                .weapons
                .iter()
                .filter(|weapon| weapon.targets.ground)
                .any(|weapon| {
                    weapon_covers_point(briefing, shooter, weapon, aim, DefenseDomain::Ground)
                })
        })
        .min_by_key(|goal| (goal.y, goal.x))?;
    let aim = asset.aim_point(shooter, goal);
    let source_tile = footprint_tiles(anchor, stats.size)
        .into_iter()
        .min_by_key(|tile| (tile.center().dist_sq(aim), tile.y, tile.x))?;
    Some((goal, source_tile))
}

fn approaches_with_candidate(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    baseline: &[Approach],
    candidate: PlacementFootprint,
    domain: DefenseDomain,
    max_detour: Option<u32>,
) -> Option<Vec<Approach>> {
    approaches_with_candidate_inner(
        ground, assets, baseline, candidate, domain, max_detour, None,
    )
}

fn approaches_with_candidate_cached(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    baseline: &[Approach],
    candidate: PlacementFootprint,
    domain: DefenseDomain,
    max_detour: Option<u32>,
    baseline_endpoint_routes: &mut BTreeMap<
        (DefenseDomain, TilePos, TilePos),
        BaselineEndpointRoute,
    >,
) -> Option<Vec<Approach>> {
    approaches_with_candidate_inner(
        ground,
        assets,
        baseline,
        candidate,
        domain,
        max_detour,
        Some(baseline_endpoint_routes),
    )
}

fn approaches_with_candidate_inner(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    baseline: &[Approach],
    candidate: PlacementFootprint,
    domain: DefenseDomain,
    max_detour: Option<u32>,
    mut baseline_endpoint_routes: Option<
        &mut BTreeMap<(DefenseDomain, TilePos, TilePos), BaselineEndpointRoute>,
    >,
) -> Option<Vec<Approach>> {
    let mut rerouted = Vec::with_capacity(baseline.len());
    for approach in baseline {
        if !candidate_affects_path(&approach.path, candidate, domain) {
            rerouted.push(approach.clone());
            continue;
        }
        let goals = assets[approach.asset].shape.approach_tiles(ground, domain);
        let (goal, path) = match baseline_endpoint_routes.as_deref_mut() {
            Some(endpoint_routes) => approach_path_cached(
                ground,
                approach.source,
                &assets[approach.asset].shape,
                &goals,
                candidate,
                domain,
                endpoint_routes,
            ),
            None => approach_path(
                ground,
                approach.source,
                &assets[approach.asset].shape,
                &goals,
                Some(candidate),
                domain,
            ),
        }?;
        let detour = path_cost(&path).saturating_sub(approach.baseline_cost);
        if max_detour.is_some_and(|limit| detour > limit) {
            return None;
        }
        rerouted.push(Approach {
            asset: approach.asset,
            source: approach.source,
            goal,
            baseline_cost: approach.baseline_cost,
            path,
            disrupted: true,
        });
    }
    Some(rerouted)
}

fn mobile_ground_standoff(
    source: ThreatOrigin,
    asset: &AssetShape,
    briefing: &PublicMapBriefing,
    route_goal: TilePos,
    path: Vec<TilePos>,
    domain: DefenseDomain,
) -> Vec<TilePos> {
    let ThreatCapability::Mobile(kind) = source.capability else {
        return path;
    };
    if domain != DefenseDomain::Ground {
        return path;
    }
    let Some(index) = path
        .iter()
        .position(|tile| mobile_threat_can_attack_asset(kind, asset, briefing, route_goal, *tile))
    else {
        return path;
    };
    path[..=index].to_vec()
}

fn mobile_threat_inside_minimum_range(
    kind: UnitKind,
    asset: &AssetShape,
    route_goal: TilePos,
    stand: TilePos,
) -> bool {
    let Some(weapon) = kind
        .stats()
        .weapons
        .iter()
        .find(|weapon| weapon.targets.ground)
    else {
        return false;
    };
    let shooter = stand.center();
    shooter.dist_sq(asset.aim_point(shooter, route_goal))
        < weapon.minimum_range * weapon.minimum_range
}

fn retreat_to_mobile_firing_stand(
    ground: &GroundKnowledge<'_>,
    source: ThreatOrigin,
    asset: &AssetShape,
    route_goal: TilePos,
    candidate: Option<PlacementFootprint>,
) -> Option<Vec<TilePos>> {
    let ThreatCapability::Mobile(kind) = source.capability else {
        return None;
    };
    let weapon = kind
        .stats()
        .weapons
        .iter()
        .find(|weapon| weapon.targets.ground)?;
    let (min_x, min_y, max_x, max_y) = match asset {
        AssetShape::Building { anchor, size } => (
            anchor.x,
            anchor.y,
            anchor.x + size.0 - 1,
            anchor.y + size.1 - 1,
        ),
        AssetShape::Scrap { .. } => (route_goal.x, route_goal.y, route_goal.x, route_goal.y),
    };
    let reach = weapon.range.ceil().to_num::<i32>();
    let mut candidates = Vec::new();
    for y in (min_y - reach).max(0)..=(max_y + reach).min(ground.obs.map_height - 1) {
        for x in (min_x - reach).max(0)..=(max_x + reach).min(ground.obs.map_width - 1) {
            let tile = TilePos::new(x, y);
            if ground.open(tile, candidate, DefenseDomain::Ground)
                && mobile_threat_can_attack_asset(kind, asset, ground.briefing, route_goal, tile)
            {
                candidates.push((
                    source.anchor.center().dist_sq(tile.center()),
                    tile.y,
                    tile.x,
                    tile,
                ));
            }
        }
    }
    candidates.sort_unstable_by_key(|candidate| (candidate.0, candidate.1, candidate.2));
    candidates.into_iter().find_map(|(_, _, _, stand)| {
        shortest_path_between(
            ground,
            &[source.anchor],
            &[stand],
            candidate,
            DefenseDomain::Ground,
        )
        .map(|(_, _, path)| path)
    })
}

fn mobile_threat_can_attack_asset(
    kind: UnitKind,
    asset: &AssetShape,
    briefing: &PublicMapBriefing,
    route_goal: TilePos,
    stand: TilePos,
) -> bool {
    let shooter = stand.center();
    let aim = asset.aim_point(shooter, route_goal);
    if kind == UnitKind::Sapper {
        return shooter.dist_sq(aim) <= SAPPER_CONTACT_RANGE * SAPPER_CONTACT_RANGE;
    }
    kind.stats()
        .weapons
        .iter()
        .find(|weapon| weapon.targets.ground)
        .is_some_and(|weapon| {
            weapon_covers_point(briefing, shooter, weapon, aim, DefenseDomain::Ground)
        })
}

fn operationally_supported_approaches(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    baseline: &[Approach],
    candidate: PlacementFootprint,
    domain: DefenseDomain,
    max_detour: Option<u32>,
) -> Option<Vec<Approach>> {
    let supported_assets = supported_assets_for_candidate(ground, assets, candidate);

    Some(
        approaches_with_candidate(ground, assets, baseline, candidate, domain, max_detour)?
            .into_iter()
            .filter(|approach| supported_assets.contains(&approach.asset))
            .collect(),
    )
}

fn supported_assets_for_candidate(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    candidate: PlacementFootprint,
) -> BTreeSet<usize> {
    let doorsteps = building_doorsteps(ground, candidate.anchor, candidate.size);
    let locally_reachable =
        tiles_reachable_within_steps(ground, &doorsteps, candidate, DEFENSE_RADIUS as usize);
    assets
        .iter()
        .enumerate()
        .filter_map(|(index, asset)| {
            let goals = asset.shape.approach_tiles(ground, DefenseDomain::Ground);
            if !doorsteps.iter().any(|start| {
                goals
                    .iter()
                    .any(|goal| start.chebyshev(*goal) <= DEFENSE_RADIUS)
            }) {
                return None;
            }
            if !goals.iter().any(|goal| {
                tile_index(ground.obs.map_width, ground.obs.map_height, *goal)
                    .is_some_and(|goal| locally_reachable[goal])
            }) {
                return None;
            }
            shortest_path_between(
                ground,
                &doorsteps,
                &goals,
                Some(candidate),
                DefenseDomain::Ground,
            )
            .filter(|(_, _, path)| path.len().saturating_sub(1) <= DEFENSE_RADIUS as usize)
            .map(|_| index)
        })
        .collect()
}

fn cached_operationally_supported_approaches(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    baseline: &[Approach],
    candidate: PlacementFootprint,
    domain: DefenseDomain,
    max_detour: Option<u32>,
    cache: &mut DefenseEvaluationCache,
) -> Option<Vec<Approach>> {
    match cache.supported_assets.entry(candidate) {
        std::collections::btree_map::Entry::Occupied(_) => {
            #[cfg(test)]
            {
                cache.stats.supported_assets_hits += 1;
            }
        }
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(supported_assets_for_candidate(ground, assets, candidate));
            #[cfg(test)]
            {
                cache.stats.supported_assets_builds += 1;
            }
        }
    }
    let reroute_key = (domain, candidate);
    match cache.rerouted_approaches.entry(reroute_key) {
        std::collections::btree_map::Entry::Occupied(_) => {
            #[cfg(test)]
            {
                cache.stats.rerouted_approach_hits += 1;
            }
        }
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(approaches_with_candidate_cached(
                ground,
                assets,
                baseline,
                candidate,
                domain,
                None,
                &mut cache.baseline_endpoint_routes,
            ));
            #[cfg(test)]
            {
                cache.stats.rerouted_approach_builds += 1;
            }
        }
    }
    let rerouted = cache.rerouted_approaches.get(&reroute_key)?.as_ref()?;
    if max_detour.is_some_and(|limit| {
        rerouted.iter().any(|approach| {
            approach.disrupted
                && path_cost(&approach.path).saturating_sub(approach.baseline_cost) > limit
        })
    }) {
        return None;
    }
    let supported_assets = cache.supported_assets.get(&candidate)?;
    Some(
        rerouted
            .iter()
            .filter(|approach| supported_assets.contains(&approach.asset))
            .cloned()
            .collect(),
    )
}

/// Cheap exact-negative preflight for the local support-radius query.
///
/// The ranked path remains authoritative below. This flood only avoids asking
/// whole-map A* to prove that no route of at most `maximum_steps` can exist;
/// whenever it answers yes, the original canonical path and length check still
/// decide the result.
fn tiles_reachable_within_steps(
    ground: &GroundKnowledge<'_>,
    starts: &[TilePos],
    candidate: PlacementFootprint,
    maximum_steps: usize,
) -> Vec<bool> {
    let width = ground.obs.map_width;
    let height = ground.obs.map_height;
    let area = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .unwrap_or(0);
    let mut distance = vec![u8::MAX; area];
    let mut open = VecDeque::new();
    for start in starts.iter().copied() {
        let Some(index) = tile_index(width, height, start) else {
            continue;
        };
        if distance[index] == u8::MAX {
            distance[index] = 0;
            open.push_back(start);
        }
    }

    while let Some(current) = open.pop_front() {
        let current_index = tile_index(width, height, current)
            .expect("the local route flood contains only in-bounds tiles");
        let steps = usize::from(distance[current_index]);
        if steps >= maximum_steps {
            continue;
        }
        let mut cardinal_open = [false; 4];
        for (dx, dy) in CARDINALS {
            let next = current.offset(dx, dy);
            if ground.open(next, Some(candidate), DefenseDomain::Ground) {
                let slot = if dy == 0 {
                    usize::from(dx < 0)
                } else {
                    2 + usize::from(dy < 0)
                };
                cardinal_open[slot] = true;
                enqueue_local_route_tile(&mut distance, &mut open, width, height, next, steps + 1);
            }
        }
        for (dx, dy) in DIAGONALS {
            if cardinal_open[usize::from(dx < 0)] && cardinal_open[2 + usize::from(dy < 0)] {
                let next = current.offset(dx, dy);
                if ground.open(next, Some(candidate), DefenseDomain::Ground) {
                    enqueue_local_route_tile(
                        &mut distance,
                        &mut open,
                        width,
                        height,
                        next,
                        steps + 1,
                    );
                }
            }
        }
    }

    distance
        .into_iter()
        .map(|steps| usize::from(steps) <= maximum_steps)
        .collect()
}

fn enqueue_local_route_tile(
    distance: &mut [u8],
    open: &mut VecDeque<TilePos>,
    width: i32,
    height: i32,
    tile: TilePos,
    steps: usize,
) {
    let Some(index) = tile_index(width, height, tile) else {
        return;
    };
    if usize::from(distance[index]) <= steps {
        return;
    }
    distance[index] = u8::try_from(steps).unwrap_or(u8::MAX);
    open.push_back(tile);
}

fn shortest_path_between(
    ground: &GroundKnowledge<'_>,
    starts: &[TilePos],
    goals: &[TilePos],
    candidate: Option<PlacementFootprint>,
    domain: DefenseDomain,
) -> Option<(TilePos, TilePos, Vec<TilePos>)> {
    let mut pairs: Vec<_> = starts
        .iter()
        .flat_map(|start| goals.iter().map(move |goal| (*start, *goal)))
        .collect();
    pairs.sort_unstable_by_key(|(start, goal)| {
        (octile_cost(*start, *goal), start.y, start.x, goal.y, goal.x)
    });

    let mut best: Option<(TilePos, TilePos, Vec<TilePos>)> = None;
    let mut proven_unreachable = BTreeSet::new();
    let mut scratch = chassis::path::AstarScratch::default();
    for (start, goal) in pairs {
        if proven_unreachable.contains(&(start, goal)) {
            continue;
        }
        if best
            .as_ref()
            .is_some_and(|(_, _, path)| octile_cost(start, goal) > path_cost(path))
        {
            // Every remaining pair has a strictly worse obstacle-free lower
            // bound, so none can replace the complete route-choice key.
            break;
        }
        let Some(mut path) = chassis::path::astar_with_scratch(
            ground.obs.map_width,
            ground.obs.map_height,
            start,
            goal,
            |tile| ground.open(tile, candidate, domain),
            PATH_EXPANSION_CAP,
            &mut scratch,
        ) else {
            if scratch.last_search_exhausted() {
                // One exhaustive search proves the whole passability
                // component. Reuse that proof for its other doorsteps.
                let reached_starts: Vec<_> = starts
                    .iter()
                    .copied()
                    .filter(|tile| scratch.last_search_reached(*tile))
                    .collect();
                for reached_start in reached_starts {
                    for unreachable_goal in goals
                        .iter()
                        .copied()
                        .filter(|tile| !scratch.last_search_reached(*tile))
                    {
                        proven_unreachable.insert((reached_start, unreachable_goal));
                    }
                }
            }
            continue;
        };
        path.insert(0, start);
        let replace = best
            .as_ref()
            .is_none_or(|(best_start, best_goal, best_path)| {
                (
                    path_cost(&path),
                    path.len(),
                    start.y,
                    start.x,
                    goal.y,
                    goal.x,
                    path.as_slice(),
                ) < (
                    path_cost(best_path),
                    best_path.len(),
                    best_start.y,
                    best_start.x,
                    best_goal.y,
                    best_goal.x,
                    best_path.as_slice(),
                )
            });
        if replace {
            best = Some((start, goal, path));
        }
    }
    best
}

fn shortest_path_between_cached(
    ground: &GroundKnowledge<'_>,
    starts: &[TilePos],
    goals: &[TilePos],
    candidate: PlacementFootprint,
    domain: DefenseDomain,
    baseline_endpoint_routes: &mut BTreeMap<
        (DefenseDomain, TilePos, TilePos),
        BaselineEndpointRoute,
    >,
) -> Option<(TilePos, TilePos, Vec<TilePos>)> {
    let mut pairs: Vec<_> = starts
        .iter()
        .flat_map(|start| goals.iter().map(move |goal| (*start, *goal)))
        .collect();
    pairs.sort_unstable_by_key(|(start, goal)| {
        (octile_cost(*start, *goal), start.y, start.x, goal.y, goal.x)
    });

    let mut best: Option<(TilePos, TilePos, Vec<TilePos>)> = None;
    let mut scratch = chassis::path::AstarScratch::default();
    for (start, goal) in pairs {
        if best
            .as_ref()
            .is_some_and(|(_, _, path)| octile_cost(start, goal) > path_cost(path))
        {
            break;
        }

        let key = (domain, start, goal);
        let baseline = match baseline_endpoint_routes.entry(key) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let mut path = chassis::path::astar_with_scratch(
                    ground.obs.map_width,
                    ground.obs.map_height,
                    start,
                    goal,
                    |tile| ground.open(tile, None, domain),
                    PATH_EXPANSION_CAP,
                    &mut scratch,
                );
                if let Some(path) = path.as_mut() {
                    path.insert(0, start);
                }
                entry.insert(BaselineEndpointRoute {
                    path,
                    exhausted: scratch.last_search_exhausted(),
                })
            }
        };
        if best.as_ref().is_some_and(|(_, _, best_path)| {
            baseline
                .path
                .as_ref()
                .is_some_and(|baseline_path| path_cost(baseline_path) > path_cost(best_path))
        }) {
            // A blocking footprint cannot improve this endpoint pair over its
            // no-candidate route. Keep equal-cost pairs because the complete
            // route-choice key can still prefer their length, endpoints, or
            // canonical path.
            continue;
        }
        let path = match &baseline.path {
            Some(path) if !candidate_affects_path(path, candidate, domain) => Some(path.clone()),
            Some(_) => {
                let mut path = chassis::path::astar_with_scratch(
                    ground.obs.map_width,
                    ground.obs.map_height,
                    start,
                    goal,
                    |tile| ground.open(tile, Some(candidate), domain),
                    PATH_EXPANSION_CAP,
                    &mut scratch,
                );
                if let Some(path) = path.as_mut() {
                    path.insert(0, start);
                }
                path
            }
            None if baseline.exhausted => None,
            None => {
                let mut path = chassis::path::astar_with_scratch(
                    ground.obs.map_width,
                    ground.obs.map_height,
                    start,
                    goal,
                    |tile| ground.open(tile, Some(candidate), domain),
                    PATH_EXPANSION_CAP,
                    &mut scratch,
                );
                if let Some(path) = path.as_mut() {
                    path.insert(0, start);
                }
                path
            }
        };
        let Some(path) = path else { continue };
        let replace = best
            .as_ref()
            .is_none_or(|(best_start, best_goal, best_path)| {
                (
                    path_cost(&path),
                    path.len(),
                    start.y,
                    start.x,
                    goal.y,
                    goal.x,
                    path.as_slice(),
                ) < (
                    path_cost(best_path),
                    best_path.len(),
                    best_start.y,
                    best_start.x,
                    best_goal.y,
                    best_goal.x,
                    best_path.as_slice(),
                )
            });
        if replace {
            best = Some((start, goal, path));
        }
    }
    best
}

fn octile_cost(start: TilePos, goal: TilePos) -> u32 {
    let dx = (start.x - goal.x).unsigned_abs();
    let dy = (start.y - goal.y).unsigned_abs();
    10 * dx.max(dy) + 4 * dx.min(dy)
}

#[cfg(test)]
fn shortest_path_between_exhaustive(
    ground: &GroundKnowledge<'_>,
    starts: &[TilePos],
    goals: &[TilePos],
    candidate: Option<PlacementFootprint>,
    domain: DefenseDomain,
) -> Option<(TilePos, TilePos, Vec<TilePos>)> {
    starts
        .iter()
        .flat_map(|start| goals.iter().map(move |goal| (*start, *goal)))
        .filter_map(|(start, goal)| {
            chassis::path::astar(
                ground.obs.map_width,
                ground.obs.map_height,
                start,
                goal,
                |tile| ground.open(tile, candidate, domain),
                PATH_EXPANSION_CAP,
            )
            .map(|mut path| {
                path.insert(0, start);
                (start, goal, path)
            })
        })
        .min_by_key(|(start, goal, path)| {
            (
                path_cost(path),
                path.len(),
                start.y,
                start.x,
                goal.y,
                goal.x,
                path.clone(),
            )
        })
}

fn path_cost(path: &[TilePos]) -> u32 {
    path.windows(2).fold(0, |cost, pair| {
        cost + if pair[0].x != pair[1].x && pair[0].y != pair[1].y {
            14
        } else {
            10
        }
    })
}

fn candidate_affects_path(
    path: &[TilePos],
    candidate: PlacementFootprint,
    domain: DefenseDomain,
) -> bool {
    if domain == DefenseDomain::Air || !candidate.blocks_ground {
        return false;
    }
    path.iter().any(|tile| candidate.blocks(*tile))
        || path.windows(2).any(|pair| {
            pair[0].x != pair[1].x
                && pair[0].y != pair[1].y
                && (candidate.blocks(TilePos::new(pair[1].x, pair[0].y))
                    || candidate.blocks(TilePos::new(pair[0].x, pair[1].y)))
        })
}

fn scrap_work_tiles(
    ground: &GroundKnowledge<'_>,
    cluster: &[TilePos],
    candidate: Option<PlacementFootprint>,
) -> Vec<TilePos> {
    let cluster: BTreeSet<_> = cluster.iter().copied().collect();
    let mut work_tiles = Vec::new();
    for tile in &cluster {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let neighbor = tile.offset(dx, dy);
                if !cluster.contains(&neighbor)
                    && ground.open(neighbor, candidate, DefenseDomain::Ground)
                {
                    work_tiles.push(neighbor);
                }
            }
        }
    }
    sorted_tiles(work_tiles)
}

fn building_doorsteps(
    ground: &GroundKnowledge<'_>,
    anchor: TilePos,
    size: (i32, i32),
) -> Vec<TilePos> {
    sorted_tiles(
        crate::tick::rect_adjacent_tiles(anchor, size)
            .filter(|tile| ground.open(*tile, None, DefenseDomain::Ground)),
    )
}

fn scrap_access_survives(
    ground: &GroundKnowledge<'_>,
    assets: &[DefendedAsset],
    candidate: PlacementFootprint,
    max_detour: Option<u32>,
) -> bool {
    if !candidate.blocks_ground {
        return true;
    }
    assets.iter().all(|asset| {
        let Some(access) = &asset.access else {
            return true;
        };
        if !candidate_affects_path(&access.path, candidate, DefenseDomain::Ground) {
            return true;
        }
        shortest_path_between(
            ground,
            &building_doorsteps(
                ground,
                access.foundry,
                BuildingKind::Foundry.base_stats().size,
            ),
            &access.work_tiles,
            Some(candidate),
            DefenseDomain::Ground,
        )
        .is_some_and(|(_, _, path)| {
            let detour = path_cost(&path).saturating_sub(path_cost(&access.path));
            max_detour.is_none_or(|limit| detour <= limit)
        })
    })
}

fn existing_defenses(obs: &Observation, domain: DefenseDomain) -> Vec<&BuildingObs> {
    obs.my_buildings
        .iter()
        .chain(obs.ally_buildings.iter())
        .filter(|building| {
            building.built
                && building.hp > 0
                && (building.kind == BuildingKind::ScuttleCharge && domain == DefenseDomain::Ground
                    || building
                        .kind
                        .tier_stats(building.tier)
                        .weapons
                        .iter()
                        .any(|weapon| weapon_targets_domain(weapon, domain)))
        })
        .collect()
}

fn planned_defenses(obs: &Observation, domain: DefenseDomain) -> Vec<PlannedDefense> {
    let mut planned: Vec<_> = obs
        .my_buildings
        .iter()
        .chain(obs.ally_buildings.iter())
        .filter(|building| !building.built && building.hp > 0)
        .filter_map(|building| {
            let profile = DefenseProfile::for_kind(building.kind)?;
            (profile.domain == domain).then_some(PlannedDefense {
                profile,
                anchor: building.anchor,
                tier: building.tier,
            })
        })
        .chain(obs.my_units.iter().filter_map(|unit| {
            let (kind, anchor) = unit.founding?;
            let profile = DefenseProfile::for_kind(kind)?;
            (profile.domain == domain).then_some(PlannedDefense {
                profile,
                anchor,
                tier: 0,
            })
        }))
        .collect();
    planned.sort_by_key(|defense| {
        (
            defense.anchor.y,
            defense.anchor.x,
            defense.profile.kind,
            defense.tier,
        )
    });
    planned.dedup();
    planned
}

struct CoverageContext<'a> {
    obs: &'a Observation,
    briefing: &'a PublicMapBriefing,
    assets: &'a [DefendedAsset],
    approaches: &'a [Approach],
    existing: &'a [&'a BuildingObs],
    planned: &'a [PlannedDefense],
}

fn score_coverage(
    context: &CoverageContext<'_>,
    profile: DefenseProfile,
    candidate: TilePos,
) -> Coverage {
    let CoverageContext {
        obs,
        briefing,
        assets,
        approaches,
        existing,
        planned,
    } = context;
    let mut coverage = Coverage {
        new: 0,
        reinforced: 0,
        unplanned_new: 0,
        unplanned_reinforced: 0,
        interception: 0,
        protected_value: 0,
        planned_overlap: 0,
        blind_exposure: 0,
        spotted_reach: 0,
        redundant: 0,
        lateral: i32::MAX,
    };
    for (asset_index, asset) in assets.iter().enumerate() {
        let asset_approaches = approaches
            .iter()
            .filter(|approach| approach.asset == asset_index);
        if profile.kind == BuildingKind::Barricade {
            let best_detour = asset_approaches
                .filter(|approach| approach.disrupted)
                .map(|approach| path_cost(&approach.path).saturating_sub(approach.baseline_cost))
                .filter(|detour| *detour > 0)
                .max();
            if let Some(detour) = best_detour {
                coverage.new = coverage.new.saturating_add(asset.value);
                coverage.unplanned_new = coverage.unplanned_new.saturating_add(asset.value);
                coverage.protected_value = coverage.protected_value.saturating_add(asset.value);
                coverage.interception = coverage.interception.saturating_add(
                    asset
                        .value
                        .saturating_mul(detour.div_ceil(10).min(INTERCEPTION_DEPTH as u32)),
                );
                coverage.lateral = 0;
                if planned.iter().any(|defense| {
                    defense.profile.kind == BuildingKind::Barricade
                        && defense.anchor.chebyshev(candidate) <= 2
                }) {
                    coverage.planned_overlap = coverage
                        .planned_overlap
                        .saturating_add(asset.value.saturating_mul(detour.div_ceil(10)));
                }
            }
            continue;
        }

        let mut protects = false;
        let mut adds_new = false;
        let mut reinforces = false;
        let mut adds_unplanned_new = false;
        let mut adds_unplanned_reinforcement = false;
        let mut planned_overlap_tiles = 0u32;
        let mut live_overlap_tiles = 0u32;
        let mut blind_exposure = false;
        let mut uses_spotter = false;
        let mut best_depth = 0;
        for approach in asset_approaches {
            for (index, tile) in approach.path.iter().copied().enumerate() {
                if profile.kind == BuildingKind::Bastion
                    && inside_minimum_range(profile, candidate, tile)
                    && !existing.iter().any(|building| {
                        building_covers(obs, briefing, building, tile, DefenseDomain::Ground)
                    })
                {
                    blind_exposure = true;
                }
                if !candidate_covers(obs, briefing, profile, candidate, tile) {
                    continue;
                }
                protects = true;
                coverage.lateral = coverage.lateral.min(tile.chebyshev(candidate));
                if profile.kind == BuildingKind::Bastion
                    && !candidate_sees(profile, candidate, tile)
                {
                    uses_spotter = true;
                }
                let planned_coverage = planned
                    .iter()
                    .any(|defense| planned_defense_covers(obs, briefing, defense, tile));
                if planned_coverage {
                    planned_overlap_tiles = planned_overlap_tiles.saturating_add(1);
                }
                let existing_count = existing
                    .iter()
                    .filter(|building| {
                        building_covers(obs, briefing, building, tile, profile.domain)
                    })
                    .count();
                match existing_count {
                    0 => {
                        adds_new = true;
                        adds_unplanned_new |= !planned_coverage;
                    }
                    1 => {
                        reinforces = true;
                        adds_unplanned_reinforcement |= !planned_coverage;
                        live_overlap_tiles = live_overlap_tiles.saturating_add(1);
                    }
                    _ => live_overlap_tiles = live_overlap_tiles.saturating_add(1),
                }
                let depth = approach
                    .path
                    .len()
                    .saturating_sub(1)
                    .saturating_sub(index)
                    .min(INTERCEPTION_DEPTH) as u32;
                best_depth = best_depth.max(depth);
            }
        }
        if protects {
            coverage.protected_value = coverage.protected_value.saturating_add(asset.value);
            if adds_new {
                coverage.new = coverage.new.saturating_add(asset.value);
            } else if reinforces {
                coverage.reinforced = coverage.reinforced.saturating_add(asset.value);
            }
            if adds_unplanned_new {
                coverage.unplanned_new = coverage.unplanned_new.saturating_add(asset.value);
            } else if adds_unplanned_reinforcement {
                coverage.unplanned_reinforced =
                    coverage.unplanned_reinforced.saturating_add(asset.value);
            }
            if planned_overlap_tiles > 0 {
                coverage.planned_overlap = coverage.planned_overlap.saturating_add(
                    asset
                        .value
                        .saturating_mul(planned_overlap_tiles.min(INTERCEPTION_DEPTH as u32)),
                );
            }
            if live_overlap_tiles > 0 {
                coverage.redundant = coverage.redundant.saturating_add(
                    asset
                        .value
                        .saturating_mul(live_overlap_tiles.min(INTERCEPTION_DEPTH as u32)),
                );
            }
            if blind_exposure {
                coverage.blind_exposure = coverage.blind_exposure.saturating_add(asset.value);
            }
            if uses_spotter {
                coverage.spotted_reach = coverage.spotted_reach.saturating_add(asset.value);
            }
            coverage.interception = coverage
                .interception
                .saturating_add(asset.value.saturating_mul(best_depth));
        }
    }
    coverage
}

fn candidate_covers(
    obs: &Observation,
    briefing: &PublicMapBriefing,
    profile: DefenseProfile,
    anchor: TilePos,
    target: TilePos,
) -> bool {
    defense_covers(briefing, profile, anchor, target)
        && (profile.kind != BuildingKind::Bastion
            || candidate_sees(profile, anchor, target)
            || durable_spotter_sees(obs, target))
}

fn durable_spotter_sees(obs: &Observation, target: TilePos) -> bool {
    obs.my_buildings
        .iter()
        .chain(obs.ally_buildings.iter())
        .filter(|building| building.built && building.hp > 0)
        .any(|building| {
            let stats = building.kind.tier_stats(building.tier);
            footprint_sees(building.anchor, stats.size, stats.vision, target)
        })
}

fn candidate_sees(profile: DefenseProfile, anchor: TilePos, target: TilePos) -> bool {
    let stats = profile.kind.base_stats();
    footprint_sees(anchor, stats.size, stats.vision, target)
}

fn footprint_sees(anchor: TilePos, size: (i32, i32), vision: i32, target: TilePos) -> bool {
    let horizontal = if target.x < anchor.x {
        anchor.x - target.x
    } else if target.x >= anchor.x + size.0 {
        target.x - (anchor.x + size.0 - 1)
    } else {
        0
    };
    let vertical = if target.y < anchor.y {
        anchor.y - target.y
    } else if target.y >= anchor.y + size.1 {
        target.y - (anchor.y + size.1 - 1)
    } else {
        0
    };
    horizontal * horizontal + vertical * vertical <= vision * vision
}

fn inside_minimum_range(profile: DefenseProfile, anchor: TilePos, target: TilePos) -> bool {
    let stats = profile.kind.base_stats();
    let center = footprint_center(anchor, stats.size);
    stats
        .weapons
        .iter()
        .filter(|weapon| weapon_targets_domain(weapon, profile.domain))
        .any(|weapon| center.dist_sq(target.center()) < weapon.minimum_range * weapon.minimum_range)
}

fn defense_covers(
    briefing: &PublicMapBriefing,
    profile: DefenseProfile,
    anchor: TilePos,
    target: TilePos,
) -> bool {
    if profile.kind == BuildingKind::ScuttleCharge {
        return anchor.center().dist_sq(target.center())
            <= CHARGE_TRIGGER_RADIUS * CHARGE_TRIGGER_RADIUS;
    }
    let stats = profile.kind.base_stats();
    let center = footprint_center(anchor, stats.size);
    stats
        .weapons
        .iter()
        .filter(|weapon| weapon_targets_domain(weapon, profile.domain))
        .any(|weapon| weapon_covers(briefing, center, weapon, target, profile.domain))
}

fn building_covers(
    obs: &Observation,
    briefing: &PublicMapBriefing,
    building: &BuildingObs,
    target: TilePos,
    domain: DefenseDomain,
) -> bool {
    if building.kind == BuildingKind::ScuttleCharge {
        return domain == DefenseDomain::Ground
            && building.anchor.center().dist_sq(target.center())
                <= CHARGE_TRIGGER_RADIUS * CHARGE_TRIGGER_RADIUS;
    }
    let stats = building.kind.tier_stats(building.tier);
    let center = footprint_center(building.anchor, stats.size);
    stats
        .weapons
        .iter()
        .filter(|weapon| weapon_targets_domain(weapon, domain))
        .any(|weapon| weapon_covers(briefing, center, weapon, target, domain))
        && (building.kind != BuildingKind::Bastion
            || footprint_sees(building.anchor, stats.size, stats.vision, target)
            || durable_spotter_sees(obs, target))
}

pub(super) fn completed_ground_defense_covers_asset(
    obs: &Observation,
    briefing: &PublicMapBriefing,
    defense: &BuildingObs,
    asset_anchor: TilePos,
    asset_size: (i32, i32),
) -> bool {
    defense.built
        && defense.hp > 0
        && (0..asset_size.1).all(|dy| {
            (0..asset_size.0).all(|dx| {
                building_covers(
                    obs,
                    briefing,
                    defense,
                    asset_anchor.offset(dx, dy),
                    DefenseDomain::Ground,
                )
            })
        })
}

fn planned_defense_covers(
    obs: &Observation,
    briefing: &PublicMapBriefing,
    defense: &PlannedDefense,
    target: TilePos,
) -> bool {
    if defense.profile.kind == BuildingKind::ScuttleCharge {
        return defense.profile.domain == DefenseDomain::Ground
            && defense.anchor.center().dist_sq(target.center())
                <= CHARGE_TRIGGER_RADIUS * CHARGE_TRIGGER_RADIUS;
    }
    let stats = defense.profile.kind.tier_stats(defense.tier);
    let center = footprint_center(defense.anchor, stats.size);
    stats
        .weapons
        .iter()
        .filter(|weapon| weapon_targets_domain(weapon, defense.profile.domain))
        .any(|weapon| weapon_covers(briefing, center, weapon, target, defense.profile.domain))
        && (defense.profile.kind != BuildingKind::Bastion
            || footprint_sees(defense.anchor, stats.size, stats.vision, target)
            || durable_spotter_sees(obs, target))
}

fn weapon_targets_domain(weapon: &WeaponStats, domain: DefenseDomain) -> bool {
    match domain {
        DefenseDomain::Ground => weapon.targets.ground,
        DefenseDomain::Air => weapon.targets.air,
    }
}

fn weapon_covers(
    briefing: &PublicMapBriefing,
    shooter: Vec2Fx,
    weapon: &WeaponStats,
    target: TilePos,
    target_domain: DefenseDomain,
) -> bool {
    weapon_covers_point(briefing, shooter, weapon, target.center(), target_domain)
}

fn weapon_covers_point(
    briefing: &PublicMapBriefing,
    shooter: Vec2Fx,
    weapon: &WeaponStats,
    target: Vec2Fx,
    target_domain: DefenseDomain,
) -> bool {
    let distance = shooter.dist_sq(target);
    if distance < weapon.minimum_range * weapon.minimum_range
        || distance > weapon.range * weapon.range
    {
        return false;
    }
    let full_cover = !weapon.indirect && target_domain == DefenseDomain::Ground;
    let crosses = |tile: TilePos| {
        briefing.terrain_at(tile).is_some_and(|terrain| {
            !terrain.blocks_all_fire() && (!full_cover || !terrain.blocks_direct_fire())
        })
    };
    crosses(TilePos::containing(target)) && !chassis::path::line_blocked(shooter, target, crosses)
}

fn closest_point_to_footprint(anchor: TilePos, size: (i32, i32), from: Vec2Fx) -> Vec2Fx {
    let half = Fx::lit("0.5");
    let min = anchor.center() - Vec2Fx::new(half, half);
    let max = min + Vec2Fx::new(Fx::from_num(size.0), Fx::from_num(size.1));
    Vec2Fx::new(from.x.clamp(min.x, max.x), from.y.clamp(min.y, max.y))
}

fn footprint_center(anchor: TilePos, size: (i32, i32)) -> Vec2Fx {
    Vec2Fx::new(
        Fx::from_num(anchor.x) + Fx::from_num(size.0) / Fx::from_num(2),
        Fx::from_num(anchor.y) + Fx::from_num(size.1) / Fx::from_num(2),
    )
}

fn footprint_contains(anchor: TilePos, size: (i32, i32), tile: TilePos) -> bool {
    tile.x >= anchor.x
        && tile.x < anchor.x + size.0
        && tile.y >= anchor.y
        && tile.y < anchor.y + size.1
}

fn footprint_tiles(anchor: TilePos, size: (i32, i32)) -> Vec<TilePos> {
    (0..size.1)
        .flat_map(|dy| (0..size.0).map(move |dx| anchor.offset(dx, dy)))
        .collect()
}

fn sorted_tiles(tiles: impl IntoIterator<Item = TilePos>) -> Vec<TilePos> {
    let mut tiles: Vec<_> = tiles.into_iter().collect();
    tiles.sort_by_key(|tile| (tile.y, tile.x));
    tiles.dedup();
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::Orientation;

    use crate::command::{Command, PlayerCommand};
    use crate::ids::{BuildingId, PlayerId, Target, UnitId};
    use crate::scenario::{PlayerSpec, Scenario, UnitSpec};
    use chassis::Tick;

    const WIDTH: i32 = 40;
    const HEIGHT: i32 = 24;
    const LEFT_HOME: TilePos = TilePos::new(4, 10);
    const RIGHT_HOME: TilePos = TilePos::new(34, 10);

    macro_rules! scored_coverage {
        ($obs:expr, $briefing:expr, $assets:expr, $approaches:expr, $existing:expr, $planned:expr, $profile:expr, $candidate:expr $(,)?) => {
            score_coverage(
                &CoverageContext {
                    obs: $obs,
                    briefing: $briefing,
                    assets: $assets,
                    approaches: $approaches,
                    existing: $existing,
                    planned: $planned,
                },
                $profile,
                $candidate,
            )
        };
    }

    fn scenario_with(terrain: impl FnMut(TilePos) -> char) -> Scenario {
        scenario_with_starts(LEFT_HOME, RIGHT_HOME, terrain)
    }

    fn scenario_with_starts(
        first: TilePos,
        second: TilePos,
        mut terrain: impl FnMut(TilePos) -> char,
    ) -> Scenario {
        let mut rows = Vec::new();
        for y in 0..HEIGHT {
            let mut row = Vec::new();
            for x in 0..WIDTH {
                let tile = TilePos::new(x, y);
                let authored = if tile == first {
                    '1'
                } else if tile == second {
                    '2'
                } else {
                    terrain(tile)
                };
                row.push(authored as u8);
            }
            rows.push(String::from_utf8(row).expect("ASCII fixture row"));
        }
        Scenario {
            name: "defense fixture".into(),
            seed: 7,
            map: rows,
            players: vec![
                PlayerSpec {
                    name: "left".into(),
                    faction: crate::state::Faction::Ferrous,
                    team: None,
                    scrap: 10_000,
                    bot: true,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "right".into(),
                    faction: crate::state::Faction::Cupric,
                    team: None,
                    scrap: 10_000,
                    bot: true,
                    bot_config: None,
                },
            ],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        }
    }

    fn briefing() -> PublicMapBriefing {
        PublicMapBriefing::from_scenario(&scenario_with(|_| '.')).expect("briefing fixture")
    }

    fn barricade_lane(tile: TilePos) -> char {
        let bypass = tile.y == 9 && ((11..=13).contains(&tile.x) || (26..=28).contains(&tile.x));
        let main_lane = (10..=11).contains(&tile.y);
        let bottleneck = tile.y == 11 && matches!(tile.x, 12 | 27);
        if (main_lane || bypass) && !bottleneck {
            '.'
        } else {
            '^'
        }
    }

    fn vertical_barricade_lane(tile: TilePos) -> char {
        let bypass = tile.x == 18 && ((7..=9).contains(&tile.y) || (14..=16).contains(&tile.y));
        let main_lane = (19..=20).contains(&tile.x);
        let bottleneck = tile.x == 20 && matches!(tile.y, 8 | 15);
        if (main_lane || bypass) && !bottleneck {
            '.'
        } else {
            '^'
        }
    }

    fn full_turn_barricade_lane(tile: TilePos) -> char {
        let top = tile.y == 5 && (4..=20).contains(&tile.x);
        let middle = (19..=20).contains(&tile.x) && (5..=18).contains(&tile.y);
        let bottom = tile.y == 18 && (19..=35).contains(&tile.x);
        let bypass = (tile.y == 4 && (11..=13).contains(&tile.x))
            || (tile.y == 19 && (26..=28).contains(&tile.x));
        if top || middle || bottom || bypass {
            '.'
        } else {
            '^'
        }
    }

    fn building(id: u32, player: PlayerId, kind: BuildingKind, anchor: TilePos) -> BuildingObs {
        BuildingObs {
            id: BuildingId(id),
            player,
            kind,
            anchor,
            hp: kind.base_stats().max_hp,
            built: true,
            seen: true,
            tier: 0,
        }
    }

    fn remembered_building(building: &BuildingObs, last_seen: Tick) -> BuildingContact {
        BuildingContact {
            id: Some(building.id),
            player: building.player,
            kind: building.kind,
            anchor: building.anchor,
            hp: building.hp,
            built: building.built,
            tier: building.tier,
            last_seen: Some(last_seen),
            evidence: ContactEvidence::Remembered,
        }
    }

    fn unit(id: u32, player: PlayerId, kind: UnitKind, tile: TilePos) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player,
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

    fn observation(me: PlayerId, home: TilePos) -> Observation {
        let worker_tile = if me == PlayerId(0) {
            home.offset(3, 3)
        } else {
            TilePos::new(WIDTH - 1 - (LEFT_HOME.x + 3), home.y + 3)
        };
        let mut worker = unit(1, me, UnitKind::Harvester, worker_tile);
        worker.idle = true;
        Observation {
            tick: 1_000,
            me,
            scrap: 10_000,
            map_width: WIDTH,
            map_height: HEIGHT,
            my_units: vec![worker],
            my_buildings: vec![building(0, me, BuildingKind::Foundry, home)],
            my_queues: vec![Vec::new()],
            visible: vec![false; (WIDTH * HEIGHT) as usize],
            explored: vec![true; (WIDTH * HEIGHT) as usize],
            faction: if me == PlayerId(0) {
                crate::state::Faction::Ferrous
            } else {
                crate::state::Faction::Cupric
            },
            ..Observation::default()
        }
    }

    #[test]
    fn builder_travel_uses_the_authoritative_rotated_doorstep() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let origin = TilePos::new(2, 3);
        obs.my_units = [4, 20]
            .map(|id| unit(id, PlayerId(0), UnitKind::Harvester, origin))
            .to_vec();
        obs.enemy_units = [7, 12]
            .map(|id| unit(id, PlayerId(1), UnitKind::Sentinel, TilePos::new(10, 6)))
            .to_vec();
        obs.visible.fill(true);
        let grounding = DefenseGrounding::new(&UtilityPolicy::new(), &obs, &map);
        let placement = PlacementFootprint {
            anchor: TilePos::new(8, 3),
            size: BuildingKind::Bastion.base_stats().size,
            blocks_ground: true,
        };

        assert_eq!(
            grounding.builder_travel_cost(&obs.my_units[0], placement, None),
            Some(54),
            "rank zero must price the rotated first doorstep, not the globally shortest west tile"
        );
        assert_eq!(
            grounding.builder_travel_cost(&obs.my_units[1], placement, None),
            Some(50),
            "the next owner-local rank rotates the shortest west doorstep into first place"
        );
    }

    fn site(
        policy: &UtilityPolicy,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        units: &[UnitContact],
        buildings: &[BuildingContact],
    ) -> Option<TilePos> {
        site_for(
            BuildingKind::Turret,
            policy,
            obs,
            briefing,
            units,
            buildings,
        )
    }

    fn site_for(
        kind: BuildingKind,
        policy: &UtilityPolicy,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        units: &[UnitContact],
        buildings: &[BuildingContact],
    ) -> Option<TilePos> {
        let builders: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().harvest.is_some())
            .collect();
        policy.strategic_defense_site(kind, obs, briefing, units, buildings, &builders)
    }

    fn assert_existence_matches_ranked_search(
        kind: BuildingKind,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        units: &[UnitContact],
        buildings: &[BuildingContact],
    ) {
        let expected =
            site_for(kind, &UtilityPolicy::new(), obs, briefing, units, buildings).is_some();
        let policy = UtilityPolicy::new();
        let builders: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().harvest.is_some())
            .collect();
        let actual = policy.strategic_defense_site_exists_grounded(
            kind, obs, briefing, units, buildings, &builders, &mut None,
        );
        assert_eq!(
            actual, expected,
            "existence and ranked selection disagreed for {kind:?}"
        );
    }

    fn emergency_site_for(
        kind: BuildingKind,
        policy: &UtilityPolicy,
        obs: &Observation,
        briefing: &PublicMapBriefing,
        units: &[UnitContact],
        buildings: &[BuildingContact],
    ) -> Option<TilePos> {
        let builders: Vec<_> = obs
            .my_units
            .iter()
            .filter(|unit| unit.kind.stats().harvest.is_some())
            .collect();
        policy
            .emergency_defense_placement(kind, obs, briefing, units, buildings, &builders)
            .map(|placement| placement.anchor)
    }

    fn reveal(obs: &mut Observation, tile: TilePos) {
        let index = usize::try_from(tile.y * obs.map_width + tile.x)
            .expect("fixture tile has a valid visibility index");
        obs.visible[index] = true;
    }

    #[test]
    fn public_start_places_the_opening_turret_on_the_hostile_front() {
        let obs = observation(PlayerId(0), LEFT_HOME);
        let selected = site(&UtilityPolicy::new(), &obs, &briefing(), &[], &[])
            .expect("a legal exposed-front site");

        assert!(
            selected.x > LEFT_HOME.x + 1,
            "the east-side enemy prior should not put the turret behind home: {selected}"
        );
        assert!(
            selected.manhattan(RIGHT_HOME) < LEFT_HOME.manhattan(RIGHT_HOME),
            "the site should intercept before the hostile approach reaches the Foundry"
        );
    }

    #[test]
    fn strategic_quote_retains_the_ranked_site_builder_and_public_evidence() {
        let obs = observation(PlayerId(0), LEFT_HOME);
        let map = briefing();
        let builders: Vec<_> = obs.my_units.iter().collect();
        let policy = UtilityPolicy::new();
        let mut grounding = None;
        let quote = policy
            .strategic_defense_quote_grounded(
                BuildingKind::Turret,
                &obs,
                &map,
                &[],
                &[],
                &builders,
                &mut grounding,
            )
            .expect("the public approach has a useful Turret quote");
        let ranked = policy
            .strategic_defense_site_grounded(
                BuildingKind::Turret,
                &obs,
                &map,
                &[],
                &[],
                &builders,
                &mut grounding,
            )
            .expect("the legacy wrapper sees the same opportunity");

        assert_eq!(quote.placement.anchor, ranked);
        assert_eq!(quote.placement.builder, obs.my_units[0].id);
        assert_eq!(quote.evidence, DefenseOpportunityEvidence::PublicPrior);
        assert!(quote.uncovered_value > 0 || quote.reinforced_value > 0);
    }

    #[test]
    fn every_weapon_and_obstacle_role_can_produce_an_exact_quote() {
        let obs = observation(PlayerId(0), LEFT_HOME);
        let map = briefing();
        let builders: Vec<_> = obs.my_units.iter().collect();
        let policy = UtilityPolicy::new();
        let mut grounding = None;

        for kind in [
            BuildingKind::Turret,
            BuildingKind::Bastion,
            BuildingKind::FlakTurret,
            BuildingKind::ScuttleCharge,
        ] {
            let quote = policy.strategic_defense_quote_grounded(
                kind,
                &obs,
                &map,
                &[],
                &[],
                &builders,
                &mut grounding,
            );
            assert!(quote.is_some(), "{kind:?} lost its opportunity scorer");
        }

        let lane = scenario_with(barricade_lane);
        let lane_map = PublicMapBriefing::from_scenario(&lane).expect("lane briefing");
        let mut lane_obs = observation(PlayerId(0), LEFT_HOME);
        lane_obs.my_units[0].tile = LEFT_HOME.offset(3, 1);
        lane_obs.known_peaks = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| barricade_lane(*tile) == '^')
            .collect();
        lane_obs.known_rock = lane_obs.known_peaks.clone();
        let lane_builders: Vec<_> = lane_obs.my_units.iter().collect();
        assert!(
            policy
                .strategic_defense_quote_grounded(
                    BuildingKind::Barricade,
                    &lane_obs,
                    &lane_map,
                    &[],
                    &[],
                    &lane_builders,
                    &mut None,
                )
                .is_some(),
            "Barricade lost its lane-shaping opportunity scorer"
        );
    }

    #[test]
    fn one_think_context_reuses_exact_projection_and_route_answers() {
        let obs = observation(PlayerId(0), LEFT_HOME);
        let map = briefing();
        let builders: Vec<_> = obs.my_units.iter().collect();
        let policy = UtilityPolicy::new();
        let mut context = DefenseThinkContext::new(&policy, &obs, &map, &[], &[]);

        for kind in [
            BuildingKind::Turret,
            BuildingKind::Bastion,
            BuildingKind::FlakTurret,
            BuildingKind::ScuttleCharge,
        ] {
            let uncached = policy.strategic_defense_quote_grounded(
                kind,
                &obs,
                &map,
                &[],
                &[],
                &builders,
                &mut None,
            );
            let first = policy.strategic_defense_quote_in_context(kind, &builders, &mut context);
            let repeated = policy.strategic_defense_quote_in_context(kind, &builders, &mut context);
            assert_eq!(first, uncached, "{kind:?} changed under the think cache");
            assert_eq!(repeated, first, "{kind:?} cache lookup changed its quote");
        }

        let uncached_array =
            policy.strategic_array_quote(&obs, &map, LEFT_HOME, &[], &[], &builders);
        let cached_array =
            policy.strategic_array_quote_in_context(LEFT_HOME, &builders, &mut context);
        assert_eq!(
            cached_array, uncached_array,
            "Array changed under shared grounding"
        );

        let stats = context.cache_stats();
        assert_eq!(stats.ground_projection_builds, 1);
        assert_eq!(stats.air_projection_builds, 1);
        assert!(stats.builder_safety_builds > 0);
        assert!(stats.builder_safety_hits > 0);
        assert!(stats.builder_travel_hits > 0);
        assert!(stats.resource_access_hits > 0);
        assert!(stats.supported_assets_hits > 0);
        assert!(stats.rerouted_approach_hits > 0);
        assert_eq!(policy.harvest_danger_build_count(), 1);
    }

    #[test]
    fn trailing_stealthy_build_cannot_hide_an_earlier_producer_seal() {
        let obs = observation(PlayerId(0), LEFT_HOME);
        let map = briefing();
        let mut builds: Vec<_> =
            crate::tick::rect_adjacent_tiles(LEFT_HOME, BuildingKind::Foundry.base_stats().size)
                .map(|anchor| (BuildingKind::Barricade, anchor))
                .collect();
        builds.push((BuildingKind::ScuttleCharge, TilePos::new(20, 4)));

        assert!(
            !UtilityPolicy::new().combined_build_layout_is_safe(&obs, &map, &builds),
            "the closing blocking footprints must be checked even when a mine sorts last"
        );
    }

    #[test]
    fn combined_layout_preserves_a_proposed_foundry_egress() {
        let foundry_anchor = TilePos::new(10, 10);
        let exit = TilePos::new(12, 10);
        let terrain = |tile: TilePos| {
            let inside_foundry = (10..=11).contains(&tile.x) && (10..=11).contains(&tile.y);
            if inside_foundry || tile.y == 10 && tile.x >= exit.x {
                '.'
            } else {
                '^'
            }
        };
        let scenario = scenario_with(terrain);
        let map = PublicMapBriefing::from_scenario(&scenario).expect("single-exit briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.known_peaks = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| *tile != LEFT_HOME && *tile != RIGHT_HOME && terrain(*tile) == '^')
            .collect();
        obs.known_rock = obs.known_peaks.clone();
        let policy = UtilityPolicy::new();
        let foundry = (BuildingKind::Foundry, foundry_anchor);

        assert!(
            policy.combined_build_layout_is_safe(&obs, &map, &[foundry]),
            "the proposed Foundry can spawn into its authored outside component"
        );
        assert!(
            !policy.combined_build_layout_is_safe(
                &obs,
                &map,
                &[foundry, (BuildingKind::Turret, exit)],
            ),
            "the paired defense cannot close the proposed producer's only doorstep"
        );
    }

    #[test]
    fn combined_layout_preserves_an_unfinished_foundry_future_egress() {
        let foundry_anchor = TilePos::new(10, 10);
        let exit = TilePos::new(12, 10);
        let safe_site = TilePos::new(25, 5);
        let terrain = |tile: TilePos| {
            let inside_foundry = footprint_contains(
                foundry_anchor,
                BuildingKind::Foundry.base_stats().size,
                tile,
            );
            if inside_foundry || tile.y == 10 && tile.x >= exit.x || tile.chebyshev(safe_site) <= 1
            {
                '.'
            } else {
                '^'
            }
        };
        let scenario = scenario_with(terrain);
        let map = PublicMapBriefing::from_scenario(&scenario).expect("future-egress briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.known_peaks = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| *tile != LEFT_HOME && *tile != RIGHT_HOME && terrain(*tile) == '^')
            .collect();
        obs.known_rock = obs.known_peaks.clone();
        let mut unfinished = building(7, PlayerId(0), BuildingKind::Foundry, foundry_anchor);
        unfinished.built = false;
        obs.my_buildings.push(unfinished);
        obs.my_queues.push(Vec::new());
        let policy = UtilityPolicy::new();

        assert!(policy.combined_build_layout_is_safe(
            &obs,
            &map,
            &[(BuildingKind::Turret, safe_site)],
        ));
        assert!(
            !policy.combined_build_layout_is_safe(&obs, &map, &[(BuildingKind::Turret, exit)],),
            "a paid unfinished producer must retain the doorstep it will need on completion"
        );
    }

    #[test]
    fn combined_layout_preserves_a_deferred_foundry_future_egress() {
        let foundry_anchor = TilePos::new(10, 10);
        let exit = TilePos::new(12, 10);
        let safe_site = TilePos::new(25, 5);
        let terrain = |tile: TilePos| {
            let inside_foundry = footprint_contains(
                foundry_anchor,
                BuildingKind::Foundry.base_stats().size,
                tile,
            );
            if inside_foundry || tile.y == 10 && tile.x >= exit.x || tile.chebyshev(safe_site) <= 1
            {
                '.'
            } else {
                '^'
            }
        };
        let scenario = scenario_with(terrain);
        let map = PublicMapBriefing::from_scenario(&scenario).expect("future-egress briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.known_peaks = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| *tile != LEFT_HOME && *tile != RIGHT_HOME && terrain(*tile) == '^')
            .collect();
        obs.known_rock = obs.known_peaks.clone();
        obs.my_units[0].founding = Some((BuildingKind::Foundry, foundry_anchor));
        let policy = UtilityPolicy::new();

        assert!(policy.combined_build_layout_is_safe(
            &obs,
            &map,
            &[(BuildingKind::Turret, safe_site)],
        ));
        assert!(
            !policy.combined_build_layout_is_safe(&obs, &map, &[(BuildingKind::Turret, exit)],),
            "a deferred producer must retain the doorstep it will need once its site exists"
        );
    }

    #[test]
    fn single_defense_candidates_preserve_a_deferred_foundry_future_egress() {
        let foundry_anchor = TilePos::new(10, 10);
        let exit = TilePos::new(12, 10);
        let safe_site = TilePos::new(25, 5);
        let terrain = |tile: TilePos| {
            let inside_foundry = footprint_contains(
                foundry_anchor,
                BuildingKind::Foundry.base_stats().size,
                tile,
            );
            if inside_foundry || tile.y == 10 && tile.x >= exit.x || tile.chebyshev(safe_site) <= 1
            {
                '.'
            } else {
                '^'
            }
        };
        let scenario = scenario_with(terrain);
        let map = PublicMapBriefing::from_scenario(&scenario).expect("future-egress briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.known_peaks = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| *tile != LEFT_HOME && *tile != RIGHT_HOME && terrain(*tile) == '^')
            .collect();
        obs.known_rock = obs.known_peaks.clone();
        obs.my_units[0].founding = Some((BuildingKind::Foundry, foundry_anchor));
        let orientation = Orientation::for_home(&obs, LEFT_HOME);
        let mut context = DefenseThinkContext::new_oriented(
            &UtilityPolicy::new(),
            &obs,
            &map,
            &[],
            &[],
            orientation,
        );

        assert!(context.future_ground_producer_egress_survives(BuildingKind::Turret, safe_site,));
        assert!(!context.future_ground_producer_egress_survives(BuildingKind::Turret, exit,));
        assert!(!context.future_ground_producer_egress_survives(BuildingKind::Array, exit,));
        let stats = context.cache_stats();
        assert_eq!(stats.future_producer_baseline_builds, 1);
        assert_eq!(stats.future_producer_egress_builds, 2);
        assert_eq!(stats.future_producer_egress_hits, 1);
    }

    #[test]
    fn strategic_quote_skips_a_deferred_producers_only_exit() {
        let map = briefing();
        let producer_anchor = TilePos::new(11, 6);
        let exit = TilePos::new(10, 6);
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.my_units[0].tile = TilePos::new(6, 6);
        obs.my_units[0].founding = Some((BuildingKind::Foundry, producer_anchor));
        obs.ally_buildings.extend(
            crate::tick::rect_adjacent_tiles(
                producer_anchor,
                BuildingKind::Foundry.base_stats().size,
            )
            .filter(|tile| *tile != exit)
            .enumerate()
            .map(|(index, tile)| {
                building(
                    100 + index as u32,
                    PlayerId(1),
                    BuildingKind::Barricade,
                    tile,
                )
            }),
        );
        let policy = UtilityPolicy::new();
        let builders = vec![&obs.my_units[0]];
        let mut unguarded = DefenseThinkContext::new(&policy, &obs, &map, &[], &[]);
        let otherwise_best = policy
            .strategic_defense_quote_in_context(BuildingKind::Turret, &builders, &mut unguarded)
            .expect("the public approach has an otherwise-useful Turret site");
        assert_eq!(otherwise_best.placement.anchor, exit);

        let mut guarded = DefenseThinkContext::new_oriented(
            &policy,
            &obs,
            &map,
            &[],
            &[],
            Orientation::for_home(&obs, LEFT_HOME),
        );
        let safe = policy
            .strategic_defense_quote_in_context(BuildingKind::Turret, &builders, &mut guarded)
            .expect("a safe alternative Turret site remains useful");
        assert_ne!(safe.placement.anchor, exit);
        assert!(
            guarded.future_ground_producer_egress_survives(
                BuildingKind::Turret,
                safe.placement.anchor,
            )
        );
    }

    fn split_component_planned_foundry_fixture() -> (Observation, PublicMapBriefing) {
        let foundry_anchor = TilePos::new(18, 18);
        let terrain = |tile: TilePos| {
            let inside_foundry = footprint_contains(
                foundry_anchor,
                BuildingKind::Foundry.base_stats().size,
                tile,
            );
            let upper_component = tile.x == 17 && tile.y <= 17;
            let lower_component = tile.x == 17 && tile.y >= 20;
            if inside_foundry || upper_component || lower_component {
                '.'
            } else {
                '^'
            }
        };
        let scenario = scenario_with(terrain);
        let map = PublicMapBriefing::from_scenario(&scenario).expect("split-component briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.known_peaks = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| *tile != LEFT_HOME && *tile != RIGHT_HOME && terrain(*tile) == '^')
            .collect();
        obs.known_rock = obs.known_peaks.clone();
        (obs, map)
    }

    #[test]
    fn combined_layout_rejects_a_cut_off_authoritative_planned_spawn() {
        let (obs, map) = split_component_planned_foundry_fixture();
        let policy = UtilityPolicy::new();
        let foundry = (BuildingKind::Foundry, TilePos::new(18, 18));
        let lower_blocker = (BuildingKind::Turret, TilePos::new(17, 21));

        assert!(policy.combined_build_layout_is_safe(&obs, &map, &[foundry]));
        assert!(policy.combined_build_layout_is_safe(&obs, &map, &[lower_blocker]));
        assert!(
            !policy.combined_build_layout_is_safe(&obs, &map, &[foundry, lower_blocker]),
            "the radial doorstep is in the lower component even though row-major visits the upper component first"
        );
    }

    #[test]
    fn combined_layout_keeps_a_safe_authoritative_planned_spawn() {
        let (obs, map) = split_component_planned_foundry_fixture();
        let policy = UtilityPolicy::new();
        let foundry = (BuildingKind::Foundry, TilePos::new(18, 18));
        let upper_blocker = (BuildingKind::Turret, TilePos::new(17, 16));

        assert!(policy.combined_build_layout_is_safe(&obs, &map, &[foundry]));
        assert!(policy.combined_build_layout_is_safe(&obs, &map, &[upper_blocker]));
        assert!(
            policy.combined_build_layout_is_safe(&obs, &map, &[foundry, upper_blocker]),
            "cutting the row-major component must not reject a layout whose radial doorstep remains connected"
        );
    }

    #[test]
    fn planned_producer_doorstep_tie_breaks_in_the_authoritative_frame() {
        let obs = observation(PlayerId(0), LEFT_HOME);
        let map = briefing();
        let ground = GroundKnowledge::new(&obs, &map, &[]);
        let footprint = PlacementFootprint {
            anchor: TilePos::new(8, 11),
            size: BuildingKind::Foundry.base_stats().size,
            blocks_ground: true,
        };
        let orientation = Orientation::for_home(&obs, TilePos::new(WIDTH - 1, 0));

        assert_eq!(
            planned_ground_producer_spawn_doorstep(
                &ground,
                footprint,
                Some(footprint),
                Some(orientation),
            ),
            Some(TilePos::new(7, 10)),
            "the mirrored policy must retain the world's top-side cross-product tie-break"
        );
        assert_eq!(
            planned_ground_producer_spawn_doorstep(&ground, footprint, Some(footprint), None),
            Some(TilePos::new(7, 13)),
            "running the same key in policy coordinates would choose the opposite tied doorstep"
        );
    }

    #[test]
    fn combined_layout_revalidates_the_exact_builder_against_remembered_danger() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.visible.fill(true);
        obs.my_units[0].tile = TilePos::new(8, 10);
        let build = (BuildingKind::Turret, TilePos::new(25, 10), UnitId(1));
        let policy = UtilityPolicy::new();
        let orientation = Orientation::for_home(&obs, LEFT_HOME);
        assert!(policy.combined_build_layout_with_builders_is_safe(
            &obs,
            &map,
            &[],
            &[],
            orientation,
            &[build],
        ));

        let remembered = UnitContact {
            id: UnitId(20),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: TilePos::new(16, 10),
            hp: UnitKind::Sentinel.stats().max_hp,
            grounded: false,
            last_seen: obs.tick,
            evidence: ContactEvidence::Remembered,
        };
        assert!(
            !policy.combined_build_layout_with_builders_is_safe(
                &obs,
                &map,
                std::slice::from_ref(&remembered),
                &[],
                orientation,
                &[build],
            ),
            "a frozen builder whose exact command route enters remembered danger is unsafe"
        );
    }

    #[test]
    fn existence_search_matches_ranked_selection_across_strategic_boundaries() {
        let map = briefing();
        let supported = [
            BuildingKind::Turret,
            BuildingKind::Bastion,
            BuildingKind::FlakTurret,
            BuildingKind::ScuttleCharge,
            BuildingKind::Barricade,
        ];
        let baseline = observation(PlayerId(0), LEFT_HOME);
        for kind in supported {
            assert_existence_matches_ranked_search(kind, &baseline, &map, &[], &[]);
        }
        assert_existence_matches_ranked_search(BuildingKind::Foundry, &baseline, &map, &[], &[]);

        let mut no_builder = baseline.clone();
        no_builder.my_units.clear();
        assert_existence_matches_ranked_search(BuildingKind::Turret, &no_builder, &map, &[], &[]);

        let mut no_asset = baseline.clone();
        no_asset.my_buildings.clear();
        assert_existence_matches_ranked_search(BuildingKind::Turret, &no_asset, &map, &[], &[]);

        let hostile = building(20, PlayerId(1), BuildingKind::Turret, TilePos::new(4, 5));
        let mut current_threat = baseline.clone();
        current_threat.enemy_buildings.push(hostile.clone());
        for kind in supported {
            assert_existence_matches_ranked_search(kind, &current_threat, &map, &[], &[]);
        }

        let remembered = remembered_building(&hostile, baseline.tick);
        for kind in supported {
            assert_existence_matches_ranked_search(
                kind,
                &baseline,
                &map,
                &[],
                std::slice::from_ref(&remembered),
            );
        }

        let divided_scenario = scenario_with(|tile| if tile.x == 20 { '#' } else { '.' });
        let divided_map =
            PublicMapBriefing::from_scenario(&divided_scenario).expect("divided briefing");
        for kind in supported {
            assert_existence_matches_ranked_search(kind, &baseline, &divided_map, &[], &[]);
        }
    }

    #[test]
    fn bastion_uses_its_long_gun_to_cover_the_hostile_front() {
        let map = briefing();
        let obs = observation(PlayerId(0), LEFT_HOME);
        let selected = site_for(
            BuildingKind::Bastion,
            &UtilityPolicy::new(),
            &obs,
            &map,
            &[],
            &[],
        )
        .expect("a legal Bastion site");
        let profile = DefenseProfile::for_kind(BuildingKind::Bastion).expect("Bastion profile");

        assert!(
            selected.x > LEFT_HOME.x + 1,
            "the southeast hostile prior must not strand the long gun behind home: {selected}",
        );
        let starts = UtilityPolicy::new().uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let assets = defended_assets(&UtilityPolicy::new(), &obs, &ground);
        let approaches = approaches(
            &ground,
            &threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Ground)[3],
            &assets,
            Some(profile.footprint(selected)),
            DefenseDomain::Ground,
        );
        assert!(approaches.iter().any(|approach| {
            approach
                .path
                .iter()
                .copied()
                .any(|tile| defense_covers(&map, profile, selected, tile))
        }));
    }

    #[test]
    fn flak_tracks_air_approaches_without_treating_ground_contacts_as_air() {
        let map = briefing();
        let mut air = observation(PlayerId(0), LEFT_HOME);
        air.enemy_units
            .push(unit(20, PlayerId(1), UnitKind::Condor, TilePos::new(4, 1)));
        let selected = site_for(
            BuildingKind::FlakTurret,
            &UtilityPolicy::new(),
            &air,
            &map,
            &[],
            &[],
        )
        .expect("the northern bomber approach admits Flak");
        assert!(
            selected.y < LEFT_HOME.y,
            "current airborne danger must outrank the eastern public start: {selected}",
        );

        air.enemy_units.clear();
        air.enemy_units.push(unit(
            21,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(4, 1),
        ));
        let ground_ignored = site_for(
            BuildingKind::FlakTurret,
            &UtilityPolicy::new(),
            &air,
            &map,
            &[],
            &[],
        )
        .expect("the public air approach remains available after prior air evidence");
        assert!(
            ground_ignored.x > LEFT_HOME.x + 1,
            "a ground-only contact must not pull anti-air off the expected air lane",
        );
    }

    #[test]
    fn current_visible_threats_admit_matching_emergency_defenses() {
        let map = briefing();
        let policy = UtilityPolicy::new();

        let ground_threat = TilePos::new(4, 2);
        let mut ground = observation(PlayerId(0), LEFT_HOME);
        ground
            .enemy_units
            .push(unit(20, PlayerId(1), UnitKind::Sentinel, ground_threat));
        reveal(&mut ground, ground_threat);
        let ground_site =
            emergency_site_for(BuildingKind::Turret, &policy, &ground, &map, &[], &[])
                .expect("a currently visible ground attacker near home admits an emergency Turret");
        assert!(policy.current_emergency_defense_claim_is_useful(
            BuildingKind::Turret,
            ground_site,
            &ground,
            &map
        ));
        let mut already_defended = ground.clone();
        already_defended.my_buildings.push(building(
            30,
            PlayerId(0),
            BuildingKind::Turret,
            ground_site.offset(-3, 0),
        ));
        already_defended.my_queues.push(Vec::new());
        assert!(
            !policy.current_emergency_defense_claim_is_useful(
                BuildingKind::Turret,
                ground_site,
                &already_defended,
                &map,
            ),
            "a projected Turret already consumes the one opening emergency exception"
        );
        assert!(
            !policy.current_emergency_defense_claim_is_useful(
                BuildingKind::Turret,
                TilePos::new(4, 19),
                &ground,
                &map,
            ),
            "a safe rear Turret claim cannot consume a northern emergency exception"
        );

        let air_threat = LEFT_HOME.offset(6, -4);
        for (id, kind) in [(21, UnitKind::Condor), (22, UnitKind::Moth)] {
            let mut air = observation(PlayerId(0), LEFT_HOME);
            let mut landed_bomber = unit(id, PlayerId(1), kind, air_threat);
            landed_bomber.grounded = true;
            air.enemy_units.push(landed_bomber);
            reveal(&mut air, air_threat);
            assert!(
                emergency_site_for(BuildingKind::FlakTurret, &policy, &air, &map, &[], &[],)
                    .is_some(),
                "a currently visible parked {kind:?} near home should admit emergency Flak"
            );
            assert!(policy.current_emergency_defense_required(
                BuildingKind::FlakTurret,
                &air,
                &map
            ));
            assert!(
                emergency_site_for(BuildingKind::Turret, &policy, &air, &map, &[], &[]).is_none(),
                "a parked {kind:?} remains an air threat rather than masquerading as a ground body"
            );
        }
        assert!(
            emergency_site_for(BuildingKind::FlakTurret, &policy, &ground, &map, &[], &[])
                .is_none(),
            "a ground attacker cannot unlock the anti-air opening exception"
        );

        for (id, kind) in [(23, UnitKind::Talon), (24, UnitKind::Wisp)] {
            let mut air_superiority = observation(PlayerId(0), LEFT_HOME);
            air_superiority
                .enemy_units
                .push(unit(id, PlayerId(1), kind, air_threat));
            reveal(&mut air_superiority, air_threat);
            assert!(
                emergency_site_for(
                    BuildingKind::FlakTurret,
                    &policy,
                    &air_superiority,
                    &map,
                    &[],
                    &[],
                )
                .is_none(),
                "a currently visible pure air-to-air {kind:?} cannot unlock emergency Flak"
            );
            assert!(!policy.current_emergency_defense_required(
                BuildingKind::FlakTurret,
                &air_superiority,
                &map,
            ));
        }
    }

    #[test]
    fn an_unpaid_defense_claim_cannot_mask_the_current_emergency_that_justifies_it() {
        let map = PublicMapBriefing::from_scenario(&scenario_with(|tile| {
            if tile.y == LEFT_HOME.y { '.' } else { '^' }
        }))
        .expect("the one-lane briefing builds");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let claim = TilePos::new(8, LEFT_HOME.y);
        obs.my_units[0].tile = TilePos::new(6, LEFT_HOME.y);
        obs.my_units[0].founding = Some((BuildingKind::Turret, claim));
        let threat = TilePos::new(11, LEFT_HOME.y);
        obs.enemy_units
            .push(unit(20, PlayerId(1), UnitKind::Sentinel, threat));
        reveal(&mut obs, threat);

        assert!(UtilityPolicy::new().current_emergency_defense_required(
            BuildingKind::Turret,
            &obs,
            &map
        ));
    }

    #[test]
    fn noncurrent_evidence_cannot_admit_an_emergency_defense() {
        let map = briefing();
        let policy = UtilityPolicy::new();
        let dark = observation(PlayerId(0), LEFT_HOME);
        for kind in [BuildingKind::Turret, BuildingKind::FlakTurret] {
            assert_eq!(
                emergency_site_for(kind, &policy, &dark, &map, &[], &[]),
                None,
                "the public hostile start alone must not justify an emergency {kind:?}"
            );
            assert!(!policy.current_emergency_defense_required(kind, &dark, &map));
        }

        let mut blip_only = dark.clone();
        blip_only.blips.push(LEFT_HOME.offset(6, 0));
        for kind in [BuildingKind::Turret, BuildingKind::FlakTurret] {
            assert_eq!(
                emergency_site_for(kind, &policy, &blip_only, &map, &[], &[]),
                None,
                "an anonymous radar blip alone must not justify an emergency {kind:?}"
            );
            assert!(!policy.current_emergency_defense_required(kind, &blip_only, &map));
        }

        let remembered_ground = UnitContact {
            id: UnitId(20),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: LEFT_HOME.offset(7, 0),
            hp: UnitKind::Sentinel.stats().max_hp,
            grounded: false,
            last_seen: dark.tick,
            evidence: ContactEvidence::Remembered,
        };
        let remembered_turret = remembered_building(
            &building(
                21,
                PlayerId(1),
                BuildingKind::Turret,
                LEFT_HOME.offset(7, 0),
            ),
            dark.tick,
        );
        assert_eq!(
            emergency_site_for(
                BuildingKind::Turret,
                &policy,
                &dark,
                &map,
                std::slice::from_ref(&remembered_ground),
                std::slice::from_ref(&remembered_turret),
            ),
            None,
            "remembered ground threats must not justify an emergency Turret"
        );
        let mut ghost_only = dark.clone();
        let mut ghost = building(
            21,
            PlayerId(1),
            BuildingKind::Turret,
            LEFT_HOME.offset(7, 0),
        );
        ghost.seen = false;
        ghost_only.enemy_buildings.push(ghost);
        assert_eq!(
            emergency_site_for(BuildingKind::Turret, &policy, &ghost_only, &map, &[], &[],),
            None,
            "a remembered building ghost in the observation must not justify an emergency Turret"
        );
        assert!(!policy.current_emergency_defense_required(
            BuildingKind::Turret,
            &ghost_only,
            &map,
        ));

        let remembered_air = UnitContact {
            id: UnitId(22),
            player: PlayerId(1),
            kind: UnitKind::Condor,
            tile: LEFT_HOME.offset(6, -4),
            hp: UnitKind::Condor.stats().max_hp,
            grounded: false,
            last_seen: dark.tick,
            evidence: ContactEvidence::Remembered,
        };
        assert_eq!(
            emergency_site_for(
                BuildingKind::FlakTurret,
                &policy,
                &dark,
                &map,
                std::slice::from_ref(&remembered_air),
                &[],
            ),
            None,
            "remembered aircraft must not justify emergency Flak"
        );
    }

    #[test]
    fn mines_trigger_on_the_approach_while_barricades_reroute_it() {
        let map = briefing();
        let obs = observation(PlayerId(0), LEFT_HOME);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let assets = defended_assets(&policy, &obs, &ground);
        let origins =
            threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Ground)[3].clone();
        let baseline = approaches(&ground, &origins, &assets, None, DefenseDomain::Ground);

        let mine = site_for(BuildingKind::ScuttleCharge, &policy, &obs, &map, &[], &[])
            .expect("a public ground lane admits a buried charge");
        assert!(
            baseline
                .iter()
                .any(|approach| approach.path.contains(&mine)),
            "a trigger-radius defense belongs under hostile treads, not merely near the lane",
        );
        let mine_profile =
            DefenseProfile::for_kind(BuildingKind::ScuttleCharge).expect("mine profile");
        let mine_approaches = approaches_with_candidate(
            &ground,
            &assets,
            &baseline,
            mine_profile.footprint(mine),
            DefenseDomain::Ground,
            None,
        )
        .expect("a nonblocking charge preserves every approach");
        assert_eq!(
            mine_approaches, baseline,
            "a buried charge must not reroute the ground path it needs enemies to walk",
        );

        let corridor_scenario = scenario_with(barricade_lane);
        let corridor_map =
            PublicMapBriefing::from_scenario(&corridor_scenario).expect("broad lane briefing");
        let mut corridor_obs = observation(PlayerId(0), LEFT_HOME);
        corridor_obs.my_units[0].tile = LEFT_HOME.offset(3, 1);
        corridor_obs.known_rock = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| barricade_lane(*tile) == '^')
            .collect();
        corridor_obs.known_peaks = corridor_obs.known_rock.clone();
        let corridor_starts = policy.uncleared_hostile_starts(&corridor_map, corridor_obs.me);
        let corridor_ground = GroundKnowledge::new(&corridor_obs, &corridor_map, &corridor_starts);
        let corridor_assets = defended_assets(&policy, &corridor_obs, &corridor_ground);
        let corridor_baseline = approaches(
            &corridor_ground,
            &threat_origin_tiers(
                &corridor_obs,
                &[],
                &[],
                &corridor_starts,
                DefenseDomain::Ground,
            )[3],
            &corridor_assets,
            None,
            DefenseDomain::Ground,
        );
        let barricade = site_for(
            BuildingKind::Barricade,
            &policy,
            &corridor_obs,
            &corridor_map,
            &[],
            &[],
        )
        .expect("a bounded approach admits a delaying barricade");
        let barricade_profile =
            DefenseProfile::for_kind(BuildingKind::Barricade).expect("Barricade profile");
        let delayed = approaches_with_candidate(
            &corridor_ground,
            &corridor_assets,
            &corridor_baseline,
            barricade_profile.footprint(barricade),
            DefenseDomain::Ground,
            Some(MAX_BARRICADE_DETOUR_COST),
        )
        .expect("the selected Barricade keeps every approach viable");
        let detours: Vec<_> = delayed
            .iter()
            .filter(|approach| approach.disrupted)
            .map(|approach| path_cost(&approach.path).saturating_sub(approach.baseline_cost))
            .collect();
        assert!(detours.iter().any(|detour| *detour > 0));
        assert!(
            detours
                .iter()
                .all(|detour| *detour <= MAX_BARRICADE_DETOUR_COST)
        );
    }

    #[test]
    fn barricades_reject_long_ratchets_even_when_a_route_still_exists() {
        let scenario = scenario_with(|tile| {
            let main = tile.y == 10;
            let bypass = tile.y == 2 && (10..=30).contains(&tile.x);
            let connector = matches!(tile.x, 10 | 30) && (2..=10).contains(&tile.y);
            if main || bypass || connector {
                '.'
            } else {
                '^'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("long bypass briefing");
        let obs = observation(PlayerId(0), LEFT_HOME);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let assets = defended_assets(&policy, &obs, &ground);
        let origins =
            threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Ground)[3].clone();
        let baseline = approaches(&ground, &origins, &assets, None, DefenseDomain::Ground);
        let anchor = TilePos::new(20, 10);
        let barricade = DefenseProfile::for_kind(BuildingKind::Barricade)
            .expect("Barricade profile")
            .footprint(anchor);

        assert!(
            baseline
                .iter()
                .any(|approach| approach.path.contains(&anchor))
        );
        assert!(
            approaches_with_candidate(
                &ground,
                &assets,
                &baseline,
                barricade,
                DefenseDomain::Ground,
                Some(MAX_BARRICADE_DETOUR_COST),
            )
            .is_none(),
            "a technically reachable but strategically extreme approach detour is not a valid wall site"
        );

        let work = TilePos::new(32, 10);
        let (_, _, access_path) = shortest_path_between(
            &ground,
            &building_doorsteps(&ground, LEFT_HOME, BuildingKind::Foundry.base_stats().size),
            &[work],
            None,
            DefenseDomain::Ground,
        )
        .expect("the unblocked income road is direct");
        let resource = DefendedAsset {
            value: 1,
            shape: AssetShape::Scrap {
                tiles: vec![work],
                work_tiles: vec![work],
            },
            access: Some(AccessRoute {
                foundry: LEFT_HOME,
                work_tiles: vec![work],
                path: access_path,
            }),
        };
        assert!(scrap_access_survives(
            &ground,
            std::slice::from_ref(&resource),
            barricade,
            None,
        ));
        assert!(
            !scrap_access_survives(
                &ground,
                std::slice::from_ref(&resource),
                barricade,
                Some(MAX_BARRICADE_RESOURCE_DETOUR_COST),
            ),
            "a wall cannot make every Harvester take the long bypass forever"
        );
    }

    #[test]
    fn current_then_remembered_threats_override_the_public_start_direction() {
        let map = briefing();
        let mut current = observation(PlayerId(0), LEFT_HOME);
        current.enemy_units.push(unit(
            20,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(4, 0),
        ));
        let current_site = site(&UtilityPolicy::new(), &current, &map, &[], &[])
            .expect("current threat admits a site");
        assert!(current_site.y < LEFT_HOME.y);

        let remembered = UnitContact {
            id: UnitId(20),
            player: PlayerId(1),
            kind: UnitKind::Sentinel,
            tile: TilePos::new(4, 0),
            hp: UnitKind::Sentinel.stats().max_hp,
            grounded: false,
            last_seen: current.tick,
            evidence: ContactEvidence::Remembered,
        };
        let dark = observation(PlayerId(0), LEFT_HOME);
        let remembered_site = site(&UtilityPolicy::new(), &dark, &map, &[remembered], &[])
            .expect("remembered threat admits a site");
        assert!(remembered_site.y < LEFT_HOME.y);

        let public_site =
            site(&UtilityPolicy::new(), &dark, &map, &[], &[]).expect("public start admits a site");
        assert!(public_site.x > LEFT_HOME.x + 1);
    }

    #[test]
    fn threat_tiers_prefer_current_units_then_footholds_then_memory_then_public_starts() {
        let map = briefing();
        let north = TilePos::new(4, 0);
        let south = TilePos::new(4, 20);
        let west = TilePos::new(0, 10);
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.tick = 4_000;
        obs.enemy_units
            .push(unit(20, PlayerId(1), UnitKind::Sentinel, north));
        obs.enemy_buildings
            .push(building(21, PlayerId(1), BuildingKind::Fabricator, south));
        let remembered = BuildingContact {
            id: Some(BuildingId(22)),
            player: PlayerId(1),
            kind: BuildingKind::Fabricator,
            anchor: west,
            hp: BuildingKind::Fabricator.base_stats().max_hp,
            built: true,
            tier: 0,
            last_seen: Some(obs.tick),
            evidence: ContactEvidence::Remembered,
        };
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);

        let tiers = threat_origin_tiers(
            &obs,
            &[],
            std::slice::from_ref(&remembered),
            &starts,
            DefenseDomain::Ground,
        );
        assert_eq!(
            tiers.map(|tier| tier.iter().map(|origin| origin.anchor).collect::<Vec<_>>()),
            [vec![north], vec![south], vec![west], vec![RIGHT_HOME]]
        );
        let current_unit_site = site(&policy, &obs, &map, &[], std::slice::from_ref(&remembered))
            .expect("the current mobile threat admits a site");
        assert!(current_unit_site.y < LEFT_HOME.y);

        obs.enemy_units.clear();
        let current_foothold_site =
            site(&policy, &obs, &map, &[], std::slice::from_ref(&remembered))
                .expect("the current foothold admits a site");
        assert!(current_foothold_site.y > LEFT_HOME.y + 1);

        obs.enemy_buildings.clear();
        let remembered_site = site(&policy, &obs, &map, &[], std::slice::from_ref(&remembered))
            .expect("the remembered foothold admits a site");

        let expired = BuildingContact {
            last_seen: Some(0),
            ..remembered
        };
        assert_eq!(expired.confidence_at(obs.tick), 0);
        assert!(
            threat_origin_tiers(
                &obs,
                &[],
                std::slice::from_ref(&expired),
                &starts,
                DefenseDomain::Ground,
            )[2]
            .is_empty()
        );
        let public_site = site(&policy, &obs, &map, &[], std::slice::from_ref(&expired))
            .expect("the public approach remains after stale memory expires");
        assert_ne!(
            remembered_site, public_site,
            "actionable remembered foothold evidence must materially change the chosen defense"
        );
        assert!(public_site.x > LEFT_HOME.x + 1);
    }

    #[test]
    fn visible_and_remembered_forward_turrets_enable_safe_counterbattery_sites() {
        let map = briefing();
        let hostile = building(20, PlayerId(1), BuildingKind::Turret, TilePos::new(4, 5));
        let mut visible = observation(PlayerId(0), LEFT_HOME);
        visible.enemy_buildings.push(hostile.clone());
        assert_eq!(
            site(&UtilityPolicy::new(), &visible, &map, &[], &[]),
            None,
            "an equal-range Turret cannot ask a Harvester to build inside hostile fire"
        );
        let current_site = site_for(
            BuildingKind::Bastion,
            &UtilityPolicy::new(),
            &visible,
            &map,
            &[],
            &[],
        )
        .expect("a visible forward Turret shelling home admits safe counterbattery");
        let bastion = DefenseProfile::for_kind(BuildingKind::Bastion).expect("Bastion profile");
        assert!(
            candidate_covers(&visible, &map, bastion, current_site, hostile.anchor),
            "the selected Bastion must actually cover the hostile Turret: {current_site}"
        );

        let mut right = observation(PlayerId(1), RIGHT_HOME);
        let orientation = Orientation::for_home(&right, RIGHT_HOME);
        right.enemy_buildings.push(building(
            20,
            PlayerId(0),
            BuildingKind::Turret,
            orientation.anchor(hostile.anchor, BuildingKind::Turret.base_stats().size),
        ));
        let oriented_site = site_for(
            BuildingKind::Bastion,
            &UtilityPolicy::new(),
            &orientation.observe(&right),
            &orientation.briefing(&map),
            &[],
            &[],
        )
        .expect("the mirrored shelling admits the same oriented counterbattery");
        assert_eq!(oriented_site, current_site);

        let mut dark = observation(PlayerId(0), LEFT_HOME);
        let remembered = remembered_building(&hostile, dark.tick);
        let remembered_site = site_for(
            BuildingKind::Bastion,
            &UtilityPolicy::new(),
            &dark,
            &map,
            &[],
            std::slice::from_ref(&remembered),
        )
        .expect("fresh memory of the same static threat remains actionable");
        assert_eq!(remembered_site, current_site);

        let public_site = site_for(
            BuildingKind::Bastion,
            &UtilityPolicy::new(),
            &dark,
            &map,
            &[],
            &[],
        )
        .expect("the public eastern prior remains available");
        assert_ne!(remembered_site, public_site);

        dark.enemy_buildings.push(BuildingObs {
            seen: false,
            ..hostile
        });
        assert_eq!(
            site_for(
                BuildingKind::Bastion,
                &UtilityPolicy::new(),
                &dark,
                &map,
                &[],
                &[],
            ),
            Some(public_site),
            "an uncorroborated fog ghost cannot bypass strategic memory"
        );
    }

    #[test]
    fn current_and_remembered_bastions_keep_stationary_fire_on_an_expansion() {
        let map = briefing();
        let expansion = TilePos::new(14, 10);
        let hostile = building(20, PlayerId(1), BuildingKind::Bastion, TilePos::new(14, 2));
        let mut visible = observation(PlayerId(0), LEFT_HOME);
        visible
            .my_buildings
            .push(building(2, PlayerId(0), BuildingKind::Foundry, expansion));
        visible.my_queues.push(Vec::new());
        visible.enemy_buildings.push(hostile.clone());
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, visible.me);
        let ground = GroundKnowledge::new(&visible, &map, &starts);
        let assets = defended_assets(&policy, &visible, &ground);
        let expansion_asset = assets
            .iter()
            .position(|asset| {
                matches!(
                    asset.shape,
                    AssetShape::Building { anchor, .. } if anchor == expansion
                )
            })
            .expect("the expansion is defended");
        let current_origins =
            threat_origin_tiers(&visible, &[], &[], &starts, DefenseDomain::Ground)[1].clone();
        let current = approaches(
            &ground,
            &current_origins,
            &assets,
            None,
            DefenseDomain::Ground,
        );
        let current_shell = current
            .iter()
            .find(|approach| approach.asset == expansion_asset)
            .expect("the visible Bastion can shell the expansion");
        assert_eq!(current_shell.path.len(), 1);
        assert!(matches!(
            current_shell.source.capability,
            ThreatCapability::StaticDefense {
                kind: BuildingKind::Bastion,
                tier: 0
            }
        ));

        let mut dark = visible.clone();
        dark.enemy_buildings.clear();
        let remembered = remembered_building(&hostile, dark.tick);
        let remembered_ground = GroundKnowledge::new(&dark, &map, &starts);
        let remembered_origins = threat_origin_tiers(
            &dark,
            &[],
            std::slice::from_ref(&remembered),
            &starts,
            DefenseDomain::Ground,
        )[2]
        .clone();
        let remembered_routes = approaches(
            &remembered_ground,
            &remembered_origins,
            &assets,
            None,
            DefenseDomain::Ground,
        );
        let remembered_shell = remembered_routes
            .iter()
            .find(|approach| approach.asset == expansion_asset)
            .expect("fresh Bastion memory preserves its known fire envelope");
        assert_eq!(remembered_shell, current_shell);
    }

    #[test]
    fn static_defense_approaches_require_real_range_domain_and_terrain_clearance() {
        let asset = DefendedAsset {
            value: 16,
            shape: AssetShape::Building {
                anchor: LEFT_HOME,
                size: BuildingKind::Foundry.base_stats().size,
            },
            access: None,
        };
        let route_for = |kind: BuildingKind,
                         anchor: TilePos,
                         terrain: fn(TilePos) -> char,
                         domain: DefenseDomain| {
            let scenario = scenario_with(terrain);
            let map = PublicMapBriefing::from_scenario(&scenario).expect("static-threat map");
            let mut obs = observation(PlayerId(0), LEFT_HOME);
            obs.enemy_buildings
                .push(building(20, PlayerId(1), kind, anchor));
            let policy = UtilityPolicy::new();
            let starts = policy.uncleared_hostile_starts(&map, obs.me);
            let ground = GroundKnowledge::new(&obs, &map, &starts);
            let origins = threat_origin_tiers(&obs, &[], &[], &starts, domain)[1].clone();
            let routes = approaches(
                &ground,
                &origins,
                std::slice::from_ref(&asset),
                None,
                domain,
            );
            (origins, routes)
        };
        let open = |_| '.';
        let rock = |tile: TilePos| {
            if tile.y == 7 && (4..=5).contains(&tile.x) {
                '#'
            } else {
                '.'
            }
        };
        let peak = |tile: TilePos| {
            if tile.y == 7 && (4..=5).contains(&tile.x) {
                '^'
            } else {
                '.'
            }
        };

        let (turret, turret_routes) = route_for(
            BuildingKind::Turret,
            TilePos::new(4, 5),
            open,
            DefenseDomain::Ground,
        );
        assert!(matches!(
            turret[0].capability,
            ThreatCapability::StaticDefense {
                kind: BuildingKind::Turret,
                tier: 0
            }
        ));
        assert_eq!(turret_routes.len(), 1);
        assert_eq!(turret_routes[0].path.len(), 1);
        assert!(
            route_for(
                BuildingKind::Turret,
                TilePos::new(4, 5),
                rock,
                DefenseDomain::Ground,
            )
            .1
            .is_empty(),
            "direct Turret fire cannot cross a rock"
        );
        assert!(
            route_for(
                BuildingKind::Turret,
                TilePos::new(4, 0),
                open,
                DefenseDomain::Ground,
            )
            .1
            .is_empty(),
            "a stationary Turret cannot invent a walk from outside its range"
        );

        assert_eq!(
            route_for(
                BuildingKind::Bastion,
                TilePos::new(4, 1),
                rock,
                DefenseDomain::Ground,
            )
            .1
            .len(),
            1,
            "indirect Bastion fire crosses ordinary cover"
        );
        assert!(
            route_for(
                BuildingKind::Bastion,
                TilePos::new(4, 1),
                peak,
                DefenseDomain::Ground,
            )
            .1
            .is_empty(),
            "a peak still blocks indirect shelling"
        );

        assert!(
            route_for(
                BuildingKind::FlakTurret,
                TilePos::new(4, 4),
                open,
                DefenseDomain::Ground,
            )
            .0
            .is_empty(),
            "Flak is not a ground-fire threat"
        );
        let (air_flak, air_flak_routes) = route_for(
            BuildingKind::FlakTurret,
            TilePos::new(4, 4),
            open,
            DefenseDomain::Air,
        );
        assert!(air_flak.is_empty());
        assert!(
            air_flak_routes.is_empty(),
            "stationary anti-air cannot originate an approach against a ground asset"
        );
    }

    #[test]
    fn demolition_walkers_and_air_transports_are_current_defensive_threats() {
        let map = briefing();
        let sapper = TilePos::new(12, 4);
        let skyhook = TilePos::new(24, 6);
        let condor = TilePos::new(28, 8);
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let mut landed_skyhook = unit(21, PlayerId(1), UnitKind::Skyhook, skyhook);
        landed_skyhook.grounded = true;
        let mut landed_condor = unit(22, PlayerId(1), UnitKind::Condor, condor);
        landed_condor.grounded = true;
        obs.enemy_units.extend([
            unit(20, PlayerId(1), UnitKind::Sapper, sapper),
            landed_skyhook,
            landed_condor,
        ]);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);

        assert_eq!(
            threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Ground)[0]
                .iter()
                .map(|origin| origin.anchor)
                .collect::<Vec<_>>(),
            vec![sapper],
            "an unarmed Sapper is still a direct structure threat"
        );
        assert_eq!(
            threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Air)[0]
                .iter()
                .map(|origin| origin.anchor)
                .collect::<Vec<_>>(),
            vec![skyhook, condor],
            "landed transports and bombers remain aircraft in the AA threat tier"
        );
    }

    #[test]
    fn known_mobile_attackers_stop_at_their_exact_standoff_while_priors_press_home() {
        let map = briefing();
        let obs = observation(PlayerId(0), LEFT_HOME);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let asset = DefendedAsset {
            value: 16,
            shape: AssetShape::Building {
                anchor: LEFT_HOME,
                size: BuildingKind::Foundry.base_stats().size,
            },
            access: None,
        };
        let source = TilePos::new(30, 10);
        let origins = [
            ThreatOrigin {
                anchor: source,
                size: None,
                capability: ThreatCapability::Mobile(UnitKind::Sentinel),
                tie: 1,
            },
            ThreatOrigin {
                anchor: source,
                size: None,
                capability: ThreatCapability::Mobile(UnitKind::Avalanche),
                tie: 2,
            },
            ThreatOrigin {
                anchor: source,
                size: None,
                capability: ThreatCapability::Foothold,
                tie: 3,
            },
        ];
        let routes = approaches(
            &ground,
            &origins,
            std::slice::from_ref(&asset),
            None,
            DefenseDomain::Ground,
        );
        let route_for = |mobile| {
            routes
                .iter()
                .find(|route| route.source.mobile_kind() == mobile)
                .expect("each distinct threat provenance keeps an approach")
        };
        let sentinel = route_for(Some(UnitKind::Sentinel));
        let avalanche = route_for(Some(UnitKind::Avalanche));
        let prior = route_for(None);

        assert!(
            avalanche.path.last().unwrap().x > sentinel.path.last().unwrap().x,
            "the Avalanche must stop substantially farther out than a line unit"
        );
        for route in [sentinel, avalanche] {
            let stand = *route.path.last().expect("mobile route has a stand");
            assert!(mobile_threat_can_attack_asset(
                route.source.mobile_kind().unwrap(),
                &asset.shape,
                &map,
                route.goal,
                stand,
            ));
            if route.path.len() > 1 {
                assert!(!mobile_threat_can_attack_asset(
                    route.source.mobile_kind().unwrap(),
                    &asset.shape,
                    &map,
                    route.goal,
                    route.path[route.path.len() - 2],
                ));
            }
        }
        assert_eq!(
            prior.path.last(),
            Some(&prior.goal),
            "a production or public prior has no known weapon and keeps the complete route"
        );

        let blind_origin = ThreatOrigin {
            anchor: TilePos::new(9, 10),
            size: None,
            capability: ThreatCapability::Mobile(UnitKind::Avalanche),
            tie: 4,
        };
        let blind = approaches(
            &ground,
            &[blind_origin],
            std::slice::from_ref(&asset),
            None,
            DefenseDomain::Ground,
        );
        assert_eq!(blind[0].path.first(), Some(&blind_origin.anchor));
        assert_eq!(blind[0].path.last(), Some(&TilePos::new(10, 10)));
        assert!(mobile_threat_can_attack_asset(
            UnitKind::Avalanche,
            &asset.shape,
            &map,
            blind[0].goal,
            *blind[0].path.last().unwrap(),
        ));
        assert!(
            !mobile_threat_can_attack_asset(
                UnitKind::Avalanche,
                &asset.shape,
                &map,
                blind[0].goal,
                blind_origin.anchor,
            ),
            "the source starts inside the Avalanche's dead zone"
        );

        let sapper = ThreatOrigin {
            anchor: source,
            size: None,
            capability: ThreatCapability::Mobile(UnitKind::Sapper),
            tie: 5,
        };
        let sapper_route = &approaches(
            &ground,
            &[sapper],
            std::slice::from_ref(&asset),
            None,
            DefenseDomain::Ground,
        )[0];
        assert!(mobile_threat_can_attack_asset(
            UnitKind::Sapper,
            &asset.shape,
            &map,
            sapper_route.goal,
            *sapper_route.path.last().unwrap(),
        ));
    }

    #[test]
    fn tile_resolution_dead_zone_projection_matches_the_sim_retreat_goal() {
        let mut scenario = scenario_with(|_| '.');
        scenario.units.push(UnitSpec {
            player: 1,
            kind: UnitKind::Avalanche,
            x: 9,
            y: 10,
        });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("retreat briefing");
        let mut state = scenario.build().expect("retreat state");
        let avalanche = state
            .units()
            .iter()
            .find(|unit| unit.player == PlayerId(1) && unit.kind == UnitKind::Avalanche)
            .expect("enemy Avalanche")
            .id;
        let foundry = state
            .buildings()
            .iter()
            .find(|building| {
                building.player == PlayerId(0) && building.kind == BuildingKind::Foundry
            })
            .expect("owned Foundry")
            .id;
        let obs = Observation::omniscient(&state, PlayerId(0));
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let assets = defended_assets(&policy, &obs, &ground);
        let home_asset = assets
            .iter()
            .position(|asset| {
                matches!(
                    asset.shape,
                    AssetShape::Building { anchor, .. } if anchor == LEFT_HOME
                )
            })
            .expect("home is defended");
        let origins =
            threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Ground)[0].clone();
        let projected = approaches(&ground, &origins, &assets, None, DefenseDomain::Ground)
            .into_iter()
            .find(|approach| approach.asset == home_asset)
            .expect("inside-minimum Avalanche has a retreat approach");

        state.tick(&[PlayerCommand {
            player: PlayerId(1),
            command: Command::Attack {
                units: vec![avalanche],
                target: Target::Building(foundry),
                queue: false,
            },
        }]);
        let sim_goal = state
            .unit(avalanche)
            .and_then(|unit| unit.path.as_ref())
            .expect("the sim routes the Avalanche out of its dead zone")
            .goal;
        assert_eq!(projected.path.last(), Some(&sim_goal));
        assert_eq!(sim_goal, TilePos::new(10, 10));
    }

    #[test]
    fn remembered_units_keep_weapon_provenance_but_remembered_production_does_not() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.tick = 4_000;
        let remembered_unit = UnitContact {
            id: UnitId(20),
            player: PlayerId(1),
            kind: UnitKind::Avalanche,
            tile: TilePos::new(30, 10),
            hp: UnitKind::Avalanche.stats().max_hp,
            grounded: false,
            last_seen: obs.tick,
            evidence: ContactEvidence::Remembered,
        };
        let remembered_factory = BuildingContact {
            id: Some(BuildingId(21)),
            player: PlayerId(1),
            kind: BuildingKind::Fabricator,
            anchor: TilePos::new(30, 14),
            hp: BuildingKind::Fabricator.base_stats().max_hp,
            built: true,
            tier: 0,
            last_seen: Some(obs.tick),
            evidence: ContactEvidence::Remembered,
        };
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let origins = threat_origin_tiers(
            &obs,
            std::slice::from_ref(&remembered_unit),
            std::slice::from_ref(&remembered_factory),
            &starts,
            DefenseDomain::Ground,
        )[2]
        .clone();
        assert_eq!(
            origins
                .iter()
                .map(|origin| origin.mobile_kind())
                .collect::<Vec<_>>(),
            vec![Some(UnitKind::Avalanche), None]
        );
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let asset = DefendedAsset {
            value: 16,
            shape: AssetShape::Building {
                anchor: LEFT_HOME,
                size: BuildingKind::Foundry.base_stats().size,
            },
            access: None,
        };
        let routes = approaches(&ground, &origins, &[asset], None, DefenseDomain::Ground);
        let mobile = routes
            .iter()
            .find(|route| route.source.mobile_kind().is_some())
            .expect("remembered Avalanche route");
        let production = routes
            .iter()
            .find(|route| route.source.capability == ThreatCapability::Foothold)
            .expect("remembered production route");
        assert_ne!(mobile.path.last(), Some(&mobile.goal));
        assert_eq!(production.path.last(), Some(&production.goal));
    }

    #[test]
    fn pruned_endpoint_search_matches_exhaustive_routing() {
        let scenario = scenario_with(|tile| {
            let dividing_ridge = tile.x == 20 && !matches!(tile.y, 5 | 18);
            let offset_ridge = tile.x == 26 && (8..=15).contains(&tile.y);
            if dividing_ridge || offset_ridge {
                '#'
            } else {
                '.'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("routing briefing");
        let obs = observation(PlayerId(0), LEFT_HOME);
        let starts = [TilePos::new(8, 4), TilePos::new(8, 12), TilePos::new(8, 19)];
        let goals = [
            TilePos::new(30, 4),
            TilePos::new(30, 12),
            TilePos::new(30, 19),
        ];
        let hostile_starts: Vec<_> = map.hostile_starting_foundries(obs.me).copied().collect();
        let ground = GroundKnowledge::new(&obs, &map, &hostile_starts);

        for (candidate, domain) in [
            (None, DefenseDomain::Ground),
            (
                Some(PlacementFootprint {
                    anchor: TilePos::new(19, 4),
                    size: (2, 2),
                    blocks_ground: true,
                }),
                DefenseDomain::Ground,
            ),
            (None, DefenseDomain::Air),
        ] {
            assert_eq!(
                shortest_path_between(&ground, &starts, &goals, candidate, domain),
                shortest_path_between_exhaustive(&ground, &starts, &goals, candidate, domain),
                "lower-bound and exhausted-component pruning must retain the exhaustive route"
            );
        }

        let divided_scenario = scenario_with(|tile| if tile.x == 20 { '#' } else { '.' });
        let divided_map =
            PublicMapBriefing::from_scenario(&divided_scenario).expect("divided briefing");
        let divided_starts: Vec<_> = divided_map
            .hostile_starting_foundries(obs.me)
            .copied()
            .collect();
        let divided = GroundKnowledge::new(&obs, &divided_map, &divided_starts);
        assert_eq!(
            shortest_path_between(&divided, &starts, &goals, None, DefenseDomain::Ground,),
            shortest_path_between_exhaustive(
                &divided,
                &starts,
                &goals,
                None,
                DefenseDomain::Ground,
            ),
            "one exhausted island component must prove every disconnected endpoint without changing the result"
        );
    }

    #[test]
    fn cached_endpoint_routes_match_candidate_specific_searches() {
        let scenario = scenario_with(|tile| {
            let first_ridge = tile.x == 16 && !matches!(tile.y, 4 | 5 | 17 | 18);
            let second_ridge = tile.x == 24 && (8..=14).contains(&tile.y);
            if first_ridge || second_ridge {
                '#'
            } else {
                '.'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("route-cache briefing");
        let obs = observation(PlayerId(0), LEFT_HOME);
        let hostile_starts: Vec<_> = map.hostile_starting_foundries(obs.me).copied().collect();
        let ground = GroundKnowledge::new(&obs, &map, &hostile_starts);
        let starts = [TilePos::new(8, 4), TilePos::new(8, 12), TilePos::new(8, 19)];
        let goals = [
            TilePos::new(30, 4),
            TilePos::new(30, 12),
            TilePos::new(30, 19),
        ];
        let baseline = shortest_path_between(&ground, &starts, &goals, None, DefenseDomain::Ground)
            .expect("the endpoint fixture has a baseline route")
            .2;
        let mut anchors = BTreeSet::from([
            TilePos::new(1, 1),
            TilePos::new(16, 4),
            TilePos::new(24, 7),
            TilePos::new(24, 15),
            TilePos::new(31, 20),
        ]);
        anchors.extend(baseline.iter().copied());
        for pair in baseline
            .windows(2)
            .filter(|pair| pair[0].x != pair[1].x && pair[0].y != pair[1].y)
        {
            anchors.insert(TilePos::new(pair[1].x, pair[0].y));
            anchors.insert(TilePos::new(pair[0].x, pair[1].y));
        }

        let mut cache = BTreeMap::new();
        for anchor in anchors {
            let candidate = PlacementFootprint {
                anchor,
                size: (1, 1),
                blocks_ground: true,
            };
            assert_eq!(
                shortest_path_between_cached(
                    &ground,
                    &starts,
                    &goals,
                    candidate,
                    DefenseDomain::Ground,
                    &mut cache,
                ),
                shortest_path_between(
                    &ground,
                    &starts,
                    &goals,
                    Some(candidate),
                    DefenseDomain::Ground,
                ),
                "cached routing changed the exact endpoint or path at {anchor:?}"
            );
        }
    }

    #[test]
    fn legal_fire_across_disconnected_ground_remains_a_current_approach() {
        for (kind, source, barrier_x, barrier) in [
            (UnitKind::Avalanche, TilePos::new(19, 10), 12, '#'),
            (UnitKind::Lancer, TilePos::new(11, 10), 8, '~'),
        ] {
            let scenario = scenario_with(|tile| {
                if tile.x == barrier_x || tile.x == WIDTH - 1 - barrier_x {
                    barrier
                } else {
                    '.'
                }
            });
            let map = PublicMapBriefing::from_scenario(&scenario)
                .unwrap_or_else(|error| panic!("{kind:?} barrier fixture: {error}"));
            let obs = observation(PlayerId(0), LEFT_HOME);
            let policy = UtilityPolicy::new();
            let starts = policy.uncleared_hostile_starts(&map, obs.me);
            let ground = GroundKnowledge::new(&obs, &map, &starts);
            let asset = DefendedAsset {
                value: 16,
                shape: AssetShape::Building {
                    anchor: LEFT_HOME,
                    size: BuildingKind::Foundry.base_stats().size,
                },
                access: None,
            };
            let goals = asset.shape.approach_tiles(&ground, DefenseDomain::Ground);
            assert!(
                shortest_path_between(&ground, &[source], &goals, None, DefenseDomain::Ground,)
                    .is_none(),
                "the {kind:?} cannot walk across the authored barrier"
            );
            let origin = ThreatOrigin {
                anchor: source,
                size: None,
                capability: ThreatCapability::Mobile(kind),
                tie: 1,
            };
            let routes = approaches(
                &ground,
                &[origin],
                std::slice::from_ref(&asset),
                None,
                DefenseDomain::Ground,
            );
            assert_eq!(routes.len(), 1);
            assert_eq!(routes[0].path, vec![source]);
            assert!(mobile_threat_can_attack_asset(
                kind,
                &asset.shape,
                &map,
                routes[0].goal,
                source,
            ));

            let right = observation(PlayerId(1), RIGHT_HOME);
            let orientation = Orientation::for_home(&right, RIGHT_HOME);
            let oriented_obs = orientation.observe(&right);
            let oriented_map = orientation.briefing(&map);
            let oriented_starts = policy.uncleared_hostile_starts(&oriented_map, oriented_obs.me);
            let oriented_ground =
                GroundKnowledge::new(&oriented_obs, &oriented_map, &oriented_starts);
            let oriented_asset = DefendedAsset {
                shape: AssetShape::Building {
                    anchor: LEFT_HOME,
                    size: BuildingKind::Foundry.base_stats().size,
                },
                ..asset.clone()
            };
            let oriented = approaches(
                &oriented_ground,
                &[origin],
                &[oriented_asset],
                None,
                DefenseDomain::Ground,
            );
            assert_eq!(oriented, routes, "{kind:?} disconnected fire must mirror");
        }
    }

    #[test]
    fn disconnected_out_of_range_mobile_threat_does_not_invent_a_firing_stand() {
        let scenario = scenario_with(|tile| if tile.x == 12 { '~' } else { '.' });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("pit-wall briefing");
        let obs = observation(PlayerId(0), LEFT_HOME);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let asset = DefendedAsset {
            value: 16,
            shape: AssetShape::Building {
                anchor: LEFT_HOME,
                size: BuildingKind::Foundry.base_stats().size,
            },
            access: None,
        };
        let origin = ThreatOrigin {
            anchor: TilePos::new(19, 10),
            size: None,
            capability: ThreatCapability::Mobile(UnitKind::Lancer),
            tie: 1,
        };
        assert!(
            approaches(&ground, &[origin], &[asset], None, DefenseDomain::Ground,).is_empty(),
            "an out-of-range Lancer cannot cross the pit or invent a rim stand"
        );
    }

    #[test]
    fn mobile_standoff_uses_public_direct_and_indirect_cover() {
        let asset = AssetShape::Building {
            anchor: TilePos::new(10, 10),
            size: (1, 1),
        };
        let route_goal = TilePos::new(9, 10);
        let sentinel_stand = TilePos::new(13, 10);
        let avalanche_stand = TilePos::new(24, 10);
        let open = briefing();
        assert!(mobile_threat_can_attack_asset(
            UnitKind::Sentinel,
            &asset,
            &open,
            route_goal,
            sentinel_stand,
        ));
        assert!(mobile_threat_can_attack_asset(
            UnitKind::Avalanche,
            &asset,
            &open,
            route_goal,
            avalanche_stand,
        ));

        let rock = PublicMapBriefing::from_scenario(&scenario_with(|tile| {
            if tile == TilePos::new(12, 10) {
                '#'
            } else {
                '.'
            }
        }))
        .expect("rock briefing");
        assert!(!mobile_threat_can_attack_asset(
            UnitKind::Sentinel,
            &asset,
            &rock,
            route_goal,
            sentinel_stand,
        ));
        assert!(mobile_threat_can_attack_asset(
            UnitKind::Avalanche,
            &asset,
            &rock,
            route_goal,
            avalanche_stand,
        ));

        let peak = PublicMapBriefing::from_scenario(&scenario_with(|tile| {
            if tile == TilePos::new(12, 10) {
                '^'
            } else {
                '.'
            }
        }))
        .expect("peak briefing");
        assert!(!mobile_threat_can_attack_asset(
            UnitKind::Sentinel,
            &asset,
            &peak,
            route_goal,
            sentinel_stand,
        ));
        assert!(!mobile_threat_can_attack_asset(
            UnitKind::Avalanche,
            &asset,
            &peak,
            route_goal,
            avalanche_stand,
        ));
    }

    fn assert_mobile_standoff_is_oriented_symmetrically(first: TilePos, second: TilePos) {
        let scenario = scenario_with_starts(first, second, |_| '.');
        let map = PublicMapBriefing::from_scenario(&scenario).expect("symmetric open briefing");
        let first_obs = observation(PlayerId(0), first);
        let second_obs = observation(PlayerId(1), second);
        let orientation = Orientation::for_home(&second_obs, second);
        let oriented_obs = orientation.observe(&second_obs);
        let oriented_map = orientation.briefing(&map);
        let source = first.offset(8, 8);
        let origin = ThreatOrigin {
            anchor: source,
            size: None,
            capability: ThreatCapability::Mobile(UnitKind::Avalanche),
            tie: 1,
        };
        let asset = DefendedAsset {
            value: 16,
            shape: AssetShape::Building {
                anchor: first,
                size: BuildingKind::Foundry.base_stats().size,
            },
            access: None,
        };
        let policy = UtilityPolicy::new();
        let first_starts = policy.uncleared_hostile_starts(&map, first_obs.me);
        let first_ground = GroundKnowledge::new(&first_obs, &map, &first_starts);
        let first_route = approaches(
            &first_ground,
            &[origin],
            std::slice::from_ref(&asset),
            None,
            DefenseDomain::Ground,
        );

        let oriented_starts = policy.uncleared_hostile_starts(&oriented_map, oriented_obs.me);
        let oriented_ground = GroundKnowledge::new(&oriented_obs, &oriented_map, &oriented_starts);
        let oriented_origin = ThreatOrigin {
            anchor: source,
            ..origin
        };
        let oriented_asset = DefendedAsset {
            shape: AssetShape::Building {
                anchor: orientation.anchor(second, BuildingKind::Foundry.base_stats().size),
                size: BuildingKind::Foundry.base_stats().size,
            },
            ..asset.clone()
        };
        let oriented_route = approaches(
            &oriented_ground,
            &[oriented_origin],
            &[oriented_asset],
            None,
            DefenseDomain::Ground,
        );

        assert_eq!(oriented_route, first_route);
    }

    #[test]
    fn mobile_standoff_preserves_y_only_and_full_turn_symmetry() {
        assert_mobile_standoff_is_oriented_symmetrically(TilePos::new(18, 3), TilePos::new(18, 19));
        assert_mobile_standoff_is_oriented_symmetrically(TilePos::new(4, 3), TilePos::new(34, 19));
    }

    #[test]
    fn simultaneous_current_fronts_each_contribute_an_approach() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let north = TilePos::new(4, 1);
        let east = TilePos::new(25, 10);
        obs.enemy_units.extend([
            unit(20, PlayerId(1), UnitKind::Sentinel, north),
            unit(21, PlayerId(1), UnitKind::Sentinel, east),
        ]);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let assets = defended_assets(&policy, &obs, &ground);
        let home_asset = assets
            .iter()
            .position(|asset| {
                matches!(
                    asset.shape,
                    AssetShape::Building { anchor, .. } if anchor == LEFT_HOME
                )
            })
            .expect("the home Foundry is defended");
        let current = &threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Ground)[0];
        let sources: BTreeSet<_> =
            approaches(&ground, current, &assets, None, DefenseDomain::Ground)
                .into_iter()
                .filter(|approach| approach.asset == home_asset)
                .map(|approach| approach.source.anchor)
                .collect();

        assert_eq!(sources, BTreeSet::from([north, east]));
    }

    #[test]
    fn candidate_threat_timing_ignores_an_uncovered_nearer_lane() {
        let map = briefing();
        let obs = observation(PlayerId(0), LEFT_HOME);
        let profile = DefenseProfile::for_kind(BuildingKind::Turret).expect("Turret profile");
        let candidate = TilePos::new(15, 10);
        let near_source = ThreatOrigin {
            anchor: TilePos::new(1, 1),
            size: None,
            capability: ThreatCapability::Mobile(UnitKind::Sentinel),
            tie: 20,
        };
        let far_source = ThreatOrigin {
            anchor: TilePos::new(35, 10),
            size: None,
            capability: ThreatCapability::Mobile(UnitKind::Sentinel),
            tie: 21,
        };
        let near_cost = 10;
        let far_cost = 180;
        let approaches = [
            Approach {
                asset: 0,
                source: near_source,
                goal: TilePos::new(2, 2),
                path: vec![TilePos::new(2, 2)],
                baseline_cost: near_cost,
                disrupted: false,
            },
            Approach {
                asset: 1,
                source: far_source,
                goal: candidate.offset(2, 0),
                path: vec![candidate.offset(2, 0)],
                baseline_cost: far_cost,
                disrupted: false,
            },
            Approach {
                asset: 2,
                source: far_source,
                goal: candidate.offset(0, 2),
                path: vec![candidate.offset(0, 2)],
                baseline_cost: far_cost + 10,
                disrupted: false,
            },
        ];

        let summary = candidate_threat_summary(
            &obs,
            &map,
            &[],
            &[],
            profile,
            candidate,
            &approaches,
            DefenseOpportunityEvidence::CurrentArmed,
        )
        .expect("the far lane contributes marginal coverage");

        assert_eq!(summary.evidence, DefenseOpportunityEvidence::CurrentArmed);
        assert_eq!(summary.evidence_count, 1);
        assert_eq!(
            summary.threat_arrival_ticks,
            Some(travel_ticks(far_cost, UnitKind::Sentinel.stats().speed))
        );
        assert!(
            travel_ticks(near_cost, UnitKind::Sentinel.stats().speed)
                < summary.threat_arrival_ticks.unwrap(),
            "the uncovered near lane must not make this exact site look imminent"
        );
    }

    #[test]
    fn unreachable_current_gun_cannot_promote_a_contributing_foothold() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.enemy_buildings.extend([
            building(20, PlayerId(1), BuildingKind::Foundry, RIGHT_HOME),
            building(
                21,
                PlayerId(1),
                BuildingKind::Turret,
                TilePos::new(WIDTH - 1, 0),
            ),
        ]);
        let builders = vec![&obs.my_units[0]];

        let quote = UtilityPolicy::new()
            .strategic_defense_quote_grounded(
                BuildingKind::Turret,
                &obs,
                &map,
                &[],
                &[],
                &builders,
                &mut None,
            )
            .expect("the reachable current Foundry supports a defensive opportunity");

        assert_eq!(quote.evidence, DefenseOpportunityEvidence::CurrentFoothold);
        assert_eq!(quote.evidence_count, 1);
        assert_eq!(quote.threat_arrival_ticks, None);
    }

    #[test]
    fn air_contacts_and_blips_do_not_misdirect_ground_defense() {
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.enemy_units
            .push(unit(20, PlayerId(1), UnitKind::Condor, TilePos::new(4, 0)));
        obs.blips.push(TilePos::new(4, 1));

        let selected = site(&UtilityPolicy::new(), &obs, &briefing(), &[], &[])
            .expect("the public ground approach remains actionable");
        assert!(selected.x > LEFT_HOME.x + 1);
    }

    #[test]
    fn bastion_ties_break_backline_while_short_guns_break_toward_the_threat() {
        let neutral = Coverage {
            new: 16,
            reinforced: 0,
            unplanned_new: 16,
            unplanned_reinforced: 0,
            interception: 64,
            protected_value: 16,
            planned_overlap: 0,
            blind_exposure: 0,
            spotted_reach: 0,
            redundant: 0,
            lateral: 1,
        };
        let front = Candidate {
            anchor: TilePos::new(14, 10),
            builder_travel: 20,
            coverage: neutral,
            threat_distance: 5,
        };
        let back = Candidate {
            anchor: TilePos::new(9, 10),
            threat_distance: 10,
            ..front
        };
        let turret = DefenseProfile::for_kind(BuildingKind::Turret).expect("Turret profile");
        let bastion = DefenseProfile::for_kind(BuildingKind::Bastion).expect("Bastion profile");

        assert!(front.key(turret) > back.key(turret));
        assert!(back.key(bastion) > front.key(bastion));
    }

    #[test]
    fn bastion_outer_fire_requires_a_spotter_and_its_blind_ring_rewards_a_screen() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let asset = DefendedAsset {
            value: 16,
            shape: AssetShape::Building {
                anchor: LEFT_HOME,
                size: BuildingKind::Foundry.base_stats().size,
            },
            access: None,
        };
        let outer = TilePos::new(19, 11);
        let blind = TilePos::new(12, 11);
        let approach = Approach {
            asset: 0,
            source: ThreatOrigin {
                anchor: outer,
                size: None,
                capability: ThreatCapability::Foothold,
                tie: 1,
            },
            goal: blind,
            path: vec![outer, blind],
            baseline_cost: 10,
            disrupted: false,
        };
        let bastion = DefenseProfile::for_kind(BuildingKind::Bastion).expect("Bastion profile");
        let anchor = TilePos::new(10, 10);

        let unspotted = scored_coverage!(
            &obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[],
            &[],
            bastion,
            anchor,
        );
        assert_eq!(unspotted.new, 0);

        let index = usize::try_from(outer.y * WIDTH + outer.x).expect("visible index in bounds");
        obs.visible[index] = true;
        let exposed = scored_coverage!(
            &obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[],
            &[],
            bastion,
            anchor,
        );
        assert_eq!(
            exposed.new, 0,
            "transient sight cannot underwrite a Bastion"
        );

        let screen = building(30, PlayerId(0), BuildingKind::Turret, TilePos::new(13, 11));
        obs.my_buildings.push(screen.clone());
        obs.my_queues.push(Vec::new());
        let screened = scored_coverage!(
            &obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[&screen],
            &[],
            bastion,
            anchor,
        );
        assert_eq!(screened.new, asset.value);
        assert_eq!(screened.blind_exposure, 0);
        assert!(
            Candidate {
                anchor,
                builder_travel: 0,
                coverage: screened,
                threat_distance: 10,
            }
            .key(bastion)
                > Candidate {
                    anchor,
                    builder_travel: 0,
                    coverage: exposed,
                    threat_distance: 10,
                }
                .key(bastion)
        );
    }

    #[test]
    fn unsupported_live_and_planned_bastions_do_not_suppress_supported_coverage() {
        let map = briefing();
        let asset = DefendedAsset {
            value: 16,
            shape: AssetShape::Building {
                anchor: LEFT_HOME,
                size: BuildingKind::Foundry.base_stats().size,
            },
            access: None,
        };
        let target = TilePos::new(15, 4);
        let approach = Approach {
            asset: 0,
            source: ThreatOrigin {
                anchor: target,
                size: None,
                capability: ThreatCapability::Foothold,
                tie: 1,
            },
            goal: target,
            path: vec![target],
            baseline_cost: 0,
            disrupted: false,
        };
        let profile = DefenseProfile::for_kind(BuildingKind::Bastion).expect("Bastion profile");
        let candidate = TilePos::new(10, 4);
        let unsupported_anchor = TilePos::new(6, 4);

        let mut live_obs = observation(PlayerId(0), LEFT_HOME);
        live_obs.my_buildings.push(building(
            30,
            PlayerId(0),
            BuildingKind::Bastion,
            unsupported_anchor,
        ));
        live_obs.my_queues.push(Vec::new());
        let existing = existing_defenses(&live_obs, DefenseDomain::Ground);
        assert!(defense_covers(&map, profile, unsupported_anchor, target));
        assert!(!building_covers(
            &live_obs,
            &map,
            existing[0],
            target,
            DefenseDomain::Ground,
        ));
        let against_live = scored_coverage!(
            &live_obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &existing,
            &[],
            profile,
            candidate,
        );
        assert_eq!(against_live.new, asset.value);
        assert_eq!(against_live.reinforced, 0);

        let mut planned_obs = observation(PlayerId(0), LEFT_HOME);
        let mut planned_bastion =
            building(30, PlayerId(0), BuildingKind::Bastion, unsupported_anchor);
        planned_bastion.built = false;
        planned_obs.my_buildings.push(planned_bastion);
        planned_obs.my_queues.push(Vec::new());
        let planned = planned_defenses(&planned_obs, DefenseDomain::Ground);
        assert!(!planned_defense_covers(
            &planned_obs,
            &map,
            &planned[0],
            target,
        ));
        let against_planned = scored_coverage!(
            &planned_obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[],
            &planned,
            profile,
            candidate,
        );
        assert_eq!(against_planned.new, asset.value);
        assert_eq!(against_planned.planned_overlap, 0);
    }

    #[test]
    fn unreachable_current_contact_yields_to_the_reachable_public_approach() {
        let trapped = TilePos::new(4, 3);
        let scenario = scenario_with(|tile| {
            if tile.chebyshev(trapped) == 1 {
                '^'
            } else {
                '.'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("trap briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.known_rock = (-1..=1)
            .flat_map(|dy| (-1..=1).map(move |dx| trapped.offset(dx, dy)))
            .filter(|tile| *tile != trapped)
            .collect();
        obs.known_peaks = obs.known_rock.clone();
        obs.enemy_units
            .push(unit(20, PlayerId(1), UnitKind::Sentinel, trapped));

        let selected = site(&UtilityPolicy::new(), &obs, &map, &[], &[])
            .expect("the lower-priority public road remains actionable");
        assert!(selected.x > LEFT_HOME.x + 1);
    }

    #[test]
    fn statically_disconnected_public_start_does_not_buy_a_ground_turret() {
        let scenario = scenario_with(|tile| if tile.x == 20 { '^' } else { '.' });
        let briefing = PublicMapBriefing::from_scenario(&scenario).expect("island briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.known_rock = (0..HEIGHT).map(|y| TilePos::new(20, y)).collect();
        obs.known_peaks = obs.known_rock.clone();

        assert_eq!(site(&UtilityPolicy::new(), &obs, &briefing, &[], &[]), None);
    }

    #[test]
    fn a_candidate_on_an_approach_is_rerouted_and_a_one_tile_lane_cannot_be_severed() {
        let open_map = briefing();
        let open_obs = observation(PlayerId(0), LEFT_HOME);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&open_map, open_obs.me);
        let ground = GroundKnowledge::new(&open_obs, &open_map, &starts);
        let assets = defended_assets(&policy, &open_obs, &ground);
        let origins =
            threat_origin_tiers(&open_obs, &[], &[], &starts, DefenseDomain::Ground)[3].clone();
        let baseline = approaches(&ground, &origins, &assets, None, DefenseDomain::Ground);
        let home_asset = assets
            .iter()
            .position(|asset| {
                matches!(
                    asset.shape,
                    AssetShape::Building { anchor, .. } if anchor == LEFT_HOME
                )
            })
            .expect("the Foundry is a defended asset");
        let original = baseline
            .iter()
            .find(|approach| approach.asset == home_asset)
            .expect("the public start has an open approach to home");
        let candidate = original.path[original.path.len() / 2];
        let rerouted = approaches_with_candidate(
            &ground,
            &assets,
            &baseline,
            DefenseProfile::for_kind(BuildingKind::Turret)
                .expect("Turret profile")
                .footprint(candidate),
            DefenseDomain::Ground,
            None,
        )
        .expect("the open map has an alternate route");
        let changed = rerouted
            .iter()
            .find(|approach| approach.asset == home_asset)
            .expect("the open map has an alternate route");
        assert_ne!(changed.path, original.path);
        assert!(!changed.path.contains(&candidate));
        assert!(path_cost(&changed.path) >= path_cost(&original.path));

        let corridor_scenario = scenario_with(|tile| if tile.y == LEFT_HOME.y { '.' } else { '^' });
        let corridor_map =
            PublicMapBriefing::from_scenario(&corridor_scenario).expect("corridor briefing");
        let corridor_obs = observation(PlayerId(0), LEFT_HOME);
        let corridor_starts = policy.uncleared_hostile_starts(&corridor_map, corridor_obs.me);
        let corridor_ground = GroundKnowledge::new(&corridor_obs, &corridor_map, &corridor_starts);
        let corridor_assets = defended_assets(&policy, &corridor_obs, &corridor_ground);
        let corridor_origins = threat_origin_tiers(
            &corridor_obs,
            &[],
            &[],
            &corridor_starts,
            DefenseDomain::Ground,
        )[3]
        .clone();
        let corridor_baseline = approaches(
            &corridor_ground,
            &corridor_origins,
            &corridor_assets,
            None,
            DefenseDomain::Ground,
        );
        assert!(
            corridor_baseline
                .iter()
                .any(|approach| approach.path.contains(&TilePos::new(20, LEFT_HOME.y))),
            "the authored road has one real choke tile"
        );
        let blocked = approaches_with_candidate(
            &corridor_ground,
            &corridor_assets,
            &corridor_baseline,
            DefenseProfile::for_kind(BuildingKind::Turret)
                .expect("Turret profile")
                .footprint(TilePos::new(20, LEFT_HOME.y)),
            DefenseDomain::Ground,
            None,
        );
        assert!(
            blocked.is_none(),
            "a defensive footprint occupying the only road must be rejected"
        );
    }

    #[test]
    fn a_candidate_on_a_diagonal_companion_reroutes_the_actual_approach() {
        let map = briefing();
        let obs = observation(PlayerId(0), LEFT_HOME);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let source = ThreatOrigin {
            anchor: TilePos::new(10, 4),
            size: None,
            capability: ThreatCapability::Foothold,
            tie: 0,
        };
        let goal = TilePos::new(12, 6);
        let original_path = vec![source.anchor, TilePos::new(11, 5), goal];
        let baseline = vec![Approach {
            asset: 0,
            source,
            goal,
            baseline_cost: path_cost(&original_path),
            path: original_path.clone(),
            disrupted: false,
        }];
        let assets = vec![DefendedAsset {
            value: 1,
            shape: AssetShape::Scrap {
                tiles: vec![goal],
                work_tiles: vec![goal],
            },
            access: None,
        }];
        let barricade = DefenseProfile::for_kind(BuildingKind::Barricade)
            .expect("Barricade profile")
            .footprint(TilePos::new(11, 4));
        assert!(original_path.iter().all(|tile| !barricade.blocks(*tile)));

        let rerouted = approaches_with_candidate(
            &ground,
            &assets,
            &baseline,
            barricade,
            DefenseDomain::Ground,
            None,
        )
        .expect("the open map retains a two-cardinal detour");
        assert_eq!(rerouted.len(), 1);
        assert!(rerouted[0].disrupted);
        assert_ne!(rerouted[0].path, original_path);
        assert_eq!(path_cost(&rerouted[0].path), baseline[0].baseline_cost + 6);
    }

    #[test]
    fn a_turret_candidate_cannot_cut_the_only_foundry_to_scrap_route() {
        let scrap = TilePos::new(12, LEFT_HOME.y);
        let scenario = scenario_with(|tile| {
            if tile == scrap {
                's'
            } else if tile.y == LEFT_HOME.y {
                '.'
            } else {
                '^'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("scrap corridor briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.known_scrap = vec![(scrap, SCRAP_NODE_AMOUNT)];
        obs.my_units[0].tile = scrap.offset(-1, 0);
        obs.my_units[0].idle = false;
        obs.my_units[0].harvesting = Some(scrap);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let assets = defended_assets(&policy, &obs, &ground);
        let access = assets
            .iter()
            .find_map(|asset| asset.access.as_ref())
            .expect("the nearby scrap cluster has a Foundry work route");
        let choke = TilePos::new(9, LEFT_HOME.y);
        assert!(access.path.contains(&choke));
        let turret = DefenseProfile::for_kind(BuildingKind::Turret).expect("Turret profile");
        assert!(scrap_access_survives(
            &ground,
            &assets,
            turret.footprint(TilePos::new(20, LEFT_HOME.y)),
            None,
        ));
        assert!(
            !scrap_access_survives(&ground, &assets, turret.footprint(choke), None),
            "the only income route cannot be traded for a defensive footprint"
        );
    }

    #[test]
    fn a_rectangular_footprint_on_one_diagonal_companion_keeps_resource_access() {
        let map = briefing();
        let obs = observation(PlayerId(0), LEFT_HOME);
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let work = TilePos::new(14, 20);
        let foundry_doorsteps =
            building_doorsteps(&ground, LEFT_HOME, BuildingKind::Foundry.base_stats().size);
        let (_, _, path) = shortest_path_between(
            &ground,
            &foundry_doorsteps,
            &[work],
            None,
            DefenseDomain::Ground,
        )
        .expect("the open map has a diagonal income route");
        let diagonal = path
            .windows(2)
            .find(|pair| pair[0].x != pair[1].x && pair[0].y != pair[1].y)
            .expect("the chosen work route contains a diagonal step");
        let companions = [
            TilePos::new(diagonal[1].x, diagonal[0].y),
            TilePos::new(diagonal[0].x, diagonal[1].y),
        ];
        let anchor = companions
            .into_iter()
            .find(|tile| !path.contains(tile))
            .expect("a diagonal companion is not itself a path vertex");
        let barricade = DefenseProfile::for_kind(BuildingKind::Barricade)
            .expect("Barricade profile")
            .footprint(anchor);
        assert!(path.iter().all(|tile| !barricade.blocks(*tile)));
        assert_eq!(
            companions
                .into_iter()
                .filter(|tile| barricade.blocks(*tile))
                .count(),
            1,
            "the rectangular footprint touches exactly one corner companion"
        );

        let (_, _, rerouted) = shortest_path_between(
            &ground,
            &foundry_doorsteps,
            &[work],
            Some(barricade),
            DefenseDomain::Ground,
        )
        .expect("the other cardinal companion leaves a two-step detour");
        let detour = path_cost(&rerouted).saturating_sub(path_cost(&path));
        assert!((1..=MAX_BARRICADE_RESOURCE_DETOUR_COST).contains(&detour));

        let resource = DefendedAsset {
            value: 1,
            shape: AssetShape::Scrap {
                tiles: vec![work],
                work_tiles: vec![work],
            },
            access: Some(AccessRoute {
                foundry: LEFT_HOME,
                work_tiles: vec![work],
                path,
            }),
        };
        assert!(
            !scrap_access_survives(
                &ground,
                std::slice::from_ref(&resource),
                barricade,
                Some(detour - 1),
            ),
            "the cached diagonal must not hide the candidate's real detour"
        );
        assert!(scrap_access_survives(
            &ground,
            std::slice::from_ref(&resource),
            barricade,
            Some(MAX_BARRICADE_RESOURCE_DETOUR_COST),
        ));
    }

    #[test]
    fn unworked_public_scrap_cannot_anchor_an_isolated_frontier_turret() {
        let frontier = TilePos::new(18, LEFT_HOME.y);
        let isolated_site = TilePos::new(18, LEFT_HOME.y - 2);
        let scenario = scenario_with(|tile| if tile == frontier { 'S' } else { '.' });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("frontier briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.explored.fill(false);
        obs.my_units[0].tile = frontier.offset(-1, 0);
        obs.my_units[0].idle = false;
        obs.my_units[0].carrying = 1;
        let policy = UtilityPolicy::new();
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        assert!(ground.scrap.contains_key(&frontier));
        let assets = defended_assets(&policy, &obs, &ground);
        assert!(
            assets
                .iter()
                .all(|asset| !matches!(asset.shape, AssetShape::Scrap { .. })),
            "authored salvage remains public terrain knowledge, not established value"
        );
        let baseline = approaches(
            &ground,
            &threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Ground)[3],
            &assets,
            None,
            DefenseDomain::Ground,
        );
        let turret = DefenseProfile::for_kind(BuildingKind::Turret).expect("Turret profile");
        let isolated_footprint = turret.footprint(isolated_site);
        let isolated_approaches = approaches_with_candidate(
            &ground,
            &assets,
            &baseline,
            isolated_footprint,
            DefenseDomain::Ground,
            None,
        )
        .expect("the isolated site does not sever the open map");
        let old_coverage = scored_coverage!(
            &obs,
            &map,
            &assets,
            &isolated_approaches,
            &[],
            &[],
            turret,
            isolated_site,
        );
        assert!(
            old_coverage.new > 0,
            "the geometric scorer alone still likes the unsupported interception line"
        );
        assert!(
            operationally_supported_approaches(
                &ground,
                &assets,
                &baseline,
                isolated_footprint,
                DefenseDomain::Ground,
                None,
            )
            .expect("the open map remains connected")
            .is_empty(),
            "an emplacement beyond base support cannot defend speculative salvage"
        );

        obs.my_units[0].harvesting = Some(frontier);
        let worked_ground = GroundKnowledge::new(&obs, &map, &starts);
        let worked_assets = defended_assets(&policy, &obs, &worked_ground);
        let resource_asset = worked_assets
            .iter()
            .position(|asset| matches!(asset.shape, AssetShape::Scrap { .. }))
            .expect("a Harvester actively working the frontier establishes its value");
        let worked_baseline = approaches(
            &worked_ground,
            &threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Ground)[3],
            &worked_assets,
            None,
            DefenseDomain::Ground,
        );
        assert!(
            operationally_supported_approaches(
                &worked_ground,
                &worked_assets,
                &worked_baseline,
                isolated_footprint,
                DefenseDomain::Ground,
                None,
            )
            .expect("the open map remains connected")
            .iter()
            .any(|approach| approach.asset == resource_asset),
            "the same frontier becomes defensible once ordinary workers actually operate there"
        );
    }

    #[test]
    fn public_terrain_requires_a_reachable_builder_for_the_site() {
        let scenario = scenario_with(|tile| if tile.x == 20 { '^' } else { '.' });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("split briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.my_units[0].tile = TilePos::new(28, 5);
        obs.enemy_buildings.push(building(
            20,
            PlayerId(1),
            BuildingKind::Fabricator,
            TilePos::new(12, 3),
        ));

        assert_eq!(site(&UtilityPolicy::new(), &obs, &map, &[], &[]), None);

        obs.my_units.push(unit(
            2,
            PlayerId(0),
            UnitKind::Harvester,
            LEFT_HOME.offset(3, 3),
        ));
        assert!(site(&UtilityPolicy::new(), &obs, &map, &[], &[]).is_some());
    }

    #[test]
    fn ground_mobile_reinforcement_requires_a_public_route() {
        let blocked_scenario = scenario_with(|tile| if tile.x == 20 { '^' } else { '.' });
        let blocked_map =
            PublicMapBriefing::from_scenario(&blocked_scenario).expect("split briefing");
        let mut blocked_obs = observation(PlayerId(0), LEFT_HOME);
        blocked_obs.my_units = vec![unit(
            7,
            PlayerId(0),
            UnitKind::Sentinel,
            TilePos::new(28, 10),
        )];
        blocked_obs.known_peaks = (0..HEIGHT).map(|y| TilePos::new(20, y)).collect();
        blocked_obs.known_rock = blocked_obs.known_peaks.clone();
        let policy = UtilityPolicy::new();
        let mut blocked = DefenseThinkContext::new(&policy, &blocked_obs, &blocked_map, &[], &[]);
        assert_eq!(
            blocked.reinforcement_travel_cost(
                &blocked_obs.my_units[0],
                BuildingKind::Turret,
                LEFT_HOME.offset(4, 0),
            ),
            None,
            "a ground fighter across impassable public terrain is not reinforcement"
        );
        assert_eq!(
            blocked.reinforcement_travel_cost(
                &blocked_obs.my_units[0],
                BuildingKind::Turret,
                LEFT_HOME.offset(4, 0),
            ),
            None,
            "the cached unreachable result is exact"
        );
        assert_eq!(blocked.cache_stats().reinforcement_route_builds, 1);
        assert_eq!(blocked.cache_stats().reinforcement_route_hits, 1);

        let routed_scenario = scenario_with(|tile| {
            if tile.x == 20 && tile.y != 10 {
                '^'
            } else {
                '.'
            }
        });
        let routed_map =
            PublicMapBriefing::from_scenario(&routed_scenario).expect("one-gap briefing");
        let mut routed_obs = blocked_obs;
        routed_obs.known_peaks = (0..HEIGHT)
            .filter(|y| *y != 10)
            .map(|y| TilePos::new(20, y))
            .collect();
        routed_obs.known_rock = routed_obs.known_peaks.clone();
        let mut routed = DefenseThinkContext::new(&policy, &routed_obs, &routed_map, &[], &[]);
        assert!(
            routed
                .reinforcement_travel_cost(
                    &routed_obs.my_units[0],
                    BuildingKind::Turret,
                    LEFT_HOME.offset(4, 0),
                )
                .is_some(),
            "opening a terrain-valid route makes the same fighter available"
        );
    }

    #[test]
    fn public_terrain_routes_defense_builders_around_quarantined_gaps() {
        let dangerous_gap = TilePos::new(20, 4);
        let safe_gap = TilePos::new(20, 22);
        let scenario = scenario_with(|tile| {
            if tile.x == 20 && tile != dangerous_gap && tile != safe_gap {
                '~'
            } else {
                '.'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("two-gap briefing");
        let anchor = TilePos::new(28, 10);
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.my_units = vec![
            unit(1, PlayerId(0), UnitKind::Harvester, TilePos::new(8, 10)),
            unit(2, PlayerId(0), UnitKind::Harvester, TilePos::new(8, 22)),
        ];
        obs.visible.fill(false);
        obs.explored.fill(false);
        let anchor_index = (anchor.y * WIDTH + anchor.x) as usize;
        obs.visible[anchor_index] = true;
        obs.explored[anchor_index] = true;

        let policy = UtilityPolicy {
            contested_harvest_regions: vec![ContestedHarvestRegion {
                center: dangerous_gap,
                last_evidence: obs.tick,
                sweep_started_at: None,
            }],
            ..UtilityPolicy::new()
        };
        let danger = policy.harvest_danger_projection(&obs, None, None);

        let mut observation_only = vec![&obs.my_units[0]];
        assert_eq!(
            policy.safe_implicit_builder(
                &obs,
                BuildingKind::Turret,
                anchor,
                &mut observation_only,
                &danger,
                None,
            ),
            Some(UnitId(1)),
            "unexplored Pit terrain makes the direct observation route look safe"
        );

        let mut unsafe_only = vec![&obs.my_units[0]];
        assert_eq!(
            policy.safe_implicit_builder(
                &obs,
                BuildingKind::Turret,
                anchor,
                &mut unsafe_only,
                &danger,
                Some(&map),
            ),
            None,
            "the public wall forces this worker through the quarantined upper gap"
        );

        let mut builders: Vec<_> = obs.my_units.iter().collect();
        assert_eq!(
            policy.safe_implicit_builder(
                &obs,
                BuildingKind::Turret,
                anchor,
                &mut builders,
                &danger,
                Some(&map),
            ),
            Some(UnitId(2)),
            "a farther worker with a safe public-terrain route must remain eligible"
        );

        let clear_policy = UtilityPolicy::new();
        let clear_danger = clear_policy.harvest_danger_projection(&obs, None, None);
        let mut clear_builders: Vec<_> = obs.my_units.iter().collect();
        assert_eq!(
            clear_policy.safe_implicit_builder(
                &obs,
                BuildingKind::Turret,
                anchor,
                &mut clear_builders,
                &clear_danger,
                Some(&map),
            ),
            Some(UnitId(1)),
            "authored terrain may reroute an ordinary safe build without rejecting it"
        );
    }

    #[test]
    fn a_second_turret_adds_coverage_instead_of_reusing_the_first_anchor() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.my_buildings.push(building(
            2,
            PlayerId(0),
            BuildingKind::Extractor,
            LEFT_HOME.offset(7, -4),
        ));
        obs.my_queues.push(Vec::new());
        let first =
            site(&UtilityPolicy::new(), &obs, &map, &[], &[]).expect("first defensive site");
        obs.my_buildings
            .push(building(3, PlayerId(0), BuildingKind::Turret, first));
        obs.my_queues.push(Vec::new());
        let second = site(&UtilityPolicy::new(), &obs, &map, &[], &[])
            .expect("another exposed lane remains");

        assert_ne!(first, second);
        assert!(
            first.chebyshev(second) > 1,
            "marginal coverage should not stack the second site on the first: {first} then {second}"
        );
    }

    #[test]
    fn existing_defense_changes_new_coverage_and_reverses_the_preferred_site() {
        let map = briefing();
        let obs = observation(PlayerId(0), LEFT_HOME);
        let asset = DefendedAsset {
            value: 10,
            shape: AssetShape::Building {
                anchor: LEFT_HOME,
                size: BuildingKind::Foundry.base_stats().size,
            },
            access: None,
        };
        let approach = Approach {
            asset: 0,
            source: ThreatOrigin {
                anchor: TilePos::new(30, 10),
                size: None,
                capability: ThreatCapability::Foothold,
                tie: 1,
            },
            goal: TilePos::new(6, 10),
            path: (6..=30).rev().map(|x| TilePos::new(x, 10)).collect(),
            baseline_cost: 240,
            disrupted: false,
        };
        let broad = TilePos::new(15, 10);
        let edge = TilePos::new(6, 10);
        let candidate = |anchor, coverage| Candidate {
            anchor,
            builder_travel: 0,
            coverage,
            threat_distance: 0,
        };
        let turret = DefenseProfile::for_kind(BuildingKind::Turret).expect("Turret profile");
        let broad_alone = scored_coverage!(
            &obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[],
            &[],
            turret,
            broad,
        );
        let edge_alone = scored_coverage!(
            &obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[],
            &[],
            turret,
            edge,
        );
        assert_eq!(broad_alone.new, edge_alone.new);
        assert!(broad_alone.interception > edge_alone.interception);
        assert!(
            candidate(broad, broad_alone).key(turret) > candidate(edge, edge_alone).key(turret)
        );

        let existing = building(30, PlayerId(0), BuildingKind::Turret, TilePos::new(15, 10));
        let broad_reinforced = scored_coverage!(
            &obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[&existing],
            &[],
            turret,
            broad,
        );
        let edge_extended = scored_coverage!(
            &obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[&existing],
            &[],
            turret,
            edge,
        );
        assert!(edge_extended.new > broad_reinforced.new);
        assert!(broad_reinforced.reinforced > edge_extended.reinforced);
        assert!(
            candidate(edge, edge_extended).key(turret)
                > candidate(broad, broad_reinforced).key(turret),
            "extending uncovered fire must outrank piling onto an existing Turret envelope"
        );
    }

    #[test]
    fn repeated_tiles_on_a_minor_lane_cannot_outvote_one_critical_asset() {
        let map = briefing();
        let obs = observation(PlayerId(0), LEFT_HOME);
        let assets = [
            DefendedAsset {
                value: 16,
                shape: AssetShape::Building {
                    anchor: LEFT_HOME,
                    size: BuildingKind::Foundry.base_stats().size,
                },
                access: None,
            },
            DefendedAsset {
                value: 1,
                shape: AssetShape::Building {
                    anchor: TilePos::new(20, 10),
                    size: (1, 1),
                },
                access: None,
            },
        ];
        let critical_tile = TilePos::new(5, 5);
        let approaches = [
            Approach {
                asset: 0,
                source: ThreatOrigin {
                    anchor: critical_tile,
                    size: None,
                    capability: ThreatCapability::Foothold,
                    tie: 1,
                },
                goal: critical_tile,
                path: vec![critical_tile],
                baseline_cost: 0,
                disrupted: false,
            },
            Approach {
                asset: 1,
                source: ThreatOrigin {
                    anchor: TilePos::new(24, 10),
                    size: None,
                    capability: ThreatCapability::Foothold,
                    tie: 2,
                },
                goal: TilePos::new(16, 10),
                path: (16..=24).rev().map(|x| TilePos::new(x, 10)).collect(),
                baseline_cost: 80,
                disrupted: false,
            },
        ];
        let turret = DefenseProfile::for_kind(BuildingKind::Turret).expect("Turret profile");
        let critical = scored_coverage!(
            &obs,
            &map,
            &assets,
            &approaches,
            &[],
            &[],
            turret,
            critical_tile,
        );
        let minor = scored_coverage!(
            &obs,
            &map,
            &assets,
            &approaches,
            &[],
            &[],
            turret,
            TilePos::new(20, 10),
        );

        assert_eq!(critical.new, 16);
        assert_eq!(minor.new, 1);
        assert!(
            Candidate {
                anchor: critical_tile,
                builder_travel: 0,
                coverage: critical,
                threat_distance: 0,
            }
            .key(turret)
                > Candidate {
                    anchor: TilePos::new(20, 10),
                    builder_travel: 0,
                    coverage: minor,
                    threat_distance: 0,
                }
                .key(turret),
            "one Foundry must outweigh any number of repeated tiles on a value-one lane"
        );
    }

    #[test]
    fn unfinished_emplacements_do_not_claim_live_coverage() {
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let mut turret = building(30, PlayerId(0), BuildingKind::Turret, TilePos::new(15, 10));
        turret.built = false;
        let mut bastion = building(31, PlayerId(0), BuildingKind::Bastion, TilePos::new(15, 14));
        bastion.built = false;
        obs.my_buildings.extend([turret, bastion]);

        assert!(
            existing_defenses(&obs, DefenseDomain::Ground).is_empty(),
            "paid construction sites cannot cover an approach before their weapons can fire"
        );

        for building in obs
            .my_buildings
            .iter_mut()
            .filter(|building| [BuildingId(30), BuildingId(31)].contains(&building.id))
        {
            building.built = true;
        }
        let live: Vec<_> = existing_defenses(&obs, DefenseDomain::Ground)
            .into_iter()
            .map(|building| building.kind)
            .collect();
        assert_eq!(live, vec![BuildingKind::Turret, BuildingKind::Bastion]);
    }

    #[test]
    fn completed_allied_defenses_contribute_live_coverage() {
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let ally = building(30, PlayerId(2), BuildingKind::Turret, TilePos::new(15, 10));
        obs.ally_buildings.push(ally);

        let live = existing_defenses(&obs, DefenseDomain::Ground);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].player, PlayerId(2));
        assert_eq!(live[0].kind, BuildingKind::Turret);
    }

    #[test]
    fn upgrading_defenses_reserve_the_committed_tier_envelope() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let mut upgrade = building(30, PlayerId(0), BuildingKind::Turret, TilePos::new(15, 10));
        upgrade.built = false;
        upgrade.tier = 1;
        obs.my_buildings.push(upgrade);
        let planned = planned_defenses(&obs, DefenseDomain::Ground);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].tier, 1);

        let tier_one_edge = TilePos::new(21, 10);
        assert!(planned_defense_covers(
            &obs,
            &map,
            &planned[0],
            tier_one_edge
        ));
        let base_envelope = PlannedDefense {
            tier: 0,
            ..planned[0]
        };
        assert!(
            !planned_defense_covers(&obs, &map, &base_envelope, tier_one_edge),
            "the regression point must lie outside base range but inside the committed upgrade"
        );
    }

    #[test]
    fn unfinished_and_promised_envelopes_reserve_space_without_claiming_live_fire() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let mut unfinished = building(30, PlayerId(0), BuildingKind::Turret, TilePos::new(15, 10));
        unfinished.built = false;
        obs.my_buildings.push(unfinished);
        obs.my_units[0].founding = Some((BuildingKind::Turret, TilePos::new(15, 14)));

        assert!(existing_defenses(&obs, DefenseDomain::Ground).is_empty());
        let planned = planned_defenses(&obs, DefenseDomain::Ground);
        assert_eq!(
            planned
                .iter()
                .map(|defense| defense.anchor)
                .collect::<Vec<_>>(),
            vec![TilePos::new(15, 10), TilePos::new(15, 14)]
        );

        let asset = DefendedAsset {
            value: 16,
            shape: AssetShape::Building {
                anchor: LEFT_HOME,
                size: BuildingKind::Foundry.base_stats().size,
            },
            access: None,
        };
        let approach = Approach {
            asset: 0,
            source: ThreatOrigin {
                anchor: TilePos::new(30, 10),
                size: None,
                capability: ThreatCapability::Foothold,
                tie: 1,
            },
            goal: TilePos::new(6, 10),
            path: (6..=30).rev().map(|x| TilePos::new(x, 10)).collect(),
            baseline_cost: 240,
            disrupted: false,
        };
        let turret = DefenseProfile::for_kind(BuildingKind::Turret).expect("Turret profile");
        let overlapping = scored_coverage!(
            &obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[],
            &planned,
            turret,
            TilePos::new(14, 10),
        );
        let extending = scored_coverage!(
            &obs,
            &map,
            std::slice::from_ref(&asset),
            std::slice::from_ref(&approach),
            &[],
            &planned,
            turret,
            TilePos::new(7, 10),
        );

        assert_eq!(overlapping.new, asset.value);
        assert_eq!(extending.new, asset.value);
        assert!(overlapping.planned_overlap > extending.planned_overlap);
        assert!(
            Candidate {
                anchor: TilePos::new(7, 10),
                builder_travel: 0,
                coverage: extending,
                threat_distance: 0,
            }
            .key(turret)
                > Candidate {
                    anchor: TilePos::new(14, 10),
                    builder_travel: 0,
                    coverage: overlapping,
                    threat_distance: 0,
                }
                .key(turret)
        );
    }

    #[test]
    fn unreserved_marginal_coverage_outranks_larger_fully_promised_coverage() {
        let turret = DefenseProfile::for_kind(BuildingKind::Turret).expect("Turret profile");
        let coverage = |new, unplanned_new, planned_overlap| Coverage {
            new,
            reinforced: 0,
            unplanned_new,
            unplanned_reinforced: 0,
            interception: 32,
            protected_value: new,
            planned_overlap,
            blind_exposure: 0,
            spotted_reach: 0,
            redundant: 0,
            lateral: 1,
        };
        let promised = Candidate {
            anchor: TilePos::new(14, 10),
            builder_travel: 0,
            coverage: coverage(16, 0, 16),
            threat_distance: 0,
        };
        let useful = Candidate {
            anchor: TilePos::new(7, 10),
            builder_travel: 0,
            coverage: coverage(6, 6, 0),
            threat_distance: 0,
        };

        assert!(
            useful.key(turret) > promised.key(turret),
            "a paid construction claim must not hide a lower-total site with real marginal coverage"
        );
    }

    #[test]
    fn a_promised_barricade_changes_the_next_route_and_site() {
        let scenario = scenario_with(barricade_lane);
        let map = PublicMapBriefing::from_scenario(&scenario).expect("broad lane briefing");
        let known_peaks: Vec<_> = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| barricade_lane(*tile) == '^')
            .collect();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.my_units[0].tile = LEFT_HOME.offset(3, 1);
        obs.known_rock = known_peaks.clone();
        obs.known_peaks = known_peaks;
        let policy = UtilityPolicy::new();
        let first = site_for(BuildingKind::Barricade, &policy, &obs, &map, &[], &[])
            .expect("first lane-shaping wall");

        obs.my_units[0].founding = Some((BuildingKind::Barricade, first));
        obs.my_units.push(unit(
            2,
            PlayerId(0),
            UnitKind::Harvester,
            LEFT_HOME.offset(3, 0),
        ));
        let starts = policy.uncleared_hostile_starts(&map, obs.me);
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let assets = defended_assets(&policy, &obs, &ground);
        let origins =
            threat_origin_tiers(&obs, &[], &[], &starts, DefenseDomain::Ground)[3].clone();
        let projected = approaches(&ground, &origins, &assets, None, DefenseDomain::Ground);
        assert!(
            projected
                .iter()
                .all(|approach| !approach.path.contains(&first)),
            "a deferred wall must block route projection before its site exists"
        );

        let second = site_for(BuildingKind::Barricade, &policy, &obs, &map, &[], &[])
            .expect("the remaining lane admits a distinct wall");
        assert_ne!(second, first);
        assert!(
            second.chebyshev(first) > 1,
            "planned-overlap scoring must not stack deferred Barricades: {first:?}, {second:?}"
        );
    }

    #[test]
    fn every_defensive_role_chooses_exactly_half_turned_world_sites() {
        let map = briefing();
        for kind in [
            BuildingKind::Turret,
            BuildingKind::Bastion,
            BuildingKind::FlakTurret,
            BuildingKind::ScuttleCharge,
        ] {
            let left = observation(PlayerId(0), LEFT_HOME);
            let left_site = site_for(kind, &UtilityPolicy::new(), &left, &map, &[], &[])
                .unwrap_or_else(|| panic!("left {kind:?} site"));

            let right = observation(PlayerId(1), RIGHT_HOME);
            let orientation = Orientation::for_home(&right, RIGHT_HOME);
            let oriented_obs = orientation.observe(&right);
            let oriented_map = orientation.briefing(&map);
            let oriented_site = site_for(
                kind,
                &UtilityPolicy::new(),
                &oriented_obs,
                &oriented_map,
                &[],
                &[],
            )
            .unwrap_or_else(|| panic!("right oriented {kind:?} site"));
            let size = kind.base_stats().size;
            let right_site = orientation.anchor(oriented_site, size);

            assert_eq!(
                right_site,
                TilePos::new(WIDTH - size.0 - left_site.x, left_site.y),
                "{kind:?} must preserve its whole footprint under a map half-turn",
            );
        }
    }

    fn assert_open_map_roles_are_symmetric(first: TilePos, second: TilePos) {
        let scenario = scenario_with_starts(first, second, |_| '.');
        let map = PublicMapBriefing::from_scenario(&scenario).expect("symmetric open briefing");
        let mut first_obs = observation(PlayerId(0), first);
        first_obs.my_units[0].tile = first.offset(3, 3);
        let mut second_obs = observation(PlayerId(1), second);
        let orientation = Orientation::for_home(&second_obs, second);
        second_obs.my_units[0].tile = orientation.tile(first_obs.my_units[0].tile);

        for kind in [
            BuildingKind::Turret,
            BuildingKind::Bastion,
            BuildingKind::FlakTurret,
            BuildingKind::ScuttleCharge,
        ] {
            let first_site = site_for(kind, &UtilityPolicy::new(), &first_obs, &map, &[], &[])
                .unwrap_or_else(|| panic!("first {kind:?} site"));
            let oriented_site = site_for(
                kind,
                &UtilityPolicy::new(),
                &orientation.observe(&second_obs),
                &orientation.briefing(&map),
                &[],
                &[],
            )
            .unwrap_or_else(|| panic!("second oriented {kind:?} site"));
            let second_site = orientation.anchor(oriented_site, kind.base_stats().size);

            assert_eq!(
                second_site,
                orientation.anchor(first_site, kind.base_stats().size),
                "{kind:?} placement must preserve its whole footprint under {orientation:?}",
            );
        }
    }

    #[test]
    fn defensive_roles_preserve_y_only_and_full_turn_symmetry() {
        assert_open_map_roles_are_symmetric(TilePos::new(18, 3), TilePos::new(18, 19));
        assert_open_map_roles_are_symmetric(TilePos::new(4, 3), TilePos::new(34, 19));
    }

    #[test]
    fn barricade_choke_placement_is_half_turn_symmetric() {
        let scenario = scenario_with(barricade_lane);
        let map = PublicMapBriefing::from_scenario(&scenario).expect("broad lane briefing");
        let known_peaks: Vec<_> = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| barricade_lane(*tile) == '^')
            .collect();

        let mut left = observation(PlayerId(0), LEFT_HOME);
        left.my_units[0].tile = LEFT_HOME.offset(3, 1);
        left.known_rock = known_peaks.clone();
        left.known_peaks = known_peaks.clone();
        let left_site = site_for(
            BuildingKind::Barricade,
            &UtilityPolicy::new(),
            &left,
            &map,
            &[],
            &[],
        )
        .expect("left Barricade site");

        let mut right = observation(PlayerId(1), RIGHT_HOME);
        right.my_units[0].tile =
            TilePos::new(WIDTH - 1 - left.my_units[0].tile.x, left.my_units[0].tile.y);
        right.known_rock = known_peaks.clone();
        right.known_peaks = known_peaks;
        let orientation = Orientation::for_home(&right, RIGHT_HOME);
        let oriented_site = site_for(
            BuildingKind::Barricade,
            &UtilityPolicy::new(),
            &orientation.observe(&right),
            &orientation.briefing(&map),
            &[],
            &[],
        )
        .expect("right oriented Barricade site");
        let right_site =
            orientation.anchor(oriented_site, BuildingKind::Barricade.base_stats().size);

        assert_eq!(
            right_site,
            TilePos::new(WIDTH - 1 - left_site.x, left_site.y)
        );
    }

    fn assert_barricade_orientation_symmetry(
        scenario: Scenario,
        first_home: TilePos,
        second_home: TilePos,
        first_worker: TilePos,
    ) {
        let map = PublicMapBriefing::from_scenario(&scenario).expect("Barricade lane briefing");
        let known_peaks: Vec<_> = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| map.terrain_at(*tile).is_some_and(Terrain::blocks_ground))
            .collect();
        let mut first = observation(PlayerId(0), first_home);
        first.my_units[0].tile = first_worker;
        first.known_rock = known_peaks.clone();
        first.known_peaks = known_peaks.clone();
        let first_site = site_for(
            BuildingKind::Barricade,
            &UtilityPolicy::new(),
            &first,
            &map,
            &[],
            &[],
        )
        .expect("first Barricade site");

        let mut second = observation(PlayerId(1), second_home);
        let orientation = Orientation::for_home(&second, second_home);
        second.my_units[0].tile = orientation.tile(first_worker);
        second.known_rock = known_peaks.clone();
        second.known_peaks = known_peaks;
        let oriented_site = site_for(
            BuildingKind::Barricade,
            &UtilityPolicy::new(),
            &orientation.observe(&second),
            &orientation.briefing(&map),
            &[],
            &[],
        )
        .expect("second oriented Barricade site");
        let second_site =
            orientation.anchor(oriented_site, BuildingKind::Barricade.base_stats().size);

        assert_eq!(
            second_site,
            orientation.anchor(first_site, BuildingKind::Barricade.base_stats().size,)
        );
    }

    #[test]
    fn barricades_preserve_y_only_and_full_turn_symmetry() {
        let top = TilePos::new(19, 3);
        let bottom = TilePos::new(19, 19);
        assert_barricade_orientation_symmetry(
            scenario_with_starts(top, bottom, vertical_barricade_lane),
            top,
            bottom,
            TilePos::new(19, 6),
        );

        let northwest = TilePos::new(4, 3);
        let southeast = TilePos::new(34, 19);
        assert_barricade_orientation_symmetry(
            scenario_with_starts(northwest, southeast, full_turn_barricade_lane),
            northwest,
            southeast,
            TilePos::new(7, 5),
        );
    }

    #[test]
    fn building_value_counts_tech_queues_and_active_sites_but_not_orphans() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let mut airworks = building(
            2,
            PlayerId(0),
            BuildingKind::Airworks,
            LEFT_HOME.offset(0, -5),
        );
        airworks.tier = 0;
        let mut orphan = building(
            3,
            PlayerId(0),
            BuildingKind::Fabricator,
            LEFT_HOME.offset(5, 5),
        );
        orphan.built = false;
        let extractor_anchor = LEFT_HOME.offset(7, 0);
        let mut active_site = building(4, PlayerId(0), BuildingKind::Extractor, extractor_anchor);
        active_site.built = false;
        obs.my_buildings.extend([
            airworks,
            building(
                5,
                PlayerId(0),
                BuildingKind::Crucible,
                LEFT_HOME.offset(5, -5),
            ),
            orphan,
            active_site,
        ]);
        obs.my_queues
            .extend([vec![UnitKind::Condor], Vec::new(), Vec::new(), Vec::new()]);
        obs.my_units[0].site = Some(BuildingId(4));
        let starts: Vec<_> = map.hostile_starting_foundries(obs.me).copied().collect();
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let assets = defended_assets(&UtilityPolicy::new(), &obs, &ground);
        let value_at = |anchor| {
            assets.iter().find_map(|asset| match asset.shape {
                AssetShape::Building {
                    anchor: candidate, ..
                } if candidate == anchor => Some(asset.value),
                _ => None,
            })
        };

        assert_eq!(value_at(LEFT_HOME), Some(16));
        assert_eq!(value_at(LEFT_HOME.offset(0, -5)), Some(13));
        assert_eq!(value_at(LEFT_HOME.offset(5, -5)), Some(10));
        assert_eq!(value_at(LEFT_HOME.offset(5, 5)), None);
        assert_eq!(value_at(extractor_anchor), Some(4));
    }

    #[test]
    fn explored_depletion_overrides_authored_scrap_while_dark_ground_keeps_the_prior() {
        let scrap = TilePos::new(12, 10);
        let scenario = scenario_with(|tile| if tile == scrap { 'S' } else { '.' });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("scrap briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.explored[(scrap.y * WIDTH + scrap.x) as usize] = false;
        let starts: Vec<_> = map.hostile_starting_foundries(obs.me).copied().collect();
        let dark = GroundKnowledge::new(&obs, &map, &starts);
        assert_eq!(
            dark.scrap.get(&scrap),
            Some(&crate::stats::RICH_SCRAP_NODE_AMOUNT)
        );

        obs.explored[(scrap.y * WIDTH + scrap.x) as usize] = true;
        let depleted = GroundKnowledge::new(&obs, &map, &starts);
        assert!(!depleted.scrap.contains_key(&scrap));

        obs.known_scrap.push((scrap, 175));
        let live = GroundKnowledge::new(&obs, &map, &starts);
        assert_eq!(live.scrap.get(&scrap), Some(&175));
    }

    #[test]
    fn reachable_scrap_near_owned_foundries_is_defensible_on_every_front() {
        let rear = TilePos::new(2, 10);
        let front = TilePos::new(12, 10);
        let enemy_side = TilePos::new(31, 10);
        let scenario = scenario_with(|tile| {
            if [rear, front, enemy_side].contains(&tile) {
                's'
            } else {
                '.'
            }
        });
        let map = PublicMapBriefing::from_scenario(&scenario).expect("scrap briefing");
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        let expansion = TilePos::new(24, 10);
        obs.my_buildings
            .push(building(2, PlayerId(0), BuildingKind::Foundry, expansion));
        obs.my_queues.push(Vec::new());
        for (id, tile, target) in [
            (2, rear.offset(-1, 0), rear),
            (3, front.offset(-1, 0), front),
            (4, enemy_side.offset(-1, 0), enemy_side),
        ] {
            let mut worker = unit(id, PlayerId(0), UnitKind::Harvester, tile);
            worker.idle = false;
            worker.harvesting = Some(target);
            obs.my_units.push(worker);
        }
        obs.explored.fill(false);
        let starts: Vec<_> = map.hostile_starting_foundries(obs.me).copied().collect();
        let ground = GroundKnowledge::new(&obs, &map, &starts);
        let assets = scrap_assets(&UtilityPolicy::new(), &ground, &[LEFT_HOME, expansion]);
        let defended: BTreeSet<_> = assets
            .iter()
            .flat_map(|asset| match &asset.shape {
                AssetShape::Scrap { tiles, .. } => tiles.iter().copied(),
                AssetShape::Building { .. } => [].iter().copied(),
            })
            .collect();

        assert!(defended.contains(&rear));
        assert!(defended.contains(&front));
        assert!(
            defended.contains(&enemy_side),
            "an exposed cluster remains worth protecting when an owned expansion can serve it"
        );
    }

    #[test]
    fn visible_foothold_beats_the_authored_start_without_becoming_a_target_id() {
        let map = briefing();
        let mut obs = observation(PlayerId(0), LEFT_HOME);
        obs.enemy_buildings.push(building(
            20,
            PlayerId(1),
            BuildingKind::Fabricator,
            TilePos::new(4, 1),
        ));

        let selected = site(&UtilityPolicy::new(), &obs, &map, &[], &[])
            .expect("the visible northern foothold admits a safe site");
        assert!(selected.y < LEFT_HOME.y);
    }

    #[test]
    fn direct_fire_coverage_respects_rock_and_worker_danger_can_refuse_every_site() {
        let rock = TilePos::new(7, 10);
        let blocked_scenario = scenario_with(|tile| if tile == rock { '#' } else { '.' });
        let blocked = PublicMapBriefing::from_scenario(&blocked_scenario).expect("rock briefing");
        let open = briefing();
        let weapon = &BuildingKind::Turret.base_stats().weapons[0];
        let shooter = TilePos::new(4, 10).center();
        let target = TilePos::new(9, 10);
        assert!(!weapon_covers(
            &blocked,
            shooter,
            weapon,
            target,
            DefenseDomain::Ground,
        ));
        assert!(weapon_covers(
            &open,
            shooter,
            weapon,
            target,
            DefenseDomain::Ground,
        ));

        let mut endangered = observation(PlayerId(0), LEFT_HOME);
        endangered.enemy_units.push(unit(
            20,
            PlayerId(1),
            UnitKind::Sentinel,
            LEFT_HOME.offset(-2, 0),
        ));
        assert_eq!(
            site(&UtilityPolicy::new(), &endangered, &open, &[], &[]),
            None,
            "a geometric firing site is not actionable without a safe construction crew"
        );
    }

    #[test]
    fn weapon_coverage_uses_exact_range_minimum_range_and_terrain_line_of_fire() {
        let open = briefing();
        let turret = &BuildingKind::Turret.base_stats().weapons[0];
        let turret_center = TilePos::new(10, 10).center();
        assert!(weapon_covers(
            &open,
            turret_center,
            turret,
            TilePos::new(15, 10),
            DefenseDomain::Ground,
        ));
        assert!(
            !weapon_covers(
                &open,
                turret_center,
                turret,
                TilePos::new(16, 10),
                DefenseDomain::Ground,
            ),
            "one tile beyond the Turret's exact five-tile reach is uncovered"
        );

        let rock_scenario = scenario_with(|tile| {
            if tile == TilePos::new(13, 10) {
                '#'
            } else {
                '.'
            }
        });
        let rock = PublicMapBriefing::from_scenario(&rock_scenario).expect("rock briefing");
        assert!(!weapon_covers(
            &rock,
            turret_center,
            turret,
            TilePos::new(15, 10),
            DefenseDomain::Ground,
        ));

        let bastion = &BuildingKind::Bastion.base_stats().weapons[0];
        assert!(bastion.indirect);
        let bastion_center = footprint_center(
            TilePos::new(10, 10),
            BuildingKind::Bastion.base_stats().size,
        );
        assert!(
            !weapon_covers(
                &open,
                bastion_center,
                bastion,
                TilePos::new(12, 10),
                DefenseDomain::Ground,
            ),
            "Bastion fire respects its close-range dead zone"
        );
        assert!(
            weapon_covers(
                &rock,
                bastion_center,
                bastion,
                TilePos::new(15, 10),
                DefenseDomain::Ground,
            ),
            "indirect fire crosses ordinary rock"
        );
        let peak_scenario = scenario_with(|tile| {
            if tile == TilePos::new(13, 10) {
                '^'
            } else {
                '.'
            }
        });
        let peak = PublicMapBriefing::from_scenario(&peak_scenario).expect("peak briefing");
        assert!(
            !weapon_covers(
                &peak,
                bastion_center,
                bastion,
                TilePos::new(15, 10),
                DefenseDomain::Ground,
            ),
            "peaks block even indirect fire"
        );

        let flak = &BuildingKind::FlakTurret.base_stats().weapons[0];
        assert!(
            weapon_covers(
                &rock,
                turret_center,
                flak,
                TilePos::new(15, 10),
                DefenseDomain::Air,
            ),
            "air-involved direct fire crosses ordinary ground cover",
        );
        assert!(
            !weapon_covers(
                &peak,
                turret_center,
                flak,
                TilePos::new(15, 10),
                DefenseDomain::Air,
            ),
            "peaks still wall anti-air fire",
        );

        let mine = DefenseProfile::for_kind(BuildingKind::ScuttleCharge).expect("mine profile");
        let mine_anchor = TilePos::new(10, 10);
        assert!(defense_covers(&open, mine, mine_anchor, mine_anchor,));
        assert!(
            !defense_covers(&open, mine, mine_anchor, mine_anchor.offset(1, 0)),
            "a mine scores the trigger tile itself rather than borrowing Turret range",
        );
        assert!(
            DefenseProfile::for_kind(BuildingKind::Array).is_none(),
            "support buildings without a defensive effect do not enter this scorer",
        );
    }
}
