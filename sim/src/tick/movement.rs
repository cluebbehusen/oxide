//! Phases 4–5: footprint eviction, path following, and collision
//! resolution.
//!
//! Movement is per-unit work. Ground can close *during* a walk because a
//! construction site claims its footprint when the command lands, so each
//! step revalidates its next waypoint and drops a blocked path for the brain
//! to plan again next tick. A pathless ground body left on claimed ground
//! walks itself off through [`evict_claimed_ground`] rather than teleporting.
//! Collision resolution then pushes overlapping
//! bodies apart until they fit — units are solid to each other, but tiles
//! are only ever blocked by terrain and buildings, so pathfinding stays
//! deadlock-free while crowds physically jostle.

mod cruise;
mod ground;

use super::flight;
use crate::map::Map;
use crate::state::{GroundTerrain, Order, ParkedBodies, PathFollow, State};
use chassis::fx::{Fx, Vec2Fx, sqrt};
use chassis::grid::TilePos;

use crate::stats::{
    COLLISION_ITERATIONS, COLLISION_MAX_STEP, COLLISION_SLOP, SLIDE_LATERAL_SHARE,
    SLIDE_RADIAL_SHARE, WAYPOINT_ACCEPT,
};

pub(super) fn steer_ground_heading(unit: &mut crate::state::Unit, direction: Vec2Fx) -> bool {
    let rate = unit.kind.ground_turn_rate();
    steer_heading(unit, direction, rate)
}

pub(super) fn steer_weapon_heading(unit: &mut crate::state::Unit, direction: Vec2Fx) -> bool {
    if unit.kind.has_ground_turret() {
        let bearing = unit.turret_heading.get_or_insert(unit.heading);
        return steer_bearing(bearing, direction, unit.kind.turret_turn_rate());
    }
    if unit.drive_speed > Fx::ZERO {
        return false;
    }
    if unit.kind == crate::UnitKind::Bombard {
        if !ground_weapon_aligned(unit, direction) {
            if unit.brace_ticks > 0 {
                unit.retract_braces();
            } else {
                steer_ground_heading(unit, direction);
            }
            return false;
        }
        unit.brace_ticks = (unit.brace_ticks + 1).min(crate::stats::BOMBARD_BRACE_TICKS);
        return unit.brace_ticks == crate::stats::BOMBARD_BRACE_TICKS;
    }
    let rate = unit
        .kind
        .ground_turn_rate()
        .max(unit.kind.turret_turn_rate())
        .max(unit.kind.cruise_turn_rate());
    steer_heading(unit, direction, rate)
}

fn steer_heading(unit: &mut crate::state::Unit, direction: Vec2Fx, rate: u8) -> bool {
    steer_bearing(&mut unit.heading, direction, rate)
}

fn steer_bearing(heading: &mut u8, direction: Vec2Fx, rate: u8) -> bool {
    if rate == 0 || direction == Vec2Fx::ZERO {
        return true;
    }
    let desired = chassis::compass::heading_of(direction);
    let delta = i16::from(desired.wrapping_sub(*heading) as i8);
    let step = delta.clamp(-i16::from(rate), i16::from(rate));
    *heading = heading.wrapping_add_signed(step as i8);
    heading_aligned(*heading, desired)
}

fn heading_aligned(current: u8, desired: u8) -> bool {
    desired.wrapping_sub(current).cast_signed().unsigned_abs() <= 2
}

pub(super) fn ground_weapon_aligned(unit: &crate::state::Unit, direction: Vec2Fx) -> bool {
    (unit.kind.ground_turn_rate() == 0 && unit.kind.cruise_turn_rate() == 0)
        || direction == Vec2Fx::ZERO
        || heading_aligned(
            unit.weapon_heading(),
            chassis::compass::heading_of(direction),
        )
}

pub(super) fn advancing_weapon_aligned(unit: &crate::state::Unit, direction: Vec2Fx) -> bool {
    unit.kind.has_ground_turret() || ground_weapon_aligned(unit, direction)
}

fn work_aim(state: &State, unit: &crate::state::Unit) -> Option<Vec2Fx> {
    if unit.path.is_some() || unit.kind.ground_turn_rate() == 0 {
        return None;
    }
    if let Some(release) = unit.unloading {
        return state
            .building(release.foundry)
            .map(|b| state.contact_surface(b).closest(unit.pos));
    }
    match unit.order {
        Order::ReturnCargo { foundry, .. } => state
            .building(foundry)
            .map(|b| state.contact_surface(b).closest(unit.pos)),
        Order::Harvest { node, .. } => Some(node.center()),
        Order::Build { site } => state
            .building(site)
            .map(|b| state.contact_surface(b).closest(unit.pos)),
        Order::Repair { building } | Order::Salvage { building } => state
            .building(building)
            .map(|b| state.contact_surface(b).closest(unit.pos)),
        Order::RepairUnit { unit: patient } => state.unit(patient).map(|u| u.pos),
        _ => None,
    }
}

/// Whether skipping from tile `cur` toward `nxt` early can clip impassable ground. Cardinal
/// neighbors are always safe — the swept band stays inside two open tiles.
/// Diagonals are safe only when both shared cardinal tiles are open (then
/// the whole 2×2 block is open); that is the same invariant A* enforces on
/// the path itself. Callers check both the authored path leg and the shortcut
/// from the body's actual tile because collision can carry a body past a
/// waypoint from an adjacent tile.
fn early_advance_safe(cur: TilePos, nxt: TilePos, terrain: &GroundTerrain) -> bool {
    let (dx, dy) = (nxt.x - cur.x, nxt.y - cur.y);
    if dx == 0 || dy == 0 {
        return true;
    }
    terrain.open(cur.offset(dx, 0)) && terrain.open(cur.offset(0, dy))
}

/// Whether a body deflected around traffic has already crossed an
/// intermediate waypoint toward the following leg. The bounded reach keeps
/// an unrelated point in the onward half-plane from skipping part of a route;
/// the caller still applies [`early_advance_safe`] and revalidates the next
/// step before moving.
fn passed_intermediate_waypoint(pos: Vec2Fx, waypoint: TilePos, next: TilePos, radius: Fx) -> bool {
    let center = waypoint.center();
    let offset = pos - center;
    let onward = next.center() - center;
    let reach = radius.max(WAYPOINT_ACCEPT) + COLLISION_MAX_STEP;
    offset.length_sq() <= reach * reach && offset.x * onward.x + offset.y * onward.y > Fx::ZERO
}

/// The nearest walkable escape from a body's own (possibly blocked)
/// tile: candidates ring-scan outward in a half-turn-equivariant frame, and the
/// first one that routes wins (A* consults `passable` for every tile except the start,
/// so a body paths out of ground it could not enter). Bounded: any real
/// escape begins on an adjacent open tile, so the reach only pads for
/// corner-cut geometry.
pub(super) fn escape_route(
    state: &State,
    kind: crate::stats::UnitKind,
    from: TilePos,
    heading: u8,
) -> Option<PathFollow> {
    let reflected = TilePos::new(
        state.map.width() - 1 - from.x,
        state.map.height() - 1 - from.y,
    );
    let reverse = match (from.y, from.x).cmp(&(reflected.y, reflected.x)) {
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Equal => heading < 128,
    };
    let direction = if reverse { -1 } else { 1 };
    for r in 1..=crate::stats::EVICT_SCAN_RADIUS {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let goal = from.offset(dx * direction, dy * direction);
                if !state.passable(goal) {
                    continue;
                }
                if let Some(waypoints) = super::route_for(state, kind, from, goal) {
                    return Some(PathFollow {
                        final_point: None,
                        goal,
                        waypoints,
                        next: 0,
                    });
                }
            }
        }
    }
    None
}

/// Pure preview of the phase-5 claimed-ground eviction for one unit.
///
/// Brains that require a body to remain still can consult the exact same
/// predicate and route that [`evict_claimed_ground`] will apply later in
/// the tick, without mutating the state early.
pub(super) fn claimed_ground_escape(state: &State, id: crate::ids::UnitId) -> Option<PathFollow> {
    let unit = state.unit(id)?;
    if unit.hp == 0
        || unit.domain() != crate::stats::Domain::Ground
        || unit.path.is_some()
        || !state.ground_terrain().building_blocks(unit.tile())
    {
        return None;
    }
    if super::brain::contact::surface_for(state, unit).is_some_and(|s| {
        s.clear(
            unit.pos,
            unit.pos,
            super::brain::contact::collision_radius(unit),
        )
    }) && matches!(
        unit.order,
        Order::Attack { .. }
            | Order::Build { .. }
            | Order::Repair { .. }
            | Order::Salvage { .. }
            | Order::ReturnCargo { .. }
            | Order::Harvest { .. }
    ) {
        return None;
    }
    escape_route(state, unit.kind, unit.tile(), unit.heading)
}

/// Phase-5 pre-pass: a pathless ground body standing on a building
/// footprint walks off — an accepted foundation claims its ground
/// instantly, and no sim rule expects a resting unit on a claimed
/// footprint. Sets `path` ONLY: orders, queue, progress, leash, and
/// settle all survive, so the body keeps its job while it clears the
/// ground. Re-arms every tick because working brains null the path
/// while standing still (extract, attack-in-range) — brains run first,
/// eviction re-arms, movement consumes. Id order; deterministic scan.
/// No route means the body stays put — a crowd the sim already
/// tolerates — except at placement time, where `apply_build` deals a
/// routeless body onto the perimeter instantly so nothing can end up
/// inside a finished building.
pub(super) fn evict_claimed_ground(state: &mut State) {
    for i in 0..state.units.len() {
        let id = state.units[i].id;
        if let Some(path) = claimed_ground_escape(state, id) {
            // A landed airframe leaves a claimed footprint the only way it
            // can: it lifts off along that route.
            state.units[i].landed = false;
            state.units[i].path = Some(path);
        }
    }
}

/// After collisions: a ground body whose contact cancelled most of its
/// intended progress toward its waypoint, or whose motor refused the step
/// outright, for [`crate::stats::STALL_REPLAN_TICKS`] running ticks drops
/// its route, so its brain plans again from where the body actually is.
/// `travel`, `refused` and `driven` are this tick's propulsion, motor
/// refusals and post-propulsion positions, indexed like `state.units`.
pub(super) fn note_stalls(
    state: &mut State,
    travel: &[Vec2Fx],
    refused: &[bool],
    driven: &[Vec2Fx],
) {
    for (slot, unit) in state.units.iter_mut().enumerate() {
        let stalled = unit.hp > 0
            && unit.domain() == crate::stats::Domain::Ground
            && (refused[slot] || travel[slot] != Vec2Fx::ZERO)
            && unit
                .path
                .as_ref()
                .and_then(|path| {
                    path.waypoints
                        .get(path.next as usize)
                        .map(|_| ground::path_point(path, path.next as usize))
                })
                .is_some_and(|point| {
                    if refused[slot] {
                        return true;
                    }
                    let before = driven[slot] - travel[slot];
                    let toward = point - before;
                    let net = unit.pos - before;
                    let wanted = travel[slot].x * toward.x + travel[slot].y * toward.y;
                    let made = net.x * toward.x + net.y * toward.y;
                    wanted > Fx::ZERO && made * Fx::from_num(4) < wanted
                });
        if !stalled {
            unit.stall_ticks = 0;
            continue;
        }
        unit.stall_ticks += 1;
        if unit.stall_ticks >= crate::stats::STALL_REPLAN_TICKS {
            unit.path = None;
            unit.stall_ticks = 0;
        }
    }
}

