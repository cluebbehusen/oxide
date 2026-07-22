//! Phase 1: command validation and application.
//!
//! Commands mutate *intent* (orders, queues) and nothing else — the later
//! phases do the actual work. Invalid commands are dropped with a
//! [`Event::CommandRejected`]; per-unit problems (a dead id in an otherwise
//! fine selection) are skipped silently, matching how an RTS should feel.

use super::domain_goal;
use crate::command::{Command, PlayerCommand, RejectReason};
use crate::event::Event;
use crate::ids::{PlayerId, Target, UnitId};
use crate::state::{Order, State};
use crate::stats::{Domain, GOAL_SNAP_RADIUS, ORDER_QUEUE_CAP, QUEUE_CAP};
use chassis::grid::TilePos;

/// Whether a commanded coordinate is sane: on the map or within snap
/// distance of it. Rejecting here keeps hostile i32 extremes away from the
/// neighborhood scans (whose offset arithmetic is unchecked by design).
fn in_envelope(state: &State, pos: TilePos) -> bool {
    let r = GOAL_SNAP_RADIUS;
    pos.x >= -r && pos.y >= -r && pos.x < state.map.width() + r && pos.y < state.map.height() + r
}

pub(super) fn apply(state: &mut State, commands: &[PlayerCommand], events: &mut Vec<Event>) {
    for pc in commands {
        if (pc.player.0 as usize) >= state.players.len() {
            continue; // malformed traffic from outside the sim; nothing to attribute
        }
        // The eliminated don't give orders (matters in 3+ player games —
        // two-player matches freeze on the result before this can bite).
        // Alive means a standing Foundry, matching the victory rule.
        if !state
            .buildings
            .iter()
            .any(|b| b.player == pc.player && b.kind == crate::stats::BuildingKind::Foundry)
        {
            events.push(Event::CommandRejected {
                player: pc.player,
                reason: RejectReason::Eliminated,
            });
            continue;
        }
        let outcome = match &pc.command {
            Command::Move { units, goal, queue } => {
                apply_move(state, pc.player, units, *goal, *queue)
            }
            Command::Attack {
                units,
                target,
                queue,
            } => apply_attack(state, pc.player, units, *target, *queue),
            Command::AttackMove { units, goal, queue } => {
                apply_attack_move(state, pc.player, units, *goal, *queue)
            }
            Command::Harvest { units, node, queue } => {
                apply_harvest(state, pc.player, units, *node, *queue)
            }
            Command::Patrol { units, waypoints } => {
                apply_patrol(state, pc.player, units, waypoints)
            }
            Command::Build {
                units,
                kind,
                anchor,
            } => apply_build(state, pc.player, units, *kind, *anchor),
            Command::Cancel { building } => apply_cancel(state, pc.player, *building, events),
            Command::Repair { units, building } => apply_repair(state, pc.player, units, *building),
            Command::Stop { units } => apply_stop(state, pc.player, units),
            Command::Train { building, kind } => apply_train(state, pc.player, *building, *kind),
            Command::CancelTrain { building, index } => {
                apply_cancel_train(state, pc.player, *building, *index)
            }
            Command::SetRally { building, rally } => {
                apply_set_rally(state, pc.player, *building, *rally)
            }
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

/// The units of `ids` that exist and belong to `player`, deduplicated and
/// in id order — the deterministic basis for spread-goal assignment.
fn accepted_units(state: &State, player: PlayerId, ids: &[UnitId]) -> Vec<UnitId> {
    let mut accepted: Vec<UnitId> = ids
        .iter()
        .copied()
        .filter(|id| state.unit(*id).is_some_and(|u| u.player == player))
        .collect();
    accepted.sort_unstable();
    accepted.dedup();
    accepted
}

/// Hands a unit its next order: replacing wipes any queued program;
/// appending parks the order behind the current one (bounded — a hostile
/// stream of appends must not grow memory forever). Returns whether the
/// order actually landed — a full queue drops the append, and the caller
/// reports it instead of pretending.
fn assign(unit: &mut crate::state::Unit, order: Order, queue: bool) -> bool {
    if queue && !matches!(unit.order, Order::Idle) {
        if unit.queue.len() < ORDER_QUEUE_CAP {
            unit.queue.push_back(order);
            return true;
        }
        return false;
    }
    if !queue {
        unit.queue.clear();
        unit.looping = false;
        // Reissuing the exact current order is a no-op past the queue
        // wipe: progress and path survive. Resetting them let a
        // re-commanded welder heal forever without ever crossing a
        // billing tick, dropped a re-clicked harvester's half-extracted
        // scrap, and threw away perfectly good paths on every army
        // re-push.
        if unit.order == order {
            return true;
        }
    }
    unit.order = order;
    unit.path = None;
    unit.progress = 0;
    true
}

/// The first `count` tiles open to `domain`, ring-scanned outward from
/// `center` — per-unit goals for a group order, so crowds fan out over an
/// area instead of magnetizing onto a single tile. Falls back to
/// repeating the last tile if open ground runs out (they'll jostle;
/// that's honest).
fn spread_goals(state: &State, center: TilePos, count: usize, domain: Domain) -> Vec<TilePos> {
    let mut out = Vec::with_capacity(count);
    'scan: for r in 0..=GOAL_SNAP_RADIUS + 3 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let t = center.offset(dx, dy);
                if state.passable_for(domain, t) {
                    out.push(t);
                    if out.len() == count {
                        break 'scan;
                    }
                }
            }
        }
    }
    while out.len() < count {
        out.push(out.last().copied().unwrap_or(center));
    }
    out
}

