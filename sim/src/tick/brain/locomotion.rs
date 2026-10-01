//! Getting there: idle auto-acquire, advance and hunt routing,
//! plain walking, contact-propagated arrival, and doorstep approach.

use super::super::landing::{self, Pick, RunIn};
use super::super::reach::Reach;
use super::super::{
    flight, rect_adjacent_tiles, rect_approach_key_from, rect_approach_origin, route_for,
    route_for_position, tile_adjacent_to_rect,
};
use super::combat::{acquire_target, acquire_target_from};
use crate::event::{Event, StallReason};
use crate::ids::{Target, UnitId};
use crate::state::{Goal, Order, PathFollow, State};
use chassis::grid::TilePos;

/// Idle combat units pick fights on their own — on a tether. The
/// leash is set here (and refreshed by retaliation), never by player
/// commands: an explicit attack is a commitment, and `assign` clears
/// any tether the moment a command lands.
pub(super) fn idle(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    id: UnitId,
    acquired: Option<Option<Target>>,
) {
    // A guard back at its post cools down before it looks for the next
    // fight; the leash clears when the cooldown drains — and the guard
    // is instantly STATIONED again (it verifiably stood the whole
    // cooldown), so the dancer finds no untethered window to bait.
    // Idle with a spent tether and no cooldown means the homecoming
    // just finished (walk's arrival advanced the queue) — arm the
    // post stand.
    if let Some(leash) = state.unit(id).expect("caller checked").leash {
        let unit = state.unit_mut(id).expect("caller checked");
        match leash.cooldown {
            0 => {
                unit.leash.as_mut().expect("just seen").cooldown =
                    crate::stats::LEASH_REACQUIRE_COOLDOWN
            }
            1 => {
                unit.leash = None;
                unit.settled = crate::stats::LEASH_STATION_TICKS;
            }
            _ => unit.leash.as_mut().expect("just seen").cooldown -= 1,
        }
        return;
    }
    if let Some(target) = acquired.unwrap_or_else(|| acquire_target(state, index, id)) {
        let unit = state.unit_mut(id).expect("caller checked");
        let anchor = unit.tile();
        let stationed = unit.settled >= crate::stats::LEASH_STATION_TICKS;
        unit.order = Order::Attack {
            pursue: false,
            target: target.into(),
            resume: None,
        };
        unit.path = None;
        unit.settled = 0;
        // Only a stationed guard's fight tethers — a unit cycling
        // through idle mid-battle hunts unleashed, like it always
        // did. No blood yet either way: the warm window starts
        // empty, so a bait that never comes in reach is dropped at
        // the radius line exactly.
        if stationed {
            unit.leash = Some(crate::state::Leash {
                anchor,
                patience: 0,
                cooldown: 0,
            });
        }
    } else {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.settled = unit.settled.saturating_add(1);
        // An airframe with nothing to do orbits for a while, then sets
        // itself down on the nearest clear tile. The order is written
        // directly rather than assigned so `settled` survives: a parked
        // aircraft is a stationed guard whose fights tether to its pad.
        let stats = unit.kind.stats();
        let wants_ground = stats.turn_rate > 0 && !unit.landed && auto_land_probe_due(unit.settled);
        if wants_ground {
            let (tile, pos, heading) = (unit.tile(), unit.pos, unit.heading);
            if let Some(goal) = landing::nearest_landable(
                state,
                stats,
                id,
                tile,
                pos,
                heading,
                crate::stats::AUTO_LAND_SCAN_RADIUS,
                None,
                Pick::StraightIn,
                None,
            ) {
                let unit = state.unit_mut(id).expect("caller checked");
                unit.order = Order::Land { goal, from: None };
                unit.path = None;
            }
        }
    }
}

