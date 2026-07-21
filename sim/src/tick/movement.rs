//! Phases 4–5: path following and collision resolution.
//!
//! Movement is per-unit work. Since 0.5, ground can close *during* a walk —
//! a construction site claims its footprint the moment the command lands —
//! so each step revalidates the waypoint it is about to move toward and
//! drops the path when the ground has closed (the brain repaths around the
//! new obstacle next tick). Collision resolution then pushes overlapping
//! bodies apart until they fit — units are solid to each other, but tiles
//! are only ever blocked by terrain and buildings, so pathfinding stays
//! deadlock-free while crowds physically jostle.

use crate::map::Map;
use crate::state::{Order, State};
use chassis::fx::{Fx, Vec2Fx, sqrt};
use chassis::grid::TilePos;

use crate::stats::{
    ANCHORED_PUSH_SHARE, COLLISION_ITERATIONS, COLLISION_MAX_STEP, WAYPOINT_ACCEPT,
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
    let open = |t: TilePos| map.terrain_passable(t) && !buildings.iter().any(|b| b.contains(t));
    let (dx, dy) = (nxt.x - cur.x, nxt.y - cur.y);
    if dx == 0 || dy == 0 {
        return true;
    }
    open(cur.offset(dx, 0)) && open(cur.offset(0, dy))
}

/// Advances every unit along its path by its speed. Intermediate waypoints
/// are accepted within [`WAYPOINT_ACCEPT`] (when geometry allows) so a
/// unit shoved off the line flows forward instead of re-seeking each exact
/// center; final waypoints are still landed exactly.
pub(super) fn run(state: &mut State) {
    // Disjoint field borrows: units move, terrain is read-only.
    let State {
        units,
        map,
        buildings,
        ..
    } = state;
    for unit in units.iter_mut() {
        if unit.hp == 0 {
            continue;
        }
        let stats = unit.kind.stats();
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
                    map.terrain_passable(t) && !buildings.iter().any(|b| b.contains(t))
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
                && dist <= WAYPOINT_ACCEPT
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
    }
}

/// A unit that is standing still to work — extracting or holding fire on a
/// target — resists shoving; movers yield around it.
fn is_anchored(unit: &crate::state::Unit) -> bool {
    unit.path.is_none() && matches!(unit.order, Order::Harvest { .. } | Order::Attack { .. })
}

/// Unit directions for perfectly stacked pairs, indexed by id xor — any
/// fixed assignment works, it just has to break the tie deterministically.
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

/// Resolves unit-unit collisions: several deterministic relaxation passes
/// push overlapping pairs apart, half the overlap each, so units cannot
/// stack — grouped movers fan out and a body-blocked unit stays blocked. A
/// push that would land in an impassable tile is discarded (rocks beat
/// crowd pressure), and each pass caps per-unit displacement so packed
/// crowds settle instead of exploding.
pub(super) fn resolve_collisions(state: &mut State) {
    // Direction alternates by tick parity — Gauss-Seidel's sequential
    // application must not always favor the same ids (see brain::run).
    let reversed = state.tick % 2 == 1;
    for _ in 0..COLLISION_ITERATIONS {
        if !relaxation_pass(state, reversed) {
            break;
        }
    }
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
fn relaxation_pass(state: &mut State, reversed: bool) -> bool {
    let n = state.units.len();
    if n < 2 {
        return false;
    }
    // Tile buckets as a sorted list — no hash maps in sim code. Interaction
    // reach is under one tile (radii sum < 1), so 3x3 neighborhoods suffice.
    // Buckets are snapshotted at pass start; corrections are small enough
    // (≤ COLLISION_MAX_STEP) that a newly-adjacent pair simply waits for
    // the next pass.
    let mut by_tile: Vec<(TilePos, usize)> = state
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.hp > 0)
        .map(|(i, u)| (u.tile(), i))
        .collect();
    by_tile.sort_unstable_by_key(|&(t, i)| (t.y, t.x, i));

    let order: Vec<usize> = if reversed {
        (0..n).rev().collect()
    } else {
        (0..n).collect()
    };
    let mut any_overlap = false;
    // Per-unit displacement budget for this pass. Clamping only per pair
    // lets a unit in k overlaps move k × the cap — dense stacks visibly
    // exploded outward. Spent distance is tracked per unit instead, so the
    // cap in stats.rs means what it says.
    let mut spent = vec![Fx::ZERO; n];
    for i in order {
        if state.units[i].hp == 0 {
            continue;
        }
        let home = state.units[i].tile();
        for dy in -1..=1 {
            for dx in -1..=1 {
                let tile = home.offset(dx, dy);
                let start = by_tile.partition_point(|&(t, _)| (t.y, t.x) < (tile.y, tile.x));
                for &(_, j) in by_tile[start..].iter().take_while(|&&(t, _)| t == tile) {
                    if j <= i {
                        continue; // each pair once, in (i, j) id order
                    }
                    let (pos_i, radius_i, id_i, dom_i) = {
                        let u = &state.units[i];
                        (u.pos, u.kind.stats().radius, u.id, u.kind.stats().domain)
                    };
                    let (pos_j, radius_j, id_j, dom_j) = {
                        let u = &state.units[j];
                        (u.pos, u.kind.stats().radius, u.id, u.kind.stats().domain)
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
                    let (dir, overlap) = if dist == Fx::ZERO {
                        let pick = ((id_i.0 ^ id_j.0) % 8) as usize;
                        (STACKED_DIRS[pick], min_dist)
                    } else {
                        (delta / dist, min_dist - dist)
                    };
                    // Anchored units (working in place) yield a sliver;
                    // movers absorb the correction and flow around them.
                    let (share_i, share_j) =
                        match (is_anchored(&state.units[i]), is_anchored(&state.units[j])) {
                            (true, false) => (ANCHORED_PUSH_SHARE, Fx::ONE - ANCHORED_PUSH_SHARE),
                            (false, true) => (Fx::ONE - ANCHORED_PUSH_SHARE, ANCHORED_PUSH_SHARE),
                            _ => (chassis::fx::HALF, chassis::fx::HALF),
                        };
                    let step_j = (overlap * share_j).min(COLLISION_MAX_STEP - spent[j]);
                    if step_j > Fx::ZERO {
                        let away_j = pos_j + dir * step_j;
                        if state.passable_for(dom_j, TilePos::containing(away_j)) {
                            state.units[j].pos = away_j;
                            spent[j] += step_j;
                        }
                    }
                    let step_i = (overlap * share_i).min(COLLISION_MAX_STEP - spent[i]);
                    if step_i > Fx::ZERO {
                        let away_i = pos_i - dir * step_i;
                        if state.passable_for(dom_i, TilePos::containing(away_i)) {
                            state.units[i].pos = away_i;
                            spent[i] += step_i;
                        }
                    }
                }
            }
        }
    }
    any_overlap
}
