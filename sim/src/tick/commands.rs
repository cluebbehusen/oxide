//! Phase 1: command validation and application.
//!
//! Commands mutate *intent* (orders, queues) and nothing else — the later
//! phases do the actual work. Invalid commands are dropped with a
//! [`Event::CommandRejected`]; per-unit problems (a dead id in an otherwise
//! fine selection) are skipped silently, matching how an RTS should feel — a
//! repeated id among them, which [`canonical_units`] folds away at dispatch
//! before any handler sees the list.

use super::domain_goal;
use crate::command::{Command, PlayerCommand, RejectReason};
use crate::event::Event;
use crate::ids::{BuildingId, PlayerId, Target, UnitId};
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
        // Alive means a standing Foundry and no concession, matching the
        // victory rule — which also makes a second Surrender reject here.
        if state.players[pc.player.0 as usize].resigned
            || !state
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
                apply_move(state, pc.player, &canonical_units(units), *goal, *queue)
            }
            Command::Attack {
                units,
                target,
                queue,
            } => apply_attack(state, pc.player, &canonical_units(units), *target, *queue),
            Command::AttackMove { units, goal, queue } => {
                apply_attack_move(state, pc.player, &canonical_units(units), *goal, *queue)
            }
            Command::Harvest { units, node, queue } => {
                apply_harvest(state, pc.player, &canonical_units(units), *node, *queue)
            }
            Command::Patrol { units, waypoints } => {
                apply_patrol(state, pc.player, &canonical_units(units), waypoints)
            }
            Command::Build {
                units,
                kind,
                anchor,
                queue,
                defer,
            } => apply_build(
                state,
                pc.player,
                &canonical_units(units),
                *kind,
                *anchor,
                *queue,
                *defer,
            ),
            Command::Cancel { building } => apply_cancel(state, pc.player, *building, events),
            Command::Repair {
                units,
                building,
                queue,
            } => apply_repair(state, pc.player, &canonical_units(units), *building, *queue),
            Command::Salvage {
                units,
                building,
                queue,
            } => apply_salvage(state, pc.player, &canonical_units(units), *building, *queue),
            Command::Stop { units } => apply_stop(state, pc.player, &canonical_units(units)),
            Command::Train { building, kind } => apply_train(state, pc.player, *building, *kind),
            Command::CancelTrain { building, index } => {
                apply_cancel_train(state, pc.player, *building, *index)
            }
            Command::SetRally { building, rally } => {
                apply_set_rally(state, pc.player, *building, *rally)
            }
            Command::Surrender => apply_surrender(state, pc.player, events),
            Command::RepairUnit {
                units,
                target,
                queue,
            } => apply_repair_unit(state, pc.player, &canonical_units(units), *target, *queue),
            Command::Advance { units, goal, queue } => {
                apply_advance(state, pc.player, &canonical_units(units), *goal, *queue)
            }
            Command::FocusFire { buildings, target } => {
                apply_focus_fire(state, pc.player, &canonical_buildings(buildings), *target)
            }
            Command::CancelFound { kind, anchor } => {
                apply_cancel_found(state, pc.player, *kind, *anchor)
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

/// A command's unit list read as the SET it means: id-ordered, each id
/// once. Every unit-bearing command passes through here at dispatch, so no
/// handler can double-apply a repeated id — a duplicate used to append a
/// second queued leg, and on an idle unit it appended a clone of the order
/// it had just been given. Ownership filtering stays in the handlers, whose
/// `NoValidUnits` reporting also weighs what a unit can do.
///
/// The recorded command keeps the client's bytes; this is how the sim
/// *interprets* a list, not a rewrite of it.
fn canonical_units(ids: &[UnitId]) -> Vec<UnitId> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// A command's building list read as the set it means. Validation remains
/// all-or-nothing: canonicalization only removes ordering and duplication,
/// never a bad member.
fn canonical_buildings(ids: &[BuildingId]) -> Vec<BuildingId> {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids
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

/// [`for_owned_units`] narrowed to the labor crew — the three economy
/// verbs all address the same harvesters, and each reports `NoValidUnits`
/// when the selection holds none.
fn for_owned_workers(
    state: &mut State,
    player: PlayerId,
    ids: &[UnitId],
    mut f: impl FnMut(&mut crate::state::Unit),
) -> usize {
    let mut applied = 0;
    for &id in ids {
        if let Some(unit) = state.unit_mut(id)
            && unit.player == player
            && unit.kind.stats().harvest.is_some()
        {
            f(unit);
            applied += 1;
        }
    }
    applied
}

/// The units of `ids` that exist and belong to `player` — the deterministic
/// basis for spread-goal assignment. Order and uniqueness come from the
/// canonical list dispatch built; filtering preserves both.
fn accepted_units(state: &State, player: PlayerId, ids: &[UnitId]) -> Vec<UnitId> {
    ids.iter()
        .copied()
        .filter(|id| state.unit(*id).is_some_and(|u| u.player == player))
        .collect()
}

/// Any command is the player (or bot) speaking: whatever tether a
/// self-acquired fight put on this machine ends here, and station
/// keeping restarts — a commanded machine is on assignment, not
/// standing a post. Runs UNCONDITIONALLY at the head of every verb
/// that writes a unit's program ([`assign`], [`assign_circuit`],
/// [`apply_stop`]) — before `assign`'s no-op early return, because a
/// player re-ordering the exact attack the unit already picked itself
/// compares equal, returns early, and would otherwise silently keep
/// the leash on an explicit commitment. A new program-writing verb
/// must pass through here too, not restate the contract inline.
fn end_station_keeping(unit: &mut crate::state::Unit) {
    unit.leash = None;
    unit.settled = 0;
}

/// Drops the active leg without rotating it into a looping program. This is
/// the edit operation for one explicitly cancelled order, not ordinary order
/// completion.
fn remove_active_order(unit: &mut crate::state::Unit) {
    end_station_keeping(unit);
    unit.order = unit.queue.pop_front().unwrap_or(Order::Idle);
    if matches!(unit.order, Order::Idle) {
        unit.looping = false;
    }
    unit.path = None;
    unit.progress = 0;
}

/// Hands a unit its next order: replacing wipes any queued program;
/// appending parks the order behind the current one (bounded — a hostile
/// stream of appends must not grow memory forever). Returns whether the
/// order actually landed — a full queue drops the append, and the caller
/// reports it instead of pretending.
fn assign(unit: &mut crate::state::Unit, order: Order, queue: bool) -> bool {
    end_station_keeping(unit);
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

/// Hands a unit a whole looping circuit wholesale — patrol's shape:
/// first leg active, the rest queued, path and progress reset. The
/// caller validates the route; `legs` must be non-empty. Shares
/// [`end_station_keeping`] with [`assign`] so the command contract
/// cannot drift between program writers.
fn assign_circuit(unit: &mut crate::state::Unit, mut legs: impl Iterator<Item = Order>) {
    end_station_keeping(unit);
    unit.order = legs.next().expect("caller validated a non-empty route");
    unit.queue = legs.collect();
    unit.looping = true;
    unit.path = None;
    unit.progress = 0;
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

fn apply_advance(
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
                Order::Advance { goal }
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
    // A source counts if it is visible now or the issuer remembers it —
    // ordering harvesters onto stale memory is legitimate play (they walk
    // over, discover the truth, and retarget), while never-seen live
    // salvage must not become a command-success oracle through fog.
    let vision = state.vision(player);
    let known = if vision.visible(node) {
        state.map.scrap_at(node) > 0 || state.map.wreck_at(node) > 0
    } else {
        vision.remembered_scrap(node) > 0 || vision.remembered_wreck(node) > 0
    };
    if !known {
        return Err(RejectReason::NotANode);
    }
    let mut landed = 0;
    let applied = for_owned_workers(state, player, units, |unit| {
        if assign(
            unit,
            Order::Harvest {
                node,
                anchor: Some(node),
                retiring: false,
            },
            queue,
        ) {
            landed += 1;
        }
    });
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
            let legs = snapped.iter().map(|&goal| {
                if can_fight {
                    Order::AttackMove { goal }
                } else {
                    Order::Move { goal }
                }
            });
            assign_circuit(unit, legs);
        }
    }
    if !routed {
        return Err(RejectReason::UnreachableGoal);
    }
    Ok(())
}

/// Claims the site immediately (full price, footprint blocks) and
/// commits the whole accepted crew: the first accepted harvester founds
/// the site — pays, proves a doorstep is reachable — and every other
/// accepted harvester takes the same Build order (builders stack).
/// Aiming at an existing own unfinished site resumes it instead —
/// that's how a dead builder's work gets picked back up. With `defer`,
/// nothing is claimed now: the crew takes [`Order::Found`] and the
/// founder claims through [`found_site`] on arrival.
fn apply_build(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    kind: crate::stats::BuildingKind,
    anchor: TilePos,
    queue: bool,
    defer: bool,
) -> Result<(), RejectReason> {
    if !in_envelope(state, anchor) {
        return Err(RejectReason::OutOfBounds);
    }
    // The crew: every accepted harvester, in id order. `crew[0]` is the
    // founder — the same unit the fresh-placement path always chose.
    let crew: Vec<UnitId> = accepted_units(state, player, units)
        .into_iter()
        .filter(|id| {
            state
                .unit(*id)
                .is_some_and(|u| u.kind.stats().harvest.is_some())
        })
        .collect();
    let &builder = crew.first().ok_or(RejectReason::NoValidUnits)?;

    // Resume an existing site of ours at this anchor? Every hand joins —
    // builders stack, and a dead builder's work is picked back up by
    // however many hands the player sends.
    let existing = state
        .buildings
        .iter()
        .find(|b| b.anchor == anchor && b.kind == kind && b.player == player && !b.built)
        .map(|b| b.id);
    if let Some(site) = existing {
        let mut landed = 0;
        for id in crew {
            if let Some(unit) = state.unit_mut(id)
                && assign(unit, Order::Build { site }, queue)
            {
                landed += 1;
            }
        }
        return (landed > 0).then_some(()).ok_or(RejectReason::QueueFull);
    }
    if defer {
        // The deferred mode: validate against the issuer's KNOWLEDGE,
        // then hand out intent. No site, no charge, no route demand —
        // an unroutable claim stalls honestly at walk time, exactly
        // like a Move into fog. Affordability is judged at arrival
        // too: the bank when the ground is claimed is the bank that
        // matters.
        let replaced_units = if queue { &[][..] } else { crew.as_slice() };
        if state
            .place_intent_refusal_replacing(player, kind, anchor, replaced_units)
            .is_some()
        {
            return Err(RejectReason::BadSite);
        }
        let mut landed = 0;
        for id in crew {
            if let Some(unit) = state.unit_mut(id)
                && assign(unit, Order::Found { kind, anchor }, queue)
            {
                landed += 1;
            }
        }
        return (landed > 0).then_some(()).ok_or(RejectReason::QueueFull);
    }
    if !state.can_place(player, kind, anchor) {
        return Err(RejectReason::BadSite);
    }
    let site = found_site(state, player, builder, kind, anchor, |state, site| {
        // Assign BEFORE paying: a founder whose order queue is
        // full must reject the whole command with the site
        // retracted and nothing spent — the old code discarded
        // this result and could charge for a site nobody was
        // ordered to build.
        let unit = state.unit_mut(builder).expect("filtered above");
        assign(unit, Order::Build { site }, queue)
    })?;
    // The rest of the crew joins best-effort, in id order: an
    // individual full queue drops that hand, never the command —
    // the founder alone gates acceptance.
    for &id in crew.iter().skip(1) {
        if let Some(unit) = state.unit_mut(id) {
            let _ = assign(unit, Order::Build { site }, queue);
        }
    }
    Ok(())
}

/// The one ground-claiming path: place the site, prove a doorstep,
/// commit the builder, pay, bury wreck, and deal walk-less friendlies
/// onto the perimeter. Every rejection retracts the site and leaves no
/// trace on the hash. Serves the instant command path and the deferred
/// founder's arrival identically — `commit_builder` is each caller's
/// own way of putting the founder to work (`false` aborts with the
/// site retracted and nothing spent). The caller has already proved
/// the placement predicate appropriate to its information: `can_place`
/// for instant builds, the arrival re-check for deferred ones.
pub(super) fn found_site(
    state: &mut State,
    player: PlayerId,
    builder: UnitId,
    kind: crate::stats::BuildingKind,
    anchor: TilePos,
    commit_builder: impl FnOnce(&mut State, crate::ids::BuildingId) -> bool,
) -> Result<crate::ids::BuildingId, RejectReason> {
    let cost = kind.stats().construction.ok_or(RejectReason::BadSite)?.cost;
    if state.player(player).scrap < cost {
        return Err(RejectReason::NotEnoughScrap);
    }
    // Place first, then prove the founder can actually reach a
    // doorstep *around the now-blocking footprint* — otherwise
    // undo for free. Charging for a site nobody can ever touch
    // would burn 80% of the price through the hp-scaled refund.
    // (A* tolerates a blocked start, so a founder standing inside
    // the fresh footprint routes out of it like any unit on newly
    // claimed ground.)
    let site = state.place_site(player, kind, anchor);
    let from = state.unit(builder).expect("caller checked").tile();
    let size = kind.stats().size;
    let reachable = super::rect_adjacent_tiles(anchor, size)
        .filter(|&t| state.passable(t))
        .any(|t| from == t || super::astar_for(state, from, t).is_some());
    if !reachable {
        state.retract_site(site);
        return Err(RejectReason::UnreachableGoal);
    }
    if !commit_builder(state, site) {
        state.retract_site(site);
        return Err(RejectReason::QueueFull);
    }
    state.player_mut(player).scrap -= cost;
    // The accepted foundation buries whatever wreck salvage lay
    // there (only now — a rejected site must leave no trace).
    for dy in 0..size.1 {
        for dx in 0..size.0 {
            state.map.clear_wreck(anchor.offset(dx, dy));
        }
    }
    // Friendly machines make way as the site claims the ground: no
    // sim rule expects a resting unit on a claimed footprint. Since
    // 0.13 they WALK off — the builders' own approach and the
    // phase-5 eviction pre-pass both route out of the footprint —
    // so only a body with NO escape route takes the instant deal
    // onto the passable perimeter ring, round-robin in (y, x)
    // order, id order among the dealt: nothing may end up inside a
    // finished building. Strictly after the last rejection path and
    // the payment — a rejected command must not move the state hash
    // (retract_site's contract). Hostiles can't be here: the
    // caller's placement predicate refused them.
    let ring: Vec<TilePos> = {
        let mut ring: Vec<TilePos> = super::rect_adjacent_tiles(anchor, size)
            .filter(|&t| state.passable(t))
            .collect();
        ring.sort_unstable_by_key(|t| (t.y, t.x));
        ring
    };
    let inside = |t: TilePos| {
        t.x >= anchor.x && t.x < anchor.x + size.0 && t.y >= anchor.y && t.y < anchor.y + size.1
    };
    let mut dealt = 0usize;
    for i in 0..state.units.len() {
        let u = &state.units[i];
        if u.hp == 0 || u.kind.stats().domain != crate::stats::Domain::Ground || !inside(u.tile()) {
            continue;
        }
        let (unit_kind, tile) = (u.kind, u.tile());
        if super::movement::escape_route(state, unit_kind, tile).is_some() {
            continue; // it can walk; the eviction pre-pass sees to it
        }
        let Some(&to) = ring.get(dealt % ring.len().max(1)) else {
            continue; // no perimeter at all: leave it; collision resolves
        };
        dealt += 1;
        let unit = &mut state.units[i];
        unit.pos = to.center();
        unit.path = None;
    }
    Ok(site)
}

/// Salvage an unfinished site: refund scales with its current health, so
/// enemy fire on the scaffold burns the owner's money.
fn apply_cancel(
    state: &mut State,
    player: PlayerId,
    building: crate::ids::BuildingId,
    events: &mut Vec<Event>,
) -> Result<(), RejectReason> {
    let refund = {
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
        cost * b.hp / stats.max_hp
    };
    let bank = &mut state.player_mut(player).scrap;
    *bank = bank.saturating_add(refund);
    state.buildings.retain(|b| b.id != building);
    for unit in state.units.iter_mut().filter(|unit| unit.player == player) {
        unit.queue
            .retain(|order| !matches!(order, Order::Build { site } if *site == building));
        if matches!(unit.order, Order::Build { site } if site == building) {
            remove_active_order(unit);
        }
    }
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
    queue: bool,
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
    let applied = for_owned_workers(state, player, units, |unit| {
        if assign(unit, Order::Repair { building }, queue) {
            landed += 1;
        }
    });
    if applied == 0 {
        return Err(RejectReason::NoValidUnits);
    }
    if landed == 0 {
        return Err(RejectReason::QueueFull);
    }
    // Eviction only on a command that actually landed: a REJECTED
    // command must leave the world untouched (a misfiring client once
    // cancelled its own welders with an invalid salvage), and running
    // it after the assignment is safe because the purge only matches
    // the OPPOSING verb.
    purge_opposing_verb(state, player, building, Verb::Salvage);
    Ok(())
}

/// Stripping is for standing, built, own, non-Foundry buildings —
/// unbuilt sites keep [`Command::Cancel`]'s instant refund, and the
/// victory token never comes apart by its own crew's hands.
fn apply_salvage(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    building: crate::ids::BuildingId,
    queue: bool,
) -> Result<(), RejectReason> {
    let b = state
        .building(building)
        .ok_or(RejectReason::NotYourBuilding)?;
    if b.player != player {
        return Err(RejectReason::NotYourBuilding);
    }
    if !b.built || b.kind == crate::stats::BuildingKind::Foundry {
        return Err(RejectReason::InvalidTarget);
    }
    let mut landed = 0;
    let applied = for_owned_workers(state, player, units, |unit| {
        if assign(unit, Order::Salvage { building }, queue) {
            landed += 1;
        }
    });
    if applied == 0 {
        return Err(RejectReason::NoValidUnits);
    }
    if landed == 0 {
        return Err(RejectReason::QueueFull);
    }
    purge_opposing_verb(state, player, building, Verb::Repair);
    Ok(())
}

/// Unit welding is for wounded, own, GROUND machines. Air patients
/// refuse (a harvester cannot stand where a flyer hovers; the ring
/// stand-in machinery is a chase tool, not a service bay), the healthy
/// leave nothing to do, and the patient never joins its own crew.
/// No eviction rule: nothing else targets a friendly unit, and the
/// patient's own orders are deliberately untouched — welding is the
/// crew's job, not a hold order on the wounded.
fn apply_repair_unit(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    target: UnitId,
    queue: bool,
) -> Result<(), RejectReason> {
    let t = state.unit(target).ok_or(RejectReason::InvalidTarget)?;
    let stats = t.kind.stats();
    if t.player != player || t.hp == 0 || t.hp >= stats.max_hp || stats.domain != Domain::Ground {
        return Err(RejectReason::InvalidTarget);
    }
    let crew: Vec<UnitId> = units.iter().copied().filter(|&id| id != target).collect();
    let mut landed = 0;
    let applied = for_owned_workers(state, player, &crew, |unit| {
        if assign(unit, Order::RepairUnit { unit: target }, queue) {
            landed += 1;
        }
    });
    if applied == 0 {
        return Err(RejectReason::NoValidUnits);
    }
    if landed == 0 {
        return Err(RejectReason::QueueFull);
    }
    Ok(())
}

/// The verb a repair/salvage command evicts from its target: the two
/// never share a building, or a welder and a stripper would feed the
/// resolver an oscillator (and the bot's deepest-wound repair pick
/// would re-crew every salvage it sees).
enum Verb {
    Repair,
    Salvage,
}

/// Clears every own unit's orders of the opposing verb on `building` —
/// queued legs are dropped, a matching active order advances to its
/// next leg (the program survives; only the conflicting job dies).
fn purge_opposing_verb(
    state: &mut State,
    player: PlayerId,
    building: crate::ids::BuildingId,
    verb: Verb,
) {
    let conflicts = |o: &Order| match verb {
        Verb::Repair => matches!(o, Order::Repair { building: b } if *b == building),
        Verb::Salvage => matches!(o, Order::Salvage { building: b } if *b == building),
    };
    for unit in state.units.iter_mut().filter(|u| u.player == player) {
        unit.queue.retain(|o| !conflicts(o));
        if conflicts(&unit.order) {
            // A looping program ROTATES the finished order to the back
            // of the queue we just cleaned — strip it again, or a
            // patrolling welder brings the evicted job around forever.
            unit.advance_queue();
            unit.queue.retain(|o| !conflicts(o));
        }
    }
}

fn apply_stop(state: &mut State, player: PlayerId, units: &[UnitId]) -> Result<(), RejectReason> {
    let applied = for_owned_units(state, player, units, |u| {
        end_station_keeping(u);
        u.clear_program();
    });
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
    let bank = &mut state.player_mut(player).scrap;
    *bank = bank.saturating_add(kind.stats().cost);
    Ok(())
}

fn apply_cancel_found(
    state: &mut State,
    player: PlayerId,
    kind: crate::stats::BuildingKind,
    anchor: TilePos,
) -> Result<(), RejectReason> {
    let matches_site = |order: &Order| {
        matches!(order, Order::Found { kind: found_kind, anchor: found_anchor }
            if *found_kind == kind && *found_anchor == anchor)
    };
    let mut removed = false;
    for worker in state.units.iter_mut().filter(|unit| unit.player == player) {
        let before = worker.queue.len();
        worker.queue.retain(|order| !matches_site(order));
        removed |= worker.queue.len() != before;
        if matches_site(&worker.order) {
            remove_active_order(worker);
            removed = true;
        }
    }
    removed.then_some(()).ok_or(RejectReason::InvalidTarget)
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

/// Concession is a fact, not a macro for razing the base: one flag the
/// victory check and the command gate both read. Commands are phase 1
/// and victory phase 10, so a decisive surrender ends the match on its
/// own tick.
fn apply_surrender(
    state: &mut State,
    player: PlayerId,
    events: &mut Vec<Event>,
) -> Result<(), RejectReason> {
    state.player_mut(player).resigned = true;
    events.push(Event::PlayerResigned { player });
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
    if !b.built || b.kind.stats().produces.is_empty() {
        return Err(RejectReason::InvalidTarget);
    }
    // Any tile is a legal rally — spawns snap to walkable ground later, and
    // a scrap-node rally is exactly how auto-harvest is asked for.
    b.rally = rally;
    Ok(())
}

fn apply_focus_fire(
    state: &mut State,
    player: PlayerId,
    buildings: &[BuildingId],
    target: Target,
) -> Result<(), RejectReason> {
    if buildings.is_empty() {
        return Err(RejectReason::NotYourBuilding);
    }

    // Check every defense before writing any preference. A mixed selection
    // containing a stale, foreign, unfinished, or incompatible building is
    // one rejected command, never a partially retasked line.
    let mut weapons = Vec::with_capacity(buildings.len());
    for &id in buildings {
        let building = state.building(id).ok_or(RejectReason::NotYourBuilding)?;
        if building.player != player {
            return Err(RejectReason::NotYourBuilding);
        }
        if !building.built {
            return Err(RejectReason::InvalidTarget);
        }
        let weapon = building
            .kind
            .stats()
            .weapons
            .first()
            .copied()
            .ok_or(RejectReason::InvalidTarget)?;
        weapons.push(weapon);
    }

    let domain = state
        .visible_hostile_target_domain(player, target)
        .ok_or(RejectReason::InvalidTarget)?;
    if weapons.iter().any(|weapon| !weapon.targets.covers(domain)) {
        return Err(RejectReason::InvalidTarget);
    }

    for &id in buildings {
        state.building_mut(id).expect("validated above").focus = Some(target);
    }
    Ok(())
}