/// Flies the run-in onto `goal` and sets the airframe down on its center.
/// Touchdown belongs here, not to the steering ring: the final leg is
/// never accepted early, so a pass either meets the center within
/// [`crate::stats::LANDING_TOUCHDOWN`] or flies through and comes around
/// for another run. A go-around keeps `from`, the clicked tile of the walk
/// the landing took over.
pub(super) fn land(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    id: UnitId,
    goal: TilePos,
    from: Option<TilePos>,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if stats.turn_rate == 0 || unit.landed {
        state.unit_mut(id).expect("caller checked").advance_queue();
        return;
    }
    // Nothing sets down with an enemy in reach of the tile: what the
    // approach brings into sight turns the landing into the fight an idle
    // unit would pick, rather than a one-tick touchdown followed by a
    // scramble. Judged from the tile, so a retreat past a gun still
    // completes.
    if let Some(target) = acquire_target_from(state, index, id, goal.center()) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            pursue: false,
            target: target.into(),
            resume: None,
        };
        unit.path = None;
        return;
    }
    let (pos, heading, kind) = (unit.pos, unit.heading, unit.kind);
    let center = goal.center();
    let radius = stats.turn_radius();
    let touchdown = crate::stats::LANDING_TOUCHDOWN;
    if pos.dist_sq(center) <= touchdown * touchdown {
        if !landing::landing_clear(state, stats, id, goal, pos) {
            // The tile filled during the approach: go around onto the
            // nearest clear one, or give up when there is none.
            let next = landing::nearest_landable(
                state,
                stats,
                id,
                goal,
                pos,
                heading,
                crate::stats::LANDING_REPLAN_RADIUS,
                Some(goal),
                Pick::StraightIn,
                None,
            );
            let unit = state.unit_mut(id).expect("caller checked");
            match next {
                Some(goal) => {
                    unit.order = Order::Land { goal, from };
                    unit.path = None;
                }
                None => {
                    let (player, pos) = (unit.player, unit.pos);
                    unit.drop_active_order();
                    events.push(Event::OrderStalled {
                        unit: id,
                        player,
                        pos,
                        reason: StallReason::NoOpenGround,
                    });
                }
            }
            return;
        }
        // Judged where the airframe will actually rest, which is what the
        // validator holds a parked heading to.
        if !flight::escapable(state.map(), pos, heading, radius) {
            // Arrived on a heading no takeoff could fly out of: missed
            // approach, plan a fresh run-in.
            state.unit_mut(id).expect("caller checked").path = None;
            return;
        }
        // The airframe rests where it met the tile: no snap to the center,
        // so the touchdown reads as the end of the run rather than a hop.
        let unit = state.unit_mut(id).expect("caller checked");
        unit.landed = true;
        unit.path = None;
        unit.advance_queue();
        return;
    }
    let on_final = unit
        .path
        .as_ref()
        .is_some_and(|p| p.goal == goal && p.next as usize + 1 == p.waypoints.len());
    if on_final {
        let path = unit.path.as_ref().expect("checked above");
        let len = path.waypoints.len();
        if len >= 2 {
            // The final leg runs from the initial point to the tile. The
            // aircraft chases a carrot on that centerline a couple of turn
            // radii ahead of its own projection, which settles it onto the
            // line from whatever heading it reached the point on, then
            // walks the carrot down to the tile itself.
            let bearing = chassis::compass::heading_of(center - path.waypoints[len - 2].center());
            let v = chassis::compass::dir(bearing);
            let d = center - pos;
            let ahead = v.x * d.x + v.y * d.y;
            if ahead < chassis::fx::Fx::ZERO {
                // Overflew the tile without touching down: the leg is spent.
                state.unit_mut(id).expect("caller checked").path = None;
            } else {
                let lookahead = radius + radius;
                let short = (ahead - lookahead).max(chassis::fx::Fx::ZERO);
                let carrot = TilePos::containing(center - v * short);
                let unit = state.unit_mut(id).expect("caller checked");
                let path = unit.path.as_mut().expect("checked above");
                let last = path.waypoints.len() - 1;
                path.waypoints[last] = carrot;
                path.next = last as u32;
                return;
            }
        } else {
            let hv = chassis::compass::dir(heading);
            let d = center - pos;
            let behind = hv.x * d.x + hv.y * d.y < chassis::fx::Fx::ZERO;
            let accept = stats.turn_acceptance();
            if !(behind && pos.dist_sq(center) <= accept * accept) {
                return;
            }
            state.unit_mut(id).expect("caller checked").path = None;
        }
    }
    if state
        .unit(id)
        .expect("caller checked")
        .path
        .as_ref()
        .is_some_and(|p| p.goal == goal)
    {
        return;
    }
    let route = landing::run_in_route(state, stats, kind, pos, heading, goal, RunIn::Landing);
    let unit = state.unit_mut(id).expect("caller checked");
    match route {
        Some(waypoints) => {
            unit.path = Some(PathFollow {
                final_point: None,
                goal,
                waypoints,
                next: 0,
            });
        }
        None => {
            let (player, pos) = (unit.player, unit.pos);
            unit.drop_active_order();
            events.push(Event::OrderStalled {
                unit: id,
                player,
                pos,
                reason: StallReason::NoRoute,
            });
        }
    }
}