/// Splits accepted unit ids by movement domain, preserving id order —
/// each half gets goals its own domain can actually stand on.
fn split_domains(state: &State, ids: Vec<UnitId>) -> [(Vec<UnitId>, Domain); 2] {
    let (ground, air): (Vec<UnitId>, Vec<UnitId>) = ids.into_iter().partition(|&id| {
        state.unit(id).expect("caller filtered").kind.stats().domain == Domain::Ground
    });
    [(ground, Domain::Ground), (air, Domain::Air)]
}

fn apply_move(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    goal: TilePos,
    queue: bool,
) -> Result<(), RejectReason> {
    if !in_envelope(state, goal) {
        return Err(RejectReason::OutOfBounds);
    }
    let accepted = accepted_units(state, player, units);
    if accepted.is_empty() {
        return Err(RejectReason::NoValidUnits);
    }
    let mut landed = 0;
    let mut routed = false;
    for (ids, domain) in split_domains(state, accepted) {
        if ids.is_empty() {
            continue;
        }
        let Some(snapped) = domain_goal(state, goal, domain) else {
            continue; // nowhere for this half to stand; the other may fly
        };
        routed = true;
        let goals = spread_goals(state, snapped, ids.len(), domain);
        for (id, goal) in ids.into_iter().zip(goals) {
            let unit = state.unit_mut(id).expect("filtered above");
            if assign(unit, Order::Move { goal }, queue) {
                landed += 1;
            }
        }
    }
    if !routed {
        return Err(RejectReason::UnreachableGoal);
    }
    (landed > 0).then_some(()).ok_or(RejectReason::QueueFull)
}

