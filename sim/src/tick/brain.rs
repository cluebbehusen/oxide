//! Phase 3: unit brains — intent becomes action.
//!
//! Units act strictly in id order. Damage lands the moment an attack
//! resolves, so earlier ids shoot first and a unit reduced to 0 hp loses its
//! action for the tick (it is removed later, in cleanup). Every selection a
//! brain makes (targets, doorstep tiles, replacement nodes) is ordered by an
//! explicit key ending in an id or a position, so there is exactly one
//! possible choice.

use super::{astar_for, rect_adjacent_tiles, tile_adjacent_to_rect};
use crate::event::Event;
use crate::ids::{Target, UnitId};
use crate::state::{Order, PathFollow, State};
use crate::stats::RETARGET_RADIUS;
use chassis::fx::Vec2Fx;
use chassis::grid::TilePos;

pub(super) fn run(state: &mut State, events: &mut Vec<Event>) {
    let ids: Vec<UnitId> = state.units.iter().map(|u| u.id).collect();
    for id in ids {
        let Some(unit) = state.unit(id) else { continue };
        if unit.hp == 0 {
            continue; // killed earlier this tick
        }
        if let Some(unit) = state.unit_mut(id) {
            unit.cooldown = unit.cooldown.saturating_sub(1);
        }
        let order = state.unit(id).expect("just seen").order;
        match order {
            Order::Idle => idle(state, id),
            Order::Move { goal } => walk(state, id, goal, events),
            Order::Harvest { node } => harvest(state, id, node, events),
            Order::Attack { target, resume } => attack(state, id, target, resume, events),
            Order::AttackMove { goal } => attack_move(state, id, goal, events),
        }
    }
}

/// The nearest enemy in this unit's aggro range — units before buildings,
/// ties to the lowest id. `None` for pacifists and empty horizons.
fn acquire_target(state: &State, id: UnitId) -> Option<Target> {
    let unit = state.unit(id).expect("caller checked");
    let atk = unit.kind.stats().attack?;
    let (pos, me) = (unit.pos, unit.player);
    let aggro_sq = atk.aggro_range * atk.aggro_range;

    let unit_target = state
        .units
        .iter()
        .filter(|u| u.player != me && u.hp > 0)
        .map(|u| (pos.dist_sq(u.pos), u.id))
        .filter(|(d, _)| *d <= aggro_sq)
        .min();
    if let Some((_, uid)) = unit_target {
        return Some(Target::Unit(uid));
    }
    state
        .buildings
        .iter()
        .filter(|b| b.player != me && b.hp > 0)
        .map(|b| (pos.dist_sq(b.closest_point_to(pos)), b.id))
        .filter(|(d, _)| *d <= aggro_sq)
        .min()
        .map(|(_, bid)| Target::Building(bid))
}

/// Idle combat units pick fights on their own.
fn idle(state: &mut State, id: UnitId) {
    if let Some(target) = acquire_target(state, id) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target,
            resume: None,
        };
        unit.path = None;
    }
}

/// March toward the goal, but engage anything that shows up on the way;
/// the attack order remembers the goal and hands it back afterwards.
fn attack_move(state: &mut State, id: UnitId, goal: TilePos, events: &mut Vec<Event>) {
    if let Some(target) = acquire_target(state, id) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target,
            resume: Some(goal),
        };
        unit.path = None;
        return;
    }
    walk(state, id, goal, events);
}

/// Walks toward an exact goal tile; going idle on arrival or when no route
/// exists. A unit close to the goal that bumps into an already-settled
/// arrival also counts as arrived — the whole group parks instead of
/// churning around the click point forever.
fn walk(state: &mut State, id: UnitId, goal: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let tile = unit.tile();
    if tile == goal || touching_settled_arrival(state, id, goal) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Idle;
        unit.path = None;
        return;
    }
    let unit = state.unit(id).expect("caller checked");
    let has_fresh_path = unit.path.as_ref().is_some_and(|p| p.goal == goal);
    if has_fresh_path {
        return;
    }
    let tile = unit.tile();
    let path = astar_for(state, tile, goal);
    let unit = state.unit_mut(id).expect("caller checked");
    match path {
        Some(waypoints) => {
            unit.path = Some(PathFollow {
                goal,
                waypoints,
                next: 0,
            });
        }
        None => {
            let (player, pos) = (unit.player, unit.pos);
            unit.order = Order::Idle;
            unit.path = None;
            events.push(Event::OrderStalled {
                unit: id,
                player,
                pos,
            });
        }
    }
}

/// Whether this near-goal unit is in contact with a settled (idle,
/// pathless) unit that itself sits near the same goal — the arrival wave
/// propagates outward from the first unit to park.
fn touching_settled_arrival(state: &State, id: UnitId, goal: TilePos) -> bool {
    let unit = state.unit(id).expect("caller checked");
    let near_sq = crate::stats::ARRIVAL_NEAR * crate::stats::ARRIVAL_NEAR;
    let goal_center = goal.center();
    if unit.pos.dist_sq(goal_center) > near_sq {
        return false;
    }
    let my_radius = unit.kind.stats().radius;
    let contact_slack = chassis::fx::Fx::lit("0.05");
    state.units.iter().any(|other| {
        other.id != id
            && other.hp > 0
            && other.path.is_none()
            && other.order == Order::Idle
            && other.pos.dist_sq(goal_center) <= near_sq
            && unit.pos.dist(other.pos) <= my_radius + other.kind.stats().radius + contact_slack
    })
}