/// March toward the goal, but engage anything that shows up on the way;
/// the attack order remembers the goal and hands it back afterwards.
pub(super) fn hunt(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    reach: &mut Reach,
    id: UnitId,
    goal: Goal,
    events: &mut Vec<Event>,
) {
    if let Some(target) = acquire_target(state, index, id) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            pursue: false,
            target: target.into(),
            resume: Some(goal),
        };
        unit.path = None;
        return;
    }
    if land_at_destination(state, index, reach, id, events) {
        return;
    }
    walk(state, index, reach, id, events);
}

/// A flier's ground destination is a landing. Once the last step of its
/// program is within run-in reach of where it is actually headed and
/// nothing is in acquisition range, the flight hands over to a landing on
/// that tile, or on the nearest landable one. Returns false when the order
/// stands: a queued follow-up, a patrol loop, an enemy in reach, or no
/// ground to park on all keep the plain arrival contract.
///
/// Without a live route, the destination is resolved first, so an
/// unreachable goal never picks its pad around the target itself. A landing
/// that replaces a walk short of its target reports the shortfall the walk
/// would have.
///
/// The pad is chosen only among tiles the owner's team has explored, and
/// only once the goal has taken its slot: until its clicked tile is
/// explored the flier keeps flying toward it. The landing keeps the clicked
/// tile as `from`.
pub(super) fn land_at_destination(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    reach: &mut Reach,
    id: UnitId,
    events: &mut Vec<Event>,
) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let stats = unit.kind.stats();
    if stats.turn_rate == 0 || unit.looping || !unit.queue.is_empty() {
        return false;
    }
    let Some(mut goal) = unit.order.walk_goal() else {
        return false;
    };
    if goal.is_pending() {
        return false;
    }
    let routed = unit
        .path
        .as_ref()
        .is_some_and(|p| p.goal == goal.destination());
    if !routed && let Some(endpoint) = reach.endpoint(state, id, goal.target(), goal.endpoint) {
        goal.settle_for(endpoint);
        store_goal(state, id, goal);
    }
    let destination = goal.destination();
    let unit = state.unit(id).expect("caller checked");
    let handoff = crate::stats::LANDING_HANDOFF_REACH;
    if unit.pos.dist_sq(destination.center()) > handoff * handoff {
        return false;
    }
    if acquire_target_from(state, index, id, destination.center()).is_some() {
        return false;
    }
    let (pos, heading, player) = (unit.pos, unit.heading, unit.player);
    let pad = landing::nearest_landable(
        state,
        stats,
        id,
        destination,
        pos,
        heading,
        crate::stats::GOAL_SNAP_RADIUS,
        None,
        Pick::Nearest,
        Some(player),
    );
    let Some(pad) = pad else {
        return false;
    };
    let unit = state.unit_mut(id).expect("caller checked");
    unit.order = Order::Land {
        goal: pad,
        from: Some(goal.tile()),
    };
    unit.path = None;
    if goal.short() {
        let (player, pos) = (unit.player, unit.pos);
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
    true
}

/// Walks the active order toward its goal and completes it on arrival.
/// A unit close to the goal that bumps into an already-settled arrival also
/// counts as arrived — the whole group parks instead of churning around the
/// click point forever. A goal out of reach completes where the unit got as
/// close as it could.
pub(super) fn walk(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    reach: &mut Reach,
    id: UnitId,
    events: &mut Vec<Event>,
) {
    if let Steer::Done { short } = steer(state, index, reach, id, Approach::Settle) {
        finish(state, id, short, events);
    }
}

/// Ends a walk: the program advances, and an order that ended short of its
/// target reports it once. Patrol laps stay silent, since a looping leg
/// would otherwise report on every pass.
fn finish(state: &mut State, id: UnitId, short: bool, events: &mut Vec<Event>) {
    let unit = state.unit_mut(id).expect("caller checked");
    let (looping, player, pos) = (unit.looping, unit.player, unit.pos);
    unit.advance_queue();
    if short && !looping {
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
            reason: StallReason::NoRoute,
        });
    }
}

