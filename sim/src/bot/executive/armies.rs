//! Army lifecycle, marching, contact, and strength assessment.

use super::*;
use core::cmp::Reverse;

const PULLBACK_NUM: u32 = 35;
const PULLBACK_DEN: u32 = 100;

/// Withdraw only from catastrophe: below half the local enemy strength.
/// Nothing in this world outruns its pursuers, so a merely-losing fight
/// finished on the spot costs less than a rout — disengaging under fire
/// is free damage handed to the enemy.
const WITHDRAW_MARGIN_NUM: u32 = 1;
const WITHDRAW_MARGIN_DEN: u32 = 2;
/// A marching or withdrawing army that has not bettered its best
/// distance to its goal for this long is wedged — usually ordered
/// across terrain with no route — and re-stages where it stands.
/// Staging reopens the trained-legal verbs (Scout for staged members,
/// Push, reinforcement), so the operations head never goes dark
/// behind an unroutable order. Matches the recovery patience scale.
pub(super) const ARMY_PROGRESS_PATIENCE_TICKS: u64 = 1_200;

/// Radius (tiles) around the army centroid scored as "the fight".
const ENGAGE_RADIUS: i32 = 8;
/// A pushing army is engaged once enemies are inside this radius.
const CONTACT_RADIUS: i32 = 6;
const OBJECTIVE_RADIUS: i32 = 2;

/// Integer macro positions are chosen in the owner's stable home-facing frame.
/// Flooring a mean directly in world space favors the southeast seat whenever
/// the exact mean lies between tiles: a half-turn maps `floor(x)` to
/// `ceil(mirror(x))`, not another floor. Transforming into the owner's frame
/// before division gives both seats the same deterministic tie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CentroidFrame {
    flip_x: bool,
    flip_y: bool,
    width: i32,
    height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MaintenanceMode {
    player_facing: bool,
    centroid_frame: Option<CentroidFrame>,
    coordinated_focus: bool,
    coordinated_defense_focus: bool,
}

impl CentroidFrame {
    fn for_rear(obs: &Observation, rear: TilePos) -> Self {
        Self {
            flip_x: 2 * rear.x >= obs.map_width,
            flip_y: 2 * rear.y >= obs.map_height,
            width: obs.map_width,
            height: obs.map_height,
        }
    }

    fn tile(self, tile: TilePos) -> TilePos {
        TilePos::new(
            if self.flip_x {
                self.width - 1 - tile.x
            } else {
                tile.x
            },
            if self.flip_y {
                self.height - 1 - tile.y
            } else {
                tile.y
            },
        )
    }
}

/// Read-only lookup over the observation's canonical own-unit roster.
/// Army membership is also kept in id order, so one binary search per member
/// replaces the former full-roster scan and linear membership probe.
struct UnitRoster<'a> {
    units: &'a [UnitObs],
    #[cfg(test)]
    lookups: std::cell::Cell<usize>,
    #[cfg(test)]
    comparisons: std::cell::Cell<usize>,
}

trait UnitLookup<'a> {
    fn get(&self, id: UnitId) -> Option<&'a UnitObs>;

    fn members(&self, ids: &[UnitId]) -> Vec<&'a UnitObs> {
        ids.iter().filter_map(|id| self.get(*id)).collect()
    }
}

impl<'a> UnitRoster<'a> {
    fn new(units: &'a [UnitObs]) -> Self {
        Self {
            units,
            #[cfg(test)]
            lookups: std::cell::Cell::new(0),
            #[cfg(test)]
            comparisons: std::cell::Cell::new(0),
        }
    }

    #[cfg(test)]
    fn work(&self) -> (usize, usize) {
        (self.lookups.get(), self.comparisons.get())
    }
}

impl<'a> UnitLookup<'a> for UnitRoster<'a> {
    fn get(&self, id: UnitId) -> Option<&'a UnitObs> {
        #[cfg(test)]
        self.lookups.set(self.lookups.get() + 1);
        self.units
            .binary_search_by(|unit| {
                #[cfg(test)]
                self.comparisons.set(self.comparisons.get() + 1);
                unit.id.cmp(&id)
            })
            .ok()
            .map(|index| &self.units[index])
    }
}

#[cfg(test)]
struct LinearUnitRoster<'a>(&'a [UnitObs]);

#[cfg(test)]
impl<'a> UnitLookup<'a> for LinearUnitRoster<'a> {
    fn get(&self, id: UnitId) -> Option<&'a UnitObs> {
        self.0.iter().find(|unit| unit.id == id)
    }
}

impl Executive {
    /// Prune the dead, rotate the wounded to the rear, advance army
    /// states, and withdraw from fights that have turned. `rear` is
    /// behind the lines, not the army's rally (which may be the fight).
    pub fn maintain(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        rear: TilePos,
    ) -> Vec<PlayerCommand> {
        self.maintain_inner(
            me,
            obs,
            rear,
            MaintenanceMode {
                player_facing: false,
                centroid_frame: None,
                coordinated_focus: true,
                coordinated_defense_focus: false,
            },
        )
    }

    /// Player-facing maintenance releases machines after a real repair, or
    /// once the retreat has consumed a full progress-patience window and the
    /// machine is idle. The latter prevents an unfunded rear line from
    /// consuming the whole fighting roster forever. The Overseer keeps its
    /// historical permanent rear-line behavior through [`Self::maintain`].
    pub fn maintain_player_facing(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        rear: TilePos,
    ) -> Vec<PlayerCommand> {
        self.maintain_player_facing_with_tactics(me, obs, rear, true, false)
    }

    pub(in crate::bot) fn maintain_player_facing_with_tactics(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        rear: TilePos,
        coordinated_focus: bool,
        coordinated_defense_focus: bool,
    ) -> Vec<PlayerCommand> {
        let frame = self
            .player_frame
            .get_or_insert_with(|| PlayerFacingTactics {
                frame: CentroidFrame::for_rear(obs, rear),
                defense_focus: None,
            })
            .frame;
        self.maintain_inner(
            me,
            obs,
            rear,
            MaintenanceMode {
                player_facing: true,
                centroid_frame: Some(frame),
                coordinated_focus,
                coordinated_defense_focus,
            },
        )
    }

    fn maintain_inner(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        rear: TilePos,
        mode: MaintenanceMode,
    ) -> Vec<PlayerCommand> {
        let roster = UnitRoster::new(&obs.my_units);
        self.maintain_with_roster(me, obs, rear, mode, &roster)
    }