/// A stall count describes the route being walked. Construction, site
/// cancellation and cleanup run after [`note_stalls`] and may drop that route,
/// so the count goes with it before the tick ends; otherwise the state fails
/// its own invariants and the next route inherits a stale count.
pub(super) fn forget_stalls_without_routes(state: &mut State) {
    for unit in &mut state.units {
        if unit.path.is_none() {
            unit.stall_ticks = 0;
        }
    }
}

/// Advances every unit along its path by its speed, returning each
/// unit's displacement this tick and whether the ground motor refused the
/// step its route asked for (both indexed like `state.units`) — the
/// collision resolver reads travel to slide movers around each other
/// instead of grinding them head-on. Intermediate waypoints are accepted
/// within [`WAYPOINT_ACCEPT`], or after a nearby collision deflection has
/// carried the body across the onward plane, so a unit does not turn back
/// toward a center it already passed. Final waypoints are landed exactly.
pub(super) fn run(state: &mut State) -> (Vec<Vec2Fx>, Vec<bool>) {
    let contacts: Vec<_> = state
        .units
        .iter()
        .map(|unit| super::brain::contact::surface_for(state, unit))
        .collect();
    let work_aims: Vec<_> = state
        .units
        .iter()
        .map(|unit| work_aim(state, unit))
        .collect();
    // Friendly bodies at rest, per side: a ground follower will not admit
    // a lookahead leg through one, though it still walks its planned tiles.
    let parked: Vec<ParkedBodies> = (0..state.players.len())
        .map(|player| state.parked_bodies(crate::ids::PlayerId(player as u8)))
        .collect();
    // Disjoint field borrows: units move, terrain is read-only.
    let State {
        units,
        map,
        building_occupancy,
        ..
    } = state;
    let terrain = GroundTerrain::new(map, building_occupancy);
    let mut travel = vec![Vec2Fx::ZERO; units.len()];
    let mut refused = vec![false; units.len()];
    for (slot, unit) in units.iter_mut().enumerate() {
        if unit.hp == 0 || unit.brace_ticks > 0 {
            continue;
        }
        let before = unit.pos;
        let stats = unit.kind.stats();
        if stats.turn_rate > 0 {
            // A landed airframe rests on its tile until an order lifts it.
            if !unit.landed {
                steer_turn_limited(unit, map, stats);
                travel[slot] = unit.pos - before;
            }
            continue;
        }
        if unit.kind.cruise_turn_rate() > 0 {
            cruise::advance(unit, map);
            travel[slot] = unit.pos - before;
            continue;
        }
        if stats.domain == crate::stats::Domain::Ground {
            refused[slot] = ground::advance(
                unit,
                &terrain.with_contact(contacts[slot]),
                &parked[unit.player.0 as usize],
            );
            if unit.drive_speed == Fx::ZERO
                && let Some(aim) = work_aims[slot]
            {
                steer_ground_heading(unit, aim - unit.pos);
            }
            travel[slot] = unit.pos - before;
            continue;
        }
        let mut budget = stats.speed;
        let sky_open = |tile| map.tile(tile).is_some_and(|t| !t.terrain.blocks_air());
        while budget > Fx::ZERO {
            let Some(path) = &mut unit.path else { break };
            let Some(&waypoint) = path.waypoints.get(path.next as usize) else {
                unit.path = None;
                break;
            };
            let center = ground::path_point(path, path.next as usize);
            let dist = unit.pos.dist(center);
            if let Some(&next_wp) = path.waypoints.get(path.next as usize + 1) {
                let next_point = ground::path_point(path, path.next as usize + 1);
                if (dist <= WAYPOINT_ACCEPT
                    || passed_intermediate_waypoint(unit.pos, waypoint, next_wp, stats.radius))
                    && sky_open(TilePos::containing(next_point))
                    && !chassis::path::line_blocked(unit.pos, next_point, sky_open)
                {
                    path.next += 1;
                    continue;
                }
            }
            let next = unit.pos.move_toward(center, budget);
            if !sky_open(TilePos::containing(next))
                || chassis::path::line_blocked(unit.pos, next, sky_open)
            {
                unit.path = None;
                break;
            }
            unit.pos = next;
            if dist <= budget {
                budget -= dist;
                path.next += 1;
                if path.next as usize >= path.waypoints.len() {
                    unit.path = None;
                    break;
                }
            } else {
                break;
            }
        }
        let direction = unit.pos - before;
        if direction != Vec2Fx::ZERO {
            steer_ground_heading(unit, direction);
        }
        travel[slot] = direction;
    }
    (travel, refused)
}

/// Turn-limited flight: the body advances along its heading and only
/// the heading steers, at most `turn_rate` compass steps per tick.
/// Waypoints are accepted inside the kind's turn-acceptance ring — a
/// bounded arc cannot promise an exact center, and a ring tighter than
/// the turn radius is an orbit trap. A committed airframe never stops:
/// without a path it orbits at its full turn rate where it lost the
/// route, and at the world's edge it slides along the boundary while it
/// turns back in. A step into a Peak invalidates the route so the owning
/// brain can plan again from the aircraft's actual position instead of
/// steering into the same mountain forever, while the airframe slides
/// along the face rather than stopping.
fn steer_turn_limited(
    unit: &mut crate::state::Unit,
    map: &Map,
    stats: &'static crate::stats::UnitStats,
) {
    let accept = stats.turn_acceptance();
    let arrive_sq = accept * accept;
    // A landing's final leg is never accepted by the ring: the brain owns
    // touchdown at the tile center.
    let landing = matches!(unit.order, Order::Land { .. });
    // Accept every waypoint the arc has already effectively reached. A
    // terrain-routing waypoint is not disposable merely because the wide
    // acceptance ring overlaps it: only skip it when the direct air segment
    // to the following waypoint is also clear. Otherwise a bomber can erase
    // the one waypoint that would have turned it around a peak, then keep
    // replanning the same impossible shortcut.
    while let Some(path) = &unit.path {
        let Some(&waypoint) = path.waypoints.get(path.next as usize) else {
            unit.path = None;
            break;
        };
        if landing && path.next as usize + 1 == path.waypoints.len() {
            break;
        }
        if unit.pos.dist_sq(waypoint.center()) > arrive_sq {
            break;
        }
        if let Some(&next) = path.waypoints.get(path.next as usize + 1)
            && chassis::path::line_blocked(unit.pos, next.center(), |tile| {
                map.tile(tile)
                    .is_some_and(|map_tile| !map_tile.terrain.blocks_air())
            })
        {
            break;
        }
        let path = unit.path.as_mut().expect("checked above");
        path.next += 1;
        if path.next as usize >= path.waypoints.len() {
            unit.path = None;
            break;
        }
    }
    let radius = stats.turn_radius();
    let target = unit
        .path
        .as_ref()
        .map(|path| path.waypoints[path.next as usize].center());
    let heading_before = unit.heading;
    let straight =
        map.clamp_to_envelope(unit.pos + chassis::compass::dir(unit.heading) * stats.speed);
    let bank_away = |unit: &mut crate::state::Unit| {
        let step = flight::safest_step(map, unit.pos, heading_before, radius);
        unit.heading = heading_before.wrapping_add(step.wrapping_mul(stats.turn_rate));
    };
    if flight::escapable(map, straight, unit.heading, radius) {
        match target {
            Some(target) => steer_toward(unit, map, stats, target),
            // No route: hold the bank and orbit. The circle is tangent to
            // the point where the path ran out, so an idle aircraft stays
            // within one turn diameter of it instead of hanging motionless.
            None => bank_away(unit),
        }
        // A route's turn is planned one leg at a time, and a leg accepted
        // early by the ring can hand the next one a heading its own arc
        // never checked. Whatever the leg wants, a tick that would carry an
        // escapable airframe into a state it cannot fly out of banks the
        // safe way instead.
        let ahead =
            map.clamp_to_envelope(unit.pos + chassis::compass::dir(unit.heading) * stats.speed);
        if flight::escapable(map, unit.pos, heading_before, radius)
            && !flight::escapable(map, ahead, unit.heading, radius)
        {
            bank_away(unit);
        }
    } else {
        // Wall reflex: one more straight tick would leave no arc that stays
        // inside the world, so bank away now, whatever the route wants.
        bank_away(unit);
    }
    let ahead = map.clamp_to_envelope(unit.pos + chassis::compass::dir(unit.heading) * stats.speed);
    let sky_open = |p: Vec2Fx| {
        map.tile(TilePos::containing(p))
            .is_some_and(|t| !t.terrain.blocks_air())
    };
    if sky_open(ahead) {
        unit.pos = ahead;
        return;
    }
    // A Peak face: drop the route so the brain replans from here, but keep
    // the airframe moving by sliding along the face on whichever axis is
    // still open, the same way the envelope slides it along the world's edge.
    unit.path = None;
    for slid in [
        Vec2Fx::new(ahead.x, unit.pos.y),
        Vec2Fx::new(unit.pos.x, ahead.y),
    ] {
        if slid != unit.pos && sky_open(slid) {
            unit.pos = slid;
            return;
        }
    }
}

/// Rotates the heading toward `target` by at most `turn_rate` compass
/// steps, settling on the nearest bearing with a small angular deadband. Every
/// input is Q32.32, so each platform turns identically. The turn is a
/// committed arc, not a nudge: the shorter rotation is taken only when the
/// arc it sweeps stays inside the world and ends somewhere the airframe can
/// still be flown out of; otherwise the long way round is taken when that
/// one qualifies, and the shorter arc only as a last resort.
fn steer_toward(
    unit: &mut crate::state::Unit,
    map: &Map,
    stats: &'static crate::stats::UnitStats,
    target: Vec2Fx,
) {
    let d = target - unit.pos;
    let facing = chassis::compass::dir(unit.heading);
    let cross = facing.x * d.y - facing.y * d.x;
    let dot = facing.x * d.x + facing.y * d.y;
    // Three quarters of a compass step: retain the current bearing near
    // the quantization boundary instead of reversing on successive ticks.
    if dot >= Fx::ZERO && cross.abs() <= dot * const { Fx::lit("0.0184") } {
        return;
    }
    let Some((short, sweep)) = flight::turn_to(unit.heading, d) else {
        return;
    };
    let radius = stats.turn_radius();
    let long = flight::reverse(short);
    let qualifies = |step: u8, sweep: u16| {
        flight::arc_fits(map, unit.pos, unit.heading, step, sweep, radius) && {
            let (end, end_heading) = flight::arc_end(unit.pos, unit.heading, step, sweep, radius);
            flight::escapable(map, end, end_heading, radius)
        }
    };
    let step = if qualifies(short, sweep) {
        short
    } else if qualifies(long, flight::FULL_TURN - sweep) {
        long
    } else if flight::arc_fits(map, unit.pos, unit.heading, short, sweep, radius) {
        short
    } else if flight::arc_fits(
        map,
        unit.pos,
        unit.heading,
        long,
        flight::FULL_TURN - sweep,
        radius,
    ) {
        long
    } else {
        short
    };
    for _ in 0..stats.turn_rate {
        let hv = chassis::compass::dir(unit.heading);
        let cross = hv.x * d.y - hv.y * d.x;
        let dot = hv.x * d.x + hv.y * d.y;
        if cross == Fx::ZERO && dot >= Fx::ZERO {
            break;
        }
        let next = unit.heading.wrapping_add(step);
        let nhv = chassis::compass::dir(next);
        let ncross = nhv.x * d.y - nhv.y * d.x;
        let ndot = nhv.x * d.x + nhv.y * d.y;
        // The sign of the cross product also flips when the nose sweeps
        // through dead astern on the long way round; only a crossing
        // with the target ahead is the goal ray.
        if ndot >= Fx::ZERO && (cross > Fx::ZERO) != (ncross > Fx::ZERO) {
            if ndot > dot {
                unit.heading = next;
            }
            break;
        }
        unit.heading = next;
    }
}

