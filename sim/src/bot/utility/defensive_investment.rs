//! Typed voluntary-defense opportunities for cross-domain allocation.

use super::defense::{
    DefenseOpportunityEvidence, DefenseThinkContext, StrategicDefenseQuote, travel_ticks,
    unit_threatens_ground,
};
use super::sensor::StrategicArrayQuote;
use super::{
    BuildingContact, Intent, Observation, PublicMapBriefing, ResolvedProfile, UnitContact, UnitObs,
    UtilityPolicy,
};
use crate::bot::Orientation;
use crate::bot::allocation::{
    Confidence, ExecutionSafety, ProposalCase, StrategicValue, TimeToImpact, Urgency,
};
use crate::bot::resources::{ResourceSnapshot, SiteFootprint};
use crate::ids::UnitId;
use crate::stats::{BuildingKind, Domain};
use chassis::Tick;
use chassis::grid::TilePos;
use std::cmp::Reverse;

/// Closed set of construction roles owned by voluntary defense allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::bot) enum DefenseConstruction {
    Turret,
    Bastion,
    FlakTurret,
    ScuttleCharge,
    Barricade,
    Array,
}

impl DefenseConstruction {
    const ALL: [Self; 6] = [
        Self::Turret,
        Self::Bastion,
        Self::FlakTurret,
        Self::ScuttleCharge,
        Self::Barricade,
        Self::Array,
    ];

    pub(in crate::bot) const fn kind(self) -> BuildingKind {
        match self {
            Self::Turret => BuildingKind::Turret,
            Self::Bastion => BuildingKind::Bastion,
            Self::FlakTurret => BuildingKind::FlakTurret,
            Self::ScuttleCharge => BuildingKind::ScuttleCharge,
            Self::Barricade => BuildingKind::Barricade,
            Self::Array => BuildingKind::Array,
        }
    }

    fn personality_emphasis(self, profile: &ResolvedProfile) -> u16 {
        let traits = profile.traits;
        match self {
            Self::Turret | Self::Bastion | Self::Barricade => u16::from(traits.fortification),
            Self::FlakTurret => (u16::from(traits.fortification) + u16::from(traits.support)) / 2,
            Self::ScuttleCharge | Self::Array => {
                (u16::from(traits.fortification) + u16::from(traits.guile)) / 2
            }
        }
    }
}

/// One exact, independently useful static-defense alternative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) struct FreshDefenseProposal {
    construction: DefenseConstruction,
    anchor: TilePos,
    builder: UnitId,
    construction_capital: u32,
    site: SiteFootprint,
    case: ProposalCase,
    minimum_residual_scrap: u32,
    personality_emphasis: u16,
    ready_at: Tick,
    evidence: DefenseOpportunityEvidence,
    marginal_value: u32,
}

impl FreshDefenseProposal {
    pub(in crate::bot) const fn kind(&self) -> BuildingKind {
        self.construction.kind()
    }

    pub(in crate::bot) const fn anchor(&self) -> TilePos {
        self.anchor
    }

    pub(in crate::bot) const fn builder(&self) -> UnitId {
        self.builder
    }

    pub(in crate::bot) const fn construction_capital(&self) -> u32 {
        self.construction_capital
    }

    pub(in crate::bot) const fn site(&self) -> SiteFootprint {
        self.site
    }

    pub(in crate::bot) const fn case(&self) -> ProposalCase {
        self.case
    }

    pub(in crate::bot) const fn minimum_residual_scrap(&self) -> u32 {
        self.minimum_residual_scrap
    }

    pub(in crate::bot) const fn ready_at(&self) -> Tick {
        self.ready_at
    }

    #[cfg(test)]
    pub(in crate::bot) fn fixture(
        construction: DefenseConstruction,
        anchor: TilePos,
        builder: UnitId,
        case: ProposalCase,
        personality_emphasis: u16,
        minimum_residual_scrap: u32,
    ) -> Self {
        let kind = construction.kind();
        let stats = kind
            .base_stats()
            .construction
            .expect("a defense fixture must name a constructible kind");
        Self {
            construction,
            anchor,
            builder,
            construction_capital: stats.cost,
            site: SiteFootprint::new(anchor, kind.base_stats().size)
                .expect("building footprints are positive"),
            case,
            minimum_residual_scrap,
            personality_emphasis,
            ready_at: 0,
            evidence: DefenseOpportunityEvidence::PublicPrior,
            marginal_value: 1,
        }
    }
}

