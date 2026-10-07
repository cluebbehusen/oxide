//! The defend mission: every threatened Foundry recruits free units that can
//! hit its threats and sends them at the threat nearest it, then lets them go
//! once the threat has been gone a while. An ally's Foundry under ground
//! attack gets the units the seat's own Foundries leave free. Shells from a
//! known enemy building send ground fighters beside it, shells from a gun out
//! of sight advance them on where it probably stands, and a gun in sight is
//! attacked.

use super::attack::{defense, defense_around};
use super::{
    DefendPhase, MISSION_CAP, Mission, Missions, Task, UNIT_CAP, hits, hunt, insert, mine, value,
    walking_gun,
};
use crate::decision::Ledger;
use crate::frame::{HomeFrame, centre_distance, doubled, footprint_centre, gap, ring};
use crate::map::MapModel;
use crate::memory::Memory;
use chassis::grid::TilePos;
use oxide_sim::observation::{BuildingObs, ObservationData, UnitObs};
use oxide_sim::stats::{Domain, WeaponStats};
use oxide_sim::{AttackTarget, BuildingKind, Command, UnitId, UnitKind};

/// Empty tiles between an enemy and an own building inside which the enemy
/// threatens it, unless its weapon reaches further.
const THREAT_GAP: i32 = 8;

/// Ticks a defense waits without a threat before letting its units go.
const QUIET_TICKS: u64 = 120;

/// Ticks a defense may stay engaged before evaluation calls it stuck.
const ENGAGE_TICKS: u64 = 3_600;

/// Tiles a threat may move before its defenders are sent after it again.
const RETARGET_TILES: i32 = 3;

/// Ticks a defense's `phase` should end within.
pub(super) fn timeout(phase: DefendPhase) -> u64 {
    match phase {
        DefendPhase::Engage { .. } => ENGAGE_TICKS,
        DefendPhase::Recover => QUIET_TICKS,
    }
}

