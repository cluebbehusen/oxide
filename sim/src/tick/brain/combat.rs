//! The fighting half of the brain: target acquisition, the weapons
//! matrix (direct, indirect, projectile, sidearms), turret fire, and
//! the retaliation contract. Every shot buffers into the tick's volley;
//! nothing here applies damage directly.

use super::super::route_for;
use super::PendingHit;
use super::locomotion::approach_rect;
use crate::event::{Event, StallReason};
use crate::ids::{PlayerId, Target, UnitId};
use crate::state::{Order, PathFollow, State};
use crate::stats::{Domain, WeaponStats};
use chassis::fx::Vec2Fx;
use chassis::grid::TilePos;

/// The movement domain a target occupies (buildings sit on the ground).
fn target_domain(state: &State, target: Target) -> Domain {
    match target {
        Target::Unit(uid) => state
            .unit(uid)
            .map_or(Domain::Ground, |u| u.kind.stats().domain),
        Target::Building(_) => Domain::Ground,
    }
}

/// Whether full terrain cover applies to a shot: only direct fire between
/// two ground parties traces rock and buildings — rock reaches nobody in
/// the air, and indirect shells arc over it. Peaks are checked on every
/// shot regardless (see the `shot_open` closures): a mountain outreaches
/// any arc.
fn traces_terrain(weapon: &WeaponStats, shooter: Domain, victim: Domain) -> bool {
    !weapon.indirect && shooter == Domain::Ground && victim == Domain::Ground
}

/// Buffers a shot: the direct hit, plus — for splash weapons — one hit on
/// every other hostile unit inside the radius that the weapon can cover.
/// Victims are chosen against the start-of-tick world like every other
/// decision this phase makes; buildings only ever take the direct hit.
///
/// Splash deliberately skips the owner-sight fire gate the aimed paths
/// enforce: the gate governs *choosing* a victim, and a shell in flight
/// chooses nothing — whatever stands in the blast is hit, seen or not.
/// No information leaks through the hole: the only emitted event names
/// the aimed victim, and retaliation stays gated on the sufferer seeing
/// the shooter, so an unseen bystander takes damage silently and nobody
/// learns anything they could not already see.
/// Launches a real projectile at the victim's fire-time position:
/// unguided from this instant, arriving after a distance-proportional
/// flight, resolving against whatever stands there then. Returns the
/// flight length for the launch event.
fn launch_shell(
    state: &State,
    launches: &mut Vec<crate::state::Shell>,
    attacker: Target,
    attacker_owner: PlayerId,
    from: Vec2Fx,
    aim: Vec2Fx,
    weapon: &WeaponStats,
) -> u64 {
    let dist = from.dist(aim);
    let flight = (dist / crate::stats::SHELL_SPEED)
        .ceil()
        .to_num::<u64>()
        .max(1);
    launches.push(crate::state::Shell {
        shooter: attacker,
        player: attacker_owner,
        launch: from,
        impact: aim,
        arrival: state.tick + flight,
        damage: weapon.damage,
        targets: weapon.targets,
        splash: weapon.splash,
    });
    flight
}

