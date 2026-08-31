//! Player-facing multi-factory composition scheduling.

use super::*;
use crate::ids::BuildingId;
use crate::stats::Role;
use std::cmp::Reverse;

/// Keep factory commitments shallow enough to react when fresh intelligence
/// changes the desired composition. The simulation permits longer queues, but
/// filling them would turn a seeded preference into minutes of sunk orders.
const PLANNING_DEPTH: usize = 2;
const ADAPTIVE_AIRWORKS_DEPTH: usize = 1;
const ALLY_DISCOUNT: usize = 2;
const SENTINEL_EQUIVALENTS_PER_EXTRA_FOUNDRY: u64 = 6;

#[derive(Debug, Clone, Copy)]
struct Producer {
    id: BuildingId,
    kind: BuildingKind,
    depth: usize,
}

/// Exact ordinary-combat strength projected after the intents already emitted
/// during this think.
///
/// `missing_scrap` prices the remaining strength in whole Sentinels, because
/// that is the unit the ordinary-core pass can add without further tech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::bot) struct CombatCoreStatus {
    pub(in crate::bot) projected_strength: u64,
    pub(in crate::bot) target_strength: u64,
    pub(in crate::bot) missing_strength: u64,
    pub(in crate::bot) missing_scrap: u32,
    pub(in crate::bot) ready: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct AdaptiveProductionContext<'a> {
    reserved: &'a [UnitId],
    allow_defensive_air: bool,
    allow_repeatable_ground: bool,
    capital_reserve: u32,
}

impl<'a> AdaptiveProductionContext<'a> {
    pub(super) const fn new(
        reserved: &'a [UnitId],
        allow_defensive_air: bool,
        capital_reserve: u32,
    ) -> Self {
        Self {
            reserved,
            allow_defensive_air,
            allow_repeatable_ground: true,
            capital_reserve,
        }
    }

    pub(super) const fn with_repeatable_ground(mut self, allow: bool) -> Self {
        self.allow_repeatable_ground = allow;
        self
    }
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    kind: UnitKind,
    target: usize,
    own_floor: usize,
    signature_floor: usize,
    discount_allies: bool,
    repeatable: bool,
    order: u8,
}

#[derive(Debug, Clone, Copy)]
struct CandidateContext<'a> {
    allow_repeatable_ground: bool,
    repeatable_pass: bool,
    capital_reserve: u32,
    core_reserve: u32,
    budget: u32,
    intents: &'a [Intent],
    support_target: usize,
}

impl Candidate {
    const fn bounded(
        kind: UnitKind,
        target: usize,
        own_floor: usize,
        signature_floor: usize,
        discount_allies: bool,
        order: u8,
    ) -> Self {
        Self {
            kind,
            target,
            own_floor,
            signature_floor,
            discount_allies,
            repeatable: false,
            order,
        }
    }

    const fn repeatable(kind: UnitKind, order: u8) -> Self {
        Self {
            kind,
            target: 1,
            own_floor: 0,
            signature_floor: 0,
            discount_allies: false,
            repeatable: true,
            order,
        }
    }
}

impl UtilityPolicy {
    /// Fill shallow queues across every built factory after the legacy opening
    /// and emergency arms have had first claim on the bank. All projections
    /// include orders emitted earlier in this same think.
    pub(super) fn adaptive_production(
        &self,
        dials: &Dials,
        obs: &Observation,
        context: AdaptiveProductionContext<'_>,
        budget: &mut u32,
        intents: &mut Vec<Intent>,
    ) {
        let AdaptiveProductionContext {
            reserved,
            allow_defensive_air,
            allow_repeatable_ground,
            capital_reserve,
        } = context;
        let core_reserve = fill_combat_core(
            obs,
            reserved,
            u64::from(dials.army_size),
            capital_reserve,
            budget,
            intents,
        )
        .missing_scrap;
        if dials.discretionary_slots == 0 {
            return;
        }

        let mut producers: Vec<Producer> = obs
            .my_buildings
            .iter()
            .enumerate()
            .filter(|(_, building)| building.built)
            .filter(|(_, building)| !building.kind.base_stats().produces.is_empty())
            .map(|(queue_index, building)| Producer {
                id: building.id,
                kind: building.kind,
                depth: obs
                    .my_queues
                    .get(queue_index)
                    .map_or(PLANNING_DEPTH, Vec::len)
                    .saturating_add(planned_at(intents, building.id)),
            })
            .collect();
        producers.sort_by_key(|producer| producer.id);
        let support_target = current_support_target(dials, obs);

        let mut serviced = 0usize;
        // Breadth before depth: every empty factory gets a chance before an
        // earlier id receives its second discretionary order. Within one
        // depth, unmet finite composition goals precede open-ended line
        // production across every producer.
        for target_depth in 1..=PLANNING_DEPTH {
            for repeatable_pass in [false, true] {
                for producer in &mut producers {
                    if serviced >= dials.discretionary_slots {
                        return;
                    }
                    if producer.kind == BuildingKind::Airworks
                        && (!allow_defensive_air || target_depth > ADAPTIVE_AIRWORKS_DEPTH)
                    {
                        continue;
                    }
                    if producer.depth >= target_depth {
                        continue;
                    }
                    let Some(candidate) = best_candidate(
                        dials,
                        obs,
                        producer.kind,
                        CandidateContext {
                            allow_repeatable_ground,
                            repeatable_pass,
                            capital_reserve,
                            core_reserve,
                            budget: *budget,
                            intents,
                            support_target,
                        },
                    ) else {
                        continue;
                    };
                    *budget -= candidate.kind.stats().cost;
                    intents.push(Intent::TrainAt {
                        building: producer.id,
                        kind: candidate.kind,
                    });
                    producer.depth += 1;
                    serviced += 1;
                }
                if !repeatable_pass
                    && producers.iter().any(|producer| {
                        producer.depth < target_depth
                            && !(producer.kind == BuildingKind::Airworks
                                && (!allow_defensive_air || target_depth > ADAPTIVE_AIRWORKS_DEPTH))
                            && best_candidate(
                                dials,
                                obs,
                                producer.kind,
                                CandidateContext {
                                    allow_repeatable_ground,
                                    repeatable_pass: false,
                                    capital_reserve,
                                    core_reserve,
                                    budget: u32::MAX,
                                    intents,
                                    support_target,
                                },
                            )
                            .is_some()
                    })
                {
                    // A bounded composition target is also a savings target.
                    // Spending the partial fund on a repeatable line would
                    // keep a more expensive specialist permanently unfunded.
                    return;
                }
            }
        }
    }
}

/// Bring the ordinary line to the force strength that the army channel uses as
/// its minimum commitment. Strategic reservations, raiders, artillery, and
/// support pieces do not discharge this obligation: they have other owners or
/// need a line in front of them. This pass is independent of discretionary
/// attention, so a higher rung extends the same core instead of replacing it.
pub(super) fn fill_combat_core(
    obs: &Observation,
    reserved: &[UnitId],
    sentinel_equivalent_floor: u64,
    capital_reserve: u32,
    budget: &mut u32,
    intents: &mut Vec<Intent>,
) -> CombatCoreStatus {
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
    let status = combat_core_status(obs, reserved, intents, sentinel_equivalent_floor);
    if status.ready {
        return status;
    }
    let mut projected_strength = status.projected_strength;

    let mut foundries: Vec<Producer> = obs
        .my_buildings
        .iter()
        .enumerate()
        .filter(|(_, building)| building.built && building.kind == BuildingKind::Foundry)
        .map(|(queue_index, building)| Producer {
            id: building.id,
            kind: building.kind,
            depth: obs
                .my_queues
                .get(queue_index)
                .map_or(PLANNING_DEPTH, Vec::len)
                .saturating_add(planned_at(intents, building.id)),
        })
        .collect();
    foundries.sort_by_key(|producer| producer.id);
    if foundries.is_empty() {
        return status;
    }

    let sentinel_cost = UnitKind::Sentinel.stats().cost;
    'depths: for target_depth in 1..=PLANNING_DEPTH {
        for foundry in &mut foundries {
            if projected_strength >= status.target_strength {
                break 'depths;
            }
            if foundry.depth >= target_depth
                || *budget < sentinel_cost.saturating_add(capital_reserve)
            {
                continue;
            }
            *budget -= sentinel_cost;
            intents.push(Intent::TrainAt {
                building: foundry.id,
                kind: UnitKind::Sentinel,
            });
            foundry.depth += 1;
            projected_strength = projected_strength.saturating_add(sentinel_strength);
        }
    }

    combat_core_status(obs, reserved, intents, sentinel_equivalent_floor)
}

/// Measure an explicit Sentinel-equivalent floor without inferring ownership
/// from army bookkeeping. Callers exclude only the exact units committed to a
/// strategic operation; ordinary Executive armies therefore remain part of the
/// available fighting line.
pub(in crate::bot) fn combat_core_status(
    obs: &Observation,
    reserved: &[UnitId],
    intents: &[Intent],
    sentinel_equivalent_floor: u64,
) -> CombatCoreStatus {
    let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
    let target_strength = sentinel_strength.saturating_mul(sentinel_equivalent_floor);
    let live = obs
        .my_units
        .iter()
        .filter(|unit| !reserved.contains(&unit.id) && ordinary_core_unit(unit.kind))
        .map(crate::bot::executive::unit_strength)
        .sum::<u64>();
    let queued = obs
        .my_queues
        .iter()
        .flatten()
        .copied()
        .filter(|kind| ordinary_core_unit(*kind))
        .map(full_ground_strength)
        .sum::<u64>();
    let planned = intents
        .iter()
        .filter_map(|intent| match intent {
            Intent::TrainAt { kind, .. } if ordinary_core_unit(*kind) => Some(*kind),
            _ => None,
        })
        .map(full_ground_strength)
        .sum::<u64>();
    let projected_strength = live.saturating_add(queued).saturating_add(planned);
    let missing_strength = target_strength.saturating_sub(projected_strength);
    CombatCoreStatus {
        projected_strength,
        target_strength,
        missing_strength,
        missing_scrap: missing_core_scrap(
            missing_strength,
            sentinel_strength,
            UnitKind::Sentinel.stats().cost,
        ),
        ready: missing_strength == 0,
    }
}

fn missing_core_scrap(missing_strength: u64, sentinel_strength: u64, sentinel_cost: u32) -> u32 {
    let missing_sentinels = missing_strength.div_ceil(sentinel_strength);
    u32::try_from(missing_sentinels)
        .unwrap_or(u32::MAX)
        .saturating_mul(sentinel_cost)
}