fn apply_attack(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    target: Target,
    queue: bool,
) -> Result<(), RejectReason> {
    // The target must exist, be an enemy's, and be visible to the issuer —
    // fog of war means no sniping at things you cannot see.
    let (target_owner, target_tile, seen) = match target {
        Target::Unit(id) => state
            .unit(id)
            .map(|u| (u.player, u.tile(), state.can_see(player, u.tile())))
            .ok_or(RejectReason::InvalidTarget)?,
        Target::Building(id) => state
            .building(id)
            .map(|b| {
                let seen = b.tiles().any(|t| state.can_see(player, t));
                (b.player, b.anchor, seen)
            })
            .ok_or(RejectReason::InvalidTarget)?,
    };
    // Teammates (and yourself) are not targets.
    if !state.hostile(player, target_owner) || !seen {
        return Err(RejectReason::InvalidTarget);
    }
    // Units that can't fight — or whose weapons can't cover the target's
    // domain — walk to the target area instead.
    let victim_domain = match target {
        Target::Unit(id) => state
            .unit(id)
            .map_or(Domain::Ground, |u| u.kind.stats().domain),
        Target::Building(_) => Domain::Ground,
    };
    let walk_goals = [
        domain_goal(state, target_tile, Domain::Ground),
        domain_goal(state, target_tile, Domain::Air),
    ];
    let mut landed = 0;
    let applied = for_owned_units(state, player, units, |u| {
        let stats = u.kind.stats();
        if stats.can_target(victim_domain) {
            if assign(
                u,
                Order::Attack {
                    target,
                    resume: None,
                },
                queue,
            ) {
                landed += 1;
            }
        } else if let Some(goal) = walk_goals[(stats.domain == Domain::Air) as usize]
            && assign(u, Order::Move { goal }, queue)
        {
            landed += 1;
        }
    });
    if applied > 0 && landed == 0 {
        return Err(RejectReason::QueueFull);
    }
    (applied > 0)
        .then_some(())
        .ok_or(RejectReason::NoValidUnits)
}

fn apply_attack_move(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    goal: TilePos,
    queue: bool,
) -> Result<(), RejectReason> {
    if !in_envelope(state, goal) {
        return Err(RejectReason::OutOfBounds);
    }
    let accepted = accepted_units(state, player, units);
    if accepted.is_empty() {
        return Err(RejectReason::NoValidUnits);
    }
    let mut landed = 0;
    let mut routed = false;
    for (ids, domain) in split_domains(state, accepted) {
        if ids.is_empty() {
            continue;
        }
        let Some(snapped) = domain_goal(state, goal, domain) else {
            continue;
        };
        routed = true;
        let goals = spread_goals(state, snapped, ids.len(), domain);
        for (id, goal) in ids.into_iter().zip(goals) {
            let unit = state.unit_mut(id).expect("filtered above");
            let order = if unit.kind.stats().can_fight() {
                Order::AttackMove { goal }
            } else {
                Order::Move { goal }
            };
            if assign(unit, order, queue) {
                landed += 1;
            }
        }
    }
    if !routed {
        return Err(RejectReason::UnreachableGoal);
    }
    (landed > 0).then_some(()).ok_or(RejectReason::QueueFull)
}

fn apply_harvest(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    node: TilePos,
    queue: bool,
) -> Result<(), RejectReason> {
    if !in_envelope(state, node) {
        return Err(RejectReason::OutOfBounds);
    }
    // A source counts if it exists *or* the issuer remembers it existing —
    // ordering harvesters onto stale memory is legitimate play (they walk
    // over, discover the truth, and retarget), and rejecting it would leak
    // that an unseen node or wreck has been emptied.
    let live = state.map.scrap_at(node) > 0 || state.map.wreck_at(node) > 0;
    let remembered = state.vision(player).remembered_scrap(node) > 0
        || state.vision(player).remembered_wreck(node) > 0;
    if !live && !remembered {
        return Err(RejectReason::NotANode);
    }
    let mut applied = 0;
    let mut landed = 0;
    for &id in units {
        if let Some(unit) = state.unit_mut(id)
            && unit.player == player
            && unit.kind.stats().harvest.is_some()
        {
            if assign(unit, Order::Harvest { node }, queue) {
                landed += 1;
            }
            applied += 1;
        }
    }
    if applied == 0 {
        return Err(RejectReason::NoValidUnits);
    }
    (landed > 0).then_some(()).ok_or(RejectReason::QueueFull)
}