    fn maintain_with_roster<'a>(
        &mut self,
        me: PlayerId,
        obs: &'a Observation,
        rear: TilePos,
        mode: MaintenanceMode,
        roster: &impl UnitLookup<'a>,
    ) -> Vec<PlayerCommand> {
        let MaintenanceMode {
            player_facing,
            centroid_frame,
            coordinated_focus,
            coordinated_defense_focus,
        } = mode;
        let mut out = Vec::new();
        if player_facing {
            self.maintain_defense_focus(me, obs, coordinated_defense_focus, &mut out);
        }
        if player_facing {
            self.exhausted_rear.retain(|id| {
                roster.get(*id).is_some_and(|unit| {
                    unit.hp.saturating_mul(4) < unit.kind.stats().max_hp.saturating_mul(3)
                })
            });
        }
        self.rear.retain(|rear_unit| {
            roster.get(rear_unit.id).is_some_and(|unit| {
                !player_facing
                    || unit.hp.saturating_mul(4) < unit.kind.stats().max_hp.saturating_mul(3)
            })
        });
        if player_facing {
            let releasable: Vec<UnitId> = self
                .rear
                .iter()
                .filter_map(|rear_unit| {
                    roster
                        .get(rear_unit.id)
                        .filter(|unit| {
                            unit.idle
                                && obs.tick.saturating_sub(rear_unit.since)
                                    >= ARMY_PROGRESS_PATIENCE_TICKS
                        })
                        .map(|unit| unit.id)
                })
                .collect();
            self.rear
                .retain(|rear_unit| releasable.binary_search(&rear_unit.id).is_err());
            self.exhausted_rear.extend(releasable);
            self.exhausted_rear.sort_unstable();
            self.exhausted_rear.dedup();
        }
        for army in &mut self.armies {
            army.members.retain(|id| roster.get(*id).is_some());
            if army.members.is_empty() {
                continue; // swept below
            }
            let mut members = roster.members(&army.members);
            let in_contact = enemies_near(obs, &members, CONTACT_RADIUS);

            // Rotate the badly wounded out, but only between fights.
            // Mid-engagement a wounded machine still deals full damage,
            // and at equal speeds it cannot escape a pursuer anyway;
            // pulling it then just thins the line.
            if !in_contact {
                let mut pulled: Vec<UnitId> = Vec::new();
                army.members.retain(|id| {
                    let Some(u) = roster.get(*id) else {
                        return false;
                    };
                    let max = u.kind.stats().max_hp;
                    if u.hp * PULLBACK_DEN < max * PULLBACK_NUM
                        && (!player_facing || self.exhausted_rear.binary_search(id).is_err())
                    {
                        pulled.push(*id);
                        false
                    } else {
                        true
                    }
                });
                if !pulled.is_empty() {
                    out.push(PlayerCommand {
                        player: me,
                        command: Command::Move {
                            units: pulled.clone(),
                            goal: rear,
                            queue: false,
                        },
                    });
                    for id in pulled {
                        if self.rear.binary_search_by_key(&id, |unit| unit.id).is_err() {
                            self.rear.push(RearUnit {
                                id,
                                since: obs.tick,
                            });
                        }
                    }
                    self.rear.sort_unstable_by_key(|unit| unit.id);
                }
                members = roster.members(&army.members);
            }
            if army.members.is_empty() {
                continue; // swept below
            }

            let centroid = centroid(&members, centroid_frame);
            match army.state {
                ArmyState::Staging => {
                    if player_facing
                        && army
                            .target
                            .is_some_and(|target| objective_cleared(obs, target))
                    {
                        army.members.clear();
                        continue;
                    }
                    // A staged army can be attacked where it stands — the
                    // fight evaluation must not wait for a push order.
                    if in_contact {
                        army.state = ArmyState::Engaging;
                    }
                }
                ArmyState::Pushing => {
                    let vanguard = vanguard_centroid_for(&members, centroid_frame);
                    // Judged on the escorts, the units the march order
                    // actually names: artillery takes a separate routable
                    // side-move to staging, and counting it kept a refused
                    // march from ever reading as idle.
                    let all_idle = members
                        .iter()
                        .filter(|unit| !is_artillery(unit))
                        .all(|unit| unit.idle);
                    // A march order the sim refused leaves every member
                    // idle exactly where it stood. Checked only on a LATER
                    // think than the order, since this think's commands
                    // have not executed yet.
                    let bounced = all_idle
                        && army.issued.is_some_and(|(at, from)| {
                            obs.tick > at && vanguard.chebyshev(from) <= 1
                        });
                    if in_contact {
                        army.state = ArmyState::Engaging;
                        army.progress = None;
                        army.issued = None;
                        army.bounces = 0;
                    } else if let Some(target) = army.target
                        && tiles_within(vanguard, target, 2)
                    {
                        if player_facing && objective_cleared(obs, target) {
                            army.members.clear();
                        } else {
                            // A live objective still needs a coherent body to
                            // hold the ground while its attack-move finishes it.
                            army.state = ArmyState::Staging;
                            army.staging = target;
                            if !player_facing {
                                army.target = None;
                            }
                            army.progress = None;
                            army.issued = None;
                            army.bounces = 0;
                        }
                    } else if bounced {
                        // Two refused orders in a row are route testimony
                        // on the first think a wedge clock would only begin
                        // counting — an order refused at issue never
                        // marches, so it never stalls. Two immediate
                        // bounces are enough to stop repeating it.
                        army.issued = None;
                        army.bounces = army.bounces.saturating_add(1);
                        if army.bounces >= 2 && army.target.is_some() {
                            if player_facing {
                                army.members.clear();
                            } else {
                                army.state = ArmyState::Staging;
                                army.staging = centroid;
                                army.target = None;
                                army.progress = None;
                                army.bounces = 0;
                            }
                        }
                    } else if let Some(target) = army.target
                        && wedged(&mut army.progress, vanguard.chebyshev(target), obs.tick)
                    {
                        if player_facing {
                            army.members.clear();
                        } else {
                            // The march has not gained a tile in the whole
                            // patience window — usually an order across
                            // terrain with no route. Rally where it stands
                            // so the seat's verbs come back.
                            army.state = ArmyState::Staging;
                            army.staging = centroid;
                            army.target = None;
                            army.progress = None;
                            army.issued = None;
                            army.bounces = 0;
                        }
                    }
                }
                ArmyState::Engaging => {
                    let (mine, theirs) = local_strength(obs, &members);
                    if theirs == 0 && (!player_facing || !in_contact) {
                        if player_facing
                            && army
                                .target
                                .is_some_and(|target| objective_cleared(obs, target))
                        {
                            army.members.clear();
                        } else {
                            // Fight's over here; march on if a target remains.
                            army.state = match army.target {
                                Some(_) => ArmyState::Pushing,
                                None => ArmyState::Staging,
                            };
                            army.focus = None;
                            army.progress = None;
                            if let Some(target) = army.target {
                                march_with_roster(me, obs, army, target, &mut out, roster);
                            }
                        }
                    } else if mine * u64::from(WITHDRAW_MARGIN_DEN)
                        < theirs * u64::from(WITHDRAW_MARGIN_NUM)
                        && (!player_facing || !tiles_within(centroid, army.staging, 2))
                    {
                        // Losing decisively: leave together, fighting.
                        // Nothing here outruns its pursuers, so an
                        // oblivious Move retreat is shot in the back for
                        // free the whole way home — the attack-move falls
                        // back along the same line but answers fire.
                        army.state = ArmyState::Withdrawing;
                        army.target = if player_facing {
                            withdrawal_threat(obs, &members)
                        } else {
                            None
                        };
                        army.focus = None;
                        army.progress = None;
                        out.push(PlayerCommand {
                            player: me,
                            command: Command::AttackMove {
                                units: army.members.clone(),
                                goal: army.staging,
                                queue: false,
                            },
                        });
                    } else {
                        // Player-facing coordination concentrates compatible
                        // front-line fire on a threat while specialists keep
                        // their existing orders. The profile-free path retains
                        // its historical whole-army weakest-gun choice.
                        // Candidates stay inside contact radius so the sim's
                        // see-the-victim rule holds even for an omniscient
                        // policy.
                        let near = |t: TilePos| {
                            members
                                .iter()
                                .map(|member| member.tile.chebyshev(t))
                                .min()
                                .unwrap_or(i32::MAX)
                        };
                        let legal_focus = |unit: &UnitObs| {
                            if !player_facing {
                                return near(unit.tile) <= CONTACT_RADIUS
                                    && unit.kind.stats().can_fight();
                            }
                            let focus_members: Vec<_> = members
                                .iter()
                                .copied()
                                .filter(|member| {
                                    !is_artillery(member)
                                        && can_attack_domain(member, unit.kind.stats().domain)
                                })
                                .collect();
                            obs.visible(unit.tile)
                                && near(unit.tile) <= CONTACT_RADIUS
                                // A ground body holds against aircraft and
                                // lets ordinary acquisition fire in range;
                                // pursuing a flyer's moving tile can ask
                                // every member to path onto water or roofs.
                                && unit.kind.stats().domain == crate::stats::Domain::Ground
                                && members.iter().any(|member| {
                                    can_attack_domain(unit, member.kind.stats().domain)
                                })
                                && !focus_members.is_empty()
                                // Focus fire is a shooting decision, not a
                                // replacement march. If even one compatible
                                // front-liner would have to chase, retain the
                                // body's attack-move and let ordinary
                                // acquisition handle the contact.
                                && focus_members
                                    .iter()
                                    .all(|member| can_focus_without_chasing(member, unit))
                        };
                        if coordinated_focus {
                            let choose_focus = || {
                                obs.enemy_units
                                    .iter()
                                    .filter(|unit| legal_focus(unit))
                                    .map(|u| (u.hp, near(u.tile), u.id))
                                    .min()
                                    .map(|(.., id)| id)
                            };
                            if !player_facing {
                                if let Some(target) = choose_focus()
                                    && army.focus != Some(target)
                                {
                                    army.focus = Some(target);
                                    out.push(PlayerCommand {
                                        player: me,
                                        command: Command::Attack {
                                            units: army.members.clone(),
                                            target: crate::ids::Target::Unit(target),
                                            queue: false,
                                        },
                                    });
                                }
                                continue;
                            }
                            let current_focus = army.focus.filter(|target| {
                                obs.enemy_units
                                    .iter()
                                    .any(|unit| unit.id == *target && legal_focus(unit))
                            });
                            let focus = current_focus.or_else(choose_focus);
                            if army.focus != focus {
                                let previous_focus = army.focus;
                                army.focus = focus;
                                if let Some(target) = focus {
                                    let target_domain = obs
                                        .enemy_units
                                        .iter()
                                        .find(|unit| unit.id == target)
                                        .expect("a selected focus remains observable")
                                        .kind
                                        .stats()
                                        .domain;
                                    let units = members
                                        .iter()
                                        .filter(|member| {
                                            !is_artillery(member)
                                                && can_attack_domain(member, target_domain)
                                        })
                                        .map(|member| member.id)
                                        .collect();
                                    out.push(PlayerCommand {
                                        player: me,
                                        command: Command::Attack {
                                            units,
                                            target: crate::ids::Target::Unit(target),
                                            queue: false,
                                        },
                                    });
                                } else if previous_focus.is_some() {
                                    let units: Vec<UnitId> = members
                                        .iter()
                                        .filter(|member| {
                                            !is_artillery(member)
                                                && can_attack_domain(
                                                    member,
                                                    crate::stats::Domain::Ground,
                                                )
                                        })
                                        .map(|member| member.id)
                                        .collect();
                                    if !units.is_empty() {
                                        out.push(PlayerCommand {
                                            player: me,
                                            command: Command::AttackMove {
                                                units,
                                                goal: army.target.unwrap_or(army.staging),
                                                queue: false,
                                            },
                                        });
                                    }
                                }
                            }
                        } else {
                            army.focus = None;
                        }
                    }
                }
                ArmyState::Withdrawing => {
                    if tiles_within(centroid, army.staging, 2) {
                        army.progress = None;
                        // A routed player-facing body that reaches its fallback
                        // while the same fight is still on top of it must hold
                        // that line. Re-staging immediately makes the next
                        // high-cadence think focus a target, chase away from the
                        // fallback, and trigger another identical retreat.
                        // Its existing attack-move already answers local fire;
                        // fresh production can muster as a separate body.
                        let threat_remains = army
                            .target
                            .is_some_and(|target| withdrawal_area_contested(obs, target));
                        if !player_facing || (!in_contact && !threat_remains) {
                            army.state = ArmyState::Staging;
                            army.target = None;
                        }
                    } else if wedged(
                        &mut army.progress,
                        centroid.chebyshev(army.staging),
                        obs.tick,
                    ) {
                        // The way home is as unroutable as the way out.
                        // Rally here; Recall and Push become meaningful
                        // again instead of both being illegal forever.
                        army.state = ArmyState::Staging;
                        army.staging = centroid;
                        army.progress = None;
                    }
                }
            }
        }
        self.armies.retain(|a| !a.members.is_empty());
        out
    }

    #[cfg(test)]
    fn maintain_reference(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        rear: TilePos,
        player_facing: bool,
    ) -> Vec<PlayerCommand> {
        let centroid_frame = if player_facing {
            Some(
                self.player_frame
                    .get_or_insert_with(|| PlayerFacingTactics {
                        frame: CentroidFrame::for_rear(obs, rear),
                        defense_focus: None,
                    })
                    .frame,
            )
        } else {
            None
        };
        let roster = LinearUnitRoster(&obs.my_units);
        self.maintain_with_roster(
            me,
            obs,
            rear,
            MaintenanceMode {
                player_facing,
                centroid_frame,
                coordinated_focus: true,
                coordinated_defense_focus: false,
            },
            &roster,
        )
    }

    fn maintain_defense_focus(
        &mut self,
        me: PlayerId,
        obs: &Observation,
        coordinated: bool,
        out: &mut Vec<PlayerCommand>,
    ) {
        let tactics = self
            .player_frame
            .as_mut()
            .expect("player-facing maintenance latches tactical state first");
        if !coordinated {
            tactics.defense_focus = None;
            return;
        }

        let current = tactics.defense_focus.as_ref().map(|(target, _)| *target);
        let selected = coordinated_defense_target(obs, current);
        if tactics.defense_focus == selected {
            return;
        }
        tactics.defense_focus = selected.clone();
        if let Some((target, buildings)) = selected {
            out.push(PlayerCommand {
                player: me,
                command: Command::FocusFire { buildings, target },
            });
        }
    }
}

