//! What the army needs: deficits by role, from what the seat has seen of the
//! enemy and what it owns, and the unit that best fills a role at a producer.
//! Choices use coarse suitability, never a combat simulation.

use crate::memory::{Memory, SeenUnit};
use crate::profile::PersonalityTraits;
use chassis::fx::Fx;
use chassis::grid::TilePos;
use oxide_sim::observation::ObservationData;
use oxide_sim::scenario::BotStance;
use oxide_sim::stats::Domain;
use oxide_sim::{BuildingKind, UnitKind};
use std::cmp::Reverse;

/// The part of the army a unit fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// Direct-fire ground fighters.
    Line,
    /// Long-range and anti-structure ground fire.
    Siege,
    /// Anything built to shoot down aircraft.
    AntiAir,
    /// Aircraft that attack the ground.
    AirStrike,
}

const ROLES: [Role; 4] = [Role::Line, Role::Siege, Role::AntiAir, Role::AirStrike];

/// What a needed but untrainable role adds to its cheapest building's score.
const PULL: u32 = 600;

/// Per mille: the most a premium unit's suitability over the cheaper one
/// multiplies saving for it, a computation bound on an otherwise unbounded
/// ratio.
const PREMIUM_CAP: u64 = 4_000;

/// Enemy buildings that shoot back.
const DEFENSES: [BuildingKind; 3] = [
    BuildingKind::Turret,
    BuildingKind::FlakTurret,
    BuildingKind::Bastion,
];

/// The role a unit fills. Harvesters, raiders, support, scouts and transports
/// fill none here. Every unit kind is listed, so a new one must be placed
/// before the bot can field it.
pub(crate) fn role(kind: UnitKind) -> Option<Role> {
    match kind {
        UnitKind::Sentinel | UnitKind::Warden | UnitKind::Breaker => Some(Role::Line),
        UnitKind::Lancer | UnitKind::Bombard | UnitKind::Avalanche => Some(Role::Siege),
        UnitKind::Flakhound | UnitKind::Talon | UnitKind::Shrike => Some(Role::AntiAir),
        UnitKind::Buzzard | UnitKind::Condor => Some(Role::AirStrike),
        UnitKind::Harvester
        | UnitKind::Excavator
        | UnitKind::Scuttler
        | UnitKind::Sapper
        | UnitKind::Tender
        | UnitKind::Kestrel
        | UnitKind::Skyhook => None,
    }
}

/// How far a unit's longest weapon reaches: under three tiles, up to five,
/// or beyond.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Reach {
    Short,
    Medium,
    Long,
}

fn reach(kind: UnitKind) -> Option<Reach> {
    let range = kind
        .stats()
        .weapons
        .iter()
        .map(|weapon| weapon.range)
        .max()?;
    Some(if range < Fx::from_num(3) {
        Reach::Short
    } else if range <= Fx::from_num(5) {
        Reach::Medium
    } else {
        Reach::Long
    })
}

/// What the seat knows of the enemy army, weighted by confidence.
struct Enemy {
    air: u64,
    ground: u64,
    defenses: u64,
    reach: Option<Reach>,
    clustered: bool,
    /// The most recently seen enemy ground units, by tile and full health.
    targets: Vec<(TilePos, u32)>,
}

/// Remembered enemy ground units a blast is weighed against, the most
/// recently seen first: a computation bound on a pairwise check, not a limit
/// on what the seat knows.
const BLAST_TARGETS: usize = 64;

impl Enemy {
    fn of(observation: &ObservationData, memory: &Memory) -> Self {
        let now = observation.tick;
        let mut enemy = Self {
            air: 0,
            ground: 0,
            defenses: 0,
            reach: None,
            clustered: false,
            targets: Vec::new(),
        };
        let mut reaches = [0_u64; 3];
        for unit in memory.units() {
            let value = unit.value(now);
            if unit.kind.stats().domain == Domain::Air {
                enemy.air += value;
            } else if unit.kind.stats().can_fight() {
                enemy.ground += value;
            }
            if let Some(reach) = reach(unit.kind) {
                reaches[reach as usize] += value;
            }
        }
        enemy.reach = [Reach::Short, Reach::Medium, Reach::Long]
            .into_iter()
            .filter(|reach| reaches[*reach as usize] > 0)
            .max_by_key(|reach| reaches[*reach as usize]);
        for building in &observation.enemy_buildings {
            if building.kind == BuildingKind::Airworks {
                enemy.air += 200;
            }
            if DEFENSES.contains(&building.kind) {
                enemy.defenses += crate::missions::building_value(building);
            }
        }
        enemy.clustered = clustered(memory.units());
        let mut ground: Vec<&SeenUnit> = memory
            .units()
            .iter()
            .filter(|unit| unit.kind.stats().domain == Domain::Ground)
            .collect();
        ground.sort_by_key(|unit| (Reverse(unit.seen), unit.id));
        ground.truncate(BLAST_TARGETS);
        enemy.targets = ground
            .iter()
            .map(|unit| (unit.tile, unit.kind.stats().max_hp))
            .collect();
        enemy
    }