/// Walk a looping circuit: every waypoint must snap to open ground, and
/// the whole route is one program — combat units attack-move each leg,
/// pacifists walk them obliviously.
fn apply_patrol(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    waypoints: &[TilePos],
) -> Result<(), RejectReason> {
    if waypoints.is_empty() || waypoints.len() > ORDER_QUEUE_CAP {
        return Err(RejectReason::UnreachableGoal);
    }
    if waypoints.iter().any(|w| !in_envelope(state, *w)) {
        return Err(RejectReason::OutOfBounds);
    }
    let accepted = accepted_units(state, player, units);
    if accepted.is_empty() {
        return Err(RejectReason::NoValidUnits);
    }
    let mut routed = false;
    for (ids, domain) in split_domains(state, accepted) {
        if ids.is_empty() {
            continue;
        }
        let snapped: Vec<TilePos> = waypoints
            .iter()
            .filter_map(|w| domain_goal(state, *w, domain))
            .collect();
        if snapped.len() != waypoints.len() {
            continue; // a leg this domain can't stand on grounds the route
        }
        routed = true;
        for id in ids {
            let unit = state.unit_mut(id).expect("filtered above");
            let can_fight = unit.kind.stats().can_fight();
            let mut legs = snapped.iter().map(|&goal| {
                if can_fight {
                    Order::AttackMove { goal }
                } else {
                    Order::Move { goal }
                }
            });
            unit.order = legs.next().expect("validated non-empty");
            unit.queue = legs.collect();
            unit.looping = true;
            unit.path = None;
            unit.progress = 0;
        }
    }
    if !routed {
        return Err(RejectReason::UnreachableGoal);
    }
    Ok(())
}

/// Claims the site immediately (full price, footprint blocks) and sends
/// the first accepted harvester to stand it up. Aiming at an existing own
/// unfinished site resumes it instead — that's how a dead builder's work
/// gets picked back up.
fn apply_build(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    kind: crate::stats::BuildingKind,
    anchor: TilePos,
) -> Result<(), RejectReason> {
    if !in_envelope(state, anchor) {
        return Err(RejectReason::OutOfBounds);
    }
    let builder = accepted_units(state, player, units)
        .into_iter()
        .find(|id| {
            state
                .unit(*id)
                .is_some_and(|u| u.kind.stats().harvest.is_some())
        })
        .ok_or(RejectReason::NoValidUnits)?;

    // Resume an existing site of ours at this anchor?
    let existing = state
        .buildings
        .iter()
        .find(|b| b.anchor == anchor && b.kind == kind && b.player == player && !b.built)
        .map(|b| b.id);
    let site = match existing {
        Some(site) => site,
        None => {
            let cost = kind.stats().construction.ok_or(RejectReason::BadSite)?.cost;
            if !state.can_place(player, kind, anchor) {
                return Err(RejectReason::BadSite);
            }
            if state.player(player).scrap < cost {
                return Err(RejectReason::NotEnoughScrap);
            }
            // Place first, then prove the builder can actually reach a
            // doorstep *around the now-blocking footprint* — otherwise
            // undo for free. Charging for a site nobody can ever touch
            // would burn 80% of the price through the hp-scaled refund.
            let site = state.place_site(player, kind, anchor);
            let from = state.unit(builder).expect("filtered above").tile();
            let size = kind.stats().size;
            let reachable = super::rect_adjacent_tiles(anchor, size)
                .filter(|&t| state.passable(t))
                .any(|t| from == t || super::astar_for(state, from, t).is_some());
            if !reachable {
                state.retract_site(site);
                return Err(RejectReason::UnreachableGoal);
            }
            state.player_mut(player).scrap -= cost;
            // The accepted foundation buries whatever wreck salvage lay
            // there (only now — a rejected site must leave no trace).
            let size = kind.stats().size;
            for dy in 0..size.1 {
                for dx in 0..size.0 {
                    state.map.clear_wreck(anchor.offset(dx, dy));
                }
            }
            site
        }
    };
    let unit = state.unit_mut(builder).expect("filtered above");
    assign(unit, Order::Build { site }, false);
    Ok(())
}

