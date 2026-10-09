//! Map audit: usable room per seat, route lengths by movement domain,
//! resource spread, artillery pressure, and spawn spacing. Pace bands use
//! these figures instead of raw dimensions, since a large map with close
//! bases still plays short.

use anyhow::{Context, Result};
use chassis::grid::TilePos;
use chassis::grid::as_index;
use oxide_sim::stats::BuildingKind;
use oxide_sim::{Scenario, State};
use serde::Serialize;
use std::collections::BinaryHeap;

/// One seat's room and spacing.
#[derive(Debug, Serialize)]
pub struct SeatAudit {
    /// Seat index.
    pub seat: u8,
    /// Passable ground tiles reachable from the Foundry doorstep.
    pub reachable_tiles: usize,
    /// Straight-line distance to the nearest scrap node, in tiles.
    pub nearest_scrap: f64,
    /// Shortest route to any enemy Foundry by the cheapest mover:
    /// ground steps, or the air detour (rounded up) when no ground
    /// route exists.
    pub nearest_enemy_route: Option<usize>,
    /// Straight-line distance to the nearest derelict extractor frame,
    /// in tiles; None on maps without frames.
    pub nearest_extractor: Option<f64>,
}

/// A Foundry-to-Foundry route measured both ways of moving.
#[derive(Debug, Serialize)]
pub struct RouteAudit {
    /// Seat pair.
    pub seats: (u8, u8),
    /// Ground route between the doorstep sets, in tile-equivalents; None
    /// when sealed.
    pub ground_steps: Option<usize>,
    /// Air distance between Foundry centers: the straight line unless a
    /// peak forces the sim's air router around, then the detour over
    /// air-passable tiles. None when peaks seal the sky entirely.
    pub air_tiles: Option<f64>,
    /// The longest artillery reach as a fraction of the ground route.
    pub artillery_pressure: Option<f64>,
}

impl RouteAudit {
    /// The distance the cheapest mover pays: ground steps when a ground
    /// route exists, else the air detour rounded up. A built scenario never
    /// yields `None`: the sim's connectivity gate refuses a pair no mover
    /// can reach.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an air detour is a short positive distance"
    )]
    pub fn effective_steps(&self) -> Option<usize> {
        self.ground_steps
            .or_else(|| self.air_tiles.map(|tiles| tiles.ceil() as usize))
    }
}

/// Everything the audit measures for one scenario.
#[derive(Debug, Serialize)]
pub struct MapAudit {
    /// Scenario display name.
    pub name: String,
    /// Grid dimensions.
    pub size: (i32, i32),
    /// Passable ground tiles across the whole map.
    pub free_tiles: usize,
    /// Scrap nodes and their total salvage.
    pub scrap_nodes: usize,
    /// Total scrap on the map.
    pub scrap_total: u64,
    /// Per-seat room and spacing.
    pub seats: Vec<SeatAudit>,
    /// Every cross-team Foundry pair.
    pub routes: Vec<RouteAudit>,
}

/// Longest artillery reach, read from the Bombard, Avalanche and Bastion
/// stats; a new artillery kind joins this list.
fn longest_reach() -> f64 {
    let bombard = oxide_sim::UnitKind::Bombard.stats().weapons.iter();
    let avalanche = oxide_sim::UnitKind::Avalanche.stats().weapons.iter();
    let bastion = BuildingKind::Bastion.base_stats().weapons.iter();
    bombard
        .chain(avalanche)
        .chain(bastion)
        .map(|w| w.range.to_num::<f64>())
        .fold(0.0, f64::max)
}

/// Passable tiles ringing a building footprint, where ground traffic
/// enters and leaves.
fn doorsteps(state: &State, anchor: TilePos, size: (i32, i32)) -> Vec<TilePos> {
    let mut out = Vec::new();
    for dy in -1..=size.1 {
        for dx in -1..=size.0 {
            let inside = (0..size.0).contains(&dx) && (0..size.1).contains(&dy);
            let t = anchor.offset(dx, dy);
            if !inside && state.passable(t) {
                out.push(t);
            }
        }
    }
    out
}