/// How a walking order decides it has arrived.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Approach {
    /// The exact tile, a turn-limited flier's acceptance ring, or a settled
    /// crowd; routes start from the body's exact position.
    Settle,
    /// The exact tile only; routes start from the tile center. A transport
    /// disgorges where it stands, so it must stand on the tile.
    Exact,
}

/// Where a walking order stands after one tick of steering.
pub(super) enum Steer {
    /// A route is planned or kept.
    Walking,
    /// The walk is over.
    Done {
        /// The walk ended away from its target: the target is out of
        /// reach, or no route to the nearest reachable tile exists.
        short: bool,
    },
}

/// Steers the active walking order (Run, Hunt, Advance, or Unload)
/// toward its destination.
///
/// Routes go to the target while it is reachable, and otherwise to the
/// nearest reachable tile, which the order stores as its endpoint only
/// while it differs from the target. A reachable walk therefore leaves its
/// order untouched. An arrival short of the target resolves once more in
/// case ground has opened since.
pub(super) fn steer(
    state: &mut State,
    index: &super::super::spatial::UnitIndex,
    reach: &mut Reach,
    id: UnitId,
    approach: Approach,
) -> Steer {
    let unit = state.unit(id).expect("caller checked");
    let Some(mut goal) = unit.order.walk_goal() else {
        return Steer::Done { short: false };
    };
    let target = goal.target();
    if arrived(state, index, reach, id, goal, approach) {
        if !goal.short() {
            return Steer::Done { short: false };
        }
        let destination = goal.destination();
        match reach.endpoint(state, id, target, goal.endpoint) {
            Some(endpoint) if endpoint != destination => {
                goal.settle_for(endpoint);
                store_goal(state, id, goal);
            }
            _ => return Steer::Done { short: true },
        }
    }
    let unit = state.unit(id).expect("caller checked");
    if unit
        .path
        .as_ref()
        .is_some_and(|p| p.goal == goal.destination())
    {
        return Steer::Walking;
    }
    let Some(endpoint) = reach.endpoint(state, id, target, goal.endpoint) else {
        return Steer::Done { short: true };
    };
    goal.settle_for(endpoint);
    store_goal(state, id, goal);
    let unit = state.unit(id).expect("caller checked");
    if unit.tile() == endpoint {
        return Steer::Done {
            short: endpoint != target,
        };
    }
    let (pos, kind) = (unit.pos, unit.kind);
    let from = match approach {
        Approach::Settle => pos,
        Approach::Exact => unit.tile().center(),
    };
    match route_for_position(state, kind, from, endpoint) {
        Some(waypoints) => {
            state.unit_mut(id).expect("caller checked").path = Some(PathFollow {
                final_point: None,
                goal: endpoint,
                waypoints,
                next: 0,
            });
            Steer::Walking
        }
        None => Steer::Done { short: true },
    }
}

/// Writes a resolved goal back onto the active walking order.
fn store_goal(state: &mut State, id: UnitId, goal: Goal) {
    if let Some(slot) = state
        .unit_mut(id)
        .expect("caller checked")
        .order
        .walk_goal_mut()
    {
        *slot = goal;
    }
}

/// Whether the walker has reached its destination under `approach`.
fn arrived(
    state: &State,
    index: &super::super::spatial::UnitIndex,
    reach: &mut Reach,
    id: UnitId,
    goal: Goal,
    approach: Approach,
) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let destination = goal.destination();
    if unit.tile() == destination {
        return true;
    }
    if approach == Approach::Exact {
        return false;
    }
    // A bounded-turn flier cannot promise an exact tile center — the
    // ring the steering integrator accepts is the arrival contract.
    let stats = unit.kind.stats();
    let arced_in = stats.turn_rate > 0 && {
        let accept = stats.turn_acceptance();
        unit.pos.dist_sq(destination.center()) <= accept * accept
    };
    arced_in
        || touching_settled_arrival(state, index, id, destination)
        || (goal.short() && reach.crowd_touches(state, index, id, destination))
}