/// Ground bodies stop at terrain, but nothing but the world's edge bounds
/// the sky: an air push is clamped to the flight envelope so no correction
/// can carry an aircraft past the boundary its own steering respects.
fn envelope_bound(state: &State, domain: crate::stats::Domain, to: Vec2Fx) -> Vec2Fx {
    match domain {
        crate::stats::Domain::Air => state.map.clamp_to_envelope(to),
        crate::stats::Domain::Ground => to,
    }
}

fn contact_push_open(state: &State, slot: usize, to: Vec2Fx) -> bool {
    let unit = &state.units[slot];
    if let Some(surface) = super::brain::contact::surface_for(state, unit) {
        return state
            .ground_terrain()
            .with_contact(Some(surface))
            .contact_clear(unit.pos, to, super::brain::contact::collision_radius(unit));
    }
    collision_position_open(state, unit.domain(), to)
}

fn collision_position_open(state: &State, domain: crate::stats::Domain, pos: Vec2Fx) -> bool {
    let tile = TilePos::containing(pos);
    // An exact edge touches both cells (a corner touches four). Testing only
    // the containing cell admits one face of an obstacle but rejects its mirror.
    let edge_x = i32::from(pos.x.frac() == Fx::ZERO);
    let edge_y = i32::from(pos.y.frac() == Fx::ZERO);
    (-edge_y..=0).all(|dy| (-edge_x..=0).all(|dx| state.passable_for(domain, tile.offset(dx, dy))))
}

/// A unit that is standing still to work — extracting, welding, or
/// holding fire on a target — resists shoving; movers yield around it.
fn is_anchored(unit: &crate::state::Unit) -> bool {
    unit.landed
        || unit.kind.stats().turn_rate == 0
            && unit.path.is_none()
            && unit.drive_speed == Fx::ZERO
            && matches!(
                unit.order,
                Order::Harvest { .. }
                    | Order::Attack { .. }
                    | Order::Repair { .. }
                    | Order::Build { .. }
                    | Order::Salvage { .. }
                    | Order::RepairUnit { .. }
                    | Order::ReturnCargo { .. }
            )
}

/// Unit directions for perfectly stacked pairs, indexed by owner-local rank
/// xor and then oriented in the stack's map-relative half-turn frame.
const STACKED_DIRS: [Vec2Fx; 8] = [
    Vec2Fx::new(Fx::lit("1"), Fx::lit("0")),
    Vec2Fx::new(Fx::lit("0.7071"), Fx::lit("0.7071")),
    Vec2Fx::new(Fx::lit("0"), Fx::lit("1")),
    Vec2Fx::new(Fx::lit("-0.7071"), Fx::lit("0.7071")),
    Vec2Fx::new(Fx::lit("-1"), Fx::lit("0")),
    Vec2Fx::new(Fx::lit("-0.7071"), Fx::lit("-0.7071")),
    Vec2Fx::new(Fx::lit("0"), Fx::lit("-1")),
    Vec2Fx::new(Fx::lit("0.7071"), Fx::lit("-0.7071")),
];

pub(super) fn uses_rotated_map_frame(state: &State, pos: Vec2Fx) -> bool {
    let twice_x = pos.x + pos.x;
    let twice_y = pos.y + pos.y;
    let map_width = Fx::from_num(state.map.width());
    let map_height = Fx::from_num(state.map.height());
    twice_y > map_height || (twice_y == map_height && twice_x > map_width)
}

fn stacked_direction(state: &State, pos: Vec2Fx, rank_i: usize, rank_j: usize) -> Vec2Fx {
    let rotated_half = uses_rotated_map_frame(state, pos);
    let frame_offset = if rotated_half { 4 } else { 0 };
    STACKED_DIRS[((rank_i ^ rank_j) + frame_offset) % STACKED_DIRS.len()]
}

fn owner_local_ranks(state: &State) -> Vec<usize> {
    let mut next = vec![0; state.players.len()];
    state
        .units
        .iter()
        .map(|unit| {
            let owner = unit.player.0 as usize;
            let rank = next[owner];
            next[owner] += 1;
            rank
        })
        .collect()
}

/// Resolves unit-unit collisions: several deterministic relaxation passes
/// push overlapping pairs apart, each body taking the share of the overlap
/// its partner's footprint area earns it, so units cannot stack — grouped
/// movers fan out and a body-blocked unit stays blocked. A
/// push that would land in an impassable tile is discarded (rocks beat
/// crowd pressure), and one per-unit budget spans every pass in the tick
/// so packed crowds settle instead of exploding.
///
/// `travel` is each unit's displacement from this tick's path
/// following: a unit that actually TRAVELED into a contact takes its
/// correction as a slide (see [`correction_dirs`]) instead of a pure
/// radial push. Snapshotted once for all passes — corrections can
/// stale it by at most one step, which only softens the slide.
pub(super) fn resolve_collisions(
    state: &mut State,
    travel: &[Vec2Fx],
    index: &mut super::spatial::UnitIndex,
) {
    // Direction alternates by tick parity — Gauss-Seidel's sequential
    // application must not always favor the same ids (see brain::run).
    let reversed = state.tick % 2 == 1;
    let owner_ranks = owner_local_ranks(state);
    let Some(pairs) = collision_pairs(state, reversed, index, &owner_ranks) else {
        return;
    };
    let mut spent = vec![Fx::ZERO; state.units.len()];
    for _ in 0..COLLISION_ITERATIONS {
        if !relaxation_pass(state, travel, &owner_ranks, &mut spent, &pairs) {
            break;
        }
    }
}

/// Correction candidates for one body of an overlapping pair, best
/// first. `away` is its radial escape (unit length). A body that
/// traveled INTO the contact slides: the correction blends a reduced
/// radial share with a lateral share. For a head-on pair the caller derives
/// one body's candidates by exact negation of the other's, producing stable
/// opposite world sides. Other contacts pick the side toward the body's own
/// travel. Both rules are geometric and 180-degree rotation-equivariant, so
/// mirror seats slide mirror ways. Parked and non-closing bodies keep the
/// pure radial push.
///
/// A slide candidate the terrain rejects degrades in order: against a
/// head-on partner the lateral is DROPPED, never reversed — the
/// opposite side is the partner's side, and taking it walls a
/// corridor pair back into the freeze as a wobble. Against anything
/// else the opposite side gets one try before the radial fallback.
fn correction_dirs(away: Vec2Fx, travel: Vec2Fx, partner_head_on: bool) -> [Option<Vec2Fx>; 3] {
    let closing = travel.x * away.x + travel.y * away.y < Fx::ZERO;
    if !closing {
        return [Some(away), None, None];
    }
    let perp = Vec2Fx::new(-away.y, away.x);
    let side = if partner_head_on {
        perp
    } else {
        let lat = travel.x * perp.x + travel.y * perp.y;
        if lat >= Fx::ZERO { perp } else { -perp }
    };
    let blended = away * SLIDE_RADIAL_SHARE + side * SLIDE_LATERAL_SHARE;
    if partner_head_on {
        [Some(blended), Some(away), None]
    } else {
        let flipped = away * SLIDE_RADIAL_SHARE - side * SLIDE_LATERAL_SHARE;
        [Some(blended), Some(flipped), Some(away)]
    }
}

/// One spatial row in a body's half-turn-oriented frame: x groups reverse on
/// the rotated half of the map, while canonical slot order within one tile is
/// preserved. Reversing the entire row would also reverse coincident bodies
/// and give mirrored dense crowds a different Gauss-Seidel contact order.
struct OrientedRow<'a> {
    row: &'a [(TilePos, usize)],
    rotated: bool,
    next: usize,
    group_start: usize,
    group_end: usize,
}

impl<'a> OrientedRow<'a> {
    fn new(row: &'a [(TilePos, usize)], rotated: bool) -> Self {
        let mut oriented = Self {
            row,
            rotated,
            next: 0,
            group_start: row.len(),
            group_end: row.len(),
        };
        if rotated {
            oriented.open_previous_group();
        }
        oriented
    }

    fn open_previous_group(&mut self) {
        self.group_end = self.group_start;
        if self.group_end == 0 {
            return;
        }
        let x = self.row[self.group_end - 1].0.x;
        self.group_start = self.group_end - 1;
        while self.group_start > 0 && self.row[self.group_start - 1].0.x == x {
            self.group_start -= 1;
        }
        self.next = self.group_start;
    }
}

impl Iterator for OrientedRow<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.rotated {
            let (_, slot) = *self.row.get(self.next)?;
            self.next += 1;
            return Some(slot);
        }
        if self.next == self.group_end {
            if self.group_start == 0 {
                return None;
            }
            self.open_previous_group();
        }
        let (_, slot) = *self.row.get(self.next)?;
        self.next += 1;
        Some(slot)
    }
}

/// The owner-rank part of a pair's resolution key. Ranks decide almost every
/// comparison, so pairs sort on them first and on
/// [`collision_pair_position`] only within a run of equal ranks.
fn collision_pair_ranks(
    state: &State,
    owner_ranks: &[usize],
    i: usize,
    j: usize,
) -> (usize, usize, bool) {
    let (low, high) = if owner_ranks[i] <= owner_ranks[j] {
        (owner_ranks[i], owner_ranks[j])
    } else {
        (owner_ranks[j], owner_ranks[i])
    };
    (low, high, state.units[i].player == state.units[j].player)
}

/// The geometric part of a pair's resolution key: the pair's canonical
/// coordinates in whichever half-turn frame orders first.
fn collision_pair_position(state: &State, i: usize, j: usize) -> (Vec2Fx, Vec2Fx) {
    let ordered = |a: Vec2Fx, b: Vec2Fx| if a <= b { (a, b) } else { (b, a) };
    let world = ordered(state.units[i].pos, state.units[j].pos);
    let center_twice = Vec2Fx::new(
        Fx::from_num(state.map.width()),
        Fx::from_num(state.map.height()),
    );
    let rotated = ordered(
        center_twice - state.units[i].pos,
        center_twice - state.units[j].pos,
    );
    world.min(rotated)
}

