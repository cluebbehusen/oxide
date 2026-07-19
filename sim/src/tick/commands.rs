//! Phase 1: command validation and application.
//!
//! Commands mutate *intent* (orders, queues) and nothing else — the later
//! phases do the actual work. Invalid commands are dropped with a
//! [`Event::CommandRejected`]; per-unit problems (a dead id in an otherwise
//! fine selection) are skipped silently, matching how an RTS should feel.

use super::find_nearby_passable;
use crate::command::{Command, PlayerCommand, RejectReason};
use crate::event::Event;
use crate::ids::{PlayerId, Target, UnitId};
use crate::state::{Order, State};
use crate::stats::{GOAL_SNAP_RADIUS, QUEUE_CAP};
use chassis::grid::TilePos;

pub(super) fn apply(state: &mut State, commands: &[PlayerCommand], events: &mut Vec<Event>) {
    for pc in commands {
        if (pc.player.0 as usize) >= state.players.len() {
            continue; // malformed traffic from outside the sim; nothing to attribute
        }
        let outcome = match &pc.command {
            Command::Move { units, goal } => apply_move(state, pc.player, units, *goal),
            Command::Attack { units, target } => apply_attack(state, pc.player, units, *target),
            Command::AttackMove { units, goal } => {
                apply_attack_move(state, pc.player, units, *goal)
            }
            Command::Harvest { units, node } => apply_harvest(state, pc.player, units, *node),
            Command::Stop { units } => apply_stop(state, pc.player, units),
            Command::Train { building, kind } => apply_train(state, pc.player, *building, *kind),
        };
        if let Err(reason) = outcome {
            events.push(Event::CommandRejected {
                player: pc.player,
                reason,
            });
        }
    }
}

/// Iterates the subset of `ids` that exist and belong to `player`, applying
/// `f`. Returns how many units accepted the order.
fn for_owned_units(
    state: &mut State,
    player: PlayerId,
    ids: &[UnitId],
    mut f: impl FnMut(&mut crate::state::Unit),
) -> usize {
    let mut applied = 0;
    for &id in ids {
        if let Some(unit) = state.unit_mut(id)
            && unit.player == player
        {
            f(unit);
            applied += 1;
        }
    }
    applied
}

fn apply_move(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    goal: TilePos,
) -> Result<(), RejectReason> {
    let goal =
        find_nearby_passable(state, goal, GOAL_SNAP_RADIUS).ok_or(RejectReason::UnreachableGoal)?;
    let applied = for_owned_units(state, player, units, |u| {
        u.order = Order::Move { goal };
        u.path = None;
        u.progress = 0;
    });
    (applied > 0)
        .then_some(())
        .ok_or(RejectReason::NoValidUnits)
}

fn apply_attack(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    target: Target,
) -> Result<(), RejectReason> {
    // The target must exist and be an enemy's.
    let (target_owner, target_tile) = match target {
        Target::Unit(id) => state
            .unit(id)
            .map(|u| (u.player, u.tile()))
            .ok_or(RejectReason::InvalidTarget)?,
        Target::Building(id) => state
            .building(id)
            .map(|b| (b.player, b.anchor))
            .ok_or(RejectReason::InvalidTarget)?,
    };
    if target_owner == player {
        return Err(RejectReason::InvalidTarget);
    }
    // Units that can't fight walk to the target area instead.
    let walk_goal = find_nearby_passable(state, target_tile, GOAL_SNAP_RADIUS);
    let applied = for_owned_units(state, player, units, |u| {
        if u.kind.stats().attack.is_some() {
            u.order = Order::Attack {
                target,
                resume: None,
            };
            u.path = None;
            u.progress = 0;
        } else if let Some(goal) = walk_goal {
            u.order = Order::Move { goal };
            u.path = None;
            u.progress = 0;
        }
    });
    (applied > 0)
        .then_some(())
        .ok_or(RejectReason::NoValidUnits)
}

fn apply_attack_move(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    goal: TilePos,
) -> Result<(), RejectReason> {
    let goal =
        find_nearby_passable(state, goal, GOAL_SNAP_RADIUS).ok_or(RejectReason::UnreachableGoal)?;
    let applied = for_owned_units(state, player, units, |u| {
        u.order = if u.kind.stats().attack.is_some() {
            Order::AttackMove { goal }
        } else {
            Order::Move { goal }
        };
        u.path = None;
        u.progress = 0;
    });
    (applied > 0)
        .then_some(())
        .ok_or(RejectReason::NoValidUnits)
}

fn apply_harvest(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    node: TilePos,
) -> Result<(), RejectReason> {
    if state.map.scrap_at(node) == 0 {
        return Err(RejectReason::NotANode);
    }
    let mut applied = 0;
    for &id in units {
        if let Some(unit) = state.unit_mut(id)
            && unit.player == player
            && unit.kind.stats().harvest.is_some()
        {
            unit.order = Order::Harvest { node };
            unit.path = None;
            unit.progress = 0;
            applied += 1;
        }
    }
    (applied > 0)
        .then_some(())
        .ok_or(RejectReason::NoValidUnits)
}

fn apply_stop(state: &mut State, player: PlayerId, units: &[UnitId]) -> Result<(), RejectReason> {
    let applied = for_owned_units(state, player, units, |u| {
        u.order = Order::Idle;
        u.path = None;
        u.progress = 0;
    });
    (applied > 0)
        .then_some(())
        .ok_or(RejectReason::NoValidUnits)
}

fn apply_train(
    state: &mut State,
    player: PlayerId,
    building: crate::ids::BuildingId,
    kind: crate::stats::UnitKind,
) -> Result<(), RejectReason> {
    let cost = kind.stats().cost;
    {
        let b = state
            .building(building)
            .ok_or(RejectReason::NotYourBuilding)?;
        if b.player != player {
            return Err(RejectReason::NotYourBuilding);
        }
        if b.queue.len() >= QUEUE_CAP {
            return Err(RejectReason::QueueFull);
        }
    }
    let bank = &mut state.player_mut(player).scrap;
    if *bank < cost {
        return Err(RejectReason::NotEnoughScrap);
    }
    *bank -= cost;
    state
        .building_mut(building)
        .expect("checked above")
        .queue
        .push(kind);
    Ok(())
}