/// Flood fill over ground passability from a doorstep set.
fn reachable_from(state: &State, starts: &[TilePos]) -> usize {
    let (w, h) = (state.map().width(), state.map().height());
    let index = |t: TilePos| as_index(t.y * w + t.x);
    let mut seen = vec![false; as_index(w * h)];
    let mut queue: std::collections::VecDeque<TilePos> = starts.iter().copied().collect();
    for &s in starts {
        seen[index(s)] = true;
    }
    let mut count = 0;
    while let Some(t) = queue.pop_front() {
        count += 1;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let n = t.offset(dx, dy);
                let in_bounds = n.x >= 0 && n.y >= 0 && n.x < w && n.y < h;
                if !in_bounds || seen[index(n)] || !state.passable(n) {
                    continue;
                }
                // The sim's A* forbids corner cutting: a diagonal step
                // is legal only when both cardinal companions are open.
                if dx != 0
                    && dy != 0
                    && !(state.passable(t.offset(dx, 0)) && state.passable(t.offset(0, dy)))
                {
                    continue;
                }
                seen[index(n)] = true;
                queue.push_back(n);
            }
        }
    }
    count
}

/// Shortest ground route between two doorstep sets, in tile-equivalents:
/// Dijkstra under the sim's movement costs (8-connected, no corner
/// cutting, diagonals at 14/10 as in `chassis::path::astar`). Searching
/// from every doorstep keeps a rock on one side of a Foundry from
/// understating or sealing the route.
fn ground_route(state: &State, from: &[TilePos], to: &[TilePos]) -> Option<usize> {
    if from.is_empty() || to.is_empty() {
        return None;
    }
    let (w, h) = (state.map().width(), state.map().height());
    let index = |t: TilePos| as_index(t.y * w + t.x);
    let mut dist: Vec<u32> = vec![u32::MAX; as_index(w * h)];
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(u32, i32, i32)>> =
        BinaryHeap::default();
    for &s in from {
        dist[index(s)] = 0;
        heap.push(std::cmp::Reverse((0, s.x, s.y)));
    }
    let mut target = vec![false; as_index(w * h)];
    for t in to {
        target[index(*t)] = true;
    }
    while let Some(std::cmp::Reverse((d, x, y))) = heap.pop() {
        let t = TilePos::new(x, y);
        if d > dist[index(t)] {
            continue; // stale entry
        }
        if target[index(t)] {
            // Costs run in tenths; report whole tile-equivalents.
            return Some(((d + 5) / 10) as usize);
        }
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let n = t.offset(dx, dy);
                let in_bounds = n.x >= 0 && n.y >= 0 && n.x < w && n.y < h;
                if !in_bounds || !state.passable(n) {
                    continue;
                }
                if dx != 0
                    && dy != 0
                    && !(state.passable(t.offset(dx, 0)) && state.passable(t.offset(0, dy)))
                {
                    continue;
                }
                let step = if dx != 0 && dy != 0 { 14 } else { 10 };
                let nd = d + step;
                if nd < dist[index(n)] {
                    dist[index(n)] = nd;
                    heap.push(std::cmp::Reverse((nd, n.x, n.y)));
                }
            }
        }
    }
    None
}

