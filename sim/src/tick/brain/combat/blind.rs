//! Firing decisions use observations; only impact resolution reads occupants.

use super::*;
use crate::state::AttackView;
use crate::{AttackTarget, BuildingId};

pub(in crate::tick::brain) struct ShotBuffers<'a> {
    pub(in crate::tick::brain) events: &'a mut Vec<Event>,
    pub(in crate::tick::brain) hits: &'a mut Vec<PendingHit>,
    pub(in crate::tick::brain) launches: &'a mut Vec<crate::state::Shell>,
}

pub(super) fn solution(
    state: &State,
    from: Vec2Fx,
    shooter: Domain,
    view: AttackView,
    weapon: &WeaponStats,
) -> bool {
    let aim = view.aim_from(from);
    view.domain.is_none_or(|d| weapon.targets.covers(d))
        && within_weapon_reach(weapon, from.dist_sq(aim))
        && predicted_impact_open(state, from, aim, terrain_cover(view, weapon, shooter))
}

fn terrain_cover(view: AttackView, weapon: &WeaponStats, shooter: Domain) -> bool {
    view.domain
        .map_or(!weapon.indirect && weapon.targets.ground, |domain| {
            traces_terrain(weapon, shooter, domain)
        })
}

pub(super) fn radar_in_range(
    state: &State,
    player: PlayerId,
    from: Vec2Fx,
    domain: Domain,
    weapon: &WeaponStats,
) -> Option<AttackView> {
    state
        .vision(player)
        .tracks()
        .iter()
        .filter(|track| track.visible_unit.is_none())
        .filter_map(|track| {
            state
                .attack_view(player, AttackTarget::Contact(track.id))
                .map(|view| (track.id, view))
        })
        .filter(|(_, view)| solution(state, from, domain, *view, weapon))
        .min_by_key(|(id, view)| (from.dist_sq(view.position), *id))
        .map(|(_, view)| view)
}

fn predicted_aim(
    state: &State,
    from: Vec2Fx,
    domain: Domain,
    view: AttackView,
    weapon: &WeaponStats,
) -> Vec2Fx {
    let current = view.aim_from(from);
    let Some(projectile) = weapon.projectile.filter(|_| view.footprint.is_none()) else {
        return current;
    };
    let mut aim = current;
    let mut flight = shell_flight(from, aim, projectile.speed);
    for _ in 0..8 {
        let predicted =
            current + view.velocity * Fx::from_num(flight.min(crate::stats::MAX_LEAD_TICKS));
        let next = weapon.splash.map_or(predicted, |radius| {
            aim_at_near_splash_edge(current, predicted, radius)
        });
        if from.dist_sq(next) < weapon.minimum_range * weapon.minimum_range {
            return current;
        }
        aim = if from.dist_sq(next) > weapon.range * weapon.range {
            from.move_toward(next, weapon.range)
        } else {
            next
        };
        let next_flight = shell_flight(from, aim, projectile.speed);
        if next_flight == flight {
            break;
        }
        flight = next_flight;
    }
    if predicted_impact_open(state, from, aim, terrain_cover(view, weapon, domain)) {
        aim
    } else {
        current
    }
}