/// Whether this near-goal unit is in contact with a settled (idle,
/// pathless) unit that itself sits near the same goal — the arrival wave
/// propagates outward from the first unit to park.
fn touching_settled_arrival(
    state: &State,
    index: &super::super::spatial::UnitIndex,
    id: UnitId,
    goal: TilePos,
) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let near_sq = crate::stats::ARRIVAL_NEAR * crate::stats::ARRIVAL_NEAR;
    let goal_center = goal.center();
    if unit.pos.dist_sq(goal_center) > near_sq {
        return false;
    }
    let my_stats = unit.kind.stats();
    let my_radius = my_stats.radius;
    let contact_slack = const { chassis::fx::Fx::lit("0.05") };
    // Contact only means anything between bodies that collide: a flyer
    // hovering over a parked crowd is not "touching" it.
    let reach = crate::stats::ARRIVAL_NEAR.to_num::<i32>() + 1;
    (goal.y - reach..=goal.y + reach).any(|y| {
        index
            .row_span(y, goal.x - reach, goal.x + reach)
            .iter()
            .any(|&(_, slot)| {
                let other = &state.units[slot];
                other.id != id
                    && other.hp > 0
                    && other.domain() == unit.domain()
                    && other.path.is_none()
                    && other.drive_speed == chassis::fx::Fx::ZERO
                    && other.order == Order::Idle
                    && other.pos.dist_sq(goal_center) <= near_sq
                    && unit.pos.dist(other.pos)
                        <= my_radius + other.kind.stats().radius + contact_slack
            })
    })
}

/// Ensures the unit is walking to some passable tile touching the rectangle.
/// Returns false when no ring tile is reachable.
pub(super) fn approach_rect(
    state: &mut State,
    id: UnitId,
    anchor: TilePos,
    size: (i32, i32),
) -> bool {
    let (tile, kind, player) = {
        let u = state.unit(id).expect("caller checked");
        (u.tile(), u.kind, u.player)
    };
    let keep = state
        .unit(id)
        .expect("caller checked")
        .path
        .as_ref()
        .is_some_and(|p| tile_adjacent_to_rect(p.goal, anchor, size));
    if keep {
        return true;
    }
    // Candidate doorsteps, nearest first (stable sort, so ties stay in
    // ring order). The nearest few are then rotated by unit id so a crowd
    // heading for the same rectangle fans out across doorsteps instead of
    // magnetizing onto one tile and jamming — the exact configuration that
    // froze bot economies. Only the near face rotates: a lone unit never
    // detours to the building's far side.
    let domain = kind.stats().domain;
    let mut candidates: Vec<TilePos> = rect_adjacent_tiles(anchor, size)
        .filter(|&t| state.passable_for(domain, t))
        .collect();
    let approach_from = rect_approach_origin(state, player, tile, anchor, size);
    candidates.sort_by_key(|t| rect_approach_key_from(tile, approach_from, anchor, size, *t));
    let near = candidates.len().min(4);
    if near > 1 {
        let rank = crate::ids::owner_local_unit_rank(
            id,
            player,
            state.units.iter().map(|unit| (unit.id, unit.player)),
        );
        candidates[..near].rotate_left(rank % near);
    }
    for goal in candidates {
        if let Some(waypoints) = route_for(state, kind, tile, goal) {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                final_point: None,
                goal,
                waypoints,
                next: 0,
            });
            return true;
        }
    }
    false
}

/// Whether an idle airframe's auto-land ground scan fires this tick.
/// Failed probes retry on a cadence, not every tick: the scan pays full
/// run-in geometry per tile, and an airframe over a sealed or crowded
/// pocket would otherwise pay it forever. `settled` keeps counting while
/// idle, so each unit's phase starts at the moment it went idle and
/// survives the probe-succeeds-but-landing-stalls loop, which returns to
/// idle with `settled` preserved. At saturation the schedule degrades to
/// every tick — the retry period divides the saturated phase — so a very
/// long orbit never stops probing entirely.
fn auto_land_probe_due(settled: u16) -> bool {
    settled >= crate::stats::AUTO_LAND_IDLE_TICKS
        && (settled - crate::stats::AUTO_LAND_IDLE_TICKS)
            .is_multiple_of(crate::stats::AUTO_LAND_RETRY_TICKS)
}

#[cfg(test)]
mod tests {
    use super::auto_land_probe_due;
    use crate::stats::{AUTO_LAND_IDLE_TICKS, AUTO_LAND_RETRY_TICKS};