/// Whether the ordinary fighting line can support one more Foundry.
///
/// The first expansion, from one total Foundry to two, is the common economic
/// baseline. Every later Foundry requires another six Sentinel-equivalents of
/// unreserved live or queued ordinary-core ground strength.
pub(super) fn extra_foundry_core_ready(
    obs: &Observation,
    reserved: &[UnitId],
    projected_foundries: usize,
) -> bool {
    let extra_expansions = projected_foundries.saturating_sub(1);
    let required_equivalents = u64::try_from(extra_expansions)
        .unwrap_or(u64::MAX)
        .saturating_mul(SENTINEL_EQUIVALENTS_PER_EXTRA_FOUNDRY);
    combat_core_status(obs, reserved, &[], required_equivalents).ready
}

fn ordinary_core_unit(kind: UnitKind) -> bool {
    matches!(kind.role(), Role::Sentinel | Role::Warden | Role::Breaker)
}

fn full_ground_strength(kind: UnitKind) -> u64 {
    let stats = kind.stats();
    let damage_per_hundred_ticks = stats
        .weapons
        .iter()
        .filter(|weapon| weapon.targets.covers(crate::stats::Domain::Ground))
        .map(|weapon| u64::from(weapon.damage) * 100 / u64::from(weapon.cooldown_ticks))
        .sum::<u64>();
    u64::from(stats.max_hp).saturating_mul(damage_per_hundred_ticks)
}

fn best_candidate(
    dials: &Dials,
    obs: &Observation,
    producer: BuildingKind,
    context: CandidateContext<'_>,
) -> Option<Candidate> {
    let CandidateContext {
        allow_repeatable_ground,
        repeatable_pass,
        capital_reserve,
        core_reserve,
        budget,
        intents,
        support_target,
    } = context;
    candidates(dials, obs, producer, support_target)
        .into_iter()
        .filter(|candidate| trainable_at(obs, producer, candidate.kind))
        .filter(|candidate| candidate.repeatable == repeatable_pass)
        .filter(|candidate| allow_repeatable_ground || !candidate.repeatable)
        .filter(|candidate| {
            let own = own_role_count(obs, intents, candidate.kind.role());
            let allied = if candidate.discount_allies {
                ally_role_count(obs, candidate.kind.role()) / ALLY_DISCOUNT
            } else {
                0
            };
            candidate.repeatable
                || own < candidate.own_floor
                || own.saturating_add(allied) < candidate.target
        })
        .filter(|candidate| {
            let fighting_reserve =
                if matches!(candidate.kind, UnitKind::Harvester | UnitKind::Sentinel) {
                    0
                } else {
                    UnitKind::Sentinel.stats().cost
                };
            let reserve = capital_reserve.saturating_add(core_reserve.max(fighting_reserve));
            budget >= candidate.kind.stats().cost.saturating_add(reserve)
        })
        .min_by_key(|candidate| {
            let own = own_role_count(obs, intents, candidate.kind.role());
            let allied = if candidate.discount_allies {
                ally_role_count(obs, candidate.kind.role()) / ALLY_DISCOUNT
            } else {
                0
            };
            let effective = own.saturating_add(allied);
            let signature_rank = u8::from(own >= candidate.signature_floor);
            let floor_rank = u8::from(own >= candidate.own_floor);
            let saturation = if candidate.repeatable {
                usize::MAX
            } else {
                effective.saturating_mul(1_000) / candidate.target.max(1)
            };
            (
                signature_rank,
                floor_rank,
                saturation,
                Reverse(candidate.target),
                candidate.order,
                candidate.kind,
            )
        })
}

fn candidates(
    dials: &Dials,
    obs: &Observation,
    producer: BuildingKind,
    support_target: usize,
) -> Vec<Candidate> {
    let role = |role: Role, target, own_floor, signature_floor, discount_allies, order| {
        Candidate::bounded(
            role.unit_for(obs.faction),
            target,
            own_floor,
            signature_floor,
            discount_allies,
            order,
        )
    };
    match producer {
        BuildingKind::Foundry => {
            let workers = dials.harvester_target as usize;
            let bootstrap_workers = immediate_harvester_target(dials) as usize;
            let excavators = workers.saturating_sub(3).div_ceil(2).max(1);
            let mut choices = vec![
                role(
                    Role::Harvester,
                    bootstrap_workers,
                    bootstrap_workers,
                    0,
                    false,
                    0,
                ),
                role(Role::Scuttler, dials.raider_target, 1, 0, true, 20),
            ];
            if workers > bootstrap_workers && renewable_economy_stands(obs) {
                choices.push(role(Role::Harvester, workers, 0, 0, false, 5));
            }
            if dials.tech {
                choices.push(role(Role::Excavator, excavators, 1, 0, false, 10));
            }
            choices.push(Candidate::repeatable(UnitKind::Sentinel, 250));
            choices
        }
        BuildingKind::Fabricator if dials.tech => {
            vec![
                role(
                    Role::Bombard,
                    dials.siege_target,
                    1,
                    usize::from(dials.siege_target >= 3) * 2,
                    true,
                    10,
                ),
                role(
                    Role::Tender,
                    support_target,
                    1,
                    usize::from(support_target >= 3) * 2,
                    true,
                    20,
                ),
                role(
                    Role::Warden,
                    (dials.army_size as usize).div_ceil(2).clamp(2, 4),
                    1,
                    0,
                    true,
                    30,
                ),
                role(Role::AntiAir, 1, 1, 0, true, 40),
                Candidate::repeatable(UnitKind::Lancer, 250),
            ]
        }
        BuildingKind::Airworks if dials.deep_tech => vec![
            // These are independently useful air-defense purchases. Screen
            // and bomber cohorts instead have persistent ownership, budgets,
            // and dispatch semantics in StrategicPlanner.
            role(
                Role::AirAir,
                dials.air_wing.div_ceil(2).max(1),
                1,
                0,
                true,
                20,
            ),
            role(
                Role::Interceptor,
                dials.bomber_target.div_ceil(2).max(1),
                1,
                0,
                true,
                30,
            ),
        ],
        BuildingKind::Crucible if dials.deep_tech => vec![
            role(Role::Avalanche, dials.siege_target, 1, 0, true, 10),
            role(
                Role::Breaker,
                (dials.army_size as usize).div_ceil(3).clamp(1, 2),
                1,
                0,
                true,
                20,
            ),
        ],
        _ => Vec::new(),
    }
}

fn current_support_target(dials: &Dials, obs: &Observation) -> usize {
    if dials.support_target == 0 {
        return dials.support_target;
    }

    let origins: Vec<TilePos> = obs
        .my_units
        .iter()
        .filter(|unit| unit.kind.role() == Role::Tender)
        .map(|unit| unit.tile)
        .chain(
            obs.my_buildings
                .iter()
                .filter(|building| building.built && building.kind == BuildingKind::Fabricator)
                .map(|building| building.anchor),
        )
        .collect();
    let mut routes = crate::bot::routing::RouteProjection::known_ground(obs);
    let demand = obs
        .my_units
        .iter()
        .filter(|unit| is_mobile_support_patient(unit))
        .filter(|patient| {
            origins
                .iter()
                .any(|origin| routes.reaches(*origin, patient.tile))
        })
        .count();
    demand.saturating_add(1).min(dials.support_target)
}

fn renewable_economy_stands(obs: &Observation) -> bool {
    obs.my_buildings.iter().any(|building| {
        building.built
            && matches!(
                building.kind,
                BuildingKind::Extractor | BuildingKind::Reclaimer
            )
    })
}

fn trainable_at(obs: &Observation, producer: BuildingKind, kind: UnitKind) -> bool {
    producer.base_stats().produces.contains(&kind)
        && kind.faction().is_none_or(|faction| faction == obs.faction)
        && kind.stats().requires.iter().all(|required| {
            obs.my_buildings
                .iter()
                .any(|building| building.kind == *required && building.built)
        })
}

pub(super) fn planned_at(intents: &[Intent], building: BuildingId) -> usize {
    intents
        .iter()
        .filter(|intent| {
            matches!(intent, Intent::TrainAt { building: planned, .. } if *planned == building)
        })
        .count()
}

fn own_role_count(obs: &Observation, intents: &[Intent], role: Role) -> usize {
    obs.my_units
        .iter()
        .filter(|unit| unit.kind.role() == role)
        .count()
        + obs
            .my_queues
            .iter()
            .flatten()
            .filter(|kind| kind.role() == role)
            .count()
        + intents
            .iter()
            .filter(|intent| matches!(intent, Intent::TrainAt { kind, .. } if kind.role() == role))
            .count()
}