fn coordinated_defense_target(
    obs: &Observation,
    current: Option<Target>,
) -> Option<(Target, Vec<BuildingId>)> {
    let candidate = |enemy: &UnitObs| {
        let target_domain = enemy.kind.stats().domain;
        let target_center = enemy.tile.center();
        let mut buildings: Vec<_> = obs
            .my_buildings
            .iter()
            .filter(|building| building.built && building.hp > 0)
            .filter_map(|building| {
                let stats = building.kind.tier_stats(building.tier);
                let weapon = stats.weapons.first()?;
                if !weapon.targets.covers(target_domain) {
                    return None;
                }
                let far = building.anchor.offset(
                    stats.size.0.saturating_sub(1),
                    stats.size.1.saturating_sub(1),
                );
                let center = (building.anchor.center() + far.center()) * chassis::fx::HALF;
                let distance = center.dist_sq(target_center);
                (distance >= weapon.minimum_range * weapon.minimum_range
                    && distance <= weapon.range * weapon.range)
                    .then_some((building.id, distance))
            })
            .collect();
        if buildings.len() < 2 {
            return None;
        }
        buildings.sort_unstable_by_key(|(building, _)| *building);
        let nearest = buildings
            .iter()
            .map(|(_, distance)| *distance)
            .min()
            .expect("an overlapping defense pair has a nearest member");
        let ids = buildings
            .into_iter()
            .map(|(building, _)| building)
            .collect();
        Some((ids, nearest))
    };
    let legal = |enemy: &&UnitObs| {
        obs.visible(enemy.tile)
            && (enemy.kind.stats().can_target(crate::stats::Domain::Ground)
                || enemy.kind.stats().demolition)
    };

    if let Some(Target::Unit(target)) = current
        && let Some(enemy) = obs
            .enemy_units
            .iter()
            .filter(legal)
            .find(|enemy| enemy.id == target)
        && let Some((buildings, _)) = candidate(enemy)
    {
        return Some((Target::Unit(target), buildings));
    }

    obs.enemy_units
        .iter()
        .filter(legal)
        .filter_map(|enemy| {
            let (ids, nearest) = candidate(enemy)?;
            Some(((enemy.hp, Reverse(ids.len()), nearest, enemy.id), ids))
        })
        .min_by_key(|(key, _)| *key)
        .map(|((_, _, _, target), buildings)| (Target::Unit(target), buildings))
}

/// A long gun: ordered reach beyond its own eyes. It fires on the
/// team's sight, so it must never lead the march into what it cannot
/// see.
pub(super) fn is_artillery(u: &UnitObs) -> bool {
    let stats = u.kind.stats();
    stats
        .max_range_vs(crate::stats::Domain::Ground)
        .is_some_and(|r| r > chassis::fx::Fx::from_num(stats.vision))
}

fn can_attack_domain(unit: &UnitObs, domain: crate::stats::Domain) -> bool {
    let stats = unit.kind.stats();
    stats.can_target(domain) || (stats.demolition && domain == crate::stats::Domain::Ground)
}

fn can_focus_without_chasing(attacker: &UnitObs, target: &UnitObs) -> bool {
    let distance = attacker.tile.chebyshev(target.tile);
    let target_domain = target.kind.stats().domain;
    attacker.kind.stats().weapons.iter().any(|weapon| {
        weapon.targets.covers(target_domain) && distance <= weapon.range.ceil().to_num::<i32>()
    }) || (attacker.kind.stats().demolition
        && target_domain == crate::stats::Domain::Ground
        && distance <= crate::stats::SAPPER_CONTACT_RANGE.ceil().to_num::<i32>())
}

/// Whether the army has enough non-artillery bodies to move its long guns
/// out of staging. This is the shared contract for marching and pre-march
/// strength estimates.
pub(super) fn artillery_has_escort_quorum(army: &Army, obs: &Observation) -> bool {
    let roster = UnitRoster::new(&obs.my_units);
    artillery_has_escort_quorum_with_roster(army, &roster)
}

fn artillery_has_escort_quorum_with_roster<'a>(army: &Army, roster: &impl UnitLookup<'a>) -> bool {
    let escorts = army
        .members
        .iter()
        .filter(|id| roster.get(**id).is_some_and(|unit| !is_artillery(unit)))
        .count();
    escorts * 3 >= army.members.len()
}

/// How far short of the push target artillery parks — inside its own
/// reach of the target, outside a defending turret's.
const ARTY_STANDOFF: i32 = 7;

/// Marching orders for a push: escorts attack-move onto the target;
/// artillery holds a standoff point pulled back along the line of
/// advance — and without an escort quorum (a third of the army) the
/// guns stay at the staging ground instead. Nobody pushes blind
/// artillery.
pub(super) fn march(
    me: PlayerId,
    obs: &Observation,
    army: &Army,
    target: TilePos,
    out: &mut Vec<PlayerCommand>,
) {
    let roster = UnitRoster::new(&obs.my_units);
    march_with_roster(me, obs, army, target, out, &roster);
}

fn march_with_roster<'a>(
    me: PlayerId,
    _obs: &'a Observation,
    army: &Army,
    target: TilePos,
    out: &mut Vec<PlayerCommand>,
    roster: &impl UnitLookup<'a>,
) {
    let (arty, escorts): (Vec<UnitId>, Vec<UnitId>) = army
        .members
        .iter()
        .partition(|id| roster.get(**id).is_some_and(is_artillery));
    if !escorts.is_empty() {
        out.push(PlayerCommand {
            player: me,
            command: Command::AttackMove {
                units: escorts.clone(),
                goal: target,
                queue: false,
            },
        });
    }
    if arty.is_empty() {
        return;
    }
    if artillery_has_escort_quorum_with_roster(army, roster) {
        let (dx, dy) = (army.staging.x - target.x, army.staging.y - target.y);
        let d = dx.abs().max(dy.abs());
        let stand = if d == 0 {
            target
        } else {
            let pull = ARTY_STANDOFF.min(d);
            TilePos::new(target.x + dx * pull / d, target.y + dy * pull / d)
        };
        out.push(PlayerCommand {
            player: me,
            command: Command::AttackMove {
                units: arty,
                goal: stand,
                queue: false,
            },
        });
    } else {
        out.push(PlayerCommand {
            player: me,
            command: Command::Move {
                units: arty,
                goal: army.staging,
                queue: false,
            },
        });
    }
}

/// The escorts' mean tile — artillery hanging back must not drag the
/// army's sense of "arrived" backward with it. Falls back to the whole
/// body for a pure-artillery force.
pub(super) fn vanguard_centroid(
    members: &[UnitId],
    obs: &Observation,
    centroid_frame: Option<CentroidFrame>,
) -> TilePos {
    let roster = UnitRoster::new(&obs.my_units);
    let members = roster.members(members);
    vanguard_centroid_for(&members, centroid_frame)
}

fn vanguard_centroid_for(members: &[&UnitObs], centroid_frame: Option<CentroidFrame>) -> TilePos {
    let escorts: Vec<&UnitObs> = members
        .iter()
        .copied()
        .filter(|unit| !is_artillery(unit))
        .collect();
    if escorts.is_empty() {
        centroid(members, centroid_frame)
    } else {
        centroid(&escorts, centroid_frame)
    }
}

/// Mean member tile (integer division — a macro-scale center).
/// Advance a march's wedge clock: records a strictly better distance,
/// and reports true once the best has stood unimproved for the whole
/// patience window.
pub(super) fn wedged(progress: &mut Option<(i32, u64)>, distance: i32, tick: u64) -> bool {
    match progress {
        Some((best, _)) if distance < *best => {
            *progress = Some((distance, tick));
            false
        }
        Some((_, since)) => tick.saturating_sub(*since) >= ARMY_PROGRESS_PATIENCE_TICKS,
        None => {
            *progress = Some((distance, tick));
            false
        }
    }
}

fn centroid(members: &[&UnitObs], centroid_frame: Option<CentroidFrame>) -> TilePos {
    if members.is_empty() {
        TilePos::new(0, 0)
    } else {
        let (sx, sy) = members.iter().fold((0i64, 0i64), |(sx, sy), unit| {
            let tile = centroid_frame.map_or(unit.tile, |frame| frame.tile(unit.tile));
            (sx + i64::from(tile.x), sy + i64::from(tile.y))
        });
        let count = members.len() as i64;
        let tile = TilePos::new((sx / count) as i32, (sy / count) as i32);
        centroid_frame.map_or(tile, |frame| frame.tile(tile))
    }
}

/// Whether an army counts as fighting: a third of it (at least one
/// member) has an armed enemy inside `radius`. A lone straggler brushing
/// past an enemy is not the army's fight — quorum keeps the state
/// machine from being yanked around by grazing contact.
fn enemies_near(obs: &Observation, members: &[&UnitObs], radius: i32) -> bool {
    let touched = members
        .iter()
        .filter(|member| {
            obs.enemy_units.iter().any(|enemy| {
                mutually_relevant(member, enemy) && member.tile.chebyshev(enemy.tile) <= radius
            })
        })
        .count();
    touched > 0 && touched * 3 >= members.len()
}

fn withdrawal_threat(obs: &Observation, members: &[&UnitObs]) -> Option<TilePos> {
    obs.enemy_units
        .iter()
        .filter(|enemy| obs.visible(enemy.tile))
        .filter_map(|enemy| {
            members
                .iter()
                .filter(|member| mutually_relevant(member, enemy))
                .map(|member| member.tile.chebyshev(enemy.tile))
                .min()
                .filter(|distance| *distance <= ENGAGE_RADIUS)
                .map(|distance| (distance, enemy.tile.y, enemy.tile.x, enemy.id, enemy.tile))
        })
        .min()
        .map(|(.., tile)| tile)
}

fn withdrawal_area_contested(obs: &Observation, target: TilePos) -> bool {
    obs.enemy_units.iter().any(|enemy| {
        obs.visible(enemy.tile)
            && enemy.kind.stats().can_fight()
            && enemy.tile.chebyshev(target) <= ENGAGE_RADIUS
    })
}