    /// Per mille of one ground target's damage that a blast of `kind`
    /// deals to the densest known clump of enemy ground units: how many
    /// targets one shot is worth. A kind without ground splash, or with
    /// nothing known to hit, scores one target.
    fn blast(&self, kind: UnitKind) -> u64 {
        let Some((damage, radius)) = kind
            .stats()
            .weapons
            .iter()
            .filter(|weapon| weapon.targets.ground)
            .find_map(|weapon| Some((u64::from(weapon.damage.max(1)), weapon.splash?)))
        else {
            return 1_000;
        };
        let reach = radius * radius;
        let dealt = |centre: TilePos| -> u64 {
            self.targets
                .iter()
                .filter(|(tile, _)| {
                    let (dx, dy) = (tile.x - centre.x, tile.y - centre.y);
                    Fx::from_num(dx * dx + dy * dy) <= reach
                })
                .map(|(_, hp)| damage.min(u64::from(*hp)))
                .sum()
        };
        self.targets
            .iter()
            .map(|(tile, _)| dealt(*tile))
            .max()
            .map_or(1_000, |most| (most * 1_000 / damage).max(1_000))
    }
}

/// Whether `units` gather in clumps: four within three tiles of one another,
/// where a shell's splash hits several.
pub(crate) fn clustered<'a>(units: impl IntoIterator<Item = &'a SeenUnit>) -> bool {
    // The most recently seen units stand in for the rest: a computation
    // bound on a pairwise check, not a limit on what the seat knows.
    let mut recent: Vec<&SeenUnit> = units.into_iter().collect();
    recent.sort_by_key(|unit| (Reverse(unit.seen), unit.id));
    recent.truncate(32);
    recent.iter().any(|unit| {
        recent
            .iter()
            .filter(|other| other.tile.chebyshev(unit.tile) <= 3)
            .count()
            >= 4
    })
}

/// Deficits by role, in scrap, and the weight personality gives each role.
pub(crate) struct Needs {
    need: [i64; 4],
    weight: [u64; 4],
    /// Whether ground units can reach an enemy: a building or start, or
    /// invaders on the seat's own ground.
    ground: bool,
    /// Whether ground units reach an enemy only by lift.
    lifted: bool,
    enemy: Enemy,
    traits: PersonalityTraits,
    income: u32,
    /// Units the seat has of each kind, alive, carried or queued, in
    /// `UnitKind::ALL` order.
    kinds: [u32; UnitKind::ALL.len()],
}

/// Per-mille weight of a 0..=100 trait.
pub(crate) fn weight(trait_value: u8) -> u64 {
    750 + 5 * u64::from(trait_value)
}

/// Per mille of the army the siege role holds at a middling siege trait: a
/// stance limit on ranged units behind a line that screens them. Turtle and
/// Balanced seats attack with their army massed; an Aggressive seat attacks
/// early with small armies, which slow, fragile siege only weakens.
fn siege_share(stance: BotStance) -> i64 {
    match stance {
        BotStance::Turtle | BotStance::Balanced => 500,
        BotStance::Aggressive => 200,
    }
}

/// Per mille of the army each point of the siege trait away from 50 moves
/// the siege share: a personality limit.
const SIEGE_SHARE_PER_TRAIT: i64 = 3;

/// Where the seat's army can go.
#[derive(Clone, Copy)]
pub(crate) struct Outlet {
    /// Ground units can reach an enemy building or start: by ground, or by
    /// lift once an Airworks stands.
    pub(crate) ground: bool,
    /// Ground units reach an enemy only by lift.
    pub(crate) lifted: bool,
    /// Value of the known enemy ground units on the seat's own ground, which
    /// its ground units reach even where they reach no enemy building.
    pub(crate) invaders: u64,
    /// Air strikes are wanted: an Airworks stands, or ground reaches no enemy.
    pub(crate) air_strikes: bool,
    /// Air strike value wanted at the least: while ground reaches no enemy,
    /// what a strike needs against the easiest known target, or bombers need
    /// to clear the anti-air around the next lift's target, whichever is
    /// more.
    pub(crate) strike: u64,
}