impl Missions {
    /// Answers every threatened Foundry: recruits free units that can hit its
    /// threats until, in each domain it is attacked from, they outweigh the
    /// attackers by half again, and sends them at the grounded threat nearest
    /// the Foundry, else at a gun out of sight. A shelling building counts
    /// with the known defense around it, and only while the units the defense
    /// holds or could take would beat that defense by half again. A defense that is only
    /// recovering lends its units, as does an attack not yet fighting.
    /// Returns each of the seat's own Foundries whose defense falls short of
    /// its attackers, home-nearest first.
    ///
    /// Only threats standing on or beside the Foundry's ground count. One
    /// across water or a chasm is left to production: chasing it would stall
    /// every defender, and a defense short forever would never save for the
    /// tech that answers it. Nor does a defense against shelling, from a
    /// building or a gun out of sight, count as short: static defenses cannot
    /// reach the guns.
    pub(crate) fn defend(
        &mut self,
        observation: &ObservationData,
        map: &MapModel,
        frame: HomeFrame,
        memory: &Memory,
        ledger: &mut Ledger,
    ) -> Vec<Shortfall> {
        let now = observation.tick;
        let lendable = self.available(observation, true);
        let mut groups = threats(observation, map, frame);
        for (foundry, siege, _) in &mut groups {
            let component = map.component(foundry.anchor);
            // What the Foundry's defense holds and could take, not units out
            // on missions that will not lend them.
            let defenders = self
                .list
                .iter()
                .filter(|mission| {
                    matches!(mission.task, Task::Defend { asset, .. } if asset == foundry.id)
                })
                .flat_map(|mission| mission.units.iter().copied());
            // A recovering defense lends its units, so they may be in both.
            let mut pool: Vec<UnitId> = lendable.iter().copied().chain(defenders).collect();
            pool.sort_unstable();
            pool.dedup();
            let force: u64 = pool
                .into_iter()
                .filter(|id| !ledger.stuck(*id))
                .filter_map(|id| mine(observation, id))
                .filter(|unit| hits(unit, Domain::Ground))
                .filter(|unit| {
                    unit.kind.stats().domain == Domain::Air || map.component(unit.tile) == component
                })
                .map(value)
                .sum();
            // A battery among defenses the seat cannot beat now is left to
            // production: fighters sent beside it would only feed its guns.
            siege
                .batteries
                .retain(|(_, beside)| defense(observation, memory, *beside) * 3 / 2 <= force);
        }
        groups.retain(|(_, siege, _)| {
            !siege.units.is_empty() || !siege.batteries.is_empty() || siege.unseen.is_some()
        });
        for mission in &mut self.list {
            let Task::Defend { asset, phase } = &mut mission.task else {
                continue;
            };
            let threatened = groups.iter().any(|(foundry, _, _)| foundry.id == *asset);
            if let DefendPhase::Engage { focus } = *phase
                && !threatened
            {
                // A focus order chases its target; without a fresh Hunt the
                // members would follow a retreating enemy out of the base.
                if focus.is_some() {
                    ledger.order(hunt(mission.units.clone(), mission.goal));
                }
                *phase = DefendPhase::Recover;
                mission.since = now;
            }
        }
        self.list.retain(|mission| {
            !matches!(
                mission.task,
                Task::Defend {
                    phase: DefendPhase::Recover,
                    ..
                }
            ) || now < mission.since + QUIET_TICKS
        });

        let mut free = self.available(observation, true);
        let mut shortfalls = Vec::new();
        for (foundry, siege, own) in &groups {
            let centre = footprint_centre(foundry.kind, foundry.anchor);
            let component = map.component(foundry.anchor);
            let nearest = |tiles: &mut dyn Iterator<Item = TilePos>| {
                tiles.min_by_key(|tile| frame.rank(centre, doubled(*tile)))
            };
            let grounded = nearest(
                &mut siege
                    .units
                    .iter()
                    .filter(|threat| threat.body_domain() == Domain::Ground)
                    .map(|threat| threat.tile)
                    .chain(siege.batteries.iter().map(|(_, beside)| *beside)),
            );
            let Some(goal) = grounded.or(siege.unseen).or_else(|| {
                let flyer = nearest(&mut siege.units.iter().map(|threat| threat.tile))?;
                own.then(|| guard(observation, map, frame, component, flyer))?
            }) else {
                continue;
            };
            // Artillery is reached rather than hunted: a hunt engages whatever
            // comes first, such as a spotter overhead, while the guns shell the
            // defenders from beyond reach. A gun in sight on the Foundry's
            // ground is attacked; one out of sight is advanced on.
            let gun = siege
                .units
                .iter()
                .filter(|threat| walking_gun(threat) && map.component(threat.tile) == component)
                .min_by_key(|threat| (frame.rank(centre, doubled(threat.tile)), threat.id))
                .map(|threat| Pursuit::Gun(threat.id));
            let pursuit = gun.unwrap_or(if grounded.is_none() && siege.unseen.is_some() {
                Pursuit::Advance
            } else {
                Pursuit::Hunt
            });
            let gun = match pursuit {
                Pursuit::Gun(id) => Some(id),
                _ => None,
            };
            let pressing = [Domain::Ground, Domain::Air].map(|domain| {
                siege
                    .units
                    .iter()
                    .filter(|threat| threat.body_domain() == domain)
                    .map(|threat| value(threat))
                    .sum::<u64>()
                    * 3
                    / 2
            });
            let besides: Vec<TilePos> = siege.batteries.iter().map(|(_, beside)| *beside).collect();
            let guns = defense_around(observation, memory, &besides)
                + siege.unseen.map_or(0, |_| gun_value());
            let need = [pressing[0] + guns * 3 / 2, pressing[1]];
            let index = self.list.iter().position(
                |mission| matches!(mission.task, Task::Defend { asset, .. } if asset == foundry.id),
            );
            let members = index.map_or(&[][..], |index| &self.list[index].units);
            let mut have = [Domain::Ground, Domain::Air].map(|domain| {
                members
                    .iter()
                    .filter_map(|id| mine(observation, *id))
                    .filter(|unit| hits(unit, domain))
                    .map(value)
                    .sum::<u64>()
            });
            let room = UNIT_CAP - members.len();
            let mut candidates: Vec<&UnitObs> = free
                .iter()
                .filter_map(|id| mine(observation, *id))
                .filter(|unit| !members.contains(&unit.id) && !ledger.stuck(unit.id))
                .filter(|unit| {
                    [Domain::Ground, Domain::Air]
                        .into_iter()
                        .enumerate()
                        .any(|(index, domain)| need[index] > 0 && hits(unit, domain))
                })
                .filter(|unit| {
                    unit.kind.stats().domain == Domain::Air || map.component(unit.tile) == component
                })
                .collect();
            candidates.sort_by_key(|unit| (frame.rank(centre, doubled(unit.tile)), unit.id));
            let wanted = |have: &[u64; 2], unit: &UnitObs| {
                [Domain::Ground, Domain::Air]
                    .into_iter()
                    .enumerate()
                    .any(|(index, domain)| have[index] < need[index] && hits(unit, domain))
            };
            let mut recruits = Vec::new();
            for unit in candidates {
                if recruits.len() == room || (have[0] >= need[0] && have[1] >= need[1]) {
                    break;
                }
                if !wanted(&have, unit) {
                    continue;
                }
                for (index, domain) in [Domain::Ground, Domain::Air].into_iter().enumerate() {
                    if hits(unit, domain) {
                        have[index] += value(unit);
                    }
                }
                recruits.push(unit.id);
            }
            let gap = [0, 1].map(|index| pressing[index].saturating_sub(have[index]));
            if *own && gap != [0, 0] {
                shortfalls.push(Shortfall {
                    ground: component,
                    gap,
                });
            }

            let Some(index) = index else {
                if recruits.is_empty() || self.list.len() >= MISSION_CAP {
                    continue;
                }
                recruits.sort_unstable();
                let ordered = dispatch(observation, ledger, recruits, goal, pursuit);
                if !ordered.is_empty() {
                    take(&mut free, &ordered);
                    self.release(&ordered);
                    self.list.push(Mission {
                        id: self.next,
                        since: now,
                        units: ordered,
                        goal,
                        task: Task::Defend {
                            asset: foundry.id,
                            phase: DefendPhase::Engage { focus: gun },
                        },
                    });
                    self.next += 1;
                }
                continue;
            };
            let mission = &mut self.list[index];
            let Task::Defend { phase, .. } = &mut mission.task else {
                unreachable!("a defend mission's task");
            };
            let recovering = *phase == DefendPhase::Recover;
            let focus = match *phase {
                DefendPhase::Engage { focus } => focus,
                DefendPhase::Recover => None,
            };
            let lost = lost(observation, gun, focus);
            // Chasing a threat off the Foundry's ground, such as a flyer over a
            // chasm, would stall every defender each time it moved.
            let resend = recovering
                || lost
                || (gun.is_some() && focus != gun)
                || (mission.goal.chebyshev(goal) > RETARGET_TILES
                    && map.component(goal) == component);
            let mut sent = recruits.clone();
            if resend {
                sent.extend_from_slice(&mission.units);
            }
            if sent.is_empty() {
                continue;
            }
            sent.sort_unstable();
            let ordered = dispatch(observation, ledger, sent, goal, pursuit);
            if !ordered.is_empty() {
                // Only recruits that were given an order join.
                let recruits: Vec<UnitId> = recruits
                    .into_iter()
                    .filter(|id| ordered.contains(id))
                    .collect();
                for id in &recruits {
                    insert(&mut mission.units, *id);
                }
                take(&mut free, &recruits);
                if resend {
                    mission.goal = goal;
                }
                if recovering || lost || gun.is_some() {
                    *phase = DefendPhase::Engage { focus: gun };
                }
                if recovering {
                    mission.since = now;
                }
                self.release_from_others(index, &recruits);
            }
        }
        shortfalls
    }
}

