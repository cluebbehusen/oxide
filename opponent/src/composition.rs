//! What the army needs: deficits by role, from what the seat has seen of the
//! enemy and what it owns, and the unit that best fills a role at a producer.
//! Choices use coarse suitability, never a combat simulation.

use crate::memory::Memory;
use crate::profile::PersonalityTraits;
use chassis::fx::Fx;
use oxide_sim::observation::ObservationData;
use oxide_sim::stats::{Domain, Role as Kind};
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

/// Enemy buildings that shoot back.
const DEFENSES: [BuildingKind; 3] = [
    BuildingKind::Turret,
    BuildingKind::FlakTurret,
    BuildingKind::Bastion,
];

/// The role a unit fills. Harvesters, raiders, support, scouts and transports
/// fill none here.
pub(crate) fn role(kind: UnitKind) -> Option<Role> {
    match kind.role() {
        Kind::Sentinel | Kind::Warden | Kind::Breaker => Some(Role::Line),
        Kind::Lancer | Kind::Bombard | Kind::Avalanche => Some(Role::Siege),
        Kind::AntiAir | Kind::AirAir | Kind::Interceptor => Some(Role::AntiAir),
        Kind::AirGround | Kind::Bomber => Some(Role::AirStrike),
        _ => None,
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
}

impl Enemy {
    fn of(observation: &ObservationData, memory: &Memory) -> Self {
        let now = observation.tick;
        let mut enemy = Self {
            air: 0,
            ground: 0,
            defenses: 0,
            reach: None,
            clustered: false,
        };
        let mut reaches = [0_u64; 3];
        for unit in memory.units() {
            let value = unit.value(now);
            if unit.kind.stats().domain == Domain::Air {
                enemy.air += value;
            } else if !unit.kind.stats().weapons.is_empty() {
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
                enemy.defenses += u64::from(
                    building
                        .kind
                        .base_stats()
                        .construction
                        .as_ref()
                        .map_or(0, |construction| construction.cost),
                );
            }
        }
        let mut recent: Vec<_> = memory.units().iter().collect();
        recent.sort_by_key(|unit| (Reverse(unit.seen), unit.id));
        recent.truncate(32);
        enemy.clustered = recent.iter().any(|unit| {
            recent
                .iter()
                .filter(|other| other.tile.chebyshev(unit.tile) <= 3)
                .count()
                >= 4
        });
        enemy
    }
}

/// Deficits by role, in scrap, and the weight personality gives each role.
pub(crate) struct Needs {
    need: [i64; 4],
    weight: [u64; 4],
    enemy: Enemy,
    traits: PersonalityTraits,
    income: u32,
}

/// Per-mille weight of a 0..=100 trait.
fn weight(trait_value: u8) -> u64 {
    750 + 5 * u64::from(trait_value)
}

/// Needs from what the seat has seen and owns. Air strikes are needed only
/// once the seat has or is saving for an Airworks.
pub(crate) fn needs(
    observation: &ObservationData,
    memory: &Memory,
    traits: PersonalityTraits,
    income: u32,
    air_strikes: bool,
) -> Needs {
    let enemy = Enemy::of(observation, memory);
    let mut own = [0_i64; 4];
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
    }
    let army: i64 = own.iter().sum();
    let air = enemy.air as i64;
    let ground = enemy.ground as i64;
    let defenses = enemy.defenses as i64;
    let mut need = [0_i64; 4];
    need[Role::Line as usize] = (3 * ground / 4).max(2 * army / 5) - own[Role::Line as usize];
    need[Role::Siege as usize] =
        defenses / 2 + army * i64::from(traits.siege) / 400 - own[Role::Siege as usize];
    need[Role::AntiAir as usize] = 3 * air / 4 - own[Role::AntiAir as usize];
    if air_strikes {
        need[Role::AirStrike as usize] =
            army * i64::from(traits.air) / 400 - own[Role::AirStrike as usize];
    }
    Needs {
        need,
        weight: [
            1_000,
            weight(traits.siege),
            weight(traits.fortification),
            weight(traits.air),
        ],
        enemy,
        traits,
        income,
    }
}

impl Needs {
    /// The most wanted role among `roles`. With nothing wanted, unprotected
    /// scrap still becomes line units.
    fn pick(&self, roles: impl Iterator<Item = Role> + Clone) -> Option<Role> {
        roles
            .clone()
            .filter(|role| self.need[*role as usize] > 0)
            .max_by_key(|role| self.need[*role as usize] * self.weight[*role as usize] as i64)
            .or_else(|| roles.into_iter().find(|role| *role == Role::Line))
    }

    /// The best unit `producer` can train now for its most wanted role within
    /// `budget`. A better unit it cannot afford yet gives way to the best one
    /// it can, since other producers spend the scrap meanwhile.
    pub(crate) fn choose(
        &self,
        observation: &ObservationData,
        producer: BuildingKind,
        budget: u32,
    ) -> Option<UnitKind> {
        let options: Vec<UnitKind> = producible(observation, producer).collect();
        let role = self.pick(
            ROLES
                .into_iter()
                .filter(|role| options.iter().any(|kind| self::role(*kind) == Some(*role))),
        )?;
        options
            .into_iter()
            .filter(|kind| self::role(*kind) == Some(role) && kind.stats().cost <= budget)
            .max_by_key(|kind| (self.suitability(*kind, role), Reverse(kind.stats().cost)))
    }