/// Needs from what the seat has seen and owns and where its army can go.
/// Ground roles are wanted only while ground units can reach an enemy
/// building or start; until then line units are wanted only against
/// invaders on the seat's own ground. A seat that delivers its ground army
/// only by lift wants siege only against known defenses: Lancers and
/// Bombards carry less scrap per Skyhook seat than Sentinels.
pub(crate) fn needs(
    observation: &ObservationData,
    memory: &Memory,
    traits: PersonalityTraits,
    stance: BotStance,
    income: u32,
    outlet: Outlet,
) -> Needs {
    let enemy = Enemy::of(observation, memory);
    let mut own = [0_i64; 4];
    let mut kinds = [0_u32; UnitKind::ALL.len()];
    let owned = observation
        .my_units
        .iter()
        .map(|unit| unit.kind)
        .chain(observation.my_carried_units.iter().map(|unit| unit.kind))
        .chain(observation.my_queues.iter().flatten().copied());
    for kind in owned {
        if let Some(role) = role(kind) {
            own[role as usize] += i64::from(kind.stats().cost);
        }
        kinds[index(kind)] += 1;
    }
    let army: i64 = own.iter().sum();
    let air = i64::try_from(enemy.air).unwrap_or(i64::MAX);
    let ground = i64::try_from(enemy.ground).unwrap_or(i64::MAX);
    let defenses = i64::try_from(enemy.defenses).unwrap_or(i64::MAX);
    let mut need = [0_i64; 4];
    if outlet.ground {
        need[Role::Line as usize] = (3 * ground / 4).max(2 * army / 5) - own[Role::Line as usize];
        let share = if outlet.lifted {
            0
        } else {
            siege_share(stance) + SIEGE_SHARE_PER_TRAIT * (i64::from(traits.siege) - 50)
        };
        need[Role::Siege as usize] =
            defenses / 2 + army * share / 1_000 - own[Role::Siege as usize];
    } else {
        let invaders = i64::try_from(outlet.invaders).unwrap_or(i64::MAX);
        need[Role::Line as usize] = 3 * invaders / 4 - own[Role::Line as usize];
    }
    need[Role::AntiAir as usize] = 3 * air / 4 - own[Role::AntiAir as usize];
    if outlet.air_strikes {
        let strike = i64::try_from(outlet.strike).unwrap_or(i64::MAX);
        need[Role::AirStrike as usize] =
            (army * i64::from(traits.air) / 400).max(strike) - own[Role::AirStrike as usize];
    }
    Needs {
        need,
        ground: outlet.ground || outlet.invaders > 0,
        lifted: outlet.lifted,
        weight: [
            1_000,
            weight(traits.siege),
            weight(traits.fortification),
            weight(traits.air),
        ],
        enemy,
        traits,
        income,
        kinds,
    }
}

/// Per mille: `kind`'s damage per second per 100 scrap against ground,
/// 1000 at ten, within a band narrow enough that variety still gives every
/// kind its turn. A Lancer's rail tops it; a Bombard's single slow shell
/// sits at the floor until splash and reach lift it.
fn firepower(kind: UnitKind) -> u64 {
    let stats = kind.stats();
    let per_second: u64 = stats
        .weapons
        .iter()
        .filter(|weapon| weapon.targets.ground)
        .map(|weapon| {
            u64::from(weapon.damage) * u64::from(oxide_sim::TICKS_PER_SECOND) * 1_000
                / u64::from(weapon.cooldown_ticks.max(1))
        })
        .sum();
    let per_hundred = per_second * 100 / u64::from(stats.cost.max(1));
    (500 + per_hundred / 20).clamp(900, 1_100)
}

/// `kind`'s position in `UnitKind::ALL`.
fn index(kind: UnitKind) -> usize {
    UnitKind::ALL
        .iter()
        .position(|each| *each == kind)
        .expect("every kind is listed")
}