impl UtilityPolicy {
    /// Derives at most one exact opportunity for each voluntary defense role
    /// from the shared immutable resource and observation boundary.
    #[expect(
        clippy::too_many_arguments,
        reason = "one shared allocation evidence boundary"
    )]
    pub(in crate::bot) fn fresh_defense_proposals(
        &self,
        profile: &ResolvedProfile,
        obs: &Observation,
        resources: &ResourceSnapshot,
        briefing: &PublicMapBriefing,
        orientation: Orientation,
        home: TilePos,
        unit_contacts: &[UnitContact],
        building_contacts: &[BuildingContact],
        builders: &[&UnitObs],
        unavailable_reinforcements: &[UnitId],
        minimum_residual_scrap: u32,
        committed_current_scrap: u32,
    ) -> Vec<FreshDefenseProposal> {
        debug_assert_eq!(resources.forecast().observed_at(), obs.tick);
        let construction_builders = self.construction_builders(obs, &[], &[]);
        let owned_builders: Vec<_> = builders
            .iter()
            .copied()
            .filter(|builder| {
                construction_builders
                    .iter()
                    .any(|candidate| candidate.id == builder.id)
                    && !self.evacuating_workers.contains(&builder.id)
                    && self
                        .retreating_contested_scout
                        .is_none_or(|retreat| retreat.unit != builder.id)
                    && resources
                        .builders()
                        .binary_search_by_key(&builder.id, |resource| resource.id)
                        .is_ok()
            })
            .collect();
        if owned_builders.is_empty() {
            return Vec::new();
        }

        let mut proposals = Vec::with_capacity(DefenseConstruction::ALL.len());
        let mut prepared = None;
        for construction in DefenseConstruction::ALL {
            let kind = construction.kind();
            let Some(construction_stats) = kind.base_stats().construction else {
                continue;
            };
            if resources
                .current_scrap()
                .amount()
                .saturating_sub(committed_current_scrap)
                < construction_stats
                    .cost
                    .saturating_add(minimum_residual_scrap)
            {
                continue;
            }
            if !construction_prerequisites_met(obs, kind) {
                continue;
            }
            if construction == DefenseConstruction::FlakTurret
                && !confirmed_air_threat(obs, unit_contacts, building_contacts)
            {
                continue;
            }
            let (context, eligible_builders) = prepared.get_or_insert_with(|| {
                let context = DefenseThinkContext::new_oriented(
                    self,
                    obs,
                    briefing,
                    unit_contacts,
                    building_contacts,
                    orientation,
                );
                let eligible_builders = owned_builders
                    .iter()
                    .copied()
                    .filter(|builder| {
                        !self.harvest_location_contested(builder.tile)
                            && context.worker_start_is_safe(builder.tile)
                    })
                    .collect::<Vec<_>>();
                (context, eligible_builders)
            });
            if eligible_builders.is_empty() {
                return Vec::new();
            }
            let proposal = if construction == DefenseConstruction::Array {
                self.strategic_array_quote_in_context(home, eligible_builders, context)
                    .and_then(|quote| {
                        array_proposal(
                            construction,
                            quote,
                            profile,
                            obs,
                            eligible_builders,
                            minimum_residual_scrap,
                        )
                    })
            } else {
                self.strategic_defense_quote_in_context(kind, eligible_builders, context)
                    .and_then(|quote| {
                        let reinforcement_ticks = mobile_reinforcement_ticks(
                            obs,
                            context,
                            kind,
                            quote.placement.anchor,
                            unavailable_reinforcements,
                        );
                        defense_proposal(
                            construction,
                            quote,
                            profile,
                            obs,
                            eligible_builders,
                            minimum_residual_scrap,
                            reinforcement_ticks,
                        )
                    })
            };
            if let Some(proposal) = proposal {
                proposals.push(proposal);
            }
        }
        proposals.sort_unstable_by_key(proposal_preference_key);
        proposals
    }

    /// Emits the allocator-selected exact kind, site, and builder without a
    /// second observation or placement pass.
    pub(in crate::bot) fn commit_adjudicated_defense(
        &mut self,
        proposal: FreshDefenseProposal,
        intents: &mut Vec<Intent>,
    ) {
        let kind = proposal.kind();
        Self::insert_build_before_harvest(
            intents,
            kind,
            proposal.anchor,
            Intent::BuildWith {
                builder: proposal.builder,
                kind,
                anchor: proposal.anchor,
            },
        );
    }
}

fn defense_proposal(
    construction: DefenseConstruction,
    quote: StrategicDefenseQuote,
    profile: &ResolvedProfile,
    obs: &Observation,
    builders: &[&UnitObs],
    minimum_residual_scrap: u32,
    reinforcement_ticks: Option<u64>,
) -> Option<FreshDefenseProposal> {
    let kind = construction.kind();
    let construction_stats = kind.base_stats().construction?;
    let builder = builders
        .iter()
        .find(|builder| builder.id == quote.placement.builder)?;
    let ready_ticks = travel_ticks(quote.builder_travel_cost, builder.kind.stats().speed)
        .saturating_add(u64::from(
            construction_stats
                .build_ticks
                .div_ceil(builder.kind.stats().build_rate.max(1)),
        ));
    let marginal_value = quote
        .uncovered_value
        .saturating_add(quote.reinforced_value.div_ceil(2));
    if marginal_value == 0 {
        return None;
    }
    let mut value = strategic_value(marginal_value);
    if reinforcement_ticks.is_some_and(|ticks| ticks < ready_ticks) {
        value = downgrade_value(value);
    }
    let current_pressure = quote.evidence == DefenseOpportunityEvidence::CurrentArmed;
    let urgency = match quote.evidence {
        DefenseOpportunityEvidence::CurrentArmed
            if reinforcement_ticks
                .zip(quote.threat_arrival_ticks)
                .is_none_or(|(reinforcement, hostile)| reinforcement >= hostile) =>
        {
            Urgency::Pressing
        }
        DefenseOpportunityEvidence::CurrentArmed
        | DefenseOpportunityEvidence::CurrentFoothold
        | DefenseOpportunityEvidence::Remembered => Urgency::Timely,
        DefenseOpportunityEvidence::PublicPrior => Urgency::Developmental,
    };
    let confidence = confidence(quote.evidence, quote.evidence_count);
    let time_to_impact = if current_pressure
        && quote
            .threat_arrival_ticks
            .is_some_and(|arrival| ready_ticks <= arrival)
    {
        TimeToImpact::Immediate
    } else if ready_ticks <= 600 {
        TimeToImpact::Near
    } else {
        TimeToImpact::Patient
    };
    let safety = if quote
        .threat_arrival_ticks
        .is_some_and(|arrival| arrival < ready_ticks)
        || construction == DefenseConstruction::Bastion && quote.blind_exposure > 0
    {
        ExecutionSafety::Managed
    } else {
        ExecutionSafety::Secure
    };
    let case = ProposalCase {
        urgency,
        confidence,
        value,
        time_to_impact,
        safety,
    };
    if weakest_voluntary_weapon_case(case) {
        return None;
    }
    make_proposal(
        construction,
        quote.placement.anchor,
        quote.placement.builder,
        case,
        profile,
        obs.tick.saturating_add(ready_ticks),
        quote.evidence,
        marginal_value,
        minimum_residual_scrap,
    )
}