/// How far one of the seat's Foundries' defense falls short of its attackers.
pub(crate) struct Shortfall {
    /// The Foundry's ground: ground units help only from there.
    pub(crate) ground: Option<u32>,
    /// The shortfall, in scrap, against ground and air attackers.
    pub(crate) gap: [u64; 2],
}

/// Removes `taken` from the sorted `free`.
fn take(free: &mut Vec<UnitId>, taken: &[UnitId]) {
    free.retain(|id| !taken.contains(id));
}

/// Whether a defense's `focus` no longer holds while it has no `gun` to go
/// after: its target fell, left sight, or is a gun this Foundry no longer
/// faces. Any other focus is focus fire's, kept while it lasts.
fn lost(observation: &ObservationData, gun: Option<UnitId>, focus: Option<UnitId>) -> bool {
    gun.is_none()
        && focus.is_some_and(|id| {
            observation
                .enemy_units
                .iter()
                .find(|enemy| enemy.id == id)
                .is_none_or(walking_gun)
        })
}

/// How defenders go after a Foundry's threats.
#[derive(Clone, Copy)]
enum Pursuit {
    /// March on the nearest threat, engaging everything on the way.
    Hunt,
    /// Move on a gun out of sight without stopping for anything on the way.
    Advance,
    /// Attack a gun in sight.
    Gun(UnitId),
}