    #[test]
    fn indexed_arrival_matches_live_neighbors_and_domains() {
        use crate::{Order, PlayerId, Scenario, UnitKind};
        use chassis::fx::{Fx, Vec2Fx};
        use chassis::grid::TilePos;
        let mut state = Scenario::skirmish().build().unwrap();
        state.units.clear();
        let goal = TilePos::new(15, 12);
        let mover = state.spawn_unit(PlayerId(0), UnitKind::Sentinel, goal.center());
        for y in -4..=4 {
            for x in -4..=4 {
                state.spawn_unit(
                    PlayerId(0),
                    if x % 2 == 0 {
                        UnitKind::Sentinel
                    } else {
                        UnitKind::Condor
                    },
                    goal.center() + Vec2Fx::new(Fx::from_num(x), Fx::from_num(y)),
                );
            }
        }
        let mut index = super::super::super::spatial::UnitIndex::new();
        for offset in -12..=12 {
            state.unit_mut(mover).unwrap().pos =
                goal.center() + Vec2Fx::new(Fx::from_num(offset) / 4, Fx::lit("0.49"));
            index.rebuild(&state.units);
            // These facts can change during the brain pass without invalidating
            // the position index. Queries must still read them from the world.
            for phase in 0..4 {
                for (slot, unit) in state.units.iter_mut().enumerate().skip(1) {
                    unit.landed = phase % 2 == 0 && unit.kind == UnitKind::Condor;
                    unit.order = if (slot + phase) % 3 == 0 {
                        Order::Run { goal: goal.into() }
                    } else {
                        Order::Idle
                    };
                    unit.drive_speed = if (slot + phase) % 5 == 0 {
                        Fx::ONE
                    } else {
                        Fx::ZERO
                    };
                }
                let unit = state.unit(mover).unwrap();
                let near_sq = crate::stats::ARRIVAL_NEAR * crate::stats::ARRIVAL_NEAR;
                let expected = unit.pos.dist_sq(goal.center()) <= near_sq
                    && state.units.iter().any(|other| {
                        other.id != mover
                            && other.hp > 0
                            && other.domain() == unit.domain()
                            && other.path.is_none()
                            && other.drive_speed == Fx::ZERO
                            && other.order == Order::Idle
                            && other.pos.dist_sq(goal.center()) <= near_sq
                            && unit.pos.dist(other.pos)
                                <= unit.kind.stats().radius
                                    + other.kind.stats().radius
                                    + Fx::lit("0.05")
                    });
                assert_eq!(
                    super::touching_settled_arrival(&state, &index, mover, goal),
                    expected
                );
            }
        }
    }

    #[test]
    fn coasting_arrival_does_not_complete_a_neighbor_order() {
        use super::{touching_settled_arrival, walk};
        use crate::state::{Order, PathFollow};
        use chassis::fx::{Fx, Vec2Fx};
        use chassis::grid::TilePos;

        let mut state = crate::Scenario::skirmish().build().unwrap();
        state.units.truncate(2);
        let goal = TilePos::new(12, 8);
        for unit in &mut state.units {
            unit.kind = crate::UnitKind::Sentinel;
            unit.order = Order::Run { goal: goal.into() };
            unit.path = Some(PathFollow {
                final_point: None,
                goal,
                waypoints: vec![goal],
                next: 0,
            });
        }
        state.units[0].pos = Vec2Fx::new(Fx::lit("12.01"), Fx::lit("8.5"));
        state.units[0].drive_speed = state.units[0].kind.stats().speed;
        let gap = state.units[0].kind.stats().radius * 2 + Fx::lit("0.02");
        state.units[1].pos = state.units[0].pos - Vec2Fx::new(gap, Fx::ZERO);
        let leader = state.units[0].id;
        let follower = state.units[1].id;
        let mut events = Vec::new();

        let mut index = super::super::super::spatial::UnitIndex::new();
        index.rebuild(&state.units);
        let mut reach = super::super::super::reach::Reach::new(&state);
        walk(&mut state, &index, &mut reach, leader, &mut events);
        assert!(state.units[0].path.is_none());
        assert!(state.units[0].drive_speed > Fx::ZERO);
        assert_ne!(state.units[1].tile(), goal);
        assert!(!touching_settled_arrival(&state, &index, follower, goal));
        walk(&mut state, &index, &mut reach, follower, &mut events);
        assert_eq!(state.units[1].order, Order::Run { goal: goal.into() });

        state.units[0].drive_speed = Fx::ZERO;
        assert!(touching_settled_arrival(&state, &index, follower, goal));
        walk(&mut state, &index, &mut reach, follower, &mut events);
        assert_eq!(state.units[1].order, Order::Idle);
    }