/// Air distance between two Foundries, center to center, so an asymmetric
/// doorstep ring cannot skew a metric flyers never feel. The straight line
/// serves unless a peak crosses it; then an 8-connected search over
/// air-passable tiles, footprint to footprint, measures the detour the
/// sim's air router would take.
fn air_route(state: &State, a: &oxide_sim::Building, b: &oxide_sim::Building) -> Option<f64> {
    // Uniform-cost search with diagonals at sqrt(2), so the detour
    // reports the same tile unit as the straight line.
    const SQRT2: f64 = std::f64::consts::SQRT_2;
    let center = |f: &oxide_sim::Building| {
        let (w, h) = f.stats().size;
        (
            f64::from(f.anchor.x) + f64::from(w) / 2.0,
            f64::from(f.anchor.y) + f64::from(h) / 2.0,
        )
    };
    let (ax, ay) = center(a);
    let (bx, by) = center(b);
    let euclid = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
    let blocked = chassis::path::line_blocked(
        chassis::fx::Vec2Fx::new(chassis::fx::Fx::from_num(ax), chassis::fx::Fx::from_num(ay)),
        chassis::fx::Vec2Fx::new(chassis::fx::Fx::from_num(bx), chassis::fx::Fx::from_num(by)),
        |t| {
            state
                .map()
                .tile(t)
                .is_none_or(|tile| !tile.terrain.blocks_air())
        },
    );
    if !blocked {
        return Some(euclid);
    }
    let (w, h) = (state.map().width(), state.map().height());
    let index = |t: TilePos| as_index(t.y * w + t.x);
    let open = |t: TilePos| {
        state
            .map()
            .tile(t)
            .is_some_and(|tile| !tile.terrain.blocks_air())
    };
    let mut dist: Vec<f64> = vec![f64::INFINITY; as_index(w * h)];
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(u64, i32, i32)>> =
        BinaryHeap::default();
    // Costs are ordered through their raw bit patterns: all values are
    // non-negative finite floats, where the IEEE ordering agrees with
    // the numeric one.
    for t in a.tiles() {
        dist[index(t)] = 0.0;
        heap.push(std::cmp::Reverse((0u64, t.x, t.y)));
    }
    let mut target = vec![false; as_index(w * h)];
    for t in b.tiles() {
        target[index(t)] = true;
    }
    while let Some(std::cmp::Reverse((bits, x, y))) = heap.pop() {
        let t = TilePos::new(x, y);
        let d = f64::from_bits(bits);
        if d > dist[index(t)] {
            continue; // stale entry
        }
        if target[index(t)] {
            // The open branch measures geometric center to center; the
            // search runs footprint tile to footprint tile. Add both
            // center-to-nearest-tile offsets (sqrt(2)/2 each on a 2x2)
            // so an obstruction can never *shrink* the reported route.
            return Some(d + std::f64::consts::SQRT_2);
        }
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let n = t.offset(dx, dy);
                let in_bounds = n.x >= 0 && n.y >= 0 && n.x < w && n.y < h;
                if !in_bounds || !open(n) {
                    continue;
                }
                // The sim's air A* refuses to cut a diagonal between two
                // touching peaks; the metric must walk the same sky.
                if dx != 0 && dy != 0 && !(open(t.offset(dx, 0)) && open(t.offset(0, dy))) {
                    continue;
                }
                let step = if dx != 0 && dy != 0 { SQRT2 } else { 1.0 };
                let nd = d + step;
                if nd < dist[index(n)] {
                    dist[index(n)] = nd;
                    heap.push(std::cmp::Reverse((nd.to_bits(), n.x, n.y)));
                }
            }
        }
    }
    None
}