/// Sends defenders after a Foundry's threats: those that can hit ground as
/// `pursuit` says and the rest on a hunt to `goal`. Returns the units given an
/// order, none when the first order was refused.
fn dispatch(
    observation: &ObservationData,
    ledger: &mut Ledger,
    units: Vec<UnitId>,
    goal: TilePos,
    pursuit: Pursuit,
) -> Vec<UnitId> {
    let (shooters, rest): (Vec<UnitId>, Vec<UnitId>) = match pursuit {
        Pursuit::Hunt => (Vec::new(), units),
        Pursuit::Advance | Pursuit::Gun(_) => units
            .into_iter()
            .partition(|id| mine(observation, *id).is_some_and(|unit| hits(unit, Domain::Ground))),
    };
    if shooters.is_empty() {
        return if ledger.order(hunt(rest.clone(), goal)) {
            rest
        } else {
            Vec::new()
        };
    }
    let command = match pursuit {
        Pursuit::Gun(gun) => Command::Attack {
            units: shooters.clone(),
            target: AttackTarget::Unit(gun),
            queue: false,
        },
        Pursuit::Hunt | Pursuit::Advance => Command::Advance {
            units: shooters.clone(),
            goal,
            queue: false,
        },
    };
    if !ledger.order(command) {
        return Vec::new();
    }
    let mut ordered = shooters;
    if !rest.is_empty() && ledger.order(hunt(rest.clone(), goal)) {
        ordered.extend(rest);
        ordered.sort_unstable();
    }
    ordered
}

/// What threatens a Foundry: enemies in sight and shelling enemy buildings,
/// or, with neither, a gun out of sight.
#[derive(Default)]
struct Siege<'a> {
    /// Enemies in sight.
    units: Vec<&'a UnitObs>,
    /// Known enemy buildings, in sight or remembered, whose shells land near
    /// the base, each with the tile beside it defenders go to.
    batteries: Vec<(&'a BuildingObs, TilePos)>,
    /// Where a gun shelling the base from out of sight probably stands.
    unseen: Option<TilePos>,
}