fn array_proposal(
    construction: DefenseConstruction,
    quote: StrategicArrayQuote,
    profile: &ResolvedProfile,
    obs: &Observation,
    builders: &[&UnitObs],
    minimum_residual_scrap: u32,
) -> Option<FreshDefenseProposal> {
    if quote.novel_radar == 0 {
        return None;
    }
    let kind = construction.kind();
    let construction_stats = kind.base_stats().construction?;
    let builder = builders
        .iter()
        .find(|builder| builder.id == quote.builder)?;
    let ready_ticks = travel_ticks(quote.builder_travel_cost, builder.kind.stats().speed)
        .saturating_add(u64::from(
            construction_stats
                .build_ticks
                .div_ceil(builder.kind.stats().build_rate.max(1)),
        ));
    let value = if quote.novel_radar.saturating_mul(2) >= quote.usable_radar {
        StrategicValue::Material
    } else {
        StrategicValue::Incremental
    };
    let urgency = match quote.evidence {
        DefenseOpportunityEvidence::CurrentArmed
        | DefenseOpportunityEvidence::CurrentFoothold
        | DefenseOpportunityEvidence::Remembered => Urgency::Timely,
        DefenseOpportunityEvidence::PublicPrior => Urgency::Developmental,
    };
    make_proposal(
        construction,
        quote.anchor,
        quote.builder,
        ProposalCase {
            urgency,
            confidence: confidence(quote.evidence, quote.evidence_count),
            value,
            time_to_impact: if ready_ticks <= 600 {
                TimeToImpact::Near
            } else {
                TimeToImpact::Patient
            },
            safety: match quote.evidence {
                DefenseOpportunityEvidence::CurrentArmed
                | DefenseOpportunityEvidence::CurrentFoothold => ExecutionSafety::Managed,
                DefenseOpportunityEvidence::Remembered
                | DefenseOpportunityEvidence::PublicPrior => ExecutionSafety::Secure,
            },
        },
        profile,
        obs.tick.saturating_add(ready_ticks),
        quote.evidence,
        quote.novel_radar,
        minimum_residual_scrap,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "constructs the exact typed proposal"
)]
fn make_proposal(
    construction: DefenseConstruction,
    anchor: TilePos,
    builder: UnitId,
    case: ProposalCase,
    profile: &ResolvedProfile,
    ready_at: Tick,
    evidence: DefenseOpportunityEvidence,
    marginal_value: u32,
    minimum_residual_scrap: u32,
) -> Option<FreshDefenseProposal> {
    let kind = construction.kind();
    let stats = kind.base_stats().construction?;
    Some(FreshDefenseProposal {
        construction,
        anchor,
        builder,
        construction_capital: stats.cost,
        site: SiteFootprint::new(anchor, kind.base_stats().size)?,
        case,
        minimum_residual_scrap,
        personality_emphasis: construction.personality_emphasis(profile),
        ready_at,
        evidence,
        marginal_value,
    })
}

fn construction_prerequisites_met(obs: &Observation, kind: BuildingKind) -> bool {
    kind.base_stats().construction.is_some_and(|construction| {
        construction.requires.iter().all(|required| {
            obs.my_buildings.iter().any(|building| {
                building.player == obs.me
                    && building.kind == *required
                    && building.built
                    && building.hp > 0
            })
        })
    })
}

fn confirmed_air_threat(
    obs: &Observation,
    unit_contacts: &[UnitContact],
    building_contacts: &[BuildingContact],
) -> bool {
    let threatening_airframe = |kind: crate::stats::UnitKind| {
        let stats = kind.stats();
        stats.domain == Domain::Air && (unit_threatens_ground(kind) || stats.transport_capacity > 0)
    };
    obs.enemy_units
        .iter()
        .any(|unit| unit.hp > 0 && threatening_airframe(unit.kind))
        || unit_contacts.iter().any(|contact| {
            contact.hp > 0
                && contact.confidence_at(obs.tick) > 0
                && threatening_airframe(contact.kind)
        })
        || obs.enemy_buildings.iter().any(|building| {
            building.hp > 0
                && building.built
                && building.seen
                && building.kind == BuildingKind::Airworks
        })
        || building_contacts.iter().any(|contact| {
            contact.hp > 0
                && contact.built
                && contact.kind == BuildingKind::Airworks
                && contact.confidence_at(obs.tick) > 0
        })
}

fn confidence(evidence: DefenseOpportunityEvidence, evidence_count: usize) -> Confidence {
    match evidence {
        DefenseOpportunityEvidence::CurrentArmed | DefenseOpportunityEvidence::CurrentFoothold => {
            Confidence::Current
        }
        DefenseOpportunityEvidence::Remembered if evidence_count > 1 => Confidence::Supported,
        DefenseOpportunityEvidence::Remembered | DefenseOpportunityEvidence::PublicPrior => {
            Confidence::Prior
        }
    }
}

fn strategic_value(marginal_value: u32) -> StrategicValue {
    if marginal_value >= 12 {
        StrategicValue::Decisive
    } else if marginal_value >= 6 {
        StrategicValue::Material
    } else {
        StrategicValue::Incremental
    }
}

fn downgrade_value(value: StrategicValue) -> StrategicValue {
    match value {
        StrategicValue::Decisive => StrategicValue::Material,
        StrategicValue::Material | StrategicValue::Incremental => StrategicValue::Incremental,
    }
}

fn weakest_voluntary_weapon_case(case: ProposalCase) -> bool {
    case.urgency == Urgency::Developmental
        && case.confidence == Confidence::Prior
        && case.value == StrategicValue::Incremental
}