fn objective_cleared(obs: &Observation, target: TilePos) -> bool {
    obs.visible(target)
        && !obs
            .enemy_units
            .iter()
            .any(|unit| unit.tile.chebyshev(target) <= OBJECTIVE_RADIUS)
        && !obs
            .enemy_buildings
            .iter()
            .any(|building| building.anchor.chebyshev(target) <= OBJECTIVE_RADIUS)
}
/// Strength sums for an army's fight: every member counts (the army is
/// the fighting body wherever its parts stand), and the opposition is
/// every enemy within the engagement radius of a member that is itself
/// in contact. Anchoring on fighting members instead of a centroid keeps
/// the estimate stable when the line bends — a mean position can land in
/// empty ground and blind every radius test around it — while stragglers
/// don't sweep distant enemies into the count.
fn local_strength(obs: &Observation, members: &[&UnitObs]) -> (u64, u64) {
    let engaged: Vec<TilePos> = members
        .iter()
        .filter(|m| {
            obs.enemy_units
                .iter()
                .any(|e| mutually_relevant(m, e) && m.tile.chebyshev(e.tile) <= CONTACT_RADIUS)
        })
        .map(|m| m.tile)
        .collect();
    let opposition: Vec<&UnitObs> = obs
        .enemy_units
        .iter()
        .filter(|e| engaged.iter().any(|m| m.chebyshev(e.tile) <= ENGAGE_RADIUS))
        .collect();
    matched_strength(members, &opposition)
}

/// Whether marching this body into the visible force near `target` would
/// immediately cross the Executive's own catastrophic-withdrawal floor.
/// Utility uses this before recommitting a body that has already made it home;
/// otherwise its staged state erases the evidence that the same fight just
/// routed it.
pub(in crate::bot) fn catastrophically_outmatched_near(
    obs: &Observation,
    members: &[UnitId],
    target: TilePos,
    radius: i32,
) -> bool {
    let roster = UnitRoster::new(&obs.my_units);
    let mine_units = roster.members(members);
    let opposition: Vec<&UnitObs> = obs
        .enemy_units
        .iter()
        .filter(|unit| obs.visible(unit.tile) && unit.tile.chebyshev(target) <= radius)
        .collect();
    let (mine, theirs) = matched_strength(&mine_units, &opposition);
    catastrophic_matchup(mine, theirs)
}

/// Whether at least two members already near a visible threat hold a clear
/// matched-strength advantage. This is the local-defense exception to a full
/// offensive muster: it lets an existing screen crush a lone intruder without
/// sending one fresh unit across the map as a trickle.
pub(in crate::bot) fn locally_overmatches_near(
    obs: &Observation,
    members: &[UnitId],
    target: TilePos,
    radius: i32,
) -> bool {
    let roster = UnitRoster::new(&obs.my_units);
    let mine_units: Vec<_> = roster
        .members(members)
        .into_iter()
        .filter(|unit| unit.tile.chebyshev(target) <= radius)
        .collect();
    if mine_units.len() < 2 {
        return false;
    }
    let opposition: Vec<_> = obs
        .enemy_units
        .iter()
        .filter(|unit| obs.visible(unit.tile) && unit.tile.chebyshev(target) <= radius)
        .collect();
    let (mine, theirs) = matched_strength(&mine_units, &opposition);
    theirs > 0 && mine.saturating_mul(2) >= theirs.saturating_mul(3)
}

fn catastrophic_matchup(mine: u64, theirs: u64) -> bool {
    theirs > 0
        && mine.saturating_mul(u64::from(WITHDRAW_MARGIN_DEN))
            < theirs.saturating_mul(u64::from(WITHDRAW_MARGIN_NUM))
}

fn matched_strength(mine_units: &[&UnitObs], opposition: &[&UnitObs]) -> (u64, u64) {
    use crate::stats::Domain;
    // Matched pairs: each side is worth what it can actually apply to
    // the domains the other side fields. An interceptor over a pure
    // ground brawl contributes nothing to either column.
    let domains_of = |units: &[&UnitObs]| {
        let ground = units
            .iter()
            .any(|u| u.kind.stats().domain == Domain::Ground);
        let air = units.iter().any(|u| u.kind.stats().domain == Domain::Air);
        (ground, air)
    };
    let (their_ground, their_air) = domains_of(opposition);
    let (my_ground, my_air) = domains_of(mine_units);
    let applicable = |u: &UnitObs, ground: bool, air: bool| -> u64 {
        let g = if ground {
            strength_vs(u, Domain::Ground)
        } else {
            0
        };
        let a = if air { strength_vs(u, Domain::Air) } else { 0 };
        g.max(a)
    };
    let mine: u64 = mine_units
        .iter()
        .map(|u| applicable(u, their_ground, their_air))
        .sum();
    let theirs: u64 = opposition
        .iter()
        .map(|u| applicable(u, my_ground, my_air))
        .sum();
    (mine, theirs)
}

