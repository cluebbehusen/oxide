//! The fighting half of the brain: target acquisition, the weapons
//! matrix (direct, indirect, projectile, sidearms), turret fire, and
//! the retaliation contract. Every shot buffers into the tick's volley;
//! nothing here applies damage directly.

use super::super::route_for;
use super::PendingHit;
use super::locomotion::{approach_rect, walk};
use crate::event::{Event, StallReason};
use crate::ids::{PlayerId, Target, UnitId};
use crate::state::{Order, PathFollow, State};
use crate::stats::{Domain, UnitKind, WeaponStats};
use chassis::fx::{Fx, Vec2Fx};
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
/// two ground parties traces rock — rock reaches nobody in the air, and
/// indirect shells arc over it. Buildings never block fire in any pairing:
/// they block movement, not bullets (terrain is the only cover). Peaks are
/// checked on every shot regardless (see the `shot_open` closures): a
/// mountain outreaches any arc.
fn traces_terrain(weapon: &WeaponStats, shooter: Domain, victim: Domain) -> bool {
    !weapon.indirect && shooter == Domain::Ground && victim == Domain::Ground
}

/// Whether a shot's line may pass over `t`. Peaks wall every pairing and
/// every arc; full-cover tracing (direct ground-vs-ground) additionally
/// respects terrain that grants cover. Pits block neither: machines trade
/// fire across a void nobody can walk.
fn shot_crosses(state: &State, t: TilePos, full: bool) -> bool {
    state.map.tile(t).is_some_and(|tile| {
        !tile.terrain.blocks_all_fire() && (!full || !tile.terrain.blocks_direct_fire())
    })
}

