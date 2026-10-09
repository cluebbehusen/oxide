//! Where a clicked tile sends each member of a group.
//!
//! A tile goal keeps the tile the player clicked. Each movement domain's half
//! of a group snaps that tile to ground its units can stand on and spreads
//! its members over the open tiles around it, one slot per member in id
//! order, scanned in the group's approach frame. The spread is decided only
//! on ground the owner's team has explored: a click into unexplored ground
//! sends every member to the clicked tile itself, and [`expose`] hands each
//! its slot at the end of the tick that explores the tile. Resolution reads
//! the real map, like every route.

use crate::ids::{PlayerId, UnitId};
use crate::state::{Aim, Goal, Order, State};
use crate::stats::{Domain, GOAL_SNAP_RADIUS};
use chassis::grid::TilePos;
use std::collections::BTreeMap;

/// Radius of the spread around a snapped center.
const SPREAD_RADIUS: i32 = GOAL_SNAP_RADIUS + 3;

/// Every tile within [`SPREAD_RADIUS`]: no spread holds more slots.
pub(super) const MAX_SLOTS: usize = ((2 * SPREAD_RADIUS + 1) * (2 * SPREAD_RADIUS + 1)) as usize;

/// A group member's goal on a clicked tile, decided once for its whole
/// domain half.
pub(super) struct Issue {
    tile: TilePos,
    reverse: bool,
    /// The spread slots, decided now when the owner's team has explored the
    /// tile.
    slots: Option<Vec<TilePos>>,
}

impl Issue {
    /// The goal of the member at `rank`, its place among its domain half in
    /// id order.
    pub(super) fn goal(&self, rank: usize) -> Goal {
        match &self.slots {
            Some(slots) => resolved(self.tile, slots, rank),
            None => Goal::pending(
                self.tile,
                u8::try_from(rank).unwrap_or(u8::MAX),
                self.reverse,
            ),
        }
    }
}

/// `player`'s click on `tile` for its units of `domain`, whose spread scan
/// runs half-turned when `reverse`.
pub(super) fn issue(
    state: &State,
    player: PlayerId,
    tile: TilePos,
    domain: Domain,
    reverse: bool,
) -> Issue {
    let slots = state
        .vision(player)
        .explored(tile)
        .then(|| slots(state, tile, domain, reverse));
    Issue {
        tile,
        reverse,
        slots,
    }
}

/// Every spread slot around `clicked` for `domain` in the frame `reverse`,
/// in rank order and padded with the last to [`MAX_SLOTS`]. Empty when
/// nothing near the clicked tile is open to the domain: the group then
/// heads for the clicked tile and settles as close as it can.
pub(super) fn slots(
    state: &State,
    clicked: TilePos,
    domain: Domain,
    reverse: bool,
) -> Vec<TilePos> {
    group_domain_goal(state, clicked, domain, reverse).map_or_else(Vec::new, |center| {
        spread_goals_by(center, MAX_SLOTS, reverse, |t| {
            state.passable_for(domain, t)
        })
    })
}

/// The goal of the member at `rank` on `clicked` once its slots are known.
fn resolved(clicked: TilePos, slots: &[TilePos], rank: usize) -> Goal {
    match slots.get(rank).or(slots.last()) {
        Some(&slot) => Goal::slotted(clicked, slot),
        None => Goal::at(clicked),
    }
}

/// Hands every pending goal whose clicked tile its owner's team has now
/// explored the slot it was waiting for, in active orders, queued orders,
/// and the marches engagements will resume. The endpoint resolved for the
/// clicked tile goes with it. Runs at the end of the tick, after vision, so
/// no pending goal names an explored tile at a tick boundary.
pub(super) fn expose(state: &mut State) {
    let mut memo: BTreeMap<(TilePos, bool, bool), Vec<TilePos>> = BTreeMap::new();
    for index in 0..state.units.len() {
        let unit = &state.units[index];
        if !std::iter::once(&unit.order)
            .chain(&unit.queue)
            .any(holds_pending)
        {
            continue;
        }
        let (player, domain) = (unit.player, unit.kind.stats().domain);
        let mut order = unit.order;
        let mut queue = std::mem::take(&mut state.units[index].queue);
        {
            let state = &*state;
            let vision = state.vision(player);
            let mut resolve = |goal: &mut Goal| {
                let Aim::Pending { rank, reverse } = goal.aim else {
                    return;
                };
                let tile = goal.tile();
                if !vision.explored(tile) {
                    return;
                }
                let slots = memo
                    .entry((tile, domain == Domain::Air, reverse))
                    .or_insert_with(|| slots(state, tile, domain, reverse));
                *goal = resolved(tile, slots, usize::from(rank));
            };
            for order in std::iter::once(&mut order).chain(queue.iter_mut()) {
                if let Some(goal) = order_goal_mut(order) {
                    resolve(goal);
                }
            }
        }
        let unit = &mut state.units[index];
        unit.order = order;
        unit.queue = queue;
    }
}