/// Salvage an unfinished site: refund scales with its current health, so
/// enemy fire on the scaffold burns the owner's money.
fn apply_cancel(
    state: &mut State,
    player: PlayerId,
    building: crate::ids::BuildingId,
    events: &mut Vec<Event>,
) -> Result<(), RejectReason> {
    let b = state
        .building(building)
        .ok_or(RejectReason::NotYourBuilding)?;
    if b.player != player {
        return Err(RejectReason::NotYourBuilding);
    }
    if b.built {
        return Err(RejectReason::BadSite);
    }
    let stats = b.kind.stats();
    let cost = stats.construction.expect("sites are buildable kinds").cost;
    let refund = cost * b.hp / stats.max_hp;
    let bank = &mut state.player_mut(player).scrap;
    *bank = bank.saturating_add(refund);
    state.buildings.retain(|b| b.id != building);
    events.push(Event::BuildCancelled {
        building,
        player,
        refund,
    });
    Ok(())
}

/// Welding is for standing, wounded, own buildings; sites are resumed
/// through Build instead, and full health leaves nothing to do.
fn apply_repair(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    building: crate::ids::BuildingId,
) -> Result<(), RejectReason> {
    let b = state
        .building(building)
        .ok_or(RejectReason::NotYourBuilding)?;
    if b.player != player {
        return Err(RejectReason::NotYourBuilding);
    }
    if !b.built || b.hp >= b.kind.stats().max_hp {
        return Err(RejectReason::InvalidTarget);
    }
    let mut landed = 0;
    let mut applied = 0;
    for &id in units {
        if let Some(unit) = state.unit_mut(id)
            && unit.player == player
            && unit.kind.stats().harvest.is_some()
        {
            if assign(unit, Order::Repair { building }, false) {
                landed += 1;
            }
            applied += 1;
        }
    }
    if applied == 0 {
        return Err(RejectReason::NoValidUnits);
    }
    (landed > 0).then_some(()).ok_or(RejectReason::QueueFull)
}

fn apply_stop(state: &mut State, player: PlayerId, units: &[UnitId]) -> Result<(), RejectReason> {
    let applied = for_owned_units(state, player, units, |u| u.clear_program());
    (applied > 0)
        .then_some(())
        .ok_or(RejectReason::NoValidUnits)
}

fn apply_cancel_train(
    state: &mut State,
    player: PlayerId,
    building: crate::ids::BuildingId,
    index: u8,
) -> Result<(), RejectReason> {
    let kind = {
        let b = state
            .building(building)
            .ok_or(RejectReason::NotYourBuilding)?;
        if b.player != player {
            return Err(RejectReason::NotYourBuilding);
        }
        *b.queue
            .get(index as usize)
            .ok_or(RejectReason::InvalidTarget)?
    };
    let b = state.building_mut(building).expect("checked above");
    b.queue.remove(index as usize);
    if index == 0 {
        // The next in line starts fresh; half-built progress is not a
        // thing that transfers between machines.
        b.progress = 0;
    }
    state.player_mut(player).scrap += kind.stats().cost;
    Ok(())
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
        if !b.built || !b.kind.stats().produces.contains(&kind) {
            return Err(RejectReason::CannotProduce);
        }
        // The produces lists carry every faction's variant of a role; the
        // seat's faction decides which of them it may actually train.
        if let Some(faction) = kind.faction()
            && faction != state.player(player).faction
        {
            return Err(RejectReason::WrongFaction);
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
        .push_back(kind);
    Ok(())
}

fn apply_set_rally(
    state: &mut State,
    player: PlayerId,
    building: crate::ids::BuildingId,
    rally: Option<TilePos>,
) -> Result<(), RejectReason> {
    if let Some(rally) = rally
        && !in_envelope(state, rally)
    {
        return Err(RejectReason::OutOfBounds);
    }
    let b = state
        .building_mut(building)
        .ok_or(RejectReason::NotYourBuilding)?;
    if b.player != player {
        return Err(RejectReason::NotYourBuilding);
    }
    // Any tile is a legal rally — spawns snap to walkable ground later, and
    // a scrap-node rally is exactly how auto-harvest is asked for.
    b.rally = rally;
    Ok(())
}