fn within_weapon_reach(weapon: &WeaponStats, distance_sq: chassis::fx::Fx) -> bool {
    distance_sq <= weapon.range * weapon.range
        && distance_sq >= weapon.minimum_range * weapon.minimum_range
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
/// A real projectile flies to one fixed fire-time aim point and resolves
/// against whatever stands there then. Predictive artillery chooses that
/// point before launch; this flight remains unguided. Returns the flight
/// length for the launch event.
fn shell_flight(from: Vec2Fx, aim: Vec2Fx) -> u64 {
    (from.dist(aim) / crate::stats::SHELL_SPEED)
        .ceil()
        .to_num::<u64>()
        .max(1)
}

/// One tick-boundary sample of the motion a visible body is already showing.
///
/// The sample is captured before any unit brain runs. That keeps artillery
/// independent of the alternating brain order, and recording only the current
/// steering line avoids reading an opponent's later A* turns or destination.
pub(super) struct MotionSnapshot {
    velocities: Vec<(UnitId, Vec2Fx)>,
}

impl MotionSnapshot {
    pub(super) fn capture(state: &State) -> Self {
        let velocities = state
            .units
            .iter()
            .filter_map(|unit| {
                let path = unit.path.as_ref()?;
                let next = path.waypoints.get(path.next as usize)?.center();
                let delta = next - unit.pos;
                let distance = delta.length();
                (distance > Fx::ZERO)
                    .then(|| (unit.id, delta * (unit.kind.stats().speed / distance)))
            })
            .collect();
        Self { velocities }
    }

    fn position_after(&self, target: UnitId, current: Vec2Fx, ticks: u64) -> Option<Vec2Fx> {
        const MAX_LEAD_TICKS: u64 = 96;

        let slot = self
            .velocities
            .binary_search_by_key(&target, |(id, _)| *id)
            .ok()?;
        Some(current + self.velocities[slot].1 * Fx::from_num(ticks.min(MAX_LEAD_TICKS)))
    }
}

fn predicted_impact_open(state: &State, from: Vec2Fx, aim: Vec2Fx, full: bool) -> bool {
    let shot_open = |tile: TilePos| shot_crosses(state, tile, full);
    shot_open(TilePos::containing(aim)) && !chassis::path::line_blocked(from, aim, shot_open)
}

fn position_along_current_motion(
    motion: &MotionSnapshot,
    target: UnitId,
    current: Vec2Fx,
    ticks: u64,
) -> Option<Vec2Fx> {
    motion.position_after(target, current, ticks)
}

fn aim_at_near_splash_edge(current: Vec2Fx, predicted: Vec2Fx, radius: Fx) -> Vec2Fx {
    predicted.move_toward(current, radius)
}

fn visible_hostile_cluster_at(
    state: &State,
    owner: PlayerId,
    current: Vec2Fx,
    weapon: &WeaponStats,
) -> bool {
    let Some(radius) = weapon.splash else {
        return false;
    };
    let radius_sq = radius * radius;
    state
        .units
        .iter()
        .filter(|unit| {
            unit.hp > 0
                && state.hostile(owner, unit.player)
                && weapon.targets.covers(unit.kind.stats().domain)
                && state.can_see(owner, unit.tile())
                && unit.pos.dist_sq(current) <= radius_sq
        })
        .take(2)
        .count()
        >= 2
}

#[derive(Clone, Copy)]
struct ProjectileShooter {
    owner: PlayerId,
    domain: Domain,
}

/// Leads a moving unit along its tick-boundary steering line for the shell's
/// estimated flight. For splash shells, the aim backs toward the current
/// position by one blast radius: a straight commitment remains just inside
/// the footprint, while stopping or turning gets a principled margin for
/// error. If the current footprint already contains a second visible eligible
/// hostile, the gun keeps that known cluster instead of leading away from it.
/// Later path turns remain private, and the target remains free to change
/// course after launch; shells are still unguided. Buildings keep their
/// closest-footprint aim unchanged.
fn projectile_aim(
    state: &State,
    motion: &MotionSnapshot,
    shooter: ProjectileShooter,
    from: Vec2Fx,
    target: Target,
    current: Vec2Fx,
    weapon: &WeaponStats,
) -> Vec2Fx {
    let Target::Unit(target) = target else {
        return current;
    };
    if visible_hostile_cluster_at(state, shooter.owner, current, weapon) {
        return current;
    }
    let mut flight = shell_flight(from, current);
    let mut aim = current;
    let mut unhedged_aim = current;
    // Ground artillery can only lead ground units, all slower than a shell.
    // Eight fixed iterations settle even the fastest current ground body;
    // the projection itself is bounded in case future balance breaks that
    // relationship.
    for _ in 0..8 {
        let Some(predicted) = position_along_current_motion(motion, target, current, flight) else {
            break;
        };
        let next_aim = weapon.splash.map_or(predicted, |radius| {
            aim_at_near_splash_edge(current, predicted, radius)
        });
        let distance_sq = from.dist_sq(next_aim);
        if distance_sq < weapon.minimum_range * weapon.minimum_range {
            // There is no legal predicted impact inside a weapon's dead zone.
            // Keep the target's current, already-validated aim instead of
            // inventing a radial boundary point the target never occupies.
            aim = current;
            unhedged_aim = current;
            break;
        }
        unhedged_aim = if from.dist_sq(predicted) > weapon.range * weapon.range {
            from.move_toward(predicted, weapon.range)
        } else {
            predicted
        };
        aim = if distance_sq > weapon.range * weapon.range {
            from.move_toward(next_aim, weapon.range)
        } else {
            next_aim
        };
        let next_flight = shell_flight(from, aim);
        if next_flight == flight {
            break;
        }
        flight = next_flight;
    }
    let full = traces_terrain(
        weapon,
        shooter.domain,
        target_domain(state, Target::Unit(target)),
    );
    if predicted_impact_open(state, from, unhedged_aim, full)
        && predicted_impact_open(state, from, aim, full)
    {
        aim
    } else {
        current
    }
}

fn launch_shell(
    state: &State,
    launches: &mut Vec<crate::state::Shell>,
    attacker: Target,
    attacker_owner: PlayerId,
    from: Vec2Fx,
    aim: Vec2Fx,
    weapon: &WeaponStats,
) -> u64 {
    let flight = shell_flight(from, aim);
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
/// ignored until they move). A Bastion with no eligible unit can shell a
/// currently visible hostile building. Stateless — target choice re-evaluates
/// every shot, in building-id order.
pub(super) fn turret_fire(
    state: &mut State,
    motion: &MotionSnapshot,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
) {
    let ids: Vec<crate::ids::BuildingId> = state.buildings.iter().map(|b| b.id).collect();
    for id in ids {
        let Some(b) = state.building(id) else {
            continue;
        };
        let Some(atk) = b.stats().weapons.first() else {
            continue;
        };
        if !b.built || b.hp == 0 {
            continue;
        }
        let (me, center, cooling, kind, focus) =
            (b.player, b.center(), b.cooldown > 0, b.kind, b.focus);
        let focus_domain = focus.and_then(|target| {
            state
                .visible_hostile_target_domain(me, target)
                .filter(|domain| atk.targets.covers(*domain))
        });
        if focus.is_some() && focus_domain.is_none() {
            state.building_mut(id).expect("just seen").focus = None;
        }
        if cooling {
            let b = state.building_mut(id).expect("just seen");
            b.cooldown -= 1;
            if b.cooldown > 0 {
                continue;
            }
            // Reached zero this tick: fire now, like unit cooldowns do.
        }
        let shot_open = |t: TilePos, full: bool| shot_crosses(state, t, full);
        // The owner must see the victim's tile — a turret that outranges
        // its own mast fires on a spotter's eyes, never into fog.
        let focused_victim = focus.zip(focus_domain).and_then(|(target, domain)| {
            let aim = match target {
                Target::Unit(unit) => state.unit(unit)?.pos,
                Target::Building(building) => state.building(building)?.closest_point_to(center),
            };
            let distance = center.dist_sq(aim);
            let full = traces_terrain(atk, Domain::Ground, domain);
            (within_weapon_reach(atk, distance)
                && shot_open(TilePos::containing(aim), full)
                && !chassis::path::line_blocked(center, aim, |tile| shot_open(tile, full)))
            .then_some((target, aim))
        });
        let unit_victim = focused_victim
            .is_none()
            .then(|| {
                state
                    .units
                    .iter()
                    .filter(|u| {
                        state.hostile(me, u.player)
                            && u.hp > 0
                            && atk.targets.covers(u.kind.stats().domain)
                    })
                    .filter(|u| state.can_see(me, u.tile()))
                    .map(|u| (center.dist_sq(u.pos), u.id, u.pos, u.kind.stats().domain))
                    .filter(|(d, _, _, _)| within_weapon_reach(atk, *d))
                    .filter(|(_, _, pos, dom)| {
                        let full = traces_terrain(atk, Domain::Ground, *dom);
                        !chassis::path::line_blocked(center, *pos, |t| shot_open(t, full))
                    })
                    .min_by_key(|&(d, uid, _, _)| (d, uid))
            })
            .flatten();
        let building_victim = (focused_victim.is_none()
            && unit_victim.is_none()
            && kind == crate::stats::BuildingKind::Bastion
            && atk.targets.covers(Domain::Ground))
        .then(|| {
            state
                .buildings
                .iter()
                .filter(|target| target.hp > 0 && state.hostile(me, target.player))
                .filter(|target| target.tiles().any(|tile| state.can_see(me, tile)))
                .map(|target| {
                    let aim = target.closest_point_to(center);
                    (center.dist_sq(aim), target.id, aim)
                })
                .filter(|(distance, _, _)| within_weapon_reach(atk, *distance))
                .filter(|(_, _, aim)| {
                    let full = traces_terrain(atk, Domain::Ground, Domain::Ground);
                    shot_open(TilePos::containing(*aim), full)
                        && !chassis::path::line_blocked(center, *aim, |tile| shot_open(tile, full))
                })
                .min_by_key(|&(distance, target, _)| (distance, target))
        })
        .flatten();
        let (victim, aim) = if let Some(focused) = focused_victim {
            focused
        } else if let Some((_, uid, position, _)) = unit_victim {
            (Target::Unit(uid), position)
        } else if let Some((_, target, position)) = building_victim {
            (Target::Building(target), position)
        } else {
            continue;
        };
        let aim = if atk.projectile {
            projectile_aim(
                state,
                motion,
                ProjectileShooter {
                    owner: me,
                    domain: Domain::Ground,
                },
                center,
                victim,
                aim,
                atk,
            )
        } else {
            aim
        };
        let b = state.building_mut(id).expect("just seen");
        b.cooldown = atk.cooldown_ticks;
        if atk.projectile {
            let flight = launch_shell(state, launches, Target::Building(id), me, center, aim, atk);
            events.push(Event::ShellLaunched {
                shooter: Target::Building(id),
                target: victim,
                player: me,
                from: center,
                to: aim,
                flight,
            });
        } else {
            buffer_shot(state, Target::Building(id), me, victim, aim, atk, hits);
            events.push(Event::TurretFired {
                turret: id,
                kind,
                target: victim,
                turret_pos: center,
                target_pos: aim,
            });
        }
    }
}

/// Firing positions for a chaser around an unstandable victim tile:
/// ring-scanned outward (row-major within a ring — the deterministic
/// snap every goal uses), keeping only tiles the chaser can stand on
/// AND shoot from — a stand-in beyond the weapon's Euclidean reach is
/// no stand-in at all (ring corners sit √2 further out than their
/// Chebyshev radius suggests). Candidates come back in scan order; the
/// caller takes the first it can actually route to. Empty when the
/// victim sits deeper in blocked ground than any weapon reaches.
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

/// The nearest enemy this unit's weapons can cover, in its autonomous
/// acquisition range —
/// units before buildings, ties to the lowest id. `None` for pacifists,
/// empty horizons, and everything outside the weapon masks (a flak
/// crawler never picks a fight with infantry it cannot shoot).
///
/// Unit candidates come from the phase's spatial `index` instead of a
/// full scan: everything within that range of `pos` stands within
/// `floor(range) + 1` tiles Chebyshev of its tile (the extra tile
/// covers both bodies' sub-tile offsets), so the window is a strict
/// superset of the old scan's survivors — and the pick is a `min` over
/// `(dist_sq, id)`, which no scan order can move.
pub(super) fn acquire_target(
    state: &State,
    index: &super::super::spatial::UnitIndex,
    id: UnitId,
) -> Option<Target> {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if !stats.can_fight() {
        return None;
    }
    let (pos, me) = (unit.pos, unit.player);
    let acquisition_range = stats.aggro_range;
    let needs_shared_sight = unit.kind == UnitKind::Bombard;
    let aggro_sq = acquisition_range * acquisition_range;

    let home = unit.tile();
    let reach = acquisition_range.floor().to_num::<i32>() + 1;
    let mut unit_target: Option<(chassis::fx::Fx, UnitId)> = None;
    for dy in -reach..=reach {
        for &(_, slot) in index.row_span(home.y + dy, home.x - reach, home.x + reach) {
            let u = &state.units[slot];
            if !state.hostile(me, u.player)
                || u.hp == 0
                || !stats.can_target(u.kind.stats().domain)
                || (needs_shared_sight && !state.can_see(me, u.tile()))
            {
                continue;
            }
            let d = pos.dist_sq(u.pos);
            let outside_dead_zone = stats.weapons.iter().any(|weapon| {
                weapon.targets.covers(u.kind.stats().domain)
                    && d >= weapon.minimum_range * weapon.minimum_range
            });
            if d <= aggro_sq && outside_dead_zone && unit_target.is_none_or(|best| (d, u.id) < best)
            {
                unit_target = Some((d, u.id));
            }
        }
    }
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
        .filter(|b| !needs_shared_sight || b.tiles().any(|tile| state.can_see(me, tile)))
        .map(|b| (pos.dist_sq(b.closest_point_to(pos)), b.id))
        .filter(|(d, _)| {
            *d <= aggro_sq
                && stats.weapons.iter().any(|weapon| {
                    weapon.targets.covers(Domain::Ground)
                        && *d >= weapon.minimum_range * weapon.minimum_range
                })
        })
        .min()
        .map(|(_, bid)| Target::Building(bid))
}

/// Keeps an advance moving while the primary weapon takes a shot that is
/// already available. There is deliberately no acquisition transition:
/// the order and path remain `Advance`, blocked and out-of-range targets
/// are ignored, and retaliation does not recognize this stance. Units
/// therefore never spend movement chasing a pot-shot.
#[allow(clippy::too_many_arguments)]
pub(super) fn advance(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    motion: &MotionSnapshot,
    id: UnitId,
    goal: TilePos,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
) {
    walk(state, id, goal, events);
    if !state
        .unit(id)
        .is_some_and(|u| matches!(u.order, Order::Advance { goal: current } if current == goal))
    {
        return;
    }

    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    let Some(weapon) = stats.weapons.first().copied() else {
        return;
    };
    if unit.cooldowns[0] > 0 {
        return;
    }
    let (pos, home, me, kind) = (unit.pos, unit.tile(), unit.player, unit.kind);
    let reach = weapon.range.floor().to_num::<i32>() + 1;
    let shot_open = |t: TilePos, full: bool| shot_crosses(state, t, full);

    let mut unit_target: Option<(chassis::fx::Fx, UnitId, Vec2Fx)> = None;
    for dy in -reach..=reach {
        for &(_, slot) in index.row_span(home.y + dy, home.x - reach, home.x + reach) {
            let target = &state.units[slot];
            let domain = target.kind.stats().domain;
            if target.hp == 0
                || !state.hostile(me, target.player)
                || !weapon.targets.covers(domain)
                || !state.can_see(me, target.tile())
            {
                continue;
            }
            let dist = pos.dist_sq(target.pos);
            let full = traces_terrain(&weapon, stats.domain, domain);
            if !within_weapon_reach(&weapon, dist)
                || !shot_open(target.tile(), full)
                || chassis::path::line_blocked(pos, target.pos, |t| shot_open(t, full))
            {
                continue;
            }
            if unit_target.is_none_or(|best| (dist, target.id) < (best.0, best.1)) {
                unit_target = Some((dist, target.id, target.pos));
            }
        }
    }

    let target = unit_target
        .map(|(_, uid, aim)| (Target::Unit(uid), aim))
        .or_else(|| {
            if !weapon.targets.covers(Domain::Ground) {
                return None;
            }
            state
                .buildings
                .iter()
                .filter(|b| b.hp > 0 && state.hostile(me, b.player))
                .filter(|b| b.tiles().any(|tile| state.can_see(me, tile)))
                .map(|b| {
                    let aim = b.closest_point_to(pos);
                    (pos.dist_sq(aim), b.id, aim)
                })
                .filter(|(dist, _, aim)| {
                    let full = traces_terrain(&weapon, stats.domain, Domain::Ground);
                    within_weapon_reach(&weapon, *dist)
                        && shot_open(TilePos::containing(*aim), full)
                        && !chassis::path::line_blocked(pos, *aim, |t| shot_open(t, full))
                })
                .min_by_key(|(dist, bid, _)| (*dist, *bid))
                .map(|(_, bid, aim)| (Target::Building(bid), aim))
        });
    let Some((target, aim)) = target else {
        return;
    };

    let projectile_aim = weapon.projectile.then(|| {
        projectile_aim(
            state,
            motion,
            ProjectileShooter {
                owner: me,
                domain: stats.domain,
            },
            pos,
            target,
            aim,
            &weapon,
        )
    });
    state.unit_mut(id).expect("caller checked").cooldowns[0] = weapon.cooldown_ticks;
    if weapon.projectile {
        let aim = projectile_aim.expect("projectile aim computed");
        let flight = launch_shell(state, launches, Target::Unit(id), me, pos, aim, &weapon);
        events.push(Event::ShellLaunched {
            shooter: Target::Unit(id),
            target,
            player: me,
            from: pos,
            to: aim,
            flight,
        });
    } else {
        buffer_shot(state, Target::Unit(id), me, target, aim, &weapon, hits);
        events.push(Event::AttackHit {
            attacker: id,
            attacker_kind: kind,
            weapon: 0,
            target,
            attacker_pos: pos,
            target_pos: aim,
        });
    }
}

/// The attack run: a turn-limited flier never takes a firing stance.
/// It keeps a live route on its victim, steers there on a bounded arc
/// (movement's steering integrator), and releases only when the bay is
/// cold, the victim is inside release range, AND the victim sits in the
/// forward cone — the geometry a straight pass produces and a tight
/// orbit cannot. Each release lays `salvo` bombs along the flight line
/// and rolls the bomber onto an egress leg past the target, so the wide
/// loop back IS the reload.
fn bomber_attack(
    state: &mut State,
    motion: &MotionSnapshot,
    id: UnitId,
    target: Target,
    resume: Option<TilePos>,
    events: &mut Vec<Event>,
    launches: &mut Vec<crate::state::Shell>,
) {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    let (pos, tile, me, kind, heading, cooldowns) = (
        unit.pos,
        unit.tile(),
        unit.player,
        unit.kind,
        unit.heading,
        unit.cooldowns,
    );

    // Vanished or uncoverable target: same hand-back as the ground arm.
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

    // Release gate: cold bay, in range, spotted, clear arc, and the
    // victim dead ahead. The sight and trace rules are the shared ones.
    let full = traces_terrain(weapon, stats.domain, victim_domain);
    let shot_open = |t: TilePos| shot_crosses(state, t, full);
    let seen = match target {
        Target::Unit(_) => state.can_see(me, target_tile),
        Target::Building(bid) => state
            .building(bid)
            .is_some_and(|b| b.tiles().any(|t| state.can_see(me, t))),
    };
    let to_target = aim_point - pos;
    let in_range = within_weapon_reach(weapon, pos.dist_sq(aim_point));
    let hv = chassis::compass::dir(heading);
    let ahead = hv.x * to_target.x + hv.y * to_target.y
        >= to_target.length() * crate::stats::BOMBER_CONE_DOT;
    if cooldowns[pi] == 0
        && in_range
        && seen
        && ahead
        && shot_open(TilePos::containing(aim_point))
        && !chassis::path::line_blocked(pos, aim_point, shot_open)
    {
        let center = projectile_aim(
            state,
            motion,
            ProjectileShooter {
                owner: me,
                domain: stats.domain,
            },
            pos,
            target,
            aim_point,
            weapon,
        );
        // The stick lays out along the flight line, centered on the aim
        // point; a single bomb is a one-entry stick.
        let salvo = weapon.salvo.max(1) as i32;
        for k in 0..salvo {
            let along = Fx::from_num(2 * k - (salvo - 1)) * chassis::fx::HALF;
            let impact = center + hv * (along * crate::stats::BOMB_SALVO_SPACING);
            let impact = clamp_to_envelope(state, impact);
            let flight = launch_shell(state, launches, Target::Unit(id), me, pos, impact, weapon);
            events.push(Event::ShellLaunched {
                shooter: Target::Unit(id),
                target,
                player: me,
                from: pos,
                to: impact,
                flight,
            });
        }
        // Egress: fly through and past the release point. The leg is a
        // plain path goal, so steering, arrival, and repath all reuse
        // the ordinary machinery; when it completes (or goes stale) the
        // chase below lines up the next run.
        let egress = egress_goal(state, pos, heading, stats.turn_acceptance());
        let unit = state.unit_mut(id).expect("caller checked");
        unit.cooldowns[pi] = weapon.cooldown_ticks;
        unit.path = egress.map(|goal| PathFollow {
            goal,
            waypoints: vec![goal],
            next: 0,
        });
        return;
    }

    // A hot bay keeps flying. Until the reload is half done the bomber
    // never turns back toward its victim: it finishes the egress leg,
    // then keeps extending the run straight ahead — a committed
    // airframe has no brakes, and hovering over the target to re-bomb
    // point-blank is exactly the stop-and-strafe this chassis forbids.
    if cooldowns[pi] > weapon.cooldown_ticks / 2 {
        if state.unit(id).expect("caller checked").path.is_none()
            && let Some(goal) = egress_goal(state, pos, heading, stats.turn_acceptance())
        {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                goal,
                waypoints: vec![goal],
                next: 0,
            });
        }
        return;
    }

    // Chase: keep a live route on the victim, repathing when it drifts.
    let stale = state
        .unit(id)
        .expect("caller checked")
        .path
        .as_ref()
        .is_none_or(|p| p.goal != target_tile && p.goal.chebyshev(target_tile) > 1);
    if stale {
        match route_for(state, kind, tile, target_tile) {
            Some(waypoints) => {
                let unit = state.unit_mut(id).expect("caller checked");
                unit.path = Some(PathFollow {
                    goal: target_tile,
                    waypoints,
                    next: 0,
                });
            }
            None => {
                let unit = state.unit_mut(id).expect("caller checked");
                let (player, upos) = (unit.player, unit.pos);
                unit.clear_program();
                events.push(Event::OrderStalled {
                    unit: id,
                    player,
                    pos: upos,
                    reason: StallReason::NoRoute,
                });
            }
        }
    }
}