/// Audits a built scenario.
pub fn audit(scenario: &Scenario) -> Result<MapAudit> {
    let state = scenario.build().context("building scenario")?;
    let map = state.map();
    let (w, h) = (map.width(), map.height());

    let mut free_tiles = 0;
    let mut scrap_nodes = 0;
    let mut scrap_total: u64 = 0;
    let mut nodes = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let t = TilePos::new(x, y);
            if state.passable(t) {
                free_tiles += 1;
            }
            let amount = map.scrap_at(t);
            if amount > 0 {
                scrap_nodes += 1;
                scrap_total += u64::from(amount);
                nodes.push(t);
            }
        }
    }

    // Every seat's Foundry (and its doorstep set), in seat order.
    let mut steps: Vec<(u8, &oxide_sim::Building, Vec<TilePos>)> = Vec::new();
    for (i, player) in state.players().iter().enumerate() {
        let _ = player;
        let Some(foundry) = state
            .buildings()
            .iter()
            .find(|b| b.player.0 as usize == i && b.kind == BuildingKind::Foundry)
        else {
            continue;
        };
        steps.push((
            u8::try_from(i).expect("seat indices fit in u8"),
            foundry,
            doorsteps(&state, foundry.anchor, foundry.stats().size),
        ));
    }

    let reach = longest_reach();
    let mut routes = Vec::new();
    let mut nearest_enemy: Vec<Option<usize>> = vec![None; steps.len()];
    for i in 0..steps.len() {
        for j in (i + 1)..steps.len() {
            let (sa, sb) = (steps[i].0, steps[j].0);
            if !state.hostile(oxide_sim::PlayerId(sa), oxide_sim::PlayerId(sb)) {
                continue;
            }
            let ground_steps = ground_route(&state, &steps[i].2, &steps[j].2);
            let air_tiles = air_route(&state, steps[i].1, steps[j].1);
            let artillery_pressure = ground_steps.map(|s| reach / s.max(1) as f64);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "an air detour is a short positive distance"
            )]
            let effective = ground_steps.or_else(|| air_tiles.map(|tiles| tiles.ceil() as usize));
            for (seat, slot) in [(i, effective), (j, effective)] {
                nearest_enemy[seat] = match (nearest_enemy[seat], slot) {
                    (Some(cur), Some(new)) => Some(cur.min(new)),
                    (None, new) => new,
                    (cur, None) => cur,
                };
            }
            routes.push(RouteAudit {
                seats: (sa, sb),
                ground_steps,
                air_tiles,
                artillery_pressure,
            });
        }
    }

    let seats = steps
        .iter()
        .enumerate()
        .map(|(idx, (seat, _, doorstep))| {
            // Scrap distance measures from the Foundry's center, not a
            // doorstep — doorstep order is row-major and would report
            // different numbers for mirror-identical seats.
            let center = state
                .buildings()
                .iter()
                .find(|b| b.player.0 == *seat && b.kind == BuildingKind::Foundry)
                .map_or((0.0, 0.0), |b| {
                    let size = b.stats().size;
                    (
                        f64::from(b.anchor.x) + f64::from(size.0) / 2.0,
                        f64::from(b.anchor.y) + f64::from(size.1) / 2.0,
                    )
                });
            let nearest_scrap = nodes
                .iter()
                .map(|n| {
                    let (dx, dy) = (
                        f64::from(n.x) + 0.5 - center.0,
                        f64::from(n.y) + 0.5 - center.1,
                    );
                    (dx * dx + dy * dy).sqrt()
                })
                .fold(f64::INFINITY, f64::min);
            let nearest_extractor = state
                .map()
                .extractor_frames()
                .iter()
                .map(|f| {
                    let (dx, dy) = (
                        f64::from(f.x) + 1.0 - center.0,
                        f64::from(f.y) + 1.0 - center.1,
                    );
                    (dx * dx + dy * dy).sqrt()
                })
                .fold(None, |best: Option<f64>, d| {
                    Some(best.map_or(d, |b| b.min(d)))
                });
            SeatAudit {
                seat: *seat,
                reachable_tiles: reachable_from(&state, doorstep),
                nearest_scrap,
                nearest_enemy_route: nearest_enemy[idx],
                nearest_extractor,
            }
        })
        .collect();

    Ok(MapAudit {
        name: scenario.name.clone(),
        size: (w, h),
        free_tiles,
        scrap_nodes,
        scrap_total,
        seats,
        routes,
    })
}

impl MapAudit {
    /// A one-screen human summary; the JSON form carries the same data.
    pub fn table(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} — {}x{}, {} free tiles, {} nodes ({} scrap)",
            self.name,
            self.size.0,
            self.size.1,
            self.free_tiles,
            self.scrap_nodes,
            self.scrap_total
        );
        for seat in &self.seats {
            let _ = writeln!(
                out,
                "  seat {}: {} reachable tiles, scrap at {:.1}, enemy at {}",
                seat.seat,
                seat.reachable_tiles,
                seat.nearest_scrap,
                seat.nearest_enemy_route
                    .map_or("∞".into(), |s| s.to_string()),
            );
        }
        for route in &self.routes {
            let _ = writeln!(
                out,
                "  {}v{}: ground {} / air {} — artillery pressure {}",
                route.seats.0,
                route.seats.1,
                route
                    .ground_steps
                    .map_or("sealed".into(), |s| s.to_string()),
                route
                    .air_tiles
                    .map_or("sealed".into(), |a| format!("{a:.1}")),
                route
                    .artillery_pressure
                    .map_or("-".into(), |p| format!("{:.0}%", p * 100.0)),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests;
