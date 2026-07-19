//! Phases 4–5: path following and separation.
//!
//! Movement is pure per-unit work (paths never re-check passability because
//! nothing dynamic blocks tiles: buildings are static in v1 and depleting
//! scrap only ever *opens* tiles). Separation then softly pushes overlapping
//! units apart so grouped units spread out instead of stacking into one
//! sprite — cosmetic-feeling, but it runs inside the sim so it must be as
//! deterministic as everything else.

use crate::state::State;
use chassis::fx::{Fx, Vec2Fx, sqrt};
use chassis::grid::TilePos;

use crate::stats::SEPARATION_MAX_PUSH;

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

/// Pushes overlapping unit pairs apart, half the overlap each, capped per
/// tick. A push that would land in an impassable tile is discarded — rocks
/// beat crowd pressure.
pub(super) fn separate(state: &mut State) {
    let n = state.units.len();
    if n < 2 {
        return;
    }
    // Tile buckets as a sorted list — no hash maps in sim code. Interaction
    // reach is under one tile (radii sum < 1), so 3x3 neighborhoods suffice.
    let mut by_tile: Vec<(TilePos, usize)> = state
        .units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.tile(), i))
        .collect();
    by_tile.sort_unstable_by_key(|&(t, i)| (t.y, t.x, i));

    let units_in_tile = |tile: TilePos| {
        let start = by_tile.partition_point(|&(t, _)| (t.y, t.x) < (tile.y, tile.x));
        by_tile[start..]
            .iter()
            .take_while(move |&&(t, _)| t == tile)
            .map(|&(_, i)| i)
    };

    let mut pushes = vec![Vec2Fx::ZERO; n];
    for i in 0..n {
        let (pos_i, radius_i, id_i) = {
            let u = &state.units[i];
            (u.pos, u.kind.stats().radius, u.id)
        };
        let home = state.units[i].tile();
        for dy in -1..=1 {
            for dx in -1..=1 {
                for j in units_in_tile(home.offset(dx, dy)) {
                    if j <= i {
                        continue; // each pair once, in (i, j) id order
                    }
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
                    let dist = sqrt(dist_sq);
                    let (dir, overlap) = if dist == Fx::ZERO {
                        let pick = ((id_i.0 ^ id_j.0) % 8) as usize;
                        (STACKED_DIRS[pick], min_dist)
                    } else {
                        (delta / dist, min_dist - dist)
                    };
                    let push = dir * (overlap * chassis::fx::HALF);
                    pushes[j] += push;
                    pushes[i] -= push;
                }
            }
        }
    }

    for (i, push) in pushes.into_iter().enumerate() {
        if push == Vec2Fx::ZERO {
            continue;
        }
        let len = push.length();
        let push = if len > SEPARATION_MAX_PUSH {
            push * (SEPARATION_MAX_PUSH / len)
        } else {
            push
        };
        let candidate = state.units[i].pos + push;
        if state.passable(TilePos::containing(candidate)) {
            state.units[i].pos = candidate;
        }
    }
}