    /// A sandbox over `map` holding one seat and the given units.
    fn sandbox(map: &[&str], units: &[(crate::UnitKind, i32, i32)]) -> crate::State {
        crate::Scenario {
            mode: crate::scenario::ScenarioMode::Sandbox,
            name: "locomotion".into(),
            seed: 5,
            map: map.iter().map(|row| (*row).to_owned()).collect(),
            players: vec![crate::scenario::PlayerSpec {
                name: "p0".into(),
                faction: crate::Faction::Ferrous,
                team: None,
                scrap: 0,
                bot: false,
                bot_config: None,
            }],
            units: units
                .iter()
                .map(|&(kind, x, y)| crate::scenario::UnitSpec {
                    player: 0,
                    kind,
                    x,
                    y,
                })
                .collect(),
            buildings: Vec::new(),
            meta: None,
        }
        .build()
        .expect("locomotion fixture builds")
    }

    /// The shape walking orders had when their goal was a bare tile.
    #[derive(serde::Serialize)]
    #[serde(tag = "order", rename_all = "snake_case")]
    enum LegacyOrder {
        Run { goal: chassis::grid::TilePos },
        Hunt { goal: chassis::grid::TilePos },
        Advance { goal: chassis::grid::TilePos },
        Unload { at: chassis::grid::TilePos },
    }

    #[test]
    fn a_reachable_routed_walk_serializes_exactly_like_the_legacy_tile_order() {
        use crate::{Command, Order, PlayerCommand, PlayerId, UnitKind};
        use chassis::grid::TilePos;
        let map = [".............."; 8];
        let goal = TilePos::new(6, 4);
        type Issue = fn(Vec<crate::UnitId>) -> Command;
        let commands: [(Issue, LegacyOrder); 3] = [
            (
                |units| Command::Run {
                    units,
                    goal: TilePos::new(6, 4),
                    queue: false,
                },
                LegacyOrder::Run { goal },
            ),
            (
                |units| Command::Hunt {
                    units,
                    goal: TilePos::new(6, 4),
                    queue: false,
                },
                LegacyOrder::Hunt { goal },
            ),
            (
                |units| Command::Advance {
                    units,
                    goal: TilePos::new(6, 4),
                    queue: false,
                },
                LegacyOrder::Advance { goal },
            ),
        ];
        for (command, legacy) in commands {
            let mut state = sandbox(&map, &[(UnitKind::Sentinel, 1, 1)]);
            assert!(
                state.vision(PlayerId(0)).explored(goal),
                "premise: an explored click resolves at issue"
            );
            let walker = state.units()[0].id;
            state.tick(&[PlayerCommand {
                player: PlayerId(0),
                command: command(vec![walker]),
            }]);
            let unit = state.unit(walker).unwrap();
            assert!(unit.path.is_some(), "premise: the walk routed");
            assert_eq!(unit.order.walk_goal().unwrap().endpoint, None);
            assert_eq!(
                chassis::hash::state_hash(&unit.order),
                chassis::hash::state_hash(&legacy)
            );
            assert_eq!(
                serde_json::to_value(unit.order).unwrap(),
                serde_json::to_value(&legacy).unwrap()
            );
        }

        let mut state = sandbox(&map, &[(UnitKind::Skyhook, 1, 1)]);
        assert!(state.vision(PlayerId(0)).explored(goal));
        let sling = state.units()[0].id;
        state.tick(&[PlayerCommand {
            player: PlayerId(0),
            command: Command::Unload {
                transport: sling,
                at: goal,
                queue: false,
            },
        }]);
        let unit = state.unit(sling).unwrap();
        assert!(unit.path.is_some(), "premise: the flight routed");
        assert!(matches!(unit.order, Order::Unload { at } if at.endpoint.is_none()));
        assert_eq!(
            chassis::hash::state_hash(&unit.order),
            chassis::hash::state_hash(&LegacyOrder::Unload { at: goal })
        );
    }