impl Needs {
    /// Roles with a deficit, most wanted first.
    pub(crate) fn wanted(&self) -> Vec<Role> {
        let mut wanted: Vec<Role> = ROLES
            .into_iter()
            .filter(|role| self.need[*role as usize] > 0)
            .collect();
        wanted.sort_by_key(|role| {
            Reverse(
                self.need[*role as usize]
                    * i64::try_from(self.weight[*role as usize])
                        .expect("role weights stay near a thousand"),
            )
        });
        wanted
    }

    /// Whether an idle producer with no wanted role still trains ground
    /// units: only while ground units can reach an enemy.
    pub(crate) fn fallback(&self) -> bool {
        self.ground
    }

    /// The ground role an idle producer with no wanted role trains for: of
    /// line and siege, the one it serves furthest below its target, line
    /// on a tie, and line alone for a seat that lifts its ground army.
    /// `None` when it serves neither.
    pub(crate) fn fallback_role(
        &self,
        observation: &ObservationData,
        producer: BuildingKind,
    ) -> Option<Role> {
        [Role::Line, Role::Siege]
            .into_iter()
            .filter(|role| *role == Role::Line || !self.lifted)
            .filter(|role| serves(observation, producer, *role))
            .max_by_key(|role| (self.need[*role as usize], Reverse(*role as usize)))
    }

    /// The best unit `producer` can train now for `role` within `budget`. A
    /// better unit it cannot afford yet gives way to the best one it can,
    /// since other producers spend the scrap meanwhile.
    pub(crate) fn unit(
        &self,
        observation: &ObservationData,
        producer: BuildingKind,
        role: Role,
        budget: u32,
    ) -> Option<UnitKind> {
        producible(observation, producer)
            .filter(|kind| self::role(*kind) == Some(role) && kind.stats().cost <= budget)
            .max_by_key(|kind| (self.suitability(*kind, role), Reverse(kind.stats().cost)))
    }

    /// For each wanted role, the unit any built producer could train that
    /// suits it best, when it costs more than `spendable` and the role has a
    /// cheaper unit production would buy instead, with how much the seat
    /// wants to save for it: its role's weight, scaled by how much of two of
    /// it the role lacks and, for strike aircraft at a seat that can only
    /// lift its army, by how much better it suits the role than the cheaper
    /// unit.
    pub(crate) fn premium(
        &self,
        observation: &ObservationData,
        spendable: u32,
    ) -> Vec<(UnitKind, u32)> {
        let producers: Vec<BuildingKind> = BuildingKind::ALL
            .into_iter()
            .filter(|kind| {
                observation
                    .my_buildings
                    .iter()
                    .any(|building| building.kind == *kind && building.built)
            })
            .collect();
        let mut premium: Vec<(UnitKind, u32)> = Vec::new();
        for role in self.wanted() {
            let in_role: Vec<UnitKind> = producers
                .iter()
                .flat_map(|producer| producible(observation, *producer))
                .filter(|kind| self::role(*kind) == Some(role))
                .collect();
            let Some(best) = in_role
                .iter()
                .copied()
                .max_by_key(|kind| (self.suitability(*kind, role), Reverse(kind.stats().cost)))
            else {
                continue;
            };
            let cost = best.stats().cost;
            let Some(instead) = in_role
                .iter()
                .filter(|kind| kind.stats().cost < cost)
                .map(|kind| self.suitability(*kind, role))
                .max()
            else {
                continue;
            };
            if cost <= spendable {
                continue;
            }
            // A seat that can only lift its army saves for a bomber the more,
            // the more targets its blast takes than the cheaper strike
            // aircraft production would buy instead.
            let better = if role == Role::AirStrike && self.lifted {
                (self.suitability(best, role) * 1_000 / instead.max(1)).min(PREMIUM_CAP)
            } else {
                1_000
            };
            let score = u32::try_from(u64::from(self.worth(role, best)) * better / 1_000)
                .unwrap_or(u32::MAX);
            match premium.iter_mut().find(|(kind, _)| *kind == best) {
                Some((_, kept)) => *kept = (*kept).max(score),
                None => premium.push((best, score)),
            }
        }
        premium
    }