/// Arrived shells join this tick's volley, computed against the same
/// start-of-tick world every buffered shot uses. The direct hit lands
/// on the hostile building whose footprint covers the impact tile
/// (buildings cannot dodge — sieges are preserved); units take splash
/// only, which is the standing splash rule. No fire gate here: the
/// gate cleared at launch, and a shell in flight chooses nothing.
pub(super) fn land_shells(state: &mut State, hits: &mut Vec<PendingHit>, events: &mut Vec<Event>) {
    let now = state.tick;
    let mut due = Vec::new();
    state.shells.retain(|shell| {
        if shell.arrival <= now {
            due.push(shell.clone());
            false
        } else {
            true
        }
    });
    for shell in due {
        events.push(Event::ShellLanded {
            player: shell.player,
            targets: shell.targets,
            at: shell.impact,
            splash: shell.splash,
        });
        // The direct hit is distance-zero to a footprint, not tile
        // containment: a shell aimed at a building lands on the
        // footprint's closest EDGE point, whose exact coordinate floors
        // into the neighboring tile — containment alone made sieges
        // deal nothing. First hostile footprint touching the impact
        // (id order) takes the hit.
        let direct = state.buildings.iter().find(|b| {
            b.hp > 0
                && state.hostile(shell.player, b.player)
                && b.closest_point_to(shell.impact).dist_sq(shell.impact)
                    <= chassis::fx::Fx::lit("0.0001")
        });
        if let Some(b) = direct {
            hits.push(PendingHit {
                attacker: shell.shooter,
                victim: Target::Building(b.id),
                damage: shell.damage,
            });
        }
        let Some(radius) = shell.splash else { continue };
        let radius_sq = radius * radius;
        for u in state.units.iter() {
            if u.hp == 0
                || !state.hostile(shell.player, u.player)
                || !shell.targets.covers(u.kind.stats().domain)
                || u.pos.dist_sq(shell.impact) > radius_sq
            {
                continue;
            }
            hits.push(PendingHit {
                attacker: shell.shooter,
                victim: Target::Unit(u.id),
                damage: shell.damage,
            });
        }
    }
}

fn buffer_shot(
    state: &State,
    attacker: Target,
    attacker_owner: PlayerId,
    victim: Target,
    aim: Vec2Fx,
    weapon: &WeaponStats,
    hits: &mut Vec<PendingHit>,
) {
    hits.push(PendingHit {
        attacker,
        victim,
        damage: weapon.damage,
    });
    let Some(radius) = weapon.splash else { return };
    let radius_sq = radius * radius;
    for u in state.units.iter() {
        if u.hp == 0
            || !state.hostile(attacker_owner, u.player)
            || Target::Unit(u.id) == victim
            || !weapon.targets.covers(u.kind.stats().domain)
            || u.pos.dist_sq(aim) > radius_sq
        {
            continue;
        }
        hits.push(PendingHit {
            attacker,
            victim: Target::Unit(u.id),
            damage: weapon.damage,
        });
    }
}

/// Built turrets pick their own fights: nearest enemy unit in range with a
/// clear line (buildings can't chase, so out-of-line targets are simply
/// ignored until they move). Stateless — target choice re-evaluates every
/// shot, in building-id order.
pub(super) fn turret_fire(
    state: &mut State,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
) {
    let ids: Vec<crate::ids::BuildingId> = state.buildings.iter().map(|b| b.id).collect();
    for id in ids {
        let Some(b) = state.building(id) else {
            continue;
        };
        let Some(atk) = b.kind.stats().weapons.first() else {
            continue;
        };
        if !b.built || b.hp == 0 {
            continue;
        }
        let (me, center, cooling, kind) = (b.player, b.center(), b.cooldown > 0, b.kind);
        if cooling {
            let b = state.building_mut(id).expect("just seen");
            b.cooldown -= 1;
            if b.cooldown > 0 {
                continue;
            }
            // Reached zero this tick: fire now, like unit cooldowns do.
        }
        let range_sq = atk.range * atk.range;
        let shot_open = |t: TilePos, full: bool| {
            let Some(tile) = state.map.tile(t) else {
                return false;
            };
            if tile.terrain == crate::map::Terrain::Peak {
                return false;
            }
            !full
                || (tile.terrain == crate::map::Terrain::Ground
                    && state.building_at(t).is_none_or(|other| other.id == id))
        };
        // The owner must see the victim's tile — a turret that outranges
        // its own mast fires on a spotter's eyes, never into fog.
        let victim = state
            .units
            .iter()
            .filter(|u| {
                state.hostile(me, u.player) && u.hp > 0 && atk.targets.covers(u.kind.stats().domain)
            })
            .filter(|u| state.can_see(me, u.tile()))
            .map(|u| (center.dist_sq(u.pos), u.id, u.pos, u.kind.stats().domain))
            .filter(|(d, _, _, _)| *d <= range_sq)
            .filter(|(_, _, pos, dom)| {
                let full = traces_terrain(atk, Domain::Ground, *dom);
                !chassis::path::line_blocked(center, *pos, |t| shot_open(t, full))
            })
            .min_by_key(|&(d, uid, _, _)| (d, uid));
        let Some((_, uid, upos, _)) = victim else {
            continue;
        };
        let b = state.building_mut(id).expect("just seen");
        b.cooldown = atk.cooldown_ticks;
        if atk.projectile {
            let flight = launch_shell(state, launches, Target::Building(id), me, center, upos, atk);
            events.push(Event::ShellLaunched {
                shooter: Target::Building(id),
                player: me,
                from: center,
                to: upos,
                flight,
            });
        } else {
            buffer_shot(
                state,
                Target::Building(id),
                me,
                Target::Unit(uid),
                upos,
                atk,
                hits,
            );
            events.push(Event::TurretFired {
                turret: id,
                kind,
                target: uid,
                turret_pos: center,
                target_pos: upos,
            });
        }
    }
}