    #[test]
    fn a_hovering_flier_sent_onto_a_raw_peak_stops_beside_it() {
        use crate::{Event, Order, StallReason, UnitKind};
        use chassis::grid::TilePos;
        let map = [
            "............",
            "............",
            "....^^^.....",
            "....^^^.....",
            "....^^^.....",
            "............",
            "............",
        ];
        let peak = TilePos::new(5, 3);
        for kind in [UnitKind::Wisp, UnitKind::Skyhook] {
            let mut state = sandbox(&map, &[(kind, 1, 1)]);
            let flier = state.units()[0].id;
            // Commands snap peaks away; a forged or legacy order may not.
            state.unit_mut(flier).unwrap().order = Order::Run { goal: peak.into() };
            let mut stalls = 0;
            for _ in 0..100 {
                let report = state.tick(&[]);
                stalls += report
                    .events
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            Event::OrderStalled {
                                reason: StallReason::NoRoute,
                                ..
                            }
                        )
                    })
                    .count();
                if state.unit(flier).unwrap().order == Order::Idle {
                    break;
                }
            }
            let unit = state.unit(flier).unwrap();
            assert_eq!(unit.order, Order::Idle, "{kind:?} never settled");
            assert_eq!(stalls, 1, "{kind:?} reports the shortfall once");
            assert_eq!(unit.tile(), TilePos::new(5, 1), "{kind:?}");
        }
    }

    #[test]
    fn a_short_walk_ends_on_meeting_the_crowd_parked_at_its_endpoint() {
        use crate::{Event, Goal, Order, StallReason, UnitKind};
        use chassis::fx::{Fx, Vec2Fx};
        use chassis::grid::TilePos;
        // Rock fills columns 10-13, so (9, 1) is the reachable tile nearest
        // the target (12, 1).
        let map = ["..........####"; 3];
        let endpoint = TilePos::new(9, 1);
        let target = TilePos::new(12, 1);
        let home = TilePos::new(0, 0);
        // Four parked bodies run west from the endpoint in touching steps.
        // The walker touches only the last, more than ARRIVAL_NEAR out.
        let run = |goal: Goal, crowd: bool| {
            let bodies = if crowd { 4 } else { 0 };
            let units = vec![(UnitKind::Sentinel, 0, 1); bodies + 1];
            let mut state = sandbox(&map, &units);
            let step = UnitKind::Sentinel.stats().radius * 2;
            for (slot, unit) in state.units.iter_mut().enumerate() {
                let steps = if slot == bodies { 4 } else { slot as i32 };
                unit.pos = endpoint.center() - Vec2Fx::new(step * Fx::from_num(steps), Fx::ZERO);
            }
            let walker = state.units[bodies].id;
            let walker_pos = state.units[bodies].pos;
            assert!(
                walker_pos.dist(endpoint.center()) > crate::stats::ARRIVAL_NEAR,
                "premise: the ordinary arrival wave cannot reach the walker"
            );
            let unit = state.unit_mut(walker).unwrap();
            unit.order = Order::Run { goal };
            unit.queue.push_back(Order::Run { goal: home.into() });
            let mut index = super::super::super::spatial::UnitIndex::new();
            index.rebuild(&state.units);
            let mut reach = super::super::super::reach::Reach::new(&state);
            let mut events = Vec::new();
            super::walk(&mut state, &index, &mut reach, walker, &mut events);
            let stalls = events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        Event::OrderStalled { unit, reason: StallReason::NoRoute, .. }
                            if *unit == walker
                    )
                })
                .count();
            (state.unit(walker).unwrap().order, events.len(), stalls)
        };
        let mut short = Goal::at(target);
        short.endpoint = Some(endpoint);

        assert_eq!(
            run(short, true),
            (Order::Run { goal: home.into() }, 1, 1),
            "meeting the crowd ends the walk once, and the program carries on"
        );
        assert_eq!(
            run(Goal::at(endpoint), true),
            (
                Order::Run {
                    goal: endpoint.into()
                },
                0,
                0
            ),
            "a reachable goal keeps its ordinary arrival reach"
        );
        assert_eq!(
            run(short, false),
            (Order::Run { goal: short }, 0, 0),
            "a walker touching nothing keeps walking"
        );
    }

    #[test]
    fn auto_land_probes_fire_on_the_retry_cadence_not_every_tick() {
        assert!(!auto_land_probe_due(AUTO_LAND_IDLE_TICKS - 1));
        assert!(auto_land_probe_due(AUTO_LAND_IDLE_TICKS));
        for offset in 1..AUTO_LAND_RETRY_TICKS {
            assert!(
                !auto_land_probe_due(AUTO_LAND_IDLE_TICKS + offset),
                "a failed probe must wait out the retry period (offset {offset})"
            );
        }
        assert!(auto_land_probe_due(
            AUTO_LAND_IDLE_TICKS + AUTO_LAND_RETRY_TICKS
        ));
        assert!(
            auto_land_probe_due(u16::MAX),
            "a saturated orbiter degrades to every-tick probing, never to silence"
        );
    }
}