/// Visible enemies and shelling enemy buildings that threaten the seat's
/// buildings, grouped by the built Foundry each is nearest, home-nearest
/// Foundry first, then the seat's other Foundries shelled by a gun out of
/// sight, then enemies that could hit an ally's buildings, grouped by the
/// ally's Foundry, each marked whether the Foundry is the seat's own. An
/// enemy joins only if it stands on or beside that Foundry's ground.
fn threats<'a>(
    observation: &'a ObservationData,
    map: &MapModel,
    frame: HomeFrame,
) -> Vec<(&'a BuildingObs, Siege<'a>, bool)> {
    let mut groups: Vec<(&BuildingObs, Siege, bool)> = Vec::new();
    let mut claimed: Vec<UnitId> = Vec::new();
    // Remembered buildings share one id, so batteries are told apart by anchor.
    let mut claimed_batteries: Vec<TilePos> = Vec::new();
    for (buildings, own) in [
        (&observation.my_buildings, true),
        (&observation.ally_buildings, false),
    ] {
        for (foundry, siege) in besieged(
            observation,
            map,
            frame,
            buildings,
            &claimed,
            &claimed_batteries,
        ) {
            claimed.extend(siege.units.iter().map(|threat| threat.id));
            claimed_batteries.extend(siege.batteries.iter().map(|(battery, _)| battery.anchor));
            groups.push((foundry, siege, own));
        }
        if own {
            // A gun out of sight waits until the threats in sight are gone:
            // answered together, shelling that never stops keeps the defense
            // recruiting ground units and the army never leaves home.
            for (foundry, gun) in unseen(observation, map, frame) {
                if !groups.iter().any(|(other, _, _)| other.id == foundry.id) {
                    let siege = Siege {
                        unseen: Some(gun),
                        ..Siege::default()
                    };
                    groups.push((foundry, siege, true));
                }
            }
        }
    }
    groups
}

/// Visible enemies not `claimed` that could hit one of `buildings`, and known
/// enemy buildings whose anchors are not in `claimed_batteries` shelling near
/// them, grouped by the built Foundry among them each is nearest,
/// home-nearest first.
fn besieged<'a>(
    observation: &'a ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    buildings: &'a [BuildingObs],
    claimed: &[UnitId],
    claimed_batteries: &[TilePos],
) -> Vec<(&'a BuildingObs, Siege<'a>)> {
    let mut foundries: Vec<&BuildingObs> = buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry && building.built)
        .collect();
    foundries.sort_by_key(|foundry| {
        (
            frame.rank(frame.home, footprint_centre(foundry.kind, foundry.anchor)),
            foundry.id,
        )
    });
    let mut groups: Vec<(&BuildingObs, Siege)> = foundries
        .iter()
        .map(|foundry| (*foundry, Siege::default()))
        .collect();
    let shelled = shelled(observation, buildings);
    let nearest = |anchor: TilePos, size: (i32, i32), centre: (i64, i64)| {
        (0..foundries.len()).min_by_key(|index| {
            let foundry = foundries[*index];
            (
                gap(foundry.anchor, foundry.kind.base_stats().size, anchor, size),
                frame.rank(centre, footprint_centre(foundry.kind, foundry.anchor)),
            )
        })
    };
    for enemy in &observation.enemy_units {
        if claimed.contains(&enemy.id) {
            continue;
        }
        let Some(reach) = ground_reach(enemy) else {
            continue;
        };
        // A gun shelling the base from beyond the threat gap still counts.
        let near = buildings.iter().any(|building| {
            gap(
                building.anchor,
                building.kind.base_stats().size,
                enemy.tile,
                (1, 1),
            ) < reach
        }) || shelled.iter().any(|impact| {
            enemy
                .kind
                .stats()
                .weapons
                .iter()
                .any(|weapon| shells(weapon, doubled(enemy.tile), *impact))
        });
        if !near {
            continue;
        }
        let Some(index) = nearest(enemy.tile, (1, 1), doubled(enemy.tile)) else {
            continue;
        };
        let reachable = map
            .component(foundries[index].anchor)
            .is_some_and(|component| {
                map.component(enemy.tile) == Some(component) || map.touches(enemy.tile, component)
            });
        if reachable {
            groups[index].1.units.push(enemy);
        }
    }
    for building in &observation.enemy_buildings {
        if claimed_batteries.contains(&building.anchor)
            || !shelled.iter().any(|impact| battery(building, *impact))
        {
            continue;
        }
        let size = building.kind.base_stats().size;
        let centre = footprint_centre(building.kind, building.anchor);
        let Some(index) = nearest(building.anchor, size, centre) else {
            continue;
        };
        let foundry = foundries[index];
        let home = footprint_centre(foundry.kind, foundry.anchor);
        let component = map.component(foundry.anchor);
        // A battery off the Foundry's ground is left to production, as a
        // unit across a chasm is.
        let beside = ring(building.anchor, size)
            .filter(|tile| component.is_some() && map.component(*tile) == component)
            .min_by_key(|tile| frame.rank(home, doubled(*tile)));
        if let Some(beside) = beside {
            groups[index].1.batteries.push((building, beside));
        }
    }
    groups.retain(|(_, siege)| !siege.units.is_empty() || !siege.batteries.is_empty());
    groups
}