/// Where a bomber rolls out after a release: straight ahead along its
/// heading when the sky is open, bending up to ninety degrees when a
/// wall or mesa closes the line. Every candidate must sit beyond the
/// aircraft's own acceptance ring — a goal inside it completes without
/// a single tick of flight, which is how a wall-facing bomber once
/// parked through its whole reload. `None` only in a closed pocket.
fn egress_goal(state: &State, pos: Vec2Fx, heading: u8, accept: Fx) -> Option<TilePos> {
    let accept_sq = accept * accept;
    for offset in [0u8, 32, 224, 64, 192] {
        let hv = chassis::compass::dir(heading.wrapping_add(offset));
        for reach in [5i64, 4, 3] {
            let probe = pos + hv * Fx::from_num(reach);
            let tile = TilePos::containing(probe);
            if tile.x >= 0
                && tile.y >= 0
                && tile.x < state.map().width()
                && tile.y < state.map().height()
                && state.passable_for(Domain::Air, tile)
                && tile.center().dist_sq(pos) > accept_sq
            {
                return Some(tile);
            }
        }
    }
    None
}

/// Clamps an impact point into the map envelope so an edge-of-map stick
/// never lands a shell outside the world.
fn clamp_to_envelope(state: &State, p: Vec2Fx) -> Vec2Fx {
    let max_x = Fx::from_num(state.map().width()) - chassis::fx::HALF;
    let max_y = Fx::from_num(state.map().height()) - chassis::fx::HALF;
    Vec2Fx::new(
        p.x.clamp(chassis::fx::HALF, max_x),
        p.y.clamp(chassis::fx::HALF, max_y),
    )
}