fn buffer_blind(
    state: &State,
    shooter: (Target, PlayerId),
    from: Vec2Fx,
    aim: Vec2Fx,
    view: AttackView,
    weapon: &WeaponStats,
    hits: &mut Vec<PendingHit>,
) {
    let (shooter, player) = shooter;
    let building = || {
        state
            .buildings()
            .iter()
            .filter(|b| {
                weapon.targets.ground
                    && b.hp > 0
                    && !b.provisional
                    && state.hostile(player, b.player)
                    && b.closest_point_to(aim).dist_sq(aim) <= const { Fx::lit("0.0001") }
            })
            .min_by_key(|b| b.id)
            .map(|b| Target::Building(b.id))
    };
    let direct = if view.footprint.is_some() {
        building()
    } else {
        let tile = TilePos::containing(view.position);
        state
            .units()
            .iter()
            .filter(|u| {
                u.hp > 0
                    && state.hostile(player, u.player)
                    && weapon.targets.covers(u.domain())
                    && u.tile() == tile
            })
            .min_by_key(|u| (u.pos.dist_sq(tile.center()), u.id))
            .map(|u| Target::Unit(u.id))
            // A radar blip can be a building's footprint tile.
            .or_else(building)
    };
    if let Some(victim) = direct {
        buffer_shot(state, shooter, victim, from, aim, weapon, hits);
    } else if let Some(radius) = weapon.splash {
        for u in state.units().iter().filter(|u| {
            u.hp > 0
                && state.hostile(player, u.player)
                && weapon.targets.covers(u.domain())
                && u.pos.dist_sq(aim) <= radius * radius
        }) {
            hits.push(PendingHit::along(
                state,
                shooter,
                Target::Unit(u.id),
                weapon.damage,
                from,
                aim,
            ));
        }
        for b in state.buildings().iter().filter(|b| {
            b.hp > 0
                && !b.provisional
                && state.hostile(player, b.player)
                && weapon.targets.ground
                && b.kind.is_stealthy()
                && b.center().dist_sq(aim) <= radius * radius
        }) {
            hits.push(PendingHit::along(
                state,
                shooter,
                Target::Building(b.id),
                weapon.damage,
                from,
                aim,
            ));
        }
    }
}

pub(super) fn fire_building(
    state: &mut State,
    id: BuildingId,
    view: AttackView,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
) {
    let b = state.building(id).expect("live defense");
    let (player, from, kind, tier, weapon) =
        (b.player, b.center(), b.kind, b.tier, b.stats().weapons[0]);
    let aim = predicted_aim(state, from, Domain::Ground, view, &weapon);
    state.building_mut(id).expect("live defense").cooldown = weapon.cooldown_ticks;
    if weapon.projectile.is_some() {
        let flight = launch_shell(
            state,
            launches,
            Target::Building(id),
            player,
            from,
            aim,
            &weapon,
        );
        events.push(Event::ShellLaunched {
            shooter: Target::Building(id),
            unit_pose: None,
            target: None,
            player,
            from,
            to: aim,
            flight,
        });
    } else {
        buffer_blind(
            state,
            (Target::Building(id), player),
            from,
            aim,
            view,
            &weapon,
            hits,
        );
        events.push(Event::TurretFired {
            turret: id,
            kind,
            tier,
            target: None,
            turret_pos: from,
            target_pos: aim,
        });
    }
}

fn fire_unit(
    state: &mut State,
    id: UnitId,
    view: AttackView,
    primary: usize,
    moving: bool,
    buffers: ShotBuffers<'_>,
) {
    let ShotBuffers {
        events,
        hits,
        launches,
    } = buffers;
    let u = state.unit(id).expect("live shooter");
    let (player, from, kind) = (u.player, u.pos, u.kind);
    let weapon = &kind.stats().weapons[primary];
    let aim = predicted_aim(state, from, kind.stats().domain, view, weapon);
    let u = state.unit_mut(id).expect("live shooter");
    if !moving {
        u.path = None;
    }
    if moving && kind.stats().turret_turn_rate == 0 {
        if !super::super::super::movement::ground_weapon_aligned(u, aim - from) {
            return;
        }
    } else if !super::super::super::movement::steer_weapon_heading(u, aim - from) {
        return;
    }
    if u.cooldowns[primary] > 0 {
        return;
    }
    u.cooldowns[primary] = weapon.cooldown_ticks;
    if weapon.projectile.is_some() {
        let flight = launch_shell(state, launches, Target::Unit(id), player, from, aim, weapon);
        events.push(Event::ShellLaunched {
            shooter: Target::Unit(id),
            unit_pose: Some(UnitLaunchPose::from(state.unit(id).expect("live shooter"))),
            target: None,
            player,
            from,
            to: aim,
            flight,
        });
    } else {
        buffer_blind(
            state,
            (Target::Unit(id), player),
            from,
            aim,
            view,
            weapon,
            hits,
        );
        events.push(Event::AttackHit {
            attacker: id,
            attacker_kind: kind,
            weapon: primary,
            target: None,
            attacker_pos: from,
            target_pos: aim,
        });
    }
}