    /// Counts a unit queued this decision against its role's deficit.
    pub(crate) fn queued(&mut self, kind: UnitKind) {
        if let Some(role) = role(kind) {
            self.need[role as usize] -= i64::from(kind.stats().cost);
        }
    }

    /// A product of coarse per-mille factors: reach against the enemy's
    /// usual reach, durability for the price, covering both of the enemy's
    /// domains, splash against clustered enemies, affordability at the seat's
    /// income, and personality.
    fn suitability(&self, kind: UnitKind, role: Role) -> u64 {
        let stats = kind.stats();
        let reach = match (reach(kind), self.enemy.reach) {
            (Some(own), Some(enemy)) if own > enemy => 1_250,
            (Some(own), Some(enemy)) if own < enemy => 750,
            _ => 1_000,
        };
        let durability = (750 + 5 * (i64::from(stats.max_hp) * 100 / i64::from(stats.cost) - 50))
            .clamp(750, 1_250) as u64;
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
        let splash = if splashes && self.enemy.clustered {
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
        [reach, durability, coverage, splash, affordable]
            .into_iter()
            .fold(preference, |score, factor| score * factor / 1_000)
    }

    /// For each role the seat needs but cannot train at all, the cheapest
    /// building that would let it, with what that adds to the building's
    /// investment score.
    pub(crate) fn pull(&self, observation: &ObservationData) -> Vec<(BuildingKind, u32)> {
        let mut pull = Vec::new();
        for role in ROLES {
            if self.need[role as usize] <= 0 || trainable(observation, role) {
                continue;
            }
            let cheapest = BuildingKind::ALL
                .into_iter()
                .filter(|building| {
                    building.base_stats().produces.iter().any(|kind| {
                        self::role(*kind) == Some(role)
                            && legal(observation, *kind)
                            && kind.stats().requires.is_empty()
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
fn producible(
    observation: &ObservationData,
    producer: BuildingKind,
) -> impl Iterator<Item = UnitKind> + '_ {
    producer
        .base_stats()
        .produces
        .iter()
        .copied()
        .filter(move |kind| {
            legal(observation, *kind)
                && kind.stats().requires.iter().all(|required| {
                    observation
                        .my_buildings
                        .iter()
                        .any(|building| building.kind == *required && building.built)
                })
        })
}

/// Whether the seat's faction fields `kind`.
fn legal(observation: &ObservationData, kind: UnitKind) -> bool {
    kind.faction()
        .is_none_or(|faction| faction == observation.faction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needs(reach: Option<Reach>, clustered: bool) -> Needs {
        Needs {
            need: [0; 4],
            weight: [1_000; 4],
            enemy: Enemy {
                air: 0,
                ground: 1_000,
                defenses: 0,
                reach,
                clustered,
            },
            traits: PersonalityTraits {
                air: 50,
                siege: 50,
                support: 50,
                fortification: 50,
                greed: 50,
                guile: 50,
            },
            income: 1_000,
        }
    }

    #[test]
    fn longer_reaching_enemies_count_against_short_reach() {
        let sentinel =
            |reach| needs(Some(reach), false).suitability(UnitKind::Sentinel, Role::Line);
        assert!(sentinel(Reach::Long) < sentinel(Reach::Medium));
        assert!(sentinel(Reach::Medium) < sentinel(Reach::Short));
    }

    #[test]
    fn clustered_enemies_favour_splash() {
        let prefers = |clustered| {
            let needs = needs(None, clustered);
            needs.suitability(UnitKind::Bombard, Role::Siege)
                > needs.suitability(UnitKind::Lancer, Role::Siege)
        };
        assert!(prefers(true));
        assert!(!prefers(false));
    }

    #[test]
    fn clustering_reads_the_most_recently_seen_enemies() {
        use oxide_sim::observation::UnitObs;
        use oxide_sim::{PlayerId, Scenario, UnitId};
        let state = Scenario::skirmish().build().unwrap();
        let mut observation = ObservationData::fog_honest(&state, PlayerId(0));
        let template = observation.my_units[0].clone();
        let unit = |id: u32, x: i32, y: i32| UnitObs {
            id: UnitId(id),
            kind: UnitKind::Sentinel,
            tile: chassis::grid::TilePos::new(x, y),
            ..template.clone()
        };
        observation
            .visible
            .iter_mut()
            .for_each(|visible| *visible = false);
        observation.enemy_units = (0..36)
            .map(|index| unit(100 + index, (index % 6) as i32 * 6, (index / 6) as i32 * 4))
            .collect();
        let mut memory = Memory::default();
        memory.observe(&observation);
        assert!(
            !Enemy::of(&observation, &memory).clustered,
            "premise: spread out"
        );
        observation.tick = 300;
        observation.enemy_units = (1..=4)
            .map(|index| unit(index, 20 + (index % 2) as i32, 20 + (index / 2) as i32))
            .collect();
        memory.observe(&observation);
        assert!(Enemy::of(&observation, &memory).clustered);
    }
}