/// Impacts of hostile shells near any of `buildings`.
fn shelled(observation: &ObservationData, buildings: &[BuildingObs]) -> Vec<TilePos> {
    observation
        .incoming_shells
        .iter()
        .copied()
        .filter(|impact| {
            buildings.iter().any(|building| {
                gap(
                    building.anchor,
                    building.kind.base_stats().size,
                    *impact,
                    (1, 1),
                ) < THREAT_GAP
            })
        })
        .collect()
}

/// Whether the known enemy `building` could have sent a shell landing on
/// `impact`. A site in sight cannot fire; a remembered one may have been
/// finished since it was seen.
fn battery(building: &BuildingObs, impact: TilePos) -> bool {
    let centre = footprint_centre(building.kind, building.anchor);
    (building.built || !building.seen)
        && building
            .kind
            .base_stats()
            .weapons
            .iter()
            .any(|weapon| shells(weapon, centre, impact))
}

/// Whether `weapon`, fired from the doubled position `from`, sends shells
/// that could land on `impact`.
fn shells(weapon: &WeaponStats, from: (i64, i64), impact: TilePos) -> bool {
    let reach = 2 * i64::from(weapon.range.ceil().to_num::<i32>());
    let to = doubled(impact);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    weapon.targets.ground
        && (weapon.indirect || weapon.projectile)
        && dx * dx + dy * dy <= reach * reach
}

/// The seat's built Foundries shelled by a gun out of sight, home-nearest
/// first, each with the tile the gun probably stands at: a hostile shell
/// lands near the seat's buildings, no visible enemy or known enemy building
/// could have fired it, and the built Foundry nearest the impact answers for
/// it. Of several such impacts, the one nearest that Foundry counts.
fn unseen<'a>(
    observation: &'a ObservationData,
    map: &MapModel,
    frame: HomeFrame,
) -> Vec<(&'a BuildingObs, TilePos)> {
    let fired = |impact: TilePos| {
        let units = observation.enemy_units.iter().any(|enemy| {
            enemy
                .kind
                .stats()
                .weapons
                .iter()
                .any(|weapon| shells(weapon, doubled(enemy.tile), impact))
        });
        units
            || observation
                .enemy_buildings
                .iter()
                .any(|building| battery(building, impact))
    };
    let impacts: Vec<TilePos> = shelled(observation, &observation.my_buildings)
        .into_iter()
        .filter(|impact| !fired(*impact))
        .collect();
    if impacts.is_empty() {
        return Vec::new();
    }
    let mut foundries: Vec<&BuildingObs> = observation
        .my_buildings
        .iter()
        .filter(|building| building.kind == BuildingKind::Foundry && building.built)
        .collect();
    foundries.sort_by_key(|foundry| {
        (
            frame.rank(frame.home, footprint_centre(foundry.kind, foundry.anchor)),
            foundry.id,
        )
    });
    let nearest = |impact: TilePos| {
        foundries.iter().copied().min_by_key(|foundry| {
            (
                gap(
                    foundry.anchor,
                    foundry.kind.base_stats().size,
                    impact,
                    (1, 1),
                ),
                frame.rank(
                    doubled(impact),
                    footprint_centre(foundry.kind, foundry.anchor),
                ),
            )
        })
    };
    foundries
        .iter()
        .filter_map(|foundry| {
            let centre = footprint_centre(foundry.kind, foundry.anchor);
            let impact = impacts
                .iter()
                .copied()
                .filter(|impact| nearest(*impact).is_some_and(|other| other.id == foundry.id))
                .min_by_key(|impact| frame.rank(centre, doubled(*impact)))?;
            let gun = gun(
                observation,
                map,
                frame,
                impact,
                map.component(foundry.anchor),
            )?;
            Some((*foundry, gun))
        })
        .collect()
}