/// Whether an order carries a goal still waiting for its slot.
fn holds_pending(order: &Order) -> bool {
    match order {
        Order::Attack { resume, .. } => resume.is_some_and(|goal| goal.is_pending()),
        _ => order.walk_goal().is_some_and(|goal| goal.is_pending()),
    }
}

/// The tile goal an order carries: a walk's own, or the march an
/// engagement resumes.
fn order_goal_mut(order: &mut Order) -> Option<&mut Goal> {
    match order {
        Order::Attack { resume, .. } => resume.as_mut(),
        _ => order.walk_goal_mut(),
    }
}

/// The first tiles `legal` accepts, ring-scanned outward from `center` in
/// the commanded group's approach frame, padded with the last to `count`.
/// Mirrored groups therefore receive mirrored slots instead of inheriting an
/// absolute northwest-first bias.
pub(super) fn spread_goals_by(
    center: TilePos,
    count: usize,
    reverse: bool,
    legal: impl Fn(TilePos) -> bool,
) -> Vec<TilePos> {
    let mut out: Vec<TilePos> = ring_scan(center, SPREAD_RADIUS, reverse)
        .filter(|&t| legal(t))
        .take(count)
        .collect();
    while out.len() < count {
        out.push(out.last().copied().unwrap_or(center));
    }
    out
}

/// Whether the canonical spread scan needs a half-turn for this group's
/// approach. The first selected unit not already at the goal defines the
/// frame; an owned Foundry is the deterministic fallback for a stacked group.
pub(super) fn spread_scan_reversed(state: &State, center: TilePos, ids: &[UnitId]) -> bool {
    let player = state
        .unit(ids[0])
        .expect("a spread has at least one accepted unit")
        .player;
    let foundry = state
        .buildings
        .iter()
        .find(|building| {
            building.player == player
                && !building.provisional
                && building.kind == crate::stats::BuildingKind::Foundry
        })
        .map(|building| (building.anchor, building.kind.size()));
    super::group_spread_scan_reversed(
        center,
        ids.iter()
            .map(|id| state.unit(*id).expect("accepted spread unit exists").tile()),
        foundry,
        (state.map.width(), state.map.height()),
        player,
    )
}

/// Resolve a blocked group-command center in the same approach frame used
/// to spread its individual slots.
pub(super) fn group_domain_goal(
    state: &State,
    goal: TilePos,
    domain: Domain,
    reverse: bool,
) -> Option<TilePos> {
    let (center, radius) = match domain {
        Domain::Ground => (goal, GOAL_SNAP_RADIUS),
        Domain::Air => (
            TilePos::new(
                goal.x.clamp(0, state.map.width() - 1),
                goal.y.clamp(0, state.map.height() - 1),
            ),
            crate::stats::AIR_GOAL_SNAP_RADIUS,
        ),
    };
    ring_scan(center, radius, reverse).find(|&t| state.passable_for(domain, t))
}

/// Tiles in Chebyshev rings around `center` out to `radius`, each ring in
/// [`ring`] order, or the half-turn of that order when `reverse`. A mirrored
/// caller passing the mirrored center and the opposite `reverse` visits the
/// mirrored tiles in the same sequence.
pub(super) fn ring_scan(
    center: TilePos,
    radius: i32,
    reverse: bool,
) -> impl Iterator<Item = TilePos> {
    (0..=radius).flat_map(ring).map(move |(dx, dy)| {
        if reverse {
            center.offset(-dx, -dy)
        } else {
            center.offset(dx, dy)
        }
    })
}

/// The offsets of Chebyshev ring `r` in the spread-slot scan order: rows top
/// to bottom, columns left to right.
pub(super) fn ring(r: i32) -> impl Iterator<Item = (i32, i32)> {
    let top = (-r..=r).map(move |dx| (dx, -r));
    let sides = (1 - r..r).flat_map(move |dy| [(-r, dy), (r, dy)]);
    let bottom = (-r..=r).map(move |dx| (dx, r)).filter(move |_| r > 0);
    top.chain(sides).chain(bottom)
}

#[cfg(test)]
mod tests;