/// Whether a candidate pair can reach the overlap test during this tick's
/// passes. Bodies of different layers never do. A ground body moves no
/// farther than [`COLLISION_MAX_STEP`] in a tick, so a ground pair farther
/// apart than its full spacing plus both bodies' travel never does either.
/// Air corrections clamp to the flight envelope, which can carry a body
/// farther, so air pairs always stay.
fn may_touch_this_tick(a: &Body, b: &Body) -> bool {
    if a.domain != b.domain {
        return false;
    }
    if a.domain == crate::stats::Domain::Air {
        return true;
    }
    let reach = a.radius + b.radius + COLLISION_MAX_STEP * 2 + const { Fx::lit("0.015625") };
    a.pos.dist_sq(b.pos) < reach * reach
}

/// Whether a pair overlaps beyond [`COLLISION_SLOP`] right now.
fn pressing(a: &Body, b: &Body) -> bool {
    let rest = a.radius + b.radius - COLLISION_SLOP;
    a.domain == b.domain && a.pos.dist_sq(b.pos) < rest * rest
}

/// What candidate gathering reads of one body, copied once per tick so the
/// scan walks a small array instead of the whole unit table.
#[derive(Clone, Copy)]
struct Body {
    pos: Vec2Fx,
    radius: Fx,
    domain: crate::stats::Domain,
    alive: bool,
    /// A heading-first airframe flies a committed arc that its steering has
    /// already checked against the world; a shove would carry it faster than
    /// its speed and off that arc, so such aircraft neither push nor yield.
    shoveable: bool,
}

/// Candidate contacts in a seat-local order. A half-turn maps each pair to a
/// pair with the same owner-local ranks and canonical geometry. Counterpart
/// pairs therefore remain adjacent; they touch disjoint units and commute,
/// while the orbit order around a crowded crossing is identical for both
/// seats. Raw unit ids cannot provide that property because corresponding
/// seats receive adjacent, not mirrored, global ids.
///
/// Gathered and ordered once per tick from the tick's starting positions;
/// every pass walks the same list. Interaction reach is under one tile (radii
/// sum < 1), so 3x3 tile neighborhoods suffice, and a pair that only comes
/// into one through this tick's corrections waits for the next tick. `None`
/// means no pair overlaps beyond [`COLLISION_SLOP`], so no pass could move
/// anything and nothing is sorted; likewise only groups of linked pairs with
/// such an overlap are kept.
fn collision_pairs(
    state: &State,
    reversed: bool,
    index: &mut super::spatial::UnitIndex,
    owner_ranks: &[usize],
) -> Option<Vec<(usize, usize)>> {
    index.rebuild(&state.units);
    let mut pairs = Vec::new();
    let mut pressed = Vec::new();
    let bodies: Vec<Body> = state
        .units
        .iter()
        .map(|unit| Body {
            pos: unit.pos,
            radius: unit.kind.stats().radius,
            domain: unit.domain(),
            alive: unit.hp > 0,
            shoveable: unit.kind.stats().turn_rate == 0 || unit.landed,
        })
        .collect();
    for (i, body) in bodies.iter().enumerate() {
        if !body.alive || !body.shoveable {
            continue;
        }
        let home = TilePos::containing(body.pos);
        let rotated_frame = uses_rotated_map_frame(state, body.pos);
        for row_offset in 0..3 {
            let dy = if rotated_frame {
                1 - row_offset
            } else {
                row_offset - 1
            };
            let row = index.row_span(home.y + dy, home.x - 1, home.x + 1);
            for j in OrientedRow::new(row, rotated_frame) {
                if j > i && bodies[j].shoveable && may_touch_this_tick(body, &bodies[j]) {
                    if pressing(body, &bodies[j]) {
                        pressed.push(i);
                    }
                    pairs.push((i, j));
                }
            }
        }
    }
    if pressed.is_empty() {
        return None;
    }
    // Pairs sharing a unit form groups. A group with no pressing pair moves
    // nothing this tick: only an overlap beyond the slop corrects, a
    // correction moves only that pair's units, and two units in different
    // groups were never within reach of each other. Such groups are dropped
    // before the sort; the order of every pair that remains is unchanged.
    // When most pairs press, as in a melee, little would be dropped, so the
    // grouping is skipped.
    if pressed.len() * 4 >= pairs.len() {
        return Some(sort_collision_pairs(state, owner_ranks, reversed, pairs));
    }
    let mut group: Vec<usize> = (0..bodies.len()).collect();
    fn root(group: &mut [usize], mut i: usize) -> usize {
        while group[i] != i {
            group[i] = group[group[i]];
            i = group[i];
        }
        i
    }
    for &(i, j) in &pairs {
        let (a, b) = (root(&mut group, i), root(&mut group, j));
        group[a.max(b)] = a.min(b);
    }
    let mut moving = vec![false; bodies.len()];
    for i in pressed {
        let r = root(&mut group, i);
        moving[r] = true;
    }
    pairs.retain(|&(i, _)| moving[root(&mut group, i)]);
    Some(sort_collision_pairs(state, owner_ranks, reversed, pairs))
}

fn sort_collision_pairs(
    state: &State,
    owner_ranks: &[usize],
    reversed: bool,
    mut pairs: Vec<(usize, usize)>,
) -> Vec<(usize, usize)> {
    pairs.sort_by_cached_key(|&(i, j)| {
        (
            collision_pair_ranks(state, owner_ranks, i, j),
            collision_pair_position(state, i, j),
        )
    });
    if reversed {
        pairs.reverse();
    }
    pairs
}

/// One pass over the tick's candidate pairs; returns whether any pair
/// overlapped beyond [`COLLISION_SLOP`].
///
/// Corrections apply *immediately*, pair by pair, in deterministic order
/// (Gauss–Seidel, not Jacobi). Accumulating all pushes first looks tidier
/// but admits frozen equilibria: symmetric arrangements — several full
/// harvesters magnetized to one doorstep — cancel to exactly zero net
/// correction while everything still overlaps, and the bot economy stalls
/// forever. Sequential application cannot cancel, so jams always evolve.
/// Dead units are skipped: a corpse should not shove the living on its
/// removal tick.
fn relaxation_pass(
    state: &mut State,
    travel: &[Vec2Fx],
    owner_ranks: &[usize],
    spent: &mut Vec<Fx>,
    pairs: &[(usize, usize)],
) -> bool {
    let n = state.units.len();
    let mut any_overlap = false;
    // One per-unit displacement budget spans all relaxation passes in a
    // tick. Clamping only per pair lets a unit in k overlaps move k × the
    // cap, while resetting here lets it move one cap per pass; both made
    // dense stacks visibly explode outward. Direct unit tests may call one
    // pass with a fresh buffer, so initialize only when its shape differs.
    if spent.len() != n {
        spent.clear();
        spent.resize(n, Fx::ZERO);
    }
    for &(i, j) in pairs {
        let (pos_i, radius_i, dom_i) = {
            let u = &state.units[i];
            (u.pos, u.kind.stats().radius, u.domain())
        };
        let (pos_j, radius_j, dom_j) = {
            let u = &state.units[j];
            (u.pos, u.kind.stats().radius, u.domain())
        };
        // Bodies only collide within their own layer: a flyer
        // and a crawler occupy the same tile without touching.
        if dom_i != dom_j {
            continue;
        }
        let delta = pos_j - pos_i;
        let dist_sq = delta.length_sq();
        let full_spacing = radius_i + radius_j;
        if dist_sq >= full_spacing * full_spacing {
            continue;
        }
        let productive_i = super::crowding::productive(state, &state.units[i]);
        let productive_j = super::crowding::productive(state, &state.units[j]);
        let min_dist = if productive_i || productive_j {
            super::crowding::spacing(&state.units[i], &state.units[j])
        } else {
            full_spacing
        };
        let rest = min_dist - COLLISION_SLOP;
        if dist_sq >= rest * rest {
            continue;
        }
        any_overlap = true;
        let dist = sqrt(dist_sq);
        // Perfectly stacked pairs keep the fixed-direction
        // radial split — there is no geometry to slide on.
        let (dir, overlap, stacked) = if dist == Fx::ZERO {
            (
                stacked_direction(state, pos_i, owner_ranks[i], owner_ranks[j]),
                min_dist,
                true,
            )
        } else {
            (delta / dist, min_dist - dist, false)
        };
        // Anchored units (working in place) yield a sliver;
        // movers absorb the correction and flow around them.
        // A landed airframe is a fixture on its tile center: it takes no
        // correction at all, so the whole overlap falls on the mover.
        let (share_i, share_j) = match (state.units[i].landed, state.units[j].landed) {
            (true, true) => (Fx::ZERO, Fx::ZERO),
            (true, false) => (Fx::ZERO, Fx::ONE),
            (false, true) => (Fx::ONE, Fx::ZERO),
            (false, false) => match (
                is_anchored(&state.units[i]) && productive_i,
                is_anchored(&state.units[j]) && productive_j,
            ) {
                (true, false) => (Fx::ZERO, Fx::ONE),
                (false, true) => (Fx::ONE, Fx::ZERO),
                // Footprint area stands in for mass, so a heavy hull gives
                // less ground than the light one shoving it. Equal radii
                // still split exactly in half. The division rounds, so it is
                // always taken for the lighter body: keyed to pair order, a
                // seat's mirror image would receive the other rounding.
                _ => {
                    let (mass_i, mass_j) = (radius_i * radius_i, radius_j * radius_j);
                    let light_share = mass_i.max(mass_j) / (mass_i + mass_j);
                    if mass_i <= mass_j {
                        (light_share, Fx::ONE - light_share)
                    } else {
                        (Fx::ONE - light_share, light_share)
                    }
                }
            },
        };
        let (away_i, away_j) = (-dir, dir);
        let closing_i = travel[i].x * away_i.x + travel[i].y * away_i.y < Fx::ZERO;
        let closing_j = travel[j].x * away_j.x + travel[j].y * away_j.y < Fx::ZERO;
        let dirs_j = if stacked {
            [Some(away_j), None, None]
        } else {
            correction_dirs(away_j, travel[j], closing_i)
        };
        let dirs_i = if stacked {
            [Some(away_i), None, None]
        } else if closing_i && closing_j {
            dirs_j.map(|direction| direction.map(|direction| -direction))
        } else {
            correction_dirs(away_i, travel[i], closing_j)
        };
        let step_j = (overlap * share_j).min(COLLISION_MAX_STEP - spent[j]);
        if step_j > Fx::ZERO {
            for cand in dirs_j.into_iter().flatten() {
                let to = envelope_bound(state, dom_j, pos_j + cand * step_j);
                if contact_push_open(state, j, to) {
                    state.units[j].pos = to;
                    spent[j] += step_j;
                    break;
                }
            }
        }
        let step_i = (overlap * share_i).min(COLLISION_MAX_STEP - spent[i]);
        if step_i > Fx::ZERO {
            for cand in dirs_i.into_iter().flatten() {
                let to = envelope_bound(state, dom_i, pos_i + cand * step_i);
                if contact_push_open(state, i, to) {
                    state.units[i].pos = to;
                    spent[i] += step_i;
                    break;
                }
            }
        }
    }
    any_overlap
}

#[cfg(test)]
mod tests {
    use super::super::spatial::UnitIndex;
    use super::*;
    use crate::scenario::{PlayerSpec, Scenario, UnitSpec};
    use crate::state::Faction;
    use crate::stats::UnitKind;