/// Stand up an own unfinished site: walk adjacent, then feed it progress.
/// One built tick raises hp along a linear ramp to full at completion
/// (damage taken meanwhile is simply kept — nobody rebuilds for free).

fn chase_stand_ins(
    state: &State,
    domain: Domain,
    around: TilePos,
    range: chassis::fx::Fx,
) -> Vec<TilePos> {
    /// Furthest ring hunted for standing room — covers the longest
    /// anti-air reach (range 5 lands exactly on ring 5's axis tiles).
    const CHASE_STAND_RADIUS: i32 = 5;
    let aim = around.center();
    let mut out = Vec::new();
    for r in 1..=CHASE_STAND_RADIUS {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let t = around.offset(dx, dy);
                if state.passable_for(domain, t) && t.center().dist_sq(aim) <= range * range {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// The nearest enemy this unit's weapons can cover, in aggro range —
/// units before buildings, ties to the lowest id. `None` for pacifists,
/// empty horizons, and everything outside the weapon masks (a flak
/// crawler never picks a fight with infantry it cannot shoot).
pub(super) fn acquire_target(state: &State, id: UnitId) -> Option<Target> {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if !stats.can_fight() {
        return None;
    }
    let (pos, me) = (unit.pos, unit.player);
    let aggro_sq = stats.aggro_range * stats.aggro_range;

    let unit_target = state
        .units
        .iter()
        .filter(|u| {
            state.hostile(me, u.player) && u.hp > 0 && stats.can_target(u.kind.stats().domain)
        })
        .map(|u| (pos.dist_sq(u.pos), u.id))
        .filter(|(d, _)| *d <= aggro_sq)
        .min();
    if let Some((_, uid)) = unit_target {
        return Some(Target::Unit(uid));
    }
    if !stats.can_target(Domain::Ground) {
        return None;
    }
    state
        .buildings
        .iter()
        .filter(|b| state.hostile(me, b.player) && b.hp > 0)
        .map(|b| (pos.dist_sq(b.closest_point_to(pos)), b.id))
        .filter(|(d, _)| *d <= aggro_sq)
        .min()
        .map(|(_, bid)| Target::Building(bid))
}

/// Idle combat units pick fights on their own.

pub(super) fn attack(
    state: &mut State,
    id: UnitId,
    target: Target,
    resume: Option<TilePos>,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
) {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if !stats.can_fight() {
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    }
    let (pos, tile, me, kind, cooldowns) = (
        unit.pos,
        unit.tile(),
        unit.player,
        unit.kind,
        unit.cooldowns,
    );

    // An attack-mover pounding a building stays alert: an enemy *unit*
    // wandering into aggro takes priority (deterministic — acquire prefers
    // units), so marching armies fight back instead of tunnel-visioning.
    if resume.is_some()
        && matches!(target, Target::Building(_))
        && let Some(better @ Target::Unit(_)) = acquire_target(state, id)
    {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target: better,
            resume,
        };
        unit.path = None;
        return;
    }

    // Resolve the target's current position; None means it is gone. A
    // target outside every weapon mask ends the engagement the same way —
    // nothing this chassis carries will ever land on it.
    let target_info: Option<(Vec2Fx, TilePos)> = match target {
        Target::Unit(uid) => state
            .unit(uid)
            .filter(|t| t.hp > 0)
            .map(|t| (t.pos, t.tile())),
        Target::Building(bid) => state
            .building(bid)
            .filter(|b| b.hp > 0)
            .map(|b| (b.closest_point_to(pos), b.anchor)),
    };
    let victim_domain = target_domain(state, target);
    let primary = stats
        .weapons
        .iter()
        .position(|w| w.targets.covers(victim_domain));
    let (Some((aim_point, target_tile)), Some(pi)) = (target_info, primary) else {
        let unit = state.unit_mut(id).expect("caller checked");
        match resume {
            Some(goal) => {
                unit.order = Order::AttackMove { goal };
                unit.path = None;
            }
            None => unit.advance_queue(),
        }
        return;
    };
    let weapon = &stats.weapons[pi];

    // In range only counts with a clear line — and with eyes. Terrain
    // cover (rock, non-victim buildings) applies to direct ground-vs-
    // ground fire only; shots to or from the air and indirect shells arc
    // past it — but nothing arcs past a peak. The owner must currently
    // *see* the victim's tile: a gun that outranges its own vision fires
    // on a spotter's sight (scrap piles are low junk — fire passes over
    // them). No shot → keep approaching; the chase path already routes
    // around what's in the way.
    let shot_open = |t: TilePos, full: bool| {
        let Some(tile) = state.map.tile(t) else {
            return false;
        };
        if tile.terrain == crate::map::Terrain::Peak {
            return false;
        }
        if !full {
            return true;
        }
        let building_open = match target {
            Target::Building(bid) => state.building_at(t).is_none_or(|b| b.id == bid),
            _ => state.building_at(t).is_none(),
        };
        tile.terrain == crate::map::Terrain::Ground && building_open
    };
    // Sight of any footprint tile serves for a building (matching attack
    // validation); a unit is seen at its own tile. The line trace runs
    // only once in range — it is not built for cross-map endpoints.
    let seen = match target {
        Target::Unit(_) => state.can_see(me, target_tile),
        Target::Building(bid) => state
            .building(bid)
            .is_some_and(|b| b.tiles().any(|t| state.can_see(me, t))),
    };
    let in_range = pos.dist_sq(aim_point) <= weapon.range * weapon.range;
    let full = traces_terrain(weapon, stats.domain, victim_domain);
    // The line trace skips endpoint tiles by design — but a building's
    // closest-footprint aim point is an exact edge coordinate that can
    // floor into the NEIGHBORING tile. Flush against a peak, that
    // neighbor is the mountain itself, and an unchecked endpoint let
    // direct fire through it. The endpoint tile must be open for this
    // shot too (a unit's aim point is its own standable tile, and a
    // footprint tile passes through shot_open's own-target exemption).
    let endpoint_open = shot_open(chassis::grid::TilePos::containing(aim_point), full);
    if in_range
        && seen
        && endpoint_open
        && !chassis::path::line_blocked(pos, aim_point, |t| shot_open(t, full))
    {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        if cooldowns[pi] == 0 {
            unit.cooldowns[pi] = weapon.cooldown_ticks;
            if weapon.projectile {
                let flight = launch_shell(
                    state,
                    launches,
                    Target::Unit(id),
                    me,
                    pos,
                    aim_point,
                    weapon,
                );
                events.push(Event::ShellLaunched {
                    shooter: Target::Unit(id),
                    player: me,
                    from: pos,
                    to: aim_point,
                    flight,
                });
            } else {
                buffer_shot(state, Target::Unit(id), me, target, aim_point, weapon, hits);
                events.push(Event::AttackHit {
                    attacker: id,
                    attacker_kind: kind,
                    weapon: pi,
                    target,
                    attacker_pos: pos,
                    target_pos: aim_point,
                });
            }
        }
        fire_sidearms(state, id, pi, hits, events);
        return;
    }
    // Opportunist guns don't wait for the march to end.
    fire_sidearms(state, id, pi, hits, events);

    // Out of range (or blind, or blocked): chase.
    let reached: Result<(), StallReason> = match target {
        Target::Unit(_) => {
            // A ground chaser cannot stand where a flyer hovers — over
            // rock, over a roof — so it marches to a tile it CAN stand
            // on and shoot from instead; getting within weapon range is
            // the job, occupying the victim's tile never was. Air
            // chasers (and reachable tiles) keep the direct goal.
            let direct = state.passable_for(stats.domain, target_tile);
            // Repath when the target has drifted a tile from the
            // path's goal — cheap pursuit without per-tick A*. A path
            // already aimed at a firing position for this victim stays
            // fresh while it parks, or a grounded chaser would repath
            // forever.
            let stale = state
                .unit(id)
                .expect("caller checked")
                .path
                .as_ref()
                .is_none_or(|p| {
                    if direct {
                        p.goal != target_tile && p.goal.chebyshev(target_tile) > 1
                    } else {
                        !(state.passable_for(stats.domain, p.goal)
                            && p.goal.center().dist_sq(target_tile.center())
                                <= weapon.range * weapon.range)
                    }
                });
            if stale {
                let routed = if direct {
                    route_for(state, kind, tile, target_tile).map(|w| (target_tile, w))
                } else {
                    // Scan-order candidates, first one that routes wins:
                    // an isolated pocket next to the victim must not
                    // stall a chaser that could fire from the far side.
                    chase_stand_ins(state, stats.domain, target_tile, weapon.range)
                        .into_iter()
                        .find_map(|goal| route_for(state, kind, tile, goal).map(|w| (goal, w)))
                };
                match routed {
                    Some((goal, waypoints)) => {
                        let unit = state.unit_mut(id).expect("caller checked");
                        unit.path = Some(PathFollow {
                            goal,
                            waypoints,
                            next: 0,
                        });
                        Ok(())
                    }
                    // NoFiringPosition derives from the victim's footing;
                    // speak it only while the team sees that ground, or
                    // the stall toast leaks where a fogged flyer parked.
                    // Unseen, the honest own-state fact is that no route
                    // worked.
                    None if !direct
                        && state.can_see(me, target_tile)
                        && chase_stand_ins(state, stats.domain, target_tile, weapon.range)
                            .is_empty() =>
                    {
                        Err(StallReason::NoFiringPosition)
                    }
                    None => Err(StallReason::NoRoute),
                }
            } else {
                Ok(())
            }
        }
        Target::Building(bid) => {
            let b = state.building(bid).expect("resolved above");
            let (anchor, size) = (b.anchor, b.kind.stats().size);
            if approach_rect(state, id, anchor, size) {
                Ok(())
            } else {
                Err(StallReason::NoRoute)
            }
        }
    };
    if let Err(reason) = reached {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.clear_program();
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason,
        });
    }
}