    /// The building the seat lacks that the best unit its built producers
    /// make for `role` requires, when that unit would suit the role better
    /// than any it can train now, with what saving for that unit is worth.
    /// Nothing while such a building is already going up.
    fn better_tech(
        &self,
        observation: &ObservationData,
        role: Role,
    ) -> Option<(BuildingKind, u32)> {
        let rank = |kind: UnitKind| (self.suitability(kind, role), Reverse(kind.stats().cost));
        let built = |kind: BuildingKind| {
            observation
                .my_buildings
                .iter()
                .any(|own| own.kind == kind && own.built)
        };
        let owned =
            |kind: BuildingKind| observation.my_buildings.iter().any(|own| own.kind == kind);
        let producers: Vec<BuildingKind> = BuildingKind::ALL
            .into_iter()
            .filter(|kind| built(*kind))
            .collect();
        let now = producers
            .iter()
            .flat_map(|producer| producible(observation, *producer))
            .filter(|kind| self::role(*kind) == Some(role))
            .map(rank)
            .max()?;
        producers
            .iter()
            .flat_map(|producer| producer.base_stats().produces.iter().copied())
            .filter(|kind| self::role(*kind) == Some(role))
            .filter_map(|kind| {
                let missing: Vec<BuildingKind> = kind
                    .stats()
                    .requires
                    .iter()
                    .copied()
                    .filter(|required| !built(*required))
                    .collect();
                let first = *missing.first()?;
                (!missing.iter().any(|required| owned(*required)))
                    .then(|| (rank(kind), first, kind))
            })
            .filter(|(ranked, _, _)| *ranked > now)
            .max_by_key(|(ranked, _, _)| *ranked)
            .map(|(_, building, kind)| (building, self.worth(role, kind)))
    }

    /// The producer the seat lacks whose best unit for `role` suits it better
    /// than any unit it can train now, with what saving for that unit is
    /// worth.
    fn better_producer(
        &self,
        observation: &ObservationData,
        role: Role,
    ) -> Option<(BuildingKind, u32)> {
        let rank = |kind: UnitKind| (self.suitability(kind, role), Reverse(kind.stats().cost));
        let now = BuildingKind::ALL
            .into_iter()
            .filter(|building| {
                observation
                    .my_buildings
                    .iter()
                    .any(|own| own.kind == *building && own.built)
            })
            .flat_map(|building| producible(observation, building))
            .filter(|kind| self::role(*kind) == Some(role))
            .map(rank)
            .max()?;
        BuildingKind::ALL
            .into_iter()
            .filter(|building| building.base_stats().construction.is_some())
            .filter(|building| {
                !observation
                    .my_buildings
                    .iter()
                    .any(|own| own.kind == *building)
            })
            .flat_map(|building| {
                building
                    .base_stats()
                    .produces
                    .iter()
                    .copied()
                    .filter(move |kind| {
                        self::role(*kind) == Some(role) && kind.stats().requires.is_empty()
                    })
                    .map(move |kind| (rank(kind), building, kind))
            })
            .filter(|(ranked, _, _)| *ranked > now)
            .max_by_key(|(ranked, _, _)| *ranked)
            .map(|(_, building, kind)| (building, self.worth(role, kind)))
    }

    /// How much the seat wants `kind` for `role`: the role's weight, scaled
    /// by how much of two of it the role lacks.
    fn worth(&self, role: Role, kind: UnitKind) -> u32 {
        let lacking = u64::try_from(self.need[role as usize].max(0)).unwrap_or(0);
        let pair = 2 * u64::from(kind.stats().cost.max(1));
        let score = self.weight[role as usize] * lacking.min(pair) / pair;
        u32::try_from(score).unwrap_or(u32::MAX)
    }

    /// Counts a unit queued this decision against its role's deficit.
    pub(crate) fn queued(&mut self, kind: UnitKind) {
        if let Some(role) = role(kind) {
            self.need[role as usize] -= i64::from(kind.stats().cost);
        }
        self.kinds[index(kind)] += 1;
    }