    /// A resting row of bodies has no pair to correct and is dropped; a
    /// touching neighbor of a pressing pair stays, since the pair's push can
    /// reach it.
    #[test]
    fn only_groups_with_a_pressing_pair_are_kept() {
        let mut state = Scenario::skirmish().build().expect("skirmish builds");
        state.units.clear();
        let spacing = UnitKind::Harvester.stats().radius * 2;
        let step = |from: Vec2Fx, gap: &str| from + Vec2Fx::new(spacing + Fx::lit(gap), Fx::ZERO);
        let mut resting = TilePos::new(6, 5).center();
        for _ in 0..4 {
            state.spawn_unit(crate::PlayerId(0), UnitKind::Harvester, resting);
            resting = step(resting, "0.1");
        }
        let first = TilePos::new(6, 12).center();
        let second = step(first, "-0.1");
        let third = step(second, "0.1");
        let chain = [first, second, third]
            .map(|pos| state.spawn_unit(crate::PlayerId(0), UnitKind::Harvester, pos));
        let slot = |id| state.units.iter().position(|u| u.id == id).unwrap();
        let ranks = owner_local_ranks(&state);
        let pairs = collision_pairs(&state, false, &mut UnitIndex::new(), &ranks)
            .expect("the chain presses");
        let mut kept: Vec<_> = pairs.into_iter().collect();
        kept.sort_unstable();
        let mut expected = vec![
            (slot(chain[0]), slot(chain[1])),
            (slot(chain[1]), slot(chain[2])),
        ];
        expected.sort_unstable();
        assert_eq!(kept, expected);
    }

    #[test]
    fn contact_that_cancels_progress_drops_the_route_after_the_stall_bound() {
        for precise in [false, true] {
            let mut state = Scenario::skirmish().build().unwrap();
            let waypoint = TilePos::new(20, 12);
            let slot = 0;
            state.units[slot].path = Some(PathFollow {
                final_point: precise.then(|| Vec2Fx::new(Fx::lit("20.8"), Fx::lit("12.5"))),
                goal: waypoint,
                waypoints: vec![waypoint],
                next: 0,
            });
            state.units[slot].pos = if precise {
                Vec2Fx::new(Fx::lit("20.7"), Fx::lit("12.5"))
            } else {
                TilePos::new(10, 12).center()
            };
            let travel: Vec<Vec2Fx> = (0..state.units.len())
                .map(|i| {
                    if i == slot {
                        Vec2Fx::new(Fx::lit("0.1"), Fx::ZERO)
                    } else {
                        Vec2Fx::ZERO
                    }
                })
                .collect();
            // Propulsion carried the body east; contact shoved it back to where
            // it started, cancelling the whole step.
            let refused = vec![false; state.units.len()];
            let driven: Vec<Vec2Fx> = state
                .units
                .iter()
                .zip(&travel)
                .map(|(unit, &step)| unit.pos + step)
                .collect();
            for tick in 1..crate::stats::STALL_REPLAN_TICKS {
                note_stalls(&mut state, &travel, &refused, &driven);
                assert_eq!(state.units[slot].stall_ticks, tick);
                assert!(state.units[slot].path.is_some());
            }
            // One tick of real progress clears the count.
            state.units[slot].pos += travel[slot];
            note_stalls(&mut state, &travel, &refused, &driven);
            assert_eq!(state.units[slot].stall_ticks, 0);
            assert!(state.units[slot].path.is_some());
            state.units[slot].pos -= travel[slot];
            for _ in 1..crate::stats::STALL_REPLAN_TICKS {
                note_stalls(&mut state, &travel, &refused, &driven);
            }
            assert!(state.units[slot].path.is_some());
            note_stalls(&mut state, &travel, &refused, &driven);
            assert!(
                state.units[slot].path.is_none(),
                "the stalled route was kept"
            );
            assert_eq!(state.units[slot].stall_ticks, 0);
        }
    }

    /// A refused step moves the body nowhere, so no deflection measures it;
    /// it still counts toward the bound, while a tick spent pivoting in
    /// place does not.
    #[test]
    fn refused_steps_drop_the_route_after_the_stall_bound() {
        let mut state = Scenario::skirmish().build().unwrap();
        let waypoint = TilePos::new(20, 12);
        let slot = 0;
        state.units[slot].path = Some(PathFollow {
            final_point: None,
            goal: waypoint,
            waypoints: vec![waypoint],
            next: 0,
        });
        let travel = vec![Vec2Fx::ZERO; state.units.len()];
        let driven: Vec<Vec2Fx> = state.units.iter().map(|unit| unit.pos).collect();
        let pivoting = vec![false; state.units.len()];
        let mut refused = pivoting.clone();
        refused[slot] = true;
        note_stalls(&mut state, &travel, &pivoting, &driven);
        assert_eq!(state.units[slot].stall_ticks, 0);
        for tick in 1..crate::stats::STALL_REPLAN_TICKS {
            note_stalls(&mut state, &travel, &refused, &driven);
            assert_eq!(state.units[slot].stall_ticks, tick);
        }
        note_stalls(&mut state, &travel, &pivoting, &driven);
        assert_eq!(state.units[slot].stall_ticks, 0);
        for _ in 1..crate::stats::STALL_REPLAN_TICKS {
            note_stalls(&mut state, &travel, &refused, &driven);
        }
        assert!(state.units[slot].path.is_some());
        note_stalls(&mut state, &travel, &refused, &driven);
        assert!(
            state.units[slot].path.is_none(),
            "the refused route was kept"
        );
        assert_eq!(state.units[slot].stall_ticks, 0);
    }

    #[test]
    fn a_route_dropped_after_stalls_were_noted_takes_its_count_with_it() {
        let mut state = Scenario::skirmish().build().unwrap();
        let waypoint = TilePos::new(20, 12);
        let slot = 0;
        state.units[slot].path = Some(PathFollow {
            final_point: None,
            goal: waypoint,
            waypoints: vec![waypoint],
            next: 0,
        });
        let mut travel = vec![Vec2Fx::ZERO; state.units.len()];
        travel[slot] = Vec2Fx::new(Fx::lit("0.1"), Fx::ZERO);
        let refused = vec![false; state.units.len()];
        let driven: Vec<Vec2Fx> = state
            .units
            .iter()
            .zip(&travel)
            .map(|(unit, &step)| unit.pos + step)
            .collect();
        note_stalls(&mut state, &travel, &refused, &driven);
        assert_eq!(state.units[slot].stall_ticks, 1);
        state
            .validate_invariants()
            .expect("a stalled walker is valid");

        // A late phase, such as revealing the site a builder is walking to,
        // retargets the unit after the stalls were noted.
        state.units[slot].path = None;
        assert!(matches!(
            state.validate_invariants(),
            Err(crate::state::StateIntegrityError::InvalidStallTicks(_))
        ));
        forget_stalls_without_routes(&mut state);
        assert_eq!(state.units[slot].stall_ticks, 0);
        state
            .validate_invariants()
            .expect("the count left with the route");
    }

    #[test]
    fn footprint_escape_routes_rotate_with_the_body() {
        let state = Scenario::skirmish().build().unwrap();
        let mirror = |tile: TilePos| {
            TilePos::new(
                state.map.width() - 1 - tile.x,
                state.map.height() - 1 - tile.y,
            )
        };
        for from in [
            TilePos::new(4, 4),
            TilePos::new(5, 4),
            TilePos::new(4, 5),
            TilePos::new(5, 5),
        ] {
            assert!(!state.passable(from));
            assert!(!state.passable(mirror(from)));
            for heading in 0..128 {
                let a = escape_route(&state, UnitKind::Harvester, from, heading).unwrap();
                let b =
                    escape_route(&state, UnitKind::Harvester, mirror(from), heading + 128).unwrap();
                assert_eq!(b.goal, mirror(a.goal));
                assert_eq!(
                    b.waypoints,
                    a.waypoints.into_iter().map(mirror).collect::<Vec<_>>()
                );
            }
        }
    }

    #[test]
    fn mirrored_collision_pushes_agree_at_rock_faces_and_corners() {
        let radius = UnitKind::Sentinel.stats().radius;
        let separation = Fx::lit("0.5");
        let step = ((radius + radius - separation) * chassis::fx::HALF).min(COLLISION_MAX_STEP);
        let extent = Vec2Fx::new(Fx::from_num(40), Fx::from_num(24));
        for (boundary, inward) in [
            (
                Vec2Fx::new(Fx::lit("12.5"), Fx::from_num(8)),
                Vec2Fx::new(Fx::ZERO, Fx::ONE),
            ),
            (
                Vec2Fx::new(Fx::from_num(13), Fx::lit("7.5")),
                Vec2Fx::new(Fx::ONE, Fx::ZERO),
            ),
            (
                Vec2Fx::new(Fx::from_num(13), Fx::from_num(8)),
                Vec2Fx::new(Fx::ZERO, Fx::ONE),
            ),
        ] {
            for offset in [-Fx::DELTA, Fx::ZERO, Fx::DELTA] {
                let mut scenario = Scenario::skirmish();
                let mut rows = vec![vec!['.'; 40]; 24];
                for (x, y, tile) in [(4, 4, '1'), (34, 18, '2'), (12, 7, '#'), (27, 16, '#')] {
                    rows[y][x] = tile;
                }
                scenario.map = rows
                    .into_iter()
                    .map(|row| row.into_iter().collect())
                    .collect();
                scenario.units = (0..4)
                    .map(|i| UnitSpec {
                        player: i / 2,
                        kind: UnitKind::Sentinel,
                        x: 15 + i32::from(i),
                        y: 10,
                    })
                    .collect();
                let mut state = scenario.build().unwrap();
                let first = boundary + inward * (step + offset);
                let second = first + inward * separation;
                for (unit, position) in
                    state
                        .units
                        .iter_mut()
                        .zip([first, second, extent - first, extent - second])
                {
                    unit.pos = position;
                }
                let ranks = owner_local_ranks(&state);
                let mut index = UnitIndex::new();
                let pairs = collision_pairs(&state, false, &mut index, &ranks).unwrap_or_default();
                relaxation_pass(&mut state, &[Vec2Fx::ZERO; 4], &ranks, &mut vec![], &pairs);
                for (a, b) in [(0, 2), (1, 3)] {
                    assert_eq!(
                        state.units[b].pos,
                        extent - state.units[a].pos,
                        "boundary={boundary:?}, offset={offset}"
                    );
                }
            }
        }
    }