/// Where a gun that shelled `impact` from out of sight probably stands: the
/// impact moved toward the nearest hostile start by the longest artillery
/// reach, or less where that leaves `component`.
fn gun(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    impact: TilePos,
    component: Option<u32>,
) -> Option<TilePos> {
    component?;
    let from = doubled(impact);
    let to = map
        .hostiles(observation.me)
        .filter_map(|owner| map.start(owner))
        .map(|start| footprint_centre(BuildingKind::Foundry, start))
        .min_by_key(|centre| (centre_distance(from, *centre), frame.rank(from, *centre)))?;
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).isqrt();
    let steps = (2 * artillery().map_or(0, |(reach, _)| reach)).min(length);
    // A doubled coordinate on a tile edge rounds toward the impact, so
    // mirrored impacts give mirrored tiles.
    let tile = |at: i64, toward: i64| -> i32 {
        let whole = if at % 2 != 0 {
            (at - 1) / 2
        } else if toward > 0 {
            at / 2 - 1
        } else {
            at / 2
        };
        i32::try_from(whole).unwrap_or(0)
    };
    (0..=steps).rev().step_by(2).find_map(|step| {
        let (x, y) = if length == 0 {
            from
        } else {
            (from.0 + dx * step / length, from.1 + dy * step / length)
        };
        let goal = TilePos::new(tile(x, dx), tile(y, dy));
        (map.component(goal) == component).then_some(goal)
    })
}

/// The longest reach in tiles among ground units that shell ground from out
/// of sight, and the lowest price among them.
fn artillery() -> Option<(i64, u64)> {
    let guns = UnitKind::ALL.iter().filter(|kind| {
        kind.stats().domain == Domain::Ground
            && kind
                .stats()
                .weapons
                .iter()
                .any(|weapon| weapon.indirect && weapon.targets.ground)
    });
    let reach = guns
        .clone()
        .flat_map(|kind| kind.stats().weapons.iter())
        .filter(|weapon| weapon.indirect && weapon.targets.ground)
        .map(|weapon| i64::from(weapon.range.ceil().to_num::<i32>()))
        .max()?;
    let price = guns.map(|kind| u64::from(kind.stats().cost)).min()?;
    Some((reach, price))
}

/// The value a defense against a gun out of sight wants to outweigh: one of
/// the cheapest guns.
fn gun_value() -> u64 {
    artillery().map_or(0, |(_, price)| price)
}

/// Where ground defenders wait out an air raid: beside the seat's building
/// on the Foundry's ground nearest `flyer`, on the tile nearest it. Chasing a
/// flyer's shadow only sends them to tiles they cannot stand on.
fn guard(
    observation: &ObservationData,
    map: &MapModel,
    frame: HomeFrame,
    component: Option<u32>,
    flyer: TilePos,
) -> Option<TilePos> {
    component?;
    let building = observation
        .my_buildings
        .iter()
        .filter(|building| map.component(building.anchor) == component)
        .min_by_key(|building| {
            (
                gap(
                    building.anchor,
                    building.kind.base_stats().size,
                    flyer,
                    (1, 1),
                ),
                frame.rank(
                    doubled(flyer),
                    footprint_centre(building.kind, building.anchor),
                ),
            )
        })?;
    ring(building.anchor, building.kind.base_stats().size)
        .filter(|tile| map.component(*tile) == component)
        .min_by_key(|tile| frame.rank(doubled(flyer), doubled(*tile)))
}