/// Fires an automatic unit's weapon at a radar contact in reach when it has
/// no ordinary target. `acquired` receives the ordinary acquisition whenever
/// this judged one, so a brain later in the same tick need not repeat it.
#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::option_option,
    reason = "`None` means no ordinary search ran; `Some(None)` means it found nothing"
)]
pub(in crate::tick::brain) fn automatic_radar(
    state: &mut State,
    index: &super::super::super::spatial::UnitIndex,
    id: UnitId,
    moving: bool,
    events: &mut Vec<Event>,
    hits: &mut Vec<PendingHit>,
    launches: &mut Vec<crate::state::Shell>,
    acquired: &mut Option<Option<Target>>,
) -> bool {
    let u = state.unit(id).expect("live unit");
    let stats = u.kind.stats();
    if u.kind.stats().contact_reach.is_some()
        || stats.turn_rate > 0
        || stats.weapons.is_empty()
        || (moving && stats.brace.is_some())
        || {
            let target = acquire_target(state, index, id);
            *acquired = Some(target);
            target.is_some()
        }
    {
        return false;
    }
    let Some((slot, view)) = stats.weapons.iter().enumerate().find_map(|(slot, weapon)| {
        radar_in_range(state, u.player, u.pos, stats.domain, weapon).map(|view| (slot, view))
    }) else {
        return false;
    };
    fire_unit(
        state,
        id,
        view,
        slot,
        moving,
        ShotBuffers {
            events,
            hits,
            launches,
        },
    );
    fire_sidearms(state, index, id, slot, hits, events);
    true
}

pub(super) fn sidearm_radar(
    state: &mut State,
    id: UnitId,
    slot: usize,
    hits: &mut Vec<PendingHit>,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("live unit");
    let (player, pos, kind) = (unit.player, unit.pos, unit.kind);
    let weapon = &kind.stats().weapons[slot];
    let Some(view) = radar_in_range(state, player, pos, unit.domain(), weapon) else {
        return;
    };
    if !super::super::super::movement::ground_weapon_aligned(unit, view.position - pos) {
        return;
    }
    state.unit_mut(id).expect("live unit").cooldowns[slot] = weapon.cooldown_ticks;
    buffer_blind(
        state,
        (Target::Unit(id), player),
        pos,
        view.position,
        view,
        weapon,
        hits,
    );
    events.push(Event::AttackHit {
        attacker: id,
        attacker_kind: kind,
        weapon: slot,
        target: None,
        attacker_pos: pos,
        target_pos: view.position,
    });
}

fn complete(state: &mut State, id: UnitId, resume: Option<crate::state::Goal>) {
    state
        .unit_mut(id)
        .expect("live unit")
        .complete_attack(resume);
}