    #[test]
    fn collision_boundaries_respect_domain_and_all_touching_obstacles() {
        use crate::stats::Domain;
        for terrain in ['#', '^', 's'] {
            let mut scenario = Scenario::skirmish();
            let mut rows = vec![vec!['.'; 40]; 24];
            rows[4][4] = '1';
            rows[18][34] = '2';
            rows[7][12] = terrain;
            rows[16][27] = terrain;
            scenario.map = rows
                .into_iter()
                .map(|row| row.into_iter().collect())
                .collect();
            scenario.units.clear();
            let state = scenario.build().unwrap();
            let extent = Vec2Fx::new(Fx::from_num(40), Fx::from_num(24));
            for (edge, direction) in [
                (
                    Vec2Fx::new(Fx::lit("12.5"), Fx::from_num(8)),
                    Vec2Fx::new(Fx::ZERO, Fx::ONE),
                ),
                (
                    Vec2Fx::new(Fx::from_num(13), Fx::lit("7.5")),
                    Vec2Fx::new(Fx::ONE, Fx::ZERO),
                ),
                (
                    Vec2Fx::new(Fx::from_num(13), Fx::from_num(8)),
                    Vec2Fx::new(Fx::ONE, Fx::ONE),
                ),
            ] {
                for delta in [-Fx::DELTA, Fx::ZERO, Fx::DELTA] {
                    let pos = edge + direction * delta;
                    for domain in [Domain::Ground, Domain::Air] {
                        let expected =
                            delta > Fx::ZERO || (domain == Domain::Air && terrain != '^');
                        assert_eq!(collision_position_open(&state, domain, pos), expected);
                        assert_eq!(
                            collision_position_open(&state, domain, extent - pos),
                            expected
                        );
                    }
                }
            }
            for pos in [
                Vec2Fx::new(Fx::from_num(6), Fx::from_num(5)),
                Vec2Fx::new(Fx::from_num(6), Fx::from_num(6)),
            ] {
                assert!(!collision_position_open(&state, Domain::Ground, pos));
                assert!(!collision_position_open(
                    &state,
                    Domain::Ground,
                    extent - pos
                ));
                assert!(collision_position_open(&state, Domain::Air, pos));
                assert!(collision_position_open(&state, Domain::Air, extent - pos));
            }
            assert!(collision_position_open(
                &state,
                Domain::Ground,
                TilePos::new(20, 12).center()
            ));
            assert!(collision_position_open(
                &state,
                Domain::Ground,
                Vec2Fx::new(Fx::from_num(20), Fx::from_num(12))
            ));
        }
    }