fn mobile_reinforcement_ticks(
    obs: &Observation,
    context: &mut DefenseThinkContext<'_>,
    defense: BuildingKind,
    anchor: TilePos,
    unavailable: &[UnitId],
) -> Option<u64> {
    let targets_air = defense == BuildingKind::FlakTurret;
    obs.my_units
        .iter()
        .filter(|unit| unit.hp > 0)
        .filter(|unit| unavailable.binary_search(&unit.id).is_err())
        .filter(|unit| {
            unit.kind.stats().weapons.iter().any(|weapon| {
                if targets_air {
                    weapon.targets.air
                } else {
                    weapon.targets.ground
                }
            })
        })
        .filter_map(|unit| {
            context
                .reinforcement_travel_cost(unit, defense, anchor)
                .map(|path_cost| travel_ticks(path_cost, unit.kind.stats().speed))
        })
        .min()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct ProposalPreferenceKey {
    urgency: Reverse<u8>,
    confidence_and_evidence: Reverse<(u8, u8)>,
    value: Reverse<u8>,
    time_to_impact: Reverse<u8>,
    safety: Reverse<u8>,
    personality: Reverse<u16>,
    marginal_value: Reverse<u32>,
    capital: u32,
    construction: DefenseConstruction,
    anchor_y: i32,
    anchor_x: i32,
    builder: UnitId,
}

fn proposal_preference_key(proposal: &FreshDefenseProposal) -> ProposalPreferenceKey {
    let case = proposal.case;
    ProposalPreferenceKey {
        urgency: Reverse(urgency_rank(case.urgency)),
        confidence_and_evidence: Reverse((
            confidence_rank(case.confidence),
            evidence_rank(proposal.evidence),
        )),
        value: Reverse(value_rank(case.value)),
        time_to_impact: Reverse(time_rank(case.time_to_impact)),
        safety: Reverse(safety_rank(case.safety)),
        personality: Reverse(proposal.personality_emphasis),
        marginal_value: Reverse(proposal.marginal_value),
        capital: proposal.construction_capital,
        construction: proposal.construction,
        anchor_y: proposal.anchor.y,
        anchor_x: proposal.anchor.x,
        builder: proposal.builder,
    }
}

const fn evidence_rank(value: DefenseOpportunityEvidence) -> u8 {
    match value {
        DefenseOpportunityEvidence::PublicPrior => 0,
        DefenseOpportunityEvidence::Remembered => 1,
        DefenseOpportunityEvidence::CurrentFoothold => 2,
        DefenseOpportunityEvidence::CurrentArmed => 3,
    }
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

const fn time_rank(value: TimeToImpact) -> u8 {
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
    use super::super::construction::{
        FoundryConfidence, FoundryExecutionSafety, FoundryOpportunityCase, FoundryStrategicValue,
        FoundryTimeToImpact, FoundryUrgency, FreshFoundryProposal,
    };
    use super::super::{HarvesterWatch, RetreatingContestedScout};
    use super::*;
    use crate::bot::intelligence::ContactEvidence;
    use crate::bot::observation::{BuildingObs, UnitObs};
    use crate::ids::{BuildingId, PlayerId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance, PlayerSpec, Scenario};
    use crate::state::Faction;
    use crate::stats::UnitKind;
    use std::collections::BTreeSet;

    const WIDTH: i32 = 40;
    const HEIGHT: i32 = 24;
    const HOME: TilePos = TilePos::new(4, 10);
    const ENEMY_HOME: TilePos = TilePos::new(34, 10);

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

    fn building(kind: BuildingKind, built: bool) -> BuildingObs {
        BuildingObs {
            id: BuildingId(1),
            player: PlayerId(0),
            kind,
            anchor: TilePos::new(2, 2),
            hp: kind.base_stats().max_hp,
            built,
            seen: true,
            tier: 0,
        }
    }

    fn opportunity_terrain(tile: TilePos) -> char {
        let open_base = tile.x <= 10 || tile.x >= 30;
        let bypass = tile.y == 9 && ((11..=13).contains(&tile.x) || (26..=28).contains(&tile.x));
        let main_lane = (10..=11).contains(&tile.y);
        let bottleneck = tile.y == 11 && matches!(tile.x, 12 | 27);
        if open_base || (main_lane || bypass) && !bottleneck {
            '.'
        } else {
            '^'
        }
    }

    fn opportunity_fixture() -> (Observation, PublicMapBriefing) {
        let mut map = Vec::new();
        for y in 0..HEIGHT {
            let mut row = Vec::new();
            for x in 0..WIDTH {
                let tile = TilePos::new(x, y);
                let authored = if tile == HOME {
                    '1'
                } else if tile == ENEMY_HOME {
                    '2'
                } else {
                    opportunity_terrain(tile)
                };
                row.push(authored as u8);
            }
            map.push(String::from_utf8(row).expect("ASCII fixture row"));
        }
        let scenario = Scenario {
            name: "defense opportunity fixture".into(),
            seed: 7,
            map,
            players: vec![
                PlayerSpec {
                    name: "left".into(),
                    faction: Faction::Ferrous,
                    team: None,
                    scrap: 1_000,
                    bot: true,
                    bot_config: None,
                },
                PlayerSpec {
                    name: "right".into(),
                    faction: Faction::Cupric,
                    team: None,
                    scrap: 1_000,
                    bot: true,
                    bot_config: None,
                },
            ],
            units: Vec::new(),
            buildings: Vec::new(),
            meta: None,
        };
        let briefing = PublicMapBriefing::from_scenario(&scenario).expect("valid public map");
        let known_peaks: Vec<_> = (0..HEIGHT)
            .flat_map(|y| (0..WIDTH).map(move |x| TilePos::new(x, y)))
            .filter(|tile| opportunity_terrain(*tile) == '^')
            .collect();
        let mut foundry = building(BuildingKind::Foundry, true);
        foundry.id = BuildingId(1);
        foundry.anchor = HOME;
        let mut fabricator = building(BuildingKind::Fabricator, true);
        fabricator.id = BuildingId(2);
        fabricator.anchor = TilePos::new(7, 6);
        let observation = Observation {
            tick: 1_000,
            me: PlayerId(0),
            scrap: 1_000,
            map_width: WIDTH,
            map_height: HEIGHT,
            my_units: vec![unit(
                1,
                PlayerId(0),
                UnitKind::Harvester,
                TilePos::new(7, 11),
            )],
            my_buildings: vec![foundry, fabricator],
            my_queues: vec![Vec::new(), Vec::new()],
            enemy_units: vec![
                unit(20, PlayerId(1), UnitKind::Sentinel, TilePos::new(30, 10)),
                unit(21, PlayerId(1), UnitKind::Skyhook, TilePos::new(31, 10)),
            ],
            visible: vec![true; (WIDTH * HEIGHT) as usize],
            explored: vec![true; (WIDTH * HEIGHT) as usize],
            known_rock: known_peaks.clone(),
            known_peaks,
            faction: Faction::Ferrous,
            ..Observation::default()
        };
        (observation, briefing)
    }

    fn low_fortification_profile() -> ResolvedProfile {
        (0..1_024)
            .map(|seed| {
                ResolvedProfile::resolve(BotConfig::scripted(
                    BotDifficulty::Standard,
                    BotStance::Balanced,
                    seed,
                ))
            })
            .min_by_key(|profile| profile.traits.fortification)
            .expect("the bounded seed search is nonempty")
    }

    #[test]
    fn voluntary_defense_uses_only_unowned_safe_construction_builders() {
        let (mut obs, briefing) = opportunity_fixture();
        obs.my_units.extend([
            unit(2, PlayerId(0), UnitKind::Harvester, TilePos::new(6, 11)),
            unit(3, PlayerId(0), UnitKind::Harvester, TilePos::new(6, 12)),
            unit(4, PlayerId(0), UnitKind::Harvester, TilePos::new(7, 12)),
            unit(5, PlayerId(0), UnitKind::Harvester, TilePos::new(8, 11)),
        ]);
        obs.my_units.sort_unstable_by_key(|unit| unit.id);

        let mut policy = UtilityPolicy::new();
        policy.scout = Some(UnitId(1));
        policy.evacuating_workers.push(UnitId(2));
        policy.retreating_contested_scout = Some(RetreatingContestedScout {
            unit: UnitId(3),
            order_dispatched: true,
            suspend_solo_air_on_loss: false,
        });
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries are constructible")
            .cost;
        policy
            .commit_adjudicated_foundry(
                FreshFoundryProposal::fixture(
                    TilePos::new(8, 14),
                    UnitId(4),
                    0,
                    foundry_cost,
                    0,
                    obs.tick.saturating_add(12),
                    FoundryOpportunityCase::fixture(
                        FoundryUrgency::Timely,
                        FoundryConfidence::Supported,
                        FoundryStrategicValue::Material,
                        FoundryTimeToImpact::Near,
                        FoundryExecutionSafety::Secure,
                    ),
                ),
                obs.tick,
                &mut Vec::new(),
            )
            .expect("the fixture installs one saved Foundry builder");

        let resources = ResourceSnapshot::from_observation(&obs);
        let profile = low_fortification_profile();
        let builders: Vec<_> = obs.my_units.iter().collect();
        let proposals = policy.fresh_defense_proposals(
            &profile,
            &obs,
            &resources,
            &briefing,
            Orientation::for_home(&obs, HOME),
            HOME,
            &[],
            &[],
            &builders,
            &[],
            0,
            0,
        );
        assert!(!proposals.is_empty());
        assert!(
            proposals
                .iter()
                .all(|proposal| proposal.builder() == UnitId(5)),
            "only the unowned, non-evacuating construction builder may receive a defense quote: {proposals:?}"
        );

        let protected_builders: Vec<_> = obs
            .my_units
            .iter()
            .filter(|builder| builder.id != UnitId(5))
            .collect();
        assert!(
            policy
                .fresh_defense_proposals(
                    &profile,
                    &obs,
                    &resources,
                    &briefing,
                    Orientation::for_home(&obs, HOME),
                    HOME,
                    &[],
                    &[],
                    &protected_builders,
                    &[],
                    0,
                    0,
                )
                .is_empty(),
            "active scouts, evacuating workers, retreating recovery scouts, and saved Foundry builders are not voluntary defense fallbacks"
        );
    }

    #[test]
    fn same_observation_worker_incident_excludes_the_endangered_builder() {
        let (mut obs, briefing) = opportunity_fixture();
        let endangered = obs.my_units[0].tile;
        let mut safe = unit(2, PlayerId(0), UnitKind::Harvester, TilePos::new(2, 11));
        safe.idle = true;
        obs.my_units.push(safe);
        obs.my_units[0].hp = obs.my_units[0].hp.saturating_sub(1);
        obs.salvage_incidents = vec![endangered];
        obs.my_units.sort_unstable_by_key(|unit| unit.id);

        let mut policy = UtilityPolicy::new();
        policy.harvester_watch = vec![HarvesterWatch {
            id: UnitId(1),
            tile: endangered,
            hp: UnitKind::Harvester.stats().max_hp,
            source: None,
        }];
        policy.refresh_allocation_worker_safety(&obs, &[], &[]);
        let resources = ResourceSnapshot::from_observation(&obs);
        let profile = low_fortification_profile();
        let builders = obs.my_units.iter().collect::<Vec<_>>();
        let proposals = policy.fresh_defense_proposals(
            &profile,
            &obs,
            &resources,
            &briefing,
            Orientation::for_home(&obs, HOME),
            HOME,
            &[],
            &[],
            &builders,
            &[],
            0,
            0,
        );

        assert!(!proposals.is_empty());
        assert!(
            proposals
                .iter()
                .all(|proposal| proposal.builder() == UnitId(2)),
            "a newly quarantined worker must remain available for evacuation, not frozen into Defense: {proposals:?}"
        );
    }

    fn defense_quote(
        evidence: DefenseOpportunityEvidence,
        marginal_value: u32,
        anchor: TilePos,
    ) -> StrategicDefenseQuote {
        StrategicDefenseQuote {
            placement: crate::bot::utility::defense::DefensePlacement {
                anchor,
                builder: UnitId(1),
            },
            uncovered_value: marginal_value,
            reinforced_value: 0,
            builder_travel_cost: 0,
            evidence,
            evidence_count: 1,
            threat_arrival_ticks: None,
            blind_exposure: 0,
        }
    }

    #[test]
    fn protected_value_bands_are_explicit() {
        assert_eq!(strategic_value(5), StrategicValue::Incremental);
        assert_eq!(strategic_value(6), StrategicValue::Material);
        assert_eq!(strategic_value(11), StrategicValue::Material);
        assert_eq!(strategic_value(12), StrategicValue::Decisive);
    }

    #[test]
    fn defense_readiness_retains_the_exact_build_command_route_cost() {
        let (obs, _) = opportunity_fixture();
        let profile = low_fortification_profile();
        let builders = vec![&obs.my_units[0]];
        let quote = StrategicDefenseQuote {
            builder_travel_cost: 54,
            threat_arrival_ticks: Some(10_000),
            ..defense_quote(
                DefenseOpportunityEvidence::CurrentArmed,
                12,
                TilePos::new(8, 3),
            )
        };
        let construction = BuildingKind::Bastion
            .base_stats()
            .construction
            .expect("Bastions are constructible");
        let build_ticks = u64::from(
            construction
                .build_ticks
                .div_ceil(UnitKind::Harvester.stats().build_rate.max(1)),
        );
        let exact_travel = travel_ticks(54, UnitKind::Harvester.stats().speed);
        let globally_shortest_travel = travel_ticks(50, UnitKind::Harvester.stats().speed);
        assert!(exact_travel > globally_shortest_travel);

        let proposal = defense_proposal(
            DefenseConstruction::Bastion,
            quote,
            &profile,
            &obs,
            &builders,
            0,
            None,
        )
        .expect("the current high-value Bastion remains worthwhile");

        assert_eq!(
            proposal.ready_at(),
            obs.tick.saturating_add(exact_travel + build_ticks)
        );
    }

    #[test]
    fn mobile_reinforcement_downgrades_only_one_band() {
        assert_eq!(
            downgrade_value(StrategicValue::Decisive),
            StrategicValue::Material
        );
        assert_eq!(
            downgrade_value(StrategicValue::Material),
            StrategicValue::Incremental
        );
        assert_eq!(
            downgrade_value(StrategicValue::Incremental),
            StrategicValue::Incremental
        );
    }

    #[test]
    fn claimed_mobile_reinforcement_cannot_downgrade_a_defense_case() {
        let (mut obs, briefing) = opportunity_fixture();
        obs.enemy_units.clear();
        let anchor = TilePos::new(9, 10);
        obs.my_units.push(unit(
            50,
            PlayerId(0),
            UnitKind::Sentinel,
            anchor.offset(-1, 0),
        ));
        let builders = vec![&obs.my_units[0]];
        let profile = low_fortification_profile();
        let quote = StrategicDefenseQuote {
            threat_arrival_ticks: Some(1_000),
            ..defense_quote(DefenseOpportunityEvidence::CurrentArmed, 12, anchor)
        };
        let policy = UtilityPolicy::new();

        let mut available_context = DefenseThinkContext::new(&policy, &obs, &briefing, &[], &[]);
        let available_reinforcement = mobile_reinforcement_ticks(
            &obs,
            &mut available_context,
            BuildingKind::Turret,
            anchor,
            &[],
        );
        let available = defense_proposal(
            DefenseConstruction::Turret,
            quote,
            &profile,
            &obs,
            &builders,
            0,
            available_reinforcement,
        )
        .expect("the current threat admits a defense");

        let mut claimed_context = DefenseThinkContext::new(&policy, &obs, &briefing, &[], &[]);
        let claimed_reinforcement = mobile_reinforcement_ticks(
            &obs,
            &mut claimed_context,
            BuildingKind::Turret,
            anchor,
            &[UnitId(50)],
        );
        let claimed = defense_proposal(
            DefenseConstruction::Turret,
            quote,
            &profile,
            &obs,
            &builders,
            0,
            claimed_reinforcement,
        )
        .expect("excluding reinforcement does not erase the static opportunity");

        assert_eq!(available.case.value, StrategicValue::Material);
        assert_eq!(available.case.urgency, Urgency::Timely);
        assert_eq!(claimed.case.value, StrategicValue::Decisive);
        assert_eq!(claimed.case.urgency, Urgency::Pressing);
    }

    #[test]
    fn rich_bank_abstains_only_from_the_weakest_weapon_case() {
        let (mut obs, _briefing) = opportunity_fixture();
        obs.scrap = u32::MAX;
        obs.enemy_units.clear();
        let builders = vec![&obs.my_units[0]];
        let profile = low_fortification_profile();
        let anchor = TilePos::new(9, 10);
        assert!(
            defense_proposal(
                DefenseConstruction::Turret,
                defense_quote(DefenseOpportunityEvidence::PublicPrior, 1, anchor),
                &profile,
                &obs,
                &builders,
                0,
                None,
            )
            .is_none(),
            "wealth alone must not fund an incremental public-prior weapon"
        );

        assert!(
            defense_proposal(
                DefenseConstruction::Turret,
                defense_quote(DefenseOpportunityEvidence::PublicPrior, 6, anchor),
                &profile,
                &obs,
                &builders,
                0,
                None,
            )
            .is_some(),
            "material public-prior coverage remains worthwhile"
        );

        assert!(
            defense_proposal(
                DefenseConstruction::Turret,
                defense_quote(DefenseOpportunityEvidence::CurrentArmed, 1, anchor),
                &profile,
                &obs,
                &builders,
                0,
                None,
            )
            .is_some(),
            "current pressure keeps an incremental weapon eligible"
        );

        assert!(
            array_proposal(
                DefenseConstruction::Array,
                StrategicArrayQuote {
                    anchor,
                    builder: UnitId(1),
                    usable_radar: 100,
                    novel_radar: 1,
                    builder_travel_cost: 0,
                    evidence: DefenseOpportunityEvidence::PublicPrior,
                    evidence_count: 1,
                },
                &profile,
                &obs,
                &builders,
                0,
            )
            .is_some(),
            "Array retains its explicit positive-novel-coverage contract"
        );
    }

    #[test]
    fn construction_roles_are_complete_and_unique() {
        let kinds = DefenseConstruction::ALL.map(DefenseConstruction::kind);
        assert_eq!(
            kinds,
            [
                BuildingKind::Turret,
                BuildingKind::Bastion,
                BuildingKind::FlakTurret,
                BuildingKind::ScuttleCharge,
                BuildingKind::Barricade,
                BuildingKind::Array,
            ]
        );
    }

    #[test]
    fn only_ground_threatening_air_evidence_unlocks_flak() {
        let mut obs = Observation {
            tick: 200,
            ..Observation::default()
        };
        obs.blips.push(TilePos::new(4, 4));
        assert!(!confirmed_air_threat(&obs, &[], &[]));

        obs.enemy_units
            .push(unit(7, PlayerId(1), UnitKind::Kestrel, TilePos::new(4, 4)));
        assert!(
            !confirmed_air_threat(&obs, &[], &[]),
            "an unarmed scout is information, not an anti-air demand"
        );

        obs.enemy_units.clear();
        obs.enemy_units
            .push(unit(8, PlayerId(1), UnitKind::Talon, TilePos::new(4, 4)));
        assert!(
            !confirmed_air_threat(&obs, &[], &[]),
            "a pure air-to-air fighter cannot threaten a ground asset"
        );

        obs.enemy_units.clear();
        obs.enemy_units
            .push(unit(9, PlayerId(1), UnitKind::Skyhook, TilePos::new(4, 4)));
        assert!(confirmed_air_threat(&obs, &[], &[]));

        obs.enemy_units.clear();
        let remembered_fighter = UnitContact {
            id: UnitId(10),
            player: PlayerId(1),
            kind: UnitKind::Wisp,
            tile: TilePos::new(5, 4),
            hp: UnitKind::Wisp.stats().max_hp,
            grounded: false,
            last_seen: obs.tick,
            evidence: ContactEvidence::Remembered,
        };
        assert!(!confirmed_air_threat(
            &obs,
            std::slice::from_ref(&remembered_fighter),
            &[]
        ));

        let remembered_bomber = UnitContact {
            id: UnitId(11),
            player: PlayerId(1),
            kind: UnitKind::Condor,
            tile: TilePos::new(5, 4),
            hp: UnitKind::Condor.stats().max_hp,
            grounded: false,
            last_seen: obs.tick,
            evidence: ContactEvidence::Remembered,
        };
        assert!(confirmed_air_threat(
            &obs,
            std::slice::from_ref(&remembered_bomber),
            &[]
        ));

        let mut airworks = building(BuildingKind::Airworks, true);
        airworks.player = PlayerId(1);
        airworks.seen = true;
        obs.enemy_buildings.push(airworks);
        assert!(confirmed_air_threat(&obs, &[], &[]));

        obs.enemy_buildings[0].seen = false;
        assert!(
            !confirmed_air_threat(&obs, &[], &[]),
            "an unseen Observation ghost is not current Airworks evidence"
        );
        let remembered_airworks = BuildingContact {
            id: Some(obs.enemy_buildings[0].id),
            player: PlayerId(1),
            kind: BuildingKind::Airworks,
            anchor: obs.enemy_buildings[0].anchor,
            hp: BuildingKind::Airworks.base_stats().max_hp,
            built: true,
            tier: 0,
            last_seen: Some(obs.tick),
            evidence: ContactEvidence::Remembered,
        };
        assert!(confirmed_air_threat(
            &obs,
            &[],
            std::slice::from_ref(&remembered_airworks)
        ));
    }

    #[test]
    fn pure_air_to_air_evidence_does_not_produce_a_voluntary_flak_quote() {
        let (mut obs, briefing) = opportunity_fixture();
        let profile = low_fortification_profile();
        let flak = |obs: &Observation, contacts: &[UnitContact]| {
            let resources = ResourceSnapshot::from_observation(obs);
            let builders = vec![&obs.my_units[0]];
            UtilityPolicy::new()
                .fresh_defense_proposals(
                    &profile,
                    obs,
                    &resources,
                    &briefing,
                    Orientation::for_home(obs, HOME),
                    HOME,
                    contacts,
                    &[],
                    &builders,
                    &[],
                    0,
                    0,
                )
                .into_iter()
                .find(|proposal| proposal.kind() == BuildingKind::FlakTurret)
        };

        obs.enemy_units = vec![unit(20, PlayerId(1), UnitKind::Talon, TilePos::new(30, 10))];
        assert!(flak(&obs, &[]).is_none());

        obs.enemy_units.clear();
        let remembered_fighter = UnitContact {
            id: UnitId(21),
            player: PlayerId(1),
            kind: UnitKind::Wisp,
            tile: TilePos::new(30, 10),
            hp: UnitKind::Wisp.stats().max_hp,
            grounded: false,
            last_seen: obs.tick,
            evidence: ContactEvidence::Remembered,
        };
        assert!(flak(&obs, std::slice::from_ref(&remembered_fighter)).is_none());

        obs.enemy_units = vec![unit(
            22,
            PlayerId(1),
            UnitKind::Condor,
            TilePos::new(30, 10),
        )];
        assert!(
            flak(&obs, &[]).is_some(),
            "a current ground-attack aircraft remains a credible Flak opportunity"
        );
    }

    #[test]
    fn only_real_construction_prerequisites_gate_a_role() {
        let mut obs = Observation::default();
        assert!(construction_prerequisites_met(&obs, BuildingKind::Turret));
        assert!(!construction_prerequisites_met(
            &obs,
            BuildingKind::ScuttleCharge
        ));

        obs.my_buildings
            .push(building(BuildingKind::Fabricator, true));
        assert!(construction_prerequisites_met(
            &obs,
            BuildingKind::ScuttleCharge
        ));
    }

    #[test]
    fn array_case_is_bounded_below_decisive_and_pressing() {
        let mut obs = Observation {
            tick: 20,
            ..Observation::default()
        };
        obs.my_units.push(unit(
            3,
            PlayerId(0),
            UnitKind::Harvester,
            TilePos::new(3, 3),
        ));
        let builders = vec![&obs.my_units[0]];
        let proposal = array_proposal(
            DefenseConstruction::Array,
            StrategicArrayQuote {
                anchor: TilePos::new(4, 4),
                builder: UnitId(3),
                usable_radar: 100,
                novel_radar: 60,
                builder_travel_cost: 10,
                evidence: DefenseOpportunityEvidence::CurrentArmed,
                evidence_count: 1,
            },
            &ResolvedProfile::resolve(BotConfig::default()),
            &obs,
            &builders,
            90,
        )
        .expect("positive novel coverage admits an Array");

        assert_eq!(proposal.case.urgency, Urgency::Timely);
        assert_eq!(proposal.case.value, StrategicValue::Material);
        assert_ne!(proposal.case.time_to_impact, TimeToImpact::Immediate);
        assert_eq!(proposal.minimum_residual_scrap, 90);
    }

    #[test]
    fn adjudicated_commit_emits_the_frozen_build() {
        let proposal = FreshDefenseProposal::fixture(
            DefenseConstruction::Bastion,
            TilePos::new(9, 11),
            UnitId(7),
            ProposalCase {
                urgency: Urgency::Timely,
                confidence: Confidence::Current,
                value: StrategicValue::Material,
                time_to_impact: TimeToImpact::Near,
                safety: ExecutionSafety::Secure,
            },
            80,
            90,
        );
        let mut intents = Vec::new();
        UtilityPolicy::new().commit_adjudicated_defense(proposal, &mut intents);

        assert_eq!(
            intents,
            vec![Intent::BuildWith {
                builder: UnitId(7),
                kind: BuildingKind::Bastion,
                anchor: TilePos::new(9, 11),
            }]
        );
    }

    #[test]
    fn real_derivation_keeps_every_legal_role_for_low_fortification() {
        let (obs, briefing) = opportunity_fixture();
        let resources = ResourceSnapshot::from_observation(&obs);
        let builders = vec![&obs.my_units[0]];
        let profile = low_fortification_profile();
        let proposals = UtilityPolicy::new().fresh_defense_proposals(
            &profile,
            &obs,
            &resources,
            &briefing,
            Orientation::for_home(&obs, HOME),
            HOME,
            &[],
            &[],
            &builders,
            &[],
            90,
            0,
        );
        let roles: BTreeSet<_> = proposals.iter().map(FreshDefenseProposal::kind).collect();

        assert_eq!(
            roles,
            BTreeSet::from([
                BuildingKind::Turret,
                BuildingKind::Bastion,
                BuildingKind::FlakTurret,
                BuildingKind::ScuttleCharge,
                BuildingKind::Barricade,
                BuildingKind::Array,
            ])
        );
        assert!(profile.traits.fortification < 50);
        assert!(proposals.iter().all(|proposal| {
            proposal.builder() == UnitId(1)
                && proposal.site().anchor() == proposal.anchor()
                && proposal.construction_capital()
                    == proposal
                        .kind()
                        .base_stats()
                        .construction
                        .expect("every proposal is constructible")
                        .cost
        }));
    }

    #[test]
    fn committed_capital_prunes_only_defenses_that_cannot_win_allocation() {
        let (obs, briefing) = opportunity_fixture();
        let resources = ResourceSnapshot::from_observation(&obs);
        let builders = vec![&obs.my_units[0]];
        let profile = low_fortification_profile();
        let derive = |committed| {
            UtilityPolicy::new().fresh_defense_proposals(
                &profile,
                &obs,
                &resources,
                &briefing,
                Orientation::for_home(&obs, HOME),
                HOME,
                &[],
                &[],
                &builders,
                &[],
                90,
                committed,
            )
        };
        let baseline = derive(0);
        assert_eq!(baseline.len(), DefenseConstruction::ALL.len());
        assert!(derive(obs.scrap).is_empty());
        for proposal in &baseline {
            let required = proposal.construction_capital() + proposal.minimum_residual_scrap();
            for available in [required - 1, required] {
                assert_eq!(
                    derive(obs.scrap - available),
                    baseline
                        .iter()
                        .copied()
                        .filter(|quote| quote.construction_capital() + 90 <= available)
                        .collect::<Vec<_>>(),
                    "prior capital prunes unaffordable roles without changing exact viable quotes"
                );
            }
        }
    }

    #[test]
    fn planned_arrays_exhaust_novel_coverage_in_diminishing_steps() {
        let (mut obs, briefing) = opportunity_fixture();
        let profile = low_fortification_profile();
        let mut prior_novel = u32::MAX;
        let mut anchors = BTreeSet::new();
        let mut exhausted = false;

        for id in 100..116 {
            let resources = ResourceSnapshot::from_observation(&obs);
            let builders = vec![&obs.my_units[0]];
            let proposal = UtilityPolicy::new()
                .fresh_defense_proposals(
                    &profile,
                    &obs,
                    &resources,
                    &briefing,
                    Orientation::for_home(&obs, HOME),
                    HOME,
                    &[],
                    &[],
                    &builders,
                    &[],
                    90,
                    0,
                )
                .into_iter()
                .find(|proposal| proposal.kind() == BuildingKind::Array);
            let Some(proposal) = proposal else {
                exhausted = true;
                break;
            };
            assert!(proposal.marginal_value > 0);
            assert!(proposal.marginal_value <= prior_novel);
            assert!(
                anchors.insert(proposal.anchor()),
                "a reserved Array site must not be proposed twice"
            );
            prior_novel = proposal.marginal_value;

            let mut planned = building(BuildingKind::Array, false);
            planned.id = BuildingId(id);
            planned.anchor = proposal.anchor();
            obs.my_buildings.push(planned);
            obs.my_queues.push(Vec::new());
        }

        assert!(
            exhausted,
            "finite in-range radar coverage must stop producing redundant Arrays"
        );
    }
}