fn ally_role_count(obs: &Observation, role: Role) -> usize {
    obs.ally_units
        .iter()
        .filter(|unit| unit.kind.role() == role)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::Specialty;
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION, UnitObs};
    use crate::ids::{PlayerId, UnitId};
    use crate::scenario::{BotConfig, BotDifficulty, BotStance};
    use crate::state::Faction;

    fn observation() -> Observation {
        Observation {
            version: OBSERVATION_VERSION,
            tick: 200,
            me: PlayerId(0),
            scrap: 20_000,
            map_width: 24,
            map_height: 14,
            my_units: Vec::new(),
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units: Vec::new(),
            enemy_buildings: Vec::new(),
            visible: vec![true; 24 * 14],
            explored: vec![true; 24 * 14],
            known_scrap: Vec::new(),
            known_rock: Vec::new(),
            known_frames: Vec::new(),
            known_peaks: Vec::new(),
            known_wrecks: Vec::new(),
            salvage_incidents: Vec::new(),
            blips: Vec::new(),
            faction: Faction::Ferrous,
            my_shells: 0,
            incoming_shells: Vec::new(),
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

    fn unit(id: u32, player: u8, kind: UnitKind) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player: PlayerId(player),
            kind,
            tile: TilePos::new(3 + i32::try_from(id % 10).unwrap(), 6),
            hp: kind.stats().max_hp,
            idle: player == 0,
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

    fn add_building(obs: &mut Observation, id: u32, kind: BuildingKind, queue: Vec<UnitKind>) {
        obs.my_buildings.push(building(id, kind));
        obs.my_queues.push(queue);
    }

    fn adaptive_dials(slots: usize) -> Dials {
        let mut dials = Dials::full();
        dials.deep_tech = true;
        dials.adaptive_composition = true;
        dials.discretionary_slots = slots;
        dials
    }

    fn adaptive_context(reserved: &[UnitId]) -> AdaptiveProductionContext<'_> {
        AdaptiveProductionContext::new(reserved, true, 0)
    }

    fn resolved_dials(seed: u64) -> (ResolvedProfile, Dials) {
        let difficulty = BotDifficulty::Prime;
        let profile = BotConfig::scripted(difficulty, BotStance::Balanced, seed).resolve_profile();
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
        (profile, dials)
    }

    fn support_savings_fixture(foundry_id: u32, fabricator_id: u32) -> (Observation, Dials) {
        let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_305)
            .resolve_profile();
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
        assert_eq!(profile.primary, Specialty::Support);
        assert_eq!(dials.support_target, 2);

        let mut obs = observation();
        for (id, kind) in [
            (foundry_id, BuildingKind::Foundry),
            (fabricator_id, BuildingKind::Fabricator),
            (3, BuildingKind::Airworks),
            (4, BuildingKind::Crucible),
        ] {
            add_building(&mut obs, id, kind, Vec::new());
        }
        let mut enemy = building(99, BuildingKind::Foundry);
        enemy.player = PlayerId(1);
        enemy.anchor = TilePos::new(18, 8);
        obs.enemy_buildings.push(enemy);

        let mut next_id = 10_u32;
        let mut add_units = |kind: UnitKind, count: usize| {
            for _ in 0..count {
                obs.my_units.push(unit(next_id, 0, kind));
                next_id += 1;
            }
        };
        add_units(UnitKind::Harvester, dials.harvester_target as usize);
        add_units(UnitKind::Sentinel, dials.army_size as usize);
        add_units(UnitKind::Scuttler, dials.raider_target);
        add_units(UnitKind::Excavator, dials.harvester_target as usize);
        add_units(Role::Bombard.unit_for(obs.faction), dials.siege_target);
        add_units(Role::Tender.unit_for(obs.faction), 1);
        add_units(
            Role::Warden.unit_for(obs.faction),
            (dials.army_size as usize).div_ceil(2).clamp(2, 4),
        );
        add_units(Role::AntiAir.unit_for(obs.faction), 1);
        add_units(
            Role::AirAir.unit_for(obs.faction),
            dials.air_wing.div_ceil(2).max(1),
        );
        add_units(
            Role::Interceptor.unit_for(obs.faction),
            dials.bomber_target.div_ceil(2).max(1),
        );
        add_units(Role::Avalanche.unit_for(obs.faction), dials.siege_target);
        add_units(
            Role::Breaker.unit_for(obs.faction),
            (dials.army_size as usize).div_ceil(3).clamp(1, 2),
        );
        for patient in obs
            .my_units
            .iter_mut()
            .filter(|unit| unit.kind == UnitKind::Sentinel)
            .take(2)
        {
            patient.hp = patient.kind.stats().max_hp / 2;
        }

        (obs, dials)
    }

    fn full_production_schedule(
        dials: &Dials,
        obs: &Observation,
        budget: u32,
    ) -> (Vec<(BuildingId, UnitKind)>, u32) {
        let mut budget = budget;
        let mut intents = Vec::new();
        UtilityPolicy::new().production(
            dials,
            obs,
            TilePos::new(2, 2),
            ConstructionClaims {
                player_facing: true,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );
        (train_intents(&intents), budget)
    }

    fn train_intents(intents: &[Intent]) -> Vec<(BuildingId, UnitKind)> {
        intents
            .iter()
            .filter_map(|intent| match intent {
                Intent::TrainAt { building, kind } => Some((*building, *kind)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn breadth_first_scheduling_uses_every_factory_without_overcommitting() {
        let mut obs = observation();
        for id in 0..4 {
            obs.my_units.push(unit(id, 0, UnitKind::Harvester));
        }
        for (id, kind) in [
            (7, BuildingKind::Foundry),
            (2, BuildingKind::Foundry),
            (9, BuildingKind::Fabricator),
            (4, BuildingKind::Fabricator),
            (12, BuildingKind::Airworks),
            (6, BuildingKind::Airworks),
            (14, BuildingKind::Crucible),
            (10, BuildingKind::Crucible),
        ] {
            add_building(&mut obs, id, kind, Vec::new());
        }
        let dials = adaptive_dials(8);
        let mut policy = UtilityPolicy::new();
        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        policy.production(
            &dials,
            &obs,
            TilePos::new(2, 2),
            ConstructionClaims {
                player_facing: true,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );

        let trains = train_intents(&intents);
        for building in &obs.my_buildings {
            if building.kind == BuildingKind::Airworks {
                let air_orders: Vec<_> =
                    trains.iter().filter(|(id, _)| *id == building.id).collect();
                assert_eq!(
                    air_orders.len(),
                    1,
                    "adaptive air defense must leave the strategic queue slot open: {trains:?}"
                );
                assert!(
                    air_orders
                        .iter()
                        .all(|(_, kind)| matches!(kind.role(), Role::AirAir | Role::Interceptor)),
                    "generic scheduling trespassed on a strategic attack cohort: {trains:?}"
                );
            } else {
                assert!(
                    trains.iter().any(|(id, _)| *id == building.id),
                    "factory {:?} was left idle: {trains:?}",
                    building.id
                );
            }
            let queued = obs.my_queues[obs
                .my_buildings
                .iter()
                .position(|candidate| candidate.id == building.id)
                .unwrap()]
            .len();
            assert!(
                queued + planned_at(&intents, building.id) <= PLANNING_DEPTH,
                "factory {:?} exceeded the responsive queue depth: {trains:?}",
                building.id
            );
        }
        assert!(
            trains.iter().all(|(_, kind)| *kind != UnitKind::Sapper),
            "demolition units stay out until a raid controller can use them"
        );
        assert!(trains.iter().all(|(_, kind)| {
            kind.faction()
                .is_none_or(|faction| faction == Faction::Ferrous)
        }));
    }

    #[test]
    fn higher_difficulty_extends_the_same_canonical_production_prefix() {
        let mut obs = observation();
        for (id, kind) in [
            (8, BuildingKind::Foundry),
            (2, BuildingKind::Foundry),
            (9, BuildingKind::Fabricator),
            (3, BuildingKind::Fabricator),
            (10, BuildingKind::Airworks),
            (4, BuildingKind::Airworks),
            (11, BuildingKind::Crucible),
            (5, BuildingKind::Crucible),
        ] {
            add_building(&mut obs, id, kind, Vec::new());
        }

        for stance in BotStance::ALL {
            for seed in 0..256 {
                let results: Vec<_> = BotDifficulty::ALL
                    .into_iter()
                    .map(|difficulty| {
                        let profile =
                            BotConfig::scripted(difficulty, stance, seed).resolve_profile();
                        let dials =
                            Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
                        let mut budget = obs.scrap;
                        let mut intents = Vec::new();
                        UtilityPolicy::new().adaptive_production(
                            &dials,
                            &obs,
                            adaptive_context(&[]),
                            &mut budget,
                            &mut intents,
                        );
                        (dials, train_intents(&intents))
                    })
                    .collect();

                for pair in results.windows(2) {
                    let [(lower_dials, lower), (higher_dials, higher)] = pair else {
                        unreachable!()
                    };
                    let mut lower_composition = lower_dials.clone();
                    lower_composition.cadence = higher_dials.cadence;
                    lower_composition.discretionary_slots = higher_dials.discretionary_slots;
                    lower_composition.minimum_core_equivalents =
                        higher_dials.minimum_core_equivalents;
                    lower_composition.own_strength_scale = higher_dials.own_strength_scale;
                    lower_composition.enemy_strength_scale = higher_dials.enemy_strength_scale;
                    lower_composition.opponent_force_memory = higher_dials.opponent_force_memory;
                    lower_composition.coordinated_focus = higher_dials.coordinated_focus;
                    lower_composition.coordinated_defense_focus =
                        higher_dials.coordinated_defense_focus;
                    assert_eq!(
                        &lower_composition, higher_dials,
                        "{stance:?}, seed {seed}: difficulty redealt the strategic identity"
                    );
                    assert_eq!(
                        lower.as_slice(),
                        &higher[..lower.len()],
                        "{stance:?}, seed {seed}: a higher rung changed existing production choices"
                    );
                }
                for (difficulty, (dials, orders)) in BotDifficulty::ALL.into_iter().zip(&results) {
                    assert_eq!(
                        orders.len(),
                        4 + dials.discretionary_slots,
                        "{difficulty:?}, {stance:?}, seed {seed} did not preserve the four-order core before exercising attention"
                    );
                }
            }
        }
    }

    #[test]
    fn competent_difficulties_preserve_composition_across_queue_lifecycles() {
        fn lifecycle(
            difficulty: BotDifficulty,
            stance: BotStance,
            seed: u64,
            faction: Faction,
        ) -> Vec<Vec<(BuildingId, UnitKind)>> {
            let profile = BotConfig::scripted(difficulty, stance, seed).resolve_profile();
            let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
            let mut obs = observation();
            obs.faction = faction;
            for (id, kind) in [
                (8, BuildingKind::Foundry),
                (3, BuildingKind::Fabricator),
                (10, BuildingKind::Airworks),
                (5, BuildingKind::Crucible),
            ] {
                add_building(&mut obs, id, kind, Vec::new());
            }

            let mut next_unit_id = 100_u32;
            let mut transcript = Vec::new();
            for _ in 0..16 {
                let mut budget = obs.scrap;
                let mut intents = Vec::new();
                UtilityPolicy::new().adaptive_production(
                    &dials,
                    &obs,
                    adaptive_context(&[]),
                    &mut budget,
                    &mut intents,
                );
                let orders = train_intents(&intents);
                for (building, kind) in &orders {
                    let queue_index = obs
                        .my_buildings
                        .iter()
                        .position(|candidate| candidate.id == *building)
                        .expect("a production order names an observed factory");
                    obs.my_queues[queue_index].push(*kind);
                }

                let completed: Vec<_> = obs
                    .my_queues
                    .iter_mut()
                    .filter_map(|queue| (!queue.is_empty()).then(|| queue.remove(0)))
                    .collect();
                for kind in completed {
                    obs.my_units.push(unit(next_unit_id, 0, kind));
                    next_unit_id += 1;
                }
                transcript.push(orders);
            }
            transcript
        }

        let competent = [
            BotDifficulty::Standard,
            BotDifficulty::Veteran,
            BotDifficulty::Prime,
        ];
        for faction in [Faction::Ferrous, Faction::Cupric] {
            for stance in BotStance::ALL {
                for seed in [0, 1_616_200, 1_616_305, u64::MAX] {
                    let transcripts =
                        competent.map(|difficulty| lifecycle(difficulty, stance, seed, faction));
                    assert_eq!(
                        transcripts[0], transcripts[1],
                        "Veteran redealt {faction:?} {stance:?} seed {seed} production"
                    );
                    assert_eq!(
                        transcripts[1], transcripts[2],
                        "Prime redealt {faction:?} {stance:?} seed {seed} production"
                    );
                    let order_count = transcripts[0].iter().map(Vec::len).sum::<usize>();
                    let kinds = transcripts[0]
                        .iter()
                        .flatten()
                        .map(|(_, kind)| *kind)
                        .collect::<std::collections::BTreeSet<_>>();
                    assert!(
                        order_count >= 16,
                        "fixture did not exercise repeated queues"
                    );
                    assert!(kinds.len() >= 4, "fixture did not exercise composition");
                }
            }
        }
    }

    #[test]
    fn prior_orders_project_queue_roster_and_residual_budget() {
        let mut obs = observation();
        add_building(&mut obs, 1, BuildingKind::Fabricator, Vec::new());
        add_building(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        let aa = Role::AntiAir.unit_for(obs.faction);
        obs.my_units.extend([
            unit(10, 0, UnitKind::Bombard),
            unit(11, 0, UnitKind::Bombard),
            unit(12, 0, UnitKind::Warden),
            unit(13, 0, UnitKind::Warden),
            unit(14, 0, UnitKind::Warden),
            unit(15, 0, aa),
        ]);
        for patient in obs.my_units.iter_mut().take(2) {
            patient.hp = patient.kind.stats().max_hp / 2;
        }
        let mut dials = adaptive_dials(6);
        dials.support_target = 2;
        let mut intents = vec![Intent::TrainAt {
            building: BuildingId(1),
            kind: UnitKind::Tender,
        }];
        // One more Tender is affordable while retaining the fighting screen;
        // a third order is neither in the target nor in the residual bank.
        let mut budget = UnitKind::Tender.stats().cost + UnitKind::Sentinel.stats().cost;
        UtilityPolicy::new().adaptive_production(
            &dials,
            &obs,
            adaptive_context(&[]),
            &mut budget,
            &mut intents,
        );

        assert_eq!(
            train_intents(&intents)
                .iter()
                .filter(|(_, kind)| *kind == UnitKind::Tender)
                .count(),
            2
        );
        assert_eq!(budget, UnitKind::Sentinel.stats().cost);
        assert_eq!(planned_at(&intents, BuildingId(1)), 1);
        assert_eq!(planned_at(&intents, BuildingId(2)), 1);

        // An observed queue plus an earlier same-think order already reaches
        // depth two, so no discretionary order may enter this producer.
        let mut blocked = observation();
        let air_air = Role::AirAir.unit_for(blocked.faction);
        add_building(&mut blocked, 8, BuildingKind::Airworks, vec![air_air]);
        add_building(&mut blocked, 9, BuildingKind::Crucible, Vec::new());
        let bomber = Role::Bomber.unit_for(blocked.faction);
        let mut prior = vec![Intent::TrainAt {
            building: BuildingId(8),
            kind: bomber,
        }];
        let mut bank = 10_000;
        UtilityPolicy::new().adaptive_production(
            &dials,
            &blocked,
            adaptive_context(&[]),
            &mut bank,
            &mut prior,
        );
        assert_eq!(planned_at(&prior, BuildingId(8)), 1);
    }

    #[test]
    fn generic_production_leaves_air_cohorts_to_the_strategic_planner() {
        let mut obs = observation();
        add_building(&mut obs, 8, BuildingKind::Airworks, Vec::new());
        let dials = adaptive_dials(6);
        let mut budget = 10_000;
        let mut intents = Vec::new();

        UtilityPolicy::new().adaptive_production(
            &dials,
            &obs,
            adaptive_context(&[]),
            &mut budget,
            &mut intents,
        );

        assert_eq!(planned_at(&intents, BuildingId(8)), 1);
        assert!(intents.iter().all(|intent| matches!(
            intent,
            Intent::TrainAt { kind, .. }
                if matches!(kind.role(), Role::AirAir | Role::Interceptor)
        )));

        obs.my_queues[0] = vec![Role::AirGround.unit_for(obs.faction)];
        obs.my_units
            .push(unit(10, 0, Role::Bomber.unit_for(obs.faction)));
        intents.clear();
        UtilityPolicy::new().adaptive_production(
            &dials,
            &obs,
            adaptive_context(&[]),
            &mut budget,
            &mut intents,
        );
        assert!(
            intents.is_empty(),
            "utility must not top up a partial strategic screen or bomber cohort"
        );
    }

    #[test]
    fn no_deployable_ground_job_stops_only_the_repeatable_stream() {
        let mut obs = observation();
        add_building(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_building(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        obs.my_units
            .extend((10..14).map(|id| unit(id, 0, UnitKind::Harvester)));
        obs.my_units
            .extend((20..25).map(|id| unit(id, 0, UnitKind::Sentinel)));
        obs.my_units
            .extend((30..34).map(|id| unit(id, 0, UnitKind::Scuttler)));
        obs.my_units.push(unit(40, 0, UnitKind::Excavator));
        obs.my_units.extend([
            unit(50, 0, UnitKind::Bombard),
            unit(51, 0, UnitKind::Bombard),
            unit(52, 0, UnitKind::Tender),
            unit(53, 0, UnitKind::Warden),
            unit(54, 0, UnitKind::Warden),
            unit(55, 0, UnitKind::Warden),
            unit(56, 0, Role::AntiAir.unit_for(obs.faction)),
        ]);
        let dials = adaptive_dials(4);

        let schedule = |world: &Observation, allow_repeatable_ground, reserved: &[UnitId]| {
            let mut budget = world.scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().adaptive_production(
                &dials,
                world,
                adaptive_context(reserved).with_repeatable_ground(allow_repeatable_ground),
                &mut budget,
                &mut intents,
            );
            train_intents(&intents)
        };

        assert!(
            schedule(&obs, false, &[]).is_empty(),
            "a saturated roster must not keep buying idle ground bodies without a deployable job"
        );
        let deployed = schedule(&obs, true, &[]);
        assert_eq!(deployed.len(), 4);
        assert!(
            deployed
                .iter()
                .all(|(_, kind)| matches!(kind, UnitKind::Sentinel | UnitKind::Lancer)),
            "only the repeatable stream should differ: {deployed:?}"
        );
        assert!(deployed.iter().any(|(_, kind)| *kind == UnitKind::Sentinel));
        assert!(deployed.iter().any(|(_, kind)| *kind == UnitKind::Lancer));

        let core_schedule = |count: u32, reserved: &[UnitId]| {
            let mut core = observation();
            add_building(&mut core, 1, BuildingKind::Foundry, Vec::new());
            core.my_units
                .extend((0..count).map(|id| unit(100 + id, 0, UnitKind::Sentinel)));
            let mut core_dials = adaptive_dials(0);
            core_dials.army_size = 5;
            let mut budget = core.scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().adaptive_production(
                &core_dials,
                &core,
                adaptive_context(reserved).with_repeatable_ground(false),
                &mut budget,
                &mut intents,
            );
            train_intents(&intents)
        };
        assert_eq!(
            core_schedule(4, &[]),
            vec![(BuildingId(1), UnitKind::Sentinel)],
            "the finite minimum core must still replace one missing line unit"
        );
        assert_eq!(
            core_schedule(5, &[UnitId(100)]),
            vec![(BuildingId(1), UnitKind::Sentinel)],
            "a strategically reserved line unit must be replaced without reopening the repeatable stream"
        );

        obs.my_units.retain(|unit| unit.kind != UnitKind::Tender);
        assert_eq!(
            schedule(&obs, false, &[]),
            vec![(BuildingId(2), UnitKind::Tender)],
            "a missing bounded specialist remains justified without an ordinary ground objective"
        );
        obs.my_units.push(unit(52, 0, UnitKind::Tender));
        let anti_air = Role::AntiAir.unit_for(obs.faction);
        obs.my_units.retain(|unit| unit.kind != anti_air);
        assert_eq!(
            schedule(&obs, false, &[]),
            vec![(BuildingId(2), anti_air)],
            "a missing hard counter remains justified without reopening repeatable ground production"
        );
    }

    #[test]
    fn strategic_air_prelude_remains_the_only_owner_of_screen_and_bomber_orders() {
        let mut obs = observation();
        add_building(&mut obs, 8, BuildingKind::Airworks, Vec::new());
        let dials = adaptive_dials(6);
        let mut budget = 10_000;
        let mut intents = vec![
            Intent::TrainAt {
                building: BuildingId(8),
                kind: Role::AirGround.unit_for(obs.faction),
            },
            Intent::TrainAt {
                building: BuildingId(8),
                kind: Role::Bomber.unit_for(obs.faction),
            },
        ];
        let strategic = intents.clone();

        UtilityPolicy::new().adaptive_production(
            &dials,
            &obs,
            adaptive_context(&[]),
            &mut budget,
            &mut intents,
        );

        assert_eq!(intents, strategic);
        assert_eq!(budget, 10_000);
    }

    #[test]
    fn mixed_specialists_do_not_substitute_for_the_ordinary_ground_core() {
        let mut obs = observation();
        obs.my_units
            .extend((10..14).map(|id| unit(id, 0, UnitKind::Sentinel)));
        obs.my_units.extend([
            unit(20, 0, UnitKind::Scuttler),
            unit(21, 0, UnitKind::Scuttler),
            unit(22, 0, Role::AntiAir.unit_for(obs.faction)),
            unit(23, 0, Role::AntiAir.unit_for(obs.faction)),
            unit(24, 0, UnitKind::Buzzard),
            unit(25, 0, UnitKind::Kestrel),
        ]);
        add_building(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        let dials = adaptive_dials(4);
        let mut budget = UnitKind::Sentinel.stats().cost + UnitKind::Scuttler.stats().cost;
        let mut intents = Vec::new();

        UtilityPolicy::new().adaptive_production(
            &dials,
            &obs,
            adaptive_context(&[]),
            &mut budget,
            &mut intents,
        );

        assert_eq!(
            train_intents(&intents),
            vec![(BuildingId(1), UnitKind::Sentinel)],
            "the replay-derived four-Sentinel line must be completed before another specialist"
        );
        assert_eq!(budget, UnitKind::Scuttler.stats().cost);
        assert!(combat_core_status(&obs, &[], &intents, u64::from(dials.army_size)).ready);
    }

    #[test]
    fn core_projection_counts_queues_and_same_think_orders_but_not_reserved_units() {
        let mut obs = observation();
        obs.my_units
            .extend((10..13).map(|id| unit(id, 0, UnitKind::Sentinel)));
        add_building(&mut obs, 1, BuildingKind::Foundry, vec![UnitKind::Sentinel]);
        add_building(&mut obs, 2, BuildingKind::Foundry, Vec::new());
        let mut dials = adaptive_dials(0);
        dials.army_size = 5;
        let mut budget = UnitKind::Sentinel.stats().cost * 2;
        let mut intents = vec![Intent::TrainAt {
            building: BuildingId(1),
            kind: UnitKind::Sentinel,
        }];

        UtilityPolicy::new().adaptive_production(
            &dials,
            &obs,
            adaptive_context(&[UnitId(12)]),
            &mut budget,
            &mut intents,
        );

        assert_eq!(
            train_intents(&intents),
            vec![
                (BuildingId(1), UnitKind::Sentinel),
                (BuildingId(2), UnitKind::Sentinel),
            ]
        );
        assert_eq!(
            budget,
            UnitKind::Sentinel.stats().cost,
            "exactly one missing unreserved Sentinel-equivalent was purchased"
        );
    }

    #[test]
    fn combat_core_status_reports_empty_exact_and_saturated_boundaries() {
        let mut obs = observation();
        let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
        let sentinel_cost = UnitKind::Sentinel.stats().cost;

        let empty_floor = combat_core_status(&obs, &[], &[], 0);
        assert_eq!(
            empty_floor,
            CombatCoreStatus {
                projected_strength: 0,
                target_strength: 0,
                missing_strength: 0,
                missing_scrap: 0,
                ready: true,
            }
        );

        obs.my_units.push(unit(10, 0, UnitKind::Sentinel));
        let short = combat_core_status(&obs, &[], &[], 2);
        assert_eq!(short.projected_strength, sentinel_strength);
        assert_eq!(short.target_strength, sentinel_strength * 2);
        assert_eq!(short.missing_strength, sentinel_strength);
        assert_eq!(short.missing_scrap, sentinel_cost);
        assert!(!short.ready);

        add_building(&mut obs, 1, BuildingKind::Foundry, vec![UnitKind::Sentinel]);
        let exact = combat_core_status(&obs, &[], &[], 2);
        assert_eq!(exact.projected_strength, exact.target_strength);
        assert_eq!(exact.missing_strength, 0);
        assert_eq!(exact.missing_scrap, 0);
        assert!(exact.ready);

        let saturated = combat_core_status(&observation(), &[], &[], u64::MAX);
        assert_eq!(saturated.target_strength, u64::MAX);
        assert_eq!(saturated.missing_scrap, u32::MAX);
        assert!(!saturated.ready);
    }

    #[test]
    fn combat_core_status_hp_weights_every_line_hull_and_ignores_specialists() {
        let mut obs = observation();
        let mut wounded = unit(10, 0, UnitKind::Sentinel);
        wounded.hp /= 2;
        let warden = unit(11, 0, UnitKind::Warden);
        let breaker = unit(12, 0, UnitKind::Breaker);
        obs.my_units.extend([
            wounded.clone(),
            warden.clone(),
            breaker.clone(),
            unit(13, 0, UnitKind::Lancer),
            unit(14, 0, UnitKind::Bombard),
            unit(15, 0, UnitKind::Tender),
        ]);

        let expected = crate::bot::executive::unit_strength(&wounded)
            .saturating_add(crate::bot::executive::unit_strength(&warden))
            .saturating_add(crate::bot::executive::unit_strength(&breaker));
        let sentinel_strength = full_ground_strength(UnitKind::Sentinel);
        assert_eq!(
            crate::bot::executive::unit_strength(&wounded),
            sentinel_strength / 2,
            "a wounded line hull contributes only its remaining HP fraction"
        );

        let floor = expected / sentinel_strength + 1;
        let status = combat_core_status(&obs, &[], &[], floor);
        assert_eq!(status.projected_strength, expected);
        assert_eq!(status.missing_strength, status.target_strength - expected);
        assert_eq!(status.missing_scrap, UnitKind::Sentinel.stats().cost);
        assert!(!status.ready);

        let without_warden = combat_core_status(&obs, &[warden.id], &[], floor);
        assert_eq!(
            without_warden.projected_strength,
            expected - crate::bot::executive::unit_strength(&warden),
            "only the explicitly reserved line hull leaves the ordinary core"
        );
    }

    #[test]
    fn combat_core_status_counts_queues_and_each_same_think_order_once() {
        let mut obs = observation();
        let ordinary_army_member = unit(10, 0, UnitKind::Sentinel);
        let strategically_reserved = unit(11, 0, UnitKind::Warden);
        obs.my_units
            .extend([ordinary_army_member.clone(), strategically_reserved.clone()]);
        add_building(
            &mut obs,
            1,
            BuildingKind::Foundry,
            vec![UnitKind::Sentinel, UnitKind::Harvester],
        );
        add_building(
            &mut obs,
            2,
            BuildingKind::Fabricator,
            vec![UnitKind::Warden, UnitKind::Tender],
        );
        add_building(&mut obs, 3, BuildingKind::Crucible, Vec::new());
        let intents = vec![
            Intent::TrainAt {
                building: BuildingId(1),
                kind: UnitKind::Sentinel,
            },
            Intent::TrainAt {
                building: BuildingId(3),
                kind: UnitKind::Breaker,
            },
            Intent::FormArmy {
                staging: TilePos::new(3, 3),
                size: 8,
            },
        ];

        let expected = crate::bot::executive::unit_strength(&ordinary_army_member)
            .saturating_add(full_ground_strength(UnitKind::Sentinel))
            .saturating_add(full_ground_strength(UnitKind::Warden))
            .saturating_add(full_ground_strength(UnitKind::Sentinel))
            .saturating_add(full_ground_strength(UnitKind::Breaker));
        let status = combat_core_status(
            &obs,
            &[strategically_reserved.id, UnitId(u32::MAX)],
            &intents,
            100,
        );
        assert_eq!(status.projected_strength, expected);
        assert_eq!(status.missing_strength, status.target_strength - expected);
        assert_eq!(
            status.missing_scrap,
            u32::try_from(
                status
                    .missing_strength
                    .div_ceil(full_ground_strength(UnitKind::Sentinel))
            )
            .unwrap()
            .saturating_mul(UnitKind::Sentinel.stats().cost)
        );

        let including_ordinary_army = combat_core_status(&obs, &[], &intents, 100);
        assert_eq!(
            including_ordinary_army.projected_strength,
            expected.saturating_add(crate::bot::executive::unit_strength(
                &strategically_reserved
            )),
            "ordinary Executive armies count unless the caller explicitly reserves their units"
        );
    }

    #[test]
    fn full_foundry_queue_banks_a_partial_core_fund_until_capacity_opens() {
        let mut obs = observation();
        obs.my_units
            .extend((10..14).map(|id| unit(id, 0, UnitKind::Sentinel)));
        add_building(
            &mut obs,
            1,
            BuildingKind::Foundry,
            vec![UnitKind::Harvester, UnitKind::Harvester],
        );
        add_building(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        let dials = adaptive_dials(4);
        let partial_bank = UnitKind::Sentinel.stats().cost + UnitKind::Scuttler.stats().cost;

        let schedule = |world: &Observation| {
            let mut budget = partial_bank;
            let mut intents = Vec::new();
            UtilityPolicy::new().adaptive_production(
                &dials,
                world,
                adaptive_context(&[]),
                &mut budget,
                &mut intents,
            );
            (train_intents(&intents), budget)
        };

        assert_eq!(
            schedule(&obs),
            (Vec::new(), partial_bank),
            "a cheaper specialty cannot skim the missing line unit's fund"
        );

        obs.my_queues[0].pop();
        assert_eq!(
            schedule(&obs),
            (
                vec![(BuildingId(1), UnitKind::Sentinel)],
                UnitKind::Scuttler.stats().cost,
            ),
            "the held fund becomes a line unit as soon as its queue accepts work"
        );
    }

    #[test]
    fn queued_core_strength_matches_full_health_live_strength() {
        for kind in [UnitKind::Sentinel, UnitKind::Warden, UnitKind::Breaker] {
            let member = unit(10, 0, kind);
            assert_eq!(
                full_ground_strength(kind),
                crate::bot::executive::unit_strength(&member),
                "queued and live {kind:?} strength must use the same combat currency"
            );
            assert!(ordinary_core_unit(kind));
        }
        for kind in [
            UnitKind::Scuttler,
            UnitKind::Lancer,
            UnitKind::Bombard,
            UnitKind::Avalanche,
        ] {
            assert!(
                !ordinary_core_unit(kind),
                "{kind:?} needs an ordinary line rather than replacing it"
            );
        }
    }

    #[test]
    fn extra_foundry_gate_counts_unreserved_ordinary_strength_and_scales() {
        let mut obs = observation();
        obs.my_units
            .extend((10..15).map(|id| unit(id, 0, UnitKind::Sentinel)));

        assert!(
            extra_foundry_core_ready(&obs, &[], 1),
            "the common second Foundry must not require a prior army"
        );
        assert!(
            !extra_foundry_core_ready(&obs, &[], 2),
            "five Sentinel-equivalents are below the third-Foundry boundary"
        );

        obs.my_queues.push(vec![UnitKind::Lancer]);
        assert!(
            !extra_foundry_core_ready(&obs, &[], 2),
            "a specialist cannot substitute for the ordinary screen"
        );
        obs.my_queues[0] = vec![UnitKind::Sentinel];
        assert!(extra_foundry_core_ready(&obs, &[], 2));
        assert!(
            !extra_foundry_core_ready(&obs, &[UnitId(10)], 2),
            "strategically reserved fighters are not the available home screen"
        );

        obs.my_units
            .extend((15..20).map(|id| unit(id, 0, UnitKind::Sentinel)));
        assert!(
            !extra_foundry_core_ready(&obs, &[], 3),
            "eleven Sentinel-equivalents are below the fourth-Foundry boundary"
        );
        obs.my_units.push(unit(20, 0, UnitKind::Sentinel));
        assert!(
            extra_foundry_core_ready(&obs, &[], 3),
            "the fourth Foundry unlocks at exactly twelve Sentinel-equivalents"
        );
    }

    #[test]
    fn finite_bank_higher_rungs_extend_the_same_core_prefix() {
        let mut obs = observation();
        obs.my_units
            .extend((10..13).map(|id| unit(id, 0, UnitKind::Sentinel)));
        for (id, kind) in [
            (1, BuildingKind::Foundry),
            (2, BuildingKind::Foundry),
            (3, BuildingKind::Fabricator),
            (4, BuildingKind::Airworks),
            (5, BuildingKind::Crucible),
        ] {
            add_building(&mut obs, id, kind, Vec::new());
        }
        obs.scrap = 900;

        let schedules: Vec<_> = BotDifficulty::ALL
            .into_iter()
            .map(|difficulty| {
                let profile = BotConfig::scripted(difficulty, BotStance::Balanced, 1_616_201)
                    .resolve_profile();
                let dials = Dials::scripted(&profile, DifficultyTuning::for_level(difficulty));
                let mut budget = obs.scrap;
                let mut intents = Vec::new();
                UtilityPolicy::new().adaptive_production(
                    &dials,
                    &obs,
                    adaptive_context(&[]),
                    &mut budget,
                    &mut intents,
                );
                (dials, train_intents(&intents), budget)
            })
            .collect();

        for (dials, orders, _) in &schedules {
            assert_eq!(
                &orders[..2],
                &[
                    (BuildingId(1), UnitKind::Sentinel),
                    (BuildingId(2), UnitKind::Sentinel),
                ],
                "every rung closes the same core before specialties: {orders:?}"
            );
            let intents: Vec<_> = orders
                .iter()
                .map(|(building, kind)| Intent::TrainAt {
                    building: *building,
                    kind: *kind,
                })
                .collect();
            assert!(combat_core_status(&obs, &[], &intents, u64::from(dials.army_size)).ready);
            assert!(
                orders
                    .iter()
                    .all(|(_, kind)| { !matches!(kind.role(), Role::AirGround | Role::Bomber) })
            );
        }
        for pair in schedules.windows(2) {
            let [(_, lower, _), (_, higher, _)] = pair else {
                unreachable!()
            };
            assert_eq!(
                lower.as_slice(),
                &higher[..lower.len()],
                "higher attention changed a lower rung's finite-bank production prefix"
            );
        }
        assert!(
            schedules
                .iter()
                .any(|(dials, orders, _)| orders.len() < 2 + dials.discretionary_slots),
            "the fixture must actually exercise a finite-bank or finite-queue boundary"
        );
    }

    #[test]
    fn ordinary_core_precedes_raiders_and_leaves_capital_banked() {
        let mut obs = observation();
        for id in 0..4 {
            obs.my_units.push(unit(id, 0, UnitKind::Harvester));
        }
        for id in 10..13 {
            obs.my_units.push(unit(id, 0, UnitKind::Sentinel));
        }
        add_building(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_building(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        let dials = adaptive_dials(6);
        let capital = BuildingKind::Airworks
            .base_stats()
            .construction
            .expect("Airworks has a build price")
            .cost
            + TECH_RESERVE;
        obs.scrap = UnitKind::Sentinel.stats().cost
            + UnitKind::Scuttler.stats().cost
            + UnitKind::Sentinel.stats().cost
            + capital;

        let mut policy = UtilityPolicy::new();
        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        policy.production(
            &dials,
            &obs,
            TilePos::new(2, 2),
            ConstructionClaims {
                player_facing: true,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );

        assert_eq!(
            train_intents(&intents)
                .iter()
                .filter(|(_, kind)| *kind == UnitKind::Sentinel)
                .count(),
            2,
            "the ordinary drip and core pass close the five-Sentinel commitment"
        );
        assert!(
            train_intents(&intents)
                .iter()
                .all(|(_, kind)| *kind != UnitKind::Scuttler),
            "a raider must not displace the unfinished ordinary line"
        );
        assert_eq!(budget, capital + UnitKind::Scuttler.stats().cost);
    }

    #[test]
    fn adaptive_production_does_not_double_reserve_a_walking_founder() {
        let mut obs = observation();
        obs.my_units.extend(
            (0..4)
                .map(|id| unit(id, 0, UnitKind::Harvester))
                .chain((10..13).map(|id| unit(id, 0, UnitKind::Sentinel))),
        );
        for (id, kind) in [
            (1, BuildingKind::Foundry),
            (2, BuildingKind::Foundry),
            (3, BuildingKind::Fabricator),
            (4, BuildingKind::Airworks),
            (5, BuildingKind::Crucible),
        ] {
            add_building(&mut obs, id, kind, Vec::new());
        }
        let promised = TilePos::new(8, 5);
        obs.my_units[0].idle = false;
        obs.my_units[0].founding = Some((BuildingKind::Foundry, promised));
        obs.known_scrap = vec![(TilePos::new(23, 13), 500)];
        let mut dials = adaptive_dials(6);
        dials.expansion = true;
        dials.foundry_cap = 4;
        let foundry_cost = BuildingKind::Foundry
            .base_stats()
            .construction
            .expect("Foundries have a build price")
            .cost;
        let capital = foundry_cost + TECH_RESERVE;
        obs.scrap = foundry_cost + UnitKind::Sentinel.stats().cost;

        let run = |world: &Observation| {
            let mut budget = world
                .scrap
                .saturating_sub(UtilityPolicy::deferred_construction_commitment(world));
            let mut intents = Vec::new();
            UtilityPolicy::new().production(
                &dials,
                world,
                TilePos::new(3, 2),
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                &mut budget,
                &mut intents,
            );
            (train_intents(&intents), budget)
        };
        let (walking_orders, walking_budget) = run(&obs);
        assert_eq!(
            walking_orders,
            vec![(BuildingId(1), UnitKind::Sentinel)],
            "a walking founder's single escrow must not freeze the remaining fighting reserve"
        );
        assert_eq!(
            walking_budget, 0,
            "only scrap outside the walking Foundry's escrow may be spent"
        );

        obs.my_units[0].founding = None;
        let mut third = building(6, BuildingKind::Foundry);
        third.anchor = promised;
        obs.my_buildings.push(third);
        obs.my_queues.push(Vec::new());
        obs.my_units
            .extend((13..22).map(|id| unit(id, 0, UnitKind::Sentinel)));
        obs.scrap = capital;
        assert_eq!(
            run(&obs),
            (Vec::new(), capital),
            "a twelve-Sentinel-equivalent screen unlocks the fourth Foundry's fresh bank"
        );
    }

    #[test]
    fn producer_order_does_not_change_canonical_output() {
        let mut left = observation();
        for id in 0..4 {
            left.my_units.push(unit(id, 0, UnitKind::Harvester));
        }
        for (id, kind, queue) in [
            (9, BuildingKind::Fabricator, vec![UnitKind::Tender]),
            (2, BuildingKind::Foundry, Vec::new()),
            (7, BuildingKind::Airworks, Vec::new()),
            (5, BuildingKind::Crucible, Vec::new()),
        ] {
            add_building(&mut left, id, kind, queue);
        }
        let mut right = left.clone();
        right.my_buildings.reverse();
        right.my_queues.reverse();
        let dials = adaptive_dials(6);

        let run = |obs: &Observation| {
            let mut policy = UtilityPolicy::new();
            let mut budget = obs.scrap;
            let mut intents = Vec::new();
            policy.production(
                &dials,
                obs,
                TilePos::new(2, 2),
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                &mut budget,
                &mut intents,
            );
            (train_intents(&intents), budget)
        };
        assert_eq!(run(&left), run(&right));
    }

    #[test]
    fn adaptive_traits_do_not_displace_hard_air_or_turret_answers() {
        let mut obs = observation();
        for id in 0..4 {
            obs.my_units.push(unit(id, 0, UnitKind::Harvester));
        }
        add_building(&mut obs, 1, BuildingKind::Foundry, Vec::new());
        add_building(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        obs.enemy_units.push(unit(90, 1, UnitKind::Darter));

        let mut narrow = adaptive_dials(2);
        narrow.siege_target = 1;
        narrow.support_target = 1;
        narrow.raider_target = 2;
        let mut broad = narrow.clone();
        broad.siege_target = 4;
        broad.support_target = 3;
        broad.raider_target = 2;

        let hard_order = |dials: &Dials, world: &Observation| {
            let mut policy = UtilityPolicy::new();
            let mut budget = world.scrap;
            let mut intents = Vec::new();
            policy.production(
                dials,
                world,
                TilePos::new(2, 2),
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                &mut budget,
                &mut intents,
            );
            train_intents(&intents)
                .into_iter()
                .find(|(building, kind)| {
                    *building == BuildingId(2)
                        && (*kind == Role::AntiAir.unit_for(world.faction)
                            || *kind == UnitKind::Lancer)
                })
                .expect("the hard response is emitted")
        };
        assert_eq!(hard_order(&narrow, &obs), hard_order(&broad, &obs));

        obs.enemy_units.clear();
        obs.enemy_buildings.push(BuildingObs {
            player: PlayerId(1),
            ..building(91, BuildingKind::Turret)
        });
        assert_eq!(hard_order(&narrow, &obs), (BuildingId(2), UnitKind::Lancer));
        assert_eq!(hard_order(&broad, &obs), (BuildingId(2), UnitKind::Lancer));
    }

    #[test]
    fn higher_siege_and_support_targets_use_additional_factory_capacity() {
        let mut obs = observation();
        for (id, kind) in [
            (1, BuildingKind::Fabricator),
            (2, BuildingKind::Fabricator),
            (3, BuildingKind::Crucible),
            (4, BuildingKind::Crucible),
        ] {
            add_building(&mut obs, id, kind, Vec::new());
        }
        obs.my_units.extend([
            unit(10, 0, UnitKind::Bombard),
            unit(11, 0, UnitKind::Tender),
            unit(12, 0, UnitKind::Warden),
            unit(13, 0, UnitKind::Warden),
            unit(14, 0, UnitKind::Warden),
            unit(15, 0, Role::AntiAir.unit_for(obs.faction)),
            unit(16, 0, UnitKind::Avalanche),
            unit(17, 0, UnitKind::Breaker),
            unit(18, 0, UnitKind::Breaker),
        ]);
        for patient in obs
            .my_units
            .iter_mut()
            .filter(|unit| unit.kind != UnitKind::Tender)
            .take(2)
        {
            patient.hp = patient.kind.stats().max_hp / 2;
        }

        let train = |siege_target, support_target| {
            let mut dials = adaptive_dials(4);
            dials.siege_target = siege_target;
            dials.support_target = support_target;
            let mut budget = obs.scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().adaptive_production(
                &dials,
                &obs,
                adaptive_context(&[]),
                &mut budget,
                &mut intents,
            );
            train_intents(&intents)
        };

        let narrow = train(1, 1);
        assert_eq!(narrow.len(), 4);
        assert!(narrow.iter().all(|(_, kind)| *kind == UnitKind::Lancer));
        let broad = train(4, 3);
        assert_eq!(broad.len(), 4);
        assert_eq!(
            broad
                .iter()
                .filter(|(_, kind)| matches!(kind, UnitKind::Bombard | UnitKind::Avalanche))
                .count(),
            3
        );
        assert!(broad.contains(&(BuildingId(2), UnitKind::Tender)));
    }

    #[test]
    fn resolved_siege_identity_takes_an_earlier_second_artillery_opportunity() {
        let mut obs = observation();
        add_building(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        let (high_profile, high) = resolved_dials(20_043);
        let (low_profile, low) = resolved_dials(20_042);
        assert_eq!(
            (high_profile.primary, high.siege_target),
            (Specialty::Siege, 3)
        );
        assert_eq!(low.siege_target, 1, "premise: {low_profile:?}");

        let schedule = |dials: &Dials| {
            let mut budget = obs.scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().adaptive_production(
                dials,
                &obs,
                adaptive_context(&[]),
                &mut budget,
                &mut intents,
            );
            train_intents(&intents)
        };

        assert_eq!(
            schedule(&high),
            vec![
                (BuildingId(2), UnitKind::Bombard),
                (BuildingId(2), UnitKind::Bombard),
            ]
        );
        assert!(
            schedule(&low)
                .iter()
                .filter(|(_, kind)| *kind == UnitKind::Bombard)
                .count()
                <= 1,
            "low Siege must retain the ordinary mixed-roster order"
        );
    }

    #[test]
    fn resolved_support_identity_takes_an_earlier_second_tender_opportunity() {
        let mut obs = observation();
        add_building(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        for id in 10..13 {
            let mut patient = unit(id, 0, UnitKind::Sentinel);
            patient.hp = patient.kind.stats().max_hp / 2;
            obs.my_units.push(patient);
        }
        let (high_profile, high) = resolved_dials(20_042);
        let (low_profile, low) = resolved_dials(20_044);
        assert_eq!(
            (high_profile.primary, high.support_target),
            (Specialty::Support, 3)
        );
        assert_eq!(low.support_target, 1, "premise: {low_profile:?}");

        let schedule = |dials: &Dials| {
            let mut budget = obs.scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().adaptive_production(
                dials,
                &obs,
                adaptive_context(&[]),
                &mut budget,
                &mut intents,
            );
            train_intents(&intents)
        };

        assert_eq!(
            schedule(&high),
            vec![
                (BuildingId(2), UnitKind::Tender),
                (BuildingId(2), UnitKind::Tender),
            ]
        );
        assert!(
            schedule(&low)
                .iter()
                .filter(|(_, kind)| *kind == UnitKind::Tender)
                .count()
                <= 1,
            "low Support must retain the ordinary mixed-roster order"
        );
    }

    #[test]
    fn support_target_adds_one_tender_per_distinct_reachable_patient() {
        let mut obs = observation();
        add_building(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        let mut dials = adaptive_dials(4);
        dials.support_target = 3;

        assert_eq!(current_support_target(&dials, &obs), 1);
        let mut first = unit(10, 0, UnitKind::Sentinel);
        first.hp = first.kind.stats().max_hp / 2;
        obs.my_units.push(first);
        assert_eq!(current_support_target(&dials, &obs), 2);

        let mut second = unit(11, 0, UnitKind::Bombard);
        second.hp = second.kind.stats().max_hp / 2;
        obs.my_units.push(second);
        assert_eq!(current_support_target(&dials, &obs), 3);

        let mut capped = unit(12, 0, UnitKind::Warden);
        capped.hp = capped.kind.stats().max_hp / 2;
        obs.my_units.push(capped);
        assert_eq!(current_support_target(&dials, &obs), 3);

        for patient in &mut obs.my_units {
            patient.hp = patient.kind.stats().max_hp;
        }
        let mut unreachable = unit(13, 0, UnitKind::Sentinel);
        unreachable.tile = TilePos::new(18, 6);
        unreachable.hp = unreachable.kind.stats().max_hp / 2;
        obs.my_units.push(unreachable);
        obs.known_rock = (0..obs.map_height).map(|y| TilePos::new(12, y)).collect();
        assert_eq!(
            current_support_target(&dials, &obs),
            1,
            "damage across a known severance cannot justify support production on this component"
        );
    }

    #[test]
    fn support_commitments_count_live_queued_and_planned_tenders_once() {
        let mut base = observation();
        add_building(&mut base, 1, BuildingKind::Fabricator, Vec::new());
        add_building(&mut base, 2, BuildingKind::Fabricator, Vec::new());
        base.my_units.extend([
            unit(10, 0, UnitKind::Bombard),
            unit(11, 0, UnitKind::Warden),
            unit(12, 0, UnitKind::Warden),
            unit(13, 0, UnitKind::Warden),
            unit(14, 0, Role::AntiAir.unit_for(base.faction)),
        ]);
        for patient in base
            .my_units
            .iter_mut()
            .filter(|unit| matches!(unit.kind, UnitKind::Warden))
            .take(2)
        {
            patient.hp = patient.kind.stats().max_hp / 2;
        }

        let mut dials = adaptive_dials(8);
        dials.siege_target = 1;
        dials.support_target = 3;
        let tender = Role::Tender.unit_for(base.faction);
        let schedule = |world: &Observation, prior: Vec<Intent>| {
            let prior_len = prior.len();
            let mut intents = prior;
            let mut budget = 20_000;
            UtilityPolicy::new().adaptive_production(
                &dials,
                world,
                adaptive_context(&[]).with_repeatable_ground(false),
                &mut budget,
                &mut intents,
            );
            intents[prior_len..]
                .iter()
                .filter(|intent| matches!(intent, Intent::TrainAt { kind, .. } if *kind == tender))
                .count()
        };

        assert_eq!(schedule(&base, Vec::new()), 3);

        let mut one_live = base.clone();
        one_live.my_units.push(unit(20, 0, tender));
        assert_eq!(schedule(&one_live, Vec::new()), 2);

        let mut one_queued = base.clone();
        one_queued.my_queues[0].push(tender);
        assert_eq!(schedule(&one_queued, Vec::new()), 2);

        let prior = vec![Intent::TrainAt {
            building: BuildingId(1),
            kind: tender,
        }];
        assert_eq!(schedule(&base, prior.clone()), 2);

        let mut two_observed = one_live;
        two_observed.my_queues[0].push(tender);
        assert_eq!(schedule(&two_observed, Vec::new()), 1);
        assert_eq!(schedule(&two_observed, prior), 0);

        two_observed
            .my_units
            .retain(|unit| unit.kind.role() != Role::Tender);
        assert_eq!(
            schedule(&two_observed, Vec::new()),
            2,
            "a queued Tender remains counted while a lost live Tender is replaced"
        );

        for patient in &mut two_observed.my_units {
            patient.hp = patient.kind.stats().max_hp;
        }
        assert_eq!(
            schedule(&two_observed, Vec::new()),
            0,
            "when the damage disappears, the queued baseline releases every extra support slot"
        );
    }

    #[test]
    fn personalities_share_the_worker_bootstrap_before_renewable_income() {
        let high_greed = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_304)
            .resolve_profile();
        let low_greed = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_305)
            .resolve_profile();
        let tuning = DifficultyTuning::for_level(BotDifficulty::Prime);
        let high = Dials::scripted(&high_greed, tuning);
        let low = Dials::scripted(&low_greed, tuning);
        assert_eq!((high.harvester_target, low.harvester_target), (5, 4));

        let schedule = |dials: &Dials, workers: u32, renewable| {
            let mut obs = observation();
            add_building(&mut obs, 1, BuildingKind::Foundry, Vec::new());
            if renewable {
                add_building(&mut obs, 2, BuildingKind::Extractor, Vec::new());
            }
            obs.my_units
                .extend((0..workers).map(|id| unit(10 + id, 0, UnitKind::Harvester)));
            obs.my_units
                .extend((0..dials.army_size).map(|id| unit(30 + id, 0, UnitKind::Sentinel)));
            obs.my_units.extend(
                (0..u32::try_from(dials.raider_target).unwrap())
                    .map(|id| unit(50 + id, 0, UnitKind::Scuttler)),
            );
            obs.my_units.push(unit(60, 0, UnitKind::Excavator));

            let mut budget = obs.scrap;
            let mut intents = Vec::new();
            UtilityPolicy::new().production(
                dials,
                &obs,
                TilePos::new(2, 2),
                ConstructionClaims {
                    player_facing: true,
                    enlisted: &[],
                    reserved: &[],
                },
                &mut budget,
                &mut intents,
            );
            train_intents(&intents)
        };

        let shared = vec![(BuildingId(1), UnitKind::Harvester)];
        assert_eq!(schedule(&high, 3, false), shared);
        assert_eq!(schedule(&low, 3, false), shared);
        assert_eq!(
            schedule(&high, 4, true),
            vec![(BuildingId(1), UnitKind::Harvester)],
            "Greed may buy its fifth worker after renewable income stands"
        );
        assert!(
            schedule(&low, 4, true).is_empty(),
            "the lower appetite is already satisfied at the shared floor"
        );
    }

    #[test]
    fn bounded_personality_signature_precedes_repeatable_line_regardless_of_producer_id() {
        let profile = BotConfig::scripted(BotDifficulty::Prime, BotStance::Balanced, 1_616_305)
            .resolve_profile();
        let dials = Dials::scripted(&profile, DifficultyTuning::for_level(BotDifficulty::Prime));
        assert_eq!(profile.primary, Specialty::Support);
        assert_eq!(dials.support_target, 2);

        let schedule = |foundry_id, fabricator_id| {
            let mut obs = observation();
            add_building(&mut obs, foundry_id, BuildingKind::Foundry, Vec::new());
            add_building(
                &mut obs,
                fabricator_id,
                BuildingKind::Fabricator,
                Vec::new(),
            );
            obs.my_units.extend(
                (0..dials.harvester_target)
                    .map(|id| unit(10 + id, 0, UnitKind::Harvester))
                    .chain((0..dials.army_size).map(|id| unit(30 + id, 0, UnitKind::Sentinel)))
                    .chain(
                        (0..u32::try_from(dials.raider_target).unwrap())
                            .map(|id| unit(50 + id, 0, UnitKind::Scuttler)),
                    ),
            );
            obs.my_units.push(unit(60, 0, UnitKind::Excavator));
            obs.my_units.extend([
                unit(70, 0, UnitKind::Bombard),
                unit(71, 0, UnitKind::Bombard),
                unit(72, 0, UnitKind::Tender),
                unit(73, 0, UnitKind::Warden),
                unit(74, 0, UnitKind::Warden),
                unit(75, 0, UnitKind::Warden),
                unit(76, 0, Role::AntiAir.unit_for(obs.faction)),
            ]);
            for patient in obs
                .my_units
                .iter_mut()
                .filter(|unit| unit.kind == UnitKind::Sentinel)
                .take(2)
            {
                patient.hp = patient.kind.stats().max_hp / 2;
            }

            let mut budget = UnitKind::Tender.stats().cost + UnitKind::Sentinel.stats().cost;
            let mut intents = Vec::new();
            UtilityPolicy::new().adaptive_production(
                &dials,
                &obs,
                adaptive_context(&[]),
                &mut budget,
                &mut intents,
            );
            (train_intents(&intents), budget)
        };

        for (foundry, fabricator) in [(1, 2), (2, 1)] {
            assert_eq!(
                schedule(foundry, fabricator),
                (
                    vec![
                        (BuildingId(fabricator), UnitKind::Tender),
                        (BuildingId(foundry), UnitKind::Sentinel),
                    ],
                    0,
                ),
                "the finite Support signature must not depend on which producer was authored first"
            );
        }
    }

    #[test]
    fn full_production_path_saves_for_support_before_buying_repeatable_infantry() {
        let (obs, dials) = support_savings_fixture(1, 2);
        let tender = Role::Tender.unit_for(obs.faction);
        let sentinel_cost = UnitKind::Sentinel.stats().cost;
        let tender_fund = tender.stats().cost;

        for partial in 0..tender_fund + sentinel_cost {
            assert_eq!(
                full_production_schedule(&dials, &obs, partial),
                (Vec::new(), partial),
                "partial specialist fund {partial} leaked into a cheaper line purchase"
            );
        }
        assert_eq!(
            full_production_schedule(&dials, &obs, tender_fund + sentinel_cost),
            (
                vec![(BuildingId(2), tender), (BuildingId(1), UnitKind::Sentinel),],
                0,
            ),
            "once the specialist and fighting reserve are funded, the bounded Support signature precedes the recurring line"
        );
    }

    #[test]
    fn healed_patients_release_the_second_tender_fund_to_the_fighting_line() {
        let (mut obs, dials) = support_savings_fixture(1, 2);
        for patient in &mut obs.my_units {
            patient.hp = patient.kind.stats().max_hp;
        }
        let budget = UnitKind::Sentinel.stats().cost.saturating_mul(2);

        assert_eq!(
            full_production_schedule(&dials, &obs, budget),
            (
                vec![
                    (BuildingId(1), UnitKind::Sentinel),
                    (BuildingId(1), UnitKind::Sentinel),
                ],
                0,
            ),
            "an obsolete specialist goal must not keep a finite bank from both ordinary lines"
        );
    }

    #[test]
    fn support_savings_survive_observation_boundaries_without_duplicate_specialists() {
        for (foundry_id, fabricator_id) in [(1, 2), (2, 1)] {
            let (mut obs, dials) = support_savings_fixture(foundry_id, fabricator_id);
            let tender = Role::Tender.unit_for(obs.faction);
            let tender_cost = tender.stats().cost;
            let sentinel_cost = UnitKind::Sentinel.stats().cost;

            let partial_fund = tender_cost + sentinel_cost - 1;
            assert_eq!(
                full_production_schedule(&dials, &obs, partial_fund),
                (Vec::new(), partial_fund),
                "a nearly complete specialist fund must survive one observation intact"
            );

            let funded = full_production_schedule(&dials, &obs, tender_cost + sentinel_cost);
            assert_eq!(
                funded,
                (
                    vec![
                        (BuildingId(fabricator_id), tender),
                        (BuildingId(foundry_id), UnitKind::Sentinel),
                    ],
                    0,
                ),
                "the newly affordable bounded specialist must be scheduled exactly once before recurring infantry"
            );

            let foundry_index = obs
                .my_buildings
                .iter()
                .position(|building| building.id == BuildingId(foundry_id))
                .unwrap();
            let fabricator_index = obs
                .my_buildings
                .iter()
                .position(|building| building.id == BuildingId(fabricator_id))
                .unwrap();
            obs.my_queues[foundry_index].push(UnitKind::Sentinel);
            obs.my_queues[fabricator_index].push(tender);
            obs.tick += 24;

            let lancer_cost = UnitKind::Lancer.stats().cost;
            let repeatable_fund = lancer_cost + sentinel_cost.saturating_mul(2);
            let (next_orders, remaining) = full_production_schedule(&dials, &obs, repeatable_fund);
            let expected = if foundry_id < fabricator_id {
                vec![
                    (BuildingId(foundry_id), UnitKind::Sentinel),
                    (BuildingId(fabricator_id), UnitKind::Lancer),
                ]
            } else {
                vec![
                    (BuildingId(fabricator_id), UnitKind::Lancer),
                    (BuildingId(foundry_id), UnitKind::Sentinel),
                ]
            };
            assert_eq!(
                next_orders, expected,
                "an observed queued Tender must satisfy the bounded target while both recurring lines resume in canonical producer order"
            );
            assert_eq!(remaining, sentinel_cost);
            assert!(
                next_orders.iter().all(|(_, kind)| *kind != tender),
                "the next observation must not schedule a duplicate Tender"
            );
        }
    }

    #[test]
    fn allies_reduce_elective_saturation_but_cannot_erase_an_owned_floor() {
        let mut obs = observation();
        add_building(&mut obs, 2, BuildingKind::Fabricator, Vec::new());
        let aa = Role::AntiAir.unit_for(obs.faction);
        obs.my_units.extend([
            unit(10, 0, UnitKind::Bombard),
            unit(11, 0, UnitKind::Tender),
            unit(12, 0, UnitKind::Warden),
            unit(13, 0, UnitKind::Warden),
            unit(14, 0, UnitKind::Warden),
            unit(15, 0, aa),
        ]);
        let mut dials = adaptive_dials(1);
        dials.siege_target = 2;
        dials.support_target = 1;

        let choose = |world: &Observation| {
            let mut intents = Vec::new();
            let mut budget = 10_000;
            UtilityPolicy::new().adaptive_production(
                &dials,
                world,
                adaptive_context(&[]),
                &mut budget,
                &mut intents,
            );
            train_intents(&intents)[0].1
        };
        assert_eq!(choose(&obs), UnitKind::Bombard);

        obs.ally_units.extend([
            unit(80, 1, UnitKind::Bombard),
            unit(81, 1, UnitKind::Bombard),
        ]);
        assert_eq!(
            choose(&obs),
            UnitKind::Lancer,
            "two allied guns count as one elective gun, saturating the team target"
        );

        obs.my_units.retain(|unit| unit.kind != UnitKind::Bombard);
        obs.ally_units.extend([
            unit(82, 1, UnitKind::Bombard),
            unit(83, 1, UnitKind::Bombard),
        ]);
        assert_eq!(
            choose(&obs),
            UnitKind::Bombard,
            "an ally can complement the roster but cannot remove the bot's own siege floor"
        );
    }

    #[test]
    fn legacy_mode_keeps_secondary_factories_out_of_the_old_policy() {
        let mut obs = observation();
        for id in 0..5 {
            obs.my_units.push(unit(id, 0, UnitKind::Harvester));
        }
        add_building(&mut obs, 10, BuildingKind::Foundry, Vec::new());
        add_building(&mut obs, 2, BuildingKind::Foundry, Vec::new());
        add_building(&mut obs, 11, BuildingKind::Fabricator, Vec::new());
        add_building(&mut obs, 3, BuildingKind::Fabricator, Vec::new());
        let mut budget = obs.scrap;
        let mut intents = Vec::new();
        UtilityPolicy::new().production(
            &Dials::overseer(),
            &obs,
            TilePos::new(2, 2),
            ConstructionClaims {
                player_facing: false,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );
        let trains = train_intents(&intents);
        assert!(
            trains
                .iter()
                .all(|(id, _)| { *id != BuildingId(10) && *id != BuildingId(11) })
        );
    }

    #[test]
    fn profile_free_policy_retains_its_legacy_air_production() {
        let mut obs = observation();
        obs.my_units.extend(
            (0..5)
                .map(|id| unit(id, 0, UnitKind::Harvester))
                .chain((10..13).map(|id| unit(id, 0, UnitKind::Sentinel))),
        );
        for (id, kind) in [
            (1, BuildingKind::Foundry),
            (2, BuildingKind::Fabricator),
            (3, BuildingKind::Airworks),
            (4, BuildingKind::Crucible),
        ] {
            add_building(&mut obs, id, kind, Vec::new());
        }
        obs.enemy_buildings.push(BuildingObs {
            player: PlayerId(1),
            ..building(90, BuildingKind::Foundry)
        });
        let mut budget = obs.scrap;
        let mut intents = Vec::new();

        UtilityPolicy::new().production(
            &Dials::overseer(),
            &obs,
            TilePos::new(2, 2),
            ConstructionClaims {
                player_facing: false,
                enlisted: &[],
                reserved: &[],
            },
            &mut budget,
            &mut intents,
        );

        let air: Vec<_> = train_intents(&intents)
            .into_iter()
            .filter(|(building, _)| *building == BuildingId(3))
            .map(|(_, kind)| kind.role())
            .collect();
        assert!(
            air.contains(&Role::Bomber),
            "legacy bomber drip changed: {air:?}"
        );
        assert!(
            air.contains(&Role::AirGround),
            "legacy harassment wing changed: {air:?}"
        );
    }
}