    #[test]
    fn shallow_air_bearing_does_not_alternate_across_the_goal_ray() {
        let map = Map::parse(&vec![".".repeat(200); 200]).unwrap().0;
        for heading in [0u8, 64, 128, 192] {
            let mut unit = boundary_pair().units[0].clone();
            unit.kind = UnitKind::Condor;
            unit.heading = heading;
            unit.pos = Vec2Fx::new(Fx::from_num(100), Fx::from_num(100));
            let forward = chassis::compass::dir(heading);
            let sideways = chassis::compass::dir(heading.wrapping_add(64));
            let target = unit.pos + forward * Fx::from_num(80) + sideways;
            for _ in 0..200 {
                steer_toward(&mut unit, &map, UnitKind::Condor.stats(), target);
                assert_eq!(
                    unit.heading, heading,
                    "straight approach must not wag its nose"
                );
                unit.pos += chassis::compass::dir(unit.heading) * unit.kind.stats().speed;
            }
            let target = unit.pos + sideways * Fx::from_num(20);
            steer_toward(&mut unit, &map, UnitKind::Condor.stats(), target);
            assert_eq!(
                unit.heading,
                heading.wrapping_add(unit.kind.stats().turn_rate)
            );
        }
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

    fn boundary_pair() -> State {
        Scenario {
            mode: Default::default(),
            name: "boundary-pair".into(),
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
        .expect("boundary pair builds")
    }

    #[test]
    fn coasting_worker_is_not_anchored_until_its_motor_stops() {
        let state = boundary_pair();
        let terrain = state.ground_terrain();
        let parked = ParkedBodies::default();
        let mut unit = state.units[0].clone();
        let unit = &mut unit;
        unit.kind = UnitKind::Harvester;
        unit.heading = 0;
        unit.order = Order::Harvest {
            node: TilePos::new(7, 1),
            anchor: None,
            retiring: false,
        };
        unit.drive_speed = unit.kind.stats().speed;
        let before = unit.pos;
        assert!(!is_anchored(unit));
        ground::advance(unit, &terrain, &parked);
        assert!(unit.pos.x > before.x);
        assert!(!is_anchored(unit));
        for _ in 0..2 {
            ground::advance(unit, &terrain, &parked);
        }
        assert_eq!(unit.drive_speed, Fx::ZERO);
        assert!(is_anchored(unit));
    }

    fn corner_shortcut_pair(
        name: &str,
        kind: UnitKind,
        pos: Vec2Fx,
        waypoint: TilePos,
        next: TilePos,
        blocked: TilePos,
    ) -> State {
        let width = 20;
        let height = 14;
        let mirror_tile = |tile: TilePos| TilePos::new(width - 1 - tile.x, height - 1 - tile.y);
        let mut map = vec![".".repeat(width as usize); height as usize];
        map[1].replace_range(1..2, "1");
        map[height as usize - 2].replace_range(width as usize - 2..width as usize - 1, "2");
        map[blocked.y as usize].replace_range((blocked.x as usize)..=(blocked.x as usize), "#");
        let mirrored_blocked = mirror_tile(blocked);
        map[mirrored_blocked.y as usize].replace_range(
            (mirrored_blocked.x as usize)..=(mirrored_blocked.x as usize),
            "#",
        );

        let mut state = Scenario {
            mode: Default::default(),
            name: name.into(),
            seed: 24_722,
            map,
            players: vec![
                seat("West", Faction::Ferrous),
                seat("East", Faction::Cupric),
            ],
            units: vec![
                UnitSpec {
                    player: 0,
                    kind,
                    x: pos.x.floor().to_num(),
                    y: pos.y.floor().to_num(),
                },
                UnitSpec {
                    player: 1,
                    kind,
                    x: width - 1 - pos.x.floor().to_num::<i32>(),
                    y: height - 1 - pos.y.floor().to_num::<i32>(),
                },
            ],
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("corner-shortcut pair builds");

        let mirrored_pos = Vec2Fx::new(Fx::from_num(width) - pos.x, Fx::from_num(height) - pos.y);
        let paths = [
            PathFollow {
                final_point: None,
                goal: next,
                waypoints: vec![waypoint, next],
                next: 0,
            },
            PathFollow {
                final_point: None,
                goal: mirror_tile(next),
                waypoints: vec![mirror_tile(waypoint), mirror_tile(next)],
                next: 0,
            },
        ];
        for ((unit, position), path) in state.units.iter_mut().zip([pos, mirrored_pos]).zip(paths) {
            unit.heading = chassis::compass::heading_of(
                path.waypoints[path.next as usize].center() - position,
            );
            unit.pos = position;
            unit.order = Order::Run {
                goal: path.goal.into(),
            };
            unit.path = Some(path);
        }
        state
    }

    fn assert_corner_shortcut_pair_reaches_goal(mut state: State) {
        let width = Fx::from_num(state.map.width());
        let height = Fx::from_num(state.map.height());
        let starts = [state.units[0].pos, state.units[1].pos];

        for _ in 0..64 {
            run(&mut state);
            assert_eq!(
                state.units[1].pos,
                Vec2Fx::new(width - state.units[0].pos.x, height - state.units[0].pos.y),
                "corner recovery lost half-turn symmetry"
            );
            if state.units.iter().all(|unit| unit.path.is_none()) {
                break;
            }
        }

        for (unit, start) in state.units.iter().zip(starts) {
            assert_ne!(
                unit.pos, start,
                "the worker never escaped the blocked corner"
            );
            let Order::Run { goal } = unit.order else {
                panic!("test worker lost its move order")
            };
            assert_eq!(unit.pos, goal.tile().center());
            assert!(
                unit.path.is_none(),
                "the worker did not finish within the bound"
            );
        }
    }

    /// A Harvester south of its own Fabricator's south-east corner with a
    /// route to (10, 3) that first rounds that corner, and the half-turned
    /// copy for the other seat. Offsets from the corner come from a match
    /// where the body pinned itself against the corner of a Foundry.
    fn corner_hugging_pair(offset: Vec2Fx, heading: u8, next: u32) -> State {
        let width = 32;
        let height = 14;
        let mirror_tile = |tile: TilePos| TilePos::new(width - 1 - tile.x, height - 1 - tile.y);
        let mut map = vec![".".repeat(width as usize); height as usize];
        map[1].replace_range(1..2, "1");
        map[height as usize - 3].replace_range(width as usize - 3..width as usize - 2, "2");
        let anchor = TilePos::new(6, 6);
        let mirrored_anchor = TilePos::new(width - 2 - anchor.x, height - 2 - anchor.y);
        let mut state = Scenario {
            mode: Default::default(),
            name: "corner-hugging-pair".into(),
            seed: 7_002,
            map,
            players: vec![
                seat("West", Faction::Ferrous),
                seat("East", Faction::Cupric),
            ],
            units: [(0, anchor), (1, mirrored_anchor)]
                .into_iter()
                .map(|(player, anchor)| UnitSpec {
                    player,
                    kind: UnitKind::Harvester,
                    x: anchor.x + 1,
                    y: anchor.y + 2,
                })
                .collect(),
            buildings: [(0, anchor), (1, mirrored_anchor)]
                .into_iter()
                .map(|(player, anchor)| crate::scenario::BuildingSpec {
                    player,
                    kind: crate::stats::BuildingKind::Fabricator,
                    x: anchor.x,
                    y: anchor.y,
                })
                .collect(),
            meta: None,
        }
        .build()
        .expect("corner-hugging pair builds");

        let corner = Vec2Fx::new(Fx::from_num(anchor.x + 2), Fx::from_num(anchor.y + 2));
        let pos = corner + offset;
        let mirrored_pos = Vec2Fx::new(Fx::from_num(width) - pos.x, Fx::from_num(height) - pos.y);
        let waypoints = tiles(&[(8, 8), (9, 7), (10, 6), (10, 5), (10, 4), (10, 3)]);
        let paths = [
            PathFollow {
                final_point: None,
                goal: *waypoints.last().unwrap(),
                waypoints: waypoints.clone(),
                next,
            },
            PathFollow {
                final_point: None,
                goal: mirror_tile(*waypoints.last().unwrap()),
                waypoints: waypoints.iter().copied().map(mirror_tile).collect(),
                next,
            },
        ];
        let headings = [heading, heading.wrapping_add(128)];
        for (((unit, position), path), heading) in state
            .units
            .iter_mut()
            .zip([pos, mirrored_pos])
            .zip(paths)
            .zip(headings)
        {
            unit.pos = position;
            unit.heading = heading;
            unit.order = Order::Run {
                goal: path.goal.into(),
            };
            unit.path = Some(path);
        }
        state
    }

    fn tiles(points: &[(i32, i32)]) -> Vec<TilePos> {
        points.iter().map(|&(x, y)| TilePos::new(x, y)).collect()
    }

    fn assert_corner_hugging_pair_arrives(mut state: State) {
        let width = Fx::from_num(state.map.width());
        let height = Fx::from_num(state.map.height());
        let goal = TilePos::new(10, 3);
        let clearance = |state: &State| {
            let unit = &state.units[0];
            let fabricator = state
                .buildings
                .iter()
                .find(|b| {
                    b.player == unit.player && b.kind == crate::stats::BuildingKind::Fabricator
                })
                .expect("the pair has a Fabricator");
            unit.pos.dist(fabricator.closest_point_to(unit.pos))
        };
        let start = clearance(&state);
        for _ in 0..240 {
            let report = state.tick(&[]);
            assert!(
                clearance(&state) >= start,
                "the Harvester steered closer to the Fabricator: {:?}",
                state.units[0].pos
            );
            assert!(
                !report
                    .events
                    .iter()
                    .any(|event| matches!(event, crate::Event::OrderStalled { .. })),
                "an open route must not stall"
            );
            assert_eq!(
                state.units[1].pos,
                Vec2Fx::new(width - state.units[0].pos.x, height - state.units[0].pos.y),
                "corner recovery lost half-turn symmetry"
            );
            if state.units[0].order == Order::Idle {
                break;
            }
        }
        let unit = &state.units[0];
        assert_eq!(
            unit.tile(),
            goal,
            "the Harvester pinned itself on the corner at {:?}",
            unit.pos
        );
        assert_eq!(unit.order, Order::Idle);
        assert!(unit.path.is_none());
    }

    #[test]
    fn a_body_pinned_on_a_building_corner_drives_off_it() {
        let state = corner_hugging_pair(
            Vec2Fx::new(Fx::lit("-0.005956769"), Fx::lit("0.0085311425")),
            217,
            1,
        );
        assert_corner_hugging_pair_arrives(state);
    }

    #[test]
    fn a_hull_overlapping_a_corner_never_steers_a_leg_across_it() {
        let state = corner_hugging_pair(
            Vec2Fx::new(Fx::lit("-0.182642685"), Fx::lit("0.1853985682")),
            240,
            0,
        );
        assert_corner_hugging_pair_arrives(state);
    }

    fn collision_trio() -> State {
        Scenario {
            mode: Default::default(),
            name: "collision-trio".into(),
            seed: 3,
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
                    y: 3,
                },
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Sentinel,
                    x: 4,
                    y: 3,
                },
                UnitSpec {
                    player: 0,
                    kind: UnitKind::Sentinel,
                    x: 6,
                    y: 3,
                },
            ],
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("collision trio builds")
    }

    fn replay_center_crossing() -> State {
        let mut map = vec![".".repeat(48); 30];
        map[5].replace_range(5..6, "1");
        map[24].replace_range(42..43, "2");
        let mut state = Scenario {
            mode: Default::default(),
            name: "replay-center-crossing".into(),
            seed: 1_616_101,
            map,
            players: vec![
                seat("West", Faction::Ferrous),
                seat("East", Faction::Ferrous),
            ],
            units: [0, 1, 0, 1]
                .into_iter()
                .enumerate()
                .map(|(slot, player)| UnitSpec {
                    player,
                    kind: UnitKind::Flakhound,
                    x: 10 + slot as i32,
                    y: 10,
                })
                .collect(),
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("replay-shaped crossing builds");
        state.tick = 12_269;
        let positions = [
            Vec2Fx::new(
                Fx::from_bits(104_588_663_713),
                Fx::from_bits(66_428_111_470),
            ),
            Vec2Fx::new(
                Fx::from_bits(101_569_766_495),
                Fx::from_bits(62_420_907_410),
            ),
            Vec2Fx::new(
                Fx::from_bits(101_367_326_202),
                Fx::from_bits(65_708_255_908),
            ),
            Vec2Fx::new(
                Fx::from_bits(104_791_104_006),
                Fx::from_bits(63_140_762_972),
            ),
        ];
        let paths = [
            PathFollow {
                final_point: None,
                goal: TilePos::new(22, 13),
                waypoints: vec![
                    TilePos::new(24, 15),
                    TilePos::new(23, 14),
                    TilePos::new(22, 13),
                ],
                next: 1,
            },
            PathFollow {
                final_point: None,
                goal: TilePos::new(25, 16),
                waypoints: vec![
                    TilePos::new(23, 14),
                    TilePos::new(24, 15),
                    TilePos::new(25, 16),
                ],
                next: 1,
            },
            PathFollow {
                final_point: None,
                goal: TilePos::new(23, 14),
                waypoints: vec![TilePos::new(23, 15), TilePos::new(23, 14)],
                next: 1,
            },
            PathFollow {
                final_point: None,
                goal: TilePos::new(24, 15),
                waypoints: vec![TilePos::new(24, 14), TilePos::new(24, 15)],
                next: 1,
            },
        ];
        for ((unit, pos), path) in state.units.iter_mut().zip(positions).zip(paths) {
            unit.heading =
                chassis::compass::heading_of(path.waypoints[path.next as usize].center() - pos);
            unit.pos = pos;
            unit.order = Order::Hunt {
                goal: path.goal.into(),
            };
            unit.path = Some(path);
        }
        state
    }

    fn assert_replay_pairs_are_half_turns(state: &State) {
        let center_twice = Vec2Fx::new(
            Fx::from_num(state.map.width()),
            Fx::from_num(state.map.height()),
        );
        for (west, east) in [(0, 1), (2, 3)] {
            assert_eq!(
                state.units[east].pos,
                center_twice - state.units[west].pos,
                "owner-local rank {west:?}/{east:?} lost half-turn symmetry"
            );
        }
    }

    fn assert_collision_half_turn(mut original: State, travel: Vec<Vec2Fx>) {
        let width = Fx::from_num(original.map.width());
        let height = Fx::from_num(original.map.height());
        let mut rotated = original.clone();
        for unit in &mut rotated.units {
            unit.pos = Vec2Fx::new(width - unit.pos.x, height - unit.pos.y);
        }
        let rotated_travel = travel.iter().map(|step| -*step).collect::<Vec<_>>();

        let mut original_index = UnitIndex::new();
        let mut rotated_index = UnitIndex::new();
        resolve_collisions(&mut original, &travel, &mut original_index);
        resolve_collisions(&mut rotated, &rotated_travel, &mut rotated_index);

        for (unit, rotated_unit) in original.units.iter().zip(&rotated.units) {
            assert_eq!(unit.id, rotated_unit.id);
            assert_eq!(
                rotated_unit.pos,
                Vec2Fx::new(width - unit.pos.x, height - unit.pos.y),
                "collision resolution favored an absolute direction for {:?}",
                unit.id
            );
        }
    }

    #[test]
    fn ordered_multi_body_collision_is_equivariant_under_half_turns() {
        let mut state = collision_trio();
        state.units[0].pos = Vec2Fx::new(Fx::lit("5.5"), Fx::lit("3.5"));
        state.units[1].pos = Vec2Fx::new(Fx::lit("5.05"), Fx::lit("3.35"));
        state.units[2].pos = Vec2Fx::new(Fx::lit("5.95"), Fx::lit("3.65"));
        let travel = vec![
            Vec2Fx::new(Fx::lit("0.04"), Fx::lit("0.01")),
            Vec2Fx::new(Fx::lit("0.08"), Fx::lit("0.02")),
            Vec2Fx::new(Fx::lit("-0.05"), Fx::lit("-0.01")),
        ];

        assert_collision_half_turn(state, travel);
    }

    #[test]
    fn mirrored_crossing_armies_remain_exact_half_turns_through_collision() {
        let mut state = replay_center_crossing();
        assert_replay_pairs_are_half_turns(&state);
        let (travel, _) = run(&mut state);
        assert_replay_pairs_are_half_turns(&state);
        let mut index = UnitIndex::new();

        resolve_collisions(&mut state, &travel, &mut index);

        assert_replay_pairs_are_half_turns(&state);
    }

    #[test]
    fn perfectly_stacked_collision_is_equivariant_under_half_turns() {
        let mut state = collision_trio();
        let stack = Vec2Fx::new(Fx::lit("5.5"), Fx::lit("3.5"));
        for unit in &mut state.units {
            unit.pos = stack;
        }

        assert_collision_half_turn(state, vec![Vec2Fx::ZERO; 3]);
    }

    #[test]
    fn mirrored_seat_stacks_ignore_global_id_blocks() {
        let mut state = Scenario {
            mode: Default::default(),
            name: "mirrored-seat-stacks".into(),
            seed: 4,
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
                seat("West", Faction::Ferrous),
                seat("East", Faction::Ferrous),
            ],
            units: (0..6)
                .map(|slot| UnitSpec {
                    player: u8::from(slot >= 3),
                    kind: UnitKind::Sentinel,
                    x: 2 + slot,
                    y: 2,
                })
                .collect(),
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("mirrored stack scenario builds");
        let width = Fx::from_num(state.map.width());
        let height = Fx::from_num(state.map.height());
        let west_stack = Vec2Fx::new(Fx::lit("4.5"), Fx::lit("3.5"));
        let east_stack = Vec2Fx::new(width - west_stack.x, height - west_stack.y);
        state.units[0].pos = west_stack;
        state.units[1].pos = west_stack;
        state.units[2].pos = Vec2Fx::new(Fx::lit("2.5"), Fx::lit("1.5"));
        state.units[3].pos = east_stack;
        state.units[4].pos = east_stack;
        state.units[5].pos =
            Vec2Fx::new(width - state.units[2].pos.x, height - state.units[2].pos.y);
        let mut index = UnitIndex::new();

        resolve_collisions(&mut state, &[Vec2Fx::ZERO; 6], &mut index);

        for (west, east) in [(0, 3), (1, 4)] {
            assert_eq!(
                state.units[east].pos,
                Vec2Fx::new(
                    width - state.units[west].pos.x,
                    height - state.units[west].pos.y,
                ),
                "matching owner-local ranks must separate in mirrored directions"
            );
        }
    }

    #[test]
    fn collision_finds_border_crossing_pairs_in_either_id_order() {
        let height = boundary_pair().map.height();
        let edges = [
            ("north", Fx::lit("0.1"), Fx::lit("-0.1")),
            (
                "south",
                Fx::from_num(height) - Fx::lit("0.1"),
                Fx::from_num(height) + Fx::lit("0.1"),
            ),
        ];

        for (edge, inside_y, outside_y) in edges {
            for outside_slot in 0..2 {
                let mut state = boundary_pair();
                let inside_slot = 1 - outside_slot;
                state.units[outside_slot].pos = Vec2Fx::new(Fx::lit("5.5"), outside_y);
                state.units[inside_slot].pos = Vec2Fx::new(Fx::lit("5.5"), inside_y);
                state.refresh_vision();
                state
                    .validate_invariants()
                    .expect("the accepted coordinate envelope includes border rows");
                let before = state.units[inside_slot].pos;
                let travel = vec![Vec2Fx::ZERO; state.units.len()];
                let mut index = UnitIndex::new();
                let owner_ranks = owner_local_ranks(&state);
                let mut spent = Vec::new();
                let pairs = collision_pairs(&state, false, &mut index, &owner_ranks)
                    .expect("the boundary pair overlaps");

                assert!(
                    relaxation_pass(&mut state, &travel, &owner_ranks, &mut spent, &pairs),
                    "{edge} pair with outside slot {outside_slot} was not visited"
                );
                assert_ne!(
                    state.units[inside_slot].pos, before,
                    "{edge} pair with outside slot {outside_slot} was not separated"
                );
            }
        }
    }

    fn resting_overlap(kinds: [UnitKind; 2], separation: Fx) -> [Fx; 2] {
        let mut state = boundary_pair();
        let center = TilePos::new(5, 2).center();
        let half = separation * chassis::fx::HALF;
        for ((unit, kind), side) in state.units.iter_mut().zip(kinds).zip([-Fx::ONE, Fx::ONE]) {
            unit.kind = kind;
            unit.pos = Vec2Fx::new(center.x + half * side, center.y);
        }
        let before: Vec<Vec2Fx> = state.units.iter().map(|unit| unit.pos).collect();
        let travel = vec![Vec2Fx::ZERO; state.units.len()];
        resolve_collisions(&mut state, &travel, &mut UnitIndex::new());
        [0, 1].map(|slot| state.units[slot].pos.dist(before[slot]))
    }

    #[test]
    fn a_contact_within_the_slop_rests_and_a_deeper_one_separates() {
        let spacing = UnitKind::Sentinel.stats().radius * 2;
        let resting = resting_overlap([UnitKind::Sentinel; 2], spacing - COLLISION_SLOP / 2);
        assert_eq!(
            resting,
            [Fx::ZERO; 2],
            "rounding-deep contact needs no push"
        );
        let [left, right] = resting_overlap([UnitKind::Sentinel; 2], spacing - COLLISION_SLOP * 2);
        assert!(left > Fx::ZERO && right > Fx::ZERO);
    }

    #[test]
    fn equal_bodies_split_an_overlap_exactly_in_half() {
        let [left, right] = resting_overlap([UnitKind::Sentinel; 2], Fx::lit("0.6"));
        assert!(left > Fx::ZERO);
        assert_eq!(left, right);
    }

    #[test]
    fn a_heavy_hull_yields_less_than_the_light_one_shoving_it() {
        let kinds = [UnitKind::Sentinel, UnitKind::Breaker];
        let radii = kinds.map(|kind| kind.stats().radius);
        let overlap = Fx::lit("0.1");
        let [light, heavy] = resting_overlap(kinds, radii[0] + radii[1] - overlap);
        let tolerance = Fx::lit("0.000001");
        assert!(heavy > Fx::ZERO && light > heavy + heavy);
        assert!((light + heavy - overlap).abs() < tolerance);
        let imbalance = light * radii[0] * radii[0] - heavy * radii[1] * radii[1];
        assert!(
            imbalance.abs() < tolerance,
            "shares are not inverse to area"
        );
    }

    #[test]
    fn mixed_size_collision_is_equivariant_under_half_turns() {
        let mut state = collision_trio();
        state.units[0].kind = UnitKind::Breaker;
        state.units[2].kind = UnitKind::Scuttler;
        state.units[0].pos = Vec2Fx::new(Fx::lit("5.5"), Fx::lit("3.5"));
        state.units[1].pos = Vec2Fx::new(Fx::lit("4.85"), Fx::lit("3.3"));
        state.units[2].pos = Vec2Fx::new(Fx::lit("6.1"), Fx::lit("3.75"));
        let travel = vec![
            Vec2Fx::new(Fx::lit("0.02"), Fx::lit("0.01")),
            Vec2Fx::new(Fx::lit("0.08"), Fx::lit("0.02")),
            Vec2Fx::new(Fx::lit("-0.12"), Fx::lit("-0.03")),
        ];

        assert_collision_half_turn(state, travel);
    }

    /// A seat's light body meeting the other seat's heavy one, and the
    /// half-turn image with the kinds exchanged between the seats: which body
    /// the pair loop reaches first differs, so the split must not depend on it.
    #[test]
    fn mixed_size_split_ignores_which_seat_owns_the_heavier_body() {
        let kinds = [UnitKind::Sentinel, UnitKind::Breaker];
        let mut original = boundary_pair();
        let width = Fx::from_num(original.map.width());
        let height = Fx::from_num(original.map.height());
        let rotate = |pos: Vec2Fx| Vec2Fx::new(width - pos.x, height - pos.y);
        for step in 0..64 {
            let gap = Fx::lit("0.5") + Fx::lit("0.00617") * Fx::from_num(step);
            let at = [
                Vec2Fx::new(Fx::lit("5.31"), Fx::lit("2.47")),
                Vec2Fx::new(
                    Fx::lit("5.31") + gap,
                    Fx::lit("2.47") + gap * Fx::lit("0.37"),
                ),
            ];
            let mut rotated = original.clone();
            for slot in 0..2 {
                original.units[slot].kind = kinds[slot];
                original.units[slot].pos = at[slot];
                rotated.units[slot].kind = kinds[1 - slot];
                rotated.units[slot].pos = rotate(at[1 - slot]);
            }
            let travel = vec![Vec2Fx::ZERO; 2];
            let mut a = original.clone();
            resolve_collisions(&mut a, &travel, &mut UnitIndex::new());
            resolve_collisions(&mut rotated, &travel, &mut UnitIndex::new());
            for slot in 0..2 {
                assert_eq!(
                    rotated.units[slot].pos,
                    rotate(a.units[1 - slot].pos),
                    "gap {gap}: the split depended on pair order"
                );
            }
        }
    }

    #[test]
    fn collision_budget_spans_every_relaxation_pass_in_a_tick() {
        let mut state = boundary_pair();
        let stacked = TilePos::new(5, 2).center();
        for unit in &mut state.units {
            unit.pos = stacked;
        }
        let before: Vec<Vec2Fx> = state.units.iter().map(|u| u.pos).collect();
        let travel = vec![Vec2Fx::ZERO; state.units.len()];
        let mut index = UnitIndex::new();

        resolve_collisions(&mut state, &travel, &mut index);

        for (unit, before) in state.units.iter().zip(before) {
            let correction = unit.pos.dist(before);
            assert!(
                correction <= COLLISION_MAX_STEP,
                "{:?} received {correction:?} of correction in one tick",
                unit.id
            );
        }
    }

    #[test]
    fn coordinated_head_on_slide_is_rotation_equivariant() {
        let away = Vec2Fx::new(Fx::lit("0.6"), Fx::lit("0.8"));
        let travel = -away;
        let original = correction_dirs(away, travel, true);
        let rotated = correction_dirs(-away, -travel, true);

        for (original, rotated) in original.into_iter().zip(rotated) {
            let (Some(original), Some(rotated)) = (original, rotated) else {
                assert_eq!(original, rotated);
                continue;
            };
            let error = original + rotated;
            let tolerance = Fx::DELTA * 2;
            assert!(error.x.abs() <= tolerance);
            assert!(error.y.abs() <= tolerance);
        }
    }

    #[test]
    fn oriented_collision_rows_reverse_x_groups_without_reversing_slot_order() {
        let row = [
            (TilePos::new(3, 4), 1),
            (TilePos::new(3, 4), 5),
            (TilePos::new(4, 4), 2),
            (TilePos::new(6, 4), 0),
            (TilePos::new(6, 4), 7),
        ];

        assert_eq!(
            OrientedRow::new(&row, false).collect::<Vec<_>>(),
            [1, 5, 2, 0, 7]
        );
        assert_eq!(
            OrientedRow::new(&row, true).collect::<Vec<_>>(),
            [0, 7, 2, 1, 5]
        );
        assert!(OrientedRow::new(&[], true).next().is_none());
    }

    #[test]
    fn passed_waypoint_still_rejects_a_blocked_next_step() {
        let mut state = Scenario {
            mode: Default::default(),
            name: "blocked-next-waypoint".into(),
            seed: 2,
            map: vec![
                "............".into(),
                "............".into(),
                "......#.....".into(),
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
            units: vec![UnitSpec {
                player: 0,
                kind: UnitKind::Avalanche,
                x: 5,
                y: 2,
            }],
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("blocked waypoint state builds");
        let unit = &mut state.units[0];
        unit.pos = Vec2Fx::new(Fx::lit("5.9"), Fx::lit("2.5"));
        unit.order = Order::Run {
            goal: TilePos::new(6, 2).into(),
        };
        unit.path = Some(PathFollow {
            final_point: None,
            goal: TilePos::new(6, 2),
            waypoints: vec![TilePos::new(5, 2), TilePos::new(6, 2)],
            next: 0,
        });
        let before = unit.pos;

        run(&mut state);

        assert_eq!(state.units[0].pos, before);
        assert!(state.units[0].path.is_none());
        assert!(state.map.terrain_passable(state.units[0].tile()));
    }

    #[test]
    fn harvester_deflected_past_a_waypoint_does_not_drop_its_route() {
        let state = corner_shortcut_pair(
            "harvester-corner-shortcut",
            UnitKind::Harvester,
            Vec2Fx::new(Fx::lit("7.0030496502"), Fx::lit("7.5430232098")),
            TilePos::new(6, 7),
            TilePos::new(6, 8),
            TilePos::new(7, 8),
        );

        assert_corner_shortcut_pair_reaches_goal(state);
    }

    #[test]
    fn excavator_deflected_past_a_waypoint_does_not_drop_its_route() {
        let state = corner_shortcut_pair(
            "excavator-corner-shortcut",
            UnitKind::Excavator,
            Vec2Fx::new(Fx::lit("8.4923983465"), Fx::lit("6.9311533265")),
            TilePos::new(8, 7),
            TilePos::new(7, 7),
            TilePos::new(7, 6),
        );

        assert_corner_shortcut_pair_reaches_goal(state);
    }

    #[test]
    fn diagonal_head_on_pair_passes_shared_waypoint() {
        let mut state = boundary_pair();
        state.tick = 21_549;
        let shared = TilePos::new(5, 3);
        let paths = [
            PathFollow {
                final_point: None,
                goal: TilePos::new(2, 1),
                waypoints: vec![
                    shared,
                    TilePos::new(4, 2),
                    TilePos::new(3, 1),
                    TilePos::new(2, 1),
                ],
                next: 0,
            },
            PathFollow {
                final_point: None,
                goal: TilePos::new(9, 6),
                waypoints: vec![
                    shared,
                    TilePos::new(6, 4),
                    TilePos::new(7, 5),
                    TilePos::new(8, 6),
                    TilePos::new(9, 6),
                ],
                next: 0,
            },
        ];
        let positions = [
            Vec2Fx::new(Fx::lit("5.922209162"), Fx::lit("3.837034112")),
            Vec2Fx::new(Fx::lit("5.236790039"), Fx::lit("3.125446076")),
        ];
        for ((unit, pos), path) in state.units.iter_mut().zip(positions).zip(paths) {
            unit.kind = UnitKind::Avalanche;
            unit.heading =
                chassis::compass::heading_of(path.waypoints[path.next as usize].center() - pos);
            unit.hp = UnitKind::Avalanche.stats().max_hp;
            unit.pos = pos;
            unit.order = Order::Run {
                goal: path.goal.into(),
            };
            unit.path = Some(path);
        }

        let mut index = UnitIndex::new();
        for _ in 0..80 {
            let (travel, _) = run(&mut state);
            resolve_collisions(&mut state, &travel, &mut index);
            state.tick += 1;
        }

        assert!(
            state
                .units
                .iter()
                .all(|unit| { unit.path.as_ref().is_none_or(|path| path.next > 0) }),
            "both Avalanches must pass the shared waypoint instead of oscillating: {:?}",
            state
                .units
                .iter()
                .map(|unit| (unit.id, unit.pos, unit.path.as_ref().map(|path| path.next)))
                .collect::<Vec<_>>()
        );
    }
}
