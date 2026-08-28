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

use crate::map::Map;
use crate::state::{Order, PathFollow, State};
use chassis::fx::{Fx, Vec2Fx, sqrt};
use chassis::grid::TilePos;

use crate::stats::{
    ANCHORED_PUSH_SHARE, COLLISION_ITERATIONS, COLLISION_MAX_STEP, SLIDE_LATERAL_SHARE,
    SLIDE_RADIAL_SHARE, WAYPOINT_ACCEPT,
};

/// Whether skipping from waypoint `cur` toward `nxt` early (from anywhere
/// within the acceptance radius) can clip impassable ground. Cardinal
/// neighbors are always safe — the swept band stays inside two open tiles.
/// Diagonals are safe only when both shared cardinal tiles are open (then
/// the whole 2×2 block is open); that is the same invariant A* enforces on
/// the path itself, re-checked here because acceptance cuts the corner
/// tighter than the path did.
fn early_advance_safe(
    cur: TilePos,
    nxt: TilePos,
    map: &Map,
    buildings: &[crate::state::Building],
) -> bool {
    let open = |t: TilePos| {
        map.terrain_passable(t)
            && !buildings
                .iter()
                .any(|b| b.contains(t) && !b.kind.is_stealthy())
    };
    let (dx, dy) = (nxt.x - cur.x, nxt.y - cur.y);
    if dx == 0 || dy == 0 {
        return true;
    }
    open(cur.offset(dx, 0)) && open(cur.offset(0, dy))
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
/// tile: candidates ring-scan outward in (chebyshev, y, x) order — the
/// deterministic order every ring scan uses — and the first one that
/// routes wins (A* consults `passable` for every tile except the start,
/// so a body paths out of ground it could not enter). Bounded: any real
/// escape begins on an adjacent open tile, so the reach only pads for
/// corner-cut geometry.
pub(super) fn escape_route(
    state: &State,
    kind: crate::stats::UnitKind,
    from: TilePos,
) -> Option<PathFollow> {
    for r in 1..=crate::stats::EVICT_SCAN_RADIUS {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let goal = from.offset(dx, dy);
                if !state.passable(goal) {
                    continue;
                }
                if let Some(waypoints) = super::route_for(state, kind, from, goal) {
                    return Some(PathFollow {
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
        || unit.kind.stats().domain != crate::stats::Domain::Ground
        || unit.path.is_some()
        || state
            .building_at(unit.tile())
            .is_none_or(|b| b.kind.is_stealthy())
    {
        return None;
    }
    escape_route(state, unit.kind, unit.tile())
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
            state.units[i].path = Some(path);
        }
    }
}

/// Advances every unit along its path by its speed, returning each
/// unit's displacement this tick (indexed like `state.units`) — the
/// collision resolver reads travel to slide movers around each other
/// instead of grinding them head-on. Intermediate waypoints are accepted
/// within [`WAYPOINT_ACCEPT`], or after a nearby collision deflection has
/// carried the body across the onward plane, so a unit does not turn back
/// toward a center it already passed. Final waypoints are landed exactly.
pub(super) fn run(state: &mut State) -> Vec<Vec2Fx> {
    // Disjoint field borrows: units move, terrain is read-only.
    let State {
        units,
        map,
        buildings,
        ..
    } = state;
    let mut travel = vec![Vec2Fx::ZERO; units.len()];
    for (slot, unit) in units.iter_mut().enumerate() {
        if unit.hp == 0 {
            continue;
        }
        let before = unit.pos;
        let stats = unit.kind.stats();
        if stats.turn_rate > 0 {
            steer_turn_limited(unit, map, stats);
            travel[slot] = unit.pos - before;
            continue;
        }
        let airborne = stats.domain == crate::stats::Domain::Air;
        let mut budget = stats.speed;
        while budget > Fx::ZERO {
            let Some(path) = &mut unit.path else { break };
            let Some(&waypoint) = path.waypoints.get(path.next as usize) else {
                unit.path = None;
                break;
            };
            // Ground can close mid-walk (a site claims its footprint at
            // command time): never step toward a waypoint that is no
            // longer open — and never take a diagonal whose two flanking
            // cardinals aren't both open either, the same no-corner-cut
            // rule A* guaranteed when the path was computed. Drop the path
            // and let the brain repath around whatever appeared. None of
            // it binds a flyer: air routes never close.
            if !airborne {
                let open = |t: TilePos| {
                    map.terrain_passable(t)
                        && !buildings
                            .iter()
                            .any(|b| b.contains(t) && !b.kind.is_stealthy())
                };
                let here = TilePos::containing(unit.pos);
                let (dx, dy) = (waypoint.x - here.x, waypoint.y - here.y);
                let corner_cut = dx != 0
                    && dy != 0
                    && !(open(here.offset(dx.signum(), 0)) && open(here.offset(0, dy.signum())));
                if !open(waypoint) || corner_cut {
                    unit.path = None;
                    break;
                }
            }
            let center = waypoint.center();
            let dist = unit.pos.dist(center);
            if let Some(&next_wp) = path.waypoints.get(path.next as usize + 1)
                && (dist <= WAYPOINT_ACCEPT
                    || passed_intermediate_waypoint(unit.pos, waypoint, next_wp, stats.radius))
                && (airborne || early_advance_safe(waypoint, next_wp, map, buildings))
            {
                path.next += 1;
                continue; // spend the budget on the next leg instead
            }
            if dist <= budget {
                unit.pos = center;
                budget -= dist;
                path.next += 1;
                if path.next as usize >= path.waypoints.len() {
                    unit.path = None;
                    break;
                }
            } else {
                unit.pos = unit.pos.move_toward(center, budget);
                break;
            }
        }
        travel[slot] = unit.pos - before;
    }
    travel
}

/// Turn-limited flight: the body advances along its heading and only
/// the heading steers, at most `turn_rate` compass steps per tick.
/// Waypoints are accepted inside the kind's turn-acceptance ring — a
/// bounded arc cannot promise an exact center, and a ring tighter than
/// the turn radius is an orbit trap. A step whose tile is closed to air
/// invalidates the route so the owning brain can plan again from the
/// aircraft's actual position instead of steering into the same mountain
/// forever. Pathless means hovering.
fn steer_turn_limited(
    unit: &mut crate::state::Unit,
    map: &Map,
    stats: &'static crate::stats::UnitStats,
) {
    let accept = stats.turn_acceptance();
    let arrive_sq = accept * accept;
    // Accept every waypoint the arc has already effectively reached. A
    // terrain-routing waypoint is not disposable merely because the wide
    // acceptance ring overlaps it: only skip it when the direct air segment
    // to the following waypoint is also clear. Otherwise a bomber can erase
    // the one waypoint that would have turned it around a peak, then keep
    // replanning the same impossible shortcut.
    loop {
        let Some(path) = &unit.path else { return };
        let Some(&waypoint) = path.waypoints.get(path.next as usize) else {
            unit.path = None;
            return;
        };
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
            return;
        }
    }
    let target = {
        let path = unit.path.as_ref().expect("checked above");
        path.waypoints[path.next as usize].center()
    };
    // Steer: rotate one compass step at a time toward the goal ray,
    // stopping early the moment the nose crosses it. The cross product's
    // sign picks the turn direction; dead astern breaks the tie toward
    // +1, and every input is Q32.32, so each platform turns identically.
    let d = target - unit.pos;
    for _ in 0..stats.turn_rate {
        let hv = chassis::compass::dir(unit.heading);
        let cross = hv.x * d.y - hv.y * d.x;
        let dot = hv.x * d.x + hv.y * d.y;
        if cross == Fx::ZERO && dot >= Fx::ZERO {
            break;
        }
        let step: u8 = if cross > Fx::ZERO { 1 } else { 255 };
        let next = unit.heading.wrapping_add(step);
        let nhv = chassis::compass::dir(next);
        let ncross = nhv.x * d.y - nhv.y * d.x;
        unit.heading = next;
        if (cross > Fx::ZERO) != (ncross > Fx::ZERO) {
            break;
        }
    }
    let ahead = unit.pos + chassis::compass::dir(unit.heading) * stats.speed;
    let tile = TilePos::containing(ahead);
    let open = map.tile(tile).is_some_and(|t| !t.terrain.blocks_air());
    if open {
        unit.pos = ahead;
    } else {
        unit.path = None;
    }
}

/// A unit that is standing still to work — extracting, welding, or
/// holding fire on a target — resists shoving; movers yield around it.
fn is_anchored(unit: &crate::state::Unit) -> bool {
    unit.path.is_none()
        && matches!(
            unit.order,
            Order::Harvest { .. } | Order::Attack { .. } | Order::Repair { .. }
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

fn uses_rotated_map_frame(state: &State, pos: Vec2Fx) -> bool {
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
/// push overlapping pairs apart, half the overlap each, so units cannot
/// stack — grouped movers fan out and a body-blocked unit stays blocked. A
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
    let mut spent = vec![Fx::ZERO; state.units.len()];
    for _ in 0..COLLISION_ITERATIONS {
        if !relaxation_pass(state, reversed, travel, index, &owner_ranks, &mut spent) {
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

fn collision_pair_key(
    state: &State,
    owner_ranks: &[usize],
    i: usize,
    j: usize,
) -> (usize, usize, bool, (Vec2Fx, Vec2Fx)) {
    let ranks = if owner_ranks[i] <= owner_ranks[j] {
        (owner_ranks[i], owner_ranks[j])
    } else {
        (owner_ranks[j], owner_ranks[i])
    };
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
    (
        ranks.0,
        ranks.1,
        state.units[i].player == state.units[j].player,
        world.min(rotated),
    )
}

/// Candidate contacts in a seat-local order. A half-turn maps each pair to a
/// pair with the same owner-local ranks and canonical geometry. Counterpart
/// pairs therefore remain adjacent; they touch disjoint units and commute,
/// while the orbit order around a crowded crossing is identical for both
/// seats. Raw unit ids cannot provide that property because corresponding
/// seats receive adjacent, not mirrored, global ids.
fn collision_pairs(
    state: &State,
    reversed: bool,
    index: &super::spatial::UnitIndex,
    owner_ranks: &[usize],
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    for i in 0..state.units.len() {
        if state.units[i].hp == 0 {
            continue;
        }
        let home = state.units[i].tile();
        let rotated_frame = uses_rotated_map_frame(state, state.units[i].pos);
        for row_offset in 0..3 {
            let dy = if rotated_frame {
                1 - row_offset
            } else {
                row_offset - 1
            };
            let row = index.row_span(home.y + dy, home.x - 1, home.x + 1);
            for j in OrientedRow::new(row, rotated_frame) {
                if j > i {
                    pairs.push((i, j));
                }
            }
        }
    }
    pairs.sort_by_key(|&(i, j)| collision_pair_key(state, owner_ranks, i, j));
    if reversed {
        pairs.reverse();
    }
    pairs
}

/// One pass; returns whether any overlap was found.
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
    reversed: bool,
    travel: &[Vec2Fx],
    index: &mut super::spatial::UnitIndex,
    owner_ranks: &[usize],
    spent: &mut Vec<Fx>,
) -> bool {
    let n = state.units.len();
    if n < 2 {
        return false;
    }
    // Tile buckets as a sorted list — no hash maps in sim code. Interaction
    // reach is under one tile (radii sum < 1), so 3x3 neighborhoods suffice.
    // Buckets are snapshotted at pass start; corrections are small enough
    // (≤ COLLISION_MAX_STEP) that a newly-adjacent pair simply waits for
    // the next pass.
    index.rebuild(&state.units);

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
    for (i, j) in collision_pairs(state, reversed, index, owner_ranks) {
        let (pos_i, radius_i, dom_i) = {
            let u = &state.units[i];
            (u.pos, u.kind.stats().radius, u.kind.stats().domain)
        };
        let (pos_j, radius_j, dom_j) = {
            let u = &state.units[j];
            (u.pos, u.kind.stats().radius, u.kind.stats().domain)
        };
        // Bodies only collide within their own layer: a flyer
        // and a crawler occupy the same tile without touching.
        if dom_i != dom_j {
            continue;
        }
        let min_dist = radius_i + radius_j;
        let delta = pos_j - pos_i;
        let dist_sq = delta.length_sq();
        if dist_sq >= min_dist * min_dist {
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
        let (share_i, share_j) = match (is_anchored(&state.units[i]), is_anchored(&state.units[j]))
        {
            (true, false) => (ANCHORED_PUSH_SHARE, Fx::ONE - ANCHORED_PUSH_SHARE),
            (false, true) => (Fx::ONE - ANCHORED_PUSH_SHARE, ANCHORED_PUSH_SHARE),
            _ => (chassis::fx::HALF, chassis::fx::HALF),
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
                let to = pos_j + cand * step_j;
                if state.passable_for(dom_j, TilePos::containing(to)) {
                    state.units[j].pos = to;
                    spent[j] += step_j;
                    break;
                }
            }
        }
        let step_i = (overlap * share_i).min(COLLISION_MAX_STEP - spent[i]);
        if step_i > Fx::ZERO {
            for cand in dirs_i.into_iter().flatten() {
                let to = pos_i + cand * step_i;
                if state.passable_for(dom_i, TilePos::containing(to)) {
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

    fn collision_trio() -> State {
        Scenario {
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
                goal: TilePos::new(22, 13),
                waypoints: vec![
                    TilePos::new(24, 15),
                    TilePos::new(23, 14),
                    TilePos::new(22, 13),
                ],
                next: 1,
            },
            PathFollow {
                goal: TilePos::new(25, 16),
                waypoints: vec![
                    TilePos::new(23, 14),
                    TilePos::new(24, 15),
                    TilePos::new(25, 16),
                ],
                next: 1,
            },
            PathFollow {
                goal: TilePos::new(23, 14),
                waypoints: vec![TilePos::new(23, 15), TilePos::new(23, 14)],
                next: 1,
            },
            PathFollow {
                goal: TilePos::new(24, 15),
                waypoints: vec![TilePos::new(24, 14), TilePos::new(24, 15)],
                next: 1,
            },
        ];
        for ((unit, pos), path) in state.units.iter_mut().zip(positions).zip(paths) {
            unit.pos = pos;
            unit.order = Order::AttackMove { goal: path.goal };
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
        let travel = run(&mut state);
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
                state
                    .validate_invariants()
                    .expect("the accepted coordinate envelope includes border rows");
                let before = state.units[inside_slot].pos;
                let travel = vec![Vec2Fx::ZERO; state.units.len()];
                let mut index = UnitIndex::new();
                let owner_ranks = owner_local_ranks(&state);
                let mut spent = Vec::new();

                assert!(
                    relaxation_pass(
                        &mut state,
                        false,
                        &travel,
                        &mut index,
                        &owner_ranks,
                        &mut spent,
                    ),
                    "{edge} pair with outside slot {outside_slot} was not visited"
                );
                assert_ne!(
                    state.units[inside_slot].pos, before,
                    "{edge} pair with outside slot {outside_slot} was not separated"
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
        unit.order = Order::Move {
            goal: TilePos::new(6, 2),
        };
        unit.path = Some(PathFollow {
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
    fn diagonal_head_on_pair_passes_shared_waypoint() {
        let mut state = boundary_pair();
        state.tick = 21_549;
        let shared = TilePos::new(5, 3);
        let paths = [
            PathFollow {
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
            unit.hp = UnitKind::Avalanche.stats().max_hp;
            unit.pos = pos;
            unit.order = Order::Move { goal: path.goal };
            unit.path = Some(path);
        }

        let mut index = UnitIndex::new();
        for _ in 0..20 {
            let travel = run(&mut state);
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