/// Weapons other than the one engaging the ordered target pick their own
/// fights: the nearest hostile unit each can cover, in range, seen by the
/// owner, and clear — opportunist fire that never steers the chassis.
fn fire_sidearms(
    state: &mut State,
    id: UnitId,
    primary: usize,
    hits: &mut Vec<PendingHit>,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if stats.weapons.len() < 2 {
        return;
    }
    let (pos, me, kind, cooldowns) = (unit.pos, unit.player, unit.kind, unit.cooldowns);
    for (wi, weapon) in stats.weapons.iter().enumerate() {
        if wi == primary || cooldowns[wi] > 0 {
            continue;
        }
        let range_sq = weapon.range * weapon.range;
        let shot_open = |t: TilePos, full: bool| {
            let Some(tile) = state.map.tile(t) else {
                return false;
            };
            if tile.terrain == crate::map::Terrain::Peak {
                return false;
            }
            !full || (tile.terrain == crate::map::Terrain::Ground && state.building_at(t).is_none())
        };
        let victim = state
            .units
            .iter()
            .filter(|u| {
                state.hostile(me, u.player)
                    && u.hp > 0
                    && weapon.targets.covers(u.kind.stats().domain)
            })
            .filter(|u| state.can_see(me, u.tile()))
            .map(|u| (pos.dist_sq(u.pos), u.id, u.pos, u.kind.stats().domain))
            .filter(|(d, _, _, _)| *d <= range_sq)
            .filter(|(_, _, upos, dom)| {
                let full = traces_terrain(weapon, stats.domain, *dom);
                !chassis::path::line_blocked(pos, *upos, |t| shot_open(t, full))
            })
            .min_by_key(|&(d, uid, _, _)| (d, uid));
        let Some((_, uid, upos, _)) = victim else {
            continue;
        };
        state.unit_mut(id).expect("caller checked").cooldowns[wi] = weapon.cooldown_ticks;
        buffer_shot(
            state,
            Target::Unit(id),
            me,
            Target::Unit(uid),
            upos,
            weapon,
            hits,
        );
        events.push(Event::AttackHit {
            attacker: id,
            attacker_kind: kind,
            weapon: wi,
            target: Target::Unit(uid),
            attacker_pos: pos,
            target_pos: upos,
        });
    }
}