pub(in crate::tick::brain) fn attack_known(
    state: &mut State,
    index: &super::super::super::spatial::UnitIndex,
    motion: &MotionSnapshot,
    id: UnitId,
    program: (AttackTarget, Option<crate::state::Goal>, bool),
    buffers: ShotBuffers<'_>,
) {
    let (target, resume, pursue) = program;
    let ShotBuffers {
        events,
        hits,
        launches,
    } = buffers;
    let u = state.unit(id).expect("live unit");
    let (player, pos, tile, kind) = (u.player, u.pos, u.tile(), u.kind);
    let Some(target) = state.attack_objective(player, target) else {
        complete(state, id, resume);
        return;
    };
    let Some(view) = state.attack_view(player, target) else {
        complete(state, id, resume);
        return;
    };
    state.unit_mut(id).expect("live unit").order = Order::Attack {
        target,
        resume,
        pursue,
    };
    if !pursue
        && view.domain.is_none()
        && let Some(visible) = acquire_target(state, index, id)
    {
        state.unit_mut(id).expect("live unit").order = Order::Attack {
            target: visible.into(),
            resume,
            pursue: false,
        };
        attack(
            state, index, motion, id, visible, resume, events, hits, launches,
        );
        normalize_order(state, id);
        return;
    }
    if let Some(entity) = view.entity {
        attack(
            state, index, motion, id, entity, resume, events, hits, launches,
        );
        normalize_order(state, id);
        return;
    }
    let stats = kind.stats();
    let primary = stats
        .weapons
        .iter()
        .position(|weapon| view.domain.is_none_or(|d| weapon.targets.covers(d)));
    if let Some(primary) = primary {
        let weapon = &stats.weapons[primary];
        if kind.stats().contact_reach.is_none() && solution(state, pos, stats.domain, view, weapon)
        {
            // Turn-limited bombers obtain their own sight before release range.
            if stats.turn_rate == 0 {
                fire_unit(
                    state,
                    id,
                    view,
                    primary,
                    false,
                    ShotBuffers {
                        events: &mut *events,
                        hits: &mut *hits,
                        launches: &mut *launches,
                    },
                );
                fire_sidearms(state, index, id, primary, hits, events);
                return;
            }
        }
    } else if stats.demolition.is_none() {
        complete(state, id, resume);
        return;
    }
    if !pursue && view.footprint.is_none() {
        complete(state, id, resume);
        return;
    }
    if let Some(primary) = primary {
        fire_sidearms(state, index, id, primary, hits, events);
    }
    state.unit_mut(id).expect("live unit").retract_braces();
    let aim = view.aim_from(pos);
    let goal = TilePos::containing(view.position);
    let inside_dead_zone = primary.is_some_and(|i| {
        pos.dist_sq(aim) < stats.weapons[i].minimum_range * stats.weapons[i].minimum_range
    });
    if inside_dead_zone {
        if !route_to_firing_stand(state, id, view, &stats.weapons[primary.expect("weapon")]) {
            stall_attack(state, id, resume, StallReason::NoRoute, events);
        }
        return;
    }
    let routed = if let Some((anchor, size)) = view.footprint
        && stats.domain == Domain::Ground
    {
        approach_rect(state, id, anchor, size)
    } else if state
        .unit(id)
        .expect("live unit")
        .path
        .as_ref()
        .is_some_and(|p| p.goal == goal)
    {
        true
    } else if let Some(waypoints) = route_for(state, kind, tile, goal) {
        state.unit_mut(id).expect("live unit").path = Some(PathFollow {
            final_point: None,
            goal,
            waypoints,
            next: 0,
        });
        true
    } else {
        false
    };
    if !routed
        && !primary
            .is_some_and(|primary| route_to_firing_stand(state, id, view, &stats.weapons[primary]))
    {
        stall_attack(state, id, resume, StallReason::NoRoute, events);
    }
}

fn route_to_firing_stand(
    state: &mut State,
    id: UnitId,
    view: AttackView,
    weapon: &WeaponStats,
) -> bool {
    let domain = state.unit(id).expect("live unit").kind.stats().domain;
    let legal = |state: &State, tile: TilePos| {
        state.passable_for(domain, tile) && solution(state, tile.center(), domain, view, weapon)
    };
    let footprint = view
        .footprint
        .unwrap_or((TilePos::containing(view.position), (1, 1)));
    let reach = weapon.range.ceil().to_num::<i32>();
    route_to_stand(state, id, footprint, reach, legal)
}

pub(in crate::tick::brain) fn normalize_order(state: &mut State, id: UnitId) {
    let u = state.unit(id).expect("live unit");
    if let Order::Attack {
        target,
        resume,
        pursue,
    } = u.order
        && let Some(target) = state.attack_objective(u.player, target)
    {
        state.unit_mut(id).expect("live unit").order = Order::Attack {
            target,
            resume,
            pursue,
        };
    }
}

#[cfg(test)]
mod tests;