    /// A product of coarse per-mille factors: reach against the enemy's
    /// usual reach, what the role is for at the price (a siege unit's
    /// firepower; any other's health, which keeps it fighting), covering
    /// both of the enemy's domains, splash against clustered enemies,
    /// affordability at the seat's income, variety within the role, and
    /// personality.
    fn suitability(&self, kind: UnitKind, role: Role) -> u64 {
        let stats = kind.stats();
        let reach = match (reach(kind), self.enemy.reach) {
            (Some(own), Some(enemy)) if own > enemy => 1_250,
            (Some(own), Some(enemy)) if own < enemy => 750,
            _ => 1_000,
        };
        let worth = match role {
            Role::Siege => firepower(kind),
            Role::Line | Role::AntiAir | Role::AirStrike => (750
                + 5 * (i64::from(stats.max_hp) * 100 / i64::from(stats.cost) - 50))
                .clamp(750, 1_250)
                .cast_unsigned(),
        };
        let hits = |air: bool| {
            stats.weapons.iter().any(|weapon| {
                if air {
                    weapon.targets.air
                } else {
                    weapon.targets.ground
                }
            })
        };
        let both = self.enemy.air > 0 && self.enemy.ground > 0 && hits(true) && hits(false);
        let coverage = if both { 1_250 } else { 1_000 };
        let splashes = stats.weapons.iter().any(|weapon| weapon.splash.is_some());
        // A seat that can only lift its army leans on bombers, whose worth
        // against clumped ground is the targets one blast takes.
        let splash = if role == Role::AirStrike && self.lifted {
            self.enemy.blast(kind)
        } else if splashes && self.enemy.clustered {
            1_250
        } else {
            1_000
        };
        let price = u64::from(stats.cost);
        let affordable = (500 * u64::from(self.income) / price).clamp(500, 1_000);
        let preference = match role {
            Role::Line => 1_000,
            Role::Siege => weight(self.traits.siege),
            Role::AntiAir => weight(self.traits.fortification),
            Role::AirStrike => weight(self.traits.air),
        };
        [
            reach,
            worth,
            coverage,
            splash,
            affordable,
            self.variety(kind, role),
        ]
        .into_iter()
        .fold(preference, |score, factor| score * factor / 1_000)
    }

    /// Per mille: more for a kind the seat has few of among its units in
    /// `role`, less for one that makes up most of them, so a role's other
    /// kinds get their turn.
    fn variety(&self, kind: UnitKind, role: Role) -> u64 {
        let in_role: u64 = UnitKind::ALL
            .iter()
            .filter(|each| self::role(**each) == Some(role))
            .map(|each| u64::from(self.kinds[index(*each)]))
            .sum();
        if in_role == 0 {
            return 1_000;
        }
        let share = u64::from(self.kinds[index(kind)]) * 1_000 / in_role;
        1_200 - 400 * share / 1_000
    }

    /// For each role the seat needs but cannot train at all, the cheapest
    /// building that would let it; for each it can, the producer it lacks
    /// whose unit would suit the role better than any it can train now. Each
    /// comes with what it adds to the building's investment score.
    pub(crate) fn pull(&self, observation: &ObservationData) -> Vec<(BuildingKind, u32)> {
        let mut pull = Vec::new();
        for role in ROLES {
            if self.need[role as usize] <= 0 {
                continue;
            }
            if trainable(observation, role) {
                pull.extend(self.better_producer(observation, role));
                if role == Role::AirStrike && self.lifted {
                    pull.extend(self.better_tech(observation, role));
                }
                continue;
            }
            let cheapest = BuildingKind::ALL
                .into_iter()
                .filter(|building| {
                    building.base_stats().produces.iter().any(|kind| {
                        self::role(*kind) == Some(role) && kind.stats().requires.is_empty()
                    })
                })
                .filter_map(|building| {
                    let cost = building.base_stats().construction.as_ref()?.cost;
                    Some((cost, building))
                })
                .min();
            if let Some((_, building)) = cheapest {
                pull.push((building, PULL));
            }
        }
        pull
    }
}

/// Whether `producer` can train a unit in `role` for the seat now.
pub(crate) fn serves(observation: &ObservationData, producer: BuildingKind, role: Role) -> bool {
    producible(observation, producer).any(|kind| self::role(kind) == Some(role))
}

/// Whether any built producer of the seat can train a unit in `role`.
fn trainable(observation: &ObservationData, role: Role) -> bool {
    observation
        .my_buildings
        .iter()
        .filter(|building| building.built)
        .any(|building| {
            producible(observation, building.kind).any(|kind| self::role(kind) == Some(role))
        })
}

/// Units `producer` can train for the seat now.
pub(crate) fn producible(
    observation: &ObservationData,
    producer: BuildingKind,
) -> impl Iterator<Item = UnitKind> + '_ {
    producer
        .base_stats()
        .produces
        .iter()
        .copied()
        .filter(move |kind| {
            kind.stats().requires.iter().all(|required| {
                observation
                    .my_buildings
                    .iter()
                    .any(|building| building.kind == *required && building.built)
            })
        })
}

#[cfg(test)]
mod tests;