/// Damage answers back: a hit unit that can fight and isn't already
/// fighting turns on its attacker — the counter to weapons that outrange
/// aggro (nothing else ever gets this far: inside aggro, auto-acquire
/// already found the attacker). An attack-mover keeps its destination as
/// the resume point. Brains run in id order, so the first hit of a tick
/// picks the target deterministically.
pub(super) fn retaliate(state: &mut State, victim: UnitId, attacker: Target) {
    let Some(unit) = state.unit(victim) else {
        return;
    };
    let stats = unit.kind.stats();
    let attacker_domain = target_domain(state, attacker);
    if unit.hp == 0 || !stats.can_target(attacker_domain) {
        return;
    }
    // Answering fire needs eyes: an indirect shell lobbed from beyond
    // every friendly sight line reveals nothing to march after — chasing
    // it would hand out free intel and a suicide route.
    let seen = match attacker {
        Target::Unit(uid) => state
            .unit(uid)
            .is_some_and(|a| state.can_see(unit.player, a.tile())),
        Target::Building(bid) => state
            .building(bid)
            .is_some_and(|b| b.tiles().any(|t| state.can_see(unit.player, t))),
    };
    if !seen {
        return;
    }
    let resume = match unit.order {
        Order::Idle => None,
        Order::AttackMove { goal } => Some(goal),
        // An attack aimed at something that just died in resolution is no
        // engagement — a victim auto-acquired a neighbor this tick, the
        // neighbor fell in the volley, and without this arm the busy-guard
        // would let a surviving out-of-aggro shooter fire unanswered for
        // another full cooldown. Live targets stay protected.
        Order::Attack { target, resume } if !target_standing(state, target) => resume,
        _ => return, // already busy fighting or working
    };
    let unit = state.unit_mut(victim).expect("checked above");
    unit.order = Order::Attack {
        target: attacker,
        resume,
    };
    unit.path = None;
}

/// Whether a target is still on the field with hit points.
pub(super) fn target_standing(state: &State, target: Target) -> bool {
    match target {
        Target::Unit(u) => state.unit(u).is_some_and(|u| u.hp > 0),
        Target::Building(b) => state.building(b).is_some_and(|b| b.hp > 0),
    }
}
