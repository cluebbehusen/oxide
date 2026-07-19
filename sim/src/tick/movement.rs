//! Phases 4–5: path following and collision resolution.
//!
//! Movement is pure per-unit work (paths never re-check passability because
//! nothing dynamic blocks tiles: buildings are static in v1 and depleting
//! scrap only ever *opens* tiles). Collision resolution then pushes
//! overlapping bodies apart until they fit — units are solid to each other,
//! but tiles are only ever blocked by terrain and buildings, so pathfinding
//! stays deadlock-free while crowds physically jostle.

use crate::state::State;
use chassis::fx::{Fx, Vec2Fx, sqrt};
use chassis::grid::TilePos;

use crate::stats::{COLLISION_ITERATIONS, COLLISION_MAX_STEP};

/// Advances every unit along its path by its speed, consuming waypoints
/// exactly (positions land on tile centers, never near them).
pub(super) fn run(state: &mut State) {
    for unit in &mut state.units {
        if unit.hp == 0 {
            continue;
        }
        let mut budget = unit.kind.stats().speed;
        while budget > Fx::ZERO {
            let Some(path) = &mut unit.path else { break };
            let Some(&waypoint) = path.waypoints.get(path.next as usize) else {
                unit.path = None;
                break;
            };
            let center = waypoint.center();
            let dist = unit.pos.dist(center);
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
    for _ in 0..COLLISION_ITERATIONS {
        if !relaxation_pass(state) {
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
fn relaxation_pass(state: &mut State) -> bool {
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

    let mut any_overlap = false;
    for i in 0..n {
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
                    let (pos_i, radius_i, id_i) = {
                        let u = &state.units[i];
                        (u.pos, u.kind.stats().radius, u.id)
                    };
                    let (pos_j, radius_j, id_j) = {
                        let u = &state.units[j];
                        (u.pos, u.kind.stats().radius, u.id)
                    };
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
                    let step = (overlap * chassis::fx::HALF).min(COLLISION_MAX_STEP);
                    let push = dir * step;
                    let away_j = pos_j + push;
                    if state.passable(TilePos::containing(away_j)) {
                        state.units[j].pos = away_j;
                    }
                    let away_i = pos_i - push;
                    if state.passable(TilePos::containing(away_i)) {
                        state.units[i].pos = away_i;
                    }
                }
            }
        }
    }
    any_overlap
}