/// The harvest loop: walk to node, extract to capacity, haul to the nearest
/// Foundry, repeat; when the node dies, hop to a neighbor node or go idle.
fn harvest(state: &mut State, id: UnitId, node: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let Some(hstats) = unit.kind.stats().harvest else {
        // Only harvesters ever get this order; be defensive anyway.
        state.unit_mut(id).expect("caller checked").order = Order::Idle;
        return;
    };
    let (tile, carrying) = (unit.tile(), unit.carrying);
    let node_scrap = state.map.scrap_at(node);

    if carrying >= hstats.capacity {
        deliver(state, id, node, events);
    } else if node_scrap > 0 {
        if tile_adjacent_to_rect(tile, node, (1, 1)) {
            extract(state, id, node, hstats.ticks_per_scrap, events);
        } else if !approach_rect(state, id, node, (1, 1)) {
            let unit = state.unit_mut(id).expect("caller checked");
            let (player, pos) = (unit.player, unit.pos);
            unit.order = Order::Idle;
            events.push(Event::OrderStalled {
                unit: id,
                player,
                pos,
            });
        }
    } else {
        // Node is dry. Find a replacement, else wrap up.
        match replacement_node(state, node, tile) {
            Some(next) => {
                let unit = state.unit_mut(id).expect("caller checked");
                unit.order = Order::Harvest { node: next };
                unit.path = None;
                unit.progress = 0;
            }
            None if carrying > 0 => deliver(state, id, node, events),
            None => {
                let unit = state.unit_mut(id).expect("caller checked");
                unit.order = Order::Idle;
                unit.path = None;
            }
        }
    }
}

/// Stand at the node and chip scrap off it.
fn extract(
    state: &mut State,
    id: UnitId,
    node: TilePos,
    ticks_per_scrap: u32,
    events: &mut Vec<Event>,
) {
    let unit = state.unit_mut(id).expect("caller checked");
    unit.path = None;
    unit.progress += 1;
    if unit.progress < ticks_per_scrap {
        return;
    }
    unit.progress = 0;
    unit.carrying += 1;
    if state.map.extract_scrap(node) == Some(0) {
        events.push(Event::NodeDepleted { pos: node });
    }
}

/// Haul the load to the nearest own Foundry; deposit when adjacent. After
/// depositing, resume the node (or go idle if it is gone for good).
fn deliver(state: &mut State, id: UnitId, node: TilePos, events: &mut Vec<Event>) {
    let unit = state.unit(id).expect("caller checked");
    let (tile, me, carrying) = (unit.tile(), unit.player, unit.carrying);
    let pos = unit.pos;

    let nearest = state
        .buildings
        .iter()
        .filter(|b| b.player == me && b.hp > 0)
        .map(|b| (pos.dist_sq(b.center()), b.id))
        .min();
    let Some((_, foundry_id)) = nearest else {
        // Homeless: hold the scrap and stand down.
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Idle;
        unit.path = None;
        return;
    };
    let foundry = state.building(foundry_id).expect("just found");
    let (anchor, size) = (foundry.anchor, foundry.kind.stats().size);

    if tile_adjacent_to_rect(tile, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.carrying = 0;
        unit.progress = 0;
        unit.path = None;
        // Saturating: a hostile scenario can start a bank near u32::MAX.
        // The event reports what was actually credited, not what was
        // carried — at the ceiling those differ.
        let bank = &mut state.player_mut(me).scrap;
        let credited = bank.saturating_add(carrying) - *bank;
        *bank += credited;
        events.push(Event::ScrapDeposited {
            player: me,
            amount: credited,
        });
        // Nothing left to go back to? Then we're done hauling.
        if state.map.scrap_at(node) == 0 && replacement_node(state, node, tile).is_none() {
            state.unit_mut(id).expect("caller checked").order = Order::Idle;
        }
    } else if !approach_rect(state, id, anchor, size) {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.order = Order::Idle;
        unit.path = None;
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
        });
    }
}

