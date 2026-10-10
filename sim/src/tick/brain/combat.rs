//! The fighting half of the brain: target acquisition, the weapons
//! matrix (direct, indirect, projectile, sidearms), turret fire, and
//! the retaliation contract. Every shot buffers into the tick's volley;
//! nothing here applies damage directly.

mod blind;
use super::contact;
pub(super) use blind::{ShotBuffers, attack_known, automatic_radar, normalize_order};

use super::super::flight;
use super::super::landing;
use super::super::route_for;
use super::PendingHit;
use super::locomotion::{approach_rect, walk};
use crate::event::{Event, StallReason, UnitLaunchPose};
use crate::ids::{PlayerId, Target, UnitId};
use crate::state::{Order, PathFollow, State};
use crate::stats::{Domain, WeaponStats};
use chassis::fx::{Fx, Vec2Fx};
use chassis::grid::TilePos;

/// The movement domain a target occupies (buildings sit on the ground).
fn target_domain(state: &State, target: Target) -> Domain {
    match target {
        Target::Unit(uid) => state
            .unit(uid)
            .map_or(Domain::Ground, crate::state::Unit::domain),
        Target::Building(_) => Domain::Ground,
    }
}

/// Whether full terrain cover applies to a shot: only direct fire between
/// two ground parties traces rock; shots involving aircraft and indirect
/// shells pass over it. Buildings never block fire. Peaks block every shot
/// regardless (see the `shot_open` closures).
fn traces_terrain(weapon: &WeaponStats, shooter: Domain, victim: Domain) -> bool {
    !weapon.indirect && shooter == Domain::Ground && victim == Domain::Ground
}

/// Whether a shot's line may pass over `t`. Peaks wall every pairing and
/// every arc; full-cover tracing (direct ground-vs-ground) additionally
/// respects terrain that grants cover. Pits block neither.
fn shot_crosses(state: &State, t: TilePos, full: bool) -> bool {
    state.map.tile(t).is_some_and(|tile| {
        !tile.terrain.blocks_all_fire() && (!full || !tile.terrain.blocks_direct_fire())
    })
}

fn within_weapon_reach(weapon: &WeaponStats, distance_sq: chassis::fx::Fx) -> bool {
    distance_sq <= weapon.range * weapon.range
        && distance_sq >= weapon.minimum_range * weapon.minimum_range
}

fn within_unit_weapon_reach(
    state: &State,
    kind: crate::UnitKind,
    target: Target,
    weapon: &WeaponStats,
    distance_sq: Fx,
) -> bool {
    let range = if let Some(reach) = kind.stats().contact_reach {
        let target_radius = match target {
            Target::Unit(id) => state.unit(id).map_or(Fx::ZERO, |u| u.kind.stats().radius),
            Target::Building(_) => Fx::ZERO,
        };
        weapon
            .range
            .min(kind.stats().radius + target_radius + reach)
    } else {
        weapon.range
    };
    distance_sq <= range * range && distance_sq >= weapon.minimum_range * weapon.minimum_range
}

/// Flight length in ticks (at least one) for a shell from `from` to `aim`.
/// A projectile flies to one fixed fire-time aim point and resolves against
/// whatever stands there then; predictive artillery chooses that point
/// before launch, and the flight is never guided.
fn shell_flight(from: Vec2Fx, aim: Vec2Fx, speed: Fx) -> u64 {
    (from.dist(aim) / speed).ceil().to_num::<u64>().max(1)
}

/// One tick-boundary sample of the motion a visible body is already showing.
///
/// The sample is captured before any unit brain runs. That keeps artillery
/// independent of the alternating brain order. Ground samples use retained
/// motor speed and heading, including pathless coasting; air samples keep the
/// current steering line. Neither reads an opponent's later A* turns.
pub(super) struct MotionSnapshot {
    velocities: Vec<(UnitId, Vec2Fx)>,
}

