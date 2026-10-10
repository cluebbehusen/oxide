//! Command validation and application.
//!
//! Commands mutate intent (orders, queues) and nothing else; later phases do
//! the work. Invalid commands are dropped with a
//! [`Event::CommandRejected`]. Per-unit problems (a dead id in an otherwise
//! valid selection) are skipped silently, and [`canonical_units`] folds away
//! repeated ids at dispatch before any handler sees the list.

use super::goals::{self, spread_scan_reversed};
use crate::command::{Command, PlayerCommand, RejectReason};
use crate::event::Event;
use crate::ids::{AttackTarget, BuildingId, PlayerId, UnitId};
use crate::state::{Goal, Order, OrderKey, State, Unit};
use crate::stats::{Domain, GOAL_SNAP_RADIUS, ORDER_QUEUE_CAP, QUEUE_CAP};
use chassis::grid::TilePos;

/// Whether a Build anchor or Harvest node is sane: on the map or within snap
/// distance of it. Rejecting here keeps hostile i32 extremes away from the
/// neighborhood scans (whose offset arithmetic is unchecked by design).
fn in_envelope(state: &State, pos: TilePos) -> bool {
    let r = GOAL_SNAP_RADIUS;
    pos.x >= -r && pos.y >= -r && pos.x < state.map.width() + r && pos.y < state.map.height() + r
}

/// Whether a tile goal lies on the map. A goal keeps the tile it names, and
/// only a tile on the map can ever be explored and take its spread slot.
fn on_map(state: &State, pos: TilePos) -> bool {
    pos.x >= 0 && pos.y >= 0 && pos.x < state.map.width() && pos.y < state.map.height()
}

pub(super) fn apply(state: &mut State, commands: &[PlayerCommand], events: &mut Vec<Event>) {
    for pc in commands {
        if (pc.player.0 as usize) >= state.players.len() {
            continue; // malformed traffic from outside the sim; nothing to attribute
        }
        // Eliminated seats don't give orders (two-player matches end on the
        // result first, so this matters with three or more seats). Sandbox
        // seats need no Foundry, but surrender still relinquishes control.
        if !state.accepts_commands(pc.player) {
            events.push(Event::CommandRejected {
                player: pc.player,
                reason: RejectReason::Eliminated,
            });
            continue;
        }
        let outcome = match &pc.command {
            Command::Run { units, goal, queue } => {
                apply_run(state, pc.player, &canonical_units(units), *goal, *queue)
            }
            Command::Attack {
                units,
                target,
                queue,
            } => apply_attack(state, pc.player, &canonical_units(units), *target, *queue),
            Command::Hunt { units, goal, queue } => {
                apply_hunt(state, pc.player, &canonical_units(units), *goal, *queue)
            }
            Command::Harvest { units, node, queue } => {
                apply_harvest(state, pc.player, &canonical_units(units), *node, *queue)
            }
            Command::ReturnCargo {
                units,
                foundry,
                repair,
            } => apply_return_cargo(state, pc.player, &canonical_units(units), *foundry, *repair),
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
                BuildPlacement {
                    kind: *kind,
                    anchor: *anchor,
                    defer: *defer,
                },
                *queue,
                events,
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
            Command::Surrender => {
                apply_surrender(state, pc.player, events);
                Ok(())
            }
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
            Command::ClearFocus { buildings } => {
                let ids = canonical_buildings(buildings);
                if ids.is_empty()
                    || ids.iter().any(|id| {
                        state.building(*id).is_none_or(|b| {
                            b.player != pc.player || !b.built || b.stats().weapons.is_empty()
                        })
                    })
                {
                    Err(RejectReason::NotYourBuilding)
                } else {
                    for id in ids {
                        state.building_mut(id).expect("validated").focus = None;
                    }
                    Ok(())
                }
            }
            Command::CancelFound { kind, anchor } => {
                apply_cancel_found(state, pc.player, *kind, *anchor, events)
            }
            Command::UpgradeBuilding { building } => apply_upgrade(state, pc.player, *building),
            Command::Load {
                units,
                transport,
                queue,
            } => apply_load(
                state,
                pc.player,
                &canonical_units(units),
                *transport,
                *queue,
            ),
            Command::Unload {
                transport,
                at,
                queue,
            } => apply_unload(state, pc.player, *transport, *at, *queue),
            Command::CancelOrder {
                unit,
                key,
                from_end,
                units,
            } => apply_cancel_order(
                state,
                pc.player,
                *unit,
                *key,
                *from_end,
                &canonical_units(units),
            ),
        };
        if let Err(reason) = outcome {
            events.push(Event::CommandRejected {
                player: pc.player,
                reason,
            });
        } else {
            super::construction::cancel_abandoned(state, events);
        }
    }
}