fn tiles_within(a: TilePos, b: TilePos, radius: i32) -> bool {
    a.chebyshev(b) <= radius
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::observation::{BuildingObs, OBSERVATION_VERSION};
    use crate::ids::BuildingId;
    use crate::state::Faction;

    fn unit(
        id: u32,
        player: PlayerId,
        kind: UnitKind,
        tile: TilePos,
        hp: u32,
        idle: bool,
    ) -> UnitObs {
        UnitObs {
            id: UnitId(id),
            player,
            kind,
            tile,
            hp,
            idle,
            carrying: 0,
            cargo: 0,
            site: None,
            salvaging: None,
            founding: None,
            repairing: false,
        }
    }

    fn observation(
        tick: u64,
        map_size: (i32, i32),
        my_units: Vec<UnitObs>,
        enemy_units: Vec<UnitObs>,
    ) -> Observation {
        let cells = usize::try_from(map_size.0 * map_size.1).unwrap();
        Observation {
            version: OBSERVATION_VERSION,
            tick,
            me: PlayerId(0),
            scrap: 0,
            map_width: map_size.0,
            map_height: map_size.1,
            my_units,
            my_buildings: Vec::new(),
            my_queues: Vec::new(),
            ally_units: Vec::new(),
            ally_buildings: Vec::new(),
            enemy_units,
            enemy_buildings: Vec::new(),
            visible: vec![true; cells],
            explored: vec![true; cells],
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

    fn half_turn(tile: TilePos, map_size: (i32, i32)) -> TilePos {
        TilePos::new(map_size.0 - 1 - tile.x, map_size.1 - 1 - tile.y)
    }

    fn army(id: u32, members: Vec<UnitId>, state: ArmyState, staging: TilePos) -> Army {
        Army {
            id: ArmyId(id),
            members,
            state,
            staging,
            target: None,
            focus: None,
            progress: None,
            issued: None,
            bounces: 0,
        }
    }

    fn two_ground_defenses(enemy_units: Vec<UnitObs>) -> Observation {
        let mut obs = observation(0, (32, 24), Vec::new(), enemy_units);
        obs.my_buildings = vec![
            building(9, PlayerId(0), BuildingKind::Turret, TilePos::new(10, 9)),
            building(11, PlayerId(0), BuildingKind::Turret, TilePos::new(10, 12)),
            building(
                12,
                PlayerId(0),
                BuildingKind::FlakTurret,
                TilePos::new(11, 10),
            ),
        ];
        obs.my_queues = vec![Vec::new(); obs.my_buildings.len()];
        obs
    }

    #[test]
    fn coordinated_static_defenses_lock_one_visible_threat_without_command_churn() {
        let primary = unit(
            100,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(14, 10),
            15,
            false,
        );
        let competitor = unit(
            101,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(14, 11),
            30,
            false,
        );
        let mut obs = two_ground_defenses(vec![primary, competitor]);
        let mut executive = Executive::default();
        let rear = TilePos::new(2, 2);

        let initial =
            executive.maintain_player_facing_with_tactics(PlayerId(0), &obs, rear, false, true);
        assert_eq!(
            initial,
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::FocusFire {
                    buildings: vec![BuildingId(9), BuildingId(11)],
                    target: Target::Unit(UnitId(100)),
                },
            }]
        );

        obs.tick = 12;
        obs.enemy_units[1].hp = 1;
        assert!(
            executive
                .maintain_player_facing_with_tactics(PlayerId(0), &obs, rear, false, true,)
                .is_empty(),
            "a still-legal focus must not churn when another target becomes weaker"
        );

        obs.tick = 24;
        obs.enemy_units.remove(0);
        assert_eq!(
            executive.maintain_player_facing_with_tactics(PlayerId(0), &obs, rear, false, true,),
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::FocusFire {
                    buildings: vec![BuildingId(9), BuildingId(11)],
                    target: Target::Unit(UnitId(101)),
                },
            }],
            "losing the preferred contact should retarget the same overlapping line once"
        );
    }

    #[test]
    fn coordinated_static_defenses_require_current_sight_a_real_threat_and_an_overlap() {
        let enemy = unit(
            100,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(14, 10),
            15,
            false,
        );
        let rear = TilePos::new(2, 2);

        let mut single = two_ground_defenses(vec![enemy.clone()]);
        single
            .my_buildings
            .retain(|building| building.id != BuildingId(11));
        let mut executive = Executive::default();
        assert!(
            executive
                .maintain_player_facing_with_tactics(PlayerId(0), &single, rear, false, true,)
                .is_empty(),
            "one ground defense is ordinary acquisition, not coordination"
        );

        let mut hidden = two_ground_defenses(vec![enemy]);
        let target = hidden.enemy_units[0].tile;
        let index = usize::try_from(target.y * hidden.map_width + target.x).unwrap();
        hidden.visible[index] = false;
        assert!(
            Executive::default()
                .maintain_player_facing_with_tactics(PlayerId(0), &hidden, rear, false, true,)
                .is_empty(),
            "an omniscient fixture must not make a hidden target legal"
        );

        let harmless_aa = unit(
            101,
            PlayerId(1),
            UnitKind::Flakhound,
            TilePos::new(14, 10),
            1,
            false,
        );
        let harmless = two_ground_defenses(vec![harmless_aa]);
        assert!(
            Executive::default()
                .maintain_player_facing_with_tactics(PlayerId(0), &harmless, rear, false, true,)
                .is_empty(),
            "static ground defense must not be distracted by an AA-only crawler"
        );
    }

    #[test]
    fn coordinated_static_defenses_select_only_the_compatible_domain_line() {
        let bomber = unit(
            100,
            PlayerId(1),
            UnitKind::Condor,
            TilePos::new(14, 10),
            UnitKind::Condor.stats().max_hp,
            false,
        );
        let mut obs = observation(0, (32, 24), Vec::new(), vec![bomber]);
        obs.my_buildings = vec![
            building(8, PlayerId(0), BuildingKind::Turret, TilePos::new(10, 8)),
            building(
                9,
                PlayerId(0),
                BuildingKind::FlakTurret,
                TilePos::new(10, 9),
            ),
            building(
                11,
                PlayerId(0),
                BuildingKind::FlakTurret,
                TilePos::new(10, 12),
            ),
        ];
        obs.my_queues = vec![Vec::new(); obs.my_buildings.len()];

        let commands = Executive::default().maintain_player_facing_with_tactics(
            PlayerId(0),
            &obs,
            TilePos::new(2, 2),
            false,
            true,
        );

        assert_eq!(
            commands,
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::FocusFire {
                    buildings: vec![BuildingId(9), BuildingId(11)],
                    target: Target::Unit(UnitId(100)),
                },
            }],
            "the atomic command must exclude an incompatible ground-only turret"
        );
    }

    #[test]
    fn coordinated_static_defense_selection_is_half_turn_equivariant() {
        let map_size = (32, 24);
        let enemy_tile = TilePos::new(14, 10);
        let enemy = unit(100, PlayerId(1), UnitKind::Sentinel, enemy_tile, 15, false);
        let left = two_ground_defenses(vec![enemy]);
        let mut right = left.clone();
        right.me = PlayerId(1);
        for building in &mut right.my_buildings {
            building.player = PlayerId(1);
            building.anchor = half_turn(building.anchor, map_size);
        }
        right.enemy_units[0].player = PlayerId(0);
        right.enemy_units[0].tile = half_turn(enemy_tile, map_size);

        let left_command = Executive::default().maintain_player_facing_with_tactics(
            PlayerId(0),
            &left,
            TilePos::new(2, 2),
            false,
            true,
        );
        let right_command = Executive::default().maintain_player_facing_with_tactics(
            PlayerId(1),
            &right,
            half_turn(TilePos::new(2, 2), map_size),
            false,
            true,
        );

        assert_eq!(left_command.len(), 1);
        assert_eq!(right_command.len(), 1);
        assert_eq!(left_command[0].command, right_command[0].command);
    }

    fn assert_reference_parity(
        executive: &Executive,
        obs: &Observation,
        rear: TilePos,
        player_facing: bool,
    ) -> (Executive, Vec<PlayerCommand>) {
        let mut optimized = executive.clone();
        let mut reference = executive.clone();
        let commands = if player_facing {
            optimized.maintain_player_facing(PlayerId(0), obs, rear)
        } else {
            optimized.maintain(PlayerId(0), obs, rear)
        };
        let expected = reference.maintain_reference(PlayerId(0), obs, rear, player_facing);
        assert_eq!(
            commands, expected,
            "command order diverged from linear lookup"
        );
        assert_eq!(
            optimized, reference,
            "Executive state diverged from linear lookup"
        );
        (optimized, commands)
    }

    #[test]
    fn owner_facing_centroid_ties_are_half_turn_equivariant() {
        let map_size = (48, 30);
        let left_tiles = [
            TilePos::new(19, 14),
            TilePos::new(19, 15),
            TilePos::new(18, 14),
            TilePos::new(19, 14),
            TilePos::new(18, 16),
            TilePos::new(18, 17),
            TilePos::new(18, 15),
        ];
        let left_units: Vec<_> = left_tiles
            .iter()
            .enumerate()
            .map(|(rank, tile)| {
                unit(
                    rank as u32,
                    PlayerId(0),
                    UnitKind::Sentinel,
                    *tile,
                    UnitKind::Sentinel.stats().max_hp,
                    false,
                )
            })
            .collect();
        let right_units: Vec<_> = left_tiles
            .iter()
            .enumerate()
            .map(|(rank, tile)| {
                unit(
                    rank as u32,
                    PlayerId(1),
                    UnitKind::Sentinel,
                    half_turn(*tile, map_size),
                    UnitKind::Sentinel.stats().max_hp,
                    false,
                )
            })
            .collect();
        let left_obs = observation(0, map_size, left_units, Vec::new());
        let mut right_obs = observation(0, map_size, right_units, Vec::new());
        right_obs.me = PlayerId(1);
        let left_members: Vec<_> = left_obs.my_units.iter().collect();
        let right_members: Vec<_> = right_obs.my_units.iter().collect();
        let left_frame = CentroidFrame::for_rear(&left_obs, TilePos::new(5, 5));
        let right_frame = CentroidFrame::for_rear(&right_obs, TilePos::new(42, 24));

        let left = centroid(&left_members, Some(left_frame));
        let right = centroid(&right_members, Some(right_frame));
        assert_eq!(left, TilePos::new(18, 15));
        assert_eq!(right, half_turn(left, map_size));
        assert_eq!(centroid(&left_members, None), left);
        assert_ne!(centroid(&right_members, None), right);

        let integral_left_units = [
            unit(
                100,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(18, 14),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                101,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(20, 16),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
        ];
        let integral_right_units = [
            unit(
                100,
                PlayerId(1),
                UnitKind::Sentinel,
                half_turn(integral_left_units[0].tile, map_size),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                101,
                PlayerId(1),
                UnitKind::Sentinel,
                half_turn(integral_left_units[1].tile, map_size),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
        ];
        let integral_left: Vec<_> = integral_left_units.iter().collect();
        let integral_right: Vec<_> = integral_right_units.iter().collect();
        assert_eq!(
            centroid(&integral_left, Some(left_frame)),
            centroid(&integral_left, None),
            "an exact integer mean must not move in the identity frame"
        );
        assert_eq!(
            centroid(&integral_right, Some(right_frame)),
            centroid(&integral_right, None),
            "an exact integer mean must not move in the flipped frame"
        );
    }

    #[test]
    fn mirrored_odd_bodies_cross_the_objective_boundary_together() {
        let map_size = (48, 30);
        let left_tiles = [
            TilePos::new(19, 14),
            TilePos::new(19, 15),
            TilePos::new(18, 14),
            TilePos::new(19, 14),
            TilePos::new(18, 16),
            TilePos::new(18, 17),
            TilePos::new(18, 15),
        ];
        let left_units: Vec<_> = left_tiles
            .iter()
            .enumerate()
            .map(|(rank, tile)| {
                unit(
                    rank as u32,
                    PlayerId(0),
                    UnitKind::Sentinel,
                    *tile,
                    UnitKind::Sentinel.stats().max_hp,
                    false,
                )
            })
            .collect();
        let right_units: Vec<_> = left_tiles
            .iter()
            .enumerate()
            .map(|(rank, tile)| {
                unit(
                    rank as u32,
                    PlayerId(1),
                    UnitKind::Sentinel,
                    half_turn(*tile, map_size),
                    UnitKind::Sentinel.stats().max_hp,
                    false,
                )
            })
            .collect();
        let left_obs = observation(11_853, map_size, left_units, Vec::new());
        let mut right_obs = observation(11_853, map_size, right_units, Vec::new());
        right_obs.me = PlayerId(1);
        let left_target = TilePos::new(21, 15);
        let right_target = half_turn(left_target, map_size);
        let mut left_body = army(
            0,
            (0..left_tiles.len())
                .map(|rank| UnitId(rank as u32))
                .collect(),
            ArmyState::Pushing,
            TilePos::new(8, 8),
        );
        left_body.target = Some(left_target);
        let mut right_body = left_body.clone();
        right_body.staging = half_turn(left_body.staging, map_size);
        right_body.target = Some(right_target);
        let mut left = Executive {
            armies: vec![left_body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };
        let mut right = Executive {
            armies: vec![right_body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        assert!(
            left.maintain_player_facing(PlayerId(0), &left_obs, TilePos::new(5, 5))
                .is_empty()
        );
        assert!(
            right
                .maintain_player_facing(PlayerId(1), &right_obs, TilePos::new(42, 24))
                .is_empty()
        );
        assert_eq!(left.armies[0].state, ArmyState::Pushing);
        assert_eq!(right.armies[0].state, ArmyState::Pushing);
        assert_eq!(right.armies[0].target, Some(right_target));
        assert_eq!(right.armies[0].members, left.armies[0].members);

        let left_push = left.apply_with_reservations(
            PlayerId(0),
            &left_obs,
            &[Intent::PushArmy {
                army: ArmyId(0),
                target: left_target,
            }],
            &[],
        );
        let right_push = right.apply_with_reservations(
            PlayerId(1),
            &right_obs,
            &[Intent::PushArmy {
                army: ArmyId(0),
                target: right_target,
            }],
            &[],
        );
        assert_eq!(left_push.len(), 1);
        assert_eq!(right_push.len(), 1);
        let left_issued = left.armies[0].issued.expect("left march was recorded").1;
        let right_issued = right.armies[0].issued.expect("right march was recorded").1;
        assert_eq!(right_issued, half_turn(left_issued, map_size));

        let mut left_idle = left_obs.clone();
        left_idle.tick += 3;
        for unit in &mut left_idle.my_units {
            unit.idle = true;
        }
        let mut right_idle = right_obs.clone();
        right_idle.tick += 3;
        for unit in &mut right_idle.my_units {
            unit.idle = true;
        }
        assert!(
            left.maintain_player_facing(PlayerId(0), &left_idle, TilePos::new(5, 5))
                .is_empty()
        );
        assert!(
            right
                .maintain_player_facing(PlayerId(1), &right_idle, TilePos::new(42, 24))
                .is_empty()
        );
        assert_eq!(left.armies[0].bounces, 1);
        assert_eq!(right.armies[0].bounces, 1);
        assert_eq!(left.armies[0].issued, None);
        assert_eq!(right.armies[0].issued, None);
    }

    #[test]
    fn player_facing_arrival_at_a_cleared_objective_releases_the_force() {
        let target = TilePos::new(20, 20);
        let sentinel = unit(
            1,
            PlayerId(0),
            UnitKind::Sentinel,
            target,
            UnitKind::Sentinel.stats().max_hp,
            false,
        );
        let obs = observation(10, (40, 40), vec![sentinel], Vec::new());
        let mut body = army(0, vec![UnitId(1)], ArmyState::Pushing, TilePos::new(4, 4));
        body.target = Some(target);
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let commands = executive.maintain_player_facing(PlayerId(0), &obs, TilePos::new(4, 4));

        assert!(commands.is_empty());
        assert!(
            executive.armies.is_empty(),
            "a force standing on a visibly empty objective must return to the shared pool"
        );
    }

    #[test]
    fn player_facing_route_failures_release_both_pushes_and_withdrawals() {
        let sentinel = unit(
            1,
            PlayerId(0),
            UnitKind::Sentinel,
            TilePos::new(10, 10),
            UnitKind::Sentinel.stats().max_hp,
            false,
        );
        let tick = ARMY_PROGRESS_PATIENCE_TICKS;
        let obs = observation(tick, (40, 40), vec![sentinel], Vec::new());

        let mut stuck_push = army(0, vec![UnitId(1)], ArmyState::Pushing, TilePos::new(2, 2));
        stuck_push.target = Some(TilePos::new(30, 10));
        stuck_push.progress = Some((20, 0));
        let mut pushing = Executive {
            armies: vec![stuck_push],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };
        assert!(
            pushing
                .maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2))
                .is_empty()
        );
        assert!(
            pushing.armies.is_empty(),
            "an operation whose push made no progress must release its member for replanning"
        );

        let mut stuck_withdrawal = army(
            1,
            vec![UnitId(1)],
            ArmyState::Withdrawing,
            TilePos::new(2, 2),
        );
        stuck_withdrawal.progress = Some((8, 0));
        let mut withdrawing = Executive {
            armies: vec![stuck_withdrawal],
            next_army: 2,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };
        assert!(
            withdrawing
                .maintain_player_facing(PlayerId(0), &obs, TilePos::new(2, 2))
                .is_empty()
        );
        assert_eq!(withdrawing.armies.len(), 1);
        assert_eq!(withdrawing.armies[0].state, ArmyState::Staging);
        assert_eq!(withdrawing.armies[0].staging, TilePos::new(10, 10));
        assert_eq!(withdrawing.armies[0].progress, None);
    }

    #[test]
    fn player_facing_army_completes_a_contact_to_objective_lifecycle() {
        let staging = TilePos::new(4, 4);
        let target = TilePos::new(30, 10);
        let units = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                2,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 11),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(11, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
        ];
        let enemy = unit(
            101,
            PlayerId(1),
            UnitKind::Scuttler,
            TilePos::new(13, 10),
            UnitKind::Scuttler.stats().max_hp,
            false,
        );
        assert!(
            units
                .iter()
                .all(|unit| can_focus_without_chasing(unit, &enemy)),
            "the lifecycle fixture needs a legal whole-formation focus target"
        );
        let mut body = army(
            0,
            vec![UnitId(1), UnitId(2), UnitId(3)],
            ArmyState::Staging,
            staging,
        );
        body.target = Some(target);
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let mut contact = observation(10, (40, 40), units.clone(), vec![enemy.clone()]);
        let target_index = usize::try_from(target.y * contact.map_width + target.x).unwrap();
        contact.visible[target_index] = false;
        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &contact, staging)
                .is_empty()
        );
        assert_eq!(executive.armies[0].state, ArmyState::Engaging);

        let commands = executive.maintain_player_facing(PlayerId(0), &contact, staging);
        assert_eq!(commands.len(), 1);
        assert!(
            matches!(
                &commands[0].command,
                Command::Attack { units, target: Target::Unit(UnitId(101)), queue: false }
                    if units == &vec![UnitId(1), UnitId(2), UnitId(3)]
            ),
            "unexpected contact command: {commands:?}"
        );

        let mut corridor = observation(11, (40, 40), units.clone(), Vec::new());
        let target_index = usize::try_from(target.y * corridor.map_width + target.x).unwrap();
        corridor.visible[target_index] = false;
        let commands = executive.maintain_player_facing(PlayerId(0), &corridor, staging);
        assert_eq!(executive.armies[0].state, ArmyState::Pushing);
        assert!(matches!(
            &commands[0].command,
            Command::AttackMove { units, goal, queue: false }
                if units == &vec![UnitId(1), UnitId(2), UnitId(3)] && *goal == target
        ));

        let arrived_units: Vec<UnitObs> = units
            .iter()
            .enumerate()
            .map(|(index, unit)| {
                let mut unit = unit.clone();
                unit.tile = TilePos::new(target.x - i32::try_from(index).unwrap(), target.y);
                unit
            })
            .collect();
        let mut defended = observation(12, (40, 40), arrived_units.clone(), Vec::new());
        defended
            .enemy_buildings
            .push(building(201, PlayerId(1), BuildingKind::Foundry, target));
        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &defended, staging)
                .is_empty()
        );
        assert_eq!(executive.armies[0].state, ArmyState::Staging);
        assert_eq!(executive.armies[0].target, Some(target));

        let cleared = observation(13, (40, 40), arrived_units, Vec::new());
        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &cleared, staging)
                .is_empty()
        );
        assert!(
            executive.armies.is_empty(),
            "the body should return to the shared roster only after current sight proves the objective clear"
        );
    }

    #[test]
    fn player_facing_focus_stays_locked_until_the_target_is_no_longer_legal() {
        let staging = TilePos::new(4, 4);
        let members = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                2,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 11),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(11, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
        ];
        let original = unit(
            101,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(13, 10),
            30,
            false,
        );
        let competitor = unit(
            102,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(13, 11),
            UnitKind::Sentinel.stats().max_hp,
            false,
        );
        let mut obs = observation(0, (40, 40), members, vec![original, competitor]);
        let body = army(
            0,
            vec![UnitId(1), UnitId(2), UnitId(3)],
            ArmyState::Engaging,
            staging,
        );
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let mut uncoordinated = executive.clone();
        assert!(
            uncoordinated
                .maintain_player_facing_with_tactics(PlayerId(0), &obs, staging, false, false)
                .is_empty(),
            "an easier army should let the simulation's ordinary acquisition choose each unit's target"
        );
        assert_eq!(uncoordinated.armies[0].focus, None);

        let initial = executive.maintain_player_facing(PlayerId(0), &obs, staging);
        assert!(matches!(
            initial.as_slice(),
            [PlayerCommand {
                command: Command::Attack {
                    target: Target::Unit(UnitId(101)),
                    ..
                },
                ..
            }]
        ));

        obs.tick = 6;
        obs.enemy_units[1].hp = 1;
        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &obs, staging)
                .is_empty(),
            "a faster think must not replace a still-legal focus when another target becomes weaker"
        );
        assert_eq!(executive.armies[0].focus, Some(UnitId(101)));

        obs.tick = 12;
        obs.enemy_units[1].hp = UnitKind::Sentinel.stats().max_hp;
        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &obs, staging)
                .is_empty(),
            "leaving the instantaneous argmin must not churn the locked order either"
        );
        assert_eq!(executive.armies[0].focus, Some(UnitId(101)));

        for invalidation in ["gone", "hidden", "distant", "untargetable"] {
            let mut variant = executive.clone();
            let mut changed = obs.clone();
            changed.tick = 18;
            match invalidation {
                "gone" => changed.enemy_units.retain(|unit| unit.id != UnitId(101)),
                "hidden" => {
                    let target = changed.enemy_units[0].tile;
                    let index = usize::try_from(target.y * changed.map_width + target.x).unwrap();
                    changed.visible[index] = false;
                }
                "distant" => changed.enemy_units[0].tile = TilePos::new(30, 30),
                "untargetable" => changed.enemy_units[0].kind = UnitKind::Buzzard,
                _ => unreachable!(),
            }

            let commands = variant.maintain_player_facing(PlayerId(0), &changed, staging);
            assert!(
                matches!(
                    commands.as_slice(),
                    [PlayerCommand {
                        command: Command::Attack {
                            target: Target::Unit(UnitId(102)),
                            ..
                        },
                        ..
                    }]
                ),
                "{invalidation} focus did not retarget to the remaining legal contact: {commands:?}"
            );
            assert_eq!(variant.armies[0].focus, Some(UnitId(102)));
        }
    }

    #[test]
    fn coordinated_focus_ignores_harmless_aa_and_keeps_specialists_on_their_orders() {
        let staging = TilePos::new(4, 4);
        let members = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                2,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 11),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Flakhound,
                TilePos::new(11, 10),
                UnitKind::Flakhound.stats().max_hp,
                false,
            ),
            unit(
                4,
                PlayerId(0),
                UnitKind::Bombard,
                TilePos::new(9, 10),
                UnitKind::Bombard.stats().max_hp,
                false,
            ),
        ];
        let harmless_aa = unit(
            101,
            PlayerId(1),
            UnitKind::Flakhound,
            TilePos::new(13, 10),
            1,
            false,
        );
        let ground_threat = unit(
            102,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(13, 11),
            UnitKind::Sentinel.stats().max_hp,
            false,
        );
        let mut obs = observation(0, (40, 40), members, vec![harmless_aa, ground_threat]);
        let body = army(
            0,
            vec![UnitId(1), UnitId(2), UnitId(3), UnitId(4)],
            ArmyState::Engaging,
            staging,
        );
        let executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        for permuted in [false, true] {
            let mut variant = executive.clone();
            if permuted {
                obs.enemy_units.reverse();
            }
            let commands = variant.maintain_player_facing(PlayerId(0), &obs, staging);

            assert_eq!(
                commands,
                vec![PlayerCommand {
                    player: PlayerId(0),
                    command: Command::Attack {
                        units: vec![UnitId(1), UnitId(2)],
                        target: Target::Unit(UnitId(102)),
                        queue: false,
                    },
                }],
                "focus must select a real threat without retasking AA or long guns: {:?}",
                variant.armies
            );
            assert_eq!(variant.armies[0].focus, Some(UnitId(102)));
        }
    }

    #[test]
    fn coordinated_focus_never_pulls_a_distant_frontliner_off_the_army_march() {
        let staging = TilePos::new(4, 4);
        let target = unit(
            101,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(13, 10),
            UnitKind::Sentinel.stats().max_hp,
            false,
        );
        let body = army(
            0,
            vec![UnitId(1), UnitId(2), UnitId(3)],
            ArmyState::Engaging,
            staging,
        );
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };
        let spread_body = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(8, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                2,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 11),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
        ];
        let spread = observation(0, (40, 40), spread_body, vec![target.clone()]);

        let commands = executive.maintain_player_facing(PlayerId(0), &spread, staging);

        assert!(
            commands.is_empty(),
            "the existing attack-move must remain authoritative instead of making the rear member chase: {commands:?}"
        );
        assert_eq!(executive.armies[0].focus, None);

        let compact_body = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 9),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                2,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 11),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
        ];
        let compact = observation(12, (40, 40), compact_body, vec![target]);

        let commands = executive.maintain_player_facing(PlayerId(0), &compact, staging);

        assert_eq!(
            commands,
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::Attack {
                    units: vec![UnitId(1), UnitId(2), UnitId(3)],
                    target: Target::Unit(UnitId(101)),
                    queue: false,
                },
            }],
            "once the complete front line is in its firing envelope, concentrating fire is legal"
        );
        assert_eq!(executive.armies[0].focus, Some(UnitId(101)));
    }

    #[test]
    fn losing_the_last_legal_focus_resumes_the_army_objective() {
        let staging = TilePos::new(4, 4);
        let objective = TilePos::new(30, 30);
        let members = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                2,
                PlayerId(0),
                UnitKind::Flakhound,
                TilePos::new(10, 11),
                UnitKind::Flakhound.stats().max_hp,
                false,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Bombard,
                TilePos::new(9, 10),
                UnitKind::Bombard.stats().max_hp,
                false,
            ),
        ];
        let harmless_aa = unit(
            102,
            PlayerId(1),
            UnitKind::Flakhound,
            TilePos::new(13, 10),
            UnitKind::Flakhound.stats().max_hp,
            false,
        );
        let mut body = army(
            0,
            vec![UnitId(1), UnitId(2), UnitId(3)],
            ArmyState::Engaging,
            staging,
        );
        body.target = Some(objective);
        body.focus = Some(UnitId(101));
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };
        let obs = observation(12, (40, 40), members, vec![harmless_aa]);

        let commands = executive.maintain_player_facing(PlayerId(0), &obs, staging);

        assert_eq!(executive.armies[0].focus, None);
        assert_eq!(
            commands,
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::AttackMove {
                    units: vec![UnitId(1)],
                    goal: objective,
                    queue: false,
                },
            }],
            "front-line units must resume the operation while artillery retains its standoff order"
        );
    }

    #[test]
    fn coordinated_focus_treats_demolition_as_ground_attack_capability() {
        let staging = TilePos::new(4, 4);
        let members = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                2,
                PlayerId(0),
                UnitKind::Sapper,
                TilePos::new(10, 11),
                UnitKind::Sapper.stats().max_hp,
                false,
            ),
        ];
        let enemy_sapper = unit(
            101,
            PlayerId(1),
            UnitKind::Sapper,
            TilePos::new(11, 10),
            1,
            false,
        );
        let enemy_sentinel = unit(
            102,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(13, 11),
            UnitKind::Sentinel.stats().max_hp,
            false,
        );
        let obs = observation(0, (40, 40), members, vec![enemy_sapper, enemy_sentinel]);
        let body = army(0, vec![UnitId(1), UnitId(2)], ArmyState::Engaging, staging);
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let commands = executive.maintain_player_facing(PlayerId(0), &obs, staging);

        assert_eq!(
            commands,
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::Attack {
                    units: vec![UnitId(1), UnitId(2)],
                    target: Target::Unit(UnitId(101)),
                    queue: false,
                },
            }]
        );
    }

    #[test]
    fn profile_free_focus_preserves_legacy_whole_army_targeting() {
        let staging = TilePos::new(4, 4);
        let members = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                2,
                PlayerId(0),
                UnitKind::Flakhound,
                TilePos::new(10, 11),
                UnitKind::Flakhound.stats().max_hp,
                false,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Bombard,
                TilePos::new(9, 10),
                UnitKind::Bombard.stats().max_hp,
                false,
            ),
        ];
        let weakest_gun = unit(
            101,
            PlayerId(1),
            UnitKind::Flakhound,
            TilePos::new(13, 10),
            1,
            false,
        );
        let other_gun = unit(
            102,
            PlayerId(1),
            UnitKind::Sentinel,
            TilePos::new(13, 11),
            UnitKind::Sentinel.stats().max_hp,
            false,
        );
        let obs = observation(0, (40, 40), members, vec![weakest_gun, other_gun]);
        let body = army(
            0,
            vec![UnitId(1), UnitId(2), UnitId(3)],
            ArmyState::Engaging,
            staging,
        );
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let commands = executive.maintain(PlayerId(0), &obs, staging);

        assert_eq!(
            commands,
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::Attack {
                    units: vec![UnitId(1), UnitId(2), UnitId(3)],
                    target: Target::Unit(UnitId(101)),
                    queue: false,
                },
            }]
        );
    }

    #[test]
    fn losing_focus_after_the_last_frontliner_dies_emits_no_empty_order() {
        let staging = TilePos::new(4, 4);
        let bombard = unit(
            3,
            PlayerId(0),
            UnitKind::Bombard,
            TilePos::new(9, 10),
            UnitKind::Bombard.stats().max_hp,
            false,
        );
        let harmless_aa = unit(
            102,
            PlayerId(1),
            UnitKind::Flakhound,
            TilePos::new(13, 10),
            UnitKind::Flakhound.stats().max_hp,
            false,
        );
        let mut body = army(0, vec![UnitId(3)], ArmyState::Engaging, staging);
        body.target = Some(TilePos::new(30, 30));
        body.focus = Some(UnitId(101));
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };
        let obs = observation(12, (40, 40), vec![bombard], vec![harmless_aa]);

        let commands = executive.maintain_player_facing(PlayerId(0), &obs, staging);

        assert_eq!(executive.armies[0].focus, None);
        assert!(
            commands.is_empty(),
            "an artillery survivor must retain its standoff order instead of receiving an invalid empty command"
        );
    }

    #[test]
    fn destroyed_members_leave_the_next_push_and_survivors_release_on_objective_clear() {
        let staging = TilePos::new(4, 4);
        let target = TilePos::new(24, 10);
        let survivors = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 11),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
        ];
        let mut contact_ended = observation(20, (40, 24), survivors.clone(), Vec::new());
        contact_ended.enemy_buildings.push(building(
            50,
            PlayerId(1),
            BuildingKind::Foundry,
            target,
        ));
        let mut body = army(
            0,
            vec![UnitId(1), UnitId(2), UnitId(3)],
            ArmyState::Engaging,
            staging,
        );
        body.target = Some(target);
        body.focus = Some(UnitId(90));
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let commands = executive.maintain_player_facing(PlayerId(0), &contact_ended, staging);
        assert_eq!(
            commands,
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::AttackMove {
                    units: vec![UnitId(1), UnitId(3)],
                    goal: target,
                    queue: false,
                },
            }],
            "the destroyed member must not survive into the next operation command"
        );
        assert_eq!(executive.armies.len(), 1);
        assert_eq!(executive.armies[0].members, vec![UnitId(1), UnitId(3)]);
        assert_eq!(executive.armies[0].state, ArmyState::Pushing);
        assert_eq!(executive.armies[0].target, Some(target));
        assert_eq!(executive.armies[0].focus, None);

        let arrived: Vec<_> = survivors
            .into_iter()
            .enumerate()
            .map(|(index, mut unit)| {
                unit.tile = target.offset(-i32::try_from(index).unwrap(), 0);
                unit
            })
            .collect();
        let objective_clear = observation(21, (40, 24), arrived, Vec::new());
        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &objective_clear, staging)
                .is_empty(),
            "a visibly cleared objective needs no terminal order"
        );
        assert!(
            executive.armies.is_empty(),
            "the surviving members must return to the shared roster once current sight proves the objective clear"
        );
    }

    #[test]
    fn current_sight_releases_an_engaging_army_without_a_stale_order_or_reservation() {
        let staging = TilePos::new(4, 4);
        let target = TilePos::new(24, 10);
        let survivors = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                target.offset(-1, 0),
                UnitKind::Sentinel.stats().max_hp,
                true,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Sentinel,
                target.offset(0, 1),
                UnitKind::Sentinel.stats().max_hp,
                true,
            ),
        ];
        let observation = observation(24, (40, 24), survivors, Vec::new());
        let mut body = army(0, vec![UnitId(1), UnitId(3)], ArmyState::Engaging, staging);
        body.target = Some(target);
        body.focus = Some(UnitId(90));
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &observation, staging)
                .is_empty(),
            "current negative evidence should not emit a terminal attack, retreat, or regroup"
        );
        assert!(
            executive.armies.is_empty(),
            "the cleared Engaging body must release every survivor back to the shared roster"
        );
        assert!(executive.enlisted().next().is_none());
    }

    #[test]
    fn pure_artillery_uses_its_own_centroid_and_holds_at_staging_without_escorts() {
        let staging = TilePos::new(4, 4);
        let target = TilePos::new(30, 10);
        let artillery = vec![
            unit(
                10,
                PlayerId(0),
                UnitKind::Bombard,
                TilePos::new(10, 10),
                UnitKind::Bombard.stats().max_hp,
                false,
            ),
            unit(
                20,
                PlayerId(0),
                UnitKind::Avalanche,
                TilePos::new(12, 10),
                UnitKind::Avalanche.stats().max_hp,
                false,
            ),
        ];
        assert!(artillery.iter().all(is_artillery));
        let mut obs = observation(30, (40, 24), artillery, Vec::new());
        obs.enemy_buildings
            .push(building(50, PlayerId(1), BuildingKind::Foundry, target));
        let mut body = army(
            0,
            vec![UnitId(10), UnitId(20)],
            ArmyState::Engaging,
            staging,
        );
        body.target = Some(target);
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let commands = executive.maintain_player_facing(PlayerId(0), &obs, staging);
        assert_eq!(
            commands,
            vec![PlayerCommand {
                player: PlayerId(0),
                command: Command::Move {
                    units: vec![UnitId(10), UnitId(20)],
                    goal: staging,
                    queue: false,
                },
            }],
            "long guns without an escort quorum must hold at staging rather than advance alone"
        );
        assert_eq!(executive.armies[0].members, vec![UnitId(10), UnitId(20)]);
        assert_eq!(executive.armies[0].state, ArmyState::Pushing);
        assert_eq!(executive.armies[0].target, Some(target));
        assert_eq!(
            vanguard_centroid(&executive.armies[0].members, &obs, None),
            TilePos::new(11, 10),
            "a pure-artillery force must fall back to its own centroid"
        );

        obs.tick += 1;
        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &obs, staging)
                .is_empty(),
            "maintenance must not replace the staging hold with a blind target order"
        );
        assert_eq!(executive.armies[0].members, vec![UnitId(10), UnitId(20)]);
        assert_eq!(executive.armies[0].state, ArmyState::Pushing);
        assert_eq!(executive.armies[0].progress, Some((19, obs.tick)));
    }

    #[test]
    fn player_facing_ground_army_holds_position_instead_of_chasing_air() {
        let staging = TilePos::new(4, 4);
        let my_units = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                2,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(10, 11),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(11, 10),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                4,
                PlayerId(0),
                UnitKind::Flakhound,
                TilePos::new(11, 11),
                UnitKind::Flakhound.stats().max_hp,
                false,
            ),
        ];
        let enemy_units = vec![
            unit(
                101,
                PlayerId(1),
                UnitKind::Scuttler,
                TilePos::new(13, 10),
                UnitKind::Scuttler.stats().max_hp,
                false,
            ),
            unit(
                102,
                PlayerId(1),
                UnitKind::Buzzard,
                TilePos::new(12, 10),
                1,
                false,
            ),
        ];
        let obs = observation(20, (40, 40), my_units, enemy_units);
        let body = army(
            0,
            vec![UnitId(1), UnitId(2), UnitId(3), UnitId(4)],
            ArmyState::Engaging,
            staging,
        );
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let commands = executive.maintain_player_facing(PlayerId(0), &obs, staging);

        assert_eq!(commands.len(), 1);
        assert!(matches!(
            commands[0].command,
            Command::Attack {
                target: Target::Unit(UnitId(101)),
                ..
            }
        ));
    }

    #[test]
    fn refused_pushes_release_player_facing_units_but_restage_overseer_units() {
        let staging = TilePos::new(4, 4);
        let current = TilePos::new(10, 10);
        let target = TilePos::new(30, 10);
        let sentinel = unit(
            1,
            PlayerId(0),
            UnitKind::Sentinel,
            current,
            UnitKind::Sentinel.stats().max_hp,
            true,
        );
        let mut obs = observation(11, (40, 40), vec![sentinel], Vec::new());
        let target_index = usize::try_from(target.y * obs.map_width + target.x).unwrap();
        obs.visible[target_index] = false;
        let mut body = army(0, vec![UnitId(1)], ArmyState::Pushing, staging);
        body.target = Some(target);
        body.issued = Some((10, current));
        body.bounces = 1;
        let executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let (player_facing, commands) = assert_reference_parity(&executive, &obs, staging, true);
        assert!(commands.is_empty());
        assert!(player_facing.armies.is_empty());

        let (overseer, commands) = assert_reference_parity(&executive, &obs, staging, false);
        assert!(commands.is_empty());
        assert_eq!(overseer.armies[0].state, ArmyState::Staging);
        assert_eq!(overseer.armies[0].staging, current);
        assert_eq!(overseer.armies[0].target, None);
        assert_eq!(overseer.armies[0].bounces, 0);
    }

    #[test]
    fn routed_player_facing_army_holds_a_contested_fallback_then_recovers() {
        let fallback = TilePos::new(10, 10);
        let sentinel = unit(
            1,
            PlayerId(0),
            UnitKind::Sentinel,
            fallback,
            UnitKind::Sentinel.stats().max_hp,
            false,
        );
        let enemy = unit(
            101,
            PlayerId(1),
            UnitKind::Breaker,
            TilePos::new(14, 10),
            UnitKind::Breaker.stats().max_hp,
            false,
        );
        let mut body = army(0, vec![UnitId(1)], ArmyState::Withdrawing, fallback);
        body.target = Some(enemy.tile);
        body.progress = Some((8, 0));
        let mut executive = Executive {
            armies: vec![body],
            next_army: 1,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        let contested = observation(20, (40, 40), vec![sentinel.clone()], vec![enemy]);
        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &contested, fallback)
                .is_empty()
        );
        assert_eq!(executive.armies[0].state, ArmyState::Withdrawing);
        assert_eq!(executive.armies[0].target, Some(TilePos::new(14, 10)));
        assert_eq!(executive.armies[0].progress, None);

        let clear = observation(21, (40, 40), vec![sentinel], Vec::new());
        assert!(
            executive
                .maintain_player_facing(PlayerId(0), &clear, fallback)
                .is_empty()
        );
        assert_eq!(executive.armies[0].state, ArmyState::Staging);
        assert_eq!(executive.armies[0].target, None);
    }

    #[test]
    fn sorted_lookup_matches_linear_maintenance_for_many_sparse_and_large_armies() {
        let mut units = Vec::new();
        for index in 0..1_000u32 {
            let kind = if index.is_multiple_of(17) {
                UnitKind::Bombard
            } else {
                UnitKind::Sentinel
            };
            units.push(unit(
                index * 3 + 1,
                PlayerId(0),
                kind,
                TilePos::new((index % 100) as i32 + 2, (index / 100) as i32 + 2),
                kind.stats().max_hp,
                false,
            ));
        }
        let mut obs = observation(500, (200, 200), units, Vec::new());
        let target = TilePos::new(180, 180);
        let target_index = usize::try_from(target.y * obs.map_width + target.x).unwrap();
        obs.visible[target_index] = false;

        let mut armies = Vec::new();
        for group in 0..100u32 {
            let start = group as usize * 5;
            let mut members: Vec<UnitId> = obs.my_units[start..start + 5]
                .iter()
                .map(|unit| unit.id)
                .collect();
            members.push(UnitId(10_000 + group));
            let state = if group % 2 == 0 {
                ArmyState::Staging
            } else {
                ArmyState::Pushing
            };
            let mut candidate = army(group, members, state, TilePos::new(2, 2));
            if state == ArmyState::Pushing {
                candidate.target = Some(target);
            }
            armies.push(candidate);
        }
        let mut large = army(
            100,
            obs.my_units[500..].iter().map(|unit| unit.id).collect(),
            ArmyState::Engaging,
            TilePos::new(2, 2),
        );
        large.target = Some(target);
        armies.push(large);
        let executive = Executive {
            armies,
            next_army: 101,
            rear: Vec::new(),
            exhausted_rear: Vec::new(),
            player_frame: None,
        };

        for player_facing in [false, true] {
            let (maintained, commands) =
                assert_reference_parity(&executive, &obs, TilePos::new(1, 1), player_facing);
            assert_eq!(maintained.armies.len(), 101);
            assert!(
                maintained
                    .armies
                    .iter()
                    .all(|army| { army.members.iter().all(|id| id.0 < 10_000) })
            );
            assert!(commands.iter().any(|command| matches!(
                command.command,
                Command::AttackMove { goal, .. } if goal == target
            )));
            assert!(
                commands.iter().any(|command| matches!(
                    command.command,
                    Command::AttackMove { goal, .. } if goal != target
                )),
                "the large mixed army sends artillery to its standoff separately"
            );
        }
    }

    #[test]
    fn sorted_lookup_matches_linear_rear_rotation_release_and_visible_contact() {
        let wounded = UnitKind::Sentinel.stats().max_hp * 3 / 10;
        let my_units = vec![
            unit(
                1,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(4, 4),
                wounded,
                true,
            ),
            unit(
                3,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(6, 4),
                wounded,
                true,
            ),
            unit(
                5,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(20, 20),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
            unit(
                7,
                PlayerId(0),
                UnitKind::Bombard,
                TilePos::new(20, 21),
                UnitKind::Bombard.stats().max_hp,
                false,
            ),
            unit(
                9,
                PlayerId(0),
                UnitKind::Sentinel,
                TilePos::new(21, 20),
                UnitKind::Sentinel.stats().max_hp,
                false,
            ),
        ];
        let enemy = unit(
            101,
            PlayerId(1),
            UnitKind::Breaker,
            TilePos::new(22, 20),
            UnitKind::Breaker.stats().max_hp,
            false,
        );
        let obs = observation(
            ARMY_PROGRESS_PATIENCE_TICKS + 1,
            (64, 64),
            my_units,
            vec![enemy],
        );
        let mut contact = army(
            1,
            vec![UnitId(5), UnitId(7), UnitId(9), UnitId(999)],
            ArmyState::Engaging,
            TilePos::new(4, 4),
        );
        contact.target = Some(TilePos::new(40, 40));
        let executive = Executive {
            armies: vec![
                army(
                    0,
                    vec![UnitId(3), UnitId(998)],
                    ArmyState::Staging,
                    TilePos::new(4, 4),
                ),
                contact,
            ],
            next_army: 2,
            rear: vec![RearUnit {
                id: UnitId(1),
                since: 0,
            }],
            exhausted_rear: vec![UnitId(997)],
            player_frame: None,
        };

        let (player_facing, player_commands) =
            assert_reference_parity(&executive, &obs, TilePos::new(2, 2), true);
        assert_eq!(
            player_facing
                .rear
                .iter()
                .map(|unit| unit.id)
                .collect::<Vec<_>>(),
            vec![UnitId(3)]
        );
        assert_eq!(player_facing.exhausted_rear, vec![UnitId(1)]);
        assert!(player_commands.iter().any(|command| matches!(
            &command.command,
            Command::Move { units, .. } if units == &vec![UnitId(3)]
        )));
        assert_eq!(player_facing.armies[0].state, ArmyState::Withdrawing);

        let (overseer, _) = assert_reference_parity(&executive, &obs, TilePos::new(2, 2), false);
        assert_eq!(
            overseer.rear.iter().map(|unit| unit.id).collect::<Vec<_>>(),
            vec![UnitId(1), UnitId(3)]
        );
        assert_eq!(overseer.exhausted_rear, vec![UnitId(997)]);
    }

    #[test]
    fn canonical_roster_work_scales_with_members_not_roster_times_armies() {
        let units: Vec<UnitObs> = (0..1_000u32)
            .map(|index| {
                unit(
                    index * 5 + 2,
                    PlayerId(0),
                    UnitKind::Sentinel,
                    TilePos::new((index % 100) as i32, (index / 100) as i32),
                    UnitKind::Sentinel.stats().max_hp,
                    true,
                )
            })
            .collect();
        let roster = UnitRoster::new(&units);

        for group in 0..100usize {
            let members: Vec<UnitId> = units[group * 10..group * 10 + 10]
                .iter()
                .map(|unit| unit.id)
                .collect();
            assert_eq!(roster.members(&members).len(), 10);
        }
        let (small_lookups, small_comparisons) = roster.work();
        assert_eq!(small_lookups, 1_000);
        assert!(
            small_comparisons <= small_lookups * 12,
            "binary search should need logarithmic comparisons: {small_comparisons}"
        );
        assert!(
            small_comparisons < 100 * units.len() / 4,
            "100 small armies must not approach one full-roster scan apiece"
        );

        let all_members: Vec<UnitId> = units.iter().map(|unit| unit.id).collect();
        assert_eq!(roster.members(&all_members).len(), units.len());
        let (total_lookups, total_comparisons) = roster.work();
        assert_eq!(total_lookups, 2_000);
        assert!(total_comparisons <= total_lookups * 12);
        assert!(
            total_comparisons < units.len() * all_members.len() / 20,
            "one large army must not regress to roster-by-members work"
        );
    }
}