impl MotionSnapshot {
    pub(super) fn capture(state: &State) -> Self {
        let velocities = state
            .units
            .iter()
            .filter_map(|unit| {
                if unit.kind.stats().domain == Domain::Ground {
                    return (unit.drive_speed > Fx::ZERO).then_some((
                        unit.id,
                        chassis::compass::dir(unit.heading) * unit.drive_speed,
                    ));
                }
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
        let slot = self
            .velocities
            .binary_search_by_key(&target, |(id, _)| *id)
            .ok()?;
        Some(
            current
                + self.velocities[slot].1 * Fx::from_num(ticks.min(crate::stats::MAX_LEAD_TICKS)),
        )
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
                && weapon.targets.covers(unit.domain())
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

/// Leads a moving unit using its tick-boundary motion sample for the shell's
/// estimated flight. For splash shells, the aim backs toward the current
/// position by one blast radius: a straight commitment remains just inside
/// the footprint, while stopping or turning gets a margin for error. If the
/// current footprint already contains a second visible eligible hostile, the
/// gun keeps that known cluster instead of leading away from it. Later path
/// turns remain private, and the target remains free to change course after
/// launch; shells are still unguided. Buildings keep their closest-footprint
/// aim unchanged.
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
    let speed = weapon
        .projectile
        .expect("only projectile weapons lead their aim")
        .speed;
    let mut flight = shell_flight(from, current, speed);
    let mut aim = current;
    let mut unhedged_aim = current;
    // Ground artillery can only lead ground units, all slower than a shell.
    // Eight fixed iterations settle even the fastest ground body; the
    // projection itself stays bounded if a balance change breaks that.
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
        let next_flight = shell_flight(from, aim, speed);
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
    let projectile = weapon.projectile.expect("only projectile weapons launch");
    let flight = shell_flight(from, aim, projectile.speed);
    launches.push(crate::state::Shell {
        kind: projectile.payload,
        shooter: attacker,
        player: attacker_owner,
        launch: from,
        impact: aim,
        launched_at: state.tick,
        arrival: state.tick + flight,
        damage: weapon.damage,
        targets: weapon.targets,
        splash: weapon.splash,
    });
    flight
}

/// Arrived shells join this tick's volley, computed against the same
/// start-of-tick world every buffered shot uses. The direct hit lands on
/// the hostile building whose footprint touches the impact point; units
/// take splash only. No fire gate here: the gate cleared at launch.
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
        // containment: a shell aimed at a building lands on the footprint's
        // closest edge point, whose exact coordinate can floor into the
        // neighboring tile. A scaffold above a buried charge takes the
        // direct hit; the charge still takes splash. Other ties go to the
        // lowest id.
        let direct = state
            .buildings
            .iter()
            .filter(|b| {
                b.hp > 0
                    && !b.provisional()
                    && shell.targets.ground
                    && state.hostile(shell.player, b.player)
                    && b.closest_point_to(shell.impact).dist_sq(shell.impact)
                        <= const { chassis::fx::Fx::lit("0.0001") }
            })
            .min_by_key(|b| (b.kind.is_stealthy() && b.built(), b.id));
        if let Some(b) = direct {
            hits.push(PendingHit::along(
                state,
                shell.shooter,
                Target::Building(b.id),
                shell.damage,
                shell.launch,
                shell.impact,
            ));
        }
        let Some(radius) = shell.splash else { continue };
        let radius_sq = radius * radius;
        // Buried charges take splash, as in `buffer_shot`.
        if shell.targets.ground {
            for b in &state.buildings {
                if b.hp == 0
                    || b.provisional()
                    || !b.kind.is_stealthy()
                    || !state.hostile(shell.player, b.player)
                    || direct.is_some_and(|d| d.id == b.id)
                    || b.center().dist_sq(shell.impact) > radius_sq
                {
                    continue;
                }
                hits.push(PendingHit::along(
                    state,
                    shell.shooter,
                    Target::Building(b.id),
                    shell.damage,
                    shell.launch,
                    shell.impact,
                ));
            }
        }
        for u in &state.units {
            if u.hp == 0
                || !state.hostile(shell.player, u.player)
                || !shell.targets.covers(u.domain())
                || u.pos.dist_sq(shell.impact) > radius_sq
            {
                continue;
            }
            hits.push(PendingHit::along(
                state,
                shell.shooter,
                Target::Unit(u.id),
                shell.damage,
                shell.launch,
                shell.impact,
            ));
        }
    }
}

/// Buffers a shot: the direct hit, plus, for splash weapons, one hit on
/// every other hostile unit inside the radius that the weapon can cover and
/// on hostile buried charges. Other buildings only take the direct hit.
///
/// Splash skips the owner-sight fire gate the aimed paths enforce: the gate
/// governs choosing a victim, and splash hits whatever stands in the blast.
/// No information leaks: a bystander's owner learns only that its own body
/// took damage, and retaliation stays gated on the victim seeing the
/// shooter.
fn buffer_shot(
    state: &State,
    attacker: Target,
    victim: Target,
    from: Vec2Fx,
    aim: Vec2Fx,
    weapon: &WeaponStats,
    hits: &mut Vec<PendingHit>,
) {
    let attacker_owner = match attacker {
        Target::Unit(id) => state.unit(id).map(|unit| unit.player),
        Target::Building(id) => state.building(id).map(|building| building.player),
    }
    .expect("a buffered shot has a live source");
    hits.push(PendingHit::along(
        state,
        attacker,
        victim,
        weapon.damage,
        from,
        aim,
    ));
    let Some(radius) = weapon.splash else { return };
    let radius_sq = radius * radius;
    for u in &state.units {
        if u.hp == 0
            || !state.hostile(attacker_owner, u.player)
            || Target::Unit(u.id) == victim
            || !weapon.targets.covers(u.domain())
            || u.pos.dist_sq(aim) > radius_sq
        {
            continue;
        }
        hits.push(PendingHit::along(
            state,
            attacker,
            Target::Unit(u.id),
            weapon.damage,
            from,
            aim,
        ));
    }
    // The one building exception to direct-hits-only: a buried charge takes
    // splash, detected or not, so saturation fire can clear a minefield.
    if weapon.targets.ground {
        for b in &state.buildings {
            if b.hp == 0
                || b.provisional()
                || !b.kind.is_stealthy()
                || !state.hostile(attacker_owner, b.player)
                || Target::Building(b.id) == victim
                || b.center().dist_sq(aim) > radius_sq
            {
                continue;
            }
            hits.push(PendingHit::along(
                state,
                attacker,
                Target::Building(b.id),
                weapon.damage,
                from,
                aim,
            ));
        }
    }
}

/// Built turrets pick their own fights: the nearest enemy unit in range with
/// a clear line; out-of-line targets are ignored until they move.
/// Ground-capable defenses with no eligible unit fall back to currently
/// apparent hostile buildings. Target choice is re-evaluated every shot, in
/// building-id order.
pub(super) fn turret_fire(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
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
        if !b.built() || b.hp == 0 {
            continue;
        }
        let (me, center, cooling, kind, tier, focus) = (
            b.player,
            b.center(),
            b.cooldown > 0,
            b.kind,
            b.tier,
            b.focus,
        );
        let focused = focus
            .and_then(|target| state.attack_view(me, target))
            .filter(|view| view.domain.is_none_or(|domain| atk.targets.covers(domain)));
        if focus.is_some() && focused.is_none() {
            state.building_mut(id).expect("just seen").focus = None;
        }
        if cooling {
            let b = state.building_mut(id).expect("just seen");
            b.cooldown -= 1;
            if b.cooldown > 0 {
                continue;
            }
        }
        let shot_open = |t: TilePos, full: bool| shot_crosses(state, t, full);
        let focused =
            focused.filter(|view| blind::solution(state, center, Domain::Ground, *view, atk));
        if let Some(view) = focused.filter(|view| view.entity.is_none()) {
            blind::fire_building(state, id, view, events, hits, launches);
            continue;
        }
        let focused_victim =
            focused.and_then(|view| view.entity.map(|target| (target, view.aim_from(center))));
        // A unit in range stands on a tile within `reach` of the turret's;
        // a building in range covers one, or the next where its closest
        // point sits on an edge. Positions hold still through the brain
        // phase, so the phase's index and survey still describe them.
        let home = TilePos::containing(center);
        let reach = atk.range.floor().to_num::<i32>() + 1;
        let hostiles = index.hostiles_near(state.player(me).team, home, reach + 1);
        let unit_victim = (focused_victim.is_none() && hostiles.bodies)
            .then(|| {
                (home.y - reach..=home.y + reach)
                    .flat_map(|y| index.row_span(y, home.x - reach, home.x + reach))
                    .map(|&(_, slot)| &state.units[slot])
                    .filter(|u| {
                        state.hostile(me, u.player) && u.hp > 0 && atk.targets.covers(u.domain())
                    })
                    .filter(|u| state.can_see(me, u.tile()))
                    .map(|u| (center.dist_sq(u.pos), u.id, u.pos, u.domain()))
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
            && atk.targets.covers(Domain::Ground))
        .then(|| {
            let candidates = if hostiles.buildings {
                state.buildings()
            } else {
                index.buildings_since_survey(state.buildings())
            };
            candidates
                .iter()
                .filter(|target| {
                    state
                        .visible_hostile_target_domain(me, Target::Building(target.id))
                        .is_some()
                })
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
            if let Some(view) = blind::radar_in_range(state, me, center, Domain::Ground, atk) {
                blind::fire_building(state, id, view, events, hits, launches);
            }
            continue;
        };
        let aim = if atk.projectile.is_some() {
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
        if atk.projectile.is_some() {
            let flight = launch_shell(state, launches, Target::Building(id), me, center, aim, atk);
            events.push(Event::ShellLaunched {
                shooter: Target::Building(id),
                unit_pose: None,
                target: Some(victim),
                player: me,
                from: center,
                to: aim,
                flight,
            });
        } else {
            buffer_shot(state, Target::Building(id), victim, center, aim, atk, hits);
            events.push(Event::TurretFired {
                turret: id,
                kind,
                tier,
                target: Some(victim),
                turret_pos: center,
                target_pos: aim,
            });
        }
    }
}

/// Firing positions for a chaser around an unstandable victim tile,
/// ring-scanned outward, keeping only tiles the chaser can stand on and
/// that lie within the weapon's Euclidean reach (ring corners sit √2
/// further out than their Chebyshev radius). Within a ring the scan is
/// row-major, which a half-turn does not preserve, so a caller that picks
/// one must rank them. Empty when the victim sits deeper in blocked ground
/// than any weapon reaches.
fn chase_stand_ins(
    state: &State,
    domain: Domain,
    around: TilePos,
    range: chassis::fx::Fx,
) -> Vec<TilePos> {
    let aim = around.center();
    let mut out = Vec::new();
    for r in 1..=crate::stats::CHASE_STAND_RADIUS {
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

/// Routes a mobile artillery unit out of its own dead zone to the nearest
/// reachable tile from which its primary weapon has a clear, legal shot.
/// Keeping a still-valid goal avoids rebuilding the route every brain tick.
fn retreat_to_firing_stand(
    state: &mut State,
    id: UnitId,
    target: Target,
    victim_domain: Domain,
    weapon: &WeaponStats,
) -> bool {
    let domain = state.unit(id).expect("caller checked").kind.stats().domain;
    let full = traces_terrain(weapon, domain, victim_domain);
    let legal = |state: &State, tile: TilePos| {
        if !state.passable_for(domain, tile) {
            return false;
        }
        let position = tile.center();
        let aim = match target {
            Target::Unit(target) => state
                .unit(target)
                .filter(|unit| unit.hp > 0)
                .map(|unit| unit.pos),
            Target::Building(target) => state
                .building(target)
                .filter(|building| building.hp > 0)
                .map(|building| building.closest_point_to(position)),
        };
        let Some(aim) = aim else { return false };
        let distance_sq = position.dist_sq(aim);
        if !within_weapon_reach(weapon, distance_sq) {
            return false;
        }
        let shot_open = |tile| shot_crosses(state, tile, full);
        shot_open(TilePos::containing(aim))
            && !chassis::path::line_blocked(position, aim, shot_open)
    };

    let footprint = match target {
        Target::Unit(target) => (state.unit(target).expect("resolved target").tile(), (1, 1)),
        Target::Building(target) => {
            let building = state.building(target).expect("resolved target");
            (building.anchor, building.kind.size())
        }
    };
    let reach = weapon.range.ceil().to_num::<i32>();
    route_to_stand(state, id, footprint, reach, legal)
}

/// Routes a unit to the nearest tile within `reach` tiles of a footprint
/// that `legal` accepts and a route reaches, keeping a path whose goal is
/// still legal. Equally near stands rank in the unit's approach frame around
/// the footprint, so mirrored units take mirrored stands. Clears the path
/// and returns `false` when no stand routes.
fn route_to_stand(
    state: &mut State,
    id: UnitId,
    (anchor, size): (TilePos, (i32, i32)),
    reach: i32,
    legal: impl Fn(&State, TilePos) -> bool,
) -> bool {
    let unit = state.unit(id).expect("caller checked");
    if unit
        .path
        .as_ref()
        .is_some_and(|path| legal(state, path.goal))
    {
        return true;
    }
    let (from, from_tile, kind, player) = (unit.pos, unit.tile(), unit.kind, unit.player);
    let frame = crate::tick::rect_approach_origin(state, player, from_tile, anchor, size);
    let (width, height) = size;
    let mut candidates = Vec::new();
    for y in
        (anchor.y - reach).max(0)..=(anchor.y + height - 1 + reach).min(state.map().height() - 1)
    {
        for x in
            (anchor.x - reach).max(0)..=(anchor.x + width - 1 + reach).min(state.map().width() - 1)
        {
            let tile = TilePos::new(x, y);
            if legal(state, tile) {
                candidates.push(tile);
            }
        }
    }
    candidates.sort_unstable_by_key(|&tile| {
        (
            from.dist_sq(tile.center()),
            crate::geometry::rect_approach_key_from(from_tile, frame, anchor, size, tile),
        )
    });
    let path = candidates.into_iter().find_map(|goal| {
        route_for(state, kind, from_tile, goal).map(|waypoints| PathFollow {
            final_point: None,
            goal,
            waypoints,
            next: 0,
        })
    });
    let found = path.is_some();
    state.unit_mut(id).expect("caller checked").path = path;
    found
}

/// The nearest enemy this unit's weapons can cover, in its autonomous
/// acquisition range: units before buildings, ties to the lowest id.
/// `None` for unarmed units, empty horizons, and everything outside the
/// weapon masks.
///
/// Unit candidates come from the phase's spatial `index`: everything within
/// that range of `pos` stands within `floor(range) + 1` tiles Chebyshev of
/// its tile (the extra tile covers both bodies' sub-tile offsets), and the
/// pick is a `min` over `(dist_sq, id)`, so window visit order cannot move
/// it.
pub(super) fn acquire_target(
    state: &State,
    index: &super::super::spatial::UnitIndex,
    id: UnitId,
) -> Option<Target> {
    let pos = state.unit(id).expect("caller checked").pos;
    acquire_target_from(state, index, id, pos)
}

/// The same pick judged as if the unit stood at `pos`: what a landing
/// aircraft would find in reach once parked on its tile, not what it
/// happens to pass on the way there.
pub(super) fn acquire_target_from(
    state: &State,
    index: &super::super::spatial::UnitIndex,
    id: UnitId,
    pos: chassis::fx::Vec2Fx,
) -> Option<Target> {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if !stats.can_fight() {
        return None;
    }
    let me = unit.player;
    let acquisition_range = stats.aggro_range;
    // Acquisition normally rides on the unit's own eyes because its vision
    // covers its aggro range. A long gun whose acquisition horizon outruns
    // those eyes, or a pick judged from somewhere else, must instead lean on
    // current team sight or it becomes an oracle.
    let needs_sight = acquisition_range > Fx::from_num(stats.vision) || pos != unit.pos;
    let aggro_sq = acquisition_range * acquisition_range;

    let home = TilePos::containing(pos);
    let reach = acquisition_range.floor().to_num::<i32>() + 1;
    let team = state.player(me).team;
    let mut unit_target: Option<(chassis::fx::Fx, UnitId)> = None;
    // A building in acquisition range covers a tile within `reach` of the
    // unit's tile, or the next one where its closest point sits on an edge.
    let hostiles = index.hostiles_near(team, home, reach + 1);
    // An army at rest far from any enemy is the common case: the survey
    // proves the window holds no hostile body before any row is walked.
    if hostiles.bodies {
        for dy in -reach..=reach {
            for &(_, slot) in index.row_span(home.y + dy, home.x - reach, home.x + reach) {
                let u = &state.units[slot];
                if !state.hostile(me, u.player)
                    || u.hp == 0
                    || !stats.can_target(u.domain())
                    || (needs_sight && !state.can_see(me, u.tile()))
                {
                    continue;
                }
                let d = pos.dist_sq(u.pos);
                let outside_dead_zone = stats.weapons.iter().any(|weapon| {
                    weapon.targets.covers(u.domain())
                        && d >= weapon.minimum_range * weapon.minimum_range
                });
                if !(d <= aggro_sq
                    && outside_dead_zone
                    && unit_target.is_none_or(|best| (d, u.id) < best))
                {
                    continue;
                }
                // A victim on ground the chaser cannot stand on needs a
                // firing position to exist, the same test the chase applies
                // one tick later. Without it the unit would acquire an order
                // it can only stall, clear it, and re-acquire every tick.
                let victim_tile = u.tile();
                if !state.passable_for(stats.domain, victim_tile) {
                    let range = stats
                        .weapons
                        .iter()
                        .find(|w| w.targets.covers(u.domain()))
                        .map_or(chassis::fx::Fx::ZERO, |w| w.range);
                    if chase_stand_ins(state, stats.domain, victim_tile, range).is_empty() {
                        continue;
                    }
                }
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
    let candidates = if hostiles.buildings {
        state.buildings()
    } else {
        index.buildings_since_survey(state.buildings())
    };
    candidates
        .iter()
        .filter(|b| state.hostile(me, b.player) && b.hp > 0)
        .map(|b| (pos.dist_sq(b.closest_point_to(pos)), b))
        // Range gates run before the knowledge gates: apparency for a
        // buried charge scans every friendly scout and radar mast. The
        // filters commute, so ordering only saves work.
        .filter(|(d, _)| {
            *d <= aggro_sq
                && stats.weapons.iter().any(|weapon| {
                    weapon.targets.covers(Domain::Ground)
                        && *d >= weapon.minimum_range * weapon.minimum_range
                })
        })
        // An undetected buried charge must never be auto-attacked: that
        // would leak its existence to the attacker's owner.
        .filter(|(_, b)| state.building_apparent(me, b))
        .filter(|(_, b)| !needs_sight || b.tiles().any(|tile| state.can_see(me, tile)))
        .map(|(d, b)| (d, b.id))
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
    reach: &mut super::super::reach::Reach,
    motion: &MotionSnapshot,
    id: UnitId,
    goal: crate::state::Goal,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
) {
    if super::locomotion::land_at_destination(state, index, reach, id, events) {
        return;
    }
    walk(state, index, reach, id, events);
    if !state.unit(id).is_some_and(
        |u| matches!(u.order, Order::Advance { goal: current } if current.tile() == goal.tile()),
    ) {
        return;
    }

    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    let Some(weapon) = stats.weapons.first().copied() else {
        return;
    };
    let cooldown = unit.cooldowns[0];
    // A braced gun fires only once its spades are down, never on the move.
    if stats.brace.is_some() {
        return;
    }
    if cooldown > 0 && stats.turret_turn_rate == 0 {
        return;
    }
    let (pos, home, me, kind) = (unit.pos, unit.tile(), unit.player, unit.kind);
    let reach = weapon.range.floor().to_num::<i32>() + 1;
    let shot_open = |t: TilePos, full: bool| shot_crosses(state, t, full);

    let mut unit_target: Option<(chassis::fx::Fx, UnitId, Vec2Fx)> = None;
    for dy in -reach..=reach {
        for &(_, slot) in index.row_span(home.y + dy, home.x - reach, home.x + reach) {
            let target = &state.units[slot];
            let domain = target.domain();
            if target.hp == 0
                || !state.hostile(me, target.player)
                || !weapon.targets.covers(domain)
                || !state.can_see(me, target.tile())
                || !super::super::movement::advancing_weapon_aligned(unit, target.pos - pos)
            {
                continue;
            }
            let dist = pos.dist_sq(target.pos);
            let full = traces_terrain(&weapon, stats.domain, domain);
            if !within_unit_weapon_reach(state, kind, Target::Unit(target.id), &weapon, dist)
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
                .filter(|b| state.building_apparent(me, b))
                .map(|b| {
                    let aim = b.closest_point_to(pos);
                    (pos.dist_sq(aim), b.id, aim)
                })
                .filter(|(dist, bid, aim)| {
                    let full = traces_terrain(&weapon, stats.domain, Domain::Ground);
                    within_unit_weapon_reach(state, kind, Target::Building(*bid), &weapon, *dist)
                        && super::super::movement::advancing_weapon_aligned(unit, *aim - pos)
                        && shot_open(TilePos::containing(*aim), full)
                        && !chassis::path::line_blocked(pos, *aim, |t| shot_open(t, full))
                })
                .min_by_key(|(dist, bid, _)| (*dist, *bid))
                .map(|(_, bid, aim)| (Target::Building(bid), aim))
        });
    let Some((target, aim)) = target else {
        automatic_radar(state, index, id, true, events, hits, launches, &mut None);
        return;
    };

    let projectile_aim = weapon.projectile.is_some().then(|| {
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
    if !super::super::movement::advancing_weapon_aligned(unit, projectile_aim.unwrap_or(aim) - pos)
    {
        return;
    }
    if kind.stats().turret_turn_rate > 0
        && !super::super::movement::steer_weapon_heading(
            state.unit_mut(id).expect("caller checked"),
            projectile_aim.unwrap_or(aim) - pos,
        )
    {
        return;
    }
    if cooldown > 0 {
        return;
    }
    state.unit_mut(id).expect("caller checked").cooldowns[0] = weapon.cooldown_ticks;
    if weapon.projectile.is_some() {
        let aim = projectile_aim.expect("projectile aim computed");
        let flight = launch_shell(state, launches, Target::Unit(id), me, pos, aim, &weapon);
        events.push(Event::ShellLaunched {
            shooter: Target::Unit(id),
            unit_pose: Some(UnitLaunchPose::from(
                state.unit(id).expect("shooter exists during combat"),
            )),
            target: Some(target),
            player: me,
            from: pos,
            to: aim,
            flight,
        });
    } else {
        buffer_shot(state, Target::Unit(id), target, pos, aim, &weapon, hits);
        events.push(Event::AttackHit {
            attacker: id,
            attacker_kind: kind,
            weapon: 0,
            target: Some(target),
            attacker_pos: pos,
            target_pos: aim,
        });
    }
}

/// The walking charge: chase the ordered target to contact and detonate.
/// Damage is buffered like every shot, so the sapper's own death lands in
/// the same resolution as its blast. Structures take the full charge,
/// every hostile ground machine in the ring (the victim included) takes
/// the splash, and the sapper itself is always consumed.
fn sapper_attack(
    state: &mut State,
    id: UnitId,
    target: Target,
    resume: Option<crate::state::Goal>,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
) {
    let unit = state.unit(id).expect("caller checked");
    let (pos, tile, me, kind) = (unit.pos, unit.tile(), unit.player, unit.kind);
    let demolition = kind
        .stats()
        .demolition
        .expect("only demolition machines press their charge");
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
    let Some((aim_point, target_tile)) = target_info else {
        state
            .unit_mut(id)
            .expect("caller checked")
            .complete_attack(resume);
        return;
    };
    // A charge only ever presses on ground: air victims are out of reach.
    if victim_domain != Domain::Ground {
        state.unit_mut(id).expect("caller checked").clear_program();
        return;
    }
    let reach = demolition.contact_range;
    if pos.dist_sq(aim_point) <= reach * reach {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        if !super::super::movement::steer_weapon_heading(unit, aim_point - pos) {
            return;
        }
        let direct = match target {
            Target::Building(_) => demolition.structure_damage,
            Target::Unit(_) => demolition.splash_damage,
        };
        hits.push(PendingHit::along(
            state,
            Target::Unit(id),
            target,
            direct,
            pos,
            aim_point,
        ));
        let ring = demolition.blast_radius;
        let ring_sq = ring * ring;
        for u in &state.units {
            if u.hp == 0
                || !state.hostile(me, u.player)
                || Target::Unit(u.id) == target
                || u.domain() != Domain::Ground
                || u.pos.dist_sq(aim_point) > ring_sq
            {
                continue;
            }
            hits.push(PendingHit::along(
                state,
                Target::Unit(id),
                Target::Unit(u.id),
                demolition.splash_damage,
                pos,
                aim_point,
            ));
        }
        // The charge consumes its carrier, through the same buffer, so
        // a simultaneous kill on the sapper changes nothing.
        let own_hp = state.unit(id).expect("caller checked").hp;
        hits.push(PendingHit::along(
            state,
            Target::Unit(id),
            Target::Unit(id),
            own_hp,
            pos,
            aim_point,
        ));
        events.push(Event::AttackHit {
            attacker: id,
            attacker_kind: kind,
            weapon: 0,
            target: Some(target),
            attacker_pos: pos,
            target_pos: aim_point,
        });
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        return;
    }
    // Not there yet: chase without a firing stance. A path still pressing
    // the victim's tile or any doorstep around its footprint stays.
    let size = match target {
        Target::Unit(_) => (1, 1),
        Target::Building(bid) => state.building(bid).expect("resolved target").kind.size(),
    };
    let stale = state
        .unit(id)
        .expect("caller checked")
        .path
        .as_ref()
        .is_none_or(|p| {
            p.goal != target_tile && !super::super::tile_adjacent_to_rect(p.goal, target_tile, size)
        });
    if !stale {
        return;
    }
    let routed = if state.passable_for(Domain::Ground, target_tile) {
        route_for(state, kind, tile, target_tile).map(|w| (target_tile, w))
    } else if let Target::Building(_) = target {
        // A building's tiles are closed ground: press to the nearest open
        // doorstep that routes, equally near ones ranked in the Sapper's
        // approach frame so mirrored Sappers take mirrored doorsteps.
        let frame = crate::tick::rect_approach_origin(state, me, tile, target_tile, size);
        let mut doorsteps: Vec<TilePos> = super::super::rect_adjacent_tiles(target_tile, size)
            .filter(|&t| state.passable_for(Domain::Ground, t))
            .collect();
        doorsteps.sort_unstable_by_key(|&t| {
            (
                pos.dist_sq(t.center()),
                crate::geometry::rect_approach_key_from(tile, frame, target_tile, size, t),
            )
        });
        doorsteps
            .into_iter()
            .find_map(|t| route_for(state, kind, tile, t).map(|w| (t, w)))
    } else {
        None
    };
    if let Some((goal, waypoints)) = routed {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = Some(PathFollow {
            final_point: None,
            goal,
            waypoints,
            next: 0,
        });
    } else {
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

/// The attack run: a turn-limited flier never takes a firing stance. It
/// keeps a live route on its victim, steers there on a bounded arc, and
/// releases only when the bay is cold, the victim is inside release range,
/// and the victim sits in the forward cone (the geometry a straight pass
/// produces and a tight orbit cannot). Each release lays `salvo` bombs
/// along the flight line and rolls the bomber onto an egress leg past the
/// target, so the loop back covers the reload.
fn bomber_attack(
    state: &mut State,
    motion: &MotionSnapshot,
    id: UnitId,
    target: Target,
    resume: Option<crate::state::Goal>,
    events: &mut Vec<Event>,
    launches: &mut Vec<crate::state::Shell>,
) {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    let (pos, me, kind, heading, cooldowns) = (
        unit.pos,
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
        state
            .unit_mut(id)
            .expect("caller checked")
            .complete_attack(resume);
        return;
    };
    let weapon = &stats.weapons[pi];

    // Release gate: cold bay, in range, spotted, clear arc, and the
    // victim dead ahead. The sight and trace rules are the shared ones.
    let full = traces_terrain(weapon, stats.domain, victim_domain);
    let shot_open = |t: TilePos| shot_crosses(state, t, full);
    let seen = match target {
        Target::Unit(_) => state.can_see(me, target_tile),
        Target::Building(bid) => state.building(bid).is_some_and(|b| {
            b.tiles().any(|t| state.can_see(me, t)) && state.building_apparent(me, b)
        }),
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
        // A bomb falls on the building, not on the edge point that range
        // is measured to: a footprint's closest point can be a corner it
        // shares with a neighbour, and a shell landing exactly there is
        // credited to the lowest-id footprint touching it.
        let center = match target {
            Target::Building(bid) => state
                .building(bid)
                .map_or(center, crate::state::Building::center),
            Target::Unit(_) => center,
        };
        // The stick lays out along the flight line, centered on the aim
        // point; a single bomb is a one-entry stick.
        let salvo = i32::from(weapon.salvo.max(1));
        for k in 0..salvo {
            let along = Fx::from_num(2 * k - (salvo - 1)) * chassis::fx::HALF;
            let impact = center + hv * (along * crate::stats::BOMB_SALVO_SPACING);
            let impact = state.map().clamp_to_envelope(impact);
            let flight = launch_shell(state, launches, Target::Unit(id), me, pos, impact, weapon);
            events.push(Event::ShellLaunched {
                shooter: Target::Unit(id),
                unit_pose: Some(UnitLaunchPose::from(
                    state.unit(id).expect("shooter exists during combat"),
                )),
                target: Some(target),
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
        let egress = egress_goal(state, pos, heading, stats);
        let unit = state.unit_mut(id).expect("caller checked");
        unit.cooldowns[pi] = weapon.cooldown_ticks;
        unit.path = egress.map(|goal| PathFollow {
            final_point: None,
            goal,
            waypoints: vec![goal],
            next: 0,
        });
        return;
    }

    // A hot bay keeps flying. Until the reload is half done the bomber
    // never turns back toward its victim: it finishes the egress leg, then
    // keeps extending the run straight ahead. Hovering over the target to
    // re-bomb point-blank is the stop-and-strafe this chassis forbids.
    if cooldowns[pi] > weapon.cooldown_ticks / 2 {
        if state.unit(id).expect("caller checked").path.is_none()
            && let Some(goal) = egress_goal(state, pos, heading, stats)
        {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                final_point: None,
                goal,
                waypoints: vec![goal],
                next: 0,
            });
        }
        return;
    }

    // A bomber already inside release range, or inside its own turn
    // acceptance ring of the attack tile, cannot line up a pass from where
    // it is: the victim may sit outside the forward cone, and any route to
    // the tile would complete without a tick of flight. Once its approach
    // path completes, send it onward along a clear departure leg instead.
    // This covers the second half of a reload as well as a cold bay;
    // otherwise a warm bomber would plan an instantly accepted route every
    // tick and hang over the target. Keep the attack tile as PathFollow's
    // semantic goal so the chase retains this leg; when it completes, the
    // next exact-position route lines up another pass.
    let accept = stats.turn_acceptance();
    let inside_ring = pos.dist_sq(target_tile.center()) <= accept * accept;
    if (in_range || inside_ring) && state.unit(id).expect("caller checked").path.is_none() {
        let departure = egress_goal(state, pos, heading, stats);
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = departure.map(|waypoint| PathFollow {
            final_point: None,
            goal: target_tile,
            waypoints: vec![waypoint],
            next: 0,
        });
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
        if let Some(waypoints) = landing::run_in_route(
            state,
            stats,
            kind,
            pos,
            heading,
            target_tile,
            landing::RunIn::Attack,
        ) {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                final_point: None,
                goal: target_tile,
                waypoints,
                next: 0,
            });
        } else {
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

/// Where a bomber rolls out after a release: straight ahead along its
/// heading when the sky is open, bending progressively further, up to a
/// full reversal, when a wall, mesa, or map corner closes the line.
/// Every candidate must sit beyond the aircraft's own acceptance ring (a
/// goal inside it completes without a single tick of flight) and must be a
/// state the airframe can still be flown out of on arrival, so a roll-out
/// never ends pressed against the world's edge. `None` only in a closed
/// pocket, where the airframe orbits until the brain replans.
fn egress_goal(
    state: &State,
    pos: Vec2Fx,
    heading: u8,
    stats: &crate::stats::UnitStats,
) -> Option<TilePos> {
    let accept = stats.turn_acceptance();
    let accept_sq = accept * accept;
    let radius = stats.turn_radius();
    for offset in [0u8, 32, 224, 64, 192, 96, 160, 128] {
        let leg_heading = heading.wrapping_add(offset);
        let hv = chassis::compass::dir(leg_heading);
        for reach in [5i64, 4, 3] {
            let probe = pos + hv * Fx::from_num(reach);
            let tile = TilePos::containing(probe);
            if tile.x >= 0
                && tile.y >= 0
                && tile.x < state.map().width()
                && tile.y < state.map().height()
                && state.passable_for(Domain::Air, tile)
                && tile.center().dist_sq(pos) > accept_sq
                && flight::escapable(state.map(), tile.center(), leg_heading, radius)
                && !chassis::path::line_blocked(pos, tile.center(), |crossed| {
                    state
                        .map()
                        .tile(crossed)
                        .is_some_and(|terrain| !terrain.terrain.blocks_air())
                })
            {
                return Some(tile);
            }
        }
    }
    None
}

/// Chase-and-hit. Range is measured to the target's closest point and
/// shots are buffered. A vanished target — or one no carried weapon can
/// cover — hands control back to the remembered hunt (or idle,
/// where auto-acquire finds the next fight).
#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "the whole chase-and-hit decision for one unit"
)]
pub(super) fn attack(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    motion: &MotionSnapshot,
    id: UnitId,
    target: Target,
    resume: Option<crate::state::Goal>,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
) {
    let previous_braces = state.unit(id).expect("caller checked").brace_ticks;
    state.unit_mut(id).expect("caller checked").retract_braces();
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
    if stats.demolition.is_some() {
        sapper_attack(state, id, target, resume, events, hits);
        return;
    }
    let (pos, tile, me, kind, cooldowns) = (
        unit.pos,
        unit.tile(),
        unit.player,
        unit.kind,
        unit.cooldowns,
    );

    // A hunter attacking a building stays alert: an enemy unit wandering
    // into aggro takes priority (acquisition prefers units), so marching
    // armies fight back.
    if resume.is_some()
        && matches!(target, Target::Building(_))
        && let Some(better @ Target::Unit(_)) = acquire_target(state, index, id)
    {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            pursue: false,
            target: better.into(),
            resume,
        };
        unit.path = None;
        return;
    }

    // Resolve the target's current position; None means it is gone. A
    // target outside every weapon mask ends the engagement the same way.
    let target_info: Option<(Vec2Fx, TilePos)> = match target {
        Target::Unit(uid) => state
            .unit(uid)
            .filter(|t| t.hp > 0)
            .map(|t| (t.pos, t.tile())),
        Target::Building(bid) => state.building(bid).filter(|b| b.hp > 0).map(|b| {
            (
                if stats.contact_reach.is_some() {
                    state.contact_surface(b).closest(pos)
                } else {
                    b.closest_point_to(pos)
                },
                b.anchor,
            )
        }),
    };
    let victim_domain = target_domain(state, target);
    let primary = stats
        .weapons
        .iter()
        .position(|w| w.targets.covers(victim_domain));
    let (Some((aim_point, target_tile)), Some(pi)) = (target_info, primary) else {
        state
            .unit_mut(id)
            .expect("caller checked")
            .complete_attack(resume);
        return;
    };
    let weapon = &stats.weapons[pi];

    // In range only counts with a clear line and with sight (see
    // `traces_terrain`). The owner must currently see the victim's tile, so
    // a gun that outranges its own vision fires on a spotter's sight. Scrap
    // piles do not block fire. With no shot, keep approaching.
    let shot_open = |t: TilePos, full: bool| shot_crosses(state, t, full);
    // Sight of any footprint tile serves for a building (matching attack
    // validation); a unit is seen at its own tile. The line trace runs
    // only once in range — it is not built for cross-map endpoints.
    let seen = match target {
        Target::Unit(_) => state.can_see(me, target_tile),
        Target::Building(bid) => state.building(bid).is_some_and(|b| {
            b.tiles().any(|t| state.can_see(me, t)) && state.building_apparent(me, b)
        }),
    };
    let in_range = within_unit_weapon_reach(state, kind, target, weapon, pos.dist_sq(aim_point));
    let full = traces_terrain(weapon, stats.domain, victim_domain);
    // The line trace skips endpoint tiles, but a building's
    // closest-footprint aim point is an exact edge coordinate that can
    // floor into the neighboring tile. Flush against a peak, that neighbor
    // is the mountain itself, so the endpoint tile must be open too (a
    // unit's aim point is its own standable tile, and a footprint tile
    // stands on ground, which `shot_open` passes).
    let endpoint_open = shot_open(chassis::grid::TilePos::containing(aim_point), full);
    if in_range
        && seen
        && endpoint_open
        && !chassis::path::line_blocked(pos, aim_point, |t| shot_open(t, full))
    {
        let projectile_aim = weapon.projectile.is_some().then(|| {
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
        // Reaching the firing stance refreshes the leash patience window,
        // so a guard can finish a wounded runner past the radius without
        // licensing a cross-map dive. A target that never comes in reach
        // grants no window, and its chaser breaks at the radius line.
        if resume.is_none()
            && let Some(leash) = unit.leash.as_mut()
        {
            leash.patience = crate::stats::LEASH_PATIENCE;
        }
        unit.brace_ticks = previous_braces;
        if !super::super::movement::steer_weapon_heading(
            unit,
            projectile_aim.unwrap_or(aim_point) - pos,
        ) {
            return;
        }
        if cooldowns[pi] == 0 {
            unit.cooldowns[pi] = weapon.cooldown_ticks;
            if weapon.projectile.is_some() {
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
                    unit_pose: Some(UnitLaunchPose::from(
                        state.unit(id).expect("shooter exists during combat"),
                    )),
                    target: Some(target),
                    player: me,
                    from: pos,
                    to: aim_point,
                    flight,
                });
            } else {
                buffer_shot(
                    state,
                    Target::Unit(id),
                    target,
                    pos,
                    aim_point,
                    weapon,
                    hits,
                );
                events.push(Event::AttackHit {
                    attacker: id,
                    attacker_kind: kind,
                    weapon: pi,
                    target: Some(target),
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

    // A mobile long gun inside its own dead zone must create space; the
    // generic "not in range" chase would close in further. If terrain offers
    // no legal stand, hold and retry instead of making the geometry worse.
    if pos.dist_sq(aim_point) < weapon.minimum_range * weapon.minimum_range {
        if let Target::Building(building) = target {
            approach_firing_area(state, id, building, weapon);
        } else {
            retreat_to_firing_stand(state, id, target, victim_domain, weapon);
        }
        return;
    }

    // The tether binds the chase, not the trigger or the firing stand above.
    // It measures the guard's own distance from its anchor, never the
    // target's: a stationed machine travels at most the radius from its post
    // plus the patience window, independent of weapon range or speed.
    // Inside the radius the guard hunts freely; beyond it every chase tick
    // spends patience, and an empty window sends the guard walking home,
    // leash kept so the homecoming arms the post cooldown.
    if resume.is_none()
        && let Some(leash) = state.unit(id).expect("caller checked").leash
    {
        let radius_sq = crate::stats::LEASH_RADIUS * crate::stats::LEASH_RADIUS;
        if leash.anchor.center().dist_sq(pos) > radius_sq {
            if leash.patience == 0 {
                let unit = state.unit_mut(id).expect("caller checked");
                unit.order = Order::Run {
                    goal: leash.anchor.into(),
                };
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
            // A ground chaser cannot stand where a flyer hovers over rock or
            // a roof, so it marches to a tile it can stand on and shoot from
            // instead. Air chasers and standable victim tiles keep the
            // direct goal.
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
                    // Nearest ring first, then ranked in the chaser's
                    // approach frame so a mirrored chaser tries the mirrored
                    // stand-in; the first that routes wins, so an isolated
                    // pocket next to the victim must not stall a chaser that
                    // could fire from the far side.
                    let mut stand_ins =
                        chase_stand_ins(state, stats.domain, target_tile, weapon.range);
                    stand_ins.sort_by_key(|goal| {
                        (
                            goal.chebyshev(target_tile),
                            crate::geometry::rect_approach_key(tile, target_tile, (1, 1), *goal),
                        )
                    });
                    stand_ins
                        .into_iter()
                        .find_map(|goal| route_for(state, kind, tile, goal).map(|w| (goal, w)))
                };
                match routed {
                    Some((goal, waypoints)) => {
                        let unit = state.unit_mut(id).expect("caller checked");
                        unit.path = Some(PathFollow {
                            final_point: None,
                            goal,
                            waypoints,
                            next: 0,
                        });
                        Ok(())
                    }
                    // NoFiringPosition derives from the victim's footing;
                    // report it only while the team sees that ground, or the
                    // stall would leak where a fogged flyer parked. Unseen,
                    // report that no route worked.
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
            let approached = if stats.contact_reach.is_some() {
                contact::approach(state, id, bid)
            } else {
                approach_firing_area(state, id, bid, weapon)
            };
            if approached {
                Ok(())
            } else {
                Err(StallReason::NoRoute)
            }
        }
    };
    if let Err(reason) = reached {
        stall_attack(state, id, resume, reason, events);
    }
}

fn stall_attack(
    state: &mut State,
    id: UnitId,
    resume: Option<crate::state::Goal>,
    reason: StallReason,
    events: &mut Vec<Event>,
) {
    let unit = state.unit_mut(id).expect("caller checked");
    // A tethered chase that cannot route breaks off home quietly: it
    // abandons its own acquisition, not a player order, so no stall event.
    if resume.is_none()
        && let Some(leash) = unit.leash
    {
        unit.order = Order::Run {
            goal: leash.anchor.into(),
        };
        unit.path = None;
        return;
    }
    let (player, pos) = (unit.player, unit.pos);
    // A chase that could not be prosecuted leaves the unit stationed,
    // so its next idle acquisition tethers and the leash cooldown
    // paces re-acquisition instead of the same refused chase firing
    // every tick.
    unit.settled = crate::stats::LEASH_STATION_TICKS;
    unit.clear_program();
    events.push(Event::OrderStalled {
        unit: id,
        player,
        pos,
        reason,
    });
}

/// The sidearm's opportunist victim: the nearest hostile unit the weapon
/// can cover, in range, seen by the owner, and clear, chosen through the
/// spatial index's reach window. Selection is keyed `(distance, id)`, so
/// window visit order cannot change the answer.
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
            let domain = u.domain();
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
            blind::sidearm_radar(state, id, wi, hits, events);
            continue;
        };
        if !super::super::movement::ground_weapon_aligned(
            state.unit(id).expect("caller checked"),
            upos - pos,
        ) {
            continue;
        }
        state.unit_mut(id).expect("caller checked").cooldowns[wi] = weapon.cooldown_ticks;
        buffer_shot(
            state,
            Target::Unit(id),
            Target::Unit(uid),
            pos,
            upos,
            weapon,
            hits,
        );
        events.push(Event::AttackHit {
            attacker: id,
            attacker_kind: kind,
            weapon: wi,
            target: Some(Target::Unit(uid)),
            attacker_pos: pos,
            target_pos: upos,
        });
    }
}

/// Damage answers back: a hit unit that can fight and isn't already
/// fighting turns on its attacker, which counters weapons that outrange
/// aggro. A hunter keeps its destination as the resume point. Hits resolve
/// in decision order, so the first answerable attacker of a tick wins.
pub(super) fn retaliate(state: &mut State, victim: UnitId, attacker: Target) {
    let Some(unit) = state.unit(victim) else {
        return;
    };
    let stats = unit.kind.stats();
    let attacker_domain = target_domain(state, attacker);
    if unit.hp == 0 || !stats.can_target(attacker_domain) {
        return;
    }
    // Answering fire needs sight: chasing a shell lobbed from beyond every
    // friendly sight line would leak the shooter's position.
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
        Order::Hunt { goal } => Some(goal),
        // A tethered homecoming answers fire: the walk home resumes through
        // the leash once the attacker falls, so no resume goal is carried. A
        // plain Run is the player's recall verb and never auto-engages.
        Order::Run { .. } if unit.leash.is_some() => None,
        // An attack aimed at something that just died in resolution is no
        // engagement; without this arm a surviving out-of-aggro shooter
        // would fire unanswered for another full cooldown. Attacks on live
        // targets are not interrupted.
        Order::Attack { target, resume, .. }
            if state.attack_view(unit.player, target).is_none() =>
        {
            resume
        }
        _ => return, // already busy fighting or working
    };
    let target = state
        .attack_objective(unit.player, attacker.into())
        .unwrap_or(attacker.into());
    let unit = state.unit_mut(victim).expect("checked above");
    unit.order = Order::Attack {
        pursue: false,
        target,
        resume,
    };
    unit.path = None;
    // Answering a hit refreshes the patience window, so the answer can
    // reach an attacker just past the radius (such as a repositioning
    // Bombard) without opening a cross-map dive. An answer that resumes a
    // march stays untethered. An answer with no resume is self-acquisition:
    // an existing tether keeps its anchor, a stationed machine gets a fresh
    // one where it stood, and an unsettled machine answers unleashed.
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

fn approach_firing_area(
    state: &mut State,
    id: UnitId,
    building: crate::BuildingId,
    weapon: &WeaponStats,
) -> bool {
    let unit = state.unit(id).expect("attacker");
    let b = state.building(building).expect("visible building");
    let (from, kind, player, pos) = (unit.tile(), unit.kind, unit.player, unit.pos);
    let domain = unit.domain();
    let full = traces_terrain(weapon, domain, Domain::Ground);
    let legal = |point: Vec2Fx| {
        let aim = b.closest_point_to(point);
        state.passable_for(domain, TilePos::containing(point))
            && crate::tick::crowding::standable(state, unit, point)
            && within_weapon_reach(weapon, point.dist_sq(aim))
            && shot_crosses(state, TilePos::containing(aim), full)
            && !chassis::path::line_blocked(point, aim, |t| shot_crosses(state, t, full))
    };
    if unit.path.as_ref().is_some_and(|path| {
        let point = path.final_point.unwrap_or(path.goal.center());
        legal(point) && !crate::tick::crowding::claimed(state, id, point, true)
    }) {
        return true;
    }
    let r = weapon.range.ceil().to_num::<i32>();
    let (w, h) = b.kind.size();
    let frame = crate::tick::rect_approach_origin(state, player, from, b.anchor, (w, h));
    let mut points = Vec::new();
    for y in ((b.anchor.y - r).max(0) * 2)..((b.anchor.y + h + r).min(state.map.height()) * 2) {
        for x in ((b.anchor.x - r).max(0) * 2)..((b.anchor.x + w + r).min(state.map.width()) * 2) {
            let point = Vec2Fx::new(
                Fx::from_num(x) / 2 + const { Fx::lit("0.25") },
                Fx::from_num(y) / 2 + const { Fx::lit("0.25") },
            );
            if legal(point) {
                points.push(point);
            }
        }
    }
    let aim = b.closest_point_to(pos);
    let distance = pos.dist(aim);
    if distance > weapon.range {
        let point = pos.move_toward(aim, distance - weapon.range + const { Fx::lit("0.02") });
        if legal(point) {
            points.push(point);
        }
    }
    points.sort_by_key(|&point| {
        (
            pos.dist_sq(point),
            crate::geometry::rect_approach_key_from(
                from,
                frame,
                b.anchor,
                (w, h),
                TilePos::containing(point),
            ),
            (point.x - pos.x) * (frame.center().y - pos.y)
                - (point.y - pos.y) * (frame.center().x - pos.x),
        )
    });
    let candidates = points
        .into_iter()
        .map(|point| crate::tick::crowding::Position {
            goal: TilePos::containing(point),
            point,
        })
        .collect();
    let path = crate::tick::crowding::choose(
        state,
        id,
        candidates,
        b.center(),
        |tile| state.passable_for(domain, tile),
        |goal| super::super::route_for_position(state, kind, pos, goal),
    );
    let reachable = path.is_some();
    state.unit_mut(id).expect("attacker").path = path;
    reachable
}

#[cfg(test)]
mod tests;