/// A command's unit list read as the SET it means: id-ordered, each id
/// once. Every unit-bearing command passes through here at dispatch, so no
/// handler can double-apply a repeated id. Ownership filtering stays in the
/// handlers, whose `NoValidUnits` reporting also weighs what a unit can do.
///
/// The recorded command keeps the client's bytes; this is how the sim
/// interprets a list, not a rewrite of it.
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

/// [`for_owned_units`] narrowed to the gathering crew — Harvest and
/// Salvage address machines with cargo gear, and each reports
/// `NoValidUnits` when the selection holds none.
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

/// [`for_owned_units`] narrowed to the welding crew: the Repair verbs
/// take anyone carrying a torch — harvesters, Excavators, and the
/// Tender alike.
fn for_owned_welders(
    state: &mut State,
    player: PlayerId,
    ids: &[UnitId],
    mut f: impl FnMut(&mut crate::state::Unit),
) -> usize {
    let mut applied = 0;
    for &id in ids {
        if let Some(unit) = state.unit_mut(id)
            && unit.player == player
            && unit.kind.stats().welder
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

/// Ends any tether a self-acquired fight put on this machine, restarts
/// station keeping, and clears a pending danger-hold retry so the new order
/// is judged at once. Every verb that writes a unit's program ([`assign`],
/// [`assign_circuit`], [`apply_stop`]) calls this, before `assign`'s no-op
/// early return: re-ordering the exact attack the unit already picked
/// itself compares equal and would otherwise keep the leash on an explicit
/// commitment. An append refused by a full queue never reaches it, so a
/// rejected order leaves the unit untouched. A new program-writing verb must
/// call this too.
fn end_station_keeping(unit: &mut crate::state::Unit) {
    unit.leash = None;
    unit.settled = 0;
    unit.danger_retry_at = None;
}

/// Drops the active leg without rotating it into a looping program. This is
/// the edit operation for one explicitly cancelled order, not ordinary order
/// completion.
fn remove_active_order(unit: &mut crate::state::Unit) {
    end_station_keeping(unit);
    unit.drop_active_order();
}

/// Hands a unit its next order: replacing wipes any queued program;
/// appending parks the order behind the current one (bounded, so a hostile
/// stream of appends cannot grow memory forever). Returns whether the
/// order landed; a full queue drops the append, and the caller reports it.
fn assign(unit: &mut crate::state::Unit, order: Order, queue: bool) -> bool {
    if queue && !matches!(unit.order, Order::Idle) {
        if unit.queue.len() >= ORDER_QUEUE_CAP {
            return false;
        }
        end_station_keeping(unit);
        unit.queue.push_back(order);
        return true;
    }
    end_station_keeping(unit);
    if !queue {
        unit.queue.clear();
        unit.looping = false;
        // Reissuing the current order continues it past the queue wipe:
        // progress and path survive. Resetting them would let a
        // re-commanded welder heal without ever crossing a billing tick,
        // drop a re-clicked harvester's half-extracted scrap, and discard
        // valid paths on every army re-push. A walk matches on its clicked
        // tile and takes the new aim; a path to a superseded destination is
        // replanned by the walk itself.
        if unit.order.reissue_matches(&order) {
            unit.order.reissue(order);
            return true;
        }
    }
    unit.unloading = None;
    unit.order = order;
    unit.path = None;
    unit.progress = 0;
    true
}

/// Hands a unit a whole looping circuit (patrol's shape): first leg
/// active, the rest queued, path and progress reset. The caller validates
/// the route; `legs` must be non-empty.
fn assign_circuit(unit: &mut crate::state::Unit, mut legs: impl Iterator<Item = Order>) {
    end_station_keeping(unit);
    unit.unloading = None;
    unit.order = legs.next().expect("caller validated a non-empty route");
    unit.queue = legs.collect();
    unit.looping = true;
    unit.path = None;
    unit.progress = 0;
}

/// Splits accepted unit ids by movement domain, preserving id order —
/// each half gets goals its own domain can actually stand on.
fn split_domains(state: &State, ids: Vec<UnitId>) -> [(Vec<UnitId>, Domain); 2] {
    let (ground, air): (Vec<UnitId>, Vec<UnitId>) = ids.into_iter().partition(|&id| {
        state.unit(id).expect("caller filtered").kind.stats().domain == Domain::Ground
    });
    [(ground, Domain::Ground), (air, Domain::Air)]
}

/// Sends a group toward one clicked tile: each movement domain's half takes
/// its spread slots around it (see [`goals`]), and `order_for` picks the
/// order each unit takes there.
fn apply_group_goal(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    goal: TilePos,
    queue: bool,
    order_for: impl Fn(&Unit, Goal) -> Order,
) -> Result<(), RejectReason> {
    if !on_map(state, goal) {
        return Err(RejectReason::OutOfBounds);
    }
    let accepted = accepted_units(state, player, units);
    if accepted.is_empty() {
        return Err(RejectReason::NoValidUnits);
    }
    let mut landed = 0;
    for (ids, domain) in split_domains(state, accepted) {
        if ids.is_empty() {
            continue;
        }
        let reverse = spread_scan_reversed(state, goal, &ids);
        let issue = goals::issue(state, player, goal, domain, reverse);
        for (rank, id) in ids.into_iter().enumerate() {
            let unit = state.unit_mut(id).expect("filtered above");
            let order = order_for(unit, issue.goal(rank));
            if assign(unit, order, queue) {
                landed += 1;
            }
        }
    }
    (landed > 0).then_some(()).ok_or(RejectReason::QueueFull)
}

fn apply_run(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    goal: TilePos,
    queue: bool,
) -> Result<(), RejectReason> {
    apply_group_goal(state, player, units, goal, queue, |_, goal| Order::Run {
        goal,
    })
}

fn apply_attack(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    target: AttackTarget,
    queue: bool,
) -> Result<(), RejectReason> {
    let target = state
        .attack_objective(player, target)
        .ok_or(RejectReason::InvalidTarget)?;
    let view = state
        .attack_view(player, target)
        .ok_or(RejectReason::InvalidTarget)?;
    let target_tile = chassis::grid::TilePos::containing(view.position);
    let victim_domain = view.domain;
    // A demolition machine carries no gun, but its charge covers ground the
    // same way a weapon would.
    let covers = |stats: &crate::stats::UnitStats| {
        victim_domain.map_or(stats.can_fight(), |domain| {
            stats.can_target(domain) || (stats.demolition.is_some() && domain == Domain::Ground)
        })
    };
    // Machines that cannot hit the target walk to its tile instead, each
    // movement domain sharing the one tile its first slot names.
    let walkers = accepted_units(state, player, units)
        .into_iter()
        .filter(|id| !covers(state.unit(*id).expect("accepted").kind.stats()))
        .collect();
    let walk_goals = split_domains(state, walkers).map(|(ids, domain)| {
        (!ids.is_empty()).then(|| {
            let reverse = spread_scan_reversed(state, target_tile, &ids);
            goals::issue(state, player, target_tile, domain, reverse).goal(0)
        })
    });
    let mut landed = 0;
    let applied = for_owned_units(state, player, units, |u| {
        let stats = u.kind.stats();
        if covers(stats) {
            if assign(
                u,
                Order::Attack {
                    target,
                    resume: None,
                    pursue: true,
                },
                queue,
            ) {
                landed += 1;
            }
        } else if let Some(goal) = walk_goals[usize::from(stats.domain == Domain::Air)]
            && assign(u, Order::Run { goal }, queue)
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

fn apply_hunt(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    goal: TilePos,
    queue: bool,
) -> Result<(), RejectReason> {
    apply_group_goal(state, player, units, goal, queue, |unit, goal| {
        if unit.kind.stats().can_fight() {
            Order::Hunt { goal }
        } else {
            Order::Run { goal }
        }
    })
}

fn apply_advance(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    goal: TilePos,
    queue: bool,
) -> Result<(), RejectReason> {
    apply_group_goal(state, player, units, goal, queue, |unit, goal| {
        if unit.kind.stats().can_fight() {
            Order::Advance { goal }
        } else {
            Order::Run { goal }
        }
    })
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
    // A source counts if it is visible now or the issuer remembers it.
    // Ordering harvesters onto stale memory is legitimate play (they walk
    // over, discover the truth, and retarget), while never-seen salvage
    // must not become a command-success oracle through fog.
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
                anchor: node,
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

/// Walk a looping circuit as one program: each leg spreads the group over
/// its waypoint like a group move. Combat units hunt each leg; unarmed
/// units walk them.
fn apply_patrol(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    waypoints: &[TilePos],
) -> Result<(), RejectReason> {
    if waypoints.is_empty() || waypoints.len() > ORDER_QUEUE_CAP {
        return Err(RejectReason::UnreachableGoal);
    }
    if waypoints.iter().any(|w| !on_map(state, *w)) {
        return Err(RejectReason::OutOfBounds);
    }
    let accepted = accepted_units(state, player, units);
    if accepted.is_empty() {
        return Err(RejectReason::NoValidUnits);
    }
    for (ids, domain) in split_domains(state, accepted) {
        if ids.is_empty() {
            continue;
        }
        let legs: Vec<_> = waypoints
            .iter()
            .map(|&waypoint| {
                let reverse = spread_scan_reversed(state, waypoint, &ids);
                goals::issue(state, player, waypoint, domain, reverse)
            })
            .collect();
        for (rank, id) in ids.into_iter().enumerate() {
            let unit = state.unit_mut(id).expect("filtered above");
            let can_fight = unit.kind.stats().can_fight();
            let legs = legs.iter().map(|leg| {
                let goal = leg.goal(rank);
                if can_fight {
                    Order::Hunt { goal }
                } else {
                    Order::Run { goal }
                }
            });
            assign_circuit(unit, legs);
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct BuildPlacement {
    kind: crate::stats::BuildingKind,
    anchor: TilePos,
    defer: bool,
}

/// Claims the site immediately (full price, footprint blocks) and commits
/// the whole accepted crew: the first accepted harvester founds the site
/// (pays, proves a doorstep is reachable) and every other accepted
/// harvester takes the same Build order (builders stack). Aiming at an
/// existing own unfinished site resumes it instead. With `defer`,
/// a paid provisional scaffold reserves the plan without occupying hidden
/// ground. The crew takes [`Order::Found`] until visibility verifies the site.
fn apply_build(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    placement: BuildPlacement,
    queue: bool,
    events: &mut Vec<Event>,
) -> Result<(), RejectReason> {
    let BuildPlacement {
        kind,
        anchor,
        defer,
    } = placement;
    let replaced = if queue {
        Vec::new()
    } else {
        super::construction::replaced_sites(state, player, units)
            .into_iter()
            .filter(|id| {
                state
                    .building(*id)
                    .is_some_and(|b| b.kind != kind || b.anchor != anchor)
            })
            .collect::<Vec<_>>()
    };
    if replaced.is_empty() {
        return apply_build_inner(state, player, units, kind, anchor, queue, defer);
    }
    let mut candidate = state.clone();
    let mut refunds = Vec::new();
    for id in replaced {
        super::construction::refund(&mut candidate, id, &mut refunds);
    }
    apply_build_inner(&mut candidate, player, units, kind, anchor, queue, defer)?;
    *state = candidate;
    events.extend(refunds);
    Ok(())
}

fn apply_build_inner(
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
    // founder.
    let crew: Vec<UnitId> = accepted_units(state, player, units)
        .into_iter()
        .filter(|id| {
            state
                .unit(*id)
                .is_some_and(|u| u.kind.stats().harvest.is_some())
        })
        .collect();
    let &builder = crew.first().ok_or(RejectReason::NoValidUnits)?;

    // Resume an existing site of ours at this anchor: every accepted
    // builder joins.
    let existing = state
        .buildings
        .iter()
        .find(|b| {
            b.anchor == anchor && b.kind == kind && b.player == player && !b.built && b.tier == 0
        })
        .map(|b| b.id);
    if let Some(site) = existing {
        let provisional = state.building(site).expect("found site").provisional;
        let mut landed = 0;
        for id in crew {
            if let Some(unit) = state.unit_mut(id)
                && assign(
                    unit,
                    if provisional {
                        Order::Found { kind, anchor }
                    } else {
                        Order::Build { site }
                    },
                    queue,
                )
            {
                landed += 1;
            }
        }
        return (landed > 0).then_some(()).ok_or(RejectReason::QueueFull);
    }
    // Tech gating answers before site questions, with its own reason.
    // Resuming an existing site above skips this: losing the Fabricator
    // does not orphan work already claimed.
    if !state.prerequisites_met(player, kind) {
        return Err(RejectReason::MissingPrerequisite);
    }
    if defer {
        // Hidden occupancy cannot affect acceptance or payment. Physical
        // validation waits until the entire footprint is visible.
        let replaced_units = if queue { &[][..] } else { crew.as_slice() };
        if state
            .place_intent_refusal_replacing(player, kind, anchor, replaced_units)
            .is_some()
        {
            return Err(RejectReason::BadSite);
        }
        let cost = kind
            .base_stats()
            .construction
            .ok_or(RejectReason::BadSite)?
            .cost;
        if state.player(player).scrap < cost {
            return Err(RejectReason::NotEnoughScrap);
        }
        if !crew.iter().any(|id| {
            state.unit(*id).is_some_and(|unit| {
                !queue || unit.order == Order::Idle || unit.queue.len() < ORDER_QUEUE_CAP
            })
        }) {
            return Err(RejectReason::QueueFull);
        }
        state.place_provisional_site(player, kind, anchor);
        state.player_mut(player).scrap -= cost;
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
        // Assign before paying: a founder whose order queue is full
        // must reject the whole command with the site retracted and
        // nothing spent.
        let unit = state.unit_mut(builder).expect("filtered above");
        assign(unit, Order::Build { site }, queue)
    })?;
    // The rest of the crew joins best-effort, in id order: a full queue
    // drops that builder, never the command; the founder alone gates
    // acceptance.
    for &id in crew.iter().skip(1) {
        if let Some(unit) = state.unit_mut(id) {
            let _ = assign(unit, Order::Build { site }, queue);
        }
    }
    Ok(())
}

/// Creates an immediate paid site only after its builder can accept and reach it.
fn found_site(
    state: &mut State,
    player: PlayerId,
    builder: UnitId,
    kind: crate::stats::BuildingKind,
    anchor: TilePos,
    commit_builder: impl FnOnce(&mut State, crate::ids::BuildingId) -> bool,
) -> Result<crate::ids::BuildingId, RejectReason> {
    let cost = kind
        .base_stats()
        .construction
        .ok_or(RejectReason::BadSite)?
        .cost;
    if state.player(player).scrap < cost {
        return Err(RejectReason::NotEnoughScrap);
    }
    // Place first, then prove the founder can reach a doorstep around the
    // now-blocking footprint; otherwise undo. Unreachable placement must
    // leave the command rejected without spending scrap or consuming a
    // building id.
    let site = state.place_site(player, kind, anchor);
    let from = state.unit(builder).expect("caller checked").tile();
    let size = kind.size();
    // An enclosed founder uses the same post-acceptance perimeter relocation
    // as other friendly bodies trapped by a newly claimed footprint.
    let inside = state.building(site).expect("just placed").contains(from);
    let reachable = super::rect_adjacent_tiles(anchor, size)
        .filter(|&t| state.passable(t))
        .any(|t| inside || from == t || super::astar_for(state, from, t).is_some());
    if !reachable {
        state.retract_site(site);
        return Err(RejectReason::UnreachableGoal);
    }
    if !commit_builder(state, site) {
        state.retract_site(site);
        return Err(RejectReason::QueueFull);
    }
    state.player_mut(player).scrap -= cost;
    finish_site_claim(state, site, builder);
    Ok(site)
}

pub(super) fn finish_site_claim(state: &mut State, site: BuildingId, builder: UnitId) {
    let building = state.building(site).expect("accepted site");
    let (player, anchor, size) = (building.player, building.anchor, building.kind.size());
    let from = state.unit(builder).expect("committed builder").tile();
    // Friendly machines make way as the site claims the ground: nothing may
    // end up inside a finished building. The builders' own approach and the
    // eviction pre-pass both route out of the footprint, so only a body with
    // no escape route is placed instantly onto the passable perimeter ring,
    // ordered in the founder's approach frame and dealt round-robin in id
    // order. This runs strictly after the last rejection path and the
    // payment, because a rejected command must not move the state hash
    // (`retract_site`'s contract). Hostiles cannot be here: the caller's
    // placement predicate refused them.
    let ring: Vec<TilePos> = {
        let approach = super::rect_approach_origin(state, player, from, anchor, size);
        let mut ring: Vec<TilePos> = super::rect_adjacent_tiles(anchor, size)
            .filter(|&t| state.passable(t))
            .collect();
        ring.sort_unstable_by_key(|&t| {
            (
                super::rect_approach_key_from(from, approach, anchor, size, t),
                t.y,
                t.x,
            )
        });
        ring
    };
    let inside = |t: TilePos| {
        t.x >= anchor.x && t.x < anchor.x + size.0 && t.y >= anchor.y && t.y < anchor.y + size.1
    };
    let mut dealt = 0usize;
    for i in 0..state.units.len() {
        let u = &state.units[i];
        if u.hp == 0 || u.domain() != crate::stats::Domain::Ground || !inside(u.tile()) {
            continue;
        }
        let (unit_kind, tile) = (u.kind, u.tile());
        if super::movement::escape_route(state, unit_kind, tile, u.heading).is_some() {
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
}

/// Refund an unstarted site in full; after work starts, scale by current hp.
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
        // An upgrading works (tier already lifted, offline) is committed:
        // cancelling would demolish a standing machine for its site
        // refund. Only fresh tier-zero sites can be scrapped.
        if b.tier > 0 {
            return Err(RejectReason::InvalidTarget);
        }
        let stats = b.stats();
        let cost = stats.construction.expect("sites are buildable kinds").cost;
        if b.progress == 0 {
            cost
        } else {
            cost * b.hp / stats.max_hp
        }
    };
    cancel_site(state, player, building, refund, events);
    Ok(())
}

pub(super) fn cancel_site(
    state: &mut State,
    player: PlayerId,
    building: crate::ids::BuildingId,
    refund: u32,
    events: &mut Vec<Event>,
) {
    let bank = &mut state.player_mut(player).scrap;
    *bank = bank.saturating_add(refund);
    if let Some(index) = state.buildings.iter().position(|b| b.id == building) {
        state.stamp_building_occupancy(index, false);
    }
    crate::vision::forget_observed_building(state, building, false);
    clear_site_orders(state, player, building);
    state.buildings.retain(|b| b.id != building);
    events.push(Event::BuildCancelled {
        building,
        player,
        refund,
    });
}

pub(super) fn clear_site_orders(
    state: &mut State,
    player: PlayerId,
    building: crate::ids::BuildingId,
) {
    let claim = state.building(building).map(|b| (b.kind, b.anchor));
    let matches_site = |order: &Order| {
        matches!(order, Order::Build { site } if *site == building)
            || matches!(order, Order::Found { kind, anchor } if Some((*kind, *anchor)) == claim)
    };
    for unit in state.units.iter_mut().filter(|unit| unit.player == player) {
        unit.queue.retain(|order| !matches_site(order));
        if matches_site(&unit.order) {
            remove_active_order(unit);
        }
    }
}

fn apply_return_cargo(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    foundry: Option<BuildingId>,
    repair: bool,
) -> Result<(), RejectReason> {
    if repair && foundry.is_none() {
        return Err(RejectReason::InvalidTarget);
    }
    if let Some(id) = foundry {
        let building = state.building(id).ok_or(RejectReason::NotYourBuilding)?;
        if building.player != player {
            return Err(RejectReason::NotYourBuilding);
        }
        if !building.built || building.hp == 0 || !building.kind.is_drop_off() {
            return Err(RejectReason::InvalidTarget);
        }
    }
    let workers: Vec<_> = units
        .iter()
        .copied()
        .filter(|id| {
            state.unit(*id).is_some_and(|unit| {
                unit.player == player
                    && unit.hp > 0
                    && unit.kind.stats().harvest.is_some()
                    && unit.carrying > 0
            })
        })
        .collect();
    if workers.is_empty() {
        return Err(RejectReason::NoValidUnits);
    }
    let danger = crate::vision::GroundSalvageDanger::capture(state, player);
    let deliveries: Vec<_> = workers
        .into_iter()
        .filter_map(|id| {
            super::brain::return_cargo_destination(state, &danger, id, foundry)
                .map(|destination| (id, destination))
        })
        .collect();
    if deliveries.is_empty() {
        return Err(RejectReason::UnreachableGoal);
    }
    for (id, foundry) in deliveries {
        assign(
            state.unit_mut(id).expect("validated worker"),
            Order::ReturnCargo { foundry, repair },
            false,
        );
    }
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
    if !b.built || b.hp >= b.stats().max_hp {
        return Err(RejectReason::InvalidTarget);
    }
    let mut landed = 0;
    let applied = for_owned_welders(state, player, units, |unit| {
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
    // Evict only on a command that landed: a rejected command must leave
    // the world untouched. Running it after the assignment is safe because
    // the purge only matches the opposing verb.
    purge_opposing_verb(state, player, building, Verb::Salvage);
    Ok(())
}

/// Salvage is for standing, built, own, non-Foundry buildings. Unbuilt
/// sites use [`Command::Cancel`]'s instant refund, and a Foundry cannot be
/// salvaged.
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

/// Unit welding is for wounded, own ground bodies (including parked
/// airframes). Airborne patients are refused, since a welder cannot stand
/// where a flyer hovers; the healthy leave nothing to do, and the patient
/// never joins its own crew. No eviction rule: nothing else targets a
/// friendly unit, and the patient's own orders are untouched.
fn apply_repair_unit(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    target: UnitId,
    queue: bool,
) -> Result<(), RejectReason> {
    let t = state.unit(target).ok_or(RejectReason::InvalidTarget)?;
    let stats = t.kind.stats();
    // A parked airframe is a ground body a welder can reach; an airborne
    // one is not.
    if t.player != player || t.hp == 0 || t.hp >= stats.max_hp || t.domain() != Domain::Ground {
        return Err(RejectReason::InvalidTarget);
    }
    let crew: Vec<UnitId> = units.iter().copied().filter(|&id| id != target).collect();
    let mut landed = 0;
    let applied = for_owned_welders(state, player, &crew, |unit| {
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

/// Sends carriable ground machines to climb aboard an own transport.
/// Capacity is checked at the sling, not here, because contents change
/// while the boarders walk.
fn apply_load(
    state: &mut State,
    player: PlayerId,
    units: &[UnitId],
    transport: UnitId,
    queue: bool,
) -> Result<(), RejectReason> {
    let t = state.unit(transport).ok_or(RejectReason::InvalidTarget)?;
    if t.player != player || t.hp == 0 || t.kind.stats().transport_capacity == 0 {
        return Err(RejectReason::InvalidTarget);
    }
    let mut landed = 0;
    let mut applied = 0;
    for &id in units {
        if id == transport {
            continue;
        }
        if let Some(unit) = state.unit_mut(id)
            && unit.player == player
            && unit.hp > 0
            && unit.kind.stats().transport_size > 0
        {
            applied += 1;
            if assign(unit, Order::Board { transport }, queue) {
                landed += 1;
            }
        }
    }
    if applied == 0 {
        return Err(RejectReason::NoValidUnits);
    }
    (landed > 0).then_some(()).ok_or(RejectReason::QueueFull)
}

/// Flies a transport to a drop point and unloads there. An empty sling
/// still flies.
fn apply_unload(
    state: &mut State,
    player: PlayerId,
    transport: UnitId,
    at: TilePos,
    queue: bool,
) -> Result<(), RejectReason> {
    if !on_map(state, at) {
        return Err(RejectReason::OutOfBounds);
    }
    let t = state.unit(transport).ok_or(RejectReason::InvalidTarget)?;
    if t.player != player || t.hp == 0 || t.kind.stats().transport_capacity == 0 {
        return Err(RejectReason::InvalidTarget);
    }
    // The drop point snaps like a lone unit's move, so it names sky the
    // transport can occupy once the tile is explored.
    let domain = t.kind.stats().domain;
    let reverse = spread_scan_reversed(state, at, &[transport]);
    let at = goals::issue(state, player, at, domain, reverse).goal(0);
    let unit = state.unit_mut(transport).expect("just seen");
    if assign(unit, Order::Unload { at, reverse }, queue) {
        Ok(())
    } else {
        Err(RejectReason::QueueFull)
    }
}

/// The verb a repair/salvage command evicts from its target: the two never
/// share a building, or a welder and a salvager would make the resolver
/// oscillate.
#[derive(Clone, Copy)]
enum Verb {
    Repair,
    Salvage,
}

/// Clears every own unit's orders of the opposing verb on `building`:
/// queued legs are dropped, and a matching active order advances to its
/// next leg, so the rest of the program survives.
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
            // A looping program rotates the finished order to the back of
            // the queue just cleaned; strip it again, or a patrolling welder
            // brings the evicted job around forever.
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
        // The next in line starts fresh; progress does not transfer between
        // machines.
        b.progress = 0;
    }
    let bank = &mut state.player_mut(player).scrap;
    *bank = bank.saturating_add(kind.stats().cost);
    Ok(())
}

/// Where `unit`'s `from_end`-th match of `key`, counted back from the end
/// of its program, sits: 0 is the active order and `i` is `queue[i - 1]`.
fn program_match(
    state: &State,
    player: PlayerId,
    unit: &Unit,
    key: OrderKey,
    from_end: u8,
) -> Option<usize> {
    (0..=unit.queue.len())
        .rev()
        .filter(|&slot| {
            let order = if slot == 0 {
                &unit.order
            } else {
                &unit.queue[slot - 1]
            };
            order.key(state, player) == Some(key)
        })
        .nth(usize::from(from_end))
}

/// Removes one order from each unit's program. The subject is checked
/// before anything changes; every other unit edits only if it holds a
/// match.
fn apply_cancel_order(
    state: &mut State,
    player: PlayerId,
    subject: UnitId,
    key: OrderKey,
    from_end: u8,
    units: &[UnitId],
) -> Result<(), RejectReason> {
    let unit = state
        .unit(subject)
        .filter(|unit| unit.player == player)
        .ok_or(RejectReason::NoValidUnits)?;
    let slot =
        program_match(state, player, unit, key, from_end).ok_or(RejectReason::InvalidTarget)?;
    let mut edits = vec![(subject, slot)];
    edits.extend(units.iter().filter(|&&id| id != subject).filter_map(|&id| {
        let unit = state.unit(id).filter(|unit| unit.player == player)?;
        Some((id, program_match(state, player, unit, key, from_end)?))
    }));
    for (id, slot) in edits {
        let unit = state.unit_mut(id).expect("matched above");
        if slot == 0 {
            remove_active_order(unit);
        } else {
            end_station_keeping(unit);
            unit.queue.remove(slot - 1);
        }
    }
    Ok(())
}

pub(super) fn apply_cancel_found(
    state: &mut State,
    player: PlayerId,
    kind: crate::stats::BuildingKind,
    anchor: TilePos,
    events: &mut Vec<Event>,
) -> Result<(), RejectReason> {
    if let Some((id, started)) = state
        .buildings
        .iter()
        .find(|b| {
            b.player == player && b.kind == kind && b.anchor == anchor && !b.built && b.tier == 0
        })
        .map(|b| (b.id, b.progress > 0))
    {
        // A site placed by a command still in flight can be under way by
        // the time this lands; it then cancels like any started site.
        if started {
            return apply_cancel(state, player, id, events);
        }
        super::construction::refund(state, id, events);
        return Ok(());
    }
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
        if !b.built || !b.stats().produces.contains(&kind) {
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
        // Some machines require completed tech buildings beyond their
        // producer (the same rule for every command source).
        let met = kind.stats().requires.iter().all(|required| {
            state
                .buildings
                .iter()
                .any(|owned| owned.player == player && owned.kind == *required && owned.built)
        });
        if !met {
            return Err(RejectReason::MissingPrerequisite);
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

/// Concession sets one flag that the victory check and the command gate
/// both read; it does not raze the base. Commands apply before the tick's
/// victory check, so a decisive surrender ends the match on its own tick.
fn apply_surrender(state: &mut State, player: PlayerId, events: &mut Vec<Event>) {
    state.player_mut(player).resigned = true;
    events.push(Event::PlayerResigned { player });
}

fn apply_set_rally(
    state: &mut State,
    player: PlayerId,
    building: crate::ids::BuildingId,
    rally: Option<TilePos>,
) -> Result<(), RejectReason> {
    if let Some(rally) = rally
        && !on_map(state, rally)
    {
        return Err(RejectReason::OutOfBounds);
    }
    let b = state
        .building_mut(building)
        .ok_or(RejectReason::NotYourBuilding)?;
    if b.player != player {
        return Err(RejectReason::NotYourBuilding);
    }
    if !b.built || b.stats().produces.is_empty() {
        return Err(RejectReason::InvalidTarget);
    }
    // Any tile on the map is a legal rally: each newborn resolves it like a
    // move of its own, and a scrap-node rally requests auto-harvest.
    b.rally = rally;
    Ok(())
}

fn apply_focus_fire(
    state: &mut State,
    player: PlayerId,
    buildings: &[BuildingId],
    target: AttackTarget,
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
            .stats()
            .weapons
            .first()
            .copied()
            .ok_or(RejectReason::InvalidTarget)?;
        weapons.push(weapon);
    }

    let target = state
        .attack_objective(player, target)
        .ok_or(RejectReason::InvalidTarget)?;
    let view = state
        .attack_view(player, target)
        .ok_or(RejectReason::InvalidTarget)?;
    if view
        .domain
        .is_some_and(|domain| weapons.iter().any(|weapon| !weapon.targets.covers(domain)))
    {
        return Err(RejectReason::InvalidTarget);
    }

    for &id in buildings {
        state.building_mut(id).expect("validated above").focus = Some(target);
    }
    Ok(())
}

/// Lifts a completed own building one rung up its ladder: full price is
/// charged now and the works goes offline on the new tier's row. Its own
/// deterministic rebuild clock advances once per tick; no worker can pause
/// or accelerate it. There is no cancel once the upgrade is committed.
fn apply_upgrade(
    state: &mut State,
    player: PlayerId,
    building: BuildingId,
) -> Result<(), RejectReason> {
    let Some(b) = state.building(building) else {
        return Err(RejectReason::NotYourBuilding);
    };
    if b.player != player {
        return Err(RejectReason::NotYourBuilding);
    }
    if !b.built {
        return Err(RejectReason::InvalidTarget);
    }
    let kind = b.kind;
    let tier = b.tier;
    let Some(upgrade) = kind.upgrade_from(tier) else {
        return Err(RejectReason::InvalidTarget);
    };
    let met = upgrade.requires.iter().all(|required| {
        state
            .buildings
            .iter()
            .any(|owned| owned.player == player && owned.kind == *required && owned.built)
    });
    if !met {
        return Err(RejectReason::MissingPrerequisite);
    }
    if state.players[player.0 as usize].scrap < upgrade.cost {
        return Err(RejectReason::NotEnoughScrap);
    }
    state.players[player.0 as usize].scrap -= upgrade.cost;
    let b = state.building_mut(building).expect("validated above");
    let old_max = b.stats().max_hp;
    b.tier += 1;
    b.built = false;
    b.progress = 0;
    // The commitment re-founds the machine as a fresh site of the new
    // tier: hp restarts at the new tier's construction floor, scaled by
    // the previous tier's condition, so an undamaged input completes at
    // the new maximum and battle damage carries through the rebuild.
    // Retaining the previous hp would double-count it against the new ramp.
    let start_hp = b.stats().max_hp / 5;
    b.hp = (b.hp.saturating_mul(start_hp) / old_max.max(1)).max(1);
    // The offline site neither fires nor cools, so combat state resets: a
    // stale cooldown can exceed the new tier's ceiling, and a focus on an
    // unbuilt building fails validation.
    b.cooldown = 0;
    b.focus = None;
    // The salvage ledger is priced against the previous tier; the rebuilt
    // machine starts a clean one, and any crew mid-salvage finds an
    // unbuilt target next tick and stands down.
    b.salvage_drained = 0;
    b.salvage_credited = 0;
    Ok(())
}

#[cfg(test)]
mod tests;