/// Chase-and-hit. Range is measured to the target's closest point and damage
/// is immediate. A vanished target hands control back to the remembered
/// attack-move (or idle, where auto-acquire finds the next fight).
fn attack(
    state: &mut State,
    id: UnitId,
    target: Target,
    resume: Option<TilePos>,
    events: &mut Vec<Event>,
) {
    let unit = state.unit(id).expect("caller checked");
    let Some(atk) = unit.kind.stats().attack else {
        state.unit_mut(id).expect("caller checked").order = Order::Idle;
        return;
    };
    let (pos, tile, cooldown) = (unit.pos, unit.tile(), unit.cooldown);

    // An attack-mover pounding a building stays alert: an enemy *unit*
    // wandering into aggro takes priority (deterministic — acquire prefers
    // units), so marching armies fight back instead of tunnel-visioning.
    if resume.is_some()
        && matches!(target, Target::Building(_))
        && let Some(better @ Target::Unit(_)) = acquire_target(state, id)
    {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = Order::Attack {
            target: better,
            resume,
        };
        unit.path = None;
        return;
    }

    // Resolve the target's current position; None means it is gone.
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
    let Some((aim_point, target_tile)) = target_info else {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.order = match resume {
            Some(goal) => Order::AttackMove { goal },
            None => Order::Idle,
        };
        unit.path = None;
        return;
    };

    // In range only counts with a clear line: rock is cover, and buildings
    // (other than the victim itself) block shots. Scrap piles are low junk
    // — fire passes over them. No LOS → keep approaching; the chase path
    // already routes around whatever is in the way.
    let clear_shot = |t: TilePos| {
        let terrain_open = state
            .map
            .tile(t)
            .is_some_and(|tile| tile.terrain != crate::map::Terrain::Rock);
        let building_open = match target {
            Target::Building(bid) => state.building_at(t).is_none_or(|b| b.id == bid),
            _ => state.building_at(t).is_none(),
        };
        terrain_open && building_open
    };
    let in_range = pos.dist_sq(aim_point) <= atk.range * atk.range;
    if in_range && !chassis::path::line_blocked(pos, aim_point, clear_shot) {
        let unit = state.unit_mut(id).expect("caller checked");
        unit.path = None;
        if cooldown > 0 {
            return;
        }
        unit.cooldown = atk.cooldown_ticks;
        match target {
            Target::Unit(uid) => {
                let victim = state.unit_mut(uid).expect("resolved above");
                victim.hp = victim.hp.saturating_sub(atk.damage);
            }
            Target::Building(bid) => {
                let victim = state.building_mut(bid).expect("resolved above");
                victim.hp = victim.hp.saturating_sub(atk.damage);
            }
        }
        events.push(Event::AttackHit {
            attacker: id,
            target,
            attacker_pos: pos,
            target_pos: aim_point,
        });
        return;
    }

    // Out of range: chase.
    let reached = match target {
        Target::Unit(_) => {
            // Repath only when the target has drifted a tile away from the
            // path's goal — cheap pursuit without per-tick A*.
            let stale = state
                .unit(id)
                .expect("caller checked")
                .path
                .as_ref()
                .is_none_or(|p| p.goal.chebyshev(target_tile) > 1);
            if stale {
                let path = astar_for(state, tile, target_tile);
                let unit = state.unit_mut(id).expect("caller checked");
                match path {
                    Some(waypoints) => {
                        unit.path = Some(PathFollow {
                            goal: target_tile,
                            waypoints,
                            next: 0,
                        });
                        true
                    }
                    None => false,
                }
            } else {
                true
            }
        }
        Target::Building(bid) => {
            let b = state.building(bid).expect("resolved above");
            let (anchor, size) = (b.anchor, b.kind.stats().size);
            approach_rect(state, id, anchor, size)
        }
    };
    if !reached {
        let unit = state.unit_mut(id).expect("caller checked");
        let (player, pos) = (unit.player, unit.pos);
        unit.order = Order::Idle;
        unit.path = None;
        events.push(Event::OrderStalled {
            unit: id,
            player,
            pos,
        });
    }
}

/// Ensures the unit is walking to some passable tile touching the rectangle.
/// Returns false when no ring tile is reachable.
fn approach_rect(state: &mut State, id: UnitId, anchor: TilePos, size: (i32, i32)) -> bool {
    let tile = state.unit(id).expect("caller checked").tile();
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
    let mut candidates: Vec<TilePos> = rect_adjacent_tiles(anchor, size)
        .filter(|&t| state.passable(t))
        .collect();
    candidates.sort_by_key(|t| t.chebyshev(tile));
    let near = candidates.len().min(4);
    if near > 1 {
        candidates[..near].rotate_left(id.0 as usize % near);
    }
    for goal in candidates {
        if let Some(waypoints) = astar_for(state, tile, goal) {
            let unit = state.unit_mut(id).expect("caller checked");
            unit.path = Some(PathFollow {
                goal,
                waypoints,
                next: 0,
            });
            return true;
        }
    }
    false
}

/// The nearest tile still holding scrap within [`RETARGET_RADIUS`] of a dead
/// node — keyed by (distance from the unit, y, x) so the pick is unique.
fn replacement_node(state: &State, around: TilePos, unit_tile: TilePos) -> Option<TilePos> {
    let mut best: Option<(i32, i32, i32)> = None;
    for dy in -RETARGET_RADIUS..=RETARGET_RADIUS {
        for dx in -RETARGET_RADIUS..=RETARGET_RADIUS {
            let t = around.offset(dx, dy);
            if state.map.scrap_at(t) == 0 {
                continue;
            }
            let key = (t.manhattan(unit_tile), t.y, t.x);
            if best.is_none_or(|b| key < b) {
                best = Some(key);
            }
        }
    }
    best.map(|(_, y, x)| TilePos::new(x, y))
}