/// Chase-and-hit. Range is measured to the target's closest point and
/// shots are buffered. A vanished target — or one no carried weapon can
/// cover — hands control back to the remembered attack-move (or idle,
/// where auto-acquire finds the next fight).
#[allow(clippy::too_many_arguments)]
pub(super) fn attack(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    motion: &MotionSnapshot,
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
    if stats.turn_rate > 0 {
        bomber_attack(state, motion, id, target, resume, events, launches);
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
        && let Some(better @ Target::Unit(_)) = acquire_target(state, index, id)
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
            None => {
                // VICTORY (the target is gone) keeps the baseline
                // rhythm exactly: stand down where the fight ended
                // and let next tick's idle() pick the next one —
                // walking home mid-battle lost duels, and re-hunting
                // inside this arm perturbed scripted battles enough
                // to flip whole tier rungs onto the seat-parity coin.
                // A tethered victor stays STATIONED though, so its
                // next acquisition re-tethers on the spot: one
                // sacrificial unit must not buy the bait an
                // unleashed guard.
                if unit.leash.take().is_some() {
                    unit.settled = crate::stats::LEASH_STATION_TICKS;
                }
                unit.advance_queue();
            }
        }
        return;
    };
    let weapon = &stats.weapons[pi];

    // In range only counts with a clear line — and with eyes. Terrain
    // cover (rock) applies to direct ground-vs-ground fire only; shots
    // to or from the air and indirect shells arc past it — but nothing
    // arcs past a peak. Buildings never block fire: they block movement,
    // not bullets. The owner must currently *see* the victim's tile: a
    // gun that outranges its own vision fires on a spotter's sight
    // (scrap piles are low junk — fire passes over them). No shot →
    // keep approaching; the chase path already routes around what's in
    // the way.
    let shot_open = |t: TilePos, full: bool| shot_crosses(state, t, full);
    // Sight of any footprint tile serves for a building (matching attack
    // validation); a unit is seen at its own tile. The line trace runs
    // only once in range — it is not built for cross-map endpoints.
    let seen = match target {
        Target::Unit(_) => state.can_see(me, target_tile),
        Target::Building(bid) => state
            .building(bid)
            .is_some_and(|b| b.tiles().any(|t| state.can_see(me, t))),
    };
    let in_range = within_weapon_reach(weapon, pos.dist_sq(aim_point));
    let full = traces_terrain(weapon, stats.domain, victim_domain);
    // The line trace skips endpoint tiles by design — but a building's
    // closest-footprint aim point is an exact edge coordinate that can
    // floor into the NEIGHBORING tile. Flush against a peak, that
    // neighbor is the mountain itself, and an unchecked endpoint let
    // direct fire through it. The endpoint tile must be open for this
    // shot too (a unit's aim point is its own standable tile, and a
    // footprint tile stands on ground, which shot_open passes).
    let endpoint_open = shot_open(chassis::grid::TilePos::containing(aim_point), full);
    if in_range
        && seen
        && endpoint_open
        && !chassis::path::line_blocked(pos, aim_point, |t| shot_open(t, full))
    {
        let projectile_aim = weapon.projectile.then(|| {
            projectile_aim(
                state,
                motion,
                ProjectileShooter {
                    owner: me,
                    domain: stats.domain,
                },
                pos,
                target,
                aim_point,
                weapon,
            )
        });
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        // Blood drawn: reaching the firing stance refreshes the warm
        // window, buying followthrough past the radius — what lets a
        // guard finish the wounded runner rotating to the rear (the
        // scripted tiers' preservation trick, otherwise unpunishable)
        // without licensing a cross-map dive. A kiting harvester
        // outruns every line fighter, never grants a window, and its
        // chaser breaks at the radius line exactly.
        if resume.is_none()
            && let Some(leash) = unit.leash.as_mut()
        {
            leash.patience = crate::stats::LEASH_PATIENCE;
        }
        if cooldowns[pi] == 0 {
            unit.cooldowns[pi] = weapon.cooldown_ticks;
            if weapon.projectile {
                let aim_point = projectile_aim.expect("projectile aim computed");
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
                    target,
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
        fire_sidearms(state, index, id, pi, hits, events);
        return;
    }
    // Opportunist guns don't wait for the march to end.
    fire_sidearms(state, index, id, pi, hits, events);

    // The tether binds here — the chase, not the trigger and not the
    // firing stand above. It measures the GUARD's own distance from
    // its anchor, never the target's: the promise is "a stationed
    // machine travels at most the radius from its post (plus the
    // warm window)", and a target-based measure made the effective
    // pursuit depend on how far the chaser trails — weapon range and
    // speed matchup — turning a zero-window guard home while still
    // tiles inside its own zone. Inside the radius the guard hunts
    // freely (that ground is its zone); beyond it every chase tick
    // spends the warm-blood window, and an empty window sends the
    // guard walking home, leash kept so the homecoming arms the post
    // cooldown.
    if resume.is_none()
        && let Some(leash) = state.unit(id).expect("caller checked").leash
    {
        let radius_sq = crate::stats::LEASH_RADIUS * crate::stats::LEASH_RADIUS;
        if leash.anchor.center().dist_sq(pos) > radius_sq {
            if leash.patience == 0 {
                let unit = state.unit_mut(id).expect("caller checked");
                unit.order = Order::Move { goal: leash.anchor };
                unit.path = None;
                return;
            }
            state
                .unit_mut(id)
                .expect("caller checked")
                .leash
                .as_mut()
                .expect("just seen")
                .patience -= 1;
        }
    }

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
            let (anchor, size) = (b.anchor, b.stats().size);
            if approach_rect(state, id, anchor, size) {
                Ok(())
            } else {
                Err(StallReason::NoRoute)
            }
        }
    };
    if let Err(reason) = reached {
        let unit = state.unit_mut(id).expect("caller checked");
        // A tethered chase that cannot route breaks off home quietly:
        // abandoning its own acquisition is the guard's decision, not
        // a player order failing — no stall toast.
        if resume.is_none()
            && let Some(leash) = unit.leash
        {
            unit.order = Order::Move { goal: leash.anchor };
            unit.path = None;
            return;
        }
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

/// The sidearm's opportunist victim: the nearest hostile unit the weapon
/// can cover, in range, seen by the owner, and clear — chosen through the
/// spatial index's reach window. Selection is keyed `(distance, id)`, so
/// window visit order cannot change the answer; a differential test pins
/// this against the plain full-scan chain it replaced.
fn sidearm_victim(
    state: &State,
    index: &super::super::spatial::UnitIndex,
    shooter_pos: Vec2Fx,
    me: crate::ids::PlayerId,
    shooter_domain: Domain,
    weapon: &WeaponStats,
) -> Option<(chassis::fx::Fx, UnitId, Vec2Fx, Domain)> {
    let shot_open = |t: TilePos, full: bool| shot_crosses(state, t, full);
    // |Δpos| <= range on either axis puts the victim's tile within
    // floor(range) + 1 of the shooter's — the acquire_target bound.
    let home = TilePos::containing(shooter_pos);
    let reach = weapon.range.floor().to_num::<i32>() + 1;
    let mut victim: Option<(chassis::fx::Fx, UnitId, Vec2Fx, Domain)> = None;
    for dy in -reach..=reach {
        for &(_, slot) in index.row_span(home.y + dy, home.x - reach, home.x + reach) {
            let u = &state.units[slot];
            let domain = u.kind.stats().domain;
            if !state.hostile(me, u.player)
                || u.hp == 0
                || !weapon.targets.covers(domain)
                || !state.can_see(me, u.tile())
            {
                continue;
            }
            let d = shooter_pos.dist_sq(u.pos);
            if !within_weapon_reach(weapon, d) {
                continue;
            }
            // Pay for the line trace only when the candidate would win;
            // a blocked better candidate never shadows a clear worse one.
            if victim.is_some_and(|(best_d, best_id, ..)| (best_d, best_id) <= (d, u.id)) {
                continue;
            }
            let full = traces_terrain(weapon, shooter_domain, domain);
            if chassis::path::line_blocked(shooter_pos, u.pos, |t| shot_open(t, full)) {
                continue;
            }
            victim = Some((d, u.id, u.pos, domain));
        }
    }
    victim
}

/// Weapons other than the one engaging the ordered target pick their own
/// fights: the nearest hostile unit each can cover, in range, seen by the
/// owner, and clear — opportunist fire that never steers the chassis.
fn fire_sidearms(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
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
        let victim = sidearm_victim(state, index, pos, me, stats.domain, weapon);
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
        // A tethered homecoming answers fire: the walk home resumes
        // through the leash once the attacker falls, so no resume
        // goal is carried. A plain Move stays oblivious — it is the
        // player's recall verb, and auto-engaging on damage would
        // undo exactly what it was issued to do.
        Order::Move { .. } if unit.leash.is_some() => None,
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
    // Answering a hit is blood drawn: the warm window refreshes, so
    // the answer can reach an attacker just past the radius (the
    // repositioning-Bombard case) without opening a cross-map dive.
    // An answer that resumes a march keeps its player commitment
    // un-tethered; an answer with no resume is self-acquisition by
    // damage — an existing tether keeps its anchor (never re-anchored
    // forward), and a STATIONED machine gets a fresh one where it
    // stood. An unsettled machine (battle-cycling) answers unleashed,
    // like it always did.
    if resume.is_none() {
        let stationed = unit.settled >= crate::stats::LEASH_STATION_TICKS;
        unit.settled = 0;
        match unit.leash.as_mut() {
            Some(leash) => {
                leash.patience = crate::stats::LEASH_PATIENCE;
                leash.cooldown = 0;
            }
            None if stationed => {
                let anchor = unit.tile();
                unit.leash = Some(crate::state::Leash {
                    anchor,
                    patience: crate::stats::LEASH_PATIENCE,
                    cooldown: 0,
                });
            }
            None => {}
        }
    }
}

/// Whether a target is still on the field with hit points.
pub(super) fn target_standing(state: &State, target: Target) -> bool {
    match target {
        Target::Unit(u) => state.unit(u).is_some_and(|u| u.hp > 0),
        Target::Building(b) => state.building(b).is_some_and(|b| b.hp > 0),
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::spatial::UnitIndex;
    use super::*;
    use crate::command::{Command, PlayerCommand};
    use crate::scenario::{PlayerSpec, Scenario, UnitSpec};
    use crate::state::Faction;
    use crate::stats::UnitKind;

    /// The pre-index acquisition, kept verbatim as the reference: a full
    /// scan of the unit list. The indexed window prunes candidates and
    /// must never change the pick.
    fn linear_acquire(state: &State, id: UnitId) -> Option<Target> {
        let unit = state.unit(id).expect("caller checked");
        let stats = unit.kind.stats();
        if !stats.can_fight() {
            return None;
        }
        let (pos, me) = (unit.pos, unit.player);
        let acquisition_range = stats.aggro_range;
        let needs_shared_sight = unit.kind == UnitKind::Bombard;
        let aggro_sq = acquisition_range * acquisition_range;
        let unit_target = state
            .units
            .iter()
            .filter(|u| {
                state.hostile(me, u.player)
                    && u.hp > 0
                    && stats.can_target(u.kind.stats().domain)
                    && (!needs_shared_sight || state.can_see(me, u.tile()))
            })
            .map(|u| (pos.dist_sq(u.pos), u.id))
            .filter(|(d, uid)| {
                *d <= aggro_sq
                    && stats.weapons.iter().any(|weapon| {
                        weapon.targets.covers(
                            state
                                .unit(*uid)
                                .expect("candidate exists")
                                .kind
                                .stats()
                                .domain,
                        ) && *d >= weapon.minimum_range * weapon.minimum_range
                    })
            })
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
            .filter(|b| !needs_shared_sight || b.tiles().any(|tile| state.can_see(me, tile)))
            .map(|b| (pos.dist_sq(b.closest_point_to(pos)), b.id))
            .filter(|(d, _)| {
                *d <= aggro_sq
                    && stats.weapons.iter().any(|weapon| {
                        weapon.targets.covers(Domain::Ground)
                            && *d >= weapon.minimum_range * weapon.minimum_range
                    })
            })
            .min()
            .map(|(_, bid)| Target::Building(bid))
    }

    fn seat(name: &str, faction: Faction) -> PlayerSpec {
        PlayerSpec {
            name: name.into(),
            faction,
            team: None,
            scrap: 0,
            bot: false,
            bot_config: None,
        }
    }

    fn boundary_duel() -> State {
        Scenario {
            name: "boundary-duel".into(),
            seed: 1,
            map: vec![
                "............".into(),
                "............".into(),
                "............".into(),
                "1.........2.".into(),
                "............".into(),
                "............".into(),
                "............".into(),
                "............".into(),
            ],
            players: vec![
                seat("North", Faction::Ferrous),
                seat("South", Faction::Cupric),
            ],
            units: vec![
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Sentinel,
                    x: 5,
                    y: 1,
                },
                UnitSpec {
                    player: 1,
                    kind: UnitKind::Sentinel,
                    x: 6,
                    y: 1,
                },
            ],
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("boundary duel builds")
    }

    #[test]
    fn indexed_acquisition_keeps_targets_beyond_north_and_south_rows() {
        let mut state = boundary_duel();
        let height = state.map.height();
        let attacker = state.units[0].id;
        let victim = state.units[1].id;
        let mut index = UnitIndex::new();

        for (inside, outside) in [
            (TilePos::new(5, 0), TilePos::new(5, -1)),
            (TilePos::new(5, height - 1), TilePos::new(5, height)),
        ] {
            state.units[0].pos = inside.center();
            state.units[1].pos = outside.center();
            state
                .validate_invariants()
                .expect("the accepted coordinate envelope includes border rows");
            index.rebuild(&state.units);

            let indexed = acquire_target(&state, &index, attacker);
            assert_eq!(indexed, Some(Target::Unit(victim)));
            assert_eq!(indexed, linear_acquire(&state, attacker));
        }
    }

    /// Two mixed armies crossing an open arena, compared pick-for-pick
    /// against the linear scan every tick — range edges, weapon-mask
    /// misses (flak vs ground, ground-only vs flyers), dying candidates,
    /// and the building fallback near the far Foundry all churn as the
    /// fronts meet and melt.
    #[test]
    fn indexed_acquisition_matches_the_linear_scan() {
        let width = 31usize;
        let height = 19usize;
        let mut rows = vec![vec!['#'; width]; height];
        for row in rows.iter_mut().take(height - 1).skip(1) {
            for cell in row.iter_mut().take(width - 1).skip(1) {
                *cell = '.';
            }
        }
        rows[1][1] = '1';
        rows[height - 3][width - 3] = '2';
        let west: &[UnitKind] = &[
            UnitKind::Sentinel,
            UnitKind::Sentinel,
            UnitKind::Scuttler,
            UnitKind::Lancer,
            UnitKind::Flakhound,
            UnitKind::Buzzard,
            UnitKind::Talon,
            UnitKind::Harvester,
        ];
        let east: &[UnitKind] = &[
            UnitKind::Sentinel,
            UnitKind::Sentinel,
            UnitKind::Scuttler,
            UnitKind::Lancer,
            UnitKind::Stinger,
            UnitKind::Darter,
            UnitKind::Wisp,
            UnitKind::Harvester,
        ];
        let mut units = Vec::new();
        for (i, &kind) in west.iter().enumerate() {
            let (dx, dy) = ((i as i32) % 4, (i as i32) / 4);
            units.push(UnitSpec {
                player: 0,
                kind,
                x: 3 + dx * 2,
                y: 4 + dy * 2,
            });
        }
        for (i, &kind) in east.iter().enumerate() {
            let (dx, dy) = ((i as i32) % 4, (i as i32) / 4);
            units.push(UnitSpec {
                player: 1,
                kind,
                x: 27 - dx * 2,
                y: 14 - dy * 2,
            });
        }
        let scenario = Scenario {
            name: "acquisition-differential".into(),
            seed: 7,
            map: rows.into_iter().map(|r| r.into_iter().collect()).collect(),
            players: vec![
                seat("West", Faction::Ferrous),
                seat("East", Faction::Cupric),
            ],
            units,
            buildings: Vec::new(),
            meta: None,
        };
        let mut state = scenario.build().expect("arena builds");
        let march = |state: &State, player: u8, goal: TilePos| -> PlayerCommand {
            PlayerCommand {
                player: PlayerId(player),
                command: Command::AttackMove {
                    units: state
                        .units
                        .iter()
                        .filter(|u| u.player == PlayerId(player))
                        .map(|u| u.id)
                        .collect(),
                    goal,
                    queue: false,
                },
            }
        };
        let opening = [
            march(&state, 0, TilePos::new(27, 15)),
            march(&state, 1, TilePos::new(3, 3)),
        ];
        state.tick(&opening);

        let mut index = UnitIndex::new();
        let mut picks = 0usize;
        for _ in 0..400 {
            state.tick(&[]);
            index.rebuild(&state.units);
            for unit in &state.units {
                if unit.hp == 0 {
                    continue;
                }
                let indexed = acquire_target(&state, &index, unit.id);
                assert_eq!(
                    indexed,
                    linear_acquire(&state, unit.id),
                    "unit {:?} at tick {}",
                    unit.id,
                    state.tick
                );
                picks += usize::from(indexed.is_some());
            }
        }
        assert!(picks > 100, "the armies never met ({picks} picks)");
    }

    /// The linear chain `sidearm_victim`'s window replaced, kept verbatim
    /// as the reference the differential below compares against.
    fn linear_sidearm_victim(
        state: &State,
        shooter_pos: Vec2Fx,
        me: PlayerId,
        shooter_domain: Domain,
        weapon: &WeaponStats,
    ) -> Option<(Fx, UnitId, Vec2Fx, Domain)> {
        let shot_open = |t: TilePos, full: bool| {
            state.map.tile(t).is_some_and(|tile| {
                !tile.terrain.blocks_all_fire() && (!full || !tile.terrain.blocks_direct_fire())
            })
        };
        state
            .units
            .iter()
            .filter(|u| {
                state.hostile(me, u.player)
                    && u.hp > 0
                    && weapon.targets.covers(u.kind.stats().domain)
            })
            .filter(|u| state.can_see(me, u.tile()))
            .map(|u| {
                (
                    shooter_pos.dist_sq(u.pos),
                    u.id,
                    u.pos,
                    u.kind.stats().domain,
                )
            })
            .filter(|(d, ..)| within_weapon_reach(weapon, *d))
            .filter(|(_, _, upos, dom)| {
                let full = traces_terrain(weapon, shooter_domain, *dom);
                !chassis::path::line_blocked(shooter_pos, *upos, |t| shot_open(t, full))
            })
            .min_by_key(|&(d, uid, _, _)| (d, uid))
    }

    /// The same mixed-armies churn as the acquisition differential, but
    /// comparing every multi-weapon unit's sidearm pick — window edges,
    /// fog, air-vs-ground weapon masks, and dying candidates included.
    #[test]
    fn windowed_sidearm_victim_matches_the_linear_scan() {
        let width = 31usize;
        let height = 19usize;
        let mut rows = vec![vec!['#'; width]; height];
        for row in rows.iter_mut().take(height - 1).skip(1) {
            for cell in row.iter_mut().take(width - 1).skip(1) {
                *cell = '.';
            }
        }
        rows[1][1] = '1';
        rows[height - 3][width - 3] = '2';
        let west: &[UnitKind] = &[
            UnitKind::Sentinel,
            UnitKind::Sentinel,
            UnitKind::Sentinel,
            UnitKind::Buzzard,
            UnitKind::Talon,
            UnitKind::Flakhound,
        ];
        let east: &[UnitKind] = &[
            UnitKind::Sentinel,
            UnitKind::Sentinel,
            UnitKind::Wisp,
            UnitKind::Wisp,
            UnitKind::Darter,
            UnitKind::Scuttler,
        ];
        let mut units = Vec::new();
        for (i, &kind) in west.iter().enumerate() {
            let (dx, dy) = ((i as i32) % 3, (i as i32) / 3);
            units.push(UnitSpec {
                player: 0,
                kind,
                x: 3 + dx * 2,
                y: 4 + dy * 2,
            });
        }
        for (i, &kind) in east.iter().enumerate() {
            let (dx, dy) = ((i as i32) % 3, (i as i32) / 3);
            units.push(UnitSpec {
                player: 1,
                kind,
                x: 27 - dx * 2,
                y: 14 - dy * 2,
            });
        }
        let scenario = Scenario {
            name: "sidearm-differential".into(),
            seed: 11,
            map: rows.into_iter().map(|r| r.into_iter().collect()).collect(),
            players: vec![
                seat("West", Faction::Ferrous),
                seat("East", Faction::Cupric),
            ],
            units,
            buildings: Vec::new(),
            meta: None,
        };
        let mut state = scenario.build().expect("arena builds");
        let march = |state: &State, player: u8, goal: TilePos| -> PlayerCommand {
            PlayerCommand {
                player: PlayerId(player),
                command: Command::AttackMove {
                    units: state
                        .units
                        .iter()
                        .filter(|u| u.player == PlayerId(player))
                        .map(|u| u.id)
                        .collect(),
                    goal,
                    queue: false,
                },
            }
        };
        let opening = [
            march(&state, 0, TilePos::new(27, 15)),
            march(&state, 1, TilePos::new(3, 3)),
        ];
        state.tick(&opening);

        let mut index = UnitIndex::new();
        let mut picks = 0usize;
        for _ in 0..400 {
            state.tick(&[]);
            index.rebuild(&state.units);
            for unit in &state.units {
                let stats = unit.kind.stats();
                if unit.hp == 0 || stats.weapons.len() < 2 {
                    continue;
                }
                for weapon in stats.weapons {
                    let windowed =
                        sidearm_victim(&state, &index, unit.pos, unit.player, stats.domain, weapon);
                    assert_eq!(
                        windowed,
                        linear_sidearm_victim(&state, unit.pos, unit.player, stats.domain, weapon),
                        "unit {:?} at tick {}",
                        unit.id,
                        state.tick
                    );
                    picks += usize::from(windowed.is_some());
                }
            }
        }
        assert!(picks > 50, "no sidearm ever found a victim ({picks} picks)");
    }

    #[test]
    fn motion_snapshot_uses_only_the_current_steering_line() {
        let aim_with_later_turn = |turn: TilePos| {
            let mut state = boundary_duel();
            state.units[0].kind = UnitKind::Bombard;
            state.units[0].pos = TilePos::new(2, 1).center();
            state.units[1].kind = UnitKind::Scuttler;
            state.units[1].pos = TilePos::new(7, 1).center();
            let target = state.units[1].id;
            state.units[1].path = Some(PathFollow {
                goal: turn,
                waypoints: vec![TilePos::new(8, 1), turn],
                next: 0,
            });
            let motion = MotionSnapshot::capture(&state);
            let shooter = state.units[0].pos;
            let current = state.units[1].pos;
            projectile_aim(
                &state,
                &motion,
                ProjectileShooter {
                    owner: PlayerId(0),
                    domain: Domain::Ground,
                },
                shooter,
                Target::Unit(target),
                current,
                &UnitKind::Bombard.stats().weapons[0],
            )
        };

        assert_eq!(
            aim_with_later_turn(TilePos::new(8, 6)),
            aim_with_later_turn(TilePos::new(8, -4)),
            "future A* turns are private intent, not observable velocity"
        );
    }

    #[test]
    fn predictive_splash_aim_uses_the_near_edge_of_the_footprint() {
        let current = Vec2Fx::new(Fx::from_num(4), Fx::from_num(3));
        let predicted = Vec2Fx::new(Fx::from_num(9), Fx::from_num(3));
        let radius = Fx::lit("1.4");

        let aim = aim_at_near_splash_edge(current, predicted, radius);

        let expected = Vec2Fx::new(Fx::lit("7.6"), Fx::from_num(3));
        let tolerance = Fx::DELTA * Fx::from_num(8);
        assert!(
            aim.dist(expected) <= tolerance,
            "fixed-point direction scaling stays within eight ulps of the near edge"
        );
        assert!(
            aim.dist(predicted) <= radius && aim.dist(predicted) >= radius - tolerance,
            "a straight mover sits exactly on the predicted blast edge: distance={}, radius={}, tolerance={}",
            aim.dist(predicted),
            radius,
            tolerance
        );
    }
}