/// Empty tiles inside which `enemy` threatens a building: the threat gap, or
/// its longest reach against ground if longer. `None` when it cannot hit
/// ground at all.
fn ground_reach(enemy: &UnitObs) -> Option<i32> {
    enemy
        .kind
        .stats()
        .weapons
        .iter()
        .filter(|weapon| weapon.targets.ground)
        .map(|weapon| weapon.range.ceil().to_num::<i32>().max(THREAT_GAP))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_sim::scenario::{PlayerSpec, ScenarioMode, UnitSpec};
    use oxide_sim::{Faction, PlayerId, Scenario};

    /// West's view of a 20 by 9 field, starts at (2, 1) and (17, 1), with
    /// `units` as owner, kind and tile, and their ids in that order.
    fn viewed(units: &[(u8, UnitKind, i32, i32)]) -> (ObservationData, Vec<UnitId>) {
        let map: Vec<String> = (0..9)
            .map(|row| {
                let mut tiles: Vec<char> = ".".repeat(20).chars().collect();
                if row == 1 {
                    tiles[2] = '1';
                    tiles[17] = '2';
                }
                tiles.into_iter().collect()
            })
            .collect();
        let scenario = Scenario {
            mode: ScenarioMode::Match,
            name: "defense".into(),
            seed: 1,
            map,
            players: [Faction::Ferrous, Faction::Cupric]
                .into_iter()
                .map(|faction| PlayerSpec {
                    name: format!("{faction:?}"),
                    faction,
                    team: None,
                    scrap: 0,
                    bot: false,
                    bot_config: None,
                })
                .collect(),
            units: units
                .iter()
                .map(|&(player, kind, x, y)| UnitSpec { player, kind, x, y })
                .collect(),
            buildings: Vec::new(),
            meta: None,
        };
        let state = scenario.build().unwrap();
        let ids = units
            .iter()
            .map(|&(_, _, x, y)| {
                state
                    .units()
                    .iter()
                    .find(|unit| unit.tile() == TilePos::new(x, y))
                    .unwrap()
                    .id
            })
            .collect();
        (ObservationData::fog_honest(&state, PlayerId(0)), ids)
    }

    #[test]
    fn a_gun_the_foundry_no_longer_faces_is_lost_but_focus_fire_is_kept() {
        let (observation, ids) = viewed(&[
            (0, UnitKind::Sentinel, 3, 4),
            (1, UnitKind::Bombard, 6, 4),
            (1, UnitKind::Sentinel, 6, 6),
        ]);
        let (gun, raider) = (ids[1], ids[2]);
        assert!(observation.enemy_units.len() == 2, "premise: both in sight");
        assert!(lost(&observation, None, Some(gun)));
        assert!(!lost(&observation, None, Some(raider)));
        assert!(lost(&observation, None, Some(UnitId(999))));
        assert!(!lost(&observation, Some(gun), Some(gun)));
    }

    #[test]
    fn defenders_refused_an_order_are_not_counted_as_sent() {
        let (observation, ids) = viewed(&[
            (0, UnitKind::Sentinel, 3, 4),
            (0, UnitKind::Flakhound, 3, 5),
            (1, UnitKind::Bombard, 6, 4),
        ]);
        let (sentinel, flakhound, gun) = (ids[0], ids[1], ids[2]);
        let goal = TilePos::new(6, 4);
        let sent = |allowance| {
            let mut ledger = Ledger::new(PlayerId(0), 0, allowance);
            dispatch(
                &observation,
                &mut ledger,
                vec![sentinel, flakhound],
                goal,
                Pursuit::Gun(gun),
            )
        };
        let mut both = vec![sentinel, flakhound];
        both.sort_unstable();
        assert_eq!(sent(2), both);
        assert_eq!(sent(1), [sentinel], "the anti-air hunt was refused");
        assert!(sent(0).is_empty());
    }
}
